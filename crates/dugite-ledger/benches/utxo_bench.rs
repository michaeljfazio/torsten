//! Criterion benchmarks for the UTxO store.
//!
//! Covers: insert, lookup, remove, contains, apply_transaction, and
//! total_lovelace scan with both default and custom LSM configurations.
//!
//! Scales are based on Cardano mainnet reference numbers:
//! - UTxO set: ~15-20M entries
//! - Multi-asset UTxOs: ~30% of all UTxOs
//! - Transactions per block: ~50 average (20-300 range)
//! - Transaction shape: 2-5 inputs, 2-3 outputs
//!
//! Default benchmarks run at 100K-1M; larger scales (5M, 10M) are `#[ignore]`-gated
//! behind the `utxo_large_scale_benches` group (not included in default criterion_main).
//!
//! Run:  cargo bench -p dugite-ledger
//! HTML: target/criterion/report/index.html

use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion};
use dugite_ledger::utxo_store::UtxoStore;
use dugite_primitives::hash::{Hash28, Hash32};
use dugite_primitives::transaction::{OutputDatum, TransactionInput, TransactionOutput};
use dugite_primitives::value::{AssetName, Lovelace, Value as TxValue};
use std::collections::BTreeMap;
use std::hint::black_box;

// ---------------------------------------------------------------------------
// Constants — mainnet-representative scales
// ---------------------------------------------------------------------------

/// Core benchmark sizes: 1M and 5M (mainnet UTxO set is 15-20M)
const CORE_SET_1M: usize = 1_000_000;

const LOOKUP_COUNT: usize = 1_000;

/// LSM configurations to benchmark: (label, memtable_mb, cache_mb, bloom_bits)
///
/// Sized for real-world node memory budgets:
/// - low_mem: 8GB system (~5GB available for LSM)
/// - default: 16GB system (~12GB available)
/// - high_mem: 32GB system (~28GB available)
const LSM_CONFIGS: &[(&str, u64, u64, u32)] = &[
    ("low_8gb", 256, 2048, 10),
    ("mid_16gb", 512, 4096, 10),
    ("high_32gb", 512, 8192, 10),
    ("high_bloom_16gb", 512, 4096, 15),
    ("legacy_small", 128, 256, 10),
];

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_input_u64(i: u64) -> TransactionInput {
    let mut bytes = [0u8; 32];
    bytes[0..8].copy_from_slice(&i.to_le_bytes());
    TransactionInput {
        transaction_id: Hash32::from_bytes(bytes),
        index: 0,
    }
}

fn make_output(lovelace: u64) -> TransactionOutput {
    TransactionOutput {
        address: dugite_primitives::address::Address::Byron(
            dugite_primitives::address::ByronAddress {
                payload: vec![0u8; 32],
            },
        ),
        value: TxValue::lovelace(lovelace),
        datum: OutputDatum::None,
        script_ref: None,
        raw_cbor: None,
        is_legacy: false,
    }
}

/// Create a multi-asset output (~30% of mainnet UTxOs carry tokens).
/// 3 policies, 2 assets each — representative of typical NFT/token UTxOs.
fn make_multi_asset_output(lovelace: u64, seed: u64) -> TransactionOutput {
    let mut multi_asset = BTreeMap::new();
    for p in 0..3u64 {
        let mut policy_bytes = [0u8; 28];
        policy_bytes[0..8].copy_from_slice(&(seed.wrapping_add(p * 1000)).to_le_bytes());
        let policy_id = Hash28::from_bytes(policy_bytes);

        let mut assets = BTreeMap::new();
        for a in 0..2u64 {
            let name = AssetName::new(format!("token_{seed}_{p}_{a}").into_bytes())
                .unwrap_or_else(|_| AssetName::new(format!("t{seed}{p}{a}").into_bytes()).unwrap());
            assets.insert(name, 1_000_000 + a * 100);
        }
        multi_asset.insert(policy_id, assets);
    }

    TransactionOutput {
        address: dugite_primitives::address::Address::Byron(
            dugite_primitives::address::ByronAddress {
                payload: vec![0u8; 32],
            },
        ),
        value: TxValue {
            coin: Lovelace(lovelace),
            multi_asset,
        },
        datum: OutputDatum::None,
        script_ref: None,
        raw_cbor: None,
        is_legacy: false,
    }
}

fn populate_store(store: &mut UtxoStore, count: usize) -> Vec<TransactionInput> {
    let mut inputs = Vec::with_capacity(count);
    for i in 0..count {
        let input = make_input_u64(i as u64);
        store.insert(input.clone(), make_output(2_000_000));
        inputs.push(input);
    }
    inputs
}

/// Populate store with ~30% multi-asset UTxOs (matching mainnet distribution).
fn populate_store_mixed(store: &mut UtxoStore, count: usize) -> Vec<TransactionInput> {
    let mut inputs = Vec::with_capacity(count);
    for i in 0..count {
        let input = make_input_u64(i as u64);
        let output = if i % 10 < 3 {
            // 30% multi-asset
            make_multi_asset_output(2_000_000, i as u64)
        } else {
            make_output(2_000_000)
        };
        store.insert(input.clone(), output);
        inputs.push(input);
    }
    inputs
}

fn open_configured_store(
    dir: &std::path::Path,
    memtable_mb: u64,
    cache_mb: u64,
    bloom_bits: u32,
) -> UtxoStore {
    let path = dir.join("utxo");
    UtxoStore::open_with_config(&path, memtable_mb, cache_mb, bloom_bits).unwrap()
}

fn lcg_next(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state
}

// ===========================================================================
// Benchmarks
// ===========================================================================

// ---------------------------------------------------------------------------
// 1. Insert throughput (mainnet-representative sizes)
// ---------------------------------------------------------------------------

fn bench_utxo_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("utxo_store/insert");
    group.sample_size(10);

    let size = CORE_SET_1M;
    group.bench_with_input(BenchmarkId::new("default", size), &size, |b, &size| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let store = UtxoStore::open(dir.path().join("utxo")).unwrap();
                (dir, store)
            },
            |(_dir, mut store)| {
                populate_store(&mut store, size);
                black_box(store.len());
            },
            BatchSize::PerIteration,
        );
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// 2. Insert throughput with varying LSM configs (1M UTxOs)
// ---------------------------------------------------------------------------

fn bench_utxo_insert_configs(c: &mut Criterion) {
    let mut group = c.benchmark_group("utxo_store/insert_configs");
    group.sample_size(10);
    let size = CORE_SET_1M;

    for &(label, memtable, cache, bloom) in LSM_CONFIGS {
        group.bench_function(BenchmarkId::new(label, size), |b| {
            b.iter_batched(
                || {
                    let dir = tempfile::tempdir().unwrap();
                    let store = open_configured_store(dir.path(), memtable, cache, bloom);
                    (dir, store)
                },
                |(_dir, mut store)| {
                    populate_store(&mut store, size);
                    black_box(store.len());
                },
                BatchSize::PerIteration,
            );
        });
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// 3. Lookup throughput (1M UTxOs)
// ---------------------------------------------------------------------------

fn bench_utxo_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("utxo_store/lookup");

    let size = CORE_SET_1M;
    let dir = tempfile::tempdir().unwrap();
    let mut store = UtxoStore::open(dir.path().join("utxo")).unwrap();
    let inputs = populate_store(&mut store, size);

    // Random lookup indices
    let mut rng = 42u64;
    let lookup_inputs: Vec<TransactionInput> = (0..LOOKUP_COUNT)
        .map(|_| inputs[(lcg_next(&mut rng) % size as u64) as usize].clone())
        .collect();

    group.bench_with_input(BenchmarkId::new("hit", size), &size, |b, _| {
        b.iter(|| {
            for input in &lookup_inputs {
                let r = store.lookup(input);
                black_box(r.is_some());
            }
        });
    });

    // Miss lookups (guaranteed not in store)
    let miss_inputs: Vec<TransactionInput> = (0..LOOKUP_COUNT)
        .map(|i| make_input_u64(size as u64 + i as u64 + 1000))
        .collect();

    group.bench_with_input(BenchmarkId::new("miss", size), &size, |b, _| {
        b.iter(|| {
            for input in &miss_inputs {
                let r = store.lookup(input);
                black_box(r.is_none());
            }
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// 4. Lookup with varying LSM configs (1M UTxOs)
// ---------------------------------------------------------------------------

fn bench_utxo_lookup_configs(c: &mut Criterion) {
    let mut group = c.benchmark_group("utxo_store/lookup_configs");
    let size = CORE_SET_1M;

    for &(label, memtable, cache, bloom) in LSM_CONFIGS {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open_configured_store(dir.path(), memtable, cache, bloom);
        let inputs = populate_store(&mut store, size);

        let mut rng = 42u64;
        let lookup_inputs: Vec<TransactionInput> = (0..LOOKUP_COUNT)
            .map(|_| inputs[(lcg_next(&mut rng) % size as u64) as usize].clone())
            .collect();

        group.bench_function(BenchmarkId::new(label, size), |b| {
            b.iter(|| {
                for input in &lookup_inputs {
                    black_box(store.lookup(input));
                }
            });
        });
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// 5. Contains check (1M UTxOs)
// ---------------------------------------------------------------------------

fn bench_utxo_contains(c: &mut Criterion) {
    let mut group = c.benchmark_group("utxo_store/contains");
    let size = CORE_SET_1M;

    let dir = tempfile::tempdir().unwrap();
    let mut store = UtxoStore::open(dir.path().join("utxo")).unwrap();
    let inputs = populate_store(&mut store, size);

    let mut rng = 42u64;
    let check_inputs: Vec<TransactionInput> = (0..LOOKUP_COUNT)
        .map(|_| inputs[(lcg_next(&mut rng) % size as u64) as usize].clone())
        .collect();

    group.bench_function("hit", |b| {
        b.iter(|| {
            for input in &check_inputs {
                black_box(store.contains(input));
            }
        });
    });

    let miss_inputs: Vec<TransactionInput> = (0..LOOKUP_COUNT)
        .map(|i| make_input_u64(size as u64 + i as u64 + 1000))
        .collect();

    group.bench_function("miss", |b| {
        b.iter(|| {
            for input in &miss_inputs {
                black_box(store.contains(input));
            }
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// 6. Remove throughput (1M UTxOs)
// ---------------------------------------------------------------------------

fn bench_utxo_remove(c: &mut Criterion) {
    let mut group = c.benchmark_group("utxo_store/remove");
    group.sample_size(10);
    let size = CORE_SET_1M;

    group.bench_function(BenchmarkId::new("sequential", size), |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let mut store = UtxoStore::open(dir.path().join("utxo")).unwrap();
                let inputs = populate_store(&mut store, size);
                (dir, store, inputs)
            },
            |(_dir, mut store, inputs)| {
                for input in &inputs {
                    store.remove(input);
                }
                assert_eq!(store.len(), 0);
            },
            BatchSize::PerIteration,
        );
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// 7. Apply transaction — realistic mainnet workload
//    50 txs/block, each with 2-5 inputs and 2-3 outputs
// ---------------------------------------------------------------------------

fn bench_utxo_apply_transaction(c: &mut Criterion) {
    let mut group = c.benchmark_group("utxo_store/apply_tx");
    group.sample_size(10);

    // Realistic block: 50 transactions, each consuming 3 inputs, producing 2 outputs
    group.bench_function("block_50tx_3in_2out", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let mut store = UtxoStore::open(dir.path().join("utxo")).unwrap();
                // Pre-populate with enough UTxOs (need 50*3 = 150 inputs minimum)
                let inputs = populate_store(&mut store, CORE_SET_1M);
                (dir, store, inputs)
            },
            |(_dir, mut store, inputs)| {
                // Apply 50 transactions (one mainnet block), each consuming 3 inputs
                // and producing 2 outputs
                for tx_idx in 0..50 {
                    let tx_hash = {
                        let mut bytes = [0u8; 32];
                        bytes[0..8].copy_from_slice(&(tx_idx as u64 + 100_000).to_le_bytes());
                        Hash32::from_bytes(bytes)
                    };
                    let tx_inputs: Vec<&TransactionInput> =
                        (0..3).map(|j| &inputs[tx_idx * 3 + j]).collect();
                    let outputs = vec![make_output(1_500_000), make_output(500_000)];
                    for input in &tx_inputs {
                        store.remove(input);
                    }
                    for (out_idx, output) in outputs.into_iter().enumerate() {
                        let out_input = TransactionInput {
                            transaction_id: tx_hash,
                            index: out_idx as u32,
                        };
                        store.insert(out_input, output);
                    }
                }
                black_box(store.len());
            },
            BatchSize::PerIteration,
        );
    });

    // Large block: 300 transactions (mainnet max), 2 inputs 2 outputs each
    group.bench_function("block_300tx_2in_2out", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let mut store = UtxoStore::open(dir.path().join("utxo")).unwrap();
                let inputs = populate_store(&mut store, CORE_SET_1M);
                (dir, store, inputs)
            },
            |(_dir, mut store, inputs)| {
                for tx_idx in 0..300 {
                    let tx_hash = {
                        let mut bytes = [0u8; 32];
                        bytes[0..8].copy_from_slice(&(tx_idx as u64 + 200_000).to_le_bytes());
                        Hash32::from_bytes(bytes)
                    };
                    let tx_inputs: Vec<&TransactionInput> =
                        (0..2).map(|j| &inputs[tx_idx * 2 + j]).collect();
                    let outputs = vec![make_output(1_000_000), make_output(1_000_000)];
                    for input in &tx_inputs {
                        store.remove(input);
                    }
                    for (out_idx, output) in outputs.into_iter().enumerate() {
                        let out_input = TransactionInput {
                            transaction_id: tx_hash,
                            index: out_idx as u32,
                        };
                        store.insert(out_input, output);
                    }
                }
                black_box(store.len());
            },
            BatchSize::PerIteration,
        );
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// 8. Multi-asset UTxO benchmarks (30% of mainnet UTxOs have tokens)
// ---------------------------------------------------------------------------

fn bench_utxo_multi_asset(c: &mut Criterion) {
    let mut group = c.benchmark_group("utxo_store/multi_asset");
    group.sample_size(10);
    let size = CORE_SET_1M;

    // Insert mixed (70% ADA-only, 30% multi-asset)
    group.bench_function(BenchmarkId::new("insert_mixed_30pct", size), |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let store = UtxoStore::open(dir.path().join("utxo")).unwrap();
                (dir, store)
            },
            |(_dir, mut store)| {
                populate_store_mixed(&mut store, size);
                black_box(store.len());
            },
            BatchSize::PerIteration,
        );
    });

    // Lookup in mixed store
    group.bench_function(BenchmarkId::new("lookup_mixed_30pct", size), |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let mut store = UtxoStore::open(dir.path().join("utxo")).unwrap();
                let inputs = populate_store_mixed(&mut store, size);
                let mut rng = 42u64;
                let lookup_inputs: Vec<TransactionInput> = (0..LOOKUP_COUNT)
                    .map(|_| inputs[(lcg_next(&mut rng) % size as u64) as usize].clone())
                    .collect();
                (dir, store, lookup_inputs)
            },
            |(_dir, store, lookup_inputs)| {
                for input in &lookup_inputs {
                    black_box(store.lookup(input));
                }
            },
            BatchSize::PerIteration,
        );
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// 9. Total lovelace scan (1M UTxOs)
// ---------------------------------------------------------------------------

fn bench_utxo_total_lovelace(c: &mut Criterion) {
    let mut group = c.benchmark_group("utxo_store/total_lovelace");

    let size = CORE_SET_1M;
    let dir = tempfile::tempdir().unwrap();
    let mut store = UtxoStore::open(dir.path().join("utxo")).unwrap();
    populate_store(&mut store, size);

    group.bench_with_input(BenchmarkId::new("scan", size), &size, |b, _| {
        b.iter(|| {
            let total = store.total_lovelace();
            black_box(total);
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// 10. Rebuild address index (1M UTxOs)
// ---------------------------------------------------------------------------

fn bench_utxo_rebuild_index(c: &mut Criterion) {
    let mut group = c.benchmark_group("utxo_store/rebuild_address_index");
    group.sample_size(10);

    let size = CORE_SET_1M;
    group.bench_with_input(BenchmarkId::new("rebuild", size), &size, |b, &size| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let utxo_path = dir.path().join("utxo");
                {
                    let mut store = UtxoStore::open(&utxo_path).unwrap();
                    populate_store(&mut store, size);
                    store.save_snapshot("bench").unwrap();
                }
                // Reopen from snapshot — address index is empty and needs rebuild
                let store = UtxoStore::open_from_snapshot(&utxo_path, "bench").unwrap();
                (dir, store)
            },
            |(_dir, mut store)| {
                store.rebuild_address_index();
                black_box(store.len());
            },
            BatchSize::PerIteration,
        );
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// 11. Dataset scaling — measure how performance degrades as UTxO set grows
//     Default sizes: 100K, 500K, 1M (run in CI)
//     Large sizes: 5M, 10M (behind utxo_large_scale_benches, not in default main)
// ---------------------------------------------------------------------------

/// Default scaling sizes — suitable for CI
const SCALING_SIZES: &[usize] = &[100_000, 500_000, 1_000_000];

/// Large scaling sizes — too slow for CI, run manually
const LARGE_SCALING_SIZES: &[usize] = &[5_000_000, 10_000_000];

fn bench_utxo_scaling_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("utxo_scaling/insert");
    group.sample_size(10);

    for &size in SCALING_SIZES {
        group.bench_with_input(BenchmarkId::new("default", size), &size, |b, &size| {
            b.iter_batched(
                || {
                    let dir = tempfile::tempdir().unwrap();
                    let store = UtxoStore::open(dir.path().join("utxo")).unwrap();
                    (dir, store)
                },
                |(_dir, mut store)| {
                    populate_store(&mut store, size);
                    black_box(store.len());
                },
                BatchSize::PerIteration,
            );
        });
    }

    group.finish();
}

fn bench_utxo_scaling_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("utxo_scaling/lookup");
    group.sample_size(10);

    for &size in SCALING_SIZES {
        let dir = tempfile::tempdir().unwrap();
        let mut store = UtxoStore::open(dir.path().join("utxo")).unwrap();
        let inputs = populate_store(&mut store, size);

        let mut rng = 42u64;
        let lookup_inputs: Vec<TransactionInput> = (0..LOOKUP_COUNT)
            .map(|_| inputs[(lcg_next(&mut rng) % size as u64) as usize].clone())
            .collect();

        group.bench_with_input(BenchmarkId::new("hit", size), &size, |b, _| {
            b.iter(|| {
                for input in &lookup_inputs {
                    black_box(store.lookup(input));
                }
            });
        });
    }

    group.finish();
}

fn bench_utxo_scaling_apply_tx(c: &mut Criterion) {
    let mut group = c.benchmark_group("utxo_scaling/apply_tx");
    group.sample_size(10);

    // Measure apply_transaction at different base UTxO set sizes
    // Using realistic tx shape: 3 inputs, 2 outputs, batch of 50 (one block)
    for &size in &[100_000usize, 500_000, 1_000_000] {
        group.bench_with_input(
            BenchmarkId::new("block_50tx_3in_2out", size),
            &size,
            |b, &size| {
                b.iter_batched(
                    || {
                        let dir = tempfile::tempdir().unwrap();
                        let mut store = UtxoStore::open(dir.path().join("utxo")).unwrap();
                        let inputs = populate_store(&mut store, size);
                        (dir, store, inputs)
                    },
                    |(_dir, mut store, inputs)| {
                        for tx_idx in 0..50 {
                            let tx_hash = {
                                let mut bytes = [0u8; 32];
                                bytes[0..8]
                                    .copy_from_slice(&(tx_idx as u64 + 100_000).to_le_bytes());
                                Hash32::from_bytes(bytes)
                            };
                            // 3 inputs per tx
                            for j in 0..3 {
                                store.remove(&inputs[tx_idx * 3 + j]);
                            }
                            // 2 outputs per tx
                            for out_idx in 0..2u32 {
                                let out_input = TransactionInput {
                                    transaction_id: tx_hash,
                                    index: out_idx,
                                };
                                store.insert(out_input, make_output(1_000_000));
                            }
                        }
                        black_box(store.len());
                    },
                    BatchSize::PerIteration,
                );
            },
        );
    }

    group.finish();
}

fn bench_utxo_scaling_total_lovelace(c: &mut Criterion) {
    let mut group = c.benchmark_group("utxo_scaling/total_lovelace");
    group.sample_size(10);

    for &size in SCALING_SIZES {
        let dir = tempfile::tempdir().unwrap();
        let mut store = UtxoStore::open(dir.path().join("utxo")).unwrap();
        populate_store(&mut store, size);

        group.bench_with_input(BenchmarkId::new("scan", size), &size, |b, _| {
            b.iter(|| {
                black_box(store.total_lovelace());
            });
        });
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// 12. Large-scale benchmarks (5M, 10M) — NOT in default criterion_main
//     Run manually: cargo bench -p dugite-ledger -- utxo_large_scale
// ---------------------------------------------------------------------------

fn bench_utxo_large_scale_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("utxo_large_scale/insert");
    group.sample_size(10);

    for &size in LARGE_SCALING_SIZES {
        group.bench_with_input(BenchmarkId::new("default", size), &size, |b, &size| {
            b.iter_batched(
                || {
                    let dir = tempfile::tempdir().unwrap();
                    let store = UtxoStore::open(dir.path().join("utxo")).unwrap();
                    (dir, store)
                },
                |(_dir, mut store)| {
                    populate_store(&mut store, size);
                    black_box(store.len());
                },
                BatchSize::PerIteration,
            );
        });
    }

    group.finish();
}

fn bench_utxo_large_scale_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("utxo_large_scale/lookup");
    group.sample_size(10);

    for &size in LARGE_SCALING_SIZES {
        let dir = tempfile::tempdir().unwrap();
        let mut store = UtxoStore::open(dir.path().join("utxo")).unwrap();
        let inputs = populate_store(&mut store, size);

        let mut rng = 42u64;
        let lookup_inputs: Vec<TransactionInput> = (0..LOOKUP_COUNT)
            .map(|_| inputs[(lcg_next(&mut rng) % size as u64) as usize].clone())
            .collect();

        group.bench_with_input(BenchmarkId::new("hit", size), &size, |b, _| {
            b.iter(|| {
                for input in &lookup_inputs {
                    black_box(store.lookup(input));
                }
            });
        });
    }

    group.finish();
}

fn bench_utxo_large_scale_total_lovelace(c: &mut Criterion) {
    let mut group = c.benchmark_group("utxo_large_scale/total_lovelace");
    group.sample_size(10);

    for &size in LARGE_SCALING_SIZES {
        let dir = tempfile::tempdir().unwrap();
        let mut store = UtxoStore::open(dir.path().join("utxo")).unwrap();
        populate_store(&mut store, size);

        group.bench_with_input(BenchmarkId::new("scan", size), &size, |b, _| {
            b.iter(|| {
                black_box(store.total_lovelace());
            });
        });
    }

    group.finish();
}

// ===========================================================================
// Criterion harness
// ===========================================================================

criterion_group!(
    utxo_core_benches,
    bench_utxo_insert,
    bench_utxo_lookup,
    bench_utxo_contains,
    bench_utxo_remove,
    bench_utxo_apply_transaction,
    bench_utxo_multi_asset,
    bench_utxo_total_lovelace,
    bench_utxo_rebuild_index,
);

criterion_group!(
    utxo_config_benches,
    bench_utxo_insert_configs,
    bench_utxo_lookup_configs,
);

criterion_group!(
    utxo_scaling_benches,
    bench_utxo_scaling_insert,
    bench_utxo_scaling_lookup,
    bench_utxo_scaling_apply_tx,
    bench_utxo_scaling_total_lovelace,
);

// Large-scale benchmarks: not in criterion_main! so they don't run by default.
// Run manually with: cargo bench -p dugite-ledger -- utxo_large_scale
criterion_group!(
    utxo_large_scale_benches,
    bench_utxo_large_scale_insert,
    bench_utxo_large_scale_lookup,
    bench_utxo_large_scale_total_lovelace,
);

criterion_main!(
    utxo_core_benches,
    utxo_config_benches,
    utxo_scaling_benches,
    utxo_large_scale_benches
);
