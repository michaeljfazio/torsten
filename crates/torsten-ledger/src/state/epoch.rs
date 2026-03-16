use super::{stake_credential_hash, LedgerState, StakeSnapshot};
use std::collections::HashMap;
use std::sync::Arc;
use torsten_primitives::hash::Hash32;
use torsten_primitives::time::EpochNo;
use torsten_primitives::value::Lovelace;
use tracing::{debug, info, warn};

impl LedgerState {
    /// Process an epoch transition
    ///
    /// Follows Haskell's NEWEPOCH STS rule ordering:
    /// 1. Apply pending reward update from the PREVIOUS epoch transition (RUPD)
    /// 2. Rotate snapshots: go = set, set = mark, mark = new
    /// 3. Compute new reward update using go snapshot -> store as pending
    /// 4. Process retirements, governance, nonce, etc.
    pub fn process_epoch_transition(&mut self, new_epoch: EpochNo) {
        debug!("Epoch transition: {} -> {}", self.epoch.0, new_epoch.0);

        // Step 1: Apply the pending reward update from the PREVIOUS epoch boundary.
        // This matches Haskell's RUPD deferred application: rewards computed at
        // E-1 -> E are applied at E -> E+1.
        self.apply_pending_reward_update();

        // Step 3 (after snapshot rotation below): Compute new rewards using the
        // go snapshot and store them for deferred application at the NEXT boundary.
        // We capture the go snapshot before rotation consumes it.
        if let Some(ref go_snapshot) = self.snapshots.go {
            let rupd = self.calculate_rewards(go_snapshot);
            self.pending_reward_update = Some(rupd);
        }

        // Rotate snapshots: go = set, set = mark, mark = new snapshot
        self.snapshots.go = self.snapshots.set.take();
        self.snapshots.set = self.snapshots.mark.take();

        // Rebuild stake distribution from the full UTxO set at epoch boundaries.
        // During fast replay (needs_stake_rebuild=false), skip the expensive full
        // UTxO scan. During live sync (needs_stake_rebuild=true, the default),
        // always rebuild to ensure correctness and prevent incremental drift.
        // Note: needs_stake_rebuild stays true once set — every live epoch boundary rebuilds.
        if self.needs_stake_rebuild {
            self.rebuild_stake_distribution();
        }

        // Per Cardano spec, total stake = UTxO-delegated stake + reward account balance.
        let mut pool_stake: HashMap<torsten_primitives::hash::Hash28, Lovelace> = HashMap::new();
        for (cred_hash, pool_id) in self.delegations.iter() {
            let utxo_stake = self
                .stake_distribution
                .stake_map
                .get(cred_hash)
                .copied()
                .unwrap_or(Lovelace(0));
            let reward_balance = self
                .reward_accounts
                .get(cred_hash)
                .copied()
                .unwrap_or(Lovelace(0));
            let total_stake = Lovelace(utxo_stake.0 + reward_balance.0);
            *pool_stake.entry(*pool_id).or_insert(Lovelace(0)) += total_stake;
        }

        // Build per-credential stake including reward balances
        let mut snapshot_stake = self.stake_distribution.stake_map.clone();
        for (cred_hash, reward) in self.reward_accounts.iter() {
            if reward.0 > 0 {
                *snapshot_stake.entry(*cred_hash).or_insert(Lovelace(0)) += *reward;
            }
        }

        let total_utxo_stake: u64 = self
            .stake_distribution
            .stake_map
            .values()
            .fold(0u64, |acc, l| acc.saturating_add(l.0));
        let total_pool_stake: u64 = pool_stake
            .values()
            .fold(0u64, |acc, l| acc.saturating_add(l.0));
        debug!(
            epoch = new_epoch.0,
            credentials = self.stake_distribution.stake_map.len(),
            delegations = self.delegations.len(),
            pools = pool_stake.len(),
            total_utxo_stake_ada = total_utxo_stake / 1_000_000,
            total_pool_stake_ada = total_pool_stake / 1_000_000,
            "Epoch snapshot: stake distribution rebuilt from UTxO set"
        );

        self.snapshots.mark = Some(StakeSnapshot {
            epoch: new_epoch,
            delegations: Arc::clone(&self.delegations),
            pool_stake,
            pool_params: Arc::clone(&self.pool_params),
            stake_distribution: Arc::new(snapshot_stake),
        });

        // Process pending pool retirements for this epoch
        if let Some(retiring_pools) = self.pending_retirements.remove(&new_epoch) {
            let pool_deposit = self.protocol_params.pool_deposit;
            for pool_id in &retiring_pools {
                // Refund pool deposit to operator's registered reward account
                if let Some(pool_reg) = Arc::make_mut(&mut self.pool_params).remove(pool_id) {
                    let op_key = Self::reward_account_to_hash(&pool_reg.reward_account);
                    *Arc::make_mut(&mut self.reward_accounts)
                        .entry(op_key)
                        .or_insert(Lovelace(0)) += pool_deposit;
                    debug!(
                        "Pool retired at epoch {}: {} (deposit {} refunded)",
                        new_epoch.0,
                        pool_id.to_hex(),
                        pool_deposit.0
                    );
                } else {
                    debug!(
                        "Pool retired at epoch {}: {} (no params found)",
                        new_epoch.0,
                        pool_id.to_hex()
                    );
                }
            }
        }

        // Clean up retirements from past epochs (shouldn't happen but be safe)
        self.pending_retirements
            .retain(|epoch, _| *epoch >= new_epoch);

        // Apply pre-Conway protocol parameter update proposals (PPUP rule).
        // In Shelley-Babbage, genesis delegates submit update proposals targeting epoch E.
        // At the epoch boundary E -> E+1, proposals targeting E are evaluated:
        // if enough distinct genesis delegates proposed updates (>= update_quorum),
        // their proposals are merged and applied to take effect in epoch E+1.
        // Note: self.epoch still holds the OLD epoch at this point (updated at end).
        if let Some(proposals) = self.pending_pp_updates.remove(&self.epoch) {
            // Count distinct proposers (genesis delegate hashes)
            let mut proposer_set: std::collections::HashSet<Hash32> =
                std::collections::HashSet::new();
            for (genesis_hash, _) in &proposals {
                proposer_set.insert(*genesis_hash);
            }
            let distinct_proposers = proposer_set.len() as u64;

            if distinct_proposers >= self.update_quorum {
                // Merge all proposals: later proposals override earlier ones per field
                let mut merged = torsten_primitives::transaction::ProtocolParamUpdate::default();
                for (_, ppu) in &proposals {
                    // Merge each field: if the proposal sets it, override
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
                // Log protocol version change if applicable
                if merged.protocol_version_major.is_some()
                    || merged.protocol_version_minor.is_some()
                {
                    info!(
                        "Protocol     version change {}.{} -> {}.{} (epoch {})",
                        self.protocol_params.protocol_version_major,
                        self.protocol_params.protocol_version_minor,
                        merged
                            .protocol_version_major
                            .unwrap_or(self.protocol_params.protocol_version_major),
                        merged
                            .protocol_version_minor
                            .unwrap_or(self.protocol_params.protocol_version_minor),
                        new_epoch.0,
                    );
                }
                if let Err(e) = self.apply_protocol_param_update(&merged) {
                    warn!(
                        epoch = new_epoch.0,
                        error = %e,
                        "Pre-Conway protocol parameter update rejected"
                    );
                } else {
                    debug!(
                        epoch = new_epoch.0,
                        proposers = distinct_proposers,
                        "Pre-Conway protocol parameter update applied"
                    );
                }
            } else {
                debug!(
                    epoch = new_epoch.0,
                    proposers = distinct_proposers,
                    quorum = self.update_quorum,
                    "Pre-Conway protocol parameter update: insufficient quorum"
                );
            }
        }
        // Clean up proposals targeting past epochs (already applied above).
        // Keep proposals targeting new_epoch or later — they'll be applied at
        // the NEXT epoch boundary (new_epoch -> new_epoch+1).
        self.pending_pp_updates
            .retain(|epoch, _| *epoch >= new_epoch);

        // Ratify governance proposals that have met their voting thresholds
        self.ratify_proposals();

        // Expire governance proposals that have passed their lifetime
        // and refund deposits to the return address
        let expired: Vec<torsten_primitives::transaction::GovActionId> = self
            .governance
            .proposals
            .iter()
            // Per Haskell: `gasExpiresAfter < reCurrentEpoch` — proposals are active
            // through their expires_epoch and expire at the NEXT epoch boundary.
            .filter(|(_, state)| state.expires_epoch < new_epoch)
            .map(|(id, _)| id.clone())
            .collect();
        if !expired.is_empty() {
            for action_id in &expired {
                if let Some(proposal_state) = Arc::make_mut(&mut self.governance)
                    .proposals
                    .remove(action_id)
                {
                    // Refund deposit to return address's reward account
                    let deposit = proposal_state.procedure.deposit;
                    if deposit.0 > 0 {
                        let return_addr = &proposal_state.procedure.return_addr;
                        if return_addr.len() >= 29 {
                            let key = Self::reward_account_to_hash(return_addr);
                            *Arc::make_mut(&mut self.reward_accounts)
                                .entry(key)
                                .or_insert(Lovelace(0)) += deposit;
                        }
                    }
                    debug!(
                        "Governance proposal expired: {:?} (deposit {} returned)",
                        action_id, deposit.0
                    );
                }
            }
            // Remove all votes for expired proposals
            for id in &expired {
                Arc::make_mut(&mut self.governance)
                    .votes_by_action
                    .remove(id);
            }
            debug!(
                "Expired {} governance proposals at epoch {}",
                expired.len(),
                new_epoch.0
            );
        }

        // Store expired proposal IDs for GetRatifyState query (tag 32).
        // This is set regardless of whether proposals expired (clears stale data).
        Arc::make_mut(&mut self.governance).last_expired = expired;

        // Mark inactive DReps per CIP-1694
        // DReps that haven't voted or updated within drep_activity epochs are marked inactive
        // and excluded from voting power calculations. They remain registered and keep their deposits.
        let drep_activity = self.protocol_params.drep_activity;
        if drep_activity > 0 {
            let mut newly_inactive = 0u64;
            let mut reactivated = 0u64;
            for drep in Arc::make_mut(&mut self.governance).dreps.values_mut() {
                let inactive = new_epoch.0.saturating_sub(drep.last_active_epoch.0) > drep_activity;
                if inactive && drep.active {
                    drep.active = false;
                    newly_inactive += 1;
                } else if !inactive && !drep.active {
                    drep.active = true;
                    reactivated += 1;
                }
            }
            if newly_inactive > 0 || reactivated > 0 {
                debug!(
                    "DRep activity update at epoch {}: {} newly inactive, {} reactivated (threshold: {} epochs)",
                    new_epoch.0,
                    newly_inactive,
                    reactivated,
                    drep_activity
                );
            }
        }

        // Expire committee members that have passed their expiration epoch
        let expired_members: Vec<Hash32> = self
            .governance
            .committee_expiration
            .iter()
            .filter(|(_, exp_epoch)| **exp_epoch <= new_epoch)
            .map(|(hash, _)| *hash)
            .collect();
        if !expired_members.is_empty() {
            for hash in &expired_members {
                Arc::make_mut(&mut self.governance)
                    .committee_hot_keys
                    .remove(hash);
                Arc::make_mut(&mut self.governance)
                    .committee_expiration
                    .remove(hash);
            }
            debug!(
                "Expired {} committee members at epoch {}",
                expired_members.len(),
                new_epoch.0
            );
        }

        // Compute new epoch nonce per Haskell tickChainDepState:
        //   epoch_nonce = hash(candidate_nonce || last_epoch_block_nonce)
        //
        // The candidate_nonce was frozen 4k/f slots before epoch end.
        // The last_epoch_block_nonce is the lab_nonce snapshot from the previous epoch boundary.
        let prev_epoch_nonce = self.epoch_nonce;
        self.last_epoch_block_nonce = self.lab_nonce;

        let mut nonce_input = Vec::with_capacity(64);
        nonce_input.extend_from_slice(self.candidate_nonce.as_bytes());
        nonce_input.extend_from_slice(self.last_epoch_block_nonce.as_bytes());
        self.epoch_nonce = torsten_primitives::hash::blake2b_256(&nonce_input);

        debug!(
            "New epoch nonce: {} (candidate {} \u{22c4} lab {}), prev: {}",
            self.epoch_nonce.to_hex(),
            self.candidate_nonce.to_hex(),
            self.last_epoch_block_nonce.to_hex(),
            prev_epoch_nonce.to_hex(),
        );

        // evolving_nonce and candidate_nonce carry forward unchanged
        // (they are NOT reset at epoch boundaries)

        // Reset per-epoch accumulators
        self.epoch_fees = Lovelace(0);
        Arc::make_mut(&mut self.epoch_blocks_by_pool).clear();
        self.epoch_block_count = 0;

        self.epoch = new_epoch;
    }

    /// Rebuild stake_distribution.stake_map from the full UTxO set.
    ///
    /// This recomputes per-credential UTxO stake by scanning all UTxOs,
    /// matching Haskell's behavior at epoch boundaries. This corrects any
    /// drift from incremental tracking (e.g., after snapshot load or Mithril import).
    pub fn rebuild_stake_distribution(&mut self) {
        let mut new_map: HashMap<Hash32, Lovelace> = HashMap::new();
        for (_, output) in self.utxo_set.iter() {
            if let Some(cred_hash) = stake_credential_hash(&output.address) {
                *new_map.entry(cred_hash).or_insert(Lovelace(0)) += Lovelace(output.value.coin.0);
            }
        }
        // Also ensure all registered stake credentials have entries (even with 0 stake)
        for cred_hash in self.delegations.keys() {
            new_map.entry(*cred_hash).or_insert(Lovelace(0));
        }
        self.stake_distribution.stake_map = new_map;
    }

    /// Recompute pool_stake for all existing snapshots (mark/set/go).
    ///
    /// After rebuilding stake_distribution from the UTxO set, this updates
    /// each snapshot's pool_stake map using the snapshot's own delegations
    /// with the current (rebuilt) stake distribution and reward accounts.
    /// This corrects any drift in snapshot pool_stake values after snapshot load.
    pub fn recompute_snapshot_pool_stakes(&mut self) {
        for (name, snapshot) in [
            ("mark", &mut self.snapshots.mark),
            ("set", &mut self.snapshots.set),
            ("go", &mut self.snapshots.go),
        ] {
            if let Some(snap) = snapshot {
                let old_total: u64 = snap
                    .pool_stake
                    .values()
                    .fold(0u64, |acc, s| acc.saturating_add(s.0));
                let mut new_pool_stake: HashMap<torsten_primitives::hash::Hash28, Lovelace> =
                    HashMap::new();
                for (cred_hash, pool_id) in snap.delegations.iter() {
                    let utxo_stake = self
                        .stake_distribution
                        .stake_map
                        .get(cred_hash)
                        .copied()
                        .unwrap_or(Lovelace(0));
                    let reward_balance = self
                        .reward_accounts
                        .get(cred_hash)
                        .copied()
                        .unwrap_or(Lovelace(0));
                    let total_stake = Lovelace(utxo_stake.0.saturating_add(reward_balance.0));
                    *new_pool_stake.entry(*pool_id).or_insert(Lovelace(0)) += total_stake;
                }
                let new_total: u64 = new_pool_stake
                    .values()
                    .fold(0u64, |acc, s| acc.saturating_add(s.0));
                if old_total != new_total {
                    debug!(
                        snapshot = name,
                        epoch = snap.epoch.0,
                        old_total_ada = old_total / 1_000_000,
                        new_total_ada = new_total / 1_000_000,
                        delta_ada = (new_total as i128 - old_total as i128) / 1_000_000,
                        "Snapshot pool_stake recomputed (corrected drift)"
                    );
                }
                snap.pool_stake = new_pool_stake;
            }
        }
    }

    /// Update the evolving nonce with a pre-computed nonce VRF contribution (eta).
    ///
    /// evolving_nonce = blake2b_256(evolving_nonce || eta)
    ///
    /// The `nonce_eta` argument is the era-specific nonce contribution stored in
    /// `BlockHeader::nonce_vrf_output`:
    ///
    /// - Shelley/Allegra/Mary/Alonzo (TPraos): eta = blake2b_256(nonce_vrf_cert.0)
    /// - Babbage/Conway (Praos): eta = blake2b_256("N" || vrf_result.0)
    ///
    /// This function does NOT do any additional hashing of the input — the caller
    /// (serialization) is responsible for computing eta correctly per era.  This
    /// exactly matches Haskell's reupdateChainDepState:
    ///
    ///   eta = vrfNonceValue block
    ///   evolving_nonce' = updateNonce evolving_nonce eta
    ///   where updateNonce n e = hash (n <> e)
    pub(crate) fn update_evolving_nonce(&mut self, nonce_eta: &[u8]) {
        // evolving_nonce' = blake2b_256(evolving_nonce || eta)
        let mut data = Vec::with_capacity(64);
        data.extend_from_slice(self.evolving_nonce.as_bytes());
        data.extend_from_slice(nonce_eta);
        self.evolving_nonce = torsten_primitives::hash::blake2b_256(&data);
    }
}
