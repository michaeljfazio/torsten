---
name: project_state_2026_03_15
description: State assessment as of 2026-03-15 — Tranche 5 (SPO Mainnet Readiness) planning, post-session-8 milestone summary
type: project
---

# Torsten State Assessment — 2026-03-15 (Updated: Tranche 5 Planning)

**Why:** Tracks completed milestones and open gaps for the tranche planning cycle.

**How to apply:** Use when asked about current state, gap analysis, or what to work on next.

---

## Session-8 Accomplishments (38 commits)

- torsten-lsm custom LSM engine (replacing cardano-lsm)
- RUPD timing fix, Byron EBB chain continuity fix
- state/mod.rs split (was 8918L)
- Mainnet validated: 115K blocks, 5 Byron epoch boundaries, 0 errors
- Preview validated: 4.1M blocks, 0 errors
- torsten-tui dashboard (ratatui, 12 tests)
- Transaction build auto-balance
- CLI hardening
- Storage hardening (mmap unwraps, WAL guard, mem::forget)
- Dead code cleanup (45→33 annotations remain)
- Security/community standards, nightly benchmarks, code scanning
- Wiki with 8 ADRs, 6 pages, CI badges
- Architectural review items addressed (phantom deps, file splits)
- Conformance test suite: 58 vectors (30 utxo, 14 cert, 8 gov, 6 epoch)
- N2C golden tests: 4 tests (GetCurrentPParams, GetEpochNo, result encoding)

## Tranche 5 — SPO Mainnet Readiness (CURRENT)

**Mission:** Get Torsten to a state where a technically capable SPO can run it on mainnet
as a passive observer (non-block-producing), connect to mainnet peers, sync from
Mithril snapshot, and serve cardano-cli queries. Not a full production node yet —
but the bar for "worth trying" in an SPO's test environment.

**The gap list for this bar:**
1. Mainnet full sync validation (Byron→Shelley→current) — never been run end-to-end
2. node.rs and validation.rs are unmaintainable at 5649L / 6052L — bugs hide there
3. NonMyopicMemberRewards has zero E2E test coverage (same risk class as the nonce_vrf bug)
4. cncli compatibility: SPOs depend on it for leader log; Torsten has no cncli compatibility layer
5. WAL prev_hash placeholder is a correctness gap for crash-restart scenarios
6. Dead code (33 annotations) signals incomplete implementations that confuse SPOs reading the code

---

### T5-1: Full Mainnet Sync Validation — Byron→Shelley→Conway (CRITICAL / L)
- Never been run past 115K blocks (Mithril import skips Byron entirely for most users)
- Acceptance: sync from Mithril mainnet snapshot, reach tip, 0 errors over 48hr window
- Files: mainnet config, network bootstrap
- Owner: Ops Lead + Node Lead
- Blocker for: cncli compatibility testing, block producer validation
- Parallelizable: NO — gates T5-4 and T5-6

### T5-2: node.rs Split (5649 lines → ≤5 files of ≤800L each) (HIGH / M)
- node.rs handles: sync loop, block apply, peer management dispatch, epoch transitions,
  snapshot triggers, mempool, block announce, LoE dispatch — too many concerns
- Proposed split:
  - sync_loop.rs — pipelined ChainSync receive + apply loop
  - peer_dispatch.rs — hot/warm/cold peer lifecycle, BlockFetch task launch
  - snapshot_manager.rs — snapshot trigger policy, save/load coordination
  - epoch_handler.rs — epoch boundary logic, RUPD, transition
  - node.rs — thin coordinator (<300L)
- Owner: Node Lead
- Can run in parallel with T5-3 and T5-4

### T5-3: validation.rs Split (6052 lines → ≤6 files of ≤800L each) (HIGH / M)
- validation.rs handles: Phase-1 rules for all eras, fee checks, size checks,
  script witness checks, governance rules, multiasset checks — unbounded growth vector
- Proposed split (by era group + concern):
  - phase1/shelley_alonzo.rs
  - phase1/babbage_conway.rs
  - phase1/common.rs (shared predicates)
  - phase1/scripts.rs (witness/script integrity)
  - phase1/governance.rs (CIP-1694 tx-level rules)
  - validation.rs — dispatcher only (<200L)
- Owner: Ledger Lead
- Can run in parallel with T5-2 and T5-4

### T5-4: NonMyopicMemberRewards E2E Test (HIGH / S)
- Formula implemented in protocol.rs but zero end-to-end test coverage
- Risk: same class as the Alonzo VRF nonce_vrf bug — formula looks right until it doesn't
- Use Koios MCP: koios_pool_history for per-pool reward amounts per epoch
- Use koios_epoch_params for a0, nOpt, rho, tau to recompute expected NMRW
- Acceptance: within 1 lovelace of Koios ground truth for 3 mainnet pools across 5 epochs
- Owner: Ledger Lead + Test Lead
- Can run in parallel with T5-1

### T5-5: WAL prev_hash Format Upgrade (MEDIUM / S)
- Expand WAL entry: 56 bytes → 88 bytes (add 32-byte prev_hash field)
- Add migration: detect old 56-byte format on open, re-derive prev_hash from block CBOR
- Acceptance: crash-restart test shows correct fork detection after recovery
- Owner: Storage Lead
- Can run in parallel with all other items

### T5-6: cncli Compatibility Layer (MEDIUM / L)
- SPOs universally use cncli for leader schedule computation and block logging
- cncli queries the node via N2C LocalStateQuery; Torsten already implements all required tags
- Need: validate that cncli 6.x can connect to a running Torsten node and:
  - `cncli ping` returns success
  - `cncli sync` syncs blocks from Torsten's N2C socket
  - `cncli leaderlog` computes leader schedule using Torsten's VRF/epoch data
- This is primarily a validation exercise, not a new implementation
- Likely issues: N2C handshake version negotiation, GetStakeDistribution2 response format
- Owner: Ops Lead + Network Lead
- Depends on: T5-1 (mainnet sync running)

### T5-7: N2C Golden Tests Expansion (MEDIUM / M)
- Current: 4 tests (GetCurrentPParams, GetEpochNo, two result encodings)
- Need: GetNonMyopicMemberRewards, GetRewardInfoPools, GetPoolState, GetDRepState,
        GetConstitution, GetStakeDistribution2, GetAccountState, GetCBOR wrapping
- Each test: capture real CBOR from Haskell cardano-node, assert Torsten encodes identically
- 8 new golden fixture files + 8 new test cases
- Owner: Test Lead + Network Lead
- Can run in parallel with all items

### T5-8: Dead Code Cleanup Round 2 (LOW / S)
- 33 #[allow(dead_code)] annotations remain across 10 files
- Each one represents either: (a) a function that should be deleted, or (b) a function
  that IS used and the annotation is suppressing a false positive that should be fixed properly
- Audit each one: delete truly unused code, expose properly via pub(crate) where needed
- Owner: Any Lead (small, parallel)
- Fully parallel

---

## Tranche 6 — Queued (Post-SPO-Readiness)

### Block Producer Full Validation (HIGH/L)
- Run Torsten as active block producer on preview testnet for 2 full epochs
- Validate: VRF leader check correct, KES rotation working, blocks accepted by network
- Depends on: T5-1 full sync, T5-6 cncli compat

### Plutus Script Witness CLI (HIGH/L)
- --tx-in-script-file, --redeemer-file, --datum-file, --tx-in-execution-units
- Depends on: T5-2 (node.rs split to reduce diff surface), T5-3 (validation.rs split)

### CDDL Conformance Vectors — Plutus (HIGH/M)
- Current: 58 vectors (utxo/cert/gov/epoch), no Plutus execution vectors
- Add 5 Plutus vectors: always-succeeds, always-fails, reference scripts, V3
- Real tx CBOR roundtrip tests via Koios MCP (one per era)

### N2N V16 Genesis Protocol (LOW/L)
- Full Ouroboros Genesis: BLP peer selection, GDD state, HAA
- Prerequisite: T5-6 (confirm standard V14/V15 is solid first)

### Mempool Revalidation on Epoch Boundary (MEDIUM/S)
- cardano-node drains+revalidates mempool at epoch boundary
- Torsten only clears on rollback — correctness gap for block producers
- ~30-line change in epoch_handler.rs (post-T5-2 split)

---

## Key Risk Flags (current)

- NonMyopicMemberRewards: formula implemented, ZERO E2E validation — HIGH risk
- Mainnet never fully synced: confidence in Byron→Shelley transition is limited to 115K blocks
- node.rs / validation.rs size: bugs hide in 5600+ line files; split is prerequisite for safe feature work
- WAL prev_hash: post-crash fork detection impaired until new blocks arrive
- cncli compat unknown: SPOs can't use Torsten without it regardless of other correctness
