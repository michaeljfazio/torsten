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
    #[allow(dead_code)]
    path: PathBuf,
    immutable: ImmutableDB,
    volatile: VolatileDB,
    immutable_tip: Option<(SlotNo, Hash32, BlockNo)>,
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

        debug!("ChainDB opened successfully");
        Ok(ChainDB {
            path: db_path.to_path_buf(),
            immutable,
            volatile: VolatileDB::new(),
            immutable_tip,
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

    // -- Rollback -----------------------------------------------------------

    /// Take a rollback snapshot. No-op in the new architecture — rollback
    /// is handled by the VolatileDB directly.
    pub fn take_rollback_snapshot(&mut self) {
        // No-op: VolatileDB handles rollback without snapshots
    }

    /// Rollback the chain to a given point.
    ///
    /// Only affects the VolatileDB (immutable blocks can't be rolled back).
    /// Returns the hashes of the removed blocks.
    pub fn rollback_to_point(
        &mut self,
        point: &Point,
    ) -> Result<Vec<BlockHeaderHash>, ChainDBError> {
        warn!(point = ?point, "ChainDB: rollback requested");

        let target_slot = point.slot().map(|s| s.0).unwrap_or(0);
        let target_hash = point.hash().copied();

        // Check if rollback is a no-op
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

        debug!(blocks_removed = removed.len(), "ChainDB: rollback complete");

        Ok(removed)
    }

    // -- Flush / Lifecycle --------------------------------------------------

    /// Flush finalized blocks from VolatileDB to ImmutableDB.
    ///
    /// Blocks with block_no <= (tip_block_no - k) are considered finalized.
    /// Call this after each batch of blocks is processed.
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

        // Collect blocks to finalize (in order by block_no)
        let mut to_finalize: Vec<(u64, Hash32, u64, Vec<u8>)> = Vec::new(); // (slot, hash, block_no, cbor)
        for block_no in 1..=finalize_up_to_block_no {
            if let Some((slot, hash, cbor)) = self.volatile.get_block_by_number(block_no) {
                // Skip if already in immutable
                if self.immutable.has_block(&hash) {
                    continue;
                }
                to_finalize.push((slot, hash, block_no, cbor.to_vec()));
            }
        }

        if to_finalize.is_empty() {
            return Ok(0);
        }

        let count = to_finalize.len() as u64;

        // Append to ImmutableDB
        for (slot, hash, block_no, cbor) in &to_finalize {
            self.immutable.append_block(*slot, *block_no, hash, cbor)?;
        }

        // Update immutable tip
        if let Some((slot, hash, block_no, _)) = to_finalize.last() {
            self.immutable_tip = Some((SlotNo(*slot), *hash, BlockNo(*block_no)));
        }

        // Remove finalized blocks from VolatileDB
        let max_slot = to_finalize.last().map(|(s, _, _, _)| *s).unwrap_or(0);
        self.volatile.remove_blocks_up_to_slot(max_slot);

        debug!(
            flushed = count,
            immutable_tip_slot = self.immutable_tip.map(|(s, _, _)| s.0).unwrap_or(0),
            volatile_remaining = self.volatile.len(),
            "ChainDB: flushed blocks to immutable"
        );

        Ok(count)
    }

    /// Flush ALL volatile blocks to ImmutableDB, bypassing the k-depth check.
    ///
    /// Only safe to call during graceful shutdown — after this, the VolatileDB
    /// is empty and the ImmutableDB contains all blocks up to the chain tip.
    /// This ensures the ledger snapshot (saved at the volatile tip) is
    /// consistent with the ImmutableDB tip on restart.
    pub fn flush_all_to_immutable(&mut self) -> Result<u64, ChainDBError> {
        let vol_tip = match self.volatile.get_tip() {
            Some(t) => t,
            None => return Ok(0),
        };

        let tip_block_no = vol_tip.2;

        // Collect all volatile blocks in block_no order
        let mut to_finalize: Vec<(u64, Hash32, u64, Vec<u8>)> = Vec::new();
        for block_no in 1..=tip_block_no {
            if let Some((slot, hash, cbor)) = self.volatile.get_block_by_number(block_no) {
                if self.immutable.has_block(&hash) {
                    continue;
                }
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

        if let Some((slot, hash, block_no, _)) = to_finalize.last() {
            self.immutable_tip = Some((SlotNo(*slot), *hash, BlockNo(*block_no)));
        }

        // Clear volatile — everything is now in immutable
        self.volatile.clear();

        debug!(
            flushed = count,
            immutable_tip_slot = self.immutable_tip.map(|(s, _, _)| s.0).unwrap_or(0),
            "ChainDB: flushed all volatile blocks to immutable (shutdown)"
        );

        Ok(count)
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

        // Blocks 4 and 5 should be gone
        assert!(!db.has_block(&make_hash(4)));
        assert!(!db.has_block(&make_hash(5)));

        // Blocks 1-3 should still exist
        assert!(db.has_block(&make_hash(1)));
        assert!(db.has_block(&make_hash(2)));
        assert!(db.has_block(&make_hash(3)));
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

        // flush_all should move everything
        let flushed = db.flush_all_to_immutable().unwrap();
        assert_eq!(flushed, 20);
        assert_eq!(db.volatile.len(), 0);

        // All blocks should still be readable from immutable
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
}
