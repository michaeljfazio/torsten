# Sub-project 1 — Shelley Reward Finalization

**Parent:** [Ledger Completion Decomposition](2026-04-11-ledger-completion-decomposition.md)
**Date:** 2026-04-11
**Closes:** `eras/shelley.rs:200`, `eras/shelley.rs:575`, `state/rewards.rs` `unwrap_or(0)` audit

---

## Problem

Two distinct symptoms, one root cause: the Shelley-era `BlockValidator::process_epoch_transition` implementation at `crates/dugite-ledger/src/eras/shelley.rs:198-210` intentionally skips RUPD (reward update calculation). The comment at line 200 explains:

> The full RUPD (reward calculation using GO snapshot + bprev + ss_fee) is not computed here because it requires `calculate_rewards_full` which operates on `&self LedgerState`. The orchestrator (Task 12) will handle wiring reward calculation before this point.

"Task 12" never landed. `calculate_rewards_full` already exists at `state/rewards.rs:172` — it's just not called from the Shelley era path. Concretely:

1. `EpochSubState::snapshots.rupd_ready` is flipped to `true` at SNAP time but the RUPD is never computed, so `pending_reward_update` is never set.
2. The deferred-application path at `shelley.rs:182-196` therefore only fires when the *previous* path populated `pending_reward_update`. In practice this means rewards are being taken from whatever legacy code runs before the era-rules trait routes through — not through the era rules path.
3. Byron→Shelley at `shelley.rs:575` silently returns `Ok(())` with a comment saying the real init is deferred to the orchestrator that never arrived. The genesis delegation certs from `ShelleyGenesis.initial_staking` are applied elsewhere (via the legacy init path), so for fresh syncs this mostly-works-by-accident, but the era-rules path is a lie.

The net effect today: rewards are (mostly) right on snapshots already imported from Mithril, but the epoch-transition path through the era-rules trait is incomplete, and this blocks a clean Ralph loop on sub-projects 2 and 3 (Conway governance needs correct `reward_accounts` + `treasury` + `reserves` bookkeeping from RUPD to validate).

## Goal

Wire RUPD into the Shelley-era transition path so that:

1. At every Shelley+ (Shelley, Allegra, Mary, Alonzo, Babbage, Conway) epoch boundary, the era's `process_epoch_transition` computes the `PendingRewardUpdate` using `go_snapshot`, `bprev`, and `ss_fee` from the current `EpochSubState`, stores it in `pending_reward_update`, and the next boundary's deferred-apply step credits reward accounts.
2. Byron→Shelley `on_era_transition` actually initializes the staking state from `ShelleyGenesis` (or is explicitly documented as a no-op because `LedgerState::new_from_shelley_genesis` already did it, with a test proving equivalence).
3. Every `unwrap_or(0)` / `unwrap_or(Lovelace(0))` in `state/rewards.rs` either has a comment justifying why missing data is correct, or is fixed to propagate the error.

## Non-goals

- Recomputing rewards from scratch — `calculate_rewards_full` is already correct (verified against cstreamer in previous work). This is a wiring task.
- Changing the snapshot mark/set/go rotation or the `rupd_ready` flag semantics. The rotation happens at the right point; only the RUPD computation is missing.
- Touching Conway-specific reward behavior (treasury withdrawals crediting reward accounts is sub-project 3).
- Pool-ranking / non-myopic reward exposed via queries (separate feature).

## Design

### 1. Wire RUPD into the Shelley era transition

**File:** `crates/dugite-ledger/src/eras/shelley.rs`

At `shelley.rs:198-210`, after the SNAP rotation but before returning, compute and store the pending reward update. The `calculate_rewards_full` function needs `&LedgerState`, which is not currently in scope of `process_epoch_transition`. Two options:

**Option A (chosen):** Change `process_epoch_transition`'s signature to accept `&LedgerState` as an additional borrow. The trait lives in `eras/mod.rs`; every era impl must be updated. The `LedgerState` reference is read-only (RUPD reads, never mutates ledger-state), so we add a `ledger: &LedgerState` parameter alongside the existing `&mut` sub-state borrows.

Rejected: **Option B** — move `calculate_rewards_full` to a free function that takes `(GO, bprev, ss_fee, prev_pparams, reserves)`. This is cleaner but touches ~300 LoC of reward math and risks regressions.

**Implementation sketch:**

```rust
// eras/shelley.rs, inside process_epoch_transition, after SNAP rotation.
// GO snapshot now holds the "2 epochs ago" distribution; bprev holds "1 epoch ago"
// blocks-made; ss_fee holds the "1 epoch ago" fee pot. These are exactly the three
// inputs to startStep (Haskell PulsingReward.hs).
if let Some(go) = epochs.snapshots.go.as_ref() {
    // The SET snapshot (rotated out in the line above) is the bprev source.
    // epochs.snapshots.set is now the NEW mark; the OLD set was moved to go.
    // But go already has epoch_blocks_by_pool carried through, since SNAP
    // moves the set into go including its blocks-made counts — see snapshot.rs.
    let rupd = ledger.calculate_rewards_full(
        go,
        go, // bprev snapshot; in Haskell this is nesBprev (prev epoch).
             // In our model, after rotation, `go` carries the blocks-made
             // that were accumulated during the epoch just ended.
        epochs.snapshots.ss_fee,
    );
    epochs.pending_reward_update = Some(rupd);
}
```

Verify during implementation that `go.epoch_blocks_by_pool` is populated correctly after SNAP rotation. If it isn't, we populate it from `bprev_blocks_by_pool` fields already being stored at `snapshots.bprev_blocks_by_pool` at line 209.

**Haskell reference:** `cardano-ledger/Shelley/Rules/NewEpoch.hs` `createRUpd` and `PulsingReward.hs` `startStep`. Cross-check with cardano-ledger-oracle agent during implementation.

### 2. Propagate the `&LedgerState` borrow through the era-rules trait

**File:** `crates/dugite-ledger/src/eras/mod.rs`

Add `ledger: &LedgerState` to the `BlockValidator::process_epoch_transition` trait method. Update the six era impls: `byron.rs`, `shelley.rs`, `alonzo.rs`, `babbage.rs`, `conway.rs` plus any test stubs.

The Byron impl discards the borrow (no staking state). The Shelley impl uses it for RUPD. Alonzo/Babbage/Conway delegate to the Shelley impl via `ShelleyRules::process_epoch_transition` — they just pass through.

Callers in `state/apply.rs` and `state/epoch.rs` need the `&LedgerState`. Check via grep; this is where the current "operates on `&self LedgerState`" obstacle actually bites. The caller is `LedgerState::apply_block`, which has `&mut self`; we split the borrow by taking the `LedgerState` reference before entering the sub-state borrows, or by restructuring to call `process_epoch_transition` at a point where only one borrow is live.

During implementation, if the borrow checker fights us, fall back to **Option B** above and punch the math out into a free function.

### 3. Byron→Shelley bootstrap

**File:** `crates/dugite-ledger/src/eras/shelley.rs:573-601`

Three possibilities for what this should do:

**a)** Fresh genesis sync: `LedgerState::new_from_shelley_genesis` already initializes `stake_distribution`, `delegations`, `pool_params` from `ShelleyGenesis.initial_staking`. In that case, the era-transition call is redundant — but it should explicitly *say so* and verify invariants, not silently `Ok(())`.

**b)** Byron-era Rust sync (not currently supported, but the era trait is called for it): the era transition would need to walk `initial_funds` + `initial_staking` from genesis and build the Shelley state.

**c)** Mithril import: state is already populated from the snapshot; era transition is a no-op.

Chosen behavior: treat this as a **verification point**. On Byron→Shelley, assert that `certs.stake_distribution` / `certs.pool_params` match what `ShelleyGenesis.initial_staking` prescribes, and if not, initialize them. This is cheap, correct, and makes the code self-documenting. Add a unit test that drives the era transition on a fresh state and verifies the post-transition substate matches `ShelleyGenesis`.

```rust
Era::Byron => {
    // If we came via Mithril or a fresh ShelleyGenesis load, the staking
    // state is already populated. Verify invariants, seed anything missing.
    let expected = ctx.shelley_genesis.initial_staking_as_stake_subset();
    if certs.stake_distribution.is_empty() && !expected.is_empty() {
        certs.apply_initial_staking(&expected);
    } else {
        // Sanity check in debug builds only.
        debug_assert_eq!(certs.pool_params.len(), expected.pool_count());
    }
    Ok(())
}
```

`initial_staking_as_stake_subset` and `apply_initial_staking` are new helpers on `ShelleyGenesis` and `CertSubState` respectively. ~40 LoC each.

### 4. `unwrap_or(0)` audit in `state/rewards.rs`

Five occurrences at lines 407, 427, 460, 503, and one in `governance.rs:1621,1627,1736,1896` (out of scope — belongs to sub-project 2/3). The rewards.rs ones:

| Line | Current | Justification |
|------|---------|---------------|
| 407 | `.unwrap_or(0)` on reward-address → pool lookup | Correct: stake credential may not be delegated. Add comment. |
| 427 | `.unwrap_or(0)` on member-stake lookup in pool | Suspicious: if a delegation points at a pool, the delegator must be in the stake map. **Fix:** propagate. |
| 460 | `.unwrap_or(0)` on owner-stake-by-pool | Correct: owner may not have any stake. Add comment. |
| 503 | `.unwrap_or(0)` on pool blocks | Correct: a pool may have produced zero blocks. Add comment. |

Each change is a one-liner plus a comment. Line 427 gets converted to a `match` that returns `PendingRewardUpdate::empty()` or bubbles a structured error, depending on whether the existing test suite relies on the silent-zero behavior (run tests first, then decide).

### 5. Validation

- **Unit test 1** — drive `process_epoch_transition` for two consecutive Shelley epochs with a synthetic `go_snapshot` + `ss_fee`, assert `pending_reward_update` is set after epoch N, then assert `reward_accounts` are credited after epoch N+1 with the exact values produced by `calculate_rewards_full`.
- **Unit test 2** — Byron→Shelley era transition on a state where `stake_distribution` is empty and `ShelleyGenesis.initial_staking` has 3 pools + 5 delegators. Assert post-state matches expected.
- **Property test** — for any random valid `(GO, bprev, ss_fee, pparams)`, the deferred-apply path preserves `reserves + treasury + sum(reward_accounts)` modulo monetary expansion (conservation check).
- **Golden fixture** — capture Haskell query outputs (`queryRewardProvenance`, `queryStakeDistribution`, `queryTip`) at preview epochs 220 and 221 (post-Conway), replay dugite, assert reward maps match field-for-field.
- **cstreamer diff** — at the epoch boundary crossing 220→221, dump dugite ledger state, compare against reference cstreamer dump. `accountState`, `esSnapshots`, and `rs` must match byte-for-byte.
- **Manual soak** — run dugite from Mithril import at epoch 215 for 8 hours, confirm every epoch boundary emits a non-None RUPD and reward accounts credit without drift against a Haskell peer.

## Risk / tradeoffs

- **Trait signature churn.** Adding `ledger: &LedgerState` to `process_epoch_transition` touches every era impl. Low risk — each impl is ~10 lines of signature update.
- **Borrow-checker pain in `apply_block`.** The caller currently holds mutable borrows on sub-states. If we can't get a second `&LedgerState` borrow simultaneously, we restructure to compute the RUPD *before* entering the substate borrows and pass it as a pre-computed `Option<PendingRewardUpdate>`. Estimated 30 extra LoC if needed.
- **`go.epoch_blocks_by_pool` may be stale.** If SNAP rotation doesn't carry blocks-made into the GO slot, the test suite will catch it. Falls back to using `epochs.snapshots.bprev_blocks_by_pool` (already stored, so always fresh).
- **Double-credit risk.** If the legacy init path also computes rewards, we'd double-credit. Verify during implementation by grepping for `calculate_rewards_full` callers — currently just `state/tests.rs` and `state/apply.rs` under the `cfg(test)` guard. Safe.

## Order of operations

1. Add a failing unit test (`test_shelley_rupd_wired_through_era_trait`) that drives two epochs and asserts `pending_reward_update.is_some()` after epoch 1 and `reward_accounts` credited after epoch 2. Fails today.
2. Update the `BlockValidator::process_epoch_transition` trait signature to include `ledger: &LedgerState`.
3. Update all era impls to compile with the new signature (Byron/Alonzo/Babbage/Conway pass-through, Shelley is the only one that uses it).
4. Wire RUPD in Shelley's impl.
5. Run the failing test; confirm pass.
6. Add Byron→Shelley bootstrap test; implement helpers.
7. Audit `unwrap_or(0)` in `rewards.rs`; fix line 427, comment the others.
8. Capture golden fixture; add golden-fixture test.
9. Manual soak.
10. Clippy + fmt + nextest; commit.

## Done when

- `rg -n 'TODO' crates/dugite-ledger/src/eras/shelley.rs` returns nothing.
- `rg -n '\.unwrap_or\(0\)' crates/dugite-ledger/src/state/rewards.rs` returns zero uncommented hits.
- New unit tests pass.
- Golden fixture test passes.
- cstreamer diff at epoch 220→221 is empty.
- 8-hour preview soak green.
