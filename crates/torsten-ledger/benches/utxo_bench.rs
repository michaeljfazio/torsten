//! Criterion benchmarks for the UTxO store.
//!
//! Covers: insert, lookup, remove, contains, apply_transaction, and
//! total_lovelace scan with both default and custom LSM configurations.
//!
//! Run:  cargo bench -p torsten-ledger
//! HTML: target/criterion/report/index.html

use criterion::{black_box, criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion};
use torsten_ledger::utxo_store::UtxoStore;
use torsten_primitives::hash::Hash32;
use torsten_primitives::transaction::{OutputDatum, TransactionInput, TransactionOutput};
use torsten_primitives::value::Value as TxValue;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const SMALL_SET: usize = 1_000;
const MEDIUM_SET: usize = 10_000;
const LOOKUP_COUNT: usize = 500;

/// LSM configurations to benchmark: (label, memtable_mb, cache_mb, bloom_bits)
const LSM_CONFIGS: &[(&str, u64, u64, u32)] = &[
    ("default", 128, 256, 10),
    ("low_mem", 64, 128, 10),
    ("tiny", 32, 64, 5),
    ("large_cache", 128, 512, 10),
    ("high_bloom", 128, 256, 15),
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
        address: torsten_primitives::address::Address::Byron(
            torsten_primitives::address::ByronAddress {
                payload: vec![0u8; 32],
            },
        ),
        value: TxValue::lovelace(lovelace),
        datum: OutputDatum::None,
        script_ref: None,
        raw_cbor: None,
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
// 1. Insert throughput (varying set sizes)
// ---------------------------------------------------------------------------

fn bench_utxo_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("utxo_store/insert");

    for &size in &[SMALL_SET, MEDIUM_SET] {
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

// ---------------------------------------------------------------------------
// 2. Insert throughput with varying LSM configs
// ---------------------------------------------------------------------------

fn bench_utxo_insert_configs(c: &mut Criterion) {
    let mut group = c.benchmark_group("utxo_store/insert_configs");
    let size = MEDIUM_SET;

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
// 3. Lookup throughput
// ---------------------------------------------------------------------------

fn bench_utxo_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("utxo_store/lookup");

    for &size in &[SMALL_SET, MEDIUM_SET] {
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
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// 4. Lookup with varying LSM configs
// ---------------------------------------------------------------------------

fn bench_utxo_lookup_configs(c: &mut Criterion) {
    let mut group = c.benchmark_group("utxo_store/lookup_configs");
    let size = MEDIUM_SET;

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
// 5. Contains check
// ---------------------------------------------------------------------------

fn bench_utxo_contains(c: &mut Criterion) {
    let mut group = c.benchmark_group("utxo_store/contains");
    let size = MEDIUM_SET;

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
// 6. Remove throughput
// ---------------------------------------------------------------------------

fn bench_utxo_remove(c: &mut Criterion) {
    let mut group = c.benchmark_group("utxo_store/remove");
    let size = MEDIUM_SET;

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
// 7. Apply transaction (realistic workload)
// ---------------------------------------------------------------------------

fn bench_utxo_apply_transaction(c: &mut Criterion) {
    let mut group = c.benchmark_group("utxo_store/apply_tx");

    group.bench_function("batch_100", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let mut store = UtxoStore::open(dir.path().join("utxo")).unwrap();
                // Pre-populate with enough UTxOs
                let inputs = populate_store(&mut store, 10_000);
                (dir, store, inputs)
            },
            |(_dir, mut store, inputs)| {
                // Apply 100 transactions, each consuming 1 input and producing 2 outputs
                for (i, input) in inputs.iter().enumerate().take(100) {
                    let tx_hash = {
                        let mut bytes = [0u8; 32];
                        bytes[0..8].copy_from_slice(&(i as u64 + 100_000).to_le_bytes());
                        Hash32::from_bytes(bytes)
                    };
                    let outputs = vec![make_output(1_000_000), make_output(1_000_000)];
                    store
                        .apply_transaction(&tx_hash, std::slice::from_ref(input), &outputs)
                        .unwrap();
                }
                black_box(store.len());
            },
            BatchSize::PerIteration,
        );
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// 8. Total lovelace scan
// ---------------------------------------------------------------------------

fn bench_utxo_total_lovelace(c: &mut Criterion) {
    let mut group = c.benchmark_group("utxo_store/total_lovelace");

    for &size in &[SMALL_SET, MEDIUM_SET] {
        let dir = tempfile::tempdir().unwrap();
        let mut store = UtxoStore::open(dir.path().join("utxo")).unwrap();
        populate_store(&mut store, size);

        group.bench_with_input(BenchmarkId::new("scan", size), &size, |b, _| {
            b.iter(|| {
                let total = store.total_lovelace();
                black_box(total);
            });
        });
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// 9. Rebuild address index
// ---------------------------------------------------------------------------

fn bench_utxo_rebuild_index(c: &mut Criterion) {
    let mut group = c.benchmark_group("utxo_store/rebuild_address_index");

    for &size in &[SMALL_SET, MEDIUM_SET] {
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
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// 10. Dataset scaling — measure how performance degrades as UTxO set grows
// ---------------------------------------------------------------------------

/// Scaling sizes for growth assessment
const SCALING_SIZES: &[usize] = &[1_000, 5_000, 10_000, 25_000, 50_000, 100_000];

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
    for &size in &[1_000usize, 5_000, 10_000, 25_000, 50_000] {
        group.bench_with_input(BenchmarkId::new("batch_50", size), &size, |b, &size| {
            b.iter_batched(
                || {
                    let dir = tempfile::tempdir().unwrap();
                    let mut store = UtxoStore::open(dir.path().join("utxo")).unwrap();
                    let inputs = populate_store(&mut store, size);
                    (dir, store, inputs)
                },
                |(_dir, mut store, inputs)| {
                    for (i, input) in inputs.iter().enumerate().take(50) {
                        let tx_hash = {
                            let mut bytes = [0u8; 32];
                            bytes[0..8].copy_from_slice(&(i as u64 + 100_000).to_le_bytes());
                            Hash32::from_bytes(bytes)
                        };
                        let outputs = vec![make_output(1_000_000), make_output(1_000_000)];
                        store
                            .apply_transaction(&tx_hash, std::slice::from_ref(input), &outputs)
                            .unwrap();
                    }
                    black_box(store.len());
                },
                BatchSize::PerIteration,
            );
        });
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

criterion_main!(utxo_core_benches, utxo_config_benches, utxo_scaling_benches);
