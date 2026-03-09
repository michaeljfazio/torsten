# Node Validator Agent Memory

## Key Files
- Node binary: `./target/release/torsten-node`
- CLI binary: `./target/release/torsten-cli`
- Config dir: `./config/` (preview-config.json, preview-topology.json)
- Preview DB: `./db-preview/` (also /tmp/torsten-preview-db/) — slot ~106.4M, block ~4.09M
- Ledger snapshot: `<db>/ledger-snapshot.bin`
- Node logs: `/tmp/torsten-validation-run.log` (or /tmp/torsten-preview-node.log)

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
NOTE: socket-path can be `./node.sock` or `/tmp/torsten-preview.socket` — both work fine.

## Preview Testnet Baselines (2026-03-10, commit 461f768)
- DB at slot ~106.4M / block ~4.09M / epoch 1,231
- Peers: 52.215.17.31, 3.70.89.92, 18.117.34.199, 52.211.202.88, 3.74.40.92 (8 known, 5 hot)
- N2N handshake: ~565-741ms, version 14
- At-tip block rate: ~1 block every 15-35 seconds (live testnet, ~5% active slots)
- Block fetch pool: 4 fetchers connected in parallel (3 cold, 0 warm, 5 hot at startup)
- Catchup from intersection: 584 blocks in ~28 seconds (~21 blocks/sec at tip proximity)
- Catchup slot range: 106401541 → 106420818 (~19,277 slots)

## VRF Verification — Root Cause
See `vrf-debugging.md` for details. Summary:
- VRF fails for EVERY live block at tip (WARNING level, non-fatal)
- Root cause: epoch nonce in ledger is genesis-based (wrong) instead of epoch 1231 actual nonce
- Fix requires: full chain replay from genesis OR correct Mithril snapshot import that captures epoch nonce

## Known Issues (Persistent)
1. **VRF proof verification fails for every live block at tip** (WARNING level, non-fatal)
   - `Praos: VRF proof verification failed slot=... error=VRF verification failed`
   - Root cause: ledger epoch nonce = hash(genesis || genesis) = wrong (Mithril import bootstrapping)
   - IMPORTANT: VRF + opcert + KES failures are ALL non-fatal (WARN only, never block rejection)
   - Blocks are accepted normally despite these warnings
   - NOTE: commit 06cb82f introduced strict-mode that made these FATAL — fixed in next commit
     which made all 3 crypto verification failures always non-fatal until nonce is trustworthy

2. **UTxO/delegation/treasury all 0 at tip** — ledger starts from fresh state (no UTxOs)
   - After Mithril import, ledger starts at genesis (no UTxO data in snapshot)
   - Full ledger state requires 4M+ block replay from genesis (several hours)

3. **N2N server "Address already in use"** if old node not killed before restart
   - Always `pkill -f torsten-node && rm -f ./node.sock` before restart

4. **rollback_count metric always 0** — rollback at tip to slot 106420818 seen but not counted (confirmed 2026-03-10)
   - Log line: `WARN torsten_node::node: Rollback to slot:106420818@0aeb04836c360c80b966764fab4abfcca7665eedf018fe05ca05e73e1aaed3c5`
   - Counter remains 0 after rollback; early-return guard in rollback handler may be too aggressive
   - Investigate rollback handler in `crates/torsten-node/src/node.rs`

5. **`query stake-pools` shows garbled data** — CLI decodes wrong CBOR format
   - File: `crates/torsten-cli/src/commands/query.rs` lines 821-910
   - Fix: decoder must match CBOR map format `{pool_id_bytes -> [ratio, vrf]}`

6. **`query tip` syncProgress wrong without --testnet-magic** — 60.11% instead of 100%
   - Always pass `--testnet-magic 2` when querying preview testnet

## Fixed Issues (verified 2026-03-09, commit fd838c5 and later)
- **transactions_received_total / transactions_validated_total** — FIXED (commit after fd838c5)
  - Previously 0 (never incremented); now correctly counts txs from applied blocks
  - Snapshot 1: 32 txs; Snapshot 2: 34 txs (3 live blocks with txs between snapshots)
- **"Syncing 100.00%" log suppression** — FIXED — zero "Syncing" log messages when at tip
  - Only "New block slot=... txs=..." messages appear for live blocks
- **`peers_connected` metric** — FIXED — correctly shows 5 (1 chainSync + 4 block fetchers)
- **`peers_cold/warm/hot` metrics** — NEW — now exported (cold=3, warm=0, hot=5 at tip)
- **`query tip` block_number** — FIXED — correct block number (e.g., 4093411)
- **`query protocol-parameters` Conway fields** — FIXED — all 31 fields including governance

## Working Features Confirmed (2026-03-09, latest commit)
- Mithril snapshot import: WORKS
- Peer connections: 5 peers all connect (3 cold, 0 warm, 5 hot)
- Chain sync to tip: WORKS — reaches 100% sync, receives live blocks
- Live block reception: WORKS — ~1 block/20-60s at tip, correct "New block" log format
- Rollback handling: WORKS — clean rollback observed (slot 106410874), non-fatal
- N2C query tip: WORKS — correct slot/block/epoch/era/syncProgress (100.00%)
- N2C protocol-parameters: WORKS — all 31 fields including full Conway governance fields
- N2C gov-state, tx-mempool, treasury, committee-state: WORKS
- N2C drep-state, stake-distribution, stake-address-info, stake-snapshot: WORKS
- Prometheus metrics: WORKS — all counters including transactions now functional
- "Syncing" log suppression at 100%: WORKS — zero noise when at tip

## Prometheus Metrics (Preview at-tip, 2026-03-10, commit 461f768)
- blocks_received_total: 588 (584 catchup + 4 live after ~2 min)
- blocks_applied_total: 588
- peers_connected: 5
- peers_cold: 3, peers_warm: 0, peers_hot: 5
- sync_progress_percent: 10000 (100.00%)
- slot_number: 106,420,914
- block_number: 4,093,719
- epoch_number: 1,231
- utxo_count: 0 (no UTxO replay — Mithril bootstrap starts ledger from genesis)
- delegation_count: 1 (minimal — no full ledger replay)
- transactions_received_total: 592
- transactions_validated_total: 592
- transactions_rejected_total: 0
- rollback_count_total: 0 (BUG — 1 rollback occurred but not counted)

## Operational Notes
- `pkill -f torsten-node && rm -f ./node.sock` before restart
- Always `--testnet-magic 2` with CLI query tip for correct syncProgress
- N2N port 3001 / Metrics port 12798 — conflict if old node running
- After Mithril import, node replays ~23 blocks from intersection silently (no Syncing log), then switches to live blocks
- No `torsten-config.json` — use `config/preview-config.json` directly
