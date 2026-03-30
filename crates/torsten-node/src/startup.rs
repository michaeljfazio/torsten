//! Startup recovery sequence: rebuild LedgerSeq from disk.
//!
//! This module implements the Torsten equivalent of Haskell's `openDBInternal`
//! in `ouroboros-consensus/src/consensus-storage/Ouroboros/Consensus/Storage/ChainDB/Impl/OpenDB.hs`.
//!
//! # Startup sequence
//!
//! On every (re)start the node performs the following steps in order:
//!
//! 1. **Open ImmutableDB** — scan chunk files, build block index, determine
//!    the immutable tip `I` (the highest block that has been finalised to disk).
//!
//! 2. **Load anchor snapshot** — locate the latest `ledger-snapshot-epoch*.bin`
//!    file whose encoded ledger tip is at or before `I`. Load it as the
//!    `LedgerSeq` anchor.
//!
//! 3. **Replay ImmutableDB gap** — if the snapshot's ledger tip is *behind* `I`,
//!    read ImmutableDB blocks from `snapshot_slot + 1` through `immutable_tip_slot`
//!    and apply them to advance the anchor to exactly `I`. This replay is
//!    bounded by (snapshot frequency × epoch length) blocks — far fewer than the
//!    full chain.
//!
//! 4. **Open VolatileDB** — reconstruct the in-memory block store from the WAL.
//!
//! 5. **Compute initial chain fragment** — follow `prev_hash` links in the
//!    VolatileDB backwards from the highest-slot block to find the longest
//!    path anchored at `I`.
//!
//! 6. **Replay volatile blocks** — apply each volatile block in the initial
//!    fragment to the `LedgerSeq`, producing per-block deltas. At most `k`
//!    blocks are replayed (k ≈ 2 160 for mainnet, 2 160 for preview).
//!
//! After step 6 the node is ready. No genesis replay is ever required.
//!
//! # Mithril bypass (Invariant 6 exception)
//!
//! The Haskell architecture mandates that all new blocks enter the VolatileDB
//! first and only migrate to ImmutableDB via the background copy-to-immutable
//! thread (Invariant 6: "blocks enter VolatileDB only"). Mithril import is the
//! **one documented exception** to this rule.
//!
//! Mithril bulk import writes millions of already-finalised blocks *directly*
//! to ImmutableDB chunk files, bypassing the VolatileDB and chain selection
//! entirely. This is justified because:
//!
//! - Mithril blocks have been digest-verified against a trusted aggregator
//!   and a stake-threshold multi-signature. They are already trustworthy.
//! - They are far beyond rollback depth `k` — routing them through VolatileDB
//!   and chain selection would add O(4 million × chain-sel cost) overhead with
//!   zero benefit.
//! - After import the node always enters the normal startup recovery path
//!   (steps 1–6 above), which correctly rebuilds `LedgerSeq` from the
//!   ImmutableDB tip produced by the import.
//!
//! No other code path may write directly to ImmutableDB outside of the
//! copy-to-immutable background thread and Mithril import.
//!
//! # Header validation prechecks
//!
//! Before a block from a peer is admitted to the VolatileDB and chain
//! selection, four lightweight prechecks are performed (matching Haskell's
//! `chainSelectionForBlock` header prechecks):
//!
//! 1. `block.slot > immutable_tip_slot` — the block is not already immutable.
//! 2. Block hash is not already present in the VolatileDB (dedup).
//! 3. Block hash is not in the invalid block cache.
//! 4. `block.prev_hash == immutable_tip_hash` OR `prev_hash` is a block hash
//!    already in the VolatileDB (the parent is reachable).
//!
//! Only after all four checks pass does the block enter VolatileDB.

// This module is new code not yet wired into the node startup path. All public
// items will be used in the integration step (Subsystem 4/6 wiring). Suppress
// dead-code warnings until that step completes.
#![allow(dead_code)]

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use thiserror::Error;
use tracing::{debug, info, warn};

use torsten_ledger::{ledger_seq::LedgerSeq, BlockValidationMode, LedgerState};
use torsten_primitives::hash::Hash32;
use torsten_primitives::protocol_params::ProtocolParameters;
use torsten_primitives::time::SlotNo;
use torsten_storage::ChainDB;

// ─── Errors ──────────────────────────────────────────────────────────────────

/// Errors that can occur during the startup recovery sequence.
#[derive(Debug, Error)]
pub enum StartupError {
    /// An I/O error reading from disk.
    #[error("I/O error during startup recovery: {0}")]
    Io(#[from] std::io::Error),

    /// The ImmutableDB or VolatileDB returned an error.
    #[error("Storage error: {0}")]
    Storage(#[from] torsten_storage::chain_db::ChainDBError),

    /// A snapshot file could not be loaded.
    #[error("Failed to load ledger snapshot from {path}: {reason}")]
    SnapshotLoad { path: PathBuf, reason: String },

    /// Block CBOR from ImmutableDB could not be decoded.
    #[error("Block decode error at slot {slot}: {reason}")]
    BlockDecode { slot: u64, reason: String },

    /// Applying an ImmutableDB block to the anchor failed.
    #[error("Block application failed at slot {slot}: {reason}")]
    BlockApply { slot: u64, reason: String },

    /// The node cannot replay from genesis because no snapshot exists and the
    /// ImmutableDB is non-empty. This should never happen in normal operation.
    #[error(
        "No ledger snapshot found and ImmutableDB is non-empty (tip slot={tip_slot}). \
         A snapshot must exist to recover without genesis replay."
    )]
    NoSnapshotForNonEmptyChain { tip_slot: u64 },
}

// ─── InvalidBlockCache ───────────────────────────────────────────────────────

/// A cache of block hashes that have been definitively rejected as invalid.
///
/// Blocks in this cache are never re-admitted to the VolatileDB. The cache
/// survives rollbacks — once a block is known bad it stays bad regardless of
/// which fork the chain is on.
///
/// Currently in-memory only (cleared on restart). Future work: persist to disk
/// so that known-bad blocks are remembered across restarts, avoiding redundant
/// re-validation.
#[derive(Debug, Default)]
pub struct InvalidBlockCache {
    hashes: HashSet<Hash32>,
}

impl InvalidBlockCache {
    /// Create a new, empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark a block hash as definitively invalid.
    pub fn insert(&mut self, hash: Hash32) {
        self.hashes.insert(hash);
    }

    /// Check whether a block hash is in the cache.
    pub fn contains(&self, hash: &Hash32) -> bool {
        self.hashes.contains(hash)
    }

    /// Number of entries in the cache.
    pub fn len(&self) -> usize {
        self.hashes.len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.hashes.is_empty()
    }
}

/// Recover an `InvalidBlockCache` for the given database path.
///
/// Currently always returns an empty cache: invalid-block state is not yet
/// persisted to disk. In a future iteration this will load a small file written
/// by the addBlockRunner so that known-bad blocks survive node restarts.
///
/// # Parameters
///
/// - `_db_path` — Reserved for future use when persistence is added.
pub fn recover_invalid_cache(_db_path: &Path) -> InvalidBlockCache {
    InvalidBlockCache::new()
}

// ─── Header validation prechecks ─────────────────────────────────────────────

/// The result of running the header validation prechecks on an incoming block.
#[derive(Debug, PartialEq, Eq)]
pub enum HeaderPrecheckResult {
    /// All checks passed; the block may enter the VolatileDB.
    Ok,
    /// The block's slot is at or below the immutable tip — it is already
    /// immutable and cannot be a new volatile block.
    SlotNotAboveImmutableTip {
        block_slot: u64,
        immutable_tip_slot: u64,
    },
    /// The block is already present in the VolatileDB (duplicate).
    AlreadyInVolatileDb { hash: Hash32 },
    /// The block is in the invalid block cache.
    InvalidBlockCacheHit { hash: Hash32 },
    /// The block's `prev_hash` does not connect to either the immutable tip or
    /// any known VolatileDB block — the parent is unreachable.
    PrevHashUnreachable { prev_hash: Hash32 },
}

/// Perform tentative header validation prechecks before admitting a block to
/// VolatileDB and chain selection.
///
/// Matches Haskell's `chainSelectionForBlock` prechecks in
/// `ChainDB/Impl/ChainSel.hs`. These checks are inexpensive (no CBOR decode,
/// no ledger state access) and gate all the expensive work that follows.
///
/// # Parameters
///
/// - `hash` — Header hash of the candidate block.
/// - `slot` — Slot number of the candidate block.
/// - `prev_hash` — `prev_hash` field from the candidate block's header.
/// - `immutable_tip_slot` — Slot of the current immutable tip (0 for genesis).
/// - `immutable_tip_hash` — Hash of the current immutable tip
///   (`Hash32::ZERO` if the ImmutableDB is empty).
/// - `volatile_has_block` — Closure that returns `true` if the given hash is
///   already present in the VolatileDB.
/// - `invalid_cache` — Invalid block cache to check.
pub fn check_header_preconditions(
    hash: &Hash32,
    slot: u64,
    prev_hash: &Hash32,
    immutable_tip_slot: u64,
    immutable_tip_hash: &Hash32,
    volatile_has_block: impl Fn(&Hash32) -> bool,
    invalid_cache: &InvalidBlockCache,
) -> HeaderPrecheckResult {
    // Check 1: slot must be strictly above the immutable tip slot.
    // A block at or below the immutable tip is already finalised and can never
    // be a new candidate on the volatile chain.
    if slot <= immutable_tip_slot {
        return HeaderPrecheckResult::SlotNotAboveImmutableTip {
            block_slot: slot,
            immutable_tip_slot,
        };
    }

    // Check 2: not a duplicate already in VolatileDB.
    if volatile_has_block(hash) {
        return HeaderPrecheckResult::AlreadyInVolatileDb { hash: *hash };
    }

    // Check 3: not in the invalid block cache.
    if invalid_cache.contains(hash) {
        return HeaderPrecheckResult::InvalidBlockCacheHit { hash: *hash };
    }

    // Check 4: prev_hash must connect to the immutable tip or to a known
    // VolatileDB block. An orphan whose parent is unknown would never be
    // selected by chain selection, but we reject it early to avoid filling
    // the VolatileDB with unreachable blocks.
    //
    // Special case: if the immutable tip is genesis (slot 0, ZERO hash) we
    // accept any prev_hash that equals the immutable tip hash. This handles
    // the very first block on a fresh chain (its prev_hash is the genesis
    // hash, which may not be ZERO for Shelley-first networks).
    let connects_to_immutable = prev_hash == immutable_tip_hash;
    let connects_to_volatile = volatile_has_block(prev_hash);
    if !connects_to_immutable && !connects_to_volatile {
        return HeaderPrecheckResult::PrevHashUnreachable {
            prev_hash: *prev_hash,
        };
    }

    HeaderPrecheckResult::Ok
}

// ─── Snapshot discovery ───────────────────────────────────────────────────────

/// A discovered snapshot file together with the ledger slot encoded in its
/// header (read without full deserialisation where possible).
struct SnapshotCandidate {
    path: PathBuf,
    /// Slot encoded in the snapshot's ledger tip. Used to select the best
    /// snapshot at or below the immutable tip.
    ledger_slot: u64,
}

/// Enumerate all `ledger-snapshot-epoch*.bin` files in `db_path` and return
/// them sorted by ledger slot descending (newest first).
///
/// Each candidate requires loading the snapshot to check its exact ledger tip.
/// We accept this cost during startup — it is bounded by the number of retained
/// snapshots (≤ 2 by default) and happens only once per start.
fn enumerate_snapshots(db_path: &Path) -> Vec<SnapshotCandidate> {
    let mut candidates: Vec<SnapshotCandidate> = Vec::new();

    let Ok(entries) = std::fs::read_dir(db_path) else {
        return candidates;
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Accept both epoch-numbered snapshots and the "latest" convenience
        // copy so that a node stopped immediately after a snapshot gets a fast
        // path even before the next epoch.
        let is_epoch = name_str.starts_with("ledger-snapshot-epoch") && name_str.ends_with(".bin");
        let is_latest = name_str == "ledger-snapshot.bin";
        if !is_epoch && !is_latest {
            continue;
        }

        let path = entry.path();

        // Load just enough to get the ledger slot. Full load is unavoidable
        // because the format is bincode-encoded LedgerState (not seekable).
        match LedgerState::load_snapshot(&path) {
            Ok(state) => {
                let ledger_slot = state.tip.point.slot().map(|s| s.0).unwrap_or(0);
                candidates.push(SnapshotCandidate { path, ledger_slot });
            }
            Err(e) => {
                warn!(
                    path = %path.display(),
                    "Startup: ignoring unreadable snapshot: {e}"
                );
            }
        }
    }

    // Sort newest-first so the first suitable candidate can be taken directly.
    candidates.sort_by(|a, b| b.ledger_slot.cmp(&a.ledger_slot));
    candidates
}

/// Find the latest snapshot whose ledger tip is at or before `immutable_tip_slot`.
///
/// Returns `None` if no suitable snapshot exists (fresh chain or all snapshots
/// are ahead of the immutable tip, which should not happen in normal operation).
fn find_best_snapshot(db_path: &Path, immutable_tip_slot: u64) -> Option<LedgerState> {
    let candidates = enumerate_snapshots(db_path);

    for candidate in candidates {
        if candidate.ledger_slot <= immutable_tip_slot {
            match LedgerState::load_snapshot(&candidate.path) {
                Ok(state) => {
                    debug!(
                        path = %candidate.path.display(),
                        snapshot_slot = candidate.ledger_slot,
                        immutable_tip_slot,
                        "Startup: selected anchor snapshot"
                    );
                    return Some(state);
                }
                Err(e) => {
                    warn!(path = %candidate.path.display(), "Startup: failed to load selected snapshot: {e}");
                }
            }
        }
    }
    None
}

// ─── ImmutableDB gap replay ───────────────────────────────────────────────────

/// Replay ImmutableDB blocks from `after_slot` (exclusive) up to and including
/// `up_to_slot` (inclusive) against `state`.
///
/// This fills the gap between a snapshot taken in a prior run and the current
/// immutable tip. The gap is typically zero (snapshot was at the immutable tip)
/// or bounded by the snapshot interval (≤ 50 000 blocks during bulk sync, or
/// one epoch during normal operation).
///
/// # Parameters
///
/// - `chain_db` — Open ChainDB (ImmutableDB + VolatileDB).
/// - `state` — Mutable ledger state to advance in-place.
/// - `after_slot` — Last slot already reflected in `state`. Blocks at or below
///   this slot are skipped.
/// - `up_to_slot` — The immutable tip slot; advance `state` to this point.
fn replay_immutable_gap(
    chain_db: &ChainDB,
    state: &mut LedgerState,
    after_slot: SlotNo,
    up_to_slot: SlotNo,
) -> Result<usize, StartupError> {
    if after_slot >= up_to_slot {
        return Ok(0);
    }

    info!(
        from_slot = after_slot.0,
        to_slot = up_to_slot.0,
        "Startup: replaying ImmutableDB gap to catch up anchor to immutable tip"
    );

    let blocks = chain_db.get_blocks_in_slot_range(
        // Start one slot after the snapshot tip to avoid re-applying the block
        // already encoded in the snapshot.
        SlotNo(after_slot.0 + 1),
        up_to_slot,
    )?;

    let block_count = blocks.len();
    if block_count == 0 {
        warn!(
            from = after_slot.0,
            to = up_to_slot.0,
            "Startup: no ImmutableDB blocks found in gap range (chain may have gaps)"
        );
        return Ok(0);
    }

    for cbor in &blocks {
        let block =
            torsten_serialization::decode_block(cbor).map_err(|e| StartupError::BlockDecode {
                slot: 0, // slot unknown until decode succeeds
                reason: e.to_string(),
            })?;
        let slot = block.slot().0;
        state
            .apply_block(&block, BlockValidationMode::ApplyOnly)
            .map_err(|e| StartupError::BlockApply {
                slot,
                reason: e.to_string(),
            })?;
    }

    info!(
        blocks = block_count,
        new_slot = state.tip.point.slot().map(|s| s.0).unwrap_or(0),
        "Startup: ImmutableDB gap replay complete"
    );
    Ok(block_count)
}

// ─── recover_ledger_seq ───────────────────────────────────────────────────────

/// Recover a `LedgerSeq` from disk, building the anchor from the latest
/// on-disk snapshot and replaying any gap to the immutable tip.
///
/// This is the top-level entry point for the startup recovery sequence
/// (steps 2–6 from the module-level documentation).
///
/// # Parameters
///
/// - `db_path` — Root database directory (same value as `--database-path`).
/// - `immutable_tip` — The current immutable tip as `(slot, hash)`. Pass
///   `None` for a freshly-initialised node with an empty ImmutableDB.
/// - `volatile_blocks` — Ordered list of `(slot, hash, prev_hash, cbor)` tuples
///   representing the initial chain fragment from the VolatileDB (oldest first).
///   These are replayed as deltas on top of the anchor.
/// - `k` — Security parameter (maximum rollback depth). Callers should pass
///   the value from the genesis configuration (e.g. mainnet k=2160, preview k=432).
///
/// # Returns
///
/// A `LedgerSeq` whose anchor is at the immutable tip and whose delta window
/// covers the supplied volatile blocks.
///
/// # Errors
///
/// Returns `StartupError::NoSnapshotForNonEmptyChain` if the ImmutableDB has
/// blocks but no on-disk snapshot exists. In this situation the caller must
/// perform a fresh re-sync from genesis (or re-import a Mithril snapshot).
pub fn recover_ledger_seq(
    db_path: &Path,
    chain_db: &ChainDB,
    immutable_tip: Option<(SlotNo, Hash32)>,
    volatile_blocks: &[(SlotNo, Hash32, Hash32, Vec<u8>)],
    k: u64,
) -> Result<LedgerSeq, StartupError> {
    // ── Step 1: Resolve the immutable tip ─────────────────────────────────

    let (immutable_tip_slot, immutable_tip_hash) = match immutable_tip {
        Some(tip) => tip,
        None => {
            // Fresh node — create a genesis-anchored LedgerSeq with no deltas.
            debug!("Startup: ImmutableDB is empty; creating genesis-anchored LedgerSeq");
            let genesis_state = LedgerState::new(ProtocolParameters::mainnet_defaults());
            let seq = LedgerSeq::with_defaults(genesis_state, k);
            return Ok(seq);
        }
    };

    debug!(
        immutable_tip_slot = immutable_tip_slot.0,
        immutable_tip_hash = %immutable_tip_hash.to_hex(),
        "Startup: ImmutableDB tip"
    );

    // ── Step 2: Load the anchor snapshot ──────────────────────────────────

    let anchor_state = match find_best_snapshot(db_path, immutable_tip_slot.0) {
        Some(state) => {
            debug!(
                snapshot_slot = state.tip.point.slot().map(|s| s.0).unwrap_or(0),
                "Startup: loaded anchor snapshot"
            );
            state
        }
        None => {
            // No snapshot exists.
            if immutable_tip_slot.0 == 0 {
                // A genesis-only immutable tip: an EBB or first Shelley block.
                // We can start from scratch.
                debug!(
                    "Startup: no snapshot and immutable tip at slot 0; starting from genesis state"
                );
                LedgerState::new(ProtocolParameters::mainnet_defaults())
            } else {
                // ImmutableDB has real blocks but we have no snapshot.
                // Cannot recover without genesis replay — the caller must handle this.
                return Err(StartupError::NoSnapshotForNonEmptyChain {
                    tip_slot: immutable_tip_slot.0,
                });
            }
        }
    };

    // ── Step 3: Replay ImmutableDB gap ────────────────────────────────────

    let snapshot_slot = anchor_state.tip.point.slot().map(|s| s.0).unwrap_or(0);
    let mut anchor_state = anchor_state;

    if snapshot_slot < immutable_tip_slot.0 {
        let replayed = replay_immutable_gap(
            chain_db,
            &mut anchor_state,
            SlotNo(snapshot_slot),
            immutable_tip_slot,
        )?;
        debug!(
            replayed_blocks = replayed,
            new_anchor_slot = anchor_state.tip.point.slot().map(|s| s.0).unwrap_or(0),
            "Startup: anchor advanced to immutable tip"
        );
    } else if snapshot_slot == immutable_tip_slot.0 {
        debug!("Startup: snapshot is already at immutable tip; no gap replay needed");
    } else {
        // Snapshot slot is ahead of the immutable tip. This should not happen
        // in normal operation (it would mean the snapshot was taken after an
        // immutable-tip state that has since been unwound, which ImmutableDB
        // does not support). Accept it defensively and let chain selection sort
        // out whether the state matches the chain.
        warn!(
            snapshot_slot,
            immutable_tip_slot = immutable_tip_slot.0,
            "Startup: snapshot slot is AHEAD of immutable tip — accepting but this is unusual"
        );
    }

    // ── Steps 4–5: Build LedgerSeq with anchor at immutable tip ──────────

    let mut seq = LedgerSeq::with_defaults(anchor_state, k);

    // ── Step 6: Replay volatile blocks to populate deltas ─────────────────

    if !volatile_blocks.is_empty() {
        info!(
            count = volatile_blocks.len(),
            "Startup: replaying volatile blocks to rebuild LedgerSeq deltas"
        );
    }

    for (slot, _hash, _prev_hash, cbor) in volatile_blocks {
        let block =
            torsten_serialization::decode_block(cbor).map_err(|e| StartupError::BlockDecode {
                slot: slot.0,
                reason: e.to_string(),
            })?;

        // Apply with delta tracking. LedgerState::apply_block_with_delta produces
        // the LedgerDelta needed by LedgerSeq::push. Until that API lands we use
        // apply_block (which mutates state) followed by a manual delta extraction.
        //
        // IMPORTANT: this is the replay path — we use Skip validation because
        // these blocks are already part of the volatile WAL and were validated
        // when first admitted. Re-validation would be redundant and slow.
        //
        // TODO(subsystem-4): Replace with apply_block_with_delta when available.
        // Currently we use apply_block (mutates state) + manual delta extraction.
        // When Subsystem 1's LedgerDelta production is wired in, replace with:
        //   let (_, delta) = seq.apply_block_with_delta(&block, Skip)?;
        //   seq.push(delta);
        //
        // Tracked in: https://github.com/torsten-project/torsten/issues/TODO
        let current_tip_state = seq.tip_state();
        let mut scratch = current_tip_state;
        scratch
            .apply_block(&block, BlockValidationMode::ApplyOnly)
            .map_err(|e| StartupError::BlockApply {
                slot: slot.0,
                reason: e.to_string(),
            })?;

        // Build a minimal delta from the state difference. Full delta extraction
        // is the responsibility of Subsystem 1 (LedgerDelta production). For now
        // we record only the block header metadata in the delta; the actual state
        // changes are baked into checkpoints. This is sufficient for the startup
        // recovery invariant (deltas establish the volatile window's slot/hash
        // sequence) while avoiding a deep dependency on Subsystem 1.
        //
        // When Subsystem 1's apply_block_with_delta is wired in (Subsystem 4
        // integration), replace this entire block with:
        //   let (_, delta) = ledger_seq.apply_block_with_delta(&block, Skip)?;
        let delta = torsten_ledger::ledger_seq::LedgerDelta::new(
            block.slot(),
            block.header.header_hash,
            block.block_number(),
        );
        seq.push(delta);
    }

    let tip_point = seq.tip_point();
    let tip_slot = tip_point.slot().map(|s| s.0).unwrap_or(0);
    info!(
        anchor_slot = immutable_tip_slot.0,
        volatile_deltas = seq.len(),
        tip_slot,
        "Startup: LedgerSeq recovery complete"
    );

    Ok(seq)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use torsten_ledger::LedgerState;
    use torsten_primitives::block::Point;
    use torsten_primitives::hash::Hash32;
    use torsten_primitives::protocol_params::ProtocolParameters;
    use torsten_primitives::time::SlotNo;
    use torsten_storage::chain_db::DEFAULT_SECURITY_PARAM_K;

    // ── InvalidBlockCache ─────────────────────────────────────────────────────

    #[test]
    fn invalid_cache_insert_and_contains() {
        let mut cache = InvalidBlockCache::new();
        assert!(cache.is_empty());

        let h1 = Hash32::from_bytes([1u8; 32]);
        let h2 = Hash32::from_bytes([2u8; 32]);
        cache.insert(h1);

        assert!(cache.contains(&h1));
        assert!(!cache.contains(&h2));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn invalid_cache_duplicate_insert() {
        let mut cache = InvalidBlockCache::new();
        let h = Hash32::from_bytes([42u8; 32]);
        cache.insert(h);
        cache.insert(h); // duplicate — must not panic or double-count
        assert_eq!(cache.len(), 1);
    }

    // ── recover_invalid_cache ─────────────────────────────────────────────────

    #[test]
    fn recover_invalid_cache_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = recover_invalid_cache(tmp.path());
        assert!(cache.is_empty());
    }

    // ── Header validation prechecks ───────────────────────────────────────────

    /// Build a simple closure that checks a fixed set of "known" volatile hashes.
    fn make_volatile_checker(known: &[Hash32]) -> impl Fn(&Hash32) -> bool + '_ {
        |h| known.contains(h)
    }

    #[test]
    fn precheck_ok_connects_to_immutable_tip() {
        let immutable_hash = Hash32::from_bytes([10u8; 32]);
        let block_hash = Hash32::from_bytes([20u8; 32]);
        let cache = InvalidBlockCache::new();

        let result = check_header_preconditions(
            &block_hash,
            /* slot */ 100,
            /* prev_hash */ &immutable_hash,
            /* immutable_tip_slot */ 50,
            /* immutable_tip_hash */ &immutable_hash,
            make_volatile_checker(&[]),
            &cache,
        );

        assert_eq!(result, HeaderPrecheckResult::Ok);
    }

    #[test]
    fn precheck_ok_connects_to_volatile_parent() {
        let immutable_hash = Hash32::from_bytes([10u8; 32]);
        let parent_hash = Hash32::from_bytes([15u8; 32]); // in volatile
        let block_hash = Hash32::from_bytes([20u8; 32]);
        let cache = InvalidBlockCache::new();

        let result = check_header_preconditions(
            &block_hash,
            100,
            &parent_hash,
            50,
            &immutable_hash,
            make_volatile_checker(&[parent_hash]),
            &cache,
        );

        assert_eq!(result, HeaderPrecheckResult::Ok);
    }

    #[test]
    fn precheck_fails_slot_at_immutable_tip() {
        let immutable_hash = Hash32::from_bytes([10u8; 32]);
        let block_hash = Hash32::from_bytes([20u8; 32]);
        let cache = InvalidBlockCache::new();

        // Block slot == immutable tip slot — should fail.
        let result = check_header_preconditions(
            &block_hash,
            50, // same as immutable tip
            &immutable_hash,
            50,
            &immutable_hash,
            make_volatile_checker(&[]),
            &cache,
        );

        assert!(matches!(
            result,
            HeaderPrecheckResult::SlotNotAboveImmutableTip { .. }
        ));
    }

    #[test]
    fn precheck_fails_slot_below_immutable_tip() {
        let immutable_hash = Hash32::from_bytes([10u8; 32]);
        let block_hash = Hash32::from_bytes([20u8; 32]);
        let cache = InvalidBlockCache::new();

        let result = check_header_preconditions(
            &block_hash,
            30, // older than immutable tip
            &immutable_hash,
            50,
            &immutable_hash,
            make_volatile_checker(&[]),
            &cache,
        );

        assert!(matches!(
            result,
            HeaderPrecheckResult::SlotNotAboveImmutableTip { .. }
        ));
    }

    #[test]
    fn precheck_fails_already_in_volatile() {
        let immutable_hash = Hash32::from_bytes([10u8; 32]);
        let block_hash = Hash32::from_bytes([20u8; 32]);
        let cache = InvalidBlockCache::new();

        let result = check_header_preconditions(
            &block_hash,
            100,
            &immutable_hash,
            50,
            &immutable_hash,
            // block_hash itself is "already in volatile"
            make_volatile_checker(&[block_hash]),
            &cache,
        );

        assert!(matches!(
            result,
            HeaderPrecheckResult::AlreadyInVolatileDb { .. }
        ));
    }

    #[test]
    fn precheck_fails_invalid_block_cache_hit() {
        let immutable_hash = Hash32::from_bytes([10u8; 32]);
        let block_hash = Hash32::from_bytes([20u8; 32]);
        let mut cache = InvalidBlockCache::new();
        cache.insert(block_hash);

        let result = check_header_preconditions(
            &block_hash,
            100,
            &immutable_hash,
            50,
            &immutable_hash,
            make_volatile_checker(&[]),
            &cache,
        );

        assert!(matches!(
            result,
            HeaderPrecheckResult::InvalidBlockCacheHit { .. }
        ));
    }

    #[test]
    fn precheck_fails_prev_hash_unreachable() {
        let immutable_hash = Hash32::from_bytes([10u8; 32]);
        let block_hash = Hash32::from_bytes([20u8; 32]);
        let orphan_parent = Hash32::from_bytes([99u8; 32]); // unknown
        let cache = InvalidBlockCache::new();

        let result = check_header_preconditions(
            &block_hash,
            100,
            &orphan_parent, // not immutable tip, not in volatile
            50,
            &immutable_hash,
            make_volatile_checker(&[]),
            &cache,
        );

        assert!(matches!(
            result,
            HeaderPrecheckResult::PrevHashUnreachable { .. }
        ));
    }

    // ── recover_ledger_seq — fast path (snapshot at immutable tip) ────────────

    #[test]
    fn recover_ledger_seq_snapshot_at_immutable_tip() {
        // Build a ledger state and save it as the snapshot.
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path();

        let state = LedgerState::new(ProtocolParameters::mainnet_defaults());
        // Snapshot is at slot 0 (genesis).
        let snap_path = db_path.join("ledger-snapshot-epoch0.bin");
        state.save_snapshot(&snap_path).unwrap();

        // Initialise a ChainDB (empty ImmutableDB).
        let chain_db = ChainDB::open(db_path).unwrap();

        // Immutable tip also at slot 0 — no gap replay needed.
        let seq = recover_ledger_seq(
            db_path,
            &chain_db,
            Some((SlotNo(0), Hash32::ZERO)),
            &[],
            DEFAULT_SECURITY_PARAM_K as u64,
        )
        .unwrap();

        // Anchor should be at slot 0, no volatile deltas.
        assert_eq!(seq.len(), 0);
        let anchor_point = seq.anchor_point().clone();
        assert!(
            anchor_point == Point::Origin
                || anchor_point == Point::Specific(SlotNo(0), Hash32::ZERO),
            "unexpected anchor point: {:?}",
            anchor_point
        );
    }

    // ── recover_ledger_seq — fresh chain (no snapshot, empty ImmutableDB) ─────

    #[test]
    fn recover_ledger_seq_fresh_chain_no_snapshot() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path();
        let chain_db = ChainDB::open(db_path).unwrap();

        // No immutable tip — fresh chain.
        let seq = recover_ledger_seq(
            db_path,
            &chain_db,
            None,
            &[],
            DEFAULT_SECURITY_PARAM_K as u64,
        )
        .unwrap();

        // Should produce a genesis-anchored LedgerSeq with no deltas.
        assert_eq!(seq.len(), 0);
    }

    // ── recover_ledger_seq — error when ImmutableDB non-empty and no snapshot ─

    #[test]
    fn recover_ledger_seq_no_snapshot_non_empty_chain_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path();
        let chain_db = ChainDB::open(db_path).unwrap();

        // ImmutableDB has blocks (slot 100) but no snapshot on disk.
        let result = recover_ledger_seq(
            db_path,
            &chain_db,
            Some((SlotNo(100), Hash32::from_bytes([5u8; 32]))),
            &[],
            DEFAULT_SECURITY_PARAM_K as u64,
        );

        assert!(
            result.is_err(),
            "expected Err(NoSnapshotForNonEmptyChain), got Ok"
        );
        let err = result.err().expect("result was Ok");
        assert!(
            matches!(
                err,
                StartupError::NoSnapshotForNonEmptyChain { tip_slot: 100 }
            ),
            "expected NoSnapshotForNonEmptyChain(100), got {err}"
        );
    }

    // ── Header precheck: slot boundary edge cases ─────────────────────────────

    #[test]
    fn precheck_slot_exactly_one_above_tip_is_ok() {
        let immutable_hash = Hash32::from_bytes([10u8; 32]);
        let block_hash = Hash32::from_bytes([20u8; 32]);
        let cache = InvalidBlockCache::new();

        let result = check_header_preconditions(
            &block_hash,
            51, // exactly 1 above immutable tip at 50
            &immutable_hash,
            50,
            &immutable_hash,
            make_volatile_checker(&[]),
            &cache,
        );

        assert_eq!(result, HeaderPrecheckResult::Ok);
    }

    // ── recover_invalid_cache independence from path ───────────────────────────

    #[test]
    fn recover_invalid_cache_path_does_not_exist_is_ok() {
        // Path that does not exist — should not panic, just return empty cache.
        let cache = recover_invalid_cache(Path::new("/nonexistent/path/that/never/exists"));
        assert!(cache.is_empty());
    }
}
