//! Anchored chain fragment — the volatile portion of the current chain.
//!
//! This module provides [`ChainFragment`], which is Torsten's Rust equivalent of
//! Haskell's `AnchoredFragment` from `ouroboros-network`.  It represents the
//! sequence of block headers on the *selected* chain that live in the VolatileDB
//! — i.e., the window of the last k blocks that have not yet been copied to the
//! ImmutableDB.
//!
//! ## Design
//!
//! ```text
//!  anchor (immutable tip)
//!     │
//!     ▼
//!  [H₀, H₁, H₂, … , Hₙ]    ← headers (VecDeque, oldest first)
//!                     ▲
//!                     tip
//! ```
//!
//! The anchor is a [`Point`] representing where the ImmutableDB ends.  Every
//! header in the deque has its `prev_hash` forming an unbroken chain back to
//! the anchor.
//!
//! ## Haskell Reference
//!
//! `ouroboros-network/ouroboros-network-api/src/Ouroboros/Network/AnchoredFragment.hs`
//! — `AnchoredFragment`, `anchorPoint`, `headPoint`, `rollback`, `addBlock`,
//!   `findFirstPoint`.

use std::collections::VecDeque;

use torsten_primitives::block::{BlockHeader, Point};
use torsten_primitives::time::BlockNo;

/// An anchored fragment of block headers representing the volatile portion of
/// the selected chain.
///
/// The anchor is always the immutable tip point.  When the fragment is empty,
/// `tip()` returns the anchor itself.  All headers are stored in chronological
/// order (oldest at index 0, newest at the back).
///
/// # Invariants
///
/// - If the fragment is non-empty, `headers[0].prev_hash` hashes to the
///   anchor's block header hash (i.e., the first header extends the anchor).
/// - Consecutive headers satisfy `headers[i+1].prev_hash == headers[i].header_hash`.
/// - Slot numbers are strictly increasing across all headers.
/// - Block numbers are strictly increasing across all headers.
///
/// These invariants are NOT enforced by the data structure itself — it is the
/// caller's responsibility to maintain them.  Enforcement happens in the header
/// validation layer (`validate_header_full`).
#[derive(Debug, Clone)]
pub struct ChainFragment {
    /// The anchor point — corresponds to the current immutable tip.
    /// When the fragment is empty this IS the effective tip of the chain.
    anchor: Point,
    /// Block headers in chronological order (index 0 = oldest).
    headers: VecDeque<BlockHeader>,
}

impl ChainFragment {
    // -----------------------------------------------------------------------
    // Constructors
    // -----------------------------------------------------------------------

    /// Create a new, empty fragment anchored at `anchor`.
    ///
    /// An empty fragment means the volatile window contains no blocks — the
    /// chain tip is identical to the immutable tip (`anchor`).
    pub fn new(anchor: Point) -> Self {
        Self {
            anchor,
            headers: VecDeque::new(),
        }
    }

    /// Create a fragment pre-populated with `headers`, anchored at `anchor`.
    ///
    /// The caller is responsible for ensuring the headers form a valid chain
    /// extending from `anchor`.  Headers must be in chronological order
    /// (oldest first).
    pub fn from_headers(anchor: Point, headers: impl IntoIterator<Item = BlockHeader>) -> Self {
        Self {
            anchor,
            headers: headers.into_iter().collect(),
        }
    }

    // -----------------------------------------------------------------------
    // Read accessors
    // -----------------------------------------------------------------------

    /// The anchor point (immutable tip).
    pub fn anchor(&self) -> &Point {
        &self.anchor
    }

    /// The tip of the fragment.
    ///
    /// Returns the point of the last (newest) header in the fragment.
    /// If the fragment is empty, returns the anchor point (the immutable tip).
    ///
    /// Matches Haskell's `headPoint :: AnchoredFragment block -> Point block`.
    pub fn tip(&self) -> Point {
        match self.headers.back() {
            Some(h) => Point::Specific(h.slot, h.header_hash),
            None => self.anchor.clone(),
        }
    }

    /// Number of headers in the fragment (NOT counting the anchor).
    ///
    /// Equivalent to Haskell's `length :: AnchoredFragment block -> Int`.
    pub fn length(&self) -> usize {
        self.headers.len()
    }

    /// Whether the fragment contains no headers (tip == anchor).
    pub fn is_empty(&self) -> bool {
        self.headers.is_empty()
    }

    /// The block number at the tip of the fragment.
    ///
    /// - If the fragment is non-empty, returns the `block_number` of the last
    ///   header.
    /// - If the fragment is empty and the anchor is `Origin`, returns
    ///   `BlockNo(0)`.
    /// - If the fragment is empty and the anchor is `Specific`, there is no
    ///   block number embedded in the anchor point, so `BlockNo(0)` is returned
    ///   as a safe default.  Callers that need accurate block numbers for an
    ///   empty fragment should track this separately.
    pub fn tip_block_no(&self) -> BlockNo {
        match self.headers.back() {
            Some(h) => h.block_number,
            None => BlockNo(0),
        }
    }

    /// Return all headers as a slice (oldest first).
    pub fn headers(&self) -> &VecDeque<BlockHeader> {
        &self.headers
    }

    /// Return a reference to the oldest (front) header, or `None` if the
    /// fragment is empty.
    ///
    /// Used by the copy-to-immutable background operation to determine which
    /// block should be promoted to ImmutableDB when `fragment.len() > k`.
    pub fn oldest_header(&self) -> Option<&BlockHeader> {
        self.headers.front()
    }

    /// Remove and return the oldest (front) header from the fragment.
    ///
    /// Called after a block has been successfully copied to ImmutableDB to
    /// advance the volatile window.  The anchor is NOT updated here — callers
    /// that need to advance the anchor should update it separately via
    /// [`ChainFragment::new`] or by reconstructing from the new immutable tip.
    ///
    /// Returns `None` if the fragment is already empty.
    pub fn pop_oldest(&mut self) -> Option<BlockHeader> {
        self.headers.pop_front()
    }

    // -----------------------------------------------------------------------
    // Mutation
    // -----------------------------------------------------------------------

    /// Append a new header to the tip of the fragment.
    ///
    /// The caller is responsible for ensuring `header` is a valid extension of
    /// the current tip (correct `prev_hash`, strictly increasing slot/block
    /// number, valid VRF/KES signatures).
    ///
    /// Matches Haskell's `addBlock :: HasHeader b => b -> AnchoredFragment b -> ...`.
    pub fn push(&mut self, header: BlockHeader) {
        self.headers.push_back(header);
    }

    /// Roll the fragment back so that its tip becomes `point`.
    ///
    /// All headers whose slot is *after* `point` are dropped.  If `point` is
    /// the anchor, all headers are dropped and the fragment becomes empty.  If
    /// `point` is not found in the fragment or the anchor, returns `false` and
    /// leaves the fragment unchanged.
    ///
    /// Returns `true` if the rollback succeeded (the point was found).
    ///
    /// Matches Haskell's `rollback :: HasHeader b => Point block -> AnchoredFragment b -> Maybe (AnchoredFragment b)`.
    pub fn rollback_to(&mut self, point: &Point) -> bool {
        // Rolling back to the anchor always succeeds — clear all headers.
        if point == &self.anchor {
            self.headers.clear();
            return true;
        }

        // Find the header whose point matches.
        let target_slot = match point {
            Point::Origin => {
                // Can only roll back to Origin if our anchor is also Origin.
                if self.anchor == Point::Origin {
                    self.headers.clear();
                    return true;
                } else {
                    return false;
                }
            }
            Point::Specific(slot, hash) => (*slot, *hash),
        };

        // Search from the back (common case: rolling back a few recent headers).
        let position = self
            .headers
            .iter()
            .rposition(|h| h.slot == target_slot.0 && h.header_hash == target_slot.1);

        match position {
            Some(idx) => {
                // Keep headers[0..=idx], drop everything after.
                self.headers.truncate(idx + 1);
                true
            }
            None => {
                // Point not in fragment.
                false
            }
        }
    }

    /// Advance the anchor forward to `new_anchor`, removing all headers whose
    /// slot is ≤ the new anchor's slot from the front of the deque.
    ///
    /// This is called by the copy-to-immutable background thread when a block
    /// becomes k-deep and is moved from VolatileDB to ImmutableDB.
    ///
    /// Returns the number of headers removed.
    pub fn advance_anchor(&mut self, new_anchor: Point) -> usize {
        let new_slot = match &new_anchor {
            Point::Origin => {
                self.anchor = new_anchor;
                return 0;
            }
            Point::Specific(slot, _) => *slot,
        };

        let mut removed = 0;
        // Pop headers from the front that are now part of the immutable chain.
        while let Some(front) = self.headers.front() {
            if front.slot <= new_slot {
                self.headers.pop_front();
                removed += 1;
            } else {
                break;
            }
        }

        self.anchor = new_anchor;
        removed
    }

    // -----------------------------------------------------------------------
    // Intersection (ChainSync server support)
    // -----------------------------------------------------------------------

    /// Find the first point in `points` that exists in this fragment (or IS
    /// the anchor).
    ///
    /// The search respects the order of `points` — the first match wins.  This
    /// is used by the ChainSync server to find a common intersection with a
    /// peer's `FindIntersect` message.
    ///
    /// Matches Haskell's `findFirstPoint :: [Point block] -> AnchoredFragment b -> Maybe (Point block)`.
    ///
    /// # Complexity
    ///
    /// O(|points| × |headers|) — acceptable because `points` is bounded by
    /// the protocol (≤ ~O(log k) in practice) and `headers` is bounded by k.
    pub fn find_intersect(&self, points: &[Point]) -> Option<Point> {
        for point in points {
            if self.contains_point(point) {
                return Some(point.clone());
            }
        }
        None
    }

    /// Return true if `point` is either the anchor or the point of any header
    /// in the fragment.
    pub fn contains_point(&self, point: &Point) -> bool {
        if point == &self.anchor {
            return true;
        }
        match point {
            Point::Origin => self.anchor == Point::Origin,
            Point::Specific(slot, hash) => self
                .headers
                .iter()
                .any(|h| h.slot == *slot && h.header_hash == *hash),
        }
    }
}

// ---------------------------------------------------------------------------
// Display
// ---------------------------------------------------------------------------

impl std::fmt::Display for ChainFragment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ChainFragment {{ anchor: {}, length: {}, tip: {} }}",
            self.anchor,
            self.length(),
            self.tip()
        )
    }
}

// ---------------------------------------------------------------------------
// Tests (unit — integration tests live in tests/chain_fragment_tests.rs)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use torsten_primitives::hash::BlockHeaderHash;
    use torsten_primitives::time::SlotNo;

    /// Build a minimal `BlockHeader` for testing.
    ///
    /// Only the fields relevant to `ChainFragment` (`header_hash`, `prev_hash`,
    /// `slot`, `block_number`) are meaningful; all others use zero/empty values.
    fn make_header(slot: u64, block_no: u64, hash: u8, prev: u8) -> BlockHeader {
        use torsten_primitives::block::{OperationalCert, ProtocolVersion, VrfOutput};
        BlockHeader {
            header_hash: BlockHeaderHash::from_bytes([hash; 32]),
            prev_hash: BlockHeaderHash::from_bytes([prev; 32]),
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
        }
    }

    fn point(slot: u64, hash_byte: u8) -> Point {
        Point::Specific(SlotNo(slot), BlockHeaderHash::from_bytes([hash_byte; 32]))
    }

    #[test]
    fn empty_fragment_tip_is_anchor() {
        let anchor = point(100, 0xAA);
        let frag = ChainFragment::new(anchor.clone());
        assert_eq!(frag.tip(), anchor);
        assert_eq!(frag.length(), 0);
        assert!(frag.is_empty());
    }

    #[test]
    fn push_updates_tip_and_length() {
        let anchor = point(100, 0xAA);
        let mut frag = ChainFragment::new(anchor.clone());

        frag.push(make_header(101, 1, 0xBB, 0xAA));
        assert_eq!(frag.length(), 1);
        assert_eq!(frag.tip(), point(101, 0xBB));

        frag.push(make_header(102, 2, 0xCC, 0xBB));
        assert_eq!(frag.length(), 2);
        assert_eq!(frag.tip(), point(102, 0xCC));
    }

    #[test]
    fn rollback_to_anchor_clears_all_headers() {
        let anchor = point(100, 0xAA);
        let mut frag = ChainFragment::new(anchor.clone());
        frag.push(make_header(101, 1, 0xBB, 0xAA));
        frag.push(make_header(102, 2, 0xCC, 0xBB));

        let ok = frag.rollback_to(&anchor);
        assert!(ok);
        assert!(frag.is_empty());
        assert_eq!(frag.tip(), anchor);
    }

    #[test]
    fn rollback_to_mid_point() {
        let anchor = point(100, 0xAA);
        let mut frag = ChainFragment::new(anchor.clone());
        frag.push(make_header(101, 1, 0xBB, 0xAA));
        frag.push(make_header(102, 2, 0xCC, 0xBB));
        frag.push(make_header(103, 3, 0xDD, 0xCC));

        let ok = frag.rollback_to(&point(101, 0xBB));
        assert!(ok);
        assert_eq!(frag.length(), 1);
        assert_eq!(frag.tip(), point(101, 0xBB));
    }

    #[test]
    fn rollback_to_unknown_point_fails() {
        let anchor = point(100, 0xAA);
        let mut frag = ChainFragment::new(anchor.clone());
        frag.push(make_header(101, 1, 0xBB, 0xAA));

        let ok = frag.rollback_to(&point(999, 0xFF));
        assert!(!ok);
        // Fragment unchanged
        assert_eq!(frag.length(), 1);
    }

    #[test]
    fn find_intersect_anchor() {
        let anchor = point(100, 0xAA);
        let frag = ChainFragment::new(anchor.clone());
        let result = frag.find_intersect(std::slice::from_ref(&anchor));
        assert_eq!(result, Some(anchor));
    }

    #[test]
    fn find_intersect_returns_first_match() {
        let anchor = point(100, 0xAA);
        let mut frag = ChainFragment::new(anchor.clone());
        frag.push(make_header(101, 1, 0xBB, 0xAA));
        frag.push(make_header(102, 2, 0xCC, 0xBB));

        // Both points exist; should return the first one tried
        let result = frag.find_intersect(&[point(102, 0xCC), point(101, 0xBB)]);
        assert_eq!(result, Some(point(102, 0xCC)));
    }

    #[test]
    fn find_intersect_no_match() {
        let anchor = point(100, 0xAA);
        let mut frag = ChainFragment::new(anchor.clone());
        frag.push(make_header(101, 1, 0xBB, 0xAA));

        let result = frag.find_intersect(&[point(999, 0xFF), point(888, 0xEE)]);
        assert_eq!(result, None);
    }

    #[test]
    fn tip_block_no_empty() {
        let anchor = point(100, 0xAA);
        let frag = ChainFragment::new(anchor);
        assert_eq!(frag.tip_block_no(), BlockNo(0));
    }

    #[test]
    fn tip_block_no_nonempty() {
        let anchor = point(100, 0xAA);
        let mut frag = ChainFragment::new(anchor);
        frag.push(make_header(101, 42, 0xBB, 0xAA));
        assert_eq!(frag.tip_block_no(), BlockNo(42));
    }
}
