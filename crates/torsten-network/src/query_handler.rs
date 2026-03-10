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
    /// Set of pool key hashes (GetStakePools tag 16)
    StakePools(Vec<Vec<u8>>),
    PoolParams(Vec<PoolParamsSnapshot>),
    AccountState {
        treasury: u64,
        reserves: u64,
    },
    GenesisConfig(Box<GenesisConfigSnapshot>),
    /// NonMyopicMemberRewards: map from stake_amount → pool rewards
    NonMyopicMemberRewards(Vec<NonMyopicRewardEntry>),
    /// Empty proposed protocol parameter updates (Conway uses governance proposals)
    ProposedPParamsUpdates,
    /// Constitution: anchor + optional guardrail script hash
    Constitution {
        url: String,
        data_hash: Vec<u8>,
        script_hash: Option<Vec<u8>>,
    },
    /// Pool distribution: Map<pool_hash, IndividualPoolStake> (tag 21)
    PoolDistr(Vec<StakePoolSnapshot>),
    /// Stake delegation deposits: Map<Credential, Coin> (tag 22)
    StakeDelegDeposits(Vec<StakeDelegDepositEntry>),
    /// DRep stake distribution: Map<DRep, Coin> (tag 26)
    DRepStakeDistr(Vec<DRepStakeEntry>),
    /// Filtered vote delegatees: Map<Credential, DRep> (tag 28)
    FilteredVoteDelegatees(Vec<VoteDelegateeEntry>),
    /// Era history: list of era summaries for slot/time conversions
    EraHistory(Vec<EraSummary>),
    Error(String),
}

/// Summary of a single Cardano era for GetEraHistory responses
#[derive(Debug, Clone)]
pub struct EraSummary {
    /// Start slot number
    pub start_slot: u64,
    /// Start epoch number
    pub start_epoch: u64,
    /// Start time in picoseconds relative to system start
    pub start_time_pico: u64,
    /// End bound (None = current/unbounded era)
    pub end: Option<EraBound>,
    /// Epoch length in slots
    pub epoch_size: u64,
    /// Slot length in milliseconds
    pub slot_length_ms: u64,
    /// Safe zone (number of slots past era end where predictions are still valid)
    pub safe_zone: u64,
}

/// Era boundary
#[derive(Debug, Clone)]
pub struct EraBound {
    pub slot: u64,
    pub epoch: u64,
    pub time_pico: u64,
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

/// Pool relay snapshot for CBOR encoding
#[derive(Debug, Clone)]
pub enum RelaySnapshot {
    SingleHostAddr {
        port: Option<u16>,
        ipv4: Option<[u8; 4]>,
        ipv6: Option<[u8; 16]>,
    },
    SingleHostName {
        port: Option<u16>,
        dns_name: String,
    },
    MultiHostName {
        dns_name: String,
    },
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
    pub relays: Vec<RelaySnapshot>,
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
    pub dvt_pp_network_group_num: u64,
    pub dvt_pp_network_group_den: u64,
    pub dvt_pp_economic_group_num: u64,
    pub dvt_pp_economic_group_den: u64,
    pub dvt_pp_technical_group_num: u64,
    pub dvt_pp_technical_group_den: u64,
    pub dvt_pp_gov_group_num: u64,
    pub dvt_pp_gov_group_den: u64,
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
    pub pvt_motion_no_confidence_num: u64,
    pub pvt_motion_no_confidence_den: u64,
    pub pvt_committee_normal_num: u64,
    pub pvt_committee_normal_den: u64,
    pub pvt_committee_no_confidence_num: u64,
    pub pvt_committee_no_confidence_den: u64,
    pub pvt_hard_fork_num: u64,
    pub pvt_hard_fork_den: u64,
    pub pvt_pp_security_group_num: u64,
    pub pvt_pp_security_group_den: u64,
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
            dvt_pp_network_group_num: 67,
            dvt_pp_network_group_den: 100,
            dvt_pp_economic_group_num: 67,
            dvt_pp_economic_group_den: 100,
            dvt_pp_technical_group_num: 67,
            dvt_pp_technical_group_den: 100,
            dvt_pp_gov_group_num: 67,
            dvt_pp_gov_group_den: 100,
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
            pvt_motion_no_confidence_num: 51,
            pvt_motion_no_confidence_den: 100,
            pvt_committee_normal_num: 51,
            pvt_committee_normal_den: 100,
            pvt_committee_no_confidence_num: 51,
            pvt_committee_no_confidence_den: 100,
            pvt_hard_fork_num: 51,
            pvt_hard_fork_den: 100,
            pvt_pp_security_group_num: 51,
            pvt_pp_security_group_den: 100,
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

/// Stake delegation deposit entry for GetStakeDelegDeposits (tag 22)
#[derive(Debug, Clone)]
pub struct StakeDelegDepositEntry {
    /// Stake credential hash (28 bytes)
    pub credential_hash: Vec<u8>,
    /// Credential type: 0=KeyHash, 1=ScriptHash
    pub credential_type: u8,
    /// Deposit amount in lovelace
    pub deposit: u64,
}

/// DRep stake distribution entry for GetDRepStakeDistr (tag 26)
#[derive(Debug, Clone)]
pub struct DRepStakeEntry {
    /// DRep type: 0=KeyHash, 1=ScriptHash, 2=AlwaysAbstain, 3=AlwaysNoConfidence
    pub drep_type: u8,
    /// DRep credential hash (28 bytes, None for AlwaysAbstain/AlwaysNoConfidence)
    pub drep_hash: Option<Vec<u8>>,
    /// Total delegated stake in lovelace
    pub stake: u64,
}

/// Vote delegatee entry for GetFilteredVoteDelegatees (tag 28)
#[derive(Debug, Clone)]
pub struct VoteDelegateeEntry {
    /// Stake credential hash (28 bytes)
    pub credential_hash: Vec<u8>,
    /// Credential type: 0=KeyHash, 1=ScriptHash
    pub credential_type: u8,
    /// DRep type: 0=KeyHash, 1=ScriptHash, 2=AlwaysAbstain, 3=AlwaysNoConfidence
    pub drep_type: u8,
    /// DRep credential hash (28 bytes, None for AlwaysAbstain/AlwaysNoConfidence)
    pub drep_hash: Option<Vec<u8>>,
}

/// CompactGenesis snapshot for GetGenesisConfig (tag 11).
/// CBOR: array(15) matching ShelleyGenesis wire format.
#[derive(Debug, Clone)]
pub struct GenesisConfigSnapshot {
    /// ISO-8601 UTC timestamp, e.g. "2022-04-01T00:00:00Z"
    pub system_start: String,
    pub network_magic: u32,
    /// 0=Testnet, 1=Mainnet
    pub network_id: u8,
    /// activeSlotsCoeff as numerator/denominator (no tag(30))
    pub active_slots_coeff_num: u64,
    pub active_slots_coeff_den: u64,
    pub security_param: u64,
    pub epoch_length: u64,
    pub slots_per_kes_period: u64,
    pub max_kes_evolutions: u64,
    /// Slot length in microseconds (1 second = 1_000_000)
    pub slot_length_micros: u64,
    pub update_quorum: u64,
    pub max_lovelace_supply: u64,
    /// Legacy Shelley protocol params
    pub protocol_params: ShelleyPParamsSnapshot,
    /// Genesis delegates: Vec<(genesis_keyhash_28, delegate_keyhash_28, vrf_hash_32)>
    pub gen_delegs: Vec<(Vec<u8>, Vec<u8>, Vec<u8>)>,
}

/// Legacy Shelley-era protocol parameters for CompactGenesis.
/// CBOR: array(18) with split protocolVersion major/minor (legacy encoding).
#[derive(Debug, Clone)]
pub struct ShelleyPParamsSnapshot {
    pub min_fee_a: u64,
    pub min_fee_b: u64,
    pub max_block_body_size: u32,
    pub max_tx_size: u32,
    pub max_block_header_size: u16,
    pub key_deposit: u64,
    pub pool_deposit: u64,
    pub e_max: u32,
    pub n_opt: u16,
    /// a0 as tagged rational tag(30)[num, den]
    pub a0_num: u64,
    pub a0_den: u64,
    /// rho as tagged rational
    pub rho_num: u64,
    pub rho_den: u64,
    /// tau as tagged rational
    pub tau_num: u64,
    pub tau_den: u64,
    /// decentralization parameter (d) — 0 in Conway
    pub d_num: u64,
    pub d_den: u64,
    /// protocol version major
    pub protocol_version_major: u64,
    /// protocol version minor
    pub protocol_version_minor: u64,
    pub min_utxo_value: u64,
    pub min_pool_cost: u64,
}

/// Snapshot of a stake pool for query results (GetStakeDistribution).
///
/// Wire format: Map<pool_hash(28), [tag(30)[num,den], vrf_hash(32)]>
#[derive(Debug, Clone)]
pub struct StakePoolSnapshot {
    pub pool_id: Vec<u8>,
    pub stake: u64,
    pub vrf_keyhash: Vec<u8>,
    /// Total active stake across all pools (for computing stake fraction)
    pub total_active_stake: u64,
}

/// Snapshot of a DRep for query results.
///
/// Wire format: Map<Credential, DRepState>
///   Credential: [0=KeyHash|1=ScriptHash, hash(28)]
///   DRepState: array(4) [expiry, maybe_anchor, deposit, set_delegators]
#[derive(Debug, Clone)]
pub struct DRepSnapshot {
    pub credential_hash: Vec<u8>,
    /// 0 = KeyHashObj, 1 = ScriptHashObj
    pub credential_type: u8,
    pub deposit: u64,
    pub anchor_url: Option<String>,
    pub anchor_hash: Option<Vec<u8>>,
    /// Epoch when this DRep expires (drepExpiry)
    pub expiry_epoch: u64,
    /// Delegator credential hashes
    pub delegator_hashes: Vec<Vec<u8>>,
}

/// Snapshot of Conway governance state for array(7) CBOR encoding.
///
/// ConwayGovState = array(7):
///   [0] Proposals (roots + ordered proposals)
///   [1] Committee (StrictMaybe)
///   [2] Constitution (anchor + optional script hash)
///   [3] curPParams (array(31))
///   [4] prevPParams (array(31))
///   [5] FuturePParams (sum type)
///   [6] DRepPulsingState (DRComplete)
#[derive(Debug, Clone, Default)]
pub struct GovStateSnapshot {
    pub proposals: Vec<ProposalSnapshot>,
    /// Committee data (reused from CommitteeSnapshot)
    pub committee: CommitteeSnapshot,
    /// Constitution anchor URL
    pub constitution_url: String,
    /// Constitution anchor data hash (32 bytes)
    pub constitution_hash: Vec<u8>,
    /// Constitution guardrail script hash (28 bytes), if any
    pub constitution_script: Option<Vec<u8>>,
    /// Current protocol parameters
    pub cur_pparams: Box<ProtocolParamsSnapshot>,
    /// Previous protocol parameters (defaults to current if not tracked)
    pub prev_pparams: Box<ProtocolParamsSnapshot>,
}

/// Snapshot of a governance proposal (GovActionState)
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
    /// Deposit amount in lovelace
    pub deposit: u64,
    /// Return address (raw bytes)
    pub return_addr: Vec<u8>,
    /// Anchor URL for the proposal
    pub anchor_url: String,
    /// Anchor data hash (32 bytes)
    pub anchor_hash: Vec<u8>,
}

/// Snapshot of a stake address for query results
#[derive(Debug, Clone)]
pub struct StakeAddressSnapshot {
    pub credential_hash: Vec<u8>,
    pub delegated_pool: Option<Vec<u8>>,
    pub reward_balance: u64,
}

/// Snapshot of the constitutional committee.
///
/// Wire format: array(3)
///   [0] Map<ColdCredential, CommitteeMemberState>
///   [1] Maybe(UnitInterval) — quorum threshold
///   [2] EpochNo — current epoch
#[derive(Debug, Clone, Default)]
pub struct CommitteeSnapshot {
    pub members: Vec<CommitteeMemberSnapshot>,
    /// Quorum threshold (numerator, denominator)
    pub threshold: Option<(u64, u64)>,
    /// Current epoch
    pub current_epoch: u64,
}

/// Snapshot of a committee member
#[derive(Debug, Clone)]
pub struct CommitteeMemberSnapshot {
    pub cold_credential: Vec<u8>,
    pub cold_credential_type: u8,
    /// Hot credential authorization status: 0=Authorized(cred), 1=NotAuthorized, 2=Resigned
    pub hot_status: u8,
    pub hot_credential: Option<Vec<u8>>,
    /// 0=Active, 1=Expired, 2=Unrecognized
    pub member_status: u8,
    /// Term expiration epoch
    pub expiry_epoch: Option<u64>,
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
    /// Constitution anchor URL
    pub constitution_url: String,
    /// Constitution anchor data hash
    pub constitution_hash: Vec<u8>,
    /// Constitution guardrail script hash (if any)
    pub constitution_script: Option<Vec<u8>>,
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
    /// Security parameter (k)
    pub security_param: u64,
    /// Full genesis config for GetGenesisConfig query (tag 11)
    pub genesis_config: Option<GenesisConfigSnapshot>,
    /// Stake delegation deposits for GetStakeDelegDeposits (tag 22)
    pub stake_deleg_deposits: Vec<StakeDelegDepositEntry>,
    /// DRep stake distribution for GetDRepStakeDistr (tag 26)
    pub drep_stake_distr: Vec<DRepStakeEntry>,
    /// Vote delegatees for GetFilteredVoteDelegatees (tag 28)
    pub vote_delegatees: Vec<VoteDelegateeEntry>,
    /// Era summaries for GetEraHistory query
    pub era_summaries: Vec<EraSummary>,
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
            constitution_url: String::new(),
            constitution_hash: vec![0u8; 32],
            constitution_script: None,
            stake_addresses: Vec::new(),
            stake_snapshots: StakeSnapshotsResult::default(),
            pool_params_entries: Vec::new(),
            epoch_length: 432000,     // Mainnet default
            slot_length_secs: 1,      // Shelley slot length
            network_magic: 764824073, // Mainnet magic
            security_param: 2160,     // Mainnet security parameter
            genesis_config: None,
            stake_deleg_deposits: Vec::new(),
            drep_stake_distr: Vec::new(),
            vote_delegatees: Vec::new(),
            era_summaries: Vec::new(),
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

    /// Parse a set of credential hashes from CBOR.
    /// Handles: tag(258) [credential, ...] where credential = [0|1, hash(28)]
    /// Also handles plain array of bytes (legacy/simplified format).
    fn parse_credential_set_static(decoder: &mut minicbor::Decoder<'_>) -> Vec<Vec<u8>> {
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
    fn handle_shelley_query(
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
            2 => {
                // Tag 2: GetNonMyopicMemberRewards
                debug!("Query: GetNonMyopicMemberRewards");
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
                    vec![1_000_000_000_000]
                } else {
                    amounts
                };
                let total_stake: u64 = self.state.stake_pools.iter().map(|p| p.stake).sum();
                let rewards_pot = self.state.reserves / 200;
                // Build a cost/margin lookup from pool params
                let pool_params_map: std::collections::HashMap<&[u8], &PoolParamsSnapshot> = self
                    .state
                    .pool_params_entries
                    .iter()
                    .map(|pp| (pp.pool_id.as_slice(), pp))
                    .collect();
                let mut result = Vec::new();
                for amount in &stake_amounts {
                    let mut pool_rewards = Vec::new();
                    for pool in &self.state.stake_pools {
                        if pool.stake == 0 || total_stake == 0 {
                            continue;
                        }
                        let pool_reward =
                            (pool.stake as u128 * rewards_pot as u128 / total_stake as u128) as u64;
                        // Look up cost/margin from pool params
                        let (cost, margin) =
                            if let Some(pp) = pool_params_map.get(pool.pool_id.as_slice()) {
                                let m = pp.margin_num as f64 / pp.margin_den.max(1) as f64;
                                (pp.cost, m)
                            } else {
                                (340_000_000, 0.0) // defaults
                            };
                        let after_cost = pool_reward.saturating_sub(cost);
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
            3 => {
                // Tag 3: GetCurrentPParams
                debug!("Query: GetCurrentPParams");
                QueryResult::ProtocolParams(Box::new(self.state.protocol_params.clone()))
            }
            4 => {
                // Tag 4: GetProposedPParamsUpdates (deprecated in Conway)
                debug!("Query: GetProposedPParamsUpdates");
                QueryResult::ProposedPParamsUpdates
            }
            5 => {
                // Tag 5: GetStakeDistribution
                debug!("Query: GetStakeDistribution");
                QueryResult::StakeDistribution(self.state.stake_pools.clone())
            }
            6 => {
                // Tag 6: GetUTxOByAddress
                // Argument: tag(258) Set<Address> or single address bytes
                debug!("Query: GetUTxOByAddress");
                let mut addresses: Vec<Vec<u8>> = Vec::new();
                let pos = decoder.position();
                // Try single bare address bytes first (most common case)
                if let Ok(bytes) = decoder.bytes() {
                    addresses.push(bytes.to_vec());
                } else {
                    // Try tag(258) Set<Address>
                    decoder.set_position(pos);
                    let _ = decoder.tag(); // consume tag(258)
                    if let Ok(Some(n)) = decoder.array() {
                        for _ in 0..n {
                            if let Ok(bytes) = decoder.bytes() {
                                addresses.push(bytes.to_vec());
                            }
                        }
                    }
                }
                // Fallback: use remaining decoder bytes as raw address
                if addresses.is_empty() {
                    decoder.set_position(pos);
                    let remaining = &decoder.input()[pos..];
                    if !remaining.is_empty() {
                        addresses.push(remaining.to_vec());
                    }
                }
                if let Some(provider) = &self.utxo_provider {
                    let mut all_utxos = Vec::new();
                    for addr in &addresses {
                        all_utxos.extend(provider.utxos_at_address_bytes(addr));
                    }
                    QueryResult::UtxoByAddress(all_utxos)
                } else {
                    QueryResult::UtxoByAddress(vec![])
                }
            }
            7 => {
                // Tag 7: GetUTxOWhole (too large to serve in practice)
                debug!("Query: GetUTxOWhole (returning empty)");
                QueryResult::UtxoByAddress(vec![])
            }
            // Tag 8: DebugEpochState — not implemented
            // Tag 9: GetCBOR — not implemented
            10 => {
                // Tag 10: GetFilteredDelegationsAndRewardAccounts
                // Argument: tag(258) Set<Credential> where Credential = [0|1, hash(28)]
                debug!("Query: GetFilteredDelegationsAndRewardAccounts");
                let filter_hashes = Self::parse_credential_set_static(decoder);
                if filter_hashes.is_empty() {
                    QueryResult::StakeAddressInfo(self.state.stake_addresses.clone())
                } else {
                    let filtered = self
                        .state
                        .stake_addresses
                        .iter()
                        .filter(|s| filter_hashes.iter().any(|h| h == &s.credential_hash))
                        .cloned()
                        .collect();
                    QueryResult::StakeAddressInfo(filtered)
                }
            }
            11 => {
                // Tag 11: GetGenesisConfig (CompactGenesis)
                debug!("Query: GetGenesisConfig");
                if let Some(ref gc) = self.state.genesis_config {
                    QueryResult::GenesisConfig(Box::new(gc.clone()))
                } else {
                    // Fallback: minimal genesis config from node state fields
                    QueryResult::GenesisConfig(Box::new(GenesisConfigSnapshot {
                        system_start: self.state.system_start.clone(),
                        network_magic: self.state.network_magic,
                        network_id: if self.state.network_magic == 764824073 {
                            1
                        } else {
                            0
                        },
                        active_slots_coeff_num: 1,
                        active_slots_coeff_den: 20,
                        security_param: self.state.security_param,
                        epoch_length: self.state.epoch_length,
                        slots_per_kes_period: 129600,
                        max_kes_evolutions: 62,
                        slot_length_micros: self.state.slot_length_secs * 1_000_000,
                        update_quorum: 5,
                        max_lovelace_supply: 45_000_000_000_000_000,
                        protocol_params: ShelleyPParamsSnapshot {
                            min_fee_a: self.state.protocol_params.min_fee_a,
                            min_fee_b: self.state.protocol_params.min_fee_b,
                            max_block_body_size: self.state.protocol_params.max_block_body_size
                                as u32,
                            max_tx_size: self.state.protocol_params.max_tx_size as u32,
                            max_block_header_size: self.state.protocol_params.max_block_header_size
                                as u16,
                            key_deposit: self.state.protocol_params.key_deposit,
                            pool_deposit: self.state.protocol_params.pool_deposit,
                            e_max: self.state.protocol_params.e_max as u32,
                            n_opt: self.state.protocol_params.n_opt as u16,
                            a0_num: self.state.protocol_params.a0_num,
                            a0_den: self.state.protocol_params.a0_den,
                            rho_num: self.state.protocol_params.rho_num,
                            rho_den: self.state.protocol_params.rho_den,
                            tau_num: self.state.protocol_params.tau_num,
                            tau_den: self.state.protocol_params.tau_den,
                            d_num: 0,
                            d_den: 1,
                            protocol_version_major: self
                                .state
                                .protocol_params
                                .protocol_version_major,
                            protocol_version_minor: self
                                .state
                                .protocol_params
                                .protocol_version_minor,
                            min_utxo_value: 0,
                            min_pool_cost: self.state.protocol_params.min_pool_cost,
                        },
                        gen_delegs: Vec::new(),
                    }))
                }
            }
            // Tag 12: DebugNewEpochState — not implemented
            // Tag 13: DebugChainDepState — not implemented
            // Tag 14: GetRewardProvenance — not implemented
            15 => {
                // Tag 15: GetUTxOByTxIn
                debug!("Query: GetUTxOByTxIn");
                let mut inputs = Vec::new();
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
            16 => {
                // Tag 16: GetStakePools — returns Set<KeyHash StakePool>
                debug!("Query: GetStakePools");
                let pool_ids: Vec<Vec<u8>> = self
                    .state
                    .stake_pools
                    .iter()
                    .map(|p| p.pool_id.clone())
                    .collect();
                QueryResult::StakePools(pool_ids)
            }
            17 => {
                // Tag 17: GetStakePoolParams
                // Argument: tag(258) Set<KeyHash StakePool>
                debug!("Query: GetStakePoolParams");
                let mut filter_pools: Vec<Vec<u8>> = Vec::new();
                // Try to consume tag(258) if present
                let _ = decoder.tag();
                if let Ok(Some(n)) = decoder.array() {
                    for _ in 0..n {
                        if let Ok(bytes) = decoder.bytes() {
                            filter_pools.push(bytes.to_vec());
                        }
                    }
                }
                if filter_pools.is_empty() {
                    QueryResult::PoolParams(self.state.pool_params_entries.clone())
                } else {
                    let filtered = self
                        .state
                        .pool_params_entries
                        .iter()
                        .filter(|p| filter_pools.iter().any(|h| h == &p.pool_id))
                        .cloned()
                        .collect();
                    QueryResult::PoolParams(filtered)
                }
            }
            // Tag 18: GetRewardInfoPools — not implemented (complex reward provenance data)
            19 => {
                // Tag 19: GetPoolState — returns pool params (same data as tag 17)
                // Argument: tag(258) Set<KeyHash StakePool>
                debug!("Query: GetPoolState");
                let mut filter_pools: Vec<Vec<u8>> = Vec::new();
                let _ = decoder.tag();
                if let Ok(Some(n)) = decoder.array() {
                    for _ in 0..n {
                        if let Ok(bytes) = decoder.bytes() {
                            filter_pools.push(bytes.to_vec());
                        }
                    }
                }
                if filter_pools.is_empty() {
                    QueryResult::PoolParams(self.state.pool_params_entries.clone())
                } else {
                    let filtered = self
                        .state
                        .pool_params_entries
                        .iter()
                        .filter(|p| filter_pools.iter().any(|h| h == &p.pool_id))
                        .cloned()
                        .collect();
                    QueryResult::PoolParams(filtered)
                }
            }
            20 => {
                // Tag 20: GetStakeSnapshots
                debug!("Query: GetStakeSnapshots");
                QueryResult::StakeSnapshots(self.state.stake_snapshots.clone())
            }
            21 => {
                // Tag 21: GetPoolDistr — returns pool stake distribution
                // Argument: tag(258) Set<KeyHash StakePool> (optional filter)
                debug!("Query: GetPoolDistr");
                let mut filter_pools: Vec<Vec<u8>> = Vec::new();
                let _ = decoder.tag();
                if let Ok(Some(n)) = decoder.array() {
                    for _ in 0..n {
                        if let Ok(bytes) = decoder.bytes() {
                            filter_pools.push(bytes.to_vec());
                        }
                    }
                }
                if filter_pools.is_empty() {
                    QueryResult::PoolDistr(self.state.stake_pools.clone())
                } else {
                    let filtered = self
                        .state
                        .stake_pools
                        .iter()
                        .filter(|p| filter_pools.iter().any(|h| h == &p.pool_id))
                        .cloned()
                        .collect();
                    QueryResult::PoolDistr(filtered)
                }
            }
            22 => {
                // Tag 22: GetStakeDelegDeposits
                // Argument: tag(258) Set<Credential>
                // Returns: Map<Credential, Coin> — deposit amount per registered stake credential
                debug!("Query: GetStakeDelegDeposits");
                let filter_hashes = Self::parse_credential_set_static(decoder);
                if filter_hashes.is_empty() {
                    QueryResult::StakeDelegDeposits(self.state.stake_deleg_deposits.clone())
                } else {
                    let filtered = self
                        .state
                        .stake_deleg_deposits
                        .iter()
                        .filter(|d| filter_hashes.iter().any(|h| h == &d.credential_hash))
                        .cloned()
                        .collect();
                    QueryResult::StakeDelegDeposits(filtered)
                }
            }
            23 => {
                // Tag 23: GetConstitution
                debug!("Query: GetConstitution");
                QueryResult::Constitution {
                    url: self.state.constitution_url.clone(),
                    data_hash: self.state.constitution_hash.clone(),
                    script_hash: self.state.constitution_script.clone(),
                }
            }
            24 => {
                // Tag 24: GetGovState
                debug!("Query: GetGovState");
                QueryResult::GovState(GovStateSnapshot {
                    proposals: self.state.governance_proposals.clone(),
                    committee: self.state.committee.clone(),
                    constitution_url: self.state.constitution_url.clone(),
                    constitution_hash: self.state.constitution_hash.clone(),
                    constitution_script: self.state.constitution_script.clone(),
                    cur_pparams: Box::new(self.state.protocol_params.clone()),
                    prev_pparams: Box::new(self.state.protocol_params.clone()),
                })
            }
            25 => {
                // Tag 25: GetDRepState
                // Argument: tag(258) Set<Credential> where Credential = [0|1, hash(28)]
                debug!("Query: GetDRepState");
                let filter_hashes = Self::parse_credential_set_static(decoder);
                if filter_hashes.is_empty() {
                    QueryResult::DRepState(self.state.drep_entries.clone())
                } else {
                    let filtered = self
                        .state
                        .drep_entries
                        .iter()
                        .filter(|d| filter_hashes.iter().any(|h| h == &d.credential_hash))
                        .cloned()
                        .collect();
                    QueryResult::DRepState(filtered)
                }
            }
            26 => {
                // Tag 26: GetDRepStakeDistr
                // Argument: tag(258) Set<DRep>
                // Returns: Map<DRep, Coin> — total delegated stake per DRep
                debug!("Query: GetDRepStakeDistr");
                // Return all DRep stake distribution (filtering by DRep is complex, return all)
                QueryResult::DRepStakeDistr(self.state.drep_stake_distr.clone())
            }
            27 => {
                // Tag 27: GetCommitteeMembersState
                debug!("Query: GetCommitteeMembersState");
                QueryResult::CommitteeState(self.state.committee.clone())
            }
            28 => {
                // Tag 28: GetFilteredVoteDelegatees
                // Argument: tag(258) Set<Credential>
                // Returns: Map<Credential, DRep> — vote delegation for filtered credentials
                debug!("Query: GetFilteredVoteDelegatees");
                let filter_hashes = Self::parse_credential_set_static(decoder);
                if filter_hashes.is_empty() {
                    QueryResult::FilteredVoteDelegatees(self.state.vote_delegatees.clone())
                } else {
                    let filtered = self
                        .state
                        .vote_delegatees
                        .iter()
                        .filter(|v| filter_hashes.iter().any(|h| h == &v.credential_hash))
                        .cloned()
                        .collect();
                    QueryResult::FilteredVoteDelegatees(filtered)
                }
            }
            29 => {
                // Tag 29: GetAccountState (treasury + reserves)
                debug!("Query: GetAccountState");
                QueryResult::AccountState {
                    treasury: self.state.treasury,
                    reserves: self.state.reserves,
                }
            }
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

        // ChainBlockNo is outer tag 2 — build a MsgQuery CBOR: [3, [2]]
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
