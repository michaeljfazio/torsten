//! Collateral validation (Rule 11).
//!
//! Collateral is required for all transactions that include Plutus scripts.
//! This module implements:
//! - Presence and count checks for collateral inputs
//! - Lookup of collateral UTxOs
//! - Multi-asset net-token check (collateral net must be pure ADA)
//! - `total_collateral` declaration matching
//! - Minimum collateral percentage enforcement (ceiling division, matching Haskell)
//! - Per-transaction execution-unit limit check
//! - Redeemer index bounds check (Rule 11b)
//! - Missing Spend redeemer for script-locked inputs (Rule 11c)
//! - Missing Reward redeemer for script-locked withdrawals (Rule 11c, Haskell `scriptsNeeded`)
//! - Missing Mint redeemer for Plutus minting policies (Rule 11c, Haskell `scriptsNeeded`)
//! - Missing Cert redeemer for script-credential certificates (Rule 11c, Haskell `conwayCertsNeeded`)
//! - Missing Vote redeemer for script-credential voters (Rule 11c, Haskell `conwayVotesNeeded`)
//! - Missing Propose redeemer for governed proposals with a policy_hash (Rule 11c)

use std::collections::{BTreeMap, HashMap, HashSet};

use torsten_primitives::credentials::Credential;
use torsten_primitives::hash::{Hash28, PolicyId};
use torsten_primitives::protocol_params::ProtocolParameters;
use torsten_primitives::transaction::{
    Certificate, GovAction, RedeemerTag, ScriptRef, Transaction, Voter,
};
use torsten_primitives::value::AssetName;

use crate::utxo::UtxoLookup;

use super::scripts::compute_script_ref_hash;
use super::ValidationError;

/// Validate all collateral-related rules for a Plutus transaction (Rule 11).
///
/// This function is only called when `has_plutus_scripts(tx)` is true.
pub(crate) fn check_collateral(
    tx: &Transaction,
    utxo_set: &dyn UtxoLookup,
    params: &ProtocolParameters,
    errors: &mut Vec<ValidationError>,
) {
    let body = &tx.body;

    // Rule 11 – collateral inputs must be present
    if body.collateral.is_empty() {
        errors.push(ValidationError::InsufficientCollateral);
        // Cannot proceed with further collateral checks without inputs
        check_ex_units(tx, params, errors);
        check_redeemer_indices(tx, errors);
        return;
    }

    // Rule 11 – max collateral inputs count
    if body.collateral.len() as u64 > params.max_collateral_inputs {
        errors.push(ValidationError::TooManyCollateralInputs {
            max: params.max_collateral_inputs,
            actual: body.collateral.len() as u64,
        });
    }

    // Accumulate collateral value and multi-asset balances
    let mut collateral_value = 0u64;
    let mut collateral_multi_asset: BTreeMap<PolicyId, BTreeMap<AssetName, i128>> = BTreeMap::new();

    for col_input in &body.collateral {
        match utxo_set.lookup(col_input) {
            Some(output) => {
                collateral_value = collateral_value.saturating_add(output.value.coin.0);
                // Accumulate multi-asset from collateral inputs
                for (policy, assets) in &output.value.multi_asset {
                    for (name, qty) in assets {
                        *collateral_multi_asset
                            .entry(*policy)
                            .or_default()
                            .entry(name.clone())
                            .or_insert(0) += *qty as i128;
                    }
                }
            }
            None => {
                errors.push(ValidationError::CollateralNotFound(col_input.to_string()));
            }
        }
    }

    // Account for collateral return output (Babbage+)
    let effective_collateral = if let Some(col_return) = &body.collateral_return {
        // Subtract collateral_return multi-asset from net balance
        for (policy, assets) in &col_return.value.multi_asset {
            for (name, qty) in assets {
                *collateral_multi_asset
                    .entry(*policy)
                    .or_default()
                    .entry(name.clone())
                    .or_insert(0) -= *qty as i128;
            }
        }
        collateral_value.saturating_sub(col_return.value.coin.0)
    } else {
        collateral_value
    };

    // Net collateral (inputs minus return) must be pure ADA
    let has_net_tokens = collateral_multi_asset
        .values()
        .any(|assets: &BTreeMap<AssetName, i128>| assets.values().any(|qty| *qty > 0));
    if has_net_tokens {
        errors.push(ValidationError::CollateralHasTokens(
            "net collateral has non-ADA tokens after collateral_return".to_string(),
        ));
    }

    // If total_collateral is declared, it must match the effective collateral
    if let Some(total_col) = body.total_collateral {
        if total_col.0 != effective_collateral {
            errors.push(ValidationError::CollateralMismatch {
                declared: total_col.0,
                computed: effective_collateral,
            });
        }
    }

    // Effective collateral must be >= ceil(fee * collateral_percentage / 100).
    //
    // Haskell uses `ceiling (fromIntegral fee * fromIntegral pct % 100)`, which
    // is equivalent to ceiling division.  Truncating division under-counts by at
    // most 1 lovelace, which would incorrectly accept transactions whose
    // collateral sits exactly on the fractional threshold.
    //
    // Example: fee=101, pct=150 → exact=151.5 → required=152 (not 151).
    let required_collateral = (body.fee.0 * params.collateral_percentage).div_ceil(100);
    if effective_collateral < required_collateral {
        errors.push(ValidationError::InsufficientCollateral);
    }

    // Rule 11 – execution unit limits
    check_ex_units(tx, params, errors);

    // Rule 11b – redeemer index bounds
    check_redeemer_indices(tx, errors);
}

/// Check that total execution units in the transaction do not exceed the
/// per-transaction limits.
fn check_ex_units(
    tx: &Transaction,
    params: &ProtocolParameters,
    errors: &mut Vec<ValidationError>,
) {
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
    if total_mem > params.max_tx_ex_units.mem || total_steps > params.max_tx_ex_units.steps {
        errors.push(ValidationError::ExUnitsExceeded);
    }
}

/// Rule 11b: Check that each redeemer's index is within the valid range for
/// its tag type.
///
/// For Vote redeemers, the valid index range is `[0, vote_voter_count)` where
/// `vote_voter_count` is the total number of voters in `voting_procedures` whose
/// credential is a `Script`.  The index is the position in the full canonically
/// sorted voter list (BTreeMap iteration order: CC < DRep < SPO by enum
/// discriminant, then by credential hash within each type).
///
/// For Propose redeemers, the valid index range is `[0, propose_count)` where
/// `propose_count` is the number of `proposal_procedures` entries that carry a
/// `policy_hash` (i.e. `ParameterChange` or `TreasuryWithdrawals` with a
/// non-None `policy_hash`).
fn check_redeemer_indices(tx: &Transaction, errors: &mut Vec<ValidationError>) {
    let body = &tx.body;
    let input_count = body.inputs.len();
    let mint_count = body.mint.len();
    let cert_count = body.certificates.len();
    let withdrawal_count = body.withdrawals.len();

    // Count voters with script credentials — the upper bound for Vote redeemer indices.
    // The index is the position in the full BTreeMap-ordered voter list (all voter types),
    // not re-numbered to exclude key-credential voters.  So the max valid index is the
    // last position that holds a script voter.  For simplicity, and matching Haskell's
    // `conwayVotesNeeded` which enumerates all voters, we bound by the total voter count.
    let vote_voter_count = body.voting_procedures.len();

    // Propose redeemer indices are positional in the full `proposal_procedures` list,
    // matching Haskell's `zip [0..] (toList (txProposalProcedures txb))`.  The index
    // is NOT renumbered to skip non-governed proposals — it is the raw position.
    let propose_count = body.proposal_procedures.len();

    for redeemer in &tx.witness_set.redeemers {
        let (max, tag_name) = match redeemer.tag {
            RedeemerTag::Spend => (input_count, "Spend"),
            RedeemerTag::Mint => (mint_count, "Mint"),
            RedeemerTag::Cert => (cert_count, "Cert"),
            RedeemerTag::Reward => (withdrawal_count, "Reward"),
            RedeemerTag::Vote => (vote_voter_count, "Vote"),
            RedeemerTag::Propose => (propose_count, "Propose"),
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

/// Rule 11c: Every script-locked spending input, script-locked withdrawal, and
/// Plutus minting policy must have a matching redeemer of the appropriate tag.
///
/// Matches Haskell's `scriptsNeeded` which requires:
/// - A `Spend` redeemer at the sorted index for each script-locked input.
/// - A `Reward` redeemer at the sorted index for each script-locked withdrawal
///   (reward address whose stake credential is a script hash).
/// - A `Mint` redeemer at the sorted index for each Plutus minting policy (a
///   policy ID that matches a script in the witness set or reference inputs).
pub(crate) fn check_script_redeemers(
    tx: &Transaction,
    utxo_set: &dyn UtxoLookup,
    errors: &mut Vec<ValidationError>,
) {
    let body = &tx.body;

    // ------------------------------------------------------------------ Spend
    // Collect existing Spend redeemer indices.
    let spend_indices: HashSet<u32> = tx
        .witness_set
        .redeemers
        .iter()
        .filter(|r| r.tag == RedeemerTag::Spend)
        .map(|r| r.index)
        .collect();

    // Cardano sorts inputs by (tx_id, index) for deterministic redeemer index
    // assignment.
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

    // ----------------------------------------------------------------- Reward
    // Withdrawals are kept in a BTreeMap<Vec<u8>, Lovelace> keyed by the
    // raw reward address bytes.  The BTreeMap iteration order is the
    // canonical sorted order, which gives us the deterministic index that
    // Haskell uses for Reward redeemers.
    //
    // A reward address is script-locked when the header nibble is 0xF
    // (i.e., bit 4 of the header byte is set), indicating a script stake
    // credential.
    let reward_indices: HashSet<u32> = tx
        .witness_set
        .redeemers
        .iter()
        .filter(|r| r.tag == RedeemerTag::Reward)
        .map(|r| r.index)
        .collect();

    for (idx, reward_addr) in body.withdrawals.keys().enumerate() {
        if reward_addr.len() < 29 {
            continue;
        }
        let header = reward_addr[0];
        // Bit 4 of the header distinguishes script (1) from key (0) credentials.
        // 0xE0/0xE1 = key stake credential (no redeemer required)
        // 0xF0/0xF1 = script stake credential (Reward redeemer required)
        let is_script_credential = (header & 0x10) != 0;
        if is_script_credential && !reward_indices.contains(&(idx as u32)) {
            errors.push(ValidationError::MissingRedeemer {
                tag: "Reward".to_string(),
                index: idx as u32,
            });
        }
    }

    // ------------------------------------------------------------------ Mint
    // Build the set of Plutus script hashes available to this transaction
    // (witness set V1/V2/V3 + reference input script_refs).
    let plutus_script_hashes: HashSet<Hash28> = collect_plutus_script_hashes(tx, utxo_set);

    // Minting policies are sorted by policy ID (the BTreeMap key order) to
    // assign deterministic Mint redeemer indices, matching Haskell's
    // `Map.toAscList (txmint txb)`.
    let mint_indices: HashSet<u32> = tx
        .witness_set
        .redeemers
        .iter()
        .filter(|r| r.tag == RedeemerTag::Mint)
        .map(|r| r.index)
        .collect();

    for (idx, policy_id) in body.mint.keys().enumerate() {
        // Only Plutus policies need a Mint redeemer.  Native script policies
        // are authenticated by including the script in native_scripts — they
        // do not use redeemers.
        if plutus_script_hashes.contains(policy_id) && !mint_indices.contains(&(idx as u32)) {
            errors.push(ValidationError::MissingRedeemer {
                tag: "Mint".to_string(),
                index: idx as u32,
            });
        }
    }

    // ------------------------------------------------------------------ Cert
    // Per Haskell's `conwayCertsNeeded` (Cardano.Ledger.Conway.Rules.Cert),
    // every certificate that requires a script witness must have a matching
    // `Cert` redeemer at the certificate's 0-based position in the body's
    // certificate list.  The index is NOT re-numbered by skipping key-credential
    // certs; it is the raw positional index, matching Haskell's
    // `zip [0..] (txcerts txb)`.
    //
    // Certificates that require a script credential witness (i.e. contribute to
    // `scriptsNeeded`):
    //   - StakeDeregistration (pre-Conway, credential must be Script)
    //   - StakeDelegation (credential must be Script)
    //   - ConwayStakeDeregistration (credential must be Script)
    //   - VoteDelegation (credential must be Script)
    //   - StakeVoteDelegation (credential must be Script)
    //   - RegStakeDeleg (credential must be Script)
    //   - RegStakeVoteDeleg (credential must be Script)
    //   - VoteRegDeleg (credential must be Script)
    //   - CommitteeHotAuth (cold_credential must be Script)
    //   - CommitteeColdResign (cold_credential must be Script)
    //   - UnregDRep (credential must be Script)
    //   - UpdateDRep (credential must be Script)
    //
    // Certificates that do NOT require a redeemer regardless of credential type:
    //   - StakeRegistration, ConwayStakeRegistration, PoolRegistration,
    //     PoolRetirement, RegDRep, GenesisKeyDelegation, MoveInstantaneousRewards
    //     (either registrations or pool-operator-only actions)
    let cert_indices: HashSet<u32> = tx
        .witness_set
        .redeemers
        .iter()
        .filter(|r| r.tag == RedeemerTag::Cert)
        .map(|r| r.index)
        .collect();

    for (idx, cert) in body.certificates.iter().enumerate() {
        // Extract the script credential that must be witnessed, if any.
        let script_cred: Option<&Credential> = match cert {
            // Pre-Conway: stake deregistration requires witness when credential is Script.
            Certificate::StakeDeregistration(c) => Some(c),
            // Pre-Conway: stake delegation requires witness when credential is Script.
            Certificate::StakeDelegation { credential: c, .. } => Some(c),
            // Conway deregistration requires a witness when credential is Script.
            Certificate::ConwayStakeDeregistration { credential: c, .. } => Some(c),
            // Conway delegation variants require a witness when credential is Script.
            Certificate::VoteDelegation { credential: c, .. } => Some(c),
            Certificate::StakeVoteDelegation { credential: c, .. } => Some(c),
            Certificate::RegStakeDeleg { credential: c, .. } => Some(c),
            Certificate::RegStakeVoteDeleg { credential: c, .. } => Some(c),
            Certificate::VoteRegDeleg { credential: c, .. } => Some(c),
            // CC hot-key authorisation: the cold credential authorises the hot key.
            Certificate::CommitteeHotAuth {
                cold_credential: c, ..
            } => Some(c),
            // CC cold resign: the cold credential must sign.
            Certificate::CommitteeColdResign {
                cold_credential: c, ..
            } => Some(c),
            // DRep unregistration: credential must sign.
            Certificate::UnregDRep { credential: c, .. } => Some(c),
            // DRep update: credential must sign.
            Certificate::UpdateDRep { credential: c, .. } => Some(c),
            // Registrations and pool operations do not require a redeemer.
            Certificate::StakeRegistration(_)
            | Certificate::ConwayStakeRegistration { .. }
            | Certificate::PoolRegistration(_)
            | Certificate::PoolRetirement { .. }
            | Certificate::RegDRep { .. }
            | Certificate::GenesisKeyDelegation { .. }
            | Certificate::MoveInstantaneousRewards { .. } => None,
        };

        // A redeemer is only required when the extracted credential is a script hash.
        if let Some(Credential::Script(_)) = script_cred {
            if !cert_indices.contains(&(idx as u32)) {
                errors.push(ValidationError::MissingRedeemer {
                    tag: "Cert".to_string(),
                    index: idx as u32,
                });
            }
        }
    }

    // ------------------------------------------------------------------ Vote
    // Per Haskell's `conwayVotesNeeded` (Cardano.Ledger.Conway.Rules.Certs),
    // every voter in `voting_procedures` whose credential is a script hash must
    // have a matching `Vote` redeemer.  The redeemer index is the voter's 0-based
    // position in the full canonically sorted voter list.
    //
    // Canonical voter ordering (matching Haskell's `OMap` / CBOR map order in
    // the `voting_procedures` body field):
    //   ConstitutionalCommittee < DRep < StakePool  (by enum discriminant order)
    //   Within each group: sorted by credential hash bytes.
    //
    // SPO voters (`StakePool(Hash32)`) are identified by a pool ID (a key hash)
    // and can never be a script credential — they never require a Vote redeemer.
    // Only `ConstitutionalCommittee(Credential::Script(_))` and
    // `DRep(Credential::Script(_))` require one.
    //
    // The `Voter` enum derives `Ord`, which uses Rust's discriminant order followed
    // by field comparison.  `BTreeMap<Voter, _>` iteration therefore produces the
    // same canonical order that Haskell uses.
    let vote_indices: HashSet<u32> = tx
        .witness_set
        .redeemers
        .iter()
        .filter(|r| r.tag == RedeemerTag::Vote)
        .map(|r| r.index)
        .collect();

    for (idx, voter) in body.voting_procedures.keys().enumerate() {
        // Extract the script credential from voters that can carry one.
        // `StakePool` voters are pool key hashes, never scripts.
        let is_script_voter = match voter {
            Voter::ConstitutionalCommittee(cred) | Voter::DRep(cred) => {
                matches!(cred, Credential::Script(_))
            }
            Voter::StakePool(_) => false,
        };
        if is_script_voter && !vote_indices.contains(&(idx as u32)) {
            errors.push(ValidationError::MissingRedeemer {
                tag: "Vote".to_string(),
                index: idx as u32,
            });
        }
    }

    // --------------------------------------------------------------- Propose
    // Per the Conway ledger spec (Section 4.9 "Proposals"), a governance action
    // that carries a `policy_hash` (i.e. `ParameterChange` or
    // `TreasuryWithdrawals` with a non-None `policy_hash`) requires a
    // constitutionality script to approve it.  The script must be executed via
    // a `Propose` redeemer at the action's 0-based position in
    // `proposal_procedures`.
    //
    // All other action types (HardForkInitiation, NoConfidence, UpdateCommittee,
    // NewConstitution, InfoAction) do not carry a policy_hash and never require
    // a Propose redeemer.
    //
    // The index is the raw positional index in `proposal_procedures` (not
    // re-numbered by skipping non-governed actions), matching Haskell's
    // `zip [0..] (toList (txProposalProcedures txb))`.
    let propose_indices: HashSet<u32> = tx
        .witness_set
        .redeemers
        .iter()
        .filter(|r| r.tag == RedeemerTag::Propose)
        .map(|r| r.index)
        .collect();

    for (idx, proposal) in body.proposal_procedures.iter().enumerate() {
        if govaction_has_policy_hash(&proposal.gov_action)
            && !propose_indices.contains(&(idx as u32))
        {
            errors.push(ValidationError::MissingRedeemer {
                tag: "Propose".to_string(),
                index: idx as u32,
            });
        }
    }
}

/// Return `true` if a governance action requires a constitutionality script
/// witness (i.e. carries a non-None `policy_hash`).
///
/// Only `ParameterChange` and `TreasuryWithdrawals` can carry a `policy_hash`.
/// All other action types never require a Propose redeemer.
fn govaction_has_policy_hash(action: &GovAction) -> bool {
    match action {
        GovAction::ParameterChange { policy_hash, .. } => policy_hash.is_some(),
        GovAction::TreasuryWithdrawals { policy_hash, .. } => policy_hash.is_some(),
        GovAction::HardForkInitiation { .. }
        | GovAction::NoConfidence { .. }
        | GovAction::UpdateCommittee { .. }
        | GovAction::NewConstitution { .. }
        | GovAction::InfoAction => false,
    }
}

/// Build the set of Plutus script hashes (V1, V2, V3) that are available to
/// this transaction: witness set scripts plus Plutus script_refs on spending
/// and reference inputs.
///
/// This mirrors the Plutus subset of Haskell's `scriptsProvided`, limited to
/// Plutus language scripts (native scripts do not use redeemers).
fn collect_plutus_script_hashes(tx: &Transaction, utxo_set: &dyn UtxoLookup) -> HashSet<Hash28> {
    // Collect all Plutus scripts with their version tag for hashing.
    // Map from hash → present (we only need membership).
    let mut hashes: HashSet<Hash28> = HashSet::new();

    // Witness set: V1 (tag 0x01), V2 (0x02), V3 (0x03)
    for s in &tx.witness_set.plutus_v1_scripts {
        hashes.insert(torsten_primitives::hash::blake2b_224_tagged(1, s));
    }
    for s in &tx.witness_set.plutus_v2_scripts {
        hashes.insert(torsten_primitives::hash::blake2b_224_tagged(2, s));
    }
    for s in &tx.witness_set.plutus_v3_scripts {
        hashes.insert(torsten_primitives::hash::blake2b_224_tagged(3, s));
    }

    // Reference scripts from spending inputs and reference inputs.
    for inp in tx.body.inputs.iter().chain(tx.body.reference_inputs.iter()) {
        if let Some(utxo) = utxo_set.lookup(inp) {
            if let Some(script_ref) = &utxo.script_ref {
                match script_ref {
                    ScriptRef::PlutusV1(_) | ScriptRef::PlutusV2(_) | ScriptRef::PlutusV3(_) => {
                        hashes.insert(compute_script_ref_hash(script_ref));
                    }
                    // Native scripts do not require redeemers.
                    ScriptRef::NativeScript(_) => {}
                }
            }
        }
    }

    hashes
}

/// Build a map from Plutus script hash → language version tag (1=V1, 2=V2,
/// 3=V3) for all Plutus scripts available to this transaction.
///
/// Used by the V3 non-Unit return check in `plutus.rs` to determine which
/// scripts executing in a given transaction are PlutusV3.
pub(crate) fn plutus_script_version_map(
    tx: &Transaction,
    utxo_set: &dyn UtxoLookup,
) -> HashMap<Hash28, u8> {
    let mut map: HashMap<Hash28, u8> = HashMap::new();
    for s in &tx.witness_set.plutus_v1_scripts {
        map.insert(torsten_primitives::hash::blake2b_224_tagged(1, s), 1);
    }
    for s in &tx.witness_set.plutus_v2_scripts {
        map.insert(torsten_primitives::hash::blake2b_224_tagged(2, s), 2);
    }
    for s in &tx.witness_set.plutus_v3_scripts {
        map.insert(torsten_primitives::hash::blake2b_224_tagged(3, s), 3);
    }
    for inp in tx.body.inputs.iter().chain(tx.body.reference_inputs.iter()) {
        if let Some(utxo) = utxo_set.lookup(inp) {
            if let Some(script_ref) = &utxo.script_ref {
                let tag = match script_ref {
                    ScriptRef::PlutusV1(_) => 1u8,
                    ScriptRef::PlutusV2(_) => 2,
                    ScriptRef::PlutusV3(_) => 3,
                    ScriptRef::NativeScript(_) => continue,
                };
                map.insert(compute_script_ref_hash(script_ref), tag);
            }
        }
    }
    map
}

/// Build a map from `(redeemer_tag_byte, redeemer_index)` → Plutus language
/// version (1=V1, 2=V2, 3=V3) for every redeemer in the transaction.
///
/// The redeemer tag byte matches the Cardano CDDL encoding used by
/// `eval_phase_two_raw`'s result tuple (the CBOR-encoded pallas `Redeemer`):
///   0 = Spend, 1 = Mint, 2 = Cert, 3 = Reward, 4 = Vote, 5 = Propose.
///
/// This is used by `evaluate_plutus_scripts` to apply the PlutusV3 Unit-return
/// check only to redeemers that execute a V3 script, not to all redeemers in
/// a transaction that happens to contain any V3 script.
///
/// Resolution logic (matching Haskell's `scriptsNeeded` / Cardano CDDL spec):
/// - `Spend` at index `i` → `i`-th input in `(txid, txix)` sorted order →
///   script hash from the payment credential of the input's UTxO address.
/// - `Mint` at index `i` → `i`-th policy ID in ascending BTreeMap order.
/// - `Cert` at index `i` → `i`-th certificate's script credential hash.
/// - `Reward` at index `i` → `i`-th withdrawal's script stake credential
///   (bytes 1..29 of the reward address).
/// - `Vote` at index `i` → `i`-th voter's script credential hash.
/// - `Propose` at index `i` → the proposal's `policy_hash` field.
///
/// If a redeemer cannot be resolved to a script (e.g. the UTxO is missing or
/// the credential is not a script credential) the entry is omitted from the
/// result.  The caller falls back to the permissive (non-Unit) check for
/// unresolved redeemers, which is the safe direction.
pub(crate) fn redeemer_script_version_map(
    tx: &Transaction,
    utxo_set: &dyn UtxoLookup,
    version_map: &HashMap<Hash28, u8>,
) -> HashMap<(u8, u32), u8> {
    use torsten_primitives::address::Address;

    let body = &tx.body;
    let mut result: HashMap<(u8, u32), u8> = HashMap::new();

    // ------------------------------------------------------------------ Spend
    // Inputs are sorted by (tx_hash, output_index) to determine the redeemer
    // index, matching Haskell's `toSortedList (txins txb)`.
    let mut sorted_inputs: Vec<_> = body.inputs.iter().collect();
    sorted_inputs.sort_by(|a, b| {
        a.transaction_id
            .cmp(&b.transaction_id)
            .then(a.index.cmp(&b.index))
    });
    for (idx, input) in sorted_inputs.iter().enumerate() {
        if let Some(utxo) = utxo_set.lookup(input) {
            let script_hash = match &utxo.address {
                Address::Base(b) => match &b.payment {
                    Credential::Script(h) => Some(*h),
                    _ => None,
                },
                Address::Enterprise(e) => match &e.payment {
                    Credential::Script(h) => Some(*h),
                    _ => None,
                },
                Address::Pointer(p) => match &p.payment {
                    Credential::Script(h) => Some(*h),
                    _ => None,
                },
                _ => None,
            };
            if let Some(hash) = script_hash {
                if let Some(&ver) = version_map.get(&hash) {
                    // Spend tag = 0
                    result.insert((0u8, idx as u32), ver);
                }
            }
        }
    }

    // ------------------------------------------------------------------ Mint
    // Policy IDs are iterated in BTreeMap ascending order, matching Haskell's
    // `Map.toAscList (txmint txb)`.
    for (idx, policy_id) in body.mint.keys().enumerate() {
        if let Some(&ver) = version_map.get(policy_id) {
            // Mint tag = 1
            result.insert((1u8, idx as u32), ver);
        }
    }

    // ------------------------------------------------------------------ Cert
    // Raw positional index (0-based) in the certificate list, matching
    // Haskell's `zip [0..] (txcerts txb)`.
    for (idx, cert) in body.certificates.iter().enumerate() {
        let script_hash: Option<Hash28> = match cert {
            Certificate::StakeDeregistration(Credential::Script(h)) => Some(*h),
            Certificate::StakeDelegation {
                credential: Credential::Script(h),
                ..
            } => Some(*h),
            Certificate::ConwayStakeDeregistration {
                credential: Credential::Script(h),
                ..
            } => Some(*h),
            Certificate::VoteDelegation {
                credential: Credential::Script(h),
                ..
            } => Some(*h),
            Certificate::StakeVoteDelegation {
                credential: Credential::Script(h),
                ..
            } => Some(*h),
            Certificate::RegStakeDeleg {
                credential: Credential::Script(h),
                ..
            } => Some(*h),
            Certificate::RegStakeVoteDeleg {
                credential: Credential::Script(h),
                ..
            } => Some(*h),
            Certificate::VoteRegDeleg {
                credential: Credential::Script(h),
                ..
            } => Some(*h),
            Certificate::CommitteeHotAuth {
                cold_credential: Credential::Script(h),
                ..
            } => Some(*h),
            Certificate::CommitteeColdResign {
                cold_credential: Credential::Script(h),
                ..
            } => Some(*h),
            Certificate::UnregDRep {
                credential: Credential::Script(h),
                ..
            } => Some(*h),
            Certificate::UpdateDRep {
                credential: Credential::Script(h),
                ..
            } => Some(*h),
            _ => None,
        };
        if let Some(hash) = script_hash {
            if let Some(&ver) = version_map.get(&hash) {
                // Cert tag = 2
                result.insert((2u8, idx as u32), ver);
            }
        }
    }

    // --------------------------------------------------------------- Reward
    // Withdrawals are iterated in BTreeMap ascending key order.  Script stake
    // credentials occupy bytes 1..29 of the reward address.  The header nibble
    // bit-4 distinguishes script (1) from key (0) credentials:
    //   0xF0/0xF1 = script stake credential on mainnet/testnet.
    for (idx, reward_addr) in body.withdrawals.keys().enumerate() {
        if reward_addr.len() < 29 {
            continue;
        }
        let header = reward_addr[0];
        // Bit 4 of the first byte: 1 = script credential.
        if (header & 0x10) == 0 {
            continue; // key credential — no redeemer required
        }
        let hash_bytes: &[u8] = &reward_addr[1..29];
        if let Ok(hash_arr) = <[u8; 28]>::try_from(hash_bytes) {
            let hash = Hash28::from_bytes(hash_arr);
            if let Some(&ver) = version_map.get(&hash) {
                // Reward tag = 3
                result.insert((3u8, idx as u32), ver);
            }
        }
    }

    // ------------------------------------------------------------------ Vote
    // Voters are iterated in BTreeMap<Voter, _> ascending order.  Only
    // `ConstitutionalCommittee(Script)` and `DRep(Script)` carry a script
    // credential.  `StakePool` voters are key hashes and are never scripts.
    for (idx, voter) in body.voting_procedures.keys().enumerate() {
        let script_hash: Option<Hash28> = match voter {
            Voter::ConstitutionalCommittee(Credential::Script(h))
            | Voter::DRep(Credential::Script(h)) => Some(*h),
            _ => None,
        };
        if let Some(hash) = script_hash {
            if let Some(&ver) = version_map.get(&hash) {
                // Vote tag = 4
                result.insert((4u8, idx as u32), ver);
            }
        }
    }

    // --------------------------------------------------------------- Propose
    // Only `ParameterChange` and `TreasuryWithdrawals` carry a `policy_hash`.
    // The policy hash IS the script hash (it is the hash of the constitution
    // script), so we look it up directly in version_map.
    for (idx, proposal) in body.proposal_procedures.iter().enumerate() {
        let policy_hash: Option<Hash28> = match &proposal.gov_action {
            GovAction::ParameterChange { policy_hash, .. } => *policy_hash,
            GovAction::TreasuryWithdrawals { policy_hash, .. } => *policy_hash,
            _ => None,
        };
        if let Some(hash) = policy_hash {
            if let Some(&ver) = version_map.get(&hash) {
                // Propose tag = 5
                result.insert((5u8, idx as u32), ver);
            }
        }
    }

    result
}
