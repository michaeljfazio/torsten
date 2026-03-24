//! In-memory volatile block storage with optional write-ahead log.
//!
//! Stores the last k blocks (and any forks) in memory. The optional WAL
//! persists volatile blocks to disk so they survive crashes — on restart
//! the WAL is replayed to rebuild the in-memory state.
//!
//! Use `VolatileDB::new()` for a pure in-memory store (backward compatible)
//! or `VolatileDB::open(path)` to enable crash recovery via WAL.
//!
//! # WAL Format
//!
//! Each entry in the write-ahead log uses the following 88-byte header:
//!
//! ```text
//! magic(4) + slot(8) + block_no(8) + hash(32) + prev_hash(32) + cbor_len(4)
//! = 88 bytes total, followed by cbor_len bytes of block CBOR
//! ```
//!
//! Fields are stored big-endian. Magic is the ASCII bytes `TWAL`.
//!
//! ## Legacy format (56 bytes)
//!
//! An older format without `prev_hash` was used before this version:
//!
//! ```text
//! magic(4) + slot(8) + block_no(8) + hash(32) + cbor_len(4)
//! = 56 bytes total
//! ```
//!
//! On open, legacy entries are detected by checking the byte at offset 56:
//! if bytes `[56..60]` equal the `TWAL` magic (meaning the next entry starts
//! immediately after a 56-byte header), or if the `cbor_len` field encoded at
//! offset 52 would produce a `cbor_end` that aligns with a `TWAL` magic at
//! that position, we treat the file as legacy. A one-time warning is logged
//! and `prev_hash` is set to `Hash32::ZERO` for all legacy entries.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use torsten_primitives::hash::Hash32;
use tracing::{debug, info, warn};

/// WAL entry magic bytes: "TWAL"
const WAL_MAGIC: [u8; 4] = *b"TWAL";

/// Size of the v2 WAL entry header (no trailing CRC).
///
/// Layout: magic(4) + slot(8) + block_no(8) + hash(32) + prev_hash(32) + cbor_len(4)
const WAL_HEADER_SIZE: usize = 4 + 8 + 8 + 32 + 32 + 4; // 88 bytes

/// Size of the CRC32 trailer appended after CBOR in v3 entries.
const WAL_CRC_SIZE: usize = 4;

/// Size of the legacy (v1) WAL entry header, which lacked prev_hash.
///
/// Layout: magic(4) + slot(8) + block_no(8) + hash(32) + cbor_len(4)
const WAL_HEADER_SIZE_LEGACY: usize = 4 + 8 + 8 + 32 + 4; // 56 bytes

/// WAL file name within the volatile directory.
const WAL_FILENAME: &str = "volatile-wal.bin";

/// Manages the write-ahead log file for crash recovery.
struct WalWriter {
    file: BufWriter<File>,
    path: PathBuf,
}

impl WalWriter {
    /// Open or create a WAL file at the given path in append mode.
    fn open(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(WalWriter {
            file: BufWriter::new(file),
            path: path.to_path_buf(),
        })
    }

    /// Append a WAL entry and sync to disk.
    ///
    /// Writes the 88-byte header followed by the block CBOR and a 4-byte
    /// CRC32 trailer, then flushes the BufWriter and issues a `sync_data()`
    /// to guarantee the entry is durable before the caller proceeds.
    ///
    /// The CRC32 covers the entire entry (header + CBOR) and is used during
    /// replay to detect partial or corrupted writes that passed the magic
    /// and length checks but have damaged payload data.
    fn append(
        &mut self,
        slot: u64,
        block_no: u64,
        hash: &Hash32,
        prev_hash: &Hash32,
        cbor: &[u8],
    ) -> io::Result<()> {
        // Cardano blocks are well under 4 GiB, but guard against accidental
        // silent truncation if the invariant is ever violated.
        let cbor_len = u32::try_from(cbor.len()).map_err(|_| {
            io::Error::other(format!(
                "WAL entry too large: {} bytes exceeds u32::MAX",
                cbor.len()
            ))
        })?;

        // Build the header for CRC computation
        let mut header = [0u8; WAL_HEADER_SIZE];
        header[0..4].copy_from_slice(&WAL_MAGIC);
        header[4..12].copy_from_slice(&slot.to_be_bytes());
        header[12..20].copy_from_slice(&block_no.to_be_bytes());
        header[20..52].copy_from_slice(hash.as_bytes());
        header[52..84].copy_from_slice(prev_hash.as_bytes());
        header[84..88].copy_from_slice(&cbor_len.to_be_bytes());

        // Compute CRC32 over header + CBOR
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(&header);
        hasher.update(cbor);
        let crc = hasher.finalize();

        self.file.write_all(&header)?;
        self.file.write_all(cbor)?;
        self.file.write_all(&crc.to_be_bytes())?;
        self.file.flush()?;
        self.file.get_ref().sync_data()?;
        Ok(())
    }

    /// Truncate the WAL file to zero bytes.
    fn truncate(&mut self) -> io::Result<()> {
        // Re-open the file in truncate mode
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&self.path)?;
        file.sync_data()?;
        // Re-open in append mode
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        self.file = BufWriter::new(file);
        Ok(())
    }

    /// Rewrite the WAL with only the given entries (used after flush or rollback).
    ///
    /// Writes to a temporary file and renames atomically so that a crash
    /// during rewrite cannot leave the WAL in a partially-written state.
    fn rewrite(
        &mut self,
        entries: &[(u64, u64, Hash32, Hash32, Vec<u8>)], // (slot, block_no, hash, prev_hash, cbor)
    ) -> io::Result<()> {
        // Write to a temp file, then rename for atomicity
        let tmp_path = self.path.with_extension("tmp");
        {
            let file = File::create(&tmp_path)?;
            let mut writer = BufWriter::new(file);
            for (slot, block_no, hash, prev_hash, cbor) in entries {
                // Guard against silent truncation of a block that somehow
                // exceeds 4 GiB (should be impossible, but treat as fatal).
                let cbor_len = u32::try_from(cbor.len()).map_err(|_| {
                    io::Error::other(format!(
                        "WAL rewrite: entry too large: {} bytes exceeds u32::MAX",
                        cbor.len()
                    ))
                })?;

                // Build header for CRC computation
                let mut header = [0u8; WAL_HEADER_SIZE];
                header[0..4].copy_from_slice(&WAL_MAGIC);
                header[4..12].copy_from_slice(&slot.to_be_bytes());
                header[12..20].copy_from_slice(&block_no.to_be_bytes());
                header[20..52].copy_from_slice(hash.as_bytes());
                header[52..84].copy_from_slice(prev_hash.as_bytes());
                header[84..88].copy_from_slice(&cbor_len.to_be_bytes());

                let mut hasher = crc32fast::Hasher::new();
                hasher.update(&header);
                hasher.update(cbor);
                let crc = hasher.finalize();

                writer.write_all(&header)?;
                writer.write_all(cbor)?;
                writer.write_all(&crc.to_be_bytes())?;
            }
            writer.flush()?;
            writer.get_ref().sync_data()?;
        }
        fs::rename(&tmp_path, &self.path)?;
        // Fsync the parent directory to ensure the rename is durable.
        // Without this, a crash could leave the directory entry pointing
        // at the old file.
        if let Some(parent) = self.path.parent() {
            if let Ok(dir) = File::open(parent) {
                let _ = dir.sync_all();
            }
        }
        // Re-open in append mode
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        self.file = BufWriter::new(file);
        Ok(())
    }
}

/// Parsed WAL entry for replay.
struct WalEntry {
    slot: u64,
    block_no: u64,
    hash: Hash32,
    /// The hash of the parent block. Set to `Hash32::ZERO` when recovered
    /// from a legacy 56-byte WAL entry that predates this field.
    prev_hash: Hash32,
    cbor: Vec<u8>,
}

/// Detect whether a WAL file uses the legacy 56-byte header format.
///
/// Strategy: scan forward from the start treating the data as 56-byte
/// headers. If, after consuming `WAL_HEADER_SIZE_LEGACY` bytes plus the
/// CBOR payload, we land on another `TWAL` magic (or EOF), the file is
/// legacy. If the scan fails to align (i.e. the implied cbor_len for a
/// 56-byte header would produce a position that does NOT start with `TWAL`
/// and is not EOF), we conclude the file is the new 88-byte format.
///
/// An empty file returns `false` (new format, no-op either way).
fn detect_legacy_format(data: &[u8]) -> bool {
    if data.len() < WAL_HEADER_SIZE_LEGACY {
        // Too short to contain even one legacy entry; treat as new.
        return false;
    }

    // The first 4 bytes must be TWAL for either format.
    if data[..4] != WAL_MAGIC {
        return false;
    }

    // Read cbor_len from the legacy header position (offset 52).
    let cbor_len = u32::from_be_bytes(data[52..56].try_into().unwrap()) as usize;

    // Where would the next entry begin under the 56-byte assumption?
    let next_pos = WAL_HEADER_SIZE_LEGACY + cbor_len;

    if next_pos > data.len() {
        // Not enough data — could be a truncated write in either format.
        // Default to new format; the truncated header will be discarded by
        // the normal replay loop regardless.
        return false;
    }

    if next_pos == data.len() {
        // Exactly one entry that consumes the entire file — ambiguous.
        // Check whether the u32 at offset 84 (new-format cbor_len) would
        // also land at EOF. If both land at EOF the entry is ambiguous but
        // since the file is too short to confirm a 88-byte header we
        // conservatively return false (new format).
        if data.len() < WAL_HEADER_SIZE {
            return true; // Can't fit 88-byte header; must be legacy.
        }
        return false;
    }

    // There is data beyond the first entry. Under legacy assumption the next
    // entry must begin with TWAL.
    if next_pos + 4 <= data.len() && data[next_pos..next_pos + 4] == WAL_MAGIC {
        return true;
    }

    // The next position does not start with TWAL under the legacy assumption.
    // Check if it does under the new-format assumption (v2 or v3).
    if data.len() >= WAL_HEADER_SIZE {
        let new_cbor_len = u32::from_be_bytes(data[84..88].try_into().unwrap()) as usize;
        // v2: header + cbor (no CRC)
        let new_next_v2 = WAL_HEADER_SIZE + new_cbor_len;
        if new_next_v2 <= data.len()
            && (new_next_v2 == data.len()
                || (new_next_v2 + 4 <= data.len()
                    && data[new_next_v2..new_next_v2 + 4] == WAL_MAGIC))
        {
            return false; // v2 format aligns correctly.
        }
        // v3: header + cbor + CRC trailer
        let new_next_v3 = WAL_HEADER_SIZE + new_cbor_len + WAL_CRC_SIZE;
        if new_next_v3 <= data.len()
            && (new_next_v3 == data.len()
                || (new_next_v3 + 4 <= data.len()
                    && data[new_next_v3..new_next_v3 + 4] == WAL_MAGIC))
        {
            return false; // v3 format aligns correctly.
        }
    }

    // Neither format aligns cleanly on the first entry. Fall back to
    // checking the magic at offset 56: if it looks like the old
    // cbor_len field was zero the entry ends at byte 56 exactly, and
    // byte 56 starts with TWAL → legacy.
    false
}

/// Detect whether a WAL file uses the v3 format (with CRC32 trailers).
///
/// Strategy: parse the first entry using 88-byte headers, then check if
/// the 4 bytes after the CBOR contain a valid CRC32 of (header + cbor).
/// Legacy (56-byte) files are never v3.
fn detect_crc_format(data: &[u8]) -> bool {
    if data.len() < WAL_HEADER_SIZE {
        return false;
    }
    if data[..4] != WAL_MAGIC {
        return false;
    }
    let cbor_len = u32::from_be_bytes(data[84..88].try_into().unwrap()) as usize;
    let cbor_end = WAL_HEADER_SIZE + cbor_len;
    if cbor_end + WAL_CRC_SIZE > data.len() {
        return false;
    }
    let stored_crc =
        u32::from_be_bytes(data[cbor_end..cbor_end + WAL_CRC_SIZE].try_into().unwrap());
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&data[..cbor_end]);
    let computed_crc = hasher.finalize();
    stored_crc == computed_crc
}

/// Replay WAL entries from the given file path.
///
/// Validates magic bytes for each entry and stops on corrupted/truncated
/// data. Automatically detects legacy 56-byte format and v3 CRC format,
/// logging a one-time warning when upgrading from legacy.
fn replay_wal(path: &Path) -> io::Result<Vec<WalEntry>> {
    let mut entries = Vec::new();

    let data = match fs::read(path) {
        Ok(d) => d,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(entries),
        Err(e) => return Err(e),
    };

    if data.is_empty() {
        return Ok(entries);
    }

    let legacy = detect_legacy_format(&data);
    if legacy {
        warn!(
            "WAL: detected legacy 56-byte format (pre-prev_hash). \
             Entries will be recovered with prev_hash=ZERO. \
             The WAL will be upgraded to 88-byte format on the next rewrite."
        );
    }

    // Detect v3 CRC format (only applicable to non-legacy files).
    let has_crc = !legacy && detect_crc_format(&data);

    let header_size = if legacy {
        WAL_HEADER_SIZE_LEGACY
    } else {
        WAL_HEADER_SIZE
    };

    let mut pos = 0;
    while pos < data.len() {
        // Check if we have enough bytes for the header
        if pos + header_size > data.len() {
            warn!(
                pos,
                remaining = data.len() - pos,
                "WAL: truncated entry header, stopping replay"
            );
            break;
        }

        // Validate magic
        if data[pos..pos + 4] != WAL_MAGIC {
            warn!(pos, "WAL: invalid magic bytes, stopping replay");
            break;
        }

        let slot = u64::from_be_bytes(data[pos + 4..pos + 12].try_into().unwrap());
        let block_no = u64::from_be_bytes(data[pos + 12..pos + 20].try_into().unwrap());
        let mut hash_bytes = [0u8; 32];
        hash_bytes.copy_from_slice(&data[pos + 20..pos + 52]);

        let (prev_hash, cbor_len) = if legacy {
            // Legacy layout: cbor_len at offset 52, no prev_hash field.
            let cl = u32::from_be_bytes(data[pos + 52..pos + 56].try_into().unwrap()) as usize;
            (Hash32::ZERO, cl)
        } else {
            // New layout: prev_hash at offset 52, cbor_len at offset 84.
            let mut prev_bytes = [0u8; 32];
            prev_bytes.copy_from_slice(&data[pos + 52..pos + 84]);
            let cl = u32::from_be_bytes(data[pos + 84..pos + 88].try_into().unwrap()) as usize;
            (Hash32::from_bytes(prev_bytes), cl)
        };

        let cbor_start = pos + header_size;
        let cbor_end = cbor_start + cbor_len;

        // Check if we have enough bytes for the CBOR data
        if cbor_end > data.len() {
            warn!(
                pos,
                cbor_len,
                available = data.len() - cbor_start,
                "WAL: truncated CBOR data, stopping replay"
            );
            break;
        }

        // For v3 (CRC) format, validate the trailing CRC32 checksum.
        if has_crc {
            if cbor_end + WAL_CRC_SIZE > data.len() {
                warn!(pos, "WAL: truncated CRC32 trailer, stopping replay");
                break;
            }
            let stored_crc =
                u32::from_be_bytes(data[cbor_end..cbor_end + WAL_CRC_SIZE].try_into().unwrap());
            let mut hasher = crc32fast::Hasher::new();
            hasher.update(&data[pos..cbor_end]);
            let computed_crc = hasher.finalize();
            if stored_crc != computed_crc {
                warn!(
                    pos,
                    stored_crc,
                    computed_crc,
                    "WAL: CRC32 mismatch, entry corrupted — stopping replay"
                );
                break;
            }
            pos = cbor_end + WAL_CRC_SIZE;
        } else {
            pos = cbor_end;
        }

        entries.push(WalEntry {
            slot,
            block_no,
            hash: Hash32::from_bytes(hash_bytes),
            prev_hash,
            cbor: data[cbor_start..cbor_end].to_vec(),
        });
    }

    Ok(entries)
}

/// A block stored in the volatile DB.
#[derive(Debug, Clone)]
pub struct VolatileBlock {
    pub slot: u64,
    pub block_no: u64,
    pub prev_hash: Hash32,
    pub cbor: Vec<u8>,
}

/// The default delay before orphaned fork blocks are garbage-collected.
/// Matches Haskell cardano-node's 60-second GC delay for VolatileDB.
const GC_DELAY: Duration = Duration::from_secs(60);

/// Plan for switching from one chain to another in the VolatileDB.
///
/// Computed by `switch_chain()` — describes which blocks to roll back and
/// which to apply when atomically swapping the selected chain.
#[derive(Debug, Clone)]
pub struct SwitchPlan {
    /// The common ancestor of the old and new chains.
    pub intersection: Hash32,
    /// Blocks to roll back from the old chain (most-recent first).
    pub rollback: Vec<Hash32>,
    /// Blocks to apply from the new chain (oldest first).
    pub apply: Vec<Hash32>,
}

/// In-memory store for recent (volatile) blocks.
///
/// Stores blocks from all forks — blocks are never deleted on rollback.
/// The `selected_chain` tracks which chain is currently active. Fork
/// switching updates `selected_chain` without removing the old chain's
/// blocks. Orphaned blocks are garbage-collected after a delay.
///
/// This matches Haskell cardano-node's VolatileDB architecture where
/// blocks from all forks coexist and chain selection is tracked separately.
pub struct VolatileDB {
    /// All blocks from all forks, indexed by hash.
    blocks: HashMap<Hash32, VolatileBlock>,
    /// Slot-based index for all blocks (all forks).
    slot_index: BTreeMap<u64, Vec<Hash32>>,
    /// Block-number index for the SELECTED chain only.
    block_no_index: BTreeMap<u64, Hash32>,
    /// Successor relationships for all blocks (all forks).
    successors: HashMap<Hash32, Vec<Hash32>>,
    /// Tip of the selected chain: (slot, hash, block_no).
    tip: Option<(u64, Hash32, u64)>,
    wal: Option<WalWriter>,
    /// Currently-selected chain fragment, ordered oldest to newest.
    selected_chain: Vec<Hash32>,
    /// Blocks scheduled for garbage collection (orphaned fork blocks).
    gc_schedule: HashMap<Hash32, Instant>,
}

impl VolatileDB {
    /// Create a new in-memory-only VolatileDB (no WAL, no crash recovery).
    pub fn new() -> Self {
        VolatileDB {
            blocks: HashMap::new(),
            slot_index: BTreeMap::new(),
            block_no_index: BTreeMap::new(),
            successors: HashMap::new(),
            tip: None,
            wal: None,
            selected_chain: Vec::new(),
            gc_schedule: HashMap::new(),
        }
    }

    /// Open a VolatileDB with WAL-based crash recovery.
    ///
    /// Opens/creates the WAL file at `path/volatile-wal.bin` and replays
    /// any existing entries to rebuild the in-memory state. Legacy 56-byte
    /// WAL files are automatically detected and replayed with
    /// `prev_hash = Hash32::ZERO`; the successors map will be repopulated
    /// correctly as new blocks arrive.
    pub fn open(path: &Path) -> io::Result<Self> {
        fs::create_dir_all(path)?;
        let wal_path = path.join(WAL_FILENAME);

        // Replay existing WAL entries
        let entries = replay_wal(&wal_path)?;
        let replayed = entries.len();

        let mut db = VolatileDB {
            blocks: HashMap::new(),
            slot_index: BTreeMap::new(),
            block_no_index: BTreeMap::new(),
            successors: HashMap::new(),
            tip: None,
            wal: None,
            selected_chain: Vec::new(),
            gc_schedule: HashMap::new(),
        };

        // Rebuild in-memory state from WAL, using the recovered prev_hash.
        // For legacy entries prev_hash is Hash32::ZERO; the successors map
        // will still be consistent with itself (all point to ZERO parent)
        // and will be corrected over time as blocks are re-added via
        // normal sync after the first node startup post-upgrade.
        for entry in entries {
            db.insert_block_internal(
                entry.hash,
                entry.slot,
                entry.block_no,
                entry.prev_hash,
                entry.cbor,
            );
        }

        // Rebuild selected chain from replayed blocks
        if !db.blocks.is_empty() {
            db.rebuild_selected_chain();
        }

        // Open WAL writer for new entries
        let wal = WalWriter::open(&wal_path)?;
        db.wal = Some(wal);

        if replayed > 0 {
            debug!(
                replayed,
                selected_chain_len = db.selected_chain.len(),
                "VolatileDB: replayed WAL entries, rebuilt selected chain"
            );
        }

        Ok(db)
    }

    /// Add a block to the volatile store.
    ///
    /// If WAL is enabled, the block is appended to the WAL before being
    /// inserted into the in-memory state. The `prev_hash` is persisted in
    /// the WAL so that the successors map can be reconstructed accurately
    /// after a crash.
    pub fn add_block(
        &mut self,
        hash: Hash32,
        slot: u64,
        block_no: u64,
        prev_hash: Hash32,
        cbor: Vec<u8>,
    ) {
        // Write to WAL first (if enabled) so that prev_hash is durable
        // before the in-memory state is updated.
        if let Some(ref mut wal) = self.wal {
            if let Err(e) = wal.append(slot, block_no, &hash, &prev_hash, &cbor) {
                warn!(error = %e, "WAL: failed to append entry");
            }
        }

        self.insert_block_internal(hash, slot, block_no, prev_hash, cbor);
    }

    /// Internal block insertion (no WAL write).
    ///
    /// Stores the block in all indexes. If it extends the selected chain
    /// (prev_hash matches selected chain tip), also updates `selected_chain`,
    /// `block_no_index`, and `tip`. Otherwise stored as a fork block.
    fn insert_block_internal(
        &mut self,
        hash: Hash32,
        slot: u64,
        block_no: u64,
        prev_hash: Hash32,
        cbor: Vec<u8>,
    ) {
        // Track successor relationship (all forks)
        self.successors.entry(prev_hash).or_default().push(hash);
        // Slot index (all forks)
        self.slot_index.entry(slot).or_default().push(hash);

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

        // Extend selected chain if this block connects to it
        let extends = match self.selected_chain.last() {
            Some(tip_hash) => prev_hash == *tip_hash,
            None => true,
        };
        if extends {
            self.selected_chain.push(hash);
            self.block_no_index.insert(block_no, hash);
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

    /// Get the first block on the selected chain strictly after a given slot.
    ///
    /// Only blocks that belong to the current `selected_chain` are considered.
    /// This prevents returning fork/orphan blocks that are still in `slot_index`
    /// but pending garbage collection after a rollback.
    pub fn get_next_block_after_slot(&self, after_slot: u64) -> Option<(u64, Hash32, &[u8])> {
        // Build a set of hashes on the selected chain for O(1) lookup.
        let on_chain: HashSet<&Hash32> = self.selected_chain.iter().collect();
        for (&slot, hashes) in self.slot_index.range((after_slot + 1)..) {
            for hash in hashes {
                if on_chain.contains(hash) {
                    if let Some(block) = self.blocks.get(hash) {
                        return Some((slot, *hash, &block.cbor));
                    }
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

    /// Remove a specific block from all indexes.
    pub fn remove_block(&mut self, hash: &Hash32) {
        if let Some(block) = self.blocks.remove(hash) {
            if let Some(hashes) = self.slot_index.get_mut(&block.slot) {
                hashes.retain(|h| h != hash);
                if hashes.is_empty() {
                    self.slot_index.remove(&block.slot);
                }
            }
            if self.block_no_index.get(&block.block_no) == Some(hash) {
                self.block_no_index.remove(&block.block_no);
            }
            if let Some(succs) = self.successors.get_mut(&block.prev_hash) {
                succs.retain(|h| h != hash);
                if succs.is_empty() {
                    self.successors.remove(&block.prev_hash);
                }
            }
            self.selected_chain.retain(|h| h != hash);
            self.gc_schedule.remove(hash);
        }
    }

    /// Remove all blocks at or below a given slot.
    /// Returns the hashes of removed blocks.
    ///
    /// If WAL is enabled, rewrites the WAL with only the remaining entries.
    /// The rewritten WAL uses the current 88-byte format, so this operation
    /// also upgrades any legacy 56-byte WAL to the new format.
    pub fn remove_blocks_up_to_slot(&mut self, slot: u64) -> Vec<Hash32> {
        let slots_to_remove: Vec<u64> = self.slot_index.range(..=slot).map(|(&s, _)| s).collect();

        let mut removed_set = HashSet::new();
        let mut removed = Vec::new();
        for s in slots_to_remove {
            if let Some(hashes) = self.slot_index.remove(&s) {
                for hash in hashes {
                    if let Some(block) = self.blocks.remove(&hash) {
                        if self.block_no_index.get(&block.block_no) == Some(&hash) {
                            self.block_no_index.remove(&block.block_no);
                        }
                        if let Some(succs) = self.successors.get_mut(&block.prev_hash) {
                            succs.retain(|h| *h != hash);
                            if succs.is_empty() {
                                self.successors.remove(&block.prev_hash);
                            }
                        }
                    }
                    self.gc_schedule.remove(&hash);
                    removed_set.insert(hash);
                    removed.push(hash);
                }
            }
        }

        // Trim flushed blocks from selected_chain front
        self.selected_chain.retain(|h| !removed_set.contains(h));

        // Rewrite WAL with remaining entries
        if let Some(ref mut wal) = self.wal {
            let remaining: Vec<(u64, u64, Hash32, Hash32, Vec<u8>)> = self
                .blocks
                .iter()
                .map(|(h, b)| (b.slot, b.block_no, *h, b.prev_hash, b.cbor.clone()))
                .collect();
            if let Err(e) = wal.rewrite(&remaining) {
                warn!(error = %e, "WAL: failed to rewrite after flush");
            }
        }

        removed
    }

    /// Non-destructive rollback: truncate the selected chain to a given point.
    ///
    /// Blocks from the old chain suffix remain in VolatileDB and are scheduled
    /// for delayed GC. This matches Haskell cardano-node behavior.
    ///
    /// Returns hashes removed from the selected chain (most recent first).
    pub fn rollback_to_point(
        &mut self,
        target_slot: u64,
        target_hash: Option<&Hash32>,
    ) -> Vec<Hash32> {
        // Find the cut point in selected_chain
        let cut_point = self
            .selected_chain
            .iter()
            .position(|h| {
                if let Some(block) = self.blocks.get(h) {
                    if block.slot == target_slot {
                        return target_hash.is_none_or(|th| h == th);
                    }
                }
                false
            })
            .or_else(|| {
                // Fallback: last block at or before target_slot
                self.selected_chain
                    .iter()
                    .rposition(|h| self.blocks.get(h).is_some_and(|b| b.slot <= target_slot))
            });

        let rolled_back = match cut_point {
            Some(idx) => {
                let suffix: Vec<Hash32> = self.selected_chain.drain((idx + 1)..).rev().collect();
                let now = Instant::now();
                for h in &suffix {
                    self.gc_schedule.insert(*h, now);
                    if let Some(block) = self.blocks.get(h) {
                        if self.block_no_index.get(&block.block_no) == Some(h) {
                            self.block_no_index.remove(&block.block_no);
                        }
                    }
                }
                if let Some(tip_hash) = self.selected_chain.last() {
                    if let Some(block) = self.blocks.get(tip_hash) {
                        self.tip = Some((block.slot, *tip_hash, block.block_no));
                    }
                } else {
                    self.tip = None;
                }
                suffix
            }
            None => {
                let all: Vec<Hash32> = self.selected_chain.drain(..).rev().collect();
                let now = Instant::now();
                for h in &all {
                    self.gc_schedule.insert(*h, now);
                    if let Some(block) = self.blocks.get(h) {
                        if self.block_no_index.get(&block.block_no) == Some(h) {
                            self.block_no_index.remove(&block.block_no);
                        }
                    }
                }
                self.tip = None;
                all
            }
        };

        if !rolled_back.is_empty() {
            debug!(
                rolled_back = rolled_back.len(),
                remaining = self.selected_chain.len(),
                total_blocks = self.blocks.len(),
                "VolatileDB: non-destructive rollback (fork blocks retained)"
            );
        }

        rolled_back
    }

    /// Destructive rollback: remove all blocks after a given point entirely.
    /// Used only for catastrophic scenarios (all peers Origin, deep divergence).
    pub fn rollback_and_prune(
        &mut self,
        target_slot: u64,
        target_hash: Option<&Hash32>,
    ) -> Vec<Hash32> {
        let mut removed = Vec::new();
        let slots_to_remove: Vec<u64> = self
            .slot_index
            .range((target_slot + 1)..)
            .map(|(&s, _)| s)
            .collect();
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
        for s in slots_to_remove.into_iter().rev() {
            if let Some(hashes) = self.slot_index.remove(&s) {
                for hash in hashes {
                    if let Some(block) = self.blocks.remove(&hash) {
                        if self.block_no_index.get(&block.block_no) == Some(&hash) {
                            self.block_no_index.remove(&block.block_no);
                        }
                        if let Some(succs) = self.successors.get_mut(&block.prev_hash) {
                            succs.retain(|h| *h != hash);
                        }
                    }
                    self.gc_schedule.remove(&hash);
                    removed.push(hash);
                }
            }
        }
        for hash in at_target {
            self.remove_block(&hash);
            removed.push(hash);
        }
        self.rebuild_selected_chain();
        if let Some(ref mut wal) = self.wal {
            let remaining: Vec<(u64, u64, Hash32, Hash32, Vec<u8>)> = self
                .blocks
                .iter()
                .map(|(h, b)| (b.slot, b.block_no, *h, b.prev_hash, b.cbor.clone()))
                .collect();
            if let Err(e) = wal.rewrite(&remaining) {
                warn!(error = %e, "WAL: failed to rewrite after destructive rollback");
            }
        }
        removed
    }

    /// Clear all blocks.
    ///
    /// If WAL is enabled, truncates the WAL file to zero.
    pub fn clear(&mut self) {
        self.blocks.clear();
        self.slot_index.clear();
        self.block_no_index.clear();
        self.successors.clear();
        self.selected_chain.clear();
        self.gc_schedule.clear();
        self.tip = None;

        if let Some(ref mut wal) = self.wal {
            if let Err(e) = wal.truncate() {
                warn!(error = %e, "WAL: failed to truncate on clear");
            }
        }
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

    /// Return `(hash, slot, block_no, prev_hash)` tuples for every block on the
    /// current selected chain, ordered oldest → newest.
    ///
    /// Used by `ChainDB::get_volatile_chain_headers` to build the initial
    /// `ChainFragment` at node startup without decoding any CBOR.
    pub fn selected_chain_entries(&self) -> Vec<(Hash32, u64, u64, Hash32)> {
        self.selected_chain
            .iter()
            .filter_map(|hash| {
                self.blocks
                    .get(hash)
                    .map(|blk| (*hash, blk.slot, blk.block_no, blk.prev_hash))
            })
            .collect()
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

    /// Recompute tip from the selected chain.
    fn recompute_tip(&mut self) {
        self.tip = self
            .selected_chain
            .last()
            .and_then(|h| self.blocks.get(h).map(|b| (b.slot, *h, b.block_no)));
    }

    /// Walk backwards from `tip_hash` through `prev_hash` links.
    /// Returns oldest-to-newest order.
    pub fn walk_chain_back(&self, tip_hash: &Hash32) -> Vec<Hash32> {
        let mut chain = Vec::new();
        let mut current = *tip_hash;
        while let Some(block) = self.blocks.get(&current) {
            chain.push(current);
            if self.blocks.contains_key(&block.prev_hash) {
                current = block.prev_hash;
            } else {
                break;
            }
        }
        chain.reverse();
        chain
    }

    /// Find all leaf tips: blocks with no successors.
    pub fn get_leaf_tips(&self) -> Vec<(Hash32, u64, u64)> {
        self.blocks
            .iter()
            .filter(|(hash, _)| {
                self.successors
                    .get(*hash)
                    .is_none_or(|succs| succs.is_empty())
            })
            .map(|(hash, block)| (*hash, block.slot, block.block_no))
            .collect()
    }

    /// Switch the selected chain to end at `new_tip_hash`.
    /// Returns a `SwitchPlan` with blocks to rollback/apply.
    pub fn switch_chain(&mut self, new_tip_hash: &Hash32) -> Option<SwitchPlan> {
        let new_chain = self.walk_chain_back(new_tip_hash);
        if new_chain.is_empty() {
            return None;
        }
        let new_chain_set: HashSet<&Hash32> = new_chain.iter().collect();
        let intersection_idx = self
            .selected_chain
            .iter()
            .rposition(|h| new_chain_set.contains(h));

        let (intersection, rollback, apply) = match intersection_idx {
            Some(idx) => {
                let intersection_hash = self.selected_chain[idx];
                let rollback: Vec<Hash32> = self.selected_chain[(idx + 1)..]
                    .iter()
                    .rev()
                    .copied()
                    .collect();
                let intersection_pos_in_new =
                    new_chain.iter().position(|h| *h == intersection_hash);
                let apply: Vec<Hash32> = match intersection_pos_in_new {
                    Some(pos) => new_chain[(pos + 1)..].to_vec(),
                    None => new_chain.clone(),
                };
                (intersection_hash, rollback, apply)
            }
            None => {
                let rollback: Vec<Hash32> = self.selected_chain.iter().rev().copied().collect();
                let anchor = self
                    .blocks
                    .get(&new_chain[0])
                    .map(|b| b.prev_hash)
                    .unwrap_or(new_chain[0]);
                (anchor, rollback, new_chain.clone())
            }
        };

        let now = Instant::now();
        for h in &rollback {
            self.gc_schedule.insert(*h, now);
        }
        for h in &apply {
            self.gc_schedule.remove(h);
        }

        if let Some(idx) = intersection_idx {
            self.selected_chain.truncate(idx + 1);
        } else {
            self.selected_chain.clear();
        }
        self.selected_chain.extend_from_slice(&apply);

        self.block_no_index.clear();
        for h in &self.selected_chain {
            if let Some(block) = self.blocks.get(h) {
                self.block_no_index.insert(block.block_no, *h);
            }
        }
        self.recompute_tip();

        info!(
            rollback_count = rollback.len(),
            apply_count = apply.len(),
            selected_len = self.selected_chain.len(),
            "VolatileDB: chain switch"
        );

        Some(SwitchPlan {
            intersection,
            rollback,
            apply,
        })
    }

    /// Rebuild selected chain by finding the longest chain from all tips.
    fn rebuild_selected_chain(&mut self) {
        let tips = self.get_leaf_tips();
        let mut best_chain: Vec<Hash32> = Vec::new();
        let mut best_block_no: u64 = 0;
        for (tip_hash, _, tip_block_no) in &tips {
            if *tip_block_no >= best_block_no {
                let chain = self.walk_chain_back(tip_hash);
                if chain.len() > best_chain.len()
                    || (chain.len() == best_chain.len() && *tip_block_no > best_block_no)
                {
                    best_chain = chain;
                    best_block_no = *tip_block_no;
                }
            }
        }
        self.selected_chain = best_chain;
        self.block_no_index.clear();
        for h in &self.selected_chain {
            if let Some(block) = self.blocks.get(h) {
                self.block_no_index.insert(block.block_no, *h);
            }
        }
        self.recompute_tip();
    }

    /// GC orphaned fork blocks whose delay has expired. Returns count removed.
    pub fn gc_orphaned_blocks(&mut self) -> usize {
        let now = Instant::now();
        let expired: Vec<Hash32> = self
            .gc_schedule
            .iter()
            .filter(|(_, at)| now.duration_since(**at) >= GC_DELAY)
            .map(|(hash, _)| *hash)
            .collect();
        let count = expired.len();
        for hash in &expired {
            self.gc_schedule.remove(hash);
            if !self.selected_chain.contains(hash) {
                if let Some(block) = self.blocks.remove(hash) {
                    if let Some(hashes) = self.slot_index.get_mut(&block.slot) {
                        hashes.retain(|h| h != hash);
                        if hashes.is_empty() {
                            self.slot_index.remove(&block.slot);
                        }
                    }
                    if let Some(succs) = self.successors.get_mut(&block.prev_hash) {
                        succs.retain(|h| h != hash);
                        if succs.is_empty() {
                            self.successors.remove(&block.prev_hash);
                        }
                    }
                }
            }
        }
        if count > 0 {
            debug!(gc_removed = count, "VolatileDB: GC orphaned blocks");
            if let Some(ref mut wal) = self.wal {
                let remaining: Vec<(u64, u64, Hash32, Hash32, Vec<u8>)> = self
                    .blocks
                    .iter()
                    .map(|(h, b)| (b.slot, b.block_no, *h, b.prev_hash, b.cbor.clone()))
                    .collect();
                if let Err(e) = wal.rewrite(&remaining) {
                    warn!(error = %e, "WAL: failed to rewrite after GC");
                }
            }
        }
        count
    }

    /// Get the selected chain length.
    pub fn selected_chain_len(&self) -> usize {
        self.selected_chain.len()
    }

    /// Get the number of orphaned blocks pending GC.
    pub fn gc_pending_count(&self) -> usize {
        self.gc_schedule.len()
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

    // -----------------------------------------------------------------------
    // Core in-memory behaviour
    // -----------------------------------------------------------------------

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
    fn test_rollback_truncates_selected_chain() {
        let mut db = VolatileDB::new();
        db.add_block(h(1), 100, 10, h(0), b"b1".to_vec());
        db.add_block(h(2), 200, 20, h(1), b"b2".to_vec());
        db.add_block(h(3), 300, 30, h(2), b"b3".to_vec());

        let removed = db.rollback_to_point(100, Some(&h(1)));
        assert_eq!(removed.len(), 2);
        assert!(db.has_block(&h(1)));
        assert!(db.has_block(&h(2))); // Non-destructive: still in store
        assert!(db.has_block(&h(3))); // Non-destructive: still in store
        assert_eq!(db.selected_chain_len(), 1);
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

    // -----------------------------------------------------------------------
    // WAL tests — new 88-byte format
    // -----------------------------------------------------------------------

    #[test]
    fn test_wal_creation_and_replay() {
        let dir = tempfile::tempdir().unwrap();
        let wal_dir = dir.path().join("volatile");

        // Create DB with WAL and add blocks
        {
            let mut db = VolatileDB::open(&wal_dir).unwrap();
            db.add_block(h(1), 100, 10, h(0), b"block1".to_vec());
            db.add_block(h(2), 200, 20, h(1), b"block2".to_vec());
            db.add_block(h(3), 300, 30, h(2), b"block3".to_vec());

            assert_eq!(db.len(), 3);
            assert!(db.has_block(&h(1)));
            assert!(db.has_block(&h(2)));
            assert!(db.has_block(&h(3)));
        }

        // Re-open: should replay WAL and recover all blocks
        {
            let db = VolatileDB::open(&wal_dir).unwrap();
            assert_eq!(db.len(), 3);
            assert!(db.has_block(&h(1)));
            assert!(db.has_block(&h(2)));
            assert!(db.has_block(&h(3)));
            assert_eq!(db.get_block_cbor(&h(1)).unwrap(), b"block1");
            assert_eq!(db.get_block_cbor(&h(2)).unwrap(), b"block2");
            assert_eq!(db.get_block_cbor(&h(3)).unwrap(), b"block3");
            assert_eq!(db.get_tip(), Some((300, h(3), 30)));
        }
    }

    #[test]
    fn test_wal_prev_hash_survives_crash() {
        // Verify that prev_hash is correctly recovered after a simulated crash
        // (process exit without clean shutdown — WAL not rewritten).
        let dir = tempfile::tempdir().unwrap();
        let wal_dir = dir.path().join("volatile");

        {
            let mut db = VolatileDB::open(&wal_dir).unwrap();
            // Chain: genesis(h0) -> b1(h1) -> b2(h2) -> b3(h3)
            db.add_block(h(1), 100, 10, h(0), b"block1".to_vec());
            db.add_block(h(2), 200, 20, h(1), b"block2".to_vec());
            db.add_block(h(3), 300, 30, h(2), b"block3".to_vec());
            // Drop without calling clear() — simulates a crash.
        }

        // Re-open and check prev_hash was recovered for each block.
        {
            let db = VolatileDB::open(&wal_dir).unwrap();
            assert_eq!(db.len(), 3);

            let b1 = db.get_block(&h(1)).unwrap();
            assert_eq!(b1.prev_hash, h(0), "b1 prev_hash should be h(0)");

            let b2 = db.get_block(&h(2)).unwrap();
            assert_eq!(b2.prev_hash, h(1), "b2 prev_hash should be h(1)");

            let b3 = db.get_block(&h(3)).unwrap();
            assert_eq!(b3.prev_hash, h(2), "b3 prev_hash should be h(2)");
        }
    }

    #[test]
    fn test_wal_successors_map_after_recovery() {
        // After crash-recovery the successors map must reflect the
        // recovered prev_hash values so fork detection works correctly.
        let dir = tempfile::tempdir().unwrap();
        let wal_dir = dir.path().join("volatile");

        {
            let mut db = VolatileDB::open(&wal_dir).unwrap();
            // Two forks from h(1): h(2a) and h(2b)
            db.add_block(h(1), 100, 10, h(0), b"b1".to_vec());
            db.add_block(h(2), 200, 20, h(1), b"b2a".to_vec());
            db.add_block(h(3), 200, 20, h(1), b"b2b_fork".to_vec());
            // Drop without shutdown — simulates crash.
        }

        {
            let db = VolatileDB::open(&wal_dir).unwrap();
            assert_eq!(db.len(), 3);

            // Both fork blocks report h(1) as their parent.
            assert_eq!(db.get_block(&h(2)).unwrap().prev_hash, h(1));
            assert_eq!(db.get_block(&h(3)).unwrap().prev_hash, h(1));

            // The successors map for h(1) should contain both fork hashes.
            let succs = db.successors.get(&h(1)).cloned().unwrap_or_default();
            assert!(
                succs.contains(&h(2)),
                "successors of h(1) should include h(2)"
            );
            assert!(
                succs.contains(&h(3)),
                "successors of h(1) should include h(3)"
            );
        }
    }

    #[test]
    fn test_wal_truncation_on_flush() {
        let dir = tempfile::tempdir().unwrap();
        let wal_dir = dir.path().join("volatile");

        // Create DB, add blocks, then remove some (simulating flush)
        {
            let mut db = VolatileDB::open(&wal_dir).unwrap();
            db.add_block(h(1), 100, 10, h(0), b"b1".to_vec());
            db.add_block(h(2), 200, 20, h(1), b"b2".to_vec());
            db.add_block(h(3), 300, 30, h(2), b"b3".to_vec());

            // Simulate flush: remove blocks up to slot 200
            db.remove_blocks_up_to_slot(200);
            assert_eq!(db.len(), 1);
            assert!(db.has_block(&h(3)));
        }

        // Re-open: WAL should only contain block 3
        {
            let db = VolatileDB::open(&wal_dir).unwrap();
            assert_eq!(db.len(), 1);
            assert!(!db.has_block(&h(1)));
            assert!(!db.has_block(&h(2)));
            assert!(db.has_block(&h(3)));
            assert_eq!(db.get_block_cbor(&h(3)).unwrap(), b"b3");
            // prev_hash for h(3) must be preserved through rewrite
            assert_eq!(db.get_block(&h(3)).unwrap().prev_hash, h(2));
        }
    }

    // -----------------------------------------------------------------------
    // WAL corruption recovery tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_wal_corrupted_recovery_partial_write() {
        let dir = tempfile::tempdir().unwrap();
        let wal_dir = dir.path().join("volatile");
        fs::create_dir_all(&wal_dir).unwrap();
        let wal_path = wal_dir.join(WAL_FILENAME);

        // Write one valid new-format entry followed by a truncated entry
        {
            let mut file = File::create(&wal_path).unwrap();

            // Valid entry: slot=100, block_no=10, hash=h(1), prev_hash=h(0)
            let cbor = b"block1";
            file.write_all(&WAL_MAGIC).unwrap();
            file.write_all(&100u64.to_be_bytes()).unwrap();
            file.write_all(&10u64.to_be_bytes()).unwrap();
            file.write_all(h(1).as_bytes()).unwrap();
            file.write_all(h(0).as_bytes()).unwrap(); // prev_hash
            file.write_all(&(cbor.len() as u32).to_be_bytes()).unwrap();
            file.write_all(cbor).unwrap();

            // Truncated entry: only magic + partial slot
            file.write_all(&WAL_MAGIC).unwrap();
            file.write_all(&[0u8; 3]).unwrap(); // Incomplete slot
        }

        // Open should recover the first valid entry and skip the truncated one
        let db = VolatileDB::open(&wal_dir).unwrap();
        assert_eq!(db.len(), 1);
        assert!(db.has_block(&h(1)));
        assert_eq!(db.get_block_cbor(&h(1)).unwrap(), b"block1");
        assert_eq!(db.get_block(&h(1)).unwrap().prev_hash, h(0));
    }

    #[test]
    fn test_wal_corrupted_recovery_bad_magic() {
        let dir = tempfile::tempdir().unwrap();
        let wal_dir = dir.path().join("volatile");
        fs::create_dir_all(&wal_dir).unwrap();
        let wal_path = wal_dir.join(WAL_FILENAME);

        // Write one valid entry followed by garbage
        {
            let mut file = File::create(&wal_path).unwrap();

            let cbor = b"block1";
            file.write_all(&WAL_MAGIC).unwrap();
            file.write_all(&100u64.to_be_bytes()).unwrap();
            file.write_all(&10u64.to_be_bytes()).unwrap();
            file.write_all(h(1).as_bytes()).unwrap();
            file.write_all(h(0).as_bytes()).unwrap(); // prev_hash
            file.write_all(&(cbor.len() as u32).to_be_bytes()).unwrap();
            file.write_all(cbor).unwrap();

            // Invalid magic followed by garbage
            file.write_all(b"JUNK").unwrap();
            file.write_all(&[0xAB; 100]).unwrap();
        }

        let db = VolatileDB::open(&wal_dir).unwrap();
        assert_eq!(db.len(), 1);
        assert!(db.has_block(&h(1)));
    }

    #[test]
    fn test_wal_corrupted_recovery_truncated_cbor() {
        let dir = tempfile::tempdir().unwrap();
        let wal_dir = dir.path().join("volatile");
        fs::create_dir_all(&wal_dir).unwrap();
        let wal_path = wal_dir.join(WAL_FILENAME);

        // Write one valid entry followed by an entry with truncated CBOR
        {
            let mut file = File::create(&wal_path).unwrap();

            // Valid entry
            let cbor = b"ok";
            file.write_all(&WAL_MAGIC).unwrap();
            file.write_all(&100u64.to_be_bytes()).unwrap();
            file.write_all(&10u64.to_be_bytes()).unwrap();
            file.write_all(h(1).as_bytes()).unwrap();
            file.write_all(h(0).as_bytes()).unwrap(); // prev_hash
            file.write_all(&(cbor.len() as u32).to_be_bytes()).unwrap();
            file.write_all(cbor).unwrap();

            // Entry with valid header but cbor_len says 1000 bytes, only 5 written
            file.write_all(&WAL_MAGIC).unwrap();
            file.write_all(&200u64.to_be_bytes()).unwrap();
            file.write_all(&20u64.to_be_bytes()).unwrap();
            file.write_all(h(2).as_bytes()).unwrap();
            file.write_all(h(1).as_bytes()).unwrap(); // prev_hash
            file.write_all(&1000u32.to_be_bytes()).unwrap();
            file.write_all(&[0u8; 5]).unwrap(); // Only 5 of 1000 bytes
        }

        let db = VolatileDB::open(&wal_dir).unwrap();
        assert_eq!(db.len(), 1);
        assert!(db.has_block(&h(1)));
        assert!(!db.has_block(&h(2)));
    }

    #[test]
    fn test_wal_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let wal_dir = dir.path().join("volatile");
        fs::create_dir_all(&wal_dir).unwrap();
        let wal_path = wal_dir.join(WAL_FILENAME);

        // Create empty WAL file
        File::create(&wal_path).unwrap();

        let db = VolatileDB::open(&wal_dir).unwrap();
        assert_eq!(db.len(), 0);
        assert!(db.is_empty());
        assert_eq!(db.get_tip(), None);
    }

    #[test]
    fn test_wal_no_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let wal_dir = dir.path().join("volatile");

        // No WAL file exists yet
        let db = VolatileDB::open(&wal_dir).unwrap();
        assert_eq!(db.len(), 0);
        assert!(db.is_empty());
    }

    #[test]
    fn test_wal_clear_truncates() {
        let dir = tempfile::tempdir().unwrap();
        let wal_dir = dir.path().join("volatile");

        {
            let mut db = VolatileDB::open(&wal_dir).unwrap();
            db.add_block(h(1), 100, 10, h(0), b"b1".to_vec());
            db.add_block(h(2), 200, 20, h(1), b"b2".to_vec());
            db.clear();
        }

        // Re-open: WAL should be empty
        let db = VolatileDB::open(&wal_dir).unwrap();
        assert_eq!(db.len(), 0);
        assert!(db.is_empty());
    }

    #[test]
    fn test_wal_rollback_preserves_all_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let wal_dir = dir.path().join("volatile");

        {
            let mut db = VolatileDB::open(&wal_dir).unwrap();
            db.add_block(h(1), 100, 10, h(0), b"b1".to_vec());
            db.add_block(h(2), 200, 20, h(1), b"b2".to_vec());
            db.add_block(h(3), 300, 30, h(2), b"b3".to_vec());

            // Non-destructive rollback
            db.rollback_to_point(100, Some(&h(1)));
            assert_eq!(db.len(), 3); // All blocks retained
            assert_eq!(db.selected_chain_len(), 1);
        }

        // Re-open: ALL blocks recovered, longest chain selected
        let db = VolatileDB::open(&wal_dir).unwrap();
        assert_eq!(db.len(), 3);
        assert!(db.has_block(&h(1)));
        assert!(db.has_block(&h(2)));
        assert!(db.has_block(&h(3)));
        assert_eq!(db.selected_chain_len(), 3);
        assert_eq!(db.get_block(&h(1)).unwrap().prev_hash, h(0));
    }

    #[test]
    fn test_wal_destructive_rollback_prunes() {
        let dir = tempfile::tempdir().unwrap();
        let wal_dir = dir.path().join("volatile");
        {
            let mut db = VolatileDB::open(&wal_dir).unwrap();
            db.add_block(h(1), 100, 10, h(0), b"b1".to_vec());
            db.add_block(h(2), 200, 20, h(1), b"b2".to_vec());
            db.add_block(h(3), 300, 30, h(2), b"b3".to_vec());
            db.rollback_and_prune(100, Some(&h(1)));
            assert_eq!(db.len(), 1);
        }
        let db = VolatileDB::open(&wal_dir).unwrap();
        assert_eq!(db.len(), 1);
        assert!(db.has_block(&h(1)));
        assert!(!db.has_block(&h(2)));
    }

    #[test]
    fn test_wal_multiple_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let wal_dir = dir.path().join("volatile");

        // Session 1: add blocks 1-2
        {
            let mut db = VolatileDB::open(&wal_dir).unwrap();
            db.add_block(h(1), 100, 10, h(0), b"b1".to_vec());
            db.add_block(h(2), 200, 20, h(1), b"b2".to_vec());
        }

        // Session 2: recover, add block 3
        {
            let mut db = VolatileDB::open(&wal_dir).unwrap();
            assert_eq!(db.len(), 2);
            db.add_block(h(3), 300, 30, h(2), b"b3".to_vec());
            assert_eq!(db.len(), 3);
        }

        // Session 3: recover all 3 blocks
        {
            let db = VolatileDB::open(&wal_dir).unwrap();
            assert_eq!(db.len(), 3);
            assert!(db.has_block(&h(1)));
            assert!(db.has_block(&h(2)));
            assert!(db.has_block(&h(3)));
        }
    }

    // -----------------------------------------------------------------------
    // Legacy 56-byte WAL migration tests
    // -----------------------------------------------------------------------

    /// Write a single legacy 56-byte WAL entry directly to disk.
    fn write_legacy_entry(file: &mut File, slot: u64, block_no: u64, hash: Hash32, cbor: &[u8]) {
        file.write_all(&WAL_MAGIC).unwrap();
        file.write_all(&slot.to_be_bytes()).unwrap();
        file.write_all(&block_no.to_be_bytes()).unwrap();
        file.write_all(hash.as_bytes()).unwrap();
        file.write_all(&(cbor.len() as u32).to_be_bytes()).unwrap();
        file.write_all(cbor).unwrap();
    }

    #[test]
    fn test_legacy_wal_opens_without_panic() {
        // A legacy 56-byte WAL with three entries must open without error and
        // return all blocks with prev_hash = Hash32::ZERO.
        let dir = tempfile::tempdir().unwrap();
        let wal_dir = dir.path().join("volatile");
        fs::create_dir_all(&wal_dir).unwrap();
        let wal_path = wal_dir.join(WAL_FILENAME);

        {
            let mut file = File::create(&wal_path).unwrap();
            write_legacy_entry(&mut file, 100, 10, h(1), b"block1");
            write_legacy_entry(&mut file, 200, 20, h(2), b"block2");
            write_legacy_entry(&mut file, 300, 30, h(3), b"block3");
        }

        let db = VolatileDB::open(&wal_dir).unwrap();
        assert_eq!(db.len(), 3, "all three legacy blocks should be recovered");
        assert!(db.has_block(&h(1)));
        assert!(db.has_block(&h(2)));
        assert!(db.has_block(&h(3)));
    }

    #[test]
    fn test_legacy_wal_prev_hash_zero() {
        // Legacy entries have no stored prev_hash; they must recover as ZERO.
        let dir = tempfile::tempdir().unwrap();
        let wal_dir = dir.path().join("volatile");
        fs::create_dir_all(&wal_dir).unwrap();
        let wal_path = wal_dir.join(WAL_FILENAME);

        {
            let mut file = File::create(&wal_path).unwrap();
            write_legacy_entry(&mut file, 100, 10, h(1), b"b1");
            write_legacy_entry(&mut file, 200, 20, h(2), b"b2");
        }

        let db = VolatileDB::open(&wal_dir).unwrap();
        assert_eq!(
            db.get_block(&h(1)).unwrap().prev_hash,
            Hash32::ZERO,
            "legacy entry must have prev_hash=ZERO"
        );
        assert_eq!(
            db.get_block(&h(2)).unwrap().prev_hash,
            Hash32::ZERO,
            "legacy entry must have prev_hash=ZERO"
        );
    }

    #[test]
    fn test_legacy_wal_upgraded_after_rewrite() {
        // After a flush rewrite the WAL file must use the new 88-byte format,
        // so subsequent reopens parse it as new format.
        let dir = tempfile::tempdir().unwrap();
        let wal_dir = dir.path().join("volatile");
        fs::create_dir_all(&wal_dir).unwrap();
        let wal_path = wal_dir.join(WAL_FILENAME);

        // Seed a legacy WAL with two blocks
        {
            let mut file = File::create(&wal_path).unwrap();
            write_legacy_entry(&mut file, 100, 10, h(1), b"b1");
            write_legacy_entry(&mut file, 200, 20, h(2), b"b2");
        }

        // Open (legacy parse), add a new block, remove the first two to
        // trigger a rewrite in new format.
        {
            let mut db = VolatileDB::open(&wal_dir).unwrap();
            assert_eq!(db.len(), 2);
            // Add a block with a real prev_hash
            db.add_block(h(3), 300, 30, h(2), b"b3".to_vec());
            // Remove legacy blocks — this rewrites the WAL in new format
            db.remove_blocks_up_to_slot(200);
            assert_eq!(db.len(), 1);
        }

        // The WAL file should now be in new format with CRC trailer
        let wal_data = fs::read(&wal_path).unwrap();
        // One entry: 88-byte header + 2 bytes CBOR + 4 bytes CRC trailer
        assert_eq!(
            wal_data.len(),
            WAL_HEADER_SIZE + 2 + WAL_CRC_SIZE,
            "rewritten WAL must use v3 format (88-byte header + CRC trailer)"
        );

        // Reopen and verify prev_hash is correct for the surviving block
        let db = VolatileDB::open(&wal_dir).unwrap();
        assert_eq!(db.len(), 1);
        assert_eq!(
            db.get_block(&h(3)).unwrap().prev_hash,
            h(2),
            "prev_hash must be exact after upgrade rewrite"
        );
    }

    #[test]
    fn test_legacy_wal_single_entry() {
        // A legacy WAL with only one entry is an edge case for detection.
        let dir = tempfile::tempdir().unwrap();
        let wal_dir = dir.path().join("volatile");
        fs::create_dir_all(&wal_dir).unwrap();
        let wal_path = wal_dir.join(WAL_FILENAME);

        {
            let mut file = File::create(&wal_path).unwrap();
            write_legacy_entry(&mut file, 100, 10, h(1), b"solo");
        }

        let db = VolatileDB::open(&wal_dir).unwrap();
        assert_eq!(db.len(), 1);
        assert!(db.has_block(&h(1)));
        assert_eq!(db.get_block_cbor(&h(1)).unwrap(), b"solo");
    }

    // -----------------------------------------------------------------------
    // Additional WAL edge case tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_wal_single_entry_add_remove_empty() {
        // Add one block, remove it, verify WAL file is effectively empty on reopen
        let dir = tempfile::tempdir().unwrap();
        let wal_dir = dir.path().join("volatile");

        {
            let mut db = VolatileDB::open(&wal_dir).unwrap();
            db.add_block(h(1), 100, 10, h(0), b"only_block".to_vec());
            assert_eq!(db.len(), 1);

            // Remove the block by removing all blocks up to its slot
            db.remove_blocks_up_to_slot(100);
            assert_eq!(db.len(), 0);
            assert!(db.is_empty());
        }

        // Reopen: should be empty
        let db = VolatileDB::open(&wal_dir).unwrap();
        assert_eq!(db.len(), 0);
        assert!(db.is_empty());
        assert_eq!(db.get_tip(), None);
    }

    #[test]
    fn test_wal_replay_duplicate_entries() {
        // Manually write WAL with duplicate entries (same hash, different data).
        // Last entry should win since HashMap replaces previous value.
        let dir = tempfile::tempdir().unwrap();
        let wal_dir = dir.path().join("volatile");
        fs::create_dir_all(&wal_dir).unwrap();
        let wal_path = wal_dir.join(WAL_FILENAME);

        {
            let mut file = File::create(&wal_path).unwrap();

            // Write two entries with the same hash but different CBOR
            for cbor in &[b"version_1".as_slice(), b"version_2".as_slice()] {
                file.write_all(&WAL_MAGIC).unwrap();
                file.write_all(&100u64.to_be_bytes()).unwrap();
                file.write_all(&10u64.to_be_bytes()).unwrap();
                file.write_all(h(1).as_bytes()).unwrap();
                file.write_all(h(0).as_bytes()).unwrap(); // prev_hash
                file.write_all(&(cbor.len() as u32).to_be_bytes()).unwrap();
                file.write_all(cbor).unwrap();
            }
        }

        let db = VolatileDB::open(&wal_dir).unwrap();
        // Should have 1 block (duplicate hash -> last wins)
        assert_eq!(db.len(), 1);
        assert!(db.has_block(&h(1)));
        // The second entry overwrites the first
        assert_eq!(db.get_block_cbor(&h(1)).unwrap(), b"version_2");
    }

    #[test]
    fn test_wal_large_100_entries_flush_and_shrink() {
        // Add 100+ entries, flush some, verify WAL shrinks on reopen
        let dir = tempfile::tempdir().unwrap();
        let wal_dir = dir.path().join("volatile");

        {
            let mut db = VolatileDB::open(&wal_dir).unwrap();
            for i in 0..120u8 {
                let hash = Hash32::from_bytes([i; 32]);
                db.add_block(hash, i as u64 * 10, i as u64, h(0), vec![i; 64]);
            }
            assert_eq!(db.len(), 120);

            // Remove first 100 blocks (simulating flush to immutable)
            db.remove_blocks_up_to_slot(990); // slots 0..=990 -> indices 0..=99
        }

        // Reopen: should only have remaining blocks
        let db = VolatileDB::open(&wal_dir).unwrap();
        // Blocks at slots 1000..1190 should remain (indices 100..119)
        assert!(db.len() <= 20);
        assert!(!db.is_empty());

        // Verify the WAL file is smaller than writing 120 entries.
        // Each entry is 88-byte header + 64-byte CBOR = 152 bytes.
        // 120 entries would be ~18240 bytes; remaining ~20 should be much less.
        let wal_path = wal_dir.join(WAL_FILENAME);
        let wal_size = fs::metadata(&wal_path).unwrap().len();
        assert!(wal_size < 5000, "WAL should have shrunk, got {wal_size}");
    }

    #[test]
    fn test_wal_open_close_reopen_state_preserved() {
        // Test that open, add, close, reopen preserves state exactly
        let dir = tempfile::tempdir().unwrap();
        let wal_dir = dir.path().join("volatile");

        {
            let mut db = VolatileDB::open(&wal_dir).unwrap();
            db.add_block(h(1), 100, 10, h(0), b"block_one".to_vec());
            db.add_block(h(2), 200, 20, h(1), b"block_two".to_vec());
            assert_eq!(db.len(), 2);
            assert_eq!(db.get_tip(), Some((200, h(2), 20)));
        }

        // Reopen - all state should be intact
        let db = VolatileDB::open(&wal_dir).unwrap();
        assert_eq!(db.len(), 2);
        assert!(db.has_block(&h(1)));
        assert!(db.has_block(&h(2)));
        assert_eq!(db.get_block_cbor(&h(1)).unwrap(), b"block_one");
        assert_eq!(db.get_block_cbor(&h(2)).unwrap(), b"block_two");
        // Tip should be the highest slot
        assert_eq!(db.get_tip(), Some((200, h(2), 20)));
        // prev_hash round-trips exactly
        assert_eq!(db.get_block(&h(1)).unwrap().prev_hash, h(0));
        assert_eq!(db.get_block(&h(2)).unwrap().prev_hash, h(1));
    }

    #[test]
    fn test_wal_entry_at_64kb_boundary() {
        // Test WAL entry with CBOR data exactly 64KB
        let dir = tempfile::tempdir().unwrap();
        let wal_dir = dir.path().join("volatile");
        let big_cbor = vec![0xAB; 65536]; // exactly 64KB

        {
            let mut db = VolatileDB::open(&wal_dir).unwrap();
            db.add_block(h(1), 100, 10, h(0), big_cbor.clone());
            assert_eq!(db.len(), 1);
        }

        // Reopen and verify the large block survived
        let db = VolatileDB::open(&wal_dir).unwrap();
        assert_eq!(db.len(), 1);
        assert!(db.has_block(&h(1)));
        assert_eq!(db.get_block_cbor(&h(1)).unwrap().len(), 65536);
        assert_eq!(db.get_block_cbor(&h(1)).unwrap(), big_cbor.as_slice());
        assert_eq!(db.get_block(&h(1)).unwrap().prev_hash, h(0));
    }

    #[test]
    fn test_remove_block_updates_tip() {
        let mut db = VolatileDB::new();
        db.add_block(h(1), 100, 10, h(0), b"b1".to_vec());
        db.add_block(h(2), 200, 20, h(1), b"b2".to_vec());
        db.add_block(h(3), 300, 30, h(2), b"b3".to_vec());

        assert_eq!(db.get_tip(), Some((300, h(3), 30)));

        // Remove the tip block
        db.remove_block(&h(3));
        assert!(!db.has_block(&h(3)));
        assert_eq!(db.len(), 2);
        // Note: tip is not recomputed on remove_block, only on rollback
    }

    #[test]
    fn test_volatile_default_trait() {
        let db = VolatileDB::default();
        assert!(db.is_empty());
        assert_eq!(db.len(), 0);
        assert_eq!(db.get_tip(), None);
    }

    // -- Multi-fork chain management tests ------------------------------------

    #[test]
    fn test_selected_chain_grows_sequentially() {
        let mut db = VolatileDB::new();
        db.add_block(h(1), 100, 10, h(0), b"b1".to_vec());
        db.add_block(h(2), 200, 20, h(1), b"b2".to_vec());
        db.add_block(h(3), 300, 30, h(2), b"b3".to_vec());
        assert_eq!(db.selected_chain_len(), 3);
        assert_eq!(db.get_tip(), Some((300, h(3), 30)));
    }

    #[test]
    fn test_fork_block_not_on_selected_chain() {
        let mut db = VolatileDB::new();
        db.add_block(h(1), 100, 10, h(0), b"b1".to_vec());
        db.add_block(h(2), 200, 20, h(1), b"b2".to_vec());
        db.add_block(h(3), 100, 10, h(0), b"b3_fork".to_vec());
        assert!(db.has_block(&h(3)));
        assert_eq!(db.selected_chain_len(), 2);
        assert_eq!(db.get_tip(), Some((200, h(2), 20)));
    }

    #[test]
    fn test_rollback_then_extend_new_chain() {
        let mut db = VolatileDB::new();
        db.add_block(h(1), 100, 10, h(0), b"b1".to_vec());
        db.add_block(h(2), 200, 20, h(1), b"b2".to_vec());
        db.add_block(h(3), 300, 30, h(2), b"b3".to_vec());

        db.rollback_to_point(100, Some(&h(1)));
        db.add_block(h(4), 200, 20, h(1), b"b4_new".to_vec());
        db.add_block(h(5), 300, 30, h(4), b"b5_new".to_vec());

        assert_eq!(db.selected_chain_len(), 3);
        assert_eq!(db.get_tip(), Some((300, h(5), 30)));
        assert_eq!(db.len(), 5);
        let (_, hash, _) = db.get_block_by_number(20).unwrap();
        assert_eq!(hash, h(4));
    }

    #[test]
    fn test_walk_chain_back() {
        let mut db = VolatileDB::new();
        db.add_block(h(1), 100, 10, h(0), b"b1".to_vec());
        db.add_block(h(2), 200, 20, h(1), b"b2".to_vec());
        db.add_block(h(3), 300, 30, h(2), b"b3".to_vec());
        assert_eq!(db.walk_chain_back(&h(3)), vec![h(1), h(2), h(3)]);
        assert_eq!(db.walk_chain_back(&h(2)), vec![h(1), h(2)]);
    }

    #[test]
    fn test_get_leaf_tips() {
        let mut db = VolatileDB::new();
        db.add_block(h(1), 100, 10, h(0), b"b1".to_vec());
        db.add_block(h(2), 200, 20, h(1), b"b2".to_vec());
        assert_eq!(db.get_leaf_tips().len(), 1);
        db.add_block(h(3), 200, 20, h(1), b"b3_fork".to_vec());
        assert_eq!(db.get_leaf_tips().len(), 2);
    }

    #[test]
    fn test_switch_chain() {
        let mut db = VolatileDB::new();
        db.add_block(h(1), 100, 10, h(0), b"b1".to_vec());
        db.add_block(h(2), 200, 20, h(1), b"b2".to_vec());
        db.add_block(h(3), 300, 30, h(2), b"b3".to_vec());
        db.add_block(h(4), 200, 20, h(1), b"b4".to_vec());
        db.add_block(h(5), 300, 30, h(4), b"b5".to_vec());
        db.add_block(h(6), 400, 40, h(5), b"b6".to_vec());

        assert_eq!(db.get_tip(), Some((300, h(3), 30)));
        let plan = db.switch_chain(&h(6)).unwrap();
        assert_eq!(plan.intersection, h(1));
        assert_eq!(plan.rollback, vec![h(3), h(2)]);
        assert_eq!(plan.apply, vec![h(4), h(5), h(6)]);
        assert_eq!(db.get_tip(), Some((400, h(6), 40)));
        assert_eq!(db.len(), 6);
    }

    #[test]
    fn test_slot_battle_scenario() {
        let mut db = VolatileDB::new();
        db.add_block(h(1), 90, 9, h(0), b"parent".to_vec());
        db.add_block(h(2), 100, 10, h(1), b"our_forged".to_vec());
        db.add_block(h(3), 100, 10, h(1), b"network_better".to_vec());

        db.rollback_to_point(90, Some(&h(1)));
        assert!(db.has_block(&h(2)));
        assert!(db.has_block(&h(3)));

        let plan = db.switch_chain(&h(3)).unwrap();
        assert_eq!(plan.apply, vec![h(3)]);
        assert_eq!(db.get_tip(), Some((100, h(3), 10)));
        assert_eq!(db.len(), 3);
    }

    #[test]
    fn test_flush_trims_selected_chain() {
        let mut db = VolatileDB::new();
        db.add_block(h(1), 100, 10, h(0), b"b1".to_vec());
        db.add_block(h(2), 200, 20, h(1), b"b2".to_vec());
        db.add_block(h(3), 300, 30, h(2), b"b3".to_vec());
        assert_eq!(db.selected_chain_len(), 3);
        db.remove_blocks_up_to_slot(200);
        assert_eq!(db.len(), 1);
        assert_eq!(db.selected_chain_len(), 1);
    }

    #[test]
    fn test_rebuild_selected_chain_picks_longest() {
        let mut db = VolatileDB::new();
        db.add_block(h(1), 100, 10, h(0), b"b1".to_vec());
        db.add_block(h(2), 200, 20, h(1), b"b2".to_vec());
        db.add_block(h(3), 200, 20, h(1), b"b3".to_vec());
        db.add_block(h(4), 300, 30, h(3), b"b4".to_vec());
        db.add_block(h(5), 400, 40, h(4), b"b5".to_vec());
        // Currently on A: h(1)→h(2)
        assert_eq!(db.get_tip(), Some((200, h(2), 20)));
        db.rebuild_selected_chain();
        // B is longer: h(1)→h(3)→h(4)→h(5)
        assert_eq!(db.selected_chain_len(), 4);
        assert_eq!(db.get_tip(), Some((400, h(5), 40)));
    }

    // -----------------------------------------------------------------------
    // WAL CRC32 validation tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_wal_crc_validates_on_replay() {
        // Write entries via the normal API (produces v3 with CRC), then
        // verify that replay succeeds and all entries are recovered.
        let dir = tempfile::tempdir().unwrap();
        let wal_dir = dir.path().join("volatile");

        {
            let mut db = VolatileDB::open(&wal_dir).unwrap();
            db.add_block(h(1), 100, 10, h(0), b"block1".to_vec());
            db.add_block(h(2), 200, 20, h(1), b"block2".to_vec());
            db.add_block(h(3), 300, 30, h(2), b"block3".to_vec());
        }

        // Reopen — must recover all three blocks via CRC-validated replay.
        let db = VolatileDB::open(&wal_dir).unwrap();
        assert_eq!(db.len(), 3);
        assert!(db.has_block(&h(1)));
        assert!(db.has_block(&h(2)));
        assert!(db.has_block(&h(3)));
    }

    #[test]
    fn test_wal_crc_detects_corrupted_cbor() {
        // Write a v3 WAL entry, then corrupt the CBOR data. Replay should
        // detect the CRC mismatch and stop before the corrupted entry.
        let dir = tempfile::tempdir().unwrap();
        let wal_dir = dir.path().join("volatile");
        fs::create_dir_all(&wal_dir).unwrap();
        let wal_path = wal_dir.join(WAL_FILENAME);

        // Write two valid v3 entries via the API
        {
            let mut db = VolatileDB::open(&wal_dir).unwrap();
            db.add_block(h(1), 100, 10, h(0), b"block1".to_vec());
            db.add_block(h(2), 200, 20, h(1), b"block2".to_vec());
        }

        // Corrupt the CBOR of the second entry by flipping a byte.
        {
            let mut data = fs::read(&wal_path).unwrap();
            // First entry: 88 header + 6 cbor + 4 crc = 98 bytes
            // Second entry starts at 98: header(88) + cbor starts at 186
            let cbor_offset = 98 + WAL_HEADER_SIZE;
            assert!(cbor_offset < data.len(), "offset must be within file");
            data[cbor_offset] ^= 0xFF; // flip bits
            fs::write(&wal_path, &data).unwrap();
        }

        // Reopen — should recover only the first (uncorrupted) entry.
        let db = VolatileDB::open(&wal_dir).unwrap();
        assert_eq!(db.len(), 1, "only the first entry should survive CRC check");
        assert!(db.has_block(&h(1)));
        assert!(!db.has_block(&h(2)));
    }

    #[test]
    fn test_wal_crc_detects_truncated_trailer() {
        // Write a v3 entry then truncate the CRC trailer.
        let dir = tempfile::tempdir().unwrap();
        let wal_dir = dir.path().join("volatile");
        fs::create_dir_all(&wal_dir).unwrap();
        let wal_path = wal_dir.join(WAL_FILENAME);

        // Write one valid entry
        {
            let mut db = VolatileDB::open(&wal_dir).unwrap();
            db.add_block(h(1), 100, 10, h(0), b"block1".to_vec());
        }

        // Truncate the last 2 bytes (partial CRC)
        {
            let data = fs::read(&wal_path).unwrap();
            fs::write(&wal_path, &data[..data.len() - 2]).unwrap();
        }

        // Reopen — the single entry has a truncated CRC trailer.
        // detect_crc_format returns false (CRC doesn't validate because
        // it's truncated), so replay treats it as v2 and recovers the
        // entry since the header+cbor are complete.
        let db = VolatileDB::open(&wal_dir).unwrap();
        assert_eq!(
            db.len(),
            1,
            "should recover entry when CRC is truncated (v2 fallback)"
        );
        assert!(db.has_block(&h(1)));
    }

    #[test]
    fn test_wal_v2_entries_replay_without_crc() {
        // Manually construct a v2 WAL file (no CRC trailers) and verify
        // that replay works correctly without CRC validation.
        let dir = tempfile::tempdir().unwrap();
        let wal_dir = dir.path().join("volatile");
        fs::create_dir_all(&wal_dir).unwrap();
        let wal_path = wal_dir.join(WAL_FILENAME);

        {
            use std::io::Write;
            let mut file = File::create(&wal_path).unwrap();
            // Write two v2 entries (no CRC trailer)
            for i in 1..=2u8 {
                let cbor = format!("block{i}");
                let cbor_bytes = cbor.as_bytes();
                file.write_all(&WAL_MAGIC).unwrap();
                file.write_all(&(i as u64 * 100).to_be_bytes()).unwrap(); // slot
                file.write_all(&(i as u64 * 10).to_be_bytes()).unwrap(); // block_no
                file.write_all(h(i).as_bytes()).unwrap(); // hash
                file.write_all(h(i - 1).as_bytes()).unwrap(); // prev_hash
                file.write_all(&(cbor_bytes.len() as u32).to_be_bytes())
                    .unwrap();
                file.write_all(cbor_bytes).unwrap();
                // No CRC trailer — v2 format
            }
        }

        let db = VolatileDB::open(&wal_dir).unwrap();
        assert_eq!(db.len(), 2, "v2 entries must replay without CRC");
        assert!(db.has_block(&h(1)));
        assert!(db.has_block(&h(2)));
        assert_eq!(db.get_block(&h(2)).unwrap().prev_hash, h(1));
    }
}
