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

## Preview Testnet Baselines (2026-03-15, run #9 — LSM PageOverflow fix validation)
- Fresh Mithril import: epoch=1237, immutable=24744, 2.7 GB archive, ~9 min total
  (download 3.6min, extract 23s, verify 52s, copy 12s, cleanup 5s)
- Ledger replay (LSM backend, 65KB pages): 4,108,694 blocks in 296s = 13,848 blk/s
  - Speed profile: ~22K blk/s early (slot 0-2M), slows to ~6K at slot 12M (UTxO growth), recovers to ~14K
  - UTxO spike zone at slot ~44M: jumped from 2.3M to 3.2M in one 5s interval — NO PageOverflow
  - UTxOs at replay end: 2,938,963 (correct — matches in-memory backend count)
  - Snapshot saved: epoch=1237, 2938963 UTxOs, 79.8 MB (vs 1113 MB for in-memory)
- No PageOverflow errors anywhere in log (0 occurrences)
- No block application failures during replay (0 errors)
- N2N peer connected: 52.211.202.88:3001 rtt_ms=732, 5 hot peers total
- Caught up to tip in ~22s after network connect: 512 blocks applied
- At-tip block reception: 4 live Conway blocks received, 0 failed to apply
- Rollbacks observed: 0
- UTxO store on-disk size: 1.8 GB (vs 0 with broken LSM, vs RAM usage with in-memory)
- Total DB size: 15 GB (immutable chunk files + UTxO LSM store)
- Ledger snapshot: 79.8 MB (compact — LSM has UTxOs on disk, snapshot only stores ledger state)

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

## Known Issues (Current — 2026-03-15, run #9)
1. **ScriptDataHash ignores reference scripts** — CRITICAL, causes block application failure
   - File: `crates/torsten-ledger/src/validation.rs:785-795`
2. **CollateralHasTokens fires for collateral with collateral_return** — CRITICAL, same effect
   - File: `crates/torsten-ledger/src/validation.rs:650-687`
3. **macOS high-memory OOM during replay** — use default profile (LSM now works on macOS)

## Working Features Confirmed (2026-03-15, run #9, LSM backend)
- Build: WORKS
- Fresh Mithril import: WORKS — 2.7 GB, epoch=1237
- Ledger replay (LSM backend): WORKS — 13,848 blk/s, 4.1M blocks in 296s
- No PageOverflow errors: CONFIRMED — 0 errors across all 4.1M blocks
- UTxO count correct: CONFIRMED — 2,938,963 (not 91 or 693K like before)
- Snapshot save at replay end: WORKS — 79.8 MB at epoch 1237
- Peer connections: WORKS — 52.211.202.88:3001, 5 hot peers
- Catch-up to tip: WORKS — 512 blocks in ~22s
- Live block reception at tip: WORKS — 4 Conway blocks received, 0 failed
- N2C query tip: WORKS — slot=106915709, block=4109209, epoch=1237, era=Conway, syncProgress=100.00
- N2C protocol-parameters: WORKS — full Conway PParams with PlutusV1+V3 cost models
- N2C treasury: WORKS — Treasury=14067498812 ADA, Reserves=870230606 ADA
- N2C stake-distribution: WORKS — multiple pools
- N2C gov-state: WORKS — committee_members=1, active_proposals=2
- Prometheus metrics: WORKS — utxo_count=2938973, blocks_applied=516, peers_connected=5, epoch=1237
  treasury=14067498812557595 lovelace, drep_count=8791, proposal_count=2, sync_progress=10000 (100%)
- LSM UTxO store on-disk: WORKS — 1.8 GB of on-disk state, correct data

## Operational Notes
- Always `--testnet-magic 2` with CLI query tip for correct syncProgress
- CLI subcommand is `query treasury` (not `query account-state`)
- gov-state shows committee_members=1 on epoch 1237 (was 7 on epoch 1236 — committee changed)
- LSM backend snapshot is much smaller (~80 MB) than in-memory snapshot (~1.1 GB)
- Replay speed: LSM is slower than in-memory (13.8K vs 35K blk/s) — LSM does real disk I/O
- Replay speed slows at slot ~12M due to UTxO growth to ~1.5M entries; recovers after slot ~15M
- Governance "prev_action_id chain mismatch" INFO messages are normal during epoch transitions
