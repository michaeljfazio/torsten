---
name: drep-count-phantom-entries
description: Root cause and fix for issue #173 — DRep count 8791 vs Koios 1569
type: project
---

# Issue #173: DRep Count Phantom Entries

**Why:** Dugite reported 8,791 DReps vs Koios ~1,569–2,975 because the Prometheus metric
and N2C query snapshot were using `governance.dreps.len()` — the total size of the
registered-DRep HashMap — rather than the count of *active* DReps.

## Root Cause

The `dreps` HashMap contains ALL currently-registered DReps (inserted by `RegDRep`,
removed only by `UnregDRep`). At each epoch boundary, DReps that haven't voted or
updated within `drep_activity` epochs are marked `active = false` but are **NOT
removed** from the map — this matches Haskell's `vsDReps` semantics (inactive DReps
retain their deposit and can reactivate).

Koios and other tooling report only *active* DReps (those still within their activity
window), so `dreps.len()` always over-counts vs Koios.

## Fix (PR for issue #173)

1. Added `GovernanceState::active_drep_count()` helper that returns
   `dreps.values().filter(|d| d.active).count()`.

2. Changed Prometheus metric in `sync.rs` to use `active_drep_count()`.

3. Changed N2C query snapshot `drep_count` field in `query.rs` to use
   `active_drep_count()`.

4. Added 7 regression tests in `state/tests.rs`:
   - `test_vote_delegation_keyhash_does_not_create_drep_entry`
   - `test_vote_reg_deleg_does_not_create_drep_entry`
   - `test_reg_stake_vote_deleg_does_not_create_drep_entry`
   - `test_stake_vote_delegation_does_not_create_drep_entry`
   - `test_active_drep_count_excludes_inactive`
   - `test_epoch_transition_marks_inactive_drep`
   - `test_unreg_drep_removes_from_registry_and_active_count`

## Key Invariants Confirmed

- `VoteDelegation`, `VoteRegDeleg`, `RegStakeVoteDeleg`, `StakeVoteDelegation` with
  `DRep::KeyHash` do NOT create entries in `dreps` — only `RegDRep` does.
- `UnregDRep` removes entries — verified by test.
- Inactive DReps remain in `dreps` with `active=false` (they can reactivate via voting).
- The `drep_registration_count` field is a monotonic counter of all RegDRep certs ever
  processed — not the same as `dreps.len()`.

**How to apply:** When reporting DRep counts to users or external tools, always use
`governance.active_drep_count()`, not `governance.dreps.len()`. When doing internal
ledger logic (voting power, ratification), use the `active` flag on each DRepRegistration.
