# Node Validator Agent Memory

## Key Files
- Node binary: `./target/release/torsten-node`
- CLI binary: `./target/release/torsten-cli`
- Config dir: `./config/` (preview-config.json, preview-topology.json)
- Preview DB: `./db-preview/` — slot ~106.778M, block ~4.1048M, epoch 1235
- Ledger snapshot: `<db>/ledger-snapshot.bin` (~65 MB without UTxOs — see Known Issues #3)
- Node logs: `/tmp/torsten-validation-run.log`

## Startup Command Pattern
```
TORSTEN_PIPELINE_DEPTH=150 ./target/release/torsten-node run \
  --config config/preview-config.json \
  --topology config/preview-topology.json \
  --database-path ./db-preview \
  --socket-path ./node.sock \
  --host-addr 0.0.0.0 --port 3001 \
  > /tmp/torsten-validation-run.log 2>&1 &
```
NOTE: Always `pkill -f torsten-node && rm -f ./node.sock` before restart.
NOTE: Delete `db-preview/ledger-snapshot.bin` to force fresh replay with genesis seeding.

## Preview Testnet Baselines (2026-03-14, run #6 — main 6b32d4c)
- DB at slot ~106.778M / block ~4.1048M / epoch 1235 (resumed from run #5 snapshot)
- Build: already built (incremental, 0.17s) — previous release binary still valid
- New commits: 4806c2a (Phase-2 Plutus, P2P governor, GSM, enhanced mempool), 6b32d4c (MsgRejectTx CBOR)
- Snapshot loaded: epoch=1235, utxos=0 (UTxO-HD LSM persistence bug still present, unfixed)
- Single peer connected at startup: 3.74.40.92:3001 rtt_ms=738 (5 hot peers total)
- Catch-up to tip after snapshot load: 9 blocks in ~86s (slower due to sparse blocks at tip)
- "Caught up to chain tip blocks_applied=9" at T+86s
- At-tip block reception: 3 blocks received each with 1 tx, all failed to apply (UTxO bug)
- Rollbacks observed: 0
- Flush on SIGTERM: 12 volatile blocks flushed → 65.3 MB snapshot saved → Shutdown complete
- Cascading UTxO divergence observed: tx outputs from failed blocks referenced by subsequent blocks

## Prometheus Metrics Port Conflict Note
- Haskell cardano-node also runs on `localhost:12798` (127.0.0.1 only)
- Torsten binds to `*:12798` (all interfaces)
- `curl http://localhost:12798/metrics` hits the HASKELL node (more specific binding wins)
- Use `curl http://192.168.1.112:12798/metrics` (LAN IP) to reach Torsten metrics (IP may vary)
- Or: kill the Haskell node before running Torsten for exclusive port access
- NOTE: Metrics gauges show 0 for ~30s after startup before first update. This is normal.

## N2C Client Bug: Large Response Reassembly (2026-03-13, OPEN)
- **BUG**: `recv_segment()` in `crates/torsten-network/src/n2c_client.rs:687` only handles ONE mux segment
- Fix needed: collect multiple segments with same protocol_id until CBOR is complete
- Queries affected: `query drep-state` and `query pool-params` (and any large response)
- Error message: `Error: Failed to query DRep state: Protocol error: response too large`

## Storage Architecture (post-redesign, commit 83c4f11)
- ImmutableDB: append-only `.chunk` + `.secondary` files (db-preview/immutable/)
- tip.meta file tracks immutable tip (binary: slot u64 BE + hash32 + block_no u64 BE)
- VolatileDB: in-memory HashMap, last k=2160 blocks, LOST ON RESTART
- ChainDB: routes volatile→immutable for blocks deeper than k
- UtxoStore: cardano-lsm in `db-preview/utxo-store/` (active/, snapshots/ subdirs)
- Ledger snapshot: bincode-serialized LedgerState, epoch-numbered + latest symlink

## FIXED: Shutdown Flush (2026-03-13, validated multiple runs)
- On SIGTERM: volatile blocks flushed to ImmutableDB FIRST, then snapshot saved
- Log: "Flushed volatile blocks to ImmutableDB blocks=N" → "Snapshot saved" → "Shutdown complete"
- On restart: snapshot loads in <1 second (NO replay) — confirmed working
- File: `crates/torsten-storage/src/chain_db.rs` (flush_all_to_immutable)
- File: `crates/torsten-node/src/node.rs` line ~1890 (shutdown flow)

## CRITICAL BUG: UTxO-HD LSM Store Not Persisted (2026-03-14, OPEN — unfixed in run #6)
- Root cause: `open_with_config()` called on startup instead of loading existing LSM data
- Also: `save_utxo_snapshot()` in `crates/torsten-ledger/src/state/mod.rs:863` is NEVER called
- Effect: every restart loses entire UTxO state → blocks with transactions fail to apply
- Error pattern: `InputNotFound("txhash#N")` for ALL inputs in any tx-containing block
- Symptom: `utxo_count=0` in Prometheus metrics, snapshot is ~65MB (not ~1.1GB)
- Cascading divergence: failed tx outputs get referenced by later blocks, compounding failures
- Files to fix:
  1. `crates/torsten-node/src/node.rs` ~line 927: call `open_from_snapshot()` if snapshot exists
  2. `crates/torsten-ledger/src/state/mod.rs:863`: `save_utxo_snapshot()` exists but is dead code
  3. Shutdown: add call to `save_utxo_snapshot()` alongside snapshot save

## Known Issues (Current — 2026-03-14, run #6)
1. **Bincode snapshot version mismatch on struct field addition** — non-fatal, warning fires
   - Warning: `WARN torsten_ledger::state: Snapshot version mismatch — snapshot may fail to load.`
   - File: `crates/torsten-ledger/src/state/mod.rs` (SNAPSHOT_VERSION = 2, load_snapshot)
2. **N2C large response reassembly** — `recv_segment` only handles one 65535-byte segment
   - Affects: `query drep-state`, `query pool-params`
   - File: `crates/torsten-network/src/n2c_client.rs:687`
3. **CRITICAL: UTxO-HD LSM store not persisted between restarts** — see CRITICAL BUG section above

## Working Features Confirmed (2026-03-14, run #6, main branch 6b32d4c)
- Build: WORKS — incremental compile, 0.17s (no crates recompiled)
- Snapshot load: WORKS — epoch=1235, instant load, no warning
- Peer connections: WORKS — 3.74.40.92:3001 rtt_ms=738, 5 hot peers
- Catch-up to tip: WORKS — 9 blocks applied in ~86s after snapshot restore
- Empty-block processing: WORKS — blocks with txs=0 apply cleanly
- N2C query tip: WORKS — syncProgress=100.00%, era=Conway, epoch=1235, slot=106778502
- N2C treasury: WORKS — Treasury=9,141,780,486 ADA, Reserves=2,620,888,790 ADA
- N2C constitution: WORKS — empty URL + zero hash + no guardrail (expected on preview)
- N2C protocol-parameters: WORKS — full Conway PParams with Plutus V1/V3 cost models
- N2C stake-distribution: WORKS — 657 pools listed with fractions
- N2C gov-state: WORKS — committee_members=7, active_proposals=2 (TreasuryWithdrawals)
- N2C ratify-state: WORKS — enacted=0, expired=0, delayed=false
- N2C constitution: WORKS — empty URL, zero hash, no guardrail script
- Prometheus metrics: WORKS — http://192.168.1.112:12798/metrics (use LAN IP)
  - blocks_applied=12, sync_progress=10000 (100.00%), utxo_count=0 (BUG)
  - peers_connected=5, hot=5, epoch=1235, treasury=9.14T lovelace
  - drep_count=8791, proposal_count=2, pool_count=657, delegation_count=11560
  - NOTE: metrics show zeros for ~30s on startup before first update — normal behavior
- SIGTERM shutdown: WORKS — flushes 12 volatile blocks, saves 65.3 MB snapshot instantly
- MsgRejectTx CBOR encoding: NEW in 6b32d4c — not tested (no mempool tx submission done)
- GSM/P2P governor/enhanced mempool: NEW in 4806c2a — code present, no GSM state visible in logs

## Operational Notes
- Always `--testnet-magic 2` with CLI query tip for correct syncProgress
- N2N port 3001 / Metrics port 12798 — conflict if old node running
- No `torsten-config.json` — use `config/preview-config.json` directly
- On restart after clean SIGTERM: snapshot loads in <1 second (NO replay) — WORKS
- On restart after crash/SIGKILL: still does full replay — volatile data lost without flush
- CLI subcommand is `query treasury` (not `query account-state`)
- gov-state shows committee_members=7 — governance active on preview
