//! Fuzz target for Phase-1 transaction validation.
//!
//! Constructs a minimal transaction and UTxO set from fuzz bytes, then runs
//! `validate_transaction()` to exercise the full Phase-1 validation pipeline.
//! Catches panics, overflows, and logic errors in validation code.
//!
//! Run with: cargo +nightly fuzz run fuzz_tx_validation -- -max_total_time=300

#![no_main]

use libfuzzer_sys::fuzz_target;
use std::collections::HashMap;

use dugite_ledger::validation::validate_transaction;
use dugite_ledger::utxo::UtxoLookup;
use dugite_primitives::address::Address;
use dugite_primitives::hash::Hash32;
use dugite_primitives::protocol_params::ProtocolParameters;
use dugite_primitives::time::SlotNo;
use dugite_primitives::transaction::{
    OutputDatum, Transaction, TransactionBody, TransactionInput, TransactionOutput,
    TransactionWitnessSet,
};
use dugite_primitives::value::{Lovelace, Value};

/// Simple wrapper around HashMap to implement UtxoLookup.
struct FuzzUtxoSet(HashMap<TransactionInput, TransactionOutput>);

impl UtxoLookup for FuzzUtxoSet {
    fn lookup(&self, input: &TransactionInput) -> Option<TransactionOutput> {
        self.0.get(input).cloned()
    }

    fn contains(&self, input: &TransactionInput) -> bool {
        self.0.contains_key(input)
    }
}

/// Helper: extract a 32-byte hash from the fuzz data at the given offset.
/// Pads with zeros if insufficient bytes remain.
fn read_hash32(data: &[u8], offset: usize) -> Hash32 {
    let mut bytes = [0u8; 32];
    let available = data.len().saturating_sub(offset).min(32);
    if available > 0 {
        bytes[..available].copy_from_slice(&data[offset..offset + available]);
    }
    Hash32::from_bytes(bytes)
}

/// Helper: read a u64 from 8 bytes at offset (little-endian).
fn read_u64(data: &[u8], offset: usize) -> u64 {
    let mut bytes = [0u8; 8];
    let available = data.len().saturating_sub(offset).min(8);
    if available > 0 {
        bytes[..available].copy_from_slice(&data[offset..offset + available]);
    }
    u64::from_le_bytes(bytes)
}

/// Helper: read a u32 from 4 bytes at offset (little-endian).
fn read_u32(data: &[u8], offset: usize) -> u32 {
    let mut bytes = [0u8; 4];
    let available = data.len().saturating_sub(offset).min(4);
    if available > 0 {
        bytes[..available].copy_from_slice(&data[offset..offset + available]);
    }
    u32::from_le_bytes(bytes)
}

/// Create a simple enterprise address (type 0x60, network 0) from a 28-byte hash.
fn make_enterprise_address(hash_bytes: &[u8; 28]) -> Address {
    let mut addr_bytes = vec![0x60u8]; // enterprise address, network 0
    addr_bytes.extend_from_slice(hash_bytes);
    Address::from_bytes(&addr_bytes).unwrap_or_else(|_| {
        // Fallback: use a known-good enterprise address
        Address::from_bytes(&[0x60; 29]).unwrap()
    })
}

fuzz_target!(|data: &[u8]| {
    // Need at least 80 bytes to construct a meaningful test:
    // 32 (tx hash) + 8 (fee) + 8 (ttl) + 32 (input tx hash) = 80
    if data.len() < 80 {
        return;
    }

    let params = ProtocolParameters::mainnet_defaults();

    // Parse fuzz data into transaction components
    let tx_hash = read_hash32(data, 0);
    let fee = read_u64(data, 32);
    let ttl_raw = read_u64(data, 40);
    let current_slot = read_u64(data, 48).wrapping_rem(1_000_000_000);

    // Number of inputs: 1-4 based on a control byte
    let num_inputs = ((data.get(56).copied().unwrap_or(0) % 4) + 1) as usize;
    // Number of outputs: 1-4 based on a control byte
    let num_outputs = ((data.get(57).copied().unwrap_or(0) % 4) + 1) as usize;

    let mut utxo_map = HashMap::new();
    let mut inputs = Vec::new();

    // Build inputs and corresponding UTxO entries starting at offset 58
    let mut offset = 58;
    for i in 0..num_inputs {
        if offset + 36 > data.len() {
            break;
        }
        let input_tx_hash = read_hash32(data, offset);
        let input_index = read_u32(data, offset + 32);
        offset += 36;

        let input = TransactionInput {
            transaction_id: input_tx_hash,
            index: input_index,
        };

        // Create a UTxO entry for this input with some ADA
        let utxo_ada = (read_u64(data, offset.min(data.len().saturating_sub(8)))
            .wrapping_rem(100_000_000_000))
        .max(2_000_000); // At least 2 ADA

        let addr_seed = [((i as u8).wrapping_mul(17)); 28];
        let utxo_output = TransactionOutput {
            address: make_enterprise_address(&addr_seed),
            value: Value::lovelace(utxo_ada),
            datum: OutputDatum::None,
            script_ref: None,
            is_legacy: false,
            raw_cbor: None,
        };

        utxo_map.insert(input.clone(), utxo_output);
        inputs.push(input);
    }

    if inputs.is_empty() {
        return;
    }

    // Build transaction outputs
    let mut outputs = Vec::new();
    for i in 0..num_outputs {
        let output_ada = (read_u64(
            data,
            (offset + i * 8).min(data.len().saturating_sub(8)),
        )
        .wrapping_rem(50_000_000_000))
        .max(1_000_000);

        let addr_seed = [((i as u8).wrapping_add(100).wrapping_mul(13)); 28];
        outputs.push(TransactionOutput {
            address: make_enterprise_address(&addr_seed),
            value: Value::lovelace(output_ada),
            datum: OutputDatum::None,
            script_ref: None,
            is_legacy: false,
            raw_cbor: None,
        });
    }

    // Build the transaction
    let tx = Transaction {
        hash: tx_hash,
        era: dugite_primitives::era::Era::Conway,
        body: TransactionBody {
            inputs,
            outputs,
            fee: Lovelace(fee),
            ttl: if ttl_raw > 0 {
                Some(SlotNo(ttl_raw))
            } else {
                None
            },
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
    };

    let utxo_set = FuzzUtxoSet(utxo_map);

    // Run Phase-1 validation — must never panic regardless of input.
    // ValidationError results are expected and silently dropped.
    let _ = validate_transaction(
        &tx,
        &utxo_set,
        &params,
        current_slot,
        data.len() as u64, // Use fuzz data length as an approximation of tx size
        None,               // No slot config (skip Plutus evaluation)
    );
});
