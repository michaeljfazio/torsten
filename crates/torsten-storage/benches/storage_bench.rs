//! Criterion benchmarks for the cardano-lsm ImmutableDB backend.
//!
//! Run: cargo bench -p torsten-storage

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use torsten_primitives::hash::Hash32;
use torsten_primitives::time::{BlockNo, SlotNo};

use torsten_storage::lsm::LsmImmutableDB as DB;

const NUM_BLOCKS: u64 = 5_000;
const NUM_LOOKUPS: usize = 500;
const BATCH_SIZE: usize = 100;
/// Simulated block CBOR size (~1KB, realistic for small blocks).
const BLOCK_DATA_SIZE: usize = 1024;

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

/// Generate fake block data of a given size.
fn make_block_data(index: u64) -> Vec<u8> {
    let mut data = vec![0u8; BLOCK_DATA_SIZE];
    let seed = index.to_le_bytes();
    for (i, byte) in data.iter_mut().enumerate() {
        *byte = seed[i % 8].wrapping_add(i as u8);
    }
    data
}

/// Pre-generate all test blocks: (slot, hash, block_no, cbor_data).
fn generate_blocks(count: u64) -> Vec<(SlotNo, Hash32, BlockNo, Vec<u8>)> {
    (0..count)
        .map(|i| {
            let slot = SlotNo(i + 1);
            let hash = make_hash(i);
            let block_no = BlockNo(i + 1);
            let data = make_block_data(i);
            (slot, hash, block_no, data)
        })
        .collect()
}

fn open_db(path: &std::path::Path) -> DB {
    DB::open(path).unwrap()
}

fn populate_db(path: &std::path::Path, blocks: &[(SlotNo, Hash32, BlockNo, Vec<u8>)]) -> DB {
    let mut db = open_db(path);
    for chunk in blocks.chunks(BATCH_SIZE) {
        let batch: Vec<_> = chunk
            .iter()
            .map(|(s, h, b, d)| (*s, h, *b, d.as_slice()))
            .collect();
        db.put_blocks_batch(&batch).unwrap();
    }
    db
}

fn bench_sequential_insert(c: &mut Criterion) {
    let blocks = generate_blocks(NUM_BLOCKS);

    c.bench_function("lsm/sequential_insert", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                (dir, blocks.clone())
            },
            |(dir, blocks)| {
                let mut db = open_db(dir.path());
                for (slot, hash, block_no, data) in &blocks {
                    db.put_block(*slot, hash, *block_no, data).unwrap();
                }
            },
            BatchSize::PerIteration,
        );
    });
}

fn bench_batch_insert(c: &mut Criterion) {
    let blocks = generate_blocks(NUM_BLOCKS);

    c.bench_function("lsm/batch_insert", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                (dir, blocks.clone())
            },
            |(dir, blocks)| {
                let mut db = open_db(dir.path());
                for chunk in blocks.chunks(BATCH_SIZE) {
                    let batch: Vec<_> = chunk
                        .iter()
                        .map(|(s, h, b, d)| (*s, h, *b, d.as_slice()))
                        .collect();
                    db.put_blocks_batch(&batch).unwrap();
                }
            },
            BatchSize::PerIteration,
        );
    });
}

fn bench_random_read_by_hash(c: &mut Criterion) {
    let blocks = generate_blocks(NUM_BLOCKS);

    let mut rng_state: u64 = 42;
    let lookup_hashes: Vec<Hash32> = (0..NUM_LOOKUPS)
        .map(|_| {
            rng_state = rng_state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            make_hash(rng_state % NUM_BLOCKS)
        })
        .collect();

    c.bench_function("lsm/random_read_by_hash", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let db = populate_db(dir.path(), &blocks);
                (dir, db)
            },
            |(_dir, db)| {
                for hash in &lookup_hashes {
                    let result = db.get_block_by_hash(hash).unwrap();
                    assert!(result.is_some());
                }
            },
            BatchSize::PerIteration,
        );
    });
}

fn bench_random_read_by_slot(c: &mut Criterion) {
    let blocks = generate_blocks(NUM_BLOCKS);

    let mut rng_state: u64 = 123;
    let lookup_slots: Vec<SlotNo> = (0..NUM_LOOKUPS)
        .map(|_| {
            rng_state = rng_state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            SlotNo((rng_state % NUM_BLOCKS) + 1)
        })
        .collect();

    c.bench_function("lsm/random_read_by_slot", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let db = populate_db(dir.path(), &blocks);
                (dir, db)
            },
            |(_dir, db)| {
                for slot in &lookup_slots {
                    let result = db.get_block_by_slot(*slot).unwrap();
                    assert!(result.is_some());
                }
            },
            BatchSize::PerIteration,
        );
    });
}

fn bench_sequential_scan(c: &mut Criterion) {
    let blocks = generate_blocks(NUM_BLOCKS);

    c.bench_function("lsm/sequential_scan", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let db = populate_db(dir.path(), &blocks);
                (dir, db)
            },
            |(_dir, db)| {
                let result = db
                    .get_blocks_in_slot_range(SlotNo(1), SlotNo(NUM_BLOCKS))
                    .unwrap();
                assert_eq!(result.len(), NUM_BLOCKS as usize);
            },
            BatchSize::PerIteration,
        );
    });
}

fn bench_negative_lookup(c: &mut Criterion) {
    let blocks = generate_blocks(NUM_BLOCKS);

    let nonexistent_hashes: Vec<Hash32> = (NUM_BLOCKS..NUM_BLOCKS + NUM_LOOKUPS as u64)
        .map(make_hash)
        .collect();

    c.bench_function("lsm/negative_lookup", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let db = populate_db(dir.path(), &blocks);
                (dir, db)
            },
            |(_dir, db)| {
                for hash in &nonexistent_hashes {
                    let result = db.get_block_by_hash(hash).unwrap();
                    assert!(result.is_none());
                }
            },
            BatchSize::PerIteration,
        );
    });
}

fn bench_tip_query(c: &mut Criterion) {
    let blocks = generate_blocks(NUM_BLOCKS);

    c.bench_function("lsm/tip_query", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let db = populate_db(dir.path(), &blocks);
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
    bench_batch_insert,
    bench_random_read_by_hash,
    bench_random_read_by_slot,
    bench_sequential_scan,
    bench_negative_lookup,
    bench_tip_query,
);
criterion_main!(benches);
