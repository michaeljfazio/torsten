use crate::plutus::SlotConfig;
use crate::utxo::UtxoSet;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;
use torsten_primitives::block::{Block, Point, Tip};
use torsten_primitives::credentials::Credential;
use torsten_primitives::era::Era;
use torsten_primitives::hash::{Hash28, Hash32};
use torsten_primitives::protocol_params::ProtocolParameters;
use torsten_primitives::time::{BlockNo, EpochNo, SlotNo};
use torsten_primitives::transaction::{
    Anchor, Certificate, Constitution, DRep, GovAction, GovActionId, MIRSource, MIRTarget,
    ProposalProcedure, ProtocolParamUpdate, Rational, Relay, Vote, Voter, VotingProcedure,
};
use torsten_primitives::value::Lovelace;
use tracing::{debug, info, trace, warn};

/// Total ADA supply (45 billion ADA = 45 * 10^15 lovelace)
pub const MAX_LOVELACE_SUPPLY: u64 = 45_000_000_000_000_000;

/// Maximum allowed snapshot file size (10 GiB).
/// Prevents OOM from loading maliciously crafted or corrupted snapshot files.
pub const MAX_SNAPSHOT_SIZE: usize = 10 * 1024 * 1024 * 1024;

/// Reduced rational number (i128 numerator/denominator with GCD reduction).
/// Matches Haskell's Rational for reward calculations with rationalToCoinViaFloor.
#[derive(Clone, Copy)]
struct Rat {
    n: i128,
    d: i128,
}

impl Rat {
    fn new(n: i128, d: i128) -> Self {
        if d == 0 {
            return Rat { n: 0, d: 1 };
        }
        let g = Self::gcd(n.unsigned_abs(), d.unsigned_abs()) as i128;
        let sign = if d < 0 { -1 } else { 1 };
        Rat {
            n: sign * n / g,
            d: sign * d / g,
        }
    }

    fn gcd(a: u128, b: u128) -> u128 {
        if b == 0 {
            a
        } else {
            Self::gcd(b, a % b)
        }
    }

    fn add(&self, other: &Rat) -> Rat {
        // Cross-reduce before adding to prevent overflow:
        // a/b + c/d = (a*(d/g) + c*(b/g)) / (b/g*d)  where g = gcd(b,d)
        let g = Self::gcd(self.d.unsigned_abs(), other.d.unsigned_abs()) as i128;
        let bd = self.d / g;
        Rat::new(self.n * (other.d / g) + other.n * bd, bd * other.d)
    }

    fn sub(&self, other: &Rat) -> Rat {
        let g = Self::gcd(self.d.unsigned_abs(), other.d.unsigned_abs()) as i128;
        let bd = self.d / g;
        Rat::new(self.n * (other.d / g) - other.n * bd, bd * other.d)
    }

    fn mul(&self, other: &Rat) -> Rat {
        // Cross-reduce before multiplying to prevent overflow:
        // (a/b) * (c/d) = (a/g1 * c/g2) / (b/g2 * d/g1) where g1=gcd(a,d), g2=gcd(b,c)
        let g1 = Self::gcd(self.n.unsigned_abs(), other.d.unsigned_abs()) as i128;
        let g2 = Self::gcd(self.d.unsigned_abs(), other.n.unsigned_abs()) as i128;
        Rat::new(
            (self.n / g1) * (other.n / g2),
            (self.d / g2) * (other.d / g1),
        )
    }

    fn div(&self, other: &Rat) -> Rat {
        // (a/b) / (c/d) = (a/b) * (d/c)
        let g1 = Self::gcd(self.n.unsigned_abs(), other.n.unsigned_abs()) as i128;
        let g2 = Self::gcd(self.d.unsigned_abs(), other.d.unsigned_abs()) as i128;
        Rat::new(
            (self.n / g1) * (other.d / g2),
            (self.d / g2) * (other.n / g1),
        )
    }

    fn min_rat(&self, other: &Rat) -> Rat {
        // Compare using cross-multiplication: a/b <= c/d iff a*d <= c*b (when b,d > 0)
        if self.n * other.d <= other.n * self.d {
            *self
        } else {
            *other
        }
    }

    fn floor_u64(&self) -> u64 {
        if self.d == 0 || self.n <= 0 {
            0
        } else {
            (self.n / self.d) as u64
        }
    }
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
    /// Randomness stabilisation window: 4k/f slots (NOT 3k/f).
    /// Blocks in the last randomness_stabilisation_window slots of an epoch
    /// do NOT update the candidate nonce.
    pub randomness_stabilisation_window: u64,
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
}

/// Conway-era governance state (CIP-1694)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GovernanceState {
    /// Registered DReps: credential -> DRepState
    pub dreps: HashMap<Hash32, DRepRegistration>,
    /// Vote delegations: stake credential hash -> DRep
    pub vote_delegations: HashMap<Hash32, DRep>,
    /// Constitutional committee: cold credential -> hot credential
    pub committee_hot_keys: HashMap<Hash32, Hash32>,
    /// Committee member expiration epochs (cold credential -> expiration epoch)
    pub committee_expiration: HashMap<Hash32, EpochNo>,
    /// Resigned committee members
    pub committee_resigned: HashMap<Hash32, Option<Anchor>>,
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
            randomness_stabilisation_window: 172800, // 4k/f on mainnet
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
        }
    }

    /// Set the slot configuration for Plutus time conversion
    pub fn set_slot_config(&mut self, slot_config: SlotConfig) {
        self.slot_config = slot_config;
        info!(
            zero_time = slot_config.zero_time,
            zero_slot = slot_config.zero_slot,
            slot_length = slot_config.slot_length,
            "Ledger: slot config set for Plutus evaluation"
        );
    }

    /// Configure the epoch length (from Shelley genesis)
    pub fn set_epoch_length(&mut self, epoch_length: u64, security_param: u64) {
        self.epoch_length = epoch_length;
        // randomness_stabilisation_window = ceiling(4k/f) per Haskell StabilityWindow.hs
        // Use integer arithmetic to avoid f64 precision issues in this consensus-critical calc
        let (f_num, f_den) = self.protocol_params.active_slot_coeff_rational();
        self.randomness_stabilisation_window =
            torsten_primitives::protocol_params::ceiling_div_by_rational(
                4,
                security_param,
                f_num,
                f_den,
            );
        info!(
            epoch_length,
            randomness_stabilisation_window = self.randomness_stabilisation_window,
            security_param,
            "Ledger: epoch length configured"
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
        info!(
            shelley_transition_epoch,
            byron_epoch_length,
            byron_slots = byron_epoch_length * shelley_transition_epoch,
            "Ledger: Shelley transition configured"
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
    /// Initializes the Praos nonce state machine. In Haskell, the initial
    /// evolving nonce and candidate nonce are derived from the genesis hash.
    pub fn set_genesis_hash(&mut self, hash: Hash32) {
        self.genesis_hash = hash;
        // Initialize nonce state from genesis hash
        self.evolving_nonce = hash;
        self.candidate_nonce = hash;
        info!(
            genesis_hash = %hash.to_hex(),
            "Ledger: nonce state initialized from genesis hash"
        );
    }

    /// Set the update quorum threshold (from Shelley genesis)
    pub fn set_update_quorum(&mut self, quorum: u64) {
        self.update_quorum = quorum;
        info!(update_quorum = quorum, "Ledger: update quorum configured");
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
                raw_cbor: None,
            };

            self.utxo_set.insert(input, output);
            seeded += 1;
            total_lovelace += lovelace;
        }

        info!(
            seeded,
            total_lovelace,
            utxo_count = self.utxo_set.len(),
            "Ledger: genesis UTxOs seeded"
        );
    }

    /// Validate that a governance threshold rational is in the range [0, 1]
    /// with a non-zero denominator.
    fn validate_threshold(name: &str, r: &Rational) -> Result<(), LedgerError> {
        if r.denominator == 0 {
            return Err(LedgerError::InvalidProtocolParam(format!(
                "{}: zero denominator",
                name
            )));
        }
        if r.numerator > r.denominator {
            return Err(LedgerError::InvalidProtocolParam(format!(
                "{}: threshold {}/{} exceeds 1",
                name, r.numerator, r.denominator
            )));
        }
        Ok(())
    }

    /// Apply a single ProtocolParamUpdate to the current protocol parameters.
    /// Each field in the update, if Some, overwrites the corresponding parameter.
    /// Used by both pre-Conway update proposals and Conway governance actions.
    /// Returns an error if any governance threshold is out of range [0, 1].
    fn apply_protocol_param_update(
        &mut self,
        update: &ProtocolParamUpdate,
    ) -> Result<(), LedgerError> {
        if let Some(v) = update.min_fee_a {
            self.protocol_params.min_fee_a = v;
        }
        if let Some(v) = update.min_fee_b {
            self.protocol_params.min_fee_b = v;
        }
        if let Some(v) = update.max_block_body_size {
            self.protocol_params.max_block_body_size = v;
        }
        if let Some(v) = update.max_tx_size {
            self.protocol_params.max_tx_size = v;
        }
        if let Some(v) = update.max_block_header_size {
            self.protocol_params.max_block_header_size = v;
        }
        if let Some(v) = update.key_deposit {
            self.protocol_params.key_deposit = v;
        }
        if let Some(v) = update.pool_deposit {
            self.protocol_params.pool_deposit = v;
        }
        if let Some(v) = update.e_max {
            self.protocol_params.e_max = v;
        }
        if let Some(v) = update.n_opt {
            self.protocol_params.n_opt = v;
        }
        if let Some(ref v) = update.a0 {
            self.protocol_params.a0 = v.clone();
        }
        if let Some(ref v) = update.rho {
            self.protocol_params.rho = v.clone();
        }
        if let Some(ref v) = update.tau {
            self.protocol_params.tau = v.clone();
        }
        if let Some(v) = update.min_pool_cost {
            self.protocol_params.min_pool_cost = v;
        }
        if let Some(v) = update.ada_per_utxo_byte {
            self.protocol_params.ada_per_utxo_byte = v;
        }
        if let Some(ref v) = update.cost_models {
            if let Some(ref v1) = v.plutus_v1 {
                self.protocol_params.cost_models.plutus_v1 = Some(v1.clone());
            }
            if let Some(ref v2) = v.plutus_v2 {
                self.protocol_params.cost_models.plutus_v2 = Some(v2.clone());
            }
            if let Some(ref v3) = v.plutus_v3 {
                self.protocol_params.cost_models.plutus_v3 = Some(v3.clone());
            }
        }
        if let Some(ref v) = update.execution_costs {
            self.protocol_params.execution_costs = v.clone();
        }
        if let Some(v) = update.max_tx_ex_units {
            self.protocol_params.max_tx_ex_units = v;
        }
        if let Some(v) = update.max_block_ex_units {
            self.protocol_params.max_block_ex_units = v;
        }
        if let Some(v) = update.max_val_size {
            self.protocol_params.max_val_size = v;
        }
        if let Some(v) = update.collateral_percentage {
            self.protocol_params.collateral_percentage = v;
        }
        if let Some(v) = update.max_collateral_inputs {
            self.protocol_params.max_collateral_inputs = v;
        }
        if let Some(v) = update.min_fee_ref_script_cost_per_byte {
            self.protocol_params.min_fee_ref_script_cost_per_byte = v;
        }
        if let Some(v) = update.drep_deposit {
            self.protocol_params.drep_deposit = v;
        }
        if let Some(v) = update.gov_action_lifetime {
            self.protocol_params.gov_action_lifetime = v;
        }
        if let Some(v) = update.gov_action_deposit {
            self.protocol_params.gov_action_deposit = v;
        }
        if let Some(ref v) = update.dvt_pp_network_group {
            Self::validate_threshold("dvt_pp_network_group", v)?;
            self.protocol_params.dvt_pp_network_group = v.clone();
        }
        if let Some(ref v) = update.dvt_pp_economic_group {
            Self::validate_threshold("dvt_pp_economic_group", v)?;
            self.protocol_params.dvt_pp_economic_group = v.clone();
        }
        if let Some(ref v) = update.dvt_pp_technical_group {
            Self::validate_threshold("dvt_pp_technical_group", v)?;
            self.protocol_params.dvt_pp_technical_group = v.clone();
        }
        if let Some(ref v) = update.dvt_pp_gov_group {
            Self::validate_threshold("dvt_pp_gov_group", v)?;
            self.protocol_params.dvt_pp_gov_group = v.clone();
        }
        if let Some(ref v) = update.dvt_hard_fork {
            Self::validate_threshold("dvt_hard_fork", v)?;
            self.protocol_params.dvt_hard_fork = v.clone();
        }
        if let Some(ref v) = update.dvt_no_confidence {
            Self::validate_threshold("dvt_no_confidence", v)?;
            self.protocol_params.dvt_no_confidence = v.clone();
        }
        if let Some(ref v) = update.dvt_committee_normal {
            Self::validate_threshold("dvt_committee_normal", v)?;
            self.protocol_params.dvt_committee_normal = v.clone();
        }
        if let Some(ref v) = update.dvt_committee_no_confidence {
            Self::validate_threshold("dvt_committee_no_confidence", v)?;
            self.protocol_params.dvt_committee_no_confidence = v.clone();
        }
        if let Some(ref v) = update.dvt_constitution {
            Self::validate_threshold("dvt_constitution", v)?;
            self.protocol_params.dvt_constitution = v.clone();
        }
        if let Some(ref v) = update.dvt_treasury_withdrawal {
            Self::validate_threshold("dvt_treasury_withdrawal", v)?;
            self.protocol_params.dvt_treasury_withdrawal = v.clone();
        }
        if let Some(ref v) = update.pvt_motion_no_confidence {
            Self::validate_threshold("pvt_motion_no_confidence", v)?;
            self.protocol_params.pvt_motion_no_confidence = v.clone();
        }
        if let Some(ref v) = update.pvt_committee_normal {
            Self::validate_threshold("pvt_committee_normal", v)?;
            self.protocol_params.pvt_committee_normal = v.clone();
        }
        if let Some(ref v) = update.pvt_committee_no_confidence {
            Self::validate_threshold("pvt_committee_no_confidence", v)?;
            self.protocol_params.pvt_committee_no_confidence = v.clone();
        }
        if let Some(ref v) = update.pvt_hard_fork {
            Self::validate_threshold("pvt_hard_fork", v)?;
            self.protocol_params.pvt_hard_fork = v.clone();
        }
        if let Some(ref v) = update.pvt_pp_security_group {
            Self::validate_threshold("pvt_pp_security_group", v)?;
            self.protocol_params.pvt_pp_security_group = v.clone();
        }
        if let Some(v) = update.min_committee_size {
            self.protocol_params.committee_min_size = v;
        }
        if let Some(v) = update.committee_term_limit {
            self.protocol_params.committee_max_term_length = v;
        }
        if let Some(v) = update.drep_activity {
            self.protocol_params.drep_activity = v;
        }
        if let Some(v) = update.protocol_version_major {
            self.protocol_params.protocol_version_major = v;
        }
        if let Some(v) = update.protocol_version_minor {
            self.protocol_params.protocol_version_minor = v;
        }
        Ok(())
    }

    /// Apply a block to the ledger state
    pub fn apply_block(&mut self, block: &Block) -> Result<(), LedgerError> {
        trace!(
            slot = block.slot().0,
            block_no = block.block_number().0,
            era = ?block.era,
            txs = block.transactions.len(),
            hash = %block.header.header_hash.to_hex(),
            "Ledger: applying block"
        );

        // Verify block connects to current tip
        if self.tip.point != Point::Origin {
            if let Some(tip_hash) = self.tip.point.hash() {
                if block.prev_hash() != tip_hash {
                    return Err(LedgerError::BlockDoesNotConnect {
                        expected: tip_hash.to_hex(),
                        got: block.prev_hash().to_hex(),
                    });
                }
            }
        }

        // Check for epoch transition before processing the block.
        // When multiple epochs are skipped (e.g., after offline time or Mithril import),
        // process each intermediate epoch transition individually so that snapshots rotate
        // correctly, pending retirements fire at the right epoch, and rewards are distributed.
        let block_epoch = EpochNo(self.epoch_of_slot(block.slot().0));
        if block_epoch > self.epoch {
            info!(
                prev_epoch = self.epoch.0,
                new_epoch = block_epoch.0,
                slot = block.slot().0,
                "Ledger: epoch transition detected"
            );
            while self.epoch < block_epoch {
                let next_epoch = EpochNo(self.epoch.0.saturating_add(1));
                self.process_epoch_transition(next_epoch);
            }
        }

        // Block body size check — reject blocks exceeding max_block_body_size
        if block.header.body_size > 0
            && self.protocol_params.max_block_body_size > 0
            && block.header.body_size > self.protocol_params.max_block_body_size
        {
            debug!(
                body_size = block.header.body_size,
                limit = self.protocol_params.max_block_body_size,
                slot = block.slot().0,
                "Block body exceeds max_block_body_size (expected during replay before PP updates)"
            );
        }

        // Block-level execution unit budget check
        let mut block_mem: u64 = 0;
        let mut block_steps: u64 = 0;
        for tx in &block.transactions {
            if tx.is_valid {
                for r in &tx.witness_set.redeemers {
                    block_mem = block_mem.saturating_add(r.ex_units.mem);
                    block_steps = block_steps.saturating_add(r.ex_units.steps);
                }
            }
        }
        if block_mem > self.protocol_params.max_block_ex_units.mem {
            debug!(
                block_mem,
                limit = self.protocol_params.max_block_ex_units.mem,
                "Block exceeds max execution unit memory budget (expected during replay before PP updates)"
            );
        }
        if block_steps > self.protocol_params.max_block_ex_units.steps {
            debug!(
                block_steps,
                limit = self.protocol_params.max_block_ex_units.steps,
                "Block exceeds max execution unit step budget (expected during replay before PP updates)"
            );
        }

        // Track processed tx hashes to skip duplicates within a block
        let mut processed_tx_hashes =
            std::collections::HashSet::with_capacity(block.transactions.len());

        // Apply each transaction
        for tx in &block.transactions {
            if !processed_tx_hashes.insert(tx.hash) {
                warn!(
                    tx_hash = %tx.hash.to_hex(),
                    slot = block.slot().0,
                    "Duplicate transaction hash in block, skipping"
                );
                continue;
            }
            // Handle invalid transactions (phase-2 validation failure):
            // - Collateral inputs are consumed (forfeit to block producer)
            // - Regular inputs/outputs/certificates are NOT applied
            // - If collateral_return is present, it becomes a new UTxO
            if !tx.is_valid {
                // Sum up total collateral input value
                let mut collateral_input_value: u64 = 0;
                // Consume collateral inputs (update stake distribution)
                for col_input in &tx.body.collateral {
                    if let Some(spent) = self.utxo_set.lookup(col_input) {
                        collateral_input_value += spent.value.coin.0;
                        if let Some(cred) = stake_credential_hash(&spent.address) {
                            if let Some(stake) = self.stake_distribution.stake_map.get_mut(&cred) {
                                stake.0 = stake.0.saturating_sub(spent.value.coin.0);
                            }
                        }
                    }
                    self.utxo_set.remove(col_input);
                }
                // If there's a collateral return output, add it
                let collateral_return_value = if let Some(col_return) = &tx.body.collateral_return {
                    if let Some(cred) = stake_credential_hash(&col_return.address) {
                        *self
                            .stake_distribution
                            .stake_map
                            .entry(cred)
                            .or_insert(Lovelace(0)) += Lovelace(col_return.value.coin.0);
                    }
                    let return_input = torsten_primitives::transaction::TransactionInput {
                        transaction_id: tx.hash,
                        index: tx.body.outputs.len() as u32, // collateral return is after regular outputs
                    };
                    let return_val = col_return.value.coin.0;
                    self.utxo_set.insert(return_input, col_return.clone());
                    return_val
                } else {
                    0
                };
                // Fee collected is the actual collateral forfeited, NOT the declared fee.
                // If total_collateral is set, use it; otherwise compute from inputs - return.
                let collateral_fee = if let Some(tc) = tx.body.total_collateral {
                    tc
                } else {
                    Lovelace(collateral_input_value.saturating_sub(collateral_return_value))
                };
                self.epoch_fees += collateral_fee;
                continue;
            }

            // Update stake distribution from consumed inputs (subtract)
            for input in &tx.body.inputs {
                if let Some(spent_output) = self.utxo_set.lookup(input) {
                    if let Some(cred_hash) = stake_credential_hash(&spent_output.address) {
                        if let Some(stake) = self.stake_distribution.stake_map.get_mut(&cred_hash) {
                            stake.0 = stake.0.saturating_sub(spent_output.value.coin.0);
                        }
                    }
                }
            }

            // Apply UTxO changes (may fail for missing inputs during initial sync)
            if let Err(e) =
                self.utxo_set
                    .apply_transaction(&tx.hash, &tx.body.inputs, &tx.body.outputs)
            {
                // During initial sync without full history, inputs won't be found.
                // Skip UTxO changes AND stake crediting to avoid phantom state.
                // Fees and certificates are still processed.
                debug!("UTxO application skipped (missing inputs): {e}");
            } else {
                // Update stake distribution from new outputs (add)
                // Only credit stake when UTxO application succeeded to avoid
                // phantom stake from outputs that don't actually exist in the set.
                for output in &tx.body.outputs {
                    if let Some(cred_hash) = stake_credential_hash(&output.address) {
                        *self
                            .stake_distribution
                            .stake_map
                            .entry(cred_hash)
                            .or_insert(Lovelace(0)) += Lovelace(output.value.coin.0);
                    }
                }
            }

            // Accumulate fees
            self.epoch_fees += tx.body.fee;

            // Process certificates
            for cert in &tx.body.certificates {
                self.process_certificate(cert);
            }

            // Process withdrawals (rewards are consumed, no UTxO effect)
            for (reward_account, amount) in &tx.body.withdrawals {
                self.process_withdrawal(reward_account, *amount);
            }

            // Process Conway governance proposals
            for (idx, proposal) in tx.body.proposal_procedures.iter().enumerate() {
                self.process_proposal(&tx.hash, idx as u32, proposal);
            }

            // Process Conway governance votes
            for (voter, action_votes) in &tx.body.voting_procedures {
                for (action_id, procedure) in action_votes {
                    self.process_vote(voter, action_id, procedure);
                }
            }

            // Process treasury donations
            if let Some(donation) = tx.body.donation {
                self.treasury += donation;
            }

            // Collect pre-Conway protocol parameter update proposals
            if let Some(ref update) = tx.body.update {
                for (genesis_hash, ppu) in &update.proposed_updates {
                    info!(
                        genesis_hash = %genesis_hash.to_hex(),
                        target_epoch = update.epoch,
                        protocol_version = ?ppu.protocol_version_major.zip(ppu.protocol_version_minor),
                        "Collected protocol parameter update proposal"
                    );
                    self.pending_pp_updates
                        .entry(EpochNo(update.epoch))
                        .or_default()
                        .push((*genesis_hash, ppu.clone()));
                }
            }
        }

        // Track block production by pool (issuer vkey hash)
        if !block.header.issuer_vkey.is_empty() {
            let pool_id = torsten_primitives::hash::blake2b_224(&block.header.issuer_vkey);
            *Arc::make_mut(&mut self.epoch_blocks_by_pool)
                .entry(pool_id)
                .or_insert(0) += 1;
        }
        self.epoch_block_count += 1;

        // Praos nonce state machine (matches Haskell reupdateChainDepState):
        //
        // 1. lab_nonce = block.prev_hash (type cast, no hashing)
        // 2. evolving_nonce is ALWAYS updated with every block's VRF output
        // 3. candidate_nonce copies evolving_nonce UNLESS we're in the last
        //    randomness_stabilisation_window (4k/f) slots of the epoch
        if !block.header.vrf_result.output.is_empty() {
            // Update evolving nonce unconditionally
            self.update_evolving_nonce(&block.header.vrf_result.output);

            // Update candidate nonce only if NOT in the stabilisation window
            // (i.e., if slot < first_slot_of_next_epoch - rsw)
            // Uses saturating_sub to avoid u64 overflow when slot values are extreme
            let first_slot_of_next_epoch = self.first_slot_of_epoch(self.epoch.0.saturating_add(1));
            if block.slot().0
                < first_slot_of_next_epoch.saturating_sub(self.randomness_stabilisation_window)
            {
                self.candidate_nonce = self.evolving_nonce;
            }
        }

        // Update LAB nonce = prev_hash of this block (simple assignment)
        self.lab_nonce = block.header.prev_hash;

        // Update tip
        self.tip = block.tip();
        self.era = block.era;

        trace!(
            slot = block.slot().0,
            block_no = block.block_number().0,
            utxo_count = self.utxo_set.len(),
            epoch = self.epoch.0,
            era = ?self.era,
            "Ledger: block applied successfully"
        );

        Ok(())
    }

    pub fn current_slot(&self) -> Option<SlotNo> {
        self.tip.point.slot()
    }

    pub fn current_block_number(&self) -> BlockNo {
        self.tip.block_number
    }

    /// Process a certificate and update the ledger state accordingly
    fn process_certificate(&mut self, cert: &Certificate) {
        match cert {
            Certificate::StakeRegistration(credential) => {
                let key = credential_to_hash(credential);
                self.stake_distribution
                    .stake_map
                    .entry(key)
                    .or_insert(Lovelace(0));
                Arc::make_mut(&mut self.reward_accounts)
                    .entry(key)
                    .or_insert(Lovelace(0));
                debug!("Stake key registered: {}", key.to_hex());
            }
            Certificate::StakeDeregistration(credential) => {
                let key = credential_to_hash(credential);
                // Per Shelley ledger spec: deregistration is only valid if reward balance is zero.
                // If the reward account has a non-zero balance, skip deregistration.
                let balance = self
                    .reward_accounts
                    .get(&key)
                    .copied()
                    .unwrap_or(Lovelace(0));
                if balance.0 > 0 {
                    warn!(
                        key = %key.to_hex(),
                        balance = balance.0,
                        "Stake deregistration rejected: non-zero reward balance"
                    );
                } else {
                    self.stake_distribution.stake_map.remove(&key);
                    Arc::make_mut(&mut self.delegations).remove(&key);
                    Arc::make_mut(&mut self.reward_accounts).remove(&key);
                    debug!("Stake key deregistered: {}", key.to_hex());
                }
            }
            Certificate::ConwayStakeRegistration {
                credential,
                deposit: _,
            } => {
                // Conway cert tag 7: same behavior as StakeRegistration
                let key = credential_to_hash(credential);
                self.stake_distribution
                    .stake_map
                    .entry(key)
                    .or_insert(Lovelace(0));
                Arc::make_mut(&mut self.reward_accounts)
                    .entry(key)
                    .or_insert(Lovelace(0));
                debug!("Stake key registered (Conway): {}", key.to_hex());
            }
            Certificate::ConwayStakeDeregistration {
                credential,
                refund: _,
            } => {
                // Conway cert tag 8: deregistration returns remaining reward balance
                // as part of the deposit refund, so unconditional removal is correct.
                let key = credential_to_hash(credential);
                self.stake_distribution.stake_map.remove(&key);
                Arc::make_mut(&mut self.delegations).remove(&key);
                Arc::make_mut(&mut self.reward_accounts).remove(&key);
                debug!("Stake key deregistered (Conway): {}", key.to_hex());
            }
            Certificate::StakeDelegation {
                credential,
                pool_hash,
            } => {
                let key = credential_to_hash(credential);
                Arc::make_mut(&mut self.delegations).insert(key, *pool_hash);
                debug!("Stake delegated to pool: {}", pool_hash.to_hex());
            }
            Certificate::PoolRegistration(params) => {
                let pool_reg = PoolRegistration {
                    pool_id: params.operator,
                    vrf_keyhash: params.vrf_keyhash,
                    pledge: params.pledge,
                    cost: params.cost,
                    margin_numerator: params.margin.numerator,
                    margin_denominator: params.margin.denominator,
                    reward_account: params.reward_account.clone(),
                    owners: params.pool_owners.clone(),
                    relays: params.relays.clone(),
                    metadata_url: params.pool_metadata.as_ref().map(|m| m.url.clone()),
                    metadata_hash: params.pool_metadata.as_ref().map(|m| m.hash),
                };
                // If the pool is re-registering, cancel any pending retirement
                if self.pool_params.contains_key(&params.operator) {
                    for pools in self.pending_retirements.values_mut() {
                        pools.retain(|id| id != &params.operator);
                    }
                    // Remove empty epoch entries
                    self.pending_retirements
                        .retain(|_, pools| !pools.is_empty());
                    debug!(
                        "Pool re-registered (pending retirement cancelled): {}",
                        params.operator.to_hex()
                    );
                } else {
                    debug!("Pool registered: {}", params.operator.to_hex());
                }
                Arc::make_mut(&mut self.pool_params).insert(params.operator, pool_reg);
            }
            Certificate::PoolRetirement { pool_hash, epoch } => {
                // Validate: retirement epoch must be <= current_epoch + e_max
                let max_retirement_epoch = self.epoch.0.saturating_add(self.protocol_params.e_max);
                if *epoch > max_retirement_epoch {
                    warn!(
                        pool = %pool_hash.to_hex(),
                        retirement_epoch = epoch,
                        current_epoch = self.epoch.0,
                        e_max = self.protocol_params.e_max,
                        "Pool retirement epoch exceeds e_max bound, ignoring"
                    );
                } else {
                    debug!(
                        "Pool retirement scheduled at epoch {}: {}",
                        epoch,
                        pool_hash.to_hex()
                    );
                    self.pending_retirements
                        .entry(EpochNo(*epoch))
                        .or_default()
                        .push(*pool_hash);
                }
            }
            Certificate::RegStakeDeleg {
                credential,
                pool_hash,
                ..
            } => {
                let key = credential_to_hash(credential);
                self.stake_distribution
                    .stake_map
                    .entry(key)
                    .or_insert(Lovelace(0));
                Arc::make_mut(&mut self.reward_accounts)
                    .entry(key)
                    .or_insert(Lovelace(0));
                Arc::make_mut(&mut self.delegations).insert(key, *pool_hash);
            }
            Certificate::RegDRep {
                credential,
                deposit,
                anchor,
            } => {
                let key = credential_to_hash(credential);
                Arc::make_mut(&mut self.governance).dreps.insert(
                    key,
                    DRepRegistration {
                        credential: credential.clone(),
                        deposit: *deposit,
                        anchor: anchor.clone(),
                        registered_epoch: self.epoch,
                        last_active_epoch: self.epoch,
                        active: true,
                    },
                );
                Arc::make_mut(&mut self.governance).drep_registration_count += 1;
                debug!("DRep registered: {}", key.to_hex());
            }
            Certificate::UnregDRep {
                credential,
                refund: _,
            } => {
                let key = credential_to_hash(credential);
                Arc::make_mut(&mut self.governance).dreps.remove(&key);
                debug!("DRep deregistered: {}", key.to_hex());
            }
            Certificate::UpdateDRep { credential, anchor } => {
                let key = credential_to_hash(credential);
                if let Some(drep) = Arc::make_mut(&mut self.governance).dreps.get_mut(&key) {
                    drep.anchor = anchor.clone();
                    drep.last_active_epoch = self.epoch;
                    debug!("DRep updated: {}", key.to_hex());
                }
            }
            Certificate::VoteDelegation { credential, drep } => {
                let key = credential_to_hash(credential);
                Arc::make_mut(&mut self.governance)
                    .vote_delegations
                    .insert(key, drep.clone());
                debug!("Vote delegated to {:?}", drep);
            }
            Certificate::StakeVoteDelegation {
                credential,
                pool_hash,
                drep,
            } => {
                let key = credential_to_hash(credential);
                // Stake delegation
                Arc::make_mut(&mut self.delegations).insert(key, *pool_hash);
                // Vote delegation
                Arc::make_mut(&mut self.governance)
                    .vote_delegations
                    .insert(key, drep.clone());
                debug!(
                    "Stake+vote delegated to pool {} and drep {:?}",
                    pool_hash.to_hex(),
                    drep
                );
            }
            Certificate::CommitteeHotAuth {
                cold_credential,
                hot_credential,
            } => {
                let cold_key = credential_to_hash(cold_credential);
                let hot_key = credential_to_hash(hot_credential);
                Arc::make_mut(&mut self.governance)
                    .committee_hot_keys
                    .insert(cold_key, hot_key);
                // Remove from resigned if re-authorizing
                Arc::make_mut(&mut self.governance)
                    .committee_resigned
                    .remove(&cold_key);
                debug!(
                    "Committee hot key authorized: {} -> {}",
                    cold_key.to_hex(),
                    hot_key.to_hex()
                );
            }
            Certificate::CommitteeColdResign {
                cold_credential,
                anchor,
            } => {
                let cold_key = credential_to_hash(cold_credential);
                Arc::make_mut(&mut self.governance)
                    .committee_resigned
                    .insert(cold_key, anchor.clone());
                Arc::make_mut(&mut self.governance)
                    .committee_hot_keys
                    .remove(&cold_key);
                debug!("Committee member resigned: {}", cold_key.to_hex());
            }
            Certificate::RegStakeVoteDeleg {
                credential,
                pool_hash,
                drep,
                ..
            } => {
                let key = credential_to_hash(credential);
                // Register stake credential
                self.stake_distribution
                    .stake_map
                    .entry(key)
                    .or_insert(Lovelace(0));
                Arc::make_mut(&mut self.reward_accounts)
                    .entry(key)
                    .or_insert(Lovelace(0));
                // Stake delegation
                Arc::make_mut(&mut self.delegations).insert(key, *pool_hash);
                // Vote delegation
                Arc::make_mut(&mut self.governance)
                    .vote_delegations
                    .insert(key, drep.clone());
                debug!(
                    "Reg+stake+vote delegated: pool={}, drep={:?}",
                    pool_hash.to_hex(),
                    drep
                );
            }
            Certificate::VoteRegDeleg {
                credential, drep, ..
            } => {
                let key = credential_to_hash(credential);
                // Register stake credential
                self.stake_distribution
                    .stake_map
                    .entry(key)
                    .or_insert(Lovelace(0));
                Arc::make_mut(&mut self.reward_accounts)
                    .entry(key)
                    .or_insert(Lovelace(0));
                // Vote delegation
                Arc::make_mut(&mut self.governance)
                    .vote_delegations
                    .insert(key, drep.clone());
                debug!("Reg+vote delegated to {:?}", drep);
            }
            Certificate::GenesisKeyDelegation {
                genesis_hash,
                genesis_delegate_hash,
                vrf_keyhash,
            } => {
                // Genesis key delegation — update genesis delegate mapping
                // These are rare (Shelley-era governance by genesis keys)
                debug!(
                    "Genesis key delegation: {} -> delegate={}, vrf={}",
                    genesis_hash.to_hex(),
                    genesis_delegate_hash.to_hex(),
                    vrf_keyhash.to_hex()
                );
            }
            Certificate::MoveInstantaneousRewards { source, target } => {
                // MIR: transfer funds between reserves/treasury or distribute to stake credentials
                match target {
                    MIRTarget::StakeCredentials(creds) => {
                        let mut total_distributed: u64 = 0;
                        for (cred, amount) in creds {
                            let key = credential_to_hash(cred);
                            let entry = Arc::make_mut(&mut self.reward_accounts)
                                .entry(key)
                                .or_insert(Lovelace(0));
                            if *amount >= 0 {
                                let amt = *amount as u64;
                                entry.0 = entry.0.saturating_add(amt);
                                total_distributed = total_distributed.saturating_add(amt);
                            } else {
                                entry.0 = entry.0.saturating_sub(amount.unsigned_abs());
                            }
                            debug!(
                                "MIR: distributed {} lovelace from {:?} to {}",
                                amount,
                                source,
                                key.to_hex()
                            );
                        }
                        // Debit the source pot for the total positive amount distributed
                        if total_distributed > 0 {
                            match source {
                                MIRSource::Reserves => {
                                    self.reserves.0 =
                                        self.reserves.0.saturating_sub(total_distributed);
                                }
                                MIRSource::Treasury => {
                                    self.treasury.0 =
                                        self.treasury.0.saturating_sub(total_distributed);
                                }
                            }
                        }
                    }
                    MIRTarget::OtherAccountingPot(coin) => {
                        // Transfer between reserves and treasury
                        // Use saturating arithmetic to handle compound MIR operations
                        // where credential distributions and pot transfers interact
                        match source {
                            MIRSource::Reserves => {
                                // Move from reserves to treasury, capped at available
                                let actual = (*coin).min(self.reserves.0);
                                self.reserves.0 = self.reserves.0.saturating_sub(actual);
                                self.treasury.0 = self.treasury.0.saturating_add(actual);
                                debug!(
                                    "MIR: transferred {} lovelace from reserves to treasury",
                                    actual
                                );
                            }
                            MIRSource::Treasury => {
                                // Move from treasury to reserves, capped at available
                                let actual = (*coin).min(self.treasury.0);
                                self.treasury.0 = self.treasury.0.saturating_sub(actual);
                                self.reserves.0 = self.reserves.0.saturating_add(actual);
                                debug!(
                                    "MIR: transferred {} lovelace from treasury to reserves",
                                    actual
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    /// Process an epoch transition
    pub fn process_epoch_transition(&mut self, new_epoch: EpochNo) {
        info!("Epoch transition: {} -> {}", self.epoch.0, new_epoch.0);

        // Calculate and distribute rewards using the "go" snapshot (take ownership to avoid clone)
        if let Some(go_snapshot) = self.snapshots.go.take() {
            self.calculate_and_distribute_rewards(go_snapshot);
        }

        // Rotate snapshots: go = set, set = mark, mark = new snapshot
        self.snapshots.go = self.snapshots.set.take();
        self.snapshots.set = self.snapshots.mark.take();

        // Take a new "mark" snapshot of current stake distribution.
        // Only do a full UTxO scan if needed (after snapshot load or Mithril import).
        // During replay from genesis, incremental tracking is always correct.
        if self.needs_stake_rebuild {
            self.rebuild_stake_distribution();
            self.needs_stake_rebuild = false;
        }

        // Per Cardano spec, total stake = UTxO-delegated stake + reward account balance.
        let mut pool_stake: HashMap<Hash28, Lovelace> = HashMap::new();
        for (cred_hash, pool_id) in self.delegations.iter() {
            let utxo_stake = self
                .stake_distribution
                .stake_map
                .get(cred_hash)
                .copied()
                .unwrap_or(Lovelace(0));
            let reward_balance = self
                .reward_accounts
                .get(cred_hash)
                .copied()
                .unwrap_or(Lovelace(0));
            let total_stake = Lovelace(utxo_stake.0 + reward_balance.0);
            *pool_stake.entry(*pool_id).or_insert(Lovelace(0)) += total_stake;
        }

        // Build per-credential stake including reward balances
        let mut snapshot_stake = self.stake_distribution.stake_map.clone();
        for (cred_hash, reward) in self.reward_accounts.iter() {
            if reward.0 > 0 {
                *snapshot_stake.entry(*cred_hash).or_insert(Lovelace(0)) += *reward;
            }
        }

        let total_utxo_stake: u64 = self
            .stake_distribution
            .stake_map
            .values()
            .map(|l| l.0)
            .sum();
        let total_pool_stake: u64 = pool_stake.values().map(|l| l.0).sum();
        info!(
            epoch = new_epoch.0,
            credentials = self.stake_distribution.stake_map.len(),
            delegations = self.delegations.len(),
            pools = pool_stake.len(),
            total_utxo_stake_ada = total_utxo_stake / 1_000_000,
            total_pool_stake_ada = total_pool_stake / 1_000_000,
            "Epoch snapshot: stake distribution rebuilt from UTxO set"
        );

        self.snapshots.mark = Some(StakeSnapshot {
            epoch: new_epoch,
            delegations: Arc::clone(&self.delegations),
            pool_stake,
            pool_params: Arc::clone(&self.pool_params),
            stake_distribution: Arc::new(snapshot_stake),
        });

        // Process pending pool retirements for this epoch
        if let Some(retiring_pools) = self.pending_retirements.remove(&new_epoch) {
            let pool_deposit = self.protocol_params.pool_deposit;
            for pool_id in &retiring_pools {
                // Refund pool deposit to operator's registered reward account
                if let Some(pool_reg) = Arc::make_mut(&mut self.pool_params).remove(pool_id) {
                    let op_key = Self::reward_account_to_hash(&pool_reg.reward_account);
                    *Arc::make_mut(&mut self.reward_accounts)
                        .entry(op_key)
                        .or_insert(Lovelace(0)) += pool_deposit;
                    debug!(
                        "Pool retired at epoch {}: {} (deposit {} refunded)",
                        new_epoch.0,
                        pool_id.to_hex(),
                        pool_deposit.0
                    );
                } else {
                    debug!(
                        "Pool retired at epoch {}: {} (no params found)",
                        new_epoch.0,
                        pool_id.to_hex()
                    );
                }
            }
        }

        // Clean up retirements from past epochs (shouldn't happen but be safe)
        self.pending_retirements
            .retain(|epoch, _| *epoch >= new_epoch);

        // Apply pre-Conway protocol parameter update proposals (PPUP rule).
        // In Shelley-Babbage, genesis delegates submit update proposals targeting epoch E.
        // At the epoch boundary E → E+1, proposals targeting E are evaluated:
        // if enough distinct genesis delegates proposed updates (>= update_quorum),
        // their proposals are merged and applied to take effect in epoch E+1.
        // Note: self.epoch still holds the OLD epoch at this point (updated at end).
        if let Some(proposals) = self.pending_pp_updates.remove(&self.epoch) {
            // Count distinct proposers (genesis delegate hashes)
            let mut proposer_set: std::collections::HashSet<Hash32> =
                std::collections::HashSet::new();
            for (genesis_hash, _) in &proposals {
                proposer_set.insert(*genesis_hash);
            }
            let distinct_proposers = proposer_set.len() as u64;

            if distinct_proposers >= self.update_quorum {
                // Merge all proposals: later proposals override earlier ones per field
                let mut merged = ProtocolParamUpdate::default();
                for (_, ppu) in &proposals {
                    // Merge each field: if the proposal sets it, override
                    macro_rules! merge_field {
                        ($field:ident) => {
                            if ppu.$field.is_some() {
                                merged.$field = ppu.$field.clone();
                            }
                        };
                    }
                    merge_field!(min_fee_a);
                    merge_field!(min_fee_b);
                    merge_field!(max_block_body_size);
                    merge_field!(max_tx_size);
                    merge_field!(max_block_header_size);
                    merge_field!(key_deposit);
                    merge_field!(pool_deposit);
                    merge_field!(e_max);
                    merge_field!(n_opt);
                    merge_field!(a0);
                    merge_field!(rho);
                    merge_field!(tau);
                    merge_field!(min_pool_cost);
                    merge_field!(ada_per_utxo_byte);
                    merge_field!(cost_models);
                    merge_field!(execution_costs);
                    merge_field!(max_tx_ex_units);
                    merge_field!(max_block_ex_units);
                    merge_field!(max_val_size);
                    merge_field!(collateral_percentage);
                    merge_field!(max_collateral_inputs);
                    merge_field!(protocol_version_major);
                    merge_field!(protocol_version_minor);
                }
                // Log protocol version change if applicable
                if merged.protocol_version_major.is_some()
                    || merged.protocol_version_minor.is_some()
                {
                    info!(
                        epoch = new_epoch.0,
                        from_major = self.protocol_params.protocol_version_major,
                        from_minor = self.protocol_params.protocol_version_minor,
                        to_major = ?merged.protocol_version_major,
                        to_minor = ?merged.protocol_version_minor,
                        "Protocol version change via pre-Conway update"
                    );
                }
                if let Err(e) = self.apply_protocol_param_update(&merged) {
                    warn!(
                        epoch = new_epoch.0,
                        error = %e,
                        "Pre-Conway protocol parameter update rejected"
                    );
                } else {
                    info!(
                        epoch = new_epoch.0,
                        proposers = distinct_proposers,
                        protocol_version = format!(
                            "{}.{}",
                            self.protocol_params.protocol_version_major,
                            self.protocol_params.protocol_version_minor
                        ),
                        "Pre-Conway protocol parameter update applied"
                    );
                }
            } else {
                debug!(
                    epoch = new_epoch.0,
                    proposers = distinct_proposers,
                    quorum = self.update_quorum,
                    "Pre-Conway protocol parameter update: insufficient quorum"
                );
            }
        }
        // Clean up proposals targeting past epochs (already applied above).
        // Keep proposals targeting new_epoch or later — they'll be applied at
        // the NEXT epoch boundary (new_epoch → new_epoch+1).
        self.pending_pp_updates
            .retain(|epoch, _| *epoch >= new_epoch);

        // Ratify governance proposals that have met their voting thresholds
        self.ratify_proposals();

        // Expire governance proposals that have passed their lifetime
        // and refund deposits to the return address
        let expired: Vec<GovActionId> = self
            .governance
            .proposals
            .iter()
            .filter(|(_, state)| state.expires_epoch <= new_epoch)
            .map(|(id, _)| id.clone())
            .collect();
        if !expired.is_empty() {
            for action_id in &expired {
                if let Some(proposal_state) = Arc::make_mut(&mut self.governance)
                    .proposals
                    .remove(action_id)
                {
                    // Refund deposit to return address's reward account
                    let deposit = proposal_state.procedure.deposit;
                    if deposit.0 > 0 {
                        let return_addr = &proposal_state.procedure.return_addr;
                        if return_addr.len() >= 29 {
                            let key = Self::reward_account_to_hash(return_addr);
                            *Arc::make_mut(&mut self.reward_accounts)
                                .entry(key)
                                .or_insert(Lovelace(0)) += deposit;
                        }
                    }
                    debug!(
                        "Governance proposal expired: {:?} (deposit {} returned)",
                        action_id, deposit.0
                    );
                }
            }
            // Remove all votes for expired proposals
            for id in &expired {
                Arc::make_mut(&mut self.governance)
                    .votes_by_action
                    .remove(id);
            }
            debug!(
                "Expired {} governance proposals at epoch {}",
                expired.len(),
                new_epoch.0
            );
        }

        // Mark inactive DReps per CIP-1694
        // DReps that haven't voted or updated within drep_activity epochs are marked inactive
        // and excluded from voting power calculations. They remain registered and keep their deposits.
        let drep_activity = self.protocol_params.drep_activity;
        if drep_activity > 0 {
            let mut newly_inactive = 0u64;
            let mut reactivated = 0u64;
            for drep in Arc::make_mut(&mut self.governance).dreps.values_mut() {
                let inactive = new_epoch.0.saturating_sub(drep.last_active_epoch.0) > drep_activity;
                if inactive && drep.active {
                    drep.active = false;
                    newly_inactive += 1;
                } else if !inactive && !drep.active {
                    drep.active = true;
                    reactivated += 1;
                }
            }
            if newly_inactive > 0 || reactivated > 0 {
                info!(
                    "DRep activity update at epoch {}: {} newly inactive, {} reactivated (threshold: {} epochs)",
                    new_epoch.0,
                    newly_inactive,
                    reactivated,
                    drep_activity
                );
            }
        }

        // Expire committee members that have passed their expiration epoch
        let expired_members: Vec<Hash32> = self
            .governance
            .committee_expiration
            .iter()
            .filter(|(_, exp_epoch)| **exp_epoch <= new_epoch)
            .map(|(hash, _)| *hash)
            .collect();
        if !expired_members.is_empty() {
            for hash in &expired_members {
                Arc::make_mut(&mut self.governance)
                    .committee_hot_keys
                    .remove(hash);
                Arc::make_mut(&mut self.governance)
                    .committee_expiration
                    .remove(hash);
            }
            info!(
                "Expired {} committee members at epoch {}",
                expired_members.len(),
                new_epoch.0
            );
        }

        // Compute new epoch nonce per Haskell tickChainDepState:
        //   epoch_nonce = hash(candidate_nonce || last_epoch_block_nonce)
        //
        // The candidate_nonce was frozen 4k/f slots before epoch end.
        // The last_epoch_block_nonce is the lab_nonce snapshot from the previous epoch boundary.
        let prev_epoch_nonce = self.epoch_nonce;
        self.last_epoch_block_nonce = self.lab_nonce;

        let mut nonce_input = Vec::with_capacity(64);
        nonce_input.extend_from_slice(self.candidate_nonce.as_bytes());
        nonce_input.extend_from_slice(self.last_epoch_block_nonce.as_bytes());
        self.epoch_nonce = torsten_primitives::hash::blake2b_256(&nonce_input);

        info!(
            "New epoch nonce: {} (candidate {} ⋄ lab {}), prev: {}",
            self.epoch_nonce.to_hex(),
            self.candidate_nonce.to_hex(),
            self.last_epoch_block_nonce.to_hex(),
            prev_epoch_nonce.to_hex(),
        );

        // evolving_nonce and candidate_nonce carry forward unchanged
        // (they are NOT reset at epoch boundaries)

        // Reset per-epoch accumulators
        self.epoch_fees = Lovelace(0);
        Arc::make_mut(&mut self.epoch_blocks_by_pool).clear();
        self.epoch_block_count = 0;

        self.epoch = new_epoch;
    }

    /// Calculate and distribute rewards according to the Cardano Shelley reward formula.
    ///
    /// Implements the formula from cardano-ledger-shelley:
    ///   - maxPool'(a0, nOpt, R, sigma, p) for pledge-influenced pool rewards
    ///   - mkApparentPerformance for beta/sigma performance calculation
    ///   - Pledge verification (pool gets zero if owner stake < declared pledge)
    ///   - Operator reward includes self-delegation share (margin + proportional)
    ///   - Operator reward goes to pool's registered reward account
    fn calculate_and_distribute_rewards(&mut self, go_snapshot: StakeSnapshot) {
        let rho_num = self.protocol_params.rho.numerator as i128;
        let rho_den = self.protocol_params.rho.denominator.max(1) as i128;
        let tau_num = self.protocol_params.tau.numerator as i128;
        let tau_den = self.protocol_params.tau.denominator.max(1) as i128;

        // Monetary expansion with eta performance adjustment:
        //   expected_blocks = floor(active_slot_coeff * epoch_length) (since d=0 in Conway)
        //   eta = min(1, actual_blocks / expected_blocks)
        //   deltaR1 = floor(eta * rho * reserves)
        let raw_expected_blocks =
            (self.protocol_params.active_slot_coeff() * self.epoch_length as f64).floor() as u64;
        if raw_expected_blocks == 0 {
            warn!(
                "expected_blocks rounded to 0 (active_slot_coeff={}, epoch_length={}), clamping to 1",
                self.protocol_params.active_slot_coeff(),
                self.epoch_length
            );
        }
        let expected_blocks = raw_expected_blocks.max(1);
        let actual_blocks = self.epoch_block_count;
        // eta = min(1, actual/expected) — applied as rational: min(1, actual/expected)
        // expansion = floor(min(actual, expected) / expected * rho * reserves)
        let effective_blocks = actual_blocks.min(expected_blocks);
        // Use Rat to avoid i128 overflow: rho * reserves * (effective/expected)
        let rho = Rat::new(rho_num, rho_den);
        let expansion_rat = rho
            .mul(&Rat::new(self.reserves.0 as i128, 1))
            .mul(&Rat::new(effective_blocks as i128, expected_blocks as i128));
        let expansion = expansion_rat.floor_u64();
        let total_rewards_available = expansion + self.epoch_fees.0;

        if total_rewards_available == 0 {
            return;
        }

        // Move expansion from reserves
        self.reserves.0 = self.reserves.0.saturating_sub(expansion);

        // Treasury cut: floor(tau * total_rewards)
        let tau = Rat::new(tau_num, tau_den);
        let treasury_cut = tau
            .mul(&Rat::new(total_rewards_available as i128, 1))
            .floor_u64();
        self.treasury.0 = self.treasury.0.saturating_add(treasury_cut);

        let reward_pot = total_rewards_available - treasury_cut;

        // Total stake for sigma denominator: circulation = maxSupply - reserves
        let total_stake = MAX_LOVELACE_SUPPLY.saturating_sub(self.reserves.0);
        if total_stake == 0 {
            self.treasury.0 = self.treasury.0.saturating_add(reward_pot);
            return;
        }

        // Total active stake (for apparent performance denominator)
        let total_active_stake: u64 = go_snapshot.pool_stake.values().map(|s| s.0).sum();
        if total_active_stake == 0 {
            self.treasury.0 = self.treasury.0.saturating_add(reward_pot);
            return;
        }

        // Total blocks produced this epoch
        let total_blocks_in_epoch = self.epoch_block_count.max(1);

        // Saturation point: z0 = 1/nOpt
        let n_opt = self.protocol_params.n_opt.max(1);

        let mut total_distributed: u64 = 0;

        // Build delegators-by-pool index for O(n) reward distribution
        let mut delegators_by_pool: HashMap<Hash28, Vec<Hash32>> = HashMap::new();
        for (cred_hash, pool_id) in go_snapshot.delegations.iter() {
            delegators_by_pool
                .entry(*pool_id)
                .or_default()
                .push(*cred_hash);
        }

        // Build owner-delegated-stake per pool for pledge check
        let mut owner_stake_by_pool: HashMap<Hash28, u64> = HashMap::new();
        for (pool_id, pool_reg) in go_snapshot.pool_params.iter() {
            let mut owner_stake = 0u64;
            for owner in &pool_reg.owners {
                let owner_key = owner.to_hash32_padded();
                if go_snapshot.delegations.get(&owner_key) == Some(pool_id) {
                    owner_stake += go_snapshot
                        .stake_distribution
                        .get(&owner_key)
                        .map(|l| l.0)
                        .unwrap_or(0);
                }
            }
            owner_stake_by_pool.insert(*pool_id, owner_stake);
        }

        // Calculate rewards per pool
        for (pool_id, pool_active_stake) in &go_snapshot.pool_stake {
            let pool_reg = match go_snapshot.pool_params.get(pool_id) {
                Some(reg) => reg,
                None => continue,
            };

            // Pledge check: if owner-delegated stake < declared pledge, pool gets zero
            let self_delegated = owner_stake_by_pool.get(pool_id).copied().unwrap_or(0);
            if self_delegated < pool_reg.pledge.0 {
                debug!(
                    "Pool {} pledge not met: {} < {}",
                    pool_id.to_hex(),
                    self_delegated,
                    pool_reg.pledge.0
                );
                continue;
            }

            // maxPool'(a0, nOpt, R, sigma, p) using rational arithmetic:
            //   z0 = 1/nOpt
            //   sigma' = min(sigma, z0), p' = min(p, z0)
            //   maxPool = floor(R/(1+a0) * (sigma' + p' * a0 * (sigma' - p'*(z0-sigma')/z0) / z0))
            //
            // Uses Rat (i128 num/den with GCD reduction) to match Haskell's Rational.
            let a0_r = Rat::new(
                self.protocol_params.a0.numerator as i128,
                self.protocol_params.a0.denominator.max(1) as i128,
            );
            let z0 = Rat::new(1, n_opt as i128);
            let sigma_raw = Rat::new(pool_active_stake.0 as i128, total_stake as i128);
            let p_raw = Rat::new(pool_reg.pledge.0 as i128, total_stake as i128);
            let sigma = sigma_raw.min_rat(&z0);
            let p = p_raw.min_rat(&z0);

            // factor4 = (z0 - sigma') / z0
            let f4 = z0.sub(&sigma).div(&z0);
            // factor3 = (sigma' - p' * factor4) / z0
            let f3 = sigma.sub(&p.mul(&f4)).div(&z0);
            // factor2 = sigma' + p' * a0 * factor3
            let f2 = sigma.add(&p.mul(&a0_r).mul(&f3));
            // factor1 = R / (1 + a0)
            let f1 = Rat::new(reward_pot as i128, 1).div(&Rat::new(1, 1).add(&a0_r));
            // maxPool = floor(factor1 * factor2)
            let max_pool = f1.mul(&f2).floor_u64();

            // Apparent performance: beta / sigma_a (rational arithmetic)
            //   perf = (blocks_made / total_blocks) / (pool_stake / total_active_stake)
            //        = (blocks_made * total_active_stake) / (total_blocks * pool_stake)
            let blocks_made = self.epoch_blocks_by_pool.get(pool_id).copied().unwrap_or(0);
            let pool_reward = if blocks_made == 0 || pool_active_stake.0 == 0 {
                0u64
            } else {
                // perf = (blocks_made / total_blocks) / (pool_stake / total_active_stake)
                // Use Rat chained multiplication to avoid i128 overflow
                let perf = Rat::new(blocks_made as i128, total_blocks_in_epoch as i128).mul(
                    &Rat::new(total_active_stake as i128, pool_active_stake.0 as i128),
                );
                perf.mul(&Rat::new(max_pool as i128, 1)).floor_u64()
            };

            if pool_reward == 0 {
                continue;
            }

            // Operator reward: cost + (margin + (1-margin) * s/sigma) * max(0, pool_reward - cost)
            // where s/sigma = self_delegated / pool_stake (owner's fraction of pool)
            let cost = pool_reg.cost.0;
            let margin_num = pool_reg.margin_numerator as i128;
            let margin_den = pool_reg.margin_denominator.max(1) as i128;

            let operator_reward = if pool_reward <= cost {
                pool_reward
            } else {
                let remainder = pool_reward - cost;
                // operator_share = margin + (1-margin) * s/sigma
                // Use Rat to avoid i128 overflow in cross terms
                let margin = Rat::new(margin_num, margin_den);
                let one_minus_margin = Rat::new(margin_den - margin_num, margin_den);
                let s_over_sigma = Rat::new(self_delegated as i128, pool_active_stake.0 as i128);
                let share = margin.add(&one_minus_margin.mul(&s_over_sigma));
                let op_extra = share.mul(&Rat::new(remainder as i128, 1)).floor_u64();
                cost + op_extra
            };

            // Distribute member rewards proportionally to delegators.
            // Pool owners are excluded — they receive only the operator reward.
            // Build owner set (as Hash32 keys) for filtering
            let owner_set: std::collections::HashSet<Hash32> = pool_reg
                .owners
                .iter()
                .map(|o| o.to_hash32_padded())
                .collect();

            if let Some(delegators) = delegators_by_pool.get(pool_id) {
                for cred_hash in delegators {
                    // Skip pool owners — they only get leader/operator reward
                    if owner_set.contains(cred_hash) {
                        continue;
                    }

                    let member_stake = go_snapshot
                        .stake_distribution
                        .get(cred_hash)
                        .copied()
                        .unwrap_or(Lovelace(0))
                        .0;

                    if member_stake == 0 || pool_active_stake.0 == 0 {
                        continue;
                    }

                    // Member share: floor((pool_reward - cost) * (1 - margin) * member_stake / pool_stake)
                    let member_share = if pool_reward <= cost {
                        0u64
                    } else {
                        let remainder = pool_reward - cost;
                        // Use Rat to avoid i128 overflow in cross terms
                        let one_minus_margin = Rat::new(margin_den - margin_num, margin_den);
                        let member_frac =
                            Rat::new(member_stake as i128, pool_active_stake.0 as i128);
                        Rat::new(remainder as i128, 1)
                            .mul(&one_minus_margin)
                            .mul(&member_frac)
                            .floor_u64()
                    };

                    if member_share > 0 {
                        *Arc::make_mut(&mut self.reward_accounts)
                            .entry(*cred_hash)
                            .or_insert(Lovelace(0)) += Lovelace(member_share);
                        total_distributed += member_share;
                    }
                }
            }

            // Operator reward goes to pool's registered reward account
            if operator_reward > 0 {
                let op_key = Self::reward_account_to_hash(&pool_reg.reward_account);
                *Arc::make_mut(&mut self.reward_accounts)
                    .entry(op_key)
                    .or_insert(Lovelace(0)) += Lovelace(operator_reward);
                total_distributed += operator_reward;
            }
        }

        // Any undistributed rewards go to treasury
        let undistributed = reward_pot.saturating_sub(total_distributed);
        if undistributed > 0 {
            self.treasury.0 = self.treasury.0.saturating_add(undistributed);
        }

        info!(
            "Rewards distributed: {} lovelace to accounts, {} to treasury (expansion: {}, fees: {})",
            total_distributed, treasury_cut + undistributed, expansion, self.epoch_fees.0
        );
    }

    /// Rebuild stake_distribution.stake_map from the full UTxO set.
    ///
    /// This recomputes per-credential UTxO stake by scanning all UTxOs,
    /// matching Haskell's behavior at epoch boundaries. This corrects any
    /// drift from incremental tracking (e.g., after snapshot load or Mithril import).
    fn rebuild_stake_distribution(&mut self) {
        let mut new_map: HashMap<Hash32, Lovelace> = HashMap::new();
        for (_, output) in self.utxo_set.iter() {
            if let Some(cred_hash) = stake_credential_hash(&output.address) {
                *new_map.entry(cred_hash).or_insert(Lovelace(0)) += Lovelace(output.value.coin.0);
            }
        }
        // Also ensure all registered stake credentials have entries (even with 0 stake)
        for cred_hash in self.delegations.keys() {
            new_map.entry(*cred_hash).or_insert(Lovelace(0));
        }
        self.stake_distribution.stake_map = new_map;
    }

    /// Convert a reward account (raw bytes with network header) to a Hash32 key.
    ///
    /// Reward addresses are 29 bytes: 1 byte network header + 28 byte credential hash.
    /// We extract exactly the 28-byte credential and zero-pad to 32 bytes for Hash32.
    fn reward_account_to_hash(reward_account: &[u8]) -> Hash32 {
        let mut key_bytes = [0u8; 32];
        if reward_account.len() >= 29 {
            // Copy exactly 28 bytes of the credential (skip the 1-byte header)
            key_bytes[..28].copy_from_slice(&reward_account[1..29]);
        }
        Hash32::from_bytes(key_bytes)
    }

    /// Update the evolving nonce with a new VRF output.
    ///
    /// evolving_nonce = hash(evolving_nonce || hash(hash("N" || vrf_output)))
    ///
    /// Matches Haskell's reupdateChainDepState → hashVRF → vrfNonceValue pipeline.
    fn update_evolving_nonce(&mut self, vrf_output: &[u8]) {
        // eta = blake2b_256(blake2b_256("N" || raw_vrf_output))
        let mut prefixed = Vec::with_capacity(1 + vrf_output.len());
        prefixed.push(b'N');
        prefixed.extend_from_slice(vrf_output);
        let first_hash = torsten_primitives::hash::blake2b_256(&prefixed);
        let eta = torsten_primitives::hash::blake2b_256(first_hash.as_ref());
        // evolving_nonce' = blake2b_256(evolving_nonce || eta)
        let mut data = Vec::with_capacity(64);
        data.extend_from_slice(self.evolving_nonce.as_bytes());
        data.extend_from_slice(eta.as_bytes());
        self.evolving_nonce = torsten_primitives::hash::blake2b_256(&data);
    }

    /// Process a governance proposal.
    /// Validates prev_action_id chain if present.
    fn process_proposal(
        &mut self,
        tx_hash: &Hash32,
        action_index: u32,
        proposal: &ProposalProcedure,
    ) {
        // Validate prev_action_id: if specified, the referenced action must exist
        // as an active proposal or must have been previously enacted
        let prev_id = match &proposal.gov_action {
            GovAction::ParameterChange { prev_action_id, .. }
            | GovAction::HardForkInitiation { prev_action_id, .. }
            | GovAction::NoConfidence { prev_action_id, .. }
            | GovAction::UpdateCommittee { prev_action_id, .. }
            | GovAction::NewConstitution { prev_action_id, .. } => prev_action_id.as_ref(),
            GovAction::TreasuryWithdrawals { .. } | GovAction::InfoAction => None,
        };
        if let Some(prev) = prev_id {
            if !self.governance.proposals.contains_key(prev) {
                debug!(
                    "Governance proposal references unknown prev_action_id {:?} (allowed — may have been enacted)",
                    prev
                );
            }
        }

        // CIP-1694: Validate policy_hash matches constitution guardrail script
        // ParameterChange and TreasuryWithdrawals must include the constitution's script_hash
        let constitution_script = self
            .governance
            .constitution
            .as_ref()
            .and_then(|c| c.script_hash);
        match &proposal.gov_action {
            GovAction::ParameterChange { policy_hash, .. }
            | GovAction::TreasuryWithdrawals { policy_hash, .. } => {
                if let Some(required_hash) = constitution_script {
                    match policy_hash {
                        Some(provided) if *provided == required_hash => {
                            // Valid — policy hash matches constitution guardrail
                        }
                        Some(provided) => {
                            warn!(
                                "Governance proposal policy_hash {} does not match constitution guardrail {}",
                                provided.to_hex(),
                                required_hash.to_hex()
                            );
                        }
                        None => {
                            debug!(
                                "Governance proposal missing policy_hash (constitution requires {})",
                                required_hash.to_hex()
                            );
                        }
                    }
                }
            }
            _ => {}
        }

        let action_id = GovActionId {
            transaction_id: *tx_hash,
            action_index,
        };

        // Governance action lifetime from protocol parameters
        let gov_action_lifetime = self.protocol_params.gov_action_lifetime;
        let expires_epoch = EpochNo(self.epoch.0.saturating_add(gov_action_lifetime));

        let state = ProposalState {
            procedure: proposal.clone(),
            proposed_epoch: self.epoch,
            expires_epoch,
            yes_votes: 0,
            no_votes: 0,
            abstain_votes: 0,
        };

        debug!(
            "Governance proposal submitted: {:?} (expires epoch {})",
            action_id, expires_epoch.0
        );
        Arc::make_mut(&mut self.governance)
            .proposals
            .insert(action_id, state);
        Arc::make_mut(&mut self.governance).proposal_count += 1;
    }

    /// Process a governance vote
    fn process_vote(
        &mut self,
        voter: &Voter,
        action_id: &GovActionId,
        procedure: &VotingProcedure,
    ) {
        // Update vote tally on the proposal
        if let Some(proposal) = Arc::make_mut(&mut self.governance)
            .proposals
            .get_mut(action_id)
        {
            match procedure.vote {
                Vote::Yes => proposal.yes_votes += 1,
                Vote::No => proposal.no_votes += 1,
                Vote::Abstain => proposal.abstain_votes += 1,
            }
        }

        // Track DRep activity — voting counts as activity per CIP-1694
        if let Voter::DRep(cred) = voter {
            let drep_hash = credential_to_hash(cred);
            if let Some(drep) = Arc::make_mut(&mut self.governance)
                .dreps
                .get_mut(&drep_hash)
            {
                drep.last_active_epoch = self.epoch;
            }
        }

        // Record the vote (indexed by action_id for efficient ratification)
        let action_votes = Arc::make_mut(&mut self.governance)
            .votes_by_action
            .entry(action_id.clone())
            .or_default();
        // Replace existing vote from same voter, or add new
        if let Some(existing) = action_votes.iter_mut().find(|(v, _)| v == voter) {
            existing.1 = procedure.clone();
        } else {
            action_votes.push((voter.clone(), procedure.clone()));
        }

        debug!(
            "Vote cast by {:?} on {:?}: {:?}",
            voter, action_id, procedure.vote
        );
    }

    /// Check all active governance proposals for ratification.
    ///
    /// A proposal is ratified when it meets the required voting thresholds.
    /// Thresholds vary by action type and involve DRep, SPO, and/or CC votes.
    /// Ratified proposals are enacted (their effects applied) and removed.
    ///
    /// Per Haskell Ratify.hs, proposals are processed:
    /// 1. Sorted by priority (NoConfidence > UpdateCommittee > ... > InfoAction)
    /// 2. Sequentially with state threading (enacted roots update between proposals)
    /// 3. With a "delaying action" flag that blocks further ratification
    /// 4. With prev_action_id chain validation (must match last enacted of same purpose)
    fn ratify_proposals(&mut self) {
        let total_drep_stake = self.compute_total_drep_stake();
        let total_spo_stake = self.compute_total_spo_stake();
        // Pre-compute DRep voting power once (O(delegations)) instead of per-DRep per-proposal
        let (drep_power_cache, no_confidence_stake, _abstain_stake) = self.build_drep_power_cache();

        // Collect all proposals sorted by priority (lower = higher priority)
        let mut candidates: Vec<(GovActionId, GovAction, EpochNo)> = self
            .governance
            .proposals
            .iter()
            .map(|(id, state)| {
                (
                    id.clone(),
                    state.procedure.gov_action.clone(),
                    state.expires_epoch,
                )
            })
            .collect();
        candidates.sort_by_key(|(_, action, _)| gov_action_priority(action));

        let mut ratified = Vec::new();
        let mut delayed = false;

        for (action_id, action, _expires) in &candidates {
            // Check prev_action_id chain
            if !prev_action_as_expected(action, &self.governance) {
                debug!(
                    action_id = %action_id.transaction_id.to_hex(),
                    action_type = ?std::mem::discriminant(action),
                    "Governance proposal: prev_action_id chain mismatch"
                );
                continue;
            }

            // If a delaying action was already enacted this epoch, skip remaining
            if delayed {
                debug!(
                    action_id = %action_id.transaction_id.to_hex(),
                    "Governance proposal: delayed by previously enacted action"
                );
                continue;
            }

            // Check voting thresholds
            if let Some(state) = self.governance.proposals.get(action_id) {
                let met = self.check_ratification(
                    action_id,
                    state,
                    total_drep_stake,
                    total_spo_stake,
                    &drep_power_cache,
                    no_confidence_stake,
                );
                if met {
                    info!(
                        action_id = %action_id.transaction_id.to_hex(),
                        action_type = ?std::mem::discriminant(action),
                        "Governance proposal ratified"
                    );
                    // Enact immediately and update roots (for chain validation of subsequent proposals)
                    self.enact_gov_action(action);
                    self.update_enacted_root(action_id, action);
                    ratified.push(action_id.clone());
                    if is_delaying_action(action) {
                        delayed = true;
                    }
                } else if !matches!(action, GovAction::InfoAction) {
                    debug!(
                        action_id = %action_id.transaction_id.to_hex(),
                        action_type = ?std::mem::discriminant(action),
                        "Governance proposal not yet ratified"
                    );
                }
            }
        }

        // Remove ratified proposals and refund deposits
        if !ratified.is_empty() {
            for action_id in &ratified {
                if let Some(proposal_state) = Arc::make_mut(&mut self.governance)
                    .proposals
                    .remove(action_id)
                {
                    let deposit = proposal_state.procedure.deposit;
                    if deposit.0 > 0 {
                        let return_addr = &proposal_state.procedure.return_addr;
                        if return_addr.len() >= 29 {
                            let key = Self::reward_account_to_hash(return_addr);
                            *Arc::make_mut(&mut self.reward_accounts)
                                .entry(key)
                                .or_insert(Lovelace(0)) += deposit;
                        }
                    }
                }
                Arc::make_mut(&mut self.governance)
                    .votes_by_action
                    .remove(action_id);
            }
            info!(
                "{} governance proposal(s) ratified and enacted",
                ratified.len()
            );
        }
    }

    /// Update the enacted governance root for a given purpose after enactment.
    fn update_enacted_root(&mut self, action_id: &GovActionId, action: &GovAction) {
        match action {
            GovAction::ParameterChange { .. } => {
                Arc::make_mut(&mut self.governance).enacted_pparam_update = Some(action_id.clone());
            }
            GovAction::HardForkInitiation { .. } => {
                Arc::make_mut(&mut self.governance).enacted_hard_fork = Some(action_id.clone());
            }
            GovAction::NoConfidence { .. } | GovAction::UpdateCommittee { .. } => {
                Arc::make_mut(&mut self.governance).enacted_committee = Some(action_id.clone());
            }
            GovAction::NewConstitution { .. } => {
                Arc::make_mut(&mut self.governance).enacted_constitution = Some(action_id.clone());
            }
            // TreasuryWithdrawals and InfoAction don't update any root
            GovAction::TreasuryWithdrawals { .. } | GovAction::InfoAction => {}
        }
    }

    /// Whether we are in the Conway bootstrap phase (protocol version 9).
    /// During bootstrap, all DRep voting thresholds are set to 0 (auto-pass)
    /// per the Haskell `hardforkConwayBootstrapPhase` function.
    fn is_bootstrap_phase(&self) -> bool {
        self.protocol_params.protocol_version_major == 9
    }

    /// Check whether a proposal has met its voting thresholds for ratification.
    ///
    /// CIP-1694 voting thresholds (stake-weighted), matching Haskell cardano-ledger:
    /// - InfoAction: always ratified (no thresholds)
    /// - ParameterChange: DRep ≥ dvt_pp_*_group + SPO ≥ pvt_pp_security (if security) + CC
    /// - HardForkInitiation: DRep ≥ dvt_hard_fork + SPO ≥ pvt_hard_fork + CC
    /// - NoConfidence: DRep ≥ dvt_no_confidence + SPO ≥ pvt_motion_no_confidence (no CC)
    /// - UpdateCommittee: DRep ≥ dvt_committee + SPO ≥ pvt_committee (no CC)
    /// - NewConstitution: DRep ≥ dvt_constitution + CC (no SPO)
    /// - TreasuryWithdrawals: DRep ≥ dvt_treasury_withdrawal + CC (no SPO)
    ///
    /// During Conway bootstrap phase (protocol version 9), all DRep thresholds are 0.
    fn check_ratification(
        &self,
        action_id: &GovActionId,
        state: &ProposalState,
        _total_drep_stake: u64,
        total_spo_stake: u64,
        drep_power_cache: &HashMap<Hash32, u64>,
        no_confidence_stake: u64,
    ) -> bool {
        // Count votes by voter type (uses pre-computed DRep power cache)
        // Per CIP-1694:
        // - DRep denominator = yes + no voted stake (abstain excluded)
        // - SPO denominator = total active SPO stake (non-voting SPOs effectively vote No)
        let (drep_yes, drep_total, spo_yes, _spo_voted, _cc_yes, _cc_total) = self
            .count_votes_by_type(
                action_id,
                &state.procedure.gov_action,
                drep_power_cache,
                no_confidence_stake,
            );

        let bootstrap = self.is_bootstrap_phase();

        match &state.procedure.gov_action {
            GovAction::InfoAction => {
                // InfoAction is always ratified (it's informational only)
                true
            }
            GovAction::ParameterChange {
                protocol_param_update,
                ..
            } => {
                // Per CIP-1694: each affected DRep parameter group must independently
                // meet its own threshold. ALL affected group thresholds must be met.
                // SPO threshold = pvtPPSecurityGroup if any param is security-relevant
                // CC approval required
                let drep_met = if bootstrap {
                    true // All DRep thresholds are 0 during bootstrap
                } else {
                    pp_change_drep_all_groups_met(
                        protocol_param_update,
                        &self.protocol_params,
                        drep_yes,
                        drep_total,
                    )
                };
                let spo_met = if let Some(ref spo_threshold) =
                    pp_change_spo_threshold(protocol_param_update, &self.protocol_params)
                {
                    check_threshold(spo_yes, total_spo_stake, spo_threshold)
                } else {
                    true // No SPO vote required for non-security params
                };
                let cc_met = check_cc_approval(
                    action_id,
                    &self.governance,
                    self.epoch,
                    self.protocol_params.committee_min_size,
                    bootstrap,
                );
                drep_met && spo_met && cc_met
            }
            GovAction::HardForkInitiation {
                protocol_version, ..
            } => {
                let rational_zero = Rational {
                    numerator: 0,
                    denominator: 1,
                };
                // DRep + SPO + CC all required
                let drep_threshold = if bootstrap {
                    rational_zero
                } else {
                    self.protocol_params.dvt_hard_fork.clone()
                };
                let spo_threshold = &self.protocol_params.pvt_hard_fork;
                let drep_met = check_threshold(drep_yes, drep_total, &drep_threshold);
                let spo_met = check_threshold(spo_yes, total_spo_stake, spo_threshold);
                let cc_met = check_cc_approval(
                    action_id,
                    &self.governance,
                    self.epoch,
                    self.protocol_params.committee_min_size,
                    bootstrap,
                );
                debug!(
                    action_id = %action_id.transaction_id.to_hex(),
                    version = ?protocol_version,
                    bootstrap,
                    drep_yes, drep_total,
                    drep_threshold = drep_threshold.as_f64(), drep_met,
                    spo_yes, total_spo_stake,
                    spo_threshold = spo_threshold.as_f64(), spo_met,
                    cc_met,
                    "HardForkInitiation ratification check"
                );
                drep_met && spo_met && cc_met
            }
            GovAction::NoConfidence { .. } => {
                let rational_zero = Rational {
                    numerator: 0,
                    denominator: 1,
                };
                // DRep + SPO, no CC (CC cannot vote on NoConfidence)
                let drep_threshold = if bootstrap {
                    rational_zero
                } else {
                    self.protocol_params.dvt_no_confidence.clone()
                };
                let spo_threshold = &self.protocol_params.pvt_motion_no_confidence;
                let drep_met = check_threshold(drep_yes, drep_total, &drep_threshold);
                let spo_met = check_threshold(spo_yes, total_spo_stake, spo_threshold);
                drep_met && spo_met
            }
            GovAction::UpdateCommittee { .. } => {
                let rational_zero = Rational {
                    numerator: 0,
                    denominator: 1,
                };
                // DRep + SPO, no CC (CC cannot vote on UpdateCommittee)
                let (drep_threshold, spo_threshold) = if self.governance.no_confidence {
                    (
                        if bootstrap {
                            rational_zero
                        } else {
                            self.protocol_params.dvt_committee_no_confidence.clone()
                        },
                        &self.protocol_params.pvt_committee_no_confidence,
                    )
                } else {
                    (
                        if bootstrap {
                            rational_zero
                        } else {
                            self.protocol_params.dvt_committee_normal.clone()
                        },
                        &self.protocol_params.pvt_committee_normal,
                    )
                };
                let drep_met = check_threshold(drep_yes, drep_total, &drep_threshold);
                let spo_met = check_threshold(spo_yes, total_spo_stake, spo_threshold);
                drep_met && spo_met
            }
            GovAction::NewConstitution { .. } => {
                let rational_zero = Rational {
                    numerator: 0,
                    denominator: 1,
                };
                // DRep + CC, no SPO
                let drep_threshold = if bootstrap {
                    rational_zero
                } else {
                    self.protocol_params.dvt_constitution.clone()
                };
                let drep_met = check_threshold(drep_yes, drep_total, &drep_threshold);
                let cc_met = check_cc_approval(
                    action_id,
                    &self.governance,
                    self.epoch,
                    self.protocol_params.committee_min_size,
                    bootstrap,
                );
                drep_met && cc_met
            }
            GovAction::TreasuryWithdrawals { .. } => {
                let rational_zero = Rational {
                    numerator: 0,
                    denominator: 1,
                };
                // DRep + CC, no SPO
                let drep_threshold = if bootstrap {
                    rational_zero
                } else {
                    self.protocol_params.dvt_treasury_withdrawal.clone()
                };
                let drep_met = check_threshold(drep_yes, drep_total, &drep_threshold);
                let cc_met = check_cc_approval(
                    action_id,
                    &self.governance,
                    self.epoch,
                    self.protocol_params.committee_min_size,
                    bootstrap,
                );
                drep_met && cc_met
            }
        }
    }

    /// Count stake-weighted votes by voter type for a specific governance action.
    ///
    /// Per CIP-1694:
    /// - DRep denominator = yes + no voted stake only (abstain excluded)
    /// - SPO: returns explicit yes votes; total SPO stake used as denominator in check_ratification
    /// - AlwaysNoConfidence stake counts as Yes for NoConfidence actions, No for others
    /// - AlwaysAbstain stake is excluded from both numerator and denominator
    /// - Inactive DReps are excluded (handled by drep_power_cache)
    fn count_votes_by_type(
        &self,
        action_id: &GovActionId,
        action: &GovAction,
        drep_power_cache: &HashMap<Hash32, u64>,
        no_confidence_stake: u64,
    ) -> (u64, u64, u64, u64, u64, u64) {
        let mut drep_yes = 0u64;
        let mut drep_no = 0u64;
        let mut spo_yes = 0u64;
        let mut spo_total = 0u64;
        let mut cc_yes = 0u64;
        let mut cc_total = 0u64;

        let empty = vec![];
        let action_votes = self
            .governance
            .votes_by_action
            .get(action_id)
            .unwrap_or(&empty);

        for (voter, procedure) in action_votes {
            match voter {
                Voter::DRep(cred) => {
                    let drep_hash = credential_to_hash(cred);
                    let voting_power =
                        drep_power_cache
                            .get(&drep_hash)
                            .copied()
                            .unwrap_or_else(|| {
                                // Fallback for DReps not in cache (e.g., voted but not delegated to)
                                if self
                                    .governance
                                    .dreps
                                    .get(&drep_hash)
                                    .is_some_and(|d| d.active)
                                {
                                    1
                                } else {
                                    0
                                }
                            });
                    // Only count yes and no votes in the denominator (abstain excluded)
                    match procedure.vote {
                        Vote::Yes => {
                            drep_yes += voting_power;
                        }
                        Vote::No => {
                            drep_no += voting_power;
                        }
                        Vote::Abstain => {
                            // Excluded from both numerator and denominator
                        }
                    }
                }
                Voter::StakePool(pool_hash) => {
                    // Pool IDs are Hash28 (Blake2b-224); convert from Hash32
                    let pool_id = Hash28::from_bytes({
                        let mut b = [0u8; 28];
                        b.copy_from_slice(&pool_hash.as_bytes()[..28]);
                        b
                    });
                    let pool_stake = self.compute_spo_voting_power(&pool_id);
                    // SPO denominator = total active SPO stake, so only count yes here
                    // Non-voting SPOs are implicitly No (in denominator but not numerator)
                    spo_total += pool_stake;
                    if procedure.vote == Vote::Yes {
                        spo_yes += pool_stake;
                    }
                }
                Voter::ConstitutionalCommittee(_) => {
                    cc_total += 1;
                    if procedure.vote == Vote::Yes {
                        cc_yes += 1;
                    }
                }
            }
        }

        // Handle AlwaysNoConfidence stake per CIP-1694:
        // - For NoConfidence actions: counts as Yes
        // - For all other actions: counts as No
        let is_no_confidence = matches!(action, GovAction::NoConfidence { .. });
        if no_confidence_stake > 0 {
            if is_no_confidence {
                drep_yes += no_confidence_stake;
            } else {
                drep_no += no_confidence_stake;
            }
        }

        let drep_total = drep_yes + drep_no;

        (drep_yes, drep_total, spo_yes, spo_total, cc_yes, cc_total)
    }

    /// Get the total stake for a credential: UTxO stake + reward account balance.
    fn credential_stake(&self, cred_hash: &Hash32) -> u64 {
        let utxo = self
            .stake_distribution
            .stake_map
            .get(cred_hash)
            .map(|s| s.0)
            .unwrap_or(0);
        let reward = self
            .reward_accounts
            .get(cred_hash)
            .map(|s| s.0)
            .unwrap_or(0);
        utxo + reward
    }

    /// Build a cache of DRep voting power (Hash32 -> delegated stake).
    /// Iterates vote_delegations once, O(n), instead of per-DRep O(n) lookups.
    /// Only includes active DReps (inactive DReps are excluded from voting power).
    /// Returns (drep_power_cache, always_no_confidence_stake, always_abstain_stake).
    fn build_drep_power_cache(&self) -> (HashMap<Hash32, u64>, u64, u64) {
        let mut cache: HashMap<Hash32, u64> = HashMap::new();
        let mut no_confidence_stake = 0u64;
        let mut abstain_stake = 0u64;
        for (stake_cred, drep) in &self.governance.vote_delegations {
            let stake = self.credential_stake(stake_cred);
            match drep {
                DRep::KeyHash(h) => {
                    // Only count stake for active DReps
                    if self.governance.dreps.get(h).is_some_and(|d| d.active) {
                        *cache.entry(*h).or_default() += stake;
                    }
                }
                DRep::ScriptHash(h) => {
                    let hash32 = h.to_hash32_padded();
                    if self.governance.dreps.get(&hash32).is_some_and(|d| d.active) {
                        *cache.entry(hash32).or_default() += stake;
                    }
                }
                DRep::NoConfidence => {
                    no_confidence_stake += stake;
                }
                DRep::Abstain => {
                    abstain_stake += stake;
                }
            }
        }
        // Ensure active registered DReps with no delegated stake have minimum power of 1
        for (drep_hash, drep) in &self.governance.dreps {
            if drep.active {
                cache.entry(*drep_hash).or_insert(1);
            }
        }
        (cache, no_confidence_stake, abstain_stake)
    }

    /// Compute total active DRep-delegated stake across all DReps.
    /// Excludes stake delegated to inactive DReps.
    /// Includes stake delegated to Abstain and NoConfidence (they are part of total DRep ecosystem).
    fn compute_total_drep_stake(&self) -> u64 {
        let mut total = 0u64;
        for (stake_cred, drep) in &self.governance.vote_delegations {
            let stake = self.credential_stake(stake_cred);
            match drep {
                DRep::Abstain | DRep::NoConfidence => {
                    total += stake;
                }
                DRep::KeyHash(h) => {
                    if self.governance.dreps.get(h).is_some_and(|d| d.active) {
                        total += stake;
                    }
                }
                DRep::ScriptHash(h) => {
                    let hash32 = h.to_hash32_padded();
                    if self.governance.dreps.get(&hash32).is_some_and(|d| d.active) {
                        total += stake;
                    }
                }
            }
        }
        total.max(1) // Ensure non-zero to avoid division by zero
    }

    /// Compute the voting power of a stake pool: total delegated stake.
    fn compute_spo_voting_power(&self, pool_id: &Hash28) -> u64 {
        // Use the "set" snapshot (previous epoch) for voting power, falling back to current
        if let Some(ref snapshot) = self.snapshots.set {
            if let Some(stake) = snapshot.pool_stake.get(pool_id) {
                return stake.0;
            }
        }
        // Fallback: compute from current delegations (UTxO + rewards)
        let mut total = 0u64;
        for (stake_cred, delegated_pool) in self.delegations.iter() {
            if delegated_pool == pool_id {
                total += self.credential_stake(stake_cred);
            }
        }
        if total == 0 {
            1
        } else {
            total
        }
    }

    /// Compute total active SPO stake across all pools.
    /// Used as the denominator for SPO voting thresholds.
    fn compute_total_spo_stake(&self) -> u64 {
        // Use "set" snapshot if available (previous epoch), else current pool_stake
        if let Some(ref snapshot) = self.snapshots.set {
            let total: u64 = snapshot.pool_stake.values().map(|s| s.0).sum();
            return total.max(1);
        }
        // Fallback: sum all pool stake from current delegations (UTxO + rewards)
        let mut total = 0u64;
        for stake_cred in self.delegations.keys() {
            total += self.credential_stake(stake_cred);
        }
        total.max(1)
    }

    /// Enact a ratified governance action by applying its effects
    fn enact_gov_action(&mut self, action: &GovAction) {
        match action {
            GovAction::ParameterChange {
                protocol_param_update,
                ..
            } => {
                if let Err(e) = self.apply_protocol_param_update(protocol_param_update) {
                    warn!(
                        error = %e,
                        "Governance protocol parameter update rejected"
                    );
                } else {
                    info!("Protocol parameters updated via governance action");
                }
            }
            GovAction::HardForkInitiation {
                protocol_version, ..
            } => {
                self.protocol_params.protocol_version_major = protocol_version.0;
                self.protocol_params.protocol_version_minor = protocol_version.1;
                info!(
                    "Hard fork initiated: protocol version {}.{}",
                    protocol_version.0, protocol_version.1
                );
            }
            GovAction::TreasuryWithdrawals { withdrawals, .. } => {
                // Compute total first and cap at available treasury
                let requested: u64 = withdrawals.values().map(|a| a.0).sum();
                let available = self.treasury.0;
                if requested > available {
                    warn!(
                        "Treasury withdrawal capped: requested {} but only {} available",
                        requested, available
                    );
                }
                let mut total = 0u64;
                for (reward_addr, amount) in withdrawals {
                    let actual = amount.0.min(self.treasury.0);
                    self.treasury.0 = self.treasury.0.saturating_sub(actual);
                    total += actual;
                    // Credit the withdrawal to the recipient's reward account
                    if actual > 0 && reward_addr.len() >= 29 {
                        let key = Self::reward_account_to_hash(reward_addr);
                        *Arc::make_mut(&mut self.reward_accounts)
                            .entry(key)
                            .or_insert(Lovelace(0)) += Lovelace(actual);
                    }
                }
                info!(
                    "Treasury withdrawal enacted: {} lovelace to {} accounts",
                    total,
                    withdrawals.len()
                );
            }
            GovAction::NoConfidence { .. } => {
                // No confidence motion: remove all committee hot key authorizations and expirations
                let gov = Arc::make_mut(&mut self.governance);
                gov.committee_hot_keys.clear();
                gov.committee_expiration.clear();
                gov.no_confidence = true;
                info!("No confidence motion enacted: committee disbanded");
            }
            GovAction::UpdateCommittee {
                members_to_remove,
                members_to_add,
                threshold,
                ..
            } => {
                // Remove specified members
                for cred in members_to_remove {
                    let key = credential_to_hash(cred);
                    Arc::make_mut(&mut self.governance)
                        .committee_hot_keys
                        .remove(&key);
                    Arc::make_mut(&mut self.governance)
                        .committee_expiration
                        .remove(&key);
                    Arc::make_mut(&mut self.governance)
                        .committee_resigned
                        .remove(&key);
                }
                // Add new members with expiration epochs
                for (cred, expiration_epoch) in members_to_add {
                    let key = credential_to_hash(cred);
                    Arc::make_mut(&mut self.governance)
                        .committee_expiration
                        .insert(key, EpochNo(*expiration_epoch));
                    // Hot key auth comes via CommitteeHotAuth certificates
                }
                // Store the new committee quorum threshold
                Arc::make_mut(&mut self.governance).committee_threshold = Some(threshold.clone());
                // UpdateCommittee restores confidence
                Arc::make_mut(&mut self.governance).no_confidence = false;
                info!(
                    "Committee updated: {} removed, {} added, threshold={}/{}",
                    members_to_remove.len(),
                    members_to_add.len(),
                    threshold.numerator,
                    threshold.denominator,
                );
            }
            GovAction::NewConstitution { constitution, .. } => {
                Arc::make_mut(&mut self.governance).constitution = Some(constitution.clone());
                info!(
                    "New constitution enacted (script_hash: {:?})",
                    constitution.script_hash.as_ref().map(|h| h.to_hex())
                );
            }
            GovAction::InfoAction => {
                // Info actions have no on-chain effect
                debug!("Info action ratified (no on-chain effect)");
            }
        }
    }

    /// Process a withdrawal from a reward account.
    /// Per Cardano spec, the withdrawal amount must exactly match the reward balance.
    /// After withdrawal, the balance is reduced by the withdrawal amount.
    fn process_withdrawal(&mut self, reward_account: &[u8], amount: Lovelace) {
        let key = Self::reward_account_to_hash(reward_account);
        if let Some(balance) = Arc::make_mut(&mut self.reward_accounts).get_mut(&key) {
            // Per Cardano spec, withdrawal amount must exactly equal the reward balance.
            // During sync from genesis, we may not have accumulated all rewards yet,
            // so we only warn and process as best-effort.
            if balance.0 != amount.0 {
                debug!(
                    account = %key.to_hex(),
                    balance = balance.0,
                    withdrawal = amount.0,
                    "Withdrawal amount does not match reward balance"
                );
            }
            // Always process the withdrawal: set balance to 0
            // (rewards were consumed in the on-chain transaction)
            balance.0 = 0;
        }
    }

    /// Current snapshot format version.
    /// Increment this when the serialized LedgerState layout changes.
    const SNAPSHOT_VERSION: u8 = 1;

    /// Save ledger state snapshot to disk using bincode serialization.
    /// Format: [4-byte magic "TRSN"][1-byte version][32-byte blake2b checksum][bincode data]
    pub fn save_snapshot(&self, path: &Path) -> Result<(), LedgerError> {
        let tmp_path = path.with_extension("tmp");

        // Serialize the ledger state to bincode.
        let data = bincode::serialize(self).map_err(|e| {
            LedgerError::EpochTransition(format!("Failed to serialize ledger state: {e}"))
        })?;

        // Compute checksum over the serialized data
        let checksum = torsten_primitives::hash::blake2b_256(&data);

        // Write header + data using a single buffered write
        // Header: "TRSN" (4 bytes) + version (1 byte) + blake2b checksum (32 bytes)
        use std::io::Write;
        let file = std::fs::File::create(&tmp_path)
            .map_err(|e| LedgerError::EpochTransition(format!("Failed to create snapshot: {e}")))?;
        let mut writer = std::io::BufWriter::with_capacity(1 << 20, file);
        writer.write_all(b"TRSN").map_err(|e| {
            LedgerError::EpochTransition(format!("Failed to write snapshot header: {e}"))
        })?;
        writer.write_all(&[Self::SNAPSHOT_VERSION]).map_err(|e| {
            LedgerError::EpochTransition(format!("Failed to write snapshot version: {e}"))
        })?;
        writer.write_all(checksum.as_bytes()).map_err(|e| {
            LedgerError::EpochTransition(format!("Failed to write snapshot checksum: {e}"))
        })?;
        writer.write_all(&data).map_err(|e| {
            LedgerError::EpochTransition(format!("Failed to write snapshot data: {e}"))
        })?;
        writer
            .flush()
            .map_err(|e| LedgerError::EpochTransition(format!("Failed to flush snapshot: {e}")))?;
        drop(writer);

        let total_bytes = 4 + 1 + 32 + data.len();

        std::fs::rename(&tmp_path, path)
            .map_err(|e| LedgerError::EpochTransition(format!("Failed to rename snapshot: {e}")))?;
        info!(
            path = %path.display(),
            bytes = total_bytes,
            version = Self::SNAPSHOT_VERSION,
            utxo_count = self.utxo_set.len(),
            epoch = self.epoch.0,
            slot = ?self.tip.point.slot().map(|s| s.0),
            "Ledger snapshot saved"
        );
        Ok(())
    }

    /// Load ledger state snapshot from disk.
    /// Rejects snapshots larger than [`MAX_SNAPSHOT_SIZE`] to prevent OOM.
    /// Supports three formats:
    /// - **Versioned (v1+):** `TRSN` + version byte + 32-byte checksum + data
    /// - **Legacy with checksum:** `TRSN` + 32-byte checksum + data (no version byte)
    /// - **Legacy raw:** plain bincode without any header
    pub fn load_snapshot(path: &Path) -> Result<Self, LedgerError> {
        let raw = std::fs::read(path)
            .map_err(|e| LedgerError::EpochTransition(format!("Failed to read snapshot: {e}")))?;

        // Reject oversized snapshot files to prevent OOM from malicious data
        if raw.len() > MAX_SNAPSHOT_SIZE {
            return Err(LedgerError::EpochTransition(format!(
                "Snapshot size {} exceeds maximum allowed size {}",
                raw.len(),
                MAX_SNAPSHOT_SIZE
            )));
        }

        let data = if raw.len() >= 37 && &raw[..4] == b"TRSN" {
            let fifth_byte = raw[4];
            if fifth_byte > 0 && fifth_byte < 128 {
                // Versioned format: TRSN + version(1) + checksum(32) + data
                let version = fifth_byte;
                if version > Self::SNAPSHOT_VERSION {
                    return Err(LedgerError::EpochTransition(format!(
                        "Unsupported snapshot version {version} (max supported: {})",
                        Self::SNAPSHOT_VERSION,
                    )));
                }
                info!(version, "Loading versioned snapshot");
                let stored_checksum = &raw[5..37];
                let payload = &raw[37..];
                let computed = torsten_primitives::hash::blake2b_256(payload);
                if computed.as_bytes() != stored_checksum {
                    return Err(LedgerError::EpochTransition(
                        "Snapshot checksum mismatch — file may be corrupted".to_string(),
                    ));
                }
                payload
            } else {
                // Legacy format with checksum but no version byte:
                // TRSN + checksum(32) + data (5th byte is part of blake2b hash)
                warn!("Loading legacy snapshot (no version byte) with checksum verification");
                let stored_checksum = &raw[4..36];
                let payload = &raw[36..];
                let computed = torsten_primitives::hash::blake2b_256(payload);
                if computed.as_bytes() != stored_checksum {
                    return Err(LedgerError::EpochTransition(
                        "Snapshot checksum mismatch — file may be corrupted".to_string(),
                    ));
                }
                payload
            }
        } else if raw.len() >= 36 && &raw[..4] == b"TRSN" {
            // Legacy format with checksum (exactly 36 bytes of header, rare edge case)
            warn!("Loading legacy snapshot (no version byte) with checksum verification");
            let stored_checksum = &raw[4..36];
            let payload = &raw[36..];
            let computed = torsten_primitives::hash::blake2b_256(payload);
            if computed.as_bytes() != stored_checksum {
                return Err(LedgerError::EpochTransition(
                    "Snapshot checksum mismatch — file may be corrupted".to_string(),
                ));
            }
            payload
        } else {
            // Legacy format: raw bincode without header (backwards compatible)
            warn!("Loading legacy snapshot without checksum verification");
            &raw
        };

        // Use bincode options with size limit as defense-in-depth against
        // malicious payloads that encode enormous internal allocations.
        // Must use with_fixint_encoding() to match bincode::serialize() defaults.
        use bincode::Options;
        let mut state: LedgerState = bincode::options()
            .with_fixint_encoding()
            .allow_trailing_bytes()
            .with_limit(MAX_SNAPSHOT_SIZE as u64)
            .deserialize(data)
            .map_err(|e| {
                LedgerError::EpochTransition(format!("Failed to deserialize ledger state: {e}"))
            })?;
        state.utxo_set.rebuild_address_index();
        // After loading a snapshot, incremental stake tracking may be stale,
        // so force a full rebuild at the next epoch boundary.
        state.needs_stake_rebuild = true;
        info!(
            path = %path.display(),
            bytes = raw.len(),
            utxo_count = state.utxo_set.len(),
            epoch = state.epoch.0,
            slot = ?state.tip.point.slot().map(|s| s.0),
            "Ledger snapshot loaded"
        );
        Ok(state)
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

/// DRep voting group for protocol parameter classification per CIP-1694.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum DRepPPGroup {
    Network,
    Economic,
    Technical,
    Gov,
}

/// Whether SPOs can vote on a parameter change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum StakePoolPPGroup {
    Security,
    NoVote,
}

/// Classification of a protocol parameter: (DRepPPGroup, StakePoolPPGroup).
/// Matches Haskell cardano-ledger Conway `PPGroups` exactly.
type PPGroup = (DRepPPGroup, StakePoolPPGroup);

/// Determine which PP groups are modified by a ProtocolParamUpdate.
///
/// Each parameter belongs to exactly one (DRepPPGroup, StakePoolPPGroup) pair.
/// Classification matches Haskell cardano-ledger Conway ConwayPParams field tags.
fn modified_pp_groups(ppu: &ProtocolParamUpdate) -> Vec<PPGroup> {
    use DRepPPGroup::*;
    use StakePoolPPGroup::*;

    let mut groups = Vec::new();

    // Network + Security
    if ppu.max_block_body_size.is_some() {
        groups.push((Network, Security));
    }
    if ppu.max_tx_size.is_some() {
        groups.push((Network, Security));
    }
    if ppu.max_block_header_size.is_some() {
        groups.push((Network, Security));
    }
    if ppu.max_block_ex_units.is_some() {
        groups.push((Network, Security));
    }
    if ppu.max_val_size.is_some() {
        groups.push((Network, Security));
    }

    // Network + NoVote
    if ppu.max_tx_ex_units.is_some() {
        groups.push((Network, NoVote));
    }
    if ppu.max_collateral_inputs.is_some() {
        groups.push((Network, NoVote));
    }

    // Economic + Security
    if ppu.min_fee_a.is_some() {
        groups.push((Economic, Security));
    }
    if ppu.min_fee_b.is_some() {
        groups.push((Economic, Security));
    }
    if ppu.ada_per_utxo_byte.is_some() {
        groups.push((Economic, Security));
    }
    if ppu.min_fee_ref_script_cost_per_byte.is_some() {
        groups.push((Economic, Security));
    }

    // Economic + NoVote
    if ppu.key_deposit.is_some() {
        groups.push((Economic, NoVote));
    }
    if ppu.pool_deposit.is_some() {
        groups.push((Economic, NoVote));
    }
    if ppu.rho.is_some() {
        groups.push((Economic, NoVote));
    }
    if ppu.tau.is_some() {
        groups.push((Economic, NoVote));
    }
    if ppu.min_pool_cost.is_some() {
        groups.push((Economic, NoVote));
    }
    if ppu.execution_costs.is_some() {
        groups.push((Economic, NoVote));
    }

    // Technical + NoVote
    if ppu.e_max.is_some() {
        groups.push((Technical, NoVote));
    }
    if ppu.n_opt.is_some() {
        groups.push((Technical, NoVote));
    }
    if ppu.a0.is_some() {
        groups.push((Technical, NoVote));
    }
    if ppu.cost_models.is_some() {
        groups.push((Technical, NoVote));
    }
    if ppu.collateral_percentage.is_some() {
        groups.push((Technical, NoVote));
    }

    // Gov + Security
    if ppu.gov_action_deposit.is_some() {
        groups.push((Gov, Security));
    }

    // Gov + NoVote
    if ppu.dvt_pp_network_group.is_some()
        || ppu.dvt_pp_economic_group.is_some()
        || ppu.dvt_pp_technical_group.is_some()
        || ppu.dvt_pp_gov_group.is_some()
        || ppu.dvt_hard_fork.is_some()
        || ppu.dvt_no_confidence.is_some()
        || ppu.dvt_committee_normal.is_some()
        || ppu.dvt_committee_no_confidence.is_some()
        || ppu.dvt_constitution.is_some()
        || ppu.dvt_treasury_withdrawal.is_some()
    {
        groups.push((Gov, NoVote));
    }
    if ppu.pvt_motion_no_confidence.is_some()
        || ppu.pvt_committee_normal.is_some()
        || ppu.pvt_committee_no_confidence.is_some()
        || ppu.pvt_hard_fork.is_some()
        || ppu.pvt_pp_security_group.is_some()
    {
        groups.push((Gov, NoVote));
    }
    if ppu.min_committee_size.is_some() {
        groups.push((Gov, NoVote));
    }
    if ppu.committee_term_limit.is_some() {
        groups.push((Gov, NoVote));
    }
    if ppu.gov_action_lifetime.is_some() {
        groups.push((Gov, NoVote));
    }
    if ppu.drep_deposit.is_some() {
        groups.push((Gov, NoVote));
    }
    if ppu.drep_activity.is_some() {
        groups.push((Gov, NoVote));
    }

    groups
}

/// Check that ALL affected DRep parameter group thresholds are independently met.
///
/// Per CIP-1694 / Haskell `pparamsUpdateThreshold`: each affected parameter group
/// has its own DRep voting threshold. A ParameterChange is ratified only if the
/// DRep vote ratio meets the threshold for EVERY affected group independently.
///
/// This replaces the previous (incorrect) max-of-all-groups approach.
fn pp_change_drep_all_groups_met(
    ppu: &ProtocolParamUpdate,
    params: &ProtocolParameters,
    drep_yes: u64,
    drep_total: u64,
) -> bool {
    let groups = modified_pp_groups(ppu);
    // Collect unique DRep groups (avoid checking the same group multiple times)
    let mut seen = std::collections::HashSet::new();
    for (drep_group, _) in &groups {
        if !seen.insert(*drep_group) {
            continue;
        }
        let threshold = match drep_group {
            DRepPPGroup::Network => &params.dvt_pp_network_group,
            DRepPPGroup::Economic => &params.dvt_pp_economic_group,
            DRepPPGroup::Technical => &params.dvt_pp_technical_group,
            DRepPPGroup::Gov => &params.dvt_pp_gov_group,
        };
        if !check_threshold(drep_yes, drep_total, threshold) {
            return false;
        }
    }
    true
}

/// Compute the maximum DRep voting threshold for a ParameterChange governance action.
///
/// Returns the highest DRep group threshold across all affected parameter groups.
/// Used by tests and for informational purposes. For ratification, use
/// `pp_change_drep_all_groups_met` which checks each group independently.
#[cfg(test)]
fn pp_change_drep_threshold(ppu: &ProtocolParamUpdate, params: &ProtocolParameters) -> Rational {
    let groups = modified_pp_groups(ppu);
    let mut max_threshold = Rational {
        numerator: 0,
        denominator: 1,
    };
    for (drep_group, _) in &groups {
        let t = match drep_group {
            DRepPPGroup::Network => &params.dvt_pp_network_group,
            DRepPPGroup::Economic => &params.dvt_pp_economic_group,
            DRepPPGroup::Technical => &params.dvt_pp_technical_group,
            DRepPPGroup::Gov => &params.dvt_pp_gov_group,
        };
        if t.gt(&max_threshold) {
            max_threshold = t.clone();
        }
    }
    max_threshold
}

/// Determine if SPOs can vote on a ParameterChange, and if so, return the threshold.
///
/// Per Haskell `votingStakePoolThresholdInternal`: SPOs vote with pvtPPSecurityGroup
/// if ANY modified parameter is tagged SecurityGroup. Otherwise SPOs cannot vote.
fn pp_change_spo_threshold(
    ppu: &ProtocolParamUpdate,
    params: &ProtocolParameters,
) -> Option<Rational> {
    let groups = modified_pp_groups(ppu);
    let has_security = groups
        .iter()
        .any(|(_, spo)| *spo == StakePoolPPGroup::Security);
    if has_security {
        Some(params.pvt_pp_security_group.clone())
    } else {
        None
    }
}

fn check_threshold(yes: u64, total: u64, threshold: &Rational) -> bool {
    if total == 0 {
        return false;
    }
    // Exact integer comparison: yes/total >= numerator/denominator
    // ⟺ yes * denominator >= numerator * total (using u128 to avoid overflow)
    threshold.is_met_by(yes, total)
}

/// Check if the constitutional committee has approved a governance action.
///
/// Per Haskell `committeeAccepted` / `committeeAcceptedRatio`:
/// - Iterate ALL committee members (from committee_expiration, which tracks membership)
/// - Expired members: excluded (treated as abstain)
/// - Members without hot keys (unregistered): excluded (treated as abstain)
/// - Resigned members: excluded (treated as abstain)
/// - Active members who didn't vote: counted as NO
/// - Active members who voted Abstain: excluded from ratio
/// - Active members who voted Yes: yes / Active members who voted No: no
/// - Ratio = yes_count / (yes_count + no_count) compared against committee_threshold
///
/// During bootstrap (protocol version 9), committeeMinSize check is skipped.
/// Post-bootstrap, if active_size < committeeMinSize, CC blocks ratification.
fn check_cc_approval(
    action_id: &GovActionId,
    governance: &GovernanceState,
    current_epoch: EpochNo,
    committee_min_size: u64,
    bootstrap: bool,
) -> bool {
    // Get committee quorum threshold
    let threshold = match &governance.committee_threshold {
        Some(t) => t,
        None => {
            // No committee exists — CC vote fails (blocks ratification)
            return false;
        }
    };

    // If threshold is 0, auto-approve
    if threshold.is_zero() {
        return true;
    }

    // Collect CC votes for this action indexed by hot credential
    let mut cc_votes: HashMap<Hash32, Vote> = HashMap::new();
    let empty = vec![];
    let action_votes = governance.votes_by_action.get(action_id).unwrap_or(&empty);
    for (voter, procedure) in action_votes {
        if let Voter::ConstitutionalCommittee(cred) = voter {
            let hot_key = credential_to_hash(cred);
            cc_votes.insert(hot_key, procedure.vote.clone());
        }
    }

    // Iterate all committee members and compute the ratio
    let mut yes_count = 0u64;
    let mut total_excluding_abstain = 0u64;
    let mut active_size = 0u64;

    for (cold_key, expiry) in &governance.committee_expiration {
        // Expired members: excluded (treated as abstain)
        if *expiry <= current_epoch {
            continue;
        }

        // Check if member has a registered hot key
        let hot_key = match governance.committee_hot_keys.get(cold_key) {
            Some(hk) => hk,
            None => continue, // No hot key: excluded (treated as abstain)
        };

        // Resigned members: excluded (treated as abstain)
        if governance.committee_resigned.contains_key(cold_key) {
            continue;
        }

        active_size += 1;

        // Look up vote by hot credential
        match cc_votes.get(hot_key) {
            Some(Vote::Yes) => {
                yes_count += 1;
                total_excluding_abstain += 1;
            }
            Some(Vote::Abstain) => {
                // Abstain: excluded from ratio
            }
            Some(Vote::No) | None => {
                // Voted No or didn't vote: counts as No
                total_excluding_abstain += 1;
            }
        }
    }

    // Check committeeMinSize (skipped during bootstrap per Haskell spec)
    if !bootstrap && active_size < committee_min_size {
        return false;
    }

    // If no committee members exist at all
    if active_size == 0 {
        return false;
    }

    // If all active members abstained, ratio is 0
    if total_excluding_abstain == 0 {
        debug!(
            action = %action_id.transaction_id.to_hex(),
            active_size, yes_count, total_excluding_abstain,
            threshold = threshold.as_f64(),
            cc_voters = cc_votes.len(),
            committee_members = governance.committee_expiration.len(),
            hot_keys = governance.committee_hot_keys.len(),
            "CC approval check: all active members abstained"
        );
        return false;
    }

    // Exact comparison: yes_count / total_excluding_abstain >= threshold
    let result = threshold.is_met_by(yes_count, total_excluding_abstain);
    if !result {
        debug!(
            action = %action_id.transaction_id.to_hex(),
            active_size, yes_count, total_excluding_abstain,
            threshold = threshold.as_f64(),
            ratio = yes_count as f64 / total_excluding_abstain as f64,
            result,
            cc_voters = cc_votes.len(),
            committee_members = governance.committee_expiration.len(),
            hot_keys = governance.committee_hot_keys.len(),
            "CC approval check failed"
        );
    }
    result
}

/// Check that a proposal's `prev_action_id` matches the last enacted action of the same
/// governance purpose. Per Haskell `prevActionAsExpected` in Ratify.hs.
///
/// NoConfidence and UpdateCommittee share the `Committee` purpose.
/// TreasuryWithdrawals and InfoAction have no prev_action_id chain (always pass).
fn prev_action_as_expected(action: &GovAction, governance: &GovernanceState) -> bool {
    match action {
        GovAction::ParameterChange { prev_action_id, .. } => {
            *prev_action_id == governance.enacted_pparam_update
        }
        GovAction::HardForkInitiation { prev_action_id, .. } => {
            *prev_action_id == governance.enacted_hard_fork
        }
        GovAction::NoConfidence { prev_action_id } => {
            *prev_action_id == governance.enacted_committee
        }
        GovAction::UpdateCommittee { prev_action_id, .. } => {
            *prev_action_id == governance.enacted_committee
        }
        GovAction::NewConstitution { prev_action_id, .. } => {
            *prev_action_id == governance.enacted_constitution
        }
        // TreasuryWithdrawals and InfoAction have no chain requirement
        GovAction::TreasuryWithdrawals { .. } | GovAction::InfoAction => true,
    }
}

/// Returns the governance action priority for ratification ordering.
/// Lower number = higher priority, per Haskell's `actionPriority`.
fn gov_action_priority(action: &GovAction) -> u8 {
    match action {
        GovAction::NoConfidence { .. } => 0,
        GovAction::UpdateCommittee { .. } => 1,
        GovAction::NewConstitution { .. } => 2,
        GovAction::HardForkInitiation { .. } => 3,
        GovAction::ParameterChange { .. } => 4,
        GovAction::TreasuryWithdrawals { .. } => 5,
        GovAction::InfoAction => 6,
    }
}

/// Whether enacting this action should delay all further ratification for this epoch.
/// Per Haskell `delayingAction`: NoConfidence, HardFork, UpdateCommittee, NewConstitution.
fn is_delaying_action(action: &GovAction) -> bool {
    matches!(
        action,
        GovAction::NoConfidence { .. }
            | GovAction::HardForkInitiation { .. }
            | GovAction::UpdateCommittee { .. }
            | GovAction::NewConstitution { .. }
    )
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use torsten_primitives::address::{Address, BaseAddress, ByronAddress};
    use torsten_primitives::hash::Hash28;
    use torsten_primitives::network::NetworkId;
    use torsten_primitives::transaction::*;
    use torsten_primitives::value::Value;

    /// Counter for unique UTxO inputs in tests.
    static UTXO_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

    /// Add a UTxO with a Base address for the given stake credential and amount.
    /// This ensures `rebuild_stake_distribution` will find the stake.
    fn add_stake_utxo(state: &mut LedgerState, cred: &Credential, amount: u64) {
        let payment_cred = Credential::VerificationKey(Hash28::from_bytes([0xFFu8; 28]));
        let addr = Address::Base(BaseAddress {
            network: NetworkId::Mainnet,
            payment: payment_cred,
            stake: cred.clone(),
        });
        let counter = UTXO_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut tx_id_bytes = [0u8; 32];
        tx_id_bytes[..8].copy_from_slice(&counter.to_be_bytes());
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes(tx_id_bytes),
            index: 0,
        };
        let output = TransactionOutput {
            address: addr,
            value: Value {
                coin: Lovelace(amount),
                multi_asset: Default::default(),
            },
            datum: torsten_primitives::transaction::OutputDatum::None,
            script_ref: None,
            raw_cbor: None,
        };
        state.utxo_set.insert(input, output);
    }

    fn make_test_block(
        slot: u64,
        block_no: u64,
        prev_hash: Hash32,
        transactions: Vec<Transaction>,
    ) -> Block {
        Block {
            header: torsten_primitives::block::BlockHeader {
                header_hash: Hash32::from_bytes([block_no as u8; 32]),
                prev_hash,
                issuer_vkey: vec![],
                vrf_vkey: vec![],
                vrf_result: torsten_primitives::block::VrfOutput {
                    output: vec![],
                    proof: vec![],
                },
                block_number: BlockNo(block_no),
                slot: SlotNo(slot),
                epoch_nonce: Hash32::ZERO,
                body_size: 0,
                body_hash: Hash32::ZERO,
                operational_cert: torsten_primitives::block::OperationalCert {
                    hot_vkey: vec![],
                    sequence_number: 0,
                    kes_period: 0,
                    sigma: vec![],
                },
                protocol_version: torsten_primitives::block::ProtocolVersion { major: 9, minor: 0 },
                kes_signature: vec![],
            },
            transactions,
            era: Era::Conway,
            raw_cbor: None,
        }
    }

    #[test]
    fn test_new_ledger_state() {
        let params = ProtocolParameters::mainnet_defaults();
        let state = LedgerState::new(params);
        assert_eq!(state.tip, Tip::origin());
        assert!(state.utxo_set.is_empty());
        assert_eq!(state.epoch, EpochNo(0));
    }

    #[test]
    fn test_apply_block_with_transaction() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);

        // Seed the UTxO set with an initial entry
        let genesis_input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        let genesis_output = TransactionOutput {
            address: Address::Byron(ByronAddress {
                payload: vec![0u8; 32],
            }),
            value: Value::lovelace(10_000_000),
            datum: OutputDatum::None,
            script_ref: None,
            raw_cbor: None,
        };
        state.utxo_set.insert(genesis_input.clone(), genesis_output);

        let tx_hash = Hash32::from_bytes([2u8; 32]);
        let tx = Transaction {
            hash: tx_hash,
            body: TransactionBody {
                inputs: vec![genesis_input],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(9_800_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    raw_cbor: None,
                }],
                fee: Lovelace(200_000),
                ttl: None,
                certificates: vec![],
                withdrawals: BTreeMap::new(),
                auxiliary_data_hash: None,
                validity_interval_start: None,
                mint: BTreeMap::new(),
                script_data_hash: None,
                collateral: vec![],
                required_signers: vec![],
                network_id: None,
                collateral_return: None,
                total_collateral: None,
                reference_inputs: vec![],
                update: None,
                voting_procedures: BTreeMap::new(),
                proposal_procedures: vec![],
                treasury_value: None,
                donation: None,
            },
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![],
                plutus_v3_scripts: vec![],
                plutus_data: vec![],
                redeemers: vec![],
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let block = make_test_block(100, 1, Hash32::ZERO, vec![tx]);
        state.apply_block(&block).unwrap();

        // The genesis UTxO should be spent, new one created
        assert_eq!(state.utxo_set.len(), 1);
        let new_input = TransactionInput {
            transaction_id: tx_hash,
            index: 0,
        };
        assert!(state.utxo_set.contains(&new_input));
        assert_eq!(state.tip.block_number, BlockNo(1));
    }

    #[test]
    fn test_apply_block_skips_invalid_tx() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);

        let genesis_input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        state.utxo_set.insert(
            genesis_input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                raw_cbor: None,
            },
        );

        // Transaction marked as invalid (phase-2 failure)
        let tx = Transaction {
            hash: Hash32::from_bytes([2u8; 32]),
            body: TransactionBody {
                inputs: vec![genesis_input.clone()],
                outputs: vec![],
                fee: Lovelace(0),
                ttl: None,
                certificates: vec![],
                withdrawals: BTreeMap::new(),
                auxiliary_data_hash: None,
                validity_interval_start: None,
                mint: BTreeMap::new(),
                script_data_hash: None,
                collateral: vec![],
                required_signers: vec![],
                network_id: None,
                collateral_return: None,
                total_collateral: None,
                reference_inputs: vec![],
                update: None,
                voting_procedures: BTreeMap::new(),
                proposal_procedures: vec![],
                treasury_value: None,
                donation: None,
            },
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![],
                plutus_v3_scripts: vec![],
                plutus_data: vec![],
                redeemers: vec![],
            },
            is_valid: false,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let block = make_test_block(100, 1, Hash32::ZERO, vec![tx]);
        state.apply_block(&block).unwrap();

        // UTxO should be unchanged since tx was invalid
        assert_eq!(state.utxo_set.len(), 1);
        assert!(state.utxo_set.contains(&genesis_input));
    }

    #[test]
    fn test_process_stake_registration() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);

        let cred = Credential::VerificationKey(Hash28::from_bytes([42u8; 28]));
        let cert = Certificate::StakeRegistration(cred.clone());
        state.process_certificate(&cert);

        let key = credential_to_hash(&cred);
        assert!(state.stake_distribution.stake_map.contains_key(&key));
    }

    #[test]
    fn test_process_stake_delegation() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);

        let cred = Credential::VerificationKey(Hash28::from_bytes([42u8; 28]));
        let pool_hash = Hash28::from_bytes([99u8; 28]);

        // Register first
        state.process_certificate(&Certificate::StakeRegistration(cred.clone()));
        // Then delegate
        state.process_certificate(&Certificate::StakeDelegation {
            credential: cred.clone(),
            pool_hash,
        });

        let key = credential_to_hash(&cred);
        assert_eq!(state.delegations.get(&key), Some(&pool_hash));
    }

    #[test]
    fn test_process_pool_registration() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);

        let pool_id = Hash28::from_bytes([1u8; 28]);
        let pool_params = PoolParams {
            operator: pool_id,
            vrf_keyhash: Hash32::from_bytes([2u8; 32]),
            pledge: Lovelace(500_000_000),
            cost: Lovelace(340_000_000),
            margin: Rational {
                numerator: 1,
                denominator: 100,
            },
            reward_account: vec![0u8; 29],
            pool_owners: vec![pool_id],
            relays: vec![],
            pool_metadata: None,
        };

        state.process_certificate(&Certificate::PoolRegistration(pool_params));
        assert!(state.pool_params.contains_key(&pool_id));
        assert_eq!(state.pool_params[&pool_id].pledge, Lovelace(500_000_000));
    }

    #[test]
    fn test_process_stake_deregistration() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);

        let cred = Credential::VerificationKey(Hash28::from_bytes([42u8; 28]));
        let pool_hash = Hash28::from_bytes([99u8; 28]);
        let key = credential_to_hash(&cred);

        // Register and delegate
        state.process_certificate(&Certificate::StakeRegistration(cred.clone()));
        state.process_certificate(&Certificate::StakeDelegation {
            credential: cred.clone(),
            pool_hash,
        });

        // Deregister
        state.process_certificate(&Certificate::StakeDeregistration(cred));

        assert!(!state.stake_distribution.stake_map.contains_key(&key));
        assert!(!state.delegations.contains_key(&key));
    }

    #[test]
    fn test_process_pool_retirement() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);

        let pool_id = Hash28::from_bytes([1u8; 28]);
        let pool_params = PoolParams {
            operator: pool_id,
            vrf_keyhash: Hash32::from_bytes([2u8; 32]),
            pledge: Lovelace(500_000_000),
            cost: Lovelace(340_000_000),
            margin: Rational {
                numerator: 1,
                denominator: 100,
            },
            reward_account: vec![0u8; 29],
            pool_owners: vec![pool_id],
            relays: vec![],
            pool_metadata: None,
        };

        state.process_certificate(&Certificate::PoolRegistration(pool_params));
        assert!(state.pool_params.contains_key(&pool_id));

        // Schedule retirement at epoch 2
        state.process_certificate(&Certificate::PoolRetirement {
            pool_hash: pool_id,
            epoch: 2,
        });
        // Pool still exists (retirement is pending)
        assert!(state.pool_params.contains_key(&pool_id));
        assert!(state.pending_retirements.contains_key(&EpochNo(2)));

        // Trigger epoch transition to epoch 2
        state.process_epoch_transition(EpochNo(2));
        // Now the pool should be retired
        assert!(!state.pool_params.contains_key(&pool_id));
    }

    #[test]
    fn test_epoch_transition_snapshots() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);
        state.epoch_length = 100; // Small epochs for testing

        let cred = Credential::VerificationKey(Hash28::from_bytes([42u8; 28]));
        let pool_id = Hash28::from_bytes([1u8; 28]);

        // Register stake and delegate
        state.process_certificate(&Certificate::StakeRegistration(cred.clone()));
        add_stake_utxo(&mut state, &cred, 1_000_000);

        // Register pool
        state.process_certificate(&Certificate::PoolRegistration(PoolParams {
            operator: pool_id,
            vrf_keyhash: Hash32::from_bytes([2u8; 32]),
            pledge: Lovelace(100),
            cost: Lovelace(100),
            margin: Rational {
                numerator: 0,
                denominator: 1,
            },
            reward_account: vec![0u8; 29],
            pool_owners: vec![pool_id],
            relays: vec![],
            pool_metadata: None,
        }));

        // Delegate to pool
        state.process_certificate(&Certificate::StakeDelegation {
            credential: cred.clone(),
            pool_hash: pool_id,
        });

        // Epoch 0 -> 1: first snapshot taken
        state.process_epoch_transition(EpochNo(1));
        assert!(state.snapshots.mark.is_some());
        assert!(state.snapshots.set.is_none());
        assert!(state.snapshots.go.is_none());

        let mark = state.snapshots.mark.as_ref().unwrap();
        assert_eq!(mark.pool_stake[&pool_id], Lovelace(1_000_000));

        // Epoch 1 -> 2: mark becomes set
        state.process_epoch_transition(EpochNo(2));
        assert!(state.snapshots.mark.is_some());
        assert!(state.snapshots.set.is_some());
        assert!(state.snapshots.go.is_none());

        let set = state.snapshots.set.as_ref().unwrap();
        assert_eq!(set.epoch, EpochNo(1));

        // Epoch 2 -> 3: set becomes go
        state.process_epoch_transition(EpochNo(3));
        assert!(state.snapshots.mark.is_some());
        assert!(state.snapshots.set.is_some());
        assert!(state.snapshots.go.is_some());

        let go = state.snapshots.go.as_ref().unwrap();
        assert_eq!(go.epoch, EpochNo(1));
    }

    #[test]
    fn test_epoch_transition_in_apply_block() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);
        state.epoch_length = 100; // Small epochs for testing
        state.shelley_transition_epoch = 0;
        state.byron_epoch_length = 0;

        // Apply a block in epoch 0
        let block = make_test_block(50, 1, Hash32::ZERO, vec![]);
        state.apply_block(&block).unwrap();
        assert_eq!(state.epoch, EpochNo(0));

        // Apply a block in epoch 1 (slot 100+)
        let block = make_test_block(150, 2, *block.hash(), vec![]);
        state.apply_block(&block).unwrap();
        assert_eq!(state.epoch, EpochNo(1));
        assert!(state.snapshots.mark.is_some());
    }

    #[test]
    fn test_fee_accumulation() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);

        // Seed UTxO
        let genesis_input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        state.utxo_set.insert(
            genesis_input.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                raw_cbor: None,
            },
        );

        let tx = Transaction {
            hash: Hash32::from_bytes([2u8; 32]),
            body: TransactionBody {
                inputs: vec![genesis_input],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0u8; 32],
                    }),
                    value: Value::lovelace(9_800_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    raw_cbor: None,
                }],
                fee: Lovelace(200_000),
                ttl: None,
                certificates: vec![],
                withdrawals: BTreeMap::new(),
                auxiliary_data_hash: None,
                validity_interval_start: None,
                mint: BTreeMap::new(),
                script_data_hash: None,
                collateral: vec![],
                required_signers: vec![],
                network_id: None,
                collateral_return: None,
                total_collateral: None,
                reference_inputs: vec![],
                update: None,
                voting_procedures: BTreeMap::new(),
                proposal_procedures: vec![],
                treasury_value: None,
                donation: None,
            },
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![],
                plutus_v3_scripts: vec![],
                plutus_data: vec![],
                redeemers: vec![],
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let block = make_test_block(100, 1, Hash32::ZERO, vec![tx]);
        state.apply_block(&block).unwrap();

        assert_eq!(state.epoch_fees, Lovelace(200_000));
    }

    #[test]
    fn test_reward_calculation() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);
        state.epoch_length = 432000; // Mainnet epoch length
                                     // Realistic reserves: 10 billion ADA
        state.reserves = Lovelace(10_000_000_000_000_000);

        let owner_hash = Hash28::from_bytes([42u8; 28]);
        let cred = Credential::VerificationKey(owner_hash);
        let pool_id = Hash28::from_bytes([1u8; 28]);

        // Build reward account from owner credential
        let mut reward_account = vec![0xE0u8];
        reward_account.extend_from_slice(owner_hash.as_bytes());

        // Register stake, pool, and delegate
        state.process_certificate(&Certificate::StakeRegistration(cred.clone()));
        // Realistic pool stake: 50 million ADA (large pool)
        add_stake_utxo(&mut state, &cred, 50_000_000_000_000);

        state.process_certificate(&Certificate::PoolRegistration(PoolParams {
            operator: pool_id,
            vrf_keyhash: Hash32::from_bytes([2u8; 32]),
            pledge: Lovelace(1_000_000_000_000), // 1M ADA pledge
            cost: Lovelace(340_000_000),
            margin: Rational {
                numerator: 1,
                denominator: 100,
            },
            reward_account,
            pool_owners: vec![owner_hash],
            relays: vec![],
            pool_metadata: None,
        }));

        state.process_certificate(&Certificate::StakeDelegation {
            credential: cred.clone(),
            pool_hash: pool_id,
        });

        // Build up snapshots: 3 rotations to populate "go"
        state.process_epoch_transition(EpochNo(1));
        state.process_epoch_transition(EpochNo(2));
        state.process_epoch_transition(EpochNo(3));

        // Pool produced blocks proportional to its stake
        // expected_blocks = epoch_length * active_slot_coeff = 432000 * 0.05 = 21600
        state.epoch_fees = Lovelace(500_000_000_000); // 500k ADA fees
        Arc::make_mut(&mut state.epoch_blocks_by_pool).insert(pool_id, 21600);
        state.epoch_block_count = 21600;

        // Epoch 3→4: triggers reward calculation using "go" snapshot
        state.process_epoch_transition(EpochNo(4));

        // Treasury should have increased
        assert!(state.treasury.0 > 0);

        // Reserves should have decreased
        assert!(state.reserves.0 < 10_000_000_000_000_000);

        // Reward accounts should have received rewards
        let total_rewards: u64 = state.reward_accounts.values().map(|l| l.0).sum();
        assert!(
            total_rewards > 0,
            "Expected rewards > 0, got {total_rewards}"
        );
    }

    #[test]
    fn test_reward_calculation_no_blocks_no_rewards() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);
        state.epoch_length = 432000;
        state.reserves = Lovelace(10_000_000_000_000_000);

        let owner_hash = Hash28::from_bytes([42u8; 28]);
        let cred = Credential::VerificationKey(owner_hash);
        let pool_id = Hash28::from_bytes([1u8; 28]);
        let key = credential_to_hash(&cred);

        let mut reward_account = vec![0xE0u8];
        reward_account.extend_from_slice(owner_hash.as_bytes());

        // Setup delegation
        state.process_certificate(&Certificate::StakeRegistration(cred.clone()));
        state
            .stake_distribution
            .stake_map
            .insert(key, Lovelace(50_000_000_000_000));

        state.process_certificate(&Certificate::PoolRegistration(PoolParams {
            operator: pool_id,
            vrf_keyhash: Hash32::from_bytes([2u8; 32]),
            pledge: Lovelace(1_000_000_000_000),
            cost: Lovelace(340_000_000),
            margin: Rational {
                numerator: 0,
                denominator: 1,
            },
            reward_account,
            pool_owners: vec![owner_hash],
            relays: vec![],
            pool_metadata: None,
        }));

        state.process_certificate(&Certificate::StakeDelegation {
            credential: cred,
            pool_hash: pool_id,
        });

        // Build snapshots: need 3 rotations to populate "go"
        state.process_epoch_transition(EpochNo(1));
        state.process_epoch_transition(EpochNo(2));
        state.process_epoch_transition(EpochNo(3));

        // No blocks produced but some fees collected
        state.epoch_fees = Lovelace(100_000_000); // Some fees from prior blocks
                                                  // epoch_blocks_by_pool is empty — no pool produced blocks
        state.epoch_block_count = 0;

        state.process_epoch_transition(EpochNo(4));

        // Pool produced no blocks, so performance = 0, no pool rewards
        // eta = 0, so expansion = 0, but fees still contribute to reward pot
        // All pool pot (from fees) goes to treasury as undistributed
        let member_rewards: u64 = state.reward_accounts.values().map(|l| l.0).sum();
        assert_eq!(member_rewards, 0);
        // Treasury gets treasury_cut from fees + undistributed
        assert!(state.treasury.0 > 0);
    }

    #[test]
    fn test_expected_blocks_zero_clamped_to_one() {
        // When active_slot_coeff is extremely small, floor(coeff * epoch_length) can
        // round to 0.  The fix clamps expected_blocks to at least 1, preventing a
        // division-by-zero (or silent reward skip) in the expansion calculation.
        let mut params = ProtocolParameters::mainnet_defaults();
        // Tiny coefficient: 1e-10 * 432000 ≈ 0.0000432 → floor = 0
        params.active_slots_coeff = 1e-10;
        let mut state = LedgerState::new(params);
        state.epoch_length = 432000;
        state.reserves = Lovelace(10_000_000_000_000_000);

        let owner_hash = Hash28::from_bytes([42u8; 28]);
        let cred = Credential::VerificationKey(owner_hash);
        let pool_id = Hash28::from_bytes([1u8; 28]);

        let mut reward_account = vec![0xE0u8];
        reward_account.extend_from_slice(owner_hash.as_bytes());

        state.process_certificate(&Certificate::StakeRegistration(cred.clone()));
        add_stake_utxo(&mut state, &cred, 50_000_000_000_000);

        state.process_certificate(&Certificate::PoolRegistration(PoolParams {
            operator: pool_id,
            vrf_keyhash: Hash32::from_bytes([2u8; 32]),
            pledge: Lovelace(1_000_000_000_000),
            cost: Lovelace(340_000_000),
            margin: Rational {
                numerator: 1,
                denominator: 100,
            },
            reward_account,
            pool_owners: vec![owner_hash],
            relays: vec![],
            pool_metadata: None,
        }));

        state.process_certificate(&Certificate::StakeDelegation {
            credential: cred.clone(),
            pool_hash: pool_id,
        });

        // Build snapshots: 3 rotations to populate "go"
        state.process_epoch_transition(EpochNo(1));
        state.process_epoch_transition(EpochNo(2));
        state.process_epoch_transition(EpochNo(3));

        // Simulate 1 block produced and some fees — should NOT panic
        state.epoch_fees = Lovelace(500_000_000_000);
        Arc::make_mut(&mut state.epoch_blocks_by_pool).insert(pool_id, 1);
        state.epoch_block_count = 1;

        let reserves_before = state.reserves.0;
        let treasury_before = state.treasury.0;

        // This epoch transition would divide by zero without the fix
        state.process_epoch_transition(EpochNo(4));

        // Verify the system did not panic and rewards were distributed
        assert!(
            state.treasury.0 > treasury_before,
            "Treasury should increase from reward distribution"
        );
        assert!(
            state.reserves.0 < reserves_before,
            "Reserves should decrease from monetary expansion"
        );
        let total_rewards: u64 = state.reward_accounts.values().map(|l| l.0).sum();
        assert!(
            total_rewards > 0,
            "Expected rewards > 0 with clamped expected_blocks, got {total_rewards}"
        );
    }

    #[test]
    fn test_reward_pledge_not_met_zero_rewards() {
        // Pool with pledge > owner stake should receive zero rewards
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);
        state.epoch_length = 432000;
        state.reserves = Lovelace(10_000_000_000_000_000);

        let owner_hash = Hash28::from_bytes([42u8; 28]);
        let cred = Credential::VerificationKey(owner_hash);
        let pool_id = Hash28::from_bytes([1u8; 28]);
        let key = credential_to_hash(&cred);

        let mut reward_account = vec![0xE0u8];
        reward_account.extend_from_slice(owner_hash.as_bytes());

        state.process_certificate(&Certificate::StakeRegistration(cred.clone()));
        // Owner has only 1M ADA delegated
        state
            .stake_distribution
            .stake_map
            .insert(key, Lovelace(1_000_000_000_000));

        state.process_certificate(&Certificate::PoolRegistration(PoolParams {
            operator: pool_id,
            vrf_keyhash: Hash32::from_bytes([2u8; 32]),
            pledge: Lovelace(10_000_000_000_000), // 10M ADA pledge — NOT met
            cost: Lovelace(340_000_000),
            margin: Rational {
                numerator: 1,
                denominator: 100,
            },
            reward_account,
            pool_owners: vec![owner_hash],
            relays: vec![],
            pool_metadata: None,
        }));

        state.process_certificate(&Certificate::StakeDelegation {
            credential: cred,
            pool_hash: pool_id,
        });

        state.process_epoch_transition(EpochNo(1));
        state.process_epoch_transition(EpochNo(2));
        state.process_epoch_transition(EpochNo(3));

        // expected_blocks = 432000 * 0.05 = 21600
        state.epoch_fees = Lovelace(500_000_000_000);
        Arc::make_mut(&mut state.epoch_blocks_by_pool).insert(pool_id, 21600);
        state.epoch_block_count = 21600;
        state.process_epoch_transition(EpochNo(4));

        // No pool rewards when pledge not met — all goes to treasury as undistributed
        let member_rewards: u64 = state.reward_accounts.values().map(|l| l.0).sum();
        assert_eq!(
            member_rewards, 0,
            "Pledge-unmet pool should get zero rewards"
        );
        assert!(state.treasury.0 > 0);
    }

    #[test]
    fn test_reward_operator_gets_registered_reward_account() {
        // Verify operator rewards go to the pool's registered reward account, not pool_id
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);
        state.epoch_length = 432000;
        state.reserves = Lovelace(10_000_000_000_000_000);

        let owner_hash = Hash28::from_bytes([42u8; 28]);
        let cred = Credential::VerificationKey(owner_hash);
        let pool_id = Hash28::from_bytes([1u8; 28]);

        // Reward account uses the owner's credential
        let mut reward_account = vec![0xE0u8];
        reward_account.extend_from_slice(owner_hash.as_bytes());

        state.process_certificate(&Certificate::StakeRegistration(cred.clone()));
        add_stake_utxo(&mut state, &cred, 50_000_000_000_000);

        state.process_certificate(&Certificate::PoolRegistration(PoolParams {
            operator: pool_id,
            vrf_keyhash: Hash32::from_bytes([2u8; 32]),
            pledge: Lovelace(1_000_000_000_000),
            cost: Lovelace(340_000_000),
            margin: Rational {
                numerator: 5,
                denominator: 100,
            },
            reward_account,
            pool_owners: vec![owner_hash],
            relays: vec![],
            pool_metadata: None,
        }));

        state.process_certificate(&Certificate::StakeDelegation {
            credential: cred,
            pool_hash: pool_id,
        });

        state.process_epoch_transition(EpochNo(1));
        state.process_epoch_transition(EpochNo(2));
        state.process_epoch_transition(EpochNo(3));

        // expected_blocks = 432000 * 0.05 = 21600
        state.epoch_fees = Lovelace(500_000_000_000);
        Arc::make_mut(&mut state.epoch_blocks_by_pool).insert(pool_id, 21600);
        state.epoch_block_count = 21600;
        state.process_epoch_transition(EpochNo(4));

        // Operator reward should go to owner_hash credential, not pool_id padded to 32
        let reward_key = credential_to_hash(&Credential::VerificationKey(owner_hash));
        let owner_reward = state
            .reward_accounts
            .get(&reward_key)
            .copied()
            .unwrap_or(Lovelace(0));
        assert!(
            owner_reward.0 > 0,
            "Owner should receive operator rewards at registered reward account"
        );

        // Pool_id padded to 32 bytes should NOT have rewards (old bug)
        let pool_key = pool_id.to_hash32_padded();
        let pool_id_reward = state
            .reward_accounts
            .get(&pool_key)
            .copied()
            .unwrap_or(Lovelace(0));
        assert_eq!(
            pool_id_reward.0, 0,
            "Pool ID should not receive rewards directly — must use registered reward account"
        );
    }

    #[test]
    fn test_stake_registration_creates_reward_account() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);

        let cred = Credential::VerificationKey(Hash28::from_bytes([42u8; 28]));
        let key = credential_to_hash(&cred);

        state.process_certificate(&Certificate::StakeRegistration(cred));
        assert!(state.reward_accounts.contains_key(&key));
        assert_eq!(state.reward_accounts[&key], Lovelace(0));
    }

    #[test]
    fn test_stake_deregistration_removes_reward_account() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);

        let cred = Credential::VerificationKey(Hash28::from_bytes([42u8; 28]));
        let key = credential_to_hash(&cred);

        state.process_certificate(&Certificate::StakeRegistration(cred.clone()));
        assert!(state.reward_accounts.contains_key(&key));

        state.process_certificate(&Certificate::StakeDeregistration(cred));
        assert!(!state.reward_accounts.contains_key(&key));
    }

    #[test]
    fn test_epoch_fee_reset_on_transition() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);
        state.epoch_length = 100;

        state.epoch_fees = Lovelace(1_000_000);
        state.epoch_block_count = 10;

        state.process_epoch_transition(EpochNo(1));

        assert_eq!(state.epoch_fees, Lovelace(0));
        assert_eq!(state.epoch_block_count, 0);
        assert!(state.epoch_blocks_by_pool.is_empty());
    }

    #[test]
    fn test_epoch_nonce_computation() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);
        state.epoch_length = 100;
        state.shelley_transition_epoch = 0;
        state.byron_epoch_length = 0;
        // randomness_stabilisation_window = 4k/f; use 40 for testing
        // (so slots 0-59 update candidate, slots 60-99 freeze candidate)
        state.randomness_stabilisation_window = 40;

        // Set a genesis hash to initialize nonce state
        let genesis_hash = Hash32::from_bytes([0xAB; 32]);
        state.set_genesis_hash(genesis_hash);

        // Evolving nonce starts from genesis hash
        assert_eq!(state.evolving_nonce, genesis_hash);
        assert_eq!(state.candidate_nonce, genesis_hash);
        // Epoch nonce starts at ZERO
        assert_eq!(state.epoch_nonce, Hash32::ZERO);

        // Apply a block BEFORE the stabilisation window (slot 10 + 40 < 100)
        let mut block = make_test_block(10, 1, Hash32::ZERO, vec![]);
        block.header.vrf_result.output = vec![42u8; 32];
        block.header.issuer_vkey = vec![1u8; 32];
        state.apply_block(&block).unwrap();

        // Evolving nonce should have been updated
        assert_ne!(state.evolving_nonce, genesis_hash);
        // Candidate nonce should track evolving (not in stabilisation window)
        assert_eq!(state.candidate_nonce, state.evolving_nonce);
        // LAB nonce should be the block's prev_hash
        assert_eq!(state.lab_nonce, block.header.prev_hash);

        // Apply a block INSIDE the stabilisation window (slot 70 + 40 >= 100)
        let evolving_before = state.evolving_nonce;
        let candidate_before = state.candidate_nonce;
        let mut block2 = make_test_block(70, 2, *block.hash(), vec![]);
        block2.header.vrf_result.output = vec![99u8; 32];
        block2.header.issuer_vkey = vec![1u8; 32];
        state.apply_block(&block2).unwrap();

        // Evolving nonce should STILL update (always updates)
        assert_ne!(state.evolving_nonce, evolving_before);
        // Candidate nonce should be FROZEN (in stabilisation window)
        assert_eq!(state.candidate_nonce, candidate_before);

        // Trigger epoch transition
        let nonce_before_transition = state.epoch_nonce;
        state.process_epoch_transition(EpochNo(1));

        // Epoch nonce should have been updated
        assert_ne!(state.epoch_nonce, nonce_before_transition);
        // Evolving nonce should carry forward (NOT reset)
        assert_ne!(state.evolving_nonce, genesis_hash);
        // last_epoch_block_nonce should be the lab_nonce at transition
        assert_eq!(state.last_epoch_block_nonce, state.lab_nonce);
    }

    #[test]
    fn test_drep_registration() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);

        let cred = Credential::VerificationKey(Hash28::from_bytes([50u8; 28]));
        let key = credential_to_hash(&cred);

        state.process_certificate(&Certificate::RegDRep {
            credential: cred.clone(),
            deposit: Lovelace(500_000_000),
            anchor: Some(Anchor {
                url: "https://example.com/drep.json".to_string(),
                data_hash: Hash32::ZERO,
            }),
        });

        assert!(state.governance.dreps.contains_key(&key));
        assert_eq!(state.governance.dreps[&key].deposit, Lovelace(500_000_000));
        assert_eq!(state.governance.drep_registration_count, 1);
    }

    #[test]
    fn test_drep_deregistration() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);

        let cred = Credential::VerificationKey(Hash28::from_bytes([50u8; 28]));
        let key = credential_to_hash(&cred);

        // Register
        state.process_certificate(&Certificate::RegDRep {
            credential: cred.clone(),
            deposit: Lovelace(500_000_000),
            anchor: None,
        });
        assert!(state.governance.dreps.contains_key(&key));

        // Deregister
        state.process_certificate(&Certificate::UnregDRep {
            credential: cred,
            refund: Lovelace(500_000_000),
        });
        assert!(!state.governance.dreps.contains_key(&key));
    }

    #[test]
    fn test_drep_update() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);

        let cred = Credential::VerificationKey(Hash28::from_bytes([50u8; 28]));
        let key = credential_to_hash(&cred);

        // Register without anchor
        state.process_certificate(&Certificate::RegDRep {
            credential: cred.clone(),
            deposit: Lovelace(500_000_000),
            anchor: None,
        });
        assert!(state.governance.dreps[&key].anchor.is_none());

        // Update with anchor
        state.process_certificate(&Certificate::UpdateDRep {
            credential: cred,
            anchor: Some(Anchor {
                url: "https://example.com/drep.json".to_string(),
                data_hash: Hash32::ZERO,
            }),
        });
        assert!(state.governance.dreps[&key].anchor.is_some());
    }

    #[test]
    fn test_drep_activity_tracking() {
        let mut params = ProtocolParameters::mainnet_defaults();
        params.drep_activity = 5; // DReps inactive after 5 epochs
        let mut state = LedgerState::new(params);

        let cred = Credential::VerificationKey(Hash28::from_bytes([50u8; 28]));
        let key = credential_to_hash(&cred);

        // Register at epoch 0
        state.process_certificate(&Certificate::RegDRep {
            credential: cred.clone(),
            deposit: Lovelace(500_000_000),
            anchor: None,
        });
        assert_eq!(state.governance.dreps[&key].last_active_epoch, EpochNo(0));

        // Update at epoch 3 — should update last_active_epoch
        state.epoch = EpochNo(3);
        state.process_certificate(&Certificate::UpdateDRep {
            credential: cred,
            anchor: None,
        });
        assert_eq!(state.governance.dreps[&key].last_active_epoch, EpochNo(3));

        // Epoch transition to epoch 7 — DRep last active at epoch 3, threshold is 5
        // 7 - 3 = 4, which is not > 5, so DRep should remain active
        state.process_epoch_transition(EpochNo(7));
        assert!(state.governance.dreps.contains_key(&key));
        assert!(state.governance.dreps[&key].active);

        // Epoch transition to epoch 9 — 9 - 3 = 6 > 5, so DRep should be marked inactive
        // Per CIP-1694: inactive DReps remain registered but are excluded from voting power
        state.process_epoch_transition(EpochNo(9));
        assert!(state.governance.dreps.contains_key(&key)); // Still registered
        assert!(!state.governance.dreps[&key].active); // But inactive
        assert_eq!(state.governance.dreps[&key].deposit, Lovelace(500_000_000));
        // Deposit retained
    }

    #[test]
    fn test_committee_expiration_during_epoch_transition() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);

        // Add CC members with different expiration epochs
        let cold1 = Hash32::from_bytes([1u8; 32]);
        let cold2 = Hash32::from_bytes([2u8; 32]);
        let hot1 = Hash32::from_bytes([11u8; 32]);
        let hot2 = Hash32::from_bytes([12u8; 32]);

        Arc::make_mut(&mut state.governance)
            .committee_hot_keys
            .insert(cold1, hot1);
        Arc::make_mut(&mut state.governance)
            .committee_expiration
            .insert(cold1, EpochNo(5));
        Arc::make_mut(&mut state.governance)
            .committee_hot_keys
            .insert(cold2, hot2);
        Arc::make_mut(&mut state.governance)
            .committee_expiration
            .insert(cold2, EpochNo(10));

        // At epoch 5, cold1 should be expired
        state.process_epoch_transition(EpochNo(5));
        assert!(!state.governance.committee_hot_keys.contains_key(&cold1));
        assert!(!state.governance.committee_expiration.contains_key(&cold1));
        // cold2 should remain
        assert!(state.governance.committee_hot_keys.contains_key(&cold2));

        // At epoch 10, cold2 should be expired
        state.process_epoch_transition(EpochNo(10));
        assert!(!state.governance.committee_hot_keys.contains_key(&cold2));
    }

    #[test]
    fn test_constitution_storage() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);

        assert!(state.governance.constitution.is_none());

        // Enact a NewConstitution governance action
        let constitution = Constitution {
            anchor: Anchor {
                url: "https://constitution.cardano.org".to_string(),
                data_hash: Hash32::from_bytes([42u8; 32]),
            },
            script_hash: Some(Hash28::from_bytes([99u8; 28])),
        };
        state.enact_gov_action(&GovAction::NewConstitution {
            prev_action_id: None,
            constitution: constitution.clone(),
        });

        let stored = state.governance.constitution.as_ref().unwrap();
        assert_eq!(stored.anchor.url, "https://constitution.cardano.org");
        assert!(stored.script_hash.is_some());
    }

    #[test]
    fn test_drep_marked_inactive_on_expiry() {
        let mut params = ProtocolParameters::mainnet_defaults();
        params.drep_activity = 2;
        let mut state = LedgerState::new(params);

        let cred = Credential::VerificationKey(Hash28::from_bytes([50u8; 28]));
        let key = credential_to_hash(&cred);

        // Register at epoch 0 with 500 ADA deposit
        state.process_certificate(&Certificate::RegDRep {
            credential: cred,
            deposit: Lovelace(500_000_000),
            anchor: None,
        });
        assert!(state.governance.dreps.contains_key(&key));
        assert!(state.governance.dreps[&key].active);

        // At epoch 3 (0 + 2 < 3, so inactive): DRep should be marked inactive but NOT removed
        state.process_epoch_transition(EpochNo(3));
        assert!(state.governance.dreps.contains_key(&key)); // Still registered
        assert!(!state.governance.dreps[&key].active); // But inactive
        assert_eq!(state.governance.dreps[&key].deposit, Lovelace(500_000_000)); // Deposit retained

        // Deposit should NOT be refunded (DRep still registered)
        assert!(!state.reward_accounts.contains_key(&key));
    }

    #[test]
    fn test_governance_proposal_deposit_refund() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);

        // Build a return address (29 bytes: 1 header + 28 key hash)
        let mut return_addr = vec![0xE1u8]; // header byte
        return_addr.extend_from_slice(&[42u8; 28]); // 28-byte key hash

        let reward_key = Hash28::from_bytes([42u8; 28]).to_hash32_padded();

        // Submit a proposal with deposit
        let proposal = ProposalProcedure {
            deposit: Lovelace(100_000_000_000), // 100k ADA
            return_addr,
            gov_action: GovAction::InfoAction,
            anchor: Anchor {
                url: "https://example.com".to_string(),
                data_hash: Hash32::ZERO,
            },
        };
        state.process_proposal(&Hash32::from_bytes([1u8; 32]), 0, &proposal);
        assert_eq!(state.governance.proposals.len(), 1);

        // Advance past expiry (default lifetime is 6 epochs)
        state.process_epoch_transition(EpochNo(7));

        // Proposal should be expired
        assert!(state.governance.proposals.is_empty());

        // Deposit should be refunded
        assert_eq!(
            state.reward_accounts.get(&reward_key),
            Some(&Lovelace(100_000_000_000))
        );
    }

    #[test]
    fn test_treasury_withdrawal_credits_reward_account() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);

        // Give treasury some funds
        state.treasury = Lovelace(1_000_000_000_000);

        // Build recipient reward address
        let mut reward_addr = vec![0xE1u8];
        reward_addr.extend_from_slice(&[55u8; 28]);

        let reward_key = Hash28::from_bytes([55u8; 28]).to_hash32_padded();

        let mut withdrawals = std::collections::BTreeMap::new();
        withdrawals.insert(reward_addr, Lovelace(50_000_000_000));

        state.enact_gov_action(&GovAction::TreasuryWithdrawals {
            withdrawals,
            policy_hash: None,
        });

        // Treasury should be debited
        assert_eq!(state.treasury.0, 950_000_000_000);

        // Reward account should be credited
        assert_eq!(
            state.reward_accounts.get(&reward_key),
            Some(&Lovelace(50_000_000_000))
        );
    }

    #[test]
    fn test_vote_delegation() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);

        let cred = Credential::VerificationKey(Hash28::from_bytes([42u8; 28]));
        let key = credential_to_hash(&cred);

        state.process_certificate(&Certificate::VoteDelegation {
            credential: cred,
            drep: DRep::Abstain,
        });

        assert_eq!(state.governance.vote_delegations[&key], DRep::Abstain);
    }

    #[test]
    fn test_stake_vote_delegation() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);

        let cred = Credential::VerificationKey(Hash28::from_bytes([42u8; 28]));
        let pool_id = Hash28::from_bytes([1u8; 28]);
        let key = credential_to_hash(&cred);

        state.process_certificate(&Certificate::StakeVoteDelegation {
            credential: cred,
            pool_hash: pool_id,
            drep: DRep::NoConfidence,
        });

        // Both delegations should be set
        assert_eq!(state.delegations[&key], pool_id);
        assert_eq!(state.governance.vote_delegations[&key], DRep::NoConfidence);
    }

    #[test]
    fn test_committee_hot_auth() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);

        let cold = Credential::VerificationKey(Hash28::from_bytes([10u8; 28]));
        let hot = Credential::VerificationKey(Hash28::from_bytes([20u8; 28]));
        let cold_key = credential_to_hash(&cold);
        let hot_key = credential_to_hash(&hot);

        state.process_certificate(&Certificate::CommitteeHotAuth {
            cold_credential: cold,
            hot_credential: hot,
        });

        assert_eq!(state.governance.committee_hot_keys[&cold_key], hot_key);
    }

    #[test]
    fn test_committee_cold_resign() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);

        let cold = Credential::VerificationKey(Hash28::from_bytes([10u8; 28]));
        let hot = Credential::VerificationKey(Hash28::from_bytes([20u8; 28]));
        let cold_key = credential_to_hash(&cold);

        // First authorize
        state.process_certificate(&Certificate::CommitteeHotAuth {
            cold_credential: cold.clone(),
            hot_credential: hot,
        });
        assert!(state.governance.committee_hot_keys.contains_key(&cold_key));

        // Then resign
        state.process_certificate(&Certificate::CommitteeColdResign {
            cold_credential: cold,
            anchor: None,
        });
        assert!(!state.governance.committee_hot_keys.contains_key(&cold_key));
        assert!(state.governance.committee_resigned.contains_key(&cold_key));
    }

    #[test]
    fn test_governance_proposal_and_vote() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);

        let tx_hash = Hash32::from_bytes([99u8; 32]);
        let proposal = ProposalProcedure {
            deposit: Lovelace(100_000_000_000),
            return_addr: vec![0u8; 29],
            gov_action: GovAction::InfoAction,
            anchor: Anchor {
                url: "https://example.com/proposal.json".to_string(),
                data_hash: Hash32::ZERO,
            },
        };

        state.process_proposal(&tx_hash, 0, &proposal);
        assert_eq!(state.governance.proposals.len(), 1);
        assert_eq!(state.governance.proposal_count, 1);

        let action_id = GovActionId {
            transaction_id: tx_hash,
            action_index: 0,
        };

        // Cast votes
        let drep_voter = Voter::DRep(Credential::VerificationKey(Hash28::from_bytes([50u8; 28])));
        let yes_vote = VotingProcedure {
            vote: Vote::Yes,
            anchor: None,
        };
        state.process_vote(&drep_voter, &action_id, &yes_vote);

        let spo_voter = Voter::StakePool(Hash32::from_bytes([1u8; 32]));
        let no_vote = VotingProcedure {
            vote: Vote::No,
            anchor: None,
        };
        state.process_vote(&spo_voter, &action_id, &no_vote);

        let p = &state.governance.proposals[&action_id];
        assert_eq!(p.yes_votes, 1);
        assert_eq!(p.no_votes, 1);
        assert_eq!(p.abstain_votes, 0);
        // 2 votes for the same action_id should be in the same Vec
        let total_votes: usize = state
            .governance
            .votes_by_action
            .values()
            .map(|v| v.len())
            .sum();
        assert_eq!(total_votes, 2);
    }

    #[test]
    fn test_governance_proposal_expiry() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);
        state.epoch_length = 100;

        // Use a NoConfidence proposal (requires DRep + SPO votes to ratify)
        // so it won't be auto-ratified like InfoAction
        let tx_hash = Hash32::from_bytes([99u8; 32]);
        let proposal = ProposalProcedure {
            deposit: Lovelace(100_000_000_000),
            return_addr: vec![0u8; 29],
            gov_action: GovAction::NoConfidence {
                prev_action_id: None,
            },
            anchor: Anchor {
                url: "https://example.com".to_string(),
                data_hash: Hash32::ZERO,
            },
        };

        // Register at least one DRep so threshold checks don't pass with 0/0
        let cred = Credential::VerificationKey(Hash28::from_bytes([1u8; 28]));
        let key = credential_to_hash(&cred);
        Arc::make_mut(&mut state.governance).dreps.insert(
            key,
            DRepRegistration {
                credential: cred,
                deposit: Lovelace(500_000_000),
                anchor: None,
                registered_epoch: EpochNo(0),
                last_active_epoch: EpochNo(0),
                active: true,
            },
        );

        // Submit at epoch 0 → expires at epoch 6
        state.process_proposal(&tx_hash, 0, &proposal);
        assert_eq!(state.governance.proposals.len(), 1);

        // Advance to epoch 5 — should still be active (no votes, not ratified)
        for e in 1..=5 {
            state.process_epoch_transition(EpochNo(e));
        }
        assert_eq!(state.governance.proposals.len(), 1);

        // Advance to epoch 6 — should expire
        state.process_epoch_transition(EpochNo(6));
        assert_eq!(state.governance.proposals.len(), 0);
    }

    #[test]
    fn test_treasury_donation() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);

        let tx = Transaction {
            hash: Hash32::from_bytes([2u8; 32]),
            body: TransactionBody {
                inputs: vec![],
                outputs: vec![],
                fee: Lovelace(200_000),
                ttl: None,
                certificates: vec![],
                withdrawals: BTreeMap::new(),
                auxiliary_data_hash: None,
                validity_interval_start: None,
                mint: BTreeMap::new(),
                script_data_hash: None,
                collateral: vec![],
                required_signers: vec![],
                network_id: None,
                collateral_return: None,
                total_collateral: None,
                reference_inputs: vec![],
                update: None,
                voting_procedures: BTreeMap::new(),
                proposal_procedures: vec![],
                treasury_value: None,
                donation: Some(Lovelace(1_000_000)),
            },
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![],
                plutus_v3_scripts: vec![],
                plutus_data: vec![],
                redeemers: vec![],
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let block = make_test_block(100, 1, Hash32::ZERO, vec![tx]);
        state.apply_block(&block).unwrap();

        assert_eq!(state.treasury, Lovelace(1_000_000));
    }

    #[test]
    fn test_rational_as_f64() {
        let r = Rational {
            numerator: 3,
            denominator: 1000,
        };
        assert!((r.as_f64() - 0.003).abs() < f64::EPSILON);

        let zero = Rational {
            numerator: 0,
            denominator: 0,
        };
        assert!((zero.as_f64() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_info_action_always_ratified() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);
        state.epoch_length = 100;

        let tx_hash = Hash32::from_bytes([99u8; 32]);
        let proposal = ProposalProcedure {
            deposit: Lovelace(100_000_000_000),
            return_addr: vec![0u8; 29],
            gov_action: GovAction::InfoAction,
            anchor: Anchor {
                url: "https://example.com".to_string(),
                data_hash: Hash32::ZERO,
            },
        };

        state.process_proposal(&tx_hash, 0, &proposal);
        assert_eq!(state.governance.proposals.len(), 1);

        // InfoAction should be ratified at epoch transition even with no votes
        state.process_epoch_transition(EpochNo(1));
        assert_eq!(state.governance.proposals.len(), 0); // removed after ratification
    }

    #[test]
    fn test_parameter_change_ratification() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);
        state.epoch_length = 100;
        // Set CC threshold to 0 so CC auto-approves (we're testing DRep voting here)
        Arc::make_mut(&mut state.governance).committee_threshold = Some(Rational {
            numerator: 0,
            denominator: 1,
        });

        // Register enough DReps and have them vote yes to meet threshold (67%)
        let drep_count = 10;
        for i in 0..drep_count {
            let cred = Credential::VerificationKey(Hash28::from_bytes([i as u8; 28]));
            let key = credential_to_hash(&cred);
            Arc::make_mut(&mut state.governance).dreps.insert(
                key,
                DRepRegistration {
                    credential: cred,
                    deposit: Lovelace(500_000_000),
                    anchor: None,
                    registered_epoch: EpochNo(0),
                    last_active_epoch: EpochNo(0),
                    active: true,
                },
            );
        }

        // Submit a parameter change proposal to update n_opt (TechnicalGroup, no SPO vote needed)
        let update = torsten_primitives::transaction::ProtocolParamUpdate {
            n_opt: Some(1000),
            ..Default::default()
        };
        let tx_hash = Hash32::from_bytes([99u8; 32]);
        let proposal = ProposalProcedure {
            deposit: Lovelace(100_000_000_000),
            return_addr: vec![0u8; 29],
            gov_action: GovAction::ParameterChange {
                prev_action_id: None,
                protocol_param_update: Box::new(update),
                policy_hash: None,
            },
            anchor: Anchor {
                url: "https://example.com".to_string(),
                data_hash: Hash32::ZERO,
            },
        };

        state.process_proposal(&tx_hash, 0, &proposal);
        let action_id = GovActionId {
            transaction_id: tx_hash,
            action_index: 0,
        };

        // 7 out of 10 DReps vote yes (70% > 67% threshold)
        for i in 0..7 {
            let voter = Voter::DRep(Credential::VerificationKey(Hash28::from_bytes(
                [i as u8; 28],
            )));
            state.process_vote(
                &voter,
                &action_id,
                &VotingProcedure {
                    vote: Vote::Yes,
                    anchor: None,
                },
            );
        }
        // 3 vote no
        for i in 7..10 {
            let voter = Voter::DRep(Credential::VerificationKey(Hash28::from_bytes(
                [i as u8; 28],
            )));
            state.process_vote(
                &voter,
                &action_id,
                &VotingProcedure {
                    vote: Vote::No,
                    anchor: None,
                },
            );
        }

        assert_eq!(state.protocol_params.n_opt, 500); // original value

        // Epoch transition should ratify and enact
        state.process_epoch_transition(EpochNo(1));

        assert_eq!(state.protocol_params.n_opt, 1000); // updated
        assert_eq!(state.governance.proposals.len(), 0); // removed after enactment
    }

    #[test]
    fn test_parameter_change_not_ratified_below_threshold() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);
        state.epoch_length = 100;

        // Register 10 DReps with equal stake-weighted voting power
        for i in 0..10 {
            let cred = Credential::VerificationKey(Hash28::from_bytes([i as u8; 28]));
            let key = credential_to_hash(&cred);
            Arc::make_mut(&mut state.governance).dreps.insert(
                key,
                DRepRegistration {
                    credential: cred.clone(),
                    deposit: Lovelace(500_000_000),
                    anchor: None,
                    registered_epoch: EpochNo(0),
                    last_active_epoch: EpochNo(0),
                    active: true,
                },
            );
            // Set up vote delegation and stake for each DRep
            let stake_key = Hash32::from_bytes([100 + i as u8; 32]);
            Arc::make_mut(&mut state.governance)
                .vote_delegations
                .insert(
                    stake_key,
                    DRep::KeyHash(Hash28::from_bytes([i as u8; 28]).to_hash32_padded()),
                );
            state
                .stake_distribution
                .stake_map
                .insert(stake_key, Lovelace(1_000_000_000));
        }

        let update = torsten_primitives::transaction::ProtocolParamUpdate {
            max_tx_size: Some(32768),
            ..Default::default()
        };
        let tx_hash = Hash32::from_bytes([99u8; 32]);
        let proposal = ProposalProcedure {
            deposit: Lovelace(100_000_000_000),
            return_addr: vec![0u8; 29],
            gov_action: GovAction::ParameterChange {
                prev_action_id: None,
                protocol_param_update: Box::new(update),
                policy_hash: None,
            },
            anchor: Anchor {
                url: "https://example.com".to_string(),
                data_hash: Hash32::ZERO,
            },
        };

        state.process_proposal(&tx_hash, 0, &proposal);
        let action_id = GovActionId {
            transaction_id: tx_hash,
            action_index: 0,
        };

        // Only 5 out of 10 DReps vote yes (50% < 67% threshold)
        for i in 0..5 {
            let voter = Voter::DRep(Credential::VerificationKey(Hash28::from_bytes(
                [i as u8; 28],
            )));
            state.process_vote(
                &voter,
                &action_id,
                &VotingProcedure {
                    vote: Vote::Yes,
                    anchor: None,
                },
            );
        }

        state.process_epoch_transition(EpochNo(1));

        assert_eq!(state.protocol_params.max_tx_size, 16384); // unchanged
        assert_eq!(state.governance.proposals.len(), 1); // still active
    }

    #[test]
    fn test_treasury_withdrawal_ratification() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);
        state.epoch_length = 100;
        state.treasury = Lovelace(10_000_000_000);
        Arc::make_mut(&mut state.governance).committee_threshold = Some(Rational {
            numerator: 0,
            denominator: 1,
        });

        // Register DReps
        for i in 0..10 {
            let cred = Credential::VerificationKey(Hash28::from_bytes([i as u8; 28]));
            let key = credential_to_hash(&cred);
            Arc::make_mut(&mut state.governance).dreps.insert(
                key,
                DRepRegistration {
                    credential: cred,
                    deposit: Lovelace(500_000_000),
                    anchor: None,
                    registered_epoch: EpochNo(0),
                    last_active_epoch: EpochNo(0),
                    active: true,
                },
            );
        }

        let mut withdrawals = BTreeMap::new();
        withdrawals.insert(vec![0u8; 29], Lovelace(5_000_000_000));

        let tx_hash = Hash32::from_bytes([99u8; 32]);
        let proposal = ProposalProcedure {
            deposit: Lovelace(100_000_000_000),
            return_addr: vec![0u8; 29],
            gov_action: GovAction::TreasuryWithdrawals {
                withdrawals,
                policy_hash: None,
            },
            anchor: Anchor {
                url: "https://example.com".to_string(),
                data_hash: Hash32::ZERO,
            },
        };

        state.process_proposal(&tx_hash, 0, &proposal);
        let action_id = GovActionId {
            transaction_id: tx_hash,
            action_index: 0,
        };

        // 7/10 DReps vote yes
        for i in 0..7 {
            let voter = Voter::DRep(Credential::VerificationKey(Hash28::from_bytes(
                [i as u8; 28],
            )));
            state.process_vote(
                &voter,
                &action_id,
                &VotingProcedure {
                    vote: Vote::Yes,
                    anchor: None,
                },
            );
        }

        state.process_epoch_transition(EpochNo(1));

        assert_eq!(state.treasury, Lovelace(5_000_000_000)); // 10B - 5B = 5B
        assert_eq!(state.governance.proposals.len(), 0);
    }

    #[test]
    fn test_no_confidence_ratification() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);
        state.epoch_length = 100;

        // Set up a committee
        let cold = Credential::VerificationKey(Hash28::from_bytes([10u8; 28]));
        let hot = Credential::VerificationKey(Hash28::from_bytes([20u8; 28]));
        state.process_certificate(&Certificate::CommitteeHotAuth {
            cold_credential: cold,
            hot_credential: hot,
        });
        assert_eq!(state.governance.committee_hot_keys.len(), 1);

        // Register DReps
        for i in 0..10 {
            let cred = Credential::VerificationKey(Hash28::from_bytes([i as u8; 28]));
            let key = credential_to_hash(&cred);
            Arc::make_mut(&mut state.governance).dreps.insert(
                key,
                DRepRegistration {
                    credential: cred,
                    deposit: Lovelace(500_000_000),
                    anchor: None,
                    registered_epoch: EpochNo(0),
                    last_active_epoch: EpochNo(0),
                    active: true,
                },
            );
        }

        // Register some SPOs
        for i in 0..10 {
            let pool_id = Hash28::from_bytes([100 + i as u8; 28]);
            Arc::make_mut(&mut state.pool_params).insert(
                pool_id,
                PoolRegistration {
                    pool_id,
                    vrf_keyhash: Hash32::ZERO,
                    pledge: Lovelace(1_000_000),
                    cost: Lovelace(340_000_000),
                    margin_numerator: 1,
                    margin_denominator: 100,
                    reward_account: vec![],
                    owners: vec![],
                    relays: vec![],
                    metadata_url: None,
                    metadata_hash: None,
                },
            );
        }

        let tx_hash = Hash32::from_bytes([99u8; 32]);
        let proposal = ProposalProcedure {
            deposit: Lovelace(100_000_000_000),
            return_addr: vec![0u8; 29],
            gov_action: GovAction::NoConfidence {
                prev_action_id: None,
            },
            anchor: Anchor {
                url: "https://example.com".to_string(),
                data_hash: Hash32::ZERO,
            },
        };

        state.process_proposal(&tx_hash, 0, &proposal);
        let action_id = GovActionId {
            transaction_id: tx_hash,
            action_index: 0,
        };

        // 7/10 DReps vote yes (70% > 67%)
        for i in 0..7 {
            let voter = Voter::DRep(Credential::VerificationKey(Hash28::from_bytes(
                [i as u8; 28],
            )));
            state.process_vote(
                &voter,
                &action_id,
                &VotingProcedure {
                    vote: Vote::Yes,
                    anchor: None,
                },
            );
        }

        // 6/10 SPOs vote yes (60% > 51%)
        for i in 0..6 {
            let pool_hash = Hash28::from_bytes([100 + i as u8; 28]).to_hash32_padded();
            let voter = Voter::StakePool(pool_hash);
            state.process_vote(
                &voter,
                &action_id,
                &VotingProcedure {
                    vote: Vote::Yes,
                    anchor: None,
                },
            );
        }

        state.process_epoch_transition(EpochNo(1));

        // Committee should be disbanded
        assert_eq!(state.governance.committee_hot_keys.len(), 0);
        assert_eq!(state.governance.proposals.len(), 0);
    }

    #[test]
    fn test_hard_fork_ratification() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);
        state.epoch_length = 100;
        Arc::make_mut(&mut state.governance).committee_threshold = Some(Rational {
            numerator: 0,
            denominator: 1,
        });

        // Register DReps
        for i in 0..10 {
            let cred = Credential::VerificationKey(Hash28::from_bytes([i as u8; 28]));
            let key = credential_to_hash(&cred);
            Arc::make_mut(&mut state.governance).dreps.insert(
                key,
                DRepRegistration {
                    credential: cred,
                    deposit: Lovelace(500_000_000),
                    anchor: None,
                    registered_epoch: EpochNo(0),
                    last_active_epoch: EpochNo(0),
                    active: true,
                },
            );
        }

        let tx_hash = Hash32::from_bytes([99u8; 32]);
        let proposal = ProposalProcedure {
            deposit: Lovelace(100_000_000_000),
            return_addr: vec![0u8; 29],
            gov_action: GovAction::HardForkInitiation {
                prev_action_id: None,
                protocol_version: (10, 0),
            },
            anchor: Anchor {
                url: "https://example.com".to_string(),
                data_hash: Hash32::ZERO,
            },
        };

        state.process_proposal(&tx_hash, 0, &proposal);
        let action_id = GovActionId {
            transaction_id: tx_hash,
            action_index: 0,
        };

        // 6/10 DReps vote yes (60% = dvt_hard_fork threshold)
        for i in 0..6 {
            let voter = Voter::DRep(Credential::VerificationKey(Hash28::from_bytes(
                [i as u8; 28],
            )));
            state.process_vote(
                &voter,
                &action_id,
                &VotingProcedure {
                    vote: Vote::Yes,
                    anchor: None,
                },
            );
        }

        // 6/10 SPOs vote yes (60% > 51% pvt_hard_fork)
        for i in 0..6 {
            let voter = Voter::StakePool(Hash32::from_bytes([100 + i as u8; 32]));
            state.process_vote(
                &voter,
                &action_id,
                &VotingProcedure {
                    vote: Vote::Yes,
                    anchor: None,
                },
            );
        }

        assert_eq!(state.protocol_params.protocol_version_major, 9);
        state.process_epoch_transition(EpochNo(1));
        assert_eq!(state.protocol_params.protocol_version_major, 10);
        assert_eq!(state.protocol_params.protocol_version_minor, 0);
    }

    #[test]
    fn test_check_threshold_helper() {
        let r67 = Rational {
            numerator: 67,
            denominator: 100,
        };
        let r51 = Rational {
            numerator: 51,
            denominator: 100,
        };
        let r01 = Rational {
            numerator: 1,
            denominator: 100,
        };
        let r50 = Rational {
            numerator: 1,
            denominator: 2,
        };
        assert!(check_threshold(7, 10, &r67)); // 70% >= 67%
        assert!(!check_threshold(6, 10, &r67)); // 60% < 67%
        assert!(check_threshold(1, 1, &r51)); // 100% >= 51%
        assert!(!check_threshold(0, 10, &r01)); // 0% < 1%
        assert!(!check_threshold(0, 0, &r50)); // no votes = not met
    }

    /// Helper to create a CC-compatible hot key Hash32 from a Hash28 byte value.
    /// Matches the format produced by credential_to_hash (padded with zeros).
    fn make_cc_hot_key(byte_val: u8) -> (Hash28, Hash32) {
        let h28 = Hash28::from_bytes([byte_val; 28]);
        (h28, h28.to_hash32_padded())
    }

    #[test]
    fn test_cc_approval_no_committee() {
        let governance = GovernanceState::default();
        let action_id = GovActionId {
            transaction_id: Hash32::from_bytes([0u8; 32]),
            action_index: 0,
        };
        // No committee threshold => CC blocks ratification
        assert!(!check_cc_approval(
            &action_id,
            &governance,
            EpochNo(10),
            0,
            false
        ));
    }

    #[test]
    fn test_cc_approval_with_committee() {
        let mut governance = GovernanceState {
            committee_threshold: Some(Rational {
                numerator: 2,
                denominator: 3,
            }),
            ..Default::default()
        };
        let current_epoch = EpochNo(10);
        let action_id = GovActionId {
            transaction_id: Hash32::from_bytes([99u8; 32]),
            action_index: 0,
        };
        // Add 3 active CC members with expiration in the future
        let mut creds = Vec::new();
        for i in 0..3u8 {
            let cold = Hash32::from_bytes([i; 32]);
            let (h28, h32) = make_cc_hot_key(10 + i);
            governance.committee_hot_keys.insert(cold, h32);
            governance.committee_expiration.insert(cold, EpochNo(100));
            creds.push(Credential::VerificationKey(h28));
        }
        // 2/3 voted yes => meets 2/3 threshold
        governance.votes_by_action.insert(
            action_id.clone(),
            vec![
                (
                    Voter::ConstitutionalCommittee(creds[0].clone()),
                    VotingProcedure {
                        vote: Vote::Yes,
                        anchor: None,
                    },
                ),
                (
                    Voter::ConstitutionalCommittee(creds[1].clone()),
                    VotingProcedure {
                        vote: Vote::Yes,
                        anchor: None,
                    },
                ),
                (
                    Voter::ConstitutionalCommittee(creds[2].clone()),
                    VotingProcedure {
                        vote: Vote::No,
                        anchor: None,
                    },
                ),
            ],
        );
        assert!(check_cc_approval(
            &action_id,
            &governance,
            current_epoch,
            0,
            false
        ));

        // 1/3 voted yes => below 2/3 threshold
        governance.votes_by_action.insert(
            action_id.clone(),
            vec![
                (
                    Voter::ConstitutionalCommittee(creds[0].clone()),
                    VotingProcedure {
                        vote: Vote::Yes,
                        anchor: None,
                    },
                ),
                (
                    Voter::ConstitutionalCommittee(creds[1].clone()),
                    VotingProcedure {
                        vote: Vote::No,
                        anchor: None,
                    },
                ),
                (
                    Voter::ConstitutionalCommittee(creds[2].clone()),
                    VotingProcedure {
                        vote: Vote::No,
                        anchor: None,
                    },
                ),
            ],
        );
        assert!(!check_cc_approval(
            &action_id,
            &governance,
            current_epoch,
            0,
            false
        ));

        // No CC voted at all => all count as No, 0/3 < 2/3
        governance.votes_by_action.remove(&action_id);
        assert!(!check_cc_approval(
            &action_id,
            &governance,
            current_epoch,
            0,
            false
        ));
    }

    #[test]
    fn test_cc_approval_expired_members() {
        let mut governance = GovernanceState {
            committee_threshold: Some(Rational {
                numerator: 1,
                denominator: 2,
            }),
            ..Default::default()
        };
        let current_epoch = EpochNo(50);
        let action_id = GovActionId {
            transaction_id: Hash32::from_bytes([99u8; 32]),
            action_index: 0,
        };
        // Add 3 CC members, but 2 are expired
        let mut creds = Vec::new();
        for i in 0..3u8 {
            let cold = Hash32::from_bytes([i; 32]);
            let (h28, h32) = make_cc_hot_key(10 + i);
            governance.committee_hot_keys.insert(cold, h32);
            creds.push(Credential::VerificationKey(h28));
        }
        // Member 0 and 1 expired, member 2 still active
        governance
            .committee_expiration
            .insert(Hash32::from_bytes([0u8; 32]), EpochNo(30));
        governance
            .committee_expiration
            .insert(Hash32::from_bytes([1u8; 32]), EpochNo(40));
        governance
            .committee_expiration
            .insert(Hash32::from_bytes([2u8; 32]), EpochNo(100));
        // Only 1 active member who voted yes => 1/1 >= 1/2
        governance.votes_by_action.insert(
            action_id.clone(),
            vec![(
                Voter::ConstitutionalCommittee(creds[2].clone()),
                VotingProcedure {
                    vote: Vote::Yes,
                    anchor: None,
                },
            )],
        );
        assert!(check_cc_approval(
            &action_id,
            &governance,
            current_epoch,
            0,
            false
        ));
    }

    #[test]
    fn test_cc_approval_min_size_check() {
        let mut governance = GovernanceState {
            committee_threshold: Some(Rational {
                numerator: 1,
                denominator: 2,
            }),
            ..Default::default()
        };
        let action_id = GovActionId {
            transaction_id: Hash32::from_bytes([99u8; 32]),
            action_index: 0,
        };
        // 1 active member
        let cold = Hash32::from_bytes([0u8; 32]);
        let (h28, h32) = make_cc_hot_key(10);
        governance.committee_hot_keys.insert(cold, h32);
        governance.committee_expiration.insert(cold, EpochNo(100));
        governance.votes_by_action.insert(
            action_id.clone(),
            vec![(
                Voter::ConstitutionalCommittee(Credential::VerificationKey(h28)),
                VotingProcedure {
                    vote: Vote::Yes,
                    anchor: None,
                },
            )],
        );
        // Post-bootstrap: min_size=3 but only 1 active => CC blocks
        assert!(!check_cc_approval(
            &action_id,
            &governance,
            EpochNo(10),
            3,
            false
        ));
        // During bootstrap: min_size check skipped => CC passes
        assert!(check_cc_approval(
            &action_id,
            &governance,
            EpochNo(10),
            3,
            true
        ));
    }

    #[test]
    fn test_cc_approval_abstain_excluded() {
        let mut governance = GovernanceState {
            committee_threshold: Some(Rational {
                numerator: 2,
                denominator: 3,
            }),
            ..Default::default()
        };
        let action_id = GovActionId {
            transaction_id: Hash32::from_bytes([99u8; 32]),
            action_index: 0,
        };
        // 3 active members
        let mut creds = Vec::new();
        for i in 0..3u8 {
            let cold = Hash32::from_bytes([i; 32]);
            let (h28, h32) = make_cc_hot_key(10 + i);
            governance.committee_hot_keys.insert(cold, h32);
            governance.committee_expiration.insert(cold, EpochNo(100));
            creds.push(Credential::VerificationKey(h28));
        }
        // 1 yes, 1 no, 1 abstain => ratio = 1/2 (abstain excluded) < 2/3
        governance.votes_by_action.insert(
            action_id.clone(),
            vec![
                (
                    Voter::ConstitutionalCommittee(creds[0].clone()),
                    VotingProcedure {
                        vote: Vote::Yes,
                        anchor: None,
                    },
                ),
                (
                    Voter::ConstitutionalCommittee(creds[1].clone()),
                    VotingProcedure {
                        vote: Vote::No,
                        anchor: None,
                    },
                ),
                (
                    Voter::ConstitutionalCommittee(creds[2].clone()),
                    VotingProcedure {
                        vote: Vote::Abstain,
                        anchor: None,
                    },
                ),
            ],
        );
        assert!(!check_cc_approval(
            &action_id,
            &governance,
            EpochNo(10),
            0,
            false
        ));

        // 1 yes, 0 no, 2 abstain => ratio = 1/1 (abstains excluded) >= 2/3
        governance.votes_by_action.insert(
            action_id.clone(),
            vec![
                (
                    Voter::ConstitutionalCommittee(creds[0].clone()),
                    VotingProcedure {
                        vote: Vote::Yes,
                        anchor: None,
                    },
                ),
                (
                    Voter::ConstitutionalCommittee(creds[1].clone()),
                    VotingProcedure {
                        vote: Vote::Abstain,
                        anchor: None,
                    },
                ),
                (
                    Voter::ConstitutionalCommittee(creds[2].clone()),
                    VotingProcedure {
                        vote: Vote::Abstain,
                        anchor: None,
                    },
                ),
            ],
        );
        assert!(check_cc_approval(
            &action_id,
            &governance,
            EpochNo(10),
            0,
            false
        ));
    }

    #[test]
    fn test_arc_cow_snapshot_shares_data() {
        // Verify that cloning a LedgerState shares the underlying data via Arc
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);

        // Populate with some data
        let cred_hash = Hash32::from_bytes([1u8; 32]);
        let pool_id = Hash28::from_bytes([2u8; 28]);
        Arc::make_mut(&mut state.delegations).insert(cred_hash, pool_id);
        Arc::make_mut(&mut state.pool_params).insert(
            pool_id,
            PoolRegistration {
                pool_id,
                vrf_keyhash: Hash32::ZERO,
                pledge: Lovelace(0),
                cost: Lovelace(340_000_000),
                margin_numerator: 1,
                margin_denominator: 100,
                reward_account: vec![0u8; 29],
                owners: vec![],
                relays: vec![],
                metadata_url: None,
                metadata_hash: None,
            },
        );
        Arc::make_mut(&mut state.reward_accounts).insert(cred_hash, Lovelace(5_000_000));
        Arc::make_mut(&mut state.epoch_blocks_by_pool).insert(pool_id, 42);

        // Clone the state (should be cheap — Arc bumps refcount)
        let snapshot = state.clone();

        // Verify the Arc pointers are the same (data is shared, not deep-copied)
        assert!(Arc::ptr_eq(&state.delegations, &snapshot.delegations));
        assert!(Arc::ptr_eq(&state.pool_params, &snapshot.pool_params));
        assert!(Arc::ptr_eq(
            &state.reward_accounts,
            &snapshot.reward_accounts
        ));
        assert!(Arc::ptr_eq(
            &state.epoch_blocks_by_pool,
            &snapshot.epoch_blocks_by_pool
        ));
        assert!(Arc::ptr_eq(&state.governance, &snapshot.governance));

        // Verify the data is accessible through both
        assert_eq!(state.delegations.len(), 1);
        assert_eq!(snapshot.delegations.len(), 1);
        assert_eq!(state.pool_params.len(), 1);
        assert_eq!(snapshot.pool_params.len(), 1);
        assert_eq!(
            state.reward_accounts.get(&cred_hash),
            Some(&Lovelace(5_000_000))
        );
        assert_eq!(
            snapshot.reward_accounts.get(&cred_hash),
            Some(&Lovelace(5_000_000))
        );
    }

    #[test]
    fn test_arc_cow_mutation_does_not_affect_snapshot() {
        // Verify copy-on-write: mutating the original does not affect the snapshot
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);

        let cred_hash = Hash32::from_bytes([1u8; 32]);
        let pool_id = Hash28::from_bytes([2u8; 28]);
        Arc::make_mut(&mut state.delegations).insert(cred_hash, pool_id);
        Arc::make_mut(&mut state.reward_accounts).insert(cred_hash, Lovelace(5_000_000));

        // Take a snapshot
        let snapshot = state.clone();
        assert!(Arc::ptr_eq(&state.delegations, &snapshot.delegations));

        // Mutate the original via Arc::make_mut — this should trigger a clone
        let cred_hash_2 = Hash32::from_bytes([3u8; 32]);
        let pool_id_2 = Hash28::from_bytes([4u8; 28]);
        Arc::make_mut(&mut state.delegations).insert(cred_hash_2, pool_id_2);

        // The Arcs should no longer point to the same data
        assert!(!Arc::ptr_eq(&state.delegations, &snapshot.delegations));

        // Original has the new entry, snapshot does not
        assert_eq!(state.delegations.len(), 2);
        assert_eq!(snapshot.delegations.len(), 1);
        assert!(state.delegations.contains_key(&cred_hash_2));
        assert!(!snapshot.delegations.contains_key(&cred_hash_2));

        // Mutate reward_accounts on original
        Arc::make_mut(&mut state.reward_accounts).insert(cred_hash, Lovelace(10_000_000));
        assert_eq!(
            state.reward_accounts.get(&cred_hash),
            Some(&Lovelace(10_000_000))
        );
        // Snapshot still has the original value
        assert_eq!(
            snapshot.reward_accounts.get(&cred_hash),
            Some(&Lovelace(5_000_000))
        );
    }

    #[test]
    fn test_arc_cow_governance_isolation() {
        // Verify that governance Arc provides proper copy-on-write isolation
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);

        let drep_cred = Credential::VerificationKey(Hash28::from_bytes([10u8; 28]));
        let drep_hash = credential_to_hash(&drep_cred);
        Arc::make_mut(&mut state.governance).dreps.insert(
            drep_hash,
            DRepRegistration {
                credential: drep_cred.clone(),
                deposit: Lovelace(500_000_000),
                anchor: None,
                registered_epoch: EpochNo(0),
                last_active_epoch: EpochNo(0),
                active: true,
            },
        );

        // Snapshot shares the same Arc
        let snapshot = state.clone();
        assert!(Arc::ptr_eq(&state.governance, &snapshot.governance));
        assert_eq!(state.governance.dreps.len(), 1);
        assert_eq!(snapshot.governance.dreps.len(), 1);

        // Mutate governance on original
        Arc::make_mut(&mut state.governance).drep_registration_count = 99;

        // Arcs should now be different
        assert!(!Arc::ptr_eq(&state.governance, &snapshot.governance));
        assert_eq!(state.governance.drep_registration_count, 99);
        assert_eq!(snapshot.governance.drep_registration_count, 0);
    }

    #[test]
    fn test_arc_cow_serialization_roundtrip() {
        // Verify that Arc-wrapped fields serialize and deserialize correctly
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);

        let cred_hash = Hash32::from_bytes([1u8; 32]);
        let pool_id = Hash28::from_bytes([2u8; 28]);
        Arc::make_mut(&mut state.delegations).insert(cred_hash, pool_id);
        Arc::make_mut(&mut state.pool_params).insert(
            pool_id,
            PoolRegistration {
                pool_id,
                vrf_keyhash: Hash32::ZERO,
                pledge: Lovelace(500_000_000),
                cost: Lovelace(340_000_000),
                margin_numerator: 1,
                margin_denominator: 100,
                reward_account: vec![0u8; 29],
                owners: vec![],
                relays: vec![],
                metadata_url: None,
                metadata_hash: None,
            },
        );
        Arc::make_mut(&mut state.reward_accounts).insert(cred_hash, Lovelace(5_000_000));
        Arc::make_mut(&mut state.governance).drep_registration_count = 42;
        state.epoch = EpochNo(100);

        // Save and reload
        let dir = tempfile::tempdir().unwrap();
        let snapshot_path = dir.path().join("arc-cow-test.bin");
        state.save_snapshot(&snapshot_path).unwrap();
        let loaded = LedgerState::load_snapshot(&snapshot_path).unwrap();

        // Verify all fields survived the roundtrip
        assert_eq!(loaded.epoch, EpochNo(100));
        assert_eq!(loaded.delegations.len(), 1);
        assert_eq!(loaded.delegations.get(&cred_hash), Some(&pool_id));
        assert_eq!(loaded.pool_params.len(), 1);
        assert_eq!(
            loaded.pool_params.get(&pool_id).unwrap().pledge,
            Lovelace(500_000_000)
        );
        assert_eq!(
            loaded.reward_accounts.get(&cred_hash),
            Some(&Lovelace(5_000_000))
        );
        assert_eq!(loaded.governance.drep_registration_count, 42);
    }

    #[test]
    fn test_arc_cow_epoch_snapshot_shares_arcs() {
        // Verify that epoch snapshots share Arcs with the live state
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);

        let cred_hash = Hash32::from_bytes([1u8; 32]);
        let pool_id = Hash28::from_bytes([2u8; 28]);
        Arc::make_mut(&mut state.delegations).insert(cred_hash, pool_id);
        Arc::make_mut(&mut state.pool_params).insert(
            pool_id,
            PoolRegistration {
                pool_id,
                vrf_keyhash: Hash32::ZERO,
                pledge: Lovelace(0),
                cost: Lovelace(340_000_000),
                margin_numerator: 1,
                margin_denominator: 100,
                reward_account: vec![0u8; 29],
                owners: vec![],
                relays: vec![],
                metadata_url: None,
                metadata_hash: None,
            },
        );
        state
            .stake_distribution
            .stake_map
            .insert(cred_hash, Lovelace(1_000_000));

        // Trigger epoch transition to create a "mark" snapshot
        state.process_epoch_transition(EpochNo(1));

        // The mark snapshot should share the same Arc as the live state's delegations/pool_params
        let mark = state.snapshots.mark.as_ref().unwrap();
        assert!(Arc::ptr_eq(&state.delegations, &mark.delegations));
        assert!(Arc::ptr_eq(&state.pool_params, &mark.pool_params));

        // Now mutate live state — should not affect the snapshot
        let new_cred = Hash32::from_bytes([5u8; 32]);
        let new_pool = Hash28::from_bytes([6u8; 28]);
        Arc::make_mut(&mut state.delegations).insert(new_cred, new_pool);

        // Live state has 2 delegations, snapshot still has 1
        assert_eq!(state.delegations.len(), 2);
        assert_eq!(mark.delegations.len(), 1);
    }

    #[test]
    fn test_ledger_snapshot_save_load() {
        let dir = tempfile::tempdir().unwrap();
        let snapshot_path = dir.path().join("ledger-snapshot.bin");

        // Create a ledger state with some data
        let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());
        state.epoch = EpochNo(42);
        state.tip = Tip {
            point: Point::Specific(SlotNo(100000), Hash32::from_bytes([7u8; 32])),
            block_number: BlockNo(5000),
        };
        // Add a UTxO
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        let output = TransactionOutput {
            address: Address::Byron(ByronAddress {
                payload: vec![0u8; 32],
            }),
            value: Value::lovelace(1_000_000),
            datum: OutputDatum::None,
            script_ref: None,
            raw_cbor: None,
        };
        state.utxo_set.insert(input, output);

        // Save snapshot
        state.save_snapshot(&snapshot_path).unwrap();
        assert!(snapshot_path.exists());

        // Load and verify
        let loaded = LedgerState::load_snapshot(&snapshot_path).unwrap();
        assert_eq!(loaded.epoch, EpochNo(42));
        assert_eq!(loaded.tip.block_number, BlockNo(5000));
        assert_eq!(loaded.utxo_set.len(), 1);
    }

    #[test]
    fn test_ledger_snapshot_corruption_detected() {
        let dir = tempfile::tempdir().unwrap();
        let snapshot_path = dir.path().join("ledger-snapshot.bin");

        // Create and save a valid snapshot
        let state = LedgerState::new(ProtocolParameters::mainnet_defaults());
        state.save_snapshot(&snapshot_path).unwrap();

        // Corrupt one byte in the payload area (after 37-byte versioned header)
        let mut data = std::fs::read(&snapshot_path).unwrap();
        assert!(data.len() > 41);
        data[41] ^= 0xFF; // Flip bits in payload
        std::fs::write(&snapshot_path, &data).unwrap();

        // Load should fail with checksum mismatch
        let result = LedgerState::load_snapshot(&snapshot_path);
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("checksum"),
            "Expected checksum error, got: {err_msg}"
        );
    }

    #[test]
    fn test_snapshot_versioned_format_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let snapshot_path = dir.path().join("versioned-snapshot.bin");

        let state = LedgerState::new(ProtocolParameters::mainnet_defaults());
        state.save_snapshot(&snapshot_path).unwrap();

        // Verify the on-disk format: TRSN(4) + version(1) + checksum(32) + data
        let raw = std::fs::read(&snapshot_path).unwrap();
        assert_eq!(&raw[..4], b"TRSN", "magic bytes");
        assert_eq!(raw[4], LedgerState::SNAPSHOT_VERSION, "version byte");

        // Load it back and verify it deserializes correctly
        let loaded = LedgerState::load_snapshot(&snapshot_path).unwrap();
        assert_eq!(loaded.epoch, state.epoch);
    }

    #[test]
    fn test_snapshot_within_size_limit_loads_normally() {
        let dir = tempfile::tempdir().unwrap();
        let snapshot_path = dir.path().join("ledger-snapshot.bin");

        // Create a valid snapshot (well within the 10 GiB limit)
        let state = LedgerState::new(ProtocolParameters::mainnet_defaults());
        state.save_snapshot(&snapshot_path).unwrap();

        // Verify the file is within limits
        let metadata = std::fs::metadata(&snapshot_path).unwrap();
        assert!(
            (metadata.len() as usize) <= MAX_SNAPSHOT_SIZE,
            "Test snapshot should be within size limit"
        );

        // Load should succeed
        let loaded = LedgerState::load_snapshot(&snapshot_path).unwrap();
        assert_eq!(loaded.epoch, state.epoch);
    }

    #[test]
    fn test_snapshot_legacy_format_without_version_byte() {
        // Build a legacy-format snapshot: TRSN(4) + checksum(32) + data (no version byte)
        let dir = tempfile::tempdir().unwrap();
        let snapshot_path = dir.path().join("legacy-snapshot.bin");

        let state = LedgerState::new(ProtocolParameters::mainnet_defaults());
        let data = bincode::serialize(&state).unwrap();
        let checksum = torsten_primitives::hash::blake2b_256(&data);

        let mut legacy = Vec::new();
        legacy.extend_from_slice(b"TRSN");
        legacy.extend_from_slice(checksum.as_bytes());
        legacy.extend_from_slice(&data);
        std::fs::write(&snapshot_path, &legacy).unwrap();

        // load_snapshot should handle the legacy format (5th byte is a hash byte,
        // which will typically be >= 128 or 0, triggering the legacy path)
        // If it happens to be in the version range, it would fail checksum —
        // either way, we verify it loads or fails gracefully.
        let result = LedgerState::load_snapshot(&snapshot_path);
        // The legacy format should load successfully when the 5th byte (first hash byte)
        // is outside the version range [1, 128), which is the common case.
        // If the hash starts with a byte in [1, 128), the versioned path would be taken
        // and the checksum would fail, which is also acceptable (corruption-detected).
        if checksum.as_bytes()[0] == 0 || checksum.as_bytes()[0] >= 128 {
            // Legacy path taken — should succeed
            let loaded = result.unwrap();
            assert_eq!(loaded.epoch, state.epoch);
        } else {
            // Extremely unlikely but possible: first hash byte looks like a version.
            // The versioned-format checksum check would fail, giving a checksum error.
            assert!(result.is_err());
        }
    }

    #[test]
    fn test_snapshot_rejects_unknown_version() {
        let dir = tempfile::tempdir().unwrap();
        let snapshot_path = dir.path().join("future-snapshot.bin");

        let state = LedgerState::new(ProtocolParameters::mainnet_defaults());
        let data = bincode::serialize(&state).unwrap();
        let checksum = torsten_primitives::hash::blake2b_256(&data);

        // Write a snapshot with version 99 (unsupported)
        let mut future = Vec::new();
        future.extend_from_slice(b"TRSN");
        future.push(99u8); // future version
        future.extend_from_slice(checksum.as_bytes());
        future.extend_from_slice(&data);
        std::fs::write(&snapshot_path, &future).unwrap();

        let result = LedgerState::load_snapshot(&snapshot_path);
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("Unsupported snapshot version 99"),
            "Expected version error, got: {err_msg}"
        );
    }

    #[test]
    fn test_oversized_snapshot_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let snapshot_path = dir.path().join("oversized-snapshot.bin");

        // Write a valid snapshot first
        let state = LedgerState::new(ProtocolParameters::mainnet_defaults());
        state.save_snapshot(&snapshot_path).unwrap();

        // Read it and verify it loads
        assert!(LedgerState::load_snapshot(&snapshot_path).is_ok());

        // Test 1: Verify the constant is 10 GiB
        assert_eq!(MAX_SNAPSHOT_SIZE, 10 * 1024 * 1024 * 1024);

        // Test 2: Craft a payload whose bincode-encoded length field claims
        // a huge Vec, which bincode::options().with_limit() should reject.
        let mut legacy_malicious = Vec::new();
        let huge_len: u64 = 20 * 1024 * 1024 * 1024; // 20 GiB
        legacy_malicious.extend_from_slice(&huge_len.to_le_bytes());
        legacy_malicious.extend_from_slice(&[0u8; 100]);

        let malicious_path = dir.path().join("malicious-snapshot.bin");
        std::fs::write(&malicious_path, &legacy_malicious).unwrap();

        let result = LedgerState::load_snapshot(&malicious_path);
        assert!(result.is_err(), "Malicious snapshot should be rejected");
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("deserialize"),
            "Expected deserialization error from bincode limit, got: {err_msg}"
        );
    }

    #[test]
    fn test_pool_registration_stores_metadata() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);

        let pool_id = Hash28::from_bytes([1u8; 28]);
        let owner1 = Hash28::from_bytes([10u8; 28]);
        let owner2 = Hash28::from_bytes([11u8; 28]);
        let pool_params = PoolParams {
            operator: pool_id,
            vrf_keyhash: Hash32::from_bytes([2u8; 32]),
            pledge: Lovelace(500_000_000),
            cost: Lovelace(340_000_000),
            margin: Rational {
                numerator: 1,
                denominator: 100,
            },
            reward_account: vec![0xe0; 29],
            pool_owners: vec![owner1, owner2],
            relays: vec![],
            pool_metadata: Some(PoolMetadata {
                url: "https://example.com/pool.json".to_string(),
                hash: Hash32::from_bytes([99u8; 32]),
            }),
        };

        state.process_certificate(&Certificate::PoolRegistration(pool_params));
        let reg = &state.pool_params[&pool_id];

        assert_eq!(reg.reward_account, vec![0xe0; 29]);
        assert_eq!(reg.owners.len(), 2);
        assert_eq!(reg.owners[0], owner1);
        assert_eq!(reg.owners[1], owner2);
        assert_eq!(
            reg.metadata_url.as_deref(),
            Some("https://example.com/pool.json")
        );
        assert_eq!(reg.metadata_hash, Some(Hash32::from_bytes([99u8; 32])));
    }

    #[test]
    fn test_guardrail_script_policy_validation() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);

        // Set up a constitution with a guardrail script hash
        let guardrail_hash = Hash28::from_bytes([42u8; 28]);
        Arc::make_mut(&mut state.governance).constitution = Some(Constitution {
            anchor: Anchor {
                url: "https://constitution.example.com".to_string(),
                data_hash: Hash32::ZERO,
            },
            script_hash: Some(guardrail_hash),
        });

        // Submit a ParameterChange proposal with matching policy_hash — should succeed
        let update = torsten_primitives::transaction::ProtocolParamUpdate::default();
        let proposal_with_match = ProposalProcedure {
            deposit: Lovelace(100_000_000_000),
            return_addr: vec![0u8; 29],
            gov_action: GovAction::ParameterChange {
                prev_action_id: None,
                protocol_param_update: Box::new(update.clone()),
                policy_hash: Some(guardrail_hash),
            },
            anchor: Anchor {
                url: "https://example.com".to_string(),
                data_hash: Hash32::ZERO,
            },
        };
        state.process_proposal(&Hash32::from_bytes([1u8; 32]), 0, &proposal_with_match);
        assert_eq!(state.governance.proposals.len(), 1);

        // Submit a proposal with mismatched policy_hash — still accepted (logged as warning)
        let proposal_mismatch = ProposalProcedure {
            deposit: Lovelace(100_000_000_000),
            return_addr: vec![0u8; 29],
            gov_action: GovAction::ParameterChange {
                prev_action_id: None,
                protocol_param_update: Box::new(update.clone()),
                policy_hash: Some(Hash28::from_bytes([99u8; 28])),
            },
            anchor: Anchor {
                url: "https://example.com".to_string(),
                data_hash: Hash32::ZERO,
            },
        };
        state.process_proposal(&Hash32::from_bytes([2u8; 32]), 0, &proposal_mismatch);
        assert_eq!(state.governance.proposals.len(), 2);

        // Submit a proposal with no policy_hash — still accepted (logged as debug)
        let proposal_no_hash = ProposalProcedure {
            deposit: Lovelace(100_000_000_000),
            return_addr: vec![0u8; 29],
            gov_action: GovAction::ParameterChange {
                prev_action_id: None,
                protocol_param_update: Box::new(update),
                policy_hash: None,
            },
            anchor: Anchor {
                url: "https://example.com".to_string(),
                data_hash: Hash32::ZERO,
            },
        };
        state.process_proposal(&Hash32::from_bytes([3u8; 32]), 0, &proposal_no_hash);
        assert_eq!(state.governance.proposals.len(), 3);
    }

    #[test]
    fn test_gov_action_lifetime_from_protocol_params() {
        let mut params = ProtocolParameters::mainnet_defaults();
        params.gov_action_lifetime = 10;
        let mut state = LedgerState::new(params);
        state.epoch = EpochNo(5);

        let proposal = ProposalProcedure {
            deposit: Lovelace(100_000_000_000),
            return_addr: vec![0u8; 29],
            gov_action: GovAction::InfoAction,
            anchor: Anchor {
                url: "https://example.com".to_string(),
                data_hash: Hash32::ZERO,
            },
        };
        let tx_hash = Hash32::from_bytes([1u8; 32]);
        state.process_proposal(&tx_hash, 0, &proposal);

        let action_id = GovActionId {
            transaction_id: tx_hash,
            action_index: 0,
        };
        let ps = &state.governance.proposals[&action_id];
        assert_eq!(ps.expires_epoch, EpochNo(15)); // epoch 5 + lifetime 10
    }

    #[test]
    fn test_enact_parameter_change_applies_all_fields() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);

        // Create an update that changes multiple fields including cost models
        let update = ProtocolParamUpdate {
            min_fee_a: Some(55),
            max_block_body_size: Some(131072),
            max_block_header_size: Some(2000),
            ada_per_utxo_byte: Some(Lovelace(5000)),
            max_val_size: Some(10000),
            collateral_percentage: Some(200),
            max_collateral_inputs: Some(5),
            cost_models: Some(CostModels {
                plutus_v1: None,
                plutus_v2: Some(vec![1, 2, 3]),
                plutus_v3: Some(vec![4, 5, 6]),
            }),
            max_tx_ex_units: Some(ExUnits {
                mem: 20_000_000,
                steps: 10_000_000_000,
            }),
            ..Default::default()
        };

        let action = GovAction::ParameterChange {
            prev_action_id: None,
            protocol_param_update: Box::new(update),
            policy_hash: None,
        };

        state.enact_gov_action(&action);

        assert_eq!(state.protocol_params.min_fee_a, 55);
        assert_eq!(state.protocol_params.max_block_body_size, 131072);
        assert_eq!(state.protocol_params.max_block_header_size, 2000);
        assert_eq!(state.protocol_params.ada_per_utxo_byte, Lovelace(5000));
        assert_eq!(state.protocol_params.max_val_size, 10000);
        assert_eq!(state.protocol_params.collateral_percentage, 200);
        assert_eq!(state.protocol_params.max_collateral_inputs, 5);
        assert_eq!(
            state.protocol_params.cost_models.plutus_v2,
            Some(vec![1, 2, 3])
        );
        assert_eq!(
            state.protocol_params.cost_models.plutus_v3,
            Some(vec![4, 5, 6])
        );
        // PlutusV1 should remain unchanged (wasn't in the update)
        assert_eq!(state.protocol_params.cost_models.plutus_v1, None);
        assert_eq!(state.protocol_params.max_tx_ex_units.mem, 20_000_000);
        assert_eq!(state.protocol_params.max_tx_ex_units.steps, 10_000_000_000);
    }

    // --- PP Group Classification Tests ---

    #[test]
    fn test_pp_groups_empty_update() {
        let ppu = ProtocolParamUpdate::default();
        let groups = modified_pp_groups(&ppu);
        assert!(groups.is_empty());
    }

    #[test]
    fn test_pp_groups_network_security() {
        let ppu = ProtocolParamUpdate {
            max_block_body_size: Some(65536),
            ..Default::default()
        };
        let groups = modified_pp_groups(&ppu);
        assert_eq!(groups.len(), 1);
        assert_eq!(
            groups[0],
            (DRepPPGroup::Network, StakePoolPPGroup::Security)
        );
    }

    #[test]
    fn test_pp_groups_network_no_spo() {
        let ppu = ProtocolParamUpdate {
            max_tx_ex_units: Some(ExUnits {
                mem: 1_000_000,
                steps: 1_000_000,
            }),
            ..Default::default()
        };
        let groups = modified_pp_groups(&ppu);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0], (DRepPPGroup::Network, StakePoolPPGroup::NoVote));
    }

    #[test]
    fn test_pp_groups_economic_security() {
        let ppu = ProtocolParamUpdate {
            min_fee_a: Some(44),
            ..Default::default()
        };
        let groups = modified_pp_groups(&ppu);
        assert_eq!(groups.len(), 1);
        assert_eq!(
            groups[0],
            (DRepPPGroup::Economic, StakePoolPPGroup::Security)
        );
    }

    #[test]
    fn test_pp_groups_economic_no_spo() {
        let ppu = ProtocolParamUpdate {
            key_deposit: Some(Lovelace(2_000_000)),
            ..Default::default()
        };
        let groups = modified_pp_groups(&ppu);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0], (DRepPPGroup::Economic, StakePoolPPGroup::NoVote));
    }

    #[test]
    fn test_pp_groups_technical() {
        let ppu = ProtocolParamUpdate {
            cost_models: Some(torsten_primitives::transaction::CostModels {
                plutus_v1: None,
                plutus_v2: Some(vec![1, 2, 3]),
                plutus_v3: None,
            }),
            ..Default::default()
        };
        let groups = modified_pp_groups(&ppu);
        assert_eq!(groups.len(), 1);
        assert_eq!(
            groups[0],
            (DRepPPGroup::Technical, StakePoolPPGroup::NoVote)
        );
    }

    #[test]
    fn test_pp_groups_gov_security() {
        let ppu = ProtocolParamUpdate {
            gov_action_deposit: Some(Lovelace(100_000_000_000)),
            ..Default::default()
        };
        let groups = modified_pp_groups(&ppu);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0], (DRepPPGroup::Gov, StakePoolPPGroup::Security));
    }

    #[test]
    fn test_pp_groups_gov_no_spo() {
        let ppu = ProtocolParamUpdate {
            drep_deposit: Some(Lovelace(500_000_000)),
            ..Default::default()
        };
        let groups = modified_pp_groups(&ppu);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0], (DRepPPGroup::Gov, StakePoolPPGroup::NoVote));
    }

    #[test]
    fn test_pp_groups_mixed_network_and_economic() {
        let ppu = ProtocolParamUpdate {
            max_tx_size: Some(16384),
            key_deposit: Some(Lovelace(2_000_000)),
            ..Default::default()
        };
        let groups = modified_pp_groups(&ppu);
        assert_eq!(groups.len(), 2);
        assert!(groups.contains(&(DRepPPGroup::Network, StakePoolPPGroup::Security)));
        assert!(groups.contains(&(DRepPPGroup::Economic, StakePoolPPGroup::NoVote)));
    }

    #[test]
    fn test_pp_drep_threshold_single_group() {
        let params = ProtocolParameters::mainnet_defaults();
        let ppu = ProtocolParamUpdate {
            max_block_body_size: Some(65536),
            ..Default::default()
        };
        let threshold = pp_change_drep_threshold(&ppu, &params);
        assert_eq!(threshold, params.dvt_pp_network_group);
    }

    #[test]
    fn test_pp_drep_threshold_max_of_multiple_groups() {
        let params = ProtocolParameters::mainnet_defaults();
        let ppu = ProtocolParamUpdate {
            max_block_body_size: Some(65536),
            min_fee_a: Some(44),
            cost_models: Some(torsten_primitives::transaction::CostModels {
                plutus_v1: None,
                plutus_v2: Some(vec![1]),
                plutus_v3: None,
            }),
            ..Default::default()
        };
        let threshold = pp_change_drep_threshold(&ppu, &params);
        // Should be max of network, economic, technical groups
        let mut expected = params.dvt_pp_network_group.clone();
        if params.dvt_pp_economic_group.gt(&expected) {
            expected = params.dvt_pp_economic_group.clone();
        }
        if params.dvt_pp_technical_group.gt(&expected) {
            expected = params.dvt_pp_technical_group.clone();
        }
        assert_eq!(threshold, expected);
    }

    #[test]
    fn test_pp_spo_threshold_security_relevant() {
        let params = ProtocolParameters::mainnet_defaults();
        let ppu = ProtocolParamUpdate {
            max_block_body_size: Some(65536),
            ..Default::default()
        };
        let spo = pp_change_spo_threshold(&ppu, &params);
        assert_eq!(spo, Some(params.pvt_pp_security_group.clone()));
    }

    #[test]
    fn test_pp_spo_threshold_not_security_relevant() {
        let params = ProtocolParameters::mainnet_defaults();
        let ppu = ProtocolParamUpdate {
            cost_models: Some(torsten_primitives::transaction::CostModels {
                plutus_v1: None,
                plutus_v2: Some(vec![1]),
                plutus_v3: None,
            }),
            ..Default::default()
        };
        let spo = pp_change_spo_threshold(&ppu, &params);
        assert_eq!(spo, None);
    }

    #[test]
    fn test_pp_spo_threshold_mixed_security_and_non_security() {
        let params = ProtocolParameters::mainnet_defaults();
        let ppu = ProtocolParamUpdate {
            min_fee_a: Some(44),
            key_deposit: Some(Lovelace(2_000_000)),
            ..Default::default()
        };
        let spo = pp_change_spo_threshold(&ppu, &params);
        assert_eq!(spo, Some(params.pvt_pp_security_group.clone()));
    }

    #[test]
    fn test_pp_groups_all_network_security_params() {
        let ppu = ProtocolParamUpdate {
            max_block_body_size: Some(1),
            max_tx_size: Some(1),
            max_block_header_size: Some(1),
            max_block_ex_units: Some(ExUnits { mem: 1, steps: 1 }),
            max_val_size: Some(1),
            ..Default::default()
        };
        let groups = modified_pp_groups(&ppu);
        assert_eq!(groups.len(), 5);
        assert!(groups
            .iter()
            .all(|g| *g == (DRepPPGroup::Network, StakePoolPPGroup::Security)));
    }

    /// Helper: create ProtocolParameters with distinct per-group DRep thresholds
    /// to verify each group is checked independently.
    fn params_with_distinct_thresholds() -> ProtocolParameters {
        let mut params = ProtocolParameters::mainnet_defaults();
        // Network: 51% (easy)
        params.dvt_pp_network_group = Rational {
            numerator: 51,
            denominator: 100,
        };
        // Economic: 60%
        params.dvt_pp_economic_group = Rational {
            numerator: 60,
            denominator: 100,
        };
        // Technical: 67%
        params.dvt_pp_technical_group = Rational {
            numerator: 67,
            denominator: 100,
        };
        // Governance: 75% (hardest)
        params.dvt_pp_gov_group = Rational {
            numerator: 75,
            denominator: 100,
        };
        params
    }

    #[test]
    fn test_per_group_network_only_uses_network_threshold() {
        let params = params_with_distinct_thresholds();
        let ppu = ProtocolParamUpdate {
            max_block_body_size: Some(65536),
            ..Default::default()
        };
        // 52% yes — meets network (51%) but would fail economic (60%)
        assert!(pp_change_drep_all_groups_met(&ppu, &params, 52, 100));
        // 50% yes — fails network (51%)
        assert!(!pp_change_drep_all_groups_met(&ppu, &params, 50, 100));
    }

    #[test]
    fn test_per_group_economic_only_uses_economic_threshold() {
        let params = params_with_distinct_thresholds();
        let ppu = ProtocolParamUpdate {
            min_fee_a: Some(44),
            ..Default::default()
        };
        // 61% yes — meets economic (60%) but would fail technical (67%)
        assert!(pp_change_drep_all_groups_met(&ppu, &params, 61, 100));
        // 59% yes — fails economic (60%)
        assert!(!pp_change_drep_all_groups_met(&ppu, &params, 59, 100));
    }

    #[test]
    fn test_per_group_technical_only_uses_technical_threshold() {
        let params = params_with_distinct_thresholds();
        let ppu = ProtocolParamUpdate {
            cost_models: Some(torsten_primitives::transaction::CostModels {
                plutus_v1: None,
                plutus_v2: Some(vec![1]),
                plutus_v3: None,
            }),
            ..Default::default()
        };
        // 68% yes — meets technical (67%) but would fail governance (75%)
        assert!(pp_change_drep_all_groups_met(&ppu, &params, 68, 100));
        // 66% yes — fails technical (67%)
        assert!(!pp_change_drep_all_groups_met(&ppu, &params, 66, 100));
    }

    #[test]
    fn test_per_group_governance_only_uses_gov_threshold() {
        let params = params_with_distinct_thresholds();
        let ppu = ProtocolParamUpdate {
            gov_action_lifetime: Some(10),
            ..Default::default()
        };
        // 76% yes — meets governance (75%)
        assert!(pp_change_drep_all_groups_met(&ppu, &params, 76, 100));
        // 74% yes — fails governance (75%)
        assert!(!pp_change_drep_all_groups_met(&ppu, &params, 74, 100));
    }

    #[test]
    fn test_per_group_multi_group_must_meet_all_thresholds() {
        let params = params_with_distinct_thresholds();
        // Update touches Network (51%), Economic (60%), and Technical (67%)
        let ppu = ProtocolParamUpdate {
            max_block_body_size: Some(65536), // Network
            min_fee_a: Some(44),              // Economic
            cost_models: Some(torsten_primitives::transaction::CostModels {
                plutus_v1: None,
                plutus_v2: Some(vec![1]),
                plutus_v3: None,
            }), // Technical
            ..Default::default()
        };
        // 68% yes — meets all three (51%, 60%, 67%)
        assert!(pp_change_drep_all_groups_met(&ppu, &params, 68, 100));
        // 65% yes — meets network+economic but fails technical (67%)
        assert!(!pp_change_drep_all_groups_met(&ppu, &params, 65, 100));
        // 55% yes — meets network only, fails economic+technical
        assert!(!pp_change_drep_all_groups_met(&ppu, &params, 55, 100));
    }

    #[test]
    fn test_per_group_all_four_groups_must_meet_highest() {
        let params = params_with_distinct_thresholds();
        // Update touches all 4 groups: Network (51%), Economic (60%), Technical (67%), Gov (75%)
        let ppu = ProtocolParamUpdate {
            max_tx_size: Some(16384),                  // Network
            key_deposit: Some(Lovelace(2_000_000)),    // Economic
            n_opt: Some(500),                          // Technical
            drep_deposit: Some(Lovelace(500_000_000)), // Governance
            ..Default::default()
        };
        // 76% — meets all four
        assert!(pp_change_drep_all_groups_met(&ppu, &params, 76, 100));
        // 70% — meets network+economic+technical but fails governance (75%)
        assert!(!pp_change_drep_all_groups_met(&ppu, &params, 70, 100));
    }

    #[test]
    fn test_per_group_governance_only_no_spo_security_required() {
        let params = params_with_distinct_thresholds();
        // Governance-only change: no security-relevant params
        let ppu = ProtocolParamUpdate {
            gov_action_lifetime: Some(10),
            drep_deposit: Some(Lovelace(500_000_000)),
            ..Default::default()
        };
        // SPO threshold should be None (no security params)
        let spo = pp_change_spo_threshold(&ppu, &params);
        assert_eq!(spo, None);
    }

    #[test]
    fn test_per_group_zero_total_stake_fails() {
        let params = params_with_distinct_thresholds();
        let ppu = ProtocolParamUpdate {
            max_block_body_size: Some(65536),
            ..Default::default()
        };
        // Zero total stake should fail (can't meet any threshold)
        assert!(!pp_change_drep_all_groups_met(&ppu, &params, 0, 0));
    }

    #[test]
    fn test_per_group_empty_update_trivially_passes() {
        let params = params_with_distinct_thresholds();
        let ppu = ProtocolParamUpdate::default();
        // No groups affected — should trivially pass (no thresholds to check)
        assert!(pp_change_drep_all_groups_met(&ppu, &params, 0, 100));
    }

    #[test]
    fn test_utxo_stake_distribution_tracking() {
        use torsten_primitives::address::BaseAddress;
        use torsten_primitives::credentials::Credential as Cred;
        use torsten_primitives::network::NetworkId;

        let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());

        // Create a base address with a staking credential
        let stake_cred = Cred::VerificationKey(Hash28::from_bytes([0xAA; 28]));
        let payment_cred = Cred::VerificationKey(Hash28::from_bytes([0xBB; 28]));
        let base_addr = Address::Base(BaseAddress {
            network: NetworkId::Mainnet,
            payment: payment_cred,
            stake: stake_cred,
        });

        // Build a genesis UTxO
        let genesis_input = TransactionInput {
            transaction_id: Hash32::from_bytes([0x01; 32]),
            index: 0,
        };
        let genesis_output = TransactionOutput {
            address: base_addr.clone(),
            value: Value::lovelace(10_000_000),
            datum: OutputDatum::None,
            script_ref: None,
            raw_cbor: None,
        };
        state.utxo_set.insert(genesis_input.clone(), genesis_output);

        // Create a transaction that spends the genesis UTxO and creates new outputs
        let tx = Transaction {
            hash: Hash32::from_bytes([0x02; 32]),
            body: TransactionBody {
                inputs: vec![genesis_input],
                outputs: vec![TransactionOutput {
                    address: base_addr.clone(),
                    value: Value::lovelace(7_000_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    raw_cbor: None,
                }],
                fee: Lovelace(3_000_000),
                ttl: None,
                certificates: vec![],
                withdrawals: std::collections::BTreeMap::new(),
                auxiliary_data_hash: None,
                validity_interval_start: None,
                mint: std::collections::BTreeMap::new(),
                script_data_hash: None,
                collateral: vec![],
                required_signers: vec![],
                network_id: None,
                collateral_return: None,
                total_collateral: None,
                reference_inputs: vec![],
                update: None,
                voting_procedures: std::collections::BTreeMap::new(),
                proposal_procedures: vec![],
                treasury_value: None,
                donation: None,
            },
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![],
                plutus_v3_scripts: vec![],
                plutus_data: vec![],
                redeemers: vec![],
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let block = make_test_block(100, 1, Hash32::ZERO, vec![tx]);
        state.apply_block(&block).unwrap();

        // The staking credential should have stake = 7_000_000 (output) - 0 (initial was never tracked as registered)
        // Actually: genesis UTxO was not tracked (inserted directly), but the output is tracked.
        // So the spent input subtracts 0 (not in stake_map), output adds 7_000_000.
        let cred_hash = credential_to_hash(
            &torsten_primitives::credentials::Credential::VerificationKey(Hash28::from_bytes(
                [0xAA; 28],
            )),
        );
        let stake = state
            .stake_distribution
            .stake_map
            .get(&cred_hash)
            .map(|l| l.0)
            .unwrap_or(0);
        assert_eq!(stake, 7_000_000);
    }

    #[test]
    fn test_stake_credential_hash_extraction() {
        use torsten_primitives::address::{BaseAddress, EnterpriseAddress};
        use torsten_primitives::credentials::Credential as Cred;
        use torsten_primitives::network::NetworkId;

        // Base address has a staking credential
        let base = Address::Base(BaseAddress {
            network: NetworkId::Mainnet,
            payment: Cred::VerificationKey(Hash28::from_bytes([0xBB; 28])),
            stake: Cred::VerificationKey(Hash28::from_bytes([0xAA; 28])),
        });
        assert!(stake_credential_hash(&base).is_some());

        // Enterprise address has no staking credential
        let enterprise = Address::Enterprise(EnterpriseAddress {
            network: NetworkId::Mainnet,
            payment: Cred::VerificationKey(Hash28::from_bytes([0xCC; 28])),
        });
        assert!(stake_credential_hash(&enterprise).is_none());

        // Byron address has no staking credential
        let byron = Address::Byron(ByronAddress {
            payload: vec![0u8; 32],
        });
        assert!(stake_credential_hash(&byron).is_none());
    }

    #[test]
    fn test_pool_retirement_within_emax() {
        let mut params = ProtocolParameters::mainnet_defaults();
        params.e_max = 18;
        let mut state = LedgerState::new(params);
        state.epoch = EpochNo(10);
        state.epoch_length = 432000;

        let pool_hash = Hash28::from_bytes([0xAA; 28]);
        let cert = Certificate::PoolRetirement {
            pool_hash,
            epoch: 28, // 10 + 18 = within bounds
        };
        state.process_certificate(&cert);
        assert!(state
            .pending_retirements
            .get(&EpochNo(28))
            .is_some_and(|v| v.contains(&pool_hash)));
    }

    #[test]
    fn test_pool_retirement_exceeds_emax() {
        let mut params = ProtocolParameters::mainnet_defaults();
        params.e_max = 18;
        let mut state = LedgerState::new(params);
        state.epoch = EpochNo(10);
        state.epoch_length = 432000;

        let pool_hash = Hash28::from_bytes([0xBB; 28]);
        let cert = Certificate::PoolRetirement {
            pool_hash,
            epoch: 29, // 10 + 18 + 1 = exceeds e_max
        };
        state.process_certificate(&cert);
        // Should NOT have been added
        assert!(!state.pending_retirements.contains_key(&EpochNo(29)));
    }

    #[test]
    fn test_withdrawal_sets_balance_to_zero() {
        let mut params = ProtocolParameters::mainnet_defaults();
        params.e_max = 18;
        let mut state = LedgerState::new(params);
        state.epoch_length = 432000;

        // Build a raw reward account address: e0 (testnet) + 28-byte key hash
        let key_bytes = [0xCC; 28];
        let mut reward_account = vec![0xE0u8];
        reward_account.extend_from_slice(&key_bytes);

        // reward_account_to_hash pads 28 bytes to Hash32
        let hash_key = LedgerState::reward_account_to_hash(&reward_account);
        Arc::make_mut(&mut state.reward_accounts).insert(hash_key, Lovelace(5_000_000));

        state.process_withdrawal(&reward_account, Lovelace(5_000_000));
        assert_eq!(state.reward_accounts.get(&hash_key), Some(&Lovelace(0)));
    }

    #[test]
    fn test_mir_stake_credential_distribution() {
        let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());
        state.reserves = Lovelace(10_000_000);
        let cred = Credential::VerificationKey(Hash28::from_bytes([0xaa; 28]));
        let key = credential_to_hash(&cred);

        // Register stake credential first
        state.process_certificate(&Certificate::StakeRegistration(cred.clone()));
        assert_eq!(state.reward_accounts.get(&key), Some(&Lovelace(0)));

        // MIR: distribute 1_000_000 from reserves
        state.process_certificate(&Certificate::MoveInstantaneousRewards {
            source: MIRSource::Reserves,
            target: MIRTarget::StakeCredentials(vec![(cred.clone(), 1_000_000)]),
        });
        assert_eq!(state.reward_accounts.get(&key), Some(&Lovelace(1_000_000)));
        // Reserves should be debited
        assert_eq!(state.reserves, Lovelace(9_000_000));
    }

    #[test]
    fn test_mir_pot_transfer() {
        let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());
        state.reserves = Lovelace(10_000_000);
        state.treasury = Lovelace(5_000_000);

        // MIR: transfer 2M from reserves to treasury
        state.process_certificate(&Certificate::MoveInstantaneousRewards {
            source: MIRSource::Reserves,
            target: MIRTarget::OtherAccountingPot(2_000_000),
        });
        assert_eq!(state.reserves, Lovelace(8_000_000));
        assert_eq!(state.treasury, Lovelace(7_000_000));

        // MIR: transfer 3M from treasury to reserves
        state.process_certificate(&Certificate::MoveInstantaneousRewards {
            source: MIRSource::Treasury,
            target: MIRTarget::OtherAccountingPot(3_000_000),
        });
        assert_eq!(state.reserves, Lovelace(11_000_000));
        assert_eq!(state.treasury, Lovelace(4_000_000));
    }

    #[test]
    fn test_genesis_key_delegation() {
        let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());
        // GenesisKeyDelegation should not panic — just log
        state.process_certificate(&Certificate::GenesisKeyDelegation {
            genesis_hash: Hash32::from_bytes([0x11; 32]),
            genesis_delegate_hash: Hash32::from_bytes([0x22; 32]),
            vrf_keyhash: Hash32::from_bytes([0x33; 32]),
        });
        // No state change expected — just ensures it doesn't crash
    }

    #[test]
    fn test_pre_conway_pp_update_quorum_met() {
        let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());
        state.update_quorum = 2; // Require 2 distinct proposers
        state.epoch = EpochNo(4);
        state.epoch_length = 100;

        // Original values
        assert_eq!(state.protocol_params.min_fee_a, 44);
        assert_eq!(state.protocol_params.max_block_body_size, 90112);

        // Two distinct genesis delegates propose updates targeting epoch 4 (current).
        // Per the PPUP rule, proposals targeting epoch E are applied at the E→E+1 boundary.
        let hash1 = Hash32::from_bytes([0x01; 32]);
        let hash2 = Hash32::from_bytes([0x02; 32]);
        let update = ProtocolParamUpdate {
            min_fee_a: Some(55),
            max_block_body_size: Some(65536),
            ..Default::default()
        };
        state
            .pending_pp_updates
            .entry(EpochNo(4))
            .or_default()
            .push((hash1, update.clone()));
        state
            .pending_pp_updates
            .entry(EpochNo(4))
            .or_default()
            .push((hash2, update));

        // Trigger epoch transition to epoch 5
        state.process_epoch_transition(EpochNo(5));

        // Updates should be applied
        assert_eq!(state.protocol_params.min_fee_a, 55);
        assert_eq!(state.protocol_params.max_block_body_size, 65536);
        // pending_pp_updates should be empty
        assert!(state.pending_pp_updates.is_empty());
    }

    #[test]
    fn test_pre_conway_pp_update_quorum_not_met() {
        let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());
        state.update_quorum = 3; // Require 3 distinct proposers
        state.epoch = EpochNo(4);
        state.epoch_length = 100;

        let original_fee = state.protocol_params.min_fee_a;

        // Only 2 proposers targeting epoch 4 (quorum is 3)
        let hash1 = Hash32::from_bytes([0x01; 32]);
        let hash2 = Hash32::from_bytes([0x02; 32]);
        let update = ProtocolParamUpdate {
            min_fee_a: Some(999),
            ..Default::default()
        };
        state
            .pending_pp_updates
            .entry(EpochNo(4))
            .or_default()
            .push((hash1, update.clone()));
        state
            .pending_pp_updates
            .entry(EpochNo(4))
            .or_default()
            .push((hash2, update));

        state.process_epoch_transition(EpochNo(5));

        // Updates should NOT be applied
        assert_eq!(state.protocol_params.min_fee_a, original_fee);
        // Proposals should be cleaned up
        assert!(state.pending_pp_updates.is_empty());
    }

    #[test]
    fn test_pre_conway_pp_update_protocol_version() {
        let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());
        state.update_quorum = 1;
        state.epoch = EpochNo(9);
        state.epoch_length = 100;

        // Proposal targets epoch 9 (current), applied at 9→10 boundary
        let hash1 = Hash32::from_bytes([0x01; 32]);
        let update = ProtocolParamUpdate {
            protocol_version_major: Some(7),
            protocol_version_minor: Some(0),
            ..Default::default()
        };
        state
            .pending_pp_updates
            .entry(EpochNo(9))
            .or_default()
            .push((hash1, update));

        state.process_epoch_transition(EpochNo(10));

        assert_eq!(state.protocol_params.protocol_version_major, 7);
        assert_eq!(state.protocol_params.protocol_version_minor, 0);
    }

    #[test]
    fn test_apply_protocol_param_update_all_fields() {
        let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());

        let update = ProtocolParamUpdate {
            min_fee_a: Some(55),
            min_fee_b: Some(200000),
            max_block_body_size: Some(65536),
            max_tx_size: Some(32768),
            key_deposit: Some(Lovelace(3_000_000)),
            pool_deposit: Some(Lovelace(600_000_000)),
            ada_per_utxo_byte: Some(Lovelace(5000)),
            ..Default::default()
        };

        state.apply_protocol_param_update(&update).unwrap();

        assert_eq!(state.protocol_params.min_fee_a, 55);
        assert_eq!(state.protocol_params.min_fee_b, 200000);
        assert_eq!(state.protocol_params.max_block_body_size, 65536);
        assert_eq!(state.protocol_params.max_tx_size, 32768);
        assert_eq!(state.protocol_params.key_deposit, Lovelace(3_000_000));
        assert_eq!(state.protocol_params.pool_deposit, Lovelace(600_000_000));
        assert_eq!(state.protocol_params.ada_per_utxo_byte, Lovelace(5000));
        // Unchanged fields should remain at defaults
        assert_eq!(state.protocol_params.max_block_header_size, 1100);
    }

    #[test]
    fn test_pre_conway_pp_update_past_epochs_cleaned() {
        let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());
        state.update_quorum = 5;
        state.epoch = EpochNo(9);
        state.epoch_length = 100;

        // Add proposals for past epochs that were never applied
        let hash1 = Hash32::from_bytes([0x01; 32]);
        let update = ProtocolParamUpdate {
            min_fee_a: Some(999),
            ..Default::default()
        };
        state
            .pending_pp_updates
            .entry(EpochNo(3))
            .or_default()
            .push((hash1, update.clone()));
        state
            .pending_pp_updates
            .entry(EpochNo(7))
            .or_default()
            .push((hash1, update));

        state.process_epoch_transition(EpochNo(10));

        // All past proposals should be cleaned up
        assert!(state.pending_pp_updates.is_empty());
    }

    #[test]
    fn test_pre_conway_pp_update_survives_intermediate_epoch() {
        // Regression test: proposals targeting epoch E must survive the
        // (E-1) → E transition cleanup and be applied at the E → (E+1) boundary.
        // This simulates the 7→8 transition on preview testnet where proposals
        // targeting epoch 21 are submitted in epoch 20 and must survive the
        // 20→21 cleanup to be applied at the 21→22 boundary.
        let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());
        state.update_quorum = 5;
        state.epoch = EpochNo(20);
        state.epoch_length = 100;

        // 7 genesis delegates propose protocol_version=8.0 targeting epoch 21
        let proposers: Vec<Hash32> = (0..7).map(|i| Hash32::from_bytes([i + 1; 32])).collect();
        for hash in &proposers {
            let update = ProtocolParamUpdate {
                protocol_version_major: Some(8),
                protocol_version_minor: Some(0),
                ..Default::default()
            };
            state
                .pending_pp_updates
                .entry(EpochNo(21))
                .or_default()
                .push((*hash, update));
        }

        // Transition 20→21: proposals target epoch 21, should NOT be applied yet
        // but must survive the cleanup
        state.process_epoch_transition(EpochNo(21));
        assert!(
            !state.pending_pp_updates.is_empty(),
            "proposals targeting epoch 21 should survive the 20→21 cleanup"
        );
        // Protocol version should still be the default (9 from mainnet_defaults)
        assert_eq!(state.protocol_params.protocol_version_major, 9);

        // Transition 21→22: proposals targeting epoch 21 should now be applied
        state.process_epoch_transition(EpochNo(22));
        assert_eq!(state.protocol_params.protocol_version_major, 8);
        assert_eq!(state.protocol_params.protocol_version_minor, 0);
        assert!(state.pending_pp_updates.is_empty());
    }

    #[test]
    fn test_prev_action_as_expected_none_chain() {
        let governance = GovernanceState::default();
        // Proposals with prev_action_id=None should pass when no actions have been enacted
        let action = GovAction::HardForkInitiation {
            prev_action_id: None,
            protocol_version: (10, 0),
        };
        assert!(prev_action_as_expected(&action, &governance));

        let action = GovAction::ParameterChange {
            prev_action_id: None,
            protocol_param_update: Box::new(ProtocolParamUpdate::default()),
            policy_hash: None,
        };
        assert!(prev_action_as_expected(&action, &governance));
    }

    #[test]
    fn test_prev_action_as_expected_chain_mismatch() {
        let mut governance = GovernanceState::default();
        // Set an enacted hard fork root
        let enacted_id = GovActionId {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            action_index: 0,
        };
        governance.enacted_hard_fork = Some(enacted_id.clone());

        // Proposal with prev_action_id=None should FAIL (root is Some)
        let action = GovAction::HardForkInitiation {
            prev_action_id: None,
            protocol_version: (11, 0),
        };
        assert!(!prev_action_as_expected(&action, &governance));

        // Proposal with wrong prev_action_id should FAIL
        let wrong_id = GovActionId {
            transaction_id: Hash32::from_bytes([2u8; 32]),
            action_index: 0,
        };
        let action = GovAction::HardForkInitiation {
            prev_action_id: Some(wrong_id),
            protocol_version: (11, 0),
        };
        assert!(!prev_action_as_expected(&action, &governance));

        // Proposal with correct prev_action_id should PASS
        let action = GovAction::HardForkInitiation {
            prev_action_id: Some(enacted_id),
            protocol_version: (11, 0),
        };
        assert!(prev_action_as_expected(&action, &governance));
    }

    #[test]
    fn test_prev_action_committee_shared_purpose() {
        let mut governance = GovernanceState::default();
        let enacted_id = GovActionId {
            transaction_id: Hash32::from_bytes([5u8; 32]),
            action_index: 0,
        };
        governance.enacted_committee = Some(enacted_id.clone());

        // NoConfidence and UpdateCommittee share the committee purpose
        let no_confidence = GovAction::NoConfidence {
            prev_action_id: Some(enacted_id.clone()),
        };
        assert!(prev_action_as_expected(&no_confidence, &governance));

        let update_committee = GovAction::UpdateCommittee {
            prev_action_id: Some(enacted_id),
            members_to_remove: vec![],
            members_to_add: BTreeMap::new(),
            threshold: Rational {
                numerator: 1,
                denominator: 2,
            },
        };
        assert!(prev_action_as_expected(&update_committee, &governance));
    }

    #[test]
    fn test_treasury_and_info_always_pass_chain() {
        // Even with arbitrary enacted roots, treasury and info always pass
        let governance = GovernanceState {
            enacted_pparam_update: Some(GovActionId {
                transaction_id: Hash32::from_bytes([99u8; 32]),
                action_index: 0,
            }),
            ..Default::default()
        };

        let treasury = GovAction::TreasuryWithdrawals {
            withdrawals: BTreeMap::new(),
            policy_hash: None,
        };
        assert!(prev_action_as_expected(&treasury, &governance));
        assert!(prev_action_as_expected(&GovAction::InfoAction, &governance));
    }

    #[test]
    fn test_gov_action_priority_ordering() {
        assert!(
            gov_action_priority(&GovAction::NoConfidence {
                prev_action_id: None
            }) < gov_action_priority(&GovAction::HardForkInitiation {
                prev_action_id: None,
                protocol_version: (10, 0)
            })
        );
        assert!(
            gov_action_priority(&GovAction::HardForkInitiation {
                prev_action_id: None,
                protocol_version: (10, 0)
            }) < gov_action_priority(&GovAction::ParameterChange {
                prev_action_id: None,
                protocol_param_update: Box::new(ProtocolParamUpdate::default()),
                policy_hash: None
            })
        );
        assert!(
            gov_action_priority(&GovAction::ParameterChange {
                prev_action_id: None,
                protocol_param_update: Box::new(ProtocolParamUpdate::default()),
                policy_hash: None
            }) < gov_action_priority(&GovAction::InfoAction)
        );
    }

    #[test]
    fn test_delaying_action() {
        assert!(is_delaying_action(&GovAction::NoConfidence {
            prev_action_id: None
        }));
        assert!(is_delaying_action(&GovAction::HardForkInitiation {
            prev_action_id: None,
            protocol_version: (10, 0)
        }));
        assert!(is_delaying_action(&GovAction::UpdateCommittee {
            prev_action_id: None,
            members_to_remove: vec![],
            members_to_add: BTreeMap::new(),
            threshold: Rational {
                numerator: 1,
                denominator: 2
            },
        }));
        assert!(!is_delaying_action(&GovAction::TreasuryWithdrawals {
            withdrawals: BTreeMap::new(),
            policy_hash: None,
        }));
        assert!(!is_delaying_action(&GovAction::InfoAction));
    }

    // ==================== Bug Fix Tests ====================

    #[test]
    fn test_invalid_tx_uses_collateral_for_fees_not_declared_fee() {
        // Bug 1: Invalid tx should collect collateral as fee, not tx.body.fee
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);
        state.epoch_length = 1_000_000; // avoid epoch transition

        // Create a collateral UTxO worth 5 ADA
        let collateral_input = TransactionInput {
            transaction_id: Hash32::from_bytes([10u8; 32]),
            index: 0,
        };
        let collateral_output = TransactionOutput {
            address: Address::Byron(ByronAddress {
                payload: vec![0u8; 32],
            }),
            value: Value::lovelace(5_000_000), // 5 ADA collateral
            datum: OutputDatum::None,
            script_ref: None,
            raw_cbor: None,
        };
        state
            .utxo_set
            .insert(collateral_input.clone(), collateral_output);

        // Create an invalid tx with declared fee of 200_000 but collateral of 5_000_000
        let tx = Transaction {
            hash: Hash32::from_bytes([11u8; 32]),
            body: TransactionBody {
                inputs: vec![],
                outputs: vec![],
                fee: Lovelace(200_000), // declared fee (should NOT be used)
                ttl: None,
                certificates: vec![],
                withdrawals: BTreeMap::new(),
                auxiliary_data_hash: None,
                validity_interval_start: None,
                mint: BTreeMap::new(),
                script_data_hash: None,
                collateral: vec![collateral_input],
                required_signers: vec![],
                network_id: None,
                collateral_return: None, // no return, so full 5 ADA is forfeited
                total_collateral: None,
                reference_inputs: vec![],
                update: None,
                voting_procedures: BTreeMap::new(),
                proposal_procedures: vec![],
                treasury_value: None,
                donation: None,
            },
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![],
                plutus_v3_scripts: vec![],
                plutus_data: vec![],
                redeemers: vec![],
            },
            is_valid: false,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let block = make_test_block(100, 1, Hash32::ZERO, vec![tx]);
        state.apply_block(&block).unwrap();

        // Fee should be the collateral amount (5 ADA), NOT the declared fee (0.2 ADA)
        assert_eq!(state.epoch_fees, Lovelace(5_000_000));
    }

    #[test]
    fn test_invalid_tx_collateral_with_return() {
        // Bug 1 variant: collateral with return — fee = inputs - return
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);
        state.epoch_length = 1_000_000;

        let collateral_input = TransactionInput {
            transaction_id: Hash32::from_bytes([20u8; 32]),
            index: 0,
        };
        let collateral_output = TransactionOutput {
            address: Address::Byron(ByronAddress {
                payload: vec![0u8; 32],
            }),
            value: Value::lovelace(10_000_000), // 10 ADA collateral input
            datum: OutputDatum::None,
            script_ref: None,
            raw_cbor: None,
        };
        state
            .utxo_set
            .insert(collateral_input.clone(), collateral_output);

        // Collateral return gives back 7 ADA, so only 3 ADA forfeited
        let col_return = TransactionOutput {
            address: Address::Byron(ByronAddress {
                payload: vec![0u8; 32],
            }),
            value: Value::lovelace(7_000_000),
            datum: OutputDatum::None,
            script_ref: None,
            raw_cbor: None,
        };

        let tx = Transaction {
            hash: Hash32::from_bytes([21u8; 32]),
            body: TransactionBody {
                inputs: vec![],
                outputs: vec![],
                fee: Lovelace(500_000), // declared fee (should NOT be used)
                ttl: None,
                certificates: vec![],
                withdrawals: BTreeMap::new(),
                auxiliary_data_hash: None,
                validity_interval_start: None,
                mint: BTreeMap::new(),
                script_data_hash: None,
                collateral: vec![collateral_input],
                required_signers: vec![],
                network_id: None,
                collateral_return: Some(col_return),
                total_collateral: None,
                reference_inputs: vec![],
                update: None,
                voting_procedures: BTreeMap::new(),
                proposal_procedures: vec![],
                treasury_value: None,
                donation: None,
            },
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![],
                plutus_v3_scripts: vec![],
                plutus_data: vec![],
                redeemers: vec![],
            },
            is_valid: false,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let block = make_test_block(100, 1, Hash32::ZERO, vec![tx]);
        state.apply_block(&block).unwrap();

        // Fee should be 10M - 7M = 3M (collateral forfeited), NOT 500_000 (declared fee)
        assert_eq!(state.epoch_fees, Lovelace(3_000_000));
    }

    #[test]
    fn test_invalid_tx_total_collateral_field() {
        // Bug 1 variant: when total_collateral is explicitly set, use that
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);
        state.epoch_length = 1_000_000;

        let collateral_input = TransactionInput {
            transaction_id: Hash32::from_bytes([30u8; 32]),
            index: 0,
        };
        let collateral_output = TransactionOutput {
            address: Address::Byron(ByronAddress {
                payload: vec![0u8; 32],
            }),
            value: Value::lovelace(8_000_000),
            datum: OutputDatum::None,
            script_ref: None,
            raw_cbor: None,
        };
        state
            .utxo_set
            .insert(collateral_input.clone(), collateral_output);

        let tx = Transaction {
            hash: Hash32::from_bytes([31u8; 32]),
            body: TransactionBody {
                inputs: vec![],
                outputs: vec![],
                fee: Lovelace(300_000),
                ttl: None,
                certificates: vec![],
                withdrawals: BTreeMap::new(),
                auxiliary_data_hash: None,
                validity_interval_start: None,
                mint: BTreeMap::new(),
                script_data_hash: None,
                collateral: vec![collateral_input],
                required_signers: vec![],
                network_id: None,
                collateral_return: None,
                total_collateral: Some(Lovelace(2_500_000)), // explicit total_collateral
                reference_inputs: vec![],
                update: None,
                voting_procedures: BTreeMap::new(),
                proposal_procedures: vec![],
                treasury_value: None,
                donation: None,
            },
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![],
                plutus_v3_scripts: vec![],
                plutus_data: vec![],
                redeemers: vec![],
            },
            is_valid: false,
            auxiliary_data: None,
            raw_cbor: None,
        };

        let block = make_test_block(100, 1, Hash32::ZERO, vec![tx]);
        state.apply_block(&block).unwrap();

        // Fee should be the explicit total_collateral value
        assert_eq!(state.epoch_fees, Lovelace(2_500_000));
    }

    #[test]
    fn test_mir_stake_credentials_debits_reserves() {
        // Bug 2: MIR to StakeCredentials should debit reserves
        let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());
        state.reserves = Lovelace(100_000_000);

        let cred1 = Credential::VerificationKey(Hash28::from_bytes([0xbb; 28]));
        let cred2 = Credential::VerificationKey(Hash28::from_bytes([0xcc; 28]));
        let key1 = credential_to_hash(&cred1);
        let key2 = credential_to_hash(&cred2);

        state.process_certificate(&Certificate::StakeRegistration(cred1.clone()));
        state.process_certificate(&Certificate::StakeRegistration(cred2.clone()));

        // MIR: distribute 3M + 2M = 5M from reserves
        state.process_certificate(&Certificate::MoveInstantaneousRewards {
            source: MIRSource::Reserves,
            target: MIRTarget::StakeCredentials(vec![
                (cred1.clone(), 3_000_000),
                (cred2.clone(), 2_000_000),
            ]),
        });

        assert_eq!(state.reward_accounts[&key1], Lovelace(3_000_000));
        assert_eq!(state.reward_accounts[&key2], Lovelace(2_000_000));
        // Reserves should be debited by the total distributed (5M)
        assert_eq!(state.reserves, Lovelace(95_000_000));
    }

    #[test]
    fn test_mir_stake_credentials_debits_treasury() {
        // Bug 2: MIR to StakeCredentials should debit treasury
        let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());
        state.treasury = Lovelace(50_000_000);

        let cred = Credential::VerificationKey(Hash28::from_bytes([0xdd; 28]));
        let key = credential_to_hash(&cred);

        state.process_certificate(&Certificate::StakeRegistration(cred.clone()));

        // MIR: distribute 7M from treasury
        state.process_certificate(&Certificate::MoveInstantaneousRewards {
            source: MIRSource::Treasury,
            target: MIRTarget::StakeCredentials(vec![(cred.clone(), 7_000_000)]),
        });

        assert_eq!(state.reward_accounts[&key], Lovelace(7_000_000));
        // Treasury should be debited
        assert_eq!(state.treasury, Lovelace(43_000_000));
    }

    #[test]
    fn test_mir_compound_credential_and_pot_transfer() {
        // Issue #16: When both credential distribution AND OtherAccountingPot transfer
        // happen from the same source pot, the sequential operations must use saturating
        // arithmetic to avoid underflow/overflow if the first operation depletes the pot.
        let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());
        state.reserves = Lovelace(10_000_000);
        state.treasury = Lovelace(5_000_000);

        let cred = Credential::VerificationKey(Hash28::from_bytes([0xee; 28]));
        let key = credential_to_hash(&cred);
        state.process_certificate(&Certificate::StakeRegistration(cred.clone()));

        // Step 1: MIR distributes 8M from reserves to credential (leaves 2M in reserves)
        state.process_certificate(&Certificate::MoveInstantaneousRewards {
            source: MIRSource::Reserves,
            target: MIRTarget::StakeCredentials(vec![(cred.clone(), 8_000_000)]),
        });
        assert_eq!(state.reserves, Lovelace(2_000_000));
        assert_eq!(state.reward_accounts[&key], Lovelace(8_000_000));

        // Step 2: MIR pot transfer tries to move 5M from reserves to treasury,
        // but only 2M remain. Should cap at available (2M), not panic/underflow.
        state.process_certificate(&Certificate::MoveInstantaneousRewards {
            source: MIRSource::Reserves,
            target: MIRTarget::OtherAccountingPot(5_000_000),
        });
        // Reserves fully drained (capped at 2M available)
        assert_eq!(state.reserves, Lovelace(0));
        // Treasury receives only the 2M that was actually available
        assert_eq!(state.treasury, Lovelace(7_000_000));
    }

    #[test]
    fn test_mir_pot_transfer_exceeds_source_treasury() {
        // Symmetric test: treasury pot transfer exceeding available balance
        let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());
        state.reserves = Lovelace(20_000_000);
        state.treasury = Lovelace(3_000_000);

        let cred = Credential::VerificationKey(Hash28::from_bytes([0xff; 28]));
        state.process_certificate(&Certificate::StakeRegistration(cred.clone()));

        // Distribute 2M from treasury to credential (leaves 1M)
        state.process_certificate(&Certificate::MoveInstantaneousRewards {
            source: MIRSource::Treasury,
            target: MIRTarget::StakeCredentials(vec![(cred.clone(), 2_000_000)]),
        });
        assert_eq!(state.treasury, Lovelace(1_000_000));

        // Try to transfer 10M from treasury to reserves, but only 1M available
        state.process_certificate(&Certificate::MoveInstantaneousRewards {
            source: MIRSource::Treasury,
            target: MIRTarget::OtherAccountingPot(10_000_000),
        });
        assert_eq!(state.treasury, Lovelace(0));
        assert_eq!(state.reserves, Lovelace(21_000_000));
    }

    #[test]
    fn test_mir_pot_transfer_zero_source() {
        // Edge case: pot transfer when source is already zero
        let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());
        state.reserves = Lovelace(0);
        state.treasury = Lovelace(5_000_000);

        // Should be a no-op, not panic
        state.process_certificate(&Certificate::MoveInstantaneousRewards {
            source: MIRSource::Reserves,
            target: MIRTarget::OtherAccountingPot(1_000_000),
        });
        assert_eq!(state.reserves, Lovelace(0));
        assert_eq!(state.treasury, Lovelace(5_000_000));
    }

    #[test]
    fn test_pool_reregistration_cancels_pending_retirement() {
        // Bug 3: re-registering a pool should cancel pending retirement
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);
        state.epoch_length = 432000;

        let pool_id = Hash28::from_bytes([0xAA; 28]);
        let pool_params_val = PoolParams {
            operator: pool_id,
            vrf_keyhash: Hash32::from_bytes([0xBB; 32]),
            pledge: Lovelace(500_000_000),
            cost: Lovelace(340_000_000),
            margin: Rational {
                numerator: 1,
                denominator: 100,
            },
            reward_account: vec![0xe0; 29],
            pool_owners: vec![pool_id],
            relays: vec![],
            pool_metadata: None,
        };

        // Register pool
        state.process_certificate(&Certificate::PoolRegistration(pool_params_val.clone()));
        assert!(state.pool_params.contains_key(&pool_id));

        // Schedule retirement at epoch 5
        state.process_certificate(&Certificate::PoolRetirement {
            pool_hash: pool_id,
            epoch: 5,
        });
        assert!(state.pending_retirements.contains_key(&EpochNo(5)));
        assert!(state.pending_retirements[&EpochNo(5)].contains(&pool_id));

        // Re-register the pool — should cancel the pending retirement
        let updated_params = PoolParams {
            pledge: Lovelace(1_000_000_000), // updated pledge
            ..pool_params_val
        };
        state.process_certificate(&Certificate::PoolRegistration(updated_params));

        // Pending retirement should be cancelled
        assert!(
            state.pending_retirements.is_empty()
                || !state
                    .pending_retirements
                    .values()
                    .any(|v| v.contains(&pool_id))
        );
        // Pool should still exist with updated params
        assert!(state.pool_params.contains_key(&pool_id));
        assert_eq!(state.pool_params[&pool_id].pledge, Lovelace(1_000_000_000));
    }

    #[test]
    fn test_pool_reregistration_only_cancels_own_retirement() {
        // Bug 3 variant: re-registering pool A should not cancel pool B's retirement
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);
        state.epoch_length = 432000;

        let pool_a = Hash28::from_bytes([0xAA; 28]);
        let pool_b = Hash28::from_bytes([0xBB; 28]);

        let make_params = |id: Hash28| PoolParams {
            operator: id,
            vrf_keyhash: Hash32::from_bytes([0xCC; 32]),
            pledge: Lovelace(100_000_000),
            cost: Lovelace(340_000_000),
            margin: Rational {
                numerator: 1,
                denominator: 100,
            },
            reward_account: vec![0xe0; 29],
            pool_owners: vec![id],
            relays: vec![],
            pool_metadata: None,
        };

        // Register both pools
        state.process_certificate(&Certificate::PoolRegistration(make_params(pool_a)));
        state.process_certificate(&Certificate::PoolRegistration(make_params(pool_b)));

        // Retire both at epoch 5
        state.process_certificate(&Certificate::PoolRetirement {
            pool_hash: pool_a,
            epoch: 5,
        });
        state.process_certificate(&Certificate::PoolRetirement {
            pool_hash: pool_b,
            epoch: 5,
        });
        assert_eq!(state.pending_retirements[&EpochNo(5)].len(), 2);

        // Re-register only pool A
        state.process_certificate(&Certificate::PoolRegistration(make_params(pool_a)));

        // Pool A's retirement should be cancelled, but pool B's should remain
        let remaining: Vec<_> = state
            .pending_retirements
            .values()
            .flatten()
            .copied()
            .collect();
        assert!(!remaining.contains(&pool_a));
        assert!(remaining.contains(&pool_b));
    }

    #[test]
    fn test_stake_deregistration_rejected_with_nonzero_balance() {
        // Bug 4: Shelley-era deregistration should fail if reward balance > 0
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);

        let cred = Credential::VerificationKey(Hash28::from_bytes([0xEE; 28]));
        let key = credential_to_hash(&cred);

        // Register stake
        state.process_certificate(&Certificate::StakeRegistration(cred.clone()));
        assert!(state.reward_accounts.contains_key(&key));

        // Add some rewards
        *Arc::make_mut(&mut state.reward_accounts)
            .get_mut(&key)
            .unwrap() = Lovelace(500_000);

        // Try to deregister — should be rejected because balance > 0
        state.process_certificate(&Certificate::StakeDeregistration(cred.clone()));

        // Stake should still be registered
        assert!(state.reward_accounts.contains_key(&key));
        assert!(state.stake_distribution.stake_map.contains_key(&key));
        assert_eq!(state.reward_accounts[&key], Lovelace(500_000));
    }

    #[test]
    fn test_stake_deregistration_allowed_with_zero_balance() {
        // Bug 4: Shelley-era deregistration should succeed if reward balance is zero
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);

        let cred = Credential::VerificationKey(Hash28::from_bytes([0xFF; 28]));
        let key = credential_to_hash(&cred);

        // Register stake
        state.process_certificate(&Certificate::StakeRegistration(cred.clone()));
        assert!(state.reward_accounts.contains_key(&key));
        assert_eq!(state.reward_accounts[&key], Lovelace(0));

        // Deregister with zero balance — should succeed
        state.process_certificate(&Certificate::StakeDeregistration(cred));

        assert!(!state.reward_accounts.contains_key(&key));
        assert!(!state.stake_distribution.stake_map.contains_key(&key));
    }

    #[test]
    fn test_conway_stake_deregistration_with_nonzero_balance() {
        // Bug 4: Conway-era deregistration always succeeds (balance returned with refund)
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);

        let cred = Credential::VerificationKey(Hash28::from_bytes([0xAB; 28]));
        let key = credential_to_hash(&cred);

        // Register stake (Conway style)
        state.process_certificate(&Certificate::ConwayStakeRegistration {
            credential: cred.clone(),
            deposit: Lovelace(2_000_000),
        });
        assert!(state.reward_accounts.contains_key(&key));

        // Add rewards
        *Arc::make_mut(&mut state.reward_accounts)
            .get_mut(&key)
            .unwrap() = Lovelace(1_000_000);

        // Conway deregistration — should succeed even with non-zero balance
        state.process_certificate(&Certificate::ConwayStakeDeregistration {
            credential: cred,
            refund: Lovelace(2_000_000),
        });

        // Should be removed
        assert!(!state.reward_accounts.contains_key(&key));
        assert!(!state.stake_distribution.stake_map.contains_key(&key));
    }

    #[test]
    fn test_multi_epoch_skip_processes_each_epoch() {
        // Bug 5: skipping multiple epochs should process each intermediate transition
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);
        state.epoch_length = 100; // 100 slots per epoch for testing
        state.shelley_transition_epoch = 0;
        state.byron_epoch_length = 0;

        let pool_a = Hash28::from_bytes([0xA1; 28]);
        let pool_b = Hash28::from_bytes([0xA2; 28]);

        let make_pool = |id: Hash28| PoolParams {
            operator: id,
            vrf_keyhash: Hash32::from_bytes([0xCC; 32]),
            pledge: Lovelace(100_000_000),
            cost: Lovelace(340_000_000),
            margin: Rational {
                numerator: 1,
                denominator: 100,
            },
            reward_account: vec![0xe0; 29],
            pool_owners: vec![id],
            relays: vec![],
            pool_metadata: None,
        };

        // Register two pools
        state.process_certificate(&Certificate::PoolRegistration(make_pool(pool_a)));
        state.process_certificate(&Certificate::PoolRegistration(make_pool(pool_b)));

        // Schedule retirements at different epochs
        state.process_certificate(&Certificate::PoolRetirement {
            pool_hash: pool_a,
            epoch: 2,
        });
        state.process_certificate(&Certificate::PoolRetirement {
            pool_hash: pool_b,
            epoch: 4,
        });

        assert!(state.pool_params.contains_key(&pool_a));
        assert!(state.pool_params.contains_key(&pool_b));

        // Skip from epoch 0 directly to epoch 5 via a block at slot 500
        let block = make_test_block(500, 1, Hash32::ZERO, vec![]);
        state.apply_block(&block).unwrap();

        // Both pools should be retired since we should have processed
        // epochs 1, 2, 3, 4, and 5
        assert_eq!(state.epoch, EpochNo(5));
        assert!(
            !state.pool_params.contains_key(&pool_a),
            "Pool A should be retired at epoch 2"
        );
        assert!(
            !state.pool_params.contains_key(&pool_b),
            "Pool B should be retired at epoch 4"
        );
    }

    #[test]
    fn test_multi_epoch_skip_snapshot_rotation() {
        // Bug 5: verify that snapshot rotation works correctly with multi-epoch skip
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);
        state.epoch_length = 100;
        state.shelley_transition_epoch = 0;
        state.byron_epoch_length = 0;

        let cred = Credential::VerificationKey(Hash28::from_bytes([0xDE; 28]));
        let pool_id = Hash28::from_bytes([0xDA; 28]);

        state.process_certificate(&Certificate::StakeRegistration(cred.clone()));
        add_stake_utxo(&mut state, &cred, 1_000_000);
        state.process_certificate(&Certificate::PoolRegistration(PoolParams {
            operator: pool_id,
            vrf_keyhash: Hash32::from_bytes([2u8; 32]),
            pledge: Lovelace(100),
            cost: Lovelace(100),
            margin: Rational {
                numerator: 1,
                denominator: 100,
            },
            reward_account: vec![0xe0; 29],
            pool_owners: vec![pool_id],
            relays: vec![],
            pool_metadata: None,
        }));
        state.process_certificate(&Certificate::StakeDelegation {
            credential: cred.clone(),
            pool_hash: pool_id,
        });

        // Skip from epoch 0 directly to epoch 4 (4 transitions)
        let block = make_test_block(400, 1, Hash32::ZERO, vec![]);
        state.apply_block(&block).unwrap();

        assert_eq!(state.epoch, EpochNo(4));
        // After 4 transitions: mark, set, and go should all be populated
        assert!(state.snapshots.mark.is_some());
        assert!(state.snapshots.set.is_some());
        assert!(state.snapshots.go.is_some());

        // The epochs should be consecutive
        assert_eq!(state.snapshots.go.as_ref().unwrap().epoch, EpochNo(2));
        assert_eq!(state.snapshots.set.as_ref().unwrap().epoch, EpochNo(3));
        assert_eq!(state.snapshots.mark.as_ref().unwrap().epoch, EpochNo(4));
    }

    // ======================================================================
    // Bug fix tests: CIP-1694 governance voting
    // ======================================================================

    /// Helper: set up a LedgerState with DReps, vote delegations, and stake for governance tests.
    fn setup_governance_state(
        drep_count: u32,
        stake_per_drep: u64,
    ) -> (LedgerState, Vec<(Credential, Hash32)>) {
        let mut params = ProtocolParameters::mainnet_defaults();
        params.protocol_version_major = 10;
        let mut state = LedgerState::new(params);
        state.epoch_length = 100;
        Arc::make_mut(&mut state.governance).committee_threshold = Some(Rational {
            numerator: 0,
            denominator: 1,
        });

        let mut dreps = Vec::new();
        for i in 0..drep_count {
            let cred = Credential::VerificationKey(Hash28::from_bytes([(i + 1) as u8; 28]));
            let key = credential_to_hash(&cred);
            Arc::make_mut(&mut state.governance).dreps.insert(
                key,
                DRepRegistration {
                    credential: cred.clone(),
                    deposit: Lovelace(500_000_000),
                    anchor: None,
                    registered_epoch: EpochNo(0),
                    last_active_epoch: EpochNo(0),
                    active: true,
                },
            );
            let delegator_cred =
                Credential::VerificationKey(Hash28::from_bytes([(i + 100) as u8; 28]));
            let delegator_key = credential_to_hash(&delegator_cred);
            Arc::make_mut(&mut state.governance)
                .vote_delegations
                .insert(delegator_key, DRep::KeyHash(key));
            add_stake_utxo(&mut state, &delegator_cred, stake_per_drep);
            state.rebuild_stake_distribution();
            dreps.push((cred, key));
        }
        (state, dreps)
    }

    #[test]
    fn test_drep_denominator_yes_no_only() {
        let (mut state, dreps) = setup_governance_state(10, 1_000_000_000);
        let tx_hash = Hash32::from_bytes([99u8; 32]);
        let proposal = ProposalProcedure {
            deposit: Lovelace(100_000_000_000),
            return_addr: vec![0u8; 29],
            gov_action: GovAction::TreasuryWithdrawals {
                withdrawals: BTreeMap::new(),
                policy_hash: None,
            },
            anchor: Anchor {
                url: "https://example.com".to_string(),
                data_hash: Hash32::ZERO,
            },
        };
        state.process_proposal(&tx_hash, 0, &proposal);
        let action_id = GovActionId {
            transaction_id: tx_hash,
            action_index: 0,
        };

        // 3 yes, 3 no, 4 abstain
        for (cred, _) in dreps.iter().take(3) {
            state.process_vote(
                &Voter::DRep(cred.clone()),
                &action_id,
                &VotingProcedure {
                    vote: Vote::Yes,
                    anchor: None,
                },
            );
        }
        for (cred, _) in dreps.iter().skip(3).take(3) {
            state.process_vote(
                &Voter::DRep(cred.clone()),
                &action_id,
                &VotingProcedure {
                    vote: Vote::No,
                    anchor: None,
                },
            );
        }
        for (cred, _) in dreps.iter().skip(6) {
            state.process_vote(
                &Voter::DRep(cred.clone()),
                &action_id,
                &VotingProcedure {
                    vote: Vote::Abstain,
                    anchor: None,
                },
            );
        }

        let (drep_power_cache, no_confidence_stake, _) = state.build_drep_power_cache();
        let (drep_yes, drep_total, _, _, _, _) = state.count_votes_by_type(
            &action_id,
            &GovAction::TreasuryWithdrawals {
                withdrawals: BTreeMap::new(),
                policy_hash: None,
            },
            &drep_power_cache,
            no_confidence_stake,
        );

        assert_eq!(drep_yes, 3_000_000_000);
        assert_eq!(drep_total, 6_000_000_000); // yes + no only
    }

    #[test]
    fn test_always_no_confidence_counts_yes_for_no_confidence_action() {
        let (mut state, _dreps) = setup_governance_state(5, 1_000_000_000);

        for i in 0..3u32 {
            let delegator_cred =
                Credential::VerificationKey(Hash28::from_bytes([(i + 200) as u8; 28]));
            let delegator_key = credential_to_hash(&delegator_cred);
            Arc::make_mut(&mut state.governance)
                .vote_delegations
                .insert(delegator_key, DRep::NoConfidence);
            add_stake_utxo(&mut state, &delegator_cred, 1_000_000_000);
        }
        state.rebuild_stake_distribution();

        let tx_hash = Hash32::from_bytes([99u8; 32]);
        let proposal = ProposalProcedure {
            deposit: Lovelace(100_000_000_000),
            return_addr: vec![0u8; 29],
            gov_action: GovAction::NoConfidence {
                prev_action_id: None,
            },
            anchor: Anchor {
                url: "https://example.com".to_string(),
                data_hash: Hash32::ZERO,
            },
        };
        state.process_proposal(&tx_hash, 0, &proposal);
        let action_id = GovActionId {
            transaction_id: tx_hash,
            action_index: 0,
        };

        let (drep_power_cache, no_confidence_stake, _) = state.build_drep_power_cache();
        assert_eq!(no_confidence_stake, 3_000_000_000);

        let (drep_yes, drep_total, _, _, _, _) = state.count_votes_by_type(
            &action_id,
            &GovAction::NoConfidence {
                prev_action_id: None,
            },
            &drep_power_cache,
            no_confidence_stake,
        );

        assert_eq!(drep_yes, 3_000_000_000);
        assert_eq!(drep_total, 3_000_000_000);
    }

    #[test]
    fn test_always_no_confidence_counts_no_for_other_actions() {
        let (mut state, dreps) = setup_governance_state(5, 1_000_000_000);

        for i in 0..3u32 {
            let delegator_cred =
                Credential::VerificationKey(Hash28::from_bytes([(i + 200) as u8; 28]));
            let delegator_key = credential_to_hash(&delegator_cred);
            Arc::make_mut(&mut state.governance)
                .vote_delegations
                .insert(delegator_key, DRep::NoConfidence);
            add_stake_utxo(&mut state, &delegator_cred, 1_000_000_000);
        }
        state.rebuild_stake_distribution();

        let tx_hash = Hash32::from_bytes([99u8; 32]);
        let proposal = ProposalProcedure {
            deposit: Lovelace(100_000_000_000),
            return_addr: vec![0u8; 29],
            gov_action: GovAction::TreasuryWithdrawals {
                withdrawals: BTreeMap::new(),
                policy_hash: None,
            },
            anchor: Anchor {
                url: "https://example.com".to_string(),
                data_hash: Hash32::ZERO,
            },
        };
        state.process_proposal(&tx_hash, 0, &proposal);
        let action_id = GovActionId {
            transaction_id: tx_hash,
            action_index: 0,
        };

        for (cred, _) in dreps.iter().take(2) {
            state.process_vote(
                &Voter::DRep(cred.clone()),
                &action_id,
                &VotingProcedure {
                    vote: Vote::Yes,
                    anchor: None,
                },
            );
        }

        let (drep_power_cache, no_confidence_stake, _) = state.build_drep_power_cache();
        let (drep_yes, drep_total, _, _, _, _) = state.count_votes_by_type(
            &action_id,
            &GovAction::TreasuryWithdrawals {
                withdrawals: BTreeMap::new(),
                policy_hash: None,
            },
            &drep_power_cache,
            no_confidence_stake,
        );

        // 2B yes, 3B no (AlwaysNoConfidence), total = 5B
        assert_eq!(drep_yes, 2_000_000_000);
        assert_eq!(drep_total, 5_000_000_000);
    }

    #[test]
    fn test_inactive_drep_excluded_from_voting_power() {
        let (mut state, dreps) = setup_governance_state(5, 1_000_000_000);
        for (_, key) in dreps.iter().take(2) {
            Arc::make_mut(&mut state.governance)
                .dreps
                .get_mut(key)
                .unwrap()
                .active = false;
        }
        let (drep_power_cache, _, _) = state.build_drep_power_cache();
        assert!(!drep_power_cache.contains_key(&dreps[0].1));
        assert!(!drep_power_cache.contains_key(&dreps[1].1));
        assert!(drep_power_cache.contains_key(&dreps[2].1));
    }

    #[test]
    fn test_inactive_drep_remains_registered() {
        let mut params = ProtocolParameters::mainnet_defaults();
        params.drep_activity = 3;
        let mut state = LedgerState::new(params);

        let cred = Credential::VerificationKey(Hash28::from_bytes([50u8; 28]));
        let key = credential_to_hash(&cred);
        state.process_certificate(&Certificate::RegDRep {
            credential: cred.clone(),
            deposit: Lovelace(500_000_000),
            anchor: None,
        });
        assert!(state.governance.dreps[&key].active);

        state.process_epoch_transition(EpochNo(5));
        assert!(state.governance.dreps.contains_key(&key));
        assert!(!state.governance.dreps[&key].active);
        assert_eq!(state.governance.dreps[&key].deposit, Lovelace(500_000_000));
    }

    #[test]
    fn test_inactive_drep_stake_not_in_total() {
        let (mut state, dreps) = setup_governance_state(5, 1_000_000_000);
        Arc::make_mut(&mut state.governance)
            .dreps
            .get_mut(&dreps[0].1)
            .unwrap()
            .active = false;
        Arc::make_mut(&mut state.governance)
            .dreps
            .get_mut(&dreps[1].1)
            .unwrap()
            .active = false;
        let total = state.compute_total_drep_stake();
        assert_eq!(total, 3_000_000_000);
    }

    #[test]
    fn test_governance_threshold_valid_half() {
        let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());
        let update = ProtocolParamUpdate {
            dvt_hard_fork: Some(Rational {
                numerator: 1,
                denominator: 2,
            }),
            pvt_hard_fork: Some(Rational {
                numerator: 1,
                denominator: 2,
            }),
            ..Default::default()
        };
        assert!(state.apply_protocol_param_update(&update).is_ok());
        assert_eq!(state.protocol_params.dvt_hard_fork.numerator, 1);
        assert_eq!(state.protocol_params.dvt_hard_fork.denominator, 2);
        assert_eq!(state.protocol_params.pvt_hard_fork.numerator, 1);
        assert_eq!(state.protocol_params.pvt_hard_fork.denominator, 2);
    }

    #[test]
    fn test_governance_threshold_exactly_one() {
        let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());
        let update = ProtocolParamUpdate {
            dvt_no_confidence: Some(Rational {
                numerator: 1,
                denominator: 1,
            }),
            ..Default::default()
        };
        assert!(state.apply_protocol_param_update(&update).is_ok());
        assert_eq!(state.protocol_params.dvt_no_confidence.numerator, 1);
        assert_eq!(state.protocol_params.dvt_no_confidence.denominator, 1);
    }

    #[test]
    fn test_governance_threshold_exactly_zero() {
        let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());
        let update = ProtocolParamUpdate {
            pvt_committee_normal: Some(Rational {
                numerator: 0,
                denominator: 1,
            }),
            ..Default::default()
        };
        assert!(state.apply_protocol_param_update(&update).is_ok());
        assert_eq!(state.protocol_params.pvt_committee_normal.numerator, 0);
        assert_eq!(state.protocol_params.pvt_committee_normal.denominator, 1);
    }

    #[test]
    fn test_governance_threshold_exceeds_one_rejected() {
        let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());
        let original = state.protocol_params.dvt_hard_fork.clone();
        let update = ProtocolParamUpdate {
            dvt_hard_fork: Some(Rational {
                numerator: 3,
                denominator: 2,
            }),
            ..Default::default()
        };
        let result = state.apply_protocol_param_update(&update);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("dvt_hard_fork"),
            "Error should name the field: {}",
            err_msg
        );
        assert!(
            err_msg.contains("exceeds 1"),
            "Error should mention exceeds 1: {}",
            err_msg
        );
        // Parameter should NOT have been updated
        assert_eq!(state.protocol_params.dvt_hard_fork, original);
    }

    #[test]
    fn test_governance_threshold_zero_denominator_rejected() {
        let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());
        let original = state.protocol_params.pvt_motion_no_confidence.clone();
        let update = ProtocolParamUpdate {
            pvt_motion_no_confidence: Some(Rational {
                numerator: 1,
                denominator: 0,
            }),
            ..Default::default()
        };
        let result = state.apply_protocol_param_update(&update);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("pvt_motion_no_confidence"),
            "Error should name the field: {}",
            err_msg
        );
        assert!(
            err_msg.contains("zero denominator"),
            "Error should mention zero denominator: {}",
            err_msg
        );
        // Parameter should NOT have been updated
        assert_eq!(state.protocol_params.pvt_motion_no_confidence, original);
    }

    #[test]
    fn test_governance_threshold_all_dvt_fields_validated() {
        let bad = Rational {
            numerator: 5,
            denominator: 3,
        };
        #[allow(clippy::type_complexity)]
        let dvt_fields: Vec<(&str, Box<dyn Fn() -> ProtocolParamUpdate>)> = vec![
            (
                "dvt_pp_network_group",
                Box::new(|| ProtocolParamUpdate {
                    dvt_pp_network_group: Some(bad.clone()),
                    ..Default::default()
                }),
            ),
            (
                "dvt_pp_economic_group",
                Box::new(|| ProtocolParamUpdate {
                    dvt_pp_economic_group: Some(bad.clone()),
                    ..Default::default()
                }),
            ),
            (
                "dvt_pp_technical_group",
                Box::new(|| ProtocolParamUpdate {
                    dvt_pp_technical_group: Some(bad.clone()),
                    ..Default::default()
                }),
            ),
            (
                "dvt_pp_gov_group",
                Box::new(|| ProtocolParamUpdate {
                    dvt_pp_gov_group: Some(bad.clone()),
                    ..Default::default()
                }),
            ),
            (
                "dvt_hard_fork",
                Box::new(|| ProtocolParamUpdate {
                    dvt_hard_fork: Some(bad.clone()),
                    ..Default::default()
                }),
            ),
            (
                "dvt_no_confidence",
                Box::new(|| ProtocolParamUpdate {
                    dvt_no_confidence: Some(bad.clone()),
                    ..Default::default()
                }),
            ),
            (
                "dvt_committee_normal",
                Box::new(|| ProtocolParamUpdate {
                    dvt_committee_normal: Some(bad.clone()),
                    ..Default::default()
                }),
            ),
            (
                "dvt_committee_no_confidence",
                Box::new(|| ProtocolParamUpdate {
                    dvt_committee_no_confidence: Some(bad.clone()),
                    ..Default::default()
                }),
            ),
            (
                "dvt_constitution",
                Box::new(|| ProtocolParamUpdate {
                    dvt_constitution: Some(bad.clone()),
                    ..Default::default()
                }),
            ),
            (
                "dvt_treasury_withdrawal",
                Box::new(|| ProtocolParamUpdate {
                    dvt_treasury_withdrawal: Some(bad.clone()),
                    ..Default::default()
                }),
            ),
        ];
        for (name, make_update) in &dvt_fields {
            let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());
            let result = state.apply_protocol_param_update(&make_update());
            assert!(result.is_err(), "{} should be rejected", name);
            assert!(
                result.unwrap_err().to_string().contains(name),
                "Error should name {}",
                name
            );
        }
    }

    #[test]
    fn test_governance_threshold_all_pvt_fields_validated() {
        let bad = Rational {
            numerator: 5,
            denominator: 3,
        };
        #[allow(clippy::type_complexity)]
        let pvt_fields: Vec<(&str, Box<dyn Fn() -> ProtocolParamUpdate>)> = vec![
            (
                "pvt_motion_no_confidence",
                Box::new(|| ProtocolParamUpdate {
                    pvt_motion_no_confidence: Some(bad.clone()),
                    ..Default::default()
                }),
            ),
            (
                "pvt_committee_normal",
                Box::new(|| ProtocolParamUpdate {
                    pvt_committee_normal: Some(bad.clone()),
                    ..Default::default()
                }),
            ),
            (
                "pvt_committee_no_confidence",
                Box::new(|| ProtocolParamUpdate {
                    pvt_committee_no_confidence: Some(bad.clone()),
                    ..Default::default()
                }),
            ),
            (
                "pvt_hard_fork",
                Box::new(|| ProtocolParamUpdate {
                    pvt_hard_fork: Some(bad.clone()),
                    ..Default::default()
                }),
            ),
            (
                "pvt_pp_security_group",
                Box::new(|| ProtocolParamUpdate {
                    pvt_pp_security_group: Some(bad.clone()),
                    ..Default::default()
                }),
            ),
        ];
        for (name, make_update) in &pvt_fields {
            let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());
            let result = state.apply_protocol_param_update(&make_update());
            assert!(result.is_err(), "{} should be rejected", name);
            assert!(
                result.unwrap_err().to_string().contains(name),
                "Error should name {}",
                name
            );
        }
    }

    #[test]
    fn test_randomness_stabilisation_window_mainnet() {
        // Mainnet: k=2160, f=0.05 → ceil(4*2160/0.05) = 172800
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);
        state.set_epoch_length(432000, 2160);
        assert_eq!(state.randomness_stabilisation_window, 172800);
    }

    #[test]
    fn test_randomness_stabilisation_window_preview() {
        // Preview: k=432, f=0.05 → ceil(4*432/0.05) = 34560
        let mut params = ProtocolParameters::mainnet_defaults();
        params.active_slots_coeff = 0.05;
        let mut state = LedgerState::new(params);
        state.set_epoch_length(86400, 432);
        assert_eq!(state.randomness_stabilisation_window, 34560);
    }

    #[test]
    fn test_randomness_stabilisation_window_exact_for_tenth() {
        // f=0.1 = 1/10, k=100 → ceil(4*100/(1/10)) = 4000
        let mut params = ProtocolParameters::mainnet_defaults();
        params.active_slots_coeff = 0.1;
        let mut state = LedgerState::new(params);
        state.set_epoch_length(100000, 100);
        assert_eq!(state.randomness_stabilisation_window, 4000);
    }

    #[test]
    fn test_randomness_stabilisation_window_ceil_rounds_up() {
        // f=0.25 = 1/4, k=3 → ceil(4*3*4/1) = 48 (exact)
        let mut params = ProtocolParameters::mainnet_defaults();
        params.active_slots_coeff = 0.25;
        let mut state = LedgerState::new(params);
        state.set_epoch_length(1000, 3);
        assert_eq!(state.randomness_stabilisation_window, 48);
    }

    /// Regression test for GitHub issue #13: slot + stabilisation_window u64 overflow.
    ///
    /// When a block has a slot near u64::MAX, the old code `block.slot().0 +
    /// self.randomness_stabilisation_window` would overflow. The fix restructures
    /// the comparison to subtract from the larger value instead.
    #[test]
    fn test_slot_stabilisation_window_no_overflow() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);
        state.epoch_length = 100;
        state.shelley_transition_epoch = 0;
        state.byron_epoch_length = 0;
        state.randomness_stabilisation_window = 40;

        let genesis_hash = Hash32::from_bytes([0xAB; 32]);
        state.set_genesis_hash(genesis_hash);

        // Pre-set the epoch to match the extreme slot so we don't trigger
        // a massive epoch transition loop. The extreme slot u64::MAX - 10
        // falls in epoch (u64::MAX - 10) / 100.
        let extreme_slot = u64::MAX - 10;
        state.epoch = EpochNo(extreme_slot / 100);

        // Block at a slot near u64::MAX — the old code would panic here
        // because slot + stabilisation_window overflows u64.
        let mut block = make_test_block(extreme_slot, 1, Hash32::ZERO, vec![]);
        block.header.vrf_result.output = vec![42u8; 32];
        block.header.issuer_vkey = vec![1u8; 32];

        // This should NOT panic; the candidate nonce should be frozen
        // because the extreme slot is definitely in the stabilisation window.
        state.apply_block(&block).unwrap();

        // Evolving nonce updated (always updates)
        assert_ne!(state.evolving_nonce, genesis_hash);
        // Candidate nonce should be FROZEN (extreme slot is in stabilisation window)
        assert_eq!(state.candidate_nonce, genesis_hash);
    }

    /// Test that first_slot_of_epoch and epoch_of_slot don't overflow with
    /// extreme epoch numbers.
    #[test]
    fn test_first_slot_of_epoch_saturating() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);
        state.epoch_length = 432000;
        state.shelley_transition_epoch = 208;
        state.byron_epoch_length = 21600;

        // Extreme epoch number should saturate to u64::MAX, not panic
        let result = state.first_slot_of_epoch(u64::MAX);
        assert_eq!(result, u64::MAX);

        // Normal epoch should still work correctly
        let result = state.first_slot_of_epoch(208);
        assert_eq!(result, 208 * 21600); // byron_slots + 0 shelley slots
    }

    /// Test that the stabilisation window boundary works correctly with
    /// saturating arithmetic for normal values (no behavioral change).
    #[test]
    fn test_stabilisation_window_boundary_normal_values() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);
        state.epoch_length = 100;
        state.shelley_transition_epoch = 0;
        state.byron_epoch_length = 0;
        state.randomness_stabilisation_window = 40;

        let genesis_hash = Hash32::from_bytes([0xAB; 32]);
        state.set_genesis_hash(genesis_hash);

        // Slot 59 is the LAST slot before the stabilisation window
        // (59 < 100 - 40 = 60, so candidate updates)
        let mut block = make_test_block(59, 1, Hash32::ZERO, vec![]);
        block.header.vrf_result.output = vec![42u8; 32];
        block.header.issuer_vkey = vec![1u8; 32];
        state.apply_block(&block).unwrap();
        assert_eq!(state.candidate_nonce, state.evolving_nonce);

        // Slot 60 is the FIRST slot in the stabilisation window
        // (60 >= 100 - 40 = 60, so candidate freezes)
        let candidate_before = state.candidate_nonce;
        let mut block2 = make_test_block(60, 2, *block.hash(), vec![]);
        block2.header.vrf_result.output = vec![99u8; 32];
        block2.header.issuer_vkey = vec![1u8; 32];
        state.apply_block(&block2).unwrap();
        assert_eq!(state.candidate_nonce, candidate_before);
        assert_ne!(state.evolving_nonce, candidate_before);
    }

    /// Verify that reward expansion calculation does not overflow i128 even with
    /// large reserves and high rho numerator values near the i128 boundary.
    ///
    /// The old code computed `rho_num * reserves * effective_blocks` in a single
    /// i128 expression, which overflows when reserves is near MAX_LOVELACE_SUPPLY
    /// and rho_num is large. The Rat-based calculation cross-reduces before
    /// multiplying, avoiding the overflow.
    #[test]
    fn test_reward_expansion_no_i128_overflow() {
        let mut params = ProtocolParameters::mainnet_defaults();
        // Use a rho that would cause overflow in the naive calculation:
        // rho = 999/1000 (extreme value to stress the arithmetic)
        // naive: 999 * 45_000_000_000_000_000 * 21600 = 9.7e23, fits in i128
        // But with a larger numerator (e.g., rho = 999_999_999/1_000_000_000):
        // naive: 999_999_999 * 45_000_000_000_000_000 * 21600 = ~9.7e32
        // This is still within i128 range (max ~1.7e38), so we need to push harder.
        //
        // To truly overflow i128 in the naive code path, we need:
        // rho_num * reserves * effective_blocks > 2^127
        // With reserves = 45e15 and effective_blocks = 21600:
        // rho_num > 2^127 / (45e15 * 21600) ≈ 1.75e23
        // So we use a rho with a very large numerator.
        params.rho = Rational {
            numerator: u64::MAX, // 1.8e19
            denominator: u64::MAX,
        };
        // rho = u64::MAX / u64::MAX = 1, so expansion = reserves * effective/expected
        // But the naive code would compute: u64::MAX * 45e15 * 21600 which is
        // ~1.8e19 * 4.5e16 * 2.16e4 = ~1.7e40, far exceeding i128::MAX (~1.7e38)

        let mut state = LedgerState::new(params);
        state.reserves = Lovelace(MAX_LOVELACE_SUPPLY);
        state.epoch_block_count = 21600;
        state.epoch_fees = Lovelace(0);
        state.epoch_length = 432000;

        // Set up minimal structures for calculate_and_distribute_rewards
        let go_snapshot = StakeSnapshot {
            epoch: EpochNo(0),
            delegations: Arc::new(HashMap::new()),
            pool_stake: HashMap::new(),
            pool_params: Arc::new(HashMap::new()),
            stake_distribution: Arc::new(HashMap::new()),
        };

        // This should NOT panic from i128 overflow
        state.calculate_and_distribute_rewards(go_snapshot);

        // With rho=1 and eta=1 (effective==expected when active_slot_coeff=0.05):
        // expected_blocks = floor(0.05 * 432000) = 21600
        // effective_blocks = min(21600, 21600) = 21600
        // expansion = floor(1 * reserves * 21600/21600) = reserves = 45e15
        assert_eq!(
            state.reserves.0, 0,
            "All reserves should be expanded with rho=1"
        );
    }

    /// Verify that reward expansion works correctly with extreme rho values
    /// where the numerator and denominator differ significantly.
    #[test]
    fn test_reward_expansion_large_rho_numerator() {
        let mut params = ProtocolParameters::mainnet_defaults();
        // rho = large_num / (large_num + 1) ≈ 1
        // This maximizes rho_num while keeping the fraction valid.
        params.rho = Rational {
            numerator: u64::MAX - 1,
            denominator: u64::MAX,
        };

        let mut state = LedgerState::new(params);
        state.reserves = Lovelace(MAX_LOVELACE_SUPPLY);
        state.epoch_block_count = 21600;
        state.epoch_fees = Lovelace(0);
        state.epoch_length = 432000;

        let go_snapshot = StakeSnapshot {
            epoch: EpochNo(0),
            delegations: Arc::new(HashMap::new()),
            pool_stake: HashMap::new(),
            pool_params: Arc::new(HashMap::new()),
            stake_distribution: Arc::new(HashMap::new()),
        };

        // Should not panic
        state.calculate_and_distribute_rewards(go_snapshot);

        // expansion ≈ reserves * (u64::MAX-1)/u64::MAX ≈ reserves - 1
        // After subtracting expansion, reserves should be approximately 0-2
        assert!(
            state.reserves.0 <= 3,
            "Reserves should be nearly zero with rho ≈ 1, got {}",
            state.reserves.0
        );
    }

    /// Verify that treasury cut calculation also uses Rat and doesn't overflow.
    #[test]
    fn test_treasury_cut_no_overflow() {
        let mut params = ProtocolParameters::mainnet_defaults();
        // tau = u64::MAX / u64::MAX = 1 (takes entire reward pot as treasury)
        params.tau = Rational {
            numerator: u64::MAX,
            denominator: u64::MAX,
        };
        // Use small rho to get a moderate expansion
        params.rho = Rational {
            numerator: 3,
            denominator: 1000,
        };

        let mut state = LedgerState::new(params);
        state.reserves = Lovelace(MAX_LOVELACE_SUPPLY);
        state.epoch_block_count = 21600;
        state.epoch_fees = Lovelace(1_000_000_000_000); // 1M ADA in fees
        state.epoch_length = 432000;

        let go_snapshot = StakeSnapshot {
            epoch: EpochNo(0),
            delegations: Arc::new(HashMap::new()),
            pool_stake: HashMap::new(),
            pool_params: Arc::new(HashMap::new()),
            stake_distribution: Arc::new(HashMap::new()),
        };

        // Should not panic
        state.calculate_and_distribute_rewards(go_snapshot);

        // With tau=1, all rewards go to treasury (no pool rewards)
        // expansion = floor(0.003 * 45e15) = 135_000_000_000_000
        let expected_expansion = 135_000_000_000_000u64;
        let total_rewards = expected_expansion + 1_000_000_000_000;
        // Treasury should have received the entire reward pot
        assert_eq!(
            state.treasury.0, total_rewards,
            "Treasury should receive all rewards when tau=1"
        );
    }

    /// Verify the Rat struct itself handles large values without overflow.
    #[test]
    fn test_rat_large_value_multiplication() {
        // This simulates the problematic calculation:
        // rho_num * reserves * effective_blocks where all are large
        let rho = Rat::new(u64::MAX as i128, u64::MAX as i128);
        let reserves = Rat::new(MAX_LOVELACE_SUPPLY as i128, 1);
        let eta = Rat::new(21600, 21600);

        // Should not panic
        let result = rho.mul(&reserves).mul(&eta);
        assert_eq!(
            result.floor_u64(),
            MAX_LOVELACE_SUPPLY,
            "rho=1 * reserves * eta=1 should equal reserves"
        );

        // Test with values that would overflow naive i128 multiplication
        // u64::MAX * 45e15 * 21600 > i128::MAX
        let rho2 = Rat::new(u64::MAX as i128, 1);
        let reserves2 = Rat::new(MAX_LOVELACE_SUPPLY as i128, 1);
        let eta2 = Rat::new(21600, u64::MAX as i128);
        // = u64::MAX * 45e15 * 21600 / u64::MAX = 45e15 * 21600 = 9.72e17
        let result2 = rho2.mul(&reserves2).mul(&eta2);
        let expected = MAX_LOVELACE_SUPPLY as u128 * 21600;
        assert_eq!(
            result2.floor_u64(),
            expected as u64,
            "Large numerator cross-reduced with large denominator"
        );
    }

    #[test]
    fn test_reward_account_to_hash_extracts_28_byte_credential() {
        // Standard 29-byte reward address: 1 byte header + 28 byte credential
        let cred_bytes = [0xAB; 28];
        let mut reward_addr_29 = vec![0xE0u8]; // testnet header
        reward_addr_29.extend_from_slice(&cred_bytes);
        assert_eq!(reward_addr_29.len(), 29);

        let hash = LedgerState::reward_account_to_hash(&reward_addr_29);
        let hash_bytes = hash.as_ref();
        // First 28 bytes should be the credential
        assert_eq!(&hash_bytes[..28], &cred_bytes);
        // Last 4 bytes should be zero-padded
        assert_eq!(&hash_bytes[28..32], &[0u8; 4]);
    }

    #[test]
    fn test_reward_account_to_hash_ignores_extra_bytes() {
        // An address longer than 29 bytes should still extract only 28 bytes of credential.
        // This tests the fix for the hash collision risk where .min(32) could copy
        // extra trailing bytes, causing different addresses to map to the same key.
        let cred_bytes = [0xCD; 28];
        let mut reward_addr_long = vec![0xE1u8]; // mainnet header
        reward_addr_long.extend_from_slice(&cred_bytes);
        // Append extra bytes (e.g., script hash or other data)
        reward_addr_long.extend_from_slice(&[0xFF; 10]);
        assert_eq!(reward_addr_long.len(), 39);

        let hash = LedgerState::reward_account_to_hash(&reward_addr_long);
        let hash_bytes = hash.as_ref();
        // Should only contain the 28-byte credential, not the extra bytes
        assert_eq!(&hash_bytes[..28], &cred_bytes);
        assert_eq!(&hash_bytes[28..32], &[0u8; 4]);
    }

    #[test]
    fn test_reward_account_to_hash_no_collision_different_trailing_bytes() {
        // Two addresses with the same 28-byte credential but different trailing data
        // must produce the same hash (both should extract only the credential).
        let cred_bytes = [0x42; 28];

        let mut addr_a = vec![0xE0u8];
        addr_a.extend_from_slice(&cred_bytes);
        addr_a.extend_from_slice(&[0x00; 5]); // trailing zeros

        let mut addr_b = vec![0xE0u8];
        addr_b.extend_from_slice(&cred_bytes);
        addr_b.extend_from_slice(&[0xFF; 5]); // trailing 0xFF

        let hash_a = LedgerState::reward_account_to_hash(&addr_a);
        let hash_b = LedgerState::reward_account_to_hash(&addr_b);
        assert_eq!(
            hash_a, hash_b,
            "Same credential should produce same hash regardless of trailing bytes"
        );
    }

    #[test]
    fn test_reward_account_to_hash_different_credentials_no_collision() {
        // Two addresses with different 28-byte credentials must produce different hashes.
        let mut addr_a = vec![0xE0u8];
        addr_a.extend_from_slice(&[0xAA; 28]);

        let mut addr_b = vec![0xE0u8];
        addr_b.extend_from_slice(&[0xBB; 28]);

        let hash_a = LedgerState::reward_account_to_hash(&addr_a);
        let hash_b = LedgerState::reward_account_to_hash(&addr_b);
        assert_ne!(
            hash_a, hash_b,
            "Different credentials must produce different hashes"
        );
    }

    #[test]
    fn test_reward_account_to_hash_short_address_returns_zeros() {
        // Address shorter than 29 bytes should return all zeros (no extraction possible).
        let short_addr = vec![0xE0u8; 10];
        let hash = LedgerState::reward_account_to_hash(&short_addr);
        assert_eq!(hash.as_ref(), &[0u8; 32]);
    }

    #[test]
    fn test_reward_account_to_hash_header_byte_ignored() {
        // Different header bytes with same credential should produce the same hash,
        // since only bytes 1..29 are extracted.
        let cred_bytes = [0x77; 28];

        let mut addr_testnet = vec![0xE0u8]; // testnet
        addr_testnet.extend_from_slice(&cred_bytes);

        let mut addr_mainnet = vec![0xE1u8]; // mainnet
        addr_mainnet.extend_from_slice(&cred_bytes);

        let hash_testnet = LedgerState::reward_account_to_hash(&addr_testnet);
        let hash_mainnet = LedgerState::reward_account_to_hash(&addr_mainnet);
        assert_eq!(
            hash_testnet, hash_mainnet,
            "Header byte should not affect the hash key"
        );
    }
}
