//! Protocol parameter and era queries.
//!
//! Tags handled (via `QueryHandler::handle_query_cbor`):
//!   - 0  (outer) GetSystemStart          → `QueryResult::SystemStart`
//!   - 1  (outer) GetChainBlockNo         → `QueryResult::ChainBlockNo`
//!   - 2  (outer) GetChainPoint           → `QueryResult::ChainPoint`
//!   - 3  (BlockQuery) GetCurrentEra      → `QueryResult::CurrentEra`
//!   - 6  GetCurrentPParams               → `QueryResult::ProtocolParams`
//!   - 11 GetGenesisConfig                → `QueryResult::GenesisConfig`
//!   - 14 GetRewardProvenance             → `QueryResult::RewardProvenance`
//!   - 16 GetAccountState                 → `QueryResult::AccountState`
//!   - 18 GetRewardInfoPools              → `QueryResult::RewardInfoPools`
//!   - 21 GetProposedPParamsUpdates       → `QueryResult::ProposedPParamsUpdates`
//!   - 22 GetChainTip                     → `QueryResult::ChainTip`
//!   - 26 GetEpochNo                      → `QueryResult::EpochNo`
//!   - 38 GetMaxMajorProtocolVersion      → `QueryResult::MaxMajorProtocolVersion`
//!   - (HardFork) GetCurrentEra           → `QueryResult::HardForkCurrentEra`
//!   - (HardFork) GetInterpreter          → `QueryResult::EraHistory`
//!
//! The actual CBOR serialization lives in `encoding.rs`; this module
//! documents which `QueryResult` variants correspond to which protocol tags.
