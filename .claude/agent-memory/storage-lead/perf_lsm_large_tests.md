---
name: LSM large-scale test runtimes
description: Measured wall-clock runtimes for mainnet_scale_tests on debug builds (M-series Mac)
type: project
---

Measured on darwin/M-series (debug build, cargo test):

| Test                                     | Entry count | Runtime |
|------------------------------------------|-------------|---------|
| test_mainnet_scale_insert_read           | 1M inserts  | ~25s    |
| test_mainnet_scale_delete_amplification  | 500K ins / 400K del | ~20s |
| test_mainnet_scale_wal_crash_recovery    | 100K inserts (WAL only) | ~5s |
| Total (3 tests, run in parallel by cargo) | —          | 27.5s   |

Config used: `memtable_size: 16 MB, page_size: 65536, size_ratio: 4, wal_enabled: true`
WAL recovery test uses `memtable_size: 512 MB` to suppress auto-flush.

**Why:** Target is <60s total; currently 27.5s leaves margin for slower CI runners.

**How to apply:** If tests exceed 60s on CI, first suspect the delete amplification
test (range scan over 500K entries is the bottleneck). Consider dropping it to
200K/160K/40K if needed.
