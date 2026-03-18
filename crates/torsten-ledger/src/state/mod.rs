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
    /// Current protocol parameters
    pub protocol_params: ProtocolParameters,
    /// Stake distribution
    pub stake_distribution: StakeDistributionState,
    /// Treasury balance
    pub treasury: Lovelace,
    /// Reserves balance (ADA not yet in circulation)
    pub reserves: Lovelace,
    /// Delegation state: credential_hash -> pool_id (Arc for copy-on-write)
    pub delegations: Arc<HashMap<Hash32, Hash28>>,
    /// Pool registrations: pool_id -> pool registration (Arc for copy-on-write)
    pub pool_params: Arc<HashMap<Hash28, PoolRegistration>>,
    /// Pool retirements pending at a given epoch
    pub pending_retirements: BTreeMap<EpochNo, Vec<Hash28>>,
    /// Stake snapshots for the Cardano "mark/set/go" snapshot model
    pub snapshots: EpochSnapshots,
    /// Reward accounts: stake credential hash -> accumulated rewards (Arc for copy-on-write)
    pub reward_accounts: Arc<HashMap<Hash32, Lovelace>>,
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
    /// Pending protocol parameter update proposals (pre-Conway):
    /// Maps target_epoch -> [(genesis_delegate_hash, proposed_update)]
    pub pending_pp_updates: BTreeMap<EpochNo, Vec<(Hash32, ProtocolParamUpdate)>>,
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
    /// Pending reward update computed at one epoch boundary and applied at the
    /// next, matching Haskell's RUPD (Reward UPDate) / pulsing reward scheme.
    ///
    /// At boundary E -> E+1:
    ///   1. Apply `pending_reward_update` (computed at E-1 -> E boundary)
    ///   2. Rotate snapshots, build new mark snapshot
    ///   3. Compute new rewards using go snapshot -> store in `pending_reward_update`
    ///
    /// This defers reward application by one epoch, matching Haskell exactly.
    #[serde(default)]
    pub pending_reward_update: Option<PendingRewardUpdate>,
    /// Script-type stake credentials (credential_type = 1 for N2C queries).
    /// Populated from StakeRegistration / ConwayStakeRegistration / RegStakeDeleg /
    /// RegStakeVoteDeleg / VoteRegDeleg certificates when the credential is a
    /// Credential::Script variant.  Used to correctly set credential_type in
    /// GetStakeDelegDeposits and GetFilteredVoteDelegatees responses.
    #[serde(default)]
    pub script_stake_credentials: std::collections::HashSet<Hash32>,
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EpochSnapshots {
    /// Snapshot from the most recent epoch boundary ("mark")
    pub mark: Option<StakeSnapshot>,
    /// Snapshot from one epoch ago ("set") — used for leader election
    pub set: Option<StakeSnapshot>,
    /// Snapshot from two epochs ago ("go") — used for reward distribution
    pub go: Option<StakeSnapshot>,
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
    /// Used by `calculate_rewards` in the go snapshot for RUPD deltaT1.
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
            protocol_params: params,
            stake_distribution: StakeDistributionState::default(),
            treasury: Lovelace(0),
            reserves: Lovelace(MAX_LOVELACE_SUPPLY),
            delegations: Arc::new(HashMap::new()),
            pool_params: Arc::new(HashMap::new()),
            pending_retirements: BTreeMap::new(),
            snapshots: EpochSnapshots::default(),
            reward_accounts: Arc::new(HashMap::new()),
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
            update_quorum: default_update_quorum(),
            governance: Arc::new(GovernanceState::default()),
            slot_config: SlotConfig::default(),
            needs_stake_rebuild: true,
            pending_reward_update: None,
            script_stake_credentials: std::collections::HashSet::new(),
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
}

/// Extract a Hash32 from a Credential for use as a map key
fn credential_to_hash(credential: &Credential) -> Hash32 {
    credential.to_hash().to_hash32_padded()
}

/// Extract the staking credential hash from an address (Base and Reward addresses only).
/// Returns None for Enterprise, Pointer, and Byron addresses which have no staking part.
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
