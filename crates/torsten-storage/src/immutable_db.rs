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

/// Read a CRC32 checksum from bytes 12..16 of a secondary index entry.
///
/// Returns 0 if the field is all zeros (legacy entries without CRC).
#[inline]
fn read_crc32_from_entry(data: &[u8]) -> u32 {
    if data.len() < 16 {
        return 0;
    }
    u32::from_be_bytes([data[12], data[13], data[14], data[15]])
}

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
    /// CRC32 checksum of the block CBOR data (0 for legacy entries).
    checksum: u32,
    /// Byte offset from the start of the block CBOR to the header element.
    /// Used by db-sync for efficient header extraction. 0 if unknown.
    header_offset: u16,
    /// Byte size of the header CBOR element.
    /// Used by db-sync for efficient header extraction. 0 if unknown.
    header_size: u16,
}

impl SecondaryEntry {
    fn encode(&self) -> [u8; SECONDARY_ENTRY_SIZE] {
        let mut entry = [0u8; SECONDARY_ENTRY_SIZE];
        entry[0..8].copy_from_slice(&self.block_offset.to_be_bytes());
        // bytes 8..10: header_offset (u16 big-endian)
        entry[8..10].copy_from_slice(&self.header_offset.to_be_bytes());
        // bytes 10..12: header_size (u16 big-endian)
        entry[10..12].copy_from_slice(&self.header_size.to_be_bytes());
        // bytes 12..16: CRC32 checksum
        entry[12..16].copy_from_slice(&self.checksum.to_be_bytes());
        entry[16..48].copy_from_slice(&self.header_hash);
        entry[48..56].copy_from_slice(&self.slot.to_be_bytes());
        entry
    }
}

/// Extract the byte offset and size of the block header within block CBOR.
///
/// Cardano post-Shelley blocks are encoded as:
///   `array(2) [era_id, array(N) [header, tx_bodies, witnesses, ...]]`
///
/// Byron blocks (era 0) use the same outer structure but with a different
/// inner layout; we still extract the first element of the inner structure.
///
/// This function performs minimal CBOR parsing — it only needs to skip the
/// outer array tag, the era_id integer, the inner array tag, and then read
/// the header element length. Returns `(header_offset, header_size)` or
/// `(0, 0)` if parsing fails.
fn extract_header_bounds(cbor: &[u8]) -> (u16, u16) {
    // Helper: decode a CBOR initial byte and return (major_type, argument, bytes_consumed).
    // For indefinite-length items (additional info 31), argument is 0.
    fn decode_cbor_head(data: &[u8]) -> Option<(u8, u64, usize)> {
        if data.is_empty() {
            return None;
        }
        let major = data[0] >> 5;
        let additional = data[0] & 0x1f;
        match additional {
            0..=23 => Some((major, additional as u64, 1)),
            24 => {
                if data.len() < 2 {
                    return None;
                }
                Some((major, data[1] as u64, 2))
            }
            25 => {
                if data.len() < 3 {
                    return None;
                }
                let val = u16::from_be_bytes([data[1], data[2]]);
                Some((major, val as u64, 3))
            }
            26 => {
                if data.len() < 5 {
                    return None;
                }
                let val = u32::from_be_bytes([data[1], data[2], data[3], data[4]]);
                Some((major, val as u64, 5))
            }
            27 => {
                if data.len() < 9 {
                    return None;
                }
                let val = u64::from_be_bytes([
                    data[1], data[2], data[3], data[4], data[5], data[6], data[7], data[8],
                ]);
                Some((major, val, 9))
            }
            31 => Some((major, 0, 1)), // indefinite length
            _ => None,
        }
    }

    // Helper: skip one complete CBOR data item, returning bytes consumed.
    fn skip_cbor_item(data: &[u8]) -> Option<usize> {
        let (major, arg, head_len) = decode_cbor_head(data)?;
        match major {
            // 0: unsigned int, 1: negative int — head only
            0 | 1 => Some(head_len),
            // 2: byte string, 3: text string — head + arg bytes
            2 | 3 => Some(head_len + arg as usize),
            // 4: array — head + skip `arg` items
            4 => {
                let mut pos = head_len;
                for _ in 0..arg {
                    pos += skip_cbor_item(&data[pos..])?;
                }
                Some(pos)
            }
            // 5: map — head + skip 2*arg items (key+value pairs)
            5 => {
                let mut pos = head_len;
                for _ in 0..arg * 2 {
                    pos += skip_cbor_item(&data[pos..])?;
                }
                Some(pos)
            }
            // 6: tag — head + one nested item
            6 => {
                let nested = skip_cbor_item(&data[head_len..])?;
                Some(head_len + nested)
            }
            // 7: simple/float — head only
            7 => Some(head_len),
            _ => None,
        }
    }

    let result = (|| -> Option<(u16, u16)> {
        let mut pos = 0;

        // Outer: array(2) [era_id, block_body]
        let (major, _len, head_len) = decode_cbor_head(&cbor[pos..])?;
        if major != 4 {
            return None;
        }
        pos += head_len;

        // Skip era_id (unsigned integer)
        let era_skip = skip_cbor_item(&cbor[pos..])?;
        pos += era_skip;

        // Inner: array(N) [header, ...]
        let (major, _len, head_len) = decode_cbor_head(&cbor[pos..])?;
        if major != 4 {
            return None;
        }
        pos += head_len;

        // `pos` now points to the start of the header element.
        let header_start = pos;
        let header_skip = skip_cbor_item(&cbor[pos..])?;

        // Clamp to u16 range (headers should never exceed 64 KiB)
        let offset = u16::try_from(header_start).ok()?;
        let size = u16::try_from(header_skip).ok()?;
        Some((offset, size))
    })();

    result.unwrap_or((0, 0))
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
    /// CRC32 checksums for blocks (hash -> checksum). Zero means no checksum (legacy).
    checksums: HashMap<Hash32, u32>,
}

impl ImmutableDB {
    /// Open an ImmutableDB from a directory of chunk files using default (in-memory) config.
    ///
    /// Scans all `.chunk` and `.secondary` files and builds an in-memory
    /// hash index for O(1) block lookups. For preview (~4M blocks) this
    /// uses ~300 MB of memory; mainnet will need an on-disk index.
    ///
    /// After building the index, validates the most recent chunk to detect
    /// partial writes or corruption from an unclean shutdown. Corrupt entries
    /// at the tail of the last chunk are truncated so the database is always
    /// in a consistent state before use.
    pub fn open(dir: &Path) -> Result<Self, ImmutableDBError> {
        let mut db = Self::open_with_config(dir, &ImmutableConfig::default())?;
        db.validate_most_recent_chunk()?;
        Ok(db)
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
                checksums: HashMap::new(),
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
        let mut checksums: HashMap<Hash32, u32> = HashMap::new();

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

            // Parse secondary index entries: (block_offset, header_hash, slot, checksum)
            let mut entries: Vec<(u64, [u8; 32], u64, u32)> = Vec::with_capacity(entry_count);
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
                let checksum = read_crc32_from_entry(data);
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
                entries.push((block_offset, header_hash, block_or_ebb, checksum));
                pos += SECONDARY_ENTRY_SIZE;
            }

            let mut first_slot = u64::MAX;
            let mut last_slot = 0u64;

            for i in 0..entries.len() {
                let (block_offset, header_hash, slot, checksum) = entries[i];
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

                // Store non-zero checksums for read-time verification
                if checksum != 0 {
                    checksums.insert(hash, checksum);
                }

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
            checksums,
        })
    }

    /// Validate the most recent (last) finalized chunk on disk.
    ///
    /// Reads the secondary index for the last chunk and checks the CRC32
    /// of each block entry.  Any entries whose CRC32 does not match are
    /// assumed to result from a partial write during an unclean shutdown.
    ///
    /// When corrupt entries are found at the END of the chunk, the method:
    /// 1. Truncates the chunk file to exclude the corrupt entries.
    /// 2. Rewrites the secondary index without those entries.
    /// 3. Updates in-memory state (block index, checksums, tip) accordingly.
    ///
    /// Corrupt entries in the *middle* of the chunk (i.e. with valid entries
    /// after them) are unusual and indicate hardware failure.  In that case
    /// the corrupt block is removed from the index so reads don't surface it,
    /// but the file is not truncated (truncating would discard later valid
    /// data).  A `warn!` is emitted for diagnostic purposes.
    ///
    /// Legacy entries with CRC32 == 0 are skipped (no checksum to verify).
    pub fn validate_most_recent_chunk(&mut self) -> Result<(), ImmutableDBError> {
        let last_chunk = match self.chunks.last().cloned() {
            Some(c) => c,
            None => return Ok(()), // Nothing to validate
        };

        let chunk_path = self.dir.join(format!("{:05}.chunk", last_chunk.chunk_num));
        let secondary_path = self
            .dir
            .join(format!("{:05}.secondary", last_chunk.chunk_num));

        // If either file is missing, nothing to do (can happen on first run)
        if !chunk_path.exists() || !secondary_path.exists() {
            return Ok(());
        }

        let chunk_data = fs::read(&chunk_path)?;
        let secondary_data = fs::read(&secondary_path)?;

        let entry_count = secondary_data.len() / SECONDARY_ENTRY_SIZE;
        if entry_count == 0 {
            return Ok(());
        }

        // Build a list of (block_offset, block_end, hash, checksum) from the
        // secondary index so we can verify each entry independently.
        let mut entries_meta: Vec<(u64, u64, [u8; 32], u32)> = Vec::with_capacity(entry_count);
        let chunk_len = chunk_data.len() as u64;

        let mut pos = 0;
        while pos + SECONDARY_ENTRY_SIZE <= secondary_data.len() {
            let data = &secondary_data[pos..pos + SECONDARY_ENTRY_SIZE];
            let Some(block_offset) = read_be_u64(&data[0..8]) else {
                pos += SECONDARY_ENTRY_SIZE;
                continue;
            };
            let checksum = read_crc32_from_entry(data);
            let mut hash_bytes = [0u8; 32];
            hash_bytes.copy_from_slice(&data[16..48]);

            // Determine block end: start of next entry's block_offset, or chunk_len
            let next_offset = if pos + SECONDARY_ENTRY_SIZE < secondary_data.len() {
                let next_data = &secondary_data[pos + SECONDARY_ENTRY_SIZE..];
                read_be_u64(&next_data[0..8]).unwrap_or(chunk_len)
            } else {
                chunk_len
            };

            entries_meta.push((block_offset, next_offset, hash_bytes, checksum));
            pos += SECONDARY_ENTRY_SIZE;
        }

        // Scan entries for CRC32 mismatches.  Track the index of the first bad entry.
        let mut first_bad_tail: Option<usize> = None;
        let mut any_bad_middle = false;

        for (i, &(block_offset, block_end, _hash, checksum)) in entries_meta.iter().enumerate() {
            // Skip legacy entries without CRC
            if checksum == 0 {
                continue;
            }

            let start = block_offset as usize;
            let end = block_end as usize;

            if end > chunk_data.len() || start > end {
                // Truncated block data — this is a tail corruption.
                if first_bad_tail.is_none() {
                    first_bad_tail = Some(i);
                }
                continue;
            }

            let actual_crc = crc32fast::hash(&chunk_data[start..end]);
            if actual_crc != checksum {
                if i == entries_meta.len() - 1 || first_bad_tail.is_some() {
                    // Bad tail entry
                    if first_bad_tail.is_none() {
                        first_bad_tail = Some(i);
                    }
                } else {
                    // Bad middle entry — unusual, don't truncate
                    let hash = Hash32::from_bytes(entries_meta[i].2);
                    warn!(
                        chunk = last_chunk.chunk_num,
                        offset = block_offset,
                        hash = %hash.to_hex(),
                        "ImmutableDB: CRC32 mismatch for middle block entry — removing from index"
                    );
                    self.block_index.remove(&hash);
                    self.checksums.remove(&hash);
                    any_bad_middle = true;
                }
            }
        }

        if let Some(bad_start) = first_bad_tail {
            let good_count = bad_start;
            let truncate_at = if good_count > 0 {
                entries_meta[bad_start].0 // block_offset of first bad entry
            } else {
                0
            };

            warn!(
                chunk = last_chunk.chunk_num,
                bad_entries = entries_meta.len() - good_count,
                truncate_bytes = truncate_at,
                "ImmutableDB: truncating corrupt tail entries from last chunk"
            );

            // Remove bad entries from in-memory indexes
            for &(_offset, _end, hash_bytes, _crc) in &entries_meta[bad_start..] {
                let hash = Hash32::from_bytes(hash_bytes);
                self.block_index.remove(&hash);
                self.checksums.remove(&hash);
            }

            // Truncate the chunk file
            let file = std::fs::OpenOptions::new().write(true).open(&chunk_path)?;
            file.set_len(truncate_at)?;
            file.sync_all()?;

            // Rewrite the secondary index with only the valid entries
            let good_secondary = &secondary_data[..good_count * SECONDARY_ENTRY_SIZE];
            fs::write(&secondary_path, good_secondary)?;

            // Recalculate tip from the remaining entries
            if good_count > 0 {
                // Tip is the last good entry's hash and slot
                let last_good = good_count - 1;
                let data = &secondary_data[last_good * SECONDARY_ENTRY_SIZE..];
                let slot = read_be_u64(&data[48..56]).unwrap_or(0);
                let mut hash_bytes = [0u8; 32];
                hash_bytes.copy_from_slice(&data[16..48]);
                let hash = Hash32::from_bytes(hash_bytes);
                if slot >= self.tip_slot {
                    self.tip_slot = slot;
                    self.tip_hash = hash;
                }
                self.total_blocks -= (entries_meta.len() - good_count) as u64;
            } else {
                // Entire last chunk is corrupt — remove it from chunks list
                self.chunks.pop();
                self.total_blocks -= entries_meta.len() as u64;
            }
        } else if any_bad_middle {
            // Recalculate tip since middle entries were removed
            debug!(
                chunk = last_chunk.chunk_num,
                "ImmutableDB: removed corrupt middle entries, tip unchanged"
            );
        }

        Ok(())
    }

    /// Get block CBOR by header hash.
    ///
    /// Verifies CRC32 checksum if one was stored in the secondary index.
    /// On mismatch, logs an error and returns `None` so the caller can
    /// re-fetch from a peer rather than silently propagating corrupt data.
    /// Legacy entries (no stored CRC) are returned without verification.
    pub fn get_block(&self, hash: &Hash32) -> Option<Vec<u8>> {
        // Check active chunk's pending blocks first (not yet on disk via memmap)
        if let Some(ref active) = self.active_chunk {
            if let Some(cbor) = active.pending_blocks.get(hash) {
                return Some(Vec::clone(cbor));
            }
        }
        let loc = self.block_index.lookup(hash)?;
        let cbor = self.read_block_at(&loc)?;

        // Verify CRC32 if we have a stored checksum
        if let Some(&expected_crc) = self.checksums.get(hash) {
            let actual_crc = crc32fast::hash(&cbor);
            if actual_crc != expected_crc {
                warn!(
                    hash = %hash.to_hex(),
                    expected = expected_crc,
                    actual = actual_crc,
                    "CRC32 mismatch for block — rejecting corrupted data"
                );
                return None;
            }
        }

        Some(cbor)
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

        // Compute CRC32 of the block CBOR for integrity verification
        let checksum = crc32fast::hash(cbor);

        // Extract header offset and size for db-sync compatibility
        let (header_offset, header_size) = extract_header_bounds(cbor);

        // Buffer secondary entry and block data for reads
        active.secondary_entries.push(SecondaryEntry {
            block_offset,
            header_hash: *hash.as_bytes(),
            slot,
            checksum,
            header_offset,
            header_size,
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

        // Store checksum for read-time verification
        self.checksums.insert(*hash, checksum);

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

        // Flush and fsync the chunk file to guarantee durability before
        // writing the secondary index. Without this, a crash could leave the
        // chunk file with missing tail data while the secondary index already
        // references those blocks.
        let mut chunk_file = active.chunk_file;
        chunk_file.flush()?;
        chunk_file.get_ref().sync_data()?;

        // Write secondary index
        let secondary_path = self.dir.join(format!("{:05}.secondary", active.chunk_num));
        let mut secondary_file = std::io::BufWriter::new(std::fs::File::create(&secondary_path)?);
        for entry in &active.secondary_entries {
            secondary_file.write_all(&entry.encode())?;
        }
        secondary_file.flush()?;
        // Fsync the secondary index so that the chunk is fully recoverable
        // on restart even if the OS crashes immediately after this call.
        secondary_file.get_ref().sync_data()?;

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

        // Flush and fsync chunk data to guarantee durability. Without
        // sync_data(), the OS may buffer writes indefinitely and a crash
        // could lose the tail of the active chunk.
        active.chunk_file.flush()?;
        active.chunk_file.get_ref().sync_data()?;

        // Write secondary index for current state
        let secondary_path = self.dir.join(format!("{:05}.secondary", active.chunk_num));
        let mut secondary_file = std::io::BufWriter::new(std::fs::File::create(&secondary_path)?);
        for entry in &active.secondary_entries {
            secondary_file.write_all(&entry.encode())?;
        }
        secondary_file.flush()?;
        secondary_file.get_ref().sync_data()?;

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

    /// Return up to `max_count` historical (slot, hash) points by sampling
    /// the last entry of older chunks in reverse order.
    ///
    /// This is used for ChainSync intersection negotiation. When the
    /// immutable tip itself is an orphaned block (e.g. a forged block that
    /// was flushed via `flush_all_to_immutable` on graceful shutdown), the
    /// older chunk tips provide canonical points the peer can intersect on.
    pub fn get_historical_points(&self, max_count: usize) -> Vec<(u64, Hash32)> {
        let mut points = Vec::new();
        // Walk chunks in reverse (newest → oldest), reading the LAST
        // secondary entry from each chunk to get its tip (slot, hash).
        for chunk_meta in self.chunks.iter().rev() {
            if points.len() >= max_count {
                break;
            }
            let secondary_path = self
                .dir
                .join(format!("{:05}.secondary", chunk_meta.chunk_num));
            let secondary_data = match fs::read(&secondary_path) {
                Ok(data) => data,
                Err(_) => continue,
            };
            if secondary_data.len() < SECONDARY_ENTRY_SIZE {
                continue;
            }
            // Read the LAST entry in the secondary index.
            let last_entry_offset =
                (secondary_data.len() / SECONDARY_ENTRY_SIZE - 1) * SECONDARY_ENTRY_SIZE;
            let data = &secondary_data[last_entry_offset..];
            let mut header_hash = [0u8; 32];
            header_hash.copy_from_slice(&data[16..48]);
            if let Some(slot) = read_be_u64(&data[48..56]) {
                let hash = Hash32::from_bytes(header_hash);
                // Skip duplicates (same hash/slot as a previously added point).
                if !points.iter().any(|(s, h)| *s == slot && *h == hash) {
                    points.push((slot, hash));
                }
            }
        }
        points
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

    // -----------------------------------------------------------------------
    // CRC32 verification tests
    // -----------------------------------------------------------------------

    /// Build a chunk file + secondary index with CRC32 checksums in the
    /// secondary entries.
    fn create_test_chunk_with_crc(
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
            chunk_file.write_all(cbor).unwrap();

            let checksum = crc32fast::hash(cbor);

            let mut entry = [0u8; 56];
            entry[0..8].copy_from_slice(&offset.to_be_bytes());
            // bytes 12..16: CRC32 checksum
            entry[12..16].copy_from_slice(&checksum.to_be_bytes());
            entry[16..48].copy_from_slice(hash);
            entry[48..56].copy_from_slice(&slot.to_be_bytes());
            secondary_file.write_all(&entry).unwrap();

            offset += cbor.len() as u64;
        }
    }

    #[test]
    fn test_crc32_write_and_verify() {
        // Blocks written via append_block should have CRC32 stored
        let dir = tempfile::tempdir().unwrap();
        let mut db = ImmutableDB::open_for_writing(dir.path()).unwrap();

        let hash = Hash32::from_bytes([42u8; 32]);
        let cbor = b"test block data with CRC";
        db.append_block(100, 1, &hash, cbor).unwrap();

        // Read back — should succeed with valid CRC
        let result = db.get_block(&hash).unwrap();
        assert_eq!(result, cbor);

        // Verify the checksum was stored
        assert_eq!(db.checksums.get(&hash), Some(&crc32fast::hash(cbor)));
    }

    #[test]
    fn test_crc32_persisted_in_secondary_index() {
        // Write blocks, flush, re-open, verify CRC is loaded from secondary index
        let dir = tempfile::tempdir().unwrap();
        let hash = Hash32::from_bytes([42u8; 32]);
        let cbor = b"block for CRC persistence test";

        {
            let mut db = ImmutableDB::open_for_writing(dir.path()).unwrap();
            db.append_block(100, 1, &hash, cbor).unwrap();
            db.flush().unwrap();
        }

        // Re-open and verify CRC is loaded
        let db = ImmutableDB::open(dir.path()).unwrap();
        assert!(db.checksums.contains_key(&hash));
        assert_eq!(db.checksums[&hash], crc32fast::hash(cbor));

        // Read should succeed
        let result = db.get_block(&hash).unwrap();
        assert_eq!(result, cbor);
    }

    #[test]
    fn test_crc32_mismatch_detection_rejects_corrupted_data() {
        // Create a chunk with valid CRC, then corrupt the chunk data.
        // The read should return None to prevent propagation of corrupt data.
        let dir = tempfile::tempdir().unwrap();
        let hash = [42u8; 32];
        let cbor = b"original data";

        // Create chunk with correct CRC
        create_test_chunk_with_crc(dir.path(), 0, &[(cbor, hash, 100)]);

        // Now corrupt the chunk file by overwriting the data
        let chunk_path = dir.path().join("00000.chunk");
        fs::write(&chunk_path, b"corrupted dat").unwrap(); // same length, different content

        let db = ImmutableDB::open(dir.path()).unwrap();

        // CRC should be loaded
        let hash32 = Hash32::from_bytes(hash);
        assert!(db.checksums.contains_key(&hash32));

        // Read should return None because CRC mismatch indicates corruption
        let result = db.get_block(&hash32);
        assert!(result.is_none());
    }

    #[test]
    fn test_crc32_legacy_entries_no_checksum() {
        // Legacy entries (checksum=0) should not trigger CRC verification
        let dir = tempfile::tempdir().unwrap();
        let hash = [42u8; 32];

        // create_test_chunk writes entries with checksum=0 (legacy)
        create_test_chunk(dir.path(), 0, &[(b"block_data", hash, 100)]);

        let db = ImmutableDB::open(dir.path()).unwrap();

        // No checksum should be stored for legacy entries
        assert!(!db.checksums.contains_key(&Hash32::from_bytes(hash)));

        // Read should work without CRC verification
        let result = db.get_block(&Hash32::from_bytes(hash)).unwrap();
        assert_eq!(result, b"block_data");
    }

    #[test]
    fn test_crc32_valid_read_after_write_and_reopen() {
        // Full round-trip: write, flush, reopen, read with CRC verification
        let dir = tempfile::tempdir().unwrap();
        let blocks = vec![
            (Hash32::from_bytes([1u8; 32]), b"block_one".as_slice()),
            (Hash32::from_bytes([2u8; 32]), b"block_two".as_slice()),
            (Hash32::from_bytes([3u8; 32]), b"block_three".as_slice()),
        ];

        {
            let mut db = ImmutableDB::open_for_writing(dir.path()).unwrap();
            for (i, (hash, cbor)) in blocks.iter().enumerate() {
                db.append_block((i as u64 + 1) * 10, i as u64 + 1, hash, cbor)
                    .unwrap();
            }
            db.flush().unwrap();
        }

        // Re-open and verify all blocks pass CRC verification
        let db = ImmutableDB::open(dir.path()).unwrap();
        for (hash, cbor) in &blocks {
            let result = db.get_block(hash).unwrap();
            assert_eq!(result, *cbor);
            assert!(db.checksums.contains_key(hash));
        }
    }

    // -----------------------------------------------------------------------
    // Additional edge case tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_append_block_at_slot_zero() {
        // First block at slot 0 should work correctly
        let dir = tempfile::tempdir().unwrap();
        let mut db = ImmutableDB::open_for_writing(dir.path()).unwrap();

        let hash = Hash32::from_bytes([1u8; 32]);
        db.append_block(0, 0, &hash, b"genesis_block").unwrap();

        assert_eq!(db.total_blocks(), 1);
        assert_eq!(db.tip_slot(), 0);
        assert!(db.has_block(&hash));
        assert_eq!(db.get_block(&hash).unwrap(), b"genesis_block");
    }

    #[test]
    fn test_append_block_at_max_slot() {
        // Block at u64::MAX slot should work
        let dir = tempfile::tempdir().unwrap();
        let mut db = ImmutableDB::open_for_writing(dir.path()).unwrap();

        let hash = Hash32::from_bytes([1u8; 32]);
        db.append_block(u64::MAX, 1, &hash, b"far_future_block")
            .unwrap();

        assert_eq!(db.total_blocks(), 1);
        assert_eq!(db.tip_slot(), u64::MAX);
        assert!(db.has_block(&hash));
        assert_eq!(db.get_block(&hash).unwrap(), b"far_future_block");
    }

    #[test]
    fn test_secondary_index_survives_flush_and_reopen() {
        // Write blocks, flush, reopen and verify all data survives
        let dir = tempfile::tempdir().unwrap();
        let hashes: Vec<Hash32> = (1..=5u8).map(|i| Hash32::from_bytes([i; 32])).collect();

        {
            let mut db = ImmutableDB::open_for_writing(dir.path()).unwrap();
            for (i, hash) in hashes.iter().enumerate() {
                let cbor = format!("block_{}", i + 1);
                db.append_block((i as u64 + 1) * 100, i as u64 + 1, hash, cbor.as_bytes())
                    .unwrap();
            }
            db.flush().unwrap();
        }

        // Reopen and verify all blocks
        let db = ImmutableDB::open(dir.path()).unwrap();
        assert_eq!(db.total_blocks(), 5);
        assert_eq!(db.tip_slot(), 500);
        for (i, hash) in hashes.iter().enumerate() {
            assert!(db.has_block(hash));
            let cbor = db.get_block(hash).unwrap();
            assert_eq!(cbor, format!("block_{}", i + 1).as_bytes());
        }
    }

    #[test]
    fn test_secondary_index_missing_chunk_file_read() {
        // Create chunk + secondary, then delete chunk file.
        // has_block returns true (index exists) but get_block returns None (can't read).
        let dir = tempfile::tempdir().unwrap();
        let hash = [42u8; 32];
        create_test_chunk(dir.path(), 0, &[(b"block_data", hash, 100)]);

        // Delete the chunk file but keep secondary
        fs::remove_file(dir.path().join("00000.chunk")).unwrap();

        let db = ImmutableDB::open(dir.path()).unwrap();
        // Chunk file is gone so no blocks discovered (chunks found by .chunk files)
        assert_eq!(db.total_blocks(), 0);
    }

    #[test]
    fn test_crc32_mismatch_rejects_corrupted_block() {
        // Corrupt the block data on disk, verify get_block returns None
        // (CRC mismatch rejects the block to prevent propagation of corrupt data)
        let dir = tempfile::tempdir().unwrap();
        let hash = Hash32::from_bytes([42u8; 32]);
        let original = b"original_block_data_here";

        {
            let mut db = ImmutableDB::open_for_writing(dir.path()).unwrap();
            db.append_block(100, 1, &hash, original).unwrap();
            db.flush().unwrap();
        }

        // Corrupt the chunk file (overwrite with same-length different content)
        let chunk_path = dir.path().join("00000.chunk");
        let corrupted = b"CORRUPTED_block_data_hXX";
        assert_eq!(corrupted.len(), original.len());
        fs::write(&chunk_path, corrupted).unwrap();

        // Reopen - CRC mismatch should reject the corrupted block
        let db = ImmutableDB::open(dir.path()).unwrap();
        let result = db.get_block(&hash);
        assert!(result.is_none());
    }

    #[test]
    fn test_finalize_and_reopen() {
        // Write blocks to active chunk, finalize, write more, flush, reopen
        let dir = tempfile::tempdir().unwrap();
        let h1 = Hash32::from_bytes([1u8; 32]);
        let h2 = Hash32::from_bytes([2u8; 32]);
        let h3 = Hash32::from_bytes([3u8; 32]);

        {
            let mut db = ImmutableDB::open_for_writing(dir.path()).unwrap();
            db.append_block(10, 1, &h1, b"epoch0_block1").unwrap();
            db.append_block(20, 2, &h2, b"epoch0_block2").unwrap();
            db.finalize_chunk().unwrap();
            db.append_block(30, 3, &h3, b"epoch1_block1").unwrap();
            db.flush().unwrap();
        }

        // Reopen and verify all blocks across finalized + active chunks
        let db = ImmutableDB::open(dir.path()).unwrap();
        assert_eq!(db.total_blocks(), 3);
        assert!(db.has_block(&h1));
        assert!(db.has_block(&h2));
        assert!(db.has_block(&h3));
        assert_eq!(db.get_block(&h1).unwrap(), b"epoch0_block1");
        assert_eq!(db.get_block(&h3).unwrap(), b"epoch1_block1");
    }

    #[test]
    fn test_read_crc32_from_entry_helper() {
        // Short entry returns 0
        assert_eq!(read_crc32_from_entry(&[0u8; 10]), 0);

        // Entry with CRC at bytes 12..16
        let mut entry = [0u8; 56];
        entry[12..16].copy_from_slice(&42u32.to_be_bytes());
        assert_eq!(read_crc32_from_entry(&entry), 42);

        // All-zero CRC field
        assert_eq!(read_crc32_from_entry(&[0u8; 56]), 0);
    }
}
