//! Component sub-states for LedgerState.
//!
//! These structs group related fields from the monolithic LedgerState into
//! independently borrowable components, enabling granular `&mut` access
//! for era-specific rule dispatch.
//!
//! Haskell equivalents:
//! - UtxoSubState  ≈ UTxOState
//! - CertSubState  ≈ CertState (DState + PState)
//! - GovSubState   ≈ ConwayGovState / GovState era
//! - ConsensusSubState ≈ ChainDepState + NewEpochState nonce fields
//! - EpochSubState ≈ EpochState + SnapShots + protocol parameters

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use dugite_primitives::hash::{Hash28, Hash32};
use dugite_primitives::time::EpochNo;
use dugite_primitives::transaction::ProtocolParamUpdate;
use dugite_primitives::value::Lovelace;

use crate::utxo::UtxoSet;
use crate::utxo_diff::DiffSeq;

use super::{
    EpochSnapshots, GovernanceState, PendingRewardUpdate, PoolRegistration, StakeDistributionState,
};
use dugite_primitives::protocol_params::ProtocolParameters;

/// UTxO state: the unspent transaction output set and per-epoch fee accumulator.
#[derive(Debug, Clone)]
pub struct UtxoSubState {
    pub utxo_set: UtxoSet,
    pub diff_seq: DiffSeq,
    pub epoch_fees: Lovelace,
    pub pending_donations: Lovelace,
}

/// Delegation and pool state: stake credentials, pool registrations, reward accounts.
#[derive(Debug, Clone)]
pub struct CertSubState {
    pub delegations: Arc<HashMap<Hash32, Hash28>>,
    pub pool_params: Arc<HashMap<Hash28, PoolRegistration>>,
    pub future_pool_params: HashMap<Hash28, PoolRegistration>,
    pub pending_retirements: HashMap<Hash28, EpochNo>,
    pub reward_accounts: Arc<HashMap<Hash32, Lovelace>>,
    pub stake_key_deposits: HashMap<Hash32, u64>,
    pub pool_deposits: HashMap<Hash28, u64>,
    pub total_stake_key_deposits: u64,
    pub pointer_map: HashMap<dugite_primitives::credentials::Pointer, Hash32>,
    pub stake_distribution: StakeDistributionState,
    pub script_stake_credentials: HashSet<Hash32>,
}

/// Governance state: proposals, votes, DReps, committee.
#[derive(Debug, Clone)]
pub struct GovSubState {
    pub governance: Arc<GovernanceState>,
}

/// Consensus-layer state: nonces, block production counters, opcert tracking.
#[derive(Debug, Clone)]
pub struct ConsensusSubState {
    pub evolving_nonce: Hash32,
    pub candidate_nonce: Hash32,
    pub epoch_nonce: Hash32,
    pub lab_nonce: Hash32,
    pub last_epoch_block_nonce: Hash32,
    pub rolling_nonce: Hash32,
    pub first_block_hash_of_epoch: Option<Hash32>,
    pub prev_epoch_first_block_hash: Option<Hash32>,
    pub epoch_blocks_by_pool: Arc<HashMap<Hash28, u64>>,
    pub epoch_block_count: u64,
    pub opcert_counters: HashMap<Hash28, u64>,
}

/// Epoch-level state: snapshots, treasury/reserves, protocol parameters.
///
/// Protocol parameters live here because they change at epoch boundaries
/// (via governance enactment or pre-Conway PP update proposals). This allows
/// `process_epoch_transition` to mutate them via `&mut EpochSubState`.
#[derive(Debug, Clone)]
pub struct EpochSubState {
    pub snapshots: EpochSnapshots,
    pub treasury: Lovelace,
    pub reserves: Lovelace,
    pub pending_reward_update: Option<PendingRewardUpdate>,
    pub pending_pp_updates: BTreeMap<EpochNo, Vec<(Hash32, ProtocolParamUpdate)>>,
    pub future_pp_updates: BTreeMap<EpochNo, Vec<(Hash32, ProtocolParamUpdate)>>,
    pub needs_stake_rebuild: bool,
    pub ptr_stake: HashMap<dugite_primitives::credentials::Pointer, u64>,
    pub ptr_stake_excluded: bool,
    pub protocol_params: ProtocolParameters,
    pub prev_protocol_params: ProtocolParameters,
    pub prev_protocol_version_major: u64,
    pub prev_d: f64,
}
