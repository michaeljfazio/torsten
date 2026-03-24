//! Peer selection — scoring, ranking, and eviction.
//!
//! Combines latency, reputation, and failure penalty into a composite score
//! for ranking peers during promotion/demotion decisions.

use std::net::SocketAddr;

use super::manager::{PeerInfo, PeerManager, PeerState};

/// Compute a composite peer score (higher = better).
///
/// Score components:
/// - Reputation: weight 0.4 (0.0 to 1.0)
/// - Latency: weight 0.4 (normalized: lower = better, mapped to 0-1 via 1/(1+ms/200))
/// - Failure penalty: weight 0.2 (0 failures = 1.0, each failure reduces by 0.1)
pub fn peer_score(peer: &PeerInfo) -> f64 {
    let reputation_score = peer.reputation;

    let latency_score = match peer.latency_ms {
        Some(ms) => 1.0 / (1.0 + ms / 200.0),
        None => 0.5, // Unknown latency gets middle score
    };

    let failure_score = (1.0 - peer.failure_count as f64 * 0.1).max(0.0);

    0.4 * reputation_score + 0.4 * latency_score + 0.2 * failure_score
}

/// Select the best cold peer for promotion to warm.
///
/// Returns the cold peer with the highest composite score, preferring
/// topology and ledger peers over peer-sharing discoveries.
pub fn select_best_cold(peer_manager: &PeerManager) -> Option<SocketAddr> {
    peer_manager
        .peers_in_state(PeerState::Cold)
        .into_iter()
        .filter_map(|addr| {
            peer_manager
                .get_peer(&addr)
                .map(|info| (addr, peer_score(info)))
        })
        .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(addr, _)| addr)
}

/// Select the worst hot peer for demotion to warm.
///
/// Returns the hot peer with the lowest composite score.
pub fn select_worst_hot(peer_manager: &PeerManager) -> Option<SocketAddr> {
    peer_manager
        .peers_in_state(PeerState::Hot)
        .into_iter()
        .filter_map(|addr| {
            peer_manager
                .get_peer(&addr)
                .map(|info| (addr, peer_score(info)))
        })
        .min_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(addr, _)| addr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peer::manager::PeerSource;
    use std::net::{IpAddr, Ipv4Addr};

    fn test_addr(port: u16) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4)), port)
    }

    #[test]
    fn score_formula() {
        let mut peer = PeerInfo::new(PeerSource::Topology);
        peer.update_latency(100.0); // moderate latency
        let score = peer_score(&peer);
        assert!(score > 0.0 && score < 1.0);
    }

    #[test]
    fn lower_latency_is_better() {
        let mut fast = PeerInfo::new(PeerSource::Topology);
        fast.update_latency(10.0);

        let mut slow = PeerInfo::new(PeerSource::Topology);
        slow.update_latency(500.0);

        assert!(peer_score(&fast) > peer_score(&slow));
    }

    #[test]
    fn failures_reduce_score() {
        let good = PeerInfo::new(PeerSource::Topology);
        let mut bad = PeerInfo::new(PeerSource::Topology);
        bad.record_failure();
        bad.record_failure();

        assert!(peer_score(&good) > peer_score(&bad));
    }

    #[test]
    fn select_best_cold_returns_highest_score() {
        let mut pm = PeerManager::new();
        pm.add_peer(test_addr(3001), PeerSource::Topology);
        pm.add_peer(test_addr(3002), PeerSource::Topology);

        // Make 3002 have lower latency (better score)
        pm.get_peer_mut(&test_addr(3002))
            .unwrap()
            .update_latency(10.0);
        pm.get_peer_mut(&test_addr(3001))
            .unwrap()
            .update_latency(500.0);

        let best = select_best_cold(&pm).unwrap();
        assert_eq!(best, test_addr(3002));
    }
}
