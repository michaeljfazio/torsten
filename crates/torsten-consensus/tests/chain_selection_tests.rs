//! Integration tests for the Subsystem 3 chain selection API.
//!
//! Tests cover:
//! - `chain_preference` with longer / shorter / equal-length chains
//! - Equal-length VRF tiebreaker (different pools, lower VRF wins)
//! - Equal-length opcert tiebreaker (same pool, higher opcert wins)
//! - Conway slot-window restriction (VRF comparison suppressed when too far apart)
//! - `maximal_candidates` with a simple in-memory `SuccessorProvider`

use std::collections::HashMap;

use torsten_consensus::chain_fragment::ChainFragment;
use torsten_consensus::chain_selection::{
    chain_preference, maximal_candidates, ChainPreference, SuccessorProvider,
};
use torsten_primitives::block::{BlockHeader, OperationalCert, Point, ProtocolVersion, VrfOutput};
use torsten_primitives::hash::{BlockHeaderHash, Hash32};
use torsten_primitives::time::{BlockNo, SlotNo};

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Parameters for building a minimal `BlockHeader` for chain selection tests.
///
/// The fields that matter for chain selection:
/// - `hash_byte`, `prev_byte`, `slot`, `block_no` — chain structure
/// - `pool_byte` — used to identify the pool (blake2b-224 of `[pool_byte; 32]`)
/// - `opcert_seq` — opcert counter for same-pool tiebreak
/// - `vrf_output` — VRF tiebreak for cross-pool selection
/// - `proto_major` — determines era (Conway = 9+)
struct HeaderParams {
    slot: u64,
    block_no: u64,
    hash_byte: u8,
    prev_byte: u8,
    pool_byte: u8,
    opcert_seq: u64,
    vrf_output: Vec<u8>,
    proto_major: u64,
}

impl HeaderParams {
    fn build(self) -> BlockHeader {
        BlockHeader {
            header_hash: BlockHeaderHash::from_bytes([self.hash_byte; 32]),
            prev_hash: BlockHeaderHash::from_bytes([self.prev_byte; 32]),
            // Use a 32-byte pool key so blake2b-224([pool_byte; 32]) is our pool ID.
            issuer_vkey: vec![self.pool_byte; 32],
            vrf_vkey: vec![],
            vrf_result: VrfOutput {
                output: self.vrf_output,
                proof: vec![],
            },
            block_number: BlockNo(self.block_no),
            slot: SlotNo(self.slot),
            epoch_nonce: Default::default(),
            body_size: 0,
            body_hash: Default::default(),
            operational_cert: OperationalCert {
                hot_vkey: vec![],
                sequence_number: self.opcert_seq,
                kes_period: 0,
                sigma: vec![],
            },
            protocol_version: ProtocolVersion {
                major: self.proto_major,
                minor: 0,
            },
            kes_signature: vec![],
            nonce_vrf_output: vec![],
            nonce_vrf_proof: vec![],
        }
    }
}

/// Build a Conway (proto_major=9) header with full control over tiebreak fields.
fn make_conway_header(
    slot: u64,
    block_no: u64,
    hash_byte: u8,
    prev_byte: u8,
    pool_byte: u8,
    opcert_seq: u64,
    vrf_output: Vec<u8>,
) -> BlockHeader {
    HeaderParams {
        slot,
        block_no,
        hash_byte,
        prev_byte,
        pool_byte,
        opcert_seq,
        vrf_output,
        proto_major: 9,
    }
    .build()
}

/// Build a `Point::Specific` from a slot and a fill byte.
fn pt(slot: u64, byte: u8) -> Point {
    Point::Specific(SlotNo(slot), BlockHeaderHash::from_bytes([byte; 32]))
}

/// Build a `ChainFragment` anchored at the origin with the given headers.
fn fragment_from(headers: Vec<BlockHeader>) -> ChainFragment {
    ChainFragment::from_headers(Point::Origin, headers)
}

// ---------------------------------------------------------------------------
// chain_preference: longer chain
// ---------------------------------------------------------------------------

#[test]
fn longer_chain_is_preferred() {
    // current: 2 blocks; candidate: 3 blocks — candidate wins.
    let current = fragment_from(vec![
        make_conway_header(100, 1, 0x01, 0x00, 0xA0, 0, vec![0xFF; 32]),
        make_conway_header(101, 2, 0x02, 0x01, 0xA0, 0, vec![0xFF; 32]),
    ]);
    let candidate = fragment_from(vec![
        make_conway_header(100, 1, 0x01, 0x00, 0xB0, 0, vec![0xAA; 32]),
        make_conway_header(101, 2, 0x02, 0x01, 0xB0, 0, vec![0xAA; 32]),
        make_conway_header(102, 3, 0x03, 0x02, 0xB0, 0, vec![0xAA; 32]),
    ]);

    assert_eq!(
        chain_preference(&current, &candidate, u64::MAX),
        ChainPreference::PreferCandidate,
        "longer chain (3 blocks) must beat shorter chain (2 blocks)"
    );
}

#[test]
fn shorter_chain_is_not_preferred() {
    // current: 3 blocks; candidate: 2 blocks — current wins.
    let current = fragment_from(vec![
        make_conway_header(100, 1, 0x01, 0x00, 0xA0, 0, vec![0xAA; 32]),
        make_conway_header(101, 2, 0x02, 0x01, 0xA0, 0, vec![0xAA; 32]),
        make_conway_header(102, 3, 0x03, 0x02, 0xA0, 0, vec![0xAA; 32]),
    ]);
    let candidate = fragment_from(vec![
        make_conway_header(100, 1, 0x01, 0x00, 0xB0, 0, vec![0xFF; 32]),
        make_conway_header(101, 2, 0x02, 0x01, 0xB0, 0, vec![0xFF; 32]),
    ]);

    assert_eq!(
        chain_preference(&current, &candidate, u64::MAX),
        ChainPreference::PreferCurrent,
        "shorter candidate must not displace a longer current chain"
    );
}

#[test]
fn equal_length_empty_chains_are_equal() {
    let current = fragment_from(vec![]);
    let candidate = fragment_from(vec![]);
    assert_eq!(
        chain_preference(&current, &candidate, u64::MAX),
        ChainPreference::Equal
    );
}

// ---------------------------------------------------------------------------
// chain_preference: VRF tiebreaker (different pools)
// ---------------------------------------------------------------------------

#[test]
fn equal_length_lower_vrf_wins_cross_pool() {
    // Same block number, different pools.
    // VRF[0x00..] < VRF[0xFF..] — candidate has lower VRF, wins.
    let current = fragment_from(vec![make_conway_header(
        100,
        1,
        0x01,
        0x00,
        0xA0,
        0,
        vec![0xFF; 32], // higher VRF → current loses tiebreak
    )]);
    let candidate = fragment_from(vec![make_conway_header(
        100,
        1,
        0x02,
        0x00,
        0xB0,
        0,
        vec![0x00; 32], // lower VRF → candidate wins
    )]);

    assert_eq!(
        chain_preference(&current, &candidate, u64::MAX),
        ChainPreference::PreferCandidate,
        "cross-pool tiebreak: lower VRF output must win"
    );
}

#[test]
fn equal_length_higher_vrf_current_wins_cross_pool() {
    // Candidate has higher VRF — current chain is preferred.
    let current = fragment_from(vec![make_conway_header(
        100,
        1,
        0x01,
        0x00,
        0xA0,
        0,
        vec![0x00; 32], // lower VRF → current keeps the chain
    )]);
    let candidate = fragment_from(vec![make_conway_header(
        100,
        1,
        0x02,
        0x00,
        0xB0,
        0,
        vec![0xFF; 32], // higher VRF → candidate loses tiebreak
    )]);

    assert_eq!(
        chain_preference(&current, &candidate, u64::MAX),
        ChainPreference::PreferCurrent,
        "cross-pool tiebreak: higher VRF output must lose"
    );
}

// ---------------------------------------------------------------------------
// chain_preference: opcert tiebreaker (same pool)
// ---------------------------------------------------------------------------

#[test]
fn equal_length_higher_opcert_wins_same_pool() {
    // Same pool (pool_byte = 0xA0), different opcert counters.
    // Candidate has counter 2 > current counter 1 → candidate wins.
    let current = fragment_from(vec![make_conway_header(
        100,
        1,
        0x01,
        0x00,
        0xA0, // same pool as candidate
        1,    // opcert seq = 1
        vec![0xFF; 32],
    )]);
    let candidate = fragment_from(vec![make_conway_header(
        100,
        1,
        0x02,
        0x00,
        0xA0, // same pool as current
        2,    // opcert seq = 2 → wins
        vec![0xFF; 32],
    )]);

    assert_eq!(
        chain_preference(&current, &candidate, u64::MAX),
        ChainPreference::PreferCandidate,
        "same-pool tiebreak: higher opcert counter must win"
    );
}

#[test]
fn equal_length_lower_opcert_loses_same_pool() {
    // Same pool; candidate has lower counter — current keeps the chain.
    let current = fragment_from(vec![make_conway_header(
        100,
        1,
        0x01,
        0x00,
        0xA0,
        5,
        vec![0xAA; 32],
    )]);
    let candidate = fragment_from(vec![make_conway_header(
        100,
        1,
        0x02,
        0x00,
        0xA0,
        3, // lower counter — loses
        vec![0xAA; 32],
    )]);

    assert_eq!(
        chain_preference(&current, &candidate, u64::MAX),
        ChainPreference::PreferCurrent,
        "same-pool tiebreak: lower opcert counter must lose"
    );
}

#[test]
fn equal_length_equal_opcert_same_pool_is_equal() {
    // Same pool, same counter, same VRF — identical blocks seen twice.
    let header = make_conway_header(100, 1, 0x01, 0x00, 0xA0, 7, vec![0x55; 32]);
    let current = fragment_from(vec![header.clone()]);
    let candidate = fragment_from(vec![header]);

    assert_eq!(
        chain_preference(&current, &candidate, u64::MAX),
        ChainPreference::Equal,
        "same block seen from two paths should be Equal"
    );
}

// ---------------------------------------------------------------------------
// chain_preference: Conway slot-window restriction
// ---------------------------------------------------------------------------

#[test]
fn conway_vrf_comparison_suppressed_outside_window() {
    // In Conway (proto >= 9), if slot difference > slot_window the current
    // chain is preferred even if the candidate has a lower VRF output.
    //
    // current tip at slot 1000, candidate tip at slot 3000.
    // slot_window = 1000 → |3000 - 1000| = 2000 > 1000 → VRF suppressed.
    let current = fragment_from(vec![make_conway_header(
        1000,
        1,
        0x01,
        0x00,
        0xA0,
        0,
        vec![0xFF; 32], // high VRF
    )]);
    let candidate = fragment_from(vec![make_conway_header(
        3000,
        1,
        0x02,
        0x00,
        0xB0,
        0,
        vec![0x00; 32], // low VRF — but too far away
    )]);

    let pref = chain_preference(&current, &candidate, 1000);
    assert_eq!(
        pref,
        ChainPreference::PreferCurrent,
        "Conway: VRF comparison should be suppressed when slot gap > window"
    );
}

#[test]
fn conway_vrf_comparison_applied_within_window() {
    // slot_window = 5000, slot diff = 2000 — within window, VRF applies.
    let current = fragment_from(vec![make_conway_header(
        1000,
        1,
        0x01,
        0x00,
        0xA0,
        0,
        vec![0xFF; 32], // high VRF
    )]);
    let candidate = fragment_from(vec![make_conway_header(
        3000,
        1,
        0x02,
        0x00,
        0xB0,
        0,
        vec![0x00; 32], // low VRF → wins
    )]);

    let pref = chain_preference(&current, &candidate, 5000);
    assert_eq!(
        pref,
        ChainPreference::PreferCandidate,
        "Conway: VRF comparison should apply when slot gap <= window"
    );
}

// ---------------------------------------------------------------------------
// maximal_candidates: in-memory SuccessorProvider
// ---------------------------------------------------------------------------

/// A simple in-memory implementation of `SuccessorProvider` for testing.
///
/// Maps each block hash to (header, successors).  This simulates the
/// VolatileDB successor index without any storage dependency.
struct TestVolatile {
    /// All known headers, keyed by their `header_hash`.
    headers: HashMap<Hash32, BlockHeader>,
    /// Successor index: parent_hash → [child_hash, …]
    successors: HashMap<Hash32, Vec<Hash32>>,
}

impl TestVolatile {
    fn new() -> Self {
        TestVolatile {
            headers: HashMap::new(),
            successors: HashMap::new(),
        }
    }

    /// Add a block.  Automatically registers it in the successor index.
    fn add(&mut self, header: BlockHeader) {
        let hash = header.header_hash;
        let prev = header.prev_hash;
        self.headers.insert(hash, header);
        self.successors.entry(prev).or_default().push(hash);
        // Ensure the block itself appears in the successors map (as a key
        // with empty vec) if not already there, so we can detect it as a leaf.
        self.successors.entry(hash).or_default();
    }
}

impl SuccessorProvider for TestVolatile {
    fn successors_of(&self, parent_hash: &Hash32) -> Vec<Hash32> {
        self.successors
            .get(parent_hash)
            .cloned()
            .unwrap_or_default()
    }

    fn header_of(&self, hash: &Hash32) -> Option<BlockHeader> {
        self.headers.get(hash).cloned()
    }
}

/// Convenience: make a header where pool / vrf / opcert don't matter.
fn simple_header(slot: u64, block_no: u64, hash_byte: u8, prev_byte: u8) -> BlockHeader {
    make_conway_header(
        slot,
        block_no,
        hash_byte,
        prev_byte,
        0xAA,
        0,
        vec![0x00; 32],
    )
}

// ─────────────────────────────────────────────────────────────────────────────
// Test: linear chain — one candidate
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn maximal_candidates_linear_chain_one_result() {
    // ImmutableDB tip: slot 100 / hash 0xAA
    // VolatileDB: 0xBB → 0xCC (linear, no forks)
    let anchor_hash = BlockHeaderHash::from_bytes([0xAA; 32]);
    let anchor = pt(100, 0xAA);

    let mut vol = TestVolatile::new();
    // 0xBB extends anchor (prev = 0xAA)
    vol.add(simple_header(101, 1, 0xBB, 0xAA));
    // 0xCC extends 0xBB
    vol.add(simple_header(102, 2, 0xCC, 0xBB));

    let new_block = BlockHeaderHash::from_bytes([0xCC; 32]);
    let candidates = maximal_candidates(&anchor, &vol, &new_block);

    assert_eq!(
        candidates.len(),
        1,
        "linear chain must produce exactly one candidate"
    );
    let frag = &candidates[0];
    assert_eq!(
        frag.length(),
        2,
        "fragment must contain both volatile blocks"
    );
    assert_eq!(frag.tip(), pt(102, 0xCC));
    let _ = anchor_hash; // used above
}

// ─────────────────────────────────────────────────────────────────────────────
// Test: fork after new block — two candidates
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn maximal_candidates_fork_after_new_block_two_results() {
    // Anchor: slot 100 / 0xAA
    // VolatileDB:
    //   0xBB (slot 101) — the NEW block
    //   0xCC (slot 102) — extends 0xBB (fork A)
    //   0xDD (slot 102) — extends 0xBB (fork B, different hash same slot)
    let anchor = pt(100, 0xAA);

    let mut vol = TestVolatile::new();
    vol.add(simple_header(101, 1, 0xBB, 0xAA)); // new block
    vol.add(simple_header(102, 2, 0xCC, 0xBB)); // fork A
    vol.add(simple_header(102, 2, 0xDD, 0xBB)); // fork B

    let new_block = BlockHeaderHash::from_bytes([0xBB; 32]);
    let candidates = maximal_candidates(&anchor, &vol, &new_block);

    assert_eq!(
        candidates.len(),
        2,
        "fork with two children must produce two candidates"
    );

    // Both candidates should start from the same anchor and include 0xBB.
    for frag in &candidates {
        assert_eq!(frag.length(), 2, "each candidate has 2 volatile headers");
        assert_eq!(
            frag.anchor(),
            &anchor,
            "all candidates anchored at immutable tip"
        );
    }

    // Collect tip hashes to verify we get both forks.
    let tips: std::collections::HashSet<_> = candidates.iter().map(|f| f.tip()).collect();
    assert!(tips.contains(&pt(102, 0xCC)), "fork A tip must appear");
    assert!(tips.contains(&pt(102, 0xDD)), "fork B tip must appear");
}

// ─────────────────────────────────────────────────────────────────────────────
// Test: new block already has a successor in VolatileDB
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn maximal_candidates_new_block_not_at_leaf() {
    // Anchor: slot 100 / 0xAA
    // VolatileDB:
    //   0xBB (slot 101) — existing block
    //   0xCC (slot 102) — new block (its successor 0xDD is already stored)
    //   0xDD (slot 103) — successor of new block, already in DB
    let anchor = pt(100, 0xAA);

    let mut vol = TestVolatile::new();
    vol.add(simple_header(101, 1, 0xBB, 0xAA));
    vol.add(simple_header(102, 2, 0xCC, 0xBB)); // NEW block
    vol.add(simple_header(103, 3, 0xDD, 0xCC)); // already present

    let new_block = BlockHeaderHash::from_bytes([0xCC; 32]);
    let candidates = maximal_candidates(&anchor, &vol, &new_block);

    // There is one maximal path: 0xBB → 0xCC → 0xDD
    assert_eq!(candidates.len(), 1);
    let frag = &candidates[0];
    assert_eq!(frag.length(), 3);
    assert_eq!(frag.tip(), pt(103, 0xDD));
}

// ─────────────────────────────────────────────────────────────────────────────
// Test: disconnected block — returns empty
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn maximal_candidates_disconnected_block_returns_empty() {
    // new_block_hash points at a block whose prev_hash is not in VolatileDB
    // and is not the immutable tip.
    let anchor = pt(100, 0xAA); // anchor hash = 0xAA

    let mut vol = TestVolatile::new();
    // 0xBB extends some unknown block 0x99 — disconnected from anchor.
    vol.add(simple_header(101, 1, 0xBB, 0x99));

    let new_block = BlockHeaderHash::from_bytes([0xBB; 32]);
    let candidates = maximal_candidates(&anchor, &vol, &new_block);

    assert!(
        candidates.is_empty(),
        "a disconnected block must produce no candidates"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Test: single new block extending immutable tip (no predecessors in VolatileDB)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn maximal_candidates_single_block_extending_anchor() {
    // Anchor: slot 100 / 0xAA
    // VolatileDB: only one block 0xBB extending the anchor.
    let anchor = pt(100, 0xAA);

    let mut vol = TestVolatile::new();
    vol.add(simple_header(101, 1, 0xBB, 0xAA));

    let new_block = BlockHeaderHash::from_bytes([0xBB; 32]);
    let candidates = maximal_candidates(&anchor, &vol, &new_block);

    assert_eq!(
        candidates.len(),
        1,
        "single-block chain must produce one candidate"
    );
    let frag = &candidates[0];
    assert_eq!(frag.length(), 1);
    assert_eq!(frag.tip(), pt(101, 0xBB));
    assert_eq!(frag.anchor(), &anchor);
}

// ─────────────────────────────────────────────────────────────────────────────
// Test: prefer best among multiple candidates
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn chain_preference_picks_best_from_multiple_candidates() {
    // Simulate the pattern: addBlock produces N candidates; we pick the best.
    //
    // Chains:
    //   A: 2 blocks (shorter)
    //   B: 3 blocks (longer) — pool 0xB0, VRF 0xFF
    //   C: 3 blocks (longer) — pool 0xC0, VRF 0x00 (lower VRF)
    //
    // C should beat B in the VRF tiebreak.
    let chain_a = fragment_from(vec![
        simple_header(101, 1, 0x01, 0x00),
        simple_header(102, 2, 0x02, 0x01),
    ]);
    let chain_b = fragment_from(vec![
        simple_header(101, 1, 0x01, 0x00),
        simple_header(102, 2, 0x02, 0x01),
        make_conway_header(103, 3, 0x03, 0x02, 0xB0, 0, vec![0xFF; 32]),
    ]);
    let chain_c = fragment_from(vec![
        simple_header(101, 1, 0x01, 0x00),
        simple_header(102, 2, 0x02, 0x01),
        make_conway_header(103, 3, 0x04, 0x02, 0xC0, 0, vec![0x00; 32]),
    ]);

    // Start with chain_a as the current selection.
    let mut best = chain_a;

    // Evaluate chain_b.
    if chain_preference(&best, &chain_b, u64::MAX) == ChainPreference::PreferCandidate {
        best = chain_b;
    }

    // Evaluate chain_c (should beat chain_b via VRF).
    if chain_preference(&best, &chain_c, u64::MAX) == ChainPreference::PreferCandidate {
        best = chain_c;
    }

    // The winner should have VRF 0x00 (chain_c).
    let tip_header = best.headers().back().expect("must have a tip header");
    assert_eq!(
        tip_header.vrf_result.output,
        vec![0x00u8; 32],
        "chain C (lower VRF) must be the final winner"
    );
}
