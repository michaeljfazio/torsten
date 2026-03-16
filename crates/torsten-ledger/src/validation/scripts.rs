//! Script-related Phase-1 validation helpers.
//!
//! This module provides:
//! - `evaluate_native_script` — recursive native script evaluation
//! - `collect_available_script_hashes` — build the set of script hashes that a
//!   transaction has made available (witness set + reference inputs)
//! - `compute_script_ref_hash` — canonical Blake2b-224 hash for a `ScriptRef`
//! - `check_script_data_hash` — Rule 12: script integrity hash validation
//! - Reference-script size/fee helpers used by the fee rule

use std::collections::{HashMap, HashSet};

use torsten_primitives::hash::{Hash28, Hash32};
use torsten_primitives::protocol_params::ProtocolParameters;
use torsten_primitives::time::SlotNo;
use torsten_primitives::transaction::{
    Certificate, GovAction, NativeScript, ScriptRef, Transaction, TransactionInput, Voter,
};
use torsten_primitives::value::Lovelace;
use tracing::debug;

use crate::utxo::UtxoSet;

use super::ValidationError;

// ---------------------------------------------------------------------------
// Native script evaluation
// ---------------------------------------------------------------------------

/// Evaluate a native script given the set of key hashes that signed
/// the transaction and the current slot validity interval.
///
/// This is the canonical recursive evaluator matching the Cardano ledger
/// specification for native scripts (Shelley multi-sig and Mary timelocks).
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

// ---------------------------------------------------------------------------
// Script hash utilities
// ---------------------------------------------------------------------------

/// Compute the canonical script hash for a reference script.
///
/// Per the Cardano spec, the hash is `blake2b_224(type_tag || script_bytes)`:
/// - `0x00` — native script (with the script CBOR-encoded)
/// - `0x01` — Plutus V1
/// - `0x02` — Plutus V2
/// - `0x03` — Plutus V3
pub(super) fn compute_script_ref_hash(script_ref: &ScriptRef) -> Hash28 {
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

/// Collect all available script hashes from the transaction's witness set and
/// from reference input UTxOs.
///
/// Used for witness completeness checks (Rule 9b) and minting policy checks
/// (Rule 3c).
pub(super) fn collect_available_script_hashes(
    tx: &Transaction,
    utxo_set: &UtxoSet,
) -> HashSet<Hash28> {
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

// ---------------------------------------------------------------------------
// Reference script size + tiered fee (CIP-0112)
// ---------------------------------------------------------------------------

/// Calculate the total byte size of reference scripts used via reference inputs.
pub(super) fn calculate_ref_script_size(
    reference_inputs: &[TransactionInput],
    utxo_set: &UtxoSet,
) -> u64 {
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

/// Return the byte size of a single reference script.
pub(super) fn script_ref_byte_size(script_ref: &ScriptRef) -> u64 {
    match script_ref {
        ScriptRef::NativeScript(ns) => torsten_serialization::encode_native_script(ns).len() as u64,
        ScriptRef::PlutusV1(bytes) | ScriptRef::PlutusV2(bytes) | ScriptRef::PlutusV3(bytes) => {
            bytes.len() as u64
        }
    }
}

/// CIP-0112 tiered reference script fee calculation.
///
/// Divides the total script size into 25 KiB tiers, applying a 1.2× multiplier
/// per tier. The entire accumulation is kept as an exact rational value (using
/// numerator/denominator pairs with GCD reduction) and floored only once at the
/// end — matching Haskell's `tierRefScriptFee` which accumulates `Rational`
/// values before a single `floor`.
pub(super) fn calculate_ref_script_tiered_fee(base_fee_per_byte: u64, total_size: u64) -> u64 {
    const TIER_SIZE: u64 = 25_600; // 25 KiB
    const MULT_NUM: u128 = 6; // 1.2 = 6/5
    const MULT_DEN: u128 = 5;

    let mut remaining = total_size;
    // Accumulator as rational: acc_num / acc_den
    let mut acc_num: u128 = 0;
    let mut acc_den: u128 = 1;
    // Current tier price as rational: price_num / price_den
    let mut price_num: u128 = base_fee_per_byte as u128;
    let mut price_den: u128 = 1;

    while remaining > 0 {
        let chunk = remaining.min(TIER_SIZE);
        // acc += chunk * (price_num / price_den)
        acc_num = acc_num * price_den + chunk as u128 * price_num * acc_den;
        acc_den *= price_den;
        remaining -= chunk;
        // price *= 6/5 (exact rational multiplication)
        price_num *= MULT_NUM;
        price_den *= MULT_DEN;
        // Reduce to prevent overflow
        let g = gcd_u128(price_num, price_den);
        price_num /= g;
        price_den /= g;
        let g2 = gcd_u128(acc_num, acc_den);
        acc_num /= g2;
        acc_den /= g2;
    }
    // floor(acc_num / acc_den) — single floor at the end, matching Haskell
    (acc_num / acc_den) as u64
}

fn gcd_u128(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    if a == 0 {
        1
    } else {
        a
    }
}

// ---------------------------------------------------------------------------
// Output value size estimation (used by Rule 5a)
// ---------------------------------------------------------------------------

/// Estimate the CBOR-encoded size of a `Value`.
///
/// For ADA-only values this is just the CBOR integer encoding. For multi-asset
/// values this estimates the `[coin, multiasset_map]` array encoding.
pub(super) fn estimate_value_cbor_size(value: &torsten_primitives::value::Value) -> u64 {
    if value.multi_asset.is_empty() {
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

/// Estimate the CBOR encoding size of an unsigned integer.
pub(super) fn cbor_uint_size(value: u64) -> u64 {
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

// ---------------------------------------------------------------------------
// Script data hash (Rule 12)
// ---------------------------------------------------------------------------

/// Check the script integrity hash (Rule 12).
///
/// If the transaction has redeemers or Plutus data, `script_data_hash` must be
/// set and match the computed value. If neither is present but
/// `script_data_hash` is set, it is only valid when reference inputs carry
/// reference scripts (otherwise `UnexpectedScriptDataHash`).
///
/// On success the function also returns whether Phase-2 Plutus evaluation is
/// needed (i.e. `has_redeemers`).
pub(super) fn check_script_data_hash(
    tx: &Transaction,
    utxo_set: &UtxoSet,
    params: &ProtocolParameters,
    errors: &mut Vec<ValidationError>,
) {
    let body = &tx.body;
    let has_redeemers = !tx.witness_set.redeemers.is_empty();
    let has_datums = !tx.witness_set.plutus_data.is_empty();

    if has_redeemers || has_datums {
        if let Some(declared_hash) = &body.script_data_hash {
            // Determine which Plutus language versions are used.
            // Per Haskell mkScriptIntegrity: intersect scriptsProvided with
            // scriptsNeeded to determine the set of language versions that
            // contribute to the hash.
            let mut has_v1 = !tx.witness_set.plutus_v1_scripts.is_empty();
            let mut has_v2 = !tx.witness_set.plutus_v2_scripts.is_empty();
            let mut has_v3 = !tx.witness_set.plutus_v3_scripts.is_empty();

            // 1. Collect needed script hashes (spending inputs, minting, withdrawals, certs, votes)
            let mut scripts_needed: HashSet<Hash28> = HashSet::new();
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
            for policy_id in body.mint.keys() {
                scripts_needed.insert(*policy_id);
            }
            for reward_addr in body.withdrawals.keys() {
                if reward_addr.len() >= 29 {
                    let header = reward_addr[0];
                    // Reward address type: 0xF0/0xF1 = script
                    if (header & 0x10) != 0 {
                        if let Ok(h) = Hash28::try_from(&reward_addr[1..29]) {
                            scripts_needed.insert(h);
                        }
                    }
                }
            }
            // Certificates with script credentials
            use torsten_primitives::credentials::Credential as Cred;
            for cert in &body.certificates {
                let cred: Option<&Cred> = match cert {
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
                if let Some(Cred::Script(h)) = cred {
                    scripts_needed.insert(*h);
                }
            }
            // Voting procedures: DRep and CC voter script credentials
            for voter in body.voting_procedures.keys() {
                let cred: Option<&Cred> = match voter {
                    Voter::DRep(c) => Some(c),
                    Voter::ConstitutionalCommittee(c) => Some(c),
                    Voter::StakePool(_) => None,
                };
                if let Some(Cred::Script(h)) = cred {
                    scripts_needed.insert(*h);
                }
            }
            // Proposal procedures: guardrail script hashes
            for proposal in &body.proposal_procedures {
                match &proposal.gov_action {
                    GovAction::ParameterChange {
                        policy_hash: Some(h),
                        ..
                    }
                    | GovAction::TreasuryWithdrawals {
                        policy_hash: Some(h),
                        ..
                    } => {
                        scripts_needed.insert(*h);
                    }
                    _ => {}
                }
            }

            // 2. Collect provided scripts with their version tag
            let mut scripts_provided: HashMap<Hash28, u8> = HashMap::new();
            for s in &tx.witness_set.plutus_v1_scripts {
                let h = torsten_primitives::hash::blake2b_224_tagged(1, s);
                scripts_provided.insert(h, 1);
            }
            for s in &tx.witness_set.plutus_v2_scripts {
                let h = torsten_primitives::hash::blake2b_224_tagged(2, s);
                scripts_provided.insert(h, 2);
            }
            for s in &tx.witness_set.plutus_v3_scripts {
                let h = torsten_primitives::hash::blake2b_224_tagged(3, s);
                scripts_provided.insert(h, 3);
            }
            for input in body.inputs.iter().chain(body.reference_inputs.iter()) {
                if let Some(utxo) = utxo_set.lookup(input) {
                    let (tag, bytes) = match &utxo.script_ref {
                        Some(ScriptRef::PlutusV1(s)) => (1u8, s.as_slice()),
                        Some(ScriptRef::PlutusV2(s)) => (2, s.as_slice()),
                        Some(ScriptRef::PlutusV3(s)) => (3, s.as_slice()),
                        _ => continue,
                    };
                    let h = torsten_primitives::hash::blake2b_224_tagged(tag, bytes);
                    scripts_provided.insert(h, tag);
                }
            }

            // 3. Intersect: only USED scripts determine the language set
            for (hash, version) in &scripts_provided {
                if scripts_needed.contains(hash) {
                    match version {
                        1 => has_v1 = true,
                        2 => has_v2 = true,
                        3 => has_v3 = true,
                        _ => {}
                    }
                }
            }

            // Debug log when the hash-based intersection found no languages
            // despite having redeemers and non-empty scriptsNeeded/scriptsProvided.
            // The unconditional reference-input scan below will still run.
            if !has_v1 && !has_v2 && !has_v3 && has_redeemers && !scripts_needed.is_empty() {
                debug!(
                    needed_count = scripts_needed.len(),
                    provided_count = scripts_provided.len(),
                    needed = ?scripts_needed.iter().map(|h| h.to_hex()).collect::<Vec<_>>(),
                    provided = ?scripts_provided.keys().map(|h| h.to_hex()).collect::<Vec<_>>(),
                    "scriptsNeeded/Provided intersection empty after hash matching"
                );
            }

            // Always supplement language detection by scanning reference inputs
            // directly for their Plutus script version. This runs unconditionally
            // (not only when no languages were detected) to handle two cases:
            //
            //   (a) Mixed-language transactions — e.g. V1 witness scripts set
            //       `has_v1 = true` above, but a V2 reference-only script is
            //       also needed. The previous fallback guard (`!has_v1`) would
            //       prevent it from firing, leaving `has_v2 = false` and
            //       causing a ScriptDataHashMismatch (issue #82).
            //
            //   (b) Hash computation divergence — the reference script's
            //       computed hash may not appear in `scripts_needed` when our
            //       hash computation (tag||bytes) differs from the on-chain
            //       address credential hash (e.g. double-CBOR-encoding of
            //       script bytes in some eras), so the intersection at step 3
            //       misses it entirely.
            //
            // Only `body.reference_inputs` are scanned (not spending inputs):
            // spending-input UTxOs carrying a script_ref are spent for their
            // ADA value, not for their script, and must NOT contribute their
            // language to the views. Reference inputs are the only inputs whose
            // script_ref is explicitly made available to the transaction for
            // script execution. This matches the Haskell `refScripts` function
            // which restricts to `referenceInputs` when building the available
            // script set for `mkScriptIntegrity`.
            //
            // Safety: if a reference script contributes its language here but
            // is NOT actually needed by this transaction, the script_data_hash
            // declared in the tx body will still not match our computed value
            // (since the tx builder would have used the correct language set),
            // and validation will correctly reject the tx with a mismatch.
            for ref_input in &body.reference_inputs {
                if let Some(utxo) = utxo_set.lookup(ref_input) {
                    match &utxo.script_ref {
                        Some(ScriptRef::PlutusV1(_)) => has_v1 = true,
                        Some(ScriptRef::PlutusV2(_)) => has_v2 = true,
                        Some(ScriptRef::PlutusV3(_)) => has_v3 = true,
                        _ => {}
                    }
                }
            }

            // Compute the expected script_data_hash. When raw tx CBOR is
            // available we use pallas KeepRaw to preserve the original encoding
            // of redeemers and datums exactly.
            let computed = if let Some(raw) = tx.raw_cbor.as_ref() {
                torsten_serialization::compute_script_data_hash_from_cbor(
                    raw,
                    &params.cost_models,
                    has_v1,
                    has_v2,
                    has_v3,
                )
                .unwrap_or_else(|| {
                    torsten_serialization::compute_script_data_hash(
                        &tx.witness_set.redeemers,
                        &tx.witness_set.plutus_data,
                        &params.cost_models,
                        has_v1,
                        has_v2,
                        has_v3,
                        tx.witness_set.raw_redeemers_cbor.as_deref(),
                        tx.witness_set.raw_plutus_data_cbor.as_deref(),
                    )
                })
            } else {
                torsten_serialization::compute_script_data_hash(
                    &tx.witness_set.redeemers,
                    &tx.witness_set.plutus_data,
                    &params.cost_models,
                    has_v1,
                    has_v2,
                    has_v3,
                    tx.witness_set.raw_redeemers_cbor.as_deref(),
                    tx.witness_set.raw_plutus_data_cbor.as_deref(),
                )
            };

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
}

/// Return `true` when the transaction has any Plutus scripts or redeemers.
pub(super) fn has_plutus_scripts(tx: &Transaction) -> bool {
    !tx.witness_set.plutus_v1_scripts.is_empty()
        || !tx.witness_set.plutus_v2_scripts.is_empty()
        || !tx.witness_set.plutus_v3_scripts.is_empty()
        || !tx.witness_set.redeemers.is_empty()
}

/// Return the fee to add for reference scripts (0 when no reference inputs).
pub(super) fn ref_script_fee(
    reference_inputs: &[TransactionInput],
    utxo_set: &UtxoSet,
    min_fee_ref_script_cost_per_byte: u64,
) -> u64 {
    let size = calculate_ref_script_size(reference_inputs, utxo_set);
    if size > 0 {
        calculate_ref_script_tiered_fee(min_fee_ref_script_cost_per_byte, size)
    } else {
        0
    }
}

/// Compute the total execution-unit fee component from the transaction's redeemers.
///
/// Formula: `ceil(price_mem * Σ mem) + ceil(price_step * Σ steps)`
pub(super) fn ex_unit_fee(tx: &Transaction, params: &ProtocolParameters) -> u64 {
    let total_mem: u64 = tx
        .witness_set
        .redeemers
        .iter()
        .fold(0u64, |acc, r| acc.saturating_add(r.ex_units.mem));
    let total_steps: u64 = tx
        .witness_set
        .redeemers
        .iter()
        .fold(0u64, |acc, r| acc.saturating_add(r.ex_units.steps));

    let mem_cost = if total_mem > 0 && params.execution_costs.mem_price.denominator > 0 {
        let num = params.execution_costs.mem_price.numerator as u128 * total_mem as u128;
        let den = params.execution_costs.mem_price.denominator as u128;
        num.div_ceil(den) as u64
    } else {
        0
    };
    let step_cost = if total_steps > 0 && params.execution_costs.step_price.denominator > 0 {
        let num = params.execution_costs.step_price.numerator as u128 * total_steps as u128;
        let den = params.execution_costs.step_price.denominator as u128;
        num.div_ceil(den) as u64
    } else {
        0
    };
    mem_cost.saturating_add(step_cost)
}

/// Compute the minimum fee including base formula, reference-script fee and
/// execution-unit costs.
pub(super) fn compute_min_fee(
    tx: &Transaction,
    utxo_set: &UtxoSet,
    params: &ProtocolParameters,
    tx_size: u64,
) -> Lovelace {
    let rs_fee = ref_script_fee(
        &tx.body.reference_inputs,
        utxo_set,
        params.min_fee_ref_script_cost_per_byte,
    );
    let eu_fee = ex_unit_fee(tx, params);
    Lovelace(
        params
            .min_fee(tx_size)
            .0
            .saturating_add(rs_fee)
            .saturating_add(eu_fee),
    )
}
