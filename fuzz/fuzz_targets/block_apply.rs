//! Fuzz target for block application to ledger state.
//!
//! Constructs a minimal Block from fuzz bytes and applies it to a fresh
//! LedgerState via `apply_block()`. Catches state corruption and panic
//! paths in the block application pipeline.
//!
//! Run with: cargo +nightly fuzz run fuzz_block_apply -- -max_total_time=300

#![no_main]

use libfuzzer_sys::fuzz_target;

use dugite_ledger::state::{BlockValidationMode, LedgerState};
use dugite_primitives::block::{Block, BlockHeader, OperationalCert, ProtocolVersion};
use dugite_primitives::time::{BlockNo, SlotNo};
use dugite_primitives::era::Era;
use dugite_primitives::hash::Hash32;
use dugite_primitives::protocol_params::ProtocolParameters;
use dugite_primitives::transaction::{
    OutputDatum, Transaction, TransactionBody, TransactionInput, TransactionOutput,
    TransactionWitnessSet,
};
use dugite_primitives::value::{Lovelace, Value};

/// Helper: extract a 32-byte hash from the fuzz data at the given offset.
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

/// Build a minimal transaction from a slice of fuzz bytes.
fn build_tx(data: &[u8], seed: u8) -> Transaction {
    let hash = {
        let mut bytes = [seed; 32];
        let available = data.len().min(32);
        if available > 0 {
            bytes[..available].copy_from_slice(&data[..available]);
        }
        Hash32::from_bytes(bytes)
    };

    // Construct an enterprise address (type 0x60, network 0)
    let addr_bytes = [0x60u8; 29];
    let address = dugite_primitives::address::Address::from_bytes(&addr_bytes)
        .unwrap_or_else(|_| dugite_primitives::address::Address::from_bytes(&[0x60; 29]).unwrap());

    let fee_val = if data.len() >= 8 {
        u64::from_le_bytes(data[..8].try_into().unwrap_or([0; 8])) % 10_000_000
    } else {
        200_000
    };

    Transaction {
        hash,
        era: Era::Conway,
        body: TransactionBody {
            inputs: vec![TransactionInput {
                transaction_id: hash,
                index: 0,
            }],
            outputs: vec![TransactionOutput {
                address,
                value: Value::lovelace(2_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            }],
            fee: Lovelace(fee_val),
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
    // Need at least 72 bytes: 32 (header_hash) + 32 (prev_hash) + 8 (slot/block_no)
    if data.len() < 72 {
        return;
    }

    let mut ledger_state = LedgerState::new(ProtocolParameters::mainnet_defaults());

    // Parse block header fields from fuzz data
    let header_hash = read_hash32(data, 0);
    let prev_hash = read_hash32(data, 32);
    let slot = read_u64(data, 64) % 100_000_000;
    let block_number = read_u64(data, 64).wrapping_shr(32) % 10_000_000;
    let body_size = read_u64(data, 72.min(data.len().saturating_sub(8))) % 1_000_000;

    // Control byte for number of transactions (0-4)
    let num_txs = (data.get(72).copied().unwrap_or(0) % 5) as usize;

    // Build transactions from remaining fuzz data
    let tx_data_start = 73;
    let mut transactions = Vec::new();
    let tx_chunk_size = if num_txs > 0 {
        data.len().saturating_sub(tx_data_start) / num_txs.max(1)
    } else {
        0
    };

    for i in 0..num_txs {
        let start = tx_data_start + i * tx_chunk_size;
        let end = if i + 1 < num_txs {
            tx_data_start + (i + 1) * tx_chunk_size
        } else {
            data.len()
        };
        if start < end {
            transactions.push(build_tx(&data[start..end], i as u8));
        }
    }

    let block = Block {
        header: BlockHeader {
            header_hash,
            prev_hash,
            issuer_vkey: vec![0u8; 32],
            vrf_vkey: vec![0u8; 32],
            vrf_result: dugite_primitives::block::VrfOutput {
                output: vec![0u8; 64],
                proof: vec![0u8; 80],
            },
            block_number: BlockNo(block_number),
            slot: SlotNo(slot),
            epoch_nonce: Hash32::ZERO,
            body_size,
            body_hash: Hash32::ZERO,
            operational_cert: OperationalCert {
                hot_vkey: vec![0u8; 32],
                sequence_number: 0,
                kes_period: 0,
                sigma: vec![0u8; 64],
            },
            protocol_version: ProtocolVersion {
                major: 10,
                minor: 0,
            },
            kes_signature: vec![0u8; 448],
            nonce_vrf_output: vec![],
            nonce_vrf_proof: vec![],
        },
        transactions,
        era: Era::Conway,
        raw_cbor: None,
    };

    // Apply block to ledger state — must never panic.
    // LedgerError results are expected and silently dropped.
    let _ = ledger_state.apply_block(&block, BlockValidationMode::ValidateAll);
});
