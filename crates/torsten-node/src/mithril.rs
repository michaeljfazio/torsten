//! Mithril snapshot import for fast initial sync.
//!
//! Downloads a Mithril-certified snapshot of the Cardano immutable DB,
//! extracts the cardano-node chunk files, parses blocks with pallas,
//! and bulk-imports them into Torsten's RocksDB-based ImmutableDB.

use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
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

/// Snapshot metadata from the Mithril aggregator API
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

#[derive(Debug, serde::Deserialize)]
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

/// Entry from a cardano-node secondary index file.
/// Each entry is 56 bytes in the secondary index.
#[derive(Debug, Clone)]
struct SecondaryIndexEntry {
    block_offset: u64,
    _header_offset: u16,
    _header_size: u16,
    _checksum: u32,
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
        let _block_or_ebb = u64::from_be_bytes(data[48..56].try_into().ok()?);

        Some(SecondaryIndexEntry {
            block_offset,
            _header_offset: header_offset,
            _header_size: header_size,
            _checksum: checksum,
            header_hash,
            _block_or_ebb,
        })
    }
}

/// Get the aggregator URL for a given network magic
pub fn aggregator_url(network_magic: u64) -> &'static str {
    match network_magic {
        764824073 => MAINNET_AGGREGATOR,
        2 => PREVIEW_AGGREGATOR,
        1 => PREPROD_AGGREGATOR,
        _ => MAINNET_AGGREGATOR,
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

    // Step 5: Import blocks into RocksDB
    info!(
        database_path = %database_path.display(),
        "Importing blocks into ImmutableDB"
    );
    import_chunk_files(&extract_dir, database_path)?;

    // Step 6: Cleanup
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

/// Import cardano-node immutable chunk files into Torsten's RocksDB
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

    // Open the database
    let immutable_path = database_path.join("immutable");
    let mut immutable_db = torsten_storage::immutable_db::ImmutableDB::open(&immutable_path)?;

    // Check if we already have blocks (resume support)
    let existing_tip = immutable_db.tip_slot();
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

    for chunk_num in &chunk_numbers {
        let chunk_path = immutable_dir.join(format!("{chunk_num:05}.chunk"));
        let secondary_path = immutable_dir.join(format!("{chunk_num:05}.secondary"));

        let blocks = if secondary_path.exists() {
            parse_chunk_with_index(&chunk_path, &secondary_path)?
        } else {
            // Fallback: parse chunk file by sequential CBOR decoding
            parse_chunk_sequential(&chunk_path)?
        };

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

        immutable_db.put_blocks_batch(&batch)?;
        total_blocks_imported += batch.len() as u64;

        pb.inc(1);
        if *chunk_num % 100 == 0 {
            debug!(
                chunk = chunk_num,
                total_imported = total_blocks_imported,
                "Import progress"
            );
        }
    }

    pb.finish_with_message("Import complete");
    info!(
        total_blocks = total_blocks_imported,
        skipped_chunks,
        tip_slot = immutable_db.tip_slot().0,
        "Block import complete"
    );

    Ok(())
}

/// A parsed block: (slot, hash, block_number, raw_cbor)
type ParsedBlock = (SlotNo, Hash32, BlockNo, Vec<u8>);

/// Parse a chunk file using the secondary index for block boundaries
fn parse_chunk_with_index(chunk_path: &Path, secondary_path: &Path) -> Result<Vec<ParsedBlock>> {
    let secondary_data = fs::read(secondary_path).context("Failed to read secondary index file")?;
    let chunk_data = fs::read(chunk_path).context("Failed to read chunk file")?;

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

/// Parse a chunk file by sequential CBOR decoding (fallback when no secondary index)
fn parse_chunk_sequential(chunk_path: &Path) -> Result<Vec<ParsedBlock>> {
    let chunk_data = fs::read(chunk_path).context("Failed to read chunk file")?;
    if chunk_data.is_empty() {
        return Ok(Vec::new());
    }

    let mut blocks = Vec::new();
    let mut offset = 0;

    while offset < chunk_data.len() {
        // Try to find the next CBOR array start
        // Cardano blocks are encoded as CBOR arrays: tag 0x82 (2-element array) for multi-era wrapper
        let remaining = &chunk_data[offset..];
        if remaining.is_empty() {
            break;
        }

        // Try to decode starting from current offset
        match pallas_traverse::MultiEraBlock::decode(remaining) {
            Ok(pallas_block) => {
                let slot = SlotNo(pallas_block.slot());
                let block_no = BlockNo(pallas_block.number());
                let hash_bytes: [u8; 32] =
                    pallas_block.hash().as_ref().try_into().unwrap_or([0u8; 32]);
                let hash = Hash32::from_bytes(hash_bytes);

                // We need to figure out how many bytes this block consumed
                // Use minicbor to determine the CBOR item size
                let size = cbor_item_size(remaining).unwrap_or(remaining.len());
                let block_cbor = remaining[..size].to_vec();

                blocks.push((slot, hash, block_no, block_cbor));
                offset += size;
            }
            Err(_) => {
                // Skip to next byte and try again
                offset += 1;
            }
        }
    }

    Ok(blocks)
}

/// Determine the size of the next CBOR item in a byte slice
fn cbor_item_size(data: &[u8]) -> Option<usize> {
    // Use minicbor's decoder to probe the size
    let mut decoder = minicbor::Decoder::new(data);

    // Save position, skip one item, return consumed bytes
    let start = decoder.position();

    // Try to skip a CBOR data item
    // For a top-level array (which Cardano blocks are), we need to skip the entire array
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

    skip_item(&mut decoder).ok()?;
    Some(decoder.position() - start)
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
        assert_eq!(entry._checksum, 12345);
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
    fn test_find_immutable_dir_not_found() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(find_immutable_dir(dir.path()), None);
    }
}
