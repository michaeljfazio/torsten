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
use std::collections::HashSet;
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tracing::{debug, info, trace};

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
    /// Defaults matching cardano-node configuration:
    /// TargetNumberOfRootPeers=60, TargetNumberOfKnownPeers=85,
    /// TargetNumberOfEstablishedPeers=40, TargetNumberOfActivePeers=15,
    /// TargetNumberOfKnownBigLedgerPeers=15, TargetNumberOfEstablishedBigLedgerPeers=10,
    /// TargetNumberOfActiveBigLedgerPeers=5
    fn default() -> Self {
        PeerTargets {
            root_peers: 60,
            known_peers: 85,
            established_peers: 40,
            active_peers: 15,
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
            root_peers: 30,
            known_peers: 50,
            established_peers: 20,
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
    /// 1. Local root valency enforcement (per-group, highest priority)
    /// 2. Big ledger peers (most important for genesis/sync)
    /// 3. Active peers below target → promote
    /// 4. Established peers below target → connect
    /// 5. Above target → demote/disconnect (respecting local root protection)
    pub fn evaluate(&mut self, pm: &PeerManager) -> Vec<GovernorEvent> {
        let mut events = Vec::new();
        let targets = self.active_targets().clone();

        // --- Phase 0: Local root valency enforcement ---
        self.evaluate_local_root_deficit(pm, &mut events);

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

        // Deduplicate events by address — multiple phases may emit Connect or
        // Promote for the same peer (e.g., BLP phase + general deficit phase).
        let mut seen_connect = std::collections::HashSet::new();
        let mut seen_promote = std::collections::HashSet::new();
        events.retain(|e| match e {
            GovernorEvent::Connect(addr) => seen_connect.insert(*addr),
            GovernorEvent::Promote(addr) => seen_promote.insert(*addr),
            _ => true,
        });

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

    /// Phase 0: Enforce per-group local root valency.
    ///
    /// For each registered `LocalRootGroupInfo`:
    /// - If connected (warm+hot) members < `warm_valency`, emit `Connect` for
    ///   cold members of this group.
    /// - If hot members < `hot_valency`, emit `Promote` for warm members of
    ///   this group.
    ///
    /// This phase runs before all others so that local root groups (which are
    /// operator-configured trusted peers) are always kept at their target
    /// valency.  Events for peers that already have pending actions from a
    /// prior phase are deduplicated by tracking addressed already added.
    fn evaluate_local_root_deficit(&self, pm: &PeerManager, events: &mut Vec<GovernorEvent>) {
        // Collect addresses already targeted by events emitted so far, so we
        // do not double-emit for the same peer across multiple group evaluations.
        let already_targeted: HashSet<SocketAddr> = events
            .iter()
            .map(|e| match e {
                GovernorEvent::Connect(a)
                | GovernorEvent::Promote(a)
                | GovernorEvent::Demote(a)
                | GovernorEvent::Disconnect(a) => *a,
            })
            .collect();
        // Build a mutable copy we update as we add events during this phase.
        let mut targeted = already_targeted;

        let hot_set: HashSet<SocketAddr> = pm.hot_peer_addrs().into_iter().collect();
        let connected_set: HashSet<SocketAddr> = pm.connected_peer_addrs().into_iter().collect();

        for group in pm.local_root_groups() {
            // Count current hot members
            let hot_members: Vec<SocketAddr> = group
                .members
                .iter()
                .filter(|a| hot_set.contains(a))
                .copied()
                .collect();
            let hot_count = hot_members.len();

            // Count warm members (connected but not hot)
            let warm_members: Vec<SocketAddr> = group
                .members
                .iter()
                .filter(|a| connected_set.contains(a) && !hot_set.contains(a))
                .copied()
                .collect();

            // Count cold members (not connected)
            let cold_members: Vec<SocketAddr> = group
                .members
                .iter()
                .filter(|a| !connected_set.contains(a))
                .copied()
                .collect();

            let connected_count = hot_count + warm_members.len();

            // --- Warm valency: ensure enough connected members ---
            if connected_count < group.warm_valency {
                let warm_deficit = group.warm_valency - connected_count;
                // Collect candidates first to avoid aliased borrow of `targeted`
                // inside the loop body (filter borrows immutably, insert borrows
                // mutably — Rust forbids both at the same time).
                let to_connect: Vec<SocketAddr> = cold_members
                    .iter()
                    .filter(|a| !targeted.contains(*a))
                    .take(warm_deficit)
                    .copied()
                    .collect();
                for addr in to_connect {
                    debug!(
                        group_id = group.group_id,
                        %addr,
                        connected = connected_count,
                        target = group.warm_valency,
                        "Local root group: emitting Connect for warm valency deficit"
                    );
                    events.push(GovernorEvent::Connect(addr));
                    targeted.insert(addr);
                }
            }

            // --- Hot valency: ensure enough active members ---
            if hot_count < group.hot_valency {
                let hot_deficit = group.hot_valency - hot_count;
                // Same two-phase collect+iterate pattern to avoid double-borrow.
                let to_promote: Vec<SocketAddr> = warm_members
                    .iter()
                    .filter(|a| !targeted.contains(*a))
                    .take(hot_deficit)
                    .copied()
                    .collect();
                for addr in to_promote {
                    debug!(
                        group_id = group.group_id,
                        %addr,
                        hot = hot_count,
                        target = group.hot_valency,
                        "Local root group: emitting Promote for hot valency deficit"
                    );
                    events.push(GovernorEvent::Promote(addr));
                    targeted.insert(addr);
                }
            }
        }
    }

    /// Phase 1: Evaluate big ledger peer targets.
    ///
    /// When active (hot) BLPs are below target:
    /// 1. Promote warm BLPs to hot.
    /// 2. If still short after promotions, emit `Connect` for cold BLPs.
    ///
    /// This replaces the previous TODO stub that never connected cold BLPs.
    fn evaluate_blp(
        &self,
        pm: &PeerManager,
        targets: &PeerTargets,
        events: &mut Vec<GovernorEvent>,
    ) {
        let active_blp = pm.active_big_ledger_peer_count();

        if active_blp >= targets.active_blp {
            return;
        }

        let mut blp_hot_after_events = active_blp;

        // Step 1: promote warm BLPs first (connection already established)
        let hot_set: HashSet<SocketAddr> = pm.hot_peer_addrs().into_iter().collect();
        let warm_blps: Vec<SocketAddr> = pm
            .connected_peer_addrs()
            .into_iter()
            .filter(|addr| {
                pm.peer_category(addr) == Some(PeerCategory::BigLedgerPeer)
                    && !hot_set.contains(addr)
            })
            .take(targets.active_blp - blp_hot_after_events)
            .collect();

        for addr in warm_blps {
            blp_hot_after_events += 1;
            events.push(GovernorEvent::Promote(addr));
        }

        // Step 2: if we're still below target, connect cold BLPs proactively.
        // This is the key fix: previously this path was a TODO and never emitted
        // any events, meaning cold BLPs were never connected when needed.
        if blp_hot_after_events < targets.active_blp {
            let cold_deficit = targets.active_blp - blp_hot_after_events;
            let cold_blps = pm.cold_big_ledger_peer_addrs();
            for addr in cold_blps.into_iter().take(cold_deficit) {
                debug!(
                    %addr,
                    active_blp,
                    target = targets.active_blp,
                    "BLP: emitting Connect for cold big ledger peer"
                );
                events.push(GovernorEvent::Connect(addr));
            }
        }
    }

    /// Phase 2: Evaluate active (hot) peer targets.
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

    /// Phase 3: Evaluate established (warm + hot) peer targets.
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

    /// Phase 4: Evaluate known peer targets — connect more cold peers.
    ///
    /// Known peer expansion is handled by `evaluate_established`.
    /// This hook exists for future peer discovery integration.
    fn evaluate_known(
        &self,
        _pm: &PeerManager,
        _targets: &PeerTargets,
        _events: &mut Vec<GovernorEvent>,
    ) {
    }

    /// Phase 5: Evaluate surplus — demote/disconnect peers above targets.
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

    /// Check for warm peers whose dwell time has elapsed and promote them to hot.
    ///
    /// This is a lightweight, fast-path evaluation that can be called more
    /// frequently than the full `evaluate()` cycle (e.g., every second) so
    /// that peers are promoted promptly once their dwell time expires rather
    /// than having to wait up to 30 seconds for the next governor tick.
    ///
    /// Returns `Promote` events for any warm peers ready for promotion, capped
    /// at the active-peer target deficit.
    pub fn check_warm_promotions(&self, pm: &PeerManager) -> Vec<GovernorEvent> {
        let targets = self.active_targets();
        let hot = pm.hot_peer_count();
        if hot >= targets.active_peers {
            return Vec::new();
        }
        let deficit = targets.active_peers - hot;
        let ready = pm.warm_peers_ready_to_promote();
        if ready.is_empty() {
            return Vec::new();
        }
        let events: Vec<GovernorEvent> = ready
            .into_iter()
            .take(deficit)
            .inspect(|addr| {
                trace!(%addr, "Governor warm-promotion check: promoting dwell-eligible peer to hot");
            })
            .map(GovernorEvent::Promote)
            .collect();
        if !events.is_empty() {
            debug!(
                count = events.len(),
                hot,
                target = targets.active_peers,
                "Governor: promoting warm peers that have elapsed dwell time"
            );
        }
        events
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
    use crate::peer_manager::{ConnectionDirection, PeerManagerConfig};
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
            pm.peer_connected(&addr, 14, ConnectionDirection::Outbound);
            // Backdate so the warm dwell time has already elapsed, making
            // these peers immediately eligible for promotion in governor tests.
            pm.backdate_warm_dwell(&addr);
            port += 1;
        }
        for _ in 0..hot {
            let addr = test_addr(port);
            pm.add_config_peer(addr, false, false);
            pm.peer_connected(&addr, 14, ConnectionDirection::Outbound);
            pm.backdate_warm_dwell(&addr);
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

        pm.peer_connected(&local1, 14, ConnectionDirection::Outbound);
        pm.peer_connected(&local2, 14, ConnectionDirection::Outbound);
        pm.peer_connected(&regular, 14, ConnectionDirection::Outbound);
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
        assert_eq!(gov.active_targets().active_peers, 15);

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
        assert_eq!(reduced_active, 12); // 15 * 4/5 = 12

        // Force time forward again
        gov.last_churn = Instant::now() - Duration::from_secs(10);

        // Second churn call: should restore targets
        let _events = gov.maybe_churn(&pm);
        assert!(!gov.churn_active);
        assert_eq!(gov.active_targets().active_peers, 15); // restored
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

    // ── BLP proactive connection ──────────────────────────────────────────────

    #[test]
    fn test_blp_cold_connect_when_below_target() {
        // Build a PM with 3 cold BLPs and no active BLPs.
        // The governor should emit Connect events for the cold BLPs to bring
        // active_blp up to the target (default 5, but we set it to 2 here).
        let pm_config = PeerManagerConfig {
            target_hot_peers: 20,
            target_warm_peers: 20,
            target_known_peers: 200,
            ..PeerManagerConfig::default()
        };
        let mut pm = PeerManager::new(pm_config);
        let blp1 = test_addr(4001);
        let blp2 = test_addr(4002);
        let blp3 = test_addr(4003);
        pm.add_big_ledger_peer(blp1);
        pm.add_big_ledger_peer(blp2);
        pm.add_big_ledger_peer(blp3);

        let config = GovernorConfig {
            normal_targets: PeerTargets {
                active_blp: 2,
                established_blp: 3,
                ..PeerTargets::default()
            },
            ..GovernorConfig::default()
        };
        let mut gov = Governor::new(config);
        let events = gov.evaluate(&pm);

        let connect_addrs: Vec<SocketAddr> = events
            .iter()
            .filter_map(|e| match e {
                GovernorEvent::Connect(a) => Some(*a),
                _ => None,
            })
            .collect();

        // Should have emitted Connect for cold BLPs (may connect all 3 cold
        // to ensure at least 2 reach active). The count is >= target deficit.
        assert!(
            connect_addrs.len() >= 2,
            "Expected at least 2 Connect events for cold BLPs, got: {connect_addrs:?}"
        );
        // All connects must target known BLP addresses
        for addr in &connect_addrs {
            assert!(
                [blp1, blp2, blp3].contains(addr),
                "Connect target {addr} is not a registered BLP"
            );
        }
    }

    #[test]
    fn test_blp_warm_promoted_before_cold_connected() {
        // When there are warm BLPs, they should be promoted to hot before the
        // governor tries to connect cold BLPs.
        let pm_config = PeerManagerConfig {
            target_hot_peers: 20,
            target_warm_peers: 20,
            target_known_peers: 200,
            ..PeerManagerConfig::default()
        };
        let mut pm = PeerManager::new(pm_config);

        // One warm BLP
        let warm_blp = test_addr(4001);
        pm.add_big_ledger_peer(warm_blp);
        pm.peer_connected(&warm_blp, 14, ConnectionDirection::Outbound);

        // One cold BLP
        let cold_blp = test_addr(4002);
        pm.add_big_ledger_peer(cold_blp);

        let config = GovernorConfig {
            normal_targets: PeerTargets {
                active_blp: 2,
                established_blp: 2,
                ..PeerTargets::default()
            },
            ..GovernorConfig::default()
        };
        let mut gov = Governor::new(config);
        let events = gov.evaluate(&pm);

        let promotes: Vec<SocketAddr> = events
            .iter()
            .filter_map(|e| match e {
                GovernorEvent::Promote(a) => Some(*a),
                _ => None,
            })
            .collect();
        let connects: Vec<SocketAddr> = events
            .iter()
            .filter_map(|e| match e {
                GovernorEvent::Connect(a) => Some(*a),
                _ => None,
            })
            .collect();

        // warm_blp must be promoted
        assert!(
            promotes.contains(&warm_blp),
            "warm BLP should be promoted: promotes={promotes:?}"
        );
        // cold_blp should be connected to fill the remaining deficit
        assert!(
            connects.contains(&cold_blp),
            "cold BLP should be connected: connects={connects:?}"
        );
    }

    // ── Local root valency enforcement ────────────────────────────────────────

    #[test]
    fn test_local_root_deficit_emits_connect() {
        // Group with hot_valency=2, warm_valency=3.
        // All 4 members are cold — the governor should emit Connect events for
        // the cold members up to the warm_valency target (3).
        let pm_config = PeerManagerConfig {
            target_hot_peers: 20,
            target_warm_peers: 20,
            target_known_peers: 200,
            ..PeerManagerConfig::default()
        };
        let mut pm = PeerManager::new(pm_config);

        let lr1 = test_addr(5001);
        let lr2 = test_addr(5002);
        let lr3 = test_addr(5003);
        let lr4 = test_addr(5004);
        pm.add_config_peer(lr1, true, false);
        pm.add_config_peer(lr2, true, false);
        pm.add_config_peer(lr3, true, false);
        pm.add_config_peer(lr4, true, false);

        pm.add_local_root_group(vec![lr1, lr2, lr3, lr4], 2, 3);

        let mut gov = Governor::new(GovernorConfig::default());
        let events = gov.evaluate(&pm);

        let connect_addrs: Vec<SocketAddr> = events
            .iter()
            .filter_map(|e| match e {
                GovernorEvent::Connect(a) => Some(*a),
                _ => None,
            })
            .collect();

        // Should connect at least warm_valency (3) members; general deficit
        // phase may also emit Connect for the 4th cold member.
        assert!(
            connect_addrs.len() >= 3,
            "Expected at least 3 Connect events for local root warm_valency deficit; got {connect_addrs:?}"
        );
        for addr in &connect_addrs {
            assert!(
                [lr1, lr2, lr3, lr4].contains(addr),
                "Connect {addr} is not a local root group member"
            );
        }
    }

    #[test]
    fn test_local_root_deficit_emits_promote() {
        // Group with hot_valency=2, warm_valency=2.
        // 2 members are warm but 0 are hot — the governor should Promote both.
        let pm_config = PeerManagerConfig {
            target_hot_peers: 20,
            target_warm_peers: 20,
            target_known_peers: 200,
            ..PeerManagerConfig::default()
        };
        let mut pm = PeerManager::new(pm_config);

        let lr1 = test_addr(5001);
        let lr2 = test_addr(5002);
        pm.add_config_peer(lr1, true, false);
        pm.add_config_peer(lr2, true, false);
        pm.peer_connected(&lr1, 14, ConnectionDirection::Outbound);
        pm.peer_connected(&lr2, 14, ConnectionDirection::Outbound);

        pm.add_local_root_group(vec![lr1, lr2], 2, 2);

        let mut gov = Governor::new(GovernorConfig::default());
        let events = gov.evaluate(&pm);

        let promote_addrs: Vec<SocketAddr> = events
            .iter()
            .filter_map(|e| match e {
                GovernorEvent::Promote(a) => Some(*a),
                _ => None,
            })
            .collect();

        assert_eq!(
            promote_addrs.len(),
            2,
            "Expected 2 Promote events for local root hot_valency deficit; got {promote_addrs:?}"
        );
        assert!(promote_addrs.contains(&lr1));
        assert!(promote_addrs.contains(&lr2));
    }

    #[test]
    fn test_local_root_at_valency_no_events() {
        // Group hot_valency=1, warm_valency=2. Already has 1 hot + 1 warm member.
        // Should emit no valency-related events for this group.
        let pm_config = PeerManagerConfig {
            target_hot_peers: 20,
            target_warm_peers: 20,
            target_known_peers: 200,
            ..PeerManagerConfig::default()
        };
        let mut pm = PeerManager::new(pm_config);

        let lr1 = test_addr(5001);
        let lr2 = test_addr(5002);
        pm.add_config_peer(lr1, true, false);
        pm.add_config_peer(lr2, true, false);
        pm.peer_connected(&lr1, 14, ConnectionDirection::Outbound);
        pm.peer_connected(&lr2, 14, ConnectionDirection::Outbound);
        pm.promote_to_hot(&lr1);

        pm.add_local_root_group(vec![lr1, lr2], 1, 2);

        // Use a config where global targets are exactly met so surplus/deficit
        // logic in phases 2-5 adds no noise.
        let config = GovernorConfig {
            normal_targets: PeerTargets {
                active_peers: 1,
                established_peers: 2,
                ..PeerTargets::default()
            },
            ..GovernorConfig::default()
        };
        let mut gov = Governor::new(config);
        let events = gov.evaluate(&pm);

        let group_events: Vec<_> = events
            .iter()
            .filter(|e| match e {
                GovernorEvent::Connect(a) | GovernorEvent::Promote(a) => [lr1, lr2].contains(a),
                _ => false,
            })
            .collect();

        assert!(
            group_events.is_empty(),
            "No valency events should be emitted for a group already at target; got: {group_events:?}"
        );
    }

    // ── Warm dwell time ───────────────────────────────────────────────────────

    #[test]
    fn test_warm_peers_not_promoted_before_dwell_elapsed() {
        // Warm peers whose dwell has NOT elapsed must not appear in
        // peers_to_promote() or trigger Promote events from evaluate().
        let config = PeerManagerConfig {
            target_hot_peers: 2,
            target_warm_peers: 2,
            target_known_peers: 200,
            ..PeerManagerConfig::default()
        };
        let mut pm = PeerManager::new(config);
        let a1 = test_addr(6001);
        let a2 = test_addr(6002);
        pm.add_config_peer(a1, false, false);
        pm.add_config_peer(a2, false, false);
        // Connect peers — puts them in Warm with promoted_at = now.
        // Do NOT call backdate_warm_dwell, so elapsed < WARM_DWELL_TIME.
        pm.peer_connected(&a1, 14, ConnectionDirection::Outbound);
        pm.peer_connected(&a2, 14, ConnectionDirection::Outbound);

        // peers_to_promote must return nothing because dwell hasn't elapsed.
        let to_promote = pm.peers_to_promote();
        assert!(
            to_promote.is_empty(),
            "peers_to_promote should be empty before dwell elapses, got: {to_promote:?}"
        );

        // The governor's evaluate() must also produce no Promote events.
        let gov_config = GovernorConfig {
            normal_targets: PeerTargets {
                active_peers: 5,
                established_peers: 10,
                ..PeerTargets::default()
            },
            ..GovernorConfig::default()
        };
        let mut gov = Governor::new(gov_config);
        let events = gov.evaluate(&pm);
        let promotes: Vec<SocketAddr> = events
            .iter()
            .filter_map(|e| match e {
                GovernorEvent::Promote(a) => Some(*a),
                _ => None,
            })
            .collect();
        assert!(
            promotes.is_empty(),
            "evaluate() must not promote peers before dwell elapses, got: {promotes:?}"
        );
    }

    #[test]
    fn test_warm_peers_promoted_after_dwell_elapsed() {
        // After backdate_warm_dwell(), peers must be eligible for promotion.
        let config = PeerManagerConfig {
            target_hot_peers: 2,
            target_warm_peers: 2,
            target_known_peers: 200,
            ..PeerManagerConfig::default()
        };
        let mut pm = PeerManager::new(config);
        let a1 = test_addr(6001);
        let a2 = test_addr(6002);
        pm.add_config_peer(a1, false, false);
        pm.add_config_peer(a2, false, false);
        pm.peer_connected(&a1, 14, ConnectionDirection::Outbound);
        pm.peer_connected(&a2, 14, ConnectionDirection::Outbound);
        // Simulate dwell elapsed.
        pm.backdate_warm_dwell(&a1);
        pm.backdate_warm_dwell(&a2);

        let to_promote = pm.peers_to_promote();
        assert_eq!(
            to_promote.len(),
            2,
            "Both dwell-elapsed warm peers should be promotion candidates"
        );
    }

    #[test]
    fn test_check_warm_promotions_returns_events_after_dwell() {
        // check_warm_promotions() should emit Promote events for dwell-elapsed
        // warm peers, capped by the hot-peer target deficit.
        let pm_config = PeerManagerConfig {
            target_hot_peers: 20,
            target_warm_peers: 20,
            target_known_peers: 200,
            ..PeerManagerConfig::default()
        };
        let mut pm = PeerManager::new(pm_config);
        let a1 = test_addr(7001);
        let a2 = test_addr(7002);
        pm.add_config_peer(a1, false, false);
        pm.add_config_peer(a2, false, false);
        pm.peer_connected(&a1, 14, ConnectionDirection::Outbound);
        pm.peer_connected(&a2, 14, ConnectionDirection::Outbound);
        pm.backdate_warm_dwell(&a1);
        pm.backdate_warm_dwell(&a2);

        let gov_config = GovernorConfig {
            normal_targets: PeerTargets {
                active_peers: 5,
                ..PeerTargets::default()
            },
            ..GovernorConfig::default()
        };
        let gov = Governor::new(gov_config);
        let events = gov.check_warm_promotions(&pm);
        let promote_count = events
            .iter()
            .filter(|e| matches!(e, GovernorEvent::Promote(_)))
            .count();
        assert_eq!(
            promote_count, 2,
            "check_warm_promotions should promote both dwell-elapsed warm peers"
        );
    }

    #[test]
    fn test_check_warm_promotions_empty_before_dwell() {
        // check_warm_promotions() must return nothing if no peers have elapsed dwell.
        let pm_config = PeerManagerConfig {
            target_hot_peers: 20,
            target_warm_peers: 20,
            target_known_peers: 200,
            ..PeerManagerConfig::default()
        };
        let mut pm = PeerManager::new(pm_config);
        let a1 = test_addr(7001);
        pm.add_config_peer(a1, false, false);
        pm.peer_connected(&a1, 14, ConnectionDirection::Outbound);
        // Do NOT backdate — dwell hasn't elapsed.

        let gov = Governor::new(GovernorConfig::default());
        let events = gov.check_warm_promotions(&pm);
        assert!(
            events.is_empty(),
            "check_warm_promotions should be empty before dwell elapses"
        );
    }

    #[test]
    fn test_dwell_resets_on_demotion() {
        // When a hot peer is demoted to warm, promoted_at resets so the full
        // dwell time must elapse again before it can be re-promoted.
        let mut pm = PeerManager::new(PeerManagerConfig::default());
        let addr = test_addr(8001);
        pm.add_config_peer(addr, false, false);
        pm.peer_connected(&addr, 14, ConnectionDirection::Outbound);
        pm.backdate_warm_dwell(&addr);
        pm.promote_to_hot(&addr);

        // Peer is now hot — promoted_at is cleared.
        let info = pm.peer_info_for_test(&addr).unwrap();
        assert!(
            info.promoted_at.is_none(),
            "promoted_at should be None after promotion to hot"
        );

        // Demote back to warm — promoted_at should be reset to now.
        pm.demote_to_warm(&addr);
        let info = pm.peer_info_for_test(&addr).unwrap();
        assert!(
            info.promoted_at.is_some(),
            "promoted_at should be set after demotion to warm"
        );

        // The freshly demoted peer must NOT be immediately eligible.
        assert!(
            !info.can_promote_to_hot(),
            "freshly demoted peer must not pass can_promote_to_hot() immediately"
        );
    }
}
