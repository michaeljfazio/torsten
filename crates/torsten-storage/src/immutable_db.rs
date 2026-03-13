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

use crate::block_index::{BlockIndex, BlockLocation, InMemoryBlockIndex};
use crate::chunk_reader::{self, ChunkReaderTrait};
use crate::config::ImmutableConfig;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;
use torsten_primitives::hash::Hash32;
use tracing::{debug, warn};

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

/// Per-chunk metadata for slot-based lookups.
#[derive(Debug, Clone)]
struct ChunkMeta {
    chunk_num: u64,
    first_slot: u64,
    last_slot: u64,
}

/// Active chunk being written to.
struct ActiveChunk {
    chunk_num: u64,
    chunk_file: std::io::BufWriter<std::fs::File>,
    secondary_entries: Vec<SecondaryEntry>,
    current_offset: u64,
    /// In-memory block data for the active chunk (not yet readable via memmap).
    /// Keyed by block hash for O(1) lookup.
    pending_blocks: HashMap<Hash32, Vec<u8>>,
}

/// A buffered secondary index entry (written on finalize or flush).
#[derive(Clone)]
struct SecondaryEntry {
    block_offset: u64,
    header_hash: [u8; 32],
    slot: u64,
}

impl SecondaryEntry {
    fn encode(&self) -> [u8; SECONDARY_ENTRY_SIZE] {
        let mut entry = [0u8; SECONDARY_ENTRY_SIZE];
        entry[0..8].copy_from_slice(&self.block_offset.to_be_bytes());
        // bytes 8..16: header_offset(2), header_size(2), checksum(4) — zeros
        entry[16..48].copy_from_slice(&self.header_hash);
        entry[48..56].copy_from_slice(&self.slot.to_be_bytes());
        entry
    }
}

/// Storage backed by Cardano immutable chunk files.
///
/// Each chunk file (`.chunk`) stores raw block CBOR sequentially.
/// Secondary index files (`.secondary`) provide 56-byte entries with
/// block boundaries, header hashes, and slot numbers.
///
/// Supports both read-only mode (via `open`) and read-write mode
/// (via `open_for_writing`) with append-only writes.
pub struct ImmutableDB {
    dir: PathBuf,
    chunks: Vec<ChunkMeta>,
    block_index: BlockIndex,
    total_blocks: u64,
    tip_slot: u64,
    tip_hash: Hash32,
    tip_block_no: u64,
    /// Active chunk for writing (None in read-only mode).
    active_chunk: Option<ActiveChunk>,
}

impl ImmutableDB {
    /// Open an ImmutableDB from a directory of chunk files using default (in-memory) config.
    ///
    /// Scans all `.chunk` and `.secondary` files and builds an in-memory
    /// hash index for O(1) block lookups. For preview (~4M blocks) this
    /// uses ~300 MB of memory; mainnet will need an on-disk index.
    pub fn open(dir: &Path) -> Result<Self, ImmutableDBError> {
        Self::open_with_config(dir, &ImmutableConfig::default())
    }

    /// Open an ImmutableDB from a directory of chunk files with the given config.
    pub fn open_with_config(
        dir: &Path,
        config: &ImmutableConfig,
    ) -> Result<Self, ImmutableDBError> {
        debug!(dir = %dir.display(), index_type = ?config.index_type, "Opening ImmutableDB");

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
            debug!("ImmutableDB: no chunk files found");
            let block_index = BlockIndex::new(config, dir)?;
            return Ok(ImmutableDB {
                dir: dir.to_path_buf(),
                chunks: Vec::new(),
                block_index,
                total_blocks: 0,
                tip_slot: 0,
                tip_hash: Hash32::ZERO,
                tip_block_no: 0,
                active_chunk: None,
            });
        }

        // First pass: count total entries for pre-allocation
        let mut total_entry_count = 0usize;
        let mut chunks = Vec::with_capacity(chunk_nums.len());
        let mut total_blocks = 0u64;
        let mut tip_slot = 0u64;
        let mut tip_hash = Hash32::ZERO;

        // Collect all (hash, location) pairs for building the index
        let mut all_entries: Vec<(Hash32, BlockLocation)> = Vec::new();

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
                all_entries.push((
                    hash,
                    BlockLocation {
                        chunk_num,
                        block_offset,
                        block_end,
                    },
                ));

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
            total_entry_count += entry_count;
        }

        // Build the block index from collected entries
        let block_index = match config.index_type {
            crate::config::BlockIndexType::InMemory => {
                let mut idx = InMemoryBlockIndex::with_capacity(total_entry_count);
                for (hash, loc) in &all_entries {
                    idx.insert(*hash, *loc);
                }
                BlockIndex::InMemory(idx)
            }
            crate::config::BlockIndexType::Mmap => {
                // Try to reuse existing mmap file if count matches
                let mmap_path = dir.join("hash_index.dat");
                let reuse = if mmap_path.exists() {
                    match crate::block_index::MmapBlockIndex::new(dir, config.mmap_load_factor) {
                        Ok(idx) if idx.count_matches(total_blocks) => {
                            debug!("Reusing existing mmap block index");
                            Some(idx)
                        }
                        Ok(_) => {
                            debug!("Mmap block index count mismatch, rebuilding");
                            None
                        }
                        Err(_) => None,
                    }
                } else {
                    None
                };

                match reuse {
                    Some(idx) => BlockIndex::Mmap(idx),
                    None => {
                        let idx = crate::block_index::MmapBlockIndex::build_from_entries(
                            dir,
                            &all_entries,
                            config.mmap_load_factor,
                        )?;
                        BlockIndex::Mmap(idx)
                    }
                }
            }
        };

        debug!(
            chunks = chunks.len(),
            total_blocks,
            tip_slot,
            index_entries = block_index.len(),
            "ImmutableDB opened"
        );

        // Try to read persisted tip metadata (block_no not in secondary index)
        let tip_block_no = Self::read_tip_meta(dir)
            .map(|(_, _, bn)| bn)
            .unwrap_or(total_blocks);

        Ok(ImmutableDB {
            dir: dir.to_path_buf(),
            chunks,
            block_index,
            total_blocks,
            tip_slot,
            tip_hash,
            tip_block_no,
            active_chunk: None,
        })
    }

    /// Get block CBOR by header hash.
    pub fn get_block(&self, hash: &Hash32) -> Option<Vec<u8>> {
        // Check active chunk's pending blocks first (not yet on disk via memmap)
        if let Some(ref active) = self.active_chunk {
            if let Some(cbor) = active.pending_blocks.get(hash) {
                return Some(Vec::clone(cbor));
            }
        }
        let loc = self.block_index.lookup(hash)?;
        self.read_block_at(&loc)
    }

    /// Check if a block exists by header hash.
    pub fn has_block(&self, hash: &Hash32) -> bool {
        self.block_index.contains(hash)
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

    /// Tip block number of the immutable chain.
    pub fn tip_block_no(&self) -> u64 {
        self.tip_block_no
    }

    /// Directory containing the chunk files.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Read tip metadata from a `tip.meta` file in the directory.
    fn read_tip_meta(dir: &Path) -> Option<(u64, Hash32, u64)> {
        let meta_path = dir.join("tip.meta");
        let data = fs::read(&meta_path).ok()?;
        if data.len() < 48 {
            return None;
        }
        let slot = u64::from_be_bytes(data[0..8].try_into().ok()?);
        let mut hash_bytes = [0u8; 32];
        hash_bytes.copy_from_slice(&data[8..40]);
        let block_no = u64::from_be_bytes(data[40..48].try_into().ok()?);
        Some((slot, Hash32::from_bytes(hash_bytes), block_no))
    }

    /// Write tip metadata to a `tip.meta` file.
    fn write_tip_meta(
        dir: &Path,
        slot: u64,
        hash: &Hash32,
        block_no: u64,
    ) -> Result<(), ImmutableDBError> {
        let meta_path = dir.join("tip.meta");
        let mut data = [0u8; 48];
        data[0..8].copy_from_slice(&slot.to_be_bytes());
        data[8..40].copy_from_slice(hash.as_bytes());
        data[40..48].copy_from_slice(&block_no.to_be_bytes());
        fs::write(&meta_path, data)?;
        Ok(())
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

    /// Open an ImmutableDB for writing, appending to existing chunk files.
    ///
    /// Scans existing chunks read-only (like `open`), then prepares the
    /// next chunk number for writing.
    pub fn open_for_writing(dir: &Path) -> Result<Self, ImmutableDBError> {
        Self::open_for_writing_with_config(dir, &ImmutableConfig::default())
    }

    /// Open an ImmutableDB for writing with the given config.
    pub fn open_for_writing_with_config(
        dir: &Path,
        config: &ImmutableConfig,
    ) -> Result<Self, ImmutableDBError> {
        let mut db = Self::open_with_config(dir, config)?;

        // Determine next chunk number
        let next_chunk = db.chunks.last().map_or(0, |c| c.chunk_num + 1);

        let chunk_path = dir.join(format!("{next_chunk:05}.chunk"));
        let file = std::fs::File::create(&chunk_path)?;
        let writer = std::io::BufWriter::new(file);

        db.active_chunk = Some(ActiveChunk {
            chunk_num: next_chunk,
            chunk_file: writer,
            secondary_entries: Vec::new(),
            current_offset: 0,
            pending_blocks: HashMap::new(),
        });

        debug!(
            next_chunk,
            existing_chunks = db.chunks.len(),
            "ImmutableDB opened for writing"
        );

        Ok(db)
    }

    /// Append a block to the active chunk.
    ///
    /// Updates the in-memory hash index immediately so the block is
    /// readable before the secondary index is flushed.
    pub fn append_block(
        &mut self,
        slot: u64,
        _block_no: u64,
        hash: &Hash32,
        cbor: &[u8],
    ) -> Result<(), ImmutableDBError> {
        use std::io::Write;

        let active = self.active_chunk.as_mut().ok_or_else(|| {
            ImmutableDBError::Io(std::io::Error::other("ImmutableDB not opened for writing"))
        })?;

        let block_offset = active.current_offset;
        active.chunk_file.write_all(cbor)?;
        active.current_offset += cbor.len() as u64;

        // Buffer secondary entry and block data for reads
        active.secondary_entries.push(SecondaryEntry {
            block_offset,
            header_hash: *hash.as_bytes(),
            slot,
        });
        active.pending_blocks.insert(*hash, cbor.to_vec());

        // Update index for immediate reads
        let block_end = active.current_offset;
        self.block_index.insert(
            *hash,
            BlockLocation {
                chunk_num: active.chunk_num,
                block_offset,
                block_end,
            },
        );

        self.total_blocks += 1;
        if slot >= self.tip_slot {
            self.tip_slot = slot;
            self.tip_hash = *hash;
            self.tip_block_no = _block_no;
        }

        Ok(())
    }

    /// Finalize the current chunk: write its `.secondary` index and open
    /// a new chunk file. Call this at epoch boundaries.
    pub fn finalize_chunk(&mut self) -> Result<(), ImmutableDBError> {
        use std::io::Write;

        let active = match self.active_chunk.take() {
            Some(a) => a,
            None => return Ok(()),
        };

        // Flush the chunk file
        let mut chunk_file = active.chunk_file;
        chunk_file.flush()?;

        // Write secondary index
        let secondary_path = self.dir.join(format!("{:05}.secondary", active.chunk_num));
        let mut secondary_file = std::io::BufWriter::new(std::fs::File::create(&secondary_path)?);
        for entry in &active.secondary_entries {
            secondary_file.write_all(&entry.encode())?;
        }
        secondary_file.flush()?;

        // Update chunk metadata
        if let (Some(first), Some(last)) = (
            active.secondary_entries.first(),
            active.secondary_entries.last(),
        ) {
            self.chunks.push(ChunkMeta {
                chunk_num: active.chunk_num,
                first_slot: first.slot,
                last_slot: last.slot,
            });
        }

        // Open new chunk for writing
        let next_chunk = active.chunk_num + 1;
        let chunk_path = self.dir.join(format!("{next_chunk:05}.chunk"));
        let file = std::fs::File::create(&chunk_path)?;
        self.active_chunk = Some(ActiveChunk {
            chunk_num: next_chunk,
            chunk_file: std::io::BufWriter::new(file),
            secondary_entries: Vec::new(),
            current_offset: 0,
            pending_blocks: HashMap::new(),
        });

        debug!(
            finalized_chunk = active.chunk_num,
            next_chunk, "ImmutableDB: chunk finalized"
        );
        Ok(())
    }

    /// Flush the active chunk's secondary index to disk without starting
    /// a new chunk. Call this on shutdown to ensure durability.
    pub fn flush(&mut self) -> Result<(), ImmutableDBError> {
        use std::io::Write;

        let active = match self.active_chunk.as_mut() {
            Some(a) => a,
            None => return Ok(()),
        };

        // Flush chunk data
        active.chunk_file.flush()?;

        // Write secondary index for current state
        let secondary_path = self.dir.join(format!("{:05}.secondary", active.chunk_num));
        let mut secondary_file = std::io::BufWriter::new(std::fs::File::create(&secondary_path)?);
        for entry in &active.secondary_entries {
            secondary_file.write_all(&entry.encode())?;
        }
        secondary_file.flush()?;

        // Update chunk metadata (replace existing entry for this chunk if present)
        if let (Some(first), Some(last)) = (
            active.secondary_entries.first(),
            active.secondary_entries.last(),
        ) {
            let chunk_num = active.chunk_num;
            if let Some(existing) = self.chunks.iter_mut().find(|c| c.chunk_num == chunk_num) {
                existing.first_slot = first.slot;
                existing.last_slot = last.slot;
            } else {
                self.chunks.push(ChunkMeta {
                    chunk_num,
                    first_slot: first.slot,
                    last_slot: last.slot,
                });
            }
        }

        // Persist tip metadata
        if self.tip_slot > 0 {
            Self::write_tip_meta(&self.dir, self.tip_slot, &self.tip_hash, self.tip_block_no)?;
        }

        // Persist block index (mmap flush)
        self.block_index.persist()?;

        debug!(
            chunk = active.chunk_num,
            entries = active.secondary_entries.len(),
            "ImmutableDB: flushed active chunk"
        );
        Ok(())
    }

    /// Whether this ImmutableDB is open for writing.
    pub fn is_writable(&self) -> bool {
        self.active_chunk.is_some()
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

    // -----------------------------------------------------------------------
    // Mmap block index integration tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_open_with_mmap_config() {
        let dir = tempfile::tempdir().unwrap();
        let hash = [42u8; 32];
        create_test_chunk(dir.path(), 0, &[(b"block_data", hash, 100)]);

        let config = ImmutableConfig {
            index_type: crate::config::BlockIndexType::Mmap,
            mmap_load_factor: 0.7,
            mmap_initial_capacity: 0,
        };
        let db = ImmutableDB::open_with_config(dir.path(), &config).unwrap();
        assert_eq!(db.total_blocks(), 1);
        assert!(db.has_block(&Hash32::from_bytes(hash)));
        assert_eq!(
            db.get_block(&Hash32::from_bytes(hash)).unwrap(),
            b"block_data"
        );
        // hash_index.dat should be created
        assert!(dir.path().join("hash_index.dat").exists());
    }

    #[test]
    fn test_mmap_multiple_chunks() {
        let dir = tempfile::tempdir().unwrap();
        create_test_chunk(
            dir.path(),
            0,
            &[(b"block_a", [1u8; 32], 10), (b"block_b", [2u8; 32], 20)],
        );
        create_test_chunk(dir.path(), 1, &[(b"block_c", [3u8; 32], 30)]);

        let config = ImmutableConfig {
            index_type: crate::config::BlockIndexType::Mmap,
            mmap_load_factor: 0.7,
            mmap_initial_capacity: 0,
        };
        let db = ImmutableDB::open_with_config(dir.path(), &config).unwrap();
        assert_eq!(db.total_blocks(), 3);
        assert_eq!(db.tip_slot(), 30);
        assert!(db.has_block(&Hash32::from_bytes([1u8; 32])));
        assert!(db.has_block(&Hash32::from_bytes([3u8; 32])));
    }

    #[test]
    fn test_mmap_reuses_existing_index() {
        let dir = tempfile::tempdir().unwrap();
        let config = ImmutableConfig {
            index_type: crate::config::BlockIndexType::Mmap,
            mmap_load_factor: 0.7,
            mmap_initial_capacity: 0,
        };

        create_test_chunk(dir.path(), 0, &[(b"b1", [1u8; 32], 10)]);

        // First open — builds hash_index.dat
        let db1 = ImmutableDB::open_with_config(dir.path(), &config).unwrap();
        assert_eq!(db1.total_blocks(), 1);
        drop(db1);

        // Second open — should reuse existing hash_index.dat (count matches)
        let db2 = ImmutableDB::open_with_config(dir.path(), &config).unwrap();
        assert_eq!(db2.total_blocks(), 1);
        assert!(db2.has_block(&Hash32::from_bytes([1u8; 32])));
    }

    #[test]
    fn test_mmap_rebuild_when_stale() {
        let dir = tempfile::tempdir().unwrap();
        let config = ImmutableConfig {
            index_type: crate::config::BlockIndexType::Mmap,
            mmap_load_factor: 0.7,
            mmap_initial_capacity: 0,
        };

        create_test_chunk(dir.path(), 0, &[(b"b1", [1u8; 32], 10)]);

        // First open — builds hash_index.dat
        let db1 = ImmutableDB::open_with_config(dir.path(), &config).unwrap();
        drop(db1);

        // Add another chunk — now the index is stale
        create_test_chunk(dir.path(), 1, &[(b"b2", [2u8; 32], 20)]);

        // Reopen — should rebuild since count changed
        let db2 = ImmutableDB::open_with_config(dir.path(), &config).unwrap();
        assert_eq!(db2.total_blocks(), 2);
        assert!(db2.has_block(&Hash32::from_bytes([1u8; 32])));
        assert!(db2.has_block(&Hash32::from_bytes([2u8; 32])));
    }

    #[test]
    fn test_open_empty_dir_with_mmap() {
        let dir = tempfile::tempdir().unwrap();
        let config = ImmutableConfig {
            index_type: crate::config::BlockIndexType::Mmap,
            mmap_load_factor: 0.7,
            mmap_initial_capacity: 0,
        };
        let db = ImmutableDB::open_with_config(dir.path(), &config).unwrap();
        assert_eq!(db.total_blocks(), 0);
        assert_eq!(db.tip_slot(), 0);
    }

    #[test]
    fn test_open_for_writing_with_mmap_config() {
        let dir = tempfile::tempdir().unwrap();
        create_test_chunk(dir.path(), 0, &[(b"b1", [1u8; 32], 10)]);

        let config = ImmutableConfig {
            index_type: crate::config::BlockIndexType::Mmap,
            mmap_load_factor: 0.7,
            mmap_initial_capacity: 0,
        };
        let mut db = ImmutableDB::open_for_writing_with_config(dir.path(), &config).unwrap();
        assert!(db.is_writable());
        assert_eq!(db.total_blocks(), 1);

        // Append a block
        let new_hash = Hash32::from_bytes([99u8; 32]);
        db.append_block(20, 2, &new_hash, b"new_block").unwrap();
        assert!(db.has_block(&new_hash));
        assert_eq!(db.get_block(&new_hash).unwrap(), b"new_block");
        assert_eq!(db.total_blocks(), 2);
    }

    #[test]
    fn test_default_config_matches_original_behavior() {
        // Default config should produce identical results to open()
        let dir = tempfile::tempdir().unwrap();
        create_test_chunk(
            dir.path(),
            0,
            &[(b"b1", [1u8; 32], 10), (b"b2", [2u8; 32], 20)],
        );

        let db_default = ImmutableDB::open(dir.path()).unwrap();
        let db_config =
            ImmutableDB::open_with_config(dir.path(), &ImmutableConfig::default()).unwrap();

        assert_eq!(db_default.total_blocks(), db_config.total_blocks());
        assert_eq!(db_default.tip_slot(), db_config.tip_slot());
        assert_eq!(db_default.tip_hash(), db_config.tip_hash());
    }
}
