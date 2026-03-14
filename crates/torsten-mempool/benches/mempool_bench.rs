//! Criterion benchmarks for the transaction mempool.
//!
//! Simulates mainnet-representative workloads:
//! - Transaction sizes: mix of 200B, 500B, 2KB, 8KB (weighted toward 500B)
//! - Block building: get_txs_for_block sorted by fee density
//! - Rollback: drain all and re-add
//!
//! Run:  cargo bench -p torsten-mempool
//! HTML: target/criterion/report/index.html

use criterion::{black_box, criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion};
use torsten_mempool::{Mempool, MempoolConfig};
use torsten_primitives::hash::Hash32;
use torsten_primitives::transaction::Transaction;
use torsten_primitives::value::Lovelace;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Generate a deterministic transaction hash from an index.
fn make_tx_hash(i: u64) -> Hash32 {
    let mut bytes = [0u8; 32];
    bytes[0..8].copy_from_slice(&i.to_le_bytes());
    // Mix bits to avoid sequential patterns
    bytes[8..16].copy_from_slice(&i.wrapping_mul(0x9e3779b97f4a7c15).to_le_bytes());
    bytes[16..24].copy_from_slice(&i.wrapping_mul(0x517cc1b727220a95).to_le_bytes());
    bytes[24..32].copy_from_slice(&i.wrapping_mul(0x6c62272e07bb0142).to_le_bytes());
    Hash32::from_bytes(bytes)
}

/// Realistic transaction size based on mainnet distribution.
/// ~60% are ~500B, ~20% are ~200B, ~15% are ~2KB, ~5% are ~8KB.
fn tx_size_for_index(i: usize) -> usize {
    match i % 20 {
        0..=11 => 500,   // 60% at 500B
        12..=15 => 200,  // 20% at 200B
        16..=18 => 2048, // 15% at 2KB
        _ => 8192,       // 5% at 8KB
    }
}

/// Realistic fee for a given tx size — roughly 0.17 ADA per byte + base fee.
fn fee_for_size(size: usize) -> Lovelace {
    Lovelace(170_000 + (size as u64) * 44)
}

/// Create a mempool populated with `count` transactions.
fn make_populated_mempool(count: usize) -> (Mempool, Vec<Hash32>) {
    let config = MempoolConfig {
        max_transactions: count + 1000,
        max_bytes: 512 * 1024 * 1024,
        ..MempoolConfig::default()
    };
    let mempool = Mempool::new(config);
    let mut hashes = Vec::with_capacity(count);

    for i in 0..count {
        let hash = make_tx_hash(i as u64);
        let tx = Transaction::empty_with_hash(hash);
        let size = tx_size_for_index(i);
        let fee = fee_for_size(size);
        mempool.add_tx_with_fee(hash, tx, size, fee).unwrap();
        hashes.push(hash);
    }
    (mempool, hashes)
}

// ===========================================================================
// Benchmarks
// ===========================================================================

// ---------------------------------------------------------------------------
// 1. Add transactions to mempool
// ---------------------------------------------------------------------------

fn bench_mempool_add(c: &mut Criterion) {
    let mut group = c.benchmark_group("mempool/add");

    for &count in &[1_000usize, 5_000, 10_000] {
        group.bench_with_input(BenchmarkId::new("txs", count), &count, |b, &count| {
            b.iter_batched(
                || {
                    let config = MempoolConfig {
                        max_transactions: count + 1000,
                        max_bytes: 512 * 1024 * 1024,
                        ..MempoolConfig::default()
                    };
                    let mempool = Mempool::new(config);

                    // Pre-generate all transactions
                    let txs: Vec<_> = (0..count)
                        .map(|i| {
                            let hash = make_tx_hash(i as u64);
                            let tx = Transaction::empty_with_hash(hash);
                            let size = tx_size_for_index(i);
                            let fee = fee_for_size(size);
                            (hash, tx, size, fee)
                        })
                        .collect();
                    (mempool, txs)
                },
                |(mempool, txs)| {
                    for (hash, tx, size, fee) in txs {
                        let _ = mempool.add_tx_with_fee(hash, tx, size, fee);
                    }
                    black_box(mempool.len());
                },
                BatchSize::PerIteration,
            );
        });
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// 2. Remove transactions by hash (simulating block inclusion)
// ---------------------------------------------------------------------------

fn bench_mempool_remove(c: &mut Criterion) {
    let mut group = c.benchmark_group("mempool/remove");

    for &count in &[1_000usize, 5_000, 10_000] {
        group.bench_with_input(BenchmarkId::new("txs", count), &count, |b, &count| {
            b.iter_batched(
                || make_populated_mempool(count),
                |(mempool, hashes)| {
                    for hash in &hashes {
                        mempool.remove_tx(hash);
                    }
                    assert_eq!(mempool.len(), 0);
                },
                BatchSize::PerIteration,
            );
        });
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// 3. Get transactions sorted by fee density (block building)
// ---------------------------------------------------------------------------

fn bench_mempool_get_sorted(c: &mut Criterion) {
    let mut group = c.benchmark_group("mempool/get_sorted");

    for &count in &[1_000usize, 5_000, 10_000] {
        let (mempool, _hashes) = make_populated_mempool(count);

        group.bench_with_input(BenchmarkId::new("by_fee_density", count), &count, |b, _| {
            b.iter(|| {
                // Get top 300 txs (max per block) sorted by fee density
                let txs = mempool.get_txs_for_block_by_fee(300, 90_000);
                black_box(txs.len());
            });
        });
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// 4. Drain all and re-add (rollback simulation)
// ---------------------------------------------------------------------------

fn bench_mempool_drain_all(c: &mut Criterion) {
    let mut group = c.benchmark_group("mempool/drain_readd");

    for &count in &[1_000usize, 5_000, 10_000] {
        group.bench_with_input(BenchmarkId::new("txs", count), &count, |b, &count| {
            b.iter_batched(
                || make_populated_mempool(count),
                |(mempool, _hashes)| {
                    // Drain all transactions (simulating rollback)
                    let drained = mempool.drain_all();
                    black_box(drained.len());

                    // Re-add all (simulating re-validation after rollback)
                    for (i, tx) in drained.into_iter().enumerate() {
                        let size = tx_size_for_index(i);
                        let fee = fee_for_size(size);
                        let _ = mempool.add_tx_with_fee(tx.hash, tx, size, fee);
                    }
                    black_box(mempool.len());
                },
                BatchSize::PerIteration,
            );
        });
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// 5. Batch remove (simulating block inclusion of multiple txs)
// ---------------------------------------------------------------------------

fn bench_mempool_batch_remove(c: &mut Criterion) {
    let mut group = c.benchmark_group("mempool/batch_remove");

    for &pool_size in &[5_000usize, 10_000] {
        let (mempool, hashes) = make_populated_mempool(pool_size);

        // Remove 50 txs at once (average block on mainnet)
        let batch: Vec<Hash32> = hashes.iter().take(50).copied().collect();

        group.bench_with_input(
            BenchmarkId::new("50_from", pool_size),
            &pool_size,
            |b, _| {
                b.iter(|| {
                    mempool.remove_txs(black_box(&batch));
                });
            },
        );
    }

    group.finish();
}

// ===========================================================================
// Criterion harness
// ===========================================================================

criterion_group!(
    mempool_benches,
    bench_mempool_add,
    bench_mempool_remove,
    bench_mempool_get_sorted,
    bench_mempool_drain_all,
    bench_mempool_batch_remove,
);
criterion_main!(mempool_benches);
