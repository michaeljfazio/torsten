//! Criterion benchmarks for hash operations.
//!
//! Scales based on Cardano mainnet reference numbers:
//! - Block body size: 1-90KB (average ~20KB)
//! - Transactions per block: 20-300 (average ~50)
//! - Transaction size: 200B-16KB (average ~500B)
//!
//! Run:  cargo bench -p dugite-primitives
//! HTML: target/criterion/report/index.html

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use dugite_primitives::hash::{blake2b_224, blake2b_256};

fn bench_blake2b_256(c: &mut Criterion) {
    let mut group = c.benchmark_group("blake2b_256");

    // Typical Cardano payload sizes, including mainnet max block body
    let sizes: &[(&str, usize)] = &[
        ("32B_txhash", 32),
        ("64B_vkey", 64),
        ("256B_small_tx", 256),
        ("500B_avg_tx", 500),
        ("1KB_tx_body", 1024),
        ("4KB_large_tx", 4096),
        ("16KB_block_header", 16384),
        ("20KB_avg_block", 20480),
        ("90KB_max_block", 92160),
    ];

    for (label, size) in sizes {
        let data = vec![0xABu8; *size];
        group.bench_with_input(BenchmarkId::new("hash", label), &data, |b, data| {
            b.iter(|| blake2b_256(black_box(data)))
        });
    }

    group.finish();
}

fn bench_blake2b_224(c: &mut Criterion) {
    let mut group = c.benchmark_group("blake2b_224");

    let sizes: &[(&str, usize)] = &[
        ("32B_vkey_to_keyhash", 32),
        ("64B_script_bytes", 64),
        ("256B_address_payload", 256),
    ];

    for (label, size) in sizes {
        let data = vec![0xCDu8; *size];
        group.bench_with_input(BenchmarkId::new("hash", label), &data, |b, data| {
            b.iter(|| blake2b_224(black_box(data)))
        });
    }

    group.finish();
}

fn bench_blake2b_batch(c: &mut Criterion) {
    let mut group = c.benchmark_group("blake2b_batch");

    // Simulate hashing all vkey witnesses in a block (mainnet: 10-500+ witnesses)
    for count in [10, 50, 100, 500] {
        let keys: Vec<Vec<u8>> = (0..count).map(|i| vec![i as u8; 32]).collect();
        group.bench_with_input(
            BenchmarkId::new("224_keyhashes", count),
            &keys,
            |b, keys| {
                b.iter(|| {
                    for key in keys {
                        black_box(blake2b_224(key));
                    }
                })
            },
        );
    }

    // Simulate hashing transaction bodies in a block (mainnet: 20-300 txs)
    // Using 500B average tx size
    for count in [50, 100, 300] {
        let bodies: Vec<Vec<u8>> = (0..count).map(|i| vec![i as u8; 500]).collect();
        group.bench_with_input(
            BenchmarkId::new("256_txbodies_500B", count),
            &bodies,
            |b, bodies| {
                b.iter(|| {
                    for body in bodies {
                        black_box(blake2b_256(body));
                    }
                })
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_blake2b_256,
    bench_blake2b_224,
    bench_blake2b_batch
);
criterion_main!(benches);
