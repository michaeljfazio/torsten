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

// ── From conversions for snapshot roundtrip ─────────────────────────

impl From<&super::LedgerState> for LedgerStateSnapshot {
    fn from(s: &super::LedgerState) -> Self {
        LedgerStateSnapshot {
            // UTxO sub-state
            utxo_set: s.utxo.utxo_set.clone(),
            diff_seq: s.utxo.diff_seq.clone(),
            epoch_fees: s.utxo.epoch_fees,
            pending_donations: s.utxo.pending_donations,
            // Cert sub-state
            delegations: Arc::clone(&s.certs.delegations),
            pool_params: Arc::clone(&s.certs.pool_params),
            future_pool_params: s.certs.future_pool_params.clone(),
            pending_retirements: s.certs.pending_retirements.clone(),
            reward_accounts: Arc::clone(&s.certs.reward_accounts),
            stake_key_deposits: s.certs.stake_key_deposits.clone(),
            pool_deposits: s.certs.pool_deposits.clone(),
            total_stake_key_deposits: s.certs.total_stake_key_deposits,
            pointer_map: s.certs.pointer_map.clone(),
            stake_distribution: s.certs.stake_distribution.clone(),
            script_stake_credentials: s.certs.script_stake_credentials.clone(),
            // Gov sub-state
            governance: Arc::clone(&s.gov.governance),
            // Consensus sub-state
            evolving_nonce: s.consensus.evolving_nonce,
            candidate_nonce: s.consensus.candidate_nonce,
            epoch_nonce: s.consensus.epoch_nonce,
            lab_nonce: s.consensus.lab_nonce,
            last_epoch_block_nonce: s.consensus.last_epoch_block_nonce,
            rolling_nonce: s.consensus.rolling_nonce,
            first_block_hash_of_epoch: s.consensus.first_block_hash_of_epoch,
            prev_epoch_first_block_hash: s.consensus.prev_epoch_first_block_hash,
            epoch_blocks_by_pool: Arc::clone(&s.consensus.epoch_blocks_by_pool),
            epoch_block_count: s.consensus.epoch_block_count,
            opcert_counters: s.consensus.opcert_counters.clone(),
            // Epoch sub-state
            snapshots: s.epochs.snapshots.clone(),
            treasury: s.epochs.treasury,
            reserves: s.epochs.reserves,
            pending_reward_update: s.epochs.pending_reward_update.clone(),
            pending_pp_updates: s.epochs.pending_pp_updates.clone(),
            future_pp_updates: s.epochs.future_pp_updates.clone(),
            needs_stake_rebuild: s.epochs.needs_stake_rebuild,
            ptr_stake: s.epochs.ptr_stake.clone(),
            ptr_stake_excluded: s.epochs.ptr_stake_excluded,
            protocol_params: s.epochs.protocol_params.clone(),
            prev_protocol_params: s.epochs.prev_protocol_params.clone(),
            prev_protocol_version_major: s.epochs.prev_protocol_version_major,
            prev_d: s.epochs.prev_d,
            // Coordination fields
            tip: s.tip.clone(),
            era: s.era,
            pending_era_transition: s.pending_era_transition,
            epoch: s.epoch,
            epoch_length: s.epoch_length,
            shelley_transition_epoch: s.shelley_transition_epoch,
            byron_epoch_length: s.byron_epoch_length,
            slot_config: s.slot_config,
            genesis_hash: s.genesis_hash,
            genesis_delegates: s.genesis_delegates.clone(),
            update_quorum: s.update_quorum,
            node_network: s.node_network,
            randomness_stabilisation_window: s.randomness_stabilisation_window,
            stability_window_3kf: s.stability_window_3kf,
            // Legacy field (always zero for new snapshots)
            stability_window: 0,
        }
    }
}

impl From<LedgerStateSnapshot> for super::LedgerState {
    fn from(s: LedgerStateSnapshot) -> Self {
        use super::substates::*;

        super::LedgerState {
            utxo: UtxoSubState {
                utxo_set: s.utxo_set,
                diff_seq: s.diff_seq,
                epoch_fees: s.epoch_fees,
                pending_donations: s.pending_donations,
            },
            certs: CertSubState {
                delegations: s.delegations,
                pool_params: s.pool_params,
                future_pool_params: s.future_pool_params,
                pending_retirements: s.pending_retirements,
                reward_accounts: s.reward_accounts,
                stake_key_deposits: s.stake_key_deposits,
                pool_deposits: s.pool_deposits,
                total_stake_key_deposits: s.total_stake_key_deposits,
                pointer_map: s.pointer_map,
                stake_distribution: s.stake_distribution,
                script_stake_credentials: s.script_stake_credentials,
            },
            gov: GovSubState {
                governance: s.governance,
            },
            consensus: ConsensusSubState {
                evolving_nonce: s.evolving_nonce,
                candidate_nonce: s.candidate_nonce,
                epoch_nonce: s.epoch_nonce,
                lab_nonce: s.lab_nonce,
                last_epoch_block_nonce: s.last_epoch_block_nonce,
                rolling_nonce: s.rolling_nonce,
                first_block_hash_of_epoch: s.first_block_hash_of_epoch,
                prev_epoch_first_block_hash: s.prev_epoch_first_block_hash,
                epoch_blocks_by_pool: s.epoch_blocks_by_pool,
                epoch_block_count: s.epoch_block_count,
                opcert_counters: s.opcert_counters,
            },
            epochs: EpochSubState {
                snapshots: s.snapshots,
                treasury: s.treasury,
                reserves: s.reserves,
                pending_reward_update: s.pending_reward_update,
                pending_pp_updates: s.pending_pp_updates,
                future_pp_updates: s.future_pp_updates,
                needs_stake_rebuild: s.needs_stake_rebuild,
                ptr_stake: s.ptr_stake,
                ptr_stake_excluded: s.ptr_stake_excluded,
                protocol_params: s.protocol_params,
                prev_protocol_params: s.prev_protocol_params,
                prev_protocol_version_major: s.prev_protocol_version_major,
                prev_d: s.prev_d,
            },
            tip: s.tip,
            era: s.era,
            pending_era_transition: s.pending_era_transition,
            epoch: s.epoch,
            epoch_length: s.epoch_length,
            shelley_transition_epoch: s.shelley_transition_epoch,
            byron_epoch_length: s.byron_epoch_length,
            slot_config: s.slot_config,
            genesis_hash: s.genesis_hash,
            genesis_delegates: s.genesis_delegates,
            update_quorum: s.update_quorum,
            node_network: s.node_network,
            randomness_stabilisation_window: s.randomness_stabilisation_window,
            stability_window_3kf: s.stability_window_3kf,
            security_param: 0, // Set from genesis config at startup via set_epoch_length()
            conway_genesis_init: None, // Set from genesis config at startup
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LedgerState;

    #[test]
    fn test_ledger_state_snapshot_roundtrip() {
        // Create a LedgerState with non-default values to catch field mismatches
        let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());
        state.epoch = EpochNo(42);
        state.epochs.treasury = Lovelace(1_000_000);
        state.epochs.reserves = Lovelace(999_000_000);

        // Convert to snapshot format
        let snapshot = LedgerStateSnapshot::from(&state);

        // Convert back
        let restored = LedgerState::from(snapshot);

        // Verify key fields survive the roundtrip
        assert_eq!(restored.epoch, state.epoch);
        assert_eq!(restored.epochs.treasury, state.epochs.treasury);
        assert_eq!(restored.epochs.reserves, state.epochs.reserves);
        assert_eq!(restored.era, state.era);
        assert_eq!(
            restored.epochs.protocol_params.protocol_version_major,
            state.epochs.protocol_params.protocol_version_major
        );
    }

    #[test]
    fn test_bincode_roundtrip_through_snapshot_format() {
        let state = LedgerState::new(ProtocolParameters::mainnet_defaults());

        // Serialize via snapshot format
        let snapshot = LedgerStateSnapshot::from(&state);
        let bytes = bincode::serialize(&snapshot).expect("serialize");

        // Deserialize back through snapshot format
        let restored_snapshot: LedgerStateSnapshot =
            bincode::deserialize(&bytes).expect("deserialize");
        let restored = LedgerState::from(restored_snapshot);

        // Verify key fields
        assert_eq!(restored.epoch, state.epoch);
        assert_eq!(restored.era, state.era);
        assert_eq!(
            restored.epochs.protocol_params.protocol_version_major,
            state.epochs.protocol_params.protocol_version_major
        );
    }
}
