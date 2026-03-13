//! Block index abstraction for ImmutableDB.
//!
//! Provides two implementations:
//! - `InMemoryBlockIndex`: HashMap in RAM (fast, high memory)
//! - `MmapBlockIndex`: On-disk open-addressing hash table via memmap2 (low memory)
//!
//! Uses an enum dispatch (not trait objects) to avoid vtable overhead on the
//! hot lookup path.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use memmap2::{MmapMut, MmapOptions};
use torsten_primitives::hash::Hash32;
use tracing::{debug, warn};

use crate::config::ImmutableConfig;

/// Location of a block within a chunk file.
#[derive(Debug, Clone, Copy)]
pub struct BlockLocation {
    pub chunk_num: u64,
    pub block_offset: u64,
    pub block_end: u64,
}

// ---------------------------------------------------------------------------
// BlockIndex enum (dispatch without vtable)
// ---------------------------------------------------------------------------

pub enum BlockIndex {
    InMemory(InMemoryBlockIndex),
    Mmap(MmapBlockIndex),
}

impl BlockIndex {
    /// Create a new block index from configuration.
    pub fn new(config: &ImmutableConfig, dir: &Path) -> Result<Self, std::io::Error> {
        match config.index_type {
            crate::config::BlockIndexType::InMemory => {
                Ok(BlockIndex::InMemory(InMemoryBlockIndex::new()))
            }
            crate::config::BlockIndexType::Mmap => {
                let idx = MmapBlockIndex::new(dir, config.mmap_load_factor)?;
                Ok(BlockIndex::Mmap(idx))
            }
        }
    }

    pub fn lookup(&self, hash: &Hash32) -> Option<BlockLocation> {
        match self {
            BlockIndex::InMemory(idx) => idx.lookup(hash),
            BlockIndex::Mmap(idx) => idx.lookup(hash),
        }
    }

    pub fn insert(&mut self, hash: Hash32, loc: BlockLocation) {
        match self {
            BlockIndex::InMemory(idx) => idx.insert(hash, loc),
            BlockIndex::Mmap(idx) => idx.insert(hash, loc),
        }
    }

    pub fn contains(&self, hash: &Hash32) -> bool {
        match self {
            BlockIndex::InMemory(idx) => idx.contains(hash),
            BlockIndex::Mmap(idx) => idx.contains(hash),
        }
    }

    pub fn len(&self) -> usize {
        match self {
            BlockIndex::InMemory(idx) => idx.len(),
            BlockIndex::Mmap(idx) => idx.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Persist the mmap index to disk. No-op for in-memory.
    pub fn persist(&self) -> Result<(), std::io::Error> {
        match self {
            BlockIndex::InMemory(_) => Ok(()),
            BlockIndex::Mmap(idx) => idx.persist(),
        }
    }
}

// ---------------------------------------------------------------------------
// InMemoryBlockIndex
// ---------------------------------------------------------------------------

pub struct InMemoryBlockIndex {
    map: HashMap<Hash32, BlockLocation>,
}

impl InMemoryBlockIndex {
    pub fn new() -> Self {
        InMemoryBlockIndex {
            map: HashMap::new(),
        }
    }

    pub fn with_capacity(cap: usize) -> Self {
        InMemoryBlockIndex {
            map: HashMap::with_capacity(cap),
        }
    }

    pub fn lookup(&self, hash: &Hash32) -> Option<BlockLocation> {
        self.map.get(hash).copied()
    }

    pub fn insert(&mut self, hash: Hash32, loc: BlockLocation) {
        self.map.insert(hash, loc);
    }

    pub fn contains(&self, hash: &Hash32) -> bool {
        self.map.contains_key(hash)
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

impl Default for InMemoryBlockIndex {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// MmapBlockIndex
// ---------------------------------------------------------------------------

/// On-disk format:
/// - Header (32 bytes): magic [4], version [4], capacity [8], count [8], reserved [8]
/// - Entries: capacity × ENTRY_SIZE bytes each
///
/// Each entry (56 bytes): hash [32], chunk_num [8], block_offset [8], block_end [8]
/// Empty slots have an all-zero hash (Hash32::ZERO is never a valid block hash).
const MMAP_MAGIC: [u8; 4] = *b"TRIX";
const MMAP_VERSION: u32 = 1;
const HEADER_SIZE: usize = 32;
const ENTRY_SIZE: usize = 56;
/// Minimum capacity to avoid degenerate small tables.
const MIN_CAPACITY: u64 = 1024;

pub struct MmapBlockIndex {
    /// Memory-mapped file (header + entries).
    mmap: MmapMut,
    /// Path to the index file.
    path: PathBuf,
    /// Table capacity (number of slots).
    capacity: u64,
    /// Number of occupied slots.
    count: u64,
    /// Load factor threshold for resize.
    load_factor: f64,
    /// Overflow buffer for entries that don't fit (after table is full).
    /// This is a fallback — should rarely be used if load factor is sane.
    overflow: HashMap<Hash32, BlockLocation>,
}

impl MmapBlockIndex {
    /// Create a new (empty) mmap block index.
    pub fn new(dir: &Path, load_factor: f64) -> Result<Self, std::io::Error> {
        let path = dir.join("hash_index.dat");
        let load_factor = if load_factor <= 0.0 || load_factor >= 1.0 {
            0.7
        } else {
            load_factor
        };

        if path.exists() {
            // Try to open existing file
            match Self::open_existing(&path, load_factor) {
                Ok(idx) => return Ok(idx),
                Err(e) => {
                    warn!("Failed to open existing hash_index.dat, will rebuild: {e}");
                }
            }
        }

        // Create new empty table
        Self::create_new(&path, MIN_CAPACITY, load_factor)
    }

    /// Open an existing mmap file, validating the header.
    fn open_existing(path: &Path, load_factor: f64) -> Result<Self, std::io::Error> {
        let file = fs::OpenOptions::new().read(true).write(true).open(path)?;
        let file_len = file.metadata()?.len();
        if file_len < HEADER_SIZE as u64 {
            return Err(std::io::Error::other("hash_index.dat too small"));
        }

        let mmap = unsafe { MmapOptions::new().map_mut(&file)? };

        // Validate header
        if &mmap[0..4] != MMAP_MAGIC.as_slice() {
            return Err(std::io::Error::other("bad magic in hash_index.dat"));
        }
        let version = u32::from_le_bytes(mmap[4..8].try_into().unwrap());
        if version != MMAP_VERSION {
            return Err(std::io::Error::other(format!(
                "unsupported hash_index version {version}"
            )));
        }
        let capacity = u64::from_le_bytes(mmap[8..16].try_into().unwrap());
        let count = u64::from_le_bytes(mmap[16..24].try_into().unwrap());

        let expected_size = HEADER_SIZE as u64 + capacity * ENTRY_SIZE as u64;
        if file_len < expected_size {
            return Err(std::io::Error::other("hash_index.dat truncated"));
        }

        debug!(capacity, count, "Opened existing mmap block index");

        Ok(MmapBlockIndex {
            mmap,
            path: path.to_path_buf(),
            capacity,
            count,
            load_factor,
            overflow: HashMap::new(),
        })
    }

    /// Create a new mmap file with the given capacity.
    fn create_new(path: &Path, capacity: u64, load_factor: f64) -> Result<Self, std::io::Error> {
        let file_size = HEADER_SIZE as u64 + capacity * ENTRY_SIZE as u64;
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        file.set_len(file_size)?;

        let mut mmap = unsafe { MmapOptions::new().map_mut(&file)? };

        // Write header
        mmap[0..4].copy_from_slice(&MMAP_MAGIC);
        mmap[4..8].copy_from_slice(&MMAP_VERSION.to_le_bytes());
        mmap[8..16].copy_from_slice(&capacity.to_le_bytes());
        mmap[16..24].copy_from_slice(&0u64.to_le_bytes()); // count = 0
                                                           // reserved bytes 24..32 are already zero

        debug!(capacity, "Created new mmap block index");

        Ok(MmapBlockIndex {
            mmap,
            path: path.to_path_buf(),
            capacity,
            count: 0,
            load_factor,
            overflow: HashMap::new(),
        })
    }

    /// Rebuild the mmap file with a new capacity, preserving all entries.
    fn rebuild(&mut self, new_capacity: u64) -> Result<(), std::io::Error> {
        // Collect all existing entries
        let mut entries = Vec::with_capacity(self.count as usize + self.overflow.len());

        // Scan the mmap table
        for slot in 0..self.capacity {
            let offset = HEADER_SIZE + (slot as usize) * ENTRY_SIZE;
            let hash_bytes: [u8; 32] = self.mmap[offset..offset + 32].try_into().unwrap();
            if hash_bytes == [0u8; 32] {
                continue;
            }
            let chunk_num =
                u64::from_le_bytes(self.mmap[offset + 32..offset + 40].try_into().unwrap());
            let block_offset =
                u64::from_le_bytes(self.mmap[offset + 40..offset + 48].try_into().unwrap());
            let block_end =
                u64::from_le_bytes(self.mmap[offset + 48..offset + 56].try_into().unwrap());
            entries.push((
                Hash32::from_bytes(hash_bytes),
                BlockLocation {
                    chunk_num,
                    block_offset,
                    block_end,
                },
            ));
        }

        // Add overflow entries
        for (hash, loc) in self.overflow.drain() {
            entries.push((hash, loc));
        }

        // Create new file
        let mut new = Self::create_new(&self.path, new_capacity, self.load_factor)?;

        // Re-insert all entries
        for (hash, loc) in entries {
            new.insert_into_mmap(&hash, &loc);
        }

        // Persist the count to the header
        new.write_count();

        *self = new;
        Ok(())
    }

    /// Insert directly into the mmap table (no overflow, no resize check).
    fn insert_into_mmap(&mut self, hash: &Hash32, loc: &BlockLocation) {
        let hash_bytes = hash.as_bytes();
        let mut slot = self.hash_slot(hash_bytes);

        // Linear probing
        for _ in 0..self.capacity {
            let offset = HEADER_SIZE + (slot as usize) * ENTRY_SIZE;
            let existing: [u8; 32] = self.mmap[offset..offset + 32].try_into().unwrap();
            if existing == [0u8; 32] || existing == *hash_bytes {
                // Empty slot or update existing
                self.mmap[offset..offset + 32].copy_from_slice(hash_bytes);
                self.mmap[offset + 32..offset + 40].copy_from_slice(&loc.chunk_num.to_le_bytes());
                self.mmap[offset + 40..offset + 48]
                    .copy_from_slice(&loc.block_offset.to_le_bytes());
                self.mmap[offset + 48..offset + 56].copy_from_slice(&loc.block_end.to_le_bytes());
                if existing == [0u8; 32] {
                    self.count += 1;
                }
                return;
            }
            slot = (slot + 1) % self.capacity;
        }

        // Table completely full — shouldn't happen with proper load factor
        warn!("Mmap block index full, using overflow");
        self.overflow.insert(*hash, *loc);
    }

    fn hash_slot(&self, hash_bytes: &[u8; 32]) -> u64 {
        // Use first 8 bytes of the block hash as the slot index.
        // Blake2b-256 hashes are uniformly distributed, so this is fine.
        let raw = u64::from_le_bytes(hash_bytes[0..8].try_into().unwrap());
        raw % self.capacity
    }

    fn write_count(&mut self) {
        self.mmap[16..24].copy_from_slice(&self.count.to_le_bytes());
    }

    pub fn lookup(&self, hash: &Hash32) -> Option<BlockLocation> {
        // Check overflow first
        if let Some(loc) = self.overflow.get(hash) {
            return Some(*loc);
        }

        let hash_bytes = hash.as_bytes();
        let mut slot = self.hash_slot(hash_bytes);

        for _ in 0..self.capacity {
            let offset = HEADER_SIZE + (slot as usize) * ENTRY_SIZE;
            let existing: [u8; 32] = self.mmap[offset..offset + 32].try_into().unwrap();
            if existing == [0u8; 32] {
                return None; // Empty slot — not found
            }
            if existing == *hash_bytes {
                let chunk_num =
                    u64::from_le_bytes(self.mmap[offset + 32..offset + 40].try_into().unwrap());
                let block_offset =
                    u64::from_le_bytes(self.mmap[offset + 40..offset + 48].try_into().unwrap());
                let block_end =
                    u64::from_le_bytes(self.mmap[offset + 48..offset + 56].try_into().unwrap());
                return Some(BlockLocation {
                    chunk_num,
                    block_offset,
                    block_end,
                });
            }
            slot = (slot + 1) % self.capacity;
        }

        None
    }

    pub fn insert(&mut self, hash: Hash32, loc: BlockLocation) {
        // Check if we need to resize
        let threshold = (self.capacity as f64 * self.load_factor) as u64;
        if self.count >= threshold {
            let new_capacity = (self.capacity * 2).max(MIN_CAPACITY);
            if let Err(e) = self.rebuild(new_capacity) {
                warn!("Failed to resize mmap block index: {e}, using overflow");
                self.overflow.insert(hash, loc);
                return;
            }
        }

        self.insert_into_mmap(&hash, &loc);
        self.write_count();
    }

    pub fn contains(&self, hash: &Hash32) -> bool {
        self.lookup(hash).is_some()
    }

    pub fn len(&self) -> usize {
        self.count as usize + self.overflow.len()
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0 && self.overflow.is_empty()
    }

    /// Flush the mmap to disk.
    pub fn persist(&self) -> Result<(), std::io::Error> {
        self.mmap.flush()
    }

    /// Build from a set of entries (used when creating from secondary indexes).
    pub fn build_from_entries(
        dir: &Path,
        entries: &[(Hash32, BlockLocation)],
        load_factor: f64,
    ) -> Result<Self, std::io::Error> {
        let load_factor = if load_factor <= 0.0 || load_factor >= 1.0 {
            0.7
        } else {
            load_factor
        };
        let capacity = ((entries.len() as f64 / load_factor) as u64 + 1).max(MIN_CAPACITY);
        let path = dir.join("hash_index.dat");

        let mut idx = Self::create_new(&path, capacity, load_factor)?;
        for (hash, loc) in entries {
            idx.insert_into_mmap(hash, loc);
        }
        idx.write_count();
        idx.mmap.flush()?;

        debug!(
            entries = entries.len(),
            capacity, "Built mmap block index from entries"
        );

        Ok(idx)
    }

    /// Check if the stored entry count matches the expected count.
    pub fn count_matches(&self, expected: u64) -> bool {
        self.count == expected
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_hash(n: u8) -> Hash32 {
        Hash32::from_bytes([n; 32])
    }

    fn make_loc(chunk: u64, offset: u64, end: u64) -> BlockLocation {
        BlockLocation {
            chunk_num: chunk,
            block_offset: offset,
            block_end: end,
        }
    }

    // -- InMemoryBlockIndex tests --

    #[test]
    fn test_in_memory_basic() {
        let mut idx = InMemoryBlockIndex::new();
        let hash = make_hash(1);
        let loc = make_loc(0, 0, 100);

        idx.insert(hash, loc);
        assert!(idx.contains(&hash));
        assert_eq!(idx.len(), 1);

        let found = idx.lookup(&hash).unwrap();
        assert_eq!(found.chunk_num, 0);
        assert_eq!(found.block_offset, 0);
        assert_eq!(found.block_end, 100);

        assert!(!idx.contains(&make_hash(99)));
    }

    #[test]
    fn test_in_memory_with_capacity() {
        let idx = InMemoryBlockIndex::with_capacity(100);
        assert_eq!(idx.len(), 0);
    }

    // -- MmapBlockIndex tests --

    #[test]
    fn test_mmap_basic() {
        let dir = tempfile::tempdir().unwrap();
        let mut idx = MmapBlockIndex::new(dir.path(), 0.7).unwrap();

        let hash = make_hash(1);
        let loc = make_loc(0, 0, 100);

        idx.insert(hash, loc);
        assert!(idx.contains(&hash));
        assert_eq!(idx.len(), 1);

        let found = idx.lookup(&hash).unwrap();
        assert_eq!(found.chunk_num, 0);
        assert_eq!(found.block_offset, 0);
        assert_eq!(found.block_end, 100);

        assert!(!idx.contains(&make_hash(99)));
    }

    #[test]
    fn test_mmap_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let hash = make_hash(42);
        let loc = make_loc(5, 1000, 2000);

        // Write
        {
            let mut idx = MmapBlockIndex::new(dir.path(), 0.7).unwrap();
            idx.insert(hash, loc);
            idx.persist().unwrap();
        }

        // Reopen and verify
        {
            let idx = MmapBlockIndex::new(dir.path(), 0.7).unwrap();
            assert_eq!(idx.len(), 1);
            let found = idx.lookup(&hash).unwrap();
            assert_eq!(found.chunk_num, 5);
            assert_eq!(found.block_offset, 1000);
            assert_eq!(found.block_end, 2000);
        }
    }

    #[test]
    fn test_mmap_rebuild_on_stale() {
        let dir = tempfile::tempdir().unwrap();

        // Create index with some entries
        {
            let mut idx = MmapBlockIndex::new(dir.path(), 0.7).unwrap();
            for i in 1..=10u8 {
                idx.insert(make_hash(i), make_loc(i as u64, 0, 100));
            }
            idx.persist().unwrap();
        }

        // Corrupt the count to simulate stale state
        {
            let path = dir.path().join("hash_index.dat");
            let mut data = fs::read(&path).unwrap();
            // Set count to 999 (wrong)
            data[16..24].copy_from_slice(&999u64.to_le_bytes());
            fs::write(&path, data).unwrap();
        }

        // Reopen — should still work (count mismatch is just metadata)
        let idx = MmapBlockIndex::new(dir.path(), 0.7).unwrap();
        // The stored count is 999 but actual entries are 10
        // Lookups should still work since entries are intact
        assert!(idx.contains(&make_hash(1)));
        assert!(idx.contains(&make_hash(10)));
    }

    #[test]
    fn test_mmap_collisions() {
        let dir = tempfile::tempdir().unwrap();
        let mut idx = MmapBlockIndex::new(dir.path(), 0.7).unwrap();

        // Insert many entries to force collisions.
        // Start from 1 — hash_bytes all-zero is the sentinel for empty slots.
        for i in 1..=100u8 {
            let mut hash_bytes = [0u8; 32];
            hash_bytes[0] = i;
            hash_bytes[8] = i;
            let hash = Hash32::from_bytes(hash_bytes);
            idx.insert(
                hash,
                make_loc(i as u64, i as u64 * 100, (i as u64 + 1) * 100),
            );
        }

        assert_eq!(idx.len(), 100);

        // Verify all entries are retrievable
        for i in 1..=100u8 {
            let mut hash_bytes = [0u8; 32];
            hash_bytes[0] = i;
            hash_bytes[8] = i;
            let hash = Hash32::from_bytes(hash_bytes);
            let loc = idx.lookup(&hash).unwrap();
            assert_eq!(loc.chunk_num, i as u64);
        }
    }

    #[test]
    fn test_mmap_resize() {
        let dir = tempfile::tempdir().unwrap();
        // Start with very small capacity (MIN_CAPACITY=1024, load_factor=0.7 → threshold ~717)
        let mut idx = MmapBlockIndex::new(dir.path(), 0.7).unwrap();
        let initial_cap = idx.capacity;

        // Insert enough entries to trigger resize.
        // Start from 1 — i=0 produces all-zero hash which is the empty sentinel.
        for i in 1..=800u32 {
            let mut hash_bytes = [0u8; 32];
            hash_bytes[0..4].copy_from_slice(&i.to_le_bytes());
            let hash = Hash32::from_bytes(hash_bytes);
            idx.insert(hash, make_loc(0, i as u64 * 100, (i as u64 + 1) * 100));
        }

        // Should have resized
        assert!(idx.capacity > initial_cap);
        assert_eq!(idx.len(), 800);

        // Verify all entries still accessible after resize
        for i in 1..=800u32 {
            let mut hash_bytes = [0u8; 32];
            hash_bytes[0..4].copy_from_slice(&i.to_le_bytes());
            let hash = Hash32::from_bytes(hash_bytes);
            assert!(idx.contains(&hash), "entry {i} not found after resize");
        }
    }

    #[test]
    fn test_mmap_build_from_entries() {
        let dir = tempfile::tempdir().unwrap();
        let entries: Vec<(Hash32, BlockLocation)> = (1..=50u8)
            .map(|i| (make_hash(i), make_loc(i as u64, 0, 100)))
            .collect();

        let idx = MmapBlockIndex::build_from_entries(dir.path(), &entries, 0.7).unwrap();
        assert_eq!(idx.len(), 50);

        for i in 1..=50u8 {
            assert!(idx.contains(&make_hash(i)));
        }
    }

    // -- BlockIndex enum tests --

    #[test]
    fn test_block_index_in_memory() {
        let config = ImmutableConfig {
            index_type: crate::config::BlockIndexType::InMemory,
            ..Default::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let mut idx = BlockIndex::new(&config, dir.path()).unwrap();

        idx.insert(make_hash(1), make_loc(0, 0, 100));
        assert!(idx.contains(&make_hash(1)));
        assert_eq!(idx.len(), 1);
        assert!(idx.lookup(&make_hash(1)).is_some());
    }

    #[test]
    fn test_block_index_mmap() {
        let config = ImmutableConfig {
            index_type: crate::config::BlockIndexType::Mmap,
            mmap_load_factor: 0.7,
            mmap_initial_capacity: 0,
        };
        let dir = tempfile::tempdir().unwrap();
        let mut idx = BlockIndex::new(&config, dir.path()).unwrap();

        idx.insert(make_hash(1), make_loc(0, 0, 100));
        assert!(idx.contains(&make_hash(1)));
        assert_eq!(idx.len(), 1);
        assert!(idx.lookup(&make_hash(1)).is_some());
    }

    // -- Additional coverage tests --

    #[test]
    fn test_in_memory_update_existing() {
        let mut idx = InMemoryBlockIndex::new();
        let hash = make_hash(1);
        idx.insert(hash, make_loc(0, 0, 100));
        idx.insert(hash, make_loc(1, 200, 300));
        // Should update, not add
        assert_eq!(idx.len(), 1);
        let loc = idx.lookup(&hash).unwrap();
        assert_eq!(loc.chunk_num, 1);
        assert_eq!(loc.block_offset, 200);
    }

    #[test]
    fn test_in_memory_many_entries() {
        let mut idx = InMemoryBlockIndex::new();
        for i in 1..=255u8 {
            idx.insert(make_hash(i), make_loc(i as u64, 0, 100));
        }
        assert_eq!(idx.len(), 255);
        for i in 1..=255u8 {
            assert!(idx.contains(&make_hash(i)));
        }
    }

    #[test]
    fn test_mmap_update_existing() {
        let dir = tempfile::tempdir().unwrap();
        let mut idx = MmapBlockIndex::new(dir.path(), 0.7).unwrap();
        let hash = make_hash(1);

        idx.insert(hash, make_loc(0, 0, 100));
        idx.insert(hash, make_loc(5, 500, 600));

        // Should have updated in place, not added a new entry
        assert_eq!(idx.len(), 1);
        let loc = idx.lookup(&hash).unwrap();
        assert_eq!(loc.chunk_num, 5);
        assert_eq!(loc.block_offset, 500);
        assert_eq!(loc.block_end, 600);
    }

    #[test]
    fn test_mmap_empty_lookup() {
        let dir = tempfile::tempdir().unwrap();
        let idx = MmapBlockIndex::new(dir.path(), 0.7).unwrap();
        assert_eq!(idx.len(), 0);
        assert!(!idx.contains(&make_hash(1)));
        assert!(idx.lookup(&make_hash(1)).is_none());
    }

    #[test]
    fn test_mmap_count_matches() {
        let dir = tempfile::tempdir().unwrap();
        let mut idx = MmapBlockIndex::new(dir.path(), 0.7).unwrap();
        assert!(idx.count_matches(0));
        assert!(!idx.count_matches(1));

        idx.insert(make_hash(1), make_loc(0, 0, 100));
        assert!(idx.count_matches(1));
        assert!(!idx.count_matches(0));
    }

    #[test]
    fn test_mmap_bad_magic_triggers_rebuild() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hash_index.dat");

        // Write a file with bad magic
        fs::write(&path, b"BADMxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx").unwrap();

        // Should create fresh index (ignoring the bad file)
        let idx = MmapBlockIndex::new(dir.path(), 0.7).unwrap();
        assert_eq!(idx.len(), 0);
    }

    #[test]
    fn test_mmap_truncated_file_triggers_rebuild() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hash_index.dat");

        // Write valid header but truncated (capacity claims more than file has)
        let mut data = vec![0u8; HEADER_SIZE];
        data[0..4].copy_from_slice(&MMAP_MAGIC);
        data[4..8].copy_from_slice(&MMAP_VERSION.to_le_bytes());
        data[8..16].copy_from_slice(&1000u64.to_le_bytes()); // claims 1000 entries worth of space
        data[16..24].copy_from_slice(&0u64.to_le_bytes());
        fs::write(&path, &data).unwrap();

        // Should rebuild since file is truncated
        let idx = MmapBlockIndex::new(dir.path(), 0.7).unwrap();
        assert_eq!(idx.len(), 0);
    }

    #[test]
    fn test_mmap_invalid_load_factor_clamped() {
        let dir = tempfile::tempdir().unwrap();
        // Negative load factor should be clamped to 0.7
        let mut idx = MmapBlockIndex::new(dir.path(), -1.0).unwrap();
        idx.insert(make_hash(1), make_loc(0, 0, 100));
        assert!(idx.contains(&make_hash(1)));

        // Load factor >= 1.0 should also be clamped
        let dir2 = tempfile::tempdir().unwrap();
        let mut idx2 = MmapBlockIndex::new(dir2.path(), 1.5).unwrap();
        idx2.insert(make_hash(2), make_loc(0, 0, 100));
        assert!(idx2.contains(&make_hash(2)));
    }

    #[test]
    fn test_mmap_persist_and_reopen_many() {
        let dir = tempfile::tempdir().unwrap();

        // Write many entries, persist
        {
            let mut idx = MmapBlockIndex::new(dir.path(), 0.7).unwrap();
            for i in 1..=500u32 {
                let mut h = [0u8; 32];
                h[0..4].copy_from_slice(&i.to_le_bytes());
                idx.insert(
                    Hash32::from_bytes(h),
                    make_loc(i as u64, i as u64 * 10, (i as u64 + 1) * 10),
                );
            }
            idx.persist().unwrap();
        }

        // Reopen and verify all entries
        {
            let idx = MmapBlockIndex::new(dir.path(), 0.7).unwrap();
            assert_eq!(idx.len(), 500);
            for i in 1..=500u32 {
                let mut h = [0u8; 32];
                h[0..4].copy_from_slice(&i.to_le_bytes());
                let loc = idx.lookup(&Hash32::from_bytes(h)).unwrap();
                assert_eq!(loc.chunk_num, i as u64);
            }
        }
    }

    #[test]
    fn test_mmap_build_from_entries_empty() {
        let dir = tempfile::tempdir().unwrap();
        let idx = MmapBlockIndex::build_from_entries(dir.path(), &[], 0.7).unwrap();
        assert_eq!(idx.len(), 0);
        assert!(!idx.contains(&make_hash(1)));
    }

    #[test]
    fn test_block_index_persist_in_memory_is_noop() {
        let config = ImmutableConfig {
            index_type: crate::config::BlockIndexType::InMemory,
            ..Default::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let idx = BlockIndex::new(&config, dir.path()).unwrap();
        // persist() on InMemory should succeed (no-op)
        assert!(idx.persist().is_ok());
    }

    #[test]
    fn test_block_index_persist_mmap() {
        let config = ImmutableConfig {
            index_type: crate::config::BlockIndexType::Mmap,
            mmap_load_factor: 0.7,
            mmap_initial_capacity: 0,
        };
        let dir = tempfile::tempdir().unwrap();
        let mut idx = BlockIndex::new(&config, dir.path()).unwrap();
        idx.insert(make_hash(1), make_loc(0, 0, 100));
        assert!(idx.persist().is_ok());
        // Verify file exists
        assert!(dir.path().join("hash_index.dat").exists());
    }
}
