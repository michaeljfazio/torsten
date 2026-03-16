//! Ledger query handlers: UTxO, stake, and pool queries.
//!
//! Tags handled (via `QueryHandler::handle_query_cbor`):
//!   - 0  GetUTxO (deprecated)
//!   - 1  GetFilteredUTxO
//!   - 4  GetUTxOByTxIn
//!   - 5  GetUTxOWhole
//!   - 7  GetNonMyopicMemberRewards
//!   - 10 GetStakeDistribution
//!   - 15 GetStakeAddresses
//!   - 17 GetStakeSnapshots
//!   - 19 GetPoolState
//!   - 20 GetStakePools
//!   - 30 GetSPOStakeDistr
//!   - 34 GetLedgerPeerSnapshot
//!   - 36 GetPoolDistr2
//!   - 37 GetStakeDistribution2
//!
//! The actual CBOR serialization lives in `encoding.rs`; this module
//! documents which `QueryResult` variants correspond to which protocol tags.
