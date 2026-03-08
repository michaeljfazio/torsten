use std::path::{Path, PathBuf};
use thiserror::Error;
use torsten_primitives::hash::{BlockHeaderHash, Hash32};
use torsten_primitives::time::{BlockNo, SlotNo};
use tracing::{debug, info, trace};

#[derive(Error, Debug)]
pub enum ImmutableDBError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("RocksDB error: {0}")]
    RocksDB(String),
    #[error("Block not found: {0}")]
    BlockNotFound(String),
    #[error("Serialization error: {0}")]
    Serialization(String),
}

/// ImmutableDB stores blocks that are considered immutable (k blocks deep)
///
/// In Cardano, the security parameter k (currently 2160) determines
/// how many blocks deep a block must be to be considered immutable.
/// Once a block is immutable, it can never be rolled back.
///
/// Layout: blocks are stored in chunk files, each covering a range of slots.
pub struct ImmutableDB {
    db_path: PathBuf,
    db: Option<rocksdb::DB>,
    tip_slot: SlotNo,
}

impl ImmutableDB {
    pub fn open(path: &Path) -> Result<Self, ImmutableDBError> {
        info!(path = %path.display(), "Opening ImmutableDB (RocksDB)");
        std::fs::create_dir_all(path)?;

        let mut opts = rocksdb::Options::default();
        opts.create_if_missing(true);
        opts.set_compression_type(rocksdb::DBCompressionType::Lz4);

        let db =
            rocksdb::DB::open(&opts, path).map_err(|e| ImmutableDBError::RocksDB(e.to_string()))?;

        // Recover tip metadata from stored key
        let tip_slot = Self::recover_tip(&db);
        if tip_slot > SlotNo(0) {
            info!(tip_slot = tip_slot.0, "ImmutableDB recovered tip from DB");
        }

        info!("ImmutableDB opened successfully");
        Ok(ImmutableDB {
            db_path: path.to_path_buf(),
            db: Some(db),
            tip_slot,
        })
    }

    /// Store a block's raw CBOR in the immutable DB
    pub fn put_block_with_blockno(
        &mut self,
        slot: SlotNo,
        hash: &BlockHeaderHash,
        block_no: BlockNo,
        cbor: &[u8],
    ) -> Result<(), ImmutableDBError> {
        self.put_block_inner(slot, hash, Some(block_no), cbor)
    }

    /// Store a block's raw CBOR in the immutable DB
    pub fn put_block(
        &mut self,
        slot: SlotNo,
        hash: &BlockHeaderHash,
        cbor: &[u8],
    ) -> Result<(), ImmutableDBError> {
        self.put_block_inner(slot, hash, None, cbor)
    }

    fn put_block_inner(
        &mut self,
        slot: SlotNo,
        hash: &BlockHeaderHash,
        block_no: Option<BlockNo>,
        cbor: &[u8],
    ) -> Result<(), ImmutableDBError> {
        trace!(
            slot = slot.0,
            hash = %hash.to_hex(),
            cbor_bytes = cbor.len(),
            "ImmutableDB: storing block"
        );
        let db = self
            .db
            .as_ref()
            .ok_or_else(|| ImmutableDBError::RocksDB("DB not open".into()))?;

        // Key by slot number
        let slot_key = slot.0.to_be_bytes();
        db.put(slot_key, cbor)
            .map_err(|e| ImmutableDBError::RocksDB(e.to_string()))?;

        // Secondary index: hash -> slot
        let hash_key = [b"hash:", hash.as_bytes().as_slice()].concat();
        db.put(hash_key, slot_key)
            .map_err(|e| ImmutableDBError::RocksDB(e.to_string()))?;

        if slot > self.tip_slot {
            debug!(slot = slot.0, "ImmutableDB: new tip slot");
            self.tip_slot = slot;
            // Persist tip metadata: slot(8) + hash(32) + block_no(8)
            let mut tip_value = Vec::with_capacity(48);
            tip_value.extend_from_slice(&slot.0.to_be_bytes());
            tip_value.extend_from_slice(hash.as_bytes());
            tip_value.extend_from_slice(&block_no.map_or(0u64, |b| b.0).to_be_bytes());
            db.put(b"meta:tip", &tip_value)
                .map_err(|e| ImmutableDBError::RocksDB(e.to_string()))?;
        }

        Ok(())
    }

    /// Get a block's raw CBOR by slot
    pub fn get_block_by_slot(&self, slot: SlotNo) -> Result<Option<Vec<u8>>, ImmutableDBError> {
        let db = self
            .db
            .as_ref()
            .ok_or_else(|| ImmutableDBError::RocksDB("DB not open".into()))?;
        let key = slot.0.to_be_bytes();
        db.get(key)
            .map_err(|e| ImmutableDBError::RocksDB(e.to_string()))
    }

    /// Get a block's raw CBOR by hash
    pub fn get_block_by_hash(
        &self,
        hash: &BlockHeaderHash,
    ) -> Result<Option<Vec<u8>>, ImmutableDBError> {
        let db = self
            .db
            .as_ref()
            .ok_or_else(|| ImmutableDBError::RocksDB("DB not open".into()))?;
        let hash_key = [b"hash:", hash.as_bytes().as_slice()].concat();

        match db
            .get(hash_key)
            .map_err(|e| ImmutableDBError::RocksDB(e.to_string()))?
        {
            Some(slot_bytes) => db
                .get(slot_bytes)
                .map_err(|e| ImmutableDBError::RocksDB(e.to_string())),
            None => Ok(None),
        }
    }

    pub fn tip_slot(&self) -> SlotNo {
        self.tip_slot
    }

    pub fn path(&self) -> &Path {
        &self.db_path
    }

    /// Get the tip slot, hash, and block number from persisted metadata
    pub fn get_tip_info(&self) -> Option<(SlotNo, BlockHeaderHash, BlockNo)> {
        let db = self.db.as_ref()?;
        let data = db.get(b"meta:tip").ok()??;
        if data.len() < 40 {
            return None;
        }
        let slot = SlotNo(u64::from_be_bytes(data[..8].try_into().ok()?));
        let mut hash_bytes = [0u8; 32];
        hash_bytes.copy_from_slice(&data[8..40]);
        let block_no = if data.len() >= 48 {
            BlockNo(u64::from_be_bytes(data[40..48].try_into().ok()?))
        } else {
            BlockNo(0)
        };
        Some((slot, Hash32::from_bytes(hash_bytes), block_no))
    }

    /// Recover the tip slot from RocksDB on startup
    fn recover_tip(db: &rocksdb::DB) -> SlotNo {
        match db.get(b"meta:tip") {
            Ok(Some(data)) if data.len() >= 8 => {
                SlotNo(u64::from_be_bytes(data[..8].try_into().unwrap_or([0; 8])))
            }
            _ => SlotNo(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use torsten_primitives::hash::Hash32;

    #[test]
    fn test_immutable_db_put_get() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = ImmutableDB::open(dir.path()).unwrap();

        let slot = SlotNo(100);
        let hash = Hash32::from_bytes([1u8; 32]);
        let cbor = b"fake block data";

        db.put_block(slot, &hash, cbor).unwrap();

        let result = db.get_block_by_slot(slot).unwrap();
        assert_eq!(result.as_deref(), Some(cbor.as_slice()));

        let result = db.get_block_by_hash(&hash).unwrap();
        assert_eq!(result.as_deref(), Some(cbor.as_slice()));
    }

    #[test]
    fn test_immutable_db_missing_block() {
        let dir = tempfile::tempdir().unwrap();
        let db = ImmutableDB::open(dir.path()).unwrap();

        let result = db.get_block_by_slot(SlotNo(999)).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_immutable_db_tip_updates() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = ImmutableDB::open(dir.path()).unwrap();

        assert_eq!(db.tip_slot(), SlotNo(0));

        db.put_block(SlotNo(50), &Hash32::from_bytes([1u8; 32]), b"block1")
            .unwrap();
        assert_eq!(db.tip_slot(), SlotNo(50));

        db.put_block(SlotNo(100), &Hash32::from_bytes([2u8; 32]), b"block2")
            .unwrap();
        assert_eq!(db.tip_slot(), SlotNo(100));
    }

    #[test]
    fn test_immutable_db_tip_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let hash = Hash32::from_bytes([42u8; 32]);

        // Store some blocks, then close DB
        {
            let mut db = ImmutableDB::open(dir.path()).unwrap();
            db.put_block_with_blockno(SlotNo(500), &hash, BlockNo(100), b"block")
                .unwrap();
            assert_eq!(db.tip_slot(), SlotNo(500));
        }

        // Reopen — tip should be recovered
        {
            let db = ImmutableDB::open(dir.path()).unwrap();
            assert_eq!(db.tip_slot(), SlotNo(500));
            let tip_info = db.get_tip_info().unwrap();
            assert_eq!(tip_info.0, SlotNo(500));
            assert_eq!(tip_info.1, hash);
            assert_eq!(tip_info.2, BlockNo(100));
        }
    }
}
