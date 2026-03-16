---
name: project_state_2026_03_16
description: State assessment as of 2026-03-16 — Tranche 6 (SPO Live Validation) planning, post-session-9 milestone summary
type: project
---

# Torsten State Assessment — 2026-03-16 (Tranche 6 Planning)

**Why:** Tracks completed milestones and open gaps for the tranche planning cycle.

**How to apply:** Use when asked about current state, gap analysis, or what to work on next.

---

## Session-9 Accomplishments (63 commits, v0.2.0-alpha + v0.3.0-alpha)

- Bug fixes: #80 budget swap, #81 fee size (is_valid byte), #82 ScriptDataHash (ref scripts), Plutus V1/V2 success criteria
- Upstream PR to aiken: https://github.com/aiken-lang/aiken/pull/1283
- Architectural review 10/11: module visibility, SAFETY comments, doc comments, dead code, BigInt overflow, Mutex poisoning (#83 deferred)
- cncli: all 8 commands compatible including full NewEpochState for snapshot
- TUI: 7 themes, adaptive layout, 54 tests
- CI: nightly benchmarks, code scanning, security policy
- Wiki: 8 ADRs, full protocol compliance, known issues
- Community: LICENSE, CoC, CONTRIBUTING, templates, discussions
- Block production integration tests: 5 forge tests passing
- Pool registered on preview with ~9,500 ADA stake, ~3.4 expected slots/epoch
- node.rs split: 5 modules (sync.rs, epoch.rs, query.rs, serve.rs, mod.rs)
- validation.rs split: phase1/, scripts.rs, conway.rs, collateral.rs, tests.rs
- N2C golden tests: 4 → 13 fixtures, 24 tests covering 8 query tags
- Mempool revalidation at epoch boundary: ALREADY IMPLEMENTED (sync.rs:952)
- T5-4 NonMyopic visibility: done (function exists in state_query.rs:1255)
- T5-5 WAL prev_hash: DONE (part of session-9 TUI commit)
- T5-7 golden tests: DONE (13 fixtures, 24 tests)
- T5-8 dead code: DONE (0 annotations remain)
- Peer sharing privacy (outbound-only) + N2N V16 Genesis version added
- First mainnet full sync complete

## What's Confirmed Complete vs What Remains

### DONE from Tranche 5:
- T5-2: node.rs split (5 modules, mod.rs at 2074L — still large, but split done)
- T5-3: validation.rs split (split complete)
- T5-4: NonMyopic implemented; visibility confirmed
- T5-5: WAL prev_hash upgrade done
- T5-6: cncli compatibility: all 8 commands compatible (NewEpochState in place)
- T5-7: N2C golden tests expanded (13 fixtures, 24 tests)
- T5-8: Dead code cleanup (0 annotations)

### OPEN from session-9 notes:
1. Block production live test — waiting for next epoch on preview (pool registered, ~3.4 slots/epoch)
2. #83 — Extract 7,840 test lines from state/tests.rs (all 200 tests in ONE file, no sub-modules)
3. Preview re-validation (agent still running as of 2026-03-16)
4. Long-duration stability test (48hr)
5. Multi-asset coin selection in transaction build (calculate_change is ADA-only)
6. Mainnet re-validation with all Plutus/VRF/fee fixes
7. Test coverage improvements (tech lead review)
8. Guild network pool registration + BP validation
9. Reward cross-validation against Koios ground truth (NonMyopicMemberRewards E2E)

## Key Architecture State

- 13 crates in workspace (added torsten-tui, torsten-integration-tests, torsten-golden-tests per Cargo.lock)
- state/tests.rs: 7,842 lines, 200 tests, all in one file — #83 extraction needed
- validation/tests.rs: 2,832 lines, 89 tests
- n2c/state_query.rs: 3,105 lines — next major split candidate
- mempool/lib.rs: 2,745 lines — self-contained, not urgent
- ImmutableDB: 1,690 lines, 35 tests

## Tranche 6 — SPO Live Validation (CURRENT TARGET)

Mission: Validate that an SPO can run Torsten as an active block producer on preview,
forge at least one block accepted by the network, and serve cardano-cli queries correctly.
This closes the loop from "it syncs" to "it produces".
