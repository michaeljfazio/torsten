#[cfg(not(feature = "lsm"))]
use crate::immutable_db::ImmutableDB;
#[cfg(feature = "lsm")]
use crate::lsm::LsmImmutableDB;
use crate::volatile_db::VolatileDB;
use std::path::Path;
use thiserror::Error;
use torsten_primitives::block::{Point, Tip};
use torsten_primitives::hash::{BlockHeaderHash, Hash32};
use torsten_primitives::time::{BlockNo, SlotNo};
use tracing::{info, trace, warn};

#[derive(Error, Debug)]
pub enum ChainDBError {
    #[error("Immutable DB error: {0}")]
    #[cfg(not(feature = "lsm"))]
    Immutable(#[from] crate::immutable_db::ImmutableDBError),
    #[error("Immutable DB error (LSM): {0}")]
    #[cfg(feature = "lsm")]
    Immutable(#[from] crate::lsm::LsmImmutableDBError),
    #[error("Volatile DB error: {0}")]
    Volatile(#[from] crate::volatile_db::VolatileDBError),
    #[error("Block not found: {0}")]
    BlockNotFound(String),
}

/// The security parameter k (number of blocks before immutability)
pub const SECURITY_PARAM_K: usize = 2160;

/// ChainDB combines ImmutableDB and VolatileDB
///
/// Blocks within the last k slots are in VolatileDB (can be rolled back).
/// Blocks older than k slots are in ImmutableDB (permanent).
///
/// The immutable backend is selected at compile time:
/// - Default: RocksDB-backed `ImmutableDB`
/// - `lsm` feature: `cardano-lsm`-backed `LsmImmutableDB`
pub struct ChainDB {
    #[cfg(not(feature = "lsm"))]
    immutable: ImmutableDB,
    #[cfg(feature = "lsm")]
    immutable: LsmImmutableDB,
    volatile: VolatileDB,
}

impl ChainDB {
    pub fn open(db_path: &Path) -> Result<Self, ChainDBError> {
        info!(path = %db_path.display(), k = SECURITY_PARAM_K, "Opening ChainDB");
        let immutable_path = db_path.join("immutable");

        #[cfg(not(feature = "lsm"))]
        let immutable = ImmutableDB::open(&immutable_path)?;
        #[cfg(feature = "lsm")]
        let immutable = LsmImmutableDB::open(&immutable_path)?;

        let volatile = VolatileDB::new(SECURITY_PARAM_K * 2);
        info!("ChainDB opened successfully");

        Ok(ChainDB {
            immutable,
            volatile,
        })
    }

    /// Store a new block
    pub fn add_block(
        &mut self,
        hash: BlockHeaderHash,
        slot: SlotNo,
        block_no: BlockNo,
        prev_hash: BlockHeaderHash,
        cbor: Vec<u8>,
    ) -> Result<(), ChainDBError> {
        let cbor_len = cbor.len();
        trace!(
            hash = %hash.to_hex(),
            slot = slot.0,
            block_no = block_no.0,
            prev_hash = %prev_hash.to_hex(),
            cbor_bytes = cbor_len,
            "ChainDB: adding block"
        );

        // Store in volatile DB first
        self.volatile
            .put_block(hash, slot, block_no, prev_hash, cbor)?;

        // Check if we should flush old blocks to immutable DB
        self.maybe_flush_to_immutable()?;

        Ok(())
    }

    /// Store multiple blocks in a batch, flushing to immutable only once at the end.
    /// Uses batched volatile DB writes (single lock acquisition) for performance.
    /// Takes ownership of blocks to avoid cloning CBOR data.
    pub fn add_blocks_batch(
        &mut self,
        blocks: Vec<(BlockHeaderHash, SlotNo, BlockNo, BlockHeaderHash, Vec<u8>)>,
    ) -> Result<(), ChainDBError> {
        if blocks.is_empty() {
            return Ok(());
        }

        // Filter out blocks that already exist in immutable DB
        // (volatile duplicates are handled by put_blocks_batch)
        let batch: Vec<_> = blocks
            .into_iter()
            .filter(|(hash, slot, _, _, _)| {
                let exists = self.immutable.has_block(hash);
                if exists {
                    trace!(
                        hash = %hash.to_hex(),
                        slot = slot.0,
                        "ChainDB: block already in immutable DB, skipping"
                    );
                }
                !exists
            })
            .collect();

        let inserted = self.volatile.put_blocks_batch(batch);
        trace!(inserted, "ChainDB: batch insert to volatile complete");

        // Flush once at the end of the batch
        self.maybe_flush_to_immutable()?;
        Ok(())
    }

    /// Get block CBOR by hash (checks volatile first, then immutable)
    pub fn get_block(&self, hash: &BlockHeaderHash) -> Result<Option<Vec<u8>>, ChainDBError> {
        if let Some(cbor) = self.volatile.get_block(hash) {
            return Ok(Some(cbor));
        }
        Ok(self.immutable.get_block_by_hash(hash)?)
    }

    /// Get the current chain tip (checks volatile first, then immutable)
    pub fn get_tip(&self) -> Tip {
        if let Some(tip) = self.volatile.get_tip() {
            return tip;
        }
        // Fall back to immutable tip
        if let Some((slot, hash, block_no)) = self.immutable.get_tip_info() {
            return Tip {
                point: Point::Specific(slot, hash),
                block_number: block_no,
            };
        }
        Tip::origin()
    }

    /// Get the immutable DB tip info (slot, hash, block_no) if available
    pub fn get_immutable_tip(&self) -> Option<(SlotNo, BlockHeaderHash, BlockNo)> {
        self.immutable.get_tip_info()
    }

    /// Rollback to a given point by removing blocks from the volatile DB.
    /// Returns the hashes of the removed blocks (most recent first).
    pub fn rollback_to_point(
        &mut self,
        point: &Point,
    ) -> Result<Vec<BlockHeaderHash>, ChainDBError> {
        warn!(point = ?point, "ChainDB: rollback requested");

        let target_hash = match point {
            Point::Origin => {
                warn!("ChainDB: rolling back to origin — clearing all volatile blocks");
                // Rolling back to origin: clear all volatile blocks
                let tip = self.volatile.get_tip();
                if let Some(tip) = tip {
                    if let Some(tip_hash) = tip.point.hash() {
                        let chain = self.volatile.get_chain_back_to(tip_hash, &Hash32::ZERO);
                        if let Some(hashes) = chain {
                            warn!(
                                blocks_removed = hashes.len(),
                                "ChainDB: rollback to origin complete"
                            );
                            for hash in &hashes {
                                self.volatile.remove_block(hash);
                            }
                            return Ok(hashes);
                        }
                    }
                }
                return Ok(vec![]);
            }
            Point::Specific(_, hash) => *hash,
        };

        let tip = self.volatile.get_tip();
        let tip_hash = match tip {
            Some(t) => match t.point.hash() {
                Some(h) => *h,
                None => return Ok(vec![]),
            },
            None => return Ok(vec![]),
        };

        if tip_hash == target_hash {
            return Ok(vec![]); // Already at this point
        }

        // Get the chain of blocks to remove
        let chain = self
            .volatile
            .get_chain_back_to(&tip_hash, &target_hash)
            .ok_or_else(|| {
                ChainDBError::BlockNotFound(format!(
                    "Cannot find chain from {} back to {}",
                    tip_hash.to_hex(),
                    target_hash.to_hex()
                ))
            })?;

        // Remove blocks from volatile DB
        warn!(
            blocks_to_remove = chain.len(),
            target = %target_hash.to_hex(),
            "ChainDB: removing rolled-back blocks"
        );
        for hash in &chain {
            self.volatile.remove_block(hash);
        }

        // Update the tip to the rollback point
        self.volatile.update_tip_to(&target_hash);

        info!(
            blocks_removed = chain.len(),
            new_tip = %target_hash.to_hex(),
            "ChainDB: rollback complete"
        );

        Ok(chain)
    }

    /// Get blocks in a slot range [from_slot, to_slot] inclusive.
    /// Returns raw CBOR block data in slot order.
    /// Combines results from both immutable and volatile DBs.
    pub fn get_blocks_in_slot_range(
        &self,
        from_slot: SlotNo,
        to_slot: SlotNo,
    ) -> Result<Vec<Vec<u8>>, ChainDBError> {
        // Get from immutable DB first (older blocks)
        let mut blocks = self
            .immutable
            .get_blocks_in_slot_range(from_slot, to_slot)?;
        // Then append from volatile DB (newer blocks, may overlap in slot)
        let volatile_blocks = self.volatile.get_blocks_in_slot_range(from_slot, to_slot);
        blocks.extend(volatile_blocks);
        Ok(blocks)
    }

    /// Get the first block after the given slot.
    /// Returns (slot, hash, cbor) of the next block, or None.
    /// Checks both immutable and volatile DB, returns the block with the lowest slot.
    pub fn get_next_block_after_slot(
        &self,
        after_slot: SlotNo,
    ) -> Result<Option<(SlotNo, BlockHeaderHash, Vec<u8>)>, ChainDBError> {
        let imm_result = self.immutable.get_next_block_after_slot(after_slot)?;
        let vol_result = self.volatile.get_next_block_after_slot(after_slot);

        match (imm_result, vol_result) {
            (Some((imm_slot, imm_hash, imm_cbor)), Some((vol_slot, vol_hash, vol_cbor))) => {
                if vol_slot <= imm_slot {
                    Ok(Some((vol_slot, vol_hash, vol_cbor)))
                } else {
                    Ok(Some((imm_slot, imm_hash, imm_cbor)))
                }
            }
            (Some(imm), None) => Ok(Some(imm)),
            (None, Some(vol)) => Ok(Some(vol)),
            (None, None) => Ok(None),
        }
    }

    /// Check if a block exists in the chain DB (uses hash index only, no block data read)
    pub fn has_block(&self, hash: &BlockHeaderHash) -> bool {
        self.volatile.has_block(hash) || self.immutable.has_block(hash)
    }

    /// Flush ALL volatile blocks to immutable DB. Called during graceful shutdown
    /// to ensure no blocks are lost.
    pub fn flush_volatile_to_immutable(&mut self) -> Result<(), ChainDBError> {
        let volatile_count = self.volatile.block_count();
        if volatile_count == 0 {
            return Ok(());
        }
        info!(
            volatile_count,
            "ChainDB: flushing all volatile blocks to immutable DB on shutdown"
        );
        let flushed = self.volatile.drain_oldest(volatile_count);
        let batch: Vec<_> = flushed
            .iter()
            .map(|(hash, slot, block_no, cbor)| (*slot, hash, *block_no, cbor.as_slice()))
            .collect();
        self.immutable.put_blocks_batch(&batch)?;
        info!(flushed = flushed.len(), "ChainDB: shutdown flush complete");
        Ok(())
    }

    /// Flush old blocks from volatile to immutable when chain is long enough.
    /// Uses batched writes for performance.
    fn maybe_flush_to_immutable(&mut self) -> Result<(), ChainDBError> {
        let volatile_count = self.volatile.block_count();
        if volatile_count <= SECURITY_PARAM_K {
            return Ok(());
        }

        // Get oldest blocks that are beyond k-deep and flush them
        let to_flush = volatile_count - SECURITY_PARAM_K;
        info!(
            volatile_count,
            to_flush, "ChainDB: flushing volatile blocks to immutable DB"
        );
        let flushed = self.volatile.drain_oldest(to_flush);

        // Use batched write — all blocks in a single atomic write batch
        let batch: Vec<_> = flushed
            .iter()
            .map(|(hash, slot, block_no, cbor)| (*slot, hash, *block_no, cbor.as_slice()))
            .collect();
        self.immutable.put_blocks_batch(&batch)?;

        info!(
            flushed = flushed.len(),
            "ChainDB: flush to immutable complete"
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use torsten_primitives::hash::Hash32;

    #[test]
    fn test_chain_db_add_and_get() {
        let dir = tempfile::tempdir().unwrap();
        let mut chain_db = ChainDB::open(dir.path()).unwrap();

        let hash = Hash32::from_bytes([1u8; 32]);
        let prev = Hash32::from_bytes([0u8; 32]);

        chain_db
            .add_block(hash, SlotNo(100), BlockNo(50), prev, b"block data".to_vec())
            .unwrap();

        let result = chain_db.get_block(&hash).unwrap();
        assert_eq!(result.as_deref(), Some(b"block data".as_slice()));
    }

    #[test]
    fn test_chain_db_tip() {
        let dir = tempfile::tempdir().unwrap();
        let mut chain_db = ChainDB::open(dir.path()).unwrap();

        assert_eq!(chain_db.get_tip(), Tip::origin());

        let hash = Hash32::from_bytes([1u8; 32]);
        chain_db
            .add_block(
                hash,
                SlotNo(100),
                BlockNo(50),
                Hash32::ZERO,
                b"data".to_vec(),
            )
            .unwrap();

        let tip = chain_db.get_tip();
        assert_eq!(tip.block_number, BlockNo(50));
    }

    fn make_hash(n: u8) -> BlockHeaderHash {
        Hash32::from_bytes([n; 32])
    }

    fn build_chain(chain_db: &mut ChainDB, count: u8) {
        for i in 1..=count {
            chain_db
                .add_block(
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
    fn test_rollback_to_specific_point() {
        let dir = tempfile::tempdir().unwrap();
        let mut chain_db = ChainDB::open(dir.path()).unwrap();

        // Build chain: 0 <- 1 <- 2 <- 3 <- 4 <- 5
        build_chain(&mut chain_db, 5);
        assert_eq!(chain_db.get_tip().block_number, BlockNo(5));

        // Rollback to block 3
        let removed = chain_db
            .rollback_to_point(&Point::Specific(SlotNo(3), make_hash(3)))
            .unwrap();

        assert_eq!(removed.len(), 2); // blocks 5 and 4
        assert_eq!(removed[0], make_hash(5));
        assert_eq!(removed[1], make_hash(4));
        assert_eq!(chain_db.get_tip().block_number, BlockNo(3));
    }

    #[test]
    fn test_rollback_to_current_tip() {
        let dir = tempfile::tempdir().unwrap();
        let mut chain_db = ChainDB::open(dir.path()).unwrap();

        build_chain(&mut chain_db, 3);

        // Rollback to current tip should be a no-op
        let removed = chain_db
            .rollback_to_point(&Point::Specific(SlotNo(3), make_hash(3)))
            .unwrap();
        assert!(removed.is_empty());
        assert_eq!(chain_db.get_tip().block_number, BlockNo(3));
    }

    #[test]
    fn test_rollback_to_origin() {
        let dir = tempfile::tempdir().unwrap();
        let mut chain_db = ChainDB::open(dir.path()).unwrap();

        build_chain(&mut chain_db, 3);

        let removed = chain_db.rollback_to_point(&Point::Origin).unwrap();
        assert_eq!(removed.len(), 3);
    }

    #[test]
    fn test_has_block() {
        let dir = tempfile::tempdir().unwrap();
        let mut chain_db = ChainDB::open(dir.path()).unwrap();

        build_chain(&mut chain_db, 2);

        assert!(chain_db.has_block(&make_hash(1)));
        assert!(chain_db.has_block(&make_hash(2)));
        assert!(!chain_db.has_block(&make_hash(99)));
    }
}
