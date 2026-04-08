# Inline Unit Tests for Ledger Validation Core Modules

**Date:** 2026-04-08
**Issue:** #337
**Status:** Approved (cross-validated against Haskell cardano-ledger)

## Summary

Add inline `#[cfg(test)] mod tests` blocks to 10 source files in `dugite-ledger/src` that currently have zero unit tests. These are the most correctness-critical paths in the codebase — silent regressions here could cause chain divergence from Haskell cardano-node.

## Scope

### In Scope (10 files, ~120 tests)

**Tier 1 — Highest impact (~5,500 lines):**

| File | Lines | Tests | Focus |
|------|-------|-------|-------|
| `state/apply.rs` | 1,330 | ~15 | Block application, era dispatch, epoch detection, limits |
| `state/certificates.rs` | 1,225 | ~15 | Certificate processing, deposits, pointer tracking |
| `state/epoch.rs` | 1,098 | ~14 | Epoch transitions, rewards, snapshots, retirements |
| `validation/phase1.rs` | 1,095 | ~18 | All Phase-1 validation rules (UTXOW + UTXO layers) |
| `validation/collateral.rs` | 798 | ~12 | Collateral checks, redeemer indices, ex-units |

**Tier 2 — Important but smaller (~1,500 lines):**

| File | Lines | Tests | Focus |
|------|-------|-------|-------|
| `validation/scripts.rs` | 854 | ~12 | Native scripts, hash computation, tiered fees |
| `state/snapshot.rs` | 300 | ~8 | Save/load, checksums, legacy compat |
| `validation/datum.rs` | 199 | ~9 | Datum witness completeness, CIP-0069 |
| `state/protocol_params.rs` | 194 | ~6 | Param updates, threshold validation |
| `validation/conway.rs` | 177 | ~8 | Era gating, deposit/refund calculation |

### Out of Scope

- `eras/conway.rs` (27 lines) — constructor only
- `eras/shelley.rs` (15 lines) — constructor only
- `rules/shelley.rs` (9 lines) — returns a string

## Test Organization

- Inline `#[cfg(test)] mod tests` at the bottom of each source file
- Idiomatic Rust — tests live next to the code they test
- Can access private functions directly (no `pub(crate)` needed)
- Reuse existing helpers from `state/tests.rs` and `validation/tests.rs` where they exist as `pub(crate)`
- Create minimal local helpers within each test module as needed

## Cross-Validation Notes

All test conditions below have been verified against the Haskell cardano-ledger source
(IntersectMBO/cardano-ledger). Key Haskell references:

- BBODY: `eras/alonzo/impl/.../Alonzo/Rules/Bbody.hs`, `eras/conway/impl/.../Conway/Rules/Bbody.hs`
- LEDGER: `eras/conway/impl/.../Conway/Rules/Ledger.hs`
- UTXOW: `eras/conway/impl/.../Conway/Rules/Utxow.hs`
- UTXO: `eras/babbage/impl/.../Babbage/Rules/Utxo.hs`
- DELEG: `eras/conway/impl/.../Conway/Rules/Deleg.hs`
- POOL: `eras/shelley/impl/.../Shelley/Rules/Pool.hs`
- GOVCERT: `eras/conway/impl/.../Conway/Rules/GovCert.hs`
- NEWEPOCH: `eras/conway/impl/.../Conway/Rules/NewEpoch.hs`
- EPOCH: `eras/conway/impl/.../Conway/Rules/Epoch.hs`
- SNAP: `eras/shelley/impl/.../Shelley/Rules/Snap.hs`

## Test Plan by Module

### state/apply.rs (~15 tests)

Haskell references: BBODY (block-level), LEDGERS (tx sequencing), UTXOS (state mutation).

| Test | What it verifies | Haskell rule |
|------|-----------------|--------------|
| `test_apply_byron_block` | Byron era block applies correctly, UTxO updated | BBODY/LEDGERS |
| `test_apply_shelley_block` | Shelley+ block applies, certificates/withdrawals processed | BBODY/LEDGERS |
| `test_apply_empty_block` | Block with no transactions applies cleanly | BBODY |
| `test_invalid_tx_collateral_consumed` | `is_valid: false` tx: collateral consumed + return added; regular I/O, certs, withdrawals, governance all skipped | UTXOS `updateUTxOStateByTxValidity` |
| `test_epoch_transition_detected` | Block in new epoch triggers `process_epoch_transition` | TICK/NEWEPOCH |
| `test_multi_epoch_gap` | Block skipping multiple epochs triggers transitions for each | TICK |
| `test_body_size_header_mismatch` | Block body actual size ≠ header-claimed size → rejected (equality check, not <= maxBBSize) | BBODY `WrongBlockBodySizeBBODY` |
| `test_ref_script_size_per_tx_limit` | Single valid tx with >204,800 bytes ref scripts rejected (only for is_valid=true) | LEDGER `ConwayTxRefScriptsSizeTooBig` |
| `test_ref_script_size_per_block_limit` | Block total ref scripts >1,048,576 bytes rejected | BBODY `BodyRefScriptsSizeTooBig` |
| `test_block_ex_units_memory_exceeded` | Sum of all tx ExUnits across block exceeds `maxBlockExUnits.mem` | BBODY `TooManyExUnits` |
| `test_block_ex_units_steps_exceeded` | Sum of all tx ExUnits across block exceeds `maxBlockExUnits.steps` | BBODY `TooManyExUnits` |
| `test_multiple_txs_sequential_utxo` | Txs applied left-to-right via foldM; each sees prior tx's UTxO changes | LEDGERS `foldM` |
| `test_conway_pointer_stake_structural_exclusion` | Conway `InstantStake` has no ptrStake field; `StakeRefPtr` discarded structurally | Conway `addConwayInstantStake` |
| `test_apply_only_mode_skips_validation` | `BlockValidationMode::ApplyOnly` (Haskell `ValidateNone`) executes state changes but skips predicate checks | `applyBlockNoValidaton` |
| `test_certificate_processing_order` | Certificates processed in tx order within block | LEDGERS `foldM` |

### state/certificates.rs (~15 tests)

Haskell references: DELEG (stake), POOL (pools), GOVCERT (DReps/committee).

| Test | What it verifies | Haskell rule |
|------|-----------------|--------------|
| `test_stake_registration` | Creates reward_account with 0 balance, stores per-credential deposit | DELEG `ConwayRegCert` |
| `test_stake_deregistration` | Removes from delegations, reward_accounts, vote_delegations; refunds stored per-credential deposit | DELEG `ConwayUnRegCert` |
| `test_stake_delegation` | Updates delegations map to target pool | DELEG `ConwayDelegCert` |
| `test_pool_registration` | Creates pool_params entry, charges `poolDeposit` | POOL |
| `test_pool_reregistration_free` | Existing pool: params staged to `psFutureStakePoolParams`, pending retirement cancelled, no deposit charged | POOL (re-registration) |
| `test_pool_retirement` | Adds to pending retirements, processed at epoch boundary | POOL |
| `test_conway_stake_registration` | Tag 7: inline deposit amount stored per-credential (may differ from current `keyDeposit`) | DELEG |
| `test_conway_stake_deregistration_requires_zero_balance` | Tag 8: reward balance must be zero; refund is deposit only (NOT deposit+rewards) | DELEG `StakeKeyHasNonZeroAccountBalanceDELEG` |
| `test_drep_registration` | Creates DRep entry, charges `dRepDeposit` (separate from `keyDeposit`, PParams pos 27/key 31) | GOVCERT `ConwayRegDRep` |
| `test_drep_unregistration` | Removes DRep, refunds stored per-credential DRep deposit | GOVCERT `ConwayUnRegDRep` |
| `test_drep_update` | Updates DRep metadata without deposit change | GOVCERT `ConwayUpdateDRep` |
| `test_vote_delegation` | Updates vote_delegations map | DELEG `ConwayDelegCert` |
| `test_committee_hot_auth` | Authorizes hot key for cold committee member (tag 14) | GOVCERT `ConwayAuthCommitteeHotKey` |
| `test_committee_cold_resign` | Records committee member resignation; permanent — cannot re-authorize (tag 15) | GOVCERT `ConwayResignCommitteeColdKey` |
| `test_pointer_address_tracking` | Registration certs (tags 0, 7, 11, 12, 13) create pointer entry (slot/tx_index/cert_index) | Pointer map |

### state/epoch.rs (~14 tests)

Haskell references: NEWEPOCH, EPOCH (internal sub-rules), SNAP, POOLREAP.

**Correct NEWEPOCH sequence** (verified against Haskell):
1. Complete PulsingRewUpdate (force reward pulsing to finish)
2. **applyRUpd** — distribute rewards to accounts (BEFORE EPOCH)
3. **EPOCH sub-rule:**
   - 3a. SNAP — rotate snapshots, capture new mark
   - 3b. POOLREAP — retire scheduled pools
   - 3c. RATIFY — extract governance ratification result
   - 3d-3f. Apply treasury withdrawals, enact proposals, return deposits
   - 3g. Apply donations + unclaimed rewards to treasury
   - 3h. HARDFORK sub-rule
4. Set `nesPd` = `ssStakeMarkPoolDistr` from PRE-EPOCH state
5. Reset block counts: `nesBprev = old nesBcur`, `nesBcur = empty`

**Correct snapshot rotation:** go ← set, set ← mark, mark ← newly computed

| Test | What it verifies | Haskell rule |
|------|-----------------|--------------|
| `test_snapshot_rotation_direction` | go←set, set←mark, new mark computed (NOT mark→set→go) | SNAP |
| `test_rewards_applied_before_snap` | applyRUpd fires BEFORE EPOCH/SNAP; credited rewards visible in new mark snapshot | NEWEPOCH step 2 |
| `test_fee_capture_at_snap` | `utxosFees` read into mark snapshot's ssFee during SNAP, then reset | SNAP |
| `test_reward_distribution_formula` | `rewardPot = floor(rho*reserves) + ssFee(go)`; `deltaT1 = floor(tau*rewardPot)`; pools get `R = rewardPot - deltaT1`; undistributed R → reserves (deltaR2) | `RewardUpdate` |
| `test_unregistered_rewards_to_treasury` | Rewards for deregistered accounts go to treasury | applyRUpd |
| `test_pool_retirement_processing` | Pending retirements for current epoch applied, pool removed from pool_params | POOLREAP |
| `test_pool_retirement_missing_reward_account` | Pool deposit goes to treasury if reward account no longer exists | POOLREAP |
| `test_treasury_donation_in_epoch` | `utxosDonation + unclaimed` added to treasury inside EPOCH (step 3g, NOT first step) | EPOCH step 10 |
| `test_governance_ratification_after_snap` | RATIFY result extracted AFTER SNAP (step 3c), not before | EPOCH ordering |
| `test_genesis_epoch_transition` | Epoch 0→1: all snapshots empty, no rewards distributed, bprev empty | NEWEPOCH genesis |
| `test_bprev_block_count_rotation` | `nesBprev = old nesBcur`, `nesBcur = empty` at epoch boundary | NEWEPOCH step 5 |
| `test_nesPd_from_pre_epoch_mark` | Pool distribution set from ssStakeMarkPoolDistr BEFORE EPOCH overwrites mark | NEWEPOCH step 4 |
| `test_ss_fee_from_go_snapshot` | Reward calculation uses go snapshot's ssFee (2 epochs old), not current | RewardUpdate |
| `test_reward_params_from_go_epoch` | rho/tau come from go snapshot's epoch PParams, not current | RewardUpdate |

### state/protocol_params.rs (~6 tests)

| Test | What it verifies |
|------|-----------------|
| `test_apply_partial_update` | Only specified fields updated |
| `test_apply_full_update` | All fields updated |
| `test_noop_update` | All-None update leaves params unchanged |
| `test_threshold_valid` | Valid rational in [0,1] accepted |
| `test_threshold_num_exceeds_denom` | Numerator > denominator rejected |
| `test_threshold_zero_denominator` | Zero denominator rejected |

### state/snapshot.rs (~8 tests)

| Test | What it verifies |
|------|-----------------|
| `test_save_load_roundtrip` | Save then load produces identical state |
| `test_magic_bytes` | Saved file starts with `DUGT` magic |
| `test_checksum_verification` | Correct checksum passes validation |
| `test_corrupted_data_detected` | Modified bytes cause checksum mismatch |
| `test_size_limit_enforcement` | Oversized file rejected (OOM prevention) |
| `test_legacy_format_loading` | Old format (no magic) still loads |
| `test_version_in_header` | Current SNAPSHOT_VERSION written to header |
| `test_atomic_write` | Writes to `.tmp` then renames |

### validation/phase1.rs (~18 tests)

Haskell pipeline per transaction: LEDGER → CERTS → GOV → UTXOW → UTXO → UTXOS.
Tests cover all three layers where Phase-1 checks live.

| Test | What it verifies | Haskell failure |
|------|-----------------|----------------|
| `test_valid_tx_passes` | Well-formed transaction passes all rules | — |
| `test_no_inputs` | At least one input required | `InputSetEmptyUTxO` (UTXO) |
| `test_all_inputs_must_exist` | inputs ∪ collateralInputs ∪ referenceInputs must all exist in UTxO | `BadInputsUTxO` (UTXO) |
| `test_value_not_conserved_ada` | `consumed = inputs + withdrawals + refunds`; `produced = outputs + fee + deposits + mint` | `ValueNotConservedUTxO` (UTXO) |
| `test_value_not_conserved_multiasset` | Multi-asset conservation (mint on produced side) | `ValueNotConservedUTxO` (UTXO) |
| `test_fee_too_small` | `fee >= txFeeFixed + txFeePerByte*txSize + tierRefScriptFee + exUnitFee` | `FeeTooSmallUTxO` (UTXO) |
| `test_output_below_min_utxo` | Output below minimum UTxO value | `BabbageOutputTooSmallUTxO` (UTXO) |
| `test_output_value_too_large` | CBOR size exceeds max_val_size | `OutputTooBigUTxO` (UTXO) |
| `test_network_id_mismatch` | Output network ID wrong | `WrongNetwork` (UTXO) |
| `test_tx_size_too_large` | Transaction size limit | `MaxTxSizeUTxO` (UTXO) |
| `test_validity_interval_bounds` | `invalidBefore <= slot < invalidHereafter` (single check covers TTL + start) | `OutsideValidityIntervalUTxO` (UTXO) |
| `test_required_signer_missing` | Required signer without matching vkey witness | `MissingVKeyWitnessesUTXOW` (UTXOW) |
| `test_ref_inputs_must_be_disjoint` | Reference inputs must not overlap regular inputs | `BabbageNonDisjointRefInputs` (UTXO) |
| `test_missing_script_witness` | Script hash referenced but not provided in witness or ref inputs | `MissingScriptWitnessesUTXOW` (UTXOW) |
| `test_extraneous_script_witness` | Script supplied but not needed by this tx | `ExtraneousScriptWitnessesUTXOW` (UTXOW) |
| `test_tx_ex_units_too_big` | Sum of all redeemer ExUnits > `maxTxExUnits` | `ExUnitsTooBigUTxO` (UTXO) |
| `test_auxiliary_data_hash_mismatch` | Aux data hash present but doesn't match / aux data missing | `MissingTxBodyMetadataHash` / `MissingTxMetadata` (UTXOW) |
| `test_mint_without_policy_script` | Minting policy has no matching script | `MissingScriptWitnessesUTXOW` (UTXOW) |

Note: duplicate inputs (Rule 1b) are structurally rejected at CBOR deserialization (Set with tag 258),
not by a named ledger predicate failure. This is tested at the serialization layer, not here.

### validation/collateral.rs (~12 tests)

Collateral checks gate on **redeemers map being non-empty** (not "has Plutus scripts").
The pass/fail comparison uses cross-multiplication: `bal * 100 >= fee * collPerc` (not ceiling division).

| Test | What it verifies | Haskell failure |
|------|-----------------|----------------|
| `test_valid_collateral` | Tx with redeemers + sufficient collateral passes | — |
| `test_no_collateral_inputs` | Empty collateral inputs when redeemers present | `NoCollateralInputs` |
| `test_too_many_collateral_inputs` | Exceeds `maxCollateralInputs` | `TooManyCollateralInputs` |
| `test_insufficient_collateral_cross_multiply` | `bal * 100 < fee * collPerc` fails (exact integer comparison, no division) | `InsufficientCollateral` |
| `test_collateral_return_multiasset` | Babbage+ collateral return subtracts correctly | `collAdaBalance` |
| `test_non_ada_in_net_collateral` | Net collateral (inputs minus return) contains tokens | `CollateralContainsNonADA` |
| `test_total_collateral_field_mismatch` | Declared `totalCollateral` ≠ computed balance | `IncorrectTotalCollateralField` |
| `test_collateral_must_be_vkey` | Collateral inputs at script addresses rejected | `ScriptsNotPaidUTxO` |
| `test_ex_units_memory_exceeded` | Tx redeemer sum > `maxTxExUnits.mem` | `ExUnitsTooBigUTxO` |
| `test_ex_units_steps_exceeded` | Tx redeemer sum > `maxTxExUnits.steps` | `ExUnitsTooBigUTxO` |
| `test_redeemer_index_out_of_bounds` | Spend redeemer index ≥ input count | `ExtraRedeemers` (UTXOW) |
| `test_vote_redeemer_ordering` | Vote index follows `Voter` Ord: CC(Script) < CC(Key) < DRep(Script) < DRep(Key) < SPO(Key) | Conway `getConwayScriptsNeeded` |

Note: "scripts needed" checks (missing/extra redeemers) are UTXOW-level (`hasExactSetOfRedeemers`),
not inside the UTXO collateral block. Tests for `MissingRedeemers`/`ExtraRedeemers` belong in phase1.rs.

### validation/scripts.rs (~12 tests)

| Test | What it verifies | Haskell reference |
|------|-----------------|-------------------|
| `test_native_script_pubkey_match` | ScriptPubkey with matching signer → true | `evalNativeScript` |
| `test_native_script_pubkey_no_match` | ScriptPubkey without matching signer → false | `evalNativeScript` |
| `test_native_script_all` | ScriptAll: all sub-scripts must be true | `evalNativeScript` |
| `test_native_script_any` | ScriptAny: at least one sub-script true | `evalNativeScript` |
| `test_native_script_n_of_k` | ScriptNOfK: at least N of K true | `evalNativeScript` |
| `test_native_script_time_locks` | InvalidBefore: `slot >= n` (inclusive). InvalidHereafter: `slot < n` (strictly less) | Allegra timelocks |
| `test_script_hash_type_tags` | Native: `blake2b-224(0x00 \|\| CBOR(script))`. Plutus V1/V2/V3: `blake2b-224(0x0N \|\| flat_bytes)` (raw, not CBOR-wrapped) | `hashScript` |
| `test_available_scripts_from_witnesses` | Collects hashes from witness set (native + Plutus V1/V2/V3) | `scriptsProvided` |
| `test_available_scripts_from_ref_inputs` | Collects hashes from reference input UTxOs (both spending + reference inputs) | `scriptsProvided` |
| `test_tiered_fee_single_tier` | ≤25,600 bytes uses base rate (`minFeeRefScriptCostPerByte` PParams pos 29/key 33) | `tierRefScriptFee` |
| `test_tiered_fee_multiple_tiers` | >25,600 bytes: 6/5 rational multiplier per tier (hardcoded stride=25600, multiplier=6/5). Result is ceiling of exact rational sum. | `tierRefScriptFee` |
| `test_min_fee_computation` | `txFeeFixed + txFeePerByte*txSize + tierRefScriptFee + prSteps*totalSteps + prMem*totalMem` | `getMinFeeTxUtxo` |

### validation/datum.rs (~9 tests)

| Test | What it verifies | Haskell failure |
|------|-----------------|----------------|
| `test_script_input_datum_present` | PlutusV1/V2 script-locked input with DatumHash: matching datum in witness passes | `MissingRequiredDatums` |
| `test_script_input_datum_missing` | PlutusV1/V2 script-locked input with DatumHash: no datum in witness → error | `MissingRequiredDatums` |
| `test_inline_datum_no_witness_needed` | InlineDatum: pulled from UTxO directly, no witness entry required | `getBabbageSpendingDatum` |
| `test_non_script_input_no_datum` | Non-script (VKey) input needs no datum | — |
| `test_extra_datum_is_hard_error` | Witness datum not in needed set → hard predicate failure (not warning) | `NotAllowedSupplementalDatums` |
| `test_output_datum_hash_supplemental` | Output DatumHash allowed as supplemental in witness set | `getBabbageSupplementalDataHashes` |
| `test_ref_input_datum_supplemental_only` | Reference input DatumHash is supplemental (allowed in witness) but CANNOT satisfy spending input requirement | `getBabbageSupplementalDataHashes` |
| `test_multiple_script_inputs` | Multiple script inputs each need their own datum hash in witness | `MissingRequiredDatums` |
| `test_plutusv3_no_datum_required` | CIP-0069: PlutusV3 spending inputs do not require datum (no `UnspendableUTxONoDatumHash`) | Conway `getInputDataHashesTxBody` |

### validation/conway.rs (~8 tests)

Conway-only certificate types: **12** (not 13). Tags: 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18.

| Test | What it verifies | Haskell reference |
|------|-----------------|-------------------|
| `test_conway_cert_in_conway_era` | All 12 Conway cert types allowed when PV ≥ 9 | `conwayTxCertGovCert` |
| `test_conway_cert_in_pre_conway_era` | Conway certs rejected when PV < 9 | Era gating |
| `test_governance_features_era_gated` | Voting procedures + proposal procedures rejected pre-Conway | Era gating |
| `test_deposit_new_key_registration` | New key registration charges current `keyDeposit` | DELEG |
| `test_deposit_new_drep_registration` | New DRep charges `dRepDeposit` (PParams pos 27/key 31, separate from keyDeposit) | GOVCERT |
| `test_deposit_pool_reregistration_free` | Existing pool re-reg: no deposit, checks `pool_params.contains_key()` | POOL |
| `test_refund_deregistration` | Key/DRep deregistration refunds stored per-credential deposit amount | DELEG/GOVCERT |
| `test_per_credential_deposit_map` | After governance changes keyDeposit, old credentials refund at their original rate | Per-credential tracking |

## Test Infrastructure

### Existing Helpers to Reuse
- `make_test_block(slot, block_no, prev_hash, txs)` from `state/tests.rs`
- `make_simple_utxo_set()` and `make_simple_tx()` from `validation/tests.rs`
- `add_stake_utxo(state, cred, amount)` from `state/tests.rs`
- PropTest strategies from `tests/strategies.rs` (if needed)

### New Helpers (created as needed per module)
- Minimal — prefer constructing test data inline over shared abstractions
- Each test module self-contained where possible
- Mark shared helpers `pub(crate)` only when truly needed across modules

## Constraints

- All tests must pass: `cargo nextest run -p dugite-ledger`
- Zero warnings: `cargo clippy --all-targets -- -D warnings`
- Formatted: `cargo fmt --all -- --check`
- No changes to production code (except adding `pub(crate)` to helpers if needed)
- Tests should not depend on external resources (filesystem, network)
