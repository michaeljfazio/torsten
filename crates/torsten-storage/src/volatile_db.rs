//! In-memory volatile block storage.
//!
//! Stores the last k blocks (and any forks) in memory. Lost on crash —
//! re-fetched from peers in seconds. This is the simplest possible
//! implementation matching Haskell cardano-node's VolatileDB semantics.

use std::collections::{BTreeMap, HashMap};
use torsten_primitives::hash::Hash32;

/// A block stored in the volatile DB.
#[derive(Debug, Clone)]
pub struct VolatileBlock {
    pub slot: u64,
    pub block_no: u64,
    pub prev_hash: Hash32,
    pub cbor: Vec<u8>,
}

/// In-memory store for recent (volatile) blocks.
///
/// These blocks are within k of the tip and not yet finalized to
/// the ImmutableDB. On crash, they are re-fetched from peers.
pub struct VolatileDB {
    blocks: HashMap<Hash32, VolatileBlock>,
    slot_index: BTreeMap<u64, Vec<Hash32>>,
    block_no_index: BTreeMap<u64, Hash32>,
    successors: HashMap<Hash32, Vec<Hash32>>,
    tip: Option<(u64, Hash32, u64)>, // (slot, hash, block_no)
}

impl VolatileDB {
    pub fn new() -> Self {
        VolatileDB {
            blocks: HashMap::new(),
            slot_index: BTreeMap::new(),
            block_no_index: BTreeMap::new(),
            successors: HashMap::new(),
            tip: None,
        }
    }

    /// Add a block to the volatile store.
    pub fn add_block(
        &mut self,
        hash: Hash32,
        slot: u64,
        block_no: u64,
        prev_hash: Hash32,
        cbor: Vec<u8>,
    ) {
        // Track successor relationship
        self.successors.entry(prev_hash).or_default().push(hash);

        // Update indexes
        self.slot_index.entry(slot).or_default().push(hash);
        self.block_no_index.insert(block_no, hash);

        // Store the block
        self.blocks.insert(
            hash,
            VolatileBlock {
                slot,
                block_no,
                prev_hash,
                cbor,
            },
        );

        // Update tip if this is the highest slot
        let is_new_tip = match self.tip {
            Some((tip_slot, _, _)) => slot > tip_slot,
            None => true,
        };
        if is_new_tip {
            self.tip = Some((slot, hash, block_no));
        }
    }

    /// Get a block by hash.
    pub fn get_block(&self, hash: &Hash32) -> Option<&VolatileBlock> {
        self.blocks.get(hash)
    }

    /// Get block CBOR by hash.
    pub fn get_block_cbor(&self, hash: &Hash32) -> Option<&[u8]> {
        self.blocks.get(hash).map(|b| b.cbor.as_slice())
    }

    /// Check if a block exists.
    pub fn has_block(&self, hash: &Hash32) -> bool {
        self.blocks.contains_key(hash)
    }

    /// Get the first block strictly after a given slot.
    pub fn get_next_block_after_slot(&self, after_slot: u64) -> Option<(u64, Hash32, &[u8])> {
        for (&slot, hashes) in self.slot_index.range((after_slot + 1)..) {
            if let Some(hash) = hashes.first() {
                if let Some(block) = self.blocks.get(hash) {
                    return Some((slot, *hash, &block.cbor));
                }
            }
        }
        None
    }

    /// Get a block by block number.
    pub fn get_block_by_number(&self, block_no: u64) -> Option<(u64, Hash32, &[u8])> {
        let hash = self.block_no_index.get(&block_no)?;
        let block = self.blocks.get(hash)?;
        Some((block.slot, *hash, &block.cbor))
    }

    /// Remove a specific block.
    pub fn remove_block(&mut self, hash: &Hash32) {
        if let Some(block) = self.blocks.remove(hash) {
            // Remove from slot index
            if let Some(hashes) = self.slot_index.get_mut(&block.slot) {
                hashes.retain(|h| h != hash);
                if hashes.is_empty() {
                    self.slot_index.remove(&block.slot);
                }
            }
            // Remove from block_no index
            self.block_no_index.remove(&block.block_no);
            // Remove from successors
            if let Some(succs) = self.successors.get_mut(&block.prev_hash) {
                succs.retain(|h| h != hash);
                if succs.is_empty() {
                    self.successors.remove(&block.prev_hash);
                }
            }
        }
    }

    /// Remove all blocks at or below a given slot.
    /// Returns the hashes of removed blocks.
    pub fn remove_blocks_up_to_slot(&mut self, slot: u64) -> Vec<Hash32> {
        let slots_to_remove: Vec<u64> = self.slot_index.range(..=slot).map(|(&s, _)| s).collect();

        let mut removed = Vec::new();
        for s in slots_to_remove {
            if let Some(hashes) = self.slot_index.remove(&s) {
                for hash in hashes {
                    if let Some(block) = self.blocks.remove(&hash) {
                        self.block_no_index.remove(&block.block_no);
                        if let Some(succs) = self.successors.get_mut(&block.prev_hash) {
                            succs.retain(|h| *h != hash);
                            if succs.is_empty() {
                                self.successors.remove(&block.prev_hash);
                            }
                        }
                    }
                    removed.push(hash);
                }
            }
        }
        removed
    }

    /// Rollback: remove all blocks after a given point (slot, hash).
    /// Returns the removed block hashes (most recent first).
    pub fn rollback_to_point(
        &mut self,
        target_slot: u64,
        target_hash: Option<&Hash32>,
    ) -> Vec<Hash32> {
        let mut removed = Vec::new();

        // Collect all blocks with slot > target_slot
        let slots_to_remove: Vec<u64> = self
            .slot_index
            .range((target_slot + 1)..)
            .map(|(&s, _)| s)
            .collect();

        // Also check blocks at target_slot that don't match the target hash
        let mut at_target: Vec<Hash32> = Vec::new();
        if let Some(target_h) = target_hash {
            if let Some(hashes) = self.slot_index.get(&target_slot) {
                for h in hashes {
                    if h != target_h {
                        at_target.push(*h);
                    }
                }
            }
        }

        // Remove blocks after target
        for s in slots_to_remove.into_iter().rev() {
            if let Some(hashes) = self.slot_index.remove(&s) {
                for hash in hashes {
                    if let Some(block) = self.blocks.remove(&hash) {
                        self.block_no_index.remove(&block.block_no);
                        if let Some(succs) = self.successors.get_mut(&block.prev_hash) {
                            succs.retain(|h| *h != hash);
                        }
                    }
                    removed.push(hash);
                }
            }
        }

        // Remove non-matching blocks at target slot
        for hash in at_target {
            self.remove_block(&hash);
            removed.push(hash);
        }

        // Recompute tip
        self.recompute_tip();
        removed
    }

    /// Clear all blocks.
    pub fn clear(&mut self) {
        self.blocks.clear();
        self.slot_index.clear();
        self.block_no_index.clear();
        self.successors.clear();
        self.tip = None;
    }

    /// Get the current tip.
    pub fn get_tip(&self) -> Option<(u64, Hash32, u64)> {
        self.tip
    }

    /// Number of blocks stored.
    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    /// Whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// Get blocks in slot range [from, to] inclusive.
    pub fn get_blocks_in_slot_range(&self, from_slot: u64, to_slot: u64) -> Vec<(Hash32, &[u8])> {
        let mut result = Vec::new();
        for (&slot, hashes) in self.slot_index.range(from_slot..=to_slot) {
            let _ = slot;
            for hash in hashes {
                if let Some(block) = self.blocks.get(hash) {
                    result.push((*hash, block.cbor.as_slice()));
                }
            }
        }
        result
    }

    /// Recompute tip from the current blocks.
    fn recompute_tip(&mut self) {
        self.tip = self.slot_index.iter().next_back().and_then(|(_, hashes)| {
            hashes
                .first()
                .and_then(|h| self.blocks.get(h).map(|b| (b.slot, *h, b.block_no)))
        });
    }
}

impl Default for VolatileDB {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(byte: u8) -> Hash32 {
        Hash32::from_bytes([byte; 32])
    }

    #[test]
    fn test_add_get_block() {
        let mut db = VolatileDB::new();
        db.add_block(h(1), 100, 10, h(0), b"block1".to_vec());

        assert!(db.has_block(&h(1)));
        assert!(!db.has_block(&h(2)));
        assert_eq!(db.get_block_cbor(&h(1)).unwrap(), b"block1");
        assert_eq!(db.len(), 1);
        assert_eq!(db.get_tip(), Some((100, h(1), 10)));
    }

    #[test]
    fn test_rollback_removes_suffix() {
        let mut db = VolatileDB::new();
        db.add_block(h(1), 100, 10, h(0), b"b1".to_vec());
        db.add_block(h(2), 200, 20, h(1), b"b2".to_vec());
        db.add_block(h(3), 300, 30, h(2), b"b3".to_vec());

        let removed = db.rollback_to_point(100, Some(&h(1)));
        assert_eq!(removed.len(), 2);
        assert!(db.has_block(&h(1)));
        assert!(!db.has_block(&h(2)));
        assert!(!db.has_block(&h(3)));
        assert_eq!(db.get_tip(), Some((100, h(1), 10)));
    }

    #[test]
    fn test_fork_two_blocks_same_slot() {
        let mut db = VolatileDB::new();
        db.add_block(h(1), 100, 10, h(0), b"b1".to_vec());
        db.add_block(h(2), 100, 10, h(0), b"b2_fork".to_vec());

        assert!(db.has_block(&h(1)));
        assert!(db.has_block(&h(2)));
        assert_eq!(db.len(), 2);
    }

    #[test]
    fn test_remove_blocks_up_to_slot() {
        let mut db = VolatileDB::new();
        db.add_block(h(1), 100, 10, h(0), b"b1".to_vec());
        db.add_block(h(2), 200, 20, h(1), b"b2".to_vec());
        db.add_block(h(3), 300, 30, h(2), b"b3".to_vec());

        let removed = db.remove_blocks_up_to_slot(200);
        assert_eq!(removed.len(), 2);
        assert!(!db.has_block(&h(1)));
        assert!(!db.has_block(&h(2)));
        assert!(db.has_block(&h(3)));
    }

    #[test]
    fn test_next_block_after_slot() {
        let mut db = VolatileDB::new();
        db.add_block(h(1), 100, 10, h(0), b"b1".to_vec());
        db.add_block(h(2), 200, 20, h(1), b"b2".to_vec());
        db.add_block(h(3), 300, 30, h(2), b"b3".to_vec());

        let (slot, hash, _) = db.get_next_block_after_slot(0).unwrap();
        assert_eq!(slot, 100);
        assert_eq!(hash, h(1));

        let (slot, _, _) = db.get_next_block_after_slot(100).unwrap();
        assert_eq!(slot, 200);

        assert!(db.get_next_block_after_slot(300).is_none());
    }

    #[test]
    fn test_get_block_by_number() {
        let mut db = VolatileDB::new();
        db.add_block(h(1), 100, 10, h(0), b"b1".to_vec());
        db.add_block(h(2), 200, 20, h(1), b"b2".to_vec());

        let (slot, hash, cbor) = db.get_block_by_number(10).unwrap();
        assert_eq!(slot, 100);
        assert_eq!(hash, h(1));
        assert_eq!(cbor, b"b1");

        assert!(db.get_block_by_number(99).is_none());
    }

    #[test]
    fn test_clear() {
        let mut db = VolatileDB::new();
        db.add_block(h(1), 100, 10, h(0), b"b1".to_vec());
        db.add_block(h(2), 200, 20, h(1), b"b2".to_vec());

        db.clear();
        assert!(db.is_empty());
        assert_eq!(db.get_tip(), None);
    }

    #[test]
    fn test_get_blocks_in_slot_range() {
        let mut db = VolatileDB::new();
        db.add_block(h(1), 100, 10, h(0), b"b1".to_vec());
        db.add_block(h(2), 200, 20, h(1), b"b2".to_vec());
        db.add_block(h(3), 300, 30, h(2), b"b3".to_vec());

        let blocks = db.get_blocks_in_slot_range(100, 200);
        assert_eq!(blocks.len(), 2);
    }
}
