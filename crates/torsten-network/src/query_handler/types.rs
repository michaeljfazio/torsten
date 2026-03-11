use torsten_primitives::block::Tip;
use torsten_primitives::time::{BlockNo, EpochNo};

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
    GovState(Box<GovStateSnapshot>),
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
    /// NonMyopicMemberRewards: map from stake_amount -> pool rewards
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
    /// Pool ID -> estimated reward for this stake amount
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
    /// decentralization parameter (d) -- 0 in Conway
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
    /// Enacted governance roots (prev_action_ids per purpose)
    /// Each is Option<(tx_hash_bytes, action_index)>
    pub enacted_pparam_update: Option<(Vec<u8>, u32)>,
    pub enacted_hard_fork: Option<(Vec<u8>, u32)>,
    pub enacted_committee: Option<(Vec<u8>, u32)>,
    pub enacted_constitution: Option<(Vec<u8>, u32)>,
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
///   [1] Maybe(UnitInterval) -- quorum threshold
///   [2] EpochNo -- current epoch
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
    /// Enacted governance action roots (for GovState query)
    pub enacted_pparam_update: Option<(Vec<u8>, u32)>,
    pub enacted_hard_fork: Option<(Vec<u8>, u32)>,
    pub enacted_committee: Option<(Vec<u8>, u32)>,
    pub enacted_constitution: Option<(Vec<u8>, u32)>,
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
    /// Active slots coefficient as numerator/denominator
    pub active_slots_coeff_num: u64,
    pub active_slots_coeff_den: u64,
    /// Slots per KES period (genesis value)
    pub slots_per_kes_period: u64,
    /// Maximum KES evolutions (genesis value)
    pub max_kes_evolutions: u64,
    /// Update quorum (genesis value)
    pub update_quorum: u64,
    /// Maximum lovelace supply (genesis value)
    pub max_lovelace_supply: u64,
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
            enacted_pparam_update: None,
            enacted_hard_fork: None,
            enacted_committee: None,
            enacted_constitution: None,
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
            active_slots_coeff_num: 1,
            active_slots_coeff_den: 20,
            slots_per_kes_period: 129600,
            max_kes_evolutions: 62,
            update_quorum: 5,
            max_lovelace_supply: 45_000_000_000_000_000,
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
