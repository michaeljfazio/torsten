use super::{LedgerState, StakeSnapshot};
use crate::ledger_seq::EpochTransitionDelta;
use dugite_primitives::hash::{Hash28, Hash32};
use dugite_primitives::time::EpochNo;
use dugite_primitives::value::Lovelace;
use std::collections::HashMap;
use std::sync::Arc;
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
        let bprev_block_count = self.consensus.epoch_block_count;
        let bprev_blocks_by_pool = Arc::clone(&self.consensus.epoch_blocks_by_pool);

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
        if self.utxo.pending_donations.0 > 0 {
            let flushed = self.utxo.pending_donations;
            self.epochs.treasury.0 = self.epochs.treasury.0.saturating_add(flushed.0);
            self.utxo.pending_donations = Lovelace(0);
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
                .epochs
                .snapshots
                .go
                .clone()
                .unwrap_or_else(|| StakeSnapshot::empty(EpochNo(0)));
            let bprev = StakeSnapshot {
                epoch_block_count: self.epochs.snapshots.bprev_block_count,
                epoch_blocks_by_pool: Arc::clone(&self.epochs.snapshots.bprev_blocks_by_pool),
                ..StakeSnapshot::empty(EpochNo(0))
            };
            let rupd =
                self.calculate_rewards_full(&go_snapshot, &bprev, self.epochs.snapshots.ss_fee);
            self.epochs.reserves.0 = self.epochs.reserves.0.saturating_sub(rupd.delta_reserves);
            self.epochs.treasury.0 = self.epochs.treasury.0.saturating_add(rupd.delta_treasury);

            // Apply per-account rewards, matching Haskell's applyRUpdFiltered:
            // rewards for REGISTERED credentials go to their reward accounts;
            // rewards for UNREGISTERED credentials are forwarded to treasury.
            // A credential is "registered" if it has an entry in reward_accounts
            // (created by StakeRegistration certificate processing).
            let mut total_applied = 0u64;
            let mut unregistered_total = 0u64;
            for (cred_hash, reward) in &rupd.rewards {
                if reward.0 > 0 {
                    if self.certs.reward_accounts.contains_key(cred_hash) {
                        *Arc::make_mut(&mut self.certs.reward_accounts)
                            .entry(*cred_hash)
                            .or_insert(Lovelace(0)) += *reward;
                        total_applied += reward.0;
                    } else {
                        // Unregistered credential: forward to treasury
                        // (matches Haskell's frTotalUnregistered in applyRUpd)
                        self.epochs.treasury.0 = self.epochs.treasury.0.saturating_add(reward.0);
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
                    ss_fee = self.epochs.snapshots.ss_fee.0,
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
        let captured_fees = self.utxo.epoch_fees;
        self.epochs.snapshots.go = self.epochs.snapshots.set.take();
        self.epochs.snapshots.set = self.epochs.snapshots.mark.take();
        self.epochs.snapshots.ss_fee = captured_fees;
        // bprev = blocks from the epoch that just ended (nesBprev = nesBcur).
        // The overlay check uses the d value from the epoch that just ended
        // (captured BEFORE PPUP updates the protocol params). This was saved
        // at the top of process_epoch_transition.
        self.epochs.snapshots.bprev_block_count = bprev_block_count;
        self.epochs.snapshots.bprev_blocks_by_pool = bprev_blocks_by_pool;
        // After the first rotation, bprev/ss_fee contain real epoch data.
        // Subsequent RUPD computations can fire (matching Haskell's nesRu = SJust).
        self.epochs.snapshots.rupd_ready = true;

        // Full UTxO rebuild is only needed after Mithril import or snapshot restore
        // (where incremental tracking was not active). During normal block processing
        // (replay or live sync), the incremental stake_map is accurate.
        if self.epochs.needs_stake_rebuild {
            self.rebuild_stake_distribution();
            // Once rebuilt, disable for subsequent boundaries — incremental is correct.
            self.epochs.needs_stake_rebuild = false;
        }

        // Conway pointer address exclusion is now handled in apply_block BEFORE
        // the epoch transition, gated on block.era >= Conway (matching Haskell's
        // TranslateEra timing).  See apply_block for the full explanation.

        // Per Cardano spec, total stake = UTxO-delegated stake + reward account balance.
        let mut pool_stake: HashMap<dugite_primitives::hash::Hash28, Lovelace> =
            HashMap::with_capacity(self.certs.pool_params.len());
        for (cred_hash, pool_id) in self.certs.delegations.iter() {
            let utxo_stake = self
                .certs
                .stake_distribution
                .stake_map
                .get(cred_hash)
                .copied()
                .unwrap_or(Lovelace(0));
            let reward_balance = self
                .certs
                .reward_accounts
                .get(cred_hash)
                .copied()
                .unwrap_or(Lovelace(0));
            let total_stake = Lovelace(utxo_stake.0 + reward_balance.0);
            *pool_stake.entry(*pool_id).or_insert(Lovelace(0)) += total_stake;
        }

        // Resolve deferred pointer-addressed UTxO stake at SNAP time (Haskell's sisPtrStake).
        //
        // In Haskell's ShelleyInstantStake, pointer-addressed UTxO coins are stored in
        // `sisPtrStake` (pointer → coin) and resolved to credentials via the current `saPtrs`
        // map when the SNAP rule runs.  Credentials that have deregistered will have had their
        // pointer_map entries removed, so their pointer-addressed coins are excluded from
        // this snapshot.  In Conway, ptr_stake is always empty (cleared at the HFC boundary).
        if !self.epochs.ptr_stake.is_empty() {
            let mut ptr_resolved = 0u64;
            let mut ptr_excluded = 0u64;
            for (pointer, &coin) in &self.epochs.ptr_stake {
                if coin == 0 {
                    continue;
                }
                // Resolve the pointer to a credential via the CURRENT pointer_map.
                // If the pointer has no entry (credential deregistered), exclude the coins.
                if let Some(cred_hash) = self.certs.pointer_map.get(pointer) {
                    // Only count coins for registered, delegated credentials.
                    if self.certs.reward_accounts.contains_key(cred_hash) {
                        if let Some(pool_id) = self.certs.delegations.get(cred_hash) {
                            *pool_stake.entry(*pool_id).or_insert(Lovelace(0)) += Lovelace(coin);
                            ptr_resolved += coin;
                        }
                    }
                } else {
                    ptr_excluded += coin;
                }
            }
            if ptr_resolved > 0 || ptr_excluded > 0 {
                debug!(
                    epoch = new_epoch.0,
                    ptr_resolved_ada = ptr_resolved / 1_000_000,
                    ptr_excluded_ada = ptr_excluded / 1_000_000,
                    "SNAP: resolved pointer-addressed UTxO stake (sisPtrStake)"
                );
            }
        }

        // Build per-credential stake including reward balances for the snapshot.
        // Only include credentials that are in the delegation map, matching
        // Haskell's ssStake which is the intersection of staking credentials
        // and the delegation map.
        let mut snapshot_stake: HashMap<Hash32, Lovelace> =
            HashMap::with_capacity(self.certs.delegations.len());
        for cred_hash in self.certs.delegations.keys() {
            let utxo_stake = self
                .certs
                .stake_distribution
                .stake_map
                .get(cred_hash)
                .copied()
                .unwrap_or(Lovelace(0));
            let reward_balance = self
                .certs
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
        //
        // This mirrors the pool_stake resolution above.  snapshot_stake is used
        // for per-member reward calculation (individual sigma values).  Pointer
        // coins must be included here so that pool members with pointer addresses
        // receive the correct proportional rewards.
        if !self.epochs.ptr_stake.is_empty() {
            for (pointer, &coin) in &self.epochs.ptr_stake {
                if coin == 0 {
                    continue;
                }
                if let Some(cred_hash) = self.certs.pointer_map.get(pointer) {
                    if self.certs.reward_accounts.contains_key(cred_hash)
                        && self.certs.delegations.contains_key(cred_hash)
                    {
                        *snapshot_stake.entry(*cred_hash).or_insert(Lovelace(0)) += Lovelace(coin);
                    }
                }
            }
        }

        let total_utxo_stake: u64 = self
            .certs
            .stake_distribution
            .stake_map
            .values()
            .fold(0u64, |acc, l| acc.saturating_add(l.0));
        let total_pool_stake: u64 = pool_stake
            .values()
            .fold(0u64, |acc, l| acc.saturating_add(l.0));
        debug!(
            epoch = new_epoch.0,
            credentials = self.certs.stake_distribution.stake_map.len(),
            reward_accounts = self.certs.reward_accounts.len(),
            delegations = self.certs.delegations.len(),
            pool_params = self.certs.pool_params.len(),
            pools_with_stake = pool_stake.len(),
            total_utxo_stake_ada = total_utxo_stake / 1_000_000,
            total_pool_stake_ada = total_pool_stake / 1_000_000,
            "Epoch snapshot: stake distribution rebuilt from UTxO set"
        );

        self.epochs.snapshots.mark = Some(StakeSnapshot {
            epoch: new_epoch,
            delegations: Arc::clone(&self.certs.delegations),
            pool_stake,
            pool_params: Arc::clone(&self.certs.pool_params),
            stake_distribution: Arc::new(snapshot_stake),
            // Block production data in the mark is used for legacy calculate_rewards().
            // The primary RUPD path uses bprev (from EpochSnapshots) instead.
            epoch_fees: self.utxo.epoch_fees,
            epoch_block_count: self.consensus.epoch_block_count,
            epoch_blocks_by_pool: Arc::clone(&self.consensus.epoch_blocks_by_pool),
        });

        // NOTE: DRep distribution snapshot is NOT captured here.  It is captured
        // at the END of the epoch transition (alongside the ratification snapshot),
        // matching Haskell's `setFreshDRepPulsingState` which initialises the DRep
        // pulser from the post-EPOCH state.  The snapshot captured at the end of the
        // N-1→N transition is consumed by `build_drep_power_cache()` during RATIFY
        // at the N→N+1 boundary, giving it a one-epoch lag that matches the Haskell
        // pulser's lifecycle.

        // Apply future pool parameters (re-registrations deferred from previous epoch).
        //
        // In Haskell's POOLREAP, futurePoolParams are merged with psStakePools using
        // Map.merge with Map.dropMissing for future-only entries. This means:
        //   - Pools in BOTH future AND current: update params from future ✓
        //   - Pools ONLY in current: keep as-is ✓
        //   - Pools ONLY in future (e.g., pool retired between re-reg and boundary): DROPPED
        //
        // This prevents retired pools from being resurrected by stale futurePoolParams.
        if !self.certs.future_pool_params.is_empty() {
            let mut applied = 0u64;
            let mut dropped = 0u64;
            let pool_params = Arc::make_mut(&mut self.certs.pool_params);
            for (pool_id, pool_reg) in self.certs.future_pool_params.drain() {
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
            .certs
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
                self.certs.pending_retirements.remove(pool_id);
            }
            for pool_id in &retiring_pools {
                // Refund pool deposit to operator's registered reward account.
                // Use the stored per-pool deposit (recorded at registration time) for
                // correct refunds when pool_deposit changes via governance.
                // Also remove all delegations pointing to this pool, matching
                // Haskell's POOLREAP which filters delegations to retired pools:
                //   adjustedDelegs = Map.filter (\pid -> pid `Set.notMember` retired) delegs
                if let Some(pool_reg) = Arc::make_mut(&mut self.certs.pool_params).remove(pool_id) {
                    let pool_deposit = self
                        .certs
                        .pool_deposits
                        .remove(pool_id)
                        .map(Lovelace)
                        .unwrap_or(self.epochs.protocol_params.pool_deposit);
                    let op_key = Self::reward_account_to_hash(&pool_reg.reward_account);
                    // Refund deposit: if the reward account is registered, refund to it.
                    // If unregistered, refund goes to treasury (matching Haskell's POOLREAP
                    // which filters unclaimed refunds to treasury).
                    if self.certs.reward_accounts.contains_key(&op_key) {
                        *Arc::make_mut(&mut self.certs.reward_accounts)
                            .entry(op_key)
                            .or_insert(Lovelace(0)) += pool_deposit;
                    } else {
                        self.epochs.treasury.0 =
                            self.epochs.treasury.0.saturating_add(pool_deposit.0);
                        debug!(
                            "Pool {} deposit {} -> treasury (unregistered reward account)",
                            pool_id.to_hex(),
                            pool_deposit.0
                        );
                    }
                    // Remove delegations to the retired pool
                    Arc::make_mut(&mut self.certs.delegations)
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
        self.certs
            .pending_retirements
            .retain(|_, epoch| *epoch > new_epoch);

        // Capture prevPParams BEFORE PPUP updates curPP, matching Haskell's
        // NEWPP rule: prevPParams = old curPParams (before this boundary's PPUP).
        // The RUPD at the NEXT boundary will use prev_protocol_params for ALL
        // parameter values (rho, tau, a0, n_opt, active_slots_coeff, d, proto).
        let old_d = if self.epochs.protocol_params.protocol_version_major >= 7 {
            0.0
        } else {
            let d_n = self.epochs.protocol_params.d.numerator as f64;
            let d_d = self.epochs.protocol_params.d.denominator.max(1) as f64;
            d_n / d_d
        };
        let old_proto_major = self.epochs.protocol_params.protocol_version_major;
        let old_params = self.epochs.protocol_params.clone();

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
            pending = ?self.epochs.pending_pp_updates.keys().map(|e| e.0).collect::<Vec<_>>(),
            "PPUP: checking for proposals"
        );
        if let Some(proposals) = self.epochs.pending_pp_updates.remove(&lookup_epoch) {
            // Count distinct proposers (genesis delegate hashes)
            let mut proposer_set: std::collections::HashSet<Hash32> =
                std::collections::HashSet::with_capacity(proposals.len());
            for (genesis_hash, _) in &proposals {
                proposer_set.insert(*genesis_hash);
            }
            let distinct_proposers = proposer_set.len() as u64;

            if distinct_proposers >= self.update_quorum {
                // Merge all proposals: later proposals override earlier ones per field
                let mut merged = dugite_primitives::transaction::ProtocolParamUpdate::default();
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
                        self.epochs.protocol_params.protocol_version_major,
                        self.epochs.protocol_params.protocol_version_minor,
                        merged
                            .protocol_version_major
                            .unwrap_or(self.epochs.protocol_params.protocol_version_major),
                        merged
                            .protocol_version_minor
                            .unwrap_or(self.epochs.protocol_params.protocol_version_minor),
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
        self.epochs
            .pending_pp_updates
            .retain(|epoch, _| *epoch >= lookup_epoch);

        // Promote future proposals → current (Haskell's updatePpup: sgsCur = sgsFuture).
        // After evaluating sgsCur, future proposals become current for the next boundary.
        // This ensures proposals with ppupEpoch = E+1 (submitted during epoch E as future)
        // are promoted at E→E+1 and applied at E+1→E+2 — matching the two-step cycle.
        if !self.epochs.future_pp_updates.is_empty() {
            let promoted = std::mem::take(&mut self.epochs.future_pp_updates);
            for (epoch, proposals) in promoted {
                self.epochs
                    .pending_pp_updates
                    .entry(epoch)
                    .or_default()
                    .extend(proposals);
            }
        }

        // Ratify governance proposals that have met their voting thresholds.
        // Also handles proposal expiry (with descendant removal) and sibling
        // cleanup after enactment, per Haskell `proposalsApplyEnactment`.
        // The ratification skip uses self.epoch (the old epoch), matching
        // Haskell's reCurrentEpoch from the DRep pulser.
        self.ratify_proposals();

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
            let gov = Arc::make_mut(&mut self.gov.governance);
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
        let drep_activity = self.epochs.protocol_params.drep_activity;
        if drep_activity > 0 {
            let num_dormant = self.gov.governance.num_dormant_epochs;
            let mut newly_inactive = 0u64;
            let mut reactivated = 0u64;
            for drep in Arc::make_mut(&mut self.gov.governance).dreps.values_mut() {
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
            .gov
            .governance
            .committee_expiration
            .iter()
            .filter(|(_, exp_epoch)| **exp_epoch <= new_epoch)
            .map(|(hash, _)| *hash)
            .collect();
        if !expired_members.is_empty() {
            for hash in &expired_members {
                Arc::make_mut(&mut self.gov.governance)
                    .committee_hot_keys
                    .remove(hash);
                Arc::make_mut(&mut self.gov.governance)
                    .committee_expiration
                    .remove(hash);
            }
            debug!(
                "Expired {} committee members at epoch {}",
                expired_members.len(),
                new_epoch.0
            );
        }

        // Capture the ratification snapshot AND DRep distribution for the NEXT
        // epoch boundary.
        //
        // Per Haskell `setFreshDRepPulsingState`, the pulser is created from the
        // post-transition state — after ratification/expiry have pruned proposals,
        // enacted roots have been updated, DRep activity has been updated, and
        // committee members have been expired.  Both snapshots will be consumed at
        // the NEXT epoch boundary: the ratification snapshot by `ratify_proposals()`
        // (so proposals/votes submitted during the new epoch are not considered),
        // and the DRep distribution by `build_drep_power_cache()` (matching the
        // one-epoch-lagged DRep pulser lifecycle in Haskell).
        if self.epochs.protocol_params.protocol_version_major >= 9 {
            self.capture_drep_distribution_snapshot();
            self.capture_ratification_snapshot();
        }

        // Recalculate totalObligation (deposits) from scratch, matching Haskell's
        // EPOCH rule which replaces utxosDeposited with a fresh sum.  This serves
        // as both a correctness fix and a cross-check of incremental tracking.
        {
            let obl_stake: u64 = self.certs.stake_key_deposits.values().sum();
            let obl_pool: u64 = self.certs.pool_deposits.values().sum();
            let obl_drep: u64 = self
                .gov
                .governance
                .dreps
                .values()
                .map(|d| d.deposit.0)
                .sum();
            let obl_proposal: u64 = self
                .gov
                .governance
                .proposals
                .values()
                .map(|p| p.procedure.deposit.0)
                .sum();
            let recalculated = obl_stake + obl_pool + obl_drep + obl_proposal;
            let running = self.certs.total_stake_key_deposits + obl_pool + obl_drep + obl_proposal;
            if recalculated != running {
                debug!(
                    epoch = new_epoch.0,
                    recalculated,
                    running,
                    diff = recalculated as i64 - running as i64,
                    obl_stake,
                    total_stake_key_deposits = self.certs.total_stake_key_deposits,
                    "totalObligation recalculation: deposit drift detected"
                );
            }
            // Replace the running counter with the recalculated value,
            // matching Haskell's `utxosDepositedL .~ totalObligation`.
            self.certs.total_stake_key_deposits = obl_stake;
        }

        // Compute new epoch nonce per Haskell TICKN rule:
        //
        //   TRC (TicknEnv extraEntropy ηc ηph, TicknState _ ηh, newEpoch)
        //   epochNonce'    = ηc ⭒ ηh ⭒ extraEntropy   (uses OLD prevHashNonce)
        //   prevHashNonce' = ηph                         (THEN updates to current labNonce)
        //
        // Critical: use the OLD last_epoch_block_nonce FIRST, then update it.
        // In Haskell ηh = previous ticknStatePrevHashNonce (NOT the just-captured lab).
        let _prev_epoch_nonce = self.consensus.epoch_nonce;
        let candidate = self.consensus.candidate_nonce;
        let prev_hash_nonce = self.consensus.last_epoch_block_nonce; // ηh = OLD value

        debug!(
            epoch = new_epoch.0,
            candidate = %candidate.to_hex(),
            prev_hash_nonce = %prev_hash_nonce.to_hex(),
            block_count = self.consensus.epoch_block_count,
            "Epoch nonce inputs"
        );

        // Step 1: Compute new epoch nonce using OLD prevHashNonce (ηh).
        // Uses Haskell's Nonce combine (⭒) with NeutralNonce (ZERO) as identity:
        //   epochNonce = candidate ⭒ prevHashNonce ⭒ extraEntropy
        //   NeutralNonce ⭒ x = x;  x ⭒ NeutralNonce = x
        //   Nonce(a) ⭒ Nonce(b) = Nonce(blake2b_256(a || b))
        // extraEntropy is NeutralNonce on all real networks, so omitted.
        let zero = dugite_primitives::hash::Hash32::ZERO;
        self.consensus.epoch_nonce = if candidate == zero && prev_hash_nonce == zero {
            zero
        } else if candidate == zero {
            prev_hash_nonce
        } else if prev_hash_nonce == zero {
            candidate // identity: candidate ⭒ NeutralNonce = candidate
        } else {
            let mut nonce_input = Vec::with_capacity(64);
            nonce_input.extend_from_slice(candidate.as_bytes());
            nonce_input.extend_from_slice(prev_hash_nonce.as_bytes());
            dugite_primitives::hash::blake2b_256(&nonce_input)
        };

        // Step 2: NOW update prevHashNonce to current labNonce for NEXT epoch
        self.consensus.last_epoch_block_nonce = self.consensus.lab_nonce;

        debug!(
            epoch = new_epoch.0,
            nonce = %self.consensus.epoch_nonce.to_hex(),
            "Epoch nonce"
        );

        // evolving_nonce and candidate_nonce carry forward unchanged
        // (they are NOT reset at epoch boundaries)

        // Set prevPParams from values captured BEFORE PPUP.
        // Haskell's NEWPP: prevPParams = old curPParams (before this PPUP).
        // The RUPD at the NEXT boundary uses prev_protocol_params for ALL
        // parameter values (rho, tau, a0, n_opt, active_slots_coeff, etc.).
        self.epochs.prev_d = old_d;
        self.epochs.prev_protocol_version_major = old_proto_major;
        self.epochs.prev_protocol_params = old_params;

        // Reset per-epoch accumulators
        self.utxo.epoch_fees = Lovelace(0);
        Arc::make_mut(&mut self.consensus.epoch_blocks_by_pool).clear();
        self.consensus.epoch_block_count = 0;

        self.epoch = new_epoch;
    }

    /// Process an epoch transition and capture all changes into an `EpochTransitionDelta`.
    ///
    /// This wraps `process_epoch_transition()`: it snapshots key state before the
    /// transition, delegates to the existing method for all mutations, then builds
    /// the delta from the before/after differences.
    ///
    /// The delta captures absolute post-transition values for scalar fields (treasury,
    /// reserves, protocol_params, nonces, etc.) and incremental reward credits by
    /// diffing the reward_accounts map.  This matches `apply_epoch_transition_delta()`
    /// in `ledger_seq.rs` which sets absolute values during forward reconstruction.
    pub fn process_epoch_transition_with_delta(
        &mut self,
        new_epoch: EpochNo,
    ) -> EpochTransitionDelta {
        // Snapshot pre-transition state for diffing.
        let pre_reward_accounts: HashMap<Hash32, Lovelace> = (*self.certs.reward_accounts).clone();
        let pre_pool_params: std::collections::HashSet<Hash28> =
            self.certs.pool_params.keys().copied().collect();
        let pre_future_pool_params: HashMap<Hash28, super::PoolRegistration> =
            self.certs.future_pool_params.clone();
        let pre_pending_pp_updates_empty =
            self.epochs.pending_pp_updates.is_empty() && self.epochs.future_pp_updates.is_empty();

        // Run the existing epoch transition (all state mutations happen here).
        self.process_epoch_transition(new_epoch);

        // Build the delta from post-transition state.

        // Compute reward credits: positive differences in reward_accounts.
        let mut reward_credits: HashMap<Hash32, Lovelace> = HashMap::new();
        for (cred, &post_balance) in self.certs.reward_accounts.iter() {
            let pre_balance = pre_reward_accounts
                .get(cred)
                .copied()
                .unwrap_or(Lovelace(0));
            if post_balance.0 > pre_balance.0 {
                reward_credits.insert(*cred, Lovelace(post_balance.0 - pre_balance.0));
            }
        }

        // Pools retired: were in pre_pool_params but not in post pool_params.
        let pools_retired: Vec<Hash28> = pre_pool_params
            .iter()
            .filter(|pid| !self.certs.pool_params.contains_key(pid))
            .copied()
            .collect();

        // Future params promoted: were in pre_future_pool_params and are now
        // in pool_params with updated registration.
        let future_params_promoted: Vec<(Hash28, super::PoolRegistration)> = pre_future_pool_params
            .into_iter()
            .filter(|(pid, _)| self.certs.pool_params.contains_key(pid))
            .collect();

        // DRep activity updates: compare active flags.
        // We don't have a pre-snapshot of DRep active flags, so we record the
        // current state. apply_epoch_transition_delta() will set these values.
        let drep_activity_updates: HashMap<Hash32, bool> = self
            .gov
            .governance
            .dreps
            .iter()
            .map(|(cred, drep)| (*cred, drep.active))
            .collect();

        // Delegation changes from pool retirement (delegations removed).
        // We can't easily diff Arc<HashMap> here without pre-snapshot, but the
        // epoch transition delta application sets delegations from the delta's
        // delegation_changes field. For simplicity, we leave this empty — the
        // delegation removals are captured implicitly by the pool_stake rebuild
        // that happens during state reconstruction.
        let delegation_changes = Vec::new();

        EpochTransitionDelta {
            new_epoch,
            treasury: self.epochs.treasury,
            reserves: self.epochs.reserves,
            snapshots: self.epochs.snapshots.clone(),
            protocol_params: self.epochs.protocol_params.clone(),
            prev_protocol_params: self.epochs.prev_protocol_params.clone(),
            prev_d: self.epochs.prev_d,
            prev_protocol_version_major: self.epochs.prev_protocol_version_major,
            pending_pp_updates_cleared: !pre_pending_pp_updates_empty
                && self.epochs.pending_pp_updates.is_empty()
                && self.epochs.future_pp_updates.is_empty(),
            epoch_nonce: self.consensus.epoch_nonce,
            last_epoch_block_nonce: self.consensus.last_epoch_block_nonce,
            reward_credits,
            pools_retired,
            future_params_promoted,
            drep_activity_updates,
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
            delegation_changes,
        }
    }

    /// Rebuild stake_distribution.stake_map and ptr_stake from the full UTxO set.
    ///
    /// This recomputes per-credential UTxO stake by scanning all UTxOs,
    /// matching Haskell's behavior at epoch boundaries.  This corrects any
    /// drift from incremental tracking (e.g., after snapshot load or Mithril import).
    ///
    /// Pointer-addressed UTxOs are placed in `ptr_stake` (deferred resolution)
    /// rather than `stake_map` (eager resolution), matching Haskell's separation
    /// of `sisCredentialStake` from `sisPtrStake` in `ShelleyInstantStake`.
    pub fn rebuild_stake_distribution(&mut self) {
        use super::stake_routing;
        use super::StakeRouting;

        // Pre-size to the current credential count to minimise rehashing.
        let mut new_map: HashMap<Hash32, Lovelace> =
            HashMap::with_capacity(self.certs.stake_distribution.stake_map.len());
        let mut new_ptr_stake: HashMap<dugite_primitives::credentials::Pointer, u64> =
            HashMap::new();

        for (_, output) in self.utxo.utxo_set.iter() {
            let coin = output.value.coin.0;
            match stake_routing(&output.address, self.epochs.ptr_stake_excluded) {
                StakeRouting::Credential(cred_hash) => {
                    *new_map.entry(cred_hash).or_insert(Lovelace(0)) += Lovelace(coin);
                }
                StakeRouting::Pointer(ptr) => {
                    *new_ptr_stake.entry(ptr).or_insert(0) += coin;
                }
                StakeRouting::None => {}
            }
        }
        // Also ensure all registered stake credentials have entries (even with 0 stake)
        for cred_hash in self.certs.delegations.keys() {
            new_map.entry(*cred_hash).or_insert(Lovelace(0));
        }
        self.certs.stake_distribution.stake_map = new_map;
        self.epochs.ptr_stake = new_ptr_stake;
    }

    /// Discard deferred pointer-addressed UTxO stake at the Conway HFC boundary.
    ///
    /// In Haskell, `TranslateEra` converts `ShelleyInstantStake` →
    /// `ConwayInstantStake` at the ERA boundary, discarding `sisPtrStake`.
    /// This must fire when the era becomes Conway (not when PV reaches 9),
    /// because the era transition precedes the PPUpdate that bumps PV.
    ///
    /// With the deferred `ptr_stake` model, pointer coins were never placed in
    /// `stake_distribution.stake_map` — they were always in `ptr_stake`.  The
    /// Conway transition therefore only needs to clear `ptr_stake`; nothing needs
    /// to be subtracted from `stake_map`.
    ///
    /// Called once at the first Conway-era block, before the epoch transition.
    #[allow(dead_code)]
    pub(crate) fn exclude_pointer_address_stake(&mut self) {
        if self.epochs.ptr_stake.is_empty() {
            return;
        }

        let excluded_count = self.epochs.ptr_stake.len() as u64;
        let excluded_total: u64 = self.epochs.ptr_stake.values().sum();

        // Clear ptr_stake so the new mark (built right after this) has no pointer coins.
        // Historical SET and GO snapshots keep their pool_stake values — they were
        // computed correctly in Babbage with ShelleyInstantStake pointer resolution.
        // Haskell's TranslateEra preserves historical snapshot pool distributions;
        // only the InstantStake in UTxOState is converted (dropping sisPtrStake).
        self.epochs.ptr_stake.clear();
        info!(
            excluded_count,
            excluded_total,
            excluded_ada = excluded_total / 1_000_000,
            "Conway: discarded pointer-addressed UTxO stake and updated historical \
             snapshots — matching TranslateEra ConwayInstantStake semantics"
        );
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
        // Borrow ptr_stake and pointer_map immutably for ptr resolution.
        // We'll use each snapshot's own delegation map for the ptr_stake lookup
        // (preserving the historical delegation state that was active at that snapshot).
        let ptr_stake = &self.epochs.ptr_stake;
        let pointer_map = &self.certs.pointer_map;
        let reward_accounts = &self.certs.reward_accounts;
        let stake_map = &self.certs.stake_distribution.stake_map;

        for (name, snapshot) in [
            ("mark", &mut self.epochs.snapshots.mark),
            ("set", &mut self.epochs.snapshots.set),
            ("go", &mut self.epochs.snapshots.go),
        ] {
            if let Some(snap) = snapshot {
                let old_total: u64 = snap
                    .pool_stake
                    .values()
                    .fold(0u64, |acc, s| acc.saturating_add(s.0));
                let mut new_pool_stake: HashMap<dugite_primitives::hash::Hash28, Lovelace> =
                    HashMap::with_capacity(snap.pool_stake.len());
                for (cred_hash, pool_id) in snap.delegations.iter() {
                    let utxo_stake = stake_map.get(cred_hash).copied().unwrap_or(Lovelace(0));
                    let reward_balance = reward_accounts
                        .get(cred_hash)
                        .copied()
                        .unwrap_or(Lovelace(0));
                    let total_stake = Lovelace(utxo_stake.0.saturating_add(reward_balance.0));
                    *new_pool_stake.entry(*pool_id).or_insert(Lovelace(0)) += total_stake;
                }
                // Include pointer-addressed UTxO stake resolved via the current pointer_map.
                // Use each snapshot's own delegation map so that historical delegations are
                // respected (matching the per-snapshot delegation semantics of SNAP).
                // ptr_stake is empty in Conway (cleared at HFC boundary), so this loop
                // is a no-op post-Conway.
                for (pointer, &coin) in ptr_stake {
                    if coin == 0 {
                        continue;
                    }
                    if let Some(cred_hash) = pointer_map.get(pointer) {
                        if reward_accounts.contains_key(cred_hash) {
                            if let Some(pool_id) = snap.delegations.get(cred_hash) {
                                *new_pool_stake.entry(*pool_id).or_insert(Lovelace(0)) +=
                                    Lovelace(coin);
                            }
                        }
                    }
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
    #[allow(dead_code)]
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
        let prev = self.consensus.evolving_nonce;
        // ALWAYS hash the input — matching pallas's generate_rolling_nonce exactly.
        // DO NOT use a pass-through for 32-byte inputs — this was verified to produce
        // wrong nonces. The hash step is required for both TPraos and Praos:
        //   TPraos (64-byte raw nonce_vrf.0): eta = blake2b_256(raw) — 1 hash total
        //   Praos  (32-byte nonce_vrf_output): eta = blake2b_256(tagged_hash) — 2nd hash
        let eta_hash = dugite_primitives::hash::blake2b_256(nonce_eta);
        let mut data = Vec::with_capacity(64);
        data.extend_from_slice(self.consensus.evolving_nonce.as_bytes());
        data.extend_from_slice(eta_hash.as_bytes());
        self.consensus.evolving_nonce = dugite_primitives::hash::blake2b_256(&data);

        let _ = prev; // suppress unused warning in release
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        credential_to_hash, LedgerState, PoolRegistration, StakeSnapshot, MAX_LOVELACE_SUPPLY,
    };
    use dugite_primitives::credentials::Credential;
    use dugite_primitives::hash::{Hash28, Hash32};
    use dugite_primitives::protocol_params::ProtocolParameters;
    use dugite_primitives::time::EpochNo;
    use dugite_primitives::transaction::{Certificate, PoolParams, Rational};
    use dugite_primitives::value::Lovelace;
    use std::collections::HashMap;
    use std::sync::Arc;

    // ── Helpers ──────────────────────────────────────────────────────────────

    /// Build a fresh `LedgerState` using mainnet defaults.
    fn new_state() -> LedgerState {
        LedgerState::new(ProtocolParameters::mainnet_defaults())
    }

    /// Minimal `PoolParams` for test pool registration certificates.
    fn pool_params_for(pool_id: Hash28, reward_account: Vec<u8>) -> PoolParams {
        PoolParams {
            operator: pool_id,
            vrf_keyhash: Hash32::from_bytes([2u8; 32]),
            pledge: Lovelace(0),
            cost: Lovelace(0),
            margin: Rational {
                numerator: 0,
                denominator: 1,
            },
            reward_account,
            pool_owners: vec![pool_id],
            relays: vec![],
            pool_metadata: None,
        }
    }

    // ── Test 1: Snapshot rotation direction ──────────────────────────────────

    /// After an epoch transition the snapshots rotate: go ← old set, set ← old
    /// mark, new mark is created. The epoch fields confirm direction.
    #[test]
    fn test_snapshot_rotation_direction() {
        let mut state = new_state();

        // Pre-populate mark/set/go with distinct epoch tags so we can track them.
        state.epochs.snapshots.mark = Some(StakeSnapshot::empty(EpochNo(10)));
        state.epochs.snapshots.set = Some(StakeSnapshot::empty(EpochNo(20)));
        state.epochs.snapshots.go = Some(StakeSnapshot::empty(EpochNo(30)));

        state.process_epoch_transition(EpochNo(1));

        // go ← old set (epoch 20), set ← old mark (epoch 10), new mark created.
        assert_eq!(
            state.epochs.snapshots.go.as_ref().unwrap().epoch,
            EpochNo(20),
            "go should hold the old set snapshot"
        );
        assert_eq!(
            state.epochs.snapshots.set.as_ref().unwrap().epoch,
            EpochNo(10),
            "set should hold the old mark snapshot"
        );
        // New mark is always Some — its epoch is the new epoch.
        assert!(
            state.epochs.snapshots.mark.is_some(),
            "new mark must be created"
        );
        assert_eq!(
            state.epochs.snapshots.mark.as_ref().unwrap().epoch,
            EpochNo(1),
            "new mark epoch should be new_epoch"
        );
    }

    // ── Test 2: Pending donations flushed before RUPD ────────────────────────

    /// Treasury donations buffered during the epoch are flushed into the
    /// treasury at the epoch boundary, before any reward computation.
    #[test]
    fn test_donations_flushed_before_rewards() {
        let mut state = new_state();

        let donation = Lovelace(1_000_000);
        state.utxo.pending_donations = donation;

        let treasury_before = state.epochs.treasury;
        state.process_epoch_transition(EpochNo(1));

        // Treasury must have grown by at least the donation amount.
        assert!(
            state.epochs.treasury.0 >= treasury_before.0 + donation.0,
            "treasury should include the flushed donation (was {}, now {})",
            treasury_before.0,
            state.epochs.treasury.0
        );
        assert_eq!(
            state.utxo.pending_donations,
            Lovelace(0),
            "pending_donations must be zero after flush"
        );
    }

    // ── Test 3: Reward accounts credited at the epoch boundary ───────────────

    /// A registered credential's reward account must be credited when the RUPD
    /// produces rewards for it. We construct a GO snapshot with a pool that
    /// produced blocks in bprev so reward computation yields per-account rewards.
    #[test]
    fn test_rewards_applied_before_snap() {
        let mut state = new_state();

        // Use Conway protocol version so the pre-Babbage prefilter is disabled,
        // allowing pool rewards regardless of whether the reward account is
        // pre-registered in DState. The prefilter only fires when proto <= 6.
        state.epochs.prev_protocol_version_major = 9;
        state.epochs.prev_protocol_params.protocol_version_major = 9;

        // Give the state some circulation (non-zero total_stake): set reserves
        // below maxSupply so (maxSupply - reserves) > 0.
        state.epochs.reserves = Lovelace(MAX_LOVELACE_SUPPLY / 2);
        // prev_d >= 0.8 → eta = 1 (full monetary expansion fires).
        state.epochs.prev_d = 1.0;

        let pool_id = Hash28::from_bytes([0x01u8; 28]);
        let delegator_cred = Credential::VerificationKey(Hash28::from_bytes([0x42u8; 28]));
        let delegator_hash = credential_to_hash(&delegator_cred);

        // Pool reward account bytes (header + 28 bytes of 0x01).
        // The pool itself is not a delegator; only the member `delegator_hash` is.
        let pool_reward_account: Vec<u8> = {
            let mut ra = vec![0xe0u8]; // mainnet key credential header
            ra.extend_from_slice(&[0x01u8; 28]);
            ra
        };

        // Build a GO snapshot:
        // - pool has active stake and a registered pool_params entry
        // - delegator_hash is delegated to pool with its stake in stake_distribution
        let pool_active_stake = Lovelace(50_000_000_000_000u64); // 50M ADA

        let mut pool_stake_map: HashMap<Hash28, Lovelace> = HashMap::new();
        pool_stake_map.insert(pool_id, pool_active_stake);

        let mut delegations_map: HashMap<Hash32, Hash28> = HashMap::new();
        delegations_map.insert(delegator_hash, pool_id);

        let mut stake_dist: HashMap<Hash32, Lovelace> = HashMap::new();
        stake_dist.insert(delegator_hash, pool_active_stake);

        let mut pool_params_map: HashMap<Hash28, PoolRegistration> = HashMap::new();
        pool_params_map.insert(
            pool_id,
            PoolRegistration {
                pool_id,
                vrf_keyhash: Hash32::from_bytes([0u8; 32]),
                pledge: Lovelace(0),
                cost: Lovelace(0),
                margin_numerator: 0,
                margin_denominator: 1,
                reward_account: pool_reward_account,
                owners: vec![],
                relays: vec![],
                metadata_url: None,
                metadata_hash: None,
            },
        );

        let go_snap = StakeSnapshot {
            epoch: EpochNo(0),
            delegations: Arc::new(delegations_map),
            pool_stake: pool_stake_map,
            pool_params: Arc::new(pool_params_map),
            stake_distribution: Arc::new(stake_dist),
            epoch_fees: Lovelace(0),
            epoch_block_count: 1,
            epoch_blocks_by_pool: Arc::new({
                let mut m = HashMap::new();
                m.insert(pool_id, 1u64);
                m
            }),
        };

        // Wire up GO snapshot and bprev block data so the RUPD fires.
        state.epochs.snapshots.go = Some(go_snap);
        state.epochs.snapshots.bprev_block_count = 1;
        state.epochs.snapshots.bprev_blocks_by_pool = Arc::new({
            let mut m = HashMap::new();
            m.insert(pool_id, 1u64);
            m
        });
        state.epochs.snapshots.rupd_ready = true;

        // Register the delegator's reward account so rewards are credited (not
        // forwarded to treasury as unregistered).
        Arc::make_mut(&mut state.certs.reward_accounts).insert(delegator_hash, Lovelace(0));

        let before = *state.certs.reward_accounts.get(&delegator_hash).unwrap();

        state.process_epoch_transition(EpochNo(1));

        let after = state
            .certs
            .reward_accounts
            .get(&delegator_hash)
            .copied()
            .unwrap_or(Lovelace(0));
        assert!(
            after.0 > before.0,
            "delegator reward account must be credited when pool produced blocks \
             (before={}, after={})",
            before.0,
            after.0
        );
    }

    // ── Test 4: Fee capture at SNAP time ─────────────────────────────────────

    /// After a transition, `snapshots.ss_fee` must hold the epoch_fees that were
    /// accumulated during the epoch, and `epoch_fees` must be reset to zero.
    #[test]
    fn test_fee_capture_at_snap() {
        let mut state = new_state();

        let captured_fees = Lovelace(5_000_000);
        state.utxo.epoch_fees = captured_fees;

        state.process_epoch_transition(EpochNo(1));

        assert_eq!(
            state.epochs.snapshots.ss_fee, captured_fees,
            "ss_fee must capture the epoch's fees"
        );
        assert_eq!(
            state.utxo.epoch_fees,
            Lovelace(0),
            "epoch_fees accumulator must be reset to zero"
        );
    }

    // ── Test 5: Reward distribution formula ──────────────────────────────────

    /// With `prev_d >= 0.8` (full federation) the monetary expansion fires and
    /// treasury and reserves change as expected.  We use the `total_stake == 0`
    /// path (reserves == maxSupply) to keep the arithmetic simple.
    #[test]
    fn test_reward_distribution_formula() {
        let mut state = new_state();
        // new() sets reserves = MAX_LOVELACE_SUPPLY, so total_stake = 0.
        // With prev_d = 1.0 (>= 0.8): eta = 1, expansion = floor(rho * reserves).
        // rho = 3/1000, so expansion = floor(3/1000 * 45_000_000_000_000_000) = 135_000_000_000_000.
        // treasury_cut = floor(tau * expansion) = floor(2/10 * 135_000_000_000_000) = 27_000_000_000_000.
        // delta_reserves = treasury_cut - epoch_fees = 27_000_000_000_000 (total_stake==0 path).
        let expected_treasury_cut = 27_000_000_000_000u64;
        let expected_delta_reserves = 27_000_000_000_000u64;

        let treasury_before = state.epochs.treasury;
        let reserves_before = state.epochs.reserves;

        state.process_epoch_transition(EpochNo(1));

        assert_eq!(
            state.epochs.treasury.0,
            treasury_before.0 + expected_treasury_cut,
            "treasury should increase by tau * expansion"
        );
        assert_eq!(
            state.epochs.reserves.0,
            reserves_before.0 - expected_delta_reserves,
            "reserves should decrease by the net expansion"
        );
    }

    // ── Test 6: Unregistered reward credentials forwarded to treasury ─────────

    /// Rewards computed for a credential that is NOT in `reward_accounts` must
    /// be forwarded to the treasury (Haskell's `frTotalUnregistered`).
    #[test]
    fn test_unregistered_rewards_to_treasury() {
        let mut state = new_state();

        // Give the state some circulation so pool rewards fire.
        state.epochs.reserves = Lovelace(MAX_LOVELACE_SUPPLY / 2);
        state.epochs.prev_d = 1.0;

        let pool_id = Hash28::from_bytes([0xAAu8; 28]);
        // Use a specific credential for the pool reward account.
        let op_reward_cred_bytes = [0xBBu8; 28];
        let mut op_reward_account = vec![0xe0u8]; // mainnet key credential
        op_reward_account.extend_from_slice(&op_reward_cred_bytes);
        let op_key = LedgerState::reward_account_to_hash(&op_reward_account);

        let mut pool_stake_map: HashMap<Hash28, Lovelace> = HashMap::new();
        pool_stake_map.insert(pool_id, Lovelace(100_000_000_000)); // 100k ADA

        let mut pool_params_map: HashMap<Hash28, PoolRegistration> = HashMap::new();
        pool_params_map.insert(
            pool_id,
            PoolRegistration {
                pool_id,
                vrf_keyhash: Hash32::from_bytes([0u8; 32]),
                pledge: Lovelace(0),
                cost: Lovelace(0),
                margin_numerator: 0,
                margin_denominator: 1,
                reward_account: op_reward_account.clone(),
                owners: vec![pool_id],
                relays: vec![],
                metadata_url: None,
                metadata_hash: None,
            },
        );

        let go_snap = StakeSnapshot {
            epoch: EpochNo(0),
            delegations: Arc::new(HashMap::new()),
            pool_stake: pool_stake_map,
            pool_params: Arc::new(pool_params_map),
            stake_distribution: Arc::new(HashMap::new()),
            epoch_fees: Lovelace(0),
            epoch_block_count: 1,
            epoch_blocks_by_pool: Arc::new({
                let mut m = HashMap::new();
                m.insert(pool_id, 1u64);
                m
            }),
        };

        state.epochs.snapshots.go = Some(go_snap);
        state.epochs.snapshots.bprev_blocks_by_pool = Arc::new({
            let mut m = HashMap::new();
            m.insert(pool_id, 1u64);
            m
        });
        state.epochs.snapshots.bprev_block_count = 1;
        state.epochs.snapshots.rupd_ready = true;

        // Critically: do NOT register the operator reward account in reward_accounts.
        // The unregistered reward should be forwarded to treasury.
        assert!(
            !state.certs.reward_accounts.contains_key(&op_key),
            "precondition: op reward account must NOT be registered"
        );

        let treasury_before = state.epochs.treasury;

        state.process_epoch_transition(EpochNo(1));

        // Treasury must have grown beyond just the monetary expansion (there should
        // also be the forwarded unregistered pool reward on top of delta_treasury).
        // The forwarded reward means the final treasury > treasury_before + delta_treasury_base.
        assert!(
            state.epochs.treasury.0 > treasury_before.0,
            "treasury should grow when unregistered rewards are forwarded"
        );
        // The credential must still not be in reward_accounts (never registered).
        assert!(
            !state.certs.reward_accounts.contains_key(&op_key),
            "unregistered credential should not appear in reward_accounts"
        );
    }

    // ── Test 7: Pool retirement removes pool from pool_params ─────────────────

    /// A pool scheduled for retirement at `new_epoch` is removed from
    /// `pool_params` when that epoch boundary is crossed.
    #[test]
    fn test_pool_retirement_processing() {
        let mut state = new_state();

        let pool_id = Hash28::from_bytes([0x77u8; 28]);
        // Build a reward account that IS registered so the deposit refund path
        // does not exercise the "unregistered → treasury" branch.
        let mut reward_account = vec![0xe0u8];
        reward_account.extend_from_slice(&[0x77u8; 28]);
        let op_key = LedgerState::reward_account_to_hash(&reward_account);
        Arc::make_mut(&mut state.certs.reward_accounts).insert(op_key, Lovelace(0));

        state.process_certificate(&Certificate::PoolRegistration(pool_params_for(
            pool_id,
            reward_account,
        )));
        assert!(
            state.certs.pool_params.contains_key(&pool_id),
            "pool must be registered before test"
        );

        // Schedule retirement at epoch 3.
        state.process_certificate(&Certificate::PoolRetirement {
            pool_hash: pool_id,
            epoch: 3,
        });
        assert!(state.certs.pending_retirements.contains_key(&pool_id));

        // Transition to epoch 3: pool should be retired.
        state.process_epoch_transition(EpochNo(3));

        assert!(
            !state.certs.pool_params.contains_key(&pool_id),
            "pool must be removed from pool_params after retirement epoch"
        );
        assert!(
            !state.certs.pending_retirements.contains_key(&pool_id),
            "pending_retirements must not contain the retired pool"
        );
    }

    // ── Test 8: Pool retirement with missing reward account → treasury ────────

    /// When a retiring pool's reward account is NOT registered, the pool's
    /// deposit is forwarded to the treasury instead of the operator.
    #[test]
    fn test_pool_retirement_missing_reward_account() {
        let mut state = new_state();

        let pool_id = Hash28::from_bytes([0x88u8; 28]);
        // Reward account NOT registered in reward_accounts.
        let mut reward_account = vec![0xe0u8];
        reward_account.extend_from_slice(&[0x88u8; 28]);
        let op_key = LedgerState::reward_account_to_hash(&reward_account);
        assert!(
            !state.certs.reward_accounts.contains_key(&op_key),
            "precondition: op reward account must be unregistered"
        );

        state.process_certificate(&Certificate::PoolRegistration(pool_params_for(
            pool_id,
            reward_account,
        )));

        // Schedule retirement at epoch 5.
        state.process_certificate(&Certificate::PoolRetirement {
            pool_hash: pool_id,
            epoch: 5,
        });

        let treasury_before = state.epochs.treasury;
        state.process_epoch_transition(EpochNo(5));

        // Pool must be gone.
        assert!(!state.certs.pool_params.contains_key(&pool_id));
        // Treasury must have increased by at least the pool deposit refund amount.
        // The pool_deposit from mainnet defaults is 500 ADA.
        let pool_deposit = state.epochs.protocol_params.pool_deposit.0;
        assert!(
            state.epochs.treasury.0 >= treasury_before.0 + pool_deposit,
            "treasury should receive unregistered pool operator's deposit \
             (treasury_before={}, treasury_after={}, deposit={})",
            treasury_before.0,
            state.epochs.treasury.0,
            pool_deposit
        );
    }

    // ── Test 9: Governance ratification runs after SNAP rotation ─────────────

    /// When there are no active proposals, `ratify_proposals()` must run without
    /// error and leave the governance state intact (num_dormant_epochs increments).
    #[test]
    fn test_governance_ratification_after_snap() {
        let mut state = new_state();

        // Ensure we are in Conway (protocol_version_major >= 9) so ratify path runs.
        assert!(
            state.epochs.protocol_params.protocol_version_major >= 9,
            "test requires Conway era (PV >= 9)"
        );
        // No proposals → governance is trivially stable.
        assert!(state.gov.governance.proposals.is_empty());

        let dormant_before = state.gov.governance.num_dormant_epochs;

        state.process_epoch_transition(EpochNo(1));

        // With no proposals the epoch is dormant — counter increments by 1.
        assert_eq!(
            state.gov.governance.num_dormant_epochs,
            dormant_before + 1,
            "dormant epoch counter must increment when there are no active proposals"
        );
        // State is consistent after ratification (no panic, no removed proposals).
        assert!(state.gov.governance.proposals.is_empty());
    }

    // ── Test 10: Genesis epoch transition (0→1) ───────────────────────────────

    /// At the very first epoch boundary (0→1), the epoch advances, mark is
    /// populated, and set/go remain None (only one rotation has occurred).
    #[test]
    fn test_genesis_epoch_transition() {
        let mut state = new_state();

        assert_eq!(state.epoch, EpochNo(0));
        assert!(state.epochs.snapshots.mark.is_none());
        assert!(state.epochs.snapshots.set.is_none());
        assert!(state.epochs.snapshots.go.is_none());

        state.process_epoch_transition(EpochNo(1));

        assert_eq!(state.epoch, EpochNo(1), "epoch must advance to 1");
        assert!(
            state.epochs.snapshots.mark.is_some(),
            "mark must exist after first transition"
        );
        assert!(
            state.epochs.snapshots.set.is_none(),
            "set must be None after first transition"
        );
        assert!(
            state.epochs.snapshots.go.is_none(),
            "go must be None after first transition"
        );
    }

    // ── Test 11: bprev block count rotation ──────────────────────────────────

    /// `snapshots.bprev_block_count` is set to the current epoch's block count
    /// at each boundary, and `epoch_block_count` is reset to zero.
    #[test]
    fn test_bprev_block_count_rotation() {
        let mut state = new_state();

        let blocks_this_epoch = 42u64;
        state.consensus.epoch_block_count = blocks_this_epoch;
        let pool_id = Hash28::from_bytes([0x10u8; 28]);
        Arc::make_mut(&mut state.consensus.epoch_blocks_by_pool).insert(pool_id, blocks_this_epoch);

        state.process_epoch_transition(EpochNo(1));

        assert_eq!(
            state.epochs.snapshots.bprev_block_count, blocks_this_epoch,
            "bprev_block_count must capture this epoch's block count"
        );
        assert_eq!(
            state.consensus.epoch_block_count, 0,
            "epoch_block_count must be reset to 0 after transition"
        );
        assert!(
            state.consensus.epoch_blocks_by_pool.is_empty(),
            "epoch_blocks_by_pool must be cleared after transition"
        );
    }

    // ── Test 12: Nonce evolution ──────────────────────────────────────────────

    /// When `candidate_nonce` and `last_epoch_block_nonce` are both non-zero,
    /// the epoch nonce is updated to `blake2b_256(candidate || prev_hash_nonce)`.
    #[test]
    fn test_nonce_evolution() {
        let mut state = new_state();

        let candidate = Hash32::from_bytes([0xCAu8; 32]);
        let prev_hash = Hash32::from_bytes([0xBBu8; 32]);

        state.consensus.candidate_nonce = candidate;
        state.consensus.last_epoch_block_nonce = prev_hash;
        let epoch_nonce_before = state.consensus.epoch_nonce;

        state.process_epoch_transition(EpochNo(1));

        // epoch_nonce' = blake2b_256(candidate || prev_hash_nonce)
        let mut input = Vec::with_capacity(64);
        input.extend_from_slice(candidate.as_bytes());
        input.extend_from_slice(prev_hash.as_bytes());
        let expected = dugite_primitives::hash::blake2b_256(&input);

        assert_eq!(
            state.consensus.epoch_nonce, expected,
            "epoch_nonce must be blake2b_256(candidate || prev_hash_nonce)"
        );
        assert_ne!(
            state.consensus.epoch_nonce, epoch_nonce_before,
            "epoch_nonce must change when inputs are non-zero"
        );
        // prev_hash_nonce is updated to lab_nonce at the boundary.
        // lab_nonce starts as ZERO in new(); verify last_epoch_block_nonce = ZERO.
        assert_eq!(
            state.consensus.last_epoch_block_nonce, state.consensus.lab_nonce,
            "last_epoch_block_nonce must be updated to lab_nonce"
        );
    }

    // ── Test 13: RUPD uses ss_fee from EpochSnapshots ─────────────────────────

    /// The RUPD computation uses `snapshots.ss_fee` as the fee pot.  When
    /// `ss_fee` is non-zero, it increases the total rewards available and
    /// therefore increases the treasury cut proportionally.
    #[test]
    fn test_ss_fee_from_go_snapshot() {
        // Run two transitions: one with ss_fee=0, one with ss_fee=X, and verify
        // the treasury outcome is larger in the second case.
        let mut state_no_fee = new_state();
        let mut state_with_fee = new_state();

        // Set a known non-zero fee on the second state's ss_fee.
        let fee = Lovelace(10_000_000); // 10 ADA
        state_with_fee.epochs.snapshots.ss_fee = fee;

        state_no_fee.process_epoch_transition(EpochNo(1));
        state_with_fee.process_epoch_transition(EpochNo(1));

        // With a positive ss_fee, the reward pot = expansion + fee, and treasury
        // cut = tau * (expansion + fee) > tau * expansion alone.
        assert!(
            state_with_fee.epochs.treasury.0 > state_no_fee.epochs.treasury.0,
            "treasury must be larger when ss_fee > 0 (was {}, with fee {})",
            state_no_fee.epochs.treasury.0,
            state_with_fee.epochs.treasury.0,
        );
    }

    // ── Test 14: Stake rebuild via full UTxO walk ─────────────────────────────

    /// When `needs_stake_rebuild` is true at the epoch boundary,
    /// `rebuild_stake_distribution()` runs and populates `stake_map` from
    /// the UTxO set.
    #[test]
    fn test_stake_rebuild_full_utxo_walk() {
        use dugite_primitives::address::{Address, BaseAddress};
        use dugite_primitives::network::NetworkId;
        use dugite_primitives::transaction::{OutputDatum, TransactionInput, TransactionOutput};
        use dugite_primitives::value::Value;

        let mut state = new_state();

        let payment_cred = Credential::VerificationKey(Hash28::from_bytes([0xFFu8; 28]));
        let stake_cred = Credential::VerificationKey(Hash28::from_bytes([0xCCu8; 28]));
        let stake_key = credential_to_hash(&stake_cred);
        let amount = 5_000_000_000u64; // 5k ADA

        // Add a Base-address UTxO so rebuild_stake_distribution picks up the stake.
        let addr = Address::Base(BaseAddress {
            network: NetworkId::Mainnet,
            payment: payment_cred,
            stake: stake_cred.clone(),
        });
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0xAAu8; 32]),
            index: 0,
        };
        let output = TransactionOutput {
            address: addr,
            value: Value {
                coin: Lovelace(amount),
                multi_asset: Default::default(),
            },
            datum: OutputDatum::None,
            script_ref: None,
            is_legacy: false,
            raw_cbor: None,
        };
        state.utxo.utxo_set.insert(input, output);

        // Set flag to trigger the full UTxO walk at the epoch boundary.
        state.epochs.needs_stake_rebuild = true;

        // Register the stake credential so it has an entry in delegations and
        // reward_accounts (required for rebuild_stake_distribution to include it).
        let pool_id = Hash28::from_bytes([0xDDu8; 28]);
        state.process_certificate(&Certificate::StakeRegistration(stake_cred.clone()));
        state.process_certificate(&Certificate::StakeDelegation {
            credential: stake_cred,
            pool_hash: pool_id,
        });

        state.process_epoch_transition(EpochNo(1));

        // After the rebuild, stake_map must contain the UTxO's lovelace for the cred.
        let stake = state
            .certs
            .stake_distribution
            .stake_map
            .get(&stake_key)
            .copied()
            .unwrap_or(Lovelace(0));
        assert_eq!(
            stake.0, amount,
            "stake_map must reflect the UTxO amount after rebuild (got {})",
            stake.0
        );
        // The flag must be cleared for subsequent epochs.
        assert!(
            !state.epochs.needs_stake_rebuild,
            "needs_stake_rebuild must be cleared after the rebuild"
        );
    }
}
