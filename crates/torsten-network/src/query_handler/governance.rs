//! Governance query handlers (tags 23, 24, 25, 26, 27, 28, 39).

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
        treasury: state.treasury,
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
/// results from the most recent epoch transition's ratification pass.
pub(crate) fn handle_ratify_state(state: &NodeStateSnapshot) -> QueryResult {
    debug!("Query: GetRatifyState");
    // Build a GovStateSnapshot from NodeStateSnapshot fields for EnactState encoding.
    let gov = crate::query_handler::GovStateSnapshot {
        proposals: state.governance_proposals.clone(),
        committee: state.committee.clone(),
        constitution_url: state.constitution_url.clone(),
        constitution_hash: state.constitution_hash.clone(),
        constitution_script: state.constitution_script.clone(),
        cur_pparams: Box::new(state.protocol_params.clone()),
        prev_pparams: Box::new(state.protocol_params.clone()), // best available
        enacted_pparam_update: state.enacted_pparam_update.clone(),
        enacted_hard_fork: state.enacted_hard_fork.clone(),
        enacted_committee: state.enacted_committee.clone(),
        enacted_constitution: state.enacted_constitution.clone(),
        treasury: state.treasury,
    };
    QueryResult::RatifyState {
        gov: Box::new(gov),
        enacted: state.ratify_enacted.clone(),
        expired: state.ratify_expired.clone(),
        delayed: state.ratify_delayed,
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

/// Handle GetDRepDelegations (tag 39, N2C V23+).
///
/// Argument: tag(258) Set<Credential> where Credential = array(2) [0|1, hash(28)]
/// Returns: Map<Credential, DRep>
///
/// This query returns the current DRep delegation for each requested stake
/// credential.  An empty filter set means "return all delegations".
///
/// Wire format is identical to GetFilteredVoteDelegatees (tag 28), but it is
/// a distinct query introduced in V23 so that tooling can distinguish it from
/// the older tag-28 query.
pub(crate) fn handle_drep_delegations(
    state: &NodeStateSnapshot,
    decoder: &mut minicbor::Decoder<'_>,
) -> QueryResult {
    debug!("Query: GetDRepDelegations (tag 39)");
    let filter_hashes = parse_credential_set(decoder);
    if filter_hashes.is_empty() {
        // No filter — return all known DRep delegations.
        QueryResult::DRepDelegations(state.drep_delegations.to_vec())
    } else {
        let filtered = state
            .drep_delegations
            .iter()
            .filter(|v| filter_hashes.iter().any(|h| h == &v.credential_hash))
            .cloned()
            .collect();
        QueryResult::DRepDelegations(filtered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query_handler::types::{
        DRepDelegationEntry, GovActionId, NodeStateSnapshot, ProposalSnapshot,
    };

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
                gov: _,
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

    #[test]
    fn test_ratify_state_with_enacted_and_expired() {
        let enacted_proposal = ProposalSnapshot {
            tx_id: vec![0xAA; 32],
            action_index: 0,
            action_type: "NoConfidence".to_string(),
            proposed_epoch: 100,
            expires_epoch: 110,
            yes_votes: 5,
            no_votes: 1,
            abstain_votes: 0,
            deposit: 500_000_000,
            return_addr: vec![0xBB; 29],
            anchor_url: "https://example.com".to_string(),
            anchor_hash: vec![0xCC; 32],
            committee_votes: Vec::new(),
            drep_votes: Vec::new(),
            spo_votes: Vec::new(),
        };
        let enacted_id = GovActionId {
            tx_id: vec![0xAA; 32],
            action_index: 0,
        };
        let expired_id = GovActionId {
            tx_id: vec![0xDD; 32],
            action_index: 1,
        };
        let state = NodeStateSnapshot {
            ratify_enacted: vec![(enacted_proposal, enacted_id)],
            ratify_expired: vec![expired_id],
            ratify_delayed: true,
            ..NodeStateSnapshot::default()
        };
        let result = handle_ratify_state(&state);
        match result {
            QueryResult::RatifyState {
                gov: _,
                enacted,
                expired,
                delayed,
            } => {
                assert_eq!(enacted.len(), 1);
                assert_eq!(enacted[0].0.action_type, "NoConfidence");
                assert_eq!(enacted[0].1.tx_id, vec![0xAA; 32]);
                assert_eq!(expired.len(), 1);
                assert_eq!(expired[0].tx_id, vec![0xDD; 32]);
                assert_eq!(expired[0].action_index, 1);
                assert!(delayed);
            }
            _ => panic!("Expected RatifyState"),
        }
    }

    #[test]
    fn test_constitution() {
        let state = NodeStateSnapshot {
            constitution_url: "https://example.com/constitution".to_string(),
            constitution_hash: vec![0xAB; 32],
            constitution_script: Some(vec![0xCD; 28]),
            ..NodeStateSnapshot::default()
        };
        let result = handle_constitution(&state);
        match result {
            QueryResult::Constitution {
                url,
                data_hash,
                script_hash,
            } => {
                assert_eq!(url, "https://example.com/constitution");
                assert_eq!(data_hash, vec![0xAB; 32]);
                assert_eq!(script_hash, Some(vec![0xCD; 28]));
            }
            _ => panic!("Expected Constitution"),
        }
    }

    #[test]
    fn test_constitution_no_script() {
        let state = NodeStateSnapshot {
            constitution_url: "https://example.com/c".to_string(),
            constitution_hash: vec![0xAB; 32],
            constitution_script: None,
            ..NodeStateSnapshot::default()
        };
        let result = handle_constitution(&state);
        match result {
            QueryResult::Constitution { script_hash, .. } => {
                assert!(script_hash.is_none());
            }
            _ => panic!("Expected Constitution"),
        }
    }

    #[test]
    fn test_gov_state() {
        let state = make_state_with_proposals();
        let result = handle_gov_state(&state);
        match result {
            QueryResult::GovState(gs) => {
                assert_eq!(gs.proposals.len(), 2);
                assert_eq!(gs.constitution_url, "");
                // cur_pparams and prev_pparams should both be defaults
                assert_eq!(gs.cur_pparams.min_fee_a, gs.prev_pparams.min_fee_a);
            }
            _ => panic!("Expected GovState"),
        }
    }

    #[test]
    fn test_gov_state_with_enacted_roots() {
        let state = NodeStateSnapshot {
            enacted_pparam_update: Some((vec![0xAA; 32], 0)),
            enacted_hard_fork: Some((vec![0xBB; 32], 1)),
            ..NodeStateSnapshot::default()
        };
        let result = handle_gov_state(&state);
        match result {
            QueryResult::GovState(gs) => {
                assert_eq!(gs.enacted_pparam_update, Some((vec![0xAA; 32], 0)));
                assert_eq!(gs.enacted_hard_fork, Some((vec![0xBB; 32], 1)));
                assert!(gs.enacted_committee.is_none());
                assert!(gs.enacted_constitution.is_none());
            }
            _ => panic!("Expected GovState"),
        }
    }

    #[test]
    fn test_drep_state_no_filter() {
        use crate::query_handler::types::DRepSnapshot;
        let state = NodeStateSnapshot {
            drep_entries: vec![
                DRepSnapshot {
                    credential_hash: vec![0xAA; 28],
                    credential_type: 0,
                    deposit: 500_000_000,
                    anchor_url: Some("https://example.com".to_string()),
                    anchor_hash: Some(vec![0xBB; 32]),
                    expiry_epoch: 200,
                    delegator_hashes: vec![],
                },
                DRepSnapshot {
                    credential_hash: vec![0xCC; 28],
                    credential_type: 1,
                    deposit: 500_000_000,
                    anchor_url: None,
                    anchor_hash: None,
                    expiry_epoch: 300,
                    delegator_hashes: vec![vec![0xDD; 28]],
                },
            ],
            ..NodeStateSnapshot::default()
        };
        // Empty filter = return all
        let cbor = {
            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);
            enc.tag(minicbor::data::Tag::new(258)).ok();
            enc.array(0).ok();
            buf
        };
        let mut dec = minicbor::Decoder::new(&cbor);
        let result = handle_drep_state(&state, &mut dec);
        match result {
            QueryResult::DRepState(dreps) => {
                assert_eq!(dreps.len(), 2);
            }
            _ => panic!("Expected DRepState"),
        }
    }

    #[test]
    fn test_drep_state_filtered() {
        use crate::query_handler::types::DRepSnapshot;
        let state = NodeStateSnapshot {
            drep_entries: vec![
                DRepSnapshot {
                    credential_hash: vec![0xAA; 28],
                    credential_type: 0,
                    deposit: 500_000_000,
                    anchor_url: None,
                    anchor_hash: None,
                    expiry_epoch: 200,
                    delegator_hashes: vec![],
                },
                DRepSnapshot {
                    credential_hash: vec![0xCC; 28],
                    credential_type: 0,
                    deposit: 500_000_000,
                    anchor_url: None,
                    anchor_hash: None,
                    expiry_epoch: 300,
                    delegator_hashes: vec![],
                },
            ],
            ..NodeStateSnapshot::default()
        };
        // Filter for credential 0xAA only
        let cbor = {
            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);
            enc.tag(minicbor::data::Tag::new(258)).ok();
            enc.array(1).ok();
            enc.array(2).ok();
            enc.u8(0).ok(); // KeyHash
            enc.bytes(&[0xAA; 28]).ok();
            buf
        };
        let mut dec = minicbor::Decoder::new(&cbor);
        let result = handle_drep_state(&state, &mut dec);
        match result {
            QueryResult::DRepState(dreps) => {
                assert_eq!(dreps.len(), 1);
                assert_eq!(dreps[0].credential_hash, vec![0xAA; 28]);
            }
            _ => panic!("Expected DRepState"),
        }
    }

    #[test]
    fn test_committee_state() {
        use crate::query_handler::types::{CommitteeMemberSnapshot, CommitteeSnapshot};
        let state = NodeStateSnapshot {
            committee: CommitteeSnapshot {
                members: vec![CommitteeMemberSnapshot {
                    cold_credential: vec![0xAA; 28],
                    cold_credential_type: 0,
                    hot_status: 0,
                    hot_credential: Some(vec![0xBB; 28]),
                    hot_credential_type: 0,
                    member_status: 0,
                    expiry_epoch: Some(500),
                }],
                threshold: Some((2, 3)),
                current_epoch: 42,
            },
            ..NodeStateSnapshot::default()
        };
        let result = handle_committee_state(&state);
        match result {
            QueryResult::CommitteeState(committee) => {
                assert_eq!(committee.members.len(), 1);
                assert_eq!(committee.threshold, Some((2, 3)));
                assert_eq!(committee.current_epoch, 42);
            }
            _ => panic!("Expected CommitteeState"),
        }
    }

    #[test]
    fn test_drep_stake_distr() {
        use crate::query_handler::types::DRepStakeEntry;
        let state = NodeStateSnapshot {
            drep_stake_distr: vec![
                DRepStakeEntry {
                    drep_type: 0,
                    drep_hash: Some(vec![0xAA; 28]),
                    stake: 1_000_000_000,
                },
                DRepStakeEntry {
                    drep_type: 2, // AlwaysAbstain
                    drep_hash: None,
                    stake: 500_000_000,
                },
            ],
            ..NodeStateSnapshot::default()
        };
        let result = handle_drep_stake_distr(&state);
        match result {
            QueryResult::DRepStakeDistr(entries) => {
                assert_eq!(entries.len(), 2);
                assert_eq!(entries[0].stake, 1_000_000_000);
                assert_eq!(entries[1].drep_type, 2);
            }
            _ => panic!("Expected DRepStakeDistr"),
        }
    }

    #[test]
    fn test_filtered_vote_delegatees_no_filter() {
        use crate::query_handler::types::VoteDelegateeEntry;
        let state = NodeStateSnapshot {
            vote_delegatees: vec![
                VoteDelegateeEntry {
                    credential_hash: vec![0xAA; 28],
                    credential_type: 0,
                    drep_type: 0,
                    drep_hash: Some(vec![0xBB; 28]),
                },
                VoteDelegateeEntry {
                    credential_hash: vec![0xCC; 28],
                    credential_type: 0,
                    drep_type: 2, // AlwaysAbstain
                    drep_hash: None,
                },
            ],
            ..NodeStateSnapshot::default()
        };
        let cbor = {
            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);
            enc.tag(minicbor::data::Tag::new(258)).ok();
            enc.array(0).ok();
            buf
        };
        let mut dec = minicbor::Decoder::new(&cbor);
        let result = handle_filtered_vote_delegatees(&state, &mut dec);
        match result {
            QueryResult::FilteredVoteDelegatees(entries) => {
                assert_eq!(entries.len(), 2);
            }
            _ => panic!("Expected FilteredVoteDelegatees"),
        }
    }

    #[test]
    fn test_filtered_vote_delegatees_filtered() {
        use crate::query_handler::types::VoteDelegateeEntry;
        let state = NodeStateSnapshot {
            vote_delegatees: vec![
                VoteDelegateeEntry {
                    credential_hash: vec![0xAA; 28],
                    credential_type: 0,
                    drep_type: 0,
                    drep_hash: Some(vec![0xBB; 28]),
                },
                VoteDelegateeEntry {
                    credential_hash: vec![0xCC; 28],
                    credential_type: 0,
                    drep_type: 2,
                    drep_hash: None,
                },
            ],
            ..NodeStateSnapshot::default()
        };
        let cbor = {
            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);
            enc.tag(minicbor::data::Tag::new(258)).ok();
            enc.array(1).ok();
            enc.array(2).ok();
            enc.u8(0).ok();
            enc.bytes(&[0xCC; 28]).ok();
            buf
        };
        let mut dec = minicbor::Decoder::new(&cbor);
        let result = handle_filtered_vote_delegatees(&state, &mut dec);
        match result {
            QueryResult::FilteredVoteDelegatees(entries) => {
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0].credential_hash, vec![0xCC; 28]);
                assert_eq!(entries[0].drep_type, 2);
            }
            _ => panic!("Expected FilteredVoteDelegatees"),
        }
    }

    // ─── GetDRepDelegations (tag 39, V23+) ──────────────────────────────────

    fn make_drep_delegations_state() -> NodeStateSnapshot {
        NodeStateSnapshot {
            drep_delegations: vec![
                DRepDelegationEntry {
                    credential_hash: vec![0xAA; 28],
                    credential_type: 0, // KeyHash
                    drep_type: 0,       // KeyHash DRep
                    drep_hash: Some(vec![0xBB; 28]),
                },
                DRepDelegationEntry {
                    credential_hash: vec![0xCC; 28],
                    credential_type: 0, // KeyHash
                    drep_type: 2,       // AlwaysAbstain
                    drep_hash: None,
                },
                DRepDelegationEntry {
                    credential_hash: vec![0xDD; 28],
                    credential_type: 1, // ScriptHash
                    drep_type: 3,       // AlwaysNoConfidence
                    drep_hash: None,
                },
            ],
            ..NodeStateSnapshot::default()
        }
    }

    #[test]
    fn test_drep_delegations_no_filter_returns_all() {
        let state = make_drep_delegations_state();
        // Empty Set<Credential> → return all
        let cbor = {
            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);
            enc.tag(minicbor::data::Tag::new(258)).ok();
            enc.array(0).ok();
            buf
        };
        let mut dec = minicbor::Decoder::new(&cbor);
        let result = handle_drep_delegations(&state, &mut dec);
        match result {
            QueryResult::DRepDelegations(entries) => {
                assert_eq!(entries.len(), 3);
            }
            _ => panic!("Expected DRepDelegations"),
        }
    }

    #[test]
    fn test_drep_delegations_filtered_by_credential() {
        let state = make_drep_delegations_state();
        // Filter for credential 0xAA (KeyHash) only
        let cbor = {
            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);
            enc.tag(minicbor::data::Tag::new(258)).ok();
            enc.array(1).ok();
            enc.array(2).ok();
            enc.u8(0).ok(); // KeyHash
            enc.bytes(&[0xAA; 28]).ok();
            buf
        };
        let mut dec = minicbor::Decoder::new(&cbor);
        let result = handle_drep_delegations(&state, &mut dec);
        match result {
            QueryResult::DRepDelegations(entries) => {
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0].credential_hash, vec![0xAA; 28]);
                assert_eq!(entries[0].credential_type, 0);
                assert_eq!(entries[0].drep_type, 0); // KeyHash DRep
                assert_eq!(entries[0].drep_hash, Some(vec![0xBB; 28]));
            }
            _ => panic!("Expected DRepDelegations"),
        }
    }

    #[test]
    fn test_drep_delegations_filtered_returns_always_abstain() {
        let state = make_drep_delegations_state();
        // Filter for 0xCC — should return the AlwaysAbstain entry
        let cbor = {
            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);
            enc.tag(minicbor::data::Tag::new(258)).ok();
            enc.array(1).ok();
            enc.array(2).ok();
            enc.u8(0).ok();
            enc.bytes(&[0xCC; 28]).ok();
            buf
        };
        let mut dec = minicbor::Decoder::new(&cbor);
        let result = handle_drep_delegations(&state, &mut dec);
        match result {
            QueryResult::DRepDelegations(entries) => {
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0].drep_type, 2); // AlwaysAbstain
                assert!(entries[0].drep_hash.is_none());
            }
            _ => panic!("Expected DRepDelegations"),
        }
    }

    #[test]
    fn test_drep_delegations_filtered_no_match_returns_empty() {
        let state = make_drep_delegations_state();
        // Filter for a credential that doesn't exist
        let cbor = {
            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);
            enc.tag(minicbor::data::Tag::new(258)).ok();
            enc.array(1).ok();
            enc.array(2).ok();
            enc.u8(0).ok();
            enc.bytes(&[0xFF; 28]).ok();
            buf
        };
        let mut dec = minicbor::Decoder::new(&cbor);
        let result = handle_drep_delegations(&state, &mut dec);
        match result {
            QueryResult::DRepDelegations(entries) => {
                assert!(entries.is_empty());
            }
            _ => panic!("Expected DRepDelegations"),
        }
    }

    #[test]
    fn test_drep_delegations_empty_state_no_filter() {
        let state = NodeStateSnapshot::default(); // drep_delegations is empty
        let cbor = {
            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);
            enc.tag(minicbor::data::Tag::new(258)).ok();
            enc.array(0).ok();
            buf
        };
        let mut dec = minicbor::Decoder::new(&cbor);
        let result = handle_drep_delegations(&state, &mut dec);
        match result {
            QueryResult::DRepDelegations(entries) => {
                assert!(entries.is_empty());
            }
            _ => panic!("Expected DRepDelegations"),
        }
    }
}
