# Sub-project 3 — Conway Governance Ratification / Enactment Pipeline

**Parent:** [Ledger Completion Decomposition](2026-04-11-ledger-completion-decomposition.md)
**Date:** 2026-04-11
**Closes:** `eras/conway.rs:225,229,464,469,473,478,482,485,536,540`; `state/epoch.rs:458`
**Depends on:** Sub-projects 1 and 2

---

## Problem

The Conway-era `process_epoch_transition` at `crates/dugite-ledger/src/eras/conway.rs:275-583` has **10 TODOs** covering the governance pipeline. The in-file comment says "~600 lines in `state/governance.rs` will be wired when the orchestrator calls the existing code in Task 12." That orchestration never landed.

Critically, the governance logic itself already exists in `state/governance.rs`:

| Symbol | Lines | Status |
|--------|-------|--------|
| `LedgerState::ratify_proposals` | 634-~1100 | Implemented, has tests |
| `LedgerState::enact_gov_action` | 1980-~2200 | Implemented |
| `check_ratification` | 1138-~1400 | Implemented for all 7 action types |
| `build_drep_power_cache` / `build_drep_power_cache_live` | 1620-1800 | Implemented, falls back to live state |
| `test_enact_treasury_withdrawal_debits_treasury` | 4623 | Test proves enactment credits reward accounts |

What's **missing** is the orchestration: the `process_epoch_transition` path through the era-rules trait doesn't *call* any of this. So on a fresh sync, proposals accumulate in `gov_state.proposals`, votes are recorded, but nothing ever ratifies or enacts them.

Additionally:

- `state/epoch.rs:458` has a "simplified" Conway `pending_pp_updates` model that stores protocol-parameter updates under a flat key. Conway doesn't have the old PPUP; ParameterChange proposals live in `gov_state.proposals` and are enacted via ratification. The simplified model should be deleted.
- `eras/conway.rs:225,229` are two PV10-gated withdrawal checks — `validateWithdrawalsDelegated` and `testIncompleteAndMissingWithdrawals` — that are currently TODOs because preview hasn't crossed PV10 yet. They should still be implemented and gated on the current protocol version.

## Goal

At every Conway epoch boundary, the era-rules trait must produce ledger state identical (per fidelity bar C) to what Haskell's `NEWEPOCH` + `EPOCH` rules produce. Specifically:

1. **DRep pulser completion** — the mark-snapshot DRep voting power is computed and stored in `gov_state.drep_power_snapshot`. `build_drep_power_cache` takes the snapshot path.
2. **Ratification pass** — `ratify_proposals` runs against the frozen snapshot; ratified proposals are moved to an enacted set and/or a delayed set.
3. **Enactment** — each ratified proposal's `enact_gov_action` fires. Treasury withdrawals move ADA from `epochs.treasury` to `certs.reward_accounts`. ParameterChange updates `epochs.protocol_params`. HardForkInitiation bumps `protocol_version_major` on the next boundary (Haskell delays by one epoch). NoConfidence empties `gov_state.committee`. UpdateCommittee mutates membership/thresholds. NewConstitution swaps the anchor. InfoAction is record-only.
4. **Deposit returns** — for every enacted *or* expired proposal, `procedure.deposit` is credited to the proposer's `returnAddr` reward account. If the reward account is missing, the deposit goes to the treasury (matching Haskell).
5. **Expiry pruning** — proposals whose `expires_after_epoch <= current_epoch` are removed from `gov_state.proposals`, their deposits returned (step 4).
6. **Dormant-epoch counter** — if zero governance activity this epoch (no ratified, no expired, no newly submitted), `gov_state.dormant_epochs += 1`; otherwise reset to 0. Used for DRep expiry extension in the DRep inactivity rule.
7. **Hard-fork bump** — if the previous epoch enacted a `HardForkInitiation` with target version > current, bump `protocol_version_major` now (one-epoch delay matches Haskell).
8. **Pulser prep for next epoch** — seed `gov_state.drep_pulser_state` for the upcoming epoch using the new mark snapshot.
9. **PV10 withdrawal checks** (moved to Phase-1 validation) — gated on `pparams.protocol_version_major >= 10`.
10. **Delete the simplified `pending_pp_updates`** in `state/epoch.rs:458`.

## Non-goals

- Rewriting `ratify_proposals` or `enact_gov_action`. Those are the implementation; this spec is the wiring + the missing pieces around them.
- Incremental DRep pulsing (Haskell chunks pulser work across an epoch for DoS protection). Dugite computes it in one shot at the boundary; profile later if mainnet-scale soak shows it as a hotspot.
- Governance query wire-format changes. If queries need new fields (e.g., `drep_power_snapshot`), they're out of scope — add in a follow-up.

## Design

### 1. Orchestration point

**File:** `crates/dugite-ledger/src/eras/conway.rs:~280` inside `ConwayRules::process_epoch_transition`

Replace the 10-TODO block (lines 463-541) with a call to a new helper `apply_conway_governance_epoch(&mut self, ctx, new_epoch, ..substates..)` that lives either on `LedgerState` (if borrow rules permit) or as a free function in `state/governance.rs` that takes the substates explicitly.

Because `ratify_proposals` and `enact_gov_action` are methods on `LedgerState`, and the era trait operates on substates, we need to either:

**Option A (chosen):** Pass `&mut LedgerState` into `process_epoch_transition` (already being done for sub-project 1's RUPD). The Conway impl calls `ledger.apply_conway_governance_epoch(ctx, new_epoch)` which internally runs ratify/enact/return-deposits/expire/dormant-update. The substate borrows in the signature are either released before the call or the whole pipeline moves to work on `&mut LedgerState` and just reads back from it afterward.

**Option B:** Convert `ratify_proposals` and `enact_gov_action` to take `(&mut GovSubState, &mut UtxoSubState, &mut CertSubState, &EpochSubState, &ConsensusSubState)`. Cleaner signatures, but 600+ lines of changes to the existing tested code.

Option A is lower risk. Option B is cleaner. Pick A for this spec; revisit during implementation if the borrow checker forces B.

### 2. New `apply_conway_governance_epoch` method

**File:** `state/governance.rs`, new method on `LedgerState`

```rust
/// Conway EPOCH+NEWEPOCH governance sub-pipeline.
/// Called from ConwayRules::process_epoch_transition after snapshot rotation.
pub(crate) fn apply_conway_governance_epoch(&mut self, new_epoch: Epoch) {
    // Step 1: Complete the DRep pulser — freeze a snapshot of drep_power
    //         from the mark snapshot. Uses build_drep_power_cache_live and
    //         stores result in gov_state.drep_power_snapshot.
    let (drep_power, always_no_confidence, always_abstain) =
        self.build_drep_power_cache_live();
    let gov = Arc::make_mut(&mut self.gov.governance);
    gov.drep_power_snapshot = Some(drep_power);
    gov.always_no_confidence_power = always_no_confidence;
    gov.always_abstain_power = always_abstain;

    // Step 2: Ratify.
    //         ratify_proposals reads the frozen snapshot and returns
    //         a list of (action_id, RatificationOutcome) via internal state.
    self.ratify_proposals();

    // Step 3: Enact each ratified action.
    //         ratify_proposals already appends to gov.enacted_this_epoch;
    //         enact_gov_action applies the state changes.
    let enacted_ids: Vec<_> = gov.enacted_this_epoch.drain(..).collect();
    for id in &enacted_ids {
        if let Some(p) = gov.proposals.get(id).cloned() {
            self.enact_gov_action(&p.procedure.action);
        }
    }

    // Step 4: Return deposits for enacted + expired proposals.
    self.return_proposal_deposits(new_epoch, &enacted_ids);

    // Step 5: Expire & prune proposals where expires_after_epoch <= new_epoch.
    self.prune_expired_proposals(new_epoch);

    // Step 6: Dormant-epoch counter.
    let activity =
        !enacted_ids.is_empty() || gov.expired_this_epoch.drain(..).next().is_some();
    if !activity {
        gov.dormant_epochs = gov.dormant_epochs.saturating_add(1);
    } else {
        gov.dormant_epochs = 0;
    }

    // Step 7: Deferred hardfork bump (one-epoch delay).
    if let Some(pending_hf) = gov.pending_hardfork.take() {
        if pending_hf.target_version > self.epochs.protocol_params.protocol_version_major {
            self.epochs.protocol_params.protocol_version_major = pending_hf.target_version;
        }
    }

    // Step 8: Seed pulser for the next epoch using the new mark snapshot.
    //         (No-op if drep_power_snapshot was just populated in step 1;
    //         the next epoch will repeat step 1 from the rotated mark.)
}
```

New helpers `return_proposal_deposits`, `prune_expired_proposals`, fields `enacted_this_epoch`, `expired_this_epoch`, `dormant_epochs`, `pending_hardfork`, `always_no_confidence_power`, `always_abstain_power`, `drep_power_snapshot` — most of these already exist; the ones that don't are ~10 lines each.

### 3. `return_proposal_deposits` — new helper

Walk `enacted_ids` and `expired_ids`. For each, look up the `ProposalProcedure` in `gov.proposals`, read `procedure.return_addr` (a reward address), decode its stake credential, credit `procedure.deposit` to `certs.reward_accounts[cred]`. If the reward account doesn't exist in the map (i.e., was deregistered), credit treasury instead. Matches Haskell `Conway/Rules/Enact.hs::returnProposalDeposits`.

After crediting, **remove the proposal from `gov.proposals`** so it doesn't appear in subsequent ratification passes or gov queries.

### 4. `prune_expired_proposals` — new helper

```rust
fn prune_expired_proposals(&mut self, new_epoch: Epoch) {
    let gov = Arc::make_mut(&mut self.gov.governance);
    let to_expire: Vec<_> = gov.proposals.iter()
        .filter(|(_, p)| p.expires_after_epoch <= new_epoch)
        .map(|(id, _)| *id)
        .collect();
    for id in &to_expire {
        if let Some(p) = gov.proposals.remove(id) {
            self.refund_proposal_deposit(&p);
        }
    }
    gov.expired_this_epoch.extend(to_expire);
}
```

`refund_proposal_deposit` is the single-proposal variant of step 3's helper.

### 5. PV10 withdrawal checks

**File:** `crates/dugite-ledger/src/validation/phase1.rs` (not `eras/conway.rs`)

Move the TODO checks from `conway.rs:225-229` into Phase-1 validation where they belong:

```rust
// Rule: validateWithdrawalsDelegated (PV10+)
if pparams.protocol_version_major >= 10 {
    for (reward_addr, _amount) in &tx.body.withdrawals {
        let stake_cred = decode_stake_cred_from_reward_addr(reward_addr)?;
        if !gov.vote_delegations.contains_key(&stake_cred) {
            return Err(LedgerError::WithdrawalNotDelegatedToDRep { stake_cred });
        }
    }
}

// Rule: testIncompleteAndMissingWithdrawals (PV10+)
if pparams.protocol_version_major >= 10 {
    for (reward_addr, claimed) in &tx.body.withdrawals {
        let stake_cred = decode_stake_cred_from_reward_addr(reward_addr)?;
        let balance = certs.reward_accounts.get(&stake_cred).copied().unwrap_or(Lovelace(0));
        if claimed != &balance {
            return Err(LedgerError::WithdrawalNotFullBalance {
                stake_cred,
                claimed: *claimed,
                balance,
            });
        }
    }
}
```

**Haskell references:** `Conway/Rules/Deleg.hs::validateWithdrawalsDelegated`, `Conway/Rules/Utxow.hs::testIncompleteAndMissingWithdrawals`. Oracle-verify exact error shapes during implementation.

Remove the corresponding stub comments in `eras/conway.rs:220-228`.

### 6. Delete the simplified `pending_pp_updates`

**File:** `state/epoch.rs:450-465`

Remove the fields and the comment block. Any callers that currently write into `pending_pp_updates` are either:
- Shelley-to-Babbage PPUP handlers — keep them, they write their own separate field
- Conway callers — reroute to `gov_state.proposals` insertion via the normal submission path

Grep first, verify no test depends on the simplified model, delete.

### 7. Conway already has a committee-expiry step at line 488

Lines 488-511 already prune expired committee members. Leave it alone; it's already correct. Just re-order relative to the new orchestration so it runs *after* `apply_conway_governance_epoch` (enacted `UpdateCommittee` may have added new members with new expirations).

### 8. Validation

- **Unit test 1 — `test_ratify_parameter_change_end_to_end`**: submit a `ParameterChange` proposal, vote yes from enough DReps/SPOs, advance one epoch, assert `epochs.protocol_params` reflects the change.
- **Unit test 2 — `test_ratify_treasury_withdrawal`**: submit withdrawal, vote, advance, assert `epochs.treasury` decreased and target reward accounts credited.
- **Unit test 3 — `test_expire_proposal_returns_deposit`**: submit proposal with an expiration in the next epoch, don't vote, advance, assert proposal is gone and deposit is in the return address's reward account.
- **Unit test 4 — `test_dormant_epochs_counter`**: advance three epochs with zero governance activity, assert `dormant_epochs == 3`; submit a proposal, advance, assert counter resets to 0.
- **Unit test 5 — `test_hardfork_initiation_delayed_one_epoch`**: enact HardForkInitiation at epoch N, assert `protocol_version_major` unchanged at end of N, bumped at end of N+1.
- **Unit test 6 — `test_withdrawal_must_be_delegated_pv10`**: at PV9, undelegated withdrawal succeeds; at PV10, it fails with `WithdrawalNotDelegatedToDRep`.
- **Unit test 7 — `test_withdrawal_must_be_full_balance_pv10`**: at PV10, partial withdrawal fails with `WithdrawalNotFullBalance`.
- **Property test** — for any random valid set of proposals + votes, conservation holds: `reserves + treasury + sum(rewards) + sum(deposits)` constant across an epoch boundary (within the monetary-expansion delta).
- **Golden fixture** — preview epoch containing at least one enacted ParameterChange and one expired proposal. Capture Haskell query outputs for `queryGovState`, `queryConstitution`, `queryCommittee`, `queryAccountState` before and after. Replay dugite, assert field-for-field match.
- **cstreamer diff** — at the target epoch boundary, `epochState.esLState.lsUTxOState.utxosGovState` + `accountState` must match.
- **Manual soak** — on preview, submit three proposals (ParameterChange, TreasuryWithdrawal, InfoAction) via a helper script; vote from dugite and from a Haskell peer; assert both nodes report the same ratification outcome 3 epochs later.

## Risk / tradeoffs

- **`ratify_proposals` internal state coupling.** It mutates `self` while reading `self.epochs.snapshots.mark`. Implementation note: check whether the existing function already takes its snapshot via `ratification_snapshot.clone()`. If yes, we're fine. If no, we need to clone the snapshot at the call site to avoid aliased borrows.
- **Deposit-return idempotency.** If `apply_conway_governance_epoch` is called twice (restart mid-epoch), deposits must not double-return. Guard via `gov.apply_epoch != Some(new_epoch)` sentinel field; set at the end.
- **Conway HardForkInitiation delayed one epoch.** This is a Haskell behavior that's easy to get wrong (apply immediately vs. next epoch). `Conway/Rules/Enact.hs` is authoritative — oracle-check during implementation.
- **PV10 is not active on preview as of 2026-04-11.** The PV10 checks can be implemented but we can't live-validate them against a running Haskell node. Rely on unit tests + cstreamer synthetic fixtures.
- **`pending_pp_updates` removal may break non-Conway eras.** Grep first — it's used by Shelley/Allegra/Mary/Alonzo/Babbage PPUP. Only delete the *Conway* insertion path, leave the Shelley field intact. Re-read the line 458 comment; if it's Conway-specific, delete. Otherwise, narrow the delete.

## Order of operations

1. Add failing test `test_ratify_parameter_change_end_to_end`.
2. Add new fields (`enacted_this_epoch`, `expired_this_epoch`, `dormant_epochs`, `pending_hardfork`, `drep_power_snapshot`, `always_no_confidence_power`, `always_abstain_power`) to governance state.
3. Implement `apply_conway_governance_epoch` skeleton that delegates to existing `ratify_proposals` + `enact_gov_action`.
4. Implement `return_proposal_deposits` + `prune_expired_proposals` helpers.
5. Wire the call from `ConwayRules::process_epoch_transition`; delete the 10 TODOs at lines 463-541.
6. Confirm test 1 passes. Add tests 2-7.
7. Implement PV10 checks in `validation/phase1.rs`; delete stubs at `eras/conway.rs:220-228`.
8. Delete the simplified `pending_pp_updates` in `state/epoch.rs:458` (verify no non-Conway caller first).
9. Capture golden fixture for target preview epoch; add golden test.
10. Run preview soak; compare against Haskell peer.
11. Clippy + fmt + nextest.

## Done when

- `rg -n 'TODO' crates/dugite-ledger/src/eras/conway.rs` returns zero.
- `rg -n 'simplified' crates/dugite-ledger/src/state/epoch.rs` returns zero.
- All 7 unit tests pass.
- Property test passes.
- Golden fixture test passes.
- cstreamer diff at target epoch empty.
- 24-hour preview soak matches Haskell peer on governance queries.
