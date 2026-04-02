//! Fuzz target for ledger snapshot bincode deserialization.
//!
//! Ledger snapshots use bincode serialization with a header format:
//!   [4 bytes]  magic  "DUGT"
//!   [1 byte]   version
//!   [32 bytes] blake2b-256 checksum
//!   [N bytes]  bincode payload (LedgerState)
//!
//! The `load_snapshot` function must handle corrupt/malicious data without
//! panicking: it validates the checksum before deserializing, and bincode
//! deserialization itself must not panic on arbitrary input.
//!
//! This target tests both:
//! 1. The snapshot header parsing and checksum validation path
//! 2. Direct bincode deserialization of raw bytes as LedgerState
//!
//! Run with: cargo +nightly fuzz run fuzz_ledger_snapshot -- -max_total_time=300

#![no_main]

use libfuzzer_sys::fuzz_target;
use dugite_ledger::LedgerState;

fuzz_target!(|data: &[u8]| {
    // --- Test 1: Direct bincode deserialization ---
    // This bypasses the snapshot header and tests the bincode decoder directly
    // against arbitrary input. bincode::deserialize must not panic.
    let _: Result<LedgerState, _> = bincode::deserialize(data);

    // --- Test 2: Snapshot with valid-looking header but corrupt payload ---
    // Construct a buffer that looks like a valid DUGT snapshot header
    // but has random payload data. This tests the checksum validation
    // and the graceful rejection of corrupt snapshots.
    if data.len() >= 33 {
        // Use first 32 bytes as fake checksum, rest as payload
        let mut snapshot_buf = Vec::with_capacity(4 + 1 + 32 + data.len());
        snapshot_buf.extend_from_slice(b"DUGT"); // magic
        snapshot_buf.push(5); // version (current)
        snapshot_buf.extend_from_slice(&data[..32]); // fake checksum
        snapshot_buf.extend_from_slice(&data[32..]); // payload

        // Write to a temp file and attempt to load
        // (load_snapshot reads from a file path)
        let dir = std::env::temp_dir().join("dugite-fuzz-snapshot");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!(
            "fuzz_{}.snap",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));

        if std::fs::write(&path, &snapshot_buf).is_ok() {
            // load_snapshot should not panic -- it validates checksum first,
            // and returns Err for corrupt data
            let _ = LedgerState::load_snapshot(&path);
            let _ = std::fs::remove_file(&path);
        }
    }

    // --- Test 3: Valid checksum, random bincode payload ---
    // This tests the path where the checksum passes but the bincode
    // data is garbage.
    if !data.is_empty() {
        let checksum = dugite_primitives::hash::blake2b_256(data);
        let mut snapshot_buf = Vec::with_capacity(4 + 1 + 32 + data.len());
        snapshot_buf.extend_from_slice(b"DUGT");
        snapshot_buf.push(5); // current version
        snapshot_buf.extend_from_slice(checksum.as_bytes());
        snapshot_buf.extend_from_slice(data);

        let dir = std::env::temp_dir().join("dugite-fuzz-snapshot");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!(
            "fuzz_valid_{}.snap",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));

        if std::fs::write(&path, &snapshot_buf).is_ok() {
            // This will pass checksum validation but likely fail bincode
            // deserialization -- must not panic
            let _ = LedgerState::load_snapshot(&path);
            let _ = std::fs::remove_file(&path);
        }
    }
});
