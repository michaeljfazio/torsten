//! Governor — target-driven peer promotion/demotion decisions.
//!
//! The Governor compares current peer state counts against configured targets
//! and emits actions (promote, demote, discover) to bring the counts in line.
//!
//! ## Churn
//! Periodically rotates peers to prevent stale connections and improve
//! network health (every 10-20 minutes, matching Haskell).

use std::collections::HashSet;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use super::manager::{PeerManager, PeerState};
use super::selection::{
    select_best_cold_eligible, select_best_warm, select_lowest_reputation_cold, select_worst_hot,
    select_worst_warm,
};

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
    /// Minimum interval between hot churn rotations (demote worst hot, promote best warm).
    pub hot_churn_interval: Duration,
    /// Minimum interval between cold churn sweeps (forget lowest-reputation cold peers).
    pub cold_churn_interval: Duration,
    /// Minimum interval between warm churn rotations (demote worst warm, promote best cold).
    pub warm_churn_interval: Duration,
}

impl Default for GovernorConfig {
    fn default() -> Self {
        Self {
            targets: PeerTargets::default(),
            hot_churn_interval: Duration::from_secs(600), // 10 minutes
            cold_churn_interval: Duration::from_secs(900), // 15 minutes
            warm_churn_interval: Duration::from_secs(600), // 10 minutes
        }
    }
}

/// Per-group local root target, passed to the governor so it can
/// independently ensure each group meets its warm/hot valency.
///
/// Matches Haskell's `belowTargetLocal` in `EstablishedPeers.hs` and
/// `ActivePeers.hs`: each local root group is checked independently
/// against its own warm and hot valency targets, regardless of aggregate
/// peer counts.
#[derive(Debug, Clone)]
pub struct LocalRootGroupTarget {
    /// Addresses belonging to this group.
    pub members: HashSet<SocketAddr>,
    /// Target warm (established) peers in this group.
    pub warm_valency: usize,
    /// Target hot (active) peers in this group.
    pub hot_valency: usize,
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
    /// Forget (remove) a cold peer from the registry.
    ///
    /// Emitted during cold churn to evict lowest-reputation non-topology peers
    /// when the cold pool exceeds 150% of `max_cold`.
    ForgetPeer(SocketAddr),
    /// Request peer addresses from a warm peer via PeerSharing protocol.
    ///
    /// Emitted when the cold pool is low and a warm peer with PeerSharing
    /// capability is available. The connection orchestrator should send
    /// `MsgShareRequest` and add discovered routable addresses to the cold set.
    PeerShareRequest(SocketAddr),
}

/// Peer governor — computes actions to bring peer counts to targets.
///
/// Implements three independent churn timers matching Haskell's
/// `Ouroboros.Network.PeerSelection.Governor`:
/// - Hot churn: rotates one hot peer (demote worst, promote best warm)
/// - Cold churn: forgets lowest-reputation cold peers when pool is oversized
/// - Warm churn: rotates warm peers based on quality scoring
pub struct Governor {
    config: GovernorConfig,
    last_hot_churn: Instant,
    last_cold_churn: Instant,
    last_warm_churn: Instant,
    /// Peers for which a cold→warm (connect) promotion is currently in
    /// flight asynchronously.  Cleared by `promotion_cold_completed()`.
    /// Prevents double-promotion across governor ticks when async
    /// connection attempts are slow to complete.
    in_progress_promote_cold: HashSet<SocketAddr>,
    /// Peers for which a warm→hot (activate) promotion is currently in
    /// flight asynchronously.  Cleared by `promotion_warm_completed()`.
    in_progress_promote_warm: HashSet<SocketAddr>,
}

impl Governor {
    /// Create a new governor with the given configuration.
    pub fn new(config: GovernorConfig) -> Self {
        let now = Instant::now();
        Self {
            config,
            last_hot_churn: now,
            last_cold_churn: now,
            last_warm_churn: now,
            in_progress_promote_cold: HashSet::new(),
            in_progress_promote_warm: HashSet::new(),
        }
    }

    /// Signal that a cold→warm promotion has completed (succeeded or failed).
    ///
    /// The caller — typically the connection orchestrator — must invoke this
    /// after every `GovernorAction::PromoteToWarm` attempt regardless of
    /// outcome so the governor can re-evaluate the peer on the next tick.
    pub fn promotion_cold_completed(&mut self, addr: &SocketAddr) {
        self.in_progress_promote_cold.remove(addr);
    }

    /// Signal that a warm→hot promotion has completed (succeeded or failed).
    ///
    /// Must be called after every `GovernorAction::PromoteToHot` attempt.
    pub fn promotion_warm_completed(&mut self, addr: &SocketAddr) {
        self.in_progress_promote_warm.remove(addr);
    }

    /// Compute the actions needed to bring peer counts toward targets.
    ///
    /// This is the main decision function, called periodically by the
    /// connection manager. It evaluates target-driven promotions/demotions
    /// first, then three independent churn timers, then peer discovery.
    pub fn compute_actions(
        &mut self,
        peer_manager: &PeerManager,
        local_root_groups: &[LocalRootGroupTarget],
    ) -> Vec<GovernorAction> {
        use rand::seq::SliceRandom;

        let mut actions = Vec::new();

        let warm_count = peer_manager.count_by_state(PeerState::Warm);
        let hot_count = peer_manager.count_by_state(PeerState::Hot);
        let cold_count = peer_manager.count_by_state(PeerState::Cold);

        // ── Per-group local root promotions (belowTargetLocal) ─────────
        // Each local root group is checked independently against its own
        // warm and hot valency targets, matching Haskell's belowTargetLocal.
        // This runs BEFORE aggregate targets so local root deficiencies are
        // addressed with highest priority — a block producer's relays must
        // always be reconnected immediately.
        let mut already_promoted: HashSet<SocketAddr> = HashSet::new();
        // Compute eligible-to-connect set once (expensive — allocates and filters).
        let eligible_to_connect: HashSet<SocketAddr> = if local_root_groups.is_empty() {
            HashSet::new()
        } else {
            peer_manager
                .peers_eligible_to_connect()
                .into_iter()
                .collect()
        };

        for group in local_root_groups {
            // Count members that are warm or hot (established).
            let warm_or_hot_count = group
                .members
                .iter()
                .filter(|addr| {
                    peer_manager
                        .get_peer(addr)
                        .map(|p| p.state == PeerState::Warm || p.state == PeerState::Hot)
                        .unwrap_or(false)
                })
                .count();

            // Promote cold → warm if below warm_valency.
            if warm_or_hot_count < group.warm_valency {
                let needed = group.warm_valency - warm_or_hot_count;
                let mut promoted = 0;
                for addr in &group.members {
                    if promoted >= needed {
                        break;
                    }
                    if already_promoted.contains(addr) {
                        continue;
                    }
                    // Skip peers whose cold→warm promotion is already in flight.
                    if self.in_progress_promote_cold.contains(addr) {
                        continue;
                    }
                    if eligible_to_connect.contains(addr) {
                        actions.push(GovernorAction::PromoteToWarm(*addr));
                        self.in_progress_promote_cold.insert(*addr);
                        already_promoted.insert(*addr);
                        promoted += 1;
                    }
                }
            }

            // Count members that are hot.
            let hot_member_count = group
                .members
                .iter()
                .filter(|addr| {
                    peer_manager
                        .get_peer(addr)
                        .map(|p| p.state == PeerState::Hot)
                        .unwrap_or(false)
                })
                .count();

            // Promote warm → hot if below hot_valency.
            if hot_member_count < group.hot_valency {
                let needed = group.hot_valency - hot_member_count;
                let mut promoted = 0;
                for addr in &group.members {
                    if promoted >= needed {
                        break;
                    }
                    if already_promoted.contains(addr) {
                        continue;
                    }
                    // Skip peers whose warm→hot promotion is already in flight.
                    if self.in_progress_promote_warm.contains(addr) {
                        continue;
                    }
                    if let Some(peer) = peer_manager.get_peer(addr) {
                        if peer.state == PeerState::Warm {
                            actions.push(GovernorAction::PromoteToHot(*addr));
                            self.in_progress_promote_warm.insert(*addr);
                            already_promoted.insert(*addr);
                            promoted += 1;
                        }
                    }
                }
            }
        }

        // ── Target-driven promotions/demotions ──────────────────────────

        // Promote cold → warm if below target (belowTargetOther cold→warm).
        // Only select peers whose exponential backoff window has elapsed
        // (matches Haskell `availableToConnect` filtered by `nextConnectTimes`).
        // Skip peers already promoted by per-group local root logic above.
        // Also skip topology (local root) peers — they are managed exclusively
        // by the per-group belowTargetLocal path above, matching Haskell's
        // `belowTargetOther` which excludes `LocalRootPeers.keysSet`.
        if warm_count + hot_count < self.config.targets.target_warm {
            use super::manager::PeerSource;

            let needed = self.config.targets.target_warm - (warm_count + hot_count);
            let cold_peers = peer_manager.peers_eligible_to_connect();
            let mut promoted = 0;
            for &addr in &cold_peers {
                if promoted >= needed {
                    break;
                }
                if already_promoted.contains(&addr) {
                    continue;
                }
                // Skip peers with an in-flight cold→warm promotion.
                if self.in_progress_promote_cold.contains(&addr) {
                    continue;
                }
                // Exclude topology peers from aggregate cold→warm promotion.
                if let Some(info) = peer_manager.get_peer(&addr) {
                    if info.source == PeerSource::Topology {
                        continue;
                    }
                }
                actions.push(GovernorAction::PromoteToWarm(addr));
                self.in_progress_promote_cold.insert(addr);
                already_promoted.insert(addr);
                promoted += 1;
            }
        }

        // Promote warm → hot if below target (belowTargetOther warm→hot).
        // Skip peers already promoted by per-group local root logic above.
        // Also skip topology peers — handled exclusively by belowTargetLocal.
        if hot_count < self.config.targets.target_hot {
            use super::manager::PeerSource;

            let needed = self.config.targets.target_hot - hot_count;
            let warm_peers = peer_manager.peers_in_state(PeerState::Warm);
            let mut promoted = 0;
            for &addr in &warm_peers {
                if promoted >= needed {
                    break;
                }
                if already_promoted.contains(&addr) {
                    continue;
                }
                // Skip peers with an in-flight warm→hot promotion.
                if self.in_progress_promote_warm.contains(&addr) {
                    continue;
                }
                // Exclude topology peers from aggregate warm→hot promotion.
                if let Some(info) = peer_manager.get_peer(&addr) {
                    if info.source == PeerSource::Topology {
                        continue;
                    }
                }
                actions.push(GovernorAction::PromoteToHot(addr));
                self.in_progress_promote_warm.insert(addr);
                already_promoted.insert(addr);
                promoted += 1;
            }
        }

        // Demote hot → warm if above target.
        // Topology peers (local roots) are excluded — they must never be
        // demoted, matching Haskell's `Set.\\ LocalRootPeers.keysSet`.
        // Remaining candidates are sorted by score so the worst are demoted first.
        if hot_count > self.config.targets.target_hot {
            use super::manager::PeerSource;
            use super::selection::peer_score;

            let excess = hot_count - self.config.targets.target_hot;
            let mut scored: Vec<(SocketAddr, f64)> = peer_manager
                .peers_in_state(PeerState::Hot)
                .into_iter()
                .filter_map(|addr| {
                    peer_manager.get_peer(&addr).and_then(|info| {
                        if info.source == PeerSource::Topology {
                            return None;
                        }
                        Some((addr, peer_score(info)))
                    })
                })
                .collect();
            scored.sort_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            for (addr, _) in scored.into_iter().take(excess) {
                actions.push(GovernorAction::DemoteToWarm(addr));
            }
        }

        // ── aboveTargetLocal hot→warm for local root groups ────────────
        // When a local root group has MORE hot members than its hotValency,
        // the excess must be demoted. This is the ONLY path that can demote
        // topology (local root) peers — all other demotion paths exclude them.
        // Matches Haskell's `aboveTargetLocal` in `ActivePeers.hs`.
        //
        // The worst-scoring peer in the group is demoted first (ascending sort
        // so index 0 is the lowest score / highest latency).
        {
            use super::selection::peer_score;
            for group in local_root_groups {
                let mut hot_members: Vec<(SocketAddr, f64)> = group
                    .members
                    .iter()
                    .filter_map(|addr| {
                        peer_manager.get_peer(addr).and_then(|info| {
                            if info.state == PeerState::Hot {
                                Some((*addr, peer_score(info)))
                            } else {
                                None
                            }
                        })
                    })
                    .collect();
                if hot_members.len() > group.hot_valency {
                    let excess = hot_members.len() - group.hot_valency;
                    // Sort ascending — lowest score (worst peer) first.
                    hot_members.sort_by(|(_, a), (_, b)| {
                        a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
                    });
                    for (addr, _) in hot_members.into_iter().take(excess) {
                        actions.push(GovernorAction::DemoteToWarm(addr));
                    }
                }
            }
        }

        // ── Hot churn ───────────────────────────────────────────────────
        // Periodically rotate one hot peer to keep the active set fresh.
        // Uses scoring to demote the worst hot peer and promote the best warm.
        if self.last_hot_churn.elapsed() >= self.config.hot_churn_interval && hot_count > 1 {
            if let Some(churn_out) = select_worst_hot(peer_manager) {
                actions.push(GovernorAction::DemoteToWarm(churn_out));
            }
            if let Some(churn_in) = select_best_warm(peer_manager) {
                actions.push(GovernorAction::PromoteToHot(churn_in));
            }
            self.last_hot_churn = Instant::now();
        }

        // ── Cold churn ──────────────────────────────────────────────────
        // Forget lowest-reputation cold peers when the pool exceeds 150% of
        // max_cold. Topology peers are never forgotten (root peers from config).
        if self.last_cold_churn.elapsed() >= self.config.cold_churn_interval {
            let threshold = self.config.targets.max_cold * 3 / 2;
            if cold_count > threshold {
                let excess = cold_count - self.config.targets.max_cold;
                let to_forget = select_lowest_reputation_cold(peer_manager, excess);
                for addr in to_forget {
                    actions.push(GovernorAction::ForgetPeer(addr));
                }
            }
            self.last_cold_churn = Instant::now();
        }

        // ── Warm churn ──────────────────────────────────────────────────
        // Rotate warm peers based on quality: demote worst if above target,
        // promote best cold if below target.
        if self.last_warm_churn.elapsed() >= self.config.warm_churn_interval {
            if warm_count > self.config.targets.target_warm {
                if let Some(worst) = select_worst_warm(peer_manager) {
                    actions.push(GovernorAction::DemoteToCold(worst));
                }
            }
            if warm_count < self.config.targets.target_warm {
                if let Some(best) = select_best_cold_eligible(peer_manager) {
                    actions.push(GovernorAction::PromoteToWarm(best));
                }
            }
            self.last_warm_churn = Instant::now();
        }

        // ── Peer discovery ──────────────────────────────────────────────
        // Request more peers via DNS/ledger and PeerSharing when cold pool is low.
        if cold_count < self.config.targets.max_cold / 2 {
            actions.push(GovernorAction::DiscoverMore);

            // PeerSharing active outreach: ask a random sharing-capable warm peer
            // for addresses. The orchestrator sends MsgShareRequest and adds
            // routable responses to the cold set.
            let sharing_peers = peer_manager.peers_with_peer_sharing(PeerState::Warm);
            if !sharing_peers.is_empty() {
                if let Some(&peer) = sharing_peers.choose(&mut rand::thread_rng()) {
                    actions.push(GovernorAction::PeerShareRequest(peer));
                }
            }
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
            hot_churn_interval: Duration::from_secs(3600),
            cold_churn_interval: Duration::from_secs(3600),
            warm_churn_interval: Duration::from_secs(3600),
        };
        let mut gov = Governor::new(config);

        let actions = gov.compute_actions(&pm, &[]);
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
            hot_churn_interval: Duration::from_secs(3600),
            cold_churn_interval: Duration::from_secs(3600),
            warm_churn_interval: Duration::from_secs(3600),
        };
        let mut gov = Governor::new(config);

        let actions = gov.compute_actions(&pm, &[]);
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
            hot_churn_interval: Duration::from_secs(3600),
            cold_churn_interval: Duration::from_secs(3600),
            warm_churn_interval: Duration::from_secs(3600),
        };
        let mut gov = Governor::new(config);

        let actions = gov.compute_actions(&pm, &[]);
        assert!(actions.contains(&GovernorAction::DiscoverMore));
    }

    #[test]
    fn cold_churn_forgets_lowest_reputation() {
        let mut pm = PeerManager::new();
        // Add 160 cold peers (> 150% of max_cold=50 → threshold=75).
        // Use Dns/Ledger sources so they're eligible for eviction.
        for i in 0..160u16 {
            let source = if i % 2 == 0 {
                PeerSource::Dns
            } else {
                PeerSource::Ledger
            };
            pm.add_peer(test_addr(3000 + i), source);
            // Set reputation proportional to port: lower port = lower reputation.
            pm.get_peer_mut(&test_addr(3000 + i)).unwrap().reputation = i as f64 / 160.0;
        }
        // Add one topology peer with the lowest reputation — must not be forgotten.
        pm.add_peer(test_addr(2999), PeerSource::Topology);
        pm.get_peer_mut(&test_addr(2999)).unwrap().reputation = 0.0;

        let config = GovernorConfig {
            targets: PeerTargets {
                target_warm: 10,
                target_hot: 5,
                max_cold: 50,
            },
            hot_churn_interval: Duration::from_secs(3600),
            // Trigger cold churn immediately.
            cold_churn_interval: Duration::ZERO,
            warm_churn_interval: Duration::from_secs(3600),
        };
        let mut gov = Governor::new(config);
        let actions = gov.compute_actions(&pm, &[]);

        let forget_actions: Vec<_> = actions
            .iter()
            .filter_map(|a| match a {
                GovernorAction::ForgetPeer(addr) => Some(*addr),
                _ => None,
            })
            .collect();

        // Should forget excess peers: 160 cold (non-topology) - 50 max_cold = 110.
        // (topology peer at 2999 doesn't count toward cold_count in peers_in_state).
        // Actually the topology peer IS cold, so cold_count=161, excess=161-50=111.
        assert!(!forget_actions.is_empty());
        // Topology peer must never be forgotten.
        assert!(!forget_actions.contains(&test_addr(2999)));
        // All forgotten peers should be low-reputation.
        for addr in &forget_actions {
            let peer = pm.get_peer(addr).unwrap();
            assert_ne!(peer.source, PeerSource::Topology);
        }
    }

    #[test]
    fn warm_churn_demotes_worst_when_above_target() {
        let mut pm = PeerManager::new();
        // Create 15 warm peers with varying latency (target_warm=10).
        for i in 0..15u16 {
            pm.add_peer(test_addr(3000 + i), PeerSource::Dns);
            pm.promote_to_warm(&test_addr(3000 + i));
            pm.get_peer_mut(&test_addr(3000 + i))
                .unwrap()
                .update_latency((i as f64) * 100.0);
        }

        let config = GovernorConfig {
            targets: PeerTargets {
                target_warm: 10,
                target_hot: 5,
                max_cold: 100,
            },
            hot_churn_interval: Duration::from_secs(3600),
            cold_churn_interval: Duration::from_secs(3600),
            // Trigger warm churn immediately.
            warm_churn_interval: Duration::ZERO,
        };
        let mut gov = Governor::new(config);
        let actions = gov.compute_actions(&pm, &[]);

        let demote_cold: Vec<_> = actions
            .iter()
            .filter_map(|a| match a {
                GovernorAction::DemoteToCold(addr) => Some(*addr),
                _ => None,
            })
            .collect();
        assert_eq!(demote_cold.len(), 1, "should demote exactly one warm peer");
    }

    #[test]
    fn warm_churn_promotes_best_cold_when_below_target() {
        let mut pm = PeerManager::new();
        // 3 warm peers (below target_warm=10).
        for i in 0..3u16 {
            pm.add_peer(test_addr(3000 + i), PeerSource::Dns);
            pm.promote_to_warm(&test_addr(3000 + i));
        }
        // 20 cold peers available for promotion.
        for i in 10..30u16 {
            pm.add_peer(test_addr(3000 + i), PeerSource::Dns);
        }

        let config = GovernorConfig {
            targets: PeerTargets {
                target_warm: 10,
                target_hot: 5,
                max_cold: 100,
            },
            hot_churn_interval: Duration::from_secs(3600),
            cold_churn_interval: Duration::from_secs(3600),
            warm_churn_interval: Duration::ZERO,
        };
        let mut gov = Governor::new(config);
        let actions = gov.compute_actions(&pm, &[]);

        // Warm churn should emit at least one PromoteToWarm (from churn logic).
        // Note: the target-driven logic also emits promotions since warm+hot < target.
        let promote_warm_count = actions
            .iter()
            .filter(|a| matches!(a, GovernorAction::PromoteToWarm(_)))
            .count();
        // Target-driven: needs 10 - (3+0) = 7. Warm churn also adds 1.
        assert!(promote_warm_count >= 7);
    }

    #[test]
    fn peer_share_request_emitted_when_cold_low() {
        let mut pm = PeerManager::new();
        // One warm peer with peer_sharing=true, no cold peers.
        let warm_addr = test_addr(3001);
        pm.add_peer(warm_addr, PeerSource::Dns);
        pm.promote_to_warm(&warm_addr);
        pm.get_peer_mut(&warm_addr).unwrap().peer_sharing = true;

        let config = GovernorConfig {
            targets: PeerTargets {
                target_warm: 1,
                target_hot: 0,
                max_cold: 100,
            },
            hot_churn_interval: Duration::from_secs(3600),
            cold_churn_interval: Duration::from_secs(3600),
            warm_churn_interval: Duration::from_secs(3600),
        };
        let mut gov = Governor::new(config);
        let actions = gov.compute_actions(&pm, &[]);

        assert!(
            actions
                .iter()
                .any(|a| matches!(a, GovernorAction::PeerShareRequest(_))),
            "should emit PeerShareRequest when cold pool is low and sharing peers exist"
        );
    }

    #[test]
    fn peer_share_request_not_emitted_when_no_sharing_peers() {
        let mut pm = PeerManager::new();
        // Warm peer without peer_sharing.
        let warm_addr = test_addr(3001);
        pm.add_peer(warm_addr, PeerSource::Dns);
        pm.promote_to_warm(&warm_addr);
        // peer_sharing defaults to false

        let config = GovernorConfig {
            targets: PeerTargets {
                target_warm: 1,
                target_hot: 0,
                max_cold: 100,
            },
            hot_churn_interval: Duration::from_secs(3600),
            cold_churn_interval: Duration::from_secs(3600),
            warm_churn_interval: Duration::from_secs(3600),
        };
        let mut gov = Governor::new(config);
        let actions = gov.compute_actions(&pm, &[]);

        assert!(
            !actions
                .iter()
                .any(|a| matches!(a, GovernorAction::PeerShareRequest(_))),
            "should not emit PeerShareRequest when no sharing-capable peers"
        );
    }

    #[test]
    fn hot_churn_uses_scoring() {
        let mut pm = PeerManager::new();
        // Create 3 hot peers with different quality.
        for i in 0..3u16 {
            pm.add_peer(test_addr(3000 + i), PeerSource::Dns);
            pm.promote_to_warm(&test_addr(3000 + i));
            pm.promote_to_hot(&test_addr(3000 + i));
        }
        // Make 3002 the worst (highest latency, lowest reputation).
        pm.get_peer_mut(&test_addr(3002))
            .unwrap()
            .update_latency(999.0);
        pm.get_peer_mut(&test_addr(3002)).unwrap().reputation = 0.1;
        // Make 3000 the best.
        pm.get_peer_mut(&test_addr(3000))
            .unwrap()
            .update_latency(5.0);
        pm.get_peer_mut(&test_addr(3000)).unwrap().reputation = 0.9;

        let config = GovernorConfig {
            targets: PeerTargets {
                target_warm: 10,
                target_hot: 5,
                max_cold: 100,
            },
            // Trigger hot churn immediately.
            hot_churn_interval: Duration::ZERO,
            cold_churn_interval: Duration::from_secs(3600),
            warm_churn_interval: Duration::from_secs(3600),
        };
        let mut gov = Governor::new(config);
        let actions = gov.compute_actions(&pm, &[]);

        // The worst hot peer (3002) should be demoted.
        let demoted: Vec<_> = actions
            .iter()
            .filter_map(|a| match a {
                GovernorAction::DemoteToWarm(addr) => Some(*addr),
                _ => None,
            })
            .collect();
        assert!(
            demoted.contains(&test_addr(3002)),
            "worst hot peer should be demoted during churn"
        );
    }

    #[test]
    fn topology_peers_never_demoted_from_hot() {
        let mut pm = PeerManager::new();
        // 3 hot topology peers + 2 hot ledger peers. Target hot = 2.
        for i in 0..3u16 {
            pm.add_peer(test_addr(4000 + i), PeerSource::Topology);
            pm.promote_to_warm(&test_addr(4000 + i));
            pm.promote_to_hot(&test_addr(4000 + i));
        }
        for i in 0..2u16 {
            pm.add_peer(test_addr(5000 + i), PeerSource::Ledger);
            pm.promote_to_warm(&test_addr(5000 + i));
            pm.promote_to_hot(&test_addr(5000 + i));
        }

        let config = GovernorConfig {
            targets: PeerTargets {
                target_warm: 10,
                target_hot: 2,
                max_cold: 100,
            },
            hot_churn_interval: Duration::from_secs(3600),
            cold_churn_interval: Duration::from_secs(3600),
            warm_churn_interval: Duration::from_secs(3600),
        };
        let mut gov = Governor::new(config);
        let actions = gov.compute_actions(&pm, &[]);

        let demoted: Vec<_> = actions
            .iter()
            .filter_map(|a| match a {
                GovernorAction::DemoteToWarm(addr) => Some(*addr),
                _ => None,
            })
            .collect();
        for addr in &demoted {
            let peer = pm.get_peer(addr).unwrap();
            assert_ne!(
                peer.source,
                PeerSource::Topology,
                "topology peer should never be demoted"
            );
        }
    }

    #[test]
    fn topology_peers_never_demoted_from_warm() {
        let mut pm = PeerManager::new();
        for i in 0..5u16 {
            pm.add_peer(test_addr(4000 + i), PeerSource::Topology);
            pm.promote_to_warm(&test_addr(4000 + i));
        }
        for i in 0..10u16 {
            pm.add_peer(test_addr(5000 + i), PeerSource::Ledger);
            pm.promote_to_warm(&test_addr(5000 + i));
        }

        let config = GovernorConfig {
            targets: PeerTargets {
                target_warm: 5,
                target_hot: 0,
                max_cold: 100,
            },
            hot_churn_interval: Duration::from_secs(3600),
            cold_churn_interval: Duration::from_secs(3600),
            warm_churn_interval: Duration::ZERO,
        };
        let mut gov = Governor::new(config);
        let actions = gov.compute_actions(&pm, &[]);

        let demoted: Vec<_> = actions
            .iter()
            .filter_map(|a| match a {
                GovernorAction::DemoteToCold(addr) => Some(*addr),
                _ => None,
            })
            .collect();
        for addr in &demoted {
            let peer = pm.get_peer(addr).unwrap();
            assert_ne!(
                peer.source,
                PeerSource::Topology,
                "topology peer should never be demoted to cold"
            );
        }
    }

    #[test]
    fn below_target_local_promotes_deficient_group() {
        let mut pm = PeerManager::new();
        // Group A: 3 cold peers, warm_valency=2 → needs 2 promotions.
        let group_a_addrs: Vec<SocketAddr> = (0..3u16).map(|i| test_addr(4000 + i)).collect();
        for &addr in &group_a_addrs {
            pm.add_peer(addr, PeerSource::Topology);
        }
        let group_a = LocalRootGroupTarget {
            members: group_a_addrs.iter().copied().collect(),
            warm_valency: 2,
            hot_valency: 0,
        };

        // Group B: 2 peers, 1 already warm, warm_valency=1 → satisfied.
        let group_b_addrs: Vec<SocketAddr> = (0..2u16).map(|i| test_addr(5000 + i)).collect();
        for &addr in &group_b_addrs {
            pm.add_peer(addr, PeerSource::Topology);
        }
        pm.promote_to_warm(&group_b_addrs[0]);
        let group_b = LocalRootGroupTarget {
            members: group_b_addrs.iter().copied().collect(),
            warm_valency: 1,
            hot_valency: 0,
        };

        // Aggregate targets at 0 so they don't interfere.
        let config = GovernorConfig {
            targets: PeerTargets {
                target_warm: 0,
                target_hot: 0,
                max_cold: 100,
            },
            hot_churn_interval: Duration::from_secs(3600),
            cold_churn_interval: Duration::from_secs(3600),
            warm_churn_interval: Duration::from_secs(3600),
        };
        let mut gov = Governor::new(config);
        let actions = gov.compute_actions(&pm, &[group_a, group_b]);

        let promote_warm: Vec<SocketAddr> = actions
            .iter()
            .filter_map(|a| match a {
                GovernorAction::PromoteToWarm(addr) => Some(*addr),
                _ => None,
            })
            .collect();

        // Exactly 2 promotions, all from group A.
        assert_eq!(
            promote_warm.len(),
            2,
            "should promote exactly 2 from group A"
        );
        let group_a_set: HashSet<SocketAddr> = group_a_addrs.iter().copied().collect();
        for addr in &promote_warm {
            assert!(
                group_a_set.contains(addr),
                "promoted peer {addr} should be a group A member"
            );
        }
    }

    #[test]
    fn below_target_local_promotes_warm_to_hot() {
        let mut pm = PeerManager::new();
        // Group: 2 peers, both warm, hot_valency=1 → needs 1 PromoteToHot.
        let group_addrs: Vec<SocketAddr> = (0..2u16).map(|i| test_addr(6000 + i)).collect();
        for &addr in &group_addrs {
            pm.add_peer(addr, PeerSource::Topology);
            pm.promote_to_warm(&addr);
        }
        let group = LocalRootGroupTarget {
            members: group_addrs.iter().copied().collect(),
            warm_valency: 2,
            hot_valency: 1,
        };

        // Aggregate targets at 0.
        let config = GovernorConfig {
            targets: PeerTargets {
                target_warm: 0,
                target_hot: 0,
                max_cold: 100,
            },
            hot_churn_interval: Duration::from_secs(3600),
            cold_churn_interval: Duration::from_secs(3600),
            warm_churn_interval: Duration::from_secs(3600),
        };
        let mut gov = Governor::new(config);
        let actions = gov.compute_actions(&pm, &[group]);

        let promote_hot: Vec<SocketAddr> = actions
            .iter()
            .filter_map(|a| match a {
                GovernorAction::PromoteToHot(addr) => Some(*addr),
                _ => None,
            })
            .collect();

        assert_eq!(promote_hot.len(), 1, "should promote exactly 1 warm to hot");
        let group_set: HashSet<SocketAddr> = group_addrs.iter().copied().collect();
        assert!(
            group_set.contains(&promote_hot[0]),
            "promoted peer should be a group member"
        );
    }

    /// Aggregate cold→warm (belowTargetOther) must NOT promote topology peers —
    /// they are managed exclusively by the per-group belowTargetLocal path.
    /// This mirrors Haskell's `belowTargetOther` which excludes
    /// `LocalRootPeers.keysSet` from its candidate set.
    #[test]
    fn aggregate_warm_promotion_excludes_topology_peers() {
        let mut pm = PeerManager::new();

        // One topology cold peer — must NOT be promoted by aggregate logic.
        let topo_addr = test_addr(7000);
        pm.add_peer(topo_addr, PeerSource::Topology);

        // One ledger cold peer — eligible for aggregate promotion.
        let ledger_addr = test_addr(7001);
        pm.add_peer(ledger_addr, PeerSource::Ledger);

        // target_warm=2, but aggregate logic must skip topology.
        // No local root groups supplied → belowTargetLocal emits nothing.
        let config = GovernorConfig {
            targets: PeerTargets {
                target_warm: 2,
                target_hot: 0,
                max_cold: 100,
            },
            hot_churn_interval: Duration::from_secs(3600),
            cold_churn_interval: Duration::from_secs(3600),
            warm_churn_interval: Duration::from_secs(3600),
        };
        let mut gov = Governor::new(config);
        let actions = gov.compute_actions(&pm, &[]);

        let promoted: Vec<SocketAddr> = actions
            .iter()
            .filter_map(|a| match a {
                GovernorAction::PromoteToWarm(addr) => Some(*addr),
                _ => None,
            })
            .collect();

        // Only the ledger peer may be promoted; the topology peer must be skipped.
        assert!(
            !promoted.contains(&topo_addr),
            "aggregate path must not promote topology peer"
        );
        assert!(
            promoted.contains(&ledger_addr),
            "aggregate path should promote the ledger peer"
        );
        assert_eq!(promoted.len(), 1, "exactly 1 promotion: ledger peer only");
    }

    /// Peers with an in-flight cold→warm promotion must not be promoted again
    /// on the next governor tick before `promotion_cold_completed()` is called.
    /// This prevents duplicate connection attempts when async promotions are slow.
    #[test]
    fn in_progress_cold_promotion_not_duplicated() {
        let mut pm = PeerManager::new();

        // Two ledger cold peers, both eligible.
        let addr_a = test_addr(8000);
        let addr_b = test_addr(8001);
        pm.add_peer(addr_a, PeerSource::Ledger);
        pm.add_peer(addr_b, PeerSource::Ledger);

        let config = GovernorConfig {
            targets: PeerTargets {
                target_warm: 2,
                target_hot: 0,
                max_cold: 100,
            },
            hot_churn_interval: Duration::from_secs(3600),
            cold_churn_interval: Duration::from_secs(3600),
            warm_churn_interval: Duration::from_secs(3600),
        };
        let mut gov = Governor::new(config);

        // First tick — both peers are promoted (in-progress sets are populated).
        let actions1 = gov.compute_actions(&pm, &[]);
        let promoted1: Vec<SocketAddr> = actions1
            .iter()
            .filter_map(|a| match a {
                GovernorAction::PromoteToWarm(addr) => Some(*addr),
                _ => None,
            })
            .collect();
        assert_eq!(promoted1.len(), 2, "first tick should promote both peers");

        // Second tick without completing any promotions — must emit nothing new.
        // The peers are still cold (PeerManager not updated) but in-progress sets
        // prevent re-emission.
        let actions2 = gov.compute_actions(&pm, &[]);
        let promoted2: Vec<SocketAddr> = actions2
            .iter()
            .filter_map(|a| match a {
                GovernorAction::PromoteToWarm(addr) => Some(*addr),
                _ => None,
            })
            .collect();
        assert!(
            promoted2.is_empty(),
            "second tick must not re-emit PromoteToWarm while promotions are in flight"
        );

        // After completing addr_a's promotion the governor may re-evaluate it.
        gov.promotion_cold_completed(&addr_a);
        let actions3 = gov.compute_actions(&pm, &[]);
        let promoted3: Vec<SocketAddr> = actions3
            .iter()
            .filter_map(|a| match a {
                GovernorAction::PromoteToWarm(addr) => Some(*addr),
                _ => None,
            })
            .collect();
        // addr_a is eligible again; addr_b is still in-flight.
        assert_eq!(promoted3.len(), 1);
        assert_eq!(promoted3[0], addr_a);
    }

    /// Same guard for warm→hot: peers with an in-flight promotion must not be
    /// re-promoted on subsequent ticks.
    #[test]
    fn in_progress_warm_promotion_not_duplicated() {
        let mut pm = PeerManager::new();

        // Two warm ledger peers.
        let addr_a = test_addr(8100);
        let addr_b = test_addr(8101);
        pm.add_peer(addr_a, PeerSource::Ledger);
        pm.promote_to_warm(&addr_a);
        pm.add_peer(addr_b, PeerSource::Ledger);
        pm.promote_to_warm(&addr_b);

        let config = GovernorConfig {
            targets: PeerTargets {
                target_warm: 2,
                target_hot: 2,
                max_cold: 100,
            },
            hot_churn_interval: Duration::from_secs(3600),
            cold_churn_interval: Duration::from_secs(3600),
            warm_churn_interval: Duration::from_secs(3600),
        };
        let mut gov = Governor::new(config);

        // First tick — both warm peers get PromoteToHot.
        let actions1 = gov.compute_actions(&pm, &[]);
        let promoted1: Vec<SocketAddr> = actions1
            .iter()
            .filter_map(|a| match a {
                GovernorAction::PromoteToHot(addr) => Some(*addr),
                _ => None,
            })
            .collect();
        assert_eq!(promoted1.len(), 2, "first tick should promote both to hot");

        // Second tick without update — must not re-emit.
        let actions2 = gov.compute_actions(&pm, &[]);
        let promoted2: Vec<SocketAddr> = actions2
            .iter()
            .filter_map(|a| match a {
                GovernorAction::PromoteToHot(addr) => Some(*addr),
                _ => None,
            })
            .collect();
        assert!(
            promoted2.is_empty(),
            "second tick must not re-emit PromoteToHot while promotions are in flight"
        );

        // After completing addr_b, it becomes eligible again.
        gov.promotion_warm_completed(&addr_b);
        let actions3 = gov.compute_actions(&pm, &[]);
        let promoted3: Vec<SocketAddr> = actions3
            .iter()
            .filter_map(|a| match a {
                GovernorAction::PromoteToHot(addr) => Some(*addr),
                _ => None,
            })
            .collect();
        assert_eq!(promoted3.len(), 1);
        assert_eq!(promoted3[0], addr_b);
    }

    /// aboveTargetLocal: when a local root group has more hot members than
    /// its hotValency, the excess (worst-scoring) must be demoted to warm.
    /// This is the ONLY path that can demote topology peers.
    #[test]
    fn above_target_local_hot_demotes_excess() {
        let mut pm = PeerManager::new();

        // Two topology peers both promoted to Hot.
        let good_addr = test_addr(9000); // low latency → high score → keep
        let bad_addr = test_addr(9001); // high latency → low score → demote

        pm.add_peer(good_addr, PeerSource::Topology);
        pm.promote_to_warm(&good_addr);
        pm.promote_to_hot(&good_addr);
        pm.get_peer_mut(&good_addr).unwrap().update_latency(5.0);
        pm.get_peer_mut(&good_addr).unwrap().reputation = 0.9;

        pm.add_peer(bad_addr, PeerSource::Topology);
        pm.promote_to_warm(&bad_addr);
        pm.promote_to_hot(&bad_addr);
        pm.get_peer_mut(&bad_addr).unwrap().update_latency(999.0);
        pm.get_peer_mut(&bad_addr).unwrap().reputation = 0.1;

        // Group hot_valency=1 → 2 hot members, excess=1.
        let group = LocalRootGroupTarget {
            members: [good_addr, bad_addr].iter().copied().collect(),
            warm_valency: 2,
            hot_valency: 1,
        };

        // Aggregate targets high so they don't interfere.
        let config = GovernorConfig {
            targets: PeerTargets {
                target_warm: 10,
                target_hot: 10,
                max_cold: 100,
            },
            hot_churn_interval: Duration::from_secs(3600),
            cold_churn_interval: Duration::from_secs(3600),
            warm_churn_interval: Duration::from_secs(3600),
        };
        let mut gov = Governor::new(config);
        let actions = gov.compute_actions(&pm, &[group]);

        let demoted: Vec<SocketAddr> = actions
            .iter()
            .filter_map(|a| match a {
                GovernorAction::DemoteToWarm(addr) => Some(*addr),
                _ => None,
            })
            .collect();

        // Exactly one demotion: the worst-scoring peer.
        assert_eq!(demoted.len(), 1, "should demote exactly 1 excess hot peer");
        assert_eq!(
            demoted[0], bad_addr,
            "the worse-scoring peer should be demoted"
        );
        // The good peer must not be demoted.
        assert!(
            !demoted.contains(&good_addr),
            "better-scoring topology peer must be retained"
        );
    }
}
