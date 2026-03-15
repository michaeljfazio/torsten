---
name: project_state_2026_03_15
description: State assessment as of end-of-day 2026-03-15, post session-7 milestones and tranche planning — updated with Tranche 1 completion status and Tranche 2 execution plan
type: project
---

# Torsten State Assessment — 2026-03-15 (Updated: Tranche 2 Planning)

**Why:** Tracks completed milestones and open gaps for the tranche planning cycle.

**How to apply:** Use when asked about current state, gap analysis, or what to work on next.

## Tranche 1 — COMPLETE (Preview Testnet Compliance)

All Tranche 1 items resolved or confirmed done:
- Testnet re-run #10: PASS (0 errors, 2.9M UTxOs, RUPD correct)
- Plutus cost model: PASS (encoding correct, values match Koios)
- NonMyopicMemberRewards formula: IMPLEMENTED in protocol.rs (maxPool' with pledge/margin/cost)
- Tx output re-encoding verification: DONE (raw_cbor path in state/mod.rs)
- CLI improvements: build-raw, query utxo --tx-in, stake-address-info server filtering
- Credential type discrimination: DONE
- GSM, committee fields, ValidationTagMismatch tolerance: DONE
- vrf_dalek: pinned to specific commit (e1dcc02)

## Tranche 2 — Mainnet Readiness (CURRENT)

### T2-1: torsten-lsm Mainnet-Scale Stress Test (HIGH / M)
- Existing stress tests go to 50K entries only
- Need 20M+ UTxO test with compaction under concurrent flush, WAL crash recovery
- File: `crates/torsten-lsm/src/tree.rs` (stress_tests module)
- Owner: Storage Lead
- Can run in parallel with T2-2 and T2-3

### T2-2: Mainnet Configuration Validation (HIGH / M)
- Config files exist: `config/mainnet-*.json`
- Need to boot node on mainnet, connect to peers, sync Byron epoch 0→1
- Acceptance: no panic on startup, handshake succeeds, first Shelley block applied
- Owner: Ops Lead
- Depends on: T2-3 (Byron ledger) for correct UTxO state after Byron

### T2-3: Byron Ledger Validation (HIGH / L)
- `crates/torsten-ledger/src/eras/byron.rs` is a 12-line stub
- Byron blocks pass through apply_block without UTxO enforcement
- For Mithril users: unaffected (skip Byron). For genesis-from-Byron: UTxO state wrong.
- Owner: Ledger Lead
- Note: Pallas parses Byron txs; need to implement UTxO spend rules + fee check

### T2-4: Replay Throughput Benchmark (MEDIUM / S)
- Target: ~10,600 blocks/s ImmutableDB replay
- No bench harness exists yet (`benches/` dir doesn't exist)
- Need criterion bench for: block replay, LSM insert/flush, chain sync pipeline
- Owner: Perf Lead
- Can run in parallel with all other items

### T2-5: VolatileDB WAL prev_hash (MEDIUM / S)
- WAL replay uses Hash32::ZERO as prev_hash placeholder (comment: line 239 volatile_db.rs)
- Header size (56 bytes) has no slot for prev_hash — would need format bump
- Impact: successor tracking broken after crash recovery until new blocks restore it
- Fork detection during recovery window is impaired
- Owner: Storage Lead
- Can run in parallel

## NEW Tranche 2 Items (identified during Tranche 1 execution)

### T2-6: NonMyopicMemberRewards Integration Test (HIGH / S)
- Formula is implemented but has ZERO end-to-end test coverage
- Koios MCP available: koios_pool_history can provide ground truth amounts
- Risk level: same class as the VRF nonce_vrf bug — formula looks right, no E2E validation
- File: `crates/torsten-network/src/query_handler/protocol.rs`
- Owner: Ledger/Test Lead
- Can run in parallel with T2-2

### T2-7: N2C Golden Test Coverage Expansion (MEDIUM / M)
- Golden test infrastructure exists (`tests/golden/`) with n2c fixtures for GetCurrentPParams and GetEpochNo
- Missing: GetNonMyopicMemberRewards, GetRewardInfoPools, GetPoolState, GetDRepState, GetConstitution
- These are exercised in run #10 but not in CI-runnable golden tests
- Owner: Test Lead
- Can run in parallel with all other items

### T2-8: WAL prev_hash Format Upgrade (MEDIUM / S)
- Expand WAL entry from 56 bytes to 88 bytes (add 32-byte prev_hash field)
- Requires: WAL_HEADER_SIZE bump, rewrite logic update, replay logic update
- Add migration: detect old 56-byte format, re-derive prev_hash from block CBOR
- Owner: Storage Lead
- Blocked by: T2-5 design decision

## Tranche 3 — Features & Protocol Extensions (DETAILED PLAN 2026-03-15)

### T3-1: `transaction build` auto-balancing (HIGH/L)
- Phase A: Add --socket-path to BuildArgs, add query_utxo_by_inputs() to N2CClient
- Phase B: Live fee estimation from GetCurrentPParams, change output calculation
- Phase C: Largest-first coin selection when --tx-in omitted
- Files: transaction.rs, n2c_client.rs, new coin_selection.rs
- Owner: CLI Lead + Network Lead
- Critical: multi-asset change uses coinsPerUTxOByte * output_size, not flat min

### T3-2: Plutus script witness in CLI (HIGH/L)
- Add --tx-in-script-file, --redeemer-file, --datum-file, --tx-in-execution-units
- New plutus_witness.rs with ScriptWitness enum
- Hash script subcommand (torsten hash script --script-file)
- Dependency: T3-1 Phase B (fee affects witness set via tx size)
- Risk: positional arg parsing (--tx-in binds following witness args); fallback: txhash#idx:script.plutus syntax

### T3-3: CDDL conformance Plutus vectors + roundtrip tests (HIGH/M)
- 5 new Plutus execution vectors (always-succeeds/always-fails, reference scripts)
- 10 real tx CBOR roundtrip tests via Koios MCP (each era, each tx type)
- Fully parallel — no dependencies on T3-1 or T3-2

### T3-4: LoE enforcement wired into block pipeline (MEDIUM/S)
- loe_limit() exists and is tested; NOT called from node.rs apply_block path
- Add peer_tips snapshot to PeerManager (hot_peer_tips() -> Vec<Point>)
- Add pending_blocks VecDeque in sync loop; gate on loe_limit before apply_block
- Use tokio::watch channel for LoE limit updates to wake pending buffer drain
- Risk: pending buffer must be capped (max 500 blocks) to prevent unbounded growth

### T3-5: Peer sharing privacy hardening (MEDIUM/S)
- Add ConnectionDirection::Outbound/Inbound to PeerInfo
- Only share Outbound peers in peers_for_sharing()
- Sort shared peers by reputation score (highest first) — currently random HashMap order
- Fully parallel

### T3-6: N2N V16 outbound proposal (LOW/S)
- Add V16=16 to NodeToNodeVersion; add to propose_versions()
- Dependency: T3-5 (peer_sharing state must be coherent) + T2-2 complete
- Risk: CBOR encoding of V16 version data field order — validate against real cardano-node 10.x

### T3-NEW-1: Multi-era output format (LOW/S)
- Add --era flag to transaction build (babbage|conway)
- 30-line change, unblocks Guild Operators script compatibility
- Dependency: T3-1 Phase A

### T3-NEW-2: Mempool revalidation on epoch boundary (MEDIUM/S)
- cardano-node drains+revalidates mempool every epoch boundary
- Torsten only clears on rollback — correctness gap for block producers
- Add mempool.revalidate() call in epoch transition handler in node.rs
- Fully parallel, no dependencies

### Items deferred to Tranche 4:
- Genesis bootstrap from scratch (requires T2-3 Byron + T3-4 LoE first)
- Full Ouroboros Genesis protocol (BLP peer selection, full GDD) — not just V16 version bump

### Critical path: T3-1A → T3-1B → T3-2 (everything else parallel)

## Key Risk Flags (current)

- Byron ledger stub: genesis-from-Byron users accumulate incorrect UTxO state
- WAL prev_hash placeholder: fork detection impaired during post-crash recovery window
- NonMyopicMemberRewards: formula implemented, no E2E validation against live chain data
- Mainnet not yet booted: config files exist but node never connected to mainnet peers
