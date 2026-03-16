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

## Known Issues (Current — 2026-03-16, run #14 — uplc budget swap fix + new script result bug)
1. **ScriptDataHash ignores reference scripts** — CRITICAL (run #7, status unknown after recent changes)
   - File: `crates/torsten-ledger/src/validation/` (formerly validation.rs:785-795)
2. **CollateralHasTokens incorrect for txs with collateral_return** — CRITICAL (run #7, status unknown)
   - File: `crates/torsten-ledger/src/validation/collateral.rs`
3. **FIXED: Plutus budget (mem, steps) swap — scripts got 14M CPU instead of 10B** (run #14, FIXED)
   - Root cause: `validation/mod.rs:264` passed `(mem, steps)` instead of `(steps, mem)` to eval_phase_two_raw
   - uplc::tx::eval_phase_two_raw signature: `initial_budget: (cpu, mem)` = `(steps, mem)` in our terminology
   - Fix: Changed to `(params.max_tx_ex_units.steps, params.max_tx_ex_units.mem)` at both call sites:
     - `crates/torsten-ledger/src/validation/mod.rs:264`
     - `crates/torsten-ledger/src/state/apply.rs:292-295`
   - Also confirmed: Cargo.toml already points to `michaeljfazio/aiken.git` branch `torsten-budget-tolerance`
     (commit e77322cd) which adds 2% CPU tolerance — that tolerance is orthogonal to this fix
   - The aiken commit 6806d52 in `aiken-lang/aiken.git` is irrelevant — it only fixes clippy unused fields;
     the actual bigint fix is in parent b1b92f5d which fixes `Data::integer()` for large negative integers;
     neither of these caused or fixes the budget divergence
4. **Plutus script returns Data instead of Unit/Bool — "Unexpected result"** (run #14, OPEN)
   - Script hash: `4faf61d99fe87d6f1c4ae346f804a6b9824808a04047bd846fb1ea5f` (7276 bytes, PlutusV2)
   - Tx hashes affected: `9d581223...`, `e319798023...`, `1040d648...` (preview testnet, epoch 1238)
   - Error: `Unexpected result: Constant(Data(Array(Indef([BigInt(0), BigInt(0), BigInt(0), BigInt(0), BigInt(0)]))))`
   - The script ran to completion but returned its datum `[0,0,0,0,0]` as a raw Data value
   - Correct behavior: script should return `()` (unit) or `True` to indicate success
   - Likely root cause: script context construction error — uplc receives the wrong argument ordering
     (datum [0,0,0,0,0] is being returned as the result of the script application, not consumed by the script)
   - For PlutusV2 Spend scripts: uplc applies datum → redeemer → scriptContext as positional args
   - File to investigate: `crates/torsten-ledger/src/plutus.rs` — `eval_phase_two_raw` call
   - Note: the node correctly treats this as a divergence warning and trusts on-chain consensus
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

## Mainnet Full Sync Baselines (run #13 — 2026-03-15, Mithril import + replay to tip)
- Mithril import: epoch=618, immutable=8423, 52.8 GB, download ~72 min, extract 3.5 min, verify ~10 min
- DB size after import: 211 GB (25,272 chunk files)
- Ledger replay: 13,159,707 blocks in 7083s = 1,857 blk/s, 2 skipped (known ScriptDataHash bug)
  - Speed profile: starts ~127K blk/s (Byron), peaks at 136K, falls to ~1.8K in Conway
  - Speed slows substantially as UTxO set grows: 127K blk/s at start → 1.8K blk/s in Conway era
  - UTxOs grow from 14,505 (genesis) to 11,164,412 at epoch 618
  - Snapshots saved at epochs: 289 (3.19M UTxOs), 308 (4.56M), 319 (5.52M), 362 (8.49M), 411 (9.89M),
    426 (10.28M), 459 (10.77M), 516 (11.22M), 617 (11.15M), 618 (11.16M)
- ERA TRANSITIONS (ALL CLEAN, ZERO ERRORS):
  - Shelley: epoch 208 (slot ~4.4M), Byron→Shelley, EBBs handled correctly
  - Allegra: epoch 236, protocol v2.0→3.0
  - Mary: epoch 251, protocol v3.0→4.0
  - Alonzo: epoch 290, protocol v4.0→5.0
  - Alonzo v2: epoch 298, protocol v5.0→6.0
  - Babbage: epoch 365, protocol v6.0→7.0
  - Conway: epoch 394, protocol v7.0→8.0
- After replay: 4,026 blocks synced from network (slot 181,958,347 → 182,039,011), caught up in ~2 min
- Memory at tip: ~19.7 GB RSS (high: 11M UTxOs in LSM + 1024MB memtable + 12GB cache)
- Peers: 5 connected (first peer at 2989ms RTT)
- At-tip errors (3 blocks skipped, diverging from canonical chain):
  1. Block 13163558/slot 182038996: ScriptDataHashMismatch (tx 21636bfd...)
  2. Block 13163561/slot 182039011: FeeTooSmall (min=168581, actual=168537, tx 9816fcc8...)
  3. Multiple Plutus over-budget WARNs (handled gracefully — trusting on-chain, not skipping)
- NOTE: syncProgress in CLI query tip shows 68.08% (wall-clock-based) not 100% — this is normal

## NEW BUG #4: FeeTooSmall false positive (run #13, OPEN)
- Tx hash: `9816fcc8efdd80f350a2cca600a268a0e65c2df1b28022f07b99c382112c0fe2`
- Block: 13163561 / slot 182039011 — confirmed VALID on chain (node accepted it)
- Error: `FeeTooSmall { minimum: 168581, actual: 168537 }`
- The difference is only 44 lovelace (= 1 byte of txFeePerByte). Likely a reference script fee
  calculation rounding error — our fee computation doesn't exactly match Haskell's.
- File: `crates/torsten-ledger/src/validation.rs` — min_fee calculation

## Mainnet Sync Baselines (run #12 — 2026-03-15, Byron EBB fix validation)
- Start: fresh DB, from genesis
- Duration: ~22 minutes monitored (15:03 - 15:25)
- Blocks applied: 115,493 (slot 115,524, epoch 5) in ~20 minutes of actual sync
- Throughput: ~95-115 blk/s sustained (mainnet Byron era, small blocks)
- Memory: 127 MB RSS at epoch 5 (very low — Byron has no Plutus scripts, tiny UTxO growth)
- Zero rollbacks, zero errors (Byron only)
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

## Working Features Confirmed (2026-03-15, run #13 — Full mainnet Mithril sync to tip)
- Build: WORKS
- Mainnet Mithril import (52.8 GB): WORKS — epoch=618, immutable=8423, 211 GB on disk
- Mainnet ledger replay (13.16M blocks): WORKS — all 7 era transitions clean, zero panics
- All era transitions CLEAN (MAINNET CONFIRMED): Byron→Shelley(208)→Allegra(236)→Mary(251)
  →Alonzo(290+298)→Babbage(365)→Conway(394) — all 7 transitions at protocol version boundaries
- Byron EBB handling: CONFIRMED WORKING across all Byron epoch boundaries in full replay
- UTxO growth to 11.16M entries: WORKS (LSM backend handles correctly)
- Snapshot saves at each epoch: WORKS — snapshots at 10 checkpoints during mainnet replay
- Peer connections (mainnet): WORKS — 5 peers, first at 2989ms RTT
- Catch-up to tip: WORKS — 4,026 blocks applied after replay, caught up in ~2 min
- N2C query tip (mainnet): WORKS — slot=182038979, block=13163557, epoch=618, era=Conway
- N2C protocol-parameters (mainnet): WORKS — full Conway PParams with PlutusV1+V2+V3 cost models
- N2C treasury (mainnet): WORKS — Treasury=9048603530 ADA, Reserves=1954715190 ADA
- N2C stake-distribution (mainnet): WORKS — 2,949 pools returned
- N2C gov-state (mainnet): WORKS — committee_members=6, active_proposals=5, constitution visible
- Prometheus metrics (mainnet): WORKS — utxo_count=11164054, delegation_count=1687711,
  treasury=9048603530883971 lovelace, peers_connected=5, sync_progress=10000 (100%)
- Memory at tip: ~19.7 GB RSS (11M UTxOs + 1GB memtable + 12GB LSM cache)

## Preview Testnet Baselines (2026-03-15, run #10 — deferred RUPD validation)
- Fresh Mithril import: epoch=1237, immutable=24745, 2.7 GB archive, ~9 min total
- Ledger replay: 4,108,827 blocks in 299s = 13,728 blk/s, 1 skipped
- N2C treasury: Treasury=14065825286 ADA, Reserves=871897504 ADA
- Prometheus: utxo_count=2939027, blocks_applied=504, peers_connected=5, epoch=1237
- Memory at tip: 5.79 GB RSS (stable)

## Operational Notes
- Always `--testnet-magic 2` with CLI query tip for correct syncProgress
- CLI subcommand is `query treasury` (not `query account-state`)
- gov-state shows committee_members=1 on epoch 1237 (was 7 on epoch 1236 — committee changed)
- LSM backend snapshot is much smaller (~80 MB) than in-memory snapshot (~1.1 GB)
- Replay speed: LSM is slower than in-memory (13.8K vs 35K blk/s) — LSM does real disk I/O
- Replay speed slows at slot ~12M due to UTxO growth to ~1.5M entries; recovers after slot ~15M
- Governance "prev_action_id chain mismatch" INFO messages are normal during epoch transitions
