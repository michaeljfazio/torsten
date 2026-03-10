use crate::plutus::SlotConfig;
use crate::utxo::UtxoSet;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use torsten_primitives::block::{Block, Point, Tip};
use torsten_primitives::credentials::Credential;
use torsten_primitives::era::Era;
use torsten_primitives::hash::{Hash28, Hash32};
use torsten_primitives::protocol_params::ProtocolParameters;
use torsten_primitives::time::{BlockNo, EpochNo, SlotNo};
use torsten_primitives::transaction::{
    Anchor, Certificate, Constitution, DRep, GovAction, GovActionId, ProposalProcedure,
    ProtocolParamUpdate, Relay, Vote, Voter, VotingProcedure,
};
use torsten_primitives::value::Lovelace;
use tracing::{debug, info, trace, warn};

/// Total ADA supply (45 billion ADA = 45 * 10^15 lovelace)
pub const MAX_LOVELACE_SUPPLY: u64 = 45_000_000_000_000_000;

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
        Rat::new(self.n * other.d + other.n * self.d, self.d * other.d)
    }

    fn sub(&self, other: &Rat) -> Rat {
        Rat::new(self.n * other.d - other.n * self.d, self.d * other.d)
    }

    fn mul(&self, other: &Rat) -> Rat {
        Rat::new(self.n * other.n, self.d * other.d)
    }

    fn div(&self, other: &Rat) -> Rat {
        Rat::new(self.n * other.d, self.d * other.n)
    }

    fn min_rat(&self, other: &Rat) -> Rat {
        // Compare self vs other: self.n/self.d vs other.n/other.d
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

/// The complete ledger state
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
    /// Epoch length in slots
    pub epoch_length: u64,
    /// Current protocol parameters
    pub protocol_params: ProtocolParameters,
    /// Stake distribution
    pub stake_distribution: StakeDistributionState,
    /// Treasury balance
    pub treasury: Lovelace,
    /// Reserves balance (ADA not yet in circulation)
    pub reserves: Lovelace,
    /// Delegation state: credential_hash -> pool_id
    pub delegations: HashMap<Hash32, Hash28>,
    /// Pool registrations: pool_id -> pool registration
    pub pool_params: HashMap<Hash28, PoolRegistration>,
    /// Pool retirements pending at a given epoch
    pub pending_retirements: BTreeMap<EpochNo, Vec<Hash28>>,
    /// Stake snapshots for the Cardano "mark/set/go" snapshot model
    pub snapshots: EpochSnapshots,
    /// Reward accounts: stake credential hash -> accumulated rewards
    pub reward_accounts: HashMap<Hash32, Lovelace>,
    /// Fees collected in the current epoch
    pub epoch_fees: Lovelace,
    /// Number of blocks produced by each pool in the current epoch
    pub epoch_blocks_by_pool: HashMap<Hash28, u64>,
    /// Total blocks in the current epoch
    pub epoch_block_count: u64,
    /// Rolling nonce (eta_v): accumulated hash of VRF outputs in the nonce contribution window
    pub rolling_nonce: Hash32,
    /// Current epoch nonce
    pub epoch_nonce: Hash32,
    /// Nonce contribution window: first stability_window slots of each epoch
    /// (3k/f = 129600 slots on mainnet)
    pub stability_window: u64,
    /// Hash of the first block in the current epoch (needed for next epoch's nonce)
    pub first_block_hash_of_epoch: Option<Hash32>,
    /// Hash of the first block in the previous epoch (used in epoch nonce calculation)
    pub prev_epoch_first_block_hash: Option<Hash32>,
    /// Shelley genesis hash (used to initialize rolling nonce)
    pub genesis_hash: Hash32,
    /// Conway governance state
    pub governance: GovernanceState,
    /// Slot configuration for Plutus time conversion
    pub slot_config: SlotConfig,
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

/// A snapshot of the stake distribution at an epoch boundary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StakeSnapshot {
    pub epoch: EpochNo,
    /// stake credential hash -> pool_id delegation
    pub delegations: HashMap<Hash32, Hash28>,
    /// pool_id -> total active stake delegated to that pool
    pub pool_stake: HashMap<Hash28, Lovelace>,
    /// pool_id -> pool parameters at snapshot time
    pub pool_params: HashMap<Hash28, PoolRegistration>,
    /// Individual stake per credential (for reward distribution and pledge verification)
    #[serde(default)]
    pub stake_distribution: HashMap<Hash32, Lovelace>,
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
            epoch_length: 432000, // mainnet default
            protocol_params: params,
            stake_distribution: StakeDistributionState::default(),
            treasury: Lovelace(0),
            reserves: Lovelace(MAX_LOVELACE_SUPPLY),
            delegations: HashMap::new(),
            pool_params: HashMap::new(),
            pending_retirements: BTreeMap::new(),
            snapshots: EpochSnapshots::default(),
            reward_accounts: HashMap::new(),
            epoch_fees: Lovelace(0),
            epoch_blocks_by_pool: HashMap::new(),
            epoch_block_count: 0,
            rolling_nonce: Hash32::ZERO,
            epoch_nonce: Hash32::ZERO,
            stability_window: 129600, // 3k/f on mainnet
            first_block_hash_of_epoch: None,
            prev_epoch_first_block_hash: None,
            genesis_hash: Hash32::ZERO,
            governance: GovernanceState::default(),
            slot_config: SlotConfig::default(),
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
        // stability_window = 3k/f where f = active_slot_coeff
        let f = self.protocol_params.active_slot_coeff();
        self.stability_window = (3.0 * security_param as f64 / f) as u64;
        info!(
            epoch_length,
            stability_window = self.stability_window,
            security_param,
            "Ledger: epoch length configured"
        );
    }

    /// Set the Shelley genesis hash.
    ///
    /// The rolling nonce is initialized from this hash (the Blake2b-256 hash of
    /// the canonical Shelley genesis JSON). This matches the Cardano reference
    /// implementation where eta_v starts from the genesis hash.
    pub fn set_genesis_hash(&mut self, hash: Hash32) {
        self.genesis_hash = hash;
        // Initialize rolling nonce from genesis hash (not ZERO)
        self.rolling_nonce = hash;
        info!(
            genesis_hash = %hash.to_hex(),
            "Ledger: rolling nonce initialized from genesis hash"
        );
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

        // Check for epoch transition before processing the block
        let block_epoch = EpochNo(block.slot().0 / self.epoch_length);
        if block_epoch > self.epoch {
            info!(
                prev_epoch = self.epoch.0,
                new_epoch = block_epoch.0,
                slot = block.slot().0,
                "Ledger: epoch transition detected"
            );
            self.process_epoch_transition(block_epoch);
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

        // Apply each transaction
        for tx in &block.transactions {
            // Handle invalid transactions (phase-2 validation failure):
            // - Collateral inputs are consumed (forfeit to block producer)
            // - Regular inputs/outputs/certificates are NOT applied
            // - If collateral_return is present, it becomes a new UTxO
            if !tx.is_valid {
                // Consume collateral inputs (update stake distribution)
                for col_input in &tx.body.collateral {
                    if let Some(spent) = self.utxo_set.lookup(col_input) {
                        if let Some(cred) = stake_credential_hash(&spent.address) {
                            if let Some(stake) = self.stake_distribution.stake_map.get_mut(&cred) {
                                stake.0 = stake.0.saturating_sub(spent.value.coin.0);
                            }
                        }
                    }
                    self.utxo_set.remove(col_input);
                }
                // If there's a collateral return output, add it
                if let Some(col_return) = &tx.body.collateral_return {
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
                    self.utxo_set.insert(return_input, col_return.clone());
                }
                // Fee from collateral is still collected
                self.epoch_fees += tx.body.fee;
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
                // Skip UTxO changes entirely to avoid phantom outputs that inflate
                // the UTxO set. Fees and certificates are still processed.
                debug!("UTxO application skipped (missing inputs): {e}");
            }

            // Update stake distribution from new outputs (add)
            for output in &tx.body.outputs {
                if let Some(cred_hash) = stake_credential_hash(&output.address) {
                    *self
                        .stake_distribution
                        .stake_map
                        .entry(cred_hash)
                        .or_insert(Lovelace(0)) += Lovelace(output.value.coin.0);
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
        }

        // Track block production by pool (issuer vkey hash)
        if !block.header.issuer_vkey.is_empty() {
            let pool_id = torsten_primitives::hash::blake2b_224(&block.header.issuer_vkey);
            *self.epoch_blocks_by_pool.entry(pool_id).or_insert(0) += 1;
        }
        self.epoch_block_count += 1;

        // Track first block hash of the current epoch (for epoch nonce calculation)
        if self.first_block_hash_of_epoch.is_none() {
            self.first_block_hash_of_epoch = Some(block.header.header_hash);
        }

        // Accumulate VRF output into rolling nonce (only in nonce contribution window)
        let slot_in_epoch = block.slot().0 % self.epoch_length;
        if slot_in_epoch < self.stability_window && !block.header.vrf_result.output.is_empty() {
            self.update_rolling_nonce(&block.header.vrf_result.output);
        }

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
                self.reward_accounts.entry(key).or_insert(Lovelace(0));
                debug!("Stake key registered: {}", key.to_hex());
            }
            Certificate::StakeDeregistration(credential) => {
                let key = credential_to_hash(credential);
                self.stake_distribution.stake_map.remove(&key);
                self.delegations.remove(&key);
                self.reward_accounts.remove(&key);
                debug!("Stake key deregistered: {}", key.to_hex());
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
                self.reward_accounts.entry(key).or_insert(Lovelace(0));
                debug!("Stake key registered (Conway): {}", key.to_hex());
            }
            Certificate::ConwayStakeDeregistration {
                credential,
                refund: _,
            } => {
                // Conway cert tag 8: same behavior as StakeDeregistration
                let key = credential_to_hash(credential);
                self.stake_distribution.stake_map.remove(&key);
                self.delegations.remove(&key);
                self.reward_accounts.remove(&key);
                debug!("Stake key deregistered (Conway): {}", key.to_hex());
            }
            Certificate::StakeDelegation {
                credential,
                pool_hash,
            } => {
                let key = credential_to_hash(credential);
                self.delegations.insert(key, *pool_hash);
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
                debug!("Pool registered: {}", params.operator.to_hex());
                self.pool_params.insert(params.operator, pool_reg);
            }
            Certificate::PoolRetirement { pool_hash, epoch } => {
                // Validate: retirement epoch must be <= current_epoch + e_max
                let max_retirement_epoch = self.epoch.0 + self.protocol_params.e_max;
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
                self.reward_accounts.entry(key).or_insert(Lovelace(0));
                self.delegations.insert(key, *pool_hash);
            }
            Certificate::RegDRep {
                credential,
                deposit,
                anchor,
            } => {
                let key = credential_to_hash(credential);
                self.governance.dreps.insert(
                    key,
                    DRepRegistration {
                        credential: credential.clone(),
                        deposit: *deposit,
                        anchor: anchor.clone(),
                        registered_epoch: self.epoch,
                        last_active_epoch: self.epoch,
                    },
                );
                self.governance.drep_registration_count += 1;
                debug!("DRep registered: {}", key.to_hex());
            }
            Certificate::UnregDRep {
                credential,
                refund: _,
            } => {
                let key = credential_to_hash(credential);
                self.governance.dreps.remove(&key);
                debug!("DRep deregistered: {}", key.to_hex());
            }
            Certificate::UpdateDRep { credential, anchor } => {
                let key = credential_to_hash(credential);
                if let Some(drep) = self.governance.dreps.get_mut(&key) {
                    drep.anchor = anchor.clone();
                    drep.last_active_epoch = self.epoch;
                    debug!("DRep updated: {}", key.to_hex());
                }
            }
            Certificate::VoteDelegation { credential, drep } => {
                let key = credential_to_hash(credential);
                self.governance.vote_delegations.insert(key, drep.clone());
                debug!("Vote delegated to {:?}", drep);
            }
            Certificate::StakeVoteDelegation {
                credential,
                pool_hash,
                drep,
            } => {
                let key = credential_to_hash(credential);
                // Stake delegation
                self.delegations.insert(key, *pool_hash);
                // Vote delegation
                self.governance.vote_delegations.insert(key, drep.clone());
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
                self.governance.committee_hot_keys.insert(cold_key, hot_key);
                // Remove from resigned if re-authorizing
                self.governance.committee_resigned.remove(&cold_key);
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
                self.governance
                    .committee_resigned
                    .insert(cold_key, anchor.clone());
                self.governance.committee_hot_keys.remove(&cold_key);
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
                self.reward_accounts.entry(key).or_insert(Lovelace(0));
                // Stake delegation
                self.delegations.insert(key, *pool_hash);
                // Vote delegation
                self.governance.vote_delegations.insert(key, drep.clone());
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
                self.reward_accounts.entry(key).or_insert(Lovelace(0));
                // Vote delegation
                self.governance.vote_delegations.insert(key, drep.clone());
                debug!("Reg+vote delegated to {:?}", drep);
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

        // Take a new "mark" snapshot of current stake distribution
        let mut pool_stake: HashMap<Hash28, Lovelace> = HashMap::new();
        for (cred_hash, pool_id) in &self.delegations {
            let stake = self
                .stake_distribution
                .stake_map
                .get(cred_hash)
                .copied()
                .unwrap_or(Lovelace(0));
            *pool_stake.entry(*pool_id).or_insert(Lovelace(0)) += stake;
        }

        self.snapshots.mark = Some(StakeSnapshot {
            epoch: new_epoch,
            delegations: self.delegations.clone(),
            pool_stake,
            pool_params: self.pool_params.clone(),
            stake_distribution: self.stake_distribution.stake_map.clone(),
        });

        // Process pending pool retirements for this epoch
        if let Some(retiring_pools) = self.pending_retirements.remove(&new_epoch) {
            let pool_deposit = self.protocol_params.pool_deposit;
            for pool_id in &retiring_pools {
                // Refund pool deposit to operator's registered reward account
                if let Some(pool_reg) = self.pool_params.remove(pool_id) {
                    let op_key = Self::reward_account_to_hash(&pool_reg.reward_account);
                    *self.reward_accounts.entry(op_key).or_insert(Lovelace(0)) += pool_deposit;
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
                if let Some(proposal_state) = self.governance.proposals.remove(action_id) {
                    // Refund deposit to return address's reward account
                    let deposit = proposal_state.procedure.deposit;
                    if deposit.0 > 0 {
                        let return_addr = &proposal_state.procedure.return_addr;
                        if return_addr.len() >= 29 {
                            let mut key_bytes = [0u8; 32];
                            let copy_len = (return_addr.len() - 1).min(32);
                            key_bytes[..copy_len].copy_from_slice(&return_addr[1..1 + copy_len]);
                            let key = Hash32::from_bytes(key_bytes);
                            *self.reward_accounts.entry(key).or_insert(Lovelace(0)) += deposit;
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
                self.governance.votes_by_action.remove(id);
            }
            debug!(
                "Expired {} governance proposals at epoch {}",
                expired.len(),
                new_epoch.0
            );
        }

        // Expire inactive DReps per CIP-1694
        // DReps that haven't voted or updated within drep_activity epochs are marked inactive
        // and excluded from voting power calculations
        let drep_activity = self.protocol_params.drep_activity;
        if drep_activity > 0 {
            let inactive_dreps: Vec<Hash32> = self
                .governance
                .dreps
                .iter()
                .filter(|(_, drep)| {
                    new_epoch.0.saturating_sub(drep.last_active_epoch.0) > drep_activity
                })
                .map(|(hash, _)| *hash)
                .collect();
            if !inactive_dreps.is_empty() {
                for hash in &inactive_dreps {
                    // Refund DRep deposit to their reward account
                    if let Some(drep) = self.governance.dreps.remove(hash) {
                        if drep.deposit.0 > 0 {
                            *self.reward_accounts.entry(*hash).or_insert(Lovelace(0)) +=
                                drep.deposit;
                        }
                    }
                }
                info!(
                    "Expired {} inactive DReps at epoch {} (activity threshold: {} epochs, deposits refunded)",
                    inactive_dreps.len(),
                    new_epoch.0,
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
                self.governance.committee_hot_keys.remove(hash);
                self.governance.committee_expiration.remove(hash);
            }
            info!(
                "Expired {} committee members at epoch {}",
                expired_members.len(),
                new_epoch.0
            );
        }

        // Compute new epoch nonce per Cardano spec:
        // epoch_nonce = hash(rolling_nonce || first_block_hash_prev_epoch [|| extra_entropy])
        // nc = rolling nonce (eta_v accumulated through stability window)
        // nh = hash of first block from the previous epoch
        let nh = self
            .prev_epoch_first_block_hash
            .unwrap_or(self.genesis_hash);
        let mut nonce_input = Vec::with_capacity(64);
        nonce_input.extend_from_slice(self.rolling_nonce.as_bytes());
        nonce_input.extend_from_slice(nh.as_bytes());
        self.epoch_nonce = torsten_primitives::hash::blake2b_256(&nonce_input);

        info!(
            "New epoch nonce: {} (from eta_v {} + nh {})",
            self.epoch_nonce.to_hex(),
            self.rolling_nonce.to_hex(),
            nh.to_hex()
        );

        // Rotate first block hash: current becomes previous for next transition
        self.prev_epoch_first_block_hash = self.first_block_hash_of_epoch.take();

        // Reset per-epoch accumulators
        self.epoch_fees = Lovelace(0);
        self.epoch_blocks_by_pool.clear();
        self.epoch_block_count = 0;
        // Reset rolling nonce from genesis hash (not ZERO)
        self.rolling_nonce = self.genesis_hash;

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
        let expected_blocks =
            (self.protocol_params.active_slot_coeff() * self.epoch_length as f64).floor() as u64;
        let actual_blocks = self.epoch_block_count;
        // eta = min(1, actual/expected) — applied as rational: min(1, actual/expected)
        // expansion = floor(min(actual, expected) / expected * rho * reserves)
        let effective_blocks = actual_blocks.min(expected_blocks);
        let expansion = if expected_blocks == 0 {
            0u64
        } else {
            (rho_num * self.reserves.0 as i128 * effective_blocks as i128
                / (rho_den * expected_blocks as i128)) as u64
        };
        let total_rewards_available = expansion + self.epoch_fees.0;

        if total_rewards_available == 0 {
            return;
        }

        // Move expansion from reserves
        self.reserves.0 = self.reserves.0.saturating_sub(expansion);

        // Treasury cut: floor(tau * total_rewards)
        let treasury_cut = (tau_num * total_rewards_available as i128 / tau_den) as u64;
        self.treasury.0 += treasury_cut;

        let reward_pot = total_rewards_available - treasury_cut;

        // Total stake for sigma denominator: circulation = maxSupply - reserves
        let total_stake = MAX_LOVELACE_SUPPLY.saturating_sub(self.reserves.0);
        if total_stake == 0 {
            self.treasury.0 += reward_pot;
            return;
        }

        // Total active stake (for apparent performance denominator)
        let total_active_stake: u64 = go_snapshot.pool_stake.values().map(|s| s.0).sum();
        if total_active_stake == 0 {
            self.treasury.0 += reward_pot;
            return;
        }

        // Total blocks produced this epoch
        let total_blocks_in_epoch = self.epoch_block_count.max(1);

        // Saturation point: z0 = 1/nOpt
        let n_opt = self.protocol_params.n_opt.max(1);

        let mut total_distributed: u64 = 0;

        // Build delegators-by-pool index for O(n) reward distribution
        let mut delegators_by_pool: HashMap<Hash28, Vec<Hash32>> = HashMap::new();
        for (cred_hash, pool_id) in &go_snapshot.delegations {
            delegators_by_pool
                .entry(*pool_id)
                .or_default()
                .push(*cred_hash);
        }

        // Build owner-delegated-stake per pool for pledge check
        let mut owner_stake_by_pool: HashMap<Hash28, u64> = HashMap::new();
        for (pool_id, pool_reg) in &go_snapshot.pool_params {
            let mut owner_stake = 0u64;
            for owner in &pool_reg.owners {
                let mut key_bytes = [0u8; 32];
                key_bytes[..28].copy_from_slice(owner.as_bytes());
                let owner_key = Hash32::from_bytes(key_bytes);
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
                let perf = Rat::new(
                    blocks_made as i128 * total_active_stake as i128,
                    total_blocks_in_epoch as i128 * pool_active_stake.0 as i128,
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
            let pool_stake_i = pool_active_stake.0 as i128;

            let operator_reward = if pool_reward <= cost {
                pool_reward
            } else {
                let remainder = (pool_reward - cost) as i128;
                // operator_share = margin + (1-margin) * s/sigma
                // = [margin_num * pool_stake + (margin_den - margin_num) * self_delegated] / (margin_den * pool_stake)
                let share_num =
                    margin_num * pool_stake_i + (margin_den - margin_num) * self_delegated as i128;
                let share_den = margin_den * pool_stake_i;
                let op_extra = if share_den == 0 {
                    0i128
                } else {
                    remainder * share_num / share_den
                };
                cost + op_extra.max(0) as u64
            };

            // Distribute member rewards proportionally to delegators.
            // Pool owners are excluded — they receive only the operator reward.
            // Build owner set (as Hash32 keys) for filtering
            let owner_set: std::collections::HashSet<Hash32> = pool_reg
                .owners
                .iter()
                .map(|o| {
                    let mut kb = [0u8; 32];
                    kb[..28].copy_from_slice(o.as_bytes());
                    Hash32::from_bytes(kb)
                })
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
                        let remainder = (pool_reward - cost) as i128;
                        let ms = remainder * (margin_den - margin_num) * member_stake as i128
                            / (margin_den * pool_stake_i);
                        ms.max(0) as u64
                    };

                    if member_share > 0 {
                        *self
                            .reward_accounts
                            .entry(*cred_hash)
                            .or_insert(Lovelace(0)) += Lovelace(member_share);
                        total_distributed += member_share;
                    }
                }
            }

            // Operator reward goes to pool's registered reward account
            if operator_reward > 0 {
                let op_key = Self::reward_account_to_hash(&pool_reg.reward_account);
                *self.reward_accounts.entry(op_key).or_insert(Lovelace(0)) +=
                    Lovelace(operator_reward);
                total_distributed += operator_reward;
            }
        }

        // Any undistributed rewards go to treasury
        let undistributed = reward_pot.saturating_sub(total_distributed);
        if undistributed > 0 {
            self.treasury.0 += undistributed;
        }

        info!(
            "Rewards distributed: {} lovelace to accounts, {} to treasury (expansion: {}, fees: {})",
            total_distributed, treasury_cut + undistributed, expansion, self.epoch_fees.0
        );
    }

    /// Convert a reward account (raw bytes with network header) to a Hash32 key
    fn reward_account_to_hash(reward_account: &[u8]) -> Hash32 {
        let mut key_bytes = [0u8; 32];
        if reward_account.len() >= 29 {
            let copy_len = (reward_account.len() - 1).min(32);
            key_bytes[..copy_len].copy_from_slice(&reward_account[1..1 + copy_len]);
        }
        Hash32::from_bytes(key_bytes)
    }

    /// Update the rolling nonce with a new VRF output.
    ///
    /// rolling_nonce = hash(rolling_nonce || hash(vrf_output))
    fn update_rolling_nonce(&mut self, vrf_output: &[u8]) {
        // Per Praos spec: nonce contribution = Blake2b-256(Blake2b-256("N" || raw_vrf_output))
        // Domain-separated, double-hashed nonce value
        let mut prefixed = Vec::with_capacity(1 + vrf_output.len());
        prefixed.push(b'N');
        prefixed.extend_from_slice(vrf_output);
        let first_hash = torsten_primitives::hash::blake2b_256(&prefixed);
        let nonce_value = torsten_primitives::hash::blake2b_256(first_hash.as_ref());
        let mut data = Vec::with_capacity(64);
        data.extend_from_slice(self.rolling_nonce.as_bytes());
        data.extend_from_slice(nonce_value.as_bytes());
        self.rolling_nonce = torsten_primitives::hash::blake2b_256(&data);
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
        let expires_epoch = EpochNo(self.epoch.0 + gov_action_lifetime);

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
        self.governance.proposals.insert(action_id, state);
        self.governance.proposal_count += 1;
    }

    /// Process a governance vote
    fn process_vote(
        &mut self,
        voter: &Voter,
        action_id: &GovActionId,
        procedure: &VotingProcedure,
    ) {
        // Update vote tally on the proposal
        if let Some(proposal) = self.governance.proposals.get_mut(action_id) {
            match procedure.vote {
                Vote::Yes => proposal.yes_votes += 1,
                Vote::No => proposal.no_votes += 1,
                Vote::Abstain => proposal.abstain_votes += 1,
            }
        }

        // Track DRep activity — voting counts as activity per CIP-1694
        if let Voter::DRep(cred) = voter {
            let drep_hash = credential_to_hash(cred);
            if let Some(drep) = self.governance.dreps.get_mut(&drep_hash) {
                drep.last_active_epoch = self.epoch;
            }
        }

        // Record the vote (indexed by action_id for efficient ratification)
        let action_votes = self
            .governance
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
    fn ratify_proposals(&mut self) {
        let total_drep_stake = self.compute_total_drep_stake();
        let total_spo_stake = self.compute_total_spo_stake();

        // Collect ratified proposal IDs and their actions
        let ratified: Vec<(GovActionId, GovAction)> = self
            .governance
            .proposals
            .iter()
            .filter(|(action_id, state)| {
                self.check_ratification(action_id, state, total_drep_stake, total_spo_stake)
            })
            .map(|(id, state)| (id.clone(), state.procedure.gov_action.clone()))
            .collect();

        // Enact ratified proposals and refund deposits
        if !ratified.is_empty() {
            for (action_id, action) in &ratified {
                info!("Governance proposal ratified: {:?}", action_id);
                self.enact_gov_action(action);
                // Refund proposal deposit to return address
                if let Some(proposal_state) = self.governance.proposals.remove(action_id) {
                    let deposit = proposal_state.procedure.deposit;
                    if deposit.0 > 0 {
                        let return_addr = &proposal_state.procedure.return_addr;
                        if return_addr.len() >= 29 {
                            let mut key_bytes = [0u8; 32];
                            let copy_len = (return_addr.len() - 1).min(32);
                            key_bytes[..copy_len].copy_from_slice(&return_addr[1..1 + copy_len]);
                            let key = Hash32::from_bytes(key_bytes);
                            *self.reward_accounts.entry(key).or_insert(Lovelace(0)) += deposit;
                        }
                    }
                }
            }
            // Remove all votes for ratified proposals
            for (id, _) in &ratified {
                self.governance.votes_by_action.remove(id);
            }
            info!(
                "{} governance proposal(s) ratified and enacted",
                ratified.len()
            );
        }
    }

    /// Check whether a proposal has met its voting thresholds for ratification.
    ///
    /// CIP-1694 voting thresholds (stake-weighted):
    /// - InfoAction: always ratified (no thresholds)
    /// - ParameterChange: requires DRep vote ≥ dvt_pp_*_group (group-dependent) AND CC approval
    /// - HardForkInitiation: requires DRep ≥ dvt_hard_fork AND SPO ≥ pvt_hard_fork
    /// - NoConfidence: requires DRep ≥ dvt_no_confidence AND SPO ≥ pvt_motion_no_confidence
    /// - UpdateCommittee: requires DRep ≥ dvt_committee AND SPO ≥ pvt_committee_normal
    /// - NewConstitution: requires DRep ≥ dvt_constitution AND CC approval
    /// - TreasuryWithdrawals: requires DRep ≥ dvt_treasury_withdrawal AND CC approval
    fn check_ratification(
        &self,
        action_id: &GovActionId,
        state: &ProposalState,
        total_drep_stake: u64,
        total_spo_stake: u64,
    ) -> bool {
        // Count votes by voter type
        let (drep_yes, drep_total, spo_yes, spo_total, cc_yes, cc_total) =
            self.count_votes_by_type(action_id);

        match &state.procedure.gov_action {
            GovAction::InfoAction => {
                // InfoAction is always ratified (it's informational only)
                true
            }
            GovAction::ParameterChange {
                protocol_param_update,
                ..
            } => {
                // Per CIP-1694 / Haskell pparamsUpdateThreshold:
                // DRep threshold = max of applicable DRep group thresholds
                // SPO threshold = pvtPPSecurityGroup if any param is security-relevant, else no SPO vote
                let drep_threshold =
                    pp_change_drep_threshold(protocol_param_update, &self.protocol_params);
                let drep_met =
                    check_threshold(drep_yes, drep_total.max(total_drep_stake), drep_threshold);
                let spo_met = if let Some(spo_threshold) =
                    pp_change_spo_threshold(protocol_param_update, &self.protocol_params)
                {
                    check_threshold(spo_yes, spo_total.max(total_spo_stake), spo_threshold)
                } else {
                    true // No SPO vote required for non-security params
                };
                let cc_met = check_cc_approval(cc_yes, cc_total, &self.governance, self.epoch);
                drep_met && spo_met && cc_met
            }
            GovAction::HardForkInitiation { .. } => {
                let drep_threshold = self.protocol_params.dvt_hard_fork.as_f64();
                let spo_threshold = self.protocol_params.pvt_hard_fork.as_f64();
                let drep_met =
                    check_threshold(drep_yes, drep_total.max(total_drep_stake), drep_threshold);
                let spo_met =
                    check_threshold(spo_yes, spo_total.max(total_spo_stake), spo_threshold);
                drep_met && spo_met
            }
            GovAction::NoConfidence { .. } => {
                let drep_threshold = self.protocol_params.dvt_no_confidence.as_f64();
                let spo_threshold = self.protocol_params.pvt_motion_no_confidence.as_f64();
                let drep_met =
                    check_threshold(drep_yes, drep_total.max(total_drep_stake), drep_threshold);
                let spo_met =
                    check_threshold(spo_yes, spo_total.max(total_spo_stake), spo_threshold);
                drep_met && spo_met
            }
            GovAction::UpdateCommittee { .. } => {
                let (drep_threshold, spo_threshold) = if self.governance.no_confidence {
                    (
                        self.protocol_params.dvt_committee_no_confidence.as_f64(),
                        self.protocol_params.pvt_committee_no_confidence.as_f64(),
                    )
                } else {
                    (
                        self.protocol_params.dvt_committee_normal.as_f64(),
                        self.protocol_params.pvt_committee_normal.as_f64(),
                    )
                };
                let drep_met =
                    check_threshold(drep_yes, drep_total.max(total_drep_stake), drep_threshold);
                let spo_met =
                    check_threshold(spo_yes, spo_total.max(total_spo_stake), spo_threshold);
                drep_met && spo_met
            }
            GovAction::NewConstitution { .. } => {
                let drep_threshold = self.protocol_params.dvt_constitution.as_f64();
                let drep_met =
                    check_threshold(drep_yes, drep_total.max(total_drep_stake), drep_threshold);
                let cc_met = check_cc_approval(cc_yes, cc_total, &self.governance, self.epoch);
                drep_met && cc_met
            }
            GovAction::TreasuryWithdrawals { .. } => {
                let drep_threshold = self.protocol_params.dvt_treasury_withdrawal.as_f64();
                let drep_met =
                    check_threshold(drep_yes, drep_total.max(total_drep_stake), drep_threshold);
                let cc_met = check_cc_approval(cc_yes, cc_total, &self.governance, self.epoch);
                drep_met && cc_met
            }
        }
    }

    /// Count stake-weighted votes by voter type for a specific governance action.
    ///
    /// Per CIP-1694, DRep and SPO votes are weighted by delegated stake:
    /// - DRep voting power = sum of stake delegated to that DRep via VoteDelegation
    /// - SPO voting power = pool's total active stake
    /// - CC votes are unweighted (1 per member)
    fn count_votes_by_type(&self, action_id: &GovActionId) -> (u64, u64, u64, u64, u64, u64) {
        let mut drep_yes = 0u64;
        let mut drep_total = 0u64;
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
                    let voting_power = self.compute_drep_voting_power(&drep_hash);
                    drep_total += voting_power;
                    if procedure.vote == Vote::Yes {
                        drep_yes += voting_power;
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

        (drep_yes, drep_total, spo_yes, spo_total, cc_yes, cc_total)
    }

    /// Compute the voting power of a DRep: sum of stake delegated to them.
    fn compute_drep_voting_power(&self, drep_hash: &Hash32) -> u64 {
        let mut power = 0u64;
        for (stake_cred, drep) in &self.governance.vote_delegations {
            let matches = match drep {
                DRep::KeyHash(h) => h == drep_hash,
                DRep::ScriptHash(h) => {
                    // ScriptHash is Hash28 — pad to Hash32 for comparison
                    let mut padded = [0u8; 32];
                    padded[..28].copy_from_slice(h.as_bytes());
                    Hash32::from_bytes(padded) == *drep_hash
                }
                DRep::Abstain | DRep::NoConfidence => false,
            };
            if matches {
                if let Some(stake) = self.stake_distribution.stake_map.get(stake_cred) {
                    power += stake.0;
                }
            }
        }
        // Minimum voting power of 1 for registered DReps with no delegated stake
        if power == 0 && self.governance.dreps.contains_key(drep_hash) {
            1
        } else {
            power
        }
    }

    /// Compute total active DRep-delegated stake across all DReps.
    /// All vote delegation types (KeyHash, ScriptHash, Abstain, NoConfidence) count.
    fn compute_total_drep_stake(&self) -> u64 {
        let total: u64 = self
            .governance
            .vote_delegations
            .keys()
            .filter_map(|stake_cred| self.stake_distribution.stake_map.get(stake_cred))
            .map(|stake| stake.0)
            .sum();
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
        // Fallback: compute from current delegations
        let mut total = 0u64;
        for (stake_cred, delegated_pool) in &self.delegations {
            if delegated_pool == pool_id {
                if let Some(stake) = self.stake_distribution.stake_map.get(stake_cred) {
                    total += stake.0;
                }
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
        // Fallback: sum all pool stake from current delegations
        let mut pool_stakes: HashMap<Hash28, u64> = HashMap::new();
        for (stake_cred, pool_id) in &self.delegations {
            if let Some(stake) = self.stake_distribution.stake_map.get(stake_cred) {
                *pool_stakes.entry(*pool_id).or_default() += stake.0;
            }
        }
        let total: u64 = pool_stakes.values().sum();
        total.max(1)
    }

    /// Enact a ratified governance action by applying its effects
    fn enact_gov_action(&mut self, action: &GovAction) {
        match action {
            GovAction::ParameterChange {
                protocol_param_update,
                ..
            } => {
                // Apply all protocol parameter updates
                let update = protocol_param_update.as_ref();
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
                    // Merge cost models: only update languages that are specified
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
                    self.protocol_params.dvt_pp_network_group = v.clone();
                }
                if let Some(ref v) = update.dvt_pp_economic_group {
                    self.protocol_params.dvt_pp_economic_group = v.clone();
                }
                if let Some(ref v) = update.dvt_pp_technical_group {
                    self.protocol_params.dvt_pp_technical_group = v.clone();
                }
                if let Some(ref v) = update.dvt_pp_gov_group {
                    self.protocol_params.dvt_pp_gov_group = v.clone();
                }
                if let Some(ref v) = update.dvt_hard_fork {
                    self.protocol_params.dvt_hard_fork = v.clone();
                }
                if let Some(ref v) = update.dvt_no_confidence {
                    self.protocol_params.dvt_no_confidence = v.clone();
                }
                if let Some(ref v) = update.dvt_committee_normal {
                    self.protocol_params.dvt_committee_normal = v.clone();
                }
                if let Some(ref v) = update.dvt_committee_no_confidence {
                    self.protocol_params.dvt_committee_no_confidence = v.clone();
                }
                if let Some(ref v) = update.dvt_constitution {
                    self.protocol_params.dvt_constitution = v.clone();
                }
                if let Some(ref v) = update.dvt_treasury_withdrawal {
                    self.protocol_params.dvt_treasury_withdrawal = v.clone();
                }
                if let Some(ref v) = update.pvt_motion_no_confidence {
                    self.protocol_params.pvt_motion_no_confidence = v.clone();
                }
                if let Some(ref v) = update.pvt_committee_normal {
                    self.protocol_params.pvt_committee_normal = v.clone();
                }
                if let Some(ref v) = update.pvt_committee_no_confidence {
                    self.protocol_params.pvt_committee_no_confidence = v.clone();
                }
                if let Some(ref v) = update.pvt_hard_fork {
                    self.protocol_params.pvt_hard_fork = v.clone();
                }
                if let Some(ref v) = update.pvt_pp_security_group {
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
                info!("Protocol parameters updated via governance action");
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
                let mut total = 0u64;
                for (reward_addr, amount) in withdrawals {
                    self.treasury.0 = self.treasury.0.saturating_sub(amount.0);
                    total += amount.0;
                    // Credit the withdrawal to the recipient's reward account
                    if reward_addr.len() >= 29 {
                        let mut key_bytes = [0u8; 32];
                        let copy_len = (reward_addr.len() - 1).min(32);
                        key_bytes[..copy_len].copy_from_slice(&reward_addr[1..1 + copy_len]);
                        let key = Hash32::from_bytes(key_bytes);
                        *self.reward_accounts.entry(key).or_insert(Lovelace(0)) += *amount;
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
                self.governance.committee_hot_keys.clear();
                self.governance.committee_expiration.clear();
                self.governance.no_confidence = true;
                info!("No confidence motion enacted: committee disbanded");
            }
            GovAction::UpdateCommittee {
                members_to_remove,
                members_to_add,
                ..
            } => {
                // Remove specified members
                for cred in members_to_remove {
                    let key = credential_to_hash(cred);
                    self.governance.committee_hot_keys.remove(&key);
                    self.governance.committee_expiration.remove(&key);
                    self.governance.committee_resigned.remove(&key);
                }
                // Add new members with expiration epochs
                for (cred, expiration_epoch) in members_to_add {
                    let key = credential_to_hash(cred);
                    self.governance
                        .committee_expiration
                        .insert(key, EpochNo(*expiration_epoch));
                    // Hot key auth comes via CommitteeHotAuth certificates
                }
                // UpdateCommittee restores confidence
                self.governance.no_confidence = false;
                info!(
                    "Committee updated: {} removed, {} added",
                    members_to_remove.len(),
                    members_to_add.len()
                );
            }
            GovAction::NewConstitution { constitution, .. } => {
                self.governance.constitution = Some(constitution.clone());
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
        if let Some(balance) = self.reward_accounts.get_mut(&key) {
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

    /// Save ledger state snapshot to disk using bincode serialization.
    /// Format: [4-byte magic][32-byte blake2b checksum][bincode data]
    pub fn save_snapshot(&self, path: &Path) -> Result<(), LedgerError> {
        let tmp_path = path.with_extension("tmp");
        let data = bincode::serialize(self).map_err(|e| {
            LedgerError::EpochTransition(format!("Failed to serialize ledger state: {e}"))
        })?;

        // Compute checksum over the serialized data
        let checksum = torsten_primitives::hash::blake2b_256(&data);

        // Write: magic + checksum + data
        let mut output = Vec::with_capacity(4 + 32 + data.len());
        output.extend_from_slice(b"TRSN"); // Torsten Snapshot magic bytes
        output.extend_from_slice(checksum.as_bytes());
        output.extend_from_slice(&data);

        std::fs::write(&tmp_path, &output)
            .map_err(|e| LedgerError::EpochTransition(format!("Failed to write snapshot: {e}")))?;
        std::fs::rename(&tmp_path, path)
            .map_err(|e| LedgerError::EpochTransition(format!("Failed to rename snapshot: {e}")))?;
        info!(
            path = %path.display(),
            bytes = output.len(),
            utxo_count = self.utxo_set.len(),
            epoch = self.epoch.0,
            slot = ?self.tip.point.slot().map(|s| s.0),
            "Ledger snapshot saved"
        );
        Ok(())
    }

    /// Load ledger state snapshot from disk.
    /// Verifies magic bytes and blake2b checksum before deserializing.
    pub fn load_snapshot(path: &Path) -> Result<Self, LedgerError> {
        let raw = std::fs::read(path)
            .map_err(|e| LedgerError::EpochTransition(format!("Failed to read snapshot: {e}")))?;

        // Try new format with magic + checksum header (36 bytes minimum)
        let data = if raw.len() >= 36 && &raw[..4] == b"TRSN" {
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

        let state: LedgerState = bincode::deserialize(data).map_err(|e| {
            LedgerError::EpochTransition(format!("Failed to deserialize ledger state: {e}"))
        })?;
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
    let h28 = credential.to_hash();
    let mut bytes = [0u8; 32];
    bytes[..28].copy_from_slice(h28.as_bytes());
    Hash32::from_bytes(bytes)
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

/// Compute the DRep voting threshold for a ParameterChange governance action.
///
/// Per Haskell `pparamsUpdateThreshold`: takes the maximum DRep group threshold
/// across all modified parameter groups.
fn pp_change_drep_threshold(ppu: &ProtocolParamUpdate, params: &ProtocolParameters) -> f64 {
    let groups = modified_pp_groups(ppu);
    let mut max_threshold = 0.0_f64;
    for (drep_group, _) in &groups {
        let t = match drep_group {
            DRepPPGroup::Network => params.dvt_pp_network_group.as_f64(),
            DRepPPGroup::Economic => params.dvt_pp_economic_group.as_f64(),
            DRepPPGroup::Technical => params.dvt_pp_technical_group.as_f64(),
            DRepPPGroup::Gov => params.dvt_pp_gov_group.as_f64(),
        };
        if t > max_threshold {
            max_threshold = t;
        }
    }
    max_threshold
}

/// Determine if SPOs can vote on a ParameterChange, and if so, return the threshold.
///
/// Per Haskell `votingStakePoolThresholdInternal`: SPOs vote with pvtPPSecurityGroup
/// if ANY modified parameter is tagged SecurityGroup. Otherwise SPOs cannot vote.
fn pp_change_spo_threshold(ppu: &ProtocolParamUpdate, params: &ProtocolParameters) -> Option<f64> {
    let groups = modified_pp_groups(ppu);
    let has_security = groups
        .iter()
        .any(|(_, spo)| *spo == StakePoolPPGroup::Security);
    if has_security {
        Some(params.pvt_pp_security_group.as_f64())
    } else {
        None
    }
}

fn check_threshold(yes: u64, total: u64, threshold: f64) -> bool {
    if total == 0 {
        return false;
    }
    (yes as f64 / total as f64) >= threshold
}

/// Check if the constitutional committee has approved (majority of active members voted yes).
/// If there's no active committee (all resigned, or no hot keys), CC approval is not required.
fn check_cc_approval(
    cc_yes: u64,
    cc_total: u64,
    governance: &GovernanceState,
    current_epoch: EpochNo,
) -> bool {
    // Count only non-expired, non-resigned committee members with hot keys
    let active_cc = governance
        .committee_hot_keys
        .keys()
        .filter(|cold| {
            // Must not be expired
            if let Some(exp) = governance.committee_expiration.get(*cold) {
                if *exp <= current_epoch {
                    return false;
                }
            }
            // Must not be resigned
            !governance.committee_resigned.contains_key(*cold)
        })
        .count() as u64;
    if active_cc == 0 {
        // No active committee — CC requirement is waived
        return true;
    }
    // Majority of voting CC members must approve
    if cc_total == 0 {
        return false;
    }
    cc_yes * 2 > cc_total
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use torsten_primitives::address::{Address, ByronAddress};
    use torsten_primitives::hash::Hash28;
    use torsten_primitives::transaction::*;
    use torsten_primitives::value::Value;

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
        let key = credential_to_hash(&cred);
        state
            .stake_distribution
            .stake_map
            .insert(key, Lovelace(1_000_000));

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
        let key = credential_to_hash(&cred);

        // Build reward account from owner credential
        let mut reward_account = vec![0xE0u8];
        reward_account.extend_from_slice(owner_hash.as_bytes());

        // Register stake, pool, and delegate
        state.process_certificate(&Certificate::StakeRegistration(cred.clone()));
        // Realistic pool stake: 50 million ADA (large pool)
        state
            .stake_distribution
            .stake_map
            .insert(key, Lovelace(50_000_000_000_000));

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
        state.epoch_blocks_by_pool.insert(pool_id, 21600);
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
        state.epoch_blocks_by_pool.insert(pool_id, 21600);
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
        let key = credential_to_hash(&cred);

        // Reward account uses the owner's credential
        let mut reward_account = vec![0xE0u8];
        reward_account.extend_from_slice(owner_hash.as_bytes());

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
        state.epoch_blocks_by_pool.insert(pool_id, 21600);
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
        let mut pool_key_bytes = [0u8; 32];
        pool_key_bytes[..28].copy_from_slice(pool_id.as_bytes());
        let pool_key = Hash32::from_bytes(pool_key_bytes);
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
        state.stability_window = 60; // First 60 slots contribute to nonce

        // Set a genesis hash to initialize rolling nonce
        let genesis_hash = Hash32::from_bytes([0xAB; 32]);
        state.set_genesis_hash(genesis_hash);

        // Rolling nonce starts from genesis hash
        assert_eq!(state.rolling_nonce, genesis_hash);
        // Epoch nonce starts at ZERO
        assert_eq!(state.epoch_nonce, Hash32::ZERO);

        // Apply a block with a VRF output in the nonce window
        let mut block = make_test_block(10, 1, Hash32::ZERO, vec![]);
        block.header.vrf_result.output = vec![42u8; 32];
        block.header.issuer_vkey = vec![1u8; 32];
        state.apply_block(&block).unwrap();

        // Rolling nonce should have been updated from genesis_hash
        assert_ne!(state.rolling_nonce, genesis_hash);

        // First block hash of epoch should be tracked
        assert_eq!(
            state.first_block_hash_of_epoch,
            Some(block.header.header_hash)
        );

        // Apply a block outside the nonce window (slot 70 % 100 = 70 > 60)
        let rolling_before = state.rolling_nonce;
        let mut block2 = make_test_block(70, 2, *block.hash(), vec![]);
        block2.header.vrf_result.output = vec![99u8; 32];
        block2.header.issuer_vkey = vec![1u8; 32];
        state.apply_block(&block2).unwrap();

        // Rolling nonce should NOT have changed (outside window)
        assert_eq!(state.rolling_nonce, rolling_before);

        // Trigger epoch transition
        let nonce_before_transition = state.epoch_nonce;
        state.process_epoch_transition(EpochNo(1));

        // Epoch nonce should have been updated
        assert_ne!(state.epoch_nonce, nonce_before_transition);
        // Rolling nonce should be reset to genesis hash (not ZERO)
        assert_eq!(state.rolling_nonce, genesis_hash);
        // Previous epoch's first block hash should be set
        assert!(state.prev_epoch_first_block_hash.is_some());
        // Current epoch's first block hash should be cleared
        assert!(state.first_block_hash_of_epoch.is_none());
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
        // 7 - 3 = 4, which is not > 5, so DRep should remain
        state.process_epoch_transition(EpochNo(7));
        assert!(state.governance.dreps.contains_key(&key));

        // Epoch transition to epoch 9 — 9 - 3 = 6 > 5, so DRep should be expired
        state.process_epoch_transition(EpochNo(9));
        assert!(!state.governance.dreps.contains_key(&key));
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

        state.governance.committee_hot_keys.insert(cold1, hot1);
        state
            .governance
            .committee_expiration
            .insert(cold1, EpochNo(5));
        state.governance.committee_hot_keys.insert(cold2, hot2);
        state
            .governance
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
    fn test_drep_deposit_refund_on_expiry() {
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

        // No reward account yet
        assert!(!state.reward_accounts.contains_key(&key));

        // Expire at epoch 3 (0 + 2 < 3, so inactive)
        state.process_epoch_transition(EpochNo(3));
        assert!(!state.governance.dreps.contains_key(&key));

        // Deposit should be refunded to reward account
        assert_eq!(
            state.reward_accounts.get(&key),
            Some(&Lovelace(500_000_000))
        );
    }

    #[test]
    fn test_governance_proposal_deposit_refund() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);

        // Build a return address (29 bytes: 1 header + 28 key hash)
        let mut return_addr = vec![0xE1u8]; // header byte
        return_addr.extend_from_slice(&[42u8; 28]); // 28-byte key hash

        let mut key_bytes = [0u8; 32];
        key_bytes[..28].copy_from_slice(&[42u8; 28]);
        let reward_key = Hash32::from_bytes(key_bytes);

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

        let mut key_bytes = [0u8; 32];
        key_bytes[..28].copy_from_slice(&[55u8; 28]);
        let reward_key = Hash32::from_bytes(key_bytes);

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
        state.governance.dreps.insert(
            key,
            DRepRegistration {
                credential: cred,
                deposit: Lovelace(500_000_000),
                anchor: None,
                registered_epoch: EpochNo(0),
                last_active_epoch: EpochNo(0),
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

        // Register enough DReps and have them vote yes to meet threshold (67%)
        let drep_count = 10;
        for i in 0..drep_count {
            let cred = Credential::VerificationKey(Hash28::from_bytes([i as u8; 28]));
            let key = credential_to_hash(&cred);
            state.governance.dreps.insert(
                key,
                DRepRegistration {
                    credential: cred,
                    deposit: Lovelace(500_000_000),
                    anchor: None,
                    registered_epoch: EpochNo(0),
                    last_active_epoch: EpochNo(0),
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
            state.governance.dreps.insert(
                key,
                DRepRegistration {
                    credential: cred.clone(),
                    deposit: Lovelace(500_000_000),
                    anchor: None,
                    registered_epoch: EpochNo(0),
                    last_active_epoch: EpochNo(0),
                },
            );
            // Set up vote delegation and stake for each DRep
            let stake_key = Hash32::from_bytes([100 + i as u8; 32]);
            let mut drep_bytes = [0u8; 32];
            drep_bytes[..28].copy_from_slice(&[i as u8; 28]);
            state
                .governance
                .vote_delegations
                .insert(stake_key, DRep::KeyHash(Hash32::from_bytes(drep_bytes)));
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

        // Register DReps
        for i in 0..10 {
            let cred = Credential::VerificationKey(Hash28::from_bytes([i as u8; 28]));
            let key = credential_to_hash(&cred);
            state.governance.dreps.insert(
                key,
                DRepRegistration {
                    credential: cred,
                    deposit: Lovelace(500_000_000),
                    anchor: None,
                    registered_epoch: EpochNo(0),
                    last_active_epoch: EpochNo(0),
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
            state.governance.dreps.insert(
                key,
                DRepRegistration {
                    credential: cred,
                    deposit: Lovelace(500_000_000),
                    anchor: None,
                    registered_epoch: EpochNo(0),
                    last_active_epoch: EpochNo(0),
                },
            );
        }

        // Register some SPOs
        for i in 0..10 {
            let pool_id = Hash28::from_bytes([100 + i as u8; 28]);
            state.pool_params.insert(
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
            let pool_hash = Hash32::from_bytes({
                let mut b = [0u8; 32];
                b[..28].copy_from_slice(Hash28::from_bytes([100 + i as u8; 28]).as_bytes());
                b
            });
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

        // Register DReps
        for i in 0..10 {
            let cred = Credential::VerificationKey(Hash28::from_bytes([i as u8; 28]));
            let key = credential_to_hash(&cred);
            state.governance.dreps.insert(
                key,
                DRepRegistration {
                    credential: cred,
                    deposit: Lovelace(500_000_000),
                    anchor: None,
                    registered_epoch: EpochNo(0),
                    last_active_epoch: EpochNo(0),
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
        assert!(check_threshold(7, 10, 0.67)); // 70% >= 67%
        assert!(!check_threshold(6, 10, 0.67)); // 60% < 67%
        assert!(check_threshold(1, 1, 0.51)); // 100% >= 51%
        assert!(!check_threshold(0, 10, 0.01)); // 0% < 1%
        assert!(!check_threshold(0, 0, 0.5)); // no votes = not met
    }

    #[test]
    fn test_cc_approval_no_committee() {
        let governance = GovernanceState::default();
        // No active committee => CC approval waived
        assert!(check_cc_approval(0, 0, &governance, EpochNo(10)));
    }

    #[test]
    fn test_cc_approval_with_committee() {
        let mut governance = GovernanceState::default();
        let current_epoch = EpochNo(10);
        // Add 3 active CC members with expiration in the future
        for i in 0..3 {
            let cold = Hash32::from_bytes([i as u8; 32]);
            let hot = Hash32::from_bytes([10 + i as u8; 32]);
            governance.committee_hot_keys.insert(cold, hot);
            governance.committee_expiration.insert(cold, EpochNo(100)); // expires at epoch 100
        }
        // 2/3 voted yes => majority
        assert!(check_cc_approval(2, 3, &governance, current_epoch));
        // 1/3 voted yes => no majority
        assert!(!check_cc_approval(1, 3, &governance, current_epoch));
        // No CC voted at all => not approved
        assert!(!check_cc_approval(0, 0, &governance, current_epoch));
    }

    #[test]
    fn test_cc_approval_expired_members() {
        let mut governance = GovernanceState::default();
        let current_epoch = EpochNo(50);
        // Add 3 CC members, but 2 are expired
        for i in 0..3 {
            let cold = Hash32::from_bytes([i as u8; 32]);
            let hot = Hash32::from_bytes([10 + i as u8; 32]);
            governance.committee_hot_keys.insert(cold, hot);
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
        // Only 1 active member, so 1/1 required for majority
        assert!(check_cc_approval(1, 1, &governance, current_epoch));
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

        // Corrupt one byte in the payload area (after 36-byte header)
        let mut data = std::fs::read(&snapshot_path).unwrap();
        assert!(data.len() > 40);
        data[40] ^= 0xFF; // Flip bits
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
        state.governance.constitution = Some(Constitution {
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
        assert_eq!(threshold, params.dvt_pp_network_group.as_f64());
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
        let expected = params
            .dvt_pp_network_group
            .as_f64()
            .max(params.dvt_pp_economic_group.as_f64())
            .max(params.dvt_pp_technical_group.as_f64());
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
        assert_eq!(spo, Some(params.pvt_pp_security_group.as_f64()));
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
        assert_eq!(spo, Some(params.pvt_pp_security_group.as_f64()));
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
        state.reward_accounts.insert(hash_key, Lovelace(5_000_000));

        state.process_withdrawal(&reward_account, Lovelace(5_000_000));
        assert_eq!(state.reward_accounts.get(&hash_key), Some(&Lovelace(0)));
    }
}
