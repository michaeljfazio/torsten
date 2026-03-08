use crate::utxo::UtxoSet;
use std::collections::HashSet;
use torsten_primitives::hash::Hash32;
use torsten_primitives::protocol_params::ProtocolParameters;
use torsten_primitives::time::SlotNo;
use torsten_primitives::transaction::{NativeScript, Transaction};
use torsten_primitives::value::Lovelace;
use tracing::{debug, trace, warn};

#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("No inputs in transaction")]
    NoInputs,
    #[error("Input not found in UTxO set: {0}")]
    InputNotFound(String),
    #[error("Value not conserved: inputs={inputs}, outputs={outputs}, fee={fee}")]
    ValueNotConserved { inputs: u64, outputs: u64, fee: u64 },
    #[error("Fee too small: minimum={minimum}, actual={actual}")]
    FeeTooSmall { minimum: u64, actual: u64 },
    #[error("Output too small: minimum={minimum}, actual={actual}")]
    OutputTooSmall { minimum: u64, actual: u64 },
    #[error("Transaction too large: maximum={maximum}, actual={actual}")]
    TxTooLarge { maximum: u64, actual: u64 },
    #[error("Missing required signer: {0}")]
    MissingRequiredSigner(String),
    #[error("Missing witness for input: {0}")]
    MissingWitness(String),
    #[error("TTL expired: current_slot={current_slot}, ttl={ttl}")]
    TtlExpired { current_slot: u64, ttl: u64 },
    #[error("Transaction not yet valid: current_slot={current_slot}, valid_from={valid_from}")]
    NotYetValid { current_slot: u64, valid_from: u64 },
    #[error("Script validation failed: {0}")]
    ScriptFailed(String),
    #[error("Insufficient collateral")]
    InsufficientCollateral,
    #[error("Negative minting without policy script")]
    InvalidMint,
    #[error("Max execution units exceeded")]
    ExUnitsExceeded,
}

/// Validate a transaction against the current UTxO set and protocol parameters
pub fn validate_transaction(
    tx: &Transaction,
    utxo_set: &UtxoSet,
    params: &ProtocolParameters,
    current_slot: u64,
    tx_size: u64,
) -> Result<(), Vec<ValidationError>> {
    trace!(
        tx_hash = %tx.hash.to_hex(),
        inputs = tx.body.inputs.len(),
        outputs = tx.body.outputs.len(),
        fee = tx.body.fee.0,
        tx_size,
        current_slot,
        "Validation: validating transaction"
    );
    let mut errors = Vec::new();
    let body = &tx.body;

    // Rule 1: Must have at least one input
    if body.inputs.is_empty() {
        errors.push(ValidationError::NoInputs);
    }

    // Rule 2: All inputs must exist in UTxO set
    let mut input_value = Lovelace(0);
    for input in &body.inputs {
        match utxo_set.lookup(input) {
            Some(output) => {
                input_value = Lovelace(input_value.0 + output.value.coin.0);
            }
            None => {
                errors.push(ValidationError::InputNotFound(input.to_string()));
            }
        }
    }

    // Rule 3: Value conservation (inputs = outputs + fee + deposits - withdrawals)
    if errors.is_empty() {
        let output_value: u64 = body.outputs.iter().map(|o| o.value.coin.0).sum();
        let withdrawal_value: u64 = body.withdrawals.values().map(|l| l.0).sum::<u64>();
        let total_out = output_value + body.fee.0;

        // Simplified: not accounting for deposits/refunds/minting here
        if input_value.0 + withdrawal_value != total_out {
            errors.push(ValidationError::ValueNotConserved {
                inputs: input_value.0 + withdrawal_value,
                outputs: output_value,
                fee: body.fee.0,
            });
        }
    }

    // Rule 4: Fee must be >= minimum
    let min_fee = params.min_fee(tx_size);
    if body.fee.0 < min_fee.0 {
        errors.push(ValidationError::FeeTooSmall {
            minimum: min_fee.0,
            actual: body.fee.0,
        });
    }

    // Rule 5: All outputs must meet minimum UTxO value
    let min_utxo = params.min_utxo_value();
    for output in &body.outputs {
        if output.value.coin.0 < min_utxo.0 {
            errors.push(ValidationError::OutputTooSmall {
                minimum: min_utxo.0,
                actual: output.value.coin.0,
            });
        }
    }

    // Rule 6: Transaction size limit
    if tx_size > params.max_tx_size {
        errors.push(ValidationError::TxTooLarge {
            maximum: params.max_tx_size,
            actual: tx_size,
        });
    }

    // Rule 7: TTL check
    if let Some(ttl) = body.ttl {
        if current_slot > ttl.0 {
            errors.push(ValidationError::TtlExpired {
                current_slot,
                ttl: ttl.0,
            });
        }
    }

    // Rule 8: Validity interval start
    if let Some(start) = body.validity_interval_start {
        if current_slot < start.0 {
            errors.push(ValidationError::NotYetValid {
                current_slot,
                valid_from: start.0,
            });
        }
    }

    // Rule 9: Collateral check for Plutus transactions
    if has_plutus_scripts(tx) {
        if body.collateral.is_empty() {
            errors.push(ValidationError::InsufficientCollateral);
        } else {
            let mut collateral_value = 0u64;
            for col_input in &body.collateral {
                if let Some(output) = utxo_set.lookup(col_input) {
                    collateral_value += output.value.coin.0;
                }
            }
            let required_collateral = body.fee.0 * params.collateral_percentage / 100;
            if collateral_value < required_collateral {
                errors.push(ValidationError::InsufficientCollateral);
            }
        }
    }

    if errors.is_empty() {
        debug!(tx_hash = %tx.hash.to_hex(), "Validation: transaction valid");
        Ok(())
    } else {
        warn!(
            tx_hash = %tx.hash.to_hex(),
            error_count = errors.len(),
            errors = ?errors,
            "Validation: transaction rejected"
        );
        Err(errors)
    }
}

fn has_plutus_scripts(tx: &Transaction) -> bool {
    !tx.witness_set.plutus_v1_scripts.is_empty()
        || !tx.witness_set.plutus_v2_scripts.is_empty()
        || !tx.witness_set.plutus_v3_scripts.is_empty()
        || !tx.witness_set.redeemers.is_empty()
}

/// Evaluate a native script given the set of key hashes that signed
/// the transaction and the current slot validity interval.
pub fn evaluate_native_script(
    script: &NativeScript,
    signers: &HashSet<Hash32>,
    current_slot: SlotNo,
) -> bool {
    match script {
        NativeScript::ScriptPubkey(keyhash) => signers.contains(keyhash),
        NativeScript::ScriptAll(scripts) => scripts
            .iter()
            .all(|s| evaluate_native_script(s, signers, current_slot)),
        NativeScript::ScriptAny(scripts) => scripts
            .iter()
            .any(|s| evaluate_native_script(s, signers, current_slot)),
        NativeScript::ScriptNOfK(n, scripts) => {
            let count = scripts
                .iter()
                .filter(|s| evaluate_native_script(s, signers, current_slot))
                .count();
            count >= *n as usize
        }
        NativeScript::InvalidBefore(slot) => current_slot >= *slot,
        NativeScript::InvalidHereafter(slot) => current_slot < *slot,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use torsten_primitives::address::{Address, ByronAddress};
    use torsten_primitives::hash::Hash32;
    use torsten_primitives::transaction::*;
    use torsten_primitives::value::Value;

    fn make_simple_utxo_set() -> (UtxoSet, TransactionInput) {
        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        let output = TransactionOutput {
            address: Address::Byron(ByronAddress {
                payload: vec![0u8; 32],
            }),
            value: Value::lovelace(10_000_000),
            datum: OutputDatum::None,
            script_ref: None,
        };
        utxo_set.insert(input.clone(), output);
        (utxo_set, input)
    }

    fn make_simple_tx(input: TransactionInput, output_value: u64, fee: u64) -> Transaction {
        Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(output_value),
                    datum: OutputDatum::None,
                    script_ref: None,
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
        }
    }

    #[test]
    fn test_valid_transaction() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        // fee (200000) + output (9800000) = 10000000 = input value
        let tx = make_simple_tx(input, 9_800_000, 200_000);

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300);
        assert!(result.is_ok());
    }

    #[test]
    fn test_no_inputs() {
        let (utxo_set, _) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(
            TransactionInput {
                transaction_id: Hash32::ZERO,
                index: 0,
            },
            0,
            0,
        );
        tx.body.inputs.clear();

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::NoInputs)));
    }

    #[test]
    fn test_input_not_found() {
        let (utxo_set, _) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let missing_input = TransactionInput {
            transaction_id: Hash32::from_bytes([99u8; 32]),
            index: 0,
        };
        let tx = make_simple_tx(missing_input, 9_800_000, 200_000);

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300);
        assert!(result.is_err());
    }

    #[test]
    fn test_value_not_conserved() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        // output + fee > input value
        let tx = make_simple_tx(input, 10_000_000, 200_000);

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300);
        assert!(result.is_err());
    }

    #[test]
    fn test_fee_too_small() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        // Fee of 100 is way below minimum
        let tx = make_simple_tx(input, 9_999_900, 100);

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300);
        assert!(result.is_err());
    }

    #[test]
    fn test_output_too_small() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        // Output of 1000 lovelace is below minimum UTxO
        let tx = make_simple_tx(input, 1000, 9_999_000);

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300);
        assert!(result.is_err());
    }

    #[test]
    fn test_ttl_expired() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.body.ttl = Some(SlotNo(50)); // TTL in the past

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300);
        assert!(result.is_err());
    }

    #[test]
    fn test_not_yet_valid() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.body.validity_interval_start = Some(SlotNo(200)); // Not valid yet

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300);
        assert!(result.is_err());
    }

    #[test]
    fn test_tx_too_large() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let tx = make_simple_tx(input, 9_800_000, 200_000);

        // Pass a tx_size larger than max
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 20000);
        assert!(result.is_err());
    }

    // Native script evaluation tests

    #[test]
    fn test_native_script_pubkey_match() {
        let key = Hash32::from_bytes([1u8; 32]);
        let script = NativeScript::ScriptPubkey(key);
        let signers: HashSet<Hash32> = [key].into();
        assert!(evaluate_native_script(&script, &signers, SlotNo(100)));
    }

    #[test]
    fn test_native_script_pubkey_no_match() {
        let key = Hash32::from_bytes([1u8; 32]);
        let other_key = Hash32::from_bytes([2u8; 32]);
        let script = NativeScript::ScriptPubkey(key);
        let signers: HashSet<Hash32> = [other_key].into();
        assert!(!evaluate_native_script(&script, &signers, SlotNo(100)));
    }

    #[test]
    fn test_native_script_all() {
        let k1 = Hash32::from_bytes([1u8; 32]);
        let k2 = Hash32::from_bytes([2u8; 32]);
        let script = NativeScript::ScriptAll(vec![
            NativeScript::ScriptPubkey(k1),
            NativeScript::ScriptPubkey(k2),
        ]);
        let signers: HashSet<Hash32> = [k1, k2].into();
        assert!(evaluate_native_script(&script, &signers, SlotNo(100)));

        // Missing one signer
        let partial: HashSet<Hash32> = [k1].into();
        assert!(!evaluate_native_script(&script, &partial, SlotNo(100)));
    }

    #[test]
    fn test_native_script_any() {
        let k1 = Hash32::from_bytes([1u8; 32]);
        let k2 = Hash32::from_bytes([2u8; 32]);
        let script = NativeScript::ScriptAny(vec![
            NativeScript::ScriptPubkey(k1),
            NativeScript::ScriptPubkey(k2),
        ]);
        let signers: HashSet<Hash32> = [k2].into();
        assert!(evaluate_native_script(&script, &signers, SlotNo(100)));

        // No matching signers
        let empty: HashSet<Hash32> = HashSet::new();
        assert!(!evaluate_native_script(&script, &empty, SlotNo(100)));
    }

    #[test]
    fn test_native_script_n_of_k() {
        let k1 = Hash32::from_bytes([1u8; 32]);
        let k2 = Hash32::from_bytes([2u8; 32]);
        let k3 = Hash32::from_bytes([3u8; 32]);
        let script = NativeScript::ScriptNOfK(
            2,
            vec![
                NativeScript::ScriptPubkey(k1),
                NativeScript::ScriptPubkey(k2),
                NativeScript::ScriptPubkey(k3),
            ],
        );

        // 2 of 3 present
        let signers: HashSet<Hash32> = [k1, k3].into();
        assert!(evaluate_native_script(&script, &signers, SlotNo(100)));

        // Only 1 of 3 present
        let one: HashSet<Hash32> = [k1].into();
        assert!(!evaluate_native_script(&script, &one, SlotNo(100)));
    }

    #[test]
    fn test_native_script_invalid_before() {
        let script = NativeScript::InvalidBefore(SlotNo(50));
        let signers: HashSet<Hash32> = HashSet::new();

        assert!(evaluate_native_script(&script, &signers, SlotNo(50)));
        assert!(evaluate_native_script(&script, &signers, SlotNo(100)));
        assert!(!evaluate_native_script(&script, &signers, SlotNo(49)));
    }

    #[test]
    fn test_native_script_invalid_hereafter() {
        let script = NativeScript::InvalidHereafter(SlotNo(100));
        let signers: HashSet<Hash32> = HashSet::new();

        assert!(evaluate_native_script(&script, &signers, SlotNo(99)));
        assert!(!evaluate_native_script(&script, &signers, SlotNo(100)));
        assert!(!evaluate_native_script(&script, &signers, SlotNo(101)));
    }

    #[test]
    fn test_native_script_nested_timelock_multisig() {
        let k1 = Hash32::from_bytes([1u8; 32]);
        // Require k1 signature AND slot in [50, 200)
        let script = NativeScript::ScriptAll(vec![
            NativeScript::ScriptPubkey(k1),
            NativeScript::InvalidBefore(SlotNo(50)),
            NativeScript::InvalidHereafter(SlotNo(200)),
        ]);

        let signers: HashSet<Hash32> = [k1].into();
        assert!(evaluate_native_script(&script, &signers, SlotNo(100)));
        assert!(!evaluate_native_script(&script, &signers, SlotNo(49)));
        assert!(!evaluate_native_script(&script, &signers, SlotNo(200)));

        // Missing signer
        let empty: HashSet<Hash32> = HashSet::new();
        assert!(!evaluate_native_script(&script, &empty, SlotNo(100)));
    }
}
