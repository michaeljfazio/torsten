use crate::cbor::*;
use std::collections::BTreeMap;
use torsten_primitives::transaction::*;

use super::certificate::{encode_anchor, encode_credential, encode_rational};

/// Encode optional anchor
pub(crate) fn encode_optional_anchor(anchor: &Option<Anchor>) -> Vec<u8> {
    match anchor {
        Some(a) => encode_anchor(a),
        None => encode_null(),
    }
}

/// Encode a DRep
pub(crate) fn encode_drep(drep: &DRep) -> Vec<u8> {
    let mut buf = encode_array_header(2);
    match drep {
        DRep::KeyHash(h) => {
            buf.extend(encode_uint(0));
            buf.extend(encode_hash32(h));
        }
        DRep::ScriptHash(h) => {
            buf.extend(encode_uint(1));
            buf.extend(encode_hash28(h));
        }
        DRep::Abstain => {
            // [2] - single element
            return vec![0x81, 0x02];
        }
        DRep::NoConfidence => {
            // [3] - single element
            return vec![0x81, 0x03];
        }
    }
    buf
}

/// Encode voting procedures map
pub(crate) fn encode_voting_procedures(
    procedures: &BTreeMap<Voter, BTreeMap<GovActionId, VotingProcedure>>,
) -> Vec<u8> {
    let mut buf = encode_map_header(procedures.len());
    for (voter, actions) in procedures {
        buf.extend(encode_voter(voter));
        buf.extend(encode_map_header(actions.len()));
        for (action_id, procedure) in actions {
            buf.extend(encode_gov_action_id(action_id));
            buf.extend(encode_voting_procedure(procedure));
        }
    }
    buf
}

pub(crate) fn encode_voter(voter: &Voter) -> Vec<u8> {
    let mut buf = encode_array_header(2);
    match voter {
        Voter::ConstitutionalCommittee(cred) => {
            match cred {
                torsten_primitives::credentials::Credential::VerificationKey(_) => {
                    buf.extend(encode_uint(0));
                }
                torsten_primitives::credentials::Credential::Script(_) => {
                    buf.extend(encode_uint(1));
                }
            }
            buf.extend(encode_hash28(cred.to_hash()));
        }
        Voter::DRep(cred) => {
            match cred {
                torsten_primitives::credentials::Credential::VerificationKey(_) => {
                    buf.extend(encode_uint(2));
                }
                torsten_primitives::credentials::Credential::Script(_) => {
                    buf.extend(encode_uint(3));
                }
            }
            buf.extend(encode_hash28(cred.to_hash()));
        }
        Voter::StakePool(hash) => {
            buf.extend(encode_uint(4));
            buf.extend(encode_hash32(hash));
        }
    }
    buf
}

pub(crate) fn encode_gov_action_id(id: &GovActionId) -> Vec<u8> {
    let mut buf = encode_array_header(2);
    buf.extend(encode_hash32(&id.transaction_id));
    buf.extend(encode_uint(id.action_index as u64));
    buf
}

pub(crate) fn encode_voting_procedure(proc: &VotingProcedure) -> Vec<u8> {
    let mut buf = encode_array_header(2);
    buf.extend(encode_uint(match proc.vote {
        Vote::No => 0,
        Vote::Yes => 1,
        Vote::Abstain => 2,
    }));
    buf.extend(encode_optional_anchor(&proc.anchor));
    buf
}

pub(crate) fn encode_proposal_procedure(pp: &ProposalProcedure) -> Vec<u8> {
    let mut buf = encode_array_header(4);
    buf.extend(encode_uint(pp.deposit.0));
    buf.extend(encode_bytes(&pp.return_addr));
    buf.extend(encode_gov_action(&pp.gov_action));
    buf.extend(encode_anchor(&pp.anchor));
    buf
}

pub(crate) fn encode_gov_action(action: &GovAction) -> Vec<u8> {
    match action {
        GovAction::ParameterChange {
            prev_action_id,
            protocol_param_update,
            policy_hash,
        } => {
            let mut buf = encode_array_header(4);
            buf.extend(encode_uint(0));
            buf.extend(encode_optional_gov_action_id(prev_action_id));
            buf.extend(super::protocol_params::encode_protocol_param_update(
                protocol_param_update,
            ));
            match policy_hash {
                Some(h) => buf.extend(encode_hash28(h)),
                None => buf.extend(encode_null()),
            }
            buf
        }
        GovAction::HardForkInitiation {
            prev_action_id,
            protocol_version,
        } => {
            let mut buf = encode_array_header(3);
            buf.extend(encode_uint(1));
            buf.extend(encode_optional_gov_action_id(prev_action_id));
            buf.extend(encode_array_header(2));
            buf.extend(encode_uint(protocol_version.0));
            buf.extend(encode_uint(protocol_version.1));
            buf
        }
        GovAction::TreasuryWithdrawals {
            withdrawals,
            policy_hash,
        } => {
            let mut buf = encode_array_header(3);
            buf.extend(encode_uint(2));
            buf.extend(encode_map_header(withdrawals.len()));
            for (addr, amount) in withdrawals {
                buf.extend(encode_bytes(addr));
                buf.extend(encode_uint(amount.0));
            }
            match policy_hash {
                Some(h) => buf.extend(encode_hash28(h)),
                None => buf.extend(encode_null()),
            }
            buf
        }
        GovAction::NoConfidence { prev_action_id } => {
            let mut buf = encode_array_header(2);
            buf.extend(encode_uint(3));
            buf.extend(encode_optional_gov_action_id(prev_action_id));
            buf
        }
        GovAction::UpdateCommittee {
            prev_action_id,
            members_to_remove,
            members_to_add,
            threshold,
        } => {
            let mut buf = encode_array_header(5);
            buf.extend(encode_uint(4));
            buf.extend(encode_optional_gov_action_id(prev_action_id));
            buf.extend(encode_array_header(members_to_remove.len()));
            for cred in members_to_remove {
                buf.extend(encode_credential(cred));
            }
            buf.extend(encode_map_header(members_to_add.len()));
            for (cred, epoch) in members_to_add {
                buf.extend(encode_credential(cred));
                buf.extend(encode_uint(*epoch));
            }
            buf.extend(encode_rational(threshold));
            buf
        }
        GovAction::NewConstitution {
            prev_action_id,
            constitution,
        } => {
            let mut buf = encode_array_header(3);
            buf.extend(encode_uint(5));
            buf.extend(encode_optional_gov_action_id(prev_action_id));
            buf.extend(encode_array_header(2));
            buf.extend(encode_anchor(&constitution.anchor));
            match &constitution.script_hash {
                Some(h) => buf.extend(encode_hash28(h)),
                None => buf.extend(encode_null()),
            }
            buf
        }
        GovAction::InfoAction => {
            let mut buf = encode_array_header(1);
            buf.extend(encode_uint(6));
            buf
        }
    }
}

pub(crate) fn encode_optional_gov_action_id(id: &Option<GovActionId>) -> Vec<u8> {
    match id {
        Some(id) => encode_gov_action_id(id),
        None => encode_null(),
    }
}
