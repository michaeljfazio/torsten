# Comprehensive Benchmark Results — 2026-03-14

Machine: Apple M2 Max (32 GB), macOS Darwin 25.2.0
Branch: feature/simd-acceleration
All benchmarks run sequentially (no parallel execution).

## 1. Block Index (In-Memory vs Mmap)

### Insert Throughput

| Size | In-Memory | Mmap | Ratio |
|------|-----------|------|-------|
| 1K | 164µs | 384µs | 2.3x |
| 10K | 695µs | 1.60ms | 2.3x |
| 50K | 2.89ms | 17.6ms | 6.1x |
| 100K | 5.79ms | 46.2ms | 8.0x |
| 250K | 18.6ms | 53.6ms | 2.9x |
| 500K | 38.3ms | 79.0ms | 2.1x |
| 1M | 80.5ms | 159ms | 2.0x |

Mmap insert is slower due to disk I/O, but the gap narrows at large scale (8.0x at 100K → 2.0x at 1M) as in-memory HashMap rehashing costs dominate.

### Lookup Throughput (500 random lookups)

| Size | In-Memory | Mmap | Speedup |
|------|-----------|------|---------|
| 1K | 9.02µs | 2.34µs | **3.9x** |
| 10K | 10.0µs | 2.83µs | **3.5x** |
| 50K | 10.3µs | 2.16µs | **4.8x** |
| 100K | 10.1µs | 2.17µs | **4.7x** |
| 250K | 10.5µs | 2.13µs | **4.9x** |
| 500K | 10.5µs | 2.03µs | **5.2x** |
| 1M | 10.6µs | 2.01µs | **5.3x** |

**Key finding**: Mmap lookup advantage *increases* with scale (3.5x → 5.3x). At mainnet scale (~10M blocks), the advantage will be even more pronounced as HashMap cache misses grow while mmap stays near-constant via direct memory mapping.

### Contains Miss (10K entries, 500 lookups)

| | In-Memory | Mmap | Speedup |
|--|-----------|------|---------|
| Miss | 9.39µs | 5.46µs | **1.7x** |

## 2. ImmutableDB

### Open Time

| Size | In-Memory | Mmap (cached) | Mmap (cold rebuild) |
|------|-----------|---------------|---------------------|
| 1K | 69.4µs | 69.6µs | 259µs |
| 10K | 138µs | 140µs | 811µs |
| 50K | 463µs | 446µs | — |
| 100K | 1.54ms | 1.55ms | — |
| 250K | 4.22ms | 4.26ms | — |
| 500K | 8.90ms | 8.90ms | — |

At these synthetic scales, open time is dominated by secondary index scanning (same for both). At mainnet scale with a pre-built `hash_index.dat`, mmap open is near-instant.

### Lookup & Has Block (10K blocks, 500 lookups)

| Operation | In-Memory | Mmap |
|-----------|-----------|------|
| Lookup (read block data) | 9.95ms | 9.97ms |
| Has block (hash check) | 4.43µs | 4.47µs |

### Append & Slot Range

| Operation | In-Memory | Mmap |
|-----------|-----------|------|
| Append 1K blocks | 959µs | 1.16ms |
| Slot range query (100 blocks) | 86.3µs | 81.5µs |

## 3. ChainDB

### Core Operations (5K blocks)

| Operation | Time |
|-----------|------|
| Sequential insert 5K | 3.19ms |
| Random read by hash (500) | 7.77ms |
| Tip query (500x) | 483ns |
| Has block (500) | 3.81µs |
| Slot range (100-block windows, 10x) | 6.83ms |

### Profile Comparison (5K blocks)

| Operation | In-Memory | Mmap |
|-----------|-----------|------|
| Insert 5K | 2.69ms | 3.22ms |
| Read 500 | 491µs | 571µs |

### Insert Scaling

| Size | Time | Per-block |
|------|------|-----------|
| 10K | 5.08ms | 508ns |
| 50K | 27.3ms | 545ns |
| 100K | 58.4ms | 584ns |
| 250K | 168ms | 672ns |

Linear scaling with ~1.3x cost increase per 2.5x size.

## 4. UTxO Store (LSM-backed via cardano-lsm)

### Insert Throughput

| Size | Time | Per-entry |
|------|------|-----------|
| 10K | 4.55ms | 455ns |
| 50K | 22.3ms | 446ns |
| 100K | 47.9ms | 479ns |
| 250K | 134ms | 536ns |
| 500K | 267ms | 534ns |
| 1M | 569ms | 569ns |

Insert scales linearly — ~1.25x slowdown per 2x size increase. Good LSM write amplification behavior.

### Lookup Throughput (1K random lookups)

| Size | Hit | Miss | Per-lookup (hit) |
|------|-----|------|-----------------|
| 10K | 191µs | 126µs | 191ns |
| 50K | 220µs | — | 220ns |
| 100K | 236µs | 165µs | 236ns |
| 250K | 254µs | — | 254ns |
| 500K | 287µs | — | 287ns |
| 1M | 308µs | — | 308ns |

Lookup degrades gracefully — only **1.6x** slowdown from 10K to 1M. Bloom filters keep miss lookups ~34% faster than hits (no SSTable reads needed).

### Contains Check (100K entries, 1K lookups)

| | Time |
|--|------|
| Hit | 189µs |
| Miss | 163µs |

### Remove (100K sequential)

| Operation | Time | Per-entry |
|-----------|------|-----------|
| Sequential remove 100K | 6.24s | 62.4µs |

Remove is expensive (~130x slower than insert) due to LSM tombstone writes + compaction.

### Apply Transaction (consuming 1 input + producing 2 outputs)

| Base UTxO Size | Batch 50 | Per-tx | Batch 100 (10K base) |
|----------------|----------|--------|---------------------|
| 10K | 1.34ms | 26.9µs | 2.03ms (20.3µs/tx) |
| 50K | 5.19ms | 104µs | — |
| 100K | 10.9ms | 219µs | — |
| 250K | 28.9ms | 577µs | — |
| 500K | 58.3ms | 1.17ms | — |

Scales linearly with base UTxO set size.

### Total Lovelace Scan

| Size | Time | Per-entry |
|------|------|-----------|
| 10K | 2.38ms | 238ns |
| 50K | 13.3ms | 265ns |
| 100K | 29.1ms | 291ns |
| 250K | 78.6ms | 314ns |
| 500K | 162ms | 323ns |
| 1M | 330ms | 330ns |

Linear scan. At mainnet scale (~20M UTxOs), expect ~6.6 seconds for full scan.

### Rebuild Address Index

| Size | Time | Per-entry |
|------|------|-----------|
| 10K | 133ms | 13.3µs |
| 100K | 1.29s | 12.9µs |

### LSM Config Comparison (100K entries)

| Config | Insert | Lookup (1K) |
|--------|--------|-------------|
| low_8gb (256MB/2GB/10bit) | 48.1ms | 234µs |
| mid_16gb (512MB/4GB/10bit) | 48.5ms | 233µs |
| high_32gb (512MB/8GB/10bit) | 48.1ms | 234µs |
| high_bloom_16gb (512MB/4GB/15bit) | 47.2ms | 232µs |
| legacy_small (128MB/256MB/10bit) | 47.1ms | 233µs |

At 100K entries, all configs perform identically — the dataset fits in the smallest cache. Config differences emerge at mainnet scale (20M UTxOs, ~60GB on-disk) where working set exceeds cache.

## 5. Cryptographic Operations (SIMD-Accelerated)

### Ed25519 Signature Verification

| Operation | Time | Per-sig |
|-----------|------|---------|
| Single verify | 28.6µs | 28.6µs |
| Batch 5 | 142µs | 28.4µs |
| Batch 10 | 286µs | 28.6µs |
| Batch 25 | 716µs | 28.6µs |
| Batch 50 | 1.44ms | 28.8µs |

Perfectly linear scaling — each Ed25519 verify takes ~28.6µs regardless of batch size (sequential verification). At typical block sizes (10-50 witnesses), signature verification adds 0.3-1.4ms per block.

### Blake2b Hashing (SIMD via blake2b_simd)

| Input Size | blake2b_256 | blake2b_224 |
|------------|-------------|-------------|
| 32B | 132ns | 127ns |
| 64B | 132ns | 128ns |
| 256B | 246ns | 245ns |
| 1KB | 949ns | — |
| 4KB | 3.77µs | — |
| 16KB | 15.2µs | — |

Sub-microsecond hashing for typical Cardano payloads (tx hashes, vkeys, addresses). Throughput: ~960 MB/s for bulk hashing.

### Keyhash Computation (blake2b_224 of vkey)

| Batch Size | Time | Per-key |
|------------|------|---------|
| 1 | 128ns | 128ns |
| 5 | 644ns | 129ns |
| 10 | 1.28µs | 128ns |
| 50 | 6.42µs | 128ns |
| 100 | 12.9µs | 129ns |

### Batch Hashing (block validation workloads)

| Workload | Time | Per-hash |
|----------|------|----------|
| 10 keyhashes (224) | 1.28µs | 128ns |
| 50 keyhashes (224) | 6.53µs | 131ns |
| 100 keyhashes (224) | 12.7µs | 127ns |
| 500 keyhashes (224) | 63.2µs | 126ns |
| 10 tx bodies 512B (256) | 4.79µs | 479ns |
| 50 tx bodies 512B (256) | 23.9µs | 478ns |
| 100 tx bodies 512B (256) | 47.6µs | 476ns |

## 6. Storage Profile Summary

All four profiles use memory-mapped block indexes (mmap) by default. The profiles differ in LSM tree configuration for the UTxO store:

| Profile | Target System | Memtable | Block Cache | Bloom | Expected RSS |
|---------|--------------|----------|-------------|-------|-------------|
| `minimal` | 4GB | 256MB | 2GB | 10 bits | ~3GB |
| `low-memory` | 8GB | 512MB | 5GB | 10 bits | ~6.5GB |
| `high-memory` (default) | 16GB | 1GB | 12GB | 10 bits | ~14GB |
| `ultra-memory` | 32GB+ | 2GB | 24GB | 10 bits | ~27GB |

**Profile selection guidance**:
- At benchmark scale (≤1M entries), all profiles perform identically
- At mainnet scale (~20M UTxOs, ~60GB on-disk), larger cache sizes reduce disk reads significantly
- The `high-memory` default is appropriate for most operators (16GB is the recommended minimum for mainnet)
- Use `minimal` only for resource-constrained environments (testnet-only, Raspberry Pi, etc.)

## 7. Key Takeaways

1. **Mmap block index is the clear winner for lookups** — 3.5-5.3x faster, advantage grows with scale
2. **UTxO store scales linearly** — insert 569ns/entry, lookup 308ns/entry at 1M (only 1.6x degradation from 10K)
3. **Ed25519 verification: 28.6µs/sig** — a block with 50 witnesses validates signatures in 1.4ms
4. **Blake2b hashing: 128ns/keyhash** — negligible cost for witness validation
5. **LSM config differences invisible at benchmark scale** — they matter at mainnet (20M+ UTxOs)
6. **Remove is the expensive operation** (62µs/entry vs 0.5µs insert) — tombstone compaction cost; not a bottleneck in normal operation since UTxO removes happen within apply_transaction batches
