use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Instant;
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::miniprotocols::peersharing::{self, PeerAddress, PeerSharingMessage};
use crate::multiplexer::Segment;
use crate::peer_manager::PeerManager;
use crate::query_handler::QueryHandler;
use torsten_mempool::Mempool;

/// Callback trait for retrieving block data from storage
pub trait BlockProvider: Send + Sync + 'static {
    /// Get raw CBOR block bytes by header hash
    fn get_block(&self, hash: &[u8; 32]) -> Option<Vec<u8>>;
    /// Check if a block exists
    fn has_block(&self, hash: &[u8; 32]) -> bool;
    /// Get the current chain tip (slot, hash, block_number)
    fn get_tip(&self) -> (u64, [u8; 32], u64);
    /// Get the next block after a given slot.
    /// Returns (slot, hash, cbor) of the first block with slot > after_slot.
    fn get_next_block_after_slot(&self, after_slot: u64) -> Option<(u64, [u8; 32], Vec<u8>)>;
}

#[derive(Error, Debug)]
pub enum N2NServerError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Handshake failed: {0}")]
    HandshakeFailed(String),
    #[error("Protocol error: {0}")]
    Protocol(String),
}

/// N2N mini-protocol IDs
const MINI_PROTOCOL_HANDSHAKE: u16 = 0;
const MINI_PROTOCOL_CHAINSYNC: u16 = 2;
const MINI_PROTOCOL_BLOCKFETCH: u16 = 3;
const MINI_PROTOCOL_TXSUBMISSION: u16 = 4;
const MINI_PROTOCOL_KEEPALIVE: u16 = 8;
const MINI_PROTOCOL_PEERSHARING: u16 = 10;

/// Peer sharing mode for N2N handshake
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerSharingMode {
    /// No peer sharing
    NoPeerSharing = 0,
    /// Peer sharing enabled
    PeerSharingEnabled = 1,
}

/// Per-IP connection rate limiter to prevent DoS attacks.
/// Tracks connection timestamps per IP and enforces:
/// - Max connections per IP within a time window
/// - Cleanup of stale entries
struct ConnectionRateLimiter {
    /// Map of IP → list of connection timestamps
    attempts: std::sync::Mutex<HashMap<IpAddr, Vec<Instant>>>,
    /// Max connections allowed per IP within the window
    max_per_ip: usize,
    /// Time window for rate limiting
    window: std::time::Duration,
}

impl ConnectionRateLimiter {
    fn new(max_per_ip: usize, window: std::time::Duration) -> Self {
        ConnectionRateLimiter {
            attempts: std::sync::Mutex::new(HashMap::new()),
            max_per_ip,
            window,
        }
    }

    /// Check if a connection from this IP should be allowed.
    /// Returns true if allowed, false if rate-limited.
    fn check_and_record(&self, ip: IpAddr) -> bool {
        let now = Instant::now();
        let mut attempts = self.attempts.lock().unwrap();
        let timestamps = attempts.entry(ip).or_default();

        // Remove timestamps outside the window
        timestamps.retain(|t| now.duration_since(*t) < self.window);

        if timestamps.len() >= self.max_per_ip {
            false
        } else {
            timestamps.push(now);
            true
        }
    }

    /// Remove stale entries to prevent memory growth
    fn cleanup(&self) {
        let now = Instant::now();
        let mut attempts = self.attempts.lock().unwrap();
        attempts.retain(|_, timestamps| {
            timestamps.retain(|t| now.duration_since(*t) < self.window);
            !timestamps.is_empty()
        });
    }
}

/// Node-to-Node server that accepts inbound TCP connections from remote peers.
pub struct N2NServer {
    listen_addr: SocketAddr,
    network_magic: u64,
    query_handler: Arc<RwLock<QueryHandler>>,
    block_provider: Arc<dyn BlockProvider>,
    max_connections: usize,
    /// Whether this node operates in InitiatorAndResponder (bidirectional) mode
    pub initiator_and_responder: bool,
    /// Whether peer sharing is enabled
    pub peer_sharing: PeerSharingMode,
    /// Peer manager for sharing peers via PeerSharing protocol
    peer_manager: Option<Arc<RwLock<PeerManager>>>,
    /// Optional mempool for TxSubmission2 protocol
    mempool: Option<Arc<Mempool>>,
}

impl N2NServer {
    pub fn new(
        listen_addr: SocketAddr,
        network_magic: u64,
        query_handler: Arc<RwLock<QueryHandler>>,
        block_provider: Arc<dyn BlockProvider>,
        max_connections: usize,
    ) -> Self {
        N2NServer {
            listen_addr,
            network_magic,
            query_handler,
            block_provider,
            max_connections,
            initiator_and_responder: true,
            peer_sharing: PeerSharingMode::PeerSharingEnabled,
            peer_manager: None,
            mempool: None,
        }
    }

    /// Create with explicit diffusion mode and peer sharing settings
    pub fn with_config(
        listen_addr: SocketAddr,
        network_magic: u64,
        query_handler: Arc<RwLock<QueryHandler>>,
        block_provider: Arc<dyn BlockProvider>,
        max_connections: usize,
        initiator_and_responder: bool,
        peer_sharing: PeerSharingMode,
    ) -> Self {
        N2NServer {
            listen_addr,
            network_magic,
            query_handler,
            block_provider,
            max_connections,
            initiator_and_responder,
            peer_sharing,
            peer_manager: None,
            mempool: None,
        }
    }

    /// Set the mempool for TxSubmission2 protocol support
    pub fn set_mempool(&mut self, mempool: Arc<Mempool>) {
        self.mempool = Some(mempool);
    }

    /// Set the peer manager for PeerSharing protocol support
    pub fn set_peer_manager(&mut self, peer_manager: Arc<RwLock<PeerManager>>) {
        self.peer_manager = Some(peer_manager);
    }

    /// Start listening for inbound N2N connections.
    pub async fn listen(&self) -> Result<(), N2NServerError> {
        let listener = TcpListener::bind(self.listen_addr).await?;
        info!("N2N server listening on {}", self.listen_addr);

        let active_connections = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        // Rate limiter: max 10 connections per IP per 60 seconds
        let rate_limiter = Arc::new(ConnectionRateLimiter::new(
            10,
            std::time::Duration::from_secs(60),
        ));

        // Periodic cleanup of stale rate limiter entries
        let rl_cleanup = rate_limiter.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
            loop {
                interval.tick().await;
                rl_cleanup.cleanup();
            }
        });

        loop {
            match listener.accept().await {
                Ok((stream, peer_addr)) => {
                    // Rate limiting check
                    if !rate_limiter.check_and_record(peer_addr.ip()) {
                        warn!(
                            peer = %peer_addr,
                            "Rejecting connection: rate limit exceeded"
                        );
                        drop(stream);
                        continue;
                    }

                    let active = active_connections.load(std::sync::atomic::Ordering::Relaxed);
                    if active >= self.max_connections {
                        warn!(
                            peer = %peer_addr,
                            active,
                            max = self.max_connections,
                            "Rejecting connection: max connections reached"
                        );
                        drop(stream);
                        continue;
                    }

                    active_connections.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    info!(peer = %peer_addr, "N2N peer connected");

                    let query_handler = self.query_handler.clone();
                    let block_provider = self.block_provider.clone();
                    let network_magic = self.network_magic;
                    let counter = active_connections.clone();
                    let initiator_and_responder = self.initiator_and_responder;
                    let peer_sharing_mode = self.peer_sharing;
                    let peer_manager = self.peer_manager.clone();
                    let mempool = self.mempool.clone();

                    tokio::spawn(async move {
                        if let Err(e) = handle_n2n_connection(
                            stream,
                            peer_addr,
                            network_magic,
                            query_handler,
                            block_provider,
                            initiator_and_responder,
                            peer_sharing_mode,
                            peer_manager,
                            mempool,
                        )
                        .await
                        {
                            debug!(peer = %peer_addr, "N2N connection ended: {e}");
                        }
                        counter.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                        info!(peer = %peer_addr, "N2N peer disconnected");
                    });
                }
                Err(e) => {
                    error!("Failed to accept N2N connection: {e}");
                }
            }
        }
    }
}

/// Per-peer state for tracking ChainSync cursor and protocol state
struct PeerState {
    /// The peer's ChainSync cursor — the point they've synced to.
    /// When they call MsgFindIntersect, this gets set. On each MsgRequestNext,
    /// we advance from this point.
    chainsync_cursor_slot: Option<u64>,
    chainsync_cursor_hash: Option<[u8; 32]>,
    /// TxSubmission2 state: whether we've sent MsgInit
    tx_submission_init_sent: bool,
}

impl PeerState {
    fn new() -> Self {
        PeerState {
            chainsync_cursor_slot: None,
            chainsync_cursor_hash: None,
            tx_submission_init_sent: false,
        }
    }
}

/// Handle a single inbound N2N peer connection
#[allow(clippy::too_many_arguments)]
async fn handle_n2n_connection(
    mut stream: tokio::net::TcpStream,
    peer_addr: SocketAddr,
    network_magic: u64,
    query_handler: Arc<RwLock<QueryHandler>>,
    block_provider: Arc<dyn BlockProvider>,
    initiator_and_responder: bool,
    peer_sharing_mode: PeerSharingMode,
    peer_manager: Option<Arc<RwLock<PeerManager>>>,
    mempool: Option<Arc<Mempool>>,
) -> Result<(), N2NServerError> {
    let mut buf = vec![0u8; 65536];
    let mut partial = Vec::new();
    let mut peer_state = PeerState::new();

    loop {
        let n = stream.read(&mut buf).await?;
        if n == 0 {
            return Ok(()); // Peer disconnected
        }

        partial.extend_from_slice(&buf[..n]);

        // Process all complete segments
        let mut offset = 0;
        while offset < partial.len() {
            let remaining = &partial[offset..];
            if remaining.len() < 8 {
                break;
            }

            match Segment::decode(remaining) {
                Ok((segment, consumed)) => {
                    offset += consumed;

                    let response = process_n2n_segment(
                        &segment,
                        peer_addr,
                        network_magic,
                        &query_handler,
                        &block_provider,
                        initiator_and_responder,
                        peer_sharing_mode,
                        &mut peer_state,
                        &peer_manager,
                        &mempool,
                    )
                    .await?;

                    for resp in response {
                        let encoded = resp.encode();
                        stream.write_all(&encoded).await?;
                    }
                }
                Err(_) => {
                    break; // Incomplete segment, wait for more data
                }
            }
        }

        // Keep any unprocessed data
        if offset > 0 {
            partial.drain(..offset);
        }
    }
}

/// Process a single N2N multiplexer segment
#[allow(clippy::too_many_arguments)]
async fn process_n2n_segment(
    segment: &Segment,
    peer_addr: SocketAddr,
    network_magic: u64,
    query_handler: &Arc<RwLock<QueryHandler>>,
    block_provider: &Arc<dyn BlockProvider>,
    initiator_and_responder: bool,
    peer_sharing_mode: PeerSharingMode,
    peer_state: &mut PeerState,
    peer_manager: &Option<Arc<RwLock<PeerManager>>>,
    mempool: &Option<Arc<Mempool>>,
) -> Result<Vec<Segment>, N2NServerError> {
    match segment.protocol_id {
        MINI_PROTOCOL_HANDSHAKE => {
            let resp = handle_n2n_handshake(
                &segment.payload,
                network_magic,
                initiator_and_responder,
                peer_sharing_mode,
            )?;
            Ok(resp.into_iter().collect())
        }
        MINI_PROTOCOL_CHAINSYNC => {
            let resp =
                handle_n2n_chainsync(&segment.payload, query_handler, block_provider, peer_state)
                    .await?;
            Ok(resp.into_iter().collect())
        }
        MINI_PROTOCOL_BLOCKFETCH => {
            let resp = handle_n2n_blockfetch(&segment.payload, block_provider)?;
            Ok(resp)
        }
        MINI_PROTOCOL_TXSUBMISSION => {
            let resp = handle_n2n_txsubmission(&segment.payload, peer_state, mempool)?;
            Ok(resp.into_iter().collect())
        }
        MINI_PROTOCOL_KEEPALIVE => {
            let resp = handle_keepalive(&segment.payload)?;
            Ok(resp.into_iter().collect())
        }
        MINI_PROTOCOL_PEERSHARING => {
            let resp =
                handle_peersharing(&segment.payload, peer_addr, peer_sharing_mode, peer_manager)
                    .await?;
            Ok(resp.into_iter().collect())
        }
        other => {
            debug!(peer = %peer_addr, protocol = other, "Unknown N2N mini-protocol");
            Ok(vec![])
        }
    }
}

/// Handle N2N version handshake.
///
/// N2N handshake format:
///   Client sends: [0, { version: params, ... }] (MsgProposeVersions)
///   Server responds: [1, version, params] (MsgAcceptVersion)
///   Or: [2, reason] (MsgRefuse)
fn handle_n2n_handshake(
    payload: &[u8],
    network_magic: u64,
    initiator_and_responder: bool,
    peer_sharing: PeerSharingMode,
) -> Result<Option<Segment>, N2NServerError> {
    let mut decoder = minicbor::Decoder::new(payload);

    // Parse [tag, versions_map]
    let _arr_len = decoder
        .array()
        .map_err(|e| N2NServerError::HandshakeFailed(e.to_string()))?;
    let msg_tag = decoder
        .u32()
        .map_err(|e| N2NServerError::HandshakeFailed(e.to_string()))?;

    if msg_tag != 0 {
        return Err(N2NServerError::HandshakeFailed(format!(
            "Expected MsgProposeVersions (0), got {msg_tag}"
        )));
    }

    // Parse version map to find the highest version we support
    // N2N versions: 7-14 (Shelley through Conway)
    // We support versions 13-14 (Babbage/Conway)
    let mut best_version: Option<u32> = None;
    let map_len = decoder
        .map()
        .map_err(|e| N2NServerError::HandshakeFailed(e.to_string()))?;

    let count = map_len.unwrap_or(0);
    for _ in 0..count {
        let version = decoder
            .u32()
            .map_err(|e| N2NServerError::HandshakeFailed(e.to_string()))?;
        // Skip the value (params)
        decoder
            .skip()
            .map_err(|e| N2NServerError::HandshakeFailed(e.to_string()))?;

        // Accept versions 13-14 (Babbage and Conway N2N)
        if (13..=14).contains(&version)
            && (best_version.is_none() || version > best_version.unwrap())
        {
            best_version = Some(version);
        }
    }

    let version = match best_version {
        Some(v) => v,
        None => {
            // Refuse: no compatible version
            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);
            enc.array(2)
                .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
            enc.u32(2)
                .map_err(|e| N2NServerError::Protocol(e.to_string()))?; // MsgRefuse
            enc.array(2)
                .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
            enc.u32(0)
                .map_err(|e| N2NServerError::Protocol(e.to_string()))?; // VersionMismatch
            enc.array(0)
                .map_err(|e| N2NServerError::Protocol(e.to_string()))?; // empty list

            return Ok(Some(Segment {
                transmission_time: 0,
                protocol_id: MINI_PROTOCOL_HANDSHAKE,
                is_responder: true,
                payload: buf,
            }));
        }
    };

    debug!("N2N handshake: accepting version {version}, magic {network_magic}");

    // Encode MsgAcceptVersion: [1, version, params]
    // N2N V13+ params: [network_magic, initiator_only_diffusion_mode, peer_sharing, query]
    let mut buf = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut buf);
    enc.array(3)
        .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
    enc.u32(1)
        .map_err(|e| N2NServerError::Protocol(e.to_string()))?; // MsgAcceptVersion
    enc.u32(version)
        .map_err(|e| N2NServerError::Protocol(e.to_string()))?;

    // Version params: [magic, initiator_only_diffusion_mode, peer_sharing, query]
    // initiator_only_diffusion_mode: true = unidirectional, false = bidirectional
    let initiator_only = !initiator_and_responder;
    enc.array(4)
        .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
    enc.u64(network_magic)
        .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
    enc.bool(initiator_only)
        .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
    enc.u32(peer_sharing as u32)
        .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
    enc.bool(false)
        .map_err(|e| N2NServerError::Protocol(e.to_string()))?; // query = false

    Ok(Some(Segment {
        transmission_time: 0,
        protocol_id: MINI_PROTOCOL_HANDSHAKE,
        is_responder: true,
        payload: buf,
    }))
}

/// Handle N2N ChainSync mini-protocol messages.
///
/// As a server (responder), we respond to:
///   MsgRequestNext (0) → MsgRollForward (2) or MsgRollBackward (3) or MsgAwaitReply (1)
///   MsgFindIntersect (4) → MsgIntersectFound (5) or MsgIntersectNotFound (6)
///   MsgDone (7) → close protocol
///
/// Per-peer cursor tracking: after MsgFindIntersect sets the cursor, each
/// MsgRequestNext checks if a new block exists beyond the cursor slot. If so,
/// we respond with MsgRollForward carrying the raw block header; otherwise
/// we send MsgAwaitReply.
async fn handle_n2n_chainsync(
    payload: &[u8],
    _query_handler: &Arc<RwLock<QueryHandler>>,
    block_provider: &Arc<dyn BlockProvider>,
    peer_state: &mut PeerState,
) -> Result<Option<Segment>, N2NServerError> {
    let mut decoder = minicbor::Decoder::new(payload);
    let _arr_len = decoder
        .array()
        .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
    let msg_tag = decoder
        .u32()
        .map_err(|e| N2NServerError::Protocol(e.to_string()))?;

    match msg_tag {
        // MsgRequestNext → check if there's a block beyond the peer's cursor
        0 => {
            let (tip_slot, tip_hash, tip_block) = block_provider.get_tip();

            // If no cursor set or cursor is at tip, await
            let cursor_slot = peer_state.chainsync_cursor_slot.unwrap_or(0);
            if cursor_slot >= tip_slot {
                // At tip — send MsgAwaitReply
                let mut buf = Vec::new();
                let mut enc = minicbor::Encoder::new(&mut buf);
                enc.array(1)
                    .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
                enc.u32(1)
                    .map_err(|e| N2NServerError::Protocol(e.to_string()))?;

                return Ok(Some(Segment {
                    transmission_time: 0,
                    protocol_id: MINI_PROTOCOL_CHAINSYNC,
                    is_responder: true,
                    payload: buf,
                }));
            }

            // Serve the next block sequentially after the cursor
            if let Some((next_slot, next_hash, block_cbor)) =
                block_provider.get_next_block_after_slot(cursor_slot)
            {
                // Update cursor to this block
                peer_state.chainsync_cursor_slot = Some(next_slot);
                peer_state.chainsync_cursor_hash = Some(next_hash);

                // MsgRollForward: [2, wrapped_header, tip]
                // N2N chainsync sends headers, not full blocks.
                // The header is wrapped: [era_tag, [variant, cbor_header]]
                // For simplicity, we send the raw block CBOR as the header
                // (the peer will parse it).
                let mut buf = Vec::new();
                let mut enc = minicbor::Encoder::new(&mut buf);
                enc.array(3)
                    .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
                enc.u32(2)
                    .map_err(|e| N2NServerError::Protocol(e.to_string()))?;

                // Wrapped header: tag(24) bytes(header_cbor)
                // For N2N ChainSync, headers are CBOR-wrapped
                enc.tag(minicbor::data::Tag::new(24))
                    .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
                enc.bytes(&block_cbor)
                    .map_err(|e| N2NServerError::Protocol(e.to_string()))?;

                // Tip: [point, block_number]
                encode_tip(&mut enc, tip_slot, &tip_hash, tip_block)?;

                Ok(Some(Segment {
                    transmission_time: 0,
                    protocol_id: MINI_PROTOCOL_CHAINSYNC,
                    is_responder: true,
                    payload: buf,
                }))
            } else {
                // Block not found, await
                let mut buf = Vec::new();
                let mut enc = minicbor::Encoder::new(&mut buf);
                enc.array(1)
                    .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
                enc.u32(1)
                    .map_err(|e| N2NServerError::Protocol(e.to_string()))?;

                Ok(Some(Segment {
                    transmission_time: 0,
                    protocol_id: MINI_PROTOCOL_CHAINSYNC,
                    is_responder: true,
                    payload: buf,
                }))
            }
        }
        // MsgFindIntersect → search for an intersection point with the peer's chain
        4 => {
            // Parse the list of points the peer sends
            let points_len = decoder
                .array()
                .map_err(|e| N2NServerError::Protocol(e.to_string()))?
                .unwrap_or(0);

            let mut intersect_point: Option<(u64, [u8; 32])> = None;
            for _ in 0..points_len {
                if let Some((slot, hash)) = parse_point_slot_hash(&mut decoder) {
                    // Check if we have this block
                    if block_provider.has_block(&hash) {
                        // Found intersection — use the highest slot
                        if intersect_point.is_none() || slot > intersect_point.as_ref().unwrap().0 {
                            intersect_point = Some((slot, hash));
                        }
                    }
                }
            }

            let (tip_slot, tip_hash, tip_block) = block_provider.get_tip();
            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);

            if let Some((int_slot, int_hash)) = intersect_point {
                // Set peer cursor to intersection
                peer_state.chainsync_cursor_slot = Some(int_slot);
                peer_state.chainsync_cursor_hash = Some(int_hash);

                // MsgIntersectFound: [5, point, tip]
                enc.array(3)
                    .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
                enc.u32(5)
                    .map_err(|e| N2NServerError::Protocol(e.to_string()))?;

                // Intersection point
                encode_point(&mut enc, int_slot, &int_hash)?;
                // Tip
                encode_tip(&mut enc, tip_slot, &tip_hash, tip_block)?;
            } else {
                // MsgIntersectNotFound: [6, tip]
                enc.array(2)
                    .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
                enc.u32(6)
                    .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
                encode_tip(&mut enc, tip_slot, &tip_hash, tip_block)?;
            }

            Ok(Some(Segment {
                transmission_time: 0,
                protocol_id: MINI_PROTOCOL_CHAINSYNC,
                is_responder: true,
                payload: buf,
            }))
        }
        // MsgDone
        7 => {
            debug!("N2N ChainSync: peer sent MsgDone");
            Ok(None)
        }
        other => {
            warn!("N2N ChainSync: unknown message tag {other}");
            Ok(None)
        }
    }
}

/// Encode a CBOR point: [slot, hash]
fn encode_point(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    slot: u64,
    hash: &[u8; 32],
) -> Result<(), N2NServerError> {
    enc.array(2)
        .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
    enc.u64(slot)
        .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
    enc.bytes(hash)
        .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
    Ok(())
}

/// Encode a CBOR tip: [[slot, hash], block_number]
fn encode_tip(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    slot: u64,
    hash: &[u8; 32],
    block_no: u64,
) -> Result<(), N2NServerError> {
    enc.array(2)
        .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
    encode_point(enc, slot, hash)?;
    enc.u64(block_no)
        .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
    Ok(())
}

/// Handle N2N BlockFetch mini-protocol messages.
///
///   MsgRequestRange (0) [from_point, to_point] → MsgStartBatch (2) + blocks + MsgBatchDone (5)
///     or MsgNoBlocks (3) if range cannot be served
///   MsgClientDone (1) → close protocol
fn handle_n2n_blockfetch(
    payload: &[u8],
    block_provider: &Arc<dyn BlockProvider>,
) -> Result<Vec<Segment>, N2NServerError> {
    let mut decoder = minicbor::Decoder::new(payload);
    let _arr_len = decoder
        .array()
        .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
    let msg_tag = decoder
        .u32()
        .map_err(|e| N2NServerError::Protocol(e.to_string()))?;

    match msg_tag {
        // MsgRequestRange: [0, from_point, to_point]
        0 => {
            // Parse from_point [slot, hash] and to_point [slot, hash]
            let from = parse_point_slot_hash(&mut decoder);
            let to = parse_point_slot_hash(&mut decoder);

            let mut segments = Vec::new();

            // Check that we have at least the boundary blocks
            let from_exists = from
                .as_ref()
                .map(|(_, h)| block_provider.has_block(h))
                .unwrap_or(false);
            let to_exists = to
                .as_ref()
                .map(|(_, h)| block_provider.has_block(h))
                .unwrap_or(false);

            if !from_exists || !to_exists {
                // MsgNoBlocks: [2]
                let mut buf = Vec::new();
                let mut enc = minicbor::Encoder::new(&mut buf);
                enc.array(1)
                    .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
                enc.u32(2)
                    .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
                segments.push(Segment {
                    transmission_time: 0,
                    protocol_id: MINI_PROTOCOL_BLOCKFETCH,
                    is_responder: true,
                    payload: buf,
                });
                return Ok(segments);
            }

            let (from_slot, _from_hash) = from.unwrap();
            let (to_slot, _to_hash) = to.unwrap();

            // MsgStartBatch: [2]
            let mut start_buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut start_buf);
            enc.array(1)
                .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
            enc.u32(2)
                .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
            segments.push(Segment {
                transmission_time: 0,
                protocol_id: MINI_PROTOCOL_BLOCKFETCH,
                is_responder: true,
                payload: start_buf,
            });

            // Serve blocks in the slot range.
            // Use block_provider.get_blocks_in_range if available, otherwise
            // fall back to serving from/to hashes only.
            // The block provider currently works by hash, so we serve
            // all blocks we can find. For a single-block request (from == to),
            // we just serve that one block.
            if from_slot == to_slot {
                // Single block request
                if let Some(block_data) = block_provider.get_block(&_from_hash) {
                    segments.push(make_block_segment(&block_data)?);
                }
            } else {
                // Range request — try to serve known blocks
                // First the from block, then the to block.
                // A proper implementation would iterate the chain between these points.
                if let Some(block_data) = block_provider.get_block(&_from_hash) {
                    segments.push(make_block_segment(&block_data)?);
                }
                if _from_hash != _to_hash {
                    if let Some(block_data) = block_provider.get_block(&_to_hash) {
                        segments.push(make_block_segment(&block_data)?);
                    }
                }
            }

            // MsgBatchDone: [5]
            let mut done_buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut done_buf);
            enc.array(1)
                .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
            enc.u32(5)
                .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
            segments.push(Segment {
                transmission_time: 0,
                protocol_id: MINI_PROTOCOL_BLOCKFETCH,
                is_responder: true,
                payload: done_buf,
            });

            Ok(segments)
        }
        // MsgClientDone
        1 => {
            debug!("N2N BlockFetch: peer sent MsgClientDone");
            Ok(vec![])
        }
        other => {
            warn!("N2N BlockFetch: unknown message tag {other}");
            Ok(vec![])
        }
    }
}

/// Create a MsgBlock segment: [3, block_bytes]
fn make_block_segment(block_data: &[u8]) -> Result<Segment, N2NServerError> {
    let mut buf = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut buf);
    enc.array(2)
        .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
    enc.u32(3)
        .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
    enc.bytes(block_data)
        .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
    Ok(Segment {
        transmission_time: 0,
        protocol_id: MINI_PROTOCOL_BLOCKFETCH,
        is_responder: true,
        payload: buf,
    })
}

/// Handle KeepAlive mini-protocol.
///
///   MsgKeepAlive (0) [cookie] → MsgKeepAliveResponse (1) [cookie]
///   MsgDone (2) → close protocol
fn handle_keepalive(payload: &[u8]) -> Result<Option<Segment>, N2NServerError> {
    let mut decoder = minicbor::Decoder::new(payload);
    let _arr_len = decoder
        .array()
        .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
    let msg_tag = decoder
        .u32()
        .map_err(|e| N2NServerError::Protocol(e.to_string()))?;

    match msg_tag {
        // MsgKeepAlive: [0, cookie]
        0 => {
            let cookie = decoder.u16().unwrap_or(0);

            // MsgKeepAliveResponse: [1, cookie]
            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);
            enc.array(2)
                .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
            enc.u32(1)
                .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
            enc.u16(cookie)
                .map_err(|e| N2NServerError::Protocol(e.to_string()))?;

            Ok(Some(Segment {
                transmission_time: 0,
                protocol_id: MINI_PROTOCOL_KEEPALIVE,
                is_responder: true,
                payload: buf,
            }))
        }
        // MsgDone
        2 => {
            debug!("KeepAlive: peer sent MsgDone");
            Ok(None)
        }
        other => {
            debug!("KeepAlive: unknown tag {other}");
            Ok(None)
        }
    }
}

/// Handle N2N TxSubmission2 mini-protocol messages.
///
/// TxSubmission2 is the N2N transaction submission protocol.
/// As a server (responder in N2N), we handle:
///   MsgInit (6) → respond with MsgInit (6) to complete bidirectional init
///   MsgRequestTxIds (0) [blocking, ack_count, req_count] → MsgReplyTxIds (1) []
///   MsgRequestTxs (2) [tx_ids] → MsgReplyTxs (3) []
///   MsgDone (5) → close protocol
fn handle_n2n_txsubmission(
    payload: &[u8],
    peer_state: &mut PeerState,
    mempool: &Option<Arc<Mempool>>,
) -> Result<Option<Segment>, N2NServerError> {
    let mut decoder = minicbor::Decoder::new(payload);
    let _arr_len = decoder
        .array()
        .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
    let msg_tag = decoder
        .u32()
        .map_err(|e| N2NServerError::Protocol(e.to_string()))?;

    match msg_tag {
        // MsgInit: [6] — bidirectional initialization
        6 => {
            if !peer_state.tx_submission_init_sent {
                peer_state.tx_submission_init_sent = true;
                let mut buf = Vec::new();
                let mut enc = minicbor::Encoder::new(&mut buf);
                enc.array(1)
                    .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
                enc.u32(6)
                    .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
                debug!("TxSubmission2: init handshake complete");
                Ok(Some(Segment {
                    transmission_time: 0,
                    protocol_id: MINI_PROTOCOL_TXSUBMISSION,
                    is_responder: true,
                    payload: buf,
                }))
            } else {
                Ok(None)
            }
        }
        // MsgRequestTxIds: [0, blocking, ack_count, req_count]
        0 => {
            // Parse req_count (skip blocking and ack_count)
            let _blocking = decoder.bool().unwrap_or(false);
            let _ack_count = decoder.u32().unwrap_or(0);
            let req_count = decoder.u32().unwrap_or(0) as usize;

            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);

            // Get tx IDs from mempool if available
            let txs: Vec<_> = if let Some(mp) = mempool {
                let snapshot = mp.snapshot();
                snapshot
                    .tx_hashes
                    .iter()
                    .take(req_count.max(1))
                    .filter_map(|h| mp.get_tx_size(h).map(|size| (h.as_bytes().to_vec(), size)))
                    .collect()
            } else {
                vec![]
            };

            // MsgReplyTxIds: [1, [[tx_id, size], ...]]
            enc.array(2)
                .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
            enc.u32(1)
                .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
            enc.array(txs.len() as u64)
                .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
            for (tx_hash, size) in &txs {
                enc.array(2)
                    .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
                enc.bytes(tx_hash)
                    .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
                enc.u32(*size as u32)
                    .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
            }
            debug!(count = txs.len(), "TxSubmission2: replied with tx ids");
            Ok(Some(Segment {
                transmission_time: 0,
                protocol_id: MINI_PROTOCOL_TXSUBMISSION,
                is_responder: true,
                payload: buf,
            }))
        }
        // MsgRequestTxs: [2, [tx_ids]]
        2 => {
            // Parse requested tx IDs
            let requested_len = decoder
                .array()
                .map_err(|e| N2NServerError::Protocol(e.to_string()))?
                .unwrap_or(0);

            let mut tx_bodies = Vec::new();
            if let Some(mp) = mempool {
                for _ in 0..requested_len {
                    if let Ok(tx_hash_bytes) = decoder.bytes() {
                        if tx_hash_bytes.len() == 32 {
                            let hash = torsten_primitives::hash::Hash32::from_bytes(
                                tx_hash_bytes.try_into().unwrap(),
                            );
                            if let Some(tx) = mp.get_tx(&hash) {
                                if let Some(ref raw) = tx.raw_cbor {
                                    tx_bodies.push(raw.clone());
                                }
                            }
                        }
                    }
                }
            }

            // MsgReplyTxs: [3, [tx_cbor, ...]]
            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);
            enc.array(2)
                .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
            enc.u32(3)
                .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
            enc.array(tx_bodies.len() as u64)
                .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
            for body in &tx_bodies {
                enc.bytes(body)
                    .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
            }
            debug!(count = tx_bodies.len(), "TxSubmission2: replied with txs");
            Ok(Some(Segment {
                transmission_time: 0,
                protocol_id: MINI_PROTOCOL_TXSUBMISSION,
                is_responder: true,
                payload: buf,
            }))
        }
        // MsgDone
        5 => {
            debug!("TxSubmission2: peer sent MsgDone");
            Ok(None)
        }
        other => {
            debug!("TxSubmission2: unknown tag {other}");
            Ok(None)
        }
    }
}

/// Handle PeerSharing mini-protocol messages.
///
/// As a server (responder), we respond to:
///   MsgShareRequest (0) [amount] → MsgSharePeers (1) [peer_list]
///   MsgDone (2) → close protocol
async fn handle_peersharing(
    payload: &[u8],
    peer_addr: SocketAddr,
    peer_sharing_mode: PeerSharingMode,
    peer_manager: &Option<Arc<RwLock<PeerManager>>>,
) -> Result<Option<Segment>, N2NServerError> {
    let msg = peersharing::decode_message(payload)
        .map_err(|e| N2NServerError::Protocol(format!("PeerSharing decode error: {e}")))?;

    match msg {
        PeerSharingMessage::ShareRequest(amount) => {
            debug!(peer = %peer_addr, amount, "PeerSharing: received share request");

            let peers = if peer_sharing_mode == PeerSharingMode::NoPeerSharing {
                vec![]
            } else if let Some(pm) = peer_manager {
                let pm = pm.read().await;
                pm.peers_for_sharing(amount as usize)
                    .into_iter()
                    .map(PeerAddress::from_socket_addr)
                    .collect()
            } else {
                vec![]
            };

            debug!(
                peer = %peer_addr,
                count = peers.len(),
                "PeerSharing: responding with peers"
            );

            let response_msg = PeerSharingMessage::SharePeers(peers);
            let response_bytes = peersharing::encode_message(&response_msg)
                .map_err(|e| N2NServerError::Protocol(format!("PeerSharing encode error: {e}")))?;

            Ok(Some(Segment {
                transmission_time: 0,
                protocol_id: MINI_PROTOCOL_PEERSHARING,
                is_responder: true,
                payload: response_bytes,
            }))
        }
        PeerSharingMessage::Done => {
            debug!(peer = %peer_addr, "PeerSharing: peer sent MsgDone");
            Ok(None)
        }
        PeerSharingMessage::SharePeers(_) => {
            // We shouldn't receive SharePeers as a responder
            warn!(peer = %peer_addr, "PeerSharing: unexpected MsgSharePeers from initiator");
            Ok(None)
        }
    }
}

/// Parse a point's (slot, hash) from a CBOR-encoded [slot, hash] array
fn parse_point_slot_hash(decoder: &mut minicbor::Decoder) -> Option<(u64, [u8; 32])> {
    decoder.array().ok()?;
    let slot = decoder.u64().ok()?;
    let hash_bytes = decoder.bytes().ok()?;
    if hash_bytes.len() == 32 {
        let mut hash = [0u8; 32];
        hash.copy_from_slice(hash_bytes);
        Some((slot, hash))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_handle_n2n_handshake_accept() {
        // Build a MsgProposeVersions: [0, {13: [magic, false, 0, false], 14: [magic, false, 0, false]}]
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(0).unwrap(); // MsgProposeVersions
        enc.map(2).unwrap();
        // Version 13
        enc.u32(13).unwrap();
        enc.array(4).unwrap();
        enc.u64(2).unwrap(); // preview magic
        enc.bool(false).unwrap();
        enc.u32(0).unwrap();
        enc.bool(false).unwrap();
        // Version 14
        enc.u32(14).unwrap();
        enc.array(4).unwrap();
        enc.u64(2).unwrap();
        enc.bool(false).unwrap();
        enc.u32(0).unwrap();
        enc.bool(false).unwrap();

        let result =
            handle_n2n_handshake(&buf, 2, true, PeerSharingMode::PeerSharingEnabled).unwrap();
        assert!(result.is_some());
        let seg = result.unwrap();
        assert_eq!(seg.protocol_id, MINI_PROTOCOL_HANDSHAKE);
        assert!(seg.is_responder);

        // Verify response contains MsgAcceptVersion (tag 1) with version 14
        let mut dec = minicbor::Decoder::new(&seg.payload);
        dec.array().unwrap();
        let tag = dec.u32().unwrap();
        assert_eq!(tag, 1); // MsgAcceptVersion
        let version = dec.u32().unwrap();
        assert_eq!(version, 14); // highest supported
    }

    #[test]
    fn test_handle_n2n_handshake_refuse_incompatible() {
        // Propose only version 7 (too old)
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(0).unwrap();
        enc.map(1).unwrap();
        enc.u32(7).unwrap();
        enc.array(1).unwrap();
        enc.u64(764824073).unwrap();

        let result =
            handle_n2n_handshake(&buf, 764824073, true, PeerSharingMode::NoPeerSharing).unwrap();
        assert!(result.is_some());
        let seg = result.unwrap();

        let mut dec = minicbor::Decoder::new(&seg.payload);
        dec.array().unwrap();
        let tag = dec.u32().unwrap();
        assert_eq!(tag, 2); // MsgRefuse
    }

    #[test]
    fn test_handle_keepalive_response() {
        // MsgKeepAlive: [0, cookie]
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(0).unwrap();
        enc.u16(42).unwrap();

        let result = handle_keepalive(&buf).unwrap();
        assert!(result.is_some());
        let seg = result.unwrap();
        assert_eq!(seg.protocol_id, MINI_PROTOCOL_KEEPALIVE);

        let mut dec = minicbor::Decoder::new(&seg.payload);
        dec.array().unwrap();
        let tag = dec.u32().unwrap();
        assert_eq!(tag, 1); // MsgKeepAliveResponse
        let cookie = dec.u16().unwrap();
        assert_eq!(cookie, 42);
    }

    #[test]
    fn test_handle_keepalive_done() {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(1).unwrap();
        enc.u32(2).unwrap(); // MsgDone

        let result = handle_keepalive(&buf).unwrap();
        assert!(result.is_none());
    }

    struct MockBlockProvider;

    impl BlockProvider for MockBlockProvider {
        fn get_block(&self, _hash: &[u8; 32]) -> Option<Vec<u8>> {
            Some(vec![0x82, 0x01, 0x02]) // dummy CBOR
        }
        fn has_block(&self, _hash: &[u8; 32]) -> bool {
            true
        }
        fn get_tip(&self) -> (u64, [u8; 32], u64) {
            (100, [0xAA; 32], 50)
        }
        fn get_next_block_after_slot(&self, after_slot: u64) -> Option<(u64, [u8; 32], Vec<u8>)> {
            if after_slot < 100 {
                Some((after_slot + 1, [0xBB; 32], vec![0x82, 0x01, 0x02]))
            } else {
                None
            }
        }
    }

    #[test]
    fn test_handle_blockfetch_request_range() {
        let provider: Arc<dyn BlockProvider> = Arc::new(MockBlockProvider);

        // MsgRequestRange: [0, [slot, hash], [slot, hash]]
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(3).unwrap();
        enc.u32(0).unwrap(); // MsgRequestRange
                             // from point
        enc.array(2).unwrap();
        enc.u64(10).unwrap();
        enc.bytes(&[0xBB; 32]).unwrap();
        // to point
        enc.array(2).unwrap();
        enc.u64(20).unwrap();
        enc.bytes(&[0xCC; 32]).unwrap();

        let segments = handle_n2n_blockfetch(&buf, &provider).unwrap();
        // Should have: MsgStartBatch + 2 blocks + MsgBatchDone = 4 segments
        assert_eq!(segments.len(), 4);

        // First segment: MsgStartBatch [2]
        let mut dec = minicbor::Decoder::new(&segments[0].payload);
        dec.array().unwrap();
        assert_eq!(dec.u32().unwrap(), 2);

        // Last segment: MsgBatchDone [5]
        let mut dec = minicbor::Decoder::new(&segments[3].payload);
        dec.array().unwrap();
        assert_eq!(dec.u32().unwrap(), 5);
    }

    #[test]
    fn test_handle_blockfetch_client_done() {
        let provider: Arc<dyn BlockProvider> = Arc::new(MockBlockProvider);

        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(1).unwrap();
        enc.u32(1).unwrap(); // MsgClientDone

        let segments = handle_n2n_blockfetch(&buf, &provider).unwrap();
        assert!(segments.is_empty());
    }

    #[test]
    fn test_handle_txsubmission_init() {
        let mut peer_state = PeerState::new();

        // MsgInit: [6]
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(1).unwrap();
        enc.u32(6).unwrap();

        let no_mempool: Option<Arc<Mempool>> = None;
        let result = handle_n2n_txsubmission(&buf, &mut peer_state, &no_mempool).unwrap();
        assert!(result.is_some());
        let seg = result.unwrap();
        assert_eq!(seg.protocol_id, MINI_PROTOCOL_TXSUBMISSION);

        let mut dec = minicbor::Decoder::new(&seg.payload);
        dec.array().unwrap();
        assert_eq!(dec.u32().unwrap(), 6); // MsgInit response

        // Second init should be no-op
        let result2 = handle_n2n_txsubmission(&buf, &mut peer_state, &no_mempool).unwrap();
        assert!(result2.is_none());
    }

    #[test]
    fn test_handle_txsubmission_request_tx_ids() {
        let mut peer_state = PeerState::new();
        peer_state.tx_submission_init_sent = true;

        // MsgRequestTxIds: [0, false, 0, 1]
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(4).unwrap();
        enc.u32(0).unwrap();
        enc.bool(false).unwrap();
        enc.u32(0).unwrap();
        enc.u32(1).unwrap();

        let no_mempool: Option<Arc<Mempool>> = None;
        let result = handle_n2n_txsubmission(&buf, &mut peer_state, &no_mempool).unwrap();
        assert!(result.is_some());
        let seg = result.unwrap();

        let mut dec = minicbor::Decoder::new(&seg.payload);
        dec.array().unwrap();
        assert_eq!(dec.u32().unwrap(), 1); // MsgReplyTxIds
    }

    #[test]
    fn test_rate_limiter_allows_within_limit() {
        let limiter = ConnectionRateLimiter::new(3, std::time::Duration::from_secs(60));
        let ip: IpAddr = "127.0.0.1".parse().unwrap();

        assert!(limiter.check_and_record(ip));
        assert!(limiter.check_and_record(ip));
        assert!(limiter.check_and_record(ip));
        // 4th should be rejected
        assert!(!limiter.check_and_record(ip));
    }

    #[test]
    fn test_rate_limiter_different_ips() {
        let limiter = ConnectionRateLimiter::new(1, std::time::Duration::from_secs(60));
        let ip1: IpAddr = "127.0.0.1".parse().unwrap();
        let ip2: IpAddr = "127.0.0.2".parse().unwrap();

        assert!(limiter.check_and_record(ip1));
        assert!(!limiter.check_and_record(ip1)); // second from same IP rejected
        assert!(limiter.check_and_record(ip2)); // different IP allowed
    }

    #[test]
    fn test_rate_limiter_cleanup() {
        let limiter = ConnectionRateLimiter::new(1, std::time::Duration::from_millis(1));
        let ip: IpAddr = "127.0.0.1".parse().unwrap();

        assert!(limiter.check_and_record(ip));
        assert!(!limiter.check_and_record(ip));

        // Wait for window to expire
        std::thread::sleep(std::time::Duration::from_millis(5));

        limiter.cleanup();
        // Should allow again after window expires
        assert!(limiter.check_and_record(ip));
    }
}
