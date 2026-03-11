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
//!
//! ## I/O backends
//!
//! By default, chunk file reads use `memmap2`.  On Linux, enable the
//! `io-uring` feature for kernel-bypassed async I/O via `io_uring`,
//! which improves throughput on NVMe storage for large sequential scans.

use crate::chunk_reader::{self, ChunkReaderTrait};
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
    #[error("Malformed secondary index entry in chunk {chunk}: {reason}")]
    MalformedSecondaryEntry { chunk: u64, reason: String },
}

/// Read a big-endian u64 from an 8-byte slice without panicking.
///
/// Returns `None` if the slice is not exactly 8 bytes.
#[inline]
fn read_be_u64(data: &[u8]) -> Option<u64> {
    let bytes: [u8; 8] = data.try_into().ok()?;
    Some(u64::from_be_bytes(bytes))
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
                if !secondary_data.is_empty() {
                    warn!(
                        chunk = chunk_num,
                        bytes = secondary_data.len(),
                        "Secondary index too small for a single entry ({SECONDARY_ENTRY_SIZE} bytes required), skipping"
                    );
                }
                continue;
            }

            // Warn about trailing bytes that don't form a complete entry
            let remainder = secondary_data.len() % SECONDARY_ENTRY_SIZE;
            if remainder != 0 {
                warn!(
                    chunk = chunk_num,
                    total_bytes = secondary_data.len(),
                    trailing_bytes = remainder,
                    "Secondary index has trailing bytes (possibly truncated)"
                );
            }

            // Parse secondary index entries: (block_offset, header_hash, slot)
            let mut entries: Vec<(u64, [u8; 32], u64)> = Vec::with_capacity(entry_count);
            let mut pos = 0;
            while pos + SECONDARY_ENTRY_SIZE <= secondary_data.len() {
                let data = &secondary_data[pos..];
                let block_offset = match read_be_u64(&data[0..8]) {
                    Some(v) => v,
                    None => {
                        warn!(
                            chunk = chunk_num,
                            pos, "Malformed block_offset in secondary index, skipping entry"
                        );
                        pos += SECONDARY_ENTRY_SIZE;
                        continue;
                    }
                };
                let mut header_hash = [0u8; 32];
                header_hash.copy_from_slice(&data[16..48]);
                let block_or_ebb = match read_be_u64(&data[48..56]) {
                    Some(v) => v,
                    None => {
                        warn!(
                            chunk = chunk_num,
                            pos, "Malformed slot/ebb in secondary index, skipping entry"
                        );
                        pos += SECONDARY_ENTRY_SIZE;
                        continue;
                    }
                };
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
                let block_offset = match read_be_u64(&data[0..8]) {
                    Some(v) => v,
                    None => {
                        pos += SECONDARY_ENTRY_SIZE;
                        continue;
                    }
                };
                let mut header_hash = [0u8; 32];
                header_hash.copy_from_slice(&data[16..48]);
                let slot = match read_be_u64(&data[48..56]) {
                    Some(v) => v,
                    None => {
                        pos += SECONDARY_ENTRY_SIZE;
                        continue;
                    }
                };

                if slot > after_slot {
                    // Compute block end from next entry or chunk length.
                    // Only read the next entry's offset if a full entry exists
                    // (not just trailing garbage bytes).
                    let next_pos = pos + SECONDARY_ENTRY_SIZE;
                    let block_end = if next_pos + SECONDARY_ENTRY_SIZE <= secondary_data.len() {
                        read_be_u64(&secondary_data[next_pos..next_pos + 8]).unwrap_or(chunk_len)
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
    ///
    /// Uses the batched [`ChunkReader::read_ranges`] API to read all
    /// matching blocks from each chunk file in a single I/O operation
    /// when possible (e.g. io_uring submits all reads at once).
    pub fn get_blocks_in_slot_range(&self, from_slot: u64, to_slot: u64) -> Vec<Vec<u8>> {
        let mut result = Vec::new();
        let reader = chunk_reader::default_reader();

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
            let chunk_len = match chunk_path.metadata() {
                Ok(m) => m.len(),
                Err(_) => continue,
            };

            // Parse all entries to get offsets and slots
            let entry_count = secondary_data.len() / SECONDARY_ENTRY_SIZE;
            let mut entries: Vec<(u64, u64)> = Vec::with_capacity(entry_count);
            let mut pos = 0;
            while pos + SECONDARY_ENTRY_SIZE <= secondary_data.len() {
                let data = &secondary_data[pos..];
                let block_offset = match read_be_u64(&data[0..8]) {
                    Some(v) => v,
                    None => {
                        pos += SECONDARY_ENTRY_SIZE;
                        continue;
                    }
                };
                let slot = match read_be_u64(&data[48..56]) {
                    Some(v) => v,
                    None => {
                        pos += SECONDARY_ENTRY_SIZE;
                        continue;
                    }
                };
                entries.push((block_offset, slot));
                pos += SECONDARY_ENTRY_SIZE;
            }

            // Collect the (offset, length) ranges for blocks in the slot window.
            let mut ranges: Vec<(u64, usize)> = Vec::new();
            for i in 0..entries.len() {
                let (block_offset, slot) = entries[i];
                if slot < from_slot {
                    continue;
                }
                if slot > to_slot {
                    break;
                }

                let block_end = if i + 1 < entries.len() {
                    entries[i + 1].0
                } else {
                    chunk_len
                };
                if block_offset < block_end {
                    ranges.push((block_offset, (block_end - block_offset) as usize));
                }
            }

            // Batch-read all selected ranges from this chunk file.
            let batch = reader.read_ranges(&chunk_path, &ranges);
            for block in batch.into_iter().flatten() {
                result.push(block);
            }
        }

        result
    }

    /// Read a block from a chunk file at the given location.
    ///
    /// Uses the configured I/O backend (memmap2 or io_uring).
    fn read_block_at(&self, loc: &BlockLocation) -> Option<Vec<u8>> {
        let chunk_path = self.dir.join(format!("{:05}.chunk", loc.chunk_num));
        let start = loc.block_offset;
        let end = loc.block_end;
        if end <= start {
            warn!(
                chunk = loc.chunk_num,
                offset = start,
                end,
                "Invalid block location (end <= start)"
            );
            return None;
        }
        let len = (end - start) as usize;
        let reader = chunk_reader::default_reader();
        let result = reader.read_range(&chunk_path, start, len);
        if result.is_none() {
            warn!(
                chunk = loc.chunk_num,
                offset = start,
                end,
                "Failed to read block from chunk file"
            );
        }
        result
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

    // -----------------------------------------------------------------------
    // Malformed / truncated secondary index handling
    // -----------------------------------------------------------------------

    #[test]
    fn test_empty_secondary_index() {
        // Empty secondary file should be gracefully skipped
        let dir = tempfile::tempdir().unwrap();
        let chunk_path = dir.path().join("00000.chunk");
        let secondary_path = dir.path().join("00000.secondary");

        // Create a non-empty chunk but an empty secondary index
        fs::File::create(&chunk_path)
            .unwrap()
            .write_all(b"some block data")
            .unwrap();
        fs::File::create(&secondary_path).unwrap();

        let db = ImmutableDB::open(dir.path()).unwrap();
        assert_eq!(db.total_blocks(), 0);
        assert_eq!(db.tip_slot(), 0);
    }

    #[test]
    fn test_truncated_secondary_index_less_than_entry_size() {
        // Secondary file with fewer than 56 bytes should be skipped
        let dir = tempfile::tempdir().unwrap();
        let chunk_path = dir.path().join("00000.chunk");
        let secondary_path = dir.path().join("00000.secondary");

        fs::File::create(&chunk_path)
            .unwrap()
            .write_all(b"block data")
            .unwrap();
        // Write only 30 bytes — not enough for a single 56-byte entry
        fs::File::create(&secondary_path)
            .unwrap()
            .write_all(&[0u8; 30])
            .unwrap();

        let db = ImmutableDB::open(dir.path()).unwrap();
        assert_eq!(db.total_blocks(), 0);
    }

    #[test]
    fn test_truncated_secondary_index_trailing_bytes() {
        // Secondary file with one valid entry + trailing bytes that
        // don't form a complete entry. The valid entry should be parsed;
        // the trailing bytes should be ignored.
        let dir = tempfile::tempdir().unwrap();
        let chunk_path = dir.path().join("00000.chunk");
        let secondary_path = dir.path().join("00000.secondary");

        let block_data = b"hello_block";
        fs::File::create(&chunk_path)
            .unwrap()
            .write_all(block_data)
            .unwrap();

        // Build one valid 56-byte secondary entry
        let mut entry = [0u8; 56];
        entry[0..8].copy_from_slice(&0u64.to_be_bytes()); // block_offset = 0
        entry[16..48].copy_from_slice(&[7u8; 32]); // header_hash
        entry[48..56].copy_from_slice(&42u64.to_be_bytes()); // slot = 42

        let mut secondary_file = fs::File::create(&secondary_path).unwrap();
        secondary_file.write_all(&entry).unwrap();
        // Append 20 trailing garbage bytes (less than a full entry)
        secondary_file.write_all(&[0xFFu8; 20]).unwrap();

        let db = ImmutableDB::open(dir.path()).unwrap();
        assert_eq!(db.total_blocks(), 1);
        assert_eq!(db.tip_slot(), 42);
        assert!(db.has_block(&Hash32::from_bytes([7u8; 32])));

        // Block data should be readable
        let cbor = db.get_block(&Hash32::from_bytes([7u8; 32])).unwrap();
        assert_eq!(cbor, block_data);
    }

    #[test]
    fn test_corrupted_secondary_data_graceful() {
        // Even with corrupted data in the secondary index, the parser
        // should not panic. It may produce wrong block locations, but
        // read_block_at will catch invalid offsets.
        let dir = tempfile::tempdir().unwrap();
        let chunk_path = dir.path().join("00000.chunk");
        let secondary_path = dir.path().join("00000.secondary");

        fs::File::create(&chunk_path)
            .unwrap()
            .write_all(b"data")
            .unwrap();

        // Write 56 bytes of garbage — valid entry size but nonsensical values
        let garbage = [0xABu8; 56];
        fs::File::create(&secondary_path)
            .unwrap()
            .write_all(&garbage)
            .unwrap();

        // Should not panic
        let db = ImmutableDB::open(dir.path()).unwrap();
        assert_eq!(db.total_blocks(), 1);

        // The block offset decoded from garbage will likely be out of range,
        // so get_block should return None without panicking
        let hash = {
            let mut h = [0u8; 32];
            h.copy_from_slice(&garbage[16..48]);
            Hash32::from_bytes(h)
        };
        // get_block won't panic even with wild offsets
        let _ = db.get_block(&hash);
    }

    #[test]
    fn test_missing_chunk_file_skipped() {
        // Secondary exists but chunk file is missing — should skip gracefully
        let dir = tempfile::tempdir().unwrap();
        // Only create a secondary file, no .chunk file
        let secondary_path = dir.path().join("00000.secondary");
        let mut entry = [0u8; 56];
        entry[48..56].copy_from_slice(&100u64.to_be_bytes());
        fs::File::create(&secondary_path)
            .unwrap()
            .write_all(&entry)
            .unwrap();

        // Also need the chunk file to exist for it to be discovered
        // (chunks are discovered by scanning for .chunk files)
        // So this should result in 0 blocks since there's no .chunk
        let db = ImmutableDB::open(dir.path()).unwrap();
        assert_eq!(db.total_blocks(), 0);
    }

    #[test]
    fn test_read_be_u64_helper() {
        // Verify the helper function
        assert_eq!(read_be_u64(&[0, 0, 0, 0, 0, 0, 0, 1]), Some(1));
        assert_eq!(
            read_be_u64(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]),
            Some(u64::MAX)
        );
        assert_eq!(read_be_u64(&[0, 0, 0, 0, 0, 0, 0]), None); // 7 bytes
        assert_eq!(read_be_u64(&[0, 0, 0, 0, 0, 0, 0, 0, 0]), None); // 9 bytes
        assert_eq!(read_be_u64(&[]), None); // empty
    }

    #[test]
    fn test_get_next_block_after_slot_with_truncated_secondary() {
        // Create a valid chunk + secondary, then verify queries work with
        // trailing bytes in the secondary index.
        let dir = tempfile::tempdir().unwrap();
        let chunk_path = dir.path().join("00000.chunk");
        let secondary_path = dir.path().join("00000.secondary");

        fs::File::create(&chunk_path)
            .unwrap()
            .write_all(b"b1b2")
            .unwrap();

        let mut secondary_file = fs::File::create(&secondary_path).unwrap();

        // Entry 1: offset=0, hash=[1;32], slot=10
        let mut e1 = [0u8; 56];
        e1[0..8].copy_from_slice(&0u64.to_be_bytes());
        e1[16..48].copy_from_slice(&[1u8; 32]);
        e1[48..56].copy_from_slice(&10u64.to_be_bytes());
        secondary_file.write_all(&e1).unwrap();

        // Entry 2: offset=2, hash=[2;32], slot=20
        let mut e2 = [0u8; 56];
        e2[0..8].copy_from_slice(&2u64.to_be_bytes());
        e2[16..48].copy_from_slice(&[2u8; 32]);
        e2[48..56].copy_from_slice(&20u64.to_be_bytes());
        secondary_file.write_all(&e2).unwrap();

        // Trailing garbage (less than a full entry)
        secondary_file.write_all(&[0xCC; 10]).unwrap();

        let db = ImmutableDB::open(dir.path()).unwrap();
        assert_eq!(db.total_blocks(), 2);

        // get_next_block_after_slot should work correctly
        let result = db.get_next_block_after_slot(0);
        assert!(result.is_some());
        let (slot, _, _) = result.unwrap();
        assert_eq!(slot, 10);

        let result = db.get_next_block_after_slot(10);
        assert!(result.is_some());
        let (slot, _, _) = result.unwrap();
        assert_eq!(slot, 20);
    }

    #[test]
    fn test_get_blocks_in_slot_range_with_truncated_secondary() {
        // Verify slot range queries gracefully handle trailing bytes
        let dir = tempfile::tempdir().unwrap();
        let chunk_path = dir.path().join("00000.chunk");
        let secondary_path = dir.path().join("00000.secondary");

        fs::File::create(&chunk_path)
            .unwrap()
            .write_all(b"aabb")
            .unwrap();

        let mut secondary_file = fs::File::create(&secondary_path).unwrap();

        let mut e1 = [0u8; 56];
        e1[0..8].copy_from_slice(&0u64.to_be_bytes());
        e1[16..48].copy_from_slice(&[1u8; 32]);
        e1[48..56].copy_from_slice(&10u64.to_be_bytes());
        secondary_file.write_all(&e1).unwrap();

        let mut e2 = [0u8; 56];
        e2[0..8].copy_from_slice(&2u64.to_be_bytes());
        e2[16..48].copy_from_slice(&[2u8; 32]);
        e2[48..56].copy_from_slice(&20u64.to_be_bytes());
        secondary_file.write_all(&e2).unwrap();

        // 40 trailing garbage bytes
        secondary_file.write_all(&[0xDD; 40]).unwrap();

        let db = ImmutableDB::open(dir.path()).unwrap();

        let blocks = db.get_blocks_in_slot_range(5, 25);
        assert_eq!(blocks.len(), 2);
    }
}
