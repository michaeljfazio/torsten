//! Connection Lifecycle Manager — temperature-based peer lifecycle.
//!
//! # Haskell Architecture Reference
//!
//! In the Haskell cardano-node, `PeerStateActions` (ouroboros-network) manages
//! peer connection temperature transitions:
//!
//! - **Cold -> Warm**: TCP connect + handshake, start KeepAlive (Established protocols)
//! - **Warm -> Hot**: Start ChainSync + BlockFetch + TxSubmission2 (Hot protocols)
//!   on the SAME multiplexed connection — no new TCP connection is created
//! - **Hot -> Warm**: Stop hot protocol tasks, keep mux + KeepAlive alive
//! - **Warm -> Cold**: Stop all protocol tasks, close mux + TCP connection
//!
//! The key invariant is **one TCP connection per peer**. Temperature transitions
//! only add/remove protocol tasks on the existing mux, never create new connections.
//!
//! ## Duplex Connections (Simultaneous Open)
//!
//! When we already have an outbound connection to a peer and they connect inbound
//! (or vice versa), Haskell promotes the connection to `Duplex` mode. Both the
//! initiator and responder sides share the same underlying TCP connection via the
//! mux's bidirectional channel support.
//!
//! This module provides `ConnectionLifecycleManager` — the node-level orchestrator
//! that translates `GovernorAction` decisions into `PeerConnection` lifecycle calls.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

/// Per-range fetch deadline: maximum time a single `BlockFetchClient::fetch_range()`
/// call is allowed to run before being cancelled.
///
/// Matches Haskell's `bfcFetchDeadlinePolicy` (60s). When a peer's TCP connection
/// is half-open or the remote node stalls mid-batch, this timeout fires, the
/// blockfetch task exits, and the active fetcher flag is released so another peer
/// can take over. The peer is also reported as failed to the peer manager for
/// reputation scoring and exponential backoff.
const FETCH_RANGE_TIMEOUT: Duration = Duration::from_secs(60);

use tokio::sync::{broadcast, mpsc, RwLock};
use tracing::{debug, info, warn};

use dugite_network::peer::governor::GovernorAction;
use dugite_network::BlockAnnouncement;

use dugite_ledger::LedgerState;
use dugite_mempool::Mempool;
use dugite_network::{TxIdAndSize, TxSource};
use dugite_primitives::block::Block;
use dugite_storage::ChainDB;

use super::networking::{ConnectionDirection, NodePeerManager};
use super::peer_connection::{PeerConnection, PeerConnectionError, ProtocolTaskFn};
use crate::metrics::NodeMetrics;

// ─── Shared State Types ─────────────────────────────────────────────────────

/// Candidate chain state from a peer's ChainSync.
///
/// Updated by per-peer ChainSync tasks as they receive headers. Read by the
/// BlockFetch decision task to determine which blocks to fetch and from which
/// peers. This is the coordination point between ChainSync and BlockFetch,
/// matching the Haskell `FetchClientRegistry` / `FetchDecisionPolicy` pattern.
#[derive(Debug, Clone)]
pub struct CandidateChainState {
    /// Slot of the peer's reported tip.
    pub tip_slot: u64,
    /// Hash of the peer's reported tip block.
    pub tip_hash: [u8; 32],
    /// Block number (height) of the peer's reported tip.
    pub tip_block_number: u64,
    /// Headers received via ChainSync but not yet fetched by BlockFetch.
    ///
    /// These accumulate as ChainSync streams headers ahead of BlockFetch.
    /// The BlockFetch decision task consumes entries from this list when it
    /// schedules fetch requests.
    pub pending_headers: Vec<PendingHeader>,
}

/// A block header received via ChainSync, pending BlockFetch download.
///
/// Contains enough information for BlockFetch to request the full block
/// and for the decision task to reason about which range to fetch.
#[derive(Debug, Clone)]
pub struct PendingHeader {
    /// Slot of the block this header describes.
    pub slot: u64,
    /// Hash of the block (used in BlockFetch range requests).
    pub hash: [u8; 32],
    /// Raw CBOR-encoded header bytes (for header validation before fetch).
    pub header_cbor: Vec<u8>,
}

/// A block fetched by a BlockFetch task, ready for ledger application.
///
/// Sent from per-peer BlockFetch tasks to the main run loop via an `mpsc`
/// channel. The run loop applies these blocks to the ChainDB and LedgerState
/// in order.
#[derive(Debug)]
pub struct FetchedBlock {
    /// Address of the peer that served this block.
    pub peer: SocketAddr,
    /// The fully deserialized block.
    pub block: Block,
    /// Tip slot reported by the peer at the time of fetch.
    pub tip_slot: u64,
    /// Tip hash reported by the peer at the time of fetch.
    pub tip_hash: [u8; 32],
    /// Tip block number reported by the peer at the time of fetch.
    pub tip_block_number: u64,
}

/// Result of a background cold->warm connection attempt.
///
/// Sent from `spawn_connect` background tasks to the main run loop via an `mpsc`
/// channel. `Ok` carries the ready `PeerConnection` and measured handshake RTT;
/// `Err` carries the peer address and a human-readable error string.
pub type ConnectResult = Result<(SocketAddr, PeerConnection, f64), (SocketAddr, String)>;

// ─── Lifecycle Manager ──────────────────────────────────────────────────────

/// Manages per-peer connections and temperature transitions.
///
/// Matches Haskell `PeerStateActions`: one TCP connection per peer,
/// temperature-based protocol activation without creating new connections.
///
/// The lifecycle manager owns all active `PeerConnection` instances and
/// provides methods for each temperature transition. It also creates the
/// protocol task closures (KeepAlive, ChainSync, BlockFetch, TxSubmission2)
/// that capture shared node state.
///
/// # Thread Safety
///
/// This struct is NOT `Sync` — it is owned by a single async task (the
/// connection manager loop) that processes `GovernorAction`s sequentially.
/// Shared state (ChainDB, LedgerState, candidate_chains) is accessed via
/// `Arc<RwLock<_>>` to allow concurrent protocol task access.
pub struct ConnectionLifecycleManager {
    /// Active peer connections indexed by socket address.
    ///
    /// Invariant: every entry here has a live mux (is_alive() == true).
    /// Dead connections are removed by `cleanup_dead_connections()`.
    connections: HashMap<SocketAddr, PeerConnection>,

    /// Network magic for N2N handshakes (e.g. 2 for preview, 764824073 for mainnet).
    network_magic: u64,

    /// Whether this node is initiator-only (DiffusionMode::InitiatorOnly).
    initiator_only: bool,

    /// Whether peer sharing is enabled in handshake negotiation.
    peer_sharing: bool,

    /// TCP connect timeout for outbound connections.
    connect_timeout: Duration,

    /// Shared candidate chain state: updated by ChainSync tasks, read by BlockFetch decision.
    ///
    /// Each peer's ChainSync task writes its tip and pending headers here.
    /// The BlockFetch decision task reads all entries to determine optimal
    /// fetch assignments.
    candidate_chains: Arc<RwLock<HashMap<SocketAddr, CandidateChainState>>>,

    /// Channel for BlockFetch tasks to send downloaded blocks to the main run loop.
    fetched_blocks_tx: mpsc::Sender<FetchedBlock>,

    /// Broadcast channel for announcing new blocks to N2N ChainSync servers.
    block_announcement_tx: broadcast::Sender<BlockAnnouncement>,

    /// Shared ChainDB — protocol tasks read chain state for intersection finding.
    chain_db: Arc<RwLock<ChainDB>>,

    /// Shared LedgerState — protocol tasks read ledger tip for intersection.
    ledger_state: Arc<RwLock<LedgerState>>,

    /// Byron epoch length in slots (needed for era-aware slot calculations).
    byron_epoch_length: u64,

    /// Ouroboros security parameter k.
    ///
    /// Passed to each ChainSync task to enforce the k-block rollback limit:
    /// a peer that requests a rollback deeper than k blocks is disconnected
    /// (Haskell: `terminateAfterDrain RolledBackPastIntersection`).
    /// Default: 2160 (mainnet). Preview: 432.
    security_param: u64,

    /// Active slots coefficient from Shelley genesis.
    ///
    /// Used to scale the rollback depth threshold from blocks to slots:
    /// with coeff=0.05, ~20 slots per block on average, so k blocks ≈ k*20 slots.
    /// Default: 0.05 (mainnet/preview).
    active_slots_coeff: f64,

    /// Active BlockFetch peer flag.
    ///
    /// During bulk sync (matching Haskell's `bfcMaxConcurrencyBulkSync = 1`),
    /// only ONE BlockFetch worker is active at a time. This atomic stores the
    /// port number of the active peer (0 = none active). Workers compete for
    /// this flag — the first to claim it becomes the sole fetcher.
    active_fetcher: Arc<std::sync::atomic::AtomicU64>,
    /// Highest slot that has been fetched or is being fetched.
    /// Used to skip duplicate fetches from other peers.
    max_fetched_slot: Arc<std::sync::atomic::AtomicU64>,

    /// Prometheus metrics for recording peer latencies.
    metrics: Arc<NodeMetrics>,

    /// Shared mempool for TxSubmission2 tx relay to peers.
    mempool: Arc<Mempool>,

    /// Channel for protocol tasks to report peer failures (e.g. fetch timeout).
    ///
    /// When a BlockFetch task times out on a peer, it sends the peer address here
    /// so the main run loop can call `peer_failed()` for reputation scoring and
    /// exponential backoff. This provides faster failure detection than waiting
    /// for the mux to die via `cleanup_dead_connections()`.
    peer_failure_tx: mpsc::Sender<SocketAddr>,

    /// Channel for KeepAlive tasks to report per-pong RTT measurements.
    ///
    /// Each successful KeepAlive pong sends `(peer_addr, rtt_ms)` here so the
    /// main run loop can update PeerManager EWMA latency and Prometheus gauges
    /// with current peer RTT values (not cumulative histogram counts).
    keepalive_rtt_tx: mpsc::Sender<(SocketAddr, f64)>,

    /// GSM event sender — passed to ChainSync tasks so they can emit
    /// PeerRegistered, BlockReceived, PeerTipUpdated, PeerActive, PeerIdling
    /// events to the GSM actor.
    gsm_event_tx: tokio::sync::mpsc::Sender<crate::gsm::GsmEvent>,
}

/// Errors from lifecycle management operations.
#[derive(Debug)]
pub enum LifecycleError {
    /// The peer connection operation failed.
    Connection(PeerConnectionError),
    /// No connection exists for the given peer address.
    NotConnected(SocketAddr),
    /// A connection already exists for the given peer address.
    AlreadyConnected(SocketAddr),
}

impl std::fmt::Display for LifecycleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connection(e) => write!(f, "connection error: {e}"),
            Self::NotConnected(addr) => write!(f, "no connection to {addr}"),
            Self::AlreadyConnected(addr) => write!(f, "already connected to {addr}"),
        }
    }
}

impl std::error::Error for LifecycleError {}

impl From<PeerConnectionError> for LifecycleError {
    fn from(e: PeerConnectionError) -> Self {
        Self::Connection(e)
    }
}

impl ConnectionLifecycleManager {
    /// Create a new lifecycle manager with the given shared state.
    ///
    /// # Arguments
    ///
    /// * `network_magic` — Cardano network identifier for handshakes
    /// * `peer_sharing` — Whether to advertise peer sharing support
    /// * `connect_timeout` — TCP connect timeout for outbound connections
    /// * `candidate_chains` — Shared map for ChainSync -> BlockFetch coordination
    /// * `fetched_blocks_tx` — Channel for BlockFetch tasks to send blocks to the run loop
    /// * `block_announcement_tx` — Broadcast channel for block announcements
    /// * `chain_db` — Shared ChainDB reference
    /// * `ledger_state` — Shared LedgerState reference
    /// * `byron_epoch_length` — Byron epoch length in slots
    /// * `security_param` — Ouroboros k (rollback limit); 2160 mainnet, 432 preview
    /// * `active_slots_coeff` — Shelley genesis active slots coefficient (0.05 on mainnet/preview)
    /// * `metrics` — Prometheus metrics handle for recording peer latencies
    /// * `mempool` — Shared mempool for TxSubmission2 tx relay
    /// * `peer_failure_tx` — Channel for protocol tasks to report peer failures
    /// * `keepalive_rtt_tx` — Channel for KeepAlive tasks to report per-pong RTT
    /// * `gsm_event_tx` — GSM event sender for ChainSync tasks
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        network_magic: u64,
        initiator_only: bool,
        peer_sharing: bool,
        connect_timeout: Duration,
        candidate_chains: Arc<RwLock<HashMap<SocketAddr, CandidateChainState>>>,
        fetched_blocks_tx: mpsc::Sender<FetchedBlock>,
        block_announcement_tx: broadcast::Sender<BlockAnnouncement>,
        chain_db: Arc<RwLock<ChainDB>>,
        ledger_state: Arc<RwLock<LedgerState>>,
        byron_epoch_length: u64,
        security_param: u64,
        active_slots_coeff: f64,
        metrics: Arc<NodeMetrics>,
        mempool: Arc<Mempool>,
        peer_failure_tx: mpsc::Sender<SocketAddr>,
        keepalive_rtt_tx: mpsc::Sender<(SocketAddr, f64)>,
        gsm_event_tx: tokio::sync::mpsc::Sender<crate::gsm::GsmEvent>,
    ) -> Self {
        Self {
            connections: HashMap::new(),
            network_magic,
            initiator_only,
            peer_sharing,
            connect_timeout,
            candidate_chains,
            fetched_blocks_tx,
            block_announcement_tx,
            chain_db,
            ledger_state,
            byron_epoch_length,
            security_param,
            active_slots_coeff,
            active_fetcher: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            max_fetched_slot: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            metrics,
            mempool,
            peer_failure_tx,
            keepalive_rtt_tx,
            gsm_event_tx,
        }
    }

    // ─── Temperature Transitions ────────────────────────────────────────────

    /// Promote a cold peer to warm: TCP connect + handshake + start KeepAlive.
    ///
    /// This is the Cold -> Warm transition from Haskell's `PeerStateActions`.
    /// Creates a new `PeerConnection` (TCP + mux + handshake) and starts
    /// the KeepAlive warm-temperature protocol.
    ///
    /// # Errors
    ///
    /// Returns `LifecycleError::AlreadyConnected` if a connection already exists,
    /// or `LifecycleError::Connection` on TCP/handshake failure.
    pub async fn promote_to_warm(
        &mut self,
        addr: SocketAddr,
        peer_manager: &mut NodePeerManager,
    ) -> Result<(), LifecycleError> {
        if self.connections.contains_key(&addr) {
            return Err(LifecycleError::AlreadyConnected(addr));
        }

        info!(%addr, "promoting cold -> warm: connecting");

        // Time the TCP connect + handshake for RTT measurement.
        let connect_start = std::time::Instant::now();

        // Establish TCP connection, create mux, run handshake.
        let mut conn = PeerConnection::connect(
            addr,
            self.network_magic,
            self.initiator_only,
            self.peer_sharing,
            Some(self.connect_timeout),
        )
        .await?;

        // Record handshake RTT (includes TCP connect + mux setup + handshake exchange).
        let rtt_ms = connect_start.elapsed().as_secs_f64() * 1000.0;
        self.metrics.record_handshake_rtt(rtt_ms);

        // Start warm protocols (KeepAlive).
        let keepalive_fn = self.make_keepalive_task(addr);
        conn.start_warm_protocols(keepalive_fn)?;

        // Update peer manager state.
        peer_manager.peer_connected(&addr, ConnectionDirection::Outbound);

        self.connections.insert(addr, conn);
        info!(%addr, rtt_ms = format_args!("{rtt_ms:.0}"), "cold -> warm complete");
        Ok(())
    }

    /// Spawn a background task that performs the TCP connect + handshake for `addr`.
    ///
    /// This is the non-blocking alternative to `promote_to_warm`. The slow I/O
    /// (TCP connect + N2N handshake, up to `connect_timeout`) runs in a separate
    /// Tokio task rather than inside the main `select!` loop.  When the task
    /// completes it sends a [`ConnectResult`] on `tx`; the main loop receives
    /// it and calls [`Self::register_warm_connection`] (on success) or marks the
    /// peer as failed (on error).
    ///
    /// The caller is responsible for tracking in-flight addresses to avoid
    /// spawning duplicate tasks for the same peer.
    pub fn spawn_connect(&self, addr: SocketAddr, tx: mpsc::Sender<ConnectResult>) {
        let network_magic = self.network_magic;
        let initiator_only = self.initiator_only;
        let peer_sharing = self.peer_sharing;
        let connect_timeout = self.connect_timeout;
        let metrics = Arc::clone(&self.metrics);

        tokio::spawn(async move {
            let start = std::time::Instant::now();
            match PeerConnection::connect(
                addr,
                network_magic,
                initiator_only,
                peer_sharing,
                Some(connect_timeout),
            )
            .await
            {
                Ok(conn) => {
                    let rtt_ms = start.elapsed().as_secs_f64() * 1000.0;
                    metrics.record_handshake_rtt(rtt_ms);
                    // Ignore send errors — the main loop may have shut down.
                    let _ = tx.send(Ok((addr, conn, rtt_ms))).await;
                }
                Err(e) => {
                    let _ = tx.send(Err((addr, e.to_string()))).await;
                }
            }
        });
    }

    /// Register a peer that connected successfully in a background task as warm.
    ///
    /// This is the fast, synchronous post-connect step: starts the KeepAlive
    /// warm protocol on the ready connection and updates the peer manager.
    /// It must be called from the main run loop after receiving an `Ok` result
    /// from a [`Self::spawn_connect`] task.
    ///
    /// # Errors
    ///
    /// Returns `LifecycleError::AlreadyConnected` if a connection for `addr`
    /// was registered in the meantime (e.g., from a concurrent inbound connect).
    /// The caller should silently discard the duplicate `PeerConnection` in that
    /// case — it will be dropped and the mux will close gracefully.
    pub fn register_warm_connection(
        &mut self,
        addr: SocketAddr,
        mut conn: PeerConnection,
        rtt_ms: f64,
        peer_manager: &mut NodePeerManager,
    ) -> Result<(), LifecycleError> {
        if self.connections.contains_key(&addr) {
            return Err(LifecycleError::AlreadyConnected(addr));
        }

        let keepalive_fn = self.make_keepalive_task(addr);
        conn.start_warm_protocols(keepalive_fn)?;

        peer_manager.peer_connected(&addr, ConnectionDirection::Outbound);
        self.connections.insert(addr, conn);
        info!(%addr, rtt_ms = format_args!("{rtt_ms:.0}"), "cold -> warm complete (background)");
        Ok(())
    }

    /// Promote a warm peer to hot: start ChainSync + BlockFetch + TxSubmission2.
    ///
    /// This is the Warm -> Hot transition from Haskell's `PeerStateActions`.
    /// The existing mux connection stays alive — only new protocol tasks are
    /// spawned on channels that were created during the initial connect.
    ///
    /// # Errors
    ///
    /// Returns `LifecycleError::NotConnected` if no connection exists, or
    /// `LifecycleError::Connection` if protocol channels are unavailable
    /// (e.g., hot protocols already running).
    pub async fn promote_to_hot(
        &mut self,
        addr: SocketAddr,
        peer_manager: &mut NodePeerManager,
    ) -> Result<(), LifecycleError> {
        if !self.connections.contains_key(&addr) {
            return Err(LifecycleError::NotConnected(addr));
        }

        info!(%addr, "promoting warm -> hot: starting sync protocols");

        // Create task closures BEFORE taking the mutable borrow on connections,
        // since the factory methods borrow `self` immutably.
        let chainsync_fn = self.make_chainsync_task(addr);
        let blockfetch_fn = self.make_blockfetch_task(addr);
        let txsubmission_fn = self.make_txsubmission_task(addr);

        let conn = self.connections.get_mut(&addr).unwrap();
        conn.start_hot_protocols(chainsync_fn, blockfetch_fn, txsubmission_fn)?;

        // Update peer manager: warm -> hot.
        peer_manager.inner.promote_to_hot(&addr);

        // Update connection state: idle → active (outbound or inbound).
        if peer_manager.is_inbound(&addr) {
            peer_manager.mark_inbound_active(&addr);
        } else {
            peer_manager.mark_outbound_active(&addr);
        }

        info!(%addr, "warm -> hot complete");
        Ok(())
    }

    /// Demote a hot peer to warm: stop ChainSync + BlockFetch + TxSubmission2.
    ///
    /// This is the Hot -> Warm transition from Haskell's `PeerStateActions`.
    /// Only the hot protocol tasks are stopped; the mux and KeepAlive continue
    /// running. The peer can be re-promoted to hot later without reconnecting.
    ///
    /// # Errors
    ///
    /// Returns `LifecycleError::NotConnected` if no connection exists.
    pub async fn demote_to_warm(
        &mut self,
        addr: SocketAddr,
        peer_manager: &mut NodePeerManager,
    ) -> Result<(), LifecycleError> {
        let conn = self
            .connections
            .get_mut(&addr)
            .ok_or(LifecycleError::NotConnected(addr))?;

        info!(%addr, "demoting hot -> warm: stopping sync protocols");

        conn.stop_hot_protocols().await;

        // Clear candidate chain state for this peer (no longer syncing).
        {
            let mut chains = self.candidate_chains.write().await;
            chains.remove(&addr);
        }

        // Update peer manager: hot -> warm.
        peer_manager.inner.demote_to_warm(&addr);

        // Update connection state: active → idle (outbound or inbound).
        if peer_manager.is_inbound(&addr) {
            peer_manager.mark_inbound_idle(&addr);
        } else {
            peer_manager.mark_outbound_idle(&addr);
        }

        info!(%addr, "hot -> warm complete");
        Ok(())
    }

    /// Demote a warm peer to cold: stop all protocols, close connection.
    ///
    /// This is the Warm -> Cold transition from Haskell's `PeerStateActions`.
    /// Shuts down the entire connection (all protocol tasks + mux + TCP).
    /// The `PeerConnection` is removed from the connections map.
    ///
    /// # Errors
    ///
    /// Returns `LifecycleError::NotConnected` if no connection exists.
    pub async fn demote_to_cold(
        &mut self,
        addr: SocketAddr,
        peer_manager: &mut NodePeerManager,
    ) -> Result<(), LifecycleError> {
        let mut conn = self
            .connections
            .remove(&addr)
            .ok_or(LifecycleError::NotConnected(addr))?;

        info!(%addr, "demoting warm -> cold: closing connection");

        // Mark connection as terminating before shutdown (for metrics).
        peer_manager.mark_terminating(&addr);

        conn.shutdown().await;

        // Clear candidate chain state.
        {
            let mut chains = self.candidate_chains.write().await;
            chains.remove(&addr);
        }

        // Update peer manager — removes connection state entirely.
        peer_manager.peer_disconnected(&addr);

        info!(%addr, "warm -> cold complete");
        Ok(())
    }

    // ─── Inbound Connection Handling ────────────────────────────────────────

    /// Accept an inbound connection from a peer.
    ///
    /// If we already have an outbound connection to this peer, the Haskell
    /// node promotes the connection to Duplex mode. We handle this by marking
    /// the existing connection as duplex in the peer manager. If no connection
    /// exists, we create a new `PeerConnection` from the accepted stream and
    /// start warm protocols.
    ///
    /// # Arguments
    ///
    /// * `stream` — Already-accepted TCP stream from the listener
    /// * `addr` — Remote peer socket address
    /// * `peer_manager` — Node peer manager for state tracking
    pub async fn accept_inbound(
        &mut self,
        stream: tokio::net::TcpStream,
        addr: SocketAddr,
        peer_manager: &mut NodePeerManager,
    ) -> Result<(), LifecycleError> {
        // Check for simultaneous open (we already have an outbound connection).
        if self.connections.contains_key(&addr) {
            // Haskell promotes to Duplex: both initiator and responder protocols
            // share the same connection. For now, mark as duplex and close the
            // inbound stream (the outbound connection stays).
            info!(%addr, "simultaneous open: marking existing connection as duplex");
            peer_manager.mark_peer_duplex(&addr);
            // Drop the inbound stream — our outbound connection handles everything.
            drop(stream);
            return Ok(());
        }

        info!(%addr, "accepting inbound connection");

        // Time the TCP accept + handshake for RTT measurement.
        let accept_start = std::time::Instant::now();

        // Accept: create mux from stream, run handshake as server.
        let mut conn = PeerConnection::accept(
            stream,
            addr,
            self.network_magic,
            self.initiator_only,
            self.peer_sharing,
        )
        .await?;

        // Record handshake RTT (includes mux setup + handshake exchange).
        let rtt_ms = accept_start.elapsed().as_secs_f64() * 1000.0;
        self.metrics.record_handshake_rtt(rtt_ms);

        // Start warm protocols (KeepAlive).
        let keepalive_fn = self.make_keepalive_task(addr);
        conn.start_warm_protocols(keepalive_fn)?;

        // Update peer manager.
        peer_manager.peer_connected(&addr, ConnectionDirection::Inbound);

        self.connections.insert(addr, conn);
        info!(%addr, "inbound connection accepted, warm protocols started");
        Ok(())
    }

    // ─── Governor Event Dispatch ────────────────────────────────────────────

    /// Handle a governor action by dispatching to the appropriate lifecycle method.
    ///
    /// This is the main integration point between the Governor (which decides
    /// what should happen) and the ConnectionLifecycleManager (which makes it
    /// happen). Called from the connection manager loop.
    ///
    /// Non-connection actions (like `DiscoverMore`) are ignored here — they
    /// are handled by the peer discovery subsystem.
    pub async fn handle_governor_action(
        &mut self,
        action: GovernorAction,
        peer_manager: &mut NodePeerManager,
    ) {
        match action {
            GovernorAction::PromoteToWarm(addr) => {
                if let Err(e) = self.promote_to_warm(addr, peer_manager).await {
                    warn!(%addr, error = %e, "failed to promote cold -> warm");
                    peer_manager.peer_failed(&addr);
                }
            }
            GovernorAction::PromoteToHot(addr) => {
                if let Err(e) = self.promote_to_hot(addr, peer_manager).await {
                    warn!(%addr, error = %e, "failed to promote warm -> hot");
                    // Demote back to cold on hot promotion failure — the connection
                    // may be in a bad state.
                    peer_manager.mark_terminating(&addr);
                    if let Some(mut conn) = self.connections.remove(&addr) {
                        conn.shutdown().await;
                    }
                    peer_manager.peer_failed(&addr);
                }
            }
            GovernorAction::DemoteToWarm(addr) => {
                if let Err(e) = self.demote_to_warm(addr, peer_manager).await {
                    warn!(%addr, error = %e, "failed to demote hot -> warm");
                }
            }
            GovernorAction::DemoteToCold(addr) => {
                if let Err(e) = self.demote_to_cold(addr, peer_manager).await {
                    warn!(%addr, error = %e, "failed to demote warm -> cold");
                }
            }
            GovernorAction::DiscoverMore => {
                // Handled by the peer discovery subsystem, not the lifecycle manager.
                debug!("governor requested peer discovery (handled externally)");
            }
            GovernorAction::ForgetPeer(addr) => {
                // Remove the peer from the connection table and peer manager.
                // Cold churn evicts lowest-reputation non-topology peers.
                debug!(%addr, "governor forgetting low-reputation cold peer");
                if let Some(mut conn) = self.connections.remove(&addr) {
                    conn.shutdown().await;
                }
                peer_manager.inner.remove_peer(&addr);
            }
            GovernorAction::PeerShareRequest(addr) => {
                // PeerSharing active outreach — handled by the peer discovery
                // subsystem which owns the PeerSharingClient. The lifecycle
                // manager only logs the request.
                debug!(%addr, "governor requested PeerSharing outreach (handled externally)");
            }
        }
    }

    // ─── Connection Health ──────────────────────────────────────────────────

    /// Remove dead connections whose mux has terminated.
    ///
    /// Checks `is_alive()` on every connection and removes any that have died
    /// (mux task completed due to TCP close, error, etc.). Updates the peer
    /// manager to reflect the disconnection and clears candidate chain state.
    ///
    /// Should be called periodically from the connection manager loop.
    pub async fn cleanup_dead_connections(&mut self, peer_manager: &mut NodePeerManager) {
        let dead_addrs: Vec<SocketAddr> = self
            .connections
            .iter()
            .filter(|(_, conn)| !conn.is_alive())
            .map(|(addr, _)| *addr)
            .collect();

        if dead_addrs.is_empty() {
            return;
        }

        info!(count = dead_addrs.len(), "cleaning up dead connections");

        for addr in dead_addrs {
            // Mark connection as terminating before shutdown (for metrics).
            peer_manager.mark_terminating(&addr);

            if let Some(mut conn) = self.connections.remove(&addr) {
                // Best-effort shutdown (mux is already dead, but clean up tasks).
                conn.shutdown().await;
            }

            // Clear candidate chain state.
            {
                let mut chains = self.candidate_chains.write().await;
                chains.remove(&addr);
            }

            peer_manager.peer_disconnected(&addr);
            warn!(%addr, "removed dead connection");
        }
    }

    /// Get the number of active connections.
    pub fn connection_count(&self) -> usize {
        self.connections.len()
    }

    /// Check if a connection exists for the given address.
    pub fn has_connection(&self, addr: &SocketAddr) -> bool {
        self.connections.contains_key(addr)
    }

    /// Get the addresses of all connected peers.
    pub fn connected_addrs(&self) -> Vec<SocketAddr> {
        self.connections.keys().copied().collect()
    }

    /// Drain all connections, returning them as owned values.
    ///
    /// Used during shutdown to parallelize connection teardown without
    /// holding `&mut self` for the duration of each `shutdown().await`.
    pub fn drain_connections(&mut self) -> Vec<PeerConnection> {
        self.connections.drain().map(|(_, conn)| conn).collect()
    }

    // ─── Protocol Task Factories ────────────────────────────────────────────
    //
    // Each factory creates a closure matching the `ProtocolTaskFn` signature
    // that captures the shared state it needs. The `PeerConnection` spawns
    // these closures as tokio tasks when protocols are started.

    /// Create the KeepAlive protocol task closure.
    ///
    /// The KeepAlive protocol sends periodic pings to detect dead connections.
    /// Runs for the entire Warm lifetime of the connection.
    ///
    /// In Haskell, KeepAlive uses a 90-second interval and the Governor
    /// monitors RTT measurements from responses.
    fn make_keepalive_task(&self, addr: SocketAddr) -> ProtocolTaskFn {
        let peer_failure_tx = self.peer_failure_tx.clone();
        let keepalive_rtt_tx = self.keepalive_rtt_tx.clone();
        Box::new(move |mut channel, cancel| {
            Box::pin(async move {
                // CRITICAL: Delay the first KeepAlive ping until AFTER Hot protocols
                // have started and sent their first messages. The Haskell peer uses
                // StartOnDemandAny for the KeepAlive responder — it only starts when
                // ANY on-demand protocol receives data. If we send KeepAlive before
                // ChainSync/TxSubmission2 send their first messages, the peer has no
                // responder registered and RSTs the connection.
                //
                // In Haskell, this works because KeepAlive is in the Established
                // bundle and Hot protocols start at the same time with StartEagerly,
                // so ChainSync/TxSubmission data arrives before the first KeepAlive.
                //
                // We delay 2 seconds to ensure Hot protocols are active first.
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;

                // Per-peer RTT channel: each pong sends the RTT here, which the
                // spawned forwarder relays to the main loop with the peer address.
                let (rtt_tx, mut rtt_rx) = tokio::sync::mpsc::channel::<f64>(8);

                // Forwarder task: tags each RTT measurement with the peer address
                // and sends it to the main run loop for PeerManager EWMA + gauge updates.
                let ka_rtt_tx = keepalive_rtt_tx;
                let fwd_addr = addr;
                tokio::spawn(async move {
                    while let Some(rtt_ms) = rtt_rx.recv().await {
                        let _ = ka_rtt_tx.try_send((fwd_addr, rtt_ms));
                    }
                });

                let client = dugite_network::KeepAliveClient::new(
                    dugite_network::DEFAULT_KEEPALIVE_INTERVAL,
                    cancel,
                )
                .with_rtt_sender(rtt_tx);
                match client.run(&mut channel).await {
                    Ok(_rtt) => debug!(%addr, "keepalive task completed"),
                    Err(dugite_network::error::ProtocolError::KeepAliveTimeout {
                        consecutive_failures,
                    }) => {
                        warn!(
                            %addr,
                            consecutive_failures,
                            "keepalive: peer unresponsive, reporting failure",
                        );
                        let _ = peer_failure_tx.try_send(addr);
                    }
                    Err(e) => debug!(%addr, "keepalive error: {e}"),
                }
            })
        })
    }

    /// Create the ChainSync protocol task closure for a specific peer.
    ///
    /// The ChainSync client streams block headers from the peer, finds
    /// the intersection point with our chain, then pipelines header downloads.
    /// Headers are stored in `candidate_chains` for the BlockFetch decision
    /// task to consume. Does NOT fetch blocks — that's the BlockFetch
    /// decision task's responsibility.
    ///
    /// Delegates to [`super::sync::chainsync_client_task()`] which implements
    /// the full pipelined ChainSync protocol loop.
    fn make_chainsync_task(&self, addr: SocketAddr) -> ProtocolTaskFn {
        let candidate_chains = self.candidate_chains.clone();
        let chain_db = self.chain_db.clone();
        let ledger_state = self.ledger_state.clone();
        let byron_epoch_length = self.byron_epoch_length;
        let security_param = self.security_param;
        let active_slots_coeff = self.active_slots_coeff;
        let metrics = self.metrics.clone();
        let gsm_event_tx = self.gsm_event_tx.clone();

        Box::new(move |channel, cancel| {
            Box::pin(async move {
                info!(%addr, "chainsync task started");
                if let Err(e) = super::sync::chainsync_client_task(
                    channel,
                    addr,
                    candidate_chains,
                    chain_db,
                    ledger_state,
                    byron_epoch_length,
                    security_param,
                    active_slots_coeff,
                    metrics,
                    cancel,
                    gsm_event_tx,
                )
                .await
                {
                    warn!(%addr, error = %e, "chainsync task failed");
                }
                debug!(%addr, "chainsync task exiting");
            })
        })
    }

    /// Create the BlockFetch protocol task closure for a specific peer.
    ///
    /// The BlockFetch client receives fetch requests from the BlockFetch
    /// decision task and downloads full blocks from the peer. Downloaded
    /// blocks are sent to the main run loop via `fetched_blocks_tx`.
    ///
    /// Real implementation will be provided by Task 3.
    fn make_blockfetch_task(&self, addr: SocketAddr) -> ProtocolTaskFn {
        let fetched_blocks_tx = self.fetched_blocks_tx.clone();
        let candidate_chains = self.candidate_chains.clone();
        let chain_db_for_fetch = self.chain_db.clone();
        let bel = self.byron_epoch_length;
        // Shared flag: only ONE BlockFetch worker is active at a time.
        // Matches Haskell's bfcMaxConcurrencyBulkSync = 1.
        let active_fetcher = self.active_fetcher.clone();
        let _max_fetched_slot = self.max_fetched_slot.clone();
        let metrics_clone = self.metrics.clone();
        let peer_failure_tx = self.peer_failure_tx.clone();

        Box::new(move |mut channel, cancel| {
            Box::pin(async move {
                // BlockFetch worker: fetches blocks from this peer's candidate_chains.
                //
                // CRITICAL: Only ONE worker fetches at a time (matching Haskell's
                // bfcMaxConcurrencyBulkSync = 1). Workers compete for the
                // active_fetcher flag. The first to claim it becomes the sole
                // fetcher; others poll periodically to check if they should
                // take over (e.g., if the active fetcher's peer disconnects).
                use dugite_network::codec::Point as CodecPoint;
                use dugite_network::protocol::blockfetch::client::BlockFetchClient;

                // Per-worker dedup set: tracks block hashes successfully downloaded
                // in this worker's lifetime.  We do NOT drain `pending_headers` from
                // `candidate_chains` because that would permanently lose headers if
                // the connection drops mid-fetch (the ChainSync task will not
                // re-populate already-streamed headers until a rollback, causing
                // multi-minute sync stalls).  Instead we read headers in-place and
                // skip any whose hash is already in this set.
                let mut fetched_hashes: std::collections::HashSet<[u8; 32]> =
                    std::collections::HashSet::new();

                info!(%addr, "blockfetch worker started (waiting for turn)");

                let mut poll_ticker = tokio::time::interval(std::time::Duration::from_millis(500));
                poll_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

                loop {
                    tokio::select! {
                        biased;
                        _ = cancel.cancelled() => {
                            // Release the active fetcher flag if we hold it.
                            // Use hash of full SocketAddr (IP + port) for unique peer ID.
                            let mut hasher = std::collections::hash_map::DefaultHasher::new();
                            addr.hash(&mut hasher);
                            let cancel_id = hasher.finish() | 1; // ensure non-zero
                            let _ = active_fetcher.compare_exchange(
                                cancel_id,
                                0,
                                std::sync::atomic::Ordering::SeqCst,
                                std::sync::atomic::Ordering::SeqCst,
                            );
                            debug!(%addr, "blockfetch worker cancelled");
                            break;
                        }
                        _ = poll_ticker.tick() => {
                            // Only ONE worker fetches at a time to prevent duplicate
                            // downloads (matching Haskell's bfcMaxConcurrencyBulkSync=1).
                            let my_id: u64 = {
                                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                                addr.hash(&mut hasher);
                                hasher.finish() | 1
                            };
                            let claimed = active_fetcher.compare_exchange(
                                0,
                                my_id,
                                std::sync::atomic::Ordering::SeqCst,
                                std::sync::atomic::Ordering::SeqCst,
                            ).is_ok();
                            let current = active_fetcher.load(std::sync::atomic::Ordering::SeqCst);
                            if !claimed && current != my_id {
                                continue;
                            }

                            // Build the list of headers to fetch from this peer.
                            //
                            // KEY INVARIANT: we do NOT drain `pending_headers`.
                            // Headers remain in `candidate_chains` so they survive
                            // a mid-fetch connection drop.  Instead we skip any
                            // header whose hash is already in `fetched_hashes`
                            // (downloaded by this worker in an earlier iteration)
                            // or whose slot is <= max_fetched_slot (downloaded by
                            // another peer's worker).  This prevents both re-fetch
                            // loops and stalls caused by lost headers.
                            //
                            // Read chain_db tip BEFORE acquiring candidate_chains lock
                            // to avoid calling blocking_read() inside an async context
                            // (which panics with "Cannot block the current thread from
                            // within a runtime").
                            // Use ChainDB applied tip to filter — NOT max_fetched_slot.
                            // max_fetched_slot jumps to the chain tip when any worker
                            // downloads tip blocks, filtering out all gap blocks for
                            // every worker and causing 10+ minute sync stalls.
                            // The applied tip only advances when blocks are actually
                            // applied to the ledger in order, so gap blocks are never
                            // filtered out.
                            let applied_slot = {
                                let db = chain_db_for_fetch.read().await;
                                db.tip_slot().0
                            };
                            let headers_to_fetch = {
                                let chains = candidate_chains.read().await;
                                if let Some(state) = chains.get(&addr) {
                                    let filtered: Vec<_> = state.pending_headers.iter()
                                        .filter(|h| {
                                            h.slot > applied_slot
                                                && !fetched_hashes.contains(&h.hash)
                                        })
                                        .cloned()
                                        .collect();
                                    if filtered.is_empty() {
                                        active_fetcher.store(0, std::sync::atomic::Ordering::SeqCst);
                                    }
                                    filtered
                                } else {
                                    active_fetcher.store(0, std::sync::atomic::Ordering::SeqCst);
                                    continue;
                                }
                            };

                            if headers_to_fetch.is_empty() {
                                continue;
                            }

                            info!(
                                %addr,
                                count = headers_to_fetch.len(),
                                first_slot = headers_to_fetch.first().map(|h| h.slot).unwrap_or(0),
                                last_slot = headers_to_fetch.last().map(|h| h.slot).unwrap_or(0),
                                "BlockFetch: active fetcher, downloading blocks",
                            );

                            // Batch headers into ranges for efficient fetching.
                            // A single MsgRequestRange(from, to) fetches all blocks
                            // between two points, avoiding per-block round-trips.
                            let ranges: Vec<(CodecPoint, CodecPoint)> = {
                                let mut result = Vec::new();
                                let mut i = 0;
                                while i < headers_to_fetch.len() {
                                    let start = i;
                                    // Batch up to 100 consecutive headers per range
                                    let end = (i + 100).min(headers_to_fetch.len()) - 1;
                                    let from = CodecPoint::Specific(
                                        headers_to_fetch[start].slot,
                                        headers_to_fetch[start].hash,
                                    );
                                    let to = CodecPoint::Specific(
                                        headers_to_fetch[end].slot,
                                        headers_to_fetch[end].hash,
                                    );
                                    result.push((from, to));
                                    i = end + 1;
                                }
                                result
                            };

                            debug!(%addr, ranges = ranges.len(), headers = headers_to_fetch.len(), "BlockFetch: fetching in batched ranges");
                            for (from, to) in ranges {
                                let peer = addr;
                                let range_to_slot = match &to {
                                    CodecPoint::Specific(s, _) => *s,
                                    CodecPoint::Origin => 0,
                                };

                                // Collect decoded blocks in a local Vec inside the
                                // sync callback, then send them via `.send().await`
                                // after `fetch_range` returns.
                                //
                                // IMPORTANT: Do NOT call `tx.blocking_send()` inside
                                // the callback.  `fetch_range` takes a *synchronous*
                                // `FnMut` callback and calls it from within the tokio
                                // async runtime.  `blocking_send` panics with
                                // "Cannot block the current thread from within a
                                // runtime" whenever the channel is full and it tries
                                // to park the calling thread — exactly the crash we
                                // observed.  Collecting into a Vec and awaiting the
                                // sends outside the callback avoids the panic while
                                // preserving ordering and backpressure.
                                let mut decoded_blocks: Vec<FetchedBlock> = Vec::new();

                                let fetch_start = std::time::Instant::now();
                                let fetch_result = tokio::time::timeout(
                                    FETCH_RANGE_TIMEOUT,
                                    BlockFetchClient::fetch_range(
                                        &mut channel,
                                        from,
                                        to,
                                        |block_cbor| {
                                            match dugite_serialization::multi_era::decode_block_with_byron_epoch_length(
                                                &block_cbor, bel,
                                            ) {
                                                Ok(block) => {
                                                    let slot = block.slot().0;
                                                    debug!(%addr, slot, block_no = block.block_number().0, "BlockFetch: block decoded");
                                                    decoded_blocks.push(FetchedBlock {
                                                        peer,
                                                        block,
                                                        tip_slot: range_to_slot,
                                                        tip_hash: [0u8; 32],
                                                        tip_block_number: 0,
                                                    });
                                                }
                                                Err(e) => {
                                                    warn!(%addr, "block decode error: {e}");
                                                }
                                            }
                                            Ok(())
                                        },
                                    ),
                                ).await;
                                match fetch_result {
                                    Ok(Ok(count)) => {
                                        let fetch_ms = fetch_start.elapsed().as_secs_f64() * 1000.0;
                                        metrics_clone.record_block_fetch_latency(fetch_ms);
                                        debug!(%addr, count, fetch_ms, "BlockFetch: range complete");
                                    }
                                    Ok(Err(e)) => {
                                        warn!(%addr, "BlockFetch error: {e}");
                                        active_fetcher.store(0, std::sync::atomic::Ordering::SeqCst);
                                        let _ = peer_failure_tx.try_send(addr);
                                        return;
                                    }
                                    Err(_elapsed) => {
                                        // Fetch deadline exceeded — peer is stalled or
                                        // TCP connection is half-open. Release active
                                        // fetcher so another peer can take over, and
                                        // report the failure for reputation scoring.
                                        warn!(
                                            %addr,
                                            timeout_secs = FETCH_RANGE_TIMEOUT.as_secs(),
                                            "BlockFetch range timed out, releasing fetcher",
                                        );
                                        active_fetcher.store(0, std::sync::atomic::Ordering::SeqCst);
                                        let _ = peer_failure_tx.try_send(addr);
                                        return;
                                    }
                                }

                                // Send all blocks collected for this range using
                                // `.send().await` — which correctly yields to the
                                // scheduler instead of blocking the thread.
                                for fetched in decoded_blocks {
                                    let slot = fetched.block.slot().0;
                                    if let Err(e) = fetched_blocks_tx.send(fetched).await {
                                        warn!(%addr, slot, "send to run loop failed (channel closed): {e}");
                                        // Channel closed means the run loop exited.
                                        // Release the active fetcher and stop.
                                        active_fetcher.store(0, std::sync::atomic::Ordering::SeqCst);
                                        return;
                                    }
                                }
                            }

                            // Record all fetched hashes in the per-worker dedup set
                            // so subsequent iterations of this worker's loop skip
                            // them without consulting the candidate_chains lock.
                            for h in &headers_to_fetch {
                                fetched_hashes.insert(h.hash);
                            }

                            // Note: we do NOT update max_fetched_slot here.
                            // Per-worker dedup uses fetched_hashes (hash-based).
                            // Cross-worker dedup uses the applied ChainDB tip.
                            // max_fetched_slot caused sync stalls by jumping to
                            // the chain tip and filtering out all gap blocks.
                        }
                    }
                }
            })
        })
    }

    /// Create the TxSubmission2 protocol task closure for a specific peer.
    ///
    /// The TxSubmission2 protocol relays transactions between peers. As the
    /// initiator, we respond to the server's requests for transaction IDs
    /// and transaction bodies from our mempool via `TxSubmissionClient`.
    fn make_txsubmission_task(&self, addr: SocketAddr) -> ProtocolTaskFn {
        let mempool = self.mempool.clone();
        Box::new(move |mut channel, cancel| {
            Box::pin(async move {
                let source = MempoolTxSource::new(mempool);
                tokio::select! {
                    result = dugite_network::TxSubmissionClient::run(&mut channel, &source) => {
                        match result {
                            Ok(()) => debug!(%addr, "txsubmission2 client completed"),
                            Err(e) => debug!(%addr, "txsubmission2 client error: {e}"),
                        }
                    }
                    _ = cancel.cancelled() => {
                        debug!(%addr, "txsubmission2 task cancelled");
                    }
                }
            })
        })
    }
}

// ─── MempoolTxSource ─────────────────────────────────────────────────────────

/// Adapts `Mempool` to the `TxSource` trait for TxSubmission2 tx relay.
///
/// Tracks which tx IDs have been yielded to the remote peer via an internal
/// cursor over the mempool's ordered tx list. `get_tx_ids` acknowledges
/// previously sent IDs and returns the next batch.
///
/// Interior mutability via `Mutex` is used because `TxSource::get_tx_ids`
/// takes `&self` but we need to update the outstanding queue. The mutex is
/// uncontended — only the single TxSubmission2 client task accesses it.
struct MempoolTxSource {
    mempool: Arc<Mempool>,
    /// Tx hashes that have been yielded but not yet acknowledged by the peer.
    outstanding: std::sync::Mutex<std::collections::VecDeque<dugite_primitives::hash::Hash32>>,
}

impl MempoolTxSource {
    fn new(mempool: Arc<Mempool>) -> Self {
        Self {
            mempool,
            outstanding: std::sync::Mutex::new(std::collections::VecDeque::new()),
        }
    }
}

impl TxSource for MempoolTxSource {
    fn get_tx_ids(&self, ack_count: u16, max_count: u16) -> Vec<TxIdAndSize> {
        let mut outstanding = self.outstanding.lock().unwrap();

        // Acknowledge previously yielded tx IDs.
        for _ in 0..ack_count {
            outstanding.pop_front();
        }

        // Prune outstanding entries for txs no longer in the mempool
        // (included in a block or expired).  Without this, txs that
        // leave the mempool stay in `outstanding` permanently, blocking
        // re-announcement if the same tx is resubmitted.
        outstanding.retain(|h| self.mempool.get_tx_size(h).is_some());

        // Collect the set of already-outstanding hashes for dedup.
        let already_sent: std::collections::HashSet<dugite_primitives::hash::Hash32> =
            outstanding.iter().copied().collect();

        // Get ordered tx hashes from mempool and yield new ones.
        let all_hashes = self.mempool.tx_hashes_ordered();
        let mut result = Vec::new();
        for hash in all_hashes {
            if result.len() >= max_count as usize {
                break;
            }
            if already_sent.contains(&hash) {
                continue;
            }
            if let Some(size) = self.mempool.get_tx_size(&hash) {
                outstanding.push_back(hash);
                // Compute the full GenTx wire size including HFC envelope:
                //   array(2)[1] + era_id[1] + tag(24)[2] + bytes_header[1-3] + cbor_data[N]
                // bytes_header: 1 byte for size < 24, 2 bytes for < 256, 3 bytes for < 65536
                let bytes_header_len = if size < 24 {
                    1
                } else if size < 256 {
                    2
                } else {
                    3
                };
                let wire_size = 1 + 1 + 2 + bytes_header_len + size;
                result.push(TxIdAndSize {
                    era_id: 6, // Conway
                    tx_id: *hash.as_bytes(),
                    size_in_bytes: wire_size as u32,
                });
            }
        }
        result
    }

    fn get_txs(&self, tx_ids: &[(u8, [u8; 32])]) -> Vec<(u8, Vec<u8>)> {
        tx_ids
            .iter()
            .filter_map(|(era_id, id)| {
                let hash = dugite_primitives::hash::Hash32::from_bytes(*id);
                self.mempool.get_tx_cbor(&hash).map(|cbor| (*era_id, cbor))
            })
            .collect()
    }

    fn has_pending(&self) -> bool {
        !self.mempool.is_empty()
    }

    fn tx_notify(&self) -> Option<std::sync::Arc<tokio::sync::Notify>> {
        Some(self.mempool.tx_notify())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify CandidateChainState can be constructed and cloned.
    #[test]
    fn candidate_chain_state_roundtrip() {
        let state = CandidateChainState {
            tip_slot: 12345,
            tip_hash: [0xAB; 32],
            tip_block_number: 100,
            pending_headers: vec![PendingHeader {
                slot: 12345,
                hash: [0xAB; 32],
                header_cbor: vec![0x82, 0x01],
            }],
        };

        let cloned = state.clone();
        assert_eq!(cloned.tip_slot, 12345);
        assert_eq!(cloned.tip_hash, [0xAB; 32]);
        assert_eq!(cloned.tip_block_number, 100);
        assert_eq!(cloned.pending_headers.len(), 1);
        assert_eq!(cloned.pending_headers[0].slot, 12345);
    }

    /// Verify FetchedBlock can be constructed.
    #[test]
    fn fetched_block_construction() {
        // FetchedBlock contains a Block which requires real construction,
        // so we just verify the type exists and has the expected fields.
        let _: fn() -> usize = || std::mem::size_of::<FetchedBlock>();
    }

    /// Verify LifecycleError display formatting.
    #[test]
    fn lifecycle_error_display() {
        let addr: SocketAddr = "127.0.0.1:3001".parse().unwrap();

        let err = LifecycleError::NotConnected(addr);
        assert!(err.to_string().contains("no connection"));
        assert!(err.to_string().contains("127.0.0.1:3001"));

        let err = LifecycleError::AlreadyConnected(addr);
        assert!(err.to_string().contains("already connected"));

        let inner = PeerConnectionError::ConnectTimeout(addr);
        let err = LifecycleError::Connection(inner);
        assert!(err.to_string().contains("connection error"));
    }

    /// Verify LifecycleError From<PeerConnectionError> conversion.
    #[test]
    fn lifecycle_error_from_peer_connection_error() {
        let addr: SocketAddr = "127.0.0.1:3001".parse().unwrap();
        let inner = PeerConnectionError::ConnectTimeout(addr);
        let err: LifecycleError = inner.into();
        assert!(matches!(err, LifecycleError::Connection(_)));
    }

    /// Verify PendingHeader can be constructed.
    #[test]
    fn pending_header_construction() {
        let hdr = PendingHeader {
            slot: 999,
            hash: [0xFF; 32],
            header_cbor: vec![0x83, 0x01, 0x02],
        };
        assert_eq!(hdr.slot, 999);
        assert_eq!(hdr.header_cbor.len(), 3);
    }
}
