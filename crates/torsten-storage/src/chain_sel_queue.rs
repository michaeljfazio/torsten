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
//! # Current State (Phase 3 stub)
//!
//! Per the migration plan, this subsystem is introduced as new code *alongside*
//! the existing block-processing paths.  Chain selection itself (Subsystem 3)
//! will be wired in during Phase 3.  For now, `add_block_runner` writes every
//! valid, unknown block to the VolatileDB and returns [`AddBlockResult::StoredNotAdopted`].
//! The caller can query the VolatileDB tip to check for chain advancement.
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

use torsten_primitives::hash::BlockHeaderHash;
use torsten_primitives::time::{BlockNo, SlotNo};

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
    /// The block was stored and chain selection chose it as the new tip.
    ///
    /// Set by chain selection once Subsystem 3 is wired in.  In the current
    /// stub implementation this variant is **not** returned — all new blocks
    /// yield [`StoredNotAdopted`].
    AdoptedAsTip,
    /// The block was stored in the VolatileDB but a different chain remains
    /// preferred, or chain selection has not yet been wired in.
    StoredNotAdopted,
    /// The block failed validation.  The reason string is human-readable.
    Invalid(String),
    /// The block was already present in either the VolatileDB or ImmutableDB.
    AlreadyKnown,
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
/// 3. Write to VolatileDB.
/// 4. (Chain selection — wired in by Subsystem 3, not yet implemented.)
/// 5. Return `StoredNotAdopted` (until chain selection is live).
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
    {
        let mut db = chain_db.write().await;
        if let Err(e) = db.add_block(hash.to_owned(), slot, block_no, prev_hash, cbor) {
            // Storage failure: log with a warning but do NOT cache as invalid
            // — the failure is transient (e.g. I/O error), not a protocol
            // violation.  The caller will see an error-flavoured result.
            warn!(
                hash = %hash.to_hex(),
                error = %e,
                "chain_sel: failed to write block to VolatileDB"
            );
            return AddBlockResult::Invalid(format!("storage write failed: {e}"));
        }
    }

    // --- Step 4: Chain selection -------------------------------------------
    // TODO (Subsystem 3): Run chain selection here.  For now every stored
    // block is `StoredNotAdopted`; the caller can inspect `chain_db` to see
    // the current tip.

    AddBlockResult::StoredNotAdopted
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
    /// responses (pipeline depth is typically 150) while keeping backpressure
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
    use std::path::Path;
    use torsten_primitives::hash::Hash32;
    use torsten_primitives::time::{BlockNo, SlotNo};

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

        // First submission: new block → StoredNotAdopted
        let r1 = handle
            .submit_block(hash, slot, block_no, prev, cbor.clone())
            .await
            .expect("runner exited unexpectedly");
        assert_eq!(r1, AddBlockResult::StoredNotAdopted);

        // Second submission with the same hash → AlreadyKnown
        let r2 = handle
            .submit_block(hash, slot, block_no, prev, cbor.clone())
            .await
            .expect("runner exited unexpectedly");
        assert_eq!(r2, AddBlockResult::AlreadyKnown);
    }

    // -----------------------------------------------------------------------
    // 2. StoredNotAdopted: new block is stored
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_add_block_stored_not_adopted() {
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

        assert_eq!(result, AddBlockResult::StoredNotAdopted);

        // Verify the block actually landed in the VolatileDB.
        let db = chain_db.read().await;
        assert!(db.has_block(&hash), "block should be present in VolatileDB");
    }

    // -----------------------------------------------------------------------
    // 3. InvalidBlockCache: insert / lookup / TTL
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
            let mut hash_bytes = [0u8; 32];
            hash_bytes[..8].copy_from_slice(&(i as u64).to_be_bytes());
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
        let mut already_known = 0usize;

        for task in tasks {
            match task.await.unwrap() {
                AddBlockResult::StoredNotAdopted => stored += 1,
                AddBlockResult::AlreadyKnown => already_known += 1,
                other => panic!("unexpected result: {other:?}"),
            }
        }

        // All N hashes are distinct, so all must be stored exactly once.
        assert_eq!(stored, N, "all unique blocks should be stored");
        assert_eq!(already_known, 0, "no duplicates submitted");

        // Verify VolatileDB contains exactly N blocks.
        let db = chain_db.read().await;
        assert_eq!(
            db.volatile_block_count(),
            N,
            "VolatileDB should contain {N} blocks"
        );
    }
}
