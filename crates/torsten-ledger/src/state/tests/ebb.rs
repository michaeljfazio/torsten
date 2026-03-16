//! Tests for Byron Epoch Boundary Block (EBB) handling.
//!
//! EBBs carry no transactions. Their sole purpose is to provide a
//! `prev_hash` anchor so the next real Byron block can maintain the hash
//! chain across epoch boundaries.

#[allow(unused_imports)]
use super::super::*;
use super::*;
use torsten_primitives::block::{Point, Tip};
use torsten_primitives::era::Era;
use torsten_primitives::time::{BlockNo, EpochNo, SlotNo};

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Create a `LedgerState` positioned at the Byron era.
fn make_byron_ledger() -> LedgerState {
    let mut state = make_ledger();
    state.era = Era::Byron;
    state.tip = Tip {
        point: Point::Specific(SlotNo(100), make_hash32(1)),
        block_number: BlockNo(5),
    };
    state
}

// ─────────────────────────────────────────────────────────────────────────────
// advance_past_ebb — happy path
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_advance_past_ebb_updates_tip_hash() {
    let mut state = make_byron_ledger();
    let ebb_hash = make_hash32(42);

    state
        .advance_past_ebb(ebb_hash)
        .expect("advance_past_ebb should succeed");

    assert_eq!(
        state.tip.point.hash(),
        Some(&ebb_hash),
        "Tip hash should be updated to the EBB hash"
    );
}

#[test]
fn test_advance_past_ebb_preserves_slot() {
    let mut state = make_byron_ledger();
    let original_slot = state.tip.point.slot().unwrap();
    let ebb_hash = make_hash32(43);

    state
        .advance_past_ebb(ebb_hash)
        .expect("advance_past_ebb should succeed");

    assert_eq!(
        state.tip.point.slot(),
        Some(original_slot),
        "Slot should be preserved across EBB (EBBs do not occupy slots)"
    );
}

#[test]
fn test_advance_past_ebb_preserves_block_number() {
    let mut state = make_byron_ledger();
    let original_block_number = state.tip.block_number;
    let ebb_hash = make_hash32(44);

    state
        .advance_past_ebb(ebb_hash)
        .expect("advance_past_ebb should succeed");

    assert_eq!(
        state.tip.block_number, original_block_number,
        "Block number should not increment for EBBs"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// advance_past_ebb — error cases
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_advance_past_ebb_rejected_in_shelley() {
    let mut state = make_ledger(); // Conway era by default
    let ebb_hash = make_hash32(50);

    let result = state.advance_past_ebb(ebb_hash);

    assert!(
        result.is_err(),
        "advance_past_ebb should fail outside the Byron era"
    );
    match result.unwrap_err() {
        LedgerError::EpochTransition(msg) => {
            assert!(
                msg.contains("non-Byron era"),
                "Error message should mention non-Byron era"
            );
        }
        other => panic!("Unexpected error type: {:?}", other),
    }
}

#[test]
fn test_advance_past_ebb_rejected_in_conway() {
    let mut state = make_ledger();
    state.era = Era::Conway;

    let result = state.advance_past_ebb(make_hash32(51));
    assert!(result.is_err(), "Should fail in Conway era");
}

// ─────────────────────────────────────────────────────────────────────────────
// Chain continuity: tip hash after EBB must match next block's prev_hash
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_ebb_hash_becomes_prev_hash_for_next_block() {
    // After advancing past an EBB, the new tip hash is the EBB hash.
    // The subsequent block's prev_hash must equal this EBB hash for the
    // chain to be considered connected.

    let mut state = make_byron_ledger();
    let ebb_hash = make_hash32(55);

    state.advance_past_ebb(ebb_hash).unwrap();

    // Verify that the tip now holds the EBB hash (which the next block's prev_hash must match).
    assert_eq!(
        state.tip.point.hash(),
        Some(&ebb_hash),
        "After EBB advance, tip hash == EBB hash (next block's prev_hash target)"
    );
}

#[test]
fn test_advance_past_ebb_from_origin() {
    // EBBs can technically be the first block in the chain.
    let mut state = make_ledger();
    state.era = Era::Byron;
    // Start from Origin (no previous block).
    state.tip = Tip {
        point: Point::Origin,
        block_number: BlockNo(0),
    };

    let ebb_hash = make_hash32(56);
    state.advance_past_ebb(ebb_hash).unwrap();

    // Slot should be 0 (from Origin, which has no slot).
    assert_eq!(state.tip.point.slot(), Some(SlotNo(0)));
    assert_eq!(state.tip.point.hash(), Some(&ebb_hash));
}

// ─────────────────────────────────────────────────────────────────────────────
// UTxO set must not change across an EBB
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn test_ebb_does_not_change_utxo_set() {
    let mut state = make_byron_ledger();

    // Insert a UTxO before the EBB.
    let input = add_utxo(&mut state, 5_000_000);
    let utxo_count_before = state.utxo_set.len();

    state.advance_past_ebb(make_hash32(60)).unwrap();

    assert_eq!(
        state.utxo_set.len(),
        utxo_count_before,
        "UTxO set must not change when advancing through an EBB"
    );
    assert!(
        state.utxo_set.lookup(&input).is_some(),
        "Existing UTxOs should still be present after EBB"
    );
}

#[test]
fn test_ebb_does_not_change_epoch() {
    let mut state = make_byron_ledger();
    state.epoch = EpochNo(3);

    state.advance_past_ebb(make_hash32(61)).unwrap();

    assert_eq!(
        state.epoch,
        EpochNo(3),
        "Epoch must not change when advancing through an EBB"
    );
}
