---
name: project_state_2026_03_15
description: State assessment as of 2026-03-15, after torsten-lsm milestone and before incremental compliance cycle
type: project
---

# Torsten State Assessment — 2026-03-15

**Why:** Post-torsten-lsm milestone assessment, establishing the baseline for the incremental compliance development cycle targeting preview testnet full protocol compliance.

**How to apply:** Use when asked about current state, gap analysis, or what to work on next.

## Major Milestone Completed

### torsten-lsm (NEW, 2026-03-15)
- Replaced `cardano-lsm` (external git dependency) with `crates/torsten-lsm` — a pure Rust LSM-tree engine written in-house
- Features: WAL + crash recovery, lazy levelling compaction (T=4), blocked bloom filters, LRU block cache, fence pointer indexes, persistent snapshots via hard links, CRC32 checksums, exclusive session lock
- `UtxoStore` in `torsten-ledger` now delegates to `torsten-lsm` via `LsmTree`
- 84 LSM tests pass; full workspace 1,236+ tests pass (one flaky log-cleanup timing test, non-critical)

## Test Suite Health (2026-03-15)
- Test count: ~1,236 tests across all crates (excluding conformance/golden)
- One flaky test: `logging::tests::test_cleanup_old_logs_removes_expired` in torsten-node — timing-sensitive file mtime test, passes when run in isolation, fails intermittently in parallel test runs (low priority)
- Conformance tests: 174 vectors (UTXO, CERT, GOV, EPOCH) — all pass
- Golden tests: VRF nonintegral + N2C golden CBOR — all pass

## Key Gaps Remaining for Preview Testnet Compliance

### 1. torsten-lsm Production Hardening (HIGH)
The new LSM engine is ~4,500 lines with good unit tests but has NOT been stress-tested with a real UTxO-HD workload (20M+ entries, compaction under load, snapshot during replay). Key unknowns:
- Compaction correctness under concurrent flush + compact
- Snapshot atomicity: uses hard links which work on the same filesystem but need validation
- WAL replay correctness after ungraceful shutdown with partially-flushed memtable
- Range scan correctness across memtable + multiple SSTable levels
- No benchmark against cardano-lsm baseline (ImmutableDB: ~10,600 blocks/s target)

### 2. Reward Calculation Cross-Validation (HIGH)
- Zero end-to-end tests validate maxPool' output against historical Koios data
- Same failure mode as the Alonzo VRF nonce_vrf bug — could be silent and wrong
- Koios MCP is available and has epoch_info, pool_history endpoints

### 3. Byron Ledger Validation (MEDIUM-HIGH)
- ByronLedger struct is a stub — no transaction validation
- Only affects genesis-from-Byron sync (Mithril users start post-Byron)
- Preview testnet starts from Shelley, so this is lower urgency for preview

### 4. Ouroboros Genesis LoE Enforcement (MEDIUM)
- LoE computed but not wired into block pipeline
- Only matters for --consensus-mode genesis (not default)

## Risk for Preview Testnet Run
- **torsten-lsm under real load**: the biggest unknown — has never seen a 4M-block replay with 20M UTxO entries
- **Flaky log cleanup test**: minor, but indicates test isolation issues
- **WAL recovery path**: not tested with real crash scenarios
