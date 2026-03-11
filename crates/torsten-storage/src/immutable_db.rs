//! Read-only block storage over Cardano immutable chunk files.
//!
//! Provides O(1) block lookups by hash and sequential slot-based queries
//! over the chunk files produced by Mithril snapshot import or the node
//! itself. Chunk files use the same on-disk format as cardano-node's
//! ImmutableDB (`.chunk` + `.secondary` index files).
//!
//! On startup, builds an in-memory hash index from secondary index files.
//! Slot-based queries use binary search over per-chunk metadata followed
//! by a secondary index scan within the target chunk.

use memmap2::Mmap;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;
use torsten_primitives::hash::Hash32;
use tracing::{info, warn};

/// Secondary index entry size in bytes.
const SECONDARY_ENTRY_SIZE: usize = 56;

#[derive(Error, Debug)]
pub enum ImmutableDBError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Location of a block within a chunk file.
#[derive(Debug, Clone)]
struct BlockLocation {
    chunk_num: u64,
    block_offset: u64,
    block_end: u64,
}

/// Per-chunk metadata for slot-based lookups.
#[derive(Debug, Clone)]
struct ChunkMeta {
    chunk_num: u64,
    first_slot: u64,
    last_slot: u64,
}

/// Read-only storage backed by Cardano immutable chunk files.
///
/// Each chunk file (`.chunk`) stores raw block CBOR sequentially.
/// Secondary index files (`.secondary`) provide 56-byte entries with
/// block boundaries, header hashes, and slot numbers.
pub struct ImmutableDB {
    dir: PathBuf,
    chunks: Vec<ChunkMeta>,
    hash_index: HashMap<Hash32, BlockLocation>,
    total_blocks: u64,
    tip_slot: u64,
    tip_hash: Hash32,
}

impl ImmutableDB {
    /// Open an ImmutableDB from a directory of chunk files.
    ///
    /// Scans all `.chunk` and `.secondary` files and builds an in-memory
    /// hash index for O(1) block lookups. For preview (~4M blocks) this
    /// uses ~300 MB of memory; mainnet will need an on-disk index.
    pub fn open(dir: &Path) -> Result<Self, ImmutableDBError> {
        info!(dir = %dir.display(), "Opening ImmutableDB");

        let mut chunk_nums = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if let Some(num_str) = name_str.strip_suffix(".chunk") {
                if let Ok(num) = num_str.parse::<u64>() {
                    chunk_nums.push(num);
                }
            }
        }
        chunk_nums.sort();

        if chunk_nums.is_empty() {
            info!("ImmutableDB: no chunk files found");
            return Ok(ImmutableDB {
                dir: dir.to_path_buf(),
                chunks: Vec::new(),
                hash_index: HashMap::new(),
                total_blocks: 0,
                tip_slot: 0,
                tip_hash: Hash32::ZERO,
            });
        }

        let mut hash_index = HashMap::new();
        let mut chunks = Vec::with_capacity(chunk_nums.len());
        let mut total_blocks = 0u64;
        let mut tip_slot = 0u64;
        let mut tip_hash = Hash32::ZERO;

        for &chunk_num in &chunk_nums {
            let secondary_path = dir.join(format!("{chunk_num:05}.secondary"));
            let chunk_path = dir.join(format!("{chunk_num:05}.chunk"));

            if !secondary_path.exists() || !chunk_path.exists() {
                continue;
            }

            let chunk_len = chunk_path.metadata()?.len();
            let secondary_data = fs::read(&secondary_path)?;
            let entry_count = secondary_data.len() / SECONDARY_ENTRY_SIZE;
            if entry_count == 0 {
                continue;
            }

            // Parse secondary index entries: (block_offset, header_hash, slot)
            let mut entries: Vec<(u64, [u8; 32], u64)> = Vec::with_capacity(entry_count);
            let mut pos = 0;
            while pos + SECONDARY_ENTRY_SIZE <= secondary_data.len() {
                let data = &secondary_data[pos..];
                let block_offset = u64::from_be_bytes(data[0..8].try_into().unwrap());
                let mut header_hash = [0u8; 32];
                header_hash.copy_from_slice(&data[16..48]);
                let block_or_ebb = u64::from_be_bytes(data[48..56].try_into().unwrap());
                entries.push((block_offset, header_hash, block_or_ebb));
                pos += SECONDARY_ENTRY_SIZE;
            }

            let mut first_slot = u64::MAX;
            let mut last_slot = 0u64;

            for i in 0..entries.len() {
                let (block_offset, header_hash, slot) = entries[i];
                let block_end = if i + 1 < entries.len() {
                    entries[i + 1].0
                } else {
                    chunk_len
                };

                let hash = Hash32::from_bytes(header_hash);
                hash_index.insert(
                    hash,
                    BlockLocation {
                        chunk_num,
                        block_offset,
                        block_end,
                    },
                );

                if slot < first_slot {
                    first_slot = slot;
                }
                if slot > last_slot {
                    last_slot = slot;
                }
                if slot >= tip_slot {
                    tip_slot = slot;
                    tip_hash = hash;
                }
            }

            chunks.push(ChunkMeta {
                chunk_num,
                first_slot,
                last_slot,
            });
            total_blocks += entry_count as u64;
        }

        info!(
            chunks = chunks.len(),
            total_blocks,
            tip_slot,
            hash_index_entries = hash_index.len(),
            "ImmutableDB opened"
        );

        Ok(ImmutableDB {
            dir: dir.to_path_buf(),
            chunks,
            hash_index,
            total_blocks,
            tip_slot,
            tip_hash,
        })
    }

    /// Get block CBOR by header hash.
    pub fn get_block(&self, hash: &Hash32) -> Option<Vec<u8>> {
        let loc = self.hash_index.get(hash)?;
        self.read_block_at(loc)
    }

    /// Check if a block exists by header hash.
    pub fn has_block(&self, hash: &Hash32) -> bool {
        self.hash_index.contains_key(hash)
    }

    /// Total number of blocks across all chunk files.
    pub fn total_blocks(&self) -> u64 {
        self.total_blocks
    }

    /// Tip slot of the immutable chain.
    pub fn tip_slot(&self) -> u64 {
        self.tip_slot
    }

    /// Tip hash of the immutable chain.
    pub fn tip_hash(&self) -> Hash32 {
        self.tip_hash
    }

    /// Directory containing the chunk files.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Get the first block strictly after `after_slot`.
    ///
    /// Uses binary search on chunk metadata to find the starting chunk,
    /// then scans the secondary index within that chunk.
    pub fn get_next_block_after_slot(&self, after_slot: u64) -> Option<(u64, Hash32, Vec<u8>)> {
        // Find the first chunk whose last_slot > after_slot
        let start_idx = self.chunks.partition_point(|c| c.last_slot <= after_slot);

        for chunk_meta in &self.chunks[start_idx..] {
            let secondary_path = self
                .dir
                .join(format!("{:05}.secondary", chunk_meta.chunk_num));
            let secondary_data = match fs::read(&secondary_path) {
                Ok(data) => data,
                Err(_) => continue,
            };

            let chunk_path = self.dir.join(format!("{:05}.chunk", chunk_meta.chunk_num));
            let chunk_len = match chunk_path.metadata() {
                Ok(m) => m.len(),
                Err(_) => continue,
            };

            let mut pos = 0;
            while pos + SECONDARY_ENTRY_SIZE <= secondary_data.len() {
                let data = &secondary_data[pos..];
                let block_offset = u64::from_be_bytes(data[0..8].try_into().unwrap());
                let mut header_hash = [0u8; 32];
                header_hash.copy_from_slice(&data[16..48]);
                let slot = u64::from_be_bytes(data[48..56].try_into().unwrap());

                if slot > after_slot {
                    // Compute block end from next entry or chunk length
                    let next_pos = pos + SECONDARY_ENTRY_SIZE;
                    let block_end = if next_pos + 8 <= secondary_data.len() {
                        u64::from_be_bytes(
                            secondary_data[next_pos..next_pos + 8].try_into().unwrap(),
                        )
                    } else {
                        chunk_len
                    };

                    let loc = BlockLocation {
                        chunk_num: chunk_meta.chunk_num,
                        block_offset,
                        block_end,
                    };
                    if let Some(cbor) = self.read_block_at(&loc) {
                        return Some((slot, Hash32::from_bytes(header_hash), cbor));
                    }
                }

                pos += SECONDARY_ENTRY_SIZE;
            }
        }

        None
    }

    /// Get blocks in slot range `[from_slot, to_slot]` inclusive.
    pub fn get_blocks_in_slot_range(&self, from_slot: u64, to_slot: u64) -> Vec<Vec<u8>> {
        let mut result = Vec::new();

        let start_idx = self.chunks.partition_point(|c| c.last_slot < from_slot);

        for chunk_meta in &self.chunks[start_idx..] {
            if chunk_meta.first_slot > to_slot {
                break;
            }

            let secondary_path = self
                .dir
                .join(format!("{:05}.secondary", chunk_meta.chunk_num));
            let chunk_path = self.dir.join(format!("{:05}.chunk", chunk_meta.chunk_num));

            let secondary_data = match fs::read(&secondary_path) {
                Ok(data) => data,
                Err(_) => continue,
            };
            let chunk_file = match fs::File::open(&chunk_path) {
                Ok(f) => f,
                Err(_) => continue,
            };
            let chunk_mmap = match unsafe { Mmap::map(&chunk_file) } {
                Ok(m) => m,
                Err(_) => continue,
            };

            // Parse all entries to get offsets and slots
            let entry_count = secondary_data.len() / SECONDARY_ENTRY_SIZE;
            let mut entries: Vec<(u64, u64)> = Vec::with_capacity(entry_count);
            let mut pos = 0;
            while pos + SECONDARY_ENTRY_SIZE <= secondary_data.len() {
                let data = &secondary_data[pos..];
                let block_offset = u64::from_be_bytes(data[0..8].try_into().unwrap());
                let slot = u64::from_be_bytes(data[48..56].try_into().unwrap());
                entries.push((block_offset, slot));
                pos += SECONDARY_ENTRY_SIZE;
            }

            for i in 0..entries.len() {
                let (block_offset, slot) = entries[i];
                if slot < from_slot {
                    continue;
                }
                if slot > to_slot {
                    break;
                }

                let block_end = if i + 1 < entries.len() {
                    entries[i + 1].0 as usize
                } else {
                    chunk_mmap.len()
                };
                let start = block_offset as usize;
                if start < chunk_mmap.len() && block_end <= chunk_mmap.len() {
                    result.push(chunk_mmap[start..block_end].to_vec());
                }
            }
        }

        result
    }

    /// Read a block from a chunk file at the given location.
    fn read_block_at(&self, loc: &BlockLocation) -> Option<Vec<u8>> {
        let chunk_path = self.dir.join(format!("{:05}.chunk", loc.chunk_num));
        let file = fs::File::open(&chunk_path).ok()?;
        let mmap = unsafe { Mmap::map(&file).ok()? };

        let start = loc.block_offset as usize;
        let end = loc.block_end as usize;
        if end > mmap.len() || start >= end {
            warn!(
                chunk = loc.chunk_num,
                offset = start,
                end,
                chunk_len = mmap.len(),
                "Invalid block location in chunk file"
            );
            return None;
        }

        Some(mmap[start..end].to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Build a minimal chunk file + secondary index for testing.
    fn create_test_chunk(
        dir: &Path,
        chunk_num: u64,
        blocks: &[(&[u8], [u8; 32], u64)], // (cbor, hash, slot)
    ) {
        let chunk_path = dir.join(format!("{chunk_num:05}.chunk"));
        let secondary_path = dir.join(format!("{chunk_num:05}.secondary"));

        let mut chunk_file = fs::File::create(&chunk_path).unwrap();
        let mut secondary_file = fs::File::create(&secondary_path).unwrap();

        let mut offset = 0u64;
        for (cbor, hash, slot) in blocks {
            // Write block CBOR to chunk file
            chunk_file.write_all(cbor).unwrap();

            // Write 56-byte secondary index entry
            let mut entry = [0u8; 56];
            entry[0..8].copy_from_slice(&offset.to_be_bytes()); // block_offset
                                                                // header_offset (2), header_size (2), checksum (4) — zeros for test
            entry[16..48].copy_from_slice(hash); // header_hash
            entry[48..56].copy_from_slice(&slot.to_be_bytes()); // block_or_ebb
            secondary_file.write_all(&entry).unwrap();

            offset += cbor.len() as u64;
        }
    }

    #[test]
    fn test_open_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let db = ImmutableDB::open(dir.path()).unwrap();
        assert_eq!(db.total_blocks(), 0);
        assert_eq!(db.tip_slot(), 0);
    }

    #[test]
    fn test_get_block_by_hash() {
        let dir = tempfile::tempdir().unwrap();
        let hash = [42u8; 32];
        create_test_chunk(dir.path(), 0, &[(b"block_data", hash, 100)]);

        let db = ImmutableDB::open(dir.path()).unwrap();
        assert_eq!(db.total_blocks(), 1);
        assert!(db.has_block(&Hash32::from_bytes(hash)));
        assert_eq!(
            db.get_block(&Hash32::from_bytes(hash)).unwrap(),
            b"block_data"
        );
    }

    #[test]
    fn test_missing_block() {
        let dir = tempfile::tempdir().unwrap();
        create_test_chunk(dir.path(), 0, &[(b"data", [1u8; 32], 100)]);

        let db = ImmutableDB::open(dir.path()).unwrap();
        assert!(!db.has_block(&Hash32::from_bytes([99u8; 32])));
        assert!(db.get_block(&Hash32::from_bytes([99u8; 32])).is_none());
    }

    #[test]
    fn test_multiple_chunks() {
        let dir = tempfile::tempdir().unwrap();
        create_test_chunk(
            dir.path(),
            0,
            &[(b"block_a", [1u8; 32], 10), (b"block_b", [2u8; 32], 20)],
        );
        create_test_chunk(dir.path(), 1, &[(b"block_c", [3u8; 32], 30)]);

        let db = ImmutableDB::open(dir.path()).unwrap();
        assert_eq!(db.total_blocks(), 3);
        assert_eq!(db.tip_slot(), 30);
        assert!(db.has_block(&Hash32::from_bytes([1u8; 32])));
        assert!(db.has_block(&Hash32::from_bytes([3u8; 32])));
    }

    #[test]
    fn test_get_next_block_after_slot() {
        let dir = tempfile::tempdir().unwrap();
        create_test_chunk(
            dir.path(),
            0,
            &[
                (b"b1", [1u8; 32], 10),
                (b"b2", [2u8; 32], 20),
                (b"b3", [3u8; 32], 30),
            ],
        );

        let db = ImmutableDB::open(dir.path()).unwrap();

        let (slot, hash, cbor) = db.get_next_block_after_slot(0).unwrap();
        assert_eq!(slot, 10);
        assert_eq!(hash, Hash32::from_bytes([1u8; 32]));
        assert_eq!(cbor, b"b1");

        let (slot, _, cbor) = db.get_next_block_after_slot(10).unwrap();
        assert_eq!(slot, 20);
        assert_eq!(cbor, b"b2");

        let (slot, _, _) = db.get_next_block_after_slot(20).unwrap();
        assert_eq!(slot, 30);

        assert!(db.get_next_block_after_slot(30).is_none());
    }

    #[test]
    fn test_get_blocks_in_slot_range() {
        let dir = tempfile::tempdir().unwrap();
        create_test_chunk(
            dir.path(),
            0,
            &[
                (b"b1", [1u8; 32], 10),
                (b"b2", [2u8; 32], 20),
                (b"b3", [3u8; 32], 30),
            ],
        );

        let db = ImmutableDB::open(dir.path()).unwrap();

        let blocks = db.get_blocks_in_slot_range(10, 20);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0], b"b1");
        assert_eq!(blocks[1], b"b2");

        let blocks = db.get_blocks_in_slot_range(25, 35);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0], b"b3");
    }

    #[test]
    fn test_cross_chunk_slot_range() {
        let dir = tempfile::tempdir().unwrap();
        create_test_chunk(dir.path(), 0, &[(b"b1", [1u8; 32], 10)]);
        create_test_chunk(dir.path(), 1, &[(b"b2", [2u8; 32], 20)]);

        let db = ImmutableDB::open(dir.path()).unwrap();

        let blocks = db.get_blocks_in_slot_range(5, 25);
        assert_eq!(blocks.len(), 2);
    }

    #[test]
    fn test_tip_tracking() {
        let dir = tempfile::tempdir().unwrap();
        create_test_chunk(
            dir.path(),
            0,
            &[(b"b1", [1u8; 32], 100), (b"b2", [2u8; 32], 200)],
        );
        create_test_chunk(dir.path(), 1, &[(b"b3", [3u8; 32], 300)]);

        let db = ImmutableDB::open(dir.path()).unwrap();
        assert_eq!(db.tip_slot(), 300);
        assert_eq!(db.tip_hash(), Hash32::from_bytes([3u8; 32]));
    }
}
