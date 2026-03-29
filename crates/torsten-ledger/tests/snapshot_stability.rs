//! Snapshot format stability tests.
//!
//! LedgerState uses bincode serialization (field-order-dependent, not self-describing).
//! Adding, removing, or reordering fields BREAKS deserialization of existing snapshots.
//! These tests detect accidental format changes by hashing the serialized output of a
//! canonical LedgerState and comparing against a known expected hash.

use torsten_ledger::LedgerState;
use torsten_primitives::protocol_params::ProtocolParameters;

/// Create a deterministic LedgerState with known default values.
fn canonical_ledger_state() -> LedgerState {
    LedgerState::new(ProtocolParameters::mainnet_defaults())
}

/// Round-trip: serialize → deserialize → serialize produces identical bytes.
#[test]
fn snapshot_round_trip_deterministic() {
    let state = canonical_ledger_state();

    let bytes1 = bincode::serialize(&state).expect("serialize 1");
    let state2: LedgerState = bincode::deserialize(&bytes1).expect("deserialize");
    let bytes2 = bincode::serialize(&state2).expect("serialize 2");

    assert_eq!(
        bytes1, bytes2,
        "Round-trip serialization produced different bytes — bincode format is not stable"
    );
}

/// Hash the serialized bytes and compare against a known value.
/// If this test fails, it means the LedgerState serialization format has changed,
/// which will break deserialization of existing snapshots on disk.
///
/// To update: run the test, copy the new hash from the failure message, and update
/// the EXPECTED_HASH constant below. Only do this intentionally when bumping
/// SNAPSHOT_VERSION.
#[test]
fn snapshot_format_hash_stability() {
    let state = canonical_ledger_state();
    let bytes = bincode::serialize(&state).expect("serialize");

    // blake2b-256 of the serialized bytes
    let hash = torsten_primitives::hash::blake2b_256(&bytes);
    let hash_hex = hash
        .as_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();

    // This hash was computed from the current LedgerState layout.
    // If this changes, existing snapshot files become unreadable.
    const EXPECTED_HASH: &str = "deff7957541b9ab67dc6382c02268aae1de7bc7f4dcd2ea8e76567c479b0cb9e";

    if EXPECTED_HASH == "COMPUTE_ON_FIRST_RUN" {
        panic!(
            "Snapshot format hash not yet set. Current hash: {hash_hex}\n\
             Update EXPECTED_HASH in this test with the value above."
        );
    }

    assert_eq!(
        hash_hex, EXPECTED_HASH,
        "LedgerState serialization format changed — existing snapshots will be incompatible.\n\
         If this change was intentional, update EXPECTED_HASH to: {hash_hex}\n\
         and bump SNAPSHOT_VERSION in state/mod.rs."
    );
}

/// Verify the snapshot header format (magic + version + checksum).
#[test]
fn snapshot_save_load_round_trip() {
    let state = canonical_ledger_state();
    let dir = tempfile::tempdir().expect("create temp dir");
    let path = dir.path().join("test_snapshot.bin");

    state.save_snapshot(&path).expect("save snapshot");

    let loaded = LedgerState::load_snapshot(&path).expect("load snapshot");

    // Compare key fields
    assert_eq!(state.epoch, loaded.epoch);
    assert_eq!(state.era, loaded.era);
    assert_eq!(state.treasury, loaded.treasury);
    assert_eq!(state.reserves, loaded.reserves);
    assert_eq!(state.epoch_length, loaded.epoch_length);
    assert_eq!(state.evolving_nonce, loaded.evolving_nonce);
    assert_eq!(state.epoch_nonce, loaded.epoch_nonce);
}

/// Verify that the snapshot file starts with the expected header.
#[test]
fn snapshot_file_has_correct_header() {
    let state = canonical_ledger_state();
    let dir = tempfile::tempdir().expect("create temp dir");
    let path = dir.path().join("test_snapshot.bin");

    state.save_snapshot(&path).expect("save snapshot");

    let raw = std::fs::read(&path).expect("read snapshot");
    assert!(raw.len() >= 37, "snapshot file too small");
    assert_eq!(&raw[0..4], b"TRSN", "missing TRSN magic");
    assert!(raw[4] > 0 && raw[4] < 128, "invalid version byte");
}
