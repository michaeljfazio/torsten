# Inline Unit Tests for Ledger Validation Core Modules

**Date:** 2026-04-08
**Issue:** #337
**Status:** Approved

## Summary

Add inline `#[cfg(test)] mod tests` blocks to 10 source files in `dugite-ledger/src` that currently have zero unit tests. These are the most correctness-critical paths in the codebase — silent regressions here could cause chain divergence from Haskell cardano-node.

## Scope

### In Scope (10 files, ~110 tests)

**Tier 1 — Highest impact (~5,500 lines):**

| File | Lines | Tests | Focus |
|------|-------|-------|-------|
| `state/apply.rs` | 1,330 | ~15 | Block application, era dispatch, epoch detection, limits |
| `state/certificates.rs` | 1,225 | ~15 | Certificate processing, deposits, pointer tracking |
| `state/epoch.rs` | 1,098 | ~12 | Epoch transitions, rewards, snapshots, retirements |
| `validation/phase1.rs` | 1,095 | ~15 | All Phase-1 validation rules |
| `validation/collateral.rs` | 798 | ~12 | Collateral checks, redeemer indices, ex-units |

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

## Test Plan by Module

### state/apply.rs (~15 tests)

| Test | What it verifies |
|------|-----------------|
| `test_apply_byron_block` | Byron era block applies correctly, UTxO updated |
| `test_apply_shelley_block` | Shelley+ block applies, certificates/withdrawals processed |
| `test_apply_empty_block` | Block with no transactions applies cleanly |
| `test_invalid_tx_skipped` | `is_valid: false` tx: collateral consumed, regular I/O skipped |
| `test_epoch_transition_detected` | Block in new epoch triggers `process_epoch_transition` |
| `test_multi_epoch_gap` | Block skipping multiple epochs triggers transitions for each |
| `test_body_size_exceeds_max` | Oversized block body rejected |
| `test_ref_script_size_per_tx_limit` | Single tx with >200 KiB ref scripts rejected |
| `test_ref_script_size_per_block_limit` | Block total ref scripts >1 MiB rejected |
| `test_ex_units_memory_exceeded` | Block exceeds max memory budget |
| `test_ex_units_steps_exceeded` | Block exceeds max steps budget |
| `test_multiple_txs_in_block` | Multiple valid transactions all applied |
| `test_conway_hfc_pointer_exclusion` | Pointer address stake excluded at Conway boundary |
| `test_apply_only_mode_skips_validation` | `BlockValidationMode::ApplyOnly` bypasses Phase-1 |
| `test_certificate_processing_order` | Certificates processed in tx order within block |

### state/certificates.rs (~15 tests)

| Test | What it verifies |
|------|-----------------|
| `test_stake_registration` | Creates stake_map, reward_account, deposit entry |
| `test_stake_deregistration` | Removes delegation/rewards, keeps stake_map, refunds deposit |
| `test_stake_delegation` | Updates delegations map to target pool |
| `test_pool_registration` | Creates pool_params entry, charges deposit |
| `test_pool_reregistration_free` | Existing pool update doesn't charge deposit |
| `test_pool_retirement` | Adds to pending retirements |
| `test_conway_stake_registration` | Inline deposit amount stored per-credential |
| `test_conway_stake_deregistration` | Returns remaining reward balance in refund |
| `test_drep_registration` | Creates DRep entry, charges deposit |
| `test_drep_unregistration` | Removes DRep, refunds deposit |
| `test_drep_update` | Updates DRep metadata without deposit change |
| `test_vote_delegation` | Updates vote_delegations map |
| `test_committee_hot_auth` | Authorizes hot key for cold committee member |
| `test_committee_cold_resign` | Records committee member resignation |
| `test_pointer_address_tracking` | Certificate creates pointer entry (slot/tx/cert index) |

### state/epoch.rs (~12 tests)

| Test | What it verifies |
|------|-----------------|
| `test_snapshot_rotation` | Mark→set→go rotation occurs |
| `test_fee_capture_into_ss_fee` | Accumulated epoch fees captured in snapshot |
| `test_reward_distribution_monetary_expansion` | Rho/tau split between pools and treasury |
| `test_unregistered_rewards_to_treasury` | Rewards for deregistered accounts go to treasury |
| `test_pool_retirement_processing` | Pending retirements applied, params removed |
| `test_pool_retirement_deposit_refund` | Pool deposit refunded to reward account |
| `test_treasury_donation_flush` | Pending donations added to treasury |
| `test_nonce_evolution` | Next-epoch nonce computed from VRF output |
| `test_genesis_epoch_transition` | Epoch 0→1 with empty snapshots |
| `test_stake_rebuild` | Full UTxO walk rebuilds stake_map |
| `test_bprev_block_count_update` | Block counts from previous epoch captured |
| `test_governance_ratification` | Approved proposals enacted at epoch boundary |

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

### validation/phase1.rs (~15 tests)

| Test | What it verifies |
|------|-----------------|
| `test_valid_tx_passes` | Well-formed transaction passes all rules |
| `test_no_inputs` | Rule 1: at least one input required |
| `test_duplicate_inputs` | Rule 1b: duplicate inputs rejected |
| `test_missing_input` | Rule 2: input not in UTxO set |
| `test_value_not_conserved_ada` | Rule 3: ADA value conservation |
| `test_value_not_conserved_multiasset` | Rule 3b: multi-asset conservation |
| `test_mint_without_policy_script` | Rule 3c: minting policy missing |
| `test_fee_too_small` | Rule 4: fee below minimum |
| `test_output_below_min_utxo` | Rule 5: output below minimum UTxO |
| `test_output_value_too_large` | Rule 5a: CBOR size exceeds max_val_size |
| `test_network_id_mismatch` | Rule 5b: output network ID wrong |
| `test_tx_size_too_large` | Rule 6: transaction size limit |
| `test_ttl_expired` | Rule 7: TTL past current slot |
| `test_validity_interval_not_started` | Rule 8: validity start in future |
| `test_required_signer_missing` | Rule 10: required signer without witness |

### validation/collateral.rs (~12 tests)

| Test | What it verifies |
|------|-----------------|
| `test_valid_collateral` | Plutus tx with sufficient collateral passes |
| `test_missing_collateral_inputs` | No collateral inputs for Plutus tx |
| `test_too_many_collateral_inputs` | Exceeds max_collateral_inputs |
| `test_insufficient_collateral_value` | Below percentage threshold |
| `test_collateral_return_multiasset` | Babbage+ collateral return subtracts correctly |
| `test_non_ada_in_net_collateral` | Net collateral contains tokens → error |
| `test_total_collateral_mismatch` | Declared total_collateral ≠ computed |
| `test_ex_units_memory_exceeded` | Tx exceeds max_tx_ex_units.mem |
| `test_ex_units_steps_exceeded` | Tx exceeds max_tx_ex_units.steps |
| `test_missing_spend_redeemer` | Script-locked input without spend redeemer |
| `test_missing_mint_redeemer` | Plutus minting policy without redeemer |
| `test_redeemer_index_out_of_bounds` | Spend redeemer index ≥ input count |

### validation/scripts.rs (~12 tests)

| Test | What it verifies |
|------|-----------------|
| `test_native_script_pubkey_match` | ScriptPubkey with matching signer |
| `test_native_script_pubkey_no_match` | ScriptPubkey without matching signer |
| `test_native_script_all` | ScriptAll requires all sub-scripts |
| `test_native_script_any` | ScriptAny requires one sub-script |
| `test_native_script_n_of_k` | ScriptNOfK threshold logic |
| `test_native_script_time_locks` | InvalidBefore/InvalidHereafter with slot |
| `test_script_hash_type_tags` | Type tags 0x00-0x03 for native/V1/V2/V3 |
| `test_available_scripts_from_witnesses` | Collects hashes from witness set |
| `test_available_scripts_from_ref_inputs` | Collects hashes from reference input UTxOs |
| `test_tiered_fee_single_tier` | ≤25 KiB uses base rate |
| `test_tiered_fee_multiple_tiers` | >25 KiB applies 1.2× multiplier per tier |
| `test_min_fee_computation` | Base + ref_script + ex_unit fees combined |

### validation/datum.rs (~8 tests)

| Test | What it verifies |
|------|-----------------|
| `test_script_input_datum_present` | DatumHash input with matching witness passes |
| `test_script_input_datum_missing` | DatumHash input without witness → error |
| `test_inline_datum_no_witness_needed` | InlineDatum bypasses witness requirement |
| `test_non_script_input_no_datum` | Non-script input needs no datum |
| `test_extra_datum_rejected` | Witness datum not in needed set → error |
| `test_output_datum_hash_allowed` | Output DatumHash allowed as supplemental |
| `test_ref_input_datum_allowed` | Reference input DatumHash allowed |
| `test_multiple_script_inputs` | Multiple script inputs each need their datum |

### validation/conway.rs (~8 tests)

| Test | What it verifies |
|------|-----------------|
| `test_conway_cert_in_conway_era` | Conway cert allowed when PV ≥ 9 |
| `test_conway_cert_in_pre_conway_era` | Conway cert rejected when PV < 9 |
| `test_governance_in_pre_conway` | Governance features rejected pre-Conway |
| `test_deposit_new_registration` | New key/DRep registration charges deposit |
| `test_deposit_pool_reregistration_free` | Existing pool re-reg charges nothing |
| `test_refund_deregistration` | Key/DRep deregistration refunds deposit |
| `test_combined_reg_deleg_deposit` | Registration+delegation cert charges deposit |
| `test_per_credential_deposit_map` | Uses stored deposit when key_deposit has changed |

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
