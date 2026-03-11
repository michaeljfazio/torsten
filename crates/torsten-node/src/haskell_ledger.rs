//! Load and convert a Haskell ledger state snapshot into Torsten's `LedgerState`.
//!
//! When a Mithril snapshot includes the raw Haskell-encoded ledger state (in
//! `haskell-ledger/`), this module parses it and populates all non-UTxO fields
//! of the ledger: delegations, pool registrations, reward accounts, governance,
//! stake snapshots, treasury, and reserves.  UTxO is left empty because
//! MemPack encoding is not yet fully supported; the UTxO set is rebuilt
//! during subsequent block replay.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;

use torsten_ledger::state::{
    DRepRegistration, EpochSnapshots, GovernanceState, PoolRegistration, ProposalState,
    StakeDistributionState, StakeSnapshot,
};
use torsten_ledger::LedgerState;
use torsten_primitives::block::{Point, Tip};
use torsten_primitives::credentials::Credential;
use torsten_primitives::hash::{Hash28, Hash32};
use torsten_primitives::time::{BlockNo, EpochNo, SlotNo};
use torsten_primitives::transaction::{GovAction, ProposalProcedure, Vote, Voter, VotingProcedure};
use torsten_primitives::value::Lovelace;
use torsten_serialization::haskell_state::convert::*;
use torsten_serialization::haskell_state::parse_haskell_new_epoch_state;
use torsten_serialization::haskell_state::types::*;
use tracing::info;

/// Attempt to load and parse a Haskell ledger state from the
/// `haskell-ledger/` directory.  Returns the converted `LedgerState` and
/// the slot number extracted from the filename.
pub fn load_haskell_ledger_state(
    haskell_ledger_dir: &Path,
    _protocol_params_override: Option<&torsten_primitives::protocol_params::ProtocolParameters>,
) -> Result<(LedgerState, SlotNo), Box<dyn std::error::Error>> {
    // Find the latest ledger file (highest slot number)
    let (file_path, slot, _hash) = find_latest_ledger_file(haskell_ledger_dir)?;

    info!(
        slot = slot.0,
        file = %file_path.display(),
        "Loading Haskell ledger state"
    );

    // Read and parse
    let data = std::fs::read(&file_path)?;
    let nes = parse_haskell_new_epoch_state(&data)?;

    // Convert to LedgerState
    let ledger = convert_to_ledger_state(&nes, slot)?;

    info!(
        epoch = ledger.epoch.0,
        treasury = ledger.treasury.0,
        reserves = ledger.reserves.0,
        pools = ledger.pool_params.len(),
        delegations = ledger.delegations.len(),
        rewards = ledger.reward_accounts.len(),
        "Haskell ledger state loaded successfully"
    );

    Ok((ledger, slot))
}

/// Scan a directory for ledger state files named `<slot>_<hash>` and return
/// the one with the highest slot number.
fn find_latest_ledger_file(
    dir: &Path,
) -> Result<(std::path::PathBuf, SlotNo, String), Box<dyn std::error::Error>> {
    let mut latest: Option<(std::path::PathBuf, u64, String)> = None;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if let Some((slot_str, hash)) = name.split_once('_') {
            if let Ok(slot) = slot_str.parse::<u64>() {
                if latest.as_ref().is_none_or(|(_, s, _)| slot > *s) {
                    latest = Some((entry.path(), slot, hash.to_string()));
                }
            }
        }
    }
    latest
        .map(|(p, s, h)| (p, SlotNo(s), h))
        .ok_or_else(|| "No ledger state files found".into())
}

// ---------------------------------------------------------------------------
// Conversion
// ---------------------------------------------------------------------------

fn convert_to_ledger_state(
    nes: &HaskellNewEpochState,
    slot: SlotNo,
) -> Result<LedgerState, Box<dyn std::error::Error>> {
    let es = &nes.epoch_state;
    let ls = &es.ledger_state;
    let cs = &ls.cert_state;
    let us = &ls.utxo_state;

    // Convert protocol parameters from the gov state
    let params = convert_pparams(&us.gov_state.cur_pparams);

    // Start with a fresh LedgerState and populate all fields
    let mut state = LedgerState::new(params);

    // Basic state
    state.epoch = nes.epoch_no;
    state.treasury = es.treasury;
    state.reserves = es.reserves;
    state.epoch_fees = us.fees;
    state.era = torsten_primitives::era::Era::Conway;

    // Tip -- we know the slot from the filename but not the block hash / number
    state.tip = Tip {
        point: Point::Specific(slot, Hash32::ZERO),
        block_number: BlockNo(0),
    };

    // -----------------------------------------------------------------------
    // Pool parameters from PState
    // -----------------------------------------------------------------------
    let mut pool_params_map = HashMap::new();
    for (pool_id, hp) in &cs.pstate.stake_pool_params {
        let cp = convert_pool_params(hp);
        pool_params_map.insert(
            *pool_id,
            PoolRegistration {
                pool_id: cp.pool_id,
                vrf_keyhash: cp.vrf_keyhash,
                pledge: cp.pledge,
                cost: cp.cost,
                margin_numerator: cp.margin_numerator,
                margin_denominator: cp.margin_denominator,
                reward_account: cp.reward_account,
                owners: cp.owners,
                relays: cp.relays,
                metadata_url: cp.metadata_url,
                metadata_hash: cp.metadata_hash,
            },
        );
    }
    state.pool_params = Arc::new(pool_params_map);

    // -----------------------------------------------------------------------
    // Pending retirements from PState
    // -----------------------------------------------------------------------
    let mut pending_retirements: BTreeMap<EpochNo, Vec<Hash28>> = BTreeMap::new();
    for (pool_id, epoch) in &cs.pstate.retiring {
        pending_retirements
            .entry(*epoch)
            .or_default()
            .push(*pool_id);
    }
    state.pending_retirements = pending_retirements;

    // -----------------------------------------------------------------------
    // Delegations and reward accounts from DState
    // -----------------------------------------------------------------------
    let mut delegations = HashMap::new();
    let mut reward_accounts = HashMap::new();
    let mut vote_delegations = HashMap::new();

    for (cred, acct) in &cs.dstate.accounts {
        let cred_hash = credential_to_hash32(cred);
        reward_accounts.insert(cred_hash, acct.rewards);
        if let Some(pool_id) = &acct.pool_delegation {
            delegations.insert(cred_hash, *pool_id);
        }
        if let Some(drep) = &acct.drep_delegation {
            vote_delegations.insert(cred_hash, convert_drep(drep));
        }
    }
    state.delegations = Arc::new(delegations);
    state.reward_accounts = Arc::new(reward_accounts);

    // -----------------------------------------------------------------------
    // Blocks by pool (current epoch)
    // -----------------------------------------------------------------------
    let mut epoch_blocks = HashMap::new();
    for (pool_id, count) in &nes.blocks_made_cur {
        epoch_blocks.insert(*pool_id, *count);
    }
    let total_blocks: u64 = epoch_blocks.values().sum();
    state.epoch_blocks_by_pool = Arc::new(epoch_blocks);
    state.epoch_block_count = total_blocks;

    // -----------------------------------------------------------------------
    // Governance state
    // -----------------------------------------------------------------------
    let mut gov = convert_governance_state(&us.gov_state, &cs.vstate);
    gov.vote_delegations = vote_delegations;
    state.governance = Arc::new(gov);

    // -----------------------------------------------------------------------
    // Stake distribution from pool_distr
    // -----------------------------------------------------------------------
    let mut stake_map = HashMap::new();
    for (pool_id, ips) in &nes.pool_distr.individual_stakes {
        let cred_hash = pool_id.to_hash32_padded();
        stake_map.insert(cred_hash, ips.total_stake);
    }
    state.stake_distribution = StakeDistributionState { stake_map };

    // -----------------------------------------------------------------------
    // Snapshots
    // -----------------------------------------------------------------------
    state.snapshots = convert_snapshots(&es.snapshots);

    // UTxO set is empty -- will be rebuilt from block replay.
    // Mark state as needing stake rebuild after snapshot-based init.
    state.needs_stake_rebuild = true;

    Ok(state)
}

// ---------------------------------------------------------------------------
// Governance state conversion
// ---------------------------------------------------------------------------

fn convert_governance_state(
    gov: &HaskellConwayGovState,
    vstate: &HaskellVState,
) -> GovernanceState {
    let mut gs = GovernanceState::default();

    // DReps from VState
    for (cred, drep_state) in &vstate.dreps {
        let cred_hash = credential_to_hash32(cred);
        gs.dreps.insert(
            cred_hash,
            DRepRegistration {
                credential: haskell_cred_to_credential(cred),
                deposit: drep_state.deposit,
                anchor: drep_state.anchor.as_ref().map(convert_anchor),
                registered_epoch: EpochNo(0), // not available from Haskell state
                last_active_epoch: drep_state.expiry, // use expiry as proxy
                active: true,
            },
        );
    }

    // Committee from governance state
    if let Some(committee) = &gov.committee {
        for (cred, epoch) in &committee.members {
            let cred_hash = credential_to_hash32(cred);
            gs.committee_expiration.insert(cred_hash, *epoch);
        }
        gs.committee_threshold = Some(convert_rational(&committee.threshold));
    }

    // Committee hot keys and resignations from VState
    for (cred, auth) in &vstate.committee_state {
        let cred_hash = credential_to_hash32(cred);
        match auth {
            HaskellCommitteeAuth::HotCredential(hot) => {
                let hot_hash = credential_to_hash32(hot);
                gs.committee_hot_keys.insert(cred_hash, hot_hash);
            }
            HaskellCommitteeAuth::Resigned(anchor) => {
                gs.committee_resigned
                    .insert(cred_hash, anchor.as_ref().map(convert_anchor));
            }
        }
    }

    // Constitution
    gs.constitution = Some(convert_constitution(&gov.constitution));

    // Proposals from governance state
    for gas in &gov.proposals {
        let action_id = convert_gov_action_id(&gas.action_id);

        // Count votes
        let yes = count_votes_matching(&gas.drep_votes, is_yes)
            + count_votes_matching_spo(&gas.spo_votes, is_yes_haskell)
            + count_votes_matching(&gas.committee_votes, is_yes);
        let no = count_votes_matching(&gas.drep_votes, is_no)
            + count_votes_matching_spo(&gas.spo_votes, is_no_haskell)
            + count_votes_matching(&gas.committee_votes, is_no);
        let abstain = count_votes_matching(&gas.drep_votes, is_abstain)
            + count_votes_matching_spo(&gas.spo_votes, is_abstain_haskell)
            + count_votes_matching(&gas.committee_votes, is_abstain);

        gs.proposals.insert(
            action_id.clone(),
            ProposalState {
                procedure: convert_proposal_procedure(&gas.proposal),
                proposed_epoch: gas.proposed_in,
                expires_epoch: gas.expires_after,
                yes_votes: yes,
                no_votes: no,
                abstain_votes: abstain,
            },
        );

        // Individual votes
        let mut votes = Vec::new();
        for (cred, vote) in &gas.drep_votes {
            let voter = Voter::DRep(haskell_cred_to_credential(cred));
            votes.push((voter, convert_vote(vote)));
        }
        for (pool_id, vote) in &gas.spo_votes {
            let voter = Voter::StakePool(pool_id.to_hash32_padded());
            votes.push((voter, convert_vote(vote)));
        }
        for (cred, vote) in &gas.committee_votes {
            let voter = Voter::ConstitutionalCommittee(haskell_cred_to_credential(cred));
            votes.push((voter, convert_vote(vote)));
        }
        gs.votes_by_action.insert(action_id, votes);
    }

    // Enacted governance action IDs
    let roots = &gov
        .drep_pulsing
        .ratify_state
        .enact_state
        .prev_gov_action_ids;
    gs.enacted_pparam_update = roots.pparam_update.as_ref().map(convert_gov_action_id);
    gs.enacted_hard_fork = roots.hard_fork.as_ref().map(convert_gov_action_id);
    gs.enacted_committee = roots.committee.as_ref().map(convert_gov_action_id);
    gs.enacted_constitution = roots.constitution.as_ref().map(convert_gov_action_id);

    gs
}

// ---------------------------------------------------------------------------
// Vote helpers
// ---------------------------------------------------------------------------

fn convert_vote(v: &HaskellVote) -> VotingProcedure {
    VotingProcedure {
        vote: match v {
            HaskellVote::Yes => Vote::Yes,
            HaskellVote::No => Vote::No,
            HaskellVote::Abstain => Vote::Abstain,
        },
        anchor: None,
    }
}

fn is_yes(v: &HaskellVote) -> bool {
    matches!(v, HaskellVote::Yes)
}
fn is_no(v: &HaskellVote) -> bool {
    matches!(v, HaskellVote::No)
}
fn is_abstain(v: &HaskellVote) -> bool {
    matches!(v, HaskellVote::Abstain)
}
fn is_yes_haskell(v: &HaskellVote) -> bool {
    matches!(v, HaskellVote::Yes)
}
fn is_no_haskell(v: &HaskellVote) -> bool {
    matches!(v, HaskellVote::No)
}
fn is_abstain_haskell(v: &HaskellVote) -> bool {
    matches!(v, HaskellVote::Abstain)
}

fn count_votes_matching<K>(
    votes: &HashMap<K, HaskellVote>,
    predicate: fn(&HaskellVote) -> bool,
) -> u64 {
    votes.values().filter(|v| predicate(v)).count() as u64
}

fn count_votes_matching_spo(
    votes: &HashMap<Hash28, HaskellVote>,
    predicate: fn(&HaskellVote) -> bool,
) -> u64 {
    votes.values().filter(|v| predicate(v)).count() as u64
}

// ---------------------------------------------------------------------------
// Proposal / governance action conversion
// ---------------------------------------------------------------------------

fn convert_proposal_procedure(h: &HaskellProposalProcedure) -> ProposalProcedure {
    ProposalProcedure {
        deposit: h.deposit,
        return_addr: h.return_addr.clone(),
        gov_action: convert_gov_action(&h.gov_action),
        anchor: convert_anchor(&h.anchor),
    }
}

fn convert_gov_action(h: &HaskellGovAction) -> GovAction {
    match h {
        HaskellGovAction::InfoAction => GovAction::InfoAction,
        HaskellGovAction::NoConfidence { prev_action_id } => GovAction::NoConfidence {
            prev_action_id: prev_action_id.as_ref().map(convert_gov_action_id),
        },
        HaskellGovAction::HardForkInitiation {
            prev_action_id,
            protocol_version,
        } => GovAction::HardForkInitiation {
            prev_action_id: prev_action_id.as_ref().map(convert_gov_action_id),
            protocol_version: *protocol_version,
        },
        HaskellGovAction::NewConstitution {
            prev_action_id,
            constitution,
        } => GovAction::NewConstitution {
            prev_action_id: prev_action_id.as_ref().map(convert_gov_action_id),
            constitution: convert_constitution(constitution),
        },
        HaskellGovAction::UpdateCommittee {
            prev_action_id,
            members_to_remove,
            members_to_add,
            threshold,
        } => GovAction::UpdateCommittee {
            prev_action_id: prev_action_id.as_ref().map(convert_gov_action_id),
            members_to_remove: members_to_remove
                .iter()
                .map(haskell_cred_to_credential)
                .collect(),
            members_to_add: members_to_add
                .iter()
                .map(|(c, e)| (haskell_cred_to_credential(c), e.0))
                .collect(),
            threshold: convert_rational(threshold),
        },
        HaskellGovAction::ParameterChange {
            prev_action_id,
            guardrail_script,
            ..
        } => GovAction::ParameterChange {
            prev_action_id: prev_action_id.as_ref().map(convert_gov_action_id),
            protocol_param_update: Box::default(),
            policy_hash: guardrail_script.map(hash32_to_hash28),
        },
        HaskellGovAction::TreasuryWithdrawals {
            withdrawals,
            guardrail_script,
        } => GovAction::TreasuryWithdrawals {
            withdrawals: withdrawals.iter().map(|(k, v)| (k.clone(), *v)).collect(),
            policy_hash: guardrail_script.map(hash32_to_hash28),
        },
    }
}

// ---------------------------------------------------------------------------
// Snapshot conversion
// ---------------------------------------------------------------------------

fn convert_snapshots(h: &HaskellSnapShots) -> EpochSnapshots {
    EpochSnapshots {
        mark: Some(convert_snapshot(&h.mark, EpochNo(0))),
        set: Some(convert_snapshot(&h.set, EpochNo(0))),
        go: Some(convert_snapshot(&h.go, EpochNo(0))),
    }
}

fn convert_snapshot(h: &HaskellSnapShot, epoch: EpochNo) -> StakeSnapshot {
    let mut delegations = HashMap::new();
    for (cred, pool_id) in &h.delegations {
        delegations.insert(credential_to_hash32(cred), *pool_id);
    }

    let mut pool_params = HashMap::new();
    for (pool_id, hp) in &h.pool_params {
        let cp = convert_pool_params(hp);
        pool_params.insert(
            *pool_id,
            PoolRegistration {
                pool_id: cp.pool_id,
                vrf_keyhash: cp.vrf_keyhash,
                pledge: cp.pledge,
                cost: cp.cost,
                margin_numerator: cp.margin_numerator,
                margin_denominator: cp.margin_denominator,
                reward_account: cp.reward_account,
                owners: cp.owners,
                relays: cp.relays,
                metadata_url: cp.metadata_url,
                metadata_hash: cp.metadata_hash,
            },
        );
    }

    let mut pool_stake: HashMap<Hash28, Lovelace> = HashMap::new();
    let mut stake_distribution = HashMap::new();
    for (cred, amount) in &h.stake {
        let cred_hash = credential_to_hash32(cred);
        stake_distribution.insert(cred_hash, *amount);
        // Aggregate by pool for pool_stake
        if let Some(pool_id) = h.delegations.get(cred) {
            let entry = pool_stake.entry(*pool_id).or_insert(Lovelace(0));
            entry.0 += amount.0;
        }
    }

    StakeSnapshot {
        epoch,
        delegations: Arc::new(delegations),
        pool_stake,
        pool_params: Arc::new(pool_params),
        stake_distribution: Arc::new(stake_distribution),
    }
}

// ---------------------------------------------------------------------------
// Credential conversion helper
// ---------------------------------------------------------------------------

fn haskell_cred_to_credential(cred: &HaskellCredential) -> Credential {
    match cred {
        HaskellCredential::KeyHash(h) => Credential::VerificationKey(*h),
        HaskellCredential::ScriptHash(h) => Credential::Script(*h),
    }
}

/// Convert Hash32 to Hash28 by taking the first 28 bytes.
/// Used for guardrail scripts stored as Hash32 in the Haskell format
/// but represented as ScriptHash (= Hash28) in Torsten.
fn hash32_to_hash28(h: Hash32) -> Hash28 {
    let mut bytes = [0u8; 28];
    bytes.copy_from_slice(&h.as_bytes()[..28]);
    Hash28::from_bytes(bytes)
}
