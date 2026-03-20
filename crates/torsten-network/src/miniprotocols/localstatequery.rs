use torsten_primitives::block::Point;

/// Local State Query mini-protocol (node-to-client)
///
/// Allows clients to query the current ledger state at a specific point.
/// This is how wallets query UTxOs, protocol parameters, stake distribution, etc.

#[derive(Debug, Clone)]
#[allow(dead_code)]
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
#[allow(dead_code)]
pub enum AcquireFailure {
    PointTooOld,
    PointNotOnChain,
}

/// Supported query types
#[derive(Debug, Clone)]
#[allow(clippy::enum_variant_names, dead_code)]
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
#[allow(dead_code)]
pub enum QueryResponse {
    /// Raw CBOR response
    Cbor(Vec<u8>),
    /// Error
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::enum_variant_names, dead_code)]
pub enum LocalStateQueryState {
    StIdle,
    StAcquiring,
    StAcquired,
    StQuerying,
    StDone,
}

#[cfg(test)]
mod tests {
    use super::*;
    use torsten_primitives::block::Point;

    // ── LocalStateQueryState transitions ────────────────────────────────────

    #[test]
    fn test_state_variants_are_distinct() {
        // All state variants must be distinct for the state machine to work.
        let states = [
            LocalStateQueryState::StIdle,
            LocalStateQueryState::StAcquiring,
            LocalStateQueryState::StAcquired,
            LocalStateQueryState::StQuerying,
            LocalStateQueryState::StDone,
        ];
        for (i, s1) in states.iter().enumerate() {
            for (j, s2) in states.iter().enumerate() {
                if i != j {
                    assert_ne!(s1, s2, "All LSQ states must be distinct");
                }
            }
        }
    }

    // ── AcquireFailure variants ───────────────────────────────────────────────

    #[test]
    fn test_acquire_failure_debug_is_stable() {
        // Verify that the two failure reasons format differently (not the same string).
        let too_old = format!("{:?}", AcquireFailure::PointTooOld);
        let not_on_chain = format!("{:?}", AcquireFailure::PointNotOnChain);
        assert_ne!(too_old, not_on_chain);
    }

    // ── LocalStateQueryMessage construction ──────────────────────────────────

    #[test]
    fn test_acquire_with_none_point_is_tip() {
        // Acquire(None) means "acquire at tip" (no specific point).
        let msg = LocalStateQueryMessage::Acquire(None);
        assert!(matches!(msg, LocalStateQueryMessage::Acquire(None)));
    }

    #[test]
    fn test_acquire_with_specific_point() {
        // Acquire(Some(point)) should hold the specified chain point.
        let point = Some(Point::Origin);
        let msg = LocalStateQueryMessage::Acquire(point.clone());
        assert!(matches!(
            msg,
            LocalStateQueryMessage::Acquire(Some(Point::Origin))
        ));
    }

    #[test]
    fn test_query_response_cbor_wraps_raw_bytes() {
        // QueryResponse::Cbor should preserve arbitrary byte payloads unchanged.
        let payload = vec![0x82, 0x01, 0x02];
        let response = QueryResponse::Cbor(payload.clone());
        match response {
            QueryResponse::Cbor(bytes) => assert_eq!(bytes, payload),
            _ => panic!("Expected Cbor response"),
        }
    }

    #[test]
    fn test_query_response_error_carries_message() {
        // QueryResponse::Error should carry an error string.
        let msg = "query failed: unknown era".to_string();
        let response = QueryResponse::Error(msg.clone());
        match response {
            QueryResponse::Error(e) => assert_eq!(e, msg),
            _ => panic!("Expected Error response"),
        }
    }
}
