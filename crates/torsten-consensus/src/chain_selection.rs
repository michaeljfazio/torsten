use torsten_primitives::block::{Point, Tip};
use torsten_primitives::era::Era;
use torsten_primitives::hash::BlockHeaderHash;

/// Chain selection rule implementing Ouroboros chain preference.
///
/// Supports two distinct modes matching the Haskell cardano-node:
///
/// - **Byron (OBFT)**: Chains are compared by *density* — the ratio of blocks
///   to slots. A denser chain is preferred because Byron used a round-robin
///   (OBFT) protocol where honest chains should fill most slots.
///
/// - **Praos (Shelley+)**: Chains are compared by *length* (block number).
///   Longer chains are always preferred because Praos is a longest-chain protocol.
///
/// In both modes, when the primary metric is equal, a deterministic tiebreaker
/// is applied using the block header hash: the **lower** hash wins. This matches
/// the Haskell cardano-node behavior and ensures all honest nodes converge on
/// the same fork without relying on arrival order.
pub struct ChainSelection {
    pub current_tip: Tip,
}

impl ChainSelection {
    pub fn new() -> Self {
        ChainSelection {
            current_tip: Tip::origin(),
        }
    }

    /// Compare two chain candidates by block number only (legacy Praos rule).
    ///
    /// Does NOT perform deterministic tie-breaking. Retained for backward
    /// compatibility — callers that don't have hash/era context can still use
    /// this, but should prefer [`prefer_chain`] for correctness.
    pub fn prefer(&self, candidate: &Tip) -> ChainPreference {
        match (&self.current_tip.point, &candidate.point) {
            (Point::Origin, Point::Origin) => ChainPreference::Equal,
            (Point::Origin, _) => ChainPreference::PreferCandidate,
            (_, Point::Origin) => ChainPreference::PreferCurrent,
            _ => {
                if candidate.block_number > self.current_tip.block_number {
                    ChainPreference::PreferCandidate
                } else if candidate.block_number < self.current_tip.block_number {
                    ChainPreference::PreferCurrent
                } else {
                    ChainPreference::Equal
                }
            }
        }
    }

    /// Full chain preference with era-aware comparison and deterministic
    /// tie-breaking.
    ///
    /// - In **Byron** era: compares chain density (blocks / slots). A chain
    ///   covering fewer slots with the same number of blocks is denser.
    /// - In **Praos** eras (Shelley+): compares chain length (block number).
    /// - **Tiebreaker** (both eras): lower block header hash wins.
    ///
    /// `current_hash` and `candidate_hash` are the header hashes of the tip
    /// blocks of the current and candidate chains respectively.
    pub fn prefer_chain(
        &self,
        candidate: &Tip,
        era: Era,
        current_hash: &BlockHeaderHash,
        candidate_hash: &BlockHeaderHash,
    ) -> ChainPreference {
        match (&self.current_tip.point, &candidate.point) {
            (Point::Origin, Point::Origin) => ChainPreference::Equal,
            (Point::Origin, _) => ChainPreference::PreferCandidate,
            (_, Point::Origin) => ChainPreference::PreferCurrent,
            _ => {
                let primary = if era == Era::Byron {
                    self.compare_density(candidate)
                } else {
                    self.compare_length(candidate)
                };

                match primary {
                    ChainPreference::Equal => {
                        // Deterministic tiebreaker: lower header hash wins
                        hash_tiebreak(current_hash, candidate_hash)
                    }
                    other => other,
                }
            }
        }
    }

    /// Check if a candidate chain would trigger a switch (legacy API).
    pub fn should_switch(&self, candidate: &Tip) -> bool {
        matches!(self.prefer(candidate), ChainPreference::PreferCandidate)
    }

    /// Check if a candidate chain would trigger a switch using full
    /// era-aware comparison with deterministic tiebreaking.
    pub fn should_switch_chain(
        &self,
        candidate: &Tip,
        era: Era,
        current_hash: &BlockHeaderHash,
        candidate_hash: &BlockHeaderHash,
    ) -> bool {
        matches!(
            self.prefer_chain(candidate, era, current_hash, candidate_hash),
            ChainPreference::PreferCandidate
        )
    }

    /// Update the current tip.
    pub fn set_tip(&mut self, tip: Tip) {
        self.current_tip = tip;
    }

    /// Compare chains by block number (Praos longest-chain rule).
    fn compare_length(&self, candidate: &Tip) -> ChainPreference {
        if candidate.block_number > self.current_tip.block_number {
            ChainPreference::PreferCandidate
        } else if candidate.block_number < self.current_tip.block_number {
            ChainPreference::PreferCurrent
        } else {
            ChainPreference::Equal
        }
    }

    /// Compare chains by density (Byron OBFT rule).
    ///
    /// Density = block_count / slot_span. We compare using cross-multiplication
    /// to avoid floating-point: chain A is denser than B when
    ///   A.blocks * B.slots > B.blocks * A.slots
    ///
    /// If both chains span 0 slots (single genesis blocks), fall back to
    /// block number comparison.
    fn compare_density(&self, candidate: &Tip) -> ChainPreference {
        let current_slot = self.current_tip.point.slot().map(|s| s.0).unwrap_or(0);
        let candidate_slot = candidate.point.slot().map(|s| s.0).unwrap_or(0);

        let current_blocks = self.current_tip.block_number.0;
        let candidate_blocks = candidate.block_number.0;

        // If either chain has zero slot span, fall back to block count
        if current_slot == 0 && candidate_slot == 0 {
            return self.compare_length(candidate);
        }

        // Density comparison via cross-multiplication to avoid floating point:
        // candidate_density > current_density
        // ⟺ candidate_blocks / candidate_slot > current_blocks / current_slot
        // ⟺ candidate_blocks * current_slot > current_blocks * candidate_slot
        //
        // Using u128 to prevent overflow for large slot numbers
        let lhs = (candidate_blocks as u128) * (current_slot as u128);
        let rhs = (current_blocks as u128) * (candidate_slot as u128);

        if lhs > rhs {
            ChainPreference::PreferCandidate
        } else if lhs < rhs {
            ChainPreference::PreferCurrent
        } else {
            ChainPreference::Equal
        }
    }
}

/// Deterministic fork tiebreaker: the chain with the **lower** block header
/// hash is preferred. This matches the Haskell cardano-node behavior where
/// `compare` on `HeaderHash` is used as the ultimate tiebreaker.
fn hash_tiebreak(
    current_hash: &BlockHeaderHash,
    candidate_hash: &BlockHeaderHash,
) -> ChainPreference {
    if candidate_hash < current_hash {
        ChainPreference::PreferCandidate
    } else if candidate_hash > current_hash {
        ChainPreference::PreferCurrent
    } else {
        // Identical hashes — truly equal (same block)
        ChainPreference::Equal
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainPreference {
    PreferCurrent,
    PreferCandidate,
    Equal,
}

impl Default for ChainSelection {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use torsten_primitives::hash::Hash32;
    use torsten_primitives::time::{BlockNo, SlotNo};

    fn make_tip(block_no: u64, slot: u64) -> Tip {
        Tip {
            point: Point::Specific(SlotNo(slot), Hash32::from_bytes([block_no as u8; 32])),
            block_number: BlockNo(block_no),
        }
    }

    fn make_tip_with_hash(block_no: u64, slot: u64, hash: Hash32) -> Tip {
        Tip {
            point: Point::Specific(SlotNo(slot), hash),
            block_number: BlockNo(block_no),
        }
    }

    // -----------------------------------------------------------------------
    // Legacy API tests (backward compatibility)
    // -----------------------------------------------------------------------

    #[test]
    fn test_origin_vs_block() {
        let cs = ChainSelection::new();
        let candidate = make_tip(1, 100);
        assert_eq!(cs.prefer(&candidate), ChainPreference::PreferCandidate);
    }

    #[test]
    fn test_longer_chain_preferred() {
        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip(10, 200));

        let longer = make_tip(11, 210);
        assert_eq!(cs.prefer(&longer), ChainPreference::PreferCandidate);
        assert!(cs.should_switch(&longer));
    }

    #[test]
    fn test_shorter_chain_not_preferred() {
        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip(10, 200));

        let shorter = make_tip(9, 180);
        assert_eq!(cs.prefer(&shorter), ChainPreference::PreferCurrent);
        assert!(!cs.should_switch(&shorter));
    }

    #[test]
    fn test_equal_chains() {
        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip(10, 200));

        let equal = make_tip(10, 200);
        assert_eq!(cs.prefer(&equal), ChainPreference::Equal);
        assert!(!cs.should_switch(&equal));
    }

    #[test]
    fn test_both_origin() {
        let cs = ChainSelection::new();
        assert_eq!(cs.prefer(&Tip::origin()), ChainPreference::Equal);
    }

    // -----------------------------------------------------------------------
    // Praos era: longer chain preferred
    // -----------------------------------------------------------------------

    #[test]
    fn test_praos_longer_chain_preferred() {
        let current_hash = Hash32::from_bytes([0xAA; 32]);
        let candidate_hash = Hash32::from_bytes([0xBB; 32]);

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(10, 200, current_hash));

        let candidate = make_tip_with_hash(12, 240, candidate_hash);
        assert_eq!(
            cs.prefer_chain(&candidate, Era::Shelley, &current_hash, &candidate_hash),
            ChainPreference::PreferCandidate
        );
    }

    #[test]
    fn test_praos_shorter_chain_not_preferred() {
        let current_hash = Hash32::from_bytes([0xBB; 32]);
        let candidate_hash = Hash32::from_bytes([0xAA; 32]);

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(15, 300, current_hash));

        let candidate = make_tip_with_hash(12, 240, candidate_hash);
        assert_eq!(
            cs.prefer_chain(&candidate, Era::Babbage, &current_hash, &candidate_hash),
            ChainPreference::PreferCurrent
        );
    }

    #[test]
    fn test_praos_all_post_shelley_eras() {
        // Verify all Shelley+ eras use length comparison, not density
        for era in [
            Era::Shelley,
            Era::Allegra,
            Era::Mary,
            Era::Alonzo,
            Era::Babbage,
            Era::Conway,
        ] {
            let current_hash = Hash32::from_bytes([0xCC; 32]);
            let candidate_hash = Hash32::from_bytes([0xDD; 32]);

            let mut cs = ChainSelection::new();
            cs.set_tip(make_tip_with_hash(10, 200, current_hash));

            let candidate = make_tip_with_hash(11, 220, candidate_hash);
            assert_eq!(
                cs.prefer_chain(&candidate, era, &current_hash, &candidate_hash),
                ChainPreference::PreferCandidate,
                "Era {:?} should prefer longer chain",
                era
            );
        }
    }

    // -----------------------------------------------------------------------
    // Byron era: density-based chain selection
    // -----------------------------------------------------------------------

    #[test]
    fn test_byron_higher_density_preferred() {
        // Chain A (current): 8 blocks in 100 slots → density 0.08
        // Chain B (candidate): 8 blocks in 80 slots → density 0.10
        // Candidate is denser, should be preferred
        let current_hash = Hash32::from_bytes([0xAA; 32]);
        let candidate_hash = Hash32::from_bytes([0xBB; 32]);

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(8, 100, current_hash));

        let candidate = make_tip_with_hash(8, 80, candidate_hash);
        assert_eq!(
            cs.prefer_chain(&candidate, Era::Byron, &current_hash, &candidate_hash),
            ChainPreference::PreferCandidate
        );
    }

    #[test]
    fn test_byron_lower_density_not_preferred() {
        // Chain A (current): 10 blocks in 100 slots → density 0.10
        // Chain B (candidate): 8 blocks in 100 slots → density 0.08
        // Current is denser
        let current_hash = Hash32::from_bytes([0xAA; 32]);
        let candidate_hash = Hash32::from_bytes([0xBB; 32]);

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(10, 100, current_hash));

        let candidate = make_tip_with_hash(8, 100, candidate_hash);
        assert_eq!(
            cs.prefer_chain(&candidate, Era::Byron, &current_hash, &candidate_hash),
            ChainPreference::PreferCurrent
        );
    }

    #[test]
    fn test_byron_density_more_blocks_fewer_slots_wins() {
        // Chain A (current): 5 blocks in 50 slots → density 0.10
        // Chain B (candidate): 7 blocks in 50 slots → density 0.14
        // Candidate has more blocks in same slot range
        let current_hash = Hash32::from_bytes([0x11; 32]);
        let candidate_hash = Hash32::from_bytes([0x22; 32]);

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(5, 50, current_hash));

        let candidate = make_tip_with_hash(7, 50, candidate_hash);
        assert_eq!(
            cs.prefer_chain(&candidate, Era::Byron, &current_hash, &candidate_hash),
            ChainPreference::PreferCandidate
        );
    }

    #[test]
    fn test_byron_density_cross_multiplication_large_values() {
        // Test with large slot/block numbers to verify u128 overflow protection
        // Chain A: 1,000,000 blocks in 10,000,000 slots → density 0.1
        // Chain B: 1,000,001 blocks in 10,000,000 slots → density ~0.1000001
        let current_hash = Hash32::from_bytes([0x01; 32]);
        let candidate_hash = Hash32::from_bytes([0x02; 32]);

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(1_000_000, 10_000_000, current_hash));

        let candidate = make_tip_with_hash(1_000_001, 10_000_000, candidate_hash);
        assert_eq!(
            cs.prefer_chain(&candidate, Era::Byron, &current_hash, &candidate_hash),
            ChainPreference::PreferCandidate
        );
    }

    #[test]
    fn test_byron_same_density_uses_tiebreak() {
        // Chain A: 10 blocks in 100 slots → density 0.10
        // Chain B: 20 blocks in 200 slots → density 0.10
        // Same density → tiebreaker by hash
        let current_hash = Hash32::from_bytes([0xFF; 32]); // higher hash
        let candidate_hash = Hash32::from_bytes([0x01; 32]); // lower hash

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(10, 100, current_hash));

        let candidate = make_tip_with_hash(20, 200, candidate_hash);
        assert_eq!(
            cs.prefer_chain(&candidate, Era::Byron, &current_hash, &candidate_hash),
            ChainPreference::PreferCandidate, // lower hash wins
        );
    }

    #[test]
    fn test_byron_vs_praos_different_preference() {
        // Demonstrate that Byron and Praos can give different results for the
        // same pair of chains:
        // Chain A (current): 10 blocks in 100 slots (density 0.10, length 10)
        // Chain B (candidate): 9 blocks in 80 slots (density 0.1125, length 9)
        //
        // Byron prefers B (denser), Praos prefers A (longer).
        let current_hash = Hash32::from_bytes([0xAA; 32]);
        let candidate_hash = Hash32::from_bytes([0xBB; 32]);

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(10, 100, current_hash));

        let candidate = make_tip_with_hash(9, 80, candidate_hash);

        // Byron: candidate is denser (9/80 > 10/100 → 900 > 800)
        assert_eq!(
            cs.prefer_chain(&candidate, Era::Byron, &current_hash, &candidate_hash),
            ChainPreference::PreferCandidate
        );

        // Praos: current is longer (10 > 9)
        assert_eq!(
            cs.prefer_chain(&candidate, Era::Shelley, &current_hash, &candidate_hash),
            ChainPreference::PreferCurrent
        );
    }

    // -----------------------------------------------------------------------
    // Deterministic fork tie-breaking
    // -----------------------------------------------------------------------

    #[test]
    fn test_tiebreak_lower_hash_wins() {
        let low_hash = Hash32::from_bytes([0x01; 32]);
        let high_hash = Hash32::from_bytes([0xFF; 32]);

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(10, 200, high_hash));

        let candidate = make_tip_with_hash(10, 200, low_hash);

        // Same length, lower hash candidate wins
        assert_eq!(
            cs.prefer_chain(&candidate, Era::Conway, &high_hash, &low_hash),
            ChainPreference::PreferCandidate
        );
    }

    #[test]
    fn test_tiebreak_higher_hash_loses() {
        let low_hash = Hash32::from_bytes([0x01; 32]);
        let high_hash = Hash32::from_bytes([0xFF; 32]);

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(10, 200, low_hash));

        let candidate = make_tip_with_hash(10, 200, high_hash);

        // Same length, higher hash candidate loses
        assert_eq!(
            cs.prefer_chain(&candidate, Era::Conway, &low_hash, &high_hash),
            ChainPreference::PreferCurrent
        );
    }

    #[test]
    fn test_tiebreak_identical_hashes() {
        let same_hash = Hash32::from_bytes([0x42; 32]);

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(10, 200, same_hash));

        let candidate = make_tip_with_hash(10, 200, same_hash);

        // Identical tips → Equal
        assert_eq!(
            cs.prefer_chain(&candidate, Era::Conway, &same_hash, &same_hash),
            ChainPreference::Equal
        );
    }

    #[test]
    fn test_tiebreak_only_first_byte_differs() {
        let mut hash_a_bytes = [0x00; 32];
        let mut hash_b_bytes = [0x00; 32];
        hash_a_bytes[0] = 0x01;
        hash_b_bytes[0] = 0x02;
        let hash_a = Hash32::from_bytes(hash_a_bytes);
        let hash_b = Hash32::from_bytes(hash_b_bytes);

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(5, 100, hash_b));

        let candidate = make_tip_with_hash(5, 100, hash_a);
        assert_eq!(
            cs.prefer_chain(&candidate, Era::Babbage, &hash_b, &hash_a),
            ChainPreference::PreferCandidate, // hash_a < hash_b
        );
    }

    #[test]
    fn test_tiebreak_only_last_byte_differs() {
        let mut hash_a_bytes = [0x00; 32];
        let mut hash_b_bytes = [0x00; 32];
        hash_a_bytes[31] = 0x01;
        hash_b_bytes[31] = 0x02;
        let hash_a = Hash32::from_bytes(hash_a_bytes);
        let hash_b = Hash32::from_bytes(hash_b_bytes);

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(5, 100, hash_b));

        let candidate = make_tip_with_hash(5, 100, hash_a);
        assert_eq!(
            cs.prefer_chain(&candidate, Era::Shelley, &hash_b, &hash_a),
            ChainPreference::PreferCandidate, // hash_a < hash_b
        );
    }

    #[test]
    fn test_tiebreak_does_not_override_length() {
        // Length difference should take priority over hash comparison
        let low_hash = Hash32::from_bytes([0x01; 32]);
        let high_hash = Hash32::from_bytes([0xFF; 32]);

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(11, 220, low_hash)); // longer chain, lower hash

        let candidate = make_tip_with_hash(10, 200, high_hash); // shorter, higher hash

        // Current is longer → PreferCurrent, even though candidate hash is irrelevant
        assert_eq!(
            cs.prefer_chain(&candidate, Era::Conway, &low_hash, &high_hash),
            ChainPreference::PreferCurrent
        );
    }

    // -----------------------------------------------------------------------
    // Edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn test_origin_vs_candidate_with_era() {
        let candidate_hash = Hash32::from_bytes([0xAB; 32]);
        let current_hash = Hash32::ZERO;

        let cs = ChainSelection::new(); // origin
        let candidate = make_tip_with_hash(1, 10, candidate_hash);

        assert_eq!(
            cs.prefer_chain(&candidate, Era::Byron, &current_hash, &candidate_hash),
            ChainPreference::PreferCandidate
        );
        assert_eq!(
            cs.prefer_chain(&candidate, Era::Conway, &current_hash, &candidate_hash),
            ChainPreference::PreferCandidate
        );
    }

    #[test]
    fn test_candidate_origin_with_era() {
        let current_hash = Hash32::from_bytes([0xAB; 32]);
        let candidate_hash = Hash32::ZERO;

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(5, 100, current_hash));

        assert_eq!(
            cs.prefer_chain(&Tip::origin(), Era::Byron, &current_hash, &candidate_hash),
            ChainPreference::PreferCurrent
        );
    }

    #[test]
    fn test_both_origin_with_era() {
        let hash = Hash32::ZERO;
        let cs = ChainSelection::new();
        assert_eq!(
            cs.prefer_chain(&Tip::origin(), Era::Byron, &hash, &hash),
            ChainPreference::Equal
        );
    }

    #[test]
    fn test_single_block_chain_vs_origin() {
        let hash = Hash32::from_bytes([0x42; 32]);
        let cs = ChainSelection::new();
        let single_block = make_tip_with_hash(1, 1, hash);

        assert_eq!(
            cs.prefer_chain(&single_block, Era::Byron, &Hash32::ZERO, &hash),
            ChainPreference::PreferCandidate
        );
    }

    #[test]
    fn test_should_switch_chain_with_tiebreak() {
        let low_hash = Hash32::from_bytes([0x01; 32]);
        let high_hash = Hash32::from_bytes([0xFF; 32]);

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(10, 200, high_hash));

        let candidate = make_tip_with_hash(10, 200, low_hash);
        assert!(cs.should_switch_chain(&candidate, Era::Conway, &high_hash, &low_hash));

        // Reverse: candidate has higher hash, should NOT switch
        let mut cs2 = ChainSelection::new();
        cs2.set_tip(make_tip_with_hash(10, 200, low_hash));
        let candidate2 = make_tip_with_hash(10, 200, high_hash);
        assert!(!cs2.should_switch_chain(&candidate2, Era::Conway, &low_hash, &high_hash));
    }

    #[test]
    fn test_byron_density_slot_1_block_1() {
        // Single-block chains at slot 1
        // Chain A: 1 block at slot 1 → density 1.0
        // Chain B: 1 block at slot 2 → density 0.5
        let hash_a = Hash32::from_bytes([0xAA; 32]);
        let hash_b = Hash32::from_bytes([0xBB; 32]);

        let mut cs = ChainSelection::new();
        cs.set_tip(make_tip_with_hash(1, 2, hash_a));

        let candidate = make_tip_with_hash(1, 1, hash_b);
        // candidate density (1/1 = 1.0) > current density (1/2 = 0.5)
        assert_eq!(
            cs.prefer_chain(&candidate, Era::Byron, &hash_a, &hash_b),
            ChainPreference::PreferCandidate
        );
    }

    #[test]
    fn test_hash_tiebreak_function() {
        let low = Hash32::from_bytes([0x00; 32]);
        let high = Hash32::from_bytes([0xFF; 32]);
        let mid = Hash32::from_bytes([0x80; 32]);

        assert_eq!(hash_tiebreak(&high, &low), ChainPreference::PreferCandidate);
        assert_eq!(hash_tiebreak(&low, &high), ChainPreference::PreferCurrent);
        assert_eq!(hash_tiebreak(&low, &low), ChainPreference::Equal);
        assert_eq!(hash_tiebreak(&high, &mid), ChainPreference::PreferCandidate);
        assert_eq!(hash_tiebreak(&mid, &high), ChainPreference::PreferCurrent);
    }
}
