# Sub-project 2 — Conway Genesis Bootstrap

**Parent:** [Ledger Completion Decomposition](2026-04-11-ledger-completion-decomposition.md)
**Date:** 2026-04-11
**Closes:** `eras/conway.rs:689,696,699,702,706`, `state/governance.rs:1718` (DRep power cache fallback)
**Depends on:** Sub-project 1 (clean Shelley era-transition path)

---

## Problem

`eras/conway.rs:634-710` implements the Babbage→Conway `on_era_transition`. Steps 1 and 5 are implemented (pointer stake exclusion, donation pot reset). Steps 2, 3, 4, 6, 7 are TODOs:

| Step | What Haskell does | Current dugite state |
|------|-------------------|----------------------|
| 2 | Create initial `VState` from `ConwayGenesis.initialDReps` + `initialCommittee` + `constitution` | TODO — assumes `LedgerState::new_from_conway_genesis` already populated it |
| 3 | Build VRF-key-hash → pool-ID map | TODO — used by DRep pulser for SPO voting power |
| 4 | Create initial `ConwayGovState` | TODO — committee + constitution anchor |
| 6 | Recompute `InstantStake` dropping pointer-addressed UTxO | **Partially done** — step 1 clears `ptr_stake`, but the GO/SET/MARK snapshots may still carry pointer stake carried over from Babbage |
| 7 | Initial DRep pulser state | TODO |

Plus a silent gap at `state/governance.rs:1715-1719`: when the DRep power snapshot hasn't been populated, the code falls back to computing from live `vote_delegations`. That fallback is correct *behaviorally* but means every Conway epoch's ratification reads live state instead of a frozen mark-snapshot copy — which diverges from Haskell when proposals are submitted late in the epoch.

The Ralph-loop symptom: Conway ratification tests can't be written end-to-end because the initial state is incomplete. Governance proposals submit fine, votes count fine, but ratification needs a consistent frozen snapshot to operate on.

## Goal

At the Babbage→Conway boundary, produce a Conway ledger state that is indistinguishable from what Haskell's `translateToConwayLedgerState` produces from an equivalent Babbage input. Specifically:

1. `gov_state.committee` matches `ConwayGenesis.initialCommittee`.
2. `gov_state.constitution` matches `ConwayGenesis.constitution`.
3. `gov_state.dreps` contains every `initialDRep` with `deposit = 0`, `drep_anchor = None`, `drep_activity = 0`.
4. `consensus.vrf_to_pool` (new field) maps every registered pool's VRF vkey hash → pool ID.
5. `epochs.snapshots.mark` / `set` / `go` have their `stake_map` entries filtered to drop pointer-addressed stake — not just `ptr_stake` on the live state.
6. `gov_state.drep_pulser_state` (new) is seeded with the current mark snapshot's stake distribution and the initial DRep set.
7. The DRep power cache snapshot in `gov_state.drep_power_snapshot` is populated, so `build_drep_power_cache` takes the fast snapshot path on the next ratification.

## Non-goals

- Running the first ratification at the transition itself. Ratification only runs at epoch boundaries (sub-project 3).
- Loading `conway-genesis.json` from disk — that's already done in `dugite_node::config::load_conway_genesis`. This spec uses the already-loaded `ConwayGenesis` from `RuleContext`.
- Backfilling `gov_state` for a state that entered Conway via Mithril import. If Mithril state already has Conway fields, the transition is a verification/no-op pass.
- Changes to the pulser algorithm. It's a seed-and-store problem here; sub-project 3 uses the seeded pulser.

## Design

### 1. Plumb `ConwayGenesis` into `RuleContext`

**File:** `crates/dugite-ledger/src/rules/mod.rs` (or wherever `RuleContext` lives — grep confirms)

Add `conway_genesis: &'a ConwayGenesis` to `RuleContext`. `ConwayGenesis` already lives in `dugite-primitives`. Every caller that constructs `RuleContext` must supply it; today there's typically a single construction site in `LedgerState::apply_block`.

### 2. Implement Steps 2, 3, 4 (VState, VRF map, GovState)

**File:** `crates/dugite-ledger/src/eras/conway.rs:689-707`

Replace the five TODOs with:

```rust
// Step 2 — Initial VState (DRep registry + committee).
let cg = ctx.conway_genesis;
for (drep_cred, init) in &cg.initial_dreps {
    gov.governance.drep_state.insert(
        drep_cred.clone(),
        DRepState {
            deposit: Lovelace(0),
            anchor: init.anchor.clone(),
            expiry: ctx.current_epoch + Epoch(cg.drep_activity),
        },
    );
}
gov.governance.committee = Some(Committee {
    members: cg.committee.clone(),
    threshold: cg.committee_threshold.clone(),
});
gov.governance.constitution = cg.constitution.clone();

// Step 3 — VRF → pool map.
for (pool_id, pp) in &certs.pool_params {
    consensus.vrf_to_pool.insert(pp.vrf_keyhash, *pool_id);
}

// Step 4 — Initial ConwayGovState.
gov.governance.proposals.clear(); // Babbage has none; safety.
gov.governance.cur_pparams = gov.governance.cur_pparams.to_conway();
gov.governance.prev_pparams = gov.governance.prev_pparams.to_conway();
gov.governance.future_pparams = None;
```

**Haskell reference:** `cardano-ledger/Conway/Translation.hs` — `translateToConwayLedgerState` (via oracle during implementation).

### 3. Recompute `InstantStake` dropping pointers (Step 6, finish)

**File:** `crates/dugite-ledger/src/eras/conway.rs` inside the era transition

Step 1 (pointer exclusion at line ~670) clears `ptr_stake` but does not re-derive `stake_distribution.stake_map` from the UTxO set excluding pointer-addressed outputs. The snapshots (mark/set/go) carry stale pointer contributions.

Approach: walk the mark snapshot's `stake_map` and subtract any contributions that originated from pointer-addressed UTxO. The simplest correct implementation: rebuild the mark snapshot from the post-filter `certs.stake_distribution`. Because set and go carry data from prior epochs that already included pointer stake, we *don't* rewrite them — Haskell doesn't either. Only the mark is recomputed (per `translateToConwayLedgerState` which rebuilds `instantStake` from the live UTxO).

**New helper:** `EpochSubState::rebuild_mark_from_certs(&CertSubState)`. Drops the current mark, replaces with `certs.stake_distribution.clone()`. ~15 LoC.

### 4. Initial DRep pulser (Step 7)

**File:** `crates/dugite-ledger/src/eras/conway.rs` plus new fields in `state/governance.rs`

Add to `ConwayGovState`:

```rust
pub drep_pulser_state: Option<DRepPulserState>,
pub drep_power_snapshot: Option<HashMap<Hash32, u64>>,
```

`DRepPulserState` is a struct that holds `(mark_snapshot_epoch, accumulated_drep_power, accumulated_always_no_confidence, accumulated_always_abstain, next_cursor)`. For dugite, we do the non-incremental version: the pulser is fully computed at the epoch boundary and stored, not chunked over many blocks. This matches fidelity bar C (internal representation may differ).

Seed at transition:

```rust
gov.governance.drep_pulser_state = Some(DRepPulserState::seed(
    &epochs.snapshots.mark.as_ref().expect("mark must exist post-rotation"),
    &gov.governance.vote_delegations,
    &gov.governance.drep_state,
));
```

The live-fallback path in `build_drep_power_cache` (line 1718) becomes the *only* computation path but its result is cached into `drep_power_snapshot` at each epoch boundary. This closes the silent gap.

### 5. Populate `drep_power_snapshot` for existing states

**File:** `state/apply.rs` or wherever `LedgerState::new_from_*` constructors live

For fresh init from Conway genesis, seed `drep_power_snapshot` to an empty map (no DReps have delegated stake yet — stake distribution is in terms of stake credentials, not DRep credentials, and `vote_delegations` is empty until the first block).

For Mithril-imported states: snapshot may or may not contain `drep_power_snapshot`. Add a migration path in the snapshot loader: if the field is absent, call `build_drep_power_cache_live()` once after load and store the result.

### 6. Idempotency

The transition must be idempotent — calling it twice on the same state (e.g., replay after restart) must not double-insert DReps or reset the pulser. Guard with `if !gov.governance.conway_bootstrapped { ... }` and set the flag at the end. Add a boolean field `conway_bootstrapped: bool` to `GovSubState`.

### 7. Validation

- **Unit test 1** — `test_conway_transition_seeds_vstate`: run the transition on a state with no Conway fields; assert `gov_state.committee`, `gov_state.constitution`, `gov_state.drep_state`, `gov_state.drep_pulser_state`, `consensus.vrf_to_pool` are populated per `ConwayGenesis`.
- **Unit test 2** — `test_conway_transition_idempotent`: call the transition twice; assert second call is a no-op (same state hash).
- **Unit test 3** — `test_conway_transition_rebuilds_mark_without_pointers`: construct a Babbage state with pointer-addressed stake in the mark snapshot; run transition; assert mark's pointer contributions are gone and non-pointer contributions preserved.
- **Unit test 4** — `test_drep_power_snapshot_populated`: after transition + first mark rotation, assert `gov_state.drep_power_snapshot.is_some()` and `build_drep_power_cache` takes the fast path (verifiable by mocking the tracing output or adding a test-only counter).
- **Golden fixture** — the ledger-state query output at the exact slot of the Babbage→Conway transition on preview (epoch 97, slot 20908800). Capture from Haskell once, commit, assert dugite produces the same `gov_state` / `vstate` / `pool_distr` sections.
- **cstreamer diff** — at the transition slot, `epochState.esLState.lsUTxOState.utxosGovState` must be field-for-field equal.

## Risk / tradeoffs

- **`RuleContext` threading churn.** Every rule impl sees it. Low risk — adding a field is additive.
- **Conway pparams type cast.** `pparams.to_conway()` must exist; if it doesn't, implement it (trivial — Conway pparams is a superset). Verify during implementation.
- **`DRepPulserState` representation.** Haskell uses an incremental pulser to spread computation across the epoch for DoS protection. Dugite computes it in one shot at the epoch boundary. For preview/preprod scale this is fine; at mainnet scale (~thousands of DReps), the full compute is ~O(delegations) per epoch and well under 100ms. If profiling shows it's hot, incrementalize later.
- **Mithril-imported Conway state.** If the snapshot already has `gov_state.committee` populated but not `drep_power_snapshot`, the transition-idempotency guard (`conway_bootstrapped`) must recognize that case and still seed the missing fields. Use a "partial bootstrap" path: if `conway_bootstrapped == true` but `drep_power_snapshot.is_none()`, seed just the missing bits.

## Order of operations

1. Add failing unit test `test_conway_transition_seeds_vstate`.
2. Add `conway_genesis` to `RuleContext`; thread through all constructors.
3. Add `DRepPulserState`, `drep_power_snapshot`, `conway_bootstrapped` fields to governance state; default-empty.
4. Implement steps 2, 3, 4 in `on_era_transition`.
5. Implement step 6 finish (rebuild mark).
6. Implement step 7 (seed pulser).
7. Close the `state/governance.rs:1718` fallback — make it unreachable in normal operation by ensuring `drep_power_snapshot` is always `Some` post-bootstrap; leave the live path as a test-only fallback with a `cfg(test)` guard or a `warn!` in production.
8. Populate tests 2-4.
9. Capture golden fixture; add fixture test.
10. Clippy + fmt + nextest.

## Done when

- `rg -n 'TODO' crates/dugite-ledger/src/eras/conway.rs` — only TODOs remaining are in `process_epoch_transition` (sub-project 3 territory).
- `state/governance.rs:1718` no longer logs "snapshot not yet populated" during normal operation.
- Golden fixture for Babbage→Conway transition slot passes.
- cstreamer diff at transition slot empty.
- All unit tests pass, clippy clean.
