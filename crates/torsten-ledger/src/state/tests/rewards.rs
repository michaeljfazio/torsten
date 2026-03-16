//! Tests for reward calculation: RUPD timing, Rat arithmetic, epoch-boundary
//! reward distribution, pledge check, operator/member split.

use super::super::*;
use super::*;
use std::collections::HashMap;
use std::sync::Arc;
use torsten_primitives::hash::{Hash28, Hash32};
use torsten_primitives::time::EpochNo;
use torsten_primitives::value::Lovelace;

// ─────────────────────────────────────────────────────────────────────────────
// Rat arithmetic
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_rat_add() {
    use crate::state::Rat;
    // 1/3 + 1/6 = 1/2
    let a = Rat::from_i128(1, 3);
    let b = Rat::from_i128(1, 6);
    let c = a.add(&b);
    assert_eq!(c.floor_u64(), 0, "1/3 + 1/6 = 1/2 should floor to 0");
    // Use a larger numerator to verify correct sum.
    // (1/3 + 1/6) * 6 = 3 => floor = 3
    let six = Rat::from_i128(6, 1);
    let result = c.mul(&six);
    assert_eq!(result.floor_u64(), 3);
}

#[test]
fn test_rat_sub() {
    use crate::state::Rat;
    // 1/2 - 1/4 = 1/4
    let a = Rat::from_i128(1, 2);
    let b = Rat::from_i128(1, 4);
    let c = a.sub(&b);
    // Multiply by 100 to get a whole number.
    let result = c.mul(&Rat::from_i128(100, 1));
    assert_eq!(result.floor_u64(), 25);
}

#[test]
fn test_rat_mul() {
    use crate::state::Rat;
    // 3/4 * 4/3 = 1
    let a = Rat::from_i128(3, 4);
    let b = Rat::from_i128(4, 3);
    let c = a.mul(&b);
    assert_eq!(c.floor_u64(), 1);
}

#[test]
fn test_rat_div() {
    use crate::state::Rat;
    // (1/2) / (1/4) = 2
    let a = Rat::from_i128(1, 2);
    let b = Rat::from_i128(1, 4);
    let c = a.div(&b);
    assert_eq!(c.floor_u64(), 2);
}

#[test]
fn test_rat_div_by_zero_returns_zero() {
    use crate::state::Rat;
    let a = Rat::from_i128(1, 2);
    let zero = Rat::from_i128(0, 1);
    let c = a.div(&zero);
    assert_eq!(c.floor_u64(), 0);
}

#[test]
fn test_rat_min_rat_picks_smaller() {
    use crate::state::Rat;
    let a = Rat::from_i128(1, 3);
    let b = Rat::from_i128(1, 2);
    assert_eq!(a.min_rat(&b), a);
    assert_eq!(b.min_rat(&a), a);
}

#[test]
fn test_rat_floor_u64_truncates() {
    use crate::state::Rat;
    // 7/2 = 3.5, floor = 3
    let a = Rat::from_i128(7, 2);
    assert_eq!(a.floor_u64(), 3);
}

#[test]
fn test_rat_large_values_no_overflow() {
    use crate::state::Rat;
    // Simulate mainnet-scale reserves: 14 quadrillion lovelace (~14B ADA), rho = 3/1000.
    // This would overflow i128 arithmetic if the intermediate numerator is computed naively
    // as 14_000_000_000_000_000 * 3 = 42_000_000_000_000_000, but BigInt handles it.
    let reserves = 14_000_000_000_000_000i128; // 14 * 10^15 lovelace
    let rho_n = 3i128;
    let rho_d = 1000i128;
    let rho = Rat::from_i128(rho_n, rho_d);
    let result = rho.mul(&Rat::from_i128(reserves, 1));
    // 14_000_000_000_000_000 * 3 / 1000 = 42_000_000_000_000 (42 trillion lovelace)
    assert_eq!(result.floor_u64(), 42_000_000_000_000u64);
}

// ─────────────────────────────────────────────────────────────────────────────
// Build snapshot helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Build a minimal StakeSnapshot with one pool and one delegator.
fn make_single_pool_snapshot(
    pool_id: Hash28,
    cred_hash: Hash32,
    pool_params: &PoolRegistration,
    stake: u64,
) -> StakeSnapshot {
    let mut delegations = HashMap::new();
    delegations.insert(cred_hash, pool_id);

    let mut pool_stake = HashMap::new();
    pool_stake.insert(pool_id, Lovelace(stake));

    let mut pool_params_map = HashMap::new();
    pool_params_map.insert(pool_id, pool_params.clone());

    let mut stake_distribution = HashMap::new();
    stake_distribution.insert(cred_hash, Lovelace(stake));

    StakeSnapshot {
        epoch: EpochNo(1),
        delegations: Arc::new(delegations),
        pool_stake,
        pool_params: Arc::new(pool_params_map),
        stake_distribution: Arc::new(stake_distribution),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// calculate_rewards: zero rewards when no active stake
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_calculate_rewards_empty_snapshot() {
    let state = make_ledger();
    let snapshot = StakeSnapshot {
        epoch: EpochNo(1),
        delegations: Arc::new(HashMap::new()),
        pool_stake: HashMap::new(),
        pool_params: Arc::new(HashMap::new()),
        stake_distribution: Arc::new(HashMap::new()),
    };

    let rupd = state.calculate_rewards(&snapshot);

    assert!(
        rupd.rewards.is_empty(),
        "No rewards should be distributed with empty snapshot"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// calculate_rewards: pledge check prevents rewards
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_calculate_rewards_pledge_not_met() {
    let mut state = make_ledger();
    // Give the ledger some fees and make reserves non-trivial.
    state.epoch_fees = Lovelace(10_000_000);
    state.epoch_block_count = 100;

    let pool_seed = 30u8;
    let pool_id = make_hash28(pool_seed);
    let cred_hash = make_hash32(31);

    // Pool declares a pledge of 1_000 ADA, but owner has 0 delegated stake.
    let mut pool_reg = make_pool_params(pool_seed, 1_000_000_000);

    // Owner hash that does NOT appear in delegations (so owner_stake = 0 < pledge).
    pool_reg.owners = vec![make_hash28(99)];

    let snapshot = make_single_pool_snapshot(pool_id, cred_hash, &pool_reg, 10_000_000_000);

    let rupd = state.calculate_rewards(&snapshot);

    // Since pledge is not met, this pool gets zero rewards.
    assert!(
        rupd.rewards.get(&cred_hash).map(|l| l.0).unwrap_or(0) == 0,
        "Delegator should receive 0 rewards when pledge is not met"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// calculate_rewards: basic positive rewards
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_calculate_rewards_basic_positive() {
    let mut state = make_ledger();

    // Use large fees and 0 expansion (rho=0) to make math simple.
    state.epoch_fees = Lovelace(100_000_000_000); // 100K ADA fees
    state.epoch_block_count = 1;

    // Reserves must be < MAX_LOVELACE_SUPPLY so total_stake > 0.
    // total_stake = MAX_LOVELACE_SUPPLY - reserves.
    // Set reserves to leave 20T lovelace in circulation.
    state.reserves = Lovelace(MAX_LOVELACE_SUPPLY - 20_000_000_000_000u64);

    // rho = 0 (no expansion): all rewards come from fees.
    state.protocol_params.rho = torsten_primitives::transaction::Rational {
        numerator: 0,
        denominator: 1,
    };
    // tau = 0 so all fees go to reward pot.
    state.protocol_params.tau = torsten_primitives::transaction::Rational {
        numerator: 0,
        denominator: 1,
    };
    // a0 = 0 (no pledge influence).
    state.protocol_params.a0 = torsten_primitives::transaction::Rational {
        numerator: 0,
        denominator: 1,
    };
    // n_opt = 1 so the single pool is never over-saturated.
    state.protocol_params.n_opt = 1;

    let pool_seed = 32u8;
    let pool_id = make_hash28(pool_seed);
    let owner_cred_hash = cred_to_hash(&make_key_credential(pool_seed));
    let delegator_cred_hash = make_hash32(33);

    let mut pool_reg = make_pool_params(pool_seed, 0);
    // pledge = 0 so pledge check always passes.
    pool_reg.pledge = Lovelace(0);
    // cost = 0 to simplify (so all reward_pot goes to delegators).
    pool_reg.cost = Lovelace(0);
    pool_reg.margin_numerator = 0;
    pool_reg.margin_denominator = 1;
    // Owner key must appear in delegations for pledge check (pledge=0 always passes regardless).
    pool_reg.owners = vec![make_hash28(pool_seed)];

    // Pool stake = 10T lovelace out of 20T total circulation.
    let pool_stake = 10_000_000_000_000u64;
    let mut delegations = HashMap::new();
    delegations.insert(owner_cred_hash, pool_id);
    delegations.insert(delegator_cred_hash, pool_id);

    let mut pool_stake_map = HashMap::new();
    pool_stake_map.insert(pool_id, Lovelace(pool_stake));

    let mut pool_params_map = HashMap::new();
    pool_params_map.insert(pool_id, pool_reg);

    let mut stake_dist = HashMap::new();
    stake_dist.insert(owner_cred_hash, Lovelace(pool_stake / 2));
    stake_dist.insert(delegator_cred_hash, Lovelace(pool_stake / 2));

    let snapshot = StakeSnapshot {
        epoch: EpochNo(1),
        delegations: Arc::new(delegations),
        pool_stake: pool_stake_map,
        pool_params: Arc::new(pool_params_map),
        stake_distribution: Arc::new(stake_dist),
    };

    // Apparent performance: pool must have produced blocks in epoch_blocks_by_pool,
    // otherwise pool_reward = 0 regardless of stake.
    Arc::make_mut(&mut state.epoch_blocks_by_pool).insert(pool_id, 1);

    let rupd = state.calculate_rewards(&snapshot);

    // At least some rewards should be distributed.
    let total_rewards: u64 = rupd.rewards.values().map(|l| l.0).sum();
    assert!(
        total_rewards > 0,
        "Expected positive rewards with 100K ADA fees and delegated stake (got 0); \
         rupd.delta_treasury={}, rupd.delta_reserves={}, rewards.len()={}",
        rupd.delta_treasury,
        rupd.delta_reserves,
        rupd.rewards.len()
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// RUPD deferred application timing
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_rupd_applied_at_next_epoch_boundary() {
    use crate::state::PendingRewardUpdate;
    let mut state = make_ledger();
    let cred_hash = make_hash32(40);

    // Register the credential.
    Arc::make_mut(&mut state.reward_accounts).insert(cred_hash, Lovelace(0));

    // Inject a pending reward update.
    state.pending_reward_update = Some(PendingRewardUpdate {
        rewards: {
            let mut m = HashMap::new();
            m.insert(cred_hash, Lovelace(5_000_000));
            m
        },
        delta_treasury: 1_000_000,
        delta_reserves: 3_000_000,
    });

    // Process the epoch transition — this should apply the pending update.
    state.process_epoch_transition(EpochNo(1));

    assert_eq!(
        state.reward_accounts.get(&cred_hash).copied(),
        Some(Lovelace(5_000_000)),
        "Rewards should be credited at epoch boundary"
    );
    assert!(
        state.pending_reward_update.is_none(),
        "Pending update should be consumed"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Treasury cut
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_treasury_receives_tau_fraction_of_fees() {
    let mut state = make_ledger();

    // Use a pure-fees scenario (no expansion) by setting reserves to 0
    // and providing a known fee amount.
    state.reserves = Lovelace(0);
    state.epoch_fees = Lovelace(1_000_000_000); // 1000 ADA
    state.epoch_block_count = 100;

    // tau = 2/10 = 20%
    state.protocol_params.tau = torsten_primitives::transaction::Rational {
        numerator: 2,
        denominator: 10,
    };
    state.protocol_params.rho = torsten_primitives::transaction::Rational {
        numerator: 0,
        denominator: 1,
    };

    // No delegated stake — all undistributed rewards go to treasury.
    let snapshot = StakeSnapshot {
        epoch: EpochNo(1),
        delegations: Arc::new(HashMap::new()),
        pool_stake: HashMap::new(),
        pool_params: Arc::new(HashMap::new()),
        stake_distribution: Arc::new(HashMap::new()),
    };

    let rupd = state.calculate_rewards(&snapshot);

    // With rho=0, expansion=0; total = fees = 1B lovelace.
    // Treasury cut = floor(0.2 * 1B) = 200M.
    // reward_pot = 800M, but no delegated stake -> all goes to treasury too.
    // delta_treasury = 200M + 800M = 1B.
    assert_eq!(
        rupd.delta_treasury, 1_000_000_000,
        "All fees should end up in treasury when there is no delegated stake"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Epoch boundary mark/set/go rotation
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_snapshot_rotation_at_epoch_boundary() {
    let mut state = make_ledger();
    state.needs_stake_rebuild = false;

    // Transition through three epochs and verify the snapshot slots rotate.
    state.process_epoch_transition(EpochNo(1));
    assert!(
        state.snapshots.mark.is_some(),
        "Mark snapshot should exist after epoch 1"
    );
    assert!(
        state.snapshots.set.is_none(),
        "Set snapshot should still be empty after epoch 1"
    );

    state.process_epoch_transition(EpochNo(2));
    assert!(
        state.snapshots.set.is_some(),
        "Set snapshot should exist after epoch 2"
    );

    state.process_epoch_transition(EpochNo(3));
    assert!(
        state.snapshots.go.is_some(),
        "Go snapshot should exist after epoch 3"
    );
}

#[test]
fn test_mark_snapshot_captures_current_epoch() {
    let mut state = make_ledger();
    state.needs_stake_rebuild = false;

    state.process_epoch_transition(EpochNo(5));

    let mark = state.snapshots.mark.as_ref().unwrap();
    assert_eq!(
        mark.epoch,
        EpochNo(5),
        "Mark snapshot should record epoch 5"
    );
}
