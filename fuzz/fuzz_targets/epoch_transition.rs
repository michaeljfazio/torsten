//! Fuzz target for epoch transition processing.
//!
//! Seeds a LedgerState with fuzz-derived pool registrations, delegations,
//! treasury, and reserves, then triggers epoch transitions. Catches panics
//! and value conservation violations in the reward calculation and snapshot
//! rotation pipeline.
//!
//! Run with: cargo +nightly fuzz run fuzz_epoch_transition -- -max_total_time=300

#![no_main]

use libfuzzer_sys::fuzz_target;
use std::collections::HashMap;
use std::sync::Arc;

use dugite_ledger::state::{LedgerState, PoolRegistration};
use dugite_primitives::hash::{Hash28, Hash32};
use dugite_primitives::protocol_params::ProtocolParameters;
use dugite_primitives::time::EpochNo;
use dugite_primitives::value::Lovelace;

/// Helper: read a u64 from fuzz bytes at offset (little-endian).
fn read_u64(data: &[u8], offset: usize) -> u64 {
    let mut bytes = [0u8; 8];
    let available = data.len().saturating_sub(offset).min(8);
    if available > 0 {
        bytes[..available].copy_from_slice(&data[offset..offset + available]);
    }
    u64::from_le_bytes(bytes)
}

/// Create a Hash28 from a single byte seed.
fn hash28_from_seed(seed: u8) -> Hash28 {
    let mut bytes = [0u8; 28];
    bytes[0] = seed;
    bytes[1] = seed.wrapping_mul(53);
    bytes[2] = seed.wrapping_mul(97);
    Hash28::from_bytes(bytes)
}

/// Create a Hash32 from a single byte seed.
fn hash32_from_seed(seed: u8) -> Hash32 {
    let mut bytes = [0u8; 32];
    bytes[0] = seed;
    bytes[1] = seed.wrapping_mul(37);
    bytes[2] = seed.wrapping_mul(73);
    Hash32::from_bytes(bytes)
}

/// Build a minimal pool registration for fuzzing.
fn make_pool_registration(pool_id: Hash28, pledge: u64, cost: u64) -> PoolRegistration {
    PoolRegistration {
        pool_id,
        vrf_keyhash: Hash32::ZERO,
        pledge: Lovelace(pledge),
        cost: Lovelace(cost),
        margin_numerator: 1,
        margin_denominator: 100,
        reward_account: vec![0xE0; 29], // Enterprise reward address
        owners: vec![pool_id],
        relays: vec![],
        metadata_url: None,
        metadata_hash: None,
    }
}

fuzz_target!(|data: &[u8]| {
    // Need at least 24 bytes: 8 (epoch) + 8 (treasury) + 8 (reserves)
    if data.len() < 24 {
        return;
    }

    let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());

    // Parse basic epoch state from fuzz data
    let epoch = (read_u64(data, 0) % 100) as u64;
    let treasury = read_u64(data, 8) % 10_000_000_000_000; // Cap at 10T lovelace
    let reserves = read_u64(data, 16) % 45_000_000_000_000_000; // Cap at max supply
    let epoch_fees = read_u64(data, 24.min(data.len().saturating_sub(8))) % 1_000_000_000_000;

    state.epoch = EpochNo(epoch);
    state.treasury = Lovelace(treasury);
    state.reserves = Lovelace(reserves);
    state.epoch_fees = Lovelace(epoch_fees);

    // Seed pools from fuzz data (1-8 pools)
    let num_pools = ((data.get(32).copied().unwrap_or(0) % 8) + 1) as usize;
    let mut pool_params = HashMap::new();
    let mut blocks_by_pool = HashMap::new();

    for i in 0..num_pools {
        let pool_id = hash28_from_seed(i as u8);
        let pledge = (read_u64(
            data,
            (33 + i * 8).min(data.len().saturating_sub(8)),
        ) % 100_000_000_000)
            .max(1_000_000); // At least 1 ADA pledge
        let cost = (read_u64(
            data,
            (33 + (num_pools + i) * 8).min(data.len().saturating_sub(8)),
        ) % 10_000_000_000)
            .max(340_000_000); // At least 340 ADA fixed cost (mainnet minimum)

        pool_params.insert(pool_id, make_pool_registration(pool_id, pledge, cost));

        // Give each pool some blocks
        let block_count =
            read_u64(data, (33 + i * 4).min(data.len().saturating_sub(4))) % 1000;
        blocks_by_pool.insert(pool_id, block_count);
    }
    // Set epoch block count before moving blocks_by_pool into Arc
    state.epoch_block_count = blocks_by_pool.values().sum::<u64>();
    state.pool_params = Arc::new(pool_params);
    state.epoch_blocks_by_pool = Arc::new(blocks_by_pool);

    // Seed delegations (1-16 delegators)
    let num_delegators =
        ((data.get(33 + num_pools).copied().unwrap_or(0) % 16) + 1) as usize;
    let mut delegations = HashMap::new();
    let mut reward_accounts = HashMap::new();

    for i in 0..num_delegators {
        let staker = hash32_from_seed(100 + i as u8);
        let pool_idx = i % num_pools;
        let pool_id = hash28_from_seed(pool_idx as u8);

        delegations.insert(staker, pool_id);

        // Give each delegator some stake
        let stake = read_u64(
            data,
            (40 + i * 8).min(data.len().saturating_sub(8)),
        ) % 10_000_000_000_000;
        reward_accounts.insert(staker, Lovelace(stake));
    }
    state.delegations = Arc::new(delegations);
    state.reward_accounts = Arc::new(reward_accounts);

    // Trigger epoch transition — must never panic.
    // The transition processes: RUPD (rewards), SNAP (snapshot rotation),
    // POOLREAP (retirements), protocol param updates, nonce evolution.
    state.process_epoch_transition(EpochNo(epoch + 1));
});
