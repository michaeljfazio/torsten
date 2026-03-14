# Mainnet-Scale Benchmark Results — 2026-03-14

Machine: Apple M2 Max (32 GB), macOS Darwin 25.2.0
Branch: main (commit ab49598)
Rust: stable, release profile

All benchmarks use mainnet-representative parameters: 20KB average blocks,
500-byte average transactions, 50+ witnesses per block, 1M+ UTxO sets.

## Hash Functions (Blake2b)

### Single Hash Throughput

| Payload | Blake2b-256 | Blake2b-224 | Use Case |
|---------|-------------|-------------|----------|
| 32B | 120 ns | 125 ns | Transaction hash |
| 64B | 120 ns | 124 ns | Verification key hash |
| 256B | 230 ns | 237 ns | Small transaction |
| 500B | 457 ns | — | Average transaction |
| 1 KB | 905 ns | — | Transaction body |
| 4 KB | 3.62 µs | — | Large transaction |
| 16 KB | 14.4 µs | — | Block header |
| 20 KB | 18.3 µs | — | Average block body |
| 90 KB | 82.2 µs | — | Maximum block body |

### Batch Throughput

| Operation | Count | Time | Per-item |
|-----------|-------|------|----------|
| Keyhashes (224) | 10 | 1.25 µs | 125 ns |
| Keyhashes (224) | 50 | 6.22 µs | 124 ns |
| Keyhashes (224) | 100 | 12.4 µs | 124 ns |
| Keyhashes (224) | 500 | 62.3 µs | 125 ns |
| Tx body hashes (256, 500B) | 50 | 23.8 µs | 476 ns |
| Tx body hashes (256, 500B) | 100 | 47.7 µs | 477 ns |
| Tx body hashes (256, 500B) | 300 | 143 µs | 477 ns |

Linear scaling — no cache effects even at 500-item batches.

## Cryptographic Operations

### Ed25519 Signature Verification

| Witnesses | Time | Per-signature | Block % (3s budget) |
|-----------|------|---------------|---------------------|
| 1 | 27.8 µs | 27.8 µs | 0.001% |
| 10 | 282 µs | 28.2 µs | 0.009% |
| 50 | 1.41 ms | 28.2 µs | 0.047% |
| 100 | 2.85 ms | 28.5 µs | 0.095% |
| 200 | 5.72 ms | 28.6 µs | 0.191% |
| 500 | 14.3 ms | 28.7 µs | 0.478% |

Consistent ~28 µs per signature. A maximum-witness block (500 sigs) uses
<0.5% of a 3-second slot budget.

### VRF Proof Verification

| Operation | Time |
|-----------|------|
| Single VRF proof verify | 80.9 µs |

### Keyhash (Blake2b-224 of 32B vkey)

| Keys | Time | Per-key |
|------|------|---------|
| 10 | 1.21 µs | 121 ns |
| 50 | 6.07 µs | 121 ns |
| 100 | 12.0 µs | 120 ns |
| 200 | 24.1 µs | 121 ns |
| 500 | 60.4 µs | 121 ns |

## CBOR Serialization

| Operation | Time | Notes |
|-----------|------|-------|
| Encode transaction (full Conway) | 2.00 µs | 2 inputs, 2 outputs, 2 witnesses |
| Encode transaction body only | 1.13 µs | Without witness set |
| Encode block header | 1.09 µs | With VRF output |
| Encode value (ADA-only) | 16.5 ns | Single integer |
| Encode value (multi-asset) | 687 ns | 3 policies, 5 assets |

## Mempool Operations

### Add Transactions (realistic size mix: 60% 500B, 20% 200B, 15% 2KB, 5% 8KB)

| Transactions | Time | Per-tx |
|-------------|------|--------|
| 1,000 | 417 µs | 417 ns |
| 5,000 | 2.36 ms | 472 ns |
| 10,000 | 5.30 ms | 530 ns |

### Remove by Hash (simulating block inclusion)

| Pool Size | Time | Per-remove |
|-----------|------|------------|
| 1,000 | 1.10 ms | 1.10 µs |
| 5,000 | 24.9 ms | 4.98 µs |
| 10,000 | 101 ms | 10.1 µs |

Remove is O(n log n) due to BTreeSet re-sorting after each removal.

### Get Sorted by Fee Density (block building — top 300 txs)

| Pool Size | Time |
|-----------|------|
| 1,000 | 42.0 µs |
| 5,000 | 45.5 µs |
| 10,000 | 46.5 µs |

Near-constant time — BTreeSet is pre-sorted by fee density.

### Drain + Re-add (rollback simulation)

| Transactions | Time | Per-tx |
|-------------|------|--------|
| 1,000 | 373 µs | 373 ns |
| 5,000 | 2.07 ms | 414 ns |
| 10,000 | 5.81 ms | 581 ns |

## Key Takeaways

1. **Block validation is not crypto-bound.** Even a worst-case 500-witness block
   takes only 14.3ms for signature verification — well within the 3s slot budget.

2. **Hashing scales linearly.** Blake2b shows no degradation at batch sizes up to
   500, confirming it won't bottleneck block processing.

3. **Mempool is fast for block building.** Getting the top 300 txs by fee density
   takes ~45 µs regardless of pool size, enabling instant block construction.

4. **CBOR encoding is sub-microsecond for headers.** Full transaction encoding at
   2 µs means encoding 300 txs for a block takes ~600 µs.

5. **Mempool remove is the slowest operation** at 10 µs/tx with 10K pool. For a
   block confirming 300 txs from a full mempool: ~3 ms. Acceptable.
