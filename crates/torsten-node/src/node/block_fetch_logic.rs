//! Independent BlockFetch decision task.
//!
//! # Haskell Architecture Reference
//!
//! In the Haskell cardano-node, `blockFetchLogic` runs as its own dedicated thread
//! (via `Ouroboros.Network.BlockFetch.blockFetchLogic`). It:
//!
//! 1. Reads candidate chain state from all connected peers via STM `TVar`s
//!    (updated by per-peer ChainSync mini-protocol tasks).
//! 2. Every decision interval (10ms for Praos, 40ms for Genesis), evaluates which
//!    blocks need to be fetched and from which peers.
//! 3. Issues `FetchRequest` ranges to per-peer BlockFetch client tasks via STM.
//! 4. Per-peer BlockFetch tasks download the blocks and deliver them to the chain
//!    selection / ledger application pipeline.
//!
//! This module provides the Rust equivalent using `tokio::sync` channels:
//!
//! - **`BlockFetchLogicTask`** — the decision loop that reads candidate chains
//!   (via `Arc<RwLock<HashMap>>`) and dispatches fetch ranges to per-peer workers
//!   (via `mpsc` channels).
//! - **`blockfetch_worker`** — per-peer worker function that receives fetch ranges,
//!   downloads blocks via `BlockFetchClient`, and sends decoded blocks to the main
//!   run loop.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, RwLock};
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, trace, warn};

use torsten_network::codec::Point;
use torsten_network::protocol::blockfetch::decision::{
    BlockFetchDecision, FetchRange, PeerFetchState,
};
use torsten_network::{BlockFetchClient, MuxChannel};

use super::connection_lifecycle::{CandidateChainState, FetchedBlock, PendingHeader};

/// Default decision interval for Praos consensus (10ms).
///
/// Matches Haskell's `blockFetchDecisionLoopInterval` for Praos.
/// Genesis mode uses 40ms instead.
const PRAOS_DECISION_INTERVAL: Duration = Duration::from_millis(10);

/// Default decision interval for Genesis consensus (40ms).
///
/// Genesis mode uses a longer interval because the chain selection
/// algorithm is more complex and the decision task should not starve
/// other tasks.
#[allow(dead_code)]
const GENESIS_DECISION_INTERVAL: Duration = Duration::from_millis(40);

/// Maximum number of consecutive headers to batch into a single fetch range.
///
/// Batching consecutive headers reduces the number of `MsgRequestRange` round-trips
/// while keeping individual fetches bounded so that slow peers don't block progress.
const MAX_BATCH_SIZE: usize = 100;

/// Timeout for in-flight blocks.  If a block has been in-flight for longer
/// than this, the entry is purged so the block can be re-fetched from
/// another peer.  This prevents sync stalls when a peer's TCP connection
/// dies silently (half-open) and the worker never reports back.
///
/// Set to 60s to match Haskell's `bfcFetchDeadlinePolicy` fetch deadline.
const IN_FLIGHT_TIMEOUT: Duration = Duration::from_secs(60);

/// Independent block fetch decision task.
///
/// Matches Haskell's `blockFetchLogic` thread: reads candidate chain state
/// from all ChainSync peers, decides which blocks to fetch, dispatches
/// fetch requests to per-peer BlockFetch channels.
///
/// Runs in its own tokio task, communicating via channels:
/// - Reads: `candidate_chains` (`Arc<RwLock<HashMap>>`, updated by ChainSync tasks)
/// - Reads: `current_tip_slot` (updated from ledger via the run loop)
/// - Writes: `fetch_senders` per peer (`mpsc`, consumed by per-peer BlockFetch tasks)
/// - Writes: `fetched_blocks_tx` (`mpsc`, consumed by main run loop)
///
/// # Lifecycle
///
/// Created by the run loop, started via `run()`. Peers are registered/deregistered
/// as connections are promoted to hot / demoted from hot. The task runs until the
/// cancellation token is triggered (node shutdown).
pub struct BlockFetchLogicTask {
    /// Shared candidate chain state from all peers.
    ///
    /// Written by per-peer ChainSync tasks. Read here to determine which
    /// pending headers need to be fetched and from which peers.
    candidate_chains: Arc<RwLock<HashMap<SocketAddr, CandidateChainState>>>,

    /// Current chain tip slot from the ledger.
    ///
    /// Used to skip headers for blocks we already have (slot <= current_tip_slot).
    /// Updated by the run loop as blocks are applied.
    current_tip_slot: u64,

    /// Decision interval — how often the task evaluates fetch decisions.
    ///
    /// 10ms for Praos (default), 40ms for Genesis.
    decision_interval: Duration,

    /// Per-peer fetch request senders.
    ///
    /// Key: peer socket address. Value: sender end of the channel consumed
    /// by that peer's `blockfetch_worker`. When the decision task determines
    /// that a peer should fetch certain ranges, it sends `Vec<FetchRange>`
    /// to the corresponding sender.
    fetch_senders: HashMap<SocketAddr, mpsc::Sender<Vec<FetchRange>>>,

    /// Channel to send decoded blocks to the main run loop.
    ///
    /// Both this task and the per-peer workers share clones of this sender.
    /// The run loop consumes `FetchedBlock` values and applies them to
    /// ChainDB + LedgerState.
    fetched_blocks_tx: mpsc::Sender<FetchedBlock>,

    /// Byron epoch length in slots (needed for block deserialization).
    byron_epoch_length: u64,

    /// Block hashes currently in-flight, mapped to the peer that was asked
    /// to fetch them and the timestamp when the request was dispatched.
    ///
    /// Prevents duplicate fetch requests for the same block across multiple
    /// decision iterations. Entries are added when ranges are dispatched and
    /// removed when blocks are received or ranges fail.
    ///
    /// Tracking per-peer allows cleanup when a peer disconnects: all blocks
    /// assigned to that peer are released for re-fetch from another peer.
    /// The timestamp enables timeout-based cleanup of stale entries when a
    /// peer's TCP connection dies silently (half-open).
    in_flight: HashMap<[u8; 32], (SocketAddr, Instant)>,

    /// The underlying decision engine that tracks queued/in-flight ranges
    /// and selects the optimal peer for each fetch.
    decision_engine: BlockFetchDecision,

    /// Cancellation token for graceful shutdown.
    cancel: CancellationToken,
}

impl BlockFetchLogicTask {
    /// Create a new BlockFetch decision task.
    ///
    /// # Arguments
    ///
    /// * `candidate_chains` — Shared map of per-peer candidate chain state
    /// * `fetched_blocks_tx` — Channel to send downloaded blocks to the run loop
    /// * `byron_epoch_length` — Byron epoch length for block deserialization
    /// * `cancel` — Cancellation token for graceful shutdown
    pub fn new(
        candidate_chains: Arc<RwLock<HashMap<SocketAddr, CandidateChainState>>>,
        fetched_blocks_tx: mpsc::Sender<FetchedBlock>,
        byron_epoch_length: u64,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            candidate_chains,
            current_tip_slot: 0,
            decision_interval: PRAOS_DECISION_INTERVAL,
            fetch_senders: HashMap::new(),
            fetched_blocks_tx,
            byron_epoch_length,
            in_flight: HashMap::new(),
            decision_engine: BlockFetchDecision::with_defaults(),
            cancel,
        }
    }

    /// Set the decision interval.
    ///
    /// Use `PRAOS_DECISION_INTERVAL` (10ms) for normal Praos operation or
    /// `GENESIS_DECISION_INTERVAL` (40ms) for Genesis mode.
    #[allow(dead_code)]
    pub fn set_decision_interval(&mut self, interval: Duration) {
        self.decision_interval = interval;
    }

    /// Update the current chain tip slot.
    ///
    /// Called by the run loop as blocks are applied to the ledger, so the
    /// decision task can skip headers for blocks we already have.
    pub fn update_tip_slot(&mut self, slot: u64) {
        self.current_tip_slot = slot;
    }

    /// Register a new peer's BlockFetch channel.
    ///
    /// Called when a peer is promoted to hot and its BlockFetch worker is spawned.
    /// The `fetch_tx` sender is the channel the worker reads fetch requests from.
    pub fn register_peer(&mut self, addr: SocketAddr, fetch_tx: mpsc::Sender<Vec<FetchRange>>) {
        debug!(%addr, "registering peer for block fetch");
        self.fetch_senders.insert(addr, fetch_tx);
    }

    /// Deregister a peer (disconnected or demoted from hot).
    ///
    /// Removes the peer's fetch sender and releases all in-flight blocks
    /// that were assigned to this peer, so they can be re-fetched from
    /// another peer.  Without this cleanup, blocks dispatched to a dead
    /// peer would stay in `in_flight` forever, starving the sync pipeline.
    pub fn deregister_peer(&mut self, addr: &SocketAddr) {
        let before = self.in_flight.len();
        self.in_flight.retain(|_, (peer, _)| peer != addr);
        let released = before - self.in_flight.len();
        if released > 0 {
            info!(
                %addr,
                released,
                "released in-flight blocks for deregistered peer"
            );
        }
        debug!(%addr, "deregistering peer from block fetch");
        self.fetch_senders.remove(addr);
    }

    /// Run the main decision loop.
    ///
    /// Ticks at the configured `decision_interval` and evaluates which blocks
    /// to fetch on each tick. Exits when the cancellation token is triggered.
    ///
    /// This is the entry point for the tokio task — call via:
    /// ```ignore
    /// tokio::spawn(async move { task.run().await });
    /// ```
    pub async fn run(&mut self) {
        info!(
            interval_ms = self.decision_interval.as_millis(),
            "block fetch decision task started"
        );

        let mut ticker = tokio::time::interval(self.decision_interval);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    self.evaluate_and_fetch().await;
                }
                _ = self.cancel.cancelled() => {
                    info!("block fetch decision task shutting down");
                    break;
                }
            }
        }
    }

    /// One iteration of the decision loop.
    ///
    /// For each peer's candidate chain:
    /// 1. Get pending headers (not yet fetched).
    /// 2. Skip headers for blocks we already have (slot <= current_tip_slot).
    /// 3. Skip headers already in-flight.
    /// 4. Batch consecutive headers into fetch ranges.
    /// 5. Dispatch ranges to the best available peer via their BlockFetch channel.
    async fn evaluate_and_fetch(&mut self) {
        // If no peers are registered, nothing to do.
        if self.fetch_senders.is_empty() {
            return;
        }

        // Purge stale in-flight entries.  If a peer's TCP connection dies
        // silently (half-open), the worker blocks indefinitely on recv() and
        // never reports back.  Without this cleanup the sync pipeline stalls
        // because the blocks can never be re-fetched from another peer.
        let now = Instant::now();
        let before = self.in_flight.len();
        self.in_flight
            .retain(|_, (_, dispatched_at)| now.duration_since(*dispatched_at) < IN_FLIGHT_TIMEOUT);
        let expired = before - self.in_flight.len();
        if expired > 0 {
            warn!(
                expired,
                remaining = self.in_flight.len(),
                "purged stale in-flight blocks (exceeded {}s timeout)",
                IN_FLIGHT_TIMEOUT.as_secs()
            );
        }

        // Read candidate chain state from all peers.
        let chains = self.candidate_chains.read().await;

        // Collect new pending headers from all peers, filtering out already-fetched
        // and already-in-flight blocks.
        let mut new_headers: Vec<(SocketAddr, PendingHeader)> = Vec::new();

        for (addr, state) in chains.iter() {
            // Only consider peers we have a fetch channel for.
            if !self.fetch_senders.contains_key(addr) {
                continue;
            }

            for header in &state.pending_headers {
                // Skip blocks we already have (at or before our tip).
                if header.slot <= self.current_tip_slot {
                    continue;
                }

                // Skip blocks already in-flight.
                if self.in_flight.contains_key(&header.hash) {
                    continue;
                }

                new_headers.push((*addr, header.clone()));
            }
        }

        // Release the read lock before doing I/O.
        drop(chains);

        if new_headers.is_empty() {
            return;
        }

        // Sort headers by slot so we batch consecutive ranges.
        new_headers.sort_by_key(|(_, h)| h.slot);

        // Batch consecutive headers into fetch ranges.
        let ranges = batch_headers_into_ranges(&new_headers);

        if ranges.is_empty() {
            return;
        }

        trace!(
            range_count = ranges.len(),
            header_count = new_headers.len(),
            "dispatching fetch ranges"
        );

        // Build peer fetch states for the decision engine.
        let peer_states: Vec<PeerFetchState> = self
            .fetch_senders
            .keys()
            .map(|addr| PeerFetchState {
                addr: *addr,
                latency_ms: 100.0,
                in_flight: 0,
                tip_slot: 0,
            })
            .collect();

        // Add all ranges to the decision engine and dispatch.
        for range in &ranges {
            self.decision_engine
                .add_range(range.from.clone(), range.to.clone());
        }

        // Select peers and dispatch ranges.
        let mut dispatched: HashMap<SocketAddr, Vec<FetchRange>> = HashMap::new();

        while let Some((peer, range)) = self.decision_engine.select_peer(&peer_states) {
            dispatched.entry(peer).or_default().push(range);
        }

        // Send fetch requests to each peer's worker.
        //
        // In-flight tracking is only updated AFTER a successful dispatch.
        // If the peer's channel is full, the blocks are NOT marked as
        // in-flight, allowing them to be dispatched to a different peer
        // on the next decision tick.  Without this, a full channel would
        // lock the blocks in in-flight for 120 seconds with no actual
        // download happening.
        let now = Instant::now();
        for (addr, peer_ranges) in dispatched {
            if let Some(sender) = self.fetch_senders.get(&addr) {
                let range_count = peer_ranges.len();
                match sender.try_send(peer_ranges.clone()) {
                    Ok(()) => {
                        debug!(%addr, range_count, "dispatched fetch ranges to peer");
                        // Mark ALL blocks in the successfully dispatched ranges
                        // as in-flight so they aren't re-dispatched on the next
                        // decision tick.
                        for range in &peer_ranges {
                            let from_slot = match &range.from {
                                Point::Specific(s, _) => *s,
                                Point::Origin => 0,
                            };
                            let to_slot = match &range.to {
                                Point::Specific(s, _) => *s,
                                Point::Origin => 0,
                            };
                            for (_, header) in &new_headers {
                                if header.slot >= from_slot && header.slot <= to_slot {
                                    self.in_flight.insert(header.hash, (addr, now));
                                }
                            }
                        }
                    }
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        // Channel full — do NOT mark as in-flight.  The blocks
                        // will be re-dispatched to a different peer on the next
                        // decision tick.
                        debug!(
                            %addr,
                            range_count,
                            "peer fetch channel full, will retry on another peer"
                        );
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        warn!(%addr, "peer fetch channel closed, deregistering");
                        self.fetch_senders.remove(&addr);
                    }
                }
            }
        }
    }

    /// Remove a block hash from the in-flight map.
    ///
    /// Called when a block is successfully received or a fetch fails.
    pub fn mark_received(&mut self, hash: &[u8; 32]) {
        self.in_flight.remove(hash);
    }
}

/// Batch sorted pending headers into fetch ranges.
///
/// Groups headers into `FetchRange` entries of up to `MAX_BATCH_SIZE` headers
/// each.  The BlockFetch `MsgRequestRange(from, to)` protocol uses Points
/// (slot + hash) to define the range, and the server walks the chain between
/// those points — slot gaps between blocks are perfectly normal in Cardano
/// (Praos slots are sparse) and do NOT require splitting into separate ranges.
///
/// The input must be sorted by slot (ascending).
fn batch_headers_into_ranges(headers: &[(SocketAddr, PendingHeader)]) -> Vec<FetchRange> {
    if headers.is_empty() {
        return Vec::new();
    }

    let mut ranges = Vec::new();
    let mut batch_start = &headers[0].1;
    let mut batch_end = &headers[0].1;
    let mut batch_count = 1usize;

    for (_, header) in headers.iter().skip(1) {
        if batch_count < MAX_BATCH_SIZE {
            batch_end = header;
            batch_count += 1;
        } else {
            // Flush the current batch — hit size limit.
            ranges.push(FetchRange {
                from: Point::Specific(batch_start.slot, batch_start.hash),
                to: Point::Specific(batch_end.slot, batch_end.hash),
            });
            batch_start = header;
            batch_end = header;
            batch_count = 1;
        }
    }

    // Flush the final batch.
    ranges.push(FetchRange {
        from: Point::Specific(batch_start.slot, batch_start.hash),
        to: Point::Specific(batch_end.slot, batch_end.hash),
    });

    ranges
}

/// Per-peer BlockFetch worker.
///
/// Receives fetch ranges from the decision task via `request_rx`, downloads
/// blocks via `BlockFetchClient`, and sends decoded blocks to the main run loop
/// via `fetched_blocks_tx`.
///
/// Runs as a dedicated tokio task for each hot peer. Exits when:
/// - The request channel is closed (peer deregistered from decision task).
/// - A protocol error occurs (bearer died).
/// - The cancellation token is triggered (node shutdown).
///
/// # Arguments
///
/// * `channel` — Mux channel for the BlockFetch mini-protocol (protocol ID 3).
/// * `request_rx` — Receiver for fetch range requests from the decision task.
/// * `fetched_blocks_tx` — Sender for decoded blocks to the main run loop.
/// * `peer_addr` — Remote peer socket address (for logging and `FetchedBlock.peer`).
/// * `byron_epoch_length` — Byron epoch length for block deserialization.
/// * `cancel` — Cancellation token for graceful shutdown.
pub async fn blockfetch_worker(
    mut channel: MuxChannel,
    mut request_rx: mpsc::Receiver<Vec<FetchRange>>,
    fetched_blocks_tx: mpsc::Sender<FetchedBlock>,
    peer_addr: SocketAddr,
    byron_epoch_length: u64,
    cancel: CancellationToken,
) {
    info!(%peer_addr, "blockfetch worker started");

    loop {
        tokio::select! {
            biased;

            _ = cancel.cancelled() => {
                debug!(%peer_addr, "blockfetch worker shutting down");
                break;
            }

            request = request_rx.recv() => {
                let ranges = match request {
                    Some(r) => r,
                    None => {
                        debug!(%peer_addr, "blockfetch request channel closed");
                        break;
                    }
                };

                for range in ranges {
                    let from = range.from.clone();
                    let to = range.to.clone();

                    // Accumulate decoded blocks from the callback to send after
                    // the fetch completes (callback is FnMut, not async).
                    let mut decoded_blocks: Vec<torsten_primitives::block::Block> = Vec::new();
                    let epoch_len = byron_epoch_length;

                    // Wrap the fetch in a timeout to detect dead connections.
                    // If the peer's TCP connection is half-open, recv() blocks
                    // forever; this timeout ensures the worker exits and the
                    // connection lifecycle manager can clean up.
                    let fetch_future = BlockFetchClient::fetch_range(
                        &mut channel,
                        from,
                        to,
                        |block_cbor| {
                            // Decode the block from raw CBOR.
                            match torsten_serialization::multi_era::decode_block_minimal_with_byron_epoch_length(
                                &block_cbor,
                                epoch_len,
                            ) {
                                Ok(block) => {
                                    decoded_blocks.push(block);
                                    Ok(())
                                }
                                Err(e) => {
                                    Err(torsten_network::error::ProtocolError::CborDecode {
                                        protocol: "BlockFetch",
                                        reason: format!("block decode failed: {e}"),
                                    })
                                }
                            }
                        },
                    );

                    let result = match tokio::time::timeout(IN_FLIGHT_TIMEOUT, fetch_future).await {
                        Ok(inner) => inner,
                        Err(_elapsed) => {
                            error!(
                                %peer_addr,
                                timeout_secs = IN_FLIGHT_TIMEOUT.as_secs(),
                                "blockfetch range timed out, exiting worker"
                            );
                            return;
                        }
                    };

                    match result {
                        Ok(count) => {
                            debug!(
                                %peer_addr,
                                block_count = count,
                                "blockfetch range complete"
                            );

                            // Send each decoded block to the run loop.
                            for block in decoded_blocks {
                                let fetched = FetchedBlock {
                                    peer: peer_addr,
                                    tip_slot: block.slot().0,
                                    tip_hash: block.hash().0,
                                    tip_block_number: block.block_number().0,
                                    block,
                                };

                                if fetched_blocks_tx.send(fetched).await.is_err() {
                                    warn!(
                                        %peer_addr,
                                        "fetched_blocks channel closed, exiting worker"
                                    );
                                    return;
                                }
                            }
                        }
                        Err(e) => {
                            error!(
                                %peer_addr,
                                error = %e,
                                "blockfetch protocol error, exiting worker"
                            );
                            // Bearer died or protocol violation — exit the worker.
                            // The connection lifecycle manager will detect the dead
                            // connection and clean up.
                            return;
                        }
                    }
                }
            }
        }
    }

    // Send MsgClientDone to cleanly terminate the BlockFetch protocol.
    if let Err(e) = BlockFetchClient::done(&mut channel).await {
        debug!(
            %peer_addr,
            error = %e,
            "failed to send MsgClientDone (bearer may already be closed)"
        );
    }

    info!(%peer_addr, "blockfetch worker stopped");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_addr(port: u16) -> SocketAddr {
        use std::net::{IpAddr, Ipv4Addr};
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), port)
    }

    fn test_header(slot: u64) -> PendingHeader {
        let mut hash = [0u8; 32];
        hash[0..8].copy_from_slice(&slot.to_be_bytes());
        PendingHeader {
            slot,
            hash,
            header_cbor: vec![0x82, 0x01],
        }
    }

    #[test]
    fn batch_headers_single_range() {
        let addr = test_addr(3001);
        let headers: Vec<(SocketAddr, PendingHeader)> =
            (10..=15).map(|slot| (addr, test_header(slot))).collect();

        let ranges = batch_headers_into_ranges(&headers);
        assert_eq!(ranges.len(), 1);

        // Verify the range covers slots 10-15.
        match (&ranges[0].from, &ranges[0].to) {
            (Point::Specific(from_slot, _), Point::Specific(to_slot, _)) => {
                assert_eq!(*from_slot, 10);
                assert_eq!(*to_slot, 15);
            }
            _ => panic!("expected Specific points"),
        }
    }

    #[test]
    fn batch_headers_gap_does_not_split_ranges() {
        // Slot gaps are normal in Cardano (Praos slots are sparse).
        // BlockFetch uses Points for range boundaries and walks the chain,
        // so gaps do NOT require splitting into separate ranges.
        let addr = test_addr(3001);
        let headers: Vec<(SocketAddr, PendingHeader)> = vec![
            (addr, test_header(10)),
            (addr, test_header(11)),
            // Gap: slot 12-19 missing (normal in Cardano)
            (addr, test_header(20)),
            (addr, test_header(21)),
        ];

        let ranges = batch_headers_into_ranges(&headers);
        // All four headers should be in a single range.
        assert_eq!(ranges.len(), 1);

        match (&ranges[0].from, &ranges[0].to) {
            (Point::Specific(from, _), Point::Specific(to, _)) => {
                assert_eq!(*from, 10);
                assert_eq!(*to, 21);
            }
            _ => panic!("expected Specific points"),
        }
    }

    #[test]
    fn batch_headers_empty() {
        let ranges = batch_headers_into_ranges(&[]);
        assert!(ranges.is_empty());
    }

    #[test]
    fn batch_headers_single_header() {
        let addr = test_addr(3001);
        let headers = vec![(addr, test_header(42))];
        let ranges = batch_headers_into_ranges(&headers);
        assert_eq!(ranges.len(), 1);

        match (&ranges[0].from, &ranges[0].to) {
            (Point::Specific(from, _), Point::Specific(to, _)) => {
                assert_eq!(*from, 42);
                assert_eq!(*to, 42);
            }
            _ => panic!("expected Specific points"),
        }
    }

    #[tokio::test]
    async fn task_register_deregister_peer() {
        let candidate_chains = Arc::new(RwLock::new(HashMap::new()));
        let (tx, _rx) = mpsc::channel(16);
        let cancel = CancellationToken::new();

        let mut task = BlockFetchLogicTask::new(candidate_chains, tx, 21600, cancel);

        let addr = test_addr(3001);
        let (peer_tx, _peer_rx) = mpsc::channel(16);

        // Register.
        task.register_peer(addr, peer_tx);
        assert!(task.fetch_senders.contains_key(&addr));

        // Deregister.
        task.deregister_peer(&addr);
        assert!(!task.fetch_senders.contains_key(&addr));
    }

    #[tokio::test]
    async fn task_skips_blocks_at_or_below_tip() {
        let candidate_chains = Arc::new(RwLock::new(HashMap::new()));
        let (tx, mut rx) = mpsc::channel(16);
        let cancel = CancellationToken::new();

        let mut task = BlockFetchLogicTask::new(candidate_chains.clone(), tx, 21600, cancel);

        // Set current tip to slot 100.
        task.update_tip_slot(100);

        // Register a peer with a fetch channel.
        let addr = test_addr(3001);
        let (peer_tx, mut peer_rx) = mpsc::channel(16);
        task.register_peer(addr, peer_tx);

        // Add candidate chain with headers at slots 50 (below tip) and 150 (above tip).
        {
            let mut chains = candidate_chains.write().await;
            chains.insert(
                addr,
                CandidateChainState {
                    tip_slot: 200,
                    tip_hash: [0xAA; 32],
                    tip_block_number: 200,
                    pending_headers: vec![test_header(50), test_header(150)],
                },
            );
        }

        // Run one decision iteration.
        task.evaluate_and_fetch().await;

        // The peer should receive a fetch request for slot 150 only.
        match peer_rx.try_recv() {
            Ok(ranges) => {
                assert_eq!(ranges.len(), 1);
                match &ranges[0].from {
                    Point::Specific(slot, _) => assert_eq!(*slot, 150),
                    _ => panic!("expected Specific point"),
                }
            }
            Err(_) => panic!("expected fetch request"),
        }

        // No blocks should be sent to the run loop (worker isn't running).
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn task_no_dispatch_without_peers() {
        let candidate_chains = Arc::new(RwLock::new(HashMap::new()));
        let (tx, _rx) = mpsc::channel(16);
        let cancel = CancellationToken::new();

        let mut task = BlockFetchLogicTask::new(candidate_chains.clone(), tx, 21600, cancel);

        // Add headers but no peers registered.
        {
            let mut chains = candidate_chains.write().await;
            chains.insert(
                test_addr(3001),
                CandidateChainState {
                    tip_slot: 100,
                    tip_hash: [0xBB; 32],
                    tip_block_number: 100,
                    pending_headers: vec![test_header(50)],
                },
            );
        }

        // Should complete without panicking.
        task.evaluate_and_fetch().await;
    }

    #[tokio::test]
    async fn task_marks_in_flight() {
        let candidate_chains = Arc::new(RwLock::new(HashMap::new()));
        let (tx, _rx) = mpsc::channel(16);
        let cancel = CancellationToken::new();

        let mut task = BlockFetchLogicTask::new(candidate_chains.clone(), tx, 21600, cancel);

        let addr = test_addr(3001);
        let (peer_tx, _peer_rx) = mpsc::channel(16);
        task.register_peer(addr, peer_tx);

        {
            let mut chains = candidate_chains.write().await;
            chains.insert(
                addr,
                CandidateChainState {
                    tip_slot: 200,
                    tip_hash: [0xCC; 32],
                    tip_block_number: 200,
                    pending_headers: vec![test_header(150)],
                },
            );
        }

        // First evaluation should dispatch.
        task.evaluate_and_fetch().await;
        assert!(!task.in_flight.is_empty());

        // Second evaluation should skip (already in-flight).
        let addr2 = test_addr(3002);
        let (peer_tx2, mut peer_rx2) = mpsc::channel(16);
        task.register_peer(addr2, peer_tx2);

        task.evaluate_and_fetch().await;

        // Second peer should NOT receive a request (block already in-flight).
        assert!(peer_rx2.try_recv().is_err());

        // Mark as received — should be fetchable again.
        let hash = test_header(150).hash;
        task.mark_received(&hash);
        assert!(task.in_flight.is_empty());
    }

    #[tokio::test]
    async fn task_run_cancels_cleanly() {
        let candidate_chains = Arc::new(RwLock::new(HashMap::new()));
        let (tx, _rx) = mpsc::channel(16);
        let cancel = CancellationToken::new();

        let mut task = BlockFetchLogicTask::new(candidate_chains, tx, 21600, cancel.clone());

        // Cancel immediately.
        cancel.cancel();
        // run() should return without hanging.
        task.run().await;
    }
}
