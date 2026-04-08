//! Flat wire format for LedgerState snapshot serialization.
//!
//! `LedgerStateSnapshot` preserves the **exact bincode field ordering** of
//! the original monolithic `LedgerState` struct.  Bincode is positional: it
//! encodes/decodes fields in declaration order with no field names in the
//! wire format.  Adding, removing, or reordering fields silently corrupts
//! existing snapshots.
//!
//! # CRITICAL INVARIANT
//!
//! **The field order in `LedgerStateSnapshot` MUST match the field order of
//! `LedgerState` as it existed when snapshots were first written.**
//!
//! When `LedgerState` is restructured to use sub-state structs (Task 6),
//! the in-memory layout will change but this struct stays frozen.  Snapshot
//! save/load goes through `LedgerState -> LedgerStateSnapshot -> bincode`
//! (and the reverse), keeping the on-disk format stable.
//!
//! Do NOT reorder, rename (in a way that changes position), or remove any
//! field without a migration path.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use dugite_primitives::hash::{Hash28, Hash32};
use dugite_primitives::protocol_params::ProtocolParameters;
use dugite_primitives::time::EpochNo;
use dugite_primitives::transaction::ProtocolParamUpdate;
use dugite_primitives::value::Lovelace;
use serde::{Deserialize, Serialize};

use super::{
    default_d_one, default_lovelace_zero, default_prev_proto_major, default_prev_protocol_params,
    default_update_quorum, EpochSnapshots, GovernanceState, PendingRewardUpdate, PoolRegistration,
    StakeDistributionState,
};
use crate::plutus::SlotConfig;
use crate::utxo::UtxoSet;
use crate::utxo_diff::DiffSeq;
use dugite_primitives::block::Tip;
use dugite_primitives::era::Era;

/// Stable bincode wire format matching the original `LedgerState` field layout.
///
/// Every field here mirrors the original `LedgerState` declaration order,
/// including all `serde` attributes that affect deserialization defaults.
/// See the module-level documentation for the invariant that MUST be upheld.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerStateSnapshot {
    /// Current UTxO set
    pub utxo_set: UtxoSet,
    /// Current tip of the chain
    pub tip: Tip,
    /// Current era
    pub era: Era,
    /// Pending era transition detected from the block stream.
    #[serde(skip, default)]
    pub pending_era_transition: Option<(Era, Era, EpochNo)>,
    /// Current epoch
    pub epoch: EpochNo,
    /// Shelley epoch length in slots
    pub epoch_length: u64,
    /// Number of Byron epochs before the Shelley hard fork.
    #[serde(default)]
    pub shelley_transition_epoch: u64,
    /// Byron epoch length in slots (10 * k). 0 = mainnet default (21600).
    #[serde(default)]
    pub byron_epoch_length: u64,
    /// Current protocol parameters (curPParams in Haskell).
    pub protocol_params: ProtocolParameters,
    /// Previous epoch's protocol parameters (Haskell's prevPParams).
    #[serde(default = "default_prev_protocol_params")]
    pub prev_protocol_params: ProtocolParameters,
    /// Cached prev_d for backward compatibility and serde.
    #[serde(default = "default_d_one")]
    pub prev_d: f64,
    /// Cached prev protocol major version for backward compatibility.
    #[serde(default = "default_prev_proto_major")]
    pub prev_protocol_version_major: u64,
    /// Stake distribution
    pub stake_distribution: StakeDistributionState,
    /// Treasury balance
    pub treasury: Lovelace,
    /// Pending treasury donations (Conway `TreasuryDonation`).
    #[serde(default = "default_lovelace_zero")]
    pub pending_donations: Lovelace,
    /// Reserves balance (ADA not yet in circulation)
    pub reserves: Lovelace,
    /// Delegation state: credential_hash -> pool_id
    pub delegations: Arc<HashMap<Hash32, Hash28>>,
    /// Pool registrations: pool_id -> pool registration
    pub pool_params: Arc<HashMap<Hash28, PoolRegistration>>,
    /// Future pool parameters for re-registrations.
    #[serde(default)]
    pub future_pool_params: HashMap<Hash28, PoolRegistration>,
    /// Pool retirements pending: pool -> retirement epoch.
    pub pending_retirements: HashMap<Hash28, EpochNo>,
    /// Stake snapshots for the Cardano "mark/set/go" snapshot model
    pub snapshots: EpochSnapshots,
    /// Reward accounts: stake credential hash -> accumulated rewards
    pub reward_accounts: Arc<HashMap<Hash32, Lovelace>>,
    /// Pointer map: certificate pointers -> credential hashes.
    #[serde(default)]
    pub pointer_map: HashMap<dugite_primitives::credentials::Pointer, Hash32>,
    /// Genesis delegates: genesis_key_hash -> (delegate_key_hash, vrf_key_hash).
    #[serde(default)]
    pub genesis_delegates: HashMap<Hash28, (Hash28, Hash32)>,
    /// Fees collected in the current epoch
    pub epoch_fees: Lovelace,
    /// Number of blocks produced by each pool in the current epoch
    pub epoch_blocks_by_pool: Arc<HashMap<Hash28, u64>>,
    /// Total blocks in the current epoch
    pub epoch_block_count: u64,
    /// Evolving nonce (eta_v): accumulated hash of ALL VRF outputs.
    pub evolving_nonce: Hash32,
    /// Candidate nonce: snapshot of evolving_nonce that freezes late in each epoch.
    pub candidate_nonce: Hash32,
    /// Current epoch nonce.
    pub epoch_nonce: Hash32,
    /// LAB nonce: prev_hash of the most recent block.
    pub lab_nonce: Hash32,
    /// Snapshot of lab_nonce at epoch boundary.
    pub last_epoch_block_nonce: Hash32,
    /// Randomness stabilisation window: ceiling(4k/f).
    pub randomness_stabilisation_window: u64,
    /// Stability window: ceiling(3k/f).
    #[serde(default)]
    pub stability_window_3kf: u64,
    /// Shelley genesis hash (used for initial nonce state)
    pub genesis_hash: Hash32,
    // Legacy fields kept for serde backwards compatibility with existing snapshots
    #[serde(default)]
    rolling_nonce: Hash32,
    #[serde(default)]
    stability_window: u64,
    #[serde(default)]
    first_block_hash_of_epoch: Option<Hash32>,
    #[serde(default)]
    prev_epoch_first_block_hash: Option<Hash32>,
    /// Current protocol parameter update proposals (pre-Conway).
    pub pending_pp_updates: BTreeMap<EpochNo, Vec<(Hash32, ProtocolParamUpdate)>>,
    /// Future protocol parameter update proposals (pre-Conway).
    #[serde(default)]
    pub future_pp_updates: BTreeMap<EpochNo, Vec<(Hash32, ProtocolParamUpdate)>>,
    /// Quorum for pre-Conway protocol parameter updates.
    #[serde(default = "default_update_quorum")]
    pub update_quorum: u64,
    /// Conway governance state
    pub governance: Arc<GovernanceState>,
    /// Slot configuration for Plutus time conversion
    pub slot_config: SlotConfig,
    /// Whether stake distribution needs a full rebuild after snapshot load.
    #[serde(skip)]
    pub needs_stake_rebuild: bool,
    /// Pointer-addressed UTxO stake: pointer -> coin amount.
    #[serde(default)]
    pub ptr_stake: HashMap<dugite_primitives::credentials::Pointer, u64>,
    /// Whether pointer-addressed UTxO stake has been excluded from stake_distribution.
    #[serde(skip)]
    pub ptr_stake_excluded: bool,
    /// Pending reward update retained for backward compatibility.
    #[serde(default)]
    pub pending_reward_update: Option<PendingRewardUpdate>,
    /// Running total of all stake key deposits locked in the ledger (lovelace).
    #[serde(default)]
    pub total_stake_key_deposits: u64,
    /// Script-type stake credentials.
    #[serde(default)]
    pub script_stake_credentials: std::collections::HashSet<Hash32>,
    /// Per-block UTxO diffs for the last k blocks.
    #[serde(skip)]
    pub diff_seq: DiffSeq,
    /// The network this node is running on.
    #[serde(skip)]
    pub node_network: Option<dugite_primitives::network::NetworkId>,
    /// Operational certificate counters per pool.
    #[serde(default)]
    pub opcert_counters: HashMap<Hash28, u64>,
    /// Per-credential deposit paid at stake key registration time (lovelace).
    #[serde(default)]
    pub stake_key_deposits: HashMap<Hash32, u64>,
    /// Per-pool deposit paid at pool registration time (lovelace).
    #[serde(default)]
    pub pool_deposits: HashMap<Hash28, u64>,
}
