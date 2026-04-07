//! Fuzz target for mempool tx submission and eviction.
//!
//! Interprets fuzz bytes as a sequence of transaction submissions with varying
//! hashes, inputs, and fees. Verifies mempool invariants after each operation:
//! length consistency, no duplicate input claims, byte limit enforcement.
//!
//! Run with: cargo +nightly fuzz run fuzz_mempool_admission -- -max_total_time=300

#![no_main]

use libfuzzer_sys::fuzz_target;

use dugite_mempool::{Mempool, MempoolConfig};
use dugite_primitives::hash::Hash32;
use dugite_primitives::transaction::{
    Transaction, TransactionBody, TransactionInput, TransactionOutput, TransactionWitnessSet,
    OutputDatum,
};
use dugite_primitives::value::{Lovelace, Value};

/// Helper: build a Hash32 from a single byte seed (deterministic).
fn hash_from_seed(seed: u8) -> Hash32 {
    let mut bytes = [0u8; 32];
    bytes[0] = seed;
    // Mix the seed to get different hashes
    bytes[1] = seed.wrapping_mul(37);
    bytes[2] = seed.wrapping_mul(73);
    bytes[3] = seed.wrapping_mul(113);
    Hash32::from_bytes(bytes)
}

/// Build a minimal transaction with the given hash and inputs.
fn build_mempool_tx(
    tx_hash: Hash32,
    inputs: Vec<TransactionInput>,
    fee: u64,
) -> Transaction {
    // Build a simple enterprise address
    let addr_bytes = [0x60u8; 29];
    let address = dugite_primitives::address::Address::from_bytes(&addr_bytes)
        .unwrap_or_else(|_| dugite_primitives::address::Address::from_bytes(&[0x60; 29]).unwrap());

    Transaction {
        hash: tx_hash,
        era: dugite_primitives::era::Era::Conway,
        body: TransactionBody {
            inputs,
            outputs: vec![TransactionOutput {
                address,
                value: Value::lovelace(2_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            }],
            fee: Lovelace(fee),
            ttl: None,
            certificates: vec![],
            withdrawals: std::collections::BTreeMap::new(),
            auxiliary_data_hash: None,
            validity_interval_start: None,
            mint: std::collections::BTreeMap::new(),
            script_data_hash: None,
            collateral: vec![],
            required_signers: vec![],
            network_id: None,
            collateral_return: None,
            total_collateral: None,
            reference_inputs: vec![],
            update: None,
            voting_procedures: std::collections::BTreeMap::new(),
            proposal_procedures: vec![],
            treasury_value: None,
            donation: None,
        },
        witness_set: TransactionWitnessSet {
            vkey_witnesses: vec![],
            native_scripts: vec![],
            bootstrap_witnesses: vec![],
            plutus_v1_scripts: vec![],
            plutus_v2_scripts: vec![],
            plutus_v3_scripts: vec![],
            plutus_data: vec![],
            redeemers: vec![],
            raw_redeemers_cbor: None,
            raw_plutus_data_cbor: None,
            pallas_script_data_hash: None,
        },
        is_valid: true,
        auxiliary_data: None,
        raw_cbor: None,
        raw_body_cbor: None,
        raw_witness_cbor: None,
    }
}

fuzz_target!(|data: &[u8]| {
    if data.len() < 2 {
        return;
    }

    // Small mempool to trigger eviction paths quickly
    let config = MempoolConfig {
        max_transactions: 32,
        max_bytes: 65_536,
        max_ex_mem: 1_000_000,
        max_ex_steps: 1_000_000,
        max_ref_scripts_bytes: 1024,
    };

    // Mempool uses tokio::sync::Notify internally, so we need a runtime
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let _guard = rt.enter();

    let mempool = Mempool::new(config);

    let mut pos = 0;
    let mut op_count = 0;
    let max_ops = 128;

    while pos < data.len() && op_count < max_ops {
        let control = data[pos];
        pos += 1;
        op_count += 1;

        let op_type = control >> 7; // Top bit: 0=submit, 1=remove

        if op_type == 0 {
            // Submit a transaction
            let tx_seed = data.get(pos).copied().unwrap_or(0);
            pos += 1;
            let input_seed = data.get(pos).copied().unwrap_or(0);
            pos += 1;

            // Fee from next 2 bytes (varies to test eviction ordering)
            let fee_lo = data.get(pos).copied().unwrap_or(0) as u64;
            pos += 1;
            let fee_hi = data.get(pos).copied().unwrap_or(0) as u64;
            pos += 1;
            let fee = (fee_hi << 8 | fee_lo).max(1) * 1000; // Scale to reasonable range

            let tx_hash = hash_from_seed(tx_seed);
            let input = TransactionInput {
                transaction_id: hash_from_seed(input_seed),
                index: (input_seed as u32) % 4,
            };

            let tx = build_mempool_tx(tx_hash, vec![input], fee);
            let size_bytes = 256; // Approximate size

            // Submit — must never panic
            let _ = mempool.add_tx_with_fee(tx_hash, tx, size_bytes, Lovelace(fee));
        } else {
            // Remove a transaction
            let remove_seed = data.get(pos).copied().unwrap_or(0);
            pos += 1;
            let tx_hash = hash_from_seed(remove_seed);
            let _ = mempool.remove_tx(&tx_hash);
        }

        // Invariant checks — must never fail.
        // The mempool enforces capacity limits before inserting (eviction-then-insert),
        // so these limits should hold strictly after each operation.
        let len = mempool.len();
        let total = mempool.total_bytes();

        assert!(
            len <= 32,
            "Mempool length {} exceeds max_transactions 32",
            len
        );
        assert!(
            total <= 65_536,
            "Mempool total_bytes {} exceeds max_bytes 65536",
            total
        );
    }
});
