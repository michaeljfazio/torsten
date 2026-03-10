//! LSM-tree based ImmutableDB backend using `cardano-lsm`.
//!
//! This module provides `LsmImmutableDB`, an alternative to the RocksDB-backed
//! `ImmutableDB`. It uses the `cardano-lsm` crate — a pure Rust LSM tree
//! designed for Cardano blockchain indexing workloads.
//!
//! Enable with the `lsm` feature flag:
//! ```toml
//! torsten-storage = { path = "...", features = ["lsm"] }
//! ```

use std::path::{Path, PathBuf};
use thiserror::Error;
use torsten_primitives::hash::{BlockHeaderHash, Hash32};
use torsten_primitives::time::{BlockNo, SlotNo};
use tracing::{debug, info, trace};

use cardano_lsm::{CompactionStrategy, Key, LsmConfig, LsmTree, Value};

#[derive(Error, Debug)]
pub enum LsmImmutableDBError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("LSM error: {0}")]
    Lsm(String),
    #[error("Block not found: {0}")]
    BlockNotFound(String),
    #[error("Serialization error: {0}")]
    Serialization(String),
}

impl From<cardano_lsm::Error> for LsmImmutableDBError {
    fn from(e: cardano_lsm::Error) -> Self {
        LsmImmutableDBError::Lsm(e.to_string())
    }
}

/// Build a key for block CBOR storage: "slot:" + 8-byte big-endian slot
#[inline]
fn make_slot_key(slot: SlotNo) -> Key {
    let mut buf = [0u8; 13];
    buf[..5].copy_from_slice(b"slot:");
    buf[5..].copy_from_slice(&slot.0.to_be_bytes());
    Key::from(buf.as_slice())
}

/// Build a key for hash->slot index: "hash:" + 32-byte hash
#[inline]
fn make_hash_key(hash: &BlockHeaderHash) -> Key {
    let mut buf = [0u8; 37];
    buf[..5].copy_from_slice(b"hash:");
    buf[5..].copy_from_slice(hash.as_bytes());
    Key::from(buf.as_slice())
}

/// Build a key for slot->hash reverse index: "slot_hash:" + 8-byte slot
#[inline]
fn make_slot_hash_key(slot: SlotNo) -> Key {
    let mut buf = [0u8; 18];
    buf[..10].copy_from_slice(b"slot_hash:");
    buf[10..].copy_from_slice(&slot.0.to_be_bytes());
    Key::from(buf.as_slice())
}

/// Key for tip metadata
const META_TIP_KEY: &[u8] = b"meta:tip";

/// ImmutableDB backed by `cardano-lsm` LSM tree.
///
/// Provides the same interface as the RocksDB-backed `ImmutableDB`:
/// - Block storage keyed by slot number
/// - Secondary index: block hash -> slot
/// - Reverse index: slot -> block hash
/// - Tip metadata persistence
pub struct LsmImmutableDB {
    db_path: PathBuf,
    tree: LsmTree,
    tip_slot: SlotNo,
}

impl LsmImmutableDB {
    /// Open or create an LSM-backed ImmutableDB at the given path.
    pub fn open(path: &Path) -> Result<Self, LsmImmutableDBError> {
        info!(path = %path.display(), "Opening ImmutableDB (cardano-lsm)");
        std::fs::create_dir_all(path)?;

        let config = LsmConfig {
            memtable_size: 128 * 1024 * 1024, // 128MB write buffer (matches RocksDB config)
            block_cache_size: 256 * 1024 * 1024, // 256MB cache
            bloom_filter_bits_per_key: 10,
            compaction_strategy: CompactionStrategy::Hybrid {
                l0_strategy: Box::new(CompactionStrategy::Tiered {
                    size_ratio: 4.0,
                    min_merge_width: 2,
                    max_merge_width: 8,
                }),
                ln_strategy: Box::new(CompactionStrategy::Leveled {
                    size_ratio: 10.0,
                    max_level: 7,
                }),
                transition_level: 2,
            },
            ..LsmConfig::default()
        };

        let tree =
            LsmTree::open(path, config).map_err(|e| LsmImmutableDBError::Lsm(e.to_string()))?;

        // Recover tip metadata
        let tip_slot = Self::recover_tip(&tree);
        if tip_slot > SlotNo(0) {
            info!(
                tip_slot = tip_slot.0,
                "LsmImmutableDB recovered tip from DB"
            );
        }

        info!("LsmImmutableDB opened successfully");
        Ok(LsmImmutableDB {
            db_path: path.to_path_buf(),
            tree,
            tip_slot,
        })
    }

    /// Store a block's raw CBOR in the immutable DB.
    pub fn put_block_with_blockno(
        &mut self,
        slot: SlotNo,
        hash: &BlockHeaderHash,
        block_no: BlockNo,
        cbor: &[u8],
    ) -> Result<(), LsmImmutableDBError> {
        self.put_block_inner(slot, hash, Some(block_no), cbor)
    }

    /// Store a block's raw CBOR in the immutable DB.
    pub fn put_block(
        &mut self,
        slot: SlotNo,
        hash: &BlockHeaderHash,
        cbor: &[u8],
    ) -> Result<(), LsmImmutableDBError> {
        self.put_block_inner(slot, hash, None, cbor)
    }

    /// Store multiple blocks atomically using a batch insert.
    pub fn put_blocks_batch(
        &mut self,
        blocks: &[(SlotNo, &BlockHeaderHash, BlockNo, &[u8])],
    ) -> Result<(), LsmImmutableDBError> {
        if blocks.is_empty() {
            return Ok(());
        }

        let mut batch: Vec<(Key, Value)> = Vec::with_capacity(blocks.len() * 3 + 1);
        let mut max_tip_slot = self.tip_slot;
        let mut tip_entry: Option<(SlotNo, &BlockHeaderHash, BlockNo)> = None;

        for (slot, hash, block_no, cbor) in blocks {
            // Block CBOR keyed by slot
            batch.push((make_slot_key(*slot), Value::from(*cbor)));

            // Hash -> slot index
            batch.push((
                make_hash_key(hash),
                Value::from(slot.0.to_be_bytes().as_slice()),
            ));

            // Slot -> hash reverse index
            batch.push((make_slot_hash_key(*slot), Value::from(hash.as_bytes())));

            if *slot > max_tip_slot {
                max_tip_slot = *slot;
                tip_entry = Some((*slot, hash, *block_no));
            }
        }

        // Update tip metadata
        if let Some((slot, hash, block_no)) = tip_entry {
            let mut tip_value = Vec::with_capacity(48);
            tip_value.extend_from_slice(&slot.0.to_be_bytes());
            tip_value.extend_from_slice(hash.as_bytes());
            tip_value.extend_from_slice(&block_no.0.to_be_bytes());
            batch.push((Key::from(META_TIP_KEY), Value::from(tip_value.as_slice())));
        }

        self.tree
            .insert_batch(batch)
            .map_err(|e| LsmImmutableDBError::Lsm(e.to_string()))?;
        self.tip_slot = max_tip_slot;

        Ok(())
    }

    /// Trigger a manual compaction of the entire key range.
    pub fn compact(&mut self) {
        if let Err(e) = self.tree.compact_all() {
            tracing::warn!(error = %e, "LsmImmutableDB: compaction failed");
        }
    }

    fn put_block_inner(
        &mut self,
        slot: SlotNo,
        hash: &BlockHeaderHash,
        block_no: Option<BlockNo>,
        cbor: &[u8],
    ) -> Result<(), LsmImmutableDBError> {
        trace!(
            slot = slot.0,
            hash = %hash.to_hex(),
            cbor_bytes = cbor.len(),
            "LsmImmutableDB: storing block"
        );

        // Key by slot number
        self.tree
            .insert(&make_slot_key(slot), &Value::from(cbor))
            .map_err(|e| LsmImmutableDBError::Lsm(e.to_string()))?;

        // Secondary index: hash -> slot
        self.tree
            .insert(
                &make_hash_key(hash),
                &Value::from(slot.0.to_be_bytes().as_slice()),
            )
            .map_err(|e| LsmImmutableDBError::Lsm(e.to_string()))?;

        // Reverse index: slot_hash:slot -> hash
        self.tree
            .insert(&make_slot_hash_key(slot), &Value::from(hash.as_bytes()))
            .map_err(|e| LsmImmutableDBError::Lsm(e.to_string()))?;

        if slot > self.tip_slot {
            debug!(slot = slot.0, "LsmImmutableDB: new tip slot");
            self.tip_slot = slot;
            let mut tip_value = Vec::with_capacity(48);
            tip_value.extend_from_slice(&slot.0.to_be_bytes());
            tip_value.extend_from_slice(hash.as_bytes());
            tip_value.extend_from_slice(&block_no.map_or(0u64, |b| b.0).to_be_bytes());
            self.tree
                .insert(&Key::from(META_TIP_KEY), &Value::from(tip_value.as_slice()))
                .map_err(|e| LsmImmutableDBError::Lsm(e.to_string()))?;
        }

        Ok(())
    }

    /// Get a block's raw CBOR by slot.
    pub fn get_block_by_slot(&self, slot: SlotNo) -> Result<Option<Vec<u8>>, LsmImmutableDBError> {
        match self.tree.get(&make_slot_key(slot)) {
            Ok(Some(value)) => Ok(Some(value.as_ref().to_vec())),
            Ok(None) => Ok(None),
            Err(e) => Err(LsmImmutableDBError::Lsm(e.to_string())),
        }
    }

    /// Check if a block exists by hash (only checks hash index).
    pub fn has_block(&self, hash: &BlockHeaderHash) -> bool {
        self.tree.get(&make_hash_key(hash)).ok().flatten().is_some()
    }

    /// Get a block's raw CBOR by hash.
    pub fn get_block_by_hash(
        &self,
        hash: &BlockHeaderHash,
    ) -> Result<Option<Vec<u8>>, LsmImmutableDBError> {
        match self.tree.get(&make_hash_key(hash)) {
            Ok(Some(slot_value)) => {
                let slot_bytes = slot_value.as_ref();
                if slot_bytes.len() != 8 {
                    return Ok(None);
                }
                let mut key_buf = [0u8; 13];
                key_buf[..5].copy_from_slice(b"slot:");
                key_buf[5..].copy_from_slice(slot_bytes);
                match self.tree.get(&Key::from(key_buf.as_slice())) {
                    Ok(Some(block_value)) => Ok(Some(block_value.as_ref().to_vec())),
                    Ok(None) => Ok(None),
                    Err(e) => Err(LsmImmutableDBError::Lsm(e.to_string())),
                }
            }
            Ok(None) => Ok(None),
            Err(e) => Err(LsmImmutableDBError::Lsm(e.to_string())),
        }
    }

    /// Get blocks in a slot range [from_slot, to_slot] inclusive.
    /// Returns raw CBOR block data in slot order.
    pub fn get_blocks_in_slot_range(
        &self,
        from_slot: SlotNo,
        to_slot: SlotNo,
    ) -> Result<Vec<Vec<u8>>, LsmImmutableDBError> {
        let start_key = make_slot_key(from_slot);
        let end_key = make_slot_key(to_slot);
        let mut blocks = Vec::new();

        let iter = self.tree.range(&start_key, &end_key);
        for (key, value) in iter {
            let key_bytes: &[u8] = key.as_ref();
            // Only process slot: keys (13 bytes: "slot:" + 8 bytes)
            if key_bytes.len() == 13 && key_bytes.starts_with(b"slot:") {
                blocks.push(value.as_ref().to_vec());
            }
        }
        Ok(blocks)
    }

    /// Get the first block after the given slot.
    /// Returns (slot, hash, cbor) of the next block, or None.
    pub fn get_next_block_after_slot(
        &self,
        after_slot: SlotNo,
    ) -> Result<Option<(SlotNo, BlockHeaderHash, Vec<u8>)>, LsmImmutableDBError> {
        let start_key = make_slot_key(SlotNo(after_slot.0 + 1));
        // Use a far-future end key to cover all remaining slots
        let end_key = make_slot_key(SlotNo(u64::MAX));

        let iter = self.tree.range(&start_key, &end_key);
        for (key, value) in iter {
            let key_bytes: &[u8] = key.as_ref();
            // Only process slot: keys
            if key_bytes.len() != 13 || !key_bytes.starts_with(b"slot:") {
                continue;
            }
            let mut slot_bytes = [0u8; 8];
            slot_bytes.copy_from_slice(&key_bytes[5..13]);
            let slot = SlotNo(u64::from_be_bytes(slot_bytes));

            // Look up the hash via slot_hash index
            let hash = match self.tree.get(&make_slot_hash_key(slot)) {
                Ok(Some(hash_value)) => {
                    let hb = hash_value.as_ref();
                    if hb.len() == 32 {
                        let mut h = [0u8; 32];
                        h.copy_from_slice(hb);
                        Hash32::from_bytes(h)
                    } else {
                        Hash32::from_bytes([0u8; 32])
                    }
                }
                _ => Hash32::from_bytes([0u8; 32]),
            };

            return Ok(Some((slot, hash, value.as_ref().to_vec())));
        }
        Ok(None)
    }

    pub fn tip_slot(&self) -> SlotNo {
        self.tip_slot
    }

    pub fn path(&self) -> &Path {
        &self.db_path
    }

    /// Get the tip slot, hash, and block number from persisted metadata.
    pub fn get_tip_info(&self) -> Option<(SlotNo, BlockHeaderHash, BlockNo)> {
        let value = self.tree.get(&Key::from(META_TIP_KEY)).ok()??;
        let data = value.as_ref();
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

    /// Recover the tip slot from the LSM tree on startup.
    fn recover_tip(tree: &LsmTree) -> SlotNo {
        match tree.get(&Key::from(META_TIP_KEY)) {
            Ok(Some(value)) => {
                let data = value.as_ref();
                if data.len() >= 8 {
                    SlotNo(u64::from_be_bytes(data[..8].try_into().unwrap_or([0; 8])))
                } else {
                    SlotNo(0)
                }
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
    fn test_lsm_immutable_db_put_get() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = LsmImmutableDB::open(dir.path()).unwrap();

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
    fn test_lsm_immutable_db_missing_block() {
        let dir = tempfile::tempdir().unwrap();
        let db = LsmImmutableDB::open(dir.path()).unwrap();

        let result = db.get_block_by_slot(SlotNo(999)).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_lsm_immutable_db_tip_updates() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = LsmImmutableDB::open(dir.path()).unwrap();

        assert_eq!(db.tip_slot(), SlotNo(0));

        db.put_block(SlotNo(50), &Hash32::from_bytes([1u8; 32]), b"block1")
            .unwrap();
        assert_eq!(db.tip_slot(), SlotNo(50));

        db.put_block(SlotNo(100), &Hash32::from_bytes([2u8; 32]), b"block2")
            .unwrap();
        assert_eq!(db.tip_slot(), SlotNo(100));
    }

    #[test]
    fn test_lsm_put_blocks_batch() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = LsmImmutableDB::open(dir.path()).unwrap();

        let hash1 = Hash32::from_bytes([1u8; 32]);
        let hash2 = Hash32::from_bytes([2u8; 32]);
        let hash3 = Hash32::from_bytes([3u8; 32]);

        let blocks = vec![
            (SlotNo(100), &hash1, BlockNo(10), b"block1".as_slice()),
            (SlotNo(200), &hash2, BlockNo(20), b"block2".as_slice()),
            (SlotNo(300), &hash3, BlockNo(30), b"block3".as_slice()),
        ];

        db.put_blocks_batch(&blocks).unwrap();

        // Verify all blocks stored
        assert_eq!(
            db.get_block_by_slot(SlotNo(100)).unwrap().as_deref(),
            Some(b"block1".as_slice())
        );
        assert_eq!(
            db.get_block_by_hash(&hash2).unwrap().as_deref(),
            Some(b"block2".as_slice())
        );
        assert_eq!(
            db.get_block_by_slot(SlotNo(300)).unwrap().as_deref(),
            Some(b"block3".as_slice())
        );

        // Tip should be the highest slot
        assert_eq!(db.tip_slot(), SlotNo(300));
        let tip_info = db.get_tip_info().unwrap();
        assert_eq!(tip_info.0, SlotNo(300));
        assert_eq!(tip_info.1, hash3);
        assert_eq!(tip_info.2, BlockNo(30));
    }

    #[test]
    fn test_lsm_put_blocks_batch_empty() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = LsmImmutableDB::open(dir.path()).unwrap();

        db.put_blocks_batch(&[]).unwrap();
        assert_eq!(db.tip_slot(), SlotNo(0));
    }

    #[test]
    fn test_lsm_has_block() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = LsmImmutableDB::open(dir.path()).unwrap();

        let hash = Hash32::from_bytes([1u8; 32]);
        let missing = Hash32::from_bytes([99u8; 32]);

        db.put_block(SlotNo(100), &hash, b"block1").unwrap();

        assert!(db.has_block(&hash));
        assert!(!db.has_block(&missing));
    }

    #[test]
    fn test_lsm_get_blocks_in_slot_range() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = LsmImmutableDB::open(dir.path()).unwrap();

        let hash1 = Hash32::from_bytes([1u8; 32]);
        let hash2 = Hash32::from_bytes([2u8; 32]);
        let hash3 = Hash32::from_bytes([3u8; 32]);

        db.put_block(SlotNo(100), &hash1, b"block1").unwrap();
        db.put_block(SlotNo(200), &hash2, b"block2").unwrap();
        db.put_block(SlotNo(300), &hash3, b"block3").unwrap();

        let blocks = db
            .get_blocks_in_slot_range(SlotNo(100), SlotNo(200))
            .unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0], b"block1");
        assert_eq!(blocks[1], b"block2");
    }

    #[test]
    fn test_lsm_get_next_block_after_slot() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = LsmImmutableDB::open(dir.path()).unwrap();

        let hash1 = Hash32::from_bytes([1u8; 32]);
        let hash2 = Hash32::from_bytes([2u8; 32]);

        db.put_block(SlotNo(100), &hash1, b"block1").unwrap();
        db.put_block(SlotNo(200), &hash2, b"block2").unwrap();

        let result = db.get_next_block_after_slot(SlotNo(0)).unwrap();
        assert!(result.is_some());
        let (slot, hash, cbor) = result.unwrap();
        assert_eq!(slot, SlotNo(100));
        assert_eq!(hash, hash1);
        assert_eq!(cbor, b"block1");

        let result = db.get_next_block_after_slot(SlotNo(100)).unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().0, SlotNo(200));

        let result = db.get_next_block_after_slot(SlotNo(200)).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_lsm_tip_with_blockno() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = LsmImmutableDB::open(dir.path()).unwrap();

        let hash = Hash32::from_bytes([42u8; 32]);
        db.put_block_with_blockno(SlotNo(500), &hash, BlockNo(100), b"block")
            .unwrap();

        assert_eq!(db.tip_slot(), SlotNo(500));
        let tip_info = db.get_tip_info().unwrap();
        assert_eq!(tip_info.0, SlotNo(500));
        assert_eq!(tip_info.1, hash);
        assert_eq!(tip_info.2, BlockNo(100));
    }
}
