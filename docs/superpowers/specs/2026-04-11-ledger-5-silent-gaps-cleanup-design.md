# Sub-project 5 — Silent Gaps Cleanup

**Parent:** [Ledger Completion Decomposition](2026-04-11-ledger-completion-decomposition.md)
**Date:** 2026-04-11
**Closes:** `state/certificates.rs:487`; `state/certificates.rs:1323` (comment fix); `state/snapshot.rs:232` (comment fix); `ledger_seq.rs:1103`; `eras/byron.rs` Byron→Shelley transition; `validate_shelley_base` table entry
**Depends on:** Nothing — independent of sub-projects 1-3

---

## Problem

Five small unmarked gaps that the TODO scan surfaced. Each is self-contained, low-risk, and doesn't belong in any other sub-project. They're grouped here to avoid dribbling them across unrelated PRs.

### Gap A — `GenesisKeyDelegation` certificate has no state mutation

**File:** `state/certificates.rs:487-505`

The match arm for `Certificate::GenesisKeyDelegation` only emits a debug log. No state is mutated. Haskell's `Shelley/Rules/Delegs.hs` applies genesis key delegations to `FutureGenDelegs` — a queued mapping that activates one stability window later, used by the old Shelley-era genesis-key-operated hardforks.

This matters only pre-Conway. Conway removed genesis delegation entirely. Preview is already Conway, so the cert type is grandfathered and never appears. **But** a full sync from Byron replay (or historical block serving) will hit Shelley blocks that contain these certs. Silent no-op today means dugite's Shelley-era state will diverge from Haskell on those blocks.

### Gap B — Placeholder reward address in test helper

**File:** `state/certificates.rs:1323`

The line `reward_account: vec![0xe0u8; 29], // key-type reward address placeholder` is inside `#[cfg(test)]` scope (verified by context: `make_state()` at line 1331 is a test helper). This is **not a production gap** — it's a test fixture. The comment misled the TODO scan.

Action: add a line comment clarifying it's a test fixture, not a TODO. Zero runtime impact.

### Gap C — Pre-v12 snapshot migration approximation

**File:** `state/snapshot.rs:225-244`

The comment at line 232 admits an approximation: when loading a snapshot version < 12, populate `stake_key_deposits` from the *current* `key_deposit` protocol parameter, which is wrong if `key_deposit` ever changed via governance. Reality check:

- Mainnet: `key_deposit` has never changed since Shelley (2 ADA).
- Preview/preprod: unchanged.
- Conway: governance *could* change it but hasn't.

So the approximation is currently correct *by circumstance*. The proper fix is to load per-credential deposits from the snapshot itself — but v < 12 snapshots don't have that data, so we literally can't do better. Action: upgrade the comment to document the invariant ("correct iff key_deposit has never changed; cardano-node's own snapshot format carries per-cred deposits from v12 onward, so this migration path is only hit for explicitly old snapshots we still support for testing") and add a debug-log that warns on load.

Zero behavioral change. Documentation fix only.

### Gap D — `ledger_seq.rs` "stubs for Task 1.4 / 1.5" section header

**File:** `ledger_seq.rs:1102-1104`

This is a *comment section header* that refers to an obsolete task numbering scheme. The code below the header is actually implemented (the `LedgerSeqError` enum, `rollback_to_slot`, etc.). The stub reference is stale.

Action: delete the "(stubs for Task 1.4 / 1.5)" parenthetical; rename the section to "Helpers and errors". Zero code change.

### Gap E — Byron→Shelley `on_era_transition` silent `Ok(())`

**File:** `eras/byron.rs` (the byron era `process_epoch_transition` / `on_era_transition`)

Byron has no staking state, so there's literally nothing to do at the era boundary going *into* Shelley. The receiving side (sub-project 1's new Shelley impl of `on_era_transition`) now handles the initialization. Byron's side should be explicit: return `Ok(())` with a doc comment that points at `eras/shelley.rs::on_era_transition` for the actual work. Zero behavioral change, just a comment.

### Gap F — `validate_shelley_base` table entry

**File:** `eras/common.rs:24`

The module-level table at line 24 lists `validate_shelley_base` as `(stub)`. Sub-project 4 deletes the stub function; update the table to remove the row, not leave a stale reference.

## Goal

Fix the real gap (A — GenesisKeyDelegation) and clean up the four documentation/comment artifacts (B, C, D, E, F) that pollute the TODO surface.

## Non-goals

- Implementing pre-v12 snapshot migration correctly. We can't — the old snapshot format doesn't carry the data.
- Resurrecting the old Task 1.4 / 1.5 numbering.
- Adding `FutureGenDelegs` query support if dugite queries don't already expose genesis delegates.

## Design

### Gap A — implement `GenesisKeyDelegation`

**File:** `state/certificates.rs:487-505`

Add a new field to `GovSubState` (or `CertSubState`, wherever genesis delegates belong — grep for `gen_delegs` first to find the existing structure):

```rust
// In GovSubState or a new sub-state:
pub future_gen_delegs: BTreeMap<(Slot, Hash28 /* genesis_hash */), GenDelegPair>,
pub gen_delegs: BTreeMap<Hash28 /* genesis_hash */, GenDelegPair>,

pub struct GenDelegPair {
    pub delegate: Hash28,
    pub vrf_keyhash: Hash32,
}
```

At the match arm:

```rust
Certificate::GenesisKeyDelegation { genesis_hash, genesis_delegate_hash, vrf_keyhash } => {
    // Queue activation at slot = current_slot + 2*stability_window (Haskell:
    // futureGenDelegs is keyed by (activation_slot, genesis_hash)).
    let activation = current_slot + 2 * stability_window;
    Arc::make_mut(&mut gov.future_gen_delegs).insert(
        (activation, *genesis_hash),
        GenDelegPair { delegate: *genesis_delegate_hash, vrf_keyhash: *vrf_keyhash },
    );
    debug!("Genesis key delegation queued for activation at slot {}", activation.0);
}
```

And in Shelley's `process_epoch_transition`, drain `future_gen_delegs` entries whose activation slot ≤ current slot into `gen_delegs`. ~20 LoC addition.

**Haskell reference:** `Shelley/Rules/Delegs.hs` (`DELEGS` rule, `GenesisDelegCert` case).

**Unit test:** apply a `GenesisKeyDelegation` cert, advance slots past the activation window, assert `gov.gen_delegs` reflects the new delegate.

Only implemented for Shelley-era validation; Babbage+ era impls can leave this as a no-op because those eras don't emit the cert.

### Gap B — comment fix

```rust
// state/certificates.rs:1323 (test helper)
reward_account: vec![0xe0u8; 29], // test fixture: 29-byte key-type reward address
```

### Gap C — comment fix + debug log

```rust
// state/snapshot.rs, upgrade the comment and add the warn.
// The Haskell snapshot format carries per-cred deposits from v12 onward.
// For v<12 snapshots we reconstruct them from the current key_deposit
// protocol parameter. This is exact iff key_deposit has never been
// changed via governance — true on all current networks.
if state.snapshot_version < 12 {
    tracing::warn!(
        version = state.snapshot_version,
        "Loading pre-v12 snapshot: reconstructing stake_key_deposits from current key_deposit",
    );
}
```

### Gap D — section header rename

```rust
// ledger_seq.rs:1102
// ─────────────────────────────────────────────────────────────────────────────
// Helpers and errors
// ─────────────────────────────────────────────────────────────────────────────
```

### Gap E — Byron impl comment

```rust
// eras/byron.rs, on_era_transition (or equivalent)
fn on_era_transition(&self, ...) -> Result<(), LedgerError> {
    // Byron has no staking state. Initialization of Shelley-era state is
    // performed by ShelleyRules::on_era_transition at the receiving side.
    Ok(())
}
```

### Gap F — table row removal

```rust
// eras/common.rs:24, remove the validate_shelley_base row from the table
```

### Validation

- **Unit test — gap A**: `test_genesis_key_delegation_activates_after_stability_window`. Build a state with Shelley-era cert, apply it at slot S, advance to slot S + 2*stability_window, assert `gen_delegs` updated.
- **Gaps B, C, D, E, F**: no tests, comment-only changes.
- **cargo fmt + clippy + nextest** — full workspace, no warnings.

## Risk / tradeoffs

- **Gap A is the only behavioral change.** If dugite never replays Shelley-era blocks (Mithril fast-forwards past them), this is dead code in production. Implement anyway for correctness and for future full-sync-from-genesis testing.
- **`future_gen_delegs` data structure** needs to live somewhere. Pick between `GovSubState` (it's governance-adjacent) and a new `GenesisDelegSubState`. Prefer folding into `GovSubState` to avoid adding a new sub-state.
- **Stability window lookup** — the Shelley `stability_window` is derivable from `epoch_length * 3k / f`. Use the existing `epochs.stability_window` cached field if present, otherwise compute.

## Order of operations

1. Gap B, D, F (pure comment/rename fixes) — one commit.
2. Gap C (comment + warn log) — same or follow-up commit.
3. Gap E (Byron comment) — same or follow-up commit.
4. Gap A (real implementation) — separate commit with test.
5. Clippy + fmt + nextest.

## Done when

- `rg -n 'TODO|FIXME|stubs for Task' crates/dugite-ledger/src` (tests excluded) returns zero.
- `GenesisKeyDelegation` match arm has a non-empty body and mutates state.
- Unit test for delegation activation passes.
- All comment fixes in place.
- Clippy clean.
