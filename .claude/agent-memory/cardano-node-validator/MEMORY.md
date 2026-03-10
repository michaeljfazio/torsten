# Node Validator Agent Memory

## Key Files
- Node binary: `./target/release/torsten-node`
- CLI binary: `./target/release/torsten-cli`
- Config dir: `./config/` (preview-config.json, preview-topology.json)
- Preview DB: `./db-preview/` — slot ~106.4M, block ~4.09M
- Ledger snapshot: `<db>/ledger-snapshot.bin`
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

## Preview Testnet Baselines (2026-03-10, commit c973bef)
- DB at slot ~106.4M / block ~4.09M / epoch 1232
- Peers: 18.185.163.167, 18.117.34.199, 3.134.226.73, 99.80.240.19, 3.70.89.92 (8 known, 5 hot)
- N2N handshake: ~580-752ms, version 14
- Catchup from intersection: ~143 blocks/sec
- Live block rate: ~1 block every 9-52 seconds (~5% active slots)
- Chain correctness: ALL 6 live block hashes confirmed vs Koios (slots 106455572-106455706)
- Rollback handling: CLEAN — rollback to slot 106455502 confirmed canonical by Koios
- Build time: 28 seconds (8 crates compiled)
- Memory at tip: ~45MB RSS, ~0% CPU idle — stable

## Protocol Parameters Discrepancy (Known — Mithril bootstrap issue)
Torsten `query protocol-parameters` returns GENESIS values, NOT current on-chain values.
Key wrong fields vs Koios epoch_params epoch 1232:
- maxBlockBodySize: Torsten 65536, actual 90112
- protocolVersion: Torsten {major:9,minor:0}, actual {major:10,minor:6/7}
- committeeMinSize: Torsten 0, actual 3
- dRepActivity: Torsten 20, actual 31
- maxTxExecutionUnits.memory: Torsten 14M, actual 16.5M
- maxBlockExecutionUnits: Torsten {62M mem, 40B steps}, actual {72M mem, 20B steps}
- costModels: Torsten {} (empty), actual full PlutusV1/V2/V3
- dvtPPGovGroup: Torsten 67/100, actual 0.75
Root cause: ledger state starts from genesis after Mithril import; governance updates not applied.
Fix: requires full 4M+ block replay from genesis (several hours), not a code bug.

## VRF Verification — Root Cause
- VRF fails for EVERY live block at tip (DEBUG level since commit 1e5df5a, non-fatal)
- Root cause: epoch nonce wrong (genesis-based after Mithril import)
- Blocks are accepted normally; VRF/KES/opcert failures never cause block rejection

## Known Issues (Persistent)
1. **Protocol params show genesis values** — not on-chain updated values (see above)
2. **UTxO/delegation/treasury/pool_count all 0** — no full ledger replay after Mithril
3. **`query tip` returns zeros immediately after startup** — race condition
   - `update_query_state()` not called until "Caught up to chain tip" or 30s periodic
   - Workaround: wait for "Caught up to chain tip" log line before querying
4. **`query stake-pools` garbled data** — CLI decoder mismatch
   - File: `crates/torsten-cli/src/commands/query.rs` lines 821-910
5. **`query tip` syncProgress wrong without --testnet-magic** — always pass `--testnet-magic 2`
6. **N2N server "Address already in use"** if old node still running

## Working Features Confirmed (2026-03-10, commit c973bef)
- Build: WORKS — clean, zero warnings, 28 seconds
- Peer connections: 5 peers (3 cold, 0 warm, 5 hot)
- Chain sync to tip: WORKS — 143 b/s catchup, reaches 100% sync
- Live block reception: WORKS — 6 live blocks in ~3 min at tip
- Rollback handling: WORKS — clean, counter=1
- N2C query tip: WORKS (after warm-up: slot 106455706, block 4094799, epoch 1232, 100.00%)
- N2C protocol-parameters: WORKS (returns values, though genesis-based)
- N2C gov-state, tx-mempool (shows correct tip slot), treasury: WORKS
- Prometheus metrics: WORKS — all counters functional
- Zero WARNs or ERRORs in logs at tip (only expected rollback WARN)
- Zero "Syncing" log messages at 100%

## Prometheus Metrics (Preview at-tip, 2026-03-10, commit c973bef)
- blocks_received_total: 1668 (1662 catchup + 6 live)
- blocks_applied_total: 1668 (zero dropped)
- peers_connected: 5, peers_cold: 3, peers_warm: 0, peers_hot: 5
- sync_progress_percent: 10000 (100.00%)
- slot_number: 106,455,706, block_number: 4,094,799, epoch_number: 1,232
- utxo_count: 0, delegation_count: 1, treasury_lovelace: 0
- drep_count: 0, proposal_count: 0, pool_count: 0
- transactions_received_total: 1688, transactions_validated_total: 1688
- transactions_rejected_total: 0
- rollback_count_total: 1

## Operational Notes
- Always `--testnet-magic 2` with CLI query tip for correct syncProgress
- N2N port 3001 / Metrics port 12798 — conflict if old node running
- No `torsten-config.json` — use `config/preview-config.json` directly
- Wait for "Caught up to chain tip" log line before running CLI queries
