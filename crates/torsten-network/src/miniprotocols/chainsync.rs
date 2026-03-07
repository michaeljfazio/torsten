use torsten_primitives::block::{Point, Tip};

/// Chain-sync mini-protocol messages
///
/// The chain-sync protocol allows a client to synchronize the chain
/// by following the server's chain. It supports both header-only
/// and full-block variants.
#[derive(Debug, Clone)]
pub enum ChainSyncMessage {
    // Client messages
    RequestNext,
    FindIntersect(Vec<Point>),
    Done,

    // Server messages
    RollForward(RollForwardData, Tip),
    RollBackward(Point, Tip),
    AwaitReply,
    IntersectFound(Point, Tip),
    IntersectNotFound(Tip),
}

/// Data delivered with a RollForward message
#[derive(Debug, Clone)]
pub enum RollForwardData {
    /// Block header (for node-to-node chain sync)
    BlockHeader(Vec<u8>),
    /// Full block (for node-to-client chain sync)
    Block(Vec<u8>),
}

/// Chain-sync state machine states
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainSyncState {
    /// Client has agency - can request next or find intersection
    StIdle,
    /// Server has agency - will respond with roll forward/backward/await
    StNext,
    /// Server has agency - will respond to intersection query
    StIntersect,
    /// Protocol finished
    StDone,
}

/// Chain-sync client state
pub struct ChainSyncClient {
    pub state: ChainSyncState,
    pub known_points: Vec<Point>,
    pub tip: Option<Tip>,
}

impl Default for ChainSyncClient {
    fn default() -> Self {
        Self::new()
    }
}

impl ChainSyncClient {
    pub fn new() -> Self {
        ChainSyncClient {
            state: ChainSyncState::StIdle,
            known_points: vec![Point::Origin],
            tip: None,
        }
    }

    /// Generate find-intersect message with exponentially spaced points
    pub fn find_intersect_points(chain: &[Point], max_points: usize) -> Vec<Point> {
        if chain.is_empty() {
            return vec![Point::Origin];
        }

        let mut points = Vec::new();
        let mut step = 1;
        let mut i = chain.len();

        while i > 0 && points.len() < max_points {
            i = i.saturating_sub(step);
            points.push(chain[i].clone());
            step *= 2;
        }

        if points.last() != Some(&Point::Origin) {
            points.push(Point::Origin);
        }

        points
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use torsten_primitives::hash::Hash32;
    use torsten_primitives::time::SlotNo;

    #[test]
    fn test_new_client() {
        let client = ChainSyncClient::new();
        assert_eq!(client.state, ChainSyncState::StIdle);
        assert_eq!(client.known_points.len(), 1);
        assert_eq!(client.known_points[0], Point::Origin);
    }

    #[test]
    fn test_find_intersect_empty_chain() {
        let points = ChainSyncClient::find_intersect_points(&[], 10);
        assert_eq!(points, vec![Point::Origin]);
    }

    #[test]
    fn test_find_intersect_points() {
        let chain: Vec<Point> = (0..100)
            .map(|i| Point::Specific(SlotNo(i), Hash32::from_bytes([i as u8; 32])))
            .collect();

        let points = ChainSyncClient::find_intersect_points(&chain, 10);
        assert!(!points.is_empty());
        assert!(points.len() <= 10);
        // Should include Origin as the last point
        assert_eq!(*points.last().unwrap(), Point::Origin);
    }
}
