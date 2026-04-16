mod apply;
mod certificates;
mod epoch;
pub(crate) mod governance;
mod protocol_params;
mod rewards;
mod snapshot;
pub mod snapshot_format;
pub mod substates;

// Re-export governance free functions and types for use by tests
#[cfg(test)]
pub(crate) use governance::{
    check_cc_approval, check_threshold, gov_action_priority, is_delaying_action,
    modified_pp_groups, pp_change_drep_all_groups_met, pp_change_drep_threshold,
    pp_change_spo_threshold, prev_action_as_expected, DRepPPGroup, StakePoolPPGroup,
};
pub use rewards::compute_reward_update;
#[doc(hidden)]
pub use rewards::Rat;
pub use snapshot_format::LedgerStateSnapshot;
pub use substates::{CertSubState, ConsensusSubState, EpochSubState, GovSubState, UtxoSubState};

use crate::plutus::SlotConfig;
use crate::utxo::UtxoSet;
use crate::utxo_diff::DiffSeq;
#[cfg(test)]
use dugite_primitives::block::Block;
use dugite_primitives::block::{Point, Tip};
use dugite_primitives::credentials::Credential;
use dugite_primitives::era::Era;
use dugite_primitives::hash::{Hash28, Hash32};
use dugite_primitives::protocol_params::ProtocolParameters;
use dugite_primitives::time::{BlockNo, EpochNo, SlotNo};
use dugite_primitives::transaction::{
    Anchor, Constitution, DRep, GovActionId, ProposalProcedure, Rational, Relay, Voter,
    VotingProcedure,
};
use dugite_primitives::value::Lovelace;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use tracing::{debug, info, trace};

/// Total ADA supply (45 billion ADA = 45 * 10^15 lovelace)
pub const MAX_LOVELACE_SUPPLY: u64 = 45_000_000_000_000_000;

/// Maximum allowed snapshot file size (10 GiB).
/// Prevents OOM from loading maliciously crafted or corrupted snapshot files.
pub const MAX_SNAPSHOT_SIZE: usize = 10 * 1024 * 1024 * 1024;

/// Controls whether `apply_block()` re-evaluates Plutus scripts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockValidationMode {
    /// Full Phase-1 + Phase-2 Plutus evaluation for new network blocks.
    /// Rejects the block if the `is_valid` flag doesn't match the actual
    /// script evaluation result (`ValidationTagMismatch`).
    ValidateAll,
    /// Trust the block producer's `is_valid` flag without re-evaluating scripts.
    /// Used for ImmutableDB replay, Mithril import, rollback replay, and self-forged blocks.
    ApplyOnly,
}

fn default_update_quorum() -> u64 {
    5 // Mainnet default: 5 out of 7 genesis delegates
}

fn default_d_one() -> f64 {
    1.0 // Genesis default: d=1 (fully federated)
}

fn default_prev_proto_major() -> u64 {
    6 // Genesis default: Alonzo (proto 6)
}

fn default_prev_protocol_params() -> ProtocolParameters {
    ProtocolParameters::mainnet_defaults()
}

/// The complete ledger state, decomposed into component sub-states for granular borrowing.
///
/// Serialization goes through `LedgerStateSnapshot` (see `snapshot_format.rs`).
/// Do NOT derive Serialize/Deserialize on this struct directly.
///
/// Large collections (`delegations`, `pool_params`, `reward_accounts`,
/// `governance`, `epoch_blocks_by_pool`) are wrapped in `Arc` for
/// copy-on-write semantics.  Cloning a `LedgerState` is therefore cheap:
/// it only bumps reference counts instead of deep-copying megabytes of
/// data.  Mutations go through `Arc::make_mut()`, which clones the inner
/// collection only when there are other outstanding references.
#[derive(Debug, Clone)]
pub struct LedgerState {
    // ── Component sub-states (independently borrowable) ──────────────
    /// UTxO state: the unspent transaction output set, per-epoch fees, and UTxO diffs.
    pub utxo: UtxoSubState,
    /// Delegation and pool state: credentials, pool registrations, reward accounts.
    pub certs: CertSubState,
    /// Conway governance state: proposals, votes, DReps, constitutional committee.
    pub gov: GovSubState,
    /// Consensus-layer state: nonces, block production counters, opcert tracking.
    pub consensus: ConsensusSubState,
    /// Epoch-level state: snapshots, treasury/reserves, protocol parameters.
    pub epochs: EpochSubState,

    // ── Coordination (immutable config or cross-cutting bookkeeping) ──
    /// Current tip of the chain
    pub tip: Tip,
    /// Current era
    pub era: Era,
    /// Pending era transition detected from the block stream.
    /// Set when `block.era > self.era` during `apply_block`.
    /// Consumed by the node layer to update the consensus-level `EraHistory`.
    /// `(previous_era, new_era, transition_epoch)`.
    pub pending_era_transition: Option<(Era, Era, EpochNo)>,
    /// Current epoch
    pub epoch: EpochNo,
    /// Shelley epoch length in slots
    pub epoch_length: u64,
    /// Number of Byron epochs before the Shelley hard fork.
    /// Total Byron slots = byron_epoch_length * shelley_transition_epoch.
    pub shelley_transition_epoch: u64,
    /// Byron epoch length in slots (10 * k). 0 = mainnet default (21600).
    pub byron_epoch_length: u64,
    /// Slot configuration for Plutus time conversion
    pub slot_config: SlotConfig,
    /// Shelley genesis hash (used for initial nonce state)
    pub genesis_hash: Hash32,
    /// Genesis delegates: genesis_key_hash (28 bytes) -> (delegate_key_hash (28 bytes), vrf_key_hash (32 bytes)).
    ///
    /// Loaded from the Shelley genesis file and mutated by `Certificate::GenesisKeyDelegation`
    /// (Shelley-era only; Conway removed the cert type). Used for BFT overlay
    /// schedule validation during early Shelley era (when d > 0).
    pub genesis_delegates: HashMap<Hash28, (Hash28, Hash32)>,
    /// Quorum for pre-Conway protocol parameter updates (from Shelley genesis)
    pub update_quorum: u64,
    /// The network this node is running on (mainnet, testnet, etc.).
    ///
    /// Used for unconditional output/withdrawal address network checks during
    /// Phase-1 validation (Haskell's `Globals.networkId`).  Not persisted in
    /// snapshots — set from genesis/config at node startup.
    pub node_network: Option<dugite_primitives::network::NetworkId>,
    /// Randomness stabilisation window: ceiling(4k/f) for Conway+.
    pub randomness_stabilisation_window: u64,
    /// Stability window: ceiling(3k/f) for Alonzo/Babbage (per Haskell erratum 17.3).
    pub stability_window_3kf: u64,
    /// Security parameter k — maximum rollback depth.
    /// Not persisted in snapshots; set from genesis config at startup.
    pub security_param: u64,
    /// Conway genesis initialization data (needed by era-transition rules).
    /// Populated from conway-genesis.json at node startup; not persisted in snapshots.
    pub conway_genesis_init: Option<crate::eras::ConwayGenesisInit>,
}

/// Pending reward update matching Haskell's RUPD structure.
///
/// Computed at one epoch boundary and applied at the next. Contains:
/// - Per-account rewards to credit
/// - Treasury increase (tau cut + undistributed)
/// - Reserves decrease (monetary expansion)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PendingRewardUpdate {
    /// Rewards to add to each registered stake credential's reward account.
    pub rewards: HashMap<Hash32, Lovelace>,
    /// Total treasury increase (tau cut + undistributed rewards).
    pub delta_treasury: u64,
    /// Total reserves decrease (monetary expansion).
    pub delta_reserves: u64,
}

// ── Governance proposal priority forest types ─────────────────────────
//
// Per Haskell `Cardano.Ledger.Conway.Governance.Proposals`:
//   Proposals { pProps, pRoots :: GovRelation PRoot, pGraph :: GovRelation PGraph }
//
// Each governance purpose (PParam, HardFork, Committee, Constitution) maintains
// a tree of proposals rooted at the last enacted action.  This enables O(k)
// descendant removal for both expiry and sibling cleanup after enactment.

/// Parent-child edges for a proposal node within a governance purpose tree.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PEdges {
    /// Parent proposal ID (None if this proposal is a direct child of the root).
    pub parent: Option<GovActionId>,
    /// Direct children — proposals whose `prev_action_id` points to this one.
    pub children: BTreeSet<GovActionId>,
}

/// Root of a governance purpose tree — tracks the last enacted action and its
/// direct children (proposals whose `prev_action_id` matches the root).
///
/// Matches Haskell's `PRoot { prRoot, prChildren }`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PRoot {
    /// Last enacted GovActionId for this purpose (None = genesis / no enactment yet).
    pub root: Option<GovActionId>,
    /// Direct children of the root (proposals whose `prev_action_id == root`).
    pub children: BTreeSet<GovActionId>,
}

/// Per-purpose DAG of proposal parent-child relationships for non-root proposals.
///
/// Matches Haskell's `PGraph { unPGraph :: Map (GovPurposeId p) PEdges }`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PGraph {
    /// Map from proposal ID to its edges (parent + children).
    pub nodes: HashMap<GovActionId, PEdges>,
}

/// One value per governance purpose (4 purposes).
///
/// Mirrors Haskell's `GovRelation f` which holds one `f` per `GovActionPurpose`:
///   0 = PParamUpdate, 1 = HardForkInitiation, 2 = Committee (shared by
///   NoConfidence + UpdateCommittee), 3 = Constitution.
///
/// `TreasuryWithdrawals` and `InfoAction` have no purpose tree.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct GovRelation<T: Default> {
    /// ParameterChange proposals.
    pub pparam: T,
    /// HardForkInitiation proposals.
    pub hard_fork: T,
    /// Committee-purpose proposals (NoConfidence + UpdateCommittee share this).
    pub committee: T,
    /// NewConstitution proposals.
    pub constitution: T,
}

impl<T: Default> GovRelation<T> {
    /// Access the value for a governance purpose by tag.
    ///
    /// Tags match `gov_action_purpose_tag()`: 0=PParam, 1=HardFork, 2=Committee, 3=Constitution.
    ///
    /// # Panics
    /// Panics if `purpose > 3`.
    pub fn get(&self, purpose: u8) -> &T {
        match purpose {
            0 => &self.pparam,
            1 => &self.hard_fork,
            2 => &self.committee,
            3 => &self.constitution,
            _ => panic!("invalid governance purpose tag: {purpose}"),
        }
    }

    /// Mutable access to the value for a governance purpose by tag.
    ///
    /// # Panics
    /// Panics if `purpose > 3`.
    pub fn get_mut(&mut self, purpose: u8) -> &mut T {
        match purpose {
            0 => &mut self.pparam,
            1 => &mut self.hard_fork,
            2 => &mut self.committee,
            3 => &mut self.constitution,
            _ => panic!("invalid governance purpose tag: {purpose}"),
        }
    }
}

/// Conway-era governance state (CIP-1694)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GovernanceState {
    /// Registered DReps: credential -> DRepState.
    ///
    /// This map contains ALL currently-registered DReps — entries are added by
    /// `RegDRep` certificates and removed by `UnregDRep` certificates.  It does
    /// NOT shrink when a DRep becomes inactive due to `drep_activity` expiry;
    /// inactive DReps are merely flagged `active = false` at each epoch boundary
    /// (matching Haskell's `vsDReps` map semantics).
    ///
    /// Use [`GovernanceState::active_drep_count`] to obtain the count of DReps
    /// whose activity flag is still `true` (i.e. those that contribute voting
    /// power and that external tools like Koios report as "registered").
    pub dreps: HashMap<Hash32, DRepRegistration>,
    /// Vote delegations: stake credential hash -> DRep
    pub vote_delegations: HashMap<Hash32, DRep>,
    /// Constitutional committee: cold credential -> hot credential
    pub committee_hot_keys: HashMap<Hash32, Hash32>,
    /// Committee member expiration epochs (cold credential -> expiration epoch)
    pub committee_expiration: HashMap<Hash32, EpochNo>,
    /// Resigned committee members
    pub committee_resigned: HashMap<Hash32, Option<Anchor>>,
    /// Script-type cold committee credentials (credential_type = 1 for N2C queries).
    /// Populated from CommitteeHotAuth and CommitteeColdResign certificates when the cold
    /// credential is a Credential::Script variant.  Used to correctly set cold_credential_type
    /// in GetCommitteeState responses without changing the Hash32-keyed committee maps.
    #[serde(default)]
    pub script_committee_credentials: std::collections::HashSet<Hash32>,
    /// Script-type hot committee credentials (hot_credential_type = 1 for N2C queries).
    /// Populated from CommitteeHotAuth certificates when the hot credential is a
    /// Credential::Script variant.  Maps cold_credential_hash -> hot_credential_hash for
    /// script hot keys, so a re-authorization with a key hot key correctly removes the entry.
    /// Used to correctly set hot_credential_type in GetCommitteeState responses.
    #[serde(default)]
    pub script_committee_hot_credentials: std::collections::HashSet<Hash32>,
    /// Active governance proposals indexed by GovActionId
    pub proposals: BTreeMap<GovActionId, ProposalState>,
    /// Votes cast, indexed by action ID for efficient ratification lookup
    pub votes_by_action: BTreeMap<GovActionId, Vec<(Voter, VotingProcedure)>>,
    /// Proposal forest roots: last enacted action per governance purpose + direct children.
    ///
    /// Per Haskell `pRoots :: GovRelation PRoot`.  Each of the 4 purposes tracks the
    /// last enacted `GovActionId` (the root) and proposals whose `prev_action_id`
    /// matches that root.  Used for O(1) sibling lookups during enactment.
    ///
    /// `serde(default)` for backward compatibility with pre-forest snapshots.
    #[serde(default)]
    pub proposal_roots: GovRelation<PRoot>,
    /// Proposal forest graph: parent-child edges per governance purpose for non-root proposals.
    ///
    /// Per Haskell `pGraph :: GovRelation PGraph`.  Proposals deeper than one level
    /// (i.e. their `prev_action_id` points to another proposal rather than the enacted
    /// root) are tracked here.  Used for O(k) descendant collection during removal.
    ///
    /// `serde(default)` for backward compatibility with pre-forest snapshots.
    #[serde(default)]
    pub proposal_graph: GovRelation<PGraph>,
    /// Total DRep registrations count (including deregistered)
    pub drep_registration_count: u64,
    /// Total proposals submitted
    pub proposal_count: u64,
    /// Current constitution (set by NewConstitution governance action)
    pub constitution: Option<Constitution>,
    /// Whether the committee is in a no-confidence state (dissolved by NoConfidence action)
    #[serde(default)]
    pub no_confidence: bool,
    /// Committee quorum threshold (from genesis or UpdateCommittee action)
    /// This is the fraction of active CC members that must vote Yes to approve.
    #[serde(default)]
    pub committee_threshold: Option<Rational>,
    /// Last enacted governance action IDs per purpose (for prev_action_id chain validation).
    /// Matches Haskell's `GovRelation StrictMaybe` / `ensPrevGovActionIds`.
    #[serde(default)]
    pub enacted_pparam_update: Option<GovActionId>,
    #[serde(default)]
    pub enacted_hard_fork: Option<GovActionId>,
    #[serde(default)]
    pub enacted_committee: Option<GovActionId>,
    #[serde(default)]
    pub enacted_constitution: Option<GovActionId>,
    /// Last ratification results (from most recent epoch transition).
    /// Used by GetRatifyState (N2C query tag 32).
    #[serde(default)]
    pub last_ratified: Vec<(GovActionId, ProposalState)>,
    #[serde(default)]
    pub last_expired: Vec<GovActionId>,
    #[serde(default)]
    pub last_ratify_delayed: bool,
    /// Number of "dormant epochs" accumulated since the start of the Conway era.
    ///
    /// Per Haskell `vsNumDormantEpochs` (Conway.Rules.Epoch, `updateNumDormantEpochs`):
    /// an epoch is "dormant" if there were no active governance proposals at the epoch
    /// boundary (i.e. `proposals` was empty during that epoch).  The dormant count is
    /// baked into `DRepRegistration::drep_expiry` at registration/vote time via
    /// `compute_drep_expiry()`, so it is NOT subtracted again at activity-check time.
    ///
    /// `serde(default)` ensures backward compatibility with existing ledger snapshots.
    #[serde(default)]
    pub num_dormant_epochs: u64,
    /// DRep voting power snapshot captured at each epoch boundary (the "mark" snapshot).
    ///
    /// Maps DRep credential hash → total delegated stake (lovelace).  Only active DReps
    /// (those whose `active` flag is `true`) appear in this map.
    ///
    /// Per Haskell `reDRepDistr` in `Conway.Rules.Epoch`, DRep voting power used during
    /// ratification is measured against the snapshot taken at the *start* of the current
    /// epoch, not the live state.  This prevents mid-epoch stake movements from
    /// affecting in-flight governance ratification.
    ///
    /// Populated by `process_epoch_transition` at each epoch boundary.
    /// `serde(default)` ensures backward compatibility with existing ledger snapshots.
    #[serde(default)]
    pub drep_distribution_snapshot: HashMap<Hash32, u64>,
    /// Snapshot of total `AlwaysNoConfidence`-delegated stake at the last epoch boundary.
    /// Companion to `drep_distribution_snapshot`.
    #[serde(default)]
    pub drep_snapshot_no_confidence: u64,
    /// Snapshot of total `AlwaysAbstain`-delegated stake at the last epoch boundary.
    /// Companion to `drep_distribution_snapshot`.
    #[serde(default)]
    pub drep_snapshot_abstain: u64,
    /// Frozen ratification snapshot from the previous epoch boundary.
    ///
    /// Analogous to Haskell's `DRepPulsingState` / `PulsingSnapshot`.  Captured at
    /// epoch boundary E (after ratification/expiry/enactment); consumed by
    /// `ratify_proposals()` at boundary E+1.  This ensures proposals and votes
    /// submitted during epoch E are not considered for ratification until E+1→E+2,
    /// matching the Haskell DRep pulser timing.
    ///
    /// `None` at genesis or when loading a snapshot that predates this field —
    /// `ratify_proposals()` falls back to live state in that case.
    #[serde(default)]
    pub ratification_snapshot: Option<RatificationSnapshot>,
}

/// Frozen ratification inputs captured at epoch boundary E.
///
/// Consumed by `ratify_proposals()` at boundary E+1 so that proposals/votes
/// submitted during epoch E are not considered until the following boundary.
/// Analogous to Haskell's `DRepPulsingState` snapshot fields (`dpProposals`,
/// `dpCommitteeState`, `dpEnactState`, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RatificationSnapshot {
    /// Proposals active at snapshot time.
    pub proposals: BTreeMap<GovActionId, ProposalState>,
    /// Votes indexed by action ID at snapshot time.
    pub votes_by_action: BTreeMap<GovActionId, Vec<(Voter, VotingProcedure)>>,
    /// Committee hot key authorizations (cold → hot) at snapshot time.
    pub committee_hot_keys: HashMap<Hash32, Hash32>,
    /// Committee member expiration epochs at snapshot time.
    pub committee_expiration: HashMap<Hash32, EpochNo>,
    /// Resigned committee members at snapshot time.
    pub committee_resigned: HashMap<Hash32, Option<Anchor>>,
    /// Committee quorum threshold at snapshot time.
    pub committee_threshold: Option<Rational>,
    /// Whether the committee was in a no-confidence state at snapshot time.
    pub no_confidence: bool,
    /// Enacted governance action roots at snapshot time (starting point for
    /// `prev_action_id` chain validation during ratification).
    pub enacted_pparam_update: Option<GovActionId>,
    pub enacted_hard_fork: Option<GovActionId>,
    pub enacted_committee: Option<GovActionId>,
    pub enacted_constitution: Option<GovActionId>,
    /// The epoch when this snapshot was captured.
    pub snapshot_epoch: EpochNo,
    /// Vote delegations (credential → DRep) at snapshot time.
    ///
    /// Used by `default_spo_vote()` during ratification to determine the
    /// implicit vote for non-voting SPOs, matching Haskell's
    /// `dpDefaultDRepVoteDelegs` captured in the DRep pulser.
    #[serde(default)]
    pub vote_delegations: HashMap<Hash32, DRep>,
}

/// Registration state for a DRep
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DRepRegistration {
    pub credential: Credential,
    pub deposit: Lovelace,
    pub anchor: Option<Anchor>,
    pub registered_epoch: EpochNo,
    /// Absolute expiry epoch for this DRep, matching Haskell's `drepExpiry`.
    ///
    /// Computed at registration/vote/update time as:
    ///   PV >= 10: `(current_epoch + drep_activity) - num_dormant_epochs`
    ///   PV <  10: `current_epoch + drep_activity` (bootstrap, dormant ignored)
    ///
    /// A DRep is expired (inactive) when `current_epoch > drep_expiry`.
    #[serde(alias = "last_active_epoch")]
    pub drep_expiry: EpochNo,
    /// Whether this DRep is currently active (per CIP-1694 activity tracking).
    /// Inactive DReps remain registered but are excluded from voting power calculations.
    #[serde(default = "default_drep_active")]
    pub active: bool,
}

fn default_drep_active() -> bool {
    true
}

impl GovernanceState {
    /// Count of DReps whose `active` flag is currently `true`.
    ///
    /// This is the number that external tools (Koios, cardano-cli) report as
    /// "registered" DReps: all DReps that have registered and whose activity
    /// window has not yet expired.  It excludes:
    ///
    /// * DReps that became inactive due to `drep_activity` epoch inactivity
    ///   (they remain in `self.dreps` with `active = false` until explicitly
    ///   deregistered via `UnregDRep`).
    ///
    /// Per CIP-1694, inactive DReps still hold their deposit and can
    /// reactivate by voting or submitting an `UpdateDRep` certificate; they
    /// are simply excluded from voting power calculations.
    pub fn active_drep_count(&self) -> usize {
        self.dreps.values().filter(|d| d.active).count()
    }
}

/// State of a governance proposal
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProposalState {
    pub procedure: ProposalProcedure,
    pub proposed_epoch: EpochNo,
    pub expires_epoch: EpochNo,
    pub yes_votes: u64,
    pub no_votes: u64,
    pub abstain_votes: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StakeDistributionState {
    pub stake_map: HashMap<Hash32, Lovelace>,
}

/// Cardano uses a "mark / set / go" snapshot model:
/// - "mark" is the snapshot taken at the current epoch boundary
/// - "set" is the snapshot from the previous epoch (used for leader election)
/// - "go" is the snapshot from two epochs ago (used for reward calculation)
///
/// Matches Haskell's `SnapShots` data type. All snapshots start as empty
/// (not None) — Haskell uses `emptySnapShots` at genesis. The `ss_fee`
/// field is separate from individual snapshots, matching Haskell's `ssFee`
/// which is set by the SNAP rule at each epoch boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpochSnapshots {
    /// Snapshot from the most recent epoch boundary ("mark")
    pub mark: Option<StakeSnapshot>,
    /// Snapshot from one epoch ago ("set") — used for leader election
    pub set: Option<StakeSnapshot>,
    /// Snapshot from two epochs ago ("go") — used for reward distribution
    pub go: Option<StakeSnapshot>,
    /// Fee pot for the next RUPD (Haskell's `ssFee`).
    #[serde(default = "default_lovelace_zero")]
    pub ss_fee: Lovelace,
    /// Block production from the previous epoch (Haskell's `nesBprev`).
    ///
    /// At each NEWEPOCH boundary: `bprev = current epoch blocks`, then
    /// counters are reset. The RUPD uses bprev for pool reward allocation.
    /// Separate from the snapshot rotation (bprev is from 1 epoch ago,
    /// while GO stake data is from 2 epochs ago).
    #[serde(default)]
    pub bprev_block_count: u64,
    #[serde(default)]
    pub bprev_blocks_by_pool: Arc<HashMap<Hash28, u64>>,
    /// Whether bprev/ss_fee have been populated by a prior snapshot rotation.
    /// False at genesis; set to true after the first rotation captures epoch
    /// data. Matches Haskell's initial `nesRu = SNothing` (no reward update
    /// to apply at the first boundary).
    #[serde(default)]
    pub rupd_ready: bool,
}

impl Default for EpochSnapshots {
    fn default() -> Self {
        EpochSnapshots {
            mark: None,
            set: None,
            go: None,
            ss_fee: Lovelace(0),
            bprev_block_count: 0,
            bprev_blocks_by_pool: Arc::new(HashMap::new()),
            rupd_ready: false,
        }
    }
}

/// Serde default helper for `Lovelace(0)` in snapshot fields.
fn default_lovelace_zero() -> Lovelace {
    Lovelace(0)
}

/// A snapshot of the stake distribution at an epoch boundary.
/// Uses `Arc` for large HashMaps to avoid deep-cloning during epoch rotation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StakeSnapshot {
    pub epoch: EpochNo,
    /// stake credential hash -> pool_id delegation
    pub delegations: Arc<HashMap<Hash32, Hash28>>,
    /// pool_id -> total active stake delegated to that pool
    pub pool_stake: HashMap<Hash28, Lovelace>,
    /// pool_id -> pool parameters at snapshot time
    pub pool_params: Arc<HashMap<Hash28, PoolRegistration>>,
    /// Individual stake per credential (for reward distribution and pledge verification)
    #[serde(default)]
    pub stake_distribution: Arc<HashMap<Hash32, Lovelace>>,
    /// Fee pot from the epoch this snapshot was captured (Haskell's _feeSS).
    /// Used by `calculate_rewards` (via the set snapshot) for RUPD deltaT1.
    #[serde(default = "default_lovelace_zero")]
    pub epoch_fees: Lovelace,
    /// Total blocks produced in the epoch this snapshot was captured.
    /// Used for eta = actual_blocks / expected_blocks in reward calculation.
    #[serde(default)]
    pub epoch_block_count: u64,
    /// Per-pool block production in the epoch this snapshot was captured.
    /// Used for apparent performance in reward calculation.
    #[serde(default)]
    pub epoch_blocks_by_pool: Arc<HashMap<Hash28, u64>>,
}

impl StakeSnapshot {
    /// Create a default (empty) snapshot for use in struct update syntax.
    pub fn empty(epoch: EpochNo) -> Self {
        StakeSnapshot {
            epoch,
            delegations: Arc::new(HashMap::new()),
            pool_stake: HashMap::new(),
            pool_params: Arc::new(HashMap::new()),
            stake_distribution: Arc::new(HashMap::new()),
            epoch_fees: Lovelace(0),
            epoch_block_count: 0,
            epoch_blocks_by_pool: Arc::new(HashMap::new()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolRegistration {
    pub pool_id: Hash28,
    pub vrf_keyhash: Hash32,
    pub pledge: Lovelace,
    pub cost: Lovelace,
    pub margin_numerator: u64,
    pub margin_denominator: u64,
    /// Reward account for pool operator rewards
    #[serde(default)]
    pub reward_account: Vec<u8>,
    /// Pool owner stake key hashes
    #[serde(default)]
    pub owners: Vec<Hash28>,
    /// Relay endpoints declared by the pool operator
    #[serde(default)]
    pub relays: Vec<Relay>,
    /// Pool metadata URL
    #[serde(default)]
    pub metadata_url: Option<String>,
    /// Pool metadata hash
    #[serde(default)]
    pub metadata_hash: Option<Hash32>,
}

impl LedgerState {
    /// Reset the ledger tip to origin, forcing a full re-replay from storage.
    /// Used when the UTxO store is empty but the ledger snapshot has state
    /// (indicating data loss from crash or session lock issues).
    pub fn reset_to_origin(&mut self) {
        self.tip = Tip::origin();
        self.epoch = EpochNo(0);
    }

    /// Compute `drepExpiry` for a DRep whose last activity is the current epoch,
    /// matching Haskell's `computeDRepExpiryVersioned` / `computeDRepExpiry`.
    ///
    /// PV >= 10: `(current_epoch + drep_activity) - num_dormant_epochs`
    /// PV <  10: `current_epoch + drep_activity`  (bootstrap — dormant ignored)
    pub fn compute_drep_expiry(&self) -> EpochNo {
        let activity = self.epochs.protocol_params.drep_activity;
        let base = self.epoch.0 + activity;
        if self.epochs.protocol_params.protocol_version_major >= 10 {
            EpochNo(base.saturating_sub(self.gov.governance.num_dormant_epochs))
        } else {
            EpochNo(base)
        }
    }

    pub fn new(params: ProtocolParameters) -> Self {
        LedgerState {
            utxo: UtxoSubState {
                utxo_set: UtxoSet::new(),
                diff_seq: DiffSeq::new(),
                epoch_fees: Lovelace(0),
                pending_donations: Lovelace(0),
            },
            certs: CertSubState {
                delegations: Arc::new(HashMap::new()),
                pool_params: Arc::new(HashMap::new()),
                future_pool_params: HashMap::new(),
                pending_retirements: HashMap::new(),
                reward_accounts: Arc::new(HashMap::new()),
                stake_key_deposits: HashMap::new(),
                pool_deposits: HashMap::new(),
                total_stake_key_deposits: 0,
                pointer_map: HashMap::new(),
                stake_distribution: StakeDistributionState::default(),
                script_stake_credentials: std::collections::HashSet::new(),
            },
            gov: GovSubState {
                governance: Arc::new(GovernanceState::default()),
            },
            consensus: ConsensusSubState {
                evolving_nonce: Hash32::ZERO,
                candidate_nonce: Hash32::ZERO,
                epoch_nonce: Hash32::ZERO,
                lab_nonce: Hash32::ZERO,
                last_epoch_block_nonce: Hash32::ZERO,
                rolling_nonce: Hash32::ZERO,
                first_block_hash_of_epoch: None,
                prev_epoch_first_block_hash: None,
                epoch_blocks_by_pool: Arc::new(HashMap::new()),
                epoch_block_count: 0,
                opcert_counters: HashMap::new(),
            },
            epochs: EpochSubState {
                snapshots: EpochSnapshots::default(),
                treasury: Lovelace(0),
                reserves: Lovelace(MAX_LOVELACE_SUPPLY),
                pending_reward_update: None,
                pending_pp_updates: BTreeMap::new(),
                future_pp_updates: BTreeMap::new(),
                needs_stake_rebuild: false,
                ptr_stake: HashMap::new(),
                ptr_stake_excluded: false,
                protocol_params: params.clone(),
                prev_protocol_params: params,
                prev_protocol_version_major: 6, // Genesis: Alonzo (proto 6)
                prev_d: 1.0,                    // Genesis: d=1
            },
            tip: Tip::origin(),
            era: Era::Conway,
            pending_era_transition: None,
            epoch: EpochNo(0),
            epoch_length: 432000,          // mainnet default
            shelley_transition_epoch: 208, // mainnet default
            byron_epoch_length: 21600,     // mainnet default (10 * 2160)
            slot_config: SlotConfig::default(),
            genesis_hash: Hash32::ZERO,
            genesis_delegates: HashMap::new(),
            update_quorum: default_update_quorum(),
            node_network: None,
            randomness_stabilisation_window: 172800, // 4k/f on mainnet: ceil(4*2160/0.05)
            stability_window_3kf: 129600,            // 3k/f on mainnet: ceil(3*2160/0.05)
            security_param: 2160,
            conway_genesis_init: None,
        }
    }

    /// Create a `LedgerState` from a decoded Haskell `ExtLedgerState` snapshot.
    ///
    /// This is the core conversion used after Mithril ancillary import to restore
    /// a correct ledger state without replaying the entire chain from genesis.
    /// Every field is mapped from the Haskell structures; genesis-derived fields
    /// (`epoch_length`, `slot_config`, etc.) are applied by the caller afterward
    /// via the usual `set_epoch_length()` / `set_slot_config()` / etc. helpers.
    ///
    /// The UTxO set is NOT populated here — the caller must load UTxOs from the
    /// tvar file separately (they are too large to carry in the state struct).
    pub fn from_haskell_snapshot(
        hs: &dugite_serialization::haskell_snapshot::types::HaskellLedgerState,
    ) -> Self {
        use dugite_serialization::haskell_snapshot::types::*;

        // ── Tip ──────────────────────────────────────────────────────────
        let tip = Tip {
            point: Point::Specific(hs.tip_slot, hs.tip_hash),
            block_number: BlockNo(hs.tip_block_no),
        };

        // ── Protocol parameters ──────────────────────────────────────────
        let cur_pparams = hs.new_epoch_state.cur_pparams.clone();
        let prev_pparams = hs.new_epoch_state.prev_pparams.clone();
        // In Conway (proto >= 9), d = 0 (fully decentralized). The prev_d
        // field is a legacy cache; safe to set to 0.0 for Conway snapshots.
        let prev_d = 0.0;
        let prev_protocol_version_major = prev_pparams.protocol_version_major;

        // ── Delegations: (tag, Hash28) → Hash32 key, Hash28 pool value ──
        let mut delegations = HashMap::new();
        let mut reward_accounts = HashMap::new();
        let mut script_stake_credentials = std::collections::HashSet::new();
        let mut total_stake_key_deposits: u64 = 0;
        let mut stake_key_deposits = HashMap::new();
        let mut vote_delegations_map = HashMap::new();

        for ((tag, hash28), account) in &hs.new_epoch_state.cert_state.dstate.accounts {
            let cred_hash = haskell_credential_to_hash32(*tag, hash28);

            // Track script credentials
            if *tag == 1 {
                script_stake_credentials.insert(cred_hash);
            }

            // Delegation
            if let Some(pool_id) = &account.pool_delegation {
                delegations.insert(cred_hash, *pool_id);
            }

            // Reward balance (include zero-balance accounts — they are registered)
            reward_accounts.insert(cred_hash, Lovelace(account.balance));

            // Per-credential deposit tracking
            if account.deposit > 0 {
                total_stake_key_deposits += account.deposit;
                stake_key_deposits.insert(cred_hash, account.deposit);
            }

            // DRep vote delegation
            if let Some(drep) = &account.drep_delegation {
                let drep_native = convert_haskell_drep(drep);
                vote_delegations_map.insert(cred_hash, drep_native);
            }
        }

        // ── Pool registrations ───────────────────────────────────────────
        let mut pool_params_map = HashMap::new();
        let mut pool_deposits = HashMap::new();
        for (pool_id, pool) in &hs.new_epoch_state.cert_state.pstate.stake_pools {
            pool_params_map.insert(*pool_id, convert_pool_registration(*pool_id, pool));
            if pool.deposit > 0 {
                pool_deposits.insert(*pool_id, pool.deposit);
            }
        }

        // ── Future pool params ───────────────────────────────────────────
        let mut future_pool_params = HashMap::new();
        for (pool_id, pool) in &hs.new_epoch_state.cert_state.pstate.future_pool_params {
            future_pool_params.insert(*pool_id, convert_pool_registration(*pool_id, pool));
        }

        // ── Pending retirements ──────────────────────────────────────────
        let pending_retirements = hs.new_epoch_state.cert_state.pstate.retirements.clone();

        // ── Genesis delegates ────────────────────────────────────────────
        let genesis_delegates = hs
            .new_epoch_state
            .cert_state
            .dstate
            .genesis_delegates
            .clone();

        // ── Opcert counters ──────────────────────────────────────────────
        let opcert_counters = hs.praos_state.opcert_counters.clone();

        // ── Epoch block production ───────────────────────────────────────
        let epoch_blocks_by_pool = hs.new_epoch_state.blocks_made_cur.clone();
        let epoch_block_count: u64 = epoch_blocks_by_pool.values().sum();

        // ── Stake snapshots ──────────────────────────────────────────────
        let mark_snapshot = convert_stake_snapshot(&hs.new_epoch_state.snapshots.mark, hs.epoch);
        let set_snapshot = convert_stake_snapshot(
            &hs.new_epoch_state.snapshots.set,
            EpochNo(hs.epoch.0.saturating_sub(1)),
        );
        let go_snapshot = convert_stake_snapshot(
            &hs.new_epoch_state.snapshots.go,
            EpochNo(hs.epoch.0.saturating_sub(2)),
        );

        // bprev = previous epoch's block production (used by RUPD)
        let bprev_blocks_by_pool = hs.new_epoch_state.blocks_made_prev.clone();
        let bprev_block_count: u64 = bprev_blocks_by_pool.values().sum();

        let snapshots = EpochSnapshots {
            mark: Some(mark_snapshot),
            set: Some(set_snapshot),
            go: Some(go_snapshot),
            ss_fee: Lovelace(hs.new_epoch_state.snapshots.fee),
            bprev_block_count,
            bprev_blocks_by_pool: Arc::new(bprev_blocks_by_pool),
            rupd_ready: true,
        };

        // ── Governance state ─────────────────────────────────────────────
        let mut gov = GovernanceState::default();

        // DRep registrations
        for ((tag, hash28), drep_state) in &hs.new_epoch_state.cert_state.vstate.dreps {
            let cred = if *tag == 0 {
                Credential::VerificationKey(*hash28)
            } else {
                Credential::Script(*hash28)
            };
            let cred_hash = cred.to_typed_hash32();
            let anchor = drep_state.anchor.as_ref().map(|(url, hash)| Anchor {
                url: url.clone(),
                data_hash: *hash,
            });
            gov.dreps.insert(
                cred_hash,
                DRepRegistration {
                    credential: cred,
                    deposit: Lovelace(drep_state.deposit),
                    anchor,
                    registered_epoch: EpochNo(0), // Not tracked in Haskell snapshot
                    drep_expiry: drep_state.expiry,
                    active: hs.epoch.0 <= drep_state.expiry.0,
                },
            );
        }

        // Vote delegations
        gov.vote_delegations = vote_delegations_map;

        // Committee state
        for ((tag, hash28), auth) in &hs.new_epoch_state.cert_state.vstate.committee_state {
            let cold_hash = haskell_credential_to_hash32(*tag, hash28);
            if *tag == 1 {
                gov.script_committee_credentials.insert(cold_hash);
            }
            match auth {
                HaskellCommitteeAuth::Hot(hot_tag, hot_hash) => {
                    let hot_h32 = haskell_credential_to_hash32(*hot_tag, hot_hash);
                    gov.committee_hot_keys.insert(cold_hash, hot_h32);
                    if *hot_tag == 1 {
                        gov.script_committee_hot_credentials.insert(hot_h32);
                    }
                }
                HaskellCommitteeAuth::Resigned(anchor) => {
                    let a = anchor.as_ref().map(|(url, hash)| Anchor {
                        url: url.clone(),
                        data_hash: *hash,
                    });
                    gov.committee_resigned.insert(cold_hash, a);
                }
            }
        }

        // Dormant epochs
        gov.num_dormant_epochs = hs.new_epoch_state.cert_state.vstate.dormant_epochs;

        // Constitution
        if let Some(ref c) = hs.new_epoch_state.gov_state.constitution {
            gov.constitution = Some(Constitution {
                anchor: Anchor {
                    url: c.anchor_url.clone(),
                    data_hash: c.anchor_hash,
                },
                script_hash: c.script_hash,
            });
        }

        // ── Build stake distribution from mark snapshot ──────────────────
        // The "instant stake" from Haskell is the authoritative source.
        let mut stake_map = HashMap::new();
        for ((tag, hash28), lovelace) in &hs.new_epoch_state.instant_stake {
            let cred_hash = haskell_credential_to_hash32(*tag, hash28);
            stake_map.insert(cred_hash, Lovelace(*lovelace));
        }

        info!(
            epoch = hs.epoch.0,
            tip_slot = hs.tip_slot.0,
            tip_block = hs.tip_block_no,
            treasury = hs.new_epoch_state.treasury,
            reserves = hs.new_epoch_state.reserves,
            delegations = delegations.len(),
            pools = pool_params_map.len(),
            reward_accounts = reward_accounts.len(),
            dreps = gov.dreps.len(),
            stake_keys = stake_map.len(),
            "Building LedgerState from Haskell snapshot"
        );

        LedgerState {
            utxo: UtxoSubState {
                utxo_set: UtxoSet::new(),
                diff_seq: DiffSeq::new(),
                epoch_fees: Lovelace(hs.new_epoch_state.fees),
                pending_donations: Lovelace(hs.new_epoch_state.donation),
            },
            certs: CertSubState {
                delegations: Arc::new(delegations),
                pool_params: Arc::new(pool_params_map),
                future_pool_params,
                pending_retirements,
                reward_accounts: Arc::new(reward_accounts),
                stake_key_deposits,
                pool_deposits,
                total_stake_key_deposits,
                pointer_map: HashMap::new(), // Conway era: pointers excluded
                stake_distribution: StakeDistributionState { stake_map },
                script_stake_credentials,
            },
            gov: GovSubState {
                governance: Arc::new(gov),
            },
            consensus: ConsensusSubState {
                evolving_nonce: hs.praos_state.evolving_nonce,
                candidate_nonce: hs.praos_state.candidate_nonce,
                epoch_nonce: hs.praos_state.epoch_nonce,
                lab_nonce: hs.praos_state.lab_nonce,
                last_epoch_block_nonce: hs.praos_state.last_epoch_block_nonce,
                rolling_nonce: Hash32::ZERO,
                first_block_hash_of_epoch: None,
                prev_epoch_first_block_hash: None,
                epoch_blocks_by_pool: Arc::new(epoch_blocks_by_pool),
                epoch_block_count,
                opcert_counters,
            },
            epochs: EpochSubState {
                snapshots,
                treasury: Lovelace(hs.new_epoch_state.treasury),
                reserves: Lovelace(hs.new_epoch_state.reserves),
                pending_reward_update: None,
                pending_pp_updates: BTreeMap::new(),
                future_pp_updates: BTreeMap::new(),
                needs_stake_rebuild: false,
                ptr_stake: HashMap::new(), // Conway: pointers excluded
                ptr_stake_excluded: true,  // Conway: already excluded
                protocol_params: cur_pparams,
                prev_protocol_params: prev_pparams,
                prev_protocol_version_major,
                prev_d,
            },
            tip,
            era: Era::Conway,
            pending_era_transition: None,
            epoch: hs.epoch,
            // Genesis-derived fields — caller applies via set_epoch_length() etc.
            epoch_length: 432000,
            shelley_transition_epoch: 0,
            byron_epoch_length: 0,
            slot_config: SlotConfig::default(), // Will be set by set_slot_config()
            genesis_hash: Hash32::ZERO,         // Will be set by set_genesis_hash()
            genesis_delegates,
            update_quorum: 5,
            node_network: None, // Will be set by caller
            // Will be recalculated by set_epoch_length()
            randomness_stabilisation_window: 0,
            stability_window_3kf: 0,
            security_param: 0,         // Will be set by set_epoch_length()
            conway_genesis_init: None, // Will be set by caller
        }
    }

    /// Set the slot configuration for Plutus time conversion
    pub fn set_slot_config(&mut self, slot_config: SlotConfig) {
        self.slot_config = slot_config;
        debug!(
            "Ledger: slot config (zero_time={}, zero_slot={}, slot_length={})",
            slot_config.zero_time, slot_config.zero_slot, slot_config.slot_length,
        );
    }

    /// Clone without UTxO data — for LedgerSeq checkpoints.
    ///
    /// Returns a LedgerState with an empty UtxoSet and DiffSeq. All non-UTxO
    /// state (delegations, pools, rewards, governance, epochs, consensus) is
    /// cloned normally. UTxO state is reconstructed from diffs during
    /// `LedgerSeq::state_at_index()`.
    pub fn clone_without_utxos(&self) -> Self {
        LedgerState {
            utxo: UtxoSubState {
                utxo_set: UtxoSet::new(),
                diff_seq: DiffSeq::new(),
                epoch_fees: self.utxo.epoch_fees,
                pending_donations: self.utxo.pending_donations,
            },
            certs: self.certs.clone(),
            gov: self.gov.clone(),
            consensus: self.consensus.clone(),
            epochs: self.epochs.clone(),
            tip: self.tip.clone(),
            era: self.era,
            pending_era_transition: self.pending_era_transition,
            epoch: self.epoch,
            epoch_length: self.epoch_length,
            shelley_transition_epoch: self.shelley_transition_epoch,
            byron_epoch_length: self.byron_epoch_length,
            slot_config: self.slot_config,
            genesis_hash: self.genesis_hash,
            genesis_delegates: self.genesis_delegates.clone(),
            update_quorum: self.update_quorum,
            node_network: self.node_network,
            randomness_stabilisation_window: self.randomness_stabilisation_window,
            stability_window_3kf: self.stability_window_3kf,
            security_param: self.security_param,
            conway_genesis_init: self.conway_genesis_init.clone(),
        }
    }

    /// Configure the epoch length (from Shelley genesis)
    pub fn set_epoch_length(&mut self, epoch_length: u64, security_param: u64) {
        self.epoch_length = epoch_length;
        self.security_param = security_param;
        // Compute BOTH stability windows:
        //   randomness_stabilisation_window = ceiling(4k/f) — Conway+ candidate freeze
        //   stability_window_3kf            = ceiling(3k/f) — Alonzo/Babbage candidate freeze
        let (f_num, f_den) = self.epochs.protocol_params.active_slot_coeff_rational();
        self.randomness_stabilisation_window =
            dugite_primitives::protocol_params::ceiling_div_by_rational(
                4,
                security_param,
                f_num,
                f_den,
            );
        self.stability_window_3kf = dugite_primitives::protocol_params::ceiling_div_by_rational(
            3,
            security_param,
            f_num,
            f_den,
        );
        debug!(
            "Ledger: epoch length={}, rsw_4kf={}, sw_3kf={}, k={}",
            epoch_length,
            self.randomness_stabilisation_window,
            self.stability_window_3kf,
            security_param,
        );
    }

    /// Configure the Byron→Shelley hard fork boundary.
    ///
    /// `shelley_transition_epoch` is the number of Byron epochs before
    /// Shelley starts (e.g. mainnet=208, guild=2, preview=0).
    /// `byron_epoch_length` is 10*k in Byron slots.
    pub fn set_shelley_transition(
        &mut self,
        shelley_transition_epoch: u64,
        byron_epoch_length: u64,
    ) {
        self.shelley_transition_epoch = shelley_transition_epoch;
        self.byron_epoch_length = byron_epoch_length;
        debug!(
            "Ledger: Shelley transition at epoch {}, byron_epoch_len={}",
            shelley_transition_epoch, byron_epoch_length,
        );
    }

    /// Compute the HFC epoch number for a given absolute slot.
    ///
    /// Uses the CNCLI formula: for slots in the Shelley era,
    /// epoch = shelley_transition_epoch + (slot - byron_slots) / epoch_length
    /// where byron_slots = byron_epoch_length * shelley_transition_epoch.
    pub fn epoch_of_slot(&self, slot: u64) -> u64 {
        let byron_slots = self
            .byron_epoch_length
            .saturating_mul(self.shelley_transition_epoch);
        if slot < byron_slots {
            // Still in Byron era
            if self.byron_epoch_length > 0 {
                slot / self.byron_epoch_length
            } else {
                0
            }
        } else {
            // Shelley era
            let shelley_slots = slot - byron_slots;
            self.shelley_transition_epoch
                .saturating_add(shelley_slots / self.epoch_length)
        }
    }

    /// Compute the first slot of the epoch that contains the given slot.
    /// Uses saturating arithmetic to prevent u64 overflow with extreme values.
    pub fn first_slot_of_epoch(&self, epoch: u64) -> u64 {
        if epoch < self.shelley_transition_epoch {
            // Byron epoch
            epoch.saturating_mul(self.byron_epoch_length)
        } else {
            // Shelley epoch
            let byron_slots = self
                .byron_epoch_length
                .saturating_mul(self.shelley_transition_epoch);
            byron_slots.saturating_add(
                (epoch - self.shelley_transition_epoch).saturating_mul(self.epoch_length),
            )
        }
    }

    /// Return the epoch nonce that should be used to verify a block at `slot`.
    ///
    /// During normal (non-epoch-crossing) processing, this is `self.epoch_nonce`.
    /// When a block is the first block of the NEXT epoch (i.e. the epoch-transition
    /// block), its VRF proof was generated with the *new* epoch's nonce, which is
    /// computed by the TICKN rule at the epoch boundary:
    ///
    ///   epochNonce' = candidateNonce ⭒ lastEpochBlockNonce
    ///
    /// The ledger does not advance the epoch nonce until `apply_block` fires
    /// `process_epoch_transition`.  Therefore, if validation runs before apply
    /// (as in the batch-then-apply pattern in `process_forward_blocks`), the
    /// first block of a new epoch would be validated against the *old* nonce,
    /// causing a spurious VRF failure that then blocks the epoch transition from
    /// ever firing — permanently stalling the node.
    ///
    /// This function pre-computes the TICKN nonce for `slot` without mutating any
    /// state, so the validation loop can inject the correct nonce before calling
    /// `validate_header_full`.
    ///
    /// For blocks in the same epoch as the current ledger state, the existing
    /// `self.epoch_nonce` is returned directly.  For blocks exactly one epoch
    /// ahead, the next-epoch nonce is computed from `candidate_nonce` and
    /// `last_epoch_block_nonce`.  For blocks more than one epoch ahead (which
    /// should not occur at tip), `self.epoch_nonce` is returned as a fallback
    /// (VRF verification will fail non-fatally in non-strict mode, or produce
    /// an informative error in strict mode).
    pub fn epoch_nonce_for_slot(&self, slot: u64) -> Hash32 {
        let block_epoch = self.epoch_of_slot(slot);
        if block_epoch <= self.epoch.0 {
            // Same epoch (or behind, should not happen at tip): use current nonce.
            return self.consensus.epoch_nonce;
        }
        if block_epoch == self.epoch.0.saturating_add(1) {
            // Block is in the immediately following epoch.  Pre-compute the TICKN
            // nonce: epochNonce' = candidate ⭒ lastEpochBlockNonce.
            // This mirrors process_epoch_transition Step 1 exactly.
            let candidate = self.consensus.candidate_nonce;
            let prev_hash_nonce = self.consensus.last_epoch_block_nonce;
            let zero = Hash32::ZERO;
            return if candidate == zero && prev_hash_nonce == zero {
                zero
            } else if candidate == zero {
                prev_hash_nonce
            } else if prev_hash_nonce == zero {
                candidate
            } else {
                let mut buf = Vec::with_capacity(64);
                buf.extend_from_slice(candidate.as_bytes());
                buf.extend_from_slice(prev_hash_nonce.as_bytes());
                dugite_primitives::hash::blake2b_256(&buf)
            };
        }
        // Block is more than one epoch ahead.  We cannot pre-compute the nonce
        // because the intermediate epochs' VRF contributions are unknown.
        // Return the current nonce; validation will fail non-fatally (or produce
        // an informative error), and the node will retry after catching up.
        self.consensus.epoch_nonce
    }

    /// Set the Shelley genesis hash.
    ///
    /// Initializes the Praos nonce state machine per Haskell's initialChainDepState
    /// (cardano-protocol-tpraos/API.hs) and translateChainDepStateByronToShelley:
    ///
    ///   evolvingNonce       = initNonce  (= Blake2b_256 of genesis file)
    ///   candidateNonce      = initNonce
    ///   epochNonce          = initNonce
    ///   labNonce            = NeutralNonce
    ///   lastEpochBlockNonce = NeutralNonce
    ///
    /// At the first epoch boundary, the Nonce combine with NeutralNonce is identity:
    ///   epochNonce' = candidateNonce ⭒ NeutralNonce = candidateNonce
    /// This means the first epoch transition preserves the candidate nonce directly
    /// rather than hashing it with a non-zero lastEpochBlockNonce.
    pub fn set_genesis_hash(&mut self, hash: Hash32) {
        self.genesis_hash = hash;
        // evolving/candidate/epoch all start from the genesis file hash
        self.consensus.evolving_nonce = hash;
        self.consensus.candidate_nonce = hash;
        self.consensus.epoch_nonce = hash;
        // lab and lastEpochBlockNonce start as NeutralNonce (ZERO)
        // This is critical: at the first epoch boundary, NeutralNonce identity
        // means epochNonce = candidateNonce (not hash(candidate || genesisHash))
        self.consensus.lab_nonce = Hash32::ZERO;
        self.consensus.last_epoch_block_nonce = Hash32::ZERO;
        info!(
            epoch_nonce = %hash.to_hex(),
            evolving = %hash.to_hex(),
            candidate = %hash.to_hex(),
            lab = "NeutralNonce (ZERO)",
            last_epoch_block = "NeutralNonce (ZERO)",
            "Ledger: Praos nonce state initialized from genesis hash"
        );
    }

    /// Set the update quorum threshold (from Shelley genesis)
    pub fn set_update_quorum(&mut self, quorum: u64) {
        self.update_quorum = quorum;
        debug!("Ledger: update quorum={quorum}");
    }

    /// Load genesis delegates from Shelley genesis data.
    ///
    /// Each entry is (genesis_key_hash_28, delegate_key_hash_28, vrf_key_hash_32)
    /// as raw bytes. Called during node initialization from `ShelleyGenesis::gen_delegs_entries()`.
    pub fn set_genesis_delegates(&mut self, entries: &[(Vec<u8>, Vec<u8>, Vec<u8>)]) {
        self.genesis_delegates.clear();
        for (genesis_hash, delegate_hash, vrf_hash) in entries {
            if genesis_hash.len() == 28 && delegate_hash.len() == 28 && vrf_hash.len() == 32 {
                let gkey = Hash28::from_bytes({
                    let mut buf = [0u8; 28];
                    buf.copy_from_slice(genesis_hash);
                    buf
                });
                let dkey = Hash28::from_bytes({
                    let mut buf = [0u8; 28];
                    buf.copy_from_slice(delegate_hash);
                    buf
                });
                let vrf = Hash32::from_bytes({
                    let mut buf = [0u8; 32];
                    buf.copy_from_slice(vrf_hash);
                    buf
                });
                self.genesis_delegates.insert(gkey, (dkey, vrf));
            }
        }
    }

    /// Seed the UTxO set with genesis UTxOs (from Byron genesis nonAvvmBalances).
    ///
    /// Each genesis UTxO is assigned a deterministic transaction hash derived from
    /// blake2b-256 of the address bytes, with sequential output indices.
    /// This MUST be called before replaying blocks from genesis.
    pub fn seed_genesis_utxos(&mut self, entries: &[(Vec<u8>, u64)]) {
        let mut seeded = 0u64;
        let mut total_lovelace = 0u64;

        for (address, lovelace) in entries {
            if *lovelace == 0 {
                continue;
            }

            // Derive a deterministic tx hash from the address (matches Byron genesis UTxO format)
            let tx_hash = dugite_primitives::hash::blake2b_256(address);

            let input = dugite_primitives::transaction::TransactionInput {
                transaction_id: tx_hash,
                index: 0,
            };

            // Parse the address bytes, or fall back to Byron if parsing fails
            let addr = dugite_primitives::Address::from_bytes(address).unwrap_or(
                dugite_primitives::Address::Byron(dugite_primitives::address::ByronAddress {
                    payload: address.clone(),
                }),
            );

            let output = dugite_primitives::transaction::TransactionOutput {
                address: addr,
                value: dugite_primitives::value::Value {
                    coin: Lovelace(*lovelace),
                    multi_asset: std::collections::BTreeMap::new(),
                },
                datum: dugite_primitives::transaction::OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            };

            self.utxo.utxo_set.insert(input, output);
            seeded += 1;
            total_lovelace += lovelace;
        }

        // Deduct seeded lovelace from reserves per Shelley spec:
        // reserves = maxLovelaceSupply - totalBalance(initialUTxO)
        // Without this, monetary expansion (rho * reserves) is computed on too
        // large a reserves value, draining reserves too fast and overfilling
        // the treasury.
        self.epochs.reserves.0 = self.epochs.reserves.0.saturating_sub(total_lovelace);

        debug!(
            "Ledger: seeded {} genesis UTxOs ({} lovelace, reserves now {})",
            seeded, total_lovelace, self.epochs.reserves.0
        );
    }

    /// Seed a genesis pool registration into the ledger state.
    ///
    /// Inserts the pool registration into `pool_params` and registers
    /// the reward account with zero balance.
    pub fn seed_genesis_pool(&mut self, registration: PoolRegistration) {
        let pool_id = registration.pool_id;
        let reward_account = registration.reward_account.clone();

        let pool_params = Arc::make_mut(&mut self.certs.pool_params);
        pool_params.insert(pool_id, registration);

        // Register reward account with zero balance if not already present
        if reward_account.len() >= 29 {
            // Extract the 28-byte credential hash from the reward address
            // (byte 0 is the header, bytes 1-28 are the credential)
            let mut cred = [0u8; 32];
            cred[..28].copy_from_slice(&reward_account[1..29]);
            let cred_hash = Hash32::from_bytes(cred);
            let reward_accounts = Arc::make_mut(&mut self.certs.reward_accounts);
            reward_accounts.entry(cred_hash).or_insert(Lovelace(0));
        }

        debug!("Ledger: seeded genesis pool {}", pool_id.to_hex());
    }

    /// Seed a genesis stake delegation into the ledger state.
    ///
    /// Maps a stake credential (as padded Hash32) to a pool ID (Hash28).
    /// Registers the credential in reward accounts with zero balance.
    pub fn seed_genesis_delegation(&mut self, stake_credential: Hash32, pool_id: Hash28) {
        let delegations = Arc::make_mut(&mut self.certs.delegations);
        delegations.insert(stake_credential, pool_id);

        // Register stake credential in reward accounts if not present
        let reward_accounts = Arc::make_mut(&mut self.certs.reward_accounts);
        reward_accounts
            .entry(stake_credential)
            .or_insert(Lovelace(0));
    }

    /// Finalize genesis state for cold-start block production.
    ///
    /// Mirrors Haskell's `resetStakeDistribution` (cardano-ledger
    /// `Shelley/Transition.hs`): after `seed_genesis_utxos`, `seed_genesis_pool`,
    /// and `seed_genesis_delegation` have been called, builds the initial
    /// stake/pool distribution and pre-populates the `mark` and `set`
    /// snapshots so that Praos leader election works from slot 0.
    ///
    /// In Haskell, leader election reads `nesPd` (active pool distribution),
    /// which `resetStakeDistribution` fills with the post-genesis pool stake.
    /// Dugite's forge path uses `snapshots.set` for the same purpose, so we
    /// populate both `mark` and `set` with the same genesis-derived data —
    /// the first SNAP rotation at epoch 0→1 preserves the same pool stake
    /// into `go`, matching Haskell's observable behaviour on a quiet devnet.
    ///
    /// No-op on Mithril-restored state, where snapshots are already loaded
    /// from the Haskell snapshot file.
    pub fn finalize_genesis_state(&mut self) {
        use tracing::info;

        // If a snapshot is already present (Mithril restore path), do nothing.
        if self.epochs.snapshots.set.is_some() || self.epochs.snapshots.mark.is_some() {
            return;
        }

        // Build stake_map from seeded UTxOs so pool_stake can be computed.
        self.rebuild_stake_distribution();

        // Build pool_stake and snapshot_stake exactly as the SNAP rule does
        // for the mark snapshot at an epoch boundary.
        let mut pool_stake: HashMap<Hash28, Lovelace> =
            HashMap::with_capacity(self.certs.pool_params.len());
        let mut snapshot_stake: HashMap<Hash32, Lovelace> =
            HashMap::with_capacity(self.certs.delegations.len());
        for (cred_hash, pool_id) in self.certs.delegations.iter() {
            let utxo_stake = self
                .certs
                .stake_distribution
                .stake_map
                .get(cred_hash)
                .copied()
                .unwrap_or(Lovelace(0));
            let reward_balance = self
                .certs
                .reward_accounts
                .get(cred_hash)
                .copied()
                .unwrap_or(Lovelace(0));
            let total = Lovelace(utxo_stake.0.saturating_add(reward_balance.0));
            if total.0 > 0 {
                snapshot_stake.insert(*cred_hash, total);
                *pool_stake.entry(*pool_id).or_insert(Lovelace(0)) += total;
            }
        }

        if pool_stake.is_empty() {
            // No pools registered or no delegated stake yet — nothing to snapshot.
            return;
        }

        let total_pool_stake: u64 = pool_stake
            .values()
            .fold(0u64, |acc, l| acc.saturating_add(l.0));
        info!(
            pools = pool_stake.len(),
            delegations = self.certs.delegations.len(),
            total_pool_stake_ada = total_pool_stake / 1_000_000,
            "Genesis: seeded initial stake/pool snapshot for cold-start leader election"
        );

        let snap = StakeSnapshot {
            epoch: self.epoch,
            delegations: Arc::clone(&self.certs.delegations),
            pool_stake,
            pool_params: Arc::clone(&self.certs.pool_params),
            stake_distribution: Arc::new(snapshot_stake),
            epoch_fees: Lovelace(0),
            epoch_block_count: 0,
            epoch_blocks_by_pool: Arc::new(HashMap::new()),
        };

        self.epochs.snapshots.mark = Some(snap.clone());
        self.epochs.snapshots.set = Some(snap);
    }

    /// Advance the ledger tip through a Byron Epoch Boundary Block (EBB).
    ///
    /// EBBs carry no transactions and do not mutate the UTxO set, stake
    /// distribution, or any other ledger data.  They exist solely so that
    /// the next real Byron block can reference them via `prev_hash`, forming
    /// an unbroken hash chain across epoch boundaries.
    ///
    /// This method advances `self.tip` so that the EBB hash becomes the
    /// current tip hash, allowing the subsequent block's `prev_hash` check
    /// to pass.  The slot is preserved from the previous real block because
    /// EBBs do not occupy slots — this prevents incorrect "block already
    /// applied" skips in the sync loop which compares `block.slot <= ledger_slot`.
    ///
    /// # Errors
    /// Returns `LedgerError::EpochTransition` if called outside the Byron era,
    /// since EBBs do not exist in Shelley or later eras.
    pub fn advance_past_ebb(&mut self, ebb_hash: Hash32) -> Result<(), LedgerError> {
        use dugite_primitives::era::Era;

        // EBBs only exist in the Byron era.  Calling this in Shelley+ is a programming error.
        if self.era != Era::Byron {
            return Err(LedgerError::EpochTransition(format!(
                "EBB advance called in non-Byron era {:?}; EBBs do not exist after Byron",
                self.era
            )));
        }

        // Preserve the slot of the current tip.  The EBB has no slot of its
        // own; by keeping the previous slot we ensure the next real block's slot
        // satisfies `slot > ledger_slot` so it is not incorrectly skipped.
        let preserved_slot = self.tip.point.slot().unwrap_or(SlotNo(0));

        trace!(
            ebb_hash = %ebb_hash.to_hex(),
            preserved_slot = preserved_slot.0,
            current_tip = %self.tip.point,
            "Ledger: advancing tip through EBB"
        );

        // Advance the tip hash to the EBB hash while keeping the slot from the
        // previous block.  Block number is also preserved since EBBs do not
        // increment the block counter.
        self.tip = Tip {
            point: Point::Specific(preserved_slot, ebb_hash),
            block_number: self.tip.block_number,
        };

        Ok(())
    }

    pub fn current_slot(&self) -> Option<SlotNo> {
        self.tip.point.slot()
    }

    pub fn current_block_number(&self) -> BlockNo {
        self.tip.block_number
    }

    /// Roll back the ledger UTxO set by unapplying the last `n` block diffs.
    ///
    /// This is the fast rollback path: it avoids a full snapshot reload + replay
    /// by directly inverting the UTxO changes recorded during `apply_block`.
    ///
    /// For each rolled-back block (most-recent first):
    ///   - Remove every UTxO that was **inserted** by that block (undo outputs)
    ///   - Re-insert every UTxO that was **deleted** by that block (restore spent inputs)
    ///
    /// The `tip` is updated to the slot/hash of the new head after undo.
    ///
    /// Returns the number of diffs actually unapplied (may be less than `n` when
    /// fewer diffs are available in the window, e.g. after a snapshot load).
    ///
    /// # Limitations
    /// - Does **not** undo epoch-transition effects (rewards, pool retirements,
    ///   snapshot rotations) because rollbacks are bounded to the volatile window
    ///   which is always within a single epoch in normal operation.
    /// - Does **not** undo `stake_distribution` changes.  After rollback the stake
    ///   distribution may be slightly stale until the next block is applied, which
    ///   is acceptable since it is not used for consensus-critical decisions.
    /// - The tip is set to the oldest slot in the diffs that were NOT rolled back.
    ///   Callers that need an exact tip hash after rollback (e.g. for the next
    ///   block's prev_hash check) must supply `rollback_to_tip` via
    ///   `rollback_blocks_to_point`.
    /// # Deprecation
    ///
    /// This method only restores UTxO changes — nonces, delegations, rewards,
    /// governance, and epoch state are NOT rolled back.  Use
    /// `LedgerSeq::rollback()` + `tip_state()` instead, which correctly
    /// restores ALL state fields.  See issue #308.
    #[deprecated(
        note = "UTxO-only rollback is incorrect. Use LedgerSeq::rollback() instead (#308)"
    )]
    pub fn rollback_blocks(&mut self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }

        // Pop the last n diffs from the sequence (most-recent first).
        let diffs = self.utxo.diff_seq.rollback(n);
        let actually_rolled = diffs.len();

        for (_slot, _hash, diff) in &diffs {
            // Undo inserts: remove UTxOs that were created by this block.
            for (input, _output) in &diff.inserts {
                self.utxo.utxo_set.remove(input);
            }
            // Undo deletes: restore UTxOs that were consumed by this block.
            for (input, output) in &diff.deletes {
                self.utxo.utxo_set.insert(input.clone(), output.clone());
            }
        }

        actually_rolled
    }

    /// Roll back the ledger UTxO set and update the tip to a specific point.
    ///
    /// Combines `rollback_blocks(n)` with an explicit tip update.  Used by
    /// `handle_rollback` when the rollback target point is known.
    ///
    /// `n` is the number of blocks to undo (determined by caller from DiffSeq
    /// contents).  `new_tip` is the `Tip` the ledger should report after undo.
    #[deprecated(
        note = "UTxO-only rollback is incorrect. Use LedgerSeq::rollback() instead (#308)"
    )]
    pub fn rollback_blocks_to_point(&mut self, n: usize, new_tip: Tip) -> usize {
        #[allow(deprecated)]
        let rolled = self.rollback_blocks(n);
        if rolled > 0 {
            self.tip = new_tip;
        }
        rolled
    }
}

// ── Haskell snapshot conversion helpers ─────────��────────────────────────────

/// Convert a Haskell credential `(tag, Hash28)` to dugite's `Hash32` key format.
///
/// Matches `Credential::to_typed_hash32()`: the 28-byte hash occupies bytes [0..28],
/// byte 28 is `0x01` for script credentials (tag=1), `0x00` for key credentials (tag=0).
fn haskell_credential_to_hash32(tag: u8, hash: &Hash28) -> Hash32 {
    let mut bytes = [0u8; 32];
    bytes[..28].copy_from_slice(hash.as_bytes());
    if tag == 1 {
        bytes[28] = 0x01;
    }
    Hash32::from_bytes(bytes)
}

/// Convert a Haskell `HaskellStakePoolState` to dugite's `PoolRegistration`.
fn convert_pool_registration(
    pool_id: Hash28,
    pool: &dugite_serialization::haskell_snapshot::types::HaskellStakePoolState,
) -> PoolRegistration {
    use dugite_primitives::transaction::Relay;
    use dugite_serialization::haskell_snapshot::types::HaskellRelay;

    let relays: Vec<Relay> = pool
        .relays
        .iter()
        .map(|r| match r {
            HaskellRelay::SingleHostAddr(port, ipv4, ipv6) => Relay::SingleHostAddr {
                port: *port,
                ipv4: *ipv4,
                ipv6: *ipv6,
            },
            HaskellRelay::SingleHostName(port, dns) => Relay::SingleHostName {
                port: *port,
                dns_name: dns.clone(),
            },
            HaskellRelay::MultiHostName(dns) => Relay::MultiHostName {
                dns_name: dns.clone(),
            },
        })
        .collect();

    let (metadata_url, metadata_hash) = match &pool.metadata {
        Some((url, hash)) => (Some(url.clone()), Some(*hash)),
        None => (None, None),
    };

    PoolRegistration {
        pool_id,
        vrf_keyhash: pool.vrf_hash,
        pledge: Lovelace(pool.pledge),
        cost: Lovelace(pool.cost),
        margin_numerator: pool.margin_num,
        margin_denominator: pool.margin_den,
        reward_account: pool.reward_account.clone(),
        owners: pool.owners.clone(),
        relays,
        metadata_url,
        metadata_hash,
    }
}

/// Convert a Haskell `HaskellSnapShot` to dugite's `StakeSnapshot`.
fn convert_stake_snapshot(
    snap: &dugite_serialization::haskell_snapshot::types::HaskellSnapShot,
    epoch: EpochNo,
) -> StakeSnapshot {
    let mut delegations = HashMap::new();
    let mut stake_distribution = HashMap::new();
    let mut pool_stake: HashMap<Hash28, Lovelace> = HashMap::new();

    // Convert delegations and per-credential stake
    for ((tag, hash28), pool_id) in &snap.delegations {
        let cred_hash = haskell_credential_to_hash32(*tag, hash28);
        delegations.insert(cred_hash, *pool_id);
    }
    for ((tag, hash28), lovelace) in &snap.stake {
        let cred_hash = haskell_credential_to_hash32(*tag, hash28);
        stake_distribution.insert(cred_hash, Lovelace(*lovelace));

        // Accumulate pool stake from delegations
        if let Some(pool_id) = delegations.get(&cred_hash) {
            *pool_stake.entry(*pool_id).or_insert(Lovelace(0)) += Lovelace(*lovelace);
        }
    }

    // Convert pool params within the snapshot
    let mut snapshot_pool_params = HashMap::new();
    for (pool_id, pool) in &snap.pool_params {
        snapshot_pool_params.insert(*pool_id, convert_snapshot_pool_registration(*pool_id, pool));
    }

    StakeSnapshot {
        epoch,
        delegations: Arc::new(delegations),
        pool_stake,
        pool_params: Arc::new(snapshot_pool_params),
        stake_distribution: Arc::new(stake_distribution),
        epoch_fees: Lovelace(0), // Not stored per-snapshot in Haskell
        epoch_block_count: 0,    // Not stored per-snapshot in Haskell
        epoch_blocks_by_pool: Arc::new(HashMap::new()),
    }
}

/// Convert a Haskell `HaskellSnapShotPool` to dugite's `PoolRegistration`.
fn convert_snapshot_pool_registration(
    pool_id: Hash28,
    pool: &dugite_serialization::haskell_snapshot::types::HaskellSnapShotPool,
) -> PoolRegistration {
    use dugite_primitives::transaction::Relay;
    use dugite_serialization::haskell_snapshot::types::HaskellRelay;

    let relays: Vec<Relay> = pool
        .relays
        .iter()
        .map(|r| match r {
            HaskellRelay::SingleHostAddr(port, ipv4, ipv6) => Relay::SingleHostAddr {
                port: *port,
                ipv4: *ipv4,
                ipv6: *ipv6,
            },
            HaskellRelay::SingleHostName(port, dns) => Relay::SingleHostName {
                port: *port,
                dns_name: dns.clone(),
            },
            HaskellRelay::MultiHostName(dns) => Relay::MultiHostName {
                dns_name: dns.clone(),
            },
        })
        .collect();

    let (metadata_url, metadata_hash) = match &pool.metadata {
        Some((url, hash)) => (Some(url.clone()), Some(*hash)),
        None => (None, None),
    };

    PoolRegistration {
        pool_id,
        vrf_keyhash: pool.vrf_hash,
        pledge: Lovelace(pool.pledge),
        cost: Lovelace(pool.cost),
        margin_numerator: pool.margin_num,
        margin_denominator: pool.margin_den,
        reward_account: pool.reward_account.clone(),
        owners: pool.owners.clone(),
        relays,
        metadata_url,
        metadata_hash,
    }
}

/// Convert a Haskell `HaskellDRep` to dugite's native `DRep`.
fn convert_haskell_drep(drep: &dugite_serialization::haskell_snapshot::types::HaskellDRep) -> DRep {
    use dugite_serialization::haskell_snapshot::types::HaskellDRep;
    match drep {
        HaskellDRep::KeyHash(h) => DRep::KeyHash(h.to_hash32_padded()),
        HaskellDRep::ScriptHash(h) => DRep::ScriptHash(*h),
        HaskellDRep::AlwaysAbstain => DRep::Abstain,
        HaskellDRep::AlwaysNoConfidence => DRep::NoConfidence,
    }
}

/// Extract a Hash32 from a Credential for use as a map key.
///
/// Uses `to_typed_hash32()` which encodes the credential TYPE (key vs script)
/// in byte 28 of the padding. This ensures key and script credentials with
/// the same 28-byte hash are stored as separate entries, matching Haskell's
/// `KeyHashObj` / `ScriptHashObj` distinction.
fn credential_to_hash(credential: &Credential) -> Hash32 {
    credential.to_typed_hash32()
}

/// Extract the staking credential hash from an address.
///
/// Handles Base addresses (embedded credential), Reward addresses, and
/// Pointer addresses (resolved via the pointer_map, matching Haskell's
/// DState ptrs). Returns None for Enterprise and Byron addresses.
///
/// In Conway (protocol version >= 9), pointer addresses are excluded from the
/// stake distribution — Haskell's `ConwayInstantStake` has no `sisPtrStake`
/// field and `addConwayInstantStake` returns `ans` unchanged for pointer
/// addresses.  When `exclude_ptrs` is true, pointer addresses return `None`.
/// The stake routing outcome for a UTxO output address.
///
/// Haskell's `ShelleyInstantStake` tracks pointer-addressed UTxO coins separately
/// in `sisPtrStake` and defers their resolution to SNAP time.  Base/Reward addresses
/// go directly into `sisCredentialStake` (our `stake_map`).  In Conway,
/// `ConwayInstantStake` omits pointer stake entirely.
enum StakeRouting {
    /// Credential hash — route coins to `stake_distribution.stake_map`.
    Credential(Hash32),
    /// Pointer key — route coins to `ptr_stake` (deferred resolution at SNAP time).
    Pointer(dugite_primitives::credentials::Pointer),
    /// No stake routing (Enterprise / Byron / unknown).
    None,
}

/// Classify a UTxO address into its stake-routing bucket.
///
/// * Base / Reward  → `StakeRouting::Credential` (eager resolution)
/// * Pointer        → `StakeRouting::Pointer` (deferred — key stored in `ptr_stake`)
/// * Everything else → `StakeRouting::None`
///
/// When `exclude_ptrs` is true (Conway era), pointer addresses return
/// `StakeRouting::None` — they are silently excluded as in `ConwayInstantStake`.
fn stake_routing(
    address: &dugite_primitives::address::Address,
    exclude_ptrs: bool,
) -> StakeRouting {
    use dugite_primitives::address::Address;
    match address {
        Address::Base(base) => StakeRouting::Credential(credential_to_hash(&base.stake)),
        Address::Reward(reward) => StakeRouting::Credential(credential_to_hash(&reward.stake)),
        Address::Pointer(ptr_addr) => {
            if exclude_ptrs {
                StakeRouting::None
            } else {
                StakeRouting::Pointer(ptr_addr.pointer)
            }
        }
        _ => StakeRouting::None,
    }
}

/// Legacy: Extract staking credential hash without pointer resolution.
/// Used in contexts where the pointer_map isn't available.
#[cfg(test)]
fn stake_credential_hash(address: &dugite_primitives::address::Address) -> Option<Hash32> {
    use dugite_primitives::address::Address;
    match address {
        Address::Base(base) => Some(credential_to_hash(&base.stake)),
        Address::Reward(reward) => Some(credential_to_hash(&reward.stake)),
        _ => None,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LedgerError {
    #[error("Block does not connect to tip: expected {expected}, got {got}")]
    BlockDoesNotConnect { expected: String, got: String },
    #[error("UTxO error: {0}")]
    UtxoError(String),
    #[error("Invalid transaction: {0}")]
    InvalidTransaction(String),
    #[error("Epoch transition error: {0}")]
    EpochTransition(String),
    #[error("Invalid protocol parameter: {0}")]
    InvalidProtocolParam(String),
    #[error("Validation tag mismatch for tx {tx_hash}: block flag is_valid={block_flag} but evaluation result is_valid={eval_result}")]
    ValidationTagMismatch {
        tx_hash: String,
        block_flag: bool,
        eval_result: bool,
    },
    #[error("Transaction validation failed at slot {slot} tx {tx_hash}: {errors}")]
    BlockTxValidationFailed {
        slot: u64,
        tx_hash: String,
        errors: String,
    },
    #[error("Block body size mismatch: actual serialized size {actual} != header claimed size {claimed} (WrongBlockBodySizeBBODY)")]
    WrongBlockBodySize { actual: u64, claimed: u64 },
}

#[cfg(test)]
mod tests;
