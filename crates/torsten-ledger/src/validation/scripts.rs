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
// Reference script size + tiered fee (Conway ledger spec)
// ---------------------------------------------------------------------------

/// Calculate the total byte size of reference scripts from all UTxOs touched by a
/// transaction — both spending inputs and reference inputs.
///
/// Matches Haskell's `txNonDistinctRefScriptsSize` from
/// `Cardano.Ledger.Conway.Tx` (CIP-0112), which iterates over
/// `(inputs txb <> referenceInputs txb)` and sums `originalBytesSize` for every
/// UTxO that carries a `script_ref`.  The count is **non-distinct** — if the same
/// script hash appears in multiple UTxOs it is counted each time.
///
/// The `inputs` and `reference_inputs` slices are provided separately so callers can
/// supply the exact transaction body fields without allocating a merged vector.
/// Pass an empty slice for either argument if only one set is applicable (e.g. the
/// block-level pre-scan handles its own overlay and may call this differently).
///
/// # Within-block visibility
///
/// When called from `compute_min_fee` inside `validate_transaction`, `utxo_set` is
/// `&self.utxo_set` which already contains UTxOs created by all prior transactions in
/// the same block (applied sequentially by `apply_block`).  No separate overlay is
/// needed for the per-transaction fee check path.
pub(crate) fn calculate_ref_script_size(
    inputs: &[TransactionInput],
    reference_inputs: &[TransactionInput],
    utxo_set: &UtxoSet,
) -> u64 {
    let mut total_size: u64 = 0;
    // Iterate both spending inputs and reference inputs, matching Haskell's
    // `inputs txb <> referenceInputs txb` set union.
    for inp in inputs.iter().chain(reference_inputs.iter()) {
        if let Some(utxo) = utxo_set.lookup(inp) {
            if let Some(script_ref) = &utxo.script_ref {
                total_size = total_size.saturating_add(script_ref_byte_size(script_ref));
            }
        }
    }
    total_size
}

/// Return the byte size of a single reference script.
pub(crate) fn script_ref_byte_size(script_ref: &ScriptRef) -> u64 {
    match script_ref {
        ScriptRef::NativeScript(ns) => torsten_serialization::encode_native_script(ns).len() as u64,
        ScriptRef::PlutusV1(bytes) | ScriptRef::PlutusV2(bytes) | ScriptRef::PlutusV3(bytes) => {
            bytes.len() as u64
        }
    }
}

/// Conway ledger tiered reference script fee calculation.
///
/// Divides the total script size into 25 KiB tiers, applying a 1.2× multiplier
/// per tier. The result is the ceiling of the exact rational sum — matching
/// the Cardano Blueprint spec which states "you should take the ceiling as a
/// last step" and Haskell's `tierRefScriptFee` which uses `Data.Ratio.Rational`
/// and `ceiling`.
///
/// # Algorithm: scaled-integer accumulation
///
/// Naive rational accumulation (`acc_num / acc_den`) overflows u128 beyond
/// tier ~25 because the cross-product denominator grows as `5^(n*(n-1)/2)`.
/// GCD reduction is insufficient for `base_fee_per_byte` values not divisible
/// by 5 (e.g., base = 1).
///
/// This implementation avoids cross-products entirely by separating each tier's
/// contribution into an integer part and a fractional remainder that is scaled
/// to a common denominator known at entry:
///
/// 1. Pre-count tiers: `k = ceil(total_size / 25600)`.
/// 2. Set common denominator `denom = 5^(k-1)`.
///    At k=41 (the 1 MiB cap), `denom = 5^40 ≈ 9.1×10²⁷ < u128::MAX`.
/// 3. Per tier `i` with `chunk` bytes:
///    - `contribution = chunk * price_num`  (price = base * (6/5)^i, GCD-reduced)
///    - `whole = contribution / price_den`  — exact integer quotient
///    - `tier_rem = contribution % price_den`
///    - `scaled_rem = tier_rem * (denom / price_den)` — always < denom since
///      `price_den` always divides `denom` (both are powers of 5)
/// 4. Accumulate: `acc_whole += whole`, `frac_scaled += scaled_rem`.
///    Drain any whole units from `frac_scaled` into `acc_whole` when
///    `frac_scaled >= denom`.
/// 5. Single ceiling: `fee = acc_whole + (1 if frac_scaled > 0)`.
///
/// # Overflow proofs (within the 1 MiB cap)
///
/// - `denom = 5^40 < 10^28 < u128::MAX` ✓
/// - `scaled_rem < denom < u128::MAX` per iteration ✓
/// - `frac_scaled < 41 * denom < 4 × 10^29 < u128::MAX` ✓ (41 tiers × denom)
/// - `chunk * price_num`: `price_num ≤ base * 6^40`. For realistic protocol
///   params (base ≤ 10^9 lovelace/byte), `chunk * price_num ≤ 25600 × 10^9 ×
///   2.23×10^31 ≈ 5.7×10^44` — would overflow for very large `base`.  All
///   multiplications therefore use `checked_mul`; if they overflow (unreachable
///   with realistic params), the function saturates to `u64::MAX`.
///
/// # Inputs beyond the cap
///
/// For `total_size > MAX_REF_SCRIPT_SIZE_TIER_CAP` (1 MiB), the function
/// short-circuits immediately with `u64::MAX`. Such inputs are already rejected
/// by the Conway block-body rule before fee calculation is invoked in
/// production.
pub(super) fn calculate_ref_script_tiered_fee(base_fee_per_byte: u64, total_size: u64) -> u64 {
    const TIER_SIZE: u64 = 25_600; // 25 KiB (= 25 * 1024)

    // Inputs beyond the Conway 1 MiB block-body limit are rejected before this
    // function is called in production.  Saturate immediately so no floating-
    // point arithmetic is ever needed for out-of-range inputs.
    if total_size > MAX_REF_SCRIPT_SIZE_TIER_CAP {
        return u64::MAX;
    }
    if total_size == 0 || base_fee_per_byte == 0 {
        return 0;
    }

    // Pre-count tiers: ceil(total_size / TIER_SIZE).
    let k = total_size.div_ceil(TIER_SIZE);

    // Common denominator for all tier fractional parts: 5^(k-1).
    // price_den at tier i = 5^i / gcd(base, 5^i), which always divides 5^(k-1)
    // (since k-1 >= i), so scale_factor = denom / price_den is always exact.
    let denom: u128 = pow5(k - 1); // 5^0 = 1 when k = 1 (single tier)

    // Accumulated integer part of the sum.
    let mut acc_whole: u128 = 0;
    // Accumulated fractional part, scaled by `denom` (i.e., frac_scaled/denom ∈ [0,1)).
    let mut frac_scaled: u128 = 0;

    // Current tier price as exact rational price_num / price_den.
    // Tier 0: base/1.  Each tier multiplies by 6/5; GCD is reduced immediately.
    let mut price_num: u128 = base_fee_per_byte as u128;
    let mut price_den: u128 = 1;

    let mut remaining = total_size;

    while remaining > 0 {
        let chunk = remaining.min(TIER_SIZE) as u128;

        // Tier contribution = chunk * price_num / price_den.
        // checked_mul guards against overflow for very large base_fee_per_byte.
        let contribution = match chunk.checked_mul(price_num) {
            Some(v) => v,
            None => return u64::MAX,
        };
        // Exact integer quotient and remainder.
        let whole = contribution / price_den;
        let tier_rem = contribution % price_den; // in [0, price_den)

        // Accumulate integer part.
        acc_whole = match acc_whole.checked_add(whole) {
            Some(v) => v,
            None => return u64::MAX,
        };

        // Scale the fractional remainder to the common denominator.
        // scale_factor = denom / price_den is always a whole number because
        // price_den (a power of 5, after GCD reduction) divides denom = 5^(k-1).
        // scaled_rem < price_den * scale_factor = denom, so no overflow.
        let scale_factor = denom / price_den;
        let scaled_rem = match tier_rem.checked_mul(scale_factor) {
            Some(v) => v,
            None => return u64::MAX, // unreachable: scaled_rem < denom < u128::MAX
        };
        frac_scaled = match frac_scaled.checked_add(scaled_rem) {
            Some(v) => v,
            None => return u64::MAX, // unreachable: sum < 41*denom < 4e29 < u128::MAX
        };

        // Carry any whole units that accumulated in the fractional bucket.
        // This happens when multiple tiers each contribute close to 1 fractional unit.
        if frac_scaled >= denom {
            let carry = frac_scaled / denom;
            frac_scaled %= denom;
            acc_whole = match acc_whole.checked_add(carry) {
                Some(v) => v,
                None => return u64::MAX,
            };
        }

        remaining -= chunk as u64;

        // Advance price: multiply by 6/5 and immediately GCD-reduce to keep
        // price_num and price_den as small as possible, and to preserve the
        // invariant that price_den divides denom.
        price_num = match price_num.checked_mul(6) {
            Some(p) => p,
            None => return u64::MAX,
        };
        price_den = match price_den.checked_mul(5) {
            Some(p) => p,
            None => return u64::MAX,
        };
        let g = gcd_u128(price_num, price_den);
        price_num /= g;
        price_den /= g;
    }

    // Single ceiling per the Blueprint spec: add 1 if any fractional remainder.
    let ceil_bit: u128 = if frac_scaled > 0 { 1 } else { 0 };
    let total = match acc_whole.checked_add(ceil_bit) {
        Some(v) => v,
        None => return u64::MAX,
    };
    // Saturate to u64::MAX if the fee exceeds u64 range (only possible for
    // unrealistically large base_fee_per_byte values).
    u64::try_from(total).unwrap_or(u64::MAX)
}

/// Compute `5^n` exactly as a u128.
///
/// Used by [`calculate_ref_script_tiered_fee`] to build the common denominator
/// `denom = 5^(k-1)`.  At k=41 (the 1 MiB block cap), `5^40 ≈ 9.1×10²⁷`,
/// safely within u128 range.  `5^54 ≈ 5.6×10³⁷ < u128::MAX`; `5^55` overflows.
#[inline]
fn pow5(n: u64) -> u128 {
    let mut result: u128 = 1;
    for _ in 0..n {
        result = result
            .checked_mul(5)
            .expect("pow5: result overflows u128 — n must not exceed 54");
    }
    result
}

/// Upper bound on `total_size` for [`calculate_ref_script_tiered_fee`].
///
/// Any input exceeding this cap causes the function to immediately return
/// `u64::MAX` — the transaction is rejected regardless of the precise fee.
///
/// This value equals the Conway `maxRefScriptSizePerBlock` hard limit (1 MiB)
/// which is not a governance-updatable protocol parameter.  Exposed as
/// `pub(crate)` so that `apply.rs` can reuse the same constant for the
/// block-body check, keeping the two in sync.
pub(crate) const MAX_REF_SCRIPT_SIZE_TIER_CAP: u64 = 1024 * 1024; // 1 MiB

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

            // Debug log when the intersection is empty despite having redeemers
            if !has_v1 && !has_v2 && !has_v3 && has_redeemers && !scripts_needed.is_empty() {
                debug!(
                    needed_count = scripts_needed.len(),
                    provided_count = scripts_provided.len(),
                    needed = ?scripts_needed.iter().map(|h| h.to_hex()).collect::<Vec<_>>(),
                    provided = ?scripts_provided.keys().map(|h| h.to_hex()).collect::<Vec<_>>(),
                    "scriptsNeeded/Provided intersection empty — falling back to ref input scan"
                );
            }

            // Fallback: if we have redeemers but no languages were detected
            // (script hash matching failed), scan reference inputs directly.
            // This handles edge cases where our hash computation differs from
            // the address hash (e.g., double-encoded scripts).
            if !has_v1 && !has_v2 && !has_v3 && has_redeemers {
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

/// Return the tiered reference-script fee for a transaction.
///
/// Per Haskell's `txNonDistinctRefScriptsSize`, the fee is based on the total
/// script bytes reachable from BOTH spending inputs and reference inputs.
/// Passing an empty slice for either argument is valid when that class of inputs
/// is absent from the transaction.
pub(super) fn ref_script_fee(
    inputs: &[TransactionInput],
    reference_inputs: &[TransactionInput],
    utxo_set: &UtxoSet,
    min_fee_ref_script_cost_per_byte: u64,
) -> u64 {
    let size = calculate_ref_script_size(inputs, reference_inputs, utxo_set);
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

/// Compute the Haskell-compatible transaction size for fee calculation.
///
/// Haskell's `toCBORForSizeComputation` (Alonzo+ eras) encodes the transaction
/// as a **3-element** CBOR array `[body, wits, aux_data]`, deliberately omitting
/// the `is_valid` boolean field to maintain fee-formula continuity with the Mary
/// era.  The on-chain representation (and our `raw_cbor`) is a **4-element** array
/// `[body, wits, is_valid, aux_data]`.  The difference is exactly 1 byte — the
/// CBOR encoding of the boolean (`0xF4`/`0xF5`).
///
/// We detect Alonzo+ by checking whether the first byte of `raw_cbor` is `0x84`
/// (definite-length array of 4).  Pre-Alonzo transactions start with `0x83`
/// (array of 3) and have no `is_valid` field, so no adjustment is needed.
///
/// Reference:
/// - `Cardano.Ledger.Alonzo.Tx.toCBORForSizeComputation` (cardano-ledger)
/// - Conway tiered reference script fee is based on script bytes, not tx size.
fn fee_tx_size(tx: &Transaction, tx_size: u64) -> u64 {
    // The raw CBOR first byte tells us the CBOR major type and additional info.
    // 0x84 = major type 4 (array) with additional info 4 (length = 4 elements).
    // An Alonzo+ transaction is encoded as array(4); pre-Alonzo as array(3) = 0x83.
    let is_alonzo_plus = tx
        .raw_cbor
        .as_deref()
        .is_some_and(|b| b.first() == Some(&0x84));
    if is_alonzo_plus {
        // Subtract the 1-byte is_valid field that Haskell excludes from fee size.
        tx_size.saturating_sub(1)
    } else {
        tx_size
    }
}

/// Compute the minimum fee including base formula, reference-script fee and
/// execution-unit costs.
///
/// The base fee uses [`fee_tx_size`] to match Haskell's `toCBORForSizeComputation`,
/// which omits the `is_valid` boolean from the size for Alonzo+ transactions.
pub(super) fn compute_min_fee(
    tx: &Transaction,
    utxo_set: &UtxoSet,
    params: &ProtocolParameters,
    tx_size: u64,
) -> Lovelace {
    // Pass both spending inputs and reference inputs so that scripts embedded in
    // spending-input UTxOs are counted in the tiered fee — matching Haskell's
    // `txNonDistinctRefScriptsSize` which uses `inputs txb <> referenceInputs txb`.
    let rs_fee = ref_script_fee(
        &tx.body.inputs,
        &tx.body.reference_inputs,
        utxo_set,
        params.min_fee_ref_script_cost_per_byte,
    );
    let eu_fee = ex_unit_fee(tx, params);
    // Use the Haskell-compatible size (excludes is_valid for Alonzo+).
    let effective_size = fee_tx_size(tx, tx_size);
    Lovelace(
        params
            .min_fee(effective_size)
            .0
            .saturating_add(rs_fee)
            .saturating_add(eu_fee),
    )
}
