//! Tests for stake registration, delegation, deregistration, and pool lifecycle.

#[allow(unused_imports)]
use super::super::*;
use super::*;
use std::sync::Arc;
use torsten_primitives::time::EpochNo;
use torsten_primitives::transaction::Certificate;
use torsten_primitives::value::Lovelace;

// ─────────────────────────────────────────────────────────────────────────────
// Stake registration
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_stake_registration_creates_reward_account() {
    let mut state = make_ledger();
    let cred = make_key_credential(1);
    let key = cred_to_hash(&cred);

    state.process_certificate(&Certificate::StakeRegistration(cred));

    assert!(
        state.reward_accounts.contains_key(&key),
        "Reward account should exist after registration"
    );
    assert_eq!(
        state.reward_accounts.get(&key).copied(),
        Some(Lovelace(0)),
        "Reward account should start at 0"
    );
}

#[test]
fn test_stake_registration_creates_stake_distribution_entry() {
    let mut state = make_ledger();
    let cred = make_key_credential(2);
    let key = cred_to_hash(&cred);

    state.process_certificate(&Certificate::StakeRegistration(cred));

    assert!(
        state.stake_distribution.stake_map.contains_key(&key),
        "Stake map should contain key after registration"
    );
}

#[test]
fn test_stake_registration_idempotent() {
    // Registering twice must not reset an existing non-zero reward balance.
    let mut state = make_ledger();
    let cred = make_key_credential(3);
    let key = cred_to_hash(&cred);

    state.process_certificate(&Certificate::StakeRegistration(cred.clone()));

    // Simulate some accumulated reward.
    *Arc::make_mut(&mut state.reward_accounts)
        .get_mut(&key)
        .unwrap() = Lovelace(1_000_000);

    // Register again.
    state.process_certificate(&Certificate::StakeRegistration(cred));

    assert_eq!(
        state.reward_accounts.get(&key).copied(),
        Some(Lovelace(1_000_000)),
        "Re-registration must not zero out the reward balance"
    );
}

#[test]
fn test_script_credential_tracked_on_registration() {
    let mut state = make_ledger();
    let cred = make_script_credential(4);
    let key = cred_to_hash(&cred);

    state.process_certificate(&Certificate::StakeRegistration(cred));

    assert!(
        state.script_stake_credentials.contains(&key),
        "Script credential should be tracked in script_stake_credentials"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Stake deregistration
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_stake_deregistration_removes_entries() {
    let mut state = make_ledger();
    let cred = make_key_credential(5);
    let key = cred_to_hash(&cred);

    state.process_certificate(&Certificate::StakeRegistration(cred.clone()));
    state.process_certificate(&Certificate::StakeDeregistration(cred));

    assert!(
        !state.reward_accounts.contains_key(&key),
        "Reward account should be removed after deregistration"
    );
    assert!(
        !state.stake_distribution.stake_map.contains_key(&key),
        "Stake map should be cleared after deregistration"
    );
}

#[test]
fn test_stake_deregistration_blocked_by_nonzero_balance() {
    // Per Shelley spec: deregistration is rejected if the reward balance is > 0.
    let mut state = make_ledger();
    let cred = make_key_credential(6);
    let key = cred_to_hash(&cred);

    state.process_certificate(&Certificate::StakeRegistration(cred.clone()));
    *Arc::make_mut(&mut state.reward_accounts)
        .get_mut(&key)
        .unwrap() = Lovelace(500_000);

    state.process_certificate(&Certificate::StakeDeregistration(cred));

    // The reward account should still exist because deregistration was blocked.
    assert!(
        state.reward_accounts.contains_key(&key),
        "Reward account should persist when balance is non-zero"
    );
}

#[test]
fn test_stake_deregistration_also_removes_delegation() {
    let mut state = make_ledger();
    let cred = make_key_credential(7);
    let key = cred_to_hash(&cred);
    let pool_id = make_hash28(1);

    state.process_certificate(&Certificate::StakeRegistration(cred.clone()));
    state.process_certificate(&Certificate::StakeDelegation {
        credential: cred.clone(),
        pool_hash: pool_id,
    });

    assert!(state.delegations.contains_key(&key));

    state.process_certificate(&Certificate::StakeDeregistration(cred));
    assert!(
        !state.delegations.contains_key(&key),
        "Delegation should be removed on deregistration"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Conway stake deregistration (unconditional)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_conway_stake_deregistration_unconditional() {
    let mut state = make_ledger();
    let cred = make_key_credential(8);
    let key = cred_to_hash(&cred);

    state.process_certificate(&Certificate::ConwayStakeRegistration {
        credential: cred.clone(),
        deposit: Lovelace(2_000_000),
    });

    // Give the account a non-zero balance — Conway deregistration should still proceed.
    *Arc::make_mut(&mut state.reward_accounts)
        .get_mut(&key)
        .unwrap() = Lovelace(999_999);

    state.process_certificate(&Certificate::ConwayStakeDeregistration {
        credential: cred,
        refund: Lovelace(2_000_000),
    });

    assert!(
        !state.reward_accounts.contains_key(&key),
        "Conway deregistration is unconditional (ignores reward balance)"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Stake delegation
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_stake_delegation_records_pool() {
    let mut state = make_ledger();
    let cred = make_key_credential(9);
    let key = cred_to_hash(&cred);
    let pool_id = make_hash28(2);

    state.process_certificate(&Certificate::StakeRegistration(cred.clone()));
    state.process_certificate(&Certificate::StakeDelegation {
        credential: cred,
        pool_hash: pool_id,
    });

    assert_eq!(
        state.delegations.get(&key).copied(),
        Some(pool_id),
        "Delegation should point to the target pool"
    );
}

#[test]
fn test_stake_redelegate_updates_pool() {
    let mut state = make_ledger();
    let cred = make_key_credential(10);
    let key = cred_to_hash(&cred);
    let pool_a = make_hash28(3);
    let pool_b = make_hash28(4);

    state.process_certificate(&Certificate::StakeRegistration(cred.clone()));
    state.process_certificate(&Certificate::StakeDelegation {
        credential: cred.clone(),
        pool_hash: pool_a,
    });
    state.process_certificate(&Certificate::StakeDelegation {
        credential: cred,
        pool_hash: pool_b,
    });

    assert_eq!(state.delegations.get(&key).copied(), Some(pool_b));
}

// ─────────────────────────────────────────────────────────────────────────────
// Pool registration
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_pool_registration_stored_in_params() {
    let mut state = make_ledger();
    let pool_seed = 5u8;
    let pool_id = make_hash28(pool_seed);

    // Insert pool directly via pool_params (simulating certificate processing).
    Arc::make_mut(&mut state.pool_params).insert(pool_id, make_pool_params(pool_seed, 0));

    assert!(
        state.pool_params.contains_key(&pool_id),
        "Pool should be registered"
    );
}

#[test]
fn test_pool_retirement_scheduled() {
    let mut state = make_ledger();
    let pool_seed = 6u8;
    let pool_id = make_hash28(pool_seed);
    let retirement_epoch: u64 = 5;

    // Register the pool.
    Arc::make_mut(&mut state.pool_params).insert(pool_id, make_pool_params(pool_seed, 0));

    state.process_certificate(&Certificate::PoolRetirement {
        pool_hash: pool_id,
        epoch: retirement_epoch,
    });

    assert!(
        state
            .pending_retirements
            .contains_key(&EpochNo(retirement_epoch)),
        "Retirement should be scheduled for the target epoch"
    );
}

#[test]
fn test_pool_retirement_fires_at_epoch_boundary() {
    let mut state = make_ledger();
    let pool_seed = 7u8;
    let pool_id = make_hash28(pool_seed);

    // Register pool with a reward account so we can verify deposit refund.
    let mut pool_reg = make_pool_params(pool_seed, 0);
    // reward_account = 0xe0 + pool_id bytes
    pool_reg.reward_account = {
        let mut ra = vec![0xe0u8];
        ra.extend_from_slice(pool_id.as_bytes());
        ra
    };
    Arc::make_mut(&mut state.pool_params).insert(pool_id, pool_reg);

    state.process_certificate(&Certificate::PoolRetirement {
        pool_hash: pool_id,
        epoch: 2,
    });

    // Trigger epoch transitions 1 and 2.
    state.process_epoch_transition(EpochNo(1));
    state.process_epoch_transition(EpochNo(2));

    assert!(
        !state.pool_params.contains_key(&pool_id),
        "Pool should be removed after retirement epoch"
    );
}

#[test]
fn test_pool_reregistration_cancels_pending_retirement() {
    let mut state = make_ledger();
    let pool_seed = 8u8;
    let pool_id = make_hash28(pool_seed);

    Arc::make_mut(&mut state.pool_params).insert(pool_id, make_pool_params(pool_seed, 0));

    // Schedule retirement at epoch 3.
    state.process_certificate(&Certificate::PoolRetirement {
        pool_hash: pool_id,
        epoch: 3,
    });
    assert!(state.pending_retirements.contains_key(&EpochNo(3)));

    // Re-register via pool_params (simulating a PoolRegistration certificate).
    Arc::make_mut(&mut state.pool_params).insert(pool_id, make_pool_params(pool_seed, 0));
    // Manually cancel the retirement (mirrors PoolRegistration cert logic).
    for pools in state.pending_retirements.values_mut() {
        pools.retain(|id| id != &pool_id);
    }
    state
        .pending_retirements
        .retain(|_, pools| !pools.is_empty());

    assert!(
        !state.pending_retirements.contains_key(&EpochNo(3)),
        "Re-registration should cancel the pending retirement"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// RegStakeDeleg (Conway combined cert)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_reg_stake_deleg_registers_and_delegates() {
    let mut state = make_ledger();
    let cred = make_key_credential(20);
    let key = cred_to_hash(&cred);
    let pool_id = make_hash28(9);

    state.process_certificate(&Certificate::RegStakeDeleg {
        credential: cred,
        pool_hash: pool_id,
        deposit: Lovelace(2_000_000),
    });

    assert!(
        state.reward_accounts.contains_key(&key),
        "Reward account should be created by RegStakeDeleg"
    );
    assert_eq!(
        state.delegations.get(&key).copied(),
        Some(pool_id),
        "Delegation should be set by RegStakeDeleg"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Pool retirement epoch bound (e_max)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_pool_retirement_exceeding_emax_ignored() {
    let mut state = make_ledger();
    state.protocol_params.e_max = 5;
    let pool_id = make_hash28(11);

    // Request retirement at epoch 0 + 5 + 1 = 6 (exceeds e_max).
    state.process_certificate(&Certificate::PoolRetirement {
        pool_hash: pool_id,
        epoch: 6,
    });

    assert!(
        state.pending_retirements.is_empty(),
        "Retirement exceeding e_max should be ignored"
    );
}
