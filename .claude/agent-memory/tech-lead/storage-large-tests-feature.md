---
name: Large-test feature gate design
description: How the large-tests feature flag and entry counts were chosen for torsten-lsm
type: project
---

Feature flag: `large-tests = []` in `crates/torsten-lsm/Cargo.toml`.
Guard: `#[cfg(all(test, feature = "large-tests"))]` on the module.
Run command: `cargo test -p torsten-lsm --features large-tests -- mainnet_scale`

Entry counts (chosen to keep total runtime <60s on debug builds):
- test_mainnet_scale_insert_read: 1M entries, 1000-key random sample
- test_mainnet_scale_delete_amplification: 500K insert / 400K delete / 100K survive
- test_mainnet_scale_wal_crash_recovery: 100K entries (WAL-only, no auto-flush)

Key design decisions:
- Keys are 36 bytes (32-byte tx hash + 4-byte index BE), matching Cardano TransactionInput
- Values are 200 bytes, matching bincode-serialized TransactionOutput with inline datum
- Deterministic PRNG (xorshift64, seed 0xdeadbeef_cafebabe) for reproducible sampling
- Delete amplification test verifies range scan returns EXACTLY the surviving count
  (catches tombstone resurrection and inflated counts from incomplete compaction)
- WAL recovery test uses 512 MB memtable to guarantee zero auto-flushes before crash

**Why:** Per Tranche 2, Task T2-1 requirements. Normal stress tests cap at 50K.
**How to apply:** Do not reduce entry counts without re-measuring runtime on CI.
