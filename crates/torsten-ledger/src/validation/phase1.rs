//! Core Phase-1 validation rules.
//!
//! This module implements the structural rules that do not require Plutus script
//! execution. Each numbered rule corresponds to a distinct ledger invariant:
//!
//! - Rule 1  — at least one input
//! - Rule 1b — no duplicate inputs
//! - Rule 1c — auxiliary data hash / auxiliary data consistency
//! - Rule 1d — era gating (Conway-only certs/governance in pre-Conway eras)
//! - Rule 2  — all inputs exist in the UTxO set
//! - Rule 3  — ADA value conservation
//! - Rule 3b — multi-asset conservation
//! - Rule 3c — every minting policy has a matching script
//! - Rule 4  — fee >= minimum (base + ref-script + ex-unit costs)
//! - Rule 5  — all outputs >= minimum UTxO value
//! - Rule 5a — output value CBOR size <= max_val_size
//! - Rule 5b — network ID consistency
//! - Rule 6  — transaction size <= max_tx_size
//! - Rule 7  — TTL (time-to-live)
//! - Rule 8  — validity interval start
//! - Rule 9  — reference inputs exist and don't overlap regular inputs
//! - Rule 9b — witness completeness for inputs and withdrawals
//! - Rule 10 — required signers have matching vkey witnesses
//! - Rule 11 — collateral (Plutus transactions only; see `collateral` module)
//! - Rule 12 — script data hash (Plutus transactions only; see `scripts` module)
//! - Rule 13 — native script evaluation
//! - Rule 14 — Ed25519 vkey/bootstrap witness signature verification

use std::collections::HashSet;

use torsten_primitives::credentials::Credential;
use torsten_primitives::hash::Hash28;
use torsten_primitives::protocol_params::ProtocolParameters;
use torsten_primitives::time::SlotNo;
use torsten_primitives::transaction::Transaction;

use crate::utxo::UtxoSet;

use super::scripts::{
    collect_available_script_hashes, compute_min_fee, estimate_value_cbor_size,
    evaluate_native_script,
};
use super::ValidationError;

// ---------------------------------------------------------------------------
// Helper: extract stake credential from a raw reward account byte string
// ---------------------------------------------------------------------------

/// Extract the stake credential from a reward account byte string.
///
/// Reward addresses have the format `header_byte || 28-byte credential hash`.
/// - Header nibble `0b1110` (`0xE0`/`0xE1`) → `VerificationKey`
/// - Header nibble `0b1111` (`0xF0`/`0xF1`) → `Script`
pub(super) fn extract_reward_credential(reward_account: &[u8]) -> Option<Credential> {
    if reward_account.len() < 29 {
        return None;
    }
    let header = reward_account[0];
    let addr_type = (header >> 4) & 0x0F;
    match addr_type {
        0b1110 => {
            let mut hash_bytes = [0u8; 28];
            hash_bytes.copy_from_slice(&reward_account[1..29]);
            Some(Credential::VerificationKey(Hash28::from_bytes(hash_bytes)))
        }
        0b1111 => {
            let mut hash_bytes = [0u8; 28];
            hash_bytes.copy_from_slice(&reward_account[1..29]);
            Some(Credential::Script(Hash28::from_bytes(hash_bytes)))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Helper: check if the transaction involves any multi-asset tokens
// ---------------------------------------------------------------------------

/// Return `true` when any input UTxO or output carries non-ADA tokens.
pub(super) fn has_multi_assets_in_tx(tx: &Transaction, utxo_set: &UtxoSet) -> bool {
    for input in &tx.body.inputs {
        if let Some(output) = utxo_set.lookup(input) {
            if !output.value.multi_asset.is_empty() {
                return true;
            }
        }
    }
    tx.body
        .outputs
        .iter()
        .any(|o| !o.value.multi_asset.is_empty())
}

// ---------------------------------------------------------------------------
// Witness signature verification (Rule 14)
// ---------------------------------------------------------------------------

trait HasWitnessFields {
    fn vkey(&self) -> &[u8];
    fn signature(&self) -> &[u8];
}

impl HasWitnessFields for torsten_primitives::transaction::VKeyWitness {
    fn vkey(&self) -> &[u8] {
        &self.vkey
    }
    fn signature(&self) -> &[u8] {
        &self.signature
    }
}

impl HasWitnessFields for torsten_primitives::transaction::BootstrapWitness {
    fn vkey(&self) -> &[u8] {
        &self.vkey
    }
    fn signature(&self) -> &[u8] {
        &self.signature
    }
}

fn verify_single_witness<W: HasWitnessFields>(
    witness: &W,
    tx_hash_bytes: &[u8],
    prefix: &str,
) -> Option<ValidationError> {
    let vkey = witness.vkey();
    let sig = witness.signature();
    // Witnesses with non-standard key sizes are silently accepted —
    // Cardano uses Ed25519 (32-byte keys, 64-byte sigs) and we only
    // verify those.
    if vkey.len() != 32 || sig.len() != 64 {
        return None;
    }
    match torsten_crypto::keys::PaymentVerificationKey::from_bytes(vkey) {
        Ok(vk) => {
            if vk.verify(tx_hash_bytes, sig).is_err() {
                Some(ValidationError::InvalidWitnessSignature(format!(
                    "{prefix}{:?}",
                    &vkey[..8]
                )))
            } else {
                None
            }
        }
        Err(_) => Some(ValidationError::InvalidWitnessSignature(format!(
            "{prefix}{:?}",
            &vkey[..8.min(vkey.len())]
        ))),
    }
}

#[cfg(feature = "parallel-verification")]
fn verify_witness_signatures<W: HasWitnessFields + Sync>(
    witnesses: &[W],
    tx_hash_bytes: &[u8],
    prefix: &str,
) -> Vec<ValidationError> {
    use rayon::prelude::*;
    witnesses
        .par_iter()
        .filter_map(|w| verify_single_witness(w, tx_hash_bytes, prefix))
        .collect()
}

#[cfg(not(feature = "parallel-verification"))]
fn verify_witness_signatures<W: HasWitnessFields>(
    witnesses: &[W],
    tx_hash_bytes: &[u8],
    prefix: &str,
) -> Vec<ValidationError> {
    witnesses
        .iter()
        .filter_map(|w| verify_single_witness(w, tx_hash_bytes, prefix))
        .collect()
}

// ---------------------------------------------------------------------------
// Phase-1 rule execution
// ---------------------------------------------------------------------------

/// Run all core Phase-1 rules that are independent of Plutus scripts.
///
/// Rules that require the Plutus-script context (11, 12) are handled in the
/// caller (`validate_transaction_with_pools`) which invokes the `collateral`
/// and `scripts` modules. Results are accumulated in `errors`.
///
/// Returns `input_value` (sum of ADA across all resolved inputs) so the caller
/// can pass it to the value-conservation check without re-scanning inputs.
pub(super) fn run_phase1_rules(
    tx: &Transaction,
    utxo_set: &UtxoSet,
    params: &ProtocolParameters,
    current_slot: u64,
    tx_size: u64,
    registered_pools: Option<&std::collections::HashSet<Hash28>>,
    errors: &mut Vec<ValidationError>,
) {
    let body = &tx.body;

    // ------------------------------------------------------------------
    // Rule 1: Must have at least one input
    // ------------------------------------------------------------------
    if body.inputs.is_empty() {
        errors.push(ValidationError::NoInputs);
    }

    // ------------------------------------------------------------------
    // Rule 1b: No duplicate inputs
    // ------------------------------------------------------------------
    {
        let mut seen = HashSet::new();
        for input in &body.inputs {
            if !seen.insert(input) {
                errors.push(ValidationError::DuplicateInput(input.to_string()));
            }
        }
    }

    // ------------------------------------------------------------------
    // Rule 1c: Auxiliary data hash / auxiliary data consistency
    // ------------------------------------------------------------------
    match (&body.auxiliary_data_hash, &tx.auxiliary_data) {
        (Some(_), None) => {
            errors.push(ValidationError::AuxiliaryDataHashWithoutData);
        }
        (None, Some(_)) => {
            errors.push(ValidationError::AuxiliaryDataWithoutHash);
        }
        _ => {} // Both present or both absent — OK
    }

    // ------------------------------------------------------------------
    // Rule 1d: Era gating
    // ------------------------------------------------------------------
    super::conway::check_era_gating(params, body, errors);

    // ------------------------------------------------------------------
    // Rule 2: All inputs must exist in the UTxO set
    // ------------------------------------------------------------------
    let mut input_value: u128 = 0;
    for input in &body.inputs {
        match utxo_set.lookup(input) {
            Some(output) => {
                input_value += output.value.coin.0 as u128;
            }
            None => {
                errors.push(ValidationError::InputNotFound(input.to_string()));
            }
        }
    }

    // ------------------------------------------------------------------
    // Rule 3: ADA value conservation
    // consumed = Σ(inputs) + Σ(withdrawals) + Σ(refunds)
    // produced = Σ(outputs) + fee + Σ(deposits) + proposal_deposits + donation
    // ------------------------------------------------------------------
    if errors.is_empty() {
        let output_value: u128 = body.outputs.iter().map(|o| o.value.coin.0 as u128).sum();
        let withdrawal_value: u128 = body.withdrawals.values().map(|l| l.0 as u128).sum();

        let (total_deposits, total_refunds) = super::conway::calculate_deposits_and_refunds(
            &body.certificates,
            params,
            registered_pools,
        );

        // Proposal deposits (Conway governance) — use u128 to prevent mul overflow
        let proposal_deposits =
            body.proposal_procedures.len() as u128 * params.gov_action_deposit.0 as u128;

        // Treasury donation (Conway)
        let donation = body.donation.map(|d| d.0 as u128).unwrap_or(0);

        let consumed = input_value + withdrawal_value + total_refunds as u128;
        let produced = output_value
            + body.fee.0 as u128
            + total_deposits as u128
            + proposal_deposits
            + donation;

        if consumed != produced {
            errors.push(ValidationError::ValueNotConserved {
                inputs: consumed.min(u64::MAX as u128) as u64,
                outputs: output_value.min(u64::MAX as u128) as u64,
                fee: body.fee.0,
            });
        }
    }

    // ------------------------------------------------------------------
    // Rule 3b: Multi-asset conservation
    // ------------------------------------------------------------------
    if errors.is_empty() && (!body.mint.is_empty() || has_multi_assets_in_tx(tx, utxo_set)) {
        use std::collections::BTreeMap;
        use torsten_primitives::hash::PolicyId;
        use torsten_primitives::value::AssetName;

        let mut asset_balance: BTreeMap<(PolicyId, AssetName), i128> = BTreeMap::new();

        for input in &body.inputs {
            if let Some(output) = utxo_set.lookup(input) {
                for (policy, assets) in &output.value.multi_asset {
                    for (name, qty) in assets {
                        *asset_balance.entry((*policy, name.clone())).or_insert(0) += *qty as i128;
                    }
                }
            }
        }
        for (policy, assets) in &body.mint {
            for (name, qty) in assets {
                *asset_balance.entry((*policy, name.clone())).or_insert(0) += *qty as i128;
            }
        }
        for output in &body.outputs {
            for (policy, assets) in &output.value.multi_asset {
                for (name, qty) in assets {
                    *asset_balance.entry((*policy, name.clone())).or_insert(0) -= *qty as i128;
                }
            }
        }

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
                break;
            }
        }
    }

    // ------------------------------------------------------------------
    // Rule 3c: Every minting policy must have a matching script
    // ------------------------------------------------------------------
    if !body.mint.is_empty() {
        let mut available_script_hashes: HashSet<Hash28> = HashSet::new();

        for script in &tx.witness_set.native_scripts {
            let script_cbor = torsten_serialization::encode_native_script(script);
            let mut tagged = Vec::with_capacity(1 + script_cbor.len());
            tagged.push(0x00);
            tagged.extend_from_slice(&script_cbor);
            available_script_hashes.insert(torsten_primitives::hash::blake2b_224(&tagged));
        }
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
        // Per Haskell's `scriptsProvided`, script_refs are collected from BOTH
        // spending inputs AND reference inputs.  A minting policy satisfied via
        // a script_ref that lives in a spending-input UTxO is therefore valid.
        for inp in body.inputs.iter().chain(body.reference_inputs.iter()) {
            if let Some(utxo) = utxo_set.lookup(inp) {
                if let Some(script_ref) = &utxo.script_ref {
                    let hash = super::scripts::compute_script_ref_hash(script_ref);
                    available_script_hashes.insert(hash);
                }
            }
        }

        for policy in body.mint.keys() {
            if !available_script_hashes.contains(policy) {
                tracing::debug!(
                    policy = %policy.to_hex(),
                    "Minting policy without matching script in witness set, spending inputs, or reference inputs"
                );
                errors.push(ValidationError::InvalidMint);
                break;
            }
        }
    }

    // ------------------------------------------------------------------
    // Rule 4: Fee >= minimum (base + ref-script + ex-unit costs)
    // ------------------------------------------------------------------
    let min_fee = compute_min_fee(tx, utxo_set, params, tx_size);
    if body.fee.0 < min_fee.0 {
        errors.push(ValidationError::FeeTooSmall {
            minimum: min_fee.0,
            actual: body.fee.0,
        });
    }

    // ------------------------------------------------------------------
    // Rule 5: All outputs >= minimum UTxO value
    // ------------------------------------------------------------------
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

    // ------------------------------------------------------------------
    // Rule 5a: Output value CBOR size <= max_val_size
    // ------------------------------------------------------------------
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

    // ------------------------------------------------------------------
    // Rule 5b: Network ID consistency
    // ------------------------------------------------------------------
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

    // ------------------------------------------------------------------
    // Rule 6: Transaction size limit
    // ------------------------------------------------------------------
    if tx_size > params.max_tx_size {
        errors.push(ValidationError::TxTooLarge {
            maximum: params.max_tx_size,
            actual: tx_size,
        });
    }

    // ------------------------------------------------------------------
    // Rule 7: TTL check
    // ------------------------------------------------------------------
    if let Some(ttl) = body.ttl {
        if current_slot > ttl.0 {
            errors.push(ValidationError::TtlExpired {
                current_slot,
                ttl: ttl.0,
            });
        }
    }

    // ------------------------------------------------------------------
    // Rule 8: Validity interval start
    // ------------------------------------------------------------------
    if let Some(start) = body.validity_interval_start {
        if current_slot < start.0 {
            errors.push(ValidationError::NotYetValid {
                current_slot,
                valid_from: start.0,
            });
        }
    }

    // ------------------------------------------------------------------
    // Rule 9: Reference inputs must exist and not overlap with regular inputs
    // ------------------------------------------------------------------
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

    // ------------------------------------------------------------------
    // Rule 9b: Witness completeness
    // ------------------------------------------------------------------
    if errors.is_empty() {
        // Build the set of VKey witness key hashes (blake2b-224 of each vkey)
        let vkey_witness_hashes: HashSet<Hash28> = tx
            .witness_set
            .vkey_witnesses
            .iter()
            .map(|w| torsten_primitives::hash::blake2b_224(&w.vkey))
            .collect();

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
                        // Byron address — bootstrap witness is verified in Rule 14.
                        // No additional completeness check needed here.
                    }
                }
            }
        }

        // Check each withdrawal has a matching witness for its reward credential
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

    // ------------------------------------------------------------------
    // Rule 9c: Datum witness completeness
    //
    // Enforced only when all inputs resolve successfully (Rule 2 found no
    // missing UTxOs) to avoid confusing secondary errors.  We also skip
    // when there are already errors that would prevent meaningful datum
    // checks (e.g. Rule 2 failures leave UTxOs unresolvable).
    //
    // Two sub-checks mirror Haskell's Alonzo UTXOW rules:
    //   - missingRequiredDatums:          script-locked inputs with DatumHash
    //     but no matching witness datum → MissingDatumWitness
    //   - notAllowedSupplementalDatums:   witness datums not needed by any
    //     input or output → ExtraDatumWitness
    // ------------------------------------------------------------------
    if errors.is_empty() {
        super::datum::check_datum_witnesses(tx, utxo_set, errors);
    }

    // ------------------------------------------------------------------
    // Rule 10: Required signers must have corresponding vkey witnesses
    // ------------------------------------------------------------------
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

    // ------------------------------------------------------------------
    // Rule 13: Native script evaluation
    // ------------------------------------------------------------------
    if !tx.witness_set.native_scripts.is_empty() {
        let signers: HashSet<torsten_primitives::hash::Hash32> = tx
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

    // ------------------------------------------------------------------
    // Rule 14: Witness signature verification
    // ------------------------------------------------------------------
    if errors.is_empty() {
        let tx_hash_bytes = tx.hash.as_bytes();

        errors.extend(verify_witness_signatures(
            &tx.witness_set.vkey_witnesses,
            tx_hash_bytes,
            "",
        ));
        errors.extend(verify_witness_signatures(
            &tx.witness_set.bootstrap_witnesses,
            tx_hash_bytes,
            "bootstrap:",
        ));
    }
}
