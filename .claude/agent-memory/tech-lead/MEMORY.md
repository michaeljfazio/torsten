# Tech Lead Agent Memory

## Critical Invariants & Bug Patterns
- [Fork snapshot stall cascade](fork-snapshot-stall-fix.md) — 6-bug cascade: fork snapshot → bad intersection → deep rollback → UTxO corruption; all fixed (1ff9cbce)
- [Cascade failure invariant](ledger-cascade-failure-invariant.md) — Never hard-return on confirmed blocks; log+self-correct for ledger-state-divergence checks
- [Forge body size bug](forge-body-size-bug.md) — body_size miscalculation + epoch nonce not updated + KES expiry off-by-one
- [RUPD snapshot position fix](ledger-rupd-snapshot-fix.md) — Use `set` snapshot (not `go`) in calculate_rewards(); stale treasury diagnostics
- [Rollback UTxO store](ledger-rollback-utxo-store.md) — Slow-path rollback must open fresh store from LSM snapshot
- [Output CBOR re-encode](crypto-output-cbor-reencode.md) — Indefinite-length inline datum CBOR and legacy vs post-Alonzo detection
- [Deferred pointer stake (sisPtrStake)](ledger-ptr-stake-deferred.md) — ptr_stake field + StakeRouting enum; resolves at SNAP time not insertion; 603 epoch mismatches from epoch 647

## N2C Protocol Compliance
- [Hash32 padding convention](n2c-hash32-padding.md) — 28→32 byte padding/truncation rules for N2C wire output
- [Credential type discrimination](n2c-credential-type-discrimination.md) — Track KeyHash vs Script via HashSets; DRep stores full Credential
- [Committee state encoding bugs](n2c-committee-state-bugs.md) — Open issues: wrong source map, hardcoded hot credential type

## N2N Protocol
- [ChainSync server direction bug](network-chainsync-direction-bug.md) — InitiatorAndResponder role confusion; TxSubmission2 deadlock (server sends MsgRequestTxIds first)
- [Duplex connection architecture](network-duplex-connection.md) — Phase 1+2 implementation; pallas plexer semantics; Phase 3 pending
- [Duplex Phase 3 integration](node-duplex-phase3.md) — into_pipelined() conversion; TxSubmission2 responder JoinHandle

## Consensus
- [LoE enforcement](consensus-loe-enforcement.md) — flush_to_immutable_loe() gating in block pipeline; GSM integration
- [Forge pipeline depth](consensus-forge-pipeline-depth.md) — Forge disabled during sync (pipeline_depth > 1); metric interpretation
- [Preview pool expected rates](consensus-preview-pool-rates.md) — SAND pool: ~0.155 blocks/hour, 1-block expected after 6.5+ hours at tip

## Ledger
- [Reward formula validation](ledger-reward-formula-validation.md) — Koios cross-validation methodology; 1-epoch RUPD timing difference vs Haskell
- [Blueprint divergences](ledger-blueprint-divergences.md) — Ref script fee ceiling/floor, totalRefScriptSize check, chain selection tiebreaker
- [DRep count fix](ledger-drep-count-fix.md) — Use active_drep_count() not dreps.len()
- [Plutus test coverage](ledger-plutus-test-coverage.md) — is_valid=false UTxO, treasury Phase-1, per-redeemer V3 Unit tests
- [Mempool epoch revalidation](node-mempool-epoch-revalidation.md) — Revalidate mempool with new protocol params after epoch transition

## CLI
- [Build-raw alias](cli-build-raw-alias.md) — transaction build-raw as alias for transaction build
- [UTxO --tx-in query](cli-utxo-txin-query.md) — GetUTxOByTxIn (tag 15) wire format
- [Stake address info](cli-stake-address-info.md) — Server-side filtering via tag 10
- [P1 commands](cli-p1-commands.md) — calculate-min-fee, calculate-min-required-utxo, policyid, pool-params, slot-number, kes-period-info

## TUI
- [Layout polish](tui-layout-polish.md) — Wide mode, kv_aligned patterns, Monokai theme, RTT bar

## Storage
- [LSM perf baselines](storage-lsm-perf-baselines.md) — Mainnet-scale test runtimes on M-series (1M insert ~25s, total ~27.5s)
- [Large tests feature](storage-large-tests-feature.md) — Feature flag design, key/value sizing, deterministic PRNG

## Serialization
- [Serialization test coverage](crypto-serialization-tests.md) — 133 tests, public API patterns, PPU extraction for integration tests

## Soak Test Findings (2026-03-27)
- [ChainSync log flood stall](soak-test-chainsync-log-flood.md) — Inbound Haskell syncer floods 1.2M INFO logs/10min → I/O stall; fix: downgrade to DEBUG
- [CLI tx build change output](cli-tx-build-change-output.md) — --change-address computes change but doesn't append change output to tx body
