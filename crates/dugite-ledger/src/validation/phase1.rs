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

use std::collections::{HashMap, HashSet};

use dugite_primitives::credentials::Credential;
use dugite_primitives::hash::{Hash28, Hash32};
use dugite_primitives::protocol_params::ProtocolParameters;
use dugite_primitives::time::SlotNo;
use dugite_primitives::transaction::{Certificate, Transaction};

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
// Helper: required VKey witnesses for a certificate (conwayWitsVKeyNeeded)
// ---------------------------------------------------------------------------

/// Return the set of VKey hashes that must have corresponding witnesses for a
/// given certificate.  Script credentials return an empty set — their validation
/// is handled separately via native-script evaluation (Phase-1, Rule 13) or
/// Plutus redeemer matching (Phase-2).
///
/// Matches the Haskell `conwayWitsVKeyNeeded` / `shelleyWitsVKeyNeeded`
/// specification:
///
/// | Certificate                     | Required Witness                        |
/// |---------------------------------|-----------------------------------------|
/// | `PoolRegistration`              | All pool owner VKey hashes              |
/// | `PoolRetirement`                | Pool operator (cold) key hash           |
/// | `StakeDelegation`               | Delegator credential key hash           |
/// | `StakeDeregistration`           | Credential key hash                     |
/// | `ConwayStakeRegistration`       | Registrant credential key hash          |
/// | `ConwayStakeDeregistration`     | Credential key hash                     |
/// | `VoteDelegation`                | Delegator credential key hash           |
/// | `StakeVoteDelegation`           | Delegator credential key hash           |
/// | `RegStakeDeleg`                 | Registrant credential key hash          |
/// | `RegStakeVoteDeleg`             | Registrant credential key hash          |
/// | `VoteRegDeleg`                  | Registrant credential key hash          |
/// | `RegDRep`                       | DRep credential key hash                |
/// | `UnregDRep`                     | DRep credential key hash                |
/// | `UpdateDRep`                    | DRep credential key hash                |
/// | `CommitteeHotAuth`              | Cold credential key hash                |
/// | `CommitteeColdResign`           | Cold credential key hash                |
/// | `StakeRegistration` (Shelley)   | None (free registration)                |
/// | `GenesisKeyDelegation`          | None (legacy)                           |
/// | `MoveInstantaneousRewards`      | None (legacy)                           |
fn cert_required_witnesses(cert: &Certificate) -> Vec<Hash28> {
    // Helper: extract the key hash from a credential, returning None for scripts.
    let key_hash = |c: &Credential| -> Option<Hash28> {
        match c {
            Credential::VerificationKey(h) => Some(*h),
            Credential::Script(_) => None,
        }
    };

    match cert {
        // Pool registration: ALL owner key hashes must sign.
        Certificate::PoolRegistration(params) => params.pool_owners.clone(),

        // Pool retirement: the operator (cold key / pool_id) must sign.
        Certificate::PoolRetirement { pool_hash, .. } => vec![*pool_hash],

        // DRep certificates: credential key hash.
        Certificate::RegDRep { credential, .. }
        | Certificate::UnregDRep { credential, .. }
        | Certificate::UpdateDRep { credential, .. } => key_hash(credential).into_iter().collect(),

        // Committee certificates: cold credential key hash.
        Certificate::CommitteeHotAuth {
            cold_credential, ..
        }
        | Certificate::CommitteeColdResign {
            cold_credential, ..
        } => key_hash(cold_credential).into_iter().collect(),

        // Delegation and deregistration certificates with credential field.
        Certificate::StakeDelegation { credential, .. }
        | Certificate::VoteDelegation { credential, .. }
        | Certificate::StakeVoteDelegation { credential, .. }
        | Certificate::RegStakeDeleg { credential, .. }
        | Certificate::RegStakeVoteDeleg { credential, .. }
        | Certificate::VoteRegDeleg { credential, .. }
        | Certificate::StakeDeregistration(credential)
        | Certificate::ConwayStakeRegistration { credential, .. }
        | Certificate::ConwayStakeDeregistration { credential, .. } => {
            key_hash(credential).into_iter().collect()
        }

        // Shelley stake registration (cert tag 0) — no witness required.
        Certificate::StakeRegistration(_) => vec![],

        // Legacy certificates — no witness checks.
        Certificate::GenesisKeyDelegation { .. } | Certificate::MoveInstantaneousRewards { .. } => {
            vec![]
        }
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

impl HasWitnessFields for dugite_primitives::transaction::VKeyWitness {
    fn vkey(&self) -> &[u8] {
        &self.vkey
    }
    fn signature(&self) -> &[u8] {
        &self.signature
    }
}

impl HasWitnessFields for dugite_primitives::transaction::BootstrapWitness {
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
    match dugite_crypto::keys::PaymentVerificationKey::from_bytes(vkey) {
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
    node_network: Option<dugite_primitives::network::NetworkId>,
    stake_key_deposits: Option<&HashMap<Hash32, u64>>,
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
    //
    // Sub-rule 1c.i: presence/absence consistency.
    // Sub-rule 1c.ii: when both are present, verify the content hash.
    //   The declared hash must equal blake2b_256(raw_aux_cbor).
    //   We can only verify this when raw_cbor bytes were preserved from
    //   the wire (set by the serialization layer); locally-constructed
    //   transactions with raw_cbor=None skip the content check.
    // ------------------------------------------------------------------
    match (&body.auxiliary_data_hash, &tx.auxiliary_data) {
        (Some(_), None) => {
            errors.push(ValidationError::AuxiliaryDataHashWithoutData);
        }
        (None, Some(_)) => {
            errors.push(ValidationError::AuxiliaryDataWithoutHash);
        }
        (Some(declared_hash), Some(aux_data)) => {
            // Content-hash verification: only when raw CBOR bytes are available.
            if let Some(ref raw_cbor) = aux_data.raw_cbor {
                let computed = dugite_primitives::hash::blake2b_256(raw_cbor);
                if computed != *declared_hash {
                    errors.push(ValidationError::AuxiliaryDataHashMismatch);
                }
            }
        }
        (None, None) => {} // Both absent — OK
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
    // Rule 1g: Conway stake deregistration refund must match stored deposit
    //
    // Per Haskell's Conway DELEG rule (`conwayStakeDeregDeposit`):
    // `ConwayStakeDeregistration` (UnRegCert, certificate tag 8) carries an
    // explicit refund amount that must equal the deposit paid at registration
    // time (stored per-credential in `stake_key_deposits`). This ensures
    // correct refunds even if `keyDeposit` has changed via governance.
    //
    // Falls back to the current `keyDeposit` parameter when the per-credential
    // deposit map is not available or the credential is not found (e.g. old
    // snapshots before per-credential tracking was added).
    //
    // This check applies only in Conway (protocol >= 9) where the new
    // certificate tag is used.  Pre-Conway `StakeDeregistration` (tag 1)
    // implicitly refunds `key_deposit` without carrying an explicit amount.
    // ------------------------------------------------------------------
    if params.protocol_version_major >= 9 {
        for cert in &body.certificates {
            if let Certificate::ConwayStakeDeregistration { credential, refund } = cert {
                let key = credential.to_typed_hash32();
                let expected = stake_key_deposits
                    .and_then(|m| m.get(&key).copied())
                    .unwrap_or(params.key_deposit.0);
                if refund.0 != expected {
                    errors.push(ValidationError::StakeDeregistrationRefundMismatch {
                        declared: refund.0,
                        expected,
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
            dugite_primitives::network::NetworkId::Testnet
        } else {
            dugite_primitives::network::NetworkId::Mainnet
        };
        for cert in &body.certificates {
            if let Certificate::PoolRegistration(pool_params) = cert {
                // Reward account format: header_byte || 28-byte credential hash.
                // Bit 0 of the header encodes the network: 0 = testnet, 1 = mainnet.
                if let Some(header) = pool_params.reward_account.first() {
                    let network_bit = header & 0x01;
                    let actual_network = if network_bit == 0 {
                        dugite_primitives::network::NetworkId::Testnet
                    } else {
                        dugite_primitives::network::NetworkId::Mainnet
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
    // Per-proposal deposit validation (Haskell `ProposalDepositIncorrect`)
    //
    // Each governance proposal's inline deposit must exactly match the current
    // `gov_action_deposit` protocol parameter. Conway+ only.
    //
    // Reference: Haskell `ProposalDepositIncorrect` in
    // `cardano-ledger-conway:Cardano.Ledger.Conway.Rules.Gov`.
    // ------------------------------------------------------------------
    if params.protocol_version_major >= 9 {
        for proposal in &body.proposal_procedures {
            if proposal.deposit != params.gov_action_deposit {
                errors.push(ValidationError::ProposalDepositIncorrect {
                    declared: proposal.deposit.0,
                    expected: params.gov_action_deposit.0,
                });
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
            stake_key_deposits,
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
        use dugite_primitives::hash::PolicyId;
        use dugite_primitives::value::AssetName;
        use std::collections::BTreeMap;

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
            let script_cbor = dugite_serialization::encode_native_script(script);
            let mut tagged = Vec::with_capacity(1 + script_cbor.len());
            tagged.push(0x00);
            tagged.extend_from_slice(&script_cbor);
            available_script_hashes.insert(dugite_primitives::hash::blake2b_224(&tagged));
        }
        for s in &tx.witness_set.plutus_v1_scripts {
            let mut tagged = Vec::with_capacity(1 + s.len());
            tagged.push(0x01);
            tagged.extend_from_slice(s);
            available_script_hashes.insert(dugite_primitives::hash::blake2b_224(&tagged));
        }
        for s in &tx.witness_set.plutus_v2_scripts {
            let mut tagged = Vec::with_capacity(1 + s.len());
            tagged.push(0x02);
            tagged.extend_from_slice(s);
            available_script_hashes.insert(dugite_primitives::hash::blake2b_224(&tagged));
        }
        for s in &tx.witness_set.plutus_v3_scripts {
            let mut tagged = Vec::with_capacity(1 + s.len());
            tagged.push(0x03);
            tagged.extend_from_slice(s);
            available_script_hashes.insert(dugite_primitives::hash::blake2b_224(&tagged));
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
            dugite_primitives::network::NetworkId::Testnet
        } else {
            dugite_primitives::network::NetworkId::Mainnet
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
    // Rule 5c: Unconditional output address network check
    //
    // Unlike Rule 5b (which fires only when `tx.body.network_id` is set),
    // this check applies unconditionally using the node's configured network
    // (Haskell's `Globals.networkId`).  Every output address with a parseable
    // network tag must be on the node's network.
    //
    // Only enforced when `node_network` is provided. Addresses that return
    // `None` from `network_id()` (e.g. Byron addresses) are accepted without
    // a network check (they carry no explicit network tag).
    //
    // Reference: Haskell `WrongNetwork` predicate in
    // `cardano-ledger-shelley:Cardano.Ledger.Shelley.Rules.Utxo`.
    // ------------------------------------------------------------------
    if let Some(expected_net) = node_network {
        for output in &body.outputs {
            if let Some(addr_network) = output.address.network_id() {
                if addr_network != expected_net {
                    errors.push(ValidationError::WrongNetworkInOutput {
                        expected: expected_net,
                        actual: addr_network,
                    });
                    // Report once per transaction to avoid flooding.
                    break;
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Rule 5d: Unconditional withdrawal reward address network check
    //
    // Every withdrawal reward address must be on the node's configured
    // network (Haskell's `Globals.networkId`).
    // Bit 0 of the reward account header encodes the network:
    //   0 = testnet, 1 = mainnet.
    //
    // Reference: Haskell `WrongNetworkWithdrawal` in
    // `cardano-ledger-shelley:Cardano.Ledger.Shelley.Rules.Utxow`.
    // ------------------------------------------------------------------
    if let Some(expected_net) = node_network {
        for reward_account in body.withdrawals.keys() {
            if let Some(header) = reward_account.first() {
                let network_bit = header & 0x01;
                let actual_net = if network_bit == 0 {
                    dugite_primitives::network::NetworkId::Testnet
                } else {
                    dugite_primitives::network::NetworkId::Mainnet
                };
                if actual_net != expected_net {
                    errors.push(ValidationError::WrongNetworkWithdrawal {
                        expected: expected_net,
                        actual: actual_net,
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
            .map(|w| dugite_primitives::hash::blake2b_224(&w.vkey))
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

        // Check each certificate has matching witnesses for required credentials.
        // Mirrors Haskell's conwayWitsVKeyNeeded which unions certificate witness
        // requirements with input/withdrawal witness requirements.
        for cert in &body.certificates {
            for required_keyhash in cert_required_witnesses(cert) {
                if !vkey_witness_hashes.contains(&required_keyhash) {
                    errors.push(ValidationError::MissingCertificateWitness(
                        required_keyhash.to_hex(),
                    ));
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
        let script_versions = super::collateral::plutus_script_version_map(tx, utxo_set);
        super::datum::check_datum_witnesses(tx, utxo_set, &script_versions, errors);
    }

    // ------------------------------------------------------------------
    // Rule 10: Required signers must have corresponding vkey witnesses
    // ------------------------------------------------------------------
    if !body.required_signers.is_empty() && !tx.witness_set.vkey_witnesses.is_empty() {
        let witness_keyhashes: HashSet<_> = tx
            .witness_set
            .vkey_witnesses
            .iter()
            .map(|w| dugite_primitives::hash::blake2b_224(&w.vkey))
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
        let signers: HashSet<dugite_primitives::hash::Hash32> = tx
            .witness_set
            .vkey_witnesses
            .iter()
            .map(|w| {
                // Hash the vkey to get the 28-byte key hash, then pad to Hash32
                dugite_primitives::hash::blake2b_224(&w.vkey).to_hash32_padded()
            })
            .collect();
        let slot = SlotNo(current_slot);

        for script in &tx.witness_set.native_scripts {
            // Compute this script's hash: blake2b_224(0x00 || cbor(script))
            let script_cbor = dugite_serialization::encode_native_script(script);
            let mut tagged = Vec::with_capacity(1 + script_cbor.len());
            tagged.push(0x00);
            tagged.extend_from_slice(&script_cbor);
            let script_hash = dugite_primitives::hash::blake2b_224(&tagged);

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

// ---------------------------------------------------------------------------
// Inline unit tests for Phase-1 validation rules
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use dugite_primitives::address::{Address, EnterpriseAddress};
    use dugite_primitives::credentials::Credential;
    use dugite_primitives::hash::{Hash28, Hash32};
    use dugite_primitives::network::NetworkId;
    use dugite_primitives::protocol_params::ProtocolParameters;
    use dugite_primitives::time::SlotNo;
    use dugite_primitives::transaction::{
        ExUnits, OutputDatum, Redeemer, RedeemerTag, Transaction, TransactionBody,
        TransactionInput, TransactionOutput, TransactionWitnessSet, VKeyWitness,
    };
    use dugite_primitives::value::{AssetName, Lovelace, Value};

    use crate::utxo::UtxoSet;
    use crate::validation::{
        validate_transaction, validate_transaction_with_pools, ValidationError,
    };

    // -----------------------------------------------------------------------
    // Test fixture: a minimal valid Conway transaction
    //
    // UTxO:   1 input  → 10_000_000 lovelace
    // Output: 1 output →  9_800_000 lovelace
    // Fee:                  200_000 lovelace
    // -----------------------------------------------------------------------

    /// Build a UTxO set with one entry worth 10M lovelace, plus the corresponding
    /// [`TransactionInput`] that spends it.  The UTxO output uses a Byron address
    /// so that Phase-1 witness-completeness checks (Rule 9b) are satisfied
    /// without requiring a vkey witness.
    fn make_valid_tx() -> (UtxoSet, Transaction, TransactionInput) {
        let mut utxo_set = UtxoSet::new();

        // Use a Byron payload so Rule 9b requires no witness for this input.
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0xAAu8; 32]),
            index: 0,
        };
        let utxo_output = TransactionOutput {
            address: Address::Byron(dugite_primitives::address::ByronAddress {
                payload: vec![0x82, 0x00, 0x01],
            }),
            value: Value::lovelace(10_000_000),
            datum: OutputDatum::None,
            script_ref: None,
            is_legacy: false,
            raw_cbor: None,
        };
        utxo_set.insert(input.clone(), utxo_output);

        let tx = Transaction {
            era: dugite_primitives::era::Era::Conway,
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![input.clone()],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(dugite_primitives::address::ByronAddress {
                        payload: vec![0x82, 0x00, 0x01],
                    }),
                    value: Value::lovelace(9_800_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    is_legacy: false,
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

        (utxo_set, tx, input)
    }

    // -----------------------------------------------------------------------
    // Test 1 — baseline: valid transaction passes all Phase-1 rules
    // -----------------------------------------------------------------------
    #[test]
    fn test_valid_tx_passes() {
        let (utxo_set, tx, _) = make_valid_tx();
        let params = ProtocolParameters::mainnet_defaults();
        let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
        assert!(result.is_ok(), "expected Ok(()), got {result:?}");
    }

    // -----------------------------------------------------------------------
    // Test 2 — Rule 1: no inputs
    // -----------------------------------------------------------------------
    #[test]
    fn test_no_inputs() {
        let (utxo_set, mut tx, _) = make_valid_tx();
        tx.body.inputs.clear();
        let params = ProtocolParameters::mainnet_defaults();
        let errors = validate_transaction(&tx, &utxo_set, &params, 100, 300, None).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::NoInputs)),
            "expected NoInputs, got {errors:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 3 — Rule 2: input references a UTxO entry that does not exist
    // -----------------------------------------------------------------------
    #[test]
    fn test_all_inputs_must_exist() {
        let (utxo_set, mut tx, _) = make_valid_tx();
        // Overwrite the tx_id so it no longer matches the UTxO.
        tx.body.inputs[0].transaction_id = Hash32::from_bytes([0xBBu8; 32]);
        let params = ProtocolParameters::mainnet_defaults();
        let errors = validate_transaction(&tx, &utxo_set, &params, 100, 300, None).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::InputNotFound(_))),
            "expected InputNotFound, got {errors:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 4 — Rule 3: ADA value not conserved (output + fee > input)
    // -----------------------------------------------------------------------
    #[test]
    fn test_value_not_conserved_ada() {
        let (utxo_set, mut tx, _) = make_valid_tx();
        // Inflate the output so that output(10_000_000) + fee(200_000) > input(10_000_000).
        tx.body.outputs[0].value = Value::lovelace(10_000_000);
        let params = ProtocolParameters::mainnet_defaults();
        let errors = validate_transaction(&tx, &utxo_set, &params, 100, 300, None).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::ValueNotConserved { .. })),
            "expected ValueNotConserved, got {errors:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 5 — Rule 3b: multi-asset not conserved (minted tokens not in outputs)
    // -----------------------------------------------------------------------
    #[test]
    fn test_value_not_conserved_multiasset() {
        let (utxo_set, mut tx, _) = make_valid_tx();
        let policy = Hash28::from_bytes([0x11u8; 28]);
        let asset = AssetName::new(b"COIN".to_vec()).unwrap();
        // Mint 100 tokens but produce no multi-asset output.
        tx.body.mint.entry(policy).or_default().insert(asset, 100);
        let params = ProtocolParameters::mainnet_defaults();
        let errors = validate_transaction(&tx, &utxo_set, &params, 100, 300, None).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::MultiAssetNotConserved { .. })),
            "expected MultiAssetNotConserved, got {errors:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 6 — Rule 3c: minting policy present but no matching script witness
    // -----------------------------------------------------------------------
    #[test]
    fn test_mint_without_policy_script() {
        let (utxo_set, mut tx, _) = make_valid_tx();
        let policy = Hash28::from_bytes([0x22u8; 28]);
        let asset = AssetName::new(b"TKN".to_vec()).unwrap();
        // Mint and output the same amount so Rule 3b passes, but no script
        // witness is provided so Rule 3c fires.
        tx.body
            .mint
            .entry(policy)
            .or_default()
            .insert(asset.clone(), 50);
        // Mirror the minted tokens in the output so value is conserved.
        tx.body.outputs[0]
            .value
            .multi_asset
            .entry(policy)
            .or_default()
            .insert(asset, 50);
        let params = ProtocolParameters::mainnet_defaults();
        // The validation should fail with a script-related error.
        let errors = validate_transaction(&tx, &utxo_set, &params, 100, 300, None).unwrap_err();
        assert!(
            errors.iter().any(|e| matches!(
                e,
                ValidationError::InvalidMint | ValidationError::MissingScriptWitness(_)
            )),
            "expected a script-related minting error, got {errors:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 7 — Rule 4: declared fee is below the computed minimum fee
    // -----------------------------------------------------------------------
    #[test]
    fn test_fee_too_small() {
        let (utxo_set, mut tx, _) = make_valid_tx();
        // Move almost all lovelace to the output, leaving only 1 lovelace as fee.
        tx.body.outputs[0].value = Value::lovelace(9_999_999);
        tx.body.fee = Lovelace(1);
        let params = ProtocolParameters::mainnet_defaults();
        let errors = validate_transaction(&tx, &utxo_set, &params, 100, 300, None).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::FeeTooSmall { .. })),
            "expected FeeTooSmall, got {errors:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 8 — Rule 5: output below minimum UTxO value
    // -----------------------------------------------------------------------
    #[test]
    fn test_output_below_min_utxo() {
        let (utxo_set, mut tx, _) = make_valid_tx();
        // Keep value conservation: output(1) + fee(9_999_999) = input(10_000_000).
        tx.body.outputs[0].value = Value::lovelace(1);
        tx.body.fee = Lovelace(9_999_999);
        let params = ProtocolParameters::mainnet_defaults();
        let errors = validate_transaction(&tx, &utxo_set, &params, 100, 300, None).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::OutputTooSmall { .. })),
            "expected OutputTooSmall, got {errors:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 9 — Rule 5a: output value CBOR size exceeds max_val_size
    // -----------------------------------------------------------------------
    #[test]
    fn test_output_value_too_large() {
        let (utxo_set, mut tx, _) = make_valid_tx();
        // Build a multi-asset output with many policies so the estimated CBOR
        // size of the value map exceeds max_val_size (5000 bytes in mainnet
        // defaults).  Each policy+1 asset adds roughly 37 bytes; 140 policies
        // ≈ 5180 bytes which is safely above the 5000 byte limit.
        let mut multi_asset_value = Value::lovelace(9_800_000);
        for i in 0u8..140 {
            let mut policy_bytes = [0u8; 28];
            policy_bytes[0] = i;
            policy_bytes[1] = 0xFF;
            let policy = Hash28::from_bytes(policy_bytes);
            let asset = AssetName::new(vec![i; 4]).unwrap();
            multi_asset_value
                .multi_asset
                .entry(policy)
                .or_default()
                .insert(asset, 1);
        }
        tx.body.outputs[0].value = multi_asset_value;
        let params = ProtocolParameters::mainnet_defaults();
        let errors = validate_transaction(&tx, &utxo_set, &params, 100, 300, None).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::OutputValueTooLarge { .. })),
            "expected OutputValueTooLarge, got {errors:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 10 — Rule 5c: output address on wrong network (testnet addr, mainnet node)
    // -----------------------------------------------------------------------
    #[test]
    fn test_network_id_mismatch() {
        let (utxo_set, mut tx, _) = make_valid_tx();
        // Replace the output address with a testnet enterprise address.
        tx.body.outputs[0].address = Address::Enterprise(EnterpriseAddress {
            network: NetworkId::Testnet,
            payment: Credential::VerificationKey(Hash28::from_bytes([0x33u8; 28])),
        });
        let params = ProtocolParameters::mainnet_defaults();
        // Validate with node_network = Mainnet so Rule 5c fires.
        let errors = validate_transaction_with_pools(
            &tx,
            &utxo_set,
            &params,
            100,
            300,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(NetworkId::Mainnet),
            None,
            None,
            None,
            None, // constitution_script_hash
            None, // vote_delegations
        )
        .unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::WrongNetworkInOutput { .. })),
            "expected WrongNetworkInOutput, got {errors:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 11 — Rule 6: transaction size exceeds max_tx_size
    // -----------------------------------------------------------------------
    #[test]
    fn test_tx_size_too_large() {
        let (utxo_set, tx, _) = make_valid_tx();
        let params = ProtocolParameters::mainnet_defaults();
        // Pass a size that exceeds max_tx_size (16384 in mainnet defaults).
        let too_large = params.max_tx_size + 1;
        let errors =
            validate_transaction(&tx, &utxo_set, &params, 100, too_large, None).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::TxTooLarge { .. })),
            "expected TxTooLarge, got {errors:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 12 — Rule 7: TTL expired
    // -----------------------------------------------------------------------
    #[test]
    fn test_ttl_expired() {
        let (utxo_set, mut tx, _) = make_valid_tx();
        tx.body.ttl = Some(SlotNo(50));
        let params = ProtocolParameters::mainnet_defaults();
        // current_slot(100) > ttl(50) → TtlExpired
        let errors = validate_transaction(&tx, &utxo_set, &params, 100, 300, None).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::TtlExpired { .. })),
            "expected TtlExpired, got {errors:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 13 — Rule 8: validity interval start not yet reached
    // -----------------------------------------------------------------------
    #[test]
    fn test_validity_interval_not_started() {
        let (utxo_set, mut tx, _) = make_valid_tx();
        tx.body.validity_interval_start = Some(SlotNo(200));
        let params = ProtocolParameters::mainnet_defaults();
        // current_slot(100) < validity_start(200) → NotYetValid
        let errors = validate_transaction(&tx, &utxo_set, &params, 100, 300, None).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::NotYetValid { .. })),
            "expected NotYetValid, got {errors:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 14 — Rule 9: reference input overlaps with regular input
    // -----------------------------------------------------------------------
    #[test]
    fn test_ref_inputs_must_be_disjoint() {
        let (utxo_set, mut tx, input) = make_valid_tx();
        // Put the same input in both the spending set and the reference set.
        tx.body.reference_inputs.push(input.clone());
        let params = ProtocolParameters::mainnet_defaults();
        let errors = validate_transaction(&tx, &utxo_set, &params, 100, 300, None).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::ReferenceInputOverlapsInput(_))),
            "expected ReferenceInputOverlapsInput, got {errors:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 15 — Rule 9: reference input does not exist in the UTxO set
    // -----------------------------------------------------------------------
    #[test]
    fn test_ref_inputs_must_exist() {
        let (utxo_set, mut tx, _) = make_valid_tx();
        // Add a reference input that has no UTxO entry.
        tx.body.reference_inputs.push(TransactionInput {
            transaction_id: Hash32::from_bytes([0xCCu8; 32]),
            index: 0,
        });
        let params = ProtocolParameters::mainnet_defaults();
        let errors = validate_transaction(&tx, &utxo_set, &params, 100, 300, None).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::ReferenceInputNotFound(_))),
            "expected ReferenceInputNotFound, got {errors:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 16 — Rule 10: required signer has no matching vkey witness
    // -----------------------------------------------------------------------
    #[test]
    fn test_required_signer_missing() {
        let (utxo_set, mut tx, _) = make_valid_tx();
        // Declare a required signer whose hash is not present in the witness set.
        tx.body
            .required_signers
            .push(Hash32::from_bytes([0xDDu8; 32]));
        let params = ProtocolParameters::mainnet_defaults();
        let errors = validate_transaction(&tx, &utxo_set, &params, 100, 300, None).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::MissingRequiredSigner(_))),
            "expected MissingRequiredSigner, got {errors:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 17 — Rule 1c: auxiliary data hash declared but no auxiliary data body
    // -----------------------------------------------------------------------
    #[test]
    fn test_auxiliary_data_hash_mismatch() {
        let (utxo_set, mut tx, _) = make_valid_tx();
        // Declare a hash but provide no auxiliary data.
        tx.body.auxiliary_data_hash = Some(Hash32::from_bytes([0xEEu8; 32]));
        tx.auxiliary_data = None;
        let params = ProtocolParameters::mainnet_defaults();
        let errors = validate_transaction(&tx, &utxo_set, &params, 100, 300, None).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::AuxiliaryDataHashWithoutData)),
            "expected AuxiliaryDataHashWithoutData, got {errors:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 18 — Conway LEDGER rule: declared treasury value must match ledger
    // -----------------------------------------------------------------------
    #[test]
    fn test_treasury_value_mismatch() {
        let (utxo_set, mut tx, _) = make_valid_tx();
        // Declare 999 lovelace in the tx body, but ledger holds 1000.
        tx.body.treasury_value = Some(Lovelace(999));
        let params = ProtocolParameters::mainnet_defaults();
        // protocol_version_major = 9 in mainnet_defaults() so the check fires.
        let errors = validate_transaction_with_pools(
            &tx,
            &utxo_set,
            &params,
            100,
            300,
            None,
            None,
            Some(1000), // current_treasury
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None, // constitution_script_hash
            None, // vote_delegations
        )
        .unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::TreasuryValueMismatch { .. })),
            "expected TreasuryValueMismatch, got {errors:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 19 — Rule 12: script data hash missing when Plutus scripts/redeemers present
    // -----------------------------------------------------------------------
    #[test]
    fn test_script_integrity_hash_mismatch() {
        let (utxo_set, mut tx, _) = make_valid_tx();
        // Add a dummy PlutusV2 script byte-string and a redeemer so the
        // `has_plutus_scripts` guard fires and `check_script_data_hash` runs.
        // With `script_data_hash = None`, `MissingScriptDataHash` is pushed.
        tx.witness_set.plutus_v2_scripts.push(vec![0x01u8; 10]);
        tx.witness_set.redeemers.push(Redeemer {
            tag: RedeemerTag::Spend,
            index: 0,
            data: dugite_primitives::transaction::PlutusData::Integer(0),
            ex_units: ExUnits { mem: 0, steps: 0 },
        });
        // Deliberately leave script_data_hash as None → MissingScriptDataHash
        tx.body.script_data_hash = None;
        let params = ProtocolParameters::mainnet_defaults();
        let errors = validate_transaction(&tx, &utxo_set, &params, 100, 300, None).unwrap_err();
        assert!(
            errors.iter().any(|e| matches!(
                e,
                ValidationError::MissingScriptDataHash
                    | ValidationError::ScriptDataHashMismatch { .. }
            )),
            "expected MissingScriptDataHash or ScriptDataHashMismatch, got {errors:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 20 — Rule 14: Ed25519 witness with a corrupt signature is rejected
    // -----------------------------------------------------------------------
    #[test]
    fn test_ed25519_signature_verification() {
        let (utxo_set, mut tx, _) = make_valid_tx();
        // Append a vkey witness whose signature bytes are all zeros — this will
        // fail Ed25519 verification (or key parsing) and trigger the error.
        // Using `[1u8; 32]` as the vkey matches the pattern already verified in
        // the existing test suite (validation/tests.rs test_witness_signature_verification).
        tx.witness_set.vkey_witnesses.push(VKeyWitness {
            vkey: vec![1u8; 32],
            signature: vec![0u8; 64],
        });
        let params = ProtocolParameters::mainnet_defaults();
        let errors = validate_transaction(&tx, &utxo_set, &params, 100, 300, None).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::InvalidWitnessSignature(_))),
            "expected InvalidWitnessSignature, got {errors:?}"
        );
    }
}
