//! Parser for Haskell ConwayGovState CBOR encoding.
//!
//! ConwayGovState: array(7)
//!   [proposals, committee, constitution, cur_pparams, prev_pparams,
//!    future_pparams, drep_pulsing_state]

use std::collections::HashMap;

use super::types::{
    HaskellAnchor, HaskellCommittee, HaskellConstitution, HaskellConwayGovState, HaskellDRep,
    HaskellDRepPulsingState, HaskellDRepState, HaskellEnactState, HaskellFuturePParams,
    HaskellGovAction, HaskellGovActionId, HaskellGovActionState, HaskellPrevGovActionIds,
    HaskellProposalProcedure, HaskellPulsingSnapshot, HaskellRatifyState, HaskellRational,
    HaskellVote,
};
use crate::error::SerializationError;
use torsten_primitives::hash::{Hash28, Hash32};
use torsten_primitives::time::EpochNo;
use torsten_primitives::value::Lovelace;

/// Parse ConwayGovState: array(7)
pub fn parse_conway_gov_state(
    d: &mut minicbor::Decoder,
) -> Result<HaskellConwayGovState, SerializationError> {
    let len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("ConwayGovState: expected definite array".into())
    })?;
    if len != 7 {
        return Err(SerializationError::CborDecode(format!(
            "ConwayGovState: expected array(7), got array({len})"
        )));
    }

    // [0] Proposals
    let proposals = parse_proposals(d)?;

    // [1] StrictMaybe(Committee)
    let committee = parse_strict_maybe_committee(d)?;

    // [2] Constitution
    let constitution = parse_constitution(d)?;

    // [3] curPParams: PParams array(31)
    let cur_pparams = super::pparams::parse_pparams(d)?;

    // [4] prevPParams: PParams array(31)
    let prev_pparams = super::pparams::parse_pparams(d)?;

    // [5] FuturePParams
    let future_pparams = parse_future_pparams(d)?;

    // [6] DRepPulsingState
    let drep_pulsing = parse_drep_pulsing_state(d)?;

    Ok(HaskellConwayGovState {
        proposals,
        committee,
        constitution,
        cur_pparams,
        prev_pparams,
        future_pparams,
        drep_pulsing,
    })
}

// ---------------------------------------------------------------------------
// Proposals
// ---------------------------------------------------------------------------

/// Parse Proposals: array(2) [roots, omap]
fn parse_proposals(
    d: &mut minicbor::Decoder,
) -> Result<Vec<HaskellGovActionState>, SerializationError> {
    let len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("Proposals: expected definite array".into())
    })?;
    if len != 2 {
        return Err(SerializationError::CborDecode(format!(
            "Proposals: expected array(2), got array({len})"
        )));
    }

    // [0] GovRelation StrictMaybe (roots / prevGovActionIds) — parse and discard
    //     We collect this info from EnactState instead.
    parse_prev_gov_action_ids(d)?;

    // [1] OMap GovActionId GovActionState — encoded as array of GovActionState
    let omap_len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("Proposals OMap: expected definite array".into())
    })?;
    let mut proposals = Vec::with_capacity(omap_len as usize);
    for _ in 0..omap_len {
        proposals.push(parse_gov_action_state(d)?);
    }

    Ok(proposals)
}

// ---------------------------------------------------------------------------
// GovRelation StrictMaybe (PrevGovActionIds)
// ---------------------------------------------------------------------------

/// Parse GovRelation StrictMaybe: array(4) of StrictMaybe GovActionId
fn parse_prev_gov_action_ids(
    d: &mut minicbor::Decoder,
) -> Result<HaskellPrevGovActionIds, SerializationError> {
    let len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("GovRelation: expected definite array".into())
    })?;
    if len != 4 {
        return Err(SerializationError::CborDecode(format!(
            "GovRelation: expected array(4), got array({len})"
        )));
    }

    let pparam_update = parse_strict_maybe_gov_action_id(d)?;
    let hard_fork = parse_strict_maybe_gov_action_id(d)?;
    let committee = parse_strict_maybe_gov_action_id(d)?;
    let constitution = parse_strict_maybe_gov_action_id(d)?;

    Ok(HaskellPrevGovActionIds {
        pparam_update,
        hard_fork,
        committee,
        constitution,
    })
}

/// Parse StrictMaybe GovActionId: array(0) for SNothing, array(1) [GovActionId] for SJust
fn parse_strict_maybe_gov_action_id(
    d: &mut minicbor::Decoder,
) -> Result<Option<HaskellGovActionId>, SerializationError> {
    let len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("StrictMaybe GovActionId: expected definite array".into())
    })?;
    match len {
        0 => Ok(None),
        1 => Ok(Some(parse_gov_action_id(d)?)),
        _ => Err(SerializationError::CborDecode(format!(
            "StrictMaybe GovActionId: expected array(0) or array(1), got array({len})"
        ))),
    }
}

/// Parse GovActionId: array(2) [TxId(bytes32), GovActionIx(u32)]
fn parse_gov_action_id(
    d: &mut minicbor::Decoder,
) -> Result<HaskellGovActionId, SerializationError> {
    let len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("GovActionId: expected definite array".into())
    })?;
    if len != 2 {
        return Err(SerializationError::CborDecode(format!(
            "GovActionId: expected array(2), got array({len})"
        )));
    }
    let tx_id = parse_hash32(d)?;
    let action_index = d.u32()?;
    Ok(HaskellGovActionId {
        tx_id,
        action_index,
    })
}

// ---------------------------------------------------------------------------
// GovActionState
// ---------------------------------------------------------------------------

/// Parse GovActionState: array(7)
fn parse_gov_action_state(
    d: &mut minicbor::Decoder,
) -> Result<HaskellGovActionState, SerializationError> {
    let len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("GovActionState: expected definite array".into())
    })?;
    if len != 7 {
        return Err(SerializationError::CborDecode(format!(
            "GovActionState: expected array(7), got array({len})"
        )));
    }

    // [0] GovActionId
    let action_id = parse_gov_action_id(d)?;

    // [1] committeeVotes: Map(Credential -> Vote)
    let committee_votes = parse_credential_vote_map(d)?;

    // [2] dRepVotes: Map(Credential -> Vote)
    let drep_votes = parse_credential_vote_map(d)?;

    // [3] stakePoolVotes: Map(KeyHash28 -> Vote)
    let spo_votes = parse_keyhash_vote_map(d)?;

    // [4] ProposalProcedure
    let proposal = parse_proposal_procedure(d)?;

    // [5] proposedIn: EpochNo
    let proposed_in = EpochNo(d.u64()?);

    // [6] expiresAfter: EpochNo
    let expires_after = EpochNo(d.u64()?);

    Ok(HaskellGovActionState {
        action_id,
        committee_votes,
        drep_votes,
        spo_votes,
        proposal,
        proposed_in,
        expires_after,
    })
}

/// Parse Map(Credential -> Vote)
fn parse_credential_vote_map(
    d: &mut minicbor::Decoder,
) -> Result<HashMap<super::types::HaskellCredential, HaskellVote>, SerializationError> {
    let len = d
        .map()?
        .ok_or_else(|| SerializationError::CborDecode("Vote map: expected definite map".into()))?;
    let mut result = HashMap::with_capacity(len as usize);
    for _ in 0..len {
        let cred = super::parse_credential(d)?;
        let vote = parse_vote(d)?;
        result.insert(cred, vote);
    }
    Ok(result)
}

/// Parse Map(KeyHash28 -> Vote)
fn parse_keyhash_vote_map(
    d: &mut minicbor::Decoder,
) -> Result<HashMap<Hash28, HaskellVote>, SerializationError> {
    let len = d.map()?.ok_or_else(|| {
        SerializationError::CborDecode("SPO vote map: expected definite map".into())
    })?;
    let mut result = HashMap::with_capacity(len as usize);
    for _ in 0..len {
        let hash = parse_hash28(d)?;
        let vote = parse_vote(d)?;
        result.insert(hash, vote);
    }
    Ok(result)
}

/// Parse Vote: integer (0=VoteNo, 1=VoteYes, 2=Abstain)
fn parse_vote(d: &mut minicbor::Decoder) -> Result<HaskellVote, SerializationError> {
    let tag = d.u32()?;
    match tag {
        0 => Ok(HaskellVote::No),
        1 => Ok(HaskellVote::Yes),
        2 => Ok(HaskellVote::Abstain),
        _ => Err(SerializationError::CborDecode(format!(
            "Vote: unknown tag {tag}"
        ))),
    }
}

// ---------------------------------------------------------------------------
// ProposalProcedure
// ---------------------------------------------------------------------------

/// Parse ProposalProcedure: array(4)
fn parse_proposal_procedure(
    d: &mut minicbor::Decoder,
) -> Result<HaskellProposalProcedure, SerializationError> {
    let len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("ProposalProcedure: expected definite array".into())
    })?;
    if len != 4 {
        return Err(SerializationError::CborDecode(format!(
            "ProposalProcedure: expected array(4), got array({len})"
        )));
    }

    // [0] deposit: Coin
    let deposit = Lovelace(d.u64()?);

    // [1] returnAddr: RewardAccount (raw bytes)
    let return_addr = d.bytes()?.to_vec();

    // [2] govAction
    let gov_action = parse_gov_action(d)?;

    // [3] anchor
    let anchor = parse_anchor(d)?;

    Ok(HaskellProposalProcedure {
        deposit,
        return_addr,
        gov_action,
        anchor,
    })
}

// ---------------------------------------------------------------------------
// GovAction
// ---------------------------------------------------------------------------

/// Parse GovAction: tagged sum (array with constructor index as first element)
fn parse_gov_action(d: &mut minicbor::Decoder) -> Result<HaskellGovAction, SerializationError> {
    let len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("GovAction: expected definite array".into())
    })?;

    let tag = d.u32()?;

    match tag {
        // ParameterChange: array(4) [0, StrictMaybe GovActionId, PParamUpdate, StrictMaybe ScriptHash]
        0 => {
            if len != 4 {
                return Err(SerializationError::CborDecode(format!(
                    "ParameterChange: expected array(4), got array({len})"
                )));
            }
            let prev_action_id = parse_strict_maybe_gov_action_id(d)?;
            // PParamUpdate is complex — capture raw CBOR bytes
            let start = d.position();
            d.skip()?;
            let end = d.position();
            let pparams_update_raw = d.input()[start..end].to_vec();
            let guardrail_script = parse_strict_maybe_script_hash(d)?;
            Ok(HaskellGovAction::ParameterChange {
                prev_action_id,
                pparams_update_raw,
                guardrail_script,
            })
        }
        // HardForkInitiation: array(3) [1, StrictMaybe GovActionId, ProtocolVersion]
        1 => {
            if len != 3 {
                return Err(SerializationError::CborDecode(format!(
                    "HardForkInitiation: expected array(3), got array({len})"
                )));
            }
            let prev_action_id = parse_strict_maybe_gov_action_id(d)?;
            // ProtocolVersion: array(2) [major, minor]
            let pv_len = d.array()?.ok_or_else(|| {
                SerializationError::CborDecode(
                    "HardFork ProtocolVersion: expected definite array".into(),
                )
            })?;
            if pv_len != 2 {
                return Err(SerializationError::CborDecode(format!(
                    "HardFork ProtocolVersion: expected array(2), got array({pv_len})"
                )));
            }
            let major = d.u64()?;
            let minor = d.u64()?;
            Ok(HaskellGovAction::HardForkInitiation {
                prev_action_id,
                protocol_version: (major, minor),
            })
        }
        // TreasuryWithdrawals: array(3) [2, Map(RewardAccount -> Coin), StrictMaybe ScriptHash]
        2 => {
            if len != 3 {
                return Err(SerializationError::CborDecode(format!(
                    "TreasuryWithdrawals: expected array(3), got array({len})"
                )));
            }
            let map_len = d.map()?.ok_or_else(|| {
                SerializationError::CborDecode(
                    "TreasuryWithdrawals map: expected definite map".into(),
                )
            })?;
            let mut withdrawals = HashMap::with_capacity(map_len as usize);
            for _ in 0..map_len {
                let addr = d.bytes()?.to_vec();
                let coin = Lovelace(d.u64()?);
                withdrawals.insert(addr, coin);
            }
            let guardrail_script = parse_strict_maybe_script_hash(d)?;
            Ok(HaskellGovAction::TreasuryWithdrawals {
                withdrawals,
                guardrail_script,
            })
        }
        // NoConfidence: array(2) [3, StrictMaybe GovActionId]
        3 => {
            if len != 2 {
                return Err(SerializationError::CborDecode(format!(
                    "NoConfidence: expected array(2), got array({len})"
                )));
            }
            let prev_action_id = parse_strict_maybe_gov_action_id(d)?;
            Ok(HaskellGovAction::NoConfidence { prev_action_id })
        }
        // UpdateCommittee: array(5) [4, StrictMaybe GovActionId, Set(Credential), Map(Credential -> EpochNo), UnitInterval]
        4 => {
            if len != 5 {
                return Err(SerializationError::CborDecode(format!(
                    "UpdateCommittee: expected array(5), got array({len})"
                )));
            }
            let prev_action_id = parse_strict_maybe_gov_action_id(d)?;
            // Set(Credential) — encoded as array
            let set_len = d.array()?.ok_or_else(|| {
                SerializationError::CborDecode(
                    "UpdateCommittee members_to_remove: expected definite array".into(),
                )
            })?;
            let mut members_to_remove = Vec::with_capacity(set_len as usize);
            for _ in 0..set_len {
                members_to_remove.push(super::parse_credential(d)?);
            }
            // Map(Credential -> EpochNo)
            let add_len = d.map()?.ok_or_else(|| {
                SerializationError::CborDecode(
                    "UpdateCommittee members_to_add: expected definite map".into(),
                )
            })?;
            let mut members_to_add = HashMap::with_capacity(add_len as usize);
            for _ in 0..add_len {
                let cred = super::parse_credential(d)?;
                let epoch = EpochNo(d.u64()?);
                members_to_add.insert(cred, epoch);
            }
            // UnitInterval: Tag(30) [num, den]
            let (num, den) = super::parse_tagged_rational(d)?;
            Ok(HaskellGovAction::UpdateCommittee {
                prev_action_id,
                members_to_remove,
                members_to_add,
                threshold: HaskellRational {
                    numerator: num,
                    denominator: den,
                },
            })
        }
        // NewConstitution: array(3) [5, StrictMaybe GovActionId, Constitution]
        5 => {
            if len != 3 {
                return Err(SerializationError::CborDecode(format!(
                    "NewConstitution: expected array(3), got array({len})"
                )));
            }
            let prev_action_id = parse_strict_maybe_gov_action_id(d)?;
            let constitution = parse_constitution(d)?;
            Ok(HaskellGovAction::NewConstitution {
                prev_action_id,
                constitution,
            })
        }
        // InfoAction: array(1) [6]
        6 => {
            if len != 1 {
                return Err(SerializationError::CborDecode(format!(
                    "InfoAction: expected array(1), got array({len})"
                )));
            }
            Ok(HaskellGovAction::InfoAction)
        }
        _ => Err(SerializationError::CborDecode(format!(
            "GovAction: unknown tag {tag}"
        ))),
    }
}

// ---------------------------------------------------------------------------
// Committee
// ---------------------------------------------------------------------------

/// Parse StrictMaybe(Committee): array(0) or array(1) [Committee]
fn parse_strict_maybe_committee(
    d: &mut minicbor::Decoder,
) -> Result<Option<HaskellCommittee>, SerializationError> {
    let len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("StrictMaybe Committee: expected definite array".into())
    })?;
    match len {
        0 => Ok(None),
        1 => Ok(Some(parse_committee(d)?)),
        _ => Err(SerializationError::CborDecode(format!(
            "StrictMaybe Committee: expected array(0) or array(1), got array({len})"
        ))),
    }
}

/// Parse Committee: array(2) [members_map, threshold]
fn parse_committee(d: &mut minicbor::Decoder) -> Result<HaskellCommittee, SerializationError> {
    let len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("Committee: expected definite array".into())
    })?;
    if len != 2 {
        return Err(SerializationError::CborDecode(format!(
            "Committee: expected array(2), got array({len})"
        )));
    }

    // [0] Map(Credential -> EpochNo)
    let map_len = d.map()?.ok_or_else(|| {
        SerializationError::CborDecode("Committee members: expected definite map".into())
    })?;
    let mut members = HashMap::with_capacity(map_len as usize);
    for _ in 0..map_len {
        let cred = super::parse_credential(d)?;
        let epoch = EpochNo(d.u64()?);
        members.insert(cred, epoch);
    }

    // [1] threshold: UnitInterval = Tag(30) [num, den]
    let (num, den) = super::parse_tagged_rational(d)?;

    Ok(HaskellCommittee {
        members,
        threshold: HaskellRational {
            numerator: num,
            denominator: den,
        },
    })
}

// ---------------------------------------------------------------------------
// Constitution
// ---------------------------------------------------------------------------

/// Parse Constitution: array(2) [anchor, StrictMaybe ScriptHash]
fn parse_constitution(
    d: &mut minicbor::Decoder,
) -> Result<HaskellConstitution, SerializationError> {
    let len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("Constitution: expected definite array".into())
    })?;
    if len != 2 {
        return Err(SerializationError::CborDecode(format!(
            "Constitution: expected array(2), got array({len})"
        )));
    }

    let anchor = parse_anchor(d)?;
    let guardrail_script = parse_strict_maybe_script_hash(d)?;

    Ok(HaskellConstitution {
        anchor,
        guardrail_script,
    })
}

// ---------------------------------------------------------------------------
// FuturePParams
// ---------------------------------------------------------------------------

/// Parse FuturePParams: tagged sum (array + constructor index)
///   NoPParamsUpdate: array(1) [0]
///   DefinitePParamsUpdate: array(2) [1, PParams]
///   PotentialPParamsUpdate: array(2) [2, StrictMaybe PParams]
fn parse_future_pparams(
    d: &mut minicbor::Decoder,
) -> Result<HaskellFuturePParams, SerializationError> {
    let len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("FuturePParams: expected definite array".into())
    })?;
    let tag = d.u32()?;
    match tag {
        0 => {
            if len != 1 {
                return Err(SerializationError::CborDecode(format!(
                    "NoPParamsUpdate: expected array(1), got array({len})"
                )));
            }
            Ok(HaskellFuturePParams::NoPParamsUpdate)
        }
        1 => {
            if len != 2 {
                return Err(SerializationError::CborDecode(format!(
                    "DefinitePParamsUpdate: expected array(2), got array({len})"
                )));
            }
            let pp = super::pparams::parse_pparams(d)?;
            Ok(HaskellFuturePParams::DefinitePParamsUpdate(pp))
        }
        2 => {
            if len != 2 {
                return Err(SerializationError::CborDecode(format!(
                    "PotentialPParamsUpdate: expected array(2), got array({len})"
                )));
            }
            // StrictMaybe PParams
            let sm_len = d.array()?.ok_or_else(|| {
                SerializationError::CborDecode(
                    "PotentialPParamsUpdate StrictMaybe: expected definite array".into(),
                )
            })?;
            match sm_len {
                0 => Ok(HaskellFuturePParams::PotentialPParamsUpdate(None)),
                1 => {
                    let pp = super::pparams::parse_pparams(d)?;
                    Ok(HaskellFuturePParams::PotentialPParamsUpdate(Some(pp)))
                }
                _ => Err(SerializationError::CborDecode(format!(
                    "PotentialPParamsUpdate: StrictMaybe expected array(0) or array(1), got array({sm_len})"
                ))),
            }
        }
        _ => Err(SerializationError::CborDecode(format!(
            "FuturePParams: unknown tag {tag}"
        ))),
    }
}

// ---------------------------------------------------------------------------
// DRepPulsingState
// ---------------------------------------------------------------------------

/// Parse DRepPulsingState: always DRComplete on disk
///   DRComplete: array(2) [PulsingSnapshot, RatifyState]
fn parse_drep_pulsing_state(
    d: &mut minicbor::Decoder,
) -> Result<HaskellDRepPulsingState, SerializationError> {
    let len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("DRepPulsingState: expected definite array".into())
    })?;
    if len != 2 {
        return Err(SerializationError::CborDecode(format!(
            "DRepPulsingState: expected array(2), got array({len})"
        )));
    }

    let snapshot = parse_pulsing_snapshot(d)?;
    let ratify_state = parse_ratify_state(d)?;

    Ok(HaskellDRepPulsingState {
        snapshot,
        ratify_state,
    })
}

/// Parse PulsingSnapshot: array(4)
///   [0] proposals: StrictSeq(GovActionState) = array of GovActionState
///   [1] dRepDistr: Map(DRep -> CompactCoin)
///   [2] dRepState: Map(Credential -> DRepState)
///   [3] poolDistr: Map(KeyHash28 -> CompactCoin)
fn parse_pulsing_snapshot(
    d: &mut minicbor::Decoder,
) -> Result<HaskellPulsingSnapshot, SerializationError> {
    let len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("PulsingSnapshot: expected definite array".into())
    })?;
    if len != 4 {
        return Err(SerializationError::CborDecode(format!(
            "PulsingSnapshot: expected array(4), got array({len})"
        )));
    }

    // [0] proposals: array of GovActionState
    let props_len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("PulsingSnapshot proposals: expected definite array".into())
    })?;
    let mut proposals = Vec::with_capacity(props_len as usize);
    for _ in 0..props_len {
        proposals.push(parse_gov_action_state(d)?);
    }

    // [1] dRepDistr: Map(DRep -> CompactCoin)
    let drep_distr = parse_drep_distr_map(d)?;

    // [2] dRepState: Map(Credential -> DRepState)
    let drep_state = parse_drep_state_map(d)?;

    // [3] poolDistr: Map(KeyHash28 -> CompactCoin)
    let pool_distr = parse_keyhash_coin_map(d)?;

    Ok(HaskellPulsingSnapshot {
        proposals,
        drep_distr,
        drep_state,
        pool_distr,
    })
}

/// Parse Map(DRep -> CompactCoin)
fn parse_drep_distr_map(
    d: &mut minicbor::Decoder,
) -> Result<HashMap<HaskellDRep, Lovelace>, SerializationError> {
    let len = d.map()?.ok_or_else(|| {
        SerializationError::CborDecode("DRep distr map: expected definite map".into())
    })?;
    let mut result = HashMap::with_capacity(len as usize);
    for _ in 0..len {
        let drep = parse_drep(d)?;
        let coin = Lovelace(d.u64()?);
        result.insert(drep, coin);
    }
    Ok(result)
}

/// Parse DRep encoding:
///   KeyHash:      array(2) [0, bytes28]
///   ScriptHash:   array(2) [1, bytes28]
///   Abstain:      array(1) [2]
///   NoConfidence: array(1) [3]
fn parse_drep(d: &mut minicbor::Decoder) -> Result<HaskellDRep, SerializationError> {
    let len = d
        .array()?
        .ok_or_else(|| SerializationError::CborDecode("DRep: expected definite array".into()))?;
    let tag = d.u32()?;
    match tag {
        0 => {
            if len != 2 {
                return Err(SerializationError::CborDecode(format!(
                    "DRep KeyHash: expected array(2), got array({len})"
                )));
            }
            let hash = parse_hash28(d)?;
            Ok(HaskellDRep::KeyHash(hash))
        }
        1 => {
            if len != 2 {
                return Err(SerializationError::CborDecode(format!(
                    "DRep ScriptHash: expected array(2), got array({len})"
                )));
            }
            let hash = parse_hash28(d)?;
            Ok(HaskellDRep::ScriptHash(hash))
        }
        2 => {
            if len != 1 {
                return Err(SerializationError::CborDecode(format!(
                    "DRep Abstain: expected array(1), got array({len})"
                )));
            }
            Ok(HaskellDRep::Abstain)
        }
        3 => {
            if len != 1 {
                return Err(SerializationError::CborDecode(format!(
                    "DRep NoConfidence: expected array(1), got array({len})"
                )));
            }
            Ok(HaskellDRep::NoConfidence)
        }
        _ => Err(SerializationError::CborDecode(format!(
            "DRep: unknown tag {tag}"
        ))),
    }
}

/// Parse Map(Credential -> DRepState)
fn parse_drep_state_map(
    d: &mut minicbor::Decoder,
) -> Result<HashMap<super::types::HaskellCredential, HaskellDRepState>, SerializationError> {
    let len = d.map()?.ok_or_else(|| {
        SerializationError::CborDecode("DRep state map: expected definite map".into())
    })?;
    let mut result = HashMap::with_capacity(len as usize);
    for _ in 0..len {
        let cred = super::parse_credential(d)?;
        let state = parse_drep_state(d)?;
        result.insert(cred, state);
    }
    Ok(result)
}

/// Parse DRepState: array(4) [expiry, StrictMaybe(Anchor), deposit, delegators_set]
fn parse_drep_state(d: &mut minicbor::Decoder) -> Result<HaskellDRepState, SerializationError> {
    let len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("DRepState: expected definite array".into())
    })?;
    if len != 4 {
        return Err(SerializationError::CborDecode(format!(
            "DRepState: expected array(4), got array({len})"
        )));
    }

    // [0] expiry: EpochNo
    let expiry = EpochNo(d.u64()?);

    // [1] anchor: StrictMaybe(Anchor) — array(0) or array(1) [Anchor]
    let anchor = parse_strict_maybe_anchor(d)?;

    // [2] deposit: CompactCoin (Word64)
    let deposit = Lovelace(d.u64()?);

    // [3] delegators: Set(Credential) — encoded as array
    let del_len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("DRepState delegators: expected definite array".into())
    })?;
    let mut delegators = Vec::with_capacity(del_len as usize);
    for _ in 0..del_len {
        delegators.push(super::parse_credential(d)?);
    }

    Ok(HaskellDRepState {
        expiry,
        anchor,
        deposit,
        delegators,
    })
}

/// Parse Map(KeyHash28 -> CompactCoin)
fn parse_keyhash_coin_map(
    d: &mut minicbor::Decoder,
) -> Result<HashMap<Hash28, Lovelace>, SerializationError> {
    let len = d.map()?.ok_or_else(|| {
        SerializationError::CborDecode("KeyHash-Coin map: expected definite map".into())
    })?;
    let mut result = HashMap::with_capacity(len as usize);
    for _ in 0..len {
        let hash = parse_hash28(d)?;
        let coin = Lovelace(d.u64()?);
        result.insert(hash, coin);
    }
    Ok(result)
}

// ---------------------------------------------------------------------------
// RatifyState
// ---------------------------------------------------------------------------

/// Parse RatifyState: array(4)
///   [0] EnactState: array(7)
///   [1] enacted: Seq(GovActionState) = array
///   [2] expired: Set(GovActionId) = array
///   [3] delayed: Bool
fn parse_ratify_state(d: &mut minicbor::Decoder) -> Result<HaskellRatifyState, SerializationError> {
    let len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("RatifyState: expected definite array".into())
    })?;
    if len != 4 {
        return Err(SerializationError::CborDecode(format!(
            "RatifyState: expected array(4), got array({len})"
        )));
    }

    // [0] EnactState
    let enact_state = parse_enact_state(d)?;

    // [1] enacted: Seq(GovActionState)
    let enacted_len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("RatifyState enacted: expected definite array".into())
    })?;
    let mut enacted = Vec::with_capacity(enacted_len as usize);
    for _ in 0..enacted_len {
        enacted.push(parse_gov_action_state(d)?);
    }

    // [2] expired: Set(GovActionId) — encoded as array
    let expired_len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("RatifyState expired: expected definite array".into())
    })?;
    let mut expired = Vec::with_capacity(expired_len as usize);
    for _ in 0..expired_len {
        expired.push(parse_gov_action_id(d)?);
    }

    // [3] delayed: Bool
    let delayed = d.bool()?;

    Ok(HaskellRatifyState {
        enact_state,
        enacted,
        expired,
        delayed,
    })
}

/// Parse EnactState: array(7)
///   [0] committee: StrictMaybe(Committee)
///   [1] constitution: Constitution
///   [2] curPParams: PParams
///   [3] prevPParams: PParams
///   [4] treasury: Coin
///   [5] withdrawals: Map(Credential -> Coin)
///   [6] prevGovActionIds: GovRelation StrictMaybe = array(4)
fn parse_enact_state(d: &mut minicbor::Decoder) -> Result<HaskellEnactState, SerializationError> {
    let len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("EnactState: expected definite array".into())
    })?;
    if len != 7 {
        return Err(SerializationError::CborDecode(format!(
            "EnactState: expected array(7), got array({len})"
        )));
    }

    // [0] StrictMaybe(Committee)
    let committee = parse_strict_maybe_committee(d)?;

    // [1] Constitution
    let constitution = parse_constitution(d)?;

    // [2] curPParams
    let cur_pparams = super::pparams::parse_pparams(d)?;

    // [3] prevPParams
    let prev_pparams = super::pparams::parse_pparams(d)?;

    // [4] treasury: Coin
    let treasury = Lovelace(d.u64()?);

    // [5] withdrawals: Map(Credential -> Coin)
    let wd_len = d.map()?.ok_or_else(|| {
        SerializationError::CborDecode("EnactState withdrawals: expected definite map".into())
    })?;
    let mut withdrawals = HashMap::with_capacity(wd_len as usize);
    for _ in 0..wd_len {
        let cred = super::parse_credential(d)?;
        let coin = Lovelace(d.u64()?);
        withdrawals.insert(cred, coin);
    }

    // [6] prevGovActionIds: GovRelation StrictMaybe = array(4)
    let prev_gov_action_ids = parse_prev_gov_action_ids(d)?;

    Ok(HaskellEnactState {
        committee,
        constitution,
        cur_pparams,
        prev_pparams,
        treasury,
        withdrawals,
        prev_gov_action_ids,
    })
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Parse Anchor: array(2) [url_text, hash_bytes32]
fn parse_anchor(d: &mut minicbor::Decoder) -> Result<HaskellAnchor, SerializationError> {
    let len = d
        .array()?
        .ok_or_else(|| SerializationError::CborDecode("Anchor: expected definite array".into()))?;
    if len != 2 {
        return Err(SerializationError::CborDecode(format!(
            "Anchor: expected array(2), got array({len})"
        )));
    }
    let url = d.str()?.to_string();
    let data_hash = parse_hash32(d)?;
    Ok(HaskellAnchor { url, data_hash })
}

/// Parse StrictMaybe(Anchor): array(0) for SNothing, array(1) [Anchor] for SJust
fn parse_strict_maybe_anchor(
    d: &mut minicbor::Decoder,
) -> Result<Option<HaskellAnchor>, SerializationError> {
    let len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("StrictMaybe Anchor: expected definite array".into())
    })?;
    match len {
        0 => Ok(None),
        1 => Ok(Some(parse_anchor(d)?)),
        _ => Err(SerializationError::CborDecode(format!(
            "StrictMaybe Anchor: expected array(0) or array(1), got array({len})"
        ))),
    }
}

/// Parse StrictMaybe ScriptHash: array(0) for SNothing, array(1) [bytes32] for SJust
fn parse_strict_maybe_script_hash(
    d: &mut minicbor::Decoder,
) -> Result<Option<Hash32>, SerializationError> {
    let len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("StrictMaybe ScriptHash: expected definite array".into())
    })?;
    match len {
        0 => Ok(None),
        1 => Ok(Some(parse_hash32(d)?)),
        _ => Err(SerializationError::CborDecode(format!(
            "StrictMaybe ScriptHash: expected array(0) or array(1), got array({len})"
        ))),
    }
}

/// Parse a 28-byte hash from CBOR bytes
fn parse_hash28(d: &mut minicbor::Decoder) -> Result<Hash28, SerializationError> {
    let bytes = d.bytes()?;
    if bytes.len() != 28 {
        return Err(SerializationError::CborDecode(format!(
            "Hash28: expected 28 bytes, got {}",
            bytes.len()
        )));
    }
    let mut hash = [0u8; 28];
    hash.copy_from_slice(bytes);
    Ok(Hash28::from_bytes(hash))
}

/// Parse a 32-byte hash from CBOR bytes
fn parse_hash32(d: &mut minicbor::Decoder) -> Result<Hash32, SerializationError> {
    let bytes = d.bytes()?;
    if bytes.len() != 32 {
        return Err(SerializationError::CborDecode(format!(
            "Hash32: expected 32 bytes, got {}",
            bytes.len()
        )));
    }
    let mut hash = [0u8; 32];
    hash.copy_from_slice(bytes);
    Ok(Hash32::from_bytes(hash))
}
