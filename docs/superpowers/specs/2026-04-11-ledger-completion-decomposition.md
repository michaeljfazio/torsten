# Ledger Completion — Decomposition

**Date:** 2026-04-11
**Goal:** Drive `crates/dugite-ledger` from "partially implemented" to "100% complete, no gaps"
**Fidelity bar:** Option C — bit-exact for on-chain effects (UTxO, rewards, deposits, ratification outcomes, query results) but internal bookkeeping may differ from Haskell layouts
**Implementation style:** Pure dugite code. No new pallas integrations (pallas continues to supply CBOR and primitives only).
**Validation:** CI golden fixtures (committed Haskell query outputs) + cstreamer ledger-state dump cross-validation + manual cross-node soak before each sub-project is declared done.

---

## Why this is decomposed

A single spec for "finish the ledger" would be ~2000 lines, span four independent subsystems, and take days to execute as one plan. Each sub-project below is a realistic single implementation plan: small enough to hold in context, large enough to leave the tree in a better state at completion, and bounded so Ralph-loop iterations fit.

After each sub-project completes, the ledger is *more correct* than before in a way that can be validated independently. Sub-projects 4 and 5 have no dependencies on 1-3 and can be done in parallel once 1-3 are in flight.

---

## Scope — the surface area being closed

Found via `rg TODO|FIXME` plus a scan for silent gaps (stub returns, "for now", "simplified", `unwrap_or(0)`, empty match arms).

### Explicit TODOs (20)

| File | Line | Item |
|------|------|------|
| `state/apply.rs` | 193 | Full block-body hash equality (issue #377) |
| `eras/common.rs` | 712 | Extract Phase-1 rules 1-10 into shared `validate_shelley_base` |
| `eras/shelley.rs` | 200 | RUPD — reward calculation using GO snapshot + `bprev` + `ss_fee` |
| `eras/shelley.rs` | 575 | Byron→Shelley staking-state genesis initialization |
| `eras/conway.rs` | 225 | PV10 `validateWithdrawalsDelegated` |
| `eras/conway.rs` | 229 | PV10 `testIncompleteAndMissingWithdrawals` |
| `eras/conway.rs` | 281 | Epoch transition is stubbed with TODOs (meta) |
| `eras/conway.rs` | 464 | DRep pulser voting-power calculation from mark snapshot |
| `eras/conway.rs` | 469 | Enact TreasuryWithdrawals |
| `eras/conway.rs` | 473 | Full governance ratification/enactment pipeline (~600 LoC) |
| `eras/conway.rs` | 478 | Return proposal deposits for enacted/expired actions |
| `eras/conway.rs` | 482 | Advance proposal expiry tracking & prune |
| `eras/conway.rs` | 485 | DRep inactivity (dormant-epoch) tracking |
| `eras/conway.rs` | 536 | HardForkInitiation target-version bump |
| `eras/conway.rs` | 540 | Pulser prep for next epoch |
| `eras/conway.rs` | 689 | Initial VState from ConwayGenesis |
| `eras/conway.rs` | 696 | Initial VRF key hash → pool ID map |
| `eras/conway.rs` | 699 | Initial ConwayGovState |
| `eras/conway.rs` | 702 | Recompute InstantStake without pointer addresses |
| `eras/conway.rs` | 706 | Initial DRep pulser state |

### Silent gaps (unmarked)

| File | Line(s) | Issue |
|------|--------|-------|
| `state/certificates.rs` | 487 | `GenesisKeyDelegation` only logs, no state mutation |
| `state/certificates.rs` | 1323 | `reward_account: vec![0xe0u8; 29]` placeholder |
| `state/epoch.rs` | 458 | "Simplified" Conway `pending_pp_updates` model |
| `state/snapshot.rs` | 232 | Pre-v12 tracking approximation |
| `state/governance.rs` | 1718 | DRep power cache falls back to live `vote_delegations` when snapshot not populated |
| `ledger_seq.rs` | 1103 | "Snapshot helpers (stubs for Task 1.4/1.5)" |
| `eras/byron.rs` | — | `on_era_transition` returns `Ok(())` silently |
| `eras/common.rs` | 665-702 | `validate_shelley_base` declared but empty; callers still go through `validation/mod.rs` |
| `eras/alonzo.rs` | 212 | Alonzo witness logic "matches Shelley's witness logic for now" — no Plutus-specific witness rules |
| `eras/alonzo.rs` | 549 | `validate_block_body` unconditional `Ok(())` — no ExUnits budget check |
| `eras/babbage.rs` | 52 | Babbage script-size limits return `Ok(())` |
| `eras/babbage.rs` | 547 | Same ExUnits-budget gap as Alonzo |
| `eras/conway.rs` | 76, 180-186 | Ref-script-size & PV10 stubs in `validate_block_body` / `validate_tx` |
| `eras/conway.rs` | 1522 | Same ExUnits-budget gap |

---

## Decomposition

### Sub-project 1 — Shelley reward finalization
**Spec:** `2026-04-11-ledger-1-shelley-reward-finalization-design.md`
**Closes:** `eras/shelley.rs:200`, `eras/shelley.rs:575`, `rewards.rs` `unwrap_or(0)` audit

Implements RUPD (randomness-update-reward) — the Shelley reward calculation that consumes the GO snapshot, previous-epoch blocks made (`bprev`), and fee pot (`ss_fee`) to produce per-member rewards. Also fills in Byron→Shelley bootstrap of `StakeState` / snapshots so genesis delegation certificates from `ShelleyGenesis.initial_funds` and `initial_staking` create a non-empty initial stake distribution.

Gates: golden-file rewards for Shelley-era epoch boundary on preview, cstreamer dump equivalence for `accountState`, `esSnapshots`, and first post-epoch `rs` reward map.

### Sub-project 2 — Conway genesis bootstrap
**Spec:** `2026-04-11-ledger-2-conway-genesis-bootstrap-design.md`
**Closes:** `eras/conway.rs:689,696,699,702,706` plus `state/governance.rs:1718` (DRep power cache fallback)

Builds the initial Conway state at the Babbage→Conway era boundary: creates `VState` with DReps and committee populated from `ConwayGenesis`, constructs the VRF→pool map, builds initial `ConwayGovState`, recomputes `InstantStake` without pointer addresses (dropped in Conway), and seeds the DRep pulser. Also populates the DRep power cache snapshot so the governance queries stop falling back to live `vote_delegations`.

Gates: golden fixture for the exact state at the Babbage→Conway boundary on preview, cstreamer dump equivalence for `utxoState`, `govState`, `vstate`, `poolDistr` post-transition.

### Sub-project 3 — Conway governance ratification/enactment pipeline
**Spec:** `2026-04-11-ledger-3-conway-governance-pipeline-design.md`
**Closes:** `eras/conway.rs:464,469,473,478,482,485,536,540` plus `eras/conway.rs:225,229` (PV10 checks) and `state/epoch.rs:458` (simplified `pending_pp_updates`)

The big one. Implements the full Conway `EPOCH`/`NEWEPOCH` governance sub-pipeline:

1. **DRep pulser voting-power** — from mark snapshot's `stake_distr` + DRep delegations, compute the stake each DRep speaks for. Match Haskell's `DRepPulsingState` outputs (the *effects*, not the incremental chunks).
2. **Ratification** — for each proposal in priority order (in Haskell: `HardForkInitiation`, `NoConfidence`, `UpdateCommittee`, `NewConstitution`, `ParameterChange`, `TreasuryWithdrawals`, `InfoAction`), compute yes-ratio/no-ratio for each voter role (DRep, SPO, CC) against the threshold matrix defined in the Conway PParams and determine whether the action ratifies.
3. **Enactment** — apply ratified actions to state. `TreasuryWithdrawals` transfers from treasury to reward accounts, `ParameterChange` mutates PParams, `HardForkInitiation` bumps `protocol_version`, `NoConfidence` empties the committee, `UpdateCommittee` mutates committee membership/thresholds, `NewConstitution` updates the constitution anchor, `InfoAction` does nothing (record-only).
4. **Deposit returns** — return the `govActionDeposit` to `returnAddr` for enacted *and* expired proposals.
5. **Expiry pruning** — advance `current_epoch` in `gov_state`, prune proposals whose `expiresAfter` ≤ current epoch.
6. **Dormant-epoch tracking** — if no governance activity this epoch, increment `drep_activity.dormant_epochs`; otherwise reset. Used by DRep inactivity rule.
7. **Pulser prep** — stage the pulser for the upcoming epoch using the new mark snapshot and current DRep set.
8. **PV10 withdrawal checks** — `validateWithdrawalsDelegated` (every withdrawal's reward account must be delegated to a DRep) and `testIncompleteAndMissingWithdrawals` (can't withdraw more than balance, must withdraw full balance if any).
9. **Conway PParamUpdate model** — replace the "simplified" `pending_pp_updates` in `state/epoch.rs:458` with the correct Conway model: proposals are stored in `gov_state.proposals`, never keyed by protocol param group, and enacted only via ratification.

Gates: golden fixtures for a preview epoch containing at least one enacted `ParameterChange`, one enacted `TreasuryWithdrawals`, and one expired proposal; cstreamer dump equivalence for `govState.proposals`, `govState.committee`, `accountState.treasury`, `accountState.reserves`, and affected reward accounts. Manual soak: run full Conway governance life-cycle on preview and verify DRep/SPO/CC voting outcomes match a Haskell node peer-for-peer.

### Sub-project 4 — Block-body & witness completion
**Spec:** `2026-04-11-ledger-4-block-body-witness-completion-design.md`
**Closes:** `state/apply.rs:193` (#377), `eras/common.rs:712`, `eras/alonzo.rs:212,549`, `eras/babbage.rs:52,547`, `eras/conway.rs:76,180-186,1522`

Finishes the per-era `BlockValidator` / `TxValidator` surface that was deferred when the era-rules trait was introduced:

1. **Block body size/hash equality** — compute actual serialized body size via CBOR re-encoding and compare to `block.header.body_size`. Issue #377's full fix. Returns `LedgerError::WrongBlockBodySize`.
2. **Extract Phase-1 rules 1-10** — move common Shelley+ checks from `validation/mod.rs` into `eras/common::validate_shelley_base` and have Shelley/Allegra/Mary/Alonzo/Babbage/Conway era impls call it. Closes the empty stub at `eras/common.rs:665-702`.
3. **Block-level ExUnits budget** — for Alonzo/Babbage/Conway, sum each tx's redeemer ExUnits and verify `sum ≤ pparams.max_block_ex_units`. Returns `LedgerError::BlockExUnitsExceeded`.
4. **Babbage script-size limits** — enforce `pparams.max_script_size` per tx (Babbage introduced reference scripts; script sizes are inspected).
5. **Conway ref-script-size** — sum reference-script bytes across the block body and verify against `max_ref_script_size_per_block`. `BodyRefScriptsSizeTooBig`.
6. **Alonzo Plutus witness rules** — finish Alonzo's witness validation: datum hashes, redeemer presence, required signers. Conway and Babbage already delegate; Alonzo's "for now, matches Shelley" line is what needs to go.

Gates: property tests for each rule's boundary (tx at max ExUnits, block at max ref-script size, etc.), plus a golden block that would have been accepted previously but should now be rejected.

### Sub-project 5 — Silent gaps cleanup
**Spec:** `2026-04-11-ledger-5-silent-gaps-cleanup-design.md`
**Closes:** `state/certificates.rs:487,1323`, `state/snapshot.rs:232`, `ledger_seq.rs:1103`, `eras/byron.rs on_era_transition`

Small, independent fixes:

1. **GenesisKeyDelegation** — apply delegation to `gov_state.future_genesis_delegs` (Shelley-era only; still valid until Conway removes the concept).
2. **Byron→Shelley `on_era_transition`** — remove the silent `Ok(())`. Byron has no staking state, so the call is correct-but-undocumented; add an explicit comment plus any config-driven init needed from `ShelleyGenesis.genDelegs`.
3. **`certificates.rs:1323` placeholder** — build a real reward address from the cert's stake credential.
4. **`snapshot.rs:232` pre-v12 approximation** — remove the approximation path; we own the data format, so `version >= 12` is always true for fresh snapshots.
5. **`ledger_seq.rs:1103` "stubs for Task 1.4/1.5"** — implement or delete, depending on whether `ledger_seq` is still the plan of record (it is — used by the LedgerDB sequence tests).

Gates: each item has a unit test. No golden-fixture work needed.

---

## Dependency order

```
1. Shelley reward finalization ──┐
                                 │
2. Conway genesis bootstrap ─────┼─► 3. Conway governance pipeline
                                 │
4. Block-body & witness ─────────┤   (independent of 1-3)
5. Silent gaps cleanup ──────────┘   (independent of 1-3)
```

1 ships first because correct epoch rewards are a prerequisite for meaningful governance testing (reward accounts receive treasury-withdrawal targets, and deposit-refund bookkeeping interacts with the reward pot). 2 ships next because ratification has nothing to ratify unless genesis seeded the initial DReps/committee. 3 can only start after 2. 4 and 5 have no shared state with 1-3 and can run in parallel.

## Not in scope

- Consensus-layer changes (Praos, VRF, KES, chain selection) — the ledger must work with the existing consensus.
- Network layer changes (N2N, N2C) — ledger query handlers that expose new fields may be updated, but no new mini-protocols.
- Pallas version bumps. We stay on the currently pinned version.
- `validate_shelley_base` refactor beyond extracting rules 1-10. Rules 11+ stay in their current home.
- Perfect Haskell-equivalent internal representations (see fidelity bar C).
- Mainnet soak. Preview/preprod only for now.

## Validation strategy (all sub-projects)

1. **Unit tests** — per-function, inline in the appropriate `tests.rs`. Target ≥ 1 passing and ≥ 1 failing case per new rule.
2. **Property tests** — reuse the `proptest` infrastructure from `docs/superpowers/specs/2026-04-06-proptest-expansion-design.md` where applicable.
3. **Golden fixtures** — committed JSON snapshots of Haskell query outputs at specific preview slots. New fixtures generated by running the live Haskell relay (`config/haskell-relay-*`) and capturing via `dugite-cli` → `cardano-cli` comparison.
4. **cstreamer dump cross-validation** — at each epoch boundary touched by a sub-project, dump the dugite ledger state via the existing `dugite-node dump-state` command and compare against the reference cstreamer dump for the same slot. Any field-level mismatch is a regression.
5. **Manual soak** — 24-hour sync from genesis on preview; zero divergent blocks; `query tip`/`query gov-state`/`query stake-distribution` match the Haskell peer.

## Success criteria

- `rg -n 'TODO|FIXME|todo!\(|unimplemented!\(' crates/dugite-ledger/src` returns **zero hits** (tests excluded).
- No function in the ledger crate returns a hard-coded placeholder value without a comment explaining why it is correct.
- `cargo nextest run --workspace` passes with no ignored ledger tests.
- `cargo clippy --all-targets -- -D warnings` clean.
- 24-hour preview soak green against a Haskell peer.
- cstreamer dump diff is empty at sampled epoch boundaries.

---

## Links

- [Sub-project 1: Shelley reward finalization](2026-04-11-ledger-1-shelley-reward-finalization-design.md)
- [Sub-project 2: Conway genesis bootstrap](2026-04-11-ledger-2-conway-genesis-bootstrap-design.md)
- [Sub-project 3: Conway governance pipeline](2026-04-11-ledger-3-conway-governance-pipeline-design.md)
- [Sub-project 4: Block-body & witness completion](2026-04-11-ledger-4-block-body-witness-completion-design.md)
- [Sub-project 5: Silent gaps cleanup](2026-04-11-ledger-5-silent-gaps-cleanup-design.md)
