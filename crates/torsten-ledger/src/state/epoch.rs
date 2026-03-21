use super::{stake_credential_hash, LedgerState, StakeSnapshot};
use std::collections::HashMap;
use std::sync::Arc;
use torsten_primitives::hash::Hash32;
use torsten_primitives::time::EpochNo;
use torsten_primitives::value::Lovelace;
use tracing::{debug, info, warn};

impl LedgerState {
    /// Process an epoch transition.
    ///
    /// Matches Haskell's NEWEPOCH STS rule ordering (Conway/Rules/NewEpoch.hs):
    ///
    /// 1. Complete and apply the pending RUPD (computed during the previous epoch)
    ///    - applyRUpd: treasury += deltaT, reserves += deltaR, rewards += rs
    ///    - deltaF is subtracted from accumulated fees (clears fees used by RUPD)
    /// 2. Run EPOCH sub-rules:
    ///    a. SNAP: rotate mark→set→go, capture ss_fee from accumulated fees
    ///    b. POOLREAP: process pending pool retirements
    ///    c. RATIFY: governance ratification
    /// 3. Reset block counters: nesBprev = nesBcur, nesBcur = empty
    /// 4. nesRu = SNothing (RUPD for next epoch starts fresh)
    ///
    /// The RUPD computed DURING epoch E uses:
    ///   - `ssStakeGo`: the GO snapshot (epoch E-2 stake data)
    ///   - `ss_fee`: fees captured by SNAP at the E-1→E boundary
    ///   - `nesBprev`: blocks from epoch E-1
    ///
    /// At genesis all snapshots are empty (Haskell's emptySnapShots).
    /// The first RUPD fires during epoch 0 with empty GO, ss_fee=0, empty blocks.
    /// With d>=0.8: eta=1, expansion = rho*reserves, treasury gets tau*expansion.
    /// No pool rewards (empty GO has no pools). Applied at 0→1 boundary.
    pub fn process_epoch_transition(&mut self, new_epoch: EpochNo) {
        debug!("Epoch transition: {} -> {}", self.epoch.0, new_epoch.0);

        // Capture bprev (nesBprev = nesBcur) BEFORE any param updates.
        // The d value for overlay checking must be from the epoch that JUST
        // ended — using the params that were in effect during that epoch.
        // PPUP hasn't fired yet, so self.protocol_params still has the old values.
        let d_for_bprev = if self.protocol_params.protocol_version_major >= 7 {
            0.0
        } else {
            let d_n = self.protocol_params.d.numerator as f64;
            let d_d = self.protocol_params.d.denominator.max(1) as f64;
            d_n / d_d
        };
        debug!(
            epoch = self.epoch.0,
            new_epoch = new_epoch.0,
            d_for_bprev,
            proto = self.protocol_params.protocol_version_major,
            raw_block_count = self.epoch_block_count,
            raw_pools = self.epoch_blocks_by_pool.len(),
            d_num = self.protocol_params.d.numerator,
            d_den = self.protocol_params.d.denominator,
            "bprev capture"
        );
        let (bprev_block_count, bprev_blocks_by_pool) = if d_for_bprev >= 0.8 {
            (0u64, Arc::new(HashMap::new()))
        } else {
            (
                self.epoch_block_count,
                Arc::clone(&self.epoch_blocks_by_pool),
            )
        };

        // Step 1: Apply any pending reward update (backward compat for old snapshots).
        self.apply_pending_reward_update();

        // Step 2a: Compute and apply the RUPD that was "pulsed" during the epoch
        // that just ended. In Haskell this is completed by the TICK rule and applied
        // here in NEWEPOCH. We compute it now using the CURRENT GO snapshot and
        // ss_fee — these represent what Haskell's mid-epoch RUPD would have used.
        //
        // GO snapshot: installed at the PREVIOUS epoch boundary, contains data from
        // 2 epochs ago. At genesis, GO is a valid empty snapshot (not None).
        //
        // ss_fee: captured by SNAP at the previous boundary, contains fees from the
        // epoch before the one that just ended. At genesis, ss_fee = 0.
        {
            // Haskell's startStep uses THREE separate data sources:
            //   1. ssStakeGo: stake/pool/delegation data (2 epochs ago)
            //   2. nesBprev (BlocksMade): block production from previous epoch
            //   3. ssFee: fees captured by SNAP at previous boundary
            //
            // GO provides stake distribution. SET provides block counts
            // (SET = old mark = epoch that just ended = nesBprev equivalent).
            let go_snapshot = self
                .snapshots
                .go
                .clone()
                .unwrap_or_else(|| StakeSnapshot::empty(EpochNo(0)));
            // Block counts come from bprev (= nesBprev = previous epoch's blocks).
            // This is separate from the snapshot rotation — bprev is captured
            // at each boundary (nesBprev = nesBcur, step 7 in NEWEPOCH).
            let bprev = StakeSnapshot {
                epoch_block_count: self.snapshots.bprev_block_count,
                epoch_blocks_by_pool: Arc::clone(&self.snapshots.bprev_blocks_by_pool),
                ..StakeSnapshot::empty(EpochNo(0))
            };
            let rupd = self.calculate_rewards_full(&go_snapshot, &bprev, self.snapshots.ss_fee);
            self.reserves.0 = self.reserves.0.saturating_sub(rupd.delta_reserves);
            self.treasury.0 = self.treasury.0.saturating_add(rupd.delta_treasury);
            let mut total_applied = 0u64;
            for (cred_hash, reward) in &rupd.rewards {
                if reward.0 > 0 {
                    *Arc::make_mut(&mut self.reward_accounts)
                        .entry(*cred_hash)
                        .or_insert(Lovelace(0)) += *reward;
                    total_applied += reward.0;
                }
            }
            if rupd.delta_treasury > 0 || total_applied > 0 {
                debug!(
                    epoch = new_epoch.0,
                    accounts = rupd.rewards.len(),
                    total_applied,
                    treasury_delta = rupd.delta_treasury,
                    reserves_delta = rupd.delta_reserves,
                    ss_fee = self.snapshots.ss_fee.0,
                    "RUPD applied using GO snapshot + ss_fee"
                );
            }
        }

        // Step 2b: SNAP — rotate snapshots, capture fees, update bprev.
        //
        // In Haskell NEWEPOCH ordering:
        //   1. applyRUpd (done above)
        //   2. SNAP: rotate mark→set→go, capture ssFee
        //   7. nesBprev = nesBcur, nesBcur = empty
        //
        // We capture bprev BEFORE resetting the counters (step 7 equivalent).
        // bprev = current epoch's block production (matching nesBcur → nesBprev).
        let captured_fees = self.epoch_fees;
        self.snapshots.go = self.snapshots.set.take();
        self.snapshots.set = self.snapshots.mark.take();
        self.snapshots.ss_fee = captured_fees;
        // bprev = blocks from the epoch that just ended (nesBprev = nesBcur).
        // The overlay check uses the d value from the epoch that just ended
        // (captured BEFORE PPUP updates the protocol params). This was saved
        // at the top of process_epoch_transition.
        self.snapshots.bprev_block_count = bprev_block_count;
        self.snapshots.bprev_blocks_by_pool = bprev_blocks_by_pool;
        debug!(
            epoch = new_epoch.0,
            bprev_blocks = self.snapshots.bprev_block_count,
            bprev_pools = self.snapshots.bprev_blocks_by_pool.len(),
            d_for_bprev,
            "bprev updated"
        );

        // Rebuild stake distribution from the full UTxO set at epoch boundaries.
        // During fast replay (needs_stake_rebuild=false), skip the expensive full
        // UTxO scan. During live sync (needs_stake_rebuild=true, the default),
        // always rebuild to ensure correctness and prevent incremental drift.
        // Note: needs_stake_rebuild stays true once set — every live epoch boundary rebuilds.
        if self.needs_stake_rebuild {
            self.rebuild_stake_distribution();
        }

        // Per Cardano spec, total stake = UTxO-delegated stake + reward account balance.
        // Pre-size to the number of distinct pools (upper-bounded by delegations).
        let mut pool_stake: HashMap<torsten_primitives::hash::Hash28, Lovelace> =
            HashMap::with_capacity(self.pool_params.len());
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

        // Build per-credential stake including reward balances. Clone the
        // stake_map as the base and fold in reward account balances. The clone
        // already carries the correct capacity from the original map.
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
            // Block production data in the mark is used for legacy calculate_rewards().
            // The primary RUPD path uses bprev (from EpochSnapshots) instead.
            epoch_fees: self.epoch_fees,
            epoch_block_count: self.epoch_block_count,
            epoch_blocks_by_pool: Arc::clone(&self.epoch_blocks_by_pool),
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

        // NOTE: prev_protocol_version_major is updated at the END of this
        // function (after PPUP), capturing the curPP for the current epoch.
        // The RUPD at the NEXT boundary will use it as prevPP.

        // Apply pre-Conway protocol parameter update proposals (PPUP/UPEC rule).
        //
        // In Haskell, proposals targeting epoch E are evaluated at the (E-1)→E
        // boundary by the UPEC rule (within NEWEPOCH). The updated params become
        // `curPParams` for epoch E. Proposals are in `sgsCurProposals` which
        // were promoted from `sgsFutureProposals` at the previous boundary.
        //
        // Proposals targeting epoch N are applied at the N→N+1 boundary.
        // On preview, proposals targeting epoch 1 (submitted in epoch 0) are
        // applied at the 1→2 boundary. self.epoch still holds the old value
        // (N) at this point — it's updated at the end of the transition.
        //
        // NOTE: There's a remaining timing issue with proposals in the
        // transition-triggering block (see #231). For now we use new_epoch-1
        // which is equivalent to self.epoch.
        // Try both the current epoch and the new epoch for proposals.
        // On preview, proposals targeting epoch 1 are first processed at the
        // 1→2 boundary (self.epoch=1), and proposals targeting epoch 2 at
        // the 2→3 boundary (self.epoch=2).
        let ppup_epoch = self.epoch;
        if let Some(proposals) = self.pending_pp_updates.remove(&ppup_epoch) {
            // Count distinct proposers (genesis delegate hashes)
            let mut proposer_set: std::collections::HashSet<Hash32> =
                std::collections::HashSet::with_capacity(proposals.len());
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
                    // d is handled via protocol_version check (proto >= 7 → d=0).
                    // Merging d from PPUP breaks epoch 3 because prevPParams timing
                    // causes eta=0 when bprev is empty. The proto 7 upgrade naturally
                    // sets d=0 at the 2→3 boundary, which matches the observed Koios
                    // R+T pattern (full expansion through epoch 3).
                    // merge_field!(d);
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

        // Compute new epoch nonce per Haskell TICKN rule:
        //
        //   TRC (TicknEnv extraEntropy ηc ηph, TicknState _ ηh, newEpoch)
        //   epochNonce'    = ηc ⭒ ηh ⭒ extraEntropy   (uses OLD prevHashNonce)
        //   prevHashNonce' = ηph                         (THEN updates to current labNonce)
        //
        // Critical: use the OLD last_epoch_block_nonce FIRST, then update it.
        // In Haskell ηh = previous ticknStatePrevHashNonce (NOT the just-captured lab).
        let _prev_epoch_nonce = self.epoch_nonce;
        let candidate = self.candidate_nonce;
        let prev_hash_nonce = self.last_epoch_block_nonce; // ηh = OLD value

        debug!(
            epoch = new_epoch.0,
            candidate = %candidate.to_hex(),
            prev_hash_nonce = %prev_hash_nonce.to_hex(),
            block_count = self.epoch_block_count,
            "Epoch nonce inputs"
        );

        // Step 1: Compute new epoch nonce using OLD prevHashNonce (ηh).
        // Uses Haskell's Nonce combine (⭒) with NeutralNonce (ZERO) as identity:
        //   epochNonce = candidate ⭒ prevHashNonce ⭒ extraEntropy
        //   NeutralNonce ⭒ x = x;  x ⭒ NeutralNonce = x
        //   Nonce(a) ⭒ Nonce(b) = Nonce(blake2b_256(a || b))
        // extraEntropy is NeutralNonce on all real networks, so omitted.
        let zero = torsten_primitives::hash::Hash32::ZERO;
        self.epoch_nonce = if candidate == zero && prev_hash_nonce == zero {
            zero
        } else if candidate == zero {
            prev_hash_nonce
        } else if prev_hash_nonce == zero {
            candidate // identity: candidate ⭒ NeutralNonce = candidate
        } else {
            let mut nonce_input = Vec::with_capacity(64);
            nonce_input.extend_from_slice(candidate.as_bytes());
            nonce_input.extend_from_slice(prev_hash_nonce.as_bytes());
            torsten_primitives::hash::blake2b_256(&nonce_input)
        };

        // Step 2: NOW update prevHashNonce to current labNonce for NEXT epoch
        self.last_epoch_block_nonce = self.lab_nonce;

        debug!(
            epoch = new_epoch.0,
            nonce = %self.epoch_nonce.to_hex(),
            "Epoch nonce"
        );

        // evolving_nonce and candidate_nonce carry forward unchanged
        // (they are NOT reset at epoch boundaries)

        // Capture prevPParams AFTER PPUP has updated curPP.
        // In Haskell NEWEPOCH: prevPParams = old curPParams (before UPEC).
        // But UPEC fires during this boundary, updating curPP.
        // After UPEC: prevPP = old curPP, curPP = new (from UPEC).
        // The next epoch's RUPD will use prevPP (this value).
        // By capturing AFTER PPUP, we get curPP for the new epoch,
        // which the NEXT epoch's RUPD will see as prevPP.
        self.prev_protocol_version_major = self.protocol_params.protocol_version_major;

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
        // Pre-size to the current credential count to minimise rehashing.
        let mut new_map: HashMap<Hash32, Lovelace> =
            HashMap::with_capacity(self.stake_distribution.stake_map.len());
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
    /// each snapshot's pool_stake map using the current (rebuilt) stake
    /// distribution and reward accounts.  Crucially, each snapshot's own
    /// delegation map is preserved — it reflects the delegations that were
    /// active at the time the snapshot epoch boundary was crossed.  Replacing
    /// snapshot delegations with the current map would incorrectly include
    /// delegations that became active AFTER the snapshot epoch, producing
    /// inflated sigma values for historical snapshots (issue #171).
    ///
    /// The zero-stake bug (issue #113) is handled separately by the
    /// `rebuild_stake_distribution` + `recompute_snapshot_pool_stakes` call
    /// in node startup code after the UTxO store is attached.
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
                    HashMap::with_capacity(snap.pool_stake.len());
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
        // Combines evolving nonce with a pre-computed 32-byte eta:
        //   evolving' = blake2b_256(evolving || eta)
        //
        // The caller (serialization/multi_era.rs) always provides a pre-computed 32-byte eta:
        //   TPraos (Shelley-Alonzo): eta = blake2b_256(nonce_vrf.0)   — one hash of raw 64-byte VRF
        //   Praos  (Babbage/Conway): eta = blake2b_256("N"||vrf.0)    — one hash of tagged VRF
        //
        // This function does NOT add any extra hashing — the input IS the eta.
        // This matches Haskell's updateNonce: hash(evolving || eta)
        // where eta was already computed as vrfNonceValue in reupdateChainDepState.
        let prev = self.evolving_nonce;
        // ALWAYS hash the input — matching pallas's generate_rolling_nonce exactly.
        // DO NOT use a pass-through for 32-byte inputs — this was verified to produce
        // wrong nonces. The hash step is required for both TPraos and Praos:
        //   TPraos (64-byte raw nonce_vrf.0): eta = blake2b_256(raw) — 1 hash total
        //   Praos  (32-byte nonce_vrf_output): eta = blake2b_256(tagged_hash) — 2nd hash
        let eta_hash = torsten_primitives::hash::blake2b_256(nonce_eta);
        let mut data = Vec::with_capacity(64);
        data.extend_from_slice(self.evolving_nonce.as_bytes());
        data.extend_from_slice(eta_hash.as_bytes());
        self.evolving_nonce = torsten_primitives::hash::blake2b_256(&data);

        let _ = prev; // suppress unused warning in release
    }
}
