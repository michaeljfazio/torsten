//! Tests for UTxO set CRUD operations, multi-asset tracking, and rollback.

use super::super::*;
use super::*;
use torsten_primitives::transaction::{OutputDatum, TransactionInput, TransactionOutput};
use torsten_primitives::value::{AssetName, Lovelace, Value};

// ─────────────────────────────────────────────────────────────────────────────
// Insert / lookup / delete
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_utxo_insert_and_lookup() {
    // A freshly inserted UTxO entry should be retrievable.
    let mut state = make_ledger();
    let input = make_input(1);
    let output = make_output(5_000_000);

    state.utxo_set.insert(input.clone(), output.clone());

    let found = state.utxo_set.lookup(&input);
    assert!(found.is_some(), "UTxO should be present after insert");
    assert_eq!(found.unwrap().value.coin, Lovelace(5_000_000));
}

#[test]
fn test_utxo_delete_removes_entry() {
    let mut state = make_ledger();
    let input = make_input(2);
    state.utxo_set.insert(input.clone(), make_output(1_000_000));

    state.utxo_set.remove(&input);

    assert!(
        state.utxo_set.lookup(&input).is_none(),
        "UTxO should be absent after removal"
    );
}

#[test]
fn test_utxo_delete_nonexistent_is_noop() {
    let mut state = make_ledger();
    let input = make_input(3);
    // Remove without inserting — must not panic.
    state.utxo_set.remove(&input);
    assert_eq!(state.utxo_set.len(), 0);
}

#[test]
fn test_utxo_multiple_outputs_same_tx() {
    // Two outputs from the same transaction hash but different indices.
    let mut state = make_ledger();
    let tx_hash = make_hash32(10);

    let input0 = TransactionInput {
        transaction_id: tx_hash,
        index: 0,
    };
    let input1 = TransactionInput {
        transaction_id: tx_hash,
        index: 1,
    };

    state
        .utxo_set
        .insert(input0.clone(), make_output(1_000_000));
    state
        .utxo_set
        .insert(input1.clone(), make_output(2_000_000));

    assert_eq!(state.utxo_set.len(), 2);
    assert_eq!(
        state.utxo_set.lookup(&input0).unwrap().value.coin,
        Lovelace(1_000_000)
    );
    assert_eq!(
        state.utxo_set.lookup(&input1).unwrap().value.coin,
        Lovelace(2_000_000)
    );
}

#[test]
fn test_utxo_set_len_tracks_insertions_and_removals() {
    let mut state = make_ledger();

    let inputs: Vec<TransactionInput> = (0u8..5)
        .map(|i| {
            let inp = make_input(i + 20);
            state.utxo_set.insert(inp.clone(), make_output(1_000_000));
            inp
        })
        .collect();

    assert_eq!(state.utxo_set.len(), 5);

    state.utxo_set.remove(&inputs[0]);
    state.utxo_set.remove(&inputs[2]);
    assert_eq!(state.utxo_set.len(), 3);
}

// ─────────────────────────────────────────────────────────────────────────────
// Multi-asset
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_utxo_multi_asset_round_trip() {
    // An output carrying a native asset should survive an insert/lookup round-trip.
    let mut state = make_ledger();

    // Build a policy-id (Hash28) and asset name.
    let policy_id = make_hash28(42);
    let asset_name = AssetName::new(b"MY_TOKEN".to_vec()).unwrap();
    let asset_qty = 1_000u64;

    let mut multi_asset = std::collections::BTreeMap::new();
    let mut assets = std::collections::BTreeMap::new();
    assets.insert(asset_name.clone(), asset_qty);
    multi_asset.insert(policy_id, assets);

    let input = make_input(50);
    let output = TransactionOutput {
        address: make_enterprise_address(1),
        value: Value {
            coin: Lovelace(2_000_000),
            multi_asset,
        },
        datum: OutputDatum::None,
        script_ref: None,
        is_legacy: false,
        raw_cbor: None,
    };

    state.utxo_set.insert(input.clone(), output);

    let found = state.utxo_set.lookup(&input).expect("UTxO not found");
    assert_eq!(found.value.coin, Lovelace(2_000_000));
    let qty = found
        .value
        .multi_asset
        .get(&policy_id)
        .and_then(|a| a.get(&asset_name))
        .copied();
    assert_eq!(
        qty,
        Some(1_000u64),
        "Multi-asset quantity should be preserved"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Enterprise vs base address in UTxO set
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_utxo_enterprise_address_stored_correctly() {
    let mut state = make_ledger();
    let input = make_input(60);
    let output = make_output(3_000_000);
    state.utxo_set.insert(input.clone(), output.clone());

    let found = state.utxo_set.lookup(&input).unwrap();
    // Enterprise address — no staking credential.
    assert!(
        matches!(
            found.address,
            torsten_primitives::address::Address::Enterprise(_)
        ),
        "Should be an enterprise address"
    );
}

#[test]
fn test_utxo_base_address_stored_correctly() {
    let mut state = make_ledger();
    let input = make_input(61);
    let output = make_stake_output(5_000_000, 1, 2);
    state.utxo_set.insert(input.clone(), output);

    let found = state.utxo_set.lookup(&input).unwrap();
    assert!(
        matches!(found.address, torsten_primitives::address::Address::Base(_)),
        "Should be a base address"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// apply_pending_reward_update bookkeeping
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_apply_pending_reward_update_credits_accounts() {
    use crate::state::PendingRewardUpdate;
    use std::collections::HashMap;
    use std::sync::Arc;

    let mut state = make_ledger();
    let cred_hash = make_hash32(99);

    // Register the reward account before applying rewards.
    Arc::make_mut(&mut state.reward_accounts).insert(cred_hash, Lovelace(0));

    state.pending_reward_update = Some(PendingRewardUpdate {
        rewards: {
            let mut m = HashMap::new();
            m.insert(cred_hash, Lovelace(1_000_000));
            m
        },
        delta_treasury: 500_000,
        delta_reserves: 2_000_000,
    });

    state.apply_pending_reward_update();

    assert_eq!(
        state.reward_accounts.get(&cred_hash).copied(),
        Some(Lovelace(1_000_000)),
        "Reward account should be credited"
    );
    assert_eq!(
        state.treasury,
        Lovelace(500_000),
        "Treasury should increase"
    );
    // reserves started at MAX_LOVELACE_SUPPLY (from LedgerState::new).
    assert_eq!(
        state.reserves.0,
        // MAX_LOVELACE_SUPPLY is from crate::state (via super::super::*)
        MAX_LOVELACE_SUPPLY - 2_000_000,
        "Reserves should decrease"
    );
    assert!(
        state.pending_reward_update.is_none(),
        "Pending update should be consumed"
    );
}

#[test]
fn test_apply_pending_reward_update_noop_when_none() {
    let mut state = make_ledger();
    let initial_treasury = state.treasury;
    let initial_reserves = state.reserves;

    // No pending update — nothing should change.
    state.apply_pending_reward_update();

    assert_eq!(state.treasury, initial_treasury);
    assert_eq!(state.reserves, initial_reserves);
}

// ─────────────────────────────────────────────────────────────────────────────
// Epoch-of-slot calculation
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_epoch_of_slot_shelley_basic() {
    // Mainnet-like: shelley starts at epoch 208, byron_epoch_length=21600,
    // epoch_length=432000.
    let mut state = make_ledger();
    state.shelley_transition_epoch = 208;
    state.byron_epoch_length = 21_600;
    state.epoch_length = 432_000;

    // First Shelley slot = 208 * 21600 = 4_492_800.
    // Slot 4_492_800 -> epoch 208.
    let epoch = state.epoch_of_slot(4_492_800);
    assert_eq!(epoch, 208, "First Shelley slot should be epoch 208");

    // One full epoch later.
    let epoch2 = state.epoch_of_slot(4_492_800 + 432_000);
    assert_eq!(epoch2, 209);
}

#[test]
fn test_epoch_of_slot_byron() {
    let mut state = make_ledger();
    state.shelley_transition_epoch = 208;
    state.byron_epoch_length = 21_600;
    state.epoch_length = 432_000;

    // Slot 0 is Byron epoch 0.
    assert_eq!(state.epoch_of_slot(0), 0);
    // Last slot of Byron epoch 0 = 21599.
    assert_eq!(state.epoch_of_slot(21_599), 0);
    // First slot of Byron epoch 1 = 21600.
    assert_eq!(state.epoch_of_slot(21_600), 1);
}

#[test]
fn test_first_slot_of_epoch_shelley() {
    let mut state = make_ledger();
    state.shelley_transition_epoch = 208;
    state.byron_epoch_length = 21_600;
    state.epoch_length = 432_000;

    let first_shelley_slot: u64 = 208 * 21_600;
    assert_eq!(state.first_slot_of_epoch(208), first_shelley_slot);
    assert_eq!(state.first_slot_of_epoch(209), first_shelley_slot + 432_000);
}

#[test]
fn test_epoch_of_slot_and_first_slot_roundtrip() {
    let mut state = make_ledger();
    state.shelley_transition_epoch = 0;
    state.byron_epoch_length = 0;
    state.epoch_length = 100;

    for epoch in [0u64, 1, 5, 10] {
        let first = state.first_slot_of_epoch(epoch);
        let back = state.epoch_of_slot(first);
        assert_eq!(
            back, epoch,
            "epoch_of_slot(first_slot_of_epoch({0})) should be {0}",
            epoch
        );
    }
}
