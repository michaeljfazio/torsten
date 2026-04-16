/// Conway era ledger rules (protocol version 9+).
///
/// Conway (CIP-1694) introduces on-chain governance:
/// - DRep (Delegated Representatives) registration, delegation, and voting
/// - Constitutional Committee hot key authorization and resignation
/// - Governance actions (proposals) and voting by DReps, SPOs, and CC members
/// - Treasury withdrawals via governance actions
/// - Protocol parameter updates via governance (replaces pre-Conway PPUP)
/// - Tiered reference script fees (25KiB tiers, 1.2x multiplier)
/// - Plutus V3
///
/// The Conway LEDGER rule pipeline has 9 steps (compared to Babbage's 3):
/// 1. validateTreasuryValue
/// 2. validateRefScriptSize
/// 3. validateWithdrawalsDelegated (PV >= 10)
/// 4. testIncompleteAndMissingWithdrawals (PV >= 10)
/// 5. updateDormantDRepExpiries / updateVotingDRepExpiries
/// 6. drainAccounts (same as Shelley)
/// 7. Apply CERTS rule (Shelley certs + Conway governance certs)
/// 8. Apply GOV rule (votes + proposals)
/// 9. Apply UTXOW/UTXO/UTXOS rule (consume inputs, produce outputs)
///
/// The Conway epoch transition has 13 steps (compared to Shelley's ~8):
/// 1. SNAP (snapshot rotation)
/// 2. POOLREAP (pool retirements with deposit refunds)
/// 3. DRep pulser completion
/// 4. Treasury withdrawals (enact approved withdrawals)
/// 5. proposalsApplyEnactment (ratify & enact governance actions)
/// 6. Return deposits from expired/enacted proposals
/// 7. Update GovState (advance proposal epochs, remove enacted/expired)
/// 8. numDormantEpochs computation
/// 9. Prune expired committee members
/// 10. Flush donations (pending_donations -> treasury)
/// 11. totalObligation recalculation
/// 12. HARDFORK check
/// 13. setFreshDRepPulsingState
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use dugite_primitives::block::{Block, BlockHeader};
use dugite_primitives::credentials::Credential;
use dugite_primitives::era::Era;
use dugite_primitives::hash::{blake2b_256, Hash28, Hash32};
use dugite_primitives::time::EpochNo;
use dugite_primitives::transaction::{Certificate, GovActionId, Transaction, Voter};
use dugite_primitives::value::Lovelace;
use tracing::debug;

use super::common;
use super::{EraRules, RuleContext};
use crate::state::governance::{
    forest_add_proposal, gov_action_purpose_tag, gov_action_raw_prev_id,
};
use crate::state::substates::*;
use crate::state::{
    BlockValidationMode, DRepRegistration, LedgerError, ProposalState, StakeSnapshot,
};
use crate::utxo_diff::UtxoDiff;

/// Stateless Conway era rule strategy.
///
/// Implements the Conway-specific LEDGER pipeline and epoch transition.
/// Delegates shared logic (drain withdrawals, UTxO changes, nonce evolution)
/// to common helpers, and adds Conway-specific steps for governance
/// certificates, proposals, votes, and the extended epoch transition.
#[derive(Default, Debug, Clone, Copy)]
pub struct ConwayRules;

impl ConwayRules {
    pub fn new() -> Self {
        ConwayRules
    }
}

impl EraRules for ConwayRules {
    /// Validate Conway block body constraints.
    ///
    /// Checks:
    /// 1. Total ExUnit budget (memory + steps) does not exceed block limits.
    /// 2. Total reference script size across all transactions does not exceed
    ///    1 MiB (Conway `ppMaxRefScriptSizePerBlockG`).
    fn validate_block_body(
        &self,
        block: &Block,
        ctx: &RuleContext,
        utxo: &UtxoSubState,
    ) -> Result<(), LedgerError> {
        // Step 1: ExUnit budget check (shared with Alonzo/Babbage).
        common::validate_block_ex_units(block, ctx)?;

        // Step 2: Block-level reference script size check.
        // Build within-block UTxO overlay for ref script resolution (outputs
        // created earlier in the block may be referenced later).
        let mut block_utxo_overlay: std::collections::HashMap<
            dugite_primitives::transaction::TransactionInput,
            dugite_primitives::transaction::TransactionOutput,
        > = std::collections::HashMap::new();
        for tx in &block.transactions {
            if tx.is_valid {
                for (idx, output) in tx.body.outputs.iter().enumerate() {
                    block_utxo_overlay.insert(
                        dugite_primitives::transaction::TransactionInput {
                            transaction_id: tx.hash,
                            index: idx as u32,
                        },
                        output.clone(),
                    );
                }
            }
        }

        let lookup_with_overlay = |input: &dugite_primitives::transaction::TransactionInput| {
            block_utxo_overlay
                .get(input)
                .cloned()
                .or_else(|| utxo.utxo_set.lookup(input))
        };

        let total_ref_script_size: u64 = block
            .transactions
            .iter()
            .map(|tx| {
                let spending_size: u64 = tx
                    .body
                    .inputs
                    .iter()
                    .filter_map(|inp| {
                        lookup_with_overlay(inp).and_then(|utxo_out| {
                            utxo_out
                                .script_ref
                                .as_ref()
                                .map(crate::validation::script_ref_byte_size)
                        })
                    })
                    .sum();
                let reference_size: u64 = tx
                    .body
                    .reference_inputs
                    .iter()
                    .filter_map(|inp| {
                        lookup_with_overlay(inp).and_then(|utxo_out| {
                            utxo_out
                                .script_ref
                                .as_ref()
                                .map(crate::validation::script_ref_byte_size)
                        })
                    })
                    .sum();
                spending_size.saturating_add(reference_size)
            })
            .fold(0u64, |acc, x| acc.saturating_add(x));

        // Conway block body limit: 1 MiB (hardcoded, not governance-updateable).
        const MAX_REF_SCRIPT_SIZE_PER_BLOCK: u64 = 1024 * 1024;
        if total_ref_script_size > MAX_REF_SCRIPT_SIZE_PER_BLOCK {
            return Err(LedgerError::BlockTxValidationFailed {
                slot: ctx.current_slot,
                tx_hash: String::from("(block-level check)"),
                errors: format!(
                    "BodyRefScriptsSizeTooBig: totalRefScriptSize={} exceeds \
                     maxRefScriptSizePerBlock={} (Conway Bbody rule)",
                    total_ref_script_size, MAX_REF_SCRIPT_SIZE_PER_BLOCK
                ),
            });
        }

        Ok(())
    }

    /// Apply a single valid Conway transaction (IsValid=true).
    ///
    /// Implements the Conway 9-step LEDGER pipeline:
    ///
    /// 1. **validateTreasuryValue** -- if tx sets currentTreasuryValue, verify
    ///    it matches the actual treasury balance.
    /// 2. **validateRefScriptSize** -- total ref script size <= maxRefScriptSizePerTx.
    ///    (Stub: checked during tx validation, not during apply.)
    /// 3. **validateWithdrawalsDelegated** (PV >= 10) -- all withdrawal KeyHash
    ///    accounts must be DRep-delegated.
    ///    (Stub: requires PV10 which is not yet active.)
    /// 4. **testIncompleteAndMissingWithdrawals** (PV >= 10) -- withdrawals drain
    ///    accounts exactly.
    ///    (Stub: requires PV10 which is not yet active.)
    /// 5. **updateDormantDRepExpiries / updateVotingDRepExpiries** -- update DRep
    ///    last-active epoch for voting DReps in this transaction.
    /// 6. **drainAccounts** -- apply withdrawals to balances (same as Shelley).
    /// 7. **Apply CERTS rule** -- process both Shelley certs AND Conway governance
    ///    certs (DRep registration, DRep update, DRep deregistration, CC hot key
    ///    auth, CC resignation, combined delegation certs).
    /// 8. **Apply GOV rule** -- process governance votes and proposals.
    /// 9. **Apply UTXOW/UTXO/UTXOS rule** -- consume inputs, produce outputs.
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
        // Step 1: validateTreasuryValue.
        // If the transaction declares a treasury value, verify it matches.
        // During apply-only (sync from chain), we skip this check since the
        // block producer already validated it.
        if let Some(declared_treasury) = tx.body.treasury_value {
            if declared_treasury != epochs.treasury {
                debug!(
                    tx_hash = %tx.hash.to_hex(),
                    declared = declared_treasury.0,
                    actual = epochs.treasury.0,
                    "Conway: treasury value mismatch (apply-only, not fatal)"
                );
            }
        }

        // Step 2: validateRefScriptSize (stub).
        // Reference script size validation is performed during Phase-1 validation,
        // not during apply. The tiered fee check lives in validation/conway.rs.

        // Step 3: validateWithdrawalsDelegated (PV >= 10, stub).
        // TODO: When protocol version 10 is active, verify that all withdrawal
        // KeyHash accounts have a DRep delegation in gov.governance.vote_delegations.

        // Step 4: testIncompleteAndMissingWithdrawals (PV >= 10, stub).
        // TODO: When protocol version 10 is active, verify withdrawal amounts
        // exactly match reward balances.

        // Step 5: Update DRep activity for voting DReps in this transaction.
        update_drep_expiries_for_tx(tx, ctx.current_epoch, gov, epochs);

        // Step 6: Drain withdrawal accounts.
        common::drain_withdrawal_accounts(tx, certs);

        // Step 7: Process certificates (Shelley + Conway governance certs).
        //
        // Haskell processes certs in a single ordered pass per tx. Dugite
        // previously split this into two passes (Shelley then Conway) which
        // broke tx cert sequences that interleave the two cert families, for
        // example `[ConwayStakeDeregistration, ConwayStakeRegistration,
        // StakeDelegation]`: the Shelley pass inserted the delegation first,
        // then the Conway pass's DEREG wiped it. Now we walk certs in order
        // and dispatch each one to both handlers (non-matching cert variants
        // are no-ops), preserving Haskell's sequential semantics.
        for (cert_index, cert) in tx.body.certificates.iter().enumerate() {
            common::apply_shelley_cert(
                cert,
                cert_index,
                ctx.current_slot,
                ctx.tx_index,
                certs,
                epochs,
                gov,
            );
            apply_conway_cert(cert, ctx.current_epoch, certs, gov, epochs);
        }

        // Step 8: Apply GOV rule (votes + proposals).
        process_governance_votes_and_proposals(tx, ctx, gov, epochs);

        // Step 9: Apply UTxO changes and accumulate donation.
        let diff = common::apply_utxo_changes(tx, utxo, certs, epochs);

        // Conway-specific: accumulate treasury donations from this transaction.
        if let Some(donation) = tx.body.donation {
            utxo.pending_donations += donation;
        }

        Ok(diff)
    }

    /// Apply an invalid Conway transaction (IsValid=false, collateral consumption).
    ///
    /// Same as Babbage: collateral inputs are consumed, collateral_return creates
    /// a new UTxO if present, and the fee is total_collateral or computed from
    /// the difference.
    fn apply_invalid_tx(
        &self,
        tx: &Transaction,
        _mode: BlockValidationMode,
        _ctx: &RuleContext,
        utxo: &mut UtxoSubState,
        certs: &mut CertSubState,
        epochs: &mut EpochSubState,
    ) -> Result<UtxoDiff, LedgerError> {
        let diff = common::apply_collateral_consumption(tx, utxo, certs, epochs);
        Ok(diff)
    }

    /// Process a Conway epoch boundary transition.
    ///
    /// Implements the Conway 13-step epoch transition pipeline. Steps that
    /// share logic with Shelley/Babbage delegate to the same code. Steps
    /// specific to Conway governance are implemented where possible and
    /// stubbed with TODO comments where the full logic is too complex to
    /// extract (governance ratification/enactment).
    ///
    /// The full governance ratification/enactment pipeline (~600 lines in
    /// `state/governance.rs`) will continue to be used by the old apply_block
    /// path until Task 12 migrates it.
    fn process_epoch_transition(
        &self,
        new_epoch: EpochNo,
        _ctx: &RuleContext,
        utxo: &mut UtxoSubState,
        certs: &mut CertSubState,
        gov: &mut GovSubState,
        epochs: &mut EpochSubState,
        consensus: &mut ConsensusSubState,
    ) -> Result<(), LedgerError> {
        debug!("Conway epoch transition: -> {}", new_epoch.0);

        // Capture bprev BEFORE any param updates (nesBprev = nesBcur).
        let bprev_block_count = consensus.epoch_block_count;
        let bprev_blocks_by_pool = Arc::clone(&consensus.epoch_blocks_by_pool);

        // === Step 1: SNAP (snapshot rotation) ===
        // Flush pending treasury donations BEFORE snapshot.
        if utxo.pending_donations.0 > 0 {
            let flushed = utxo.pending_donations;
            epochs.treasury.0 = epochs.treasury.0.saturating_add(flushed.0);
            utxo.pending_donations = Lovelace(0);
            debug!(
                epoch = new_epoch.0,
                donations_lovelace = flushed.0,
                "Conway: flushed pending treasury donations"
            );
        }

        // Apply pending reward update (backward compat for old snapshots).
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

        // Rotate snapshots: go <- set <- mark, capture fees.
        let captured_fees = utxo.epoch_fees;
        epochs.snapshots.go = epochs.snapshots.set.take();
        epochs.snapshots.set = epochs.snapshots.mark.take();
        epochs.snapshots.ss_fee = captured_fees;
        epochs.snapshots.bprev_block_count = bprev_block_count;
        epochs.snapshots.bprev_blocks_by_pool = bprev_blocks_by_pool;
        epochs.snapshots.rupd_ready = true;

        // Handle needs_stake_rebuild flag.
        if epochs.needs_stake_rebuild {
            epochs.needs_stake_rebuild = false;
            debug!(
                epoch = new_epoch.0,
                "Conway epoch: needs_stake_rebuild flag cleared (rebuild deferred to orchestrator)"
            );
        }

        // Build pool_stake from current stake distribution + delegations.
        // Conway excludes pointer-addressed UTxO stake (ptr_stake_excluded = true).
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

        // Conway does NOT resolve pointer-addressed UTxO stake (excluded by TranslateEra).

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
            }
        }

        // === Step 2: POOLREAP (pool retirements with deposit refunds) ===
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

        // === Step 3: DRep pulser completion ===
        // TODO: The DRep pulser calculates voting power from the mark snapshot's
        // vote delegations. This is complex (~200 lines in state/governance.rs)
        // and will be wired in Task 12.

        // === Step 4: Treasury withdrawals (enact approved withdrawals) ===
        // TODO: Enacted TreasuryWithdrawal governance actions distribute funds.
        // This requires the ratification pipeline from state/governance.rs.

        // === Step 5: proposalsApplyEnactment (ratify & enact governance actions) ===
        // TODO: Full governance ratification/enactment pipeline (~600 lines in
        // state/governance.rs). This will be wired when the orchestrator calls
        // the existing code in Task 12.

        // === Step 6: Return deposits from expired/enacted proposals ===
        // TODO: Return proposal deposits for enacted/expired governance actions
        // to their return addresses' reward accounts.

        // === Step 7: Update GovState (advance proposal epochs, remove enacted/expired) ===
        // TODO: Advance proposal expiry tracking and remove enacted/expired proposals.

        // === Step 8: numDormantEpochs computation ===
        // TODO: Track consecutive epochs with no governance activity for DRep
        // expiry extension.

        // === Step 9: Prune expired committee members ===
        {
            let governance = Arc::make_mut(&mut gov.governance);
            let expired_members: Vec<Hash32> = governance
                .committee_expiration
                .iter()
                .filter_map(|(cred, exp_epoch)| {
                    if *exp_epoch < new_epoch {
                        Some(*cred)
                    } else {
                        None
                    }
                })
                .collect();
            for cred in &expired_members {
                governance.committee_expiration.remove(cred);
                governance.committee_hot_keys.remove(cred);
                debug!(
                    epoch = new_epoch.0,
                    cred = %cred.to_hex(),
                    "Conway: pruned expired committee member"
                );
            }
        }

        // === Step 10: Flush donations (pending_donations -> treasury) ===
        // Already handled above in step 1 (before snapshot rotation), matching
        // Haskell's ordering where donations are flushed early.

        // === Step 11: totalObligation recalculation ===
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
                obl_stake, obl_pool, obl_drep, obl_proposal, "Conway: totalObligation recalculated"
            );
        }

        // === Step 12: HARDFORK check ===
        // TODO: If an enacted HardForkInitiation action's target version > current,
        // trigger a protocol version bump. This is handled at the consensus layer.

        // === Step 13: setFreshDRepPulsingState ===
        // TODO: Prepare the DRep pulser for the next epoch with fresh stake
        // distribution data. This will be wired in Task 12.

        // Capture prevPParams BEFORE any PP updates.
        let old_d = 0.0; // Conway: d is always 0 (fully decentralized).
        let old_proto_major = epochs.protocol_params.protocol_version_major;
        let old_params = epochs.protocol_params.clone();

        // Conway does NOT use pre-Conway PPUP proposals. Protocol parameter
        // changes are enacted through governance actions (ParameterChange).
        // The PP update from governance enactment would happen in Step 5 above.

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

        // Set prevPParams from values captured BEFORE governance enactment.
        epochs.prev_d = old_d;
        epochs.prev_protocol_version_major = old_proto_major;
        epochs.prev_protocol_params = old_params;

        // Reset per-epoch accumulators.
        utxo.epoch_fees = Lovelace(0);
        Arc::make_mut(&mut consensus.epoch_blocks_by_pool).clear();
        consensus.epoch_block_count = 0;

        Ok(())
    }

    /// Evolve nonce state after a Conway block header.
    ///
    /// Same VRF-based nonce evolution as Babbage. Conway (proto >= 9) always
    /// has d = 0 (fully decentralized).
    fn evolve_nonce(
        &self,
        header: &BlockHeader,
        ctx: &RuleContext,
        consensus: &mut ConsensusSubState,
    ) {
        let first_slot_of_next_epoch = ctx
            .current_epoch
            .0
            .saturating_add(1)
            .saturating_mul(ctx.epoch_length)
            .saturating_add(
                ctx.shelley_transition_epoch
                    .saturating_mul(ctx.byron_epoch_length),
            );

        // Conway (proto >= 9): d is always 0 (fully decentralized).
        let d_value = 0.0;

        common::compute_shelley_nonce(
            header,
            ctx.current_slot,
            first_slot_of_next_epoch,
            ctx.stability_window,
            d_value,
            consensus,
        );
    }

    /// Conway minimum fee: `min_fee_a * tx_size + min_fee_b`.
    ///
    /// Same linear fee formula as previous eras. Conway's tiered reference
    /// script fee (25KiB tiers, 1.2x multiplier) is an additional adjustment
    /// applied during transaction validation, not in this base min_fee method.
    fn min_fee(&self, tx: &Transaction, ctx: &RuleContext, _utxo: &UtxoSubState) -> u64 {
        let tx_size = tx.raw_cbor.as_ref().map_or(0, |b| b.len() as u64);
        ctx.params
            .min_fee_a
            .checked_mul(tx_size)
            .and_then(|product| product.checked_add(ctx.params.min_fee_b))
            .unwrap_or(u64::MAX)
    }

    /// Handle hard fork state transformations when entering Conway.
    ///
    /// Babbage -> Conway (TranslateEra). The Haskell spec lists 7 transformations:
    ///
    /// 1. Purge pointer-based stake from stake distribution (ptr_stake_excluded = true).
    /// 2. Create initial VState (DRep state) from ConwayGenesis.
    /// 3. Build VRF key hash -> pool ID map.
    /// 4. Create initial ConwayGovState (committee, constitution from genesis).
    /// 5. Reset utxosDonation to 0.
    /// 6. Recompute InstantStake (without pointer addresses).
    /// 7. Set initial DRep pulser state.
    ///
    /// All steps are implemented. Steps 2-4 seed governance state from
    /// ConwayGenesis when available. Steps 6-7 are handled implicitly by
    /// the incremental stake tracker and epoch boundary logic.
    fn on_era_transition(
        &self,
        from_era: Era,
        ctx: &RuleContext,
        utxo: &mut UtxoSubState,
        _certs: &mut CertSubState,
        gov: &mut GovSubState,
        _consensus: &mut ConsensusSubState,
        epochs: &mut EpochSubState,
    ) -> Result<(), LedgerError> {
        debug!(
            "{:?} -> Conway era transition: excluding pointer stake, resetting donations",
            from_era
        );

        // Step 1: Purge pointer-based stake from stake distribution.
        // Setting ptr_stake_excluded = true causes stake_routing() in common.rs
        // to return StakeRouting::None for pointer addresses, effectively
        // excluding pointer-addressed UTxO coins from the stake distribution
        // going forward.
        //
        // Also clear the ptr_stake map itself — matching Haskell's TranslateEra
        // which converts ShelleyInstantStake → ConwayInstantStake at the ERA
        // boundary, discarding `sisPtrStake` BEFORE the TICK/SNAP rules run.
        if !epochs.ptr_stake.is_empty() {
            let excluded_count = epochs.ptr_stake.len() as u64;
            let excluded_total: u64 = epochs.ptr_stake.values().sum();
            epochs.ptr_stake.clear();
            tracing::info!(
                excluded_count,
                excluded_total,
                excluded_ada = excluded_total / 1_000_000,
                "Conway: discarded pointer-addressed UTxO stake — \
                 matching TranslateEra ConwayInstantStake semantics"
            );
        }
        epochs.ptr_stake_excluded = true;

        // Step 5: Reset utxosDonation to 0.
        utxo.pending_donations = Lovelace(0);

        // Steps 2 & 4: Seed initial DRep state, committee, and constitution
        // from ConwayGenesis config (matches Haskell's TranslateEra VState +
        // ConwayGovState construction).
        if let Some(ref genesis) = ctx.conway_genesis {
            let governance = Arc::make_mut(&mut gov.governance);

            // Step 2: Seed initial DReps from genesis.
            for (hash28, deposit) in &genesis.initial_dreps {
                // Hash28 -> Hash32: pad with trailing zeros (matching 28-byte
                // credential convention — VerificationKey type, last 4 bytes 0).
                let cred_hash = Hash32::from_bytes({
                    let mut buf = [0u8; 32];
                    buf[..28].copy_from_slice(hash28.as_bytes());
                    buf
                });
                governance
                    .dreps
                    .entry(cred_hash)
                    .or_insert(DRepRegistration {
                        credential: Credential::VerificationKey(*hash28),
                        deposit: Lovelace(*deposit),
                        drep_expiry: EpochNo(
                            ctx.current_epoch.0 + ctx.params.drep_activity,
                        ),
                        anchor: None,
                        registered_epoch: ctx.current_epoch,
                        active: true,
                    });
            }

            // Step 4a: Seed committee members from genesis.
            for (cred_bytes, expiry) in &genesis.committee_members {
                let cred = Hash32::from_bytes(*cred_bytes);
                governance.committee_expiration.insert(cred, EpochNo(*expiry));
            }

            // Step 4b: Set committee threshold from genesis.
            if let Some((num, den)) = genesis.committee_threshold {
                governance.committee_threshold =
                    Some(dugite_primitives::transaction::Rational {
                        numerator: num,
                        denominator: den,
                    });
            }

            // Step 4c: Seed constitution from genesis.
            if let Some(ref constitution) = genesis.constitution {
                governance.constitution = Some(constitution.clone());
            }

            let drep_count = genesis.initial_dreps.len();
            let committee_count = genesis.committee_members.len();
            tracing::info!(
                drep_count,
                committee_count,
                has_constitution = genesis.constitution.is_some(),
                "Conway: seeded governance state from ConwayGenesis"
            );
        }

        // Step 3: VRF key hash -> pool ID map.
        // In Haskell this is built for the DRep pulser to identify which pool
        // produced a block. Dugite uses pool_id directly from block headers,
        // so no VRF-to-pool map is needed.

        // Step 6: Recompute InstantStake without pointer addresses.
        // With ptr_stake_excluded=true (Step 1), stake_routing() returns None for
        // pointer addresses. The incremental stake tracker won't add pointer stake
        // going forward. The next SNAP's mark snapshot will be built without pointer
        // stake. No explicit full-UTxO-walk recompute is needed.

        // Step 7: Initial DRep pulser state.
        // The DRep distribution snapshot will be captured at the first Conway epoch
        // boundary (process_epoch_transition Step 13). No pre-seeding needed —
        // ratify_proposals falls back to live state when no snapshot exists.

        Ok(())
    }

    /// Compute the set of required VKey witnesses for a Conway transaction.
    ///
    /// Conway adds witness requirements beyond Babbage:
    /// - DRep voter key hashes (for DRep votes in voting_procedures)
    /// - CC hot key hashes (for committee votes in voting_procedures)
    /// - Proposer key hashes (for governance proposals)
    /// - Conway governance cert key hashes (DRep reg/unreg, CC auth/resign)
    /// - Plus all Babbage witness requirements (spending inputs, withdrawals,
    ///   Shelley certs, required_signers)
    fn required_witnesses(
        &self,
        tx: &Transaction,
        _ctx: &RuleContext,
        utxo: &UtxoSubState,
        _certs: &CertSubState,
        _gov: &GovSubState,
    ) -> HashSet<Hash28> {
        let mut witnesses = HashSet::new();

        // 1. Spending input pubkey hashes (reference_inputs excluded).
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
            if reward_account.len() >= 29 && reward_account[0] & 0x10 == 0 {
                let mut key_bytes = [0u8; 28];
                key_bytes.copy_from_slice(&reward_account[1..29]);
                witnesses.insert(Hash28::from_bytes(key_bytes));
            }
        }

        // 3. Certificate key hashes (both Shelley and Conway certs).
        for cert in &tx.body.certificates {
            match cert {
                // Shelley certs
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
                    witnesses.insert(params.operator);
                    for owner in &params.pool_owners {
                        witnesses.insert(*owner);
                    }
                }
                Certificate::PoolRetirement { pool_hash, .. } => {
                    witnesses.insert(*pool_hash);
                }

                // Conway governance certs: DRep registration/update/deregistration
                Certificate::RegDRep {
                    credential: Credential::VerificationKey(hash),
                    ..
                }
                | Certificate::UnregDRep {
                    credential: Credential::VerificationKey(hash),
                    ..
                }
                | Certificate::UpdateDRep {
                    credential: Credential::VerificationKey(hash),
                    ..
                } => {
                    witnesses.insert(*hash);
                }

                // Conway governance certs: vote delegation
                Certificate::VoteDelegation {
                    credential: Credential::VerificationKey(hash),
                    ..
                } => {
                    witnesses.insert(*hash);
                }

                // Conway governance certs: combined delegation certs
                Certificate::StakeVoteDelegation {
                    credential: Credential::VerificationKey(hash),
                    ..
                }
                | Certificate::RegStakeDeleg {
                    credential: Credential::VerificationKey(hash),
                    ..
                }
                | Certificate::RegStakeVoteDeleg {
                    credential: Credential::VerificationKey(hash),
                    ..
                }
                | Certificate::VoteRegDeleg {
                    credential: Credential::VerificationKey(hash),
                    ..
                } => {
                    witnesses.insert(*hash);
                }

                // Conway governance certs: stake registration/deregistration with deposit
                Certificate::ConwayStakeRegistration {
                    credential: Credential::VerificationKey(hash),
                    ..
                }
                | Certificate::ConwayStakeDeregistration {
                    credential: Credential::VerificationKey(hash),
                    ..
                } => {
                    witnesses.insert(*hash);
                }

                // Conway governance certs: CC hot key auth (cold key signs)
                Certificate::CommitteeHotAuth {
                    cold_credential: Credential::VerificationKey(hash),
                    ..
                } => {
                    witnesses.insert(*hash);
                }

                // Conway governance certs: CC resignation (cold key signs)
                Certificate::CommitteeColdResign {
                    cold_credential: Credential::VerificationKey(hash),
                    ..
                } => {
                    witnesses.insert(*hash);
                }

                _ => {}
            }
        }

        // 4. Required signers (Alonzo+ feature).
        for signer in &tx.body.required_signers {
            let mut key_bytes = [0u8; 28];
            key_bytes.copy_from_slice(&signer.as_bytes()[..28]);
            witnesses.insert(Hash28::from_bytes(key_bytes));
        }

        // 5. Voter key hashes from voting_procedures (Conway-specific).
        for voter in tx.body.voting_procedures.keys() {
            match voter {
                Voter::DRep(Credential::VerificationKey(hash)) => {
                    witnesses.insert(*hash);
                }
                Voter::ConstitutionalCommittee(Credential::VerificationKey(hash)) => {
                    // CC votes use the HOT key to sign, but the voter field
                    // contains the hot credential. The hot key hash IS the
                    // witness requirement.
                    witnesses.insert(*hash);
                }
                Voter::StakePool(pool_hash) => {
                    // SPO votes require the pool operator key hash (28-byte).
                    let mut key_bytes = [0u8; 28];
                    key_bytes.copy_from_slice(&pool_hash.as_bytes()[..28]);
                    witnesses.insert(Hash28::from_bytes(key_bytes));
                }
                _ => {}
            }
        }

        // 6. Proposer key hashes from proposal_procedures (Conway-specific).
        for proposal in &tx.body.proposal_procedures {
            // The return address is a reward account (29 bytes: header + 28-byte hash).
            // If it's a key credential (bit 4 of header = 0), the key hash is a
            // required witness.
            if proposal.return_addr.len() >= 29 && proposal.return_addr[0] & 0x10 == 0 {
                let mut key_bytes = [0u8; 28];
                key_bytes.copy_from_slice(&proposal.return_addr[1..29]);
                witnesses.insert(Hash28::from_bytes(key_bytes));
            }
        }

        witnesses
    }
}

// ---------------------------------------------------------------------------
// Conway-specific helper functions
// ---------------------------------------------------------------------------

/// Process Conway-era governance certificates from a transaction.
///
/// Handles certificate types introduced in the Conway era (CIP-1694):
/// - `ConwayStakeRegistration` / `ConwayStakeDeregistration` -- same as Shelley
///   but with explicit deposit amount.
/// - `RegDRep` -- register a new DRep.
/// - `UnregDRep` -- deregister a DRep (refund deposit).
/// - `UpdateDRep` -- update DRep metadata anchor.
/// - `VoteDelegation` -- delegate vote to a DRep.
/// - `StakeVoteDelegation` -- combined: delegate stake to pool + vote to DRep.
/// - `RegStakeDeleg` -- combined: register stake + delegate to pool.
/// - `RegStakeVoteDeleg` -- combined: register stake + delegate to pool + vote to DRep.
/// - `VoteRegDeleg` -- combined: register stake + delegate vote.
/// - `CommitteeHotAuth` -- authorize a hot key for a CC cold credential.
/// - `CommitteeColdResign` -- resign from the constitutional committee.
///
/// Shelley-era certificate types (StakeRegistration, StakeDeregistration, etc.)
/// are handled by `common::process_shelley_certs` and are NOT processed here.
/// Apply a single Conway-era certificate to the ledger state.
///
/// Non-Conway cert variants are ignored (no-op). Callers must invoke the
/// Shelley-era handler separately for those.
fn apply_conway_cert(
    cert: &Certificate,
    current_epoch: EpochNo,
    certs: &mut CertSubState,
    gov: &mut GovSubState,
    epochs: &EpochSubState,
) {
    let drep_activity = epochs.protocol_params.drep_activity;
    let pv_major = epochs.protocol_params.protocol_version_major;
    let governance = Arc::make_mut(&mut gov.governance);
    let drep_expiry = {
        let base = current_epoch.0 + drep_activity;
        if pv_major >= 10 {
            EpochNo(base.saturating_sub(governance.num_dormant_epochs))
        } else {
            EpochNo(base)
        }
    };

    match cert {
        // Conway stake registration with explicit deposit (cert tag 7).
        // Same effect as Shelley StakeRegistration but the deposit is in the cert.
        Certificate::ConwayStakeRegistration {
            credential,
            deposit,
        } => {
            let key = credential.to_typed_hash32();
            certs
                .stake_distribution
                .stake_map
                .entry(key)
                .or_insert(Lovelace(0));
            Arc::make_mut(&mut certs.reward_accounts)
                .entry(key)
                .or_insert(Lovelace(0));
            if matches!(credential, Credential::Script(_)) {
                certs.script_stake_credentials.insert(key);
            }
            certs.total_stake_key_deposits += deposit.0;
            certs.stake_key_deposits.insert(key, deposit.0);
            debug!("Conway stake key registered: {}", key.to_hex());
        }

        // Conway stake deregistration with explicit refund (cert tag 8).
        Certificate::ConwayStakeDeregistration { credential, refund } => {
            let key = credential.to_typed_hash32();
            let stored_deposit = certs.stake_key_deposits.remove(&key).unwrap_or(refund.0);
            certs.total_stake_key_deposits = certs
                .total_stake_key_deposits
                .saturating_sub(stored_deposit);
            Arc::make_mut(&mut certs.delegations).remove(&key);
            Arc::make_mut(&mut certs.reward_accounts).remove(&key);
            governance.vote_delegations.remove(&key);
            certs.script_stake_credentials.remove(&key);
            certs.pointer_map.retain(|_, v| *v != key);
            debug!("Conway stake key deregistered: {}", key.to_hex());
        }

        // DRep registration.
        Certificate::RegDRep {
            credential,
            deposit,
            anchor,
        } => {
            let key = credential.to_typed_hash32();
            governance.dreps.insert(
                key,
                DRepRegistration {
                    credential: credential.clone(),
                    deposit: *deposit,
                    anchor: anchor.clone(),
                    registered_epoch: current_epoch,
                    drep_expiry,
                    active: true,
                },
            );
            governance.drep_registration_count += 1;
            debug!("DRep registered: {}", key.to_hex());
        }

        // DRep deregistration.
        Certificate::UnregDRep {
            credential,
            refund: _,
        } => {
            let key = credential.to_typed_hash32();
            governance.dreps.remove(&key);
            debug!("DRep deregistered: {}", key.to_hex());
        }

        // DRep metadata update.
        Certificate::UpdateDRep {
            credential, anchor, ..
        } => {
            let key = credential.to_typed_hash32();
            if let Some(drep) = governance.dreps.get_mut(&key) {
                drep.anchor = anchor.clone();
                drep.drep_expiry = drep_expiry;
                drep.active = true;
                debug!("DRep updated: {}", key.to_hex());
            }
        }

        // Vote delegation: delegate stake credential's vote to a DRep.
        Certificate::VoteDelegation { credential, drep } => {
            let key = credential.to_typed_hash32();
            governance.vote_delegations.insert(key, drep.clone());
            debug!("Vote delegated to DRep: {}", key.to_hex());
        }

        // Combined: delegate stake to pool + vote to DRep.
        Certificate::StakeVoteDelegation {
            credential,
            pool_hash,
            drep,
        } => {
            let key = credential.to_typed_hash32();
            Arc::make_mut(&mut certs.delegations).insert(key, *pool_hash);
            governance.vote_delegations.insert(key, drep.clone());
            debug!(
                "Stake+vote delegated: {} -> pool {} + DRep",
                key.to_hex(),
                pool_hash.to_hex()
            );
        }

        // Combined: register stake + delegate to pool.
        Certificate::RegStakeDeleg {
            credential,
            pool_hash,
            deposit,
        } => {
            let key = credential.to_typed_hash32();
            // Register stake.
            certs
                .stake_distribution
                .stake_map
                .entry(key)
                .or_insert(Lovelace(0));
            Arc::make_mut(&mut certs.reward_accounts)
                .entry(key)
                .or_insert(Lovelace(0));
            if matches!(credential, Credential::Script(_)) {
                certs.script_stake_credentials.insert(key);
            }
            certs.total_stake_key_deposits += deposit.0;
            certs.stake_key_deposits.insert(key, deposit.0);
            // Delegate to pool.
            Arc::make_mut(&mut certs.delegations).insert(key, *pool_hash);
            debug!(
                "RegStakeDeleg: {} -> pool {}",
                key.to_hex(),
                pool_hash.to_hex()
            );
        }

        // Combined: register stake + delegate to pool + delegate vote.
        Certificate::RegStakeVoteDeleg {
            credential,
            pool_hash,
            drep,
            deposit,
        } => {
            let key = credential.to_typed_hash32();
            // Register stake.
            certs
                .stake_distribution
                .stake_map
                .entry(key)
                .or_insert(Lovelace(0));
            Arc::make_mut(&mut certs.reward_accounts)
                .entry(key)
                .or_insert(Lovelace(0));
            if matches!(credential, Credential::Script(_)) {
                certs.script_stake_credentials.insert(key);
            }
            certs.total_stake_key_deposits += deposit.0;
            certs.stake_key_deposits.insert(key, deposit.0);
            // Delegate to pool + DRep.
            Arc::make_mut(&mut certs.delegations).insert(key, *pool_hash);
            governance.vote_delegations.insert(key, drep.clone());
            debug!(
                "RegStakeVoteDeleg: {} -> pool {} + DRep",
                key.to_hex(),
                pool_hash.to_hex()
            );
        }

        // Combined: register stake + delegate vote.
        Certificate::VoteRegDeleg {
            credential,
            drep,
            deposit,
        } => {
            let key = credential.to_typed_hash32();
            // Register stake.
            certs
                .stake_distribution
                .stake_map
                .entry(key)
                .or_insert(Lovelace(0));
            Arc::make_mut(&mut certs.reward_accounts)
                .entry(key)
                .or_insert(Lovelace(0));
            if matches!(credential, Credential::Script(_)) {
                certs.script_stake_credentials.insert(key);
            }
            certs.total_stake_key_deposits += deposit.0;
            certs.stake_key_deposits.insert(key, deposit.0);
            // Delegate vote.
            governance.vote_delegations.insert(key, drep.clone());
            debug!("VoteRegDeleg: {} + DRep", key.to_hex());
        }

        // CC hot key authorization.
        Certificate::CommitteeHotAuth {
            cold_credential,
            hot_credential,
        } => {
            let cold_key = cold_credential.to_typed_hash32();
            let hot_key = hot_credential.to_typed_hash32();
            governance.committee_hot_keys.insert(cold_key, hot_key);
            // Track script credentials for N2C query type fields.
            if matches!(cold_credential, Credential::Script(_)) {
                governance.script_committee_credentials.insert(cold_key);
            }
            if matches!(hot_credential, Credential::Script(_)) {
                governance.script_committee_hot_credentials.insert(cold_key);
            } else {
                // Re-auth with key hot key: remove previous script tracking.
                governance
                    .script_committee_hot_credentials
                    .remove(&cold_key);
            }
            debug!(
                "CC hot key authorized: {} -> {}",
                cold_key.to_hex(),
                hot_key.to_hex()
            );
        }

        // CC cold key resignation.
        Certificate::CommitteeColdResign {
            cold_credential,
            anchor,
        } => {
            let cold_key = cold_credential.to_typed_hash32();
            governance
                .committee_resigned
                .insert(cold_key, anchor.clone());
            if matches!(cold_credential, Credential::Script(_)) {
                governance.script_committee_credentials.insert(cold_key);
            }
            debug!("CC member resigned: {}", cold_key.to_hex());
        }

        // Skip Shelley certs and any unrecognized variants -- handled by
        // apply_shelley_cert or not relevant.
        _ => {}
    }
}

/// Update DRep expiry for DReps that vote in this transaction.
///
/// Per CIP-1694, a DRep's activity timer resets whenever they cast a vote.
/// This implements step 5 of the Conway LEDGER pipeline
/// (updateVotingDRepExpiries).
fn update_drep_expiries_for_tx(
    tx: &Transaction,
    current_epoch: EpochNo,
    gov: &mut GovSubState,
    epochs: &EpochSubState,
) {
    if tx.body.voting_procedures.is_empty() {
        return;
    }

    let activity = epochs.protocol_params.drep_activity;
    let base = current_epoch.0 + activity;
    let governance = Arc::make_mut(&mut gov.governance);
    let expiry = if epochs.protocol_params.protocol_version_major >= 10 {
        EpochNo(base.saturating_sub(governance.num_dormant_epochs))
    } else {
        EpochNo(base)
    };
    for voter in tx.body.voting_procedures.keys() {
        if let Voter::DRep(credential) = voter {
            let key = credential.to_typed_hash32();
            if let Some(drep) = governance.dreps.get_mut(&key) {
                drep.drep_expiry = expiry;
                drep.active = true;
            }
        }
    }
}

/// Process governance votes and proposals from a transaction (GOV rule).
///
/// Implements step 8 of the Conway LEDGER pipeline:
/// - Record votes from voting_procedures.
/// - Register new governance proposals from proposal_procedures.
fn process_governance_votes_and_proposals(
    tx: &Transaction,
    ctx: &RuleContext,
    gov: &mut GovSubState,
    epochs: &EpochSubState,
) {
    let governance = Arc::make_mut(&mut gov.governance);

    // Process votes.
    for (voter, action_votes) in &tx.body.voting_procedures {
        for (action_id, vote_proc) in action_votes {
            governance
                .votes_by_action
                .entry(action_id.clone())
                .or_default()
                .push((voter.clone(), vote_proc.clone()));
        }
    }

    // Process proposals.
    for (idx, proposal) in tx.body.proposal_procedures.iter().enumerate() {
        let action_id = GovActionId {
            transaction_id: tx.hash,
            action_index: idx as u32,
        };
        let gov_action_lifetime = epochs.protocol_params.gov_action_lifetime;
        let proposal_state = ProposalState {
            procedure: proposal.clone(),
            proposed_epoch: ctx.current_epoch,
            expires_epoch: EpochNo(ctx.current_epoch.0 + gov_action_lifetime),
            yes_votes: 0,
            no_votes: 0,
            abstain_votes: 0,
        };
        governance
            .proposals
            .insert(action_id.clone(), proposal_state);
        governance.proposal_count += 1;

        if let Some(tag) = gov_action_purpose_tag(&proposal.gov_action) {
            let prev = gov_action_raw_prev_id(&proposal.gov_action);
            forest_add_proposal(
                &action_id,
                prev.as_ref(),
                tag,
                &mut governance.proposal_roots,
                &mut governance.proposal_graph,
            );
        }
    }
}

/// Extract a Hash32 from a raw reward account byte string (29 bytes).
///
/// Mirrors the logic in common.rs but is kept local to avoid circular deps.
fn reward_account_to_hash(reward_account: &[u8]) -> Hash32 {
    let mut key_bytes = [0u8; 32];
    if reward_account.len() >= 29 {
        key_bytes[..28].copy_from_slice(&reward_account[1..29]);
        if reward_account[0] & 0x10 != 0 {
            key_bytes[28] = 0x01; // script credential
        }
    }
    Hash32::from_bytes(key_bytes)
}

// ---------------------------------------------------------------------------
// Internal helpers for collateral stub state
// ---------------------------------------------------------------------------

#[cfg(test)]
use crate::state::{EpochSnapshots, StakeDistributionState};
#[cfg(test)]
use dugite_primitives::protocol_params::ProtocolParameters;

/// Create a minimal CertSubState for testing collateral consumption.
#[cfg(test)]
fn make_empty_cert_sub() -> CertSubState {
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

/// Create a minimal EpochSubState for testing collateral consumption.
#[cfg(test)]
fn make_empty_epoch_sub() -> EpochSubState {
    EpochSubState {
        snapshots: EpochSnapshots::default(),
        treasury: Lovelace(0),
        reserves: Lovelace(0),
        pending_reward_update: None,
        pending_pp_updates: std::collections::BTreeMap::new(),
        future_pp_updates: std::collections::BTreeMap::new(),
        needs_stake_rebuild: false,
        ptr_stake: HashMap::new(),
        ptr_stake_excluded: true, // Conway always excludes pointer stake.
        protocol_params: ProtocolParameters::mainnet_defaults(),
        prev_protocol_params: ProtocolParameters::mainnet_defaults(),
        prev_protocol_version_major: 9,
        prev_d: 0.0,
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
        BlockValidationMode, GovernanceState, PoolRegistration, StakeDistributionState,
    };
    use crate::utxo::UtxoSet;
    use crate::utxo_diff::DiffSeq;
    use dugite_primitives::address::Address;
    use dugite_primitives::hash::Hash32;
    use dugite_primitives::protocol_params::ProtocolParameters;
    use dugite_primitives::time::EpochNo;
    use dugite_primitives::transaction::{
        Anchor, DRep, OutputDatum, ProposalProcedure, TransactionBody, TransactionInput,
        TransactionOutput, TransactionWitnessSet, Vote, VotingProcedure,
    };
    use dugite_primitives::value::Lovelace;
    use dugite_primitives::value::Value;
    use std::collections::{BTreeMap, HashMap};
    use std::sync::Arc;

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    fn make_conway_ctx(params: &ProtocolParameters) -> RuleContext<'_> {
        let delegates = Box::leak(Box::new(HashMap::new()));
        RuleContext {
            params,
            current_slot: 100,
            current_epoch: EpochNo(5),
            era: Era::Conway,
            slot_config: None,
            node_network: None,
            genesis_delegates: delegates,
            update_quorum: 5,
            epoch_length: 432000,
            shelley_transition_epoch: 0,
            byron_epoch_length: 21600,
            stability_window: 129600,
            stability_window_3kf: 129600,
            randomness_stabilisation_window: 129600,
            tx_index: 0,
            conway_genesis: None,
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
            ptr_stake_excluded: true, // Conway
            protocol_params: ProtocolParameters::mainnet_defaults(),
            prev_protocol_params: ProtocolParameters::mainnet_defaults(),
            prev_protocol_version_major: 9,
            prev_d: 0.0,
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
            era: Era::Conway,
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

    /// Verify that EraRulesImpl::for_era correctly maps Conway.
    #[test]
    fn test_era_rules_impl_for_conway() {
        assert!(matches!(
            EraRulesImpl::for_era(Era::Conway),
            EraRulesImpl::Conway(_)
        ));
    }

    /// validate_block_body always succeeds for Conway (budget check not yet implemented).
    #[test]
    fn test_validate_block_body_succeeds() {
        let rules = ConwayRules::new();
        let params = ProtocolParameters::mainnet_defaults();
        let ctx = make_conway_ctx(&params);
        let utxo = make_utxo_sub(vec![]);

        let block = Block {
            era: Era::Conway,
            header: dugite_primitives::block::BlockHeader {
                header_hash: Hash32::ZERO,
                prev_hash: Hash32::ZERO,
                issuer_vkey: vec![],
                vrf_vkey: vec![],
                vrf_result: dugite_primitives::block::VrfOutput {
                    output: vec![],
                    proof: vec![],
                },
                block_number: dugite_primitives::time::BlockNo(0),
                slot: dugite_primitives::time::SlotNo(0),
                epoch_nonce: Hash32::ZERO,
                body_size: 0,
                body_hash: Hash32::ZERO,
                operational_cert: dugite_primitives::block::OperationalCert {
                    hot_vkey: vec![],
                    sequence_number: 0,
                    kes_period: 0,
                    sigma: vec![],
                },
                protocol_version: dugite_primitives::block::ProtocolVersion { major: 9, minor: 0 },
                kes_signature: vec![],
                nonce_vrf_output: vec![],
                nonce_vrf_proof: vec![],
            },
            transactions: vec![],
            raw_cbor: None,
        };

        assert!(rules.validate_block_body(&block, &ctx, &utxo).is_ok());
    }

    /// Apply a valid Conway transaction that spends a UTxO and produces a new one.
    #[test]
    fn test_apply_valid_tx_basic_utxo() {
        let rules = ConwayRules::new();
        let params = ProtocolParameters::mainnet_defaults();
        let ctx = make_conway_ctx(&params);

        let key_hash = Hash28::from_bytes([0x42; 28]);
        let addr = make_enterprise_address(key_hash);
        let input = make_input(0xAA, 0);
        let spent_output = make_output(addr.clone(), 5_000_000);
        let mut utxo = make_utxo_sub(vec![(input.clone(), spent_output)]);
        let mut certs = make_cert_sub();
        let mut gov = make_gov_sub();
        let mut epochs = make_epoch_sub();

        let new_output = make_output(addr, 4_800_000);
        let tx = make_tx(0x01, vec![input.clone()], vec![new_output], 200_000);
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
        assert!(!utxo.utxo_set.contains(&input));
    }

    /// Apply a valid Conway transaction with a treasury donation.
    #[test]
    fn test_apply_valid_tx_with_donation() {
        let rules = ConwayRules::new();
        let params = ProtocolParameters::mainnet_defaults();
        let ctx = make_conway_ctx(&params);

        let key_hash = Hash28::from_bytes([0x42; 28]);
        let addr = make_enterprise_address(key_hash);
        let input = make_input(0xAA, 0);
        let spent_output = make_output(addr.clone(), 10_000_000);
        let mut utxo = make_utxo_sub(vec![(input.clone(), spent_output)]);
        let mut certs = make_cert_sub();
        let mut gov = make_gov_sub();
        let mut epochs = make_epoch_sub();

        let new_output = make_output(addr, 9_500_000);
        let mut tx = make_tx(0x01, vec![input], vec![new_output], 200_000);
        tx.body.donation = Some(Lovelace(300_000));

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
        // Donation should be accumulated in pending_donations.
        assert_eq!(utxo.pending_donations.0, 300_000);
    }

    /// Apply an invalid Conway transaction with collateral return.
    #[test]
    fn test_apply_invalid_tx_with_collateral_return() {
        let rules = ConwayRules::new();
        let params = ProtocolParameters::mainnet_defaults();
        let ctx = make_conway_ctx(&params);

        let key_hash = Hash28::from_bytes([0x42; 28]);
        let addr = make_enterprise_address(key_hash);

        let collateral_input = make_input(0xCC, 0);
        let collateral_output = make_output(addr.clone(), 10_000_000);
        let mut utxo = make_utxo_sub(vec![(collateral_input.clone(), collateral_output)]);

        let mut tx = make_tx(0x02, vec![], vec![], 0);
        tx.is_valid = false;
        tx.body.collateral = vec![collateral_input.clone()];
        tx.body.collateral_return = Some(make_output(addr, 8_000_000));
        tx.body.total_collateral = Some(Lovelace(2_000_000));

        let mut certs = make_empty_cert_sub();
        let mut epochs = make_empty_epoch_sub();
        let result = rules.apply_invalid_tx(
            &tx,
            BlockValidationMode::ApplyOnly,
            &ctx,
            &mut utxo,
            &mut certs,
            &mut epochs,
        );
        assert!(result.is_ok());
        let diff = result.unwrap();

        assert_eq!(diff.deletes.len(), 1);
        assert_eq!(diff.inserts.len(), 1);
        assert_eq!(utxo.epoch_fees.0, 2_000_000);
    }

    /// Conway min_fee matches the linear formula.
    #[test]
    fn test_min_fee_linear() {
        let rules = ConwayRules::new();
        let mut params = ProtocolParameters::mainnet_defaults();
        params.min_fee_a = 44;
        params.min_fee_b = 155381;
        let ctx = make_conway_ctx(&params);
        let utxo = make_utxo_sub(vec![]);

        let tx = make_tx(0x01, vec![], vec![], 0);
        let fee = rules.min_fee(&tx, &ctx, &utxo);
        assert_eq!(fee, 44 * 200 + 155381);
    }

    /// on_era_transition sets ptr_stake_excluded and resets donations.
    #[test]
    fn test_on_era_transition_excludes_pointer_stake() {
        let rules = ConwayRules::new();
        let params = ProtocolParameters::mainnet_defaults();
        let ctx = make_conway_ctx(&params);
        let mut utxo = make_utxo_sub(vec![]);
        utxo.pending_donations = Lovelace(500_000);
        let mut certs = make_cert_sub();
        let mut gov = make_gov_sub();
        let mut consensus = make_consensus_sub();
        let mut epochs = make_epoch_sub();
        epochs.ptr_stake_excluded = false;

        let result = rules.on_era_transition(
            Era::Babbage,
            &ctx,
            &mut utxo,
            &mut certs,
            &mut gov,
            &mut consensus,
            &mut epochs,
        );
        assert!(result.is_ok());
        // Pointer stake should be excluded after transition.
        assert!(epochs.ptr_stake_excluded);
        // Donations should be reset.
        assert_eq!(utxo.pending_donations.0, 0);
    }

    /// Process Conway DRep registration certificate.
    #[test]
    fn test_conway_drep_registration() {
        let rules = ConwayRules::new();
        let params = ProtocolParameters::mainnet_defaults();
        let ctx = make_conway_ctx(&params);

        let mut utxo = make_utxo_sub(vec![]);
        let mut certs = make_cert_sub();
        let mut gov = make_gov_sub();
        let mut epochs = make_epoch_sub();

        let drep_key = Hash28::from_bytes([0xDD; 28]);
        let drep_cred = Credential::VerificationKey(drep_key);
        let mut tx = make_tx(0x01, vec![], vec![], 0);
        tx.body.certificates = vec![Certificate::RegDRep {
            credential: drep_cred.clone(),
            deposit: Lovelace(500_000_000),
            anchor: None,
        }];

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

        let key = drep_cred.to_typed_hash32();
        assert!(gov.governance.dreps.contains_key(&key));
        let drep = &gov.governance.dreps[&key];
        assert_eq!(drep.deposit.0, 500_000_000);
        assert!(drep.active);
        assert_eq!(drep.registered_epoch, EpochNo(5));
    }

    /// Process Conway vote delegation certificate.
    #[test]
    fn test_conway_vote_delegation() {
        let rules = ConwayRules::new();
        let params = ProtocolParameters::mainnet_defaults();
        let ctx = make_conway_ctx(&params);

        let mut utxo = make_utxo_sub(vec![]);
        let mut certs = make_cert_sub();
        let mut gov = make_gov_sub();
        let mut epochs = make_epoch_sub();

        let stake_key = Hash28::from_bytes([0xAA; 28]);
        let cred = Credential::VerificationKey(stake_key);
        let mut tx = make_tx(0x01, vec![], vec![], 0);
        tx.body.certificates = vec![Certificate::VoteDelegation {
            credential: cred.clone(),
            drep: DRep::Abstain,
        }];

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

        let key = cred.to_typed_hash32();
        assert_eq!(
            gov.governance.vote_delegations.get(&key),
            Some(&DRep::Abstain)
        );
    }

    /// Process Conway committee hot key authorization.
    #[test]
    fn test_conway_committee_hot_auth() {
        let rules = ConwayRules::new();
        let params = ProtocolParameters::mainnet_defaults();
        let ctx = make_conway_ctx(&params);

        let mut utxo = make_utxo_sub(vec![]);
        let mut certs = make_cert_sub();
        let mut gov = make_gov_sub();
        let mut epochs = make_epoch_sub();

        let cold_key = Hash28::from_bytes([0xC0; 28]);
        let hot_key = Hash28::from_bytes([0xBB; 28]);
        let cold_cred = Credential::VerificationKey(cold_key);
        let hot_cred = Credential::VerificationKey(hot_key);
        let mut tx = make_tx(0x01, vec![], vec![], 0);
        tx.body.certificates = vec![Certificate::CommitteeHotAuth {
            cold_credential: cold_cred.clone(),
            hot_credential: hot_cred.clone(),
        }];

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

        let cold_hash = cold_cred.to_typed_hash32();
        let hot_hash = hot_cred.to_typed_hash32();
        assert_eq!(
            gov.governance.committee_hot_keys.get(&cold_hash),
            Some(&hot_hash)
        );
    }

    /// Governance votes are recorded correctly.
    #[test]
    fn test_conway_governance_votes() {
        let rules = ConwayRules::new();
        let params = ProtocolParameters::mainnet_defaults();
        let ctx = make_conway_ctx(&params);

        let mut utxo = make_utxo_sub(vec![]);
        let mut certs = make_cert_sub();
        let mut gov = make_gov_sub();
        let mut epochs = make_epoch_sub();

        let drep_key = Hash28::from_bytes([0xDD; 28]);
        let drep_cred = Credential::VerificationKey(drep_key);
        let action_id = GovActionId {
            transaction_id: Hash32::from_bytes([0xAA; 32]),
            action_index: 0,
        };
        let vote_proc = VotingProcedure {
            vote: Vote::Yes,
            anchor: None,
        };

        let mut tx = make_tx(0x01, vec![], vec![], 0);
        let mut action_votes = BTreeMap::new();
        action_votes.insert(action_id.clone(), vote_proc);
        tx.body
            .voting_procedures
            .insert(Voter::DRep(drep_cred), action_votes);

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

        assert!(gov.governance.votes_by_action.contains_key(&action_id));
        assert_eq!(gov.governance.votes_by_action[&action_id].len(), 1);
    }

    /// Governance proposals are recorded correctly.
    #[test]
    fn test_conway_governance_proposals() {
        let rules = ConwayRules::new();
        let mut params = ProtocolParameters::mainnet_defaults();
        params.gov_action_lifetime = 6;
        let ctx = make_conway_ctx(&params);

        let mut utxo = make_utxo_sub(vec![]);
        let mut certs = make_cert_sub();
        let mut gov = make_gov_sub();
        let mut epochs = make_epoch_sub();
        epochs.protocol_params.gov_action_lifetime = 6;

        let mut tx = make_tx(0x01, vec![], vec![], 0);
        tx.body.proposal_procedures = vec![ProposalProcedure {
            deposit: Lovelace(100_000_000_000),
            return_addr: vec![0xe0; 29],
            gov_action: dugite_primitives::transaction::GovAction::InfoAction,
            anchor: Anchor {
                url: "https://example.com".to_string(),
                data_hash: Hash32::ZERO,
            },
        }];

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

        assert_eq!(gov.governance.proposals.len(), 1);
        assert_eq!(gov.governance.proposal_count, 1);
        let (_, ps) = gov.governance.proposals.iter().next().unwrap();
        assert_eq!(ps.proposed_epoch, EpochNo(5));
        assert_eq!(ps.expires_epoch, EpochNo(11));
    }

    /// Conway epoch transition flushes donations and prunes expired CC members.
    #[test]
    fn test_process_epoch_transition_conway() {
        let rules = ConwayRules::new();
        let params = ProtocolParameters::mainnet_defaults();
        let ctx = make_conway_ctx(&params);

        let mut utxo = make_utxo_sub(vec![]);
        utxo.pending_donations = Lovelace(1_000_000);
        utxo.epoch_fees = Lovelace(500_000);
        let mut certs = make_cert_sub();
        let mut gov = make_gov_sub();
        let mut epochs = make_epoch_sub();
        epochs.treasury = Lovelace(100_000_000);
        let mut consensus = make_consensus_sub();

        // Add an expired CC member (expired at epoch 4, we are transitioning to 6).
        let expired_cc = Hash32::from_bytes([0xCC; 32]);
        Arc::make_mut(&mut gov.governance)
            .committee_expiration
            .insert(expired_cc, EpochNo(4));
        Arc::make_mut(&mut gov.governance)
            .committee_hot_keys
            .insert(expired_cc, Hash32::from_bytes([0xBB; 32]));

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

        // Donations flushed to treasury.
        assert_eq!(utxo.pending_donations.0, 0);
        assert_eq!(epochs.treasury.0, 101_000_000);

        // Epoch fees reset.
        assert_eq!(utxo.epoch_fees.0, 0);

        // Expired CC member pruned.
        assert!(!gov
            .governance
            .committee_expiration
            .contains_key(&expired_cc));
        assert!(!gov.governance.committee_hot_keys.contains_key(&expired_cc));
    }

    /// Conway epoch transition handles pool retirement.
    #[test]
    fn test_process_epoch_transition_pool_retirement() {
        let rules = ConwayRules::new();
        let params = ProtocolParameters::mainnet_defaults();
        let ctx = make_conway_ctx(&params);

        let mut utxo = make_utxo_sub(vec![]);
        let mut certs = make_cert_sub();
        let mut gov = make_gov_sub();
        let mut epochs = make_epoch_sub();
        let mut consensus = make_consensus_sub();

        // Register a pool and schedule retirement at epoch 6.
        let pool_id = Hash28::from_bytes([0xAA; 28]);
        let mut reward_addr = vec![0xe0u8];
        reward_addr.extend_from_slice(&[0xBB; 28]);
        let pool_reg = PoolRegistration {
            pool_id,
            vrf_keyhash: Hash32::ZERO,
            pledge: Lovelace(100_000),
            cost: Lovelace(340_000_000),
            margin_numerator: 1,
            margin_denominator: 100,
            reward_account: reward_addr.clone(),
            owners: vec![pool_id],
            relays: vec![],
            metadata_url: None,
            metadata_hash: None,
        };
        Arc::make_mut(&mut certs.pool_params).insert(pool_id, pool_reg);
        certs.pool_deposits.insert(pool_id, 500_000_000);
        certs.pending_retirements.insert(pool_id, EpochNo(6));

        // Create the reward account so the deposit can be refunded.
        let op_key = reward_account_to_hash(&reward_addr);
        Arc::make_mut(&mut certs.reward_accounts).insert(op_key, Lovelace(0));

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

        // Pool should be removed.
        assert!(!certs.pool_params.contains_key(&pool_id));
        assert!(!certs.pool_deposits.contains_key(&pool_id));
        // Deposit refunded to reward account.
        assert_eq!(
            certs.reward_accounts.get(&op_key),
            Some(&Lovelace(500_000_000))
        );
    }

    /// required_witnesses includes DRep voter keys and proposer keys.
    #[test]
    fn test_required_witnesses_conway_governance() {
        let rules = ConwayRules::new();
        let params = ProtocolParameters::mainnet_defaults();
        let ctx = make_conway_ctx(&params);

        let utxo = make_utxo_sub(vec![]);
        let certs = make_cert_sub();
        let gov = make_gov_sub();

        let drep_key = Hash28::from_bytes([0xDD; 28]);
        let drep_cred = Credential::VerificationKey(drep_key);
        let action_id = GovActionId {
            transaction_id: Hash32::from_bytes([0xAA; 32]),
            action_index: 0,
        };
        let vote_proc = VotingProcedure {
            vote: Vote::Yes,
            anchor: None,
        };

        let mut tx = make_tx(0x01, vec![], vec![], 0);
        let mut action_votes = BTreeMap::new();
        action_votes.insert(action_id, vote_proc);
        tx.body
            .voting_procedures
            .insert(Voter::DRep(drep_cred), action_votes);

        // Add a proposal with a key-hash return address.
        let mut return_addr = vec![0xe0u8];
        return_addr.extend_from_slice(&[0xBB; 28]);
        tx.body.proposal_procedures = vec![ProposalProcedure {
            deposit: Lovelace(100_000_000_000),
            return_addr,
            gov_action: dugite_primitives::transaction::GovAction::InfoAction,
            anchor: Anchor {
                url: "https://example.com".to_string(),
                data_hash: Hash32::ZERO,
            },
        }];

        let witnesses = rules.required_witnesses(&tx, &ctx, &utxo, &certs, &gov);

        // DRep voter key should be required.
        assert!(witnesses.contains(&drep_key));
        // Proposer return address key should be required.
        assert!(witnesses.contains(&Hash28::from_bytes([0xBB; 28])));
        assert_eq!(witnesses.len(), 2);
    }

    /// DRep activity is updated when they cast a vote.
    #[test]
    fn test_drep_activity_updated_on_vote() {
        let rules = ConwayRules::new();
        let params = ProtocolParameters::mainnet_defaults();
        let ctx = make_conway_ctx(&params);

        let mut utxo = make_utxo_sub(vec![]);
        let mut certs = make_cert_sub();
        let mut gov = make_gov_sub();
        let mut epochs = make_epoch_sub();

        // Register a DRep at epoch 2.
        let drep_key = Hash28::from_bytes([0xDD; 28]);
        let drep_cred = Credential::VerificationKey(drep_key);
        let key = drep_cred.to_typed_hash32();
        Arc::make_mut(&mut gov.governance).dreps.insert(
            key,
            DRepRegistration {
                credential: drep_cred.clone(),
                deposit: Lovelace(500_000_000),
                anchor: None,
                registered_epoch: EpochNo(2),
                drep_expiry: EpochNo(22), // epoch 2 + drep_activity 20
                active: true,
            },
        );

        // DRep casts a vote at epoch 5.
        let action_id = GovActionId {
            transaction_id: Hash32::from_bytes([0xAA; 32]),
            action_index: 0,
        };
        let vote_proc = VotingProcedure {
            vote: Vote::Yes,
            anchor: None,
        };
        let mut tx = make_tx(0x01, vec![], vec![], 0);
        let mut action_votes = BTreeMap::new();
        action_votes.insert(action_id, vote_proc);
        tx.body
            .voting_procedures
            .insert(Voter::DRep(drep_cred), action_votes);

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

        // DRep expiry should be updated: epoch 5 + drep_activity 20 = 25.
        let drep = &gov.governance.dreps[&key];
        assert_eq!(drep.drep_expiry, EpochNo(25));
    }

    /// Conway DRep deregistration removes the DRep.
    #[test]
    fn test_conway_drep_deregistration() {
        let rules = ConwayRules::new();
        let params = ProtocolParameters::mainnet_defaults();
        let ctx = make_conway_ctx(&params);

        let mut utxo = make_utxo_sub(vec![]);
        let mut certs = make_cert_sub();
        let mut gov = make_gov_sub();
        let mut epochs = make_epoch_sub();

        let drep_key = Hash28::from_bytes([0xDD; 28]);
        let drep_cred = Credential::VerificationKey(drep_key);
        let key = drep_cred.to_typed_hash32();
        Arc::make_mut(&mut gov.governance).dreps.insert(
            key,
            DRepRegistration {
                credential: drep_cred.clone(),
                deposit: Lovelace(500_000_000),
                anchor: None,
                registered_epoch: EpochNo(2),
                drep_expiry: EpochNo(22),
                active: true,
            },
        );

        let mut tx = make_tx(0x01, vec![], vec![], 0);
        tx.body.certificates = vec![Certificate::UnregDRep {
            credential: drep_cred,
            refund: Lovelace(500_000_000),
        }];

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
        assert!(!gov.governance.dreps.contains_key(&key));
    }

    /// Regression test for the cstreamer epoch-890 divergence: in Conway era,
    /// a tx containing `[ConwayStakeDeregistration, ConwayStakeRegistration,
    /// StakeDelegation]` (in that cert order) must end with the credential
    /// still registered AND delegated to the pool.
    ///
    /// Previously the Conway era applied all Shelley certs in one pass then
    /// all Conway certs in a second pass. That made the Shelley `StakeDelegation`
    /// fire before the Conway `ConwayStakeDeregistration` even though the
    /// on-chain cert order was the opposite, so the subsequent DEREG wiped
    /// the just-inserted delegation. A script stake credential on preview
    /// (`c6a4349e...`, 1.39 B lovelace) dropped out of `delegations` as a
    /// result and never re-entered any mark snapshot, compounding into a
    /// persistent activeStake/reserves divergence vs the Haskell reference.
    #[test]
    fn test_interleaved_dereg_reg_deleg_preserves_delegation() {
        let rules = ConwayRules::new();
        let params = ProtocolParameters::mainnet_defaults();
        let ctx = make_conway_ctx(&params);

        let script_hash = Hash28::from_bytes([0xc6; 28]);
        let cred = Credential::Script(script_hash);
        let key = cred.to_typed_hash32();
        let pool_id = Hash28::from_bytes([0x24; 28]);

        let mut certs = make_cert_sub();
        // Pre-state: credential is registered + delegated to the pool (as it
        // would be at the start of the suspect tx on-chain).
        Arc::make_mut(&mut certs.delegations).insert(key, pool_id);
        Arc::make_mut(&mut certs.reward_accounts).insert(key, Lovelace(0));
        certs.stake_key_deposits.insert(key, 2_000_000);
        certs.total_stake_key_deposits = 2_000_000;
        certs.script_stake_credentials.insert(key);
        certs
            .stake_distribution
            .stake_map
            .insert(key, Lovelace(1_731_936_015));
        // Register the pool so it has a valid PoolRegistration entry -- the
        // delegation map is otherwise ignored by pool_stake aggregation.
        Arc::make_mut(&mut certs.pool_params).insert(
            pool_id,
            PoolRegistration {
                pool_id,
                vrf_keyhash: Hash32::ZERO,
                pledge: Lovelace(0),
                cost: Lovelace(0),
                margin_numerator: 0,
                margin_denominator: 1,
                reward_account: vec![0u8; 29],
                owners: vec![],
                relays: vec![],
                metadata_url: None,
                metadata_hash: None,
            },
        );

        let mut utxo = make_utxo_sub(vec![]);
        let mut gov = make_gov_sub();
        let mut epochs = make_epoch_sub();

        let mut tx = make_tx(0x94, vec![], vec![], 0);
        tx.body.certificates = vec![
            Certificate::ConwayStakeDeregistration {
                credential: cred.clone(),
                refund: Lovelace(2_000_000),
            },
            Certificate::ConwayStakeRegistration {
                credential: cred.clone(),
                deposit: Lovelace(2_000_000),
            },
            Certificate::StakeDelegation {
                credential: cred.clone(),
                pool_hash: pool_id,
            },
        ];

        rules
            .apply_valid_tx(
                &tx,
                BlockValidationMode::ApplyOnly,
                &ctx,
                &mut utxo,
                &mut certs,
                &mut gov,
                &mut epochs,
            )
            .expect("valid cert sequence should apply");

        assert_eq!(
            certs.delegations.get(&key).copied(),
            Some(pool_id),
            "credential must remain delegated after [DEREG, REG, DELEG] sequence"
        );
        assert!(
            certs.stake_distribution.stake_map.contains_key(&key),
            "credential must retain its stake_map entry"
        );
        assert_eq!(
            certs.stake_distribution.stake_map.get(&key).copied(),
            Some(Lovelace(1_731_936_015)),
            "stake_map value must be preserved through DEREG/REG cycle"
        );
        assert!(
            certs.script_stake_credentials.contains(&key),
            "script credential flag must be restored by the REG"
        );
        assert_eq!(
            certs.stake_key_deposits.get(&key).copied(),
            Some(2_000_000),
            "re-registered deposit must be tracked"
        );
    }

    // -----------------------------------------------------------------------
    // on_era_transition — ConwayGenesis seeding tests
    // -----------------------------------------------------------------------

    /// Verify DReps are populated from ConwayGenesis during era transition.
    #[test]
    fn test_on_era_transition_seeds_initial_dreps() {
        use crate::eras::ConwayGenesisInit;

        let rules = ConwayRules::new();
        let mut params = ProtocolParameters::mainnet_defaults();
        params.drep_activity = 100;
        let genesis = ConwayGenesisInit {
            initial_dreps: vec![
                (Hash28::from_bytes([0x01; 28]), 500_000_000),
                (Hash28::from_bytes([0x02; 28]), 1_000_000_000),
            ],
            ..Default::default()
        };
        let delegates = Box::leak(Box::new(HashMap::new()));
        let ctx = RuleContext {
            params: &params,
            current_slot: 100,
            current_epoch: EpochNo(10),
            era: Era::Conway,
            slot_config: None,
            node_network: None,
            genesis_delegates: delegates,
            update_quorum: 5,
            epoch_length: 432000,
            shelley_transition_epoch: 0,
            byron_epoch_length: 21600,
            stability_window: 129600,
            stability_window_3kf: 129600,
            randomness_stabilisation_window: 129600,
            tx_index: 0,
            conway_genesis: Some(&genesis),
        };

        let mut utxo = make_utxo_sub(vec![]);
        let mut certs = make_cert_sub();
        let mut gov = make_gov_sub();
        let mut consensus = make_consensus_sub();
        let mut epochs = make_epoch_sub();
        epochs.ptr_stake_excluded = false;

        rules
            .on_era_transition(
                Era::Babbage,
                &ctx,
                &mut utxo,
                &mut certs,
                &mut gov,
                &mut consensus,
                &mut epochs,
            )
            .expect("era transition should succeed");

        // Verify two DReps were seeded.
        assert_eq!(gov.governance.dreps.len(), 2);

        // Check first DRep.
        let key1 = Hash32::from_bytes({
            let mut buf = [0u8; 32];
            buf[..28].copy_from_slice(&[0x01; 28]);
            buf
        });
        let drep1 = gov.governance.dreps.get(&key1).expect("drep1 must exist");
        assert_eq!(drep1.deposit.0, 500_000_000);
        assert_eq!(drep1.drep_expiry, EpochNo(110)); // 10 + 100
        assert!(drep1.active);
        assert_eq!(drep1.registered_epoch, EpochNo(10));

        // Check second DRep.
        let key2 = Hash32::from_bytes({
            let mut buf = [0u8; 32];
            buf[..28].copy_from_slice(&[0x02; 28]);
            buf
        });
        let drep2 = gov.governance.dreps.get(&key2).expect("drep2 must exist");
        assert_eq!(drep2.deposit.0, 1_000_000_000);
    }

    /// Verify committee members and threshold are set from ConwayGenesis.
    #[test]
    fn test_on_era_transition_seeds_committee() {
        use crate::eras::ConwayGenesisInit;

        let rules = ConwayRules::new();
        let params = ProtocolParameters::mainnet_defaults();
        let cred1_bytes = [0xCC; 32];
        let cred2_bytes = [0xDD; 32];
        let genesis = ConwayGenesisInit {
            committee_members: vec![(cred1_bytes, 200), (cred2_bytes, 300)],
            committee_threshold: Some((2, 3)),
            ..Default::default()
        };
        let delegates = Box::leak(Box::new(HashMap::new()));
        let ctx = RuleContext {
            params: &params,
            current_slot: 100,
            current_epoch: EpochNo(5),
            era: Era::Conway,
            slot_config: None,
            node_network: None,
            genesis_delegates: delegates,
            update_quorum: 5,
            epoch_length: 432000,
            shelley_transition_epoch: 0,
            byron_epoch_length: 21600,
            stability_window: 129600,
            stability_window_3kf: 129600,
            randomness_stabilisation_window: 129600,
            tx_index: 0,
            conway_genesis: Some(&genesis),
        };

        let mut utxo = make_utxo_sub(vec![]);
        let mut certs = make_cert_sub();
        let mut gov = make_gov_sub();
        let mut consensus = make_consensus_sub();
        let mut epochs = make_epoch_sub();
        epochs.ptr_stake_excluded = false;

        rules
            .on_era_transition(
                Era::Babbage,
                &ctx,
                &mut utxo,
                &mut certs,
                &mut gov,
                &mut consensus,
                &mut epochs,
            )
            .expect("era transition should succeed");

        // Verify committee members.
        assert_eq!(gov.governance.committee_expiration.len(), 2);
        let c1 = Hash32::from_bytes(cred1_bytes);
        let c2 = Hash32::from_bytes(cred2_bytes);
        assert_eq!(
            gov.governance.committee_expiration.get(&c1),
            Some(&EpochNo(200))
        );
        assert_eq!(
            gov.governance.committee_expiration.get(&c2),
            Some(&EpochNo(300))
        );

        // Verify threshold.
        let threshold = gov
            .governance
            .committee_threshold
            .as_ref()
            .expect("threshold must be set");
        assert_eq!(threshold.numerator, 2);
        assert_eq!(threshold.denominator, 3);
    }

    /// Verify constitution is set from ConwayGenesis.
    #[test]
    fn test_on_era_transition_seeds_constitution() {
        use crate::eras::ConwayGenesisInit;
        use dugite_primitives::transaction::Constitution;

        let rules = ConwayRules::new();
        let params = ProtocolParameters::mainnet_defaults();
        let constitution = Constitution {
            anchor: Anchor {
                url: "https://constitution.example.com".to_string(),
                data_hash: Hash32::from_bytes([0xAB; 32]),
            },
            script_hash: None,
        };
        let genesis = ConwayGenesisInit {
            constitution: Some(constitution.clone()),
            ..Default::default()
        };
        let delegates = Box::leak(Box::new(HashMap::new()));
        let ctx = RuleContext {
            params: &params,
            current_slot: 100,
            current_epoch: EpochNo(5),
            era: Era::Conway,
            slot_config: None,
            node_network: None,
            genesis_delegates: delegates,
            update_quorum: 5,
            epoch_length: 432000,
            shelley_transition_epoch: 0,
            byron_epoch_length: 21600,
            stability_window: 129600,
            stability_window_3kf: 129600,
            randomness_stabilisation_window: 129600,
            tx_index: 0,
            conway_genesis: Some(&genesis),
        };

        let mut utxo = make_utxo_sub(vec![]);
        let mut certs = make_cert_sub();
        let mut gov = make_gov_sub();
        let mut consensus = make_consensus_sub();
        let mut epochs = make_epoch_sub();
        epochs.ptr_stake_excluded = false;

        rules
            .on_era_transition(
                Era::Babbage,
                &ctx,
                &mut utxo,
                &mut certs,
                &mut gov,
                &mut consensus,
                &mut epochs,
            )
            .expect("era transition should succeed");

        let stored = gov
            .governance
            .constitution
            .as_ref()
            .expect("constitution must be set");
        assert_eq!(stored.anchor.url, "https://constitution.example.com");
        assert_eq!(stored.anchor.data_hash, Hash32::from_bytes([0xAB; 32]));
    }

    /// Verify that `None` conway_genesis doesn't panic and is a no-op for governance.
    #[test]
    fn test_on_era_transition_no_genesis_is_noop() {
        let rules = ConwayRules::new();
        let params = ProtocolParameters::mainnet_defaults();
        let ctx = make_conway_ctx(&params); // conway_genesis = None

        let mut utxo = make_utxo_sub(vec![]);
        let mut certs = make_cert_sub();
        let mut gov = make_gov_sub();
        let mut consensus = make_consensus_sub();
        let mut epochs = make_epoch_sub();
        epochs.ptr_stake_excluded = false;

        rules
            .on_era_transition(
                Era::Babbage,
                &ctx,
                &mut utxo,
                &mut certs,
                &mut gov,
                &mut consensus,
                &mut epochs,
            )
            .expect("era transition with no genesis should succeed");

        // Governance should remain empty.
        assert!(gov.governance.dreps.is_empty());
        assert!(gov.governance.committee_expiration.is_empty());
        assert!(gov.governance.committee_threshold.is_none());
        assert!(gov.governance.constitution.is_none());

        // But steps 1 and 5 should still apply.
        assert!(epochs.ptr_stake_excluded);
        assert_eq!(utxo.pending_donations.0, 0);
    }
}
