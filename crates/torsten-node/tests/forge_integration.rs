//! Integration tests for the block forging pipeline.
//!
//! These tests are fully offline — no network, no database, no running node.
//! They exercise `forge_block()` with synthetic keys and verify:
//!
//! 1. The produced block has the correct Cardano wire-format CBOR structure.
//! 2. The VRF proof embedded in the forged header is cryptographically valid.
//! 3. Block header hashes are deterministic (same inputs → same hash).
//! 4. The KES signature embedded in the forged header verifies against the
//!    KES public key that was used at key generation time.
//! 5. Opcert counter tracking in `OuroborosPraos` correctly accepts valid
//!    counters and rejects regressions.

use torsten_consensus::{OuroborosPraos, ValidationMode};
use torsten_crypto::{kes, vrf};
use torsten_node::forge::{forge_block, BlockProducerConfig, BlockProducerCredentials};
use torsten_primitives::{
    block::{BlockHeader, OperationalCert, ProtocolVersion, VrfOutput},
    hash::{blake2b_256, Hash32},
    time::{BlockNo, SlotNo},
};

// ---------------------------------------------------------------------------
// Shared helper: build synthetic BlockProducerCredentials from scratch.
//
// All key material is deterministic from fixed seeds so that tests are
// hermetic and reproducible. No disk I/O is performed.
// ---------------------------------------------------------------------------

/// Construct a complete set of synthetic block producer credentials.
///
/// Seeds are hardcoded so each test run is identical. The VRF and KES key
/// pairs are generated in-process; the opcert sigma is a dummy 64-byte value
/// (we do not test Ed25519 opcert signing here — that is covered by the
/// `validate_operational_cert` unit tests in `torsten-consensus`).
fn synthetic_credentials() -> BlockProducerCredentials {
    // VRF key pair — deterministic from a fixed seed
    const VRF_SEED: [u8; 32] = [0xAA; 32];
    let vrf_kp = vrf::generate_vrf_keypair_from_secret(&VRF_SEED);

    // KES key pair — deterministic from a fixed seed
    const KES_SEED: [u8; 32] = [0xBB; 32];
    let (kes_sk, kes_pk) = kes::kes_keygen(&KES_SEED).expect("KES keygen should succeed");

    // Cold key — simulate an Ed25519 verification key (32 bytes)
    // We use a fixed pattern; the pool_id is derived from this.
    let cold_vkey = vec![0xCC_u8; 32];
    let pool_id = torsten_primitives::hash::blake2b_224(&cold_vkey);

    // The opcert sigma is dummy (all zeros). Signature verification is NOT
    // exercised in these tests — only structural / cryptographic VRF+KES checks.
    let opcert_sigma = vec![0u8; 64];

    BlockProducerCredentials {
        vrf_skey: vrf_kp.secret_key,
        vrf_vkey: vrf_kp.public_key,
        cold_vkey,
        kes_skey: kes_sk,
        kes_vkey: kes_pk.to_vec(),
        opcert_sequence: 0,
        opcert_kes_period: 0,
        opcert_sigma,
        pool_id,
    }
}

/// Default `BlockProducerConfig` for Conway era, protocol version 10.0.
fn conway_config() -> BlockProducerConfig {
    BlockProducerConfig {
        protocol_version: ProtocolVersion {
            major: 10,
            minor: 0,
        },
        era: torsten_primitives::era::Era::Conway,
        // Standard mainnet value — slots_per_kes_period must be consistent
        // with opcert_kes_period=0 so that offset = 0 and no key evolution
        // is needed.
        slots_per_kes_period: 129_600,
        ..BlockProducerConfig::default()
    }
}

// ---------------------------------------------------------------------------
// Test 1 — CBOR structure
// ---------------------------------------------------------------------------

/// A forged Conway block must serialise as a CBOR array(2): [era_tag, block_body].
///
/// The block body is itself an array(5):
///   [header, tx_bodies, tx_witnesses, aux_map, invalid_txs]
///
/// This verifies the outermost wire format only — pallas' decoder is the
/// authoritative Cardano wire format consumer and is used here to parse the
/// produced bytes.
#[test]
fn test_forge_block_structure() {
    let creds = synthetic_credentials();
    let config = conway_config();
    let epoch_nonce = Hash32::from_bytes([0x42_u8; 32]);

    let (_block, cbor) = forge_block(
        &creds,
        &config,
        SlotNo(1_000),
        BlockNo(100),
        Hash32::ZERO,
        &epoch_nonce,
        vec![],
    )
    .expect("forge_block must succeed with valid credentials");

    // The produced CBOR must be non-empty and parseable.
    assert!(!cbor.is_empty(), "Forged block CBOR must not be empty");

    // Validate the outer CBOR structure with minicbor:
    // array(2) → [era_tag: uint, array(5)]
    let mut decoder = minicbor::Decoder::new(&cbor);

    let outer_len = decoder.array().expect("Top-level CBOR must be an array");
    assert_eq!(
        outer_len,
        Some(2),
        "Outer array must have exactly 2 elements: [era_tag, block_body]"
    );

    // Era tag: Conway = 7
    let era_tag: u64 = decoder.u64().expect("First element must be a uint era tag");
    assert_eq!(era_tag, 7, "Conway era tag must be 7 (got {era_tag})");

    // Block body: array(5)
    let body_len = decoder
        .array()
        .expect("Second element must be the block body array");
    assert_eq!(
        body_len,
        Some(5),
        "Block body must be array(5): [header, tx_bodies, tx_witnesses, aux_map, invalid_txs]"
    );

    // The first element of the block body is the header: array(2)
    let header_len = decoder
        .array()
        .expect("First block-body element must be the header array");
    assert_eq!(
        header_len,
        Some(2),
        "Header must be array(2): [header_body, kes_signature]"
    );

    // The header body itself is array(10)
    let header_body_len = decoder.array().expect("Header body must be an array");
    assert_eq!(
        header_body_len,
        Some(10),
        "Header body must be array(10) per Babbage/Conway spec"
    );
}

// ---------------------------------------------------------------------------
// Test 2 — VRF proof validity
// ---------------------------------------------------------------------------

/// The VRF proof embedded in the forged block header must verify against the
/// VRF public key and the correct VRF input (slot + epoch nonce).
///
/// This checks end-to-end: forge_block() generates the proof internally via
/// `torsten_crypto::vrf::generate_vrf_proof`; we then re-derive the expected
/// seed and call `verify_vrf_proof` to confirm the proof is valid.
#[test]
fn test_vrf_proof_valid_for_forged_block() {
    let creds = synthetic_credentials();
    let config = conway_config();
    let epoch_nonce = Hash32::from_bytes([0x11_u8; 32]);
    let slot = SlotNo(50_000);

    let (block, _cbor) = forge_block(
        &creds,
        &config,
        slot,
        BlockNo(500),
        Hash32::ZERO,
        &epoch_nonce,
        vec![],
    )
    .expect("forge_block must succeed");

    // Re-derive the VRF seed exactly as forge_block does:
    //   seed = blake2b_256(slot_u64_BE || epoch_nonce)
    let expected_seed = torsten_consensus::slot_leader::vrf_input(&epoch_nonce, slot);

    // The VRF proof stored in the header must be exactly 80 bytes.
    let proof = &block.header.vrf_result.proof;
    assert_eq!(
        proof.len(),
        80,
        "VRF proof in header must be 80 bytes, got {}",
        proof.len()
    );

    // The VRF output stored in the header must be exactly 64 bytes.
    let output = &block.header.vrf_result.output;
    assert_eq!(
        output.len(),
        64,
        "VRF output in header must be 64 bytes, got {}",
        output.len()
    );

    // Verify the proof against the VRF public key.
    let verified_output = vrf::verify_vrf_proof(&creds.vrf_vkey, proof, &expected_seed)
        .expect("VRF proof in forged block header must verify");

    // The output returned by verification must match what was embedded.
    assert_eq!(
        verified_output.as_ref(),
        output.as_slice(),
        "VRF output from verification must match the output stored in the header"
    );
}

// ---------------------------------------------------------------------------
// Test 3 — Header hash stability (determinism)
// ---------------------------------------------------------------------------

/// Forging two blocks with identical inputs must produce the same header hash.
///
/// This validates that the header body encoding and hashing are deterministic —
/// no timestamp, random nonce, or other non-deterministic value is injected.
/// VRF proofs in Ouroboros Praos are also deterministic for a given (sk, seed)
/// pair, so every field of the header body is fully determined by the inputs.
#[test]
fn test_forged_block_header_hash_stable() {
    let creds = synthetic_credentials();
    let config = conway_config();
    let epoch_nonce = Hash32::from_bytes([0x77_u8; 32]);

    let forge = || {
        forge_block(
            &creds,
            &config,
            SlotNo(9_999),
            BlockNo(999),
            Hash32::from_bytes([0x55_u8; 32]),
            &epoch_nonce,
            vec![],
        )
        .expect("forge_block must succeed")
    };

    let (block_a, cbor_a) = forge();
    let (block_b, cbor_b) = forge();

    assert_eq!(
        block_a.header.header_hash, block_b.header.header_hash,
        "Header hashes must be identical for identical inputs"
    );

    assert_eq!(
        cbor_a, cbor_b,
        "Serialised block bytes must be byte-for-byte identical for identical inputs"
    );

    // Also verify that the header hash is actually the blake2b-256 of the
    // encoded header body — i.e. the field is correctly populated.
    let recomputed_header_body = torsten_serialization::encode_block_header_body(&block_a.header);
    let expected_hash = blake2b_256(&recomputed_header_body);
    assert_eq!(
        block_a.header.header_hash, expected_hash,
        "header_hash field must equal blake2b_256(encode_block_header_body(header))"
    );
}

// ---------------------------------------------------------------------------
// Test 4 — KES signature verification
// ---------------------------------------------------------------------------

/// The KES signature embedded in the forged block header must verify against
/// the KES public key derived at keygen time.
///
/// Sum6Kes at period 0 (no evolution needed when opcert_kes_period = 0 and
/// current slot KES period = slot / slots_per_kes_period = 0).
#[test]
fn test_kes_signature_verifies() {
    let creds = synthetic_credentials();
    let config = conway_config();
    let epoch_nonce = Hash32::from_bytes([0x33_u8; 32]);

    // Use a slot in KES period 0 to avoid needing key evolution.
    // slots_per_kes_period = 129_600, so slots 0..=129_599 are period 0.
    let slot = SlotNo(100);

    let (block, _cbor) = forge_block(
        &creds,
        &config,
        slot,
        BlockNo(1),
        Hash32::ZERO,
        &epoch_nonce,
        vec![],
    )
    .expect("forge_block must succeed");

    // The KES signature in the header must be 448 bytes (Sum6KesSig).
    let kes_sig = &block.header.kes_signature;
    assert_eq!(
        kes_sig.len(),
        448,
        "KES signature must be 448 bytes (Sum6KesSig), got {}",
        kes_sig.len()
    );

    // Re-encode the header body (this is what was signed).
    let header_body = torsten_serialization::encode_block_header_body(&block.header);

    // Extract the KES public key — must be exactly 32 bytes.
    assert_eq!(creds.kes_vkey.len(), 32, "KES public key must be 32 bytes");
    let mut pk_bytes = [0u8; 32];
    pk_bytes.copy_from_slice(&creds.kes_vkey);

    // KES period used for signing: the slot falls in period 0, opcert_kes_period = 0,
    // so kes_period_offset = 0. The signature was made at absolute period 0.
    let signing_period: u32 = 0;

    kes::kes_verify_bytes(&pk_bytes, signing_period, kes_sig, &header_body)
        .expect("KES signature in forged block header must verify at period 0");
}

// ---------------------------------------------------------------------------
// Test 5 — Opcert counter tracking
// ---------------------------------------------------------------------------

/// `OuroborosPraos::validate_header_full` tracks the highest opcert sequence
/// number seen per pool and enforces:
///
///   m ≤ n ≤ m + 1   (where m = last_seen, n = new counter)
///
/// In strict verification mode, a regression (n < m) must return
/// `ConsensusError::OpcertSequenceRegression`, and an over-increment (n > m+1)
/// must return `ConsensusError::OpcertCounterOverIncremented`.
///
/// This test:
/// 1. Accepts a block with counter = 0  (first seen for this pool).
/// 2. Accepts a block with counter = 1  (valid increment by 1).
/// 3. Rejects a block with counter = 0  (regression after seeing 1).
/// 4. Accepts a block with counter = 1  (same as last seen — valid per spec).
/// 5. Rejects a block with counter = 3  (over-increment by 2).
#[test]
fn test_opcert_counter_tracking() {
    use torsten_consensus::praos::{BlockIssuerInfo, ConsensusError};

    // The VRF key we embed in every header — must be exactly 32 bytes.
    let vrf_vkey = vec![0x42_u8; 32];

    // The issuer_info must have vrf_keyhash = blake2b_256(vrf_vkey) so that
    // the VRF key binding check passes in strict mode.
    let vrf_keyhash = blake2b_256(&vrf_vkey);
    let issuer_info = BlockIssuerInfo {
        vrf_keyhash,
        // 100% relative stake: the leader eligibility check will pass trivially
        // since phi_f(1.0, 0.05) = 0.05, and any VRF leader value below that
        // threshold passes. With all-zero VRF output the leader value
        // blake2b_256("L"||[0;64]) is deterministic; we skip this check by
        // setting snapshots_established = false so it is non-fatal during replay.
        relative_stake: 1.0,
    };

    // Use a fixed 32-byte issuer_vkey so all headers are attributed to the
    // same pool (pool_id = blake2b_224(issuer_vkey)).
    let issuer_vkey = vec![0xDE_u8; 32];

    // Build a header template we can reuse with different counter values.
    // We use ValidationMode::Replay to skip all expensive crypto checks — only
    // structural and counter checks run. This matches Haskell's
    // `reupdateChainDepState` path used for locally-stored blocks.
    let make_header = |counter: u64| BlockHeader {
        header_hash: Hash32::ZERO,
        prev_hash: Hash32::ZERO,
        issuer_vkey: issuer_vkey.clone(),
        vrf_vkey: vrf_vkey.clone(),
        vrf_result: VrfOutput {
            output: vec![0u8; 64],
            proof: vec![0u8; 80],
        },
        nonce_vrf_output: vec![],
        block_number: BlockNo(1),
        // slot must be ≤ current_slot to avoid FutureBlock error.
        slot: SlotNo(1_000),
        epoch_nonce: Hash32::ZERO,
        body_size: 0,
        body_hash: Hash32::ZERO,
        operational_cert: OperationalCert {
            hot_vkey: vec![0u8; 32],
            sequence_number: counter,
            kes_period: 0,
            sigma: vec![0u8; 64],
        },
        protocol_version: ProtocolVersion {
            major: 10,
            minor: 0,
        },
        kes_signature: vec![0u8; 448],
    };

    // current_slot must be ≥ block slot to avoid FutureBlock error.
    let current_slot = SlotNo(1_000_000);

    // Build a praos engine in strict mode.
    // nonce_established = true: VRF nonce errors are fatal.
    // snapshots_established = false: VRF leader eligibility is non-fatal
    // (we don't have real stake snapshots — only the counter matters here).
    let mut praos = OuroborosPraos::new();
    praos.set_strict_verification(true);
    praos.nonce_established = true;
    praos.snapshots_established = false; // keeps leader eligibility non-fatal

    // Helper: call validate_header_full in Replay mode with pool info supplied.
    // Returns the Result for the caller to assert on.
    let check = |praos: &mut OuroborosPraos, counter: u64| {
        praos.validate_header_full(
            &make_header(counter),
            current_slot,
            Some(&issuer_info),
            ValidationMode::Replay,
        )
    };

    // Step 1: counter = 0 → accepted (first observation for this pool, no prior state).
    assert!(
        check(&mut praos, 0).is_ok(),
        "Counter 0 must be accepted as the first observation for this pool"
    );

    // Step 2: counter = 1 → accepted (valid +1 increment from 0).
    assert!(
        check(&mut praos, 1).is_ok(),
        "Counter 1 must be accepted as a valid +1 increment from 0"
    );

    // Step 3: counter = 0 → regression — must be rejected in strict mode.
    // Last seen was 1, so 0 < 1 triggers OpcertSequenceRegression.
    let result = check(&mut praos, 0);
    assert!(
        result.is_err(),
        "Counter 0 after seeing 1 must be rejected in strict mode (regression)"
    );
    match result.unwrap_err() {
        ConsensusError::OpcertSequenceRegression { got, expected } => {
            assert_eq!(got, 0, "Regression error must report got=0");
            assert_eq!(
                expected, 1,
                "Regression error must report expected=1 (last seen)"
            );
        }
        other => panic!("Expected OpcertSequenceRegression, got: {other:?}"),
    }

    // Step 4: counter = 1 → same as last seen — still valid (m ≤ n ≤ m+1,
    // with m=1 and n=1 satisfying 1 ≤ 1 ≤ 2).
    assert!(
        check(&mut praos, 1).is_ok(),
        "Counter equal to last seen (1 == 1) must be accepted per the Praos m≤n≤m+1 rule"
    );

    // Step 5: counter = 3 → over-increment by 2 — must be rejected.
    // Last seen is still 1, so 3 > 1+1 triggers OpcertCounterOverIncremented.
    let result = check(&mut praos, 3);
    assert!(
        result.is_err(),
        "Counter 3 after seeing 1 must be rejected (over-increment by 2)"
    );
    match result.unwrap_err() {
        ConsensusError::OpcertCounterOverIncremented { got, last_seen } => {
            assert_eq!(got, 3, "Over-increment error must report got=3");
            assert_eq!(last_seen, 1, "Over-increment error must report last_seen=1");
        }
        other => panic!("Expected OpcertCounterOverIncremented, got: {other:?}"),
    }
}
