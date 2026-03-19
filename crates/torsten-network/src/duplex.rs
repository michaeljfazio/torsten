//! Full-duplex N2N peer connection (Phase 1 + Phase 2).
//!
//! Fixes GitHub issue #187: outbound N2N connections using `PeerClient` are
//! initiator-only — the remote peer cannot request our mempool transactions
//! over these connections.  This module provides `DuplexPeerConnection`, which
//! runs **both** initiator and responder mini-protocols on a single TCP
//! connection, using the pallas `Plexer`'s ability to `subscribe_client` and
//! `subscribe_server` simultaneously before `spawn()`.
//!
//! # Protocol assignment
//!
//! In a full-duplex (InitiatorAndResponder) N2N connection:
//!
//! | Mini-protocol    | We are…    | Why                                               |
//! |------------------|------------|---------------------------------------------------|
//! | Handshake (0)    | Client     | We open the connection and propose versions        |
//! | ChainSync (2)    | Client     | We sync headers *from* the peer                   |
//! | BlockFetch (3)   | Client     | We fetch full blocks *from* the peer               |
//! | TxSubmission2(4) | **Server** | Remote peer requests our mempool txs *from* us     |
//! | KeepAlive (8)    | Client     | We send pings; remote echoes back                  |
//! | PeerSharing (10) | Client     | We request peers (also tolerate responder messages)|
//!
//! The TxSubmission2 responder task (Phase 2) handles MsgInit, MsgRequestTxIds,
//! MsgRequestTxs, and MsgDone from the remote peer, serving our mempool contents
//! in response.
//!
//! # Multiplexer direction conventions (pallas)
//!
//! `subscribe_client(P)` → sends on protocol P (bit-15 = 0), receives on P|0x8000
//! `subscribe_server(P)` → sends on P|0x8000, receives on protocol P (bit-15 = 0)
//!
//! This matches the Ouroboros wire format: initiator messages have bit-15 = 0,
//! responder messages have bit-15 = 1.

use pallas_network::miniprotocols::handshake;
use pallas_network::miniprotocols::{
    PROTOCOL_N2N_BLOCK_FETCH, PROTOCOL_N2N_CHAIN_SYNC, PROTOCOL_N2N_HANDSHAKE,
    PROTOCOL_N2N_KEEP_ALIVE, PROTOCOL_N2N_PEER_SHARING, PROTOCOL_N2N_TX_SUBMISSION,
};
use pallas_network::multiplexer::{AgentChannel, Bearer, Plexer, RunningPlexer};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::ToSocketAddrs;
use torsten_primitives::mempool::MempoolProvider;
use tracing::{debug, info, warn};

use crate::client::ClientError;
use crate::n2n_server::BlockProvider;

// ─── Error type ─────────────────────────────────────────────────────────────

/// Errors from establishing or running a full-duplex peer connection.
#[derive(Debug, thiserror::Error)]
pub enum DuplexError {
    #[error("connection error: {0}")]
    Connection(String),
    #[error("handshake failed: {0}")]
    Handshake(String),
    #[error("TxSubmission2 responder error: {0}")]
    TxSubmission(String),
    #[error("CBOR error: {0}")]
    Cbor(String),
    #[error("channel error: {0}")]
    Channel(String),
    #[error("timeout: {0}")]
    Timeout(String),
}

impl From<DuplexError> for ClientError {
    fn from(e: DuplexError) -> Self {
        ClientError::Connection(e.to_string())
    }
}

// ─── Constants ──────────────────────────────────────────────────────────────

/// Maximum tx IDs we serve per MsgReplyTxIds batch.  Matches the initiator cap
/// in `txsubmission.rs`.
const MAX_TX_IDS_PER_REPLY: usize = 100;

/// Maximum inflight tx IDs (sent to a single peer, not yet acknowledged).
const MAX_TX_INFLIGHT: usize = 1000;

/// Maximum tx bodies we serve per MsgRequestTxs.
const MAX_TX_BODY_REQUEST: usize = 1000;

/// Maximum reassembled message size for the TxSubmission2 responder (8 MB).
const MAX_REASSEMBLY_SIZE: usize = 8 * 1024 * 1024;

/// Timeout for the remote peer's first MsgInit on TxSubmission2.
/// After this long with no MsgInit the remote is not participating.
const TXSUB_INIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Timeout for subsequent MsgRequestTxIds (blocking requests may hold longer).
const TXSUB_BLOCKING_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

// ─── DuplexPeerConnection ───────────────────────────────────────────────────

/// A full-duplex outbound N2N connection that simultaneously runs:
///  - ChainSync client      (we pull headers from the remote)
///  - BlockFetch client     (we pull blocks from the remote)
///  - KeepAlive client      (we ping the remote)
///  - TxSubmission2 server  (remote pulls our mempool txs)
///
/// Compared to `PipelinedPeerClient`, this connection advertises
/// `initiator_only_diffusion_mode = false` (InitiatorAndResponder) in the
/// handshake, and starts a background task that serves our mempool to the
/// remote peer via TxSubmission2.
pub struct DuplexPeerConnection {
    /// ChainSync raw buffer (bypasses pallas state machine for pipelining).
    pub cs_buf: pallas_network::multiplexer::ChannelBuffer,
    /// BlockFetch client (pallas managed).
    pub bf_client: pallas_network::miniprotocols::blockfetch::Client,
    /// KeepAlive handle (spawned background loop).
    pub _keepalive: pallas_network::facades::KeepAliveHandle,
    /// Running plexer (demuxer + muxer tasks).
    pub _plexer: RunningPlexer,
    /// PeerSharing client channel — kept alive so the demuxer never panics on
    /// incoming PeerSharing messages (~60 s after connect).
    pub _peersharing_channel: AgentChannel,
    /// Remote peer's socket address.
    pub remote_addr: SocketAddr,
    /// Number of pipelined ChainSync requests outstanding.
    pub in_flight: usize,
    /// Byron epoch length for correct slot computation on non-mainnet networks.
    pub byron_epoch_length: u64,
    /// Background TxSubmission2 responder task handle.
    /// Dropping this aborts the task, which is intentional on disconnect.
    _txsub_responder: tokio::task::JoinHandle<()>,
}

impl DuplexPeerConnection {
    /// Open a TCP connection to `addr` and establish a full-duplex N2N session.
    ///
    /// # Arguments
    /// - `addr`           — target peer address
    /// - `network_magic`  — Cardano network magic (e.g. 2 for preview)
    /// - `mempool`        — local mempool (served via TxSubmission2 to the peer)
    /// - `block_provider` — chain storage (for future ChainSync/BlockFetch server tasks)
    pub async fn connect(
        addr: impl ToSocketAddrs + std::fmt::Display + Copy,
        network_magic: u64,
        mempool: Arc<dyn MempoolProvider>,
        _block_provider: Arc<dyn BlockProvider>,
    ) -> Result<Self, DuplexError> {
        debug!("duplex: connecting to {addr}");

        // --- TCP connect with keepalive ------------------------------------------
        let stream = tokio::net::TcpStream::connect(addr)
            .await
            .map_err(|e| DuplexError::Connection(format!("tcp connect: {e}")))?;
        if let Err(e) = crate::tcp::configure_tcp_keepalive(&stream) {
            warn!("duplex: failed to set TCP keepalive: {e}");
        }
        let bearer = Bearer::Tcp(stream);

        // --- Plexer setup: subscribe both client AND server channels -------------
        //
        // We subscribe all channels we need BEFORE spawn(), then immediately
        // spawn the plexer so its demuxer/muxer tasks are running.  Any protocol
        // we do not subscribe will have its incoming segments silently dropped by
        // the demuxer, which is fine.
        let mut plexer = Plexer::new(bearer);

        // Handshake: always initiator-side (we propose versions).
        let hs_channel = plexer.subscribe_client(PROTOCOL_N2N_HANDSHAKE);

        // ChainSync: initiator (we request headers).
        let cs_channel = plexer.subscribe_client(PROTOCOL_N2N_CHAIN_SYNC);

        // BlockFetch: initiator (we request blocks).
        let bf_channel = plexer.subscribe_client(PROTOCOL_N2N_BLOCK_FETCH);

        // TxSubmission2: **server** (remote peer requests our txs).
        //   subscribe_server(P) → sends on P|0x8000, receives on P
        let txsub_server_channel = plexer.subscribe_server(PROTOCOL_N2N_TX_SUBMISSION);

        // KeepAlive: initiator (we ping; remote echoes).
        let ka_channel = plexer.subscribe_client(PROTOCOL_N2N_KEEP_ALIVE);

        // PeerSharing: initiator channel kept open to avoid demuxer panics.
        let peersharing_channel = plexer.subscribe_client(PROTOCOL_N2N_PEER_SHARING);

        // Start the plexer tasks.
        let plexer = plexer.spawn();

        // --- Handshake -----------------------------------------------------------
        //
        // We advertise InitiatorAndResponder (initiator_only = false) so the
        // remote peer knows it can send us TxSubmission2 requests.
        let mut hs_client = handshake::Client::new(hs_channel);
        let versions = build_n2n_version_table(network_magic);
        let handshake_result = hs_client
            .handshake(versions)
            .await
            .map_err(|e| DuplexError::Handshake(format!("handshake protocol: {e}")))?;

        if let handshake::Confirmation::Rejected(reason) = handshake_result {
            return Err(DuplexError::Handshake(format!(
                "handshake rejected: {reason:?}"
            )));
        }

        debug!("duplex: handshake accepted by {addr}");

        // --- KeepAlive client loop -----------------------------------------------
        let ka_client = pallas_network::miniprotocols::keepalive::Client::new(ka_channel);
        let keepalive = pallas_network::facades::KeepAliveLoop::client(
            ka_client,
            std::time::Duration::from_secs(16),
        )
        .spawn();

        // --- ChainSync buffer (raw, for pipelining) ------------------------------
        let cs_buf = pallas_network::multiplexer::ChannelBuffer::new(cs_channel);

        // --- BlockFetch client ---------------------------------------------------
        let bf_client = pallas_network::miniprotocols::blockfetch::Client::new(bf_channel);

        // --- TxSubmission2 responder task (Phase 2) ------------------------------
        //
        // This background task handles all incoming TxSubmission2 protocol messages
        // from the remote peer.  When the remote sends MsgRequestTxIds we reply
        // with our current mempool contents; when they send MsgRequestTxs we reply
        // with the raw CBOR bytes.  The task exits cleanly on MsgDone or channel
        // close.
        let remote_addr_str = format!("{addr}");
        let remote_addr: SocketAddr = remote_addr_str
            .parse()
            .unwrap_or_else(|_| std::net::SocketAddr::from(([0, 0, 0, 0], 0)));

        let txsub_responder = tokio::spawn(serve_tx_submission(
            txsub_server_channel,
            mempool,
            remote_addr,
        ));

        info!("duplex: connected to {remote_addr} (InitiatorAndResponder)");

        Ok(DuplexPeerConnection {
            cs_buf,
            bf_client,
            _keepalive: keepalive,
            _plexer: plexer,
            _peersharing_channel: peersharing_channel,
            remote_addr,
            in_flight: 0,
            byron_epoch_length: 0,
            _txsub_responder: txsub_responder,
        })
    }

    /// Set the Byron epoch length for non-mainnet networks.
    pub fn set_byron_epoch_length(&mut self, len: u64) {
        self.byron_epoch_length = len;
    }

    /// Remote address of this connection.
    pub fn remote_addr(&self) -> SocketAddr {
        self.remote_addr
    }

    /// Access the BlockFetch client for fetching full blocks.
    pub fn blockfetch(&mut self) -> &mut pallas_network::miniprotocols::blockfetch::Client {
        &mut self.bf_client
    }

    /// Abort the connection (kills the plexer and responder task).
    pub async fn abort(self) {
        self._txsub_responder.abort();
        self._plexer.abort().await;
    }
}

// ─── Phase 2: TxSubmission2 server ─────────────────────────────────────────

/// Serve TxSubmission2 requests from a remote peer on behalf of our local mempool.
///
/// This is the **responder** side of the N2N TxSubmission2 protocol.  In the
/// Ouroboros model, when a peer opens a full-duplex connection to us, we are
/// the TxSubmission2 **server** — they ask for our mempool tx IDs, then ask
/// for the corresponding CBOR bodies.
///
/// Protocol flow (remote peer is initiator/client; we are responder/server):
/// 1. Remote sends MsgInit [6]
/// 2. We respond with MsgInit [6]
/// 3. Remote sends MsgRequestTxIds [0, blocking, ack, req_count]
///    → We reply MsgReplyTxIds [1, [[tx_id, size], ...]]
/// 4. Remote sends MsgRequestTxs [2, [tx_id, ...]]
///    → We reply MsgReplyTxs [3, [tx_cbor, ...]]
/// 5. Remote sends MsgDone [4] → we exit
///
/// The function runs until the remote peer closes the protocol or the channel
/// is dropped.  All errors are logged as warnings (non-fatal for the parent
/// connection).
pub async fn serve_tx_submission(
    channel: AgentChannel,
    mempool: Arc<dyn MempoolProvider>,
    peer_addr: SocketAddr,
) {
    match serve_tx_submission_inner(channel, mempool, peer_addr).await {
        Ok(()) => debug!(%peer_addr, "TxSubmission2 responder: session complete"),
        Err(e) => warn!(%peer_addr, "TxSubmission2 responder: {e}"),
    }
}

/// Inner function that returns `Result` so we can use `?` throughout.
async fn serve_tx_submission_inner(
    mut channel: AgentChannel,
    mempool: Arc<dyn MempoolProvider>,
    peer_addr: SocketAddr,
) -> Result<(), DuplexError> {
    // Wait for MsgInit from the remote peer.  Use a short timeout — if the
    // peer does not initiate TxSubmission2 within this window they are not
    // participating in the protocol on this connection.
    let init_payload = tokio::time::timeout(TXSUB_INIT_TIMEOUT, recv_msg(&mut channel))
        .await
        .map_err(|_| DuplexError::Timeout("waiting for TxSubmission2 MsgInit".into()))??;

    let init_tag = decode_first_tag(&init_payload)?;
    if init_tag != 6 {
        return Err(DuplexError::TxSubmission(format!(
            "expected MsgInit (6) from peer, got tag {init_tag}"
        )));
    }
    debug!(%peer_addr, "TxSubmission2 responder: received MsgInit from peer");

    // Reply with our own MsgInit [6].
    let init_reply = encode_msg_init();
    send_msg(&mut channel, &init_reply).await?;
    info!(%peer_addr, "TxSubmission2 responder: init handshake complete");

    // Track inflight tx IDs (sent to this peer, not yet acknowledged).
    // This vector is ordered: front = oldest, back = newest.  We drain
    // from the front when the peer sends ack_count > 0.
    let mut inflight: Vec<[u8; 32]> = Vec::new();

    loop {
        // Use a generous timeout for the blocking MsgRequestTxIds case:
        // the peer may hold us for up to 5 minutes while waiting for new txs.
        let payload = tokio::time::timeout(TXSUB_BLOCKING_TIMEOUT, recv_msg(&mut channel))
            .await
            .map_err(|_| DuplexError::Timeout("TxSubmission2 MsgRequestTxIds timeout".into()))??;

        let tag = decode_first_tag(&payload)?;

        match tag {
            // MsgRequestTxIds: [0, blocking, ack_count, req_count]
            0 => {
                let (blocking, ack_count, req_count) = decode_request_tx_ids(&payload)?;

                debug!(
                    %peer_addr,
                    blocking,
                    ack_count,
                    req_count,
                    "TxSubmission2 responder: MsgRequestTxIds"
                );

                // Acknowledge previously sent tx IDs.
                if ack_count > 0 {
                    let drain_count = ack_count.min(inflight.len());
                    inflight.drain(..drain_count);
                }

                // Enforce inflight cap: if at the limit, reply empty so the
                // peer can send more acks before we push new tx IDs.
                let reply = if inflight.len() >= MAX_TX_INFLIGHT {
                    warn!(
                        %peer_addr,
                        inflight = inflight.len(),
                        "TxSubmission2 responder: inflight cap reached, sending empty reply"
                    );
                    encode_reply_tx_ids(&[])
                } else {
                    // Compute how many IDs we can send without exceeding the cap.
                    let remaining_cap = MAX_TX_INFLIGHT - inflight.len();
                    let capped_req = req_count.min(MAX_TX_IDS_PER_REPLY).min(remaining_cap);

                    // Pull up to capped_req new tx IDs from the mempool, excluding
                    // those already in-flight to this peer.
                    let new_ids: Vec<([u8; 32], u32)> = {
                        let snapshot = mempool.snapshot();
                        snapshot
                            .tx_hashes
                            .iter()
                            .filter(|h| {
                                let bytes = *h.as_bytes();
                                !inflight.iter().any(|inf| inf == &bytes)
                            })
                            .take(capped_req)
                            .filter_map(|h| {
                                mempool.get_tx_size(h).map(|sz| (*h.as_bytes(), sz as u32))
                            })
                            .collect()
                    };

                    // Record newly sent IDs as inflight.
                    for (hash, _) in &new_ids {
                        if inflight.len() < MAX_TX_INFLIGHT {
                            inflight.push(*hash);
                        }
                    }

                    if !new_ids.is_empty() {
                        info!(
                            %peer_addr,
                            count = new_ids.len(),
                            inflight = inflight.len(),
                            "TxSubmission2 responder: sending MsgReplyTxIds"
                        );
                    }

                    encode_reply_tx_ids(&new_ids)
                };

                send_msg(&mut channel, &reply).await?;
            }

            // MsgRequestTxs: [2, [tx_id, ...]]
            2 => {
                let requested_hashes = decode_request_txs(&payload)?;
                debug!(
                    %peer_addr,
                    count = requested_hashes.len(),
                    "TxSubmission2 responder: MsgRequestTxs"
                );

                // Fetch CBOR bodies from mempool for each requested hash.
                // Cap the number of bodies returned to prevent memory exhaustion.
                let bodies: Vec<Vec<u8>> = requested_hashes
                    .iter()
                    .take(MAX_TX_BODY_REQUEST)
                    .filter_map(|hash| {
                        let h = torsten_primitives::hash::Hash32::from_bytes(*hash);
                        // Prefer pre-stored raw CBOR bytes; fall back to
                        // get_tx().raw_cbor if the mempool caches it there.
                        mempool
                            .get_tx_cbor(&h)
                            .or_else(|| mempool.get_tx(&h).and_then(|tx| tx.raw_cbor.clone()))
                    })
                    .collect();

                info!(
                    %peer_addr,
                    count = bodies.len(),
                    requested = requested_hashes.len(),
                    "TxSubmission2 responder: sending MsgReplyTxs"
                );

                let reply = encode_reply_txs(&bodies);
                send_msg(&mut channel, &reply).await?;
            }

            // MsgDone: [4]
            4 => {
                info!(%peer_addr, "TxSubmission2 responder: peer sent MsgDone, closing");
                break;
            }

            // MsgInit again (shouldn't happen after handshake, be tolerant)
            6 => {
                debug!(%peer_addr, "TxSubmission2 responder: duplicate MsgInit, ignoring");
            }

            other => {
                warn!(
                    %peer_addr,
                    tag = other,
                    "TxSubmission2 responder: unexpected message tag, closing"
                );
                break;
            }
        }
    }

    Ok(())
}

// ─── Channel I/O helpers ────────────────────────────────────────────────────

/// Send a CBOR payload through an `AgentChannel`, splitting into chunks as
/// required by the 65535-byte segment limit.
async fn send_msg(channel: &mut AgentChannel, payload: &[u8]) -> Result<(), DuplexError> {
    for chunk in payload.chunks(pallas_network::multiplexer::MAX_SEGMENT_PAYLOAD_LENGTH) {
        channel
            .enqueue_chunk(chunk.to_vec())
            .await
            .map_err(|e| DuplexError::Channel(e.to_string()))?;
    }
    Ok(())
}

/// Receive a complete CBOR message from an `AgentChannel`, reassembling chunks.
///
/// Reads chunks until the accumulated bytes form a complete CBOR value (tested
/// via `Decoder::skip`).  Enforces `MAX_REASSEMBLY_SIZE` to prevent exhaustion
/// from a misbehaving peer.
async fn recv_msg(channel: &mut AgentChannel) -> Result<Vec<u8>, DuplexError> {
    let mut buf: Vec<u8> = Vec::new();
    loop {
        let chunk = channel
            .dequeue_chunk()
            .await
            .map_err(|e| DuplexError::Channel(e.to_string()))?;
        buf.extend_from_slice(&chunk);

        if buf.len() > MAX_REASSEMBLY_SIZE {
            return Err(DuplexError::TxSubmission(format!(
                "reassembled message exceeds {} bytes",
                MAX_REASSEMBLY_SIZE
            )));
        }

        // Test for a complete CBOR value at the start of `buf`.
        let mut probe = minicbor::Decoder::new(&buf);
        if probe.skip().is_ok() {
            return Ok(buf);
        }
        // Incomplete — keep reading chunks.
    }
}

// ─── CBOR decode helpers ────────────────────────────────────────────────────

/// Decode the first array element (the message tag) from a CBOR payload.
fn decode_first_tag(payload: &[u8]) -> Result<u32, DuplexError> {
    let mut dec = minicbor::Decoder::new(payload);
    dec.array()
        .map_err(|e| DuplexError::Cbor(format!("expected array: {e}")))?;
    dec.u32()
        .map_err(|e| DuplexError::Cbor(format!("expected tag u32: {e}")))
}

/// Decode `MsgRequestTxIds = [0, blocking, ack_count, req_count]`.
fn decode_request_tx_ids(payload: &[u8]) -> Result<(bool, usize, usize), DuplexError> {
    let mut dec = minicbor::Decoder::new(payload);
    dec.array()
        .map_err(|e| DuplexError::Cbor(format!("MsgRequestTxIds: expected array: {e}")))?;
    let _tag = dec
        .u32()
        .map_err(|e| DuplexError::Cbor(format!("MsgRequestTxIds: expected tag: {e}")))?;
    let blocking = dec
        .bool()
        .map_err(|e| DuplexError::Cbor(format!("MsgRequestTxIds: expected blocking bool: {e}")))?;
    let ack_count = dec
        .u16()
        .map_err(|e| DuplexError::Cbor(format!("MsgRequestTxIds: expected ack_count: {e}")))?
        as usize;
    let req_count = dec
        .u16()
        .map_err(|e| DuplexError::Cbor(format!("MsgRequestTxIds: expected req_count: {e}")))?
        as usize;
    Ok((blocking, ack_count, req_count))
}

/// Decode `MsgRequestTxs = [2, [tx_id, ...]]` and return the list of tx hashes.
fn decode_request_txs(payload: &[u8]) -> Result<Vec<[u8; 32]>, DuplexError> {
    let mut dec = minicbor::Decoder::new(payload);
    dec.array()
        .map_err(|e| DuplexError::Cbor(format!("MsgRequestTxs: expected outer array: {e}")))?;
    let _tag = dec
        .u32()
        .map_err(|e| DuplexError::Cbor(format!("MsgRequestTxs: expected tag: {e}")))?;

    let count = dec
        .array()
        .map_err(|e| DuplexError::Cbor(format!("MsgRequestTxs: expected id array: {e}")))?
        .unwrap_or(0);

    let cap = (count as usize).min(MAX_TX_BODY_REQUEST);
    let mut hashes = Vec::with_capacity(cap);
    for _ in 0..cap {
        let bytes = dec.bytes().map_err(|e| {
            DuplexError::Cbor(format!("MsgRequestTxs: expected tx hash bytes: {e}"))
        })?;
        if bytes.len() == 32 {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(bytes);
            hashes.push(arr);
        } else {
            warn!(
                len = bytes.len(),
                "TxSubmission2 responder: ignoring tx hash with unexpected length"
            );
        }
    }
    Ok(hashes)
}

// ─── CBOR encode helpers ────────────────────────────────────────────────────

/// Encode `MsgInit = [6]`.
fn encode_msg_init() -> Vec<u8> {
    let mut buf = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut buf);
    enc.array(1).expect("encode array");
    enc.u32(6).expect("encode tag");
    buf
}

/// Encode `MsgReplyTxIds = [1, [[tx_id, size_bytes], ...]]`.
///
/// Each element is `[bytes(tx_hash), uint(size)]`.  An empty slice produces
/// `[1, []]`, which is the correct non-blocking empty reply.
fn encode_reply_tx_ids(ids: &[([u8; 32], u32)]) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut buf);
    enc.array(2).expect("encode outer array");
    enc.u32(1).expect("encode tag");
    enc.array(ids.len() as u64).expect("encode id array");
    for (hash, size) in ids {
        enc.array(2).expect("encode pair");
        enc.bytes(hash.as_slice()).expect("encode hash");
        enc.u32(*size).expect("encode size");
    }
    buf
}

/// Encode `MsgReplyTxs = [3, [tx_cbor, ...]]`.
fn encode_reply_txs(bodies: &[Vec<u8>]) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut buf);
    enc.array(2).expect("encode outer array");
    enc.u32(3).expect("encode tag");
    enc.array(bodies.len() as u64).expect("encode body array");
    for body in bodies {
        enc.bytes(body).expect("encode tx cbor");
    }
    buf
}

// ─── Handshake version table ─────────────────────────────────────────────────

/// Build an N2N version table advertising `InitiatorAndResponder` mode.
///
/// The key difference from `VersionTable::v7_and_above()` (used by
/// `PipelinedPeerClient` and `PeerClient`) is that we set
/// `initiator_only_diffusion_mode = false`.  This tells the remote peer that
/// we accept incoming mini-protocol traffic on this connection, enabling them
/// to request our mempool transactions via TxSubmission2.
///
/// We advertise versions 14 and 15 (matching cardano-node 10.x).  Version 13
/// is the minimum that supports the 4-element params array; earlier versions
/// only carry `[magic, initiator_only]` so there is no peer_sharing field.
fn build_n2n_version_table(network_magic: u64) -> handshake::n2n::VersionTable {
    use handshake::n2n::VersionData;
    use std::collections::HashMap;

    // initiator_only_diffusion_mode = false  →  InitiatorAndResponder
    // peer_sharing = 0 (disabled)
    // query = false
    let make_version =
        |magic: u64| -> VersionData { VersionData::new(magic, false, Some(0), Some(false)) };

    let values: HashMap<u64, VersionData> = vec![
        (14u64, make_version(network_magic)),
        (15u64, make_version(network_magic)),
    ]
    .into_iter()
    .collect();

    handshake::n2n::VersionTable { values }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // CBOR encoding / decoding round-trip tests (no I/O required)
    // -----------------------------------------------------------------------

    /// MsgInit must encode as the 1-element array [6].
    #[test]
    fn test_encode_msg_init() {
        let bytes = encode_msg_init();
        let mut dec = minicbor::Decoder::new(&bytes);
        assert_eq!(dec.array().unwrap().unwrap_or(0), 1);
        assert_eq!(dec.u32().unwrap(), 6);
    }

    /// decode_first_tag must correctly identify the protocol tag.
    #[test]
    fn test_decode_first_tag_init() {
        let bytes = encode_msg_init();
        assert_eq!(decode_first_tag(&bytes).unwrap(), 6);
    }

    /// MsgReplyTxIds empty (non-blocking empty reply).
    #[test]
    fn test_encode_reply_tx_ids_empty() {
        let bytes = encode_reply_tx_ids(&[]);
        let mut dec = minicbor::Decoder::new(&bytes);
        assert_eq!(dec.array().unwrap().unwrap_or(0), 2); // outer array(2)
        assert_eq!(dec.u32().unwrap(), 1); // tag = 1
        assert_eq!(dec.array().unwrap().unwrap_or(99), 0); // inner array(0)
    }

    /// MsgReplyTxIds with two entries round-trips correctly.
    #[test]
    fn test_encode_reply_tx_ids_two_entries() {
        let hash_a = [0x11u8; 32];
        let hash_b = [0x22u8; 32];
        let ids = vec![(hash_a, 512u32), (hash_b, 1024u32)];
        let bytes = encode_reply_tx_ids(&ids);

        let mut dec = minicbor::Decoder::new(&bytes);
        assert_eq!(dec.array().unwrap().unwrap_or(0), 2);
        assert_eq!(dec.u32().unwrap(), 1); // MsgReplyTxIds tag

        let n = dec.array().unwrap().unwrap_or(0);
        assert_eq!(n, 2);
        for (expected_hash, expected_size) in &ids {
            dec.array().unwrap(); // inner pair
            let hash = dec.bytes().unwrap();
            assert_eq!(hash, expected_hash.as_slice());
            assert_eq!(dec.u32().unwrap(), *expected_size);
        }
    }

    /// MsgReplyTxs with two bodies round-trips correctly.
    #[test]
    fn test_encode_reply_txs() {
        let bodies = vec![vec![0x01, 0x02, 0x03], vec![0xFF, 0xFE]];
        let bytes = encode_reply_txs(&bodies);

        let mut dec = minicbor::Decoder::new(&bytes);
        assert_eq!(dec.array().unwrap().unwrap_or(0), 2);
        assert_eq!(dec.u32().unwrap(), 3); // MsgReplyTxs tag

        let n = dec.array().unwrap().unwrap_or(0);
        assert_eq!(n as usize, bodies.len());
        for expected in &bodies {
            assert_eq!(dec.bytes().unwrap(), expected.as_slice());
        }
    }

    /// decode_request_tx_ids extracts (blocking, ack_count, req_count).
    #[test]
    fn test_decode_request_tx_ids_non_blocking() {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(4).unwrap();
        enc.u32(0).unwrap(); // tag
        enc.bool(false).unwrap(); // non-blocking
        enc.u16(7u16).unwrap(); // ack_count
        enc.u16(100u16).unwrap(); // req_count

        let (blocking, ack, req) = decode_request_tx_ids(&buf).unwrap();
        assert!(!blocking);
        assert_eq!(ack, 7);
        assert_eq!(req, 100);
    }

    /// decode_request_tx_ids handles blocking = true.
    #[test]
    fn test_decode_request_tx_ids_blocking() {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(4).unwrap();
        enc.u32(0).unwrap();
        enc.bool(true).unwrap(); // blocking
        enc.u16(0u16).unwrap();
        enc.u16(50u16).unwrap();

        let (blocking, ack, req) = decode_request_tx_ids(&buf).unwrap();
        assert!(blocking);
        assert_eq!(ack, 0);
        assert_eq!(req, 50);
    }

    /// decode_request_txs extracts hash list correctly.
    #[test]
    fn test_decode_request_txs() {
        let hash_a = [0xAAu8; 32];
        let hash_b = [0xBBu8; 32];

        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(2).unwrap(); // MsgRequestTxs tag
        enc.array(2u64).unwrap();
        enc.bytes(&hash_a).unwrap();
        enc.bytes(&hash_b).unwrap();

        let hashes = decode_request_txs(&buf).unwrap();
        assert_eq!(hashes.len(), 2);
        assert_eq!(hashes[0], hash_a);
        assert_eq!(hashes[1], hash_b);
    }

    /// decode_request_txs ignores hashes that are not exactly 32 bytes.
    #[test]
    fn test_decode_request_txs_bad_hash_len() {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(2).unwrap();
        enc.array(1u64).unwrap();
        enc.bytes(&[0xCC; 16]).unwrap(); // 16 bytes — wrong length

        // Should succeed but return an empty list (bad-length hash dropped).
        let hashes = decode_request_txs(&buf).unwrap();
        assert_eq!(hashes.len(), 0);
    }

    /// The blocking CBOR field must be a CBOR bool (0xF5/0xF4), not an integer.
    #[test]
    fn test_request_tx_ids_bool_encoding() {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(4).unwrap();
        enc.u32(0).unwrap();
        enc.bool(true).unwrap();
        enc.u16(0u16).unwrap();
        enc.u16(100u16).unwrap();

        // CBOR true = 0xF5 (major type 7 / simple value 21)
        assert!(
            buf.contains(&0xF5),
            "blocking=true must encode as CBOR true (0xF5), not integer"
        );
    }

    /// build_n2n_version_table must produce versions 14 and 15.
    #[test]
    fn test_version_table_versions_14_15() {
        let table = build_n2n_version_table(2);
        assert!(table.values.contains_key(&14), "version 14 must be present");
        assert!(table.values.contains_key(&15), "version 15 must be present");
    }

    /// All versions in the duplex table must have initiator_only = false.
    #[test]
    fn test_version_table_initiator_only_false() {
        let table = build_n2n_version_table(764824073);
        for (version, data) in &table.values {
            assert!(
                !data.initiator_only_diffusion_mode,
                "version {version} must have initiator_only_diffusion_mode = false for \
                 InitiatorAndResponder mode"
            );
        }
    }

    /// Version data must carry our network magic correctly.
    #[test]
    fn test_version_table_network_magic() {
        let magic = 2u64; // preview
        let table = build_n2n_version_table(magic);
        for data in table.values.values() {
            assert_eq!(
                data.network_magic, magic,
                "all version entries must carry the supplied network magic"
            );
        }
    }

    /// Inflight cap logic: once inflight reaches MAX_TX_INFLIGHT we must send empty.
    #[test]
    fn test_inflight_cap_sends_empty() {
        // Simulate the cap check logic extracted from serve_tx_submission_inner.
        let mut inflight: Vec<[u8; 32]> = Vec::new();
        for i in 0..MAX_TX_INFLIGHT {
            let mut h = [0u8; 32];
            h[0] = (i >> 24) as u8;
            h[1] = (i >> 16) as u8;
            h[2] = (i >> 8) as u8;
            h[3] = i as u8;
            inflight.push(h);
        }
        assert_eq!(inflight.len(), MAX_TX_INFLIGHT);

        // When inflight is at cap, no new IDs should be sent.
        let remaining_cap = MAX_TX_INFLIGHT.saturating_sub(inflight.len());
        assert_eq!(remaining_cap, 0, "remaining cap must be 0 at the limit");
    }

    /// Ack count drain logic: draining fewer entries than in-flight.
    #[test]
    fn test_ack_drain_partial() {
        let mut inflight: Vec<[u8; 32]> = vec![[0u8; 32]; 5];
        let ack_count = 3usize;
        let drain = ack_count.min(inflight.len());
        inflight.drain(..drain);
        assert_eq!(inflight.len(), 2);
    }

    /// Ack count drain logic: draining more than in-flight (peer sent stale ack).
    #[test]
    fn test_ack_drain_over() {
        let mut inflight: Vec<[u8; 32]> = vec![[0u8; 32]; 3];
        let ack_count = 10usize; // more than inflight
        let drain = ack_count.min(inflight.len());
        inflight.drain(..drain);
        assert_eq!(inflight.len(), 0, "inflight must be empty after over-ack");
    }
}
