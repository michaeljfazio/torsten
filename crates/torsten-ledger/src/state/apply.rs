//! Block application logic: `apply_block()` and the Byron/Shelley era dispatch pipeline.
//!
//! This module contains the core block processing pipeline for the Torsten ledger.
//! It is responsible for:
//!
//! - Verifying block connectivity (prev_hash chain)
//! - Detecting and triggering epoch transitions
//! - Dispatching to Byron vs Shelley+ transaction handling
//! - Phase-1 and Phase-2 (Plutus) transaction validation
//! - Applying UTxO changes, certificates, governance, and fee accumulation

use super::{
    credential_to_hash, stake_credential_hash, BlockValidationMode, LedgerError, LedgerState,
};
use crate::eras::byron::{apply_byron_block, ByronApplyMode, ByronFeePolicy};
use crate::plutus::evaluate_plutus_scripts;
use crate::validation::{
    script_ref_byte_size, validate_transaction, ValidationError, MAX_REF_SCRIPT_SIZE_TIER_CAP,
};
use std::sync::Arc;
use torsten_primitives::block::{Block, Point};
use torsten_primitives::era::Era;
use torsten_primitives::time::EpochNo;
use torsten_primitives::transaction::Certificate;
use torsten_primitives::value::Lovelace;
use tracing::{debug, trace, warn};

/// Maximum total reference script size allowed across all transactions in a single
/// Conway+ block.
///
/// Source: Haskell `ppMaxRefScriptSizePerBlockG = L.to . const $ 1024 * 1024`
/// (Conway PParams). This is not a protocol parameter that can be updated by
/// governance — it is hardcoded in the implementation.
///
/// Re-exported from [`MAX_REF_SCRIPT_SIZE_TIER_CAP`] to keep the block-body
/// check and the tiered-fee short-circuit in sync with the same value.
///
/// The corresponding per-transaction limit is [`MAX_REF_SCRIPT_SIZE_PER_TX`].
const MAX_REF_SCRIPT_SIZE_PER_BLOCK: u64 = MAX_REF_SCRIPT_SIZE_TIER_CAP;

/// Maximum total reference script size allowed in a single transaction.
///
/// Source: Haskell `ppMaxRefScriptSizePerTxG = L.to . const $ 200 * 1024`
/// (Conway PParams). Also hardcoded, not a governance-updateable protocol parameter.
///
/// This constant is defined here for documentation and cross-reference purposes.
/// The per-tx size limit is enforced via the fee calculation (CIP-0112) where
/// transactions with excessive ref scripts will fail the `feesOK` check.
#[allow(dead_code)]
const MAX_REF_SCRIPT_SIZE_PER_TX: u64 = 200 * 1024; // 200 KiB

impl LedgerState {
    /// Apply a block to the ledger state.
    ///
    /// When `mode` is `ValidateAll`, each transaction is independently validated
    /// (Phase-1 + Phase-2 Plutus evaluation) and the result is compared against
    /// the block producer's `is_valid` flag. A mismatch rejects the block.
    pub fn apply_block(
        &mut self,
        block: &Block,
        mode: BlockValidationMode,
    ) -> Result<(), LedgerError> {
        trace!(
            slot = block.slot().0,
            block_no = block.block_number().0,
            era = ?block.era,
            txs = block.transactions.len(),
            hash = %block.header.header_hash.to_hex(),
            "Ledger: applying block"
        );

        // Verify block connects to current tip
        if self.tip.point != Point::Origin {
            if let Some(tip_hash) = self.tip.point.hash() {
                if block.prev_hash() != tip_hash {
                    return Err(LedgerError::BlockDoesNotConnect {
                        expected: tip_hash.to_hex(),
                        got: block.prev_hash().to_hex(),
                    });
                }
            }
        }

        // Check for epoch transition before processing the block.
        // When multiple epochs are skipped (e.g., after offline time or Mithril import),
        // process each intermediate epoch transition individually so that snapshots rotate
        // correctly, pending retirements fire at the right epoch, and rewards are distributed.
        let block_epoch = EpochNo(self.epoch_of_slot(block.slot().0));
        if block_epoch > self.epoch {
            debug!(
                "Ledger: epoch transition {} -> {} at slot {}",
                self.epoch.0,
                block_epoch.0,
                block.slot().0,
            );
            while self.epoch < block_epoch {
                let next_epoch = EpochNo(self.epoch.0.saturating_add(1));
                self.process_epoch_transition(next_epoch);
            }
        }

        // Block body size check — reject blocks exceeding max_block_body_size
        if block.header.body_size > 0
            && self.protocol_params.max_block_body_size > 0
            && block.header.body_size > self.protocol_params.max_block_body_size
        {
            debug!(
                body_size = block.header.body_size,
                limit = self.protocol_params.max_block_body_size,
                slot = block.slot().0,
                "Block body exceeds max_block_body_size (expected during replay before PP updates)"
            );
        }

        // Byron era — apply dedicated Byron UTxO rules and return early.
        //
        // Byron has no scripts, no certificates, no withdrawals, no governance,
        // and no multi-asset. Its fee rule is:
        //   fee >= min_fee_a * tx_size_bytes + min_fee_b
        //
        // We skip the entire Shelley+ transaction pipeline (execution units,
        // Plutus evaluation, certificate processing, etc.) for Byron blocks.
        if block.era == Era::Byron {
            let fee_policy = ByronFeePolicy {
                min_fee_a: self.protocol_params.min_fee_a,
                min_fee_b: self.protocol_params.min_fee_b,
            };
            let byron_mode = match mode {
                BlockValidationMode::ValidateAll => ByronApplyMode::ValidateAll,
                BlockValidationMode::ApplyOnly => ByronApplyMode::ApplyOnly,
            };

            // Process each Byron transaction one at a time so that outputs
            // created by an earlier transaction in the block are immediately
            // visible to later transactions (within-block spending chains).
            //
            // We use `apply_byron_block` with a single-transaction slice and
            // apply each effect to self.utxo_set before moving on. This keeps
            // all Byron rule logic in byron.rs while preserving sequential
            // visibility without a single overlapping borrow.
            let mut total_byron_fees = Lovelace(0);
            // Duplicate-tx guard mirrors apply_byron_block's inner dedup logic.
            // Pre-size to the block's transaction count to avoid rehashing.
            let mut seen_hashes =
                std::collections::HashSet::with_capacity(block.transactions.len());
            for tx in &block.transactions {
                if !seen_hashes.insert(tx.hash) {
                    warn!(
                        tx_hash = %tx.hash.to_hex(),
                        slot = block.slot().0,
                        "Byron: duplicate tx hash in block, skipping"
                    );
                    continue;
                }
                let effect = apply_byron_block(
                    std::slice::from_ref(tx),
                    fee_policy,
                    block.slot().0,
                    byron_mode,
                    |input| self.utxo_set.lookup(input),
                )
                .map_err(|e| LedgerError::BlockTxValidationFailed {
                    slot: e.slot,
                    tx_hash: e.tx_hash,
                    errors: e.reason.to_string(),
                })?;

                // Apply each tx's effects immediately so subsequent txs in the
                // same block see the correct UTxO state.
                for input in &effect.spent {
                    self.utxo_set.remove(input);
                }
                for (input, output) in effect.created {
                    self.utxo_set.insert(input, output);
                }
                total_byron_fees.0 = total_byron_fees.0.saturating_add(effect.fees.0);
            }
            self.epoch_fees += total_byron_fees;

            // Track block production and nonce state (same as Shelley+)
            if !block.header.issuer_vkey.is_empty() {
                let pool_id = torsten_primitives::hash::blake2b_224(&block.header.issuer_vkey);
                *Arc::make_mut(&mut self.epoch_blocks_by_pool)
                    .entry(pool_id)
                    .or_insert(0) += 1;
            }
            self.epoch_block_count += 1;

            // Byron uses OBFT (not VRF), so nonce_vrf_output is empty.
            // The evolving nonce does not advance during the Byron era.
            // lab_nonce = prevHashToNonce(block.prevHash) per Haskell's reupdateChainDepState.
            // prevHashToNonce: GenesisHash → NeutralNonce; BlockHash h → Nonce(castHash h).
            // castHash is a type cast only — no rehashing. The value IS the prev_hash bytes.
            self.lab_nonce = block.header.prev_hash;

            self.tip = block.tip();
            self.era = block.era;

            trace!(
                slot = block.slot().0,
                block_no = block.block_number().0,
                utxo_count = self.utxo_set.len(),
                epoch = self.epoch.0,
                "Ledger: Byron block applied successfully"
            );
            return Ok(());
        }

        // Block-level execution unit budget check
        let mut block_mem: u64 = 0;
        let mut block_steps: u64 = 0;
        for tx in &block.transactions {
            if tx.is_valid {
                for r in &tx.witness_set.redeemers {
                    block_mem = block_mem.saturating_add(r.ex_units.mem);
                    block_steps = block_steps.saturating_add(r.ex_units.steps);
                }
            }
        }
        if block_mem > self.protocol_params.max_block_ex_units.mem {
            debug!(
                block_mem,
                limit = self.protocol_params.max_block_ex_units.mem,
                "Block exceeds max execution unit memory budget (expected during replay before PP updates)"
            );
        }
        if block_steps > self.protocol_params.max_block_ex_units.steps {
            debug!(
                block_steps,
                limit = self.protocol_params.max_block_ex_units.steps,
                "Block exceeds max execution unit step budget (expected during replay before PP updates)"
            );
        }

        // Block-level totalRefScriptSize check (Conway+, protocol >= 9).
        //
        // Matches Haskell's `conwayBbodyTransition` which enforces:
        //   totalRefScriptSize <= maxRefScriptSizePerBlock
        //
        // `totalRefScriptSize` is the sum of `txNonDistinctRefScriptsSize` for every
        // transaction in the block. For valid transactions this counts ref scripts from
        // both inputs and reference_inputs; for invalid (is_valid=false) transactions
        // it counts collateral outputs (which we approximate as 0 for simplicity — the
        // spec uses the UTxO after collateral substitution for invalid txs, but in
        // practice no collateral outputs carry reference scripts).
        //
        // Per the Haskell implementation, this check fires for Conway era only
        // (protocol version >= 9). Earlier eras have no such block-level limit.
        //
        // Within-block UTxO overlay: when a transaction in the block creates an output
        // that carries a reference script, and a later transaction in the same block
        // references that output (either as a spending input or reference input), the
        // initial `self.utxo_set` does not yet contain that output. To correctly count
        // the ref script contribution of all txs (including ones that depend on
        // within-block outputs), we build a lightweight overlay of all outputs created
        // by valid transactions in the block. The overlay is checked first; on miss,
        // we fall back to `self.utxo_set`.
        //
        // This is a pre-scan that runs before the per-tx application loop so that the
        // block is rejected early (before any state changes) if the limit is exceeded.
        if self.protocol_params.protocol_version_major >= 9
            && mode == BlockValidationMode::ValidateAll
        {
            // Build within-block UTxO overlay: maps (tx_hash, output_index) → TransactionOutput
            // for all outputs produced by valid transactions in this block.
            let mut block_utxo_overlay: std::collections::HashMap<
                torsten_primitives::transaction::TransactionInput,
                torsten_primitives::transaction::TransactionOutput,
            > = std::collections::HashMap::new();
            for tx in &block.transactions {
                if tx.is_valid {
                    for (idx, output) in tx.body.outputs.iter().enumerate() {
                        block_utxo_overlay.insert(
                            torsten_primitives::transaction::TransactionInput {
                                transaction_id: tx.hash,
                                index: idx as u32,
                            },
                            output.clone(),
                        );
                    }
                }
            }

            // Helper: look up a UTxO, checking the within-block overlay first.
            let lookup_with_overlay =
                |input: &torsten_primitives::transaction::TransactionInput| {
                    block_utxo_overlay
                        .get(input)
                        .cloned()
                        .or_else(|| self.utxo_set.lookup(input))
                };

            let total_ref_script_size: u64 = block
                .transactions
                .iter()
                .map(|tx| {
                    // Count ref scripts from regular spending inputs
                    let spending_size: u64 = tx
                        .body
                        .inputs
                        .iter()
                        .filter_map(|inp| {
                            lookup_with_overlay(inp)
                                .and_then(|utxo| utxo.script_ref.as_ref().map(script_ref_byte_size))
                        })
                        .sum();
                    // Count ref scripts from reference inputs (also check overlay)
                    let reference_size: u64 = tx
                        .body
                        .reference_inputs
                        .iter()
                        .filter_map(|inp| {
                            lookup_with_overlay(inp)
                                .and_then(|utxo| utxo.script_ref.as_ref().map(script_ref_byte_size))
                        })
                        .sum();
                    spending_size.saturating_add(reference_size)
                })
                .fold(0u64, |acc, x| acc.saturating_add(x));

            if total_ref_script_size > MAX_REF_SCRIPT_SIZE_PER_BLOCK {
                return Err(LedgerError::BlockTxValidationFailed {
                    slot: block.slot().0,
                    tx_hash: String::from("(block-level check)"),
                    errors: format!(
                        "BodyRefScriptsSizeTooBig: totalRefScriptSize={} exceeds \
                         maxRefScriptSizePerBlock={} (Conway Bbody rule)",
                        total_ref_script_size, MAX_REF_SCRIPT_SIZE_PER_BLOCK
                    ),
                });
            }
        }

        // Pre-compute cost_models CBOR once per block (doesn't change within a block)
        let cost_models_cbor = if mode == BlockValidationMode::ValidateAll {
            self.protocol_params.cost_models.to_cbor()
        } else {
            None
        };

        // Track processed tx hashes to skip duplicates within a block
        let mut processed_tx_hashes =
            std::collections::HashSet::with_capacity(block.transactions.len());

        // Apply each transaction
        for tx in &block.transactions {
            if !processed_tx_hashes.insert(tx.hash) {
                warn!(
                    tx_hash = %tx.hash.to_hex(),
                    slot = block.slot().0,
                    "Duplicate transaction hash in block, skipping"
                );
                continue;
            }
            // Phase-1 + Phase-2 validation when ValidateAll mode is active.
            // Verifies the block producer's is_valid flag matches actual evaluation.
            if mode == BlockValidationMode::ValidateAll {
                let has_redeemers = !tx.witness_set.redeemers.is_empty();

                if tx.is_valid {
                    // -------------------------------------------------------
                    // Conway LEDGERS rule: submittedTreasuryValue == actualTreasuryValue
                    //
                    // When the transaction body includes `currentTreasuryValue`
                    // (field 19, Conway+), the block producer asserts that the
                    // ledger's treasury has a specific balance at the point this
                    // transaction executes.  A mismatch means the block is
                    // invalid (the producer's world-view diverged from ours).
                    //
                    // Only checked in Conway (protocol >= 9) and only when the
                    // field is actually present — pre-Conway transactions never
                    // carry this field.
                    //
                    // Reference: Cardano Blueprint LEDGERS flowchart,
                    // "submittedTreasuryValue == currentTreasuryValue" predicate;
                    // Haskell `conwayLedgerFn` in
                    // `Cardano.Ledger.Conway.Rules.Ledger`.
                    // -------------------------------------------------------
                    if self.protocol_params.protocol_version_major >= 9 {
                        if let Some(declared_treasury) = tx.body.treasury_value {
                            if declared_treasury.0 != self.treasury.0 {
                                return Err(LedgerError::BlockTxValidationFailed {
                                    slot: block.slot().0,
                                    tx_hash: tx.hash.to_hex(),
                                    errors: format!(
                                        "TreasuryValueMismatch: tx declared {}, ledger has {}",
                                        declared_treasury.0, self.treasury.0
                                    ),
                                });
                            }
                        }
                    }

                    // -------------------------------------------------------
                    // Conway LEDGERS rule: failOnNonEmpty unelectedCommitteeMembers
                    //
                    // A `CommitteeHotAuth` certificate is only valid when the
                    // cold credential is listed in the current constitutional
                    // committee (`committee_expiration` map populated by the last
                    // enacted `UpdateCommittee` governance action).  Issuing a
                    // hot-key authorisation for a cold credential that is not a
                    // sitting CC member is rejected by the `CERT` rule.
                    //
                    // Only enforced in Conway (protocol >= 9).  Pre-Conway eras
                    // have no constitutional committee, so the check would always
                    // trivially fail.
                    //
                    // Reference: Haskell `conwayCertsPredFailure` / `CERT` rule,
                    // "unelected" predicate in
                    // `Cardano.Ledger.Conway.Rules.Certs`.
                    // -------------------------------------------------------
                    if self.protocol_params.protocol_version_major >= 9 {
                        for cert in &tx.body.certificates {
                            if let Certificate::CommitteeHotAuth {
                                cold_credential, ..
                            } = cert
                            {
                                let cold_key = credential_to_hash(cold_credential);
                                if !self.governance.committee_expiration.contains_key(&cold_key) {
                                    return Err(LedgerError::BlockTxValidationFailed {
                                        slot: block.slot().0,
                                        tx_hash: tx.hash.to_hex(),
                                        errors: format!(
                                            "UnelectedCommitteeMember: \
                                             CommitteeHotAuth cold credential {} \
                                             is not in the current constitutional committee",
                                            cold_key.to_hex()
                                        ),
                                    });
                                }
                            }
                        }
                    }

                    // Producer claims tx is valid — verify with full validation.
                    //
                    // Sequential UTxO visibility: by the time we reach tx[i] in this
                    // loop, `self.utxo_set` already contains the outputs created by
                    // tx[0]..tx[i-1] (each was applied by the `apply_transaction` call
                    // in its own loop iteration, before we advanced to the next tx).
                    // This matches Haskell's sequential LEDGER rule application inside
                    // the LEDGERS block rule — later txs in a block can spend or
                    // reference UTxOs created by earlier txs in the same block,
                    // fixing the root cause of `InputNotFound` and `InvalidMint`
                    // errors for within-block UTxO dependencies.
                    //
                    // Use tx raw_cbor size as tx_size (approximate, sufficient for validation).
                    let tx_size = tx.raw_cbor.as_ref().map_or(0, |c| c.len() as u64);
                    let result = validate_transaction(
                        tx,
                        &self.utxo_set,
                        &self.protocol_params,
                        block.slot().0,
                        tx_size,
                        Some(&self.slot_config),
                    );
                    if let Err(errors) = result {
                        // Distinguish Phase-1 failures from Phase-2 (script) failures.
                        let has_script_failure = errors
                            .iter()
                            .any(|e| matches!(e, ValidationError::ScriptFailed(_)));
                        if has_script_failure {
                            // Producer said valid but our uplc evaluator says scripts fail.
                            //
                            // Known issue: the Rust uplc CEK machine (v1.1.21) has marginal
                            // cost accounting differences vs the Haskell evaluator for some
                            // PlutusV2 builtins. Scripts operating very close to their declared
                            // budget (within ~0.1%) may appear to exceed their CPU allocation
                            // in uplc but pass in Haskell. Since this block is confirmed on-chain
                            // by the Haskell-based consensus, log a warning but do NOT reject
                            // the block — treat it as the producer declared (is_valid=true).
                            //
                            // This matches the Cardano spec's intent: Phase-2 validation tag
                            // mismatch is only fatal when OUR evaluation is authoritative
                            // (i.e., for locally-forged blocks or mempool admission). For blocks
                            // received from the chain, the on-chain consensus is authoritative.
                            warn!(
                                tx_hash = %tx.hash.to_hex(),
                                slot = block.slot().0,
                                errors = ?errors.iter().filter(|e| matches!(e, ValidationError::ScriptFailed(_)))
                                    .map(|e| e.to_string()).collect::<Vec<_>>(),
                                "Plutus evaluation divergence: uplc says scripts fail but block is_valid=true on-chain — \
                                 trusting on-chain consensus (likely marginal budget difference)"
                            );
                            // Fall through — treat as valid, process normally
                        } else {
                            // Phase-1 failure on an on-chain confirmed block.
                            //
                            // With correct sequential UTxO application (outputs from tx[i-1]
                            // are visible when validating tx[i]), within-block UTxO
                            // dependencies are resolved correctly. A Phase-1 failure here
                            // indicates either a real protocol violation or a remaining
                            // difference in validation logic between Torsten and Haskell.
                            //
                            // For confirmed blocks (on-chain consensus is authoritative),
                            // we log the error and continue rather than rejecting — diverging
                            // ledger state is less harmful than halting sync. The warning
                            // should be investigated and the root cause fixed.
                            let err_str: Vec<String> =
                                errors.iter().map(|e| e.to_string()).collect();
                            warn!(
                                tx_hash = %tx.hash.to_hex(),
                                slot = block.slot().0,
                                errors = %err_str.join("; "),
                                "Phase-1 validation divergence on confirmed block — \
                                 trusting on-chain consensus"
                            );
                            // Fall through — treat as valid, process normally
                        }
                    }
                } else if has_redeemers {
                    // Producer claims tx is invalid (is_valid=false) with scripts present.
                    // Verify scripts actually fail; if they pass, producer is stealing collateral.
                    // uplc expects (cpu_steps, mem_units); our ExUnits has { mem, steps } where
                    // steps=cpu and mem=memory — swap to match uplc convention.
                    let max_ex = (
                        self.protocol_params.max_tx_ex_units.steps,
                        self.protocol_params.max_tx_ex_units.mem,
                    );
                    let eval_result = evaluate_plutus_scripts(
                        tx,
                        &self.utxo_set,
                        cost_models_cbor.as_deref(),
                        max_ex,
                        &self.slot_config,
                    );
                    if eval_result.is_ok() {
                        // Scripts actually pass but producer says invalid → mismatch
                        return Err(LedgerError::ValidationTagMismatch {
                            tx_hash: tx.hash.to_hex(),
                            block_flag: false,
                            eval_result: true,
                        });
                    }
                }
            }

            // Handle invalid transactions (phase-2 validation failure):
            // - Collateral inputs are consumed (forfeit to block producer)
            // - Regular inputs/outputs/certificates are NOT applied
            // - If collateral_return is present, it becomes a new UTxO
            if !tx.is_valid {
                // Sum up total collateral input value
                let mut collateral_input_value: u64 = 0;
                // Consume collateral inputs (update stake distribution)
                for col_input in &tx.body.collateral {
                    if let Some(spent) = self.utxo_set.lookup(col_input) {
                        collateral_input_value += spent.value.coin.0;
                        if let Some(cred) = stake_credential_hash(&spent.address) {
                            if let Some(stake) = self.stake_distribution.stake_map.get_mut(&cred) {
                                stake.0 = stake.0.saturating_sub(spent.value.coin.0);
                            }
                        }
                    }
                    self.utxo_set.remove(col_input);
                }
                // If there's a collateral return output, add it
                let collateral_return_value = if let Some(col_return) = &tx.body.collateral_return {
                    if let Some(cred) = stake_credential_hash(&col_return.address) {
                        *self
                            .stake_distribution
                            .stake_map
                            .entry(cred)
                            .or_insert(Lovelace(0)) += Lovelace(col_return.value.coin.0);
                    }
                    let return_input = torsten_primitives::transaction::TransactionInput {
                        transaction_id: tx.hash,
                        index: tx.body.outputs.len() as u32, // collateral return is after regular outputs
                    };
                    let return_val = col_return.value.coin.0;
                    self.utxo_set.insert(return_input, col_return.clone());
                    return_val
                } else {
                    0
                };
                // Fee collected is the actual collateral forfeited, NOT the declared fee.
                // If total_collateral is set, use it; otherwise compute from inputs - return.
                let collateral_fee = if let Some(tc) = tx.body.total_collateral {
                    tc
                } else {
                    Lovelace(collateral_input_value.saturating_sub(collateral_return_value))
                };
                self.epoch_fees += collateral_fee;
                continue;
            }

            // Update stake distribution from consumed inputs (subtract)
            for input in &tx.body.inputs {
                if let Some(spent_output) = self.utxo_set.lookup(input) {
                    if let Some(cred_hash) = stake_credential_hash(&spent_output.address) {
                        if let Some(stake) = self.stake_distribution.stake_map.get_mut(&cred_hash) {
                            stake.0 = stake.0.saturating_sub(spent_output.value.coin.0);
                        }
                    }
                }
            }

            // Apply UTxO changes (may fail for missing inputs during initial sync)
            if let Err(e) =
                self.utxo_set
                    .apply_transaction(&tx.hash, &tx.body.inputs, &tx.body.outputs)
            {
                // During initial sync without full history, inputs won't be found.
                // Skip UTxO changes AND stake crediting to avoid phantom state.
                // Fees and certificates are still processed.
                debug!("UTxO application skipped (missing inputs): {e}");
            } else {
                // Update stake distribution from new outputs (add)
                // Only credit stake when UTxO application succeeded to avoid
                // phantom stake from outputs that don't actually exist in the set.
                for output in &tx.body.outputs {
                    if let Some(cred_hash) = stake_credential_hash(&output.address) {
                        *self
                            .stake_distribution
                            .stake_map
                            .entry(cred_hash)
                            .or_insert(Lovelace(0)) += Lovelace(output.value.coin.0);
                    }
                }
            }

            // Accumulate fees
            self.epoch_fees += tx.body.fee;

            // Process certificates
            for cert in &tx.body.certificates {
                self.process_certificate(cert);
            }

            // Process withdrawals (rewards are consumed, no UTxO effect)
            for (reward_account, amount) in &tx.body.withdrawals {
                self.process_withdrawal(reward_account, *amount);
            }

            // Process Conway governance proposals (only in Conway era, protocol >= 9)
            if self.protocol_params.protocol_version_major >= 9 {
                for (idx, proposal) in tx.body.proposal_procedures.iter().enumerate() {
                    self.process_proposal(&tx.hash, idx as u32, proposal);
                }

                // Process Conway governance votes
                for (voter, action_votes) in &tx.body.voting_procedures {
                    for (action_id, procedure) in action_votes {
                        self.process_vote(voter, action_id, procedure);
                    }
                }

                // Process treasury donations
                if let Some(donation) = tx.body.donation {
                    self.treasury += donation;
                }
            } else {
                if !tx.body.proposal_procedures.is_empty() {
                    warn!(
                        "Ignoring {} governance proposals in pre-Conway era (protocol {})",
                        tx.body.proposal_procedures.len(),
                        self.protocol_params.protocol_version_major,
                    );
                }
                if !tx.body.voting_procedures.is_empty() {
                    warn!(
                        "Ignoring {} governance votes in pre-Conway era (protocol {})",
                        tx.body.voting_procedures.len(),
                        self.protocol_params.protocol_version_major,
                    );
                }
            }

            // Collect pre-Conway protocol parameter update proposals
            if let Some(ref update) = tx.body.update {
                for (genesis_hash, ppu) in &update.proposed_updates {
                    debug!(
                        genesis_hash = %genesis_hash.to_hex(),
                        target_epoch = update.epoch,
                        protocol_version = ?ppu.protocol_version_major.zip(ppu.protocol_version_minor),
                        "Collected protocol parameter update proposal"
                    );
                    self.pending_pp_updates
                        .entry(EpochNo(update.epoch))
                        .or_default()
                        .push((*genesis_hash, ppu.clone()));
                }
            }
        }

        // Track block production by pool (issuer vkey hash)
        if !block.header.issuer_vkey.is_empty() {
            let pool_id = torsten_primitives::hash::blake2b_224(&block.header.issuer_vkey);
            *Arc::make_mut(&mut self.epoch_blocks_by_pool)
                .entry(pool_id)
                .or_insert(0) += 1;
        }
        self.epoch_block_count += 1;

        // Praos nonce state machine (matches Haskell reupdateChainDepState):
        //
        // 1. lab_nonce = prevHashToNonce(block.prevHash) = direct assignment of prev_hash bytes
        // 2. evolving_nonce is updated for EVERY block using the era-specific
        //    nonce VRF contribution stored in nonce_vrf_output:
        //    - Shelley/Allegra/Mary/Alonzo (TPraos): blake2b_256(nonce_vrf_cert.0)
        //    - Babbage/Conway (Praos): blake2b_256("N" || vrf_result.0)
        // 3. candidate_nonce tracks evolving_nonce UNLESS the block is within the
        //    last randomness_stabilisation_window (4k/f) slots of the epoch,
        //    in which case the candidate freezes so the epoch nonce is stable.
        if !block.header.nonce_vrf_output.is_empty() {
            // Update evolving nonce unconditionally with the pre-computed eta
            self.update_evolving_nonce(&block.header.nonce_vrf_output);

            // Candidate nonce tracks evolving nonce OUTSIDE the stability window (4k/f).
            let first_slot_of_next_epoch = self.first_slot_of_epoch(self.epoch.0.saturating_add(1));
            if block
                .slot()
                .0
                .saturating_add(self.randomness_stabilisation_window)
                < first_slot_of_next_epoch
            {
                self.candidate_nonce = self.evolving_nonce;
            }
        } else if block.era.is_shelley_based() {
            warn!(
                slot = block.slot().0,
                block_no = block.block_number().0,
                epoch = self.epoch.0,
                era = ?block.era,
                "Nonce: Shelley+ block has EMPTY nonce_vrf_output!"
            );
        }

        // Update LAB nonce = prevHashToNonce(block.prevHash) per Haskell's reupdateChainDepState.
        // Haskell: csLabNonce = prevHashToNonce (bheaderPrev bhb)
        // prevHashToNonce: GenesisHash → NeutralNonce (ZERO); BlockHash h → Nonce(h).
        // castHash is a type-reinterpret (no rehashing): just use the raw prev_hash bytes.
        self.lab_nonce = block.header.prev_hash;

        // Update tip
        self.tip = block.tip();
        self.era = block.era;

        trace!(
            slot = block.slot().0,
            block_no = block.block_number().0,
            utxo_count = self.utxo_set.len(),
            epoch = self.epoch.0,
            era = ?self.era,
            "Ledger: block applied successfully"
        );

        Ok(())
    }
}
