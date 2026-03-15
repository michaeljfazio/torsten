# Node Validator Agent Memory

## Key Files
- Node binary: `./target/release/torsten-node`
- CLI binary: `./target/release/torsten-cli`
- Config dir: `./config/` (preview-config.json, preview-topology.json)
- Preview DB: `./db-preview/` — slot ~106.809M, block ~4.1059M, epoch 1236
- Ledger snapshot: `<db>/ledger-snapshot.bin` (~1.1 GB with UTxOs when using in-memory backend)
- Node logs: `/tmp/torsten-validation-run2.log`

## Startup Command Pattern
```
TORSTEN_PIPELINE_DEPTH=150 ./target/release/torsten-node run \
  --config config/preview-config.json \
  --topology config/preview-topology.json \
  --database-path ./db-preview \
  --socket-path ./node.sock \
  --host-addr 0.0.0.0 --port 3001 \
  --metrics-port 12799 \
  --storage-profile minimal \
  --utxo-backend in-memory \
  > /tmp/torsten-validation-run.log 2>&1 &
```
NOTE: Always `pkill -f torsten-node && rm -f ./node.sock` before restart.
NOTE: Use `--utxo-backend in-memory` to avoid LSM OOM crash during full replay on macOS.
NOTE: Use `--metrics-port 12799` to avoid conflict with Haskell cardano-node on 12798.
NOTE: `--storage-profile minimal` + `--utxo-backend in-memory` is required for macOS (OOM otherwise).

## Preview Testnet Baselines (2026-03-14, run #7 — main 982bbf8)
- Fresh Mithril import: epoch=1236, immutable=24719, 2.7 GB archive, ~6 min total
- Build: incremental 39.78s (multiple crates rebuilt: primitives, serialization, ledger, etc.)
- New commits since run #6: 982bbf8 (137 new tests), 7604365 (metrics before replay),
  1cabd20 (fix O(n) address index removal), 183e06e (production readiness: crash recovery, observability)
- Ledger replay (in-memory UTxO, minimal profile): 4,105,234 blocks in 116s = 35,164 blk/s peak
  - Speed profile: ~25K blk/s early, slows to ~8K at slot 12M (UTxO growth), recovers to ~35K
  - UTxOs at replay end: 2,938,400 (vs 0 in run #6 which used LSM backend)
  - Snapshot saved automatically at epoch transition: epoch=1235, 2938400 UTxOs, 1113.3 MB
- N2N peer connected: 3.134.226.73:3001 rtt_ms=570, 5 hot peers total
- Epoch transition during tip catch-up: epoch=1236, ChainDB persisted
- Caught up in ~35s after network connect: 617 blocks applied
- At-tip block reception: 622 blocks total, 2 blocks FAILED to apply (validation bugs)
- Rollbacks observed: 0
- SIGTERM shutdown: 623 volatile blocks flushed, 1,112 MB snapshot saved, Shutdown complete

## CRITICAL BUG #1: ScriptDataHash Ignores Reference Scripts (run #7, OPEN)
- Tx hash: `370f8772f8cc63598f5ffd5355704af6633df4123cb84514ec8bbfe6c06c26bb`
- Block: 4105851 / slot 106808990 — confirmed VALID on chain by Koios (num_confirmations>0)
- Error: `ScriptDataHashMismatch { expected: "7482...", actual: "dfe1..." }`
- Root cause: `compute_script_data_hash` in `validation.rs:785-787` computes `has_v1/v2/v3`
  from witness-embedded scripts only — ignores reference scripts in `body.reference_inputs`
- When a tx uses ONLY reference scripts (no embedded scripts in witness_set), all has_vN = false
  → language views = empty map → computed hash is wrong
- Fix: look up reference inputs in utxo_set to detect their script versions; include those in has_vN
- File: `crates/torsten-ledger/src/validation.rs` lines 785-795
- Ledger spec: language views must include cost models for ALL script languages used, including via reference

## CRITICAL BUG #2: CollateralHasTokens Incorrect for Transactions with CollateralReturn (run #7, OPEN)
- Tx hash: `95cdd9d9489916be8bc6cd8aa86b34a7a4651bf673f599a4195fd1ddbd1678b4`
- Block: 4105853 / slot 106809022 — confirmed VALID on chain by Koios (num_confirmations>0)
- Error: `CollateralHasTokens("eb499984...#1")`
- Root cause: `validation.rs:654-657` rejects any collateral input with multi-asset tokens
- Actual Cardano rule: collateral inputs CAN have tokens as long as a `collateral_return` output
  returns the tokens back; only the NET collateral (collateral_total - collateral_return) must be pure ADA
- Fix: only raise `CollateralHasTokens` if there is no `collateral_return` that accounts for the tokens
  OR if the net balance has non-ADA assets
- File: `crates/torsten-ledger/src/validation.rs` lines 650-687
- Note: both bugs together caused 2 blocks to be skipped at tip, diverging from the canonical chain

## FIXED (run #7): UTxO-HD LSM Store Not Persisted (2026-03-14)
- Using `--utxo-backend in-memory` workaround fully resolves the issue
- With in-memory backend: utxo_count=2,938,477 in metrics (vs 0 with LSM backend)
- Snapshot is 1.1 GB (vs ~65 MB with LSM backend)
- The underlying LSM persistence bug is still present for the LSM backend specifically

## Prometheus Metrics Port
- Use `--metrics-port 12799` to avoid conflict with Haskell cardano-node on 12798
- `http://localhost:12799/metrics` — works fine
- `http://localhost:12799/health` — returns JSON with syncProgress, last_block_received_at
- `http://localhost:12799/ready` — returns `{"ready":true}`
- NEW in run #7: metrics server starts BEFORE ledger replay (commit 7604365) — confirmed working

## Known Issues (Current — 2026-03-14, run #7)
1. **ScriptDataHash ignores reference scripts** — CRITICAL, causes block application failure
   - File: `crates/torsten-ledger/src/validation.rs:785-795`
2. **CollateralHasTokens fires for collateral with collateral_return** — CRITICAL, same effect
   - File: `crates/torsten-ledger/src/validation.rs:650-687`
3. **transactions_rejected metric stays 0 even when txs rejected** — INFO, metrics bug
   - Block-level application failures not wired to `transactions_rejected` counter
   - File: `crates/torsten-node/src/node.rs` (apply_block failure path)
4. **LSM UTxO backend loses data on restart** — use `--utxo-backend in-memory` as workaround
5. **macOS OOM during replay with high-memory/LSM profile** — use `minimal` + `in-memory`

## Working Features Confirmed (2026-03-14, run #7, main branch 982bbf8)
- Build: WORKS — incremental compile 39.78s
- Fresh Mithril import: WORKS — 2.7 GB, epoch=1236
- Ledger replay (in-memory): WORKS — 35K blk/s, 4.1M blocks in 116s
- Snapshot save during replay (epoch transition): WORKS — 1113 MB at epoch 1235
- Peer connections: WORKS — 3.134.226.73:3001, 5 hot peers
- Catch-up to tip: WORKS — 617 blocks in ~35s
- Empty-block processing at tip: WORKS
- N2C query tip: WORKS — slot=106808997, block=4105852, epoch=1236, era=Conway, syncProgress=100.00
- N2C protocol-parameters: WORKS — full Conway PParams with PlutusV1+V3 cost models
- N2C treasury: WORKS — Treasury=9142820104 ADA, Reserves=2615692514 ADA
- N2C stake-distribution: WORKS — 657 pools
- N2C gov-state: WORKS — committee_members=7, active_proposals=2
- N2C constitution: WORKS — empty URL, zero hash, no guardrail
- Prometheus metrics: WORKS — all gauges, counters, histograms populated
  - blocks_applied=622, utxo_count=2938477, peers_connected=5, epoch=1236
  - treasury=9142820104206891 lovelace, drep_count=8791, proposal_count=2
- Health endpoint: WORKS — JSON with syncProgress, last_block_received_at
- Ready endpoint: WORKS — {"ready":true}
- SIGTERM shutdown: WORKS — 623 volatile blocks flushed, 1112 MB snapshot, Shutdown complete

## Operational Notes
- Always `--testnet-magic 2` with CLI query tip for correct syncProgress
- CLI subcommand is `query treasury` (not `query account-state`)
- gov-state shows committee_members=7 — governance active on preview
- After clean shutdown with in-memory backend: snapshot is ~1.1 GB (full UTxO state)
- Replay speed slows at slot ~12M due to UTxO growth to ~1.5M entries; recovers after slot ~15M
- First OOM crash (run #7 attempt 1): high-memory + LSM backend during replay at block ~1.85M
