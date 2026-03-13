//! Property-based tests for transaction validation invariants.
//!
//! Tests use simple ADA-only, single-input/single-output transactions to verify
//! fee conservation, min fee monotonicity, and rejection of invalid mutations.

use proptest::prelude::*;
use std::collections::BTreeMap;
use torsten_ledger::validation::{validate_transaction, ValidationError};
use torsten_ledger::UtxoSet;
use torsten_primitives::address::{Address, ByronAddress};
use torsten_primitives::hash::Hash32;
use torsten_primitives::protocol_params::ProtocolParameters;
use torsten_primitives::time::SlotNo;
use torsten_primitives::transaction::*;
use torsten_primitives::value::{Lovelace, Value};

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
        raw_cbor: None,
    };
    utxo_set.insert(input.clone(), output);
    (utxo_set, input)
}

fn make_tx(input: TransactionInput, output_value: u64, fee: u64) -> Transaction {
    Transaction {
        hash: Hash32::ZERO,
        body: TransactionBody {
            inputs: vec![input],
            outputs: vec![TransactionOutput {
                address: byron_addr(),
                value: Value::lovelace(output_value),
                datum: OutputDatum::None,
                script_ref: None,
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
        },
        is_valid: true,
        auxiliary_data: None,
        raw_cbor: None,
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
        let required = fee * collateral_pct / 100;
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
