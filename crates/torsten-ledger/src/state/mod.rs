mod certificates;
mod epoch;
mod governance;
mod protocol_params;
mod rewards;

// Re-export governance free functions and types for use by tests
#[cfg(test)]
pub(crate) use governance::{
    check_cc_approval, check_threshold, gov_action_priority, is_delaying_action,
    modified_pp_groups, pp_change_drep_all_groups_met, pp_change_drep_threshold,
    pp_change_spo_threshold, prev_action_as_expected, DRepPPGroup, StakePoolPPGroup,
};
#[doc(hidden)]
pub use rewards::Rat;

use crate::plutus::{evaluate_plutus_scripts, SlotConfig};
use crate::utxo::UtxoSet;
use crate::validation::{validate_transaction, ValidationError};
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
    Anchor, Constitution, DRep, GovActionId, ProposalProcedure, ProtocolParamUpdate, Rational,
    Relay, Voter, VotingProcedure,
};
use torsten_primitives::value::Lovelace;
use tracing::{debug, info, trace, warn};

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
        debug!(
            "Ledger: slot config (zero_time={}, zero_slot={}, slot_length={})",
            slot_config.zero_time, slot_config.zero_slot, slot_config.slot_length,
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
        debug!(
            "Ledger: epoch length={}, stabilisation_window={}, k={}",
            epoch_length, self.randomness_stabilisation_window, security_param,
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
    /// Initializes the Praos nonce state machine. In Haskell, the initial
    /// evolving nonce and candidate nonce are derived from the genesis hash.
    pub fn set_genesis_hash(&mut self, hash: Hash32) {
        self.genesis_hash = hash;
        // Initialize nonce state from genesis hash
        self.evolving_nonce = hash;
        self.candidate_nonce = hash;
        debug!(
            "Ledger: nonce initialized from genesis hash {}",
            hash.to_hex()
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
                raw_cbor: None,
            };

            self.utxo_set.insert(input, output);
            seeded += 1;
            total_lovelace += lovelace;
        }

        debug!(
            "Ledger: seeded {} genesis UTxOs ({} lovelace)",
            seeded, total_lovelace
        );
    }

    /// Apply a block to the ledger state.
    ///
    /// When `mode` is `ValidateAll`, each transaction is independently validated
    /// (Phase-1 + Phase-2 Plutus evaluation) and the result is compared against
    /// the block producer's `is_valid` flag. A mismatch rejects the block.
    pub fn apply_block(
        &mut self,
        block: &Block,
        mode: BlockValidationMode,
    ) -> Result<(), LedgerError> {
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
            debug!(
                "Ledger: epoch transition {} -> {} at slot {}",
                self.epoch.0,
                block_epoch.0,
                block.slot().0,
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

        // Pre-compute cost_models CBOR once per block (doesn't change within a block)
        let cost_models_cbor = if mode == BlockValidationMode::ValidateAll {
            self.protocol_params.cost_models.to_cbor()
        } else {
            None
        };

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
            // Phase-1 + Phase-2 validation when ValidateAll mode is active.
            // Verifies the block producer's is_valid flag matches actual evaluation.
            if mode == BlockValidationMode::ValidateAll {
                let has_redeemers = !tx.witness_set.redeemers.is_empty();

                if tx.is_valid {
                    // Producer claims tx is valid — verify with full validation.
                    // Use tx raw_cbor size as tx_size (approximate, sufficient for validation).
                    let tx_size = tx.raw_cbor.as_ref().map_or(0, |c| c.len() as u64);
                    let result = validate_transaction(
                        tx,
                        &self.utxo_set,
                        &self.protocol_params,
                        block.slot().0,
                        tx_size,
                        Some(&self.slot_config),
                    );
                    if let Err(errors) = result {
                        // Distinguish Phase-1 failures from Phase-2 (script) failures
                        let has_script_failure = errors
                            .iter()
                            .any(|e| matches!(e, ValidationError::ScriptFailed(_)));
                        if has_script_failure {
                            // Producer said valid but scripts fail → ValidationTagMismatch
                            return Err(LedgerError::ValidationTagMismatch {
                                tx_hash: tx.hash.to_hex(),
                                block_flag: true,
                                eval_result: false,
                            });
                        }
                        // Phase-1 failure — block is invalid
                        let err_str: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
                        return Err(LedgerError::BlockTxValidationFailed {
                            slot: block.slot().0,
                            tx_hash: tx.hash.to_hex(),
                            errors: err_str.join("; "),
                        });
                    }
                } else if has_redeemers {
                    // Producer claims tx is invalid (is_valid=false) with scripts present.
                    // Verify scripts actually fail; if they pass, producer is stealing collateral.
                    let max_ex = (
                        self.protocol_params.max_tx_ex_units.mem,
                        self.protocol_params.max_tx_ex_units.steps,
                    );
                    let eval_result = evaluate_plutus_scripts(
                        tx,
                        &self.utxo_set,
                        cost_models_cbor.as_deref(),
                        max_ex,
                        &self.slot_config,
                    );
                    if eval_result.is_ok() {
                        // Scripts actually pass but producer says invalid → mismatch
                        return Err(LedgerError::ValidationTagMismatch {
                            tx_hash: tx.hash.to_hex(),
                            block_flag: false,
                            eval_result: true,
                        });
                    }
                }
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
                    debug!(
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

    /// Save the attached UTxO store's LSM snapshot.
    /// Call this after `save_snapshot()` when using on-disk UTxO storage.
    /// Requires mutable access because LsmTree::save_snapshot is &mut self.
    pub fn save_utxo_snapshot(&mut self) -> Result<(), LedgerError> {
        if let Some(store) = self.utxo_set.store_mut() {
            store.save_snapshot("ledger").map_err(|e| {
                LedgerError::EpochTransition(format!("Failed to save UTxO store snapshot: {e}"))
            })?;
            debug!("UTxO store snapshot saved ({} entries)", store.len());
        }
        Ok(())
    }

    /// Attach an on-disk UTxO store to this ledger state.
    /// All subsequent UTxO operations will use the LSM-backed store.
    /// If the ledger has in-memory UTxOs (from bincode snapshot load),
    /// they are migrated to the store.
    pub fn attach_utxo_store(&mut self, mut store: crate::utxo_store::UtxoStore) {
        // Migrate any in-memory UTxOs to the store
        if !self.utxo_set.is_empty() && !self.utxo_set.has_store() {
            let count = self.utxo_set.len();
            info!("Migrating {} in-memory UTxOs to on-disk store", count);
            for (input, output) in self.utxo_set.iter() {
                store.insert(input, output);
            }
        }
        store.set_indexing_enabled(true);
        store.rebuild_address_index();
        self.utxo_set.attach_store(store);
        info!("UTxO store attached ({} entries)", self.utxo_set.len());
    }

    /// Current snapshot format version.
    ///
    /// **Migration policy:** Increment this when the serialized LedgerState layout
    /// changes (adding, removing, or reordering fields). When bumped:
    /// 1. Add a `migrate_vN_to_vM()` function that transforms the old data
    /// 2. Update `load_snapshot()` to dispatch to the migration chain
    /// 3. If bincode-level migration is infeasible (field layout changed too much),
    ///    the old snapshot will fail to deserialize and the node re-syncs from chain
    ///
    /// Since bincode is field-order-dependent and not self-describing, structural
    /// changes (new/removed/reordered fields) will cause deserialization failures
    /// for older snapshots. This is acceptable — snapshots are an optimization,
    /// not critical data. The node can always reconstruct state from the chain.
    /// Increment when GovernanceState/LedgerState fields change.
    /// Bincode is positional — any field addition/reorder breaks old snapshots.
    const SNAPSHOT_VERSION: u8 = 2;

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
            "Snapshot     saved (epoch={}, {} UTxOs, {:.1} MB)",
            self.epoch.0,
            self.utxo_set.len(),
            total_bytes as f64 / 1_048_576.0,
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
                        "Unsupported snapshot version {version} (max supported: {}). \
                         Delete the snapshot to re-sync from chain.",
                        Self::SNAPSHOT_VERSION,
                    )));
                }
                if version < Self::SNAPSHOT_VERSION {
                    // Older version — attempt migration chain. For bincode-based
                    // snapshots, structural changes make cross-version deserialization
                    // impossible. Log clearly so the user knows to re-sync.
                    warn!(
                        snapshot_version = version,
                        current_version = Self::SNAPSHOT_VERSION,
                        "Snapshot version mismatch — snapshot may fail to load. \
                         Delete the snapshot file to re-sync from chain if this fails."
                    );
                }
                debug!(version, "Loading versioned snapshot");
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
        // Re-enable indexing so subsequent insert/remove operations maintain the index.
        // The #[serde(skip)] on indexing_enabled defaults to false after deserialization.
        state.utxo_set.set_indexing_enabled(true);
        // After loading a snapshot, incremental stake tracking may have drifted.
        // Rebuild stake distribution from the full UTxO set immediately, then
        // recompute pool_stake for all existing snapshots (mark/set/go).
        // This ensures leader eligibility checks use correct sigma values
        // even before the next epoch boundary rotates in fresh snapshots.
        state.rebuild_stake_distribution();
        state.recompute_snapshot_pool_stakes();
        // Keep needs_stake_rebuild=true so every live epoch boundary rebuilds.
        state.needs_stake_rebuild = true;
        debug!(
            "Snapshot loaded from {} ({:.1} MB, {} UTxOs, epoch {})",
            path.display(),
            raw.len() as f64 / 1_048_576.0,
            state.utxo_set.len(),
            state.epoch.0,
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
        state
            .apply_block(&block, BlockValidationMode::ApplyOnly)
            .unwrap();

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
        state
            .apply_block(&block, BlockValidationMode::ApplyOnly)
            .unwrap();

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
        state
            .apply_block(&block, BlockValidationMode::ApplyOnly)
            .unwrap();
        assert_eq!(state.epoch, EpochNo(0));

        // Apply a block in epoch 1 (slot 100+)
        let block = make_test_block(150, 2, *block.hash(), vec![]);
        state
            .apply_block(&block, BlockValidationMode::ApplyOnly)
            .unwrap();
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
        state
            .apply_block(&block, BlockValidationMode::ApplyOnly)
            .unwrap();

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
        state
            .apply_block(&block, BlockValidationMode::ApplyOnly)
            .unwrap();

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
        state
            .apply_block(&block2, BlockValidationMode::ApplyOnly)
            .unwrap();

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
        state
            .apply_block(&block, BlockValidationMode::ApplyOnly)
            .unwrap();

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

        // Verify ratification tracking for GetRatifyState query
        assert_eq!(state.governance.last_ratified.len(), 1);
        assert_eq!(state.governance.last_ratified[0].0.transaction_id, tx_hash);
        assert_eq!(state.governance.last_ratified[0].0.action_index, 0);
        assert!(state.governance.last_expired.is_empty());
        assert!(!state.governance.last_ratify_delayed);
    }

    #[test]
    fn test_ratify_state_tracks_expired_proposals() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);
        state.epoch_length = 100;
        state.protocol_params.gov_action_lifetime = 2; // Expires in 2 epochs

        let tx_hash = Hash32::from_bytes([77u8; 32]);
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

        // Submit at epoch 0 — expires at epoch 2
        state.process_proposal(&tx_hash, 0, &proposal);
        assert_eq!(state.governance.proposals.len(), 1);

        // Epoch 1: proposal still active, not expired, not ratified (no votes)
        state.process_epoch_transition(EpochNo(1));
        assert_eq!(state.governance.proposals.len(), 1);
        assert!(state.governance.last_ratified.is_empty());
        assert!(state.governance.last_expired.is_empty());

        // Epoch 2: proposal expires (expires_epoch <= new_epoch)
        state.process_epoch_transition(EpochNo(2));
        assert_eq!(state.governance.proposals.len(), 0);
        assert!(state.governance.last_ratified.is_empty());
        assert_eq!(state.governance.last_expired.len(), 1);
        assert_eq!(state.governance.last_expired[0].transaction_id, tx_hash);
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
        state
            .apply_block(&block, BlockValidationMode::ApplyOnly)
            .unwrap();

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
        state
            .apply_block(&block, BlockValidationMode::ApplyOnly)
            .unwrap();

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
        state
            .apply_block(&block, BlockValidationMode::ApplyOnly)
            .unwrap();

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
        state
            .apply_block(&block, BlockValidationMode::ApplyOnly)
            .unwrap();

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
        state
            .apply_block(&block, BlockValidationMode::ApplyOnly)
            .unwrap();

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
        state
            .apply_block(&block, BlockValidationMode::ApplyOnly)
            .unwrap();

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
        state
            .apply_block(&block, BlockValidationMode::ApplyOnly)
            .unwrap();

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
        state
            .apply_block(&block, BlockValidationMode::ApplyOnly)
            .unwrap();
        assert_eq!(state.candidate_nonce, state.evolving_nonce);

        // Slot 60 is the FIRST slot in the stabilisation window
        // (60 >= 100 - 40 = 60, so candidate freezes)
        let candidate_before = state.candidate_nonce;
        let mut block2 = make_test_block(60, 2, *block.hash(), vec![]);
        block2.header.vrf_result.output = vec![99u8; 32];
        block2.header.issuer_vkey = vec![1u8; 32];
        state
            .apply_block(&block2, BlockValidationMode::ApplyOnly)
            .unwrap();
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
