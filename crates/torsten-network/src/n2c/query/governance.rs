//! Governance query handlers: DRep, committee, constitution, proposals, voting.
//!
//! Tags handled (via `QueryHandler::handle_query_cbor`):
//!   - 23 GetConstitution
//!   - 24 GetGovState
//!   - 25 GetDRepState
//!   - 27 GetCommitteeState
//!   - 28 GetFilteredVoteDelegatees
//!   - 29 GetDRepStakeDistr
//!   - 31 GetProposals
//!   - 32 GetRatifyState
//!   - 33 GetFuturePParams
//!   - 35 GetStakePoolDefaultVote
//!
//! The actual CBOR serialization lives in `encoding.rs`; this module
//! documents which `QueryResult` variants correspond to which protocol tags.
