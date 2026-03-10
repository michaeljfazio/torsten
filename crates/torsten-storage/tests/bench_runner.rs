//! Manual benchmark runner for the cardano-lsm storage backend.
//! Run: cargo test -p torsten-storage --test bench_runner --release -- --nocapture

use std::time::{Duration, Instant};
use torsten_primitives::hash::Hash32;
use torsten_primitives::time::{BlockNo, SlotNo};

use torsten_storage::lsm::LsmImmutableDB as DB;

const NUM_BLOCKS: u64 = 5_000;
const NUM_LOOKUPS: usize = 500;
const BATCH_SIZE: usize = 100;
const BLOCK_DATA_SIZE: usize = 1024;
const BENCH_ITERS: usize = 3;

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

fn make_block_data(index: u64) -> Vec<u8> {
    let mut data = vec![0u8; BLOCK_DATA_SIZE];
    let seed = index.to_le_bytes();
    for (i, byte) in data.iter_mut().enumerate() {
        *byte = seed[i % 8].wrapping_add(i as u8);
    }
    data
}

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

fn print_result(name: &str, mean: Duration) {
    println!("  {:<40} {:>12.3} ms", name, mean.as_secs_f64() * 1000.0);
}

fn print_result_with_throughput(name: &str, mean: Duration, ops: usize, unit: &str) {
    println!(
        "  {:<40} {:>12.3} ms  ({:.0} {}/sec)",
        name,
        mean.as_secs_f64() * 1000.0,
        ops as f64 / mean.as_secs_f64(),
        unit,
    );
}

#[test]
fn run_all_benchmarks() {
    let blocks = generate_blocks(NUM_BLOCKS);

    println!("\n========================================");
    println!("  Storage Benchmark: CARDANO-LSM");
    println!(
        "  {} blocks, {} lookups, {}B block size",
        NUM_BLOCKS, NUM_LOOKUPS, BLOCK_DATA_SIZE
    );
    println!("  {} bench iters", BENCH_ITERS);
    println!("========================================\n");

    // Pre-compute random lookup indices with simple LCG
    let mut rng_state: u64 = 42;
    let lookup_hashes: Vec<Hash32> = (0..NUM_LOOKUPS)
        .map(|_| {
            rng_state = rng_state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            make_hash(rng_state % NUM_BLOCKS)
        })
        .collect();

    rng_state = 123;
    let lookup_slots: Vec<SlotNo> = (0..NUM_LOOKUPS)
        .map(|_| {
            rng_state = rng_state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            SlotNo((rng_state % NUM_BLOCKS) + 1)
        })
        .collect();

    let nonexistent_hashes: Vec<Hash32> = (NUM_BLOCKS..NUM_BLOCKS + NUM_LOOKUPS as u64)
        .map(make_hash)
        .collect();

    println!("  --- Write benchmarks ---\n");

    // 1. Sequential insert
    {
        let mut total = Duration::ZERO;
        for _ in 0..BENCH_ITERS {
            let dir = tempfile::tempdir().unwrap();
            let mut db = open_db(dir.path());
            let start = Instant::now();
            for (slot, hash, block_no, data) in &blocks {
                db.put_block(*slot, hash, *block_no, data).unwrap();
            }
            total += start.elapsed();
        }
        print_result("lsm/sequential_insert", total / BENCH_ITERS as u32);
    }

    // 2. Batch insert
    {
        let mut total = Duration::ZERO;
        for _ in 0..BENCH_ITERS {
            let dir = tempfile::tempdir().unwrap();
            let mut db = open_db(dir.path());
            let start = Instant::now();
            for chunk in blocks.chunks(BATCH_SIZE) {
                let batch: Vec<_> = chunk
                    .iter()
                    .map(|(s, h, b, d)| (*s, h, *b, d.as_slice()))
                    .collect();
                db.put_blocks_batch(&batch).unwrap();
            }
            total += start.elapsed();
        }
        print_result("lsm/batch_insert", total / BENCH_ITERS as u32);
    }

    println!("\n  --- Read benchmarks (pre-populated DB, read-only timing) ---\n");

    // 3. Random read by hash
    {
        let dir = tempfile::tempdir().unwrap();
        let db = populate_db(dir.path(), &blocks);
        let mut total = Duration::ZERO;
        for _ in 0..BENCH_ITERS {
            let start = Instant::now();
            for hash in &lookup_hashes {
                let result = db.get_block_by_hash(hash).unwrap();
                assert!(result.is_some());
            }
            total += start.elapsed();
        }
        print_result_with_throughput(
            "lsm/random_read_by_hash",
            total / BENCH_ITERS as u32,
            NUM_LOOKUPS,
            "ops",
        );
    }

    // 4. Random read by slot
    {
        let dir = tempfile::tempdir().unwrap();
        let db = populate_db(dir.path(), &blocks);
        let mut total = Duration::ZERO;
        for _ in 0..BENCH_ITERS {
            let start = Instant::now();
            for slot in &lookup_slots {
                let result = db.get_block_by_slot(*slot).unwrap();
                assert!(result.is_some());
            }
            total += start.elapsed();
        }
        print_result_with_throughput(
            "lsm/random_read_by_slot",
            total / BENCH_ITERS as u32,
            NUM_LOOKUPS,
            "ops",
        );
    }

    // 5. Sequential scan
    {
        let dir = tempfile::tempdir().unwrap();
        let db = populate_db(dir.path(), &blocks);
        let mut total = Duration::ZERO;
        for _ in 0..BENCH_ITERS {
            let start = Instant::now();
            let result = db
                .get_blocks_in_slot_range(SlotNo(1), SlotNo(NUM_BLOCKS))
                .unwrap();
            assert_eq!(result.len(), NUM_BLOCKS as usize);
            total += start.elapsed();
        }
        print_result_with_throughput(
            "lsm/sequential_scan",
            total / BENCH_ITERS as u32,
            NUM_BLOCKS as usize,
            "blocks",
        );
    }

    // 6. Negative lookup (bloom filter effectiveness)
    {
        let dir = tempfile::tempdir().unwrap();
        let db = populate_db(dir.path(), &blocks);
        let mut total = Duration::ZERO;
        for _ in 0..BENCH_ITERS {
            let start = Instant::now();
            for hash in &nonexistent_hashes {
                let result = db.get_block_by_hash(hash).unwrap();
                assert!(result.is_none());
            }
            total += start.elapsed();
        }
        print_result_with_throughput(
            "lsm/negative_lookup",
            total / BENCH_ITERS as u32,
            NUM_LOOKUPS,
            "ops",
        );
    }

    // 7. Tip query
    {
        let dir = tempfile::tempdir().unwrap();
        let db = populate_db(dir.path(), &blocks);
        let mut total = Duration::ZERO;
        for _ in 0..BENCH_ITERS {
            let start = Instant::now();
            for _ in 0..NUM_LOOKUPS {
                let tip = db.get_tip_info();
                assert!(tip.is_some());
            }
            total += start.elapsed();
        }
        print_result_with_throughput(
            "lsm/tip_query",
            total / BENCH_ITERS as u32,
            NUM_LOOKUPS,
            "ops",
        );
    }

    println!("\n========================================\n");
}
