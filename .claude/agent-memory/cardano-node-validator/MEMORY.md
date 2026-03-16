# Node Validator Agent Memory

## Key Files
- Node binary: `./target/release/torsten-node`
- CLI binary: `./target/release/torsten-cli`
- Config dir: `./config/` (preview-config.json, preview-topology.json)
- Preview DB: `/tmp/torsten-db-preview/` — epoch=1238, 2,935,506 UTxOs (run #16)
- Ledger snapshot: `<db>/ledger-snapshot.bin` (~80 MB with LSM backend)
- Node logs: `/tmp/torsten-vrf-debug.log` (run #16 — VRF nonce fix validation)

## Startup Command Pattern (LSM backend, default)
```
TORSTEN_PIPELINE_DEPTH=150 ./target/release/torsten-node run \
  --config config/preview-config.json \
  --topology config/preview-topology.json \
  --database-path /tmp/torsten-db-preview \
  --socket-path ./node-lsm-test.sock \
  --host-addr 0.0.0.0 --port 3002 \
  --metrics-port 12798 \
  > /tmp/torsten-vrf-debug.log 2>&1 &
```
NOTE: Always `pkill -f torsten-node && rm -f ./node-lsm-test.sock` before restart.
NOTE: `--utxo-backend in-memory` is NO LONGER required — LSM backend is production-worthy.
NOTE: Default metrics port is 12798. Use `--metrics-port 12799` only if Haskell node is also running.

## Preview Testnet Baselines (2026-03-16, run #16 — VRF nonce fix validation)
- Fresh Mithril import: epoch=1238, 2.7 GB archive, ~3.5 min download, total ~5 min
- Ledger replay: 4,111,875 blocks in 202s = **20,306 blk/s** (new record — snapshot already at epoch 1238)
  - 0 skipped, 0 errors — ZERO divergence from canonical chain
- Caught up to tip: 606 blocks applied, 0 rejected — ZERO VRF failures
- At-tip blocks (all 619 total): ALL applied, 0 rejected, ZERO WARN/ERROR messages
- Block hashes verified against Koios: EXACT MATCH for blocks 4112481, 4112482, 4112484, 4112493
- Memory at tip: ~5.5 GB RSS (stable)
- Peers: 5 connected

## EPOCH NONCE VALIDATION (run #16 — 2026-03-16, commit 1a11d0d)
- Epoch 1237: computed=`5230ed8ffb8cf1477924b0bea616525ddba94b19a4b15ae67ace5af484d3c9f2`
              Koios=`5230ed8ffb8cf1477924b0bea616525ddba94b19a4b15ae67ace5af484d3c9f2` **EXACT MATCH**
- Epoch 1238: computed=`b1d2f1fa41a5e46756f57922dad22335dc637b303fe4655e1d276210f18696a6`
              Koios=`b1d2f1fa41a5e46756f57922dad22335dc637b303fe4655e1d276210f18696a6` **EXACT MATCH**
- VRF header verification at tip: ZERO failures (strict mode enabled, 619 blocks received and applied)
- Issue #95 (VRF nonce computation): RESOLVED — epoch nonces match canonical chain

## VRF Nonce State Machine (commit 1a11d0d, CORRECT)
Key fixes applied in commit 1a11d0d:
1. epoch_nonce initialized to genesis hash (not ZERO) in `set_genesis_hash()`
2. Candidate nonce freeze: updates OUTSIDE 4k/f window (not inside) — was inverted
3. randomness_stabilisation_window = 4k/f = 172800 (was 3k/f = 129600)
4. stability_window_3kf field added for Alonzo/Babbage (proto < 10) — 3k/f
5. NeutralNonce identity at first epoch boundary: epoch_nonce = candidate (no hashing with ZERO)
6. multi_era.rs: Alonzo nonce_vrf_output pre-hashed to 32 bytes (eta = blake2b(vrf.0))
7. SNAPSHOT_VERSION bumped to 4 (stability_window_3kf field added)

## CRITICAL BUG #1: ScriptDataHash Ignores Reference Scripts (OPEN)
- Tx hash: `370f8772f8cc63598f5ffd5355704af6633df4123cb84514ec8bbfe6c06c26bb`
- Error: `ScriptDataHashMismatch { expected: "7482...", actual: "dfe1..." }`
- Fix: look up reference inputs in utxo_set to detect their script versions
- File: `crates/torsten-ledger/src/validation/` — compute_script_data_hash

## CRITICAL BUG #2: CollateralHasTokens Incorrect for Txs with CollateralReturn (OPEN)
- Tx hash: `95cdd9d9489916be8bc6cd8aa86b34a7a4651bf673f599a4195fd1ddbd1678b4`
- Fix: only reject when net collateral (total - return) has non-ADA assets
- File: `crates/torsten-ledger/src/validation/collateral.rs`

## OPEN BUG #3: Plutus script returns Data instead of Unit/Bool (run #14)
- Script hash: `4faf61d99fe87d6f1c4ae346f804a6b9824808a04047bd846fb1ea5f` (PlutusV2)
- Error: `Unexpected result: Constant(Data(Array(Indef([BigInt(0)...]))))`
- File to investigate: `crates/torsten-ledger/src/plutus.rs` — script context construction

## OPEN BUG #4: FeeTooSmall false positive (run #13 mainnet)
- Tx hash: `9816fcc8efdd80f350a2cca600a268a0e65c2df1b28022f07b99c382112c0fe2`
- Error: `FeeTooSmall { minimum: 168581, actual: 168537 }` (diff=44 lovelace)
- File: `crates/torsten-ledger/src/validation.rs` — min_fee calculation (ref script rounding)

## Prometheus Metrics
- Default port is 12798. `http://localhost:12798/metrics`, `/health`, `/ready`
- Metrics server starts BEFORE ledger replay

## Mainnet Full Sync Baselines (run #13 — 2026-03-15)
- Mithril import: epoch=618, 52.8 GB, ~72 min download
- Ledger replay: 13.16M blocks in 7083s = 1,857 blk/s, 2 skipped
- Memory at tip: ~19.7 GB RSS (11M UTxOs)
- All 7 era transitions clean: Byron→Shelley(208)→Allegra(236)→Mary(251)→Alonzo(290)→Babbage(365)→Conway(394)

## Operational Notes
- Always `--testnet-magic 2` with CLI query tip for correct syncProgress
- LSM backend snapshot ~80 MB; in-memory ~1.1 GB
- Replay speed on preview: 20K blk/s when snapshot available for same epoch (much faster than cold start)
- Replay speed slows at slot ~12M due to UTxO growth; recovers after slot ~15M
- Debug epoch nonces: `RUST_LOG="torsten_ledger::state::epoch=debug"` shows per-epoch nonce values
