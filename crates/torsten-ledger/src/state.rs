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
    Anchor, Certificate, Constitution, DRep, GovAction, GovActionId, ProposalProcedure, Relay,
    Vote, Voter, VotingProcedure,
};
use torsten_primitives::value::Lovelace;
use tracing::{debug, info, trace, warn};

/// Total ADA supply (45 billion ADA = 45 * 10^15 lovelace)
pub const MAX_LOVELACE_SUPPLY: u64 = 45_000_000_000_000_000;

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
    /// Votes cast: (voter, action_id) -> vote
    pub votes: BTreeMap<(Voter, GovActionId), VotingProcedure>,
    /// Total DRep registrations count (including deregistered)
    pub drep_registration_count: u64,
    /// Total proposals submitted
    pub proposal_count: u64,
    /// Current constitution (set by NewConstitution governance action)
    pub constitution: Option<Constitution>,
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

        // Apply each transaction
        for tx in &block.transactions {
            // Skip invalid transactions (phase-2 validation failure)
            if !tx.is_valid {
                continue;
            }

            // Apply UTxO changes (may fail for missing inputs during initial sync)
            if let Err(e) =
                self.utxo_set
                    .apply_transaction(&tx.hash, &tx.body.inputs, &tx.body.outputs)
            {
                // During initial sync, the UTxO set starts empty so inputs won't be found.
                // Log the issue but continue processing certificates and fees.
                debug!("UTxO application skipped: {e}");
                // Still add outputs even if inputs weren't found
                for (idx, output) in tx.body.outputs.iter().enumerate() {
                    let new_input = torsten_primitives::transaction::TransactionInput {
                        transaction_id: tx.hash,
                        index: idx as u32,
                    };
                    self.utxo_set.insert(new_input, output.clone());
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
        });

        // Process pending pool retirements for this epoch
        if let Some(retiring_pools) = self.pending_retirements.remove(&new_epoch) {
            for pool_id in &retiring_pools {
                self.pool_params.remove(pool_id);
                debug!(
                    "Pool retired at epoch {}: {}",
                    new_epoch.0,
                    pool_id.to_hex()
                );
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
            // Remove all votes for expired proposals in a single pass
            let expired_set: std::collections::HashSet<&GovActionId> = expired.iter().collect();
            self.governance
                .votes
                .retain(|(_, id), _| !expired_set.contains(id));
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

    /// Calculate and distribute rewards according to the Cardano reward formula.
    ///
    /// Uses the "go" snapshot (two epochs ago) for stake distribution and the
    /// current epoch's fees and block production data.
    ///
    /// Reward formula:
    ///   total_rewards = monetary_expansion_from_reserves + epoch_fees
    ///   treasury_cut = tau * total_rewards
    ///   pool_rewards_pot = total_rewards - treasury_cut
    ///
    /// For each pool:
    ///   apparent_performance = blocks_minted / expected_blocks (capped at 1.0)
    ///   pool_reward = pool_rewards_pot * (pool_relative_stake * apparent_performance)
    ///   operator_reward = cost + margin * max(0, pool_reward - cost)
    ///   member_rewards = pool_reward - operator_reward (shared proportionally)
    fn calculate_and_distribute_rewards(&mut self, go_snapshot: StakeSnapshot) {
        let rho = self.protocol_params.rho.as_f64();
        let tau = self.protocol_params.tau.as_f64();

        // Monetary expansion from reserves: rho * reserves
        let expansion = (rho * self.reserves.0 as f64) as u64;
        let total_rewards_available = expansion + self.epoch_fees.0;

        if total_rewards_available == 0 {
            return;
        }

        // Move expansion from reserves
        self.reserves.0 = self.reserves.0.saturating_sub(expansion);

        // Treasury cut
        let treasury_cut = (tau * total_rewards_available as f64) as u64;
        self.treasury.0 += treasury_cut;

        let pool_rewards_pot = total_rewards_available - treasury_cut;

        // Calculate total active stake from the go snapshot
        let total_active_stake: u64 = go_snapshot.pool_stake.values().map(|s| s.0).sum();
        if total_active_stake == 0 {
            // No active stake — all remaining goes to treasury
            self.treasury.0 += pool_rewards_pot;
            return;
        }

        // Expected number of blocks this epoch
        let expected_blocks = self.epoch_length as f64 * self.protocol_params.active_slot_coeff();

        let mut total_distributed: u64 = 0;

        // Build delegators-by-pool index for O(n) reward distribution
        // instead of O(n*m) inner loop over all delegations per pool
        let mut delegators_by_pool: HashMap<Hash28, Vec<Hash32>> = HashMap::new();
        for (cred_hash, pool_id) in &go_snapshot.delegations {
            delegators_by_pool
                .entry(*pool_id)
                .or_default()
                .push(*cred_hash);
        }

        // Calculate rewards per pool
        for (pool_id, pool_active_stake) in &go_snapshot.pool_stake {
            let pool_reg = match go_snapshot.pool_params.get(pool_id) {
                Some(reg) => reg,
                None => continue,
            };

            let relative_stake = pool_active_stake.0 as f64 / total_active_stake as f64;

            // Pool performance: blocks produced / expected blocks for this stake
            let expected_pool_blocks = expected_blocks * relative_stake;
            let blocks_minted = self.epoch_blocks_by_pool.get(pool_id).copied().unwrap_or(0) as f64;
            let performance = if expected_pool_blocks > 0.0 {
                (blocks_minted / expected_pool_blocks).min(1.0)
            } else {
                0.0
            };

            // Pool's share of the rewards pot
            let pool_reward = (pool_rewards_pot as f64 * relative_stake * performance) as u64;

            if pool_reward == 0 {
                continue;
            }

            // Operator gets cost + margin * (pool_reward - cost)
            let cost = pool_reg.cost.0;
            let margin =
                pool_reg.margin_numerator as f64 / pool_reg.margin_denominator.max(1) as f64;

            let operator_reward = if pool_reward <= cost {
                pool_reward
            } else {
                cost + (margin * (pool_reward - cost) as f64) as u64
            };

            let member_reward_pot = pool_reward.saturating_sub(operator_reward);

            // Distribute member rewards proportionally to delegators
            if let Some(delegators) = delegators_by_pool.get(pool_id) {
                for cred_hash in delegators {
                    let member_stake = self
                        .stake_distribution
                        .stake_map
                        .get(cred_hash)
                        .copied()
                        .unwrap_or(Lovelace(0))
                        .0;

                    if member_stake == 0 || pool_active_stake.0 == 0 {
                        continue;
                    }

                    let member_share = (member_reward_pot as f64 * member_stake as f64
                        / pool_active_stake.0 as f64) as u64;

                    if member_share > 0 {
                        *self
                            .reward_accounts
                            .entry(*cred_hash)
                            .or_insert(Lovelace(0)) += Lovelace(member_share);
                        total_distributed += member_share;
                    }
                }
            }

            // Operator reward goes to pool's reward account
            // (Use pool_id padded to 32 bytes as the reward key)
            if operator_reward > 0 {
                let mut op_key_bytes = [0u8; 32];
                op_key_bytes[..28].copy_from_slice(pool_id.as_bytes());
                let op_key = Hash32::from_bytes(op_key_bytes);
                *self.reward_accounts.entry(op_key).or_insert(Lovelace(0)) +=
                    Lovelace(operator_reward);
                total_distributed += operator_reward;
            }
        }

        // Any undistributed rewards go to treasury
        let undistributed = pool_rewards_pot.saturating_sub(total_distributed);
        if undistributed > 0 {
            self.treasury.0 += undistributed;
        }

        info!(
            "Rewards distributed: {} lovelace to accounts, {} to treasury (expansion: {}, fees: {})",
            total_distributed, treasury_cut + undistributed, expansion, self.epoch_fees.0
        );
    }

    /// Update the rolling nonce with a new VRF output.
    ///
    /// rolling_nonce = hash(rolling_nonce || hash(vrf_output))
    fn update_rolling_nonce(&mut self, vrf_output: &[u8]) {
        let vrf_hash = torsten_primitives::hash::blake2b_256(vrf_output);
        let mut data = Vec::with_capacity(64);
        data.extend_from_slice(self.rolling_nonce.as_bytes());
        data.extend_from_slice(vrf_hash.as_bytes());
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

        // Record the vote
        self.governance
            .votes
            .insert((voter.clone(), action_id.clone()), procedure.clone());

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

        // Collect ratified proposal IDs and their actions
        let ratified: Vec<(GovActionId, GovAction)> = self
            .governance
            .proposals
            .iter()
            .filter(|(action_id, state)| {
                self.check_ratification(action_id, state, total_drep_stake)
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
            // Remove all votes for ratified proposals in a single pass
            let ratified_set: std::collections::HashSet<&GovActionId> =
                ratified.iter().map(|(id, _)| id).collect();
            self.governance
                .votes
                .retain(|(_, id), _| !ratified_set.contains(id));
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
    /// - ParameterChange: requires DRep vote ≥ dvt_p_param_change AND CC approval
    /// - HardForkInitiation: requires DRep ≥ dvt_hard_fork AND SPO ≥ pvt_hard_fork
    /// - NoConfidence: requires DRep ≥ dvt_no_confidence AND SPO ≥ pvt_committee
    /// - UpdateCommittee: requires DRep ≥ dvt_committee AND SPO ≥ pvt_committee
    /// - NewConstitution: requires DRep ≥ dvt_constitution AND CC approval
    /// - TreasuryWithdrawals: requires DRep ≥ dvt_treasury_withdrawal AND CC approval
    fn check_ratification(
        &self,
        action_id: &GovActionId,
        state: &ProposalState,
        total_drep_stake: u64,
    ) -> bool {
        // Count votes by voter type
        let (drep_yes, drep_total, spo_yes, spo_total, cc_yes, cc_total) =
            self.count_votes_by_type(action_id);

        match &state.procedure.gov_action {
            GovAction::InfoAction => {
                // InfoAction is always ratified (it's informational only)
                true
            }
            GovAction::ParameterChange { .. } => {
                let drep_threshold = self.protocol_params.dvt_p_param_change.as_f64();
                let drep_met =
                    check_threshold(drep_yes, drep_total.max(total_drep_stake), drep_threshold);
                let cc_met = check_cc_approval(cc_yes, cc_total, &self.governance, self.epoch);
                drep_met && cc_met
            }
            GovAction::HardForkInitiation { .. } => {
                let drep_threshold = self.protocol_params.dvt_hard_fork.as_f64();
                let spo_threshold = self.protocol_params.pvt_hard_fork.as_f64();
                let drep_met =
                    check_threshold(drep_yes, drep_total.max(total_drep_stake), drep_threshold);
                let spo_met = check_threshold(spo_yes, spo_total, spo_threshold);
                drep_met && spo_met
            }
            GovAction::NoConfidence { .. } => {
                let drep_threshold = self.protocol_params.dvt_no_confidence.as_f64();
                let spo_threshold = self.protocol_params.pvt_committee.as_f64();
                let drep_met =
                    check_threshold(drep_yes, drep_total.max(total_drep_stake), drep_threshold);
                let spo_met = check_threshold(spo_yes, spo_total, spo_threshold);
                drep_met && spo_met
            }
            GovAction::UpdateCommittee { .. } => {
                let drep_threshold = self.protocol_params.dvt_committee_normal.as_f64();
                let spo_threshold = self.protocol_params.pvt_committee.as_f64();
                let drep_met =
                    check_threshold(drep_yes, drep_total.max(total_drep_stake), drep_threshold);
                let spo_met = check_threshold(spo_yes, spo_total, spo_threshold);
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

        for ((voter, aid), procedure) in &self.governance.votes {
            if aid != action_id {
                continue;
            }
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
                if let Some(v) = update.drep_deposit {
                    self.protocol_params.drep_deposit = v;
                }
                if let Some(v) = update.gov_action_lifetime {
                    self.protocol_params.gov_action_lifetime = v;
                }
                if let Some(v) = update.gov_action_deposit {
                    self.protocol_params.gov_action_deposit = v;
                }
                if let Some(ref v) = update.dvt_p_param_change {
                    self.protocol_params.dvt_p_param_change = v.clone();
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
                if let Some(ref v) = update.pvt_hard_fork {
                    self.protocol_params.pvt_hard_fork = v.clone();
                }
                if let Some(ref v) = update.pvt_committee {
                    self.protocol_params.pvt_committee = v.clone();
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

    /// Process a withdrawal from a reward account
    fn process_withdrawal(&mut self, reward_account: &[u8], amount: Lovelace) {
        // In Cardano, withdrawals consume rewards from the reward account.
        // The withdrawal amount is added to the tx input sum (handled in value conservation).
        if reward_account.len() >= 29 {
            // The reward account is 1 byte header + 28 bytes key hash
            let mut key_bytes = [0u8; 32];
            let copy_len = (reward_account.len() - 1).min(32);
            key_bytes[..copy_len].copy_from_slice(&reward_account[1..1 + copy_len]);
            let key = Hash32::from_bytes(key_bytes);
            if let Some(balance) = self.reward_accounts.get_mut(&key) {
                *balance = balance.checked_sub(amount).unwrap_or(Lovelace(0));
            }
        }
    }

    /// Save ledger state snapshot to disk using bincode serialization
    pub fn save_snapshot(&self, path: &Path) -> Result<(), LedgerError> {
        let tmp_path = path.with_extension("tmp");
        let data = bincode::serialize(self).map_err(|e| {
            LedgerError::EpochTransition(format!("Failed to serialize ledger state: {e}"))
        })?;
        std::fs::write(&tmp_path, &data)
            .map_err(|e| LedgerError::EpochTransition(format!("Failed to write snapshot: {e}")))?;
        std::fs::rename(&tmp_path, path)
            .map_err(|e| LedgerError::EpochTransition(format!("Failed to rename snapshot: {e}")))?;
        info!(
            path = %path.display(),
            bytes = data.len(),
            utxo_count = self.utxo_set.len(),
            epoch = self.epoch.0,
            slot = ?self.tip.point.slot().map(|s| s.0),
            "Ledger snapshot saved"
        );
        Ok(())
    }

    /// Load ledger state snapshot from disk
    pub fn load_snapshot(path: &Path) -> Result<Self, LedgerError> {
        let data = std::fs::read(path)
            .map_err(|e| LedgerError::EpochTransition(format!("Failed to read snapshot: {e}")))?;
        let state: LedgerState = bincode::deserialize(&data).map_err(|e| {
            LedgerError::EpochTransition(format!("Failed to deserialize ledger state: {e}"))
        })?;
        info!(
            path = %path.display(),
            bytes = data.len(),
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

/// Check if a voting threshold is met: yes_votes / total_votes >= threshold
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
        state.epoch_length = 100; // Small epochs for testing
        state.reserves = Lovelace(10_000_000_000); // 10k ADA in reserves

        let cred = Credential::VerificationKey(Hash28::from_bytes([42u8; 28]));
        let pool_id = Hash28::from_bytes([1u8; 28]);
        let key = credential_to_hash(&cred);

        // Register stake, pool, and delegate
        state.process_certificate(&Certificate::StakeRegistration(cred.clone()));
        state
            .stake_distribution
            .stake_map
            .insert(key, Lovelace(1_000_000_000)); // 1000 ADA

        state.process_certificate(&Certificate::PoolRegistration(PoolParams {
            operator: pool_id,
            vrf_keyhash: Hash32::from_bytes([2u8; 32]),
            pledge: Lovelace(100_000_000),
            cost: Lovelace(340_000_000),
            margin: Rational {
                numerator: 1,
                denominator: 100,
            },
            reward_account: vec![0u8; 29],
            pool_owners: vec![pool_id],
            relays: vec![],
            pool_metadata: None,
        }));

        state.process_certificate(&Certificate::StakeDelegation {
            credential: cred.clone(),
            pool_hash: pool_id,
        });

        // Build up snapshots: need 3 rotations before "go" is populated
        // Epoch 0→1: mark=snap1
        state.process_epoch_transition(EpochNo(1));
        // Epoch 1→2: set=snap1, mark=snap2
        state.process_epoch_transition(EpochNo(2));
        // Epoch 2→3: go=snap1, set=snap2, mark=snap3
        state.process_epoch_transition(EpochNo(3));

        // Add fees and block production for epoch 3
        state.epoch_fees = Lovelace(5_000_000); // 5 ADA in fees
                                                // Pool produced all blocks for the epoch
        state.epoch_blocks_by_pool.insert(pool_id, 5);

        // Epoch 3→4: triggers reward calculation using "go" snapshot
        state.process_epoch_transition(EpochNo(4));

        // Treasury should have increased (tau * total_rewards)
        assert!(state.treasury.0 > 0);

        // Reserves should have decreased (monetary expansion)
        assert!(state.reserves.0 < 10_000_000_000);

        // Reward accounts should have received something
        let total_rewards: u64 = state.reward_accounts.values().map(|l| l.0).sum();
        assert!(total_rewards > 0);
    }

    #[test]
    fn test_reward_calculation_no_blocks_no_rewards() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);
        state.epoch_length = 100;
        state.reserves = Lovelace(10_000_000_000);

        let cred = Credential::VerificationKey(Hash28::from_bytes([42u8; 28]));
        let pool_id = Hash28::from_bytes([1u8; 28]);
        let key = credential_to_hash(&cred);

        // Setup delegation
        state.process_certificate(&Certificate::StakeRegistration(cred.clone()));
        state
            .stake_distribution
            .stake_map
            .insert(key, Lovelace(1_000_000_000));

        state.process_certificate(&Certificate::PoolRegistration(PoolParams {
            operator: pool_id,
            vrf_keyhash: Hash32::from_bytes([2u8; 32]),
            pledge: Lovelace(100_000_000),
            cost: Lovelace(340_000_000),
            margin: Rational {
                numerator: 0,
                denominator: 1,
            },
            reward_account: vec![0u8; 29],
            pool_owners: vec![pool_id],
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

        // No blocks produced, no fees — pool produced 0 blocks
        // epoch_blocks_by_pool is empty

        state.process_epoch_transition(EpochNo(4));

        // Pool produced no blocks, so performance = 0, no pool rewards
        // All pool pot goes to treasury as undistributed
        let member_rewards: u64 = state.reward_accounts.values().map(|l| l.0).sum();
        assert_eq!(member_rewards, 0);
        // But treasury still gets the treasury cut + undistributed
        assert!(state.treasury.0 > 0);
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
        assert_eq!(state.governance.votes.len(), 2);
    }

    #[test]
    fn test_governance_proposal_expiry() {
        let params = ProtocolParameters::mainnet_defaults();
        let mut state = LedgerState::new(params);
        state.epoch_length = 100;

        // Use a ParameterChange proposal (requires DRep votes to ratify)
        // so it won't be auto-ratified like InfoAction
        let update = torsten_primitives::transaction::ProtocolParamUpdate::default();
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

        // Submit a parameter change proposal to increase max_tx_size
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

        assert_eq!(state.protocol_params.max_tx_size, 16384); // original value

        // Epoch transition should ratify and enact
        state.process_epoch_transition(EpochNo(1));

        assert_eq!(state.protocol_params.max_tx_size, 32768); // updated
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
}
