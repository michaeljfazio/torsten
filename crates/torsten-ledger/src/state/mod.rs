mod apply;
mod certificates;
mod epoch;
mod governance;
mod protocol_params;
mod rewards;
mod snapshot;

// Re-export governance free functions and types for use by tests
#[cfg(test)]
pub(crate) use governance::{
    check_cc_approval, check_threshold, gov_action_priority, is_delaying_action,
    modified_pp_groups, pp_change_drep_all_groups_met, pp_change_drep_threshold,
    pp_change_spo_threshold, prev_action_as_expected, DRepPPGroup, StakePoolPPGroup,
};
#[doc(hidden)]
pub use rewards::Rat;

use crate::plutus::SlotConfig;
use crate::utxo::UtxoSet;
use crate::utxo_diff::DiffSeq;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
#[cfg(test)]
use torsten_primitives::block::Block;
use torsten_primitives::block::{Point, Tip};
use torsten_primitives::credentials::Credential;
use torsten_primitives::era::Era;
use torsten_primitives::hash::{Hash28, Hash32};
use torsten_primitives::protocol_params::ProtocolParameters;
use torsten_primitives::time::{BlockNo, EpochNo, SlotNo};
use torsten_primitives::transaction::{
    Anchor, Constitution, DRep, GovActionId, ProposalProcedure, ProtocolParamUpdate, Rational,
    Relay, Voter, VotingProcedure,
};
use torsten_primitives::value::Lovelace;
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

/// The complete ledger state.
///
/// Large collections (`delegations`, `pool_params`, `reward_accounts`,
/// `governance`, `epoch_blocks_by_pool`) are wrapped in `Arc` for
/// copy-on-write semantics.  Cloning a `LedgerState` is therefore cheap:
/// it only bumps reference counts instead of deep-copying megabytes of
/// data.  Mutations go through `Arc::make_mut()`, which clones the inner
/// collection only when there are other outstanding references.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerState {
    /// Current UTxO set
    pub utxo_set: UtxoSet,
    /// Current tip of the chain
    pub tip: Tip,
    /// Current era
    pub era: Era,
    /// Current epoch
    pub epoch: EpochNo,
    /// Shelley epoch length in slots
    pub epoch_length: u64,
    /// Number of Byron epochs before the Shelley hard fork.
    /// Total Byron slots = byron_epoch_length * shelley_transition_epoch.
    #[serde(default)]
    pub shelley_transition_epoch: u64,
    /// Byron epoch length in slots (10 * k). 0 = mainnet default (21600).
    #[serde(default)]
    pub byron_epoch_length: u64,
    /// Current protocol parameters (curPParams in Haskell).
    pub protocol_params: ProtocolParameters,
    /// Previous epoch's protocol parameters (Haskell's prevPParams).
    ///
    /// Haskell's NEWPP rule: prevPParams = old curPParams (BEFORE each PPUP).
    /// The RUPD uses prevPParams for ALL parameter values: rho, tau, a0, n_opt,
    /// active_slot_coeff, d (via ppDG), protocol_version, etc.
    ///
    /// Updated at each epoch boundary: prev_protocol_params = curPP before PPUP.
    /// At genesis: initialized to the same as protocol_params.
    #[serde(default = "default_prev_protocol_params")]
    pub prev_protocol_params: ProtocolParameters,
    /// Cached prev_d for backward compatibility and serde.
    /// Derived from prev_protocol_params at each boundary.
    #[serde(default = "default_d_one")]
    pub prev_d: f64,
    /// Cached prev protocol major version for backward compatibility.
    #[serde(default = "default_prev_proto_major")]
    pub prev_protocol_version_major: u64,
    /// Stake distribution
    pub stake_distribution: StakeDistributionState,
    /// Treasury balance
    pub treasury: Lovelace,
    /// Pending treasury donations (Conway `TreasuryDonation` field from transaction bodies).
    ///
    /// In Haskell, `curTreasuryDonation` is accumulated in `UTxOState.utxosDonation` during
    /// block processing and flushed into the treasury at each epoch boundary (NEWEPOCH rule,
    /// step `applyRUpd`).  We mirror this by buffering here and draining in
    /// `process_epoch_transition` before reward computation so that the treasury includes
    /// donations from the epoch that just ended, matching Haskell's ordering exactly.
    ///
    /// The field must use a custom serde default so that ledger snapshots written before this
    /// field was added deserialise correctly (missing field → `Lovelace(0)`).
    #[serde(default = "default_lovelace_zero")]
    pub pending_donations: Lovelace,
    /// Reserves balance (ADA not yet in circulation)
    pub reserves: Lovelace,
    /// Delegation state: credential_hash -> pool_id (Arc for copy-on-write)
    pub delegations: Arc<HashMap<Hash32, Hash28>>,
    /// Pool registrations: pool_id -> pool registration (Arc for copy-on-write)
    pub pool_params: Arc<HashMap<Hash28, PoolRegistration>>,
    /// Future pool parameters for re-registrations (Haskell's futurePoolParams).
    ///
    /// In Cardano, pool re-registrations take effect at the NEXT epoch boundary.
    /// When a pool that is already registered submits a new PoolRegistration
    /// certificate, the new parameters are stored here and applied during
    /// the next epoch transition's POOLREAP step (before retirement processing).
    /// First registrations go directly to pool_params (active at epoch N+2).
    #[serde(default)]
    pub future_pool_params: HashMap<Hash28, PoolRegistration>,
    /// Pool retirements pending: pool → retirement epoch.
    /// Matches Haskell's `psRetiring :: Map (KeyHash StakePool) EpochNo`.
    /// A pool can only have ONE pending retirement; a new retirement for the
    /// same pool replaces the previous entry.
    pub pending_retirements: HashMap<Hash28, EpochNo>,
    /// Stake snapshots for the Cardano "mark/set/go" snapshot model
    pub snapshots: EpochSnapshots,
    /// Reward accounts: stake credential hash -> accumulated rewards (Arc for copy-on-write)
    pub reward_accounts: Arc<HashMap<Hash32, Lovelace>>,
    /// Pointer map: certificate pointers → credential hashes (Haskell's DState ptrs).
    ///
    /// When a StakeRegistration certificate is processed at (slot, tx_index, cert_index),
    /// it creates a pointer entry mapping to the credential hash. Pointer addresses
    /// (type 4/5) reference these pointers instead of embedding the credential directly.
    /// Used by `stake_credential_hash` to resolve Pointer addresses.
    #[serde(default)]
    pub pointer_map: HashMap<torsten_primitives::credentials::Pointer, Hash32>,
    /// Fees collected in the current epoch
    pub epoch_fees: Lovelace,
    /// Number of blocks produced by each pool in the current epoch (Arc for copy-on-write)
    pub epoch_blocks_by_pool: Arc<HashMap<Hash28, u64>>,
    /// Total blocks in the current epoch
    pub epoch_block_count: u64,
    /// Evolving nonce (eta_v): accumulated hash of ALL VRF outputs (never reset).
    /// Matches Haskell's `praosStateEvolvingNonce`.
    pub evolving_nonce: Hash32,
    /// Candidate nonce: snapshot of evolving_nonce that freezes in the last
    /// randomness_stabilisation_window (4k/f) slots of each epoch.
    /// Matches Haskell's `praosStateCandidateNonce`.
    pub candidate_nonce: Hash32,
    /// Current epoch nonce: hash(candidate_nonce || last_epoch_block_nonce) at epoch boundary.
    /// Matches Haskell's `praosStateEpochNonce`.
    pub epoch_nonce: Hash32,
    /// LAB nonce: prev_hash of the most recent block (type-cast, no hashing).
    /// Matches Haskell's `praosStateLabNonce`.
    pub lab_nonce: Hash32,
    /// Snapshot of lab_nonce at epoch boundary.
    /// Matches Haskell's `praosStateLastEpochBlockNonce`.
    pub last_epoch_block_nonce: Hash32,
    /// Randomness stabilisation window: ceiling(4k/f) for Conway+.
    pub randomness_stabilisation_window: u64,
    /// Stability window: ceiling(3k/f) for Alonzo/Babbage (per Haskell erratum 17.3).
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
    /// Current protocol parameter update proposals (pre-Conway, sgsCurProposals):
    /// proposals where ppupEpoch == currentEpoch at submission time.
    /// Maps target_epoch -> [(genesis_delegate_hash, proposed_update)]
    pub pending_pp_updates: BTreeMap<EpochNo, Vec<(Hash32, ProtocolParamUpdate)>>,
    /// Future protocol parameter update proposals (pre-Conway, sgsFutureProposals):
    /// proposals where ppupEpoch == currentEpoch + 1 at submission time.
    /// Promoted to `pending_pp_updates` at each epoch boundary (matching Haskell's
    /// `updatePpup` which moves sgsFuture → sgsCur after evaluating sgsCur).
    #[serde(default)]
    pub future_pp_updates: BTreeMap<EpochNo, Vec<(Hash32, ProtocolParamUpdate)>>,
    /// Quorum for pre-Conway protocol parameter updates (from Shelley genesis)
    #[serde(default = "default_update_quorum")]
    pub update_quorum: u64,
    /// Conway governance state (Arc for copy-on-write)
    pub governance: Arc<GovernanceState>,
    /// Slot configuration for Plutus time conversion
    pub slot_config: SlotConfig,
    /// When true, `rebuild_stake_distribution()` runs at each epoch boundary.
    /// Set after loading a snapshot (where incremental tracking may have drifted).
    /// During replay from genesis, incremental tracking is always correct.
    #[serde(skip)]
    pub needs_stake_rebuild: bool,
    /// Pointer-addressed UTxO stake: pointer → coin amount (Haskell's `sisPtrStake`).
    ///
    /// Haskell's `ShelleyInstantStake` tracks pointer-addressed UTxO coins separately
    /// in `sisPtrStake` and resolves them to credentials at each SNAP boundary using
    /// the current `saPtrs` map.  This deferred resolution means that if a credential
    /// deregisters (removing its pointer_map entry), its pointer-addressed coins are
    /// excluded from the snapshot — they are NOT credited to any pool.
    ///
    /// Torsten previously resolved pointer addresses eagerly at UTxO insertion time
    /// and stored coins directly in `stake_distribution.stake_map`.  This diverged
    /// from Haskell when a deregistration removed the pointer_map entry after the
    /// UTxO was created — Torsten kept the coins in stake_map while Haskell excluded
    /// them from the next snapshot.
    ///
    /// This field implements the deferred model: pointer UTxO coins are tracked here
    /// by pointer key; they are resolved to credentials at each epoch boundary
    /// (SNAP time) via the current `pointer_map`.  If a pointer has no entry in
    /// `pointer_map` (deregistered credential), its coins are excluded from the snapshot.
    ///
    /// `#[serde(default)]` ensures backward compatibility: snapshots written before
    /// this field was added deserialise with an empty map (safe because
    /// `needs_stake_rebuild` will be set, triggering a full UTxO scan that
    /// correctly populates both `stake_distribution.stake_map` and this field).
    #[serde(default)]
    pub ptr_stake: HashMap<torsten_primitives::credentials::Pointer, u64>,
    /// Whether pointer-addressed UTxO stake has been excluded from `stake_distribution`.
    ///
    /// In Conway (protocol version >= 9), Haskell's `ConwayInstantStake` has no pointer
    /// map — pointer-addressed UTxOs are silently excluded from pool stake calculations.
    /// This flag ensures the one-time exclusion happens at the first Conway epoch boundary.
    /// From that point forward, the incremental `apply_block` also skips pointer addresses.
    #[serde(skip)]
    pub ptr_stake_excluded: bool,
    /// Pending reward update retained for backward compatibility with snapshots
    /// written by the old deferred-RUPD code path.
    ///
    /// The corrected RUPD implementation computes AND applies the reward update in
    /// the same `process_epoch_transition` call (matching Haskell's NEWEPOCH rule
    /// exactly).  This field is therefore always `None` after a transition runs
    /// under the new code.  It is kept here so that snapshots produced by older
    /// node versions can still be loaded: the single pending update they carry will
    /// be applied once at the very next epoch boundary and the field will be cleared.
    ///
    /// Do NOT populate this field in new code.  Use `apply_pending_reward_update`
    /// only for the one-time migration path.
    #[serde(default)]
    pub pending_reward_update: Option<PendingRewardUpdate>,
    /// Running total of all stake key deposits locked in the ledger (lovelace).
    /// Incremented by `pp.key_deposit` on StakeRegistration / ConwayStakeRegistration /
    /// combined registration certs. Decremented on deregistration.
    /// Matches Haskell's `oblStake = sumDepositsAccounts accounts`.
    #[serde(default)]
    pub total_stake_key_deposits: u64,
    /// Script-type stake credentials (credential_type = 1 for N2C queries).
    /// Populated from StakeRegistration / ConwayStakeRegistration / RegStakeDeleg /
    /// RegStakeVoteDeleg / VoteRegDeleg certificates when the credential is a
    /// Credential::Script variant.  Used to correctly set credential_type in
    /// GetStakeDelegDeposits and GetFilteredVoteDelegatees responses.
    #[serde(default)]
    pub script_stake_credentials: std::collections::HashSet<Hash32>,
    /// Per-block UTxO diffs for the last k blocks, supporting fast diff-based
    /// rollback without a snapshot reload + replay.
    ///
    /// Not persisted in snapshots (`#[serde(skip)]`): the diff window only
    /// covers in-memory volatile blocks, so it resets to empty after any
    /// snapshot load.  The snapshot-reload+replay path in `handle_rollback`
    /// covers the case where the diff window is insufficient.
    #[serde(skip)]
    pub diff_seq: DiffSeq,
    /// The network this node is running on (mainnet, testnet, etc.).
    ///
    /// Used for unconditional output/withdrawal address network checks during
    /// Phase-1 validation (Haskell's `Globals.networkId`).  Not persisted in
    /// snapshots — set from genesis/config at node startup.
    ///
    /// Defaults to `None` (check skipped) when not set, preserving backwards
    /// compatibility with existing snapshot-loaded ledger states.
    #[serde(skip)]
    pub node_network: Option<torsten_primitives::network::NetworkId>,
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
    /// boundary (i.e. `proposals` was empty during that epoch).  Dormant epochs do not
    /// count against DRep activity — a DRep is considered inactive only when:
    ///
    ///   new_epoch - last_active_epoch - num_dormant_epochs > drep_activity_threshold
    ///
    /// This prevents DReps from being incorrectly marked inactive during quiescent
    /// periods where there was nothing to vote on.
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
}

/// Registration state for a DRep
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DRepRegistration {
    pub credential: Credential,
    pub deposit: Lovelace,
    pub anchor: Option<Anchor>,
    pub registered_epoch: EpochNo,
    /// Last epoch in which this DRep voted or updated (for activity tracking per CIP-1694)
    pub last_active_epoch: EpochNo,
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

    pub fn new(params: ProtocolParameters) -> Self {
        LedgerState {
            utxo_set: UtxoSet::new(),
            tip: Tip::origin(),
            era: Era::Conway,
            epoch: EpochNo(0),
            epoch_length: 432000,          // mainnet default
            shelley_transition_epoch: 208, // mainnet default
            byron_epoch_length: 21600,     // mainnet default (10 * 2160)
            prev_protocol_params: params.clone(),
            protocol_params: params,
            prev_d: 1.0,                    // Genesis: d=1
            prev_protocol_version_major: 6, // Genesis: Alonzo (proto 6)
            stake_distribution: StakeDistributionState::default(),
            treasury: Lovelace(0),
            pending_donations: Lovelace(0),
            reserves: Lovelace(MAX_LOVELACE_SUPPLY),
            delegations: Arc::new(HashMap::new()),
            pool_params: Arc::new(HashMap::new()),
            future_pool_params: HashMap::new(),
            pending_retirements: HashMap::new(),
            snapshots: EpochSnapshots::default(),
            reward_accounts: Arc::new(HashMap::new()),
            pointer_map: HashMap::new(),
            epoch_fees: Lovelace(0),
            epoch_blocks_by_pool: Arc::new(HashMap::new()),
            epoch_block_count: 0,
            evolving_nonce: Hash32::ZERO,
            candidate_nonce: Hash32::ZERO,
            epoch_nonce: Hash32::ZERO,
            lab_nonce: Hash32::ZERO,
            last_epoch_block_nonce: Hash32::ZERO,
            randomness_stabilisation_window: 172800, // 4k/f on mainnet: ceil(4*2160/0.05)
            stability_window_3kf: 129600,            // 3k/f on mainnet: ceil(3*2160/0.05)
            genesis_hash: Hash32::ZERO,
            // Legacy fields (serde compat)
            rolling_nonce: Hash32::ZERO,
            stability_window: 0,
            first_block_hash_of_epoch: None,
            prev_epoch_first_block_hash: None,
            pending_pp_updates: BTreeMap::new(),
            future_pp_updates: BTreeMap::new(),
            update_quorum: default_update_quorum(),
            governance: Arc::new(GovernanceState::default()),
            slot_config: SlotConfig::default(),
            needs_stake_rebuild: false,
            ptr_stake: HashMap::new(),
            ptr_stake_excluded: false,
            total_stake_key_deposits: 0,
            pending_reward_update: None,
            script_stake_credentials: std::collections::HashSet::new(),
            diff_seq: DiffSeq::new(),
            node_network: None,
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

    /// Configure the epoch length (from Shelley genesis)
    pub fn set_epoch_length(&mut self, epoch_length: u64, security_param: u64) {
        self.epoch_length = epoch_length;
        // Compute BOTH stability windows:
        //   randomness_stabilisation_window = ceiling(4k/f) — Conway+ candidate freeze
        //   stability_window_3kf            = ceiling(3k/f) — Alonzo/Babbage candidate freeze
        let (f_num, f_den) = self.protocol_params.active_slot_coeff_rational();
        self.randomness_stabilisation_window =
            torsten_primitives::protocol_params::ceiling_div_by_rational(
                4,
                security_param,
                f_num,
                f_den,
            );
        self.stability_window_3kf = torsten_primitives::protocol_params::ceiling_div_by_rational(
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
    /// (VRF verification will fail non-fatally if `nonce_established` is false,
    /// or produce an informative error if strict).
    pub fn epoch_nonce_for_slot(&self, slot: u64) -> Hash32 {
        let block_epoch = self.epoch_of_slot(slot);
        if block_epoch <= self.epoch.0 {
            // Same epoch (or behind, should not happen at tip): use current nonce.
            return self.epoch_nonce;
        }
        if block_epoch == self.epoch.0.saturating_add(1) {
            // Block is in the immediately following epoch.  Pre-compute the TICKN
            // nonce: epochNonce' = candidate ⭒ lastEpochBlockNonce.
            // This mirrors process_epoch_transition Step 1 exactly.
            let candidate = self.candidate_nonce;
            let prev_hash_nonce = self.last_epoch_block_nonce;
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
                torsten_primitives::hash::blake2b_256(&buf)
            };
        }
        // Block is more than one epoch ahead.  We cannot pre-compute the nonce
        // because the intermediate epochs' VRF contributions are unknown.
        // Return the current nonce; validation will fail non-fatally (or produce
        // an informative error), and the node will retry after catching up.
        self.epoch_nonce
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
        self.evolving_nonce = hash;
        self.candidate_nonce = hash;
        self.epoch_nonce = hash;
        // lab and lastEpochBlockNonce start as NeutralNonce (ZERO)
        // This is critical: at the first epoch boundary, NeutralNonce identity
        // means epochNonce = candidateNonce (not hash(candidate || genesisHash))
        self.lab_nonce = Hash32::ZERO;
        self.last_epoch_block_nonce = Hash32::ZERO;
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
            let tx_hash = torsten_primitives::hash::blake2b_256(address);

            let input = torsten_primitives::transaction::TransactionInput {
                transaction_id: tx_hash,
                index: 0,
            };

            // Parse the address bytes, or fall back to Byron if parsing fails
            let addr = torsten_primitives::Address::from_bytes(address).unwrap_or(
                torsten_primitives::Address::Byron(torsten_primitives::address::ByronAddress {
                    payload: address.clone(),
                }),
            );

            let output = torsten_primitives::transaction::TransactionOutput {
                address: addr,
                value: torsten_primitives::value::Value {
                    coin: Lovelace(*lovelace),
                    multi_asset: std::collections::BTreeMap::new(),
                },
                datum: torsten_primitives::transaction::OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            };

            self.utxo_set.insert(input, output);
            seeded += 1;
            total_lovelace += lovelace;
        }

        // Deduct seeded lovelace from reserves per Shelley spec:
        // reserves = maxLovelaceSupply - totalBalance(initialUTxO)
        // Without this, monetary expansion (rho * reserves) is computed on too
        // large a reserves value, draining reserves too fast and overfilling
        // the treasury.
        self.reserves.0 = self.reserves.0.saturating_sub(total_lovelace);

        debug!(
            "Ledger: seeded {} genesis UTxOs ({} lovelace, reserves now {})",
            seeded, total_lovelace, self.reserves.0
        );
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
        use torsten_primitives::era::Era;

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
    pub fn rollback_blocks(&mut self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }

        // Pop the last n diffs from the sequence (most-recent first).
        let diffs = self.diff_seq.rollback(n);
        let actually_rolled = diffs.len();

        for (_slot, _hash, diff) in &diffs {
            // Undo inserts: remove UTxOs that were created by this block.
            for (input, _output) in &diff.inserts {
                self.utxo_set.remove(input);
            }
            // Undo deletes: restore UTxOs that were consumed by this block.
            for (input, output) in &diff.deletes {
                self.utxo_set.insert(input.clone(), output.clone());
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
    pub fn rollback_blocks_to_point(&mut self, n: usize, new_tip: Tip) -> usize {
        let rolled = self.rollback_blocks(n);
        if rolled > 0 {
            self.tip = new_tip;
        }
        rolled
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
    Pointer(torsten_primitives::credentials::Pointer),
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
    address: &torsten_primitives::address::Address,
    exclude_ptrs: bool,
) -> StakeRouting {
    use torsten_primitives::address::Address;
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
fn stake_credential_hash(address: &torsten_primitives::address::Address) -> Option<Hash32> {
    use torsten_primitives::address::Address;
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
}

#[cfg(test)]
mod tests;
