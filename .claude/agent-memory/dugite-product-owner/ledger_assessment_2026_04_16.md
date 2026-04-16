---
name: ledger_assessment_2026_04_16
description: Detailed ledger completeness assessment as of 2026-04-16, per-area percentage estimates
type: project
---

# Dugite Ledger Completeness Assessment (2026-04-16)

**Why:** User asked for a thorough gap analysis of the ledger crate.

**How to apply:** Use as ground truth for roadmap prioritization.

## Summary Completeness by Area

| Area | Estimate | Notes |
|------|----------|-------|
| Phase-1 validation | ~95% | Rules 1-14 all present; PV10 stubs are acceptably deferred |
| Phase-2 / Plutus | ~90% | V1/V2/V3 via uplc; cost model enforcement present; per-redeemer V3 unit check fixed |
| UTxO management | ~98% | LSM-backed store, incremental stake tracking, pointer exclusion |
| Multi-era support | ~85% | Byron/Shelley-Conway functional; era transition stubs in ConwayRules::on_era_transition |
| Staking/delegation | ~95% | Registration, delegation, deregistration, pointer resolution all implemented |
| Rewards/RUPD | ~90% | BigRat arithmetic, correct timing (deferred RUPD), snapshot rotation; no historical cross-validation |
| Governance (Conway) | ~75% | Proposal submission, voting, ratification pipeline, enactment all implemented in state/governance.rs; ConwayRules epoch transition has TODOs 3-8 not wired to ratification code |
| Protocol param updates | ~95% | Pre-Conway PPUP and Conway ParameterChange both implemented |
| Certificate processing | ~97% | All cert types including all Conway combined certs |
| Epoch transitions | ~90% | SNAP/POOLREAP/nonce done; ConwayRules epoch transition missing RUPD wiring |
| MIR transfers | ~100% | Both StakeCredentials and OtherAccountingPot paths implemented |
| Genesis bootstrap | ~70% | ConwayRules::on_era_transition has TODO steps 2-4, 6-7 |

## Key Gaps

### Critical Architecture Gap: Two Parallel Epoch Transition Paths
- LedgerState::process_epoch_transition (old path): fully wired with RUPD, ratification, DRep activity
- ConwayRules::process_epoch_transition (new EraRules path, lines 301-598 conway.rs): TODOs for steps 3-8 (DRep pulser, treasury withdrawals, enactment, proposal expiry, dormant epochs, hardfork check, fresh DRep state)
- Current production code still uses the OLD path (LedgerState::process_epoch_transition)
- The EraRules path is not yet wired into the apply_block pipeline; Task 12 mentioned but not landed

### PV10 Stubs (Minor)
- validateWithdrawalsDelegated not implemented (not yet needed, PV10 not active)
- testIncompleteAndMissingWithdrawals not implemented (same)

### Conway Era Transition Steps (Moderate)
- on_era_transition() implements steps 1 (pointer exclusion) and 5 (reset donations)
- Steps 2-4 (VState from genesis, VRF map, initial GovState) and 6-7 (InstantStake recompute, DRep pulser) have TODO comments
- The missing steps are covered by the existing LedgerState initialization code in practice

### Reward Cross-Validation (Known, HIGH)
- No test validates per-epoch reward amounts against Koios historical data
- Formula verified for Rat arithmetic primitives but not end-to-end for real epoch values

### validate_shelley_base Stub (Low)
- common.rs line 678: validate_shelley_base() is a documented stub
- Underlying validation in validation/mod.rs and phase1.rs is called separately
- No functional gap, just refactoring debt
