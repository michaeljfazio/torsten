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
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{broadcast, mpsc, RwLock};
use tracing::{debug, info, warn};

use torsten_network::peer::governor::GovernorAction;
use torsten_network::BlockAnnouncement;

use torsten_ledger::LedgerState;
use torsten_primitives::block::Block;
use torsten_storage::ChainDB;

use super::networking::{ConnectionDirection, NodePeerManager};
use super::peer_connection::{PeerConnection, PeerConnectionError, ProtocolTaskFn};

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
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        network_magic: u64,
        peer_sharing: bool,
        connect_timeout: Duration,
        candidate_chains: Arc<RwLock<HashMap<SocketAddr, CandidateChainState>>>,
        fetched_blocks_tx: mpsc::Sender<FetchedBlock>,
        block_announcement_tx: broadcast::Sender<BlockAnnouncement>,
        chain_db: Arc<RwLock<ChainDB>>,
        ledger_state: Arc<RwLock<LedgerState>>,
        byron_epoch_length: u64,
    ) -> Self {
        Self {
            connections: HashMap::new(),
            network_magic,
            peer_sharing,
            connect_timeout,
            candidate_chains,
            fetched_blocks_tx,
            block_announcement_tx,
            chain_db,
            ledger_state,
            byron_epoch_length,
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

        // Establish TCP connection, create mux, run handshake.
        let mut conn = PeerConnection::connect(
            addr,
            self.network_magic,
            self.peer_sharing,
            Some(self.connect_timeout),
        )
        .await?;

        // Start warm protocols (KeepAlive).
        let keepalive_fn = self.make_keepalive_task();
        conn.start_warm_protocols(keepalive_fn)?;

        // Update peer manager state.
        peer_manager.peer_connected(&addr, ConnectionDirection::Outbound);

        self.connections.insert(addr, conn);
        info!(%addr, "cold -> warm complete");
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

        conn.shutdown().await;

        // Clear candidate chain state.
        {
            let mut chains = self.candidate_chains.write().await;
            chains.remove(&addr);
        }

        // Update peer manager.
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

        // Accept: create mux from stream, run handshake as server.
        let mut conn =
            PeerConnection::accept(stream, addr, self.network_magic, self.peer_sharing).await?;

        // Start warm protocols (KeepAlive).
        let keepalive_fn = self.make_keepalive_task();
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
    fn make_keepalive_task(&self) -> ProtocolTaskFn {
        Box::new(move |_channel, cancel| {
            Box::pin(async move {
                // Placeholder: KeepAlive protocol logic will be implemented in
                // the protocol integration phase. For now, just wait for cancellation
                // to keep the task alive.
                //
                // Real implementation will:
                // 1. Send MsgKeepAlive with a random cookie every 90 seconds
                // 2. Wait for MsgKeepAliveResponse with matching cookie
                // 3. Measure RTT and report to peer manager
                // 4. Detect timeout (no response within interval) as peer failure
                cancel.cancelled().await;
                debug!("keepalive task cancelled");
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
                    cancel,
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
        let _fetched_blocks_tx = self.fetched_blocks_tx.clone();

        Box::new(move |_channel, cancel| {
            Box::pin(async move {
                // Placeholder: BlockFetch protocol logic will be implemented in Task 3.
                //
                // Real implementation will:
                // 1. Wait for fetch range requests from the BlockFetch decision task
                // 2. Send MsgRequestRange to the peer
                // 3. Receive MsgBlock responses, deserialize blocks
                // 4. Send FetchedBlock to fetched_blocks_tx for ledger application
                // 5. Handle MsgBatchDone / MsgNoBlocks responses
                info!(%addr, "blockfetch task started (placeholder)");
                cancel.cancelled().await;
                debug!(%addr, "blockfetch task cancelled");
            })
        })
    }

    /// Create the TxSubmission2 protocol task closure for a specific peer.
    ///
    /// The TxSubmission2 protocol relays transactions between peers. As the
    /// initiator, we respond to the server's requests for transaction IDs
    /// and transaction bodies from our mempool.
    ///
    /// Real implementation will be provided in a later task.
    fn make_txsubmission_task(&self, addr: SocketAddr) -> ProtocolTaskFn {
        Box::new(move |_channel, cancel| {
            Box::pin(async move {
                // Placeholder: TxSubmission2 protocol logic will be implemented later.
                //
                // Real implementation will:
                // 1. Send MsgInit to the server
                // 2. Respond to MsgRequestTxIds with tx IDs from our mempool
                // 3. Respond to MsgRequestTxs with full tx bodies
                // 4. Handle MsgDone when the server is finished
                info!(%addr, "txsubmission2 task started (placeholder)");
                cancel.cancelled().await;
                debug!(%addr, "txsubmission2 task cancelled");
            })
        })
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
