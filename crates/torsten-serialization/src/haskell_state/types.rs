//! Intermediate types mirroring Haskell's NewEpochState CBOR structure.
//! These are 1:1 with the Haskell encoding and get converted to Torsten types.

use std::collections::HashMap;
use torsten_primitives::hash::{Hash28, Hash32};
use torsten_primitives::time::EpochNo;
use torsten_primitives::transaction::{TransactionInput, TransactionOutput};
use torsten_primitives::value::Lovelace;

/// Parsed NewEpochState (top-level Haskell ledger state)
#[derive(Debug)]
pub struct HaskellNewEpochState {
    pub epoch_no: EpochNo,
    pub blocks_made_prev: HashMap<Hash28, u64>,
    pub blocks_made_cur: HashMap<Hash28, u64>,
    pub epoch_state: HaskellEpochState,
    pub reward_update: Option<HaskellRewardUpdate>,
    pub pool_distr: HaskellPoolDistr,
}

/// EpochState: array(4)
#[derive(Debug)]
pub struct HaskellEpochState {
    pub treasury: Lovelace,
    pub reserves: Lovelace,
    pub ledger_state: HaskellLedgerState,
    pub snapshots: HaskellSnapShots,
    // non_myopic is parsed but discarded
}

/// LedgerState: array(2) -- CertState encoded FIRST
#[derive(Debug)]
pub struct HaskellLedgerState {
    pub cert_state: HaskellCertState,
    pub utxo_state: HaskellUTxOState,
}

/// UTxOState: array(6)
#[derive(Debug)]
pub struct HaskellUTxOState {
    pub utxo: Vec<(TransactionInput, TransactionOutput)>,
    /// Number of UTxO entries in the original state (includes skipped entries).
    /// When MemPack TxOut parsing is incomplete, `utxo.len()` may be less than this.
    pub utxo_count: u64,
    pub deposited: Lovelace,
    pub fees: Lovelace,
    pub gov_state: HaskellConwayGovState,
    pub donation: Lovelace,
}

/// CertState: array(3) -- VState encoded FIRST in Conway
#[derive(Debug)]
pub struct HaskellCertState {
    pub vstate: HaskellVState,
    pub pstate: HaskellPState,
    pub dstate: HaskellDState,
}

/// VState: array(3)
#[derive(Debug)]
pub struct HaskellVState {
    pub dreps: HashMap<HaskellCredential, HaskellDRepState>,
    pub committee_state: HashMap<HaskellCredential, HaskellCommitteeAuth>,
    pub num_dormant_epochs: EpochNo,
}

/// DRepState: array(4)
#[derive(Debug)]
pub struct HaskellDRepState {
    pub expiry: EpochNo,
    pub anchor: Option<HaskellAnchor>,
    pub deposit: Lovelace,
    pub delegators: Vec<HaskellCredential>,
}

/// Committee authorization status
#[derive(Debug)]
pub enum HaskellCommitteeAuth {
    HotCredential(HaskellCredential),
    Resigned(Option<HaskellAnchor>),
}

/// PState: array(4)
#[derive(Debug)]
pub struct HaskellPState {
    pub stake_pool_params: HashMap<Hash28, HaskellPoolParams>,
    pub future_pool_params: HashMap<Hash28, HaskellPoolParams>,
    pub retiring: HashMap<Hash28, EpochNo>,
    pub deposits: HashMap<Hash28, Lovelace>,
}

/// DState: array(4)
#[derive(Debug)]
pub struct HaskellDState {
    /// Reward accounts: stake credential -> (rewards, deposit, pool delegation, drep delegation)
    pub accounts: HashMap<HaskellCredential, HaskellAccountState>,
    // future_gen_delegs and gen_delegs are mostly empty in Conway, parse minimally
    // instantaneous_rewards: mostly empty in Conway
}

/// Per-credential account state from DState
#[derive(Debug)]
pub struct HaskellAccountState {
    pub rewards: Lovelace,
    pub deposit: Lovelace,
    pub pool_delegation: Option<Hash28>,
    pub drep_delegation: Option<HaskellDRep>,
}

/// SnapShots: array(4)
#[derive(Debug)]
pub struct HaskellSnapShots {
    pub mark: HaskellSnapShot,
    pub set: HaskellSnapShot,
    pub go: HaskellSnapShot,
    pub fee: Lovelace,
}

/// Individual SnapShot -- handles both old array(3) and new array(2) format
#[derive(Debug)]
pub struct HaskellSnapShot {
    /// Per-credential stake amounts
    pub stake: HashMap<HaskellCredential, Lovelace>,
    /// Stake credential -> pool delegation (only in old format, empty in new)
    pub delegations: HashMap<HaskellCredential, Hash28>,
    /// Pool params at snapshot time
    pub pool_params: HashMap<Hash28, HaskellPoolParams>,
}

/// Pool distribution: array(2)
#[derive(Debug)]
pub struct HaskellPoolDistr {
    pub individual_stakes: HashMap<Hash28, HaskellIndividualPoolStake>,
    pub total_active_stake: Lovelace,
}

/// IndividualPoolStake: array(3)
#[derive(Debug)]
pub struct HaskellIndividualPoolStake {
    pub stake_ratio_num: u64,
    pub stake_ratio_den: u64,
    pub total_stake: Lovelace,
    pub vrf_hash: Hash32,
}

/// ConwayGovState: array(7)
#[derive(Debug)]
pub struct HaskellConwayGovState {
    pub proposals: Vec<HaskellGovActionState>,
    pub committee: Option<HaskellCommittee>,
    pub constitution: HaskellConstitution,
    pub cur_pparams: HaskellPParams,
    pub prev_pparams: HaskellPParams,
    pub future_pparams: HaskellFuturePParams,
    pub drep_pulsing: HaskellDRepPulsingState,
}

/// Governance committee
#[derive(Debug)]
pub struct HaskellCommittee {
    pub members: HashMap<HaskellCredential, EpochNo>,
    pub threshold: HaskellRational,
}

/// Constitution
#[derive(Debug)]
pub struct HaskellConstitution {
    pub anchor: HaskellAnchor,
    pub guardrail_script: Option<Hash32>,
}

/// FuturePParams variant
#[derive(Debug)]
pub enum HaskellFuturePParams {
    NoPParamsUpdate,
    DefinitePParamsUpdate(HaskellPParams),
    PotentialPParamsUpdate(Option<HaskellPParams>),
}

/// DRepPulsingState (always DRComplete on disk)
#[derive(Debug)]
pub struct HaskellDRepPulsingState {
    pub snapshot: HaskellPulsingSnapshot,
    pub ratify_state: HaskellRatifyState,
}

/// PulsingSnapshot: array(4)
#[derive(Debug)]
pub struct HaskellPulsingSnapshot {
    pub proposals: Vec<HaskellGovActionState>,
    pub drep_distr: HashMap<HaskellDRep, Lovelace>,
    pub drep_state: HashMap<HaskellCredential, HaskellDRepState>,
    pub pool_distr: HashMap<Hash28, Lovelace>,
}

/// RatifyState: array(4)
#[derive(Debug)]
pub struct HaskellRatifyState {
    pub enact_state: HaskellEnactState,
    pub enacted: Vec<HaskellGovActionState>,
    pub expired: Vec<HaskellGovActionId>,
    pub delayed: bool,
}

/// EnactState: array(7)
#[derive(Debug)]
pub struct HaskellEnactState {
    pub committee: Option<HaskellCommittee>,
    pub constitution: HaskellConstitution,
    pub cur_pparams: HaskellPParams,
    pub prev_pparams: HaskellPParams,
    pub treasury: Lovelace,
    pub withdrawals: HashMap<HaskellCredential, Lovelace>,
    pub prev_gov_action_ids: HaskellPrevGovActionIds,
}

/// Previous governance action IDs per purpose
#[derive(Debug, Default)]
pub struct HaskellPrevGovActionIds {
    pub pparam_update: Option<HaskellGovActionId>,
    pub hard_fork: Option<HaskellGovActionId>,
    pub committee: Option<HaskellGovActionId>,
    pub constitution: Option<HaskellGovActionId>,
}

/// GovActionState
#[derive(Debug)]
pub struct HaskellGovActionState {
    pub action_id: HaskellGovActionId,
    pub committee_votes: HashMap<HaskellCredential, HaskellVote>,
    pub drep_votes: HashMap<HaskellCredential, HaskellVote>,
    pub spo_votes: HashMap<Hash28, HaskellVote>,
    pub proposal: HaskellProposalProcedure,
    pub proposed_in: EpochNo,
    pub expires_after: EpochNo,
}

/// GovActionId
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HaskellGovActionId {
    pub tx_id: Hash32,
    pub action_index: u32,
}

/// Vote
#[derive(Debug, Clone)]
pub enum HaskellVote {
    Yes,
    No,
    Abstain,
}

/// ProposalProcedure
#[derive(Debug)]
pub struct HaskellProposalProcedure {
    pub deposit: Lovelace,
    pub return_addr: Vec<u8>,
    pub gov_action: HaskellGovAction,
    pub anchor: HaskellAnchor,
}

/// GovAction (simplified -- we store the type tag + raw data for complex actions)
#[derive(Debug)]
pub enum HaskellGovAction {
    ParameterChange {
        prev_action_id: Option<HaskellGovActionId>,
        // PParams update stored as raw CBOR since PParamUpdate is complex
        pparams_update_raw: Vec<u8>,
        guardrail_script: Option<Hash32>,
    },
    HardForkInitiation {
        prev_action_id: Option<HaskellGovActionId>,
        protocol_version: (u64, u64),
    },
    TreasuryWithdrawals {
        withdrawals: HashMap<Vec<u8>, Lovelace>,
        guardrail_script: Option<Hash32>,
    },
    NoConfidence {
        prev_action_id: Option<HaskellGovActionId>,
    },
    UpdateCommittee {
        prev_action_id: Option<HaskellGovActionId>,
        members_to_remove: Vec<HaskellCredential>,
        members_to_add: HashMap<HaskellCredential, EpochNo>,
        threshold: HaskellRational,
    },
    NewConstitution {
        prev_action_id: Option<HaskellGovActionId>,
        constitution: HaskellConstitution,
    },
    InfoAction,
}

/// Credential (28-byte hash + type tag)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum HaskellCredential {
    KeyHash(Hash28),
    ScriptHash(Hash28),
}

/// DRep
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum HaskellDRep {
    KeyHash(Hash28),
    ScriptHash(Hash28),
    Abstain,
    NoConfidence,
}

/// Anchor
#[derive(Debug, Clone)]
pub struct HaskellAnchor {
    pub url: String,
    pub data_hash: Hash32,
}

/// Pool parameters
#[derive(Debug, Clone)]
pub struct HaskellPoolParams {
    pub operator: Hash28,
    pub vrf_keyhash: Hash32,
    pub pledge: Lovelace,
    pub cost: Lovelace,
    pub margin_numerator: u64,
    pub margin_denominator: u64,
    pub reward_account: Vec<u8>,
    pub owners: Vec<Hash28>,
    pub relays: Vec<HaskellRelay>,
    pub metadata: Option<HaskellPoolMetadata>,
}

/// Pool relay
#[derive(Debug, Clone)]
pub enum HaskellRelay {
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

/// Pool metadata
#[derive(Debug, Clone)]
pub struct HaskellPoolMetadata {
    pub url: String,
    pub hash: Hash32,
}

/// Rational number
#[derive(Debug, Clone)]
pub struct HaskellRational {
    pub numerator: u64,
    pub denominator: u64,
}

/// Protocol parameters (the fields of Conway PParams)
/// We parse these into Torsten's ProtocolParameters directly
#[derive(Debug)]
pub struct HaskellPParams {
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
    pub protocol_version_major: u64,
    pub protocol_version_minor: u64,
    pub min_pool_cost: u64,
    pub ada_per_utxo_byte: u64,
    pub cost_models: HashMap<u8, Vec<i64>>,
    pub prices_mem_num: u64,
    pub prices_mem_den: u64,
    pub prices_step_num: u64,
    pub prices_step_den: u64,
    pub max_tx_ex_units_mem: u64,
    pub max_tx_ex_units_steps: u64,
    pub max_block_ex_units_mem: u64,
    pub max_block_ex_units_steps: u64,
    pub max_val_size: u64,
    pub collateral_percentage: u64,
    pub max_collateral_inputs: u64,
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
    pub dvt_motion_no_confidence_num: u64,
    pub dvt_motion_no_confidence_den: u64,
    pub dvt_committee_normal_num: u64,
    pub dvt_committee_normal_den: u64,
    pub dvt_committee_no_confidence_num: u64,
    pub dvt_committee_no_confidence_den: u64,
    pub dvt_update_constitution_num: u64,
    pub dvt_update_constitution_den: u64,
    pub dvt_hard_fork_num: u64,
    pub dvt_hard_fork_den: u64,
    pub dvt_pp_network_group_num: u64,
    pub dvt_pp_network_group_den: u64,
    pub dvt_pp_economic_group_num: u64,
    pub dvt_pp_economic_group_den: u64,
    pub dvt_pp_technical_group_num: u64,
    pub dvt_pp_technical_group_den: u64,
    pub dvt_pp_gov_group_num: u64,
    pub dvt_pp_gov_group_den: u64,
    pub dvt_treasury_withdrawal_num: u64,
    pub dvt_treasury_withdrawal_den: u64,
    pub committee_min_size: u64,
    pub committee_max_term_length: u64,
    pub gov_action_lifetime: u64,
    pub gov_action_deposit: u64,
    pub drep_deposit: u64,
    pub drep_activity: u64,
    pub min_fee_ref_script_cost_per_byte_num: u64,
    pub min_fee_ref_script_cost_per_byte_den: u64,
}

/// Reward update (simplified -- we only need delta amounts)
#[derive(Debug)]
pub struct HaskellRewardUpdate {
    pub delta_treasury: i64,
    pub delta_reserves: i64,
    pub delta_fees: i64,
    pub rewards: HashMap<HaskellCredential, Lovelace>,
}
