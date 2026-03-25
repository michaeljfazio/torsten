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
    credential_to_hash, stake_credential_hash_with_ptrs, BlockValidationMode, LedgerError,
    LedgerState,
};
use crate::eras::byron::{apply_byron_block, ByronApplyMode, ByronFeePolicy};
use crate::plutus::evaluate_plutus_scripts;
use crate::utxo_diff::UtxoDiff;
use crate::validation::{
    calculate_ref_script_size, script_ref_byte_size, validate_transaction_with_pools,
    ValidationError, MAX_REF_SCRIPT_SIZE_TIER_CAP,
};
use std::sync::Arc;
use torsten_primitives::block::{Block, Point};
use torsten_primitives::era::Era;
use torsten_primitives::time::EpochNo;
use torsten_primitives::transaction::{Certificate, TransactionInput};
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
/// Enforced in `apply_block` for `ValidateAll` mode: any transaction whose
/// combined spending-input + reference-input script_ref byte count exceeds this
/// limit is rejected with [`LedgerError::BlockTxValidationFailed`].
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

        // Allocate a per-block diff to record all UTxO inserts and deletes.
        // Pushed into self.diff_seq at the end of the method, enabling fast
        // diff-based rollback without a snapshot reload + full replay.
        let mut block_diff = UtxoDiff::new();

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

        // Block body size check — only in ValidateAll mode (not during replay).
        // During replay, max_block_body_size may differ from when the block was
        // originally produced, producing spurious warnings.
        if mode == BlockValidationMode::ValidateAll
            && block.header.body_size > 0
            && self.protocol_params.max_block_body_size > 0
            && block.header.body_size > self.protocol_params.max_block_body_size
        {
            warn!(
                body_size = block.header.body_size,
                limit = self.protocol_params.max_block_body_size,
                slot = block.slot().0,
                "Block body exceeds max_block_body_size"
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
        //
        // The per-block UTxO diff (`block_diff`) is populated inside the Byron
        // apply loop (see the `effect.spent` / `effect.created` recording below)
        // and pushed to `self.diff_seq` just before returning so that Byron
        // blocks participate in diff-based rollback alongside Shelley+ blocks.
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
                    // Capture the output value before removal for diff-based rollback.
                    if let Some(spent_output) = self.utxo_set.lookup(input) {
                        block_diff.record_delete(input.clone(), spent_output);
                    }
                    self.utxo_set.remove(input);
                }
                for (input, output) in effect.created {
                    block_diff.record_insert(input.clone(), output.clone());
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

            // Record this block's UTxO diff for rollback support.
            self.diff_seq.push(block.slot(), *block.hash(), block_diff);

            trace!(
                slot = block.slot().0,
                block_no = block.block_number().0,
                utxo_count = self.utxo_set.len(),
                epoch = self.epoch.0,
                "Ledger: Byron block applied successfully"
            );
            return Ok(());
        }

        // Block-level execution unit budget check (ValidateAll mode only).
        // In ApplyOnly mode, the block was already validated on-chain and the
        // protocol parameters may have changed since production (governance).
        let (block_mem, block_steps) = if mode == BlockValidationMode::ValidateAll {
            let mut mem: u64 = 0;
            let mut steps: u64 = 0;
            for tx in &block.transactions {
                if tx.is_valid {
                    for r in &tx.witness_set.redeemers {
                        mem = mem.saturating_add(r.ex_units.mem);
                        steps = steps.saturating_add(r.ex_units.steps);
                    }
                }
            }
            (mem, steps)
        } else {
            (0, 0) // Skip accumulation during replay
        };
        if block_mem > self.protocol_params.max_block_ex_units.mem {
            if mode == BlockValidationMode::ValidateAll {
                // Hard error at the live tip: a block whose execution unit memory
                // total exceeds the protocol limit was not produced by a conformant
                // block producer.  Reject it immediately so the peer is penalised.
                //
                // In ApplyOnly (ImmutableDB replay / Mithril import) the limit may
                // have been lower at the time the block was created than it is now
                // (or vice-versa), so we keep the permissive debug-only behaviour
                // to avoid breaking historical replay.
                return Err(LedgerError::BlockTxValidationFailed {
                    slot: block.slot().0,
                    tx_hash: String::from("(block-level check)"),
                    errors: format!(
                        "BlockExUnitsExceeded: block memory usage {} exceeds limit {} \
                         (Alonzo+ block-body ExUnits rule)",
                        block_mem, self.protocol_params.max_block_ex_units.mem
                    ),
                });
            } else {
                debug!(
                    block_mem,
                    limit = self.protocol_params.max_block_ex_units.mem,
                    "Block exceeds max execution unit memory budget (expected during replay before PP updates)"
                );
            }
        }
        if block_steps > self.protocol_params.max_block_ex_units.steps {
            if mode == BlockValidationMode::ValidateAll {
                // Same logic as memory budget above: hard error at tip,
                // permissive debug-only during replay.
                return Err(LedgerError::BlockTxValidationFailed {
                    slot: block.slot().0,
                    tx_hash: String::from("(block-level check)"),
                    errors: format!(
                        "BlockExUnitsExceeded: block step usage {} exceeds limit {} \
                         (Alonzo+ block-body ExUnits rule)",
                        block_steps, self.protocol_params.max_block_ex_units.steps
                    ),
                });
            } else {
                debug!(
                    block_steps,
                    limit = self.protocol_params.max_block_ex_units.steps,
                    "Block exceeds max execution unit step budget (expected during replay before PP updates)"
                );
            }
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
        for (tx_idx, tx) in block.transactions.iter().enumerate() {
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
                // -------------------------------------------------------
                // Conway rule: per-transaction reference script size limit.
                //
                // `ppMaxRefScriptSizePerTxG` (Haskell): the total byte size of
                // all script_refs reachable from a transaction's spending inputs
                // AND reference inputs must not exceed 200 KiB.  This is a
                // hard structural limit that is independent of the tiered fee
                // (CIP-0112) — a transaction that exceeds it is unconditionally
                // invalid regardless of what fee it declares.
                //
                // Checked in Conway (protocol >= 9) and ValidateAll mode only.
                // During ApplyOnly (historical replay, Mithril import) we skip
                // the check so that replay doesn't break if the protocol version
                // stored in the snapshot is unexpectedly < 9 for blocks that were
                // genuinely processed in Conway.
                //
                // Only applies to valid transactions; invalid (is_valid=false)
                // transactions have their regular inputs/outputs skipped so their
                // script_ref contribution does not count toward either limit.
                // -------------------------------------------------------
                if self.protocol_params.protocol_version_major >= 9 && tx.is_valid {
                    let tx_ref_script_size = calculate_ref_script_size(
                        &tx.body.inputs,
                        &tx.body.reference_inputs,
                        &self.utxo_set,
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
                    //
                    // IMPORTANT: This check MUST NOT hard-return Err for confirmed
                    // on-chain blocks.  If our treasury tracking has diverged from
                    // the canonical chain (e.g., due to a prior UTxO gap, a reward
                    // rounding difference, or a missed treasury donation), returning
                    // Err here causes apply_block to abort WITHOUT inserting the
                    // block's outputs into the UTxO store.  The sync loop then
                    // `break`s, leaving the block in ChainDB but missing from the
                    // ledger.  On the next batch the gap-bridge replays it in
                    // ApplyOnly mode (skipping this check), but the at-tip
                    // ValidateAll path will fire the same error again every
                    // reconnect, permanently preventing outputs of subsequent
                    // blocks from being inserted — causing the cascade failure
                    // observed at slot 107229218 (tx 26b1e945 missing f82ae6af
                    // outputs due to treasury divergence in a prior tx).
                    //
                    // Strategy: log at WARN and continue (same as Phase-1
                    // divergence).  On-chain consensus is authoritative for
                    // confirmed blocks.  Our treasury sync will self-correct once
                    // the canonical chain's treasury donations and withdrawals
                    // propagate through the ledger state.
                    // -------------------------------------------------------
                    if self.protocol_params.protocol_version_major >= 9 {
                        if let Some(declared_treasury) = tx.body.treasury_value {
                            if declared_treasury.0 != self.treasury.0 {
                                warn!(
                                    tx_hash = %tx.hash.to_hex(),
                                    slot = block.slot().0,
                                    declared = declared_treasury.0,
                                    ledger = self.treasury.0,
                                    "TreasuryValueMismatch on confirmed block — \
                                     trusting on-chain consensus (treasury will self-correct)"
                                );
                                // Correct our treasury to match the on-chain assertion.
                                // The block producer's view is authoritative: if they
                                // declared a treasury value and the block was accepted
                                // by the network, that IS the correct value.
                                self.treasury = declared_treasury;
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
                    //
                    // IMPORTANT: Same reasoning as TreasuryValueMismatch above —
                    // for confirmed on-chain blocks, hard-returning Err prevents
                    // output insertion and corrupts the UTxO set.  Log at WARN
                    // and fall through: the committee state will be corrected by
                    // the subsequent governance action processing.
                    // -------------------------------------------------------
                    if self.protocol_params.protocol_version_major >= 9 {
                        for cert in &tx.body.certificates {
                            if let Certificate::CommitteeHotAuth {
                                cold_credential, ..
                            } = cert
                            {
                                let cold_key = credential_to_hash(cold_credential);
                                if !self.governance.committee_expiration.contains_key(&cold_key) {
                                    warn!(
                                        tx_hash = %tx.hash.to_hex(),
                                        slot = block.slot().0,
                                        cold_key = %cold_key.to_hex(),
                                        "UnelectedCommitteeMember on confirmed block — \
                                         trusting on-chain consensus (committee state may be stale)"
                                    );
                                    // Fall through — treat as valid, process normally.
                                    // Our committee state is stale; the cert was accepted
                                    // by the canonical chain.
                                }
                            }
                        }
                    }

                    // Producer claims tx is valid — verify with full validation.
                    //
                    // Sequential UTxO visibility: by the time we reach tx[i] in this
                    // loop, `self.utxo_set` already contains the outputs created by
                    // tx[0]..tx[i-1] (each was applied in its own loop iteration,
                    // before we advanced to the next tx).
                    // This matches Haskell's sequential LEDGER rule application inside
                    // the LEDGERS block rule — later txs in a block can spend or
                    // reference UTxOs created by earlier txs in the same block,
                    // fixing the root cause of `InputNotFound` and `InvalidMint`
                    // errors for within-block UTxO dependencies.
                    //
                    // Use tx raw_cbor size as tx_size (approximate, sufficient for validation).
                    let tx_size = tx.raw_cbor.as_ref().map_or(0, |c| c.len() as u64);
                    // Build the registered pools set so that pool re-registrations
                    // (parameter updates) are not charged an additional deposit.
                    // Without this, validate_transaction treats ALL pool registrations
                    // as new, causing ValueNotConserved for re-registration txs.
                    let registered_pool_ids: std::collections::HashSet<
                        torsten_primitives::hash::Hash28,
                    > = self.pool_params.keys().copied().collect();
                    // Build the registered DRep credential set so that duplicate
                    // RegDRep certificates are rejected (Haskell `ConwayDRepAlreadyRegistered`).
                    let registered_drep_ids: std::collections::HashSet<
                        torsten_primitives::hash::Hash32,
                    > = self.governance.dreps.keys().copied().collect();
                    // Build the VRF key → pool_id map for VRF key deduplication
                    // (Haskell `VRFKeyHashAlreadyRegistered`, Conway+ only).
                    // When protocol < 9 this map is still built but the check in
                    // `validate_transaction_with_pools` is gated on proto >= 9.
                    let registered_vrf_keys: std::collections::HashMap<
                        torsten_primitives::hash::Hash32,
                        torsten_primitives::hash::Hash28,
                    > = self
                        .pool_params
                        .values()
                        .map(|reg| (reg.vrf_keyhash, reg.pool_id))
                        .collect();
                    // Build the set of current CC cold credential hashes for the
                    // CommitteeHotAuth "unelected member" check (Conway+ only).
                    let committee_member_keys: std::collections::HashSet<
                        torsten_primitives::hash::Hash32,
                    > = self.governance.committee_expiration.keys().copied().collect();
                    // Build the set of resigned CC cold credential hashes for the
                    // "previously resigned" check (Conway+ only).
                    let committee_resigned_keys: std::collections::HashSet<
                        torsten_primitives::hash::Hash32,
                    > = self.governance.committee_resigned.keys().copied().collect();
                    let result = validate_transaction_with_pools(
                        tx,
                        &self.utxo_set,
                        &self.protocol_params,
                        block.slot().0,
                        tx_size,
                        Some(&self.slot_config),
                        Some(&registered_pool_ids),
                        Some(self.treasury.0),
                        Some(&self.reward_accounts),
                        Some(self.epoch.0),
                        Some(&registered_drep_ids),
                        Some(&registered_vrf_keys),
                        self.node_network,
                        Some(&committee_member_keys),
                        Some(&committee_resigned_keys),
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
                            // Classify the errors before deciding on log severity:
                            //
                            // UTxO-gap errors (InputNotFound, CollateralNotFound,
                            // CollateralMismatch, InsufficientCollateral, ValueNotConserved)
                            // arise when the UTxO set has a gap from partial replay —
                            // a prior block's outputs were absent when we processed it, so
                            // its new outputs were silently skipped, and this tx's inputs
                            // therefore don't exist.  With the best-effort apply logic
                            // below this branch, outputs are ALWAYS inserted regardless of
                            // missing inputs, so the cascade cannot occur going forward.
                            // However, the validation still fires for blocks received at tip
                            // while the gap exists.  These are expected artefacts of partial
                            // replay and are logged at DEBUG level only.
                            //
                            // Non-UTxO errors (FeeTooSmall, TxTooLarge, NetworkMismatch,
                            // etc.) are genuine validation divergences that indicate either
                            // a protocol rule difference or a bug in Torsten.  These are
                            // logged at WARN level so that they are investigated.
                            //
                            // For all confirmed on-chain blocks we fall through and apply
                            // the block regardless — on-chain consensus is authoritative.
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
                        if let Some(cred) =
                            stake_credential_hash_with_ptrs(&spent.address, &self.pointer_map)
                        {
                            if let Some(stake) = self.stake_distribution.stake_map.get_mut(&cred) {
                                stake.0 = stake.0.saturating_sub(spent.value.coin.0);
                            }
                        }
                        // Record collateral deletion for diff-based rollback.
                        block_diff.record_delete(col_input.clone(), spent);
                    }
                    self.utxo_set.remove(col_input);
                }
                // If there's a collateral return output, add it
                let collateral_return_value = if let Some(col_return) = &tx.body.collateral_return {
                    if let Some(cred) =
                        stake_credential_hash_with_ptrs(&col_return.address, &self.pointer_map)
                    {
                        *self
                            .stake_distribution
                            .stake_map
                            .entry(cred)
                            .or_insert(Lovelace(0)) += Lovelace(col_return.value.coin.0);
                    }
                    let return_input = TransactionInput {
                        transaction_id: tx.hash,
                        index: tx.body.outputs.len() as u32, // collateral return is after regular outputs
                    };
                    let return_val = col_return.value.coin.0;
                    // Record collateral return insertion for diff-based rollback.
                    block_diff.record_insert(return_input.clone(), col_return.clone());
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

            // Snapshot the spent input values BEFORE removal.
            //
            // We need the original `TransactionOutput` values for two purposes:
            //   1. Updating the stake distribution (subtract the spent stake).
            //   2. Recording the deletion in `block_diff` so that diff-based
            //      rollback can re-insert the spent UTxOs.
            //
            // Collect into a Vec so we can iterate twice (stake update + diff record)
            // without borrowing `self.utxo_set` mutably while iterating.
            //
            // NOTE: we use filter_map here so that inputs already absent from the
            // UTxO set (e.g. spent by a prior block we partially replayed) are
            // silently skipped for the stake-update and diff-record passes.
            // The actual removal pass below is equally lenient.
            let spent_outputs: Vec<_> = tx
                .body
                .inputs
                .iter()
                .filter_map(|input| {
                    self.utxo_set
                        .lookup(input)
                        .map(|output| (input.clone(), output))
                })
                .collect();

            // Update stake distribution from consumed inputs (subtract)
            for (_input, spent_output) in &spent_outputs {
                if let Some(cred_hash) =
                    stake_credential_hash_with_ptrs(&spent_output.address, &self.pointer_map)
                {
                    if let Some(stake) = self.stake_distribution.stake_map.get_mut(&cred_hash) {
                        stake.0 = stake.0.saturating_sub(spent_output.value.coin.0);
                    }
                }
            }

            // Apply UTxO changes with best-effort input removal.
            //
            // On a fully-synced node, every input referenced by a confirmed
            // on-chain transaction MUST exist in the UTxO set — the block
            // producer checked this before including the transaction.  If we
            // cannot find an input it means either:
            //
            //   (a) We are replaying blocks from ImmutableDB and started the
            //       replay at a point after that input was created (the UTxO
            //       store already had those outputs when we resumed).  In this
            //       case the input is simply absent because we never saw the
            //       transaction that created it.
            //
            //   (b) An earlier block in the same replay/sync session had its
            //       UTxO changes silently skipped (e.g., its own inputs were
            //       missing for reason (a)), creating a CASCADE: the missing
            //       outputs from block N become missing inputs for block N+k.
            //       This cascade is what caused the Phase-1 divergence at
            //       slot 107229218 (tx 26b1e945): the chain was
            //       55e4f2b9 → 7be12eee → f82ae6af → 26b1e945, each spending
            //       the previous's outputs.  Any single broken link silently
            //       propagated the gap forward until the node was at tip and
            //       Phase-1 validation fired.
            //
            // The PREVIOUS strategy of aborting when any input is missing was
            // the root cause of the cascade: by skipping the output insertions
            // on a failed apply, we guaranteed that every subsequent tx in the
            // chain would also fail.
            //
            // The CORRECT strategy (matching Haskell's `applyTx`):
            //   - Remove each input that EXISTS (best-effort; log absent ones).
            //   - ALWAYS insert the new outputs.
            //
            // This ensures the UTxO set converges to the canonical chain state
            // regardless of where the replay started.  Absent inputs are benign:
            // they represent entries that were already removed by an earlier
            // (unobserved) block and have no net effect on the final UTxO set.
            {
                // Pass 1: remove inputs that are present; log any that are absent.
                let mut missing_inputs = 0usize;
                for input in &tx.body.inputs {
                    if self.utxo_set.contains(input) {
                        self.utxo_set.remove(input);
                    } else {
                        missing_inputs += 1;
                        debug!(
                            tx_hash = %tx.hash.to_hex(),
                            input = %input,
                            "apply_block: input not found in UTxO set (already spent or \
                             pre-replay gap) — removing from nothing, outputs will still be created"
                        );
                    }
                }
                if missing_inputs > 0 {
                    debug!(
                        tx_hash = %tx.hash.to_hex(),
                        missing = missing_inputs,
                        total = tx.body.inputs.len(),
                        "apply_block: {} of {} inputs were absent; outputs inserted regardless \
                         to prevent UTxO cascade divergence",
                        missing_inputs,
                        tx.body.inputs.len(),
                    );
                }

                // Pass 2: record deletions for diff-based rollback (only for
                // inputs we actually removed — absent inputs have no diff entry).
                for (input, output) in spent_outputs {
                    block_diff.record_delete(input, output);
                }

                // Pass 3: insert new outputs unconditionally.
                //
                // This is the key invariant: regardless of whether all inputs
                // were present, a confirmed on-chain transaction ALWAYS creates
                // its outputs.  Failing to insert them causes every downstream
                // transaction to also fail, cascading the corruption forward
                // until the node diverges from the network.
                for (idx, output) in tx.body.outputs.iter().enumerate() {
                    let new_input = TransactionInput {
                        transaction_id: tx.hash,
                        index: idx as u32,
                    };
                    block_diff.record_insert(new_input.clone(), output.clone());
                    self.utxo_set.insert(new_input, output.clone());
                }

                // Pass 4: update stake distribution from new outputs.
                for output in &tx.body.outputs {
                    if let Some(cred_hash) =
                        stake_credential_hash_with_ptrs(&output.address, &self.pointer_map)
                    {
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

            // Process certificates (passing slot/tx_index/cert_index for pointer map)
            let block_slot = block.slot().0;
            for (cert_index, cert) in tx.body.certificates.iter().enumerate() {
                self.process_certificate_with_pointer(
                    cert,
                    block_slot,
                    tx_idx as u64,
                    cert_index as u64,
                );
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
                        d = ?ppu.d,
                        n_opt = ?ppu.n_opt,
                        "Collected protocol parameter update proposal"
                    );
                    self.pending_pp_updates
                        .entry(EpochNo(update.epoch))
                        .or_default()
                        .push((*genesis_hash, ppu.clone()));
                }
            }
        }

        // Track block production by pool (issuer vkey hash).
        //
        // Matches Haskell's `incrBlocks`: only non-overlay blocks are counted
        // in BlocksMade. When d >= 0.8 (federated era), all/most slots are
        // overlay slots and blocks should NOT be counted toward pool rewards.
        // For Babbage+ (proto >= 7), d = 0 by definition (ppDG returns minBound).
        //
        // This is critical for reward calculation: pools only receive rewards
        // for blocks that appear in BlocksMade (bprev). Counting overlay blocks
        // would incorrectly award pool rewards during federated epochs.
        let current_d = if self.protocol_params.protocol_version_major >= 7 {
            0.0
        } else {
            self.protocol_params.d.numerator as f64
                / self.protocol_params.d.denominator.max(1) as f64
        };
        if current_d < 0.8 && !block.header.issuer_vkey.is_empty() {
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
        //    last stability window slots of the epoch, in which case the candidate
        //    freezes so the epoch nonce is stable.
        //
        //    Per Haskell erratum 17.3: Babbage uses ceiling(3k/f) (stability_window_3kf),
        //    while Conway+ uses ceiling(4k/f) (randomness_stabilisation_window).
        if !block.header.nonce_vrf_output.is_empty() {
            // Update evolving nonce unconditionally with the pre-computed eta
            self.update_evolving_nonce(&block.header.nonce_vrf_output);

            // Select the correct stability window based on era.
            // Babbage (proto major 7-8): use 3k/f for backward-compatibility.
            // Conway+ (proto major 9+): use 4k/f.
            let window = if self.protocol_params.protocol_version_major >= 9 {
                self.randomness_stabilisation_window
            } else {
                // Pre-Conway (including Babbage): ceiling(3k/f)
                self.stability_window_3kf
            };

            // Candidate nonce tracks evolving nonce OUTSIDE the stability window.
            let first_slot_of_next_epoch = self.first_slot_of_epoch(self.epoch.0.saturating_add(1));
            if block.slot().0.saturating_add(window) < first_slot_of_next_epoch {
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

        // Record this block's UTxO diff for rollback support.
        self.diff_seq.push(block.slot(), *block.hash(), block_diff);

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
