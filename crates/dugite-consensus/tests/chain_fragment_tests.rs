//! Integration tests for [`ChainFragment`].
//!
//! These tests exercise the public API of `ChainFragment` from outside the
//! crate, mirroring how it will be used by the storage and node layers.

use dugite_consensus::chain_fragment::ChainFragment;
use dugite_primitives::block::{BlockHeader, OperationalCert, Point, ProtocolVersion, VrfOutput};
use dugite_primitives::hash::BlockHeaderHash;
use dugite_primitives::time::{BlockNo, SlotNo};

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Build a minimal `BlockHeader` for testing.
/// Only the chain-structure fields (slot, block_number, header_hash, prev_hash)
/// carry meaningful values; everything else is zeroed.
fn make_header(slot: u64, block_no: u64, hash_byte: u8, prev_byte: u8) -> BlockHeader {
    BlockHeader {
        header_hash: BlockHeaderHash::from_bytes([hash_byte; 32]),
        prev_hash: BlockHeaderHash::from_bytes([prev_byte; 32]),
        issuer_vkey: vec![],
        vrf_vkey: vec![],
        vrf_result: VrfOutput {
            output: vec![],
            proof: vec![],
        },
        block_number: BlockNo(block_no),
        slot: SlotNo(slot),
        epoch_nonce: Default::default(),
        body_size: 0,
        body_hash: Default::default(),
        operational_cert: OperationalCert {
            hot_vkey: vec![],
            sequence_number: 0,
            kes_period: 0,
            sigma: vec![],
        },
        protocol_version: ProtocolVersion { major: 9, minor: 0 },
        kes_signature: vec![],
        nonce_vrf_output: vec![],
        nonce_vrf_proof: vec![],
    }
}

/// Convenience: build a `Point::Specific` from a slot and a single fill byte.
fn pt(slot: u64, byte: u8) -> Point {
    Point::Specific(SlotNo(slot), BlockHeaderHash::from_bytes([byte; 32]))
}

// ---------------------------------------------------------------------------
// Empty fragment
// ---------------------------------------------------------------------------

#[test]
fn empty_fragment_tip_equals_anchor() {
    let anchor = pt(1000, 0xAA);
    let frag = ChainFragment::new(anchor.clone());

    assert_eq!(frag.tip(), anchor);
    assert_eq!(frag.length(), 0);
    assert!(frag.is_empty());
    assert_eq!(frag.tip_block_no(), BlockNo(0));
}

#[test]
fn empty_fragment_at_origin() {
    let frag = ChainFragment::new(Point::Origin);

    assert_eq!(frag.tip(), Point::Origin);
    assert!(frag.is_empty());
    assert_eq!(frag.tip_block_no(), BlockNo(0));
}

// ---------------------------------------------------------------------------
// Push / length / tip_block_no
// ---------------------------------------------------------------------------

#[test]
fn push_single_header() {
    let anchor = pt(100, 0xAA);
    let mut frag = ChainFragment::new(anchor.clone());

    frag.push(make_header(101, 1, 0xBB, 0xAA));

    assert_eq!(frag.length(), 1);
    assert_eq!(frag.tip(), pt(101, 0xBB));
    assert_eq!(frag.tip_block_no(), BlockNo(1));
    assert!(!frag.is_empty());
}

#[test]
fn push_multiple_headers_monotone() {
    let anchor = pt(200, 0x01);
    let mut frag = ChainFragment::new(anchor);

    for i in 1u64..=10 {
        let slot = 200 + i;
        let hash_byte = (i & 0xFF) as u8;
        let prev_byte = ((i - 1) & 0xFF) as u8;
        frag.push(make_header(slot, i, hash_byte, prev_byte));
    }

    assert_eq!(frag.length(), 10);
    assert_eq!(frag.tip(), pt(210, 10));
    assert_eq!(frag.tip_block_no(), BlockNo(10));
}

#[test]
fn tip_block_no_for_various_block_numbers() {
    let anchor = pt(0, 0x00);
    let mut frag = ChainFragment::new(anchor);

    frag.push(make_header(1, 100, 0x01, 0x00));
    assert_eq!(frag.tip_block_no(), BlockNo(100));

    frag.push(make_header(2, 999_999, 0x02, 0x01));
    assert_eq!(frag.tip_block_no(), BlockNo(999_999));
}

// ---------------------------------------------------------------------------
// Rollback
// ---------------------------------------------------------------------------

#[test]
fn rollback_to_anchor_empties_fragment() {
    let anchor = pt(100, 0xAA);
    let mut frag = ChainFragment::new(anchor.clone());
    frag.push(make_header(101, 1, 0xBB, 0xAA));
    frag.push(make_header(102, 2, 0xCC, 0xBB));
    frag.push(make_header(103, 3, 0xDD, 0xCC));

    let ok = frag.rollback_to(&anchor);

    assert!(ok, "rollback to anchor must succeed");
    assert!(frag.is_empty());
    assert_eq!(frag.tip(), anchor);
    assert_eq!(frag.length(), 0);
}

#[test]
fn rollback_to_middle_of_fragment() {
    let anchor = pt(100, 0xAA);
    let mut frag = ChainFragment::new(anchor.clone());
    frag.push(make_header(101, 1, 0xBB, 0xAA));
    frag.push(make_header(102, 2, 0xCC, 0xBB));
    frag.push(make_header(103, 3, 0xDD, 0xCC));
    frag.push(make_header(104, 4, 0xEE, 0xDD));

    let ok = frag.rollback_to(&pt(102, 0xCC));

    assert!(ok);
    assert_eq!(frag.length(), 2);
    assert_eq!(frag.tip(), pt(102, 0xCC));
    assert_eq!(frag.tip_block_no(), BlockNo(2));
}

#[test]
fn rollback_to_oldest_header() {
    let anchor = pt(100, 0xAA);
    let mut frag = ChainFragment::new(anchor.clone());
    frag.push(make_header(101, 1, 0xBB, 0xAA));
    frag.push(make_header(102, 2, 0xCC, 0xBB));

    let ok = frag.rollback_to(&pt(101, 0xBB));

    assert!(ok);
    assert_eq!(frag.length(), 1);
    assert_eq!(frag.tip(), pt(101, 0xBB));
}

#[test]
fn rollback_to_unknown_point_leaves_fragment_unchanged() {
    let anchor = pt(100, 0xAA);
    let mut frag = ChainFragment::new(anchor.clone());
    frag.push(make_header(101, 1, 0xBB, 0xAA));
    frag.push(make_header(102, 2, 0xCC, 0xBB));

    let original_len = frag.length();
    let original_tip = frag.tip();

    let ok = frag.rollback_to(&pt(999, 0xFF));

    assert!(!ok, "rollback to unknown point must return false");
    assert_eq!(frag.length(), original_len);
    assert_eq!(frag.tip(), original_tip);
}

#[test]
fn rollback_to_current_tip_is_no_op() {
    let anchor = pt(100, 0xAA);
    let mut frag = ChainFragment::new(anchor.clone());
    frag.push(make_header(101, 1, 0xBB, 0xAA));

    let ok = frag.rollback_to(&pt(101, 0xBB));

    assert!(ok);
    assert_eq!(frag.length(), 1);
    assert_eq!(frag.tip(), pt(101, 0xBB));
}

#[test]
fn rollback_on_empty_fragment_to_anchor_succeeds() {
    let anchor = pt(100, 0xAA);
    let mut frag = ChainFragment::new(anchor.clone());

    let ok = frag.rollback_to(&anchor);

    assert!(ok);
    assert!(frag.is_empty());
}

#[test]
fn rollback_on_empty_fragment_to_wrong_point_fails() {
    let anchor = pt(100, 0xAA);
    let mut frag = ChainFragment::new(anchor.clone());

    let ok = frag.rollback_to(&pt(50, 0x55));

    assert!(!ok);
    assert!(frag.is_empty());
}

// ---------------------------------------------------------------------------
// find_intersect
// ---------------------------------------------------------------------------

#[test]
fn find_intersect_with_anchor_in_list() {
    let anchor = pt(100, 0xAA);
    let frag = ChainFragment::new(anchor.clone());

    let result = frag.find_intersect(std::slice::from_ref(&anchor));
    assert_eq!(result, Some(anchor));
}

#[test]
fn find_intersect_finds_tip() {
    let anchor = pt(100, 0xAA);
    let mut frag = ChainFragment::new(anchor.clone());
    frag.push(make_header(101, 1, 0xBB, 0xAA));
    frag.push(make_header(102, 2, 0xCC, 0xBB));

    let result = frag.find_intersect(&[pt(102, 0xCC)]);
    assert_eq!(result, Some(pt(102, 0xCC)));
}

#[test]
fn find_intersect_returns_first_match_in_input_order() {
    let anchor = pt(100, 0xAA);
    let mut frag = ChainFragment::new(anchor.clone());
    frag.push(make_header(101, 1, 0xBB, 0xAA));
    frag.push(make_header(102, 2, 0xCC, 0xBB));
    frag.push(make_header(103, 3, 0xDD, 0xCC));

    // Both 103 and 101 exist; 103 is listed first — must get 103 back.
    let result = frag.find_intersect(&[pt(103, 0xDD), pt(101, 0xBB)]);
    assert_eq!(result, Some(pt(103, 0xDD)));

    // Reverse order: 101 listed first — must get 101 back.
    let result2 = frag.find_intersect(&[pt(101, 0xBB), pt(103, 0xDD)]);
    assert_eq!(result2, Some(pt(101, 0xBB)));
}

#[test]
fn find_intersect_no_match_returns_none() {
    let anchor = pt(100, 0xAA);
    let mut frag = ChainFragment::new(anchor.clone());
    frag.push(make_header(101, 1, 0xBB, 0xAA));

    let result = frag.find_intersect(&[pt(999, 0xFF), pt(888, 0xEE)]);
    assert_eq!(result, None);
}

#[test]
fn find_intersect_empty_input_returns_none() {
    let anchor = pt(100, 0xAA);
    let mut frag = ChainFragment::new(anchor.clone());
    frag.push(make_header(101, 1, 0xBB, 0xAA));

    let result = frag.find_intersect(&[]);
    assert_eq!(result, None);
}

#[test]
fn find_intersect_includes_anchor_point() {
    let anchor = pt(100, 0xAA);
    let mut frag = ChainFragment::new(anchor.clone());
    frag.push(make_header(101, 1, 0xBB, 0xAA));
    frag.push(make_header(102, 2, 0xCC, 0xBB));

    // The anchor itself should be findable.
    let result = frag.find_intersect(&[pt(999, 0xFF), anchor.clone()]);
    assert_eq!(result, Some(anchor));
}

#[test]
fn find_intersect_origin_on_origin_anchored_fragment() {
    let frag = ChainFragment::new(Point::Origin);
    let result = frag.find_intersect(&[Point::Origin]);
    assert_eq!(result, Some(Point::Origin));
}

#[test]
fn find_intersect_origin_not_in_non_origin_fragment() {
    // Fragment anchored at a specific point — Origin should not match.
    let anchor = pt(100, 0xAA);
    let frag = ChainFragment::new(anchor.clone());

    let result = frag.find_intersect(&[Point::Origin]);
    assert_eq!(result, None);
}

// ---------------------------------------------------------------------------
// Slot/hash disambiguation — same slot, different hash should NOT match
// ---------------------------------------------------------------------------

#[test]
fn same_slot_different_hash_does_not_match() {
    let anchor = pt(100, 0xAA);
    let mut frag = ChainFragment::new(anchor.clone());
    // Header at slot 101 with hash byte 0xBB.
    frag.push(make_header(101, 1, 0xBB, 0xAA));

    // Look for slot 101 but with a DIFFERENT hash byte — must not match.
    let result = frag.find_intersect(&[pt(101, 0xCC)]);
    assert_eq!(result, None);
}

#[test]
fn rollback_slot_match_requires_hash_match() {
    let anchor = pt(100, 0xAA);
    let mut frag = ChainFragment::new(anchor.clone());
    frag.push(make_header(101, 1, 0xBB, 0xAA));
    frag.push(make_header(102, 2, 0xCC, 0xBB));

    // Slot 102 exists but wrong hash — rollback must fail.
    let ok = frag.rollback_to(&pt(102, 0xFF));
    assert!(!ok);
    assert_eq!(frag.length(), 2);
}

// ---------------------------------------------------------------------------
// Advance anchor
// ---------------------------------------------------------------------------

#[test]
fn advance_anchor_removes_old_headers() {
    let anchor = pt(100, 0xAA);
    let mut frag = ChainFragment::new(anchor.clone());
    frag.push(make_header(101, 1, 0xBB, 0xAA));
    frag.push(make_header(102, 2, 0xCC, 0xBB));
    frag.push(make_header(103, 3, 0xDD, 0xCC));
    frag.push(make_header(104, 4, 0xEE, 0xDD));

    // Advance anchor to the block at slot 102.
    let removed = frag.advance_anchor(pt(102, 0xCC));

    assert_eq!(removed, 2, "headers at slots 101 and 102 should be removed");
    assert_eq!(frag.anchor(), &pt(102, 0xCC));
    assert_eq!(frag.length(), 2, "headers at slots 103 and 104 remain");
    assert_eq!(frag.tip(), pt(104, 0xEE));
}

#[test]
fn advance_anchor_all_headers_removed() {
    let anchor = pt(100, 0xAA);
    let mut frag = ChainFragment::new(anchor.clone());
    frag.push(make_header(101, 1, 0xBB, 0xAA));
    frag.push(make_header(102, 2, 0xCC, 0xBB));

    // Advance past everything.
    let removed = frag.advance_anchor(pt(200, 0xDD));
    assert_eq!(removed, 2);
    assert!(frag.is_empty());
    assert_eq!(frag.anchor(), &pt(200, 0xDD));
    // Tip should now be the new anchor since fragment is empty.
    assert_eq!(frag.tip(), pt(200, 0xDD));
}

// ---------------------------------------------------------------------------
// from_headers constructor
// ---------------------------------------------------------------------------

#[test]
fn from_headers_constructor() {
    let anchor = pt(100, 0xAA);
    let headers = vec![
        make_header(101, 1, 0xBB, 0xAA),
        make_header(102, 2, 0xCC, 0xBB),
    ];
    let frag = ChainFragment::from_headers(anchor.clone(), headers);

    assert_eq!(frag.length(), 2);
    assert_eq!(frag.anchor(), &anchor);
    assert_eq!(frag.tip(), pt(102, 0xCC));
}

// ---------------------------------------------------------------------------
// Display
// ---------------------------------------------------------------------------

#[test]
fn display_format_is_sensible() {
    let anchor = pt(100, 0xAA);
    let mut frag = ChainFragment::new(anchor.clone());
    frag.push(make_header(101, 1, 0xBB, 0xAA));

    let s = frag.to_string();
    assert!(s.contains("ChainFragment"));
    assert!(s.contains("length: 1"));
}
