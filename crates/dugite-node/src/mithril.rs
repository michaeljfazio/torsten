//! Mithril snapshot import for fast initial sync.
//!
//! Downloads a Mithril-certified snapshot of the Cardano immutable DB,
//! extracts the cardano-node chunk files, parses blocks with pallas,
//! and bulk-imports them into Dugite's ImmutableDB (chunk files).
//!
//! Supports both the legacy `/artifact/snapshots` API and the newer
//! `/artifact/cardano-database` API with per-immutable-file downloads.

use anyhow::{Context, Result};
#[cfg(test)]
use dugite_primitives::hash::Hash32;
#[cfg(test)]
use dugite_primitives::time::{BlockNo, SlotNo};
#[allow(unused_imports)]
use ed25519_dalek::{Signature, VerifyingKey};
use indicatif::{ProgressBar, ProgressStyle};
use memmap2::Mmap;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};
#[cfg(test)]
use tracing::debug;
use tracing::{info, warn};

/// Mithril aggregator endpoints per network
const MAINNET_AGGREGATOR: &str =
    "https://aggregator.release-mainnet.api.mithril.network/aggregator";
const PREVIEW_AGGREGATOR: &str =
    "https://aggregator.pre-release-preview.api.mithril.network/aggregator";
const PREPROD_AGGREGATOR: &str =
    "https://aggregator.release-preprod.api.mithril.network/aggregator";

// ---------------------------------------------------------------------------
// Mithril genesis verification keys (from mithril-infra/configuration/)
// ---------------------------------------------------------------------------

/// Mainnet genesis verification key (Ed25519, JSON hex-encoded).
/// Source: https://github.com/input-output-hk/mithril/blob/main/mithril-infra/configuration/release-mainnet/genesis.vkey
const MAINNET_GENESIS_VKEY: &str =
    "5b3139312c36362c3134302c3138352c3133382c31312c3233372c3230372c3235302c3134342c32372c322c3138382c33302c31322c38312c3135352c3230342c31302c3137392c37352c32332c3133382c3139362c3231372c352c31342c32302c35372c37392c33392c3137365d";

/// Preview genesis verification key (Ed25519, JSON hex-encoded).
/// Source: https://github.com/input-output-hk/mithril/blob/main/mithril-infra/configuration/pre-release-preview/genesis.vkey
const PREVIEW_GENESIS_VKEY: &str =
    "5b3132372c37332c3132342c3136312c362c3133372c3133312c3231332c3230372c3131372c3139382c38352c3137362c3139392c3136322c3234312c36382c3132332c3131392c3134352c31332c3233322c3234332c34392c3232392c322c3234392c3230352c3230352c33392c3233352c34345d";

/// Preprod genesis verification key (Ed25519, JSON hex-encoded).
/// Same key as preview.
/// Source: https://github.com/input-output-hk/mithril/blob/main/mithril-infra/configuration/release-preprod/genesis.vkey
const PREPROD_GENESIS_VKEY: &str =
    "5b3132372c37332c3132342c3136312c362c3133372c3133312c3231332c3230372c3131372c3139382c38352c3137362c3139392c3136322c3234312c36382c3132332c3131392c3134352c31332c3233322c3234332c34392c3232392c322c3234392c3230352c3230352c33392c3233352c34345d";

/// Get the genesis verification key for a given network magic.
fn genesis_verification_key(network_magic: u64) -> Option<&'static str> {
    match network_magic {
        764824073 => Some(MAINNET_GENESIS_VKEY),
        2 => Some(PREVIEW_GENESIS_VKEY),
        1 => Some(PREPROD_GENESIS_VKEY),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// API response types
// ---------------------------------------------------------------------------

/// Snapshot metadata from the Mithril aggregator API (legacy endpoint)
#[derive(Debug, serde::Deserialize)]
struct SnapshotListItem {
    digest: String,
    certificate_hash: String,
    #[serde(rename = "network")]
    _network: String,
    size: u64,
    #[serde(rename = "beacon")]
    beacon: SnapshotBeacon,
    #[serde(rename = "compression_algorithm", default)]
    _compression_algorithm: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub(crate) struct SnapshotBeacon {
    epoch: u64,
    immutable_file_number: u64,
}

/// Full snapshot detail (includes download locations)
#[derive(Debug, serde::Deserialize)]
struct SnapshotDetail {
    digest: String,
    size: u64,
    #[serde(rename = "beacon")]
    _beacon: SnapshotBeacon,
    locations: Vec<String>,
    #[serde(rename = "compression_algorithm", default)]
    _compression_algorithm: Option<String>,
}

// ---------------------------------------------------------------------------
// Secondary index parsing
// ---------------------------------------------------------------------------

/// Entry from a cardano-node secondary index file.
/// Each entry is 56 bytes in the secondary index.
#[derive(Debug, Clone)]
struct SecondaryIndexEntry {
    block_offset: u64,
    _header_offset: u16,
    _header_size: u16,
    _checksum: u32,
    _header_hash: [u8; 32],
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
            _checksum: checksum,
            _header_hash: header_hash,
            _block_or_ebb: block_or_ebb,
        })
    }
}

/// Verify a block's CRC32 checksum against the secondary index entry.
#[cfg(test)]
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

/// Run the Mithril snapshot import.
///
/// Downloads the latest certified snapshot, extracts the cardano-node
/// immutable chunk files, and bulk-imports them into Dugite's ImmutableDB.
/// The node will rebuild ledger state via block replay on first startup.
pub async fn import_snapshot(
    network_magic: u64,
    database_path: &Path,
    temp_dir: Option<&Path>,
    genesis_vkey_override: Option<&str>,
    skip_verification: bool,
) -> Result<()> {
    let aggregator = aggregator_url(network_magic);
    info!(aggregator = %aggregator, "Fetching latest Mithril snapshot");

    // Step 1: Get latest snapshot metadata
    let client = reqwest::Client::builder()
        .user_agent("dugite-node/0.1")
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
        epoch = latest.beacon.epoch,
        immutable = latest.beacon.immutable_file_number,
        size_gb = format_args!("{:.1}", latest.size as f64 / (1024.0 * 1024.0 * 1024.0)),
        "Mithril snapshot found",
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

    info!("Downloading Mithril snapshot...");

    // Step 3: Download snapshot to temp file
    let work_dir = temp_dir
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::temp_dir().join("dugite-mithril"));
    fs::create_dir_all(&work_dir)?;

    let archive_path = work_dir.join(format!("snapshot-{}.tar.zst", detail.digest));
    download_snapshot(&client, download_url, &archive_path, detail.size).await?;

    // Step 4: Extract the archive
    let extract_dir = work_dir.join(format!("extract-{}", detail.digest));
    info!("Extracting Mithril snapshot archive...");
    extract_archive(&archive_path, &extract_dir)?;

    // Step 5: Verify snapshot digest (Mithril digest over extracted immutable files)
    let network_name = mithril_network_name(network_magic);
    verify_snapshot_digest(&extract_dir, network_name, &latest.beacon, &detail.digest)?;

    // Step 5b: Verify the Mithril STM certificate chain.
    //
    // This cryptographically proves that ≥ 2/3 of Cardano stake signed this
    // snapshot by walking the certificate chain back to the genesis certificate
    // and verifying each STM multi-signature.
    if skip_verification {
        warn!(
            "Mithril STM certificate chain verification SKIPPED (--skip-certificate-verification). \
             The snapshot is trusted without cryptographic proof. \
             Do NOT use this in production."
        );
    } else {
        let genesis_vkey = genesis_vkey_override
            .or_else(|| genesis_verification_key(network_magic))
            .context(
                "No Mithril genesis verification key for this network. \
                 Use --mithril-genesis-vkey to provide one for private networks.",
            )?;

        info!("Verifying Mithril STM certificate chain...");

        let mithril = mithril_client::ClientBuilder::new(
            mithril_client::AggregatorDiscoveryType::Url(aggregator.to_string()),
        )
        .set_genesis_verification_key(mithril_client::GenesisVerificationKey::JsonHex(
            genesis_vkey.to_string(),
        ))
        .build()
        .context("Failed to build Mithril client")?;

        // Verify the full certificate chain from the snapshot's certificate
        // back to the genesis certificate. Each certificate's STM multi-signature
        // is verified against the aggregate verification key, and the genesis
        // certificate's Ed25519 signature is verified against the hardcoded key.
        let certificate = mithril
            .certificate()
            .verify_chain(&latest.certificate_hash)
            .await
            .context("Mithril certificate chain verification FAILED — snapshot rejected")?;

        info!(
            certificate_hash = %latest.certificate_hash,
            epoch = %certificate.epoch,
            "Certificate chain verified"
        );

        // Verify that the extracted snapshot content matches what the Mithril
        // signers actually certified. This re-hashes all immutable files and
        // checks the digest against the certificate's signed message.
        // On mainnet this can take 10-30+ minutes depending on disk speed.
        let file_count = find_immutable_dir(&extract_dir)
            .and_then(|d| fs::read_dir(d).ok())
            .map(|entries| entries.count())
            .unwrap_or(0);
        info!(
            files = file_count,
            "Verifying snapshot content against certificate (hashing all immutable files)..."
        );

        // Spawn a periodic heartbeat so the user knows the process isn't stuck.
        let heartbeat = tokio::spawn(async {
            let start = std::time::Instant::now();
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            interval.tick().await; // consume the immediate first tick
            loop {
                interval.tick().await;
                let elapsed = start.elapsed();
                info!(
                    elapsed_secs = elapsed.as_secs(),
                    "Still hashing immutable files ({:.0}s elapsed)...",
                    elapsed.as_secs() as f64
                );
            }
        });

        let message = mithril_client::MessageBuilder::new()
            .compute_snapshot_message(&certificate, &extract_dir)
            .await
            .context("Failed to compute snapshot message from extracted files")?;

        heartbeat.abort();

        if !certificate.match_message(&message) {
            anyhow::bail!(
                "Mithril snapshot content does not match the certified message. \
                 The snapshot may have been tampered with after signing."
            );
        }

        info!("Snapshot content verified against certificate");
    }

    // Step 6: Skip ChainDB import — chunk files are the optimal format for sequential
    // replay. The old LSM import (parse → write 5 KV pairs per block → compaction) was
    // redundant since replay reads from chunk files directly. Blocks will be imported
    // into ChainDB during normal sync after replay completes.

    // Step 7: Move immutable chunk files to permanent storage.
    // These become the ImmutableDB — ChainDB reads historical blocks directly
    // from chunk files (1x write amplification, sequential I/O). The directory
    // is NOT deleted after replay; it serves as permanent immutable block storage.
    let immutable_dir = find_immutable_dir(&extract_dir);
    let dest_dir = database_path.join("immutable");
    if let Some(ref imm) = immutable_dir {
        info!("Moving chunk files to permanent storage");
        if let Err(e) = fs::rename(imm, &dest_dir) {
            // rename may fail across filesystems, fall back to copy
            warn!(error = %e, "rename failed, falling back to copy");
            copy_dir_recursive(imm, &dest_dir)?;
        }
    }

    // Step 7b: Download ancillary archive (Haskell ledger state + next immutable trio).
    //
    // The ancillary archive contains the serialised Haskell ledger state from the
    // same snapshot epoch.  If available it is placed at database_path/haskell-ledger/
    // so that Task 11 (Node::new) can deserialise it directly, skipping the full
    // block-replay path.  Ancillary download is NON-FATAL: if it fails the node
    // falls back to full replay from genesis (original behaviour).
    info!("Downloading ancillary archive (Haskell ledger state)...");
    match download_ancillary(aggregator, network_magic, database_path, &work_dir).await {
        Ok(ancillary_dir) => {
            // Move ledger/ to database_path/haskell-ledger/
            let haskell_ledger_dir = database_path.join("haskell-ledger");
            if haskell_ledger_dir.exists() {
                if let Err(e) = fs::remove_dir_all(&haskell_ledger_dir) {
                    warn!(error = %e, "Failed to remove old haskell-ledger directory");
                }
            }

            let ancillary_ledger = ancillary_dir.join("ledger");
            if ancillary_ledger.exists() {
                // Prefer rename (zero-copy, same filesystem); fall back to recursive
                // copy when the temp dir and database live on different mounts.
                if let Err(e) = fs::rename(&ancillary_ledger, &haskell_ledger_dir) {
                    warn!(error = %e, "rename of ledger/ failed, falling back to copy");
                    copy_dir_recursive(&ancillary_ledger, &haskell_ledger_dir)?;
                    fs::remove_dir_all(&ancillary_ledger)?;
                }
                info!(
                    path = %haskell_ledger_dir.display(),
                    "Haskell ledger state saved"
                );
            } else {
                warn!(
                    path = %ancillary_dir.display(),
                    "Ancillary archive extracted but contained no ledger/ directory"
                );
            }

            // Also absorb the next-immutable trio (the partial chunk at the tip)
            // if the ancillary archive included one.
            let ancillary_immutable = ancillary_dir.join("immutable");
            if ancillary_immutable.exists() {
                let immutable_dest = database_path.join("immutable");
                fs::create_dir_all(&immutable_dest)?;
                let mut moved = 0u32;
                for entry in fs::read_dir(&ancillary_immutable)
                    .into_iter()
                    .flatten()
                    .flatten()
                {
                    let dest = immutable_dest.join(entry.file_name());
                    if let Err(e) = fs::rename(entry.path(), &dest) {
                        // Cross-filesystem fallback
                        if let Err(e2) = fs::copy(entry.path(), &dest) {
                            warn!(
                                error = %e,
                                copy_error = %e2,
                                "Failed to move ancillary immutable file"
                            );
                        } else {
                            let _ = fs::remove_file(entry.path());
                            moved += 1;
                        }
                    } else {
                        moved += 1;
                    }
                }
                if moved > 0 {
                    info!(files = moved, "Moved ancillary immutable files");
                }
            }

            // Clean up the ancillary extract directory (ledger/ was renamed above;
            // any remaining artefacts can be removed).
            if let Err(e) = fs::remove_dir_all(&ancillary_dir) {
                warn!(error = %e, "Failed to remove ancillary extract directory");
            }
        }
        Err(e) => {
            // Non-fatal: the node can still sync from genesis, just slower.
            warn!(
                error = %e,
                "Ancillary download failed — node will fall back to full block replay"
            );
        }
    }

    // Step 7c: Clear stale UTxO store and ledger snapshots.
    //
    // The on-disk LSM UTxO store (utxo-store/) is separate from the immutable DB.
    // After a Mithril import the old UTxO store reflects a previous chain state
    // that may not match the new immutable tip.  If left in place the node would
    // have phantom UTxOs (entries that were consumed on-chain but not removed from
    // the store), causing invalid transaction propagation.
    //
    // Old dugite-format ledger snapshots (ledger-snapshot*.bin) are also removed;
    // they reference the previous immutable tip.  The newly-placed haskell-ledger/
    // directory is intentionally left in place — Task 11 reads from it on startup.
    let utxo_store_path = database_path.join("utxo-store");
    if utxo_store_path.exists() {
        info!("Removing stale UTxO store (will be rebuilt during replay)");
        if let Err(e) = fs::remove_dir_all(&utxo_store_path) {
            warn!(error = %e, "Failed to remove UTxO store directory");
        }
    }
    for entry in fs::read_dir(database_path).into_iter().flatten().flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with("ledger-snapshot") && name_str.ends_with(".bin") {
            info!(file = %name_str, "Removing stale ledger snapshot");
            let _ = fs::remove_file(entry.path());
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

    info!("Mithril import complete");
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
            info!("Mithril      snapshot archive already downloaded, skipping");
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
            // Safety: template string is a compile-time constant known to be valid
            .expect("progress bar template is a valid constant")
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
        "Mithril      download complete ({:.1} GB)",
        downloaded as f64 / (1024.0 * 1024.0 * 1024.0),
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
    info!("Mithril      verifying snapshot digest...");

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
            // Safety: template string is a compile-time constant known to be valid
            .expect("progress bar template is a valid constant")
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

    info!("Mithril      digest verified ({} files)", total_files,);
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
            info!("Mithril      archive already extracted, skipping");
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
            // Safety: template string is a compile-time constant known to be valid
            .expect("progress bar template is a valid constant"),
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
    info!("Mithril      extraction complete ({} entries)", entry_count);

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

// ---------------------------------------------------------------------------
// Block import
// ---------------------------------------------------------------------------

/// Import cardano-node immutable chunk files into Dugite's ImmutableDB.
/// Retained for fallback/testing; Mithril import now skips this step.
#[cfg(test)]
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

    // Open the database for bulk import
    let mut chain_db = dugite_storage::ChainDB::open_for_bulk_import(database_path)?;

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
            // Safety: template string is a compile-time constant known to be valid
            .expect("progress bar template is a valid constant")
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

        // Build batch refs for put_blocks_batch.
        // Mithril snapshots are post-Byron, so is_ebb is always false.
        let batch: Vec<(SlotNo, &Hash32, BlockNo, &[u8], bool)> = blocks_to_import
            .iter()
            .map(|(slot, hash, block_no, cbor)| (*slot, hash, *block_no, cbor.as_slice(), false))
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

    // Persist: flush the active chunk's secondary index to disk.
    chain_db
        .persist()
        .context("Failed to persist imported blocks to disk")?;

    info!("Import persisted successfully");

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

    let mut total_blocks = 0u64;

    for chunk_num in &chunk_numbers {
        let chunk_path = immutable_dir.join(format!("{chunk_num:05}.chunk"));
        let secondary_path = immutable_dir.join(format!("{chunk_num:05}.secondary"));

        // Fast path: use secondary index for block boundaries (no pallas decode)
        if secondary_path.exists() {
            let count = replay_chunk_with_index(&chunk_path, &secondary_path, &mut on_block)?;
            if count > 0 {
                total_blocks += count;
                continue;
            }
        }

        // Fallback: sequential CBOR probe for block boundaries (no pallas decode)
        let count = replay_chunk_sequential(&chunk_path, &mut on_block)?;
        total_blocks += count;
    }

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
    // SAFETY: File is opened read-only and not modified externally during the lifetime of this Mmap.
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
    // SAFETY: File is opened read-only and not modified externally during the lifetime of this Mmap.
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
#[cfg(test)]
type ParsedBlock = (SlotNo, Hash32, BlockNo, Vec<u8>);

/// Parse a chunk file using the secondary index for block boundaries.
///
/// Uses memory-mapped I/O for the chunk file to avoid loading the entire file
/// into memory. The secondary index is small enough to read directly.
#[cfg(test)]
fn parse_chunk_with_index(
    chunk_path: &Path,
    secondary_path: &Path,
    checksum_failures: &mut u64,
) -> Result<Vec<ParsedBlock>> {
    let secondary_data = fs::read(secondary_path).context("Failed to read secondary index file")?;

    // Memory-map the chunk file instead of reading it entirely into memory
    let chunk_file = fs::File::open(chunk_path).context("Failed to open chunk file")?;
    // SAFETY: File is opened read-only and not modified externally during the lifetime of this Mmap.
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
        if entry._checksum != 0 && !verify_block_checksum(block_cbor, entry._checksum) {
            *checksum_failures += 1;
            warn!(
                chunk = %chunk_path.display(),
                offset = block_start,
                expected_crc = entry._checksum,
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
                let hash = Hash32::from_bytes(entry._header_hash);

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
#[cfg(test)]
fn parse_chunk_sequential(chunk_path: &Path) -> Result<Vec<ParsedBlock>> {
    let chunk_file = fs::File::open(chunk_path).context("Failed to open chunk file")?;
    let chunk_len = chunk_file.metadata()?.len() as usize;
    if chunk_len == 0 {
        return Ok(Vec::new());
    }

    // SAFETY: File is opened read-only and not modified externally during the lifetime of this Mmap.
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

// ---------------------------------------------------------------------------
// Ancillary archive download and Ed25519 verification (Mithril V2 API)
//
// These types and functions are used by tests now and will be integrated into
// the import_snapshot flow in a follow-up (Task 10). Allow dead_code until then.
// ---------------------------------------------------------------------------

/// Cardano Database snapshot from the V2 `/artifact/cardano-database` API.
#[allow(dead_code)]
#[derive(Debug, serde::Deserialize)]
pub(crate) struct CardanoDatabaseSnapshot {
    pub hash: String,
    pub beacon: SnapshotBeacon,
    pub ancillary: AncillaryInfo,
    pub certificate_hash: String,
}

/// Information about the ancillary archive within a Cardano Database snapshot.
#[allow(dead_code)]
#[derive(Debug, serde::Deserialize)]
pub(crate) struct AncillaryInfo {
    pub size_uncompressed: u64,
    pub locations: Vec<AncillaryLocation>,
}

/// A download location for the ancillary archive.
#[allow(dead_code)]
#[derive(Debug, serde::Deserialize)]
pub(crate) struct AncillaryLocation {
    /// Location type: "cloud_storage" or "aggregator_uri".
    #[serde(rename = "type")]
    pub location_type: String,
    /// URI — plain string for cloud_storage, or `{"Template": "..."}` for aggregator.
    pub uri: serde_json::Value,
    /// Compression algorithm (usually "zstandard").
    pub compression_algorithm: Option<String>,
}

/// Manifest accompanying the ancillary archive, listing file paths and their
/// SHA-256 digests, plus an optional Ed25519 signature over the manifest hash.
#[allow(dead_code)]
#[derive(Debug, serde::Deserialize)]
pub(crate) struct AncillaryManifest {
    /// Map of relative file paths to their hex-encoded SHA-256 digests.
    /// BTreeMap ensures deterministic iteration order (sorted keys), which is
    /// required for computing the manifest hash that the signature covers.
    pub data: BTreeMap<String, String>,
    /// Hex-encoded Ed25519 signature over the manifest hash (64 bytes decoded).
    pub signature: Option<String>,
}

// ---------------------------------------------------------------------------
// Ancillary verification keys per network
// ---------------------------------------------------------------------------
//
// These are hex-encoded JSON byte arrays (the same encoding Mithril uses for
// genesis verification keys). Each decodes to a 32-byte Ed25519 public key
// used to verify the ancillary manifest signature.
//
// Source: mithril-infra/configuration/<env>/ancillary.vkey

#[allow(dead_code)]
/// Preview ancillary verification key.
const PREVIEW_ANCILLARY_VKEY: &str = "5b3138392c3139322c3231362c3135302c3131342c3231362c3233372c3231302c34352c31382c32312c3139362c3230382c3234362c3134362c322c3235322c3234332c3235312c3139372c32382c3135372c3230342c3134352c33302c31342c3232382c3136382c3132392c38332c3133362c33365d";

#[allow(dead_code)]
/// Preprod ancillary verification key.
const PREPROD_ANCILLARY_VKEY: &str = "5b3138392c3139322c3231362c3135302c3131342c3231362c3233372c3231302c34352c31382c32312c3139362c3230382c3234362c3134362c322c3235322c3234332c3235312c3139372c32382c3135372c3230342c3134352c33302c31342c3232382c3136382c3132392c38332c3133362c33365d";

#[allow(dead_code)]
/// Mainnet ancillary verification key.
const MAINNET_ANCILLARY_VKEY: &str = "5b32332c37312c39362c3133332c34372c3235332c3232362c3133362c3233352c35372c3136342c3130362c3138362c322c32312c32392c3132302c3136332c38392c3132312c3137372c3133382c3230382c3133382c3231342c39392c35382c32322c302c35382c332c36395d";

/// Decode a Mithril-format hex-encoded JSON byte array into 32 raw key bytes.
///
/// The wire format is: hex(json_array_of_u8_values), e.g.
/// `"5b3138392c..."` → `[189,192,216,...]` → 32-byte Ed25519 public key.
#[allow(dead_code)]
fn decode_ancillary_vkey(hex_json: &str) -> Result<[u8; 32]> {
    let json_bytes = hex::decode(hex_json).context("ancillary vkey: hex decode failed")?;
    let json_str = std::str::from_utf8(&json_bytes).context("ancillary vkey: invalid UTF-8")?;
    let arr: Vec<u8> =
        serde_json::from_str(json_str).context("ancillary vkey: JSON parse failed")?;
    if arr.len() != 32 {
        anyhow::bail!("ancillary vkey: expected 32 bytes, got {}", arr.len());
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&arr);
    Ok(key)
}

/// Look up the ancillary verification key for a given network magic.
/// Returns `None` for unknown/private networks.
#[allow(dead_code)]
fn ancillary_verification_key(network_magic: u64) -> Option<[u8; 32]> {
    let hex_json = match network_magic {
        764824073 => Some(MAINNET_ANCILLARY_VKEY),
        2 => Some(PREVIEW_ANCILLARY_VKEY),
        1 => Some(PREPROD_ANCILLARY_VKEY),
        _ => None,
    }?;
    decode_ancillary_vkey(hex_json).ok()
}

/// Compute the manifest hash used for Ed25519 signature verification.
///
/// Algorithm (matches Mithril `ManifestSigner::compute_hash`):
///   sha256 = SHA256::new()
///   for (key, value) in manifest.data:  // BTreeMap = sorted by key
///     sha256.update(key.as_bytes())
///     sha256.update(value.as_bytes())   // hex string bytes, NOT decoded
///   hash = sha256.finalize()
#[allow(dead_code)]
fn compute_manifest_hash(manifest: &AncillaryManifest) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for (key, value) in &manifest.data {
        hasher.update(key.as_bytes());
        hasher.update(value.as_bytes());
    }
    hasher.finalize().into()
}

/// Verify the ancillary manifest: per-file SHA-256 digests and Ed25519 signature.
///
/// 1. For each `(path, expected_hash)` in the manifest, reads the file at
///    `base_dir/path`, computes its SHA-256 hex digest, and compares.
/// 2. Computes the manifest hash (SHA-256 over sorted key+value pairs).
/// 3. Verifies the Ed25519 signature over the 32-byte hash.
#[allow(dead_code)]
pub(crate) fn verify_ancillary_manifest(
    base_dir: &Path,
    manifest: &AncillaryManifest,
    verification_key: &[u8; 32],
) -> Result<()> {
    // Step 1: Verify per-file SHA-256 digests.
    let total_files = manifest.data.len() as u64;
    let pb = ProgressBar::new(total_files);
    pb.set_style(
        ProgressStyle::default_bar()
            .template(
                "{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {pos}/{len} files (verifying ancillary)",
            )
            .expect("progress bar template is a valid constant")
            .progress_chars("█▉▊▋▌▍▎▏ "),
    );

    let mut buf = [0u8; 256 * 1024];
    for (path, expected_hash) in &manifest.data {
        let file_path = base_dir.join(path);
        let mut file_hasher = Sha256::new();
        let mut file = fs::File::open(&file_path)
            .with_context(|| format!("ancillary: failed to open {}", file_path.display()))?;
        loop {
            let n = std::io::Read::read(&mut file, &mut buf)
                .with_context(|| format!("ancillary: IO error reading {}", file_path.display()))?;
            if n == 0 {
                break;
            }
            file_hasher.update(&buf[..n]);
        }
        let computed = hex::encode(file_hasher.finalize());
        if computed != *expected_hash {
            anyhow::bail!(
                "ancillary: file digest mismatch for {path}\n  \
                 expected: {expected_hash}\n  computed: {computed}"
            );
        }
        pb.inc(1);
    }
    pb.finish_with_message("File digests verified");

    // Step 2: Compute manifest hash and verify signature.
    let manifest_hash = compute_manifest_hash(manifest);

    let signature_hex = manifest
        .signature
        .as_ref()
        .context("ancillary manifest has no signature")?;
    let sig_bytes = hex::decode(signature_hex).context("ancillary: signature hex decode failed")?;
    let signature = Signature::from_slice(&sig_bytes)
        .map_err(|e| anyhow::anyhow!("ancillary: invalid signature: {e}"))?;

    let verifying_key = VerifyingKey::from_bytes(verification_key)
        .map_err(|e| anyhow::anyhow!("ancillary: invalid verification key: {e}"))?;

    verifying_key
        .verify_strict(&manifest_hash, &signature)
        .map_err(|e| anyhow::anyhow!("ancillary: Ed25519 signature verification failed: {e}"))?;

    info!(
        files = manifest.data.len(),
        "Ancillary manifest verified (file digests + Ed25519 signature)"
    );
    Ok(())
}

/// Download and verify the ancillary archive from the Mithril V2 API.
///
/// Steps:
/// 1. Fetch the latest Cardano Database snapshot from the aggregator.
/// 2. Extract the cloud storage URI for the ancillary archive.
/// 3. Download the tar.zst archive with progress reporting.
/// 4. Extract to a temporary directory.
/// 5. Parse and verify the ancillary manifest (file digests + Ed25519 signature).
/// 6. Return the path to the extracted ancillary directory.
#[allow(dead_code)]
pub(crate) async fn download_ancillary(
    aggregator: &str,
    network_magic: u64,
    _database_path: &Path,
    temp_dir: &Path,
) -> Result<PathBuf> {
    let client = reqwest::Client::builder()
        .user_agent("dugite-node/0.1")
        .build()?;

    // Step 1: Get latest Cardano Database snapshot list.
    info!("Fetching Cardano Database snapshot list from V2 API...");
    let snapshots: Vec<CardanoDatabaseSnapshot> = client
        .get(format!("{aggregator}/artifact/cardano-database"))
        .send()
        .await?
        .error_for_status()
        .context("Failed to fetch Cardano Database snapshot list")?
        .json()
        .await?;

    let latest = snapshots
        .first()
        .context("No Cardano Database snapshots available")?;

    info!(
        hash = %latest.hash,
        epoch = latest.beacon.epoch,
        immutable = latest.beacon.immutable_file_number,
        "Found Cardano Database snapshot"
    );

    // Step 2: Fetch snapshot detail to get full ancillary location info.
    let snapshot_detail: CardanoDatabaseSnapshot = client
        .get(format!(
            "{aggregator}/artifact/cardano-database/{}",
            latest.hash
        ))
        .send()
        .await?
        .error_for_status()
        .context("Failed to fetch Cardano Database snapshot detail")?
        .json()
        .await?;

    // Step 3: Extract the first cloud_storage URI with a plain string value.
    let download_url = snapshot_detail
        .ancillary
        .locations
        .iter()
        .find_map(|loc| {
            if loc.location_type == "cloud_storage" {
                loc.uri.as_str().map(|s| s.to_string())
            } else {
                None
            }
        })
        .context(
            "No cloud_storage URI found in ancillary locations. \
             Available locations: {:?}",
        )?;

    info!(url = %download_url, "Downloading ancillary archive...");

    // Step 4: Download to temp directory.
    fs::create_dir_all(temp_dir)?;
    let archive_path = temp_dir.join("ancillary.tar.zst");
    download_snapshot(
        &client,
        &download_url,
        &archive_path,
        snapshot_detail.ancillary.size_uncompressed,
    )
    .await?;

    // Step 5: Extract archive.
    let extract_dir = temp_dir.join("ancillary");
    info!("Extracting ancillary archive...");
    extract_archive(&archive_path, &extract_dir)?;

    // Step 6: Parse and verify the manifest.
    //
    // The manifest may be at the top level of the extract directory or inside
    // a subdirectory. Search for it.
    let manifest_path = find_ancillary_manifest(&extract_dir)
        .context("Could not find ancillary_manifest.json in extracted ancillary archive")?;

    let manifest_content = fs::read_to_string(&manifest_path)
        .with_context(|| format!("Failed to read {}", manifest_path.display()))?;
    let manifest: AncillaryManifest = serde_json::from_str(&manifest_content)
        .context("Failed to parse ancillary_manifest.json")?;

    info!(
        files = manifest.data.len(),
        signature = manifest.signature.is_some(),
        "Parsed ancillary manifest"
    );

    // The manifest file paths are relative to the directory containing the manifest.
    let manifest_base = manifest_path.parent().unwrap_or(&extract_dir);

    if let Some(vkey) = ancillary_verification_key(network_magic) {
        verify_ancillary_manifest(manifest_base, &manifest, &vkey)?;
    } else {
        warn!(
            "No ancillary verification key for network magic {network_magic}; \
             skipping Ed25519 signature verification"
        );
        // Still verify file digests without the signature check.
        verify_ancillary_file_digests(manifest_base, &manifest)?;
    }

    // Clean up the archive.
    if let Err(e) = fs::remove_file(&archive_path) {
        warn!(error = %e, "Failed to remove ancillary archive");
    }

    Ok(extract_dir)
}

/// Verify only the per-file SHA-256 digests (no signature check).
/// Used when no verification key is available for the network.
#[allow(dead_code)]
fn verify_ancillary_file_digests(base_dir: &Path, manifest: &AncillaryManifest) -> Result<()> {
    let mut buf = [0u8; 256 * 1024];
    for (path, expected_hash) in &manifest.data {
        let file_path = base_dir.join(path);
        let mut file_hasher = Sha256::new();
        let mut file = fs::File::open(&file_path)
            .with_context(|| format!("ancillary: failed to open {}", file_path.display()))?;
        loop {
            let n = std::io::Read::read(&mut file, &mut buf)
                .with_context(|| format!("ancillary: IO error reading {}", file_path.display()))?;
            if n == 0 {
                break;
            }
            file_hasher.update(&buf[..n]);
        }
        let computed = hex::encode(file_hasher.finalize());
        if computed != *expected_hash {
            anyhow::bail!(
                "ancillary: file digest mismatch for {path}\n  \
                 expected: {expected_hash}\n  computed: {computed}"
            );
        }
    }
    info!(
        files = manifest.data.len(),
        "Ancillary file digests verified (no signature — unknown network)"
    );
    Ok(())
}

/// Search for `ancillary_manifest.json` within the extract directory.
#[allow(dead_code)]
fn find_ancillary_manifest(extract_dir: &Path) -> Option<PathBuf> {
    // Direct location
    let direct = extract_dir.join("ancillary_manifest.json");
    if direct.is_file() {
        return Some(direct);
    }
    // One level deeper
    if let Ok(entries) = fs::read_dir(extract_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let nested = path.join("ancillary_manifest.json");
                if nested.is_file() {
                    return Some(nested);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::Signer;

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
        assert_eq!(entry._checksum, 12345);
        assert_eq!(entry._header_hash, [0xAB; 32]);
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
    fn test_genesis_verification_key_known_networks() {
        // Mainnet has a distinct key
        assert!(genesis_verification_key(764824073).is_some());
        let mainnet_key = genesis_verification_key(764824073).unwrap();
        assert!(mainnet_key.starts_with("5b31393"));
        assert_ne!(
            mainnet_key,
            genesis_verification_key(2).unwrap(),
            "mainnet key should differ from preview"
        );

        // Preview
        assert!(genesis_verification_key(2).is_some());

        // Preprod (same key as preview)
        assert!(genesis_verification_key(1).is_some());
        assert_eq!(
            genesis_verification_key(2).unwrap(),
            genesis_verification_key(1).unwrap(),
            "preview and preprod share the same genesis key"
        );
    }

    #[test]
    fn test_genesis_verification_key_unknown_network() {
        assert!(genesis_verification_key(999).is_none());
        assert!(genesis_verification_key(0).is_none());
    }

    #[test]
    fn test_genesis_keys_are_valid_hex() {
        // Each genesis key should be a valid hex string that decodes to a JSON array
        for magic in [764824073, 2, 1] {
            let key = genesis_verification_key(magic).unwrap();
            let decoded = hex::decode(key)
                .unwrap_or_else(|_| panic!("genesis key for magic {magic} is not valid hex"));
            let json_str = std::str::from_utf8(&decoded)
                .unwrap_or_else(|_| panic!("genesis key for magic {magic} is not valid UTF-8"));
            assert!(
                json_str.starts_with('[') && json_str.ends_with(']'),
                "genesis key for magic {magic} should decode to a JSON array, got: {json_str}"
            );
        }
    }

    /// Integration test: verify a real Mithril preview certificate chain.
    ///
    /// This test hits the real Mithril aggregator API and verifies that we can
    /// successfully build a client, fetch a snapshot, and verify its certificate
    /// chain back to genesis. Run manually with:
    ///   cargo nextest run -p dugite-node -E 'test(verify_preview_certificate_chain)' -- --ignored
    #[tokio::test]
    #[ignore]
    async fn test_verify_preview_certificate_chain() {
        let aggregator = aggregator_url(2); // preview
        let genesis_vkey = genesis_verification_key(2).unwrap();

        // Build the Mithril client
        let client = mithril_client::ClientBuilder::new(
            mithril_client::AggregatorDiscoveryType::Url(aggregator.to_string()),
        )
        .set_genesis_verification_key(mithril_client::GenesisVerificationKey::JsonHex(
            genesis_vkey.to_string(),
        ))
        .build()
        .expect("Failed to build Mithril client");

        // Fetch latest snapshot to get its certificate_hash
        let http = reqwest::Client::builder()
            .user_agent("dugite-test/0.1")
            .build()
            .unwrap();

        let snapshots: Vec<serde_json::Value> = http
            .get(format!("{aggregator}/artifact/snapshots"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();

        let cert_hash = snapshots[0]["certificate_hash"]
            .as_str()
            .expect("No certificate_hash in snapshot");

        // Verify the certificate chain — this is the core test
        let certificate = client
            .certificate()
            .verify_chain(cert_hash)
            .await
            .expect("Certificate chain verification failed");

        // Epoch implements Display, so just verify it's not the default
        let epoch_str = format!("{}", certificate.epoch);
        assert!(epoch_str != "0", "certificate epoch should be positive");
        println!(
            "Certificate chain verified: epoch={}, hash={}",
            certificate.epoch, cert_hash
        );
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

    // -----------------------------------------------------------------------
    // Ancillary verification tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_decode_ancillary_vkey_preview() {
        let key = decode_ancillary_vkey(PREVIEW_ANCILLARY_VKEY).unwrap();
        assert_eq!(key.len(), 32);
        // First byte should be 189 (from the JSON array [189,192,...])
        assert_eq!(key[0], 189);
    }

    #[test]
    fn test_decode_ancillary_vkey_mainnet() {
        let key = decode_ancillary_vkey(MAINNET_ANCILLARY_VKEY).unwrap();
        assert_eq!(key.len(), 32);
        // First byte should be 23 (from the JSON array [23,71,...])
        assert_eq!(key[0], 23);
    }

    #[test]
    fn test_decode_ancillary_vkey_all_networks() {
        // All three known networks should decode to valid 32-byte keys.
        for (magic, name) in [(764824073, "mainnet"), (2, "preview"), (1, "preprod")] {
            let key = ancillary_verification_key(magic);
            assert!(
                key.is_some(),
                "ancillary vkey should exist for {name} (magic={magic})"
            );
            assert_eq!(key.unwrap().len(), 32);
        }
    }

    #[test]
    fn test_decode_ancillary_vkey_preview_preprod_same() {
        // Preview and preprod share the same ancillary verification key.
        let preview = ancillary_verification_key(2).unwrap();
        let preprod = ancillary_verification_key(1).unwrap();
        assert_eq!(preview, preprod);
    }

    #[test]
    fn test_decode_ancillary_vkey_mainnet_differs() {
        // Mainnet has a distinct key from preview/preprod.
        let mainnet = ancillary_verification_key(764824073).unwrap();
        let preview = ancillary_verification_key(2).unwrap();
        assert_ne!(mainnet, preview);
    }

    #[test]
    fn test_ancillary_vkey_unknown_network() {
        assert!(ancillary_verification_key(999).is_none());
        assert!(ancillary_verification_key(0).is_none());
    }

    #[test]
    fn test_decode_ancillary_vkey_invalid_hex() {
        assert!(decode_ancillary_vkey("not_hex").is_err());
    }

    #[test]
    fn test_decode_ancillary_vkey_wrong_length() {
        // Encode a JSON array with only 16 bytes
        let short_arr: Vec<u8> = (0..16).collect();
        let json = serde_json::to_string(&short_arr).unwrap();
        let hex_json = hex::encode(json.as_bytes());
        assert!(decode_ancillary_vkey(&hex_json).is_err());
    }

    #[test]
    fn test_compute_manifest_hash_deterministic() {
        let mut data = BTreeMap::new();
        data.insert("file_a.dat".to_string(), "aabbcc".to_string());
        data.insert("file_b.dat".to_string(), "ddeeff".to_string());
        let manifest = AncillaryManifest {
            data,
            signature: None,
        };

        let hash1 = compute_manifest_hash(&manifest);
        let hash2 = compute_manifest_hash(&manifest);
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_compute_manifest_hash_sorted_order() {
        // BTreeMap iteration is sorted, so inserting in different order
        // should yield the same hash.
        let mut data1 = BTreeMap::new();
        data1.insert("z_file".to_string(), "hash_z".to_string());
        data1.insert("a_file".to_string(), "hash_a".to_string());

        let mut data2 = BTreeMap::new();
        data2.insert("a_file".to_string(), "hash_a".to_string());
        data2.insert("z_file".to_string(), "hash_z".to_string());

        let m1 = AncillaryManifest {
            data: data1,
            signature: None,
        };
        let m2 = AncillaryManifest {
            data: data2,
            signature: None,
        };
        assert_eq!(compute_manifest_hash(&m1), compute_manifest_hash(&m2));
    }

    #[test]
    fn test_compute_manifest_hash_known_value() {
        // Compute expected hash manually:
        //   SHA256("file_a" || "hash_a" || "file_b" || "hash_b")
        let mut data = BTreeMap::new();
        data.insert("file_a".to_string(), "hash_a".to_string());
        data.insert("file_b".to_string(), "hash_b".to_string());
        let manifest = AncillaryManifest {
            data,
            signature: None,
        };

        let hash = compute_manifest_hash(&manifest);

        let mut expected_hasher = Sha256::new();
        expected_hasher.update(b"file_a");
        expected_hasher.update(b"hash_a");
        expected_hasher.update(b"file_b");
        expected_hasher.update(b"hash_b");
        let expected: [u8; 32] = expected_hasher.finalize().into();

        assert_eq!(hash, expected);
    }

    #[test]
    fn test_verify_ancillary_manifest_file_digests() {
        // Create temp files and a manifest with correct digests.
        let dir = tempfile::tempdir().unwrap();
        let content_a = b"hello world";
        let content_b = b"foo bar baz";
        fs::write(dir.path().join("a.txt"), content_a).unwrap();
        fs::write(dir.path().join("b.txt"), content_b).unwrap();

        let hash_a = hex::encode(Sha256::digest(content_a));
        let hash_b = hex::encode(Sha256::digest(content_b));

        let mut data = BTreeMap::new();
        data.insert("a.txt".to_string(), hash_a);
        data.insert("b.txt".to_string(), hash_b);

        // Generate a real Ed25519 signature over the manifest hash.
        let signing_key = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
        let vkey_bytes: [u8; 32] = signing_key.verifying_key().to_bytes();

        let manifest_hash = {
            let mut hasher = Sha256::new();
            for (k, v) in &data {
                hasher.update(k.as_bytes());
                hasher.update(v.as_bytes());
            }
            let h: [u8; 32] = hasher.finalize().into();
            h
        };
        let sig = signing_key.sign(&manifest_hash);
        let sig_hex = hex::encode(sig.to_bytes());

        let manifest = AncillaryManifest {
            data,
            signature: Some(sig_hex),
        };

        let result = verify_ancillary_manifest(dir.path(), &manifest, &vkey_bytes);
        assert!(result.is_ok(), "Expected Ok, got: {:?}", result);
    }

    #[test]
    fn test_verify_ancillary_manifest_bad_file_digest() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), b"real content").unwrap();

        let mut data = BTreeMap::new();
        data.insert(
            "a.txt".to_string(),
            "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
        );

        let manifest = AncillaryManifest {
            data,
            signature: Some("00".repeat(64)),
        };

        let vkey = [0u8; 32];
        let result = verify_ancillary_manifest(dir.path(), &manifest, &vkey);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("digest mismatch"));
    }

    #[test]
    fn test_verify_ancillary_manifest_bad_signature() {
        let dir = tempfile::tempdir().unwrap();
        let content = b"content";
        fs::write(dir.path().join("f.txt"), content).unwrap();

        let hash = hex::encode(Sha256::digest(content));
        let mut data = BTreeMap::new();
        data.insert("f.txt".to_string(), hash);

        // Use a valid key but wrong signature (all zeros is not a valid sig
        // for this message with overwhelming probability).
        let signing_key = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
        let vkey_bytes = signing_key.verifying_key().to_bytes();

        // Create a valid-format but wrong signature.
        let wrong_key = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
        let manifest_hash = compute_manifest_hash(&AncillaryManifest {
            data: data.clone(),
            signature: None,
        });
        let wrong_sig = wrong_key.sign(&manifest_hash);

        let manifest = AncillaryManifest {
            data,
            signature: Some(hex::encode(wrong_sig.to_bytes())),
        };

        let result = verify_ancillary_manifest(dir.path(), &manifest, &vkey_bytes);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("signature verification failed"));
    }

    #[test]
    fn test_verify_ancillary_manifest_missing_signature() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("f.txt"), b"x").unwrap();

        let mut data = BTreeMap::new();
        data.insert("f.txt".to_string(), hex::encode(Sha256::digest(b"x")));

        let manifest = AncillaryManifest {
            data,
            signature: None,
        };

        let vkey = ancillary_verification_key(2).unwrap();
        let result = verify_ancillary_manifest(dir.path(), &manifest, &vkey);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("no signature"));
    }

    #[test]
    fn test_verify_ancillary_manifest_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        // Don't create the file referenced in the manifest.
        let mut data = BTreeMap::new();
        data.insert("nonexistent.dat".to_string(), "abcd".to_string());

        let manifest = AncillaryManifest {
            data,
            signature: Some("00".repeat(64)),
        };

        let vkey = [0u8; 32];
        let result = verify_ancillary_manifest(dir.path(), &manifest, &vkey);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("failed to open"));
    }

    /// End-to-end test using the real preview manifest fixture.
    /// Verifies that we can parse the manifest and that the hash/signature
    /// computation is consistent with what Mithril produces.
    #[test]
    fn test_ancillary_manifest_fixture_parse_and_hash() {
        let fixture_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../dugite-serialization/test_fixtures/preview_manifest_e1259.json"
        );
        let content =
            fs::read_to_string(fixture_path).expect("Failed to read preview manifest fixture");
        let manifest: AncillaryManifest =
            serde_json::from_str(&content).expect("Failed to parse manifest fixture");

        // Should have files in the data map.
        assert!(
            !manifest.data.is_empty(),
            "manifest data should not be empty"
        );

        // Should have a signature.
        assert!(
            manifest.signature.is_some(),
            "fixture should have a signature"
        );

        // Verify the signature hex decodes to 64 bytes.
        let sig_hex = manifest.signature.as_ref().unwrap();
        let sig_bytes = hex::decode(sig_hex).expect("signature should be valid hex");
        assert_eq!(sig_bytes.len(), 64, "Ed25519 signature should be 64 bytes");

        // Compute manifest hash and verify it's deterministic.
        let hash = compute_manifest_hash(&manifest);
        assert_eq!(hash.len(), 32);
        let hash2 = compute_manifest_hash(&manifest);
        assert_eq!(hash, hash2, "manifest hash should be deterministic");

        // Verify the signature against the preview ancillary verification key.
        let vkey = ancillary_verification_key(2).unwrap();
        let verifying_key = VerifyingKey::from_bytes(&vkey).unwrap();
        let signature = Signature::from_slice(&sig_bytes).unwrap();
        let result = verifying_key.verify_strict(&hash, &signature);
        assert!(
            result.is_ok(),
            "Preview manifest fixture signature should verify against the preview ancillary vkey: {:?}",
            result
        );
    }

    #[test]
    fn test_find_ancillary_manifest_direct() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("ancillary_manifest.json"), "{}").unwrap();
        assert_eq!(
            find_ancillary_manifest(dir.path()),
            Some(dir.path().join("ancillary_manifest.json"))
        );
    }

    #[test]
    fn test_find_ancillary_manifest_nested() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("subdir");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("ancillary_manifest.json"), "{}").unwrap();
        assert_eq!(
            find_ancillary_manifest(dir.path()),
            Some(sub.join("ancillary_manifest.json"))
        );
    }

    #[test]
    fn test_find_ancillary_manifest_not_found() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(find_ancillary_manifest(dir.path()), None);
    }

    #[test]
    fn test_cardano_database_snapshot_deserialize() {
        // Minimal JSON to verify our struct deserialization.
        let json = r#"{
            "hash": "abc123",
            "beacon": { "epoch": 100, "immutable_file_number": 5000 },
            "ancillary": {
                "size_uncompressed": 1234567,
                "locations": [
                    {
                        "type": "cloud_storage",
                        "uri": "https://example.com/ancillary.tar.zst",
                        "compression_algorithm": "zstandard"
                    },
                    {
                        "type": "aggregator_uri",
                        "uri": {"Template": "https://agg/{hash}"},
                        "compression_algorithm": null
                    }
                ]
            },
            "certificate_hash": "def456"
        }"#;

        let snapshot: CardanoDatabaseSnapshot = serde_json::from_str(json).unwrap();
        assert_eq!(snapshot.hash, "abc123");
        assert_eq!(snapshot.beacon.epoch, 100);
        assert_eq!(snapshot.beacon.immutable_file_number, 5000);
        assert_eq!(snapshot.ancillary.size_uncompressed, 1234567);
        assert_eq!(snapshot.ancillary.locations.len(), 2);
        assert_eq!(
            snapshot.ancillary.locations[0].location_type,
            "cloud_storage"
        );
        assert_eq!(
            snapshot.ancillary.locations[0].uri.as_str().unwrap(),
            "https://example.com/ancillary.tar.zst"
        );
        // Second location has an object URI.
        assert!(snapshot.ancillary.locations[1].uri.is_object());
        assert_eq!(snapshot.certificate_hash, "def456");
    }
}
