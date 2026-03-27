//! ChainDB — block storage coordinator backed by ImmutableDB + VolatileDB.
//!
//! Blocks arrive from the network into the VolatileDB (in-memory). Once a
//! block is deeper than k (the security parameter), it is flushed to the
//! ImmutableDB (append-only chunk files on disk). The VolatileDB is lost on
//! crash and re-synced from peers.
//!
//! This design matches Haskell cardano-node's storage architecture and
//! eliminates the need for snapshot-based persistence of block data.

use std::path::{Path, PathBuf};
use thiserror::Error;
use torsten_primitives::block::{Point, Tip};
use torsten_primitives::hash::{BlockHeaderHash, Hash32};
use torsten_primitives::time::{BlockNo, SlotNo};
use tracing::{debug, trace, warn};

use crate::config::ImmutableConfig;
use crate::immutable_db::ImmutableDB;
use crate::volatile_db::VolatileDB;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum ChainDBError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Block not found: {0}")]
    BlockNotFound(String),
    #[error("ImmutableDB error: {0}")]
    Immutable(#[from] crate::immutable_db::ImmutableDBError),
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// The security parameter k (number of blocks before immutability)
pub const SECURITY_PARAM_K: usize = 2160;

// ---------------------------------------------------------------------------
// ChainDB
// ---------------------------------------------------------------------------

/// Block storage coordinator: ImmutableDB (on-disk) + VolatileDB (in-memory).
///
/// Blocks are first stored in the VolatileDB. When they are deeper than k
/// blocks from the tip, they are flushed to the ImmutableDB. The ImmutableDB
/// is append-only chunk files that are inherently durable.
pub struct ChainDB {
    _path: PathBuf,
    immutable: ImmutableDB,
    volatile: VolatileDB,
    immutable_tip: Option<(SlotNo, Hash32, BlockNo)>,
    /// The highest block_no that has already been flushed to ImmutableDB.
    /// Avoids re-scanning from block 1 on every `flush_to_immutable` call.
    last_flushed_block_no: u64,
}

impl ChainDB {
    /// Open or create a ChainDB at the given path using default (in-memory) config.
    ///
    /// Opens ImmutableDB from `<path>/immutable/` for both reading existing
    /// chunk files and writing new ones. VolatileDB starts empty (re-synced
    /// from peers on restart).
    pub fn open(db_path: &Path) -> Result<Self, ChainDBError> {
        Self::open_with_config(db_path, &ImmutableConfig::default())
    }

    /// Open or create a ChainDB at the given path with the given ImmutableDB config.
    pub fn open_with_config(
        db_path: &Path,
        config: &ImmutableConfig,
    ) -> Result<Self, ChainDBError> {
        debug!(path = %db_path.display(), k = SECURITY_PARAM_K, index_type = ?config.index_type, "Opening ChainDB");
        std::fs::create_dir_all(db_path)?;

        let immutable_dir = db_path.join("immutable");
        std::fs::create_dir_all(&immutable_dir)?;

        let immutable = ImmutableDB::open_for_writing_with_config(&immutable_dir, config)?;

        let immutable_tip = if immutable.total_blocks() > 0 {
            Some((
                SlotNo(immutable.tip_slot()),
                immutable.tip_hash(),
                BlockNo(immutable.tip_block_no()),
            ))
        } else {
            None
        };

        if let Some((slot, _, _)) = immutable_tip {
            debug!(
                slot = slot.0,
                blocks = immutable.total_blocks(),
                "ChainDB: ImmutableDB tip"
            );
        }

        // Open VolatileDB with WAL for crash recovery
        let volatile_dir = db_path.join("volatile");
        let volatile = match VolatileDB::open(&volatile_dir) {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, "Failed to open VolatileDB with WAL, falling back to in-memory");
                VolatileDB::new()
            }
        };

        debug!(
            volatile_blocks = volatile.len(),
            "ChainDB opened successfully"
        );
        // Initialize last_flushed_block_no from the immutable tip so we
        // never re-scan blocks that were already persisted in a prior run.
        let last_flushed_block_no = immutable_tip
            .map(|(_, _, block_no)| block_no.0)
            .unwrap_or(0);

        Ok(ChainDB {
            _path: db_path.to_path_buf(),
            immutable,
            volatile,
            immutable_tip,
            last_flushed_block_no,
        })
    }

    /// Open with settings for bulk import (e.g. Mithril snapshot).
    ///
    /// Identical to `open()` for the new architecture since ImmutableDB
    /// chunk files don't need special configuration for bulk writes.
    pub fn open_for_bulk_import(path: &Path) -> Result<Self, ChainDBError> {
        debug!(path = %path.display(), "Opening ChainDB for bulk import");
        Self::open(path)
    }

    // -- Writes -------------------------------------------------------------

    /// Store a new block (goes to VolatileDB).
    pub fn add_block(
        &mut self,
        hash: BlockHeaderHash,
        slot: SlotNo,
        block_no: BlockNo,
        prev_hash: BlockHeaderHash,
        cbor: Vec<u8>,
    ) -> Result<(), ChainDBError> {
        trace!(
            hash = %hash.to_hex(),
            slot = slot.0,
            block_no = block_no.0,
            "ChainDB: adding block to volatile"
        );

        if self.has_block(&hash) {
            trace!(hash = %hash.to_hex(), "ChainDB: block already exists, skipping");
            return Ok(());
        }

        self.volatile
            .add_block(hash, slot.0, block_no.0, prev_hash, cbor);
        Ok(())
    }

    /// Store multiple blocks in a batch (all go to VolatileDB).
    pub fn add_blocks_batch(
        &mut self,
        blocks: Vec<(BlockHeaderHash, SlotNo, BlockNo, BlockHeaderHash, Vec<u8>)>,
    ) -> Result<(), ChainDBError> {
        if blocks.is_empty() {
            return Ok(());
        }

        for (hash, slot, block_no, prev_hash, cbor) in blocks {
            if self.has_block(&hash) {
                trace!(hash = %hash.to_hex(), slot = slot.0, "ChainDB: block already exists, skipping");
                continue;
            }
            self.volatile
                .add_block(hash, slot.0, block_no.0, prev_hash, cbor);
        }
        Ok(())
    }

    /// Store multiple blocks for bulk import (directly to ImmutableDB).
    /// Used by Mithril import where blocks are known to be unique and ordered.
    pub fn put_blocks_batch(
        &mut self,
        blocks: &[(SlotNo, &BlockHeaderHash, BlockNo, &[u8])],
    ) -> Result<(), ChainDBError> {
        if blocks.is_empty() {
            return Ok(());
        }

        for &(slot, hash, block_no, cbor) in blocks {
            self.immutable
                .append_block(slot.0, block_no.0, hash, cbor)?;
            self.immutable_tip = Some((slot, *hash, block_no));
        }
        Ok(())
    }

    // -- Reads --------------------------------------------------------------

    /// Get block CBOR by hash. Checks VolatileDB first, then ImmutableDB.
    pub fn get_block(&self, hash: &BlockHeaderHash) -> Result<Option<Vec<u8>>, ChainDBError> {
        // Check volatile first (recent blocks)
        if let Some(cbor) = self.volatile.get_block_cbor(hash) {
            return Ok(Some(cbor.to_vec()));
        }
        // Fall back to immutable (historical blocks)
        if let Some(cbor) = self.immutable.get_block(hash) {
            return Ok(Some(cbor));
        }
        Ok(None)
    }

    /// Number of blocks currently in the volatile (in-memory) database.
    ///
    /// This includes blocks that are GC-pending after a non-destructive rollback —
    /// they remain in the store until the GC delay elapses. Use
    /// `volatile_selected_chain_count()` to count only the canonical-chain window.
    pub fn volatile_block_count(&self) -> usize {
        self.volatile.len()
    }

    /// Number of blocks on the active selected chain in the volatile database.
    ///
    /// Unlike `volatile_block_count()`, this excludes GC-pending orphans that were
    /// displaced by a rollback. Returns 0 when the volatile chain is empty (e.g.,
    /// after rolling back past all volatile blocks to an immutable anchor point).
    pub fn volatile_selected_chain_count(&self) -> usize {
        self.volatile.selected_chain_len()
    }

    /// Clear all volatile blocks. Used when the volatile DB has blocks from
    /// a fork that no longer connects to the ledger tip (e.g., after crash
    /// or restart with a different chain).
    pub fn clear_volatile(&mut self) {
        self.volatile.clear();
    }

    /// Get the current chain tip (higher of volatile and immutable).
    pub fn get_tip(&self) -> Tip {
        let vol_tip = self.volatile.get_tip().map(|(slot, hash, block_no)| Tip {
            point: Point::Specific(SlotNo(slot), hash),
            block_number: BlockNo(block_no),
        });

        let imm_tip = self.immutable_tip.map(|(slot, hash, block_no)| Tip {
            point: Point::Specific(slot, hash),
            block_number: block_no,
        });

        match (vol_tip, imm_tip) {
            (Some(v), Some(i)) => {
                if v.point.slot().unwrap_or(SlotNo(0)) >= i.point.slot().unwrap_or(SlotNo(0)) {
                    v
                } else {
                    i
                }
            }
            (Some(t), None) | (None, Some(t)) => t,
            (None, None) => Tip::origin(),
        }
    }

    /// Get the tip info (slot, hash, block_no) if available.
    pub fn get_tip_info(&self) -> Option<(SlotNo, BlockHeaderHash, BlockNo)> {
        let vol = self
            .volatile
            .get_tip()
            .map(|(s, h, b)| (SlotNo(s), h, BlockNo(b)));
        let imm = self.immutable_tip;

        match (vol, imm) {
            (Some(v), Some(i)) => {
                if v.0 >= i.0 {
                    Some(v)
                } else {
                    Some(i)
                }
            }
            (Some(v), None) => Some(v),
            (None, Some(i)) => Some(i),
            (None, None) => None,
        }
    }

    /// Get the ImmutableDB tip as a `Tip`.
    ///
    /// Returns `Tip::origin()` when the ImmutableDB is empty (no blocks have
    /// been finalised yet).  Used by the `ChainFragment` initialisation at
    /// startup to establish the anchor point of the volatile chain window.
    pub fn get_immutable_tip(&self) -> Tip {
        match self.immutable_tip {
            Some((slot, hash, block_no)) => Tip {
                point: Point::Specific(slot, hash),
                block_number: block_no,
            },
            None => Tip::origin(),
        }
    }

    /// Return minimal `BlockHeader` stubs for the current volatile selected
    /// chain, ordered oldest → newest (same order as the selected chain).
    ///
    /// Only the fields required by `ChainFragment` are populated:
    /// - `header_hash` — the block hash
    /// - `prev_hash`   — the predecessor hash
    /// - `slot`        — absolute slot number
    /// - `block_number`— height
    ///
    /// All other `BlockHeader` fields (keys, VRF output, KES sig, etc.) are
    /// zeroed.  Callers that need the full header should fetch it from
    /// VolatileDB via `get_block`.
    pub fn get_volatile_chain_headers(&self) -> Vec<torsten_primitives::block::BlockHeader> {
        // Walk the selected chain from oldest to newest, building header stubs
        // from the VolatileBlock metadata (no CBOR decode needed).
        self.volatile
            .selected_chain_entries()
            .into_iter()
            .map(|(hash, slot, block_no, prev_hash)| {
                torsten_primitives::block::BlockHeader {
                    header_hash: hash,
                    prev_hash,
                    slot: torsten_primitives::time::SlotNo(slot),
                    block_number: torsten_primitives::time::BlockNo(block_no),
                    // Remaining fields are not needed for ChainFragment and
                    // are zeroed / empty.
                    issuer_vkey: Vec::new(),
                    vrf_vkey: Vec::new(),
                    vrf_result: torsten_primitives::block::VrfOutput {
                        output: Vec::new(),
                        proof: Vec::new(),
                    },
                    epoch_nonce: torsten_primitives::hash::Hash32::ZERO,
                    body_size: 0,
                    body_hash: torsten_primitives::hash::Hash32::ZERO,
                    operational_cert: torsten_primitives::block::OperationalCert {
                        hot_vkey: Vec::new(),
                        sequence_number: 0,
                        kes_period: 0,
                        sigma: Vec::new(),
                    },
                    protocol_version: torsten_primitives::block::ProtocolVersion {
                        major: 0,
                        minor: 0,
                    },
                    kes_signature: Vec::new(),
                    nonce_vrf_output: Vec::new(),
                }
            })
            .collect()
    }

    /// Check if a block exists by hash.
    pub fn has_block(&self, hash: &BlockHeaderHash) -> bool {
        self.volatile.has_block(hash) || self.immutable.has_block(hash)
    }

    /// Get block CBOR by block number.
    pub fn get_block_by_number(
        &self,
        block_no: BlockNo,
    ) -> Result<Option<(SlotNo, BlockHeaderHash, Vec<u8>)>, ChainDBError> {
        // Check volatile first
        if let Some((slot, hash, cbor)) = self.volatile.get_block_by_number(block_no.0) {
            return Ok(Some((SlotNo(slot), hash, cbor.to_vec())));
        }
        // ImmutableDB doesn't have a block_no index, so we can't look up by number there.
        // This is only used for LSM replay which won't be needed with the new architecture.
        Ok(None)
    }

    /// Get blocks in a slot range `[from, to]` inclusive, in slot order.
    pub fn get_blocks_in_slot_range(
        &self,
        from_slot: SlotNo,
        to_slot: SlotNo,
    ) -> Result<Vec<Vec<u8>>, ChainDBError> {
        let mut blocks = Vec::new();

        // Get from ImmutableDB first (historical blocks)
        blocks.extend(
            self.immutable
                .get_blocks_in_slot_range(from_slot.0, to_slot.0),
        );

        // Get from VolatileDB
        for (_hash, cbor) in self
            .volatile
            .get_blocks_in_slot_range(from_slot.0, to_slot.0)
        {
            blocks.push(cbor.to_vec());
        }

        Ok(blocks)
    }

    /// Get the first block strictly after `after_slot`.
    pub fn get_next_block_after_slot(
        &self,
        after_slot: SlotNo,
    ) -> Result<Option<(SlotNo, BlockHeaderHash, Vec<u8>)>, ChainDBError> {
        let imm_result = self
            .immutable
            .get_next_block_after_slot(after_slot.0)
            .map(|(s, h, cbor)| (SlotNo(s), h, cbor));

        let vol_result = self
            .volatile
            .get_next_block_after_slot(after_slot.0)
            .map(|(s, h, cbor)| (SlotNo(s), h, cbor.to_vec()));

        // Return whichever has the lower (earlier) slot
        match (imm_result, vol_result) {
            (Some((is, ih, ic)), Some((vs, vh, vc))) => {
                if is <= vs {
                    Ok(Some((is, ih, ic)))
                } else {
                    Ok(Some((vs, vh, vc)))
                }
            }
            (Some(r), None) | (None, Some(r)) => Ok(Some(r)),
            (None, None) => Ok(None),
        }
    }

    /// Get the first block at or after a given slot (inclusive `>=`).
    ///
    /// Checks both ImmutableDB and VolatileDB, returning the block with the
    /// lowest slot that is `>= slot`.
    pub fn get_block_at_or_after_slot(
        &self,
        slot: SlotNo,
    ) -> Result<Option<(SlotNo, BlockHeaderHash, Vec<u8>)>, ChainDBError> {
        let imm_result = self
            .immutable
            .get_block_at_or_after_slot(slot.0)
            .map(|(s, h, cbor)| (SlotNo(s), h, cbor));

        let vol_result = self
            .volatile
            .get_block_at_or_after_slot(slot.0)
            .map(|(s, h, cbor)| (SlotNo(s), h, cbor.to_vec()));

        match (imm_result, vol_result) {
            (Some((is, ih, ic)), Some((vs, vh, vc))) => {
                if is <= vs {
                    Ok(Some((is, ih, ic)))
                } else {
                    Ok(Some((vs, vh, vc)))
                }
            }
            (Some(r), None) | (None, Some(r)) => Ok(Some(r)),
            (None, None) => Ok(None),
        }
    }

    // -- Chain history ------------------------------------------------------

    /// Walk backwards from the volatile tip through `prev_hash` links,
    /// returning up to `max_count` [`Point`]s representing the recent chain
    /// history.
    ///
    /// This is used for ChainSync intersection negotiation so the peer has
    /// enough historical points to find a common ancestor — even when the
    /// local tip is a freshly-forged block the peer hasn't seen yet.
    ///
    /// The returned list starts with the volatile tip (most recent) and
    /// walks backwards.  Immutable historical points (sampled from older
    /// chunk secondary indexes) are appended so that even if the immutable
    /// tip was an orphaned forged block flushed on shutdown, older canonical
    /// blocks are still offered to the peer.
    pub fn get_chain_points(&self, max_count: usize) -> Vec<Point> {
        let mut points = Vec::new();

        // Walk backwards through volatile blocks via prev_hash links.
        if let Some((_slot, tip_hash, _block_no)) = self.volatile.get_tip() {
            let mut current_hash = tip_hash;
            while points.len() < max_count {
                if let Some(block) = self.volatile.get_block(&current_hash) {
                    points.push(Point::Specific(SlotNo(block.slot), current_hash));
                    if block.prev_hash == Hash32::ZERO {
                        break; // genesis or legacy WAL entry
                    }
                    current_hash = block.prev_hash;
                } else {
                    break; // reached immutable boundary or missing block
                }
            }
        }

        // Include the in-memory immutable tip (covers the active/unflushed
        // chunk whose secondary index hasn't been written to disk yet).
        if let Some((slot, hash, _block_no)) = self.immutable_tip {
            if !points.iter().any(|p| p.hash() == Some(&hash)) {
                points.push(Point::Specific(slot, hash));
            }
        }

        // Also sample older finalized chunks' last entries in reverse
        // order. This is critical when the immutable tip itself is an
        // orphaned block (e.g. a forged block flushed via
        // flush_all_to_immutable on graceful shutdown): older canonical
        // blocks from earlier chunks give the peer a valid ancestor.
        let remaining = max_count.saturating_sub(points.len());
        if remaining > 0 {
            for (slot, hash) in self.immutable.get_historical_points(remaining) {
                if !points.iter().any(|p| p.hash() == Some(&hash)) {
                    points.push(Point::Specific(SlotNo(slot), hash));
                }
            }
        }

        points
    }

    // -- Rollback -----------------------------------------------------------

    /// Take a rollback snapshot. No-op in the new architecture — rollback
    /// is handled by the VolatileDB directly.
    pub fn take_rollback_snapshot(&mut self) {
        // No-op: VolatileDB handles rollback without snapshots
    }

    /// Non-destructive rollback: truncate the selected chain to a given point.
    /// Blocks from the old chain remain in volatile for delayed GC.
    pub fn rollback_to_point(
        &mut self,
        point: &Point,
    ) -> Result<Vec<BlockHeaderHash>, ChainDBError> {
        warn!(point = ?point, "ChainDB: rollback (non-destructive)");

        let target_slot = point.slot().map(|s| s.0).unwrap_or(0);
        let target_hash = point.hash().copied();

        if let Some((_, tip_hash, _)) = self.volatile.get_tip() {
            if let Some(th) = target_hash {
                if tip_hash == th {
                    return Ok(vec![]);
                }
            }
        }

        let removed = self
            .volatile
            .rollback_to_point(target_slot, target_hash.as_ref());

        debug!(
            blocks_removed = removed.len(),
            total_volatile = self.volatile.len(),
            "ChainDB: rollback complete (fork blocks retained)"
        );

        Ok(removed)
    }

    /// Switch the selected chain to a new tip.
    pub fn switch_to_fork(
        &mut self,
        new_tip_hash: &BlockHeaderHash,
    ) -> Option<crate::volatile_db::SwitchPlan> {
        self.volatile.switch_chain(new_tip_hash)
    }

    /// GC orphaned fork blocks from volatile. Returns count removed.
    pub fn gc_volatile(&mut self) -> usize {
        self.volatile.gc_orphaned_blocks()
    }

    // -- Flush / Lifecycle --------------------------------------------------

    /// Remove a single block from the VolatileDB by hash.
    ///
    /// Used by [`crate::background::GcScheduler`] to remove blocks after
    /// their GC delay has elapsed. Hash-exact removal avoids accidentally
    /// deleting EBBs that share a slot with the next epoch's first block.
    pub fn remove_volatile_block(&mut self, hash: &Hash32) {
        self.volatile.remove_block(hash);
    }

    /// Flush finalized blocks from VolatileDB to ImmutableDB.
    ///
    /// Blocks with block_no <= (tip_block_no - k) are considered finalized.
    /// Call this after each batch of blocks is processed.
    ///
    /// Unlike the old block_no scan, this walks the canonical `selected_chain`
    /// fragment oldest-to-newest — exactly as Haskell cardano-node walks its
    /// `AnchoredFragment`.  Fork blocks at the same block_no are never touched:
    /// only the block on the selected chain is flushed, and only that block is
    /// removed from the VolatileDB (via hash, not by slot).
    pub fn flush_to_immutable(&mut self) -> Result<u64, ChainDBError> {
        let vol_tip = match self.volatile.get_tip() {
            Some(t) => t,
            None => return Ok(0),
        };

        let tip_block_no = vol_tip.2;
        if tip_block_no <= SECURITY_PARAM_K as u64 {
            return Ok(0); // Not enough blocks to finalize anything
        }

        let finalize_up_to_block_no = tip_block_no - SECURITY_PARAM_K as u64;
        let start_block_no = self.last_flushed_block_no + 1;

        // Walk the canonical selected-chain fragment (oldest → newest) and
        // collect only those blocks whose block_no falls in [start, finalize].
        // `selected_chain_entries` returns (hash, slot, block_no, prev_hash).
        let mut to_finalize: Vec<(u64, Hash32, u64, Vec<u8>)> = Vec::new(); // (slot, hash, block_no, cbor)
        for (hash, slot, block_no, _prev_hash) in self.volatile.selected_chain_entries() {
            if block_no < start_block_no {
                // Already flushed in a prior call — skip cheaply.
                continue;
            }
            if block_no > finalize_up_to_block_no {
                // Within the rollback window — stop here; the chain is ordered.
                break;
            }
            // Skip blocks already present in ImmutableDB (e.g. Mithril import).
            if self.immutable.has_block(&hash) {
                continue;
            }
            if let Some(cbor) = self.volatile.get_block_cbor(&hash) {
                to_finalize.push((slot, hash, block_no, cbor.to_vec()));
            }
        }

        if to_finalize.is_empty() {
            return Ok(0);
        }

        let count = to_finalize.len() as u64;

        // Append canonical-chain blocks to ImmutableDB in oldest-first order.
        for (slot, hash, block_no, cbor) in &to_finalize {
            self.immutable.append_block(*slot, *block_no, hash, cbor)?;
        }

        // Update immutable tip and last-flushed tracking.
        if let Some((slot, hash, block_no, _)) = to_finalize.last() {
            self.immutable_tip = Some((SlotNo(*slot), *hash, BlockNo(*block_no)));
            self.last_flushed_block_no = *block_no;
        }

        // Remove only the flushed canonical blocks from VolatileDB.
        // We do NOT remove by slot because fork blocks at the same slots must
        // survive until they are naturally garbage-collected.
        let flushed_hashes: Vec<Hash32> = to_finalize.iter().map(|(_, h, _, _)| *h).collect();
        self.volatile.remove_blocks_by_hashes(&flushed_hashes);

        debug!(
            flushed = count,
            immutable_tip_slot = self.immutable_tip.map(|(s, _, _)| s.0).unwrap_or(0),
            volatile_remaining = self.volatile.len(),
            "ChainDB: flushed canonical-chain blocks to immutable"
        );

        Ok(count)
    }

    /// Flush up to `max_blocks` finalized blocks from VolatileDB to ImmutableDB.
    ///
    /// Returns the number of blocks actually flushed. The caller should loop
    /// until this returns 0 to flush all pending blocks. This batch variant
    /// allows the caller to release the ChainDB write lock between batches,
    /// preventing read starvation for concurrent tasks (e.g. ChainSync server).
    pub fn flush_to_immutable_batch(&mut self, max_blocks: u64) -> Result<u64, ChainDBError> {
        let vol_tip = match self.volatile.get_tip() {
            Some(t) => t,
            None => return Ok(0),
        };

        let tip_block_no = vol_tip.2;
        if tip_block_no <= SECURITY_PARAM_K as u64 {
            return Ok(0);
        }

        let finalize_up_to_block_no = tip_block_no - SECURITY_PARAM_K as u64;
        let start_block_no = self.last_flushed_block_no + 1;

        let mut to_finalize: Vec<(u64, Hash32, u64, Vec<u8>)> = Vec::new();
        for (hash, slot, block_no, _prev_hash) in self.volatile.selected_chain_entries() {
            if block_no < start_block_no {
                continue;
            }
            if block_no > finalize_up_to_block_no {
                break;
            }
            if self.immutable.has_block(&hash) {
                continue;
            }
            if let Some(cbor) = self.volatile.get_block_cbor(&hash) {
                to_finalize.push((slot, hash, block_no, cbor.to_vec()));
            }
            if to_finalize.len() as u64 >= max_blocks {
                break;
            }
        }

        if to_finalize.is_empty() {
            return Ok(0);
        }

        let count = to_finalize.len() as u64;

        for (slot, hash, block_no, cbor) in &to_finalize {
            self.immutable.append_block(*slot, *block_no, hash, cbor)?;
        }

        if let Some((slot, hash, block_no, _)) = to_finalize.last() {
            self.immutable_tip = Some((SlotNo(*slot), *hash, BlockNo(*block_no)));
            self.last_flushed_block_no = *block_no;
        }

        let flushed_hashes: Vec<Hash32> = to_finalize.iter().map(|(_, h, _, _)| *h).collect();
        self.volatile.remove_blocks_by_hashes(&flushed_hashes);

        debug!(
            flushed = count,
            immutable_tip_slot = self.immutable_tip.map(|(s, _, _)| s.0).unwrap_or(0),
            volatile_remaining = self.volatile.len(),
            "ChainDB: flushed batch to immutable"
        );

        Ok(count)
    }

    /// Batch variant of `flush_to_immutable_loe` — flushes up to `max_blocks`.
    pub fn flush_to_immutable_loe_batch(
        &mut self,
        loe_slot: u64,
        max_blocks: u64,
    ) -> Result<u64, ChainDBError> {
        let vol_tip = match self.volatile.get_tip() {
            Some(t) => t,
            None => return Ok(0),
        };

        let tip_block_no = vol_tip.2;
        if tip_block_no <= SECURITY_PARAM_K as u64 {
            return Ok(0);
        }

        let finalize_up_to_block_no = tip_block_no - SECURITY_PARAM_K as u64;
        let start_block_no = self.last_flushed_block_no + 1;

        let mut to_finalize: Vec<(u64, Hash32, u64, Vec<u8>)> = Vec::new();
        for (hash, slot, block_no, _prev_hash) in self.volatile.selected_chain_entries() {
            if block_no < start_block_no {
                continue;
            }
            if block_no > finalize_up_to_block_no {
                break;
            }
            if slot > loe_slot {
                continue;
            }
            if self.immutable.has_block(&hash) {
                continue;
            }
            if let Some(cbor) = self.volatile.get_block_cbor(&hash) {
                to_finalize.push((slot, hash, block_no, cbor.to_vec()));
            }
            if to_finalize.len() as u64 >= max_blocks {
                break;
            }
        }

        if to_finalize.is_empty() {
            return Ok(0);
        }

        let count = to_finalize.len() as u64;

        for (slot, hash, block_no, cbor) in &to_finalize {
            self.immutable.append_block(*slot, *block_no, hash, cbor)?;
        }

        if let Some((slot, hash, block_no, _)) = to_finalize.last() {
            self.immutable_tip = Some((SlotNo(*slot), *hash, BlockNo(*block_no)));
            self.last_flushed_block_no = *block_no;
        }

        let flushed_hashes: Vec<Hash32> = to_finalize.iter().map(|(_, h, _, _)| *h).collect();
        self.volatile.remove_blocks_by_hashes(&flushed_hashes);

        debug!(
            flushed = count,
            loe_slot,
            immutable_tip_slot = self.immutable_tip.map(|(s, _, _)| s.0).unwrap_or(0),
            volatile_remaining = self.volatile.len(),
            "ChainDB: flushed LoE batch to immutable"
        );

        Ok(count)
    }

    /// Compact the volatile WAL after flushing blocks to immutable storage.
    ///
    /// Rewrites the volatile WAL to contain only the blocks that remain in
    /// the VolatileDB. This is called automatically by `remove_blocks_by_hashes`
    /// inside `flush_to_immutable`, but can also be called explicitly (e.g.
    /// after rollback) to reclaim disk space.
    ///
    /// This is a no-op when the VolatileDB has no WAL configured.
    pub fn compact_volatile_wal(&mut self) {
        self.volatile.compact_wal();
    }

    /// Flush finalized blocks up to a maximum slot (LoE-gated flush).
    ///
    /// Like `flush_to_immutable`, but also enforces a slot ceiling so that
    /// the immutable tip never advances past `loe_slot`. This is the
    /// Ouroboros Genesis Limit on Eagerness (LoE): during PreSyncing and
    /// Syncing states, the immutable tip must not advance past the common
    /// prefix of all candidate chains (the minimum peer tip slot).
    ///
    /// Blocks that are deep enough to be finalized (> k from volatile tip)
    /// but have a slot > `loe_slot` are left in the VolatileDB until the
    /// LoE constraint is lifted (when GSM transitions to CaughtUp).
    pub fn flush_to_immutable_loe(&mut self, loe_slot: u64) -> Result<u64, ChainDBError> {
        let vol_tip = match self.volatile.get_tip() {
            Some(t) => t,
            None => return Ok(0),
        };

        let tip_block_no = vol_tip.2;
        if tip_block_no <= SECURITY_PARAM_K as u64 {
            return Ok(0);
        }

        let finalize_up_to_block_no = tip_block_no - SECURITY_PARAM_K as u64;
        let start_block_no = self.last_flushed_block_no + 1;

        // Walk the canonical chain fragment (oldest → newest), applying both
        // the k-depth cutoff and the LoE slot ceiling.  The LoE ceiling is an
        // *additional* constraint: even if a block is deep enough to finalize
        // by block_no, we skip it if its slot exceeds `loe_slot`.
        //
        // Unlike the old block_no scan this never touches fork blocks.
        let mut to_finalize: Vec<(u64, Hash32, u64, Vec<u8>)> = Vec::new();
        for (hash, slot, block_no, _prev_hash) in self.volatile.selected_chain_entries() {
            if block_no < start_block_no {
                continue;
            }
            if block_no > finalize_up_to_block_no {
                // Past the rollback window; further blocks won't qualify.
                break;
            }
            // LoE: hold back blocks whose slot exceeds the common-prefix ceiling.
            if slot > loe_slot {
                continue;
            }
            if self.immutable.has_block(&hash) {
                continue;
            }
            if let Some(cbor) = self.volatile.get_block_cbor(&hash) {
                to_finalize.push((slot, hash, block_no, cbor.to_vec()));
            }
        }

        if to_finalize.is_empty() {
            return Ok(0);
        }

        let count = to_finalize.len() as u64;

        for (slot, hash, block_no, cbor) in &to_finalize {
            self.immutable.append_block(*slot, *block_no, hash, cbor)?;
        }

        // Update immutable tip and last-flushed tracking.
        if let Some((slot, hash, block_no, _)) = to_finalize.last() {
            self.immutable_tip = Some((SlotNo(*slot), *hash, BlockNo(*block_no)));
            self.last_flushed_block_no = *block_no;
        }

        // Remove only the flushed canonical blocks; leave fork blocks intact.
        let flushed_hashes: Vec<Hash32> = to_finalize.iter().map(|(_, h, _, _)| *h).collect();
        self.volatile.remove_blocks_by_hashes(&flushed_hashes);

        debug!(
            flushed = count,
            loe_slot,
            immutable_tip_slot = self.immutable_tip.map(|(s, _, _)| s.0).unwrap_or(0),
            volatile_remaining = self.volatile.len(),
            "ChainDB: flushed canonical-chain blocks to immutable (LoE-gated)"
        );

        Ok(count)
    }

    /// Flush volatile blocks to ImmutableDB during graceful shutdown.
    ///
    /// Unlike the old implementation that flushed ALL volatile blocks (violating
    /// the k-depth invariant), this now respects the same k-depth rule as
    /// `flush_to_immutable`.  Only blocks that are k-deep on the canonical
    /// chain are moved to ImmutableDB.  The last k blocks remain in volatile
    /// and will be re-fetched from peers on restart.
    ///
    /// This prevents fork blocks within the rollback window from being
    /// permanently committed to the append-only ImmutableDB, which would make
    /// the node unable to recover from forks after restart.
    pub fn flush_all_to_immutable(&mut self) -> Result<u64, ChainDBError> {
        // Delegate to the k-depth-safe flush — same behavior as normal operation.
        // This may leave some volatile blocks behind, but that's correct: those
        // blocks are within the rollback window and should be re-validated.
        let flushed = self.flush_to_immutable()?;

        debug!(
            flushed,
            immutable_tip_slot = self.immutable_tip.map(|(s, _, _)| s.0).unwrap_or(0),
            volatile_remaining = self.volatile.len(),
            "ChainDB: flushed k-deep blocks to immutable (shutdown)"
        );

        Ok(flushed)
    }

    /// Get historical points from older ImmutableDB chunks (canonical blocks).
    ///
    /// Used for fork recovery: when the immutable_tip is contaminated (orphan
    /// fork block), these deeper points from older chunks provide valid
    /// ancestors for ChainSync intersection negotiation.
    pub fn get_immutable_historical_points(&self, max_count: usize) -> Vec<(u64, Hash32)> {
        self.immutable.get_historical_points(max_count)
    }

    /// Finalize the current ImmutableDB chunk and start a new one.
    /// Call this at epoch boundaries.
    pub fn finalize_immutable_chunk(&mut self) -> Result<(), ChainDBError> {
        self.immutable.finalize_chunk()?;
        Ok(())
    }

    /// Persist ImmutableDB to disk (flush active chunk's secondary index).
    /// Call this on shutdown.
    pub fn persist(&mut self) -> Result<(), ChainDBError> {
        debug!("ChainDB: persisting immutable state");
        self.immutable.flush()?;
        debug!("ChainDB: persist complete");
        Ok(())
    }

    /// Trigger compaction. No-op in the new architecture.
    pub fn compact(&mut self) {
        // No-op: ImmutableDB chunk files don't need compaction
    }

    /// Current tip slot.
    pub fn tip_slot(&self) -> SlotNo {
        let vol_slot = self.volatile.get_tip().map(|(s, _, _)| s).unwrap_or(0);
        let imm_slot = self.immutable_tip.map(|(s, _, _)| s.0).unwrap_or(0);
        SlotNo(vol_slot.max(imm_slot))
    }

    /// Whether this ChainDB has immutable chunk files.
    pub fn has_immutable(&self) -> bool {
        self.immutable.total_blocks() > 0
    }

    /// Get the ImmutableDB directory path.
    pub fn immutable_dir(&self) -> Option<&Path> {
        Some(self.immutable.dir())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_hash(n: u8) -> BlockHeaderHash {
        Hash32::from_bytes([n; 32])
    }

    fn build_chain(db: &mut ChainDB, count: u8) {
        for i in 1..=count {
            db.add_block(
                make_hash(i),
                SlotNo(i as u64),
                BlockNo(i as u64),
                make_hash(i - 1),
                format!("block{}", i).into_bytes(),
            )
            .unwrap();
        }
    }

    #[test]
    fn test_add_and_get_block() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = ChainDB::open(dir.path()).unwrap();

        let hash = make_hash(1);
        let prev = make_hash(0);

        db.add_block(hash, SlotNo(100), BlockNo(50), prev, b"block data".to_vec())
            .unwrap();

        let result = db.get_block(&hash).unwrap();
        assert_eq!(result.as_deref(), Some(b"block data".as_slice()));
    }

    #[test]
    fn test_tip_tracking() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = ChainDB::open(dir.path()).unwrap();

        assert_eq!(db.get_tip(), Tip::origin());

        db.add_block(
            make_hash(1),
            SlotNo(100),
            BlockNo(50),
            Hash32::ZERO,
            b"data".to_vec(),
        )
        .unwrap();

        let tip = db.get_tip();
        assert_eq!(tip.block_number, BlockNo(50));
        assert_eq!(tip.point.slot(), Some(SlotNo(100)));
    }

    #[test]
    fn test_has_block() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = ChainDB::open(dir.path()).unwrap();

        build_chain(&mut db, 2);

        assert!(db.has_block(&make_hash(1)));
        assert!(db.has_block(&make_hash(2)));
        assert!(!db.has_block(&make_hash(99)));
    }

    #[test]
    fn test_rollback_to_specific_point() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = ChainDB::open(dir.path()).unwrap();

        // Build chain: 0 <- 1 <- 2 <- 3 <- 4 <- 5
        build_chain(&mut db, 5);
        assert_eq!(db.get_tip().block_number, BlockNo(5));

        // Rollback to block 3
        let removed = db
            .rollback_to_point(&Point::Specific(SlotNo(3), make_hash(3)))
            .unwrap();

        assert_eq!(removed.len(), 2); // blocks 5 and 4
        assert!(removed.contains(&make_hash(4)));
        assert!(removed.contains(&make_hash(5)));

        // Non-destructive: blocks 4 and 5 still in store
        assert!(db.has_block(&make_hash(4)));
        assert!(db.has_block(&make_hash(5)));

        // Blocks 1-3 should still exist
        assert!(db.has_block(&make_hash(1)));
        assert!(db.has_block(&make_hash(2)));
        assert!(db.has_block(&make_hash(3)));

        // Tip should be block 3
        assert_eq!(db.get_tip().block_number, BlockNo(3));
    }

    #[test]
    fn test_rollback_to_current_tip() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = ChainDB::open(dir.path()).unwrap();

        build_chain(&mut db, 3);

        let removed = db
            .rollback_to_point(&Point::Specific(SlotNo(3), make_hash(3)))
            .unwrap();
        assert!(removed.is_empty());
        assert_eq!(db.get_tip().block_number, BlockNo(3));
    }

    #[test]
    fn test_rollback_to_origin() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = ChainDB::open(dir.path()).unwrap();

        build_chain(&mut db, 3);

        let removed = db.rollback_to_point(&Point::Origin).unwrap();
        assert_eq!(removed.len(), 3);
    }

    #[test]
    fn test_add_blocks_batch() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = ChainDB::open(dir.path()).unwrap();

        let batch = vec![
            (
                make_hash(1),
                SlotNo(10),
                BlockNo(1),
                make_hash(0),
                b"b1".to_vec(),
            ),
            (
                make_hash(2),
                SlotNo(20),
                BlockNo(2),
                make_hash(1),
                b"b2".to_vec(),
            ),
            (
                make_hash(3),
                SlotNo(30),
                BlockNo(3),
                make_hash(2),
                b"b3".to_vec(),
            ),
        ];

        db.add_blocks_batch(batch).unwrap();

        assert!(db.has_block(&make_hash(1)));
        assert!(db.has_block(&make_hash(2)));
        assert!(db.has_block(&make_hash(3)));
        assert_eq!(db.get_tip().block_number, BlockNo(3));
    }

    #[test]
    fn test_add_blocks_batch_skips_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = ChainDB::open(dir.path()).unwrap();

        db.add_block(
            make_hash(1),
            SlotNo(10),
            BlockNo(1),
            make_hash(0),
            b"b1".to_vec(),
        )
        .unwrap();

        // Batch includes the duplicate and a new block
        let batch = vec![
            (
                make_hash(1),
                SlotNo(10),
                BlockNo(1),
                make_hash(0),
                b"b1_dup".to_vec(),
            ),
            (
                make_hash(2),
                SlotNo(20),
                BlockNo(2),
                make_hash(1),
                b"b2".to_vec(),
            ),
        ];

        db.add_blocks_batch(batch).unwrap();

        // Original data unchanged
        assert_eq!(
            db.get_block(&make_hash(1)).unwrap().as_deref(),
            Some(b"b1".as_slice())
        );
        assert!(db.has_block(&make_hash(2)));
    }

    #[test]
    fn test_get_blocks_in_slot_range() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = ChainDB::open(dir.path()).unwrap();

        build_chain(&mut db, 5);

        let blocks = db.get_blocks_in_slot_range(SlotNo(2), SlotNo(4)).unwrap();
        assert_eq!(blocks.len(), 3);
    }

    #[test]
    fn test_get_next_block_after_slot() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = ChainDB::open(dir.path()).unwrap();

        build_chain(&mut db, 3);

        let result = db.get_next_block_after_slot(SlotNo(0)).unwrap();
        assert!(result.is_some());
        let (slot, _, _) = result.unwrap();
        assert_eq!(slot, SlotNo(1));

        let result = db.get_next_block_after_slot(SlotNo(1)).unwrap();
        assert_eq!(result.unwrap().0, SlotNo(2));

        let result = db.get_next_block_after_slot(SlotNo(3)).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_volatile_to_immutable_flush() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = ChainDB::open(dir.path()).unwrap();

        // Add k+10 blocks (enough to finalize 10)
        let total = (SECURITY_PARAM_K + 10) as u8;
        // We can't use u8 for >255, so use a loop with u64
        for i in 1..=(SECURITY_PARAM_K as u64 + 10) {
            let mut hash_bytes = [0u8; 32];
            hash_bytes[0..8].copy_from_slice(&i.to_be_bytes());
            let hash = Hash32::from_bytes(hash_bytes);
            let mut prev_bytes = [0u8; 32];
            prev_bytes[0..8].copy_from_slice(&(i - 1).to_be_bytes());
            let prev = Hash32::from_bytes(prev_bytes);

            db.add_block(
                hash,
                SlotNo(i * 10),
                BlockNo(i),
                prev,
                format!("block{i}").into_bytes(),
            )
            .unwrap();
        }

        let _ = total; // suppress unused warning

        // Flush finalized blocks
        let flushed = db.flush_to_immutable().unwrap();
        assert_eq!(flushed, 10);

        // Verify immutable blocks are still readable
        let mut hash_bytes = [0u8; 32];
        hash_bytes[0..8].copy_from_slice(&1u64.to_be_bytes());
        let first_hash = Hash32::from_bytes(hash_bytes);
        assert!(db.has_block(&first_hash));

        // Volatile should have k blocks remaining
        assert_eq!(db.volatile.len(), SECURITY_PARAM_K);
    }

    /// Verify that `flush_to_immutable_loe` respects the LoE slot ceiling.
    ///
    /// Scenario: add k+10 blocks.  Normally `flush_to_immutable` would move
    /// the first 10 blocks to immutable.  With a LoE slot that covers only
    /// the first 5 of those finalizable blocks, only those 5 should be
    /// flushed.
    #[test]
    fn test_flush_to_immutable_loe_caps_flush() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = ChainDB::open(dir.path()).unwrap();

        // Add k+10 blocks.  Each block i has slot = i * 10.
        for i in 1..=(SECURITY_PARAM_K as u64 + 10) {
            let mut hash_bytes = [0u8; 32];
            hash_bytes[0..8].copy_from_slice(&i.to_be_bytes());
            let hash = Hash32::from_bytes(hash_bytes);
            let mut prev_bytes = [0u8; 32];
            prev_bytes[0..8].copy_from_slice(&(i - 1).to_be_bytes());
            let prev = Hash32::from_bytes(prev_bytes);
            db.add_block(
                hash,
                SlotNo(i * 10),
                BlockNo(i),
                prev,
                format!("block{i}").into_bytes(),
            )
            .unwrap();
        }

        // With LoE slot = 50 only blocks 1–5 (slots 10,20,30,40,50) qualify.
        // Normally the first 10 blocks are finalizable (k+10 - k = 10).
        let flushed = db.flush_to_immutable_loe(50).unwrap();
        assert_eq!(
            flushed, 5,
            "LoE should limit flush to blocks with slot ≤ 50"
        );

        // Blocks 1–5 should now be in immutable.
        for i in 1u64..=5 {
            let mut h = [0u8; 32];
            h[0..8].copy_from_slice(&i.to_be_bytes());
            assert!(db.has_block(&Hash32::from_bytes(h)));
        }

        // Blocks 6+ are still volatile (within LoE constraint).
        assert_eq!(
            db.volatile.len(),
            SECURITY_PARAM_K + 5,
            "Blocks beyond loe_slot must remain in volatile"
        );

        // Lifting the LoE (no slot ceiling) should flush the remaining 5.
        let flushed2 = db.flush_to_immutable().unwrap();
        assert_eq!(flushed2, 5);
    }

    /// Verify that a LoE slot of 0 prevents any flush (PreSyncing state).
    #[test]
    fn test_flush_to_immutable_loe_zero_blocks_all() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = ChainDB::open(dir.path()).unwrap();

        for i in 1..=(SECURITY_PARAM_K as u64 + 10) {
            let mut hash_bytes = [0u8; 32];
            hash_bytes[0..8].copy_from_slice(&i.to_be_bytes());
            let hash = Hash32::from_bytes(hash_bytes);
            let mut prev_bytes = [0u8; 32];
            prev_bytes[0..8].copy_from_slice(&(i - 1).to_be_bytes());
            let prev = Hash32::from_bytes(prev_bytes);
            db.add_block(
                hash,
                SlotNo(i * 10),
                BlockNo(i),
                prev,
                format!("block{i}").into_bytes(),
            )
            .unwrap();
        }

        // LoE slot = 0 — no block has slot 0, so nothing should flush.
        let flushed = db.flush_to_immutable_loe(0).unwrap();
        assert_eq!(flushed, 0, "LoE=0 must block all immutable advancement");
        assert_eq!(
            db.volatile.len(),
            SECURITY_PARAM_K + 10,
            "All blocks must stay volatile when LoE=0"
        );
    }

    #[test]
    fn test_flush_all_to_immutable() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = ChainDB::open(dir.path()).unwrap();

        // Add 20 blocks (well under k, so flush_to_immutable wouldn't move them)
        for i in 1..=20u64 {
            let mut hash_bytes = [0u8; 32];
            hash_bytes[0..8].copy_from_slice(&i.to_be_bytes());
            let hash = Hash32::from_bytes(hash_bytes);
            let mut prev_bytes = [0u8; 32];
            prev_bytes[0..8].copy_from_slice(&(i - 1).to_be_bytes());
            let prev = Hash32::from_bytes(prev_bytes);
            db.add_block(
                hash,
                SlotNo(i * 10),
                BlockNo(i),
                prev,
                format!("block{i}").into_bytes(),
            )
            .unwrap();
        }

        // Normal flush should move nothing (tip < k)
        assert_eq!(db.flush_to_immutable().unwrap(), 0);
        assert_eq!(db.volatile.len(), 20);

        // flush_all now delegates to flush_to_immutable which respects k-depth.
        // With only 20 blocks (well under k=2160), nothing should be flushed.
        // This matches Haskell behavior: shutdown never flushes beyond k.
        let flushed = db.flush_all_to_immutable().unwrap();
        assert_eq!(flushed, 0);
        assert_eq!(db.volatile.len(), 20);

        // All blocks should still be readable (from volatile, not immutable)
        for i in 1..=20u64 {
            let mut hash_bytes = [0u8; 32];
            hash_bytes[0..8].copy_from_slice(&i.to_be_bytes());
            let hash = Hash32::from_bytes(hash_bytes);
            assert!(db.has_block(&hash));
        }

        // Tip should reflect the last block
        assert_eq!(db.tip_slot(), SlotNo(200));

        // Persist and re-open — blocks should survive
        db.persist().unwrap();
        let db2 = ChainDB::open(dir.path()).unwrap();
        assert_eq!(db2.tip_slot(), SlotNo(200));
        let mut hash_bytes = [0u8; 32];
        hash_bytes[0..8].copy_from_slice(&1u64.to_be_bytes());
        assert!(db2.has_block(&Hash32::from_bytes(hash_bytes)));
    }

    #[test]
    fn test_rollback_only_affects_volatile() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = ChainDB::open(dir.path()).unwrap();

        // Add blocks and flush some to immutable
        for i in 1..=(SECURITY_PARAM_K as u64 + 5) {
            let mut hash_bytes = [0u8; 32];
            hash_bytes[0..8].copy_from_slice(&i.to_be_bytes());
            let hash = Hash32::from_bytes(hash_bytes);
            let mut prev_bytes = [0u8; 32];
            prev_bytes[0..8].copy_from_slice(&(i - 1).to_be_bytes());
            let prev = Hash32::from_bytes(prev_bytes);

            db.add_block(
                hash,
                SlotNo(i * 10),
                BlockNo(i),
                prev,
                format!("block{i}").into_bytes(),
            )
            .unwrap();
        }

        db.flush_to_immutable().unwrap();

        // Immutable blocks should survive rollback
        let imm_block_no = 3u64;
        let mut hash_bytes = [0u8; 32];
        hash_bytes[0..8].copy_from_slice(&imm_block_no.to_be_bytes());
        let imm_hash = Hash32::from_bytes(hash_bytes);

        // Rollback all volatile blocks
        db.rollback_to_point(&Point::Origin).unwrap();

        // Immutable blocks should still be accessible
        assert!(db.immutable.has_block(&imm_hash));
    }

    #[test]
    fn test_get_block_searches_both_stores() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = ChainDB::open(dir.path()).unwrap();

        // Put a block directly into immutable via bulk import
        let imm_hash = make_hash(1);
        db.put_blocks_batch(&[(SlotNo(10), &imm_hash, BlockNo(1), b"immutable_block")])
            .unwrap();

        // Put a block in volatile
        let vol_hash = make_hash(2);
        db.add_block(
            vol_hash,
            SlotNo(20),
            BlockNo(2),
            imm_hash,
            b"volatile_block".to_vec(),
        )
        .unwrap();

        // Both should be found
        assert_eq!(
            db.get_block(&imm_hash).unwrap().as_deref(),
            Some(b"immutable_block".as_slice())
        );
        assert_eq!(
            db.get_block(&vol_hash).unwrap().as_deref(),
            Some(b"volatile_block".as_slice())
        );
    }

    #[test]
    fn test_restart_recovery() {
        let dir = tempfile::tempdir().unwrap();

        // Add blocks, flush to immutable, persist
        {
            let mut db = ChainDB::open(dir.path()).unwrap();
            let hash = make_hash(1);
            db.put_blocks_batch(&[(SlotNo(100), &hash, BlockNo(1), b"block1")])
                .unwrap();
            db.persist().unwrap();
        }

        // Re-open: immutable blocks should be preserved, volatile is empty
        let db = ChainDB::open(dir.path()).unwrap();
        assert!(db.has_block(&make_hash(1)));
        assert_eq!(db.volatile.len(), 0);
        assert_eq!(db.tip_slot(), SlotNo(100));
    }

    #[test]
    fn test_slot_collision_returns_correct_block() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = ChainDB::open(dir.path()).unwrap();

        let hash_a = make_hash(1);
        let hash_b = make_hash(2);
        let same_slot = SlotNo(100);

        db.add_block(
            hash_a,
            same_slot,
            BlockNo(50),
            Hash32::ZERO,
            b"block_A".to_vec(),
        )
        .unwrap();
        db.add_block(
            hash_b,
            same_slot,
            BlockNo(51),
            Hash32::ZERO,
            b"block_B".to_vec(),
        )
        .unwrap();

        let a_data = db.get_block(&hash_a).unwrap().unwrap();
        assert_eq!(a_data, b"block_A");

        let b_data = db.get_block(&hash_b).unwrap().unwrap();
        assert_eq!(b_data, b"block_B");
    }

    #[test]
    fn test_bulk_import() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = ChainDB::open_for_bulk_import(dir.path()).unwrap();

        let hash1 = make_hash(1);
        let hash2 = make_hash(2);

        let blocks = vec![
            (SlotNo(100), &hash1, BlockNo(10), b"block1".as_slice()),
            (SlotNo(200), &hash2, BlockNo(20), b"block2".as_slice()),
        ];

        db.put_blocks_batch(&blocks).unwrap();

        assert!(db.has_block(&hash1));
        assert!(db.has_block(&hash2));
        assert_eq!(db.tip_slot(), SlotNo(200));
    }

    #[test]
    fn test_persist_and_recover() {
        let dir = tempfile::tempdir().unwrap();
        let hash = make_hash(42);

        {
            let mut db = ChainDB::open(dir.path()).unwrap();
            db.put_blocks_batch(&[(SlotNo(500), &hash, BlockNo(100), b"block")])
                .unwrap();
            db.persist().unwrap();
        }

        // Re-open and verify
        let db = ChainDB::open(dir.path()).unwrap();
        assert_eq!(db.tip_slot(), SlotNo(500));
        let (slot, h, block_no) = db.get_tip_info().unwrap();
        assert_eq!(slot, SlotNo(500));
        assert_eq!(h, hash);
        assert_eq!(block_no, BlockNo(100));
    }

    #[test]
    fn test_open_with_mmap_config() {
        let dir = tempfile::tempdir().unwrap();
        let config = ImmutableConfig {
            index_type: crate::config::BlockIndexType::Mmap,
            mmap_load_factor: 0.7,
            mmap_initial_capacity: 0,
        };
        let mut db = ChainDB::open_with_config(dir.path(), &config).unwrap();

        let hash = make_hash(1);
        let prev = make_hash(0);
        db.add_block(hash, SlotNo(100), BlockNo(50), prev, b"data".to_vec())
            .unwrap();

        assert!(db.has_block(&hash));
        assert_eq!(
            db.get_block(&hash).unwrap().as_deref(),
            Some(b"data".as_slice())
        );
    }

    #[test]
    fn test_open_with_config_persist_and_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let config = ImmutableConfig {
            index_type: crate::config::BlockIndexType::Mmap,
            mmap_load_factor: 0.7,
            mmap_initial_capacity: 0,
        };
        let hash = make_hash(1);

        {
            let mut db = ChainDB::open_with_config(dir.path(), &config).unwrap();
            db.put_blocks_batch(&[(SlotNo(100), &hash, BlockNo(1), b"block1")])
                .unwrap();
            db.persist().unwrap();
        }

        // Reopen with mmap config — should find the block
        let db = ChainDB::open_with_config(dir.path(), &config).unwrap();
        assert!(db.has_block(&hash));
        assert_eq!(db.tip_slot(), SlotNo(100));
    }

    #[test]
    fn test_default_config_backward_compatible() {
        // open() and open_with_config(default) should behave identically
        let dir = tempfile::tempdir().unwrap();

        let mut db = ChainDB::open(dir.path()).unwrap();
        db.add_block(
            make_hash(1),
            SlotNo(10),
            BlockNo(1),
            make_hash(0),
            b"b1".to_vec(),
        )
        .unwrap();

        assert!(db.has_block(&make_hash(1)));
        assert_eq!(db.tip_slot(), SlotNo(10));
    }

    // -----------------------------------------------------------------------
    // Additional ChainDB integration tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_get_tip_returns_highest_across_stores() {
        // ImmutableDB has a block at slot 100, VolatileDB has one at slot 200.
        // get_tip should return slot 200.
        let dir = tempfile::tempdir().unwrap();
        let mut db = ChainDB::open(dir.path()).unwrap();

        // Put a block directly into immutable
        let imm_hash = make_hash(1);
        db.put_blocks_batch(&[(SlotNo(100), &imm_hash, BlockNo(1), b"imm_block")])
            .unwrap();

        // Put a block in volatile with higher slot
        let vol_hash = make_hash(2);
        db.add_block(
            vol_hash,
            SlotNo(200),
            BlockNo(2),
            imm_hash,
            b"vol_block".to_vec(),
        )
        .unwrap();

        let tip = db.get_tip();
        assert_eq!(tip.point.slot(), Some(SlotNo(200)));
        assert_eq!(tip.block_number, BlockNo(2));
    }

    #[test]
    fn test_get_tip_immutable_higher_than_volatile() {
        // If immutable tip is higher than volatile tip, immutable wins
        let dir = tempfile::tempdir().unwrap();
        let mut db = ChainDB::open(dir.path()).unwrap();

        let imm_hash = make_hash(1);
        db.put_blocks_batch(&[(SlotNo(500), &imm_hash, BlockNo(50), b"imm_block")])
            .unwrap();

        let vol_hash = make_hash(2);
        db.add_block(
            vol_hash,
            SlotNo(100),
            BlockNo(10),
            Hash32::ZERO,
            b"vol_block".to_vec(),
        )
        .unwrap();

        let tip = db.get_tip();
        assert_eq!(tip.point.slot(), Some(SlotNo(500)));
        assert_eq!(tip.block_number, BlockNo(50));
    }

    #[test]
    fn test_has_block_checks_both_stores() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = ChainDB::open(dir.path()).unwrap();

        let imm_hash = make_hash(10);
        db.put_blocks_batch(&[(SlotNo(100), &imm_hash, BlockNo(1), b"imm")])
            .unwrap();

        let vol_hash = make_hash(20);
        db.add_block(vol_hash, SlotNo(200), BlockNo(2), imm_hash, b"vol".to_vec())
            .unwrap();

        assert!(db.has_block(&imm_hash));
        assert!(db.has_block(&vol_hash));
        assert!(!db.has_block(&make_hash(99)));
    }

    #[test]
    fn test_volatile_block_count() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = ChainDB::open(dir.path()).unwrap();

        assert_eq!(db.volatile_block_count(), 0);

        build_chain(&mut db, 5);
        assert_eq!(db.volatile_block_count(), 5);

        // Non-destructive rollback: blocks remain in store
        db.rollback_to_point(&Point::Specific(SlotNo(3), make_hash(3)))
            .unwrap();
        assert_eq!(db.volatile_block_count(), 5);
    }

    #[test]
    fn test_flush_then_rollback_consistency() {
        // After flushing to immutable, rollback only removes volatile blocks.
        // Immutable blocks should remain accessible.
        let dir = tempfile::tempdir().unwrap();
        let mut db = ChainDB::open(dir.path()).unwrap();

        // Put 5 blocks into immutable via bulk import
        for i in 1..=5u64 {
            let mut hash_bytes = [0u8; 32];
            hash_bytes[0..8].copy_from_slice(&i.to_be_bytes());
            let hash = Hash32::from_bytes(hash_bytes);
            db.put_blocks_batch(&[(SlotNo(i * 10), &hash, BlockNo(i), b"imm")])
                .unwrap();
        }

        // Add volatile blocks on top
        for i in 6..=10u8 {
            db.add_block(
                make_hash(i),
                SlotNo(i as u64 * 10),
                BlockNo(i as u64),
                make_hash(i - 1),
                format!("vol_{i}").into_bytes(),
            )
            .unwrap();
        }

        assert_eq!(db.volatile_block_count(), 5);

        // Non-destructive rollback: blocks remain in volatile
        db.rollback_to_point(&Point::Origin).unwrap();
        assert_eq!(db.volatile_block_count(), 5);

        // Immutable blocks should still be there
        for i in 1..=5u64 {
            let mut hash_bytes = [0u8; 32];
            hash_bytes[0..8].copy_from_slice(&i.to_be_bytes());
            let hash = Hash32::from_bytes(hash_bytes);
            assert!(db.has_block(&hash));
        }

        // Tip should be from immutable
        assert_eq!(db.tip_slot(), SlotNo(50));
    }

    #[test]
    fn test_get_block_by_number_in_volatile() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = ChainDB::open(dir.path()).unwrap();

        build_chain(&mut db, 3);

        let result = db.get_block_by_number(BlockNo(2)).unwrap();
        assert!(result.is_some());
        let (slot, hash, cbor) = result.unwrap();
        assert_eq!(slot, SlotNo(2));
        assert_eq!(hash, make_hash(2));
        assert_eq!(cbor, b"block2");

        // Non-existent block number
        assert!(db.get_block_by_number(BlockNo(99)).unwrap().is_none());
    }

    #[test]
    fn test_empty_chaindb_origin_tip() {
        let dir = tempfile::tempdir().unwrap();
        let db = ChainDB::open(dir.path()).unwrap();

        assert_eq!(db.get_tip(), Tip::origin());
        assert_eq!(db.tip_slot(), SlotNo(0));
        assert!(!db.has_immutable());
        assert_eq!(db.volatile_block_count(), 0);
    }

    #[test]
    fn test_persist_and_reopen_immutable_tip() {
        let dir = tempfile::tempdir().unwrap();

        {
            let mut db = ChainDB::open(dir.path()).unwrap();
            let h1 = make_hash(1);
            let h2 = make_hash(2);
            db.put_blocks_batch(&[
                (SlotNo(100), &h1, BlockNo(1), b"b1"),
                (SlotNo(200), &h2, BlockNo(2), b"b2"),
            ])
            .unwrap();
            db.persist().unwrap();
        }

        // Reopen and verify tip info
        let db = ChainDB::open(dir.path()).unwrap();
        let (slot, hash, block_no) = db.get_tip_info().unwrap();
        assert_eq!(slot, SlotNo(200));
        assert_eq!(hash, make_hash(2));
        assert_eq!(block_no, BlockNo(2));
        assert!(db.has_immutable());
    }

    // -- get_chain_points tests --------------------------------------------

    #[test]
    fn test_get_chain_points_empty_db() {
        let dir = tempfile::tempdir().unwrap();
        let db = ChainDB::open(dir.path()).unwrap();

        let points = db.get_chain_points(10);
        assert!(points.is_empty(), "empty DB should return no chain points");
    }

    #[test]
    fn test_get_chain_points_walks_volatile_chain() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = ChainDB::open(dir.path()).unwrap();

        // Build a 5-block chain: h(0) <- h(1) <- h(2) <- h(3) <- h(4) <- h(5)
        build_chain(&mut db, 5);

        let points = db.get_chain_points(10);

        // Should walk backwards from tip: h(5), h(4), h(3), h(2), h(1)
        // h(0) is just a prev_hash reference, not stored as a block
        assert_eq!(points.len(), 5);
        assert_eq!(points[0], Point::Specific(SlotNo(5), make_hash(5)));
        assert_eq!(points[1], Point::Specific(SlotNo(4), make_hash(4)));
        assert_eq!(points[2], Point::Specific(SlotNo(3), make_hash(3)));
        assert_eq!(points[3], Point::Specific(SlotNo(2), make_hash(2)));
        assert_eq!(points[4], Point::Specific(SlotNo(1), make_hash(1)));
    }

    #[test]
    fn test_get_chain_points_respects_max_count() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = ChainDB::open(dir.path()).unwrap();

        build_chain(&mut db, 5);

        let points = db.get_chain_points(3);
        assert_eq!(points.len(), 3);
        assert_eq!(points[0], Point::Specific(SlotNo(5), make_hash(5)));
        assert_eq!(points[1], Point::Specific(SlotNo(4), make_hash(4)));
        assert_eq!(points[2], Point::Specific(SlotNo(3), make_hash(3)));
    }

    #[test]
    fn test_get_chain_points_includes_immutable_tip() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = ChainDB::open(dir.path()).unwrap();

        // Put a block in immutable
        let imm_hash = make_hash(1);
        db.put_blocks_batch(&[(SlotNo(100), &imm_hash, BlockNo(1), b"imm")])
            .unwrap();

        // Put a volatile block on top (prev_hash = imm_hash, but imm block
        // isn't in volatile, so the chain walk stops at the volatile block)
        let vol_hash = make_hash(2);
        db.add_block(vol_hash, SlotNo(200), BlockNo(2), imm_hash, b"vol".to_vec())
            .unwrap();

        let points = db.get_chain_points(10);

        // Should have volatile tip + immutable tip (chain walk couldn't
        // reach immutable via prev_hash because it's not in volatile)
        assert_eq!(points.len(), 2);
        assert_eq!(points[0], Point::Specific(SlotNo(200), vol_hash));
        assert_eq!(points[1], Point::Specific(SlotNo(100), imm_hash));
    }

    #[test]
    fn test_get_chain_points_forged_block_scenario() {
        // Simulates the exact scenario: chain tip is a forged block that
        // the peer won't know about, but the parent block IS on the
        // canonical chain and should be included in known_points.
        let dir = tempfile::tempdir().unwrap();
        let mut db = ChainDB::open(dir.path()).unwrap();

        // Canonical chain: blocks 1..3
        build_chain(&mut db, 3);

        // Forged block 4 at slot 100 (built on top of block 3)
        let forged_hash = make_hash(99);
        db.add_block(
            forged_hash,
            SlotNo(100),
            BlockNo(4),
            make_hash(3), // prev_hash = block 3 (canonical)
            b"forged_block".to_vec(),
        )
        .unwrap();

        let points = db.get_chain_points(10);

        // The forged block is the tip, but walking back gives us block 3
        // (canonical, known to peer), then 2, then 1.
        assert_eq!(points.len(), 4);
        assert_eq!(points[0], Point::Specific(SlotNo(100), forged_hash));
        assert_eq!(points[1], Point::Specific(SlotNo(3), make_hash(3)));
        assert_eq!(points[2], Point::Specific(SlotNo(2), make_hash(2)));
        assert_eq!(points[3], Point::Specific(SlotNo(1), make_hash(1)));
    }

    /// Verify that `flush_to_immutable` only flushes the canonical-chain block
    /// when two competing blocks exist at the same block_no (fork scenario).
    ///
    /// Scenario
    /// --------
    /// Build a linear canonical chain of k+3 blocks.  At block_no = k+1
    /// (which is outside the rollback window and will be finalized) also add
    /// a fork block with the same block_no but a different hash and a
    /// different parent.  After flushing:
    ///   - Only the canonical block at block_no k+1 must appear in ImmutableDB.
    ///   - The fork block must still be present in VolatileDB (not silently
    ///     removed because it shared the same slot).
    #[test]
    fn test_flush_to_immutable_canonical_chain_only_ignores_fork_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = ChainDB::open(dir.path()).unwrap();

        // Helper: deterministic hash from an integer seed.
        let h = |seed: u64| -> Hash32 {
            let mut bytes = [0u8; 32];
            bytes[0..8].copy_from_slice(&seed.to_be_bytes());
            Hash32::from_bytes(bytes)
        };

        let k = SECURITY_PARAM_K as u64;

        // Build canonical chain: blocks 1 .. k+3
        // Each block i has slot = i and is linked to block i-1.
        for i in 1..=(k + 3) {
            db.add_block(
                h(i),
                SlotNo(i),
                BlockNo(i),
                h(i - 1),
                format!("canonical_{i}").into_bytes(),
            )
            .unwrap();
        }

        // Add a fork block at block_no = 1 (deepest finalizable position).
        // It has a different hash (seed = 10_000) and a different parent
        // (origin), so it is NOT on the selected chain.
        let fork_hash = h(10_000);
        let fork_block_no = 1u64;
        let fork_slot = 1u64; // same slot as canonical block 1
        db.add_block(
            fork_hash,
            SlotNo(fork_slot),
            BlockNo(fork_block_no),
            h(99_999), // unknown parent — off the selected chain
            b"fork_block".to_vec(),
        )
        .unwrap();

        // The volatile store now contains k+3 canonical blocks plus 1 fork.
        assert_eq!(db.volatile.len(), k as usize + 4);

        // Flush: blocks 1..3 are deep enough (tip block_no = k+3, k+3-k = 3).
        let flushed = db.flush_to_immutable().unwrap();
        assert_eq!(flushed, 3, "expected exactly 3 canonical blocks flushed");

        // Canonical blocks 1-3 must be in ImmutableDB.
        for i in 1..=3u64 {
            assert!(
                db.immutable.has_block(&h(i)),
                "canonical block {i} must be in ImmutableDB"
            );
        }

        // The fork block at the same slot as block 1 must still be in
        // VolatileDB — it was NOT on the canonical chain so it must not have
        // been silently removed by the flush.
        assert!(
            db.volatile.has_block(&fork_hash),
            "fork block must survive in VolatileDB after canonical flush"
        );

        // The canonical block 1 must have been removed from VolatileDB
        // (it was flushed to ImmutableDB).
        assert!(
            !db.volatile.has_block(&h(1)),
            "canonical block 1 must be removed from VolatileDB after flush"
        );

        // Volatile should contain exactly k canonical blocks (4..k+3) + 1 fork.
        assert_eq!(
            db.volatile.len(),
            k as usize + 1,
            "volatile should hold k canonical blocks plus the fork block"
        );
    }

    #[test]
    fn test_rollback_beyond_volatile_returns_to_immutable() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = ChainDB::open(dir.path()).unwrap();

        // Put blocks 1-3 directly into immutable using consistent make_hash hashes
        for i in 1u8..=3 {
            let hash = make_hash(i);
            let cbor = format!("imm_{}", i).into_bytes();
            db.put_blocks_batch(&[(SlotNo(i as u64 * 100), &hash, BlockNo(i as u64), &cbor)])
                .unwrap();
        }

        // Add volatile blocks on top, chaining from make_hash(3)
        for i in 4u8..=6 {
            db.add_block(
                make_hash(i),
                SlotNo(i as u64 * 100),
                BlockNo(i as u64),
                make_hash(i - 1),
                format!("vol_{}", i).into_bytes(),
            )
            .unwrap();
        }

        assert_eq!(db.tip_slot(), SlotNo(600));

        // Rollback past all volatile blocks to origin
        db.rollback_to_point(&Point::Origin).unwrap();

        // Tip should come from immutable (block 3 at slot 300), since volatile is cleared
        assert_eq!(db.tip_slot(), SlotNo(300));

        // The volatile selected chain should be empty — all volatile blocks were
        // displaced to GC-pending by the rollback to Origin.
        assert_eq!(db.volatile_selected_chain_count(), 0);

        // Immutable blocks should still be accessible via has_block
        assert!(db.has_block(&make_hash(3)));
        assert!(db.has_block(&make_hash(2)));
    }

    #[test]
    fn test_rollback_partial_volatile_to_immutable_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = ChainDB::open(dir.path()).unwrap();

        // Build chain: 1 <- 2 <- 3 <- 4 <- 5
        for i in 1..=5u8 {
            db.add_block(
                make_hash(i),
                SlotNo(i as u64 * 10),
                BlockNo(i as u64),
                make_hash(i - 1),
                format!("block{}", i).into_bytes(),
            )
            .unwrap();
        }

        // Rollback to block 3 (halfway)
        let removed = db
            .rollback_to_point(&Point::Specific(SlotNo(30), make_hash(3)))
            .unwrap();

        // Blocks 4 and 5 should be removed
        assert_eq!(removed.len(), 2);
        assert!(removed.contains(&make_hash(4)));
        assert!(removed.contains(&make_hash(5)));

        // Tip should be block 3
        assert_eq!(db.tip_slot(), SlotNo(30));

        // All blocks should still be accessible
        assert!(db.has_block(&make_hash(1)));
        assert!(db.has_block(&make_hash(2)));
        assert!(db.has_block(&make_hash(3)));
    }

    #[test]
    fn test_rollback_nonexistent_point_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = ChainDB::open(dir.path()).unwrap();

        build_chain(&mut db, 3);

        // Try to rollback to a point that doesn't exist
        let nonexistent = Point::Specific(SlotNo(999), make_hash(99));
        let removed = db.rollback_to_point(&nonexistent).unwrap();

        // Should be a no-op
        assert!(removed.is_empty());
        assert_eq!(db.tip_slot(), SlotNo(3));
    }

    #[test]
    fn test_rollback_with_flush_scenario() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = ChainDB::open(dir.path()).unwrap();

        // Write blocks 1-5 directly to immutable via put_blocks_batch, simulating
        // a post-flush state. put_blocks_batch bypasses the k-depth guard that
        // flush_to_immutable enforces, so this works even with small block counts.
        for i in 1u8..=5 {
            let hash = make_hash(i);
            let cbor = format!("block{}", i).into_bytes();
            db.put_blocks_batch(&[(SlotNo(i as u64 * 10), &hash, BlockNo(i as u64), &cbor)])
                .unwrap();
        }

        // Add more volatile blocks on top of the immutable chain
        for i in 6u8..=8 {
            db.add_block(
                make_hash(i),
                SlotNo(i as u64 * 10),
                BlockNo(i as u64),
                make_hash(i - 1),
                format!("block{}", i).into_bytes(),
            )
            .unwrap();
        }

        assert_eq!(db.tip_slot(), SlotNo(80));

        // Rollback to block 6 (the first volatile block — within the volatile window)
        let removed = db
            .rollback_to_point(&Point::Specific(SlotNo(60), make_hash(6)))
            .unwrap();

        // Blocks 7 and 8 should be removed from the selected chain
        assert!(removed.contains(&make_hash(7)));
        assert!(removed.contains(&make_hash(8)));
        assert!(!removed.contains(&make_hash(6)));

        // Tip should now be block 6 (slot 60)
        assert_eq!(db.tip_slot(), SlotNo(60));

        // Immutable blocks 1-5 must still be present and accessible
        assert!(db.immutable.has_block(&make_hash(1)));
        assert!(db.immutable.has_block(&make_hash(2)));
        assert!(db.immutable.has_block(&make_hash(3)));
        assert!(db.immutable.has_block(&make_hash(4)));
        assert!(db.immutable.has_block(&make_hash(5)));
    }

    #[test]
    fn test_rollback_point_at_different_block_no_same_slot() {
        // This test verifies that rollback correctly removes successor blocks from
        // the selected chain when rolling back to a specific point. We build a
        // simple linear chain A → B → C and roll back to A.
        let dir = tempfile::tempdir().unwrap();
        let mut db = ChainDB::open(dir.path()).unwrap();

        let hash_a = make_hash(1);
        let hash_b = make_hash(2);
        let hash_c = make_hash(3);

        // Build linear chain: hash_a (slot 100, block 10) → hash_b (200, 11) → hash_c (300, 12)
        db.add_block(
            hash_a,
            SlotNo(100),
            BlockNo(10),
            make_hash(0),
            b"block_a".to_vec(),
        )
        .unwrap();
        db.add_block(
            hash_b,
            SlotNo(200),
            BlockNo(11),
            hash_a,
            b"block_b".to_vec(),
        )
        .unwrap();
        db.add_block(
            hash_c,
            SlotNo(300),
            BlockNo(12),
            hash_b,
            b"block_c".to_vec(),
        )
        .unwrap();

        // Roll back to hash_a specifically
        let removed = db
            .rollback_to_point(&Point::Specific(SlotNo(100), hash_a))
            .unwrap();

        // Blocks B and C must appear in the removed set; A must not
        assert!(removed.contains(&hash_b), "hash_b should be removed");
        assert!(removed.contains(&hash_c), "hash_c should be removed");
        assert!(!removed.contains(&hash_a), "hash_a should not be removed");

        // Block A should still be accessible and be the new tip
        assert!(db.has_block(&hash_a));
        assert_eq!(db.tip_slot(), SlotNo(100));
    }

    #[test]
    fn test_sequential_rollbacks() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = ChainDB::open(dir.path()).unwrap();

        // Build chain: 1 <- 2 <- 3 <- 4 <- 5 <- 6
        for i in 1..=6u8 {
            db.add_block(
                make_hash(i),
                SlotNo(i as u64 * 10),
                BlockNo(i as u64),
                make_hash(i - 1),
                format!("block{}", i).into_bytes(),
            )
            .unwrap();
        }

        // First rollback to block 4
        db.rollback_to_point(&Point::Specific(SlotNo(40), make_hash(4)))
            .unwrap();
        assert_eq!(db.tip_slot(), SlotNo(40));

        // Second rollback to block 2
        db.rollback_to_point(&Point::Specific(SlotNo(20), make_hash(2)))
            .unwrap();
        assert_eq!(db.tip_slot(), SlotNo(20));

        // All blocks still accessible
        for i in 1..=6u8 {
            assert!(db.has_block(&make_hash(i)));
        }
    }
}
