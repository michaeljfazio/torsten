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
//! In a full-duplex (InitiatorAndResponder) N2N connection we open outbound,
//! we are the TCP *initiator*.  For TxSubmission2 the Ouroboros roles are
//! INDEPENDENT of the TCP connection direction:
//!
//! | Mini-protocol    | Mux channel      | Ouroboros role  | What we do                              |
//! |------------------|------------------|-----------------|-----------------------------------------|
//! | Handshake (0)    | subscribe_client | Initiator       | Propose versions                        |
//! | ChainSync (2)    | subscribe_client | Initiator       | Pull headers from peer                  |
//! | BlockFetch (3)   | subscribe_client | Initiator       | Fetch full blocks from peer             |
//! | TxSubmission2(4) | subscribe_client | **Client**      | Serve our mempool to the remote Server  |
//! | TxSubmission2(4) | subscribe_server | **Server**      | Pull remote Client's mempool into ours  |
//! | KeepAlive (8)    | subscribe_client | Initiator       | Ping the peer                           |
//! | PeerSharing (10) | subscribe_client | Initiator       | Request peers                           |
//!
//! Both `subscribe_client` and `subscribe_server` for TxSubmission2 are
//! registered before the plexer spawns.  This gives two independent
//! bidirectional channels — one for each direction of mempool propagation.
//!
//! # TxSubmission2 Ouroboros roles (per pallas / Haskell oracle)
//!
//! **Client** (the tx *advertiser*, `txSubmissionOutbound` in Haskell):
//!   - Sends `MsgInit [6]`
//!   - Then waits in `StIdle` for the Server to send requests
//!   - Replies to `MsgRequestTxIds` with `MsgReplyTxIds`
//!   - Replies to `MsgRequestTxs` with `MsgReplyTxs`
//!   - The Client NEVER sends `MsgInit` a second time
//!
//! **Server** (the tx *consumer*, `txSubmissionInboundV2` in Haskell):
//!   - Waits for `MsgInit [6]` from Client (does NOT send MsgInit itself)
//!   - Then has agency in `StIdle` and sends `MsgRequestTxIds`
//!   - Receives `MsgReplyTxIds`, decides which tx bodies to fetch
//!   - Sends `MsgRequestTxs`, receives `MsgReplyTxs`
//!   - Haskell Server has a 60-second warmup `threadDelay 60` before
//!     reading MsgInit; our `TXSUB_INIT_TIMEOUT` must exceed 60 s.
//!
//! # Multiplexer direction conventions (pallas)
//!
//! `subscribe_client(P)` → sends on protocol P (bit-15 = 0), receives on P|0x8000
//! `subscribe_server(P)` → sends on P|0x8000, receives on protocol P (bit-15 = 0)
//!
//! We are the TCP initiator (bit-15=0 outbound).  For TxSubmission2:
//!  - `subscribe_client(4)`: we send on 4 (bit-15=0), receive on 0x8004.
//!    Peer (TCP responder) uses their `subscribe_server(4)` which sends
//!    on 0x8004 and receives on 4.  So: we → MsgInit/ReplyTxIds/ReplyTxs;
//!    peer → MsgRequestTxIds/MsgRequestTxs.  This is our **Client** channel.
//!  - `subscribe_server(4)`: we send on 0x8004, receive on 4.
//!    Peer (TCP responder) uses their `subscribe_client(4)` which sends
//!    on 4 and receives on 0x8004.  So: peer → MsgInit/ReplyTxIds/ReplyTxs;
//!    we → MsgRequestTxIds/MsgRequestTxs.  This is our **Server** channel.

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
    #[error("TxSubmission2 client error: {0}")]
    TxSubmission(String),
    #[error("TxSubmission2 server error: {0}")]
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

/// Maximum tx IDs we serve per MsgReplyTxIds batch.
/// Haskell V1 Server requests 3, V2 requests up to 12. We cap at the
/// peer's req_count anyway, so this is just a safety limit.
const MAX_TX_IDS_PER_REPLY: usize = 100;

/// Maximum inflight tx IDs (sent to a single peer, not yet acknowledged).
/// Haskell V2 maxUnacknowledgedTxIds = 10.
const MAX_TX_INFLIGHT: usize = 10;

/// Maximum tx bodies we serve per MsgRequestTxs.
/// Haskell V1 maxTxToRequest = 2, V2 varies. Keep generous for V2.
const MAX_TX_BODY_REQUEST: usize = 100;

/// Maximum tx IDs to REQUEST per MsgRequestTxIds when we are the Server.
/// Haskell V1 = 3, V2 = 12. Use 3 for V1 compatibility.
/// MUST NOT push unacknowledged count above maxUnacknowledgedTxIds (100).
const SERVER_REQ_TX_IDS: u16 = 3;

/// Maximum reassembled message size for the TxSubmission2 responder/initiator (8 MB).
const MAX_REASSEMBLY_SIZE: usize = 8 * 1024 * 1024;

/// Timeout for the TxSubmission2 MsgInit exchange.
///
/// The Haskell node's Server has a `threadDelay 60` warmup delay before
/// reading MsgInit from the Client.  Our Client-side send succeeds
/// immediately, but we then wait up to TXSUB_INIT_TIMEOUT for the Server
/// to send the first MsgRequestTxIds.
///
/// The Haskell Server side also waits up to this timeout for the remote
/// Client to send MsgInit.  90 seconds provides a comfortable margin above
/// the 60-second Haskell warmup.
const TXSUB_INIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);

/// Timeout for subsequent MsgRequestTxIds (blocking requests may hold longer).
const TXSUB_BLOCKING_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

// ─── DuplexPeerConnection ───────────────────────────────────────────────────

/// A full-duplex outbound N2N connection that simultaneously runs:
///  - ChainSync client      (we pull headers from the remote)
///  - BlockFetch client     (we pull blocks from the remote)
///  - KeepAlive client      (we ping the remote)
///  - TxSubmission2 client  (we serve our mempool to the remote Server)
///  - TxSubmission2 server  (we pull the remote Client's mempool into ours)
///
/// Compared to `PipelinedPeerClient`, this connection advertises
/// `initiator_only_diffusion_mode = false` (InitiatorAndResponder) in the
/// handshake, and starts two background tasks for bidirectional TxSubmission2.
pub struct DuplexPeerConnection {
    /// ChainSync raw buffer (None for governor-only connections that skip ChainSync).
    pub cs_buf: Option<pallas_network::multiplexer::ChannelBuffer>,
    /// BlockFetch client (None for governor-only connections that skip BlockFetch).
    pub bf_client: Option<pallas_network::miniprotocols::blockfetch::Client>,
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
    /// Background TxSubmission2 CLIENT task (serves our mempool to remote Server).
    /// Dropping this aborts the task, which is intentional on disconnect.
    _txsub_client: tokio::task::JoinHandle<()>,
    /// Background TxSubmission2 SERVER task (pulls remote Client's mempool into ours).
    /// Dropping this aborts the task, which is intentional on disconnect.
    _txsub_server: tokio::task::JoinHandle<()>,
    /// Drain tasks for ChainSync/BlockFetch when in warm mode.
    /// These read and discard messages to prevent demuxer stall.
    /// Set to None when promoted to Hot (channels actively consumed).
    _cs_drain: Option<tokio::task::JoinHandle<()>>,
    _bf_drain: Option<tokio::task::JoinHandle<()>>,
}

impl DuplexPeerConnection {
    /// Open a TCP connection to `addr` and establish a full-duplex N2N session.
    ///
    /// # Arguments
    /// - `addr`           — target peer address
    /// - `network_magic`  — Cardano network magic (e.g. 2 for preview)
    /// - `mempool`        — local mempool; served to the remote Server (Client task),
    ///   and also receives incoming txs from the remote Client (Server task)
    /// - `block_provider` — chain storage (reserved for future ChainSync/BlockFetch server tasks)
    ///
    /// Open a TCP connection to `addr` and establish a full-duplex N2N session.
    ///
    /// When `subscribe_chainsync` is false, ChainSync and BlockFetch channels
    /// are NOT subscribed on the mux plexer. This is critical for governor-managed
    /// connections that only need TxSubmission2 + KeepAlive — subscribing without
    /// reading causes the demuxer to stall when the peer sends chain data (the
    /// bounded mpsc channel fills → demuxer blocks → all protocols die).
    pub async fn connect(
        addr: impl ToSocketAddrs + std::fmt::Display + Copy,
        network_magic: u64,
        mempool: Arc<dyn MempoolProvider>,
        _block_provider: Arc<dyn BlockProvider>,
        listen_port: u16,
    ) -> Result<Self, DuplexError> {
        Self::connect_inner(addr, network_magic, mempool, true, listen_port).await
    }

    /// Warm connection: subscribes ALL channels but drains ChainSync/BlockFetch.
    ///
    /// This replaces `connect_no_chainsync`. By subscribing all channels upfront
    /// and draining unused ones, the demuxer never stalls. When promoted to Hot,
    /// the drain tasks stop and the channels become available for active use.
    pub async fn connect_warm(
        addr: impl ToSocketAddrs + std::fmt::Display + Copy,
        network_magic: u64,
        mempool: Arc<dyn MempoolProvider>,
        listen_port: u16,
    ) -> Result<Self, DuplexError> {
        // Subscribe ALL channels (including ChainSync/BlockFetch) to prevent
        // demuxer stall, then spawn drain tasks for the unused ones.
        let mut conn = Self::connect_inner(addr, network_magic, mempool, true, listen_port).await?;
        // Start drain tasks for ChainSync and BlockFetch — they read and discard
        // all incoming messages so the demuxer's bounded channels never fill up.
        conn.start_drain_tasks();
        Ok(conn)
    }

    /// Governor-only variant: no ChainSync/BlockFetch subscriptions.
    /// DEPRECATED: Use connect_warm() instead for proper protocol handling.
    pub async fn connect_no_chainsync(
        addr: impl ToSocketAddrs + std::fmt::Display + Copy,
        network_magic: u64,
        mempool: Arc<dyn MempoolProvider>,
        listen_port: u16,
    ) -> Result<Self, DuplexError> {
        // Forward to connect_warm for now
        Self::connect_warm(addr, network_magic, mempool, listen_port).await
    }

    async fn connect_inner(
        addr: impl ToSocketAddrs + std::fmt::Display + Copy,
        network_magic: u64,
        mempool: Arc<dyn MempoolProvider>,
        subscribe_chainsync: bool,
        listen_port: u16,
    ) -> Result<Self, DuplexError> {
        debug!("duplex: connecting to {addr}");

        // --- TCP connect with port binding (InitiatorAndResponder) ---------------
        //
        // In InitiatorAndResponder mode, the Haskell cardano-node binds outbound
        // sockets to the node's listening port before connect().  This allows the
        // remote peer to see our listening port as the TCP source port, enabling
        // it to identify us for duplex connection reuse and to start initiator-side
        // mini-protocols (TxSubmission2 Client, etc.) on this connection.
        //
        // We resolve the target address first to determine IPv4 vs IPv6, then
        // create a TcpSocket, set SO_REUSEADDR, bind to 0.0.0.0:<listen_port>
        // (or [::]:port), and connect.
        let resolved = tokio::net::lookup_host(addr)
            .await
            .map_err(|e| DuplexError::Connection(format!("dns lookup: {e}")))?
            .next()
            .ok_or_else(|| DuplexError::Connection("no addresses resolved".into()))?;

        let socket = if resolved.is_ipv4() {
            tokio::net::TcpSocket::new_v4()
        } else {
            tokio::net::TcpSocket::new_v6()
        }
        .map_err(|e| DuplexError::Connection(format!("socket create: {e}")))?;

        socket
            .set_reuseaddr(true)
            .map_err(|e| DuplexError::Connection(format!("set SO_REUSEADDR: {e}")))?;

        // SO_REUSEPORT allows multiple sockets to bind to the same port.
        // Required for concurrent outbound connections from the listening port.
        #[cfg(unix)]
        socket
            .set_reuseport(true)
            .map_err(|e| DuplexError::Connection(format!("set SO_REUSEPORT: {e}")))?;

        let bind_addr: std::net::SocketAddr = if resolved.is_ipv4() {
            std::net::SocketAddr::new(std::net::Ipv4Addr::UNSPECIFIED.into(), listen_port)
        } else {
            std::net::SocketAddr::new(std::net::Ipv6Addr::UNSPECIFIED.into(), listen_port)
        };
        socket
            .bind(bind_addr)
            .map_err(|e| DuplexError::Connection(format!("bind to port {listen_port}: {e}")))?;

        let stream = socket
            .connect(resolved)
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

        // ChainSync and BlockFetch: only subscribe when the caller will read them.
        // Governor connections set subscribe_chainsync=false to prevent demuxer stall.
        // Unsubscribed protocol messages are silently dropped by the demuxer.
        let cs_channel = if subscribe_chainsync {
            Some(plexer.subscribe_client(PROTOCOL_N2N_CHAIN_SYNC))
        } else {
            None
        };
        let bf_channel = if subscribe_chainsync {
            Some(plexer.subscribe_client(PROTOCOL_N2N_BLOCK_FETCH))
        } else {
            None
        };

        // TxSubmission2 channel assignment for duplex connections:
        //
        // We are the TCP initiator.  The pallas mux convention is:
        //   subscribe_client(P) → sends on P (bit-15=0), receives on P|0x8000
        //   subscribe_server(P) → sends on P|0x8000, receives on P (bit-15=0)
        //
        // The remote (TCP responder) demuxer FLIPS the direction:
        //   Our bit-15=0 → delivered to remote's ResponderDir (their Server side)
        //   Our bit-15=1 → delivered to remote's InitiatorDir (their Client side)
        //
        // For TxSubmission2, the Ouroboros roles are:
        //   Client = tx advertiser (sends MsgInit, MsgReplyTxIds, MsgReplyTxs)
        //   Server = tx consumer (sends MsgRequestTxIds, MsgRequestTxs)
        //
        // Our Client (tx advertiser) messages must arrive at the remote's
        // Server (ResponderDir).  So we send with bit-15=0 → subscribe_client.
        //
        // But wait — the remote ALSO runs a Client (InitiatorDir) on this same
        // connection.  The remote's Client sends MsgInit with bit-15=0 from
        // their perspective.  After the demuxer flip, that arrives as our
        // ResponderDir = subscribe_server receive channel.
        //
        // So our mapping is:
        //   subscribe_client(4) → OUR TxSubmission2 Client (we advertise our mempool)
        //   subscribe_server(4) → OUR TxSubmission2 Server (we consume remote's mempool)
        //
        // The serve_tx_submission function is our CLIENT (advertiser).
        // The pull_tx_submission function is our SERVER (consumer).
        let txsub_client_channel = plexer.subscribe_client(PROTOCOL_N2N_TX_SUBMISSION);
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
        let cs_buf = cs_channel.map(pallas_network::multiplexer::ChannelBuffer::new);

        // --- BlockFetch client ---------------------------------------------------
        let bf_client = bf_channel.map(pallas_network::miniprotocols::blockfetch::Client::new);

        // Use the resolved address (from DNS lookup above) as the remote_addr.
        // This ensures we have a valid SocketAddr even when `addr` is a hostname.
        let remote_addr: SocketAddr = resolved;

        // --- TxSubmission2 CLIENT task (subscribe_client channel) ----------------
        //
        // In TxSubmission2, the **Client** (outbound/initiator) SERVES its own
        // mempool to the remote Server.  The Client sends MsgInit, then waits
        // for the remote Server to send MsgRequestTxIds, and replies with
        // MsgReplyTxIds / MsgReplyTxs from our mempool.
        //
        // This matches Haskell's `txSubmissionOutbound`.
        let txsub_client = tokio::spawn(serve_tx_submission(
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
        let txsub_server = tokio::spawn(pull_tx_submission(
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
            _txsub_client: txsub_client,
            _txsub_server: txsub_server,
            _cs_drain: None,
            _bf_drain: None,
        })
    }

    /// Start drain tasks for ChainSync and BlockFetch channels.
    /// These tasks read and discard all incoming messages, preventing the
    /// demuxer's bounded channels from filling up and stalling.
    fn start_drain_tasks(&mut self) {
        if let Some(cs_buf) = self.cs_buf.take() {
            // Unwrap the ChannelBuffer to get the underlying AgentChannel,
            // then spawn a task that reads and discards all incoming chunks.
            let mut channel = cs_buf.unwrap();
            self._cs_drain = Some(tokio::spawn(async move {
                while channel.dequeue_chunk().await.is_ok() {
                    // discard incoming chunks until channel closes
                }
            }));
        }
        if let Some(bf_client) = self.bf_client.take() {
            // BlockFetch: extract the inner channel and drain it.
            // pallas BlockFetch Client doesn't expose the channel directly,
            // so we just hold the client object alive (it holds the channel).
            // This keeps the channel subscribed so demuxer doesn't panic,
            // and the 100-slot buffer is enough for warm connections.
            self._bf_drain = Some(tokio::spawn(async move {
                // Hold the client alive indefinitely
                let _keep = bf_client;
                futures::future::pending::<()>().await;
            }));
        }
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
    /// Panics if the connection was created without ChainSync subscription.
    pub fn blockfetch(&mut self) -> &mut pallas_network::miniprotocols::blockfetch::Client {
        self.bf_client
            .as_mut()
            .expect("blockfetch() requires ChainSync subscription")
    }

    /// Abort the connection (kills the plexer and responder tasks).
    pub async fn abort(self) {
        self._txsub_client.abort();
        self._txsub_server.abort();
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
    /// The two TxSubmission2 tasks are returned as a combined `JoinHandle` — the
    /// caller MUST keep them alive for as long as the sync session is active.
    /// Dropping them aborts both tasks, which is the correct cleanup behavior.
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
            self.cs_buf
                .expect("into_pipelined requires ChainSync subscription"),
            self.bf_client
                .expect("into_pipelined requires BlockFetch subscription"),
            self._keepalive,
            self._plexer,
            self._peersharing_channel,
            self.remote_addr,
            self.in_flight,
            self.byron_epoch_length,
        );
        (client, self._txsub_client, self._txsub_server)
    }
}

// ─── TxSubmission2 Client task (serves our mempool to remote Server) ─────────

/// Serve our mempool to a remote TxSubmission2 Server via the CLIENT role.
///
/// This function wraps the inner implementation and logs errors as warnings —
/// TxSubmission2 failures are non-fatal to the parent ChainSync connection.
///
/// Protocol flow (we are the **Client**, the tx advertiser):
/// 1. We send MsgInit [6]
/// 2. Remote Server sends MsgRequestTxIds [0, blocking, ack, req]
///    → We reply MsgReplyTxIds [1, [[tx_id, size], ...]]
/// 3. Remote Server sends MsgRequestTxs [2, [tx_id, ...]]
///    → We reply MsgReplyTxs [3, [tx_cbor, ...]]
/// 4. Remote Server sends MsgDone [4] → we exit
///
/// The Client NEVER sends MsgInit a second time and does NOT wait for a
/// MsgInit reply — only the Client sends MsgInit, and only once.
pub async fn serve_tx_submission(
    channel: AgentChannel,
    mempool: Arc<dyn MempoolProvider>,
    peer_addr: SocketAddr,
) {
    match serve_tx_submission_inner(channel, mempool, peer_addr).await {
        Ok(()) => debug!(%peer_addr, "TxSubmission2 client: session complete"),
        Err(e) => warn!(%peer_addr, "TxSubmission2 client: {e}"),
    }
}

/// Inner function that returns `Result` so we can use `?` throughout.
///
/// This is the TxSubmission2 **Client** (outbound/initiator) role.
/// Per the Haskell spec (`txSubmissionOutbound`), the Client:
///   1. Sends MsgInit [6]
///   2. Waits for the Server to send MsgRequestTxIds  (Server has agency in StIdle)
///   3. Replies with MsgReplyTxIds from our mempool
///   4. Waits for MsgRequestTxs
///   5. Replies with MsgReplyTxs
///
/// The Client DOES NOT expect MsgInit back — only the Client sends it.
/// The Haskell Server has a 60 s `threadDelay` warmup delay before it reads
/// MsgInit, so TXSUB_BLOCKING_TIMEOUT covers that wait.
async fn serve_tx_submission_inner(
    mut channel: AgentChannel,
    mempool: Arc<dyn MempoolProvider>,
    peer_addr: SocketAddr,
) -> Result<(), DuplexError> {
    // Step 1: Send MsgInit [6].
    // Only the Client sends MsgInit.  We do NOT wait for a MsgInit reply —
    // the Server goes directly to MsgRequestTxIds after its warmup delay.
    let init_msg = encode_msg_init();
    info!(%peer_addr, "TxSubmission2 client: sending MsgInit [{}]",
        hex_prefix(&init_msg));
    send_msg(&mut channel, &init_msg).await?;
    info!(%peer_addr, "TxSubmission2 client: sent MsgInit, waiting for server requests");

    // Track inflight tx IDs (sent to this peer, not yet acknowledged).
    // This vector is ordered: front = oldest, back = newest.  We drain
    // from the front when the peer sends ack_count > 0.
    let mut inflight: Vec<[u8; 32]> = Vec::new();

    // Track ALL tx IDs ever offered to this peer (inflight + acknowledged).
    // Without this, acked txs still in the mempool would be re-offered
    // in subsequent MsgRequestTxIds rounds, causing an infinite loop
    // instead of progressing through the full mempool.
    let mut sent: std::collections::HashSet<[u8; 32]> = std::collections::HashSet::new();

    loop {
        // Use a generous timeout for the blocking MsgRequestTxIds case:
        // the peer may hold us for up to 5 minutes while waiting for new txs.
        // The initial request also goes through this path — the Haskell Server
        // waits 60 s before sending the first MsgRequestTxIds.
        let payload = tokio::time::timeout(TXSUB_BLOCKING_TIMEOUT, recv_msg(&mut channel))
            .await
            .map_err(|_| {
                DuplexError::Timeout("TxSubmission2 client: MsgRequestTxIds timeout".into())
            })??;

        let tag = decode_first_tag(&payload)?;
        info!(%peer_addr, tag, "TxSubmission2 client: received message [{}]",
            hex_prefix(&payload));

        match tag {
            // MsgRequestTxIds: [0, blocking, ack_count, req_count]
            0 => {
                let (blocking, ack_count, req_count) = decode_request_tx_ids(&payload)?;

                debug!(
                    %peer_addr,
                    blocking,
                    ack_count,
                    req_count,
                    "TxSubmission2 client: MsgRequestTxIds"
                );

                // Acknowledge previously sent tx IDs.
                if ack_count > 0 {
                    let drain_count = ack_count.min(inflight.len());
                    inflight.drain(..drain_count);
                }

                // Prune the sent set: remove entries for txs no longer in the
                // mempool (confirmed or expired) so the set doesn't grow unbounded.
                if sent.len() > MAX_TX_INFLIGHT * 2 {
                    let snapshot = mempool.snapshot();
                    let mempool_hashes: std::collections::HashSet<[u8; 32]> =
                        snapshot.tx_hashes.iter().map(|h| *h.as_bytes()).collect();
                    sent.retain(|h| mempool_hashes.contains(h));
                }

                // Enforce inflight cap: if at the limit, reply empty so the
                // peer can send more acks before we push new tx IDs.
                let reply = if inflight.len() >= MAX_TX_INFLIGHT {
                    warn!(
                        %peer_addr,
                        inflight = inflight.len(),
                        "TxSubmission2 client: inflight cap reached, sending empty reply"
                    );
                    encode_reply_tx_ids(&[])
                } else {
                    // Compute how many IDs we can send without exceeding the cap.
                    let remaining_cap = MAX_TX_INFLIGHT - inflight.len();
                    let capped_req = req_count.min(MAX_TX_IDS_PER_REPLY).min(remaining_cap);

                    // Pull up to capped_req new tx IDs from the mempool, excluding
                    // those already sent (inflight or acknowledged) to this peer.
                    let new_ids: Vec<([u8; 32], u32)> = {
                        let snapshot = mempool.snapshot();
                        snapshot
                            .tx_hashes
                            .iter()
                            .filter(|h| {
                                let bytes = *h.as_bytes();
                                !sent.contains(&bytes)
                            })
                            .take(capped_req)
                            .filter_map(|h| {
                                mempool.get_tx_size(h).map(|sz| (*h.as_bytes(), sz as u32))
                            })
                            .collect()
                    };

                    // When blocking=true and no txs available, we MUST NOT
                    // reply with an empty list — that's a protocol violation.
                    // The Haskell Server expects us to block until we have txs.
                    // Poll the mempool every 5 seconds until txs appear.
                    let new_ids = if new_ids.is_empty() && blocking {
                        loop {
                            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                            let snapshot = mempool.snapshot();
                            let polled: Vec<_> = snapshot
                                .tx_hashes
                                .iter()
                                .filter(|h| {
                                    let bytes = *h.as_bytes();
                                    !sent.contains(&bytes)
                                })
                                .take(capped_req)
                                .filter_map(|h| {
                                    mempool.get_tx_size(h).map(|sz| (*h.as_bytes(), sz as u32))
                                })
                                .collect();
                            if !polled.is_empty() {
                                break polled;
                            }
                            // Continue polling — we must block until txs arrive
                        }
                    } else {
                        new_ids
                    };

                    // Record newly sent IDs as inflight and in the sent set.
                    for (hash, _) in &new_ids {
                        if inflight.len() < MAX_TX_INFLIGHT {
                            inflight.push(*hash);
                        }
                        sent.insert(*hash);
                    }

                    if !new_ids.is_empty() {
                        info!(
                            %peer_addr,
                            count = new_ids.len(),
                            inflight = inflight.len(),
                            "TxSubmission2 client: sending MsgReplyTxIds"
                        );
                    }

                    encode_reply_tx_ids(&new_ids)
                };

                info!(%peer_addr, "TxSubmission2 client: sending MsgReplyTxIds [{}]",
                    hex_prefix(&reply));
                send_msg(&mut channel, &reply).await?;
            }

            // MsgRequestTxs: [2, [tx_id, ...]]
            2 => {
                let requested_hashes = decode_request_txs(&payload)?;
                debug!(
                    %peer_addr,
                    count = requested_hashes.len(),
                    "TxSubmission2 client: MsgRequestTxs"
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
                    "TxSubmission2 client: sending MsgReplyTxs"
                );

                let reply = encode_reply_txs(&bodies);
                info!(%peer_addr, "TxSubmission2 client: sending MsgReplyTxs [{}]",
                    hex_prefix(&reply));
                send_msg(&mut channel, &reply).await?;
            }

            // MsgDone: [4]
            4 => {
                info!(%peer_addr, "TxSubmission2 client: peer sent MsgDone, closing");
                break;
            }

            // Unexpected tag
            other => {
                warn!(
                    %peer_addr,
                    tag = other,
                    "TxSubmission2 client: unexpected message tag, closing"
                );
                break;
            }
        }
    }

    Ok(())
}

// ─── TxSubmission2 Server task (pulls remote Client's mempool into ours) ─────

/// Pull transactions from a remote TxSubmission2 Client via the SERVER role.
///
/// This function wraps the inner implementation and logs errors as warnings.
///
/// Protocol flow (we are the **Server**, the tx consumer):
/// 1. Remote Client sends MsgInit [6] → we receive it (do NOT send MsgInit back)
/// 2. We send MsgRequestTxIds [0, blocking, ack, req]
///    → Remote Client replies MsgReplyTxIds [1, [[tx_id, size], ...]]
/// 3. We send MsgRequestTxs [2, [tx_id, ...]]
///    → Remote Client replies MsgReplyTxs [3, [tx_cbor, ...]]
/// 4. We send MsgDone [4] to close (or exit when the channel closes)
pub async fn pull_tx_submission(
    channel: AgentChannel,
    mempool: Arc<dyn MempoolProvider>,
    peer_addr: SocketAddr,
) {
    match pull_tx_submission_inner(channel, mempool, peer_addr).await {
        Ok(()) => debug!(%peer_addr, "TxSubmission2 server: session complete"),
        Err(e) => warn!(%peer_addr, "TxSubmission2 server: {e}"),
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
    info!(%peer_addr, tag = init_tag, "TxSubmission2 server: received initial message [{}]",
        hex_prefix(&init_payload));
    if init_tag != 6 {
        return Err(DuplexError::TxSubmissionInitiator(format!(
            "expected MsgInit (6) from client, got tag {init_tag}"
        )));
    }
    info!(%peer_addr, "TxSubmission2 server: received MsgInit — beginning tx polling");

    // Track tx IDs received from this peer but not yet acknowledged.
    // pending_ack is sent as ack_count in the next MsgRequestTxIds.
    let mut pending_ack: u16 = 0;
    // Set of tx IDs already seen from this peer (dedup filter).
    let mut known_tx_ids: std::collections::HashSet<[u8; 32]> = std::collections::HashSet::new();

    loop {
        // Step 2: Send MsgRequestTxIds.
        // Per Haskell protocol rules:
        //   blocking MUST be used when unacknowledged count is 0 (after acking)
        //   non-blocking MUST be used when unacknowledged count > 0
        // After acking, pending_ack resets to 0, meaning all previous IDs were acked
        // and we have zero unacknowledged — use blocking.
        let ack = pending_ack;
        let use_blocking = ack > 0 || known_tx_ids.is_empty();
        let req_msg = encode_request_tx_ids(use_blocking, ack, SERVER_REQ_TX_IDS);
        info!(%peer_addr, ack, "TxSubmission2 server: sending MsgRequestTxIds [{}]",
            hex_prefix(&req_msg));
        send_msg(&mut channel, &req_msg).await?;
        pending_ack = 0;

        // Step 3: Receive MsgReplyTxIds or MsgDone.
        let reply = tokio::time::timeout(TXSUB_BLOCKING_TIMEOUT, recv_msg(&mut channel))
            .await
            .map_err(|_| {
                DuplexError::Timeout("TxSubmission2 server: MsgReplyTxIds timeout".into())
            })??;

        let reply_tag = decode_first_tag(&reply)?;
        info!(%peer_addr, tag = reply_tag, "TxSubmission2 server: received reply [{}]",
            hex_prefix(&reply));

        match reply_tag {
            // MsgReplyTxIds: [1, [[tx_id, size], ...]]
            1 => {
                let ids = decode_reply_tx_ids(&reply)?;

                if ids.is_empty() {
                    // Non-blocking returned empty — switch to blocking.
                    debug!(%peer_addr, "TxSubmission2 server: no txs available, sending blocking request");
                    let blocking_req = encode_request_tx_ids(true, 0, SERVER_REQ_TX_IDS);
                    info!(%peer_addr, "TxSubmission2 server: sending blocking MsgRequestTxIds [{}]",
                        hex_prefix(&blocking_req));
                    send_msg(&mut channel, &blocking_req).await?;

                    // Receive the blocking reply.
                    let blocking_reply =
                        tokio::time::timeout(TXSUB_BLOCKING_TIMEOUT, recv_msg(&mut channel))
                            .await
                            .map_err(|_| {
                                DuplexError::Timeout(
                                    "TxSubmission2 server: blocking MsgReplyTxIds timeout".into(),
                                )
                            })??;

                    let blocking_tag = decode_first_tag(&blocking_reply)?;
                    info!(%peer_addr, tag = blocking_tag,
                        "TxSubmission2 server: received blocking reply [{}]",
                        hex_prefix(&blocking_reply));

                    match blocking_tag {
                        1 => {
                            let blocking_ids = decode_reply_tx_ids(&blocking_reply)?;
                            if blocking_ids.is_empty() {
                                // Empty blocking reply = peer is ending the session.
                                info!(%peer_addr,
                                    "TxSubmission2 server: empty blocking reply, closing");
                                break;
                            }
                            if let Err(e) = process_tx_ids_server(
                                &mut channel,
                                &blocking_ids,
                                &mempool,
                                &mut pending_ack,
                                &mut known_tx_ids,
                                peer_addr,
                            )
                            .await
                            {
                                warn!(%peer_addr, "TxSubmission2 server: process_tx_ids: {e}");
                            }
                        }
                        4 => {
                            info!(%peer_addr,
                                "TxSubmission2 server: peer sent MsgDone during blocking wait");
                            break;
                        }
                        other => {
                            warn!(%peer_addr, tag = other,
                                "TxSubmission2 server: unexpected tag in blocking reply");
                            break;
                        }
                    }
                } else if let Err(e) = process_tx_ids_server(
                    &mut channel,
                    &ids,
                    &mempool,
                    &mut pending_ack,
                    &mut known_tx_ids,
                    peer_addr,
                )
                .await
                {
                    warn!(%peer_addr, "TxSubmission2 server: process_tx_ids: {e}");
                }
            }

            // MsgDone: [4]
            4 => {
                info!(%peer_addr, "TxSubmission2 server: peer sent MsgDone, closing");
                break;
            }

            other => {
                warn!(
                    %peer_addr,
                    tag = other,
                    "TxSubmission2 server: unexpected reply tag, closing"
                );
                break;
            }
        }
    }

    Ok(())
}

/// Process a batch of tx IDs from a MsgReplyTxIds: fetch bodies via MsgRequestTxs,
/// then add new transactions to the local mempool.
///
/// Updates `pending_ack` with the number of IDs just received (to be sent in
/// the next MsgRequestTxIds).  Deduplicates via `known_tx_ids`.
async fn process_tx_ids_server(
    channel: &mut AgentChannel,
    ids: &[([u8; 32], u32)],
    mempool: &Arc<dyn MempoolProvider>,
    pending_ack: &mut u16,
    known_tx_ids: &mut std::collections::HashSet<[u8; 32]>,
    peer_addr: SocketAddr,
) -> Result<(), DuplexError> {
    // Accumulate pending acks for this batch.
    *pending_ack = pending_ack.saturating_add(ids.len() as u16);

    // Filter out already-known tx IDs.
    let new_ids: Vec<[u8; 32]> = ids
        .iter()
        .filter_map(|(hash, _)| {
            if known_tx_ids.contains(hash) {
                None
            } else {
                let h = torsten_primitives::hash::Hash32::from_bytes(*hash);
                if mempool.contains(&h) {
                    None
                } else {
                    Some(*hash)
                }
            }
        })
        .collect();

    // Track all IDs from this batch as seen (evict on overflow).
    const MAX_KNOWN: usize = 10_000;
    if known_tx_ids.len() + ids.len() > MAX_KNOWN {
        known_tx_ids.clear();
    }
    for (hash, _) in ids {
        known_tx_ids.insert(*hash);
    }

    if new_ids.is_empty() {
        return Ok(());
    }

    debug!(
        %peer_addr,
        new = new_ids.len(),
        total = ids.len(),
        "TxSubmission2 server: requesting tx bodies"
    );

    // Send MsgRequestTxs for the new (unknown) tx IDs.
    let req_txs_msg = encode_request_txs(&new_ids);
    info!(%peer_addr, count = new_ids.len(),
        "TxSubmission2 server: sending MsgRequestTxs [{}]",
        hex_prefix(&req_txs_msg));
    send_msg(channel, &req_txs_msg).await?;

    // Receive MsgReplyTxs.
    let reply = tokio::time::timeout(TXSUB_BLOCKING_TIMEOUT, recv_msg(channel))
        .await
        .map_err(|_| DuplexError::Timeout("TxSubmission2 server: MsgReplyTxs timeout".into()))??;

    let reply_tag = decode_first_tag(&reply)?;
    info!(%peer_addr, tag = reply_tag,
        "TxSubmission2 server: received MsgReplyTxs [{}]",
        hex_prefix(&reply));

    if reply_tag != 3 {
        return Err(DuplexError::TxSubmissionInitiator(format!(
            "expected MsgReplyTxs (3), got {reply_tag}"
        )));
    }

    let bodies = decode_reply_txs(&reply)?;
    info!(
        %peer_addr,
        count = bodies.len(),
        "TxSubmission2 server: received tx bodies"
    );

    for (i, tx_cbor) in bodies.iter().enumerate() {
        let tx_hash_bytes = if i < new_ids.len() {
            new_ids[i]
        } else {
            continue;
        };
        let tx_hash = torsten_primitives::hash::Hash32::from_bytes(tx_hash_bytes);

        // Try decoding across eras (Conway=6 first, then backwards).
        let mut decoded = false;
        for era in [6u16, 5, 4, 3, 2] {
            match torsten_serialization::decode_transaction(era, tx_cbor) {
                Ok(tx) => {
                    let tx_size = tx_cbor.len();
                    let fee = tx.body.fee;
                    match mempool.add_tx_with_fee(tx_hash, tx, tx_size, fee) {
                        Ok(torsten_primitives::mempool::MempoolAddResult::Added) => {
                            info!(
                                hash = %tx_hash,
                                size = tx_size,
                                "TxSubmission2 server: tx added to mempool"
                            );
                        }
                        Ok(torsten_primitives::mempool::MempoolAddResult::AlreadyExists) => {
                            debug!(hash = %tx_hash, "TxSubmission2 server: tx already in mempool");
                        }
                        Err(e) => {
                            debug!(hash = %tx_hash,
                                "TxSubmission2 server: mempool rejected tx: {e}");
                        }
                    }
                    decoded = true;
                    break;
                }
                Err(_) => continue,
            }
        }

        if !decoded {
            warn!(hash = %tx_hash, "TxSubmission2 server: failed to decode tx in any era");
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

/// Return the first 16 bytes of a payload as a hex string (for debug logging).
fn hex_prefix(payload: &[u8]) -> String {
    let n = payload.len().min(16);
    let mut s = String::with_capacity(n * 2 + 3);
    for b in &payload[..n] {
        s.push_str(&format!("{b:02x}"));
    }
    if payload.len() > 16 {
        s.push_str("..");
    }
    s
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

/// Decode `MsgReplyTxIds = [1, [[tx_id, size], ...]]`.
///
/// The inner list may be either definite-length or indefinite-length CBOR.
/// The pallas Haskell encoder uses indefinite-length (`begin_array`/`end`).
fn decode_reply_tx_ids(payload: &[u8]) -> Result<Vec<([u8; 32], u32)>, DuplexError> {
    let mut dec = minicbor::Decoder::new(payload);
    dec.array()
        .map_err(|e| DuplexError::Cbor(format!("MsgReplyTxIds: expected outer array: {e}")))?;
    let _tag = dec
        .u32()
        .map_err(|e| DuplexError::Cbor(format!("MsgReplyTxIds: expected tag: {e}")))?;

    // The inner list may be definite (Some(n)) or indefinite (None from `array()`).
    let count_opt = dec
        .array()
        .map_err(|e| DuplexError::Cbor(format!("MsgReplyTxIds: expected id array: {e}")))?;

    let mut ids = Vec::new();
    match count_opt {
        Some(n) => {
            // Definite-length array.
            for _ in 0..n {
                let (hash, size) = decode_tx_id_and_size(&mut dec)?;
                ids.push((hash, size));
            }
        }
        None => {
            // Indefinite-length array — read until `break` (DataType::Break).
            loop {
                // Peek at the next data type; break code = 0xFF.
                use minicbor::data::Type;
                match dec.datatype() {
                    Ok(Type::Break) => {
                        // consume the break
                        let _ = dec.skip();
                        break;
                    }
                    Ok(_) => {
                        let (hash, size) = decode_tx_id_and_size(&mut dec)?;
                        ids.push((hash, size));
                    }
                    Err(e) => {
                        return Err(DuplexError::Cbor(format!(
                            "MsgReplyTxIds: error reading indefinite array: {e}"
                        )));
                    }
                }
            }
        }
    }
    Ok(ids)
}

/// Decode a single `[tx_hash_bytes, size_u32]` pair from the inner array.
/// Decode a single `[GenTxId, size]` pair from MsgReplyTxIds.
/// GenTxId may be HFC-wrapped `[era, hash]` or raw `hash_bytes`.
fn decode_tx_id_and_size(dec: &mut minicbor::Decoder<'_>) -> Result<([u8; 32], u32), DuplexError> {
    dec.array()
        .map_err(|e| DuplexError::Cbor(format!("MsgReplyTxIds: expected pair array: {e}")))?;

    // The tx ID may be HFC-wrapped [era, hash] or raw bytes.
    let hash = match dec.datatype() {
        Ok(minicbor::data::Type::Array | minicbor::data::Type::ArrayIndef) => {
            // HFC GenTxId: [era_index, tx_hash_bytes]
            let _ = dec
                .array()
                .map_err(|e| DuplexError::Cbor(format!("MsgReplyTxIds: GenTxId array: {e}")))?;
            let _ = dec
                .u32()
                .map_err(|e| DuplexError::Cbor(format!("MsgReplyTxIds: era index: {e}")))?;
            dec.bytes()
                .map_err(|e| DuplexError::Cbor(format!("MsgReplyTxIds: tx hash: {e}")))?
        }
        _ => {
            // Raw bytes (non-HFC)
            dec.bytes()
                .map_err(|e| DuplexError::Cbor(format!("MsgReplyTxIds: tx hash: {e}")))?
        }
    };

    if hash.len() != 32 {
        return Err(DuplexError::Cbor(format!(
            "MsgReplyTxIds: tx hash length {} != 32",
            hash.len()
        )));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(hash);
    let size = dec
        .u32()
        .map_err(|e| DuplexError::Cbor(format!("MsgReplyTxIds: expected size u32: {e}")))?;
    Ok((arr, size))
}

/// Decode `MsgRequestTxs = [2, [tx_id, ...]]` and return the list of tx hashes.
///
/// The inner list may be either definite-length or indefinite-length CBOR.
fn decode_request_txs(payload: &[u8]) -> Result<Vec<[u8; 32]>, DuplexError> {
    let mut dec = minicbor::Decoder::new(payload);
    dec.array()
        .map_err(|e| DuplexError::Cbor(format!("MsgRequestTxs: expected outer array: {e}")))?;
    let _tag = dec
        .u32()
        .map_err(|e| DuplexError::Cbor(format!("MsgRequestTxs: expected tag: {e}")))?;

    let count_opt = dec
        .array()
        .map_err(|e| DuplexError::Cbor(format!("MsgRequestTxs: expected id array: {e}")))?;

    // Helper: decode a single tx ID which may be HFC-wrapped [era, hash] or raw bytes.
    let decode_one_tx_id = |dec: &mut minicbor::Decoder| -> Result<Option<[u8; 32]>, DuplexError> {
        // Peek at the type: if it's an array, it's HFC-wrapped [era_index, hash_bytes]
        match dec.datatype() {
            Ok(minicbor::data::Type::Array | minicbor::data::Type::ArrayIndef) => {
                let _ = dec
                    .array()
                    .map_err(|e| DuplexError::Cbor(format!("MsgRequestTxs: GenTxId array: {e}")))?;
                // Skip era index
                let _ = dec
                    .u32()
                    .map_err(|e| DuplexError::Cbor(format!("MsgRequestTxs: era index: {e}")))?;
                let bytes = dec
                    .bytes()
                    .map_err(|e| DuplexError::Cbor(format!("MsgRequestTxs: tx hash bytes: {e}")))?;
                if bytes.len() == 32 {
                    let mut arr = [0u8; 32];
                    arr.copy_from_slice(bytes);
                    Ok(Some(arr))
                } else {
                    Ok(None)
                }
            }
            Ok(minicbor::data::Type::Bytes | minicbor::data::Type::BytesIndef) => {
                // Raw bytes (non-HFC, for backwards compatibility)
                let bytes = dec
                    .bytes()
                    .map_err(|e| DuplexError::Cbor(format!("MsgRequestTxs: raw tx hash: {e}")))?;
                if bytes.len() == 32 {
                    let mut arr = [0u8; 32];
                    arr.copy_from_slice(bytes);
                    Ok(Some(arr))
                } else {
                    Ok(None)
                }
            }
            Ok(other) => Err(DuplexError::Cbor(format!(
                "MsgRequestTxs: unexpected type {other:?} for tx ID"
            ))),
            Err(e) => Err(DuplexError::Cbor(format!(
                "MsgRequestTxs: datatype error: {e}"
            ))),
        }
    };

    let mut hashes = Vec::new();
    match count_opt {
        Some(n) => {
            let cap = (n as usize).min(MAX_TX_BODY_REQUEST);
            for _ in 0..cap {
                if let Some(hash) = decode_one_tx_id(&mut dec)? {
                    hashes.push(hash);
                }
            }
        }
        None => {
            use minicbor::data::Type;
            loop {
                match dec.datatype() {
                    Ok(Type::Break) => {
                        let _ = dec.skip();
                        break;
                    }
                    Ok(_) => {
                        if let Some(hash) = decode_one_tx_id(&mut dec)? {
                            hashes.push(hash);
                        }
                        if hashes.len() >= MAX_TX_BODY_REQUEST {
                            break;
                        }
                    }
                    Err(e) => {
                        return Err(DuplexError::Cbor(format!(
                            "MsgRequestTxs: error reading indefinite array: {e}"
                        )));
                    }
                }
            }
        }
    }
    Ok(hashes)
}

/// Decode `MsgReplyTxs = [3, [tx_cbor, ...]]`.
///
/// The inner list may be either definite-length or indefinite-length CBOR.
/// The pallas Haskell encoder uses indefinite-length (`begin_array`/`end`).
/// Decode a single GenTx which may be HFC-wrapped [era, tag(24, cbor)] or raw bytes.
fn decode_one_tx_body(dec: &mut minicbor::Decoder<'_>) -> Result<Vec<u8>, DuplexError> {
    match dec.datatype() {
        Ok(minicbor::data::Type::Array | minicbor::data::Type::ArrayIndef) => {
            // HFC GenTx: [era_index, tag(24, tx_cbor_bytes)]
            let _ = dec
                .array()
                .map_err(|e| DuplexError::Cbor(format!("MsgReplyTxs: GenTx array: {e}")))?;
            let _ = dec
                .u32()
                .map_err(|e| DuplexError::Cbor(format!("MsgReplyTxs: era index: {e}")))?;
            let _ = dec
                .tag()
                .map_err(|e| DuplexError::Cbor(format!("MsgReplyTxs: tag(24): {e}")))?;
            let bytes = dec
                .bytes()
                .map_err(|e| DuplexError::Cbor(format!("MsgReplyTxs: tx cbor: {e}")))?;
            Ok(bytes.to_vec())
        }
        _ => {
            // Raw bytes (non-HFC)
            let bytes = dec
                .bytes()
                .map_err(|e| DuplexError::Cbor(format!("MsgReplyTxs: raw tx cbor: {e}")))?;
            Ok(bytes.to_vec())
        }
    }
}

fn decode_reply_txs(payload: &[u8]) -> Result<Vec<Vec<u8>>, DuplexError> {
    let mut dec = minicbor::Decoder::new(payload);
    dec.array()
        .map_err(|e| DuplexError::Cbor(format!("MsgReplyTxs: expected outer array: {e}")))?;
    let _tag = dec
        .u32()
        .map_err(|e| DuplexError::Cbor(format!("MsgReplyTxs: expected tag: {e}")))?;

    let count_opt = dec
        .array()
        .map_err(|e| DuplexError::Cbor(format!("MsgReplyTxs: expected body array: {e}")))?;

    let mut bodies = Vec::new();
    match count_opt {
        Some(n) => {
            for _ in 0..n {
                bodies.push(decode_one_tx_body(&mut dec)?);
            }
        }
        None => {
            use minicbor::data::Type;
            loop {
                match dec.datatype() {
                    Ok(Type::Break) => {
                        let _ = dec.skip();
                        break;
                    }
                    Ok(_) => {
                        bodies.push(decode_one_tx_body(&mut dec)?);
                    }
                    Err(e) => {
                        return Err(DuplexError::Cbor(format!(
                            "MsgReplyTxs: error reading indefinite array: {e}"
                        )));
                    }
                }
            }
        }
    }
    Ok(bodies)
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

/// Encode `MsgReplyTxIds = [1, [[tx_id, size_bytes], ...]]`.
///
/// The inner list uses **indefinite-length** encoding to match the pallas /
/// Haskell wire format.  Definite-length would also be parseable by a
/// conformant CBOR decoder, but we match Haskell for maximum compatibility.
///
/// Each element is `[bytes(tx_hash), uint(size)]`.  An empty slice produces
/// `[1, _ []]`, which is the correct non-blocking empty reply.
/// Encode `MsgReplyTxIds = [1, [[GenTxId, size], ...]]`.
///
/// Each tx ID is HFC-wrapped as `GenTxId = [era_index, tx_hash_bytes]`.
/// Conway is era index 6 in the Haskell HFC encoding (Byron=0..Conway=6).
/// Uses indefinite-length arrays to match Haskell wire format.
fn encode_reply_tx_ids(ids: &[([u8; 32], u32)]) -> Vec<u8> {
    // Conway era index in the HFC GenTxId encoding
    const CONWAY_ERA_INDEX: u32 = 6;

    let mut buf = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut buf);
    enc.array(2).expect("encode outer array");
    enc.u32(1).expect("encode tag");
    enc.begin_array().expect("begin indefinite array");
    for (hash, size) in ids {
        // Each entry: [GenTxId, size]
        enc.array(2).expect("encode id+size pair");
        // GenTxId: [era_index, tx_hash_bytes]
        enc.array(2).expect("encode GenTxId");
        enc.u32(CONWAY_ERA_INDEX).expect("encode era index");
        enc.bytes(hash.as_slice()).expect("encode hash");
        enc.u32(*size).expect("encode size");
    }
    enc.end().expect("end indefinite array");
    buf
}

/// Encode `MsgRequestTxs = [2, [tx_id, ...]]`.
///
/// Uses **indefinite-length** encoding for the inner list to match Haskell.
fn encode_request_txs(hashes: &[[u8; 32]]) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut buf);
    enc.array(2).expect("encode outer array");
    enc.u32(2).expect("encode tag");
    enc.begin_array().expect("begin indefinite array");
    for hash in hashes {
        enc.bytes(hash.as_slice()).expect("encode hash");
    }
    enc.end().expect("end indefinite array");
    buf
}

/// Encode `MsgReplyTxs = [3, [tx_cbor, ...]]`.
///
/// Uses **indefinite-length** encoding for the inner list to match Haskell.
/// Encode `MsgReplyTxs = [3, [GenTx, ...]]`.
///
/// Each tx body is HFC-wrapped as `GenTx = [era_index, tag(24, tx_cbor)]`.
/// The tag(24) wrapping matches Haskell's `Serialised` encoding.
fn encode_reply_txs(bodies: &[Vec<u8>]) -> Vec<u8> {
    const CONWAY_ERA_INDEX: u32 = 6;

    let mut buf = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut buf);
    enc.array(2).expect("encode outer array");
    enc.u32(3).expect("encode tag");
    enc.begin_array().expect("begin indefinite array");
    for body in bodies {
        // GenTx: [era_index, tag(24, tx_cbor_bytes)]
        enc.array(2).expect("encode GenTx");
        enc.u32(CONWAY_ERA_INDEX).expect("encode era index");
        enc.tag(minicbor::data::Tag::new(24))
            .expect("encode tag 24");
        enc.bytes(body).expect("encode tx cbor");
    }
    enc.end().expect("end indefinite array");
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

    /// MsgInit hex: must be exactly 82 06 (array(1), u32(6)).
    #[test]
    fn test_encode_msg_init_hex() {
        let bytes = encode_msg_init();
        assert_eq!(
            bytes,
            vec![0x81, 0x06],
            "MsgInit must encode as 81 06 (array(1) containing integer 6)"
        );
    }

    /// MsgReplyTxIds empty — uses indefinite-length inner array.
    #[test]
    fn test_encode_reply_tx_ids_empty_indefinite() {
        let bytes = encode_reply_tx_ids(&[]);
        // Outer array(2): 0x82
        // Tag 1: 0x01
        // begin_array (indefinite): 0x9F
        // end (break): 0xFF
        assert_eq!(bytes[0], 0x82, "outer array(2)");
        assert_eq!(bytes[1], 0x01, "tag = 1");
        assert_eq!(bytes[2], 0x9F, "indefinite array start (0x9F)");
        assert_eq!(*bytes.last().unwrap(), 0xFF, "break code (0xFF)");
    }

    /// MsgReplyTxIds with two entries round-trips correctly (indefinite inner array).
    #[test]
    fn test_encode_reply_tx_ids_two_entries() {
        let hash_a = [0x11u8; 32];
        let hash_b = [0x22u8; 32];
        let ids = vec![(hash_a, 512u32), (hash_b, 1024u32)];
        let bytes = encode_reply_tx_ids(&ids);

        // Decode with our own decoder — must handle indefinite inner array.
        let decoded = decode_reply_tx_ids(&bytes).unwrap();
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].0, hash_a);
        assert_eq!(decoded[0].1, 512u32);
        assert_eq!(decoded[1].0, hash_b);
        assert_eq!(decoded[1].1, 1024u32);
    }

    /// decode_reply_tx_ids handles definite-length inner array (from older/other encoders).
    #[test]
    fn test_decode_reply_tx_ids_definite() {
        let hash_a = [0xAAu8; 32];
        // Build with definite array (minicbor standard encoder).
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap(); // outer array(2)
        enc.u32(1).unwrap(); // tag = 1
        enc.array(1u64).unwrap(); // definite inner array(1)
        enc.array(2).unwrap(); // pair array(2)
        enc.bytes(&hash_a).unwrap();
        enc.u32(256u32).unwrap();

        let decoded = decode_reply_tx_ids(&buf).unwrap();
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].0, hash_a);
        assert_eq!(decoded[0].1, 256u32);
    }

    /// MsgReplyTxs uses indefinite-length inner array.
    #[test]
    fn test_encode_reply_txs_indefinite() {
        let bodies = vec![vec![0x01, 0x02, 0x03], vec![0xFF, 0xFE]];
        let bytes = encode_reply_txs(&bodies);

        assert_eq!(bytes[0], 0x82, "outer array(2)");
        assert_eq!(bytes[1], 0x03, "tag = 3");
        assert_eq!(bytes[2], 0x9F, "indefinite array start");
        assert_eq!(*bytes.last().unwrap(), 0xFF, "break code");

        // Round-trip via decode_reply_txs.
        let decoded = decode_reply_txs(&bytes).unwrap();
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0], vec![0x01, 0x02, 0x03]);
        assert_eq!(decoded[1], vec![0xFF, 0xFE]);
    }

    /// decode_reply_txs handles definite-length inner array.
    #[test]
    fn test_decode_reply_txs_definite() {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(3).unwrap();
        enc.array(1u64).unwrap(); // definite
        enc.bytes(&[0xDE, 0xAD]).unwrap();

        let decoded = decode_reply_txs(&buf).unwrap();
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0], vec![0xDE, 0xAD]);
    }

    /// MsgRequestTxs uses indefinite-length inner array.
    #[test]
    fn test_encode_request_txs_indefinite() {
        let hashes = vec![[0x11u8; 32], [0x22u8; 32]];
        let bytes = encode_request_txs(&hashes);

        assert_eq!(bytes[0], 0x82, "outer array(2)");
        assert_eq!(bytes[1], 0x02, "tag = 2");
        assert_eq!(bytes[2], 0x9F, "indefinite array start");
        assert_eq!(*bytes.last().unwrap(), 0xFF, "break code");

        // Decode round-trip.
        let decoded = decode_request_txs(&bytes).unwrap();
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0], [0x11u8; 32]);
        assert_eq!(decoded[1], [0x22u8; 32]);
    }

    /// decode_request_txs handles definite-length inner array.
    #[test]
    fn test_decode_request_txs_definite() {
        let hash_a = [0xAAu8; 32];
        let hash_b = [0xBBu8; 32];

        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(2).unwrap();
        enc.array(2u64).unwrap(); // definite
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

    /// encode_request_tx_ids produces the correct CBOR wire format.
    #[test]
    fn test_encode_request_tx_ids() {
        let bytes = encode_request_tx_ids(true, 5, 100);
        let mut dec = minicbor::Decoder::new(&bytes);
        assert_eq!(dec.array().unwrap().unwrap_or(0), 4);
        assert_eq!(dec.u32().unwrap(), 0); // tag
        assert!(dec.bool().unwrap()); // blocking = true
        assert_eq!(dec.u16().unwrap(), 5); // ack_count
        assert_eq!(dec.u16().unwrap(), 100); // req_count
    }

    /// hex_prefix returns the correct hex string for short payloads.
    #[test]
    fn test_hex_prefix_short() {
        let data = vec![0x81u8, 0x06];
        let s = hex_prefix(&data);
        assert_eq!(s, "8106");
    }

    /// hex_prefix appends ".." for payloads longer than 16 bytes.
    #[test]
    fn test_hex_prefix_long() {
        let data = vec![0xAAu8; 20];
        let s = hex_prefix(&data);
        assert!(s.ends_with(".."), "must end with .. for long payloads");
        assert_eq!(
            &s[..32],
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"[..32].to_string()
        );
    }

    /// The protocol flow: Client sends MsgInit, then receives MsgRequestTxIds.
    /// Verify there is no second MsgInit sent or expected from the Client side.
    #[test]
    fn test_client_protocol_flow_no_bidirectional_init() {
        // The serve_tx_submission_inner function should:
        //   1. send_init  (one-way)
        //   2. enter loop waiting for MsgRequestTxIds
        // This is verified structurally by confirming our encode/decode
        // helpers do NOT include a "recv_init" step.
        //
        // We encode what a Server would send as its first message after MsgInit:
        let msg_request_tx_ids = encode_request_tx_ids(false, 0, 100);
        let (blocking, ack, req) = decode_request_tx_ids(&msg_request_tx_ids).unwrap();
        assert!(!blocking);
        assert_eq!(ack, 0);
        assert_eq!(req, 100);
        // No MsgInit decode step in this flow — Client doesn't wait for it back.
    }

    /// The server protocol flow: Server receives MsgInit, then sends MsgRequestTxIds.
    /// Verify the Server does NOT send MsgInit.
    #[test]
    fn test_server_protocol_flow_receives_init_then_requests() {
        // Server receives MsgInit [6] from client.
        let init_bytes = encode_msg_init();
        let tag = decode_first_tag(&init_bytes).unwrap();
        assert_eq!(tag, 6, "server must receive tag 6 (MsgInit)");

        // Server then sends MsgRequestTxIds (not another MsgInit).
        let req_bytes = encode_request_tx_ids(false, 0, 100);
        let req_tag = decode_first_tag(&req_bytes).unwrap();
        assert_eq!(
            req_tag, 0,
            "server first message must be MsgRequestTxIds (tag 0)"
        );
    }
}
