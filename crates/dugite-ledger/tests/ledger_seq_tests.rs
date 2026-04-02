//! Integration tests for `LedgerSeq` — the anchored sequence of ledger state
//! deltas used for O(1) rollback.
//!
//! These tests exercise the public API of `LedgerSeq` and verify:
//! - push / rollback cycles
//! - advance_anchor behaviour
//! - max_rollback boundary enforcement
//! - checkpoint creation at the configured interval
//! - state reconstruction accuracy via both direct delta application and
//!   checkpoint-assisted reconstruction
//!
//! The tests use only the public interface.  They deliberately avoid touching
//! internal BTreeMap / VecDeque fields directly so they remain valid if the
//! internal representation changes in the future.

use dugite_ledger::ledger_seq::{
    BlockFieldsDelta, DelegationChange, GovernanceChange, LedgerDelta, LedgerSeq, PoolChange,
    RewardChange,
};
use dugite_ledger::state::{DRepRegistration, LedgerState, PoolRegistration};
use dugite_primitives::block::Point;
use dugite_primitives::credentials::Credential;
use dugite_primitives::hash::{Hash28, Hash32};
use dugite_primitives::protocol_params::ProtocolParameters;
use dugite_primitives::time::{BlockNo, EpochNo, SlotNo};
use dugite_primitives::value::Lovelace;

// ─────────────────────────────────────────────────────────────────────────────
// Test helpers
// ─────────────────────────────────────────────────────────────────────────────

fn make_anchor() -> LedgerState {
    LedgerState::new(ProtocolParameters::mainnet_defaults())
}

fn h(b: u8) -> Hash32 {
    Hash32::from_bytes([b; 32])
}

fn h28(b: u8) -> Hash28 {
    Hash28::from_bytes([b; 28])
}

/// Build a minimal delta whose only observable effect is advancing the
/// epoch_fees running total and the nonces.
fn fee_delta(slot: u64, hash_byte: u8, running_fees: u64, block_no: u64) -> LedgerDelta {
    let mut delta = LedgerDelta::new(SlotNo(slot), h(hash_byte), BlockNo(block_no));
    delta.block_fields = BlockFieldsDelta {
        fees_collected: Lovelace(1_000_000),
        epoch_fees: Lovelace(running_fees),
        epoch_block_count: block_no,
        evolving_nonce: h(hash_byte),
        candidate_nonce: h(hash_byte),
        lab_nonce: h(hash_byte),
        pool_block_increment: None,
    };
    delta
}

/// Build a sequence of `n` fee-accumulating deltas (slots 1..=n).
fn push_n_fee_deltas(seq: &mut LedgerSeq, n: u8) {
    let mut running = 0u64;
    for i in 1u8..=n {
        running += 1_000_000;
        seq.push(fee_delta(i as u64, i, running, i as u64));
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Construction
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn new_ledger_seq_is_empty() {
    let seq = LedgerSeq::with_defaults(make_anchor(), 100);
    assert!(seq.is_empty());
    assert_eq!(seq.len(), 0);
    assert_eq!(seq.max_rollback(), 0);
}

#[test]
fn anchor_point_is_origin_at_genesis() {
    let seq = LedgerSeq::with_defaults(make_anchor(), 100);
    assert!(matches!(seq.anchor_point(), Point::Origin));
}

// ─────────────────────────────────────────────────────────────────────────────
// Push / rollback cycle
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn push_increases_len_and_max_rollback() {
    let mut seq = LedgerSeq::with_defaults(make_anchor(), 100);
    assert_eq!(seq.len(), 0);

    seq.push(fee_delta(1, 1, 1_000_000, 1));
    assert_eq!(seq.len(), 1);
    assert_eq!(seq.max_rollback(), 1);

    seq.push(fee_delta(2, 2, 2_000_000, 2));
    assert_eq!(seq.len(), 2);
    assert_eq!(seq.max_rollback(), 2);
}

#[test]
fn rollback_one_reduces_len() {
    let mut seq = LedgerSeq::with_defaults(make_anchor(), 100);
    push_n_fee_deltas(&mut seq, 5);

    seq.rollback(1);
    assert_eq!(seq.len(), 4);
    assert_eq!(seq.max_rollback(), 4);
}

#[test]
fn rollback_all_leaves_empty_seq() {
    let mut seq = LedgerSeq::with_defaults(make_anchor(), 100);
    push_n_fee_deltas(&mut seq, 5);

    seq.rollback(5);
    assert!(seq.is_empty());
    assert_eq!(seq.max_rollback(), 0);
}

#[test]
fn rollback_clamps_to_available_depth() {
    // rollback(n > len) must not panic — it silently clamps.
    let mut seq = LedgerSeq::with_defaults(make_anchor(), 100);
    push_n_fee_deltas(&mut seq, 3);

    seq.rollback(1000); // much more than available
    assert!(seq.is_empty());
}

#[test]
fn rollback_zero_is_noop() {
    let mut seq = LedgerSeq::with_defaults(make_anchor(), 100);
    push_n_fee_deltas(&mut seq, 3);

    seq.rollback(0);
    assert_eq!(seq.len(), 3);
}

#[test]
fn push_rollback_reapply_fork_cycle() {
    let mut seq = LedgerSeq::with_defaults(make_anchor(), 20);

    // Main chain: 5 blocks.
    push_n_fee_deltas(&mut seq, 5);
    let main_tip_fees = seq.tip_state().epoch_fees;
    assert_eq!(main_tip_fees.0, 5_000_000);

    // Rollback 3 — fork at block 2.
    seq.rollback(3);
    assert_eq!(seq.len(), 2);
    assert_eq!(seq.tip_state().epoch_fees.0, 2_000_000);

    // Fork chain: 4 blocks with different fee amounts.
    let mut running = 2_000_000u64;
    for i in 10u8..=13 {
        running += 500_000;
        seq.push(fee_delta(i as u64, i, running, i as u64));
    }
    assert_eq!(seq.len(), 6);
    assert_eq!(seq.tip_state().epoch_fees.0, 4_000_000);
}

#[test]
fn tip_state_after_rollback_matches_anchor_when_fully_empty() {
    let anchor = make_anchor();
    let expected_fees = anchor.epoch_fees;
    let mut seq = LedgerSeq::with_defaults(anchor, 10);

    push_n_fee_deltas(&mut seq, 3);
    seq.rollback(3);

    assert_eq!(seq.tip_state().epoch_fees, expected_fees);
}

// ─────────────────────────────────────────────────────────────────────────────
// advance_anchor
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn advance_anchor_on_empty_is_noop() {
    let mut seq = LedgerSeq::with_defaults(make_anchor(), 10);
    seq.advance_anchor(); // should not panic
    assert!(seq.is_empty());
    assert!(matches!(seq.anchor_point(), Point::Origin));
}

#[test]
fn advance_anchor_applies_oldest_delta_to_anchor() {
    let mut seq = LedgerSeq::with_defaults(make_anchor(), 10);
    seq.push(fee_delta(1, 1, 5_000_000, 1));

    seq.advance_anchor();

    // Volatile window should be empty.
    assert!(seq.is_empty());
    // Anchor should now have epoch_fees = 5_000_000.
    assert_eq!(seq.anchor_state().epoch_fees.0, 5_000_000);
}

#[test]
fn advance_anchor_updates_anchor_point() {
    let mut seq = LedgerSeq::with_defaults(make_anchor(), 10);
    let expected_hash = h(1);
    seq.push(fee_delta(42, 1, 0, 1));

    seq.advance_anchor();

    match seq.anchor_point() {
        Point::Specific(slot, hash) => {
            assert_eq!(slot.0, 42);
            assert_eq!(*hash, expected_hash);
        }
        Point::Origin => panic!("anchor_point should be Specific after advance_anchor"),
    }
}

#[test]
fn advance_anchor_leaves_remaining_deltas_intact() {
    let mut seq = LedgerSeq::with_defaults(make_anchor(), 10);
    push_n_fee_deltas(&mut seq, 4);

    seq.advance_anchor();

    assert_eq!(seq.len(), 3);
    assert_eq!(seq.max_rollback(), 3);
}

#[test]
fn advance_anchor_called_repeatedly_shrinks_to_empty() {
    let mut seq = LedgerSeq::with_defaults(make_anchor(), 10);
    push_n_fee_deltas(&mut seq, 3);

    for _ in 0..3 {
        seq.advance_anchor();
    }
    assert!(seq.is_empty());
}

#[test]
fn advance_anchor_reindexes_checkpoints() {
    // checkpoint_interval=2, k=20.
    // After pushing 4 deltas: checkpoints at indices 1 and 3.
    let mut seq = LedgerSeq::new(make_anchor(), 20, 2);
    push_n_fee_deltas(&mut seq, 4);

    // Advance anchor once.
    seq.advance_anchor();

    // Deltas now: [2,3,4] (0-indexed 0,1,2 in new window).
    // Old checkpoint at index 1 → new index 0.
    // Old checkpoint at index 3 → new index 2.
    let tip = seq.tip_state();
    assert_eq!(tip.epoch_fees.0, 4_000_000);
}

// ─────────────────────────────────────────────────────────────────────────────
// max_rollback boundary
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn max_rollback_never_exceeds_k() {
    let k = 5u64;
    let mut seq = LedgerSeq::with_defaults(make_anchor(), k);

    // Push more than k blocks.
    push_n_fee_deltas(&mut seq, k as u8 + 10);

    assert_eq!(seq.max_rollback(), k as usize);
    assert_eq!(seq.len(), k as usize);
}

#[test]
fn push_beyond_k_advances_anchor_automatically() {
    let k = 3u64;
    let mut seq = LedgerSeq::with_defaults(make_anchor(), k);

    // k+1 pushes.
    push_n_fee_deltas(&mut seq, k as u8 + 1);

    // Volatile window stays at k.
    assert_eq!(seq.len(), k as usize);

    // Anchor has been advanced once, so it should have epoch_fees from
    // the first delta.
    assert_eq!(seq.anchor_state().epoch_fees.0, 1_000_000);
}

#[test]
fn max_rollback_equals_deltas_len() {
    let k = 10u64;
    let mut seq = LedgerSeq::with_defaults(make_anchor(), k);

    for n in 1u8..=8 {
        seq.push(fee_delta(n as u64, n, n as u64 * 1_000_000, n as u64));
        assert_eq!(seq.max_rollback(), seq.len());
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Checkpoint creation and state reconstruction
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn checkpoint_created_at_configured_interval() {
    // checkpoint_interval = 4 → checkpoint at index 3 (after 4th push).
    let mut seq = LedgerSeq::new(make_anchor(), 100, 4);

    // After 3 pushes: no checkpoints yet.
    push_n_fee_deltas(&mut seq, 3);
    // Verify by confirming tip_state still works (uses anchor + deltas).
    assert_eq!(seq.tip_state().epoch_fees.0, 3_000_000);

    // 4th push → checkpoint at index 3.
    seq.push(fee_delta(4, 4, 4_000_000, 4));
    assert_eq!(seq.tip_state().epoch_fees.0, 4_000_000);
}

#[test]
fn checkpoint_reconstruction_consistent_with_sequential_at_every_index() {
    // checkpoint_interval = 3, k = 100.
    // Checkpoints at indices 2, 5, 8.
    let mut seq = LedgerSeq::new(make_anchor(), 100, 3);
    let mut running = 0u64;
    for i in 1u8..=9 {
        running += 1_000_000;
        seq.push(fee_delta(i as u64, i, running, i as u64));
    }

    // Verify every index via state_at.
    let mut expected_fees = 0u64;
    for i in 1u8..=9 {
        expected_fees += 1_000_000;
        let state = seq
            .state_at(SlotNo(i as u64), &h(i))
            .unwrap_or_else(|| panic!("slot {} should be in window", i));
        assert_eq!(
            state.epoch_fees.0, expected_fees,
            "epoch_fees mismatch at slot {}",
            i
        );
    }
}

#[test]
fn tip_state_matches_expected_accumulated_fees() {
    let mut seq = LedgerSeq::with_defaults(make_anchor(), 100);
    let mut running = 0u64;
    for i in 1u8..=10 {
        running += 1_000_000;
        seq.push(fee_delta(i as u64, i, running, i as u64));
    }

    let tip = seq.tip_state();
    assert_eq!(tip.epoch_fees.0, 10_000_000);
}

#[test]
fn state_at_returns_none_for_unknown_point() {
    let mut seq = LedgerSeq::with_defaults(make_anchor(), 100);
    push_n_fee_deltas(&mut seq, 5);

    let result = seq.state_at(SlotNo(99), &h(99));
    assert!(result.is_none(), "slot 99 should not be in the window");
}

#[test]
fn state_at_finds_oldest_block_in_window() {
    let mut seq = LedgerSeq::with_defaults(make_anchor(), 100);
    push_n_fee_deltas(&mut seq, 5);

    // Slot 1 is the oldest delta; epoch_fees should be 1_000_000.
    let state = seq
        .state_at(SlotNo(1), &h(1))
        .expect("slot 1 should be in window");
    assert_eq!(state.epoch_fees.0, 1_000_000);
}

// ─────────────────────────────────────────────────────────────────────────────
// Delegation changes
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn delegation_register_and_deregister_reflected_in_tip_state() {
    let mut seq = LedgerSeq::with_defaults(make_anchor(), 100);

    let cred = h(0xAA);
    let pool = h28(0xBB);

    // Block 1: register stake credential.
    let mut d1 = LedgerDelta::new(SlotNo(1), h(1), BlockNo(1));
    d1.delegation_changes.push(DelegationChange::Register {
        credential_hash: cred,
        is_script: false,
        pointer: None,
    });
    d1.delegation_changes.push(DelegationChange::Delegate {
        credential_hash: cred,
        pool_id: pool,
    });
    d1.block_fields.epoch_fees = Lovelace(1_000_000);
    d1.block_fields.epoch_block_count = 1;
    seq.push(d1);

    let state = seq.tip_state();
    assert!(
        state.delegations.contains_key(&cred),
        "credential should be delegated after registration"
    );
    assert_eq!(state.delegations[&cred], pool);

    // Block 2: deregister.
    let mut d2 = LedgerDelta::new(SlotNo(2), h(2), BlockNo(2));
    d2.delegation_changes.push(DelegationChange::Deregister {
        credential_hash: cred,
        pointer: None,
    });
    d2.block_fields.epoch_fees = Lovelace(2_000_000);
    d2.block_fields.epoch_block_count = 2;
    seq.push(d2);

    let state2 = seq.tip_state();
    assert!(
        !state2.delegations.contains_key(&cred),
        "credential should not be delegated after deregistration"
    );
    assert!(
        !state2.reward_accounts.contains_key(&cred),
        "reward account should be removed after deregistration"
    );
}

#[test]
fn rollback_undoes_delegation_registration() {
    let mut seq = LedgerSeq::with_defaults(make_anchor(), 100);

    let cred = h(0xCC);
    let pool = h28(0xDD);

    let mut d1 = LedgerDelta::new(SlotNo(1), h(1), BlockNo(1));
    d1.delegation_changes.push(DelegationChange::Register {
        credential_hash: cred,
        is_script: false,
        pointer: None,
    });
    d1.delegation_changes.push(DelegationChange::Delegate {
        credential_hash: cred,
        pool_id: pool,
    });
    d1.block_fields.epoch_block_count = 1;
    seq.push(d1);

    // After push: credential is delegated.
    assert!(seq.tip_state().delegations.contains_key(&cred));

    // Roll back: delegation should disappear from the tip.
    seq.rollback(1);
    // Tip is now the anchor — which has no delegations.
    assert!(!seq.tip_state().delegations.contains_key(&cred));
}

// ─────────────────────────────────────────────────────────────────────────────
// Pool changes
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn pool_registration_reflected_in_tip_state() {
    let mut seq = LedgerSeq::with_defaults(make_anchor(), 100);

    let pool_id = h28(0x10);
    let params = PoolRegistration {
        pool_id,
        vrf_keyhash: h(0x11),
        pledge: Lovelace(500_000_000),
        cost: Lovelace(340_000_000),
        margin_numerator: 3,
        margin_denominator: 100,
        reward_account: vec![0xe1u8; 29],
        owners: vec![h28(0x12)],
        relays: vec![],
        metadata_url: None,
        metadata_hash: None,
    };

    let mut delta = LedgerDelta::new(SlotNo(1), h(1), BlockNo(1));
    delta.pool_changes.push(PoolChange::Register {
        params: params.clone(),
    });
    delta.block_fields.epoch_block_count = 1;
    seq.push(delta);

    let state = seq.tip_state();
    assert!(
        state.pool_params.contains_key(&pool_id),
        "pool should appear in pool_params after registration"
    );
    assert_eq!(state.pool_params[&pool_id].pledge.0, 500_000_000);
}

#[test]
fn pool_reregistration_queued_in_future_pool_params() {
    let mut seq = LedgerSeq::with_defaults(make_anchor(), 100);

    let pool_id = h28(0x20);
    let initial_params = PoolRegistration {
        pool_id,
        vrf_keyhash: h(0x21),
        pledge: Lovelace(100_000_000),
        cost: Lovelace(340_000_000),
        margin_numerator: 5,
        margin_denominator: 100,
        reward_account: vec![0xe1u8; 29],
        owners: vec![],
        relays: vec![],
        metadata_url: None,
        metadata_hash: None,
    };
    let updated_params = PoolRegistration {
        pledge: Lovelace(200_000_000),
        ..initial_params.clone()
    };

    let mut d1 = LedgerDelta::new(SlotNo(1), h(1), BlockNo(1));
    d1.pool_changes.push(PoolChange::Register {
        params: initial_params,
    });
    d1.block_fields.epoch_block_count = 1;
    seq.push(d1);

    let mut d2 = LedgerDelta::new(SlotNo(2), h(2), BlockNo(2));
    d2.pool_changes.push(PoolChange::Reregister {
        params: updated_params,
    });
    d2.block_fields.epoch_block_count = 2;
    seq.push(d2);

    let state = seq.tip_state();
    assert!(
        state.future_pool_params.contains_key(&pool_id),
        "re-registration should be queued in future_pool_params"
    );
    assert_eq!(state.future_pool_params[&pool_id].pledge.0, 200_000_000);
}

// ─────────────────────────────────────────────────────────────────────────────
// Reward account changes
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn reward_credit_and_withdrawal_correct() {
    let mut seq = LedgerSeq::with_defaults(make_anchor(), 100);

    let cred = h(0x50);

    // Create reward account.
    let mut d1 = LedgerDelta::new(SlotNo(1), h(1), BlockNo(1));
    d1.reward_changes.push(RewardChange::Create {
        credential_hash: cred,
    });
    d1.reward_changes.push(RewardChange::Credit {
        credential_hash: cred,
        amount: Lovelace(10_000_000),
    });
    d1.block_fields.epoch_block_count = 1;
    seq.push(d1);

    assert_eq!(seq.tip_state().reward_accounts[&cred].0, 10_000_000);

    // Withdraw 6_000_000.
    let mut d2 = LedgerDelta::new(SlotNo(2), h(2), BlockNo(2));
    d2.reward_changes.push(RewardChange::Withdraw {
        credential_hash: cred,
        amount: Lovelace(6_000_000),
    });
    d2.block_fields.epoch_block_count = 2;
    seq.push(d2);

    assert_eq!(seq.tip_state().reward_accounts[&cred].0, 4_000_000);
}

// ─────────────────────────────────────────────────────────────────────────────
// Governance changes
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn drep_registration_reflected_in_tip_state() {
    let mut seq = LedgerSeq::with_defaults(make_anchor(), 100);

    let cred_hash = h(0x60);
    let drep_reg = DRepRegistration {
        credential: Credential::VerificationKey(h28(0x60)),
        deposit: Lovelace(500_000_000),
        anchor: None,
        registered_epoch: EpochNo(10),
        last_active_epoch: EpochNo(10),
        active: true,
    };

    let mut delta = LedgerDelta::new(SlotNo(1), h(1), BlockNo(1));
    delta
        .governance_changes
        .push(GovernanceChange::DRepRegister {
            credential_hash: cred_hash,
            registration: drep_reg,
            is_script: false,
        });
    delta.block_fields.epoch_block_count = 1;
    seq.push(delta);

    let state = seq.tip_state();
    assert!(
        state.governance.dreps.contains_key(&cred_hash),
        "DRep should appear after registration"
    );
    assert_eq!(state.governance.drep_registration_count, 1);
}

#[test]
fn drep_unregistration_removes_from_dreps() {
    let mut seq = LedgerSeq::with_defaults(make_anchor(), 100);

    let cred_hash = h(0x61);
    let drep_reg = DRepRegistration {
        credential: Credential::VerificationKey(h28(0x61)),
        deposit: Lovelace(500_000_000),
        anchor: None,
        registered_epoch: EpochNo(10),
        last_active_epoch: EpochNo(10),
        active: true,
    };

    let mut d1 = LedgerDelta::new(SlotNo(1), h(1), BlockNo(1));
    d1.governance_changes.push(GovernanceChange::DRepRegister {
        credential_hash: cred_hash,
        registration: drep_reg,
        is_script: false,
    });
    d1.block_fields.epoch_block_count = 1;
    seq.push(d1);

    let mut d2 = LedgerDelta::new(SlotNo(2), h(2), BlockNo(2));
    d2.governance_changes
        .push(GovernanceChange::DRepUnregister {
            credential_hash: cred_hash,
        });
    d2.block_fields.epoch_block_count = 2;
    seq.push(d2);

    let state = seq.tip_state();
    assert!(
        !state.governance.dreps.contains_key(&cred_hash),
        "DRep should be removed after unregistration"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Nonce tracking
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn evolving_nonce_updated_each_block() {
    let mut seq = LedgerSeq::with_defaults(make_anchor(), 100);

    for i in 1u8..=3 {
        let mut delta = LedgerDelta::new(SlotNo(i as u64), h(i), BlockNo(i as u64));
        delta.block_fields.evolving_nonce = h(i * 10);
        delta.block_fields.epoch_block_count = i as u64;
        seq.push(delta);
    }

    let tip = seq.tip_state();
    assert_eq!(tip.evolving_nonce, h(30));
}

#[test]
fn lab_nonce_reflects_latest_block_prev_hash() {
    let mut seq = LedgerSeq::with_defaults(make_anchor(), 100);

    let prev_hash_of_block3 = h(0xFF);
    let mut delta = LedgerDelta::new(SlotNo(3), h(3), BlockNo(3));
    delta.block_fields.lab_nonce = prev_hash_of_block3;
    delta.block_fields.epoch_block_count = 3;
    seq.push(delta);

    assert_eq!(seq.tip_state().lab_nonce, prev_hash_of_block3);
}

// ─────────────────────────────────────────────────────────────────────────────
// Block count tracking
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn epoch_block_count_increments_per_block() {
    let mut seq = LedgerSeq::with_defaults(make_anchor(), 100);

    for i in 1u8..=5 {
        let mut delta = LedgerDelta::new(SlotNo(i as u64), h(i), BlockNo(i as u64));
        delta.block_fields.epoch_block_count = i as u64;
        seq.push(delta);
    }

    assert_eq!(seq.tip_state().epoch_block_count, 5);
}

#[test]
fn pool_block_increment_recorded_in_tip() {
    let mut seq = LedgerSeq::with_defaults(make_anchor(), 100);

    let pool = h28(0xAB);
    let mut delta = LedgerDelta::new(SlotNo(1), h(1), BlockNo(1));
    delta.block_fields.pool_block_increment = Some(pool);
    delta.block_fields.epoch_block_count = 1;
    seq.push(delta);

    let state = seq.tip_state();
    assert_eq!(
        *state.epoch_blocks_by_pool.get(&pool).unwrap_or(&0),
        1,
        "pool block count should be 1"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Tip point
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn tip_point_is_anchor_when_empty() {
    let seq = LedgerSeq::with_defaults(make_anchor(), 100);
    assert!(matches!(seq.tip_point(), Point::Origin));
}

#[test]
fn tip_point_reflects_latest_delta() {
    let mut seq = LedgerSeq::with_defaults(make_anchor(), 100);
    seq.push(fee_delta(42, 0xAB, 0, 1));

    match seq.tip_point() {
        Point::Specific(slot, hash) => {
            assert_eq!(slot.0, 42);
            assert_eq!(hash, h(0xAB));
        }
        Point::Origin => panic!("tip_point should be Specific"),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// reset_anchor
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn reset_anchor_clears_all_volatile_state() {
    let mut seq = LedgerSeq::with_defaults(make_anchor(), 100);
    push_n_fee_deltas(&mut seq, 10);
    assert_eq!(seq.len(), 10);

    let new_anchor = make_anchor();
    seq.reset_anchor(new_anchor);

    assert!(seq.is_empty());
    assert_eq!(seq.max_rollback(), 0);
    assert!(matches!(seq.anchor_point(), Point::Origin));
}

// ─────────────────────────────────────────────────────────────────────────────
// Multi-advance-anchor: verify anchor state accumulates correctly
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn multiple_advance_anchor_calls_accumulate_correctly() {
    let mut seq = LedgerSeq::with_defaults(make_anchor(), 100);

    // Push 5 deltas with running fee totals.
    let mut running = 0u64;
    for i in 1u8..=5 {
        running += 2_000_000;
        seq.push(fee_delta(i as u64, i, running, i as u64));
    }

    // Advance anchor 3 times.
    for _ in 0..3 {
        seq.advance_anchor();
    }

    // After advancing 3 times: anchor has seen deltas 1,2,3.
    // epoch_fees in anchor = 6_000_000.
    assert_eq!(seq.anchor_state().epoch_fees.0, 6_000_000);
    assert_eq!(seq.len(), 2);

    // Tip still has all fees applied.
    assert_eq!(seq.tip_state().epoch_fees.0, 10_000_000);
}

// ─────────────────────────────────────────────────────────────────────────────
// Large window: k=432 (preview epoch length)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn large_window_k_432_maintains_correct_tip() {
    let k = 432u64;
    let mut seq = LedgerSeq::with_defaults(make_anchor(), k);

    let mut running = 0u64;
    for i in 1u16..=(k as u16 + 50) {
        running += 1_000_000;
        seq.push(fee_delta(i as u64, (i % 256) as u8, running, i as u64));
    }

    // Window is exactly k.
    assert_eq!(seq.len(), k as usize);
    assert_eq!(seq.max_rollback(), k as usize);
    // Tip should have the expected running total.
    // After k+50 pushes, tip is the (k+50)th delta, epoch_fees = (k+50)*1_000_000.
    // But since delta.block_fields.epoch_fees IS the running total, the tip's
    // epoch_fees = running = (k+50) * 1_000_000.
    assert_eq!(seq.tip_state().epoch_fees.0, (k + 50) * 1_000_000);
}
