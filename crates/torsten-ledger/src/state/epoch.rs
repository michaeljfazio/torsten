use super::{stake_credential_hash_with_ptrs, LedgerState, StakeSnapshot};
use std::collections::HashMap;
use std::sync::Arc;
use torsten_primitives::hash::{Hash28, Hash32};
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
        //
        // In Haskell, `incrBlocks` checks `isOverlaySlot` for each block
        // DURING the epoch using curPParams.d. We don't do per-block overlay
        // checking — instead we count ALL blocks and rely on the fact that:
        // - When d >= 0.8 (overlay), genesis delegates produce blocks with
        //   key hashes that DON'T match any pool_id in the GO snapshot
        // - The pool-block guard in calculate_rewards skips pools without
        //   matching blocks in bprev
        // - So overlay-era blocks naturally produce no pool rewards
        //
        // This avoids the timing issue where the d value at boundary capture
        // time differs from the d value during the epoch (due to PPUP updates).
        let bprev_block_count = self.epoch_block_count;
        let bprev_blocks_by_pool = Arc::clone(&self.epoch_blocks_by_pool);

        // Step 0: Flush pending treasury donations accumulated during the epoch.
        //
        // In Haskell, `UTxOState.utxosDonation` (collected from `txDonation` fields
        // in each transaction) is transferred into the treasury as part of the
        // NEWEPOCH rule before reward computation (Haskell's `applyRUpd` receives
        // the post-donation treasury balance).
        //
        // We buffer donations in `pending_donations` during block application and
        // drain them here so the donation ADA is visible to reward-pot calculations
        // for this epoch boundary — matching Haskell's ordering exactly.
        if self.pending_donations.0 > 0 {
            let flushed = self.pending_donations;
            self.treasury.0 = self.treasury.0.saturating_add(flushed.0);
            self.pending_donations = Lovelace(0);
            debug!(
                epoch = new_epoch.0,
                donations_lovelace = flushed.0,
                "Flushed pending treasury donations to treasury at epoch boundary"
            );
        }

        // Step 1: Apply any pending reward update (backward compat for old snapshots).
        self.apply_pending_reward_update();

        // Step 2a: Compute and apply the RUPD that was "pulsed" during the epoch
        // that just ended. In Haskell this is completed by the TICK rule and applied
        // here in NEWEPOCH. We compute it now using the CURRENT GO snapshot and
        // ss_fee — these represent what Haskell's mid-epoch RUPD would have used.
        //
        // At genesis (first boundary, 0→1), Haskell's TICK rule still computes the
        // reward update during epoch 0: GO=empty, bprev=empty, ss_fee=0.  With no
        // pools in GO the monetary expansion (rho × reserves) still fires and the
        // treasury cut (tau × expansion) moves reserves → treasury.  No individual
        // rewards are distributed (no pools).  We must replicate this — skipping it
        // creates a permanent reserves/treasury offset.
        {
            // Haskell's startStep uses THREE separate data sources:
            //   1. ssStakeGo: stake/pool/delegation data (2 epochs ago)
            //   2. nesBprev (BlocksMade): block production from previous epoch
            //   3. ssFee: fees captured by SNAP at previous boundary
            //
            // GO provides stake distribution. Block counts come from bprev
            // (= nesBprev = previous epoch's blocks, separate from snapshot rotation).
            // At the very first boundary (0→1) all three are empty/zero, which is
            // correct: the RUPD yields pure monetary expansion with no pool rewards.
            let go_snapshot = self
                .snapshots
                .go
                .clone()
                .unwrap_or_else(|| StakeSnapshot::empty(EpochNo(0)));
            let bprev = StakeSnapshot {
                epoch_block_count: self.snapshots.bprev_block_count,
                epoch_blocks_by_pool: Arc::clone(&self.snapshots.bprev_blocks_by_pool),
                ..StakeSnapshot::empty(EpochNo(0))
            };
            let rupd = self.calculate_rewards_full(&go_snapshot, &bprev, self.snapshots.ss_fee);
            self.reserves.0 = self.reserves.0.saturating_sub(rupd.delta_reserves);
            self.treasury.0 = self.treasury.0.saturating_add(rupd.delta_treasury);

            // Apply per-account rewards, matching Haskell's applyRUpdFiltered:
            // rewards for REGISTERED credentials go to their reward accounts;
            // rewards for UNREGISTERED credentials are forwarded to treasury.
            // A credential is "registered" if it has an entry in reward_accounts
            // (created by StakeRegistration certificate processing).
            let mut total_applied = 0u64;
            let mut unregistered_total = 0u64;
            for (cred_hash, reward) in &rupd.rewards {
                if reward.0 > 0 {
                    if self.reward_accounts.contains_key(cred_hash) {
                        *Arc::make_mut(&mut self.reward_accounts)
                            .entry(*cred_hash)
                            .or_insert(Lovelace(0)) += *reward;
                        total_applied += reward.0;
                    } else {
                        // Unregistered credential: forward to treasury
                        // (matches Haskell's frTotalUnregistered in applyRUpd)
                        self.treasury.0 = self.treasury.0.saturating_add(reward.0);
                        unregistered_total += reward.0;
                    }
                }
            }
            if rupd.delta_treasury > 0 || total_applied > 0 || unregistered_total > 0 {
                debug!(
                    epoch = new_epoch.0,
                    accounts = rupd.rewards.len(),
                    total_applied,
                    unregistered_to_treasury = unregistered_total,
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
        // After the first rotation, bprev/ss_fee contain real epoch data.
        // Subsequent RUPD computations can fire (matching Haskell's nesRu = SJust).
        self.snapshots.rupd_ready = true;

        // Full UTxO rebuild is only needed after Mithril import or snapshot restore
        // (where incremental tracking was not active). During normal block processing
        // (replay or live sync), the incremental stake_map is accurate.
        if self.needs_stake_rebuild {
            self.rebuild_stake_distribution();
            // Once rebuilt, disable for subsequent boundaries — incremental is correct.
            self.needs_stake_rebuild = false;
        }

        // Conway pointer address exclusion (matching Haskell's ConwayInstantStake).
        //
        // In Haskell, the Conway era uses `ConwayInstantStake` which has NO `sisPtrStake`
        // field — pointer-addressed UTxOs are silently excluded from the stake distribution.
        // Pre-Conway eras use `ShelleyInstantStake` which resolves pointer addresses via
        // `saPtrs` during the SNAP rule.
        //
        // At the Conway HFC boundary, the migration from ShelleyInstantStake to
        // ConwayInstantStake discards the pointer map: only `sisCredentialStake` survives.
        // Existing pointer-addressed UTxOs remain in the UTxO set but their ADA no longer
        // flows into pool stake, reward calculations, or voting power.
        //
        // Torsten resolves pointer addresses inline during UTxO processing (not deferred
        // like Haskell), so the stake_map already includes pointer-addressed UTxO coins
        // under the resolved credential.  To match Conway semantics, we must subtract
        // pointer-addressed UTxO coins from the stake map starting at the first Conway epoch.
        if self.protocol_params.protocol_version_major >= 9 && !self.ptr_stake_excluded {
            self.exclude_pointer_address_stake();
            self.ptr_stake_excluded = true;
        }

        // Per Cardano spec, total stake = UTxO-delegated stake + reward account balance.
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

        // Build per-credential stake including reward balances for the snapshot.
        // Only include credentials that are in the delegation map, matching
        // Haskell's ssStake which is the intersection of staking credentials
        // and the delegation map.
        let mut snapshot_stake: HashMap<Hash32, Lovelace> =
            HashMap::with_capacity(self.delegations.len());
        for cred_hash in self.delegations.keys() {
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
            let total = Lovelace(utxo_stake.0.saturating_add(reward_balance.0));
            if total.0 > 0 {
                snapshot_stake.insert(*cred_hash, total);
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
            reward_accounts = self.reward_accounts.len(),
            delegations = self.delegations.len(),
            pool_params = self.pool_params.len(),
            pools_with_stake = pool_stake.len(),
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

        // Capture the DRep distribution snapshot from the current live state.
        //
        // Per Haskell `snapDRepDistr` in `Conway.Rules.Epoch`, the DRep voting power
        // used during ratification for the new epoch is measured against the stake
        // distribution at the *start* of that epoch (i.e. now, after the mark snapshot
        // is taken).  This snapshot is consumed by `build_drep_power_cache` during
        // `ratify_proposals` so that mid-epoch delegation changes do not affect
        // in-progress governance votes.
        if self.protocol_params.protocol_version_major >= 9 {
            self.capture_drep_distribution_snapshot();
        }

        // Apply future pool parameters (re-registrations deferred from previous epoch).
        //
        // In Haskell's POOLREAP, futurePoolParams are merged with psStakePools using
        // Map.merge with Map.dropMissing for future-only entries. This means:
        //   - Pools in BOTH future AND current: update params from future ✓
        //   - Pools ONLY in current: keep as-is ✓
        //   - Pools ONLY in future (e.g., pool retired between re-reg and boundary): DROPPED
        //
        // This prevents retired pools from being resurrected by stale futurePoolParams.
        if !self.future_pool_params.is_empty() {
            let mut applied = 0u64;
            let mut dropped = 0u64;
            let pool_params = Arc::make_mut(&mut self.pool_params);
            for (pool_id, pool_reg) in self.future_pool_params.drain() {
                #[allow(clippy::map_entry)] // intentional: different side-effects per branch
                if pool_params.contains_key(&pool_id) {
                    // Pool still registered: update with new params
                    pool_params.insert(pool_id, pool_reg);
                    applied += 1;
                } else {
                    // Pool no longer registered (retired): drop the future params
                    // Matches Haskell's Map.dropMissing behavior
                    debug!(
                        "Dropped future pool params for retired pool: {}",
                        pool_id.to_hex()
                    );
                    dropped += 1;
                }
            }
            if applied > 0 || dropped > 0 {
                debug!(
                    "Future pool params at epoch {}: {} applied, {} dropped (retired)",
                    new_epoch.0, applied, dropped
                );
            }
        }

        // Process pending pool retirements for this epoch.
        // Haskell: retired = {k | (k, v) <- psRetiring, v == e}
        let retiring_pools: Vec<Hash28> = self
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
            // Remove retired entries from pending_retirements
            for pool_id in &retiring_pools {
                self.pending_retirements.remove(pool_id);
            }
            let pool_deposit = self.protocol_params.pool_deposit;
            for pool_id in &retiring_pools {
                // Refund pool deposit to operator's registered reward account.
                // Also remove all delegations pointing to this pool, matching
                // Haskell's POOLREAP which filters delegations to retired pools:
                //   adjustedDelegs = Map.filter (\pid -> pid `Set.notMember` retired) delegs
                if let Some(pool_reg) = Arc::make_mut(&mut self.pool_params).remove(pool_id) {
                    let op_key = Self::reward_account_to_hash(&pool_reg.reward_account);
                    // Refund deposit: if the reward account is registered, refund to it.
                    // If unregistered, refund goes to treasury (matching Haskell's POOLREAP
                    // which filters unclaimed refunds to treasury).
                    if self.reward_accounts.contains_key(&op_key) {
                        *Arc::make_mut(&mut self.reward_accounts)
                            .entry(op_key)
                            .or_insert(Lovelace(0)) += pool_deposit;
                    } else {
                        self.treasury.0 = self.treasury.0.saturating_add(pool_deposit.0);
                        debug!(
                            "Pool {} deposit {} -> treasury (unregistered reward account)",
                            pool_id.to_hex(),
                            pool_deposit.0
                        );
                    }
                    // Remove delegations to the retired pool
                    Arc::make_mut(&mut self.delegations)
                        .retain(|_, delegated_pool| delegated_pool != pool_id);
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

        // Clean up retirements from past epochs (shouldn't happen but be safe).
        // Haskell: psRetiring = Map.filter (\e -> e > eNo) psRetiring
        self.pending_retirements
            .retain(|_, epoch| *epoch > new_epoch);

        // Capture prevPParams BEFORE PPUP updates curPP, matching Haskell's
        // NEWPP rule: prevPParams = old curPParams (before this boundary's PPUP).
        // The RUPD at the NEXT boundary will use prev_protocol_params for ALL
        // parameter values (rho, tau, a0, n_opt, active_slots_coeff, d, proto).
        let old_d = if self.protocol_params.protocol_version_major >= 7 {
            0.0
        } else {
            let d_n = self.protocol_params.d.numerator as f64;
            let d_d = self.protocol_params.d.denominator.max(1) as f64;
            d_n / d_d
        };
        let old_proto_major = self.protocol_params.protocol_version_major;
        let old_params = self.protocol_params.clone();

        // Apply pre-Conway protocol parameter update proposals (PPUP/UPEC rule).
        //
        // Haskell's PPUP uses a two-step promotion cycle:
        //   1. Proposals submitted during epoch E with ppupEpoch = E go to
        //      sgsCurProposals; with ppupEpoch = E+1 go to sgsFutureProposals
        //   2. At each NEWEPOCH boundary: UPEC evaluates sgsCurProposals (apply
        //      if quorum met), then updatePpup promotes sgsFuture → sgsCur
        //   3. Proposals targeting epoch N (ppupEpoch = N) are applied at the
        //      N→(N+1) boundary (new_epoch = N+1)
        //
        // Our simplified model: pending_pp_updates stores proposals under their
        // ppupEpoch as key. At boundary (new_epoch = N), we look up key = N-1
        // since proposals targeting epoch N-1 are applied at this boundary.
        //
        // This handles both Haskell cases:
        //   - ppupEpoch = E (= currentEpoch): sgsCur, applied at E→E+1 = N-1→N
        //   - ppupEpoch = E+1 (≠ currentEpoch): sgsFuture, promoted at E→E+1,
        //     applied at (E+1)→(E+2) = N-1→N when ppupEpoch = N-1
        let lookup_epoch = EpochNo(new_epoch.0.saturating_sub(1));
        debug!(
            new_epoch = new_epoch.0,
            lookup_epoch = lookup_epoch.0,
            pending = ?self.pending_pp_updates.keys().map(|e| e.0).collect::<Vec<_>>(),
            "PPUP: checking for proposals"
        );
        if let Some(proposals) = self.pending_pp_updates.remove(&lookup_epoch) {
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
        // Clean up current proposals targeting past epochs.
        self.pending_pp_updates
            .retain(|epoch, _| *epoch >= lookup_epoch);

        // Promote future proposals → current (Haskell's updatePpup: sgsCur = sgsFuture).
        // After evaluating sgsCur, future proposals become current for the next boundary.
        // This ensures proposals with ppupEpoch = E+1 (submitted during epoch E as future)
        // are promoted at E→E+1 and applied at E+1→E+2 — matching the two-step cycle.
        if !self.future_pp_updates.is_empty() {
            let promoted = std::mem::take(&mut self.future_pp_updates);
            for (epoch, proposals) in promoted {
                self.pending_pp_updates
                    .entry(epoch)
                    .or_default()
                    .extend(proposals);
            }
        }

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

        // Update dormant epoch counter per Haskell Conway.Rules.Epoch `updateNumDormantEpochs`.
        //
        // An epoch is "dormant" when there were no active governance proposals during it
        // (i.e. `proposals` is empty at the epoch boundary, AFTER ratification+expiry have
        // run and possibly consumed all proposals).  Dormant epochs are not counted against
        // DRep activity so that DReps are not incorrectly marked inactive during periods
        // when there was nothing to vote on.
        //
        // Haskell's ordering: check proposals AFTER ratification/expiry (so a last-epoch
        // proposal that just got ratified means this epoch was NOT dormant).
        {
            let gov = Arc::make_mut(&mut self.governance);
            if gov.proposals.is_empty() {
                gov.num_dormant_epochs = gov.num_dormant_epochs.saturating_add(1);
                debug!(
                    epoch = new_epoch.0,
                    num_dormant = gov.num_dormant_epochs,
                    "Governance: epoch is dormant (no active proposals) — incrementing dormant counter"
                );
            }
            // If there were active proposals this epoch, do NOT increment.
            // The counter never resets — it accumulates across the node's lifetime.
        }

        // Mark inactive DReps per CIP-1694
        //
        // A DRep is inactive when the number of non-dormant epochs since their last
        // activity exceeds the drep_activity threshold:
        //
        //   elapsed = new_epoch - last_active_epoch
        //   active_elapsed = elapsed - num_dormant_epochs_since_last_vote
        //   inactive = active_elapsed > drep_activity
        //
        // Because num_dormant_epochs is a global cumulative counter, we compute
        // active_elapsed as: (new_epoch - last_active_epoch) - num_dormant_epochs.
        // This may overcorrect if many dormant epochs occurred BEFORE last_active_epoch,
        // but matches Haskell's `vsNumDormantEpochs` semantics which is also global.
        //
        // DReps remain registered but are excluded from voting power calculations.
        let drep_activity = self.protocol_params.drep_activity;
        if drep_activity > 0 {
            let num_dormant = self.governance.num_dormant_epochs;
            let mut newly_inactive = 0u64;
            let mut reactivated = 0u64;
            for drep in Arc::make_mut(&mut self.governance).dreps.values_mut() {
                // Compute epochs elapsed since last activity, discounting dormant epochs.
                let elapsed = new_epoch.0.saturating_sub(drep.last_active_epoch.0);
                let active_elapsed = elapsed.saturating_sub(num_dormant);
                let inactive = active_elapsed > drep_activity;
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
                    "DRep activity update at epoch {}: {} newly inactive, {} reactivated \
                     (threshold: {} epochs, dormant: {})",
                    new_epoch.0, newly_inactive, reactivated, drep_activity, num_dormant
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

        // Capture the ratification snapshot for the NEXT epoch boundary.
        //
        // Per Haskell `setFreshDRepPulsingState`, the pulser is created from the
        // post-transition state — after ratification/expiry have pruned proposals,
        // enacted roots have been updated, DRep activity has been updated, and
        // committee members have been expired.  This snapshot will be consumed by
        // `ratify_proposals()` at the NEXT epoch boundary so that proposals/votes
        // submitted during the new epoch are not considered until then.
        if self.protocol_params.protocol_version_major >= 9 {
            self.capture_ratification_snapshot();
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

        // Set prevPParams from values captured BEFORE PPUP.
        // Haskell's NEWPP: prevPParams = old curPParams (before this PPUP).
        // The RUPD at the NEXT boundary uses prev_protocol_params for ALL
        // parameter values (rho, tau, a0, n_opt, active_slots_coeff, etc.).
        self.prev_d = old_d;
        self.prev_protocol_version_major = old_proto_major;
        self.prev_protocol_params = old_params;

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
            if let Some(cred_hash) = stake_credential_hash_with_ptrs(
                &output.address,
                &self.pointer_map,
                self.ptr_stake_excluded,
            ) {
                *new_map.entry(cred_hash).or_insert(Lovelace(0)) += Lovelace(output.value.coin.0);
            }
        }
        // Also ensure all registered stake credentials have entries (even with 0 stake)
        for cred_hash in self.delegations.keys() {
            new_map.entry(*cred_hash).or_insert(Lovelace(0));
        }
        self.stake_distribution.stake_map = new_map;
    }

    /// Exclude pointer-addressed UTxO stake from the incremental stake distribution.
    ///
    /// In Conway (protocol version >= 9), Haskell's `ConwayInstantStake` has no pointer
    /// map (`sisPtrStake`).  Pointer-addressed UTxOs remain in the UTxO set but their
    /// ADA no longer flows into pool stake or reward calculations.
    ///
    /// Torsten resolves pointer addresses inline during UTxO processing, so their coins
    /// are already in `stake_distribution.stake_map` under the resolved credential.
    /// This function scans all UTxOs, finds pointer-addressed ones, and subtracts their
    /// coins from the stake map — effectively migrating from ShelleyInstantStake to
    /// ConwayInstantStake semantics.
    ///
    /// Called once at the first Conway epoch boundary.
    fn exclude_pointer_address_stake(&mut self) {
        use torsten_primitives::address::Address;

        let mut excluded_total = 0u64;
        let mut excluded_count = 0u64;

        // Build a map of credential → pointer_coins so we can also subtract from
        // existing snapshot pool_stake entries (SET and GO were built pre-Conway).
        let mut ptr_coins_by_cred: HashMap<Hash32, u64> = HashMap::new();

        for (_, output) in self.utxo_set.iter() {
            if matches!(&output.address, Address::Pointer(_)) {
                // Resolve the pointer address to a credential (using `false` since
                // ptr_stake_excluded hasn't been set yet at this point).
                if let Some(cred_hash) = stake_credential_hash_with_ptrs(
                    &output.address,
                    &self.pointer_map,
                    false, // must resolve pointers here to find what to subtract
                ) {
                    if let Some(stake) = self.stake_distribution.stake_map.get_mut(&cred_hash) {
                        stake.0 = stake.0.saturating_sub(output.value.coin.0);
                        excluded_total += output.value.coin.0;
                        excluded_count += 1;
                    }
                    *ptr_coins_by_cred.entry(cred_hash).or_default() += output.value.coin.0;
                }
            }
        }

        // NOTE: We do NOT subtract from existing SET/GO snapshots. They were built
        // in Babbage era where ShelleyInstantStake correctly resolves pointers via
        // saPtrs. Only the NEW mark (built after this exclusion) and future marks
        // will lack pointer coins, matching Haskell's ConwayInstantStake semantics.
        // The SET/GO snapshots rotate out naturally over 2 epochs.

        if excluded_count > 0 {
            info!(
                excluded_count,
                excluded_total,
                excluded_ada = excluded_total / 1_000_000,
                "Conway: excluded pointer-addressed UTxO stake from distribution \
                 (matching ConwayInstantStake semantics)"
            );
        }
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
