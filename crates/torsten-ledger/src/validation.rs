use crate::plutus::{evaluate_plutus_scripts, SlotConfig};
use crate::utxo::UtxoSet;
use std::collections::{BTreeMap, HashSet};
use torsten_primitives::hash::{Hash32, PolicyId};
use torsten_primitives::protocol_params::ProtocolParameters;
use torsten_primitives::time::SlotNo;
use torsten_primitives::transaction::{Certificate, NativeScript, Transaction};
use torsten_primitives::value::{AssetName, Lovelace};
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
    #[error("Too many collateral inputs: max={max}, actual={actual}")]
    TooManyCollateralInputs { max: u64, actual: u64 },
    #[error("Collateral input not found in UTxO set: {0}")]
    CollateralNotFound(String),
    #[error("Collateral input contains tokens (must be pure ADA): {0}")]
    CollateralHasTokens(String),
    #[error("Reference input not found in UTxO set: {0}")]
    ReferenceInputNotFound(String),
    #[error("Reference input overlaps with regular input: {0}")]
    ReferenceInputOverlapsInput(String),
    #[error("Multi-asset not conserved for policy {policy}: inputs+mint={input_side}, outputs={output_side}")]
    MultiAssetNotConserved {
        policy: String,
        input_side: i128,
        output_side: i128,
    },
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
    slot_config: Option<&SlotConfig>,
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

    // Rule 3: Value conservation
    // consumed = sum(inputs) + withdrawals + deposit_refunds
    // produced = sum(outputs) + fee + deposits
    // consumed must equal produced
    if errors.is_empty() {
        let output_value: u64 = body.outputs.iter().map(|o| o.value.coin.0).sum();
        let withdrawal_value: u64 = body.withdrawals.values().map(|l| l.0).sum::<u64>();

        // Calculate deposits and refunds from certificates
        let (total_deposits, total_refunds) =
            calculate_deposits_and_refunds(&body.certificates, params);

        // Proposal deposits (Conway governance)
        let proposal_deposits = body.proposal_procedures.len() as u64 * params.gov_action_deposit.0;

        let consumed = input_value.0 + withdrawal_value + total_refunds;
        let produced = output_value + body.fee.0 + total_deposits + proposal_deposits;

        if consumed != produced {
            errors.push(ValidationError::ValueNotConserved {
                inputs: consumed,
                outputs: output_value,
                fee: body.fee.0,
            });
        }
    }

    // Rule 3b: Multi-asset conservation
    // For each (policy, asset): sum(input_tokens) + mint = sum(output_tokens)
    if errors.is_empty() && (!body.mint.is_empty() || has_multi_assets_in_tx(tx, utxo_set)) {
        let mut asset_balance: BTreeMap<(PolicyId, AssetName), i128> = BTreeMap::new();

        // Add input tokens (positive)
        for input in &body.inputs {
            if let Some(output) = utxo_set.lookup(input) {
                for (policy, assets) in &output.value.multi_asset {
                    for (name, qty) in assets {
                        *asset_balance.entry((*policy, name.clone())).or_insert(0) += *qty as i128;
                    }
                }
            }
        }

        // Add minted tokens (can be positive or negative)
        for (policy, assets) in &body.mint {
            for (name, qty) in assets {
                *asset_balance.entry((*policy, name.clone())).or_insert(0) += *qty as i128;
            }
        }

        // Subtract output tokens
        for output in &body.outputs {
            for (policy, assets) in &output.value.multi_asset {
                for (name, qty) in assets {
                    *asset_balance.entry((*policy, name.clone())).or_insert(0) -= *qty as i128;
                }
            }
        }

        // Every asset must balance to zero
        for ((policy, _asset), balance) in &asset_balance {
            if *balance != 0 {
                errors.push(ValidationError::MultiAssetNotConserved {
                    policy: policy.to_hex(),
                    input_side: if *balance > 0 { *balance } else { 0 },
                    output_side: if *balance < 0 {
                        balance.unsigned_abs() as i128
                    } else {
                        0
                    },
                });
                break; // One error is enough
            }
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

    // Rule 9: Reference inputs must exist in UTxO and not overlap with regular inputs
    if !body.reference_inputs.is_empty() {
        let input_set: HashSet<_> = body.inputs.iter().collect();
        for ref_input in &body.reference_inputs {
            if utxo_set.lookup(ref_input).is_none() {
                errors.push(ValidationError::ReferenceInputNotFound(
                    ref_input.to_string(),
                ));
            }
            if input_set.contains(ref_input) {
                errors.push(ValidationError::ReferenceInputOverlapsInput(
                    ref_input.to_string(),
                ));
            }
        }
    }

    // Rule 10: Required signers must have corresponding vkey witnesses
    // Required signers are key hashes (blake2b-224 of the verification key)
    if !body.required_signers.is_empty() && !tx.witness_set.vkey_witnesses.is_empty() {
        let witness_keyhashes: HashSet<_> = tx
            .witness_set
            .vkey_witnesses
            .iter()
            .map(|w| torsten_primitives::hash::blake2b_224(&w.vkey))
            .collect();
        for required_signer in &body.required_signers {
            // Compare first 28 bytes (Hash32 may be zero-padded Hash28)
            let signer_28 = &required_signer.as_bytes()[..28];
            let has_witness = witness_keyhashes
                .iter()
                .any(|kh| kh.as_bytes() == signer_28);
            if !has_witness {
                errors.push(ValidationError::MissingRequiredSigner(
                    required_signer.to_hex(),
                ));
            }
        }
    } else if !body.required_signers.is_empty() {
        // Required signers but no vkey witnesses at all
        for required_signer in &body.required_signers {
            errors.push(ValidationError::MissingRequiredSigner(
                required_signer.to_hex(),
            ));
        }
    }

    // Rule 11: Collateral check for Plutus transactions
    if has_plutus_scripts(tx) {
        if body.collateral.is_empty() {
            errors.push(ValidationError::InsufficientCollateral);
        } else {
            // Check max collateral inputs
            if body.collateral.len() as u64 > params.max_collateral_inputs {
                errors.push(ValidationError::TooManyCollateralInputs {
                    max: params.max_collateral_inputs,
                    actual: body.collateral.len() as u64,
                });
            }

            let mut collateral_value = 0u64;
            for col_input in &body.collateral {
                match utxo_set.lookup(col_input) {
                    Some(output) => {
                        // Collateral inputs must be pure ADA (no multi-assets)
                        if !output.value.multi_asset.is_empty() {
                            errors
                                .push(ValidationError::CollateralHasTokens(col_input.to_string()));
                        }
                        collateral_value += output.value.coin.0;
                    }
                    None => {
                        errors.push(ValidationError::CollateralNotFound(col_input.to_string()));
                    }
                }
            }
            let required_collateral = body.fee.0 * params.collateral_percentage / 100;
            if collateral_value < required_collateral {
                errors.push(ValidationError::InsufficientCollateral);
            }
        }

        // Check total execution units don't exceed per-tx limits
        let total_mem: u64 = tx
            .witness_set
            .redeemers
            .iter()
            .map(|r| r.ex_units.mem)
            .sum();
        let total_steps: u64 = tx
            .witness_set
            .redeemers
            .iter()
            .map(|r| r.ex_units.steps)
            .sum();
        if total_mem > params.max_tx_ex_units.mem || total_steps > params.max_tx_ex_units.steps {
            errors.push(ValidationError::ExUnitsExceeded);
        }

        // Phase-2: Execute Plutus scripts if we have raw CBOR and a slot config
        if errors.is_empty() && tx.raw_cbor.is_some() {
            if let Some(sc) = slot_config {
                let cost_models_cbor = params.cost_models.to_cbor();
                let max_ex = (params.max_tx_ex_units.mem, params.max_tx_ex_units.steps);
                if let Err(e) =
                    evaluate_plutus_scripts(tx, utxo_set, cost_models_cbor.as_deref(), max_ex, sc)
                {
                    errors.push(ValidationError::ScriptFailed(e.to_string()));
                }
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

/// Calculate total deposits and refunds from certificates in a transaction.
///
/// Deposits are required for: stake registration, pool registration, DRep registration,
/// stake+delegation registration.
/// Refunds are returned for: stake deregistration, DRep unregistration.
fn calculate_deposits_and_refunds(
    certificates: &[Certificate],
    params: &ProtocolParameters,
) -> (u64, u64) {
    let mut deposits = 0u64;
    let mut refunds = 0u64;

    for cert in certificates {
        match cert {
            Certificate::StakeRegistration(_) => {
                deposits += params.key_deposit.0;
            }
            Certificate::StakeDeregistration(_) => {
                refunds += params.key_deposit.0;
            }
            Certificate::PoolRegistration(_) => {
                deposits += params.pool_deposit.0;
            }
            Certificate::RegDRep { deposit, .. } => {
                deposits += deposit.0;
            }
            Certificate::UnregDRep { refund, .. } => {
                refunds += refund.0;
            }
            Certificate::RegStakeDeleg { deposit, .. } => {
                deposits += deposit.0;
            }
            Certificate::RegStakeVoteDeleg { deposit, .. } => {
                deposits += deposit.0;
            }
            Certificate::VoteRegDeleg { deposit, .. } => {
                deposits += deposit.0;
            }
            _ => {}
        }
    }

    (deposits, refunds)
}

/// Check if the transaction involves any multi-asset tokens (in inputs or outputs).
fn has_multi_assets_in_tx(tx: &Transaction, utxo_set: &UtxoSet) -> bool {
    // Check inputs
    for input in &tx.body.inputs {
        if let Some(output) = utxo_set.lookup(input) {
            if !output.value.multi_asset.is_empty() {
                return true;
            }
        }
    }
    // Check outputs
    for output in &tx.body.outputs {
        if !output.value.multi_asset.is_empty() {
            return true;
        }
    }
    false
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
            raw_cbor: None,
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

    #[test]
    fn test_valid_transaction() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        // fee (200000) + output (9800000) = 10000000 = input value
        let tx = make_simple_tx(input, 9_800_000, 200_000);

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
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

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
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

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_value_not_conserved() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        // output + fee > input value
        let tx = make_simple_tx(input, 10_000_000, 200_000);

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_fee_too_small() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        // Fee of 100 is way below minimum
        let tx = make_simple_tx(input, 9_999_900, 100);

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_output_too_small() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        // Output of 1000 lovelace is below minimum UTxO
        let tx = make_simple_tx(input, 1000, 9_999_000);

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_ttl_expired() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.body.ttl = Some(SlotNo(50)); // TTL in the past

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_not_yet_valid() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.body.validity_interval_start = Some(SlotNo(200)); // Not valid yet

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_tx_too_large() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let tx = make_simple_tx(input, 9_800_000, 200_000);

        // Pass a tx_size larger than max
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 20000, None);
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

    #[test]
    fn test_stake_registration_deposit() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let key_deposit = params.key_deposit.0; // 2_000_000

        // With stake registration: inputs = outputs + fee + deposit
        let mut tx = make_simple_tx(input, 10_000_000 - 200_000 - key_deposit, 200_000);
        tx.body.certificates.push(Certificate::StakeRegistration(
            torsten_primitives::credentials::Credential::VerificationKey(
                torsten_primitives::hash::Hash28::from_bytes([5u8; 28]),
            ),
        ));

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_stake_deregistration_refund() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let key_deposit = params.key_deposit.0; // 2_000_000

        // With stake deregistration: inputs + refund = outputs + fee
        let mut tx = make_simple_tx(input, 10_000_000 - 200_000 + key_deposit, 200_000);
        tx.body.certificates.push(Certificate::StakeDeregistration(
            torsten_primitives::credentials::Credential::VerificationKey(
                torsten_primitives::hash::Hash28::from_bytes([5u8; 28]),
            ),
        ));

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_deposit_not_accounted_fails() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();

        // Stake registration without accounting for deposit should fail
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.body.certificates.push(Certificate::StakeRegistration(
            torsten_primitives::credentials::Credential::VerificationKey(
                torsten_primitives::hash::Hash28::from_bytes([5u8; 28]),
            ),
        ));

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_deposits_and_refunds_calculation() {
        let params = ProtocolParameters::mainnet_defaults();
        let cred = torsten_primitives::credentials::Credential::VerificationKey(
            torsten_primitives::hash::Hash28::from_bytes([5u8; 28]),
        );

        // Two registrations, one deregistration
        let certs = vec![
            Certificate::StakeRegistration(cred.clone()),
            Certificate::StakeRegistration(cred.clone()),
            Certificate::StakeDeregistration(cred),
        ];

        let (deposits, refunds) = calculate_deposits_and_refunds(&certs, &params);
        assert_eq!(deposits, params.key_deposit.0 * 2);
        assert_eq!(refunds, params.key_deposit.0);
    }

    #[test]
    fn test_multi_asset_conservation_valid() {
        // Input has 10 ADA + 100 tokens, output has 9.8 ADA + 100 tokens, fee = 0.2 ADA
        let policy = torsten_primitives::hash::Hash28::from_bytes([10u8; 28]);
        let asset = AssetName::new(b"Token".to_vec()).unwrap();

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        let mut input_value = Value::lovelace(10_000_000);
        input_value
            .multi_asset
            .entry(policy)
            .or_default()
            .insert(asset.clone(), 100);
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: input_value,
                datum: OutputDatum::None,
                script_ref: None,
                raw_cbor: None,
            },
        );

        let mut output_value = Value::lovelace(9_800_000);
        output_value
            .multi_asset
            .entry(policy)
            .or_default()
            .insert(asset, 100);

        let params = ProtocolParameters::mainnet_defaults();
        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: output_value,
                    datum: OutputDatum::None,
                    script_ref: None,
                    raw_cbor: None,
                }],
                fee: Lovelace(200_000),
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
            raw_cbor: None,
        };

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_multi_asset_not_conserved() {
        // Input has 100 tokens but output has 200 — tokens created from thin air
        let policy = torsten_primitives::hash::Hash28::from_bytes([10u8; 28]);
        let asset = AssetName::new(b"Token".to_vec()).unwrap();

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        let mut input_value = Value::lovelace(10_000_000);
        input_value
            .multi_asset
            .entry(policy)
            .or_default()
            .insert(asset.clone(), 100);
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: input_value,
                datum: OutputDatum::None,
                script_ref: None,
                raw_cbor: None,
            },
        );

        let mut output_value = Value::lovelace(9_800_000);
        output_value
            .multi_asset
            .entry(policy)
            .or_default()
            .insert(asset, 200); // More tokens than input!

        let params = ProtocolParameters::mainnet_defaults();
        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: output_value,
                    datum: OutputDatum::None,
                    script_ref: None,
                    raw_cbor: None,
                }],
                fee: Lovelace(200_000),
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
            raw_cbor: None,
        };

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::MultiAssetNotConserved { .. })));
    }

    #[test]
    fn test_multi_asset_with_minting() {
        // Input has 10 ADA, mint 50 tokens, output has 9.8 ADA + 50 tokens
        let policy = torsten_primitives::hash::Hash28::from_bytes([10u8; 28]);
        let asset = AssetName::new(b"Token".to_vec()).unwrap();

        let (utxo_set, input) = make_simple_utxo_set();

        let mut output_value = Value::lovelace(9_800_000);
        output_value
            .multi_asset
            .entry(policy)
            .or_default()
            .insert(asset.clone(), 50);

        let mut mint: BTreeMap<PolicyId, BTreeMap<AssetName, i64>> = BTreeMap::new();
        mint.entry(policy).or_default().insert(asset, 50);

        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.body.outputs[0].value = output_value;
        tx.body.mint = mint;

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_multi_asset_burning() {
        // Input has 100 tokens, burn 30, output has 70
        let policy = torsten_primitives::hash::Hash28::from_bytes([10u8; 28]);
        let asset = AssetName::new(b"Token".to_vec()).unwrap();

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        let mut input_value = Value::lovelace(10_000_000);
        input_value
            .multi_asset
            .entry(policy)
            .or_default()
            .insert(asset.clone(), 100);
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: input_value,
                datum: OutputDatum::None,
                script_ref: None,
                raw_cbor: None,
            },
        );

        let mut output_value = Value::lovelace(9_800_000);
        output_value
            .multi_asset
            .entry(policy)
            .or_default()
            .insert(asset.clone(), 70);

        let mut mint: BTreeMap<PolicyId, BTreeMap<AssetName, i64>> = BTreeMap::new();
        mint.entry(policy).or_default().insert(asset, -30); // burn 30

        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.body.outputs[0].value = output_value;
        tx.body.mint = mint;

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_ok());
    }

    fn make_plutus_tx_with_collateral(
        input: TransactionInput,
        output_value: u64,
        fee: u64,
        collateral: Vec<TransactionInput>,
    ) -> Transaction {
        let mut tx = make_simple_tx(input, output_value, fee);
        tx.body.collateral = collateral;
        // Add a dummy redeemer to make it a Plutus tx
        tx.witness_set.redeemers.push(Redeemer {
            tag: RedeemerTag::Spend,
            index: 0,
            data: PlutusData::Integer(0),
            ex_units: ExUnits {
                mem: 1_000_000,
                steps: 1_000_000_000,
            },
        });
        tx.witness_set.plutus_v2_scripts.push(vec![0x01]); // dummy script
        tx
    }

    #[test]
    fn test_plutus_collateral_valid() {
        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                raw_cbor: None,
            },
        );
        let col_input = TransactionInput {
            transaction_id: Hash32::from_bytes([2u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            col_input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                raw_cbor: None,
            },
        );

        let params = ProtocolParameters::mainnet_defaults();
        let tx = make_plutus_tx_with_collateral(input, 9_800_000, 200_000, vec![col_input]);
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_plutus_collateral_not_found() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();

        let missing_col = TransactionInput {
            transaction_id: Hash32::from_bytes([99u8; 32]),
            index: 0,
        };
        let tx = make_plutus_tx_with_collateral(input, 9_800_000, 200_000, vec![missing_col]);
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::CollateralNotFound(_))));
    }

    #[test]
    fn test_plutus_collateral_has_tokens() {
        let policy = torsten_primitives::hash::Hash28::from_bytes([10u8; 28]);
        let asset = AssetName::new(b"Token".to_vec()).unwrap();

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                raw_cbor: None,
            },
        );

        // Collateral with tokens
        let col_input = TransactionInput {
            transaction_id: Hash32::from_bytes([2u8; 32]),
            index: 0,
        };
        let mut col_value = Value::lovelace(5_000_000);
        col_value
            .multi_asset
            .entry(policy)
            .or_default()
            .insert(asset, 100);
        utxo_set.insert(
            col_input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: col_value,
                datum: OutputDatum::None,
                script_ref: None,
                raw_cbor: None,
            },
        );

        let params = ProtocolParameters::mainnet_defaults();
        let tx = make_plutus_tx_with_collateral(input, 9_800_000, 200_000, vec![col_input]);
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::CollateralHasTokens(_))));
    }

    #[test]
    fn test_plutus_too_many_collateral_inputs() {
        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                raw_cbor: None,
            },
        );

        // Create 4 collateral inputs (max is 3)
        let mut collateral = Vec::new();
        for i in 2..=5u8 {
            let col = TransactionInput {
                transaction_id: Hash32::from_bytes([i; 32]),
                index: 0,
            };
            utxo_set.insert(
                col.clone(),
                TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(1_000_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    raw_cbor: None,
                },
            );
            collateral.push(col);
        }

        let params = ProtocolParameters::mainnet_defaults();
        let tx = make_plutus_tx_with_collateral(input, 9_800_000, 200_000, collateral);
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::TooManyCollateralInputs { .. })));
    }

    #[test]
    fn test_plutus_ex_units_exceeded() {
        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                raw_cbor: None,
            },
        );
        let col_input = TransactionInput {
            transaction_id: Hash32::from_bytes([2u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            col_input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                raw_cbor: None,
            },
        );

        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_plutus_tx_with_collateral(input, 9_800_000, 200_000, vec![col_input]);
        // Set excessive execution units
        tx.witness_set.redeemers[0].ex_units = ExUnits {
            mem: u64::MAX,
            steps: u64::MAX,
        };

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::ExUnitsExceeded)));
    }

    #[test]
    fn test_reference_input_valid() {
        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                raw_cbor: None,
            },
        );
        let ref_input = TransactionInput {
            transaction_id: Hash32::from_bytes([2u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            ref_input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                raw_cbor: None,
            },
        );

        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.body.reference_inputs = vec![ref_input];
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_reference_input_not_found() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();

        let missing_ref = TransactionInput {
            transaction_id: Hash32::from_bytes([99u8; 32]),
            index: 0,
        };
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.body.reference_inputs = vec![missing_ref];

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::ReferenceInputNotFound(_))));
    }

    #[test]
    fn test_reference_input_overlaps_input() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();

        // Use the same input as both regular and reference
        let mut tx = make_simple_tx(input.clone(), 9_800_000, 200_000);
        tx.body.reference_inputs = vec![input];

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::ReferenceInputOverlapsInput(_))));
    }

    #[test]
    fn test_required_signer_missing() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();

        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        // Require a signer that doesn't exist in witnesses
        tx.body.required_signers = vec![Hash32::from_bytes([0xAA; 32])];

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::MissingRequiredSigner(_))));
    }
}
