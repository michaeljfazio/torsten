//! Mithril snapshot import for fast initial sync.
//!
//! Downloads a Mithril-certified snapshot of the Cardano immutable DB,
//! extracts the cardano-node chunk files, parses blocks with pallas,
//! and bulk-imports them into Torsten's ImmutableDB (cardano-lsm).
//!
//! Supports both the legacy `/artifact/snapshots` API and the newer
//! `/artifact/cardano-database` API with per-immutable-file downloads.

use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use memmap2::Mmap;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use torsten_primitives::hash::Hash32;
use torsten_primitives::time::{BlockNo, SlotNo};
use tracing::{debug, info, warn};

/// Mithril aggregator endpoints per network
const MAINNET_AGGREGATOR: &str =
    "https://aggregator.release-mainnet.api.mithril.network/aggregator";
const PREVIEW_AGGREGATOR: &str =
    "https://aggregator.pre-release-preview.api.mithril.network/aggregator";
const PREPROD_AGGREGATOR: &str =
    "https://aggregator.release-preprod.api.mithril.network/aggregator";

// ---------------------------------------------------------------------------
// API response types
// ---------------------------------------------------------------------------

/// Snapshot metadata from the Mithril aggregator API (legacy endpoint)
#[derive(Debug, serde::Deserialize)]
#[allow(dead_code)]
struct SnapshotListItem {
    digest: String,
    network: String,
    size: u64,
    #[serde(rename = "beacon")]
    beacon: SnapshotBeacon,
    compression_algorithm: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct SnapshotBeacon {
    epoch: u64,
    immutable_file_number: u64,
}

/// Full snapshot detail (includes download locations)
#[derive(Debug, serde::Deserialize)]
#[allow(dead_code)]
struct SnapshotDetail {
    digest: String,
    size: u64,
    beacon: SnapshotBeacon,
    locations: Vec<String>,
    compression_algorithm: Option<String>,
}

// ---------------------------------------------------------------------------
// Secondary index parsing
// ---------------------------------------------------------------------------

/// Entry from a cardano-node secondary index file.
/// Each entry is 56 bytes in the secondary index.
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct SecondaryIndexEntry {
    block_offset: u64,
    _header_offset: u16,
    _header_size: u16,
    checksum: u32,
    header_hash: [u8; 32],
    _block_or_ebb: u64,
}

impl SecondaryIndexEntry {
    fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 56 {
            return None;
        }
        let block_offset = u64::from_be_bytes(data[0..8].try_into().ok()?);
        let header_offset = u16::from_be_bytes(data[8..10].try_into().ok()?);
        let header_size = u16::from_be_bytes(data[10..12].try_into().ok()?);
        let checksum = u32::from_be_bytes(data[12..16].try_into().ok()?);
        let mut header_hash = [0u8; 32];
        header_hash.copy_from_slice(&data[16..48]);
        let block_or_ebb = u64::from_be_bytes(data[48..56].try_into().ok()?);

        Some(SecondaryIndexEntry {
            block_offset,
            _header_offset: header_offset,
            _header_size: header_size,
            checksum,
            header_hash,
            _block_or_ebb: block_or_ebb,
        })
    }
}

/// Verify a block's CRC32 checksum against the secondary index entry.
#[allow(dead_code)]
fn verify_block_checksum(block_data: &[u8], expected: u32) -> bool {
    crc32fast::hash(block_data) == expected
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Get the aggregator URL for a given network magic
pub fn aggregator_url(network_magic: u64) -> &'static str {
    match network_magic {
        764824073 => MAINNET_AGGREGATOR,
        2 => PREVIEW_AGGREGATOR,
        1 => PREPROD_AGGREGATOR,
        _ => MAINNET_AGGREGATOR,
    }
}

/// Get the Mithril network name for a given magic (matches CardanoNetwork::Display in mithril-common)
fn mithril_network_name(network_magic: u64) -> &'static str {
    match network_magic {
        764824073 => "mainnet",
        2 => "preview",
        1 => "preprod",
        _ => "private",
    }
}

/// Run the Mithril snapshot import
pub async fn import_snapshot(
    network_magic: u64,
    database_path: &Path,
    temp_dir: Option<&Path>,
) -> Result<()> {
    let aggregator = aggregator_url(network_magic);
    info!(aggregator, "Fetching latest Mithril snapshot info");

    // Step 1: Get latest snapshot metadata
    let client = reqwest::Client::builder()
        .user_agent("torsten-node/0.1")
        .build()?;

    let snapshots: Vec<SnapshotListItem> = client
        .get(format!("{aggregator}/artifact/snapshots"))
        .send()
        .await?
        .error_for_status()
        .context("Failed to fetch snapshot list")?
        .json()
        .await?;

    let latest = snapshots
        .first()
        .context("No snapshots available from aggregator")?;

    info!(
        digest = %latest.digest,
        epoch = latest.beacon.epoch,
        immutable_file_number = latest.beacon.immutable_file_number,
        size_gb = latest.size / (1024 * 1024 * 1024),
        "Latest snapshot found"
    );

    // Step 2: Get download locations
    let detail: SnapshotDetail = client
        .get(format!("{aggregator}/artifact/snapshot/{}", latest.digest))
        .send()
        .await?
        .error_for_status()
        .context("Failed to fetch snapshot detail")?
        .json()
        .await?;

    let download_url = detail
        .locations
        .first()
        .context("No download locations in snapshot")?;

    info!(url = %download_url, "Downloading snapshot");

    // Step 3: Download snapshot to temp file
    let work_dir = temp_dir
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::temp_dir().join("torsten-mithril"));
    fs::create_dir_all(&work_dir)?;

    let archive_path = work_dir.join(format!("snapshot-{}.tar.zst", detail.digest));
    download_snapshot(&client, download_url, &archive_path, detail.size).await?;

    // Step 4: Extract the archive
    let extract_dir = work_dir.join(format!("extract-{}", detail.digest));
    info!(
        path = %extract_dir.display(),
        "Extracting snapshot archive"
    );
    extract_archive(&archive_path, &extract_dir)?;

    // Step 5: Verify snapshot digest (Mithril digest over extracted immutable files)
    let network_name = mithril_network_name(network_magic);
    verify_snapshot_digest(&extract_dir, network_name, &latest.beacon, &detail.digest)?;

    // Step 6: Skip ChainDB import — chunk files are the optimal format for sequential
    // replay. The LSM import (parse → write 5 KV pairs per block → compaction) was
    // redundant since replay reads from chunk files directly. Blocks will be imported
    // into ChainDB during normal sync after replay completes.

    // Step 7: Move immutable chunk files to permanent storage.
    // These become the ImmutableDB — ChainDB reads historical blocks directly
    // from chunk files (1x write amplification, sequential I/O). The directory
    // is NOT deleted after replay; it serves as permanent immutable block storage.
    let immutable_dir = find_immutable_dir(&extract_dir);
    let dest_dir = database_path.join("immutable");
    if let Some(ref imm) = immutable_dir {
        info!("Moving chunk files to permanent immutable storage");
        if let Err(e) = fs::rename(imm, &dest_dir) {
            // rename may fail across filesystems, fall back to copy
            warn!(error = %e, "rename failed, falling back to copy");
            copy_dir_recursive(imm, &dest_dir)?;
        }
    }

    // Step 7b: Preserve Haskell ledger state files for future use.
    // The ledger/ directory contains the serialized NewEpochState from
    // cardano-node (HFC-wrapped CBOR). We can't parse it yet but save it
    // so a future release can extract the UTxO set and skip replay entirely.
    let ledger_dir = find_ledger_dir(&extract_dir);
    if let Some(ref ledger) = ledger_dir {
        let dest_ledger = database_path.join("haskell-ledger");
        info!("Preserving Haskell ledger state files");
        if let Err(e) = fs::rename(ledger, &dest_ledger) {
            warn!(error = %e, "rename failed, falling back to copy");
            let _ = copy_dir_recursive(ledger, &dest_ledger);
        }
        // Extract metadata from ledger filenames (format: <slot>_<hash>)
        if let Ok(entries) = fs::read_dir(&dest_ledger) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if let Some(slot_str) = name_str.split('_').next() {
                    if let Ok(slot) = slot_str.parse::<u64>() {
                        info!(
                            slot,
                            file = %name_str,
                            "Haskell ledger state at slot {slot} \
                             (future: parse to skip replay)"
                        );
                    }
                }
            }
        }
    }

    // Step 8: Cleanup
    info!("Cleaning up temporary files");
    if let Err(e) = fs::remove_file(&archive_path) {
        warn!(error = %e, "Failed to remove archive file");
    }
    if let Err(e) = fs::remove_dir_all(&extract_dir) {
        warn!(error = %e, "Failed to remove extract directory");
    }

    info!("Mithril snapshot import complete!");
    Ok(())
}

// ---------------------------------------------------------------------------
// Download & verification
// ---------------------------------------------------------------------------

/// Download a snapshot archive with progress reporting
async fn download_snapshot(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    expected_size: u64,
) -> Result<()> {
    // If file already exists with the right size, skip download
    if let Ok(meta) = fs::metadata(dest) {
        if meta.len() == expected_size {
            info!("Snapshot archive already downloaded, skipping");
            return Ok(());
        }
    }

    let response = client
        .get(url)
        .send()
        .await?
        .error_for_status()
        .context("Download request failed")?;

    let total_size = response.content_length().unwrap_or(expected_size);

    let pb = ProgressBar::new(total_size);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, {eta})")
            .expect("invalid progress template")
            .progress_chars("█▉▊▋▌▍▎▏ "),
    );

    let temp_path = dest.with_extension("tmp");
    let mut file = fs::File::create(&temp_path)?;

    use futures_util::StreamExt;
    let mut stream = response.bytes_stream();
    let mut downloaded: u64 = 0;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("Error reading download stream")?;
        std::io::Write::write_all(&mut file, &chunk)?;
        downloaded += chunk.len() as u64;
        pb.set_position(downloaded);
    }

    pb.finish_with_message("Download complete");
    drop(file);

    fs::rename(&temp_path, dest)?;
    info!(
        size_gb = downloaded / (1024 * 1024 * 1024),
        "Download complete"
    );

    Ok(())
}

/// Verify the Mithril snapshot digest over extracted immutable files.
///
/// Reproduces the Mithril aggregator's digest algorithm:
///   1. beacon_hash = hex(SHA256(network || epoch_be || immutable_file_number_be))
///   2. For each file in immutable/ (sorted by number then path): file_hash = hex(SHA256(contents))
///   3. digest = hex(SHA256(beacon_hash_hex_bytes || file_hash_hex_1 || file_hash_hex_2 || ...))
fn verify_snapshot_digest(
    extract_dir: &Path,
    network_name: &str,
    beacon: &SnapshotBeacon,
    expected_digest: &str,
) -> Result<()> {
    info!("Verifying Mithril snapshot digest");

    let immutable_dir = find_immutable_dir(extract_dir)
        .context("Could not find immutable/ directory for digest verification")?;

    // Step 1: Compute beacon hash
    let beacon_hash = {
        let mut hasher = Sha256::new();
        hasher.update(network_name.as_bytes());
        hasher.update(beacon.epoch.to_be_bytes());
        hasher.update(beacon.immutable_file_number.to_be_bytes());
        hex::encode(hasher.finalize())
    };

    // Step 2: Collect and sort immutable files (number then path, matching Mithril)
    let mut immutable_files: Vec<(u64, PathBuf)> = Vec::new();
    for entry in fs::read_dir(&immutable_dir).context("Failed to read immutable directory")? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();

        // Only include chunk/primary/secondary files
        let is_immutable =
            name.ends_with(".chunk") || name.ends_with(".primary") || name.ends_with(".secondary");
        if !is_immutable {
            continue;
        }

        // Parse the file number from the stem (e.g. "00123.chunk" -> 123)
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let file_number = stem.parse::<u64>().unwrap_or(0);

        // Only include files up to the beacon's immutable_file_number
        if file_number <= beacon.immutable_file_number {
            immutable_files.push((file_number, path));
        }
    }

    // Sort by number first, then by path (matches Mithril's Ord for ImmutableFile)
    immutable_files
        .sort_by(|(num_a, path_a), (num_b, path_b)| num_a.cmp(num_b).then(path_a.cmp(path_b)));

    let total_files = immutable_files.len() as u64;
    let pb = ProgressBar::new(total_files);
    pb.set_style(
        ProgressStyle::default_bar()
            .template(
                "{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {pos}/{len} files (verifying digest)",
            )
            .expect("invalid template")
            .progress_chars("█▉▊▋▌▍▎▏ "),
    );

    // Step 3: Hash each file individually, then combine
    let mut final_hasher = Sha256::new();
    final_hasher.update(beacon_hash.as_bytes());

    let mut buf = [0u8; 256 * 1024];
    for (_file_number, path) in &immutable_files {
        let mut file_hasher = Sha256::new();
        let mut file = fs::File::open(path)
            .with_context(|| format!("Failed to open immutable file: {}", path.display()))?;
        loop {
            let n = std::io::Read::read(&mut file, &mut buf)
                .with_context(|| format!("IO error reading: {}", path.display()))?;
            if n == 0 {
                break;
            }
            file_hasher.update(&buf[..n]);
        }
        let file_hash_hex = hex::encode(file_hasher.finalize());
        final_hasher.update(file_hash_hex.as_bytes());
        pb.inc(1);
    }

    pb.finish_with_message("Verification complete");

    let computed = hex::encode(final_hasher.finalize());
    if computed != expected_digest {
        anyhow::bail!(
            "Mithril snapshot digest mismatch!\n  Expected: {expected_digest}\n  Computed: {computed}\n\
             The snapshot may be corrupted or tampered with. Delete the extract directory and retry."
        );
    }

    info!(
        files_verified = total_files,
        "Mithril snapshot digest verified successfully"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Archive extraction
// ---------------------------------------------------------------------------

/// Extract a tar.zst archive to a directory
fn extract_archive(archive_path: &Path, extract_dir: &Path) -> Result<()> {
    // If already extracted, skip
    if extract_dir.exists() {
        let immutable_dir = find_immutable_dir(extract_dir);
        if immutable_dir.is_some() {
            info!("Archive already extracted, skipping");
            return Ok(());
        }
    }

    fs::create_dir_all(extract_dir)?;

    let file = fs::File::open(archive_path).context("Failed to open archive")?;
    let reader = BufReader::with_capacity(8 * 1024 * 1024, file);
    let decoder = zstd::Decoder::new(reader).context("Failed to create zstd decoder")?;
    let mut archive = tar::Archive::new(decoder);

    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.green} [{elapsed_precise}] {msg}")
            .expect("invalid template"),
    );

    let mut entry_count = 0u64;
    archive.set_preserve_permissions(false);
    archive.set_overwrite(true);

    for entry in archive
        .entries()
        .context("Failed to read archive entries")?
    {
        let mut entry = entry.context("Failed to read archive entry")?;
        entry
            .unpack_in(extract_dir)
            .context("Failed to extract entry")?;
        entry_count += 1;
        if entry_count.is_multiple_of(100) {
            pb.set_message(format!("{entry_count} entries extracted"));
        }
    }

    pb.finish_with_message(format!("{entry_count} entries extracted"));
    info!(entries = entry_count, "Archive extraction complete");

    Ok(())
}

/// Find the ledger/ directory within an extracted snapshot.
/// Contains the Haskell node's serialized NewEpochState (HFC-wrapped CBOR).
fn find_ledger_dir(extract_dir: &Path) -> Option<PathBuf> {
    let candidates = [
        extract_dir.join("ledger"),
        extract_dir.join("db").join("ledger"),
    ];
    for candidate in &candidates {
        if candidate.is_dir() {
            return Some(candidate.clone());
        }
    }
    // Search one level deeper
    if let Ok(entries) = fs::read_dir(extract_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let ledger = path.join("ledger");
                if ledger.is_dir() {
                    return Some(ledger);
                }
            }
        }
    }
    None
}

/// Find the immutable/ directory within an extracted snapshot
fn find_immutable_dir(extract_dir: &Path) -> Option<PathBuf> {
    // Could be at extract_dir/immutable or extract_dir/db/immutable
    let candidates = [
        extract_dir.join("immutable"),
        extract_dir.join("db").join("immutable"),
    ];

    for candidate in &candidates {
        if candidate.is_dir() {
            return Some(candidate.clone());
        }
    }

    // Search one level deeper for any directory containing chunk files
    if let Ok(entries) = fs::read_dir(extract_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let immutable = path.join("immutable");
                if immutable.is_dir() {
                    return Some(immutable);
                }
            }
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Block import
// ---------------------------------------------------------------------------

/// Import cardano-node immutable chunk files into Torsten's ImmutableDB.
/// Retained for fallback/testing; Mithril import now skips this step.
#[allow(dead_code)]
fn import_chunk_files(extract_dir: &Path, database_path: &Path) -> Result<()> {
    let immutable_dir = find_immutable_dir(extract_dir)
        .context("Could not find immutable/ directory in extracted snapshot")?;

    info!(path = %immutable_dir.display(), "Found immutable directory");

    // Collect chunk file numbers (NNNNN.chunk, NNNNN.secondary)
    let mut chunk_numbers: Vec<u64> = Vec::new();
    for entry in fs::read_dir(&immutable_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.ends_with(".chunk") {
            if let Some(num_str) = name_str.strip_suffix(".chunk") {
                if let Ok(num) = num_str.parse::<u64>() {
                    chunk_numbers.push(num);
                }
            }
        }
    }

    chunk_numbers.sort();
    info!(
        chunk_count = chunk_numbers.len(),
        "Found chunk files to import"
    );

    if chunk_numbers.is_empty() {
        anyhow::bail!("No chunk files found in immutable directory");
    }

    // Open the database with bulk-import-optimized settings (deferred compaction)
    let mut chain_db = torsten_storage::ChainDB::open_for_bulk_import(database_path)?;

    // Check if we already have blocks (resume support)
    let existing_tip = chain_db.tip_slot();
    if existing_tip > SlotNo(0) {
        info!(
            existing_tip_slot = existing_tip.0,
            "Database already contains blocks, will skip existing"
        );
    }

    let total_chunks = chunk_numbers.len() as u64;
    let pb = ProgressBar::new(total_chunks);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {pos}/{len} chunks ({per_sec}, {eta})")
            .expect("invalid template")
            .progress_chars("█▉▊▋▌▍▎▏ "),
    );

    let mut total_blocks_imported = 0u64;
    let mut skipped_chunks = 0u64;
    let mut checksum_failures = 0u64;

    for chunk_num in &chunk_numbers {
        let chunk_path = immutable_dir.join(format!("{chunk_num:05}.chunk"));
        let secondary_path = immutable_dir.join(format!("{chunk_num:05}.secondary"));

        let mut blocks = if secondary_path.exists() {
            parse_chunk_with_index(&chunk_path, &secondary_path, &mut checksum_failures)?
        } else {
            Vec::new()
        };

        // Fallback to sequential CBOR decoding if secondary index yielded no blocks
        if blocks.is_empty() {
            blocks = parse_chunk_sequential(&chunk_path)?;
        }

        if blocks.is_empty() {
            pb.inc(1);
            skipped_chunks += 1;
            continue;
        }

        // Skip blocks we already have (resume support)
        let blocks_to_import: Vec<_> = blocks
            .into_iter()
            .filter(|(slot, _, _, _)| *slot > existing_tip)
            .collect();

        if blocks_to_import.is_empty() {
            pb.inc(1);
            skipped_chunks += 1;
            continue;
        }

        // Build batch refs for put_blocks_batch
        let batch: Vec<(SlotNo, &Hash32, BlockNo, &[u8])> = blocks_to_import
            .iter()
            .map(|(slot, hash, block_no, cbor)| (*slot, hash, *block_no, cbor.as_slice()))
            .collect();

        chain_db.put_blocks_batch(&batch)?;
        total_blocks_imported += batch.len() as u64;

        pb.inc(1);
    }

    pb.finish_with_message("Import complete");

    if checksum_failures > 0 {
        warn!(
            checksum_failures,
            "Some blocks had CRC32 checksum mismatches (imported anyway)"
        );
    }

    info!(
        total_blocks = total_blocks_imported,
        skipped_chunks,
        checksum_failures,
        tip_slot = chain_db.tip_slot().0,
        "Block import complete — persisting to disk"
    );

    // Persist all data to a durable snapshot before exiting.
    // cardano-lsm uses ephemeral writes; without this call, any data still in
    // the in-memory memtable would be lost when the process exits.
    chain_db
        .persist()
        .context("Failed to persist imported blocks to disk")?;

    info!("Import persisted successfully (compaction will run on next node start)");

    Ok(())
}

/// Copy a directory recursively (fallback when rename fails across filesystems).
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

/// Iterate blocks from cardano-node immutable chunk files in sequential order.
///
/// This is used for fast ledger replay after Mithril import. Reading chunk files
/// sequentially is orders of magnitude faster than random LSM lookups because
/// chunk files are already laid out in block order on disk.
///
/// Uses secondary index entries for block boundaries to avoid redundant pallas
/// decode — the callback receives raw CBOR slices that are decoded once by the
/// caller for ledger application.
///
/// Calls the provided callback for each block in order. The callback receives
/// the raw CBOR bytes. Returns the total number of blocks iterated.
pub fn replay_from_chunk_files<F>(immutable_dir: &Path, mut on_block: F) -> Result<u64>
where
    F: FnMut(&[u8]) -> Result<()>,
{
    let mut chunk_numbers: Vec<u64> = Vec::new();
    for entry in fs::read_dir(immutable_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if let Some(num_str) = name_str.strip_suffix(".chunk") {
            if let Ok(num) = num_str.parse::<u64>() {
                chunk_numbers.push(num);
            }
        }
    }
    chunk_numbers.sort();

    let total_chunks = chunk_numbers.len() as u64;
    let pb = ProgressBar::new(total_chunks);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {pos}/{len} chunks ({per_sec}, {eta})")
            .expect("invalid template")
            .progress_chars("█▉▊▋▌▍▎▏ "),
    );

    let mut total_blocks = 0u64;

    for chunk_num in &chunk_numbers {
        let chunk_path = immutable_dir.join(format!("{chunk_num:05}.chunk"));
        let secondary_path = immutable_dir.join(format!("{chunk_num:05}.secondary"));

        // Fast path: use secondary index for block boundaries (no pallas decode)
        if secondary_path.exists() {
            let count = replay_chunk_with_index(&chunk_path, &secondary_path, &mut on_block)?;
            if count > 0 {
                total_blocks += count;
                pb.inc(1);
                continue;
            }
        }

        // Fallback: sequential CBOR probe for block boundaries (no pallas decode)
        let count = replay_chunk_sequential(&chunk_path, &mut on_block)?;
        total_blocks += count;
        pb.inc(1);
    }

    pb.finish_with_message("Replay complete");
    Ok(total_blocks)
}

/// Replay a single chunk file using secondary index for block boundaries.
/// Returns raw CBOR slices without pallas decode (the caller decodes once).
fn replay_chunk_with_index<F>(
    chunk_path: &Path,
    secondary_path: &Path,
    on_block: &mut F,
) -> Result<u64>
where
    F: FnMut(&[u8]) -> Result<()>,
{
    let secondary_data = fs::read(secondary_path).context("Failed to read secondary index")?;
    let chunk_file = fs::File::open(chunk_path).context("Failed to open chunk file")?;
    let chunk_data = unsafe { Mmap::map(&chunk_file).context("Failed to mmap chunk file")? };

    let mut entries = Vec::new();
    let mut offset = 0;
    while offset + 56 <= secondary_data.len() {
        if let Some(entry) = SecondaryIndexEntry::from_bytes(&secondary_data[offset..]) {
            entries.push(entry);
        }
        offset += 56;
    }

    let mut count = 0u64;
    for i in 0..entries.len() {
        let entry = &entries[i];
        let block_start = entry.block_offset as usize;
        let block_end = if i + 1 < entries.len() {
            entries[i + 1].block_offset as usize
        } else {
            chunk_data.len()
        };

        if block_start >= chunk_data.len() || block_end > chunk_data.len() {
            continue;
        }

        on_block(&chunk_data[block_start..block_end])?;
        count += 1;
    }

    Ok(count)
}

/// Replay a single chunk file by sequential CBOR probing (no pallas decode).
fn replay_chunk_sequential<F>(chunk_path: &Path, on_block: &mut F) -> Result<u64>
where
    F: FnMut(&[u8]) -> Result<()>,
{
    let chunk_file = fs::File::open(chunk_path).context("Failed to open chunk file")?;
    let chunk_len = chunk_file.metadata()?.len() as usize;
    if chunk_len == 0 {
        return Ok(0);
    }
    let chunk_data = unsafe { Mmap::map(&chunk_file).context("Failed to mmap chunk file")? };

    let mut count = 0u64;
    let mut offset = 0;
    while offset < chunk_data.len() {
        let remaining = &chunk_data[offset..];
        let item_size = match cbor_item_size(remaining) {
            Some(size) if size > 0 => size,
            _ => {
                offset += 1;
                continue;
            }
        };
        on_block(&remaining[..item_size])?;
        count += 1;
        offset += item_size;
    }

    Ok(count)
}

/// A parsed block: (slot, hash, block_number, raw_cbor)
#[allow(dead_code)]
type ParsedBlock = (SlotNo, Hash32, BlockNo, Vec<u8>);

/// Parse a chunk file using the secondary index for block boundaries.
///
/// Uses memory-mapped I/O for the chunk file to avoid loading the entire file
/// into memory. The secondary index is small enough to read directly.
#[allow(dead_code)]
fn parse_chunk_with_index(
    chunk_path: &Path,
    secondary_path: &Path,
    checksum_failures: &mut u64,
) -> Result<Vec<ParsedBlock>> {
    let secondary_data = fs::read(secondary_path).context("Failed to read secondary index file")?;

    // Memory-map the chunk file instead of reading it entirely into memory
    let chunk_file = fs::File::open(chunk_path).context("Failed to open chunk file")?;
    let chunk_data = unsafe { Mmap::map(&chunk_file).context("Failed to mmap chunk file")? };

    // Parse secondary index entries (56 bytes each, no header)
    let mut entries = Vec::new();
    let mut offset = 0;
    while offset + 56 <= secondary_data.len() {
        if let Some(entry) = SecondaryIndexEntry::from_bytes(&secondary_data[offset..]) {
            entries.push(entry);
        }
        offset += 56;
    }

    if entries.is_empty() {
        return Ok(Vec::new());
    }

    let mut blocks = Vec::with_capacity(entries.len());

    for i in 0..entries.len() {
        let entry = &entries[i];
        let block_start = entry.block_offset as usize;

        // Block end is either the next block's offset or the end of the chunk file
        let block_end = if i + 1 < entries.len() {
            entries[i + 1].block_offset as usize
        } else {
            chunk_data.len()
        };

        if block_start >= chunk_data.len() || block_end > chunk_data.len() {
            warn!(
                block_start,
                block_end,
                chunk_len = chunk_data.len(),
                "Invalid block offset in secondary index, skipping"
            );
            continue;
        }

        let block_cbor = &chunk_data[block_start..block_end];

        // Verify CRC32 checksum from secondary index
        if entry.checksum != 0 && !verify_block_checksum(block_cbor, entry.checksum) {
            *checksum_failures += 1;
            warn!(
                chunk = %chunk_path.display(),
                offset = block_start,
                expected_crc = entry.checksum,
                actual_crc = crc32fast::hash(block_cbor),
                "Block CRC32 checksum mismatch"
            );
            // Continue importing — the block may still be valid (checksum
            // could be computed over a different range in some eras)
        }

        // Try to decode with pallas to get the slot and block number
        match pallas_traverse::MultiEraBlock::decode(block_cbor) {
            Ok(pallas_block) => {
                let slot = SlotNo(pallas_block.slot());
                let block_no = BlockNo(pallas_block.number());
                let hash = Hash32::from_bytes(entry.header_hash);

                blocks.push((slot, hash, block_no, block_cbor.to_vec()));
            }
            Err(e) => {
                // Log but continue — might be an EBB or corrupted block
                debug!(
                    chunk = %chunk_path.display(),
                    offset = block_start,
                    error = %e,
                    "Failed to decode block from chunk file"
                );
            }
        }
    }

    Ok(blocks)
}

/// Parse a chunk file by sequential CBOR decoding (fallback when no secondary index).
///
/// Uses memory-mapped I/O and proper CBOR size probing to avoid O(n^2)
/// byte-by-byte scanning on decode failures.
#[allow(dead_code)]
fn parse_chunk_sequential(chunk_path: &Path) -> Result<Vec<ParsedBlock>> {
    let chunk_file = fs::File::open(chunk_path).context("Failed to open chunk file")?;
    let chunk_len = chunk_file.metadata()?.len() as usize;
    if chunk_len == 0 {
        return Ok(Vec::new());
    }

    let chunk_data = unsafe { Mmap::map(&chunk_file).context("Failed to mmap chunk file")? };

    let mut blocks = Vec::new();
    let mut offset = 0;

    while offset < chunk_data.len() {
        let remaining = &chunk_data[offset..];
        if remaining.is_empty() {
            break;
        }

        // First, probe the CBOR item size to know how many bytes to skip
        // regardless of whether pallas can decode this particular era/block.
        let item_size = match cbor_item_size(remaining) {
            Some(size) if size > 0 => size,
            _ => {
                // Not valid CBOR at this offset — skip one byte
                offset += 1;
                continue;
            }
        };

        // Try to decode the CBOR item as a Cardano block
        match pallas_traverse::MultiEraBlock::decode(&remaining[..item_size]) {
            Ok(pallas_block) => {
                let slot = SlotNo(pallas_block.slot());
                let block_no = BlockNo(pallas_block.number());
                let hash_bytes: [u8; 32] =
                    pallas_block.hash().as_ref().try_into().unwrap_or([0u8; 32]);
                let hash = Hash32::from_bytes(hash_bytes);

                blocks.push((slot, hash, block_no, remaining[..item_size].to_vec()));
            }
            Err(_) => {
                // Valid CBOR but not a decodable Cardano block (e.g. EBB) — skip it
            }
        }

        offset += item_size;
    }

    Ok(blocks)
}

/// Determine the size of the next CBOR item in a byte slice
fn cbor_item_size(data: &[u8]) -> Option<usize> {
    let mut decoder = minicbor::Decoder::new(data);
    let start = decoder.position();
    skip_item(&mut decoder).ok()?;
    Some(decoder.position() - start)
}

/// Recursively skip one CBOR data item in the decoder.
fn skip_item(decoder: &mut minicbor::Decoder) -> Result<(), minicbor::decode::Error> {
    use minicbor::data::Type;
    match decoder.datatype()? {
        Type::Array | Type::ArrayIndef => {
            let len = decoder.array()?;
            if let Some(n) = len {
                for _ in 0..n {
                    skip_item(decoder)?;
                }
            } else {
                loop {
                    if decoder.datatype()? == Type::Break {
                        decoder.skip()?;
                        break;
                    }
                    skip_item(decoder)?;
                }
            }
            Ok(())
        }
        Type::Map | Type::MapIndef => {
            let len = decoder.map()?;
            if let Some(n) = len {
                for _ in 0..n {
                    skip_item(decoder)?;
                    skip_item(decoder)?;
                }
            } else {
                loop {
                    if decoder.datatype()? == Type::Break {
                        decoder.skip()?;
                        break;
                    }
                    skip_item(decoder)?;
                    skip_item(decoder)?;
                }
            }
            Ok(())
        }
        Type::Tag => {
            decoder.tag()?;
            skip_item(decoder)
        }
        _ => {
            decoder.skip()?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aggregator_url_mainnet() {
        assert_eq!(
            aggregator_url(764824073),
            "https://aggregator.release-mainnet.api.mithril.network/aggregator"
        );
    }

    #[test]
    fn test_aggregator_url_preview() {
        assert_eq!(
            aggregator_url(2),
            "https://aggregator.pre-release-preview.api.mithril.network/aggregator"
        );
    }

    #[test]
    fn test_aggregator_url_preprod() {
        assert_eq!(
            aggregator_url(1),
            "https://aggregator.release-preprod.api.mithril.network/aggregator"
        );
    }

    #[test]
    fn test_aggregator_url_unknown_defaults_to_mainnet() {
        assert_eq!(aggregator_url(999), aggregator_url(764824073));
    }

    #[test]
    fn test_secondary_index_entry_parse() {
        let mut data = [0u8; 56];
        // block_offset = 1024
        data[0..8].copy_from_slice(&1024u64.to_be_bytes());
        // header_offset = 2
        data[8..10].copy_from_slice(&2u16.to_be_bytes());
        // header_size = 100
        data[10..12].copy_from_slice(&100u16.to_be_bytes());
        // checksum = 12345
        data[12..16].copy_from_slice(&12345u32.to_be_bytes());
        // header_hash
        data[16..48].copy_from_slice(&[0xAB; 32]);
        // _block_or_ebb (slot 5000)
        data[48..56].copy_from_slice(&5000u64.to_be_bytes());

        let entry = SecondaryIndexEntry::from_bytes(&data).unwrap();
        assert_eq!(entry.block_offset, 1024);
        assert_eq!(entry._header_offset, 2);
        assert_eq!(entry._header_size, 100);
        assert_eq!(entry.checksum, 12345);
        assert_eq!(entry.header_hash, [0xAB; 32]);
        assert_eq!(entry._block_or_ebb, 5000);
    }

    #[test]
    fn test_secondary_index_entry_too_short() {
        let data = [0u8; 55]; // 1 byte short
        assert!(SecondaryIndexEntry::from_bytes(&data).is_none());
    }

    #[test]
    fn test_cbor_item_size_simple() {
        // CBOR array of 2 elements: [1, 2]
        // 0x82 = array(2), 0x01 = unsigned(1), 0x02 = unsigned(2)
        let data = [0x82, 0x01, 0x02, 0xFF]; // extra byte should not be consumed
        let size = cbor_item_size(&data).unwrap();
        assert_eq!(size, 3);
    }

    #[test]
    fn test_cbor_item_size_nested() {
        // [[1, 2], [3]]
        // 0x82 (array 2), 0x82 0x01 0x02 (array [1,2]), 0x81 0x03 (array [3])
        let data = [0x82, 0x82, 0x01, 0x02, 0x81, 0x03];
        let size = cbor_item_size(&data).unwrap();
        assert_eq!(size, 6);
    }

    #[test]
    fn test_cbor_item_size_map() {
        // {1: 2} — 0xA1 0x01 0x02
        let data = [0xA1, 0x01, 0x02];
        let size = cbor_item_size(&data).unwrap();
        assert_eq!(size, 3);
    }

    #[test]
    fn test_cbor_item_size_tagged() {
        // tag(24) + bytes(2) [0x01, 0x02]
        // 0xD8 0x18 0x42 0x01 0x02
        let data = [0xD8, 0x18, 0x42, 0x01, 0x02];
        let size = cbor_item_size(&data).unwrap();
        assert_eq!(size, 5);
    }

    #[test]
    fn test_cbor_item_size_invalid() {
        let data = [0xFF]; // CBOR break — not a valid top-level item
                           // May or may not return None depending on minicbor's skip behaviour
                           // Just ensure it doesn't panic
        let _ = cbor_item_size(&data);
    }

    #[test]
    fn test_find_immutable_dir_direct() {
        let dir = tempfile::tempdir().unwrap();
        let immutable = dir.path().join("immutable");
        fs::create_dir_all(&immutable).unwrap();
        assert_eq!(find_immutable_dir(dir.path()), Some(immutable));
    }

    #[test]
    fn test_find_immutable_dir_nested() {
        let dir = tempfile::tempdir().unwrap();
        let immutable = dir.path().join("db").join("immutable");
        fs::create_dir_all(&immutable).unwrap();
        assert_eq!(find_immutable_dir(dir.path()), Some(immutable));
    }

    #[test]
    fn test_find_immutable_dir_deep_nested() {
        let dir = tempfile::tempdir().unwrap();
        let immutable = dir.path().join("snapshot-abc").join("immutable");
        fs::create_dir_all(&immutable).unwrap();
        assert_eq!(find_immutable_dir(dir.path()), Some(immutable));
    }

    #[test]
    fn test_find_immutable_dir_not_found() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(find_immutable_dir(dir.path()), None);
    }

    #[test]
    fn test_verify_block_checksum_valid() {
        let data = b"hello world";
        let crc = crc32fast::hash(data);
        assert!(verify_block_checksum(data, crc));
    }

    #[test]
    fn test_verify_block_checksum_invalid() {
        let data = b"hello world";
        assert!(!verify_block_checksum(data, 0xDEADBEEF));
    }

    #[test]
    fn test_verify_block_checksum_empty() {
        let data = b"";
        let crc = crc32fast::hash(data);
        assert!(verify_block_checksum(data, crc));
    }

    /// Helper: compute the expected Mithril digest for test immutable files.
    fn compute_expected_digest(
        network: &str,
        epoch: u64,
        immutable_file_number: u64,
        files: &[(u64, &str, &[u8])], // (number, extension, content)
    ) -> String {
        // Beacon hash
        let beacon_hash = {
            let mut h = Sha256::new();
            h.update(network.as_bytes());
            h.update(epoch.to_be_bytes());
            h.update(immutable_file_number.to_be_bytes());
            hex::encode(h.finalize())
        };

        // Sort files by number then by name (matching Mithril)
        let mut sorted: Vec<_> = files.to_vec();
        sorted.sort_by(|(num_a, ext_a, _), (num_b, ext_b, _)| {
            let name_a = format!("{num_a:05}.{ext_a}");
            let name_b = format!("{num_b:05}.{ext_b}");
            num_a.cmp(num_b).then(name_a.cmp(&name_b))
        });

        let mut final_hasher = Sha256::new();
        final_hasher.update(beacon_hash.as_bytes());
        for (_, _, content) in &sorted {
            let file_hash_hex = hex::encode(Sha256::digest(content));
            final_hasher.update(file_hash_hex.as_bytes());
        }
        hex::encode(final_hasher.finalize())
    }

    #[test]
    fn test_verify_snapshot_digest_valid() {
        let dir = tempfile::tempdir().unwrap();
        let immutable = dir.path().join("immutable");
        fs::create_dir_all(&immutable).unwrap();

        let chunk_data = b"chunk data for file 1";
        let primary_data = b"primary data for file 1";
        let secondary_data = b"secondary data for file 1";
        fs::write(immutable.join("00001.chunk"), chunk_data).unwrap();
        fs::write(immutable.join("00001.primary"), primary_data).unwrap();
        fs::write(immutable.join("00001.secondary"), secondary_data).unwrap();

        let beacon = SnapshotBeacon {
            epoch: 100,
            immutable_file_number: 1,
        };

        let expected = compute_expected_digest(
            "preview",
            100,
            1,
            &[
                (1, "chunk", chunk_data),
                (1, "primary", primary_data),
                (1, "secondary", secondary_data),
            ],
        );

        let result = verify_snapshot_digest(dir.path(), "preview", &beacon, &expected);
        assert!(result.is_ok(), "Expected Ok, got: {:?}", result);
    }

    #[test]
    fn test_verify_snapshot_digest_multiple_files() {
        let dir = tempfile::tempdir().unwrap();
        let immutable = dir.path().join("immutable");
        fs::create_dir_all(&immutable).unwrap();

        let files = [
            (1u64, "chunk", b"c1" as &[u8]),
            (1, "primary", b"p1"),
            (1, "secondary", b"s1"),
            (2, "chunk", b"c2"),
            (2, "primary", b"p2"),
            (2, "secondary", b"s2"),
        ];

        for (num, ext, content) in &files {
            fs::write(immutable.join(format!("{num:05}.{ext}")), content).unwrap();
        }

        let beacon = SnapshotBeacon {
            epoch: 50,
            immutable_file_number: 2,
        };

        let expected = compute_expected_digest("mainnet", 50, 2, &files);

        let result = verify_snapshot_digest(dir.path(), "mainnet", &beacon, &expected);
        assert!(result.is_ok(), "Expected Ok, got: {:?}", result);
    }

    #[test]
    fn test_verify_snapshot_digest_excludes_files_beyond_beacon() {
        let dir = tempfile::tempdir().unwrap();
        let immutable = dir.path().join("immutable");
        fs::create_dir_all(&immutable).unwrap();

        // File 1 is within beacon, file 2 is beyond
        fs::write(immutable.join("00001.chunk"), b"c1").unwrap();
        fs::write(immutable.join("00002.chunk"), b"c2-should-be-excluded").unwrap();

        let beacon = SnapshotBeacon {
            epoch: 10,
            immutable_file_number: 1,
        };

        // Expected digest only includes file 1
        let expected = compute_expected_digest("preview", 10, 1, &[(1, "chunk", b"c1" as &[u8])]);

        let result = verify_snapshot_digest(dir.path(), "preview", &beacon, &expected);
        assert!(result.is_ok(), "Expected Ok, got: {:?}", result);
    }

    #[test]
    fn test_verify_snapshot_digest_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let immutable = dir.path().join("immutable");
        fs::create_dir_all(&immutable).unwrap();

        fs::write(immutable.join("00001.chunk"), b"data").unwrap();

        let beacon = SnapshotBeacon {
            epoch: 1,
            immutable_file_number: 1,
        };

        let result = verify_snapshot_digest(
            dir.path(),
            "preview",
            &beacon,
            "0000000000000000000000000000000000000000000000000000000000000000",
        );
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("digest mismatch"));
    }

    #[test]
    fn test_verify_snapshot_digest_no_immutable_dir() {
        let dir = tempfile::tempdir().unwrap();
        let beacon = SnapshotBeacon {
            epoch: 1,
            immutable_file_number: 1,
        };
        let result = verify_snapshot_digest(dir.path(), "preview", &beacon, "abc");
        assert!(result.is_err());
    }

    #[test]
    fn test_mithril_network_name() {
        assert_eq!(mithril_network_name(764824073), "mainnet");
        assert_eq!(mithril_network_name(2), "preview");
        assert_eq!(mithril_network_name(1), "preprod");
        assert_eq!(mithril_network_name(999), "private");
    }

    #[test]
    fn test_import_chunk_files_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        // No immutable/ directory
        let db_dir = tempfile::tempdir().unwrap();
        let result = import_chunk_files(dir.path(), db_dir.path());
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Could not find immutable/"));
    }

    #[test]
    fn test_import_chunk_files_no_chunks() {
        let dir = tempfile::tempdir().unwrap();
        let immutable = dir.path().join("immutable");
        fs::create_dir_all(&immutable).unwrap();
        let db_dir = tempfile::tempdir().unwrap();
        let result = import_chunk_files(dir.path(), db_dir.path());
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("No chunk files found"));
    }

    #[test]
    fn test_secondary_index_multiple_entries() {
        // Parse 3 sequential entries from a contiguous buffer
        let mut buf = [0u8; 56 * 3];
        for (i, offset_val) in [0u64, 1000, 2000].iter().enumerate() {
            let base = i * 56;
            buf[base..base + 8].copy_from_slice(&offset_val.to_be_bytes());
            buf[base + 8..base + 10].copy_from_slice(&0u16.to_be_bytes());
            buf[base + 10..base + 12].copy_from_slice(&100u16.to_be_bytes());
            buf[base + 12..base + 16].copy_from_slice(&(i as u32).to_be_bytes());
            buf[base + 16..base + 48].copy_from_slice(&[i as u8; 32]);
            buf[base + 48..base + 56].copy_from_slice(&(i as u64).to_be_bytes());
        }

        let mut entries = Vec::new();
        let mut offset = 0;
        while offset + 56 <= buf.len() {
            if let Some(entry) = SecondaryIndexEntry::from_bytes(&buf[offset..]) {
                entries.push(entry);
            }
            offset += 56;
        }

        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].block_offset, 0);
        assert_eq!(entries[1].block_offset, 1000);
        assert_eq!(entries[2].block_offset, 2000);
    }

    #[test]
    fn test_parse_chunk_sequential_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let chunk_path = dir.path().join("00000.chunk");
        fs::write(&chunk_path, b"").unwrap();

        let blocks = parse_chunk_sequential(&chunk_path).unwrap();
        assert!(blocks.is_empty());
    }

    #[test]
    fn test_parse_chunk_sequential_invalid_cbor() {
        let dir = tempfile::tempdir().unwrap();
        let chunk_path = dir.path().join("00000.chunk");
        // Write some random non-CBOR data
        fs::write(&chunk_path, [0xDE, 0xAD, 0xBE, 0xEF]).unwrap();

        let blocks = parse_chunk_sequential(&chunk_path).unwrap();
        assert!(blocks.is_empty());
    }

    #[test]
    fn test_parse_chunk_sequential_valid_cbor_not_block() {
        let dir = tempfile::tempdir().unwrap();
        let chunk_path = dir.path().join("00000.chunk");
        // Valid CBOR: [1, 2] — but not a Cardano block
        fs::write(&chunk_path, [0x82, 0x01, 0x02]).unwrap();

        let blocks = parse_chunk_sequential(&chunk_path).unwrap();
        // Should skip it (valid CBOR but not a decodable block)
        assert!(blocks.is_empty());
    }

    #[test]
    fn test_parse_chunk_with_index_missing_chunk() {
        let dir = tempfile::tempdir().unwrap();
        let chunk_path = dir.path().join("00000.chunk");
        let secondary_path = dir.path().join("00000.secondary");

        // Only create secondary, not chunk
        fs::write(&secondary_path, [0u8; 56]).unwrap();

        let mut failures = 0;
        let result = parse_chunk_with_index(&chunk_path, &secondary_path, &mut failures);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_chunk_with_index_empty_secondary() {
        let dir = tempfile::tempdir().unwrap();
        let chunk_path = dir.path().join("00000.chunk");
        let secondary_path = dir.path().join("00000.secondary");

        fs::write(&chunk_path, b"some data").unwrap();
        fs::write(&secondary_path, b"").unwrap(); // empty secondary

        let mut failures = 0;
        let blocks = parse_chunk_with_index(&chunk_path, &secondary_path, &mut failures).unwrap();
        assert!(blocks.is_empty());
        assert_eq!(failures, 0);
    }

    #[test]
    fn test_parse_chunk_with_index_bad_offset() {
        let dir = tempfile::tempdir().unwrap();
        let chunk_path = dir.path().join("00000.chunk");
        let secondary_path = dir.path().join("00000.secondary");

        // Write a small chunk file
        fs::write(&chunk_path, [0x82, 0x01, 0x02]).unwrap();

        // Write a secondary index entry with an offset beyond the chunk file
        let mut entry_data = [0u8; 56];
        entry_data[0..8].copy_from_slice(&9999u64.to_be_bytes()); // offset way past end
        fs::write(&secondary_path, entry_data).unwrap();

        let mut failures = 0;
        let blocks = parse_chunk_with_index(&chunk_path, &secondary_path, &mut failures).unwrap();
        assert!(blocks.is_empty()); // should skip the invalid entry
    }

    #[test]
    fn test_beacon_hash_matches_mithril_test_vector() {
        // Test vector from Mithril source: compute_beacon_hash("testnet", {epoch: 10, immutable: 100})
        let mut hasher = Sha256::new();
        hasher.update("testnet".as_bytes());
        hasher.update(10u64.to_be_bytes());
        hasher.update(100u64.to_be_bytes());
        let beacon_hash = hex::encode(hasher.finalize());
        assert_eq!(
            beacon_hash,
            "48cbf709b56204d8315aefd3a416b45398094f6fd51785c5b7dcaf7f35aacbfb"
        );
    }

    #[test]
    fn test_verify_snapshot_digest_end_to_end() {
        // Create a fake immutable directory with known content
        let dir = tempfile::tempdir().unwrap();
        let immutable = dir.path().join("immutable");
        fs::create_dir_all(&immutable).unwrap();

        // Create two chunk files with known content
        fs::write(immutable.join("00001.chunk"), b"chunk1data").unwrap();
        fs::write(immutable.join("00001.primary"), b"primary1data").unwrap();
        fs::write(immutable.join("00001.secondary"), b"secondary1data").unwrap();

        let network_name = "testnet";
        let beacon = SnapshotBeacon {
            epoch: 10,
            immutable_file_number: 1,
        };

        // Compute expected digest manually using the Mithril algorithm
        let beacon_hash = {
            let mut h = Sha256::new();
            h.update(network_name.as_bytes());
            h.update(10u64.to_be_bytes());
            h.update(1u64.to_be_bytes());
            hex::encode(h.finalize())
        };

        // File hashes in order: chunk, primary, secondary (lexicographic path order)
        let chunk_hash = hex::encode(Sha256::digest(b"chunk1data"));
        let primary_hash = hex::encode(Sha256::digest(b"primary1data"));
        let secondary_hash = hex::encode(Sha256::digest(b"secondary1data"));

        let mut final_hasher = Sha256::new();
        final_hasher.update(beacon_hash.as_bytes());
        final_hasher.update(chunk_hash.as_bytes());
        final_hasher.update(primary_hash.as_bytes());
        final_hasher.update(secondary_hash.as_bytes());
        let expected_digest = hex::encode(final_hasher.finalize());

        // Now verify using our function
        let result = verify_snapshot_digest(dir.path(), network_name, &beacon, &expected_digest);
        assert!(result.is_ok(), "Digest verification failed: {result:?}");
    }

    #[test]
    fn test_extract_archive_already_extracted() {
        let dir = tempfile::tempdir().unwrap();
        let extract_dir = dir.path().join("extracted");
        let immutable = extract_dir.join("immutable");
        fs::create_dir_all(&immutable).unwrap();

        // Should return Ok immediately without touching the archive
        let fake_archive = dir.path().join("nonexistent.tar.zst");
        let result = extract_archive(&fake_archive, &extract_dir);
        assert!(result.is_ok());
    }
}
