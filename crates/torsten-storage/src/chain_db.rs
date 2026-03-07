use crate::immutable_db::ImmutableDB;
use crate::volatile_db::VolatileDB;
use std::path::Path;
use thiserror::Error;
use torsten_primitives::block::{Point, Tip};
use torsten_primitives::hash::{BlockHeaderHash, Hash32};
use torsten_primitives::time::{BlockNo, SlotNo};

#[derive(Error, Debug)]
pub enum ChainDBError {
    #[error("Immutable DB error: {0}")]
    Immutable(#[from] crate::immutable_db::ImmutableDBError),
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
pub struct ChainDB {
    immutable: ImmutableDB,
    volatile: VolatileDB,
}

impl ChainDB {
    pub fn open(db_path: &Path) -> Result<Self, ChainDBError> {
        let immutable_path = db_path.join("immutable");
        let immutable = ImmutableDB::open(&immutable_path)?;
        let volatile = VolatileDB::new(SECURITY_PARAM_K * 2);

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
        // Store in volatile DB first
        self.volatile
            .put_block(hash, slot, block_no, prev_hash, cbor)?;

        // Check if we should flush old blocks to immutable DB
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

    /// Get the current chain tip
    pub fn get_tip(&self) -> Tip {
        self.volatile.get_tip().unwrap_or_else(Tip::origin)
    }

    /// Rollback to a given point by removing blocks from the volatile DB.
    /// Returns the hashes of the removed blocks (most recent first).
    pub fn rollback_to_point(
        &mut self,
        point: &Point,
    ) -> Result<Vec<BlockHeaderHash>, ChainDBError> {
        let target_hash = match point {
            Point::Origin => {
                // Rolling back to origin: clear all volatile blocks
                let tip = self.volatile.get_tip();
                if let Some(tip) = tip {
                    if let Some(tip_hash) = tip.point.hash() {
                        let chain = self.volatile.get_chain_back_to(tip_hash, &Hash32::ZERO);
                        if let Some(hashes) = chain {
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
        for hash in &chain {
            self.volatile.remove_block(hash);
        }

        // Update the tip to the rollback point
        self.volatile.update_tip_to(&target_hash);

        Ok(chain)
    }

    /// Check if a block exists in the chain DB
    pub fn has_block(&self, hash: &BlockHeaderHash) -> bool {
        self.volatile.get_block(hash).is_some()
            || self
                .immutable
                .get_block_by_hash(hash)
                .ok()
                .flatten()
                .is_some()
    }

    /// Flush old blocks from volatile to immutable when chain is long enough
    fn maybe_flush_to_immutable(&mut self) -> Result<(), ChainDBError> {
        let volatile_count = self.volatile.block_count();
        if volatile_count <= SECURITY_PARAM_K {
            return Ok(());
        }

        // Get oldest blocks that are beyond k-deep and flush them
        let to_flush = volatile_count - SECURITY_PARAM_K;
        let flushed = self.volatile.drain_oldest(to_flush);

        for (hash, slot, _block_no, cbor) in flushed {
            self.immutable.put_block(slot, &hash, &cbor)?;
        }

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
