use torsten_primitives::block::{Point, Tip};

/// Chain selection rule: prefer the longest chain (by block number)
///
/// In Ouroboros Praos, the chain selection rule is:
/// 1. Prefer the chain with more blocks
/// 2. If equal length, prefer the chain received first
/// 3. Only consider chains within the stability window
pub struct ChainSelection {
    pub current_tip: Tip,
}

impl ChainSelection {
    pub fn new() -> Self {
        ChainSelection {
            current_tip: Tip::origin(),
        }
    }

    /// Compare two chain candidates and determine which is preferred
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

    /// Update the current tip
    pub fn set_tip(&mut self, tip: Tip) {
        self.current_tip = tip;
    }

    /// Check if a candidate chain would trigger a switch
    pub fn should_switch(&self, candidate: &Tip) -> bool {
        matches!(self.prefer(candidate), ChainPreference::PreferCandidate)
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
}
