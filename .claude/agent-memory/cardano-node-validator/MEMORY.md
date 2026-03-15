# Node Validator Agent Memory

## Key Files
- Node binary: `./target/release/torsten-node`
- CLI binary: `./target/release/torsten-cli`
- Config dir: `./config/` (preview-config.json, preview-topology.json)
- Preview DB: `./db-preview/` — slot ~106.809M, block ~4.1059M, epoch 1236 (in-memory backend)
- Preview DB (LSM): `./db-preview-lsm-test/` — slot ~106.915M, block ~4.1092M, epoch 1237
- Ledger snapshot: `<db>/ledger-snapshot.bin` (~80 MB with LSM backend, ~1.1 GB with in-memory)
- Node logs: `/tmp/torsten-validation-lsm-retest.log` (run #9 — LSM PageOverflow fix validation)

## Startup Command Pattern (LSM backend, default)
```
TORSTEN_PIPELINE_DEPTH=150 ./target/release/torsten-node run \
  --config config/preview-config.json \
  --topology config/preview-topology.json \
  --database-path ./db-preview-lsm-test \
  --socket-path ./node-lsm-test.sock \
  --host-addr 0.0.0.0 --port 3002 \
  --metrics-port 12798 \
  > /tmp/torsten-validation-lsm-retest.log 2>&1 &
```
NOTE: Always `pkill -f torsten-node && rm -f ./node-lsm-test.sock` before restart.
NOTE: LSM backend now works on macOS with page_size=65536 (PageOverflow bug FIXED in run #9).
NOTE: `--utxo-backend in-memory` is NO LONGER required — LSM backend is now production-worthy.
NOTE: Default metrics port is 12798. Use `--metrics-port 12799` only if Haskell node is also running.
NOTE: No `--storage-profile` or `--utxo-backend` flags needed — defaults work correctly.

## Preview Testnet Baselines (2026-03-15, run #10 — deferred RUPD validation)
- Fresh Mithril import: epoch=1237, immutable=24745, 2.7 GB archive, ~9 min total
  (download 3.6min, extract 22s, verify 52s, copy 12s, cleanup 5s)
- Ledger replay (LSM backend, 65KB pages): 4,108,827 blocks in 299s = 13,728 blk/s
  - Speed profile: ~22K blk/s early (slot 0-2M), slows to ~7K at slot 12M (UTxO growth), recovers to ~14K
  - UTxOs at replay end: 2,938,835 (correct)
  - Snapshot saved: epoch=1237, 2938835 UTxOs, 80.0 MB
- No PageOverflow errors anywhere in log (0 occurrences)
- No block application failures during replay (1 skipped — likely same old ScriptDataHash/CollateralToken bug)
- N2N peer connected: 3.70.89.92:3001 rtt_ms=747, 5 hot peers total
- Caught up to tip in ~21s after network connect: 501 blocks applied
- At-tip block reception: 504 total blocks applied, 2 rejected (Plutus budget bug — see Known Issues #3)
- Rollbacks observed: 0
- Memory at tip: 5.79 GB RSS (6.07 GB per metrics)
- Governance proposals ratified during replay: 590

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

## FIXED BUG #3: torsten-lsm PageOverflow (FIXED in run #9)
- Previous issue (run #8): entries >4088 bytes were silently dropped, UTxO count froze at 693K
- Fix applied: page_size expanded from 4096 → 65536 (64KB), jumbo page allocation for oversized entries,
  u16 header fields expanded to u32, UtxoStore now panics on LSM error instead of silently dropping
- Validation result: 0 PageOverflow errors, 2,938,963 UTxOs (correct), LSM backend production-worthy

## Prometheus Metrics Port
- Default port is 12798. Use `--metrics-port 12799` only if Haskell cardano-node is also running.
- `http://localhost:12798/metrics` — works fine
- `http://localhost:12798/health` — returns JSON with syncProgress, last_block_received_at
- `http://localhost:12798/ready` — returns `{"ready":true}`
- Metrics server starts BEFORE ledger replay — confirmed working

## Known Issues (Current — 2026-03-15, run #12 — Byron EBB fix validation on mainnet)
1. **ScriptDataHash ignores reference scripts** — CRITICAL (run #7, status unknown after recent changes)
   - File: `crates/torsten-ledger/src/validation.rs:785-795`
2. **CollateralHasTokens incorrect for txs with collateral_return** — CRITICAL (run #7, status unknown)
   - File: `crates/torsten-ledger/src/validation.rs:650-687`
3. **Plutus ExBudget mismatch: uplc evaluates scripts over declared budget** — CRITICAL (run #10, OPEN)
   - Tx hash: `3c6d9cb477106657cf4d47dfd74f2aa2e0f5f7f73c23b2f4fa00a792f747a8be` (block 4109328)
   - Error: `ScriptFailed("Plutus evaluation failed: ... Spend[0] execution went over budget CPU -28413")`
   - File: `crates/torsten-ledger/src/plutus.rs` — eval_phase_two_raw call at line 132
4. **FIXED: Stale peer detection incorrect wall-clock slot for mainnet** (run #11, FIXED)
   - Fix: Updated `current_wall_clock_slot()` to account for Byron era before computing Shelley slots
   - File: `crates/torsten-node/src/node.rs` function `current_wall_clock_slot()`
5. **FIXED: Byron Epoch Boundary Block (EBB) causes chain sync stall on mainnet** (run #11→12, FIXED in commit 56f063b)
   - Validation: run #12 (2026-03-15) confirmed fix works — 5 Byron epoch boundaries crossed cleanly
   - Epochs 0→1 (slot 21600), 1→2 (slot 43200), 2→3 (slot 64800), 3→4 (slot 86400), 4→5 (slot 108000)
   - Zero "Block does not connect" errors, zero panics
   - "Block count mismatch: expected=35 got=36" warning is EXPECTED/BENIGN — EBB counted in range,
     node falls back to individual block fetches correctly
   - Files fixed: `crates/torsten-serialization/src/multi_era.rs`, `crates/torsten-ledger/src/state/mod.rs`

## Mainnet Sync Baselines (run #12 — 2026-03-15, Byron EBB fix validation)
- Start: fresh DB, from genesis
- Duration: ~22 minutes monitored (15:03 - 15:25)
- Blocks applied: 115,493 (slot 115,524, epoch 5) in ~20 minutes of actual sync
- Throughput: ~95-115 blk/s sustained (mainnet Byron era, small blocks)
- Peers: 5 hot (backbone peers, all high-latency: 500ms - 40s handshake RTT)
- Peer handshake delays: IOG backbone 36s, CF backbone 40s, others 500ms-30s
- Memory: 127 MB RSS at epoch 5 (very low — Byron has no Plutus scripts, tiny UTxO growth)
- UTxO count: 27,330 at epoch 5 (growing from 14,505 genesis)
- Treasury at epoch 5: 41,656,759,300,310 lovelace (reward accumulation working)
- Zero rollbacks, zero transaction rejections, zero errors
- First peer connected: 51.161.86.220:3001 (WARN: 29s for raw TCP connect)
  - This is NOT a "Peer connected" in the proper sense; handshake didn't complete until later
  - Actual first hot peer: 135.148.7.9:3001 at 15:03:51 (36s into run)
  - Sync only started after 4 fetchers connected (15:05:03, ~2 min after startup)
- NOTE: mainnet bootstrap peers have HIGH latency, expect 2+ minutes before sync starts

## Mainnet Config Facts (run #11 — 2026-03-15)
- Config files: `config/mainnet-config.json`, `config/mainnet-topology.json`, `config/mainnet-*-genesis.json`
- Network magic: 764824073, Byron k=2160, epoch_length=21600, slot_duration=20s, UTxOs=14505
- Shelley genesis system_start: 2017-09-23T21:44:51Z (Byron genesis start, NOT Shelley start)
- Shelley first slot: 4,492,800 (= 208 Byron epochs × 21,600 Byron slots)
- Current mainnet tip: ~slot 182,015,000, ~block 13,162,400, epoch ~618
- Wall clock slot formula: byron_slots + (now - shelley_start) / 1s
  = 208*21600 + (now - 2020-07-29T21:44:51Z) = ~182M (matches actual mainnet tip)
- Mithril snapshot: epoch=618, immutable=8423, 52.8 GB, download rate ~900 MB/min (1 hour total)
- Mainnet bootstrap peers: backbone.cardano.iog.io:3001, backbone.mainnet.cardanofoundation.org:3001
- First Byron epoch boundary is at slot 21600 (epoch 0→1 transition), block_no ~21587

## Working Features Confirmed (2026-03-15, run #10 — deferred RUPD, LSM backend)
- Build: WORKS
- Fresh Mithril import: WORKS — epoch=1237, immutable=24745, 2.7 GB archive, ~9 min total
- Ledger replay (LSM backend): WORKS — 13,728 blk/s, 4,108,827 blocks in 299s, 1 skipped
- No PageOverflow errors: CONFIRMED — 0 occurrences
- UTxO count correct: CONFIRMED — 2,938,835 at snapshot, 2,939,027 at tip
- Snapshot save at replay end: WORKS — epoch=1237, 80.0 MB
- Peer connections: WORKS — 3.70.89.92:3001 rtt_ms=747, 5 hot peers
- Catch-up to tip: WORKS — 501 blocks in ~21s after peer connect
- Live block reception at tip: WORKS — 504 total blocks applied, 2 rejected (Plutus budget bug)
- N2C query tip: WORKS — slot=106919624, block=4109330, epoch=1237, era=Conway, syncProgress=100.00
- N2C protocol-parameters: WORKS — full Conway PParams with PlutusV1+V2+V3 cost models
- N2C treasury: WORKS — Treasury=14065825286 ADA, Reserves=871897504 ADA
- N2C stake-distribution: WORKS — 656 pools registered, multiple with stake fractions
- N2C gov-state: WORKS — committee_members=1, active_proposals=2
- Prometheus metrics: WORKS — utxo_count=2939027, blocks_applied=504, peers_connected=5, epoch=1237
  treasury=14065825286441725 lovelace, drep_count=8791, proposal_count=2, sync_progress=10000 (100%)
- Epoch transitions (RUPD deferred): WORKS — 1237 epochs traversed without panic
  (Protocol version changes at epochs 3, 22, 646 logged correctly)
- Governance ratifications during replay: 590 proposals ratified, no panics
- Memory at tip: 5.79 GB RSS (stable, no growth observed after sync complete)

## Operational Notes
- Always `--testnet-magic 2` with CLI query tip for correct syncProgress
- CLI subcommand is `query treasury` (not `query account-state`)
- gov-state shows committee_members=1 on epoch 1237 (was 7 on epoch 1236 — committee changed)
- LSM backend snapshot is much smaller (~80 MB) than in-memory snapshot (~1.1 GB)
- Replay speed: LSM is slower than in-memory (13.8K vs 35K blk/s) — LSM does real disk I/O
- Replay speed slows at slot ~12M due to UTxO growth to ~1.5M entries; recovers after slot ~15M
- Governance "prev_action_id chain mismatch" INFO messages are normal during epoch transitions
