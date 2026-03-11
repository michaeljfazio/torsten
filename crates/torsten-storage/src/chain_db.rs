//! Unified ChainDB backed by a single `cardano-lsm` LSM tree.
//!
//! All blocks (volatile and immutable) are stored in one LSM tree.
//! Chain rollbacks use cardano-lsm's native `snapshot()`/`rollback()`
//! mechanism for atomic, O(1) state restoration.

use cardano_lsm::{CompactionStrategy, Key, LsmConfig, LsmSnapshot, LsmTree, Value};
use std::path::{Path, PathBuf};
use thiserror::Error;
use torsten_primitives::block::{Point, Tip};
use torsten_primitives::hash::{BlockHeaderHash, Hash32};
use torsten_primitives::time::{BlockNo, SlotNo};
use tracing::{debug, info, trace, warn};

use crate::immutable_db::ImmutableDB;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum ChainDBError {
    #[error("LSM error: {0}")]
    Lsm(#[from] cardano_lsm::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Block not found: {0}")]
    BlockNotFound(String),
    #[error("Corrupt tip metadata ({0} bytes, expected {TIP_METADATA_SIZE})")]
    CorruptTipMetadata(usize),
    #[error("ImmutableDB error: {0}")]
    Immutable(#[from] crate::immutable_db::ImmutableDBError),
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// The security parameter k (number of blocks before immutability)
pub const SECURITY_PARAM_K: usize = 2160;

/// Build the default hybrid compaction strategy.
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
fn bulk_import_config() -> LsmConfig {
    LsmConfig {
        memtable_size: 256 * 1024 * 1024,
        block_cache_size: 256 * 1024 * 1024,
        bloom_filter_bits_per_key: 10,
        level0_compaction_trigger: usize::MAX,
        compaction_strategy: default_compaction_strategy(),
        ..LsmConfig::default()
    }
}

// ---------------------------------------------------------------------------
// Key encoding
// ---------------------------------------------------------------------------

// Key prefixes for the LSM tree:
//
// | Prefix        | Len  | Payload          | Value                   |
// |---------------|------|------------------|-------------------------|
// | blk:          | 36   | 32-byte hash     | Block CBOR (primary)    |
// | hash:         | 37   | 32-byte hash     | 8-byte BE slot          |
// | slot_hash:    | 18   | 8-byte BE slot   | 32-byte hash            |
// | prev:         | 37   | 32-byte hash     | 32-byte prev_hash       |
// | blkn:         | 13   | 8-byte BE blk_no | 8-byte BE slot          |
// | meta:tip      |  8   | —                | TipMetadata (48 B)      |

/// Primary CBOR storage key — indexed by block hash (no slot collisions).
#[inline]
fn make_blk_key(hash: &BlockHeaderHash) -> Key {
    let mut buf = [0u8; 36];
    buf[..4].copy_from_slice(b"blk:");
    buf[4..].copy_from_slice(hash.as_bytes());
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

#[inline]
fn make_prev_hash_key(hash: &BlockHeaderHash) -> Key {
    let mut buf = [0u8; 37];
    buf[..5].copy_from_slice(b"prev:");
    buf[5..].copy_from_slice(hash.as_bytes());
    Key::from(buf.as_slice())
}

#[inline]
fn make_block_no_key(block_no: BlockNo) -> Key {
    let mut buf = [0u8; 13];
    buf[..5].copy_from_slice(b"blkn:");
    buf[5..].copy_from_slice(&block_no.0.to_be_bytes());
    Key::from(buf.as_slice())
}

const META_TIP_KEY: &[u8] = b"meta:tip";

// ---------------------------------------------------------------------------
// Tip metadata
// ---------------------------------------------------------------------------

const TIP_METADATA_SIZE: usize = 48;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TipMetadata {
    slot: SlotNo,
    hash: BlockHeaderHash,
    block_no: BlockNo,
}

impl TipMetadata {
    fn encode(&self) -> [u8; TIP_METADATA_SIZE] {
        let mut buf = [0u8; TIP_METADATA_SIZE];
        buf[..8].copy_from_slice(&self.slot.0.to_be_bytes());
        buf[8..40].copy_from_slice(self.hash.as_bytes());
        buf[40..48].copy_from_slice(&self.block_no.0.to_be_bytes());
        buf
    }

    fn decode(data: &[u8]) -> Result<Self, ChainDBError> {
        if data.len() < TIP_METADATA_SIZE {
            return Err(ChainDBError::CorruptTipMetadata(data.len()));
        }
        let slot = SlotNo(u64::from_be_bytes(data[..8].try_into().unwrap()));
        let mut hash_bytes = [0u8; 32];
        hash_bytes.copy_from_slice(&data[8..40]);
        let block_no = BlockNo(u64::from_be_bytes(data[40..48].try_into().unwrap()));
        Ok(TipMetadata {
            slot,
            hash: Hash32::from_bytes(hash_bytes),
            block_no,
        })
    }
}

// ---------------------------------------------------------------------------
// ChainDB
// ---------------------------------------------------------------------------

/// Unified block storage backed by a single `cardano-lsm` LSM tree.
///
/// All blocks are stored in one tree. Rollbacks use cardano-lsm's native
/// `snapshot()`/`rollback()` for atomic state restoration.
///
/// **Persistence model**: cardano-lsm uses *ephemeral writes* — all inserts
/// live in the memtable until [`persist()`](Self::persist) is called, which
/// saves a durable snapshot. On [`open()`](Self::open), the tree is restored
/// from the most recent snapshot.
pub struct ChainDB {
    #[allow(dead_code)]
    path: PathBuf,
    tree: LsmTree,
    tip: Option<TipMetadata>,
    /// In-memory snapshot for rollback support.
    /// Taken periodically so we can restore to a known-good state.
    rollback_snapshot: Option<RollbackSnapshot>,
    /// Optional read-only chunk-file storage for historical blocks.
    /// Present after Mithril snapshot import places chunk files in `immutable/`.
    immutable: Option<ImmutableDB>,
}

/// A snapshot that can be rolled back to, along with the chain tip at
/// the time the snapshot was taken.
struct RollbackSnapshot {
    snapshot: LsmSnapshot,
    tip: Option<TipMetadata>,
}

impl ChainDB {
    /// Open or create a ChainDB at the given path.
    pub fn open(db_path: &Path) -> Result<Self, ChainDBError> {
        info!(path = %db_path.display(), k = SECURITY_PARAM_K, "Opening ChainDB");
        std::fs::create_dir_all(db_path)?;

        let tree = open_tree(db_path, normal_config())?;
        let tip = recover_tip(&tree);

        if let Some(ref t) = tip {
            info!(
                slot = t.slot.0,
                block_no = t.block_no.0,
                "ChainDB recovered tip"
            );
        }

        // Open ImmutableDB if chunk files directory exists
        let immutable_dir = db_path.join("immutable");
        let immutable = if immutable_dir.is_dir() {
            match ImmutableDB::open(&immutable_dir) {
                Ok(imm) if imm.total_blocks() > 0 => {
                    info!(
                        blocks = imm.total_blocks(),
                        tip_slot = imm.tip_slot(),
                        "ImmutableDB available (chunk files)"
                    );
                    Some(imm)
                }
                Ok(_) => None,
                Err(e) => {
                    warn!(error = %e, "Failed to open ImmutableDB, continuing without it");
                    None
                }
            }
        } else {
            None
        };

        let mut db = ChainDB {
            path: db_path.to_path_buf(),
            tree,
            tip,
            rollback_snapshot: None,
            immutable,
        };

        // Take an initial snapshot for rollback support
        db.take_rollback_snapshot();

        info!("ChainDB opened successfully");
        Ok(db)
    }

    /// Open with settings optimised for bulk import (e.g. Mithril snapshot).
    pub fn open_for_bulk_import(path: &Path) -> Result<Self, ChainDBError> {
        info!(path = %path.display(), "Opening ChainDB for bulk import");
        std::fs::create_dir_all(path)?;

        let tree = LsmTree::open(path, bulk_import_config())?;
        let tip = recover_tip(&tree);

        if let Some(ref t) = tip {
            info!(slot = t.slot.0, "ChainDB recovered tip (bulk import mode)");
        }

        info!("ChainDB opened for bulk import (compaction deferred)");
        Ok(ChainDB {
            path: path.to_path_buf(),
            tree,
            tip,
            rollback_snapshot: None,
            immutable: None,
        })
    }

    // -- Writes -------------------------------------------------------------

    /// Store a new block.
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

        let mut batch = Vec::with_capacity(6);
        batch.push((make_blk_key(&hash), Value::from(cbor.as_slice())));
        batch.push((
            make_hash_key(&hash),
            Value::from(slot.0.to_be_bytes().as_slice()),
        ));
        batch.push((make_slot_hash_key(slot), Value::from(hash.as_bytes())));
        batch.push((make_prev_hash_key(&hash), Value::from(prev_hash.as_bytes())));
        batch.push((
            make_block_no_key(block_no),
            Value::from(slot.0.to_be_bytes().as_slice()),
        ));

        if self.tip.is_none_or(|t| slot > t.slot) {
            let meta = TipMetadata {
                slot,
                hash,
                block_no,
            };
            batch.push((
                Key::from(META_TIP_KEY),
                Value::from(meta.encode().as_slice()),
            ));
            self.tip = Some(meta);
        }

        self.tree.insert_batch(batch)?;
        Ok(())
    }

    /// Store multiple blocks in a batch.
    pub fn add_blocks_batch(
        &mut self,
        blocks: Vec<(BlockHeaderHash, SlotNo, BlockNo, BlockHeaderHash, Vec<u8>)>,
    ) -> Result<(), ChainDBError> {
        if blocks.is_empty() {
            return Ok(());
        }

        let mut batch: Vec<(Key, Value)> = Vec::with_capacity(blocks.len() * 5 + 1);

        for (hash, slot, block_no, prev_hash, cbor) in &blocks {
            // Skip blocks that already exist
            if self.has_block(hash) {
                trace!(
                    hash = %hash.to_hex(),
                    slot = slot.0,
                    "ChainDB: block already exists, skipping"
                );
                continue;
            }

            batch.push((make_blk_key(hash), Value::from(cbor.as_slice())));
            batch.push((
                make_hash_key(hash),
                Value::from(slot.0.to_be_bytes().as_slice()),
            ));
            batch.push((make_slot_hash_key(*slot), Value::from(hash.as_bytes())));
            batch.push((make_prev_hash_key(hash), Value::from(prev_hash.as_bytes())));
            batch.push((
                make_block_no_key(*block_no),
                Value::from(slot.0.to_be_bytes().as_slice()),
            ));

            if self.tip.is_none_or(|t| *slot > t.slot) {
                self.tip = Some(TipMetadata {
                    slot: *slot,
                    hash: *hash,
                    block_no: *block_no,
                });
            }
        }

        // Write tip metadata
        if let Some(meta) = self.tip {
            batch.push((
                Key::from(META_TIP_KEY),
                Value::from(meta.encode().as_slice()),
            ));
        }

        if !batch.is_empty() {
            self.tree.insert_batch(batch)?;
        }

        Ok(())
    }

    /// Store multiple blocks for bulk import (no duplicate check, no prev_hash).
    /// Used by Mithril import where blocks are known to be unique and ordered.
    pub fn put_blocks_batch(
        &mut self,
        blocks: &[(SlotNo, &BlockHeaderHash, BlockNo, &[u8])],
    ) -> Result<(), ChainDBError> {
        if blocks.is_empty() {
            return Ok(());
        }

        let mut batch: Vec<(Key, Value)> = Vec::with_capacity(blocks.len() * 4 + 1);
        let mut new_tip: Option<TipMetadata> = None;

        for &(slot, hash, block_no, cbor) in blocks {
            batch.push((make_blk_key(hash), Value::from(cbor)));
            batch.push((
                make_hash_key(hash),
                Value::from(slot.0.to_be_bytes().as_slice()),
            ));
            batch.push((make_slot_hash_key(slot), Value::from(hash.as_bytes())));
            batch.push((
                make_block_no_key(block_no),
                Value::from(slot.0.to_be_bytes().as_slice()),
            ));

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

    /// Get block CBOR by hash.
    pub fn get_block(&self, hash: &BlockHeaderHash) -> Result<Option<Vec<u8>>, ChainDBError> {
        // Try LSM first (recent/volatile blocks)
        if let Some(v) = self.tree.get(&make_blk_key(hash))? {
            return Ok(Some(v.as_ref().to_vec()));
        }
        // Fall back to ImmutableDB (historical blocks from chunk files)
        if let Some(ref imm) = self.immutable {
            if let Some(cbor) = imm.get_block(hash) {
                return Ok(Some(cbor));
            }
        }
        Ok(None)
    }

    /// Get the current chain tip.
    ///
    /// Returns the higher of the LSM tip (volatile/recent blocks) and
    /// the ImmutableDB tip (historical chunk files).
    pub fn get_tip(&self) -> Tip {
        let lsm_tip = self.tip.map(|t| Tip {
            point: Point::Specific(t.slot, t.hash),
            block_number: t.block_no,
        });

        let imm_tip = self.immutable.as_ref().and_then(|imm| {
            if imm.total_blocks() > 0 {
                Some(Tip {
                    point: Point::Specific(SlotNo(imm.tip_slot()), imm.tip_hash()),
                    block_number: BlockNo(imm.total_blocks()),
                })
            } else {
                None
            }
        });

        match (lsm_tip, imm_tip) {
            (Some(lsm), Some(imm)) => {
                if lsm.point.slot().unwrap_or(SlotNo(0)) >= imm.point.slot().unwrap_or(SlotNo(0)) {
                    lsm
                } else {
                    imm
                }
            }
            (Some(t), None) | (None, Some(t)) => t,
            (None, None) => Tip::origin(),
        }
    }

    /// Get the tip info (slot, hash, block_no) if available.
    pub fn get_tip_info(&self) -> Option<(SlotNo, BlockHeaderHash, BlockNo)> {
        self.tip.map(|t| (t.slot, t.hash, t.block_no))
    }

    /// Check if a block exists by hash (bloom-filter accelerated, no CBOR read).
    pub fn has_block(&self, hash: &BlockHeaderHash) -> bool {
        if self.tree.get(&make_hash_key(hash)).ok().flatten().is_some() {
            return true;
        }
        if let Some(ref imm) = self.immutable {
            return imm.has_block(hash);
        }
        false
    }

    /// Get block CBOR by block number.
    ///
    /// O(1) lookup using the `blkn:` index. Used for sequential replay
    /// after Mithril import to avoid expensive range scans.
    pub fn get_block_by_number(
        &self,
        block_no: BlockNo,
    ) -> Result<Option<(SlotNo, BlockHeaderHash, Vec<u8>)>, ChainDBError> {
        // blkn -> slot
        let slot_value = match self.tree.get(&make_block_no_key(block_no))? {
            Some(v) => v,
            None => return Ok(None),
        };
        let slot_bytes: &[u8] = slot_value.as_ref();
        if slot_bytes.len() != 8 {
            return Ok(None);
        }
        let slot = SlotNo(u64::from_be_bytes(slot_bytes.try_into().unwrap()));

        // slot -> hash
        let hash = self
            .tree
            .get(&make_slot_hash_key(slot))?
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

        // hash -> CBOR (via blk: key, immune to slot collision)
        let cbor = match self.tree.get(&make_blk_key(&hash))? {
            Some(v) => v.as_ref().to_vec(),
            None => return Ok(None),
        };

        Ok(Some((slot, hash, cbor)))
    }

    /// Get blocks in a slot range `[from, to]` inclusive, in slot order.
    pub fn get_blocks_in_slot_range(
        &self,
        from_slot: SlotNo,
        to_slot: SlotNo,
    ) -> Result<Vec<Vec<u8>>, ChainDBError> {
        let mut blocks = Vec::new();

        // Get from ImmutableDB first (historical blocks, lower slots)
        if let Some(ref imm) = self.immutable {
            blocks.extend(imm.get_blocks_in_slot_range(from_slot.0, to_slot.0));
        }

        // Get from LSM (recent/volatile blocks)
        let iter = self
            .tree
            .range(&make_slot_hash_key(from_slot), &make_slot_hash_key(to_slot));
        for (key, value) in iter {
            let kb: &[u8] = key.as_ref();
            if kb.len() == 18 && kb.starts_with(b"slot_hash:") {
                let hb = value.as_ref();
                if hb.len() == 32 {
                    let mut h = [0u8; 32];
                    h.copy_from_slice(hb);
                    let hash = Hash32::from_bytes(h);
                    if let Some(cbor) = self.tree.get(&make_blk_key(&hash))? {
                        blocks.push(cbor.as_ref().to_vec());
                    }
                }
            }
        }
        Ok(blocks)
    }

    /// Get the first block strictly after `after_slot`.
    pub fn get_next_block_after_slot(
        &self,
        after_slot: SlotNo,
    ) -> Result<Option<(SlotNo, BlockHeaderHash, Vec<u8>)>, ChainDBError> {
        // Try ImmutableDB first (historical blocks)
        let imm_result = self.immutable.as_ref().and_then(|imm| {
            imm.get_next_block_after_slot(after_slot.0)
                .map(|(s, h, cbor)| (SlotNo(s), h, cbor))
        });

        // Try LSM (recent/volatile blocks)
        let lsm_result = {
            let start = make_slot_hash_key(SlotNo(after_slot.0.saturating_add(1)));
            let end = make_slot_hash_key(SlotNo(u64::MAX));
            let mut found = None;
            for (key, value) in self.tree.range(&start, &end) {
                let kb: &[u8] = key.as_ref();
                if kb.len() != 18 || !kb.starts_with(b"slot_hash:") {
                    continue;
                }
                let slot = SlotNo(u64::from_be_bytes(kb[10..18].try_into().unwrap()));
                let hb = value.as_ref();
                if hb.len() != 32 {
                    continue;
                }
                let mut h = [0u8; 32];
                h.copy_from_slice(hb);
                let hash = Hash32::from_bytes(h);
                if let Some(cbor) = self.tree.get(&make_blk_key(&hash))? {
                    found = Some((slot, hash, cbor.as_ref().to_vec()));
                    break;
                }
            }
            found
        };

        // Return whichever has the lower (earlier) slot
        match (imm_result, lsm_result) {
            (Some((is, ih, ic)), Some((ls, lh, lc))) => {
                if is <= ls {
                    Ok(Some((is, ih, ic)))
                } else {
                    Ok(Some((ls, lh, lc)))
                }
            }
            (Some(r), None) | (None, Some(r)) => Ok(Some(r)),
            (None, None) => Ok(None),
        }
    }

    // -- Rollback -----------------------------------------------------------

    /// Take a snapshot of the current state for future rollback.
    ///
    /// Should be called periodically (e.g. after each batch of blocks is
    /// applied to the ledger) so that `rollback_to_point()` can restore
    /// to this state.
    pub fn take_rollback_snapshot(&mut self) {
        let snapshot = self.tree.snapshot();
        self.rollback_snapshot = Some(RollbackSnapshot {
            snapshot,
            tip: self.tip,
        });
        debug!(
            tip_slot = self.tip.map_or(0, |t| t.slot.0),
            "ChainDB: rollback snapshot taken"
        );
    }

    /// Rollback the chain to a given point.
    ///
    /// If we have a rollback snapshot, atomically restores the LSM tree state.
    /// Returns the hashes of the removed blocks (most recent first).
    pub fn rollback_to_point(
        &mut self,
        point: &Point,
    ) -> Result<Vec<BlockHeaderHash>, ChainDBError> {
        warn!(point = ?point, "ChainDB: rollback requested");

        let current_tip = self.tip;
        let target_slot = point.slot().map(|s| s.0).unwrap_or(0);
        let target_hash = point.hash().copied();

        // Check if rollback is a no-op
        if let Some(t) = current_tip {
            if let Some(th) = target_hash {
                if t.hash == th {
                    return Ok(vec![]);
                }
            }
        }

        // Collect hashes of blocks that will be removed (for caller's use)
        let mut removed = Vec::new();
        if let Some(tip_meta) = current_tip {
            // Walk backwards from tip collecting blocks to remove
            let mut current_hash = tip_meta.hash;
            loop {
                // If we've reached the target, stop
                if let Some(th) = target_hash {
                    if current_hash == th {
                        break;
                    }
                }

                // If we've gone past origin, stop
                if current_hash == Hash32::ZERO {
                    break;
                }

                // Check if this block's slot is at or before the target
                if let Ok(Some(slot_val)) = self.tree.get(&make_hash_key(&current_hash)) {
                    let slot_bytes: &[u8] = slot_val.as_ref();
                    if slot_bytes.len() == 8 {
                        let block_slot = u64::from_be_bytes(slot_bytes.try_into().unwrap());
                        if block_slot <= target_slot && target_hash.is_none() {
                            break;
                        }
                    }
                }

                removed.push(current_hash);

                // Follow prev_hash chain
                match self.tree.get(&make_prev_hash_key(&current_hash)) {
                    Ok(Some(prev_val)) => {
                        let pb: &[u8] = prev_val.as_ref();
                        if pb.len() == 32 {
                            let mut h = [0u8; 32];
                            h.copy_from_slice(pb);
                            current_hash = Hash32::from_bytes(h);
                        } else {
                            break;
                        }
                    }
                    _ => break,
                }
            }
        }

        // Restore from snapshot if available
        if let Some(rb) = self.rollback_snapshot.take() {
            info!(
                blocks_removed = removed.len(),
                "ChainDB: rolling back via LSM snapshot"
            );
            self.tree.rollback(rb.snapshot)?;
            self.tip = rb.tip;

            // Take a fresh snapshot after rollback
            self.take_rollback_snapshot();
        } else {
            // No snapshot available — delete blocks individually via tombstones
            warn!("ChainDB: no rollback snapshot available, using tombstone deletion");
            for hash in &removed {
                if let Ok(Some(slot_val)) = self.tree.get(&make_hash_key(hash)) {
                    let slot_bytes: &[u8] = slot_val.as_ref();
                    if slot_bytes.len() == 8 {
                        let slot = SlotNo(u64::from_be_bytes(slot_bytes.try_into().unwrap()));
                        let _ = self.tree.delete(&make_slot_hash_key(slot));
                    }
                }
                let _ = self.tree.delete(&make_blk_key(hash));
                let _ = self.tree.delete(&make_hash_key(hash));
                let _ = self.tree.delete(&make_prev_hash_key(hash));
            }

            // Update tip to the rollback target
            if let Some(th) = target_hash {
                if let Ok(Some(slot_val)) = self.tree.get(&make_hash_key(&th)) {
                    let slot_bytes: &[u8] = slot_val.as_ref();
                    if slot_bytes.len() == 8 {
                        let slot = SlotNo(u64::from_be_bytes(slot_bytes.try_into().unwrap()));
                        // Derive block_no: old tip block_no minus removed count
                        let new_block_no = current_tip
                            .map(|t| BlockNo(t.block_no.0.saturating_sub(removed.len() as u64)))
                            .unwrap_or(BlockNo(0));
                        self.tip = Some(TipMetadata {
                            slot,
                            hash: th,
                            block_no: new_block_no,
                        });
                        let meta = self.tip.unwrap();
                        let _ = self.tree.insert(
                            &Key::from(META_TIP_KEY),
                            &Value::from(meta.encode().as_slice()),
                        );
                    }
                }
            } else {
                self.tip = None;
            }

            self.take_rollback_snapshot();
        }

        info!(
            blocks_removed = removed.len(),
            new_tip_slot = self.tip.map_or(0, |t| t.slot.0),
            "ChainDB: rollback complete"
        );

        Ok(removed)
    }

    // -- Lifecycle ----------------------------------------------------------

    /// Persist all in-memory data to a durable snapshot.
    ///
    /// Uses save-to-temp-then-rename to avoid data loss if the process
    /// crashes between deleting the old snapshot and writing the new one.
    pub fn persist(&mut self) -> Result<(), ChainDBError> {
        info!("ChainDB: persisting snapshot");
        // Save to a temporary name first
        let _ = self.tree.delete_snapshot("latest_tmp");
        self.tree.save_snapshot("latest_tmp", "torsten")?;

        // Atomically replace the old "latest" with the new one via directory rename
        let snapshots_dir = self.path.join("snapshots");
        let latest_dir = snapshots_dir.join("latest");
        let tmp_dir = snapshots_dir.join("latest_tmp");

        if latest_dir.exists() {
            // Remove old latest — if we crash here, latest_tmp is a valid recovery
            std::fs::remove_dir_all(&latest_dir)?;
        }
        // Rename tmp → latest
        std::fs::rename(&tmp_dir, &latest_dir)?;

        info!("ChainDB: snapshot persisted");
        Ok(())
    }

    /// Trigger a full compaction of all SSTables.
    pub fn compact(&mut self) {
        if let Err(e) = self.tree.compact_all() {
            warn!(error = %e, "compaction failed");
            return;
        }
        if let Err(e) = self.persist() {
            warn!(error = %e, "persist after compaction failed");
        }
    }

    /// Current tip slot (convenience).
    pub fn tip_slot(&self) -> SlotNo {
        let lsm_slot = self.tip.map_or(0, |t| t.slot.0);
        let imm_slot = self.immutable.as_ref().map_or(0, |imm| imm.tip_slot());
        SlotNo(lsm_slot.max(imm_slot))
    }

    /// Whether this ChainDB has an ImmutableDB with chunk files.
    pub fn has_immutable(&self) -> bool {
        self.immutable.is_some()
    }

    /// Get the ImmutableDB directory path, if available.
    pub fn immutable_dir(&self) -> Option<&Path> {
        self.immutable.as_ref().map(|imm| imm.dir())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Open (or restore) an LSM tree from a persisted snapshot.
fn open_tree(path: &Path, config: LsmConfig) -> Result<LsmTree, ChainDBError> {
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
    if data.len() >= TIP_METADATA_SIZE {
        TipMetadata::decode(data).ok()
    } else if data.len() >= 40 {
        // Support legacy 40-byte format (without block_no) gracefully
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

        // Take a snapshot at block 5 (already taken at open)
        // The snapshot was taken at open (before any blocks).
        // We need a snapshot that includes blocks 1-3 to rollback to 3.
        // Actually, the snapshot/rollback approach restores to the snapshot state.
        // For this test, let's take a snapshot after block 3.

        // Re-do: build in two phases
        let dir = tempfile::tempdir().unwrap();
        let mut db = ChainDB::open(dir.path()).unwrap();

        // Phase 1: blocks 1-3
        for i in 1..=3u8 {
            db.add_block(
                make_hash(i),
                SlotNo(i as u64),
                BlockNo(i as u64),
                make_hash(i - 1),
                format!("block{i}").into_bytes(),
            )
            .unwrap();
        }

        // Snapshot after block 3
        db.take_rollback_snapshot();

        // Phase 2: blocks 4-5
        for i in 4..=5u8 {
            db.add_block(
                make_hash(i),
                SlotNo(i as u64),
                BlockNo(i as u64),
                make_hash(i - 1),
                format!("block{i}").into_bytes(),
            )
            .unwrap();
        }
        assert_eq!(db.get_tip().block_number, BlockNo(5));

        // Rollback to block 3
        let removed = db
            .rollback_to_point(&Point::Specific(SlotNo(3), make_hash(3)))
            .unwrap();

        assert_eq!(removed.len(), 2); // blocks 5 and 4
        assert_eq!(removed[0], make_hash(5));
        assert_eq!(removed[1], make_hash(4));
        assert_eq!(db.get_tip().block_number, BlockNo(3));

        // Blocks 4 and 5 should be gone (rolled back)
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

        // Snapshot at open (empty state)
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
    fn test_persist_and_recover() {
        let dir = tempfile::tempdir().unwrap();
        let hash = make_hash(42);

        {
            let mut db = ChainDB::open(dir.path()).unwrap();
            db.add_block(
                hash,
                SlotNo(500),
                BlockNo(100),
                Hash32::ZERO,
                b"block".to_vec(),
            )
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
    fn test_slot_collision_returns_correct_block() {
        // Two blocks at the same slot (from different forks) should
        // both be retrievable by their hash via the blk: key.
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

        // Both blocks should return their OWN data, not each other's
        let a_data = db.get_block(&hash_a).unwrap().unwrap();
        assert_eq!(a_data, b"block_A", "hash_a should return block_A data");

        let b_data = db.get_block(&hash_b).unwrap().unwrap();
        assert_eq!(b_data, b"block_B", "hash_b should return block_B data");
    }

    #[test]
    fn test_get_block_by_number_uses_blk_key() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = ChainDB::open(dir.path()).unwrap();

        let hash = make_hash(42);
        db.add_block(
            hash,
            SlotNo(500),
            BlockNo(100),
            Hash32::ZERO,
            b"my_block".to_vec(),
        )
        .unwrap();

        let result = db.get_block_by_number(BlockNo(100)).unwrap().unwrap();
        assert_eq!(result.0, SlotNo(500));
        assert_eq!(result.1, hash);
        assert_eq!(result.2, b"my_block");
    }
}
