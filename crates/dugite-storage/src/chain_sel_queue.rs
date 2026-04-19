//! ChainSelQueue — sequential block processing queue, matching Haskell's
//! `addBlockAsync` / `addBlockRunner` pattern from `ChainSel.hs`.
//!
//! All blocks (whether received from peers or forged locally) enter the node
//! via a single bounded MPSC channel and are processed **one at a time** by
//! the [`add_block_runner`] task.  The sequential discipline means:
//!
//! * No concurrency hazard between chain selection and storage writes.
//! * Invalid-block decisions are visible to every subsequent block immediately.
//! * Fork tracking is deterministic and audit-able.
//!
//! # Current State
//!
//! `add_block_runner` writes every valid, unknown block to the VolatileDB,
//! runs chain selection, and returns [`AddBlockResult::AddedAsTip`] when the
//! block extended the selected chain or [`AddBlockResult::StoredAsFork`] when
//! it was stored as a fork block.  The caller no longer needs a post-hoc tip
//! re-lookup to distinguish the two cases.
//!
//! # Haskell reference
//!
//! `ouroboros-consensus ChainDB/Impl/ChainSel.hs` — `addBlockAsync`,
//! `addBlockRunner`, `chainSelectionForBlock`.
//!
//! # Invariants
//!
//! * Only one outstanding `add_block_runner` task must run per `ChainSelQueue`.
//! * Blocks are stored to VolatileDB **before** any chain-selection logic runs.
//! * Once a block is in the invalid-block cache, it can never become valid.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, oneshot, RwLock};
use tracing::{debug, trace, warn};

use dugite_primitives::hash::BlockHeaderHash;
use dugite_primitives::time::{BlockNo, SlotNo};

use crate::chain_db::ChainDB;

// ---------------------------------------------------------------------------
// Public message types
// ---------------------------------------------------------------------------

/// Message sent to the chain-selection background task.
///
/// Currently there is only one variant; future work may add `Shutdown`,
/// `Flush`, or priority hint messages.
pub enum ChainSelMessage {
    /// Request to add a block to the chain.
    ///
    /// The block is identified by its header hash plus enough metadata to
    /// write it to storage without re-parsing the CBOR.  The `result_tx`
    /// oneshot is fulfilled when the runner finishes processing the block.
    AddBlock {
        /// Blake2b-256 hash of the block header.
        hash: BlockHeaderHash,
        /// Absolute slot number of the block.
        slot: SlotNo,
        /// Sequential block number (height).
        block_no: BlockNo,
        /// Hash of the predecessor block (links the chain).
        prev_hash: BlockHeaderHash,
        /// Raw CBOR bytes of the complete block.
        cbor: Vec<u8>,
        /// Fulfillment channel for the processing result.
        result_tx: oneshot::Sender<AddBlockResult>,
    },
}

/// Result returned to the caller after `AddBlock` is processed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddBlockResult {
    /// The block was stored and is the new selected-chain tip.
    ///
    /// The caller's submitted block IS the new tip iff `tip_hash` equals
    /// the hash they submitted. If the block was stored but another block
    /// already extended the tip in a race, this variant is NOT returned —
    /// see `StoredAsFork`.
    ///
    /// Mirrors Haskell's `SuccesfullyAddedBlock (Point blk)` in
    /// `Storage/ChainDB/API.hs` — the new tip point is always carried.
    AddedAsTip {
        tip_hash: BlockHeaderHash,
        tip_slot: SlotNo,
        tip_block_no: BlockNo,
    },
    /// The block was stored in the VolatileDB but is NOT on the selected
    /// chain (a fork block with reachable ancestry but not winning chain
    /// selection).
    StoredAsFork,
    /// The block failed validation. The reason string is human-readable.
    Invalid(String),
    /// The block was already present in either the VolatileDB or ImmutableDB.
    AlreadyKnown,
    /// Chain selection switched to a strictly-longer competing fork. The
    /// VolatileDB has already updated `selected_chain`. The caller must
    /// rollback the ledger to `intersection_hash`/`intersection_slot`.
    ///
    /// Matches Haskell `ChainDiff` (Paths.hs:~55).
    TriggeredFork {
        /// Common ancestor of the old and new chains (the fork point).
        intersection_hash: BlockHeaderHash,
        /// Slot of the intersection block, pre-resolved by VolatileDB so the
        /// caller can build a rollback `Point` without a second lookup.
        intersection_slot: SlotNo,
        /// Hashes of blocks on the old chain to roll back, newest-first.
        rollback: Vec<BlockHeaderHash>,
        /// Hashes of blocks on the new chain to apply, oldest-first.
        apply: Vec<BlockHeaderHash>,
    },
}

// ---------------------------------------------------------------------------
// Invalid-block cache
// ---------------------------------------------------------------------------

/// An entry in the invalid-block cache.
struct InvalidEntry {
    /// Human-readable reason the block was rejected.
    reason: String,
    /// Monotonic instant at which this entry was inserted.
    inserted_at: Instant,
}

/// Bounded cache of recently-rejected block hashes, with TTL expiry.
///
/// Matches Haskell's `invalidBlocks :: STM.TVar (Set (RealPoint blk))` field
/// in `ChainDbEnv`.  The cache is consulted by `add_block_runner` before
/// writing any block to storage; if the block is already known-invalid the
/// runner immediately returns `Invalid` without re-validating.
///
/// The cache is bounded to [`InvalidBlockCache::MAX_ENTRIES`] entries.  When
/// the cache is full, the oldest entry is evicted before inserting the new one
/// (FIFO eviction, not LRU, to match the simplicity of the Haskell implementation).
///
/// TTL expiry is lazy: entries are not proactively removed, but any lookup
/// that finds a stale entry (older than `ttl`) treats it as absent and
/// removes it.
pub struct InvalidBlockCache {
    /// Map from block hash to invalidation reason and insertion instant.
    entries: HashMap<BlockHeaderHash, InvalidEntry>,
    /// Time-to-live for each cache entry.
    ttl: Duration,
    /// Insertion-order queue for FIFO eviction (oldest first).
    order: std::collections::VecDeque<BlockHeaderHash>,
}

impl InvalidBlockCache {
    /// Maximum number of entries retained without eviction.
    ///
    /// Matches a reasonable upper bound for the number of distinct invalid
    /// blocks that could arrive in a TTL window.  Haskell uses an unbounded
    /// `Set` but GC handles it; we use a bounded structure to cap memory.
    pub const MAX_ENTRIES: usize = 1_024;

    /// Default TTL: 10 minutes.  After this interval, a cached entry is
    /// treated as absent and removed on next lookup.
    pub const DEFAULT_TTL: Duration = Duration::from_secs(600);

    /// Create a new cache with the default capacity and TTL.
    pub fn new() -> Self {
        Self::with_ttl(Self::DEFAULT_TTL)
    }

    /// Create a new cache with a custom TTL.  Useful in tests.
    pub fn with_ttl(ttl: Duration) -> Self {
        InvalidBlockCache {
            entries: HashMap::new(),
            ttl,
            order: std::collections::VecDeque::new(),
        }
    }

    /// Insert a block hash into the cache with the given rejection reason.
    ///
    /// If the cache has reached [`MAX_ENTRIES`], the oldest entry is evicted
    /// first.  If `hash` is already present its entry is updated in-place
    /// (TTL reset, reason updated) without affecting the eviction queue order.
    pub fn insert(&mut self, hash: BlockHeaderHash, reason: String) {
        if self.entries.contains_key(&hash) {
            // Refresh the existing entry in-place; no queue change needed.
            if let Some(entry) = self.entries.get_mut(&hash) {
                entry.reason = reason;
                entry.inserted_at = Instant::now();
            }
            return;
        }

        // Evict oldest entry if at capacity.
        if self.entries.len() >= Self::MAX_ENTRIES {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }

        self.entries.insert(
            hash,
            InvalidEntry {
                reason,
                inserted_at: Instant::now(),
            },
        );
        self.order.push_back(hash);
    }

    /// Look up a block hash in the cache.
    ///
    /// Returns `Some(reason)` if the block is known-invalid and its cache
    /// entry has not expired.  Expired entries are lazily removed.
    ///
    /// Returns `None` if the block is unknown or its entry has expired.
    pub fn get(&mut self, hash: &BlockHeaderHash) -> Option<&str> {
        if let Some(entry) = self.entries.get(hash) {
            if entry.inserted_at.elapsed() < self.ttl {
                // Entry is still valid — return a reference to the reason.
                // Safety: re-borrow through the map for lifetime correctness.
                return self.entries.get(hash).map(|e| e.reason.as_str());
            }
            // Entry has expired — remove it lazily.
            self.entries.remove(hash);
            // Also remove from the order queue (O(n) but rare in practice).
            self.order.retain(|h| h != hash);
        }
        None
    }

    /// Number of live (non-expired) entries in the cache.
    ///
    /// This is an *approximate* count because expiry is lazy — expired entries
    /// are only removed on [`get`] or when a new insert triggers eviction.
    /// Use this for monitoring/debugging, not for correctness decisions.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` if the cache contains no live entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for InvalidBlockCache {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// add_block_runner
// ---------------------------------------------------------------------------

/// Background task that processes blocks from the [`ChainSelMessage`] queue.
///
/// This is the Rust equivalent of Haskell's `addBlockRunner` loop.  It must
/// run as exactly **one** `tokio::spawn`-ed task per [`ChainSelHandle`].
///
/// # Processing steps for each `AddBlock` message
///
/// 1. Check VolatileDB and ImmutableDB — return `AlreadyKnown` if present.
/// 2. Check the invalid-block cache — return `Invalid` if previously rejected.
/// 3. Write to VolatileDB (captures `extended_tip` bool from `insert_block_internal`).
/// 4. Run chain selection — switch to any strictly-longer fork found in VolatileDB.
/// 5. Return `AddedAsTip` if the block extended the selected chain, else `StoredAsFork`.
///
/// # Arguments
///
/// * `rx` — receiving end of the MPSC queue.
/// * `chain_db` — shared ChainDB protected by a read-write lock.
/// * `invalid_cache` — shared invalid-block cache (also protected by a lock
///   so the handle can inspect it independently if needed).
pub async fn add_block_runner(
    mut rx: mpsc::Receiver<ChainSelMessage>,
    chain_db: Arc<RwLock<ChainDB>>,
    invalid_cache: Arc<RwLock<InvalidBlockCache>>,
) {
    debug!("add_block_runner: started");

    while let Some(msg) = rx.recv().await {
        match msg {
            ChainSelMessage::AddBlock {
                hash,
                slot,
                block_no,
                prev_hash,
                cbor,
                result_tx,
            } => {
                let result = process_add_block(
                    &hash,
                    slot,
                    block_no,
                    prev_hash,
                    cbor,
                    &chain_db,
                    &invalid_cache,
                )
                .await;

                trace!(
                    hash = %hash.to_hex(),
                    slot = slot.0,
                    block_no = block_no.0,
                    result = ?result,
                    "add_block_runner: processed block"
                );

                // A send failure means the caller dropped their receiver —
                // this is not an error; log at trace and continue.
                if result_tx.send(result).is_err() {
                    trace!(hash = %hash.to_hex(), "add_block_runner: result receiver dropped");
                }
            }
        }
    }

    debug!("add_block_runner: channel closed, exiting");
}

/// Core processing logic for a single `AddBlock` message.
///
/// Extracted from the runner loop to make unit testing straightforward.
async fn process_add_block(
    hash: &BlockHeaderHash,
    slot: SlotNo,
    block_no: BlockNo,
    prev_hash: BlockHeaderHash,
    cbor: Vec<u8>,
    chain_db: &Arc<RwLock<ChainDB>>,
    invalid_cache: &Arc<RwLock<InvalidBlockCache>>,
) -> AddBlockResult {
    // --- Step 1: Duplicate check (VolatileDB + ImmutableDB) ----------------
    {
        // Acquire a read lock — no writes needed just for the duplicate check.
        let db = chain_db.read().await;
        if db.has_block(hash) {
            trace!(hash = %hash.to_hex(), "chain_sel: block already known");
            return AddBlockResult::AlreadyKnown;
        }
    }

    // --- Step 2: Invalid-block cache check ---------------------------------
    {
        let mut cache = invalid_cache.write().await;
        if let Some(reason) = cache.get(hash) {
            debug!(
                hash = %hash.to_hex(),
                reason,
                "chain_sel: block is in invalid cache"
            );
            return AddBlockResult::Invalid(reason.to_owned());
        }
    }

    // --- Step 3: Write to VolatileDB ---------------------------------------
    let extended_tip;
    {
        let mut db = chain_db.write().await;
        match db.add_block(hash.to_owned(), slot, block_no, prev_hash, cbor) {
            Ok(did_extend) => {
                extended_tip = did_extend;
            }
            Err(e) => {
                warn!(
                    hash = %hash.to_hex(),
                    error = %e,
                    "chain_sel: failed to write block to VolatileDB"
                );
                return AddBlockResult::Invalid(format!("storage write failed: {e}"));
            }
        }
    }

    // --- Step 4: Chain selection (Haskell `chainSelectionForBlock`) ---------
    //
    // Query the VolatileDB for all competing fork tips — leaf blocks that are
    // NOT on the currently-selected chain.  If any has a strictly-higher block
    // number than the current selected-chain tip, we switch to that fork.
    //
    // This matches Haskell's `constructPreferableCandidates` + `switchFork`
    // in `ChainSel.hs`:
    //   1. `maximalCandidates` → our `get_all_fork_tips()`
    //   2. `preferAnchoredCandidate` (longest-chain rule) → block_no comparison
    //   3. `switchFork` → `switch_to_fork()`
    //
    // The "strictly preferred" invariant (block_no MUST be strictly greater)
    // matches Haskell's `preferCandidate` which requires the candidate to be
    // "at least as long and at least as heavy" — we use strict length (block_no)
    // for correctness in the simple case; tiebreaking via VRF / density will
    // be added when headers are available in this path.
    //
    // NOTE: This check is performed AFTER writing to VolatileDB so the new
    // block is visible when computing fork tips.
    {
        let mut db = chain_db.write().await;

        // Current selected-chain tip block_no. If 0 / unknown there is
        // nothing to compare against and no fork is possible yet.
        let current_tip_block_no: u64 = db
            .get_tip_info()
            .map(|(_slot, _hash, bn)| bn.0)
            .unwrap_or(0);

        // Enumerate all competing fork tips.
        let fork_tips = db.get_all_fork_tips();

        // Find the fork tip (if any) that is STRICTLY longer than the current
        // selected-chain tip.  If multiple forks qualify, pick the one with the
        // highest block_no (i.e. the longest chain).
        let best_fork = fork_tips
            .into_iter()
            .filter(|(_h, bn, _slot)| bn.0 > current_tip_block_no)
            .max_by_key(|(_h, bn, _slot)| bn.0);

        if let Some((fork_hash, fork_bn, fork_slot)) = best_fork {
            // A strictly-preferred fork exists — switch to it.
            debug!(
                fork_hash = %fork_hash.to_hex(),
                fork_block_no = fork_bn.0,
                fork_slot = fork_slot.0,
                current_tip_block_no,
                "chain_sel: switching to longer fork"
            );

            if let Some(plan) = db.switch_to_fork(&fork_hash) {
                return AddBlockResult::TriggeredFork {
                    intersection_hash: plan.intersection,
                    intersection_slot: SlotNo(plan.intersection_slot),
                    rollback: plan.rollback,
                    apply: plan.apply,
                };
            }
            // `switch_to_fork` returned None: the intersection is not
            // reachable within the VolatileDB window.  Per Haskell
            // `isReachable = Nothing` (`Paths.hs`), this is the
            // `StoreButDontChange` case — the block stays in VolatileDB but
            // no chain selection occurs.  We fall through so the caller does
            // NOT attempt a ledger rollback; the block will re-enter chain
            // selection later if its ancestry becomes complete.
            debug!(
                fork_hash = %fork_hash.to_hex(),
                "chain_sel: fork unreachable — StoreButDontChange"
            );
        }
    }

    // If the block extended our selected_chain, surface the new tip.
    if extended_tip {
        let db = chain_db.read().await;
        if let Some((tip_slot, tip_hash, tip_block_no)) = db.get_tip_info() {
            return AddBlockResult::AddedAsTip {
                tip_hash,
                tip_slot,
                tip_block_no,
            };
        }
    }

    AddBlockResult::StoredAsFork
}

// ---------------------------------------------------------------------------
// ChainSelHandle
// ---------------------------------------------------------------------------

/// Client-side handle for submitting blocks to the chain-selection queue.
///
/// Cheap to clone — it is just an `mpsc::Sender` plus a reference to the
/// shared invalid-block cache.  Each handle can be given to a different
/// subsystem (sync pipeline, block forger, test harness) independently.
///
/// # Example
///
/// ```rust,ignore
/// let (handle, runner_future) = ChainSelHandle::new(chain_db.clone());
/// tokio::spawn(runner_future);
///
/// let result = handle
///     .submit_block(hash, slot, block_no, prev_hash, cbor)
///     .await
///     .unwrap();
/// ```
#[derive(Clone)]
pub struct ChainSelHandle {
    tx: mpsc::Sender<ChainSelMessage>,
    /// Shared invalid-block cache.  Exposed so callers can pre-seed the cache
    /// (e.g. from a persisted blacklist) or inspect it for monitoring.
    pub invalid_cache: Arc<RwLock<InvalidBlockCache>>,
}

impl ChainSelHandle {
    /// Default MPSC channel capacity.
    ///
    /// Chosen to be large enough to absorb a burst of pipelined block-fetch
    /// responses (pipeline depth is typically 300) while keeping backpressure
    /// intact.  When the queue fills the sender will apply natural backpressure
    /// via `await` in [`submit_block`].
    pub const DEFAULT_CHANNEL_CAPACITY: usize = 512;

    /// Create a new `ChainSelHandle` and return the associated runner future.
    ///
    /// Callers MUST spawn the returned future (via `tokio::spawn`) before
    /// calling `submit_block`.
    ///
    /// ```rust,ignore
    /// let (handle, runner) = ChainSelHandle::new(chain_db);
    /// tokio::spawn(runner);
    /// ```
    pub fn new(chain_db: Arc<RwLock<ChainDB>>) -> (Self, impl std::future::Future<Output = ()>) {
        Self::with_capacity(chain_db, Self::DEFAULT_CHANNEL_CAPACITY)
    }

    /// Create with a custom channel capacity.  Primarily useful in tests.
    pub fn with_capacity(
        chain_db: Arc<RwLock<ChainDB>>,
        capacity: usize,
    ) -> (Self, impl std::future::Future<Output = ()>) {
        let invalid_cache = Arc::new(RwLock::new(InvalidBlockCache::new()));
        let (tx, rx) = mpsc::channel(capacity);

        let runner = add_block_runner(rx, chain_db, Arc::clone(&invalid_cache));

        let handle = ChainSelHandle { tx, invalid_cache };

        (handle, runner)
    }

    /// Submit a block for chain-selection processing.
    ///
    /// Awaits backpressure if the queue is full.  Returns `None` if the
    /// background runner has exited (i.e. the channel is closed).
    ///
    /// # Arguments
    ///
    /// * `hash` — Blake2b-256 block header hash.
    /// * `slot` — Absolute slot number.
    /// * `block_no` — Block height.
    /// * `prev_hash` — Hash of the parent block.
    /// * `cbor` — Raw CBOR bytes of the complete block.
    pub async fn submit_block(
        &self,
        hash: BlockHeaderHash,
        slot: SlotNo,
        block_no: BlockNo,
        prev_hash: BlockHeaderHash,
        cbor: Vec<u8>,
    ) -> Option<AddBlockResult> {
        let (result_tx, result_rx) = oneshot::channel();

        self.tx
            .send(ChainSelMessage::AddBlock {
                hash,
                slot,
                block_no,
                prev_hash,
                cbor,
                result_tx,
            })
            .await
            .ok()?;

        result_rx.await.ok()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use dugite_primitives::hash::Hash32;
    use dugite_primitives::time::{BlockNo, SlotNo};
    use std::path::Path;

    // -----------------------------------------------------------------------
    // Helper: open a ChainDB in a temp dir
    // -----------------------------------------------------------------------

    fn make_chain_db(dir: &Path) -> Arc<RwLock<ChainDB>> {
        let db = ChainDB::open(dir).expect("failed to open test ChainDB");
        Arc::new(RwLock::new(db))
    }

    /// Minimal synthetic CBOR for tests: just the hash bytes, enough to be
    /// non-empty and distinguishable per block.
    fn fake_cbor(hash: &Hash32) -> Vec<u8> {
        hash.as_bytes().to_vec()
    }

    // -----------------------------------------------------------------------
    // 1. AlreadyKnown: duplicate block
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_add_block_already_known() {
        let dir = tempfile::tempdir().unwrap();
        let chain_db = make_chain_db(dir.path());

        let (handle, runner) = ChainSelHandle::new(Arc::clone(&chain_db));
        let _runner_task = tokio::spawn(runner);

        let hash = Hash32::from_bytes([0x01; 32]);
        let slot = SlotNo(1000);
        let block_no = BlockNo(1);
        let prev = Hash32::ZERO;
        let cbor = fake_cbor(&hash);

        // First submission: new block extends chain → AddedAsTip
        let r1 = handle
            .submit_block(hash, slot, block_no, prev, cbor.clone())
            .await
            .expect("runner exited unexpectedly");
        assert!(
            matches!(r1, AddBlockResult::AddedAsTip { .. }),
            "first submission of a chain-extending block must return AddedAsTip, got {r1:?}"
        );

        // Second submission with the same hash → AlreadyKnown
        let r2 = handle
            .submit_block(hash, slot, block_no, prev, cbor.clone())
            .await
            .expect("runner exited unexpectedly");
        assert_eq!(r2, AddBlockResult::AlreadyKnown);
    }

    // -----------------------------------------------------------------------
    // 2. AddedAsTip: new block extends selected chain
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_add_block_added_as_tip() {
        let dir = tempfile::tempdir().unwrap();
        let chain_db = make_chain_db(dir.path());

        let (handle, runner) = ChainSelHandle::new(Arc::clone(&chain_db));
        let _runner_task = tokio::spawn(runner);

        let hash = Hash32::from_bytes([0xAB; 32]);
        let slot = SlotNo(42);
        let block_no = BlockNo(1);
        let prev = Hash32::ZERO;
        let cbor = fake_cbor(&hash);

        let result = handle
            .submit_block(hash, slot, block_no, prev, cbor)
            .await
            .expect("runner exited unexpectedly");

        match result {
            AddBlockResult::AddedAsTip {
                tip_hash,
                tip_slot,
                tip_block_no,
            } => {
                assert_eq!(
                    tip_hash, hash,
                    "tip_hash must equal the submitted block hash"
                );
                assert_eq!(tip_slot, slot);
                assert_eq!(tip_block_no, block_no);
            }
            other => panic!("expected AddedAsTip, got {other:?}"),
        }

        // Verify the block actually landed in the VolatileDB.
        let db = chain_db.read().await;
        assert!(db.has_block(&hash), "block should be present in VolatileDB");
    }

    // -----------------------------------------------------------------------
    // 2b. Forge-path invariant: extending block becomes selected_chain tip
    // -----------------------------------------------------------------------
    //
    // Positive case for #439 follow-up: when the submitted block's `prev_hash`
    // matches the current selected_chain tip, the block MUST become the new tip
    // and `AddedAsTip` must be returned (no separate re-lookup needed).

    #[tokio::test]
    async fn test_forge_path_extending_block_becomes_tip() {
        let dir = tempfile::tempdir().unwrap();
        let chain_db = make_chain_db(dir.path());

        let (handle, runner) = ChainSelHandle::new(Arc::clone(&chain_db));
        let _runner_task = tokio::spawn(runner);

        // Genesis block.
        let genesis = Hash32::from_bytes([0x01; 32]);
        handle
            .submit_block(
                genesis,
                SlotNo(1),
                BlockNo(0),
                Hash32::ZERO,
                fake_cbor(&genesis),
            )
            .await
            .unwrap();

        // Forged block extending genesis.
        let forged = Hash32::from_bytes([0x02; 32]);
        let result = handle
            .submit_block(forged, SlotNo(10), BlockNo(1), genesis, fake_cbor(&forged))
            .await
            .unwrap();

        match result {
            AddBlockResult::AddedAsTip {
                tip_hash,
                tip_slot,
                tip_block_no,
            } => {
                assert_eq!(
                    tip_hash, forged,
                    "AddedAsTip.tip_hash must equal the forged block hash"
                );
                assert_eq!(tip_slot, SlotNo(10));
                assert_eq!(tip_block_no, BlockNo(1));
            }
            other => panic!(
                "extending block on an unopposed chain should return AddedAsTip, got {other:?}"
            ),
        }

        // Critical invariant: VolatileDB's selected-chain tip MUST be the
        // forged block, because `insert_block_internal` advances the tip
        // whenever `prev_hash == selected_chain.last()`.
        let db = chain_db.read().await;
        let (_slot, tip_hash, tip_bn) = db
            .get_tip_info()
            .expect("tip info should exist after forge");
        assert_eq!(
            tip_hash, forged,
            "forge-path invariant: forged block MUST be selected-chain tip"
        );
        assert_eq!(tip_bn, BlockNo(1));
    }

    // -----------------------------------------------------------------------
    // 2c. Forge-path invariant: race-lost block is NOT selected_chain tip
    // -----------------------------------------------------------------------
    //
    // Models the sequence behind issue #439:
    //   1. BP starts forging against tip X at height H.
    //   2. Upstream delivers Y at H (also extending X), becoming tip.
    //   3. BP's forged block Z at height H+1 WITH prev_hash=X arrives at the
    //      queue AFTER Y — so Z's prev_hash no longer matches the
    //      selected_chain tip (which is now Y).
    //   4. `insert_block_internal` stores Z as a FORK block (not on
    //      selected_chain).  `forged_is_tip` must be false and the forge
    //      path must abort without ledger-applying or announcing Z.

    #[tokio::test]
    async fn test_forge_path_race_lost_block_is_not_tip() {
        let dir = tempfile::tempdir().unwrap();
        let chain_db = make_chain_db(dir.path());

        let (handle, runner) = ChainSelHandle::new(Arc::clone(&chain_db));
        let _runner_task = tokio::spawn(runner);

        let x = Hash32::from_bytes([0xA0; 32]);
        handle
            .submit_block(x, SlotNo(1), BlockNo(0), Hash32::ZERO, fake_cbor(&x))
            .await
            .unwrap();

        // Upstream block Y lands first — Y extends X and becomes tip.
        let y = Hash32::from_bytes([0xB0; 32]);
        handle
            .submit_block(y, SlotNo(2), BlockNo(1), x, fake_cbor(&y))
            .await
            .unwrap();

        // BP's forged block Z arrives LATE — still claims prev_hash=X but
        // selected_chain tip is now Y. Z is stored as a fork block.
        let z = Hash32::from_bytes([0xC0; 32]);
        let result = handle
            .submit_block(z, SlotNo(3), BlockNo(1), x, fake_cbor(&z))
            .await
            .unwrap();

        assert_eq!(
            result,
            AddBlockResult::StoredAsFork,
            "race-lost block must return StoredAsFork (not AddedAsTip)"
        );

        // Forge-path invariant: the forged block must NOT be selected-chain tip.
        let db = chain_db.read().await;
        let (_slot, tip_hash, _tip_bn) = db.get_tip_info().expect("tip exists");
        assert_eq!(
            tip_hash, y,
            "selected-chain tip must remain at Y (the race winner) — Z lost"
        );
        assert_ne!(
            tip_hash, z,
            "forge-path invariant: race-lost Z MUST NOT be the tip; \
             forge-path `forged_is_tip` check must detect this and abort"
        );
        assert!(db.has_block(&z), "Z must still be stored as a fork block");
    }

    // -----------------------------------------------------------------------
    // 3. Chain selection: TriggeredFork returned for longer competing fork
    // -----------------------------------------------------------------------

    /// Verify that submitting two competing forks causes chain selection to
    /// return `TriggeredFork` for the block that makes the fork strictly longer.
    ///
    /// Chain layout:
    ///
    ///   common → a2 → a3          (selected chain, block_nos 2, 3)
    ///          ↘ b2 → b3 → b4    (fork, block_nos 2, 3, 4 — strictly longer)
    ///
    /// When b4 arrives, chain selection should switch to the b-fork and return
    /// `TriggeredFork { rollback: [a3, a2], apply: [b2, b3, b4] }`.
    #[tokio::test]
    async fn test_chain_selection_switches_to_longer_fork() {
        let dir = tempfile::tempdir().unwrap();
        let chain_db = make_chain_db(dir.path());

        let (handle, runner) = ChainSelHandle::new(Arc::clone(&chain_db));
        let _runner = tokio::spawn(runner);

        // All hashes use a fixed high byte to stay far from ZERO.
        let common = Hash32::from_bytes([0xC0; 32]);
        let a2 = Hash32::from_bytes([0xA2; 32]);
        let a3 = Hash32::from_bytes([0xA3; 32]);
        let b2 = Hash32::from_bytes([0xB2; 32]);
        let b3 = Hash32::from_bytes([0xB3; 32]);
        let b4 = Hash32::from_bytes([0xB4; 32]);

        // Build main (a) chain.
        let r = handle
            .submit_block(
                common,
                SlotNo(100),
                BlockNo(1),
                Hash32::ZERO,
                fake_cbor(&common),
            )
            .await
            .unwrap();
        assert!(
            matches!(r, AddBlockResult::AddedAsTip { .. }),
            "common: {r:?}"
        );

        let r = handle
            .submit_block(a2, SlotNo(200), BlockNo(2), common, fake_cbor(&a2))
            .await
            .unwrap();
        assert!(matches!(r, AddBlockResult::AddedAsTip { .. }), "a2: {r:?}");

        let r = handle
            .submit_block(a3, SlotNo(300), BlockNo(3), a2, fake_cbor(&a3))
            .await
            .unwrap();
        assert!(matches!(r, AddBlockResult::AddedAsTip { .. }), "a3: {r:?}");

        // Build competing (b) fork starting from common.
        // b2 and b3 have the same block_nos as a2/a3 — no switch yet.
        let r = handle
            .submit_block(b2, SlotNo(200), BlockNo(2), common, fake_cbor(&b2))
            .await
            .unwrap();
        // b2 is a fork tip with block_no=2, but selected chain tip is a3 at
        // block_no=3, so b2 does NOT trigger a switch.
        assert_eq!(r, AddBlockResult::StoredAsFork, "b2: {r:?}");

        let r = handle
            .submit_block(b3, SlotNo(300), BlockNo(3), b2, fake_cbor(&b3))
            .await
            .unwrap();
        // b3 block_no=3 == current tip a3 block_no=3.
        // Strictly-greater check: 3 > 3 is false → no switch.
        assert_eq!(r, AddBlockResult::StoredAsFork, "b3: {r:?}");

        // b4 extends the fork to block_no=4, strictly longer than a3 (3).
        let r = handle
            .submit_block(b4, SlotNo(400), BlockNo(4), b3, fake_cbor(&b4))
            .await
            .unwrap();

        match r {
            AddBlockResult::TriggeredFork {
                intersection_hash: _,
                intersection_slot: _,
                rollback,
                apply,
            } => {
                // Rollback should un-apply the a-chain blocks above common.
                assert!(
                    rollback.contains(&a3) && rollback.contains(&a2),
                    "rollback should include a3 and a2, got: {rollback:?}"
                );
                // Apply should bring in the b-chain blocks.
                assert!(
                    apply.contains(&b2) && apply.contains(&b3) && apply.contains(&b4),
                    "apply should include b2, b3, b4, got: {apply:?}"
                );
                // common should NOT appear in either list.
                assert!(
                    !rollback.contains(&common) && !apply.contains(&common),
                    "intersection block should not appear in rollback/apply"
                );
            }
            other => panic!("expected TriggeredFork but got: {other:?}"),
        }

        // After the switch, the VolatileDB tip should be b4.
        let db = chain_db.read().await;
        let tip = db.get_tip_info().expect("should have a tip");
        assert_eq!(tip.2 .0, 4, "tip block_no should be 4 (b4)");
    }

    /// Verify that equal-length chains do NOT trigger a fork switch.
    ///
    /// Haskell invariant: chain selection only switches to a STRICTLY-preferred
    /// candidate (block_no > current tip). Equal block_no is not sufficient.
    #[tokio::test]
    async fn test_chain_selection_no_switch_equal_length() {
        let dir = tempfile::tempdir().unwrap();
        let chain_db = make_chain_db(dir.path());

        let (handle, runner) = ChainSelHandle::new(Arc::clone(&chain_db));
        let _runner = tokio::spawn(runner);

        let common = Hash32::from_bytes([0xC0; 32]);
        let a2 = Hash32::from_bytes([0xA2; 32]);
        let b2 = Hash32::from_bytes([0xB2; 32]);

        handle
            .submit_block(
                common,
                SlotNo(100),
                BlockNo(1),
                Hash32::ZERO,
                fake_cbor(&common),
            )
            .await
            .unwrap();
        handle
            .submit_block(a2, SlotNo(200), BlockNo(2), common, fake_cbor(&a2))
            .await
            .unwrap();

        // b2 has the same block_no as a2 — no switch should occur.
        let r = handle
            .submit_block(b2, SlotNo(200), BlockNo(2), common, fake_cbor(&b2))
            .await
            .unwrap();
        assert_eq!(
            r,
            AddBlockResult::StoredAsFork,
            "equal-length fork must not trigger a switch (b2 is a fork block)"
        );

        // Selected chain tip is still a2.
        let db = chain_db.read().await;
        let tip = db.get_tip_info().expect("should have a tip");
        assert_eq!(tip.2 .0, 2, "selected-chain tip block_no should still be 2");
    }

    // -----------------------------------------------------------------------
    // 4. InvalidBlockCache: insert / lookup / TTL
    // -----------------------------------------------------------------------

    #[test]
    fn test_invalid_block_cache_insert_and_lookup() {
        let mut cache = InvalidBlockCache::new();
        let hash = Hash32::from_bytes([0x11; 32]);

        // Initially absent.
        assert!(cache.get(&hash).is_none());

        cache.insert(hash, "bad VRF proof".to_string());

        // Now present.
        let reason = cache.get(&hash).expect("should be in cache");
        assert_eq!(reason, "bad VRF proof");
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn test_invalid_block_cache_ttl_expiry() {
        // Use a TTL short enough to expire within a test.
        let mut cache = InvalidBlockCache::with_ttl(Duration::from_millis(1));
        let hash = Hash32::from_bytes([0x22; 32]);

        cache.insert(hash, "expired entry".to_string());

        // Entry is present immediately after insertion.
        assert!(cache.get(&hash).is_some());

        // Wait for TTL to expire.
        std::thread::sleep(Duration::from_millis(10));

        // Lookup should find the entry expired and return None.
        assert!(cache.get(&hash).is_none(), "expired entry should be absent");
        // Lazy removal should have shrunk the map.
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn test_invalid_block_cache_refresh_existing() {
        let mut cache = InvalidBlockCache::new();
        let hash = Hash32::from_bytes([0x33; 32]);

        cache.insert(hash, "reason A".to_string());
        assert_eq!(cache.len(), 1);

        // Re-inserting the same hash updates reason, does not grow the cache.
        cache.insert(hash, "reason B".to_string());
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.get(&hash).unwrap(), "reason B");
    }

    #[test]
    fn test_invalid_block_cache_eviction_at_capacity() {
        let mut cache = InvalidBlockCache::new();

        // Fill to exactly MAX_ENTRIES.
        for i in 0..InvalidBlockCache::MAX_ENTRIES {
            let mut bytes = [0u8; 32];
            let idx_bytes = (i as u64).to_be_bytes();
            bytes[..8].copy_from_slice(&idx_bytes);
            cache.insert(Hash32::from_bytes(bytes), format!("reason {i}"));
        }
        assert_eq!(cache.len(), InvalidBlockCache::MAX_ENTRIES);

        // The first entry inserted (i=0).
        let mut first_bytes = [0u8; 32];
        first_bytes[..8].copy_from_slice(&0u64.to_be_bytes());
        let first_hash = Hash32::from_bytes(first_bytes);

        // Inserting one more entry should evict the oldest (i=0).
        let mut new_bytes = [0xFF; 32];
        new_bytes[0] = 0xFE; // make it unique
        cache.insert(Hash32::from_bytes(new_bytes), "new entry".to_string());

        // Cache size must stay bounded.
        assert_eq!(cache.len(), InvalidBlockCache::MAX_ENTRIES);

        // The oldest entry should be gone.
        assert!(
            cache.get(&first_hash).is_none(),
            "oldest entry should have been evicted"
        );
    }

    // -----------------------------------------------------------------------
    // 4. Invalid block cache wired into the runner
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_add_block_invalid_from_cache() {
        let dir = tempfile::tempdir().unwrap();
        let chain_db = make_chain_db(dir.path());

        let (handle, runner) = ChainSelHandle::new(Arc::clone(&chain_db));
        let _runner_task = tokio::spawn(runner);

        let hash = Hash32::from_bytes([0x99; 32]);

        // Pre-seed the invalid cache.
        {
            let mut cache = handle.invalid_cache.write().await;
            cache.insert(hash, "pre-seeded invalid".to_string());
        }

        let result = handle
            .submit_block(hash, SlotNo(5), BlockNo(1), Hash32::ZERO, fake_cbor(&hash))
            .await
            .expect("runner exited unexpectedly");

        assert_eq!(
            result,
            AddBlockResult::Invalid("pre-seeded invalid".to_string())
        );

        // Verify the block was NOT written to storage.
        let db = chain_db.read().await;
        assert!(
            !db.has_block(&hash),
            "invalid block must not reach VolatileDB"
        );
    }

    // -----------------------------------------------------------------------
    // 5. Concurrent block submission
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_concurrent_block_submission() {
        let dir = tempfile::tempdir().unwrap();
        let chain_db = make_chain_db(dir.path());

        // Use a generous channel capacity for this burst test.
        let (handle, runner) = ChainSelHandle::with_capacity(Arc::clone(&chain_db), 256);
        let _runner_task = tokio::spawn(runner);

        const N: usize = 64;
        let mut tasks = Vec::with_capacity(N);

        for i in 0..N {
            let h = handle.clone();
            // Use i+1 so that no block hash collides with Hash32::ZERO
            // (which is used as prev_hash for all blocks).  If hash == ZERO
            // == prev_hash, walk_chain_back() would loop forever.
            let mut hash_bytes = [0u8; 32];
            hash_bytes[..8].copy_from_slice(&((i as u64) + 1).to_be_bytes());
            let hash = Hash32::from_bytes(hash_bytes);
            let cbor = fake_cbor(&hash);

            tasks.push(tokio::spawn(async move {
                h.submit_block(
                    hash,
                    SlotNo(i as u64),
                    BlockNo(i as u64),
                    Hash32::ZERO,
                    cbor,
                )
                .await
                .expect("runner exited")
            }));
        }

        let mut stored = 0usize;
        let mut switched = 0usize;
        let mut already_known = 0usize;

        for task in tasks {
            match task.await.unwrap() {
                AddBlockResult::AddedAsTip { .. } | AddBlockResult::StoredAsFork => stored += 1,
                AddBlockResult::AlreadyKnown => already_known += 1,
                // Each block has a unique block_no so chain selection may
                // switch to a longer fork as blocks arrive out of order.
                AddBlockResult::TriggeredFork { .. } => switched += 1,
                other => panic!("unexpected result: {other:?}"),
            }
        }

        // All N hashes are distinct; every block must be either stored or
        // trigger a fork switch (both outcomes mean the block is in storage).
        assert_eq!(
            stored + switched,
            N,
            "all unique blocks should be stored (stored={stored}, switched={switched})"
        );
        assert_eq!(already_known, 0, "no duplicates submitted");

        // Verify VolatileDB contains exactly N blocks.
        let db = chain_db.read().await;
        assert_eq!(
            db.volatile_block_count(),
            N,
            "VolatileDB should contain {N} blocks"
        );
    }

    // -----------------------------------------------------------------------
    // 6. AddedAsTip / StoredAsFork disambiguation (TDD for #439 follow-up)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_extending_block_returns_added_as_tip() {
        let dir = tempfile::tempdir().unwrap();
        let chain_db = make_chain_db(dir.path());

        let (handle, runner) = ChainSelHandle::new(Arc::clone(&chain_db));
        let _runner_task = tokio::spawn(runner);

        let genesis = Hash32::from_bytes([0x01; 32]);
        handle
            .submit_block(
                genesis,
                SlotNo(1),
                BlockNo(0),
                Hash32::ZERO,
                fake_cbor(&genesis),
            )
            .await
            .unwrap();

        let extending = Hash32::from_bytes([0x02; 32]);
        let result = handle
            .submit_block(
                extending,
                SlotNo(10),
                BlockNo(1),
                genesis,
                fake_cbor(&extending),
            )
            .await
            .unwrap();

        match result {
            AddBlockResult::AddedAsTip {
                tip_hash,
                tip_slot,
                tip_block_no,
            } => {
                assert_eq!(tip_hash, extending);
                assert_eq!(tip_slot, SlotNo(10));
                assert_eq!(tip_block_no, BlockNo(1));
            }
            other => panic!(
                "Extending block must return AddedAsTip, got {other:?}. \
                 This disambiguates the normal forge path from StoredAsFork (race lost)."
            ),
        }
    }

    #[tokio::test]
    async fn test_race_lost_block_returns_stored_as_fork() {
        let dir = tempfile::tempdir().unwrap();
        let chain_db = make_chain_db(dir.path());

        let (handle, runner) = ChainSelHandle::new(Arc::clone(&chain_db));
        let _runner_task = tokio::spawn(runner);

        let x = Hash32::from_bytes([0xA0; 32]);
        handle
            .submit_block(x, SlotNo(1), BlockNo(0), Hash32::ZERO, fake_cbor(&x))
            .await
            .unwrap();

        let y = Hash32::from_bytes([0xB0; 32]);
        handle
            .submit_block(y, SlotNo(2), BlockNo(1), x, fake_cbor(&y))
            .await
            .unwrap();

        // Z arrives late — still claims prev_hash = x, but selected_chain tip is Y now.
        let z = Hash32::from_bytes([0xC0; 32]);
        let result = handle
            .submit_block(z, SlotNo(3), BlockNo(1), x, fake_cbor(&z))
            .await
            .unwrap();

        assert_eq!(
            result,
            AddBlockResult::StoredAsFork,
            "Race-lost block is a fork block, must return StoredAsFork (not AddedAsTip)"
        );
    }
}
