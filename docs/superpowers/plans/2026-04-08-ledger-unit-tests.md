# Ledger Unit Tests Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add inline unit tests to 10 untested modules in `crates/dugite-ledger/src/`, covering ~117 tests across block application, certificate processing, epoch transitions, validation rules, collateral, scripts, datums, snapshots, protocol params, and Conway era gating.

**Architecture:** Each source file gets a `#[cfg(test)] mod tests` block appended at the bottom. Tests use minimal inline helpers (no shared test utilities across files). Each module's tests are self-contained and independently compilable.

**Tech Stack:** Rust, cargo nextest, tempfile (for snapshot tests), dugite-primitives types

**Spec:** `docs/superpowers/specs/2026-04-08-ledger-unit-tests-design.md`

---

## File Structure

No new files created. Each task appends a `#[cfg(test)] mod tests { ... }` block to an existing source file:

| File | Action | Tests |
|------|--------|-------|
| `crates/dugite-ledger/src/validation/conway.rs` | Append test module | 8 |
| `crates/dugite-ledger/src/validation/scripts.rs` | Append test module | 12 |
| `crates/dugite-ledger/src/validation/datum.rs` | Append test module | 8 |
| `crates/dugite-ledger/src/validation/collateral.rs` | Append test module | 11 |
| `crates/dugite-ledger/src/validation/phase1.rs` | Append test module | 20 |
| `crates/dugite-ledger/src/state/protocol_params.rs` | Append test module | 6 |
| `crates/dugite-ledger/src/state/snapshot.rs` | Append test module | 8 |
| `crates/dugite-ledger/src/state/certificates.rs` | Append test module | 15 |
| `crates/dugite-ledger/src/state/epoch.rs` | Append test module | 14 |
| `crates/dugite-ledger/src/state/apply.rs` | Append test module | 15 |

## Common Patterns

All test modules follow this structure:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    // Additional imports as needed per module

    // Minimal helpers (inline, not shared)

    // #[test] functions
}
```

**Key imports used across modules:**
```rust
use dugite_primitives::hash::{Hash28, Hash32};
use dugite_primitives::credentials::Credential;
use dugite_primitives::protocol_params::ProtocolParameters;
use dugite_primitives::transaction::*;
use dugite_primitives::value::{Lovelace, Value};
use dugite_primitives::time::{EpochNo, SlotNo};
use dugite_primitives::address::*;
use dugite_primitives::network::NetworkId;
use std::collections::{BTreeMap, HashMap, HashSet};
```

**Running tests:** After each task, run:
```bash
cargo nextest run -p dugite-ledger -E 'test(/^MODULE_NAME::tests::/)' 
cargo clippy -p dugite-ledger --all-targets -- -D warnings
cargo fmt -p dugite-ledger -- --check
```

Where `MODULE_NAME` is the module path (e.g., `validation::conway`).

---

## Task 1: validation/conway.rs (8 tests)

**Files:**
- Modify: `crates/dugite-ledger/src/validation/conway.rs`

Tests for `conway_only_certificate_name()`, `check_era_gating()`, and `calculate_deposits_and_refunds()`.

- [ ] **Step 1: Read the current file to understand the exact function signatures and logic**

Read `crates/dugite-ledger/src/validation/conway.rs` in full.

- [ ] **Step 2: Append the test module**

Append the following `#[cfg(test)] mod tests` block to the end of the file. The tests exercise:
- Era gating: Conway certs accepted at PV≥9, rejected at PV<9
- Governance features era-gated
- Deposit calculation: new key reg, DRep reg, pool re-reg (free), refund
- Per-credential deposit map usage

The test module must use the exact function names and types from the file. Since these are `pub(super)` functions, the inline test module can access them via `use super::*`.

Tests to implement:

| Test | Setup | Assertion |
|------|-------|-----------|
| `test_conway_cert_in_conway_era` | PV=9 params, tx body with Conway cert (e.g. `RegDRep`) | `check_era_gating` produces no errors |
| `test_conway_cert_in_pre_conway_era` | PV=8 params, tx body with Conway cert | errors contains `EraGatingViolation` |
| `test_governance_features_era_gated` | PV=8 params, tx body with non-empty `voting_procedures` | errors contains `GovernancePreConway` |
| `test_deposit_new_key_registration` | `StakeRegistration` cert, empty registered_pools | deposits > 0, refunds == 0 |
| `test_deposit_new_drep_registration` | `RegDRep` cert | deposits == drep_deposit |
| `test_deposit_pool_reregistration_free` | `PoolRegistration` cert, pool_id in registered_pools set | deposits == 0 |
| `test_refund_deregistration` | `StakeDeregistration` cert | refunds == key_deposit |
| `test_per_credential_deposit_map` | `ConwayStakeDeregistration` with stake_key_deposits map containing different amount than current key_deposit | refunds == stored amount, not current |

Each test constructs minimal `ProtocolParameters` (via `mainnet_defaults()` then overriding `protocol_version_major`), a minimal `TransactionBody` with the relevant certificates/governance fields, and calls the function under test.

- [ ] **Step 3: Build and run tests**

```bash
cargo nextest run -p dugite-ledger -E 'test(/conway::tests/)'
```

Expected: All 8 tests pass.

- [ ] **Step 4: Run clippy and fmt**

```bash
cargo clippy -p dugite-ledger --all-targets -- -D warnings && cargo fmt --all -- --check
```

Expected: Clean.

- [ ] **Step 5: Commit**

```bash
git add crates/dugite-ledger/src/validation/conway.rs
git commit -m "test(ledger): add 8 inline unit tests for validation/conway.rs (#337)

Tests cover era gating (Conway certs in PV>=9 vs PV<9), governance
feature gating, deposit/refund calculations for key/DRep/pool
registration and deregistration, and per-credential deposit map usage."
```

---

## Task 2: validation/scripts.rs (12 tests)

**Files:**
- Modify: `crates/dugite-ledger/src/validation/scripts.rs`

Tests for `evaluate_native_script()`, script hash computation, tiered fee calculation, and min fee computation.

- [ ] **Step 1: Read the current file**

Read `crates/dugite-ledger/src/validation/scripts.rs` in full. Note the exact signatures of `evaluate_native_script`, `calculate_ref_script_tiered_fee`, `compute_min_fee`, `compute_script_ref_hash`, `collect_available_script_hashes`.

- [ ] **Step 2: Append the test module**

Tests to implement:

| Test | What it verifies |
|------|-----------------|
| `test_native_script_pubkey_match` | `ScriptPubkey(hash)` with matching signer → true |
| `test_native_script_pubkey_no_match` | `ScriptPubkey(hash)` without matching signer → false |
| `test_native_script_all` | `ScriptAll([a, b])` — both must pass |
| `test_native_script_any` | `ScriptAny([a, b])` — one suffices |
| `test_native_script_n_of_k` | `ScriptNOfK(2, [a, b, c])` — 2 of 3 |
| `test_native_script_time_locks` | `InvalidBefore(100)`: slot 99→false, 100→true. `InvalidHereafter(100)`: slot 99→true, 100→false |
| `test_script_hash_type_tags` | Native: `blake2b_224(0x00 \|\| cbor)`. Plutus V2: `blake2b_224(0x02 \|\| raw_bytes)` |
| `test_available_scripts_from_witnesses` | Tx with native + Plutus scripts in witness set → correct hash set |
| `test_available_scripts_from_ref_inputs` | UTxO with script_ref → hash included in available set |
| `test_tiered_fee_single_tier` | ≤25,600 bytes → `base_rate * size` |
| `test_tiered_fee_multiple_tiers` | 51,200 bytes (2 full tiers) → first tier at base, second at 6/5× |
| `test_min_fee_computation` | `min_fee_a * size + min_fee_b + tiered_ref_fee + ex_unit_fee` |

For `evaluate_native_script` tests: construct `NativeScript` variants, a `HashSet<Hash32>` of signers, and a `SlotNo`. Call `evaluate_native_script(script, &signers, slot)` and assert the boolean result.

For `calculate_ref_script_tiered_fee` tests: call with `(base_fee_per_byte, total_size)` and assert the result. Example: `calculate_ref_script_tiered_fee(15, 25_600)` should equal `15 * 25_600 = 384_000`.

For multi-tier: `calculate_ref_script_tiered_fee(15, 51_200)` = tier0 `15 * 25_600` + tier1 `18 * 25_600` (where 18 = 15 * 6/5).

- [ ] **Step 3: Build and run tests**

```bash
cargo nextest run -p dugite-ledger -E 'test(/scripts::tests/)'
```

- [ ] **Step 4: Run clippy and fmt**

```bash
cargo clippy -p dugite-ledger --all-targets -- -D warnings && cargo fmt --all -- --check
```

- [ ] **Step 5: Commit**

```bash
git add crates/dugite-ledger/src/validation/scripts.rs
git commit -m "test(ledger): add 12 inline unit tests for validation/scripts.rs (#337)

Tests cover native script evaluation (Pubkey, All, Any, NOfK, time locks),
script hash type tags, available script collection from witnesses and
ref inputs, tiered fee calculation, and min fee computation."
```

---

## Task 3: validation/datum.rs (8 tests)

**Files:**
- Modify: `crates/dugite-ledger/src/validation/datum.rs`

Tests for `check_datum_witnesses()`.

- [ ] **Step 1: Read the current file**

Read `crates/dugite-ledger/src/validation/datum.rs` in full. Note the signature of `check_datum_witnesses(tx, utxo_set, errors)`.

- [ ] **Step 2: Append the test module**

Tests need a helper to construct script-locked UTxOs (Base address with `Credential::Script` payment credential and `OutputDatum::DatumHash`).

Helper pattern:
```rust
fn make_script_locked_utxo(datum_hash: Hash32) -> TransactionOutput {
    TransactionOutput {
        address: Address::Base(BaseAddress {
            network: NetworkId::Mainnet,
            payment: Credential::Script(Hash28::from_bytes([0xAAu8; 28])),
            stake: Credential::VerificationKey(Hash28::from_bytes([0xBBu8; 28])),
        }),
        value: Value::lovelace(5_000_000),
        datum: OutputDatum::DatumHash(datum_hash),
        script_ref: None,
        is_legacy: false,
        raw_cbor: None,
    }
}
```

Tests to implement:

| Test | Setup | Assertion |
|------|-------|-----------|
| `test_script_input_datum_present` | Script-locked input with DatumHash; matching datum in `witness_set.plutus_data` | No `MissingDatumWitness` error |
| `test_script_input_datum_missing` | Script-locked input with DatumHash; no datum in witness | `MissingDatumWitness` error |
| `test_inline_datum_no_witness_needed` | Script-locked input with `OutputDatum::InlineDatum` | No errors |
| `test_non_script_input_no_datum` | VKey-locked input (no script credential) | No errors |
| `test_extra_datum_is_hard_error` | Datum in witness set that matches nothing | `ExtraDatumWitness` error |
| `test_output_datum_hash_supplemental` | Output with DatumHash + matching datum in witness | No `ExtraDatumWitness` (supplemental allowed) |
| `test_ref_input_datum_supplemental_only` | Ref input has DatumHash; spending input also has same DatumHash but no witness datum | `MissingDatumWitness` (ref input can't satisfy spend) |
| `test_multiple_script_inputs` | Two script inputs with different DatumHashes; only one datum in witness | One `MissingDatumWitness` for the missing one |

Each test: construct a `Transaction` with appropriate inputs/outputs/witness data, an `UtxoSet` with matching UTxOs, call `check_datum_witnesses(&tx, &utxo_set, &mut errors)`, then assert on the errors vec.

- [ ] **Step 3: Build and run tests**

```bash
cargo nextest run -p dugite-ledger -E 'test(/datum::tests/)'
```

- [ ] **Step 4: Run clippy and fmt**

- [ ] **Step 5: Commit**

```bash
git add crates/dugite-ledger/src/validation/datum.rs
git commit -m "test(ledger): add 8 inline unit tests for validation/datum.rs (#337)

Tests cover datum witness requirements for script-locked inputs,
inline datums, extra datum rejection, supplemental datums from
outputs and reference inputs, and multiple script inputs."
```

---

## Task 4: validation/collateral.rs (11 tests)

**Files:**
- Modify: `crates/dugite-ledger/src/validation/collateral.rs`

Tests for `check_collateral()` and related sub-functions.

- [ ] **Step 1: Read the current file**

Read `crates/dugite-ledger/src/validation/collateral.rs` in full.

- [ ] **Step 2: Append the test module**

Helper: a function to create a Plutus tx with collateral inputs, redeemers, and a script witness (similar to `make_plutus_tx_with_collateral` from validation/tests.rs but inline).

Tests to implement:

| Test | Setup | Assertion |
|------|-------|-----------|
| `test_valid_collateral` | Plutus tx, collateral=10M, fee=200K, collateral_pct=150 | No errors |
| `test_no_collateral_inputs` | Plutus tx with redeemers but empty collateral vec | errors contains "collateral" related error |
| `test_too_many_collateral_inputs` | 4 collateral inputs, max_collateral_inputs=3 | `TooManyCollateralInputs` |
| `test_insufficient_collateral_value` | fee=1M, collateral_pct=150, collateral=1M (need 1.5M) | `InsufficientCollateral` |
| `test_collateral_return_multiasset` | Collateral inputs with multi-asset, collateral_return subtracts tokens | Net is pure ADA, passes |
| `test_non_ada_in_net_collateral` | Collateral inputs with tokens, no collateral_return | `CollateralHasTokens` |
| `test_total_collateral_field_mismatch` | `total_collateral=500K` but computed=1M | `CollateralMismatch` |
| `test_ex_units_memory_exceeded` | Redeemer with mem > max_tx_ex_units.mem | `ExUnitsExceeded` |
| `test_ex_units_steps_exceeded` | Redeemer with steps > max_tx_ex_units.steps | `ExUnitsExceeded` |
| `test_redeemer_index_out_of_bounds` | Spend redeemer index=5 but only 1 input | `RedeemerIndexOutOfRange` |
| `test_vote_redeemer_ordering` | Multiple voters in voting_procedures; verify index assignment matches BTreeMap key ordering | Correct redeemer-to-voter mapping |

- [ ] **Step 3: Build and run tests**

```bash
cargo nextest run -p dugite-ledger -E 'test(/collateral::tests/)'
```

- [ ] **Step 4: Run clippy and fmt**

- [ ] **Step 5: Commit**

```bash
git add crates/dugite-ledger/src/validation/collateral.rs
git commit -m "test(ledger): add 11 inline unit tests for validation/collateral.rs (#337)

Tests cover valid collateral, missing/excessive inputs, insufficient
value with ceiling division, multi-asset return, total_collateral
mismatch, ExUnits limits, redeemer index bounds, and vote ordering."
```

---

## Task 5: validation/phase1.rs (20 tests)

**Files:**
- Modify: `crates/dugite-ledger/src/validation/phase1.rs`

Tests for Phase-1 validation rules. These tests call `crate::validation::validate_transaction()` or `validate_transaction_with_pools()` to exercise the full Phase-1 pipeline, which dispatches to the helpers defined in phase1.rs.

- [ ] **Step 1: Read the current file and mod.rs**

Read `crates/dugite-ledger/src/validation/phase1.rs` and `crates/dugite-ledger/src/validation/mod.rs` to understand the dispatch and helper functions.

- [ ] **Step 2: Append the test module**

Helper: inline `make_utxo_and_tx(input_value, output_value, fee) -> (UtxoSet, Transaction, TransactionInput)` that creates a valid simple tx with one input and one output.

Tests to implement (each calls `validate_transaction` or `validate_transaction_with_pools` with a crafted invalid tx):

| Test | Mutation from valid tx | Expected error |
|------|----------------------|----------------|
| `test_valid_tx_passes` | None (valid tx) | `Ok(())` |
| `test_no_inputs` | `body.inputs = vec![]` | `NoInputs` |
| `test_all_inputs_must_exist` | Input with tx_id not in UTxO set | `InputNotFound` |
| `test_value_not_conserved_ada` | output_value + fee ≠ input_value | `ValueNotConserved` |
| `test_value_not_conserved_multiasset` | Mint tokens without matching outputs | `MultiAssetNotConserved` |
| `test_mint_without_policy_script` | `body.mint` with policy but no script witness | `MissingScriptWitness` |
| `test_fee_too_small` | fee=1 (below min_fee) | `FeeTooSmall` |
| `test_output_below_min_utxo` | output with 1 lovelace | `OutputTooSmall` |
| `test_output_value_too_large` | Output with huge multi-asset exceeding max_val_size | `OutputValueTooLarge` |
| `test_network_id_mismatch` | Output with wrong NetworkId, pass node_network | `WrongNetworkInOutput` |
| `test_tx_size_too_large` | tx_size param > max_tx_size | `TxTooLarge` |
| `test_ttl_expired` | `body.ttl = Some(SlotNo(50))`, current_slot=100 | `TtlExpired` |
| `test_validity_interval_not_started` | `body.validity_interval_start = Some(SlotNo(200))`, current_slot=100 | `NotYetValid` |
| `test_ref_inputs_must_be_disjoint` | Same input in both `inputs` and `reference_inputs` | `ReferenceInputOverlapsInput` |
| `test_ref_inputs_must_exist` | Reference input not in UTxO | `ReferenceInputNotFound` |
| `test_required_signer_missing` | `body.required_signers = [hash]` but no vkey witness | `MissingRequiredSigner` |
| `test_auxiliary_data_hash_mismatch` | `body.auxiliary_data_hash = Some(hash)` but no aux data | `AuxiliaryDataHashWithoutData` |
| `test_treasury_value_mismatch` | `body.treasury_value = Some(999)`, actual treasury=1000 | `TreasuryValueMismatch` |
| `test_script_integrity_hash_mismatch` | Plutus tx with wrong `script_data_hash` | `ScriptDataHashMismatch` |
| `test_ed25519_signature_verification` | VKey witness with corrupted signature bytes | `InvalidWitnessSignature` |

Each test follows the pattern: create valid tx → apply one mutation → validate → assert specific error variant appears in the error list.

- [ ] **Step 3: Build and run tests**

```bash
cargo nextest run -p dugite-ledger -E 'test(/phase1::tests/)'
```

- [ ] **Step 4: Run clippy and fmt**

- [ ] **Step 5: Commit**

```bash
git add crates/dugite-ledger/src/validation/phase1.rs
git commit -m "test(ledger): add 20 inline unit tests for validation/phase1.rs (#337)

Tests cover all Phase-1 validation rules: empty inputs, missing inputs,
value conservation (ADA + multi-asset), fee minimum, output limits,
network ID, tx size, TTL/validity interval, reference input disjointness,
required signers, auxiliary data hash, treasury value, script integrity
hash, and Ed25519 signature verification."
```

---

## Task 6: state/protocol_params.rs (6 tests)

**Files:**
- Modify: `crates/dugite-ledger/src/state/protocol_params.rs`

Tests for `validate_threshold()` and `apply_protocol_param_update()`.

- [ ] **Step 1: Read the current file**

Read `crates/dugite-ledger/src/state/protocol_params.rs` in full.

- [ ] **Step 2: Append the test module**

Tests to implement:

| Test | Setup | Assertion |
|------|-------|-----------|
| `test_apply_partial_update` | Update with only `min_fee_a = Some(50)` | Only `min_fee_a` changed, all others unchanged |
| `test_apply_full_update` | Update with multiple fields set | All specified fields updated |
| `test_noop_update` | `ProtocolParamUpdate` with all fields None | `Ok(())`, params unchanged |
| `test_threshold_valid` | `validate_threshold("test", &Rational{num:1, den:2})` | `Ok(())` |
| `test_threshold_num_exceeds_denom` | `validate_threshold("test", &Rational{num:3, den:2})` | `Err(InvalidProtocolParam)` |
| `test_threshold_zero_denominator` | `validate_threshold("test", &Rational{num:1, den:0})` | `Err(InvalidProtocolParam)` |

Create a `LedgerState` via `LedgerState::new(ProtocolParameters::mainnet_defaults())`, then call the methods. For `apply_protocol_param_update`, construct a `ProtocolParamUpdate` with the desired fields set to `Some(value)`.

- [ ] **Step 3: Build and run tests**

```bash
cargo nextest run -p dugite-ledger -E 'test(/protocol_params::tests/)'
```

- [ ] **Step 4: Run clippy and fmt**

- [ ] **Step 5: Commit**

```bash
git add crates/dugite-ledger/src/state/protocol_params.rs
git commit -m "test(ledger): add 6 inline unit tests for state/protocol_params.rs (#337)

Tests cover partial, full, and no-op protocol param updates, plus
threshold validation for valid rationals, numerator > denominator,
and zero denominator cases."
```

---

## Task 7: state/snapshot.rs (8 tests)

**Files:**
- Modify: `crates/dugite-ledger/src/state/snapshot.rs`

Tests for `save_snapshot()` and `load_snapshot()`.

- [ ] **Step 1: Read the current file**

Read `crates/dugite-ledger/src/state/snapshot.rs` in full. Note `SNAPSHOT_VERSION`, `MAX_SNAPSHOT_SIZE`, the file format layout, and the three load format branches.

- [ ] **Step 2: Append the test module**

These tests use `tempfile::tempdir()` for filesystem operations.

Tests to implement:

| Test | What it does |
|------|-------------|
| `test_save_load_roundtrip` | Save state to temp file, load it back, compare key fields (epoch, treasury, era) |
| `test_magic_bytes` | Save, read raw bytes, assert first 4 = `b"DUGT"` |
| `test_checksum_verification` | Save, read raw bytes, verify blake2b_256 of payload matches stored checksum |
| `test_corrupted_data_detected` | Save, flip a byte in payload region, load → error |
| `test_size_limit_enforcement` | Write a file > MAX_SNAPSHOT_SIZE (or mock), load → error |
| `test_legacy_format_loading` | Write raw bincode (no DUGT header) to file, load → succeeds |
| `test_version_in_header` | Save, read byte 5, assert == SNAPSHOT_VERSION (14) |
| `test_atomic_write` | Save, verify `.tmp` file does NOT exist after completion (was renamed) |

Pattern for each test:
```rust
let dir = tempfile::tempdir().unwrap();
let path = dir.path().join("test.snapshot");
let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());
state.treasury = Lovelace(42_000_000);
state.save_snapshot(&path).unwrap();
// ... assertions on raw bytes or loaded state
```

- [ ] **Step 3: Build and run tests**

```bash
cargo nextest run -p dugite-ledger -E 'test(/snapshot::tests/)'
```

- [ ] **Step 4: Run clippy and fmt**

- [ ] **Step 5: Commit**

```bash
git add crates/dugite-ledger/src/state/snapshot.rs
git commit -m "test(ledger): add 8 inline unit tests for state/snapshot.rs (#337)

Tests cover save/load roundtrip, DUGT magic bytes, blake2b-256 checksum
verification, corrupted data detection, size limit enforcement, legacy
format loading, version header, and atomic write behavior."
```

---

## Task 8: state/certificates.rs (15 tests)

**Files:**
- Modify: `crates/dugite-ledger/src/state/certificates.rs`

Tests for `process_certificate()` and `process_certificate_with_pointer()`.

- [ ] **Step 1: Read the current file**

Read `crates/dugite-ledger/src/state/certificates.rs` in full.

- [ ] **Step 2: Append the test module**

Each test: create `LedgerState::new(mainnet_defaults())`, optionally pre-populate state (e.g., register a pool before testing re-registration), call `process_certificate()` with the appropriate `Certificate` variant, then assert state changes.

Tests to implement:

| Test | Certificate | Key assertions |
|------|------------|----------------|
| `test_stake_registration` | `StakeRegistration(cred)` | reward_accounts contains key with 0 balance; stake_key_deposits has entry |
| `test_stake_deregistration` | `StakeDeregistration(cred)` | delegations removed; reward_accounts removed; stake_map NOT removed; deposit refunded |
| `test_stake_delegation` | `StakeDelegation{cred, pool}` | delegations[cred] == pool_id |
| `test_pool_registration` | `PoolRegistration(params)` | pool_params contains entry; pool_deposits has entry |
| `test_pool_reregistration_staged` | Register pool first, then `PoolRegistration` again | future_pool_params has entry; pending retirement cancelled; no new deposit |
| `test_pool_retirement` | `PoolRetirement{pool, epoch}` | pending_retirements contains entry |
| `test_conway_stake_registration` | `ConwayStakeRegistration{cred, deposit}` | stake_key_deposits stores the cert's deposit amount (not current keyDeposit) |
| `test_conway_stake_deregistration` | `ConwayStakeDeregistration{cred, refund}` | deposit refunded; reward balance included (Dugite behavior) |
| `test_drep_registration` | `RegDRep{cred, deposit, anchor}` | governance.dreps contains entry with deposit |
| `test_drep_unregistration` | Register DRep first, then `UnregDRep{cred, refund}` | governance.dreps removed; deposit refunded |
| `test_drep_update` | Register DRep first, then `UpdateDRep{cred, anchor}` | governance.dreps updated; no deposit change |
| `test_vote_delegation` | `VoteDelegation{cred, drep}` | governance.vote_delegations[cred] == drep |
| `test_committee_hot_auth` | `CommitteeHotAuth{cold, hot}` | governance.committee_hot_keys[cold] == hot |
| `test_committee_cold_resign` | `CommitteeColdResign{cold, anchor}` | governance.committee_resigned contains cold |
| `test_pointer_address_tracking` | `StakeRegistration` via `process_certificate_with_pointer(cert, slot=100, tx_idx=2, cert_idx=0)` | pointer_map contains `Pointer{100, 2, 0}` → credential hash |

- [ ] **Step 3: Build and run tests**

```bash
cargo nextest run -p dugite-ledger -E 'test(/certificates::tests/)'
```

- [ ] **Step 4: Run clippy and fmt**

- [ ] **Step 5: Commit**

```bash
git add crates/dugite-ledger/src/state/certificates.rs
git commit -m "test(ledger): add 15 inline unit tests for state/certificates.rs (#337)

Tests cover all certificate types: stake registration/deregistration/
delegation, pool registration/re-registration/retirement, Conway
stake reg/dereg with inline deposits, DRep lifecycle (reg/unreg/update),
vote delegation, committee hot auth/cold resign, and pointer tracking."
```

---

## Task 9: state/epoch.rs (14 tests)

**Files:**
- Modify: `crates/dugite-ledger/src/state/epoch.rs`

Tests for `process_epoch_transition()` and related methods.

- [ ] **Step 1: Read the current file**

Read `crates/dugite-ledger/src/state/epoch.rs` in full. Note the exact step ordering, snapshot rotation, reward formula, and nonce evolution.

- [ ] **Step 2: Append the test module**

Setup pattern: create `LedgerState::new(mainnet_defaults())`, pre-populate with pools/delegations/rewards as needed, then call `process_epoch_transition(EpochNo(N))`.

Tests to implement:

| Test | Setup | Key assertion |
|------|-------|---------------|
| `test_snapshot_rotation_direction` | Set mark/set/go snapshots to known values, trigger epoch transition | go == old set; set == old mark; new mark computed |
| `test_donations_flushed_before_rewards` | `pending_donations = 1M` | treasury increased by 1M (donations flushed early in Dugite) |
| `test_rewards_applied_before_snap` | Pre-populate reward accounts with pending reward update | After transition, reward accounts have credited amounts |
| `test_fee_capture_at_snap` | `epoch_fees = 5M` | `snapshots.ss_fee == 5M` after transition; `epoch_fees == 0` |
| `test_reward_distribution_formula` | Set reserves, rho, tau; add a pool with delegation | Verify treasury/reserves delta matches formula |
| `test_unregistered_rewards_to_treasury` | Reward for credential not in reward_accounts | Treasury increases by unclaimed amount |
| `test_pool_retirement_processing` | Add pool with pending retirement for current epoch | pool_params no longer contains pool; deposit refunded |
| `test_pool_retirement_missing_reward_account` | Retiring pool whose operator has no reward account | Pool deposit goes to treasury |
| `test_governance_ratification_after_snap` | Add approved proposal in governance state | Proposal enacted after snapshot rotation |
| `test_genesis_epoch_transition` | Fresh state, transition 0→1 | Snapshots exist but go is empty; no rewards distributed |
| `test_bprev_block_count_rotation` | Set block counters | bprev = old current; current reset to empty |
| `test_nonce_evolution` | Set candidate_nonce and lab_nonce | epoch_nonce updated from combination |
| `test_ss_fee_from_go_snapshot` | Set go snapshot's ss_fee to known value | Reward calculation uses go's fee, not current epoch's |
| `test_stake_rebuild_full_utxo_walk` | Add UTxOs, set `needs_stake_rebuild = true` | After rebuild, stake_map reflects UTxO set |

- [ ] **Step 3: Build and run tests**

```bash
cargo nextest run -p dugite-ledger -E 'test(/epoch::tests/)'
```

- [ ] **Step 4: Run clippy and fmt**

- [ ] **Step 5: Commit**

```bash
git add crates/dugite-ledger/src/state/epoch.rs
git commit -m "test(ledger): add 14 inline unit tests for state/epoch.rs (#337)

Tests cover snapshot rotation (go←set←mark), donation flush timing,
reward application before SNAP, fee capture and reset, reward formula
(rho/tau split), unregistered rewards to treasury, pool retirement
processing, governance ratification ordering, genesis epoch, block
count rotation, nonce evolution, and full UTxO stake rebuild."
```

---

## Task 10: state/apply.rs (15 tests)

**Files:**
- Modify: `crates/dugite-ledger/src/state/apply.rs`

Tests for `apply_block()` — the main block application entry point.

- [ ] **Step 1: Read the current file**

Read `crates/dugite-ledger/src/state/apply.rs` in full. Note `apply_block` signature, `BlockValidationMode`, era dispatch, epoch detection, and all the limit checks.

- [ ] **Step 2: Append the test module**

Helpers needed:
- `make_test_block(slot, block_no, txs) -> Block` — inline version
- `make_simple_tx(input, output_value, fee) -> Transaction` — inline version
- Helper to create UTxOs in state

Tests to implement:

| Test | Setup | Key assertion |
|------|-------|---------------|
| `test_apply_byron_block` | Byron-era block with one tx | UTxO consumed and created |
| `test_apply_shelley_block` | Shelley+ block with certificates | Certificates processed, UTxO updated |
| `test_apply_empty_block` | Block with no transactions | State advances (tip/slot updated), no UTxO changes |
| `test_invalid_tx_collateral_consumed` | Tx with `is_valid=false`, collateral input in UTxO | Collateral consumed; regular inputs untouched; certs/withdrawals skipped |
| `test_epoch_transition_detected` | Block at slot in epoch 1 (state at epoch 0) | `state.epoch == EpochNo(1)` after apply |
| `test_multi_epoch_gap` | Block at slot in epoch 3 (state at epoch 0) | Epoch transitions for 1, 2, 3 all processed |
| `test_body_size_exceeds_max` | Block with `header.body_size > max_block_body_size` | Apply returns error |
| `test_ref_script_size_per_tx_limit` | Valid Conway tx with ref_script UTxO > 204,800 bytes | Apply returns error (only in ValidateAll mode) |
| `test_ref_script_size_per_block_limit` | Multiple txs totaling > 1,048,576 bytes ref scripts | Apply returns error |
| `test_block_ex_units_memory_exceeded` | Multiple Plutus txs with total mem > maxBlockExUnits.mem | Apply returns error |
| `test_block_ex_units_steps_exceeded` | Multiple Plutus txs with total steps > maxBlockExUnits.steps | Apply returns error |
| `test_multiple_txs_sequential_utxo` | Tx1 creates output; Tx2 spends it (both in same block) | Both succeed (sequential visibility) |
| `test_conway_pointer_stake_exclusion` | Set `ptr_stake_excluded = false`, apply Conway block | `ptr_stake_excluded = true`; ptr_stake cleared |
| `test_apply_only_mode_skips_validation` | Block with body_size > max in ApplyOnly mode | Apply succeeds (predicate skipped) |
| `test_certificate_processing_order` | Block with multiple txs containing certificates | Certificates processed in tx order |

- [ ] **Step 3: Build and run tests**

```bash
cargo nextest run -p dugite-ledger -E 'test(/apply::tests/)'
```

- [ ] **Step 4: Run clippy and fmt**

- [ ] **Step 5: Commit**

```bash
git add crates/dugite-ledger/src/state/apply.rs
git commit -m "test(ledger): add 15 inline unit tests for state/apply.rs (#337)

Tests cover Byron/Shelley block application, empty blocks, invalid tx
collateral handling, epoch transition detection (single and multi-gap),
body size limit, ref-script per-tx and per-block limits, ExUnits budget
(mem and steps independently), sequential tx UTxO visibility, Conway
pointer stake exclusion, ApplyOnly mode, and certificate ordering."
```

---

## Task 11: Final verification and cleanup

- [ ] **Step 1: Run the complete test suite**

```bash
cargo nextest run -p dugite-ledger
```

All existing + new tests must pass.

- [ ] **Step 2: Run full workspace checks**

```bash
cargo clippy --all-targets -- -D warnings && cargo fmt --all -- --check
```

- [ ] **Step 3: Count new tests**

```bash
cargo nextest run -p dugite-ledger --list 2>/dev/null | grep -c "test "
```

Compare with the baseline count to verify ~117 new tests were added.

- [ ] **Step 4: Push to remote**

```bash
git push
```
