//! LSM-tree based ImmutableDB backend using `cardano-lsm`.
//!
//! Uses the `cardano-lsm` crate — a pure Rust LSM tree designed for
//! Cardano blockchain indexing workloads. On Linux, enable the `io-uring`
//! feature for async I/O with batched reads during compaction.

use std::path::{Path, PathBuf};
use thiserror::Error;
use torsten_primitives::hash::{BlockHeaderHash, Hash32};
use torsten_primitives::time::{BlockNo, SlotNo};
use tracing::{debug, info, trace, warn};

use cardano_lsm::{CompactionStrategy, Key, LsmConfig, LsmTree, Value};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum LsmImmutableDBError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("LSM error: {0}")]
    Lsm(#[from] cardano_lsm::Error),
    #[error("Block not found: {0}")]
    BlockNotFound(String),
    #[error("Corrupt tip metadata ({0} bytes, expected {TIP_METADATA_SIZE})")]
    CorruptTipMetadata(usize),
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Build the default hybrid compaction strategy shared across all open modes.
fn default_compaction_strategy() -> CompactionStrategy {
    CompactionStrategy::Hybrid {
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
    }
}

/// Standard configuration for normal node operation.
fn normal_config() -> LsmConfig {
    LsmConfig {
        memtable_size: 128 * 1024 * 1024,    // 128 MB write buffer
        block_cache_size: 256 * 1024 * 1024, // 256 MB read cache
        bloom_filter_bits_per_key: 10,
        compaction_strategy: default_compaction_strategy(),
        ..LsmConfig::default()
    }
}

/// Configuration optimised for bulk import (e.g. Mithril snapshot).
///
/// Raises `level0_compaction_trigger` to defer automatic compaction and
/// increases the memtable to 256 MB.  Call [`LsmImmutableDB::compact`] once
/// after the import finishes to consolidate all SSTables.
fn bulk_import_config() -> LsmConfig {
    LsmConfig {
        memtable_size: 256 * 1024 * 1024, // 256 MB write buffer
        block_cache_size: 256 * 1024 * 1024,
        bloom_filter_bits_per_key: 10,
        level0_compaction_trigger: usize::MAX, // defer all automatic compaction
        compaction_strategy: default_compaction_strategy(),
        ..LsmConfig::default()
    }
}

// ---------------------------------------------------------------------------
// Key encoding
// ---------------------------------------------------------------------------

// Key prefixes — every entry stored in the LSM tree uses one of these.
//
// | Prefix        | Len  | Payload          | Value                   |
// |---------------|------|------------------|-------------------------|
// | slot:         | 13   | 8-byte BE slot   | Block CBOR              |
// | hash:         | 37   | 32-byte hash     | 8-byte BE slot          |
// | slot_hash:    | 18   | 8-byte BE slot   | 32-byte hash            |
// | meta:tip      |  8   | —                | TipMetadata (48 B)      |

#[inline]
fn make_slot_key(slot: SlotNo) -> Key {
    let mut buf = [0u8; 13];
    buf[..5].copy_from_slice(b"slot:");
    buf[5..].copy_from_slice(&slot.0.to_be_bytes());
    Key::from(buf.as_slice())
}

#[inline]
fn make_hash_key(hash: &BlockHeaderHash) -> Key {
    let mut buf = [0u8; 37];
    buf[..5].copy_from_slice(b"hash:");
    buf[5..].copy_from_slice(hash.as_bytes());
    Key::from(buf.as_slice())
}

#[inline]
fn make_slot_hash_key(slot: SlotNo) -> Key {
    let mut buf = [0u8; 18];
    buf[..10].copy_from_slice(b"slot_hash:");
    buf[10..].copy_from_slice(&slot.0.to_be_bytes());
    Key::from(buf.as_slice())
}

const META_TIP_KEY: &[u8] = b"meta:tip";

// ---------------------------------------------------------------------------
// Tip metadata
// ---------------------------------------------------------------------------

/// Fixed-size binary layout for the chain tip persisted inside the LSM tree.
///
/// ```text
/// [ slot: u64 BE | hash: 32 bytes | block_no: u64 BE ]   = 48 bytes
/// ```
const TIP_METADATA_SIZE: usize = 48;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TipMetadata {
    pub slot: SlotNo,
    pub hash: BlockHeaderHash,
    pub block_no: BlockNo,
}

impl TipMetadata {
    fn encode(&self) -> [u8; TIP_METADATA_SIZE] {
        let mut buf = [0u8; TIP_METADATA_SIZE];
        buf[..8].copy_from_slice(&self.slot.0.to_be_bytes());
        buf[8..40].copy_from_slice(self.hash.as_bytes());
        buf[40..48].copy_from_slice(&self.block_no.0.to_be_bytes());
        buf
    }

    fn decode(data: &[u8]) -> Result<Self, LsmImmutableDBError> {
        if data.len() < TIP_METADATA_SIZE {
            return Err(LsmImmutableDBError::CorruptTipMetadata(data.len()));
        }
        let slot =
            SlotNo(u64::from_be_bytes(data[..8].try_into().map_err(|_| {
                LsmImmutableDBError::CorruptTipMetadata(data.len())
            })?));
        let mut hash_bytes = [0u8; 32];
        hash_bytes.copy_from_slice(&data[8..40]);
        let block_no =
            BlockNo(u64::from_be_bytes(data[40..48].try_into().map_err(
                |_| LsmImmutableDBError::CorruptTipMetadata(data.len()),
            )?));
        Ok(TipMetadata {
            slot,
            hash: Hash32::from_bytes(hash_bytes),
            block_no,
        })
    }
}

// ---------------------------------------------------------------------------
// LsmImmutableDB
// ---------------------------------------------------------------------------

/// ImmutableDB backed by `cardano-lsm` LSM tree.
///
/// Provides:
/// - Block storage keyed by slot number
/// - Secondary index: block hash -> slot
/// - Reverse index: slot -> block hash
/// - Tip metadata persistence
///
/// **Persistence model**: cardano-lsm uses *ephemeral writes* — all inserts
/// live in an in-memory memtable until [`persist()`](Self::persist) is called,
/// which saves a durable snapshot.  On [`open()`](Self::open), the tree is
/// automatically restored from the most recent snapshot.
pub struct LsmImmutableDB {
    path: PathBuf,
    tree: LsmTree,
    tip: Option<TipMetadata>,
}

impl LsmImmutableDB {
    /// Open or create an LSM-backed ImmutableDB at the given path.
    ///
    /// If a `"latest"` snapshot exists (from a previous [`persist()`](Self::persist)
    /// call), the tree is restored from that snapshot.
    pub fn open(path: &Path) -> Result<Self, LsmImmutableDBError> {
        info!(path = %path.display(), "Opening ImmutableDB (cardano-lsm)");
        std::fs::create_dir_all(path)?;

        let tree = open_tree(path, normal_config())?;
        let tip = recover_tip(&tree);

        if let Some(ref t) = tip {
            info!(
                slot = t.slot.0,
                block_no = t.block_no.0,
                "ImmutableDB recovered tip"
            );
        }

        info!("ImmutableDB opened successfully");
        Ok(LsmImmutableDB {
            path: path.to_path_buf(),
            tree,
            tip,
        })
    }

    /// Open with settings optimised for bulk import (e.g. Mithril snapshot).
    ///
    /// Defers automatic compaction during the import and increases the write
    /// buffer.  Call [`compact()`](Self::compact) once after the import
    /// finishes to consolidate all SSTables.
    pub fn open_for_bulk_import(path: &Path) -> Result<Self, LsmImmutableDBError> {
        info!(path = %path.display(), "Opening ImmutableDB for bulk import");
        std::fs::create_dir_all(path)?;

        let tree = LsmTree::open(path, bulk_import_config())?;
        let tip = recover_tip(&tree);

        if let Some(ref t) = tip {
            info!(
                slot = t.slot.0,
                "ImmutableDB recovered tip (bulk import mode)"
            );
        }

        info!("ImmutableDB opened for bulk import (compaction deferred)");
        Ok(LsmImmutableDB {
            path: path.to_path_buf(),
            tree,
            tip,
        })
    }

    // -- Writes -------------------------------------------------------------

    /// Store a single block (slot + hash + block_no + CBOR) atomically.
    pub fn put_block(
        &mut self,
        slot: SlotNo,
        hash: &BlockHeaderHash,
        block_no: BlockNo,
        cbor: &[u8],
    ) -> Result<(), LsmImmutableDBError> {
        trace!(slot = slot.0, hash = %hash.to_hex(), bytes = cbor.len(), "storing block");

        let mut batch = Vec::with_capacity(4);
        batch.push((make_slot_key(slot), Value::from(cbor)));
        batch.push((
            make_hash_key(hash),
            Value::from(slot.0.to_be_bytes().as_slice()),
        ));
        batch.push((make_slot_hash_key(slot), Value::from(hash.as_bytes())));

        if self.tip.is_none_or(|t| slot > t.slot) {
            let meta = TipMetadata {
                slot,
                hash: *hash,
                block_no,
            };
            batch.push((
                Key::from(META_TIP_KEY),
                Value::from(meta.encode().as_slice()),
            ));
            self.tip = Some(meta);
            debug!(slot = slot.0, "new tip slot");
        }

        self.tree.insert_batch(batch)?;
        Ok(())
    }

    /// Store multiple blocks atomically using a single batch insert.
    pub fn put_blocks_batch(
        &mut self,
        blocks: &[(SlotNo, &BlockHeaderHash, BlockNo, &[u8])],
    ) -> Result<(), LsmImmutableDBError> {
        if blocks.is_empty() {
            return Ok(());
        }

        let mut batch: Vec<(Key, Value)> = Vec::with_capacity(blocks.len() * 3 + 1);
        let mut new_tip: Option<TipMetadata> = None;

        for &(slot, hash, block_no, cbor) in blocks {
            batch.push((make_slot_key(slot), Value::from(cbor)));
            batch.push((
                make_hash_key(hash),
                Value::from(slot.0.to_be_bytes().as_slice()),
            ));
            batch.push((make_slot_hash_key(slot), Value::from(hash.as_bytes())));

            let dominated = match (&new_tip, &self.tip) {
                (Some(nt), _) => slot > nt.slot,
                (None, Some(t)) => slot > t.slot,
                (None, None) => true,
            };
            if dominated {
                new_tip = Some(TipMetadata {
                    slot,
                    hash: *hash,
                    block_no,
                });
            }
        }

        if let Some(meta) = new_tip {
            batch.push((
                Key::from(META_TIP_KEY),
                Value::from(meta.encode().as_slice()),
            ));
            self.tip = Some(meta);
        }

        self.tree.insert_batch(batch)?;
        Ok(())
    }

    // -- Reads --------------------------------------------------------------

    /// Get a block's raw CBOR by slot.
    pub fn get_block_by_slot(&self, slot: SlotNo) -> Result<Option<Vec<u8>>, LsmImmutableDBError> {
        Ok(self
            .tree
            .get(&make_slot_key(slot))?
            .map(|v| v.as_ref().to_vec()))
    }

    /// Get a block's raw CBOR by hash (two-hop: hash -> slot -> CBOR).
    pub fn get_block_by_hash(
        &self,
        hash: &BlockHeaderHash,
    ) -> Result<Option<Vec<u8>>, LsmImmutableDBError> {
        let slot_value = match self.tree.get(&make_hash_key(hash))? {
            Some(v) => v,
            None => return Ok(None),
        };
        let slot_bytes: &[u8] = slot_value.as_ref();
        let slot_arr: [u8; 8] = match slot_bytes.try_into() {
            Ok(arr) => arr,
            Err(_) => return Ok(None),
        };
        let slot = SlotNo(u64::from_be_bytes(slot_arr));
        self.get_block_by_slot(slot)
    }

    /// Check if a block exists by hash (bloom-filter accelerated, no CBOR read).
    pub fn has_block(&self, hash: &BlockHeaderHash) -> bool {
        self.tree.get(&make_hash_key(hash)).ok().flatten().is_some()
    }

    /// Get blocks in a slot range `[from, to]` inclusive, in slot order.
    pub fn get_blocks_in_slot_range(
        &self,
        from_slot: SlotNo,
        to_slot: SlotNo,
    ) -> Result<Vec<Vec<u8>>, LsmImmutableDBError> {
        let iter = self
            .tree
            .range(&make_slot_key(from_slot), &make_slot_key(to_slot));
        let mut blocks = Vec::new();
        for (key, value) in iter {
            let kb: &[u8] = key.as_ref();
            if kb.len() == 13 && kb.starts_with(b"slot:") {
                blocks.push(value.as_ref().to_vec());
            }
        }
        Ok(blocks)
    }

    /// Get the first block strictly after `after_slot`.
    ///
    /// Returns `(slot, hash, cbor)` or `None` if no block exists beyond that slot.
    pub fn get_next_block_after_slot(
        &self,
        after_slot: SlotNo,
    ) -> Result<Option<(SlotNo, BlockHeaderHash, Vec<u8>)>, LsmImmutableDBError> {
        let start = make_slot_key(SlotNo(after_slot.0.saturating_add(1)));
        let end = make_slot_key(SlotNo(u64::MAX));

        for (key, value) in self.tree.range(&start, &end) {
            let kb: &[u8] = key.as_ref();
            if kb.len() != 13 || !kb.starts_with(b"slot:") {
                continue;
            }
            let slot_arr: [u8; 8] = match kb[5..13].try_into() {
                Ok(arr) => arr,
                Err(_) => continue,
            };
            let slot = SlotNo(u64::from_be_bytes(slot_arr));
            let hash = self
                .tree
                .get(&make_slot_hash_key(slot))
                .ok()
                .flatten()
                .map(|v| {
                    let hb = v.as_ref();
                    if hb.len() == 32 {
                        let mut h = [0u8; 32];
                        h.copy_from_slice(hb);
                        Hash32::from_bytes(h)
                    } else {
                        Hash32::ZERO
                    }
                })
                .unwrap_or(Hash32::ZERO);

            return Ok(Some((slot, hash, value.as_ref().to_vec())));
        }
        Ok(None)
    }

    // -- Tip ----------------------------------------------------------------

    /// Cached tip slot (in-memory, no tree lookup).
    pub fn tip_slot(&self) -> SlotNo {
        self.tip.map_or(SlotNo(0), |t| t.slot)
    }

    /// Full tip metadata (slot, hash, block_no) if available.
    pub fn get_tip_info(&self) -> Option<(SlotNo, BlockHeaderHash, BlockNo)> {
        self.tip.map(|t| (t.slot, t.hash, t.block_no))
    }

    // -- Lifecycle ----------------------------------------------------------

    /// Filesystem path of this database.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Persist all in-memory data to a durable snapshot.
    ///
    /// Must be called before dropping the `LsmImmutableDB` to avoid data loss,
    /// because cardano-lsm uses ephemeral writes.
    pub fn persist(&mut self) -> Result<(), LsmImmutableDBError> {
        info!("ImmutableDB: persisting snapshot");
        // Save to temp name first to avoid data loss on crash
        let _ = self.tree.delete_snapshot("latest_tmp");
        self.tree.save_snapshot("latest_tmp", "torsten")?;

        // Atomically replace old "latest" with new one
        let snapshots_dir = self.path.join("snapshots");
        let latest_dir = snapshots_dir.join("latest");
        let tmp_dir = snapshots_dir.join("latest_tmp");
        if latest_dir.exists() {
            std::fs::remove_dir_all(&latest_dir)
                .map_err(|e| LsmImmutableDBError::Lsm(cardano_lsm::Error::Io(e)))?;
        }
        std::fs::rename(&tmp_dir, &latest_dir)
            .map_err(|e| LsmImmutableDBError::Lsm(cardano_lsm::Error::Io(e)))?;

        info!("ImmutableDB: snapshot persisted");
        Ok(())
    }

    /// Trigger a full compaction of all SSTables.
    ///
    /// Runs `compact_all` to consolidate all SSTables into a single sorted
    /// run, then persists a new snapshot.
    pub fn compact(&mut self) {
        if let Err(e) = self.tree.compact_all() {
            warn!(error = %e, "compaction failed");
            return;
        }
        if let Err(e) = self.persist() {
            warn!(error = %e, "persist after compaction failed");
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Open (or restore) an LSM tree.
///
/// If a `"latest"` snapshot exists (from a previous [`persist()`] call),
/// the tree is restored via `open_snapshot`.  Otherwise a fresh tree is
/// created with `open`.  If the snapshot is corrupt, it is removed and we
/// fall back to a fresh open.
fn open_tree(path: &Path, config: LsmConfig) -> Result<LsmTree, LsmImmutableDBError> {
    let snapshot_dir = path.join("snapshots").join("latest");
    if snapshot_dir.exists() {
        match LsmTree::open_snapshot(path, "latest") {
            Ok(tree) => {
                info!("Restored from persisted snapshot");
                return Ok(tree);
            }
            Err(e) => {
                warn!(error = %e, "Failed to open snapshot, removing and retrying");
                if let Err(rm_err) = std::fs::remove_dir_all(&snapshot_dir) {
                    warn!(error = %rm_err, "Failed to remove corrupted snapshot dir");
                }
            }
        }
    }
    Ok(LsmTree::open(path, config)?)
}

/// Read tip metadata from the tree (used once during open).
fn recover_tip(tree: &LsmTree) -> Option<TipMetadata> {
    let value = tree.get(&Key::from(META_TIP_KEY)).ok()??;
    let data = value.as_ref();
    // Support legacy 40-byte format (without block_no) gracefully
    if data.len() >= TIP_METADATA_SIZE {
        TipMetadata::decode(data).ok()
    } else if data.len() >= 40 {
        let slot = SlotNo(u64::from_be_bytes(data[..8].try_into().ok()?));
        let mut hash_bytes = [0u8; 32];
        hash_bytes.copy_from_slice(&data[8..40]);
        Some(TipMetadata {
            slot,
            hash: Hash32::from_bytes(hash_bytes),
            block_no: BlockNo(0),
        })
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use torsten_primitives::hash::Hash32;

    #[test]
    fn test_put_get_by_slot() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = LsmImmutableDB::open(dir.path()).unwrap();

        let slot = SlotNo(100);
        let hash = Hash32::from_bytes([1u8; 32]);

        db.put_block(slot, &hash, BlockNo(10), b"fake block data")
            .unwrap();

        assert_eq!(
            db.get_block_by_slot(slot).unwrap().as_deref(),
            Some(b"fake block data".as_slice())
        );
    }

    #[test]
    fn test_put_get_by_hash() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = LsmImmutableDB::open(dir.path()).unwrap();

        let hash = Hash32::from_bytes([1u8; 32]);
        db.put_block(SlotNo(100), &hash, BlockNo(10), b"block data")
            .unwrap();

        assert_eq!(
            db.get_block_by_hash(&hash).unwrap().as_deref(),
            Some(b"block data".as_slice())
        );
    }

    #[test]
    fn test_missing_block() {
        let dir = tempfile::tempdir().unwrap();
        let db = LsmImmutableDB::open(dir.path()).unwrap();
        assert!(db.get_block_by_slot(SlotNo(999)).unwrap().is_none());
    }

    #[test]
    fn test_tip_updates() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = LsmImmutableDB::open(dir.path()).unwrap();

        assert_eq!(db.tip_slot(), SlotNo(0));

        db.put_block(
            SlotNo(50),
            &Hash32::from_bytes([1u8; 32]),
            BlockNo(5),
            b"b1",
        )
        .unwrap();
        assert_eq!(db.tip_slot(), SlotNo(50));

        db.put_block(
            SlotNo(100),
            &Hash32::from_bytes([2u8; 32]),
            BlockNo(10),
            b"b2",
        )
        .unwrap();
        assert_eq!(db.tip_slot(), SlotNo(100));
    }

    #[test]
    fn test_batch_insert() {
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

        assert_eq!(db.tip_slot(), SlotNo(300));
        let (slot, hash, block_no) = db.get_tip_info().unwrap();
        assert_eq!(slot, SlotNo(300));
        assert_eq!(hash, hash3);
        assert_eq!(block_no, BlockNo(30));
    }

    #[test]
    fn test_batch_empty() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = LsmImmutableDB::open(dir.path()).unwrap();
        db.put_blocks_batch(&[]).unwrap();
        assert_eq!(db.tip_slot(), SlotNo(0));
    }

    #[test]
    fn test_has_block() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = LsmImmutableDB::open(dir.path()).unwrap();

        let hash = Hash32::from_bytes([1u8; 32]);
        let missing = Hash32::from_bytes([99u8; 32]);

        db.put_block(SlotNo(100), &hash, BlockNo(10), b"block1")
            .unwrap();

        assert!(db.has_block(&hash));
        assert!(!db.has_block(&missing));
    }

    #[test]
    fn test_slot_range() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = LsmImmutableDB::open(dir.path()).unwrap();

        db.put_block(
            SlotNo(100),
            &Hash32::from_bytes([1u8; 32]),
            BlockNo(1),
            b"b1",
        )
        .unwrap();
        db.put_block(
            SlotNo(200),
            &Hash32::from_bytes([2u8; 32]),
            BlockNo(2),
            b"b2",
        )
        .unwrap();
        db.put_block(
            SlotNo(300),
            &Hash32::from_bytes([3u8; 32]),
            BlockNo(3),
            b"b3",
        )
        .unwrap();

        let blocks = db
            .get_blocks_in_slot_range(SlotNo(100), SlotNo(200))
            .unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0], b"b1");
        assert_eq!(blocks[1], b"b2");
    }

    #[test]
    fn test_next_block_after_slot() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = LsmImmutableDB::open(dir.path()).unwrap();

        let hash1 = Hash32::from_bytes([1u8; 32]);
        let hash2 = Hash32::from_bytes([2u8; 32]);

        db.put_block(SlotNo(100), &hash1, BlockNo(1), b"b1")
            .unwrap();
        db.put_block(SlotNo(200), &hash2, BlockNo(2), b"b2")
            .unwrap();

        let (slot, hash, cbor) = db.get_next_block_after_slot(SlotNo(0)).unwrap().unwrap();
        assert_eq!(slot, SlotNo(100));
        assert_eq!(hash, hash1);
        assert_eq!(cbor, b"b1");

        let result = db.get_next_block_after_slot(SlotNo(100)).unwrap().unwrap();
        assert_eq!(result.0, SlotNo(200));

        assert!(db.get_next_block_after_slot(SlotNo(200)).unwrap().is_none());
    }

    #[test]
    fn test_tip_metadata_roundtrip() {
        let meta = TipMetadata {
            slot: SlotNo(500),
            hash: Hash32::from_bytes([42u8; 32]),
            block_no: BlockNo(100),
        };
        let encoded = meta.encode();
        let decoded = TipMetadata::decode(&encoded).unwrap();
        assert_eq!(meta, decoded);
    }

    #[test]
    fn test_persist_and_recover() {
        let dir = tempfile::tempdir().unwrap();
        let hash = Hash32::from_bytes([42u8; 32]);

        {
            let mut db = LsmImmutableDB::open(dir.path()).unwrap();
            db.put_block(SlotNo(500), &hash, BlockNo(100), b"block")
                .unwrap();
            db.persist().unwrap();
        }

        // Re-open and verify tip is recovered
        let db = LsmImmutableDB::open(dir.path()).unwrap();
        assert_eq!(db.tip_slot(), SlotNo(500));
        let (slot, h, block_no) = db.get_tip_info().unwrap();
        assert_eq!(slot, SlotNo(500));
        assert_eq!(h, hash);
        assert_eq!(block_no, BlockNo(100));
    }
}
