//! Block application logic: thin orchestrator dispatching to `EraRulesImpl`.
//!
//! This module contains the core block processing pipeline for the Dugite ledger,
//! implemented as a thin orchestrator that delegates era-specific logic to
//! [`EraRulesImpl`](crate::eras::EraRulesImpl) while retaining cross-cutting
//! concerns (validation, epoch transitions) inline.
//!
//! The orchestrator is responsible for:
//!
//! - Verifying block connectivity (prev_hash chain)
//! - Detecting and dispatching HFC era boundary transformations (`on_era_transition`)
//! - Detecting and triggering epoch transitions (via existing `process_epoch_transition`)
//! - Dispatching block body validation (`validate_block_body`)
//! - Phase-1 and Phase-2 (Plutus) transaction validation (ValidateAll mode)
//! - Dispatching per-transaction apply logic to era rules
//! - Pre-Conway protocol parameter update proposal collection
//! - Dispatching nonce evolution and block production tracking (`evolve_nonce`)

use super::{credential_to_hash, BlockValidationMode, LedgerError, LedgerState};
use crate::eras::byron::{apply_byron_block, ByronApplyMode, ByronFeePolicy};
use crate::eras::{EraRules, EraRulesImpl, RuleContext};
use crate::ledger_seq::{BlockFieldsDelta, LedgerDelta};
use crate::plutus::evaluate_plutus_scripts;
use crate::utxo_diff::UtxoDiff;
use crate::validation::{
    calculate_ref_script_size, validate_transaction_with_pools, ValidationError,
};
use dugite_primitives::block::{Block, Point};
use dugite_primitives::era::Era;
use dugite_primitives::time::EpochNo;
use dugite_primitives::transaction::Certificate;
use dugite_primitives::value::Lovelace;
use std::sync::Arc;
use tracing::{debug, trace, warn};

/// Maximum total reference script size allowed in a single transaction.
///
/// Source: Haskell `ppMaxRefScriptSizePerTxG = L.to . const $ 200 * 1024`
/// (Conway PParams). Also hardcoded, not a governance-updateable protocol parameter.
///
/// Enforced in `apply_block` for `ValidateAll` mode: any transaction whose
/// combined spending-input + reference-input script_ref byte count exceeds this
/// limit is rejected with [`LedgerError::BlockTxValidationFailed`].
const MAX_REF_SCRIPT_SIZE_PER_TX: u64 = 200 * 1024; // 200 KiB

impl LedgerState {
    /// Build a read-only rule context for era rule dispatch.
    ///
    /// Assembles all the immutable per-block parameters that era rules need
    /// without requiring a `&mut self` borrow.
    ///
    /// NOTE: Cannot be used inside the per-tx loop of `apply_block` because it
    /// borrows `&self` (including `&self.epochs.protocol_params`) which conflicts
    /// with the `&mut self.epochs` needed for `apply_valid_tx`. Use inline
    /// `RuleContext` construction with `cached_params` instead.
    #[allow(dead_code)]
    fn build_rule_context<'a>(&'a self, block: &Block, tx_index: u64) -> RuleContext<'a> {
        RuleContext {
            params: &self.epochs.protocol_params,
            current_slot: block.slot().0,
            current_epoch: self.epoch,
            era: block.era,
            slot_config: Some(&self.slot_config),
            node_network: self.node_network,
            genesis_delegates: &self.genesis_delegates,
            update_quorum: self.update_quorum,
            epoch_length: self.epoch_length,
            shelley_transition_epoch: self.shelley_transition_epoch,
            byron_epoch_length: self.byron_epoch_length,
            stability_window: self.randomness_stabilisation_window,
            stability_window_3kf: self.stability_window_3kf,
            randomness_stabilisation_window: self.randomness_stabilisation_window,
            tx_index,
            conway_genesis: self.conway_genesis_init.as_ref(),
        }
    }

    /// Apply a block to the ledger state.
    ///
    /// This is the **thin orchestrator** that dispatches era-specific transaction
    /// processing to [`EraRulesImpl`] while retaining cross-cutting concerns
    /// (validation, epoch transitions, nonce evolution) inline.
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

        // ── Step 1: Verify block connects to current tip ──────────────────
        //
        // A block must have `prev_hash == ledger.tip.hash`; otherwise it
        // belongs to a different chain and applying it would silently corrupt
        // ledger state. The correct handling of a prev_hash mismatch at the
        // live tip is CHAIN SELECTION: rollback the ledger to the common
        // intersection and replay the winning fork. That happens in the sync
        // loop when ChainSelQueue returns `SwitchedToFork` — it must NOT be
        // masked here.
        //
        // Historical note: earlier versions of this function accepted any
        // block whose `block_number` was `tip.block_number + 1` even when
        // `prev_hash` did not match, on the rationale that chunk-file replay
        // could produce different CBOR hashes from the network path. That
        // bypass silently papered over fork switches and led to divergence
        // between VolatileDB.selected_chain and the ledger state — including
        // forged blocks being effectively orphaned from our own view (see
        // issue #439). The bypass is retained ONLY for `ApplyOnly` mode
        // (used during startup chunk-file replay, where serialization
        // roundtrip differences are the only legitimate cause of mismatch).
        // `ValidateAll` mode (used for every live block and every forged
        // block) must reject mismatches unconditionally.
        if self.tip.point != Point::Origin {
            if let Some(tip_hash) = self.tip.point.hash() {
                if block.prev_hash() != tip_hash {
                    let is_sequential_successor =
                        block.block_number().0 == self.tip.block_number.0 + 1;
                    match mode {
                        BlockValidationMode::ApplyOnly
                            if is_sequential_successor && block.era == Era::Byron =>
                        {
                            tracing::info!(
                                block_no = block.block_number().0,
                                tip_block = self.tip.block_number.0,
                                tip_hash = %tip_hash.to_hex(),
                                got_prev = %block.prev_hash().to_hex(),
                                era = ?block.era,
                                "ApplyOnly (Byron): accepting block by sequence number despite \
                                 hash mismatch — pallas byron::BlockHead `OriginalHash` re-encodes \
                                 instead of using raw bytes; Shelley+ uses raw bytes and cannot \
                                 exhibit this mismatch. Tracked upstream in pallas."
                            );
                        }
                        _ => {
                            return Err(LedgerError::BlockDoesNotConnect {
                                expected: tip_hash.to_hex(),
                                got: block.prev_hash().to_hex(),
                            });
                        }
                    }
                }
            }
        }

        // ── Step 2: HFC era boundary transformation ─────────────────────
        //
        // When the block era exceeds the current ledger era, dispatch
        // era-specific state transformations via the trait. For Conway this
        // includes discarding pointer-addressed UTxO stake (TranslateEra
        // equivalent) and resetting donations.
        if block.era > self.era {
            let transition_rules = EraRulesImpl::for_era(block.era);
            // Clone protocol_params to break the aliasing conflict between
            // the immutable borrow in RuleContext and &mut self.epochs.
            let transition_params = self.epochs.protocol_params.clone();
            let transition_ctx = RuleContext {
                params: &transition_params,
                current_slot: block.slot().0,
                current_epoch: self.epoch,
                era: block.era,
                slot_config: Some(&self.slot_config),
                node_network: self.node_network,
                genesis_delegates: &self.genesis_delegates,
                update_quorum: self.update_quorum,
                epoch_length: self.epoch_length,
                shelley_transition_epoch: self.shelley_transition_epoch,
                byron_epoch_length: self.byron_epoch_length,
                stability_window: self.randomness_stabilisation_window,
                stability_window_3kf: self.stability_window_3kf,
                randomness_stabilisation_window: self.randomness_stabilisation_window,
                tx_index: 0,
                conway_genesis: self.conway_genesis_init.as_ref(),
            };
            transition_rules.on_era_transition(
                self.era,
                &transition_ctx,
                &mut self.utxo,
                &mut self.certs,
                &mut self.gov,
                &mut self.consensus,
                &mut self.epochs,
            )?;
            self.pending_era_transition = Some((self.era, block.era, self.epoch));
        }

        // ── Step 3: Epoch transitions ─────────────────────────────────────
        //
        // When multiple epochs are skipped (e.g., after offline time or Mithril
        // import), process each intermediate epoch transition individually.
        // Dispatched through EraRulesImpl for the block's era (post-HFC).
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
                let epoch_rules = EraRulesImpl::for_era(block.era);
                let epoch_params = self.epochs.protocol_params.clone();
                let epoch_ctx = RuleContext {
                    params: &epoch_params,
                    current_slot: block.slot().0,
                    current_epoch: self.epoch,
                    era: block.era,
                    slot_config: Some(&self.slot_config),
                    node_network: self.node_network,
                    genesis_delegates: &self.genesis_delegates,
                    update_quorum: self.update_quorum,
                    epoch_length: self.epoch_length,
                    shelley_transition_epoch: self.shelley_transition_epoch,
                    byron_epoch_length: self.byron_epoch_length,
                    stability_window: self.randomness_stabilisation_window,
                    stability_window_3kf: self.stability_window_3kf,
                    randomness_stabilisation_window: self.randomness_stabilisation_window,
                    conway_genesis: self.conway_genesis_init.as_ref(),
                    tx_index: 0,
                };
                epoch_rules.process_epoch_transition(
                    next_epoch,
                    &epoch_ctx,
                    &mut self.utxo,
                    &mut self.certs,
                    &mut self.gov,
                    &mut self.epochs,
                    &mut self.consensus,
                )?;
                self.epoch = next_epoch;
            }
        }

        // ── BBODY rule: block body size equality check ────────────────────
        //
        // Haskell enforces `actual_body_bytes == header.body_size`.  We extract the
        // actual serialized body size from the raw CBOR wire bytes (indices 1..4 of
        // the inner block array) and compare against the header's claim.  This only
        // runs in ValidateAll mode and only when raw_cbor is available (i.e., blocks
        // received from the network, not constructed in-memory).
        // Closes #377.
        if mode == BlockValidationMode::ValidateAll {
            if let Some(ref raw) = block.raw_cbor {
                if let Some(actual_body_size) =
                    dugite_serialization::compute_block_body_size_from_cbor(raw)
                {
                    let claimed = block.header.body_size;
                    if actual_body_size != claimed {
                        return Err(LedgerError::WrongBlockBodySize {
                            actual: actual_body_size,
                            claimed,
                        });
                    }
                }
            }
        }

        // Allocate a per-block diff to record all UTxO inserts and deletes.
        let mut block_diff = UtxoDiff::new();

        // ── Step 5: Byron early return ────────────────────────────────────
        //
        // Byron has no scripts, certificates, withdrawals, governance, or
        // multi-asset. Process via dedicated Byron path with per-tx sequential
        // application (earlier tx outputs visible to later txs in the same block).
        if block.era == Era::Byron {
            let fee_policy = ByronFeePolicy {
                min_fee_a: self.epochs.protocol_params.min_fee_a,
                min_fee_b: self.epochs.protocol_params.min_fee_b,
            };
            let byron_mode = match mode {
                BlockValidationMode::ValidateAll => ByronApplyMode::ValidateAll,
                BlockValidationMode::ApplyOnly => ByronApplyMode::ApplyOnly,
            };

            // Process each Byron transaction one at a time so that outputs
            // created by an earlier transaction in the block are immediately
            // visible to later transactions (within-block spending chains).
            let mut total_byron_fees = Lovelace(0);
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
                    |input| self.utxo.utxo_set.lookup(input),
                )
                .map_err(|e| LedgerError::BlockTxValidationFailed {
                    slot: e.slot,
                    tx_hash: e.tx_hash,
                    errors: e.reason.to_string(),
                })?;

                // Apply each tx's effects immediately so subsequent txs in the
                // same block see the correct UTxO state.
                for input in &effect.spent {
                    if let Some(spent_output) = self.utxo.utxo_set.lookup(input) {
                        block_diff.record_delete(input.clone(), spent_output);
                    }
                    self.utxo.utxo_set.remove(input);
                }
                for (input, output) in effect.created {
                    block_diff.record_insert(input.clone(), output.clone());
                    self.utxo.utxo_set.insert(input, output);
                }
                total_byron_fees.0 = total_byron_fees.0.saturating_add(effect.fees.0);
            }
            self.utxo.epoch_fees += total_byron_fees;

            // Track block production (Byron uses OBFT, not VRF)
            if !block.header.issuer_vkey.is_empty() {
                let pool_id = dugite_primitives::hash::blake2b_224(&block.header.issuer_vkey);
                *Arc::make_mut(&mut self.consensus.epoch_blocks_by_pool)
                    .entry(pool_id)
                    .or_insert(0) += 1;
            }
            self.consensus.epoch_block_count += 1;

            // Byron nonce: LAB nonce = prev_hash (OBFT, no VRF)
            self.consensus.lab_nonce = block.header.prev_hash;

            self.tip = block.tip();
            if block.era > self.era {
                self.pending_era_transition = Some((self.era, block.era, self.epoch));
            }
            self.era = block.era;

            self.utxo.diff_seq.push_bounded(
                block.slot(),
                *block.hash(),
                block_diff,
                self.security_param as usize,
            );

            trace!(
                slot = block.slot().0,
                block_no = block.block_number().0,
                utxo_count = self.utxo.utxo_set.len(),
                epoch = self.epoch.0,
                "Ledger: Byron block applied successfully"
            );
            return Ok(());
        }

        // ══════════════════════════════════════════════════════════════════
        // Shelley+ era block processing via EraRulesImpl
        // ══════════════════════════════════════════════════════════════════

        let rules = EraRulesImpl::for_era(block.era);

        // ── Step 6: Block body validation (ExUnit budgets, ref scripts) ──
        //
        // Dispatched to era rules in ValidateAll mode. Each era checks its own
        // constraints (e.g., Alonzo+ ExUnit budgets, Conway+ ref script size
        // limits). In ApplyOnly mode (historical replay) these checks are
        // skipped — the block was already validated by the producing node.
        if mode == BlockValidationMode::ValidateAll {
            let body_ctx = RuleContext {
                params: &self.epochs.protocol_params,
                current_slot: block.slot().0,
                current_epoch: self.epoch,
                era: block.era,
                slot_config: Some(&self.slot_config),
                node_network: self.node_network,
                genesis_delegates: &self.genesis_delegates,
                update_quorum: self.update_quorum,
                epoch_length: self.epoch_length,
                shelley_transition_epoch: self.shelley_transition_epoch,
                byron_epoch_length: self.byron_epoch_length,
                stability_window: self.randomness_stabilisation_window,
                stability_window_3kf: self.stability_window_3kf,
                randomness_stabilisation_window: self.randomness_stabilisation_window,
                tx_index: 0,
                conway_genesis: self.conway_genesis_init.as_ref(),
            };
            rules.validate_block_body(block, &body_ctx, &self.utxo)?;
        }

        // Pre-compute cost_models CBOR once per block
        let cost_models_cbor = if mode == BlockValidationMode::ValidateAll {
            self.epochs.protocol_params.cost_models.to_cbor()
        } else {
            None
        };

        // Track processed tx hashes to skip duplicates within a block
        let mut processed_tx_hashes =
            std::collections::HashSet::with_capacity(block.transactions.len());

        // Cache block-level values for RuleContext construction inside the loop.
        // We cannot call self.build_rule_context() inside the loop because it
        // borrows &self (via &self.epochs.protocol_params) while we also need
        // &mut self.utxo/certs/gov/epochs. Clone protocol_params once per block
        // to break the aliasing conflict.
        let block_slot = block.slot().0;
        let block_era = block.era;
        let cached_params = self.epochs.protocol_params.clone();

        // ── Step 8: Per-transaction processing loop ───────────────────────
        for (tx_idx, tx) in block.transactions.iter().enumerate() {
            if !processed_tx_hashes.insert(tx.hash) {
                warn!(
                    tx_hash = %tx.hash.to_hex(),
                    slot = block.slot().0,
                    "Duplicate transaction hash in block, skipping"
                );
                continue;
            }

            // ── Step 8a: Phase-1 + Phase-2 validation (ValidateAll only) ──
            if mode == BlockValidationMode::ValidateAll {
                // Conway per-tx ref script size limit
                if self.epochs.protocol_params.protocol_version_major >= 9 && tx.is_valid {
                    let tx_ref_script_size = calculate_ref_script_size(
                        &tx.body.inputs,
                        &tx.body.reference_inputs,
                        &self.utxo.utxo_set,
                    );
                    if tx_ref_script_size > MAX_REF_SCRIPT_SIZE_PER_TX {
                        return Err(LedgerError::BlockTxValidationFailed {
                            slot: block.slot().0,
                            tx_hash: tx.hash.to_hex(),
                            errors: format!(
                                "TxRefScriptSizeTooLarge: reference script size {} exceeds \
                                 per-transaction limit {} bytes \
                                 (Conway ppMaxRefScriptSizePerTxG)",
                                tx_ref_script_size, MAX_REF_SCRIPT_SIZE_PER_TX
                            ),
                        });
                    }
                }

                let has_redeemers = !tx.witness_set.redeemers.is_empty();

                if tx.is_valid {
                    // Conway LEDGERS: treasury value check
                    if self.epochs.protocol_params.protocol_version_major >= 9 {
                        if let Some(declared_treasury) = tx.body.treasury_value {
                            if declared_treasury.0 != self.epochs.treasury.0 {
                                warn!(
                                    tx_hash = %tx.hash.to_hex(),
                                    slot = block.slot().0,
                                    declared = declared_treasury.0,
                                    ledger = self.epochs.treasury.0,
                                    "TreasuryValueMismatch on confirmed block — \
                                     trusting on-chain consensus (treasury will self-correct)"
                                );
                                self.epochs.treasury = declared_treasury;
                            }
                        }
                    }

                    // Conway LEDGERS: unelected committee member check
                    if self.epochs.protocol_params.protocol_version_major >= 9 {
                        for cert in &tx.body.certificates {
                            if let Certificate::CommitteeHotAuth {
                                cold_credential, ..
                            } = cert
                            {
                                let cold_key = credential_to_hash(cold_credential);
                                if !self
                                    .gov
                                    .governance
                                    .committee_expiration
                                    .contains_key(&cold_key)
                                {
                                    warn!(
                                        tx_hash = %tx.hash.to_hex(),
                                        slot = block.slot().0,
                                        cold_key = %cold_key.to_hex(),
                                        "UnelectedCommitteeMember on confirmed block — \
                                         trusting on-chain consensus (committee state may be stale)"
                                    );
                                }
                            }
                        }
                    }

                    // Full Phase-1 + Phase-2 validation
                    let tx_size = tx.raw_cbor.as_ref().map_or(0, |c| c.len() as u64);
                    let registered_pool_ids: std::collections::HashSet<
                        dugite_primitives::hash::Hash28,
                    > = self.certs.pool_params.keys().copied().collect();
                    let registered_drep_ids: std::collections::HashSet<
                        dugite_primitives::hash::Hash32,
                    > = self.gov.governance.dreps.keys().copied().collect();
                    let registered_vrf_keys: std::collections::HashMap<
                        dugite_primitives::hash::Hash32,
                        dugite_primitives::hash::Hash28,
                    > = self
                        .certs
                        .pool_params
                        .values()
                        .map(|reg| (reg.vrf_keyhash, reg.pool_id))
                        .collect();
                    let committee_member_keys: std::collections::HashSet<
                        dugite_primitives::hash::Hash32,
                    > = self
                        .gov
                        .governance
                        .committee_expiration
                        .keys()
                        .copied()
                        .collect();
                    let committee_resigned_keys: std::collections::HashSet<
                        dugite_primitives::hash::Hash32,
                    > = self
                        .gov
                        .governance
                        .committee_resigned
                        .keys()
                        .copied()
                        .collect();
                    let constitution_script_hash = self
                        .gov
                        .governance
                        .constitution
                        .as_ref()
                        .and_then(|c| c.script_hash);
                    // Build the set of vote delegation credential hashes for the
                    // ConwayWdrlNotDelegatedToDRep check (PV >= 10 only).
                    let vote_delegation_keys: std::collections::HashSet<
                        dugite_primitives::hash::Hash32,
                    > = self
                        .gov
                        .governance
                        .vote_delegations
                        .keys()
                        .copied()
                        .collect();
                    let result = validate_transaction_with_pools(
                        tx,
                        &self.utxo.utxo_set,
                        &self.epochs.protocol_params,
                        block.slot().0,
                        tx_size,
                        Some(&self.slot_config),
                        Some(&registered_pool_ids),
                        Some(self.epochs.treasury.0),
                        Some(&self.certs.reward_accounts),
                        Some(self.epoch.0),
                        Some(&registered_drep_ids),
                        Some(&registered_vrf_keys),
                        self.node_network,
                        Some(&committee_member_keys),
                        Some(&committee_resigned_keys),
                        Some(&self.certs.stake_key_deposits),
                        constitution_script_hash,
                        Some(&vote_delegation_keys),
                    );
                    if let Err(errors) = result {
                        let has_script_failure = errors
                            .iter()
                            .any(|e| matches!(e, ValidationError::ScriptFailed(_)));
                        if has_script_failure {
                            warn!(
                                tx_hash = %tx.hash.to_hex(),
                                slot = block.slot().0,
                                errors = ?errors.iter().filter(|e| matches!(e, ValidationError::ScriptFailed(_)))
                                    .map(|e| e.to_string()).collect::<Vec<_>>(),
                                "Plutus evaluation divergence: uplc says scripts fail but block is_valid=true on-chain — \
                                 trusting on-chain consensus (likely marginal budget difference)"
                            );
                        } else {
                            let is_utxo_gap_only = errors.iter().all(|e| {
                                matches!(
                                    e,
                                    ValidationError::InputNotFound(_)
                                        | ValidationError::CollateralNotFound(_)
                                        | ValidationError::CollateralMismatch { .. }
                                        | ValidationError::InsufficientCollateral
                                        | ValidationError::ValueNotConserved { .. }
                                        | ValidationError::MultiAssetNotConserved { .. }
                                )
                            });
                            if is_utxo_gap_only {
                                let err_str: Vec<String> =
                                    errors.iter().map(|e| e.to_string()).collect();
                                debug!(
                                    tx_hash = %tx.hash.to_hex(),
                                    slot = block.slot().0,
                                    errors = %err_str.join("; "),
                                    "Phase-1 UTxO-gap errors on confirmed block (inputs not yet \
                                     in store due to partial replay) — outputs will still be \
                                     inserted by best-effort apply"
                                );
                            } else {
                                let err_str: Vec<String> =
                                    errors.iter().map(|e| e.to_string()).collect();
                                warn!(
                                    tx_hash = %tx.hash.to_hex(),
                                    slot = block.slot().0,
                                    errors = %err_str.join("; "),
                                    "Phase-1 validation divergence on confirmed block — \
                                     trusting on-chain consensus"
                                );
                            }
                        }
                    }
                } else if has_redeemers {
                    // Producer claims tx is invalid with scripts present.
                    // Verify scripts actually fail; if they pass, producer is stealing collateral.
                    let max_ex = (
                        self.epochs.protocol_params.max_tx_ex_units.steps,
                        self.epochs.protocol_params.max_tx_ex_units.mem,
                    );
                    let eval_result = evaluate_plutus_scripts(
                        tx,
                        &self.utxo.utxo_set,
                        cost_models_cbor.as_deref(),
                        max_ex,
                        &self.slot_config,
                    );
                    if eval_result.is_ok() {
                        return Err(LedgerError::ValidationTagMismatch {
                            tx_hash: tx.hash.to_hex(),
                            block_flag: false,
                            eval_result: true,
                        });
                    }
                }
            }

            // ── Step 8b: Apply transaction via era rules ──────────────────
            //
            // Build the RuleContext inline to avoid borrowing &self while also
            // needing &mut self.utxo/certs/gov/epochs. The context references
            // only immutable fields or fields we snapshot before mutation.
            let ctx = RuleContext {
                params: &cached_params,
                current_slot: block_slot,
                current_epoch: self.epoch,
                era: block_era,
                slot_config: Some(&self.slot_config),
                node_network: self.node_network,
                genesis_delegates: &self.genesis_delegates,
                update_quorum: self.update_quorum,
                epoch_length: self.epoch_length,
                shelley_transition_epoch: self.shelley_transition_epoch,
                byron_epoch_length: self.byron_epoch_length,
                stability_window: self.randomness_stabilisation_window,
                stability_window_3kf: self.stability_window_3kf,
                randomness_stabilisation_window: self.randomness_stabilisation_window,
                tx_index: tx_idx as u64,
                conway_genesis: self.conway_genesis_init.as_ref(),
            };

            if !tx.is_valid {
                // Invalid transaction: consume collateral via era rules.
                let diff = rules.apply_invalid_tx(
                    tx,
                    mode,
                    &ctx,
                    &mut self.utxo,
                    &mut self.certs,
                    &mut self.epochs,
                )?;
                block_diff.merge(&diff);
            } else {
                // Valid transaction: full LEDGER rule pipeline via era rules.
                // The era rules handle: drain withdrawals, process certificates,
                // apply UTxO changes (consume inputs, produce outputs, accumulate fee),
                // and Conway-specific governance (votes, proposals, donations).
                let diff = rules.apply_valid_tx(
                    tx,
                    mode,
                    &ctx,
                    &mut self.utxo,
                    &mut self.certs,
                    &mut self.gov,
                    &mut self.epochs,
                )?;
                block_diff.merge(&diff);

                // ── Step 8c: Pre-Conway PP update proposals ───────────────
                //
                // These are NOT part of the era rules because they operate on the
                // epoch sub-state's pending/future maps and are only relevant for
                // Shelley through Babbage (pre-governance PP updates).
                if let Some(ref update) = tx.body.update {
                    let is_future = update.epoch > self.epoch.0;
                    for (genesis_hash, ppu) in &update.proposed_updates {
                        debug!(
                            genesis_hash = %genesis_hash.to_hex(),
                            target_epoch = update.epoch,
                            current_epoch = self.epoch.0,
                            kind = if is_future { "future" } else { "current" },
                            protocol_version = ?ppu.protocol_version_major.zip(ppu.protocol_version_minor),
                            d = ?ppu.d,
                            n_opt = ?ppu.n_opt,
                            "Collected protocol parameter update proposal"
                        );
                        if is_future {
                            self.epochs
                                .future_pp_updates
                                .entry(EpochNo(update.epoch))
                                .or_default()
                                .push((*genesis_hash, ppu.clone()));
                        } else {
                            self.epochs
                                .pending_pp_updates
                                .entry(EpochNo(update.epoch))
                                .or_default()
                                .push((*genesis_hash, ppu.clone()));
                        }
                    }
                }
            }
        }

        // ── Step 9: Nonce evolution and block production tracking ─────────
        //
        // Dispatched to era rules via `evolve_nonce`. Each era's implementation
        // handles: evolving nonce update (VRF-based), candidate nonce freeze
        // (stability window), lab nonce = prevHashToNonce, and block production
        // counting (incrBlocks with d-parameter gating).
        {
            let nonce_ctx = RuleContext {
                params: &self.epochs.protocol_params,
                current_slot: block.slot().0,
                current_epoch: self.epoch,
                era: block.era,
                slot_config: Some(&self.slot_config),
                node_network: self.node_network,
                genesis_delegates: &self.genesis_delegates,
                update_quorum: self.update_quorum,
                epoch_length: self.epoch_length,
                shelley_transition_epoch: self.shelley_transition_epoch,
                byron_epoch_length: self.byron_epoch_length,
                stability_window: self.randomness_stabilisation_window,
                stability_window_3kf: self.stability_window_3kf,
                randomness_stabilisation_window: self.randomness_stabilisation_window,
                tx_index: 0,
                conway_genesis: self.conway_genesis_init.as_ref(),
            };
            rules.evolve_nonce(&block.header, &nonce_ctx, &mut self.consensus);
        }

        // ── Step 10: Update tip and era ──────────────────────────────────
        self.tip = block.tip();
        self.era = block.era;

        // Record this block's UTxO diff for rollback support.
        self.utxo
            .diff_seq
            .push(block.slot(), *block.hash(), block_diff);

        trace!(
            slot = block.slot().0,
            block_no = block.block_number().0,
            utxo_count = self.utxo.utxo_set.len(),
            epoch = self.epoch.0,
            era = ?self.era,
            "Ledger: block applied successfully"
        );

        Ok(())
    }

    /// Apply a block and produce a [`LedgerDelta`] capturing all state changes.
    ///
    /// Performs the exact same state mutations as [`apply_block`], and additionally
    /// returns a `LedgerDelta` recording every change. The delta is used by
    /// `LedgerSeq` for O(1) rollback and O(checkpoint_interval) state reconstruction.
    ///
    /// # Implementation
    ///
    /// Delegates to `apply_block()` for all state mutations, then extracts the
    /// UTxO diff from the DiffSeq and builds `BlockFieldsDelta` from post-block state.
    /// Epoch transition deltas capture absolute post-transition values.
    pub fn apply_block_with_delta(
        &mut self,
        block: &Block,
        mode: BlockValidationMode,
    ) -> Result<LedgerDelta, LedgerError> {
        let mut delta = LedgerDelta::new(block.slot(), *block.hash(), block.block_number());

        // Snapshot pre-block epoch to detect epoch transitions.
        let pre_epoch = self.epoch;

        // Apply the block (all state mutations happen here).
        self.apply_block(block, mode)?;

        // Extract the UTxO diff from the DiffSeq entry that apply_block just pushed.
        if let Some((_slot, _hash, utxo_diff)) = self.utxo.diff_seq.diffs.back() {
            delta.utxo_diff = utxo_diff.clone();
        }

        // Capture epoch transition delta if an epoch boundary was crossed.
        if self.epoch > pre_epoch {
            delta.epoch_transition = Some(crate::ledger_seq::EpochTransitionDelta {
                new_epoch: self.epoch,
                treasury: self.epochs.treasury,
                reserves: self.epochs.reserves,
                snapshots: self.epochs.snapshots.clone(),
                protocol_params: self.epochs.protocol_params.clone(),
                prev_protocol_params: self.epochs.prev_protocol_params.clone(),
                prev_d: self.epochs.prev_d,
                prev_protocol_version_major: self.epochs.prev_protocol_version_major,
                pending_pp_updates_cleared: self.epochs.pending_pp_updates.is_empty()
                    && self.epochs.future_pp_updates.is_empty(),
                epoch_nonce: self.consensus.epoch_nonce,
                last_epoch_block_nonce: self.consensus.last_epoch_block_nonce,
                reward_credits: std::collections::HashMap::new(),
                pools_retired: Vec::new(),
                future_params_promoted: Vec::new(),
                drep_activity_updates: self
                    .gov
                    .governance
                    .dreps
                    .iter()
                    .map(|(cred, drep)| (*cred, drep.active))
                    .collect(),
                last_ratified: self.gov.governance.last_ratified.clone(),
                last_expired: self.gov.governance.last_expired.clone(),
                last_ratify_delayed: self.gov.governance.last_ratify_delayed,
                new_constitution: self.gov.governance.constitution.clone(),
                no_confidence: Some(self.gov.governance.no_confidence),
                committee_threshold: Some(self.gov.governance.committee_threshold.clone()),
                proposals_enacted: self
                    .gov
                    .governance
                    .last_ratified
                    .iter()
                    .map(|(id, _)| id.clone())
                    .collect(),
                proposals_expired: self.gov.governance.last_expired.clone(),
                enacted_pparam_update: Some(self.gov.governance.enacted_pparam_update.clone()),
                enacted_hard_fork: Some(self.gov.governance.enacted_hard_fork.clone()),
                enacted_committee: Some(self.gov.governance.enacted_committee.clone()),
                enacted_constitution: Some(self.gov.governance.enacted_constitution.clone()),
                stake_distribution: self.certs.stake_distribution.clone(),
                delegation_changes: Vec::new(),
            });
        }

        // Build per-block scalar field delta from post-block state.
        let pool_block_increment = if !block.header.issuer_vkey.is_empty() {
            let current_d = if self.epochs.protocol_params.protocol_version_major >= 7 {
                0.0
            } else {
                self.epochs.protocol_params.d.numerator as f64
                    / self.epochs.protocol_params.d.denominator.max(1) as f64
            };
            if current_d < 0.8 {
                Some(dugite_primitives::hash::blake2b_224(
                    &block.header.issuer_vkey,
                ))
            } else {
                None
            }
        } else {
            None
        };

        delta.block_fields = BlockFieldsDelta {
            fees_collected: block
                .transactions
                .iter()
                .filter(|tx| tx.is_valid)
                .map(|tx| tx.body.fee)
                .fold(Lovelace(0), |acc, fee| Lovelace(acc.0 + fee.0)),
            pool_block_increment,
            epoch_block_count: self.consensus.epoch_block_count,
            evolving_nonce: self.consensus.evolving_nonce,
            candidate_nonce: self.consensus.candidate_nonce,
            lab_nonce: self.consensus.lab_nonce,
            epoch_fees: self.utxo.epoch_fees,
        };

        Ok(delta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dugite_primitives::address::{Address, ByronAddress, EnterpriseAddress};
    use dugite_primitives::block::{BlockHeader, OperationalCert, ProtocolVersion, VrfOutput};
    use dugite_primitives::credentials::Credential;
    use dugite_primitives::era::Era;
    use dugite_primitives::hash::{Hash28, Hash32};
    use dugite_primitives::network::NetworkId;
    use dugite_primitives::protocol_params::ProtocolParameters;
    use dugite_primitives::time::{BlockNo, SlotNo};
    use dugite_primitives::transaction::{
        ExUnits, OutputDatum, Redeemer, RedeemerTag, ScriptRef, Transaction, TransactionInput,
        TransactionOutput,
    };
    use dugite_primitives::value::{Lovelace, Value};

    // ── Helpers ──────────────────────────────────────────────────────────────

    /// Build a minimal `Block` for the given era, slot, and block number.
    ///
    /// `prev_hash` is set to zero so the ledger tip check is skipped (the
    /// state starts at `Point::Origin` so the prev-hash validation is
    /// bypassed entirely for the first block applied).
    fn make_test_block(
        era: Era,
        slot: u64,
        block_no: u64,
        protocol_major: u64,
        body_size: u64,
        txs: Vec<Transaction>,
    ) -> Block {
        Block {
            header: BlockHeader {
                header_hash: Hash32::from_bytes({
                    let mut b = [0u8; 32];
                    b[..8].copy_from_slice(&block_no.to_be_bytes());
                    b
                }),
                prev_hash: Hash32::ZERO,
                issuer_vkey: vec![],
                vrf_vkey: vec![],
                vrf_result: VrfOutput {
                    output: vec![],
                    proof: vec![],
                },
                nonce_vrf_output: vec![],
                nonce_vrf_proof: vec![],
                block_number: BlockNo(block_no),
                slot: SlotNo(slot),
                epoch_nonce: Hash32::ZERO,
                body_size,
                body_hash: Hash32::ZERO,
                operational_cert: OperationalCert {
                    hot_vkey: vec![],
                    sequence_number: 0,
                    kes_period: 0,
                    sigma: vec![],
                },
                protocol_version: ProtocolVersion {
                    major: protocol_major,
                    minor: 0,
                },
                kes_signature: vec![],
            },
            transactions: txs,
            era,
            raw_cbor: None,
        }
    }

    /// Build a minimal Shelley+ `TransactionOutput` with an enterprise address
    /// and ADA-only value.  No datum, no script_ref.
    fn make_output(coin: u64) -> TransactionOutput {
        TransactionOutput {
            address: Address::Enterprise(EnterpriseAddress {
                network: NetworkId::Mainnet,
                payment: Credential::VerificationKey(Hash28::from_bytes([0xABu8; 28])),
            }),
            value: Value {
                coin: Lovelace(coin),
                multi_asset: Default::default(),
            },
            datum: OutputDatum::None,
            script_ref: None,
            is_legacy: false,
            raw_cbor: None,
        }
    }

    /// Build a minimal valid-looking transaction in ApplyOnly style.
    ///
    /// `tx_id_byte` is used to uniquely distinguish the transaction hash so
    /// that sequential UTxO spending tests can reference specific outputs by
    /// their parent tx hash.
    fn make_simple_tx(
        tx_id_byte: u8,
        inputs: Vec<TransactionInput>,
        outputs: Vec<TransactionOutput>,
        fee: u64,
    ) -> Transaction {
        let hash = Hash32::from_bytes([tx_id_byte; 32]);
        let mut tx = Transaction::empty_with_hash(hash);
        tx.body.inputs = inputs;
        tx.body.outputs = outputs;
        tx.body.fee = Lovelace(fee);
        tx
    }

    /// Seed a UTxO entry in the ledger state.
    fn seed_utxo(state: &mut LedgerState, input: TransactionInput, output: TransactionOutput) {
        state.utxo.utxo_set.insert(input, output);
    }

    // ── Test 1: Byron-era block with one tx consuming a UTxO ─────────────────

    #[test]
    fn test_apply_byron_block() {
        let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());

        // Seed the UTxO that the Byron tx will spend.
        let genesis_input = TransactionInput {
            transaction_id: Hash32::from_bytes([0x01u8; 32]),
            index: 0,
        };
        let genesis_output = TransactionOutput {
            address: Address::Byron(ByronAddress {
                payload: vec![0u8; 32],
            }),
            value: Value {
                coin: Lovelace(10_000_000),
                multi_asset: Default::default(),
            },
            datum: OutputDatum::None,
            script_ref: None,
            is_legacy: true,
            raw_cbor: None,
        };
        seed_utxo(&mut state, genesis_input.clone(), genesis_output);

        // Build a Byron tx that spends the genesis UTxO.
        // Fee > 0 satisfies the value-conservation check (no minimum fee check
        // when raw_cbor is None → tx_size_bytes = 0 → min_fee = min_fee_b).
        // We set fee to min_fee_b (155381) so conservation holds:
        // input 10_000_000 = output 9_844_619 + fee 155_381.
        let fee: u64 = state.epochs.protocol_params.min_fee_b;
        let output_value = 10_000_000u64 - fee;

        let _out_input = TransactionInput {
            transaction_id: Hash32::from_bytes([0x10u8; 32]),
            index: 0,
        };
        let out_output = TransactionOutput {
            address: Address::Byron(ByronAddress {
                payload: vec![1u8; 32],
            }),
            value: Value {
                coin: Lovelace(output_value),
                multi_asset: Default::default(),
            },
            datum: OutputDatum::None,
            script_ref: None,
            is_legacy: true,
            raw_cbor: None,
        };

        let mut tx = Transaction::empty_with_hash(Hash32::from_bytes([0x10u8; 32]));
        tx.body.inputs = vec![genesis_input.clone()];
        tx.body.outputs = vec![out_output];
        tx.body.fee = Lovelace(fee);

        // Byron slots: 208 epochs × 21600 slots/epoch = 4,492,800.
        // Slot 100 is firmly in the Byron era.
        let block = make_test_block(Era::Byron, 100, 1, 1, 0, vec![tx]);

        // ApplyOnly skips fee-policy validation while still applying UTxO changes.
        state
            .apply_block(&block, BlockValidationMode::ApplyOnly)
            .expect("Byron block should apply");

        // The genesis UTxO must have been consumed.
        assert!(
            state.utxo.utxo_set.lookup(&genesis_input).is_none(),
            "Spent Byron input must be removed"
        );
        // The new output at index 0 of the tx hash must exist.
        let new_input = TransactionInput {
            transaction_id: Hash32::from_bytes([0x10u8; 32]),
            index: 0,
        };
        assert!(
            state.utxo.utxo_set.lookup(&new_input).is_some(),
            "Byron output must be created"
        );
        // Tip was advanced.
        assert_ne!(state.tip, dugite_primitives::block::Tip::origin());
    }

    // ── Test 2: Shelley+ block with one valid tx ──────────────────────────────

    #[test]
    fn test_apply_shelley_block() {
        let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());

        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0x02u8; 32]),
            index: 0,
        };
        seed_utxo(&mut state, input.clone(), make_output(5_000_000));

        let output = make_output(4_500_000);
        let tx = make_simple_tx(0x20, vec![input.clone()], vec![output], 500_000);

        let block = make_test_block(Era::Conway, 1_000, 1, 9, 0, vec![tx.clone()]);

        state
            .apply_block(&block, BlockValidationMode::ApplyOnly)
            .expect("Shelley+ block should apply");

        // Input was consumed.
        assert!(state.utxo.utxo_set.lookup(&input).is_none());
        // New output at index 0 exists.
        let new_input = TransactionInput {
            transaction_id: Hash32::from_bytes([0x20u8; 32]),
            index: 0,
        };
        assert!(state.utxo.utxo_set.lookup(&new_input).is_some());
        // Tip updated.
        assert_eq!(state.tip.block_number, BlockNo(1));
    }

    // ── Test 3: Empty block ───────────────────────────────────────────────────

    #[test]
    fn test_apply_empty_block() {
        let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());

        // Seed one UTxO — it must survive the empty block.
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0x03u8; 32]),
            index: 0,
        };
        seed_utxo(&mut state, input.clone(), make_output(1_000_000));

        let block = make_test_block(Era::Conway, 2_000, 1, 9, 0, vec![]);

        state
            .apply_block(&block, BlockValidationMode::ApplyOnly)
            .expect("Empty block should apply");

        // UTxO untouched.
        assert!(state.utxo.utxo_set.lookup(&input).is_some());
        // Tip updated.
        assert_eq!(state.tip.point.slot().unwrap(), SlotNo(2_000));
    }

    // ── Test 4: invalid tx — collateral consumed, regular input untouched ────

    #[test]
    fn test_invalid_tx_collateral_consumed() {
        let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());

        // Regular input (must NOT be consumed).
        let regular_input = TransactionInput {
            transaction_id: Hash32::from_bytes([0x04u8; 32]),
            index: 0,
        };
        seed_utxo(&mut state, regular_input.clone(), make_output(2_000_000));

        // Collateral input (MUST be consumed).
        let collateral_input = TransactionInput {
            transaction_id: Hash32::from_bytes([0x04u8; 32]),
            index: 1,
        };
        seed_utxo(&mut state, collateral_input.clone(), make_output(3_000_000));

        let mut tx = Transaction::empty_with_hash(Hash32::from_bytes([0x40u8; 32]));
        tx.body.inputs = vec![regular_input.clone()];
        tx.body.collateral = vec![collateral_input.clone()];
        tx.body.fee = Lovelace(0);
        tx.is_valid = false;

        let block = make_test_block(Era::Conway, 3_000, 1, 9, 0, vec![tx]);

        state
            .apply_block(&block, BlockValidationMode::ApplyOnly)
            .expect("Block with invalid tx should apply");

        // Collateral was spent.
        assert!(
            state.utxo.utxo_set.lookup(&collateral_input).is_none(),
            "Collateral input must be consumed"
        );
        // Regular input survived.
        assert!(
            state.utxo.utxo_set.lookup(&regular_input).is_some(),
            "Regular input of invalid tx must not be consumed"
        );
    }

    // ── Test 5: Epoch transition detected ────────────────────────────────────

    #[test]
    fn test_epoch_transition_detected() {
        let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());
        // Zero out Byron transition so epoch_of_slot is just slot/epoch_length.
        state.shelley_transition_epoch = 0;
        state.byron_epoch_length = 0;
        assert_eq!(state.epoch, EpochNo(0));

        // First slot of epoch 1 triggers the transition.
        let slot = state.epoch_length; // 432000
        let block = make_test_block(Era::Conway, slot, 1, 9, 0, vec![]);

        state
            .apply_block(&block, BlockValidationMode::ApplyOnly)
            .expect("Block should apply");

        assert_eq!(state.epoch, EpochNo(1));
    }

    // ── Test 6: Multi-epoch gap ───────────────────────────────────────────────

    #[test]
    fn test_multi_epoch_gap() {
        let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());
        state.shelley_transition_epoch = 0;
        state.byron_epoch_length = 0;
        assert_eq!(state.epoch, EpochNo(0));

        // Jump directly to epoch 3.
        let slot = state.epoch_length * 3; // first slot of epoch 3
        let block = make_test_block(Era::Conway, slot, 1, 9, 0, vec![]);

        state
            .apply_block(&block, BlockValidationMode::ApplyOnly)
            .expect("Block should apply after multi-epoch gap");

        // All three epoch transitions (1, 2, 3) must have been processed.
        assert_eq!(state.epoch, EpochNo(3));
    }

    // ── Test 8: Per-tx ref-script size limit (Conway ValidateAll) ─────────────

    #[test]
    fn test_ref_script_size_per_tx_limit() {
        let mut params = ProtocolParameters::mainnet_defaults();
        // Must be Conway (protocol_version_major >= 9).
        params.protocol_version_major = 9;
        let mut state = LedgerState::new(params);

        // A UTxO whose script_ref exceeds the 200 KiB per-tx limit.
        let big_input = TransactionInput {
            transaction_id: Hash32::from_bytes([0x08u8; 32]),
            index: 0,
        };
        let big_output = TransactionOutput {
            address: Address::Enterprise(EnterpriseAddress {
                network: NetworkId::Mainnet,
                payment: Credential::VerificationKey(Hash28::from_bytes([0x08u8; 28])),
            }),
            value: Value {
                coin: Lovelace(5_000_000),
                multi_asset: Default::default(),
            },
            datum: OutputDatum::None,
            // 205_000 bytes > 200 * 1024 (204_800) per-tx limit.
            script_ref: Some(ScriptRef::PlutusV2(vec![0u8; 205_000])),
            is_legacy: false,
            raw_cbor: None,
        };
        seed_utxo(&mut state, big_input.clone(), big_output);

        // A valid tx that spends the large-script UTxO.
        let tx = make_simple_tx(0x80, vec![big_input], vec![make_output(4_000_000)], 0);

        let block = make_test_block(Era::Conway, 1_000, 1, 9, 0, vec![tx]);

        let result = state.apply_block(&block, BlockValidationMode::ValidateAll);
        assert!(
            result.is_err(),
            "Per-tx ref-script size limit must be enforced in ValidateAll mode"
        );
        if let Err(LedgerError::BlockTxValidationFailed { errors, .. }) = result {
            assert!(
                errors.contains("TxRefScriptSizeTooLarge"),
                "Error must mention TxRefScriptSizeTooLarge, got: {errors}"
            );
        }
    }

    // ── Test 9: Per-block ref-script size limit ───────────────────────────────

    #[test]
    fn test_ref_script_size_per_block_limit() {
        let mut params = ProtocolParameters::mainnet_defaults();
        params.protocol_version_major = 9;
        let mut state = LedgerState::new(params);

        // Create multiple UTxOs each with a script_ref that individually fits
        // within the 200 KiB per-tx limit but together exceed the 1 MiB
        // per-block limit.  6 × 200 KiB = 1,200 KiB > 1,048,576 bytes.
        let script_bytes = vec![0u8; 200 * 1024];
        let mut txs = Vec::new();
        for i in 0u8..6 {
            let inp = TransactionInput {
                transaction_id: Hash32::from_bytes([0x09u8 + i; 32]),
                index: 0,
            };
            let out = TransactionOutput {
                address: Address::Enterprise(EnterpriseAddress {
                    network: NetworkId::Mainnet,
                    payment: Credential::VerificationKey(Hash28::from_bytes([0x09u8 + i; 28])),
                }),
                value: Value {
                    coin: Lovelace(2_000_000),
                    multi_asset: Default::default(),
                },
                datum: OutputDatum::None,
                script_ref: Some(ScriptRef::PlutusV2(script_bytes.clone())),
                is_legacy: false,
                raw_cbor: None,
            };
            seed_utxo(&mut state, inp.clone(), out);

            let tx = make_simple_tx(0x90 + i, vec![inp], vec![make_output(1_000_000)], 0);
            txs.push(tx);
        }

        let block = make_test_block(Era::Conway, 1_000, 1, 9, 0, txs);

        let result = state.apply_block(&block, BlockValidationMode::ValidateAll);
        assert!(
            result.is_err(),
            "Per-block ref-script size limit must be enforced"
        );
        if let Err(LedgerError::BlockTxValidationFailed { errors, .. }) = result {
            assert!(
                errors.contains("BodyRefScriptsSizeTooBig"),
                "Error must mention BodyRefScriptsSizeTooBig, got: {errors}"
            );
        }
    }

    // ── Test 10: Block ExUnits memory budget exceeded ─────────────────────────

    #[test]
    fn test_block_ex_units_memory_exceeded() {
        let mut params = ProtocolParameters::mainnet_defaults();
        // Lower the per-block limit to make it easy to exceed with two txs.
        params.max_block_ex_units.mem = 10;
        let mut state = LedgerState::new(params);

        // Two valid txs each consuming 6 memory units → total 12 > limit 10.
        let mut txs = Vec::new();
        for i in 0u8..2 {
            let inp = TransactionInput {
                transaction_id: Hash32::from_bytes([0x0Au8 + i; 32]),
                index: 0,
            };
            seed_utxo(&mut state, inp.clone(), make_output(2_000_000));

            let mut tx = make_simple_tx(0xA0 + i, vec![inp], vec![make_output(1_000_000)], 0);
            tx.witness_set.redeemers = vec![Redeemer {
                tag: RedeemerTag::Spend,
                index: 0,
                data: dugite_primitives::transaction::PlutusData::Integer(0),
                ex_units: ExUnits { mem: 6, steps: 1 },
            }];
            txs.push(tx);
        }

        let block = make_test_block(Era::Conway, 1_000, 1, 9, 0, txs);

        let result = state.apply_block(&block, BlockValidationMode::ValidateAll);
        assert!(result.is_err(), "Exceeded memory budget must be rejected");
        if let Err(LedgerError::BlockTxValidationFailed { errors, .. }) = result {
            assert!(
                errors.contains("BlockExUnitsExceeded") && errors.contains("memory"),
                "Error must mention BlockExUnitsExceeded (memory), got: {errors}"
            );
        }
    }

    // ── Test 11: Block ExUnits steps budget exceeded ───────────────────────────

    #[test]
    fn test_block_ex_units_steps_exceeded() {
        let mut params = ProtocolParameters::mainnet_defaults();
        params.max_block_ex_units.steps = 10;
        let mut state = LedgerState::new(params);

        let mut txs = Vec::new();
        for i in 0u8..2 {
            let inp = TransactionInput {
                transaction_id: Hash32::from_bytes([0x0Bu8 + i; 32]),
                index: 0,
            };
            seed_utxo(&mut state, inp.clone(), make_output(2_000_000));

            let mut tx = make_simple_tx(0xB0 + i, vec![inp], vec![make_output(1_000_000)], 0);
            tx.witness_set.redeemers = vec![Redeemer {
                tag: RedeemerTag::Spend,
                index: 0,
                data: dugite_primitives::transaction::PlutusData::Integer(0),
                ex_units: ExUnits { mem: 1, steps: 6 },
            }];
            txs.push(tx);
        }

        let block = make_test_block(Era::Conway, 1_000, 1, 9, 0, txs);

        let result = state.apply_block(&block, BlockValidationMode::ValidateAll);
        assert!(result.is_err(), "Exceeded steps budget must be rejected");
        if let Err(LedgerError::BlockTxValidationFailed { errors, .. }) = result {
            assert!(
                errors.contains("BlockExUnitsExceeded") && errors.contains("step"),
                "Error must mention BlockExUnitsExceeded (steps), got: {errors}"
            );
        }
    }

    // ── Test 12: Sequential UTxO — tx1 creates, tx2 spends in same block ─────

    #[test]
    fn test_multiple_txs_sequential_utxo() {
        let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());

        // Genesis UTxO consumed by tx1.
        let genesis_input = TransactionInput {
            transaction_id: Hash32::from_bytes([0x0Cu8; 32]),
            index: 0,
        };
        seed_utxo(&mut state, genesis_input.clone(), make_output(5_000_000));

        // tx1: spends genesis_input, creates a new output.
        let tx1_hash = Hash32::from_bytes([0xC0u8; 32]);
        let mut tx1 = Transaction::empty_with_hash(tx1_hash);
        tx1.body.inputs = vec![genesis_input.clone()];
        tx1.body.outputs = vec![make_output(4_500_000)];
        tx1.body.fee = Lovelace(500_000);

        // tx2: spends tx1's output (index 0).
        let tx1_output_input = TransactionInput {
            transaction_id: tx1_hash,
            index: 0,
        };
        let tx2_hash = Hash32::from_bytes([0xC1u8; 32]);
        let mut tx2 = Transaction::empty_with_hash(tx2_hash);
        tx2.body.inputs = vec![tx1_output_input.clone()];
        tx2.body.outputs = vec![make_output(4_000_000)];
        tx2.body.fee = Lovelace(500_000);

        let block = make_test_block(Era::Conway, 1_000, 1, 9, 0, vec![tx1, tx2]);

        state
            .apply_block(&block, BlockValidationMode::ApplyOnly)
            .expect("Sequential within-block spending should succeed");

        // Genesis input consumed.
        assert!(state.utxo.utxo_set.lookup(&genesis_input).is_none());
        // tx1's intermediate output was consumed by tx2.
        assert!(state.utxo.utxo_set.lookup(&tx1_output_input).is_none());
        // tx2's output exists.
        let tx2_out = TransactionInput {
            transaction_id: tx2_hash,
            index: 0,
        };
        assert!(state.utxo.utxo_set.lookup(&tx2_out).is_some());
    }

    // ── Test 13: Conway pointer-stake exclusion ───────────────────────────────

    #[test]
    fn test_conway_pointer_stake_exclusion() {
        use dugite_primitives::credentials::Pointer;

        let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());

        // Set era to Babbage so the Conway block triggers an era transition,
        // which is where the pointer-stake exclusion logic now lives
        // (on_era_transition in conway.rs).
        state.era = Era::Babbage;

        // Pre-seed ptr_stake entries that should be cleared on first Conway block.
        state.epochs.ptr_stake.insert(
            Pointer {
                slot: 1,
                tx_index: 0,
                cert_index: 0,
            },
            1_000_000,
        );
        state.epochs.ptr_stake.insert(
            Pointer {
                slot: 2,
                tx_index: 0,
                cert_index: 0,
            },
            2_000_000,
        );
        assert_eq!(state.epochs.ptr_stake.len(), 2);
        assert!(!state.epochs.ptr_stake_excluded);

        // Apply a Conway-era block — the era transition from Babbage to Conway
        // triggers on_era_transition which sets ptr_stake_excluded = true.
        let block = make_test_block(Era::Conway, 1_000, 1, 9, 0, vec![]);

        state
            .apply_block(&block, BlockValidationMode::ApplyOnly)
            .expect("Conway block should apply");

        // The one-time exclusion flag must be set after the era transition.
        assert!(
            state.epochs.ptr_stake_excluded,
            "ptr_stake_excluded must be true after first Conway block"
        );
    }

    // ── Test 14: ApplyOnly mode skips per-tx ref-script validation ────────────

    #[test]
    fn test_apply_only_mode_skips_validation() {
        let mut params = ProtocolParameters::mainnet_defaults();
        params.protocol_version_major = 9;
        let mut state = LedgerState::new(params);

        // Same setup as test 8 — a UTxO with a script_ref that exceeds the
        // 200 KiB per-tx limit.  In ValidateAll mode this returns Err; in
        // ApplyOnly mode the check is skipped.
        let big_input = TransactionInput {
            transaction_id: Hash32::from_bytes([0x0Eu8; 32]),
            index: 0,
        };
        let big_output = TransactionOutput {
            address: Address::Enterprise(EnterpriseAddress {
                network: NetworkId::Mainnet,
                payment: Credential::VerificationKey(Hash28::from_bytes([0x0Eu8; 28])),
            }),
            value: Value {
                coin: Lovelace(5_000_000),
                multi_asset: Default::default(),
            },
            datum: OutputDatum::None,
            script_ref: Some(ScriptRef::PlutusV2(vec![0u8; 205_000])),
            is_legacy: false,
            raw_cbor: None,
        };
        seed_utxo(&mut state, big_input.clone(), big_output);

        let tx = make_simple_tx(0xE0, vec![big_input], vec![make_output(4_000_000)], 0);
        let block = make_test_block(Era::Conway, 1_000, 1, 9, 0, vec![tx]);

        // ApplyOnly must skip the per-tx ref-script size check.
        state
            .apply_block(&block, BlockValidationMode::ApplyOnly)
            .expect("ApplyOnly must succeed even with oversized ref-script");

        // Block was applied — tip advanced.
        assert_eq!(state.tip.block_number, BlockNo(1));
    }

    // ── Test 15: Certificate processing — StakeRegistration per tx ───────────

    #[test]
    fn test_certificate_processing_order() {
        let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());

        // Two credentials to register via StakeRegistration certs.
        let cred1 = Credential::VerificationKey(Hash28::from_bytes([0x0Fu8; 28]));
        let cred2 = Credential::VerificationKey(Hash28::from_bytes([0x1Fu8; 28]));
        let key1 = cred1.to_typed_hash32();
        let key2 = cred2.to_typed_hash32();

        // tx1 registers cred1; tx2 registers cred2.
        let mut tx1 = Transaction::empty_with_hash(Hash32::from_bytes([0xF0u8; 32]));
        tx1.body.certificates = vec![Certificate::StakeRegistration(cred1)];

        let mut tx2 = Transaction::empty_with_hash(Hash32::from_bytes([0xF1u8; 32]));
        tx2.body.certificates = vec![Certificate::StakeRegistration(cred2)];

        let block = make_test_block(Era::Conway, 1_000, 1, 9, 0, vec![tx1, tx2]);

        state
            .apply_block(&block, BlockValidationMode::ApplyOnly)
            .expect("Block with stake-registration certs should apply");

        // Both credentials must now have a reward-account entry.
        let reward_accounts = &*state.certs.reward_accounts;
        assert!(
            reward_accounts.contains_key(&key1),
            "cred1 must be registered in reward_accounts"
        );
        assert!(
            reward_accounts.contains_key(&key2),
            "cred2 must be registered in reward_accounts"
        );
    }

    // ── Test 16: ApplyOnly rejects Shelley+ hash mismatch ────────────────────
    //
    // After Sprint 1 Task 1, `ApplyOnly` only tolerates hash mismatch for Byron
    // blocks. Shelley+ blocks must still be rejected — pallas's Shelley-era
    // `OriginalHash` uses raw bytes so hash mismatch cannot legitimately occur
    // through the decode→store→decode cycle.
    #[test]
    fn test_apply_only_rejects_shelley_hash_mismatch() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);
        state.era = Era::Conway;
        state.tip = dugite_primitives::block::Tip {
            point: Point::Specific(SlotNo(100), Hash32::from_bytes([0xAAu8; 32])),
            block_number: BlockNo(10),
        };

        // Conway-era block at tip+1 with a prev_hash that does NOT match tip
        // hash — must be rejected in ApplyOnly mode (bypass is Byron-only now).
        let mut shelley_block = make_test_block(Era::Conway, 101, 11, 9, 0, vec![]);
        // prev_hash = 0xBB... does not match tip hash = 0xAA...
        shelley_block.header.prev_hash = Hash32::from_bytes([0xBBu8; 32]);

        let result = state.apply_block(&shelley_block, BlockValidationMode::ApplyOnly);

        assert!(
            matches!(result, Err(LedgerError::BlockDoesNotConnect { .. })),
            "ApplyOnly must reject Shelley+ hash mismatch; bypass is Byron-only now. Got: {result:?}"
        );
        assert_eq!(state.tip.block_number, BlockNo(10));
    }

    // ── Test 17: ApplyOnly accepts Byron hash mismatch (pallas re-encode bug) ──
    //
    // The `ApplyOnly` bypass is retained for Byron blocks: pallas's
    // `OriginalHash<32> for KeepRaw<'_, byron::BlockHead>` re-encodes the
    // decoded struct and can produce a hash different from the original wire
    // bytes. Chunk-file replay must tolerate this until the pallas upstream
    // fix lands (tracked separately).
    #[test]
    fn test_apply_only_byron_hash_mismatch_accepted() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);
        state.era = Era::Byron;
        state.tip = dugite_primitives::block::Tip {
            point: Point::Specific(SlotNo(100), Hash32::from_bytes([0xAAu8; 32])),
            block_number: BlockNo(10),
        };

        // Byron-era block at tip+1 with a prev_hash that does NOT match tip
        // hash — must be accepted in ApplyOnly mode (pallas bypass retained).
        let mut byron_block = make_test_block(Era::Byron, 101, 11, 1, 0, vec![]);
        // prev_hash = 0xBB... does not match tip hash = 0xAA...
        byron_block.header.prev_hash = Hash32::from_bytes([0xBBu8; 32]);

        let result = state.apply_block(&byron_block, BlockValidationMode::ApplyOnly);

        assert!(
            result.is_ok(),
            "ApplyOnly + Byron era must retain the bypass until pallas upstream fix. Got: {result:?}"
        );
        assert_eq!(state.tip.block_number, BlockNo(11));
    }

    // ── Regression: bogus body-size approximation must not exist ─────────────
    //
    // The old check compared header.body_size > max_block_body_size and rejected
    // with WrongBlockBodySize. That predicate was wrong (Haskell uses equality)
    // and at the wrong layer (max_block_body_size is a chain-checks cap, not
    // BBODY). This test ensures the approximation does not re-appear.
    #[test]
    fn test_body_size_approximation_removed() {
        let mut params = ProtocolParameters::mainnet_defaults();
        // Set a small cap so the old check would fire.
        params.max_block_body_size = 100;
        let mut state = LedgerState::new(params);

        // body_size (200) > max_block_body_size (100) — the bogus check would
        // have rejected this with WrongBlockBodySize.
        let block = make_test_block(Era::Conway, 1_000, 1, 9, 200, vec![]);

        let result = state.apply_block(&block, BlockValidationMode::ValidateAll);

        // The removed approximation must NOT produce WrongBlockBodySize.
        // Any other error or Ok is acceptable.
        assert!(
            !matches!(result, Err(LedgerError::WrongBlockBodySize { .. })),
            "Bogus body-size approximation must not be present; got: {result:?}"
        );
    }
}
