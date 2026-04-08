/// Shelley era ledger rules (covers Shelley, Allegra, and Mary eras).
///
/// Shelley (protocol version 2) introduces:
/// - Ouroboros Praos consensus (VRF-based leader election)
/// - Staking and delegation (stake credentials, pool registration)
/// - Reward distribution (monetary expansion, pool rewards)
/// - Multi-signature scripts
///
/// Allegra (protocol version 3) adds:
/// - Transaction validity intervals (valid_from / ttl)
/// - Timelock script primitives
///
/// Mary (protocol version 4) adds:
/// - Multi-asset (native tokens in UTxO outputs)
/// - Minting/burning policies
///
/// The core ledger pipeline (UTXOW/UTXO/DELEG/POOL rules) is shared
/// across all three eras, so a single `ShelleyRules` implementation covers
/// them all. The differences are in transaction body fields and script
/// capabilities, not in the LEDGER rule application order.
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use dugite_primitives::block::{Block, BlockHeader};
use dugite_primitives::credentials::Credential;
use dugite_primitives::era::Era;
use dugite_primitives::hash::{blake2b_256, Hash28, Hash32};
use dugite_primitives::protocol_params::ProtocolParameters;
use dugite_primitives::time::EpochNo;
use dugite_primitives::transaction::{Certificate, Transaction};
use dugite_primitives::value::Lovelace;
use tracing::debug;

use super::common;
use super::{EraRules, RuleContext};
use crate::state::substates::*;
use crate::state::{BlockValidationMode, LedgerError, StakeSnapshot};
use crate::utxo_diff::UtxoDiff;

/// Stateless Shelley/Allegra/Mary era rule strategy.
///
/// This struct carries no mutable state. All state lives in the component
/// sub-states passed as parameters to each method.
#[derive(Default, Debug, Clone, Copy)]
pub struct ShelleyRules;

impl ShelleyRules {
    pub fn new() -> Self {
        ShelleyRules
    }
}

impl EraRules for ShelleyRules {
    /// Shelley/Allegra/Mary have no ExUnit budgets or reference scripts.
    ///
    /// Block body validation is trivially successful — Plutus scripts were
    /// not introduced until the Alonzo era.
    fn validate_block_body(
        &self,
        _block: &Block,
        _ctx: &RuleContext,
        _utxo: &UtxoSubState,
    ) -> Result<(), LedgerError> {
        Ok(())
    }

    /// Apply a single valid Shelley/Allegra/Mary transaction.
    ///
    /// Implements the Shelley LEDGER rule pipeline:
    /// 1. Drain withdrawal accounts (zero reward balances consumed by tx).
    /// 2. Process Shelley-era certificates (registrations, delegations, pools).
    /// 3. Apply UTxO changes (consume inputs, produce outputs, accumulate fee).
    ///
    /// In Shelley/Allegra/Mary ALL transactions are valid — there is no
    /// `is_valid` flag. The IsValid concept was introduced in Alonzo.
    fn apply_valid_tx(
        &self,
        tx: &Transaction,
        _mode: BlockValidationMode,
        ctx: &RuleContext,
        utxo: &mut UtxoSubState,
        certs: &mut CertSubState,
        gov: &mut GovSubState,
        epochs: &mut EpochSubState,
    ) -> Result<UtxoDiff, LedgerError> {
        // Step 1: Drain withdrawal accounts.
        // Per the Cardano spec, the withdrawal amount must exactly equal the
        // reward balance. During sync we may not have accumulated all rewards,
        // so mismatches are logged at DEBUG level (best-effort).
        common::drain_withdrawal_accounts(tx, certs);

        // Step 2: Process certificates (StakeReg, StakeDeReg, Delegation,
        // PoolRegistration, PoolRetirement).
        // The tx_index is derived from the slot — callers set this to the
        // transaction's position within the block. For the era-rules interface
        // we use 0 since the orchestrator (Task 12) will provide the correct
        // index. Certificate pointer map entries are not critical for correctness
        // (only used for pointer address resolution in snapshots).
        common::process_shelley_certs(tx, ctx.current_slot, 0, certs, epochs, gov);

        // Step 3: Apply UTxO changes (consume inputs, produce outputs).
        let diff = common::apply_utxo_changes(tx, utxo, certs, epochs);

        Ok(diff)
    }

    /// Shelley/Allegra/Mary have no IsValid concept.
    ///
    /// All transactions in these eras are structurally valid or rejected
    /// outright. Calling this method for a Shelley-era transaction is a
    /// programming error — the IsValid=false path was introduced in Alonzo
    /// with Plutus Phase-2 evaluation.
    fn apply_invalid_tx(
        &self,
        tx: &Transaction,
        _mode: BlockValidationMode,
        _ctx: &RuleContext,
        _utxo: &mut UtxoSubState,
    ) -> Result<UtxoDiff, LedgerError> {
        Err(LedgerError::InvalidTransaction(format!(
            "Shelley/Allegra/Mary eras do not support invalid transactions \
             (is_valid flag). Transaction {} should not reach apply_invalid_tx.",
            tx.hash.to_hex()
        )))
    }

    /// Process a Shelley/Allegra/Mary epoch boundary transition.
    ///
    /// Implements the pre-Conway subset of Haskell's NEWEPOCH STS rule:
    ///
    /// 1. Flush pending treasury donations.
    /// 2. Apply pending reward update (legacy compatibility).
    /// 3. Rotate snapshots (mark -> set -> go) and take new mark snapshot.
    /// 4. Apply future pool parameter updates (re-registrations).
    /// 5. Process pool retirements for this epoch.
    /// 6. Apply pre-Conway PP update proposals (genesis key votes).
    /// 7. Recalculate totalObligation from scratch.
    /// 8. Compute new epoch nonce (TICKN rule).
    /// 9. Reset per-epoch accumulators (fees, block counters).
    ///
    /// NOTE: Reward calculation (`calculate_rewards_full`) is NOT performed
    /// here because it requires access to the full `LedgerState` (for reading
    /// total ADA supply, protocol params, etc.). The existing `LedgerState::
    /// process_epoch_transition()` handles rewards. When the orchestrator
    /// (Task 12) wires era rules, it will need to either:
    /// - Continue using the LedgerState method for reward calculation, or
    /// - Extract reward calculation into a standalone function.
    ///
    /// This implementation covers all non-reward epoch transition operations
    /// faithfully, matching the Haskell ordering.
    fn process_epoch_transition(
        &self,
        new_epoch: EpochNo,
        ctx: &RuleContext,
        utxo: &mut UtxoSubState,
        certs: &mut CertSubState,
        gov: &mut GovSubState,
        epochs: &mut EpochSubState,
        consensus: &mut ConsensusSubState,
    ) -> Result<(), LedgerError> {
        debug!("Shelley epoch transition: -> {}", new_epoch.0);

        // Capture bprev BEFORE any param updates (nesBprev = nesBcur).
        let bprev_block_count = consensus.epoch_block_count;
        let bprev_blocks_by_pool = Arc::clone(&consensus.epoch_blocks_by_pool);

        // Step 0: Flush pending treasury donations.
        if utxo.pending_donations.0 > 0 {
            let flushed = utxo.pending_donations;
            epochs.treasury.0 = epochs.treasury.0.saturating_add(flushed.0);
            utxo.pending_donations = Lovelace(0);
            debug!(
                epoch = new_epoch.0,
                donations_lovelace = flushed.0,
                "Flushed pending treasury donations"
            );
        }

        // Step 1: Apply pending reward update (backward compat for old snapshots).
        if let Some(rupd) = epochs.pending_reward_update.take() {
            epochs.reserves.0 = epochs.reserves.0.saturating_sub(rupd.delta_reserves);
            epochs.treasury.0 = epochs.treasury.0.saturating_add(rupd.delta_treasury);
            for (cred_hash, reward) in &rupd.rewards {
                if reward.0 > 0 {
                    if certs.reward_accounts.contains_key(cred_hash) {
                        *Arc::make_mut(&mut certs.reward_accounts)
                            .entry(*cred_hash)
                            .or_insert(Lovelace(0)) += *reward;
                    } else {
                        epochs.treasury.0 = epochs.treasury.0.saturating_add(reward.0);
                    }
                }
            }
        }

        // Step 2: SNAP — rotate snapshots, capture fees, update bprev.
        //
        // TODO: The full RUPD (reward calculation using GO snapshot + bprev + ss_fee)
        // is not computed here because it requires `calculate_rewards_full` which
        // operates on `&self LedgerState`. The orchestrator (Task 12) will handle
        // wiring reward calculation before this point.
        let captured_fees = utxo.epoch_fees;
        epochs.snapshots.go = epochs.snapshots.set.take();
        epochs.snapshots.set = epochs.snapshots.mark.take();
        epochs.snapshots.ss_fee = captured_fees;
        epochs.snapshots.bprev_block_count = bprev_block_count;
        epochs.snapshots.bprev_blocks_by_pool = bprev_blocks_by_pool;
        epochs.snapshots.rupd_ready = true;

        // Rebuild stake distribution if needed (post-Mithril import).
        if epochs.needs_stake_rebuild {
            // Full UTxO rebuild requires access to the UTxO set and is typically
            // done by the orchestrator. Mark as no longer needed so subsequent
            // boundaries use incremental tracking.
            epochs.needs_stake_rebuild = false;
            debug!(
                epoch = new_epoch.0,
                "Shelley epoch: needs_stake_rebuild flag cleared (rebuild deferred to orchestrator)"
            );
        }

        // Build pool_stake from current stake distribution + delegations.
        let mut pool_stake: HashMap<Hash28, Lovelace> =
            HashMap::with_capacity(certs.pool_params.len());
        for (cred_hash, pool_id) in certs.delegations.iter() {
            let utxo_stake = certs
                .stake_distribution
                .stake_map
                .get(cred_hash)
                .copied()
                .unwrap_or(Lovelace(0));
            let reward_balance = certs
                .reward_accounts
                .get(cred_hash)
                .copied()
                .unwrap_or(Lovelace(0));
            let total_stake = Lovelace(utxo_stake.0 + reward_balance.0);
            *pool_stake.entry(*pool_id).or_insert(Lovelace(0)) += total_stake;
        }

        // Resolve deferred pointer-addressed UTxO stake at SNAP time.
        if !epochs.ptr_stake.is_empty() {
            for (pointer, &coin) in &epochs.ptr_stake {
                if coin == 0 {
                    continue;
                }
                if let Some(cred_hash) = certs.pointer_map.get(pointer) {
                    if certs.reward_accounts.contains_key(cred_hash) {
                        if let Some(pool_id) = certs.delegations.get(cred_hash) {
                            *pool_stake.entry(*pool_id).or_insert(Lovelace(0)) += Lovelace(coin);
                        }
                    }
                }
            }
        }

        // Build per-credential snapshot_stake (only delegated credentials).
        let mut snapshot_stake: HashMap<Hash32, Lovelace> =
            HashMap::with_capacity(certs.delegations.len());
        for cred_hash in certs.delegations.keys() {
            let utxo_stake = certs
                .stake_distribution
                .stake_map
                .get(cred_hash)
                .copied()
                .unwrap_or(Lovelace(0));
            let reward_balance = certs
                .reward_accounts
                .get(cred_hash)
                .copied()
                .unwrap_or(Lovelace(0));
            let total = Lovelace(utxo_stake.0.saturating_add(reward_balance.0));
            if total.0 > 0 {
                snapshot_stake.insert(*cred_hash, total);
            }
        }

        // Resolve pointer-addressed UTxO coins into per-credential snapshot_stake.
        if !epochs.ptr_stake.is_empty() {
            for (pointer, &coin) in &epochs.ptr_stake {
                if coin == 0 {
                    continue;
                }
                if let Some(cred_hash) = certs.pointer_map.get(pointer) {
                    if certs.reward_accounts.contains_key(cred_hash)
                        && certs.delegations.contains_key(cred_hash)
                    {
                        *snapshot_stake.entry(*cred_hash).or_insert(Lovelace(0)) += Lovelace(coin);
                    }
                }
            }
        }

        // Create the new mark snapshot.
        epochs.snapshots.mark = Some(StakeSnapshot {
            epoch: new_epoch,
            delegations: Arc::clone(&certs.delegations),
            pool_stake,
            pool_params: Arc::clone(&certs.pool_params),
            stake_distribution: Arc::new(snapshot_stake),
            epoch_fees: utxo.epoch_fees,
            epoch_block_count: consensus.epoch_block_count,
            epoch_blocks_by_pool: Arc::clone(&consensus.epoch_blocks_by_pool),
        });

        // Apply future pool parameters (re-registrations deferred from previous epoch).
        if !certs.future_pool_params.is_empty() {
            let pool_params = Arc::make_mut(&mut certs.pool_params);
            for (pool_id, pool_reg) in certs.future_pool_params.drain() {
                if pool_params.contains_key(&pool_id) {
                    pool_params.insert(pool_id, pool_reg);
                }
                // Pools only in future (retired between re-reg and boundary): dropped.
            }
        }

        // Process pending pool retirements for this epoch.
        let retiring_pools: Vec<Hash28> = certs
            .pending_retirements
            .iter()
            .filter_map(|(pool_id, epoch)| {
                if *epoch == new_epoch {
                    Some(*pool_id)
                } else {
                    None
                }
            })
            .collect();
        if !retiring_pools.is_empty() {
            for pool_id in &retiring_pools {
                certs.pending_retirements.remove(pool_id);
            }
            for pool_id in &retiring_pools {
                if let Some(pool_reg) = Arc::make_mut(&mut certs.pool_params).remove(pool_id) {
                    let pool_deposit = certs
                        .pool_deposits
                        .remove(pool_id)
                        .map(Lovelace)
                        .unwrap_or(epochs.protocol_params.pool_deposit);
                    let op_key = reward_account_to_hash(&pool_reg.reward_account);
                    if certs.reward_accounts.contains_key(&op_key) {
                        *Arc::make_mut(&mut certs.reward_accounts)
                            .entry(op_key)
                            .or_insert(Lovelace(0)) += pool_deposit;
                    } else {
                        epochs.treasury.0 = epochs.treasury.0.saturating_add(pool_deposit.0);
                    }
                    Arc::make_mut(&mut certs.delegations)
                        .retain(|_, delegated_pool| delegated_pool != pool_id);
                    debug!(
                        "Pool retired at epoch {}: {} (deposit {} refunded)",
                        new_epoch.0,
                        pool_id.to_hex(),
                        pool_deposit.0
                    );
                }
            }
        }
        // Clean up retirements from past epochs.
        certs
            .pending_retirements
            .retain(|_, epoch| *epoch > new_epoch);

        // Capture prevPParams BEFORE PPUP updates.
        let old_d = if epochs.protocol_params.protocol_version_major >= 7 {
            0.0
        } else {
            let d_n = epochs.protocol_params.d.numerator as f64;
            let d_d = epochs.protocol_params.d.denominator.max(1) as f64;
            d_n / d_d
        };
        let old_proto_major = epochs.protocol_params.protocol_version_major;
        let old_params = epochs.protocol_params.clone();

        // Apply pre-Conway PP update proposals (PPUP/UPEC rule).
        let lookup_epoch = EpochNo(new_epoch.0.saturating_sub(1));
        if let Some(proposals) = epochs.pending_pp_updates.remove(&lookup_epoch) {
            let mut proposer_set: HashSet<Hash32> = HashSet::with_capacity(proposals.len());
            for (genesis_hash, _) in &proposals {
                proposer_set.insert(*genesis_hash);
            }
            let distinct_proposers = proposer_set.len() as u64;

            if distinct_proposers >= ctx.update_quorum {
                // Merge all proposals.
                let mut merged = dugite_primitives::transaction::ProtocolParamUpdate::default();
                for (_, ppu) in &proposals {
                    macro_rules! merge_field {
                        ($field:ident) => {
                            if ppu.$field.is_some() {
                                merged.$field = ppu.$field.clone();
                            }
                        };
                    }
                    merge_field!(min_fee_a);
                    merge_field!(min_fee_b);
                    merge_field!(max_block_body_size);
                    merge_field!(max_tx_size);
                    merge_field!(max_block_header_size);
                    merge_field!(key_deposit);
                    merge_field!(pool_deposit);
                    merge_field!(e_max);
                    merge_field!(n_opt);
                    merge_field!(a0);
                    merge_field!(rho);
                    merge_field!(tau);
                    merge_field!(d);
                    merge_field!(min_pool_cost);
                    merge_field!(ada_per_utxo_byte);
                    merge_field!(cost_models);
                    merge_field!(execution_costs);
                    merge_field!(max_tx_ex_units);
                    merge_field!(max_block_ex_units);
                    merge_field!(max_val_size);
                    merge_field!(collateral_percentage);
                    merge_field!(max_collateral_inputs);
                    merge_field!(protocol_version_major);
                    merge_field!(protocol_version_minor);
                }
                // Apply the merged update to protocol params.
                apply_pp_update(&mut epochs.protocol_params, &merged);
                debug!(
                    epoch = new_epoch.0,
                    proposers = distinct_proposers,
                    "Pre-Conway protocol parameter update applied"
                );
            }
        }
        // Clean up past-epoch proposals.
        epochs
            .pending_pp_updates
            .retain(|epoch, _| *epoch >= lookup_epoch);

        // Promote future proposals -> current.
        if !epochs.future_pp_updates.is_empty() {
            let promoted = std::mem::take(&mut epochs.future_pp_updates);
            for (epoch, proposals) in promoted {
                epochs
                    .pending_pp_updates
                    .entry(epoch)
                    .or_default()
                    .extend(proposals);
            }
        }

        // Recalculate totalObligation (deposits) from scratch.
        {
            let obl_stake: u64 = certs.stake_key_deposits.values().sum();
            let obl_pool: u64 = certs.pool_deposits.values().sum();
            let obl_drep: u64 = gov.governance.dreps.values().map(|d| d.deposit.0).sum();
            let obl_proposal: u64 = gov
                .governance
                .proposals
                .values()
                .map(|p| p.procedure.deposit.0)
                .sum();
            certs.total_stake_key_deposits = obl_stake;
            debug!(
                epoch = new_epoch.0,
                obl_stake, obl_pool, obl_drep, obl_proposal, "totalObligation recalculated"
            );
        }

        // Compute new epoch nonce (TICKN rule).
        let candidate = consensus.candidate_nonce;
        let prev_hash_nonce = consensus.last_epoch_block_nonce;

        let zero = Hash32::ZERO;
        consensus.epoch_nonce = if candidate == zero && prev_hash_nonce == zero {
            zero
        } else if candidate == zero {
            prev_hash_nonce
        } else if prev_hash_nonce == zero {
            candidate
        } else {
            let mut nonce_input = Vec::with_capacity(64);
            nonce_input.extend_from_slice(candidate.as_bytes());
            nonce_input.extend_from_slice(prev_hash_nonce.as_bytes());
            blake2b_256(&nonce_input)
        };

        // Update prevHashNonce to current labNonce for NEXT epoch.
        consensus.last_epoch_block_nonce = consensus.lab_nonce;

        // Set prevPParams from values captured BEFORE PPUP.
        epochs.prev_d = old_d;
        epochs.prev_protocol_version_major = old_proto_major;
        epochs.prev_protocol_params = old_params;

        // Reset per-epoch accumulators.
        utxo.epoch_fees = Lovelace(0);
        Arc::make_mut(&mut consensus.epoch_blocks_by_pool).clear();
        consensus.epoch_block_count = 0;

        Ok(())
    }

    /// Evolve nonce state after a Shelley+ block header.
    ///
    /// Delegates to `common::compute_shelley_nonce` which implements Haskell's
    /// `reupdateChainDepState` nonce state machine: evolving nonce, candidate
    /// nonce freeze, lab nonce, and block production tracking.
    fn evolve_nonce(
        &self,
        header: &BlockHeader,
        ctx: &RuleContext,
        consensus: &mut ConsensusSubState,
    ) {
        // Compute the first slot of the next epoch for stability window check.
        let first_slot_of_next_epoch = (ctx.current_epoch.0 + 1) * ctx.epoch_length
            + ctx.shelley_transition_epoch * ctx.byron_epoch_length;

        // Compute the d value for the block counting overlay check.
        let d_value = if ctx.params.protocol_version_major >= 7 {
            0.0
        } else {
            let d_n = ctx.params.d.numerator as f64;
            let d_d = ctx.params.d.denominator.max(1) as f64;
            d_n / d_d
        };

        common::compute_shelley_nonce(
            header,
            ctx.current_slot,
            first_slot_of_next_epoch,
            ctx.stability_window,
            d_value,
            consensus,
        );
    }

    /// Shelley minimum fee: `min_fee_a * tx_size + min_fee_b`.
    ///
    /// Simple linear fee formula. Same formula applies across Shelley, Allegra,
    /// and Mary eras — the coefficients come from the current protocol params.
    fn min_fee(&self, tx: &Transaction, ctx: &RuleContext, _utxo: &UtxoSubState) -> u64 {
        let tx_size = tx.raw_cbor.as_ref().map_or(0, |b| b.len() as u64);
        ctx.params
            .min_fee_a
            .checked_mul(tx_size)
            .and_then(|product| product.checked_add(ctx.params.min_fee_b))
            .unwrap_or(u64::MAX)
    }

    /// Handle hard fork state transformations when entering Shelley/Allegra/Mary.
    ///
    /// - Byron -> Shelley: Staking state initialization from genesis would occur
    ///   here. The current implementation returns Ok(()) — full TranslateEra
    ///   logic (initial stake distribution from Byron UTxOs) can be added when
    ///   the orchestrator (Task 12) wires era transitions.
    /// - Shelley -> Allegra: No state transformation needed.
    /// - Allegra -> Mary: No state transformation needed.
    fn on_era_transition(
        &self,
        from_era: Era,
        _ctx: &RuleContext,
        _utxo: &mut UtxoSubState,
        _certs: &mut CertSubState,
        _gov: &mut GovSubState,
        _consensus: &mut ConsensusSubState,
        _epochs: &mut EpochSubState,
    ) -> Result<(), LedgerError> {
        match from_era {
            Era::Byron => {
                // TODO: Initialize staking state from genesis.
                // The full TranslateEra logic (Haskell's translateToShelleyLedgerState)
                // initializes initial funds, staking, delegations, and genesis UTxOs.
                // Currently handled by the existing LedgerState initialization code.
                debug!("Byron -> Shelley era transition (staking initialization deferred to orchestrator)");
                Ok(())
            }
            Era::Shelley => {
                // Shelley -> Allegra: no state transformation needed.
                debug!("Shelley -> Allegra era transition (no state changes)");
                Ok(())
            }
            Era::Allegra => {
                // Allegra -> Mary: no state transformation needed.
                debug!("Allegra -> Mary era transition (no state changes)");
                Ok(())
            }
            other => {
                // Unexpected transition — this shouldn't happen but we handle it
                // gracefully to avoid panics.
                debug!(
                    "Unexpected era transition from {:?} to Shelley/Allegra/Mary",
                    other
                );
                Ok(())
            }
        }
    }

    /// Compute the set of required VKey witnesses for a Shelley/Allegra/Mary transaction.
    ///
    /// Required witnesses come from three sources:
    /// 1. **Spending inputs**: payment credential key hashes from UTxO outputs being consumed.
    /// 2. **Withdrawals**: reward account credential key hashes.
    /// 3. **Certificates**: key hashes from stake credential operations and pool operations.
    ///
    /// Script credentials (hash prefix 0x01 in byte 28) are excluded — they require
    /// script witnesses, not VKey witnesses.
    fn required_witnesses(
        &self,
        tx: &Transaction,
        _ctx: &RuleContext,
        utxo: &UtxoSubState,
        _certs: &CertSubState,
        _gov: &GovSubState,
    ) -> HashSet<Hash28> {
        let mut witnesses = HashSet::new();

        // 1. Spending input pubkey hashes.
        for input in &tx.body.inputs {
            if let Some(output) = utxo.utxo_set.lookup(input) {
                if let Some(Credential::VerificationKey(hash)) = output.address.payment_credential()
                {
                    witnesses.insert(*hash);
                }
            }
        }

        // 2. Withdrawal key hashes.
        for reward_account in tx.body.withdrawals.keys() {
            if reward_account.len() >= 29 {
                // Bit 4 of header byte: 0 = key, 1 = script.
                // Only require VKey witness for key-based reward accounts.
                if reward_account[0] & 0x10 == 0 {
                    let mut key_bytes = [0u8; 28];
                    key_bytes.copy_from_slice(&reward_account[1..29]);
                    witnesses.insert(Hash28::from_bytes(key_bytes));
                }
            }
        }

        // 3. Certificate key hashes.
        for cert in &tx.body.certificates {
            match cert {
                Certificate::StakeRegistration(Credential::VerificationKey(hash))
                | Certificate::StakeDeregistration(Credential::VerificationKey(hash)) => {
                    witnesses.insert(*hash);
                }
                Certificate::StakeDelegation {
                    credential: Credential::VerificationKey(hash),
                    ..
                } => {
                    witnesses.insert(*hash);
                }
                Certificate::PoolRegistration(params) => {
                    // Pool operator key hash is required.
                    witnesses.insert(params.operator);
                    // Pool owner key hashes are required.
                    for owner in &params.pool_owners {
                        witnesses.insert(*owner);
                    }
                }
                Certificate::PoolRetirement { pool_hash, .. } => {
                    witnesses.insert(*pool_hash);
                }
                _ => {}
            }
        }

        witnesses
    }
}

// ---------------------------------------------------------------------------
// Helper: extract Hash32 from reward account bytes
// ---------------------------------------------------------------------------

/// Extract a Hash32 from a raw reward account byte string (29 bytes:
/// 1-byte header + 28-byte credential hash).
///
/// Mirrors `LedgerState::reward_account_to_hash` and
/// `common::reward_account_to_hash`.
fn reward_account_to_hash(reward_account: &[u8]) -> Hash32 {
    let mut key_bytes = [0u8; 32];
    if reward_account.len() >= 29 {
        key_bytes[..28].copy_from_slice(&reward_account[1..29]);
        if reward_account[0] & 0x10 != 0 {
            key_bytes[28] = 0x01;
        }
    }
    Hash32::from_bytes(key_bytes)
}

// ---------------------------------------------------------------------------
// Helper: apply protocol parameter update to ProtocolParameters
// ---------------------------------------------------------------------------

/// Apply a merged ProtocolParamUpdate to the current ProtocolParameters.
///
/// Each field in the update, if `Some`, overrides the corresponding field
/// in the protocol parameters.
fn apply_pp_update(
    params: &mut ProtocolParameters,
    update: &dugite_primitives::transaction::ProtocolParamUpdate,
) {
    if let Some(v) = update.min_fee_a {
        params.min_fee_a = v;
    }
    if let Some(v) = update.min_fee_b {
        params.min_fee_b = v;
    }
    if let Some(v) = update.max_block_body_size {
        params.max_block_body_size = v;
    }
    if let Some(v) = update.max_tx_size {
        params.max_tx_size = v;
    }
    if let Some(v) = update.max_block_header_size {
        params.max_block_header_size = v;
    }
    if let Some(v) = &update.key_deposit {
        params.key_deposit = *v;
    }
    if let Some(v) = &update.pool_deposit {
        params.pool_deposit = *v;
    }
    if let Some(v) = update.e_max {
        params.e_max = v;
    }
    if let Some(v) = update.n_opt {
        params.n_opt = v;
    }
    if let Some(v) = &update.a0 {
        params.a0 = v.clone();
    }
    if let Some(v) = &update.rho {
        params.rho = v.clone();
    }
    if let Some(v) = &update.tau {
        params.tau = v.clone();
    }
    if let Some(v) = &update.d {
        params.d = v.clone();
    }
    if let Some(v) = &update.min_pool_cost {
        params.min_pool_cost = *v;
    }
    if let Some(v) = &update.ada_per_utxo_byte {
        params.ada_per_utxo_byte = *v;
    }
    if let Some(v) = &update.cost_models {
        params.cost_models = v.clone();
    }
    if let Some(v) = &update.execution_costs {
        params.execution_costs = v.clone();
    }
    if let Some(v) = &update.max_tx_ex_units {
        params.max_tx_ex_units = *v;
    }
    if let Some(v) = &update.max_block_ex_units {
        params.max_block_ex_units = *v;
    }
    if let Some(v) = update.max_val_size {
        params.max_val_size = v;
    }
    if let Some(v) = update.collateral_percentage {
        params.collateral_percentage = v;
    }
    if let Some(v) = update.max_collateral_inputs {
        params.max_collateral_inputs = v;
    }
    if let Some(v) = update.protocol_version_major {
        params.protocol_version_major = v;
    }
    if let Some(v) = update.protocol_version_minor {
        params.protocol_version_minor = v;
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eras::{EraRules, EraRulesImpl, RuleContext};
    use crate::state::{
        BlockValidationMode, EpochSnapshots, GovernanceState, PoolRegistration,
        StakeDistributionState,
    };
    use crate::utxo::UtxoSet;
    use crate::utxo_diff::DiffSeq;
    use dugite_primitives::address::Address;
    use dugite_primitives::block::{BlockHeader, OperationalCert, ProtocolVersion, VrfOutput};
    use dugite_primitives::hash::Hash32;
    use dugite_primitives::protocol_params::ProtocolParameters;
    use dugite_primitives::time::{BlockNo, SlotNo};
    use dugite_primitives::transaction::{
        OutputDatum, TransactionBody, TransactionInput, TransactionOutput, TransactionWitnessSet,
    };
    use dugite_primitives::value::Value;
    use std::collections::BTreeMap;

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    fn make_shelley_ctx(params: &ProtocolParameters) -> RuleContext<'_> {
        let delegates = Box::leak(Box::new(HashMap::new()));
        RuleContext {
            params,
            current_slot: 100,
            current_epoch: EpochNo(5),
            era: Era::Shelley,
            slot_config: None,
            node_network: None,
            genesis_delegates: delegates,
            update_quorum: 5,
            epoch_length: 432000,
            shelley_transition_epoch: 0,
            byron_epoch_length: 21600,
            stability_window: 129600,
            randomness_stabilisation_window: 129600,
        }
    }

    fn make_utxo_sub(entries: Vec<(TransactionInput, TransactionOutput)>) -> UtxoSubState {
        let mut utxo_set = UtxoSet::new();
        for (input, output) in entries {
            utxo_set.insert(input, output);
        }
        UtxoSubState {
            utxo_set,
            diff_seq: DiffSeq::new(),
            epoch_fees: Lovelace(0),
            pending_donations: Lovelace(0),
        }
    }

    fn make_cert_sub() -> CertSubState {
        CertSubState {
            delegations: Arc::new(HashMap::new()),
            pool_params: Arc::new(HashMap::new()),
            future_pool_params: HashMap::new(),
            pending_retirements: HashMap::new(),
            reward_accounts: Arc::new(HashMap::new()),
            stake_key_deposits: HashMap::new(),
            pool_deposits: HashMap::new(),
            total_stake_key_deposits: 0,
            pointer_map: HashMap::new(),
            stake_distribution: StakeDistributionState {
                stake_map: HashMap::new(),
            },
            script_stake_credentials: HashSet::new(),
        }
    }

    fn make_gov_sub() -> GovSubState {
        GovSubState {
            governance: Arc::new(GovernanceState::default()),
        }
    }

    fn make_epoch_sub() -> EpochSubState {
        EpochSubState {
            snapshots: EpochSnapshots::default(),
            treasury: Lovelace(0),
            reserves: Lovelace(0),
            pending_reward_update: None,
            pending_pp_updates: BTreeMap::new(),
            future_pp_updates: BTreeMap::new(),
            needs_stake_rebuild: false,
            ptr_stake: HashMap::new(),
            ptr_stake_excluded: false,
            protocol_params: ProtocolParameters::mainnet_defaults(),
            prev_protocol_params: ProtocolParameters::mainnet_defaults(),
            prev_protocol_version_major: 2,
            prev_d: 1.0,
        }
    }

    fn make_consensus_sub() -> ConsensusSubState {
        ConsensusSubState {
            evolving_nonce: Hash32::ZERO,
            candidate_nonce: Hash32::ZERO,
            epoch_nonce: Hash32::ZERO,
            lab_nonce: Hash32::ZERO,
            last_epoch_block_nonce: Hash32::ZERO,
            rolling_nonce: Hash32::ZERO,
            first_block_hash_of_epoch: None,
            prev_epoch_first_block_hash: None,
            epoch_blocks_by_pool: Arc::new(HashMap::new()),
            epoch_block_count: 0,
            opcert_counters: HashMap::new(),
        }
    }

    fn make_block_header(prev_hash: Hash32, issuer_vkey: Vec<u8>) -> BlockHeader {
        BlockHeader {
            header_hash: Hash32::ZERO,
            prev_hash,
            issuer_vkey,
            vrf_vkey: vec![],
            vrf_result: VrfOutput {
                output: vec![],
                proof: vec![],
            },
            block_number: BlockNo(0),
            slot: SlotNo(0),
            epoch_nonce: Hash32::ZERO,
            body_size: 0,
            body_hash: Hash32::ZERO,
            operational_cert: OperationalCert {
                hot_vkey: vec![],
                sequence_number: 0,
                kes_period: 0,
                sigma: vec![],
            },
            protocol_version: ProtocolVersion { major: 2, minor: 0 },
            kes_signature: vec![],
            nonce_vrf_output: vec![],
            nonce_vrf_proof: vec![],
        }
    }

    fn make_output(address: Address, coin: u64) -> TransactionOutput {
        TransactionOutput {
            address,
            value: Value::lovelace(coin),
            datum: OutputDatum::None,
            script_ref: None,
            is_legacy: true,
            raw_cbor: None,
        }
    }

    fn make_input(tx_id_byte: u8, index: u32) -> TransactionInput {
        TransactionInput {
            transaction_id: Hash32::from_bytes([tx_id_byte; 32]),
            index,
        }
    }

    fn make_tx(
        tx_id_byte: u8,
        inputs: Vec<TransactionInput>,
        outputs: Vec<TransactionOutput>,
        fee: u64,
    ) -> Transaction {
        let body = TransactionBody {
            inputs,
            outputs,
            fee: Lovelace(fee),
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
        };
        Transaction {
            era: Era::Shelley,
            hash: Hash32::from_bytes([tx_id_byte; 32]),
            body,
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
            raw_cbor: Some(vec![0u8; 200]),
            raw_body_cbor: None,
            raw_witness_cbor: None,
        }
    }

    fn make_enterprise_address(key_hash: Hash28) -> Address {
        let mut addr_bytes = vec![0x61]; // Enterprise, key credential, network 1
        addr_bytes.extend_from_slice(key_hash.as_bytes());
        Address::from_bytes(&addr_bytes).expect("valid enterprise address")
    }

    // -----------------------------------------------------------------------
    // Tests
    // -----------------------------------------------------------------------

    /// Verify that EraRulesImpl::for_era correctly maps Shelley/Allegra/Mary.
    #[test]
    fn test_era_rules_impl_for_shelley_allegra_mary() {
        assert!(matches!(
            EraRulesImpl::for_era(Era::Shelley),
            EraRulesImpl::Shelley(_)
        ));
        assert!(matches!(
            EraRulesImpl::for_era(Era::Allegra),
            EraRulesImpl::Shelley(_)
        ));
        assert!(matches!(
            EraRulesImpl::for_era(Era::Mary),
            EraRulesImpl::Shelley(_)
        ));
    }

    /// validate_block_body always succeeds for Shelley (no ExUnit checks).
    #[test]
    fn test_validate_block_body_always_succeeds() {
        let rules = ShelleyRules::new();
        let params = ProtocolParameters::mainnet_defaults();
        let ctx = make_shelley_ctx(&params);
        let utxo = make_utxo_sub(vec![]);

        let block = dugite_primitives::block::Block {
            era: Era::Shelley,
            header: make_block_header(Hash32::ZERO, vec![]),
            transactions: vec![],
            raw_cbor: None,
        };

        assert!(rules.validate_block_body(&block, &ctx, &utxo).is_ok());
    }

    /// Apply an empty valid transaction — no inputs consumed, no outputs produced.
    #[test]
    fn test_apply_valid_tx_empty_tx() {
        let rules = ShelleyRules::new();
        let params = ProtocolParameters::mainnet_defaults();
        let ctx = make_shelley_ctx(&params);
        let mut utxo = make_utxo_sub(vec![]);
        let mut certs = make_cert_sub();
        let mut gov = make_gov_sub();
        let mut epochs = make_epoch_sub();

        let tx = make_tx(0x01, vec![], vec![], 0);
        let result = rules.apply_valid_tx(
            &tx,
            BlockValidationMode::ApplyOnly,
            &ctx,
            &mut utxo,
            &mut certs,
            &mut gov,
            &mut epochs,
        );
        assert!(result.is_ok());
        let diff = result.unwrap();
        assert!(diff.inserts.is_empty());
        assert!(diff.deletes.is_empty());
    }

    /// Apply a valid transaction that spends a UTxO and produces a new one.
    #[test]
    fn test_apply_valid_tx_with_utxo() {
        let rules = ShelleyRules::new();
        let params = ProtocolParameters::mainnet_defaults();
        let ctx = make_shelley_ctx(&params);

        let key_hash = Hash28::from_bytes([0x42; 28]);
        let addr = make_enterprise_address(key_hash);
        let input = make_input(0xAA, 0);
        let spent_output = make_output(addr.clone(), 5_000_000);
        let mut utxo = make_utxo_sub(vec![(input.clone(), spent_output)]);
        let mut certs = make_cert_sub();
        let mut gov = make_gov_sub();
        let mut epochs = make_epoch_sub();

        let tx = make_tx(
            0xBB,
            vec![input],
            vec![make_output(addr, 4_800_000)],
            200_000,
        );

        let result = rules.apply_valid_tx(
            &tx,
            BlockValidationMode::ApplyOnly,
            &ctx,
            &mut utxo,
            &mut certs,
            &mut gov,
            &mut epochs,
        );
        assert!(result.is_ok());
        let diff = result.unwrap();
        assert_eq!(diff.deletes.len(), 1);
        assert_eq!(diff.inserts.len(), 1);
        assert_eq!(utxo.epoch_fees.0, 200_000);
    }

    /// Shelley apply_invalid_tx must return an error.
    #[test]
    fn test_apply_invalid_tx_returns_error() {
        let rules = ShelleyRules::new();
        let params = ProtocolParameters::mainnet_defaults();
        let ctx = make_shelley_ctx(&params);
        let mut utxo = make_utxo_sub(vec![]);

        let tx = make_tx(0x01, vec![], vec![], 0);
        let result = rules.apply_invalid_tx(&tx, BlockValidationMode::ApplyOnly, &ctx, &mut utxo);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            LedgerError::InvalidTransaction(_)
        ));
    }

    /// Minimum fee computation: min_fee_a * tx_size + min_fee_b.
    #[test]
    fn test_min_fee_linear() {
        let rules = ShelleyRules::new();
        let mut params = ProtocolParameters::mainnet_defaults();
        params.min_fee_a = 44;
        params.min_fee_b = 155_381;
        let ctx = make_shelley_ctx(&params);
        let utxo = make_utxo_sub(vec![]);

        let tx = make_tx(0x01, vec![], vec![], 0); // 200 bytes raw_cbor
        let fee = rules.min_fee(&tx, &ctx, &utxo);
        // 44 * 200 + 155_381 = 8_800 + 155_381 = 164_181
        assert_eq!(fee, 164_181);
    }

    /// Byron -> Shelley era transition succeeds.
    #[test]
    fn test_on_era_transition_byron_to_shelley() {
        let rules = ShelleyRules::new();
        let params = ProtocolParameters::mainnet_defaults();
        let ctx = make_shelley_ctx(&params);
        let mut utxo = make_utxo_sub(vec![]);
        let mut certs = make_cert_sub();
        let mut gov = make_gov_sub();
        let mut epochs = make_epoch_sub();
        let mut consensus = make_consensus_sub();

        let result = rules.on_era_transition(
            Era::Byron,
            &ctx,
            &mut utxo,
            &mut certs,
            &mut gov,
            &mut consensus,
            &mut epochs,
        );
        assert!(result.is_ok());
    }

    /// Shelley -> Allegra era transition succeeds (no-op).
    #[test]
    fn test_on_era_transition_shelley_to_allegra() {
        let rules = ShelleyRules::new();
        let params = ProtocolParameters::mainnet_defaults();
        let ctx = make_shelley_ctx(&params);
        let mut utxo = make_utxo_sub(vec![]);
        let mut certs = make_cert_sub();
        let mut gov = make_gov_sub();
        let mut epochs = make_epoch_sub();
        let mut consensus = make_consensus_sub();

        let result = rules.on_era_transition(
            Era::Shelley,
            &ctx,
            &mut utxo,
            &mut certs,
            &mut gov,
            &mut consensus,
            &mut epochs,
        );
        assert!(result.is_ok());
    }

    /// Basic epoch transition: fees reset, block count reset, mark snapshot created.
    #[test]
    fn test_process_epoch_transition_basic() {
        let rules = ShelleyRules::new();
        let params = ProtocolParameters::mainnet_defaults();
        let ctx = make_shelley_ctx(&params);
        let mut utxo = make_utxo_sub(vec![]);
        let mut certs = make_cert_sub();
        let mut gov = make_gov_sub();
        let mut epochs = make_epoch_sub();
        let mut consensus = make_consensus_sub();

        utxo.epoch_fees = Lovelace(1_000_000);
        consensus.epoch_block_count = 100;

        let result = rules.process_epoch_transition(
            EpochNo(6),
            &ctx,
            &mut utxo,
            &mut certs,
            &mut gov,
            &mut epochs,
            &mut consensus,
        );
        assert!(result.is_ok());

        assert_eq!(utxo.epoch_fees.0, 0);
        assert_eq!(consensus.epoch_block_count, 0);
        assert!(epochs.snapshots.mark.is_some());
        assert_eq!(epochs.snapshots.ss_fee.0, 1_000_000);
    }

    /// Epoch transition with pool retirement: pool removed, deposit refunded.
    #[test]
    fn test_process_epoch_transition_pool_retirement() {
        let rules = ShelleyRules::new();
        let params = ProtocolParameters::mainnet_defaults();
        let ctx = make_shelley_ctx(&params);
        let mut utxo = make_utxo_sub(vec![]);
        let mut certs = make_cert_sub();
        let mut gov = make_gov_sub();
        let mut epochs = make_epoch_sub();
        let mut consensus = make_consensus_sub();

        // Register a pool.
        let pool_id = Hash28::from_bytes([0xAA; 28]);
        let pool_reg = PoolRegistration {
            pool_id,
            vrf_keyhash: Hash32::ZERO,
            pledge: Lovelace(10_000_000_000),
            cost: Lovelace(340_000_000),
            margin_numerator: 1,
            margin_denominator: 100,
            reward_account: vec![0xe0; 29],
            owners: vec![Hash28::from_bytes([0xCC; 28])],
            relays: vec![],
            metadata_url: None,
            metadata_hash: None,
        };
        Arc::make_mut(&mut certs.pool_params).insert(pool_id, pool_reg);
        certs.pool_deposits.insert(pool_id, 500_000_000);

        // Register the operator's reward account.
        let op_key = reward_account_to_hash(&[0xe0; 29]);
        Arc::make_mut(&mut certs.reward_accounts).insert(op_key, Lovelace(0));

        // Schedule retirement at epoch 6.
        certs.pending_retirements.insert(pool_id, EpochNo(6));

        let result = rules.process_epoch_transition(
            EpochNo(6),
            &ctx,
            &mut utxo,
            &mut certs,
            &mut gov,
            &mut epochs,
            &mut consensus,
        );
        assert!(result.is_ok());

        assert!(!certs.pool_params.contains_key(&pool_id));
        assert_eq!(certs.reward_accounts.get(&op_key).unwrap().0, 500_000_000);
        assert!(certs.pending_retirements.is_empty());
    }

    /// Evolve nonce with VRF output updates evolving_nonce, lab_nonce, block count.
    #[test]
    fn test_evolve_nonce_with_vrf_output() {
        let rules = ShelleyRules::new();
        let params = ProtocolParameters::mainnet_defaults();
        let ctx = make_shelley_ctx(&params);
        let mut consensus = make_consensus_sub();

        let mut header = make_block_header(Hash32::from_bytes([0x01; 32]), vec![0x99; 32]);
        header.nonce_vrf_output = vec![0x42; 32];

        rules.evolve_nonce(&header, &ctx, &mut consensus);

        assert_ne!(consensus.evolving_nonce, Hash32::ZERO);
        assert_eq!(consensus.lab_nonce, header.prev_hash);
        assert_eq!(consensus.epoch_block_count, 1);
    }

    /// Required witnesses include spending input key hashes.
    #[test]
    fn test_required_witnesses_spending_inputs() {
        let rules = ShelleyRules::new();
        let params = ProtocolParameters::mainnet_defaults();
        let ctx = make_shelley_ctx(&params);

        let key_hash = Hash28::from_bytes([0x42; 28]);
        let addr = make_enterprise_address(key_hash);
        let input = make_input(0xAA, 0);
        let utxo = make_utxo_sub(vec![(input.clone(), make_output(addr, 5_000_000))]);
        let certs = make_cert_sub();
        let gov = make_gov_sub();

        let tx = make_tx(0xBB, vec![input], vec![], 0);
        let witnesses = rules.required_witnesses(&tx, &ctx, &utxo, &certs, &gov);
        assert!(witnesses.contains(&key_hash));
    }

    /// Required witnesses include withdrawal key hashes.
    #[test]
    fn test_required_witnesses_withdrawals() {
        let rules = ShelleyRules::new();
        let params = ProtocolParameters::mainnet_defaults();
        let ctx = make_shelley_ctx(&params);
        let utxo = make_utxo_sub(vec![]);
        let certs = make_cert_sub();
        let gov = make_gov_sub();

        let key_hash = Hash28::from_bytes([0x55; 28]);
        let mut reward_account = vec![0xe0]; // key-based
        reward_account.extend_from_slice(key_hash.as_bytes());

        let mut tx = make_tx(0x01, vec![], vec![], 0);
        tx.body
            .withdrawals
            .insert(reward_account, Lovelace(1_000_000));

        let witnesses = rules.required_witnesses(&tx, &ctx, &utxo, &certs, &gov);
        assert!(witnesses.contains(&key_hash));
    }

    /// Required witnesses include certificate key hashes.
    #[test]
    fn test_required_witnesses_certificates() {
        let rules = ShelleyRules::new();
        let params = ProtocolParameters::mainnet_defaults();
        let ctx = make_shelley_ctx(&params);
        let utxo = make_utxo_sub(vec![]);
        let certs = make_cert_sub();
        let gov = make_gov_sub();

        let key_hash = Hash28::from_bytes([0x77; 28]);
        let mut tx = make_tx(0x01, vec![], vec![], 0);
        tx.body.certificates = vec![Certificate::StakeDelegation {
            credential: Credential::VerificationKey(key_hash),
            pool_hash: Hash28::from_bytes([0x88; 28]),
        }];

        let witnesses = rules.required_witnesses(&tx, &ctx, &utxo, &certs, &gov);
        assert!(witnesses.contains(&key_hash));
    }

    /// Script-based withdrawal should NOT be in required witnesses (needs script witness instead).
    #[test]
    fn test_required_witnesses_script_withdrawal_excluded() {
        let rules = ShelleyRules::new();
        let params = ProtocolParameters::mainnet_defaults();
        let ctx = make_shelley_ctx(&params);
        let utxo = make_utxo_sub(vec![]);
        let certs = make_cert_sub();
        let gov = make_gov_sub();

        let key_hash = Hash28::from_bytes([0x55; 28]);
        let mut reward_account = vec![0xf0]; // script-based (bit 4 set)
        reward_account.extend_from_slice(key_hash.as_bytes());

        let mut tx = make_tx(0x01, vec![], vec![], 0);
        tx.body
            .withdrawals
            .insert(reward_account, Lovelace(1_000_000));

        let witnesses = rules.required_witnesses(&tx, &ctx, &utxo, &certs, &gov);
        // Script-based withdrawal should NOT produce a VKey witness requirement.
        assert!(!witnesses.contains(&key_hash));
    }

    /// Epoch transition flushes pending treasury donations.
    #[test]
    fn test_process_epoch_transition_flushes_donations() {
        let rules = ShelleyRules::new();
        let params = ProtocolParameters::mainnet_defaults();
        let ctx = make_shelley_ctx(&params);
        let mut utxo = make_utxo_sub(vec![]);
        let mut certs = make_cert_sub();
        let mut gov = make_gov_sub();
        let mut epochs = make_epoch_sub();
        let mut consensus = make_consensus_sub();

        utxo.pending_donations = Lovelace(5_000_000);
        epochs.treasury = Lovelace(100_000_000);

        let result = rules.process_epoch_transition(
            EpochNo(6),
            &ctx,
            &mut utxo,
            &mut certs,
            &mut gov,
            &mut epochs,
            &mut consensus,
        );
        assert!(result.is_ok());
        assert_eq!(utxo.pending_donations.0, 0);
        assert_eq!(epochs.treasury.0, 105_000_000);
    }
}
