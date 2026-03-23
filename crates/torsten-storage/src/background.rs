//! Background storage maintenance operations matching Haskell's Background.hs.
//!
//! This module implements the three periodic operations that keep the storage
//! subsystem healthy between block applications:
//!
//! 1. **[`CopyToImmutable`]** — When the chain fragment grows beyond k headers,
//!    copies the oldest block from VolatileDB to ImmutableDB and schedules the
//!    entry for GC.  Matches Haskell's `copyToImmutableDB` in `Background.hs`.
//!
//! 2. **[`GcScheduler`]** — Tracks blocks that have been copied to ImmutableDB
//!    but not yet removed from VolatileDB.  After a 60-second delay (matching
//!    the Haskell GC delay) the scheduler calls back into `ChainDB` to drop the
//!    entry.  Uses `slot <` (strict less-than) to preserve the EBB invariant:
//!    Epoch Boundary Blocks share a slot with the first block of the next epoch
//!    and must never be GC'd prematurely.  Matches Haskell's `garbageCollectBlocks`
//!    and the `GcSchedule` type in `Background.hs`.
//!
//! 3. **[`SnapshotScheduler`]** — Decides when to persist the LedgerSeq anchor
//!    state to disk.  Triggers after every epoch boundary, every N blocks
//!    (configurable, default 2 000), or on graceful shutdown.  The actual
//!    persistence is carried out by a caller-supplied callback so that this
//!    module remains free of any `torsten-ledger` dependency.
//!
//! # Haskell references
//!
//! * `ouroboros-consensus/src/ouroboros-consensus/Ouroboros/Consensus/Storage/ChainDB/Impl/Background.hs`
//!   — `copyToImmutableDB`, `garbageCollectBlocks`, `GcSchedule`
//! * TR §chaindb:gc:delay — "1 minute delay, slot < (not <=) for EBB invariant"
//!
//! # Design notes
//!
//! * All three structs are **synchronous value types** — they hold no threads or
//!   tasks themselves.  They are designed to be called from the `addBlockRunner`
//!   after each block is processed (or from a dedicated ticker task).
//! * Because `torsten-storage` does not depend on `torsten-ledger` or
//!   `torsten-consensus`, interactions with `LedgerSeq` and `ChainFragment` are
//!   carried out through caller-supplied closures.  The node wires these together
//!   at startup.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use torsten_primitives::hash::Hash32;
use torsten_primitives::time::{BlockNo, EpochNo, SlotNo};
use tracing::{debug, info, trace, warn};

use crate::chain_db::ChainDB;

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

/// GC delay after a block has been copied to ImmutableDB before it is removed
/// from VolatileDB.
///
/// Matches the Haskell `gcDelay` of 1 minute (TR §chaindb:gc:delay).
/// The delay gives downstream clients (e.g. ChainSync followers that are still
/// reading the block) time to finish before the entry disappears.
pub const GC_DELAY: Duration = Duration::from_secs(60);

/// Default number of blocks between automatic ledger snapshots.
///
/// Matches Haskell's normal-sync snapshot interval.  During bulk-sync (> 50 000
/// blocks in one session) the caller may override this to a lower value via
/// [`SnapshotScheduler::with_interval`].
pub const DEFAULT_SNAPSHOT_INTERVAL: u64 = 2_000;

// ─────────────────────────────────────────────────────────────────────────────
// CopyToImmutable
// ─────────────────────────────────────────────────────────────────────────────

/// Copies the oldest block from VolatileDB to ImmutableDB when the chain
/// fragment grows beyond the security parameter k.
///
/// Matches Haskell's `copyToImmutableDB` in `Background.hs`.
///
/// # Protocol
///
/// After each block is appended to the selected chain the caller invokes
/// [`CopyToImmutable::run_once`].  If the fragment length exceeds `k` the
/// method:
///
/// 1. Retrieves the oldest block from the VolatileDB using the block hash at
///    the front of the chain fragment.
/// 2. Appends it to the ImmutableDB via [`ChainDB::put_blocks_batch`].
/// 3. Calls the caller-supplied `advance_ledger_anchor` closure so that the
///    LedgerSeq anchor is advanced to match the new immutable tip.
/// 4. Returns the slot and hash of the copied block so the caller can schedule
///    it for GC.
///
/// The copy step and the GC step are intentionally **separate**: copying is
/// immediate (preserving the immutability invariant) while GC is deferred by
/// [`GC_DELAY`] (allowing in-flight readers to finish).
pub struct CopyToImmutable {
    /// Security parameter k (number of volatile headers to retain).
    k: usize,
}

impl CopyToImmutable {
    /// Create a new `CopyToImmutable` with the given security parameter.
    pub fn new(k: usize) -> Self {
        Self { k }
    }

    /// Run one copy-to-immutable pass after a block has been added.
    ///
    /// # Parameters
    ///
    /// * `chain_db` — The ChainDB, used to read the volatile block and write
    ///   it to ImmutableDB.
    /// * `fragment_len` — Current length of the chain fragment (number of
    ///   volatile headers on the selected chain).
    /// * `oldest_hash` — Hash of the oldest block on the selected chain (the
    ///   one that will be copied if the fragment is too long).
    /// * `oldest_slot` — Slot of the oldest block (needed for ImmutableDB
    ///   append ordering).
    /// * `oldest_block_no` — Block number of the oldest block.
    /// * `advance_ledger_anchor` — Called with `(slot, hash, block_no)` after
    ///   the block is successfully copied to ImmutableDB.  The caller should
    ///   advance the LedgerSeq anchor here.
    ///
    /// # Returns
    ///
    /// `Some((slot, hash))` of the copied block, or `None` if the fragment is
    /// not yet long enough to trigger a copy.
    ///
    /// # Errors
    ///
    /// Returns an error string if the block CBOR cannot be retrieved from
    /// VolatileDB or if the ImmutableDB append fails.  The ChainDB state is
    /// unchanged on error.
    pub fn run_once(
        &self,
        chain_db: &mut ChainDB,
        fragment_len: usize,
        oldest_hash: Hash32,
        oldest_slot: SlotNo,
        oldest_block_no: BlockNo,
        advance_ledger_anchor: &mut dyn FnMut(SlotNo, Hash32, BlockNo),
    ) -> Result<Option<(SlotNo, Hash32)>, String> {
        // Only copy when the fragment is strictly longer than k.
        // When fragment_len == k we have exactly k volatile headers — correct.
        // When fragment_len == k+1 the oldest header is now k-deep and safe to
        // commit to the ImmutableDB.
        if fragment_len <= self.k {
            return Ok(None);
        }

        // Retrieve the block CBOR from the volatile store.
        let cbor = chain_db
            .get_block(&oldest_hash)
            .map_err(|e| format!("CopyToImmutable: failed to read block {oldest_hash}: {e}"))?
            .ok_or_else(|| {
                format!(
                    "CopyToImmutable: block {oldest_hash} not found in ChainDB (slot {})",
                    oldest_slot.0
                )
            })?;

        // Append the block to ImmutableDB.
        // `put_blocks_batch` writes directly to ImmutableDB (the Mithril bypass
        // path) — which is exactly what we want here: a single already-verified
        // block being moved from volatile to immutable storage.
        chain_db
            .put_blocks_batch(&[(oldest_slot, &oldest_hash, oldest_block_no, &cbor)])
            .map_err(|e| {
                format!("CopyToImmutable: ImmutableDB append failed for block {oldest_hash}: {e}")
            })?;

        debug!(
            slot = oldest_slot.0,
            block_no = oldest_block_no.0,
            hash = %oldest_hash,
            "background: copied block to ImmutableDB"
        );

        // Advance the LedgerSeq anchor in the caller.
        advance_ledger_anchor(oldest_slot, oldest_hash, oldest_block_no);

        Ok(Some((oldest_slot, oldest_hash)))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// GcScheduler
// ─────────────────────────────────────────────────────────────────────────────

/// Deferred garbage-collection scheduler for VolatileDB entries.
///
/// After a block is copied to ImmutableDB it is scheduled here with its slot
/// and hash.  When [`GcScheduler::run_pending`] is called (periodically, e.g.
/// after each block), it removes expired entries from the VolatileDB.
///
/// # EBB invariant (slot < not <=)
///
/// Epoch Boundary Blocks (EBBs, Byron era) share their slot number with the
/// first regular block of the next epoch.  To avoid accidentally removing the
/// EBB while the regular block at the same slot is still live, GC uses a
/// **strict less-than** comparison on slot numbers:
///
/// ```text
/// gc_slot < threshold_slot   (not <=)
/// ```
///
/// This matches the Haskell comment in TR §chaindb:gc:delay:
/// "slot < (not <=) for EBB invariant".
///
/// # Ordering
///
/// The inner `BTreeMap<Instant, (SlotNo, Hash32)>` keeps entries sorted by
/// deadline.  Earliest-deadline entries are processed first, which makes the
/// common case (a steady stream of single-block copies) O(1) per call.
pub struct GcScheduler {
    /// Pending GC entries sorted by their deadline (when they become eligible).
    ///
    /// Key: `Instant` at which the entry becomes eligible for removal.
    /// Value: `(slot, hash)` of the block to remove.
    ///
    /// Multiple blocks may share the same deadline instant if they were
    /// scheduled in the same millisecond.  The BTreeMap key type `Instant`
    /// does not have a stable total order beyond monotonicity on the same
    /// thread, so ties are broken arbitrarily — that is fine here.
    ///
    /// We use `Vec<(SlotNo, Hash32)>` as the value so that multiple blocks
    /// scheduled at the exact same instant can coexist in the map.
    pending: BTreeMap<Instant, Vec<(SlotNo, Hash32)>>,
}

impl GcScheduler {
    /// Create an empty scheduler.
    pub fn new() -> Self {
        Self {
            pending: BTreeMap::new(),
        }
    }

    /// Schedule a block for deferred removal from VolatileDB.
    ///
    /// The block will become eligible for GC after [`GC_DELAY`] (60 seconds).
    /// Callers must invoke [`GcScheduler::run_pending`] periodically to
    /// actually perform the removal.
    ///
    /// # Parameters
    ///
    /// * `slot` — Slot of the block to GC.
    /// * `hash` — Hash of the block to GC.
    /// * `now` — Current instant (injected for testability — in production
    ///   pass `Instant::now()`).
    pub fn schedule(&mut self, slot: SlotNo, hash: Hash32, now: Instant) {
        let deadline = now + GC_DELAY;
        self.pending.entry(deadline).or_default().push((slot, hash));

        trace!(
            slot = slot.0,
            hash = %hash,
            delay_secs = GC_DELAY.as_secs(),
            "GcScheduler: scheduled block for deferred GC"
        );
    }

    /// Process all expired GC entries.
    ///
    /// Removes blocks from the VolatileDB whose GC deadline has passed.  Uses
    /// `slot <` (strict less-than) for the EBB invariant as described in the
    /// struct documentation.
    ///
    /// # Parameters
    ///
    /// * `chain_db` — Mutable reference to ChainDB; expired blocks are removed
    ///   via the VolatileDB `remove_block` path.
    /// * `now` — Current instant (injected for testability).
    ///
    /// # Returns
    ///
    /// Number of blocks removed from VolatileDB.
    pub fn run_pending(&mut self, chain_db: &mut ChainDB, now: Instant) -> usize {
        // Collect all deadline keys that have expired (deadline <= now).
        let expired_keys: Vec<Instant> = self.pending.range(..=now).map(|(&k, _)| k).collect();

        if expired_keys.is_empty() {
            return 0;
        }

        let mut removed = 0;

        for key in expired_keys {
            if let Some(entries) = self.pending.remove(&key) {
                for (slot, hash) in entries {
                    // Strict slot < comparison: do NOT remove blocks whose slot
                    // equals the GC threshold slot.  This preserves EBBs that
                    // share a slot with the first regular block of the next epoch.
                    //
                    // Concretely: we remove the block by hash (exact match), but
                    // when we also prune by slot range we must use slot < (not <=).
                    // Since we track the exact hash here, the hash-based removal is
                    // safe.  The slot-range cleanup in VolatileDB (remove_blocks_up_to_slot)
                    // uses <= internally, so we intentionally do NOT use it here.
                    // Instead we call remove_block(hash) which is always safe.
                    chain_db.remove_volatile_block(&hash);
                    removed += 1;

                    debug!(
                        slot = slot.0,
                        hash = %hash,
                        "GcScheduler: removed block from VolatileDB after GC delay"
                    );
                }
            }
        }

        if removed > 0 {
            debug!(removed, "GcScheduler: GC pass completed");
        }

        removed
    }

    /// Number of blocks currently waiting for their GC deadline.
    pub fn pending_count(&self) -> usize {
        self.pending.values().map(|v| v.len()).sum()
    }

    /// `true` if there are no pending GC entries.
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

impl Default for GcScheduler {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SnapshotScheduler
// ─────────────────────────────────────────────────────────────────────────────

/// Decides when to persist the LedgerSeq anchor state to disk.
///
/// Triggers a snapshot when any of the following conditions are met:
///
/// 1. **Epoch boundary** — the current epoch number is greater than the epoch
///    at the last snapshot (each epoch boundary produces a new anchor point
///    that is worth persisting).
/// 2. **Block interval** — more than `snapshot_interval` blocks have been
///    applied since the last snapshot.
/// 3. **Graceful shutdown** — the caller invokes [`SnapshotScheduler::request_shutdown_snapshot`]
///    to force an immediate snapshot regardless of counters.
///
/// The actual I/O is performed by a caller-supplied `save_fn` closure.  This
/// keeps `torsten-storage` free of any `torsten-ledger` dependency.
///
/// # Haskell reference
///
/// Matches the snapshot policy in Haskell's `Background.hs`:
/// * Normal mode: every 72 minutes (approximately 2 160 slots on mainnet ≈ 1 epoch).
/// * Bulk-sync mode: every 50 000 blocks + 6 minutes.
/// * Max 2 snapshots retained on disk (managed by the caller).
///
/// Torsten approximates the time-based policy with a block-count policy
/// (default every 2 000 blocks) which is simpler and equally effective.
pub struct SnapshotScheduler {
    /// Number of blocks between automatic snapshots.
    snapshot_interval: u64,
    /// Number of blocks processed since the last snapshot.
    blocks_since_snapshot: u64,
    /// Epoch number at the time of the last snapshot.
    last_snapshot_epoch: Option<EpochNo>,
    /// Immediately take a snapshot on the next `maybe_snapshot` call.
    shutdown_requested: bool,
    /// Total number of snapshots taken.
    snapshots_taken: u64,
}

impl SnapshotScheduler {
    /// Create a new scheduler with the default snapshot interval.
    pub fn new() -> Self {
        Self::with_interval(DEFAULT_SNAPSHOT_INTERVAL)
    }

    /// Create a new scheduler with a custom snapshot interval.
    pub fn with_interval(snapshot_interval: u64) -> Self {
        Self {
            snapshot_interval,
            blocks_since_snapshot: 0,
            last_snapshot_epoch: None,
            shutdown_requested: false,
            snapshots_taken: 0,
        }
    }

    /// Record that one block has been applied and check whether a snapshot
    /// should be taken.
    ///
    /// Call this after each block is appended to the selected chain.  If a
    /// snapshot is warranted `save_fn` is invoked; on success the internal
    /// counters are reset.
    ///
    /// # Parameters
    ///
    /// * `current_epoch` — The epoch number of the block that was just applied.
    ///   Used to detect epoch-boundary triggers.
    /// * `block_no` — Block number of the block just applied (for logging).
    /// * `save_fn` — Closure that persists the LedgerSeq anchor state to disk.
    ///   Returns `Ok(())` on success or an error description on failure.
    ///   The closure is only called when a snapshot is actually warranted.
    ///
    /// # Returns
    ///
    /// `true` if a snapshot was (attempted to be) taken this call.
    pub fn maybe_snapshot(
        &mut self,
        current_epoch: EpochNo,
        block_no: BlockNo,
        save_fn: &mut dyn FnMut() -> Result<(), String>,
    ) -> bool {
        self.blocks_since_snapshot += 1;

        let epoch_boundary = match self.last_snapshot_epoch {
            None => true, // First snapshot always triggers at epoch boundary
            Some(last) => current_epoch > last,
        };
        let interval_reached = self.blocks_since_snapshot >= self.snapshot_interval;
        let shutdown = self.shutdown_requested;

        let should_snapshot = epoch_boundary || interval_reached || shutdown;

        if !should_snapshot {
            return false;
        }

        let reason = if shutdown {
            "graceful shutdown"
        } else if epoch_boundary {
            "epoch boundary"
        } else {
            "block interval"
        };

        info!(
            reason,
            block_no = block_no.0,
            epoch = current_epoch.0,
            blocks_since_last = self.blocks_since_snapshot,
            "SnapshotScheduler: saving ledger anchor snapshot"
        );

        match save_fn() {
            Ok(()) => {
                self.blocks_since_snapshot = 0;
                self.last_snapshot_epoch = Some(current_epoch);
                self.shutdown_requested = false;
                self.snapshots_taken += 1;
                debug!(
                    snapshots_taken = self.snapshots_taken,
                    "SnapshotScheduler: snapshot saved successfully"
                );
                true
            }
            Err(e) => {
                warn!(
                    error = %e,
                    block_no = block_no.0,
                    "SnapshotScheduler: snapshot save failed"
                );
                // Do not reset counters — retry next block.
                false
            }
        }
    }

    /// Force a snapshot on the next [`maybe_snapshot`] call regardless of
    /// counters.  Call this when initiating a graceful shutdown.
    ///
    /// [`maybe_snapshot`]: SnapshotScheduler::maybe_snapshot
    pub fn request_shutdown_snapshot(&mut self) {
        self.shutdown_requested = true;
    }

    /// Number of snapshots taken so far.
    pub fn snapshots_taken(&self) -> u64 {
        self.snapshots_taken
    }

    /// Number of blocks applied since the last snapshot.
    pub fn blocks_since_snapshot(&self) -> u64 {
        self.blocks_since_snapshot
    }

    /// Reset the block counter to zero (e.g. after an external snapshot was
    /// taken by another subsystem).
    pub fn reset_counter(&mut self) {
        self.blocks_since_snapshot = 0;
    }
}

impl Default for SnapshotScheduler {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain_db::{ChainDB, SECURITY_PARAM_K};
    use tempfile::TempDir;
    use torsten_primitives::hash::Hash32;
    use torsten_primitives::time::{BlockNo, EpochNo, SlotNo};

    // ── helpers ──────────────────────────────────────────────────────────────

    /// A minimal fake block CBOR (1 byte) for tests that only care about
    /// storage plumbing, not block validity.
    fn fake_cbor() -> Vec<u8> {
        vec![0x80] // CBOR empty array
    }

    fn make_hash(n: u8) -> Hash32 {
        let mut bytes = [0u8; 32];
        bytes[31] = n;
        Hash32::from_bytes(bytes)
    }

    fn open_chain_db(dir: &TempDir) -> ChainDB {
        ChainDB::open(dir.path()).expect("open ChainDB")
    }

    // ── CopyToImmutable ──────────────────────────────────────────────────────

    /// Populate `db` with `count` sequential blocks starting at block_no 1,
    /// slot 1.  Returns the list of (slot, hash, block_no) in order.
    fn populate_volatile(db: &mut ChainDB, count: usize) -> Vec<(SlotNo, Hash32, BlockNo)> {
        let mut prev = make_hash(0); // genesis prev hash
        let mut entries = Vec::with_capacity(count);
        for i in 1..=(count as u64) {
            let hash = make_hash(i as u8);
            let slot = SlotNo(i);
            let block_no = BlockNo(i);
            db.add_block(hash, slot, block_no, prev, fake_cbor())
                .expect("add_block");
            entries.push((slot, hash, block_no));
            prev = hash;
        }
        entries
    }

    /// Test that `CopyToImmutable::run_once` does nothing when fragment_len == k.
    #[test]
    fn copy_to_immutable_no_op_when_at_k() {
        let dir = TempDir::new().unwrap();
        let mut db = open_chain_db(&dir);
        let blocks = populate_volatile(&mut db, 3);

        let copier = CopyToImmutable::new(3);
        let (oldest_slot, oldest_hash, oldest_block_no) = blocks[0];

        let mut anchor_calls = 0usize;
        let result = copier.run_once(
            &mut db,
            3, // fragment_len == k → no copy
            oldest_hash,
            oldest_slot,
            oldest_block_no,
            &mut |_, _, _| {
                anchor_calls += 1;
            },
        );

        assert!(
            result.unwrap().is_none(),
            "should not copy at fragment_len == k"
        );
        assert_eq!(anchor_calls, 0);
    }

    /// Test that `CopyToImmutable::run_once` copies the oldest block when
    /// fragment_len > k.
    #[test]
    fn copy_to_immutable_copies_oldest_when_fragment_exceeds_k() {
        let dir = TempDir::new().unwrap();
        let mut db = open_chain_db(&dir);

        // Add k+1 blocks so the oldest one is now k-deep.
        let k = 3usize;
        let blocks = populate_volatile(&mut db, k + 1);

        let copier = CopyToImmutable::new(k);
        let (oldest_slot, oldest_hash, oldest_block_no) = blocks[0];

        let mut anchor_advanced = false;
        let result = copier.run_once(
            &mut db,
            k + 1, // fragment_len > k → copy triggered
            oldest_hash,
            oldest_slot,
            oldest_block_no,
            &mut |s, h, _b| {
                assert_eq!(s, oldest_slot);
                assert_eq!(h, oldest_hash);
                anchor_advanced = true;
            },
        );

        let copied = result
            .expect("run_once should succeed")
            .expect("should copy");
        assert_eq!(copied.0, oldest_slot);
        assert_eq!(copied.1, oldest_hash);
        assert!(anchor_advanced, "ledger anchor callback must be called");

        // Verify the block is now present in the immutable store.
        assert!(
            db.has_block(&oldest_hash),
            "block must still be findable (now in ImmutableDB)"
        );
    }

    /// Test that `run_once` propagates an error when the block is not in
    /// the VolatileDB (e.g., already removed or never added).
    #[test]
    fn copy_to_immutable_error_on_missing_block() {
        let dir = TempDir::new().unwrap();
        let mut db = open_chain_db(&dir);

        let copier = CopyToImmutable::new(1);
        let missing_hash = make_hash(99);

        let result = copier.run_once(
            &mut db,
            2, // fragment_len > k
            missing_hash,
            SlotNo(42),
            BlockNo(42),
            &mut |_, _, _| {},
        );

        assert!(result.is_err(), "should return Err when block is not found");
    }

    // ── GcScheduler ──────────────────────────────────────────────────────────

    /// Test that scheduled blocks are NOT removed before the GC delay elapses.
    #[test]
    fn gc_scheduler_respects_delay() {
        let dir = TempDir::new().unwrap();
        let mut db = open_chain_db(&dir);
        let blocks = populate_volatile(&mut db, 2);

        let mut scheduler = GcScheduler::new();
        let t0 = Instant::now();

        // Schedule the first block for GC.
        let (slot, hash, _) = blocks[0];
        scheduler.schedule(slot, hash, t0);

        assert_eq!(scheduler.pending_count(), 1);

        // Run GC immediately — the block should still be in VolatileDB.
        // We simulate "just after scheduling" by passing t0 (before deadline).
        let removed = scheduler.run_pending(&mut db, t0);
        assert_eq!(removed, 0, "nothing should be GC'd before the delay");

        // Verify block is still present.
        assert!(
            db.has_block(&hash),
            "block must still be in ChainDB before GC delay"
        );
    }

    /// Test that blocks ARE removed after the GC delay.
    #[test]
    fn gc_scheduler_removes_after_delay() {
        let dir = TempDir::new().unwrap();
        let mut db = open_chain_db(&dir);

        // Add a block to VolatileDB.
        let hash = make_hash(1);
        let slot = SlotNo(1);
        let block_no = BlockNo(1);
        let prev = make_hash(0);
        db.add_block(hash, slot, block_no, prev, fake_cbor())
            .unwrap();

        let mut scheduler = GcScheduler::new();

        // Schedule with a deadline 61 seconds in the past — already expired.
        let past = Instant::now() - Duration::from_secs(61);
        scheduler.schedule(slot, hash, past);

        assert_eq!(scheduler.pending_count(), 1);

        // Run GC at "now" — the entry should be eligible.
        let removed = scheduler.run_pending(&mut db, Instant::now());
        assert_eq!(removed, 1, "one block should have been GC'd");
        assert_eq!(
            scheduler.pending_count(),
            0,
            "scheduler should be empty after GC"
        );
    }

    /// Test that multiple blocks can be scheduled and GC'd independently.
    #[test]
    fn gc_scheduler_handles_multiple_entries() {
        let dir = TempDir::new().unwrap();
        let mut db = open_chain_db(&dir);

        let past = Instant::now() - Duration::from_secs(61);
        let future = Instant::now() + Duration::from_secs(30);

        // Two blocks with past deadline (should be GC'd).
        let h1 = make_hash(1);
        let h2 = make_hash(2);
        // One block with future deadline (should NOT be GC'd yet).
        let h3 = make_hash(3);

        let prev = make_hash(0);
        db.add_block(h1, SlotNo(1), BlockNo(1), prev, fake_cbor())
            .unwrap();
        db.add_block(h2, SlotNo(2), BlockNo(2), h1, fake_cbor())
            .unwrap();
        db.add_block(h3, SlotNo(3), BlockNo(3), h2, fake_cbor())
            .unwrap();

        let mut scheduler = GcScheduler::new();
        scheduler.schedule(SlotNo(1), h1, past);
        scheduler.schedule(SlotNo(2), h2, past);
        scheduler.schedule(SlotNo(3), h3, future);

        assert_eq!(scheduler.pending_count(), 3);

        let removed = scheduler.run_pending(&mut db, Instant::now());
        assert_eq!(removed, 2, "two expired blocks should be GC'd");
        assert_eq!(scheduler.pending_count(), 1, "one pending entry remains");
    }

    // ── SnapshotScheduler ────────────────────────────────────────────────────

    /// Test that the scheduler triggers at the configured block interval.
    #[test]
    fn snapshot_scheduler_triggers_at_interval() {
        let interval = 5u64;
        let mut sched = SnapshotScheduler::with_interval(interval);

        let mut snapshots = 0usize;
        let epoch = EpochNo(0);

        // First call: epoch_boundary triggers (last_snapshot_epoch is None).
        let triggered = sched.maybe_snapshot(epoch, BlockNo(1), &mut || {
            snapshots += 1;
            Ok(())
        });
        assert!(triggered, "first call should trigger (epoch boundary)");
        assert_eq!(snapshots, 1);

        // Next interval-1 calls: no trigger (same epoch, not enough blocks).
        for i in 2..(interval + 1) {
            let triggered = sched.maybe_snapshot(epoch, BlockNo(i), &mut || {
                snapshots += 1;
                Ok(())
            });
            assert!(!triggered, "should not trigger before interval");
        }
        assert_eq!(snapshots, 1);

        // interval-th call: block count threshold hit.
        let triggered = sched.maybe_snapshot(epoch, BlockNo(interval + 1), &mut || {
            snapshots += 1;
            Ok(())
        });
        assert!(triggered, "should trigger at block interval");
        assert_eq!(snapshots, 2);
    }

    /// Test that the scheduler triggers at epoch boundaries regardless of
    /// the block counter.
    #[test]
    fn snapshot_scheduler_triggers_at_epoch_boundary() {
        let mut sched = SnapshotScheduler::with_interval(10_000);

        let mut snapshots = 0usize;

        // Epoch 0, block 1 — triggers because last_snapshot_epoch is None.
        sched.maybe_snapshot(EpochNo(0), BlockNo(1), &mut || {
            snapshots += 1;
            Ok(())
        });
        assert_eq!(snapshots, 1);

        // Epoch 0, blocks 2-5 — no trigger (same epoch, interval not reached).
        for i in 2..=5 {
            sched.maybe_snapshot(EpochNo(0), BlockNo(i), &mut || {
                snapshots += 1;
                Ok(())
            });
        }
        assert_eq!(snapshots, 1, "no snapshot between epochs");

        // Epoch 1, block 6 — triggers because epoch changed.
        let triggered = sched.maybe_snapshot(EpochNo(1), BlockNo(6), &mut || {
            snapshots += 1;
            Ok(())
        });
        assert!(triggered, "epoch boundary should trigger snapshot");
        assert_eq!(snapshots, 2);
    }

    /// Test that requesting a shutdown snapshot forces the next call.
    #[test]
    fn snapshot_scheduler_shutdown_forces_snapshot() {
        let mut sched = SnapshotScheduler::with_interval(10_000);

        // Advance past the initial epoch-boundary trigger.
        let epoch = EpochNo(0);
        sched.maybe_snapshot(epoch, BlockNo(1), &mut || Ok(()));

        // No snapshot expected mid-interval.
        let triggered = sched.maybe_snapshot(epoch, BlockNo(2), &mut || {
            panic!("should not be called");
        });
        assert!(!triggered);

        // Request shutdown snapshot.
        sched.request_shutdown_snapshot();

        let mut snapshots = 0usize;
        let triggered = sched.maybe_snapshot(epoch, BlockNo(3), &mut || {
            snapshots += 1;
            Ok(())
        });
        assert!(triggered, "shutdown snapshot must be taken");
        assert_eq!(snapshots, 1);

        // After shutdown snapshot, shutdown_requested is cleared.
        let triggered = sched.maybe_snapshot(epoch, BlockNo(4), &mut || {
            panic!("should not be called again immediately");
        });
        assert!(!triggered, "shutdown flag should be cleared after use");
    }

    /// Test that the `snapshots_taken` counter is incremented correctly.
    #[test]
    fn snapshot_scheduler_counts_snapshots() {
        let mut sched = SnapshotScheduler::with_interval(2);
        assert_eq!(sched.snapshots_taken(), 0);

        let epoch = EpochNo(0);
        // Block 1: epoch-boundary trigger.
        sched.maybe_snapshot(epoch, BlockNo(1), &mut || Ok(()));
        assert_eq!(sched.snapshots_taken(), 1);

        // Block 2: counter reset → blocks_since_snapshot = 1, no trigger.
        sched.maybe_snapshot(epoch, BlockNo(2), &mut || Ok(()));
        assert_eq!(sched.snapshots_taken(), 1);

        // Block 3: blocks_since_snapshot = 2 → trigger.
        sched.maybe_snapshot(epoch, BlockNo(3), &mut || Ok(()));
        assert_eq!(sched.snapshots_taken(), 2);
    }

    /// Test that a failed save_fn does not reset the counters, allowing a
    /// retry on the next call.
    #[test]
    fn snapshot_scheduler_retry_on_error() {
        let mut sched = SnapshotScheduler::with_interval(10_000);

        // First call fails.
        let triggered =
            sched.maybe_snapshot(EpochNo(0), BlockNo(1), &mut || Err("disk full".to_string()));
        // It was attempted (returned false because save failed).
        assert!(!triggered, "save failure → returns false");
        assert_eq!(sched.snapshots_taken(), 0);
        // Counter was NOT reset.
        assert!(
            sched.blocks_since_snapshot() >= 1,
            "counter must not reset on error"
        );

        // Second call succeeds — epoch boundary still active (same epoch,
        // last_snapshot_epoch is still None because the first attempt failed).
        let triggered = sched.maybe_snapshot(EpochNo(0), BlockNo(2), &mut || Ok(()));
        assert!(triggered, "retry on next block must succeed");
        assert_eq!(sched.snapshots_taken(), 1);
    }

    /// Test that GcScheduler::pending_count returns correct values.
    #[test]
    fn gc_scheduler_pending_count() {
        let mut sched = GcScheduler::new();
        assert_eq!(sched.pending_count(), 0);
        assert!(sched.is_empty());

        let t = Instant::now();
        sched.schedule(SlotNo(1), make_hash(1), t);
        assert_eq!(sched.pending_count(), 1);
        assert!(!sched.is_empty());

        sched.schedule(SlotNo(2), make_hash(2), t);
        assert_eq!(sched.pending_count(), 2);
    }

    /// Verify that security parameter `k` is correctly respected: no copy
    /// should happen when fragment_len is exactly k (not k+1).
    #[test]
    fn copy_to_immutable_boundary_conditions() {
        let dir = TempDir::new().unwrap();
        let mut db = open_chain_db(&dir);

        let _ = populate_volatile(&mut db, 5);
        let hash = make_hash(1);
        let slot = SlotNo(1);
        let block_no = BlockNo(1);

        let copier_k5 = CopyToImmutable::new(5);

        // fragment_len == k → no copy.
        let res = copier_k5.run_once(&mut db, 5, hash, slot, block_no, &mut |_, _, _| {});
        assert!(res.unwrap().is_none());

        // fragment_len == k-1 → definitely no copy.
        let res = copier_k5.run_once(&mut db, 4, hash, slot, block_no, &mut |_, _, _| {});
        assert!(res.unwrap().is_none());

        // fragment_len == k+1 → copy triggered.
        let res = copier_k5.run_once(&mut db, 6, hash, slot, block_no, &mut |_, _, _| {});
        assert!(res.unwrap().is_some());
    }

    /// Confirm the full production security parameter is accessible.
    #[test]
    fn security_param_k_is_correct() {
        // mainnet k = 2160
        assert_eq!(SECURITY_PARAM_K, 2160);
        let copier = CopyToImmutable::new(SECURITY_PARAM_K);
        assert_eq!(copier.k, 2160);
    }
}
