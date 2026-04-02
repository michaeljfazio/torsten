//! Block fetch decision engine — decides which peer to fetch blocks from.
//!
//! Sits between ChainSync (which receives headers) and BlockFetch (which downloads blocks).
//! Maintains a download queue, selects peers by latency, distributes ranges for parallel
//! fetching, retries on failure, and handles rollbacks.

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;

use crate::codec::Point;

/// Maximum in-flight ranges per peer.
const DEFAULT_MAX_IN_FLIGHT: usize = 100;

/// State of a peer for block fetch decisions.
#[derive(Debug, Clone)]
pub struct PeerFetchState {
    /// Peer address.
    pub addr: SocketAddr,
    /// Estimated latency in milliseconds.
    pub latency_ms: f64,
    /// Number of ranges currently in-flight for this peer.
    pub in_flight: usize,
    /// Tip slot advertised by this peer via ChainSync.
    pub tip_slot: u64,
}

/// A range of blocks to fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchRange {
    /// Start of the range.
    pub from: Point,
    /// End of the range.
    pub to: Point,
}

/// Block fetch decision engine.
pub struct BlockFetchDecision {
    /// Queue of ranges that need to be downloaded.
    queue: VecDeque<FetchRange>,
    /// Ranges currently in-flight, keyed by peer address.
    in_flight: HashMap<SocketAddr, Vec<FetchRange>>,
    /// Maximum in-flight ranges per peer.
    max_in_flight: usize,
}

impl BlockFetchDecision {
    /// Create a new decision engine.
    pub fn new(max_in_flight: usize) -> Self {
        Self {
            queue: VecDeque::new(),
            in_flight: HashMap::new(),
            max_in_flight,
        }
    }

    /// Create with default settings.
    pub fn with_defaults() -> Self {
        Self::new(DEFAULT_MAX_IN_FLIGHT)
    }

    /// Add a range to the download queue.
    pub fn add_range(&mut self, from: Point, to: Point) {
        self.queue.push_back(FetchRange { from, to });
    }

    /// Select the next peer to fetch from, considering latency and in-flight limits.
    ///
    /// Returns `Some((peer_addr, range))` if a peer and range are available,
    /// `None` if no work is available or all peers are at capacity.
    pub fn select_peer(&mut self, peers: &[PeerFetchState]) -> Option<(SocketAddr, FetchRange)> {
        if self.queue.is_empty() {
            return None;
        }

        // Sort peers by latency (lowest first), then filter by in-flight capacity
        let mut candidates: Vec<&PeerFetchState> = peers
            .iter()
            .filter(|p| {
                let current = self.in_flight.get(&p.addr).map_or(0, |v| v.len());
                current < self.max_in_flight
            })
            .collect();
        candidates.sort_by(|a, b| {
            a.latency_ms
                .partial_cmp(&b.latency_ms)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        if let Some(best) = candidates.first() {
            if let Some(range) = self.queue.pop_front() {
                self.in_flight
                    .entry(best.addr)
                    .or_default()
                    .push(range.clone());
                return Some((best.addr, range));
            }
        }

        None
    }

    /// Mark a range as completed for a peer.
    pub fn mark_completed(&mut self, peer: SocketAddr, range: &FetchRange) {
        if let Some(ranges) = self.in_flight.get_mut(&peer) {
            ranges.retain(|r| r != range);
            if ranges.is_empty() {
                self.in_flight.remove(&peer);
            }
        }
    }

    /// Mark a range as failed — re-queue it for retry on a different peer.
    pub fn mark_failed(&mut self, peer: SocketAddr, range: &FetchRange) {
        // Remove from in-flight
        if let Some(ranges) = self.in_flight.get_mut(&peer) {
            ranges.retain(|r| r != range);
            if ranges.is_empty() {
                self.in_flight.remove(&peer);
            }
        }
        // Re-queue for retry
        self.queue.push_back(range.clone());
    }

    /// Handle a rollback — remove any queued or in-flight ranges that are
    /// beyond the rollback point.
    pub fn rollback_to(&mut self, point: &Point) {
        let rollback_slot = match point {
            Point::Origin => 0,
            Point::Specific(slot, _) => *slot,
        };

        // Remove from queue
        self.queue.retain(|range| {
            let from_slot = match &range.from {
                Point::Origin => 0,
                Point::Specific(s, _) => *s,
            };
            from_slot <= rollback_slot
        });

        // Remove from in-flight
        for ranges in self.in_flight.values_mut() {
            ranges.retain(|range| {
                let from_slot = match &range.from {
                    Point::Origin => 0,
                    Point::Specific(s, _) => *s,
                };
                from_slot <= rollback_slot
            });
        }
        self.in_flight.retain(|_, v| !v.is_empty());
    }

    /// Number of ranges in the download queue.
    pub fn queue_len(&self) -> usize {
        self.queue.len()
    }

    /// Total number of ranges in-flight across all peers.
    pub fn total_in_flight(&self) -> usize {
        self.in_flight.values().map(|v| v.len()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn test_addr(port: u16) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4)), port)
    }

    fn test_peer(port: u16, latency: f64) -> PeerFetchState {
        PeerFetchState {
            addr: test_addr(port),
            latency_ms: latency,
            in_flight: 0,
            tip_slot: 1000,
        }
    }

    #[test]
    fn selects_lowest_latency_peer() {
        let mut decision = BlockFetchDecision::with_defaults();
        decision.add_range(
            Point::Specific(10, [0x01; 32]),
            Point::Specific(20, [0x02; 32]),
        );

        let peers = vec![test_peer(3001, 100.0), test_peer(3002, 50.0)];

        let (addr, _range) = decision.select_peer(&peers).unwrap();
        assert_eq!(addr, test_addr(3002)); // lower latency
    }

    #[test]
    fn respects_in_flight_limit() {
        let mut decision = BlockFetchDecision::new(1); // max 1 in-flight

        // Add two ranges
        decision.add_range(
            Point::Specific(10, [0x01; 32]),
            Point::Specific(20, [0x02; 32]),
        );
        decision.add_range(
            Point::Specific(30, [0x03; 32]),
            Point::Specific(40, [0x04; 32]),
        );

        let peers = vec![test_peer(3001, 50.0)];

        // First select should work
        let result1 = decision.select_peer(&peers);
        assert!(result1.is_some());

        // Second select should fail (peer at capacity)
        let result2 = decision.select_peer(&peers);
        assert!(result2.is_none());
    }

    #[test]
    fn failed_range_requeued() {
        let mut decision = BlockFetchDecision::with_defaults();
        let range = FetchRange {
            from: Point::Specific(10, [0x01; 32]),
            to: Point::Specific(20, [0x02; 32]),
        };
        decision.add_range(range.from.clone(), range.to.clone());

        let peers = vec![test_peer(3001, 50.0)];
        let (addr, fetched) = decision.select_peer(&peers).unwrap();
        assert_eq!(decision.queue_len(), 0);

        // Mark as failed — should be re-queued
        decision.mark_failed(addr, &fetched);
        assert_eq!(decision.queue_len(), 1);
        assert_eq!(decision.total_in_flight(), 0);
    }

    #[test]
    fn rollback_removes_future_ranges() {
        let mut decision = BlockFetchDecision::with_defaults();
        decision.add_range(
            Point::Specific(10, [0x01; 32]),
            Point::Specific(20, [0x02; 32]),
        );
        decision.add_range(
            Point::Specific(100, [0x03; 32]),
            Point::Specific(200, [0x04; 32]),
        );

        assert_eq!(decision.queue_len(), 2);

        // Rollback to slot 50 — should remove the second range
        decision.rollback_to(&Point::Specific(50, [0x05; 32]));
        assert_eq!(decision.queue_len(), 1);
    }

    #[test]
    fn completed_range_removed_from_inflight() {
        let mut decision = BlockFetchDecision::with_defaults();
        decision.add_range(
            Point::Specific(10, [0x01; 32]),
            Point::Specific(20, [0x02; 32]),
        );

        let peers = vec![test_peer(3001, 50.0)];
        let (addr, range) = decision.select_peer(&peers).unwrap();
        assert_eq!(decision.total_in_flight(), 1);

        decision.mark_completed(addr, &range);
        assert_eq!(decision.total_in_flight(), 0);
    }
}
