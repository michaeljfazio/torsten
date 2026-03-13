//! P2P peer selection governor.
//!
//! Implements Haskell cardano-node's target-driven peer management model.
//! The governor evaluates peer deficit/surplus across categories and emits
//! events (Connect, Disconnect, Promote, Demote) to drive the node's
//! connection management.
//!
//! Key features:
//! - Separate targets for regular and big ledger peers
//! - Local root protection (never demoted even above target)
//! - Sync-state-aware target switching
//! - Periodic churn for peer rotation
//! - Connection limit enforcement (hard/soft)

use crate::peer_manager::{PeerCategory, PeerManager};
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tracing::{debug, info};

/// Peer count targets for the governor decision loop.
#[derive(Debug, Clone)]
pub struct PeerTargets {
    /// Target number of known root peers
    pub root_peers: usize,
    /// Target number of total known peers
    pub known_peers: usize,
    /// Target number of established (warm + hot) peers
    pub established_peers: usize,
    /// Target number of active (hot) peers
    pub active_peers: usize,
    /// Target number of known big ledger peers
    pub known_blp: usize,
    /// Target number of established big ledger peers
    pub established_blp: usize,
    /// Target number of active big ledger peers
    pub active_blp: usize,
}

impl Default for PeerTargets {
    fn default() -> Self {
        PeerTargets {
            root_peers: 10,
            known_peers: 150,
            established_peers: 30,
            active_peers: 20,
            known_blp: 15,
            established_blp: 10,
            active_blp: 5,
        }
    }
}

/// Targets used during syncing (lower than normal for faster convergence)
impl PeerTargets {
    pub fn syncing() -> Self {
        PeerTargets {
            root_peers: 5,
            known_peers: 50,
            established_peers: 15,
            active_peers: 10,
            known_blp: 15,
            established_blp: 10,
            active_blp: 5,
        }
    }
}

/// Events emitted by the governor for the node to act on.
#[derive(Debug, Clone)]
pub enum GovernorEvent {
    /// Connect to a cold peer (promote to warm)
    Connect(SocketAddr),
    /// Disconnect from a peer (demote to cold)
    Disconnect(SocketAddr),
    /// Promote a warm peer to hot (start syncing)
    Promote(SocketAddr),
    /// Demote a hot peer to warm (stop syncing)
    Demote(SocketAddr),
}

/// Sync state hint from the node — determines which targets to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncState {
    /// Waiting for enough trusted peers (HAA)
    PreSyncing,
    /// Active download with LoE/GDD protection
    Syncing,
    /// Normal Praos operation at chain tip
    CaughtUp,
}

/// Governor configuration
#[derive(Debug, Clone)]
pub struct GovernorConfig {
    /// Targets for normal (CaughtUp) operation
    pub normal_targets: PeerTargets,
    /// Targets during syncing
    pub sync_targets: PeerTargets,
    /// Hard connection limit — reject above this
    pub hard_limit: usize,
    /// Soft connection limit — delay acceptance above this
    pub soft_limit: usize,
    /// Normal churn interval (seconds)
    pub churn_interval_normal_secs: u64,
    /// Sync churn interval (seconds)
    pub churn_interval_sync_secs: u64,
}

impl Default for GovernorConfig {
    fn default() -> Self {
        GovernorConfig {
            normal_targets: PeerTargets::default(),
            sync_targets: PeerTargets::syncing(),
            hard_limit: 512,
            soft_limit: 384,
            churn_interval_normal_secs: 3300, // 55 minutes
            churn_interval_sync_secs: 900,    // 15 minutes
        }
    }
}

/// The P2P peer selection governor.
///
/// Evaluates peer counts against targets and produces events to drive
/// the node's connection management. The governor runs as a periodic
/// task, examining the PeerManager state and emitting events.
pub struct Governor {
    config: GovernorConfig,
    sync_state: SyncState,
    /// Tracks when churn was last performed
    last_churn: Instant,
    /// Whether churn is currently in the "reduced" phase
    churn_active: bool,
    /// Saved targets during churn (restored after churn completes)
    pre_churn_targets: Option<PeerTargets>,
}

impl Governor {
    pub fn new(config: GovernorConfig) -> Self {
        Governor {
            config,
            sync_state: SyncState::CaughtUp,
            last_churn: Instant::now(),
            churn_active: false,
            pre_churn_targets: None,
        }
    }

    /// Update the sync state (called by the node when GSM transitions)
    pub fn set_sync_state(&mut self, state: SyncState) {
        if self.sync_state != state {
            info!(?state, "Governor: sync state changed");
            self.sync_state = state;
        }
    }

    /// Get current sync state
    pub fn sync_state(&self) -> SyncState {
        self.sync_state
    }

    /// Get the active targets based on current sync state
    fn active_targets(&self) -> &PeerTargets {
        match self.sync_state {
            SyncState::PreSyncing | SyncState::Syncing => &self.config.sync_targets,
            SyncState::CaughtUp => &self.config.normal_targets,
        }
    }

    /// Check if a new connection should be accepted based on connection limits.
    /// Returns `None` if accepted, `Some(delay)` if should delay, or panics above hard limit.
    pub fn connection_check(&self, total_connections: usize) -> ConnectionDecision {
        if total_connections >= self.config.hard_limit {
            ConnectionDecision::Reject
        } else if total_connections >= self.config.soft_limit {
            ConnectionDecision::Delay(Duration::from_secs(5))
        } else {
            ConnectionDecision::Accept
        }
    }

    /// Run one evaluation cycle of the governor decision loop.
    ///
    /// Examines the PeerManager state, compares against targets, and returns
    /// a list of events that should be executed. Events are prioritized:
    /// 1. Big ledger peers (most important for genesis/sync)
    /// 2. Active peers below target → promote
    /// 3. Established peers below target → connect
    /// 4. Above target → demote/disconnect (respecting local root protection)
    pub fn evaluate(&mut self, pm: &PeerManager) -> Vec<GovernorEvent> {
        let mut events = Vec::new();
        let targets = self.active_targets().clone();

        // --- Phase 1: Big Ledger Peers ---
        self.evaluate_blp(pm, &targets, &mut events);

        // --- Phase 2: Active peers (hot) ---
        self.evaluate_active(pm, &targets, &mut events);

        // --- Phase 3: Established peers (warm + hot) ---
        self.evaluate_established(pm, &targets, &mut events);

        // --- Phase 4: Known peers (connect cold) ---
        self.evaluate_known(pm, &targets, &mut events);

        // --- Phase 5: Surplus reduction ---
        self.evaluate_surplus(pm, &targets, &mut events);

        if !events.is_empty() {
            debug!(
                event_count = events.len(),
                hot = pm.hot_peer_count(),
                warm = pm.warm_peer_count(),
                cold = pm.cold_peer_count(),
                "Governor: evaluation produced events"
            );
        }

        events
    }

    /// Evaluate big ledger peer targets
    fn evaluate_blp(
        &self,
        pm: &PeerManager,
        targets: &PeerTargets,
        events: &mut Vec<GovernorEvent>,
    ) {
        let active_blp = pm.active_big_ledger_peer_count();

        if active_blp < targets.active_blp {
            // Need more active BLPs — promote warm BLPs
            let deficit = targets.active_blp - active_blp;
            let warm_blps: Vec<SocketAddr> = pm
                .connected_peer_addrs()
                .into_iter()
                .filter(|addr| {
                    pm.peer_category(addr) == Some(PeerCategory::BigLedgerPeer)
                        && pm.peer_performance(addr).is_some_and(|_| true) // warm, not hot
                })
                .filter(|addr| {
                    // Only warm peers (not already hot)
                    !pm.hot_peer_addrs().contains(addr)
                })
                .take(deficit)
                .collect();

            for addr in warm_blps {
                events.push(GovernorEvent::Promote(addr));
            }

            // If still short, connect cold BLPs
            if active_blp + events.len() < targets.active_blp {
                // The node will handle connecting cold BLPs through the regular connect path
                // We just ensure they get prioritized
            }
        }
    }

    /// Evaluate active (hot) peer targets
    fn evaluate_active(
        &self,
        pm: &PeerManager,
        targets: &PeerTargets,
        events: &mut Vec<GovernorEvent>,
    ) {
        let hot = pm.hot_peer_count();
        if hot < targets.active_peers {
            let deficit = targets.active_peers - hot;
            let to_promote = pm.peers_to_promote();
            for addr in to_promote.into_iter().take(deficit) {
                events.push(GovernorEvent::Promote(addr));
            }
        }
    }

    /// Evaluate established (warm + hot) peer targets
    fn evaluate_established(
        &self,
        pm: &PeerManager,
        targets: &PeerTargets,
        events: &mut Vec<GovernorEvent>,
    ) {
        let established = pm.hot_peer_count() + pm.warm_peer_count();
        if established < targets.established_peers {
            let deficit = targets.established_peers - established;
            let to_connect = pm.peers_to_connect();
            for addr in to_connect.into_iter().take(deficit) {
                events.push(GovernorEvent::Connect(addr));
            }
        }
    }

    /// Evaluate known peer targets — connect more cold peers
    fn evaluate_known(
        &self,
        _pm: &PeerManager,
        _targets: &PeerTargets,
        _events: &mut Vec<GovernorEvent>,
    ) {
        // Known peer expansion is handled by evaluate_established.
        // This hook exists for future peer discovery integration.
    }

    /// Evaluate surplus — demote/disconnect peers above targets
    fn evaluate_surplus(
        &self,
        pm: &PeerManager,
        targets: &PeerTargets,
        events: &mut Vec<GovernorEvent>,
    ) {
        // Hot peers above target — demote non-local-root peers
        let hot = pm.hot_peer_count();
        if hot > targets.active_peers {
            let surplus = hot - targets.active_peers;
            let mut demote_candidates: Vec<(SocketAddr, f64)> = pm
                .hot_peer_addrs()
                .into_iter()
                .filter(|addr| !pm.is_local_root(addr))
                .filter_map(|addr| pm.peer_performance(&addr).map(|p| (addr, p.reputation)))
                .collect();

            // Demote worst reputation first
            demote_candidates
                .sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
            for (addr, _) in demote_candidates.into_iter().take(surplus) {
                events.push(GovernorEvent::Demote(addr));
            }
        }

        // Established peers above target — disconnect non-local-root warm peers
        let established = pm.hot_peer_count() + pm.warm_peer_count();
        if established > targets.established_peers {
            let surplus = established - targets.established_peers;
            let warm_addrs = pm.connected_peer_addrs();
            let hot_addrs: std::collections::HashSet<_> = pm.hot_peer_addrs().into_iter().collect();
            let mut disconnect_candidates: Vec<(SocketAddr, f64)> = warm_addrs
                .into_iter()
                .filter(|addr| !hot_addrs.contains(addr) && !pm.is_local_root(addr))
                .filter_map(|addr| pm.peer_performance(&addr).map(|p| (addr, p.reputation)))
                .collect();

            disconnect_candidates
                .sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
            for (addr, _) in disconnect_candidates.into_iter().take(surplus) {
                events.push(GovernorEvent::Disconnect(addr));
            }
        }
    }

    /// Run the churn mechanism if enough time has elapsed.
    ///
    /// Churn periodically reduces targets by ~20%, causing the governor to
    /// demote some peers, then restores targets to force peer rotation.
    /// Returns events from the evaluation after churn adjustment.
    pub fn maybe_churn(&mut self, pm: &PeerManager) -> Vec<GovernorEvent> {
        let churn_interval = match self.sync_state {
            SyncState::PreSyncing | SyncState::Syncing => {
                Duration::from_secs(self.config.churn_interval_sync_secs)
            }
            SyncState::CaughtUp => Duration::from_secs(self.config.churn_interval_normal_secs),
        };

        if self.last_churn.elapsed() < churn_interval {
            return Vec::new();
        }

        if self.churn_active {
            // Restore targets — churn complete
            if let Some(saved) = self.pre_churn_targets.take() {
                match self.sync_state {
                    SyncState::PreSyncing | SyncState::Syncing => {
                        // Don't restore if we switched to syncing
                    }
                    SyncState::CaughtUp => {
                        self.config.normal_targets = saved;
                    }
                }
            }
            self.churn_active = false;
            self.last_churn = Instant::now();
            info!("Governor: churn phase complete, targets restored");
            return self.evaluate(pm);
        }

        // Start churn: reduce targets by ~20%
        let current = self.active_targets().clone();
        self.pre_churn_targets = Some(current.clone());

        let reduced = PeerTargets {
            root_peers: current.root_peers,
            known_peers: current.known_peers,
            established_peers: (current.established_peers * 4) / 5,
            active_peers: (current.active_peers * 4) / 5,
            known_blp: current.known_blp,
            established_blp: (current.established_blp * 4) / 5,
            active_blp: (current.active_blp * 4) / 5,
        };

        match self.sync_state {
            SyncState::PreSyncing | SyncState::Syncing => {
                self.config.sync_targets = reduced;
            }
            SyncState::CaughtUp => {
                self.config.normal_targets = reduced;
            }
        }

        self.churn_active = true;
        info!(
            active = current.active_peers,
            reduced = (current.active_peers * 4) / 5,
            "Governor: churn phase started, targets reduced by 20%"
        );

        self.evaluate(pm)
    }

    /// Get current governor config (for testing/inspection)
    pub fn config(&self) -> &GovernorConfig {
        &self.config
    }
}

/// Connection limit decision
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionDecision {
    /// Accept immediately
    Accept,
    /// Delay acceptance by the given duration
    Delay(Duration),
    /// Reject the connection
    Reject,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peer_manager::PeerManagerConfig;
    use std::net::SocketAddr;

    fn test_addr(port: u16) -> SocketAddr {
        format!("127.0.0.1:{port}").parse().unwrap()
    }

    fn setup_pm_with_peers(hot: usize, warm: usize, cold: usize) -> PeerManager {
        let config = PeerManagerConfig {
            target_hot_peers: 20,
            target_warm_peers: 20,
            target_known_peers: 200,
            ..PeerManagerConfig::default()
        };
        let mut pm = PeerManager::new(config);
        let mut port = 3000u16;

        for _ in 0..cold {
            pm.add_config_peer(test_addr(port), false, false);
            port += 1;
        }
        for _ in 0..warm {
            let addr = test_addr(port);
            pm.add_config_peer(addr, false, false);
            pm.peer_connected(&addr, 14, true);
            port += 1;
        }
        for _ in 0..hot {
            let addr = test_addr(port);
            pm.add_config_peer(addr, false, false);
            pm.peer_connected(&addr, 14, true);
            pm.promote_to_hot(&addr);
            port += 1;
        }
        pm
    }

    #[test]
    fn test_below_target_triggers_promotion() {
        let pm = setup_pm_with_peers(5, 10, 20); // 5 hot, need 20
        let mut gov = Governor::new(GovernorConfig::default());

        let events = gov.evaluate(&pm);
        let promote_count = events
            .iter()
            .filter(|e| matches!(e, GovernorEvent::Promote(_)))
            .count();
        assert!(
            promote_count > 0,
            "Should emit Promote events when hot < target"
        );
    }

    #[test]
    fn test_below_target_triggers_connect() {
        let pm = setup_pm_with_peers(0, 5, 50); // 5 established, need 30
        let mut gov = Governor::new(GovernorConfig::default());

        let events = gov.evaluate(&pm);
        let connect_count = events
            .iter()
            .filter(|e| matches!(e, GovernorEvent::Connect(_)))
            .count();
        assert!(
            connect_count > 0,
            "Should emit Connect events when established < target"
        );
    }

    #[test]
    fn test_above_target_triggers_demotion() {
        let config = GovernorConfig {
            normal_targets: PeerTargets {
                active_peers: 5,
                established_peers: 10,
                ..PeerTargets::default()
            },
            ..GovernorConfig::default()
        };
        let pm = setup_pm_with_peers(10, 5, 20); // 10 hot, target 5
        let mut gov = Governor::new(config);

        let events = gov.evaluate(&pm);
        let demote_count = events
            .iter()
            .filter(|e| matches!(e, GovernorEvent::Demote(_)))
            .count();
        assert!(
            demote_count > 0,
            "Should emit Demote events when hot > target"
        );
    }

    #[test]
    fn test_local_roots_never_demoted() {
        let config = GovernorConfig {
            normal_targets: PeerTargets {
                active_peers: 1,
                established_peers: 2,
                ..PeerTargets::default()
            },
            ..GovernorConfig::default()
        };

        let pm_config = PeerManagerConfig {
            target_hot_peers: 20,
            target_warm_peers: 20,
            target_known_peers: 200,
            ..PeerManagerConfig::default()
        };
        let mut pm = PeerManager::new(pm_config);

        // Add 3 hot peers: 2 local roots + 1 regular
        let local1 = test_addr(3001);
        let local2 = test_addr(3002);
        let regular = test_addr(3003);

        pm.add_config_peer(local1, true, false); // trustable = LocalRoot
        pm.add_config_peer(local2, true, false);
        pm.add_config_peer(regular, false, false);

        pm.peer_connected(&local1, 14, true);
        pm.peer_connected(&local2, 14, true);
        pm.peer_connected(&regular, 14, true);
        pm.promote_to_hot(&local1);
        pm.promote_to_hot(&local2);
        pm.promote_to_hot(&regular);

        let mut gov = Governor::new(config);
        let events = gov.evaluate(&pm);

        // Only the regular peer should be demoted, not the local roots
        let demoted: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                GovernorEvent::Demote(addr) => Some(*addr),
                _ => None,
            })
            .collect();

        for addr in &demoted {
            assert!(
                !pm.is_local_root(addr),
                "Local root {addr} should never be demoted"
            );
        }
    }

    #[test]
    fn test_sync_vs_normal_targets() {
        let mut gov = Governor::new(GovernorConfig::default());

        gov.set_sync_state(SyncState::CaughtUp);
        assert_eq!(gov.active_targets().active_peers, 20);

        gov.set_sync_state(SyncState::Syncing);
        assert_eq!(gov.active_targets().active_peers, 10);
    }

    #[test]
    fn test_connection_limit_accept() {
        let gov = Governor::new(GovernorConfig::default());
        assert_eq!(gov.connection_check(100), ConnectionDecision::Accept);
    }

    #[test]
    fn test_connection_limit_delay() {
        let gov = Governor::new(GovernorConfig::default());
        let decision = gov.connection_check(400); // above soft_limit=384
        assert!(matches!(decision, ConnectionDecision::Delay(_)));
    }

    #[test]
    fn test_connection_limit_reject() {
        let gov = Governor::new(GovernorConfig::default());
        assert_eq!(gov.connection_check(512), ConnectionDecision::Reject);
    }

    #[test]
    fn test_churn_reduces_then_restores() {
        let pm = setup_pm_with_peers(20, 10, 50);
        let mut gov = Governor::new(GovernorConfig {
            churn_interval_normal_secs: 0, // immediate churn for testing
            ..GovernorConfig::default()
        });

        // Force last_churn to be old enough
        gov.last_churn = Instant::now() - Duration::from_secs(10);

        // First churn: should reduce targets
        let _events = gov.maybe_churn(&pm);
        assert!(gov.churn_active);
        let reduced_active = gov.active_targets().active_peers;
        assert_eq!(reduced_active, 16); // 20 * 4/5 = 16

        // Force time forward again
        gov.last_churn = Instant::now() - Duration::from_secs(10);

        // Second churn call: should restore targets
        let _events = gov.maybe_churn(&pm);
        assert!(!gov.churn_active);
        assert_eq!(gov.active_targets().active_peers, 20); // restored
    }

    #[test]
    fn test_at_target_no_events() {
        let config = GovernorConfig {
            normal_targets: PeerTargets {
                active_peers: 5,
                established_peers: 10,
                known_peers: 50,
                ..PeerTargets::default()
            },
            ..GovernorConfig::default()
        };
        let pm = setup_pm_with_peers(5, 5, 40); // exactly at targets
        let mut gov = Governor::new(config);

        let events = gov.evaluate(&pm);
        // Should not produce surplus events since we're exactly at target
        let demote_count = events
            .iter()
            .filter(|e| matches!(e, GovernorEvent::Demote(_)))
            .count();
        assert_eq!(demote_count, 0);
    }
}
