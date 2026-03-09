use std::sync::Arc;
use torsten_primitives::block::{Point, Tip};
use torsten_primitives::time::{BlockNo, EpochNo};
use tracing::debug;

/// Results from local state queries
#[derive(Debug, Clone)]
pub enum QueryResult {
    EpochNo(u64),
    ChainTip {
        slot: u64,
        hash: Vec<u8>,
        block_no: u64,
    },
    CurrentEra(u32),
    SystemStart(String),
    ChainBlockNo(u64),
    ProtocolParams(Box<ProtocolParamsSnapshot>),
    StakeDistribution(Vec<StakePoolSnapshot>),
    GovState(GovStateSnapshot),
    DRepState(Vec<DRepSnapshot>),
    CommitteeState(CommitteeSnapshot),
    StakeAddressInfo(Vec<StakeAddressSnapshot>),
    UtxoByAddress(Vec<UtxoSnapshot>),
    StakeSnapshots(StakeSnapshotsResult),
    PoolParams(Vec<PoolParamsSnapshot>),
    AccountState {
        treasury: u64,
        reserves: u64,
    },
    GenesisConfig {
        system_start: String,
        network_magic: u32,
        epoch_length: u64,
        slot_length_secs: u64,
        security_param: u64,
    },
    /// NonMyopicMemberRewards: map from stake_amount → pool rewards
    NonMyopicMemberRewards(Vec<NonMyopicRewardEntry>),
    Error(String),
}

/// Stake snapshot result (mark/set/go)
#[derive(Debug, Clone, Default)]
pub struct StakeSnapshotsResult {
    pub pools: Vec<PoolStakeSnapshotEntry>,
    pub total_mark_stake: u64,
    pub total_set_stake: u64,
    pub total_go_stake: u64,
}

/// Per-pool stake across mark/set/go snapshots
#[derive(Debug, Clone)]
pub struct PoolStakeSnapshotEntry {
    pub pool_id: Vec<u8>,
    pub mark_stake: u64,
    pub set_stake: u64,
    pub go_stake: u64,
}

/// Pool parameters snapshot
#[derive(Debug, Clone)]
pub struct PoolParamsSnapshot {
    pub pool_id: Vec<u8>,
    pub vrf_keyhash: Vec<u8>,
    pub pledge: u64,
    pub cost: u64,
    pub margin_num: u64,
    pub margin_den: u64,
    pub reward_account: Vec<u8>,
    pub owners: Vec<Vec<u8>>,
    pub relays: Vec<String>,
    pub metadata_url: Option<String>,
    pub metadata_hash: Option<Vec<u8>>,
}

/// Protocol parameters snapshot for CBOR encoding in N2C queries
#[derive(Debug, Clone)]
pub struct ProtocolParamsSnapshot {
    pub min_fee_a: u64,
    pub min_fee_b: u64,
    pub max_block_body_size: u64,
    pub max_tx_size: u64,
    pub max_block_header_size: u64,
    pub key_deposit: u64,
    pub pool_deposit: u64,
    pub e_max: u64,
    pub n_opt: u64,
    pub a0_num: u64,
    pub a0_den: u64,
    pub rho_num: u64,
    pub rho_den: u64,
    pub tau_num: u64,
    pub tau_den: u64,
    pub min_pool_cost: u64,
    pub ada_per_utxo_byte: u64,
    pub cost_models_v1: Option<Vec<i64>>,
    pub cost_models_v2: Option<Vec<i64>>,
    pub cost_models_v3: Option<Vec<i64>>,
    pub execution_costs_mem_num: u64,
    pub execution_costs_mem_den: u64,
    pub execution_costs_step_num: u64,
    pub execution_costs_step_den: u64,
    pub max_tx_ex_mem: u64,
    pub max_tx_ex_steps: u64,
    pub max_block_ex_mem: u64,
    pub max_block_ex_steps: u64,
    pub max_val_size: u64,
    pub collateral_percentage: u64,
    pub max_collateral_inputs: u64,
    pub protocol_version_major: u64,
    pub protocol_version_minor: u64,
    pub min_fee_ref_script_cost_per_byte: u64,
    // Conway governance
    pub drep_deposit: u64,
    pub drep_activity: u64,
    pub gov_action_deposit: u64,
    pub gov_action_lifetime: u64,
    pub committee_min_size: u64,
    pub committee_max_term_length: u64,
    pub dvt_p_param_change_num: u64,
    pub dvt_p_param_change_den: u64,
    pub dvt_hard_fork_num: u64,
    pub dvt_hard_fork_den: u64,
    pub dvt_no_confidence_num: u64,
    pub dvt_no_confidence_den: u64,
    pub dvt_committee_normal_num: u64,
    pub dvt_committee_normal_den: u64,
    pub dvt_committee_no_confidence_num: u64,
    pub dvt_committee_no_confidence_den: u64,
    pub dvt_constitution_num: u64,
    pub dvt_constitution_den: u64,
    pub dvt_treasury_withdrawal_num: u64,
    pub dvt_treasury_withdrawal_den: u64,
    pub pvt_hard_fork_num: u64,
    pub pvt_hard_fork_den: u64,
    pub pvt_committee_num: u64,
    pub pvt_committee_den: u64,
}

impl Default for ProtocolParamsSnapshot {
    fn default() -> Self {
        ProtocolParamsSnapshot {
            min_fee_a: 44,
            min_fee_b: 155381,
            max_block_body_size: 90112,
            max_tx_size: 16384,
            max_block_header_size: 1100,
            key_deposit: 2_000_000,
            pool_deposit: 500_000_000,
            e_max: 18,
            n_opt: 500,
            a0_num: 3,
            a0_den: 10,
            rho_num: 3,
            rho_den: 1000,
            tau_num: 2,
            tau_den: 10,
            min_pool_cost: 170_000_000,
            ada_per_utxo_byte: 4310,
            cost_models_v1: None,
            cost_models_v2: None,
            cost_models_v3: None,
            execution_costs_mem_num: 577,
            execution_costs_mem_den: 10000,
            execution_costs_step_num: 721,
            execution_costs_step_den: 10000000,
            max_tx_ex_mem: 14_000_000,
            max_tx_ex_steps: 10_000_000_000,
            max_block_ex_mem: 62_000_000,
            max_block_ex_steps: 40_000_000_000,
            max_val_size: 5000,
            collateral_percentage: 150,
            max_collateral_inputs: 3,
            protocol_version_major: 9,
            protocol_version_minor: 0,
            min_fee_ref_script_cost_per_byte: 15,
            drep_deposit: 500_000_000,
            drep_activity: 20,
            gov_action_deposit: 100_000_000_000,
            gov_action_lifetime: 6,
            committee_min_size: 7,
            committee_max_term_length: 146,
            dvt_p_param_change_num: 67,
            dvt_p_param_change_den: 100,
            dvt_hard_fork_num: 60,
            dvt_hard_fork_den: 100,
            dvt_no_confidence_num: 67,
            dvt_no_confidence_den: 100,
            dvt_committee_normal_num: 67,
            dvt_committee_normal_den: 100,
            dvt_committee_no_confidence_num: 60,
            dvt_committee_no_confidence_den: 100,
            dvt_constitution_num: 75,
            dvt_constitution_den: 100,
            dvt_treasury_withdrawal_num: 67,
            dvt_treasury_withdrawal_den: 100,
            pvt_hard_fork_num: 51,
            pvt_hard_fork_den: 100,
            pvt_committee_num: 51,
            pvt_committee_den: 100,
        }
    }
}

/// Entry for NonMyopicMemberRewards query result
#[derive(Debug, Clone)]
pub struct NonMyopicRewardEntry {
    pub stake_amount: u64,
    /// Pool ID → estimated reward for this stake amount
    pub pool_rewards: Vec<(Vec<u8>, u64)>,
}

/// Snapshot of a stake pool for query results
#[derive(Debug, Clone)]
pub struct StakePoolSnapshot {
    pub pool_id: Vec<u8>,
    pub stake: u64,
    pub pledge: u64,
    pub cost: u64,
    pub margin_num: u64,
    pub margin_den: u64,
}

/// Snapshot of a DRep for query results
#[derive(Debug, Clone)]
pub struct DRepSnapshot {
    pub credential_hash: Vec<u8>,
    pub deposit: u64,
    pub anchor_url: Option<String>,
    pub registered_epoch: u64,
}

/// Snapshot of governance state
#[derive(Debug, Clone, Default)]
pub struct GovStateSnapshot {
    pub proposals: Vec<ProposalSnapshot>,
    pub drep_count: usize,
    pub committee_member_count: usize,
    pub treasury: u64,
}

/// Snapshot of a governance proposal
#[derive(Debug, Clone)]
pub struct ProposalSnapshot {
    pub tx_id: Vec<u8>,
    pub action_index: u32,
    pub action_type: String,
    pub proposed_epoch: u64,
    pub expires_epoch: u64,
    pub yes_votes: u64,
    pub no_votes: u64,
    pub abstain_votes: u64,
}

/// Snapshot of a stake address for query results
#[derive(Debug, Clone)]
pub struct StakeAddressSnapshot {
    pub credential_hash: Vec<u8>,
    pub delegated_pool: Option<Vec<u8>>,
    pub reward_balance: u64,
}

/// Snapshot of the constitutional committee
#[derive(Debug, Clone, Default)]
pub struct CommitteeSnapshot {
    pub members: Vec<CommitteeMemberSnapshot>,
    pub resigned: Vec<Vec<u8>>,
}

/// Snapshot of a committee member
#[derive(Debug, Clone)]
pub struct CommitteeMemberSnapshot {
    pub cold_credential: Vec<u8>,
    pub hot_credential: Vec<u8>,
}

/// Snapshot of the node state used for answering queries.
/// This is updated from the node when the ledger state changes.
#[derive(Debug, Clone)]
pub struct NodeStateSnapshot {
    pub tip: Tip,
    pub epoch: EpochNo,
    pub era: u32,
    pub block_number: BlockNo,
    pub system_start: String,
    pub utxo_count: usize,
    pub delegations_count: usize,
    pub pool_count: usize,
    pub treasury: u64,
    pub reserves: u64,
    pub drep_count: usize,
    pub proposal_count: usize,
    /// Protocol parameters for CBOR encoding
    pub protocol_params: ProtocolParamsSnapshot,
    /// Stake pool distribution data
    pub stake_pools: Vec<StakePoolSnapshot>,
    /// DRep registration data
    pub drep_entries: Vec<DRepSnapshot>,
    /// Governance proposals
    pub governance_proposals: Vec<ProposalSnapshot>,
    /// Committee members
    pub committee: CommitteeSnapshot,
    /// Stake address info (delegations + rewards)
    pub stake_addresses: Vec<StakeAddressSnapshot>,
    /// Stake snapshots (mark/set/go) for stake-snapshot queries
    pub stake_snapshots: StakeSnapshotsResult,
    /// Pool parameters for pool-params queries
    pub pool_params_entries: Vec<PoolParamsSnapshot>,
    /// Epoch length in slots (for era history query)
    pub epoch_length: u64,
    /// Slot length in seconds (for era history query)
    pub slot_length_secs: u64,
    /// Network magic number
    pub network_magic: u32,
}

impl Default for NodeStateSnapshot {
    fn default() -> Self {
        NodeStateSnapshot {
            tip: Tip::origin(),
            epoch: EpochNo(0),
            era: 6, // Conway
            block_number: BlockNo(0),
            system_start: "2017-09-23T21:44:51Z".to_string(), // Mainnet
            utxo_count: 0,
            delegations_count: 0,
            pool_count: 0,
            treasury: 0,
            reserves: 0,
            drep_count: 0,
            proposal_count: 0,
            protocol_params: ProtocolParamsSnapshot::default(),
            stake_pools: Vec::new(),
            drep_entries: Vec::new(),
            governance_proposals: Vec::new(),
            committee: CommitteeSnapshot::default(),
            stake_addresses: Vec::new(),
            stake_snapshots: StakeSnapshotsResult::default(),
            pool_params_entries: Vec::new(),
            epoch_length: 432000,     // Mainnet default
            slot_length_secs: 1,      // Shelley slot length
            network_magic: 764824073, // Mainnet magic
        }
    }
}

/// Multi-asset snapshot: Vec<(policy_id_bytes, Vec<(asset_name_bytes, quantity)>)>
pub type MultiAssetSnapshot = Vec<(Vec<u8>, Vec<(Vec<u8>, u64)>)>;

/// Snapshot of a UTxO entry for query results.
///
/// Encodes as Cardano wire format: Map<[tx_hash, index], output>
/// where output = {0: address_bytes, 1: value, 2: datum_option, 3: script_ref}
#[derive(Debug, Clone)]
pub struct UtxoSnapshot {
    pub tx_hash: Vec<u8>,
    pub output_index: u32,
    /// Raw address bytes for CBOR encoding
    pub address_bytes: Vec<u8>,
    pub lovelace: u64,
    pub multi_asset: MultiAssetSnapshot,
    /// Datum hash (32 bytes) if present
    pub datum_hash: Option<Vec<u8>>,
    /// Raw CBOR of the transaction output (for pass-through if available)
    pub raw_cbor: Option<Vec<u8>>,
}

/// Trait for providing UTxO query access to the query handler.
/// Implemented by the node to give the query handler on-demand access
/// to the UTxO set without coupling to the ledger crate.
pub trait UtxoQueryProvider: Send + Sync {
    /// Look up UTxOs at a specific address (raw bytes)
    fn utxos_at_address_bytes(&self, addr_bytes: &[u8]) -> Vec<UtxoSnapshot>;

    /// Look up UTxOs by transaction input references (tx_hash, output_index pairs)
    fn utxos_by_tx_inputs(&self, _inputs: &[(Vec<u8>, u32)]) -> Vec<UtxoSnapshot> {
        vec![] // Default: no results
    }
}

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
                        // Could be GetCurrentEra (hardcoded query) or era-wrapped query
                        if len == 1 {
                            debug!("Query: GetCurrentEra");
                            return QueryResult::CurrentEra(self.state.era);
                        }
                        // Era-wrapped query: [era, [query_tag, ...]]
                        self.dispatch_era_query(decoder)
                    }
                    2 => {
                        debug!("Query: GetSystemStart");
                        QueryResult::SystemStart(self.state.system_start.clone())
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
                    0 => QueryResult::CurrentEra(self.state.era),
                    2 => QueryResult::SystemStart(self.state.system_start.clone()),
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
            Ok(2) => QueryResult::SystemStart(self.state.system_start.clone()),
            _ => QueryResult::Error("Unknown simple query".into()),
        }
    }

    /// Dispatch an era-specific query
    fn dispatch_era_query(&self, decoder: &mut minicbor::Decoder<'_>) -> QueryResult {
        // Try to parse inner query: [query_tag, ...]
        match decoder.array() {
            Ok(_) => {
                let query_tag = decoder.u32().unwrap_or(999);
                self.handle_shelley_query(query_tag, decoder)
            }
            Err(_) => {
                // Try as a simple integer tag
                let query_tag = decoder.u32().unwrap_or(999);
                self.handle_shelley_query(query_tag, decoder)
            }
        }
    }

    /// Handle Shelley-era queries by tag
    fn handle_shelley_query(
        &self,
        query_tag: u32,
        decoder: &mut minicbor::Decoder<'_>,
    ) -> QueryResult {
        match query_tag {
            0 => {
                // GetLedgerTip / GetEpochNo
                debug!("Query: GetEpochNo");
                QueryResult::EpochNo(self.state.epoch.0)
            }
            1 => {
                // GetEpochNo (alternate)
                debug!("Query: GetEpochNo (alt)");
                QueryResult::EpochNo(self.state.epoch.0)
            }
            2 => {
                // GetUTxOByTxIn — look up specific UTxOs by transaction input references
                debug!("Query: GetUTxOByTxIn");
                let mut inputs = Vec::new();
                // Try to decode array of [tx_hash, output_index] pairs
                if let Ok(Some(n)) = decoder.array() {
                    for _ in 0..n {
                        if let Ok(Some(_)) = decoder.array() {
                            let tx_hash = decoder.bytes().unwrap_or(&[]).to_vec();
                            let idx = decoder.u32().unwrap_or(0);
                            inputs.push((tx_hash, idx));
                        }
                    }
                }
                if let Some(provider) = &self.utxo_provider {
                    QueryResult::UtxoByAddress(provider.utxos_by_tx_inputs(&inputs))
                } else {
                    QueryResult::UtxoByAddress(vec![])
                }
            }
            3 => {
                // GetAccountState (treasury + reserves)
                debug!("Query: GetAccountState");
                QueryResult::AccountState {
                    treasury: self.state.treasury,
                    reserves: self.state.reserves,
                }
            }
            4 => {
                // GetUTxOByAddress
                debug!("Query: GetUTxOByAddress");
                // Try to read address bytes from the query payload
                let addr_bytes = decoder.bytes().unwrap_or(&[]).to_vec();
                if let Some(provider) = &self.utxo_provider {
                    let utxos = provider.utxos_at_address_bytes(&addr_bytes);
                    QueryResult::UtxoByAddress(utxos)
                } else {
                    QueryResult::UtxoByAddress(vec![])
                }
            }
            5 => {
                // GetStakeDistribution
                debug!("Query: GetStakeDistribution");
                QueryResult::StakeDistribution(self.state.stake_pools.clone())
            }
            6 => {
                // GetNonMyopicMemberRewards — estimate rewards for given stake amounts
                debug!("Query: GetNonMyopicMemberRewards");

                // Parse requested stake amounts from the remaining query payload
                let mut amounts = Vec::new();
                if let Ok(Some(n)) = decoder.array() {
                    for _ in 0..n {
                        if let Ok(amt) = decoder.u64() {
                            amounts.push(amt);
                        } else {
                            decoder.skip().ok();
                        }
                    }
                }
                let stake_amounts = if amounts.is_empty() {
                    vec![1_000_000_000_000] // Default: 1M ADA in lovelace
                } else {
                    amounts
                };

                // Compute approximate rewards per pool for each stake amount
                // Uses simplified reward formula:
                //   reward ≈ (stake/total_stake) * rewards_pot * (1-margin) - cost_share
                let total_stake: u64 = self.state.stake_pools.iter().map(|p| p.stake).sum();
                let rewards_pot = self.state.reserves / 200; // ~0.5% of reserves per epoch

                let mut result = Vec::new();
                for amount in &stake_amounts {
                    let mut pool_rewards = Vec::new();
                    for pool in &self.state.stake_pools {
                        if pool.stake == 0 || total_stake == 0 {
                            continue;
                        }
                        let pool_reward =
                            (pool.stake as u128 * rewards_pot as u128 / total_stake as u128) as u64;
                        let after_cost = pool_reward.saturating_sub(pool.cost);
                        let margin = pool.margin_num as f64 / pool.margin_den.max(1) as f64;
                        let delegator_share = (after_cost as f64 * (1.0 - margin)) as u64;
                        let delegator_reward = (*amount as u128 * delegator_share as u128
                            / pool.stake.max(1) as u128)
                            as u64;
                        pool_rewards.push((pool.pool_id.clone(), delegator_reward));
                    }
                    result.push(NonMyopicRewardEntry {
                        stake_amount: *amount,
                        pool_rewards,
                    });
                }

                QueryResult::NonMyopicMemberRewards(result)
            }
            7 => {
                // GetCurrentPParams
                debug!("Query: GetCurrentPParams");
                QueryResult::ProtocolParams(Box::new(self.state.protocol_params.clone()))
            }
            8 => {
                // GetProposedPParamsUpdates — returns empty (no pending updates tracked)
                debug!("Query: GetProposedPParamsUpdates");
                QueryResult::StakeDistribution(vec![])
            }
            10 => {
                // GetChainBlockNo
                debug!("Query: GetChainBlockNo");
                QueryResult::ChainBlockNo(self.state.block_number.0)
            }
            11 => {
                // GetChainPoint (chain tip)
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
            9 => {
                // GetGenesisConfig
                debug!("Query: GetGenesisConfig");
                QueryResult::GenesisConfig {
                    system_start: self.state.system_start.clone(),
                    network_magic: self.state.network_magic,
                    epoch_length: self.state.epoch_length,
                    slot_length_secs: self.state.slot_length_secs,
                    security_param: 2160,
                }
            }
            13 => {
                // GetUTxOWhole — return empty set (too large to serve in practice)
                debug!("Query: GetUTxOWhole (returning empty — use GetUTxOByAddress instead)");
                QueryResult::UtxoByAddress(vec![])
            }
            20 => {
                // GetGovState (Conway governance)
                debug!("Query: GetGovState");
                QueryResult::GovState(GovStateSnapshot {
                    proposals: self.state.governance_proposals.clone(),
                    drep_count: self.state.drep_count,
                    committee_member_count: self.state.committee.members.len(),
                    treasury: self.state.treasury,
                })
            }
            21 => {
                // GetDRepState
                debug!("Query: GetDRepState");
                QueryResult::DRepState(self.state.drep_entries.clone())
            }
            22 => {
                // GetCommitteeState
                debug!("Query: GetCommitteeState");
                QueryResult::CommitteeState(self.state.committee.clone())
            }
            23 => {
                // GetStakeAddressInfo
                debug!("Query: GetStakeAddressInfo");
                QueryResult::StakeAddressInfo(self.state.stake_addresses.clone())
            }
            12 => {
                // GetStakePoolParams
                debug!("Query: GetStakePoolParams");
                QueryResult::PoolParams(self.state.pool_params_entries.clone())
            }
            24 => {
                // GetStakeSnapshots
                debug!("Query: GetStakeSnapshots");
                QueryResult::StakeSnapshots(self.state.stake_snapshots.clone())
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

#[cfg(test)]
mod tests {
    use super::*;
    use torsten_primitives::hash::Hash32;
    use torsten_primitives::time::SlotNo;

    /// Helper to call handle_shelley_query with an empty decoder
    fn query(handler: &QueryHandler, tag: u32) -> QueryResult {
        let empty = [0u8; 0];
        let mut decoder = minicbor::Decoder::new(&empty);
        handler.handle_shelley_query(tag, &mut decoder)
    }

    #[test]
    fn test_query_handler_default_state() {
        let handler = QueryHandler::new();
        match query(&handler, 0) {
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

        match query(&handler, 0) {
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

        match query(&handler, 11) {
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

        match query(&handler, 10) {
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
        // Build a MsgQuery CBOR: [3, [0, [0]]]
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(3).unwrap(); // MsgQuery
        enc.array(2).unwrap();
        enc.u32(0).unwrap(); // era tag
        enc.array(1).unwrap();
        enc.u32(0).unwrap(); // GetEpochNo

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
                    pledge: 500_000_000,
                    cost: 340_000_000,
                    margin_num: 1,
                    margin_den: 100,
                },
                StakePoolSnapshot {
                    pool_id: vec![0xbb; 28],
                    stake: 2_000_000_000,
                    pledge: 1_000_000_000,
                    cost: 340_000_000,
                    margin_num: 5,
                    margin_den: 100,
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

        match query(&handler, 7) {
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
                    cold_credential: vec![0x01; 32],
                    hot_credential: vec![0x02; 32],
                }],
                resigned: vec![],
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
            }],
            ..Default::default()
        });

        match query(&handler, 20) {
            QueryResult::GovState(gov) => {
                assert_eq!(gov.drep_count, 5);
                assert_eq!(gov.committee_member_count, 1);
                assert_eq!(gov.treasury, 1_000_000_000_000);
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
                credential_hash: vec![0xdd; 32],
                deposit: 500_000_000,
                anchor_url: Some("https://example.com/drep".to_string()),
                registered_epoch: 42,
            }],
            ..Default::default()
        });

        match query(&handler, 21) {
            QueryResult::DRepState(dreps) => {
                assert_eq!(dreps.len(), 1);
                assert_eq!(dreps[0].credential_hash, vec![0xdd; 32]);
                assert_eq!(dreps[0].deposit, 500_000_000);
                assert_eq!(
                    dreps[0].anchor_url,
                    Some("https://example.com/drep".to_string())
                );
                assert_eq!(dreps[0].registered_epoch, 42);
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
                        cold_credential: vec![0x01; 32],
                        hot_credential: vec![0x02; 32],
                    },
                    CommitteeMemberSnapshot {
                        cold_credential: vec![0x03; 32],
                        hot_credential: vec![0x04; 32],
                    },
                ],
                resigned: vec![vec![0x05; 32]],
            },
            ..Default::default()
        });

        match query(&handler, 22) {
            QueryResult::CommitteeState(committee) => {
                assert_eq!(committee.members.len(), 2);
                assert_eq!(committee.resigned.len(), 1);
                assert_eq!(committee.members[0].cold_credential, vec![0x01; 32]);
                assert_eq!(committee.members[0].hot_credential, vec![0x02; 32]);
                assert_eq!(committee.resigned[0], vec![0x05; 32]);
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

        match query(&handler, 23) {
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
        match handler.handle_shelley_query(4, &mut decoder) {
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
        match handler.handle_shelley_query(4, &mut decoder) {
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
        match query(&handler, 20) {
            QueryResult::GovState(gov) => {
                assert_eq!(gov.drep_count, 0);
                assert_eq!(gov.proposals.len(), 0);
                assert_eq!(gov.treasury, 0);
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

        match query(&handler, 24) {
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
                relays: vec!["relay1.example.com:3001".to_string()],
                metadata_url: None,
                metadata_hash: None,
            }],
            ..Default::default()
        });

        match query(&handler, 12) {
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
}
