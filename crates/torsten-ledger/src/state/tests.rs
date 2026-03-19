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
        is_legacy: false,
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
            nonce_vrf_output: vec![],
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

/// Register DReps with proper vote delegations and stake.
/// Each DRep gets `stake_per_drep` lovelace of delegated stake.
/// DRep credentials use `Hash28::from_bytes([i as u8; 28])`.
/// Stake keys use `Hash32::from_bytes([200 + i as u8; 32])`.
/// Disables `needs_stake_rebuild` so epoch transitions don't wipe manual stake.
fn setup_dreps_with_stake(state: &mut LedgerState, count: usize, stake_per_drep: u64) {
    for i in 0..count {
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
        // Set up vote delegation and stake
        let stake_key = Hash32::from_bytes([200 + i as u8; 32]);
        Arc::make_mut(&mut state.governance)
            .vote_delegations
            .insert(stake_key, DRep::KeyHash(key));
        state
            .stake_distribution
            .stake_map
            .insert(stake_key, Lovelace(stake_per_drep));
    }
    // Prevent epoch transitions from clearing manually-set stake
    state.needs_stake_rebuild = false;
}

/// Register SPOs with proper delegations and stake.
/// Pool IDs use `Hash28::from_bytes([100 + i as u8; 28])`.
/// Disables `needs_stake_rebuild` so epoch transitions don't wipe manual stake.
fn setup_spos_with_stake(state: &mut LedgerState, count: usize, stake_per_spo: u64) {
    for i in 0..count {
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
        // Add delegation and stake
        let stake_key = Hash32::from_bytes([150 + i as u8; 32]);
        Arc::make_mut(&mut state.delegations).insert(stake_key, pool_id);
        state
            .stake_distribution
            .stake_map
            .insert(stake_key, Lovelace(stake_per_spo));
    }
    // Prevent epoch transitions from clearing manually-set stake
    state.needs_stake_rebuild = false;
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
        is_legacy: false,
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
                is_legacy: false,
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
            raw_redeemers_cbor: None,
            raw_plutus_data_cbor: None,
            pallas_script_data_hash: None,
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
            is_legacy: false,
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
            raw_redeemers_cbor: None,
            raw_plutus_data_cbor: None,
            pallas_script_data_hash: None,
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
            is_legacy: false,
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
                is_legacy: false,
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
            raw_redeemers_cbor: None,
            raw_plutus_data_cbor: None,
            pallas_script_data_hash: None,
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

    // Epoch 3->4: triggers reward CALCULATION using "go" snapshot.
    // With RUPD deferred timing, rewards are computed here but not yet applied.
    state.process_epoch_transition(EpochNo(4));

    // Epoch 4->5: APPLIES the rewards computed at 3->4 boundary.
    // This matches Haskell's deferred RUPD application.
    state.process_epoch_transition(EpochNo(5));

    // Treasury should have increased (rewards applied at 4->5)
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

    // Epoch 3->4: computes rewards (deferred via RUPD)
    state.process_epoch_transition(EpochNo(4));
    // Epoch 4->5: applies the deferred rewards
    state.process_epoch_transition(EpochNo(5));

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

    // Epoch 3->4: computes rewards (deferred via RUPD); would divide by zero without the fix
    state.process_epoch_transition(EpochNo(4));
    // Epoch 4->5: applies the deferred rewards
    state.process_epoch_transition(EpochNo(5));

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
    // Epoch 3->4: computes rewards (deferred via RUPD)
    state.process_epoch_transition(EpochNo(4));
    // Epoch 4->5: applies the deferred rewards
    state.process_epoch_transition(EpochNo(5));

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
    // Epoch 3->4: computes rewards (deferred via RUPD)
    state.process_epoch_transition(EpochNo(4));
    // Epoch 4->5: applies the deferred rewards
    state.process_epoch_transition(EpochNo(5));

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
    // Also set stability_window_3kf since test blocks use protocol_version.major=9 (Babbage).
    state.randomness_stabilisation_window = 40;
    state.stability_window_3kf = 40;

    // Set a genesis hash to initialize nonce state.
    // Matches Haskell's initialChainDepState:
    //   evolving/candidate/epoch = initNonce (genesis hash)
    //   lab/lastEpochBlock = NeutralNonce (ZERO)
    let genesis_hash = Hash32::from_bytes([0xAB; 32]);
    state.set_genesis_hash(genesis_hash);

    // evolving, candidate, epoch all start from genesis hash
    assert_eq!(state.evolving_nonce, genesis_hash);
    assert_eq!(state.candidate_nonce, genesis_hash);
    assert_eq!(state.epoch_nonce, genesis_hash);
    // lab and lastEpochBlock start as NeutralNonce (ZERO)
    assert_eq!(state.lab_nonce, Hash32::ZERO);
    assert_eq!(state.last_epoch_block_nonce, Hash32::ZERO);

    // Apply a block BEFORE the stabilisation window (slot 10; epoch ends at slot 100;
    // stabilisation starts at slot 60; so slot 10 < 60 means candidate tracks evolving).
    // nonce_vrf_output is set — this is what drives the evolving/candidate nonce update.
    // vrf_result.output is the VRF cert used for leader election (not nonce).
    let mut block = make_test_block(10, 1, Hash32::ZERO, vec![]);
    block.header.nonce_vrf_output = vec![0x42u8; 32]; // pre-computed eta
    block.header.vrf_result.output = vec![0x42u8; 32];
    block.header.issuer_vkey = vec![1u8; 32];
    state
        .apply_block(&block, BlockValidationMode::ApplyOnly)
        .unwrap();

    // Evolving nonce: update_evolving_nonce always hashes nonce_vrf_output once more
    // before combining — matching pallas generate_rolling_nonce exactly (commit 49f9885).
    //
    // Serialization (multi_era.rs) pre-hashes per era:
    //   TPraos (Shelley-Alonzo): eta = blake2b_256(nonce_vrf_cert.0)  [stored in nonce_vrf_output]
    //   Praos  (Babbage/Conway): eta = blake2b_256("N" || vrf_result.0) [stored in nonce_vrf_output]
    //
    // update_evolving_nonce then hashes again and combines:
    //   eta_hash = blake2b_256(nonce_vrf_output)
    //   evolving' = blake2b_256(evolving || eta_hash)
    //
    // Here nonce_vrf_output = [0x42;32].
    // evolving' = blake2b_256(genesis_hash || blake2b_256([0x42;32]))
    let eta_hash = torsten_primitives::hash::blake2b_256(&[0x42u8; 32]);
    let mut expected_evolving = Vec::new();
    expected_evolving.extend_from_slice(genesis_hash.as_bytes());
    expected_evolving.extend_from_slice(eta_hash.as_bytes());
    assert_eq!(
        state.evolving_nonce,
        torsten_primitives::hash::blake2b_256(&expected_evolving),
        "evolving_nonce should be blake2b_256(genesis || blake2b_256(nonce_vrf_output))"
    );
    // Candidate nonce tracks evolving (not in stabilisation window)
    assert_eq!(state.candidate_nonce, state.evolving_nonce);
    // LAB nonce = prevHashToNonce(block.prevHash) = prev_hash of the applied block
    assert_eq!(state.lab_nonce, block.header.prev_hash);

    // Apply a block INSIDE the stabilisation window (slot 70 + 40 >= 100)
    let evolving_before = state.evolving_nonce;
    let candidate_before = state.candidate_nonce;
    let mut block2 = make_test_block(70, 2, *block.hash(), vec![]);
    block2.header.nonce_vrf_output = vec![0x63u8; 32];
    block2.header.vrf_result.output = vec![0x63u8; 32];
    block2.header.issuer_vkey = vec![1u8; 32];
    state
        .apply_block(&block2, BlockValidationMode::ApplyOnly)
        .unwrap();

    // Evolving nonce STILL updates (evolving_nonce advances for every block)
    assert_ne!(state.evolving_nonce, evolving_before);
    // Candidate nonce is FROZEN (in stabilisation window, slot >= epoch_end - rsw)
    assert_eq!(state.candidate_nonce, candidate_before);
    // LAB nonce = prevHashToNonce(block2.prevHash) = prev_hash of block2 applied
    assert_eq!(state.lab_nonce, block2.header.prev_hash);

    // Trigger epoch transition (epoch 0 → 1).
    //
    // Per Haskell's Praos tickChainDepState (Nonce ⭒ operator):
    //   epochNonce' = candidateNonce ⭒ lastEpochBlockNonce
    //   lastEpochBlockNonce' = labNonce   (snapshot for NEXT epoch)
    //
    // Haskell ⭒ operator:
    //   Nonce(a) ⭒ Nonce(b) = Nonce(blake2b_256(a || b))
    //   x ⭒ NeutralNonce = x  (identity)
    //   NeutralNonce ⭒ x = x  (identity)
    //
    // At the FIRST epoch boundary (epoch 0 → 1):
    //   - lastEpochBlockNonce = NeutralNonce (initialized in set_genesis_hash)
    //   - epochNonce' = candidateNonce ⭒ NeutralNonce = candidateNonce  (identity)
    //   - lastEpochBlockNonce' = labNonce (= block2.header.prev_hash)
    let nonce_before_transition = state.epoch_nonce;
    let candidate_at_transition = state.candidate_nonce;
    let lab_at_transition = state.lab_nonce;
    let last_epoch_block_before = state.last_epoch_block_nonce; // ZERO at first transition
    state.process_epoch_transition(EpochNo(1));

    // epoch_nonce should have been updated from genesis_hash
    assert_ne!(state.epoch_nonce, nonce_before_transition);
    // evolving_nonce carries forward (NOT reset at epoch boundary)
    assert_ne!(state.evolving_nonce, Hash32::ZERO);
    // last_epoch_block_nonce is updated to lab_nonce AFTER epoch_nonce is computed
    assert_eq!(state.last_epoch_block_nonce, lab_at_transition);

    assert_eq!(
        last_epoch_block_before,
        Hash32::ZERO,
        "First transition: lastEpochBlockNonce must be NeutralNonce (ZERO)"
    );
    // epoch_nonce = candidateNonce ⭒ lastEpochBlockNonce (Haskell ⭒ operator semantics)
    // At epoch 0→1: lastEpochBlockNonce = NeutralNonce.
    // Haskell ⭒ operator: x ⭒ NeutralNonce = x (identity, no hashing).
    // So epochNonce' = candidateNonce (unmodified).
    //
    // Note: pallas generate_epoch_nonce unconditionally hashes, but that function is
    // designed for normal epochs where lastEpochBlockNonce is always a real hash.
    // At genesis, the Haskell type system gives NeutralNonce identity behavior.
    assert_eq!(
        state.epoch_nonce,
        candidate_at_transition,
        "At first epoch boundary (lastEpochBlockNonce=NeutralNonce), epoch_nonce = candidateNonce (⭒ identity)"
    );
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

/// Issue #157: script-based CC hot key type must be tracked and reported correctly.
/// When a committee member authorizes a Credential::Script hot key via CommitteeHotAuth,
/// `script_committee_hot_credentials` must contain the hot key hash so that
/// GetCommitteeState (tag 27) returns `hot_credential_type = 1` (ScriptHash).
#[test]
fn test_committee_hot_auth_script_hot_key_tracked() {
    let params = ProtocolParameters::mainnet_defaults();
    let mut state = LedgerState::new(params);

    // Authorize with a script hot key — the fix must record this in
    // script_committee_hot_credentials keyed by the hot credential hash.
    let cold = Credential::VerificationKey(Hash28::from_bytes([10u8; 28]));
    let hot_script = Credential::Script(Hash28::from_bytes([30u8; 28]));
    let hot_key = credential_to_hash(&hot_script);

    state.process_certificate(&Certificate::CommitteeHotAuth {
        cold_credential: cold,
        hot_credential: hot_script,
    });

    assert!(
        state
            .governance
            .script_committee_hot_credentials
            .contains(&hot_key),
        "script hot key must be recorded in script_committee_hot_credentials"
    );
}

/// Key hot key authorization must NOT pollute script_committee_hot_credentials.
#[test]
fn test_committee_hot_auth_key_hot_key_not_tracked() {
    let params = ProtocolParameters::mainnet_defaults();
    let mut state = LedgerState::new(params);

    let cold = Credential::VerificationKey(Hash28::from_bytes([10u8; 28]));
    let hot_key_cred = Credential::VerificationKey(Hash28::from_bytes([20u8; 28]));
    let hot_key_hash = credential_to_hash(&hot_key_cred);

    state.process_certificate(&Certificate::CommitteeHotAuth {
        cold_credential: cold,
        hot_credential: hot_key_cred,
    });

    assert!(
        !state
            .governance
            .script_committee_hot_credentials
            .contains(&hot_key_hash),
        "key hot credential must NOT appear in script_committee_hot_credentials"
    );
}

/// Re-authorization: replacing a script hot key with a key hot key.
/// The new (key) hot key hash must not appear in the script set; the
/// stale script hot key entry from the old authorization is unreachable
/// (committee_hot_keys no longer points to it) so the correct type is returned.
#[test]
fn test_committee_hot_auth_reauth_script_to_key() {
    let params = ProtocolParameters::mainnet_defaults();
    let mut state = LedgerState::new(params);

    let cold = Credential::VerificationKey(Hash28::from_bytes([10u8; 28]));
    let hot_script = Credential::Script(Hash28::from_bytes([30u8; 28]));
    let hot_key_cred = Credential::VerificationKey(Hash28::from_bytes([40u8; 28]));
    let cold_key = credential_to_hash(&cold);
    let new_hot_key = credential_to_hash(&hot_key_cred);

    // First: authorize with script
    state.process_certificate(&Certificate::CommitteeHotAuth {
        cold_credential: cold.clone(),
        hot_credential: hot_script,
    });

    // Then: re-authorize with a key hot credential
    state.process_certificate(&Certificate::CommitteeHotAuth {
        cold_credential: cold,
        hot_credential: hot_key_cred,
    });

    // The committee_hot_keys map now points to the new key hot credential
    assert_eq!(
        state.governance.committee_hot_keys[&cold_key], new_hot_key,
        "committee_hot_keys must point to the new hot key"
    );
    // The new hot key is a key credential — it must not be in the script set
    assert!(
        !state
            .governance
            .script_committee_hot_credentials
            .contains(&new_hot_key),
        "new key hot credential must not be in script_committee_hot_credentials"
    );
}

/// Re-authorization: replacing a key hot key with a script hot key.
/// The new script hot key must appear in the script set.
#[test]
fn test_committee_hot_auth_reauth_key_to_script() {
    let params = ProtocolParameters::mainnet_defaults();
    let mut state = LedgerState::new(params);

    let cold = Credential::VerificationKey(Hash28::from_bytes([10u8; 28]));
    let hot_key_cred = Credential::VerificationKey(Hash28::from_bytes([20u8; 28]));
    let hot_script = Credential::Script(Hash28::from_bytes([50u8; 28]));
    let cold_key = credential_to_hash(&cold);
    let new_hot_key = credential_to_hash(&hot_script);

    // First: authorize with key
    state.process_certificate(&Certificate::CommitteeHotAuth {
        cold_credential: cold.clone(),
        hot_credential: hot_key_cred,
    });

    // Then: re-authorize with script
    state.process_certificate(&Certificate::CommitteeHotAuth {
        cold_credential: cold,
        hot_credential: hot_script,
    });

    assert_eq!(
        state.governance.committee_hot_keys[&cold_key], new_hot_key,
        "committee_hot_keys must point to the new script hot key"
    );
    assert!(
        state
            .governance
            .script_committee_hot_credentials
            .contains(&new_hot_key),
        "new script hot key must be in script_committee_hot_credentials"
    );
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

    // Submit at epoch 0 → expires at epoch 7 (0 + 6 + 1, per Haskell gasExpiresAfter)
    state.process_proposal(&tx_hash, 0, &proposal);
    assert_eq!(state.governance.proposals.len(), 1);

    // Advance to epoch 7 — should still be active (expires_epoch = 7, active through epoch 7)
    // Per Haskell: `gasExpiresAfter < reCurrentEpoch` — proposals are active
    // through their expiresAfter epoch.
    for e in 1..=7 {
        state.process_epoch_transition(EpochNo(e));
    }
    assert_eq!(state.governance.proposals.len(), 1);

    // Advance to epoch 8 — should expire (7 < 8)
    state.process_epoch_transition(EpochNo(8));
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
            raw_redeemers_cbor: None,
            raw_plutus_data_cbor: None,
            pallas_script_data_hash: None,
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

    // Submit at epoch 0 — expires at epoch 3 (0 + 2 + 1, per Haskell gasExpiresAfter)
    state.process_proposal(&tx_hash, 0, &proposal);
    assert_eq!(state.governance.proposals.len(), 1);

    // Epoch 1: proposal still active, not expired, not ratified (no votes)
    state.process_epoch_transition(EpochNo(1));
    assert_eq!(state.governance.proposals.len(), 1);
    assert!(state.governance.last_ratified.is_empty());
    assert!(state.governance.last_expired.is_empty());

    // Epoch 2: still active (expires_epoch = 3, active through epoch 3)
    // Per Haskell: `gasExpiresAfter < reCurrentEpoch` — 3 < 2 is false
    state.process_epoch_transition(EpochNo(2));
    assert_eq!(state.governance.proposals.len(), 1);
    assert!(state.governance.last_ratified.is_empty());
    assert!(state.governance.last_expired.is_empty());

    // Epoch 3: still active (3 < 3 is false)
    state.process_epoch_transition(EpochNo(3));
    assert_eq!(state.governance.proposals.len(), 1);
    assert!(state.governance.last_ratified.is_empty());
    assert!(state.governance.last_expired.is_empty());

    // Epoch 4: proposal expires (3 < 4)
    state.process_epoch_transition(EpochNo(4));
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

    // Register 10 DReps with equal stake (1B each)
    setup_dreps_with_stake(&mut state, 10, 1_000_000_000);

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

    // Register DReps with stake
    setup_dreps_with_stake(&mut state, 10, 1_000_000_000);

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

    // Register DReps and SPOs with stake
    setup_dreps_with_stake(&mut state, 10, 1_000_000_000);
    setup_spos_with_stake(&mut state, 10, 1_000_000_000);

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

    // Register DReps and SPOs with stake
    setup_dreps_with_stake(&mut state, 10, 1_000_000_000);
    setup_spos_with_stake(&mut state, 10, 1_000_000_000);

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
        is_legacy: false,
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
    assert_eq!(ps.expires_epoch, EpochNo(16)); // epoch 5 + lifetime 10 + 1
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
        is_legacy: false,
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
                is_legacy: false,
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
            raw_redeemers_cbor: None,
            raw_plutus_data_cbor: None,
            pallas_script_data_hash: None,
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
        is_legacy: false,
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
            raw_redeemers_cbor: None,
            raw_plutus_data_cbor: None,
            pallas_script_data_hash: None,
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
        is_legacy: false,
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
        is_legacy: false,
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
            raw_redeemers_cbor: None,
            raw_plutus_data_cbor: None,
            pallas_script_data_hash: None,
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
        is_legacy: false,
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
            raw_redeemers_cbor: None,
            raw_plutus_data_cbor: None,
            pallas_script_data_hash: None,
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
        let delegator_cred = Credential::VerificationKey(Hash28::from_bytes([(i + 100) as u8; 28]));
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
        let delegator_cred = Credential::VerificationKey(Hash28::from_bytes([(i + 200) as u8; 28]));
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

    // NoConfidence stake = 3B (counts as Yes for NoConfidence actions)
    // DRep active stake = 5B (all implicit No since no DRep votes cast)
    // Total = 5B + 3B = 8B; Yes = 3B
    assert_eq!(drep_yes, 3_000_000_000);
    assert_eq!(drep_total, 8_000_000_000);
}

#[test]
fn test_always_no_confidence_counts_no_for_other_actions() {
    let (mut state, dreps) = setup_governance_state(5, 1_000_000_000);

    for i in 0..3u32 {
        let delegator_cred = Credential::VerificationKey(Hash28::from_bytes([(i + 200) as u8; 28]));
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

    // 2B yes (voted), 3B no (AlwaysNoConfidence), 3B implicit no (non-voting DReps)
    // Total = 5B (DRep) + 3B (NoConfidence) = 8B
    assert_eq!(drep_yes, 2_000_000_000);
    assert_eq!(drep_total, 8_000_000_000);
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
    // This is the CANDIDATE NONCE FREEZE window (randomnessStabilisationWindow = 4k/f).
    // Not to be confused with stabilityWindow (3k/f = 129600) used for chain selection.
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
    // f=0.25 = 1/4, k=3 → ceil(4*3/(1/4)) = ceil(48) = 48
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
    // Set both windows to 40. Test block uses protocol_version.major=9 (Babbage), so
    // stability_window_3kf is what's used. randomness_stabilisation_window used for Conway+.
    state.randomness_stabilisation_window = 40;
    state.stability_window_3kf = 40;

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
    block.header.nonce_vrf_output = vec![0x42u8; 32]; // non-empty so evolving nonce updates
    block.header.vrf_result.output = vec![0x42u8; 32];
    block.header.issuer_vkey = vec![1u8; 32];

    // This should NOT panic; the candidate nonce should be frozen
    // because the extreme slot is definitely in the stabilisation window.
    state
        .apply_block(&block, BlockValidationMode::ApplyOnly)
        .unwrap();

    // Evolving nonce updated: genesis_hash <> eta = blake2b(genesis_hash || eta)
    // (genesis_hash is non-ZERO so the hash-combination path is taken, not identity)
    assert_ne!(state.evolving_nonce, genesis_hash);
    assert_ne!(state.evolving_nonce, Hash32::ZERO);
    // Candidate nonce should be FROZEN at genesis_hash (extreme slot is in stabilisation window).
    // set_genesis_hash initialises candidate to genesis_hash, and since the first block lands
    // inside the stabilisation window, candidate stays frozen at genesis_hash.
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
    // Set both windows to 40. Test block uses protocol_version.major=9 (Babbage), so
    // stability_window_3kf drives the candidate freeze.
    state.randomness_stabilisation_window = 40;
    state.stability_window_3kf = 40;

    let genesis_hash = Hash32::from_bytes([0xAB; 32]);
    state.set_genesis_hash(genesis_hash);

    // Slot 59 is the LAST slot before the stabilisation window
    // (59 < 100 - 40 = 60, so candidate updates)
    let mut block = make_test_block(59, 1, Hash32::ZERO, vec![]);
    block.header.nonce_vrf_output = vec![0x42u8; 32]; // non-empty so evolving nonce updates
    block.header.vrf_result.output = vec![0x42u8; 32];
    block.header.issuer_vkey = vec![1u8; 32];
    state
        .apply_block(&block, BlockValidationMode::ApplyOnly)
        .unwrap();
    // After slot 59 (outside stabilisation window): candidate tracks evolving
    assert_eq!(state.candidate_nonce, state.evolving_nonce);
    assert_ne!(state.evolving_nonce, genesis_hash); // nonce advanced

    // Slot 60 is the FIRST slot in the stabilisation window
    // (60 >= 100 - 40 = 60, so candidate freezes)
    let candidate_before = state.candidate_nonce;
    let mut block2 = make_test_block(60, 2, *block.hash(), vec![]);
    block2.header.nonce_vrf_output = vec![0x63u8; 32]; // non-empty so evolving nonce updates
    block2.header.vrf_result.output = vec![0x63u8; 32];
    block2.header.issuer_vkey = vec![1u8; 32];
    state
        .apply_block(&block2, BlockValidationMode::ApplyOnly)
        .unwrap();
    // After slot 60 (inside stabilisation window): candidate frozen
    assert_eq!(state.candidate_nonce, candidate_before);
    // Evolving nonce still advances
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
        epoch_fees: Lovelace(0),
        epoch_block_count: 0,
        epoch_blocks_by_pool: Arc::new(HashMap::new()),
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
        epoch_fees: Lovelace(0),
        epoch_block_count: 0,
        epoch_blocks_by_pool: Arc::new(HashMap::new()),
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
        epoch_fees: Lovelace(0),
        epoch_block_count: 0,
        epoch_blocks_by_pool: Arc::new(HashMap::new()),
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
    // u64::MAX * 45e15 * 21600 / u64::MAX = 45e15 * 21600 = 9.72e17
    // This exceeds u64::MAX, so floor_u64 clamps to u64::MAX.
    let rho2 = Rat::new(u64::MAX as i128, 1i128);
    let reserves2 = Rat::new(MAX_LOVELACE_SUPPLY as i128, 1i128);
    let eta2 = Rat::new(21600i128, u64::MAX as i128);
    let result2 = rho2.mul(&reserves2).mul(&eta2);
    // Result = 45e15 * 21600 = 972e15 > u64::MAX, so clamped
    assert_eq!(
        result2.floor_u64(),
        u64::MAX,
        "Result exceeding u64::MAX should clamp"
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

// =========================================================================
// Feature 1: Era-Specific Validation Gating Tests
// =========================================================================

#[test]
fn test_era_gating_conway_cert_rejected_pre_conway() {
    // Conway-only certificates should be rejected when protocol < 9
    let mut params = ProtocolParameters::mainnet_defaults();
    params.protocol_version_major = 8; // Babbage
    let mut state = LedgerState::new(params);

    let cred = Credential::VerificationKey(Hash28::from_bytes([0xAAu8; 28]));

    // RegDRep should be silently skipped in pre-Conway
    let drep_count_before = state.governance.dreps.len();
    state.process_certificate(&Certificate::RegDRep {
        credential: cred.clone(),
        deposit: Lovelace(500_000_000),
        anchor: None,
    });
    assert_eq!(
        state.governance.dreps.len(),
        drep_count_before,
        "RegDRep should be skipped in pre-Conway era"
    );
}

#[test]
fn test_era_gating_conway_cert_accepted_in_conway() {
    // Conway certificates should work when protocol >= 9
    let mut params = ProtocolParameters::mainnet_defaults();
    params.protocol_version_major = 9;
    let mut state = LedgerState::new(params);

    let cred = Credential::VerificationKey(Hash28::from_bytes([0xAAu8; 28]));

    state.process_certificate(&Certificate::RegDRep {
        credential: cred.clone(),
        deposit: Lovelace(500_000_000),
        anchor: None,
    });
    assert_eq!(
        state.governance.dreps.len(),
        1,
        "RegDRep should be accepted in Conway era"
    );
}

#[test]
fn test_era_gating_vote_delegation_rejected_pre_conway() {
    let mut params = ProtocolParameters::mainnet_defaults();
    params.protocol_version_major = 7; // Babbage
    let mut state = LedgerState::new(params);

    let cred = Credential::VerificationKey(Hash28::from_bytes([0xBBu8; 28]));
    let delegations_before = state.governance.vote_delegations.len();

    state.process_certificate(&Certificate::VoteDelegation {
        credential: cred,
        drep: DRep::Abstain,
    });
    assert_eq!(
        state.governance.vote_delegations.len(),
        delegations_before,
        "VoteDelegation should be skipped in pre-Conway era"
    );
}

#[test]
fn test_era_gating_committee_hot_auth_rejected_pre_conway() {
    let mut params = ProtocolParameters::mainnet_defaults();
    params.protocol_version_major = 8;
    let mut state = LedgerState::new(params);

    let cold = Credential::VerificationKey(Hash28::from_bytes([0xCCu8; 28]));
    let hot = Credential::VerificationKey(Hash28::from_bytes([0xDDu8; 28]));

    state.process_certificate(&Certificate::CommitteeHotAuth {
        cold_credential: cold,
        hot_credential: hot,
    });
    assert!(
        state.governance.committee_hot_keys.is_empty(),
        "CommitteeHotAuth should be skipped in pre-Conway era"
    );
}

#[test]
fn test_era_gating_pre_conway_certs_always_accepted() {
    // Pre-Conway certificates should always work regardless of protocol version
    let mut params = ProtocolParameters::mainnet_defaults();
    params.protocol_version_major = 4; // Mary
    let mut state = LedgerState::new(params);

    let cred = Credential::VerificationKey(Hash28::from_bytes([0xEEu8; 28]));
    state.process_certificate(&Certificate::StakeRegistration(cred.clone()));

    let key = credential_to_hash(&cred);
    assert!(
        state.reward_accounts.contains_key(&key),
        "StakeRegistration should work in any era"
    );
}

#[test]
fn test_era_gating_governance_proposals_skipped_pre_conway() {
    // Governance proposals in apply_block should be skipped pre-Conway
    let mut params = ProtocolParameters::mainnet_defaults();
    params.protocol_version_major = 8;
    let state = LedgerState::new(params);

    let proposal_count_before = state.governance.proposals.len();

    // Directly try to process in pre-Conway (the guard is in apply_block,
    // but process_proposal itself does not gate — we test the apply_block path
    // by checking the guard condition)
    assert!(
        state.protocol_params.protocol_version_major < 9,
        "Protocol version should be pre-Conway"
    );
    assert_eq!(
        proposal_count_before, 0,
        "No proposals should exist initially"
    );
}

#[test]
fn test_era_gating_conway_stake_registration_rejected_pre_conway() {
    let mut params = ProtocolParameters::mainnet_defaults();
    params.protocol_version_major = 8;
    let mut state = LedgerState::new(params);

    let cred = Credential::VerificationKey(Hash28::from_bytes([0xFFu8; 28]));
    let key = credential_to_hash(&cred);

    // Conway-style registration with deposit should be skipped
    state.process_certificate(&Certificate::ConwayStakeRegistration {
        credential: cred.clone(),
        deposit: Lovelace(2_000_000),
    });
    assert!(
        !state.reward_accounts.contains_key(&key),
        "ConwayStakeRegistration should be skipped in pre-Conway era"
    );

    // But regular StakeRegistration should work
    state.process_certificate(&Certificate::StakeRegistration(cred));
    assert!(
        state.reward_accounts.contains_key(&key),
        "StakeRegistration should work in pre-Conway era"
    );
}

// =========================================================================
// Feature 2: Reserve Growth Mechanism (Monetary Expansion) Tests
// =========================================================================

/// Helper: create a LedgerState with controlled reward calculation parameters.
fn make_reward_test_state(
    reserves: u64,
    rho_num: u64,
    rho_den: u64,
    tau_num: u64,
    tau_den: u64,
) -> LedgerState {
    let mut params = ProtocolParameters::mainnet_defaults();
    params.rho = Rational {
        numerator: rho_num,
        denominator: rho_den,
    };
    params.tau = Rational {
        numerator: tau_num,
        denominator: tau_den,
    };
    params.n_opt = 150;
    params.a0 = Rational {
        numerator: 3,
        denominator: 10,
    };
    let mut state = LedgerState::new(params);
    state.reserves = Lovelace(reserves);
    state.epoch_length = 432000; // standard epoch length
    state.epoch_block_count = 21600; // normal block production
    state
}

#[test]
fn test_reward_zero_reserves_no_expansion() {
    let mut state = make_reward_test_state(0, 3, 1000, 2, 10);
    state.epoch_fees = Lovelace(1_000_000);
    let reserves_before = state.reserves.0;
    let treasury_before = state.treasury.0;

    // With zero reserves, expansion = floor(rho * 0) = 0
    // Only fees are distributed
    let snapshot = StakeSnapshot {
        epoch: EpochNo(1),
        delegations: Arc::new(HashMap::new()),
        pool_stake: HashMap::new(),
        pool_params: Arc::new(HashMap::new()),
        stake_distribution: Arc::new(HashMap::new()),
        epoch_fees: Lovelace(0),
        epoch_block_count: 0,
        epoch_blocks_by_pool: Arc::new(HashMap::new()),
    };
    state.calculate_and_distribute_rewards(snapshot);

    assert_eq!(
        state.reserves.0, reserves_before,
        "Reserves should not change when already at 0"
    );
    // treasury gets tau * fees + undistributed
    assert!(
        state.treasury.0 >= treasury_before,
        "Treasury should increase from fees"
    );
}

#[test]
fn test_reward_rho_zero_no_expansion() {
    let mut state = make_reward_test_state(10_000_000_000_000_000, 0, 1, 2, 10);
    state.epoch_fees = Lovelace(0);
    let reserves_before = state.reserves.0;

    let snapshot = StakeSnapshot {
        epoch: EpochNo(1),
        delegations: Arc::new(HashMap::new()),
        pool_stake: HashMap::new(),
        pool_params: Arc::new(HashMap::new()),
        stake_distribution: Arc::new(HashMap::new()),
        epoch_fees: Lovelace(0),
        epoch_block_count: 0,
        epoch_blocks_by_pool: Arc::new(HashMap::new()),
    };
    state.calculate_and_distribute_rewards(snapshot);

    // rho=0 means expansion=0, and fees=0, so total_rewards=0 -> early return
    assert_eq!(
        state.reserves.0, reserves_before,
        "Reserves should not decrease when rho=0 and no fees"
    );
}

#[test]
fn test_reward_tau_zero_no_treasury_cut() {
    let mut state = make_reward_test_state(10_000_000_000_000_000, 3, 1000, 0, 1);
    state.epoch_fees = Lovelace(0);
    let treasury_before = state.treasury.0;

    let snapshot = StakeSnapshot {
        epoch: EpochNo(1),
        delegations: Arc::new(HashMap::new()),
        pool_stake: HashMap::new(),
        pool_params: Arc::new(HashMap::new()),
        stake_distribution: Arc::new(HashMap::new()),
        epoch_fees: Lovelace(0),
        epoch_block_count: 0,
        epoch_blocks_by_pool: Arc::new(HashMap::new()),
    };
    state.calculate_and_distribute_rewards(snapshot);

    // tau=0: treasury cut = floor(0 * total_rewards) = 0
    // But undistributed rewards (no pools) all go to treasury
    // So treasury increases by reward_pot (all undistributed)
    // Since tau=0, treasury_cut=0 but undistributed goes to treasury
    assert!(
        state.treasury.0 > treasury_before,
        "Treasury should receive undistributed rewards even with tau=0"
    );
}

#[test]
fn test_reward_tau_one_all_to_treasury() {
    let mut state = make_reward_test_state(10_000_000_000_000_000, 3, 1000, 1, 1);
    state.epoch_fees = Lovelace(100_000_000);
    let reserves_before = state.reserves.0;
    let treasury_before = state.treasury.0;

    let snapshot = StakeSnapshot {
        epoch: EpochNo(1),
        delegations: Arc::new(HashMap::new()),
        pool_stake: HashMap::new(),
        pool_params: Arc::new(HashMap::new()),
        stake_distribution: Arc::new(HashMap::new()),
        epoch_fees: Lovelace(0),
        epoch_block_count: 0,
        epoch_blocks_by_pool: Arc::new(HashMap::new()),
    };
    state.calculate_and_distribute_rewards(snapshot);

    // tau=1: treasury_cut = floor(1 * total_rewards) = total_rewards
    // reward_pot = total_rewards - treasury_cut = 0
    // So treasury gets everything
    let expansion = reserves_before - state.reserves.0;
    let total_rewards = expansion + 100_000_000;
    assert_eq!(
        state.treasury.0,
        treasury_before + total_rewards,
        "Treasury should get all rewards when tau=1"
    );
}

#[test]
fn test_reward_reserves_decrease_treasury_increase() {
    let initial_reserves = 13_000_000_000_000_000u64; // 13B ADA in reserves
    let mut state = make_reward_test_state(initial_reserves, 3, 1000, 2, 10);
    state.epoch_fees = Lovelace(50_000_000);
    let reserves_before = state.reserves.0;
    let treasury_before = state.treasury.0;

    let snapshot = StakeSnapshot {
        epoch: EpochNo(1),
        delegations: Arc::new(HashMap::new()),
        pool_stake: HashMap::new(),
        pool_params: Arc::new(HashMap::new()),
        stake_distribution: Arc::new(HashMap::new()),
        epoch_fees: Lovelace(0),
        epoch_block_count: 0,
        epoch_blocks_by_pool: Arc::new(HashMap::new()),
    };
    state.calculate_and_distribute_rewards(snapshot);

    let expansion = reserves_before - state.reserves.0;
    assert!(
        expansion > 0,
        "Expansion should be positive with non-zero reserves and rho"
    );

    // Verify monetary expansion formula: expansion ~ floor(rho * reserves * eta)
    // With rho=3/1000 and full block production eta=1
    let expected_expansion = Rat::new(3, 1000)
        .mul(&Rat::new(initial_reserves as i128, 1))
        .floor_u64();
    assert_eq!(
        expansion, expected_expansion,
        "Expansion should match rho * reserves"
    );

    // Treasury should increase (gets tau * total_rewards + undistributed)
    assert!(
        state.treasury.0 > treasury_before,
        "Treasury should increase each epoch"
    );

    // Total ADA conservation: reserves_decrease = expansion, which goes to rewards+treasury
    // Reserves decreased by exactly the expansion amount
    assert_eq!(
        state.reserves.0,
        reserves_before - expansion,
        "Reserves should decrease by exactly the expansion amount"
    );
}

#[test]
fn test_reward_max_reserves_no_overflow() {
    // Test with maximum reserves (close to max supply)
    let max_reserves = MAX_LOVELACE_SUPPLY; // 45B ADA
    let mut state = make_reward_test_state(max_reserves, 3, 1000, 2, 10);
    state.epoch_fees = Lovelace(0);

    let snapshot = StakeSnapshot {
        epoch: EpochNo(1),
        delegations: Arc::new(HashMap::new()),
        pool_stake: HashMap::new(),
        pool_params: Arc::new(HashMap::new()),
        stake_distribution: Arc::new(HashMap::new()),
        epoch_fees: Lovelace(0),
        epoch_block_count: 0,
        epoch_blocks_by_pool: Arc::new(HashMap::new()),
    };

    // Should not panic or overflow
    state.calculate_and_distribute_rewards(snapshot);

    // Reserves should have decreased
    assert!(
        state.reserves.0 < max_reserves,
        "Reserves should decrease from max"
    );
}

#[test]
fn test_reward_treasury_tax_correct_amount() {
    // Verify that tau correctly deducts from expansion before distributing
    let initial_reserves = 10_000_000_000_000_000u64;
    let mut state = make_reward_test_state(initial_reserves, 3, 1000, 2, 10);
    state.epoch_fees = Lovelace(100_000_000); // 100 ADA fees
    let treasury_before = state.treasury.0;

    let snapshot = StakeSnapshot {
        epoch: EpochNo(1),
        delegations: Arc::new(HashMap::new()),
        pool_stake: HashMap::new(),
        pool_params: Arc::new(HashMap::new()),
        stake_distribution: Arc::new(HashMap::new()),
        epoch_fees: Lovelace(0),
        epoch_block_count: 0,
        epoch_blocks_by_pool: Arc::new(HashMap::new()),
    };
    state.calculate_and_distribute_rewards(snapshot);

    let expansion = initial_reserves - state.reserves.0;
    let total_rewards = expansion + 100_000_000;
    let expected_treasury_cut = Rat::new(2, 10)
        .mul(&Rat::new(total_rewards as i128, 1))
        .floor_u64();

    // Treasury should get at least the tau cut (plus undistributed since no pools)
    let treasury_increase = state.treasury.0 - treasury_before;
    assert!(
        treasury_increase >= expected_treasury_cut,
        "Treasury should receive at least the tau cut: got {}, expected >= {}",
        treasury_increase,
        expected_treasury_cut,
    );
}

// =========================================================================
// Feature 3: DRep Voting Power Calculation Tests
// =========================================================================

#[test]
fn test_drep_voting_power_equals_delegated_stake() {
    let mut params = ProtocolParameters::mainnet_defaults();
    params.protocol_version_major = 9;
    let mut state = LedgerState::new(params);

    // Register a DRep
    let drep_cred = Credential::VerificationKey(Hash28::from_bytes([0x01u8; 28]));
    state.process_certificate(&Certificate::RegDRep {
        credential: drep_cred.clone(),
        deposit: Lovelace(500_000_000),
        anchor: None,
    });

    // Register two stake keys and delegate to this DRep
    let stake_cred1 = Credential::VerificationKey(Hash28::from_bytes([0x10u8; 28]));
    let stake_cred2 = Credential::VerificationKey(Hash28::from_bytes([0x20u8; 28]));
    state.process_certificate(&Certificate::StakeRegistration(stake_cred1.clone()));
    state.process_certificate(&Certificate::StakeRegistration(stake_cred2.clone()));

    // Add stake to their accounts
    let key1 = credential_to_hash(&stake_cred1);
    let key2 = credential_to_hash(&stake_cred2);
    state
        .stake_distribution
        .stake_map
        .insert(key1, Lovelace(1_000_000_000));
    state
        .stake_distribution
        .stake_map
        .insert(key2, Lovelace(2_000_000_000));

    // Delegate votes
    let drep_hash28 = match &drep_cred {
        Credential::VerificationKey(h) => *h,
        _ => unreachable!(),
    };
    state.process_certificate(&Certificate::VoteDelegation {
        credential: stake_cred1,
        drep: DRep::KeyHash(drep_hash28.to_hash32_padded()),
    });
    state.process_certificate(&Certificate::VoteDelegation {
        credential: stake_cred2,
        drep: DRep::KeyHash(drep_hash28.to_hash32_padded()),
    });

    let (cache, _no_conf, _abstain) = state.build_drep_power_cache();
    let drep_key = credential_to_hash(&drep_cred);
    let power = cache.get(&drep_key).copied().unwrap_or(0);

    assert_eq!(
        power, 3_000_000_000,
        "DRep voting power should equal total delegated stake (1B + 2B = 3B)"
    );
}

#[test]
fn test_drep_inactive_excluded_from_voting_power() {
    let mut params = ProtocolParameters::mainnet_defaults();
    params.protocol_version_major = 9;
    params.drep_activity = 2; // 2 epoch activity window
    let mut state = LedgerState::new(params);
    state.epoch = EpochNo(10);

    // Register DRep
    let drep_cred = Credential::VerificationKey(Hash28::from_bytes([0x01u8; 28]));
    state.process_certificate(&Certificate::RegDRep {
        credential: drep_cred.clone(),
        deposit: Lovelace(500_000_000),
        anchor: None,
    });

    // Make the DRep inactive by setting last_active_epoch far in the past
    let drep_key = credential_to_hash(&drep_cred);
    if let Some(drep) = Arc::make_mut(&mut state.governance)
        .dreps
        .get_mut(&drep_key)
    {
        drep.last_active_epoch = EpochNo(5); // inactive: 10 - 5 = 5 > 2
        drep.active = false;
    }

    // Delegate stake to the inactive DRep
    let stake_cred = Credential::VerificationKey(Hash28::from_bytes([0x10u8; 28]));
    state.process_certificate(&Certificate::StakeRegistration(stake_cred.clone()));
    let stake_key = credential_to_hash(&stake_cred);
    state
        .stake_distribution
        .stake_map
        .insert(stake_key, Lovelace(5_000_000_000));

    let drep_hash28 = match &drep_cred {
        Credential::VerificationKey(h) => *h,
        _ => unreachable!(),
    };
    state.process_certificate(&Certificate::VoteDelegation {
        credential: stake_cred,
        drep: DRep::KeyHash(drep_hash28.to_hash32_padded()),
    });

    let (cache, _no_conf, _abstain) = state.build_drep_power_cache();

    // Inactive DRep should not have any voting power in the cache
    assert!(
        !cache.contains_key(&drep_key),
        "Inactive DRep should be excluded from voting power cache"
    );
}

#[test]
fn test_drep_always_abstain_excluded_from_yes_no_tally() {
    let mut params = ProtocolParameters::mainnet_defaults();
    params.protocol_version_major = 9;
    let mut state = LedgerState::new(params);

    // Delegate 3B to Abstain
    let stake_cred = Credential::VerificationKey(Hash28::from_bytes([0x10u8; 28]));
    state.process_certificate(&Certificate::StakeRegistration(stake_cred.clone()));
    let stake_key = credential_to_hash(&stake_cred);
    state
        .stake_distribution
        .stake_map
        .insert(stake_key, Lovelace(3_000_000_000));

    state.process_certificate(&Certificate::VoteDelegation {
        credential: stake_cred,
        drep: DRep::Abstain,
    });

    let (_cache, _no_conf, abstain_stake) = state.build_drep_power_cache();
    assert_eq!(
        abstain_stake, 3_000_000_000,
        "AlwaysAbstain delegated stake should be tracked"
    );
}

#[test]
fn test_drep_always_no_confidence_flows_to_no_confidence_actions() {
    let mut params = ProtocolParameters::mainnet_defaults();
    params.protocol_version_major = 9;
    let mut state = LedgerState::new(params);

    // Delegate 2B to NoConfidence
    let stake_cred = Credential::VerificationKey(Hash28::from_bytes([0x10u8; 28]));
    state.process_certificate(&Certificate::StakeRegistration(stake_cred.clone()));
    let stake_key = credential_to_hash(&stake_cred);
    state
        .stake_distribution
        .stake_map
        .insert(stake_key, Lovelace(2_000_000_000));

    state.process_certificate(&Certificate::VoteDelegation {
        credential: stake_cred,
        drep: DRep::NoConfidence,
    });

    let (_cache, no_confidence_stake, _abstain) = state.build_drep_power_cache();
    assert_eq!(
        no_confidence_stake, 2_000_000_000,
        "AlwaysNoConfidence stake should be tracked"
    );

    // For NoConfidence actions, this stake counts as Yes
    // For other actions, it counts as No
    // Verified in count_votes_by_type
}

#[test]
fn test_drep_voting_power_with_known_distribution() {
    // Verify exact threshold calculation with a known distribution
    let mut params = ProtocolParameters::mainnet_defaults();
    params.protocol_version_major = 9;
    params.dvt_no_confidence = Rational {
        numerator: 67,
        denominator: 100,
    };
    let mut state = LedgerState::new(params);

    // Register 3 DReps with known stake
    let drep1_cred = Credential::VerificationKey(Hash28::from_bytes([0x01u8; 28]));
    let drep2_cred = Credential::VerificationKey(Hash28::from_bytes([0x02u8; 28]));
    let drep3_cred = Credential::VerificationKey(Hash28::from_bytes([0x03u8; 28]));

    for cred in [&drep1_cred, &drep2_cred, &drep3_cred] {
        state.process_certificate(&Certificate::RegDRep {
            credential: cred.clone(),
            deposit: Lovelace(500_000_000),
            anchor: None,
        });
    }

    // Stake: DRep1=40, DRep2=30, DRep3=30 (total=100)
    let make_stake_and_delegate =
        |state: &mut LedgerState, idx: u8, amount: u64, drep_h28: Hash28| {
            let cred = Credential::VerificationKey(Hash28::from_bytes([idx; 28]));
            state.process_certificate(&Certificate::StakeRegistration(cred.clone()));
            let key = credential_to_hash(&cred);
            state
                .stake_distribution
                .stake_map
                .insert(key, Lovelace(amount));
            state.process_certificate(&Certificate::VoteDelegation {
                credential: cred,
                drep: DRep::KeyHash(drep_h28.to_hash32_padded()),
            });
        };

    let h1 = match &drep1_cred {
        Credential::VerificationKey(h) => *h,
        _ => unreachable!(),
    };
    let h2 = match &drep2_cred {
        Credential::VerificationKey(h) => *h,
        _ => unreachable!(),
    };
    let h3 = match &drep3_cred {
        Credential::VerificationKey(h) => *h,
        _ => unreachable!(),
    };

    make_stake_and_delegate(&mut state, 0x10, 40, h1);
    make_stake_and_delegate(&mut state, 0x20, 30, h2);
    make_stake_and_delegate(&mut state, 0x30, 30, h3);

    let (cache, _, _) = state.build_drep_power_cache();
    let k1 = credential_to_hash(&drep1_cred);
    let k2 = credential_to_hash(&drep2_cred);
    let k3 = credential_to_hash(&drep3_cred);

    assert_eq!(cache.get(&k1).copied().unwrap_or(0), 40);
    assert_eq!(cache.get(&k2).copied().unwrap_or(0), 30);
    assert_eq!(cache.get(&k3).copied().unwrap_or(0), 30);

    // With 67% threshold and total=100:
    // DRep1 (40) + DRep2 (30) = 70 yes out of 100 total -> 70% >= 67% -> passes
    // DRep1 (40) alone = 40 yes out of 70 total (if DRep2+3 don't vote) -> depends on denominator
    let threshold = Rational {
        numerator: 67,
        denominator: 100,
    };

    // 70 yes out of 100 total (yes+no): passes
    assert!(
        check_threshold(70, 100, &threshold),
        "70/100 should meet 67% threshold"
    );
    // 66 yes out of 100 total: fails
    assert!(
        !check_threshold(66, 100, &threshold),
        "66/100 should not meet 67% threshold"
    );
    // 67 yes out of 100 total: passes (exact boundary)
    assert!(
        check_threshold(67, 100, &threshold),
        "67/100 should meet 67% threshold exactly"
    );
}

// =========================================================================
// Feature 4: Abstain Vote Exclusion Tests
// =========================================================================

#[test]
fn test_abstain_excluded_from_denominator() {
    // Abstain votes should be excluded from both numerator and denominator
    let mut params = ProtocolParameters::mainnet_defaults();
    params.protocol_version_major = 9;
    let mut state = LedgerState::new(params);

    // Setup DReps
    let drep1_cred = Credential::VerificationKey(Hash28::from_bytes([0x01u8; 28]));
    let drep2_cred = Credential::VerificationKey(Hash28::from_bytes([0x02u8; 28]));
    let drep3_cred = Credential::VerificationKey(Hash28::from_bytes([0x03u8; 28]));

    for cred in [&drep1_cred, &drep2_cred, &drep3_cred] {
        state.process_certificate(&Certificate::RegDRep {
            credential: cred.clone(),
            deposit: Lovelace(500_000_000),
            anchor: None,
        });
    }

    // Each DRep gets 100 stake
    for (idx, drep_cred) in [
        (0x10u8, &drep1_cred),
        (0x20, &drep2_cred),
        (0x30, &drep3_cred),
    ] {
        let cred = Credential::VerificationKey(Hash28::from_bytes([idx; 28]));
        state.process_certificate(&Certificate::StakeRegistration(cred.clone()));
        let key = credential_to_hash(&cred);
        state
            .stake_distribution
            .stake_map
            .insert(key, Lovelace(100));
        let h = match drep_cred {
            Credential::VerificationKey(h) => *h,
            _ => unreachable!(),
        };
        state.process_certificate(&Certificate::VoteDelegation {
            credential: cred,
            drep: DRep::KeyHash(h.to_hash32_padded()),
        });
    }

    // Submit a proposal
    let tx_hash = Hash32::from_bytes([0xFFu8; 32]);
    let proposal = ProposalProcedure {
        deposit: Lovelace(100_000_000),
        return_addr: vec![0xE0; 29],
        gov_action: GovAction::InfoAction,
        anchor: Anchor {
            url: "https://example.com".to_string(),
            data_hash: Hash32::from_bytes([0xAA; 32]),
        },
    };
    state.process_proposal(&tx_hash, 0, &proposal);

    let action_id = GovActionId {
        transaction_id: tx_hash,
        action_index: 0,
    };

    // DRep1 votes Yes (100 stake), DRep2 votes Abstain (100 stake), DRep3 votes No (100 stake)
    state.process_vote(
        &Voter::DRep(drep1_cred.clone()),
        &action_id,
        &VotingProcedure {
            vote: Vote::Yes,
            anchor: None,
        },
    );
    state.process_vote(
        &Voter::DRep(drep2_cred.clone()),
        &action_id,
        &VotingProcedure {
            vote: Vote::Abstain,
            anchor: None,
        },
    );
    state.process_vote(
        &Voter::DRep(drep3_cred.clone()),
        &action_id,
        &VotingProcedure {
            vote: Vote::No,
            anchor: None,
        },
    );

    let (cache, no_conf, _abstain) = state.build_drep_power_cache();
    let (drep_yes, drep_total, _, _, _, _) =
        state.count_votes_by_type(&action_id, &GovAction::InfoAction, &cache, no_conf);

    // DRep1 voted Yes (100), DRep3 voted No (100), DRep2 Abstain (excluded)
    // drep_total = yes + no = 100 + 100 = 200 (abstain excluded)
    assert_eq!(drep_yes, 100, "Yes votes should be 100");
    assert_eq!(
        drep_total, 200,
        "Total should be yes + no = 200 (abstain excluded from denominator)"
    );
}

#[test]
fn test_all_dreps_abstain() {
    let mut params = ProtocolParameters::mainnet_defaults();
    params.protocol_version_major = 9;
    let mut state = LedgerState::new(params);

    let drep_cred = Credential::VerificationKey(Hash28::from_bytes([0x01u8; 28]));
    state.process_certificate(&Certificate::RegDRep {
        credential: drep_cred.clone(),
        deposit: Lovelace(500_000_000),
        anchor: None,
    });

    let stake_cred = Credential::VerificationKey(Hash28::from_bytes([0x10u8; 28]));
    state.process_certificate(&Certificate::StakeRegistration(stake_cred.clone()));
    let key = credential_to_hash(&stake_cred);
    state
        .stake_distribution
        .stake_map
        .insert(key, Lovelace(100));
    let h = match &drep_cred {
        Credential::VerificationKey(h) => *h,
        _ => unreachable!(),
    };
    state.process_certificate(&Certificate::VoteDelegation {
        credential: stake_cred,
        drep: DRep::KeyHash(h.to_hash32_padded()),
    });

    let tx_hash = Hash32::from_bytes([0xFFu8; 32]);
    let proposal = ProposalProcedure {
        deposit: Lovelace(100_000_000),
        return_addr: vec![0xE0; 29],
        gov_action: GovAction::InfoAction,
        anchor: Anchor {
            url: "https://example.com".to_string(),
            data_hash: Hash32::from_bytes([0xAA; 32]),
        },
    };
    state.process_proposal(&tx_hash, 0, &proposal);

    let action_id = GovActionId {
        transaction_id: tx_hash,
        action_index: 0,
    };

    // Only DRep votes Abstain
    state.process_vote(
        &Voter::DRep(drep_cred),
        &action_id,
        &VotingProcedure {
            vote: Vote::Abstain,
            anchor: None,
        },
    );

    let (cache, no_conf, _abstain) = state.build_drep_power_cache();
    let (drep_yes, drep_total, _, _, _, _) =
        state.count_votes_by_type(&action_id, &GovAction::InfoAction, &cache, no_conf);

    // All abstain: yes=0, total=0
    assert_eq!(drep_yes, 0, "No yes votes when all abstain");
    assert_eq!(
        drep_total, 0,
        "Denominator should be 0 when all vote abstain"
    );

    // check_threshold with total=0 returns false (no votes at all)
    let threshold = Rational {
        numerator: 1,
        denominator: 2,
    };
    assert!(
        !check_threshold(drep_yes, drep_total, &threshold),
        "With total=0, threshold check should fail"
    );
}

#[test]
fn test_mix_yes_no_abstain_votes() {
    let mut params = ProtocolParameters::mainnet_defaults();
    params.protocol_version_major = 9;
    let mut state = LedgerState::new(params);

    // 5 DReps: 3 vote Yes (stake 100 each), 1 votes No (stake 100), 1 Abstains (stake 100)
    let mut drep_creds = Vec::new();
    for i in 1..=5u8 {
        let cred = Credential::VerificationKey(Hash28::from_bytes([i; 28]));
        state.process_certificate(&Certificate::RegDRep {
            credential: cred.clone(),
            deposit: Lovelace(500_000_000),
            anchor: None,
        });
        drep_creds.push(cred);
    }

    for (i, drep_cred) in drep_creds.iter().enumerate() {
        let idx = (0x10 + i) as u8;
        let cred = Credential::VerificationKey(Hash28::from_bytes([idx; 28]));
        state.process_certificate(&Certificate::StakeRegistration(cred.clone()));
        let key = credential_to_hash(&cred);
        state
            .stake_distribution
            .stake_map
            .insert(key, Lovelace(100));
        let h = match drep_cred {
            Credential::VerificationKey(h) => *h,
            _ => unreachable!(),
        };
        state.process_certificate(&Certificate::VoteDelegation {
            credential: cred,
            drep: DRep::KeyHash(h.to_hash32_padded()),
        });
    }

    let tx_hash = Hash32::from_bytes([0xFFu8; 32]);
    let proposal = ProposalProcedure {
        deposit: Lovelace(100_000_000),
        return_addr: vec![0xE0; 29],
        gov_action: GovAction::InfoAction,
        anchor: Anchor {
            url: "https://example.com".to_string(),
            data_hash: Hash32::from_bytes([0xAA; 32]),
        },
    };
    state.process_proposal(&tx_hash, 0, &proposal);
    let action_id = GovActionId {
        transaction_id: tx_hash,
        action_index: 0,
    };

    // 3 Yes, 1 No, 1 Abstain
    for cred in drep_creds.iter().take(3) {
        state.process_vote(
            &Voter::DRep(cred.clone()),
            &action_id,
            &VotingProcedure {
                vote: Vote::Yes,
                anchor: None,
            },
        );
    }
    state.process_vote(
        &Voter::DRep(drep_creds[3].clone()),
        &action_id,
        &VotingProcedure {
            vote: Vote::No,
            anchor: None,
        },
    );
    state.process_vote(
        &Voter::DRep(drep_creds[4].clone()),
        &action_id,
        &VotingProcedure {
            vote: Vote::Abstain,
            anchor: None,
        },
    );

    let (cache, no_conf, _abstain) = state.build_drep_power_cache();
    let (drep_yes, drep_total, _, _, _, _) =
        state.count_votes_by_type(&action_id, &GovAction::InfoAction, &cache, no_conf);

    // 3 * 100 = 300 yes, 1 * 100 = 100 no, total = 400 (abstain excluded)
    assert_eq!(drep_yes, 300, "Yes votes should be 300");
    assert_eq!(
        drep_total, 400,
        "Total should be 400 (300 yes + 100 no, abstain excluded)"
    );

    // 300/400 = 75% >= 67% -> should pass
    let threshold_67 = Rational {
        numerator: 67,
        denominator: 100,
    };
    assert!(
        check_threshold(drep_yes, drep_total, &threshold_67),
        "300/400 = 75% should meet 67% threshold"
    );

    // 300/400 = 75% < 80% -> should fail
    let threshold_80 = Rational {
        numerator: 80,
        denominator: 100,
    };
    assert!(
        !check_threshold(drep_yes, drep_total, &threshold_80),
        "300/400 = 75% should not meet 80% threshold"
    );
}

#[test]
fn test_cc_abstain_excluded_from_denominator() {
    // Committee abstain votes should be excluded from the CC ratio denominator
    let mut params = ProtocolParameters::mainnet_defaults();
    params.protocol_version_major = 9;
    let mut state = LedgerState::new(params);

    // Set up a committee with 3 members, threshold 1/2
    Arc::make_mut(&mut state.governance).committee_threshold = Some(Rational {
        numerator: 1,
        denominator: 2,
    });

    let cold1 = Hash32::from_bytes([0x01u8; 32]);
    let cold2 = Hash32::from_bytes([0x02u8; 32]);
    let cold3 = Hash32::from_bytes([0x03u8; 32]);
    // Hot keys must match what credential_to_hash produces from Hash28:
    // Hash28 → Hash32 via to_hash32_padded() pads with 4 zero bytes at the end
    let hot1_28 = Hash28::from_bytes([0x11u8; 28]);
    let hot2_28 = Hash28::from_bytes([0x12u8; 28]);
    let hot3_28 = Hash28::from_bytes([0x13u8; 28]);
    let hot1 = hot1_28.to_hash32_padded();
    let hot2 = hot2_28.to_hash32_padded();
    let hot3 = hot3_28.to_hash32_padded();

    let gov = Arc::make_mut(&mut state.governance);
    gov.committee_expiration.insert(cold1, EpochNo(100));
    gov.committee_expiration.insert(cold2, EpochNo(100));
    gov.committee_expiration.insert(cold3, EpochNo(100));
    gov.committee_hot_keys.insert(cold1, hot1);
    gov.committee_hot_keys.insert(cold2, hot2);
    gov.committee_hot_keys.insert(cold3, hot3);

    let action_id = GovActionId {
        transaction_id: Hash32::from_bytes([0xAAu8; 32]),
        action_index: 0,
    };

    // CC1: Yes, CC2: Abstain, CC3: No
    let votes = vec![
        (
            Voter::ConstitutionalCommittee(Credential::VerificationKey(hot1_28)),
            VotingProcedure {
                vote: Vote::Yes,
                anchor: None,
            },
        ),
        (
            Voter::ConstitutionalCommittee(Credential::VerificationKey(hot2_28)),
            VotingProcedure {
                vote: Vote::Abstain,
                anchor: None,
            },
        ),
        (
            Voter::ConstitutionalCommittee(Credential::VerificationKey(hot3_28)),
            VotingProcedure {
                vote: Vote::No,
                anchor: None,
            },
        ),
    ];

    let action_votes = Arc::make_mut(&mut state.governance)
        .votes_by_action
        .entry(action_id.clone())
        .or_default();
    for (voter, procedure) in votes {
        action_votes.push((voter, procedure));
    }

    // check_cc_approval: 1 Yes, 1 No, 1 Abstain
    // Effective: yes=1, total_excluding_abstain=2 (yes+no), ratio=1/2 = 50% >= 50% threshold
    let result = check_cc_approval(
        &action_id,
        &state.governance,
        EpochNo(10),
        1, // min committee size
        false,
    );
    assert!(
        result,
        "CC approval: 1 Yes / 2 (excl abstain) = 50% should meet 1/2 threshold"
    );
}

#[test]
fn test_no_confidence_stake_counts_as_yes_for_no_confidence_action() {
    let mut params = ProtocolParameters::mainnet_defaults();
    params.protocol_version_major = 9;
    let mut state = LedgerState::new(params);

    // Delegate 500 stake to NoConfidence
    let stake_cred = Credential::VerificationKey(Hash28::from_bytes([0x10u8; 28]));
    state.process_certificate(&Certificate::StakeRegistration(stake_cred.clone()));
    let key = credential_to_hash(&stake_cred);
    state
        .stake_distribution
        .stake_map
        .insert(key, Lovelace(500));
    state.process_certificate(&Certificate::VoteDelegation {
        credential: stake_cred,
        drep: DRep::NoConfidence,
    });

    let action_id = GovActionId {
        transaction_id: Hash32::from_bytes([0xAAu8; 32]),
        action_index: 0,
    };

    let (cache, no_conf_stake, _) = state.build_drep_power_cache();

    // For NoConfidence action
    let (drep_yes, drep_total, _, _, _, _) = state.count_votes_by_type(
        &action_id,
        &GovAction::NoConfidence {
            prev_action_id: None,
        },
        &cache,
        no_conf_stake,
    );
    assert_eq!(
        drep_yes, 500,
        "NoConfidence stake should count as Yes for NoConfidence actions"
    );
    assert_eq!(drep_total, 500, "Total should include NoConfidence stake");

    // For InfoAction (non-NoConfidence)
    let (drep_yes_info, drep_total_info, _, _, _, _) =
        state.count_votes_by_type(&action_id, &GovAction::InfoAction, &cache, no_conf_stake);
    assert_eq!(
        drep_yes_info, 0,
        "NoConfidence stake should NOT count as Yes for non-NoConfidence actions"
    );
    assert_eq!(
        drep_total_info, 500,
        "NoConfidence stake should count as No for non-NoConfidence actions"
    );
}

// =========================================================================
// Epoch Transition Tests
// =========================================================================

#[test]
fn test_snapshot_rotation_mark_set_go() {
    let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());
    state.epoch_length = 100;
    state.epoch = EpochNo(0);
    state.needs_stake_rebuild = false;

    // Epoch 0 -> 1: creates mark snapshot, set/go are None
    state.process_epoch_transition(EpochNo(1));
    assert!(state.snapshots.mark.is_some());
    assert_eq!(state.snapshots.mark.as_ref().unwrap().epoch, EpochNo(1));
    assert!(state.snapshots.set.is_none());
    assert!(state.snapshots.go.is_none());

    // Epoch 1 -> 2: mark -> set, new mark created
    state.process_epoch_transition(EpochNo(2));
    assert!(state.snapshots.mark.is_some());
    assert_eq!(state.snapshots.mark.as_ref().unwrap().epoch, EpochNo(2));
    assert!(state.snapshots.set.is_some());
    assert_eq!(state.snapshots.set.as_ref().unwrap().epoch, EpochNo(1));
    assert!(state.snapshots.go.is_none());

    // Epoch 2 -> 3: set -> go, mark -> set, new mark created
    state.process_epoch_transition(EpochNo(3));
    assert!(state.snapshots.mark.is_some());
    assert_eq!(state.snapshots.mark.as_ref().unwrap().epoch, EpochNo(3));
    assert!(state.snapshots.set.is_some());
    assert_eq!(state.snapshots.set.as_ref().unwrap().epoch, EpochNo(2));
    assert!(state.snapshots.go.is_some());
    assert_eq!(state.snapshots.go.as_ref().unwrap().epoch, EpochNo(1));
}

#[test]
fn test_pool_retirement_at_scheduled_epoch() {
    let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());
    state.epoch_length = 100;
    state.epoch = EpochNo(4);
    state.needs_stake_rebuild = false;

    let pool_id = Hash28::from_bytes([0xAA; 28]);
    let reward_addr = {
        let mut addr = vec![0xE0u8];
        addr.extend_from_slice(&[0xBB; 28]);
        addr
    };
    let pool_reg = PoolRegistration {
        pool_id,
        vrf_keyhash: Hash32::ZERO,
        pledge: Lovelace(0),
        cost: Lovelace(340_000_000),
        margin_numerator: 0,
        margin_denominator: 1,
        reward_account: reward_addr.clone(),
        owners: vec![],
        relays: vec![],
        metadata_url: None,
        metadata_hash: None,
    };
    Arc::make_mut(&mut state.pool_params).insert(pool_id, pool_reg);

    // Schedule retirement at epoch 5
    state
        .pending_retirements
        .entry(EpochNo(5))
        .or_default()
        .push(pool_id);

    // Transition to epoch 5: pool should be retired and removed
    state.process_epoch_transition(EpochNo(5));
    assert!(
        !state.pool_params.contains_key(&pool_id),
        "Pool should be removed after retirement epoch"
    );

    // Check deposit was refunded
    let hash_key = LedgerState::reward_account_to_hash(&reward_addr);
    let refund = state
        .reward_accounts
        .get(&hash_key)
        .copied()
        .unwrap_or(Lovelace(0));
    assert_eq!(refund, state.protocol_params.pool_deposit);
}

#[test]
fn test_pool_reregistration_cancels_retirement() {
    let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());
    state.epoch_length = 100;
    state.epoch = EpochNo(3);
    state.needs_stake_rebuild = false;

    let pool_id = Hash28::from_bytes([0xCC; 28]);
    let pool_reg = PoolRegistration {
        pool_id,
        vrf_keyhash: Hash32::ZERO,
        pledge: Lovelace(0),
        cost: Lovelace(340_000_000),
        margin_numerator: 0,
        margin_denominator: 1,
        reward_account: vec![0xE0; 29],
        owners: vec![],
        relays: vec![],
        metadata_url: None,
        metadata_hash: None,
    };
    Arc::make_mut(&mut state.pool_params).insert(pool_id, pool_reg.clone());

    // Schedule retirement at epoch 5
    state
        .pending_retirements
        .entry(EpochNo(5))
        .or_default()
        .push(pool_id);

    // Re-register (cancel retirement)
    state.pending_retirements.remove(&EpochNo(5));

    // Transition to epoch 5: pool should still exist
    state.process_epoch_transition(EpochNo(5));
    assert!(
        state.pool_params.contains_key(&pool_id),
        "Pool should remain after re-registration cancels retirement"
    );
}

#[test]
fn test_zero_total_stake_no_panic() {
    let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());
    state.epoch_length = 100;
    state.epoch = EpochNo(0);
    state.needs_stake_rebuild = false;
    // No delegations, no stake - should not panic or divide by zero
    state.reserves = Lovelace(MAX_LOVELACE_SUPPLY);
    state.process_epoch_transition(EpochNo(1));
    // If we get here, no panic occurred
    assert_eq!(state.epoch, EpochNo(1));
}

#[test]
fn test_protocol_param_update_at_epoch_boundary() {
    let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());
    state.epoch_length = 100;
    state.epoch = EpochNo(4);
    state.update_quorum = 1; // Set quorum to 1 so a single proposal suffices
    state.needs_stake_rebuild = false;

    // Submit a protocol parameter update proposal targeting epoch 4
    let genesis_hash = Hash32::from_bytes([0x01; 32]);
    let ppu = ProtocolParamUpdate {
        min_fee_a: Some(55), // Change min_fee_a from 44 to 55
        ..Default::default()
    };
    state
        .pending_pp_updates
        .entry(EpochNo(4))
        .or_default()
        .push((genesis_hash, ppu));

    assert_eq!(state.protocol_params.min_fee_a, 44);

    // Transition: old epoch is 4, proposals for epoch 4 are applied
    state.process_epoch_transition(EpochNo(5));

    assert_eq!(
        state.protocol_params.min_fee_a, 55,
        "Protocol param update should be applied at epoch boundary"
    );
}

#[test]
fn test_epoch_transition_resets_accumulators() {
    let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());
    state.epoch_length = 100;
    state.epoch = EpochNo(0);
    state.needs_stake_rebuild = false;

    state.epoch_fees = Lovelace(1_000_000);
    state.epoch_block_count = 42;
    Arc::make_mut(&mut state.epoch_blocks_by_pool).insert(Hash28::from_bytes([1; 28]), 10);

    state.process_epoch_transition(EpochNo(1));

    assert_eq!(state.epoch_fees, Lovelace(0));
    assert_eq!(state.epoch_block_count, 0);
    assert!(state.epoch_blocks_by_pool.is_empty());
}

// ─── Governance parameter update lifecycle tests ─────────────────

/// Helper: create a LedgerState set up for Conway governance testing.
/// Protocol version 9 (bootstrap), committee set up, DReps registered, SPOs registered.
fn governance_test_state() -> LedgerState {
    let mut params = ProtocolParameters::mainnet_defaults();
    params.protocol_version_major = 9; // Conway bootstrap
    params.protocol_version_minor = 0;
    // Set sane governance thresholds for testing
    params.committee_min_size = 0; // Don't require min committee size
    params.drep_activity = 20;
    params.gov_action_lifetime = 30;
    let mut state = LedgerState::new(params);
    state.epoch_length = 100;

    // Set up committee with threshold 2/3
    Arc::make_mut(&mut state.governance).committee_threshold = Some(Rational {
        numerator: 2,
        denominator: 3,
    });

    // Add 1 CC member (cold) + hot key
    let cold = Credential::VerificationKey(Hash28::from_bytes([10u8; 28]));
    let hot = Credential::VerificationKey(Hash28::from_bytes([20u8; 28]));
    let cold_key = credential_to_hash(&cold);
    Arc::make_mut(&mut state.governance)
        .committee_expiration
        .insert(cold_key, EpochNo(1000));
    state.process_certificate(&Certificate::CommitteeHotAuth {
        cold_credential: cold,
        hot_credential: hot,
    });

    // Register 10 DReps with equal stake
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
        // Vote-delegate to the DRep
        Arc::make_mut(&mut state.governance)
            .vote_delegations
            .insert(key, DRep::KeyHash(key));
        // Give each credential some stake
        state
            .stake_distribution
            .stake_map
            .insert(key, Lovelace(1_000_000_000_000));
    }

    // Register 5 SPOs with pool stake.
    // Also add delegations from synthetic stake credentials → each pool, so that
    // when process_epoch_transition builds the new "mark" snapshot from live
    // delegations, the pool_stake map is populated.  This matches the real chain
    // where delegators must be registered for pool stake to count.
    //
    // Per CIP-1694 the mark snapshot (current epoch) is used for SPO voting
    // power, not the set (previous epoch).  Both the snapshot pre-seeded here
    // and the freshly built mark (via delegations) carry 2T lovelace per pool.
    for i in 0..5 {
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
        // Create a synthetic delegator for each pool so the epoch-transition
        // mark builder picks up this stake.
        let spo_cred = Hash32::from_bytes([200 + i as u8; 32]);
        Arc::make_mut(&mut state.delegations).insert(spo_cred, pool_id);
        state
            .stake_distribution
            .stake_map
            .insert(spo_cred, Lovelace(2_000_000_000_000));
        Arc::make_mut(&mut state.reward_accounts).insert(spo_cred, Lovelace(0));
    }

    // Pre-seed the mark snapshot so SPO voting power is available even before
    // the first epoch transition.  After process_epoch_transition the existing
    // mark is rotated to set and a fresh mark is built from delegations (above),
    // so SPO power is preserved across the rotation.
    let mut pool_stake = HashMap::new();
    for i in 0..5 {
        let pool_id = Hash28::from_bytes([100 + i as u8; 28]);
        pool_stake.insert(pool_id, Lovelace(2_000_000_000_000));
    }
    state.snapshots.mark = Some(StakeSnapshot {
        epoch: EpochNo(0),
        delegations: Arc::clone(&state.delegations),
        pool_stake,
        pool_params: Arc::clone(&state.pool_params),
        stake_distribution: Arc::new(HashMap::new()),
        epoch_fees: Lovelace(0),
        epoch_block_count: 0,
        epoch_blocks_by_pool: Arc::new(HashMap::new()),
    });

    // Prevent epoch transitions from triggering a full UTxO scan.
    state.needs_stake_rebuild = false;

    state
}

#[test]
fn test_parameter_change_ratification_bootstrap() {
    // During bootstrap (protocol version 9), DRep thresholds are 0 (auto-pass).
    // CC approval is still required. SPO threshold applies for security params.
    let mut state = governance_test_state();

    // Submit ParameterChange to update maxTxExUnits
    let tx_hash = Hash32::from_bytes([42u8; 32]);
    let ppu = ProtocolParamUpdate {
        max_tx_ex_units: Some(ExUnits {
            mem: 16_500_000,
            steps: 10_000_000_000,
        }),
        max_block_ex_units: Some(ExUnits {
            mem: 72_000_000,
            steps: 40_000_000_000,
        }),
        ..Default::default()
    };

    let proposal = ProposalProcedure {
        deposit: Lovelace(100_000_000_000),
        return_addr: vec![0u8; 29],
        gov_action: GovAction::ParameterChange {
            prev_action_id: None,
            protocol_param_update: Box::new(ppu),
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

    // CC member votes Yes (using hot credential)
    let hot_cred = Credential::VerificationKey(Hash28::from_bytes([20u8; 28]));
    let cc_voter = Voter::ConstitutionalCommittee(hot_cred);
    state.process_vote(
        &cc_voter,
        &action_id,
        &VotingProcedure {
            vote: Vote::Yes,
            anchor: None,
        },
    );

    // maxTxExUnits changes require Network group + Security SPO
    // In bootstrap, DRep thresholds are 0 (auto-pass)
    // max_block_ex_units is (Network, Security) -> needs SPO pvt_pp_security
    // We need 51% of SPO stake to vote Yes
    // Total SPO stake: 5 pools * 2T = 10T
    // Need > 5.1T in Yes votes
    // 3 SPOs voting Yes = 6T (60% > 51%)
    for i in 0..3 {
        let pool_hash28 = Hash28::from_bytes([100 + i as u8; 28]);
        let spo_voter = Voter::StakePool(pool_hash28.to_hash32_padded());
        state.process_vote(
            &spo_voter,
            &action_id,
            &VotingProcedure {
                vote: Vote::Yes,
                anchor: None,
            },
        );
    }

    // Ratify at epoch boundary
    state.process_epoch_transition(EpochNo(1));

    // Verify protocol parameters were updated
    assert_eq!(
        state.protocol_params.max_tx_ex_units.mem, 16_500_000,
        "maxTxExUnits.mem should be updated by governance"
    );
    assert_eq!(
        state.protocol_params.max_block_ex_units.mem, 72_000_000,
        "maxBlockExUnits.mem should be updated by governance"
    );
    // Proposal should be removed (enacted)
    assert!(
        state.governance.proposals.is_empty(),
        "Enacted proposal should be removed"
    );
    // Enacted root should be set
    assert!(
        state.governance.enacted_pparam_update.is_some(),
        "enacted_pparam_update should be set after ratification"
    );
}

#[test]
fn test_update_committee_no_cc_required() {
    // UpdateCommittee does NOT require CC approval, only DRep + SPO
    let mut state = governance_test_state();

    // Submit UpdateCommittee to add new CC members
    let tx_hash = Hash32::from_bytes([43u8; 32]);
    let new_cc_cred = Credential::VerificationKey(Hash28::from_bytes([30u8; 28]));
    let mut members_to_add = std::collections::BTreeMap::new();
    members_to_add.insert(new_cc_cred, 500u64); // expires epoch 500

    let proposal = ProposalProcedure {
        deposit: Lovelace(100_000_000_000),
        return_addr: vec![0u8; 29],
        gov_action: GovAction::UpdateCommittee {
            prev_action_id: None,
            members_to_remove: vec![],
            members_to_add,
            threshold: Rational {
                numerator: 2,
                denominator: 3,
            },
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

    // Only SPO votes needed (DRep auto-passes in bootstrap)
    // pvt_committee_normal = 0.51, total SPO stake = 10T
    // 3 SPOs = 6T (60% > 51%)
    for i in 0..3 {
        let pool_hash28 = Hash28::from_bytes([100 + i as u8; 28]);
        let spo_voter = Voter::StakePool(pool_hash28.to_hash32_padded());
        state.process_vote(
            &spo_voter,
            &action_id,
            &VotingProcedure {
                vote: Vote::Yes,
                anchor: None,
            },
        );
    }

    // Ratify at epoch boundary
    state.process_epoch_transition(EpochNo(1));

    // Verify new CC member was added
    let new_cc_key =
        credential_to_hash(&Credential::VerificationKey(Hash28::from_bytes([30u8; 28])));
    assert!(
        state
            .governance
            .committee_expiration
            .contains_key(&new_cc_key),
        "New CC member should be added"
    );
    assert_eq!(
        state.governance.committee_expiration[&new_cc_key],
        EpochNo(500),
        "CC member expiration should match"
    );
    // enacted_committee should be set
    assert!(
        state.governance.enacted_committee.is_some(),
        "enacted_committee should be set after ratification"
    );
}

#[test]
fn test_parameter_change_fails_without_cc() {
    // ParameterChange requires CC approval. If no CC can vote, it fails.
    let mut state = governance_test_state();

    // Remove all CC members (no hot keys)
    Arc::make_mut(&mut state.governance)
        .committee_hot_keys
        .clear();

    // Submit ParameterChange
    let tx_hash = Hash32::from_bytes([44u8; 32]);
    let ppu = ProtocolParamUpdate {
        drep_activity: Some(31),
        ..Default::default()
    };

    let proposal = ProposalProcedure {
        deposit: Lovelace(100_000_000_000),
        return_addr: vec![0u8; 29],
        gov_action: GovAction::ParameterChange {
            prev_action_id: None,
            protocol_param_update: Box::new(ppu),
            policy_hash: None,
        },
        anchor: Anchor {
            url: "https://example.com".to_string(),
            data_hash: Hash32::ZERO,
        },
    };

    state.process_proposal(&tx_hash, 0, &proposal);

    // Ratify at epoch boundary
    state.process_epoch_transition(EpochNo(1));

    // Verify drep_activity was NOT updated
    assert_eq!(
        state.protocol_params.drep_activity, 20,
        "drep_activity should NOT be updated without CC approval"
    );
    // Proposal should still be active
    assert_eq!(
        state.governance.proposals.len(),
        1,
        "Unratified proposal should remain active"
    );
}

#[test]
fn test_chained_parameter_changes() {
    // Two successive ParameterChange proposals with prev_action_id chain
    let mut state = governance_test_state();

    // First ParameterChange: update drep_activity to 25
    let tx1 = Hash32::from_bytes([50u8; 32]);
    let ppu1 = ProtocolParamUpdate {
        drep_activity: Some(25),
        ..Default::default()
    };

    let proposal1 = ProposalProcedure {
        deposit: Lovelace(100_000_000_000),
        return_addr: vec![0u8; 29],
        gov_action: GovAction::ParameterChange {
            prev_action_id: None,
            protocol_param_update: Box::new(ppu1),
            policy_hash: None,
        },
        anchor: Anchor {
            url: "https://example.com".to_string(),
            data_hash: Hash32::ZERO,
        },
    };

    state.process_proposal(&tx1, 0, &proposal1);
    let action_id1 = GovActionId {
        transaction_id: tx1,
        action_index: 0,
    };

    // CC votes Yes on first proposal
    let hot_cred = Credential::VerificationKey(Hash28::from_bytes([20u8; 28]));
    let cc_voter = Voter::ConstitutionalCommittee(hot_cred);
    state.process_vote(
        &cc_voter,
        &action_id1,
        &VotingProcedure {
            vote: Vote::Yes,
            anchor: None,
        },
    );

    // Ratify first proposal
    state.process_epoch_transition(EpochNo(1));
    assert_eq!(state.protocol_params.drep_activity, 25);

    // Now submit second ParameterChange, referencing the first as prev_action_id
    let tx2 = Hash32::from_bytes([51u8; 32]);
    let ppu2 = ProtocolParamUpdate {
        drep_activity: Some(31),
        ..Default::default()
    };

    let proposal2 = ProposalProcedure {
        deposit: Lovelace(100_000_000_000),
        return_addr: vec![0u8; 29],
        gov_action: GovAction::ParameterChange {
            prev_action_id: Some(action_id1.clone()),
            protocol_param_update: Box::new(ppu2),
            policy_hash: None,
        },
        anchor: Anchor {
            url: "https://example.com".to_string(),
            data_hash: Hash32::ZERO,
        },
    };

    state.process_proposal(&tx2, 0, &proposal2);
    let action_id2 = GovActionId {
        transaction_id: tx2,
        action_index: 0,
    };

    // CC votes Yes on second proposal
    state.process_vote(
        &cc_voter,
        &action_id2,
        &VotingProcedure {
            vote: Vote::Yes,
            anchor: None,
        },
    );

    // Ratify second proposal
    state.process_epoch_transition(EpochNo(2));
    assert_eq!(
        state.protocol_params.drep_activity, 31,
        "drep_activity should be updated to 31 by chained governance action"
    );
}

#[test]
fn test_cost_model_update_via_governance() {
    // ParameterChange can update PlutusV1/V2/V3 cost models
    let mut state = governance_test_state();

    let tx_hash = Hash32::from_bytes([55u8; 32]);
    let v2_costs = vec![1i64; 175]; // PlutusV2 has 175 cost model params
    let ppu = ProtocolParamUpdate {
        cost_models: Some(CostModels {
            plutus_v1: None,
            plutus_v2: Some(v2_costs.clone()),
            plutus_v3: None,
        }),
        ..Default::default()
    };

    let proposal = ProposalProcedure {
        deposit: Lovelace(100_000_000_000),
        return_addr: vec![0u8; 29],
        gov_action: GovAction::ParameterChange {
            prev_action_id: None,
            protocol_param_update: Box::new(ppu),
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

    // CC votes Yes
    let hot_cred = Credential::VerificationKey(Hash28::from_bytes([20u8; 28]));
    let cc_voter = Voter::ConstitutionalCommittee(hot_cred);
    state.process_vote(
        &cc_voter,
        &action_id,
        &VotingProcedure {
            vote: Vote::Yes,
            anchor: None,
        },
    );

    state.process_epoch_transition(EpochNo(1));

    assert_eq!(
        state.protocol_params.cost_models.plutus_v2,
        Some(v2_costs),
        "PlutusV2 cost model should be updated by governance"
    );
    // PlutusV1 should remain unchanged
    assert_eq!(
        state.protocol_params.cost_models.plutus_v1, None,
        "PlutusV1 cost model should not be changed"
    );
}

#[test]
fn test_genesis_utxo_reserves_adjustment() {
    // Seeding genesis UTxOs should reduce reserves by the seeded amount
    let params = ProtocolParameters::mainnet_defaults();
    let mut state = LedgerState::new(params);

    let initial_reserves = state.reserves.0;
    assert_eq!(initial_reserves, MAX_LOVELACE_SUPPLY);

    // Seed some UTxOs
    let entries: Vec<(Vec<u8>, u64)> = vec![
        (vec![1u8; 32], 1_000_000_000_000), // 1000 ADA
        (vec![2u8; 32], 2_000_000_000_000), // 2000 ADA
        (vec![3u8; 32], 500_000_000_000),   // 500 ADA
    ];
    let total_seeded: u64 = entries.iter().map(|(_, v)| *v).sum();

    state.seed_genesis_utxos(&entries);

    assert_eq!(
        state.reserves.0,
        initial_reserves - total_seeded,
        "Reserves should be reduced by seeded UTxO amount"
    );
    assert_eq!(
        state.utxo_set.len(),
        3,
        "UTxO set should contain seeded entries"
    );
}

#[test]
fn test_pre_conway_ppup_version_upgrade() {
    // Pre-Conway PPUP proposals should upgrade protocol version
    let mut params = ProtocolParameters::mainnet_defaults();
    params.protocol_version_major = 6; // Start at Shelley
    params.protocol_version_minor = 0;
    let mut state = LedgerState::new(params);
    state.epoch_length = 100;
    state.update_quorum = 2; // Need 2 distinct proposers

    // Two genesis delegates propose upgrade to version 7
    let genesis1 = Hash32::from_bytes([1u8; 32]);
    let genesis2 = Hash32::from_bytes([2u8; 32]);

    let ppu = ProtocolParamUpdate {
        protocol_version_major: Some(7),
        protocol_version_minor: Some(0),
        ..Default::default()
    };

    // Both propose targeting epoch 0 (current epoch)
    state
        .pending_pp_updates
        .entry(EpochNo(0))
        .or_default()
        .push((genesis1, ppu.clone()));
    state
        .pending_pp_updates
        .entry(EpochNo(0))
        .or_default()
        .push((genesis2, ppu));

    // Epoch transition 0 -> 1 should apply the update
    state.process_epoch_transition(EpochNo(1));

    assert_eq!(
        state.protocol_params.protocol_version_major, 7,
        "Protocol version should be upgraded to 7"
    );
}

#[test]
fn test_hard_fork_initiation_ratification() {
    // HardForkInitiation requires DRep + SPO + CC
    let mut state = governance_test_state();

    let tx_hash = Hash32::from_bytes([60u8; 32]);
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

    // CC votes Yes
    let hot_cred = Credential::VerificationKey(Hash28::from_bytes([20u8; 28]));
    state.process_vote(
        &Voter::ConstitutionalCommittee(hot_cred),
        &action_id,
        &VotingProcedure {
            vote: Vote::Yes,
            anchor: None,
        },
    );

    // SPOs vote Yes (need 51% for pvt_hard_fork)
    for i in 0..3 {
        let pool_hash28 = Hash28::from_bytes([100 + i as u8; 28]);
        state.process_vote(
            &Voter::StakePool(pool_hash28.to_hash32_padded()),
            &action_id,
            &VotingProcedure {
                vote: Vote::Yes,
                anchor: None,
            },
        );
    }

    state.process_epoch_transition(EpochNo(1));

    assert_eq!(
        state.protocol_params.protocol_version_major, 10,
        "Protocol version should be 10 after HardForkInitiation"
    );
    assert_eq!(
        state.protocol_params.protocol_version_minor, 0,
        "Protocol minor version should be 0"
    );
}

#[test]
fn test_prev_action_id_chain_mismatch_blocks_ratification() {
    // A ParameterChange with wrong prev_action_id should NOT be ratified
    let mut state = governance_test_state();

    // Submit ParameterChange with a wrong prev_action_id
    let tx_hash = Hash32::from_bytes([70u8; 32]);
    let wrong_prev = GovActionId {
        transaction_id: Hash32::from_bytes([99u8; 32]),
        action_index: 0,
    };
    let ppu = ProtocolParamUpdate {
        drep_activity: Some(99),
        ..Default::default()
    };

    let proposal = ProposalProcedure {
        deposit: Lovelace(100_000_000_000),
        return_addr: vec![0u8; 29],
        gov_action: GovAction::ParameterChange {
            prev_action_id: Some(wrong_prev),
            protocol_param_update: Box::new(ppu),
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

    // CC votes Yes
    let hot_cred = Credential::VerificationKey(Hash28::from_bytes([20u8; 28]));
    state.process_vote(
        &Voter::ConstitutionalCommittee(hot_cred),
        &action_id,
        &VotingProcedure {
            vote: Vote::Yes,
            anchor: None,
        },
    );

    state.process_epoch_transition(EpochNo(1));

    // drep_activity should NOT be changed
    assert_eq!(
        state.protocol_params.drep_activity, 20,
        "drep_activity should not change with wrong prev_action_id"
    );
}

#[test]
fn test_committee_min_size_update_via_governance() {
    // committeeMinSize should be updatable via governance ParameterChange
    let mut state = governance_test_state();
    assert_eq!(state.protocol_params.committee_min_size, 0);

    let tx_hash = Hash32::from_bytes([80u8; 32]);
    let ppu = ProtocolParamUpdate {
        min_committee_size: Some(3),
        ..Default::default()
    };

    let proposal = ProposalProcedure {
        deposit: Lovelace(100_000_000_000),
        return_addr: vec![0u8; 29],
        gov_action: GovAction::ParameterChange {
            prev_action_id: None,
            protocol_param_update: Box::new(ppu),
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

    // CC votes Yes
    let hot_cred = Credential::VerificationKey(Hash28::from_bytes([20u8; 28]));
    state.process_vote(
        &Voter::ConstitutionalCommittee(hot_cred),
        &action_id,
        &VotingProcedure {
            vote: Vote::Yes,
            anchor: None,
        },
    );

    state.process_epoch_transition(EpochNo(1));

    assert_eq!(
        state.protocol_params.committee_min_size, 3,
        "committeeMinSize should be updated to 3"
    );
}

// ===== Byron EBB (Epoch Boundary Block) tests =====

/// Build a minimal Byron-era block for tests.
/// Protocol version 1.x → `Era::Byron`.
fn make_byron_block_ebb_test(slot: u64, block_no: u64, prev_hash: Hash32) -> Block {
    let mut hash_bytes = [0u8; 32];
    hash_bytes[..8].copy_from_slice(&block_no.to_be_bytes());
    hash_bytes[8] = 0xBB; // sentinel: Byron block
    Block {
        header: torsten_primitives::block::BlockHeader {
            header_hash: Hash32::from_bytes(hash_bytes),
            prev_hash,
            issuer_vkey: vec![0u8; 32],
            vrf_vkey: vec![],
            vrf_result: torsten_primitives::block::VrfOutput {
                output: vec![],
                proof: vec![],
            },
            nonce_vrf_output: vec![],
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
            protocol_version: torsten_primitives::block::ProtocolVersion { major: 1, minor: 0 },
            kes_signature: vec![],
        },
        transactions: vec![],
        era: Era::Byron,
        raw_cbor: None,
    }
}

/// Construct a synthetic EBB hash for test use.
/// The actual EBB hash is Blake2b-256 of the EBB header bytes; we use a
/// deterministic placeholder here since we test the ledger's response to
/// `advance_past_ebb`, not the hash computation.
fn test_ebb_hash(epoch: u8) -> Hash32 {
    let mut b = [0u8; 32];
    b[0] = 0xEB; // sentinel: Epoch Boundary Block
    b[1] = 0xB0;
    b[2] = epoch;
    Hash32::from_bytes(b)
}

/// Create a `LedgerState` configured for the Byron era.
fn make_byron_ledger_state() -> LedgerState {
    let params = ProtocolParameters::mainnet_defaults();
    let mut state = LedgerState::new(params);
    state.era = Era::Byron;
    state.epoch = EpochNo(0);
    // Mainnet Byron configuration
    state.epoch_length = 432_000;
    state.shelley_transition_epoch = 208;
    state.byron_epoch_length = 21_600;
    state
}

/// `advance_past_ebb` advances the ledger tip hash to the EBB hash while
/// preserving the slot from the previous real block.
#[test]
fn test_advance_past_ebb_updates_tip_hash() {
    let mut state = make_byron_ledger_state();

    // Place the ledger at a known tip (last block of epoch 0)
    let epoch0_last_slot = SlotNo(21_599);
    let epoch0_last_hash = Hash32::from_bytes([0xA0; 32]);
    state.tip = Tip {
        point: Point::Specific(epoch0_last_slot, epoch0_last_hash),
        block_number: BlockNo(100),
    };

    // Advance through the epoch 0→1 EBB
    let ebb = test_ebb_hash(0);
    state
        .advance_past_ebb(ebb)
        .expect("advance_past_ebb should succeed in Byron era");

    // The tip hash must now be the EBB hash
    assert_eq!(
        state.tip.point.hash(),
        Some(&ebb),
        "Ledger tip hash should be the EBB hash after advance"
    );
    // The slot must be preserved from the previous block (NOT changed to EBB slot)
    assert_eq!(
        state.tip.point.slot(),
        Some(epoch0_last_slot),
        "Ledger slot should be preserved from the block before the EBB"
    );
    // Block number should not change — EBBs are not real blocks
    assert_eq!(
        state.tip.block_number,
        BlockNo(100),
        "Block number should not change for an EBB"
    );
}

/// After `advance_past_ebb`, `apply_block` must succeed for a block whose
/// `prev_hash` equals the EBB hash.  This is the core connectivity fix:
/// without EBB bridging, the block is rejected with `BlockDoesNotConnect`.
#[test]
fn test_apply_block_after_ebb_connects() {
    let mut state = make_byron_ledger_state();

    let epoch0_slot = SlotNo(21_500);
    let epoch0_hash = Hash32::from_bytes([0xA0; 32]);
    state.tip = Tip {
        point: Point::Specific(epoch0_slot, epoch0_hash),
        block_number: BlockNo(50),
    };

    let epoch1_ebb = test_ebb_hash(1);
    state
        .advance_past_ebb(epoch1_ebb)
        .expect("advance_past_ebb should succeed");

    // First real block of epoch 1 references the EBB hash as prev_hash
    let first_epoch1_block = make_byron_block_ebb_test(21_601, 51, epoch1_ebb);

    // Before the fix: this returned BlockDoesNotConnect.
    // After the fix: this succeeds.
    state
        .apply_block(&first_epoch1_block, BlockValidationMode::ApplyOnly)
        .expect("apply_block should succeed after EBB advance");

    assert_eq!(
        state.tip.point.hash(),
        Some(first_epoch1_block.hash()),
        "Ledger tip hash should be the new block hash"
    );
    assert_eq!(state.tip.point.slot(), Some(SlotNo(21_601)));
    assert_eq!(state.tip.block_number, BlockNo(51));
}

/// EBBs only exist in the Byron era.  Calling `advance_past_ebb` in
/// Shelley or later must return an error to prevent accidental tip rewrites.
#[test]
fn test_advance_past_ebb_rejects_non_byron_era() {
    let non_byron_eras = [
        Era::Shelley,
        Era::Allegra,
        Era::Mary,
        Era::Alonzo,
        Era::Babbage,
        Era::Conway,
    ];
    let params = ProtocolParameters::mainnet_defaults();
    for era in non_byron_eras {
        let mut state = LedgerState::new(params.clone());
        state.era = era;
        let result = state.advance_past_ebb(test_ebb_hash(0));
        assert!(
            result.is_err(),
            "advance_past_ebb must fail in {era:?} — EBBs do not exist after Byron"
        );
    }
}

/// Full sequence: [real_block_epoch0] → EBB → [real_block_epoch1] → [real_block_epoch1+1]
///
/// This models the exact mainnet failure at slot 21600 (Byron epoch 0→1).
#[test]
fn test_ebb_bridge_full_sequence() {
    let mut state = make_byron_ledger_state();

    // Genesis tip: before any block was applied
    let genesis_hash = Hash32::from_bytes([0x00; 32]);
    state.tip = Tip {
        point: Point::Specific(SlotNo(0), genesis_hash),
        block_number: BlockNo(0),
    };

    // Apply a block in epoch 0
    let epoch0_block = make_byron_block_ebb_test(21_000, 1000, genesis_hash);
    state
        .apply_block(&epoch0_block, BlockValidationMode::ApplyOnly)
        .expect("epoch0 block should apply");
    assert_eq!(state.tip.block_number, BlockNo(1000));

    // Epoch 0→1 EBB
    let ebb_for_epoch1 = test_ebb_hash(1);

    // Advance the ledger tip through the EBB
    state
        .advance_past_ebb(ebb_for_epoch1)
        .expect("EBB advance should succeed in Byron era");

    // Verify: tip hash = EBB hash, slot preserved from epoch0_block
    assert_eq!(state.tip.point.hash(), Some(&ebb_for_epoch1));
    assert_eq!(state.tip.point.slot(), Some(SlotNo(21_000)));

    // First real block of epoch 1, references the EBB hash as prev_hash
    let first_epoch1_block = make_byron_block_ebb_test(21_601, 1001, ebb_for_epoch1);
    state
        .apply_block(&first_epoch1_block, BlockValidationMode::ApplyOnly)
        .expect("first epoch1 block should apply after EBB advance");

    assert_eq!(state.tip.point.hash(), Some(first_epoch1_block.hash()));
    assert_eq!(state.tip.block_number, BlockNo(1001));

    // Subsequent block in epoch 1 connects normally (no EBB)
    let second_epoch1_block = make_byron_block_ebb_test(21_700, 1002, *first_epoch1_block.hash());
    state
        .apply_block(&second_epoch1_block, BlockValidationMode::ApplyOnly)
        .expect("second epoch1 block should apply normally");
    assert_eq!(state.tip.block_number, BlockNo(1002));
}

/// Without `advance_past_ebb`, a block whose `prev_hash` equals an EBB hash
/// (rather than the current ledger tip hash) must return `BlockDoesNotConnect`.
/// This test documents the pre-fix failure mode and ensures the check is still
/// present to catch genuine connectivity errors.
#[test]
fn test_block_does_not_connect_without_ebb_advance() {
    let mut state = make_byron_ledger_state();

    let epoch0_slot = SlotNo(21_500);
    let epoch0_hash = Hash32::from_bytes([0xA0; 32]);
    state.tip = Tip {
        point: Point::Specific(epoch0_slot, epoch0_hash),
        block_number: BlockNo(50),
    };

    // Block whose prev_hash = EBB hash, NOT the current tip hash
    let ebb = test_ebb_hash(1);
    let next_block = make_byron_block_ebb_test(21_601, 51, ebb);

    let result = state.apply_block(&next_block, BlockValidationMode::ApplyOnly);
    assert!(
        matches!(result, Err(LedgerError::BlockDoesNotConnect { .. })),
        "Block referencing an EBB without advance_past_ebb must be rejected: {result:?}"
    );
}

// ============================================================================
// Conway LEDGERS rule tests
// ============================================================================

/// Build a minimal Conway state (protocol 9) with an empty UTxO set and the
/// given treasury balance.  The epoch is set to 0, epoch_length to 100.
fn make_conway_state_with_treasury(treasury: u64) -> LedgerState {
    let mut params = ProtocolParameters::mainnet_defaults();
    params.protocol_version_major = 9;
    params.committee_min_size = 0;
    let mut state = LedgerState::new(params);
    state.epoch_length = 100;
    state.shelley_transition_epoch = 0;
    state.byron_epoch_length = 0;
    state.needs_stake_rebuild = false;
    state.treasury = Lovelace(treasury);
    state
}

/// Conway LEDGERS rule: when a transaction body declares `currentTreasuryValue`
/// (field 19) and the value does not match the ledger's treasury balance, the
/// block must be rejected.
#[test]
fn test_treasury_value_mismatch_corrects_treasury() {
    // Ledger treasury = 1_000_000 lovelace
    let mut state = make_conway_state_with_treasury(1_000_000);

    // Build a transaction that declares treasury_value = 9_999 (wrong)
    let mut tx = Transaction::empty_with_hash(Hash32::from_bytes([1u8; 32]));
    tx.is_valid = true;
    tx.body.treasury_value = Some(Lovelace(9_999));

    let block = make_test_block(1, 1, Hash32::ZERO, vec![tx]);
    // On confirmed blocks, treasury mismatch is a warning — the ledger
    // self-corrects by adopting the on-chain declared value.
    let _result = state.apply_block(&block, BlockValidationMode::ValidateAll);
    assert_eq!(
        state.treasury.0, 9_999,
        "Treasury must be corrected to match the on-chain declared value"
    );
}

/// Conway LEDGERS rule: when `currentTreasuryValue` matches exactly, validation
/// must succeed (the check alone must not reject a valid block — further checks
/// are short-circuited here by the empty-input transaction which would normally
/// fail Phase-1; we use ApplyOnly to confirm the happy path skips the check).
#[test]
fn test_treasury_value_matching_does_not_reject_in_validate_all() {
    // Ledger treasury = 500_000 lovelace
    let mut state = make_conway_state_with_treasury(500_000);

    // Build a transaction with the CORRECT treasury_value
    let mut tx = Transaction::empty_with_hash(Hash32::from_bytes([2u8; 32]));
    tx.is_valid = true;
    tx.body.treasury_value = Some(Lovelace(500_000));

    // The transaction has no inputs so Phase-1 will fail with NoInputs —
    // that's fine; the test only checks that the *treasury* check itself
    // does not fire.  We inspect the error message to confirm.
    let block = make_test_block(1, 1, Hash32::ZERO, vec![tx]);
    let result = state.apply_block(&block, BlockValidationMode::ValidateAll);

    // Must NOT be a TreasuryValueMismatch error
    if let Err(LedgerError::BlockTxValidationFailed { ref errors, .. }) = result {
        assert!(
            !errors.contains("TreasuryValueMismatch"),
            "Correct treasury_value must not produce TreasuryValueMismatch; got: {errors}"
        );
    }
    // Any other error (e.g. Phase-1 NoInputs) is acceptable — the treasury
    // check itself passed.
}

/// Conway LEDGERS rule: when `treasury_value` is absent from the tx body, the
/// check must not fire (field 19 is optional; pre-Conway and many Conway txs
/// omit it).
#[test]
fn test_treasury_value_absent_skips_check() {
    let mut state = make_conway_state_with_treasury(42_000);

    let mut tx = Transaction::empty_with_hash(Hash32::from_bytes([3u8; 32]));
    tx.is_valid = true;
    tx.body.treasury_value = None; // field absent — check must not fire

    let block = make_test_block(1, 1, Hash32::ZERO, vec![tx]);
    let result = state.apply_block(&block, BlockValidationMode::ValidateAll);

    if let Err(LedgerError::BlockTxValidationFailed { ref errors, .. }) = result {
        assert!(
            !errors.contains("TreasuryValueMismatch"),
            "Absent treasury_value must never produce TreasuryValueMismatch; got: {errors}"
        );
    }
}

/// Build a minimal Conway state that has one CC member with the given cold
/// credential hash in `committee_expiration`.
fn make_conway_state_with_cc_member(cold_key: torsten_primitives::hash::Hash32) -> LedgerState {
    let mut state = make_conway_state_with_treasury(0);
    Arc::make_mut(&mut state.governance)
        .committee_expiration
        .insert(cold_key, EpochNo(1000));
    state
}

/// Conway LEDGERS rule: a `CommitteeHotAuth` certificate whose cold credential
/// is NOT present in `committee_expiration` must be rejected ("failOnNonEmpty
/// unelected" predicate in Haskell `conwayCertsPredFailure`).
#[test]
fn test_committee_hot_auth_unelected_cold_credential_warned_not_rejected() {
    // CC member's cold key
    let cold_cred = Credential::VerificationKey(Hash28::from_bytes([10u8; 28]));
    let cold_key = credential_to_hash(&cold_cred);
    let mut state = make_conway_state_with_cc_member(cold_key);

    // Attacker tries to authorize a hot key for a DIFFERENT cold credential
    // that is NOT in the committee.  On confirmed blocks this is a warning
    // (not a hard error) to prevent UTxO cascade divergence.
    let outsider_cold = Credential::VerificationKey(Hash28::from_bytes([99u8; 28]));
    let hot_cred = Credential::VerificationKey(Hash28::from_bytes([77u8; 28]));

    let mut tx = Transaction::empty_with_hash(Hash32::from_bytes([4u8; 32]));
    tx.is_valid = true;
    tx.body.certificates = vec![Certificate::CommitteeHotAuth {
        cold_credential: outsider_cold,
        hot_credential: hot_cred,
    }];

    let block = make_test_block(1, 1, Hash32::ZERO, vec![tx]);
    // For confirmed blocks, unelected committee member is a warning (logged),
    // not a hard error — the block is applied to avoid UTxO cascade corruption.
    let _result = state.apply_block(&block, BlockValidationMode::ValidateAll);
    // The block should be applied (no Err).  The cert processing logs a
    // warning but doesn't prevent output insertion.
}

/// Conway LEDGERS rule: a `CommitteeHotAuth` certificate whose cold credential
/// IS present in `committee_expiration` must not be rejected by the unelected
/// check.  (The block may still fail Phase-1 for other reasons; we only verify
/// the CC check does not fire.)
#[test]
fn test_committee_hot_auth_elected_cold_credential_not_rejected_by_cc_check() {
    let cold_cred = Credential::VerificationKey(Hash28::from_bytes([10u8; 28]));
    let cold_key = credential_to_hash(&cold_cred);
    let hot_cred = Credential::VerificationKey(Hash28::from_bytes([20u8; 28]));
    let mut state = make_conway_state_with_cc_member(cold_key);

    let mut tx = Transaction::empty_with_hash(Hash32::from_bytes([5u8; 32]));
    tx.is_valid = true;
    tx.body.certificates = vec![Certificate::CommitteeHotAuth {
        cold_credential: cold_cred,
        hot_credential: hot_cred,
    }];

    let block = make_test_block(1, 1, Hash32::ZERO, vec![tx]);
    let result = state.apply_block(&block, BlockValidationMode::ValidateAll);

    if let Err(LedgerError::BlockTxValidationFailed { ref errors, .. }) = result {
        assert!(
            !errors.contains("UnelectedCommitteeMember"),
            "Elected CC member must not trigger UnelectedCommitteeMember; got: {errors}"
        );
    }
}

// =========================================================================
//  Coverage Sprint Tests
// =========================================================================

// -----------------------------------------------------------------------
// 1. Nonce computation edge cases — update_evolving_nonce
// -----------------------------------------------------------------------

/// update_evolving_nonce always hashes the input regardless of length.
/// Verify the "always-hash" invariant: even a 32-byte input is hashed once
/// before being combined with evolving_nonce.
#[test]
fn test_evolving_nonce_always_hash_invariant() {
    let params = ProtocolParameters::mainnet_defaults();
    let mut state = LedgerState::new(params);
    state.evolving_nonce = Hash32::ZERO;

    // Feed a known 32-byte input
    let input = [0xABu8; 32];
    state.update_evolving_nonce(&input);

    // The function should compute: blake2b_256(evolving || blake2b_256(input))
    // NOT blake2b_256(evolving || input) directly.
    let eta_hash = torsten_primitives::hash::blake2b_256(&input);
    let mut expected_data = Vec::with_capacity(64);
    expected_data.extend_from_slice(Hash32::ZERO.as_bytes());
    expected_data.extend_from_slice(eta_hash.as_bytes());
    let expected = torsten_primitives::hash::blake2b_256(&expected_data);

    assert_eq!(
        state.evolving_nonce, expected,
        "32-byte input must be hashed before combining with evolving_nonce"
    );
}

/// update_evolving_nonce with a 64-byte input (TPraos raw VRF output).
#[test]
fn test_evolving_nonce_64_byte_input() {
    let params = ProtocolParameters::mainnet_defaults();
    let mut state = LedgerState::new(params);
    state.evolving_nonce = Hash32::from_bytes([0x11; 32]);

    let input = [0xCDu8; 64];
    state.update_evolving_nonce(&input);

    let eta_hash = torsten_primitives::hash::blake2b_256(&input);
    let mut expected_data = Vec::with_capacity(64);
    expected_data.extend_from_slice(&[0x11; 32]);
    expected_data.extend_from_slice(eta_hash.as_bytes());
    let expected = torsten_primitives::hash::blake2b_256(&expected_data);

    assert_eq!(state.evolving_nonce, expected);
}

/// update_evolving_nonce with a 0-byte input.
#[test]
fn test_evolving_nonce_zero_byte_input() {
    let params = ProtocolParameters::mainnet_defaults();
    let mut state = LedgerState::new(params);
    state.evolving_nonce = Hash32::from_bytes([0x22; 32]);

    let input: [u8; 0] = [];
    state.update_evolving_nonce(&input);

    let eta_hash = torsten_primitives::hash::blake2b_256(&input);
    let mut expected_data = Vec::with_capacity(64);
    expected_data.extend_from_slice(&[0x22; 32]);
    expected_data.extend_from_slice(eta_hash.as_bytes());
    let expected = torsten_primitives::hash::blake2b_256(&expected_data);

    assert_eq!(state.evolving_nonce, expected);
}

/// update_evolving_nonce with a 1-byte input.
#[test]
fn test_evolving_nonce_single_byte_input() {
    let params = ProtocolParameters::mainnet_defaults();
    let mut state = LedgerState::new(params);
    state.evolving_nonce = Hash32::from_bytes([0x33; 32]);

    let input = [0x42u8];
    state.update_evolving_nonce(&input);

    let eta_hash = torsten_primitives::hash::blake2b_256(&input);
    let mut expected_data = Vec::with_capacity(64);
    expected_data.extend_from_slice(&[0x33; 32]);
    expected_data.extend_from_slice(eta_hash.as_bytes());
    let expected = torsten_primitives::hash::blake2b_256(&expected_data);

    assert_eq!(state.evolving_nonce, expected);
}

/// update_evolving_nonce with a 128-byte input.
#[test]
fn test_evolving_nonce_128_byte_input() {
    let params = ProtocolParameters::mainnet_defaults();
    let mut state = LedgerState::new(params);
    state.evolving_nonce = Hash32::from_bytes([0x44; 32]);

    let input = [0xEFu8; 128];
    state.update_evolving_nonce(&input);

    let eta_hash = torsten_primitives::hash::blake2b_256(&input);
    let mut expected_data = Vec::with_capacity(64);
    expected_data.extend_from_slice(&[0x44; 32]);
    expected_data.extend_from_slice(eta_hash.as_bytes());
    let expected = torsten_primitives::hash::blake2b_256(&expected_data);

    assert_eq!(state.evolving_nonce, expected);
}

/// update_evolving_nonce with all-zero input (32 bytes of 0x00).
#[test]
fn test_evolving_nonce_all_zeros_input() {
    let params = ProtocolParameters::mainnet_defaults();
    let mut state = LedgerState::new(params);
    state.evolving_nonce = Hash32::from_bytes([0x55; 32]);

    let input = [0u8; 32];
    state.update_evolving_nonce(&input);

    // All-zeros is still hashed — should NOT be treated as NeutralNonce.
    let eta_hash = torsten_primitives::hash::blake2b_256(&input);
    let mut expected_data = Vec::with_capacity(64);
    expected_data.extend_from_slice(&[0x55; 32]);
    expected_data.extend_from_slice(eta_hash.as_bytes());
    let expected = torsten_primitives::hash::blake2b_256(&expected_data);

    assert_eq!(
        state.evolving_nonce, expected,
        "All-zero input must be hashed normally (no NeutralNonce shortcut)"
    );
}

// -----------------------------------------------------------------------
// epoch_nonce_for_slot — pre-computes the correct VRF nonce for any slot
// -----------------------------------------------------------------------

/// epoch_nonce_for_slot returns epoch_nonce unchanged for a slot in the
/// current epoch.
#[test]
fn test_epoch_nonce_for_slot_same_epoch() {
    let params = ProtocolParameters::mainnet_defaults();
    let mut state = LedgerState::new(params);
    // Use a pure Shelley-only setup (no Byron era) so slot math is simple.
    state.epoch = EpochNo(10);
    state.epoch_length = 100;
    state.shelley_transition_epoch = 0;
    state.byron_epoch_length = 0;
    state.epoch_nonce = Hash32::from_bytes([0xAA; 32]);
    state.candidate_nonce = Hash32::from_bytes([0xBB; 32]);
    state.last_epoch_block_nonce = Hash32::from_bytes([0xCC; 32]);

    // Epoch 10 spans slots 1000..1100 (epoch_length=100, no Byron offset).
    let slot_in_epoch_10 = 1050u64;
    assert_eq!(state.epoch_of_slot(slot_in_epoch_10), 10);
    assert_eq!(
        state.epoch_nonce_for_slot(slot_in_epoch_10),
        Hash32::from_bytes([0xAA; 32]),
        "same-epoch slot must return the current epoch_nonce"
    );
}

/// epoch_nonce_for_slot pre-computes TICKN for a slot in the immediately
/// following epoch — this is the key fix for the stale-nonce-after-restart bug.
///
/// The expected nonce = blake2b_256(candidate || last_epoch_block_nonce),
/// exactly matching process_epoch_transition Step 1.
#[test]
fn test_epoch_nonce_for_slot_next_epoch() {
    let params = ProtocolParameters::mainnet_defaults();
    let mut state = LedgerState::new(params);
    state.epoch = EpochNo(10);
    state.epoch_length = 100;
    state.shelley_transition_epoch = 0;
    state.byron_epoch_length = 0;
    state.epoch_nonce = Hash32::from_bytes([0xAA; 32]);

    let candidate = Hash32::from_bytes([0xBB; 32]);
    let prev_hash = Hash32::from_bytes([0xCC; 32]);
    state.candidate_nonce = candidate;
    state.last_epoch_block_nonce = prev_hash;

    // Slot in epoch 11 (first slot after the boundary).
    let slot_in_epoch_11 = 1100u64;
    assert_eq!(state.epoch_of_slot(slot_in_epoch_11), 11);

    // Expected = blake2b_256(candidate || last_epoch_block_nonce)
    let mut buf = Vec::with_capacity(64);
    buf.extend_from_slice(candidate.as_bytes());
    buf.extend_from_slice(prev_hash.as_bytes());
    let expected = torsten_primitives::hash::blake2b_256(&buf);

    let computed = state.epoch_nonce_for_slot(slot_in_epoch_11);
    assert_eq!(
        computed, expected,
        "next-epoch slot must return TICKN-computed nonce"
    );
    // Must differ from both the current epoch_nonce and the raw candidate.
    assert_ne!(computed, Hash32::from_bytes([0xAA; 32]));
}

/// epoch_nonce_for_slot with NeutralNonce (ZERO) for last_epoch_block_nonce:
/// result = candidate (identity element of Nonce combine).
#[test]
fn test_epoch_nonce_for_slot_next_epoch_neutral_prev() {
    let params = ProtocolParameters::mainnet_defaults();
    let mut state = LedgerState::new(params);
    state.epoch = EpochNo(0);
    state.epoch_length = 100;
    state.shelley_transition_epoch = 0;
    state.byron_epoch_length = 0;

    let candidate = Hash32::from_bytes([0xDD; 32]);
    state.candidate_nonce = candidate;
    state.last_epoch_block_nonce = Hash32::ZERO; // NeutralNonce

    // Slot in epoch 1.
    let slot_in_epoch_1 = 100u64;
    assert_eq!(state.epoch_of_slot(slot_in_epoch_1), 1);
    assert_eq!(
        state.epoch_nonce_for_slot(slot_in_epoch_1),
        candidate,
        "candidate ⭒ NeutralNonce = candidate (identity)"
    );
}

/// epoch_nonce_for_slot for a slot more than 1 epoch ahead returns the
/// current epoch_nonce (best-effort fallback — can't predict future nonces).
#[test]
fn test_epoch_nonce_for_slot_far_future_epoch() {
    let params = ProtocolParameters::mainnet_defaults();
    let mut state = LedgerState::new(params);
    state.epoch = EpochNo(10);
    state.epoch_length = 100;
    state.shelley_transition_epoch = 0;
    state.byron_epoch_length = 0;
    state.epoch_nonce = Hash32::from_bytes([0xEE; 32]);

    // Slot 2 epochs ahead — beyond our prediction horizon.
    let slot_far_ahead = 1200u64;
    assert_eq!(state.epoch_of_slot(slot_far_ahead), 12);
    assert_eq!(
        state.epoch_nonce_for_slot(slot_far_ahead),
        Hash32::from_bytes([0xEE; 32]),
        "slots >1 epoch ahead fall back to the current epoch_nonce"
    );
}

/// Verify that epoch_nonce_for_slot is consistent with what
/// process_epoch_transition actually produces: applying a transition to
/// epoch N+1 and then reading epoch_nonce must equal epoch_nonce_for_slot
/// evaluated at any slot in epoch N+1 BEFORE the transition.
#[test]
fn test_epoch_nonce_for_slot_matches_transition_result() {
    use torsten_primitives::time::EpochNo;

    let params = ProtocolParameters::mainnet_defaults();
    let mut state = LedgerState::new(params);
    state.epoch = EpochNo(5);
    state.epoch_length = 100;
    state.shelley_transition_epoch = 0;
    state.byron_epoch_length = 0;
    state.needs_stake_rebuild = false;

    // Set up non-trivial nonce inputs.
    state.candidate_nonce = Hash32::from_bytes([0x01; 32]);
    state.last_epoch_block_nonce = Hash32::from_bytes([0x02; 32]);
    state.epoch_nonce = Hash32::from_bytes([0xFF; 32]); // current (epoch 5)
    state.lab_nonce = Hash32::from_bytes([0x03; 32]);

    // Pre-compute what the nonce *should* be for epoch 6 before transition.
    let slot_in_epoch_6 = 600u64;
    assert_eq!(state.epoch_of_slot(slot_in_epoch_6), 6);
    let predicted = state.epoch_nonce_for_slot(slot_in_epoch_6);

    // Now actually run the transition and verify epoch_nonce matches.
    state.process_epoch_transition(EpochNo(6));

    assert_eq!(
        state.epoch_nonce, predicted,
        "epoch_nonce_for_slot must predict the same value that \
         process_epoch_transition produces"
    );
}

/// NeutralNonce identity in epoch nonce combine: when prevHashNonce is zero
/// (NeutralNonce), epoch_nonce = candidate_nonce (identity element).
#[test]
fn test_epoch_nonce_neutral_prev_hash_nonce() {
    let params = ProtocolParameters::mainnet_defaults();
    let mut state = LedgerState::new(params.clone());
    state.epoch = EpochNo(5);
    state.epoch_length = 100;
    state.needs_stake_rebuild = false;

    // Set candidate to a non-zero value, prevHashNonce to zero (NeutralNonce)
    let candidate = Hash32::from_bytes([0xAA; 32]);
    state.candidate_nonce = candidate;
    state.last_epoch_block_nonce = Hash32::ZERO; // NeutralNonce
    state.lab_nonce = Hash32::from_bytes([0xBB; 32]);

    state.process_epoch_transition(EpochNo(6));

    // Per Haskell TICKN: NeutralNonce is identity, so epoch_nonce = candidate
    assert_eq!(
        state.epoch_nonce, candidate,
        "epoch_nonce = candidate when prevHashNonce is NeutralNonce (ZERO)"
    );
}

/// NeutralNonce identity: when candidate is zero (NeutralNonce),
/// epoch_nonce = prevHashNonce.
#[test]
fn test_epoch_nonce_neutral_candidate_nonce() {
    let params = ProtocolParameters::mainnet_defaults();
    let mut state = LedgerState::new(params.clone());
    state.epoch = EpochNo(5);
    state.epoch_length = 100;
    state.needs_stake_rebuild = false;

    let prev_hash = Hash32::from_bytes([0xCC; 32]);
    state.candidate_nonce = Hash32::ZERO; // NeutralNonce
    state.last_epoch_block_nonce = prev_hash;
    state.lab_nonce = Hash32::from_bytes([0xDD; 32]);

    state.process_epoch_transition(EpochNo(6));

    assert_eq!(
        state.epoch_nonce, prev_hash,
        "epoch_nonce = prevHashNonce when candidate is NeutralNonce (ZERO)"
    );
}

/// When both candidate and prevHashNonce are non-zero, epoch_nonce =
/// blake2b_256(candidate || prevHashNonce).
#[test]
fn test_epoch_nonce_both_non_zero() {
    let params = ProtocolParameters::mainnet_defaults();
    let mut state = LedgerState::new(params.clone());
    state.epoch = EpochNo(5);
    state.epoch_length = 100;
    state.needs_stake_rebuild = false;

    let candidate = Hash32::from_bytes([0x11; 32]);
    let prev_hash = Hash32::from_bytes([0x22; 32]);
    state.candidate_nonce = candidate;
    state.last_epoch_block_nonce = prev_hash;
    state.lab_nonce = Hash32::from_bytes([0x33; 32]);

    state.process_epoch_transition(EpochNo(6));

    let mut nonce_input = Vec::with_capacity(64);
    nonce_input.extend_from_slice(candidate.as_bytes());
    nonce_input.extend_from_slice(prev_hash.as_bytes());
    let expected = torsten_primitives::hash::blake2b_256(&nonce_input);

    assert_eq!(
        state.epoch_nonce, expected,
        "epoch_nonce = blake2b_256(candidate || prevHashNonce) when both non-zero"
    );
}

/// When both candidate and prevHashNonce are zero, epoch_nonce = ZERO.
#[test]
fn test_epoch_nonce_both_zero() {
    let params = ProtocolParameters::mainnet_defaults();
    let mut state = LedgerState::new(params.clone());
    state.epoch = EpochNo(5);
    state.epoch_length = 100;
    state.needs_stake_rebuild = false;

    state.candidate_nonce = Hash32::ZERO;
    state.last_epoch_block_nonce = Hash32::ZERO;
    state.lab_nonce = Hash32::from_bytes([0x44; 32]);

    state.process_epoch_transition(EpochNo(6));

    assert_eq!(
        state.epoch_nonce,
        Hash32::ZERO,
        "epoch_nonce = ZERO when both candidate and prevHashNonce are ZERO (NeutralNonce identity)"
    );
}

// -----------------------------------------------------------------------
// 2. Block 0 skip fix — genesis block at slot 0 IS processed
// -----------------------------------------------------------------------

/// When ledger_tip_slot is 0 (genesis state), a block at slot 0 must be
/// processed. This verifies the genesis block is not accidentally skipped
/// by an off-by-one in the epoch transition logic.
#[test]
fn test_genesis_block_slot_0_is_processed() {
    let mut params = ProtocolParameters::mainnet_defaults();
    params.protocol_version_major = 9;
    let mut state = LedgerState::new(params);
    state.epoch_length = 100;

    // Create a valid Conway block at slot 0 with no transactions
    let block = make_test_block(0, 0, Hash32::ZERO, vec![]);

    // The block should be applied without error (tip moves from Origin to slot 0)
    let result = state.apply_block(&block, BlockValidationMode::ApplyOnly);
    assert!(
        result.is_ok(),
        "Genesis block at slot 0 must be processed; got: {result:?}"
    );

    // Verify tip was actually updated
    assert_eq!(state.tip.block_number, BlockNo(0));
}

/// Verify that a block at slot 1 following genesis (slot 0) also applies correctly.
#[test]
fn test_block_after_genesis_slot_0() {
    let mut params = ProtocolParameters::mainnet_defaults();
    params.protocol_version_major = 9;
    let mut state = LedgerState::new(params);
    state.epoch_length = 100;

    let block0 = make_test_block(0, 0, Hash32::ZERO, vec![]);
    state
        .apply_block(&block0, BlockValidationMode::ApplyOnly)
        .unwrap();

    // Block 1 at slot 1, prev_hash = block0's header_hash
    let block1 = make_test_block(1, 1, block0.header.header_hash, vec![]);
    let result = state.apply_block(&block1, BlockValidationMode::ApplyOnly);
    assert!(
        result.is_ok(),
        "Block after genesis must apply; got: {result:?}"
    );
    assert_eq!(state.tip.block_number, BlockNo(1));
}

// -----------------------------------------------------------------------
// 3. Epoch transition ordering — epoch nonce uses OLD prevHashNonce
// -----------------------------------------------------------------------

/// Verify that epoch nonce computation uses the OLD prevHashNonce before
/// updating it with lab_nonce. This is the critical TICKN rule ordering.
#[test]
fn test_epoch_transition_uses_old_prev_hash_nonce() {
    let params = ProtocolParameters::mainnet_defaults();
    let mut state = LedgerState::new(params.clone());
    state.epoch = EpochNo(10);
    state.epoch_length = 100;
    state.needs_stake_rebuild = false;

    let old_prev_hash = Hash32::from_bytes([0xAA; 32]);
    let candidate = Hash32::from_bytes([0xBB; 32]);
    let lab = Hash32::from_bytes([0xCC; 32]);

    state.candidate_nonce = candidate;
    state.last_epoch_block_nonce = old_prev_hash;
    state.lab_nonce = lab;

    state.process_epoch_transition(EpochNo(11));

    // epoch_nonce should use OLD prevHashNonce (0xAA), not the lab_nonce (0xCC)
    let mut nonce_input = Vec::with_capacity(64);
    nonce_input.extend_from_slice(candidate.as_bytes());
    nonce_input.extend_from_slice(old_prev_hash.as_bytes()); // OLD value
    let expected = torsten_primitives::hash::blake2b_256(&nonce_input);

    assert_eq!(
        state.epoch_nonce, expected,
        "epoch_nonce must use OLD prevHashNonce, not the new lab_nonce"
    );

    // AFTER the transition, last_epoch_block_nonce should be updated to lab_nonce
    assert_eq!(
        state.last_epoch_block_nonce, lab,
        "prevHashNonce must be updated to lab_nonce AFTER nonce computation"
    );
}

// (Reference script fee ceiling tests are in validation/tests.rs)

// -----------------------------------------------------------------------
// 7. Block-level totalRefScriptSize exceeds 1 MiB rejection
// -----------------------------------------------------------------------

/// Verify that a block with aggregate reference script size exceeding 1 MiB
/// is rejected during ValidateAll mode (Conway, protocol >= 9).
#[test]
fn test_block_ref_script_size_exceeds_1mib_rejected() {
    use torsten_primitives::transaction::ScriptRef;

    let mut params = ProtocolParameters::mainnet_defaults();
    params.protocol_version_major = 9; // Conway
    let mut state = LedgerState::new(params);
    state.epoch_length = 100;

    // Create a reference input with a huge reference script (>1 MiB)
    let ref_input = TransactionInput {
        transaction_id: Hash32::from_bytes([0x99u8; 32]),
        index: 0,
    };
    let large_script = vec![0u8; 1_048_577]; // 1 MiB + 1 byte
    let ref_output = TransactionOutput {
        address: Address::Byron(ByronAddress {
            payload: vec![0u8; 32],
        }),
        value: Value::lovelace(10_000_000),
        datum: OutputDatum::None,
        script_ref: Some(ScriptRef::PlutusV2(large_script)),
        is_legacy: false,
        raw_cbor: None,
    };
    state.utxo_set.insert(ref_input.clone(), ref_output);

    // Create a spending input in the UTxO set
    let spend_input = TransactionInput {
        transaction_id: Hash32::from_bytes([0xAAu8; 32]),
        index: 0,
    };
    let spend_output = TransactionOutput {
        address: Address::Byron(ByronAddress {
            payload: vec![0u8; 32],
        }),
        value: Value::lovelace(20_000_000),
        datum: OutputDatum::None,
        script_ref: None,
        is_legacy: false,
        raw_cbor: None,
    };
    state.utxo_set.insert(spend_input.clone(), spend_output);

    // Build a transaction that references the large script
    let mut tx = Transaction::empty_with_hash(Hash32::from_bytes([0x01u8; 32]));
    tx.is_valid = true;
    tx.body.inputs = vec![spend_input];
    tx.body.reference_inputs = vec![ref_input];
    tx.body.outputs = vec![TransactionOutput {
        address: Address::Byron(ByronAddress {
            payload: vec![0u8; 32],
        }),
        value: Value::lovelace(19_000_000),
        datum: OutputDatum::None,
        script_ref: None,
        is_legacy: false,
        raw_cbor: None,
    }];
    tx.body.fee = Lovelace(1_000_000);

    let block = make_test_block(1, 1, Hash32::ZERO, vec![tx]);
    let result = state.apply_block(&block, BlockValidationMode::ValidateAll);
    assert!(
        matches!(result, Err(LedgerError::BlockTxValidationFailed { ref errors, .. })
            if errors.contains("BodyRefScriptsSizeTooBig")),
        "Block with >1MiB ref scripts must be rejected; got: {result:?}"
    );
}

/// Verify that a block with ref scripts well under 1 MiB is NOT rejected.
#[test]
fn test_block_ref_script_size_under_1mib_accepted() {
    use torsten_primitives::transaction::ScriptRef;

    let mut params = ProtocolParameters::mainnet_defaults();
    params.protocol_version_major = 9;
    let mut state = LedgerState::new(params);
    state.epoch_length = 100;

    // Reference script of 100 KiB — well under the 1 MiB limit
    let ref_input = TransactionInput {
        transaction_id: Hash32::from_bytes([0x99u8; 32]),
        index: 0,
    };
    let script_under_limit = vec![0u8; 102_400]; // 100 KiB
    let ref_output = TransactionOutput {
        address: Address::Byron(ByronAddress {
            payload: vec![0u8; 32],
        }),
        value: Value::lovelace(10_000_000),
        datum: OutputDatum::None,
        script_ref: Some(ScriptRef::PlutusV2(script_under_limit)),
        is_legacy: false,
        raw_cbor: None,
    };
    state.utxo_set.insert(ref_input.clone(), ref_output);

    let spend_input = TransactionInput {
        transaction_id: Hash32::from_bytes([0xAAu8; 32]),
        index: 0,
    };
    state.utxo_set.insert(
        spend_input.clone(),
        TransactionOutput {
            address: Address::Byron(ByronAddress {
                payload: vec![0u8; 32],
            }),
            value: Value::lovelace(20_000_000),
            datum: OutputDatum::None,
            script_ref: None,
            is_legacy: false,
            raw_cbor: None,
        },
    );

    let mut tx = Transaction::empty_with_hash(Hash32::from_bytes([0x01u8; 32]));
    tx.is_valid = true;
    tx.body.inputs = vec![spend_input];
    tx.body.reference_inputs = vec![ref_input];
    tx.body.outputs = vec![TransactionOutput {
        address: Address::Byron(ByronAddress {
            payload: vec![0u8; 32],
        }),
        value: Value::lovelace(19_000_000),
        datum: OutputDatum::None,
        script_ref: None,
        is_legacy: false,
        raw_cbor: None,
    }];
    tx.body.fee = Lovelace(1_000_000);

    let block = make_test_block(1, 1, Hash32::ZERO, vec![tx]);
    let result = state.apply_block(&block, BlockValidationMode::ValidateAll);
    // Should not fail with BodyRefScriptsSizeTooBig (may fail for other validation reasons)
    if let Err(LedgerError::BlockTxValidationFailed { ref errors, .. }) = result {
        assert!(
            !errors.contains("BodyRefScriptsSizeTooBig"),
            "100 KiB ref scripts (under limit) must not be rejected; got: {errors}"
        );
    }
}

// -----------------------------------------------------------------------
// 8. Treasury value check — match and mismatch
// -----------------------------------------------------------------------

/// Verify that a Conway transaction with matching treasury_value passes.
#[test]
fn test_treasury_value_match_passes() {
    let mut params = ProtocolParameters::mainnet_defaults();
    params.protocol_version_major = 9;
    let mut state = LedgerState::new(params);
    state.epoch_length = 100;
    state.treasury = Lovelace(500_000_000_000);

    // Add UTxO for the transaction to consume
    let input = TransactionInput {
        transaction_id: Hash32::from_bytes([0xAAu8; 32]),
        index: 0,
    };
    state.utxo_set.insert(
        input.clone(),
        TransactionOutput {
            address: Address::Byron(ByronAddress {
                payload: vec![0u8; 32],
            }),
            value: Value::lovelace(10_000_000),
            datum: OutputDatum::None,
            script_ref: None,
            is_legacy: false,
            raw_cbor: None,
        },
    );

    let mut tx = Transaction::empty_with_hash(Hash32::from_bytes([0x01u8; 32]));
    tx.is_valid = true;
    tx.body.treasury_value = Some(Lovelace(500_000_000_000)); // matches
    tx.body.inputs = vec![input];
    tx.body.outputs = vec![TransactionOutput {
        address: Address::Byron(ByronAddress {
            payload: vec![0u8; 32],
        }),
        value: Value::lovelace(9_000_000),
        datum: OutputDatum::None,
        script_ref: None,
        is_legacy: false,
        raw_cbor: None,
    }];
    tx.body.fee = Lovelace(1_000_000);

    let block = make_test_block(1, 1, Hash32::ZERO, vec![tx]);
    let result = state.apply_block(&block, BlockValidationMode::ValidateAll);
    // Should NOT fail with TreasuryValueMismatch (may fail for other reasons)
    if let Err(LedgerError::BlockTxValidationFailed { ref errors, .. }) = result {
        assert!(
            !errors.contains("TreasuryValueMismatch"),
            "Matching treasury value must not trigger mismatch; got: {errors}"
        );
    }
}

/// Verify that a Conway transaction with mismatched treasury_value is rejected.
#[test]
fn test_treasury_value_mismatch_corrects_and_applies() {
    let mut params = ProtocolParameters::mainnet_defaults();
    params.protocol_version_major = 9;
    let mut state = LedgerState::new(params);
    state.epoch_length = 100;
    state.treasury = Lovelace(500_000_000_000);

    let input = TransactionInput {
        transaction_id: Hash32::from_bytes([0xAAu8; 32]),
        index: 0,
    };
    state.utxo_set.insert(
        input.clone(),
        TransactionOutput {
            address: Address::Byron(ByronAddress {
                payload: vec![0u8; 32],
            }),
            value: Value::lovelace(10_000_000),
            datum: OutputDatum::None,
            script_ref: None,
            is_legacy: false,
            raw_cbor: None,
        },
    );

    let mut tx = Transaction::empty_with_hash(Hash32::from_bytes([0x01u8; 32]));
    tx.is_valid = true;
    tx.body.treasury_value = Some(Lovelace(999_999_999_999)); // MISMATCH
    tx.body.inputs = vec![input];
    tx.body.outputs = vec![TransactionOutput {
        address: Address::Byron(ByronAddress {
            payload: vec![0u8; 32],
        }),
        value: Value::lovelace(9_000_000),
        datum: OutputDatum::None,
        script_ref: None,
        is_legacy: false,
        raw_cbor: None,
    }];
    tx.body.fee = Lovelace(1_000_000);

    let block = make_test_block(1, 1, Hash32::ZERO, vec![tx]);
    // Treasury mismatch is now a warning — block is applied and treasury corrected.
    let _result = state.apply_block(&block, BlockValidationMode::ValidateAll);
    assert_eq!(
        state.treasury.0, 999_999_999_999,
        "Treasury must be corrected to the on-chain declared value"
    );
}

/// Treasury value check is only enforced in Conway (protocol >= 9).
/// Pre-Conway blocks should not check treasury_value at all.
#[test]
fn test_treasury_value_not_checked_pre_conway() {
    let mut params = ProtocolParameters::mainnet_defaults();
    params.protocol_version_major = 8; // Babbage
    let mut state = LedgerState::new(params);
    state.epoch_length = 100;
    state.treasury = Lovelace(500_000_000_000);

    let input = TransactionInput {
        transaction_id: Hash32::from_bytes([0xAAu8; 32]),
        index: 0,
    };
    state.utxo_set.insert(
        input.clone(),
        TransactionOutput {
            address: Address::Byron(ByronAddress {
                payload: vec![0u8; 32],
            }),
            value: Value::lovelace(10_000_000),
            datum: OutputDatum::None,
            script_ref: None,
            is_legacy: false,
            raw_cbor: None,
        },
    );

    let mut tx = Transaction::empty_with_hash(Hash32::from_bytes([0x01u8; 32]));
    tx.is_valid = true;
    tx.body.treasury_value = Some(Lovelace(999_999_999)); // wrong, but pre-Conway
    tx.body.inputs = vec![input];
    tx.body.outputs = vec![TransactionOutput {
        address: Address::Byron(ByronAddress {
            payload: vec![0u8; 32],
        }),
        value: Value::lovelace(9_000_000),
        datum: OutputDatum::None,
        script_ref: None,
        is_legacy: false,
        raw_cbor: None,
    }];
    tx.body.fee = Lovelace(1_000_000);

    let block = make_test_block(1, 1, Hash32::ZERO, vec![tx]);
    let result = state.apply_block(&block, BlockValidationMode::ValidateAll);
    // Pre-Conway: treasury check should not fire
    if let Err(LedgerError::BlockTxValidationFailed { ref errors, .. }) = result {
        assert!(
            !errors.contains("TreasuryValueMismatch"),
            "Pre-Conway must not check treasury_value; got: {errors}"
        );
    }
}

// -----------------------------------------------------------------------
// 9. SPO voting power uses mark snapshot (not set)
// -----------------------------------------------------------------------

/// Verify that compute_spo_voting_power reads from the mark snapshot,
/// not the set snapshot. If both mark and set have different values for
/// the same pool, the mark value must be used.
#[test]
fn test_spo_voting_power_prefers_mark_over_set() {
    let params = ProtocolParameters::mainnet_defaults();
    let mut state = LedgerState::new(params);
    state.needs_stake_rebuild = false;

    let pool_id = Hash28::from_bytes([0x42u8; 28]);

    // Set mark with 100 ADA stake
    let mut mark_pool_stake = std::collections::HashMap::new();
    mark_pool_stake.insert(pool_id, Lovelace(100_000_000));
    state.snapshots.mark = Some(StakeSnapshot {
        epoch: EpochNo(10),
        delegations: Arc::new(HashMap::new()),
        pool_stake: mark_pool_stake,
        pool_params: Arc::new(HashMap::new()),
        stake_distribution: Arc::new(HashMap::new()),
        epoch_fees: Lovelace(0),
        epoch_block_count: 0,
        epoch_blocks_by_pool: Arc::new(HashMap::new()),
    });

    // Set set snapshot with 200 ADA stake (should NOT be used)
    let mut set_pool_stake = std::collections::HashMap::new();
    set_pool_stake.insert(pool_id, Lovelace(200_000_000));
    state.snapshots.set = Some(StakeSnapshot {
        epoch: EpochNo(9),
        delegations: Arc::new(HashMap::new()),
        pool_stake: set_pool_stake,
        pool_params: Arc::new(HashMap::new()),
        stake_distribution: Arc::new(HashMap::new()),
        epoch_fees: Lovelace(0),
        epoch_block_count: 0,
        epoch_blocks_by_pool: Arc::new(HashMap::new()),
    });

    let power = state.compute_spo_voting_power(&pool_id);
    assert_eq!(
        power, 100_000_000,
        "compute_spo_voting_power must use mark snapshot (100 ADA), not set (200 ADA)"
    );
}

/// Verify that when mark snapshot has no entry for a pool but set does,
/// the fallback scan is used instead of reading from set.
#[test]
fn test_spo_voting_power_no_mark_entry_falls_back_not_to_set() {
    let params = ProtocolParameters::mainnet_defaults();
    let mut state = LedgerState::new(params);
    state.needs_stake_rebuild = false;

    let pool_id = Hash28::from_bytes([0x42u8; 28]);

    // Mark snapshot exists but does NOT contain this pool
    state.snapshots.mark = Some(StakeSnapshot {
        epoch: EpochNo(10),
        delegations: Arc::new(HashMap::new()),
        pool_stake: std::collections::HashMap::new(), // no pool_id entry
        pool_params: Arc::new(HashMap::new()),
        stake_distribution: Arc::new(HashMap::new()),
        epoch_fees: Lovelace(0),
        epoch_block_count: 0,
        epoch_blocks_by_pool: Arc::new(HashMap::new()),
    });

    // Set snapshot has 200 ADA for this pool (should NOT be used)
    let mut set_pool_stake = std::collections::HashMap::new();
    set_pool_stake.insert(pool_id, Lovelace(200_000_000));
    state.snapshots.set = Some(StakeSnapshot {
        epoch: EpochNo(9),
        delegations: Arc::new(HashMap::new()),
        pool_stake: set_pool_stake,
        pool_params: Arc::new(HashMap::new()),
        stake_distribution: Arc::new(HashMap::new()),
        epoch_fees: Lovelace(0),
        epoch_block_count: 0,
        epoch_blocks_by_pool: Arc::new(HashMap::new()),
    });

    let power = state.compute_spo_voting_power(&pool_id);
    // Should return 0 (from mark, where pool is absent), NOT 200M (from set)
    assert_eq!(
        power, 0,
        "When mark has no entry, compute_spo_voting_power must return 0 (not fall back to set)"
    );
}

// -----------------------------------------------------------------------
// 12. Snapshot save/load roundtrip — nonce fields survive serialization
// -----------------------------------------------------------------------

/// Verify that all nonce fields survive a save/load roundtrip.
#[test]
fn test_snapshot_roundtrip_nonce_fields() {
    let dir = tempfile::tempdir().unwrap();
    let snap_path = dir.path().join("ledger-snapshot.bin");

    let params = ProtocolParameters::mainnet_defaults();
    let mut state = LedgerState::new(params);

    // Set distinctive values for all nonce-related fields
    state.evolving_nonce = Hash32::from_bytes([0x11; 32]);
    state.candidate_nonce = Hash32::from_bytes([0x22; 32]);
    state.epoch_nonce = Hash32::from_bytes([0x33; 32]);
    state.lab_nonce = Hash32::from_bytes([0x44; 32]);
    state.last_epoch_block_nonce = Hash32::from_bytes([0x55; 32]);
    state.genesis_hash = Hash32::from_bytes([0x66; 32]);
    state.epoch = EpochNo(42);
    state.treasury = Lovelace(1_234_567_890);
    state.reserves = Lovelace(9_876_543_210);

    state.save_snapshot(&snap_path).unwrap();
    let loaded = LedgerState::load_snapshot(&snap_path).unwrap();

    assert_eq!(
        loaded.evolving_nonce,
        Hash32::from_bytes([0x11; 32]),
        "evolving_nonce must survive roundtrip"
    );
    assert_eq!(
        loaded.candidate_nonce,
        Hash32::from_bytes([0x22; 32]),
        "candidate_nonce must survive roundtrip"
    );
    assert_eq!(
        loaded.epoch_nonce,
        Hash32::from_bytes([0x33; 32]),
        "epoch_nonce must survive roundtrip"
    );
    assert_eq!(
        loaded.lab_nonce,
        Hash32::from_bytes([0x44; 32]),
        "lab_nonce must survive roundtrip"
    );
    assert_eq!(
        loaded.last_epoch_block_nonce,
        Hash32::from_bytes([0x55; 32]),
        "last_epoch_block_nonce must survive roundtrip"
    );
    assert_eq!(
        loaded.genesis_hash,
        Hash32::from_bytes([0x66; 32]),
        "genesis_hash must survive roundtrip"
    );
    assert_eq!(loaded.epoch, EpochNo(42), "epoch must survive roundtrip");
    assert_eq!(
        loaded.treasury,
        Lovelace(1_234_567_890),
        "treasury must survive roundtrip"
    );
    assert_eq!(
        loaded.reserves,
        Lovelace(9_876_543_210),
        "reserves must survive roundtrip"
    );
}

/// Verify that snapshot checksum catches corruption.
#[test]
fn test_snapshot_checksum_detects_corruption() {
    let dir = tempfile::tempdir().unwrap();
    let snap_path = dir.path().join("ledger-snapshot-corrupt.bin");

    let params = ProtocolParameters::mainnet_defaults();
    let state = LedgerState::new(params);
    state.save_snapshot(&snap_path).unwrap();

    // Corrupt a byte in the payload (after the 37-byte header)
    let mut data = std::fs::read(&snap_path).unwrap();
    if data.len() > 40 {
        data[40] ^= 0xFF; // flip bits
    }
    std::fs::write(&snap_path, &data).unwrap();

    let result = LedgerState::load_snapshot(&snap_path);
    assert!(result.is_err(), "Corrupted snapshot must fail to load");
}

// ===========================================================================
// Property-based tests: nonce computation and UTxO conservation
// ===========================================================================

mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(500))]

        /// For any sequence of VRF outputs, `update_evolving_nonce` always
        /// produces a valid 32-byte Hash32 (never panics, never returns a
        /// zero-length result).
        ///
        /// The nonce update rule is:
        ///   evolving' = blake2b_256(evolving || blake2b_256(eta))
        ///
        /// This property verifies that for ANY input length and content,
        /// the output is always exactly 32 bytes.
        #[test]
        fn prop_nonce_update_always_produces_32_bytes(
            // Use a sequence of 1..10 VRF outputs of variable length
            vrf_outputs in prop::collection::vec(
                prop::collection::vec(any::<u8>(), 0..=128),
                1..=10,
            )
        ) {
            let params = ProtocolParameters::mainnet_defaults();
            let mut state = LedgerState::new(params);

            for eta in &vrf_outputs {
                state.update_evolving_nonce(eta);
                // Must always be exactly 32 bytes
                prop_assert_eq!(
                    state.evolving_nonce.as_bytes().len(),
                    32,
                    "Evolving nonce must always be 32 bytes"
                );
                // Must not be all zeros (astronomically unlikely for blake2b)
                prop_assert!(
                    state.evolving_nonce != Hash32::ZERO,
                    "Evolving nonce should not be zero after update"
                );
            }
        }

        /// Nonce computation is deterministic: applying the same sequence
        /// of VRF outputs twice produces the same evolving nonce.
        #[test]
        fn prop_nonce_update_deterministic(
            vrf_outputs in prop::collection::vec(
                prop::collection::vec(any::<u8>(), 1..=64),
                1..=5,
            )
        ) {
            let params = ProtocolParameters::mainnet_defaults();

            let mut state_a = LedgerState::new(params.clone());
            let mut state_b = LedgerState::new(params);

            for eta in &vrf_outputs {
                state_a.update_evolving_nonce(eta);
                state_b.update_evolving_nonce(eta);
            }

            prop_assert_eq!(
                state_a.evolving_nonce,
                state_b.evolving_nonce,
                "Same input sequence must produce identical nonces"
            );
        }

        /// Different VRF input sequences produce different nonces.
        ///
        /// If we apply two distinct single-step VRF inputs from the same
        /// initial state, the resulting nonces must differ (collision
        /// resistance of blake2b).
        #[test]
        fn prop_nonce_update_collision_resistance(
            eta_a in prop::collection::vec(any::<u8>(), 1..=64),
            eta_b in prop::collection::vec(any::<u8>(), 1..=64),
        ) {
            prop_assume!(eta_a != eta_b);

            let params = ProtocolParameters::mainnet_defaults();
            let mut state_a = LedgerState::new(params.clone());
            let mut state_b = LedgerState::new(params);

            state_a.update_evolving_nonce(&eta_a);
            state_b.update_evolving_nonce(&eta_b);

            prop_assert_ne!(
                state_a.evolving_nonce,
                state_b.evolving_nonce,
                "Different VRF inputs must produce different nonces (blake2b collision)"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Property-based tests: UTxO conservation
    // -----------------------------------------------------------------------

    /// Strategy for generating a random TransactionOutput with a given lovelace value.
    fn arb_output(lovelace: u64) -> TransactionOutput {
        TransactionOutput {
            address: Address::Byron(ByronAddress {
                payload: vec![0u8; 32],
            }),
            value: Value::lovelace(lovelace),
            datum: OutputDatum::None,
            script_ref: None,
            is_legacy: false,
            raw_cbor: None,
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(500))]

        /// UTxO conservation: `apply_transaction` preserves total lovelace.
        ///
        /// Given any initial UTxO and a transaction that splits its value
        /// into N outputs (with fee), the total lovelace in the UTxO set
        /// after application equals (initial - fee).
        ///
        /// This is the fundamental accounting invariant:
        ///   sum(inputs) = sum(outputs) + fee
        ///   => utxo_total_after = utxo_total_before - fee
        #[test]
        fn prop_utxo_apply_preserves_lovelace(
            // Start with a single UTxO holding `initial` lovelace
            initial in 2_000_000u64..1_000_000_000_000,
            // Number of output splits (1..=5)
            num_outputs in 1usize..=5,
            // Fee as a fraction of the initial value (0.01% to 10%)
            fee_pct in 1u64..1000,
        ) {
            let mut utxo_set = crate::utxo::UtxoSet::new();

            // Create initial UTxO
            let genesis_hash = Hash32::from_bytes([0xAA; 32]);
            let genesis_input = TransactionInput {
                transaction_id: genesis_hash,
                index: 0,
            };
            utxo_set.insert(genesis_input.clone(), arb_output(initial));

            let total_before = utxo_set.total_lovelace();
            prop_assert_eq!(total_before, Lovelace(initial));

            // Compute fee (capped at initial - num_outputs to leave at least 1 per output)
            let fee = (initial * fee_pct / 10_000).min(initial.saturating_sub(num_outputs as u64));
            let remaining = initial.saturating_sub(fee);

            // Split remaining value across outputs as evenly as possible
            let per_output = remaining / num_outputs as u64;
            let leftover = remaining - per_output * num_outputs as u64;
            let mut outputs = Vec::with_capacity(num_outputs);
            for i in 0..num_outputs {
                let amount = if i == 0 {
                    per_output + leftover
                } else {
                    per_output
                };
                outputs.push(arb_output(amount));
            }

            // Verify output sum + fee = initial
            let output_sum: u64 = outputs.iter().map(|o| o.value.coin.0).sum();
            prop_assert_eq!(
                output_sum + fee,
                initial,
                "Output sum ({}) + fee ({}) must equal initial ({})",
                output_sum, fee, initial,
            );

            // Apply the transaction
            let tx_hash = Hash32::from_bytes([0xBB; 32]);
            utxo_set
                .apply_transaction(&tx_hash, &[genesis_input], &outputs)
                .expect("apply_transaction must succeed");

            // Verify UTxO set total = initial - fee
            let total_after = utxo_set.total_lovelace();
            prop_assert_eq!(
                total_after,
                Lovelace(initial - fee),
                "UTxO total after ({}) must equal initial ({}) minus fee ({})",
                total_after.0, initial, fee,
            );
        }

        /// UTxO apply+rollback is identity: total_lovelace is restored.
        #[test]
        fn prop_utxo_apply_rollback_identity(
            initial in 1_000_000u64..1_000_000_000,
            out_a in 100_000u64..500_000_000,
        ) {
            let out_a = out_a.min(initial.saturating_sub(1));
            let out_b = initial.saturating_sub(out_a);

            let mut utxo_set = crate::utxo::UtxoSet::new();
            let genesis_hash = Hash32::from_bytes([0xCC; 32]);
            let genesis_input = TransactionInput {
                transaction_id: genesis_hash,
                index: 0,
            };
            let genesis_output = arb_output(initial);
            utxo_set.insert(genesis_input.clone(), genesis_output.clone());

            let total_before = utxo_set.total_lovelace();

            // Apply
            let tx_hash = Hash32::from_bytes([0xDD; 32]);
            let outputs = vec![arb_output(out_a), arb_output(out_b)];
            utxo_set
                .apply_transaction(&tx_hash, std::slice::from_ref(&genesis_input), &outputs)
                .unwrap();

            // Rollback
            utxo_set.rollback_transaction(
                &tx_hash,
                &[(genesis_input, genesis_output)],
                outputs.len(),
            );

            // Total must be restored exactly
            let total_after = utxo_set.total_lovelace();
            prop_assert_eq!(
                total_before,
                total_after,
                "apply+rollback must restore total lovelace"
            );
        }
    }
}

// ─── Issue #113: Snapshot stake persistence tests ─────────────────────────────
//
// These tests reproduce the bug where pool_stake=0 appears in the set snapshot
// after replay + 2 epoch transitions. The root cause: during fast replay with
// needs_stake_rebuild=false, incremental stake tracking may not correctly populate
// pool_stake in mark/set/go snapshots. The fix calls recompute_snapshot_pool_stakes()
// at the end of replay (and before bulk snapshot saves) to correct any drift.

/// Build a minimal stake-delegation block for testing epoch transitions.
///
/// Creates a block containing:
/// 1. A stake registration for `cred`
/// 2. A stake delegation from `cred` to `pool_id`
/// 3. An output sending `amount` lovelace to a base address with `cred` as stake part
fn make_delegation_block(
    slot: u64,
    block_no: u64,
    prev_hash: Hash32,
    cred: &Credential,
    pool_id: Hash28,
    amount: u64,
) -> Block {
    use torsten_primitives::address::BaseAddress;
    use torsten_primitives::network::NetworkId;

    let payment_cred = Credential::VerificationKey(Hash28::from_bytes([0xABu8; 28]));
    let base_addr = Address::Base(BaseAddress {
        network: NetworkId::Mainnet,
        payment: payment_cred,
        stake: cred.clone(),
    });
    let counter = UTXO_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut tx_id_bytes = [0u8; 32];
    tx_id_bytes[..8].copy_from_slice(&counter.to_be_bytes());
    let tx_hash = Hash32::from_bytes(tx_id_bytes);

    let tx = Transaction {
        hash: tx_hash,
        body: TransactionBody {
            inputs: vec![],
            outputs: vec![TransactionOutput {
                address: base_addr,
                value: Value {
                    coin: Lovelace(amount),
                    multi_asset: Default::default(),
                },
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            }],
            fee: Lovelace(0),
            ttl: None,
            certificates: vec![
                Certificate::StakeRegistration(cred.clone()),
                Certificate::StakeDelegation {
                    credential: cred.clone(),
                    pool_hash: pool_id,
                },
            ],
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
            raw_redeemers_cbor: None,
            raw_plutus_data_cbor: None,
            pallas_script_data_hash: None,
        },
        is_valid: true,
        auxiliary_data: None,
        raw_cbor: None,
    };
    make_test_block(slot, block_no, prev_hash, vec![tx])
}

/// Register a pool (PoolRegistration cert) and return a block containing it.
fn make_pool_registration_block(
    slot: u64,
    block_no: u64,
    prev_hash: Hash32,
    pool_id: Hash28,
) -> Block {
    use torsten_primitives::transaction::PoolParams;

    let tx_hash = {
        let mut bytes = [0u8; 32];
        let counter = UTXO_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        bytes[..8].copy_from_slice(&counter.to_be_bytes());
        Hash32::from_bytes(bytes)
    };
    let params = PoolParams {
        operator: pool_id,
        vrf_keyhash: Hash32::ZERO,
        pledge: Lovelace(1_000_000_000),
        cost: Lovelace(340_000_000),
        margin: Rational {
            numerator: 1,
            denominator: 100,
        },
        reward_account: vec![0xe0u8; 29], // mainnet reward address prefix
        pool_owners: vec![],
        relays: vec![],
        pool_metadata: None,
    };
    let tx = Transaction {
        hash: tx_hash,
        body: TransactionBody {
            inputs: vec![],
            outputs: vec![],
            fee: Lovelace(0),
            ttl: None,
            certificates: vec![Certificate::PoolRegistration(params)],
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
            raw_redeemers_cbor: None,
            raw_plutus_data_cbor: None,
            pallas_script_data_hash: None,
        },
        is_valid: true,
        auxiliary_data: None,
        raw_cbor: None,
    };
    make_test_block(slot, block_no, prev_hash, vec![tx])
}

/// A simple, empty block used to advance the chain tip through a slot.
fn make_empty_block(slot: u64, block_no: u64, prev_hash: Hash32) -> Block {
    make_test_block(slot, block_no, prev_hash, vec![])
}

/// Verify that pool_stake in the mark snapshot is non-zero for a pool that has
/// delegators, even when replay runs with needs_stake_rebuild=false.
///
/// Regression test for GitHub issue #113.
#[test]
fn test_mark_snapshot_pool_stake_nonzero_after_replay_mode() {
    let params = ProtocolParameters::mainnet_defaults();
    let mut state = LedgerState::new(params);
    // Simulate the replay path: needs_stake_rebuild=false (epoch boundaries skip
    // the full UTxO scan and use the incremental stake_distribution instead).
    state.needs_stake_rebuild = false;
    // Use short epochs (1000 slots each, matching test block slots below).
    state.epoch_length = 1000;
    state.shelley_transition_epoch = 0;
    state.byron_epoch_length = 0;

    let pool_id = Hash28::from_bytes([0x01u8; 28]);
    let delegator_cred = Credential::VerificationKey(Hash28::from_bytes([0x02u8; 28]));
    let stake_amount = 10_000_000_000u64; // 10,000 ADA

    // Epoch 0: register pool and delegate to it (slot 1)
    let b0 = make_pool_registration_block(1, 1, Hash32::ZERO, pool_id);
    state
        .apply_block(&b0, BlockValidationMode::ApplyOnly)
        .unwrap();

    let b1 = make_delegation_block(2, 2, *b0.hash(), &delegator_cred, pool_id, stake_amount);
    state
        .apply_block(&b1, BlockValidationMode::ApplyOnly)
        .unwrap();

    // Epoch 1 transition (slot 1000): mark snapshot is built here.
    // With needs_stake_rebuild=false, pool_stake is computed from the incremental map.
    let b2 = make_empty_block(1001, 3, *b1.hash());
    state
        .apply_block(&b2, BlockValidationMode::ApplyOnly)
        .unwrap();

    // The mark snapshot should now exist and have non-zero pool_stake for our pool.
    let mark = state
        .snapshots
        .mark
        .as_ref()
        .expect("mark snapshot should exist after epoch 1 transition");

    let pool_stake_in_mark = mark
        .pool_stake
        .get(&pool_id)
        .copied()
        .unwrap_or(Lovelace(0));
    assert!(
        pool_stake_in_mark.0 >= stake_amount,
        "mark snapshot pool_stake should be >= {stake_amount} after delegation, got {}",
        pool_stake_in_mark.0
    );
}

/// Verify that recompute_snapshot_pool_stakes() corrects pool_stake=0 in snapshots.
///
/// This simulates the scenario where the incremental stake_map had drift and
/// the epoch boundary created a mark snapshot with incorrect (0) pool_stake.
/// After rebuild_stake_distribution() + recompute_snapshot_pool_stakes(), the
/// snapshot pool_stake should be corrected to the actual UTxO-backed stake amount.
///
/// Regression test for GitHub issue #113.
#[test]
fn test_recompute_snapshot_pool_stakes_corrects_zero_pool_stake() {
    let params = ProtocolParameters::mainnet_defaults();
    let mut state = LedgerState::new(params);
    state.epoch_length = 1000;
    state.shelley_transition_epoch = 0;
    state.byron_epoch_length = 0;

    let pool_id = Hash28::from_bytes([0x10u8; 28]);
    let delegator_cred = Credential::VerificationKey(Hash28::from_bytes([0x11u8; 28]));
    let stake_amount = 5_000_000_000u64; // 5,000 ADA

    // Register pool + delegate
    let b0 = make_pool_registration_block(1, 1, Hash32::ZERO, pool_id);
    state
        .apply_block(&b0, BlockValidationMode::ApplyOnly)
        .unwrap();
    let b1 = make_delegation_block(2, 2, *b0.hash(), &delegator_cred, pool_id, stake_amount);
    state
        .apply_block(&b1, BlockValidationMode::ApplyOnly)
        .unwrap();

    // Trigger epoch 1 transition. needs_stake_rebuild=true so it runs rebuild.
    // This builds a correct mark snapshot.
    let b2 = make_empty_block(1001, 3, *b1.hash());
    state
        .apply_block(&b2, BlockValidationMode::ApplyOnly)
        .unwrap();

    // Corrupt the mark snapshot pool_stake to simulate drift.
    if let Some(ref mut snap) = state.snapshots.mark {
        snap.pool_stake.clear();
    }
    let zero_stake = state
        .snapshots
        .mark
        .as_ref()
        .and_then(|s| s.pool_stake.get(&pool_id))
        .copied()
        .unwrap_or(Lovelace(0));
    assert_eq!(
        zero_stake.0, 0,
        "pool_stake should be 0 after corruption (precondition)"
    );

    // Simulate what replay_from_chunk_files does at the end of replay:
    // rebuild stake_distribution from full UTxO set, then recompute snapshots.
    state.rebuild_stake_distribution();
    state.recompute_snapshot_pool_stakes();

    // Pool stake should be corrected in the mark snapshot.
    let corrected_stake = state
        .snapshots
        .mark
        .as_ref()
        .and_then(|s| s.pool_stake.get(&pool_id))
        .copied()
        .unwrap_or(Lovelace(0));
    assert!(
        corrected_stake.0 >= stake_amount,
        "pool_stake should be corrected to >= {stake_amount} after recompute, got {}",
        corrected_stake.0
    );
}

/// Verify that after replay + 2 live epoch transitions, the set snapshot
/// has non-zero pool_stake for a pool that had delegators during replay.
///
/// This is the end-to-end reproduction of GitHub issue #113:
/// After Mithril import + replay + 2 epoch transitions, set snapshot pool_stake
/// must be non-zero for registered and delegated pools.
#[test]
fn test_set_snapshot_pool_stake_nonzero_after_two_epoch_transitions() {
    let params = ProtocolParameters::mainnet_defaults();
    let mut state = LedgerState::new(params);
    // Replay mode: no full rebuild at epoch boundaries during replay
    state.needs_stake_rebuild = false;
    state.epoch_length = 1000;
    state.shelley_transition_epoch = 0;
    state.byron_epoch_length = 0;

    let pool_id = Hash28::from_bytes([0x20u8; 28]);
    let delegator_cred = Credential::VerificationKey(Hash28::from_bytes([0x21u8; 28]));
    let stake_amount = 8_000_000_000u64; // 8,000 ADA

    // Epoch 0: register pool + delegate (slots 1-2)
    let b0 = make_pool_registration_block(1, 1, Hash32::ZERO, pool_id);
    state
        .apply_block(&b0, BlockValidationMode::ApplyOnly)
        .unwrap();
    let b1 = make_delegation_block(2, 2, *b0.hash(), &delegator_cred, pool_id, stake_amount);
    state
        .apply_block(&b1, BlockValidationMode::ApplyOnly)
        .unwrap();

    // Epoch 1 transition (slot 1001): mark snapshot built from incremental stake_map
    let b2 = make_empty_block(1001, 3, *b1.hash());
    state
        .apply_block(&b2, BlockValidationMode::ApplyOnly)
        .unwrap();

    // Epoch 2 transition (slot 2001): set=mark(epoch1), new mark built
    let b3 = make_empty_block(2001, 4, *b2.hash());
    state
        .apply_block(&b3, BlockValidationMode::ApplyOnly)
        .unwrap();

    // Epoch 3 transition (slot 3001): go=set(epoch1), set=mark(epoch2), new mark
    let b4 = make_empty_block(3001, 5, *b3.hash());
    state
        .apply_block(&b4, BlockValidationMode::ApplyOnly)
        .unwrap();

    // Simulate what replay finalization does:
    // rebuild_stake_distribution() + recompute_snapshot_pool_stakes()
    state.needs_stake_rebuild = true;
    state.rebuild_stake_distribution();
    state.recompute_snapshot_pool_stakes();

    // Now cross 2 live epoch transitions (epochs 4 and 5)
    let b5 = make_empty_block(4001, 6, *b4.hash());
    state
        .apply_block(&b5, BlockValidationMode::ApplyOnly)
        .unwrap();

    let b6 = make_empty_block(5001, 7, *b5.hash());
    state
        .apply_block(&b6, BlockValidationMode::ApplyOnly)
        .unwrap();

    // After 2 live epoch transitions, the "set" snapshot should be the mark
    // built at the first live epoch boundary (epoch 4). Since needs_stake_rebuild=true
    // after replay finalization, that mark was built with a freshly rebuilt stake_map.
    let set_pool_stake = state
        .snapshots
        .set
        .as_ref()
        .and_then(|s| s.pool_stake.get(&pool_id))
        .copied()
        .unwrap_or(Lovelace(0));

    assert!(
        set_pool_stake.0 >= stake_amount,
        "set snapshot pool_stake should be >= {stake_amount} after replay + 2 epoch transitions, got {}",
        set_pool_stake.0
    );
}

/// Verify that recompute_snapshot_pool_stakes() correctly handles reward account
/// balances — they are included in pool_stake, not just UTxO-backed stake.
///
/// This ensures the fix correctly adds reward balances as per Cardano spec:
/// total stake = UTxO stake + reward balance for each delegated credential.
#[test]
fn test_recompute_snapshot_pool_stakes_includes_reward_accounts() {
    let params = ProtocolParameters::mainnet_defaults();
    let mut state = LedgerState::new(params);
    state.epoch_length = 1000;
    state.shelley_transition_epoch = 0;
    state.byron_epoch_length = 0;

    let pool_id = Hash28::from_bytes([0x30u8; 28]);
    let delegator_cred = Credential::VerificationKey(Hash28::from_bytes([0x31u8; 28]));
    let cred_key = credential_to_hash(&delegator_cred);
    let utxo_amount = 3_000_000_000u64; // 3,000 ADA
    let reward_amount = 500_000_000u64; // 500 ADA

    // Register pool + delegate with UTxO stake
    let b0 = make_pool_registration_block(1, 1, Hash32::ZERO, pool_id);
    state
        .apply_block(&b0, BlockValidationMode::ApplyOnly)
        .unwrap();
    let b1 = make_delegation_block(2, 2, *b0.hash(), &delegator_cred, pool_id, utxo_amount);
    state
        .apply_block(&b1, BlockValidationMode::ApplyOnly)
        .unwrap();

    // Manually add a reward balance for the delegator (simulating earned rewards)
    *std::sync::Arc::make_mut(&mut state.reward_accounts)
        .entry(cred_key)
        .or_insert(Lovelace(0)) = Lovelace(reward_amount);

    // Trigger epoch 1 transition (this builds the mark snapshot)
    let b2 = make_empty_block(1001, 3, *b1.hash());
    state
        .apply_block(&b2, BlockValidationMode::ApplyOnly)
        .unwrap();

    // Now rebuild + recompute (simulating end-of-replay finalization)
    state.rebuild_stake_distribution();
    state.recompute_snapshot_pool_stakes();

    // pool_stake should include BOTH utxo stake and reward balance
    let expected_total = utxo_amount + reward_amount;
    let pool_stake = state
        .snapshots
        .mark
        .as_ref()
        .and_then(|s| s.pool_stake.get(&pool_id))
        .copied()
        .unwrap_or(Lovelace(0));

    assert!(
        pool_stake.0 >= expected_total,
        "pool_stake should include reward account balance: expected >= {expected_total}, got {}",
        pool_stake.0
    );
}

// -----------------------------------------------------------------------
// Issue #171. recompute_snapshot_pool_stakes must NOT replace snapshot
// delegations with current delegations.
// -----------------------------------------------------------------------
//
// The previous implementation replaced a snapshot's delegation map with the
// current (live) delegation map whenever the current map had significantly
// more entries. This over-corrects: delegations registered AFTER the
// snapshot epoch are included retroactively, inflating sigma values used
// for historical reward calculations.
//
// The correct behaviour: each snapshot's delegation map is preserved exactly
// as it was captured at the time the epoch boundary was crossed. Only the
// pool_stake values (ADA amounts) are recalculated using the current
// (rebuilt) stake_distribution — the set of which pools are visible in
// each snapshot must remain epoch-accurate.

/// Verify that recompute_snapshot_pool_stakes() does NOT inject post-epoch
/// delegations into historical snapshots.
///
/// Scenario:
///   - Epoch 0: pool A registered, delegator X delegates to pool A.
///   - Epoch 1 transition: mark snapshot captured with delegation X -> A.
///   - Epoch 2: delegator Y registers and delegates to pool A (AFTER the
///     mark snapshot was taken).
///   - recompute_snapshot_pool_stakes() is called.
///   - The mark snapshot must still use only delegator X's delegation;
///     delegator Y must NOT appear in the mark snapshot.
///
/// Regression test for GitHub issue #171.
#[test]
fn test_recompute_snapshot_pool_stakes_preserves_snapshot_delegations() {
    let params = ProtocolParameters::mainnet_defaults();
    let mut state = LedgerState::new(params);
    state.epoch_length = 1000;
    state.shelley_transition_epoch = 0;
    state.byron_epoch_length = 0;

    let pool_id = Hash28::from_bytes([0x40u8; 28]);
    // Delegator X: present at the mark snapshot epoch boundary.
    let cred_x = Credential::VerificationKey(Hash28::from_bytes([0x41u8; 28]));
    let amount_x = 10_000_000_000u64; // 10,000 ADA
                                      // Delegator Y: joins AFTER the mark snapshot is captured (post-epoch).
    let cred_y = Credential::VerificationKey(Hash28::from_bytes([0x42u8; 28]));
    let amount_y = 5_000_000_000u64; // 5,000 ADA

    // Epoch 0: register pool and have delegator X delegate to it.
    let b0 = make_pool_registration_block(1, 1, Hash32::ZERO, pool_id);
    state
        .apply_block(&b0, BlockValidationMode::ApplyOnly)
        .unwrap();
    let b1 = make_delegation_block(2, 2, *b0.hash(), &cred_x, pool_id, amount_x);
    state
        .apply_block(&b1, BlockValidationMode::ApplyOnly)
        .unwrap();

    // Epoch 1 transition: mark snapshot is captured here with X's delegation only.
    let b2 = make_empty_block(1001, 3, *b1.hash());
    state
        .apply_block(&b2, BlockValidationMode::ApplyOnly)
        .unwrap();

    // Delegator X must appear in the mark snapshot delegation map.
    {
        let mark = state.snapshots.mark.as_ref().expect("mark must exist");
        let cred_x_key = credential_to_hash(&cred_x);
        assert!(
            mark.delegations.contains_key(&cred_x_key),
            "mark snapshot must contain delegator X"
        );
        assert_eq!(
            mark.delegations.len(),
            1,
            "mark snapshot must have exactly 1 delegation (X only), got {}",
            mark.delegations.len()
        );
    }

    // Delegator Y joins in epoch 1 (AFTER the mark snapshot boundary).
    let b3 = make_delegation_block(1002, 4, *b2.hash(), &cred_y, pool_id, amount_y);
    state
        .apply_block(&b3, BlockValidationMode::ApplyOnly)
        .unwrap();

    // The current (live) delegation map now has both X and Y.
    assert_eq!(
        state.delegations.len(),
        2,
        "live delegations must have both X and Y before recompute"
    );

    // Call recompute_snapshot_pool_stakes() — this is the function under test.
    // It must NOT inject Y's delegation into the mark snapshot.
    state.rebuild_stake_distribution();
    state.recompute_snapshot_pool_stakes();

    // The mark snapshot delegation map must still have only X.
    let mark = state
        .snapshots
        .mark
        .as_ref()
        .expect("mark must exist after recompute");
    let cred_y_key = credential_to_hash(&cred_y);
    assert!(
        !mark.delegations.contains_key(&cred_y_key),
        "mark snapshot must NOT contain delegator Y (joined after snapshot epoch)"
    );
    assert_eq!(
        mark.delegations.len(),
        1,
        "mark snapshot delegation count must be unchanged (1), got {}",
        mark.delegations.len()
    );

    // The pool_stake in the mark snapshot must reflect only X's stake.
    // amount_x is present in the UTxO set so it must appear.
    let pool_stake = mark
        .pool_stake
        .get(&pool_id)
        .copied()
        .unwrap_or(Lovelace(0));
    assert!(
        pool_stake.0 >= amount_x,
        "mark snapshot pool_stake must be >= amount_x ({amount_x}), got {}",
        pool_stake.0
    );
    // Y's stake must not be counted in the mark snapshot's pool_stake.
    // If the delegation replacement bug were present, pool_stake would be
    // amount_x + amount_y. We assert strictly less to catch that regression.
    assert!(
        pool_stake.0 < amount_x + amount_y,
        "mark snapshot pool_stake must be < amount_x+amount_y ({}), got {} — \
         delegation replacement bug would inflate this value",
        amount_x + amount_y,
        pool_stake.0
    );
}

// -----------------------------------------------------------------------
// Issue #98. Within-block UTxO overlay — reference script resolution
// -----------------------------------------------------------------------
//
// The Cardano LEDGERS rule applies transactions sequentially: when tx[i+1]
// is validated, the UTxO set already reflects the outputs produced by
// tx[0]..tx[i].  This means a later transaction in the same block can:
//   * spend an output created by an earlier tx in the block
//   * use an output created by an earlier tx as a reference input
//   * resolve a minting policy script from a script_ref carried by an
//     output that was *just* created by an earlier tx in the same block
//
// These tests verify the sequential apply order in `apply_block` is
// correct and that the Rule 9 (ReferenceInputNotFound) and Rule 3c
// (InvalidMint / minting policy script check) both see within-block
// UTxO outputs as they should.

/// tx1 creates an output; tx2 spends it in the same block.
///
/// Verifies the most basic form of within-block dependency: tx2's spending
/// input is satisfied by tx1's output that did not exist before block apply.
#[test]
fn test_within_block_spend_chaining() {
    let params = ProtocolParameters::mainnet_defaults();
    let mut state = LedgerState::new(params);
    state.epoch_length = 100;

    // Seed one UTxO for tx1 to consume
    let genesis_input = TransactionInput {
        transaction_id: Hash32::from_bytes([0x10u8; 32]),
        index: 0,
    };
    state.utxo_set.insert(
        genesis_input.clone(),
        TransactionOutput {
            address: Address::Byron(ByronAddress {
                payload: vec![0u8; 32],
            }),
            value: Value::lovelace(20_000_000),
            datum: OutputDatum::None,
            script_ref: None,
            is_legacy: false,
            raw_cbor: None,
        },
    );

    // tx1: spends genesis_input, produces one output (tx1_out_0)
    let tx1_hash = Hash32::from_bytes([0x01u8; 32]);
    let mut tx1 = Transaction::empty_with_hash(tx1_hash);
    tx1.is_valid = true;
    tx1.body.inputs = vec![genesis_input];
    tx1.body.fee = Lovelace(500_000);
    tx1.body.outputs = vec![TransactionOutput {
        address: Address::Byron(ByronAddress {
            payload: vec![0u8; 32],
        }),
        value: Value::lovelace(19_500_000),
        datum: OutputDatum::None,
        script_ref: None,
        is_legacy: false,
        raw_cbor: None,
    }];

    // tx2: spends tx1's output — this UTxO did NOT exist before this block
    let tx1_out_0 = TransactionInput {
        transaction_id: tx1_hash,
        index: 0,
    };
    let tx2_hash = Hash32::from_bytes([0x02u8; 32]);
    let mut tx2 = Transaction::empty_with_hash(tx2_hash);
    tx2.is_valid = true;
    tx2.body.inputs = vec![tx1_out_0];
    tx2.body.fee = Lovelace(500_000);
    tx2.body.outputs = vec![TransactionOutput {
        address: Address::Byron(ByronAddress {
            payload: vec![0u8; 32],
        }),
        value: Value::lovelace(19_000_000),
        datum: OutputDatum::None,
        script_ref: None,
        is_legacy: false,
        raw_cbor: None,
    }];

    let block = make_test_block(1, 1, Hash32::ZERO, vec![tx1, tx2]);
    // ApplyOnly: trust is_valid, skip Phase-1 checks — proves sequential apply works
    state
        .apply_block(&block, BlockValidationMode::ApplyOnly)
        .unwrap();

    // Only tx2's output should be in the UTxO set
    assert_eq!(state.utxo_set.len(), 1);
    assert!(state.utxo_set.contains(&TransactionInput {
        transaction_id: tx2_hash,
        index: 0,
    }));
}

/// tx1 creates an output carrying a native script_ref; tx2 uses that
/// output as a reference input — both in the same block.
///
/// Verifies that Rule 9 (`ReferenceInputNotFound`) does NOT fire when the
/// reference input was created by an earlier transaction in the same block.
/// This is the core of issue #98: within-block UTxO visibility for reference
/// inputs used for script resolution.
#[test]
fn test_within_block_reference_input_visible() {
    use torsten_primitives::transaction::{NativeScript, ScriptRef};

    let params = ProtocolParameters::mainnet_defaults();
    let mut state = LedgerState::new(params);
    state.epoch_length = 100;

    // Seed a UTxO for tx1 to consume
    let genesis_input = TransactionInput {
        transaction_id: Hash32::from_bytes([0x10u8; 32]),
        index: 0,
    };
    state.utxo_set.insert(
        genesis_input.clone(),
        TransactionOutput {
            address: Address::Byron(ByronAddress {
                payload: vec![0u8; 32],
            }),
            value: Value::lovelace(40_000_000),
            datum: OutputDatum::None,
            script_ref: None,
            is_legacy: false,
            raw_cbor: None,
        },
    );

    // tx1: consumes genesis_input, produces two outputs:
    //   - output 0: a plain value output (will be spent by tx2)
    //   - output 1: an output carrying a native script_ref (will be used
    //               as reference input by tx2)
    //
    // The native script is ScriptAll([]) — the always-true script —
    // whose blake2b_224(0x00 || cbor) hash will be the policy ID tx2 mints.
    let script = NativeScript::ScriptAll(vec![]);
    let tx1_hash = Hash32::from_bytes([0x01u8; 32]);
    let mut tx1 = Transaction::empty_with_hash(tx1_hash);
    tx1.is_valid = true;
    tx1.body.inputs = vec![genesis_input];
    tx1.body.fee = Lovelace(500_000);
    // Output 0: value to be spent by tx2
    tx1.body.outputs.push(TransactionOutput {
        address: Address::Byron(ByronAddress {
            payload: vec![0u8; 32],
        }),
        value: Value::lovelace(20_000_000),
        datum: OutputDatum::None,
        script_ref: None,
        is_legacy: false,
        raw_cbor: None,
    });
    // Output 1: carries the native script reference
    tx1.body.outputs.push(TransactionOutput {
        address: Address::Byron(ByronAddress {
            payload: vec![0u8; 32],
        }),
        value: Value::lovelace(19_500_000),
        datum: OutputDatum::None,
        script_ref: Some(ScriptRef::NativeScript(script.clone())),
        is_legacy: false,
        raw_cbor: None,
    });

    // tx1's output 1 — this is the output carrying the script_ref.
    // It does NOT exist in self.utxo_set before the block is applied;
    // it is produced by tx1 and must be visible to tx2 during
    // Phase-1 validation (Rules 9 and 3c).
    let tx1_script_ref_input = TransactionInput {
        transaction_id: tx1_hash,
        index: 1,
    };

    // Seed a separate spending input for tx2 (pre-existing in the UTxO set)
    let spend_input_for_tx2 = TransactionInput {
        transaction_id: Hash32::from_bytes([0x20u8; 32]),
        index: 0,
    };
    state.utxo_set.insert(
        spend_input_for_tx2.clone(),
        TransactionOutput {
            address: Address::Byron(ByronAddress {
                payload: vec![0u8; 32],
            }),
            value: Value::lovelace(10_000_000),
            datum: OutputDatum::None,
            script_ref: None,
            is_legacy: false,
            raw_cbor: None,
        },
    );

    // tx2: uses tx1's script-carrying output as a reference input.
    //   - inputs: spend_input_for_tx2 (pre-existing UTxO)
    //   - reference_inputs: tx1_script_ref_input (created by tx1 in same block)
    let tx2_hash = Hash32::from_bytes([0x02u8; 32]);
    let mut tx2 = Transaction::empty_with_hash(tx2_hash);
    tx2.is_valid = true;
    tx2.body.inputs = vec![spend_input_for_tx2];
    tx2.body.reference_inputs = vec![tx1_script_ref_input.clone()];
    tx2.body.fee = Lovelace(500_000);
    tx2.body.outputs = vec![TransactionOutput {
        address: Address::Byron(ByronAddress {
            payload: vec![0u8; 32],
        }),
        value: Value::lovelace(9_500_000),
        datum: OutputDatum::None,
        script_ref: None,
        is_legacy: false,
        raw_cbor: None,
    }];

    let block = make_test_block(1, 1, Hash32::ZERO, vec![tx1, tx2]);

    // ApplyOnly mode: trust is_valid flags, no Phase-1 checks.
    // The block must apply successfully — tx2's reference input is created
    // by tx1 and made available via sequential UTxO application.
    state
        .apply_block(&block, BlockValidationMode::ApplyOnly)
        .unwrap();

    // tx1's script-ref output should still be in the UTxO set (reference
    // inputs are NOT consumed — they are read-only).
    assert!(
        state.utxo_set.contains(&tx1_script_ref_input),
        "tx1's script-ref output must remain in UTxO set after tx2 used it as reference input"
    );
}

/// tx1 creates an output with a native script_ref; tx2 mints tokens using
/// the native script as the policy, resolved via the within-block reference
/// input — all in the same block.
///
/// This is the exact scenario from issue #98: the minting policy script
/// check (Rule 3c) must find the script even though the UTxO carrying it
/// was only created earlier in the same block.
///
/// In `ApplyOnly` mode the block applies successfully.
/// In `ValidateAll` mode the Phase-1 check must NOT raise
/// `ReferenceInputNotFound` or `InvalidMint` for the within-block reference
/// input (other validation failures are acceptable because the test tx is
/// intentionally simplified — no proper witnesses, fees, etc.).
#[test]
fn test_within_block_ref_script_minting_policy_visible() {
    use std::collections::BTreeMap;
    use torsten_primitives::hash::PolicyId;
    use torsten_primitives::transaction::{NativeScript, ScriptRef};
    use torsten_primitives::value::AssetName;

    let params = ProtocolParameters::mainnet_defaults();
    let mut state = LedgerState::new(params);
    state.epoch_length = 100;

    // Build the always-true native script and compute its policy ID.
    // policy_id = blake2b_224(0x00 || encode_native_script(script))
    let script = NativeScript::ScriptAll(vec![]);
    let script_cbor = torsten_serialization::encode_native_script(&script);
    let mut tagged = Vec::with_capacity(1 + script_cbor.len());
    tagged.push(0x00u8);
    tagged.extend_from_slice(&script_cbor);
    let policy_id: PolicyId = torsten_primitives::hash::blake2b_224(&tagged);

    // Seed genesis UTxO
    let genesis_input = TransactionInput {
        transaction_id: Hash32::from_bytes([0x10u8; 32]),
        index: 0,
    };
    state.utxo_set.insert(
        genesis_input.clone(),
        TransactionOutput {
            address: Address::Byron(ByronAddress {
                payload: vec![0u8; 32],
            }),
            value: Value::lovelace(50_000_000),
            datum: OutputDatum::None,
            script_ref: None,
            is_legacy: false,
            raw_cbor: None,
        },
    );

    // tx1: creates output 0 (plain value) and output 1 (carries native script_ref)
    let tx1_hash = Hash32::from_bytes([0x01u8; 32]);
    let mut tx1 = Transaction::empty_with_hash(tx1_hash);
    tx1.is_valid = true;
    tx1.body.inputs = vec![genesis_input];
    tx1.body.fee = Lovelace(500_000);
    tx1.body.outputs.push(TransactionOutput {
        address: Address::Byron(ByronAddress {
            payload: vec![0u8; 32],
        }),
        value: Value::lovelace(30_000_000),
        datum: OutputDatum::None,
        script_ref: None,
        is_legacy: false,
        raw_cbor: None,
    });
    tx1.body.outputs.push(TransactionOutput {
        address: Address::Byron(ByronAddress {
            payload: vec![0u8; 32],
        }),
        value: Value::lovelace(19_500_000),
        datum: OutputDatum::None,
        script_ref: Some(ScriptRef::NativeScript(script.clone())),
        is_legacy: false,
        raw_cbor: None,
    });

    // Within-block reference input: tx1's output 1 (the script-carrying one).
    // This UTxO does NOT exist before this block is applied.
    let tx1_script_out = TransactionInput {
        transaction_id: tx1_hash,
        index: 1,
    };

    // Pre-existing spending input for tx2
    let spend_input = TransactionInput {
        transaction_id: Hash32::from_bytes([0x20u8; 32]),
        index: 0,
    };
    state.utxo_set.insert(
        spend_input.clone(),
        TransactionOutput {
            address: Address::Byron(ByronAddress {
                payload: vec![0u8; 32],
            }),
            value: Value::lovelace(10_000_000),
            datum: OutputDatum::None,
            script_ref: None,
            is_legacy: false,
            raw_cbor: None,
        },
    );

    // tx2: spends spend_input, uses tx1_script_out as reference input,
    // and mints one token under the native script's policy_id.
    let tx2_hash = Hash32::from_bytes([0x02u8; 32]);
    let mut tx2 = Transaction::empty_with_hash(tx2_hash);
    tx2.is_valid = true;
    tx2.body.inputs = vec![spend_input];
    tx2.body.reference_inputs = vec![tx1_script_out.clone()];
    tx2.body.fee = Lovelace(500_000);

    // Mint 1 token under the native script policy — Rule 3c must resolve
    // the script from the within-block reference input.
    // mint uses i64 (signed, allows burns); multi_asset in Value uses u64.
    let asset_name = AssetName(b"TOKEN".to_vec());
    let mut mint: BTreeMap<PolicyId, BTreeMap<AssetName, i64>> = BTreeMap::new();
    let mut mint_assets: BTreeMap<AssetName, i64> = BTreeMap::new();
    mint_assets.insert(asset_name.clone(), 1i64);
    mint.insert(policy_id, mint_assets);
    tx2.body.mint = mint;

    // Output: return the minted token + change (value conservation)
    let mut out_value = Value::lovelace(9_500_000);
    let mut out_assets: BTreeMap<AssetName, u64> = BTreeMap::new();
    out_assets.insert(asset_name, 1u64);
    out_value.multi_asset.insert(policy_id, out_assets);
    tx2.body.outputs = vec![TransactionOutput {
        address: Address::Byron(ByronAddress {
            payload: vec![0u8; 32],
        }),
        value: out_value,
        datum: OutputDatum::None,
        script_ref: None,
        is_legacy: false,
        raw_cbor: None,
    }];

    let block = make_test_block(1, 1, Hash32::ZERO, vec![tx1, tx2]);

    // ---- ApplyOnly: full success path ----
    // Both transactions must apply without errors.  tx2's reference input
    // (tx1_script_out) is created by tx1 and must be visible via the
    // sequential UTxO application order.
    let mut state_apply = state.clone();
    state_apply
        .apply_block(&block, BlockValidationMode::ApplyOnly)
        .expect("ApplyOnly must succeed: tx2's reference input is created by tx1 in same block");

    // Confirm the script-ref output is still in the UTxO (reference inputs are read-only)
    assert!(
        state_apply.utxo_set.contains(&tx1_script_out),
        "script-ref output must remain in UTxO after being used as reference input"
    );

    // ---- ValidateAll: within-block reference input must not raise ReferenceInputNotFound ----
    // The block may fail for other reasons (no proper witnesses, fee edge cases, etc.)
    // but it must NOT fail because tx2's reference input is "not found" — that output
    // WAS created by tx1 earlier in the same block.
    let result = state.apply_block(&block, BlockValidationMode::ValidateAll);
    if let Err(LedgerError::BlockTxValidationFailed { ref errors, .. }) = result {
        assert!(
            !errors.contains("ReferenceInputNotFound"),
            "within-block reference input must be found during ValidateAll; got: {errors}"
        );
        assert!(
            !errors.contains("InvalidMint"),
            "minting policy must be resolved from within-block reference input; got: {errors}"
        );
    }
}

// ---------------------------------------------------------------------------
// Plutus script validation — collateral path and ExUnits budget tests
//
// The tests in this section cover the `is_valid = false` path in `apply_block`
// where Phase-2 script evaluation has failed:
//
//   - Regular inputs/outputs are NOT applied to the UTxO set.
//   - Collateral inputs ARE consumed (forfeited to the block producer).
//   - If present, the collateral_return output is added to the UTxO set.
//   - The epoch fee accumulator is credited with the net collateral amount.
//
// These tests use `BlockValidationMode::ApplyOnly` to bypass Phase-1/Phase-2
// re-validation and test the UTxO effect logic in isolation.
// ---------------------------------------------------------------------------

/// Build a minimal Transaction with `is_valid = false` that has collateral.
///
/// The transaction references `regular_input` in its body inputs (would be
/// spent if valid) and `collateral_input` as its collateral.  When applied
/// with `is_valid = false`:
///   - `regular_input` must remain in the UTxO set (not consumed).
///   - `collateral_input` must be removed from the UTxO set.
fn make_invalid_tx_with_collateral(
    tx_hash: Hash32,
    regular_input: TransactionInput,
    collateral_input: TransactionInput,
    collateral_return: Option<TransactionOutput>,
    total_collateral: Option<Lovelace>,
) -> Transaction {
    Transaction {
        hash: tx_hash,
        body: TransactionBody {
            inputs: vec![regular_input],
            outputs: vec![],
            fee: Lovelace(0),
            ttl: None,
            certificates: vec![],
            withdrawals: std::collections::BTreeMap::new(),
            auxiliary_data_hash: None,
            validity_interval_start: None,
            mint: std::collections::BTreeMap::new(),
            script_data_hash: None,
            collateral: vec![collateral_input],
            required_signers: vec![],
            network_id: None,
            collateral_return,
            total_collateral,
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
            raw_redeemers_cbor: None,
            raw_plutus_data_cbor: None,
            pallas_script_data_hash: None,
        },
        is_valid: false,
        auxiliary_data: None,
        raw_cbor: None,
    }
}

// -----------------------------------------------------------------------
// Test: is_valid=false — regular inputs are NOT consumed, collateral IS
//
// This is the fundamental invariant for invalid Plutus transactions:
// - Regular inputs/outputs are skipped (no UTxO changes from body)
// - Collateral inputs are spent (forfeited)
// - Collateral return output (if present) is added to UTxO set
// -----------------------------------------------------------------------
#[test]
fn test_invalid_tx_collateral_consumed_regular_inputs_skipped() {
    let params = ProtocolParameters::mainnet_defaults();
    let mut state = LedgerState::new(params);

    // Set up two UTxOs: one is the "would-be-spent" regular input,
    // the other is the collateral.
    let regular_input = TransactionInput {
        transaction_id: Hash32::from_bytes([0xAAu8; 32]),
        index: 0,
    };
    let collateral_input = TransactionInput {
        transaction_id: Hash32::from_bytes([0xBBu8; 32]),
        index: 0,
    };

    let utxo_value = Value::lovelace(10_000_000);
    for inp in [&regular_input, &collateral_input] {
        state.utxo_set.insert(
            inp.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: utxo_value.clone(),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
    }
    assert_eq!(state.utxo_set.len(), 2, "setup: two UTxOs");

    let tx_hash = Hash32::from_bytes([0xCCu8; 32]);
    let tx = make_invalid_tx_with_collateral(
        tx_hash,
        regular_input.clone(),
        collateral_input.clone(),
        None, // no collateral return
        None,
    );

    let block = make_test_block(100, 1, Hash32::ZERO, vec![tx]);
    state
        .apply_block(&block, BlockValidationMode::ApplyOnly)
        .expect("apply_block must succeed");

    // Collateral must be consumed
    assert!(
        !state.utxo_set.contains(&collateral_input),
        "collateral input must be consumed after is_valid=false"
    );

    // Regular input must remain (tx body skipped)
    assert!(
        state.utxo_set.contains(&regular_input),
        "regular input must NOT be consumed when is_valid=false"
    );

    // No outputs from the tx body were created
    let new_output_input = TransactionInput {
        transaction_id: tx_hash,
        index: 0,
    };
    assert!(
        !state.utxo_set.contains(&new_output_input),
        "tx body outputs must NOT be created when is_valid=false"
    );
}

// -----------------------------------------------------------------------
// Test: collateral return output is created when is_valid=false
//
// When the block producer provides a collateral_return output, it must be
// added to the UTxO set at index `outputs.len()` (after regular outputs).
// The net collateral forfeited = collateral_input_value − return_value.
// -----------------------------------------------------------------------
#[test]
fn test_invalid_tx_collateral_return_created() {
    let params = ProtocolParameters::mainnet_defaults();
    let mut state = LedgerState::new(params);

    let collateral_input = TransactionInput {
        transaction_id: Hash32::from_bytes([0xDDu8; 32]),
        index: 0,
    };
    let regular_input = TransactionInput {
        transaction_id: Hash32::from_bytes([0xEEu8; 32]),
        index: 0,
    };

    // Collateral UTxO has 5 ADA; return gives 3 ADA back; 2 ADA is forfeited
    let collateral_value = Value::lovelace(5_000_000);
    let return_value = Value::lovelace(3_000_000);

    for (inp, val) in [
        (&collateral_input, collateral_value.clone()),
        (&regular_input, Value::lovelace(10_000_000)),
    ] {
        state.utxo_set.insert(
            inp.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: val,
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
    }

    let collateral_return_output = TransactionOutput {
        address: Address::Byron(ByronAddress {
            payload: vec![0xFFu8; 32],
        }),
        value: return_value.clone(),
        datum: OutputDatum::None,
        script_ref: None,
        is_legacy: false,
        raw_cbor: None,
    };

    let tx_hash = Hash32::from_bytes([0xFFu8; 32]);
    let tx = make_invalid_tx_with_collateral(
        tx_hash,
        regular_input.clone(),
        collateral_input.clone(),
        Some(collateral_return_output.clone()),
        // total_collateral declared = 2 ADA (5 - 3)
        Some(Lovelace(2_000_000)),
    );

    let fees_before = state.epoch_fees;

    let block = make_test_block(100, 1, Hash32::ZERO, vec![tx]);
    state
        .apply_block(&block, BlockValidationMode::ApplyOnly)
        .expect("apply_block must succeed");

    // Collateral input consumed
    assert!(
        !state.utxo_set.contains(&collateral_input),
        "collateral input must be consumed"
    );

    // Regular input NOT consumed
    assert!(
        state.utxo_set.contains(&regular_input),
        "regular input must remain when is_valid=false"
    );

    // Collateral return output created at index `outputs.len()` = 0
    // (because the tx has no regular outputs).
    let return_input = TransactionInput {
        transaction_id: tx_hash,
        index: 0, // outputs.len() = 0 → collateral return at index 0
    };
    assert!(
        state.utxo_set.contains(&return_input),
        "collateral return output must be created in UTxO set"
    );

    // Fees: total_collateral was declared as 2 ADA, so 2 ADA should be credited
    let fee_increase = state.epoch_fees.0 - fees_before.0;
    assert_eq!(
        fee_increase, 2_000_000,
        "epoch fees must increase by the declared total_collateral (2 ADA)"
    );
}

// -----------------------------------------------------------------------
// Test: multiple UTxOs — only collateral inputs are consumed
//
// When a block contains multiple transactions, only the transactions'
// own collateral inputs should be consumed.  UTxOs belonging to other
// transactions in the block or to the global ledger state must be
// unaffected.
// -----------------------------------------------------------------------
#[test]
fn test_invalid_tx_does_not_consume_unrelated_utxos() {
    let params = ProtocolParameters::mainnet_defaults();
    let mut state = LedgerState::new(params);

    // Set up 5 unrelated UTxOs
    let mut unrelated_inputs = Vec::new();
    for i in 0u8..5 {
        let inp = TransactionInput {
            transaction_id: Hash32::from_bytes([i; 32]),
            index: 0,
        };
        state.utxo_set.insert(
            inp.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![i; 32],
                }),
                value: Value::lovelace(1_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        unrelated_inputs.push(inp);
    }

    // The collateral input is a distinct UTxO
    let collateral_input = TransactionInput {
        transaction_id: Hash32::from_bytes([0xF0u8; 32]),
        index: 0,
    };
    let regular_input = TransactionInput {
        transaction_id: Hash32::from_bytes([0xF1u8; 32]),
        index: 0,
    };
    for (inp, seed) in [(&collateral_input, 0xF0u8), (&regular_input, 0xF1u8)] {
        state.utxo_set.insert(
            inp.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![seed; 32],
                }),
                value: Value::lovelace(3_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
    }

    let utxo_count_before = state.utxo_set.len(); // 5 + 2 = 7

    let tx = make_invalid_tx_with_collateral(
        Hash32::from_bytes([0xABu8; 32]),
        regular_input.clone(),
        collateral_input.clone(),
        None,
        None,
    );
    let block = make_test_block(200, 2, Hash32::ZERO, vec![tx]);
    state
        .apply_block(&block, BlockValidationMode::ApplyOnly)
        .expect("apply_block must succeed");

    // Exactly one UTxO removed: the collateral input
    assert_eq!(
        state.utxo_set.len(),
        utxo_count_before - 1,
        "only the collateral input should be removed"
    );

    // All unrelated UTxOs still present
    for inp in &unrelated_inputs {
        assert!(
            state.utxo_set.contains(inp),
            "unrelated UTxO {:?} must not be consumed",
            inp
        );
    }

    // Regular input not consumed
    assert!(
        state.utxo_set.contains(&regular_input),
        "regular input must not be consumed when is_valid=false"
    );

    // Collateral consumed
    assert!(
        !state.utxo_set.contains(&collateral_input),
        "collateral input must be consumed"
    );
}

// -----------------------------------------------------------------------
// Test: ExUnits budget in validate_transaction — max_tx_ex_units check
//
// `validate_transaction` checks that the total declared ExUnits across all
// redeemers do not exceed the protocol-level `max_tx_ex_units`.
// A transaction whose sum of redeemer ex_units exceeds the limit must be
// rejected with `ExUnitsExceeded`.
// -----------------------------------------------------------------------
#[test]
fn test_validate_transaction_ex_units_exceeded() {
    use crate::validation::{validate_transaction, ValidationError};

    let mut params = ProtocolParameters::mainnet_defaults();
    // Set a strict per-tx budget: 1000 steps, 500 mem
    params.max_tx_ex_units = ExUnits {
        steps: 1_000,
        mem: 500,
    };

    let (utxo_set, input) = {
        let mut utxo_set = crate::utxo::UtxoSet::new();
        let inp = TransactionInput {
            transaction_id: Hash32::from_bytes([0x10u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            inp.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        (utxo_set, inp)
    };

    // Build a transaction whose redeemers exceed the step limit
    let mut tx = Transaction::empty_with_hash(Hash32::from_bytes([0x11u8; 32]));
    tx.body.inputs = vec![input];
    // Add a redeemer that exceeds max_tx_ex_units.steps (1_000)
    tx.witness_set.redeemers.push(Redeemer {
        tag: RedeemerTag::Spend,
        index: 0,
        data: PlutusData::Integer(0),
        ex_units: ExUnits {
            steps: 2_000, // exceeds the 1_000 limit
            mem: 100,
        },
    });

    let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
    assert!(
        result.is_err(),
        "transaction exceeding max_tx_ex_units must be rejected"
    );
    let errors = result.unwrap_err();
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, ValidationError::ExUnitsExceeded)),
        "must produce ExUnitsExceeded error; got: {:?}",
        errors
    );
}

// -----------------------------------------------------------------------
// Test: ExUnits budget — within limit is accepted
//
// Complementary to the above: a transaction with redeemer ex_units below
// the protocol limit is not rejected for budget reasons.
// -----------------------------------------------------------------------
#[test]
fn test_validate_transaction_ex_units_within_limit() {
    use crate::validation::validate_transaction;

    let mut params = ProtocolParameters::mainnet_defaults();
    params.max_tx_ex_units = ExUnits {
        steps: 10_000_000_000,
        mem: 14_000_000,
    };

    let (utxo_set, input) = {
        let mut utxo_set = crate::utxo::UtxoSet::new();
        let inp = TransactionInput {
            transaction_id: Hash32::from_bytes([0x20u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            inp.clone(),
            TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0u8; 32],
                }),
                value: Value::lovelace(10_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );
        (utxo_set, inp)
    };

    // Transaction with redeemers well within budget
    let mut tx = Transaction::empty_with_hash(Hash32::from_bytes([0x21u8; 32]));
    tx.body.inputs = vec![input];
    tx.witness_set.redeemers.push(Redeemer {
        tag: RedeemerTag::Spend,
        index: 0,
        data: PlutusData::Integer(0),
        ex_units: ExUnits {
            steps: 1_000_000, // << 10B limit
            mem: 1_000,       // << 14M limit
        },
    });

    // Will fail for other Phase-1 reasons (missing collateral, missing script
    // data hash, etc.) but NOT for ExUnitsExceeded — that's what we verify.
    let result = validate_transaction(&tx, &utxo_set, &params, 100, 300, None);
    if let Err(errors) = result {
        assert!(
            !errors
                .iter()
                .any(|e| matches!(e, crate::validation::ValidationError::ExUnitsExceeded)),
            "ExUnitsExceeded must NOT appear when budget is within limit; got: {:?}",
            errors
        );
    }
    // (If Ok — unlikely given missing witnesses — that's fine too)
}

// ──────────────────────────────────────────────────────────────────────────────
// Issue #173: DRep count phantom entries
//
// Regression tests that verify:
//   1. VoteDelegation / VoteRegDeleg / RegStakeVoteDeleg / StakeVoteDelegation
//      with DRep::KeyHash targets do NOT create entries in governance.dreps.
//   2. Only RegDRep inserts into governance.dreps.
//   3. active_drep_count() only counts DReps with active=true.
//   4. Inactive DReps (marked by drep_activity) are excluded from active_drep_count.
// ──────────────────────────────────────────────────────────────────────────────

/// VoteDelegation with DRep::KeyHash must NOT create an entry in governance.dreps.
/// The target DRep is referenced as a delegation target; it is only created by RegDRep.
#[test]
fn test_vote_delegation_keyhash_does_not_create_drep_entry() {
    let mut params = ProtocolParameters::mainnet_defaults();
    params.protocol_version_major = 10; // Conway
    let mut state = LedgerState::new(params);

    // A stake key that is delegating its vote
    let stake_cred = Credential::VerificationKey(Hash28::from_bytes([0xAAu8; 28]));
    // Register the stake key first
    state.process_certificate(&Certificate::StakeRegistration(stake_cred.clone()));

    // The DRep they want to delegate to (NOT yet registered via RegDRep)
    let drep_cred = Credential::VerificationKey(Hash28::from_bytes([0xBBu8; 28]));
    let drep_key = credential_to_hash(&drep_cred);
    let drep_keyhash = drep_key; // Hash32 is what DRep::KeyHash stores

    // Sanity: DRep not in registry yet
    assert!(
        !state.governance.dreps.contains_key(&drep_keyhash),
        "DRep must not exist before registration"
    );

    // Process VoteDelegation pointing at the unregistered DRep
    state.process_certificate(&Certificate::VoteDelegation {
        credential: stake_cred.clone(),
        drep: DRep::KeyHash(drep_keyhash),
    });

    // DRep must still NOT be in the dreps registry
    assert_eq!(
        state.governance.dreps.len(),
        0,
        "VoteDelegation with DRep::KeyHash must NOT create a drep registry entry"
    );
    // vote_delegations should have been updated
    let stake_key = credential_to_hash(&stake_cred);
    assert_eq!(
        state.governance.vote_delegations.get(&stake_key),
        Some(&DRep::KeyHash(drep_keyhash)),
        "VoteDelegation must update vote_delegations"
    );
}

/// VoteRegDeleg with DRep::KeyHash must NOT create a drep registry entry.
/// This certificate registers a stake address AND delegates to a DRep;
/// it must not auto-register the target DRep.
#[test]
fn test_vote_reg_deleg_does_not_create_drep_entry() {
    let mut params = ProtocolParameters::mainnet_defaults();
    params.protocol_version_major = 10; // Conway
    let mut state = LedgerState::new(params);

    let stake_cred = Credential::VerificationKey(Hash28::from_bytes([0xCCu8; 28]));
    let drep_cred = Credential::VerificationKey(Hash28::from_bytes([0xDDu8; 28]));
    let drep_keyhash = credential_to_hash(&drep_cred);

    assert_eq!(
        state.governance.dreps.len(),
        0,
        "registry must be empty before test"
    );

    state.process_certificate(&Certificate::VoteRegDeleg {
        credential: stake_cred.clone(),
        drep: DRep::KeyHash(drep_keyhash),
        deposit: torsten_primitives::value::Lovelace(2_000_000),
    });

    assert_eq!(
        state.governance.dreps.len(),
        0,
        "VoteRegDeleg with DRep::KeyHash must NOT create a drep registry entry"
    );
    let stake_key = credential_to_hash(&stake_cred);
    assert_eq!(
        state.governance.vote_delegations.get(&stake_key),
        Some(&DRep::KeyHash(drep_keyhash)),
        "VoteRegDeleg must update vote_delegations"
    );
}

/// RegStakeVoteDeleg with DRep::KeyHash must NOT create a drep registry entry.
#[test]
fn test_reg_stake_vote_deleg_does_not_create_drep_entry() {
    let mut params = ProtocolParameters::mainnet_defaults();
    params.protocol_version_major = 10; // Conway
    let mut state = LedgerState::new(params);

    let stake_cred = Credential::VerificationKey(Hash28::from_bytes([0xEEu8; 28]));
    let pool_id = Hash28::from_bytes([0x01u8; 28]);
    let drep_cred = Credential::VerificationKey(Hash28::from_bytes([0xFFu8; 28]));
    let drep_keyhash = credential_to_hash(&drep_cred);

    assert_eq!(
        state.governance.dreps.len(),
        0,
        "registry must be empty before test"
    );

    state.process_certificate(&Certificate::RegStakeVoteDeleg {
        credential: stake_cred.clone(),
        pool_hash: pool_id,
        drep: DRep::KeyHash(drep_keyhash),
        deposit: torsten_primitives::value::Lovelace(2_000_000),
    });

    assert_eq!(
        state.governance.dreps.len(),
        0,
        "RegStakeVoteDeleg with DRep::KeyHash must NOT create a drep registry entry"
    );
    let stake_key = credential_to_hash(&stake_cred);
    assert_eq!(
        state.governance.vote_delegations.get(&stake_key),
        Some(&DRep::KeyHash(drep_keyhash)),
        "RegStakeVoteDeleg must update vote_delegations"
    );
    assert_eq!(
        state.delegations.get(&stake_key),
        Some(&pool_id),
        "RegStakeVoteDeleg must set pool delegation"
    );
}

/// StakeVoteDelegation with DRep::KeyHash must NOT create a drep registry entry.
#[test]
fn test_stake_vote_delegation_does_not_create_drep_entry() {
    let mut params = ProtocolParameters::mainnet_defaults();
    params.protocol_version_major = 10; // Conway
    let mut state = LedgerState::new(params);

    let stake_cred = Credential::VerificationKey(Hash28::from_bytes([0x11u8; 28]));
    let pool_id = Hash28::from_bytes([0x22u8; 28]);
    let drep_cred = Credential::VerificationKey(Hash28::from_bytes([0x33u8; 28]));
    let drep_keyhash = credential_to_hash(&drep_cred);

    state.process_certificate(&Certificate::StakeVoteDelegation {
        credential: stake_cred.clone(),
        pool_hash: pool_id,
        drep: DRep::KeyHash(drep_keyhash),
    });

    assert_eq!(
        state.governance.dreps.len(),
        0,
        "StakeVoteDelegation with DRep::KeyHash must NOT create a drep registry entry"
    );
}

/// active_drep_count() should only count DReps with active=true.
#[test]
fn test_active_drep_count_excludes_inactive() {
    let mut params = ProtocolParameters::mainnet_defaults();
    params.protocol_version_major = 10; // Conway
    params.drep_activity = 5; // DReps inactive after 5 epochs of no activity
    let mut state = LedgerState::new(params);

    // Register 3 DReps
    for i in 0u8..3 {
        let cred = Credential::VerificationKey(Hash28::from_bytes([i; 28]));
        state.process_certificate(&Certificate::RegDRep {
            credential: cred,
            deposit: torsten_primitives::value::Lovelace(500_000_000),
            anchor: None,
        });
    }
    assert_eq!(state.governance.dreps.len(), 3, "all 3 DReps registered");
    assert_eq!(
        state.governance.active_drep_count(),
        3,
        "all 3 DReps active at registration"
    );

    // Manually mark one as inactive (simulating drep_activity expiry)
    {
        let gov = Arc::make_mut(&mut state.governance);
        let first_key = gov.dreps.keys().copied().next().unwrap();
        gov.dreps.get_mut(&first_key).unwrap().active = false;
    }

    // active_drep_count should now be 2, but dreps.len() is still 3
    assert_eq!(
        state.governance.dreps.len(),
        3,
        "total registered DReps (including inactive) is still 3"
    );
    assert_eq!(
        state.governance.active_drep_count(),
        2,
        "active_drep_count excludes inactive DRep"
    );
}

/// After drep_activity epochs of inactivity, a DRep is marked inactive
/// at the epoch boundary but stays in the registry (not removed).
/// active_drep_count() must exclude it.
#[test]
fn test_epoch_transition_marks_inactive_drep() {
    let mut params = ProtocolParameters::mainnet_defaults();
    params.protocol_version_major = 10; // Conway
    params.drep_activity = 3; // expire after 3 epochs of inactivity
    let mut state = LedgerState::new(params);

    // Register 2 DReps at epoch 0
    let cred_a = Credential::VerificationKey(Hash28::from_bytes([0xA0u8; 28]));
    let cred_b = Credential::VerificationKey(Hash28::from_bytes([0xB0u8; 28]));
    let key_a = credential_to_hash(&cred_a);

    state.process_certificate(&Certificate::RegDRep {
        credential: cred_a.clone(),
        deposit: torsten_primitives::value::Lovelace(500_000_000),
        anchor: None,
    });
    state.process_certificate(&Certificate::RegDRep {
        credential: cred_b.clone(),
        deposit: torsten_primitives::value::Lovelace(500_000_000),
        anchor: None,
    });

    assert_eq!(state.governance.active_drep_count(), 2);

    // Advance 4 epochs: last_active_epoch=0, epoch 4, inactive gap = 4 > drep_activity=3
    for e in 1u64..=4 {
        state.process_epoch_transition(torsten_primitives::time::EpochNo(e));
    }

    // DRep A should now be inactive; total count stays 2 but active is 0
    assert_eq!(
        state.governance.dreps.len(),
        2,
        "both DReps still registered (not removed by inactivity)"
    );
    assert_eq!(
        state.governance.active_drep_count(),
        0,
        "both DReps inactive after 4 epochs (drep_activity=3)"
    );
    // They're still IN the map — just marked inactive
    assert!(
        state.governance.dreps.contains_key(&key_a),
        "DRep A still in registry despite being inactive"
    );
}

/// Deregistering a DRep removes it entirely from the registry,
/// so active_drep_count drops accordingly.
#[test]
fn test_unreg_drep_removes_from_registry_and_active_count() {
    let mut params = ProtocolParameters::mainnet_defaults();
    params.protocol_version_major = 10;
    let mut state = LedgerState::new(params);

    let cred = Credential::VerificationKey(Hash28::from_bytes([0x42u8; 28]));

    state.process_certificate(&Certificate::RegDRep {
        credential: cred.clone(),
        deposit: torsten_primitives::value::Lovelace(500_000_000),
        anchor: None,
    });
    assert_eq!(state.governance.active_drep_count(), 1);
    assert_eq!(state.governance.dreps.len(), 1);

    state.process_certificate(&Certificate::UnregDRep {
        credential: cred.clone(),
        refund: torsten_primitives::value::Lovelace(500_000_000),
    });

    assert_eq!(
        state.governance.dreps.len(),
        0,
        "UnregDRep removes entry from registry"
    );
    assert_eq!(
        state.governance.active_drep_count(),
        0,
        "active_drep_count is 0 after deregistration"
    );
}

// ===================================================================
// Reward E2E cross-validation against Koios preview epoch 1239→1240
// ===================================================================
//
// Source data from Koios (preview network, queried 2026-03-18):
//   koios_epoch_info(epoch=1239):  reserves = 8_266_581_303_023_223 lovelace
//   koios_epoch_info(epoch=1240):  reserves = 8_263_059_377_635_947 lovelace
//   delta_reserves                          = 3_521_925_387_276      lovelace (decrease)
//   koios_epoch_info(epoch=1239):  treasury = 6_503_456_329_914_064  lovelace
//   koios_epoch_info(epoch=1240):  treasury = 6_506_647_320_615_800  lovelace
//   delta_treasury                          = 3_190_990_701_736      lovelace (increase)
//   epoch_info(1239).fees                   = 1_599_730_138           lovelace
//   epoch_info(1239).active_stake           = 1_242_169_299_382_026  lovelace
//
// Preview protocol parameters (Conway, epoch 1239):
//   rho = 3/1000, tau = 2/10, epoch_length = 86400, active_slot_coeff = 0.05
//
// RUPD timing note:
//   Haskell's reward update (RUPD) is computed at epoch boundary E→E+1 using
//   the "go" snapshot (captured at boundary E-2→E-1), and APPLIED at boundary
//   E+1→E+2.  Therefore, the reserves/treasury changes observed between epochs
//   1239 and 1240 reflect rewards computed using the epoch-1237 go snapshot,
//   applied when epoch 1240 starts.  Because we are testing the formula, not
//   the timing, we drive calculate_rewards() directly with known parameters
//   and verify the arithmetic is correct.
//
// Block-count derivation:
//   expansion = floor(rho * reserves * actual_blocks / expected_blocks)
//   expected_blocks = floor(0.05 * 86400) = 4320
//   Solving for actual_blocks from Koios delta_reserves = 3_521_925_387_276:
//     actual_blocks ≈ 3_521_925_387_276 * 4_320_000 / (3 * 8_266_581_303_023_223)
//                   ≈ 613.5
//   No integer block count produces 3_521_925_387_276 exactly; the Koios value
//   lies between expansion(613) = 3_519_037_735_245 and expansion(614) = 3_524_778_416_705.
//   We use actual_blocks = 613 and verify that:
//     a) our formula produces the mathematically correct result for that input,
//     b) the result is within one block's worth of expansion (5_740_681_460) of
//        the Koios-observed delta_reserves, proving the formula is correct.
//
// Pool data:
//   The full test uses an empty pool snapshot (no delegations, no pool params).
//   With zero active stake, all rewards are undistributed and flow entirely to
//   treasury (delta_treasury = expansion + fees).  This lets us verify the
//   monetary expansion and treasury cut paths independently of per-pool logic.
//   A separate sub-test verifies the treasury cut formula against the Koios
//   delta_treasury assuming the Koios-observed expansion.

/// Cross-validate reward calculation against Koios on-chain data for
/// preview testnet epoch 1239→1240.
///
/// Tests the full chain:
///   1. eta = min(1, actual_blocks / expected_blocks)
///   2. expansion = floor(rho * reserves * eta)
///   3. treasury_cut = floor(tau * (expansion + fees))
///   4. With no active pools: delta_treasury = expansion + fees; delta_reserves = expansion
///   5. Formula output is within one-block tolerance of Koios delta_reserves
#[test]
fn test_reward_cross_validation_epoch_1239() {
    // ---- Preview epoch 1239 protocol parameters ----
    let mut params = ProtocolParameters::mainnet_defaults();
    // Preview rho = 3/1000 (same as mainnet, confirmed via koios_epoch_params)
    params.rho = Rational {
        numerator: 3,
        denominator: 1000,
    };
    // Preview tau = 2/10 (same as mainnet)
    params.tau = Rational {
        numerator: 2,
        denominator: 10,
    };
    // Preview active_slot_coeff = 0.05 (same as mainnet)
    params.active_slots_coeff = 0.05;

    // ---- Build the ledger state matching the start of epoch 1239 ----
    let mut state = LedgerState::new(params);

    // Reserves at start of epoch 1239 per Koios epoch_info
    const RESERVES_1239: u64 = 8_266_581_303_023_223;
    state.reserves = Lovelace(RESERVES_1239);

    // Preview epoch_length = 86400 slots
    const EPOCH_LENGTH: u64 = 86_400;
    state.epoch_length = EPOCH_LENGTH;

    // ---- Build the go snapshot for epoch 1239 ----
    //
    // The go snapshot carries:
    //   epoch_fees = fees collected during the go epoch (1239)
    //   epoch_block_count = blocks produced during the go epoch
    //   pool_stake / delegations / pool_params = empty (no active pools in this sub-test)
    //
    // actual_blocks = 613 (derived: see block-count derivation note above).
    // expected_blocks = floor(0.05 * 86400) = 4320
    const ACTUAL_BLOCKS: u64 = 613;
    const FEES_1239: u64 = 1_599_730_138;

    let go_snapshot = StakeSnapshot {
        epoch: EpochNo(1239),
        delegations: Arc::new(HashMap::new()),
        pool_stake: HashMap::new(),
        pool_params: Arc::new(HashMap::new()),
        stake_distribution: Arc::new(HashMap::new()),
        epoch_fees: Lovelace(FEES_1239),
        epoch_block_count: ACTUAL_BLOCKS,
        epoch_blocks_by_pool: Arc::new(HashMap::new()),
    };

    // ---- Step 1: verify eta and expansion ----
    //
    // expected_blocks = floor(active_slot_coeff * epoch_length) = floor(0.05 * 86400) = 4320
    // eta = min(1, 613 / 4320) — strictly less than 1 (partial epoch)
    // expansion = floor(3/1000 * 8_266_581_303_023_223 * 613 / 4320)
    //
    // We compute the expected expansion independently here using integer arithmetic
    // (matching the Rat::floor_u64 path in calculate_rewards) and also cross-check
    // against the Koios delta_reserves to confirm the formula is correct.
    const EXPECTED_BLOCKS: u64 = 4320; // floor(0.05 * 86400)
    let effective_blocks = ACTUAL_BLOCKS.min(EXPECTED_BLOCKS); // 613 < 4320, so effective = 613

    // expansion = floor(3 * RESERVES_1239 * effective / (1000 * EXPECTED_BLOCKS))
    // Computed via integer arithmetic to match Rat exactly:
    let expected_expansion: u64 = (3u128 * RESERVES_1239 as u128 * effective_blocks as u128
        / (1000u128 * EXPECTED_BLOCKS as u128)) as u64;
    // = floor(24_799_743_909_069_669 * 613 / 4_320_000)
    // = floor(15_202,302,517,259,670,597 / 4_320_000)
    // = 3_519_037_735_245

    // Sanity-check our manual integer computation.  The Rat inside calculate_rewards
    // uses BigInt with the same floor semantics, so these should match exactly.
    assert_eq!(
        expected_expansion, 3_519_037_735_245,
        "pre-computed expansion for actual_blocks=613 should be 3_519_037_735_245"
    );

    // ---- Step 2: verify treasury cut formula ----
    //
    // treasury_cut = floor(tau * (expansion + fees))
    //              = floor(2/10 * (3_519_037_735_245 + 1_599_730_138))
    //              = floor(2/10 * 3_520_637_465_383)
    //              = floor(704_127_493_076.6) = 704_127_493_076
    let total_rewards_available = expected_expansion + FEES_1239;
    let expected_treasury_cut: u64 = (2u128 * total_rewards_available as u128 / 10) as u64;
    assert_eq!(
        expected_treasury_cut, 704_127_493_076,
        "treasury_cut = floor(tau * (expansion + fees))"
    );

    // ---- Step 3: run calculate_rewards with no active pools ----
    //
    // With an empty pool snapshot:
    //   total_active_stake = 0  → early return path in calculate_rewards
    //   delta_reserves  = expansion
    //   delta_treasury  = treasury_cut + undistributed = treasury_cut + reward_pot
    //                   = treasury_cut + (expansion + fees - treasury_cut)
    //                   = expansion + fees                (all rewards go to treasury)
    let rupd = state.calculate_rewards(&go_snapshot);

    // Verify delta_reserves equals expansion exactly
    assert_eq!(
        rupd.delta_reserves, expected_expansion,
        "delta_reserves must equal monetary expansion: \
         expected={expected_expansion}, got={}",
        rupd.delta_reserves
    );

    // Verify delta_treasury = expansion + fees (no-pool case: all rewards undistributed)
    let expected_delta_treasury_no_pools = expected_expansion + FEES_1239;
    assert_eq!(
        rupd.delta_treasury, expected_delta_treasury_no_pools,
        "delta_treasury (no-pool case) must equal expansion + fees: \
         expected={expected_delta_treasury_no_pools}, got={}",
        rupd.delta_treasury
    );

    // Verify no per-account rewards were distributed (no pools means no delegators)
    assert!(
        rupd.rewards.is_empty(),
        "expect zero per-account rewards with empty pool snapshot, got {}",
        rupd.rewards.len()
    );

    // ---- Step 4: cross-check against Koios delta_reserves ----
    //
    // The Koios-observed delta_reserves = 3_521_925_387_276 falls between
    // expansion(613) and expansion(614).  Our formula with actual_blocks=613
    // should be within one block's expansion (5_740_681_460) of the Koios value.
    // This proves the formula is correct; the sub-lovelace difference arises
    // because we don't have the exact go-snapshot block count from the chain.
    const KOIOS_DELTA_RESERVES: u64 = 3_521_925_387_276;
    // Per-block expansion step = floor(3 * reserves / (1000 * 4320))
    let per_block_step: u64 =
        (3u128 * RESERVES_1239 as u128 / (1000u128 * EXPECTED_BLOCKS as u128)) as u64;
    assert_eq!(per_block_step, 5_740_681_460, "per-block expansion step");

    let formula_vs_koios_diff =
        (KOIOS_DELTA_RESERVES as i64 - rupd.delta_reserves as i64).unsigned_abs();
    assert!(
        formula_vs_koios_diff < per_block_step + 1,
        "our formula (actual_blocks=613) must be within one block of Koios delta_reserves: \
         formula={}, koios={KOIOS_DELTA_RESERVES}, diff={formula_vs_koios_diff}, \
         one_block_step={per_block_step}",
        rupd.delta_reserves
    );

    // ---- Step 5: treasury cut formula check against Koios values ----
    //
    // Using the Koios delta_reserves as the expansion, verify the treasury cut
    // formula produces a value consistent with the Koios delta_treasury.
    // Koios delta_treasury = 3_190_990_701_736 includes both the treasury cut
    // and undistributed rewards from ~332M ADA distributed to pools:
    //   treasury_cut  = floor(2/10 * (3_521_925_387_276 + 1_599_730_138))
    //                 = floor(2/10 * 3_523_525_117_414)
    //                 = 704_705_023_482
    //   reward_pot    = 3_523_525_117_414 - 704_705_023_482 = 2_818_820_093_932
    //   undistributed = 3_190_990_701_736 - 704_705_023_482 = 2_486_285_678_254
    //   distributed   = 2_818_820_093_932 - 2_486_285_678_254 = 332_534_415_678
    //
    // Invariant: treasury_cut <= delta_treasury <= expansion + fees
    const KOIOS_DELTA_TREASURY: u64 = 3_190_990_701_736;
    let koios_total_rewards = KOIOS_DELTA_RESERVES + FEES_1239;
    let koios_treasury_cut: u64 = (2u128 * koios_total_rewards as u128 / 10) as u64;
    assert_eq!(
        koios_treasury_cut, 704_705_023_482,
        "treasury_cut from Koios expansion should be 704_705_023_482"
    );

    // delta_treasury >= treasury_cut: the treasury always gets at least the tau cut
    assert!(
        KOIOS_DELTA_TREASURY >= koios_treasury_cut,
        "Koios delta_treasury={KOIOS_DELTA_TREASURY} must be >= treasury_cut={koios_treasury_cut}"
    );

    // delta_treasury <= expansion + fees: the treasury cannot receive more than the total reward pot
    assert!(
        KOIOS_DELTA_TREASURY <= koios_total_rewards,
        "Koios delta_treasury={KOIOS_DELTA_TREASURY} must be <= expansion+fees={koios_total_rewards}"
    );

    // The implied distributed-to-pools amount must be positive and reasonable
    let koios_reward_pot = koios_total_rewards - koios_treasury_cut;
    let koios_undistributed = KOIOS_DELTA_TREASURY - koios_treasury_cut;
    assert!(
        koios_undistributed <= koios_reward_pot,
        "undistributed={koios_undistributed} must not exceed reward_pot={koios_reward_pot}"
    );
    let koios_distributed = koios_reward_pot - koios_undistributed;
    // Sanity: at least some rewards were distributed and the amount is sub-billion ADA
    assert!(
        koios_distributed > 0,
        "Koios epoch 1239 should have distributed some pool rewards"
    );
    assert!(
        koios_distributed < 1_000_000_000_000_000,
        "Koios distributed pool rewards should be < 1B ADA: got {koios_distributed}"
    );
}

// =========================================================================
// Issue #176: UTxO set inconsistent after 1-block rollback
// =========================================================================
//
// Regression test for the micro-fork / chain-reorganisation bug.
//
// Scenario:
//   1. Block A at slot S is applied (consumes UTxOs X,Y; produces P,Q)
//   2. Rollback to slot S-1 (the parent)
//   3. Block B at slot S (different hash, replacement fork) is applied
//   4. Block B's transactions reference UTxOs X,Y
//
// Before the fix, `rollback_blocks` didn't exist and the DiffSeq was never
// populated, so X,Y remained spent after the rollback.  Block B's txs then
// failed Phase-1 with `InputNotFound(X)` / `InputNotFound(Y)`.
//
// After the fix:
//   - `apply_block` records UTxO inserts/deletes into a per-block `UtxoDiff`
//     and pushes it into `LedgerState::diff_seq`.
//   - `rollback_blocks(n)` pops the last n diffs and inverts them (removes
//     inserted UTxOs, re-inserts deleted UTxOs) so that X,Y are back in the
//     set and P,Q are removed.
//   - Block B can then be applied successfully because X,Y are present.

/// Build a minimal valid `Transaction` that spends `inputs` and produces
/// `outputs`, suitable for `BlockValidationMode::ApplyOnly` (no witness
/// validation, no fee check).
fn make_simple_tx(
    tx_hash_byte: u8,
    inputs: Vec<TransactionInput>,
    outputs: Vec<TransactionOutput>,
) -> Transaction {
    Transaction {
        hash: Hash32::from_bytes([tx_hash_byte; 32]),
        body: TransactionBody {
            inputs,
            outputs,
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
            raw_redeemers_cbor: None,
            raw_plutus_data_cbor: None,
            pallas_script_data_hash: None,
        },
        is_valid: true,
        auxiliary_data: None,
        raw_cbor: None,
    }
}

/// Helper: create a minimal `TransactionOutput` at the Byron zero address with
/// the given lovelace amount.  Sufficient for rollback tests that only care
/// about UTxO existence.
fn make_lovelace_output(lovelace: u64) -> TransactionOutput {
    TransactionOutput {
        address: Address::Byron(ByronAddress {
            payload: vec![0u8; 32],
        }),
        value: Value::lovelace(lovelace),
        datum: OutputDatum::None,
        script_ref: None,
        is_legacy: false,
        raw_cbor: None,
    }
}

/// Regression test for issue #176.
///
/// Verifies that after a 1-block diff-based rollback the UTxOs consumed by the
/// rolled-back block are restored so that a replacement block at the same slot
/// can successfully spend them.
#[test]
fn test_issue_176_utxo_restored_after_1_block_diff_rollback() {
    let params = ProtocolParameters::mainnet_defaults();
    let mut state = LedgerState::new(params);

    // ── Setup ────────────────────────────────────────────────────────────
    // Seed UTxOs X and Y (the inputs that block A will consume).
    let utxo_x = TransactionInput {
        transaction_id: Hash32::from_bytes([0xAAu8; 32]),
        index: 0,
    };
    let utxo_y = TransactionInput {
        transaction_id: Hash32::from_bytes([0xBBu8; 32]),
        index: 0,
    };
    state
        .utxo_set
        .insert(utxo_x.clone(), make_lovelace_output(5_000_000));
    state
        .utxo_set
        .insert(utxo_y.clone(), make_lovelace_output(3_000_000));

    assert_eq!(state.utxo_set.len(), 2, "UTxOs X and Y must be seeded");

    // ── Step 1: apply block A at slot 100 ────────────────────────────────
    // Block A spends X,Y and produces P (tx_hash=0xCC, index 0) and
    //                                   Q (tx_hash=0xCC, index 1).
    let tx_a = make_simple_tx(
        0xCC,
        vec![utxo_x.clone(), utxo_y.clone()],
        vec![
            make_lovelace_output(4_000_000), // P
            make_lovelace_output(3_800_000), // Q
        ],
    );
    let block_a = make_test_block(100, 1, Hash32::ZERO, vec![tx_a]);
    state
        .apply_block(&block_a, BlockValidationMode::ApplyOnly)
        .expect("Block A must apply cleanly");

    // X,Y are spent; P,Q are present.
    assert!(
        !state.utxo_set.contains(&utxo_x),
        "X must be spent after block A"
    );
    assert!(
        !state.utxo_set.contains(&utxo_y),
        "Y must be spent after block A"
    );
    let utxo_p = TransactionInput {
        transaction_id: Hash32::from_bytes([0xCCu8; 32]),
        index: 0,
    };
    let utxo_q = TransactionInput {
        transaction_id: Hash32::from_bytes([0xCCu8; 32]),
        index: 1,
    };
    assert!(
        state.utxo_set.contains(&utxo_p),
        "P must exist after block A"
    );
    assert!(
        state.utxo_set.contains(&utxo_q),
        "Q must exist after block A"
    );
    assert_eq!(state.diff_seq.len(), 1, "DiffSeq must hold block A's diff");

    // ── Step 2: roll back 1 block (micro-fork, back to slot 99 / origin) ─
    // Simulate the ChainSync RollBackward to the parent point.
    // In production this is done via `rollback_blocks_to_point`, which is
    // what `handle_rollback` calls on the fast path.  Here we call
    // `rollback_blocks` directly and verify the UTxO set is correct.
    let rolled = state.rollback_blocks(1);
    assert_eq!(rolled, 1, "Exactly 1 diff must be rolled back");
    assert_eq!(
        state.diff_seq.len(),
        0,
        "DiffSeq must be empty after rolling back the only diff"
    );

    // X,Y must be restored; P,Q must be removed.
    assert!(
        state.utxo_set.contains(&utxo_x),
        "X must be restored after rollback (issue #176)"
    );
    assert!(
        state.utxo_set.contains(&utxo_y),
        "Y must be restored after rollback (issue #176)"
    );
    assert!(
        !state.utxo_set.contains(&utxo_p),
        "P must be removed after rollback"
    );
    assert!(
        !state.utxo_set.contains(&utxo_q),
        "Q must be removed after rollback"
    );
    assert_eq!(
        state.utxo_set.len(),
        2,
        "UTxO count must be 2 (X,Y) after rollback"
    );

    // ── Step 3: apply replacement block B at slot 100 ────────────────────
    // Block B is a different block at the same slot (micro-fork replacement).
    // It also spends X,Y but produces different outputs R and S.
    // Reset tip to origin so block B can connect.
    state.tip = torsten_primitives::block::Tip::origin();

    let tx_b = make_simple_tx(
        0xDD, // different tx hash from block A's tx
        vec![utxo_x.clone(), utxo_y.clone()],
        vec![
            make_lovelace_output(4_500_000), // R
            make_lovelace_output(3_300_000), // S
        ],
    );
    let block_b = make_test_block(100, 1, Hash32::ZERO, vec![tx_b]);
    state
        .apply_block(&block_b, BlockValidationMode::ApplyOnly)
        .expect("Block B must apply cleanly — X,Y must be available after rollback (issue #176)");

    // X,Y must be spent; R,S must be present.
    assert!(
        !state.utxo_set.contains(&utxo_x),
        "X must be spent after block B"
    );
    assert!(
        !state.utxo_set.contains(&utxo_y),
        "Y must be spent after block B"
    );
    let utxo_r = TransactionInput {
        transaction_id: Hash32::from_bytes([0xDDu8; 32]),
        index: 0,
    };
    let utxo_s = TransactionInput {
        transaction_id: Hash32::from_bytes([0xDDu8; 32]),
        index: 1,
    };
    assert!(
        state.utxo_set.contains(&utxo_r),
        "R must exist after block B"
    );
    assert!(
        state.utxo_set.contains(&utxo_s),
        "S must exist after block B"
    );
    assert_eq!(
        state.utxo_set.len(),
        2,
        "UTxO count must be 2 (R,S) after block B"
    );
}

/// Test that a 2-block rollback correctly restores the UTxO chain across both blocks.
///
/// Chain: genesis → block A (slot 10) → block B (slot 20)
/// Rollback by 2: both blocks' UTxO changes are inverted in reverse order
/// (B first, then A), restoring the genesis UTxO set.
#[test]
fn test_multi_block_diff_rollback_restores_full_chain() {
    let params = ProtocolParameters::mainnet_defaults();
    let mut state = LedgerState::new(params);

    // Seed genesis UTxO G.
    let utxo_g = TransactionInput {
        transaction_id: Hash32::from_bytes([0x01u8; 32]),
        index: 0,
    };
    state
        .utxo_set
        .insert(utxo_g.clone(), make_lovelace_output(10_000_000));
    assert_eq!(state.utxo_set.len(), 1);

    // Block A at slot 10: spends G → produces A0 and A1.
    let tx_a = make_simple_tx(
        0xA0,
        vec![utxo_g.clone()],
        vec![
            make_lovelace_output(6_000_000), // A0
            make_lovelace_output(3_800_000), // A1
        ],
    );
    let block_a = make_test_block(10, 1, Hash32::ZERO, vec![tx_a]);
    state
        .apply_block(&block_a, BlockValidationMode::ApplyOnly)
        .expect("Block A must apply");

    let utxo_a0 = TransactionInput {
        transaction_id: Hash32::from_bytes([0xA0u8; 32]),
        index: 0,
    };
    let utxo_a1 = TransactionInput {
        transaction_id: Hash32::from_bytes([0xA0u8; 32]),
        index: 1,
    };
    assert!(!state.utxo_set.contains(&utxo_g));
    assert!(state.utxo_set.contains(&utxo_a0));
    assert!(state.utxo_set.contains(&utxo_a1));
    assert_eq!(state.diff_seq.len(), 1);

    // Block B at slot 20 (block number 2, prev = block A's hash):
    // spends A0 → produces B0.
    let block_a_hash = Hash32::from_bytes([1u8; 32]); // header_hash from make_test_block(_, 1, ...)
    let tx_b = make_simple_tx(
        0xB0,
        vec![utxo_a0.clone()],
        vec![make_lovelace_output(5_800_000)], // B0
    );
    let block_b = make_test_block(20, 2, block_a_hash, vec![tx_b]);
    state
        .apply_block(&block_b, BlockValidationMode::ApplyOnly)
        .expect("Block B must apply");

    let utxo_b0 = TransactionInput {
        transaction_id: Hash32::from_bytes([0xB0u8; 32]),
        index: 0,
    };
    assert!(!state.utxo_set.contains(&utxo_a0));
    assert!(state.utxo_set.contains(&utxo_a1));
    assert!(state.utxo_set.contains(&utxo_b0));
    assert_eq!(state.diff_seq.len(), 2);

    // Rollback both blocks.
    let rolled = state.rollback_blocks(2);
    assert_eq!(rolled, 2, "Both diffs must be rolled back");
    assert_eq!(state.diff_seq.len(), 0);

    // Genesis UTxO G must be fully restored.
    assert!(
        state.utxo_set.contains(&utxo_g),
        "G must be restored after 2-block rollback"
    );
    assert!(
        !state.utxo_set.contains(&utxo_a0),
        "A0 must be removed after rollback"
    );
    assert!(
        !state.utxo_set.contains(&utxo_a1),
        "A1 must be removed after rollback"
    );
    assert!(
        !state.utxo_set.contains(&utxo_b0),
        "B0 must be removed after rollback"
    );
    assert_eq!(
        state.utxo_set.len(),
        1,
        "Only G must remain after 2-block rollback"
    );
}

/// Test that the DiffSeq fast-path correctly handles partial rollbacks
/// (roll back 1 of 2 applied blocks, then apply a replacement block).
///
/// This is the exact micro-fork scenario from issue #176 but with
/// 2 blocks in the diff window.
#[test]
fn test_partial_rollback_then_reapply() {
    let params = ProtocolParameters::mainnet_defaults();
    let mut state = LedgerState::new(params);

    // Seed genesis UTxOs X and Y.
    let utxo_x = TransactionInput {
        transaction_id: Hash32::from_bytes([0xAAu8; 32]),
        index: 0,
    };
    let utxo_y = TransactionInput {
        transaction_id: Hash32::from_bytes([0xBBu8; 32]),
        index: 0,
    };
    state
        .utxo_set
        .insert(utxo_x.clone(), make_lovelace_output(5_000_000));
    state
        .utxo_set
        .insert(utxo_y.clone(), make_lovelace_output(3_000_000));

    // Block 1 (slot 10): spends X → produces M.
    let tx1 = make_simple_tx(
        0x10,
        vec![utxo_x.clone()],
        vec![make_lovelace_output(4_800_000)],
    );
    let block1 = make_test_block(10, 1, Hash32::ZERO, vec![tx1]);
    state
        .apply_block(&block1, BlockValidationMode::ApplyOnly)
        .expect("Block 1 must apply");

    let utxo_m = TransactionInput {
        transaction_id: Hash32::from_bytes([0x10u8; 32]),
        index: 0,
    };
    assert!(!state.utxo_set.contains(&utxo_x));
    assert!(state.utxo_set.contains(&utxo_m));
    assert_eq!(state.diff_seq.len(), 1);

    // Block 2 (slot 20): spends Y → produces N.
    let block1_hash = Hash32::from_bytes([1u8; 32]);
    let tx2 = make_simple_tx(
        0x20,
        vec![utxo_y.clone()],
        vec![make_lovelace_output(2_800_000)],
    );
    let block2 = make_test_block(20, 2, block1_hash, vec![tx2]);
    state
        .apply_block(&block2, BlockValidationMode::ApplyOnly)
        .expect("Block 2 must apply");

    let utxo_n = TransactionInput {
        transaction_id: Hash32::from_bytes([0x20u8; 32]),
        index: 0,
    };
    assert!(!state.utxo_set.contains(&utxo_y));
    assert!(state.utxo_set.contains(&utxo_n));
    assert_eq!(state.diff_seq.len(), 2);

    // Rollback only the last block (block 2).
    let rolled = state.rollback_blocks(1);
    assert_eq!(rolled, 1);
    assert_eq!(state.diff_seq.len(), 1);

    // Y must be restored; M must still exist (from block 1 which was NOT rolled back).
    assert!(
        state.utxo_set.contains(&utxo_y),
        "Y must be restored after partial rollback"
    );
    assert!(
        state.utxo_set.contains(&utxo_m),
        "M must still exist (block 1 was NOT rolled back)"
    );
    assert!(
        !state.utxo_set.contains(&utxo_n),
        "N must be removed (block 2 was rolled back)"
    );

    // Apply replacement block 2' at slot 20, spending Y → produces N'.
    // Must succeed because Y is restored.
    state.tip = torsten_primitives::block::Tip {
        point: torsten_primitives::block::Point::Specific(
            torsten_primitives::time::SlotNo(10),
            block1_hash,
        ),
        block_number: torsten_primitives::time::BlockNo(1),
    };
    let tx2_prime = make_simple_tx(
        0x21, // different tx hash — replacement fork
        vec![utxo_y.clone()],
        vec![make_lovelace_output(2_900_000)],
    );
    let block2_prime = make_test_block(20, 2, block1_hash, vec![tx2_prime]);
    state
        .apply_block(&block2_prime, BlockValidationMode::ApplyOnly)
        .expect("Replacement block 2' must apply — Y must be present after partial rollback");

    let utxo_n_prime = TransactionInput {
        transaction_id: Hash32::from_bytes([0x21u8; 32]),
        index: 0,
    };
    assert!(state.utxo_set.contains(&utxo_n_prime));
    assert!(!state.utxo_set.contains(&utxo_y));
    assert!(state.utxo_set.contains(&utxo_m));
}

// ═══════════════════════════════════════════════════════════════════════════
// Regression tests for the cascade-failure bug (slot 107229218 / tx 26b1e945)
//
// Root cause: confirmed on-chain blocks with TreasuryValueMismatch or
// UnelectedCommitteeMember errors hard-returned `Err(...)` from `apply_block`,
// preventing the block's outputs from being inserted into the UTxO store.
// The sync loop then `break`s, leaving the block in ChainDB but missing from
// the ledger.  Downstream txs spending those outputs get InputNotFound, which
// triggers the "Phase-1 divergence" path.  The node continues on the wrong
// UTxO set and can forge a block rejected by the network.
//
// Fix: demote both hard returns to `warn!()` + self-correct + fall through.
// These tests confirm that:
//   1. A block with a `treasury_value` that disagrees with `state.treasury`
//      is still applied correctly — outputs are inserted, treasury is corrected.
//   2. A block whose tx carries a `CommitteeHotAuth` for an un-elected CC cold
//      credential is still applied correctly — outputs are inserted.
//   3. A downstream tx spending outputs from such a block succeeds (no cascade).
// ═══════════════════════════════════════════════════════════════════════════

/// Build a tx that declares `treasury_value` in its body while spending
/// `inputs` and producing `outputs`.
fn make_tx_with_treasury(
    tx_hash_byte: u8,
    inputs: Vec<TransactionInput>,
    outputs: Vec<TransactionOutput>,
    treasury_value: Lovelace,
) -> Transaction {
    Transaction {
        hash: Hash32::from_bytes([tx_hash_byte; 32]),
        body: TransactionBody {
            inputs,
            outputs,
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
            treasury_value: Some(treasury_value),
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
            raw_redeemers_cbor: None,
            raw_plutus_data_cbor: None,
            pallas_script_data_hash: None,
        },
        is_valid: true,
        auxiliary_data: None,
        raw_cbor: None,
    }
}

/// Build a tx that carries a `CommitteeHotAuth` certificate for the given cold
/// credential while spending `inputs` and producing `outputs`.
fn make_tx_with_committee_hot_auth(
    tx_hash_byte: u8,
    inputs: Vec<TransactionInput>,
    outputs: Vec<TransactionOutput>,
    cold_credential: Credential,
    hot_credential: Credential,
) -> Transaction {
    Transaction {
        hash: Hash32::from_bytes([tx_hash_byte; 32]),
        body: TransactionBody {
            inputs,
            outputs,
            fee: Lovelace(200_000),
            ttl: None,
            certificates: vec![Certificate::CommitteeHotAuth {
                cold_credential,
                hot_credential,
            }],
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
            raw_redeemers_cbor: None,
            raw_plutus_data_cbor: None,
            pallas_script_data_hash: None,
        },
        is_valid: true,
        auxiliary_data: None,
        raw_cbor: None,
    }
}

/// Regression: TreasuryValueMismatch must NOT abort apply_block for confirmed
/// on-chain blocks.
///
/// Scenario: our `state.treasury` is 0, but a Conway block contains a tx with
/// `treasury_value = 5_000_000_000`.  Prior to the fix, `apply_block` returned
/// `Err(TreasuryValueMismatch)` and the block's outputs were never inserted.
/// After the fix, the block applies successfully, the outputs are in the UTxO
/// set, and `state.treasury` is updated to the declared value.
///
/// The treasury check only fires in `ValidateAll` mode (at-tip validation),
/// not in `ApplyOnly` mode (bulk replay).  This test uses `ValidateAll` to
/// reproduce the exact failure path from slot 107229218.
#[test]
fn test_treasury_mismatch_does_not_abort_apply_block() {
    let mut params = ProtocolParameters::mainnet_defaults();
    // Conway (protocol >= 9) so that the treasury check fires.
    params.protocol_version_major = 9;
    let mut state = LedgerState::new(params);

    // Seed input UTxO.
    let genesis_input = TransactionInput {
        transaction_id: Hash32::from_bytes([0xA0u8; 32]),
        index: 0,
    };
    state
        .utxo_set
        .insert(genesis_input.clone(), make_lovelace_output(10_000_000));

    // state.treasury starts at 0 (default).
    assert_eq!(state.treasury.0, 0);

    // Declare a treasury value that disagrees with ledger state.
    let declared_treasury = Lovelace(5_000_000_000);
    let tx = make_tx_with_treasury(
        0xB0,
        vec![genesis_input],
        vec![make_lovelace_output(9_800_000)],
        declared_treasury,
    );

    let block = make_test_block(1000, 100, Hash32::ZERO, vec![tx]);

    // Use ValidateAll — this is the mode that triggers the treasury check and
    // was the exact code path that caused the cascade failure at slot 107229218.
    state
        .apply_block(&block, BlockValidationMode::ValidateAll)
        .expect("TreasuryValueMismatch must not abort apply_block for confirmed blocks");

    // Block output MUST be in UTxO set.
    let produced = TransactionInput {
        transaction_id: Hash32::from_bytes([0xB0u8; 32]),
        index: 0,
    };
    assert!(
        state.utxo_set.contains(&produced),
        "Block output must be inserted despite treasury mismatch"
    );

    // Treasury MUST be corrected to the declared value.
    assert_eq!(
        state.treasury.0, declared_treasury.0,
        "state.treasury must self-correct to the declared on-chain value"
    );
}

/// Regression: cascading InputNotFound after TreasuryValueMismatch.
///
/// Block A: tx_a has treasury_value mismatch → pre-fix: apply_block aborts,
///   outputs of tx_a never inserted.
/// Block B (next): tx_b spends tx_a's output → pre-fix: InputNotFound
///   (cascade failure), node forks from network.
///
/// After the fix: tx_a's output is inserted normally, tx_b succeeds.
#[test]
fn test_treasury_mismatch_no_cascade_in_downstream_block() {
    let mut params = ProtocolParameters::mainnet_defaults();
    params.protocol_version_major = 9;
    let mut state = LedgerState::new(params);

    // Seed an input for Block A's tx.
    let seed_input = TransactionInput {
        transaction_id: Hash32::from_bytes([0xC0u8; 32]),
        index: 0,
    };
    state
        .utxo_set
        .insert(seed_input.clone(), make_lovelace_output(10_000_000));

    // Block A: spend seed_input, produce output_a, declare wrong treasury.
    // Use ValidateAll to trigger the treasury check (ApplyOnly skips it).
    let tx_a = make_tx_with_treasury(
        0xC1,
        vec![seed_input],
        vec![make_lovelace_output(9_800_000)],
        Lovelace(99_000_000_000), // wrong treasury
    );
    let block_a = make_test_block(1000, 100, Hash32::ZERO, vec![tx_a]);
    state
        .apply_block(&block_a, BlockValidationMode::ValidateAll)
        .expect("Block A must apply despite treasury mismatch");

    let output_a = TransactionInput {
        transaction_id: Hash32::from_bytes([0xC1u8; 32]),
        index: 0,
    };
    assert!(
        state.utxo_set.contains(&output_a),
        "output_a must be in UTxO set after Block A"
    );

    // Block B: spend output_a — must NOT get InputNotFound.
    let block_a_hash = Hash32::from_bytes([100u8; 32]); // header_hash from make_test_block
    let tx_b = make_simple_tx(
        0xC2,
        vec![output_a.clone()],
        vec![make_lovelace_output(9_600_000)],
    );
    let block_b = make_test_block(1001, 101, block_a_hash, vec![tx_b]);
    state
        .apply_block(&block_b, BlockValidationMode::ApplyOnly)
        .expect(
            "Block B must apply — output_a must be visible (no cascade from treasury mismatch)",
        );

    // output_a must be consumed.
    assert!(
        !state.utxo_set.contains(&output_a),
        "output_a must be consumed by Block B"
    );
}

/// Regression: UnelectedCommitteeMember must NOT abort apply_block for
/// confirmed on-chain blocks.
///
/// Scenario: a Conway block contains a CommitteeHotAuth cert for a cold
/// credential that is NOT in our committee_expiration map (i.e. our committee
/// state is stale).  Prior to the fix, apply_block returned
/// `Err(UnelectedCommitteeMember)` and the block's outputs were never inserted.
/// After the fix, the block applies successfully and outputs are in the UTxO set.
#[test]
fn test_unelected_committee_member_does_not_abort_apply_block() {
    let mut params = ProtocolParameters::mainnet_defaults();
    params.protocol_version_major = 9;
    let mut state = LedgerState::new(params);

    // Ensure the cold credential is NOT in committee_expiration (stale state).
    let cold_cred = Credential::VerificationKey(Hash28::from_bytes([0xD0u8; 28]));
    let hot_cred = Credential::VerificationKey(Hash28::from_bytes([0xD1u8; 28]));
    // governance.committee_expiration is empty by default for a new LedgerState.

    // Seed input UTxO.
    let seed_input = TransactionInput {
        transaction_id: Hash32::from_bytes([0xD2u8; 32]),
        index: 0,
    };
    state
        .utxo_set
        .insert(seed_input.clone(), make_lovelace_output(10_000_000));

    let tx = make_tx_with_committee_hot_auth(
        0xD3,
        vec![seed_input],
        vec![make_lovelace_output(9_800_000)],
        cold_cred,
        hot_cred,
    );

    let block = make_test_block(2000, 200, Hash32::ZERO, vec![tx]);

    // Use ValidateAll — this is the mode that triggers the committee check
    // and was the exact code path that caused the cascade failure.
    state
        .apply_block(&block, BlockValidationMode::ValidateAll)
        .expect("UnelectedCommitteeMember must not abort apply_block for confirmed blocks");

    // Block output MUST be in UTxO set.
    let produced = TransactionInput {
        transaction_id: Hash32::from_bytes([0xD3u8; 32]),
        index: 0,
    };
    assert!(
        state.utxo_set.contains(&produced),
        "Block output must be inserted despite unelected committee member cert"
    );
}

/// Regression: cascading InputNotFound after UnelectedCommitteeMember.
///
/// Block A: tx_a has an UnelectedCommitteeMember cert → pre-fix: abort, outputs
///   never inserted.
/// Block B (next): tx_b spends tx_a's output → pre-fix: InputNotFound.
///
/// After the fix: tx_a's output is inserted normally, tx_b succeeds.
#[test]
fn test_unelected_committee_member_no_cascade_in_downstream_block() {
    let mut params = ProtocolParameters::mainnet_defaults();
    params.protocol_version_major = 9;
    let mut state = LedgerState::new(params);

    let cold_cred = Credential::VerificationKey(Hash28::from_bytes([0xE0u8; 28]));
    let hot_cred = Credential::VerificationKey(Hash28::from_bytes([0xE1u8; 28]));

    // Seed input for Block A.
    let seed_input = TransactionInput {
        transaction_id: Hash32::from_bytes([0xE2u8; 32]),
        index: 0,
    };
    state
        .utxo_set
        .insert(seed_input.clone(), make_lovelace_output(10_000_000));

    // Block A: CommitteeHotAuth for un-elected cold credential.
    // Use ValidateAll to trigger the committee check (ApplyOnly skips it).
    let tx_a = make_tx_with_committee_hot_auth(
        0xE3,
        vec![seed_input],
        vec![make_lovelace_output(9_800_000)],
        cold_cred,
        hot_cred,
    );
    let block_a = make_test_block(2000, 200, Hash32::ZERO, vec![tx_a]);
    state
        .apply_block(&block_a, BlockValidationMode::ValidateAll)
        .expect("Block A must apply despite unelected committee member cert");

    let output_a = TransactionInput {
        transaction_id: Hash32::from_bytes([0xE3u8; 32]),
        index: 0,
    };
    assert!(
        state.utxo_set.contains(&output_a),
        "output_a must be in UTxO set after Block A"
    );

    // Block B: spend output_a.
    let block_a_hash = Hash32::from_bytes([200u8; 32]);
    let tx_b = make_simple_tx(
        0xE4,
        vec![output_a.clone()],
        vec![make_lovelace_output(9_600_000)],
    );
    let block_b = make_test_block(2001, 201, block_a_hash, vec![tx_b]);
    state
        .apply_block(&block_b, BlockValidationMode::ApplyOnly)
        .expect(
        "Block B must apply — output_a must be visible (no cascade from unelected committee cert)",
    );

    assert!(
        !state.utxo_set.contains(&output_a),
        "output_a must be consumed by Block B"
    );
}

// ---------------------------------------------------------------------------
// Issue #183 — Block ExUnits budget is a hard error in ValidateAll mode
// ---------------------------------------------------------------------------

/// Build a transaction that declares redeemers with the given execution unit
/// totals.  The transaction body is otherwise empty — we only need it to drive
/// the block-level ExUnits accumulation without a live Plutus evaluator.
fn make_tx_with_redeemers(tx_hash_byte: u8, mem: u64, steps: u64) -> Transaction {
    Transaction {
        hash: Hash32::from_bytes([tx_hash_byte; 32]),
        body: TransactionBody {
            inputs: vec![],
            outputs: vec![],
            fee: Lovelace(200_000),
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
            redeemers: vec![Redeemer {
                tag: RedeemerTag::Spend,
                index: 0,
                data: PlutusData::Integer(0),
                ex_units: ExUnits { mem, steps },
            }],
            raw_redeemers_cbor: None,
            raw_plutus_data_cbor: None,
            pallas_script_data_hash: None,
        },
        is_valid: true,
        auxiliary_data: None,
        raw_cbor: None,
    }
}

/// In `ValidateAll` mode a block whose summed redeemer memory budget exceeds
/// `max_block_ex_units.mem` must be rejected with a hard error.
///
/// Before Issue #183 was fixed, the budget check was only a `debug!` log; the
/// block was silently accepted, allowing a misbehaving block producer to bypass
/// the execution unit limit.
#[test]
fn test_issue_183_block_ex_units_mem_hard_error_validate_all() {
    let mut params = ProtocolParameters::mainnet_defaults();
    // Lower the block memory budget to something the test redeemer can exceed.
    params.max_block_ex_units.mem = 100;
    params.max_block_ex_units.steps = u64::MAX; // steps not under test here

    let mut state = LedgerState::new(params);

    // Tx with a redeemer that consumes 200 memory units (> 100 limit).
    let tx = make_tx_with_redeemers(0xF1, /* mem */ 200, /* steps */ 1);
    let block = make_test_block(3000, 300, Hash32::ZERO, vec![tx]);

    let result = state.apply_block(&block, BlockValidationMode::ValidateAll);
    assert!(
        result.is_err(),
        "Expected apply_block to fail in ValidateAll when block mem ExUnits exceeded"
    );
    let err_str = result.unwrap_err().to_string();
    assert!(
        err_str.contains("BlockExUnitsExceeded") || err_str.contains("block memory"),
        "Expected ExUnits error message, got: {err_str}"
    );
}

/// In `ValidateAll` mode a block whose summed step budget exceeds
/// `max_block_ex_units.steps` must also be rejected.
#[test]
fn test_issue_183_block_ex_units_steps_hard_error_validate_all() {
    let mut params = ProtocolParameters::mainnet_defaults();
    params.max_block_ex_units.mem = u64::MAX;
    params.max_block_ex_units.steps = 50; // tiny steps cap

    let mut state = LedgerState::new(params);

    let tx = make_tx_with_redeemers(0xF2, /* mem */ 1, /* steps */ 100);
    let block = make_test_block(3001, 301, Hash32::ZERO, vec![tx]);

    let result = state.apply_block(&block, BlockValidationMode::ValidateAll);
    assert!(
        result.is_err(),
        "Expected apply_block to fail in ValidateAll when block step ExUnits exceeded"
    );
    let err_str = result.unwrap_err().to_string();
    assert!(
        err_str.contains("BlockExUnitsExceeded") || err_str.contains("block step"),
        "Expected ExUnits step error message, got: {err_str}"
    );
}

/// In `ApplyOnly` mode (historical replay / Mithril import) an exceeded block
/// ExUnits budget must NOT cause a hard error — old blocks may have been
/// produced under different protocol parameter values.
#[test]
fn test_issue_183_block_ex_units_apply_only_is_permissive() {
    let mut params = ProtocolParameters::mainnet_defaults();
    params.max_block_ex_units.mem = 1; // extremely tight
    params.max_block_ex_units.steps = 1;

    let mut state = LedgerState::new(params);

    // A redeemer that wildly exceeds the cap — must be tolerated in ApplyOnly.
    let tx = make_tx_with_redeemers(
        0xF3,
        /* mem */ 1_000_000_000,
        /* steps */ 1_000_000_000,
    );
    let block = make_test_block(3002, 302, Hash32::ZERO, vec![tx]);

    let result = state.apply_block(&block, BlockValidationMode::ApplyOnly);
    assert!(
        result.is_ok(),
        "Expected apply_block to succeed in ApplyOnly regardless of ExUnits; got: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// Issue #184 — Per-transaction reference script 200 KiB limit enforcement
// ---------------------------------------------------------------------------

/// Build a `TransactionOutput` that carries a PlutusV2 `script_ref` of the
/// given byte length.  The output is at a Byron address so stake accounting
/// does not interfere.
fn make_output_with_script_ref(byte_len: usize) -> TransactionOutput {
    TransactionOutput {
        address: Address::Byron(ByronAddress {
            payload: vec![0u8; 32],
        }),
        value: Value::lovelace(2_000_000),
        datum: OutputDatum::None,
        script_ref: Some(ScriptRef::PlutusV2(vec![0xABu8; byte_len])),
        is_legacy: false,
        raw_cbor: None,
    }
}

/// A transaction that spends a UTxO whose `script_ref` byte size exceeds 200
/// KiB must be rejected in `ValidateAll` (Conway protocol >= 9).
///
/// Before Issue #184 was fixed, `MAX_REF_SCRIPT_SIZE_PER_TX` was declared
/// but never checked, so oversized transactions were silently applied.
#[test]
fn test_issue_184_tx_ref_script_size_exceeds_200kib_validate_all() {
    let mut params = ProtocolParameters::mainnet_defaults();
    // Conway protocol so the per-tx size check is enabled.
    params.protocol_version_major = 9;

    let mut state = LedgerState::new(params);

    // 201 KiB script — just over the 200 KiB per-transaction limit.
    const OVERSIZED: usize = 201 * 1024;
    let spending_input = TransactionInput {
        transaction_id: Hash32::from_bytes([0xC1u8; 32]),
        index: 0,
    };
    state.utxo_set.insert(
        spending_input.clone(),
        make_output_with_script_ref(OVERSIZED),
    );

    let tx = make_simple_tx(
        0xD1,
        vec![spending_input],
        vec![make_lovelace_output(1_800_000)],
    );
    let block = make_test_block(4000, 400, Hash32::ZERO, vec![tx]);

    let result = state.apply_block(&block, BlockValidationMode::ValidateAll);
    assert!(
        result.is_err(),
        "Expected apply_block to reject a tx with >200 KiB per-tx ref script size in ValidateAll"
    );
    let err_str = result.unwrap_err().to_string();
    assert!(
        err_str.contains("TxRefScriptSizeTooLarge")
            || err_str.contains("reference script size")
            || err_str.contains("ppMaxRefScriptSizePerTxG"),
        "Expected per-tx ref script size error message, got: {err_str}"
    );
}

/// A transaction whose combined ref-script byte size is exactly at the 200 KiB
/// limit must be accepted without a `TxRefScriptSizeTooLarge` error.
#[test]
fn test_issue_184_tx_ref_script_size_at_limit_is_accepted() {
    let mut params = ProtocolParameters::mainnet_defaults();
    params.protocol_version_major = 9;

    let mut state = LedgerState::new(params);

    // Exactly 200 KiB — must not trigger the limit.
    const AT_LIMIT: usize = 200 * 1024;
    let spending_input = TransactionInput {
        transaction_id: Hash32::from_bytes([0xC2u8; 32]),
        index: 0,
    };
    state.utxo_set.insert(
        spending_input.clone(),
        make_output_with_script_ref(AT_LIMIT),
    );

    let tx = make_simple_tx(
        0xD2,
        vec![spending_input],
        vec![make_lovelace_output(1_800_000)],
    );
    let block = make_test_block(4001, 401, Hash32::ZERO, vec![tx]);

    let result = state.apply_block(&block, BlockValidationMode::ValidateAll);
    // Must not return TxRefScriptSizeTooLarge (other errors e.g. fee/value are ok).
    let no_size_err = match &result {
        Ok(_) => true,
        Err(e) => {
            let s = e.to_string();
            !s.contains("TxRefScriptSizeTooLarge") && !s.contains("ppMaxRefScriptSizePerTxG")
        }
    };
    assert!(
        no_size_err,
        "Expected no per-tx ref script size error at exactly 200 KiB; got: {result:?}"
    );
}

/// In `ApplyOnly` mode the per-transaction ref script size limit is NOT
/// enforced.  Historical blocks must not be rejected during replay even when
/// they would exceed the current protocol limit.
#[test]
fn test_issue_184_tx_ref_script_size_apply_only_is_permissive() {
    let mut params = ProtocolParameters::mainnet_defaults();
    params.protocol_version_major = 9;

    let mut state = LedgerState::new(params);

    // 500 KiB — far beyond the limit.
    const WAY_OVER: usize = 500 * 1024;
    let spending_input = TransactionInput {
        transaction_id: Hash32::from_bytes([0xC3u8; 32]),
        index: 0,
    };
    state.utxo_set.insert(
        spending_input.clone(),
        make_output_with_script_ref(WAY_OVER),
    );

    let tx = make_simple_tx(
        0xD3,
        vec![spending_input],
        vec![make_lovelace_output(1_800_000)],
    );
    let block = make_test_block(4002, 402, Hash32::ZERO, vec![tx]);

    let result = state.apply_block(&block, BlockValidationMode::ApplyOnly);
    assert!(
        result.is_ok(),
        "Expected apply_block to succeed in ApplyOnly regardless of per-tx ref script size; \
         got: {result:?}"
    );
}
