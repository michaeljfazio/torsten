# Inline Unit Tests for Ledger Validation Core Modules

**Date:** 2026-04-08
**Issue:** #337
**Status:** Approved (double cross-validated: Haskell cardano-ledger + Dugite source)

## Summary

Add inline `#[cfg(test)] mod tests` blocks to 10 source files in `dugite-ledger/src` that currently
have zero unit tests. These are the most correctness-critical paths in the codebase — silent
regressions here could cause chain divergence from Haskell cardano-node.

Tests verify **Dugite's actual behavior** (regression protection). Where Dugite deviates from
Haskell, this is documented in the "Known Correctness Gaps" appendix at the end.

## Scope

### In Scope (10 files, ~117 tests)

**Tier 1 — Highest impact (~5,500 lines):**

| File | Lines | Tests | Focus |
|------|-------|-------|-------|
| `state/apply.rs` | 1,330 | ~15 | Block application, era dispatch, epoch detection, limits |
| `state/certificates.rs` | 1,225 | ~15 | Certificate processing, deposits, pointer tracking |
| `state/epoch.rs` | 1,098 | ~14 | Epoch transitions, rewards, snapshots, retirements |
| `validation/phase1.rs` | 1,095 | ~20 | All Phase-1 validation rules (monolithic, not layered) |
| `validation/collateral.rs` | 798 | ~11 | Collateral checks, redeemer indices, ex-units |

**Tier 2 — Important but smaller (~1,500 lines):**

| File | Lines | Tests | Focus |
|------|-------|-------|-------|
| `validation/scripts.rs` | 854 | ~12 | Native scripts, hash computation, tiered fees |
| `state/snapshot.rs` | 300 | ~8 | Save/load, checksums, legacy compat |
| `validation/datum.rs` | 199 | ~8 | Datum witness completeness |
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

All test conditions have been verified against **both** the Haskell cardano-ledger source
(IntersectMBO/cardano-ledger) **and** the actual Dugite implementation. Where Dugite diverges
from Haskell, the test verifies Dugite's current behavior and the divergence is documented in
the "Known Correctness Gaps" appendix.

Key Haskell references:
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

Dugite implementation notes:
- Body size: checks `body_size > max_block_body_size` (protocol param), NOT equality vs header
- Ref-script per-tx: 204,800 bytes, gated on `is_valid=true` AND `protocol_version_major >= 9`
- ExUnits: block-level sum checked independently for mem and steps (both use `>`)
- Invalid txs: collateral consumed + return added; certs/withdrawals/governance skipped; fee = total_collateral or inputs-return
- ApplyOnly: skips all predicate checks but applies all state changes

| Test | What it verifies |
|------|-----------------|
| `test_apply_byron_block` | Byron era block applies correctly, UTxO updated |
| `test_apply_shelley_block` | Shelley+ block applies, certificates/withdrawals processed |
| `test_apply_empty_block` | Block with no transactions applies cleanly |
| `test_invalid_tx_collateral_consumed` | `is_valid: false` tx: collateral consumed + return added; regular I/O, certs, withdrawals, governance all skipped; fee collected from collateral |
| `test_epoch_transition_detected` | Block in new epoch triggers `process_epoch_transition` via `epoch_of_slot()` |
| `test_multi_epoch_gap` | Block skipping multiple epochs triggers `while self.epoch < block_epoch` loop, processing each intermediate epoch |
| `test_body_size_exceeds_max` | `block.header.body_size > max_block_body_size` → rejected (⚠ Haskell checks actual==header instead) |
| `test_ref_script_size_per_tx_limit` | Single valid tx with >204,800 bytes ref scripts rejected; only for `is_valid=true` + Conway (PV≥9) |
| `test_ref_script_size_per_block_limit` | Block total ref scripts >1,048,576 bytes rejected (pre-scan with within-block UTxO overlay) |
| `test_block_ex_units_memory_exceeded` | Sum of all valid tx ExUnits across block > `maxBlockExUnits.mem` (strict >) |
| `test_block_ex_units_steps_exceeded` | Sum of all valid tx ExUnits across block > `maxBlockExUnits.steps` (strict >) |
| `test_multiple_txs_sequential_utxo` | Txs applied left-to-right in loop; each sees prior tx's UTxO changes |
| `test_conway_pointer_stake_exclusion` | First Conway-era block calls `exclude_pointer_address_stake()` (clears `ptr_stake`), sets `ptr_stake_excluded=true` |
| `test_apply_only_mode_skips_validation` | `BlockValidationMode::ApplyOnly` skips body size, Phase-1/2, ref-script, ExUnits checks; still applies UTxO/cert/fee changes |
| `test_certificate_processing_order` | Certificates processed in tx order within block (sequential loop) |

### state/certificates.rs (~15 tests)

Dugite implementation notes:
- Conway tag 8: does NOT enforce zero reward balance (⚠ Haskell requires zero)
- Committee resign: allows re-authorization via subsequent HotAuth (⚠ Haskell: permanent)
- Pool re-reg: staged to `future_pool_params`, cancels pending retirement, no deposit
- Per-credential deposit: stored in `stake_key_deposits` HashMap, fallback to current `keyDeposit`
- DRep deposit: stored in `DRepRegistration.deposit` field (separate from keyDeposit)
- Deregistration: does NOT remove from `stake_map`; removes delegations/rewards/vote_delegations/script_credentials/pointers

| Test | What it verifies |
|------|-----------------|
| `test_stake_registration` | Creates reward_account with 0 balance, stores per-credential deposit in `stake_key_deposits` |
| `test_stake_deregistration` | Removes from delegations, reward_accounts, vote_delegations, script_credentials, pointers; does NOT remove from stake_map; refunds stored per-credential deposit (fallback to current keyDeposit) |
| `test_stake_delegation` | Updates delegations map to target pool |
| `test_pool_registration` | Creates pool_params entry, charges `poolDeposit`, records in `pool_deposits` |
| `test_pool_reregistration_staged` | Existing pool: params staged to `future_pool_params`, pending retirement cancelled, no deposit charged |
| `test_pool_retirement` | Adds to pending retirements, processed at epoch boundary |
| `test_conway_stake_registration` | Tag 7: inline deposit amount stored per-credential (may differ from current `keyDeposit`) |
| `test_conway_stake_deregistration` | Tag 8: refunds stored deposit; remaining reward balance included (⚠ Haskell requires zero balance first) |
| `test_drep_registration` | Creates DRep entry, charges deposit from cert's `deposit` field (separate from `keyDeposit`) |
| `test_drep_unregistration` | Removes DRep, refunds stored per-DRep deposit |
| `test_drep_update` | Updates DRep metadata without deposit change |
| `test_vote_delegation` | Updates vote_delegations map |
| `test_committee_hot_auth` | Authorizes hot key for cold member; removes from `committee_resigned` if present (⚠ Haskell: permanent resign) |
| `test_committee_cold_resign` | Records committee member resignation in `committee_resigned` |
| `test_pointer_address_tracking` | Tags 0, 7, 11, 12, 13 create pointer entry `Pointer { slot, tx_index, cert_index }` in `pointer_map` |

### state/epoch.rs (~14 tests)

Dugite `process_epoch_transition()` ordering (verified from source):
1. Flush pending treasury donations (step 0 — ⚠ Haskell: step 10 inside EPOCH)
2. Apply pending reward update from previous epoch
3. Compute and apply RUPD (reward distribution) using GO snapshot + ss_fee + bprev
4. SNAP: rotate snapshots (go←set, set←mark, new mark) + capture epoch_fees into ss_fee
5. POOLREAP: process pending retirements
6. RATIFY: governance ratification + enactment
7. DRep snapshot capture (after ratification, PV≥9)
8. Nonce evolution (handled locally, not consensus layer)
9. Reset accumulators (epoch_fees, block counters)

| Test | What it verifies |
|------|-----------------|
| `test_snapshot_rotation_direction` | `go = set.take(); set = mark.take()` — go←set, set←mark, then new mark built from live state |
| `test_donations_flushed_before_rewards` | Pending treasury donations flushed at step 0 before reward computation (⚠ Haskell: after governance enactment) |
| `test_rewards_applied_before_snap` | RUPD fires before SNAP rotation; credited rewards affect subsequent mark snapshot |
| `test_fee_capture_at_snap` | `epoch_fees` read into `snapshots.ss_fee` during SNAP, then reset to 0 |
| `test_reward_distribution_formula` | `expansion = floor(eta * rho * reserves)` where eta = performance factor; `rewardPot = expansion + ssFee(go)`; `treasury_cut = floor(tau * rewardPot)`; pools get `R = rewardPot - treasury_cut` |
| `test_unregistered_rewards_to_treasury` | Rewards for deregistered accounts go to treasury |
| `test_pool_retirement_processing` | Pending retirements for current epoch applied, pool removed from pool_params |
| `test_pool_retirement_missing_reward_account` | Pool deposit goes to treasury if reward account no longer exists |
| `test_governance_ratification_after_snap` | `ratify_proposals()` called after SNAP rotation (step 6) |
| `test_genesis_epoch_transition` | Epoch 0→1: empty GO snapshot → only monetary expansion, no pool rewards |
| `test_bprev_block_count_rotation` | Previous epoch block counts captured into bprev, current reset |
| `test_nonce_evolution` | Nonce computed locally: `epoch_nonce = candidate ⭒ last_epoch_block_nonce ⭒ extraEntropy` (TICKN rule) |
| `test_ss_fee_from_go_snapshot` | Reward calculation uses go snapshot's ssFee (2 epochs old), not current epoch |
| `test_stake_rebuild_full_utxo_walk` | `rebuild_stake_distribution()` walks full UTxO set (triggered by `needs_stake_rebuild` flag after Mithril/snapshot restore) |

### state/protocol_params.rs (~6 tests)

Dugite notes:
- 15 governance thresholds validated (all DVT + PVT params)
- Validation at enactment time (⚠ Haskell: at proposal submission via `ppuWellFormed`)
- Field-by-field `if let Some(v)` pattern

| Test | What it verifies |
|------|-----------------|
| `test_apply_partial_update` | Only specified fields updated, others unchanged |
| `test_apply_full_update` | All fields updated correctly |
| `test_noop_update` | All-None update returns Ok(()), no mutations |
| `test_threshold_valid` | Valid rational in [0,1] accepted (e.g., 1/2) |
| `test_threshold_num_exceeds_denom` | Numerator > denominator → `InvalidProtocolParam` error |
| `test_threshold_zero_denominator` | Zero denominator → `InvalidProtocolParam` error |

### state/snapshot.rs (~8 tests)

Format: `[DUGT(4)][version(1)][blake2b-256(32)][bincode(N)]`
- SNAPSHOT_VERSION = 14
- MAX_SNAPSHOT_SIZE = 10 GB (from mod.rs)
- Supports 3 load formats: versioned, legacy+checksum, raw bincode
- Atomic write via `.tmp` + rename
- Post-load: address index rebuild, stake rebuild, RUPD flag, deposit migration

| Test | What it verifies |
|------|-----------------|
| `test_save_load_roundtrip` | Save then load produces identical state |
| `test_magic_bytes` | Saved file starts with `b"DUGT"` magic |
| `test_checksum_verification` | blake2b-256 of bincode payload matches stored checksum |
| `test_corrupted_data_detected` | Modified payload bytes cause checksum mismatch error |
| `test_size_limit_enforcement` | File > MAX_SNAPSHOT_SIZE (10 GB) rejected before deserialization |
| `test_legacy_format_loading` | Raw bincode without DUGT header still deserializes |
| `test_version_in_header` | Byte 5 = SNAPSHOT_VERSION (14) |
| `test_atomic_write` | Writes to `path.with_extension("tmp")` then `fs::rename` to final path |

### validation/phase1.rs (~20 tests)

Dugite implementation notes:
- All checks are monolithic in `run_phase1_rules` (no LEDGER/UTXOW/UTXO layer distinction)
- Rules numbered 1-14 plus additional Conway checks
- Value conservation: Rule 3 (ADA) + Rule 3b (multi-asset) as separate checks
- Ref-script per-tx limit checked in apply.rs, not here

| Test | What it verifies | Dugite error |
|------|-----------------|-------------|
| `test_valid_tx_passes` | Well-formed transaction passes all rules | — |
| `test_no_inputs` | Rule 1: at least one input required | `NoInputs` |
| `test_all_inputs_must_exist` | Rule 2: all `body.inputs` exist in UTxO set | `InputNotFound` |
| `test_value_not_conserved_ada` | Rule 3: ADA balance `consumed = inputs + withdrawals + refunds; produced = outputs + fee + deposits` | `ValueNotConserved` |
| `test_value_not_conserved_multiasset` | Rule 3b: multi-asset minted/burned must net to zero across inputs/outputs | `MultiAssetNotConserved` |
| `test_mint_without_policy_script` | Rule 3c: minting policy has no matching script witness | `MissingScriptWitness` |
| `test_fee_too_small` | Rule 4: `fee < min_fee(tx_size) + ref_script_fee + ex_unit_fee` | `FeeTooSmall` |
| `test_output_below_min_utxo` | Rule 5: output below minimum UTxO value | `OutputTooSmall` |
| `test_output_value_too_large` | Rule 5a: CBOR size exceeds max_val_size | `OutputTooBig` |
| `test_network_id_mismatch` | Rule 5b: output network ID wrong | `WrongNetwork` |
| `test_tx_size_too_large` | Rule 6: transaction size > max_tx_size | `TxSizeTooLarge` |
| `test_ttl_expired` | Rule 7: current slot > TTL | `TTLExpired` |
| `test_validity_interval_not_started` | Rule 8: current slot < validity_start | `ValidityIntervalNotStarted` |
| `test_ref_inputs_must_be_disjoint` | Rule 9: reference inputs must not overlap regular inputs | `NonDisjointRefInputs` |
| `test_ref_inputs_must_exist` | Rule 9: reference inputs exist in UTxO set | `ReferenceInputNotFound` |
| `test_required_signer_missing` | Rule 10: required signer without matching vkey witness | `MissingRequiredSigner` |
| `test_auxiliary_data_hash_mismatch` | Rule 1c: aux data hash/data inconsistency (3 failure modes) | `AuxiliaryDataHash*` |
| `test_treasury_value_mismatch` | Conway: `currentTreasuryValue` field doesn't match actual treasury | `TreasuryValueMismatch` |
| `test_script_integrity_hash_mismatch` | Rule 12: `scriptIntegrityHash ≠ H(redeemers \|\| datums \|\| cost_models)` | `ScriptDataHashMismatch` |
| `test_ed25519_signature_verification` | Rule 14: invalid VKey witness signature rejected | `InvalidWitnessSignature` |

Note: duplicate inputs (Rule 1b) are structurally rejected at CBOR deserialization (Set with tag 258).
Withdrawal network check (Rule 5d) and pool reward account network check (Rule 1i) also implemented
but covered by existing `validation/tests.rs`.

### validation/collateral.rs (~11 tests)

Dugite implementation notes:
- Gate: checks `body.collateral.is_empty()` (presence-based, not redeemer-based)
- Comparison: `(fee * collateral_percentage).div_ceil(100)` (ceiling division, mathematically
  equivalent to Haskell's cross-multiplication form `bal * 100 >= fee * collPerc`)
- ScriptsNotPaidUTxO (collateral must be VKey): NOT implemented
- ExtraRedeemers: NOT implemented as a distinct check

| Test | What it verifies | Dugite error |
|------|-----------------|-------------|
| `test_valid_collateral` | Tx with collateral inputs + sufficient value passes | — |
| `test_no_collateral_inputs` | Empty collateral when Plutus scripts present | `NoCollateralInputs` |
| `test_too_many_collateral_inputs` | Exceeds `maxCollateralInputs` | `TooManyCollateralInputs` |
| `test_insufficient_collateral_value` | Effective collateral < `ceil(fee * collateral_pct / 100)` | `InsufficientCollateral` |
| `test_collateral_return_multiasset` | Babbage+ collateral return subtracts correctly from total | — |
| `test_non_ada_in_net_collateral` | Net collateral (inputs minus return) contains tokens | `CollateralContainsNonADA` |
| `test_total_collateral_field_mismatch` | Declared `totalCollateral` ≠ computed balance | `IncorrectTotalCollateral` |
| `test_ex_units_memory_exceeded` | Tx redeemer sum > `maxTxExUnits.mem` | `ExUnitsExceeded` |
| `test_ex_units_steps_exceeded` | Tx redeemer sum > `maxTxExUnits.steps` | `ExUnitsExceeded` |
| `test_redeemer_index_out_of_bounds` | Spend redeemer index ≥ input count | `RedeemerIndexOutOfRange` |
| `test_vote_redeemer_ordering` | Vote index follows `Voter` BTreeMap ordering: CC < DRep < SPO; within role: sorted by credential hash | — |

### validation/scripts.rs (~12 tests)

Dugite implementation notes:
- Tiered fee uses floor (not ceiling) — code comment says ceiling but implementation floors
- Script hash: native → blake2b-224(0x00 || CBOR); Plutus → blake2b-224(0x0N || raw_flat_bytes)

| Test | What it verifies |
|------|-----------------|
| `test_native_script_pubkey_match` | ScriptPubkey with matching signer → true |
| `test_native_script_pubkey_no_match` | ScriptPubkey without matching signer → false |
| `test_native_script_all` | ScriptAll: all sub-scripts must be true |
| `test_native_script_any` | ScriptAny: at least one sub-script true |
| `test_native_script_n_of_k` | ScriptNOfK: at least N of K true |
| `test_native_script_time_locks` | InvalidBefore: `slot >= n` (inclusive). InvalidHereafter: `slot < n` (strictly less) |
| `test_script_hash_type_tags` | Native: `blake2b-224(0x00 \|\| CBOR(script))`. Plutus V1/V2/V3: `blake2b-224(0x0N \|\| raw_flat_bytes)` |
| `test_available_scripts_from_witnesses` | Collects hashes from witness set (native + Plutus V1/V2/V3) |
| `test_available_scripts_from_ref_inputs` | Collects hashes from reference input UTxOs (both spending + reference inputs) |
| `test_tiered_fee_single_tier` | ≤25,600 bytes uses base rate (`minFeeRefScriptCostPerByte`); stride and 6/5 multiplier hardcoded |
| `test_tiered_fee_multiple_tiers` | >25,600 bytes: 6/5 rational multiplier per tier. Result is floor (⚠ Haskell uses ceiling) |
| `test_min_fee_computation` | `min_fee(effective_size) + tierRefScriptFee + exUnitFee` where exUnitFee = `ceil(prSteps*steps + prMem*mem)` |

### validation/datum.rs (~8 tests)

Dugite implementation notes:
- Extra datum: hard error (`ExtraDatumWitness`)
- CIP-0069 PlutusV3 no-datum: NOT implemented (all Plutus spending inputs require datum)
- Reference input datums: supplemental only, cannot satisfy spending requirements

| Test | What it verifies | Dugite error |
|------|-----------------|-------------|
| `test_script_input_datum_present` | Script-locked input with DatumHash: matching datum in witness passes | — |
| `test_script_input_datum_missing` | Script-locked input with DatumHash: no datum in witness → error | `MissingDatumWitness` |
| `test_inline_datum_no_witness_needed` | InlineDatum: pulled from UTxO directly, no witness entry required | — |
| `test_non_script_input_no_datum` | Non-script (VKey) input needs no datum | — |
| `test_extra_datum_is_hard_error` | Witness datum not in needed set → hard predicate failure | `ExtraDatumWitness` |
| `test_output_datum_hash_supplemental` | Output DatumHash allowed as supplemental in witness set | — |
| `test_ref_input_datum_supplemental_only` | Reference input DatumHash is supplemental but CANNOT satisfy spending input requirement | `MissingDatumWitness` |
| `test_multiple_script_inputs` | Multiple script inputs each need their own datum hash in witness | `MissingDatumWitness` |

### validation/conway.rs (~8 tests)

Conway-only certificate types: **12**. Tags: 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18.

| Test | What it verifies |
|------|-----------------|
| `test_conway_cert_in_conway_era` | All 12 Conway cert types allowed when PV ≥ 9 |
| `test_conway_cert_in_pre_conway_era` | Conway certs rejected when PV < 9 |
| `test_governance_features_era_gated` | Voting procedures + proposal procedures rejected pre-Conway |
| `test_deposit_new_key_registration` | New key registration charges current `keyDeposit` |
| `test_deposit_new_drep_registration` | New DRep charges from cert's deposit field (separate from keyDeposit) |
| `test_deposit_pool_reregistration_free` | Existing pool re-reg: no deposit, detected via `registered_pools.contains()` |
| `test_refund_deregistration` | Key/DRep deregistration refunds stored per-credential deposit amount |
| `test_per_credential_deposit_map` | After governance changes keyDeposit, old credentials refund at their original stored rate |

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

## Appendix: Known Correctness Gaps (Dugite vs Haskell)

The following deviations from Haskell cardano-ledger were identified during cross-validation.
Tests verify Dugite's current behavior. These gaps should be tracked for future correction.

### High Priority (ledger divergence risk)

| Gap | Dugite behavior | Haskell behavior | Risk |
|-----|----------------|-----------------|------|
| Conway tag 8 zero-balance | Allows deregistration with non-zero reward balance; includes rewards in refund | Requires reward balance = 0 before deregistration (`StakeKeyHasNonZeroAccountBalanceDELEG`) | Ledger state divergence for affected accounts |
| `ConwayWdrlNotDelegatedToDRep` | Not implemented | PV≥10: every KeyHash withdrawal must have active DRep delegation | Will accept invalid withdrawals on PV10+ |
| Body size check | Compares `body_size > max_block_body_size` (protocol param) | Compares `actualSize == headerClaimedSize` (equality vs header) | Different failure mode |
| Tiered ref script fee | Uses floor of rational sum | Uses ceiling of rational sum | Off-by-one fee calculation |
| CIP-0069 PlutusV3 no-datum | All Plutus spending inputs require datum | PlutusV3 spending inputs exempt from datum requirement | Will reject valid PlutusV3 txs |
| Treasury donation timing | Flushed first (before rewards) | Flushed as step 10 inside EPOCH (after governance enactment) | Ordering difference in treasury accounting |

### Medium Priority

| Gap | Dugite behavior | Haskell behavior | Risk |
|-----|----------------|-----------------|------|
| Committee resignation | Allows re-authorization (removes from resigned set) | Resignation is permanent | Different governance state |
| `ExtraneousScriptWitnessesUTXOW` | Not checked; unused scripts in witness tolerated | Hard error | Accepts txs Haskell rejects |
| `ScriptsNotPaidUTxO` | Not checked; collateral can be script-locked | Collateral must be VKey addresses | Accepts txs Haskell rejects |
| `ExtraRedeemers` | Not checked as distinct error | Hard error for redeemers with no matching purpose | Accepts txs Haskell rejects |
| PParam threshold validation | Validated at enactment time | Validated at proposal submission (`ppuWellFormed` in GOV rule) | Late rejection instead of early |
