use torsten_primitives::block::Point;

/// Local State Query mini-protocol (node-to-client)
///
/// Allows clients to query the current ledger state at a specific point.
/// This is how wallets query UTxOs, protocol parameters, stake distribution, etc.

#[derive(Debug, Clone)]
pub enum LocalStateQueryMessage {
    // Client messages
    Acquire(Option<Point>),
    ReAcquire(Option<Point>),
    Query(QueryRequest),
    Release,
    Done,

    // Server messages
    Acquired,
    Failure(AcquireFailure),
    Result(QueryResponse),
}

#[derive(Debug, Clone)]
pub enum AcquireFailure {
    PointTooOld,
    PointNotOnChain,
}

/// Supported query types
#[derive(Debug, Clone)]
pub enum QueryRequest {
    /// Get current protocol parameters
    GetCurrentPParams,
    /// Get UTxO set (optionally filtered by address)
    GetUTxOByAddress(Vec<Vec<u8>>),
    /// Get the whole UTxO set
    GetUTxOWhole,
    /// Get current epoch number
    GetEpochNo,
    /// Get stake distribution
    GetStakeDistribution,
    /// Get genesis configuration
    GetGenesisConfig,
    /// Get current era
    GetCurrentEra,
    /// Get system start time
    GetSystemStart,
    /// Get chain tip
    GetChainTip,
    /// Get stake pool parameters
    GetStakePoolParams(Vec<Vec<u8>>),
    /// Get rewards for stake credentials
    GetRewardInfoPools,
    /// Get chain block number
    GetChainBlockNo,
    /// Get governance state (Conway)
    GetGovState,
    /// Get DRep state (Conway)
    GetDRepState(Vec<Vec<u8>>),
    /// Get committee state (Conway)
    GetCommitteeState,
}

/// Query response
#[derive(Debug, Clone)]
pub enum QueryResponse {
    /// Raw CBOR response
    Cbor(Vec<u8>),
    /// Error
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalStateQueryState {
    StIdle,
    StAcquiring,
    StAcquired,
    StQuerying,
    StDone,
}
