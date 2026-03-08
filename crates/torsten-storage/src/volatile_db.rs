use parking_lot::RwLock;
use std::collections::{BTreeMap, HashMap};
use thiserror::Error;
use torsten_primitives::block::{Point, Tip};
use torsten_primitives::hash::BlockHeaderHash;
use torsten_primitives::time::{BlockNo, SlotNo};
use tracing::{debug, trace, warn};

#[derive(Error, Debug)]
pub enum VolatileDBError {
    #[error("Block not found: {0}")]
    BlockNotFound(String),
    #[error("Block already exists: {0}")]
    BlockAlreadyExists(String),
    #[error("Invalid chain: {0}")]
    InvalidChain(String),
}

/// Entry in the volatile DB
#[allow(dead_code)]
#[derive(Debug, Clone)]
struct VolatileEntry {
    slot: SlotNo,
    block_no: BlockNo,
    prev_hash: BlockHeaderHash,
    cbor: Vec<u8>,
}

/// VolatileDB stores recent blocks that may still be rolled back
///
/// This covers the last k blocks (security parameter, k=2160 on mainnet).
/// These blocks are kept in memory for fast access and rollback support.
pub struct VolatileDB {
    /// Blocks indexed by header hash
    blocks: RwLock<HashMap<BlockHeaderHash, VolatileEntry>>,
    /// Slot index for efficient lookup
    slot_index: RwLock<BTreeMap<SlotNo, Vec<BlockHeaderHash>>>,
    /// Current chain tip
    tip: RwLock<Option<(BlockHeaderHash, SlotNo, BlockNo)>>,
    /// Maximum number of blocks to keep
    max_blocks: usize,
}

impl VolatileDB {
    pub fn new(max_blocks: usize) -> Self {
        VolatileDB {
            blocks: RwLock::new(HashMap::new()),
            slot_index: RwLock::new(BTreeMap::new()),
            tip: RwLock::new(None),
            max_blocks,
        }
    }

    /// Store a new block
    pub fn put_block(
        &self,
        hash: BlockHeaderHash,
        slot: SlotNo,
        block_no: BlockNo,
        prev_hash: BlockHeaderHash,
        cbor: Vec<u8>,
    ) -> Result<(), VolatileDBError> {
        let cbor_len = cbor.len();
        let mut blocks = self.blocks.write();
        if blocks.contains_key(&hash) {
            debug!(hash = %hash.to_hex(), "VolatileDB: block already exists, skipping");
            return Err(VolatileDBError::BlockAlreadyExists(hash.to_hex()));
        }

        blocks.insert(
            hash,
            VolatileEntry {
                slot,
                block_no,
                prev_hash,
                cbor,
            },
        );

        self.slot_index.write().entry(slot).or_default().push(hash);

        // Update tip
        let mut tip = self.tip.write();
        let should_update = match &*tip {
            None => true,
            Some((_, _, current_block_no)) => block_no > *current_block_no,
        };
        if should_update {
            trace!(
                hash = %hash.to_hex(),
                slot = slot.0,
                block_no = block_no.0,
                cbor_bytes = cbor_len,
                total_blocks = blocks.len(),
                "VolatileDB: new tip"
            );
            *tip = Some((hash, slot, block_no));
        }

        // Garbage collect old blocks if needed
        if blocks.len() > self.max_blocks {
            debug!(
                count = blocks.len(),
                max = self.max_blocks,
                "VolatileDB: garbage collecting oldest blocks"
            );
            self.gc_oldest(&mut blocks);
        }

        Ok(())
    }

    /// Check if a block exists by hash (no data copy)
    pub fn has_block(&self, hash: &BlockHeaderHash) -> bool {
        self.blocks.read().contains_key(hash)
    }

    /// Get block CBOR by hash
    pub fn get_block(&self, hash: &BlockHeaderHash) -> Option<Vec<u8>> {
        self.blocks.read().get(hash).map(|e| e.cbor.clone())
    }

    /// Get blocks at a specific slot
    pub fn get_blocks_at_slot(&self, slot: SlotNo) -> Vec<BlockHeaderHash> {
        self.slot_index
            .read()
            .get(&slot)
            .cloned()
            .unwrap_or_default()
    }

    /// Get blocks in a slot range [from_slot, to_slot] inclusive.
    /// Returns raw CBOR block data in slot order.
    pub fn get_blocks_in_slot_range(&self, from_slot: SlotNo, to_slot: SlotNo) -> Vec<Vec<u8>> {
        let blocks = self.blocks.read();
        let slot_index = self.slot_index.read();
        let mut result = Vec::new();

        for (_, hashes) in slot_index.range(from_slot..=to_slot) {
            for hash in hashes {
                if let Some(entry) = blocks.get(hash) {
                    result.push(entry.cbor.clone());
                }
            }
        }
        result
    }

    /// Get the first block after the given slot.
    /// Returns (slot, hash, cbor) of the next block, or None if no block exists after this slot.
    pub fn get_next_block_after_slot(
        &self,
        after_slot: SlotNo,
    ) -> Option<(SlotNo, BlockHeaderHash, Vec<u8>)> {
        let blocks = self.blocks.read();
        let slot_index = self.slot_index.read();

        // Use BTreeMap range to find the first slot strictly after `after_slot`
        for (&slot, hashes) in slot_index.range((
            std::ops::Bound::Excluded(after_slot),
            std::ops::Bound::Unbounded,
        )) {
            if let Some(hash) = hashes.first() {
                if let Some(entry) = blocks.get(hash) {
                    return Some((slot, *hash, entry.cbor.clone()));
                }
            }
        }
        None
    }

    /// Get the current tip
    pub fn get_tip(&self) -> Option<Tip> {
        self.tip.read().map(|(hash, slot, block_no)| Tip {
            point: Point::Specific(slot, hash),
            block_number: block_no,
        })
    }

    /// Get the previous hash for a block
    pub fn get_prev_hash(&self, hash: &BlockHeaderHash) -> Option<BlockHeaderHash> {
        self.blocks.read().get(hash).map(|e| e.prev_hash)
    }

    /// Get the chain of hashes back to a given point (for rollback)
    pub fn get_chain_back_to(
        &self,
        from: &BlockHeaderHash,
        to: &BlockHeaderHash,
    ) -> Option<Vec<BlockHeaderHash>> {
        let blocks = self.blocks.read();
        let mut chain = Vec::new();
        let mut current = *from;

        loop {
            if current == *to {
                return Some(chain);
            }
            chain.push(current);
            match blocks.get(&current) {
                Some(entry) => current = entry.prev_hash,
                None => return None,
            }
        }
    }

    /// Remove a block (used during rollback)
    pub fn remove_block(&self, hash: &BlockHeaderHash) -> Option<Vec<u8>> {
        let mut blocks = self.blocks.write();
        if let Some(entry) = blocks.remove(hash) {
            debug!(
                hash = %hash.to_hex(),
                slot = entry.slot.0,
                block_no = entry.block_no.0,
                "VolatileDB: removing block (rollback)"
            );
            let mut slot_index = self.slot_index.write();
            if let Some(hashes) = slot_index.get_mut(&entry.slot) {
                hashes.retain(|h| h != hash);
                if hashes.is_empty() {
                    slot_index.remove(&entry.slot);
                }
            }
            Some(entry.cbor)
        } else {
            warn!(hash = %hash.to_hex(), "VolatileDB: block not found for removal");
            None
        }
    }

    /// Update the tip to point to a specific block hash (used during rollback)
    pub fn update_tip_to(&self, hash: &BlockHeaderHash) {
        let blocks = self.blocks.read();
        if let Some(entry) = blocks.get(hash) {
            *self.tip.write() = Some((*hash, entry.slot, entry.block_no));
        } else {
            // Block not in volatile DB — tip goes to None (origin)
            *self.tip.write() = None;
        }
    }

    /// Drain the oldest blocks, returning their data for flushing to immutable DB.
    /// Returns Vec of (hash, slot, block_no, cbor).
    pub fn drain_oldest(&self, count: usize) -> Vec<(BlockHeaderHash, SlotNo, BlockNo, Vec<u8>)> {
        debug!(count, "VolatileDB: draining oldest blocks");
        let mut result = Vec::new();
        let mut blocks = self.blocks.write();
        let mut slot_index = self.slot_index.write();

        let mut drained = 0;
        while drained < count {
            if let Some((&oldest_slot, _)) = slot_index.iter().next() {
                if let Some(hashes) = slot_index.remove(&oldest_slot) {
                    for hash in hashes {
                        if let Some(entry) = blocks.remove(&hash) {
                            result.push((hash, entry.slot, entry.block_no, entry.cbor));
                            drained += 1;
                        }
                    }
                }
            } else {
                break;
            }
        }

        result
    }

    /// Store multiple blocks in a single lock acquisition.
    /// More efficient than calling put_block repeatedly — acquires each lock only once.
    /// Skips blocks that already exist (idempotent).
    pub fn put_blocks_batch(
        &self,
        batch: Vec<(BlockHeaderHash, SlotNo, BlockNo, BlockHeaderHash, Vec<u8>)>,
    ) -> usize {
        if batch.is_empty() {
            return 0;
        }

        let mut blocks = self.blocks.write();
        let mut slot_index = self.slot_index.write();
        let mut tip = self.tip.write();
        let mut inserted = 0;

        for (hash, slot, block_no, prev_hash, cbor) in batch {
            if blocks.contains_key(&hash) {
                debug!(hash = %hash.to_hex(), "VolatileDB: block already exists (batch), skipping");
                continue;
            }

            blocks.insert(
                hash,
                VolatileEntry {
                    slot,
                    block_no,
                    prev_hash,
                    cbor,
                },
            );

            slot_index.entry(slot).or_default().push(hash);

            let should_update = match &*tip {
                None => true,
                Some((_, _, current_block_no)) => block_no > *current_block_no,
            };
            if should_update {
                *tip = Some((hash, slot, block_no));
            }

            inserted += 1;
        }

        // Garbage collect if needed
        if blocks.len() > self.max_blocks {
            debug!(
                count = blocks.len(),
                max = self.max_blocks,
                "VolatileDB: garbage collecting oldest blocks (batch)"
            );
            // Inline GC since we already hold the slot_index lock
            while blocks.len() > self.max_blocks {
                if let Some((&oldest_slot, _)) = slot_index.iter().next() {
                    if let Some(hashes) = slot_index.remove(&oldest_slot) {
                        for hash in hashes {
                            blocks.remove(&hash);
                        }
                    }
                } else {
                    break;
                }
            }
        }

        trace!(inserted, "VolatileDB: batch insert complete");
        inserted
    }

    pub fn block_count(&self) -> usize {
        self.blocks.read().len()
    }

    fn gc_oldest(&self, blocks: &mut HashMap<BlockHeaderHash, VolatileEntry>) {
        let mut slot_index = self.slot_index.write();
        while blocks.len() > self.max_blocks {
            if let Some((&oldest_slot, _)) = slot_index.iter().next() {
                if let Some(hashes) = slot_index.remove(&oldest_slot) {
                    for hash in hashes {
                        blocks.remove(&hash);
                    }
                }
            } else {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use torsten_primitives::hash::Hash32;

    fn make_hash(n: u8) -> BlockHeaderHash {
        Hash32::from_bytes([n; 32])
    }

    #[test]
    fn test_put_and_get() {
        let vdb = VolatileDB::new(100);
        let hash = make_hash(1);
        let prev = make_hash(0);

        vdb.put_block(hash, SlotNo(100), BlockNo(50), prev, b"block1".to_vec())
            .unwrap();

        assert_eq!(vdb.get_block(&hash).unwrap(), b"block1");
        assert_eq!(vdb.block_count(), 1);
    }

    #[test]
    fn test_duplicate_block() {
        let vdb = VolatileDB::new(100);
        let hash = make_hash(1);
        let prev = make_hash(0);

        vdb.put_block(hash, SlotNo(100), BlockNo(50), prev, b"block1".to_vec())
            .unwrap();
        let result = vdb.put_block(hash, SlotNo(100), BlockNo(50), prev, b"block1".to_vec());
        assert!(result.is_err());
    }

    #[test]
    fn test_tip_tracking() {
        let vdb = VolatileDB::new(100);

        vdb.put_block(
            make_hash(1),
            SlotNo(100),
            BlockNo(50),
            make_hash(0),
            b"b1".to_vec(),
        )
        .unwrap();
        let tip = vdb.get_tip().unwrap();
        assert_eq!(tip.block_number, BlockNo(50));

        vdb.put_block(
            make_hash(2),
            SlotNo(200),
            BlockNo(51),
            make_hash(1),
            b"b2".to_vec(),
        )
        .unwrap();
        let tip = vdb.get_tip().unwrap();
        assert_eq!(tip.block_number, BlockNo(51));
    }

    #[test]
    fn test_slot_index() {
        let vdb = VolatileDB::new(100);

        vdb.put_block(
            make_hash(1),
            SlotNo(100),
            BlockNo(50),
            make_hash(0),
            b"b1".to_vec(),
        )
        .unwrap();
        vdb.put_block(
            make_hash(2),
            SlotNo(100),
            BlockNo(50),
            make_hash(0),
            b"b2".to_vec(),
        )
        .unwrap();

        let blocks_at_100 = vdb.get_blocks_at_slot(SlotNo(100));
        assert_eq!(blocks_at_100.len(), 2);
    }

    #[test]
    fn test_remove_block() {
        let vdb = VolatileDB::new(100);
        let hash = make_hash(1);

        vdb.put_block(hash, SlotNo(100), BlockNo(50), make_hash(0), b"b1".to_vec())
            .unwrap();
        assert_eq!(vdb.block_count(), 1);

        let removed = vdb.remove_block(&hash);
        assert!(removed.is_some());
        assert_eq!(vdb.block_count(), 0);
        assert!(vdb.get_block(&hash).is_none());
    }

    #[test]
    fn test_chain_back_to() {
        let vdb = VolatileDB::new(100);

        // Build a chain: 0 <- 1 <- 2 <- 3
        vdb.put_block(
            make_hash(1),
            SlotNo(1),
            BlockNo(1),
            make_hash(0),
            b"b1".to_vec(),
        )
        .unwrap();
        vdb.put_block(
            make_hash(2),
            SlotNo(2),
            BlockNo(2),
            make_hash(1),
            b"b2".to_vec(),
        )
        .unwrap();
        vdb.put_block(
            make_hash(3),
            SlotNo(3),
            BlockNo(3),
            make_hash(2),
            b"b3".to_vec(),
        )
        .unwrap();

        let chain = vdb.get_chain_back_to(&make_hash(3), &make_hash(1)).unwrap();
        assert_eq!(chain.len(), 2); // [3, 2]
        assert_eq!(chain[0], make_hash(3));
        assert_eq!(chain[1], make_hash(2));
    }

    #[test]
    fn test_garbage_collection() {
        let vdb = VolatileDB::new(3);

        for i in 1..=5u8 {
            vdb.put_block(
                make_hash(i),
                SlotNo(i as u64),
                BlockNo(i as u64),
                make_hash(i - 1),
                format!("block{}", i).into_bytes(),
            )
            .unwrap();
        }

        assert!(vdb.block_count() <= 3);
    }

    #[test]
    fn test_get_next_block_after_slot() {
        let vdb = VolatileDB::new(100);

        // Add blocks at slots 10, 20, 30
        for (i, slot) in [10u64, 20, 30].iter().enumerate() {
            vdb.put_block(
                make_hash((i + 1) as u8),
                SlotNo(*slot),
                BlockNo(i as u64 + 1),
                make_hash(i as u8),
                format!("block{}", slot).into_bytes(),
            )
            .unwrap();
        }

        // Next after slot 0 should be slot 10
        let result = vdb.get_next_block_after_slot(SlotNo(0));
        assert!(result.is_some());
        let (slot, hash, _) = result.unwrap();
        assert_eq!(slot, SlotNo(10));
        assert_eq!(hash, make_hash(1));

        // Next after slot 10 should be slot 20
        let result = vdb.get_next_block_after_slot(SlotNo(10));
        assert!(result.is_some());
        assert_eq!(result.unwrap().0, SlotNo(20));

        // Next after slot 15 should be slot 20
        let result = vdb.get_next_block_after_slot(SlotNo(15));
        assert!(result.is_some());
        assert_eq!(result.unwrap().0, SlotNo(20));

        // Next after slot 30 should be None
        let result = vdb.get_next_block_after_slot(SlotNo(30));
        assert!(result.is_none());
    }

    #[test]
    fn test_put_blocks_batch() {
        let vdb = VolatileDB::new(100);

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

        let inserted = vdb.put_blocks_batch(batch);
        assert_eq!(inserted, 3);
        assert_eq!(vdb.block_count(), 3);

        // Verify all blocks are stored
        assert_eq!(vdb.get_block(&make_hash(1)).unwrap(), b"b1");
        assert_eq!(vdb.get_block(&make_hash(2)).unwrap(), b"b2");
        assert_eq!(vdb.get_block(&make_hash(3)).unwrap(), b"b3");

        // Verify tip is the highest block
        let tip = vdb.get_tip().unwrap();
        assert_eq!(tip.block_number, BlockNo(3));
    }

    #[test]
    fn test_put_blocks_batch_skips_duplicates() {
        let vdb = VolatileDB::new(100);

        // Insert first block individually
        vdb.put_block(
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

        let inserted = vdb.put_blocks_batch(batch);
        assert_eq!(inserted, 1); // Only block 2 was new
        assert_eq!(vdb.block_count(), 2);

        // Original block 1 data should be unchanged
        assert_eq!(vdb.get_block(&make_hash(1)).unwrap(), b"b1");
    }

    #[test]
    fn test_put_blocks_batch_gc() {
        let vdb = VolatileDB::new(3);

        let batch: Vec<_> = (1..=5u8)
            .map(|i| {
                (
                    make_hash(i),
                    SlotNo(i as u64),
                    BlockNo(i as u64),
                    make_hash(i - 1),
                    format!("block{}", i).into_bytes(),
                )
            })
            .collect();

        vdb.put_blocks_batch(batch);
        assert!(vdb.block_count() <= 3);
    }
}
