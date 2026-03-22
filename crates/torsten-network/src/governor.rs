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
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tracing::{debug, info, warn};

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
    /// Demote a hot peer to warm (stop syncing).
    ///
    /// Used both for normal surplus reduction and for active churn during
    /// randomised peer rotation.
    Demote(SocketAddr),
    /// Ask a warm/hot peer to share up to `count` peer addresses with us.
    ///
    /// Corresponds to Haskell's peer-sharing protocol: we send a
    /// `MsgShareRequest(count)` to the peer and incorporate any addresses
    /// it returns as new cold peers.
    RequestPeerSharing(SocketAddr, u8),
    /// Evict a cold peer from the known-peer set to make room for fresher
    /// discoveries.  Used when the known-peer count exceeds 1.5× target.
    EvictColdPeer(SocketAddr),
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
    /// Number of consecutive evaluation cycles in which a hot peer must
    /// serve zero new blocks before the governor demotes it back to warm.
    ///
    /// Each full evaluation cycle runs every 30 seconds, so the default of 6
    /// cycles corresponds to a 3-minute stall window.  Local root peers are
    /// exempt from stall demotion.
    pub stall_demotion_cycles: u32,
    /// Failure count above which a hot peer is unconditionally demoted to
    /// warm.  Local root peers are exempt from error-rate demotion.
    pub error_demotion_threshold: u32,
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
            stall_demotion_cycles: 6, // 6 × 30s = 3 min; only affects sync-capable connections
            error_demotion_threshold: 5, // 5 accumulated failures
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

    // ── Phase 3: Randomised churn selection ──────────────────────────────────
    /// xorshift64 PRNG state for randomised peer selection during churn.
    ///
    /// Seeded from wall-clock time at construction; advanced once per churn
    /// cycle so that each churn round picks a different random subset of peers
    /// to demote rather than always targeting the lowest-reputation ones.
    churn_seed: u64,
    /// Peers demoted during the current churn cycle.
    ///
    /// These addresses are suppressed from re-selection for the next full
    /// evaluation cycle (cleared at the start of the *following* churn).
    /// This implements Haskell's `policyPeerChurnExclusion` behaviour: a
    /// churned peer cannot immediately return to hot, giving new peers a
    /// chance to establish themselves.
    recently_churned: HashSet<SocketAddr>,

    // ── Phase 4: Peer sharing rate-limiting ──────────────────────────────────
    /// Timestamp of the last `RequestPeerSharing` emission.
    ///
    /// Peer-sharing requests are rate-limited to at most once per 60 seconds,
    /// matching Haskell's `policyPeerShareRetryTime`.
    last_peer_sharing_request: Option<Instant>,

    // ── Phase 8: Stall-detection for hot peers (#200) ─────────────────────
    /// Snapshot of `blocks_fetched` per hot peer taken at the end of each
    /// full evaluation cycle.  On the next cycle the governor compares the
    /// current count against this snapshot; if it has not increased for
    /// `config.stall_demotion_cycles` consecutive evaluations the peer is
    /// demoted back to warm.
    ///
    /// The outer `HashMap` maps peer address to `(snapshot_blocks, stale_cycles)`.
    /// `stale_cycles` is incremented each time the block count has not grown;
    /// reset to 0 when at least one new block has been fetched.
    hot_peer_stall: HashMap<SocketAddr, (u64, u32)>,
}

impl Governor {
    pub fn new(config: GovernorConfig) -> Self {
        // Seed the PRNG from wall-clock time so that each governor instance
        // (and each process restart) starts with a different sequence.
        // We must not use 0 as the xorshift64 seed — fall back to a constant.
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64 ^ (d.as_secs().wrapping_mul(0x9e37_79b9_7f4a_7c15)))
            .unwrap_or(0x5851_f42d_4c95_7f2d);
        let churn_seed = if seed == 0 {
            0x5851_f42d_4c95_7f2d
        } else {
            seed
        };

        Governor {
            config,
            sync_state: SyncState::CaughtUp,
            last_churn: Instant::now(),
            churn_active: false,
            pre_churn_targets: None,
            churn_seed,
            recently_churned: HashSet::new(),
            last_peer_sharing_request: None,
            hot_peer_stall: HashMap::new(),
        }
    }

    /// xorshift64 PRNG step — advances `self.churn_seed` and returns the new value.
    ///
    /// Standard Marsaglia xorshift64 with period 2^64 - 1.  Only used during
    /// churn to shuffle the candidate list; not used for security purposes.
    fn xorshift64_next(&mut self) -> u64 {
        let mut x = self.churn_seed;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.churn_seed = x;
        x
    }

    /// Fisher-Yates shuffle of a slice using the internal xorshift64 PRNG.
    ///
    /// Produces a uniformly random permutation; used during churn to pick a
    /// random subset of demotion candidates rather than always the worst ones.
    fn shuffle<T>(&mut self, v: &mut [T]) {
        let n = v.len();
        if n < 2 {
            return;
        }
        for i in (1..n).rev() {
            // Pick a random index in [0, i]
            let j = (self.xorshift64_next() as usize) % (i + 1);
            v.swap(i, j);
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
    /// 3. Active peers below target → promote (liveness-gated)
    /// 4. Established peers below target → connect
    /// 5. Above target → demote/disconnect (respecting local root protection)
    ///    5a. BLP preemption during Syncing (demote non-BLP hot to free slots)
    /// 6. Peer sharing when known-peer count is low
    /// 7. Known-peer surplus eviction (trim cold set when > 1.5× target)
    /// 8. Stall/error-rate demotion of hot peers that are not contributing
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

        // --- Phase 5: Surplus reduction + BLP preemption ---
        self.evaluate_surplus(pm, &targets, &mut events);

        // --- Phase 6: Peer sharing ---
        self.evaluate_peer_sharing(pm, &targets, &mut events);

        // --- Phase 7: Known-peer surplus eviction ---
        self.evaluate_known_surplus(pm, &targets, &mut events);

        // --- Phase 8: Stall/error-rate demotion ---
        self.evaluate_stale_hot_peers(pm, &mut events);

        // --- Update stall-detection snapshot for next cycle ---
        // Record the current blocks_fetched for all hot peers so that the
        // next call to evaluate() can detect which peers served no new blocks.
        self.update_stall_snapshot(pm);

        // Deduplicate events by address — multiple phases may emit Connect or
        // Promote for the same peer (e.g., BLP phase + general deficit phase).
        // Also deduplicate Demote events so that stall demotion and surplus
        // reduction don't both fire for the same peer in one cycle.
        let mut seen_connect = std::collections::HashSet::new();
        let mut seen_promote = std::collections::HashSet::new();
        let mut seen_demote = std::collections::HashSet::new();
        events.retain(|e| match e {
            GovernorEvent::Connect(addr) => seen_connect.insert(*addr),
            GovernorEvent::Promote(addr) => seen_promote.insert(*addr),
            GovernorEvent::Demote(addr) => seen_demote.insert(*addr),
            _ => true,
        });

        if !events.is_empty() {
            debug!(
                event_count = events.len(),
                hot = pm.hot_peer_count(),
                warm = pm.warm_peer_count(),
                cold = pm.cold_peer_count(),
                known = pm.known_peer_count(),
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
            .filter_map(|e| match e {
                GovernorEvent::Connect(a)
                | GovernorEvent::Promote(a)
                | GovernorEvent::Demote(a)
                | GovernorEvent::Disconnect(a)
                | GovernorEvent::EvictColdPeer(a) => Some(*a),
                // RequestPeerSharing carries a peer address too, but it is not a
                // "targeted" action in the connection-lifecycle sense — we can still
                // emit Connect/Promote for the same peer independently.
                GovernorEvent::RequestPeerSharing(_, _) => None,
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

        // Step 1: promote warm BLPs first (connection already established).
        // Gate on liveness: only promote if the peer has demonstrated activity
        // or connected recently enough that it hasn't yet had a chance to serve
        // blocks.
        let hot_set: HashSet<SocketAddr> = pm.hot_peer_addrs().into_iter().collect();
        let warm_blps: Vec<SocketAddr> = pm
            .connected_peer_addrs()
            .into_iter()
            .filter(|addr| {
                pm.peer_category(addr) == Some(PeerCategory::BigLedgerPeer)
                    && !hot_set.contains(addr)
                    && pm.is_promotion_ready(addr)
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
    ///
    /// Before promoting a warm peer to hot, we verify that it has demonstrated
    /// some liveness: either it has fetched at least one block since connecting
    /// (non-zero `blocks_fetched`), or it connected recently enough that it
    /// hasn't yet had the chance to serve blocks.  This prevents the governor
    /// from promoting peers that connected but then immediately stalled.
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
            for addr in to_promote
                .into_iter()
                .filter(|a| pm.is_promotion_ready(a))
                .take(deficit)
            {
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

    /// Phase 4 (governor internal numbering): Evaluate known peer targets.
    ///
    /// Known peer expansion is driven by `evaluate_established` (which connects
    /// cold peers) and `evaluate_peer_sharing` below.  This phase is currently
    /// a no-op placeholder for future topology-based cold peer additions.
    fn evaluate_known(
        &self,
        _pm: &PeerManager,
        _targets: &PeerTargets,
        _events: &mut Vec<GovernorEvent>,
    ) {
    }

    /// Phase 5: Evaluate surplus — demote/disconnect peers above targets,
    /// and (during Syncing) preempt non-BLP hot peers to make room for BLPs.
    ///
    /// ## BLP preemption
    ///
    /// When the node is in `SyncState::Syncing` and the active BLP count is
    /// below `targets.active_blp`, non-BLP hot peers that are NOT local roots
    /// are demoted first (before connecting new BLPs) to free hot-peer slots.
    /// This mirrors Haskell's behaviour where BLPs are treated as high-priority
    /// sync partners during initial block download.
    fn evaluate_surplus(
        &self,
        pm: &PeerManager,
        targets: &PeerTargets,
        events: &mut Vec<GovernorEvent>,
    ) {
        // ── BLP preemption (Syncing only) ────────────────────────────────────
        // If we are syncing and active BLPs are below target, demote non-BLP hot
        // peers that are not local roots to free slots for incoming BLP connections.
        if self.sync_state == SyncState::Syncing {
            let active_blp = pm.active_big_ledger_peer_count();
            if active_blp < targets.active_blp {
                let blp_deficit = targets.active_blp - active_blp;
                // How many non-BLP hot peers can we demote?
                let mut preempt_candidates: Vec<(SocketAddr, f64)> = pm
                    .hot_peer_addrs()
                    .into_iter()
                    .filter(|addr| {
                        !pm.is_local_root(addr)
                            && pm.peer_category(addr) != Some(PeerCategory::BigLedgerPeer)
                    })
                    .filter_map(|addr| pm.peer_performance(&addr).map(|p| (addr, p.reputation)))
                    .collect();

                // Demote worst-reputation non-BLP hot peers first
                preempt_candidates
                    .sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
                for (addr, _) in preempt_candidates.into_iter().take(blp_deficit) {
                    debug!(
                        %addr,
                        active_blp,
                        target_blp = targets.active_blp,
                        "BLP preemption: demoting non-BLP hot peer to make room"
                    );
                    events.push(GovernorEvent::Demote(addr));
                }
            }
        }

        // ── Hot surplus ──────────────────────────────────────────────────────
        // Hot peers above target — demote non-local-root peers (worst first).
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

        // ── Established surplus ──────────────────────────────────────────────
        // Established peers above target — disconnect non-local-root warm peers.
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

    /// Phase 6: Governor-driven peer sharing.
    ///
    /// When the total known-peer count is below `targets.known_peers`, the
    /// governor selects up to 2 warm/hot outbound peers and emits
    /// `RequestPeerSharing(addr, 10)` for each.  Requests are rate-limited to
    /// once per 60 seconds (Haskell's `policyPeerShareRetryTime`).
    ///
    /// The node is expected to send a `MsgShareRequest(10)` to each peer and
    /// add any returned addresses via `PeerManager::add_shared_peer`.
    fn evaluate_peer_sharing(
        &mut self,
        pm: &PeerManager,
        targets: &PeerTargets,
        events: &mut Vec<GovernorEvent>,
    ) {
        // Rate limit: at most one batch of requests per 60 seconds.
        const PEER_SHARE_RETRY_INTERVAL: Duration = Duration::from_secs(60);
        if let Some(last) = self.last_peer_sharing_request {
            if last.elapsed() < PEER_SHARE_RETRY_INTERVAL {
                return;
            }
        }

        let known = pm.known_peer_count();
        if known >= targets.known_peers {
            // Already at or above target — no need to request more.
            return;
        }

        let candidates = pm.warm_hot_peer_addrs_for_sharing();
        if candidates.is_empty() {
            return;
        }

        // Ask at most 2 peers per cycle, with count=10 each.
        const MAX_SHARING_PEERS_PER_CYCLE: usize = 2;
        const PEER_SHARE_REQUEST_COUNT: u8 = 10;

        let mut emitted = 0;
        for addr in candidates.into_iter().take(MAX_SHARING_PEERS_PER_CYCLE) {
            debug!(
                %addr,
                known,
                target_known = targets.known_peers,
                "Peer sharing: emitting RequestPeerSharing"
            );
            events.push(GovernorEvent::RequestPeerSharing(
                addr,
                PEER_SHARE_REQUEST_COUNT,
            ));
            emitted += 1;
        }

        if emitted > 0 {
            self.last_peer_sharing_request = Some(Instant::now());
            info!(
                emitted,
                known,
                target = targets.known_peers,
                "Governor: requested peer sharing to fill known-peer deficit"
            );
        }
    }

    /// Phase 7: Known-peer surplus eviction.
    ///
    /// When the total known-peer count exceeds `targets.known_peers * 1.5`,
    /// the governor evicts the oldest and lowest-reputation cold non-config
    /// peers to trim the set back towards target.
    ///
    /// Config peers (topology file entries) are never evicted.
    fn evaluate_known_surplus(
        &self,
        pm: &PeerManager,
        targets: &PeerTargets,
        events: &mut Vec<GovernorEvent>,
    ) {
        let known = pm.known_peer_count();
        // Only trim when more than 50% above target.
        let high_water = targets.known_peers.saturating_mul(3) / 2; // known_peers * 1.5
        if known <= high_water {
            return;
        }

        let to_evict = known - high_water;
        let eviction_candidates = pm.cold_peers_for_eviction();

        for addr in eviction_candidates.into_iter().take(to_evict) {
            debug!(
                %addr,
                known,
                high_water,
                "Known surplus: emitting EvictColdPeer"
            );
            events.push(GovernorEvent::EvictColdPeer(addr));
        }
    }

    /// Run the churn mechanism if enough time has elapsed.
    ///
    /// Churn periodically rotates a random subset of peers to prevent the
    /// node from becoming permanently attached to the same set:
    ///
    /// 1. **Start phase**: reduces targets by ~20% so `evaluate_surplus`
    ///    demotes some peers.  Instead of always demoting the worst-reputation
    ///    peers (which would never give low-reputation peers a chance to
    ///    recover), the demotion candidates are *shuffled* using the internal
    ///    xorshift64 PRNG so that any hot peer may be churned.  The demoted
    ///    peers are recorded in `recently_churned`.
    ///
    /// 2. **End phase** (next call after the interval): restores targets,
    ///    clears `recently_churned`, and runs a normal evaluation so the
    ///    governor immediately fills the freed slots from new candidates.
    ///
    /// The `recently_churned` set is consulted by callers that drive peer
    /// selection to suppress the re-connection of recently rotated peers for
    /// one full evaluation cycle (Haskell's `policyPeerChurnExclusion`).
    ///
    /// Returns events (surplus demotion events in start phase; fill-in events
    /// in end phase).
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
            // ── End phase: restore targets and clear churn state ──────────────
            if let Some(saved) = self.pre_churn_targets.take() {
                match self.sync_state {
                    SyncState::PreSyncing | SyncState::Syncing => {
                        // Sync targets were temporarily reduced; restore them.
                        self.config.sync_targets = saved;
                    }
                    SyncState::CaughtUp => {
                        self.config.normal_targets = saved;
                    }
                }
            }
            self.churn_active = false;
            self.last_churn = Instant::now();
            // Clear the recently-churned set at the start of the next cycle
            // so newly rotating peers are eligible again.
            self.recently_churned.clear();
            info!("Governor: churn phase complete, targets restored");
            return self.evaluate(pm);
        }

        // ── Start phase: reduce targets by ~20% and select random demotions ──

        // Clear the previous cycle's recently-churned set; we are about to
        // populate it with the new cycle's demotions.
        self.recently_churned.clear();

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
                self.config.sync_targets = reduced.clone();
            }
            SyncState::CaughtUp => {
                self.config.normal_targets = reduced.clone();
            }
        }

        self.churn_active = true;
        info!(
            active = current.active_peers,
            reduced = reduced.active_peers,
            "Governor: churn phase started, targets reduced by 20%"
        );

        // Build the list of hot peers eligible for churn (non-local-root),
        // then shuffle them with the PRNG so the selection is random rather
        // than always the worst-reputation peers.
        let hot = pm.hot_peer_count();
        let churn_surplus = hot.saturating_sub(reduced.active_peers);

        let mut events = Vec::new();

        if churn_surplus > 0 {
            let mut candidates: Vec<SocketAddr> = pm
                .hot_peer_addrs()
                .into_iter()
                .filter(|addr| !pm.is_local_root(addr))
                .collect();

            // Randomise the order so we don't always churn the same peers.
            self.shuffle(&mut candidates);

            for addr in candidates.into_iter().take(churn_surplus) {
                debug!(%addr, "Governor: churn demoting peer (randomised)");
                self.recently_churned.insert(addr);
                events.push(GovernorEvent::Demote(addr));
            }
        }

        // Run the full evaluation so other deficit/surplus phases also fire
        // after the target reduction.
        let mut eval_events = self.evaluate(pm);

        // Collect the set of addresses we have already emitted Demote for
        // (the randomised churn demotions above).
        let churn_demoted: HashSet<SocketAddr> = events
            .iter()
            .filter_map(|e| match e {
                GovernorEvent::Demote(a) => Some(*a),
                _ => None,
            })
            .collect();

        // `evaluate_surplus` inside `evaluate()` may also emit Demote events
        // for the same hot-surplus peers because the targets are now reduced.
        // Remove any duplicates (same address already in churn_demoted) so we
        // don't emit two Demote events for the same peer.
        eval_events.retain(|e| match e {
            GovernorEvent::Demote(a) => !churn_demoted.contains(a),
            _ => true,
        });

        // Any remaining Demote events from the evaluate phase are also part of
        // this churn cycle (they were triggered by the target reduction).
        // Record them in recently_churned so suppression covers ALL demotions
        // from this cycle, not just the randomised subset.
        for e in &eval_events {
            if let GovernorEvent::Demote(a) = e {
                self.recently_churned.insert(*a);
            }
        }

        events.extend(eval_events);
        events
    }

    /// Returns the set of peers that were demoted during the most recent churn
    /// cycle.  Callers can use this to suppress immediate re-selection of
    /// churned peers for one evaluation cycle.
    pub fn recently_churned(&self) -> &HashSet<SocketAddr> {
        &self.recently_churned
    }

    /// Phase 8: Stall and error-rate demotion of hot peers (#200).
    ///
    /// A hot peer is demoted back to warm if:
    ///
    /// 1. **Stall**: it has not fetched any new blocks for at least
    ///    `config.stall_demotion_cycles` consecutive evaluation cycles.  This
    ///    is determined by comparing the peer's current `blocks_fetched`
    ///    counter against the snapshot taken at the previous cycle.
    ///
    /// 2. **Errors**: its accumulated `failure_count` exceeds
    ///    `config.error_demotion_threshold`.
    ///
    /// Local root peers are exempt from both checks — they are operator-
    /// configured trusted peers that should be promoted again immediately.
    fn evaluate_stale_hot_peers(&mut self, pm: &PeerManager, events: &mut Vec<GovernorEvent>) {
        // Don't stall-demote when at or below the active peer target.
        // Demoting in this situation creates a promote→demote loop since
        // the deficit phase immediately re-promotes the peer.  Stall
        // demotion is only meaningful when there are surplus hot peers
        // and we can afford to drop an underperforming one.
        let targets = self.active_targets();
        let hot_count = pm.hot_peer_count();
        if hot_count <= targets.active_peers {
            return;
        }

        // Build the set of addresses already targeted for demotion by earlier
        // phases so we don't emit duplicate Demote events.
        let already_demoted: HashSet<SocketAddr> = events
            .iter()
            .filter_map(|e| {
                if let GovernorEvent::Demote(a) = e {
                    Some(*a)
                } else {
                    None
                }
            })
            .collect();

        for addr in pm.hot_peer_addrs() {
            // Local root peers are never demoted by the governor.
            if pm.is_local_root(&addr) {
                continue;
            }
            // Skip peers already scheduled for demotion this cycle.
            if already_demoted.contains(&addr) {
                continue;
            }

            // ── Error-rate check ─────────────────────────────────────────────
            // Demote immediately if the peer has accumulated too many failures.
            if let Some(info) = pm.peer_info(&addr) {
                if info.failure_count >= self.config.error_demotion_threshold {
                    warn!(
                        %addr,
                        failure_count = info.failure_count,
                        threshold = self.config.error_demotion_threshold,
                        "Governor: demoting hot peer due to excessive failures"
                    );
                    events.push(GovernorEvent::Demote(addr));
                    continue;
                }
            }

            // ── Stall check ──────────────────────────────────────────────────
            // If we have a snapshot from the previous cycle, check whether the
            // block count has grown.  If not, increment the stale counter; if
            // it has exceeded the threshold, demote.
            //
            // Skip peers that have NEVER served any blocks — these are likely
            // governor-managed connections that don't run ChainSync (only
            // TxSubmission2 + KeepAlive).  Stall demotion is meaningless for
            // them since they were never expected to serve blocks.
            let current_blocks = pm
                .peer_performance(&addr)
                .map(|p| p.blocks_fetched)
                .unwrap_or(0);

            if let Some(&(snapshot_blocks, stale_cycles)) = self.hot_peer_stall.get(&addr) {
                // Only apply stall check to peers that have served at least one block.
                // Governor-managed connections (TxSubmission2-only) never serve blocks.
                if current_blocks == 0 {
                    // This peer has never served blocks — skip stall check.
                    continue;
                }
                if current_blocks == snapshot_blocks {
                    // No new blocks this cycle.
                    let new_stale = stale_cycles + 1;
                    if new_stale >= self.config.stall_demotion_cycles {
                        warn!(
                            %addr,
                            stale_cycles = new_stale,
                            threshold = self.config.stall_demotion_cycles,
                            blocks_fetched = current_blocks,
                            "Governor: demoting stalled hot peer (no new blocks)"
                        );
                        events.push(GovernorEvent::Demote(addr));
                        // Reset stale counter to prevent repeated demotion
                        // on every subsequent cycle while the peer is still
                        // technically hot (pending external demotion apply).
                        self.hot_peer_stall.remove(&addr);
                    }
                }
                // If blocks grew, the snapshot will be updated in
                // update_stall_snapshot() called after this phase.
            }
            // Peers with no snapshot are new promotions — skip until
            // their first full cycle has elapsed.
        }
    }

    /// Update the stall-detection snapshot after each full evaluation cycle.
    ///
    /// Called at the end of `evaluate()` to record each hot peer's current
    /// `blocks_fetched` counter and the number of consecutive stale cycles.
    /// Peers that are no longer hot are removed from the map; new hot peers
    /// are inserted with their current count and `stale_cycles = 0`.
    fn update_stall_snapshot(&mut self, pm: &PeerManager) {
        let hot_addrs: HashSet<SocketAddr> = pm.hot_peer_addrs().into_iter().collect();

        // Remove entries for peers that are no longer hot (they may have been
        // demoted by this cycle or by an external action).
        self.hot_peer_stall
            .retain(|addr, _| hot_addrs.contains(addr));

        // Update/insert for each current hot peer.
        for addr in &hot_addrs {
            let current_blocks = pm
                .peer_performance(addr)
                .map(|p| p.blocks_fetched)
                .unwrap_or(0);

            self.hot_peer_stall
                .entry(*addr)
                .and_modify(|(snapshot, stale_cycles)| {
                    if current_blocks > *snapshot {
                        // Progress: reset the stale counter.
                        *snapshot = current_blocks;
                        *stale_cycles = 0;
                    } else {
                        // No progress: increment stale counter (capped to avoid
                        // wrapping on very long stalls).
                        *stale_cycles = stale_cycles.saturating_add(1);
                    }
                })
                .or_insert((current_blocks, 0));
        }
    }

    /// Get current governor config (for testing/inspection)
    pub fn config(&self) -> &GovernorConfig {
        &self.config
    }

    /// Return the stall-detection snapshot (for testing/inspection).
    ///
    /// Each entry maps a hot peer address to `(blocks_fetched_snapshot,
    /// consecutive_stale_cycles)`.
    #[cfg(test)]
    pub fn hot_peer_stall(&self) -> &HashMap<SocketAddr, (u64, u32)> {
        &self.hot_peer_stall
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
            port += 1;
        }
        for _ in 0..hot {
            let addr = test_addr(port);
            pm.add_config_peer(addr, false, false);
            pm.peer_connected(&addr, 14, ConnectionDirection::Outbound);
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

    // ── Phase 3: Randomised churn ─────────────────────────────────────────────

    #[test]
    fn test_churn_emits_demote_events() {
        // 20 hot peers, target 15 — churn reduces target to 12, so 8 surplus.
        let pm = setup_pm_with_peers(20, 5, 50);
        let mut gov = Governor::new(GovernorConfig {
            churn_interval_normal_secs: 0,
            ..GovernorConfig::default()
        });
        gov.last_churn = Instant::now() - Duration::from_secs(10);

        let events = gov.maybe_churn(&pm);
        assert!(gov.churn_active, "churn should be active after first call");

        let demote_count = events
            .iter()
            .filter(|e| matches!(e, GovernorEvent::Demote(_)))
            .count();
        // With 20 hot and reduced target of 12, we expect 8 Demote events.
        assert!(
            demote_count >= 8,
            "Expected at least 8 Demote events during churn start; got {demote_count}"
        );
    }

    #[test]
    fn test_churn_recently_churned_populated() {
        let pm = setup_pm_with_peers(20, 5, 50);
        let mut gov = Governor::new(GovernorConfig {
            churn_interval_normal_secs: 0,
            ..GovernorConfig::default()
        });
        gov.last_churn = Instant::now() - Duration::from_secs(10);

        let events = gov.maybe_churn(&pm);
        assert!(gov.churn_active);

        // recently_churned should contain the addresses we emitted Demote for
        let demoted_addrs: std::collections::HashSet<SocketAddr> = events
            .iter()
            .filter_map(|e| match e {
                GovernorEvent::Demote(a) => Some(*a),
                _ => None,
            })
            .collect();

        for addr in &demoted_addrs {
            assert!(
                gov.recently_churned().contains(addr),
                "Demoted peer {addr} must appear in recently_churned set"
            );
        }
    }

    #[test]
    fn test_churn_does_not_churn_local_roots() {
        // Set up a PM where all hot peers are local roots — churn must not
        // demote any of them.
        let pm_config = PeerManagerConfig {
            target_hot_peers: 20,
            target_warm_peers: 20,
            target_known_peers: 200,
            ..PeerManagerConfig::default()
        };
        let mut pm = PeerManager::new(pm_config);
        for i in 0..10u16 {
            let addr = test_addr(4000 + i);
            pm.add_config_peer(addr, true, false); // trustable = local root
            pm.peer_connected(&addr, 14, ConnectionDirection::Outbound);
            pm.promote_to_hot(&addr);
        }

        let mut gov = Governor::new(GovernorConfig {
            churn_interval_normal_secs: 0,
            normal_targets: PeerTargets {
                active_peers: 5, // target lower than actual hot count
                established_peers: 10,
                ..PeerTargets::default()
            },
            ..GovernorConfig::default()
        });
        gov.last_churn = Instant::now() - Duration::from_secs(10);

        let events = gov.maybe_churn(&pm);

        let demoted: Vec<SocketAddr> = events
            .iter()
            .filter_map(|e| match e {
                GovernorEvent::Demote(a) => Some(*a),
                _ => None,
            })
            .collect();

        for addr in &demoted {
            assert!(
                !pm.is_local_root(addr),
                "Local root {addr} must never be churned"
            );
        }
    }

    #[test]
    fn test_churn_recently_churned_cleared_on_restore() {
        let pm = setup_pm_with_peers(20, 5, 50);
        let mut gov = Governor::new(GovernorConfig {
            churn_interval_normal_secs: 0,
            ..GovernorConfig::default()
        });

        // Start churn
        gov.last_churn = Instant::now() - Duration::from_secs(10);
        gov.maybe_churn(&pm);
        assert!(
            !gov.recently_churned().is_empty(),
            "should be non-empty after churn start"
        );

        // Complete churn (restore phase)
        gov.last_churn = Instant::now() - Duration::from_secs(10);
        gov.maybe_churn(&pm);
        assert!(!gov.churn_active, "churn should be complete");
        assert!(
            gov.recently_churned().is_empty(),
            "recently_churned should be cleared after churn completes"
        );
    }

    // ── Phase 4: Governor-driven peer sharing ─────────────────────────────────

    #[test]
    fn test_peer_sharing_emitted_when_known_below_target() {
        // Build a PM with fewer known peers than target, with some outbound warm peers.
        let pm_config = PeerManagerConfig {
            target_known_peers: 200,
            target_hot_peers: 20,
            target_warm_peers: 20,
            ..PeerManagerConfig::default()
        };
        let mut pm = PeerManager::new(pm_config);

        // Add a few warm outbound peers (eligible for sharing requests)
        let w1 = test_addr(5001);
        let w2 = test_addr(5002);
        let w3 = test_addr(5003);
        pm.add_config_peer(w1, false, true);
        pm.add_config_peer(w2, false, true);
        pm.add_config_peer(w3, false, true);
        pm.peer_connected(&w1, 14, ConnectionDirection::Outbound);
        pm.peer_connected(&w2, 14, ConnectionDirection::Outbound);
        pm.peer_connected(&w3, 14, ConnectionDirection::Outbound);
        // Total known = 3; target = 50 in our governor config → deficit

        let config = GovernorConfig {
            normal_targets: PeerTargets {
                known_peers: 50, // 3 known vs target 50 → trigger sharing
                active_peers: 1,
                established_peers: 3,
                ..PeerTargets::default()
            },
            ..GovernorConfig::default()
        };
        let mut gov = Governor::new(config);
        let events = gov.evaluate(&pm);

        let sharing_events: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, GovernorEvent::RequestPeerSharing(_, _)))
            .collect();

        assert!(
            !sharing_events.is_empty(),
            "Expected RequestPeerSharing events when known peers < target; got: {events:?}"
        );
        // At most 2 per cycle
        assert!(
            sharing_events.len() <= 2,
            "At most 2 peer sharing requests per cycle; got {}",
            sharing_events.len()
        );
        // Verify count=10 in every request
        for e in &sharing_events {
            if let GovernorEvent::RequestPeerSharing(_, count) = e {
                assert_eq!(
                    *count, 10,
                    "Peer sharing request count should be 10, got {count}"
                );
            }
        }
    }

    #[test]
    fn test_peer_sharing_rate_limited() {
        // After one RequestPeerSharing batch, a second immediate evaluate()
        // must NOT produce another batch (rate limit = 60s).
        let pm_config = PeerManagerConfig {
            target_known_peers: 200,
            target_hot_peers: 20,
            target_warm_peers: 20,
            ..PeerManagerConfig::default()
        };
        let mut pm = PeerManager::new(pm_config);
        let w1 = test_addr(5001);
        pm.add_config_peer(w1, false, true);
        pm.peer_connected(&w1, 14, ConnectionDirection::Outbound);

        let config = GovernorConfig {
            normal_targets: PeerTargets {
                known_peers: 50,
                active_peers: 1,
                established_peers: 1,
                ..PeerTargets::default()
            },
            ..GovernorConfig::default()
        };
        let mut gov = Governor::new(config);

        // First call should emit sharing events
        let events1 = gov.evaluate(&pm);
        let sharing1 = events1
            .iter()
            .filter(|e| matches!(e, GovernorEvent::RequestPeerSharing(_, _)))
            .count();
        assert!(sharing1 > 0, "First evaluate should emit peer sharing");

        // Immediate second call must not emit another batch
        let events2 = gov.evaluate(&pm);
        let sharing2 = events2
            .iter()
            .filter(|e| matches!(e, GovernorEvent::RequestPeerSharing(_, _)))
            .count();
        assert_eq!(
            sharing2, 0,
            "Second immediate evaluate must not emit peer sharing (rate limited)"
        );
    }

    #[test]
    fn test_peer_sharing_not_emitted_when_at_target() {
        // When known peers == target, no peer sharing should be requested.
        let pm_config = PeerManagerConfig {
            target_known_peers: 200,
            target_hot_peers: 5,
            target_warm_peers: 5,
            ..PeerManagerConfig::default()
        };
        let mut pm = PeerManager::new(pm_config);

        // Fill exactly to the governor known-peers target (3 peers)
        let config = GovernorConfig {
            normal_targets: PeerTargets {
                known_peers: 3,
                active_peers: 1,
                established_peers: 2,
                ..PeerTargets::default()
            },
            ..GovernorConfig::default()
        };
        let w1 = test_addr(5001);
        let w2 = test_addr(5002);
        let w3 = test_addr(5003);
        pm.add_config_peer(w1, false, true);
        pm.add_config_peer(w2, false, true);
        pm.add_config_peer(w3, false, true);
        pm.peer_connected(&w1, 14, ConnectionDirection::Outbound);
        pm.peer_connected(&w2, 14, ConnectionDirection::Outbound);
        pm.peer_connected(&w3, 14, ConnectionDirection::Outbound);

        let mut gov = Governor::new(config);
        let events = gov.evaluate(&pm);

        let sharing_count = events
            .iter()
            .filter(|e| matches!(e, GovernorEvent::RequestPeerSharing(_, _)))
            .count();
        assert_eq!(
            sharing_count, 0,
            "No peer sharing when known peers >= target"
        );
    }

    #[test]
    fn test_peer_sharing_only_requests_outbound_peers() {
        // Inbound peers must never be targets of RequestPeerSharing.
        let pm_config = PeerManagerConfig {
            target_known_peers: 200,
            target_hot_peers: 5,
            target_warm_peers: 5,
            ..PeerManagerConfig::default()
        };
        let mut pm = PeerManager::new(pm_config);

        let inbound1 = test_addr(6001);
        let inbound2 = test_addr(6002);
        pm.add_config_peer(inbound1, false, false);
        pm.add_config_peer(inbound2, false, false);
        pm.peer_connected(&inbound1, 14, ConnectionDirection::Inbound);
        pm.peer_connected(&inbound2, 14, ConnectionDirection::Inbound);

        let config = GovernorConfig {
            normal_targets: PeerTargets {
                known_peers: 50, // deficit to trigger sharing logic
                active_peers: 1,
                established_peers: 2,
                ..PeerTargets::default()
            },
            ..GovernorConfig::default()
        };
        let mut gov = Governor::new(config);
        let events = gov.evaluate(&pm);

        let sharing_addrs: Vec<SocketAddr> = events
            .iter()
            .filter_map(|e| match e {
                GovernorEvent::RequestPeerSharing(a, _) => Some(*a),
                _ => None,
            })
            .collect();

        // No outbound peers → no sharing requests should be emitted
        assert!(
            sharing_addrs.is_empty(),
            "RequestPeerSharing must only target outbound peers; got: {sharing_addrs:?}"
        );
    }

    // ── Phase 5: Known-peer surplus eviction ─────────────────────────────────

    #[test]
    fn test_known_surplus_evicts_cold_peers() {
        // Set up a PM with far more known peers than 1.5× the governor's target.
        let pm_config = PeerManagerConfig {
            target_known_peers: 500,
            target_hot_peers: 5,
            target_warm_peers: 5,
            ..PeerManagerConfig::default()
        };
        let mut pm = PeerManager::new(pm_config);

        // Add 20 ledger (non-config) cold peers
        for i in 0..20u16 {
            pm.add_ledger_peer(test_addr(7000 + i));
        }

        let config = GovernorConfig {
            normal_targets: PeerTargets {
                known_peers: 10, // 20 known vs target 10 → high-water = 15 → surplus = 5
                active_peers: 0,
                established_peers: 0,
                ..PeerTargets::default()
            },
            ..GovernorConfig::default()
        };
        let mut gov = Governor::new(config);
        let events = gov.evaluate(&pm);

        let evict_count = events
            .iter()
            .filter(|e| matches!(e, GovernorEvent::EvictColdPeer(_)))
            .count();

        // high_water = 10 * 3 / 2 = 15; surplus = 20 - 15 = 5
        assert_eq!(
            evict_count, 5,
            "Expected exactly 5 EvictColdPeer events; got {evict_count}"
        );
    }

    #[test]
    fn test_known_surplus_never_evicts_config_peers() {
        // Even when known count is far above 1.5× target, config peers must
        // not appear in EvictColdPeer events.
        let pm_config = PeerManagerConfig {
            target_known_peers: 500,
            target_hot_peers: 5,
            target_warm_peers: 5,
            ..PeerManagerConfig::default()
        };
        let mut pm = PeerManager::new(pm_config);

        // 20 config peers (protected) + 10 ledger peers (evictable)
        for i in 0..20u16 {
            pm.add_config_peer(test_addr(7000 + i), false, false);
        }
        for i in 0..10u16 {
            pm.add_ledger_peer(test_addr(8000 + i));
        }

        let config = GovernorConfig {
            normal_targets: PeerTargets {
                known_peers: 10, // 30 known vs target 10 → high_water=15 → surplus=15
                active_peers: 0,
                established_peers: 0,
                ..PeerTargets::default()
            },
            ..GovernorConfig::default()
        };
        let mut gov = Governor::new(config);
        let events = gov.evaluate(&pm);

        let evicted_addrs: Vec<SocketAddr> = events
            .iter()
            .filter_map(|e| match e {
                GovernorEvent::EvictColdPeer(a) => Some(*a),
                _ => None,
            })
            .collect();

        // Config peers (7000+) must never be evicted
        for addr in &evicted_addrs {
            assert!(
                !pm.is_local_root(addr),
                "Config peer {addr} must never be evicted"
            );
            // Verify these are ledger peers (8000+ port range)
            assert!(
                addr.port() >= 8000,
                "Only ledger peers should be evicted; got {addr}"
            );
        }

        // All evictions must be within the ledger peer set (max 10 ledger peers)
        assert!(
            evicted_addrs.len() <= 10,
            "Cannot evict more ledger peers than exist (10); got {}",
            evicted_addrs.len()
        );
    }

    #[test]
    fn test_no_eviction_when_below_high_water() {
        // When known count <= 1.5× target, no EvictColdPeer events should appear.
        let pm_config = PeerManagerConfig {
            target_known_peers: 500,
            target_hot_peers: 0,
            target_warm_peers: 0,
            ..PeerManagerConfig::default()
        };
        let mut pm = PeerManager::new(pm_config);

        // 12 ledger peers, target=10 → high_water=15 → 12 <= 15 → no eviction
        for i in 0..12u16 {
            pm.add_ledger_peer(test_addr(9000 + i));
        }

        let config = GovernorConfig {
            normal_targets: PeerTargets {
                known_peers: 10,
                active_peers: 0,
                established_peers: 0,
                ..PeerTargets::default()
            },
            ..GovernorConfig::default()
        };
        let mut gov = Governor::new(config);
        let events = gov.evaluate(&pm);

        let evict_count = events
            .iter()
            .filter(|e| matches!(e, GovernorEvent::EvictColdPeer(_)))
            .count();
        assert_eq!(
            evict_count, 0,
            "No eviction when known peers is within 1.5× target"
        );
    }

    // ── Phase 5b: BLP preemption during Syncing ───────────────────────────────

    #[test]
    fn test_blp_preemption_during_syncing() {
        // In Syncing mode with 0 active BLPs but target_blp=2:
        // non-BLP hot peers should be demoted to make room for BLPs.
        let pm_config = PeerManagerConfig {
            target_hot_peers: 20,
            target_warm_peers: 20,
            target_known_peers: 200,
            ..PeerManagerConfig::default()
        };
        let mut pm = PeerManager::new(pm_config);

        // Add 3 non-BLP hot peers
        for i in 0..3u16 {
            let addr = test_addr(10000 + i);
            pm.add_config_peer(addr, false, false);
            pm.peer_connected(&addr, 14, ConnectionDirection::Outbound);
            pm.promote_to_hot(&addr);
        }
        // Add 2 cold BLPs (not yet hot)
        pm.add_big_ledger_peer(test_addr(11000));
        pm.add_big_ledger_peer(test_addr(11001));

        let config = GovernorConfig {
            sync_targets: PeerTargets {
                active_blp: 2, // we want 2 active BLPs but have 0
                established_blp: 2,
                active_peers: 5,
                established_peers: 10,
                ..PeerTargets::default()
            },
            ..GovernorConfig::default()
        };
        let mut gov = Governor::new(config);
        gov.set_sync_state(SyncState::Syncing);

        let events = gov.evaluate(&pm);

        let demoted: Vec<SocketAddr> = events
            .iter()
            .filter_map(|e| match e {
                GovernorEvent::Demote(a) => Some(*a),
                _ => None,
            })
            .collect();

        // At least 2 non-BLP hot peers should be demoted (one per BLP needed)
        assert!(
            demoted.len() >= 2,
            "Expected >= 2 Demote events for BLP preemption; got {demoted:?}"
        );
        // None of the demoted peers should be BLPs
        for addr in &demoted {
            assert_ne!(
                pm.peer_category(addr),
                Some(PeerCategory::BigLedgerPeer),
                "BLP {addr} must not be demoted during preemption"
            );
        }
    }

    #[test]
    fn test_blp_preemption_not_active_when_caught_up() {
        // BLP preemption only fires in Syncing; CaughtUp should not preempt.
        let pm_config = PeerManagerConfig {
            target_hot_peers: 20,
            target_warm_peers: 20,
            target_known_peers: 200,
            ..PeerManagerConfig::default()
        };
        let mut pm = PeerManager::new(pm_config);

        for i in 0..5u16 {
            let addr = test_addr(12000 + i);
            pm.add_config_peer(addr, false, false);
            pm.peer_connected(&addr, 14, ConnectionDirection::Outbound);
            pm.promote_to_hot(&addr);
        }
        pm.add_big_ledger_peer(test_addr(13000));
        pm.add_big_ledger_peer(test_addr(13001));

        let config = GovernorConfig {
            normal_targets: PeerTargets {
                active_blp: 2,
                established_blp: 2,
                active_peers: 5, // exactly at target — no normal surplus
                established_peers: 10,
                ..PeerTargets::default()
            },
            ..GovernorConfig::default()
        };
        let mut gov = Governor::new(config);
        gov.set_sync_state(SyncState::CaughtUp);

        let events = gov.evaluate(&pm);

        // No preemption demotion — we're CaughtUp and at normal target
        let preemption_demotes: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, GovernorEvent::Demote(_)))
            .collect();
        assert!(
            preemption_demotes.is_empty(),
            "BLP preemption should not fire when CaughtUp: {preemption_demotes:?}"
        );
    }

    // ── xorshift64 PRNG properties ────────────────────────────────────────────

    #[test]
    fn test_xorshift64_never_zero() {
        let mut gov = Governor::new(GovernorConfig::default());
        // Override seed to a known non-zero value for determinism
        gov.churn_seed = 0xdeadbeef_cafebabe;
        for _ in 0..10_000 {
            let v = gov.xorshift64_next();
            assert_ne!(v, 0, "xorshift64 must never produce 0");
        }
    }

    #[test]
    fn test_shuffle_produces_all_elements() {
        let mut gov = Governor::new(GovernorConfig::default());
        gov.churn_seed = 0x0102030405060708;
        let original: Vec<u32> = (0..20).collect();
        let mut shuffled = original.clone();
        gov.shuffle(&mut shuffled);

        // All elements must still be present (permutation, not filtering)
        let mut sorted = shuffled.clone();
        sorted.sort();
        assert_eq!(sorted, original, "Shuffle must preserve all elements");
    }

    #[test]
    fn test_shuffle_is_not_identity_for_large_vec() {
        // With 20 elements there is essentially zero probability the shuffle
        // returns the exact same order.  If this flakes it means the PRNG is
        // broken (or astronomically unlikely).
        let mut gov = Governor::new(GovernorConfig::default());
        gov.churn_seed = 0x1234567890abcdef;
        let original: Vec<u32> = (0..20).collect();
        let mut shuffled = original.clone();
        gov.shuffle(&mut shuffled);
        assert_ne!(
            shuffled, original,
            "Shuffle of 20 elements should not be the identity permutation"
        );
    }

    // ── #199: Warm-to-hot promotion liveness gate ─────────────────────────────

    /// A warm peer that has accumulated failures, has not fetched any blocks,
    /// and connected more than PROMOTION_NEW_PEER_GRACE ago must NOT be
    /// promoted to hot.  (Peers with zero failures are always eligible for
    /// promotion to allow TxSubmission2-only connections.)
    #[test]
    fn test_promotion_blocked_for_idle_peer_past_grace() {
        use crate::peer_manager::PROMOTION_NEW_PEER_GRACE;
        let pm_config = PeerManagerConfig {
            target_hot_peers: 5,
            target_warm_peers: 5,
            target_known_peers: 100,
            ..PeerManagerConfig::default()
        };
        let mut pm = PeerManager::new(pm_config);

        let addr = test_addr(20001);
        pm.add_config_peer(addr, false, false);
        pm.peer_connected(&addr, 14, ConnectionDirection::Outbound);
        // Force last_connected to be well past the grace period.
        pm.set_last_connected(
            &addr,
            Instant::now() - PROMOTION_NEW_PEER_GRACE - Duration::from_secs(10),
        );
        // Set a non-zero failure count so the zero-failure bypass doesn't apply.
        pm.set_failure_count(&addr, 1);
        // blocks_fetched is still 0 and last_good_fetch is None.

        let config = GovernorConfig {
            normal_targets: PeerTargets {
                active_peers: 5,
                established_peers: 10,
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

        assert!(
            !promotes.contains(&addr),
            "Idle peer past grace should NOT be promoted; promotions={promotes:?}"
        );
    }

    /// A warm peer that has fetched at least one block MUST be eligible for
    /// promotion regardless of connection age.
    #[test]
    fn test_promotion_allowed_for_peer_with_blocks() {
        let pm_config = PeerManagerConfig {
            target_hot_peers: 5,
            target_warm_peers: 5,
            target_known_peers: 100,
            ..PeerManagerConfig::default()
        };
        let mut pm = PeerManager::new(pm_config);

        let addr = test_addr(20002);
        pm.add_config_peer(addr, false, false);
        pm.peer_connected(&addr, 14, ConnectionDirection::Outbound);
        // Record a block fetch so the liveness gate is satisfied.
        pm.record_block_fetch(&addr, 50.0, 1, 1024);

        let config = GovernorConfig {
            normal_targets: PeerTargets {
                active_peers: 5,
                established_peers: 10,
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

        assert!(
            promotes.contains(&addr),
            "Peer with blocks fetched should be promoted; promotions={promotes:?}"
        );
    }

    /// A warm peer that connected very recently (within grace) with no blocks
    /// must still be eligible for promotion.
    #[test]
    fn test_promotion_allowed_during_grace_period() {
        let pm_config = PeerManagerConfig {
            target_hot_peers: 5,
            target_warm_peers: 5,
            target_known_peers: 100,
            ..PeerManagerConfig::default()
        };
        let mut pm = PeerManager::new(pm_config);

        let addr = test_addr(20003);
        pm.add_config_peer(addr, false, false);
        // peer_connected sets last_connected = Instant::now(), so we're within grace.
        pm.peer_connected(&addr, 14, ConnectionDirection::Outbound);
        // No blocks fetched yet; failure_count = 0.

        let config = GovernorConfig {
            normal_targets: PeerTargets {
                active_peers: 5,
                established_peers: 10,
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

        assert!(
            promotes.contains(&addr),
            "Peer within grace period should be promotable; promotions={promotes:?}"
        );
    }

    /// Local root peers bypass the liveness gate and are always eligible.
    #[test]
    fn test_promotion_local_root_bypasses_liveness_gate() {
        use crate::peer_manager::PROMOTION_NEW_PEER_GRACE;
        let pm_config = PeerManagerConfig {
            target_hot_peers: 5,
            target_warm_peers: 5,
            target_known_peers: 100,
            ..PeerManagerConfig::default()
        };
        let mut pm = PeerManager::new(pm_config);

        let addr = test_addr(20004);
        pm.add_config_peer(addr, true, false); // trustable = LocalRoot
        pm.peer_connected(&addr, 14, ConnectionDirection::Outbound);
        // Force connection time to be past grace, and zero blocks fetched.
        pm.set_last_connected(
            &addr,
            Instant::now() - PROMOTION_NEW_PEER_GRACE - Duration::from_secs(60),
        );

        // Register as a local root group so Phase 0 fires.
        pm.add_local_root_group(vec![addr], 1, 1);

        let mut gov = Governor::new(GovernorConfig::default());
        let events = gov.evaluate(&pm);

        let promotes: Vec<SocketAddr> = events
            .iter()
            .filter_map(|e| match e {
                GovernorEvent::Promote(a) => Some(*a),
                _ => None,
            })
            .collect();

        assert!(
            promotes.contains(&addr),
            "Local root peer must bypass liveness gate and be promoted; promotions={promotes:?}"
        );
    }

    // ── #200: Stall detection and error-rate demotion ─────────────────────────

    /// A hot peer that serves no new blocks for stall_demotion_cycles consecutive
    /// evaluation cycles must be demoted back to warm.  The peer must have
    /// fetched at least one block initially — peers with `blocks_fetched == 0`
    /// are exempt from stall checks (TxSubmission2-only connections).
    #[test]
    fn test_stall_demotion_after_threshold_cycles() {
        let pm_config = PeerManagerConfig {
            target_hot_peers: 5,
            target_warm_peers: 5,
            target_known_peers: 100,
            ..PeerManagerConfig::default()
        };
        let mut pm = PeerManager::new(pm_config);

        let addr = test_addr(21001);
        pm.add_config_peer(addr, false, false);
        pm.peer_connected(&addr, 14, ConnectionDirection::Outbound);
        pm.promote_to_hot(&addr);
        // Record one block fetch so the peer is subject to stall detection.
        pm.record_block_fetch(&addr, 50.0, 1, 1024);

        // Use a small threshold to make the test fast.
        let threshold = 3u32;
        // active_peers must be < hot_count for stall demotion to apply
        // (stall demotion is suppressed when at/below target to avoid
        // promote→demote loops).  We have 1 hot peer, so target 0.
        let config = GovernorConfig {
            normal_targets: PeerTargets {
                active_peers: 0,
                established_peers: 10,
                ..PeerTargets::default()
            },
            stall_demotion_cycles: threshold,
            ..GovernorConfig::default()
        };
        let mut gov = Governor::new(config);

        // Run `threshold` cycles without the peer serving any blocks.
        // On cycle 0 the snapshot is inserted (stale_cycles = 0).
        // On each subsequent cycle it is incremented.
        // Demotion fires when stale_cycles >= threshold.
        let mut demoted = false;
        for _ in 0..=threshold {
            let events = gov.evaluate(&pm);
            if events
                .iter()
                .any(|e| matches!(e, GovernorEvent::Demote(a) if *a == addr))
            {
                demoted = true;
                break;
            }
            // Simulate the node applying the Demote event (drop hot flag).
            // Without this the peer stays hot for subsequent cycles.
        }

        assert!(
            demoted,
            "Hot peer with no block activity should be demoted after {threshold} stale cycles"
        );
    }

    /// A hot peer that continues to serve blocks must NOT be stall-demoted.
    #[test]
    fn test_no_stall_demotion_for_active_peer() {
        let pm_config = PeerManagerConfig {
            target_hot_peers: 5,
            target_warm_peers: 5,
            target_known_peers: 100,
            ..PeerManagerConfig::default()
        };
        let mut pm = PeerManager::new(pm_config);

        let addr = test_addr(21002);
        pm.add_config_peer(addr, false, false);
        pm.peer_connected(&addr, 14, ConnectionDirection::Outbound);
        pm.promote_to_hot(&addr);

        // active_peers: 1 — peer is at target so stall demotion is
        // suppressed (by design, to avoid promote→demote loops).
        let config = GovernorConfig {
            normal_targets: PeerTargets {
                active_peers: 1,
                established_peers: 10,
                ..PeerTargets::default()
            },
            stall_demotion_cycles: 2,
            ..GovernorConfig::default()
        };
        let mut gov = Governor::new(config);

        // Run several cycles, recording one block between each so the
        // block count always grows.
        for i in 1..=5u64 {
            pm.record_block_fetch(&addr, 30.0, i, 512 * i);
            let events = gov.evaluate(&pm);
            let demoted = events
                .iter()
                .any(|e| matches!(e, GovernorEvent::Demote(a) if *a == addr));
            assert!(
                !demoted,
                "Active hot peer must not be stall-demoted at cycle {i}"
            );
        }
    }

    /// A hot peer that exceeds the error threshold must be demoted regardless
    /// of its block activity.
    #[test]
    fn test_error_rate_demotion() {
        let pm_config = PeerManagerConfig {
            target_hot_peers: 5,
            target_warm_peers: 5,
            target_known_peers: 100,
            ..PeerManagerConfig::default()
        };
        let mut pm = PeerManager::new(pm_config);

        let addr = test_addr(21003);
        pm.add_config_peer(addr, false, false);
        pm.peer_connected(&addr, 14, ConnectionDirection::Outbound);
        pm.promote_to_hot(&addr);
        // Record some blocks to avoid stall demotion path.
        pm.record_block_fetch(&addr, 20.0, 5, 4096);

        // Artificially inflate the failure count beyond the threshold.
        let threshold = 3u32;
        pm.set_failure_count(&addr, threshold + 1);

        // active_peers: 0 so stall/error logic applies.
        let config = GovernorConfig {
            normal_targets: PeerTargets {
                active_peers: 0,
                established_peers: 10,
                ..PeerTargets::default()
            },
            error_demotion_threshold: threshold,
            ..GovernorConfig::default()
        };
        let mut gov = Governor::new(config);
        let events = gov.evaluate(&pm);

        let demoted = events
            .iter()
            .any(|e| matches!(e, GovernorEvent::Demote(a) if *a == addr));
        assert!(
            demoted,
            "Peer exceeding error threshold must be demoted; events={events:?}"
        );
    }

    /// Local root peers must never be demoted by stall or error checks.
    #[test]
    fn test_stall_and_error_exempt_local_root() {
        let pm_config = PeerManagerConfig {
            target_hot_peers: 5,
            target_warm_peers: 5,
            target_known_peers: 100,
            ..PeerManagerConfig::default()
        };
        let mut pm = PeerManager::new(pm_config);

        let addr = test_addr(21004);
        pm.add_config_peer(addr, true, false); // trustable = LocalRoot
        pm.peer_connected(&addr, 14, ConnectionDirection::Outbound);
        pm.promote_to_hot(&addr);
        // High failure count.
        pm.set_failure_count(&addr, 100);

        // active_peers: 0 so stall/error logic applies.
        let config = GovernorConfig {
            normal_targets: PeerTargets {
                active_peers: 0,
                established_peers: 10,
                ..PeerTargets::default()
            },
            stall_demotion_cycles: 1,
            error_demotion_threshold: 2,
            ..GovernorConfig::default()
        };
        let mut gov = Governor::new(config);

        // Run multiple cycles with no blocks — should never demote local root.
        for cycle in 0..5 {
            let events = gov.evaluate(&pm);
            let demoted = events
                .iter()
                .any(|e| matches!(e, GovernorEvent::Demote(a) if *a == addr));
            assert!(
                !demoted,
                "Local root must never be stall/error-demoted (cycle {cycle})"
            );
        }
    }

    /// The stall snapshot is updated correctly: stale_cycles resets when blocks grow.
    #[test]
    fn test_stall_snapshot_resets_on_activity() {
        let pm_config = PeerManagerConfig {
            target_hot_peers: 5,
            target_warm_peers: 5,
            target_known_peers: 100,
            ..PeerManagerConfig::default()
        };
        let mut pm = PeerManager::new(pm_config);

        let addr = test_addr(21005);
        pm.add_config_peer(addr, false, false);
        pm.peer_connected(&addr, 14, ConnectionDirection::Outbound);
        pm.promote_to_hot(&addr);

        // active_peers: 0 so stall logic applies.
        let config = GovernorConfig {
            normal_targets: PeerTargets {
                active_peers: 0,
                established_peers: 10,
                ..PeerTargets::default()
            },
            stall_demotion_cycles: 5, // high threshold so we don't demote
            ..GovernorConfig::default()
        };
        let mut gov = Governor::new(config);

        // Cycle 1: no blocks → snapshot inserted with stale_cycles = 0.
        gov.evaluate(&pm);
        // After cycle 1 the snapshot is (0 blocks, 0 stale).

        // Cycle 2: no blocks → stale_cycles becomes 1.
        gov.evaluate(&pm);
        let (_, stale) = *gov.hot_peer_stall().get(&addr).unwrap();
        assert_eq!(stale, 1, "stale_cycles should be 1 after two idle cycles");

        // Cycle 3: serve blocks → stale_cycles should reset to 0.
        pm.record_block_fetch(&addr, 10.0, 10, 8192);
        gov.evaluate(&pm);
        let (snap_blocks, stale) = *gov.hot_peer_stall().get(&addr).unwrap();
        assert_eq!(stale, 0, "stale_cycles must reset after blocks are served");
        assert_eq!(
            snap_blocks, 10,
            "snapshot block count should reflect new total"
        );
    }

    // ── #201: Configurable connection targets ─────────────────────────────────

    /// GovernorConfig fields read from NodeConfig must be reflected in the governor.
    #[test]
    fn test_configurable_targets_reflected_in_governor() {
        let config = GovernorConfig {
            normal_targets: PeerTargets {
                root_peers: 30,
                known_peers: 42,
                established_peers: 20,
                active_peers: 8,
                known_blp: 7,
                established_blp: 5,
                active_blp: 3,
            },
            churn_interval_normal_secs: 600,
            churn_interval_sync_secs: 120,
            stall_demotion_cycles: 4,
            error_demotion_threshold: 7,
            ..GovernorConfig::default()
        };
        let gov = Governor::new(config.clone());

        // Verify that the governor honours every customised field.
        assert_eq!(gov.config().normal_targets.active_peers, 8);
        assert_eq!(gov.config().normal_targets.known_peers, 42);
        assert_eq!(gov.config().normal_targets.root_peers, 30);
        assert_eq!(gov.config().normal_targets.established_peers, 20);
        assert_eq!(gov.config().normal_targets.active_blp, 3);
        assert_eq!(gov.config().normal_targets.known_blp, 7);
        assert_eq!(gov.config().normal_targets.established_blp, 5);
        assert_eq!(gov.config().churn_interval_normal_secs, 600);
        assert_eq!(gov.config().churn_interval_sync_secs, 120);
        assert_eq!(gov.config().stall_demotion_cycles, 4);
        assert_eq!(gov.config().error_demotion_threshold, 7);
    }

    /// Changing only the churn interval does not affect peer count targets.
    #[test]
    fn test_custom_churn_interval_does_not_fire_early() {
        // When churn_interval_normal_secs is large, maybe_churn should be a no-op
        // for a freshly constructed governor.
        let pm = setup_pm_with_peers(5, 5, 20);
        let config = GovernorConfig {
            churn_interval_normal_secs: 99999,
            ..GovernorConfig::default()
        };
        let mut gov = Governor::new(config);

        let events = gov.maybe_churn(&pm);
        assert!(
            events.is_empty(),
            "maybe_churn must not fire before the interval elapses; events={events:?}"
        );
        assert!(!gov.churn_active, "churn must not be active yet");
    }
}
