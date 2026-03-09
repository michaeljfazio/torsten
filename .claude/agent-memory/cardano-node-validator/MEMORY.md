# Node Validator Agent Memory

## Key Files
- Node binary: `./target/release/torsten-node`
- CLI binary: `./target/release/torsten-cli`
- Config dir: `./config/` (preview-config.json, preview-topology.json)
- Preview DB (Mithril): `/tmp/torsten-preview-db/` (4,092,598 blocks, slot 106384314)
- Ledger snapshot: `<db>/ledger-snapshot.bin`
- Node logs: `/tmp/torsten-preview-node.log`

## Startup Command Pattern
```
TORSTEN_PIPELINE_DEPTH=150 ./target/release/torsten-node run \
  --config config/preview-config.json \
  --topology config/preview-topology.json \
  --database-path /tmp/torsten-preview-db \
  --socket-path /tmp/torsten-preview.socket \
  --host-addr 0.0.0.0 --port 3001 \
  > /tmp/torsten-preview-node.log 2>&1 &
```
NOTE: socket path must be `/tmp/torsten-preview.socket` (not `.sock`) — matches the run instructions.

## Preview Testnet Baselines (2026-03-09, commit c580901)
- Mithril import: 4,092,598 blocks, ~6 min total (download 2.7GB + import)
- DB at slot ~106.4M / block ~4.09M / epoch 1,231
- Peers: 99.80.240.19, 52.215.17.31, 3.74.40.92, 18.185.163.167, 52.211.202.88
- N2N handshake: ~729-745ms, version 14
- At-tip block rate: ~1 block every 20-60 seconds (live testnet, ~5% active slots)
- Process memory at tip: ~41MB RSS (very stable)
- Block fetch pool: 4 fetchers connected in parallel
- Ledger replay: 500-674 blocks from ChainDB intersection to tip (silent, < 5s log timer)

## VRF Verification — Root Cause
See `vrf-debugging.md` for details. Summary:
- VRF fails for EVERY live block at tip (WARNING level, non-fatal)
- Root cause: epoch nonce in ledger is genesis-based (wrong) instead of epoch 1231 actual nonce
- Fix requires: full chain replay from genesis OR correct Mithril snapshot import that captures epoch nonce

## Known Issues (Persistent)
1. **VRF proof verification fails for every live block at tip** (WARNING level, non-fatal)
   - `Praos: VRF proof verification failed slot=... error=VRF verification failed`
   - Root cause: ledger epoch nonce = hash(genesis || genesis) = wrong
   - Blocks are accepted anyway (warning only, non-blocking)

2. **`query tip` block_number now FIXED** (verified 2026-03-09, commit c580901)
   - `query tip` returns correct block number (e.g., 4093271) via N2C
   - Previously was 0 — now correctly reads from ChainTip response

3. **`peers_connected` metric undercount** — shows 1 instead of 5 connected peers
   - `hot_peer_count()` only counts peers promoted via `promote_to_hot()`
   - Block fetcher peers (4 of them) are `peer_connected()` (warm) but NOT `promote_to_hot()`
   - Only the primary ChainSync peer is promoted to hot
   - Fix: call `promote_to_hot()` for block fetcher peers too
   - File: `crates/torsten-node/src/node.rs` around line 915

4. **`query tip` syncProgress wrong without --testnet-magic** — 60.11% instead of 100%
   - CLI uses mainnet Shelley start (1596059091) instead of preview genesis (1666656000)
   - Always pass `--testnet-magic 2` when querying preview testnet

5. **UTxO/delegation/treasury all 0 at tip** — ledger starts from fresh state (no UTxOs)
   - After Mithril import, ledger starts at genesis (no UTxO data in snapshot)
   - Snapshot saved by node only captures epoch metadata, not UTxO set
   - Full ledger state requires 4M+ block replay from genesis (several hours)

6. **N2N server "Address already in use"** if old node not killed before restart
   - Always `pkill -f torsten-node && rm -f /tmp/torsten-preview.socket` before restart
   - Also clears metrics server on port 12798

7. **rollback_count metric always 0** even when rollback observed in logs (existing bug)

8. **`query stake-pools` shows garbled data** — CLI decodes wrong CBOR format
   - `StakePools` CLI calls `query_stake_distribution()` (tag 5, GetStakeDistribution)
   - Server returns `[4, [map(n){pool_id_bytes -> [ratio, vrf]}]]` (CBOR map)
   - CLI tries to decode as `array(n)` of maps with string keys — completely wrong
   - The HFC wrapper `array(1)` is misread as `arr_len=1`, then map body as 1 garbled entry
   - File: `crates/torsten-cli/src/commands/query.rs` lines 821-910
   - Fix: Either fix the decoder to match CBOR map format, or add tag-16 query to n2c_client

9. **`query protocol-parameters` missing Conway-era fields** (WARNING)
   - Server correctly encodes all 31 fields (including cost models, governance thresholds)
   - n2c_client `parse_protocol_params_cbor()` skips fields 15 (costModels), 16 (prices), 22-30 (governance)
   - File: `crates/torsten-network/src/n2c_client.rs` lines 847-888
   - All governance thresholds, DVT/PVT values, drepDeposit, govActionDeposit are missing from output
   - Fix: decode and include all 31 fields in JSON output

## Working Features Confirmed (2026-03-09, commit c580901)
- Mithril snapshot import: WORKS
- Peer connections: 5 peers all connect successfully
- Chain sync to tip: WORKS — reaches 100% sync, receives live blocks
- Live block reception: WORKS — ~1 block/20-60s at tip (1 or 2 txs per block observed)
- Rollback handling: WORKS — clean rollback observed, non-fatal
- N2C query tip: WORKS — correct slot/block/epoch/era/syncProgress
- N2C protocol-parameters: PARTIAL — basic fields present, Conway fields missing
- N2C gov-state: WORKS — responds correctly (0 values without UTxO replay)
- N2C tx-mempool: WORKS — correct slot, capacity=16384, 0 txs
- N2C treasury: WORKS — responds (0 without UTxO replay)
- N2C committee-state: WORKS — correct epoch, 0 members
- N2C drep-state: WORKS — 0 DReps (no UTxO replay)
- N2C stake-distribution: WORKS — correct CBOR map format, 0 pools
- N2C stake-address-info: WORKS — bech32 decode + query works, empty result for unknown address
- N2C stake-snapshot: WORKS — 0 stake (no UTxO replay)
- Prometheus metrics: WORKS — all metrics correctly populated (except peers_connected undercount)

## Prometheus Metrics (Preview, at-tip 2026-03-09, commit c580901)
- blocks_received_total: 684 (in ~8 min of node runtime)
- blocks_applied_total: 684
- peers_connected: 1 (undercount — actually 5 connected)
- sync_progress_percent: 10000 (100.00%)
- slot_number: 106,406,193
- block_number: 4,093,282 (correct in metrics AND N2C query now)
- epoch_number: 1,231
- utxo_count: 0 (no UTxO replay)
- delegation_count: 0
- treasury_lovelace: 0

## Operational Notes
- `pkill -f torsten-node && rm -f /tmp/torsten-preview.socket` before restart
- Always `--testnet-magic 2` with query tip for correct syncProgress
- N2C socket: `/tmp/torsten-preview.socket` — confirmed functional
- N2N port 3001 / Metrics port 12798 — conflict if old node running
- No `torsten-config.json` exists — use `config/preview-config.json` directly
- After ledger replay (< 5s), node logs go silent then resume at tip — this is NORMAL
- First sync: 500-674 blocks replayed silently from ChainDB intersection
