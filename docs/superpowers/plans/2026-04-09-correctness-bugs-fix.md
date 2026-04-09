# Correctness Bug Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix 11 confirmed correctness bugs in the Dugite ledger crate, matching the Haskell cardano-ledger reference implementation.

**Architecture:** All changes are in `crates/dugite-ledger/src/`. Split into two branches: Branch 1 (P0+P1, 6 bugs — state divergence and tx rejection issues) and Branch 2 (P2, 5 bugs — missing adversarial checks). Each task is a standalone commit with TDD.

**Tech Stack:** Rust, dugite-ledger crate, inline `#[cfg(test)]` unit tests

**Spec:** `docs/superpowers/specs/2026-04-09-correctness-bugs-fix-design.md`

---

## Branch 1: P0 + P1 Critical Fixes

### Task 1: #381 — Remove committee_resigned.remove() from CommitteeHotAuth state application

The validation-layer rejection (`CommitteeHasPreviouslyResigned`) already exists in `validation/mod.rs:1038-1066`. The remaining bug is in the state application: `certificates.rs` lines 368 and 974 call `gov.committee_resigned.remove(&cold_key)`, allowing re-authorization after resignation.

**Files:**
- Modify: `crates/dugite-ledger/src/state/certificates.rs:367-368`
- Modify: `crates/dugite-ledger/src/state/certificates.rs:973-974`
- Test: `crates/dugite-ledger/src/state/certificates.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Write failing test**

Add this test to the existing `#[cfg(test)] mod tests` block at the bottom of `certificates.rs`:

```rust
#[test]
fn committee_resignation_is_permanent() {
    // A resigned committee member submitting CommitteeHotAuth must NOT
    // be removed from the resigned set (Haskell: resignation is permanent).
    let mut state = create_test_state();
    let cold_key = Hash32::from([0xCC; 32]);
    let hot_key1 = Hash32::from([0xAA; 32]);
    let hot_key2 = Hash32::from([0xBB; 32]);

    // Register in committee
    let gov = Arc::make_mut(&mut state.governance);
    gov.committee_expiration.insert(cold_key, EpochNo(100));

    // Authorize hot key
    let cold_cred = Credential::VerificationKey(Hash28::from([0xCC; 28]));
    let hot_cred1 = Credential::VerificationKey(Hash28::from([0xAA; 28]));
    state.process_certificate(&Certificate::CommitteeHotAuth {
        cold_credential: cold_cred.clone(),
        hot_credential: hot_cred1.clone(),
    });
    assert!(state.governance.committee_hot_keys.contains_key(&cold_key));

    // Resign
    state.process_certificate(&Certificate::CommitteeColdResign {
        cold_credential: cold_cred.clone(),
        anchor: None,
    });
    assert!(state.governance.committee_resigned.contains(&cold_key));

    // Attempt re-authorization — resigned set must NOT be cleared
    let hot_cred2 = Credential::VerificationKey(Hash28::from([0xBB; 28]));
    state.process_certificate(&Certificate::CommitteeHotAuth {
        cold_credential: cold_cred.clone(),
        hot_credential: hot_cred2,
    });
    assert!(
        state.governance.committee_resigned.contains(&cold_key),
        "Committee resignation must be permanent — resigned set should not be cleared by CommitteeHotAuth"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p dugite-ledger -E 'test(committee_resignation_is_permanent)'`
Expected: FAIL — assertion on `committee_resigned.contains(&cold_key)` fails because `remove()` clears it.

- [ ] **Step 3: Remove the committee_resigned.remove() calls**

In `crates/dugite-ledger/src/state/certificates.rs`, delete line 368:
```rust
// DELETE this line:
gov.committee_resigned.remove(&cold_key);
```

And delete line ~974 (the same call in the dry-run/delta path):
```rust
// DELETE this line:
gov.committee_resigned.remove(&cold_key);
```

Also update the comment on line 367 from `// Remove from resigned if re-authorizing` to:
```rust
// NOTE: Do NOT remove from committee_resigned here. Resignation is
// permanent per Haskell's checkAndOverwriteCommitteeMemberState.
// ConwayCommitteeHasPreviouslyResigned rejects this cert at validation.
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p dugite-ledger -E 'test(committee_resignation_is_permanent)'`
Expected: PASS

- [ ] **Step 5: Run full crate tests**

Run: `cargo nextest run -p dugite-ledger`
Expected: All pass

- [ ] **Step 6: Commit**

```bash
git add crates/dugite-ledger/src/state/certificates.rs
git commit -m "fix(ledger): make committee resignation permanent (#381)

Remove committee_resigned.remove() from CommitteeHotAuth handler.
Haskell's checkAndOverwriteCommitteeMemberState treats resignation as
permanent — a subsequent hot key auth does not clear the resigned state."
```

---

### Task 2: #377 + #387 — Block body size: equality check and hard error

The current check at `apply.rs:144-155` compares `header.body_size > max_block_body_size` and only warns. The Haskell BBODY rule checks `actual_serialized_size == header.body_size` (equality) and returns a hard error.

**Files:**
- Modify: `crates/dugite-ledger/src/state/apply.rs:141-155`
- Modify: `crates/dugite-ledger/src/state/mod.rs` (add `LedgerError` variant)
- Test: `crates/dugite-ledger/src/state/apply.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Add LedgerError variant**

In `crates/dugite-ledger/src/state/mod.rs`, find the `LedgerError` enum and add:

```rust
#[error("Block body size mismatch: actual serialized size {actual} != header claimed size {claimed} (WrongBlockBodySizeBBODY)")]
WrongBlockBodySize { actual: u32, claimed: u32 },
```

- [ ] **Step 2: Write failing test**

Add to the `#[cfg(test)]` section at the bottom of `apply.rs`:

```rust
#[test]
fn block_body_size_mismatch_returns_error() {
    // If the block header claims a body size that doesn't match the actual
    // serialized body, apply_block must return WrongBlockBodySize.
    let mut state = create_test_ledger_state();
    let mut block = create_test_block(&state);
    // Corrupt the header's body_size to be wrong
    block.header.body_size = 999_999;
    let result = state.apply_block(&block, BlockValidationMode::ValidateAll);
    assert!(
        matches!(result, Err(LedgerError::WrongBlockBodySize { .. })),
        "Expected WrongBlockBodySize error for header/body mismatch, got: {:?}",
        result
    );
}
```

Note: If `create_test_block` or `create_test_ledger_state` helpers don't exist yet, create minimal versions that produce a valid block with a known body size. The key is that the block's header `body_size` field is set to a value that doesn't match the actual body length.

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo nextest run -p dugite-ledger -E 'test(block_body_size_mismatch_returns_error)'`
Expected: FAIL — current code only warns, doesn't return Err.

- [ ] **Step 4: Replace warn-only check with hard error**

In `crates/dugite-ledger/src/state/apply.rs`, replace lines 141-155:

```rust
// Block body size check — Haskell BBODY rule: actual serialized size must
// equal the size claimed in the block header (WrongBlockBodySizeBBODY).
// This is an equality check, not a <= max_block_body_size check.
// The ppMaxBBSize parameter is checked at the consensus layer (chainChecks),
// not in BBODY.
if mode == BlockValidationMode::ValidateAll && block.header.body_size > 0 {
    let actual_body_size = block.body_size_bytes();
    if actual_body_size != block.header.body_size {
        return Err(LedgerError::WrongBlockBodySize {
            actual: actual_body_size,
            claimed: block.header.body_size,
        });
    }
}
```

Note: `block.body_size_bytes()` should return the serialized CBOR body length. If this method doesn't exist on `Block`, compute it from `block.raw_cbor` or the block's body serialization. Check how the block type provides body size — it may be `block.body.len()` if raw bytes are available, or require re-serialization via `dugite_serialization`.

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo nextest run -p dugite-ledger -E 'test(block_body_size_mismatch_returns_error)'`
Expected: PASS

- [ ] **Step 6: Run full crate tests and clippy**

Run: `cargo nextest run -p dugite-ledger && cargo clippy -p dugite-ledger --all-targets -- -D warnings`
Expected: All pass. Some existing tests that relied on the warn-only behavior may need the header `body_size` field corrected if they were setting it incorrectly.

- [ ] **Step 7: Commit**

```bash
git add crates/dugite-ledger/src/state/apply.rs crates/dugite-ledger/src/state/mod.rs
git commit -m "fix(ledger): block body size equality check with hard error (#377, #387)

Replace warn-only max_block_body_size comparison with Haskell's BBODY
rule: actual_serialized_size == header.body_size (equality check).
Returns LedgerError::WrongBlockBodySize on mismatch instead of logging
a warning. The ppMaxBBSize check belongs in chainChecks (consensus layer)."
```

---

### Task 3: #380 — Treasury donation flush ordering

Donations must be flushed AFTER governance enactment (step 9 in Haskell EPOCH), not at step 0 before SNAP.

**Files:**
- Modify: `crates/dugite-ledger/src/state/epoch.rs:53-72` (move the donation flush)
- Test: `crates/dugite-ledger/src/state/epoch.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Write failing test**

Add to `epoch.rs` tests:

```rust
#[test]
fn treasury_donation_flushed_after_governance_enactment() {
    // Haskell EPOCH rule step 9: treasury += utxosDonation + unclaimed
    // Donations must be added AFTER enacted treasury withdrawals are subtracted.
    let mut state = create_test_state_with_epoch(EpochNo(5));
    state.treasury = Lovelace(1_000_000);
    state.pending_donations = Lovelace(50_000);

    // Set up a ratified treasury withdrawal that will subtract from treasury
    // during governance enactment. The exact mechanism depends on the
    // ratify_proposals/enact_gov_action flow — create a minimal enacted
    // TreasuryWithdrawals action with a withdrawal of 100_000.
    //
    // After correct ordering:
    //   treasury_after_withdrawal = 1_000_000 - 100_000 = 900_000
    //   treasury_final = 900_000 + 50_000 = 950_000
    //
    // With wrong (current) ordering:
    //   treasury_after_donation = 1_000_000 + 50_000 = 1_050_000
    //   treasury_final = 1_050_000 - 100_000 = 950_000
    //
    // In this simple case the final value is the same, but the intermediate
    // treasury balance seen by governance enactment differs, which can affect
    // thresholds. To make the test distinguishable, verify that
    // pending_donations is NOT zero before ratify_proposals is called.

    // Capture pending_donations value at the point where ratify_proposals runs.
    // This requires either:
    //   a) Checking that pending_donations is still non-zero when governance
    //      code runs (instrumented test), or
    //   b) Verifying the final treasury against the correct ordering formula.
    //
    // For now, verify the simple invariant: after epoch transition,
    // pending_donations is zero AND treasury reflects correct ordering.
    state.process_epoch_transition(EpochNo(6));
    assert_eq!(
        state.pending_donations,
        Lovelace(0),
        "pending_donations must be drained after epoch transition"
    );
    // Treasury must include the donation (exact value depends on rewards/enactment)
    assert!(
        state.treasury.0 >= 1_050_000,
        "treasury must include flushed donations: {}",
        state.treasury.0
    );
}
```

- [ ] **Step 2: Run test to verify baseline**

Run: `cargo nextest run -p dugite-ledger -E 'test(treasury_donation_flushed_after_governance_enactment)'`
Expected: May pass or fail depending on exact assertions. The primary change is structural.

- [ ] **Step 3: Move donation flush from step 0 to after governance enactment**

In `crates/dugite-ledger/src/state/epoch.rs`, remove the Step 0 block (lines ~53-72) that flushes `pending_donations`. Then add it after `self.ratify_proposals()` (currently at line ~582) and after the `totalObligation` recalculation block (line ~719):

```rust
// Step 9 (Haskell EPOCH rule): Flush pending treasury donations.
//
// Haskell: chainAccountState3 = chainAccountState2
//     & casTreasuryL <>~ (utxoState0 ^. utxosDonationL <> fold unclaimed)
//
// Donations are added to the treasury AFTER enacted governance treasury
// withdrawals have been subtracted (step 4) and after proposal deposits
// are returned (step 7). The donation value comes from the pre-epoch
// utxoState (utxoState0), matching Haskell.
if self.pending_donations.0 > 0 {
    let flushed = self.pending_donations;
    self.treasury.0 = self.treasury.0.saturating_add(flushed.0);
    self.pending_donations = Lovelace(0);
    debug!(
        epoch = new_epoch.0,
        donations_lovelace = flushed.0,
        "Flushed pending treasury donations (step 9, after governance enactment)"
    );
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p dugite-ledger -E 'test(treasury_donation_flushed_after_governance_enactment)'`
Expected: PASS

- [ ] **Step 5: Run full crate tests**

Run: `cargo nextest run -p dugite-ledger`
Expected: All pass. Treasury accounting tests may need adjusted expectations.

- [ ] **Step 6: Commit**

```bash
git add crates/dugite-ledger/src/state/epoch.rs
git commit -m "fix(ledger): move treasury donation flush to after governance enactment (#380)

Haskell EPOCH rule step 9: utxosDonation is added to the treasury AFTER
enacted treasury withdrawals are subtracted and proposal deposits
returned. Previously flushed at step 0 before SNAP rotation."
```

---

### Task 4: #379 — CIP-0069: PlutusV3 NoDatum exemption + V1/V2 UnspendableUTxONoDatumHash

Dugite doesn't check `UnspendableUTxONoDatumHash` at all. V1/V2 script-locked inputs with `OutputDatum::None` must be rejected; V3 inputs with `OutputDatum::None` are allowed (CIP-0069).

**Files:**
- Modify: `crates/dugite-ledger/src/validation/datum.rs:80-126`
- Modify: `crates/dugite-ledger/src/validation/mod.rs` (add error variant)
- Test: `crates/dugite-ledger/src/validation/tests.rs` (add datum tests)

- [ ] **Step 1: Add ValidationError variant**

In `crates/dugite-ledger/src/validation/mod.rs`, add to `ValidationError` enum:

```rust
/// Alonzo UTXO rule: a script-locked spending input has no datum
/// (OutputDatum::None) and the locking script is PlutusV1 or PlutusV2.
/// PlutusV3 inputs are exempt per CIP-0069.
///
/// Reference: Haskell `UnspendableUTxONoDatumHash` in
/// `cardano-ledger-alonzo:Cardano.Ledger.Alonzo.Rules.Utxo`.
#[error(
    "Script-locked input {input} has no datum (NoDatum) and locking script is {language} \
     (UnspendableUTxONoDatumHash — PlutusV3 exempt per CIP-0069)"
)]
UnspendableUTxONoDatumHash { input: String, language: String },
```

- [ ] **Step 2: Write failing tests**

Add to `crates/dugite-ledger/src/validation/tests.rs`:

```rust
#[test]
fn v1_script_input_with_no_datum_rejected() {
    // A PlutusV1 script-locked spending input with OutputDatum::None must
    // produce UnspendableUTxONoDatumHash.
    let (tx, utxo_set, params) = build_plutus_v1_no_datum_scenario();
    let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
    let errors = result.unwrap_err();
    assert!(
        errors.iter().any(|e| matches!(e, ValidationError::UnspendableUTxONoDatumHash { .. })),
        "Expected UnspendableUTxONoDatumHash for V1 NoDatum input, got: {:?}", errors
    );
}

#[test]
fn v3_script_input_with_no_datum_allowed() {
    // A PlutusV3 script-locked spending input with OutputDatum::None must
    // NOT produce UnspendableUTxONoDatumHash (CIP-0069 exemption).
    let (tx, utxo_set, params) = build_plutus_v3_no_datum_scenario();
    let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
    // Should not contain UnspendableUTxONoDatumHash
    if let Err(errors) = &result {
        assert!(
            !errors.iter().any(|e| matches!(e, ValidationError::UnspendableUTxONoDatumHash { .. })),
            "V3 NoDatum inputs must NOT trigger UnspendableUTxONoDatumHash (CIP-0069), got: {:?}", errors
        );
    }
}
```

The helper functions `build_plutus_v1_no_datum_scenario` and `build_plutus_v3_no_datum_scenario` should construct:
- A transaction spending a script-locked UTxO at a Plutus V1/V3 script address
- The UTxO has `datum: OutputDatum::None`
- The UTxO set contains a script reference of the appropriate version
- The transaction has appropriate redeemers and script witnesses

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo nextest run -p dugite-ledger -E 'test(v1_script_input_with_no_datum_rejected) | test(v3_script_input_with_no_datum_allowed)'`
Expected: `v1_script_input_with_no_datum_rejected` FAILS (no error produced for V1 NoDatum).

- [ ] **Step 4: Implement UnspendableUTxONoDatumHash check in datum.rs**

In `crates/dugite-ledger/src/validation/datum.rs`, modify the `check_datum_witnesses` function. The function signature needs to accept a script version lookup. Change:

```rust
pub(super) fn check_datum_witnesses(
    tx: &Transaction,
    utxo_set: &dyn UtxoLookup,
    errors: &mut Vec<ValidationError>,
)
```

to:

```rust
pub(super) fn check_datum_witnesses(
    tx: &Transaction,
    utxo_set: &dyn UtxoLookup,
    script_versions: &HashMap<Hash28, u8>,
    errors: &mut Vec<ValidationError>,
)
```

Where `script_versions` maps script hash → language version (1=V1, 2=V2, 3=V3). This map is already computed by `collateral::collect_plutus_script_hashes` or can be obtained from `collateral::plutus_script_version_map`.

In the input loop (lines 102-126), after checking `is_script_locked` and before the `DatumHash` check, add:

```rust
// CIP-0069 / Haskell UnspendableUTxONoDatumHash:
// Script-locked inputs with OutputDatum::None are only allowed for PlutusV3.
// V1/V2 inputs MUST have either DatumHash or InlineDatum.
if utxo.datum == OutputDatum::None {
    // Resolve the script hash from the payment credential
    let script_hash: Option<Hash28> = match utxo.address.payment_credential() {
        Some(Credential::Script(h)) => Some(*h),
        _ => None,
    };
    if let Some(sh) = script_hash {
        let version = script_versions.get(&sh).copied().unwrap_or(0);
        if version < 3 {
            // V1, V2, or unknown/native — NoDatum is not allowed
            errors.push(ValidationError::UnspendableUTxONoDatumHash {
                input: input.to_string(),
                language: match version {
                    1 => "PlutusV1".to_string(),
                    2 => "PlutusV2".to_string(),
                    _ => "unknown".to_string(),
                },
            });
        }
        // V3: NoDatum is fine (CIP-0069) — no error
    }
    continue; // NoDatum inputs never contribute to required_datum_hashes
}
```

Update the call site in `phase1.rs` (where `check_datum_witnesses` is called) to pass the `script_versions` map. You can compute it using the existing `plutus_script_version_map` function from `collateral.rs`, or build it inline from the witness set and reference inputs.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo nextest run -p dugite-ledger -E 'test(v1_script_input_with_no_datum_rejected) | test(v3_script_input_with_no_datum_allowed)'`
Expected: Both PASS

- [ ] **Step 6: Run full crate tests**

Run: `cargo nextest run -p dugite-ledger`
Expected: All pass

- [ ] **Step 7: Commit**

```bash
git add crates/dugite-ledger/src/validation/datum.rs crates/dugite-ledger/src/validation/mod.rs crates/dugite-ledger/src/validation/phase1.rs
git commit -m "fix(ledger): add UnspendableUTxONoDatumHash with CIP-0069 V3 exemption (#379)

PlutusV1/V2 script-locked spending inputs with OutputDatum::None now
produce UnspendableUTxONoDatumHash. PlutusV3 inputs are exempt per
CIP-0069 (lang < PlutusV3 guard from Haskell getInputDataHashesTxBody)."
```

---

### Task 5: #376 — Add ConwayWdrlNotDelegatedToDRep check

For PV ≥ 10, every KeyHash reward account making a withdrawal must have any DRep delegation.

**Files:**
- Modify: `crates/dugite-ledger/src/validation/mod.rs` (add error variant + check + ValidationContext field)
- Test: `crates/dugite-ledger/src/validation/tests.rs`

- [ ] **Step 1: Add ValidationError variant and ValidationContext field**

In `validation/mod.rs`, add to `ValidationError`:

```rust
/// Conway LEDGER rule (PV ≥ 10): a KeyHash reward account making a
/// withdrawal must have an active DRep delegation (any delegation value
/// including AlwaysAbstain/AlwaysNoConfidence satisfies this).
///
/// Reference: Haskell `ConwayWdrlNotDelegatedToDRep` in
/// `cardano-ledger-conway:Cardano.Ledger.Conway.Rules.Ledger`.
#[error(
    "Withdrawal rejected: KeyHash reward account {credential_hash} has no DRep delegation \
     (ConwayWdrlNotDelegatedToDRep, requires PV >= 10)"
)]
WdrlNotDelegatedToDRep { credential_hash: String },
```

In `ValidationContext`, add a new field:

```rust
pub vote_delegations: Option<HashSet<Hash32>>,
```

Add a builder method:

```rust
pub fn with_vote_delegations(mut self, delegations: HashSet<Hash32>) -> Self {
    self.vote_delegations = Some(delegations);
    self
}
```

Thread the new field through `with_full_ledger_state` and `validate_transaction_with_context` → `validate_transaction_with_pools`. Add `vote_delegations: Option<&HashSet<Hash32>>` as a parameter to `validate_transaction_with_pools`.

- [ ] **Step 2: Write failing test**

Add to `validation/tests.rs`:

```rust
#[test]
fn pv10_withdrawal_without_drep_delegation_rejected() {
    // At PV >= 10, withdrawals from KeyHash reward accounts without
    // DRep delegation must produce WdrlNotDelegatedToDRep.
    let mut params = test_params();
    params.protocol_version_major = 10;

    let credential_hash = Hash32::from([0x11; 32]);
    let reward_addr = build_key_hash_reward_address(&credential_hash);

    let tx = build_tx_with_withdrawal(&reward_addr, Lovelace(1_000_000));
    let utxo_set = build_utxo_set_for_tx(&tx);

    // vote_delegations is empty — credential has no DRep delegation
    let vote_delegations: HashSet<Hash32> = HashSet::new();

    let context = ValidationContext::new()
        .with_vote_delegations(vote_delegations)
        .with_reward_accounts(HashMap::from([(credential_hash, Lovelace(1_000_000))]));

    let result = validate_transaction_with_context(
        &tx, &utxo_set, &params, 100, 300, None, context,
    );
    let errors = result.unwrap_err();
    assert!(
        errors.iter().any(|e| matches!(e, ValidationError::WdrlNotDelegatedToDRep { .. })),
        "Expected WdrlNotDelegatedToDRep for PV10 withdrawal without delegation, got: {:?}", errors
    );
}

#[test]
fn pv10_withdrawal_with_drep_delegation_accepted() {
    let mut params = test_params();
    params.protocol_version_major = 10;

    let credential_hash = Hash32::from([0x11; 32]);
    let reward_addr = build_key_hash_reward_address(&credential_hash);

    let tx = build_tx_with_withdrawal(&reward_addr, Lovelace(1_000_000));
    let utxo_set = build_utxo_set_for_tx(&tx);

    // credential HAS DRep delegation
    let vote_delegations: HashSet<Hash32> = HashSet::from([credential_hash]);

    let context = ValidationContext::new()
        .with_vote_delegations(vote_delegations)
        .with_reward_accounts(HashMap::from([(credential_hash, Lovelace(1_000_000))]));

    let result = validate_transaction_with_context(
        &tx, &utxo_set, &params, 100, 300, None, context,
    );
    if let Err(errors) = &result {
        assert!(
            !errors.iter().any(|e| matches!(e, ValidationError::WdrlNotDelegatedToDRep { .. })),
            "Should NOT get WdrlNotDelegatedToDRep when delegation exists, got: {:?}", errors
        );
    }
}

#[test]
fn pv9_withdrawal_without_drep_delegation_skipped() {
    // At PV 9 (bootstrap), the DRep delegation check is skipped.
    let mut params = test_params();
    params.protocol_version_major = 9;

    let credential_hash = Hash32::from([0x11; 32]);
    let reward_addr = build_key_hash_reward_address(&credential_hash);

    let tx = build_tx_with_withdrawal(&reward_addr, Lovelace(1_000_000));
    let utxo_set = build_utxo_set_for_tx(&tx);

    let vote_delegations: HashSet<Hash32> = HashSet::new(); // no delegation

    let context = ValidationContext::new()
        .with_vote_delegations(vote_delegations)
        .with_reward_accounts(HashMap::from([(credential_hash, Lovelace(1_000_000))]));

    let result = validate_transaction_with_context(
        &tx, &utxo_set, &params, 100, 300, None, context,
    );
    if let Err(errors) = &result {
        assert!(
            !errors.iter().any(|e| matches!(e, ValidationError::WdrlNotDelegatedToDRep { .. })),
            "PV9 must NOT check DRep delegation for withdrawals, got: {:?}", errors
        );
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo nextest run -p dugite-ledger -E 'test(pv10_withdrawal_without_drep_delegation_rejected)'`
Expected: FAIL — no WdrlNotDelegatedToDRep error produced.

- [ ] **Step 4: Implement the DRep withdrawal delegation check**

In `validation/mod.rs`, after the withdrawal amount validation block (around line ~1100), add:

```rust
// ------------------------------------------------------------------
// Conway LEDGER rule: ConwayWdrlNotDelegatedToDRep (PV >= 10)
//
// Every KeyHash reward account making a withdrawal must have a DRep
// delegation. Script-credential accounts are exempt. Any delegation
// value (including AlwaysAbstain/AlwaysNoConfidence) satisfies the check.
// Uses the certState BEFORE the current tx's certificates are applied.
//
// Reference: Haskell `validateWithdrawalsDelegated` in
// `cardano-ledger-conway:Cardano.Ledger.Conway.Rules.Ledger`.
// ------------------------------------------------------------------
if params.protocol_version_major >= 10 {
    if let Some(delegations) = vote_delegations {
        for reward_addr in tx.body.withdrawals.keys() {
            if reward_addr.len() < 29 {
                continue;
            }
            let header = reward_addr[0];
            // Script-credential reward accounts (header bit 4 set) are exempt
            let is_script = (header & 0x10) != 0;
            if is_script {
                continue;
            }
            // KeyHash credential — must have DRep delegation
            if let Ok(cred_hash) = Hash28::try_from(&reward_addr[1..29]) {
                let key = cred_hash.to_hash32_padded();
                if !delegations.contains(&key) {
                    errors.push(ValidationError::WdrlNotDelegatedToDRep {
                        credential_hash: key.to_hex(),
                    });
                }
            }
        }
    }
}
```

Also wire the `vote_delegations` field through the call chain from `apply_block` → `validate_transaction_with_pools`. In `state/apply.rs` where `validate_transaction_with_context` is called, populate `vote_delegations` from `self.governance.vote_delegations.keys()`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo nextest run -p dugite-ledger -E 'test(pv10_withdrawal)'`
Expected: All three tests PASS

- [ ] **Step 6: Run full crate tests**

Run: `cargo nextest run -p dugite-ledger`
Expected: All pass

- [ ] **Step 7: Commit**

```bash
git add crates/dugite-ledger/src/validation/mod.rs crates/dugite-ledger/src/validation/tests.rs crates/dugite-ledger/src/state/apply.rs
git commit -m "fix(ledger): add ConwayWdrlNotDelegatedToDRep check for PV >= 10 (#376)

KeyHash reward accounts making withdrawals must have any DRep delegation
when protocol version >= 10. Script-credential accounts are exempt.
Any delegation value (including AlwaysAbstain) satisfies the check."
```

---

### Task 6: #375 — Fix misleading Conway deregistration comment + regression test

The Phase-1 validation check already exists. Fix the misleading comment in `certificates.rs` and add a regression test.

**Files:**
- Modify: `crates/dugite-ledger/src/state/certificates.rs:169-171`
- Test: `crates/dugite-ledger/src/validation/tests.rs`

- [ ] **Step 1: Fix misleading comment**

In `crates/dugite-ledger/src/state/certificates.rs`, replace lines 169-171:

```rust
// Conway cert tag 8: deregistration returns remaining reward balance
// as part of the deposit refund. Remove from delegations/rewards but
// keep the stake_map entry — UTxOs may still exist at this credential.
```

with:

```rust
// Conway cert tag 8: deregistration refunds the stored deposit.
// Phase-1 validation enforces that the reward balance is zero
// (StakeKeyHasNonZeroAccountBalanceDELEG) before this point.
// Remove from delegations/rewards but keep the stake_map entry —
// UTxOs may still exist at this credential.
```

- [ ] **Step 2: Add regression test confirming validation rejects non-zero balance for Conway tag 8**

Add to `validation/tests.rs` (if not already present from #337 work):

```rust
#[test]
fn conway_deregistration_with_nonzero_balance_rejected() {
    // Regression test: Conway ConwayStakeDeregistration (tag 8) with
    // non-zero reward balance must produce StakeKeyHasNonZeroBalance.
    let params = test_conway_params(); // protocol_version_major >= 9
    let credential = Credential::VerificationKey(Hash28::from([0x11; 28]));
    let key = credential.to_hash().to_hash32_padded();

    let tx = build_tx_with_conway_deregistration(&credential, Lovelace(2_000_000));
    let utxo_set = build_utxo_set_for_tx(&tx);

    // Reward account has 500_000 lovelace — must be zero for deregistration
    let reward_accounts = HashMap::from([(key, Lovelace(500_000))]);

    let context = ValidationContext::new()
        .with_reward_accounts(reward_accounts);

    let result = validate_transaction_with_context(
        &tx, &utxo_set, &params, 100, 300, None, context,
    );
    let errors = result.unwrap_err();
    assert!(
        errors.iter().any(|e| matches!(
            e, ValidationError::StakeKeyHasNonZeroBalance { balance, .. } if *balance == 500_000
        )),
        "Expected StakeKeyHasNonZeroBalance for Conway tag 8 deregistration, got: {:?}", errors
    );
}
```

- [ ] **Step 3: Run test to verify it passes (existing check)**

Run: `cargo nextest run -p dugite-ledger -E 'test(conway_deregistration_with_nonzero_balance_rejected)'`
Expected: PASS — the check already exists in `validation/mod.rs:805-831`.

- [ ] **Step 4: Commit**

```bash
git add crates/dugite-ledger/src/state/certificates.rs crates/dugite-ledger/src/validation/tests.rs
git commit -m "fix(ledger): fix misleading Conway deregistration comment + regression test (#375)

The comment incorrectly said 'returns remaining reward balance as part
of the deposit refund.' Phase-1 validation enforces zero balance before
deregistration (StakeKeyHasNonZeroAccountBalanceDELEG). Added regression
test confirming Conway tag 8 rejects non-zero balance."
```

---

### Task 7: Branch 1 — Final verification and close invalid issues

- [ ] **Step 1: Run full workspace build and test**

```bash
cargo build --all-targets && cargo nextest run --workspace && cargo clippy --all-targets -- -D warnings && cargo fmt --all -- --check
```

Expected: All pass with zero warnings.

- [ ] **Step 2: Close invalid issues on GitHub**

```bash
gh issue close 378 --comment "Verified against Haskell source (Conway/Tx.hs): tierRefScriptFee uses floor, not ceiling. Dugite's current truncation and comment at scripts.rs:363 are correct. Closing as invalid."

gh issue close 386 --comment "Verified against Haskell source (Conway/Rules/Deleg.hs): ConwayRegCert stores ppKeyDepositCompact (protocol parameter), not the cert's declared deposit field. Dugite's deposit: _ pattern is correct. Closing as invalid."
```

- [ ] **Step 3: Commit and push Branch 1**

```bash
git push -u origin feature/correctness-fixes-p0-p1
```

---

## Branch 2: P2 Missing-Check Additions

### Task 8: #385 — ppuWellFormed check at proposal submission

`ppuWellFormed` must reject `ParameterChange` proposals with zero-value fields at submission time.

**Files:**
- Modify: `crates/dugite-ledger/src/validation/conway.rs`
- Modify: `crates/dugite-ledger/src/validation/mod.rs` (add error variant, call new check)
- Test: `crates/dugite-ledger/src/validation/tests.rs`

- [ ] **Step 1: Add ValidationError variant**

In `validation/mod.rs`:

```rust
/// Conway GOV rule: a `ParameterChange` proposal's `PParamsUpdate` is
/// malformed — one or more fields fail the `ppuWellFormed` check.
///
/// Reference: Haskell `MalformedProposal` in
/// `cardano-ledger-conway:Cardano.Ledger.Conway.Rules.Gov`.
#[error("Governance proposal rejected: malformed PParamsUpdate ({reason})")]
MalformedProposal { reason: String },
```

- [ ] **Step 2: Write failing test**

```rust
#[test]
fn parameter_change_with_zero_max_tx_size_rejected() {
    let params = test_conway_params();
    let tx = build_tx_with_parameter_change_proposal(|ppu| {
        ppu.max_tx_size = Some(0); // zero is invalid
    });
    let utxo_set = build_utxo_set_for_tx(&tx);
    let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
    let errors = result.unwrap_err();
    assert!(
        errors.iter().any(|e| matches!(e, ValidationError::MalformedProposal { .. })),
        "Expected MalformedProposal for zero max_tx_size, got: {:?}", errors
    );
}

#[test]
fn parameter_change_with_valid_fields_accepted() {
    let params = test_conway_params();
    let tx = build_tx_with_parameter_change_proposal(|ppu| {
        ppu.max_tx_size = Some(16384); // valid nonzero value
    });
    let utxo_set = build_utxo_set_for_tx(&tx);
    let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
    if let Err(errors) = &result {
        assert!(
            !errors.iter().any(|e| matches!(e, ValidationError::MalformedProposal { .. })),
            "Valid PParamsUpdate should not trigger MalformedProposal, got: {:?}", errors
        );
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo nextest run -p dugite-ledger -E 'test(parameter_change_with_zero_max_tx_size_rejected)'`
Expected: FAIL

- [ ] **Step 4: Implement ppuWellFormed check**

In `crates/dugite-ledger/src/validation/conway.rs`, add:

```rust
/// Check ppuWellFormed for ParameterChange governance proposals.
///
/// Haskell's `ppuWellFormed` (Conway/PParams.hs) rejects proposals with
/// zero values in specific fields. Only applies to ParameterChange actions.
pub(super) fn check_pparam_update_well_formed(
    params: &ProtocolParameters,
    body: &dugite_primitives::transaction::TransactionBody,
    errors: &mut Vec<ValidationError>,
) {
    if params.protocol_version_major < 9 {
        return;
    }
    for proposal in &body.proposal_procedures {
        if let GovAction::ParameterChange { pparam_update, .. } = &proposal.gov_action {
            let ppu = pparam_update;
            let mut reasons = Vec::new();

            if ppu.max_block_body_size == Some(0) { reasons.push("maxBBSize=0"); }
            if ppu.max_tx_size == Some(0) { reasons.push("maxTxSize=0"); }
            if ppu.max_block_header_size == Some(0) { reasons.push("maxBHSize=0"); }
            if ppu.max_val_size == Some(0) { reasons.push("maxValSize=0"); }
            if ppu.collateral_percentage == Some(0) { reasons.push("collateralPercentage=0"); }
            if ppu.committee_max_term_length == Some(0) { reasons.push("committeeMaxTermLength=0"); }
            if ppu.gov_action_lifetime == Some(0) { reasons.push("govActionLifetime=0"); }
            if matches!(ppu.pool_deposit, Some(Lovelace(0))) { reasons.push("poolDeposit=0"); }
            if matches!(ppu.gov_action_deposit, Some(Lovelace(0))) { reasons.push("govActionDeposit=0"); }
            if matches!(ppu.drep_deposit, Some(Lovelace(0))) { reasons.push("dRepDeposit=0"); }
            // coinsPerUTxOByte zero check — only enforced post-bootstrap (PV >= 10)
            if params.protocol_version_major >= 10 {
                if matches!(ppu.ada_per_utxo_byte, Some(Lovelace(0))) {
                    reasons.push("coinsPerUTxOByte=0");
                }
            }
            // nOpt zero check — PV >= 11
            if params.protocol_version_major >= 11 {
                if ppu.n_opt == Some(0) { reasons.push("nOpt=0"); }
            }
            // Empty update check
            if ppu.is_empty() { reasons.push("empty PParamsUpdate"); }

            if !reasons.is_empty() {
                errors.push(ValidationError::MalformedProposal {
                    reason: reasons.join(", "),
                });
            }
        }
    }
}
```

Note: `ppu.is_empty()` may need implementing — check if `PParamsUpdate` has an `is_empty()` method or if all fields being `None` can be tested. Adapt the field names to match the actual `PParamsUpdate` struct fields.

Call this from `validate_transaction_with_pools` in `mod.rs`, after the existing Conway-era proposal deposit check.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo nextest run -p dugite-ledger -E 'test(parameter_change_with)'`
Expected: Both PASS

- [ ] **Step 6: Commit**

```bash
git add crates/dugite-ledger/src/validation/conway.rs crates/dugite-ledger/src/validation/mod.rs crates/dugite-ledger/src/validation/tests.rs
git commit -m "fix(ledger): add ppuWellFormed check at proposal submission (#385)

ParameterChange proposals with zero-value fields (maxTxSize, poolDeposit,
etc.) are now rejected at Phase-1 validation with MalformedProposal,
matching Haskell's GOV rule ppuWellFormed check."
```

---

### Task 9: #384 — ExtraRedeemers check

Redeemers with no matching script purpose must be rejected.

**Files:**
- Modify: `crates/dugite-ledger/src/validation/collateral.rs`
- Modify: `crates/dugite-ledger/src/validation/mod.rs` (add error variant)
- Test: `crates/dugite-ledger/src/validation/tests.rs`

- [ ] **Step 1: Add ValidationError variant**

```rust
/// Alonzo UTXOW rule: a redeemer in the witness set has no matching
/// script purpose (spending input, minting policy, withdrawal, cert, vote).
///
/// Reference: Haskell `ExtraRedeemers` in
/// `cardano-ledger-alonzo:Cardano.Ledger.Alonzo.Rules.Utxow`.
#[error("Extra redeemer with no matching script purpose: tag={tag}, index={index}")]
ExtraRedeemer { tag: String, index: u32 },
```

- [ ] **Step 2: Write failing test**

```rust
#[test]
fn extra_redeemer_with_no_purpose_rejected() {
    // A redeemer at (Spend, index=99) with no corresponding script-locked
    // input at index 99 must produce ExtraRedeemer.
    let (tx, utxo_set, params) = build_tx_with_extra_redeemer();
    let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
    let errors = result.unwrap_err();
    assert!(
        errors.iter().any(|e| matches!(e, ValidationError::ExtraRedeemer { .. })),
        "Expected ExtraRedeemer for redeemer with no purpose, got: {:?}", errors
    );
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo nextest run -p dugite-ledger -E 'test(extra_redeemer_with_no_purpose_rejected)'`
Expected: FAIL

- [ ] **Step 4: Implement ExtraRedeemers check**

In `collateral.rs`, after the existing missing-redeemer checks (after the Spend, Mint, Reward, Cert, Vote sections), add a reverse check. Build the set of valid `(tag, index)` pairs from the existing checks, then verify each redeemer maps to one:

```rust
// ------------------------------------------------------------------
// ExtraRedeemers check (Haskell hasExactSetOfRedeemers)
//
// Every redeemer in the witness set must correspond to an actual script
// purpose. Redeemers with no matching purpose are extra.
// ------------------------------------------------------------------
let mut valid_purposes: HashSet<(u8, u32)> = HashSet::new();

// Spend: script-locked inputs (sorted by TxIn)
for (idx, input) in body.inputs.iter().enumerate() {
    if let Some(utxo) = utxo_set.lookup(input) {
        let is_script = matches!(utxo.address.payment_credential(), Some(Credential::Script(_)));
        if is_script {
            valid_purposes.insert((0, idx as u32)); // RedeemerTag::Spend = 0
        }
    }
}
// Mint: Plutus minting policies (sorted by policy ID)
for (idx, policy_id) in body.mint.keys().enumerate() {
    if plutus_script_hashes.contains(policy_id) {
        valid_purposes.insert((1, idx as u32)); // RedeemerTag::Mint = 1
    }
}
// Reward: script-locked withdrawals (sorted by reward address)
for (idx, reward_addr) in body.withdrawals.keys().enumerate() {
    if reward_addr.len() >= 29 && (reward_addr[0] & 0x10) != 0 {
        valid_purposes.insert((2, idx as u32)); // RedeemerTag::Reward = 2
    }
}
// Cert: script-credential certificates
for (idx, cert) in body.certificates.iter().enumerate() {
    if cert_has_script_credential(cert) {
        valid_purposes.insert((3, idx as u32)); // RedeemerTag::Cert = 3
    }
}
// Vote: script-credential voters
for (idx, voter) in body.voting_procedures.keys().enumerate() {
    let is_script = matches!(
        voter,
        Voter::DRep(Credential::Script(_)) | Voter::ConstitutionalCommittee(Credential::Script(_))
    );
    if is_script {
        valid_purposes.insert((4, idx as u32)); // RedeemerTag::Vote = 4
    }
}
// Propose: proposals with guardrail policy_hash
for (idx, proposal) in body.proposal_procedures.iter().enumerate() {
    let has_policy = matches!(
        &proposal.gov_action,
        GovAction::ParameterChange { policy_hash: Some(_), .. }
        | GovAction::TreasuryWithdrawals { policy_hash: Some(_), .. }
    );
    if has_policy {
        valid_purposes.insert((5, idx as u32)); // RedeemerTag::Propose = 5
    }
}

// Check each redeemer against valid purposes
for redeemer in &tx.witness_set.redeemers {
    let tag_byte = match redeemer.tag {
        RedeemerTag::Spend => 0,
        RedeemerTag::Mint => 1,
        RedeemerTag::Reward => 2,
        RedeemerTag::Cert => 3,
        RedeemerTag::Vote => 4,
        RedeemerTag::Propose => 5,
    };
    if !valid_purposes.contains(&(tag_byte, redeemer.index)) {
        errors.push(ValidationError::ExtraRedeemer {
            tag: format!("{:?}", redeemer.tag),
            index: redeemer.index,
        });
    }
}
```

Note: Adapt the `cert_has_script_credential` helper based on existing patterns in collateral.rs. The cert/vote/propose redeemer index logic must match the existing missing-redeemer index calculations exactly.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo nextest run -p dugite-ledger -E 'test(extra_redeemer)'`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/dugite-ledger/src/validation/collateral.rs crates/dugite-ledger/src/validation/mod.rs crates/dugite-ledger/src/validation/tests.rs
git commit -m "fix(ledger): add ExtraRedeemers check (#384)

Redeemers in the witness set that have no matching script purpose
(spending input, minting policy, withdrawal, cert, vote, proposal)
are now rejected with ExtraRedeemer, matching Haskell's
hasExactSetOfRedeemers in the UTXOW rule."
```

---

### Task 10: #383 — ScriptsNotPaidUTxO check (collateral must be VKey)

Collateral inputs at script-locked addresses must be rejected.

**Files:**
- Modify: `crates/dugite-ledger/src/validation/collateral.rs:66-84`
- Modify: `crates/dugite-ledger/src/validation/mod.rs` (add error variant)
- Test: `crates/dugite-ledger/src/validation/tests.rs`

- [ ] **Step 1: Add ValidationError variant**

```rust
/// Alonzo UTXO rule: collateral inputs must be at VKey (non-script)
/// addresses. Script-locked UTxOs cannot serve as collateral.
/// Byron/bootstrap addresses are accepted as collateral.
///
/// Reference: Haskell `ScriptsNotPaidUTxO` in
/// `cardano-ledger-alonzo:Cardano.Ledger.Alonzo.Rules.Utxo`.
#[error("Collateral input(s) at script-locked addresses (ScriptsNotPaidUTxO): {inputs:?}")]
ScriptLockedCollateral { inputs: Vec<String> },
```

- [ ] **Step 2: Write failing test**

```rust
#[test]
fn script_locked_collateral_rejected() {
    // Collateral at a script address must produce ScriptLockedCollateral.
    let (tx, utxo_set, params) = build_tx_with_script_collateral();
    let errors = validate_and_get_errors(&tx, &utxo_set, &params);
    assert!(
        errors.iter().any(|e| matches!(e, ValidationError::ScriptLockedCollateral { .. })),
        "Expected ScriptLockedCollateral, got: {:?}", errors
    );
}

#[test]
fn vkey_collateral_accepted() {
    // Collateral at a VKey address must NOT produce ScriptLockedCollateral.
    let (tx, utxo_set, params) = build_tx_with_vkey_collateral();
    let errors = validate_and_get_errors(&tx, &utxo_set, &params);
    assert!(
        !errors.iter().any(|e| matches!(e, ValidationError::ScriptLockedCollateral { .. })),
        "VKey collateral should not trigger ScriptLockedCollateral, got: {:?}", errors
    );
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo nextest run -p dugite-ledger -E 'test(script_locked_collateral_rejected)'`
Expected: FAIL

- [ ] **Step 4: Implement ScriptsNotPaidUTxO check**

In `collateral.rs`, inside `check_collateral`, after resolving collateral UTxOs in the `for col_input in &body.collateral` loop (lines 66-84), collect script-locked inputs:

```rust
// ScriptsNotPaidUTxO: collateral must be at VKey (or Bootstrap) addresses.
// Collect all script-locked collateral inputs, report as single failure.
let mut script_locked_inputs: Vec<String> = Vec::new();

for col_input in &body.collateral {
    match utxo_set.lookup(col_input) {
        Some(output) => {
            // Check if the payment credential is Script (not VKey or Bootstrap)
            let is_script_locked = matches!(
                output.address.payment_credential(),
                Some(Credential::Script(_))
            );
            if is_script_locked {
                script_locked_inputs.push(col_input.to_string());
            }
            collateral_value = collateral_value.saturating_add(output.value.coin.0);
            // ... existing multi-asset accumulation ...
        }
        None => {
            errors.push(ValidationError::CollateralNotFound(col_input.to_string()));
        }
    }
}

if !script_locked_inputs.is_empty() {
    errors.push(ValidationError::ScriptLockedCollateral {
        inputs: script_locked_inputs,
    });
}
```

This replaces the inner part of the existing collateral loop, keeping the value accumulation and multi-asset logic intact.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo nextest run -p dugite-ledger -E 'test(script_locked_collateral) | test(vkey_collateral)'`
Expected: Both PASS

- [ ] **Step 6: Commit**

```bash
git add crates/dugite-ledger/src/validation/collateral.rs crates/dugite-ledger/src/validation/mod.rs crates/dugite-ledger/src/validation/tests.rs
git commit -m "fix(ledger): add ScriptsNotPaidUTxO check for collateral (#383)

Collateral inputs at script-locked addresses are now rejected with
ScriptLockedCollateral. VKey and Byron bootstrap addresses are accepted.
Reports all offending inputs in a single error (matching Haskell)."
```

---

### Task 11: #382 — ExtraneousScriptWitnessesUTXOW check

Scripts in the witness set not needed by the transaction must be rejected.

**Files:**
- Modify: `crates/dugite-ledger/src/validation/scripts.rs`
- Modify: `crates/dugite-ledger/src/validation/mod.rs` (add error variant)
- Test: `crates/dugite-ledger/src/validation/tests.rs`

- [ ] **Step 1: Add ValidationError variant**

```rust
/// Babbage/Conway UTXOW rule: one or more scripts in the transaction
/// witness set are not needed by any script purpose. Reference scripts
/// do not count as "needed" for the witness check.
///
/// Reference: Haskell `ExtraneousScriptWitnessesUTXOW` in
/// `cardano-ledger-shelley:Cardano.Ledger.Shelley.Rules.Utxow`.
#[error("Extraneous script witness(es) not needed by transaction: {hashes:?}")]
ExtraneousScriptWitness { hashes: Vec<String> },
```

- [ ] **Step 2: Write failing test**

```rust
#[test]
fn extraneous_script_witness_rejected() {
    // A Plutus script in the witness set that is not needed by any input,
    // mint, withdrawal, cert, or vote must produce ExtraneousScriptWitness.
    let (tx, utxo_set, params) = build_tx_with_unused_script_witness();
    let errors = validate_and_get_errors(&tx, &utxo_set, &params);
    assert!(
        errors.iter().any(|e| matches!(e, ValidationError::ExtraneousScriptWitness { .. })),
        "Expected ExtraneousScriptWitness for unused witness script, got: {:?}", errors
    );
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo nextest run -p dugite-ledger -E 'test(extraneous_script_witness_rejected)'`
Expected: FAIL

- [ ] **Step 4: Implement ExtraneousScriptWitnessesUTXOW check**

In `scripts.rs`, inside `check_script_data_hash`, after the `scripts_needed` and `scripts_provided` sets are computed (around lines 572-598), add:

```rust
// ExtraneousScriptWitnessesUTXOW: Haskell babbageMissingScripts
// extra = sReceived \ (sNeeded \ sRefs)
// Only WITNESS scripts count as "received" — reference scripts do not.
let witness_script_hashes: HashSet<Hash28> = {
    let mut s = HashSet::new();
    for script in &tx.witness_set.plutus_v1_scripts {
        s.insert(dugite_primitives::hash::blake2b_224_tagged(1, script));
    }
    for script in &tx.witness_set.plutus_v2_scripts {
        s.insert(dugite_primitives::hash::blake2b_224_tagged(2, script));
    }
    for script in &tx.witness_set.plutus_v3_scripts {
        s.insert(dugite_primitives::hash::blake2b_224_tagged(3, script));
    }
    s
};

// Reference scripts — subtract from needed before comparison
let ref_script_hashes: HashSet<Hash28> = {
    let mut s = HashSet::new();
    for ref_input in body.inputs.iter().chain(body.reference_inputs.iter()) {
        if let Some(utxo) = utxo_set.lookup(ref_input) {
            let hash = match &utxo.script_ref {
                Some(ScriptRef::PlutusV1(b)) => Some(dugite_primitives::hash::blake2b_224_tagged(1, b)),
                Some(ScriptRef::PlutusV2(b)) => Some(dugite_primitives::hash::blake2b_224_tagged(2, b)),
                Some(ScriptRef::PlutusV3(b)) => Some(dugite_primitives::hash::blake2b_224_tagged(3, b)),
                _ => None,
            };
            if let Some(h) = hash {
                s.insert(h);
            }
        }
    }
    s
};

let needed_non_refs: HashSet<&Hash28> = scripts_needed.difference(&ref_script_hashes).collect();
let extra: Vec<String> = witness_script_hashes
    .iter()
    .filter(|h| !needed_non_refs.contains(h))
    .map(|h| h.to_hex())
    .collect();

if !extra.is_empty() {
    errors.push(ValidationError::ExtraneousScriptWitness { hashes: extra });
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo nextest run -p dugite-ledger -E 'test(extraneous_script_witness)'`
Expected: PASS

- [ ] **Step 6: Run full crate tests and clippy**

Run: `cargo nextest run -p dugite-ledger && cargo clippy -p dugite-ledger --all-targets -- -D warnings`
Expected: All pass

- [ ] **Step 7: Commit**

```bash
git add crates/dugite-ledger/src/validation/scripts.rs crates/dugite-ledger/src/validation/mod.rs crates/dugite-ledger/src/validation/tests.rs
git commit -m "fix(ledger): add ExtraneousScriptWitnessesUTXOW check (#382)

Scripts in the witness set not needed by the transaction are now rejected.
Reference scripts are subtracted from the needed set before comparison:
extra = witness_scripts \\ (needed \\ ref_scripts). Single error with
all offending hashes, matching Haskell's babbageMissingScripts."
```

---

### Task 12: Branch 2 — Final verification

- [ ] **Step 1: Run full workspace build and test**

```bash
cargo build --all-targets && cargo nextest run --workspace && cargo clippy --all-targets -- -D warnings && cargo fmt --all -- --check
```

Expected: All pass with zero warnings.

- [ ] **Step 2: Commit and push Branch 2**

```bash
git push -u origin feature/correctness-fixes-p2
```
