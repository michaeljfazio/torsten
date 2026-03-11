use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Instant;
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, RwLock};
use tracing::{debug, error, info, warn};

use crate::miniprotocols::peersharing::{self, PeerAddress, PeerSharingMessage};
use crate::multiplexer::Segment;
use crate::peer_manager::PeerManager;
use crate::query_handler::QueryHandler;
use torsten_mempool::Mempool;

/// Notification sent when a new block is forged and should be announced to peers
#[derive(Debug, Clone)]
pub struct BlockAnnouncement {
    /// Slot number of the new block
    pub slot: u64,
    /// Header hash of the new block
    pub hash: [u8; 32],
    /// Block number
    pub block_number: u64,
}

/// Notification that the chain has rolled back to a specific point.
/// Sent via broadcast channel to connected peers so they receive MsgRollBackward.
#[derive(Debug, Clone)]
pub struct RollbackAnnouncement {
    /// Slot to roll back to
    pub slot: u64,
    /// Hash of the block at the rollback point
    pub hash: [u8; 32],
    /// Current tip slot after rollback
    pub tip_slot: u64,
    /// Current tip hash after rollback
    pub tip_hash: [u8; 32],
    /// Current tip block number after rollback
    pub tip_block_number: u64,
}

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

/// Maximum number of distinct IPs tracked by the rate limiter.
/// Prevents unbounded HashMap growth from attackers connecting from many IPs.
const MAX_TRACKED_IPS: usize = 100_000;

/// Run inline cleanup every N insertions to evict stale entries.
const CLEANUP_EVERY_N_INSERTIONS: usize = 1_000;

/// Per-IP connection rate limiter to prevent DoS attacks.
/// Tracks connection timestamps per IP and enforces:
/// - Max connections per IP within a time window
/// - Cleanup of stale entries
/// - Bounded memory via MAX_TRACKED_IPS cap
struct ConnectionRateLimiter {
    /// Map of IP → list of connection timestamps
    attempts: std::sync::Mutex<HashMap<IpAddr, Vec<Instant>>>,
    /// Max connections allowed per IP within the window
    max_per_ip: usize,
    /// Time window for rate limiting
    window: std::time::Duration,
    /// Insertion counter for periodic inline cleanup
    insertion_count: std::sync::atomic::AtomicUsize,
    /// Maximum number of tracked IPs (for testing override)
    max_tracked_ips: usize,
}

impl ConnectionRateLimiter {
    fn new(max_per_ip: usize, window: std::time::Duration) -> Self {
        ConnectionRateLimiter {
            attempts: std::sync::Mutex::new(HashMap::new()),
            max_per_ip,
            window,
            insertion_count: std::sync::atomic::AtomicUsize::new(0),
            max_tracked_ips: MAX_TRACKED_IPS,
        }
    }

    /// Check if a connection from this IP should be allowed.
    /// Returns true if allowed, false if rate-limited.
    fn check_and_record(&self, ip: IpAddr) -> bool {
        let now = Instant::now();
        let mut attempts = self.attempts.lock().unwrap_or_else(|e| e.into_inner());

        // Periodic inline cleanup: every N insertions, evict expired entries
        let count = self
            .insertion_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if count.is_multiple_of(CLEANUP_EVERY_N_INSERTIONS) && count > 0 {
            attempts.retain(|_, timestamps| {
                timestamps.retain(|t| now.duration_since(*t) < self.window);
                !timestamps.is_empty()
            });
        }

        // If map is at capacity and this is a new IP, reject the connection
        if attempts.len() >= self.max_tracked_ips && !attempts.contains_key(&ip) {
            warn!(
                tracked_ips = attempts.len(),
                max = self.max_tracked_ips,
                "Rate limiter: rejecting new IP, tracked IP cap reached"
            );
            return false;
        }

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
        let mut attempts = self.attempts.lock().unwrap_or_else(|e| e.into_inner());
        attempts.retain(|_, timestamps| {
            timestamps.retain(|t| now.duration_since(*t) < self.window);
            !timestamps.is_empty()
        });
    }

    /// Return the number of IPs currently tracked (for testing)
    #[cfg(test)]
    fn tracked_ip_count(&self) -> usize {
        self.attempts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
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
    /// Broadcast channel for block announcements to connected peers
    block_announcement_tx: broadcast::Sender<BlockAnnouncement>,
    /// Broadcast channel for rollback notifications to connected peers
    rollback_announcement_tx: broadcast::Sender<RollbackAnnouncement>,
}

impl N2NServer {
    pub fn new(
        listen_addr: SocketAddr,
        network_magic: u64,
        query_handler: Arc<RwLock<QueryHandler>>,
        block_provider: Arc<dyn BlockProvider>,
        max_connections: usize,
    ) -> Self {
        let (block_announcement_tx, _) = broadcast::channel(64);
        let (rollback_announcement_tx, _) = broadcast::channel(64);
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
            block_announcement_tx,
            rollback_announcement_tx,
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
        let (block_announcement_tx, _) = broadcast::channel(64);
        let (rollback_announcement_tx, _) = broadcast::channel(64);
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
            block_announcement_tx,
            rollback_announcement_tx,
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

    /// Announce a newly forged block to all connected peers.
    /// Peers waiting in MsgAwaitReply will be woken up to fetch the new block.
    pub fn announce_block(&self, slot: u64, hash: [u8; 32], block_number: u64) {
        let announcement = BlockAnnouncement {
            slot,
            hash,
            block_number,
        };
        // Ignore send errors (no receivers yet or all dropped)
        let _ = self.block_announcement_tx.send(announcement);
        info!(slot, block_number, "Block announced to peers");
    }

    /// Get a broadcast sender for block announcements.
    /// Used by the node to announce blocks without a direct reference to the server.
    pub fn block_announcement_sender(&self) -> broadcast::Sender<BlockAnnouncement> {
        self.block_announcement_tx.clone()
    }

    /// Get a clone of the rollback announcement sender for notifying peers of chain rollbacks
    pub fn rollback_announcement_sender(&self) -> broadcast::Sender<RollbackAnnouncement> {
        self.rollback_announcement_tx.clone()
    }

    /// Start listening for inbound N2N connections.
    /// Accepts connections until the shutdown signal is received.
    pub async fn listen(
        &self,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> Result<(), N2NServerError> {
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
            tokio::select! {
            result = listener.accept() => {
            match result {
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
                    let announcement_rx = self.block_announcement_tx.subscribe();
                    let rollback_rx = self.rollback_announcement_tx.subscribe();

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
                            announcement_rx,
                            rollback_rx,
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
            _ = shutdown_rx.changed() => {
                info!("N2N server shutting down");
                return Ok(());
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
    /// TxSubmission2 flow control: tx IDs sent but not yet acknowledged
    tx_inflight: Vec<[u8; 32]>,
}

impl PeerState {
    fn new() -> Self {
        PeerState {
            chainsync_cursor_slot: None,
            chainsync_cursor_hash: None,
            tx_submission_init_sent: false,
            tx_inflight: Vec::new(),
        }
    }
}

/// Handle a single inbound N2N peer connection.
/// Listens for both peer messages and block announcements concurrently.
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
    mut announcement_rx: broadcast::Receiver<BlockAnnouncement>,
    mut rollback_rx: broadcast::Receiver<RollbackAnnouncement>,
) -> Result<(), N2NServerError> {
    let mut buf = vec![0u8; 65536];
    let mut partial = Vec::new();
    let mut peer_state = PeerState::new();

    loop {
        tokio::select! {
            // Listen for peer messages
            read_result = stream.read(&mut buf) => {
                let n = read_result?;
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
            // Listen for block announcements from our node
            announcement = announcement_rx.recv() => {
                match announcement {
                    Ok(ann) => {
                        // If peer is at tip (waiting for new block), send them the new
                        // block via MsgRollForward on ChainSync protocol.
                        if let Some(cursor_slot) = peer_state.chainsync_cursor_slot {
                            if cursor_slot < ann.slot {
                                // Peer is behind the announced block — serve the next block
                                if let Some((_next_slot, next_hash, block_cbor)) =
                                    block_provider.get_next_block_after_slot(cursor_slot)
                                {
                                    peer_state.chainsync_cursor_slot = Some(ann.slot);
                                    peer_state.chainsync_cursor_hash = Some(next_hash);

                                    let payload = build_chainsync_roll_forward(
                                        &block_cbor, ann.slot, &ann.hash, ann.block_number,
                                    )?;

                                    let segment = Segment {
                                        transmission_time: 0,
                                        protocol_id: MINI_PROTOCOL_CHAINSYNC,
                                        is_responder: true,
                                        payload,
                                    };
                                    let encoded = segment.encode();
                                    stream.write_all(&encoded).await?;
                                    debug!(
                                        peer = %peer_addr,
                                        slot = ann.slot,
                                        "Pushed block announcement to peer"
                                    );
                                }
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        debug!(peer = %peer_addr, skipped = n, "Block announcement receiver lagged");
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        // Sender dropped, server is shutting down
                        return Ok(());
                    }
                }
            }
            // Listen for rollback notifications
            rollback = rollback_rx.recv() => {
                match rollback {
                    Ok(rb) => {
                        // Only send rollback if peer's cursor is beyond the rollback point
                        if let Some(cursor_slot) = peer_state.chainsync_cursor_slot {
                            if cursor_slot > rb.slot {
                                // Update peer cursor to the rollback point
                                peer_state.chainsync_cursor_slot = Some(rb.slot);
                                peer_state.chainsync_cursor_hash = Some(rb.hash);

                                // MsgRollBackward: [3, point, tip]
                                let mut payload = Vec::new();
                                let mut enc = minicbor::Encoder::new(&mut payload);
                                enc.array(3)
                                    .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
                                enc.u32(3)
                                    .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
                                encode_point(&mut enc, rb.slot, &rb.hash)?;
                                encode_tip(&mut enc, rb.tip_slot, &rb.tip_hash, rb.tip_block_number)?;

                                let segment = Segment {
                                    transmission_time: 0,
                                    protocol_id: MINI_PROTOCOL_CHAINSYNC,
                                    is_responder: true,
                                    payload,
                                };
                                let encoded = segment.encode();
                                stream.write_all(&encoded).await?;
                                debug!(
                                    peer = %peer_addr,
                                    rollback_slot = rb.slot,
                                    "Sent MsgRollBackward to peer"
                                );
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        debug!(peer = %peer_addr, skipped = n, "Rollback announcement receiver lagged");
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        return Ok(());
                    }
                }
            }
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
    // N2N versions: 14 (Plomin HF) and 15 (SRV DNS support)
    // We support versions 14-15 (matching current cardano-node)
    let mut best_version: Option<u32> = None;
    let mut magic_mismatch_version: Option<u32> = None;
    let map_len = decoder
        .map()
        .map_err(|e| N2NServerError::HandshakeFailed(e.to_string()))?;

    let count = map_len.unwrap_or(0);
    for _ in 0..count {
        let version = decoder
            .u32()
            .map_err(|e| N2NServerError::HandshakeFailed(e.to_string()))?;

        // Try to extract network_magic from params: [magic, diffusion, peer_sharing, query]
        let peer_magic = {
            let pos = decoder.position();
            let m = if let Ok(Some(_)) = decoder.array() {
                decoder.u64().ok()
            } else {
                None
            };
            decoder.set_position(pos);
            m
        };
        // Skip the value (params)
        decoder
            .skip()
            .map_err(|e| N2NServerError::HandshakeFailed(e.to_string()))?;

        // Accept versions 14-15 (current cardano-node N2N)
        if (14..=15).contains(&version) {
            if let Some(pm) = peer_magic {
                if pm != network_magic {
                    magic_mismatch_version = Some(version);
                    continue;
                }
            }
            if best_version.is_none_or(|bv| version > bv) {
                best_version = Some(version);
            }
        }
    }

    let version = match best_version {
        Some(v) => v,
        None => {
            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);
            enc.array(2)
                .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
            enc.u32(2)
                .map_err(|e| N2NServerError::Protocol(e.to_string()))?; // MsgRefuse

            if let Some(v) = magic_mismatch_version {
                // Refused: [2, version, reason_text]
                enc.array(3)
                    .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
                enc.u32(2)
                    .map_err(|e| N2NServerError::Protocol(e.to_string()))?; // Refused reason tag
                enc.u32(v)
                    .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
                enc.str(&format!("networkMagic mismatch: expected {network_magic}"))
                    .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
            } else {
                // VersionMismatch: [0, [supported_versions...]]
                enc.array(2)
                    .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
                enc.u32(0)
                    .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
                enc.array(2)
                    .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
                enc.u32(14)
                    .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
                enc.u32(15)
                    .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
            }

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

                // MsgRollForward: [2, [era_tag, tag(24) header_cbor], tip]
                let buf =
                    build_chainsync_roll_forward(&block_cbor, tip_slot, &tip_hash, tip_block)?;

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
                        if intersect_point.is_none_or(|(s, _)| slot > s) {
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

/// Extract the block header CBOR and era tag from full block CBOR.
///
/// Cardano block CBOR structure: [era_tag, [header, tx_bodies, witnesses, aux, invalid_txs]]
/// Returns (era_tag, header_cbor_bytes) or None if parsing fails.
fn extract_header_from_block(block_cbor: &[u8]) -> Option<(u16, Vec<u8>)> {
    let mut decoder = minicbor::Decoder::new(block_cbor);

    // Outer array: [era_tag, block_content]
    decoder.array().ok()?;
    let era_tag = decoder.u32().ok()? as u16;

    // Inner array: [header, tx_bodies, witnesses, aux_data, invalid_txs]
    decoder.array().ok()?;

    // Capture the raw CBOR bytes of the header element
    let header_start = decoder.position();
    // Skip over the header element (whatever its structure)
    decoder.skip().ok()?;
    let header_end = decoder.position();

    Some((era_tag, block_cbor[header_start..header_end].to_vec()))
}

/// Build a MsgRollForward ChainSync segment from block CBOR.
///
/// Extracts just the header from the full block and wraps it properly:
/// MsgRollForward: [2, [era_tag, tag(24) header_cbor], tip]
fn build_chainsync_roll_forward(
    block_cbor: &[u8],
    tip_slot: u64,
    tip_hash: &[u8; 32],
    tip_block: u64,
) -> Result<Vec<u8>, N2NServerError> {
    let mut buf = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut buf);
    enc.array(3)
        .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
    enc.u32(2)
        .map_err(|e| N2NServerError::Protocol(e.to_string()))?;

    // Extract header from full block CBOR for bandwidth efficiency
    if let Some((era_tag, header_bytes)) = extract_header_from_block(block_cbor) {
        // Wrapped header: [era_tag, tag(24) header_cbor]
        enc.array(2)
            .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
        enc.u32(era_tag as u32)
            .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
        enc.tag(minicbor::data::Tag::new(24))
            .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
        enc.bytes(&header_bytes)
            .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
    } else {
        // Fallback: send full block CBOR if header extraction fails
        enc.tag(minicbor::data::Tag::new(24))
            .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
        enc.bytes(block_cbor)
            .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
    }

    encode_tip(&mut enc, tip_slot, tip_hash, tip_block)?;
    Ok(buf)
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
                // MsgNoBlocks: [3]
                let mut buf = Vec::new();
                let mut enc = minicbor::Encoder::new(&mut buf);
                enc.array(1)
                    .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
                enc.u32(3)
                    .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
                segments.push(Segment {
                    transmission_time: 0,
                    protocol_id: MINI_PROTOCOL_BLOCKFETCH,
                    is_responder: true,
                    payload: buf,
                });
                return Ok(segments);
            }

            // Safety: from/to are guaranteed Some by the from_exists/to_exists check above
            let (from_slot, _from_hash) = from.expect("from checked above");
            let (to_slot, _to_hash) = to.expect("to checked above");

            // Limit range to prevent DoS — max 2000 blocks per request
            const MAX_BLOCKFETCH_RANGE: u64 = 2000;
            if to_slot > from_slot + MAX_BLOCKFETCH_RANGE {
                warn!(from_slot, to_slot, "BlockFetch: range too large, rejecting");
                let mut buf = Vec::new();
                let mut enc = minicbor::Encoder::new(&mut buf);
                enc.array(1)
                    .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
                enc.u32(3)
                    .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
                segments.push(Segment {
                    transmission_time: 0,
                    protocol_id: MINI_PROTOCOL_BLOCKFETCH,
                    is_responder: true,
                    payload: buf,
                });
                return Ok(segments);
            }

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
            // Serve all blocks in the range [from_slot, to_slot]
            // Start with the from block, then iterate forward until we reach to_slot
            if let Some(block_data) = block_provider.get_block(&_from_hash) {
                segments.push(make_block_segment(&block_data)?);
            }

            if from_slot < to_slot {
                let mut current_slot = from_slot;
                while let Some((next_slot, next_hash, next_cbor)) =
                    block_provider.get_next_block_after_slot(current_slot)
                {
                    if next_slot > to_slot {
                        break;
                    }
                    // Skip the from block (already served above)
                    if next_hash != _from_hash {
                        segments.push(make_block_segment(&next_cbor)?);
                    }
                    current_slot = next_slot;
                    if next_slot == to_slot {
                        break;
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

/// Create a MsgBlock segment: [4, block_bytes]
fn make_block_segment(block_data: &[u8]) -> Result<Segment, N2NServerError> {
    let mut buf = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut buf);
    enc.array(2)
        .map_err(|e| N2NServerError::Protocol(e.to_string()))?;
    enc.u32(4)
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
            let _blocking = decoder.bool().unwrap_or(false);
            let ack_count = decoder.u16().unwrap_or(0) as usize;
            let req_count = decoder.u16().unwrap_or(0) as usize;

            // Acknowledge previously sent tx IDs (remove from inflight)
            if ack_count > 0 && ack_count <= peer_state.tx_inflight.len() {
                peer_state.tx_inflight.drain(..ack_count);
                debug!(ack_count, "TxSubmission2: acknowledged tx ids");
            } else if ack_count > 0 {
                peer_state.tx_inflight.clear();
            }

            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);

            // Get new tx IDs from mempool, excluding those already inflight
            let txs: Vec<_> = if let Some(mp) = mempool {
                let snapshot = mp.snapshot();
                snapshot
                    .tx_hashes
                    .iter()
                    .filter(|h| {
                        let bytes = h.as_bytes();
                        !peer_state
                            .tx_inflight
                            .iter()
                            .any(|inflight| inflight == bytes)
                    })
                    .take(req_count.max(1))
                    .filter_map(|h| mp.get_tx_size(h).map(|size| (*h.as_bytes(), size)))
                    .collect()
            } else {
                vec![]
            };

            // Track newly sent tx IDs as inflight (capped to prevent memory growth)
            const MAX_TX_INFLIGHT: usize = 1000;
            for (tx_hash, _) in &txs {
                if peer_state.tx_inflight.len() < MAX_TX_INFLIGHT {
                    peer_state.tx_inflight.push(*tx_hash);
                }
            }

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
            debug!(
                count = txs.len(),
                inflight = peer_state.tx_inflight.len(),
                "TxSubmission2: replied with tx ids"
            );
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
                        if let Ok(hash_arr) = <[u8; 32]>::try_from(tx_hash_bytes) {
                            let hash = torsten_primitives::hash::Hash32::from_bytes(hash_arr);
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
        // Build a MsgProposeVersions: [0, {14: [magic, false, 0, false], 15: [magic, false, 0, false]}]
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(0).unwrap(); // MsgProposeVersions
        enc.map(2).unwrap();
        // Version 14
        enc.u32(14).unwrap();
        enc.array(4).unwrap();
        enc.u64(2).unwrap(); // preview magic
        enc.bool(false).unwrap();
        enc.u32(0).unwrap();
        enc.bool(false).unwrap();
        // Version 15
        enc.u32(15).unwrap();
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

        // Verify response contains MsgAcceptVersion (tag 1) with version 15
        let mut dec = minicbor::Decoder::new(&seg.payload);
        dec.array().unwrap();
        let tag = dec.u32().unwrap();
        assert_eq!(tag, 1); // MsgAcceptVersion
        let version = dec.u32().unwrap();
        assert_eq!(version, 15); // highest supported
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
    fn test_handle_n2n_handshake_refuse_magic_mismatch() {
        // Propose V14 with wrong network magic
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(0).unwrap(); // MsgProposeVersions
        enc.map(1).unwrap();
        enc.u32(14).unwrap();
        enc.array(4).unwrap();
        enc.u64(999).unwrap(); // wrong magic
        enc.bool(false).unwrap();
        enc.u32(0).unwrap();
        enc.bool(false).unwrap();

        let result = handle_n2n_handshake(&buf, 2, true, PeerSharingMode::NoPeerSharing).unwrap();
        assert!(result.is_some());
        let seg = result.unwrap();

        let mut dec = minicbor::Decoder::new(&seg.payload);
        dec.array().unwrap();
        let tag = dec.u32().unwrap();
        assert_eq!(tag, 2); // MsgRefuse
                            // Should be [2, version, reason_text] format
        dec.array().unwrap();
        let reason_tag = dec.u32().unwrap();
        assert_eq!(reason_tag, 2); // Refused (not VersionMismatch)
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
                let next_slot = after_slot + 1;
                let mut hash = [0u8; 32];
                hash[0] = (next_slot & 0xFF) as u8;
                hash[1] = ((next_slot >> 8) & 0xFF) as u8;
                Some((next_slot, hash, vec![0x82, 0x01, 0x02]))
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
        // Should have: MsgStartBatch + from_block + 10 range blocks (slots 11-20) + MsgBatchDone = 13
        assert_eq!(segments.len(), 13);

        // First segment: MsgStartBatch [2]
        let mut dec = minicbor::Decoder::new(&segments[0].payload);
        dec.array().unwrap();
        assert_eq!(dec.u32().unwrap(), 2);

        // Middle segments are MsgBlock [4, block_bytes]
        let mut dec = minicbor::Decoder::new(&segments[1].payload);
        dec.array().unwrap();
        assert_eq!(dec.u32().unwrap(), 4);

        // Last segment: MsgBatchDone [5]
        let mut dec = minicbor::Decoder::new(&segments[segments.len() - 1].payload);
        dec.array().unwrap();
        assert_eq!(dec.u32().unwrap(), 5);
    }

    #[test]
    fn test_extract_header_from_block() {
        // Build a mock block CBOR: [era_tag=6, [header, tx_bodies, witnesses, aux, invalid]]
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(6).unwrap(); // era tag (Babbage)

        // Inner array with 5 elements
        enc.array(5).unwrap();

        // Header: [header_body, kes_sig]
        enc.array(2).unwrap();
        enc.array(10).unwrap(); // header_body (10 fields)
        for i in 0..10u64 {
            enc.u64(i).unwrap();
        }
        enc.bytes(&[0xAA; 32]).unwrap(); // kes_sig placeholder

        // Remaining 4 elements (tx_bodies, witnesses, aux, invalid)
        enc.map(0).unwrap();
        enc.array(0).unwrap();
        enc.null().unwrap();
        enc.array(0).unwrap();

        let result = extract_header_from_block(&buf);
        assert!(result.is_some());
        let (era_tag, header_bytes) = result.unwrap();
        assert_eq!(era_tag, 6);

        // Verify header_bytes decode to [header_body, kes_sig]
        let mut dec = minicbor::Decoder::new(&header_bytes);
        let arr_len = dec.array().unwrap().unwrap();
        assert_eq!(arr_len, 2);
    }

    #[test]
    fn test_build_chainsync_roll_forward() {
        // Build a mock block
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(6).unwrap(); // era tag
        enc.array(5).unwrap();
        enc.array(2).unwrap(); // header
        enc.u64(42).unwrap(); // header_body placeholder
        enc.bytes(&[0xBB; 32]).unwrap(); // kes_sig
        enc.map(0).unwrap();
        enc.array(0).unwrap();
        enc.null().unwrap();
        enc.array(0).unwrap();

        let tip_hash = [0xCC; 32];
        let payload = build_chainsync_roll_forward(&buf, 100, &tip_hash, 50).unwrap();

        // Decode: [2, [era_tag, tag(24) header_cbor], tip]
        let mut dec = minicbor::Decoder::new(&payload);
        let arr = dec.array().unwrap().unwrap();
        assert_eq!(arr, 3);
        assert_eq!(dec.u32().unwrap(), 2); // MsgRollForward tag

        // Wrapped header: [era_tag, tag(24) header_bytes]
        let inner_arr = dec.array().unwrap().unwrap();
        assert_eq!(inner_arr, 2);
        assert_eq!(dec.u32().unwrap(), 6); // era tag
        assert_eq!(dec.tag().unwrap(), minicbor::data::Tag::new(24));
        let header_bytes = dec.bytes().unwrap();
        assert!(!header_bytes.is_empty());
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
    fn test_txsubmission_flow_control() {
        let mut peer_state = PeerState::new();
        peer_state.tx_submission_init_sent = true;

        // Simulate sending tx IDs: peer_state inflight tracking
        let hash1 = [1u8; 32];
        let hash2 = [2u8; 32];
        peer_state.tx_inflight.push(hash1);
        peer_state.tx_inflight.push(hash2);
        assert_eq!(peer_state.tx_inflight.len(), 2);

        // MsgRequestTxIds with ack_count=1: acknowledge first tx
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(4).unwrap();
        enc.u32(0).unwrap();
        enc.bool(false).unwrap();
        enc.u32(1).unwrap(); // ack 1
        enc.u32(1).unwrap(); // request 1

        let no_mempool: Option<Arc<Mempool>> = None;
        let _result = handle_n2n_txsubmission(&buf, &mut peer_state, &no_mempool).unwrap();

        // Should have removed one from inflight
        assert_eq!(peer_state.tx_inflight.len(), 1);
        assert_eq!(peer_state.tx_inflight[0], hash2);

        // Ack remaining
        let mut buf2 = Vec::new();
        let mut enc2 = minicbor::Encoder::new(&mut buf2);
        enc2.array(4).unwrap();
        enc2.u32(0).unwrap();
        enc2.bool(false).unwrap();
        enc2.u32(1).unwrap(); // ack 1
        enc2.u32(1).unwrap();

        let _result2 = handle_n2n_txsubmission(&buf2, &mut peer_state, &no_mempool).unwrap();
        assert!(peer_state.tx_inflight.is_empty());
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

    #[test]
    fn test_rate_limiter_cap_prevents_unbounded_growth() {
        // Use a small cap for testing
        let mut limiter = ConnectionRateLimiter::new(1, std::time::Duration::from_secs(60));
        limiter.max_tracked_ips = 10;

        // Fill up to the cap with distinct IPs
        for i in 0..10u8 {
            let ip: IpAddr = format!("10.0.0.{i}").parse().unwrap();
            assert!(
                limiter.check_and_record(ip),
                "IP 10.0.0.{i} should be allowed"
            );
        }
        assert_eq!(limiter.tracked_ip_count(), 10);

        // 11th distinct IP should be rejected (cap reached)
        let new_ip: IpAddr = "10.0.0.10".parse().unwrap();
        assert!(
            !limiter.check_and_record(new_ip),
            "New IP should be rejected when cap is reached"
        );
        // Map should not have grown
        assert_eq!(limiter.tracked_ip_count(), 10);
    }

    #[test]
    fn test_rate_limiter_cap_allows_existing_ips() {
        // Even at cap, existing IPs can still connect (up to per-IP limit)
        let mut limiter = ConnectionRateLimiter::new(3, std::time::Duration::from_secs(60));
        limiter.max_tracked_ips = 5;

        for i in 0..5u8 {
            let ip: IpAddr = format!("10.0.0.{i}").parse().unwrap();
            assert!(limiter.check_and_record(ip));
        }
        assert_eq!(limiter.tracked_ip_count(), 5);

        // Existing IP should still be allowed (under per-IP limit of 3)
        let existing_ip: IpAddr = "10.0.0.0".parse().unwrap();
        assert!(
            limiter.check_and_record(existing_ip),
            "Existing IP should still be allowed at cap"
        );
        // Still 5 tracked IPs
        assert_eq!(limiter.tracked_ip_count(), 5);
    }

    #[test]
    fn test_rate_limiter_cleanup_frees_cap_space() {
        // After cleanup of expired entries, new IPs should be allowed again
        let mut limiter = ConnectionRateLimiter::new(1, std::time::Duration::from_millis(1));
        limiter.max_tracked_ips = 5;

        // Fill up the cap
        for i in 0..5u8 {
            let ip: IpAddr = format!("10.0.0.{i}").parse().unwrap();
            assert!(limiter.check_and_record(ip));
        }
        assert_eq!(limiter.tracked_ip_count(), 5);

        // New IP rejected at cap
        let new_ip: IpAddr = "10.0.0.5".parse().unwrap();
        assert!(!limiter.check_and_record(new_ip));

        // Wait for all entries to expire
        std::thread::sleep(std::time::Duration::from_millis(5));

        // Run cleanup to evict expired entries
        limiter.cleanup();
        assert_eq!(limiter.tracked_ip_count(), 0);

        // Now new IPs should be allowed again
        assert!(
            limiter.check_and_record(new_ip),
            "New IP should be allowed after cleanup frees space"
        );
    }

    #[test]
    fn test_rate_limiter_inline_cleanup_evicts_stale() {
        // Test that inline cleanup (triggered every N insertions) removes expired entries
        let mut limiter = ConnectionRateLimiter::new(1, std::time::Duration::from_millis(1));
        limiter.max_tracked_ips = 100_000; // Don't hit cap

        // Reset the counter so next call is at count=0 (no cleanup on first)
        limiter
            .insertion_count
            .store(0, std::sync::atomic::Ordering::Relaxed);

        // Add some IPs
        for i in 0..10u8 {
            let ip: IpAddr = format!("10.0.0.{i}").parse().unwrap();
            limiter.check_and_record(ip);
        }
        assert_eq!(limiter.tracked_ip_count(), 10);

        // Wait for entries to expire
        std::thread::sleep(std::time::Duration::from_millis(5));

        // Set counter so the next fetch_add(1) returns a value that triggers cleanup.
        // fetch_add returns the old value, so store N so the returned count is N,
        // and N % N == 0 && N > 0 triggers the cleanup branch.
        limiter.insertion_count.store(
            CLEANUP_EVERY_N_INSERTIONS,
            std::sync::atomic::Ordering::Relaxed,
        );

        // This call will trigger inline cleanup (counter becomes CLEANUP_EVERY_N_INSERTIONS)
        // which should evict all expired entries. The new IP gets added.
        let trigger_ip: IpAddr = "10.0.0.100".parse().unwrap();
        assert!(limiter.check_and_record(trigger_ip));

        // After inline cleanup, only the newly added IP should remain
        assert_eq!(limiter.tracked_ip_count(), 1);
    }
}
