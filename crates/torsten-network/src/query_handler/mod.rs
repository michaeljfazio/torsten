mod governance;
pub mod protocol;
mod stake;
mod types;
mod utxo;

use std::sync::Arc;
use torsten_primitives::block::Point;
use tracing::{debug, warn};

// Re-export all public types for backwards compatibility
pub use types::{
    CommitteeMemberSnapshot, CommitteeSnapshot, DRepSnapshot, DRepStakeEntry, EraBound, EraSummary,
    GenesisConfigSnapshot, GovActionId, GovStateSnapshot, LedgerPeerEntry, MultiAssetSnapshot,
    NodeStateSnapshot, NonMyopicRewardEntry, PoolParamsSnapshot, PoolRewardInfo,
    PoolStakeSnapshotEntry, ProposalSnapshot, ProtocolParamsSnapshot, QueryResult, RelaySnapshot,
    ShelleyPParamsSnapshot, SnapshotStakeData, StakeAddressSnapshot, StakeDelegDepositEntry,
    StakePoolSnapshot, StakeSnapshotsResult, UtxoQueryProvider, UtxoSnapshot, VoteDelegateeEntry,
};

/// Handler for local state queries.
///
/// This provides a clean interface for answering LocalStateQuery protocol
/// queries from the current ledger state.
pub struct QueryHandler {
    state: Arc<NodeStateSnapshot>,
    utxo_provider: Option<Arc<dyn UtxoQueryProvider>>,
    /// Negotiated N2C protocol version for the current query (set per-dispatch).
    /// Used to gate deprecated queries. 0 = no gating (tests, internal use).
    n2c_version: std::sync::atomic::AtomicU16,
}

impl QueryHandler {
    pub fn new() -> Self {
        QueryHandler {
            state: Arc::new(NodeStateSnapshot::default()),
            utxo_provider: None,
            n2c_version: std::sync::atomic::AtomicU16::new(0),
        }
    }

    /// Set the UTxO query provider for on-demand UTxO lookups
    pub fn set_utxo_provider(&mut self, provider: Arc<dyn UtxoQueryProvider>) {
        self.utxo_provider = Some(provider);
    }

    /// Update the snapshot from the current node state.
    /// This is a cheap Arc pointer swap — no deep cloning of the snapshot data.
    pub fn update_state(&mut self, snapshot: NodeStateSnapshot) {
        self.state = Arc::new(snapshot);
    }

    /// Get a reference to the current node state snapshot
    pub fn state(&self) -> &NodeStateSnapshot {
        &self.state
    }

    /// Handle a raw CBOR query message and return a result.
    ///
    /// The CBOR payload from MsgQuery is: [3, query]
    /// where query is a nested structure depending on the query type.
    /// For Shelley-based eras, it's typically: [era_tag, [query_tag, ...]]
    /// Handle a CBOR-encoded query without version gating (backward compat).
    pub fn handle_query_cbor(&self, payload: &[u8]) -> QueryResult {
        self.handle_query_cbor_versioned(payload, 0)
    }

    /// Handle a CBOR-encoded query with version gating.
    ///
    /// `negotiated_version` is the N2C protocol version negotiated during
    /// handshake (16–22). Deprecated queries are rejected for newer versions:
    /// - Tag 4 (GetProposedPParamsUpdates): deprecated at V20+ (era < 12)
    /// - Tag 5 (GetStakeDistribution): deprecated at V21+ (use tag 37)
    /// - Tag 21 (GetPoolDistr): deprecated at V21+ (use tag 36)
    pub fn handle_query_cbor_versioned(
        &self,
        payload: &[u8],
        negotiated_version: u16,
    ) -> QueryResult {
        // Store version for use by shelley query dispatch.
        self.dispatch_query_versioned(payload, negotiated_version)
    }

    fn dispatch_query_versioned(&self, payload: &[u8], negotiated_version: u16) -> QueryResult {
        let mut decoder = minicbor::Decoder::new(payload);

        // Skip the message envelope [3, query]
        match decoder.array() {
            Ok(_) => {}
            Err(e) => return QueryResult::Error(format!("Invalid query CBOR: {e}")),
        }
        match decoder.u32() {
            Ok(3) => {} // MsgQuery tag
            Ok(other) => return QueryResult::Error(format!("Expected MsgQuery(3), got {other}")),
            Err(e) => return QueryResult::Error(format!("Invalid query tag: {e}")),
        }

        self.dispatch_query_with_version(&mut decoder, negotiated_version)
    }

    /// Version-aware query dispatch. Threads `negotiated_version` through to
    /// `handle_shelley_query` for deprecated query gating.
    fn dispatch_query_with_version(
        &self,
        decoder: &mut minicbor::Decoder<'_>,
        negotiated_version: u16,
    ) -> QueryResult {
        self.n2c_version
            .store(negotiated_version, std::sync::atomic::Ordering::Relaxed);
        self.dispatch_query_inner(decoder, negotiated_version)
    }

    fn dispatch_query_inner(
        &self,
        decoder: &mut minicbor::Decoder<'_>,
        _negotiated_version: u16,
    ) -> QueryResult {
        // The query structure varies. Try to detect common patterns.
        // GetSystemStart has no era wrapping: just the tag 2
        // GetCurrentEra has tag 0 at the top level
        // Shelley-based queries are nested: [era, [query_tag, ...]]

        let pos = decoder.position();

        // Try to decode as an array first
        match decoder.array() {
            Ok(Some(len)) => {
                let tag = match decoder.u32() {
                    Ok(t) => t,
                    Err(_) => {
                        decoder.set_position(pos);
                        return self.handle_simple_query(decoder);
                    }
                };

                match tag {
                    0 => {
                        // Outer tag 0 = BlockQuery (era-wrapped) or GetCurrentEra
                        if len == 1 {
                            debug!("Query: GetCurrentEra");
                            return QueryResult::CurrentEra(self.state.era);
                        }
                        // Era-wrapped query: [0, [era_id, [query_tag, ...]]]
                        self.dispatch_era_query(decoder)
                    }
                    1 => {
                        // Outer tag 1 = GetSystemStart
                        debug!("Query: GetSystemStart");
                        QueryResult::SystemStart(self.state.system_start.clone())
                    }
                    2 => {
                        // Outer tag 2 = GetChainBlockNo (QueryVersion2, N2C v16+)
                        debug!("Query: GetChainBlockNo");
                        QueryResult::ChainBlockNo(self.state.block_number.0)
                    }
                    3 => {
                        // Outer tag 3 = GetChainPoint (QueryVersion2, N2C v16+)
                        // Returns Point: [] for Origin, [slot, hash] for Specific
                        debug!("Query: GetChainPoint");
                        match &self.state.tip.point {
                            Point::Origin => QueryResult::ChainPoint {
                                slot: 0,
                                hash: vec![],
                            },
                            Point::Specific(s, h) => QueryResult::ChainPoint {
                                slot: s.0,
                                hash: h.to_vec(),
                            },
                        }
                    }
                    _ => {
                        // May be era-wrapped
                        self.dispatch_era_query(decoder)
                    }
                }
            }
            Ok(None) => {
                // Indefinite array
                let tag = decoder.u32().unwrap_or(999);
                match tag {
                    0 => {
                        // Try era-wrapped first, fall back to GetCurrentEra
                        self.dispatch_era_query(decoder)
                    }
                    1 => QueryResult::SystemStart(self.state.system_start.clone()),
                    2 => QueryResult::ChainBlockNo(self.state.block_number.0),
                    3 => match &self.state.tip.point {
                        Point::Origin => QueryResult::ChainPoint {
                            slot: 0,
                            hash: vec![],
                        },
                        Point::Specific(s, h) => QueryResult::ChainPoint {
                            slot: s.0,
                            hash: h.to_vec(),
                        },
                    },
                    _ => self.dispatch_era_query(decoder),
                }
            }
            Err(_) => {
                decoder.set_position(pos);
                self.handle_simple_query(decoder)
            }
        }
    }

    /// Handle a simple (non-array) query
    fn handle_simple_query(&self, decoder: &mut minicbor::Decoder<'_>) -> QueryResult {
        match decoder.u32() {
            Ok(0) => QueryResult::CurrentEra(self.state.era),
            Ok(1) => QueryResult::SystemStart(self.state.system_start.clone()),
            Ok(2) => QueryResult::ChainBlockNo(self.state.block_number.0),
            _ => QueryResult::Error("Unknown simple query".into()),
        }
    }

    /// Dispatch a BlockQuery (the inner encoding after outer tag 0 in QueryVersion2).
    ///
    /// The HFC `BlockQuery (HardForkBlock xs)` has three constructors:
    ///   `[0, ns_query]`    = QueryIfCurrent — NS-encoded era-specific Shelley query
    ///   `[1, anytime_q]`   = QueryAnytime   — GetEraStart, GetCurrentEra
    ///   `[2, hf_query]`    = QueryHardFork   — GetInterpreter (EraHistory), GetCurrentEra
    ///
    /// QueryIfCurrent inner encoding (NS): `[era_idx, [shelley_tag, ...]]`
    /// QueryAnytime inner encoding: `[sub_tag]` (0=GetEraStart, 2=GetCurrentEra)
    /// QueryHardFork inner encoding: `[sub_tag]` (0=GetInterpreter, 1=GetCurrentEra)
    ///
    /// We also accept a simplified (non-standard) format from torsten-cli
    /// where the Shelley query is sent directly without BlockQuery/NS wrapping.
    fn dispatch_era_query(&self, decoder: &mut minicbor::Decoder<'_>) -> QueryResult {
        let pos = decoder.position();

        match decoder.array() {
            Ok(Some(2)) => {
                let block_query_tag = decoder.u32().unwrap_or(999);
                match block_query_tag {
                    0 => {
                        // QueryIfCurrent: NS-encoded [era_idx, [shelley_tag, ...]]
                        debug!("dispatch_era_query: QueryIfCurrent");
                        self.dispatch_query_if_current(decoder)
                    }
                    1 => {
                        // QueryAnytime: [sub_tag]
                        debug!("dispatch_era_query: QueryAnytime");
                        self.handle_query_anytime(decoder)
                    }
                    2 => {
                        // QueryHardFork: [sub_tag]
                        debug!("dispatch_era_query: QueryHardFork");
                        self.handle_hard_fork_query(decoder)
                    }
                    other => {
                        // Might be a direct Shelley query [tag, args] from torsten-cli
                        debug!(query_tag = other, "dispatch_era_query: direct [tag, args]");
                        self.handle_shelley_query(other, decoder)
                    }
                }
            }
            Ok(Some(len)) => {
                // Length != 2: direct Shelley query [tag] or [tag, arg1, ...]
                let query_tag = decoder.u32().unwrap_or(999);
                debug!(query_tag, len, "dispatch_era_query: direct Shelley query");
                self.handle_shelley_query(query_tag, decoder)
            }
            Ok(None) => {
                let query_tag = decoder.u32().unwrap_or(999);
                self.handle_shelley_query(query_tag, decoder)
            }
            Err(e) => {
                decoder.set_position(pos);
                warn!("dispatch_era_query: array decode failed: {e}");
                let query_tag = decoder.u32().unwrap_or(999);
                self.handle_shelley_query(query_tag, decoder)
            }
        }
    }

    /// Parse a QueryIfCurrent query: NS-encoded `[era_idx, [shelley_tag, ...]]`
    fn dispatch_query_if_current(&self, decoder: &mut minicbor::Decoder<'_>) -> QueryResult {
        match decoder.array() {
            Ok(Some(2)) => {
                let era_idx = decoder.u32().unwrap_or(0);
                // Parse the inner Shelley query
                match decoder.array() {
                    Ok(_) => {
                        let query_tag = decoder.u32().unwrap_or(999);
                        debug!(era_idx, query_tag, "QueryIfCurrent: NS Shelley query");
                        self.handle_shelley_query(query_tag, decoder)
                    }
                    Err(_) => {
                        let query_tag = decoder.u32().unwrap_or(999);
                        self.handle_shelley_query(query_tag, decoder)
                    }
                }
            }
            Ok(_) => {
                // Non-standard: might be direct [shelley_tag] from torsten-cli
                let query_tag = decoder.u32().unwrap_or(999);
                self.handle_shelley_query(query_tag, decoder)
            }
            Err(_) => QueryResult::Error("Invalid QueryIfCurrent encoding".into()),
        }
    }

    /// Handle QueryAnytime queries (embedded in BlockQuery).
    /// Sub-tags: 0=GetEraStart, 2=GetCurrentEra
    fn handle_query_anytime(&self, decoder: &mut minicbor::Decoder<'_>) -> QueryResult {
        let sub_tag = match decoder.array() {
            Ok(_) => decoder.u32().unwrap_or(999),
            Err(_) => decoder.u32().unwrap_or(999),
        };
        match sub_tag {
            0 => {
                debug!("QueryAnytime: GetEraStart");
                // Return era start info — for now return system start
                QueryResult::SystemStart(self.state.system_start.clone())
            }
            2 => {
                debug!("QueryAnytime: GetCurrentEra");
                QueryResult::CurrentEra(self.state.era)
            }
            other => {
                warn!("Unknown QueryAnytime sub-tag: {other}");
                QueryResult::Error(format!("Unknown QueryAnytime sub-tag: {other}"))
            }
        }
    }

    /// Handle GetCBOR (tag 9) — wraps an inner query and returns its result as raw CBOR bytes.
    /// Wire format: tag(24) <cbor_bytes>
    fn handle_get_cbor(&self, decoder: &mut minicbor::Decoder<'_>) -> QueryResult {
        debug!("Query: GetCBOR");
        // The argument is the inner query to execute
        // Parse inner query tag
        let inner_result = match decoder.array() {
            Ok(_) => {
                let inner_tag = decoder.u32().unwrap_or(999);
                self.handle_shelley_query(inner_tag, decoder)
            }
            Err(_) => {
                let inner_tag = decoder.u32().unwrap_or(999);
                self.handle_shelley_query(inner_tag, decoder)
            }
        };
        // Wrap the result to be encoded as CBOR-in-CBOR (tag 24)
        QueryResult::WrappedCbor(Box::new(inner_result))
    }

    /// Handle QueryHardFork queries (GetInterpreter = GetEraHistory)
    /// Handle QueryHardFork queries (embedded in BlockQuery tag 2).
    /// Sub-tags: 0=GetInterpreter (EraHistory), 1=GetCurrentEra
    fn handle_hard_fork_query(&self, decoder: &mut minicbor::Decoder<'_>) -> QueryResult {
        let sub_tag = match decoder.array() {
            Ok(_) => decoder.u32().unwrap_or(999),
            Err(_) => decoder.u32().unwrap_or(999),
        };
        match sub_tag {
            0 => {
                debug!("QueryHardFork: GetInterpreter (EraHistory)");
                QueryResult::EraHistory(self.state.era_summaries.clone())
            }
            1 => {
                debug!("QueryHardFork: GetCurrentEra");
                QueryResult::HardForkCurrentEra(self.state.era)
            }
            other => {
                warn!("Unknown QueryHardFork sub-tag: {other}");
                QueryResult::Error(format!("Unknown QueryHardFork sub-tag: {other}"))
            }
        }
    }

    /// Handle Shelley-era queries by tag.
    ///
    /// Tag numbers match the Haskell cardano-ledger `BlockQuery` encoding
    /// from ouroboros-consensus-shelley `encodeShelleyQuery`.
    pub(crate) fn handle_shelley_query(
        &self,
        query_tag: u32,
        decoder: &mut minicbor::Decoder<'_>,
    ) -> QueryResult {
        // Version-gate deprecated queries per Haskell versionGate.
        // When negotiated_version > 0 (real client), reject deprecated tags.
        let version = self.n2c_version.load(std::sync::atomic::Ordering::Relaxed);
        if version >= 20 && query_tag == 4 {
            // GetProposedPParamsUpdates: deprecated at V20 (Conway governance replaces it)
            debug!(
                version,
                "Rejecting deprecated GetProposedPParamsUpdates (tag 4) for N2C V{version}"
            );
            return QueryResult::Error(format!(
                "GetProposedPParamsUpdates (tag 4) is deprecated for N2C version {version} (V20+). Use governance proposals instead."
            ));
        }
        if version >= 21 && query_tag == 5 {
            // GetStakeDistribution: deprecated at V21 (replaced by tag 37 GetStakeDistribution2)
            debug!(
                version,
                "Rejecting deprecated GetStakeDistribution (tag 5) for N2C V{version}"
            );
            return QueryResult::Error(format!(
                "GetStakeDistribution (tag 5) is deprecated for N2C version {version} (V21+). Use GetStakeDistribution2 (tag 37) instead."
            ));
        }
        if version >= 21 && query_tag == 21 {
            // GetPoolDistr: deprecated at V21 (replaced by tag 36 GetPoolDistr2)
            debug!(
                version,
                "Rejecting deprecated GetPoolDistr (tag 21) for N2C V{version}"
            );
            return QueryResult::Error(format!(
                "GetPoolDistr (tag 21) is deprecated for N2C version {version} (V21+). Use GetPoolDistr2 (tag 36) instead."
            ));
        }

        match query_tag {
            0 => {
                // Tag 0: GetLedgerTip
                debug!("Query: GetLedgerTip");
                let (slot, hash) = match &self.state.tip.point {
                    Point::Origin => (0, vec![0u8; 32]),
                    Point::Specific(s, h) => (s.0, h.to_vec()),
                };
                QueryResult::ChainTip {
                    slot,
                    hash,
                    block_no: self.state.block_number.0,
                }
            }
            1 => {
                // Tag 1: GetEpochNo
                debug!("Query: GetEpochNo");
                QueryResult::EpochNo(self.state.epoch.0)
            }
            2 => protocol::handle_non_myopic_rewards(&self.state, decoder),
            3 => protocol::handle_current_pparams(&self.state),
            4 => protocol::handle_proposed_pparams_updates(),
            5 => protocol::handle_stake_distribution(&self.state),
            6 => utxo::handle_utxo_by_address(&self.state, &self.utxo_provider, decoder),
            7 => utxo::handle_utxo_whole(),
            8 => protocol::handle_debug_epoch_state(&self.state),
            9 => self.handle_get_cbor(decoder),
            10 => stake::handle_filtered_delegations(&self.state, decoder),
            11 => protocol::handle_genesis_config(&self.state),
            12 => protocol::handle_debug_new_epoch_state(&self.state),
            13 => protocol::handle_debug_chain_dep_state(&self.state),
            14 => protocol::handle_reward_provenance(&self.state),
            15 => utxo::handle_utxo_by_txin(&self.utxo_provider, decoder),
            16 => stake::handle_stake_pools(&self.state),
            17 => stake::handle_stake_pool_params(&self.state, decoder),
            18 => stake::handle_reward_info_pools(&self.state),
            19 => stake::handle_pool_state(&self.state, decoder),
            20 => stake::handle_stake_snapshots(&self.state),
            21 => stake::handle_pool_distr(&self.state, decoder),
            22 => stake::handle_stake_deleg_deposits(&self.state, decoder),
            23 => governance::handle_constitution(&self.state),
            24 => governance::handle_gov_state(&self.state),
            25 => governance::handle_drep_state(&self.state, decoder),
            26 => governance::handle_drep_stake_distr(&self.state),
            27 => governance::handle_committee_state(&self.state),
            28 => governance::handle_filtered_vote_delegatees(&self.state, decoder),
            29 => protocol::handle_account_state(&self.state),
            30 => {
                // Tag 30: GetSPOStakeDistr — filtered SPO stake distribution
                stake::handle_spo_stake_distr(&self.state, decoder)
            }
            31 => {
                // Tag 31: GetProposals — filtered governance proposals
                governance::handle_proposals(&self.state, decoder)
            }
            32 => {
                // Tag 32: GetRatifyState — ratification state
                governance::handle_ratify_state(&self.state)
            }
            33 => {
                // Tag 33: GetFuturePParams — returns Maybe PParams (Nothing)
                debug!("Query: GetFuturePParams");
                QueryResult::NoFuturePParams
            }
            34 => {
                // Tag 34: GetLedgerPeerSnapshot (V19+)
                stake::handle_ledger_peer_snapshot(&self.state)
            }
            35 => {
                // Tag 35: QueryStakePoolDefaultVote (V20+)
                stake::handle_pool_default_vote(&self.state, decoder)
            }
            36 => {
                // Tag 36: GetPoolDistr2 (V21+) — new format with total active stake
                debug!("Query: GetPoolDistr2");
                stake::handle_pool_distr2(&self.state, decoder)
            }
            37 => {
                // Tag 37: GetStakeDistribution2 (V21+) — new PoolDistr format
                debug!("Query: GetStakeDistribution2");
                stake::handle_stake_distribution2(&self.state)
            }
            38 => {
                // Tag 38: GetMaxMajorProtocolVersion (V21+)
                debug!("Query: GetMaxMajorProtocolVersion");
                QueryResult::MaxMajorProtocolVersion(10)
            }
            _ => {
                debug!("Unhandled Shelley query tag: {query_tag}");
                QueryResult::Error(format!("Unsupported query: tag {query_tag}"))
            }
        }
    }
}

impl Default for QueryHandler {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse a set of credential hashes from CBOR.
/// Handles: tag(258) [credential, ...] where credential = [0|1, hash(28)]
/// Also handles plain array of bytes (legacy/simplified format).
fn parse_credential_set(decoder: &mut minicbor::Decoder<'_>) -> Vec<Vec<u8>> {
    let mut hashes = Vec::new();
    // Try to consume tag(258) if present
    let _ = decoder.tag();
    if let Ok(Some(n)) = decoder.array() {
        for _ in 0..n {
            let pos = decoder.position();
            // Try as credential structure: [0|1, hash(28)]
            if let Ok(Some(2)) = decoder.array() {
                let _ = decoder.u32(); // credential type tag
                if let Ok(bytes) = decoder.bytes() {
                    hashes.push(bytes.to_vec());
                }
            } else {
                // Fall back to plain bytes
                decoder.set_position(pos);
                if let Ok(bytes) = decoder.bytes() {
                    hashes.push(bytes.to_vec());
                } else {
                    decoder.set_position(pos);
                    decoder.skip().ok();
                }
            }
        }
    }
    hashes
}

#[cfg(test)]
mod tests {
    use super::*;
    use torsten_primitives::block::Tip;
    use torsten_primitives::hash::Hash32;
    use torsten_primitives::time::{BlockNo, EpochNo, SlotNo};

    /// Helper to call handle_shelley_query with an empty decoder
    fn query(handler: &QueryHandler, tag: u32) -> QueryResult {
        let empty = [0u8; 0];
        let mut decoder = minicbor::Decoder::new(&empty);
        handler.handle_shelley_query(tag, &mut decoder)
    }

    #[test]
    fn test_query_handler_default_state() {
        let handler = QueryHandler::new();
        match query(&handler, 1) {
            QueryResult::EpochNo(e) => assert_eq!(e, 0),
            other => panic!("Expected EpochNo, got {other:?}"),
        }
    }

    #[test]
    fn test_query_handler_epoch() {
        let mut handler = QueryHandler::new();
        handler.update_state(NodeStateSnapshot {
            epoch: EpochNo(500),
            ..Default::default()
        });

        match query(&handler, 1) {
            QueryResult::EpochNo(e) => assert_eq!(e, 500),
            other => panic!("Expected EpochNo, got {other:?}"),
        }
    }

    #[test]
    fn test_query_handler_chain_tip() {
        let hash = Hash32::from_bytes([0xab; 32]);
        let mut handler = QueryHandler::new();
        handler.update_state(NodeStateSnapshot {
            tip: Tip {
                point: Point::Specific(SlotNo(12345), hash),
                block_number: BlockNo(100),
            },
            block_number: BlockNo(100),
            ..Default::default()
        });

        match query(&handler, 0) {
            QueryResult::ChainTip {
                slot,
                hash: h,
                block_no,
            } => {
                assert_eq!(slot, 12345);
                assert_eq!(h, hash.to_vec());
                assert_eq!(block_no, 100);
            }
            other => panic!("Expected ChainTip, got {other:?}"),
        }
    }

    #[test]
    fn test_query_handler_current_era() {
        let handler = QueryHandler::new();
        match query(&handler, 999) {
            QueryResult::Error(_) => {} // Expected for unknown query
            other => panic!("Expected Error, got {other:?}"),
        }
    }

    #[test]
    fn test_query_handler_block_no() {
        let mut handler = QueryHandler::new();
        handler.update_state(NodeStateSnapshot {
            block_number: BlockNo(42000),
            ..Default::default()
        });

        // ChainBlockNo is outer tag 2 -- build a MsgQuery CBOR: [3, [2]]
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(3).unwrap(); // MsgQuery
        enc.array(1).unwrap();
        enc.u32(2).unwrap(); // GetChainBlockNo
        let result = handler.handle_query_cbor(&buf);
        match result {
            QueryResult::ChainBlockNo(n) => assert_eq!(n, 42000),
            other => panic!("Expected ChainBlockNo, got {other:?}"),
        }
    }

    #[test]
    fn test_query_handler_system_start() {
        let handler = QueryHandler::new();
        match query(&handler, 999) {
            QueryResult::Error(_) => {}
            _ => panic!("Expected error for unknown query"),
        }
    }

    #[test]
    fn test_query_result_cbor_roundtrip() {
        // Build a MsgQuery CBOR: [3, [0, [1]]]
        // Outer tag 0 = BlockQuery, inner tag 1 = GetEpochNo
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(3).unwrap(); // MsgQuery
        enc.array(2).unwrap();
        enc.u32(0).unwrap(); // outer: BlockQuery
        enc.array(1).unwrap();
        enc.u32(1).unwrap(); // inner: GetEpochNo

        let handler = QueryHandler::new();
        let result = handler.handle_query_cbor(&buf);
        match result {
            QueryResult::EpochNo(e) => assert_eq!(e, 0),
            other => panic!("Expected EpochNo from CBOR query, got {other:?}"),
        }
    }

    #[test]
    fn test_query_handler_stake_distribution() {
        let mut handler = QueryHandler::new();
        handler.update_state(NodeStateSnapshot {
            stake_pools: vec![
                StakePoolSnapshot {
                    pool_id: vec![0xaa; 28],
                    stake: 1_000_000_000,
                    vrf_keyhash: vec![0x11; 32],
                    total_active_stake: 3_000_000_000,
                },
                StakePoolSnapshot {
                    pool_id: vec![0xbb; 28],
                    stake: 2_000_000_000,
                    vrf_keyhash: vec![0x22; 32],
                    total_active_stake: 3_000_000_000,
                },
            ],
            ..Default::default()
        });

        match query(&handler, 5) {
            QueryResult::StakeDistribution(pools) => {
                assert_eq!(pools.len(), 2);
                assert_eq!(pools[0].pool_id, vec![0xaa; 28]);
                assert_eq!(pools[0].stake, 1_000_000_000);
                assert_eq!(pools[1].pool_id, vec![0xbb; 28]);
            }
            other => panic!("Expected StakeDistribution, got {other:?}"),
        }
    }

    #[test]
    fn test_query_handler_protocol_params() {
        let mut handler = QueryHandler::new();
        handler.update_state(NodeStateSnapshot {
            protocol_params: ProtocolParamsSnapshot {
                min_fee_a: 44,
                min_fee_b: 155381,
                ..Default::default()
            },
            ..Default::default()
        });

        match query(&handler, 3) {
            QueryResult::ProtocolParams(params) => {
                assert_eq!(params.min_fee_a, 44);
                assert_eq!(params.min_fee_b, 155381);
            }
            other => panic!("Expected ProtocolParams, got {other:?}"),
        }
    }

    #[test]
    fn test_query_handler_gov_state() {
        let mut handler = QueryHandler::new();
        handler.update_state(NodeStateSnapshot {
            drep_count: 5,
            treasury: 1_000_000_000_000,
            committee: CommitteeSnapshot {
                members: vec![CommitteeMemberSnapshot {
                    cold_credential: vec![0x01; 28],
                    cold_credential_type: 0,
                    hot_status: 0,
                    hot_credential: Some(vec![0x02; 28]),
                    hot_credential_type: 0,
                    member_status: 0,
                    expiry_epoch: Some(200),
                }],
                ..Default::default()
            },
            governance_proposals: vec![ProposalSnapshot {
                tx_id: vec![0xcc; 32],
                action_index: 0,
                action_type: "InfoAction".to_string(),
                proposed_epoch: 100,
                expires_epoch: 106,
                yes_votes: 3,
                no_votes: 1,
                abstain_votes: 0,
                deposit: 100_000_000_000,
                return_addr: vec![0xdd; 29],
                anchor_url: "https://example.com/proposal".to_string(),
                anchor_hash: vec![0xee; 32],
                committee_votes: vec![],
                drep_votes: vec![],
                spo_votes: vec![],
            }],
            ..Default::default()
        });

        match query(&handler, 24) {
            QueryResult::GovState(gov) => {
                assert_eq!(gov.committee.members.len(), 1);
                assert_eq!(gov.proposals.len(), 1);
                assert_eq!(gov.proposals[0].action_type, "InfoAction");
                assert_eq!(gov.proposals[0].yes_votes, 3);
            }
            other => panic!("Expected GovState, got {other:?}"),
        }
    }

    #[test]
    fn test_query_handler_drep_state() {
        let mut handler = QueryHandler::new();
        handler.update_state(NodeStateSnapshot {
            drep_entries: vec![DRepSnapshot {
                credential_hash: vec![0xdd; 28],
                credential_type: 0,
                deposit: 500_000_000,
                anchor_url: Some("https://example.com/drep".to_string()),
                anchor_hash: Some(vec![0xee; 32]),
                expiry_epoch: 62,
                delegator_hashes: Vec::new(),
            }],
            ..Default::default()
        });

        match query(&handler, 25) {
            QueryResult::DRepState(dreps) => {
                assert_eq!(dreps.len(), 1);
                assert_eq!(dreps[0].credential_hash, vec![0xdd; 28]);
                assert_eq!(dreps[0].deposit, 500_000_000);
                assert_eq!(
                    dreps[0].anchor_url,
                    Some("https://example.com/drep".to_string())
                );
                assert_eq!(dreps[0].expiry_epoch, 62);
            }
            other => panic!("Expected DRepState, got {other:?}"),
        }
    }

    #[test]
    fn test_query_handler_committee_state() {
        let mut handler = QueryHandler::new();
        handler.update_state(NodeStateSnapshot {
            committee: CommitteeSnapshot {
                members: vec![
                    CommitteeMemberSnapshot {
                        cold_credential: vec![0x01; 28],
                        cold_credential_type: 0,
                        hot_status: 0,
                        hot_credential: Some(vec![0x02; 28]),
                        hot_credential_type: 0,
                        member_status: 0,
                        expiry_epoch: Some(200),
                    },
                    CommitteeMemberSnapshot {
                        cold_credential: vec![0x03; 28],
                        cold_credential_type: 0,
                        hot_status: 2, // Resigned
                        hot_credential: None,
                        hot_credential_type: 0,
                        member_status: 0,
                        expiry_epoch: Some(200),
                    },
                ],
                threshold: Some((2, 3)),
                current_epoch: 100,
            },
            ..Default::default()
        });

        match query(&handler, 27) {
            QueryResult::CommitteeState(committee) => {
                assert_eq!(committee.members.len(), 2);
                assert_eq!(committee.members[0].cold_credential, vec![0x01; 28]);
                assert_eq!(committee.members[0].hot_status, 0); // Authorized
                assert_eq!(committee.members[1].hot_status, 2); // Resigned
            }
            other => panic!("Expected CommitteeState, got {other:?}"),
        }
    }

    #[test]
    fn test_query_handler_stake_address_info() {
        let mut handler = QueryHandler::new();
        handler.update_state(NodeStateSnapshot {
            stake_addresses: vec![
                StakeAddressSnapshot {
                    credential_hash: vec![0xaa; 28],
                    delegated_pool: Some(vec![0xbb; 28]),
                    reward_balance: 5_000_000,
                },
                StakeAddressSnapshot {
                    credential_hash: vec![0xcc; 28],
                    delegated_pool: None,
                    reward_balance: 0,
                },
            ],
            ..Default::default()
        });

        match query(&handler, 10) {
            QueryResult::StakeAddressInfo(addrs) => {
                assert_eq!(addrs.len(), 2);
                assert_eq!(addrs[0].credential_hash, vec![0xaa; 28]);
                assert_eq!(addrs[0].delegated_pool, Some(vec![0xbb; 28]));
                assert_eq!(addrs[0].reward_balance, 5_000_000);
                assert_eq!(addrs[1].delegated_pool, None);
            }
            other => panic!("Expected StakeAddressInfo, got {other:?}"),
        }
    }

    #[test]
    fn test_query_handler_utxo_by_address_no_provider() {
        let handler = QueryHandler::new();
        // Without a UtxoQueryProvider, should return empty
        let addr_bytes = vec![0x01; 57]; // fake address bytes
        let mut decoder = minicbor::Decoder::new(&addr_bytes);
        match handler.handle_shelley_query(6, &mut decoder) {
            QueryResult::UtxoByAddress(utxos) => {
                assert!(utxos.is_empty());
            }
            other => panic!("Expected UtxoByAddress, got {other:?}"),
        }
    }

    #[test]
    fn test_query_handler_utxo_by_address_with_provider() {
        struct MockProvider;
        impl UtxoQueryProvider for MockProvider {
            fn utxos_at_address_bytes(&self, _addr_bytes: &[u8]) -> Vec<UtxoSnapshot> {
                vec![UtxoSnapshot {
                    tx_hash: vec![0xaa; 32],
                    output_index: 0,
                    address_bytes: vec![0x01; 57],
                    lovelace: 5_000_000,
                    multi_asset: vec![],
                    datum_hash: None,
                    raw_cbor: None,
                }]
            }
        }

        let mut handler = QueryHandler::new();
        handler.set_utxo_provider(Arc::new(MockProvider));

        let addr_bytes = vec![0x01; 57];
        let mut decoder = minicbor::Decoder::new(&addr_bytes);
        match handler.handle_shelley_query(6, &mut decoder) {
            QueryResult::UtxoByAddress(utxos) => {
                assert_eq!(utxos.len(), 1);
                assert_eq!(utxos[0].lovelace, 5_000_000);
                assert_eq!(utxos[0].tx_hash, vec![0xaa; 32]);
            }
            other => panic!("Expected UtxoByAddress, got {other:?}"),
        }
    }

    #[test]
    fn test_query_handler_gov_state_empty() {
        let handler = QueryHandler::new();
        match query(&handler, 24) {
            QueryResult::GovState(gov) => {
                assert_eq!(gov.proposals.len(), 0);
                assert_eq!(gov.committee.members.len(), 0);
            }
            other => panic!("Expected GovState, got {other:?}"),
        }
    }

    #[test]
    fn test_query_handler_stake_snapshots() {
        let mut handler = QueryHandler::new();
        handler.update_state(NodeStateSnapshot {
            stake_snapshots: StakeSnapshotsResult {
                pools: vec![PoolStakeSnapshotEntry {
                    pool_id: vec![0xaa; 28],
                    mark_stake: 1_000_000,
                    set_stake: 900_000,
                    go_stake: 800_000,
                }],
                total_mark_stake: 1_000_000,
                total_set_stake: 900_000,
                total_go_stake: 800_000,
            },
            ..Default::default()
        });

        match query(&handler, 20) {
            QueryResult::StakeSnapshots(snap) => {
                assert_eq!(snap.pools.len(), 1);
                assert_eq!(snap.pools[0].mark_stake, 1_000_000);
                assert_eq!(snap.pools[0].set_stake, 900_000);
                assert_eq!(snap.pools[0].go_stake, 800_000);
                assert_eq!(snap.total_mark_stake, 1_000_000);
            }
            other => panic!("Expected StakeSnapshots, got {other:?}"),
        }
    }

    #[test]
    fn test_query_handler_pool_params() {
        let mut handler = QueryHandler::new();
        handler.update_state(NodeStateSnapshot {
            pool_params_entries: vec![PoolParamsSnapshot {
                pool_id: vec![0xbb; 28],
                vrf_keyhash: vec![0xcc; 32],
                pledge: 500_000_000,
                cost: 340_000_000,
                margin_num: 3,
                margin_den: 100,
                reward_account: Vec::new(),
                owners: Vec::new(),
                relays: vec![RelaySnapshot::SingleHostName {
                    port: Some(3001),
                    dns_name: "relay1.example.com".to_string(),
                }],
                metadata_url: None,
                metadata_hash: None,
            }],
            ..Default::default()
        });

        match query(&handler, 17) {
            QueryResult::PoolParams(params) => {
                assert_eq!(params.len(), 1);
                assert_eq!(params[0].pool_id, vec![0xbb; 28]);
                assert_eq!(params[0].pledge, 500_000_000);
                assert_eq!(params[0].cost, 340_000_000);
                assert_eq!(params[0].margin_num, 3);
                assert_eq!(params[0].relays.len(), 1);
            }
            other => panic!("Expected PoolParams, got {other:?}"),
        }
    }

    #[test]
    fn test_query_handler_pool_state() {
        let mut handler = QueryHandler::new();
        handler.update_state(NodeStateSnapshot {
            pool_params_entries: vec![PoolParamsSnapshot {
                pool_id: vec![0xcc; 28],
                vrf_keyhash: vec![0xdd; 32],
                pledge: 100_000_000,
                cost: 170_000_000,
                margin_num: 1,
                margin_den: 100,
                reward_account: vec![0xe0; 29],
                owners: vec![vec![0x11; 28]],
                relays: vec![],
                metadata_url: None,
                metadata_hash: None,
            }],
            ..Default::default()
        });

        // Tag 19: GetPoolState returns QueryPoolStateResult (4 parallel maps)
        match query(&handler, 19) {
            QueryResult::PoolState {
                pool_params,
                future_pool_params,
                retiring,
                deposits,
            } => {
                assert_eq!(pool_params.len(), 1);
                assert_eq!(pool_params[0].pool_id, vec![0xcc; 28]);
                assert!(future_pool_params.is_empty());
                assert!(retiring.is_empty());
                assert_eq!(deposits.len(), 1);
                assert_eq!(deposits[0].0, vec![0xcc; 28]);
            }
            other => panic!("Expected PoolState, got {other:?}"),
        }
    }

    #[test]
    fn test_query_handler_pool_distr() {
        let mut handler = QueryHandler::new();
        handler.update_state(NodeStateSnapshot {
            stake_pools: vec![
                StakePoolSnapshot {
                    pool_id: vec![0xaa; 28],
                    stake: 1_000_000_000,
                    vrf_keyhash: vec![0x11; 32],
                    total_active_stake: 3_000_000_000,
                },
                StakePoolSnapshot {
                    pool_id: vec![0xbb; 28],
                    stake: 2_000_000_000,
                    vrf_keyhash: vec![0x22; 32],
                    total_active_stake: 3_000_000_000,
                },
            ],
            ..Default::default()
        });

        // Tag 21: GetPoolDistr
        match query(&handler, 21) {
            QueryResult::PoolDistr(pools) => {
                assert_eq!(pools.len(), 2);
            }
            other => panic!("Expected PoolDistr, got {other:?}"),
        }
    }

    #[test]
    fn test_query_handler_stake_deleg_deposits() {
        let mut handler = QueryHandler::new();
        handler.update_state(NodeStateSnapshot {
            stake_deleg_deposits: vec![
                StakeDelegDepositEntry {
                    credential_hash: vec![0xaa; 28],
                    credential_type: 0,
                    deposit: 2_000_000,
                },
                StakeDelegDepositEntry {
                    credential_hash: vec![0xbb; 28],
                    credential_type: 0,
                    deposit: 2_000_000,
                },
            ],
            ..Default::default()
        });

        // Tag 22: GetStakeDelegDeposits
        match query(&handler, 22) {
            QueryResult::StakeDelegDeposits(deposits) => {
                assert_eq!(deposits.len(), 2);
                assert_eq!(deposits[0].deposit, 2_000_000);
            }
            other => panic!("Expected StakeDelegDeposits, got {other:?}"),
        }
    }

    #[test]
    fn test_query_handler_drep_stake_distr() {
        let mut handler = QueryHandler::new();
        handler.update_state(NodeStateSnapshot {
            drep_stake_distr: vec![
                DRepStakeEntry {
                    drep_type: 0,
                    drep_hash: Some(vec![0xdd; 28]),
                    stake: 500_000_000,
                },
                DRepStakeEntry {
                    drep_type: 2,
                    drep_hash: None,
                    stake: 100_000_000,
                },
            ],
            ..Default::default()
        });

        // Tag 26: GetDRepStakeDistr
        match query(&handler, 26) {
            QueryResult::DRepStakeDistr(entries) => {
                assert_eq!(entries.len(), 2);
                assert_eq!(entries[0].stake, 500_000_000);
            }
            other => panic!("Expected DRepStakeDistr, got {other:?}"),
        }
    }

    #[test]
    fn test_query_handler_filtered_vote_delegatees() {
        let mut handler = QueryHandler::new();
        handler.update_state(NodeStateSnapshot {
            vote_delegatees: vec![
                VoteDelegateeEntry {
                    credential_hash: vec![0xaa; 28],
                    credential_type: 0,
                    drep_type: 0,
                    drep_hash: Some(vec![0xdd; 28]),
                },
                VoteDelegateeEntry {
                    credential_hash: vec![0xbb; 28],
                    credential_type: 0,
                    drep_type: 2,
                    drep_hash: None,
                },
            ],
            ..Default::default()
        });

        // Tag 28: GetFilteredVoteDelegatees
        match query(&handler, 28) {
            QueryResult::FilteredVoteDelegatees(delegatees) => {
                assert_eq!(delegatees.len(), 2);
                assert_eq!(delegatees[0].drep_type, 0);
                assert_eq!(delegatees[1].drep_type, 2);
            }
            other => panic!("Expected FilteredVoteDelegatees, got {other:?}"),
        }
    }

    #[test]
    fn test_query_handler_debug_epoch_state() {
        let mut handler = QueryHandler::new();
        handler.update_state(NodeStateSnapshot {
            epoch: EpochNo(55),
            treasury: 2_000_000,
            reserves: 8_000_000,
            pool_count: 5,
            utxo_count: 100,
            ..NodeStateSnapshot::default()
        });
        match query(&handler, 8) {
            QueryResult::DebugEpochState {
                epoch,
                treasury,
                reserves,
                stake_pool_count,
                utxo_count,
                ..
            } => {
                assert_eq!(epoch, 55);
                assert_eq!(treasury, 2_000_000);
                assert_eq!(reserves, 8_000_000);
                assert_eq!(stake_pool_count, 5);
                assert_eq!(utxo_count, 100);
            }
            other => panic!("Expected DebugEpochState, got {other:?}"),
        }
    }

    #[test]
    fn test_query_handler_get_cbor_wraps_inner() {
        let handler = QueryHandler::new();
        // Build CBOR for inner query: [1] (GetEpochNo)
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(1).unwrap();
        enc.u32(1).unwrap(); // GetEpochNo
        let mut decoder = minicbor::Decoder::new(&buf);
        let result = handler.handle_shelley_query(9, &mut decoder);
        match result {
            QueryResult::WrappedCbor(inner) => match *inner {
                QueryResult::EpochNo(epoch) => {
                    assert_eq!(epoch, 0); // default state epoch
                }
                other => panic!("Expected EpochNo inside WrappedCbor, got {other:?}"),
            },
            other => panic!("Expected WrappedCbor, got {other:?}"),
        }
    }

    #[test]
    fn test_query_handler_debug_new_epoch_state() {
        let mut handler = QueryHandler::new();
        handler.update_state(NodeStateSnapshot {
            epoch: EpochNo(10),
            block_number: BlockNo(500),
            treasury: 1_000_000,
            reserves: 2_000_000,
            tip: Tip {
                point: torsten_primitives::block::Point::Specific(
                    SlotNo(12345),
                    Hash32::from_bytes([0xAA; 32]),
                ),
                block_number: BlockNo(500),
            },
            ..NodeStateSnapshot::default()
        });
        match query(&handler, 12) {
            QueryResult::DebugNewEpochState {
                epoch,
                treasury,
                reserves,
                ..
            } => {
                assert_eq!(epoch, 10);
                assert_eq!(treasury, 1_000_000);
                assert_eq!(reserves, 2_000_000);
            }
            other => panic!("Expected DebugNewEpochState, got {other:?}"),
        }
    }

    #[test]
    fn test_query_handler_debug_chain_dep_state() {
        let mut handler = QueryHandler::new();
        handler.update_state(NodeStateSnapshot {
            tip: Tip {
                point: torsten_primitives::block::Point::Specific(
                    SlotNo(99999),
                    Hash32::from_bytes([0xBB; 32]),
                ),
                block_number: BlockNo(100),
            },
            ..NodeStateSnapshot::default()
        });
        match query(&handler, 13) {
            QueryResult::DebugChainDepState {
                last_slot,
                epoch_nonce,
                evolving_nonce,
                candidate_nonce,
                lab_nonce,
            } => {
                assert_eq!(last_slot, 99999);
                assert_eq!(epoch_nonce.len(), 32);
                assert_eq!(evolving_nonce.len(), 32);
                assert_eq!(candidate_nonce.len(), 32);
                assert_eq!(lab_nonce.len(), 32);
            }
            other => panic!("Expected DebugChainDepState, got {other:?}"),
        }
    }

    #[test]
    fn test_query_handler_reward_provenance() {
        let mut handler = QueryHandler::new();
        handler.update_state(NodeStateSnapshot {
            epoch: EpochNo(42),
            reserves: 10_000_000,
            protocol_params: ProtocolParamsSnapshot {
                rho_num: 3,
                rho_den: 1000,
                tau_num: 2,
                tau_den: 10,
                ..ProtocolParamsSnapshot::default()
            },
            ..NodeStateSnapshot::default()
        });
        match query(&handler, 14) {
            QueryResult::RewardProvenance {
                epoch,
                total_rewards_pot,
                treasury_tax,
                ..
            } => {
                assert_eq!(epoch, 42);
                assert_eq!(total_rewards_pot, 30_000); // 10M * 3/1000
                assert_eq!(treasury_tax, 6_000); // 30K * 2/10
            }
            other => panic!("Expected RewardProvenance, got {other:?}"),
        }
    }

    #[test]
    fn test_query_handler_reward_info_pools() {
        let handler = QueryHandler::new();
        // Default state has no pools, should return empty
        match query(&handler, 18) {
            QueryResult::RewardInfoPools(pools) => {
                assert!(pools.is_empty());
            }
            other => panic!("Expected RewardInfoPools, got {other:?}"),
        }
    }

    #[test]
    fn test_query_handler_unsupported_tag() {
        let handler = QueryHandler::new();
        match query(&handler, 99) {
            QueryResult::Error(msg) => {
                assert!(msg.contains("99"));
            }
            other => panic!("Expected Error, got {other:?}"),
        }
    }
}
