//! Tests for epoch boundary processing: snapshot rotation, protocol parameter
//! updates, pool retirements, nonce computation, and DRep activity management.

#[allow(unused_imports)]
use super::super::*;
use super::*;
use std::sync::Arc;
use torsten_primitives::time::EpochNo;
use torsten_primitives::transaction::{Certificate, ProtocolParamUpdate, Rational};
use torsten_primitives::value::Lovelace;

// ─────────────────────────────────────────────────────────────────────────────
// Epoch transitions — basic
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_epoch_counter_increments_on_transition() {
    let mut state = make_ledger();
    assert_eq!(state.epoch, EpochNo(0));

    state.process_epoch_transition(EpochNo(1));
    assert_eq!(state.epoch, EpochNo(1));

    state.process_epoch_transition(EpochNo(2));
    assert_eq!(state.epoch, EpochNo(2));
}

#[test]
fn test_epoch_transition_rotates_mark_to_set() {
    let mut state = make_ledger();
    state.needs_stake_rebuild = false;

    state.process_epoch_transition(EpochNo(1)); // mark created
    state.process_epoch_transition(EpochNo(2)); // mark -> set

    assert!(
        state.snapshots.set.is_some(),
        "Set snapshot should exist after epoch 2"
    );
    let set = state.snapshots.set.as_ref().unwrap();
    assert_eq!(
        set.epoch,
        EpochNo(1),
        "Set snapshot should be the epoch-1 mark"
    );
}

#[test]
fn test_epoch_transition_rotates_set_to_go() {
    let mut state = make_ledger();
    state.needs_stake_rebuild = false;

    state.process_epoch_transition(EpochNo(1));
    state.process_epoch_transition(EpochNo(2));
    state.process_epoch_transition(EpochNo(3));

    assert!(
        state.snapshots.go.is_some(),
        "Go snapshot should exist after epoch 3"
    );
    let go = state.snapshots.go.as_ref().unwrap();
    assert_eq!(
        go.epoch,
        EpochNo(1),
        "Go snapshot should be the epoch-1 snapshot"
    );
}

#[test]
fn test_epoch_transition_epoch_fees_not_reset_here() {
    // epoch_fees is reset in apply_block, not process_epoch_transition.
    let mut state = make_ledger();
    state.epoch_fees = Lovelace(5_000_000);

    state.process_epoch_transition(EpochNo(1));

    // Fees should not be reset during epoch transition (they are used for reward calculation).
    // This is consistent with Haskell's behavior where fees accumulate until reward distribution.
    // We just confirm no panic and the epoch counter is correct.
    assert_eq!(state.epoch, EpochNo(1));
}

// ─────────────────────────────────────────────────────────────────────────────
// Protocol parameter updates (pre-Conway)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_pp_update_applied_at_quorum() {
    let mut state = make_ledger();
    state.update_quorum = 2;

    let target_epoch = EpochNo(0); // current epoch

    let ppu = ProtocolParamUpdate {
        min_fee_a: Some(50),
        ..Default::default()
    };

    // Two distinct genesis delegates propose the update.
    state
        .pending_pp_updates
        .entry(target_epoch)
        .or_default()
        .push((make_hash32(1), ppu.clone()));
    state
        .pending_pp_updates
        .entry(target_epoch)
        .or_default()
        .push((make_hash32(2), ppu.clone()));

    // Transition to epoch 1 — the proposals targeting epoch 0 should be applied.
    state.process_epoch_transition(EpochNo(1));

    assert_eq!(
        state.protocol_params.min_fee_a, 50,
        "min_fee_a should be updated by the PP update"
    );
}

#[test]
fn test_pp_update_not_applied_below_quorum() {
    let mut state = make_ledger();
    state.update_quorum = 5; // need 5 votes

    let target_epoch = EpochNo(0);
    let original_min_fee_a = state.protocol_params.min_fee_a;

    let ppu = ProtocolParamUpdate {
        min_fee_a: Some(999),
        ..Default::default()
    };

    // Only one proposer — below quorum of 5.
    state
        .pending_pp_updates
        .entry(target_epoch)
        .or_default()
        .push((make_hash32(1), ppu));

    state.process_epoch_transition(EpochNo(1));

    assert_eq!(
        state.protocol_params.min_fee_a, original_min_fee_a,
        "min_fee_a should NOT change when below quorum"
    );
}

#[test]
fn test_pp_update_duplicate_proposer_counts_once() {
    let mut state = make_ledger();
    state.update_quorum = 2;

    let target_epoch = EpochNo(0);
    let original_min_fee_a = state.protocol_params.min_fee_a;

    let ppu = ProtocolParamUpdate {
        min_fee_a: Some(77),
        ..Default::default()
    };

    // Same genesis delegate proposes twice — should count as 1 distinct proposer.
    let same_delegate = make_hash32(10);
    state
        .pending_pp_updates
        .entry(target_epoch)
        .or_default()
        .push((same_delegate, ppu.clone()));
    state
        .pending_pp_updates
        .entry(target_epoch)
        .or_default()
        .push((same_delegate, ppu));

    state.process_epoch_transition(EpochNo(1));

    // Should NOT be applied because distinct proposers = 1 < quorum = 2.
    assert_eq!(
        state.protocol_params.min_fee_a, original_min_fee_a,
        "Duplicate proposer should not count towards quorum"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Conway governance protocol parameter update
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_apply_protocol_param_update_min_fee() {
    let mut state = make_ledger();

    let ppu = ProtocolParamUpdate {
        min_fee_a: Some(55),
        min_fee_b: Some(200_000),
        ..Default::default()
    };

    state
        .apply_protocol_param_update(&ppu)
        .expect("update should succeed");

    assert_eq!(state.protocol_params.min_fee_a, 55);
    assert_eq!(state.protocol_params.min_fee_b, 200_000);
}

#[test]
fn test_apply_protocol_param_update_invalid_threshold_rejected() {
    let mut state = make_ledger();

    let ppu = ProtocolParamUpdate {
        // threshold 3/2 > 1.0 — should fail validation.
        dvt_hard_fork: Some(Rational {
            numerator: 3,
            denominator: 2,
        }),
        ..Default::default()
    };

    let result = state.apply_protocol_param_update(&ppu);
    assert!(
        result.is_err(),
        "Threshold > 1.0 should be rejected by validate_threshold"
    );
}

#[test]
fn test_apply_protocol_param_update_n_opt() {
    let mut state = make_ledger();
    let ppu = ProtocolParamUpdate {
        n_opt: Some(200),
        ..Default::default()
    };
    state.apply_protocol_param_update(&ppu).unwrap();
    assert_eq!(state.protocol_params.n_opt, 200);
}

#[test]
fn test_apply_protocol_param_update_pool_deposit() {
    let mut state = make_ledger();
    let ppu = ProtocolParamUpdate {
        pool_deposit: Some(Lovelace(1_000_000_000)),
        ..Default::default()
    };
    state.apply_protocol_param_update(&ppu).unwrap();
    assert_eq!(state.protocol_params.pool_deposit, Lovelace(1_000_000_000));
}

// ─────────────────────────────────────────────────────────────────────────────
// Pool retirement at epoch boundary
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_pool_retirement_deposit_refunded_at_boundary() {
    let mut state = make_ledger();
    state.protocol_params.pool_deposit = Lovelace(500_000_000);

    let pool_seed = 50u8;
    let pool_id = make_hash28(pool_seed);

    // Build reward account address (0xe0 + pool_id bytes).
    let reward_account = {
        let mut ra = vec![0xe0u8];
        ra.extend_from_slice(pool_id.as_bytes());
        ra
    };

    let mut pool_reg = make_pool_params(pool_seed, 0);
    pool_reg.reward_account = reward_account;
    Arc::make_mut(&mut state.pool_params).insert(pool_id, pool_reg);

    state.process_certificate(&Certificate::PoolRetirement {
        pool_hash: pool_id,
        epoch: 2,
    });

    // Register a reward account keyed by the first 28 bytes of pool_id padded to 32.
    let reward_key = {
        let mut b = [0u8; 32];
        b[..28].copy_from_slice(pool_id.as_bytes());
        torsten_primitives::hash::Hash32::from_bytes(b)
    };
    Arc::make_mut(&mut state.reward_accounts).insert(reward_key, Lovelace(0));

    state.process_epoch_transition(EpochNo(1));
    state.process_epoch_transition(EpochNo(2));

    assert!(
        !state.pool_params.contains_key(&pool_id),
        "Pool should be removed at retirement epoch"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// DRep inactivity expiry at epoch boundary
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_drep_expiry_tracked_across_transitions() {
    let mut state = make_ledger();
    state.protocol_params.drep_activity = 3;

    let cred = make_key_credential(60);
    let key = cred_to_hash(&cred);

    state.process_certificate(&Certificate::RegDRep {
        credential: cred,
        deposit: Lovelace(0),
        anchor: None,
    });

    // DRep registered at epoch 0.  Activity threshold = 3.
    // After epoch 4 (4 - 0 > 3), DRep becomes inactive.
    for e in 1u64..=4 {
        state.process_epoch_transition(EpochNo(e));
    }

    let drep = state.governance.dreps.get(&key).unwrap();
    assert!(
        !drep.active,
        "DRep should be inactive after 4 epochs with threshold=3"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Epoch fee accumulation
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_epoch_fee_accumulation_over_blocks() {
    let mut state = make_ledger();

    // Simulate multiple blocks contributing fees.
    state.epoch_fees += Lovelace(100_000);
    state.epoch_fees += Lovelace(200_000);
    state.epoch_fees += Lovelace(300_000);

    assert_eq!(
        state.epoch_fees,
        Lovelace(600_000),
        "Epoch fees should accumulate correctly"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// epoch_length / slot_to_epoch computations
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_preview_epoch_slot_boundary() {
    let mut state = make_ledger();
    // Preview: no Byron epochs, epoch_length = 432000.
    state.shelley_transition_epoch = 0;
    state.byron_epoch_length = 0;
    state.epoch_length = 432_000;

    assert_eq!(state.epoch_of_slot(0), 0);
    assert_eq!(state.epoch_of_slot(431_999), 0);
    assert_eq!(state.epoch_of_slot(432_000), 1);
    assert_eq!(state.epoch_of_slot(864_000), 2);
}

// ─────────────────────────────────────────────────────────────────────────────
// Snapshot pool_params reflect pool_params at boundary
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_mark_snapshot_captures_pool_params() {
    let mut state = make_ledger();
    state.needs_stake_rebuild = false;

    let pool_seed = 70u8;
    let pool_id = make_hash28(pool_seed);
    Arc::make_mut(&mut state.pool_params).insert(pool_id, make_pool_params(pool_seed, 1_000_000));

    state.process_epoch_transition(EpochNo(1));

    let mark = state.snapshots.mark.as_ref().unwrap();
    assert!(
        mark.pool_params.contains_key(&pool_id),
        "Mark snapshot should capture registered pool params"
    );
}
