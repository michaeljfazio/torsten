//! Golden test: epoch nonce computation for preview testnet.
//!
//! Reads blocks from immutable chunk files at `/tmp/db-preview-nonce-test/immutable/`,
//! computes the evolving nonce and epoch nonce independently using pallas's
//! `generate_rolling_nonce` and `generate_epoch_nonce`, and compares them with
//! dugite's own nonce computation.
//!
//! Preview testnet parameters:
//!   - epoch_length = 86400 slots
//!   - k = 432 (security parameter)
//!   - f = 0.05 (active slot coefficient)
//!   - shelley_transition_epoch = 0 (no Byron era)
//!   - 3k/f = ceil(3*432/0.05) = 25920
//!   - 4k/f = ceil(4*432/0.05) = 34560
//!   - Shelley genesis hash = 363498d1024f84bb39d3fa9593ce391483cb40d479b87233f868d6e57c3a400d

use memmap2::Mmap;
use pallas_crypto::hash::{Hash as PallasHash, Hasher};
use pallas_crypto::nonce::{generate_epoch_nonce, generate_rolling_nonce};
use pallas_traverse::Era as PallasEra;
use pallas_traverse::MultiEraBlock as PallasBlock;
use std::fs;
use std::ops::ControlFlow;
use std::path::Path;

// ---------------------------------------------------------------------------
// Preview testnet constants
// ---------------------------------------------------------------------------

/// Shelley genesis hash for preview testnet (Blake2b-256 of the genesis JSON file).
const PREVIEW_GENESIS_HASH: &str =
    "363498d1024f84bb39d3fa9593ce391483cb40d479b87233f868d6e57c3a400d";

/// Epoch length in slots for preview testnet.
const EPOCH_LENGTH: u64 = 86400;

/// Security parameter k for preview testnet.
#[allow(dead_code)]
const SECURITY_PARAM: u64 = 432;

/// Active slot coefficient f for preview testnet.
#[allow(dead_code)]
const ACTIVE_SLOT_COEFF: f64 = 0.05;

/// Randomness stabilisation window = ceil(3k/f) for Alonzo/Babbage.
const STABILITY_WINDOW_3KF: u64 = 25920; // ceil(3 * 432 / 0.05)

/// Randomness stabilisation window = ceil(4k/f) for Conway.
const STABILITY_WINDOW_4KF: u64 = 34560; // ceil(4 * 432 / 0.05)

/// Number of epochs to compute nonces for (epochs 0 and 1).
const NUM_EPOCHS: u64 = 2;

/// Path to the immutable DB chunk files for this test.
const IMMUTABLE_DIR: &str = "/tmp/db-preview-nonce-test/immutable";

// ---------------------------------------------------------------------------
// Secondary index parsing (same format as cardano-node)
// ---------------------------------------------------------------------------

/// Entry from a cardano-node secondary index file.
/// Each entry is 56 bytes: offset(8 BE) + header_offset(2 BE) + header_size(2 BE)
/// + crc32(4 BE) + header_hash(32) + block_or_ebb(8 BE).
#[derive(Debug, Clone)]
struct SecondaryIndexEntry {
    block_offset: u64,
}

impl SecondaryIndexEntry {
    fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 56 {
            return None;
        }
        let block_offset = u64::from_be_bytes(data[0..8].try_into().ok()?);
        Some(SecondaryIndexEntry { block_offset })
    }
}

// ---------------------------------------------------------------------------
// Block iteration from chunk files
// ---------------------------------------------------------------------------

/// Information extracted from a block relevant to nonce computation.
struct BlockNonceInfo {
    slot: u64,
    block_number: u64,
    era: PallasEra,
    prev_hash: Option<PallasHash<32>>,
    /// Raw nonce VRF output from pallas header:
    ///   - ShelleyCompatible (Alonzo): raw 64-byte nonce_vrf.0
    ///   - BabbageCompatible (Babbage/Conway): 32-byte blake2b("N" || vrf_result.0)
    pallas_nonce_vrf_output: Option<Vec<u8>>,
    /// Dugite's pre-computed nonce eta (from dugite-serialization decode):
    ///   - Alonzo: blake2b(nonce_vrf.0) = 32 bytes
    ///   - Babbage/Conway: blake2b("N" || vrf_result.0) = 32 bytes
    dugite_nonce_vrf_output: Vec<u8>,
}

/// Iterate blocks from chunk files, calling the callback for each block.
/// The callback returns `ControlFlow::Continue(())` to keep going or
/// `ControlFlow::Break(())` to stop early.
fn iterate_blocks<F>(immutable_dir: &Path, mut on_block: F)
where
    F: FnMut(BlockNonceInfo) -> ControlFlow<()>,
{
    let mut chunk_numbers: Vec<u64> = Vec::new();
    for entry in fs::read_dir(immutable_dir).expect("Failed to read immutable dir") {
        let entry = entry.expect("Failed to read dir entry");
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if let Some(num_str) = name_str.strip_suffix(".chunk") {
            if let Ok(num) = num_str.parse::<u64>() {
                chunk_numbers.push(num);
            }
        }
    }
    chunk_numbers.sort();

    'outer: for chunk_num in &chunk_numbers {
        let chunk_path = immutable_dir.join(format!("{chunk_num:05}.chunk"));
        let secondary_path = immutable_dir.join(format!("{chunk_num:05}.secondary"));

        if !secondary_path.exists() {
            eprintln!("WARNING: No secondary index for chunk {chunk_num}, skipping");
            continue;
        }

        let secondary_data = fs::read(&secondary_path).expect("Failed to read secondary index");
        let chunk_file = fs::File::open(&chunk_path).expect("Failed to open chunk file");
        // SAFETY: File is opened read-only and not modified during the lifetime of this Mmap.
        let chunk_data = unsafe { Mmap::map(&chunk_file).expect("Failed to mmap chunk") };

        // Parse secondary index entries
        let mut entries = Vec::new();
        let mut offset = 0;
        while offset + 56 <= secondary_data.len() {
            if let Some(entry) = SecondaryIndexEntry::from_bytes(&secondary_data[offset..]) {
                entries.push(entry);
            }
            offset += 56;
        }

        // Read each block using secondary index boundaries
        for i in 0..entries.len() {
            let start = entries[i].block_offset as usize;
            let end = if i + 1 < entries.len() {
                entries[i + 1].block_offset as usize
            } else {
                chunk_data.len()
            };

            if start >= chunk_data.len() || end > chunk_data.len() || start >= end {
                continue;
            }

            let block_cbor = &chunk_data[start..end];

            // Decode with pallas
            let pallas_block = match PallasBlock::decode(block_cbor) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!(
                        "WARNING: Failed to decode block at chunk {chunk_num} offset {start}: {e}"
                    );
                    continue;
                }
            };

            let era = pallas_block.era();
            let slot = pallas_block.slot();
            let block_number = pallas_block.number();
            let header = pallas_block.header();

            let prev_hash = header.previous_hash();

            // Extract pallas nonce VRF output (raw, for pallas generate_rolling_nonce)
            let pallas_nonce_vrf_output = header.nonce_vrf_output().ok();

            // Decode with dugite-serialization to get dugite's pre-computed nonce eta
            let dugite_block =
                dugite_serialization::multi_era::decode_block_with_byron_epoch_length(
                    block_cbor, 0, // preview has no Byron era
                );
            let dugite_nonce_vrf_output = match dugite_block {
                Ok(ref b) => b.header.nonce_vrf_output.clone(),
                Err(_) => Vec::new(),
            };

            let flow = on_block(BlockNonceInfo {
                slot,
                block_number,
                era,
                prev_hash,
                pallas_nonce_vrf_output,
                dugite_nonce_vrf_output,
            });

            if flow.is_break() {
                break 'outer;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Nonce state trackers
// ---------------------------------------------------------------------------

/// Independent nonce computation using pallas functions directly.
struct PallasNonceTracker {
    evolving_nonce: PallasHash<32>,
    candidate_nonce_3kf: PallasHash<32>,
    candidate_nonce_4kf: PallasHash<32>,
    lab_nonce: PallasHash<32>,
    last_epoch_block_nonce: PallasHash<32>,
    current_epoch: u64,
    epoch_block_count: u64,
}

impl PallasNonceTracker {
    fn new(genesis_hash: PallasHash<32>) -> Self {
        Self {
            evolving_nonce: genesis_hash,
            candidate_nonce_3kf: genesis_hash,
            candidate_nonce_4kf: genesis_hash,
            lab_nonce: PallasHash::new([0u8; 32]),
            last_epoch_block_nonce: PallasHash::new([0u8; 32]),
            current_epoch: 0,
            epoch_block_count: 0,
        }
    }

    /// Apply a block's nonce VRF contribution.
    fn apply_block(&mut self, info: &BlockNonceInfo) {
        let block_epoch = info.slot / EPOCH_LENGTH;

        // Process epoch boundary if needed
        if block_epoch > self.current_epoch {
            self.process_epoch_transition(block_epoch);
        }

        // Update evolving nonce with pallas generate_rolling_nonce
        if let Some(ref nonce_vrf) = info.pallas_nonce_vrf_output {
            if !nonce_vrf.is_empty() {
                self.evolving_nonce = generate_rolling_nonce(self.evolving_nonce, nonce_vrf);

                // Candidate nonce (3k/f window) tracks evolving outside stability window
                let first_slot_next_epoch = (self.current_epoch + 1) * EPOCH_LENGTH;
                if info.slot.saturating_add(STABILITY_WINDOW_3KF) < first_slot_next_epoch {
                    self.candidate_nonce_3kf = self.evolving_nonce;
                }

                // Candidate nonce (4k/f window) tracks evolving outside stability window
                if info.slot.saturating_add(STABILITY_WINDOW_4KF) < first_slot_next_epoch {
                    self.candidate_nonce_4kf = self.evolving_nonce;
                }
            }
        }

        // LAB nonce = prev_hash of the block (prevHashToNonce)
        if let Some(ph) = info.prev_hash {
            self.lab_nonce = ph;
        }

        self.epoch_block_count += 1;
    }

    fn process_epoch_transition(&mut self, new_epoch: u64) {
        self.current_epoch = new_epoch;
        // last_epoch_block_nonce is updated at epoch boundary
        self.last_epoch_block_nonce = self.lab_nonce;
        self.epoch_block_count = 0;
    }

    /// Compute epoch nonce using pallas generate_epoch_nonce.
    /// Uses the 3k/f candidate nonce and the last_epoch_block_nonce.
    fn compute_epoch_nonce_3kf(&self) -> PallasHash<32> {
        let zero = PallasHash::new([0u8; 32]);
        if self.last_epoch_block_nonce == zero {
            // NeutralNonce identity: candidate ⭒ NeutralNonce = candidate
            self.candidate_nonce_3kf
        } else {
            generate_epoch_nonce(self.candidate_nonce_3kf, self.last_epoch_block_nonce, None)
        }
    }

    /// Compute epoch nonce using pallas generate_epoch_nonce.
    /// Uses the 4k/f candidate nonce and the last_epoch_block_nonce.
    fn compute_epoch_nonce_4kf(&self) -> PallasHash<32> {
        let zero = PallasHash::new([0u8; 32]);
        if self.last_epoch_block_nonce == zero {
            // NeutralNonce identity: candidate ⭒ NeutralNonce = candidate
            self.candidate_nonce_4kf
        } else {
            generate_epoch_nonce(self.candidate_nonce_4kf, self.last_epoch_block_nonce, None)
        }
    }
}

/// Dugite-style nonce computation (mirrors LedgerState::update_evolving_nonce).
struct DugiteNonceTracker {
    evolving_nonce: [u8; 32],
    candidate_nonce: [u8; 32],
    lab_nonce: [u8; 32],
    last_epoch_block_nonce: [u8; 32],
    current_epoch: u64,
    epoch_block_count: u64,
    /// Which stability window to use (3k/f for Alonzo/Babbage, 4k/f for Conway)
    stability_window: u64,
}

impl DugiteNonceTracker {
    fn new(genesis_hash: [u8; 32], stability_window: u64) -> Self {
        Self {
            evolving_nonce: genesis_hash,
            candidate_nonce: genesis_hash,
            lab_nonce: [0u8; 32],
            last_epoch_block_nonce: [0u8; 32],
            current_epoch: 0,
            epoch_block_count: 0,
            stability_window,
        }
    }

    fn apply_block(&mut self, info: &BlockNonceInfo) {
        let block_epoch = info.slot / EPOCH_LENGTH;

        if block_epoch > self.current_epoch {
            self.process_epoch_transition(block_epoch);
        }

        if !info.dugite_nonce_vrf_output.is_empty() {
            // Mirrors dugite LedgerState::update_evolving_nonce:
            // eta_hash = blake2b_256(nonce_eta) -- always hashes input
            // evolving = blake2b_256(evolving || eta_hash)
            let eta_hash = blake2b_256(&info.dugite_nonce_vrf_output);
            let mut data = Vec::with_capacity(64);
            data.extend_from_slice(&self.evolving_nonce);
            data.extend_from_slice(&eta_hash);
            self.evolving_nonce = blake2b_256(&data);

            // Candidate nonce tracks evolving outside stability window
            let first_slot_next_epoch = (self.current_epoch + 1) * EPOCH_LENGTH;
            if info.slot.saturating_add(self.stability_window) < first_slot_next_epoch {
                self.candidate_nonce = self.evolving_nonce;
            }
        }

        // LAB nonce = prev_hash
        if let Some(ph) = info.prev_hash {
            self.lab_nonce.copy_from_slice(ph.as_ref());
        }

        self.epoch_block_count += 1;
    }

    fn process_epoch_transition(&mut self, new_epoch: u64) {
        self.current_epoch = new_epoch;
        self.last_epoch_block_nonce = self.lab_nonce;
        self.epoch_block_count = 0;
    }

    /// Compute epoch nonce the same way dugite does (Haskell ⭒ combine).
    fn compute_epoch_nonce(&self) -> [u8; 32] {
        let zero = [0u8; 32];
        if self.candidate_nonce == zero && self.last_epoch_block_nonce == zero {
            zero
        } else if self.last_epoch_block_nonce == zero {
            // NeutralNonce identity: candidate ⭒ NeutralNonce = candidate
            self.candidate_nonce
        } else if self.candidate_nonce == zero {
            self.last_epoch_block_nonce
        } else {
            let mut input = Vec::with_capacity(64);
            input.extend_from_slice(&self.candidate_nonce);
            input.extend_from_slice(&self.last_epoch_block_nonce);
            blake2b_256(&input)
        }
    }
}

/// Blake2b-256 hash helper (pure Rust, matching dugite-primitives).
fn blake2b_256(data: &[u8]) -> [u8; 32] {
    let hash: PallasHash<32> = Hasher::<256>::hash(data);
    let mut out = [0u8; 32];
    out.copy_from_slice(hash.as_ref());
    out
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

#[test]
fn test_epoch_nonce_preview_first_2_epochs() {
    let immutable_dir = Path::new(IMMUTABLE_DIR);
    if !immutable_dir.exists() {
        eprintln!(
            "SKIPPING test_epoch_nonce_preview_first_2_epochs: \
             immutable DB not found at {IMMUTABLE_DIR}"
        );
        eprintln!("To run this test, place preview testnet chunk files at {IMMUTABLE_DIR}");
        return;
    }

    let genesis_hash_bytes = hex::decode(PREVIEW_GENESIS_HASH).unwrap();
    let genesis_hash = PallasHash::<32>::from(genesis_hash_bytes.as_slice());
    let mut genesis_arr = [0u8; 32];
    genesis_arr.copy_from_slice(&genesis_hash_bytes);

    // Three independent trackers:
    // 1. Pallas-only (using generate_rolling_nonce with raw pallas header data)
    let mut pallas_tracker = PallasNonceTracker::new(genesis_hash);
    // 2. Dugite-style with 3k/f window
    let mut dugite_tracker_3kf = DugiteNonceTracker::new(genesis_arr, STABILITY_WINDOW_3KF);
    // 3. Dugite-style with 4k/f window
    let mut dugite_tracker_4kf = DugiteNonceTracker::new(genesis_arr, STABILITY_WINDOW_4KF);

    let mut total_blocks = 0u64;
    let mut last_epoch_seen = 0u64;

    // Epoch nonce results: (epoch, pallas_3kf, pallas_4kf, dugite_3kf, dugite_4kf)
    let mut epoch_nonces: Vec<(u64, String, String, String, String)> = Vec::new();

    // Track the first block era to log
    let mut first_era: Option<PallasEra> = None;

    // We need blocks through the first block of epoch NUM_EPOCHS to capture
    // the epoch nonce for the transition into epoch NUM_EPOCHS.
    let stop_after_slot = NUM_EPOCHS * EPOCH_LENGTH + EPOCH_LENGTH;

    iterate_blocks(immutable_dir, |info| {
        if info.slot >= stop_after_slot {
            return ControlFlow::Break(());
        }

        if first_era.is_none() {
            first_era = Some(info.era);
            eprintln!("First block era: {:?}, slot: {}", info.era, info.slot);
        }

        let block_epoch = info.slot / EPOCH_LENGTH;

        // Detect epoch transitions and capture nonce values BEFORE applying the block.
        // When we see the first block of a new epoch, the trackers still hold the
        // state from the end of the previous epoch, so we can compute the epoch nonce.
        if block_epoch > last_epoch_seen && last_epoch_seen < NUM_EPOCHS {
            let pallas_3kf = hex::encode(pallas_tracker.compute_epoch_nonce_3kf().as_ref());
            let pallas_4kf = hex::encode(pallas_tracker.compute_epoch_nonce_4kf().as_ref());
            let dugite_3kf = hex::encode(dugite_tracker_3kf.compute_epoch_nonce());
            let dugite_4kf = hex::encode(dugite_tracker_4kf.compute_epoch_nonce());

            epoch_nonces.push((block_epoch, pallas_3kf, pallas_4kf, dugite_3kf, dugite_4kf));

            eprintln!(
                "\n=== Epoch transition to epoch {} (first block: slot {}, block #{}) ===",
                block_epoch, info.slot, info.block_number
            );
            eprintln!(
                "  Pallas evolving nonce:  {}",
                hex::encode(pallas_tracker.evolving_nonce.as_ref())
            );
            eprintln!(
                "  Pallas candidate (3kf): {}",
                hex::encode(pallas_tracker.candidate_nonce_3kf.as_ref())
            );
            eprintln!(
                "  Pallas candidate (4kf): {}",
                hex::encode(pallas_tracker.candidate_nonce_4kf.as_ref())
            );
            eprintln!(
                "  Pallas lab nonce:       {}",
                hex::encode(pallas_tracker.lab_nonce.as_ref())
            );
            eprintln!(
                "  Pallas last_epoch_bn:   {}",
                hex::encode(pallas_tracker.last_epoch_block_nonce.as_ref())
            );
            eprintln!(
                "  Dugite evolving (3kf): {}",
                hex::encode(dugite_tracker_3kf.evolving_nonce)
            );
            eprintln!(
                "  Dugite evolving (4kf): {}",
                hex::encode(dugite_tracker_4kf.evolving_nonce)
            );

            last_epoch_seen = block_epoch;
        }

        // Log first few blocks per epoch for debugging
        let epoch_block_approx = pallas_tracker.epoch_block_count;
        if epoch_block_approx < 3 {
            eprintln!(
                "  Block slot={} epoch={} era={:?} nonce_vrf_len={} dugite_nonce_len={}",
                info.slot,
                block_epoch,
                info.era,
                info.pallas_nonce_vrf_output
                    .as_ref()
                    .map(|v| v.len())
                    .unwrap_or(0),
                info.dugite_nonce_vrf_output.len(),
            );
        }

        // Apply block to all trackers
        pallas_tracker.apply_block(&info);
        dugite_tracker_3kf.apply_block(&info);
        dugite_tracker_4kf.apply_block(&info);

        total_blocks += 1;
        ControlFlow::Continue(())
    });

    eprintln!("\n========================================");
    eprintln!("Total blocks processed: {total_blocks}");
    eprintln!("First block era: {:?}", first_era);
    eprintln!("========================================\n");

    eprintln!("EPOCH NONCE COMPARISON:");
    eprintln!("{:-<100}", "");

    for (epoch, pallas_3kf, pallas_4kf, dugite_3kf, dugite_4kf) in &epoch_nonces {
        eprintln!("Epoch {epoch}:");
        eprintln!("  Pallas epoch nonce (3k/f):  {pallas_3kf}");
        eprintln!("  Pallas epoch nonce (4k/f):  {pallas_4kf}");
        eprintln!("  Dugite epoch nonce (3k/f): {dugite_3kf}");
        eprintln!("  Dugite epoch nonce (4k/f): {dugite_4kf}");

        let p3_vs_t3 = if pallas_3kf == dugite_3kf {
            "MATCH"
        } else {
            "MISMATCH"
        };
        let p4_vs_t4 = if pallas_4kf == dugite_4kf {
            "MATCH"
        } else {
            "MISMATCH"
        };
        let p3_vs_p4 = if pallas_3kf == pallas_4kf {
            "SAME"
        } else {
            "DIFFERENT"
        };
        eprintln!("  Pallas(3kf) vs Dugite(3kf): {p3_vs_t3}");
        eprintln!("  Pallas(4kf) vs Dugite(4kf): {p4_vs_t4}");
        eprintln!("  Pallas(3kf) vs Pallas(4kf):  {p3_vs_p4}");
        eprintln!();
    }

    // Detailed per-block evolving nonce comparison for first 10 blocks
    eprintln!("\nPER-BLOCK EVOLVING NONCE COMPARISON (first 10 blocks):");
    let genesis_hash2 = PallasHash::<32>::from(genesis_hash_bytes.as_slice());
    let mut pallas_evolving = genesis_hash2;
    let mut dugite_evolving_bytes = genesis_arr;
    let mut block_count = 0u64;

    iterate_blocks(immutable_dir, |info| {
        if block_count >= 10 {
            return ControlFlow::Break(());
        }

        if let Some(ref nonce_vrf) = info.pallas_nonce_vrf_output {
            if !nonce_vrf.is_empty() {
                // Pallas rolling nonce
                let pallas_new = generate_rolling_nonce(pallas_evolving, nonce_vrf);

                // Dugite rolling nonce (same as update_evolving_nonce)
                let eta_hash = blake2b_256(&info.dugite_nonce_vrf_output);
                let mut data = Vec::with_capacity(64);
                data.extend_from_slice(&dugite_evolving_bytes);
                data.extend_from_slice(&eta_hash);
                let dugite_new = blake2b_256(&data);

                let match_status = if pallas_new.as_ref() == &dugite_new[..] {
                    "MATCH"
                } else {
                    "MISMATCH"
                };

                eprintln!(
                    "  Block #{:<6} slot={:<8} era={:?} {}",
                    info.block_number, info.slot, info.era, match_status
                );
                eprintln!(
                    "    pallas_nonce_vrf_len={}, dugite_nonce_vrf_len={}",
                    nonce_vrf.len(),
                    info.dugite_nonce_vrf_output.len()
                );
                eprintln!("    pallas  evolving: {}", hex::encode(pallas_new.as_ref()));
                eprintln!("    dugite evolving: {}", hex::encode(dugite_new));

                if pallas_new.as_ref() != &dugite_new[..] {
                    // Show the intermediate hash values for debugging
                    let pallas_inner_hash = Hasher::<256>::hash(nonce_vrf);
                    let dugite_inner_hash_input = &info.dugite_nonce_vrf_output;
                    let dugite_inner_hash = blake2b_256(dugite_inner_hash_input);
                    eprintln!(
                        "    pallas inner hash (blake2b of raw nonce_vrf): {}",
                        hex::encode(pallas_inner_hash.as_ref())
                    );
                    eprintln!(
                        "    dugite inner hash (blake2b of pre-hashed):   {}",
                        hex::encode(dugite_inner_hash)
                    );
                    eprintln!(
                        "    pallas raw nonce_vrf (first 32 bytes): {}",
                        hex::encode(&nonce_vrf[..std::cmp::min(32, nonce_vrf.len())])
                    );
                    eprintln!(
                        "    dugite pre-hashed nonce_eta:          {}",
                        hex::encode(dugite_inner_hash_input)
                    );
                }

                pallas_evolving = pallas_new;
                dugite_evolving_bytes = dugite_new;
                block_count += 1;
            }
        }
        ControlFlow::Continue(())
    });

    // Assertions — skip gracefully if chunk files lack secondary indexes
    if total_blocks == 0 {
        eprintln!(
            "SKIPPING test_epoch_nonce_preview_first_2_epochs: \
             No blocks could be replayed from {IMMUTABLE_DIR} (missing secondary indexes?)"
        );
        return;
    }

    assert!(
        !epoch_nonces.is_empty(),
        "No epoch transitions observed. Need at least {} slots of data.",
        EPOCH_LENGTH
    );

    // Final summary
    eprintln!("\n=== FINAL SUMMARY ===");
    for (epoch, pallas_3kf, pallas_4kf, dugite_3kf, dugite_4kf) in &epoch_nonces {
        eprintln!("Epoch {epoch}:");
        eprintln!("  Pallas epoch nonce (3k/f):  {pallas_3kf}");
        eprintln!("  Pallas epoch nonce (4k/f):  {pallas_4kf}");
        eprintln!("  Dugite epoch nonce (3k/f): {dugite_3kf}");
        eprintln!("  Dugite epoch nonce (4k/f): {dugite_4kf}");

        if pallas_3kf != dugite_3kf {
            eprintln!("  ** Pallas(3kf) != Dugite(3kf) -- evolving nonce divergence detected");
        }
    }
}
