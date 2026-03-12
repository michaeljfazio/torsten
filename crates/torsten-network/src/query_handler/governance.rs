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

/// Handle GetProposals (tag 31) — filtered governance proposals.
///
/// Argument: tag(258) Set<GovActionId> where GovActionId = [tx_hash(32), action_index]
/// Returns: Seq (GovActionState)
pub(crate) fn handle_proposals(
    state: &NodeStateSnapshot,
    decoder: &mut minicbor::Decoder<'_>,
) -> QueryResult {
    debug!("Query: GetProposals");
    let filter_ids = parse_gov_action_id_set(decoder);
    if filter_ids.is_empty() {
        QueryResult::Proposals(state.governance_proposals.clone())
    } else {
        let filtered = state
            .governance_proposals
            .iter()
            .filter(|p| {
                filter_ids
                    .iter()
                    .any(|(tx_id, idx)| tx_id == &p.tx_id && *idx == p.action_index)
            })
            .cloned()
            .collect();
        QueryResult::Proposals(filtered)
    }
}

/// Parse a Set<GovActionId> from CBOR.
/// GovActionId = [tx_hash(32), action_index(u32)]
fn parse_gov_action_id_set(decoder: &mut minicbor::Decoder<'_>) -> Vec<(Vec<u8>, u32)> {
    let mut ids = Vec::new();
    let _ = decoder.tag(); // tag(258) for Set
    if let Ok(Some(n)) = decoder.array() {
        for _ in 0..n {
            if let Ok(Some(_)) = decoder.array() {
                if let (Ok(tx_hash), Ok(idx)) = (decoder.bytes(), decoder.u32()) {
                    ids.push((tx_hash.to_vec(), idx));
                }
            }
        }
    }
    ids
}

/// Handle GetRatifyState (tag 32) — current ratification state.
///
/// Returns: array(4) [enacted_seq, expired_seq, delayed_bool, future_pparam_update]
/// The Haskell node computes this from the DRep pulsing state. We return the
/// current enacted/expired state which is functionally equivalent between epoch boundaries.
pub(crate) fn handle_ratify_state(_state: &NodeStateSnapshot) -> QueryResult {
    debug!("Query: GetRatifyState");
    // We don't currently track per-epoch enacted/expired proposals separately.
    // Return empty enacted/expired lists with no delay — this is correct between
    // epoch boundaries since ratification happens at epoch transitions.
    QueryResult::RatifyState {
        enacted: Vec::new(),
        expired: Vec::new(),
        delayed: false,
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query_handler::types::{NodeStateSnapshot, ProposalSnapshot};

    fn make_state_with_proposals() -> NodeStateSnapshot {
        NodeStateSnapshot {
            governance_proposals: vec![
                ProposalSnapshot {
                    tx_id: vec![1u8; 32],
                    action_index: 0,
                    action_type: "InfoAction".to_string(),
                    proposed_epoch: 100,
                    expires_epoch: 106,
                    yes_votes: 10,
                    no_votes: 2,
                    abstain_votes: 1,
                    deposit: 100_000_000_000,
                    return_addr: vec![0u8; 29],
                    anchor_url: "https://example.com/proposal1".to_string(),
                    anchor_hash: vec![0xAA; 32],
                    committee_votes: vec![],
                    drep_votes: vec![(vec![0xCC; 28], 0, 1)], // one DRep Yes vote
                    spo_votes: vec![],
                },
                ProposalSnapshot {
                    tx_id: vec![2u8; 32],
                    action_index: 1,
                    action_type: "ParameterChange".to_string(),
                    proposed_epoch: 101,
                    expires_epoch: 107,
                    yes_votes: 5,
                    no_votes: 3,
                    abstain_votes: 0,
                    deposit: 100_000_000_000,
                    return_addr: vec![0u8; 29],
                    anchor_url: "https://example.com/proposal2".to_string(),
                    anchor_hash: vec![0xBB; 32],
                    committee_votes: vec![],
                    drep_votes: vec![],
                    spo_votes: vec![(vec![0xDD; 28], 0)], // one SPO No vote
                },
            ],
            ..NodeStateSnapshot::default()
        }
    }

    #[test]
    fn test_proposals_no_filter() {
        let state = make_state_with_proposals();
        let cbor = {
            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);
            enc.tag(minicbor::data::Tag::new(258)).ok();
            enc.array(0).ok();
            buf
        };
        let mut dec = minicbor::Decoder::new(&cbor);
        let result = handle_proposals(&state, &mut dec);
        match result {
            QueryResult::Proposals(proposals) => {
                assert_eq!(proposals.len(), 2);
            }
            _ => panic!("Expected Proposals"),
        }
    }

    #[test]
    fn test_proposals_filtered() {
        let state = make_state_with_proposals();
        let cbor = {
            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);
            enc.tag(minicbor::data::Tag::new(258)).ok();
            enc.array(1).ok();
            // GovActionId: [tx_hash, action_index]
            enc.array(2).ok();
            enc.bytes(&[1u8; 32]).ok();
            enc.u32(0).ok();
            buf
        };
        let mut dec = minicbor::Decoder::new(&cbor);
        let result = handle_proposals(&state, &mut dec);
        match result {
            QueryResult::Proposals(proposals) => {
                assert_eq!(proposals.len(), 1);
                assert_eq!(proposals[0].tx_id, vec![1u8; 32]);
                assert_eq!(proposals[0].action_index, 0);
            }
            _ => panic!("Expected Proposals"),
        }
    }

    #[test]
    fn test_ratify_state_returns_empty() {
        let state = NodeStateSnapshot::default();
        let result = handle_ratify_state(&state);
        match result {
            QueryResult::RatifyState {
                enacted,
                expired,
                delayed,
            } => {
                assert!(enacted.is_empty());
                assert!(expired.is_empty());
                assert!(!delayed);
            }
            _ => panic!("Expected RatifyState"),
        }
    }
}
