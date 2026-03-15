---
name: project_state_2026_03_15
description: State assessment as of 2026-03-15, after torsten-lsm + RUPD + PageOverflow milestones
type: project
---

# Torsten State Assessment — 2026-03-15 (Updated)

**Why:** Tracks completed milestones and open gaps after the torsten-lsm and RUPD alignment work.

**How to apply:** Use when asked about current state, gap analysis, or what to work on next.

## Milestones Completed (as of 2026-03-15)

### torsten-lsm (custom LSM engine)
- Replaced cardano-lsm with in-house pure Rust LSM engine (~4,500 lines)
- WAL + crash recovery, lazy levelling compaction (T=4), blocked bloom filters, LRU block cache
- Stress-tested against preview testnet: 2,938,963 UTxOs correct
- 1,830+ tests pass, zero clippy warnings

### RUPD Timing Alignment
- Rewards now deferred by 1 epoch matching Haskell's RUPD (PendingRewardUpdate)
- calculate_rewards() stores result; apply_pending_reward_update() applies it at NEXT boundary
- Code confirmed correct in epoch.rs: step 1 applies pending, step 3 computes new pending
- **Reward formulas confirmed correct vs Koios epoch 1235 data**
- **This change has NOT been re-validated on testnet yet** (run #9 was the prior code)

### PageOverflow Fix
- Large Cardano values (13KB+ inline datums) now handled via jumbo pages in torsten-lsm

### Flaky Test Fix
- test_cleanup_old_logs_removes_expired is now deterministic

## Key Risk: RUPD Requires Testnet Re-validation
The RUPD timing change alters when rewards land in reward accounts. This affects:
- GetRewardAccountBalance LocalStateQuery responses (tag 7)
- Snapshot content at epoch boundaries (mark/set/go)
- Treasury and reserve accounting per epoch

Run #9 validation did NOT trigger the two carried-over bugs from run #7:
- ScriptDataHash ignoring reference scripts (likely already fixed at line 940)
- CollateralHasTokens with collateral_return (net calc appears correct)

## Highest-Impact Open Gap: Reward Cross-Validation
No end-to-end test validates actual per-pool reward output against historical Koios data.
Formula tests only cover primitives (Rat arithmetic, maxPool unit tests with synthetic params).
Koios MCP is available on preview network — use koios_pool_history + koios_epoch_params.

## Other Open Gaps (in priority order)
1. Reward cross-validation with Koios historical data (HIGH — could be silently wrong)
2. Testnet re-run after RUPD change to confirm 100% sync with no errors (HIGH)
3. torsten-lsm stress test with mainnet scale (20M+ UTxOs) — preview coverage is ~3M
4. Byron ledger validation (MEDIUM — only needed for genesis-from-Byron mainnet sync)
5. Ouroboros Genesis LoE wiring (MEDIUM — not default mode)
