# Correctness Bug Fixes — Design Spec

**Date:** 2026-04-09  
**Issues:** #375, #376, #377, #379, #380, #381, #382, #383, #384, #385, #387  
**Closed as invalid:** #378 (floor is correct), #386 (ppKeyDeposit is correct)  
**Approach:** Option B — two sequential branches by priority tier  
**Verified against:** Haskell cardano-ledger source via oracle cross-correlation (2026-04-09)

---

## Scope

Fix 11 confirmed-valid correctness bugs. 2 of the original 13 were invalidated by
Haskell oracle verification:

- **#378** — Haskell's `tierRefScriptFee` uses `floor` (confirmed in `Conway/Tx.hs`).
  Dugite's current truncation and the comment at `scripts.rs:363` are both correct.
- **#386** — Haskell's `ConwayRegCert` handler stores `ppKeyDepositCompact` (the current
  protocol parameter), not the cert's declared deposit field. The cert's deposit is
  validated against `ppKeyDeposit` then discarded. Dugite's `deposit: _` pattern is correct.

Both issues should be closed on GitHub.

Remaining 11 bugs are grouped into two branches by severity.

---

## Branch 1: P0 + P1 Critical Fixes (6 bugs)

Target files: `state/apply.rs`, `state/certificates.rs`, `state/epoch.rs`,
`validation/datum.rs`, `validation/mod.rs`, `validation/conway.rs`

### #377 + #387 — Block body size check (apply.rs)

**Haskell reference:** `Shelley/Rules/Bbody.hs` — `validateBlockBodySize`

The Haskell BBODY rule performs a strict equality check:
```
actualSize == sizeInBlockHeader  ?! WrongBlockBodySizeBBODY (Mismatch actual claimed)
```
where `actualSize = bBodySize protVer (blockBody block)` (re-serialized CBOR length).
The `ppMaxBBSize` parameter is checked only in `chainChecks` at the consensus layer
(header-claimed size ≤ max), not in BBODY.

Dugite's current check (`apply.rs:144-155`) compares `header.body_size > max_block_body_size`
and only logs a warning. This is wrong in two ways: wrong comparison target, and no error.

**Fix:** Replace the existing body size block in `ValidateAll` mode with:
1. **Header/body consistency (BBODY rule)**: compute actual serialized body size via
   CBOR re-encoding and compare with `block.header.body_size` for equality. Return
   `Err(LedgerError::WrongBlockBodySize { actual, claimed })` on mismatch.
2. The `max_block_body_size` comparison is removed from BBODY — it belongs in the
   consensus-layer header validation (outside scope of this PR, but the incorrect
   BBODY check must be removed to match Haskell).

The current warn-only code is deleted entirely.

**Note:** Conway BBODY also adds `BodyRefScriptsSizeTooBig` and `HeaderProtVerTooHigh`
checks. These are out of scope for this PR but tracked for future work.

### #381 — Committee resignation permanence (certificates.rs + validation/conway.rs)

**Haskell reference:** `Conway/Rules/GovCert.hs` — `checkAndOverwriteCommitteeMemberState`

Haskell's helper `checkAndOverwriteCommitteeMemberState` is shared by both
`ConwayAuthCommitteeHotKey` and `ConwayResignCommitteeColdKey`. It checks whether the
cold credential already has a `CommitteeMemberResigned` entry; if so, it fires
`ConwayCommitteeHasPreviouslyResigned` and **rejects the certificate entirely** — for
both hot-auth and resign attempts. There is no code path that removes from the resigned set.

Dugite has two problems:
1. **State application** (`certificates.rs:368`): `gov.committee_resigned.remove(&cold_key)`
   incorrectly allows re-authorization after resignation.
2. **Missing validation**: no Phase-1 check rejects `CommitteeHotAuth` for resigned members.

**Fix (two parts):**
1. Remove `gov.committee_resigned.remove(&cold_key)` at `certificates.rs:368` and the
   second occurrence at line ~974 (dry-run path).
2. Add validation in `validation/conway.rs`: when processing `CommitteeHotAuth` or
   `CommitteeColdResign`, check if the cold credential is already in
   `governance.committee_resigned`. If so, emit
   `ValidationError::CommitteeHasPreviouslyResigned { cold_credential }`.

### #380 — Treasury donation flush ordering (epoch.rs)

**Haskell reference:** `Conway/Rules/Epoch.hs` — `epochTransition` steps 1–13

Verified step ordering from the Haskell EPOCH rule:
1. SNAP — rotate snapshots
2. POOLREAP — retire pools, return deposits
3. Extract ratification result (already pulsed)
4. `applyEnactedWithdrawals` — subtract enacted treasury withdrawals from treasury
5. Apply enacted proposals to proposal set
6. Update GovState
7. `returnProposalDeposits` — refund deposits for removed proposals; unclaimed → `unclaimed`
8. Update certState
9. **`treasury += utxosDonation + fold unclaimed`** — donations flushed HERE
10. Clear `utxosDonationL`, update `utxosGovStateL`

Dugite flushes `pending_donations` at Step 0 (before SNAP). This is wrong — donations
must be added to the treasury after enacted withdrawals have been subtracted and after
unclaimed proposal deposits are computed.

**Fix:** Move the pending-donation flush from the top of `process_epoch_transition` to
after governance enactment and proposal deposit return. Combine donations with unclaimed
proposal deposits in a single addition to the treasury. The `pending_donations` field is
read from its pre-epoch value (matching `utxoState0 ^. utxosDonationL` in Haskell).

### #379 — CIP-0069: PlutusV3 datum exemption (datum.rs)

**Haskell reference:** `Alonzo/UTxO.hs` — `getInputDataHashesTxBody`

The exact Haskell guard is:
```haskell
NoDatum
  | Just lang <- spendingPlutusScriptLanguage addr
  , lang < PlutusV3 ->  -- V1/V2 only: add to "missing datum" set
```

PlutusV3 inputs with `NoDatum` fall through to `_ -> ans` (no error). V1/V2 inputs
with `NoDatum` produce `UnspendableUTxONoDatumHash`.

Dugite's `datum.rs` does not implement `UnspendableUTxONoDatumHash` at all — script-locked
inputs with `OutputDatum::None` are silently accepted for any Plutus version. This is
overly permissive for V1/V2 and happens to be correct for V3 by accident.

**Fix in `check_datum_witnesses`:** When iterating script-locked spending inputs, if
`utxo.datum == OutputDatum::None`:
1. Resolve the script hash from the UTxO address's payment credential.
2. Look up the script's language version from `scripts_provided` (witness set + ref scripts).
3. If V1 or V2 (or script not found): emit `ValidationError::UnspendableUTxONoDatumHash`.
4. If V3: no error (CIP-0069 exemption).

The function signature needs an additional parameter for script version lookup (either a
`HashMap<Hash28, u8>` or the UTxO set + witness set for inline resolution).

### #376 — Missing ConwayWdrlNotDelegatedToDRep check (validation/mod.rs)

**Haskell reference:** `Conway/Rules/Ledger.hs` — `validateWithdrawalsDelegated`

Active when `hardforkConwayBootstrapPhase` is false, i.e., PV major ≠ 9 → **PV ≥ 10**.
The check is in the LEDGER rule, executed before CERTS and GOV subrules.

Key semantics from oracle verification:
- Only **KeyHash** reward accounts are checked (`credKeyHash` returns Nothing for Script).
- "Active DRep delegation" means any non-Nothing `dRepDelegationAccountStateL` — the
  DRep itself need NOT be active or registered. Values like `AlwaysAbstain` and
  `AlwaysNoConfidence` satisfy the check.
- Uses the `certState` **before** the current transaction's certificates are applied,
  so a tx can simultaneously unregister and withdraw.

**Fix:** Add `check_withdrawal_drep_delegation` in validation pipeline. When
`params.protocol_version.major >= 10`: for each withdrawal, decode the reward address.
Skip script-credential accounts (header byte bit 4 set). For KeyHash accounts, look up
the credential in `governance.vote_delegations`. If absent, emit
`ValidationError::WdrlNotDelegatedToDRep { credential_hash }`.

### #375 — Conway deregistration zero balance enforcement (validation path audit)

**Haskell reference:** `Conway/Rules/Deleg.hs` — `ConwayUnRegCert` branch

The oracle confirmed:
- `StakeKeyHasNonZeroAccountBalanceDELEG` fires when `balanceAccountStateL /= mempty`.
- The refund is validated against `depositAccountStateL` only (not reward balance).
- Check ordering: invalid refund check fires first, then zero-balance check.

Dugite already has the `StakeKeyHasNonZeroBalance` check in `validation/mod.rs:805-831`
covering both tag 1 and tag 8. However, the state application comment in
`certificates.rs:169` ("deregistration returns remaining reward balance as part of the
deposit refund") is misleading.

**Fix:** Update the misleading comment in `certificates.rs` to accurately describe that
the reward balance must be zero at deregistration time (Phase-1 validation enforces this).
Add an inline test that confirms the validation check fires for Conway tag 8 with
non-zero balance, ensuring the existing check cannot regress.

---

## Branch 2: P2 Missing-Check Additions (5 bugs)

Target files: `validation/conway.rs`, `validation/collateral.rs`, `validation/scripts.rs`

### #385 — PParam well-formedness at proposal submission (validation/conway.rs)

**Haskell reference:** `Conway/Rules/Gov.hs` — `actionWellFormed` + `Conway/PParams.hs` — `ppuWellFormed`

The oracle clarified: `ppuWellFormed` checks **nonzero values** for specific fields,
NOT ratio bounds. The complete check list:
- `maxBBSize ≠ 0`, `maxTxSize ≠ 0`, `maxBHSize ≠ 0`, `maxValSize ≠ 0`
- `collateralPercentage ≠ 0`
- `committeeMaxTermLength ≠ EpochInterval 0`, `govActionLifetime ≠ EpochInterval 0`
- `poolDeposit ≠ 0`, `govActionDeposit ≠ 0`, `dRepDeposit ≠ 0`
- `coinsPerUTxOByte ≠ 0` (except during PV 9 bootstrap)
- `nOpt ≠ 0` (PV ≥ 11 only)
- `ppu ≠ emptyPParamsUpdate` (at least one field must be set)

Only applies to `ParameterChange` proposals; other action types return `True`.

**Fix:** Add `check_pparam_update_well_formed` in `validation/conway.rs` that implements
the above nonzero checks for `ParameterChange` proposals. Emit
`ValidationError::MalformedProposal` on failure. Remove (or keep as defense-in-depth)
the existing `validate_threshold` calls at enactment time in `protocol_params.rs`.

### #384 — Missing ExtraRedeemers check (validation/collateral.rs)

**Haskell reference:** `Alonzo/Rules/Utxow.hs` — `hasExactSetOfRedeemers`

The oracle confirmed this uses `extSymmetricDifference` for exact set equality:
- Extra redeemers (in witness but no matching purpose) → `ExtraRedeemers`
- Missing redeemers (purpose exists but no redeemer) → `MissingRedeemers`
Both can fire in the same transaction.

**Fix:** After building the set of `(tag, index)` pairs that require redeemers (spending
script inputs, Plutus minting policies, script withdrawals, script certs, script voters,
guardrail proposals), verify that every redeemer in the witness set maps to an entry in
this set. Emit `ValidationError::ExtraRedeemer { tag, index }` for any extra. Collect
all extras before emitting (matching Haskell's `NonEmpty` list behavior).

### #383 — Missing ScriptsNotPaidUTxO check (validation/collateral.rs)

**Haskell reference:** `Alonzo/Rules/Utxo.hs` — `validateScriptsNotPaidUTxO`

The oracle clarified:
- The failure carries a **map of all offending collateral inputs**, not per-input failures.
- Bootstrap (Byron) addresses are valid collateral (`vKeyLocked (AddrBootstrap _) = True`).
- The check only fires when the transaction has non-empty redeemers.

**Fix:** In `check_collateral`, after resolving collateral UTxOs, collect all inputs
whose payment credential is `Credential::Script` (NOT `Credential::VerificationKey` and
NOT bootstrap). If the set is non-empty, emit a single
`ValidationError::ScriptLockedCollateral { inputs: Vec<String> }` carrying all offending
inputs. Ensure Byron/bootstrap addresses pass the check.

### #382 — Missing ExtraneousScriptWitnessesUTXOW check (validation/scripts.rs)

**Haskell reference:** `Babbage/Rules/Utxow.hs` — `babbageMissingScripts`

The oracle clarified: `extra = sReceived \ (sNeeded \ sRefs)`. Reference scripts are
subtracted from `sNeeded` before comparing, so a script provided inline as a witness
that is only needed via a reference script IS considered extraneous.

**Fix:** After computing `scripts_needed` and `scripts_provided` (witness-set scripts only,
NOT reference scripts), compute `needed_non_refs = scripts_needed - ref_script_hashes`.
Then `extra = scripts_provided_in_witness - needed_non_refs`. Emit
`ValidationError::ExtraneousScriptWitness { hashes: Vec<String> }` for any extras
(single error carrying all hashes, matching Haskell's `NonEmptySet`).

---

## New Error Variants Required

Branch 1 additions to `ValidationError`:
- `WrongBlockBodySize { actual: u32, claimed: u32 }` (LedgerError, not ValidationError)
- `UnspendableUTxONoDatumHash { input: String }`
- `WdrlNotDelegatedToDRep { credential_hash: String }`
- `CommitteeHasPreviouslyResigned { cold_credential: String }`

Branch 2 additions:
- `MalformedProposal { reason: String }`
- `ExtraRedeemer { tag: String, index: u32 }`
- `ScriptLockedCollateral { inputs: Vec<String> }`
- `ExtraneousScriptWitness { hashes: Vec<String> }`

---

## Testing

Each fix must have a focused inline unit test:

**Branch 1:**
- Block body size: test header/body mismatch returns error; test that warn-only path is gone
- Committee resignation: test hot-auth after resign is rejected at validation; test state doesn't clear resigned set
- Treasury ordering: test treasury balance after epoch with both donations and enacted withdrawals
- CIP-0069: test V3 NoDatum allowed, V1/V2 NoDatum rejected with `UnspendableUTxONoDatumHash`
- DRep delegation: test withdrawal accepted with delegation (including AlwaysAbstain), rejected without at PV10; test script-credential withdrawals bypass the check
- Zero balance (regression): test Conway tag 8 deregistration with non-zero balance produces `StakeKeyHasNonZeroBalance`

**Branch 2:**
- PParam well-formed: test zero-value fields in ParameterChange rejected; test empty update rejected; test non-ParameterChange proposals pass
- Extra redeemers: test redeemer with no matching purpose rejected; test both extra and missing can fire together
- Collateral VKey: test script-locked collateral rejected; test Byron bootstrap address accepted
- Extraneous scripts: test unused witness scripts rejected; test reference-only scripts don't count as "needed" for witness comparison

---

## Issues to Close as Invalid

- **#378** — `tierRefScriptFee`: Haskell uses `floor` (`Coin $ floor (acc + ...)`).
  Dugite's truncation is correct. Close with reference to `Conway/Tx.hs`.
- **#386** — `ConwayRegCert` deposit: Haskell stores `ppKeyDepositCompact`, not cert field.
  Dugite's `deposit: _` pattern is correct. Close with reference to `Conway/Rules/Deleg.hs`.

---

## Non-Goals

- No changes to the wire format or CBOR encoding
- No changes to CLI, node, or storage layers
- No architectural refactoring beyond what is needed for the fixes
- Conway BBODY additions (`BodyRefScriptsSizeTooBig`, `HeaderProtVerTooHigh`) — tracked separately
- `maxBlockBodySize` consensus-layer check — belongs in header validation, not BBODY
