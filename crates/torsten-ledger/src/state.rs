use crate::utxo::UtxoSet;
use torsten_primitives::block::{Block, Point, Tip};
use torsten_primitives::credentials::Credential;
use torsten_primitives::era::Era;
use torsten_primitives::hash::Hash32;
use torsten_primitives::protocol_params::ProtocolParameters;
use torsten_primitives::time::{BlockNo, EpochNo, SlotNo};
use torsten_primitives::transaction::Certificate;
use torsten_primitives::value::Lovelace;
use std::collections::BTreeMap;
use tracing::debug;

/// The complete ledger state
#[derive(Debug, Clone)]
pub struct LedgerState {
    /// Current UTxO set
    pub utxo_set: UtxoSet,
    /// Current tip of the chain
    pub tip: Tip,
    /// Current era
    pub era: Era,
    /// Current epoch
    pub epoch: EpochNo,
    /// Current protocol parameters
    pub protocol_params: ProtocolParameters,
    /// Stake distribution
    pub stake_distribution: StakeDistributionState,
    /// Treasury balance
    pub treasury: Lovelace,
    /// Reserves balance
    pub reserves: Lovelace,
    /// Delegation state
    pub delegations: BTreeMap<Hash32, Hash32>,
    /// Pool registrations
    pub pool_params: BTreeMap<Hash32, PoolRegistration>,
}

#[derive(Debug, Clone, Default)]
pub struct StakeDistributionState {
    pub stake_map: BTreeMap<Hash32, Lovelace>,
}

#[derive(Debug, Clone)]
pub struct PoolRegistration {
    pub pool_id: Hash32,
    pub vrf_keyhash: Hash32,
    pub pledge: Lovelace,
    pub cost: Lovelace,
    pub margin_numerator: u64,
    pub margin_denominator: u64,
}

impl LedgerState {
    pub fn new(params: ProtocolParameters) -> Self {
        LedgerState {
            utxo_set: UtxoSet::new(),
            tip: Tip::origin(),
            era: Era::Conway,
            epoch: EpochNo(0),
            protocol_params: params,
            stake_distribution: StakeDistributionState::default(),
            treasury: Lovelace(0),
            reserves: Lovelace(0),
            delegations: BTreeMap::new(),
            pool_params: BTreeMap::new(),
        }
    }

    /// Apply a block to the ledger state
    pub fn apply_block(&mut self, block: &Block) -> Result<(), LedgerError> {
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

        // Apply each transaction
        for tx in &block.transactions {
            // Skip invalid transactions (phase-2 validation failure)
            if !tx.is_valid {
                continue;
            }

            self.utxo_set
                .apply_transaction(&tx.hash, &tx.body.inputs, &tx.body.outputs)
                .map_err(|e| LedgerError::UtxoError(e.to_string()))?;

            // Process certificates
            for cert in &tx.body.certificates {
                self.process_certificate(cert);
            }

            // Process withdrawals (rewards are consumed, no UTxO effect)
            for (reward_account, _amount) in &tx.body.withdrawals {
                self.process_withdrawal(reward_account);
            }
        }

        // Update tip
        self.tip = block.tip();
        self.era = block.era;

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
                self.stake_distribution.stake_map.entry(key).or_insert(Lovelace(0));
                debug!("Stake key registered: {}", key.to_hex());
            }
            Certificate::StakeDeregistration(credential) => {
                let key = credential_to_hash(credential);
                self.stake_distribution.stake_map.remove(&key);
                self.delegations.remove(&key);
                debug!("Stake key deregistered: {}", key.to_hex());
            }
            Certificate::StakeDelegation { credential, pool_hash } => {
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
                };
                debug!("Pool registered: {}", params.operator.to_hex());
                self.pool_params.insert(params.operator, pool_reg);
            }
            Certificate::PoolRetirement { pool_hash, epoch: _ } => {
                debug!("Pool retirement scheduled: {}", pool_hash.to_hex());
                // Pool retirement takes effect at epoch boundary; we just record it here
                // A full implementation would track pending retirements
                self.pool_params.remove(pool_hash);
            }
            Certificate::RegStakeDeleg { credential, pool_hash, .. } => {
                let key = credential_to_hash(credential);
                self.stake_distribution.stake_map.entry(key).or_insert(Lovelace(0));
                self.delegations.insert(key, *pool_hash);
            }
            // Governance certificates — tracked but not yet acted upon
            Certificate::RegDRep { .. }
            | Certificate::UnregDRep { .. }
            | Certificate::UpdateDRep { .. }
            | Certificate::VoteDelegation { .. }
            | Certificate::StakeVoteDelegation { .. }
            | Certificate::CommitteeHotAuth { .. }
            | Certificate::CommitteeColdResign { .. } => {
                // Conway governance — will be implemented in a later iteration
            }
        }
    }

    /// Process a withdrawal from a reward account
    fn process_withdrawal(&mut self, reward_account: &[u8]) {
        // In Cardano, withdrawals zero out the reward account balance.
        // The withdrawal amount is added to the tx input sum (handled in value conservation).
        // We just need to zero the balance here.
        if reward_account.len() >= 29 {
            // The reward account is 1 byte header + 28 bytes key hash
            let mut key_bytes = [0u8; 32];
            let copy_len = reward_account.len().min(32);
            key_bytes[..copy_len].copy_from_slice(&reward_account[..copy_len]);
            let key = Hash32::from_bytes(key_bytes);
            if let Some(stake) = self.stake_distribution.stake_map.get_mut(&key) {
                *stake = Lovelace(0);
            }
        }
    }
}

/// Extract a Hash32 from a Credential for use as a map key
fn credential_to_hash(credential: &Credential) -> Hash32 {
    let h28 = credential.to_hash();
    let mut bytes = [0u8; 32];
    bytes[..28].copy_from_slice(h28.as_bytes());
    Hash32::from_bytes(bytes)
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
                protocol_version: torsten_primitives::block::ProtocolVersion {
                    major: 9,
                    minor: 0,
                },
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
            address: Address::Byron(ByronAddress { payload: vec![0u8; 32] }),
            value: Value::lovelace(10_000_000),
            datum: OutputDatum::None,
            script_ref: None,
        };
        state.utxo_set.insert(genesis_input.clone(), genesis_output);

        let tx_hash = Hash32::from_bytes([2u8; 32]);
        let tx = Transaction {
            hash: tx_hash,
            body: TransactionBody {
                inputs: vec![genesis_input],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress { payload: vec![0u8; 32] }),
                    value: Value::lovelace(9_800_000),
                    datum: OutputDatum::None,
                    script_ref: None,
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
                address: Address::Byron(ByronAddress { payload: vec![0u8; 32] }),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
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
        let pool_hash = Hash32::from_bytes([99u8; 32]);

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

        let pool_id = Hash32::from_bytes([1u8; 32]);
        let pool_params = PoolParams {
            operator: pool_id,
            vrf_keyhash: Hash32::from_bytes([2u8; 32]),
            pledge: Lovelace(500_000_000),
            cost: Lovelace(340_000_000),
            margin: Rational { numerator: 1, denominator: 100 },
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
        let pool_hash = Hash32::from_bytes([99u8; 32]);
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

        let pool_id = Hash32::from_bytes([1u8; 32]);
        let pool_params = PoolParams {
            operator: pool_id,
            vrf_keyhash: Hash32::from_bytes([2u8; 32]),
            pledge: Lovelace(500_000_000),
            cost: Lovelace(340_000_000),
            margin: Rational { numerator: 1, denominator: 100 },
            reward_account: vec![0u8; 29],
            pool_owners: vec![pool_id],
            relays: vec![],
            pool_metadata: None,
        };

        state.process_certificate(&Certificate::PoolRegistration(pool_params));
        assert!(state.pool_params.contains_key(&pool_id));

        state.process_certificate(&Certificate::PoolRetirement {
            pool_hash: pool_id,
            epoch: 300,
        });
        assert!(!state.pool_params.contains_key(&pool_id));
    }
}
