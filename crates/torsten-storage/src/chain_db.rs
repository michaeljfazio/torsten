use crate::immutable_db::ImmutableDB;
use crate::volatile_db::VolatileDB;
use std::path::Path;
use thiserror::Error;
use torsten_primitives::block::Tip;
use torsten_primitives::hash::BlockHeaderHash;
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

    /// Flush old blocks from volatile to immutable when chain is long enough
    fn maybe_flush_to_immutable(&mut self) -> Result<(), ChainDBError> {
        // In production, this would track the immutable tip and move
        // blocks that are k-deep from volatile to immutable.
        // For now, this is a placeholder.
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
            .add_block(hash, SlotNo(100), BlockNo(50), Hash32::ZERO, b"data".to_vec())
            .unwrap();

        let tip = chain_db.get_tip();
        assert_eq!(tip.block_number, BlockNo(50));
    }
}
