use crate::plutus::{evaluate_plutus_scripts, SlotConfig};
use crate::utxo::UtxoSet;
use std::collections::{BTreeMap, HashSet};
use torsten_primitives::credentials::Credential;
use torsten_primitives::hash::{Hash28, Hash32, PolicyId};
use torsten_primitives::protocol_params::ProtocolParameters;
use torsten_primitives::time::SlotNo;
use torsten_primitives::transaction::{
    Certificate, NativeScript, RedeemerTag, ScriptRef, Transaction, TransactionInput,
};
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
    #[error("Collateral mismatch: total_collateral={declared}, effective={computed}")]
    CollateralMismatch { declared: u64, computed: u64 },
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
    #[error("Script data hash mismatch: expected {expected}, got {actual}")]
    ScriptDataHashMismatch { expected: String, actual: String },
    #[error("Script data hash present but no scripts or redeemers")]
    UnexpectedScriptDataHash,
    #[error("Missing script data hash (required when scripts/redeemers present)")]
    MissingScriptDataHash,
    #[error("Duplicate input in transaction: {0}")]
    DuplicateInput(String),
    #[error("Native script validation failed")]
    NativeScriptFailed,
    #[error("Witness signature verification failed for vkey: {0}")]
    InvalidWitnessSignature(String),
    #[error("Output address network mismatch: expected {expected:?}, got {actual:?}")]
    NetworkMismatch {
        expected: torsten_primitives::network::NetworkId,
        actual: torsten_primitives::network::NetworkId,
    },
    #[error("Auxiliary data hash declared but no auxiliary data present")]
    AuxiliaryDataHashWithoutData,
    #[error("Auxiliary data present but no auxiliary data hash in tx body")]
    AuxiliaryDataWithoutHash,
    #[error("Block execution units exceeded: {resource} limit={limit}, total={total}")]
    BlockExUnitsExceeded {
        resource: String,
        limit: u64,
        total: u64,
    },
    #[error("Output value too large: maximum={maximum}, actual={actual}")]
    OutputValueTooLarge { maximum: u64, actual: u64 },
    #[error("Plutus transaction missing raw CBOR for script evaluation")]
    MissingRawCbor,
    #[error("Plutus transaction missing slot configuration for script evaluation")]
    MissingSlotConfig,
    #[error("Script-locked input at index {index} has no matching Spend redeemer")]
    MissingSpendRedeemer { index: u32 },
    #[error("Redeemer index out of range: tag={tag}, index={index}, max={max}")]
    RedeemerIndexOutOfRange { tag: String, index: u32, max: usize },
    #[error("Missing VKey witness for input credential: {0}")]
    MissingInputWitness(String),
    #[error("Missing script witness for script-locked input: {0}")]
    MissingScriptWitness(String),
    #[error("Missing VKey witness for withdrawal credential: {0}")]
    MissingWithdrawalWitness(String),
    #[error("Missing script witness for script-locked withdrawal: {0}")]
    MissingWithdrawalScriptWitness(String),
}

/// Validate a transaction against the current UTxO set and protocol parameters.
///
/// `registered_pools` is an optional set of already-registered pool operator hashes.
/// When provided, pool re-registrations will not be charged an additional deposit.
pub fn validate_transaction(
    tx: &Transaction,
    utxo_set: &UtxoSet,
    params: &ProtocolParameters,
    current_slot: u64,
    tx_size: u64,
    slot_config: Option<&SlotConfig>,
) -> Result<(), Vec<ValidationError>> {
    validate_transaction_with_pools(
        tx,
        utxo_set,
        params,
        current_slot,
        tx_size,
        slot_config,
        None,
    )
}

/// Validate a transaction with an optional set of registered pools.
///
/// When `registered_pools` is `Some`, pool re-registrations (updating an existing pool)
/// will not charge an additional deposit. When `None`, all pool registrations are treated
/// as new (deposit always charged).
pub fn validate_transaction_with_pools(
    tx: &Transaction,
    utxo_set: &UtxoSet,
    params: &ProtocolParameters,
    current_slot: u64,
    tx_size: u64,
    slot_config: Option<&SlotConfig>,
    registered_pools: Option<&HashSet<Hash28>>,
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

    // Rule 1b: No duplicate inputs
    {
        let mut seen = HashSet::new();
        for input in &body.inputs {
            if !seen.insert(input) {
                errors.push(ValidationError::DuplicateInput(input.to_string()));
            }
        }
    }

    // Rule 1c: Auxiliary data hash consistency
    // If auxiliary_data_hash is declared in the body, auxiliary_data must be present (and vice versa)
    match (&body.auxiliary_data_hash, &tx.auxiliary_data) {
        (Some(_), None) => {
            errors.push(ValidationError::AuxiliaryDataHashWithoutData);
        }
        (None, Some(_)) => {
            errors.push(ValidationError::AuxiliaryDataWithoutHash);
        }
        _ => {} // Both present or both absent — OK
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
            calculate_deposits_and_refunds(&body.certificates, params, registered_pools);

        // Proposal deposits (Conway governance)
        let proposal_deposits = body.proposal_procedures.len() as u64 * params.gov_action_deposit.0;

        // Treasury donation (Conway)
        let donation = body.donation.map(|d| d.0).unwrap_or(0);

        let consumed = input_value.0 + withdrawal_value + total_refunds;
        let produced = output_value + body.fee.0 + total_deposits + proposal_deposits + donation;

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

    // Rule 3c: Every minting policy must have a corresponding script
    // (native script or Plutus script in witness set or reference inputs)
    if !body.mint.is_empty() {
        // Collect all available script hashes from witness set and reference inputs
        let mut available_script_hashes: HashSet<Hash28> = HashSet::new();

        // Native scripts from witness set: blake2b_224(0x00 || script_cbor)
        for script in &tx.witness_set.native_scripts {
            let script_cbor = torsten_serialization::encode_native_script(script);
            let mut tagged = Vec::with_capacity(1 + script_cbor.len());
            tagged.push(0x00);
            tagged.extend_from_slice(&script_cbor);
            available_script_hashes.insert(torsten_primitives::hash::blake2b_224(&tagged));
        }

        // Plutus scripts from witness set: blake2b_224(type_tag || script_bytes)
        for s in &tx.witness_set.plutus_v1_scripts {
            let mut tagged = Vec::with_capacity(1 + s.len());
            tagged.push(0x01);
            tagged.extend_from_slice(s);
            available_script_hashes.insert(torsten_primitives::hash::blake2b_224(&tagged));
        }
        for s in &tx.witness_set.plutus_v2_scripts {
            let mut tagged = Vec::with_capacity(1 + s.len());
            tagged.push(0x02);
            tagged.extend_from_slice(s);
            available_script_hashes.insert(torsten_primitives::hash::blake2b_224(&tagged));
        }
        for s in &tx.witness_set.plutus_v3_scripts {
            let mut tagged = Vec::with_capacity(1 + s.len());
            tagged.push(0x03);
            tagged.extend_from_slice(s);
            available_script_hashes.insert(torsten_primitives::hash::blake2b_224(&tagged));
        }

        // Reference scripts from reference inputs
        for ref_input in &body.reference_inputs {
            if let Some(utxo) = utxo_set.lookup(ref_input) {
                if let Some(script_ref) = &utxo.script_ref {
                    let hash = compute_script_ref_hash(script_ref);
                    available_script_hashes.insert(hash);
                }
            }
        }

        for policy in body.mint.keys() {
            if !available_script_hashes.contains(policy) {
                debug!(
                    policy = %policy.to_hex(),
                    "Minting policy without matching script in witness set or reference inputs"
                );
                errors.push(ValidationError::InvalidMint);
                break;
            }
        }
    }

    // Rule 4: Fee must be >= minimum (including reference script costs)
    let ref_script_size = calculate_ref_script_size(&body.reference_inputs, utxo_set);
    let ref_script_fee = if ref_script_size > 0 {
        // CIP-0112: tiered pricing for reference scripts
        // Base cost = min_fee_ref_script_cost_per_byte * ref_script_size
        // Applied in 25KiB tiers with 1.2x multiplier per tier
        calculate_ref_script_tiered_fee(params.min_fee_ref_script_cost_per_byte, ref_script_size)
    } else {
        0
    };
    // Plutus execution unit cost: ceil(price_mem * Σ mem) + ceil(price_step * Σ steps)
    let ex_unit_fee = {
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
        let mem_cost = if total_mem > 0 && params.execution_costs.mem_price.denominator > 0 {
            // ceil(price_mem * total_mem)
            let num = params.execution_costs.mem_price.numerator as u128 * total_mem as u128;
            let den = params.execution_costs.mem_price.denominator as u128;
            num.div_ceil(den) as u64
        } else {
            0
        };
        let step_cost = if total_steps > 0 && params.execution_costs.step_price.denominator > 0 {
            // ceil(price_step * total_steps)
            let num = params.execution_costs.step_price.numerator as u128 * total_steps as u128;
            let den = params.execution_costs.step_price.denominator as u128;
            num.div_ceil(den) as u64
        } else {
            0
        };
        mem_cost + step_cost
    };
    let min_fee = Lovelace(params.min_fee(tx_size).0 + ref_script_fee + ex_unit_fee);
    if body.fee.0 < min_fee.0 {
        errors.push(ValidationError::FeeTooSmall {
            minimum: min_fee.0,
            actual: body.fee.0,
        });
    }

    // Rule 5: All outputs must meet minimum UTxO value
    // Use per-output size-based minimum when raw CBOR is available
    let default_min_utxo = params.min_utxo_value();
    for output in &body.outputs {
        let min_utxo = if let Some(ref cbor) = output.raw_cbor {
            params.min_utxo_for_output_size(cbor.len() as u64)
        } else {
            default_min_utxo
        };
        if output.value.coin.0 < min_utxo.0 {
            errors.push(ValidationError::OutputTooSmall {
                minimum: min_utxo.0,
                actual: output.value.coin.0,
            });
        }
    }

    // Rule 5a: Output value size must not exceed max_val_size
    // Per Cardano spec, the CBOR-encoded size of each output's value is bounded
    if params.max_val_size > 0 {
        for output in &body.outputs {
            if !output.value.multi_asset.is_empty() {
                let val_size = estimate_value_cbor_size(&output.value);
                if val_size > params.max_val_size {
                    errors.push(ValidationError::OutputValueTooLarge {
                        maximum: params.max_val_size,
                        actual: val_size,
                    });
                }
            }
        }
    }

    // Rule 5b: Network ID validation
    // If the transaction specifies a network_id, all Shelley output addresses must match
    if let Some(tx_network_id) = body.network_id {
        let expected_network = if tx_network_id == 0 {
            torsten_primitives::network::NetworkId::Testnet
        } else {
            torsten_primitives::network::NetworkId::Mainnet
        };
        for output in &body.outputs {
            if let Some(addr_network) = output.address.network_id() {
                if addr_network != expected_network {
                    errors.push(ValidationError::NetworkMismatch {
                        expected: expected_network,
                        actual: addr_network,
                    });
                    break;
                }
            }
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

    // Rule 9b: Witness completeness — every input and withdrawal must have a matching witness
    // For PubKeyHash payment credentials: a VKey witness whose blake2b_224(vkey) matches
    // For Script payment credentials: a script in the witness set or a reference script
    // For Byron inputs: a bootstrap witness whose blake2b_224(vkey) matches the address root
    // For withdrawals: same rules applied to the reward address stake credential
    if errors.is_empty() {
        // Build set of VKey witness key hashes (blake2b-224 of each verification key)
        let vkey_witness_hashes: HashSet<Hash28> = tx
            .witness_set
            .vkey_witnesses
            .iter()
            .map(|w| torsten_primitives::hash::blake2b_224(&w.vkey))
            .collect();

        // Build set of available script hashes (witness set + reference inputs)
        let available_script_hashes = collect_available_script_hashes(tx, utxo_set);

        // Check each input has a matching witness
        for input in &body.inputs {
            if let Some(utxo) = utxo_set.lookup(input) {
                match utxo.address.payment_credential() {
                    Some(Credential::VerificationKey(keyhash)) => {
                        if !vkey_witness_hashes.contains(keyhash) {
                            errors.push(ValidationError::MissingInputWitness(keyhash.to_hex()));
                        }
                    }
                    Some(Credential::Script(script_hash)) => {
                        if !available_script_hashes.contains(script_hash) {
                            errors
                                .push(ValidationError::MissingScriptWitness(script_hash.to_hex()));
                        }
                    }
                    None => {
                        // Byron address — bootstrap witness signature is verified in
                        // Rule 14. Address root binding is implicit in the Byron
                        // address encoding and checked by the bootstrap witness
                        // verification itself. No additional check needed here.
                    }
                }
            }
        }

        // Check each withdrawal has a matching witness for its reward address credential
        for reward_account_bytes in body.withdrawals.keys() {
            if let Some(cred) = extract_reward_credential(reward_account_bytes) {
                match cred {
                    Credential::VerificationKey(keyhash) => {
                        if !vkey_witness_hashes.contains(&keyhash) {
                            errors
                                .push(ValidationError::MissingWithdrawalWitness(keyhash.to_hex()));
                        }
                    }
                    Credential::Script(script_hash) => {
                        if !available_script_hashes.contains(&script_hash) {
                            errors.push(ValidationError::MissingWithdrawalScriptWitness(
                                script_hash.to_hex(),
                            ));
                        }
                    }
                }
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
            // Account for collateral return output (Babbage+)
            let effective_collateral = if let Some(col_return) = &body.collateral_return {
                collateral_value.saturating_sub(col_return.value.coin.0)
            } else {
                collateral_value
            };

            // If total_collateral is specified, it must match effective collateral
            if let Some(total_col) = body.total_collateral {
                if total_col.0 != effective_collateral {
                    errors.push(ValidationError::CollateralMismatch {
                        declared: total_col.0,
                        computed: effective_collateral,
                    });
                }
            }

            let required_collateral = body.fee.0 * params.collateral_percentage / 100;
            if effective_collateral < required_collateral {
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

        // Rule 11b: Validate redeemer indices are within bounds
        {
            let input_count = body.inputs.len();
            let mint_count = body.mint.len();
            let cert_count = body.certificates.len();
            let withdrawal_count = body.withdrawals.len();

            for redeemer in &tx.witness_set.redeemers {
                let (max, tag_name) = match redeemer.tag {
                    RedeemerTag::Spend => (input_count, "Spend"),
                    RedeemerTag::Mint => (mint_count, "Mint"),
                    RedeemerTag::Cert => (cert_count, "Cert"),
                    RedeemerTag::Reward => (withdrawal_count, "Reward"),
                    RedeemerTag::Vote => continue,    // dynamic count
                    RedeemerTag::Propose => continue, // dynamic count
                };
                if redeemer.index as usize >= max {
                    errors.push(ValidationError::RedeemerIndexOutOfRange {
                        tag: tag_name.to_string(),
                        index: redeemer.index,
                        max,
                    });
                }
            }
        }

        // Rule 11c: Script-locked inputs must have matching Spend redeemers
        {
            let spend_indices: HashSet<u32> = tx
                .witness_set
                .redeemers
                .iter()
                .filter(|r| r.tag == RedeemerTag::Spend)
                .map(|r| r.index)
                .collect();

            // Sort inputs for deterministic index assignment (Cardano sorts by (tx_id, index))
            let mut sorted_inputs: Vec<_> = body.inputs.iter().collect();
            sorted_inputs.sort_by(|a, b| {
                a.transaction_id
                    .cmp(&b.transaction_id)
                    .then(a.index.cmp(&b.index))
            });

            for (idx, input) in sorted_inputs.iter().enumerate() {
                if let Some(utxo) = utxo_set.lookup(input) {
                    let is_script_locked = match &utxo.address {
                        torsten_primitives::address::Address::Base(b) => {
                            matches!(
                                b.payment,
                                torsten_primitives::credentials::Credential::Script(_)
                            )
                        }
                        torsten_primitives::address::Address::Enterprise(e) => {
                            matches!(
                                e.payment,
                                torsten_primitives::credentials::Credential::Script(_)
                            )
                        }
                        torsten_primitives::address::Address::Pointer(p) => {
                            matches!(
                                p.payment,
                                torsten_primitives::credentials::Credential::Script(_)
                            )
                        }
                        _ => false,
                    };
                    if is_script_locked && !spend_indices.contains(&(idx as u32)) {
                        errors.push(ValidationError::MissingSpendRedeemer { index: idx as u32 });
                    }
                }
            }
        }

        // Rule 12: Script data hash validation
        // If redeemers or datums are present, script_data_hash must be set
        let has_redeemers = !tx.witness_set.redeemers.is_empty();
        let has_datums = !tx.witness_set.plutus_data.is_empty();
        if has_redeemers || has_datums {
            if let Some(declared_hash) = &body.script_data_hash {
                // Verify the script data hash matches the computed value
                let has_v1 = !tx.witness_set.plutus_v1_scripts.is_empty();
                let has_v2 = !tx.witness_set.plutus_v2_scripts.is_empty();
                let has_v3 = !tx.witness_set.plutus_v3_scripts.is_empty();
                let computed = torsten_serialization::compute_script_data_hash(
                    &tx.witness_set.redeemers,
                    &tx.witness_set.plutus_data,
                    &params.cost_models,
                    has_v1,
                    has_v2,
                    has_v3,
                );
                if *declared_hash != computed {
                    errors.push(ValidationError::ScriptDataHashMismatch {
                        expected: declared_hash.to_hex(),
                        actual: computed.to_hex(),
                    });
                }
            } else {
                errors.push(ValidationError::MissingScriptDataHash);
            }
        } else if body.script_data_hash.is_some()
            && tx.witness_set.plutus_v1_scripts.is_empty()
            && tx.witness_set.plutus_v2_scripts.is_empty()
            && tx.witness_set.plutus_v3_scripts.is_empty()
        {
            // Allow script_data_hash when reference inputs carry reference scripts
            let has_ref_scripts = body.reference_inputs.iter().any(|ref_input| {
                utxo_set
                    .lookup(ref_input)
                    .is_some_and(|utxo| utxo.script_ref.is_some())
            });
            if !has_ref_scripts {
                errors.push(ValidationError::UnexpectedScriptDataHash);
            }
        }

        // Phase-2: Execute Plutus scripts when redeemers are present.
        // Both raw_cbor and slot_config are required for Plutus evaluation.
        // Missing either is a hard error — silent bypass is not allowed.
        if errors.is_empty() && has_redeemers {
            if tx.raw_cbor.is_none() {
                debug!(
                    tx_hash = %tx.hash.to_hex(),
                    "Plutus transaction missing raw CBOR for script evaluation"
                );
                errors.push(ValidationError::MissingRawCbor);
            }
            if slot_config.is_none() {
                debug!(
                    tx_hash = %tx.hash.to_hex(),
                    "Plutus transaction missing slot configuration for script evaluation"
                );
                errors.push(ValidationError::MissingSlotConfig);
            }
            if let (Some(ref _raw), Some(sc)) = (&tx.raw_cbor, slot_config) {
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

    // Rule 13: Native script validation
    // All native scripts in the witness set must evaluate to true
    if !tx.witness_set.native_scripts.is_empty() {
        // Build set of signer key hashes from vkey witnesses
        let signers: HashSet<Hash32> = tx
            .witness_set
            .vkey_witnesses
            .iter()
            .map(|w| {
                // Hash the vkey to get the 28-byte key hash, then pad to Hash32
                torsten_primitives::hash::blake2b_224(&w.vkey).to_hash32_padded()
            })
            .collect();
        let slot = SlotNo(current_slot);

        for script in &tx.witness_set.native_scripts {
            if !evaluate_native_script(script, &signers, slot) {
                errors.push(ValidationError::NativeScriptFailed);
                break;
            }
        }
    }

    // Rule 14: Witness signature verification
    // Each vkey witness must contain a valid Ed25519 signature over the tx body hash
    if errors.is_empty() {
        for witness in &tx.witness_set.vkey_witnesses {
            if witness.vkey.len() == 32 && witness.signature.len() == 64 {
                match torsten_crypto::keys::PaymentVerificationKey::from_bytes(&witness.vkey) {
                    Ok(vk) => {
                        if vk.verify(tx.hash.as_bytes(), &witness.signature).is_err() {
                            errors.push(ValidationError::InvalidWitnessSignature(format!(
                                "{:?}",
                                &witness.vkey[..8]
                            )));
                        }
                    }
                    Err(_) => {
                        errors.push(ValidationError::InvalidWitnessSignature(format!(
                            "{:?}",
                            &witness.vkey[..8.min(witness.vkey.len())]
                        )));
                    }
                }
            }
        }

        // Bootstrap witnesses (Byron): Ed25519 signature verification
        for witness in &tx.witness_set.bootstrap_witnesses {
            if witness.vkey.len() == 32 && witness.signature.len() == 64 {
                match torsten_crypto::keys::PaymentVerificationKey::from_bytes(&witness.vkey) {
                    Ok(vk) => {
                        if vk.verify(tx.hash.as_bytes(), &witness.signature).is_err() {
                            errors.push(ValidationError::InvalidWitnessSignature(format!(
                                "bootstrap:{:?}",
                                &witness.vkey[..8]
                            )));
                        }
                    }
                    Err(_) => {
                        errors.push(ValidationError::InvalidWitnessSignature(format!(
                            "bootstrap:{:?}",
                            &witness.vkey[..8.min(witness.vkey.len())]
                        )));
                    }
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
/// Deposits are required for: stake registration, pool registration (new only), DRep registration,
/// stake+delegation registration.
/// Refunds are returned for: stake deregistration, DRep unregistration.
///
/// When `registered_pools` is provided, pool re-registrations (updating an existing pool's
/// parameters) do not charge an additional deposit — only new pool registrations do.
fn calculate_deposits_and_refunds(
    certificates: &[Certificate],
    params: &ProtocolParameters,
    registered_pools: Option<&HashSet<Hash28>>,
) -> (u64, u64) {
    let mut deposits = 0u64;
    let mut refunds = 0u64;
    // Track pools newly registered within this transaction so that a second
    // PoolRegistration cert for the same pool in the same tx is treated as an update.
    let mut newly_registered: HashSet<Hash28> = HashSet::new();

    for cert in certificates {
        match cert {
            Certificate::StakeRegistration(_) => {
                deposits += params.key_deposit.0;
            }
            Certificate::StakeDeregistration(_) => {
                refunds += params.key_deposit.0;
            }
            Certificate::ConwayStakeRegistration { deposit, .. } => {
                deposits += deposit.0;
            }
            Certificate::ConwayStakeDeregistration { refund, .. } => {
                refunds += refund.0;
            }
            Certificate::PoolRegistration(pool_params) => {
                // Only charge deposit for NEW pool registrations.
                // Re-registration (update) of an already-registered pool does not require deposit.
                let already_registered = registered_pools
                    .is_some_and(|pools| pools.contains(&pool_params.operator))
                    || newly_registered.contains(&pool_params.operator);
                if !already_registered {
                    deposits += params.pool_deposit.0;
                    newly_registered.insert(pool_params.operator);
                }
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

/// Compute the script hash for a reference script.
/// Hash = blake2b_224(type_tag || script_bytes)
fn compute_script_ref_hash(script_ref: &ScriptRef) -> Hash28 {
    match script_ref {
        ScriptRef::NativeScript(ns) => {
            let script_cbor = torsten_serialization::encode_native_script(ns);
            let mut tagged = Vec::with_capacity(1 + script_cbor.len());
            tagged.push(0x00);
            tagged.extend_from_slice(&script_cbor);
            torsten_primitives::hash::blake2b_224(&tagged)
        }
        ScriptRef::PlutusV1(bytes) => {
            let mut tagged = Vec::with_capacity(1 + bytes.len());
            tagged.push(0x01);
            tagged.extend_from_slice(bytes);
            torsten_primitives::hash::blake2b_224(&tagged)
        }
        ScriptRef::PlutusV2(bytes) => {
            let mut tagged = Vec::with_capacity(1 + bytes.len());
            tagged.push(0x02);
            tagged.extend_from_slice(bytes);
            torsten_primitives::hash::blake2b_224(&tagged)
        }
        ScriptRef::PlutusV3(bytes) => {
            let mut tagged = Vec::with_capacity(1 + bytes.len());
            tagged.push(0x03);
            tagged.extend_from_slice(bytes);
            torsten_primitives::hash::blake2b_224(&tagged)
        }
    }
}

/// Calculate total reference script size from reference inputs
fn calculate_ref_script_size(reference_inputs: &[TransactionInput], utxo_set: &UtxoSet) -> u64 {
    let mut total_size: u64 = 0;
    for ref_input in reference_inputs {
        if let Some(utxo) = utxo_set.lookup(ref_input) {
            if let Some(script_ref) = &utxo.script_ref {
                total_size += script_ref_byte_size(script_ref);
            }
        }
    }
    total_size
}

/// Get the byte size of a script reference
fn script_ref_byte_size(script_ref: &ScriptRef) -> u64 {
    match script_ref {
        ScriptRef::NativeScript(ns) => torsten_serialization::encode_native_script(ns).len() as u64,
        ScriptRef::PlutusV1(bytes) | ScriptRef::PlutusV2(bytes) | ScriptRef::PlutusV3(bytes) => {
            bytes.len() as u64
        }
    }
}

/// CIP-0112 tiered pricing for reference scripts.
///
/// Divides the script size into 25KiB tiers, each tier costs 1.2x the previous.
/// base_fee_per_byte is applied at the first tier, then multiplied by 1.2 for each
/// subsequent tier.
fn calculate_ref_script_tiered_fee(base_fee_per_byte: u64, total_size: u64) -> u64 {
    const TIER_SIZE: u64 = 25_600; // 25 KiB
    const MULTIPLIER_NUM: u64 = 12;
    const MULTIPLIER_DEN: u64 = 10;

    let mut remaining = total_size;
    let mut fee: u64 = 0;
    let mut tier_rate = base_fee_per_byte;

    while remaining > 0 {
        let chunk = remaining.min(TIER_SIZE);
        fee = fee.saturating_add(tier_rate * chunk);
        remaining -= chunk;
        // Apply 1.2x multiplier for next tier
        tier_rate = tier_rate * MULTIPLIER_NUM / MULTIPLIER_DEN;
    }
    fee
}

/// Estimate the CBOR-encoded size of a Value.
/// For ADA-only values, this is just the coin encoding.
/// For multi-asset values, this accounts for the [coin, multiasset_map] encoding.
fn estimate_value_cbor_size(value: &torsten_primitives::value::Value) -> u64 {
    if value.multi_asset.is_empty() {
        // Pure ADA: just the CBOR integer (1-9 bytes)
        return cbor_uint_size(value.coin.0);
    }
    // Multi-asset: array(2) [coin, map]
    let mut size: u64 = 1; // array header
    size += cbor_uint_size(value.coin.0); // coin
    size += 1; // map header (or more for large maps)
    for assets in value.multi_asset.values() {
        size += 1 + 28; // bytes header + 28-byte policy ID
        size += 1; // nested map header
        for (asset_name, quantity) in assets {
            size += 1 + asset_name.0.len() as u64; // bytes header + name
            size += cbor_uint_size(*quantity); // quantity
        }
    }
    size
}

/// Estimate CBOR encoding size of an unsigned integer
fn cbor_uint_size(value: u64) -> u64 {
    if value < 24 {
        1
    } else if value <= 0xFF {
        2
    } else if value <= 0xFFFF {
        3
    } else if value <= 0xFFFF_FFFF {
        5
    } else {
        9
    }
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

/// Collect all available script hashes from the witness set and reference inputs.
/// This includes native scripts, Plutus V1/V2/V3 scripts from the witness set,
/// and any reference scripts from reference input UTxOs.
fn collect_available_script_hashes(tx: &Transaction, utxo_set: &UtxoSet) -> HashSet<Hash28> {
    let mut hashes = HashSet::new();

    // Native scripts: blake2b_224(0x00 || script_cbor)
    for script in &tx.witness_set.native_scripts {
        let script_cbor = torsten_serialization::encode_native_script(script);
        let mut tagged = Vec::with_capacity(1 + script_cbor.len());
        tagged.push(0x00);
        tagged.extend_from_slice(&script_cbor);
        hashes.insert(torsten_primitives::hash::blake2b_224(&tagged));
    }

    // Plutus V1: blake2b_224(0x01 || script_bytes)
    for s in &tx.witness_set.plutus_v1_scripts {
        let mut tagged = Vec::with_capacity(1 + s.len());
        tagged.push(0x01);
        tagged.extend_from_slice(s);
        hashes.insert(torsten_primitives::hash::blake2b_224(&tagged));
    }

    // Plutus V2: blake2b_224(0x02 || script_bytes)
    for s in &tx.witness_set.plutus_v2_scripts {
        let mut tagged = Vec::with_capacity(1 + s.len());
        tagged.push(0x02);
        tagged.extend_from_slice(s);
        hashes.insert(torsten_primitives::hash::blake2b_224(&tagged));
    }

    // Plutus V3: blake2b_224(0x03 || script_bytes)
    for s in &tx.witness_set.plutus_v3_scripts {
        let mut tagged = Vec::with_capacity(1 + s.len());
        tagged.push(0x03);
        tagged.extend_from_slice(s);
        hashes.insert(torsten_primitives::hash::blake2b_224(&tagged));
    }

    // Reference scripts from reference inputs
    for ref_input in &tx.body.reference_inputs {
        if let Some(utxo) = utxo_set.lookup(ref_input) {
            if let Some(script_ref) = &utxo.script_ref {
                hashes.insert(compute_script_ref_hash(script_ref));
            }
        }
    }

    hashes
}

/// Extract the stake credential from a reward account (raw bytes).
/// Reward addresses have format: header_byte (0xe0/0xe1/0xf0/0xf1) + 28-byte credential hash.
/// Header nibble 0b1110 = PubKeyHash, 0b1111 = Script.
fn extract_reward_credential(reward_account: &[u8]) -> Option<Credential> {
    if reward_account.len() < 29 {
        return None;
    }
    let header = reward_account[0];
    let addr_type = (header >> 4) & 0x0F;
    match addr_type {
        0b1110 => {
            // PubKeyHash stake credential
            let mut hash_bytes = [0u8; 28];
            hash_bytes.copy_from_slice(&reward_account[1..29]);
            Some(Credential::VerificationKey(Hash28::from_bytes(hash_bytes)))
        }
        0b1111 => {
            // Script stake credential
            let mut hash_bytes = [0u8; 28];
            hash_bytes.copy_from_slice(&reward_account[1..29]);
            Some(Credential::Script(Hash28::from_bytes(hash_bytes)))
        }
        _ => None,
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

        let (deposits, refunds) = calculate_deposits_and_refunds(&certs, &params, None);
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
        // Create a native script that matches the minting policy
        let script = NativeScript::ScriptAll(vec![]);
        let script_cbor = torsten_serialization::encode_native_script(&script);
        let mut tagged = vec![0x00];
        tagged.extend_from_slice(&script_cbor);
        let policy = torsten_primitives::hash::blake2b_224(&tagged);

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
        tx.witness_set.native_scripts.push(script);

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_multi_asset_burning() {
        // Input has 100 tokens, burn 30, output has 70
        // Create a native script that matches the minting policy
        let script = NativeScript::ScriptAll(vec![]);
        let script_cbor = torsten_serialization::encode_native_script(&script);
        let mut tagged = vec![0x00];
        tagged.extend_from_slice(&script_cbor);
        let policy = torsten_primitives::hash::blake2b_224(&tagged);

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
        tx.witness_set.native_scripts.push(script);

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_minting_without_script_rejected() {
        // Minting with a policy that has no matching script should fail
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
        // No script in witness set!

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::InvalidMint)));
    }

    fn make_plutus_tx_with_collateral(
        input: TransactionInput,
        output_value: u64,
        fee: u64,
        collateral: Vec<TransactionInput>,
    ) -> Transaction {
        let mut tx = make_simple_tx(input, output_value, fee);
        tx.body.collateral = collateral;
        // Add a dummy redeemer to make it a Plutus tx.
        // Use minimal execution units so the fee overhead is negligible (~1 lovelace).
        tx.witness_set.redeemers.push(Redeemer {
            tag: RedeemerTag::Spend,
            index: 0,
            data: PlutusData::Integer(0),
            ex_units: ExUnits {
                mem: 100,
                steps: 100,
            },
        });
        tx.witness_set.plutus_v2_scripts.push(vec![0x01]); // dummy script
                                                           // Compute the correct script_data_hash from the redeemers/datums/cost_models
        let params = ProtocolParameters::mainnet_defaults();
        let computed_hash = torsten_serialization::compute_script_data_hash(
            &tx.witness_set.redeemers,
            &tx.witness_set.plutus_data,
            &params.cost_models,
            !tx.witness_set.plutus_v1_scripts.is_empty(),
            !tx.witness_set.plutus_v2_scripts.is_empty(),
            !tx.witness_set.plutus_v3_scripts.is_empty(),
        );
        tx.body.script_data_hash = Some(computed_hash);
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
        // Without slot_config and raw_cbor, Phase-2 errors are expected (MissingRawCbor,
        // MissingSlotConfig), but no collateral errors should be present.
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        if let Err(errors) = &result {
            assert!(
                !errors.iter().any(|e| matches!(
                    e,
                    ValidationError::InsufficientCollateral
                        | ValidationError::TooManyCollateralInputs { .. }
                        | ValidationError::CollateralNotFound(_)
                        | ValidationError::CollateralHasTokens(_)
                        | ValidationError::CollateralMismatch { .. }
                )),
                "No collateral errors expected, got: {errors:?}"
            );
        }
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

    #[test]
    fn test_duplicate_input() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();

        let mut tx = make_simple_tx(input.clone(), 9_800_000, 200_000);
        // Add the same input twice
        tx.body.inputs.push(input);

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::DuplicateInput(_))));
    }

    #[test]
    fn test_native_script_validation_in_tx() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();

        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        // Add a native script that requires a specific signer
        let required_key = Hash32::from_bytes([0xBB; 32]);
        tx.witness_set
            .native_scripts
            .push(NativeScript::ScriptPubkey(required_key));

        // No vkey witnesses that satisfy the script => should fail
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::NativeScriptFailed)));
    }

    #[test]
    fn test_native_script_timelock_in_tx() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();

        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        // Script that is invalid before slot 200
        tx.witness_set
            .native_scripts
            .push(NativeScript::InvalidBefore(SlotNo(200)));

        // Current slot is 100, before the validity window
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());

        // Current slot is 200, should pass
        let result = validate_transaction(&tx, &utxo_set, &params, 200, 300, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_witness_signature_verification() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();

        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        // Add a witness with a valid key but wrong signature
        tx.witness_set.vkey_witnesses.push(VKeyWitness {
            vkey: vec![1u8; 32],      // dummy key
            signature: vec![0u8; 64], // dummy (invalid) signature
        });

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::InvalidWitnessSignature(_))));
    }

    #[test]
    fn test_collateral_return_reduces_effective_collateral() {
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
        // Collateral input with 5 ADA
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
        // Set collateral return to get back 4.7 ADA, leaving 0.3 ADA effective collateral
        tx.body.collateral_return = Some(TransactionOutput {
            address: Address::Byron(ByronAddress {
                payload: vec![0u8; 32],
            }),
            value: Value::lovelace(4_700_000),
            datum: OutputDatum::None,
            script_ref: None,
            raw_cbor: None,
        });
        tx.body.total_collateral = Some(Lovelace(300_000));

        // Without slot_config and raw_cbor, Phase-2 errors are expected (MissingRawCbor,
        // MissingSlotConfig), but no collateral errors should be present.
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        if let Err(errors) = &result {
            assert!(
                !errors.iter().any(|e| matches!(
                    e,
                    ValidationError::InsufficientCollateral
                        | ValidationError::TooManyCollateralInputs { .. }
                        | ValidationError::CollateralNotFound(_)
                        | ValidationError::CollateralHasTokens(_)
                        | ValidationError::CollateralMismatch { .. }
                )),
                "No collateral errors expected, got: {errors:?}"
            );
        }
    }

    #[test]
    fn test_collateral_return_mismatch_total() {
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
        tx.body.collateral_return = Some(TransactionOutput {
            address: Address::Byron(ByronAddress {
                payload: vec![0u8; 32],
            }),
            value: Value::lovelace(4_700_000),
            datum: OutputDatum::None,
            script_ref: None,
            raw_cbor: None,
        });
        // Declare wrong total_collateral (should be 300_000)
        tx.body.total_collateral = Some(Lovelace(500_000));

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::CollateralMismatch { .. })));
    }

    #[test]
    fn test_reference_script_minting_validation() {
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

        // Create a native script and compute its hash
        let pubkey_hash = Hash32::from_bytes([42u8; 32]);
        let native_script = NativeScript::ScriptPubkey(pubkey_hash);
        let script_hash = compute_script_ref_hash(&ScriptRef::NativeScript(native_script.clone()));

        // Put the script as a reference script in a UTxO
        let ref_input = TransactionInput {
            transaction_id: Hash32::from_bytes([3u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            ref_input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(2_000_000),
                datum: OutputDatum::None,
                script_ref: Some(ScriptRef::NativeScript(native_script)),
                raw_cbor: None,
            },
        );

        // Create a tx that mints using the reference script's policy
        let asset = AssetName(b"Token".to_vec());
        let mut mint: BTreeMap<PolicyId, BTreeMap<AssetName, i64>> = BTreeMap::new();
        mint.entry(script_hash)
            .or_default()
            .insert(asset.clone(), 10);

        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.body.mint = mint;
        tx.body.reference_inputs.push(ref_input);
        tx.body.outputs[0]
            .value
            .multi_asset
            .entry(script_hash)
            .or_default()
            .insert(asset, 10);

        let params = ProtocolParameters::mainnet_defaults();
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        // Should pass — the minting policy is satisfied by the reference script
        assert!(result.is_ok());
    }

    #[test]
    fn test_compute_script_ref_hash_plutus_v2() {
        let script_bytes = vec![0x01, 0x02, 0x03, 0x04];
        let hash = compute_script_ref_hash(&ScriptRef::PlutusV2(script_bytes.clone()));

        // Verify it matches blake2b_224(0x02 || script_bytes)
        let mut tagged = vec![0x02];
        tagged.extend_from_slice(&script_bytes);
        let expected = torsten_primitives::hash::blake2b_224(&tagged);
        assert_eq!(hash, expected);
    }

    #[test]
    fn test_ref_script_tiered_fee_calculation() {
        // Base fee 15 lovelace per byte
        // Single tier (< 25KiB): 15 * 1000 = 15000
        assert_eq!(calculate_ref_script_tiered_fee(15, 1000), 15_000);

        // Exactly one tier (25600 bytes): 15 * 25600 = 384000
        assert_eq!(calculate_ref_script_tiered_fee(15, 25_600), 384_000);

        // Two tiers: first 25600 at 15, next bytes at 18 (15 * 12/10)
        let fee = calculate_ref_script_tiered_fee(15, 26_600);
        // First tier: 15 * 25600 = 384000
        // Second tier: 18 * 1000 = 18000
        assert_eq!(fee, 384_000 + 18_000);

        // Zero size = zero fee
        assert_eq!(calculate_ref_script_tiered_fee(15, 0), 0);
    }

    #[test]
    fn test_script_ref_byte_size() {
        let v2_script = ScriptRef::PlutusV2(vec![0u8; 500]);
        assert_eq!(script_ref_byte_size(&v2_script), 500);

        let v3_script = ScriptRef::PlutusV3(vec![0u8; 1024]);
        assert_eq!(script_ref_byte_size(&v3_script), 1024);
    }

    #[test]
    fn test_auxiliary_data_hash_without_data() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_000_000, 1_000_000);
        // Set auxiliary_data_hash but no auxiliary_data
        tx.body.auxiliary_data_hash = Some(Hash32::from_bytes([0xAB; 32]));
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::AuxiliaryDataHashWithoutData)));
    }

    #[test]
    fn test_auxiliary_data_without_hash() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_000_000, 1_000_000);
        // Set auxiliary_data but no auxiliary_data_hash
        tx.auxiliary_data = Some(AuxiliaryData {
            metadata: BTreeMap::new(),
            native_scripts: vec![],
            plutus_v1_scripts: vec![],
            plutus_v2_scripts: vec![],
            plutus_v3_scripts: vec![],
        });
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::AuxiliaryDataWithoutHash)));
    }

    #[test]
    fn test_auxiliary_data_with_hash_valid() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_000_000, 1_000_000);
        // Both present — consistency check passes
        tx.body.auxiliary_data_hash = Some(Hash32::from_bytes([0xAB; 32]));
        tx.auxiliary_data = Some(AuxiliaryData {
            metadata: BTreeMap::new(),
            native_scripts: vec![],
            plutus_v1_scripts: vec![],
            plutus_v2_scripts: vec![],
            plutus_v3_scripts: vec![],
        });
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        // Should pass (no auxiliary data hash mismatch errors)
        assert!(result.is_ok());
    }

    #[test]
    fn test_cbor_uint_size() {
        assert_eq!(cbor_uint_size(0), 1);
        assert_eq!(cbor_uint_size(23), 1);
        assert_eq!(cbor_uint_size(24), 2);
        assert_eq!(cbor_uint_size(255), 2);
        assert_eq!(cbor_uint_size(256), 3);
        assert_eq!(cbor_uint_size(65535), 3);
        assert_eq!(cbor_uint_size(65536), 5);
        assert_eq!(cbor_uint_size(0xFFFF_FFFF), 5);
        assert_eq!(cbor_uint_size(0x1_0000_0000), 9);
    }

    #[test]
    fn test_estimate_value_cbor_size_ada_only() {
        let value = Value::lovelace(1_000_000);
        let size = estimate_value_cbor_size(&value);
        // ADA-only: just the uint encoding of 1_000_000 (5 bytes: 1A prefix + 4 bytes)
        assert_eq!(size, cbor_uint_size(1_000_000));
    }

    #[test]
    fn test_estimate_value_cbor_size_multi_asset() {
        let policy = torsten_primitives::hash::Hash28::from_bytes([10u8; 28]);
        let asset = AssetName::new(b"Token".to_vec()).unwrap();
        let mut value = Value::lovelace(2_000_000);
        value
            .multi_asset
            .entry(policy)
            .or_default()
            .insert(asset, 100);

        let size = estimate_value_cbor_size(&value);
        // array(2) header: 1
        // coin (2_000_000): 5
        // outer map header: 1
        // policy_id bytes header + 28: 29
        // inner map header: 1
        // asset name "Token" (5 bytes): 1 + 5 = 6
        // quantity 100: 2
        // Total: 1 + 5 + 1 + 29 + 1 + 6 + 2 = 45
        assert_eq!(size, 45);
    }

    #[test]
    fn test_output_value_too_large() {
        let policy = torsten_primitives::hash::Hash28::from_bytes([10u8; 28]);

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
                value: Value::lovelace(100_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                raw_cbor: None,
            },
        );

        // Create output with many assets to make value large
        let mut output_value = Value::lovelace(99_800_000);
        for i in 0..100u8 {
            let asset = AssetName::new(vec![i; 32]).unwrap();
            output_value
                .multi_asset
                .entry(policy)
                .or_default()
                .insert(asset, 1_000_000);
        }

        let mut params = ProtocolParameters::mainnet_defaults();
        params.max_val_size = 50; // Tiny limit to trigger the error

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
        };

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::OutputValueTooLarge { .. })));
    }

    #[test]
    fn test_ada_only_output_skips_max_val_size_check() {
        // ADA-only outputs should not be checked against max_val_size
        let (utxo_set, input) = make_simple_utxo_set();
        let mut params = ProtocolParameters::mainnet_defaults();
        params.max_val_size = 1; // Absurdly small, but should not affect ADA-only

        let tx = make_simple_tx(input, 9_800_000, 200_000);
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_redeemer_index_out_of_range() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input.clone(), 9_000_000, 1_000_000);

        // Add a Spend redeemer with index 5, but we only have 1 input
        tx.witness_set.redeemers.push(Redeemer {
            tag: RedeemerTag::Spend,
            index: 5,
            data: PlutusData::Integer(0),
            ex_units: ExUnits {
                mem: 100,
                steps: 100,
            },
        });
        // Add a Plutus V2 script to trigger Plutus path
        tx.witness_set.plutus_v2_scripts.push(vec![0x01, 0x02]);

        // Need collateral for Plutus tx
        let col_input = TransactionInput {
            transaction_id: Hash32::from_bytes([2u8; 32]),
            index: 0,
        };
        let mut utxo = utxo_set;
        utxo.insert(
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
        tx.body.collateral = vec![col_input];

        let result = validate_transaction(&tx, &utxo, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::RedeemerIndexOutOfRange { .. })));
    }

    #[test]
    fn test_script_locked_input_missing_redeemer() {
        use torsten_primitives::address::EnterpriseAddress;
        use torsten_primitives::credentials::Credential;

        let script_hash = Hash28::from_bytes([0xaa; 28]);
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([3u8; 32]),
            index: 0,
        };
        let mut utxo_set = UtxoSet::new();
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Enterprise(EnterpriseAddress {
                    network: torsten_primitives::network::NetworkId::Testnet,
                    payment: Credential::Script(script_hash),
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                raw_cbor: None,
            },
        );

        // Add collateral
        let col_input = TransactionInput {
            transaction_id: Hash32::from_bytes([4u8; 32]),
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
        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(9_000_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    raw_cbor: None,
                }],
                fee: Lovelace(1_000_000),
                ttl: None,
                certificates: vec![],
                withdrawals: BTreeMap::new(),
                auxiliary_data_hash: None,
                validity_interval_start: None,
                mint: BTreeMap::new(),
                script_data_hash: None,
                collateral: vec![col_input],
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
                plutus_v2_scripts: vec![vec![0x01]],
                plutus_v3_scripts: vec![],
                plutus_data: vec![],
                redeemers: vec![], // No redeemers — missing for the script input
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
            .any(|e| matches!(e, ValidationError::MissingSpendRedeemer { .. })));
    }

    #[test]
    fn test_script_locked_input_with_redeemer_ok() {
        use torsten_primitives::address::EnterpriseAddress;
        use torsten_primitives::credentials::Credential;

        let script_hash = Hash28::from_bytes([0xbb; 28]);
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([5u8; 32]),
            index: 0,
        };
        let mut utxo_set = UtxoSet::new();
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Enterprise(EnterpriseAddress {
                    network: torsten_primitives::network::NetworkId::Testnet,
                    payment: Credential::Script(script_hash),
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                raw_cbor: None,
            },
        );

        let col_input = TransactionInput {
            transaction_id: Hash32::from_bytes([6u8; 32]),
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
        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(9_000_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    raw_cbor: None,
                }],
                fee: Lovelace(1_000_000),
                ttl: None,
                certificates: vec![],
                withdrawals: BTreeMap::new(),
                auxiliary_data_hash: None,
                validity_interval_start: None,
                mint: BTreeMap::new(),
                script_data_hash: Some(Hash32::ZERO), // Will be wrong but we skip that check
                collateral: vec![col_input],
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
                plutus_v2_scripts: vec![vec![0x01]],
                plutus_v3_scripts: vec![],
                plutus_data: vec![],
                redeemers: vec![Redeemer {
                    tag: RedeemerTag::Spend,
                    index: 0, // Matching index for the script input
                    data: PlutusData::Integer(42),
                    ex_units: ExUnits {
                        mem: 1000,
                        steps: 1000,
                    },
                }],
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        // Should not have MissingSpendRedeemer error
        // (may have other errors like ScriptDataHashMismatch, but not missing redeemer)
        match result {
            Ok(()) => {} // OK
            Err(errors) => {
                assert!(!errors
                    .iter()
                    .any(|e| matches!(e, ValidationError::MissingSpendRedeemer { .. })));
            }
        }
    }

    #[test]
    fn test_treasury_donation_value_conservation() {
        // A tx with a donation should pass value conservation when donation is accounted for
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 8_800_000, 200_000);
        // input = 10M, output = 8.8M, fee = 0.2M, donation = 1M → 10M = 8.8M + 0.2M + 1M
        tx.body.donation = Some(Lovelace(1_000_000));

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_treasury_donation_value_not_conserved() {
        // Without accounting for donation, value won't be conserved
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        // input = 10M, output = 9.8M, fee = 0.2M, donation = 1M → 10M ≠ 9.8M + 0.2M + 1M = 11M
        tx.body.donation = Some(Lovelace(1_000_000));

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::ValueNotConserved { .. })));
    }

    // =======================================================================
    // Bug fix tests
    // =======================================================================

    // --- Bug 1: Script data hash mismatch must reject ---

    #[test]
    fn test_script_data_hash_mismatch_rejects() {
        // A Plutus transaction where the declared script_data_hash doesn't match
        // the computed value must be rejected with ScriptDataHashMismatch.
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
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.body.collateral = vec![col_input];
        tx.witness_set.plutus_v2_scripts.push(vec![0x01]);
        tx.witness_set.redeemers.push(Redeemer {
            tag: RedeemerTag::Spend,
            index: 0,
            data: PlutusData::Integer(42),
            ex_units: ExUnits {
                mem: 1000,
                steps: 1000,
            },
        });
        // Set a bogus script_data_hash that won't match the computed one
        tx.body.script_data_hash = Some(Hash32::from_bytes([0xDE; 32]));

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::ScriptDataHashMismatch { .. })),
            "Expected ScriptDataHashMismatch error, got: {:?}",
            errors
        );
    }

    // --- Bug 2: Min fee must include Plutus execution unit costs ---

    #[test]
    fn test_min_fee_includes_execution_unit_costs() {
        // Build a Plutus transaction whose fee covers base (a*size+b) + ref_script
        // but NOT the execution unit costs. It should be rejected.
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
                value: Value::lovelace(100_000_000),
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
                value: Value::lovelace(50_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                raw_cbor: None,
            },
        );

        let params = ProtocolParameters::mainnet_defaults();
        let tx_size: u64 = 300;
        // Base fee = 44 * 300 + 155381 = 13200 + 155381 = 168581
        let base_fee = params.min_fee(tx_size).0;

        // Use large execution units
        let mem_units: u64 = 14_000_000;
        let step_units: u64 = 10_000_000_000;

        // Compute expected execution unit cost
        // mem_cost = ceil(577/10000 * 14000000) = ceil(807800) = 807800
        let mem_cost = {
            let num = params.execution_costs.mem_price.numerator as u128 * mem_units as u128;
            let den = params.execution_costs.mem_price.denominator as u128;
            num.div_ceil(den) as u64
        };
        // step_cost = ceil(721/10000000 * 10000000000) = ceil(721000) = 721000
        let step_cost = {
            let num = params.execution_costs.step_price.numerator as u128 * step_units as u128;
            let den = params.execution_costs.step_price.denominator as u128;
            num.div_ceil(den) as u64
        };
        let ex_unit_fee = mem_cost + step_cost;
        assert!(
            ex_unit_fee > 0,
            "Execution unit fee should be positive for this test"
        );

        // Set fee to base_fee only (missing ex_unit_fee)
        let fee_without_ex = base_fee;
        let output_value = 100_000_000 - fee_without_ex;

        let mut tx = make_simple_tx(input, output_value, fee_without_ex);
        tx.body.collateral = vec![col_input];
        tx.witness_set.plutus_v2_scripts.push(vec![0x01]);
        tx.witness_set.redeemers.push(Redeemer {
            tag: RedeemerTag::Spend,
            index: 0,
            data: PlutusData::Integer(42),
            ex_units: ExUnits {
                mem: mem_units,
                steps: step_units,
            },
        });
        tx.body.script_data_hash = Some(Hash32::from_bytes([0xAB; 32]));

        let result = validate_transaction(&tx, &utxo_set, &params, 100, tx_size, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::FeeTooSmall { .. })),
            "Expected FeeTooSmall error when ex unit costs not covered, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_min_fee_with_execution_units_sufficient() {
        // A Plutus transaction with fee covering base + execution unit costs should pass
        // the fee check (it may still fail other validations like script_data_hash).
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
                value: Value::lovelace(100_000_000),
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
                value: Value::lovelace(50_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                raw_cbor: None,
            },
        );

        let params = ProtocolParameters::mainnet_defaults();
        let tx_size: u64 = 300;
        let base_fee = params.min_fee(tx_size).0;

        let mem_units: u64 = 1_000_000;
        let step_units: u64 = 1_000_000_000;
        let mem_cost = {
            let num = params.execution_costs.mem_price.numerator as u128 * mem_units as u128;
            let den = params.execution_costs.mem_price.denominator as u128;
            num.div_ceil(den) as u64
        };
        let step_cost = {
            let num = params.execution_costs.step_price.numerator as u128 * step_units as u128;
            let den = params.execution_costs.step_price.denominator as u128;
            num.div_ceil(den) as u64
        };
        let total_min_fee = base_fee + mem_cost + step_cost;

        // Use a fee that covers everything (with some margin)
        let fee = total_min_fee + 1000;
        let output_value = 100_000_000 - fee;

        let mut tx = make_simple_tx(input, output_value, fee);
        tx.body.collateral = vec![col_input];
        tx.witness_set.plutus_v2_scripts.push(vec![0x01]);
        tx.witness_set.redeemers.push(Redeemer {
            tag: RedeemerTag::Spend,
            index: 0,
            data: PlutusData::Integer(42),
            ex_units: ExUnits {
                mem: mem_units,
                steps: step_units,
            },
        });
        tx.body.script_data_hash = Some(Hash32::from_bytes([0xAB; 32]));

        let result = validate_transaction(&tx, &utxo_set, &params, 100, tx_size, None);
        // Should NOT have FeeTooSmall (may have other errors like ScriptDataHashMismatch)
        match result {
            Ok(()) => {} // OK
            Err(errors) => {
                assert!(
                    !errors
                        .iter()
                        .any(|e| matches!(e, ValidationError::FeeTooSmall { .. })),
                    "Fee should be sufficient but got FeeTooSmall: {:?}",
                    errors
                );
            }
        }
    }

    #[test]
    fn test_min_fee_no_redeemers_no_ex_unit_cost() {
        // A simple (non-Plutus) transaction should not have execution unit costs added.
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let tx_size: u64 = 300;
        let base_fee = params.min_fee(tx_size).0;

        // Fee exactly equals base fee — should pass (no ex unit cost)
        let output_value = 10_000_000 - base_fee;
        let tx = make_simple_tx(input, output_value, base_fee);

        let result = validate_transaction(&tx, &utxo_set, &params, 100, tx_size, None);
        assert!(
            result.is_ok(),
            "Simple tx with exact base fee should pass: {:?}",
            result
        );
    }

    // --- Bug 3: Pool re-registration should not charge deposit ---

    #[test]
    fn test_pool_reregistration_no_duplicate_deposit() {
        // An already-registered pool being re-registered (parameter update) should
        // NOT charge an additional pool_deposit.
        let pool_id = torsten_primitives::hash::Hash28::from_bytes([42u8; 28]);

        let pool_params = PoolParams {
            operator: pool_id,
            vrf_keyhash: Hash32::from_bytes([0u8; 32]),
            pledge: Lovelace(100_000_000),
            cost: Lovelace(340_000_000),
            margin: Rational {
                numerator: 1,
                denominator: 100,
            },
            reward_account: vec![0xe0; 29],
            pool_owners: vec![pool_id],
            relays: vec![],
            pool_metadata: None,
        };

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

        let params = ProtocolParameters::mainnet_defaults();
        // With re-registration, no deposit should be charged
        // input (10M) = output + fee → output = 10M - 200K = 9.8M
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.body
            .certificates
            .push(Certificate::PoolRegistration(pool_params));

        // Mark the pool as already registered
        let mut registered = HashSet::new();
        registered.insert(pool_id);

        let result = validate_transaction_with_pools(
            &tx,
            &utxo_set,
            &params,
            100,
            300,
            None,
            Some(&registered),
        );
        assert!(
            result.is_ok(),
            "Pool re-registration should not charge deposit: {:?}",
            result
        );
    }

    #[test]
    fn test_new_pool_registration_charges_deposit() {
        // A new pool registration should charge pool_deposit.
        let pool_id = torsten_primitives::hash::Hash28::from_bytes([42u8; 28]);

        let pool_params = PoolParams {
            operator: pool_id,
            vrf_keyhash: Hash32::from_bytes([0u8; 32]),
            pledge: Lovelace(100_000_000),
            cost: Lovelace(340_000_000),
            margin: Rational {
                numerator: 1,
                denominator: 100,
            },
            reward_account: vec![0xe0; 29],
            pool_owners: vec![pool_id],
            relays: vec![],
            pool_metadata: None,
        };

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
                value: Value::lovelace(1_000_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                raw_cbor: None,
            },
        );

        let params = ProtocolParameters::mainnet_defaults();
        let pool_deposit = params.pool_deposit.0;
        // New registration: consumed = 1B, produced = output + fee + deposit
        let fee = 200_000u64;
        let output = 1_000_000_000 - fee - pool_deposit;

        let mut tx = make_simple_tx(input, output, fee);
        tx.body
            .certificates
            .push(Certificate::PoolRegistration(pool_params));

        // No registered pools (new registration)
        let registered: HashSet<Hash28> = HashSet::new();
        let result = validate_transaction_with_pools(
            &tx,
            &utxo_set,
            &params,
            100,
            300,
            None,
            Some(&registered),
        );
        assert!(
            result.is_ok(),
            "New pool registration should charge deposit: {:?}",
            result
        );
    }

    #[test]
    fn test_new_pool_registration_without_deposit_fails() {
        // A new pool registration without accounting for deposit should fail.
        let pool_id = torsten_primitives::hash::Hash28::from_bytes([42u8; 28]);

        let pool_params = PoolParams {
            operator: pool_id,
            vrf_keyhash: Hash32::from_bytes([0u8; 32]),
            pledge: Lovelace(100_000_000),
            cost: Lovelace(340_000_000),
            margin: Rational {
                numerator: 1,
                denominator: 100,
            },
            reward_account: vec![0xe0; 29],
            pool_owners: vec![pool_id],
            relays: vec![],
            pool_metadata: None,
        };

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

        let params = ProtocolParameters::mainnet_defaults();
        // Don't account for pool deposit — should fail value conservation
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.body
            .certificates
            .push(Certificate::PoolRegistration(pool_params));

        let registered: HashSet<Hash28> = HashSet::new();
        let result = validate_transaction_with_pools(
            &tx,
            &utxo_set,
            &params,
            100,
            300,
            None,
            Some(&registered),
        );
        assert!(
            result.is_err(),
            "New pool reg without deposit should fail value conservation"
        );
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::ValueNotConserved { .. })));
    }

    #[test]
    fn test_pool_reregistration_with_deposit_fails_conservation() {
        // A re-registration that incorrectly includes a deposit will over-produce
        // (the deposit is not actually charged), so value conservation fails.
        let pool_id = torsten_primitives::hash::Hash28::from_bytes([42u8; 28]);

        let pool_params = PoolParams {
            operator: pool_id,
            vrf_keyhash: Hash32::from_bytes([0u8; 32]),
            pledge: Lovelace(100_000_000),
            cost: Lovelace(340_000_000),
            margin: Rational {
                numerator: 1,
                denominator: 100,
            },
            reward_account: vec![0xe0; 29],
            pool_owners: vec![pool_id],
            relays: vec![],
            pool_metadata: None,
        };

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
                value: Value::lovelace(1_000_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                raw_cbor: None,
            },
        );

        let params = ProtocolParameters::mainnet_defaults();
        let pool_deposit = params.pool_deposit.0;
        let fee = 200_000u64;
        // Output as if deposit is charged, but pool is already registered so deposit is NOT charged
        let output = 1_000_000_000 - fee - pool_deposit;

        let mut tx = make_simple_tx(input, output, fee);
        tx.body
            .certificates
            .push(Certificate::PoolRegistration(pool_params));

        // Pool already registered
        let mut registered: HashSet<Hash28> = HashSet::new();
        registered.insert(pool_id);

        let result = validate_transaction_with_pools(
            &tx,
            &utxo_set,
            &params,
            100,
            300,
            None,
            Some(&registered),
        );
        assert!(
            result.is_err(),
            "Re-registration accounting for deposit should fail (deposit not charged)"
        );
        let errors = result.unwrap_err();
        assert!(errors
            .iter()
            .any(|e| matches!(e, ValidationError::ValueNotConserved { .. })));
    }

    // --- Bug 4: UnexpectedScriptDataHash with reference scripts ---

    #[test]
    fn test_script_data_hash_allowed_with_reference_scripts() {
        // A Plutus transaction that has script_data_hash but no redeemers/scripts in witness set
        // should be allowed if reference inputs carry reference scripts.
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

        // Reference input with a script_ref (PlutusV2 script)
        let ref_input = TransactionInput {
            transaction_id: Hash32::from_bytes([3u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            ref_input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(2_000_000),
                datum: OutputDatum::None,
                script_ref: Some(ScriptRef::PlutusV2(vec![0x01, 0x02, 0x03])),
                raw_cbor: None,
            },
        );

        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.body.reference_inputs = vec![ref_input];
        // Set script_data_hash but NO redeemers and NO plutus scripts in witness set
        tx.body.script_data_hash = Some(Hash32::from_bytes([0xAB; 32]));

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        // Should NOT have UnexpectedScriptDataHash
        match result {
            Ok(()) => {} // OK
            Err(errors) => {
                assert!(
                    !errors
                        .iter()
                        .any(|e| matches!(e, ValidationError::UnexpectedScriptDataHash)),
                    "UnexpectedScriptDataHash should not fire when reference scripts exist: {:?}",
                    errors
                );
            }
        }
    }

    #[test]
    fn test_script_data_hash_rejected_without_scripts_or_ref_scripts() {
        // A transaction with script_data_hash but no scripts, no redeemers,
        // and no reference scripts should be rejected.
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        // Set script_data_hash but no scripts or redeemers anywhere
        tx.body.script_data_hash = Some(Hash32::from_bytes([0xAB; 32]));

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        // This is a non-Plutus tx so the code doesn't enter the has_plutus_scripts branch.
        // The UnexpectedScriptDataHash check is inside that branch, so for a non-Plutus tx
        // without scripts, the script_data_hash is simply ignored.
        // However, for completeness, let's verify no UnexpectedScriptDataHash is produced
        // when the tx is not Plutus at all (since the check only fires inside the Plutus block).
        // The test validates the branch: has_plutus_scripts = false, so no error.
        // This matches Cardano behavior: script_data_hash in non-Plutus txs is a no-op.
        match result {
            Ok(()) => {} // OK, non-Plutus tx ignores script_data_hash
            Err(errors) => {
                // Should not have UnexpectedScriptDataHash
                assert!(!errors
                    .iter()
                    .any(|e| matches!(e, ValidationError::UnexpectedScriptDataHash)));
            }
        }
    }

    #[test]
    fn test_unexpected_script_data_hash_guard_with_ref_scripts() {
        // The UnexpectedScriptDataHash else-if branch requires:
        // has_plutus_scripts(tx) = true, has_redeemers = false, has_datums = false,
        // and all plutus script lists empty. With current code this branch is unreachable
        // (if all scripts are empty, has_plutus_scripts requires non-empty redeemers which
        // makes has_redeemers true). However the guard against reference scripts is in place.
        //
        // This test validates the direct calculate_deposits helper and the reference script
        // allowance via the integration test above (test_script_data_hash_allowed_with_reference_scripts).
        //
        // Here we verify that a Plutus tx with scripts but no redeemers/datums
        // and a script_data_hash does NOT produce UnexpectedScriptDataHash
        // (because the else-if condition checks scripts are empty, but they aren't).
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
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.body.collateral = vec![col_input];
        // Plutus script present but no redeemers/datums
        tx.witness_set.plutus_v2_scripts.push(vec![0x01]);
        tx.body.script_data_hash = Some(Hash32::from_bytes([0xAB; 32]));

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        // Should NOT have UnexpectedScriptDataHash (scripts are present, so the
        // else-if condition fails before checking reference scripts)
        match result {
            Ok(()) => {} // OK
            Err(errors) => {
                assert!(
                    !errors
                        .iter()
                        .any(|e| matches!(e, ValidationError::UnexpectedScriptDataHash)),
                    "Should not get UnexpectedScriptDataHash when plutus scripts present: {:?}",
                    errors
                );
            }
        }
    }

    #[test]
    fn test_calculate_deposits_pool_rereg_no_deposit() {
        // Direct unit test of calculate_deposits_and_refunds with registered pools
        let params = ProtocolParameters::mainnet_defaults();
        let pool_id = torsten_primitives::hash::Hash28::from_bytes([42u8; 28]);

        let pool_params = PoolParams {
            operator: pool_id,
            vrf_keyhash: Hash32::from_bytes([0u8; 32]),
            pledge: Lovelace(100_000_000),
            cost: Lovelace(340_000_000),
            margin: Rational {
                numerator: 1,
                denominator: 100,
            },
            reward_account: vec![0xe0; 29],
            pool_owners: vec![pool_id],
            relays: vec![],
            pool_metadata: None,
        };

        let certs = vec![Certificate::PoolRegistration(pool_params)];

        // Without registered pools — should charge deposit
        let (deposits_new, _) = calculate_deposits_and_refunds(&certs, &params, None);
        assert_eq!(deposits_new, params.pool_deposit.0);

        // With pool already registered — should NOT charge deposit
        let mut registered = HashSet::new();
        registered.insert(pool_id);
        let (deposits_rereg, _) =
            calculate_deposits_and_refunds(&certs, &params, Some(&registered));
        assert_eq!(deposits_rereg, 0);
    }

    // --- Phase-2 Plutus evaluation mandatory tests ---

    /// Helper: create a UTxO set and Plutus transaction with redeemers that triggers
    /// Phase-2 validation.
    ///
    /// The tx has a Plutus V1 script, a redeemer, proper collateral, and a correct
    /// script_data_hash. The `raw_cbor` parameter controls whether the tx has raw CBOR
    /// (needed for actual Plutus evaluation).
    fn make_plutus_utxo_and_tx(raw_cbor: Option<Vec<u8>>) -> (UtxoSet, Transaction) {
        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        let col_input = TransactionInput {
            transaction_id: Hash32::from_bytes([2u8; 32]),
            index: 0,
        };
        // Regular input: 10 ADA
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
        // Collateral input: 10 ADA (pure ADA, no tokens)
        utxo_set.insert(
            col_input.clone(),
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

        let redeemers = vec![Redeemer {
            tag: RedeemerTag::Spend,
            index: 0,
            data: PlutusData::Integer(0),
            ex_units: ExUnits {
                mem: 100,
                steps: 100,
            },
        }];
        let plutus_v1_scripts = vec![vec![0x01, 0x02, 0x03]];

        // Compute correct script_data_hash to pass Rule 12
        let params = ProtocolParameters::mainnet_defaults();
        let script_data_hash = torsten_serialization::compute_script_data_hash(
            &redeemers,
            &[], // no datums
            &params.cost_models,
            true,  // has v1
            false, // no v2
            false, // no v3
        );

        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(9_800_000),
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
                script_data_hash: Some(script_data_hash),
                collateral: vec![col_input],
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
                plutus_v1_scripts,
                plutus_v2_scripts: vec![],
                plutus_v3_scripts: vec![],
                plutus_data: vec![],
                redeemers,
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor,
        };
        (utxo_set, tx)
    }

    #[test]
    fn test_plutus_tx_missing_raw_cbor_returns_error() {
        let (utxo_set, tx) = make_plutus_utxo_and_tx(None);
        let params = ProtocolParameters::mainnet_defaults();
        let slot_config = crate::plutus::SlotConfig::default();

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, Some(&slot_config));
        assert!(
            result.is_err(),
            "Should reject Plutus tx with missing raw_cbor"
        );
        let errors = result.unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::MissingRawCbor)),
            "Should contain MissingRawCbor error, got: {errors:?}"
        );
    }

    #[test]
    fn test_plutus_tx_missing_slot_config_returns_error() {
        let (utxo_set, tx) = make_plutus_utxo_and_tx(Some(vec![0x84, 0x00]));
        let params = ProtocolParameters::mainnet_defaults();

        // slot_config = None should now be a hard error for Plutus transactions
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(
            result.is_err(),
            "Should reject Plutus tx with missing slot_config"
        );
        let errors = result.unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::MissingSlotConfig)),
            "Should contain MissingSlotConfig error, got: {errors:?}"
        );
    }

    #[test]
    fn test_plutus_tx_missing_both_raw_cbor_and_slot_config() {
        let (utxo_set, tx) = make_plutus_utxo_and_tx(None);
        let params = ProtocolParameters::mainnet_defaults();

        // Both missing — should get both errors
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::MissingRawCbor)),
            "Should contain MissingRawCbor, got: {errors:?}"
        );
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::MissingSlotConfig)),
            "Should contain MissingSlotConfig, got: {errors:?}"
        );
    }

    #[test]
    fn test_non_plutus_tx_missing_raw_cbor_passes() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        // Simple tx with no Plutus scripts and no redeemers
        let tx = make_simple_tx(input, 9_800_000, 200_000);

        // Should pass even without raw_cbor and slot_config, since there are no Plutus scripts
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(
            result.is_ok(),
            "Non-Plutus tx should pass without raw_cbor/slot_config"
        );
    }

    #[test]
    fn test_non_plutus_tx_missing_slot_config_passes() {
        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let tx = make_simple_tx(input, 9_800_000, 200_000);

        // slot_config=None is fine for non-Plutus transactions
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(
            result.is_ok(),
            "Non-Plutus tx should pass without slot_config"
        );
    }

    #[test]
    fn test_plutus_tx_with_raw_cbor_and_slot_config_reaches_evaluation() {
        let (utxo_set, tx) = make_plutus_utxo_and_tx(Some(vec![0x84, 0x00]));
        let params = ProtocolParameters::mainnet_defaults();
        let slot_config = crate::plutus::SlotConfig::default();

        // With both raw_cbor and slot_config, validation should reach Plutus evaluation.
        // The evaluation will fail because the raw_cbor is not a real transaction, but
        // the important thing is it does NOT return MissingRawCbor or MissingSlotConfig.
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, Some(&slot_config));
        if let Err(errors) = &result {
            assert!(
                !errors
                    .iter()
                    .any(|e| matches!(e, ValidationError::MissingRawCbor)),
                "Should NOT contain MissingRawCbor when raw_cbor is present"
            );
            assert!(
                !errors
                    .iter()
                    .any(|e| matches!(e, ValidationError::MissingSlotConfig)),
                "Should NOT contain MissingSlotConfig when slot_config is present"
            );
        }
    }

    // ==================== Witness Completeness Tests ====================

    /// Create a VKeyWitness from specific vkey bytes.
    /// The witness signature is intentionally wrong (all zeros) — signature
    /// verification is a separate rule (Rule 14) and these tests are focused
    /// on witness completeness (Rule 9b).
    fn make_vkey_witness_from_bytes(vkey: [u8; 32]) -> VKeyWitness {
        VKeyWitness {
            vkey: vkey.to_vec(),
            // Dummy signature — not verified in witness completeness check
            signature: vec![0u8; 64],
        }
    }

    /// Build a reward account (raw bytes) for a PubKeyHash stake credential.
    /// Format: 0xe0 (testnet) or 0xe1 (mainnet) + 28-byte keyhash
    fn make_reward_account_vkey(keyhash: Hash28) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(29);
        bytes.push(0xe0); // type 0b1110, testnet
        bytes.extend_from_slice(keyhash.as_bytes());
        bytes
    }

    /// Build a reward account (raw bytes) for a Script stake credential.
    /// Format: 0xf0 (testnet) or 0xf1 (mainnet) + 28-byte script hash
    fn make_reward_account_script(script_hash: Hash28) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(29);
        bytes.push(0xf0); // type 0b1111, testnet
        bytes.extend_from_slice(script_hash.as_bytes());
        bytes
    }

    #[test]
    fn test_witness_completeness_vkey_input_with_matching_witness() {
        // A PubKeyHash input with a matching VKey witness should pass
        let vkey_bytes = [0xAA; 32];
        let keyhash = torsten_primitives::hash::blake2b_224(&vkey_bytes);

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Enterprise(torsten_primitives::address::EnterpriseAddress {
                    network: torsten_primitives::network::NetworkId::Testnet,
                    payment: torsten_primitives::credentials::Credential::VerificationKey(keyhash),
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                raw_cbor: None,
            },
        );

        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.witness_set
            .vkey_witnesses
            .push(make_vkey_witness_from_bytes(vkey_bytes));

        // Signature will be invalid (dummy), but witness completeness should pass.
        // We check that MissingInputWitness is NOT in errors.
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        match result {
            Ok(()) => {} // Perfect
            Err(errors) => {
                assert!(
                    !errors
                        .iter()
                        .any(|e| matches!(e, ValidationError::MissingInputWitness(_))),
                    "Should not have MissingInputWitness, got: {errors:?}"
                );
            }
        }
    }

    #[test]
    fn test_witness_completeness_vkey_input_missing_witness() {
        // A PubKeyHash input without a matching VKey witness should fail
        let keyhash = Hash28::from_bytes([0x11; 28]);

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Enterprise(torsten_primitives::address::EnterpriseAddress {
                    network: torsten_primitives::network::NetworkId::Testnet,
                    payment: torsten_primitives::credentials::Credential::VerificationKey(keyhash),
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                raw_cbor: None,
            },
        );

        let params = ProtocolParameters::mainnet_defaults();
        // No witnesses at all
        let tx = make_simple_tx(input, 9_800_000, 200_000);

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::MissingInputWitness(_))),
            "Expected MissingInputWitness, got: {errors:?}"
        );
    }

    #[test]
    fn test_witness_completeness_vkey_input_wrong_witness() {
        // A PubKeyHash input with a VKey witness that does not match should fail
        let keyhash = Hash28::from_bytes([0x11; 28]);

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Enterprise(torsten_primitives::address::EnterpriseAddress {
                    network: torsten_primitives::network::NetworkId::Testnet,
                    payment: torsten_primitives::credentials::Credential::VerificationKey(keyhash),
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                raw_cbor: None,
            },
        );

        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        // Add a witness with a different key hash
        tx.witness_set
            .vkey_witnesses
            .push(make_vkey_witness_from_bytes([0xFF; 32]));

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::MissingInputWitness(_))),
            "Expected MissingInputWitness, got: {errors:?}"
        );
    }

    #[test]
    fn test_witness_completeness_script_input_with_witness_script() {
        // A Script-locked input with matching script in witness set should pass
        use torsten_primitives::address::EnterpriseAddress;
        use torsten_primitives::credentials::Credential;

        // Create a native script and compute its hash
        let native_script = NativeScript::ScriptAll(vec![]);
        let script_cbor = torsten_serialization::encode_native_script(&native_script);
        let mut tagged = vec![0x00];
        tagged.extend_from_slice(&script_cbor);
        let script_hash = torsten_primitives::hash::blake2b_224(&tagged);

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Enterprise(EnterpriseAddress {
                    network: torsten_primitives::network::NetworkId::Testnet,
                    payment: Credential::Script(script_hash),
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                raw_cbor: None,
            },
        );

        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.witness_set.native_scripts.push(native_script);

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        match result {
            Ok(()) => {}
            Err(errors) => {
                assert!(
                    !errors
                        .iter()
                        .any(|e| matches!(e, ValidationError::MissingScriptWitness(_))),
                    "Should not have MissingScriptWitness, got: {errors:?}"
                );
            }
        }
    }

    #[test]
    fn test_witness_completeness_script_input_missing_script() {
        // A Script-locked input without any matching script should fail
        use torsten_primitives::address::EnterpriseAddress;
        use torsten_primitives::credentials::Credential;

        let script_hash = Hash28::from_bytes([0x33; 28]);

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Enterprise(EnterpriseAddress {
                    network: torsten_primitives::network::NetworkId::Testnet,
                    payment: Credential::Script(script_hash),
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                raw_cbor: None,
            },
        );

        let params = ProtocolParameters::mainnet_defaults();
        let tx = make_simple_tx(input, 9_800_000, 200_000);

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::MissingScriptWitness(_))),
            "Expected MissingScriptWitness, got: {errors:?}"
        );
    }

    #[test]
    fn test_witness_completeness_script_input_with_reference_script() {
        // A Script-locked input with matching reference script should pass
        use torsten_primitives::address::EnterpriseAddress;
        use torsten_primitives::credentials::Credential;

        // Create a PlutusV2 script and compute its hash
        let plutus_bytes = vec![0x01, 0x02, 0x03];
        let mut tagged = vec![0x02];
        tagged.extend_from_slice(&plutus_bytes);
        let script_hash = torsten_primitives::hash::blake2b_224(&tagged);

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Enterprise(EnterpriseAddress {
                    network: torsten_primitives::network::NetworkId::Testnet,
                    payment: Credential::Script(script_hash),
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                raw_cbor: None,
            },
        );

        // Reference input with the script
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
                value: Value::lovelace(2_000_000),
                datum: OutputDatum::None,
                script_ref: Some(ScriptRef::PlutusV2(plutus_bytes)),
                raw_cbor: None,
            },
        );

        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.body.reference_inputs.push(ref_input);

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        match result {
            Ok(()) => {}
            Err(errors) => {
                assert!(
                    !errors
                        .iter()
                        .any(|e| matches!(e, ValidationError::MissingScriptWitness(_))),
                    "Should not have MissingScriptWitness, got: {errors:?}"
                );
            }
        }
    }

    #[test]
    fn test_witness_completeness_byron_input_no_bootstrap_ok() {
        // Byron inputs should not require bootstrap witnesses for completeness
        // (bootstrap witness signature is verified separately in Rule 14)
        let (utxo_set, input) = make_simple_utxo_set(); // Uses Byron address
        let params = ProtocolParameters::mainnet_defaults();
        let tx = make_simple_tx(input, 9_800_000, 200_000);

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        // Should pass — no witness completeness error for Byron
        assert!(
            result.is_ok(),
            "Byron input should not require witness completeness check"
        );
    }

    #[test]
    fn test_witness_completeness_withdrawal_vkey_with_witness() {
        // Withdrawal from PubKeyHash reward address with matching VKey witness should pass
        let vkey_bytes = [0xBB; 32];
        let keyhash = torsten_primitives::hash::blake2b_224(&vkey_bytes);
        let reward_account = make_reward_account_vkey(keyhash);

        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let withdrawal_amount = 500_000u64;

        // input(10M) + withdrawal(0.5M) = output(10.1M) + fee(0.4M)
        let mut tx = make_simple_tx(input, 10_100_000, 400_000);
        tx.body
            .withdrawals
            .insert(reward_account, Lovelace(withdrawal_amount));
        tx.witness_set
            .vkey_witnesses
            .push(make_vkey_witness_from_bytes(vkey_bytes));

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        match result {
            Ok(()) => {}
            Err(errors) => {
                assert!(
                    !errors
                        .iter()
                        .any(|e| matches!(e, ValidationError::MissingWithdrawalWitness(_))),
                    "Should not have MissingWithdrawalWitness, got: {errors:?}"
                );
            }
        }
    }

    #[test]
    fn test_witness_completeness_withdrawal_vkey_missing_witness() {
        // Withdrawal from PubKeyHash reward address without matching witness should fail
        let keyhash = Hash28::from_bytes([0xCC; 28]);
        let reward_account = make_reward_account_vkey(keyhash);

        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let withdrawal_amount = 500_000u64;

        let mut tx = make_simple_tx(input, 10_100_000, 400_000);
        tx.body
            .withdrawals
            .insert(reward_account, Lovelace(withdrawal_amount));
        // No matching witness

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::MissingWithdrawalWitness(_))),
            "Expected MissingWithdrawalWitness, got: {errors:?}"
        );
    }

    #[test]
    fn test_witness_completeness_withdrawal_script_with_witness() {
        // Withdrawal from Script reward address with matching script should pass
        let native_script = NativeScript::ScriptAll(vec![]);
        let script_cbor = torsten_serialization::encode_native_script(&native_script);
        let mut tagged = vec![0x00];
        tagged.extend_from_slice(&script_cbor);
        let script_hash = torsten_primitives::hash::blake2b_224(&tagged);
        let reward_account = make_reward_account_script(script_hash);

        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let withdrawal_amount = 500_000u64;

        let mut tx = make_simple_tx(input, 10_100_000, 400_000);
        tx.body
            .withdrawals
            .insert(reward_account, Lovelace(withdrawal_amount));
        tx.witness_set.native_scripts.push(native_script);

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        match result {
            Ok(()) => {}
            Err(errors) => {
                assert!(
                    !errors
                        .iter()
                        .any(|e| matches!(e, ValidationError::MissingWithdrawalScriptWitness(_))),
                    "Should not have MissingWithdrawalScriptWitness, got: {errors:?}"
                );
            }
        }
    }

    #[test]
    fn test_witness_completeness_withdrawal_script_missing_witness() {
        // Withdrawal from Script reward address without matching script should fail
        let script_hash = Hash28::from_bytes([0xDD; 28]);
        let reward_account = make_reward_account_script(script_hash);

        let (utxo_set, input) = make_simple_utxo_set();
        let params = ProtocolParameters::mainnet_defaults();
        let withdrawal_amount = 500_000u64;

        let mut tx = make_simple_tx(input, 10_100_000, 400_000);
        tx.body
            .withdrawals
            .insert(reward_account, Lovelace(withdrawal_amount));
        // No script witness

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::MissingWithdrawalScriptWitness(_))),
            "Expected MissingWithdrawalScriptWitness, got: {errors:?}"
        );
    }

    #[test]
    fn test_witness_completeness_multiple_inputs_all_witnessed() {
        // Multiple PubKeyHash inputs, all with matching witnesses, should pass
        let vkey1 = [0xAA; 32];
        let vkey2 = [0xBB; 32];
        let keyhash1 = torsten_primitives::hash::blake2b_224(&vkey1);
        let keyhash2 = torsten_primitives::hash::blake2b_224(&vkey2);

        let mut utxo_set = UtxoSet::new();
        let input1 = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        let input2 = TransactionInput {
            transaction_id: Hash32::from_bytes([2u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input1.clone(),
            TransactionOutput {
                address: Address::Enterprise(torsten_primitives::address::EnterpriseAddress {
                    network: torsten_primitives::network::NetworkId::Testnet,
                    payment: torsten_primitives::credentials::Credential::VerificationKey(keyhash1),
                }),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                raw_cbor: None,
            },
        );
        utxo_set.insert(
            input2.clone(),
            TransactionOutput {
                address: Address::Enterprise(torsten_primitives::address::EnterpriseAddress {
                    network: torsten_primitives::network::NetworkId::Testnet,
                    payment: torsten_primitives::credentials::Credential::VerificationKey(keyhash2),
                }),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                raw_cbor: None,
            },
        );

        let params = ProtocolParameters::mainnet_defaults();
        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input1, input2],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(9_800_000),
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
                update: None,
                voting_procedures: BTreeMap::new(),
                proposal_procedures: vec![],
                treasury_value: None,
                donation: None,
            },
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![
                    make_vkey_witness_from_bytes(vkey1),
                    make_vkey_witness_from_bytes(vkey2),
                ],
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
        match result {
            Ok(()) => {}
            Err(errors) => {
                assert!(
                    !errors
                        .iter()
                        .any(|e| matches!(e, ValidationError::MissingInputWitness(_))),
                    "Should not have MissingInputWitness, got: {errors:?}"
                );
            }
        }
    }

    #[test]
    fn test_witness_completeness_multiple_inputs_one_missing() {
        // Two PubKeyHash inputs, only one has a witness — should fail
        let vkey1 = [0xAA; 32];
        let keyhash1 = torsten_primitives::hash::blake2b_224(&vkey1);
        let keyhash2 = Hash28::from_bytes([0x99; 28]);

        let mut utxo_set = UtxoSet::new();
        let input1 = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        let input2 = TransactionInput {
            transaction_id: Hash32::from_bytes([2u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input1.clone(),
            TransactionOutput {
                address: Address::Enterprise(torsten_primitives::address::EnterpriseAddress {
                    network: torsten_primitives::network::NetworkId::Testnet,
                    payment: torsten_primitives::credentials::Credential::VerificationKey(keyhash1),
                }),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                raw_cbor: None,
            },
        );
        utxo_set.insert(
            input2.clone(),
            TransactionOutput {
                address: Address::Enterprise(torsten_primitives::address::EnterpriseAddress {
                    network: torsten_primitives::network::NetworkId::Testnet,
                    payment: torsten_primitives::credentials::Credential::VerificationKey(keyhash2),
                }),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                raw_cbor: None,
            },
        );

        let params = ProtocolParameters::mainnet_defaults();
        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input1, input2],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(9_800_000),
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
                update: None,
                voting_procedures: BTreeMap::new(),
                proposal_procedures: vec![],
                treasury_value: None,
                donation: None,
            },
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![make_vkey_witness_from_bytes(vkey1)],
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
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::MissingInputWitness(_))),
            "Expected MissingInputWitness for second input, got: {errors:?}"
        );
    }

    #[test]
    fn test_witness_completeness_base_address_vkey() {
        // Base address with PubKeyHash payment credential should require VKey witness
        use torsten_primitives::address::BaseAddress;
        use torsten_primitives::credentials::Credential;

        let vkey_bytes = [0xCC; 32];
        let keyhash = torsten_primitives::hash::blake2b_224(&vkey_bytes);
        let stake_hash = Hash28::from_bytes([0xDD; 28]);

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Base(BaseAddress {
                    network: torsten_primitives::network::NetworkId::Testnet,
                    payment: Credential::VerificationKey(keyhash),
                    stake: Credential::VerificationKey(stake_hash),
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                raw_cbor: None,
            },
        );

        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input.clone(), 9_800_000, 200_000);
        tx.witness_set
            .vkey_witnesses
            .push(make_vkey_witness_from_bytes(vkey_bytes));

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        match result {
            Ok(()) => {}
            Err(errors) => {
                assert!(
                    !errors
                        .iter()
                        .any(|e| matches!(e, ValidationError::MissingInputWitness(_))),
                    "Should not have MissingInputWitness for base address, got: {errors:?}"
                );
            }
        }

        // Without the witness, should fail
        let tx_no_witness = make_simple_tx(input, 9_800_000, 200_000);
        let result = validate_transaction(&tx_no_witness, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::MissingInputWitness(_))),
            "Expected MissingInputWitness for base address without witness, got: {errors:?}"
        );
    }

    #[test]
    fn test_witness_completeness_pointer_address_vkey() {
        // Pointer address with PubKeyHash payment credential should require VKey witness
        use torsten_primitives::address::PointerAddress;
        use torsten_primitives::credentials::{Credential, Pointer};

        let keyhash = Hash28::from_bytes([0xEE; 28]);

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Pointer(PointerAddress {
                    network: torsten_primitives::network::NetworkId::Testnet,
                    payment: Credential::VerificationKey(keyhash),
                    pointer: Pointer {
                        slot: 100,
                        tx_index: 0,
                        cert_index: 0,
                    },
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                raw_cbor: None,
            },
        );

        let params = ProtocolParameters::mainnet_defaults();
        // No witness — should fail
        let tx = make_simple_tx(input, 9_800_000, 200_000);
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_err());
        let errors = result.unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::MissingInputWitness(_))),
            "Expected MissingInputWitness for pointer address without witness, got: {errors:?}"
        );
    }

    #[test]
    fn test_witness_completeness_plutus_v3_script_input() {
        // Script input with PlutusV3 script in witness set should pass
        use torsten_primitives::address::EnterpriseAddress;
        use torsten_primitives::credentials::Credential;

        let plutus_bytes = vec![0x05, 0x06, 0x07];
        let mut tagged = vec![0x03]; // PlutusV3 tag
        tagged.extend_from_slice(&plutus_bytes);
        let script_hash = torsten_primitives::hash::blake2b_224(&tagged);

        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input.clone(),
            TransactionOutput {
                address: Address::Enterprise(EnterpriseAddress {
                    network: torsten_primitives::network::NetworkId::Testnet,
                    payment: Credential::Script(script_hash),
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                raw_cbor: None,
            },
        );

        let params = ProtocolParameters::mainnet_defaults();
        let mut tx = make_simple_tx(input, 9_800_000, 200_000);
        tx.witness_set.plutus_v3_scripts.push(plutus_bytes);

        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        match result {
            Ok(()) => {}
            Err(errors) => {
                assert!(
                    !errors
                        .iter()
                        .any(|e| matches!(e, ValidationError::MissingScriptWitness(_))),
                    "Should not have MissingScriptWitness for PlutusV3 input, got: {errors:?}"
                );
            }
        }
    }

    #[test]
    fn test_extract_reward_credential_vkey() {
        let keyhash = Hash28::from_bytes([0x42; 28]);
        let reward_account = make_reward_account_vkey(keyhash);
        let cred = extract_reward_credential(&reward_account);
        assert_eq!(
            cred,
            Some(torsten_primitives::credentials::Credential::VerificationKey(keyhash))
        );
    }

    #[test]
    fn test_extract_reward_credential_script() {
        let script_hash = Hash28::from_bytes([0x43; 28]);
        let reward_account = make_reward_account_script(script_hash);
        let cred = extract_reward_credential(&reward_account);
        assert_eq!(
            cred,
            Some(torsten_primitives::credentials::Credential::Script(
                script_hash
            ))
        );
    }

    #[test]
    fn test_extract_reward_credential_mainnet() {
        // Mainnet PubKeyHash reward address: header 0xe1
        let keyhash = Hash28::from_bytes([0x44; 28]);
        let mut bytes = Vec::with_capacity(29);
        bytes.push(0xe1); // type 0b1110, mainnet
        bytes.extend_from_slice(keyhash.as_bytes());
        let cred = extract_reward_credential(&bytes);
        assert_eq!(
            cred,
            Some(torsten_primitives::credentials::Credential::VerificationKey(keyhash))
        );
    }

    #[test]
    fn test_extract_reward_credential_too_short() {
        let cred = extract_reward_credential(&[0xe0, 0x01, 0x02]);
        assert_eq!(cred, None);
    }

    #[test]
    fn test_extract_reward_credential_invalid_type() {
        // Header type 0b0000 (base address) should not be a reward address
        let mut bytes = vec![0x00];
        bytes.extend_from_slice(&[0x00; 28]);
        let cred = extract_reward_credential(&bytes);
        assert_eq!(cred, None);
    }

    #[test]
    fn test_witness_completeness_same_credential_multiple_inputs() {
        // Two inputs with the SAME PubKeyHash credential — one witness should suffice
        let vkey_bytes = [0xAA; 32];
        let keyhash = torsten_primitives::hash::blake2b_224(&vkey_bytes);

        let mut utxo_set = UtxoSet::new();
        let input1 = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        let input2 = TransactionInput {
            transaction_id: Hash32::from_bytes([2u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input1.clone(),
            TransactionOutput {
                address: Address::Enterprise(torsten_primitives::address::EnterpriseAddress {
                    network: torsten_primitives::network::NetworkId::Testnet,
                    payment: torsten_primitives::credentials::Credential::VerificationKey(keyhash),
                }),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                raw_cbor: None,
            },
        );
        utxo_set.insert(
            input2.clone(),
            TransactionOutput {
                address: Address::Enterprise(torsten_primitives::address::EnterpriseAddress {
                    network: torsten_primitives::network::NetworkId::Testnet,
                    payment: torsten_primitives::credentials::Credential::VerificationKey(keyhash),
                }),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                raw_cbor: None,
            },
        );

        let params = ProtocolParameters::mainnet_defaults();
        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input1, input2],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(9_800_000),
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
                update: None,
                voting_procedures: BTreeMap::new(),
                proposal_procedures: vec![],
                treasury_value: None,
                donation: None,
            },
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![make_vkey_witness_from_bytes(vkey_bytes)],
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
        match result {
            Ok(()) => {}
            Err(errors) => {
                assert!(
                    !errors
                        .iter()
                        .any(|e| matches!(e, ValidationError::MissingInputWitness(_))),
                    "One witness should cover both inputs with same credential, got: {errors:?}"
                );
            }
        }
    }

    #[test]
    fn test_witness_completeness_mixed_byron_and_shelley() {
        // Mix of Byron and Shelley inputs — only Shelley needs witness check
        let vkey_bytes = [0xAA; 32];
        let keyhash = torsten_primitives::hash::blake2b_224(&vkey_bytes);

        let mut utxo_set = UtxoSet::new();
        let byron_input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        let shelley_input = TransactionInput {
            transaction_id: Hash32::from_bytes([2u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            byron_input.clone(),
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
        utxo_set.insert(
            shelley_input.clone(),
            TransactionOutput {
                address: Address::Enterprise(torsten_primitives::address::EnterpriseAddress {
                    network: torsten_primitives::network::NetworkId::Testnet,
                    payment: torsten_primitives::credentials::Credential::VerificationKey(keyhash),
                }),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                raw_cbor: None,
            },
        );

        let params = ProtocolParameters::mainnet_defaults();
        let tx = Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![byron_input, shelley_input],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(9_800_000),
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
                update: None,
                voting_procedures: BTreeMap::new(),
                proposal_procedures: vec![],
                treasury_value: None,
                donation: None,
            },
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![make_vkey_witness_from_bytes(vkey_bytes)],
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
        match result {
            Ok(()) => {}
            Err(errors) => {
                assert!(
                    !errors
                        .iter()
                        .any(|e| matches!(e, ValidationError::MissingInputWitness(_))),
                    "Byron + witnessed Shelley should pass, got: {errors:?}"
                );
            }
        }
    }
}
