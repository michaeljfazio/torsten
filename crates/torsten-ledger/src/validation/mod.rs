//! Transaction validation — Phase-1 and Phase-2.
//!
//! This module is the public surface of the validation subsystem. It:
//! - Defines [`ValidationError`], the unified error type for all validation rules.
//! - Provides [`validate_transaction`] and [`validate_transaction_with_pools`] as
//!   the sole public entry points.
//! - Re-exports [`evaluate_native_script`] for callers that need to evaluate
//!   native scripts outside of full transaction validation (e.g. mempool admission).
//!
//! Internal rule logic is split across focused sub-modules:
//! - [`phase1`]    — Rules 1–10, 13–14 (structural/witness rules)
//! - [`collateral`] — Rules 11, 11b, 11c (collateral for Plutus transactions)
//! - [`scripts`]   — Rule 12 + script hash utilities + native script evaluation
//! - [`conway`]    — Era-gating checks + deposit/refund accounting

mod collateral;
mod conway;
mod datum;
mod phase1;
mod scripts;

#[cfg(test)]
mod tests;

pub use scripts::evaluate_native_script;
// Re-exported for use by the block-application layer (block-level ref script
// size check in state/apply.rs — Haskell's `conwayBbodyTransition`).
pub(crate) use scripts::script_ref_byte_size;
// Re-export the tier cap so apply.rs can reuse the same constant for the
// block-body check, keeping the tiered-fee short-circuit in sync.
pub(crate) use scripts::MAX_REF_SCRIPT_SIZE_TIER_CAP;
// Re-exported for use by the block-application layer (per-transaction 200 KiB
// ref script size check — Haskell's `ppMaxRefScriptSizePerTxG` enforcement).
pub(crate) use scripts::calculate_ref_script_size;
// Re-exported for use by plutus.rs (V3 non-Unit return value check): maps
// script hashes to their language version so the evaluator can apply the
// correct success predicate per-result.
pub(crate) use collateral::plutus_script_version_map;
// Re-exported for use by plutus.rs (per-redeemer V3 Unit-return check): maps
// (redeemer_tag_byte, index) to the language version of the script that
// redeemer executes, allowing the Unit check to be applied only to V3 redeemers.
pub(crate) use collateral::redeemer_script_version_map;

use std::collections::HashSet;

use torsten_primitives::hash::Hash28;
use torsten_primitives::protocol_params::ProtocolParameters;
use torsten_primitives::transaction::Transaction;
use tracing::{debug, trace, warn};

use crate::plutus::{evaluate_plutus_scripts, SlotConfig};
use crate::utxo::UtxoSet;

// ---------------------------------------------------------------------------
// Public error type
// ---------------------------------------------------------------------------

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
    /// A script-locked withdrawal or Plutus minting policy has no matching
    /// redeemer of the required tag/index.
    ///
    /// Mirrors Haskell's `scriptsNeeded` check: every entry in the `Reward`
    /// and `Mint` buckets that corresponds to a Plutus script must have an
    /// explicit redeemer at the correct sorted position.
    #[error("Missing {tag} redeemer at index {index}")]
    MissingRedeemer { tag: String, index: u32 },
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
    #[error("Value overflow in transaction accounting")]
    ValueOverflow,
    #[error("Era gating violation: {certificate_type} requires {required_era}, current era is {current_era}")]
    EraGatingViolation {
        certificate_type: String,
        required_era: String,
        current_era: String,
    },
    #[error("Governance feature requires Conway era (protocol >= 9), current protocol version is {current_version}")]
    GovernancePreConway { current_version: u64 },
    /// Conway LEDGERS rule: the block producer's declared treasury value in the
    /// transaction body (`currentTreasuryValue`, field 19) must match the
    /// ledger's tracked treasury balance exactly.
    ///
    /// Reference: Cardano Blueprint `LEDGERS` flowchart, "submittedTreasuryValue
    /// == currentTreasuryValue" predicate.
    #[error("Treasury value mismatch: tx declared {declared}, ledger has {actual}")]
    TreasuryValueMismatch { declared: u64, actual: u64 },
    /// Conway LEDGERS rule: the `CommitteeHotAuth` certificate's cold credential
    /// must correspond to a member currently elected to the constitutional
    /// committee (`committee_expiration` map).  Authorising a hot key for an
    /// unrecognised cold credential is rejected ("failOnNonEmpty unelected").
    ///
    /// Reference: Cardano ledger `conwayWitsVKeyNeeded` / `CERT` rule,
    /// "ccHotKeyOK" predicate from the Haskell implementation.
    #[error("CommitteeHotAuth cold credential is not a current CC member: {cold_credential_hash}")]
    UnelectedCommitteeMember { cold_credential_hash: String },
    /// Alonzo/Conway Phase-1 rule: a script-locked spending input carries a
    /// `DatumHash` in its UTxO but no corresponding datum bytes were supplied
    /// in `tx.witness_set.plutus_data`.
    ///
    /// Per Haskell's `checkWitnessesShelley` / Alonzo `UTXOW` rule
    /// "witsVKeyNeeded" extended with "reqSignerHashes" — every non-inline
    /// datum referenced by a script-locked input MUST be provided as a witness.
    #[error("Missing datum witness for script-locked input: datum hash {0}")]
    MissingDatumWitness(String),
    /// Alonzo/Conway Phase-1 rule: a datum supplied in
    /// `tx.witness_set.plutus_data` is not needed by any script-locked input
    /// or referenced output, making the transaction malformed.
    ///
    /// Haskell rejects transactions with extraneous datums under the
    /// `UTXOW` predicate "allowedSupplementalDatums ⊇ suppliedDatums".
    #[error("Extra (unreferenced) datum witness in transaction: datum hash {0}")]
    ExtraDatumWitness(String),
    /// Conway rule: the total byte size of all reference scripts reachable
    /// from a single transaction's inputs and reference inputs must not exceed
    /// 200 KiB (`ppMaxRefScriptSizePerTxG`).
    ///
    /// Source: Haskell `ppMaxRefScriptSizePerTxG = L.to . const $ 200 * 1024`
    /// (Conway PParams). This is hardcoded, not a governance-updateable parameter.
    #[error(
        "Transaction reference script size {actual} exceeds per-transaction limit \
         {limit} bytes (Conway ppMaxRefScriptSizePerTxG)"
    )]
    TxRefScriptSizeTooLarge { actual: u64, limit: u64 },
}

// ---------------------------------------------------------------------------
// Public validation entry points
// ---------------------------------------------------------------------------

/// Validate a transaction against the current UTxO set and protocol parameters.
///
/// This is a convenience wrapper around [`validate_transaction_with_pools`] that
/// treats all pool registrations as new (no re-registration discount).
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
        None,
    )
}

/// Validate a transaction with an optional set of registered pools.
///
/// When `registered_pools` is `Some`, pool re-registrations (updating an existing
/// pool's parameters) do not charge an additional deposit — only new pool
/// registrations do. When `None`, all pool registrations are treated as new
/// (deposit always charged).
///
/// The validation pipeline is:
/// 1. Phase-1 structural rules (Rules 1–10, 13–14) via [`phase1::run_phase1_rules`].
/// 2. For Plutus transactions: collateral rules (Rules 11, 11b, 11c) and
///    script data hash (Rule 12).
/// 3. Phase-2 Plutus script execution when all Phase-1 checks pass and redeemers
///    are present.
#[allow(clippy::too_many_arguments)] // validation entry point legitimately needs all context parameters
pub fn validate_transaction_with_pools(
    tx: &Transaction,
    utxo_set: &UtxoSet,
    params: &ProtocolParameters,
    current_slot: u64,
    tx_size: u64,
    slot_config: Option<&SlotConfig>,
    registered_pools: Option<&HashSet<Hash28>>,
    current_treasury: Option<u64>,
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

    // ------------------------------------------------------------------
    // Phase-1 structural rules (Rules 1–10, 13–14)
    // ------------------------------------------------------------------
    phase1::run_phase1_rules(
        tx,
        utxo_set,
        params,
        current_slot,
        tx_size,
        registered_pools,
        &mut errors,
    );

    // ------------------------------------------------------------------
    // Conway LEDGER rule: currentTreasuryValue must match ledger treasury.
    // This prevents mempool admission of transactions with stale/wrong
    // treasury assertions, which would cause forged blocks to be rejected.
    // ------------------------------------------------------------------
    if params.protocol_version_major >= 9 {
        if let (Some(declared), Some(actual)) = (tx.body.treasury_value.as_ref(), current_treasury)
        {
            if declared.0 != actual {
                errors.push(ValidationError::TreasuryValueMismatch {
                    declared: declared.0,
                    actual,
                });
            }
        }
    }

    // ------------------------------------------------------------------
    // Rules 11, 11b, 11c, 12 — Plutus-transaction-specific checks
    //
    // These are only enforced when the transaction includes Plutus scripts
    // or redeemers. They are split into their own modules to keep the rule
    // logic focused and independently testable.
    // ------------------------------------------------------------------
    if scripts::has_plutus_scripts(tx) {
        // Rule 11: collateral inputs, percentage, net-ADA check, total_collateral
        // Rule 11b: redeemer index bounds
        collateral::check_collateral(tx, utxo_set, params, &mut errors);

        // Rule 11c: every script-locked input/withdrawal and every Plutus minting
        // policy must have a matching redeemer (Spend / Reward / Mint respectively).
        // Matches Haskell's `scriptsNeeded` check.
        collateral::check_script_redeemers(tx, utxo_set, &mut errors);

        // Rule 12: script data hash (mkScriptIntegrity) — covers redeemers,
        // datums, cost models, and language versions.
        scripts::check_script_data_hash(tx, utxo_set, params, &mut errors);

        // ------------------------------------------------------------------
        // Phase-2: Execute Plutus scripts when redeemers are present.
        //
        // Both `raw_cbor` and `slot_config` are required for Plutus evaluation.
        // A missing `raw_cbor` means the transaction was constructed locally
        // without being round-tripped through CBOR — that is a programming
        // error and must be surfaced. Silent bypass is not allowed.
        // ------------------------------------------------------------------
        let has_redeemers = !tx.witness_set.redeemers.is_empty();
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
                // uplc::tx::eval_phase_two_raw expects initial_budget as (cpu_steps, mem_units).
                // Our ExUnits struct uses { mem, steps } where mem=memory_units and steps=cpu_steps.
                // Swap the fields to match the uplc convention: (steps, mem) = (cpu, mem).
                let max_ex = (params.max_tx_ex_units.steps, params.max_tx_ex_units.mem);
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
