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
use torsten_primitives::transaction::{Certificate, Transaction};

use crate::utxo::UtxoLookup;

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
pub(super) fn has_multi_assets_in_tx(tx: &Transaction, utxo_set: &dyn UtxoLookup) -> bool {
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
#[allow(clippy::too_many_arguments)] // validation entry point needs full context
pub(super) fn run_phase1_rules(
    tx: &Transaction,
    utxo_set: &dyn UtxoLookup,
    params: &ProtocolParameters,
    current_slot: u64,
    tx_size: u64,
    registered_pools: Option<&std::collections::HashSet<Hash28>>,
    current_epoch: Option<u64>,
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
    // Rule 1e: Pool retirement epoch <= current_epoch + e_max
    //
    // Per Haskell's POOL rule (Shelley spec, Figure 14): the announced
    // retirement epoch must not exceed `cepoch + emax`. Skipped when
    // `current_epoch` is not provided (e.g. mempool admission without
    // epoch context).
    // ------------------------------------------------------------------
    if let Some(epoch) = current_epoch {
        for cert in &body.certificates {
            if let Certificate::PoolRetirement {
                epoch: retirement_epoch,
                ..
            } = cert
            {
                let max_epoch = epoch.saturating_add(params.e_max);
                if *retirement_epoch > max_epoch {
                    errors.push(ValidationError::PoolRetirementTooLate {
                        retirement_epoch: *retirement_epoch,
                        current_epoch: epoch,
                        e_max: params.e_max,
                        max_epoch,
                    });
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Rule 1f: Conway stake registration deposit must match key_deposit
    //
    // Per Haskell's Conway DELEG rule: `ConwayStakeRegistration` carries
    // an inline deposit amount that must equal `keyDeposit` from the
    // current protocol parameters.
    // ------------------------------------------------------------------
    if params.protocol_version_major >= 9 {
        for cert in &body.certificates {
            if let Certificate::ConwayStakeRegistration { deposit, .. } = cert {
                if deposit.0 != params.key_deposit.0 {
                    errors.push(ValidationError::StakeRegistrationDepositMismatch {
                        declared: deposit.0,
                        expected: params.key_deposit.0,
                    });
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Rule 1g: Conway stake deregistration refund must match key_deposit
    //
    // Per Haskell's Conway DELEG rule (`conwayStakeDeregDeposit`):
    // `ConwayStakeDeregistration` (UnRegCert, certificate tag 8) carries an
    // explicit refund amount that must equal the current `keyDeposit`
    // protocol parameter. The refund field exists so that a transaction
    // built against old parameters is rejected if `keyDeposit` has since
    // changed via a governance update — preventing silent under-refunds.
    //
    // This check applies only in Conway (protocol >= 9) where the new
    // certificate tag is used.  Pre-Conway `StakeDeregistration` (tag 1)
    // implicitly refunds `key_deposit` without carrying an explicit amount.
    // ------------------------------------------------------------------
    if params.protocol_version_major >= 9 {
        for cert in &body.certificates {
            if let Certificate::ConwayStakeDeregistration { refund, .. } = cert {
                if refund.0 != params.key_deposit.0 {
                    errors.push(ValidationError::StakeDeregistrationRefundMismatch {
                        declared: refund.0,
                        expected: params.key_deposit.0,
                    });
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Rule 1h: Pool cost must meet minimum pool cost (Haskell `StakePoolCostTooLowPOOL`)
    //
    // Per Haskell's POOL rule (Shelley spec, Figure 14): every pool registration
    // certificate must declare a cost >= `minPoolCost` from the current protocol
    // parameters.  This check applies to all pool registrations regardless of
    // whether the pool is new or re-registering.
    //
    // Reference: Haskell `StakePoolCostTooLowPOOL` in
    // `cardano-ledger-shelley:Cardano.Ledger.Shelley.Rules.Pool`.
    // ------------------------------------------------------------------
    for cert in &body.certificates {
        if let Certificate::PoolRegistration(pool_params) = cert {
            if pool_params.cost.0 < params.min_pool_cost.0 {
                errors.push(ValidationError::StakePoolCostTooLow {
                    actual: pool_params.cost.0,
                    minimum: params.min_pool_cost.0,
                });
            }
        }
    }

    // ------------------------------------------------------------------
    // Rule 1i: Pool reward account network must match transaction network_id
    //          (Haskell `WrongNetworkInTxBody`, Alonzo+)
    //
    // When the transaction body declares a `network_id` (Alonzo and later),
    // every pool registration certificate's reward account must be on the
    // same network. The network is encoded in bit 0 of the reward account
    // header byte: 0 = testnet, 1 = mainnet.
    //
    // This mirrors Rule 5b (output address network check) but applies to the
    // pool reward account embedded in the certificate. A pool that registers
    // with a testnet reward account on mainnet would allow its operator
    // rewards to be sent to the wrong network, so this is a correctness check.
    //
    // Reference: Haskell `WrongNetworkInTxBody` in
    // `cardano-ledger-alonzo:Cardano.Ledger.Alonzo.Rules.Utxo`.
    // ------------------------------------------------------------------
    if let Some(tx_network_id) = body.network_id {
        let expected_network = if tx_network_id == 0 {
            torsten_primitives::network::NetworkId::Testnet
        } else {
            torsten_primitives::network::NetworkId::Mainnet
        };
        for cert in &body.certificates {
            if let Certificate::PoolRegistration(pool_params) = cert {
                // Reward account format: header_byte || 28-byte credential hash.
                // Bit 0 of the header encodes the network: 0 = testnet, 1 = mainnet.
                if let Some(header) = pool_params.reward_account.first() {
                    let network_bit = header & 0x01;
                    let actual_network = if network_bit == 0 {
                        torsten_primitives::network::NetworkId::Testnet
                    } else {
                        torsten_primitives::network::NetworkId::Mainnet
                    };
                    if actual_network != expected_network {
                        errors.push(ValidationError::PoolRewardAccountWrongNetwork {
                            expected: expected_network,
                            actual: actual_network,
                        });
                        // Report once per transaction — multiple pools with wrong
                        // network are caught by the same error.
                        break;
                    }
                }
            }
        }
    }

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
    // Per Cardano spec (Haskell `validateFailedScripts`), only evaluate
    // native scripts whose hashes appear in `scriptsNeeded(tx)`. Extra
    // scripts in the witness set are allowed but should not be evaluated.
    if !tx.witness_set.native_scripts.is_empty() {
        // Collect the set of script hashes that are actually needed by
        // the transaction: script-locked spending inputs, minting policy
        // IDs, script-locked withdrawals, and script-locked certificates.
        let mut scripts_needed: HashSet<Hash28> = HashSet::new();

        // 1. Script-locked spending inputs (address type bit 4 set)
        for input in &body.inputs {
            if let Some(utxo) = utxo_set.lookup(input) {
                let ab = utxo.address.to_bytes();
                if !ab.is_empty() {
                    let t = (ab[0] >> 4) & 0x0F;
                    // Script address types: 1,3,5,7 (bit 4 of header = 1)
                    if matches!(t, 1 | 3 | 5 | 7) && ab.len() >= 29 {
                        if let Ok(h) = Hash28::try_from(&ab[1..29]) {
                            scripts_needed.insert(h);
                        }
                    }
                }
            }
        }

        // 2. Minting policy IDs
        for policy_id in body.mint.keys() {
            scripts_needed.insert(*policy_id);
        }

        // 3. Script-locked withdrawals (reward address with script bit set)
        for reward_addr in body.withdrawals.keys() {
            if reward_addr.len() >= 29 {
                let header = reward_addr[0];
                // Reward address type: 0xF0/0xF1 — bit 4 = script
                if (header & 0x10) != 0 {
                    if let Ok(h) = Hash28::try_from(&reward_addr[1..29]) {
                        scripts_needed.insert(h);
                    }
                }
            }
        }

        // 4. Certificates with script credentials
        for cert in &body.certificates {
            let cred: Option<&Credential> = match cert {
                Certificate::StakeDeregistration(c) => Some(c),
                Certificate::StakeDelegation { credential: c, .. } => Some(c),
                Certificate::ConwayStakeRegistration { credential: c, .. } => Some(c),
                Certificate::ConwayStakeDeregistration { credential: c, .. } => Some(c),
                Certificate::VoteDelegation { credential: c, .. } => Some(c),
                Certificate::StakeVoteDelegation { credential: c, .. } => Some(c),
                Certificate::RegStakeDeleg { credential: c, .. } => Some(c),
                Certificate::RegStakeVoteDeleg { credential: c, .. } => Some(c),
                Certificate::VoteRegDeleg { credential: c, .. } => Some(c),
                Certificate::CommitteeHotAuth {
                    cold_credential: c, ..
                } => Some(c),
                Certificate::CommitteeColdResign {
                    cold_credential: c, ..
                } => Some(c),
                Certificate::RegDRep { credential: c, .. } => Some(c),
                Certificate::UnregDRep { credential: c, .. } => Some(c),
                Certificate::UpdateDRep { credential: c, .. } => Some(c),
                _ => None,
            };
            if let Some(Credential::Script(h)) = cred {
                scripts_needed.insert(*h);
            }
        }

        // Now evaluate only needed native scripts
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
            // Compute this script's hash: blake2b_224(0x00 || cbor(script))
            let script_cbor = torsten_serialization::encode_native_script(script);
            let mut tagged = Vec::with_capacity(1 + script_cbor.len());
            tagged.push(0x00);
            tagged.extend_from_slice(&script_cbor);
            let script_hash = torsten_primitives::hash::blake2b_224(&tagged);

            // Only evaluate scripts that are actually needed
            if scripts_needed.contains(&script_hash)
                && !evaluate_native_script(script, &signers, slot)
            {
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
