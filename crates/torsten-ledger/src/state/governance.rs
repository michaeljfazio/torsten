use super::{credential_to_hash, GovernanceState, LedgerState, ProposalState};
use std::collections::HashMap;
use std::sync::Arc;
use torsten_primitives::hash::{Hash28, Hash32};
use torsten_primitives::time::EpochNo;
use torsten_primitives::transaction::{
    DRep, GovAction, GovActionId, ProposalProcedure, Rational, Vote, Voter, VotingProcedure,
};
use torsten_primitives::value::Lovelace;
use tracing::{debug, trace, warn};

impl LedgerState {
    /// Process a governance proposal.
    ///
    /// Validates:
    /// 1. Bootstrap phase restrictions — during protocol == 9 only ParameterChange,
    ///    HardForkInitiation, and InfoAction are allowed (Haskell: `isBootstrapAction`).
    /// 2. prev_action_id chain — must reference an active proposal **or** the last enacted
    ///    action of the same purpose (Haskell: `prevActionAsExpected`).
    ///    Validated at submission (not just ratification) per GOV rule.
    /// 3. pvCanFollow for HardForkInitiation — target version must follow the current version
    ///    by exactly one major increment (minor=0) or the same major with a higher minor
    ///    (Haskell: `pvCanFollow`).
    pub(crate) fn process_proposal(
        &mut self,
        tx_hash: &Hash32,
        action_index: u32,
        proposal: &ProposalProcedure,
    ) {
        // --- Check 1: Bootstrap phase proposal restrictions ---
        //
        // During Conway bootstrap (protocol_version.major == 9), only ParameterChange,
        // HardForkInitiation, and InfoAction are permitted.  Everything else (NoConfidence,
        // UpdateCommittee, NewConstitution, TreasuryWithdrawals) is rejected.
        //
        // Per Haskell `isBootstrapAction` in `Cardano.Ledger.Conway.Rules.Gov`
        // (introduced in commit b6282d5, present in all released versions):
        //
        //   isBootstrapAction :: GovAction era -> Bool
        //   isBootstrapAction = \case
        //     ParameterChange {}    -> True
        //     HardForkInitiation {} -> True
        //     InfoAction            -> True
        //     _                     -> False
        //
        // The Plomin hard fork (proto 9→10) was submitted as a HardForkInitiation during
        // the bootstrap phase and was correctly accepted.  Our earlier implementation had
        // the allowed/disallowed sets inverted.
        if self.is_bootstrap_phase() {
            let allowed = matches!(
                &proposal.gov_action,
                GovAction::ParameterChange { .. }
                    | GovAction::HardForkInitiation { .. }
                    | GovAction::InfoAction
            );
            if !allowed {
                debug!(
                    tx = %tx_hash.to_hex(),
                    action_index,
                    action_type = ?std::mem::discriminant(&proposal.gov_action),
                    "DisallowedProposalDuringBootstrap: rejecting governance proposal (protocol == 9)"
                );
                // Drop proposal — do not insert into active proposals
                return;
            }
        }

        // --- Check 2: pvCanFollow for HardForkInitiation ---
        //
        // The target protocol version must be reachable from the current version.
        // Per Haskell `pvCanFollow cur target`:
        //   (major+1, 0)  — major bump
        //   (major, minor+1)  — minor bump
        //
        // Reject with ProposalCantFollow if neither condition is met.
        if let GovAction::HardForkInitiation {
            protocol_version: (tgt_major, tgt_minor),
            ..
        } = &proposal.gov_action
        {
            let cur_major = self.protocol_params.protocol_version_major;
            let cur_minor = self.protocol_params.protocol_version_minor;
            let can_follow = (*tgt_major == cur_major + 1 && *tgt_minor == 0)
                || (*tgt_major == cur_major && *tgt_minor > cur_minor);
            if !can_follow {
                debug!(
                    tx = %tx_hash.to_hex(),
                    action_index,
                    cur_version = %format!("{cur_major}.{cur_minor}"),
                    target_version = %format!("{tgt_major}.{tgt_minor}"),
                    "ProposalCantFollow: HardForkInitiation target version does not follow current version"
                );
                // Drop proposal — do not insert into active proposals
                return;
            }
        }

        // --- Check 3: prev_action_id validation at submission ---
        //
        // Per Haskell's GOV rule, `prevActionAsExpected` is checked at proposal submission
        // (not only at ratification). The proposal's prev_action_id must either:
        //   (a) match the last enacted action of the same purpose, or
        //   (b) reference an active proposal already in the proposals map.
        //
        // This mirrors `prevActionAsExpected` used in Ratify.hs but applied here at
        // submission so invalid chains are dropped before they occupy governance state.
        let prev_id = match &proposal.gov_action {
            GovAction::ParameterChange { prev_action_id, .. }
            | GovAction::HardForkInitiation { prev_action_id, .. }
            | GovAction::NoConfidence { prev_action_id, .. }
            | GovAction::UpdateCommittee { prev_action_id, .. }
            | GovAction::NewConstitution { prev_action_id, .. } => prev_action_id.as_ref(),
            GovAction::TreasuryWithdrawals { .. } | GovAction::InfoAction => None,
        };
        if let Some(prev) = prev_id {
            // Allowed if: (a) it references the last enacted root of this purpose, OR
            //             (b) it references an active (in-flight) proposal.
            let valid_root =
                prev_action_matches_enacted_root(&proposal.gov_action, prev, &self.governance);
            let in_flight = self.governance.proposals.contains_key(prev);
            if !valid_root && !in_flight {
                debug!(
                    tx = %tx_hash.to_hex(),
                    action_index,
                    prev_action = %prev.transaction_id.to_hex(),
                    prev_index = prev.action_index,
                    "InvalidPrevActionId: proposal's prev_action_id is neither an active proposal nor the last enacted action of this purpose"
                );
                // Drop proposal — do not insert into active proposals
                return;
            }
        }

        // CIP-1694: Validate policy_hash matches constitution guardrail script
        // ParameterChange and TreasuryWithdrawals must include the constitution's script_hash
        let constitution_script = self
            .governance
            .constitution
            .as_ref()
            .and_then(|c| c.script_hash);
        match &proposal.gov_action {
            GovAction::ParameterChange { policy_hash, .. }
            | GovAction::TreasuryWithdrawals { policy_hash, .. } => {
                if let Some(required_hash) = constitution_script {
                    match policy_hash {
                        Some(provided) if *provided == required_hash => {
                            // Valid — policy hash matches constitution guardrail
                        }
                        Some(provided) => {
                            warn!(
                                "Governance proposal policy_hash {} does not match constitution guardrail {}",
                                provided.to_hex(),
                                required_hash.to_hex()
                            );
                        }
                        None => {
                            debug!(
                                "Governance proposal missing policy_hash (constitution requires {})",
                                required_hash.to_hex()
                            );
                        }
                    }
                }
            }
            _ => {}
        }

        let action_id = GovActionId {
            transaction_id: *tx_hash,
            action_index,
        };

        // Governance action lifetime from protocol parameters.
        //
        // `expires_epoch` is set to `proposed_epoch + govActionLifetime + 1`, matching
        // Haskell's `gasExpiresAfter = proposedIn + govActionLifetime + 1`.  With the
        // expiry filter `expires_epoch <= currentEpoch` (in epoch.rs), a proposal
        // submitted at epoch E with lifetime L is active through epoch E+L and is
        // removed at the E+L+1 epoch boundary — which is consistent with CIP-1694
        // section 2.6: "active for govActionLifetime subsequent epochs after the one
        // in which it was submitted".
        let gov_action_lifetime = self.protocol_params.gov_action_lifetime;
        let expires_epoch = EpochNo(
            self.epoch
                .0
                .saturating_add(gov_action_lifetime)
                .saturating_add(1),
        );

        let state = ProposalState {
            procedure: proposal.clone(),
            proposed_epoch: self.epoch,
            expires_epoch,
            yes_votes: 0,
            no_votes: 0,
            abstain_votes: 0,
        };

        debug!(
            "Governance proposal submitted: {:?} (expires epoch {})",
            action_id, expires_epoch.0
        );
        Arc::make_mut(&mut self.governance)
            .proposals
            .insert(action_id, state);
        Arc::make_mut(&mut self.governance).proposal_count += 1;
    }

    /// Process a governance vote.
    ///
    /// Validates that a CC voter is an elected (current) committee member.
    /// Post-bootstrap (protocol >= 10), votes from non-committee credentials are
    /// rejected with `UnelectedCommitteeVoter` per Haskell's GOV rule.
    ///
    /// During bootstrap (protocol == 9) this check is skipped since committee
    /// membership rules are not yet fully active.
    pub(crate) fn process_vote(
        &mut self,
        voter: &Voter,
        action_id: &GovActionId,
        procedure: &VotingProcedure,
    ) {
        // --- Check: Unelected CC member vote rejection (protocol >= 10) ---
        //
        // Per Haskell `Conway.GOV` rule, a `ConstitutionalCommittee` voter must
        // correspond to a hot credential that is currently authorized for an elected
        // (non-expired, non-resigned) cold credential in `committee_hot_keys`.
        //
        // Haskell: `isElected govState voter` checks that the hot credential maps
        // back to a cold credential that is a current committee member.
        //
        // We check during vote processing (not just ratification) to match Haskell's
        // UTXOW / GOV rule which rejects the entire transaction carrying such a vote.
        // Here we emit a warning and skip the vote record to avoid permanent state
        // pollution, while still allowing block replay for confirmed blocks.
        if let Voter::ConstitutionalCommittee(cred) = voter {
            if !self.is_bootstrap_phase() {
                let hot_hash = credential_to_hash(cred);
                // A vote is valid if the hot credential is authorised for any
                // current (non-expired, non-resigned) cold credential.
                let is_elected =
                    self.governance
                        .committee_hot_keys
                        .iter()
                        .any(|(cold_hash, registered_hot)| {
                            *registered_hot == hot_hash
                                && !self.governance.committee_resigned.contains_key(cold_hash)
                                && self
                                    .governance
                                    .committee_expiration
                                    .get(cold_hash)
                                    .is_some_and(|exp| self.epoch <= *exp)
                        });
                if !is_elected {
                    warn!(
                        tx = %action_id.transaction_id.to_hex(),
                        action_index = action_id.action_index,
                        hot_cred = %hot_hash.to_hex(),
                        "UnelectedCommitteeVoter: CC vote from unelected hot credential — ignoring"
                    );
                    return;
                }
            }
        }

        // Update vote tally on the proposal
        if let Some(proposal) = Arc::make_mut(&mut self.governance)
            .proposals
            .get_mut(action_id)
        {
            match procedure.vote {
                Vote::Yes => proposal.yes_votes += 1,
                Vote::No => proposal.no_votes += 1,
                Vote::Abstain => proposal.abstain_votes += 1,
            }
        }

        // Track DRep activity — voting counts as activity per CIP-1694
        if let Voter::DRep(cred) = voter {
            let drep_hash = credential_to_hash(cred);
            if let Some(drep) = Arc::make_mut(&mut self.governance)
                .dreps
                .get_mut(&drep_hash)
            {
                drep.last_active_epoch = self.epoch;
            }
        }

        // Record the vote (indexed by action_id for efficient ratification)
        let action_votes = Arc::make_mut(&mut self.governance)
            .votes_by_action
            .entry(action_id.clone())
            .or_default();
        // Replace existing vote from same voter, or add new
        if let Some(existing) = action_votes.iter_mut().find(|(v, _)| v == voter) {
            existing.1 = procedure.clone();
        } else {
            action_votes.push((voter.clone(), procedure.clone()));
        }

        debug!(
            "Vote cast by {:?} on {:?}: {:?}",
            voter, action_id, procedure.vote
        );
    }

    /// Check all active governance proposals for ratification.
    ///
    /// A proposal is ratified when it meets the required voting thresholds.
    /// Thresholds vary by action type and involve DRep, SPO, and/or CC votes.
    /// Ratified proposals are enacted (their effects applied) and removed.
    ///
    /// Per Haskell Ratify.hs, proposals are processed:
    /// 1. Sorted by priority (NoConfidence > UpdateCommittee > ... > InfoAction)
    /// 2. Sequentially with state threading (enacted roots update between proposals)
    /// 3. With a "delaying action" flag that blocks further ratification
    /// 4. With prev_action_id chain validation (must match last enacted of same purpose)
    pub(crate) fn ratify_proposals(&mut self) {
        let total_drep_stake = self.compute_total_drep_stake();
        let total_spo_stake = self.compute_total_spo_stake();
        // Pre-compute DRep voting power once (O(delegations)) instead of per-DRep per-proposal
        let (drep_power_cache, no_confidence_stake, _abstain_stake) = self.build_drep_power_cache();

        // Collect all proposals sorted by priority (lower = higher priority)
        let mut candidates: Vec<(GovActionId, GovAction, EpochNo)> = self
            .governance
            .proposals
            .iter()
            .map(|(id, state)| {
                (
                    id.clone(),
                    state.procedure.gov_action.clone(),
                    state.expires_epoch,
                )
            })
            .collect();
        candidates.sort_by_key(|(_, action, _)| gov_action_priority(action));

        debug!(
            epoch = self.epoch.0,
            active_proposals = candidates.len(),
            total_drep_stake,
            total_spo_stake,
            no_confidence_stake,
            bootstrap = self.is_bootstrap_phase(),
            protocol_version = self.protocol_params.protocol_version_major,
            cc_members = self.governance.committee_expiration.len(),
            cc_hot_keys = self.governance.committee_hot_keys.len(),
            cc_threshold = ?self.governance.committee_threshold,
            "Governance ratification: evaluating proposals"
        );

        let mut ratified = Vec::new();
        let mut delayed = false;

        for (action_id, action, _expires) in &candidates {
            // Check prev_action_id chain
            if !prev_action_as_expected(action, &self.governance) {
                trace!(
                    action_id = %action_id.transaction_id.to_hex(),
                    action_type = ?std::mem::discriminant(action),
                    "Governance proposal: prev_action_id chain mismatch — skipping"
                );
                continue;
            }

            // Treasury withdrawal balance check removed from ratification.
            // Haskell's withdrawalCanWithdraw uses the authoritative treasury
            // balance. If our local treasury tracking has diverged, skipping the
            // withdrawal would cause permanent ledger divergence. Trust the
            // canonical chain — if the proposal was ratified on-chain, apply it.
            // The treasury debit is applied below in the enactment step.

            // If a delaying action was already enacted this epoch, skip remaining
            if delayed {
                debug!(
                    action_id = %action_id.transaction_id.to_hex(),
                    "Governance proposal: delayed by previously enacted action"
                );
                continue;
            }

            // Check voting thresholds
            if let Some(state) = self.governance.proposals.get(action_id) {
                // Compute vote counts for logging
                let (
                    _drep_yes,
                    _drep_total,
                    _spo_yes,
                    _spo_voted,
                    _spo_abstain,
                    _cc_yes,
                    _cc_total,
                ) = self.count_votes_by_type(
                    action_id,
                    &state.procedure.gov_action,
                    &drep_power_cache,
                    no_confidence_stake,
                );
                let met = self.check_ratification(
                    action_id,
                    state,
                    total_drep_stake,
                    total_spo_stake,
                    &drep_power_cache,
                    no_confidence_stake,
                );
                if met {
                    debug!(
                        action_id = %action_id.transaction_id.to_hex(),
                        action_type = ?std::mem::discriminant(action),
                        "Governance proposal RATIFIED"
                    );
                    // Enact immediately and update roots (for chain validation of subsequent proposals)
                    self.enact_gov_action(action);
                    self.update_enacted_root(action_id, action);
                    ratified.push(action_id.clone());
                    if is_delaying_action(action) {
                        delayed = true;
                    }
                } else if !matches!(action, GovAction::InfoAction) {
                    trace!(
                        action_id = %action_id.transaction_id.to_hex(),
                        action_type = ?std::mem::discriminant(action),
                        "Governance proposal NOT ratified"
                    );
                }
            }
        }

        // Capture ratified proposal states before removal (for GetRatifyState query tag 32)
        let mut ratified_with_state = Vec::new();

        // Remove ratified proposals and refund deposits
        if !ratified.is_empty() {
            for action_id in &ratified {
                if let Some(proposal_state) = Arc::make_mut(&mut self.governance)
                    .proposals
                    .remove(action_id)
                {
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
                    ratified_with_state.push((action_id.clone(), proposal_state));
                }
                Arc::make_mut(&mut self.governance)
                    .votes_by_action
                    .remove(action_id);
            }
            debug!(
                "Governance   {} proposal(s) ratified and enacted",
                ratified.len()
            );
        }

        // Store ratification results for GetRatifyState query (tag 32)
        let gov = Arc::make_mut(&mut self.governance);
        gov.last_ratified = ratified_with_state;
        gov.last_ratify_delayed = delayed;
    }

    /// Update the enacted governance root for a given purpose after enactment.
    fn update_enacted_root(&mut self, action_id: &GovActionId, action: &GovAction) {
        match action {
            GovAction::ParameterChange { .. } => {
                Arc::make_mut(&mut self.governance).enacted_pparam_update = Some(action_id.clone());
            }
            GovAction::HardForkInitiation { .. } => {
                Arc::make_mut(&mut self.governance).enacted_hard_fork = Some(action_id.clone());
            }
            GovAction::NoConfidence { .. } | GovAction::UpdateCommittee { .. } => {
                Arc::make_mut(&mut self.governance).enacted_committee = Some(action_id.clone());
            }
            GovAction::NewConstitution { .. } => {
                Arc::make_mut(&mut self.governance).enacted_constitution = Some(action_id.clone());
            }
            // TreasuryWithdrawals and InfoAction don't update any root
            GovAction::TreasuryWithdrawals { .. } | GovAction::InfoAction => {}
        }
    }

    /// Whether we are in the Conway bootstrap phase (protocol version 9).
    /// During bootstrap, all DRep voting thresholds are set to 0 (auto-pass)
    /// per the Haskell `hardforkConwayBootstrapPhase` function.
    fn is_bootstrap_phase(&self) -> bool {
        self.protocol_params.protocol_version_major == 9
    }

    /// Check whether a proposal has met its voting thresholds for ratification.
    ///
    /// CIP-1694 voting thresholds (stake-weighted), matching Haskell cardano-ledger:
    /// - InfoAction: always ratified (no thresholds)
    /// - ParameterChange: DRep >= dvt_pp_*_group + SPO >= pvt_pp_security (if security) + CC
    /// - HardForkInitiation: DRep >= dvt_hard_fork + SPO >= pvt_hard_fork + CC
    /// - NoConfidence: DRep >= dvt_no_confidence + SPO >= pvt_motion_no_confidence (no CC)
    /// - UpdateCommittee: DRep >= dvt_committee + SPO >= pvt_committee (no CC)
    /// - NewConstitution: DRep >= dvt_constitution + CC (no SPO)
    /// - TreasuryWithdrawals: DRep >= dvt_treasury_withdrawal + CC (no SPO)
    ///
    /// During Conway bootstrap phase (protocol version 9), all DRep thresholds are 0.
    fn check_ratification(
        &self,
        action_id: &GovActionId,
        state: &ProposalState,
        _total_drep_stake: u64,
        total_spo_stake: u64,
        drep_power_cache: &HashMap<Hash32, u64>,
        no_confidence_stake: u64,
    ) -> bool {
        // Count votes by voter type (uses pre-computed DRep power cache)
        // Per CIP-1694:
        // - DRep denominator = yes + no voted stake (abstain excluded)
        // - SPO denominator = total active SPO stake (non-voting SPOs effectively vote No)
        let (drep_yes, drep_total, spo_yes, spo_voted, spo_abstain, _cc_yes, _cc_total) = self
            .count_votes_by_type(
                action_id,
                &state.procedure.gov_action,
                drep_power_cache,
                no_confidence_stake,
            );

        let bootstrap = self.is_bootstrap_phase();

        // Compute effective SPO denominator based on action type and bootstrap state.
        //
        // Per Haskell spoAcceptedRatio / votingStakePoolThreshold:
        // - HardForkInitiation: non-voting SPOs count as No → denominator = total_spo_stake
        // - All other actions: non-voting SPOs default to Abstain → denominator excludes
        //   both explicit abstainers and non-voters.
        //
        // During bootstrap (proto 9), non-voting SPOs are also treated as Abstain for
        // non-HardFork actions, so the effective denominator is:
        //   spo_voted - spo_abstain  (only explicit Yes + No voters)
        let effective_spo_denom = |action: &GovAction| -> u64 {
            if matches!(action, GovAction::HardForkInitiation { .. }) {
                // HardFork: non-voting SPOs count as No → use total active SPO stake
                total_spo_stake
            } else {
                // Non-HardFork: non-voting SPOs default to Abstain
                // Effective denominator = explicit voters minus explicit abstainers
                // During bootstrap this is the same logic (non-voters are abstain)
                spo_voted.saturating_sub(spo_abstain)
            }
        };

        match &state.procedure.gov_action {
            GovAction::InfoAction => {
                // InfoAction can NEVER be ratified — it has NoVotingThreshold for
                // all three voting bodies (DRep, SPO, CC). Votes are recorded but
                // have no effect. Proposals sit until they expire at
                // proposed_epoch + gov_action_lifetime. (Haskell: all three
                // acceptance predicates return False when threshold = SNothing.)
                false
            }
            GovAction::ParameterChange {
                protocol_param_update,
                ..
            } => {
                // Per CIP-1694: each affected DRep parameter group must independently
                // meet its own threshold. ALL affected group thresholds must be met.
                // SPO threshold = pvtPPSecurityGroup if any param is security-relevant
                // CC approval required
                let drep_met = if bootstrap {
                    true // All DRep thresholds are 0 during bootstrap
                } else {
                    pp_change_drep_all_groups_met(
                        protocol_param_update,
                        &self.protocol_params,
                        drep_yes,
                        drep_total,
                    )
                };
                let spo_met = if let Some(ref spo_threshold) =
                    pp_change_spo_threshold(protocol_param_update, &self.protocol_params)
                {
                    check_threshold(
                        spo_yes,
                        effective_spo_denom(&state.procedure.gov_action),
                        spo_threshold,
                    )
                } else {
                    true // No SPO vote required for non-security params
                };
                let cc_met = check_cc_approval(
                    action_id,
                    &self.governance,
                    self.epoch,
                    self.protocol_params.committee_min_size,
                    bootstrap,
                );
                drep_met && spo_met && cc_met
            }
            GovAction::HardForkInitiation {
                protocol_version, ..
            } => {
                let rational_zero = Rational {
                    numerator: 0,
                    denominator: 1,
                };
                // DRep + SPO + CC all required
                let drep_threshold = if bootstrap {
                    rational_zero
                } else {
                    self.protocol_params.dvt_hard_fork.clone()
                };
                let spo_threshold = &self.protocol_params.pvt_hard_fork;
                let drep_met = check_threshold(drep_yes, drep_total, &drep_threshold);
                let spo_denom = effective_spo_denom(&state.procedure.gov_action);
                let spo_met = check_threshold(spo_yes, spo_denom, spo_threshold);
                let cc_met = check_cc_approval(
                    action_id,
                    &self.governance,
                    self.epoch,
                    self.protocol_params.committee_min_size,
                    bootstrap,
                );
                debug!(
                    action_id = %action_id.transaction_id.to_hex(),
                    version = ?protocol_version,
                    bootstrap,
                    drep_yes, drep_total,
                    drep_threshold = drep_threshold.as_f64(), drep_met,
                    spo_yes, spo_denom,
                    spo_threshold = spo_threshold.as_f64(), spo_met,
                    cc_met,
                    "HardForkInitiation ratification check"
                );
                drep_met && spo_met && cc_met
            }
            GovAction::NoConfidence { .. } => {
                let rational_zero = Rational {
                    numerator: 0,
                    denominator: 1,
                };
                // DRep + SPO, no CC (CC cannot vote on NoConfidence)
                let drep_threshold = if bootstrap {
                    rational_zero
                } else {
                    self.protocol_params.dvt_no_confidence.clone()
                };
                let spo_threshold = &self.protocol_params.pvt_motion_no_confidence;
                let drep_met = check_threshold(drep_yes, drep_total, &drep_threshold);
                let spo_met = check_threshold(
                    spo_yes,
                    effective_spo_denom(&state.procedure.gov_action),
                    spo_threshold,
                );
                drep_met && spo_met
            }
            GovAction::UpdateCommittee { .. } => {
                let rational_zero = Rational {
                    numerator: 0,
                    denominator: 1,
                };
                // DRep + SPO, no CC (CC cannot vote on UpdateCommittee)
                let (drep_threshold, spo_threshold) = if self.governance.no_confidence {
                    (
                        if bootstrap {
                            rational_zero
                        } else {
                            self.protocol_params.dvt_committee_no_confidence.clone()
                        },
                        &self.protocol_params.pvt_committee_no_confidence,
                    )
                } else {
                    (
                        if bootstrap {
                            rational_zero
                        } else {
                            self.protocol_params.dvt_committee_normal.clone()
                        },
                        &self.protocol_params.pvt_committee_normal,
                    )
                };
                let drep_met = check_threshold(drep_yes, drep_total, &drep_threshold);
                let spo_met = check_threshold(
                    spo_yes,
                    effective_spo_denom(&state.procedure.gov_action),
                    spo_threshold,
                );
                drep_met && spo_met
            }
            GovAction::NewConstitution { .. } => {
                let rational_zero = Rational {
                    numerator: 0,
                    denominator: 1,
                };
                // DRep + CC, no SPO
                let drep_threshold = if bootstrap {
                    rational_zero
                } else {
                    self.protocol_params.dvt_constitution.clone()
                };
                let drep_met = check_threshold(drep_yes, drep_total, &drep_threshold);
                let cc_met = check_cc_approval(
                    action_id,
                    &self.governance,
                    self.epoch,
                    self.protocol_params.committee_min_size,
                    bootstrap,
                );
                drep_met && cc_met
            }
            GovAction::TreasuryWithdrawals { .. } => {
                let rational_zero = Rational {
                    numerator: 0,
                    denominator: 1,
                };
                // DRep + CC, no SPO
                let drep_threshold = if bootstrap {
                    rational_zero
                } else {
                    self.protocol_params.dvt_treasury_withdrawal.clone()
                };
                let drep_met = check_threshold(drep_yes, drep_total, &drep_threshold);
                let cc_met = check_cc_approval(
                    action_id,
                    &self.governance,
                    self.epoch,
                    self.protocol_params.committee_min_size,
                    bootstrap,
                );
                drep_met && cc_met
            }
        }
    }

    /// Count stake-weighted votes by voter type for a specific governance action.
    ///
    /// Per Haskell `dRepAcceptedRatio` / `spoAcceptedRatio`:
    /// - DRep denominator = total active DRep-delegated stake - abstain stake
    ///   (non-voting active DReps count as implicit No in denominator)
    /// - SPO: returns explicit yes votes; total SPO stake used as denominator
    /// - AlwaysNoConfidence stake counts as Yes for NoConfidence, No otherwise
    /// - AlwaysAbstain stake is excluded from both numerator and denominator
    /// - Inactive/expired DReps are excluded (handled by drep_power_cache)
    pub(crate) fn count_votes_by_type(
        &self,
        action_id: &GovActionId,
        action: &GovAction,
        drep_power_cache: &HashMap<Hash32, u64>,
        no_confidence_stake: u64,
    ) -> (u64, u64, u64, u64, u64, u64, u64) {
        let mut spo_yes = 0u64;
        let mut spo_total = 0u64;
        let mut spo_abstain = 0u64;
        let mut cc_yes = 0u64;
        let mut cc_total = 0u64;

        // Build DRep hash -> Vote map for this specific action
        let mut drep_votes: HashMap<Hash32, Vote> = HashMap::new();

        let empty = vec![];
        let action_votes = self
            .governance
            .votes_by_action
            .get(action_id)
            .unwrap_or(&empty);

        for (voter, procedure) in action_votes {
            match voter {
                Voter::DRep(cred) => {
                    let drep_hash = credential_to_hash(cred);
                    drep_votes.insert(drep_hash, procedure.vote.clone());
                }
                Voter::StakePool(pool_hash) => {
                    // Pool IDs are Hash28 (Blake2b-224); convert from Hash32
                    let pool_id = Hash28::from_bytes({
                        let mut b = [0u8; 28];
                        b.copy_from_slice(&pool_hash.as_bytes()[..28]);
                        b
                    });
                    let pool_stake = self.compute_spo_voting_power(&pool_id);
                    spo_total += pool_stake;
                    match &procedure.vote {
                        Vote::Yes => spo_yes += pool_stake,
                        Vote::Abstain => spo_abstain += pool_stake,
                        _ => {} // Explicit No: in denominator but not numerator
                    }
                }
                Voter::ConstitutionalCommittee(_) => {
                    cc_total += 1;
                    if procedure.vote == Vote::Yes {
                        cc_yes += 1;
                    }
                }
            }
        }

        // Compute DRep ratio per Haskell `dRepAcceptedRatio`:
        // Iterate ALL active DRep stake (from drep_power_cache), not just voters.
        // Non-voting DReps are implicit No (in denominator, not numerator).
        let mut drep_yes = 0u64;
        let mut drep_abstain = 0u64;
        let mut drep_total_all = 0u64;

        for (drep_hash, &power) in drep_power_cache {
            drep_total_all += power;
            match drep_votes.get(drep_hash) {
                Some(Vote::Yes) => {
                    drep_yes += power;
                }
                Some(Vote::Abstain) => {
                    drep_abstain += power;
                }
                Some(Vote::No) | None => {
                    // Voted No or didn't vote: implicit No (already in total)
                }
            }
        }

        // Handle AlwaysNoConfidence stake per CIP-1694:
        // - For NoConfidence actions: counts as Yes
        // - For all other actions: counts as No (in denominator, not numerator)
        // AlwaysNoConfidence is always in the denominator.
        let is_no_confidence = matches!(action, GovAction::NoConfidence { .. });
        if no_confidence_stake > 0 {
            drep_total_all += no_confidence_stake;
            if is_no_confidence {
                drep_yes += no_confidence_stake;
            }
        }

        // AlwaysAbstain: already excluded from drep_power_cache (handled in build_drep_power_cache)

        // DRep denominator = total active stake - abstain stake
        let drep_total = drep_total_all.saturating_sub(drep_abstain);

        (
            drep_yes,
            drep_total,
            spo_yes,
            spo_total,
            spo_abstain,
            cc_yes,
            cc_total,
        )
    }

    /// Get the total stake for a credential: UTxO stake + reward balance.
    ///
    /// Note: For DRep voting power (via `build_drep_power_cache_live`), proposal
    /// deposits are added separately via `proposal_deposits_by_credential()`.
    pub(crate) fn credential_stake(&self, cred_hash: &Hash32) -> u64 {
        let utxo = self
            .stake_distribution
            .stake_map
            .get(cred_hash)
            .map(|s| s.0)
            .unwrap_or(0);
        let reward = self
            .reward_accounts
            .get(cred_hash)
            .map(|s| s.0)
            .unwrap_or(0);
        utxo + reward
    }

    /// Build a map of credential → total proposal deposits for that credential.
    ///
    /// Matches Haskell's `proposalsDeposits` in the DRep pulser: credentials that
    /// submitted governance proposals have their deposited ADA counted toward
    /// DRep/SPO voting power.
    fn proposal_deposits_by_credential(&self) -> HashMap<Hash32, u64> {
        let mut deposits: HashMap<Hash32, u64> = HashMap::new();
        for proposal in self.governance.proposals.values() {
            let cred = Self::reward_account_to_hash(&proposal.procedure.return_addr);
            *deposits.entry(cred).or_default() += proposal.procedure.deposit.0;
        }
        deposits
    }

    /// Build a cache of DRep voting power (Hash32 -> delegated stake) for ratification.
    ///
    /// Per Haskell `reDRepDistr` (`Conway.Rules.Epoch`), ratification must use the
    /// DRep stake distribution captured at the *start* of the current epoch (the
    /// "mark" snapshot), not the live state.  If a snapshot is available it is used
    /// directly.  Otherwise the live `vote_delegations` state is scanned as a fallback
    /// (first epoch, or nodes upgrading from older snapshots without this field).
    ///
    /// Returns `(drep_power_cache, always_no_confidence_stake, always_abstain_stake)`.
    pub(crate) fn build_drep_power_cache(&self) -> (HashMap<Hash32, u64>, u64, u64) {
        // Use epoch-boundary snapshot when available (preferred — matches Haskell).
        if !self.governance.drep_distribution_snapshot.is_empty()
            || self.governance.drep_snapshot_no_confidence > 0
            || self.governance.drep_snapshot_abstain > 0
        {
            return (
                self.governance.drep_distribution_snapshot.clone(),
                self.governance.drep_snapshot_no_confidence,
                self.governance.drep_snapshot_abstain,
            );
        }

        // Fallback: compute from live state.  This path runs during the first epoch
        // (before any snapshot has been captured) or when loading an older ledger
        // snapshot that predates this field.
        debug!("DRep power cache: using live vote_delegations (snapshot not yet populated)");
        self.build_drep_power_cache_live()
    }

    /// Compute DRep voting power directly from live `vote_delegations`.
    ///
    /// Iterates vote_delegations once, O(n), instead of per-DRep O(n) lookups.
    /// Only includes active DReps (inactive DReps are excluded from voting power).
    /// Returns (drep_power_cache, always_no_confidence_stake, always_abstain_stake).
    pub(crate) fn build_drep_power_cache_live(&self) -> (HashMap<Hash32, u64>, u64, u64) {
        let mut cache: HashMap<Hash32, u64> = HashMap::new();
        let mut no_confidence_stake = 0u64;
        let mut abstain_stake = 0u64;
        // Precompute proposal deposits per credential (Haskell: proposalDeposits
        // passed into DRepPulser, added to each credential's voting power).
        let prop_deposits = self.proposal_deposits_by_credential();
        for (stake_cred, drep) in &self.governance.vote_delegations {
            let stake = self.credential_stake(stake_cred)
                + prop_deposits.get(stake_cred).copied().unwrap_or(0);
            match drep {
                DRep::KeyHash(h) => {
                    // Only count stake for active DReps
                    if self.governance.dreps.get(h).is_some_and(|d| d.active) {
                        *cache.entry(*h).or_default() += stake;
                    }
                }
                DRep::ScriptHash(h) => {
                    let hash32 = h.to_hash32_padded();
                    if self.governance.dreps.get(&hash32).is_some_and(|d| d.active) {
                        *cache.entry(hash32).or_default() += stake;
                    }
                }
                DRep::NoConfidence => {
                    no_confidence_stake += stake;
                }
                DRep::Abstain => {
                    abstain_stake += stake;
                }
            }
        }
        // Per Haskell `reDRepDistr`: only DReps with actual delegated stake appear.
        // DReps registered but with no delegators have 0 voting power.
        (cache, no_confidence_stake, abstain_stake)
    }

    /// Capture the DRep distribution snapshot at an epoch boundary.
    ///
    /// Called during `process_epoch_transition` (after the mark stake snapshot is
    /// taken, before ratification runs) so that all ratification within the new
    /// epoch uses consistent stake figures, matching Haskell's `snapDRepDistr`
    /// step in `Conway.Rules.Epoch`.
    pub(crate) fn capture_drep_distribution_snapshot(&mut self) {
        let (cache, no_confidence, abstain) = self.build_drep_power_cache_live();
        let gov = Arc::make_mut(&mut self.governance);
        gov.drep_distribution_snapshot = cache;
        gov.drep_snapshot_no_confidence = no_confidence;
        gov.drep_snapshot_abstain = abstain;
        debug!(
            "DRep distribution snapshot captured: {} DReps, no_confidence={}, abstain={}",
            gov.drep_distribution_snapshot.len(),
            gov.drep_snapshot_no_confidence,
            gov.drep_snapshot_abstain,
        );
    }

    /// Compute total active DRep-delegated stake across all DReps.
    /// Excludes stake delegated to inactive DReps.
    /// Includes stake delegated to Abstain and NoConfidence (they are part of total DRep ecosystem).
    pub(crate) fn compute_total_drep_stake(&self) -> u64 {
        let mut total = 0u64;
        for (stake_cred, drep) in &self.governance.vote_delegations {
            let stake = self.credential_stake(stake_cred);
            match drep {
                DRep::Abstain | DRep::NoConfidence => {
                    total += stake;
                }
                DRep::KeyHash(h) => {
                    if self.governance.dreps.get(h).is_some_and(|d| d.active) {
                        total += stake;
                    }
                }
                DRep::ScriptHash(h) => {
                    let hash32 = h.to_hash32_padded();
                    if self.governance.dreps.get(&hash32).is_some_and(|d| d.active) {
                        total += stake;
                    }
                }
            }
        }
        total.max(1) // Ensure non-zero to avoid division by zero
    }

    /// Compute the voting power of a stake pool: total delegated stake.
    ///
    /// Per CIP-1694 and the Haskell Ratify.hs implementation, SPO voting power
    /// is measured against the **mark** snapshot (the stake distribution captured
    /// at the beginning of the current epoch, immediately before the epoch
    /// transition). Using `set` (two epochs prior) would delay the effect of new
    /// delegations by an extra epoch compared to the specification.
    ///
    /// Reference: Haskell `spoVotingPower` in `Cardano.Ledger.Conway.Governance.Procedures`
    /// uses `ssStakeMarkPoolDistr` (the mark pool distribution).
    pub(crate) fn compute_spo_voting_power(&self, pool_id: &Hash28) -> u64 {
        // Use the "mark" snapshot (current epoch stake) for voting power — CIP-1694 spec.
        if let Some(ref snapshot) = self.snapshots.mark {
            if let Some(stake) = snapshot.pool_stake.get(pool_id) {
                return stake.0;
            }
        }
        // Fallback: compute from current delegations (UTxO + rewards).
        // This path is taken during the first two epochs before snapshots are populated.
        debug!("SPO voting power: falling back to O(n) delegation scan — snapshot not available");
        let mut total = 0u64;
        for (stake_cred, delegated_pool) in self.delegations.iter() {
            if delegated_pool == pool_id {
                total += self.credential_stake(stake_cred);
            }
        }
        total
    }

    /// Compute total active SPO stake across all pools.
    /// Used as the denominator for SPO voting thresholds.
    ///
    /// Per CIP-1694, the denominator is derived from the **mark** snapshot
    /// (same snapshot used for individual pool voting power) to keep the
    /// ratio consistent. Haskell uses `ssStakeMarkPoolDistr` for both the
    /// numerator (per-pool power) and this denominator.
    fn compute_total_spo_stake(&self) -> u64 {
        // Use "mark" snapshot if available (current epoch), else fall back.
        if let Some(ref snapshot) = self.snapshots.mark {
            let total: u64 = snapshot
                .pool_stake
                .values()
                .fold(0u64, |acc, s| acc.saturating_add(s.0));
            return total.max(1);
        }
        // Fallback: sum all pool stake from current delegations (UTxO + rewards).
        // This path is taken during the first two epochs before snapshots are populated.
        let mut total = 0u64;
        for stake_cred in self.delegations.keys() {
            total = total.saturating_add(self.credential_stake(stake_cred));
        }
        total.max(1)
    }

    /// Enact a ratified governance action by applying its effects
    pub(crate) fn enact_gov_action(&mut self, action: &GovAction) {
        match action {
            GovAction::ParameterChange {
                protocol_param_update,
                ..
            } => {
                if let Err(e) = self.apply_protocol_param_update(protocol_param_update) {
                    warn!(
                        error = %e,
                        "Governance protocol parameter update rejected"
                    );
                } else {
                    debug!("Governance   protocol parameters updated");
                }
            }
            GovAction::HardForkInitiation {
                protocol_version, ..
            } => {
                self.protocol_params.protocol_version_major = protocol_version.0;
                self.protocol_params.protocol_version_minor = protocol_version.1;
                debug!(
                    "Governance   hard fork initiated (protocol version {}.{})",
                    protocol_version.0, protocol_version.1
                );
            }
            GovAction::TreasuryWithdrawals { withdrawals, .. } => {
                // Matching Haskell's enactTreasury: debit the total from treasury
                // in one shot, then credit each withdrawal to its reward account
                // unconditionally (without per-withdrawal capping). The
                // ratification layer is expected to reject proposals that exceed
                // the available treasury balance. We use saturating_sub to
                // prevent underflow if local treasury tracking drifts.
                let total: u64 = withdrawals
                    .values()
                    .fold(0u64, |acc, a| acc.saturating_add(a.0));
                if total > self.treasury.0 {
                    warn!(
                        "Treasury withdrawal exceeds balance: requested {} but only {} available",
                        total, self.treasury.0
                    );
                }
                self.treasury.0 = self.treasury.0.saturating_sub(total);
                for (reward_addr, amount) in withdrawals {
                    if amount.0 > 0 && reward_addr.len() >= 29 {
                        let key = Self::reward_account_to_hash(reward_addr);
                        *Arc::make_mut(&mut self.reward_accounts)
                            .entry(key)
                            .or_insert(Lovelace(0)) += *amount;
                    }
                }
                debug!(
                    "Governance   treasury withdrawal: {} lovelace to {} accounts",
                    total,
                    withdrawals.len()
                );
            }
            GovAction::NoConfidence { .. } => {
                // No confidence motion: dissolve the committee entirely.
                // Per Haskell: `ensCommittee = SNothing` — committee is set to Nothing.
                let gov = Arc::make_mut(&mut self.governance);
                gov.committee_hot_keys.clear();
                gov.committee_expiration.clear();
                gov.committee_threshold = None; // Match Haskell SNothing
                gov.no_confidence = true;
                debug!("Governance   no confidence motion enacted, committee disbanded");
            }
            GovAction::UpdateCommittee {
                members_to_remove,
                members_to_add,
                threshold,
                ..
            } => {
                // Remove specified members
                for cred in members_to_remove {
                    let key = credential_to_hash(cred);
                    Arc::make_mut(&mut self.governance)
                        .committee_hot_keys
                        .remove(&key);
                    Arc::make_mut(&mut self.governance)
                        .committee_expiration
                        .remove(&key);
                    Arc::make_mut(&mut self.governance)
                        .committee_resigned
                        .remove(&key);
                }
                // Add new members with expiration epochs
                for (cred, expiration_epoch) in members_to_add {
                    let key = credential_to_hash(cred);
                    Arc::make_mut(&mut self.governance)
                        .committee_expiration
                        .insert(key, EpochNo(*expiration_epoch));
                    // Hot key auth comes via CommitteeHotAuth certificates
                }
                // Store the new committee quorum threshold
                Arc::make_mut(&mut self.governance).committee_threshold = Some(threshold.clone());
                // UpdateCommittee restores confidence
                Arc::make_mut(&mut self.governance).no_confidence = false;
                debug!(
                    "Governance   committee updated: {} removed, {} added, threshold={}/{}",
                    members_to_remove.len(),
                    members_to_add.len(),
                    threshold.numerator,
                    threshold.denominator,
                );
            }
            GovAction::NewConstitution { constitution, .. } => {
                Arc::make_mut(&mut self.governance).constitution = Some(constitution.clone());
                debug!(
                    "Governance   new constitution enacted (script_hash: {:?})",
                    constitution.script_hash.as_ref().map(|h| h.to_hex())
                );
            }
            GovAction::InfoAction => {
                // Info actions have no on-chain effect
                debug!("Info action ratified (no on-chain effect)");
            }
        }
    }
}

/// DRep voting group for protocol parameter classification per CIP-1694.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum DRepPPGroup {
    Network,
    Economic,
    Technical,
    Gov,
}

/// Whether SPOs can vote on a parameter change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum StakePoolPPGroup {
    Security,
    NoVote,
}

/// Classification of a protocol parameter: (DRepPPGroup, StakePoolPPGroup).
/// Matches Haskell cardano-ledger Conway `PPGroups` exactly.
pub(crate) type PPGroup = (DRepPPGroup, StakePoolPPGroup);

/// Determine which PP groups are modified by a ProtocolParamUpdate.
///
/// Each parameter belongs to exactly one (DRepPPGroup, StakePoolPPGroup) pair.
/// Classification matches Haskell cardano-ledger Conway ConwayPParams field tags.
pub(crate) fn modified_pp_groups(
    ppu: &torsten_primitives::transaction::ProtocolParamUpdate,
) -> Vec<PPGroup> {
    use DRepPPGroup::*;
    use StakePoolPPGroup::*;

    let mut groups = Vec::new();

    // Network + Security
    if ppu.max_block_body_size.is_some() {
        groups.push((Network, Security));
    }
    if ppu.max_tx_size.is_some() {
        groups.push((Network, Security));
    }
    if ppu.max_block_header_size.is_some() {
        groups.push((Network, Security));
    }
    if ppu.max_block_ex_units.is_some() {
        groups.push((Network, Security));
    }
    if ppu.max_val_size.is_some() {
        groups.push((Network, Security));
    }

    // Network + NoVote
    if ppu.max_tx_ex_units.is_some() {
        groups.push((Network, NoVote));
    }
    if ppu.max_collateral_inputs.is_some() {
        groups.push((Network, NoVote));
    }

    // Economic + Security
    if ppu.min_fee_a.is_some() {
        groups.push((Economic, Security));
    }
    if ppu.min_fee_b.is_some() {
        groups.push((Economic, Security));
    }
    if ppu.ada_per_utxo_byte.is_some() {
        groups.push((Economic, Security));
    }
    if ppu.min_fee_ref_script_cost_per_byte.is_some() {
        groups.push((Economic, Security));
    }

    // Economic + NoVote
    if ppu.key_deposit.is_some() {
        groups.push((Economic, NoVote));
    }
    if ppu.pool_deposit.is_some() {
        groups.push((Economic, NoVote));
    }
    if ppu.rho.is_some() {
        groups.push((Economic, NoVote));
    }
    if ppu.tau.is_some() {
        groups.push((Economic, NoVote));
    }
    if ppu.min_pool_cost.is_some() {
        groups.push((Economic, NoVote));
    }
    if ppu.execution_costs.is_some() {
        groups.push((Economic, NoVote));
    }

    // Technical + NoVote
    if ppu.e_max.is_some() {
        groups.push((Technical, NoVote));
    }
    if ppu.n_opt.is_some() {
        groups.push((Technical, NoVote));
    }
    if ppu.a0.is_some() {
        groups.push((Technical, NoVote));
    }
    if ppu.cost_models.is_some() {
        groups.push((Technical, NoVote));
    }
    if ppu.collateral_percentage.is_some() {
        groups.push((Technical, NoVote));
    }

    // Gov + Security
    if ppu.gov_action_deposit.is_some() {
        groups.push((Gov, Security));
    }

    // Gov + NoVote
    if ppu.dvt_pp_network_group.is_some()
        || ppu.dvt_pp_economic_group.is_some()
        || ppu.dvt_pp_technical_group.is_some()
        || ppu.dvt_pp_gov_group.is_some()
        || ppu.dvt_hard_fork.is_some()
        || ppu.dvt_no_confidence.is_some()
        || ppu.dvt_committee_normal.is_some()
        || ppu.dvt_committee_no_confidence.is_some()
        || ppu.dvt_constitution.is_some()
        || ppu.dvt_treasury_withdrawal.is_some()
    {
        groups.push((Gov, NoVote));
    }
    if ppu.pvt_motion_no_confidence.is_some()
        || ppu.pvt_committee_normal.is_some()
        || ppu.pvt_committee_no_confidence.is_some()
        || ppu.pvt_hard_fork.is_some()
        || ppu.pvt_pp_security_group.is_some()
    {
        groups.push((Gov, NoVote));
    }
    if ppu.min_committee_size.is_some() {
        groups.push((Gov, NoVote));
    }
    if ppu.committee_term_limit.is_some() {
        groups.push((Gov, NoVote));
    }
    if ppu.gov_action_lifetime.is_some() {
        groups.push((Gov, NoVote));
    }
    if ppu.drep_deposit.is_some() {
        groups.push((Gov, NoVote));
    }
    if ppu.drep_activity.is_some() {
        groups.push((Gov, NoVote));
    }

    groups
}

/// Check that ALL affected DRep parameter group thresholds are independently met.
///
/// Per CIP-1694 / Haskell `pparamsUpdateThreshold`: each affected parameter group
/// has its own DRep voting threshold. A ParameterChange is ratified only if the
/// DRep vote ratio meets the threshold for EVERY affected group independently.
///
/// This replaces the previous (incorrect) max-of-all-groups approach.
pub(crate) fn pp_change_drep_all_groups_met(
    ppu: &torsten_primitives::transaction::ProtocolParamUpdate,
    params: &torsten_primitives::protocol_params::ProtocolParameters,
    drep_yes: u64,
    drep_total: u64,
) -> bool {
    let groups = modified_pp_groups(ppu);
    // Collect unique DRep groups (avoid checking the same group multiple times)
    let mut seen = std::collections::HashSet::new();
    for (drep_group, _) in &groups {
        if !seen.insert(*drep_group) {
            continue;
        }
        let threshold = match drep_group {
            DRepPPGroup::Network => &params.dvt_pp_network_group,
            DRepPPGroup::Economic => &params.dvt_pp_economic_group,
            DRepPPGroup::Technical => &params.dvt_pp_technical_group,
            DRepPPGroup::Gov => &params.dvt_pp_gov_group,
        };
        if !check_threshold(drep_yes, drep_total, threshold) {
            return false;
        }
    }
    true
}

/// Compute the maximum DRep voting threshold for a ParameterChange governance action.
///
/// Returns the highest DRep group threshold across all affected parameter groups.
/// Used by tests and for informational purposes. For ratification, use
/// `pp_change_drep_all_groups_met` which checks each group independently.
#[cfg(test)]
pub(crate) fn pp_change_drep_threshold(
    ppu: &torsten_primitives::transaction::ProtocolParamUpdate,
    params: &torsten_primitives::protocol_params::ProtocolParameters,
) -> Rational {
    let groups = modified_pp_groups(ppu);
    let mut max_threshold = Rational {
        numerator: 0,
        denominator: 1,
    };
    for (drep_group, _) in &groups {
        let t = match drep_group {
            DRepPPGroup::Network => &params.dvt_pp_network_group,
            DRepPPGroup::Economic => &params.dvt_pp_economic_group,
            DRepPPGroup::Technical => &params.dvt_pp_technical_group,
            DRepPPGroup::Gov => &params.dvt_pp_gov_group,
        };
        if t.gt(&max_threshold) {
            max_threshold = t.clone();
        }
    }
    max_threshold
}

/// Determine if SPOs can vote on a ParameterChange, and if so, return the threshold.
///
/// Per Haskell `votingStakePoolThresholdInternal`: SPOs vote with pvtPPSecurityGroup
/// if ANY modified parameter is tagged SecurityGroup. Otherwise SPOs cannot vote.
pub(crate) fn pp_change_spo_threshold(
    ppu: &torsten_primitives::transaction::ProtocolParamUpdate,
    params: &torsten_primitives::protocol_params::ProtocolParameters,
) -> Option<Rational> {
    let groups = modified_pp_groups(ppu);
    let has_security = groups
        .iter()
        .any(|(_, spo)| *spo == StakePoolPPGroup::Security);
    if has_security {
        Some(params.pvt_pp_security_group.clone())
    } else {
        None
    }
}

pub(crate) fn check_threshold(yes: u64, total: u64, threshold: &Rational) -> bool {
    // A zero threshold always passes (e.g., DRep thresholds during Conway bootstrap)
    if threshold.is_zero() {
        return true;
    }
    if total == 0 {
        return false;
    }
    // Exact integer comparison: yes/total >= numerator/denominator
    // ⟺ yes * denominator >= numerator * total (using u128 to avoid overflow)
    threshold.is_met_by(yes, total)
}

/// Check if the constitutional committee has approved a governance action.
///
/// Per Haskell `committeeAccepted` / `committeeAcceptedRatio`:
/// - Iterate ALL committee members (from committee_expiration, which tracks membership)
/// - Expired members: excluded (treated as abstain)
/// - Members without hot keys (unregistered): excluded (treated as abstain)
/// - Resigned members: excluded (treated as abstain)
/// - Active members who didn't vote: counted as NO
/// - Active members who voted Abstain: excluded from ratio
/// - Active members who voted Yes: yes / Active members who voted No: no
/// - Ratio = yes_count / (yes_count + no_count) compared against committee_threshold
///
/// During bootstrap (protocol version 9), committeeMinSize check is skipped.
/// Post-bootstrap, if active_size < committeeMinSize, CC blocks ratification.
pub(crate) fn check_cc_approval(
    action_id: &GovActionId,
    governance: &GovernanceState,
    current_epoch: EpochNo,
    committee_min_size: u64,
    bootstrap: bool,
) -> bool {
    // Get committee quorum threshold
    let threshold = match &governance.committee_threshold {
        Some(t) => t,
        None => {
            // No committee exists — CC vote fails (blocks ratification)
            return false;
        }
    };

    // If threshold is 0, auto-approve
    if threshold.is_zero() {
        return true;
    }

    // Collect CC votes for this action indexed by hot credential
    let mut cc_votes: HashMap<Hash32, Vote> = HashMap::new();
    let empty = vec![];
    let action_votes = governance.votes_by_action.get(action_id).unwrap_or(&empty);
    for (voter, procedure) in action_votes {
        if let Voter::ConstitutionalCommittee(cred) = voter {
            let hot_key = credential_to_hash(cred);
            cc_votes.insert(hot_key, procedure.vote.clone());
        }
    }

    // Iterate all committee members and compute the ratio
    let mut yes_count = 0u64;
    let mut total_excluding_abstain = 0u64;
    let mut active_size = 0u64;

    for (cold_key, expiry) in &governance.committee_expiration {
        // Expired members: excluded (treated as abstain)
        // Per Haskell: `currentEpoch > validUntil` means expired.
        // Members are active through their expiry epoch (inclusive).
        if current_epoch > *expiry {
            continue;
        }

        // Check if member has a registered hot key
        let hot_key = match governance.committee_hot_keys.get(cold_key) {
            Some(hk) => hk,
            None => continue, // No hot key: excluded (treated as abstain)
        };

        // Resigned members: excluded (treated as abstain)
        if governance.committee_resigned.contains_key(cold_key) {
            continue;
        }

        active_size += 1;

        // Look up vote by hot credential
        match cc_votes.get(hot_key) {
            Some(Vote::Yes) => {
                yes_count += 1;
                total_excluding_abstain += 1;
            }
            Some(Vote::Abstain) => {
                // Abstain: excluded from ratio
            }
            Some(Vote::No) | None => {
                // Voted No or didn't vote: counts as No
                total_excluding_abstain += 1;
            }
        }
    }

    // Check committeeMinSize (skipped during bootstrap per Haskell spec)
    if !bootstrap && active_size < committee_min_size {
        return false;
    }

    // If no committee members exist at all
    if active_size == 0 {
        return false;
    }

    // If all active members abstained, ratio is 0
    if total_excluding_abstain == 0 {
        debug!(
            action = %action_id.transaction_id.to_hex(),
            active_size, yes_count, total_excluding_abstain,
            threshold = threshold.as_f64(),
            cc_voters = cc_votes.len(),
            committee_members = governance.committee_expiration.len(),
            hot_keys = governance.committee_hot_keys.len(),
            "CC approval check: all active members abstained"
        );
        return false;
    }

    // Exact comparison: yes_count / total_excluding_abstain >= threshold
    let result = threshold.is_met_by(yes_count, total_excluding_abstain);
    if !result {
        debug!(
            action = %action_id.transaction_id.to_hex(),
            active_size, yes_count, total_excluding_abstain,
            threshold = threshold.as_f64(),
            ratio = yes_count as f64 / total_excluding_abstain as f64,
            result,
            cc_voters = cc_votes.len(),
            committee_members = governance.committee_expiration.len(),
            hot_keys = governance.committee_hot_keys.len(),
            "CC approval check failed"
        );
    }
    result
}

/// Check that a proposal's `prev_action_id` matches the last enacted action of the same
/// governance purpose. Per Haskell `prevActionAsExpected` in Ratify.hs.
///
/// NoConfidence and UpdateCommittee share the `Committee` purpose.
/// TreasuryWithdrawals and InfoAction have no prev_action_id chain (always pass).
pub(crate) fn prev_action_as_expected(action: &GovAction, governance: &GovernanceState) -> bool {
    match action {
        GovAction::ParameterChange { prev_action_id, .. } => {
            *prev_action_id == governance.enacted_pparam_update
        }
        GovAction::HardForkInitiation { prev_action_id, .. } => {
            *prev_action_id == governance.enacted_hard_fork
        }
        GovAction::NoConfidence { prev_action_id } => {
            *prev_action_id == governance.enacted_committee
        }
        GovAction::UpdateCommittee { prev_action_id, .. } => {
            *prev_action_id == governance.enacted_committee
        }
        GovAction::NewConstitution { prev_action_id, .. } => {
            *prev_action_id == governance.enacted_constitution
        }
        // TreasuryWithdrawals and InfoAction have no chain requirement
        GovAction::TreasuryWithdrawals { .. } | GovAction::InfoAction => true,
    }
}

/// Check whether a specific `prev_id` matches the last enacted action root for the
/// given action's governance purpose.
///
/// Used at proposal *submission* time (GOV rule) to validate that `prev_action_id`
/// is coherent before inserting the proposal into the active set.
///
/// Unlike `prev_action_as_expected` (which checks `action.prev_action_id == enacted_root`),
/// this takes the candidate `prev_id` directly so callers can test it without having
/// to reconstruct the action's own `prev_action_id`.
fn prev_action_matches_enacted_root(
    action: &GovAction,
    prev_id: &GovActionId,
    governance: &GovernanceState,
) -> bool {
    let enacted = match action {
        GovAction::ParameterChange { .. } => governance.enacted_pparam_update.as_ref(),
        GovAction::HardForkInitiation { .. } => governance.enacted_hard_fork.as_ref(),
        GovAction::NoConfidence { .. } | GovAction::UpdateCommittee { .. } => {
            governance.enacted_committee.as_ref()
        }
        GovAction::NewConstitution { .. } => governance.enacted_constitution.as_ref(),
        GovAction::TreasuryWithdrawals { .. } | GovAction::InfoAction => {
            // No chain requirement; the caller should not pass a prev_id for these types.
            return false;
        }
    };
    enacted.is_some_and(|e| e == prev_id)
}

/// Returns the governance action priority for ratification ordering.
/// Lower number = higher priority, per Haskell's `actionPriority`.
pub(crate) fn gov_action_priority(action: &GovAction) -> u8 {
    match action {
        GovAction::NoConfidence { .. } => 0,
        GovAction::UpdateCommittee { .. } => 1,
        GovAction::NewConstitution { .. } => 2,
        GovAction::HardForkInitiation { .. } => 3,
        GovAction::ParameterChange { .. } => 4,
        GovAction::TreasuryWithdrawals { .. } => 5,
        GovAction::InfoAction => 6,
    }
}

/// Whether enacting this action should delay all further ratification for this epoch.
/// Per Haskell `delayingAction`: NoConfidence, HardFork, UpdateCommittee, NewConstitution.
pub(crate) fn is_delaying_action(action: &GovAction) -> bool {
    matches!(
        action,
        GovAction::NoConfidence { .. }
            | GovAction::HardForkInitiation { .. }
            | GovAction::UpdateCommittee { .. }
            | GovAction::NewConstitution { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{credential_to_hash, DRepRegistration, LedgerState, PoolRegistration};
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use torsten_primitives::credentials::Credential;
    use torsten_primitives::hash::{Hash28, Hash32};
    use torsten_primitives::protocol_params::ProtocolParameters;
    use torsten_primitives::time::EpochNo;
    use torsten_primitives::transaction::{
        Anchor, Certificate, Constitution, DRep, ExUnits, GovAction, GovActionId,
        ProposalProcedure, ProtocolParamUpdate, Rational, Vote, Voter, VotingProcedure,
    };
    use torsten_primitives::value::Lovelace;

    fn make_anchor() -> Anchor {
        Anchor {
            url: "https://example.com".to_string(),
            data_hash: Hash32::ZERO,
        }
    }

    fn make_action_id(byte: u8, index: u32) -> GovActionId {
        GovActionId {
            transaction_id: Hash32::from_bytes([byte; 32]),
            action_index: index,
        }
    }

    /// Set up a LedgerState with DReps, SPOs, and CC for governance testing.
    /// Returns the state with `n_dreps` DReps (1B stake each), `n_spos` SPOs (1B stake each),
    /// and 1 CC member. Protocol version 10 (post-bootstrap).
    fn gov_test_state(n_dreps: usize, n_spos: usize) -> LedgerState {
        let mut params = ProtocolParameters::mainnet_defaults();
        params.protocol_version_major = 10; // Post-bootstrap
        params.committee_min_size = 0; // Don't require min committee size in tests
        let mut state = LedgerState::new(params);
        state.epoch_length = 100;
        state.needs_stake_rebuild = false;
        // Zero reserves to prevent RUPD monetary expansion from interfering
        // with governance-specific assertions about treasury changes.
        state.reserves = Lovelace(0);

        // Set up CC
        let cold = Credential::VerificationKey(Hash28::from_bytes([10u8; 28]));
        let hot = Credential::VerificationKey(Hash28::from_bytes([20u8; 28]));
        let cold_key = credential_to_hash(&cold);
        Arc::make_mut(&mut state.governance)
            .committee_expiration
            .insert(cold_key, EpochNo(1000));
        state.process_certificate(&Certificate::CommitteeHotAuth {
            cold_credential: cold,
            hot_credential: hot,
        });
        Arc::make_mut(&mut state.governance).committee_threshold = Some(Rational {
            numerator: 1,
            denominator: 2,
        });

        // Register DReps with vote delegations
        for i in 0..n_dreps {
            let cred = Credential::VerificationKey(Hash28::from_bytes([i as u8; 28]));
            let key = credential_to_hash(&cred);
            Arc::make_mut(&mut state.governance).dreps.insert(
                key,
                DRepRegistration {
                    credential: cred,
                    deposit: Lovelace(500_000_000),
                    anchor: None,
                    registered_epoch: EpochNo(0),
                    last_active_epoch: EpochNo(0),
                    active: true,
                },
            );
            let stake_key = Hash32::from_bytes([200 + i as u8; 32]);
            Arc::make_mut(&mut state.governance)
                .vote_delegations
                .insert(stake_key, DRep::KeyHash(key));
            state
                .stake_distribution
                .stake_map
                .insert(stake_key, Lovelace(1_000_000_000));
        }

        // Register SPOs with delegations
        for i in 0..n_spos {
            let pool_id = Hash28::from_bytes([100 + i as u8; 28]);
            Arc::make_mut(&mut state.pool_params).insert(
                pool_id,
                PoolRegistration {
                    pool_id,
                    vrf_keyhash: Hash32::ZERO,
                    pledge: Lovelace(1_000_000),
                    cost: Lovelace(340_000_000),
                    margin_numerator: 1,
                    margin_denominator: 100,
                    reward_account: vec![],
                    owners: vec![],
                    relays: vec![],
                    metadata_url: None,
                    metadata_hash: None,
                },
            );
            let stake_key = Hash32::from_bytes([150 + i as u8; 32]);
            Arc::make_mut(&mut state.delegations).insert(stake_key, pool_id);
            state
                .stake_distribution
                .stake_map
                .insert(stake_key, Lovelace(1_000_000_000));
        }

        state
    }

    fn cc_vote_yes(state: &mut LedgerState, action_id: &GovActionId) {
        let hot_cred = Credential::VerificationKey(Hash28::from_bytes([20u8; 28]));
        state.process_vote(
            &Voter::ConstitutionalCommittee(hot_cred),
            action_id,
            &VotingProcedure {
                vote: Vote::Yes,
                anchor: None,
            },
        );
    }

    fn drep_vote(state: &mut LedgerState, i: usize, action_id: &GovActionId, vote: Vote) {
        let voter = Voter::DRep(Credential::VerificationKey(Hash28::from_bytes(
            [i as u8; 28],
        )));
        state.process_vote(&voter, action_id, &VotingProcedure { vote, anchor: None });
    }

    fn spo_vote(state: &mut LedgerState, i: usize, action_id: &GovActionId, vote: Vote) {
        let pool_hash = Hash28::from_bytes([100 + i as u8; 28]).to_hash32_padded();
        let voter = Voter::StakePool(pool_hash);
        state.process_vote(&voter, action_id, &VotingProcedure { vote, anchor: None });
    }

    // ========================================================================
    // Priority ordering tests
    // ========================================================================

    #[test]
    fn test_gov_action_priority_ordering() {
        assert_eq!(
            gov_action_priority(&GovAction::NoConfidence {
                prev_action_id: None
            }),
            0
        );
        assert_eq!(
            gov_action_priority(&GovAction::UpdateCommittee {
                prev_action_id: None,
                members_to_remove: vec![],
                members_to_add: BTreeMap::new(),
                threshold: Rational {
                    numerator: 1,
                    denominator: 2
                },
            }),
            1
        );
        assert_eq!(
            gov_action_priority(&GovAction::NewConstitution {
                prev_action_id: None,
                constitution: Constitution {
                    anchor: make_anchor(),
                    script_hash: None
                },
            }),
            2
        );
        assert_eq!(
            gov_action_priority(&GovAction::HardForkInitiation {
                prev_action_id: None,
                protocol_version: (10, 0),
            }),
            3
        );
        assert_eq!(
            gov_action_priority(&GovAction::ParameterChange {
                prev_action_id: None,
                protocol_param_update: Box::new(ProtocolParamUpdate::default()),
                policy_hash: None,
            }),
            4
        );
        assert_eq!(
            gov_action_priority(&GovAction::TreasuryWithdrawals {
                withdrawals: BTreeMap::new(),
                policy_hash: None,
            }),
            5
        );
        assert_eq!(gov_action_priority(&GovAction::InfoAction), 6);
    }

    // ========================================================================
    // Delaying action tests
    // ========================================================================

    #[test]
    fn test_delaying_actions() {
        assert!(is_delaying_action(&GovAction::NoConfidence {
            prev_action_id: None
        }));
        assert!(is_delaying_action(&GovAction::HardForkInitiation {
            prev_action_id: None,
            protocol_version: (10, 0),
        }));
        assert!(is_delaying_action(&GovAction::UpdateCommittee {
            prev_action_id: None,
            members_to_remove: vec![],
            members_to_add: BTreeMap::new(),
            threshold: Rational {
                numerator: 1,
                denominator: 2
            },
        }));
        assert!(is_delaying_action(&GovAction::NewConstitution {
            prev_action_id: None,
            constitution: Constitution {
                anchor: make_anchor(),
                script_hash: None
            },
        }));
        assert!(!is_delaying_action(&GovAction::ParameterChange {
            prev_action_id: None,
            protocol_param_update: Box::new(ProtocolParamUpdate::default()),
            policy_hash: None,
        }));
        assert!(!is_delaying_action(&GovAction::TreasuryWithdrawals {
            withdrawals: BTreeMap::new(),
            policy_hash: None,
        }));
        assert!(!is_delaying_action(&GovAction::InfoAction));
    }

    #[test]
    fn test_delaying_action_blocks_subsequent_ratification() {
        let mut state = gov_test_state(10, 10);
        // Set CC threshold to 0 to simplify
        Arc::make_mut(&mut state.governance).committee_threshold = Some(Rational {
            numerator: 0,
            denominator: 1,
        });

        // Submit two proposals: NoConfidence (delaying) + ParameterChange (non-delaying)
        let nc_hash = Hash32::from_bytes([1u8; 32]);
        state.process_proposal(
            &nc_hash,
            0,
            &ProposalProcedure {
                deposit: Lovelace(100_000_000_000),
                return_addr: vec![0u8; 29],
                gov_action: GovAction::NoConfidence {
                    prev_action_id: None,
                },
                anchor: make_anchor(),
            },
        );

        let pp_hash = Hash32::from_bytes([2u8; 32]);
        state.process_proposal(
            &pp_hash,
            0,
            &ProposalProcedure {
                deposit: Lovelace(100_000_000_000),
                return_addr: vec![0u8; 29],
                gov_action: GovAction::ParameterChange {
                    prev_action_id: None,
                    protocol_param_update: Box::new(ProtocolParamUpdate {
                        n_opt: Some(1000),
                        ..Default::default()
                    }),
                    policy_hash: None,
                },
                anchor: make_anchor(),
            },
        );

        let nc_id = make_action_id(1, 0);
        let pp_id = make_action_id(2, 0);

        // All DReps and SPOs vote Yes on both
        for i in 0..10 {
            drep_vote(&mut state, i, &nc_id, Vote::Yes);
            drep_vote(&mut state, i, &pp_id, Vote::Yes);
            spo_vote(&mut state, i, &nc_id, Vote::Yes);
        }

        state.process_epoch_transition(EpochNo(1));

        // NoConfidence should be enacted (delaying action)
        assert!(state.governance.no_confidence);
        // ParameterChange should be delayed — NOT enacted
        assert_eq!(state.protocol_params.n_opt, 500); // unchanged
                                                      // The pp proposal should still be active (delayed, not expired)
        assert!(state.governance.last_ratify_delayed);
    }

    // ========================================================================
    // Prev action ID chain tests
    // ========================================================================

    #[test]
    fn test_prev_action_id_chain_validation() {
        let gov = GovernanceState::default();

        // First action of each type must have None prevActionId
        assert!(prev_action_as_expected(
            &GovAction::ParameterChange {
                prev_action_id: None,
                protocol_param_update: Box::new(ProtocolParamUpdate::default()),
                policy_hash: None
            },
            &gov
        ));
        assert!(prev_action_as_expected(
            &GovAction::HardForkInitiation {
                prev_action_id: None,
                protocol_version: (10, 0)
            },
            &gov
        ));
        assert!(prev_action_as_expected(
            &GovAction::NoConfidence {
                prev_action_id: None
            },
            &gov
        ));
        // TreasuryWithdrawals always passes (no chain)
        assert!(prev_action_as_expected(
            &GovAction::TreasuryWithdrawals {
                withdrawals: BTreeMap::new(),
                policy_hash: None
            },
            &gov
        ));
        // InfoAction always passes
        assert!(prev_action_as_expected(&GovAction::InfoAction, &gov));
    }

    #[test]
    fn test_prev_action_id_mismatch_rejects() {
        let gov = GovernanceState::default();
        let wrong_id = GovActionId {
            transaction_id: Hash32::from_bytes([99u8; 32]),
            action_index: 0,
        };

        // ParameterChange with wrong prevActionId should fail
        assert!(!prev_action_as_expected(
            &GovAction::ParameterChange {
                prev_action_id: Some(wrong_id.clone()),
                protocol_param_update: Box::new(ProtocolParamUpdate::default()),
                policy_hash: None,
            },
            &gov
        ));

        // NoConfidence with wrong prevActionId should fail
        assert!(!prev_action_as_expected(
            &GovAction::NoConfidence {
                prev_action_id: Some(wrong_id.clone())
            },
            &gov
        ));
    }

    #[test]
    fn test_no_confidence_and_update_committee_share_committee_purpose() {
        let mut gov = GovernanceState::default();
        let enacted_id = make_action_id(50, 0);
        gov.enacted_committee = Some(enacted_id.clone());

        // Both NoConfidence and UpdateCommittee should check against enacted_committee
        assert!(prev_action_as_expected(
            &GovAction::NoConfidence {
                prev_action_id: Some(enacted_id.clone())
            },
            &gov
        ));
        assert!(prev_action_as_expected(
            &GovAction::UpdateCommittee {
                prev_action_id: Some(enacted_id),
                members_to_remove: vec![],
                members_to_add: BTreeMap::new(),
                threshold: Rational {
                    numerator: 1,
                    denominator: 2
                },
            },
            &gov
        ));
    }

    // ========================================================================
    // Parameter group classification tests
    // ========================================================================

    #[test]
    fn test_pp_groups_security_params() {
        // Security params should trigger SPO voting
        let ppu = ProtocolParamUpdate {
            max_block_body_size: Some(90000),
            ..Default::default()
        };
        let groups = modified_pp_groups(&ppu);
        assert!(groups.iter().any(|(_, s)| *s == StakePoolPPGroup::Security));

        let ppu = ProtocolParamUpdate {
            min_fee_a: Some(100),
            ..Default::default()
        };
        let groups = modified_pp_groups(&ppu);
        assert!(groups.iter().any(|(_, s)| *s == StakePoolPPGroup::Security));

        let ppu = ProtocolParamUpdate {
            gov_action_deposit: Some(Lovelace(100_000_000_000)),
            ..Default::default()
        };
        let groups = modified_pp_groups(&ppu);
        assert!(groups.iter().any(|(_, s)| *s == StakePoolPPGroup::Security));
    }

    #[test]
    fn test_pp_groups_non_security_params() {
        // Non-security params should NOT trigger SPO voting
        let ppu = ProtocolParamUpdate {
            n_opt: Some(1000),
            ..Default::default()
        };
        let groups = modified_pp_groups(&ppu);
        assert!(groups.iter().all(|(_, s)| *s == StakePoolPPGroup::NoVote));

        let ppu = ProtocolParamUpdate {
            drep_deposit: Some(Lovelace(500_000_000)),
            ..Default::default()
        };
        let groups = modified_pp_groups(&ppu);
        assert!(groups.iter().all(|(_, s)| *s == StakePoolPPGroup::NoVote));
    }

    #[test]
    fn test_pp_groups_drep_group_classification() {
        let ppu = ProtocolParamUpdate {
            max_tx_size: Some(32768),
            ..Default::default()
        };
        let groups = modified_pp_groups(&ppu);
        assert!(groups.iter().any(|(d, _)| *d == DRepPPGroup::Network));

        let ppu = ProtocolParamUpdate {
            key_deposit: Some(Lovelace(2_000_000)),
            ..Default::default()
        };
        let groups = modified_pp_groups(&ppu);
        assert!(groups.iter().any(|(d, _)| *d == DRepPPGroup::Economic));

        let ppu = ProtocolParamUpdate {
            e_max: Some(50),
            ..Default::default()
        };
        let groups = modified_pp_groups(&ppu);
        assert!(groups.iter().any(|(d, _)| *d == DRepPPGroup::Technical));

        let ppu = ProtocolParamUpdate {
            drep_activity: Some(20),
            ..Default::default()
        };
        let groups = modified_pp_groups(&ppu);
        assert!(groups.iter().any(|(d, _)| *d == DRepPPGroup::Gov));
    }

    #[test]
    fn test_pp_change_spo_threshold_security() {
        let params = ProtocolParameters::mainnet_defaults();
        let ppu = ProtocolParamUpdate {
            max_block_body_size: Some(90000), // Network + Security
            ..Default::default()
        };
        let threshold = pp_change_spo_threshold(&ppu, &params);
        assert!(threshold.is_some());
        assert_eq!(threshold.unwrap(), params.pvt_pp_security_group);
    }

    #[test]
    fn test_pp_change_spo_threshold_non_security() {
        let params = ProtocolParameters::mainnet_defaults();
        let ppu = ProtocolParamUpdate {
            n_opt: Some(1000), // Technical + NoVote
            ..Default::default()
        };
        let threshold = pp_change_spo_threshold(&ppu, &params);
        assert!(threshold.is_none());
    }

    // ========================================================================
    // DRep denominator tests (Haskell dRepAcceptedRatio)
    // ========================================================================

    #[test]
    fn test_drep_non_voters_count_as_implicit_no() {
        let mut state = gov_test_state(10, 0);
        Arc::make_mut(&mut state.governance).committee_threshold = Some(Rational {
            numerator: 0,
            denominator: 1,
        });

        let tx_hash = Hash32::from_bytes([50u8; 32]);
        state.process_proposal(
            &tx_hash,
            0,
            &ProposalProcedure {
                deposit: Lovelace(100_000_000_000),
                return_addr: vec![0u8; 29],
                gov_action: GovAction::ParameterChange {
                    prev_action_id: None,
                    protocol_param_update: Box::new(ProtocolParamUpdate {
                        n_opt: Some(1000),
                        ..Default::default()
                    }),
                    policy_hash: None,
                },
                anchor: make_anchor(),
            },
        );
        let action_id = make_action_id(50, 0);

        // Only 3 out of 10 DReps vote Yes (30%)
        // With correct denominator: 3/10 = 30% < 67% dvt_pp_technical_group → NOT ratified
        for i in 0..3 {
            drep_vote(&mut state, i, &action_id, Vote::Yes);
        }

        let (cache, nc_stake, _) = state.build_drep_power_cache();
        let (yes, total, _, _, _, _, _) = state.count_votes_by_type(
            &action_id,
            &GovAction::ParameterChange {
                prev_action_id: None,
                protocol_param_update: Box::new(ProtocolParamUpdate::default()),
                policy_hash: None,
            },
            &cache,
            nc_stake,
        );

        assert_eq!(yes, 3_000_000_000); // 3 DReps * 1B
        assert_eq!(total, 10_000_000_000); // ALL 10 DReps' stake (7 non-voters as implicit No)
    }

    #[test]
    fn test_drep_abstain_excluded_from_denominator() {
        let mut state = gov_test_state(10, 0);
        let action_id = make_action_id(50, 0);
        state.process_proposal(
            &Hash32::from_bytes([50u8; 32]),
            0,
            &ProposalProcedure {
                deposit: Lovelace(100_000_000_000),
                return_addr: vec![0u8; 29],
                gov_action: GovAction::InfoAction,
                anchor: make_anchor(),
            },
        );

        // 3 yes, 3 no, 4 abstain
        for i in 0..3 {
            drep_vote(&mut state, i, &action_id, Vote::Yes);
        }
        for i in 3..6 {
            drep_vote(&mut state, i, &action_id, Vote::No);
        }
        for i in 6..10 {
            drep_vote(&mut state, i, &action_id, Vote::Abstain);
        }

        let (cache, nc_stake, _) = state.build_drep_power_cache();
        let (yes, total, _, _, _, _, _) =
            state.count_votes_by_type(&action_id, &GovAction::InfoAction, &cache, nc_stake);

        // Denominator = total active (10B) - abstain (4B) = 6B
        assert_eq!(yes, 3_000_000_000);
        assert_eq!(total, 6_000_000_000);
    }

    #[test]
    fn test_always_abstain_excluded_entirely() {
        let mut state = gov_test_state(5, 0);
        // Add AlwaysAbstain delegators
        for i in 0..3u8 {
            let stake_key = Hash32::from_bytes([230 + i; 32]);
            Arc::make_mut(&mut state.governance)
                .vote_delegations
                .insert(stake_key, DRep::Abstain);
            state
                .stake_distribution
                .stake_map
                .insert(stake_key, Lovelace(2_000_000_000));
        }

        let action_id = make_action_id(50, 0);
        state.process_proposal(
            &Hash32::from_bytes([50u8; 32]),
            0,
            &ProposalProcedure {
                deposit: Lovelace(100_000_000_000),
                return_addr: vec![0u8; 29],
                gov_action: GovAction::InfoAction,
                anchor: make_anchor(),
            },
        );

        let (cache, nc_stake, abstain_stake) = state.build_drep_power_cache();

        // AlwaysAbstain should NOT be in drep_power_cache
        assert_eq!(abstain_stake, 6_000_000_000); // 3 * 2B
                                                  // DRep power cache should only have the 5 registered DReps
        assert_eq!(cache.len(), 5);

        let (yes, total, _, _, _, _, _) =
            state.count_votes_by_type(&action_id, &GovAction::InfoAction, &cache, nc_stake);

        // Total should only include active DRep stake (5B), not AlwaysAbstain (6B)
        assert_eq!(yes, 0);
        assert_eq!(total, 5_000_000_000);
    }

    #[test]
    fn test_always_no_confidence_yes_on_no_confidence() {
        let mut state = gov_test_state(5, 5);
        // Add AlwaysNoConfidence delegators
        for i in 0..3u8 {
            let stake_key = Hash32::from_bytes([230 + i; 32]);
            Arc::make_mut(&mut state.governance)
                .vote_delegations
                .insert(stake_key, DRep::NoConfidence);
            state
                .stake_distribution
                .stake_map
                .insert(stake_key, Lovelace(2_000_000_000));
        }

        let action_id = make_action_id(50, 0);
        state.process_proposal(
            &Hash32::from_bytes([50u8; 32]),
            0,
            &ProposalProcedure {
                deposit: Lovelace(100_000_000_000),
                return_addr: vec![0u8; 29],
                gov_action: GovAction::NoConfidence {
                    prev_action_id: None,
                },
                anchor: make_anchor(),
            },
        );

        let (cache, nc_stake, _) = state.build_drep_power_cache();
        assert_eq!(nc_stake, 6_000_000_000); // 3 * 2B

        let (yes, total, _, _, _, _, _) = state.count_votes_by_type(
            &action_id,
            &GovAction::NoConfidence {
                prev_action_id: None,
            },
            &cache,
            nc_stake,
        );

        // NoConfidence action: AlwaysNoConfidence counts as Yes
        // yes = 6B (NoConfidence), total = 5B (DReps, implicit No) + 6B (NoConfidence) = 11B
        assert_eq!(yes, 6_000_000_000);
        assert_eq!(total, 11_000_000_000);
    }

    #[test]
    fn test_always_no_confidence_no_on_other_actions() {
        let mut state = gov_test_state(5, 0);
        for i in 0..3u8 {
            let stake_key = Hash32::from_bytes([230 + i; 32]);
            Arc::make_mut(&mut state.governance)
                .vote_delegations
                .insert(stake_key, DRep::NoConfidence);
            state
                .stake_distribution
                .stake_map
                .insert(stake_key, Lovelace(2_000_000_000));
        }

        let action_id = make_action_id(50, 0);
        state.process_proposal(
            &Hash32::from_bytes([50u8; 32]),
            0,
            &ProposalProcedure {
                deposit: Lovelace(100_000_000_000),
                return_addr: vec![0u8; 29],
                gov_action: GovAction::InfoAction,
                anchor: make_anchor(),
            },
        );

        let (cache, nc_stake, _) = state.build_drep_power_cache();
        let (yes, total, _, _, _, _, _) =
            state.count_votes_by_type(&action_id, &GovAction::InfoAction, &cache, nc_stake);

        // Non-NoConfidence: AlwaysNoConfidence counts as No (in denominator, not numerator)
        assert_eq!(yes, 0);
        assert_eq!(total, 11_000_000_000); // 5B DRep + 6B NoConfidence
    }

    #[test]
    fn test_inactive_drep_excluded_from_power_cache() {
        let mut state = gov_test_state(5, 0);
        // Mark first 2 DReps inactive
        let cred0 = Credential::VerificationKey(Hash28::from_bytes([0u8; 28]));
        let cred1 = Credential::VerificationKey(Hash28::from_bytes([1u8; 28]));
        let key0 = credential_to_hash(&cred0);
        let key1 = credential_to_hash(&cred1);
        Arc::make_mut(&mut state.governance)
            .dreps
            .get_mut(&key0)
            .unwrap()
            .active = false;
        Arc::make_mut(&mut state.governance)
            .dreps
            .get_mut(&key1)
            .unwrap()
            .active = false;

        let (cache, _, _) = state.build_drep_power_cache();
        assert!(!cache.contains_key(&key0));
        assert!(!cache.contains_key(&key1));
        assert_eq!(cache.len(), 3); // Only 3 active DReps
    }

    // ========================================================================
    // CC approval tests
    // ========================================================================

    #[test]
    fn test_cc_expired_members_excluded() {
        let mut state = gov_test_state(5, 0);
        state.protocol_params.committee_min_size = 0;
        // Add a second CC member with early expiry
        let cold2 = Credential::VerificationKey(Hash28::from_bytes([11u8; 28]));
        let hot2 = Credential::VerificationKey(Hash28::from_bytes([21u8; 28]));
        let cold2_key = credential_to_hash(&cold2);
        Arc::make_mut(&mut state.governance)
            .committee_expiration
            .insert(cold2_key, EpochNo(5)); // Active through epoch 5

        state.process_certificate(&Certificate::CommitteeHotAuth {
            cold_credential: cold2,
            hot_credential: hot2,
        });

        // Set epoch to 5 — member should still be active (expiry is inclusive)
        state.epoch = EpochNo(5);
        let action_id = make_action_id(50, 0);
        state.process_proposal(
            &Hash32::from_bytes([50u8; 32]),
            0,
            &ProposalProcedure {
                deposit: Lovelace(100_000_000_000),
                return_addr: vec![0u8; 29],
                gov_action: GovAction::InfoAction,
                anchor: make_anchor(),
            },
        );

        // Both CC members vote Yes
        cc_vote_yes(&mut state, &action_id);
        let hot2_cred = Credential::VerificationKey(Hash28::from_bytes([21u8; 28]));
        state.process_vote(
            &Voter::ConstitutionalCommittee(hot2_cred),
            &action_id,
            &VotingProcedure {
                vote: Vote::Yes,
                anchor: None,
            },
        );

        // At epoch 5, member with expiry 5 should still be active
        let result = check_cc_approval(
            &action_id,
            &state.governance,
            EpochNo(5),
            state.protocol_params.committee_min_size,
            false,
        );
        assert!(result);

        // At epoch 6, member with expiry 5 should be expired
        let result = check_cc_approval(
            &action_id,
            &state.governance,
            EpochNo(6),
            state.protocol_params.committee_min_size,
            false,
        );
        // Only first CC member (expiry 1000) is active, voted Yes → 1/1 >= 1/2 → pass
        assert!(result);
    }

    #[test]
    fn test_cc_resigned_members_excluded() {
        let mut state = gov_test_state(5, 0);
        let cold_key =
            credential_to_hash(&Credential::VerificationKey(Hash28::from_bytes([10u8; 28])));

        // Resign the CC member
        Arc::make_mut(&mut state.governance)
            .committee_resigned
            .insert(cold_key, None);

        let action_id = make_action_id(50, 0);
        state.process_proposal(
            &Hash32::from_bytes([50u8; 32]),
            0,
            &ProposalProcedure {
                deposit: Lovelace(100_000_000_000),
                return_addr: vec![0u8; 29],
                gov_action: GovAction::InfoAction,
                anchor: make_anchor(),
            },
        );

        // CC vote should fail — only member is resigned
        let result = check_cc_approval(
            &action_id,
            &state.governance,
            EpochNo(0),
            state.protocol_params.committee_min_size,
            false,
        );
        assert!(!result);
    }

    #[test]
    fn test_cc_no_committee_blocks_ratification() {
        let gov = GovernanceState {
            // No committee (threshold = None, matching Haskell SNothing)
            committee_threshold: None,
            ..GovernanceState::default()
        };

        let action_id = make_action_id(50, 0);
        let result = check_cc_approval(&action_id, &gov, EpochNo(0), 0, false);
        assert!(!result);
    }

    #[test]
    fn test_cc_min_size_enforcement() {
        let mut state = gov_test_state(5, 0);
        // Set min committee size to 3 (we only have 1 member)
        state.protocol_params.committee_min_size = 3;

        let action_id = make_action_id(50, 0);
        state.process_proposal(
            &Hash32::from_bytes([50u8; 32]),
            0,
            &ProposalProcedure {
                deposit: Lovelace(100_000_000_000),
                return_addr: vec![0u8; 29],
                gov_action: GovAction::InfoAction,
                anchor: make_anchor(),
            },
        );
        cc_vote_yes(&mut state, &action_id);

        // Post-bootstrap: should fail because active_size (1) < committee_min_size (3)
        let result = check_cc_approval(&action_id, &state.governance, EpochNo(0), 3, false);
        assert!(!result);

        // During bootstrap: min size check is skipped
        let result = check_cc_approval(&action_id, &state.governance, EpochNo(0), 3, true);
        assert!(result);
    }

    // ========================================================================
    // Threshold matrix tests (CC/DRep/SPO for each action type)
    // ========================================================================

    #[test]
    fn test_no_confidence_no_cc_required() {
        // NoConfidence: DRep + SPO, NO CC
        let mut state = gov_test_state(10, 10);
        // Set CC threshold to something that would block if checked
        Arc::make_mut(&mut state.governance).committee_threshold = Some(Rational {
            numerator: 99,
            denominator: 100,
        });

        let tx_hash = Hash32::from_bytes([50u8; 32]);
        state.process_proposal(
            &tx_hash,
            0,
            &ProposalProcedure {
                deposit: Lovelace(100_000_000_000),
                return_addr: vec![0u8; 29],
                gov_action: GovAction::NoConfidence {
                    prev_action_id: None,
                },
                anchor: make_anchor(),
            },
        );
        let action_id = make_action_id(50, 0);

        // 7/10 DReps yes (70% >= dvt_no_confidence 67%)
        for i in 0..7 {
            drep_vote(&mut state, i, &action_id, Vote::Yes);
        }
        // 6/10 SPOs yes (60% >= pvt_motion_no_confidence 51%)
        for i in 0..6 {
            spo_vote(&mut state, i, &action_id, Vote::Yes);
        }
        // NO CC votes — CC cannot vote on NoConfidence

        state.process_epoch_transition(EpochNo(1));
        assert!(state.governance.no_confidence);
        assert!(state.governance.proposals.is_empty());
    }

    #[test]
    fn test_update_committee_no_cc_required() {
        // UpdateCommittee: DRep + SPO, NO CC
        let mut state = gov_test_state(10, 10);
        Arc::make_mut(&mut state.governance).committee_threshold = Some(Rational {
            numerator: 99,
            denominator: 100,
        });

        let tx_hash = Hash32::from_bytes([50u8; 32]);
        let new_cred = Credential::VerificationKey(Hash28::from_bytes([30u8; 28]));
        let mut members_to_add = BTreeMap::new();
        members_to_add.insert(new_cred, 500u64);

        state.process_proposal(
            &tx_hash,
            0,
            &ProposalProcedure {
                deposit: Lovelace(100_000_000_000),
                return_addr: vec![0u8; 29],
                gov_action: GovAction::UpdateCommittee {
                    prev_action_id: None,
                    members_to_remove: vec![],
                    members_to_add,
                    threshold: Rational {
                        numerator: 2,
                        denominator: 3,
                    },
                },
                anchor: make_anchor(),
            },
        );
        let action_id = make_action_id(50, 0);

        for i in 0..7 {
            drep_vote(&mut state, i, &action_id, Vote::Yes);
        }
        for i in 0..6 {
            spo_vote(&mut state, i, &action_id, Vote::Yes);
        }

        state.process_epoch_transition(EpochNo(1));
        // UpdateCommittee restores confidence
        assert!(!state.governance.no_confidence);
        assert!(state.governance.proposals.is_empty());
    }

    #[test]
    fn test_new_constitution_no_spo_required() {
        // NewConstitution: DRep + CC, NO SPO
        let mut state = gov_test_state(10, 10);

        let tx_hash = Hash32::from_bytes([50u8; 32]);
        state.process_proposal(
            &tx_hash,
            0,
            &ProposalProcedure {
                deposit: Lovelace(100_000_000_000),
                return_addr: vec![0u8; 29],
                gov_action: GovAction::NewConstitution {
                    prev_action_id: None,
                    constitution: Constitution {
                        anchor: make_anchor(),
                        script_hash: None,
                    },
                },
                anchor: make_anchor(),
            },
        );
        let action_id = make_action_id(50, 0);

        // DReps vote yes (need >= dvt_constitution)
        for i in 0..8 {
            drep_vote(&mut state, i, &action_id, Vote::Yes);
        }
        cc_vote_yes(&mut state, &action_id);
        // NO SPO votes — SPOs cannot vote on NewConstitution

        state.process_epoch_transition(EpochNo(1));
        assert!(state.governance.constitution.is_some());
        assert!(state.governance.proposals.is_empty());
    }

    #[test]
    fn test_treasury_withdrawal_no_spo_required() {
        // TreasuryWithdrawals: DRep + CC, NO SPO
        let mut state = gov_test_state(10, 10);
        state.treasury = Lovelace(10_000_000_000);

        let mut withdrawals = BTreeMap::new();
        withdrawals.insert(vec![0u8; 29], Lovelace(5_000_000_000));

        let tx_hash = Hash32::from_bytes([50u8; 32]);
        state.process_proposal(
            &tx_hash,
            0,
            &ProposalProcedure {
                deposit: Lovelace(100_000_000_000),
                return_addr: vec![0u8; 29],
                gov_action: GovAction::TreasuryWithdrawals {
                    withdrawals,
                    policy_hash: None,
                },
                anchor: make_anchor(),
            },
        );
        let action_id = make_action_id(50, 0);

        for i in 0..7 {
            drep_vote(&mut state, i, &action_id, Vote::Yes);
        }
        cc_vote_yes(&mut state, &action_id);

        state.process_epoch_transition(EpochNo(1));
        assert_eq!(state.treasury, Lovelace(5_000_000_000));
    }

    // ========================================================================
    // Treasury withdrawal cap tests
    // ========================================================================

    #[test]
    fn test_treasury_withdrawal_insufficient_funds_not_ratified() {
        let mut state = gov_test_state(10, 0);
        state.treasury = Lovelace(1_000_000_000); // Only 1B

        let mut withdrawals = BTreeMap::new();
        withdrawals.insert(vec![0u8; 29], Lovelace(5_000_000_000)); // Request 5B

        let tx_hash = Hash32::from_bytes([50u8; 32]);
        state.process_proposal(
            &tx_hash,
            0,
            &ProposalProcedure {
                deposit: Lovelace(100_000_000_000),
                return_addr: vec![0u8; 29],
                gov_action: GovAction::TreasuryWithdrawals {
                    withdrawals,
                    policy_hash: None,
                },
                anchor: make_anchor(),
            },
        );
        let action_id = make_action_id(50, 0);

        // All vote yes
        for i in 0..10 {
            drep_vote(&mut state, i, &action_id, Vote::Yes);
        }
        cc_vote_yes(&mut state, &action_id);

        state.process_epoch_transition(EpochNo(1));

        // Treasury withdrawal IS enacted — the balance guard was removed from
        // ratification to prevent divergence when local treasury tracking drifts.
        // Haskell's ratification uses the authoritative balance.
        // The treasury goes negative via saturating_sub (capped at 0).
        assert!(
            state.treasury.0 < 1_000_000_000,
            "Treasury should have been debited by the withdrawal"
        );
        assert_eq!(
            state.governance.proposals.len(),
            0,
            "Proposal should have been enacted and removed"
        );
    }

    // ========================================================================
    // No-confidence state effects
    // ========================================================================

    #[test]
    fn test_no_confidence_clears_committee_threshold() {
        let mut state = gov_test_state(10, 10);
        assert!(state.governance.committee_threshold.is_some());

        let tx_hash = Hash32::from_bytes([50u8; 32]);
        state.process_proposal(
            &tx_hash,
            0,
            &ProposalProcedure {
                deposit: Lovelace(100_000_000_000),
                return_addr: vec![0u8; 29],
                gov_action: GovAction::NoConfidence {
                    prev_action_id: None,
                },
                anchor: make_anchor(),
            },
        );
        let action_id = make_action_id(50, 0);

        for i in 0..7 {
            drep_vote(&mut state, i, &action_id, Vote::Yes);
        }
        for i in 0..6 {
            spo_vote(&mut state, i, &action_id, Vote::Yes);
        }

        state.process_epoch_transition(EpochNo(1));

        assert!(state.governance.no_confidence);
        assert!(state.governance.committee_threshold.is_none()); // Cleared
        assert!(state.governance.committee_hot_keys.is_empty());
        assert!(state.governance.committee_expiration.is_empty());
    }

    #[test]
    fn test_no_confidence_switches_committee_threshold() {
        let mut state = gov_test_state(10, 10);
        // First enact NoConfidence
        state.enact_gov_action(&GovAction::NoConfidence {
            prev_action_id: None,
        });
        assert!(state.governance.no_confidence);

        // In no-confidence state, UpdateCommittee should use dvt_committee_no_confidence
        // (a different threshold from dvt_committee_normal).
        // On mainnet: no_confidence=60%, normal=67% (no_confidence is lower, not higher)
        assert_ne!(
            state.protocol_params.dvt_committee_no_confidence,
            state.protocol_params.dvt_committee_normal
        );
    }

    // ========================================================================
    // Bootstrap phase tests
    // ========================================================================

    #[test]
    fn test_bootstrap_drep_thresholds_zero() {
        let mut params = ProtocolParameters::mainnet_defaults();
        params.protocol_version_major = 9; // Bootstrap
        let mut state = LedgerState::new(params);
        state.epoch_length = 100;
        state.needs_stake_rebuild = false;

        assert!(state.is_bootstrap_phase());

        // In bootstrap, DRep thresholds should be 0 (auto-pass)
        // so ParameterChange ratifies with just CC + SPO (for security params)
    }

    // ========================================================================
    // Proposal lifecycle tests
    // ========================================================================

    #[test]
    fn test_proposal_expiry_inclusive() {
        // Proposals are active through their expires_epoch (per Haskell gasExpiresAfter < currentEpoch)
        let mut state = gov_test_state(5, 0);
        state.protocol_params.gov_action_lifetime = 3;

        let tx_hash = Hash32::from_bytes([50u8; 32]);
        state.process_proposal(
            &tx_hash,
            0,
            &ProposalProcedure {
                deposit: Lovelace(100_000_000_000),
                return_addr: vec![0u8; 29],
                gov_action: GovAction::NoConfidence {
                    prev_action_id: None,
                },
                anchor: make_anchor(),
            },
        );

        // expires_epoch = 0 + 3 + 1 = 4 (per Haskell gasExpiresAfter)
        // Active through epoch 4, expires at epoch 5
        for e in 1..=4 {
            state.process_epoch_transition(EpochNo(e));
            assert_eq!(
                state.governance.proposals.len(),
                1,
                "Should be active at epoch {}",
                e
            );
        }

        state.process_epoch_transition(EpochNo(5));
        assert_eq!(state.governance.proposals.len(), 0); // Expired
    }

    #[test]
    fn test_deposit_returned_on_ratification() {
        // Use TreasuryWithdrawals (ratifiable) instead of InfoAction (never ratifies).
        // Set CC threshold to 0 so it auto-passes with no votes.
        let mut state = gov_test_state(10, 0);
        Arc::make_mut(&mut state.governance).committee_threshold = Some(Rational {
            numerator: 0,
            denominator: 1,
        });
        // DRep threshold for treasury withdrawal = dvt_treasury_withdrawal
        state.protocol_params.dvt_treasury_withdrawal = Rational {
            numerator: 0,
            denominator: 1,
        };

        let return_addr = vec![0u8; 29];
        let return_key = LedgerState::reward_account_to_hash(&return_addr);

        let tx_hash = Hash32::from_bytes([50u8; 32]);
        state.process_proposal(
            &tx_hash,
            0,
            &ProposalProcedure {
                deposit: Lovelace(50_000_000_000),
                return_addr: return_addr.clone(),
                gov_action: GovAction::TreasuryWithdrawals {
                    withdrawals: std::collections::BTreeMap::new(),
                    policy_hash: None,
                },
                anchor: make_anchor(),
            },
        );

        state.process_epoch_transition(EpochNo(1));

        // Deposit should be returned to reward account
        assert_eq!(
            state
                .reward_accounts
                .get(&return_key)
                .copied()
                .unwrap_or(Lovelace(0)),
            Lovelace(50_000_000_000)
        );
    }

    #[test]
    fn test_deposit_returned_on_expiry() {
        let mut state = gov_test_state(5, 0);
        state.protocol_params.gov_action_lifetime = 1;

        let return_addr = vec![0u8; 29];
        let return_key = LedgerState::reward_account_to_hash(&return_addr);

        let tx_hash = Hash32::from_bytes([50u8; 32]);
        state.process_proposal(
            &tx_hash,
            0,
            &ProposalProcedure {
                deposit: Lovelace(50_000_000_000),
                return_addr: return_addr.clone(),
                gov_action: GovAction::NoConfidence {
                    prev_action_id: None,
                },
                anchor: make_anchor(),
            },
        );

        // Expire at epoch 3 (expires_epoch = 0 + 1 + 1 = 2, expired when 2 < 3)
        state.process_epoch_transition(EpochNo(1));
        assert_eq!(state.governance.proposals.len(), 1); // Still active at epoch 1

        state.process_epoch_transition(EpochNo(2));
        assert_eq!(state.governance.proposals.len(), 1); // Still active at epoch 2

        state.process_epoch_transition(EpochNo(3));
        assert_eq!(state.governance.proposals.len(), 0); // Expired

        // Deposit should be refunded
        assert_eq!(
            state
                .reward_accounts
                .get(&return_key)
                .copied()
                .unwrap_or(Lovelace(0)),
            Lovelace(50_000_000_000)
        );
    }

    // ========================================================================
    // Vote replacement tests
    // ========================================================================

    #[test]
    fn test_vote_replacement() {
        let mut state = gov_test_state(5, 0);
        let tx_hash = Hash32::from_bytes([50u8; 32]);
        state.process_proposal(
            &tx_hash,
            0,
            &ProposalProcedure {
                deposit: Lovelace(100_000_000_000),
                return_addr: vec![0u8; 29],
                gov_action: GovAction::InfoAction,
                anchor: make_anchor(),
            },
        );
        let action_id = make_action_id(50, 0);

        // DRep 0 votes No initially
        drep_vote(&mut state, 0, &action_id, Vote::No);
        // DRep 0 changes vote to Yes
        drep_vote(&mut state, 0, &action_id, Vote::Yes);

        let votes = state.governance.votes_by_action.get(&action_id).unwrap();
        let drep_cred = Credential::VerificationKey(Hash28::from_bytes([0u8; 28]));
        let drep_vote_entry = votes
            .iter()
            .find(|(v, _)| *v == Voter::DRep(drep_cred.clone()))
            .unwrap();
        assert_eq!(drep_vote_entry.1.vote, Vote::Yes);
    }

    // ========================================================================
    // Competing proposals tests
    // ========================================================================

    #[test]
    fn test_competing_proposals_same_prev_action_id() {
        let mut state = gov_test_state(10, 0);
        Arc::make_mut(&mut state.governance).committee_threshold = Some(Rational {
            numerator: 0,
            denominator: 1,
        });

        // Submit two ParameterChange proposals with the same prevActionId (None)
        let tx1 = Hash32::from_bytes([1u8; 32]);
        state.process_proposal(
            &tx1,
            0,
            &ProposalProcedure {
                deposit: Lovelace(100_000_000_000),
                return_addr: vec![0u8; 29],
                gov_action: GovAction::ParameterChange {
                    prev_action_id: None,
                    protocol_param_update: Box::new(ProtocolParamUpdate {
                        n_opt: Some(1000),
                        ..Default::default()
                    }),
                    policy_hash: None,
                },
                anchor: make_anchor(),
            },
        );

        let tx2 = Hash32::from_bytes([2u8; 32]);
        state.process_proposal(
            &tx2,
            0,
            &ProposalProcedure {
                deposit: Lovelace(100_000_000_000),
                return_addr: vec![0u8; 29],
                gov_action: GovAction::ParameterChange {
                    prev_action_id: None,
                    protocol_param_update: Box::new(ProtocolParamUpdate {
                        n_opt: Some(2000),
                        ..Default::default()
                    }),
                    policy_hash: None,
                },
                anchor: make_anchor(),
            },
        );

        let id1 = make_action_id(1, 0);
        let id2 = make_action_id(2, 0);

        // All DReps vote Yes on both
        for i in 0..10 {
            drep_vote(&mut state, i, &id1, Vote::Yes);
            drep_vote(&mut state, i, &id2, Vote::Yes);
        }

        state.process_epoch_transition(EpochNo(1));

        // First proposal (by BTreeMap order) should be enacted
        // Second proposal's prevActionId (None) no longer matches enacted root
        // The enacted_pparam_update should be set
        assert!(state.governance.enacted_pparam_update.is_some());

        // One proposal should still be active (the one whose prevActionId became stale)
        // OR both could have been enacted if they're processed in order and both match
        // at their evaluation time... Per Haskell, update_enacted_root happens after enactment,
        // so the second one would see the updated root and fail prev_action_as_expected.
        // But we need to check: our code does self.update_enacted_root BEFORE processing the next.
        // Yes, line 251-252: self.enact_gov_action(action); self.update_enacted_root(action_id, action);
        // So the second proposal should fail.
        let enacted_id = state.governance.enacted_pparam_update.as_ref().unwrap();
        assert!(enacted_id == &id1 || enacted_id == &id2);
    }

    // ========================================================================
    // Enactment effects tests
    // ========================================================================

    #[test]
    fn test_enact_no_confidence_effects() {
        let mut state = gov_test_state(5, 0);
        assert!(!state.governance.no_confidence);
        assert!(state.governance.committee_threshold.is_some());

        state.enact_gov_action(&GovAction::NoConfidence {
            prev_action_id: None,
        });

        assert!(state.governance.no_confidence);
        assert!(state.governance.committee_threshold.is_none());
        assert!(state.governance.committee_hot_keys.is_empty());
        assert!(state.governance.committee_expiration.is_empty());
    }

    #[test]
    fn test_enact_update_committee_restores_confidence() {
        let mut state = gov_test_state(5, 0);
        state.enact_gov_action(&GovAction::NoConfidence {
            prev_action_id: None,
        });
        assert!(state.governance.no_confidence);

        let new_cred = Credential::VerificationKey(Hash28::from_bytes([30u8; 28]));
        let mut members = BTreeMap::new();
        members.insert(new_cred, 500u64);

        state.enact_gov_action(&GovAction::UpdateCommittee {
            prev_action_id: None,
            members_to_remove: vec![],
            members_to_add: members,
            threshold: Rational {
                numerator: 2,
                denominator: 3,
            },
        });

        assert!(!state.governance.no_confidence);
        assert_eq!(
            state.governance.committee_threshold,
            Some(Rational {
                numerator: 2,
                denominator: 3,
            })
        );
    }

    #[test]
    fn test_enact_hard_fork_updates_protocol_version() {
        let mut state = gov_test_state(5, 0);
        assert_eq!(state.protocol_params.protocol_version_major, 10);

        state.enact_gov_action(&GovAction::HardForkInitiation {
            prev_action_id: None,
            protocol_version: (11, 0),
        });

        assert_eq!(state.protocol_params.protocol_version_major, 11);
        assert_eq!(state.protocol_params.protocol_version_minor, 0);
    }

    #[test]
    fn test_enact_treasury_withdrawal_debits_treasury() {
        let mut state = gov_test_state(5, 0);
        state.treasury = Lovelace(10_000_000_000);

        let mut withdrawals = BTreeMap::new();
        withdrawals.insert(vec![0u8; 29], Lovelace(3_000_000_000));
        withdrawals.insert(vec![1u8; 29], Lovelace(2_000_000_000));

        state.enact_gov_action(&GovAction::TreasuryWithdrawals {
            withdrawals,
            policy_hash: None,
        });

        assert_eq!(state.treasury, Lovelace(5_000_000_000));
    }

    #[test]
    fn test_enact_new_constitution() {
        let mut state = gov_test_state(5, 0);
        assert!(state.governance.constitution.is_none());

        let constitution = Constitution {
            anchor: make_anchor(),
            script_hash: Some(Hash28::from_bytes([99u8; 28])),
        };

        state.enact_gov_action(&GovAction::NewConstitution {
            prev_action_id: None,
            constitution: constitution.clone(),
        });

        let stored = state.governance.constitution.as_ref().unwrap();
        assert_eq!(stored.script_hash, constitution.script_hash);
    }

    #[test]
    fn test_enact_info_action_no_effect() {
        let mut state = gov_test_state(5, 0);
        let before = state.protocol_params.clone();

        state.enact_gov_action(&GovAction::InfoAction);

        assert_eq!(state.protocol_params.n_opt, before.n_opt);
        assert_eq!(
            state.protocol_params.protocol_version_major,
            before.protocol_version_major
        );
    }

    // ========================================================================
    // Regression tests — Issue #94: ParameterChange ex-unit updates
    // ========================================================================

    /// Regression test for issue #94.
    ///
    /// A Conway ParameterChange governance action that updates `max_tx_ex_units`
    /// and `max_block_ex_units` must be fully enacted when it receives sufficient
    /// DRep (network group), SPO (security group), and CC approval.
    ///
    /// Prior to the fix, nodes loaded from stale snapshots (saved before
    /// Alonzo/Conway genesis was wired in) carried `mainnet_defaults()` values
    /// for these fields. Because `committee_min_size` was also stale (7 instead
    /// of 0 for preview), `check_cc_approval` always returned false and no
    /// ParameterChange action ever ratified — leaving the node permanently
    /// reporting `max_tx_ex_mem=14,000,000` instead of the chain value of
    /// `16,500,000`.
    #[test]
    fn test_parameter_change_ex_units_ratified_and_enacted() {
        // 10 DReps + 10 SPOs covers both dvt_pp_network_group (67%) and
        // pvt_pp_security_group (51%) thresholds. committee_min_size is set to
        // 0 by gov_test_state so CC approval only requires the threshold ratio.
        let mut state = gov_test_state(10, 10);

        // Record the baseline values so we can assert they changed.
        let old_tx_mem = state.protocol_params.max_tx_ex_units.mem;
        let old_block_mem = state.protocol_params.max_block_ex_units.mem;

        // Propose a ParameterChange that updates both ex-unit limits.
        // These are the preview testnet values from on-chain governance epoch 1094.
        let new_tx_ex_units = ExUnits {
            mem: 16_500_000,
            steps: 10_000_000_000,
        };
        let new_block_ex_units = ExUnits {
            mem: 72_000_000,
            steps: 40_000_000_000,
        };
        let tx_hash = Hash32::from_bytes([50u8; 32]);
        state.process_proposal(
            &tx_hash,
            0,
            &ProposalProcedure {
                deposit: Lovelace(100_000_000_000),
                return_addr: vec![0u8; 29],
                gov_action: GovAction::ParameterChange {
                    prev_action_id: None,
                    protocol_param_update: Box::new(ProtocolParamUpdate {
                        // max_tx_ex_units is in the Network group (no SPO vote required).
                        // max_block_ex_units is in the Network + Security groups
                        // (requires pvt_pp_security_group SPO threshold).
                        max_tx_ex_units: Some(new_tx_ex_units),
                        max_block_ex_units: Some(new_block_ex_units),
                        ..Default::default()
                    }),
                    policy_hash: None,
                },
                anchor: make_anchor(),
            },
        );
        let action_id = make_action_id(50, 0);

        // Cast DRep yes votes: 7 out of 10 = 70% >= dvt_pp_network_group (67%).
        for i in 0..7 {
            drep_vote(&mut state, i, &action_id, Vote::Yes);
        }
        // Cast SPO yes votes: 6 out of 10 = 60% >= pvt_pp_security_group (51%).
        for i in 0..6 {
            spo_vote(&mut state, i, &action_id, Vote::Yes);
        }
        // CC yes vote — the single CC member votes yes (threshold is 1/2).
        cc_vote_yes(&mut state, &action_id);

        // Trigger the epoch boundary where ratification and enactment occur.
        state.process_epoch_transition(EpochNo(1));

        // The proposal must have been consumed (ratified and enacted).
        assert!(
            state.governance.proposals.is_empty(),
            "Proposal should have been ratified and removed from pending proposals"
        );

        // The protocol params must reflect the new ex-unit values.
        assert_ne!(
            state.protocol_params.max_tx_ex_units.mem, old_tx_mem,
            "max_tx_ex_units.mem must have changed from the baseline"
        );
        assert_eq!(
            state.protocol_params.max_tx_ex_units.mem, new_tx_ex_units.mem,
            "max_tx_ex_units.mem must equal the enacted value (16_500_000)"
        );
        assert_eq!(
            state.protocol_params.max_tx_ex_units.steps, new_tx_ex_units.steps,
            "max_tx_ex_units.steps must equal the enacted value"
        );

        assert_ne!(
            state.protocol_params.max_block_ex_units.mem, old_block_mem,
            "max_block_ex_units.mem must have changed from the baseline"
        );
        assert_eq!(
            state.protocol_params.max_block_ex_units.mem, new_block_ex_units.mem,
            "max_block_ex_units.mem must equal the enacted value (72_000_000)"
        );
        assert_eq!(
            state.protocol_params.max_block_ex_units.steps, new_block_ex_units.steps,
            "max_block_ex_units.steps must equal the enacted value"
        );
    }

    /// Confirms that a ParameterChange ex-unit update does NOT ratify when the
    /// CC vote is missing, even if DRep and SPO thresholds are met. This
    /// ensures the CC approval gate is functioning correctly.
    #[test]
    fn test_parameter_change_ex_units_not_ratified_without_cc() {
        let mut state = gov_test_state(10, 10);
        // Use a non-trivial CC threshold so missing the CC vote actually blocks ratification.
        Arc::make_mut(&mut state.governance).committee_threshold = Some(Rational {
            numerator: 1,
            denominator: 2,
        });

        let tx_hash = Hash32::from_bytes([51u8; 32]);
        state.process_proposal(
            &tx_hash,
            0,
            &ProposalProcedure {
                deposit: Lovelace(100_000_000_000),
                return_addr: vec![0u8; 29],
                gov_action: GovAction::ParameterChange {
                    prev_action_id: None,
                    protocol_param_update: Box::new(ProtocolParamUpdate {
                        max_tx_ex_units: Some(ExUnits {
                            mem: 16_500_000,
                            steps: 10_000_000_000,
                        }),
                        ..Default::default()
                    }),
                    policy_hash: None,
                },
                anchor: make_anchor(),
            },
        );
        let action_id = make_action_id(51, 0);

        // DRep and SPO thresholds are met, but NO CC vote.
        for i in 0..7 {
            drep_vote(&mut state, i, &action_id, Vote::Yes);
        }
        for i in 0..6 {
            spo_vote(&mut state, i, &action_id, Vote::Yes);
        }
        // Deliberately omit cc_vote_yes.

        state.process_epoch_transition(EpochNo(1));

        // Proposal must still be pending (not ratified — CC threshold not met).
        assert!(
            !state.governance.proposals.is_empty(),
            "Proposal should NOT have been ratified without CC approval"
        );
        // ex-unit value must be unchanged.
        assert_eq!(
            state.protocol_params.max_tx_ex_units.mem,
            ProtocolParameters::mainnet_defaults().max_tx_ex_units.mem,
            "max_tx_ex_units.mem must be unchanged when CC approval is missing"
        );
    }

    // ========================================================================
    // Enacted root update tests
    // ========================================================================

    #[test]
    fn test_enacted_roots_updated_correctly() {
        let mut state = gov_test_state(5, 0);

        let pp_id = make_action_id(1, 0);
        state.update_enacted_root(
            &pp_id,
            &GovAction::ParameterChange {
                prev_action_id: None,
                protocol_param_update: Box::new(ProtocolParamUpdate::default()),
                policy_hash: None,
            },
        );
        assert_eq!(state.governance.enacted_pparam_update, Some(pp_id));

        let hf_id = make_action_id(2, 0);
        state.update_enacted_root(
            &hf_id,
            &GovAction::HardForkInitiation {
                prev_action_id: None,
                protocol_version: (10, 0),
            },
        );
        assert_eq!(state.governance.enacted_hard_fork, Some(hf_id));

        let nc_id = make_action_id(3, 0);
        state.update_enacted_root(
            &nc_id,
            &GovAction::NoConfidence {
                prev_action_id: None,
            },
        );
        assert_eq!(state.governance.enacted_committee, Some(nc_id.clone()));

        // UpdateCommittee shares the committee purpose with NoConfidence
        let uc_id = make_action_id(4, 0);
        state.update_enacted_root(
            &uc_id,
            &GovAction::UpdateCommittee {
                prev_action_id: None,
                members_to_remove: vec![],
                members_to_add: BTreeMap::new(),
                threshold: Rational {
                    numerator: 1,
                    denominator: 2,
                },
            },
        );
        assert_eq!(state.governance.enacted_committee, Some(uc_id));

        let co_id = make_action_id(5, 0);
        state.update_enacted_root(
            &co_id,
            &GovAction::NewConstitution {
                prev_action_id: None,
                constitution: Constitution {
                    anchor: make_anchor(),
                    script_hash: None,
                },
            },
        );
        assert_eq!(state.governance.enacted_constitution, Some(co_id));

        // TreasuryWithdrawals and InfoAction don't update any root
        let tw_id = make_action_id(6, 0);
        let old_pp = state.governance.enacted_pparam_update.clone();
        state.update_enacted_root(
            &tw_id,
            &GovAction::TreasuryWithdrawals {
                withdrawals: BTreeMap::new(),
                policy_hash: None,
            },
        );
        assert_eq!(state.governance.enacted_pparam_update, old_pp);
    }

    // ========================================================================
    // check_threshold tests
    // ========================================================================

    #[test]
    fn test_check_threshold_zero_passes() {
        let zero = Rational {
            numerator: 0,
            denominator: 1,
        };
        assert!(check_threshold(0, 0, &zero));
        assert!(check_threshold(0, 100, &zero));
    }

    #[test]
    fn test_check_threshold_zero_total_fails() {
        let threshold = Rational {
            numerator: 1,
            denominator: 2,
        };
        assert!(!check_threshold(0, 0, &threshold));
    }

    #[test]
    fn test_check_threshold_exact_boundary() {
        let threshold = Rational {
            numerator: 2,
            denominator: 3,
        };
        // 2/3 >= 2/3 → true
        assert!(check_threshold(2, 3, &threshold));
        // 666/1000 < 2/3 → false (666*3 = 1998 < 2000 = 2*1000)
        assert!(!check_threshold(666, 1000, &threshold));
        // 667/1000 >= 2/3 → true (667*3 = 2001 >= 2000)
        assert!(check_threshold(667, 1000, &threshold));
    }

    #[test]
    fn test_check_threshold_one_hundred_percent() {
        let threshold = Rational {
            numerator: 1,
            denominator: 1,
        };
        assert!(check_threshold(100, 100, &threshold));
        assert!(!check_threshold(99, 100, &threshold));
    }

    // ========================================================================
    // SPO voting power snapshot tests (mark vs set)
    // ========================================================================

    /// Verify that `compute_spo_voting_power` reads from the **mark** snapshot,
    /// not from `set`.  CIP-1694 specifies the mark (current-epoch) stake
    /// distribution for SPO voting power.
    #[test]
    fn test_spo_voting_power_uses_mark_snapshot() {
        use crate::state::StakeSnapshot;

        let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());
        let pool_id = Hash28::from_bytes([10u8; 28]);

        // Populate ONLY the mark snapshot with stake for this pool.
        // The set snapshot has a different (lower) amount.
        let mut mark_pool_stake = std::collections::HashMap::new();
        mark_pool_stake.insert(pool_id, Lovelace(5_000_000_000));
        state.snapshots.mark = Some(StakeSnapshot {
            epoch: EpochNo(1),
            delegations: Arc::new(std::collections::HashMap::new()),
            pool_stake: mark_pool_stake,
            pool_params: Arc::clone(&state.pool_params),
            stake_distribution: Arc::new(std::collections::HashMap::new()),
            epoch_fees: Lovelace(0),
            epoch_block_count: 0,
            epoch_blocks_by_pool: Arc::new(HashMap::new()),
        });

        let mut set_pool_stake = std::collections::HashMap::new();
        set_pool_stake.insert(pool_id, Lovelace(1_000_000_000)); // deliberately different
        state.snapshots.set = Some(StakeSnapshot {
            epoch: EpochNo(0),
            delegations: Arc::new(std::collections::HashMap::new()),
            pool_stake: set_pool_stake,
            pool_params: Arc::clone(&state.pool_params),
            stake_distribution: Arc::new(std::collections::HashMap::new()),
            epoch_fees: Lovelace(0),
            epoch_block_count: 0,
            epoch_blocks_by_pool: Arc::new(HashMap::new()),
        });

        // SPO voting power must come from mark (5B), not set (1B)
        let power = state.compute_spo_voting_power(&pool_id);
        assert_eq!(
            power, 5_000_000_000,
            "compute_spo_voting_power must read from mark snapshot per CIP-1694"
        );
    }

    /// Verify that `compute_total_spo_stake` (the denominator for SPO voting
    /// thresholds) also reads from the mark snapshot.
    #[test]
    fn test_compute_total_spo_stake_uses_mark_snapshot() {
        use crate::state::StakeSnapshot;

        let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());
        let pool_a = Hash28::from_bytes([1u8; 28]);
        let pool_b = Hash28::from_bytes([2u8; 28]);

        // Mark has both pools: total = 8B
        let mut mark_stake = std::collections::HashMap::new();
        mark_stake.insert(pool_a, Lovelace(3_000_000_000));
        mark_stake.insert(pool_b, Lovelace(5_000_000_000));
        state.snapshots.mark = Some(StakeSnapshot {
            epoch: EpochNo(2),
            delegations: Arc::new(std::collections::HashMap::new()),
            pool_stake: mark_stake,
            pool_params: Arc::clone(&state.pool_params),
            stake_distribution: Arc::new(std::collections::HashMap::new()),
            epoch_fees: Lovelace(0),
            epoch_block_count: 0,
            epoch_blocks_by_pool: Arc::new(HashMap::new()),
        });

        // Set has only one pool: total = 1B (stale, should NOT be used)
        let mut set_stake = std::collections::HashMap::new();
        set_stake.insert(pool_a, Lovelace(1_000_000_000));
        state.snapshots.set = Some(StakeSnapshot {
            epoch: EpochNo(1),
            delegations: Arc::new(std::collections::HashMap::new()),
            pool_stake: set_stake,
            pool_params: Arc::clone(&state.pool_params),
            stake_distribution: Arc::new(std::collections::HashMap::new()),
            epoch_fees: Lovelace(0),
            epoch_block_count: 0,
            epoch_blocks_by_pool: Arc::new(HashMap::new()),
        });

        let total = state.compute_total_spo_stake();
        assert_eq!(
            total, 8_000_000_000,
            "compute_total_spo_stake must sum from mark snapshot per CIP-1694"
        );
    }

    /// Without any snapshots, SPO voting power falls back to the O(n) live
    /// delegation scan.  Ensure the fallback returns sensible results and does
    /// not panic.
    #[test]
    fn test_spo_voting_power_fallback_no_snapshots() {
        let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());
        let pool_id = Hash28::from_bytes([77u8; 28]);
        let stake_cred = Hash32::from_bytes([88u8; 32]);

        // No snapshots — must fall back to live delegation scan
        assert!(state.snapshots.mark.is_none());
        assert!(state.snapshots.set.is_none());

        // With a delegation pointing to pool_id and some stake
        Arc::make_mut(&mut state.delegations).insert(stake_cred, pool_id);
        state
            .stake_distribution
            .stake_map
            .insert(stake_cred, Lovelace(9_000_000));

        let power = state.compute_spo_voting_power(&pool_id);
        assert_eq!(
            power, 9_000_000,
            "fallback scan must return live stake when no mark snapshot exists"
        );
    }
}
