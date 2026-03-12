//! Criterion benchmarks for ChainDB block storage.
//!
//! Run: cargo bench -p torsten-storage

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use torsten_primitives::hash::Hash32;
use torsten_primitives::time::{BlockNo, SlotNo};

use torsten_storage::ChainDB;

const NUM_BLOCKS: u64 = 5_000;
const NUM_LOOKUPS: usize = 500;

/// Generate a deterministic hash from an index.
fn make_hash(index: u64) -> Hash32 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut h1 = DefaultHasher::new();
    index.hash(&mut h1);
    let v1 = h1.finish();
    let mut h2 = DefaultHasher::new();
    (index.wrapping_add(0x9e3779b97f4a7c15)).hash(&mut h2);
    let v2 = h2.finish();
    let mut h3 = DefaultHasher::new();
    (index.wrapping_add(0x517cc1b727220a95)).hash(&mut h3);
    let v3 = h3.finish();
    let mut h4 = DefaultHasher::new();
    (index.wrapping_add(0x6c62272e07bb0142)).hash(&mut h4);
    let v4 = h4.finish();

    let mut bytes = [0u8; 32];
    bytes[0..8].copy_from_slice(&v1.to_le_bytes());
    bytes[8..16].copy_from_slice(&v2.to_le_bytes());
    bytes[16..24].copy_from_slice(&v3.to_le_bytes());
    bytes[24..32].copy_from_slice(&v4.to_le_bytes());
    Hash32::from_bytes(bytes)
}

fn populate_db(path: &std::path::Path, count: u64) -> (ChainDB, Vec<Hash32>) {
    let mut db = ChainDB::open(path).unwrap();
    let mut hashes = Vec::with_capacity(count as usize);
    for i in 0..count {
        let hash = make_hash(i);
        let prev = if i == 0 {
            Hash32::ZERO
        } else {
            make_hash(i - 1)
        };
        let data = vec![0u8; 1024]; // ~1KB block
        db.add_block(hash, SlotNo(i + 1), BlockNo(i + 1), prev, data)
            .unwrap();
        hashes.push(hash);
    }
    (db, hashes)
}

fn bench_sequential_insert(c: &mut Criterion) {
    c.bench_function("chaindb/sequential_insert", |b| {
        b.iter_batched(
            || tempfile::tempdir().unwrap(),
            |dir| {
                populate_db(dir.path(), NUM_BLOCKS);
            },
            BatchSize::PerIteration,
        );
    });
}

fn bench_random_read_by_hash(c: &mut Criterion) {
    let mut rng_state: u64 = 42;
    let lookup_indices: Vec<u64> = (0..NUM_LOOKUPS)
        .map(|_| {
            rng_state = rng_state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            rng_state % NUM_BLOCKS
        })
        .collect();

    c.bench_function("chaindb/random_read_by_hash", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let (db, _hashes) = populate_db(dir.path(), NUM_BLOCKS);
                let lookup_hashes: Vec<Hash32> =
                    lookup_indices.iter().map(|&i| make_hash(i)).collect();
                (dir, db, lookup_hashes)
            },
            |(_dir, db, hashes)| {
                for hash in &hashes {
                    let result = db.get_block(hash).unwrap();
                    assert!(result.is_some());
                }
            },
            BatchSize::PerIteration,
        );
    });
}

fn bench_tip_query(c: &mut Criterion) {
    c.bench_function("chaindb/tip_query", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let (db, _) = populate_db(dir.path(), NUM_BLOCKS);
                (dir, db)
            },
            |(_dir, db)| {
                for _ in 0..NUM_LOOKUPS {
                    let tip = db.get_tip_info();
                    assert!(tip.is_some());
                }
            },
            BatchSize::PerIteration,
        );
    });
}

criterion_group!(
    benches,
    bench_sequential_insert,
    bench_random_read_by_hash,
    bench_tip_query,
);
criterion_main!(benches);
