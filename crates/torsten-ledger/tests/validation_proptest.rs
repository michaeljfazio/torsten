//! Property-based tests for transaction validation invariants.
//!
//! Tests use simple ADA-only, single-input/single-output transactions to verify
//! fee conservation, min fee monotonicity, and rejection of invalid mutations.
//! Additional multi-asset sections verify minting and burning conservation.

use proptest::prelude::*;
use std::collections::BTreeMap;
use torsten_ledger::validation::{validate_transaction, ValidationError};
use torsten_ledger::UtxoSet;
use torsten_primitives::address::{Address, ByronAddress};
use torsten_primitives::hash::{blake2b_224, Hash32, PolicyId};
use torsten_primitives::protocol_params::ProtocolParameters;
use torsten_primitives::time::SlotNo;
use torsten_primitives::transaction::*;
use torsten_primitives::value::{AssetName, Lovelace, Value};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn byron_addr() -> Address {
    Address::Byron(ByronAddress {
        payload: vec![0u8; 32],
    })
}

fn make_utxo_set(input_value: u64) -> (UtxoSet, TransactionInput) {
    let mut utxo_set = UtxoSet::new();
    let input = TransactionInput {
        transaction_id: Hash32::from_bytes([1u8; 32]),
        index: 0,
    };
    let output = TransactionOutput {
        address: byron_addr(),
        value: Value::lovelace(input_value),
        datum: OutputDatum::None,
        script_ref: None,
        is_legacy: false,
        raw_cbor: None,
    };
    utxo_set.insert(input.clone(), output);
    (utxo_set, input)
}

fn make_tx(input: TransactionInput, output_value: u64, fee: u64) -> Transaction {
    Transaction {
        era: torsten_primitives::era::Era::Conway,
        hash: Hash32::ZERO,
        body: TransactionBody {
            inputs: vec![input],
            outputs: vec![TransactionOutput {
                address: byron_addr(),
                value: Value::lovelace(output_value),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            }],
            fee: Lovelace(fee),
            ttl: None,
            certificates: vec![],
            withdrawals: BTreeMap::new(),
            auxiliary_data_hash: None,
            validity_interval_start: None,
            mint: BTreeMap::new(),
            script_data_hash: None,
            collateral: vec![],
            required_signers: vec![],
            network_id: None,
            collateral_return: None,
            total_collateral: None,
            reference_inputs: vec![],
            update: None,
            voting_procedures: BTreeMap::new(),
            proposal_procedures: vec![],
            treasury_value: None,
            donation: None,
        },
        witness_set: TransactionWitnessSet {
            vkey_witnesses: vec![],
            native_scripts: vec![],
            bootstrap_witnesses: vec![],
            plutus_v1_scripts: vec![],
            plutus_v2_scripts: vec![],
            plutus_v3_scripts: vec![],
            plutus_data: vec![],
            redeemers: vec![],
            raw_redeemers_cbor: None,
            raw_plutus_data_cbor: None,
            pallas_script_data_hash: None,
        },
        is_valid: true,
        auxiliary_data: None,
        raw_cbor: None,
        raw_body_cbor: None,
        raw_witness_cbor: None,
    }
}

fn params() -> ProtocolParameters {
    ProtocolParameters::mainnet_defaults()
}

fn has_error<F: Fn(&ValidationError) -> bool>(errors: &[ValidationError], pred: F) -> bool {
    errors.iter().any(pred)
}

// ---------------------------------------------------------------------------
// Property 1: Fee conservation — valid tx has sum(inputs) == sum(outputs) + fee
// ---------------------------------------------------------------------------

proptest! {
    #[test]
    fn fee_conservation(
        fee in 200_000u64..1_000_000u64,
        output_value in 1_000_000u64..9_000_000u64,
    ) {
        let input_value = fee + output_value;
        let (utxo_set, input) = make_utxo_set(input_value);
        let p = params();
        let tx = make_tx(input, output_value, fee);

        // With proper conservation, only FeeTooSmall or OutputTooSmall can trigger
        // (ValueNotConserved should never appear)
        let result = validate_transaction(&tx, &utxo_set, &p, 100, 300, None);
        match result {
            Ok(()) => {} // Fine
            Err(errors) => {
                prop_assert!(
                    !has_error(&errors, |e| matches!(e, ValidationError::ValueNotConserved { .. })),
                    "ValueNotConserved should not appear when input == output + fee\nerrors: {:?}",
                    errors
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Property 2: Breaking conservation always produces ValueNotConserved
// ---------------------------------------------------------------------------

proptest! {
    #[test]
    fn broken_conservation_detected(
        fee in 200_000u64..500_000u64,
        output_value in 2_000_000u64..9_000_000u64,
        extra in 1u64..1_000_000u64,
    ) {
        // output + fee > input_value
        let input_value = fee + output_value;
        let (utxo_set, input) = make_utxo_set(input_value);
        let p = params();
        // Make output too large by `extra`
        let tx = make_tx(input, output_value + extra, fee);

        let result = validate_transaction(&tx, &utxo_set, &p, 100, 300, None);
        prop_assert!(result.is_err(), "Should reject tx with output > input - fee");
        let errors = result.unwrap_err();
        prop_assert!(
            has_error(&errors, |e| matches!(e, ValidationError::ValueNotConserved { .. })),
            "Should contain ValueNotConserved error, got: {:?}",
            errors
        );
    }
}

// ---------------------------------------------------------------------------
// Property 3: Min fee monotonicity — larger tx_size → larger min_fee
// ---------------------------------------------------------------------------

proptest! {
    #[test]
    fn min_fee_monotonic(
        size_a in 200u64..10_000u64,
        delta in 1u64..10_000u64,
    ) {
        let p = params();
        let size_b = size_a + delta;
        let fee_a = p.min_fee(size_a);
        let fee_b = p.min_fee(size_b);
        prop_assert!(fee_b >= fee_a,
            "min_fee should be monotonically increasing: min_fee({}) = {:?} > min_fee({}) = {:?}",
            size_a, fee_a, size_b, fee_b);
    }
}

// ---------------------------------------------------------------------------
// Property 4: Fee too small is rejected
// ---------------------------------------------------------------------------

proptest! {
    #[test]
    fn fee_too_small_rejected(
        output_value in 2_000_000u64..9_000_000u64,
    ) {
        let fee = 1u64; // Way below min_fee
        let input_value = output_value + fee;
        let (utxo_set, input) = make_utxo_set(input_value);
        let p = params();
        let tx = make_tx(input, output_value, fee);

        let result = validate_transaction(&tx, &utxo_set, &p, 100, 300, None);
        prop_assert!(result.is_err());
        let errors = result.unwrap_err();
        prop_assert!(
            has_error(&errors, |e| matches!(e, ValidationError::FeeTooSmall { .. })),
            "Should contain FeeTooSmall, got: {:?}",
            errors
        );
    }
}

// ---------------------------------------------------------------------------
// Property 5: No inputs always rejected
// ---------------------------------------------------------------------------

#[test]
fn no_inputs_rejected() {
    let utxo_set = UtxoSet::new();
    let p = params();
    let mut tx = make_tx(
        TransactionInput {
            transaction_id: Hash32::ZERO,
            index: 0,
        },
        0,
        0,
    );
    tx.body.inputs.clear();

    let result = validate_transaction(&tx, &utxo_set, &p, 100, 300, None);
    assert!(result.is_err());
    let errors = result.unwrap_err();
    assert!(has_error(&errors, |e| matches!(
        e,
        ValidationError::NoInputs
    )));
}

// ---------------------------------------------------------------------------
// Property 6: Duplicate inputs rejected
// ---------------------------------------------------------------------------

#[test]
fn duplicate_inputs_rejected() {
    let (utxo_set, input) = make_utxo_set(20_000_000);
    let p = params();
    let mut tx = make_tx(input.clone(), 9_800_000, 200_000);
    tx.body.inputs.push(input); // Duplicate

    let result = validate_transaction(&tx, &utxo_set, &p, 100, 300, None);
    assert!(result.is_err());
    let errors = result.unwrap_err();
    assert!(has_error(&errors, |e| {
        matches!(e, ValidationError::DuplicateInput(..))
    }));
}

// ---------------------------------------------------------------------------
// Property 7: TTL expired is rejected
// ---------------------------------------------------------------------------

proptest! {
    #[test]
    fn ttl_expired_rejected(
        ttl in 1u64..1_000u64,
        current_slot_delta in 1u64..1_000u64,
    ) {
        let current_slot = ttl + current_slot_delta; // Past TTL
        let fee = 200_000u64;
        let output = 9_800_000u64;
        let (utxo_set, input) = make_utxo_set(fee + output);
        let p = params();
        let mut tx = make_tx(input, output, fee);
        tx.body.ttl = Some(SlotNo(ttl));

        let result = validate_transaction(&tx, &utxo_set, &p, current_slot, 300, None);
        prop_assert!(result.is_err());
        let errors = result.unwrap_err();
        prop_assert!(
            has_error(&errors, |e| matches!(e, ValidationError::TtlExpired { .. })),
            "Should contain TtlExpired, got: {:?}",
            errors
        );
    }
}

// ---------------------------------------------------------------------------
// Property 8: Valid TTL accepted
// ---------------------------------------------------------------------------

proptest! {
    #[test]
    fn valid_ttl_accepted(
        current_slot in 1u64..1_000u64,
        ttl_delta in 1u64..1_000u64,
    ) {
        let ttl = current_slot + ttl_delta; // In the future
        let fee = 200_000u64;
        let output = 9_800_000u64;
        let (utxo_set, input) = make_utxo_set(fee + output);
        let p = params();
        let mut tx = make_tx(input, output, fee);
        tx.body.ttl = Some(SlotNo(ttl));

        let result = validate_transaction(&tx, &utxo_set, &p, current_slot, 300, None);
        // Should not fail with TtlExpired (might fail with other errors like FeeTooSmall)
        if let Err(errors) = &result {
            prop_assert!(
                !has_error(errors, |e| matches!(e, ValidationError::TtlExpired { .. })),
                "Should not contain TtlExpired with future TTL, got: {:?}",
                errors
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Property 9: Input not found in UTxO set
// ---------------------------------------------------------------------------

#[test]
fn missing_input_rejected() {
    let utxo_set = UtxoSet::new(); // Empty
    let p = params();
    let input = TransactionInput {
        transaction_id: Hash32::from_bytes([99u8; 32]),
        index: 0,
    };
    let tx = make_tx(input, 5_000_000, 200_000);

    let result = validate_transaction(&tx, &utxo_set, &p, 100, 300, None);
    assert!(result.is_err());
    let errors = result.unwrap_err();
    assert!(has_error(&errors, |e| {
        matches!(e, ValidationError::InputNotFound(..))
    }));
}

// ---------------------------------------------------------------------------
// Property 10: Min UTxO monotonicity — larger output → larger min_utxo
// ---------------------------------------------------------------------------

proptest! {
    #[test]
    fn min_utxo_monotonic(
        size_a in 20u64..5_000u64,
        delta in 1u64..5_000u64,
    ) {
        let p = params();
        let size_b = size_a + delta;
        let min_a = p.min_utxo_for_output_size(size_a);
        let min_b = p.min_utxo_for_output_size(size_b);
        prop_assert!(min_b >= min_a,
            "min_utxo should increase with output size: min_utxo({}) = {:?} > min_utxo({}) = {:?}",
            size_a, min_a, size_b, min_b);
    }
}

// ---------------------------------------------------------------------------
// Property 11: Output too small is rejected
// ---------------------------------------------------------------------------

#[test]
fn output_too_small_rejected() {
    let fee = 200_000u64;
    let output_value = 100u64; // Way below min_utxo
    let input_value = fee + output_value;
    let (utxo_set, input) = make_utxo_set(input_value);
    let p = params();
    let tx = make_tx(input, output_value, fee);

    let result = validate_transaction(&tx, &utxo_set, &p, 100, 300, None);
    assert!(result.is_err());
    let errors = result.unwrap_err();
    assert!(
        has_error(&errors, |e| matches!(
            e,
            ValidationError::OutputTooSmall { .. }
        )),
        "Should contain OutputTooSmall, got: {:?}",
        errors
    );
}

// ---------------------------------------------------------------------------
// Property 12: Collateral sufficiency — sum(collateral) >= fee * pct / 100
// ---------------------------------------------------------------------------

fn make_plutus_tx(
    input: TransactionInput,
    output_value: u64,
    fee: u64,
    collateral: Vec<TransactionInput>,
) -> Transaction {
    let mut tx = make_tx(input, output_value, fee);
    tx.body.collateral = collateral;
    // Add a redeemer to make it a "Plutus transaction"
    tx.witness_set
        .redeemers
        .push(torsten_primitives::transaction::Redeemer {
            tag: torsten_primitives::transaction::RedeemerTag::Spend,
            index: 0,
            data: torsten_primitives::transaction::PlutusData::Integer(0),
            ex_units: torsten_primitives::transaction::ExUnits {
                mem: 1000,
                steps: 1000,
            },
        });
    tx
}

proptest! {
    #[test]
    fn collateral_sufficient_no_error(
        fee in 200_000u64..1_000_000u64,
        extra_pct in 0u64..100u64,
    ) {
        let p = params();
        let collateral_pct = p.collateral_percentage;
        // Use ceiling division to match Haskell: ceil(fee * pct / 100).
        // The old truncating formula `fee * pct / 100` could be 1 below the
        // required amount when the product is not an exact multiple of 100.
        let required = (fee * collateral_pct).div_ceil(100);
        let collateral_value = required + extra_pct * 1000; // Always sufficient

        let output_value = 5_000_000u64;
        let input_value = fee + output_value;
        let (mut utxo_set, input) = make_utxo_set(input_value);

        // Add collateral UTxO
        let col_input = TransactionInput {
            transaction_id: Hash32::from_bytes([2u8; 32]),
            index: 0,
        };
        utxo_set.insert(col_input.clone(), TransactionOutput {
            address: byron_addr(),
            value: Value::lovelace(collateral_value),
            datum: OutputDatum::None,
            script_ref: None,
            is_legacy: false,
            raw_cbor: None,
        });

        let tx = make_plutus_tx(input, output_value, fee, vec![col_input]);
        let result = validate_transaction(&tx, &utxo_set, &p, 100, 300, None);

        // Should not contain InsufficientCollateral
        if let Err(errors) = &result {
            prop_assert!(
                !has_error(errors, |e| matches!(e, ValidationError::InsufficientCollateral)),
                "Should not have InsufficientCollateral with sufficient collateral, got: {:?}",
                errors
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Property 13: Insufficient collateral is rejected
// ---------------------------------------------------------------------------

proptest! {
    #[test]
    fn collateral_insufficient_rejected(
        fee in 500_000u64..2_000_000u64,
    ) {
        let p = params();
        let collateral_pct = p.collateral_percentage;
        let required = fee * collateral_pct / 100;
        // Provide collateral that's 1 lovelace short
        let collateral_value = required.saturating_sub(1);

        let output_value = 5_000_000u64;
        let input_value = fee + output_value;
        let (mut utxo_set, input) = make_utxo_set(input_value);

        let col_input = TransactionInput {
            transaction_id: Hash32::from_bytes([2u8; 32]),
            index: 0,
        };
        utxo_set.insert(col_input.clone(), TransactionOutput {
            address: byron_addr(),
            value: Value::lovelace(collateral_value),
            datum: OutputDatum::None,
            script_ref: None,
            is_legacy: false,
            raw_cbor: None,
        });

        let tx = make_plutus_tx(input, output_value, fee, vec![col_input]);
        let result = validate_transaction(&tx, &utxo_set, &p, 100, 300, None);

        prop_assert!(result.is_err());
        let errors = result.unwrap_err();
        prop_assert!(
            has_error(&errors, |e| matches!(e, ValidationError::InsufficientCollateral)),
            "Should contain InsufficientCollateral, got: {:?}",
            errors
        );
    }
}

// ---------------------------------------------------------------------------
// Property 14: Reference input not found is rejected
// ---------------------------------------------------------------------------

#[test]
fn reference_input_not_found_rejected() {
    let fee = 200_000u64;
    let output_value = 9_800_000u64;
    let (utxo_set, input) = make_utxo_set(fee + output_value);
    let p = params();
    let mut tx = make_tx(input, output_value, fee);

    // Add a reference input that doesn't exist in the UTxO set
    tx.body.reference_inputs.push(TransactionInput {
        transaction_id: Hash32::from_bytes([99u8; 32]),
        index: 7,
    });

    let result = validate_transaction(&tx, &utxo_set, &p, 100, 300, None);
    assert!(result.is_err());
    let errors = result.unwrap_err();
    assert!(
        has_error(&errors, |e| matches!(
            e,
            ValidationError::ReferenceInputNotFound(..)
        )),
        "Should contain ReferenceInputNotFound, got: {:?}",
        errors
    );
}

// ---------------------------------------------------------------------------
// Property 15: Validity interval start in the future is rejected
// ---------------------------------------------------------------------------

proptest! {
    #[test]
    fn validity_start_in_future_rejected(
        current_slot in 1u64..10_000u64,
        future_delta in 1u64..10_000u64,
    ) {
        let valid_from = current_slot + future_delta;
        let fee = 200_000u64;
        let output_value = 9_800_000u64;
        let (utxo_set, input) = make_utxo_set(fee + output_value);
        let p = params();
        let mut tx = make_tx(input, output_value, fee);
        tx.body.validity_interval_start = Some(SlotNo(valid_from));

        let result = validate_transaction(&tx, &utxo_set, &p, current_slot, 300, None);
        prop_assert!(result.is_err());
        let errors = result.unwrap_err();
        prop_assert!(
            has_error(&errors, |e| matches!(e, ValidationError::NotYetValid { .. })),
            "Should contain NotYetValid, got: {:?}",
            errors
        );
    }
}

// ---------------------------------------------------------------------------
// Property 16: Reference input overlapping regular input is rejected
// ---------------------------------------------------------------------------

#[test]
fn reference_input_overlaps_rejected() {
    let fee = 200_000u64;
    let output_value = 9_800_000u64;
    let (utxo_set, input) = make_utxo_set(fee + output_value);
    let p = params();
    let mut tx = make_tx(input.clone(), output_value, fee);
    // Reference the same input that's used as a regular input
    tx.body.reference_inputs.push(input);

    let result = validate_transaction(&tx, &utxo_set, &p, 100, 300, None);
    assert!(result.is_err());
    let errors = result.unwrap_err();
    assert!(
        has_error(&errors, |e| matches!(
            e,
            ValidationError::ReferenceInputOverlapsInput(..)
        )),
        "Should contain ReferenceInputOverlapsInput, got: {:?}",
        errors
    );
}

// ===========================================================================
// Multi-asset mint / burn conservation tests (Properties 17–23)
// ===========================================================================
//
// # Design rationale
//
// Rule 3c requires every policy in `tx.body.mint` to have a matching script in
// the witness set.  We satisfy this with `NativeScript::ScriptAll(vec![])` — a
// native script that always evaluates to `true` (zero sub-scripts, no pubkey
// required, no time constraint).  Its policy ID is the blake2b-224 of the
// zero-prefixed CBOR encoding of the script:
//
//     policy_id = blake2b_224(0x00 || cbor([1, []]))
//
// `make_native_script_policy` below returns a `(NativeScript, PolicyId)` pair
// that is self-consistent: the native script in the witness set hashes to the
// returned policy ID.
//
// Rule 9b (witness completeness) skips Byron addresses, so using `byron_addr()`
// for all UTxOs avoids the vkey-witness requirement entirely.
//
// The minimum UTxO value for outputs that carry tokens is also set to 0 in the
// test params so that multi-asset outputs do not trigger `OutputTooSmall`.  We
// override only `ada_per_utxo_byte` to zero; all other params stay at
// `mainnet_defaults()`.

// ---------------------------------------------------------------------------
// Helpers for multi-asset tests
// ---------------------------------------------------------------------------

/// Return `(NativeScript, PolicyId)` for a deterministic always-succeeding
/// native policy script.  `seed` differentiates multiple policies in a single
/// transaction by constructing nested `ScriptAll` layers.
///
/// - seed=0 → `ScriptAll([])`           → depth-0, always true
/// - seed=1 → `ScriptAll([ScriptAll([])])` → depth-1, always true
/// - seed=2 → `ScriptAll([ScriptAll([ScriptAll([])])])` → depth-2, always true
///
/// Each nesting level produces a distinct blake2b-224 hash, so seeds 0..=2
/// give three independent, non-colliding policy IDs.
fn make_native_script_policy(seed: u8) -> (NativeScript, PolicyId) {
    // Build a nested ScriptAll chain of depth `seed`.
    let mut script = NativeScript::ScriptAll(vec![]);
    for _ in 0..seed {
        script = NativeScript::ScriptAll(vec![script]);
    }

    // Compute policy_id = blake2b_224(0x00 || cbor(script))
    let script_cbor = torsten_serialization::encode_native_script(&script);
    let mut tagged = Vec::with_capacity(1 + script_cbor.len());
    tagged.push(0x00u8);
    tagged.extend_from_slice(&script_cbor);
    let policy_id = blake2b_224(&tagged);

    (script, policy_id)
}

/// Fixed asset name used throughout the multi-asset tests.
fn token_name() -> AssetName {
    AssetName::new(b"TestToken".to_vec()).expect("valid asset name")
}

/// Build a `ProtocolParameters` with `ada_per_utxo_byte = 0` so that outputs
/// containing tokens never trigger `OutputTooSmall`.  All other parameters
/// remain at their mainnet defaults, including `min_fee_a / min_fee_b` for the
/// fee check.
fn params_zero_min_utxo() -> ProtocolParameters {
    let mut p = ProtocolParameters::mainnet_defaults();
    // Zero out the per-byte coin requirement so multi-asset outputs that carry
    // modest lovelace are not rejected for being below min UTxO.
    p.ada_per_utxo_byte = Lovelace(0);
    p
}

/// Insert a UTxO entry carrying both ADA and `token_qty` of `(policy, name)`
/// into `utxo_set`, and return the corresponding `TransactionInput`.
///
/// `tx_id_byte` is used to make the transaction hash unique so that multiple
/// calls do not collide on the same UTxO key.
fn insert_token_utxo(
    utxo_set: &mut UtxoSet,
    tx_id_byte: u8,
    coin: u64,
    policy: PolicyId,
    name: AssetName,
    token_qty: u64,
) -> TransactionInput {
    let input = TransactionInput {
        transaction_id: Hash32::from_bytes([tx_id_byte; 32]),
        index: 0,
    };
    let mut value = Value::lovelace(coin);
    value
        .multi_asset
        .entry(policy)
        .or_default()
        .insert(name, token_qty);
    let output = TransactionOutput {
        address: byron_addr(),
        value,
        datum: OutputDatum::None,
        script_ref: None,
        is_legacy: false,
        raw_cbor: None,
    };
    utxo_set.insert(input.clone(), output);
    input
}

/// Build a transaction that mints `mint_qty` tokens under `policy`, takes an
/// ADA-only input, and produces a single output carrying both ADA and the
/// newly minted tokens.
///
/// ADA conservation: input_coin == output_coin + fee
/// Token conservation: mint_qty (minted) == token_qty_out (in output)
#[allow(clippy::too_many_arguments)]
fn make_mint_tx(
    ada_input: TransactionInput,
    output_coin: u64,
    fee: u64,
    policy: PolicyId,
    script: NativeScript,
    name: AssetName,
    mint_qty: i64,  // tokens minted (positive)
    token_out: u64, // tokens placed in the output
) -> Transaction {
    let mut mint_map: BTreeMap<PolicyId, BTreeMap<AssetName, i64>> = BTreeMap::new();
    mint_map
        .entry(policy)
        .or_default()
        .insert(name.clone(), mint_qty);

    let mut out_value = Value::lovelace(output_coin);
    if token_out > 0 {
        out_value
            .multi_asset
            .entry(policy)
            .or_default()
            .insert(name, token_out);
    }

    Transaction {
        era: torsten_primitives::era::Era::Conway,
        hash: Hash32::ZERO,
        body: TransactionBody {
            inputs: vec![ada_input],
            outputs: vec![TransactionOutput {
                address: byron_addr(),
                value: out_value,
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            }],
            fee: Lovelace(fee),
            ttl: None,
            certificates: vec![],
            withdrawals: BTreeMap::new(),
            auxiliary_data_hash: None,
            validity_interval_start: None,
            mint: mint_map,
            script_data_hash: None,
            collateral: vec![],
            required_signers: vec![],
            network_id: None,
            collateral_return: None,
            total_collateral: None,
            reference_inputs: vec![],
            update: None,
            voting_procedures: BTreeMap::new(),
            proposal_procedures: vec![],
            treasury_value: None,
            donation: None,
        },
        witness_set: TransactionWitnessSet {
            vkey_witnesses: vec![],
            native_scripts: vec![script],
            bootstrap_witnesses: vec![],
            plutus_v1_scripts: vec![],
            plutus_v2_scripts: vec![],
            plutus_v3_scripts: vec![],
            plutus_data: vec![],
            redeemers: vec![],
            raw_redeemers_cbor: None,
            raw_plutus_data_cbor: None,
            pallas_script_data_hash: None,
        },
        is_valid: true,
        auxiliary_data: None,
        raw_cbor: None,
        raw_body_cbor: None,
        raw_witness_cbor: None,
    }
}

// ---------------------------------------------------------------------------
// Property 17: Random mint quantities always conserve multi-asset value
// ---------------------------------------------------------------------------
//
// For any mint_qty > 0 tokens minted under a valid policy, placing all minted
// tokens in the output satisfies multi-asset conservation.  The validator must
// not raise `MultiAssetNotConserved` or `ValueNotConserved`.

proptest! {
    #[test]
    fn mint_quantity_conserved(
        fee in 200_000u64..500_000u64,
        output_coin in 2_000_000u64..5_000_000u64,
        mint_qty in 1i64..1_000_000i64,
    ) {
        let (script, policy) = make_native_script_policy(0);
        let name = token_name();

        let input_coin = output_coin + fee; // ADA conservation
        let (utxo_set, ada_input) = make_utxo_set(input_coin);
        let p = params_zero_min_utxo();

        // Mint `mint_qty` tokens and place all of them in the output.
        // Token conservation: 0 (in inputs) + mint_qty (minted) - mint_qty (in output) = 0
        let tx = make_mint_tx(
            ada_input,
            output_coin,
            fee,
            policy,
            script,
            name,
            mint_qty,
            mint_qty as u64, // all minted tokens go to the output
        );

        let result = validate_transaction(&tx, &utxo_set, &p, 100, 300, None);
        match result {
            Ok(()) => {}
            Err(errors) => {
                prop_assert!(
                    !has_error(&errors, |e| matches!(e, ValidationError::ValueNotConserved { .. })),
                    "ADA conservation should hold: fee={fee}, out={output_coin}\nerrors: {errors:?}"
                );
                prop_assert!(
                    !has_error(&errors, |e| matches!(e, ValidationError::MultiAssetNotConserved { .. })),
                    "Multi-asset conservation should hold: mint_qty={mint_qty}\nerrors: {errors:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Property 18: Mint conservation failure is detected
// ---------------------------------------------------------------------------
//
// When minted tokens are not placed in any output, the validator must raise
// `MultiAssetNotConserved` because the token balance is positive on the input
// side (minted) but zero on the output side.

proptest! {
    #[test]
    fn mint_without_output_rejected(
        fee in 200_000u64..500_000u64,
        output_coin in 2_000_000u64..5_000_000u64,
        mint_qty in 1i64..1_000_000i64,
    ) {
        let (script, policy) = make_native_script_policy(0);
        let name = token_name();

        let input_coin = output_coin + fee;
        let (utxo_set, ada_input) = make_utxo_set(input_coin);
        let p = params_zero_min_utxo();

        // Mint tokens but send NONE to the output — conservation is violated.
        let tx = make_mint_tx(
            ada_input,
            output_coin,
            fee,
            policy,
            script,
            name,
            mint_qty,
            0, // no tokens in output
        );

        let result = validate_transaction(&tx, &utxo_set, &p, 100, 300, None);
        prop_assert!(result.is_err(), "Should reject: minted tokens not placed in output");
        let errors = result.unwrap_err();
        prop_assert!(
            has_error(&errors, |e| matches!(e, ValidationError::MultiAssetNotConserved { .. })),
            "Should contain MultiAssetNotConserved, got: {errors:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Property 19: Burning (negative minting) correctly reduces token supply
// ---------------------------------------------------------------------------
//
// Start with a UTxO holding `initial_qty` tokens.  Burn `burn_qty <= initial_qty`
// tokens.  Send the remaining `initial_qty - burn_qty` tokens to the output.
// Conservation must hold for both ADA and the token.

proptest! {
    #[test]
    fn burn_reduces_supply_conserved(
        fee in 200_000u64..500_000u64,
        output_coin in 2_000_000u64..5_000_000u64,
        initial_qty in 1u64..1_000_000u64,
        burn_frac in 1u64..100u64, // burn 1%..100% of holdings
    ) {
        let (script, policy) = make_native_script_policy(0);
        let name = token_name();
        let p = params_zero_min_utxo();

        // Determine how many tokens to burn (at least 1, at most initial_qty).
        let burn_qty = (initial_qty * burn_frac / 100).max(1).min(initial_qty);
        let remaining_qty = initial_qty - burn_qty;

        let input_coin = output_coin + fee;

        // Build a UTxO set with one ADA-only input and one token-bearing input.
        let mut utxo_set = UtxoSet::new();

        let ada_input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            ada_input.clone(),
            TransactionOutput {
                address: byron_addr(),
                value: Value::lovelace(input_coin),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        let token_input = insert_token_utxo(
            &mut utxo_set,
            2u8, // distinct tx id byte
            0,   // no additional ADA on the token UTxO
            policy,
            name.clone(),
            initial_qty,
        );

        // Build the mint map with negative quantity (burn).
        let mut mint_map: BTreeMap<PolicyId, BTreeMap<AssetName, i64>> = BTreeMap::new();
        mint_map
            .entry(policy)
            .or_default()
            .insert(name.clone(), -(burn_qty as i64));

        // Output: carries the remaining tokens and all the ADA (minus fee).
        let mut out_value = Value::lovelace(output_coin);
        if remaining_qty > 0 {
            out_value
                .multi_asset
                .entry(policy)
                .or_default()
                .insert(name, remaining_qty);
        }

        let tx = Transaction {
            era: torsten_primitives::era::Era::Conway,
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![ada_input, token_input],
                outputs: vec![TransactionOutput {
                    address: byron_addr(),
                    value: out_value,
                    datum: OutputDatum::None,
                    script_ref: None,
                    is_legacy: false,
                    raw_cbor: None,
                }],
                fee: Lovelace(fee),
                ttl: None,
                certificates: vec![],
                withdrawals: BTreeMap::new(),
                auxiliary_data_hash: None,
                validity_interval_start: None,
                mint: mint_map,
                script_data_hash: None,
                collateral: vec![],
                required_signers: vec![],
                network_id: None,
                collateral_return: None,
                total_collateral: None,
                reference_inputs: vec![],
                update: None,
                voting_procedures: BTreeMap::new(),
                proposal_procedures: vec![],
                treasury_value: None,
                donation: None,
            },
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![script],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![],
                plutus_v3_scripts: vec![],
                plutus_data: vec![],
                redeemers: vec![],
                raw_redeemers_cbor: None,
                raw_plutus_data_cbor: None,
                pallas_script_data_hash: None,
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
            raw_body_cbor: None,
            raw_witness_cbor: None,
        };

        let result = validate_transaction(&tx, &utxo_set, &p, 100, 300, None);
        match result {
            Ok(()) => {}
            Err(errors) => {
                prop_assert!(
                    !has_error(&errors, |e| matches!(e, ValidationError::ValueNotConserved { .. })),
                    "ADA conservation should hold: fee={fee}, out={output_coin}\nerrors: {errors:?}"
                );
                prop_assert!(
                    !has_error(&errors, |e| matches!(e, ValidationError::MultiAssetNotConserved { .. })),
                    "Token conservation should hold: initial={initial_qty}, burn={burn_qty}, remaining={remaining_qty}\nerrors: {errors:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Property 20: Burning more than held is rejected
// ---------------------------------------------------------------------------
//
// If we burn more tokens than exist in the inputs, the multi-asset balance is
// negative (more tokens leave than entered), which must be detected.

proptest! {
    #[test]
    fn over_burn_rejected(
        fee in 200_000u64..500_000u64,
        output_coin in 2_000_000u64..5_000_000u64,
        held_qty in 1u64..500_000u64,
        excess in 1u64..500_000u64,
    ) {
        let (script, policy) = make_native_script_policy(0);
        let name = token_name();
        let p = params_zero_min_utxo();

        let burn_qty = held_qty + excess; // more than we hold

        let input_coin = output_coin + fee;
        let mut utxo_set = UtxoSet::new();

        let ada_input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            ada_input.clone(),
            TransactionOutput {
                address: byron_addr(),
                value: Value::lovelace(input_coin),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        let token_input = insert_token_utxo(
            &mut utxo_set,
            2u8,
            0,
            policy,
            name.clone(),
            held_qty,
        );

        let mut mint_map: BTreeMap<PolicyId, BTreeMap<AssetName, i64>> = BTreeMap::new();
        mint_map
            .entry(policy)
            .or_default()
            .insert(name, -(burn_qty as i64));

        let tx = Transaction {
            era: torsten_primitives::era::Era::Conway,
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![ada_input, token_input],
                outputs: vec![TransactionOutput {
                    address: byron_addr(),
                    value: Value::lovelace(output_coin), // no tokens in output
                    datum: OutputDatum::None,
                    script_ref: None,
                    is_legacy: false,
                    raw_cbor: None,
                }],
                fee: Lovelace(fee),
                ttl: None,
                certificates: vec![],
                withdrawals: BTreeMap::new(),
                auxiliary_data_hash: None,
                validity_interval_start: None,
                mint: mint_map,
                script_data_hash: None,
                collateral: vec![],
                required_signers: vec![],
                network_id: None,
                collateral_return: None,
                total_collateral: None,
                reference_inputs: vec![],
                update: None,
                voting_procedures: BTreeMap::new(),
                proposal_procedures: vec![],
                treasury_value: None,
                donation: None,
            },
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![script],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![],
                plutus_v3_scripts: vec![],
                plutus_data: vec![],
                redeemers: vec![],
                raw_redeemers_cbor: None,
                raw_plutus_data_cbor: None,
                pallas_script_data_hash: None,
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
            raw_body_cbor: None,
            raw_witness_cbor: None,
        };

        let result = validate_transaction(&tx, &utxo_set, &p, 100, 300, None);
        prop_assert!(result.is_err(), "Should reject: burning more tokens than held");
        let errors = result.unwrap_err();
        prop_assert!(
            has_error(&errors, |e| matches!(e, ValidationError::MultiAssetNotConserved { .. })),
            "Should contain MultiAssetNotConserved, got: {errors:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Property 21: Mixed mint and burn in a single transaction
// ---------------------------------------------------------------------------
//
// A transaction mints tokens under policy A and burns tokens under policy B
// simultaneously.  Conservation must hold independently for each policy:
//
//   Policy A: 0 (in inputs) + mint_qty (minted) - mint_qty (in output) = 0
//   Policy B: held_qty (in inputs) - burn_qty (burned) - (held_qty - burn_qty) (in output) = 0

proptest! {
    #[test]
    fn mixed_mint_and_burn_conserved(
        fee in 200_000u64..500_000u64,
        output_coin in 2_000_000u64..5_000_000u64,
        mint_qty in 1i64..100_000i64,
        held_qty in 1u64..100_000u64,
        burn_frac in 1u64..100u64,
    ) {
        // Two independent policies: seed 0 for minting, seed 1 for burning.
        let (script_a, policy_a) = make_native_script_policy(0);
        let (script_b, policy_b) = make_native_script_policy(1);
        let name_a = AssetName::new(b"MintToken".to_vec()).expect("valid");
        let name_b = AssetName::new(b"BurnToken".to_vec()).expect("valid");

        let p = params_zero_min_utxo();

        let burn_qty = (held_qty * burn_frac / 100).max(1).min(held_qty);
        let remaining_b = held_qty - burn_qty;

        let input_coin = output_coin + fee;
        let mut utxo_set = UtxoSet::new();

        // ADA-only input to cover the fee and output ADA.
        let ada_input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            ada_input.clone(),
            TransactionOutput {
                address: byron_addr(),
                value: Value::lovelace(input_coin),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        // Token-B-bearing input.
        let token_b_input = insert_token_utxo(
            &mut utxo_set,
            2u8,
            0,
            policy_b,
            name_b.clone(),
            held_qty,
        );

        // Mint map: +mint_qty of token A, -burn_qty of token B.
        let mut mint_map: BTreeMap<PolicyId, BTreeMap<AssetName, i64>> = BTreeMap::new();
        mint_map
            .entry(policy_a)
            .or_default()
            .insert(name_a.clone(), mint_qty);
        mint_map
            .entry(policy_b)
            .or_default()
            .insert(name_b.clone(), -(burn_qty as i64));

        // Output carries all minted token-A and the remaining token-B.
        let mut out_value = Value::lovelace(output_coin);
        out_value
            .multi_asset
            .entry(policy_a)
            .or_default()
            .insert(name_a, mint_qty as u64);
        if remaining_b > 0 {
            out_value
                .multi_asset
                .entry(policy_b)
                .or_default()
                .insert(name_b, remaining_b);
        }

        let tx = Transaction {
            era: torsten_primitives::era::Era::Conway,
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![ada_input, token_b_input],
                outputs: vec![TransactionOutput {
                    address: byron_addr(),
                    value: out_value,
                    datum: OutputDatum::None,
                    script_ref: None,
                    is_legacy: false,
                    raw_cbor: None,
                }],
                fee: Lovelace(fee),
                ttl: None,
                certificates: vec![],
                withdrawals: BTreeMap::new(),
                auxiliary_data_hash: None,
                validity_interval_start: None,
                mint: mint_map,
                script_data_hash: None,
                collateral: vec![],
                required_signers: vec![],
                network_id: None,
                collateral_return: None,
                total_collateral: None,
                reference_inputs: vec![],
                update: None,
                voting_procedures: BTreeMap::new(),
                proposal_procedures: vec![],
                treasury_value: None,
                donation: None,
            },
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                // Both policy scripts must be present in the witness set.
                native_scripts: vec![script_a, script_b],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![],
                plutus_v3_scripts: vec![],
                plutus_data: vec![],
                redeemers: vec![],
                raw_redeemers_cbor: None,
                raw_plutus_data_cbor: None,
                pallas_script_data_hash: None,
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
            raw_body_cbor: None,
            raw_witness_cbor: None,
        };

        let result = validate_transaction(&tx, &utxo_set, &p, 100, 300, None);
        match result {
            Ok(()) => {}
            Err(errors) => {
                prop_assert!(
                    !has_error(&errors, |e| matches!(e, ValidationError::ValueNotConserved { .. })),
                    "ADA conservation should hold\nerrors: {errors:?}"
                );
                prop_assert!(
                    !has_error(&errors, |e| matches!(e, ValidationError::MultiAssetNotConserved { .. })),
                    "Multi-asset conservation should hold: mint_qty={mint_qty}, held_b={held_qty}, burn_b={burn_qty}\nerrors: {errors:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Property 22: Mixed mint+burn with wrong output quantity is rejected
// ---------------------------------------------------------------------------
//
// Same as Property 21 but the output quantity for the minted token is off by 1
// (too many tokens claimed).  The validator must detect `MultiAssetNotConserved`.

proptest! {
    #[test]
    fn mixed_mint_burn_wrong_output_rejected(
        fee in 200_000u64..500_000u64,
        output_coin in 2_000_000u64..5_000_000u64,
        mint_qty in 1i64..100_000i64,
        held_qty in 1u64..100_000u64,
    ) {
        let (script_a, policy_a) = make_native_script_policy(0);
        let (script_b, policy_b) = make_native_script_policy(1);
        let name_a = AssetName::new(b"MintToken".to_vec()).expect("valid");
        let name_b = AssetName::new(b"BurnToken".to_vec()).expect("valid");

        let p = params_zero_min_utxo();

        // Burn all of token B.
        let burn_qty = held_qty;

        let input_coin = output_coin + fee;
        let mut utxo_set = UtxoSet::new();

        let ada_input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            ada_input.clone(),
            TransactionOutput {
                address: byron_addr(),
                value: Value::lovelace(input_coin),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        let token_b_input = insert_token_utxo(
            &mut utxo_set,
            2u8,
            0,
            policy_b,
            name_b.clone(),
            held_qty,
        );

        let mut mint_map: BTreeMap<PolicyId, BTreeMap<AssetName, i64>> = BTreeMap::new();
        mint_map
            .entry(policy_a)
            .or_default()
            .insert(name_a.clone(), mint_qty);
        mint_map
            .entry(policy_b)
            .or_default()
            .insert(name_b, -(burn_qty as i64));

        // WRONG: claim one more token-A in output than was minted.
        let mut out_value = Value::lovelace(output_coin);
        out_value
            .multi_asset
            .entry(policy_a)
            .or_default()
            .insert(name_a, mint_qty as u64 + 1); // off by 1

        let tx = Transaction {
            era: torsten_primitives::era::Era::Conway,
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![ada_input, token_b_input],
                outputs: vec![TransactionOutput {
                    address: byron_addr(),
                    value: out_value,
                    datum: OutputDatum::None,
                    script_ref: None,
                    is_legacy: false,
                    raw_cbor: None,
                }],
                fee: Lovelace(fee),
                ttl: None,
                certificates: vec![],
                withdrawals: BTreeMap::new(),
                auxiliary_data_hash: None,
                validity_interval_start: None,
                mint: mint_map,
                script_data_hash: None,
                collateral: vec![],
                required_signers: vec![],
                network_id: None,
                collateral_return: None,
                total_collateral: None,
                reference_inputs: vec![],
                update: None,
                voting_procedures: BTreeMap::new(),
                proposal_procedures: vec![],
                treasury_value: None,
                donation: None,
            },
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![script_a, script_b],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![],
                plutus_v3_scripts: vec![],
                plutus_data: vec![],
                redeemers: vec![],
                raw_redeemers_cbor: None,
                raw_plutus_data_cbor: None,
                pallas_script_data_hash: None,
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
            raw_body_cbor: None,
            raw_witness_cbor: None,
        };

        let result = validate_transaction(&tx, &utxo_set, &p, 100, 300, None);
        prop_assert!(result.is_err(), "Should reject: output token-A quantity exceeds minted amount");
        let errors = result.unwrap_err();
        prop_assert!(
            has_error(&errors, |e| matches!(e, ValidationError::MultiAssetNotConserved { .. })),
            "Should contain MultiAssetNotConserved, got: {errors:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Property 23: Missing minting policy script is rejected (Rule 3c)
// ---------------------------------------------------------------------------
//
// A transaction that mints tokens but provides no matching script in the
// witness set must be rejected with `InvalidMint`, regardless of whether
// ADA and token quantities balance.

#[test]
fn minting_without_policy_script_rejected() {
    let (script, policy) = make_native_script_policy(0);
    let name = token_name();
    let p = params_zero_min_utxo();

    let fee = 200_000u64;
    let output_coin = 5_000_000u64;
    let mint_qty = 100i64;

    let (utxo_set, ada_input) = make_utxo_set(output_coin + fee);

    // Build the transaction with the correct token balance but NO native script.
    let mut mint_map: BTreeMap<PolicyId, BTreeMap<AssetName, i64>> = BTreeMap::new();
    mint_map
        .entry(policy)
        .or_default()
        .insert(name.clone(), mint_qty);

    let mut out_value = Value::lovelace(output_coin);
    out_value
        .multi_asset
        .entry(policy)
        .or_default()
        .insert(name, mint_qty as u64);

    // We deliberately omit `script` from native_scripts to trigger Rule 3c.
    let _ = script; // consumed to silence the unused-variable warning

    let tx = Transaction {
        era: torsten_primitives::era::Era::Conway,
        hash: Hash32::ZERO,
        body: TransactionBody {
            inputs: vec![ada_input],
            outputs: vec![TransactionOutput {
                address: byron_addr(),
                value: out_value,
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            }],
            fee: Lovelace(fee),
            ttl: None,
            certificates: vec![],
            withdrawals: BTreeMap::new(),
            auxiliary_data_hash: None,
            validity_interval_start: None,
            mint: mint_map,
            script_data_hash: None,
            collateral: vec![],
            required_signers: vec![],
            network_id: None,
            collateral_return: None,
            total_collateral: None,
            reference_inputs: vec![],
            update: None,
            voting_procedures: BTreeMap::new(),
            proposal_procedures: vec![],
            treasury_value: None,
            donation: None,
        },
        witness_set: TransactionWitnessSet {
            vkey_witnesses: vec![],
            native_scripts: vec![], // no policy script — triggers InvalidMint
            bootstrap_witnesses: vec![],
            plutus_v1_scripts: vec![],
            plutus_v2_scripts: vec![],
            plutus_v3_scripts: vec![],
            plutus_data: vec![],
            redeemers: vec![],
            raw_redeemers_cbor: None,
            raw_plutus_data_cbor: None,
            pallas_script_data_hash: None,
        },
        is_valid: true,
        auxiliary_data: None,
        raw_cbor: None,
        raw_body_cbor: None,
        raw_witness_cbor: None,
    };

    let result = validate_transaction(&tx, &utxo_set, &p, 100, 300, None);
    assert!(result.is_err(), "Should reject: no minting policy script");
    let errors = result.unwrap_err();
    assert!(
        has_error(&errors, |e| matches!(e, ValidationError::InvalidMint)),
        "Should contain InvalidMint, got: {errors:?}"
    );
}
