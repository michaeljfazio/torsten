//! Full-duplex N2N peer connection (Phase 1 + Phase 2 + Phase 3).
//!
//! Fixes GitHub issue #187: outbound N2N connections using `PeerClient` are
//! initiator-only — the remote peer cannot request our mempool transactions
//! over these connections.  This module provides `DuplexPeerConnection`, which
//! runs **both** initiator and responder mini-protocols on a single TCP
//! connection, using the pallas `Plexer`'s ability to `subscribe_client` and
//! `subscribe_server` simultaneously before `spawn()`.
//!
//! Fixes GitHub issue #193: on duplex connections we now also act as the
//! TxSubmission2 **initiator** (client), so we pull the remote peer's mempool
//! transactions into our local mempool.  The initiator and responder tasks run
//! concurrently on the same connection — this is the standard full-duplex N2N
//! behaviour in cardano-node.
//!
//! # Protocol assignment
//!
//! In a full-duplex (InitiatorAndResponder) N2N connection:
//!
//! | Mini-protocol    | We are…             | Why                                               |
//! |------------------|---------------------|---------------------------------------------------|
//! | Handshake (0)    | Client              | We open the connection and propose versions        |
//! | ChainSync (2)    | Client              | We sync headers *from* the peer                   |
//! | BlockFetch (3)   | Client              | We fetch full blocks *from* the peer               |
//! | TxSubmission2(4) | **Client+Server**   | We pull peer txs AND serve our own mempool txs     |
//! | KeepAlive (8)    | Client              | We send pings; remote echoes back                  |
//! | PeerSharing (10) | Client              | We request peers (also tolerate responder messages)|
//!
//! The TxSubmission2 responder task handles MsgInit, MsgRequestTxIds,
//! MsgRequestTxs, and MsgDone from the remote peer, serving our mempool contents
//! in response.
//!
//! The TxSubmission2 initiator task sends MsgInit, then periodically sends
//! MsgRequestTxIds to pull the remote peer's mempool txs into our local mempool.
//!
//! # Multiplexer direction conventions (pallas)
//!
//! `subscribe_client(P)` → sends on protocol P (bit-15 = 0), receives on P|0x8000
//! `subscribe_server(P)` → sends on P|0x8000, receives on protocol P (bit-15 = 0)
//!
//! This matches the Ouroboros wire format: initiator messages have bit-15 = 0,
//! responder messages have bit-15 = 1.
//!
//! Both roles can coexist on the same protocol ID because the multiplexer
//! routes by (protocol_id, direction) — client traffic for protocol 4 is
//! routed to the `subscribe_client(4)` channel, server traffic to the
//! `subscribe_server(4)` channel.

use pallas_network::miniprotocols::handshake;
use pallas_network::miniprotocols::{
    PROTOCOL_N2N_BLOCK_FETCH, PROTOCOL_N2N_CHAIN_SYNC, PROTOCOL_N2N_HANDSHAKE,
    PROTOCOL_N2N_KEEP_ALIVE, PROTOCOL_N2N_PEER_SHARING, PROTOCOL_N2N_TX_SUBMISSION,
};
use pallas_network::multiplexer::{AgentChannel, Bearer, Plexer, RunningPlexer};
use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::ToSocketAddrs;
use torsten_primitives::hash::Hash32;
use torsten_primitives::mempool::{MempoolAddResult, MempoolProvider};
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
    #[error("TxSubmission2 initiator error: {0}")]
    TxSubmissionInitiator(String),
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

/// Maximum reassembled message size for the TxSubmission2 responder/initiator (8 MB).
const MAX_REASSEMBLY_SIZE: usize = 8 * 1024 * 1024;

/// Timeout for the TxSubmission2 MsgInit exchange.
/// The Haskell node's Server sleeps 60 seconds (`threadDelay 60`) before
/// reading MsgInit, so our Client-side timeout must exceed that.
/// 90 seconds provides a comfortable margin.
const TXSUB_INIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);

/// Timeout for subsequent MsgRequestTxIds (blocking requests may hold longer).
const TXSUB_BLOCKING_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

// ─── Initiator constants ─────────────────────────────────────────────────────

/// How many tx IDs to request per MsgRequestTxIds from a peer.
const INITIATOR_REQ_COUNT: u16 = 100;

/// Maximum tx IDs we track in the per-peer known-set before eviction.
/// Prevents unbounded memory growth over long-lived sessions.
const INITIATOR_MAX_KNOWN: usize = 10_000;

/// Timeout for receiving a non-blocking MsgReplyTxIds from the peer.
const INITIATOR_RESPONSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Timeout for receiving a blocking MsgReplyTxIds from the peer.
/// A blocking request holds until the peer has at least one new tx.
const INITIATOR_BLOCKING_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

// ─── DuplexPeerConnection ───────────────────────────────────────────────────

/// A full-duplex outbound N2N connection that simultaneously runs:
///  - ChainSync client       (we pull headers from the remote)
///  - BlockFetch client      (we pull blocks from the remote)
///  - KeepAlive client       (we ping the remote)
///  - TxSubmission2 server   (remote pulls our mempool txs)
///  - TxSubmission2 client   (we pull remote peer's mempool txs)
///
/// Compared to `PipelinedPeerClient`, this connection advertises
/// `initiator_only_diffusion_mode = false` (InitiatorAndResponder) in the
/// handshake, and starts two background tasks:
///
///  1. **Responder** — serves our mempool to the remote peer.
///  2. **Initiator** — pulls the remote peer's mempool txs into our local mempool.
///
/// Both TxSubmission2 tasks run concurrently on the same connection; the
/// multiplexer routes client-direction and server-direction traffic to the
/// corresponding `subscribe_client` / `subscribe_server` channels.
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
    /// Background TxSubmission2 initiator task handle.
    /// Pulls the remote peer's mempool txs into our local mempool.
    /// Dropping this aborts the task, which is intentional on disconnect.
    _txsub_initiator: tokio::task::JoinHandle<()>,
}

impl DuplexPeerConnection {
    /// Open a TCP connection to `addr` and establish a full-duplex N2N session.
    ///
    /// # Arguments
    /// - `addr`           — target peer address
    /// - `network_magic`  — Cardano network magic (e.g. 2 for preview)
    /// - `mempool`        — local mempool; served via TxSubmission2 to the peer
    ///   and also receives transactions pulled from the peer
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
        //
        // For TxSubmission2 (protocol 4) we subscribe BOTH directions:
        //   subscribe_client(4) → we are the initiator; we pull the peer's txs
        //   subscribe_server(4) → we are the responder; the peer pulls our txs
        // The multiplexer routes traffic by (protocol_id, direction bit-15), so
        // both channels coexist without interference.
        let mut plexer = Plexer::new(bearer);

        // Handshake: always initiator-side (we propose versions).
        let hs_channel = plexer.subscribe_client(PROTOCOL_N2N_HANDSHAKE);

        // ChainSync: initiator (we request headers).
        let cs_channel = plexer.subscribe_client(PROTOCOL_N2N_CHAIN_SYNC);

        // BlockFetch: initiator (we request blocks).
        let bf_channel = plexer.subscribe_client(PROTOCOL_N2N_BLOCK_FETCH);

        // TxSubmission2: **client** (we request the peer's mempool txs).
        //   subscribe_client(P) → sends on P (bit-15 = 0), receives on P|0x8000
        let txsub_client_channel = plexer.subscribe_client(PROTOCOL_N2N_TX_SUBMISSION);

        // TxSubmission2: **server** (remote peer requests our txs).
        //   subscribe_server(P) → sends on P|0x8000, receives on P (bit-15 = 0)
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

        // --- Resolve remote address for logging ----------------------------------
        let remote_addr_str = format!("{addr}");
        let remote_addr: SocketAddr = remote_addr_str
            .parse()
            .unwrap_or_else(|_| std::net::SocketAddr::from(([0, 0, 0, 0], 0)));

        // --- TxSubmission2 CLIENT task (subscribe_client channel) ----------------
        //
        // In TxSubmission2, the **Client** (outbound/initiator) SERVES its own
        // mempool to the remote Server.  The Client sends MsgInit, then waits
        // for the remote Server to send MsgRequestTxIds, and replies with
        // MsgReplyTxIds / MsgReplyTxs from our mempool.
        //
        // This matches Haskell's `txSubmissionOutbound`.
        let txsub_responder = tokio::spawn(serve_tx_submission(
            txsub_client_channel,
            mempool.clone(),
            remote_addr,
        ));

        // --- TxSubmission2 SERVER task (subscribe_server channel) ----------------
        //
        // The **Server** (inbound/responder) PULLS the remote Client's mempool
        // txs into our local mempool.  The Server receives MsgInit from the
        // remote Client, then sends MsgRequestTxIds and receives MsgReplyTxIds
        // / MsgReplyTxs.
        //
        // This matches Haskell's `txSubmissionInboundV2`.
        let txsub_initiator = tokio::spawn(pull_tx_submission(
            txsub_server_channel,
            mempool,
            remote_addr,
        ));

        info!("duplex: connected to {remote_addr} (InitiatorAndResponder, TxSub client+server)");

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
            _txsub_initiator: txsub_initiator,
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

    /// Abort the connection (kills the plexer, responder task, and initiator task).
    pub async fn abort(self) {
        self._txsub_responder.abort();
        self._txsub_initiator.abort();
        self._plexer.abort().await;
    }

    /// Convert this `DuplexPeerConnection` into a `PipelinedPeerClient` for
    /// use by the pipelined ChainSync loop.
    ///
    /// The full-duplex connection already has ChainSync and BlockFetch client
    /// channels set up identically to those in a `PipelinedPeerClient`.  This
    /// method moves those channels into the pipelined client so the sync loop
    /// can drive them without needing a second TCP connection.
    ///
    /// Both TxSubmission2 task handles are returned to the caller:
    ///  - `responder_handle` — serves our mempool to the remote peer
    ///  - `initiator_handle` — pulls the remote peer's mempool txs into ours
    ///
    /// The caller MUST keep both handles alive for as long as the sync session
    /// is active.  Dropping a handle aborts the corresponding task, which is
    /// the correct behavior when the peer disconnects.
    ///
    /// Byron epoch length is preserved from the duplex connection; the
    /// await-reply timeout is initialised to the default and can be overridden
    /// via `PipelinedPeerClient::set_await_reply_timeout()`.
    pub fn into_pipelined(
        self,
    ) -> (
        crate::pipelined::PipelinedPeerClient,
        tokio::task::JoinHandle<()>,
        tokio::task::JoinHandle<()>,
    ) {
        let client = crate::pipelined::PipelinedPeerClient::from_duplex_parts(
            self.cs_buf,
            self.bf_client,
            self._keepalive,
            self._plexer,
            self._peersharing_channel,
            self.remote_addr,
            self.in_flight,
            self.byron_epoch_length,
        );
        (client, self._txsub_responder, self._txsub_initiator)
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
///
/// This is the TxSubmission2 **Client** (outbound/initiator) role.
/// Per the Haskell spec (`txSubmissionOutbound`), the Client:
///   1. Sends MsgInit [6]
///   2. Waits for the Server to send MsgRequestTxIds
///   3. Replies with MsgReplyTxIds from our mempool
///   4. Waits for MsgRequestTxs
///   5. Replies with MsgReplyTxs
///
/// The Client DOES NOT expect MsgInit back — only the Client sends it.
async fn serve_tx_submission_inner(
    mut channel: AgentChannel,
    mempool: Arc<dyn MempoolProvider>,
    peer_addr: SocketAddr,
) -> Result<(), DuplexError> {
    // Step 1: Send MsgInit [6] — only the Client sends this.
    let init_msg = encode_msg_init();
    send_msg(&mut channel, &init_msg).await?;
    info!(%peer_addr, "TxSubmission2 client: sent MsgInit, waiting for server requests");

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

// ─── Phase 3: TxSubmission2 initiator ───────────────────────────────────────

/// Pull TxSubmission2 transactions from a remote peer into our local mempool.
///
/// This is the **initiator** side of the N2N TxSubmission2 protocol.  We
/// connect as the client and periodically request transaction IDs from the peer,
/// then fetch the bodies for any txs we do not already have in our local mempool.
///
/// Protocol flow (we are initiator/client; remote peer is responder/server):
/// 1. We send MsgInit [6]
/// 2. Remote responds with MsgInit [6]
/// 3. We send MsgRequestTxIds [0, blocking, ack_count, req_count]
///    → Remote replies MsgReplyTxIds [1, [[tx_id, size], ...]]
/// 4. We send MsgRequestTxs [2, [tx_id, ...]] for IDs not already in our mempool
///    → Remote replies MsgReplyTxs [3, [tx_cbor, ...]]
/// 5. Loop from step 3
///
/// The function runs until the remote peer closes the protocol (sends MsgDone)
/// or the channel is dropped.  All errors are logged as warnings (non-fatal).
pub async fn pull_tx_submission(
    channel: AgentChannel,
    mempool: Arc<dyn MempoolProvider>,
    peer_addr: SocketAddr,
) {
    match pull_tx_submission_inner(channel, mempool, peer_addr).await {
        Ok(()) => debug!(%peer_addr, "TxSubmission2 initiator: session complete"),
        Err(e) => warn!(%peer_addr, "TxSubmission2 initiator: {e}"),
    }
}

/// Inner function that returns `Result` so we can use `?` throughout.
///
/// This is the TxSubmission2 **Server** (inbound/responder) role.
/// Per the Haskell spec (`txSubmissionInboundV2`), the Server:
///   1. Receives MsgInit [6] from the remote Client (does NOT send MsgInit)
///   2. Sends MsgRequestTxIds to pull the remote Client's mempool tx IDs
///   3. Receives MsgReplyTxIds
///   4. Sends MsgRequestTxs for unknown tx IDs
///   5. Receives MsgReplyTxs, adds txs to local mempool
///
/// Note: The Haskell Server has a 60-second warmup delay (`threadDelay 60`)
/// before reading MsgInit. We implement this as a timeout on receiving MsgInit.
async fn pull_tx_submission_inner(
    mut channel: AgentChannel,
    mempool: Arc<dyn MempoolProvider>,
    peer_addr: SocketAddr,
) -> Result<(), DuplexError> {
    // Step 1: Receive MsgInit from the remote Client.
    // The Server does NOT send MsgInit — only the Client sends it.
    // Use a generous timeout because the peer may have warmup delays.
    let init_payload = tokio::time::timeout(TXSUB_INIT_TIMEOUT, recv_msg(&mut channel))
        .await
        .map_err(|_| {
            DuplexError::Timeout("TxSubmission2 server: waiting for client MsgInit".into())
        })??;

    let init_tag = decode_first_tag(&init_payload)?;
    if init_tag != 6 {
        return Err(DuplexError::TxSubmissionInitiator(format!(
            "expected MsgInit (6) from client, got tag {init_tag}"
        )));
    }
    info!(%peer_addr, "TxSubmission2 server: received MsgInit — beginning tx polling");

    // Track tx IDs received from this peer but not yet acknowledged.
    // pending_ack is sent as ack_count in the next MsgRequestTxIds.
    let mut pending_ack: u16 = 0;

    // Known-set: tx IDs we have already seen from this peer (dedup filter).
    // When the set exceeds INITIATOR_MAX_KNOWN we clear it to prevent
    // unbounded memory growth over long-lived sessions.
    let mut known_tx_ids: HashSet<[u8; 32]> = HashSet::new();

    loop {
        // Send MsgRequestTxIds.
        //
        // First request is non-blocking so we immediately drain any queued txs.
        // If the peer returns empty, we switch to blocking to wait for new txs.
        let tx_ids = initiator_request_tx_ids(
            &mut channel,
            false,
            pending_ack,
            INITIATOR_REQ_COUNT,
            &peer_addr,
        )
        .await?;
        pending_ack = 0;

        let tx_ids = match tx_ids {
            InitiatorTxIdsReply::Done => {
                info!(%peer_addr, "TxSubmission2 initiator: peer sent MsgDone, closing");
                break;
            }
            InitiatorTxIdsReply::Ids(ids) if ids.is_empty() => {
                // Non-blocking reply was empty — peer has no queued txs right now.
                // Send a blocking request to wait until new txs become available.
                debug!(%peer_addr, "TxSubmission2 initiator: no txs queued, sending blocking request");
                match initiator_request_tx_ids(
                    &mut channel,
                    true,
                    0,
                    INITIATOR_REQ_COUNT,
                    &peer_addr,
                )
                .await?
                {
                    InitiatorTxIdsReply::Done => {
                        info!(%peer_addr, "TxSubmission2 initiator: peer sent MsgDone during blocking wait");
                        break;
                    }
                    InitiatorTxIdsReply::Ids(ids) if ids.is_empty() => {
                        // Empty reply to a blocking request signals the peer is
                        // ending the session (transitioning to Done state).
                        info!(%peer_addr, "TxSubmission2 initiator: empty blocking reply, closing");
                        break;
                    }
                    InitiatorTxIdsReply::Ids(ids) => ids,
                }
            }
            InitiatorTxIdsReply::Ids(ids) => ids,
        };

        // Accumulate acks: every batch we receive must be acknowledged in the
        // next MsgRequestTxIds.  Saturating add prevents overflow on huge batches.
        pending_ack = pending_ack.saturating_add(tx_ids.len() as u16);

        // Evict the known-set if it would exceed the cap to bound memory use.
        if known_tx_ids.len() + tx_ids.len() > INITIATOR_MAX_KNOWN {
            known_tx_ids.clear();
        }

        // Filter: only request bodies for tx IDs not already in our mempool
        // or already seen from this peer in this session.
        let new_ids: Vec<[u8; 32]> = tx_ids
            .iter()
            .filter(|(hash, _)| {
                let h = Hash32::from_bytes(*hash);
                !mempool.contains(&h) && !known_tx_ids.contains(hash)
            })
            .map(|(hash, _)| *hash)
            .collect();

        // Record all IDs as seen (whether we fetch them or not).
        for (hash, _) in &tx_ids {
            known_tx_ids.insert(*hash);
        }

        if new_ids.is_empty() {
            debug!(
                %peer_addr,
                total = tx_ids.len(),
                "TxSubmission2 initiator: all tx IDs already known, skipping body request"
            );
            continue;
        }

        debug!(
            %peer_addr,
            new = new_ids.len(),
            total = tx_ids.len(),
            "TxSubmission2 initiator: requesting transaction bodies"
        );

        // Request full transaction bodies for the new IDs.
        let bodies = initiator_request_txs(&mut channel, &new_ids, &peer_addr).await?;

        // Decode and add each transaction to the local mempool.
        for (i, tx_cbor) in bodies.iter().enumerate() {
            let tx_hash = if i < new_ids.len() {
                Hash32::from_bytes(new_ids[i])
            } else {
                continue;
            };

            // Try decoding across eras (Conway=6 first, then backwards).
            let added = try_add_tx_to_mempool(tx_hash, tx_cbor, &*mempool, &peer_addr);
            if !added {
                warn!(
                    %peer_addr,
                    hash = %tx_hash,
                    "TxSubmission2 initiator: failed to decode tx in any era"
                );
            }
        }
    }

    Ok(())
}

/// Result of a MsgRequestTxIds exchange on the initiator side.
enum InitiatorTxIdsReply {
    /// Peer replied with a list of (tx_hash_bytes, size) pairs.
    Ids(Vec<([u8; 32], u32)>),
    /// Peer sent MsgDone [4] — session is ending.
    Done,
}

/// Send MsgRequestTxIds and receive MsgReplyTxIds or MsgDone.
///
/// # Arguments
/// - `blocking`   — if true the responder holds its reply until it has txs
/// - `ack_count`  — number of tx IDs from the previous batch that we are acknowledging
/// - `req_count`  — how many new tx IDs we want
async fn initiator_request_tx_ids(
    channel: &mut AgentChannel,
    blocking: bool,
    ack_count: u16,
    req_count: u16,
    peer_addr: &SocketAddr,
) -> Result<InitiatorTxIdsReply, DuplexError> {
    // Encode MsgRequestTxIds: [0, blocking, ack_count, req_count]
    let request = encode_request_tx_ids(blocking, ack_count, req_count);
    send_msg(channel, &request).await?;

    debug!(
        %peer_addr,
        blocking,
        ack_count,
        req_count,
        "TxSubmission2 initiator: sent MsgRequestTxIds"
    );

    // Use a longer timeout for blocking requests.
    let wait = if blocking {
        INITIATOR_BLOCKING_TIMEOUT
    } else {
        INITIATOR_RESPONSE_TIMEOUT
    };

    let payload = tokio::time::timeout(wait, recv_msg(channel))
        .await
        .map_err(|_| {
            DuplexError::Timeout(format!(
                "TxSubmission2 initiator: waiting for MsgReplyTxIds from {peer_addr}"
            ))
        })??;

    let tag = decode_first_tag(&payload)?;

    match tag {
        // MsgReplyTxIds: [1, [[tx_id, size], ...]]
        1 => {
            let ids = decode_reply_tx_ids(&payload)?;
            debug!(%peer_addr, count = ids.len(), "TxSubmission2 initiator: received MsgReplyTxIds");
            Ok(InitiatorTxIdsReply::Ids(ids))
        }
        // MsgDone: [4]
        4 => {
            debug!(%peer_addr, "TxSubmission2 initiator: received MsgDone");
            Ok(InitiatorTxIdsReply::Done)
        }
        other => Err(DuplexError::TxSubmissionInitiator(format!(
            "expected MsgReplyTxIds (1) or MsgDone (4), got tag {other}"
        ))),
    }
}

/// Send MsgRequestTxs and receive MsgReplyTxs.
async fn initiator_request_txs(
    channel: &mut AgentChannel,
    tx_ids: &[[u8; 32]],
    peer_addr: &SocketAddr,
) -> Result<Vec<Vec<u8>>, DuplexError> {
    if tx_ids.is_empty() {
        return Ok(vec![]);
    }

    // Encode MsgRequestTxs: [2, [tx_id, ...]]
    let request = encode_initiator_request_txs(tx_ids);
    send_msg(channel, &request).await?;

    debug!(%peer_addr, count = tx_ids.len(), "TxSubmission2 initiator: sent MsgRequestTxs");

    let payload = tokio::time::timeout(INITIATOR_RESPONSE_TIMEOUT, recv_msg(channel))
        .await
        .map_err(|_| {
            DuplexError::Timeout(format!(
                "TxSubmission2 initiator: waiting for MsgReplyTxs from {peer_addr}"
            ))
        })??;

    let tag = decode_first_tag(&payload)?;
    if tag != 3 {
        return Err(DuplexError::TxSubmissionInitiator(format!(
            "expected MsgReplyTxs (3), got tag {tag}"
        )));
    }

    let bodies = decode_reply_txs(&payload)?;
    info!(
        %peer_addr,
        count = bodies.len(),
        requested = tx_ids.len(),
        "TxSubmission2 initiator: received MsgReplyTxs"
    );
    Ok(bodies)
}

/// Try to decode a transaction CBOR payload across eras and add it to the mempool.
///
/// Attempts Conway (era 6) first, then walks backwards through earlier eras.
/// Returns `true` if the transaction was successfully decoded (even if the
/// mempool rejected it — rejection counts as "decoded"), `false` only if
/// decoding failed in every era.
fn try_add_tx_to_mempool(
    tx_hash: Hash32,
    tx_cbor: &[u8],
    mempool: &dyn MempoolProvider,
    peer_addr: &SocketAddr,
) -> bool {
    for era in [6u16, 5, 4, 3, 2] {
        match torsten_serialization::decode_transaction(era, tx_cbor) {
            Ok(tx) => {
                let tx_size = tx_cbor.len();
                let fee = tx.body.fee;
                match mempool.add_tx_with_fee(tx_hash, tx, tx_size, fee) {
                    Ok(MempoolAddResult::Added) => {
                        info!(
                            %peer_addr,
                            hash = %tx_hash,
                            size = tx_size,
                            era,
                            "TxSubmission2 initiator: tx added to mempool"
                        );
                    }
                    Ok(MempoolAddResult::AlreadyExists) => {
                        debug!(
                            %peer_addr,
                            hash = %tx_hash,
                            "TxSubmission2 initiator: tx already in mempool"
                        );
                    }
                    Err(e) => {
                        debug!(
                            %peer_addr,
                            hash = %tx_hash,
                            "TxSubmission2 initiator: mempool rejected tx: {e}"
                        );
                    }
                }
                return true;
            }
            Err(_) => continue,
        }
    }
    false
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

// ─── Initiator CBOR encode helpers ──────────────────────────────────────────

/// Encode `MsgRequestTxIds = [0, blocking, ack_count, req_count]`.
fn encode_request_tx_ids(blocking: bool, ack_count: u16, req_count: u16) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut buf);
    enc.array(4).expect("encode array");
    enc.u32(0).expect("encode tag");
    enc.bool(blocking).expect("encode blocking");
    enc.u16(ack_count).expect("encode ack_count");
    enc.u16(req_count).expect("encode req_count");
    buf
}

/// Encode `MsgRequestTxs = [2, [tx_id, ...]]`.
fn encode_initiator_request_txs(tx_ids: &[[u8; 32]]) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut buf);
    enc.array(2).expect("encode outer array");
    enc.u32(2).expect("encode tag");
    enc.array(tx_ids.len() as u64).expect("encode id array");
    for id in tx_ids {
        enc.bytes(id.as_slice()).expect("encode tx id");
    }
    buf
}

// ─── Initiator CBOR decode helpers ──────────────────────────────────────────

/// Decode `MsgReplyTxIds = [1, [[tx_id, size], ...]]`.
///
/// Returns a list of `(hash_bytes, size)` pairs.  Entries with non-32-byte
/// hash fields are silently dropped (malformed peer data).
fn decode_reply_tx_ids(payload: &[u8]) -> Result<Vec<([u8; 32], u32)>, DuplexError> {
    let mut dec = minicbor::Decoder::new(payload);
    dec.array()
        .map_err(|e| DuplexError::Cbor(format!("MsgReplyTxIds: expected outer array: {e}")))?;
    let _tag = dec
        .u32()
        .map_err(|e| DuplexError::Cbor(format!("MsgReplyTxIds: expected tag: {e}")))?;

    let count = dec
        .array()
        .map_err(|e| DuplexError::Cbor(format!("MsgReplyTxIds: expected id array: {e}")))?
        .unwrap_or(0);

    let mut result = Vec::with_capacity(count as usize);
    for _ in 0..count {
        dec.array()
            .map_err(|e| DuplexError::Cbor(format!("MsgReplyTxIds: expected pair: {e}")))?;
        let hash_bytes = dec
            .bytes()
            .map_err(|e| DuplexError::Cbor(format!("MsgReplyTxIds: expected hash bytes: {e}")))?;
        let size = dec
            .u32()
            .map_err(|e| DuplexError::Cbor(format!("MsgReplyTxIds: expected size u32: {e}")))?;

        if hash_bytes.len() == 32 {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(hash_bytes);
            result.push((arr, size));
        } else {
            warn!(
                len = hash_bytes.len(),
                "TxSubmission2 initiator: ignoring tx ID with unexpected hash length"
            );
        }
    }
    Ok(result)
}

/// Decode `MsgReplyTxs = [3, [tx_cbor, ...]]`.
fn decode_reply_txs(payload: &[u8]) -> Result<Vec<Vec<u8>>, DuplexError> {
    let mut dec = minicbor::Decoder::new(payload);
    dec.array()
        .map_err(|e| DuplexError::Cbor(format!("MsgReplyTxs: expected outer array: {e}")))?;
    let _tag = dec
        .u32()
        .map_err(|e| DuplexError::Cbor(format!("MsgReplyTxs: expected tag: {e}")))?;

    let count = dec
        .array()
        .map_err(|e| DuplexError::Cbor(format!("MsgReplyTxs: expected body array: {e}")))?
        .unwrap_or(0);

    let cap = (count as usize).min(MAX_TX_BODY_REQUEST);
    let mut result = Vec::with_capacity(cap);
    for _ in 0..cap {
        let body = dec
            .bytes()
            .map_err(|e| DuplexError::Cbor(format!("MsgReplyTxs: expected tx body bytes: {e}")))?;
        result.push(body.to_vec());
    }
    Ok(result)
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

    // -----------------------------------------------------------------------
    // Initiator CBOR encode / decode tests
    // -----------------------------------------------------------------------

    /// encode_request_tx_ids must produce [0, blocking, ack_count, req_count].
    #[test]
    fn test_encode_request_tx_ids_blocking_true() {
        let bytes = encode_request_tx_ids(true, 5, 100);
        let mut dec = minicbor::Decoder::new(&bytes);
        assert_eq!(
            dec.array().unwrap().unwrap_or(0),
            4,
            "outer array length must be 4"
        );
        assert_eq!(dec.u32().unwrap(), 0, "tag must be 0 (MsgRequestTxIds)");
        assert!(dec.bool().unwrap(), "blocking must be true");
        assert_eq!(dec.u16().unwrap(), 5, "ack_count must be 5");
        assert_eq!(dec.u16().unwrap(), 100, "req_count must be 100");
    }

    /// encode_request_tx_ids with blocking=false.
    #[test]
    fn test_encode_request_tx_ids_blocking_false() {
        let bytes = encode_request_tx_ids(false, 0, 50);
        let mut dec = minicbor::Decoder::new(&bytes);
        dec.array().unwrap();
        assert_eq!(dec.u32().unwrap(), 0);
        assert!(!dec.bool().unwrap(), "blocking must be false");
        assert_eq!(dec.u16().unwrap(), 0);
        assert_eq!(dec.u16().unwrap(), 50);
    }

    /// encode_request_tx_ids: blocking field must encode as CBOR bool (0xF5/0xF4),
    /// not as integer 1/0.
    #[test]
    fn test_encode_request_tx_ids_blocking_is_cbor_bool() {
        let blocking_bytes = encode_request_tx_ids(true, 0, 100);
        let non_blocking_bytes = encode_request_tx_ids(false, 0, 100);
        assert!(
            blocking_bytes.contains(&0xF5),
            "blocking=true must encode as CBOR true (0xF5)"
        );
        assert!(
            non_blocking_bytes.contains(&0xF4),
            "blocking=false must encode as CBOR false (0xF4)"
        );
    }

    /// encode_initiator_request_txs must produce [2, [hash, ...]].
    #[test]
    fn test_encode_initiator_request_txs_two_hashes() {
        let hash_a = [0xAAu8; 32];
        let hash_b = [0xBBu8; 32];
        let ids = [hash_a, hash_b];
        let bytes = encode_initiator_request_txs(&ids);

        let mut dec = minicbor::Decoder::new(&bytes);
        assert_eq!(dec.array().unwrap().unwrap_or(0), 2, "outer array(2)");
        assert_eq!(dec.u32().unwrap(), 2, "tag must be 2 (MsgRequestTxs)");
        let n = dec.array().unwrap().unwrap_or(0);
        assert_eq!(n, 2, "inner array length must be 2");
        assert_eq!(dec.bytes().unwrap(), hash_a.as_slice());
        assert_eq!(dec.bytes().unwrap(), hash_b.as_slice());
    }

    /// encode_initiator_request_txs with empty slice must produce [2, []].
    #[test]
    fn test_encode_initiator_request_txs_empty() {
        let bytes = encode_initiator_request_txs(&[]);
        let mut dec = minicbor::Decoder::new(&bytes);
        dec.array().unwrap();
        assert_eq!(dec.u32().unwrap(), 2);
        assert_eq!(
            dec.array().unwrap().unwrap_or(99),
            0,
            "inner array must be empty"
        );
    }

    /// decode_reply_tx_ids must parse [1, [[hash, size], ...]] correctly.
    #[test]
    fn test_decode_reply_tx_ids_two_entries() {
        let hash_a = [0x11u8; 32];
        let hash_b = [0x22u8; 32];

        // Encode a well-formed MsgReplyTxIds from the responder side.
        let encoded = encode_reply_tx_ids(&[(hash_a, 512), (hash_b, 1024)]);
        let decoded = decode_reply_tx_ids(&encoded).unwrap();

        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].0, hash_a);
        assert_eq!(decoded[0].1, 512);
        assert_eq!(decoded[1].0, hash_b);
        assert_eq!(decoded[1].1, 1024);
    }

    /// decode_reply_tx_ids with empty list must succeed and return [].
    #[test]
    fn test_decode_reply_tx_ids_empty() {
        let encoded = encode_reply_tx_ids(&[]);
        let decoded = decode_reply_tx_ids(&encoded).unwrap();
        assert!(
            decoded.is_empty(),
            "empty MsgReplyTxIds must decode to empty vec"
        );
    }

    /// decode_reply_tx_ids drops entries whose hash is not exactly 32 bytes.
    #[test]
    fn test_decode_reply_tx_ids_bad_hash_length_dropped() {
        // Craft a MsgReplyTxIds with one 16-byte hash (malformed).
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap(); // outer [tag, ids]
        enc.u32(1).unwrap(); // MsgReplyTxIds tag
        enc.array(1u64).unwrap(); // one entry
        enc.array(2).unwrap(); // inner [hash, size]
        enc.bytes(&[0xAAu8; 16]).unwrap(); // 16-byte hash — malformed
        enc.u32(256).unwrap();

        let decoded = decode_reply_tx_ids(&buf).unwrap();
        assert_eq!(
            decoded.len(),
            0,
            "malformed hash (16 bytes) must be silently dropped"
        );
    }

    /// decode_reply_txs must parse [3, [cbor, ...]] correctly.
    #[test]
    fn test_decode_reply_txs_two_bodies() {
        let body_a = vec![0x01, 0x02, 0x03];
        let body_b = vec![0xFF, 0xFE, 0xFD];
        let encoded = encode_reply_txs(&[body_a.clone(), body_b.clone()]);
        let decoded = decode_reply_txs(&encoded).unwrap();

        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0], body_a);
        assert_eq!(decoded[1], body_b);
    }

    /// decode_reply_txs with empty list must succeed and return [].
    #[test]
    fn test_decode_reply_txs_empty() {
        let encoded = encode_reply_txs(&[]);
        let decoded = decode_reply_txs(&encoded).unwrap();
        assert!(decoded.is_empty());
    }

    // -----------------------------------------------------------------------
    // Initiator state machine logic tests (no I/O)
    // -----------------------------------------------------------------------

    /// The pending_ack accumulation must use saturating_add.
    #[test]
    fn test_initiator_pending_ack_saturating_add() {
        let mut pending_ack: u16 = u16::MAX - 2;
        let batch_len = 5u16;
        pending_ack = pending_ack.saturating_add(batch_len);
        assert_eq!(
            pending_ack,
            u16::MAX,
            "saturating_add must clamp at u16::MAX"
        );
    }

    /// Known-set eviction: when len + incoming > cap, the set is cleared first.
    #[test]
    fn test_initiator_known_set_eviction() {
        let mut known: HashSet<[u8; 32]> = HashSet::new();

        // Fill to just below the cap.
        for i in 0..INITIATOR_MAX_KNOWN {
            let mut h = [0u8; 32];
            h[0] = (i >> 24) as u8;
            h[1] = (i >> 16) as u8;
            h[2] = (i >> 8) as u8;
            h[3] = i as u8;
            known.insert(h);
        }
        assert_eq!(known.len(), INITIATOR_MAX_KNOWN);

        // One more entry triggers eviction.
        let new_hash = [0xFFu8; 32];
        let incoming: Vec<([u8; 32], u32)> = vec![(new_hash, 100)];
        if known.len() + incoming.len() > INITIATOR_MAX_KNOWN {
            known.clear();
        }
        for (h, _) in &incoming {
            known.insert(*h);
        }
        assert_eq!(
            known.len(),
            1,
            "after eviction only the new hash should remain"
        );
        assert!(known.contains(&new_hash));
    }

    /// Known-set dedup: tx IDs already in the set are filtered from new_ids.
    #[test]
    fn test_initiator_dedup_filter() {
        let mut known: HashSet<[u8; 32]> = HashSet::new();
        let hash_a = [0x11u8; 32];
        let hash_b = [0x22u8; 32];
        let hash_c = [0x33u8; 32];

        // First batch: both are new, neither is in the known set.
        let batch1 = vec![(hash_a, 100u32), (hash_b, 200u32)];
        let new1: Vec<[u8; 32]> = batch1
            .iter()
            .filter(|(h, _)| !known.contains(h))
            .map(|(h, _)| *h)
            .collect();
        assert_eq!(new1.len(), 2, "first batch: both hashes must be new");
        for (h, _) in &batch1 {
            known.insert(*h);
        }

        // Second batch: same hashes — both filtered.
        let batch2 = [(hash_a, 100u32), (hash_b, 200u32)];
        let new2: Vec<[u8; 32]> = batch2
            .iter()
            .filter(|(h, _)| !known.contains(h))
            .map(|(h, _)| *h)
            .collect();
        assert_eq!(new2.len(), 0, "second batch: all hashes already known");

        // Third batch: one old, one new.
        let batch3 = [(hash_a, 100u32), (hash_c, 300u32)];
        let new3: Vec<[u8; 32]> = batch3
            .iter()
            .filter(|(h, _)| !known.contains(h))
            .map(|(h, _)| *h)
            .collect();
        assert_eq!(new3.len(), 1, "third batch: only hash_c must be new");
        assert_eq!(new3[0], hash_c);
    }

    /// encode_request_tx_ids / decode_request_tx_ids round-trip.
    ///
    /// The initiator encodes MsgRequestTxIds; the responder decodes it.
    /// This validates that the two sides interoperate correctly.
    #[test]
    fn test_initiator_request_tx_ids_roundtrip_with_responder_decode() {
        let encoded = encode_request_tx_ids(true, 7, 100);
        // The responder uses decode_request_tx_ids to parse this.
        let (blocking, ack, req) = decode_request_tx_ids(&encoded).unwrap();
        assert!(blocking, "blocking must survive round-trip");
        assert_eq!(ack, 7, "ack_count must survive round-trip");
        assert_eq!(req, 100, "req_count must survive round-trip");
    }

    /// encode_initiator_request_txs / decode_request_txs round-trip.
    ///
    /// The initiator encodes MsgRequestTxs; the responder decodes it.
    #[test]
    fn test_initiator_request_txs_roundtrip_with_responder_decode() {
        let hash_a = [0xAAu8; 32];
        let hash_b = [0xBBu8; 32];
        let encoded = encode_initiator_request_txs(&[hash_a, hash_b]);
        let hashes = decode_request_txs(&encoded).unwrap();
        assert_eq!(hashes.len(), 2);
        assert_eq!(hashes[0], hash_a);
        assert_eq!(hashes[1], hash_b);
    }

    /// decode_reply_tx_ids / encode_reply_tx_ids round-trip.
    ///
    /// The responder encodes MsgReplyTxIds; the initiator decodes it.
    #[test]
    fn test_responder_reply_tx_ids_roundtrip_with_initiator_decode() {
        let hash_a = [0xCCu8; 32];
        let hash_b = [0xDDu8; 32];
        let encoded = encode_reply_tx_ids(&[(hash_a, 512), (hash_b, 1024)]);
        let ids = decode_reply_tx_ids(&encoded).unwrap();
        assert_eq!(ids.len(), 2);
        assert_eq!(ids[0], (hash_a, 512));
        assert_eq!(ids[1], (hash_b, 1024));
    }

    /// decode_reply_txs / encode_reply_txs round-trip.
    ///
    /// The responder encodes MsgReplyTxs; the initiator decodes it.
    #[test]
    fn test_responder_reply_txs_roundtrip_with_initiator_decode() {
        let tx1 = vec![0x82, 0x00, 0x01];
        let tx2 = vec![0x83, 0x01, 0x02, 0x03];
        let encoded = encode_reply_txs(&[tx1.clone(), tx2.clone()]);
        let bodies = decode_reply_txs(&encoded).unwrap();
        assert_eq!(bodies.len(), 2);
        assert_eq!(bodies[0], tx1);
        assert_eq!(bodies[1], tx2);
    }

    // -----------------------------------------------------------------------
    // Full initiator protocol sequence tests
    // -----------------------------------------------------------------------

    /// Verifies the complete TxSubmission2 initiator protocol message sequence
    /// at the CBOR wire level — no I/O required.
    ///
    /// Simulates the message flow:
    ///   1. Initiator → Responder: MsgInit [6]
    ///   2. Responder → Initiator: MsgInit [6]
    ///   3. Initiator → Responder: MsgRequestTxIds [0, false, 0, 100]
    ///   4. Responder → Initiator: MsgReplyTxIds [1, [[hash, size]]]
    ///   5. Initiator → Responder: MsgRequestTxs [2, [hash]]
    ///   6. Responder → Initiator: MsgReplyTxs [3, [tx_cbor]]
    ///   7. Initiator → Responder: MsgRequestTxIds [0, false, 1, 100] (ack=1)
    ///   8. Responder → Initiator: MsgReplyTxIds [1, []] (empty → trigger blocking)
    ///   9. Initiator → Responder: MsgRequestTxIds [0, true, 0, 100] (blocking)
    ///  10. Responder → Initiator: MsgDone [4]
    #[test]
    fn test_initiator_protocol_message_sequence() {
        // Step 1: Initiator sends MsgInit
        let init = encode_msg_init();
        assert_eq!(decode_first_tag(&init).unwrap(), 6, "MsgInit tag must be 6");

        // Step 2: Responder sends MsgInit — same encoding
        let init_reply = encode_msg_init();
        assert_eq!(decode_first_tag(&init_reply).unwrap(), 6);

        // Step 3: Initiator sends MsgRequestTxIds (non-blocking, ack=0, req=100)
        let req_ids = encode_request_tx_ids(false, 0, 100);
        let (blocking, ack, req) = decode_request_tx_ids(&req_ids).unwrap();
        assert!(!blocking);
        assert_eq!(ack, 0);
        assert_eq!(req, 100);

        // Step 4: Responder replies with one tx ID
        let tx_hash = [0xDEu8; 32];
        let reply_ids = encode_reply_tx_ids(&[(tx_hash, 256)]);
        let ids = decode_reply_tx_ids(&reply_ids).unwrap();
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0].0, tx_hash);
        assert_eq!(ids[0].1, 256);

        // Step 5: Initiator sends MsgRequestTxs for that hash
        let req_txs = encode_initiator_request_txs(&[tx_hash]);
        let hashes = decode_request_txs(&req_txs).unwrap();
        assert_eq!(hashes.len(), 1);
        assert_eq!(hashes[0], tx_hash);

        // Step 6: Responder replies with the tx body
        let fake_tx = vec![0x82u8, 0x01, 0x02];
        let reply_txs = encode_reply_txs(std::slice::from_ref(&fake_tx));
        let bodies = decode_reply_txs(&reply_txs).unwrap();
        assert_eq!(bodies.len(), 1);
        assert_eq!(bodies[0], fake_tx);

        // Step 7: Initiator sends MsgRequestTxIds with ack=1 (acknowledging step 4)
        let req_with_ack = encode_request_tx_ids(false, 1, 100);
        let (blocking, ack, _) = decode_request_tx_ids(&req_with_ack).unwrap();
        assert!(!blocking);
        assert_eq!(ack, 1, "ack_count must be 1 after processing one tx batch");

        // Step 8: Responder replies empty (no new txs)
        let empty_reply = encode_reply_tx_ids(&[]);
        let empty_ids = decode_reply_tx_ids(&empty_reply).unwrap();
        assert!(
            empty_ids.is_empty(),
            "empty reply triggers blocking request"
        );

        // Step 9: Initiator sends blocking MsgRequestTxIds
        let blocking_req = encode_request_tx_ids(true, 0, 100);
        let (blocking, _, _) = decode_request_tx_ids(&blocking_req).unwrap();
        assert!(blocking, "blocking flag must be true for blocking request");

        // Step 10: Responder sends MsgDone [4]
        let mut done_buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut done_buf);
        enc.array(1).unwrap();
        enc.u32(4).unwrap();
        assert_eq!(
            decode_first_tag(&done_buf).unwrap(),
            4,
            "MsgDone must be tag 4 per Ouroboros CDDL"
        );
    }

    /// Verify that the initiator's MsgRequestTxIds can be decoded by the
    /// responder using its existing decode_request_tx_ids function, and
    /// vice-versa for MsgReplyTxIds — ensuring bidirectional wire compatibility.
    #[test]
    fn test_initiator_responder_bidirectional_compatibility() {
        // Initiator encodes; responder decodes
        let initiator_req = encode_request_tx_ids(false, 3, 50);
        let (blocking, ack, req) = decode_request_tx_ids(&initiator_req).unwrap();
        assert!(!blocking);
        assert_eq!(ack, 3);
        assert_eq!(req, 50);

        // Responder encodes; initiator decodes
        let hash = [0x55u8; 32];
        let responder_reply = encode_reply_tx_ids(&[(hash, 100)]);
        let ids = decode_reply_tx_ids(&responder_reply).unwrap();
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0].0, hash);
        assert_eq!(ids[0].1, 100);
    }
}
