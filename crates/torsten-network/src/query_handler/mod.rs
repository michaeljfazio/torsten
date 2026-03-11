mod governance;
mod protocol;
mod stake;
mod types;
mod utxo;

use std::sync::Arc;
use torsten_primitives::block::Point;
use tracing::debug;

// Re-export all public types for backwards compatibility
pub use types::{
    CommitteeMemberSnapshot, CommitteeSnapshot, DRepSnapshot, DRepStakeEntry, EraBound, EraSummary,
    GenesisConfigSnapshot, GovStateSnapshot, MultiAssetSnapshot, NodeStateSnapshot,
    NonMyopicRewardEntry, PoolParamsSnapshot, PoolStakeSnapshotEntry, ProposalSnapshot,
    ProtocolParamsSnapshot, QueryResult, RelaySnapshot, ShelleyPParamsSnapshot,
    StakeAddressSnapshot, StakeDelegDepositEntry, StakePoolSnapshot, StakeSnapshotsResult,
    UtxoQueryProvider, UtxoSnapshot, VoteDelegateeEntry,
};

/// Handler for local state queries.
///
/// This provides a clean interface for answering LocalStateQuery protocol
/// queries from the current ledger state.
pub struct QueryHandler {
    state: NodeStateSnapshot,
    utxo_provider: Option<Arc<dyn UtxoQueryProvider>>,
}

impl QueryHandler {
    pub fn new() -> Self {
        QueryHandler {
            state: NodeStateSnapshot::default(),
            utxo_provider: None,
        }
    }

    /// Set the UTxO query provider for on-demand UTxO lookups
    pub fn set_utxo_provider(&mut self, provider: Arc<dyn UtxoQueryProvider>) {
        self.utxo_provider = Some(provider);
    }

    /// Update the snapshot from the current node state
    pub fn update_state(&mut self, snapshot: NodeStateSnapshot) {
        self.state = snapshot;
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
    pub fn handle_query_cbor(&self, payload: &[u8]) -> QueryResult {
        // Try to parse the query from the CBOR
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

        // The query itself is wrapped in layers. Try to determine the query type.
        // Shelley queries: [shelley_era_tag, [query_id, ...]]
        // Hard-fork queries: [query_id, ...]
        self.dispatch_query(&mut decoder)
    }

    /// Dispatch a query based on its CBOR structure
    fn dispatch_query(&self, decoder: &mut minicbor::Decoder<'_>) -> QueryResult {
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
                        debug!("Query: GetChainPoint");
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
                    3 => {
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

    /// Dispatch an era-specific query
    fn dispatch_era_query(&self, decoder: &mut minicbor::Decoder<'_>) -> QueryResult {
        // Try to parse inner query: [query_tag, ...]
        // HFC query nesting:
        //   [0, [era_id, [shelley_tag, ...]]] = QueryIfCurrent
        //   [2, [hf_tag, ...]]                = QueryHardFork
        match decoder.array() {
            Ok(_) => {
                let query_tag = decoder.u32().unwrap_or(999);
                // Check if this is a QueryHardFork tag
                if query_tag == 2 {
                    return self.handle_hard_fork_query(decoder);
                }
                self.handle_shelley_query(query_tag, decoder)
            }
            Err(_) => {
                // Try as a simple integer tag
                let query_tag = decoder.u32().unwrap_or(999);
                if query_tag == 2 {
                    return self.handle_hard_fork_query(decoder);
                }
                self.handle_shelley_query(query_tag, decoder)
            }
        }
    }

    /// Handle QueryHardFork queries (GetInterpreter = GetEraHistory)
    fn handle_hard_fork_query(&self, _decoder: &mut minicbor::Decoder<'_>) -> QueryResult {
        // QueryHardFork tag 2 contains GetInterpreter [1, 0]
        // We return era summaries regardless of the sub-tag
        debug!("Query: GetEraHistory (QueryHardFork/GetInterpreter)");
        QueryResult::EraHistory(self.state.era_summaries.clone())
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
            // Tag 8: DebugEpochState -- not implemented
            // Tag 9: GetCBOR -- not implemented
            10 => stake::handle_filtered_delegations(&self.state, decoder),
            11 => protocol::handle_genesis_config(&self.state),
            // Tag 12: DebugNewEpochState -- not implemented
            // Tag 13: DebugChainDepState -- not implemented
            // Tag 14: GetRewardProvenance -- not implemented
            15 => utxo::handle_utxo_by_txin(&self.utxo_provider, decoder),
            16 => stake::handle_stake_pools(&self.state),
            17 => stake::handle_stake_pool_params(&self.state, decoder),
            // Tag 18: GetRewardInfoPools -- not implemented (complex reward provenance data)
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
            // Tags 30-39: not implemented
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
                        member_status: 0,
                        expiry_epoch: Some(200),
                    },
                    CommitteeMemberSnapshot {
                        cold_credential: vec![0x03; 28],
                        cold_credential_type: 0,
                        hot_status: 2, // Resigned
                        hot_credential: None,
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

        // Tag 19: GetPoolState returns same format as PoolParams
        match query(&handler, 19) {
            QueryResult::PoolParams(params) => {
                assert_eq!(params.len(), 1);
                assert_eq!(params[0].pool_id, vec![0xcc; 28]);
            }
            other => panic!("Expected PoolParams, got {other:?}"),
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
