//! Collateral validation (Rule 11).
//!
//! Collateral is required for all transactions that include Plutus scripts.
//! This module implements:
//! - Presence and count checks for collateral inputs
//! - Lookup of collateral UTxOs
//! - Multi-asset net-token check (collateral net must be pure ADA)
//! - `total_collateral` declaration matching
//! - Minimum collateral percentage enforcement
//! - Per-transaction execution-unit limit check
//! - Redeemer index bounds check (Rule 11b)
//! - Missing Spend redeemer for script-locked inputs (Rule 11c)

use std::collections::{BTreeMap, HashSet};

use torsten_primitives::hash::PolicyId;
use torsten_primitives::protocol_params::ProtocolParameters;
use torsten_primitives::transaction::{RedeemerTag, Transaction};
use torsten_primitives::value::AssetName;

use crate::utxo::UtxoSet;

use super::ValidationError;

/// Validate all collateral-related rules for a Plutus transaction (Rule 11).
///
/// This function is only called when `has_plutus_scripts(tx)` is true.
pub(super) fn check_collateral(
    tx: &Transaction,
    utxo_set: &UtxoSet,
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

    // Effective collateral must be >= fee * collateral_percentage / 100
    let required_collateral = body.fee.0 * params.collateral_percentage / 100;
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
fn check_redeemer_indices(tx: &Transaction, errors: &mut Vec<ValidationError>) {
    let body = &tx.body;
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
            // Vote and Propose have dynamic counts not easily bounded here
            RedeemerTag::Vote => continue,
            RedeemerTag::Propose => continue,
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

/// Rule 11c: Every script-locked spending input must have a matching Spend
/// redeemer at the correct sorted index.
pub(super) fn check_spend_redeemers(
    tx: &Transaction,
    utxo_set: &UtxoSet,
    errors: &mut Vec<ValidationError>,
) {
    let body = &tx.body;
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
}
