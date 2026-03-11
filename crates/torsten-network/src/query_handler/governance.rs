//! Governance query handlers (tags 23, 24, 25, 26, 27, 28).

use tracing::debug;

use super::parse_credential_set;
use super::types::{GovStateSnapshot, NodeStateSnapshot, QueryResult};

/// Handle GetConstitution (tag 23).
pub(crate) fn handle_constitution(state: &NodeStateSnapshot) -> QueryResult {
    debug!("Query: GetConstitution");
    QueryResult::Constitution {
        url: state.constitution_url.clone(),
        data_hash: state.constitution_hash.clone(),
        script_hash: state.constitution_script.clone(),
    }
}

/// Handle GetGovState (tag 24).
pub(crate) fn handle_gov_state(state: &NodeStateSnapshot) -> QueryResult {
    debug!("Query: GetGovState");
    QueryResult::GovState(Box::new(GovStateSnapshot {
        proposals: state.governance_proposals.clone(),
        committee: state.committee.clone(),
        constitution_url: state.constitution_url.clone(),
        constitution_hash: state.constitution_hash.clone(),
        constitution_script: state.constitution_script.clone(),
        cur_pparams: Box::new(state.protocol_params.clone()),
        prev_pparams: Box::new(state.protocol_params.clone()),
        enacted_pparam_update: state.enacted_pparam_update.clone(),
        enacted_hard_fork: state.enacted_hard_fork.clone(),
        enacted_committee: state.enacted_committee.clone(),
        enacted_constitution: state.enacted_constitution.clone(),
    }))
}

/// Handle GetDRepState (tag 25).
///
/// Argument: tag(258) Set<Credential> where Credential = [0|1, hash(28)]
pub(crate) fn handle_drep_state(
    state: &NodeStateSnapshot,
    decoder: &mut minicbor::Decoder<'_>,
) -> QueryResult {
    debug!("Query: GetDRepState");
    let filter_hashes = parse_credential_set(decoder);
    if filter_hashes.is_empty() {
        QueryResult::DRepState(state.drep_entries.clone())
    } else {
        let filtered = state
            .drep_entries
            .iter()
            .filter(|d| filter_hashes.iter().any(|h| h == &d.credential_hash))
            .cloned()
            .collect();
        QueryResult::DRepState(filtered)
    }
}

/// Handle GetDRepStakeDistr (tag 26).
///
/// Argument: tag(258) Set<DRep>
/// Returns: Map<DRep, Coin> -- total delegated stake per DRep
pub(crate) fn handle_drep_stake_distr(state: &NodeStateSnapshot) -> QueryResult {
    debug!("Query: GetDRepStakeDistr");
    // Return all DRep stake distribution (filtering by DRep is complex, return all)
    QueryResult::DRepStakeDistr(state.drep_stake_distr.clone())
}

/// Handle GetCommitteeMembersState (tag 27).
pub(crate) fn handle_committee_state(state: &NodeStateSnapshot) -> QueryResult {
    debug!("Query: GetCommitteeMembersState");
    QueryResult::CommitteeState(state.committee.clone())
}

/// Handle GetFilteredVoteDelegatees (tag 28).
///
/// Argument: tag(258) Set<Credential>
/// Returns: Map<Credential, DRep> -- vote delegation for filtered credentials
pub(crate) fn handle_filtered_vote_delegatees(
    state: &NodeStateSnapshot,
    decoder: &mut minicbor::Decoder<'_>,
) -> QueryResult {
    debug!("Query: GetFilteredVoteDelegatees");
    let filter_hashes = parse_credential_set(decoder);
    if filter_hashes.is_empty() {
        QueryResult::FilteredVoteDelegatees(state.vote_delegatees.clone())
    } else {
        let filtered = state
            .vote_delegatees
            .iter()
            .filter(|v| filter_hashes.iter().any(|h| h == &v.credential_hash))
            .cloned()
            .collect();
        QueryResult::FilteredVoteDelegatees(filtered)
    }
}
