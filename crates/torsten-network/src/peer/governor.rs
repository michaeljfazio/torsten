//! Governor — target-driven peer promotion/demotion decisions.
//!
//! The Governor compares current peer state counts against configured targets
//! and emits actions (promote, demote, discover) to bring the counts in line.
//!
//! ## Churn
//! Periodically rotates peers to prevent stale connections and improve
//! network health (every 10-20 minutes, matching Haskell).

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use super::manager::{PeerManager, PeerState};

/// Target peer counts for the governor.
#[derive(Debug, Clone)]
pub struct PeerTargets {
    /// Target number of warm peers (TCP connected, keepalive).
    pub target_warm: usize,
    /// Target number of hot peers (fully syncing).
    pub target_hot: usize,
    /// Maximum number of cold peers to track.
    pub max_cold: usize,
}

impl Default for PeerTargets {
    fn default() -> Self {
        Self {
            target_warm: 10,
            target_hot: 5,
            max_cold: 100,
        }
    }
}

/// Governor configuration.
#[derive(Debug, Clone)]
pub struct GovernorConfig {
    /// Peer count targets.
    pub targets: PeerTargets,
    /// Minimum interval between churn rotations.
    pub churn_interval: Duration,
}

impl Default for GovernorConfig {
    fn default() -> Self {
        Self {
            targets: PeerTargets::default(),
            churn_interval: Duration::from_secs(600), // 10 minutes
        }
    }
}

/// Actions the governor can emit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GovernorAction {
    /// Promote a cold peer to warm (establish TCP connection).
    PromoteToWarm(SocketAddr),
    /// Promote a warm peer to hot (start sync protocols).
    PromoteToHot(SocketAddr),
    /// Demote a hot peer to warm (stop sync protocols).
    DemoteToWarm(SocketAddr),
    /// Demote a warm peer to cold (close TCP connection).
    DemoteToCold(SocketAddr),
    /// Discover more peers (not enough cold peers).
    DiscoverMore,
}

/// Peer governor — computes actions to bring peer counts to targets.
pub struct Governor {
    config: GovernorConfig,
    last_churn: Instant,
}

impl Governor {
    /// Create a new governor with the given configuration.
    pub fn new(config: GovernorConfig) -> Self {
        Self {
            config,
            last_churn: Instant::now(),
        }
    }

    /// Compute the actions needed to bring peer counts toward targets.
    ///
    /// This is the main decision function, called periodically by the
    /// connection manager.
    pub fn compute_actions(&mut self, peer_manager: &PeerManager) -> Vec<GovernorAction> {
        let mut actions = Vec::new();

        let warm_count = peer_manager.count_by_state(PeerState::Warm);
        let hot_count = peer_manager.count_by_state(PeerState::Hot);
        let cold_count = peer_manager.count_by_state(PeerState::Cold);

        // Promote cold → warm if below target.
        // Only select peers whose exponential backoff window has elapsed
        // (matches Haskell `availableToConnect` filtered by `nextConnectTimes`).
        if warm_count + hot_count < self.config.targets.target_warm {
            let needed = self.config.targets.target_warm - (warm_count + hot_count);
            let cold_peers = peer_manager.peers_eligible_to_connect();
            for &addr in cold_peers.iter().take(needed) {
                actions.push(GovernorAction::PromoteToWarm(addr));
            }
        }

        // Promote warm → hot if below target
        if hot_count < self.config.targets.target_hot {
            let needed = self.config.targets.target_hot - hot_count;
            let warm_peers = peer_manager.peers_in_state(PeerState::Warm);
            for &addr in warm_peers.iter().take(needed) {
                actions.push(GovernorAction::PromoteToHot(addr));
            }
        }

        // Demote hot → warm if above target
        if hot_count > self.config.targets.target_hot {
            let excess = hot_count - self.config.targets.target_hot;
            let hot_peers = peer_manager.peers_in_state(PeerState::Hot);
            for &addr in hot_peers.iter().take(excess) {
                actions.push(GovernorAction::DemoteToWarm(addr));
            }
        }

        // Discover more peers if cold pool is low
        if cold_count < self.config.targets.max_cold / 2 {
            actions.push(GovernorAction::DiscoverMore);
        }

        // Churn rotation
        if self.last_churn.elapsed() >= self.config.churn_interval && hot_count > 1 {
            // Demote one hot peer and promote a warm one
            let hot_peers = peer_manager.peers_in_state(PeerState::Hot);
            if let Some(&churn_out) = hot_peers.first() {
                actions.push(GovernorAction::DemoteToWarm(churn_out));
            }
            let warm_peers = peer_manager.peers_in_state(PeerState::Warm);
            if let Some(&churn_in) = warm_peers.first() {
                actions.push(GovernorAction::PromoteToHot(churn_in));
            }
            self.last_churn = Instant::now();
        }

        actions
    }
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
    fn promotes_cold_to_warm_when_below_target() {
        let mut pm = PeerManager::new();
        for i in 0..5u16 {
            pm.add_peer(test_addr(3000 + i), PeerSource::Dns);
        }

        let config = GovernorConfig {
            targets: PeerTargets {
                target_warm: 3,
                target_hot: 1,
                max_cold: 100,
            },
            churn_interval: Duration::from_secs(3600),
        };
        let mut gov = Governor::new(config);

        let actions = gov.compute_actions(&pm);
        let promote_warm_count = actions
            .iter()
            .filter(|a| matches!(a, GovernorAction::PromoteToWarm(_)))
            .count();
        assert_eq!(promote_warm_count, 3);
    }

    #[test]
    fn promotes_warm_to_hot_when_below_target() {
        let mut pm = PeerManager::new();
        for i in 0..5u16 {
            pm.add_peer(test_addr(3000 + i), PeerSource::Dns);
            pm.promote_to_warm(&test_addr(3000 + i));
        }

        let config = GovernorConfig {
            targets: PeerTargets {
                target_warm: 5,
                target_hot: 2,
                max_cold: 100,
            },
            churn_interval: Duration::from_secs(3600),
        };
        let mut gov = Governor::new(config);

        let actions = gov.compute_actions(&pm);
        let promote_hot_count = actions
            .iter()
            .filter(|a| matches!(a, GovernorAction::PromoteToHot(_)))
            .count();
        assert_eq!(promote_hot_count, 2);
    }

    #[test]
    fn discover_when_cold_pool_low() {
        let pm = PeerManager::new(); // empty = 0 cold peers

        let config = GovernorConfig {
            targets: PeerTargets {
                target_warm: 3,
                target_hot: 1,
                max_cold: 100,
            },
            churn_interval: Duration::from_secs(3600),
        };
        let mut gov = Governor::new(config);

        let actions = gov.compute_actions(&pm);
        assert!(actions.contains(&GovernorAction::DiscoverMore));
    }
}
