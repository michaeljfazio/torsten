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

/// Select the lowest-reputation cold peers for eviction during cold churn.
///
/// Returns up to `count` cold peers sorted by reputation ascending.
/// Topology peers are excluded — root peers configured in the topology file
/// should never be forgotten.
pub fn select_lowest_reputation_cold(peer_manager: &PeerManager, count: usize) -> Vec<SocketAddr> {
    use super::manager::PeerSource;

    let mut scored: Vec<(SocketAddr, f64)> = peer_manager
        .peers_in_state(PeerState::Cold)
        .into_iter()
        .filter_map(|addr| {
            peer_manager.get_peer(&addr).and_then(|info| {
                // Never forget topology peers (root peers from config file).
                if info.source == PeerSource::Topology {
                    return None;
                }
                Some((addr, info.reputation))
            })
        })
        .collect();

    // Sort ascending by reputation (lowest first = worst candidates).
    scored.sort_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    scored
        .into_iter()
        .take(count)
        .map(|(addr, _)| addr)
        .collect()
}

/// Select the worst warm peer for demotion to cold during warm churn.
///
/// Returns the warm peer with the lowest composite score.
pub fn select_worst_warm(peer_manager: &PeerManager) -> Option<SocketAddr> {
    peer_manager
        .peers_in_state(PeerState::Warm)
        .into_iter()
        .filter_map(|addr| {
            peer_manager
                .get_peer(&addr)
                .map(|info| (addr, peer_score(info)))
        })
        .min_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(addr, _)| addr)
}

/// Select the best warm peer for promotion to hot during hot churn.
///
/// Returns the warm peer with the highest composite score.
pub fn select_best_warm(peer_manager: &PeerManager) -> Option<SocketAddr> {
    peer_manager
        .peers_in_state(PeerState::Warm)
        .into_iter()
        .filter_map(|addr| {
            peer_manager
                .get_peer(&addr)
                .map(|info| (addr, peer_score(info)))
        })
        .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(addr, _)| addr)
}

/// Select the best cold peer eligible for promotion (backoff elapsed).
///
/// Like [`select_best_cold`] but only considers peers whose exponential
/// backoff window has passed, matching what the Governor should use for
/// cold→warm promotions during warm churn.
pub fn select_best_cold_eligible(peer_manager: &PeerManager) -> Option<SocketAddr> {
    let eligible: std::collections::HashSet<SocketAddr> = peer_manager
        .peers_eligible_to_connect()
        .into_iter()
        .collect();

    peer_manager
        .peers_in_state(PeerState::Cold)
        .into_iter()
        .filter(|addr| eligible.contains(addr))
        .filter_map(|addr| {
            peer_manager
                .get_peer(&addr)
                .map(|info| (addr, peer_score(info)))
        })
        .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
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

    #[test]
    fn select_lowest_reputation_cold_excludes_topology() {
        let mut pm = PeerManager::new();
        // Topology peer with very low reputation — should never be returned.
        pm.add_peer(test_addr(3001), PeerSource::Topology);
        pm.get_peer_mut(&test_addr(3001)).unwrap().reputation = 0.0;

        // DNS peers with low reputation — eligible for eviction.
        pm.add_peer(test_addr(3002), PeerSource::Dns);
        pm.get_peer_mut(&test_addr(3002)).unwrap().reputation = 0.1;
        pm.add_peer(test_addr(3003), PeerSource::Dns);
        pm.get_peer_mut(&test_addr(3003)).unwrap().reputation = 0.2;
        pm.add_peer(test_addr(3004), PeerSource::Ledger);
        pm.get_peer_mut(&test_addr(3004)).unwrap().reputation = 0.3;

        let evict = select_lowest_reputation_cold(&pm, 2);
        assert_eq!(evict.len(), 2);
        // Should be the two lowest non-topology peers.
        assert!(evict.contains(&test_addr(3002)));
        assert!(evict.contains(&test_addr(3003)));
        // Topology peer must not appear.
        assert!(!evict.contains(&test_addr(3001)));
    }

    #[test]
    fn select_worst_warm_returns_lowest_score() {
        let mut pm = PeerManager::new();
        pm.add_peer(test_addr(3001), PeerSource::Dns);
        pm.promote_to_warm(&test_addr(3001));
        pm.get_peer_mut(&test_addr(3001))
            .unwrap()
            .update_latency(10.0); // fast

        pm.add_peer(test_addr(3002), PeerSource::Dns);
        pm.promote_to_warm(&test_addr(3002));
        pm.get_peer_mut(&test_addr(3002))
            .unwrap()
            .update_latency(999.0); // slow

        let worst = select_worst_warm(&pm).unwrap();
        assert_eq!(worst, test_addr(3002));
    }

    #[test]
    fn select_best_warm_returns_highest_score() {
        let mut pm = PeerManager::new();
        pm.add_peer(test_addr(3001), PeerSource::Dns);
        pm.promote_to_warm(&test_addr(3001));
        pm.get_peer_mut(&test_addr(3001))
            .unwrap()
            .update_latency(10.0); // fast

        pm.add_peer(test_addr(3002), PeerSource::Dns);
        pm.promote_to_warm(&test_addr(3002));
        pm.get_peer_mut(&test_addr(3002))
            .unwrap()
            .update_latency(999.0); // slow

        let best = select_best_warm(&pm).unwrap();
        assert_eq!(best, test_addr(3001));
    }

    #[test]
    fn select_best_cold_eligible_respects_backoff() {
        let mut pm = PeerManager::new();
        pm.add_peer(test_addr(3001), PeerSource::Dns);
        pm.add_peer(test_addr(3002), PeerSource::Dns);

        // 3001 has better score (lower latency)
        pm.get_peer_mut(&test_addr(3001))
            .unwrap()
            .update_latency(10.0);
        pm.get_peer_mut(&test_addr(3002))
            .unwrap()
            .update_latency(100.0);

        // But 3001 is in backoff window.
        pm.get_peer_mut(&test_addr(3001)).unwrap().record_failure();

        // Should pick 3002 (only eligible peer).
        let best = select_best_cold_eligible(&pm).unwrap();
        assert_eq!(best, test_addr(3002));
    }
}
