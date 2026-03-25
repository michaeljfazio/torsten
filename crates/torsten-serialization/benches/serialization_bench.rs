//! Criterion benchmarks for CBOR serialization.
//!
//! Benchmarks encoding of realistic Cardano types at mainnet scale:
//! - Transaction encoding (Conway era, 2 inputs, 2 outputs, witnesses)
//! - Block header encoding with VRF output
//! - Value encoding (ADA-only vs multi-asset)
//!
//! Run:  cargo bench -p torsten-serialization
//! HTML: target/criterion/report/index.html

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use std::collections::BTreeMap;
use torsten_primitives::address::{Address, EnterpriseAddress};
use torsten_primitives::block::{BlockHeader, OperationalCert, ProtocolVersion, VrfOutput};
use torsten_primitives::credentials::Credential;
use torsten_primitives::hash::{Hash28, Hash32};
use torsten_primitives::network::NetworkId;
use torsten_primitives::time::{BlockNo, SlotNo};
use torsten_primitives::transaction::{
    OutputDatum, Transaction, TransactionBody, TransactionInput, TransactionOutput,
    TransactionWitnessSet, VKeyWitness,
};
use torsten_primitives::value::{AssetName, Lovelace, Value};
use torsten_serialization::encode::{
    encode_block_header_body, encode_transaction, encode_transaction_body, encode_value,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_enterprise_address(seed: u8) -> Address {
    Address::Enterprise(EnterpriseAddress {
        network: NetworkId::Mainnet,
        payment: Credential::VerificationKey(Hash28::from_bytes([seed; 28])),
    })
}

fn make_tx_input(seed: u64) -> TransactionInput {
    let mut bytes = [0u8; 32];
    bytes[0..8].copy_from_slice(&seed.to_le_bytes());
    TransactionInput {
        transaction_id: Hash32::from_bytes(bytes),
        index: (seed % 4) as u32,
    }
}

fn make_tx_output_ada(seed: u8, lovelace: u64) -> TransactionOutput {
    TransactionOutput {
        address: make_enterprise_address(seed),
        value: Value::lovelace(lovelace),
        datum: OutputDatum::None,
        script_ref: None,
        raw_cbor: None,
        is_legacy: false,
    }
}

/// Build a realistic Conway transaction: 2 inputs, 2 outputs, 2 witnesses.
fn make_realistic_transaction() -> Transaction {
    Transaction {
        era: torsten_primitives::era::Era::Conway,
        hash: Hash32::from_bytes([0xAA; 32]),
        body: TransactionBody {
            inputs: vec![make_tx_input(1), make_tx_input(2)],
            outputs: vec![
                make_tx_output_ada(0x01, 5_000_000),
                make_tx_output_ada(0x02, 1_500_000),
            ],
            fee: Lovelace(200_000),
            ttl: Some(SlotNo(50_000_000)),
            certificates: vec![],
            withdrawals: BTreeMap::new(),
            auxiliary_data_hash: None,
            validity_interval_start: None,
            mint: BTreeMap::new(),
            script_data_hash: None,
            collateral: vec![],
            required_signers: vec![],
            network_id: None,
            collateral_return: None,
            total_collateral: None,
            reference_inputs: vec![],
            update: None,
            voting_procedures: BTreeMap::new(),
            proposal_procedures: vec![],
            treasury_value: None,
            donation: None,
        },
        witness_set: TransactionWitnessSet {
            vkey_witnesses: vec![
                VKeyWitness {
                    vkey: vec![0x11; 32],
                    signature: vec![0x22; 64],
                },
                VKeyWitness {
                    vkey: vec![0x33; 32],
                    signature: vec![0x44; 64],
                },
            ],
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
    }
}

fn make_block_header() -> BlockHeader {
    BlockHeader {
        header_hash: Hash32::from_bytes([0xBB; 32]),
        prev_hash: Hash32::from_bytes([0xCC; 32]),
        issuer_vkey: vec![0x01; 32],
        vrf_vkey: vec![0x02; 32],
        vrf_result: VrfOutput {
            output: vec![0x03; 64],
            proof: vec![0x04; 80],
        },
        nonce_vrf_output: vec![],
        block_number: BlockNo(10_000_000),
        slot: SlotNo(130_000_000),
        epoch_nonce: Hash32::from_bytes([0xDD; 32]),
        body_size: 20_480,
        body_hash: Hash32::from_bytes([0xEE; 32]),
        operational_cert: OperationalCert {
            hot_vkey: vec![0x05; 32],
            sequence_number: 42,
            kes_period: 350,
            sigma: vec![0x06; 64],
        },
        protocol_version: ProtocolVersion {
            major: 10,
            minor: 0,
        },
        kes_signature: vec![],
    }
}

fn make_ada_only_value() -> Value {
    Value::lovelace(2_000_000)
}

fn make_multi_asset_value() -> Value {
    let mut multi_asset = BTreeMap::new();

    // 3 policies, ~5 assets total
    for p in 0..3u8 {
        let policy = Hash28::from_bytes([p + 1; 28]);
        let mut assets = BTreeMap::new();
        let asset_count = if p == 0 { 3 } else { 1 };
        for a in 0..asset_count {
            let name = AssetName::new(format!("Token{p}{a}").into_bytes()).unwrap();
            assets.insert(name, 1_000_000 + a as u64 * 500);
        }
        multi_asset.insert(policy, assets);
    }

    Value {
        coin: Lovelace(5_000_000),
        multi_asset,
    }
}

// ===========================================================================
// Benchmarks
// ===========================================================================

fn bench_encode_transaction(c: &mut Criterion) {
    let mut group = c.benchmark_group("serialization/encode_transaction");

    let tx = make_realistic_transaction();

    group.bench_function("conway_2in_2out_2wit", |b| {
        b.iter(|| {
            let encoded = encode_transaction(black_box(&tx));
            black_box(encoded.len());
        });
    });

    // Also benchmark just the body (used for hashing)
    group.bench_function("body_only_2in_2out", |b| {
        b.iter(|| {
            let encoded = encode_transaction_body(black_box(&tx.body));
            black_box(encoded.len());
        });
    });

    group.finish();
}

fn bench_encode_block_header(c: &mut Criterion) {
    let mut group = c.benchmark_group("serialization/encode_block_header");

    let header = make_block_header();

    group.bench_function("with_vrf_output", |b| {
        b.iter(|| {
            let encoded = encode_block_header_body(black_box(&header));
            black_box(encoded.len());
        });
    });

    group.finish();
}

fn bench_encode_value(c: &mut Criterion) {
    let mut group = c.benchmark_group("serialization/encode_value");

    let ada_only = make_ada_only_value();
    group.bench_function("ada_only", |b| {
        b.iter(|| {
            let encoded = encode_value(black_box(&ada_only));
            black_box(encoded.len());
        });
    });

    let multi_asset = make_multi_asset_value();
    group.bench_function("multi_asset_3policy_5asset", |b| {
        b.iter(|| {
            let encoded = encode_value(black_box(&multi_asset));
            black_box(encoded.len());
        });
    });

    group.finish();
}

// ===========================================================================
// Criterion harness
// ===========================================================================

criterion_group!(
    serialization_benches,
    bench_encode_transaction,
    bench_encode_block_header,
    bench_encode_value,
);
criterion_main!(serialization_benches);
