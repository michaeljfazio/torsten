use crate::cbor::*;
use dugite_primitives::transaction::*;
use std::collections::BTreeMap;

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
                dugite_primitives::credentials::Credential::VerificationKey(_) => {
                    buf.extend(encode_uint(0));
                }
                dugite_primitives::credentials::Credential::Script(_) => {
                    buf.extend(encode_uint(1));
                }
            }
            buf.extend(encode_hash28(cred.to_hash()));
        }
        Voter::DRep(cred) => {
            match cred {
                dugite_primitives::credentials::Credential::VerificationKey(_) => {
                    buf.extend(encode_uint(2));
                }
                dugite_primitives::credentials::Credential::Script(_) => {
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

#[cfg(test)]
mod tests {
    use super::*;
    use dugite_primitives::credentials::Credential;
    use dugite_primitives::hash::{Hash28, Hash32};
    use dugite_primitives::transaction::{
        Anchor, Constitution, GovAction, GovActionId, ProposalProcedure, ProtocolParamUpdate, Vote,
        Voter, VotingProcedure,
    };
    use dugite_primitives::value::Lovelace;

    // ── helpers ──────────────────────────────────────────────────────────────

    fn hash32(byte: u8) -> Hash32 {
        Hash32::from_bytes([byte; 32])
    }

    fn hash28(byte: u8) -> Hash28 {
        Hash28::from_bytes([byte; 28])
    }

    fn anchor(byte: u8) -> Anchor {
        Anchor {
            url: "https://example.com".to_string(),
            data_hash: hash32(byte),
        }
    }

    fn gov_action_id(byte: u8) -> GovActionId {
        GovActionId {
            transaction_id: hash32(byte),
            action_index: 0,
        }
    }

    // ── encode_drep ──────────────────────────────────────────────────────────

    /// DRep::KeyHash encodes as array(2) [0, bstr(32)]
    #[test]
    fn test_encode_drep_keyhash() {
        let drep = DRep::KeyHash(hash32(0xAA));
        let encoded = encode_drep(&drep);

        // array(2) header
        assert_eq!(encoded[0], 0x82);
        // uint(0)
        assert_eq!(encoded[1], 0x00);
        // bstr(32) header: 0x58 0x20
        assert_eq!(encoded[2], 0x58);
        assert_eq!(encoded[3], 0x20);
        // 32 bytes of 0xAA
        assert_eq!(&encoded[4..], &[0xAAu8; 32]);
        assert_eq!(encoded.len(), 36); // 1 + 1 + 2 + 32
    }

    /// DRep::ScriptHash encodes as array(2) [1, bstr(28)]
    #[test]
    fn test_encode_drep_scripthash() {
        let drep = DRep::ScriptHash(hash28(0xBB));
        let encoded = encode_drep(&drep);

        assert_eq!(encoded[0], 0x82); // array(2)
        assert_eq!(encoded[1], 0x01); // uint(1)
        assert_eq!(encoded[2], 0x58); // bstr, 1-byte length
        assert_eq!(encoded[3], 28);
        assert_eq!(&encoded[4..], &[0xBBu8; 28]);
        assert_eq!(encoded.len(), 32); // 1 + 1 + 2 + 28
    }

    /// DRep::Abstain encodes as [2] — array(1) containing uint(2)
    #[test]
    fn test_encode_drep_abstain() {
        let encoded = encode_drep(&DRep::Abstain);
        assert_eq!(encoded, vec![0x81, 0x02]);
    }

    /// DRep::NoConfidence encodes as [3] — array(1) containing uint(3)
    #[test]
    fn test_encode_drep_no_confidence() {
        let encoded = encode_drep(&DRep::NoConfidence);
        assert_eq!(encoded, vec![0x81, 0x03]);
    }

    // ── encode_voter ─────────────────────────────────────────────────────────

    /// ConstitutionalCommittee + VerificationKey → type tag 0
    #[test]
    fn test_encode_voter_cc_vkey() {
        let cred = Credential::VerificationKey(hash28(0x01));
        let voter = Voter::ConstitutionalCommittee(cred);
        let encoded = encode_voter(&voter);

        assert_eq!(encoded[0], 0x82); // array(2)
        assert_eq!(encoded[1], 0x00); // uint(0) — CC+vkey
        assert_eq!(encoded[2], 0x58);
        assert_eq!(encoded[3], 28);
        assert_eq!(&encoded[4..], &[0x01u8; 28]);
    }

    /// ConstitutionalCommittee + Script → type tag 1
    #[test]
    fn test_encode_voter_cc_script() {
        let cred = Credential::Script(hash28(0x02));
        let voter = Voter::ConstitutionalCommittee(cred);
        let encoded = encode_voter(&voter);

        assert_eq!(encoded[0], 0x82); // array(2)
        assert_eq!(encoded[1], 0x01); // uint(1) — CC+script
        assert_eq!(&encoded[4..], &[0x02u8; 28]);
    }

    /// DRep + VerificationKey → type tag 2
    #[test]
    fn test_encode_voter_drep_vkey() {
        let cred = Credential::VerificationKey(hash28(0x03));
        let voter = Voter::DRep(cred);
        let encoded = encode_voter(&voter);

        assert_eq!(encoded[0], 0x82);
        assert_eq!(encoded[1], 0x02); // uint(2) — DRep+vkey
        assert_eq!(&encoded[4..], &[0x03u8; 28]);
    }

    /// DRep + Script → type tag 3
    #[test]
    fn test_encode_voter_drep_script() {
        let cred = Credential::Script(hash28(0x04));
        let voter = Voter::DRep(cred);
        let encoded = encode_voter(&voter);

        assert_eq!(encoded[0], 0x82);
        assert_eq!(encoded[1], 0x03); // uint(3) — DRep+script
        assert_eq!(&encoded[4..], &[0x04u8; 28]);
    }

    /// StakePool → type tag 4, followed by 32-byte hash
    #[test]
    fn test_encode_voter_stake_pool() {
        let voter = Voter::StakePool(hash32(0x05));
        let encoded = encode_voter(&voter);

        assert_eq!(encoded[0], 0x82); // array(2)
        assert_eq!(encoded[1], 0x04); // uint(4) — StakePool
        assert_eq!(encoded[2], 0x58);
        assert_eq!(encoded[3], 32);
        assert_eq!(&encoded[4..], &[0x05u8; 32]);
        assert_eq!(encoded.len(), 36); // 1 + 1 + 2 + 32
    }

    // ── encode_gov_action_id ─────────────────────────────────────────────────

    /// GovActionId encodes as array(2) [tx_hash, action_index]
    #[test]
    fn test_encode_gov_action_id() {
        let id = GovActionId {
            transaction_id: hash32(0xCC),
            action_index: 3,
        };
        let encoded = encode_gov_action_id(&id);

        assert_eq!(encoded[0], 0x82); // array(2)
                                      // tx hash: 0x58 0x20 followed by 32 bytes
        assert_eq!(encoded[1], 0x58);
        assert_eq!(encoded[2], 0x20);
        assert_eq!(&encoded[3..35], &[0xCCu8; 32]);
        // action_index = 3 (small uint)
        assert_eq!(encoded[35], 0x03);
        assert_eq!(encoded.len(), 36);
    }

    // ── encode_voting_procedure ──────────────────────────────────────────────

    /// Vote::No without anchor encodes as array(2) [0, null]
    #[test]
    fn test_encode_voting_procedure_no_vote_no_anchor() {
        let proc = VotingProcedure {
            vote: Vote::No,
            anchor: None,
        };
        let encoded = encode_voting_procedure(&proc);

        assert_eq!(encoded[0], 0x82); // array(2)
        assert_eq!(encoded[1], 0x00); // Vote::No = 0
        assert_eq!(encoded[2], 0xf6); // null
        assert_eq!(encoded.len(), 3);
    }

    /// Vote::Yes without anchor encodes as array(2) [1, null]
    #[test]
    fn test_encode_voting_procedure_yes_vote_no_anchor() {
        let proc = VotingProcedure {
            vote: Vote::Yes,
            anchor: None,
        };
        let encoded = encode_voting_procedure(&proc);

        assert_eq!(encoded[0], 0x82);
        assert_eq!(encoded[1], 0x01); // Vote::Yes = 1
        assert_eq!(encoded[2], 0xf6); // null
    }

    /// Vote::Abstain without anchor encodes as array(2) [2, null]
    #[test]
    fn test_encode_voting_procedure_abstain_vote_no_anchor() {
        let proc = VotingProcedure {
            vote: Vote::Abstain,
            anchor: None,
        };
        let encoded = encode_voting_procedure(&proc);

        assert_eq!(encoded[0], 0x82);
        assert_eq!(encoded[1], 0x02); // Vote::Abstain = 2
        assert_eq!(encoded[2], 0xf6); // null
    }

    /// VotingProcedure with anchor encodes anchor inline (not null)
    #[test]
    fn test_encode_voting_procedure_with_anchor() {
        let proc = VotingProcedure {
            vote: Vote::Yes,
            anchor: Some(anchor(0xDD)),
        };
        let encoded = encode_voting_procedure(&proc);

        assert_eq!(encoded[0], 0x82); // array(2)
        assert_eq!(encoded[1], 0x01); // Yes = 1
                                      // Anchor starts at [2]: array(2) header
        assert_eq!(encoded[2], 0x82);
        // Encoded length must be more than 3 (there's an anchor)
        assert!(encoded.len() > 3);
    }

    // ── encode_voting_procedures ─────────────────────────────────────────────

    /// Empty voting procedures map encodes as map(0)
    #[test]
    fn test_encode_voting_procedures_empty() {
        let procs: BTreeMap<Voter, BTreeMap<GovActionId, VotingProcedure>> = BTreeMap::new();
        let encoded = encode_voting_procedures(&procs);
        assert_eq!(encoded, vec![0xa0]); // map(0)
    }

    /// Single voter with one vote produces nested map structure
    #[test]
    fn test_encode_voting_procedures_single_entry() {
        let voter = Voter::StakePool(hash32(0x10));
        let action_id = gov_action_id(0x20);
        let proc = VotingProcedure {
            vote: Vote::Yes,
            anchor: None,
        };
        let mut inner: BTreeMap<GovActionId, VotingProcedure> = BTreeMap::new();
        inner.insert(action_id, proc);
        let mut procs: BTreeMap<Voter, BTreeMap<GovActionId, VotingProcedure>> = BTreeMap::new();
        procs.insert(voter, inner);

        let encoded = encode_voting_procedures(&procs);
        // Outer map(1)
        assert_eq!(encoded[0], 0xa1);
        // Voter is first entry (StakePool = array(2) [4, hash32])
        assert_eq!(encoded[1], 0x82); // array(2) for voter
        assert_eq!(encoded[2], 0x04); // StakePool type tag
    }

    // ── encode_proposal_procedure ────────────────────────────────────────────

    /// ProposalProcedure encodes as array(4) [deposit, return_addr, gov_action, anchor]
    #[test]
    fn test_encode_proposal_procedure_info_action() {
        let pp = ProposalProcedure {
            deposit: Lovelace(500_000_000),
            return_addr: vec![0xE0, 0x01, 0x02],
            gov_action: GovAction::InfoAction,
            anchor: anchor(0xEE),
        };
        let encoded = encode_proposal_procedure(&pp);

        assert_eq!(encoded[0], 0x84); // array(4)
                                      // Deposit 500_000_000 = 0x1DCD6500 — encodes as 0x1a 0x1d 0xcd 0x65 0x00
        assert_eq!(encoded[1], 0x1a);
        // return_addr bstr starts after deposit (5 bytes)
        // gov_action and anchor follow
        assert!(encoded.len() > 10);
    }

    // ── encode_gov_action ────────────────────────────────────────────────────

    /// GovAction::ParameterChange (tag 0) encodes as array(4)
    #[test]
    fn test_encode_gov_action_parameter_change() {
        let action = GovAction::ParameterChange {
            prev_action_id: None,
            protocol_param_update: Box::new(ProtocolParamUpdate::default()),
            policy_hash: None,
        };
        let encoded = encode_gov_action(&action);

        assert_eq!(encoded[0], 0x84); // array(4)
        assert_eq!(encoded[1], 0x00); // tag 0
        assert_eq!(encoded[2], 0xf6); // null (no prev_action_id)
        assert_eq!(encoded[3], 0xa0); // empty map (all-None ProtocolParamUpdate)
        assert_eq!(encoded[4], 0xf6); // null (no policy_hash)
        assert_eq!(encoded.len(), 5);
    }

    /// GovAction::HardForkInitiation (tag 1) encodes as array(3)
    #[test]
    fn test_encode_gov_action_hard_fork_initiation() {
        let action = GovAction::HardForkInitiation {
            prev_action_id: None,
            protocol_version: (9, 0),
        };
        let encoded = encode_gov_action(&action);

        assert_eq!(encoded[0], 0x83); // array(3)
        assert_eq!(encoded[1], 0x01); // tag 1
        assert_eq!(encoded[2], 0xf6); // null (no prev_action_id)
        assert_eq!(encoded[3], 0x82); // array(2) for protocol_version
        assert_eq!(encoded[4], 0x09); // major = 9
        assert_eq!(encoded[5], 0x00); // minor = 0
        assert_eq!(encoded.len(), 6);
    }

    /// GovAction::TreasuryWithdrawals (tag 2) encodes as array(3)
    #[test]
    fn test_encode_gov_action_treasury_withdrawals() {
        let mut withdrawals = BTreeMap::new();
        withdrawals.insert(vec![0xE1, 0x02], Lovelace(1_000_000));
        let action = GovAction::TreasuryWithdrawals {
            withdrawals,
            policy_hash: None,
        };
        let encoded = encode_gov_action(&action);

        assert_eq!(encoded[0], 0x83); // array(3)
        assert_eq!(encoded[1], 0x02); // tag 2
        assert_eq!(encoded[2], 0xa1); // map(1) — one withdrawal
                                      // Withdrawal key: bstr(2) = 0x42 0xe1 0x02
        assert_eq!(encoded[3], 0x42);
        assert_eq!(encoded[4], 0xe1);
        assert_eq!(encoded[5], 0x02);
        // Amount 1_000_000: encodes as 0x1a 0x00 0x0f 0x42 0x40
        assert_eq!(encoded[6], 0x1a);
    }

    /// GovAction::NoConfidence (tag 3) encodes as array(2)
    #[test]
    fn test_encode_gov_action_no_confidence() {
        let action = GovAction::NoConfidence {
            prev_action_id: None,
        };
        let encoded = encode_gov_action(&action);

        assert_eq!(encoded[0], 0x82); // array(2)
        assert_eq!(encoded[1], 0x03); // tag 3
        assert_eq!(encoded[2], 0xf6); // null (no prev_action_id)
        assert_eq!(encoded.len(), 3);
    }

    /// GovAction::UpdateCommittee (tag 4) encodes as array(5)
    #[test]
    fn test_encode_gov_action_update_committee() {
        use dugite_primitives::transaction::Rational;
        let action = GovAction::UpdateCommittee {
            prev_action_id: None,
            members_to_remove: vec![],
            members_to_add: BTreeMap::new(),
            threshold: Rational {
                numerator: 2,
                denominator: 3,
            },
        };
        let encoded = encode_gov_action(&action);

        assert_eq!(encoded[0], 0x85); // array(5)
        assert_eq!(encoded[1], 0x04); // tag 4
        assert_eq!(encoded[2], 0xf6); // null (no prev_action_id)
        assert_eq!(encoded[3], 0x80); // array(0) — no members removed
        assert_eq!(encoded[4], 0xa0); // map(0) — no members added
                                      // Rational: CBOR tag 30 = 0xd8 0x1e, then array(2) [2, 3]
        assert_eq!(encoded[5], 0xd8);
        assert_eq!(encoded[6], 0x1e);
        assert_eq!(encoded[7], 0x82);
        assert_eq!(encoded[8], 0x02);
        assert_eq!(encoded[9], 0x03);
        assert_eq!(encoded.len(), 10);
    }

    /// GovAction::NewConstitution (tag 5) encodes as array(3) with nested constitution
    #[test]
    fn test_encode_gov_action_new_constitution_no_script() {
        let action = GovAction::NewConstitution {
            prev_action_id: None,
            constitution: Constitution {
                anchor: anchor(0xF0),
                script_hash: None,
            },
        };
        let encoded = encode_gov_action(&action);

        assert_eq!(encoded[0], 0x83); // array(3)
        assert_eq!(encoded[1], 0x05); // tag 5
        assert_eq!(encoded[2], 0xf6); // null (no prev_action_id)
                                      // Constitution: array(2) [anchor, null]
        assert_eq!(encoded[3], 0x82); // array(2)
                                      // anchor follows (array(2) with url and data_hash)
        assert_eq!(encoded[4], 0x82);
        // script_hash: null at the end
        let last = *encoded.last().unwrap();
        assert_eq!(last, 0xf6);
    }

    /// GovAction::NewConstitution with a script hash
    #[test]
    fn test_encode_gov_action_new_constitution_with_script() {
        let action = GovAction::NewConstitution {
            prev_action_id: None,
            constitution: Constitution {
                anchor: anchor(0xF1),
                script_hash: Some(hash28(0xAB)),
            },
        };
        let encoded = encode_gov_action(&action);

        assert_eq!(encoded[0], 0x83); // array(3)
        assert_eq!(encoded[1], 0x05); // tag 5
                                      // Script hash bstr(28) should be last two fields
        assert_eq!(encoded[2], 0xf6); // null (no prev_action_id)
                                      // script_hash presence: last 30 bytes should be 0x58 0x1c <28 bytes>
        let sh_start = encoded.len() - 30;
        assert_eq!(encoded[sh_start], 0x58);
        assert_eq!(encoded[sh_start + 1], 28);
        assert_eq!(&encoded[sh_start + 2..], &[0xABu8; 28]);
    }

    /// GovAction::InfoAction (tag 6) encodes as array(1) [6]
    #[test]
    fn test_encode_gov_action_info_action() {
        let encoded = encode_gov_action(&GovAction::InfoAction);
        assert_eq!(encoded, vec![0x81, 0x06]);
    }

    // ── encode_optional_anchor ───────────────────────────────────────────────

    /// None anchor encodes as CBOR null
    #[test]
    fn test_encode_optional_anchor_none() {
        let encoded = encode_optional_anchor(&None);
        assert_eq!(encoded, vec![0xf6]);
    }

    /// Some anchor encodes as the anchor itself (not null)
    #[test]
    fn test_encode_optional_anchor_some() {
        let a = anchor(0x77);
        let encoded = encode_optional_anchor(&Some(a));
        // Should NOT be null; starts with array(2)
        assert_ne!(encoded, vec![0xf6]);
        assert_eq!(encoded[0], 0x82); // array(2)
    }

    // ── encode_optional_gov_action_id ────────────────────────────────────────

    /// None GovActionId encodes as CBOR null
    #[test]
    fn test_encode_optional_gov_action_id_none() {
        let encoded = encode_optional_gov_action_id(&None);
        assert_eq!(encoded, vec![0xf6]);
    }

    /// Some GovActionId encodes as array(2) [tx_hash, index]
    #[test]
    fn test_encode_optional_gov_action_id_some() {
        let id = gov_action_id(0x99);
        let encoded = encode_optional_gov_action_id(&Some(id));
        assert_eq!(encoded[0], 0x82); // array(2)
        assert_ne!(encoded, vec![0xf6]);
    }
}
