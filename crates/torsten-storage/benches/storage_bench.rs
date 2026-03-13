//! Criterion benchmarks for the storage subsystem.
//!
//! Covers: ChainDB operations, ImmutableDB (in-memory vs mmap block index),
//! and BlockIndex raw lookup/insert performance.
//!
//! Run:  cargo bench -p torsten-storage
//! HTML: target/criterion/report/index.html

use criterion::{black_box, criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion};
use torsten_primitives::hash::Hash32;
use torsten_primitives::time::{BlockNo, SlotNo};

use torsten_storage::chain_db::ChainDB;
use torsten_storage::config::{BlockIndexType, ImmutableConfig};
use torsten_storage::immutable_db::ImmutableDB;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const NUM_BLOCKS: u64 = 5_000;
const NUM_LOOKUPS: usize = 500;

/// Sizes used for parameterized benchmarks.
const BLOCK_INDEX_SIZES: &[u64] = &[1_000, 10_000, 50_000];

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Deterministic hash from an index (uniformly distributed, never all-zero).
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

/// Simple LCG for deterministic pseudo-random sequences.
fn lcg_next(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state
}

fn populate_chaindb(path: &std::path::Path, count: u64) -> (ChainDB, Vec<Hash32>) {
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

fn populate_chaindb_with_config(
    path: &std::path::Path,
    count: u64,
    config: &ImmutableConfig,
) -> (ChainDB, Vec<Hash32>) {
    let mut db = ChainDB::open_with_config(path, config).unwrap();
    let mut hashes = Vec::with_capacity(count as usize);
    for i in 0..count {
        let hash = make_hash(i);
        let prev = if i == 0 {
            Hash32::ZERO
        } else {
            make_hash(i - 1)
        };
        let data = vec![0u8; 1024];
        db.add_block(hash, SlotNo(i + 1), BlockNo(i + 1), prev, data)
            .unwrap();
        hashes.push(hash);
    }
    (db, hashes)
}

/// Create a chunk file + secondary index with `count` blocks.
fn create_chunk_file(dir: &std::path::Path, chunk_num: u64, count: u64) -> Vec<Hash32> {
    use std::io::Write;
    let chunk_path = dir.join(format!("{chunk_num:05}.chunk"));
    let secondary_path = dir.join(format!("{chunk_num:05}.secondary"));

    let mut chunk_file = std::fs::File::create(&chunk_path).unwrap();
    let mut secondary_file = std::fs::File::create(&secondary_path).unwrap();

    let block_data = vec![0u8; 1024]; // 1KB block
    let mut hashes = Vec::with_capacity(count as usize);
    let mut offset = 0u64;

    for i in 0..count {
        let hash = make_hash(chunk_num * 100_000 + i);
        chunk_file.write_all(&block_data).unwrap();

        let mut entry = [0u8; 56];
        entry[0..8].copy_from_slice(&offset.to_be_bytes());
        entry[16..48].copy_from_slice(hash.as_bytes());
        let slot = chunk_num * 100_000 + i + 1;
        entry[48..56].copy_from_slice(&slot.to_be_bytes());
        secondary_file.write_all(&entry).unwrap();

        hashes.push(hash);
        offset += block_data.len() as u64;
    }
    hashes
}

fn random_lookup_hashes(count: usize, max_index: u64) -> Vec<Hash32> {
    let mut rng_state: u64 = 42;
    (0..count)
        .map(|_| make_hash(lcg_next(&mut rng_state) % max_index))
        .collect()
}

// ===========================================================================
// Benchmark groups
// ===========================================================================

// ---------------------------------------------------------------------------
// 1. ChainDB (existing + in-memory vs mmap comparison)
// ---------------------------------------------------------------------------

fn bench_chaindb_sequential_insert(c: &mut Criterion) {
    c.bench_function("chaindb/sequential_insert_5k", |b| {
        b.iter_batched(
            || tempfile::tempdir().unwrap(),
            |dir| {
                populate_chaindb(dir.path(), NUM_BLOCKS);
            },
            BatchSize::PerIteration,
        );
    });
}

fn bench_chaindb_random_read(c: &mut Criterion) {
    let lookup_hashes = random_lookup_hashes(NUM_LOOKUPS, NUM_BLOCKS);

    c.bench_function("chaindb/random_read_by_hash", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let (db, _) = populate_chaindb(dir.path(), NUM_BLOCKS);
                (dir, db, lookup_hashes.clone())
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

fn bench_chaindb_tip_query(c: &mut Criterion) {
    c.bench_function("chaindb/tip_query", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let (db, _) = populate_chaindb(dir.path(), NUM_BLOCKS);
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

fn bench_chaindb_has_block(c: &mut Criterion) {
    let lookup_hashes = random_lookup_hashes(NUM_LOOKUPS, NUM_BLOCKS);

    c.bench_function("chaindb/has_block", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let (db, _) = populate_chaindb(dir.path(), NUM_BLOCKS);
                (dir, db, lookup_hashes.clone())
            },
            |(_dir, db, hashes)| {
                for hash in &hashes {
                    assert!(db.has_block(hash));
                }
            },
            BatchSize::PerIteration,
        );
    });
}

fn bench_chaindb_slot_range_query(c: &mut Criterion) {
    c.bench_function("chaindb/slot_range_100", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let (db, _) = populate_chaindb(dir.path(), NUM_BLOCKS);
                (dir, db)
            },
            |(_dir, db)| {
                // Query 100-block windows at random positions
                let mut rng = 42u64;
                for _ in 0..10 {
                    let start = (lcg_next(&mut rng) % (NUM_BLOCKS - 100)) + 1;
                    let blocks = db
                        .get_blocks_in_slot_range(SlotNo(start), SlotNo(start + 100))
                        .unwrap();
                    black_box(blocks.len());
                }
            },
            BatchSize::PerIteration,
        );
    });
}

// ---------------------------------------------------------------------------
// 2. ImmutableDB: in-memory vs mmap open time + lookup
// ---------------------------------------------------------------------------

fn bench_immutabledb_open(c: &mut Criterion) {
    let mut group = c.benchmark_group("immutabledb/open");

    for &size in &[1_000u64, 10_000] {
        let dir = tempfile::tempdir().unwrap();
        create_chunk_file(dir.path(), 0, size);

        group.bench_with_input(BenchmarkId::new("in_memory", size), &size, |b, _| {
            b.iter(|| {
                let db = ImmutableDB::open(dir.path()).unwrap();
                black_box(db.total_blocks());
            });
        });

        let mmap_config = ImmutableConfig {
            index_type: BlockIndexType::Mmap,
            mmap_load_factor: 0.7,
            mmap_initial_capacity: 0,
        };

        // First open builds the mmap file
        {
            let _ = ImmutableDB::open_with_config(dir.path(), &mmap_config).unwrap();
        }

        group.bench_with_input(BenchmarkId::new("mmap_cached", size), &size, |b, _| {
            b.iter(|| {
                let db = ImmutableDB::open_with_config(dir.path(), &mmap_config).unwrap();
                black_box(db.total_blocks());
            });
        });

        // Benchmark cold mmap open (delete hash_index.dat each time to force rebuild)
        group.bench_with_input(
            BenchmarkId::new("mmap_cold_rebuild", size),
            &size,
            |b, _| {
                b.iter(|| {
                    let _ = std::fs::remove_file(dir.path().join("hash_index.dat"));
                    let db = ImmutableDB::open_with_config(dir.path(), &mmap_config).unwrap();
                    black_box(db.total_blocks());
                });
            },
        );
    }

    group.finish();
}

fn bench_immutabledb_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("immutabledb/lookup");

    let size = 10_000u64;
    let dir = tempfile::tempdir().unwrap();
    let hashes = create_chunk_file(dir.path(), 0, size);
    let lookup_hashes: Vec<Hash32> = random_lookup_hashes(NUM_LOOKUPS, size)
        .into_iter()
        .enumerate()
        .map(|(i, _)| hashes[(i * 37) % hashes.len()])
        .collect();

    // In-memory
    let db_inmem = ImmutableDB::open(dir.path()).unwrap();
    group.bench_function(BenchmarkId::new("in_memory", size), |b| {
        b.iter(|| {
            for hash in &lookup_hashes {
                let result = db_inmem.get_block(hash);
                black_box(result.is_some());
            }
        });
    });
    drop(db_inmem);

    // Mmap
    let mmap_config = ImmutableConfig {
        index_type: BlockIndexType::Mmap,
        mmap_load_factor: 0.7,
        mmap_initial_capacity: 0,
    };
    let db_mmap = ImmutableDB::open_with_config(dir.path(), &mmap_config).unwrap();
    group.bench_function(BenchmarkId::new("mmap", size), |b| {
        b.iter(|| {
            for hash in &lookup_hashes {
                let result = db_mmap.get_block(hash);
                black_box(result.is_some());
            }
        });
    });

    group.finish();
}

fn bench_immutabledb_has_block(c: &mut Criterion) {
    let mut group = c.benchmark_group("immutabledb/has_block");

    let size = 10_000u64;
    let dir = tempfile::tempdir().unwrap();
    let hashes = create_chunk_file(dir.path(), 0, size);

    // Mix of existing and non-existing hashes
    let mut lookup_hashes: Vec<Hash32> = Vec::with_capacity(NUM_LOOKUPS);
    for i in 0..NUM_LOOKUPS {
        if i % 5 == 0 {
            // Non-existing hash
            lookup_hashes.push(make_hash(size + i as u64 + 1000));
        } else {
            lookup_hashes.push(hashes[(i * 31) % hashes.len()]);
        }
    }

    // In-memory
    let db_inmem = ImmutableDB::open(dir.path()).unwrap();
    group.bench_function("in_memory", |b| {
        b.iter(|| {
            for hash in &lookup_hashes {
                black_box(db_inmem.has_block(hash));
            }
        });
    });
    drop(db_inmem);

    // Mmap
    let mmap_config = ImmutableConfig {
        index_type: BlockIndexType::Mmap,
        mmap_load_factor: 0.7,
        mmap_initial_capacity: 0,
    };
    let db_mmap = ImmutableDB::open_with_config(dir.path(), &mmap_config).unwrap();
    group.bench_function("mmap", |b| {
        b.iter(|| {
            for hash in &lookup_hashes {
                black_box(db_mmap.has_block(hash));
            }
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// 3. BlockIndex raw operations (insert + lookup throughput)
// ---------------------------------------------------------------------------

fn bench_block_index_insert(c: &mut Criterion) {
    use torsten_storage::config::BlockIndexType;

    let mut group = c.benchmark_group("block_index/insert");

    for &size in BLOCK_INDEX_SIZES {
        let hashes: Vec<Hash32> = (0..size).map(make_hash).collect();

        group.bench_with_input(BenchmarkId::new("in_memory", size), &size, |b, _| {
            b.iter(|| {
                let config = ImmutableConfig {
                    index_type: BlockIndexType::InMemory,
                    ..ImmutableConfig::default()
                };
                let dir = tempfile::tempdir().unwrap();
                let mut idx =
                    torsten_storage::block_index::BlockIndex::new(&config, dir.path()).unwrap();
                for (i, hash) in hashes.iter().enumerate() {
                    idx.insert(
                        *hash,
                        torsten_storage::block_index::BlockLocation {
                            chunk_num: 0,
                            block_offset: i as u64 * 1024,
                            block_end: (i as u64 + 1) * 1024,
                        },
                    );
                }
                black_box(idx.len());
            });
        });

        group.bench_with_input(BenchmarkId::new("mmap", size), &size, |b, _| {
            b.iter(|| {
                let config = ImmutableConfig {
                    index_type: BlockIndexType::Mmap,
                    mmap_load_factor: 0.7,
                    mmap_initial_capacity: 0,
                };
                let dir = tempfile::tempdir().unwrap();
                let mut idx =
                    torsten_storage::block_index::BlockIndex::new(&config, dir.path()).unwrap();
                for (i, hash) in hashes.iter().enumerate() {
                    idx.insert(
                        *hash,
                        torsten_storage::block_index::BlockLocation {
                            chunk_num: 0,
                            block_offset: i as u64 * 1024,
                            block_end: (i as u64 + 1) * 1024,
                        },
                    );
                }
                black_box(idx.len());
            });
        });
    }

    group.finish();
}

fn bench_block_index_lookup(c: &mut Criterion) {
    use torsten_storage::config::BlockIndexType;

    let mut group = c.benchmark_group("block_index/lookup");

    for &size in BLOCK_INDEX_SIZES {
        let hashes: Vec<Hash32> = (0..size).map(make_hash).collect();
        let lookup_indices = random_lookup_hashes(NUM_LOOKUPS, size);
        // Use actual inserted hashes for lookups
        let lookup_hashes: Vec<Hash32> = (0..NUM_LOOKUPS)
            .map(|i| hashes[(i * 37) % hashes.len()])
            .collect();

        // In-memory
        let dir_inmem = tempfile::tempdir().unwrap();
        let config_inmem = ImmutableConfig {
            index_type: BlockIndexType::InMemory,
            ..ImmutableConfig::default()
        };
        let mut idx_inmem =
            torsten_storage::block_index::BlockIndex::new(&config_inmem, dir_inmem.path()).unwrap();
        for (i, hash) in hashes.iter().enumerate() {
            idx_inmem.insert(
                *hash,
                torsten_storage::block_index::BlockLocation {
                    chunk_num: 0,
                    block_offset: i as u64 * 1024,
                    block_end: (i as u64 + 1) * 1024,
                },
            );
        }

        group.bench_with_input(BenchmarkId::new("in_memory", size), &size, |b, _| {
            b.iter(|| {
                for hash in &lookup_hashes {
                    black_box(idx_inmem.lookup(hash));
                }
            });
        });

        // Mmap
        let dir_mmap = tempfile::tempdir().unwrap();
        let config_mmap = ImmutableConfig {
            index_type: BlockIndexType::Mmap,
            mmap_load_factor: 0.7,
            mmap_initial_capacity: 0,
        };
        let mut idx_mmap =
            torsten_storage::block_index::BlockIndex::new(&config_mmap, dir_mmap.path()).unwrap();
        for (i, hash) in hashes.iter().enumerate() {
            idx_mmap.insert(
                *hash,
                torsten_storage::block_index::BlockLocation {
                    chunk_num: 0,
                    block_offset: i as u64 * 1024,
                    block_end: (i as u64 + 1) * 1024,
                },
            );
        }

        group.bench_with_input(BenchmarkId::new("mmap", size), &size, |b, _| {
            b.iter(|| {
                for hash in &lookup_hashes {
                    black_box(idx_mmap.lookup(hash));
                }
            });
        });

        let _ = lookup_indices;
    }

    group.finish();
}

fn bench_block_index_contains_miss(c: &mut Criterion) {
    use torsten_storage::config::BlockIndexType;

    let mut group = c.benchmark_group("block_index/contains_miss");
    let size = 10_000u64;
    let hashes: Vec<Hash32> = (0..size).map(make_hash).collect();

    // Non-existing hashes (offset by size to guarantee misses)
    let miss_hashes: Vec<Hash32> = (size..size + NUM_LOOKUPS as u64).map(make_hash).collect();

    // In-memory
    let dir = tempfile::tempdir().unwrap();
    let config = ImmutableConfig {
        index_type: BlockIndexType::InMemory,
        ..ImmutableConfig::default()
    };
    let mut idx = torsten_storage::block_index::BlockIndex::new(&config, dir.path()).unwrap();
    for (i, hash) in hashes.iter().enumerate() {
        idx.insert(
            *hash,
            torsten_storage::block_index::BlockLocation {
                chunk_num: 0,
                block_offset: i as u64 * 1024,
                block_end: (i as u64 + 1) * 1024,
            },
        );
    }

    group.bench_function("in_memory", |b| {
        b.iter(|| {
            for hash in &miss_hashes {
                black_box(idx.contains(hash));
            }
        });
    });
    drop(idx);

    // Mmap
    let dir2 = tempfile::tempdir().unwrap();
    let config2 = ImmutableConfig {
        index_type: BlockIndexType::Mmap,
        mmap_load_factor: 0.7,
        mmap_initial_capacity: 0,
    };
    let mut idx2 = torsten_storage::block_index::BlockIndex::new(&config2, dir2.path()).unwrap();
    for (i, hash) in hashes.iter().enumerate() {
        idx2.insert(
            *hash,
            torsten_storage::block_index::BlockLocation {
                chunk_num: 0,
                block_offset: i as u64 * 1024,
                block_end: (i as u64 + 1) * 1024,
            },
        );
    }

    group.bench_function("mmap", |b| {
        b.iter(|| {
            for hash in &miss_hashes {
                black_box(idx2.contains(hash));
            }
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// 4. ChainDB with mmap profile comparison
// ---------------------------------------------------------------------------

fn bench_chaindb_inmem_vs_mmap(c: &mut Criterion) {
    let mut group = c.benchmark_group("chaindb/profile_comparison");
    let lookup_hashes = random_lookup_hashes(NUM_LOOKUPS, NUM_BLOCKS);

    let inmem_config = ImmutableConfig {
        index_type: BlockIndexType::InMemory,
        ..ImmutableConfig::default()
    };
    let mmap_config = ImmutableConfig {
        index_type: BlockIndexType::Mmap,
        mmap_load_factor: 0.7,
        mmap_initial_capacity: 0,
    };

    // Insert throughput comparison
    group.bench_function("insert_5k/in_memory", |b| {
        b.iter_batched(
            || tempfile::tempdir().unwrap(),
            |dir| {
                populate_chaindb_with_config(dir.path(), NUM_BLOCKS, &inmem_config);
            },
            BatchSize::PerIteration,
        );
    });

    group.bench_function("insert_5k/mmap", |b| {
        b.iter_batched(
            || tempfile::tempdir().unwrap(),
            |dir| {
                populate_chaindb_with_config(dir.path(), NUM_BLOCKS, &mmap_config);
            },
            BatchSize::PerIteration,
        );
    });

    // Read throughput comparison
    group.bench_function("read_500/in_memory", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let (db, _) = populate_chaindb_with_config(dir.path(), NUM_BLOCKS, &inmem_config);
                (dir, db, lookup_hashes.clone())
            },
            |(_dir, db, hashes)| {
                for hash in &hashes {
                    let r = db.get_block(hash).unwrap();
                    assert!(r.is_some());
                }
            },
            BatchSize::PerIteration,
        );
    });

    group.bench_function("read_500/mmap", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let (db, _) = populate_chaindb_with_config(dir.path(), NUM_BLOCKS, &mmap_config);
                (dir, db, lookup_hashes.clone())
            },
            |(_dir, db, hashes)| {
                for hash in &hashes {
                    let r = db.get_block(hash).unwrap();
                    assert!(r.is_some());
                }
            },
            BatchSize::PerIteration,
        );
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// 5. ImmutableDB append throughput
// ---------------------------------------------------------------------------

fn bench_immutabledb_append(c: &mut Criterion) {
    let mut group = c.benchmark_group("immutabledb/append");
    let block_data = vec![0u8; 1024];

    for &index_type in &["in_memory", "mmap"] {
        let config = ImmutableConfig {
            index_type: if index_type == "mmap" {
                BlockIndexType::Mmap
            } else {
                BlockIndexType::InMemory
            },
            mmap_load_factor: 0.7,
            mmap_initial_capacity: 0,
        };

        group.bench_function(BenchmarkId::new("1k_blocks", index_type), |b| {
            b.iter_batched(
                || {
                    let dir = tempfile::tempdir().unwrap();
                    std::fs::create_dir_all(dir.path()).unwrap();
                    let db =
                        ImmutableDB::open_for_writing_with_config(dir.path(), &config).unwrap();
                    (dir, db)
                },
                |(_dir, mut db)| {
                    for i in 0..1_000u64 {
                        let hash = make_hash(i);
                        db.append_block(i + 1, i + 1, &hash, &block_data).unwrap();
                    }
                    black_box(db.total_blocks());
                },
                BatchSize::PerIteration,
            );
        });
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// 6. ImmutableDB slot range query (in-memory vs mmap)
// ---------------------------------------------------------------------------

fn bench_immutabledb_slot_range(c: &mut Criterion) {
    let mut group = c.benchmark_group("immutabledb/slot_range");
    let size = 10_000u64;

    for &index_type in &["in_memory", "mmap"] {
        let dir = tempfile::tempdir().unwrap();
        create_chunk_file(dir.path(), 0, size);

        let config = ImmutableConfig {
            index_type: if index_type == "mmap" {
                BlockIndexType::Mmap
            } else {
                BlockIndexType::InMemory
            },
            mmap_load_factor: 0.7,
            mmap_initial_capacity: 0,
        };
        let db = ImmutableDB::open_with_config(dir.path(), &config).unwrap();

        group.bench_function(BenchmarkId::new("range_100", index_type), |b| {
            let mut rng = 42u64;
            b.iter(|| {
                let start = (lcg_next(&mut rng) % (size - 100)) + 1;
                let blocks = db.get_blocks_in_slot_range(start, start + 100);
                black_box(blocks.len());
            });
        });
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// 7. Dataset scaling — measure how performance degrades as dataset grows
// ---------------------------------------------------------------------------

/// Scaling sizes: 1K → 5K → 10K → 25K → 50K → 100K
const SCALING_SIZES: &[u64] = &[1_000, 5_000, 10_000, 25_000, 50_000, 100_000];

fn bench_block_index_scaling_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("scaling/block_index_insert");
    group.sample_size(10); // larger datasets take longer

    for &size in SCALING_SIZES {
        let hashes: Vec<Hash32> = (0..size).map(make_hash).collect();

        group.bench_with_input(BenchmarkId::new("in_memory", size), &size, |b, _| {
            b.iter(|| {
                let config = ImmutableConfig {
                    index_type: BlockIndexType::InMemory,
                    ..ImmutableConfig::default()
                };
                let dir = tempfile::tempdir().unwrap();
                let mut idx =
                    torsten_storage::block_index::BlockIndex::new(&config, dir.path()).unwrap();
                for (i, hash) in hashes.iter().enumerate() {
                    idx.insert(
                        *hash,
                        torsten_storage::block_index::BlockLocation {
                            chunk_num: 0,
                            block_offset: i as u64 * 1024,
                            block_end: (i as u64 + 1) * 1024,
                        },
                    );
                }
                black_box(idx.len());
            });
        });

        group.bench_with_input(BenchmarkId::new("mmap", size), &size, |b, _| {
            b.iter(|| {
                let config = ImmutableConfig {
                    index_type: BlockIndexType::Mmap,
                    mmap_load_factor: 0.7,
                    mmap_initial_capacity: 0,
                };
                let dir = tempfile::tempdir().unwrap();
                let mut idx =
                    torsten_storage::block_index::BlockIndex::new(&config, dir.path()).unwrap();
                for (i, hash) in hashes.iter().enumerate() {
                    idx.insert(
                        *hash,
                        torsten_storage::block_index::BlockLocation {
                            chunk_num: 0,
                            block_offset: i as u64 * 1024,
                            block_end: (i as u64 + 1) * 1024,
                        },
                    );
                }
                black_box(idx.len());
            });
        });
    }

    group.finish();
}

fn bench_block_index_scaling_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("scaling/block_index_lookup");
    group.sample_size(10);

    for &size in SCALING_SIZES {
        let hashes: Vec<Hash32> = (0..size).map(make_hash).collect();
        let lookup_hashes: Vec<Hash32> = (0..NUM_LOOKUPS)
            .map(|i| hashes[(i * 37) % hashes.len()])
            .collect();

        // In-memory
        let dir_inmem = tempfile::tempdir().unwrap();
        let config_inmem = ImmutableConfig {
            index_type: BlockIndexType::InMemory,
            ..ImmutableConfig::default()
        };
        let mut idx_inmem =
            torsten_storage::block_index::BlockIndex::new(&config_inmem, dir_inmem.path()).unwrap();
        for (i, hash) in hashes.iter().enumerate() {
            idx_inmem.insert(
                *hash,
                torsten_storage::block_index::BlockLocation {
                    chunk_num: 0,
                    block_offset: i as u64 * 1024,
                    block_end: (i as u64 + 1) * 1024,
                },
            );
        }

        group.bench_with_input(BenchmarkId::new("in_memory", size), &size, |b, _| {
            b.iter(|| {
                for hash in &lookup_hashes {
                    black_box(idx_inmem.lookup(hash));
                }
            });
        });

        // Mmap
        let dir_mmap = tempfile::tempdir().unwrap();
        let config_mmap = ImmutableConfig {
            index_type: BlockIndexType::Mmap,
            mmap_load_factor: 0.7,
            mmap_initial_capacity: 0,
        };
        let mut idx_mmap =
            torsten_storage::block_index::BlockIndex::new(&config_mmap, dir_mmap.path()).unwrap();
        for (i, hash) in hashes.iter().enumerate() {
            idx_mmap.insert(
                *hash,
                torsten_storage::block_index::BlockLocation {
                    chunk_num: 0,
                    block_offset: i as u64 * 1024,
                    block_end: (i as u64 + 1) * 1024,
                },
            );
        }

        group.bench_with_input(BenchmarkId::new("mmap", size), &size, |b, _| {
            b.iter(|| {
                for hash in &lookup_hashes {
                    black_box(idx_mmap.lookup(hash));
                }
            });
        });
    }

    group.finish();
}

fn bench_immutabledb_scaling_open(c: &mut Criterion) {
    let mut group = c.benchmark_group("scaling/immutabledb_open");
    group.sample_size(10);

    let open_sizes: &[u64] = &[1_000, 5_000, 10_000, 25_000, 50_000];

    for &size in open_sizes {
        let dir = tempfile::tempdir().unwrap();
        create_chunk_file(dir.path(), 0, size);

        group.bench_with_input(BenchmarkId::new("in_memory", size), &size, |b, _| {
            b.iter(|| {
                let db = ImmutableDB::open(dir.path()).unwrap();
                black_box(db.total_blocks());
            });
        });

        let mmap_config = ImmutableConfig {
            index_type: BlockIndexType::Mmap,
            mmap_load_factor: 0.7,
            mmap_initial_capacity: 0,
        };
        // Pre-build mmap file
        {
            let _ = ImmutableDB::open_with_config(dir.path(), &mmap_config).unwrap();
        }

        group.bench_with_input(BenchmarkId::new("mmap_cached", size), &size, |b, _| {
            b.iter(|| {
                let db = ImmutableDB::open_with_config(dir.path(), &mmap_config).unwrap();
                black_box(db.total_blocks());
            });
        });
    }

    group.finish();
}

fn bench_chaindb_scaling_insert(c: &mut Criterion) {
    let mut group = c.benchmark_group("scaling/chaindb_insert");
    group.sample_size(10);

    let chaindb_sizes: &[u64] = &[1_000, 5_000, 10_000, 25_000];

    for &size in chaindb_sizes {
        group.bench_with_input(BenchmarkId::new("default", size), &size, |b, &size| {
            b.iter_batched(
                || tempfile::tempdir().unwrap(),
                |dir| {
                    populate_chaindb(dir.path(), size);
                },
                BatchSize::PerIteration,
            );
        });
    }

    group.finish();
}

// ===========================================================================
// Criterion harness
// ===========================================================================

criterion_group!(
    chaindb_benches,
    bench_chaindb_sequential_insert,
    bench_chaindb_random_read,
    bench_chaindb_tip_query,
    bench_chaindb_has_block,
    bench_chaindb_slot_range_query,
    bench_chaindb_inmem_vs_mmap,
);

criterion_group!(
    immutabledb_benches,
    bench_immutabledb_open,
    bench_immutabledb_lookup,
    bench_immutabledb_has_block,
    bench_immutabledb_append,
    bench_immutabledb_slot_range,
);

criterion_group!(
    block_index_benches,
    bench_block_index_insert,
    bench_block_index_lookup,
    bench_block_index_contains_miss,
);

criterion_group!(
    scaling_benches,
    bench_block_index_scaling_insert,
    bench_block_index_scaling_lookup,
    bench_immutabledb_scaling_open,
    bench_chaindb_scaling_insert,
);

criterion_main!(
    chaindb_benches,
    immutabledb_benches,
    block_index_benches,
    scaling_benches
);
