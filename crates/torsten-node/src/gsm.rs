//! Genesis State Machine (GSM) for bootstrap from genesis.
//!
//! Manages the node's sync progression through three states:
//! - **PreSyncing**: Waiting for enough trusted big ledger peers (HAA)
//! - **Syncing**: Active block download with LoE/GDD protection
//! - **CaughtUp**: Normal Praos operation at chain tip
//!
//! The GSM runs as a background task, monitoring peer counts and tip age
//! to drive state transitions. It also enforces the Limit on Eagerness (LoE)
//! and runs the Genesis Density Disconnector (GDD).
//!
//! ## LoE enforcement
//!
//! `compute_loe_slot()` returns the maximum immutable tip slot that block
//! application should advance to:
//! - **PreSyncing**: `Some(0)` — freeze the immutable tip at genesis.
//! - **Syncing**: `Some(min_intersection)` — the minimum intersection slot
//!   across all tracked peers, ensuring the immutable tip cannot pass any
//!   peer's fork point.
//! - **CaughtUp**: `None` — no constraint; normal unconstrained flushing.
//!
//! ## GDD (Genesis Density Disconnector)
//!
//! During Syncing state the GSM maintains a per-peer `DensityWindow` tracking
//! how many blocks each peer's chain contains within the genesis window
//! `(intersection_slot, intersection_slot + sgen]`.  On each GDD evaluation
//! the 4-guard Haskell `densityDisconnect` algorithm is applied pairwise:
//! a peer is disconnected when its density upper-bound is dominated by
//! another peer's density lower-bound, subject to idling, signal, and
//! meaningful-comparison guards.
//!
//! ## GSM Actor
//!
//! `run_gsm_actor` owns the `GenesisStateMachine` and communicates with
//! the rest of the node via channels:
//! - **`GsmEvent`** (mpsc): events from ChainSync, BlockFetch, networking
//! - **`GsmSnapshot`** (watch): current state broadcast to consumers
//! - **`GddAction`** (mpsc): disconnect commands sent to the peer manager
//!
//! ## Current Limitations
//!
//! - **Lightweight checkpointing**: The Ouroboros Genesis specification calls
//!   for lightweight checkpoints to speed up initial sync. Not yet implemented.
//!
//! - **Genesis-specific peer selection**: Full Genesis requires a dedicated
//!   peer selection policy that prioritises big ledger peers (BLPs). Currently,
//!   peer selection uses the standard P2P governor policy.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, watch};
use torsten_consensus::DensityWindow;
use tracing::{debug, info, warn};

// ── Sync state ──────────────────────────────────────────────────────────────

/// Genesis sync state matching Ouroboros Genesis specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenesisSyncState {
    /// Waiting for enough trusted big ledger peers (HAA satisfied)
    PreSyncing,
    /// Active block download with LoE/GDD protection
    Syncing,
    /// Normal Praos operation — node is at or near chain tip
    CaughtUp,
}

impl std::fmt::Display for GenesisSyncState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GenesisSyncState::PreSyncing => write!(f, "PreSyncing"),
            GenesisSyncState::Syncing => write!(f, "Syncing"),
            GenesisSyncState::CaughtUp => write!(f, "CaughtUp"),
        }
    }
}

// ── Event / snapshot / action types ─────────────────────────────────────────

/// Event sent to the GSM actor from producers (ChainSync, BlockFetch, networking).
#[derive(Debug)]
pub enum GsmEvent {
    /// A new peer has been registered with known intersection and tip.
    PeerRegistered {
        addr: SocketAddr,
        intersection_slot: u64,
        tip_slot: u64,
    },
    /// A peer has disconnected.
    PeerDisconnected { addr: SocketAddr },
    /// A block was received from a peer at the given slot.
    BlockReceived { addr: SocketAddr, slot: u64 },
    /// A peer's tip slot was updated (e.g., new header announcement).
    PeerTipUpdated { addr: SocketAddr, tip_slot: u64 },
    /// A peer's ChainSync client has become idle (awaiting next header).
    PeerIdling { addr: SocketAddr },
    /// A peer's ChainSync client has become active again.
    PeerActive { addr: SocketAddr },
    /// Periodic status update from the sync pipeline.
    SyncStatus {
        active_blp_count: usize,
        all_chainsync_idle: bool,
        tip_age_secs: u64,
        immutable_tip_slot: u64,
    },
}

/// Broadcast snapshot of the current GSM state.
///
/// Published via a `watch` channel so consumers always see the latest value.
#[derive(Debug, Clone, Copy)]
pub struct GsmSnapshot {
    /// Current sync state.
    pub state: GenesisSyncState,
    /// Limit on Eagerness slot, or `None` if unconstrained (CaughtUp).
    pub loe_slot: Option<u64>,
}

/// Actions the GSM actor emits for the peer manager to execute.
#[derive(Debug)]
pub enum GddAction {
    /// Disconnect this peer — GDD determined it is on a sparse chain.
    DisconnectPeer(SocketAddr),
}

// ── Configuration ───────────────────────────────────────────────────────────

/// Configuration for the Genesis State Machine.
#[derive(Debug, Clone)]
pub struct GsmConfig {
    /// Minimum active big ledger peers to transition PreSyncing → Syncing.
    pub min_active_blp: usize,
    /// Maximum tip age (seconds) to consider the node "caught up".
    /// Used in the Syncing → CaughtUp transition guard.
    pub max_caught_up_age_secs: u64,
    /// Minimum time (seconds) to stay in CaughtUp before allowing regression.
    /// Prevents thundering-herd oscillations between CaughtUp and PreSyncing.
    pub min_caught_up_dwell_secs: u64,
    /// Maximum random jitter (seconds) added to the dwell time.
    /// Prevents multiple nodes from regressing simultaneously.
    pub anti_thundering_herd_max_secs: u64,
    /// Genesis window size in slots (`sgen = 3k/f`).
    ///
    /// The GDD compares block density across peers within this window,
    /// anchored at the intersection point. Defaults to 129_600 slots
    /// (~36 hours at 1-second slot intervals, matching mainnet `k=2160, f=0.05`).
    pub genesis_window_slots: u64,
    /// Minimum interval (milliseconds) between GDD evaluations.
    /// Rate-limits the pairwise comparison to avoid CPU spikes with many peers.
    pub gdd_rate_limit_ms: u64,
    /// Security parameter `k` — the maximum rollback depth.
    /// Used by GDD guard 3 ("offers more than k").
    pub security_param_k: u64,
    /// Path for the caught_up marker file.
    pub marker_path: PathBuf,
}

impl Default for GsmConfig {
    fn default() -> Self {
        GsmConfig {
            min_active_blp: 5,
            max_caught_up_age_secs: 1200,       // 20 minutes
            min_caught_up_dwell_secs: 1200,     // 20 minutes
            anti_thundering_herd_max_secs: 300, // up to 5 minutes jitter
            genesis_window_slots: 129_600,      // 3 * 2160 / 0.05
            gdd_rate_limit_ms: 1000,            // 1 GDD tick per second
            security_param_k: 2160,             // mainnet default
            marker_path: PathBuf::from("caught_up.marker"),
        }
    }
}

// ── Per-peer chain density tracking ─────────────────────────────────────────

/// Live density information tracked per peer for GDD comparison.
///
/// The GDD requires knowing how many blocks a peer's chain places within the
/// genesis window `(I, I + sgen]` where `I` is the fork intersection slot.
/// This struct combines the sliding `DensityWindow` (which records block slots)
/// with the peer's reported tip, idling state, and latest observed block slot.
#[derive(Debug, Clone)]
pub struct PeerChainInfo {
    /// Density window tracking blocks in the genesis window for this peer.
    pub density_window: DensityWindow,
    /// Most recent tip slot reported by this peer.
    pub tip_slot: u64,
    /// Slot at which this peer's chain intersects with ours.
    pub intersection_slot: u64,
    /// Whether this peer's ChainSync client is idle (waiting for next header).
    pub idling: bool,
    /// Highest block slot actually received from this peer, or `None` if
    /// no blocks have arrived yet.
    pub latest_slot: Option<u64>,
}

impl PeerChainInfo {
    /// Create a new info record with a fresh density window.
    pub fn new(intersection_slot: u64, window_size: u64, tip_slot: u64) -> Self {
        PeerChainInfo {
            density_window: DensityWindow::new(intersection_slot, window_size),
            tip_slot,
            intersection_slot,
            idling: false,
            latest_slot: None,
        }
    }

    /// Number of blocks this peer has within the genesis window.
    #[allow(dead_code)] // public API for diagnostics and tests
    pub fn blocks_in_window(&self) -> u64 {
        self.density_window.block_count()
    }

    /// Record a block arriving at `slot` from this peer.
    ///
    /// Updates both the density window and the `latest_slot` high-water mark.
    pub fn record_block(&mut self, slot: u64) {
        self.density_window.record_block(slot);
        // Track the highest block slot seen from this peer, regardless of
        // whether it falls within the density window.
        match self.latest_slot {
            Some(prev) if slot > prev => self.latest_slot = Some(slot),
            None => self.latest_slot = Some(slot),
            _ => {}
        }
    }

    /// Update the peer's tip slot (called when a new header is received).
    pub fn update_tip(&mut self, slot: u64) {
        if slot > self.tip_slot {
            self.tip_slot = slot;
        }
    }
}

// ── GenesisStateMachine ─────────────────────────────────────────────────────

/// The Genesis State Machine.
///
/// Tracks sync state and enforces transitions based on peer availability
/// and tip freshness. The GSM also manages:
/// - **LoE (Limit on Eagerness)**: constrains block application during sync
/// - **GDD (Genesis Density Disconnector)**: disconnects sparse-chain peers
pub struct GenesisStateMachine {
    config: GsmConfig,
    state: GenesisSyncState,
    /// Whether genesis mode is enabled (opt-in via --consensus-mode genesis).
    enabled: bool,
    /// Per-peer chain density information, keyed by peer socket address.
    ///
    /// Only populated when `enabled`. Each entry holds the peer's
    /// `DensityWindow` anchored at its intersection slot plus idling/tip state.
    peer_info: HashMap<SocketAddr, PeerChainInfo>,
    /// Timestamp of when the GSM entered CaughtUp state, or `None` if it
    /// has never been CaughtUp (or has since regressed).
    caught_up_since: Option<Instant>,
    /// Random jitter (seconds) added to `min_caught_up_dwell_secs` to
    /// prevent multiple nodes from regressing simultaneously.
    anti_thundering_herd_jitter_secs: u64,
}

impl GenesisStateMachine {
    /// Create a new GSM. If not enabled, it immediately enters CaughtUp
    /// and all constraints are disabled.
    pub fn new(config: GsmConfig, enabled: bool) -> Self {
        let initial_state = if enabled {
            // Check for marker file — fast restart
            if config.marker_path.exists() {
                info!("Genesis: caught_up marker found, starting in CaughtUp state");
                GenesisSyncState::CaughtUp
            } else {
                GenesisSyncState::PreSyncing
            }
        } else {
            GenesisSyncState::CaughtUp
        };

        // Compute anti-thundering-herd jitter using PID + high-resolution
        // timestamp. This provides good per-node uniqueness without needing
        // an RNG dependency. Different from Haskell's randomRIO but achieves
        // the same goal: preventing a fleet of nodes from regressing simultaneously.
        let jitter = if config.anti_thundering_herd_max_secs > 0 {
            let pid = std::process::id() as u64;
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos() as u64;
            let seed = pid.wrapping_mul(6364136223846793005).wrapping_add(nanos);
            seed % (config.anti_thundering_herd_max_secs + 1)
        } else {
            0
        };

        let caught_up_since = if initial_state == GenesisSyncState::CaughtUp {
            Some(Instant::now())
        } else {
            None
        };

        GenesisStateMachine {
            config,
            state: initial_state,
            enabled,
            peer_info: HashMap::new(),
            caught_up_since,
            anti_thundering_herd_jitter_secs: jitter,
        }
    }

    /// Current sync state.
    pub fn state(&self) -> GenesisSyncState {
        self.state
    }

    /// Whether genesis mode is enabled.
    #[allow(dead_code)] // public API for diagnostics
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    // ── Peer density tracking ────────────────────────────────────────────────

    /// Register a newly connected peer or reset its density window.
    ///
    /// Called when the sync loop establishes an intersection with a peer.
    /// `intersection_slot` is the slot of the common point.
    pub fn register_peer(&mut self, addr: SocketAddr, intersection_slot: u64, tip_slot: u64) {
        if !self.enabled {
            return;
        }
        let info = PeerChainInfo::new(
            intersection_slot,
            self.config.genesis_window_slots,
            tip_slot,
        );
        self.peer_info.insert(addr, info);
        debug!(
            %addr,
            intersection_slot,
            window_size = self.config.genesis_window_slots,
            "GDD: registered peer"
        );
    }

    /// Remove a disconnected peer from density tracking.
    pub fn deregister_peer(&mut self, addr: &SocketAddr) {
        if self.peer_info.remove(addr).is_some() {
            debug!(%addr, "GDD: deregistered peer");
        }
    }

    /// Record a block received from `addr` at `slot`.
    ///
    /// Noop if the peer is unknown or if genesis mode is disabled.
    pub fn record_block(&mut self, addr: &SocketAddr, slot: u64) {
        if !self.enabled {
            return;
        }
        if let Some(info) = self.peer_info.get_mut(addr) {
            info.record_block(slot);
        }
    }

    /// Update the tip slot reported by `addr`.
    pub fn update_peer_tip(&mut self, addr: &SocketAddr, tip_slot: u64) {
        if let Some(info) = self.peer_info.get_mut(addr) {
            info.update_tip(tip_slot);
        }
    }

    /// Mark a peer's ChainSync client as idle.
    pub fn set_peer_idling(&mut self, addr: &SocketAddr, idling: bool) {
        if let Some(info) = self.peer_info.get_mut(addr) {
            info.idling = idling;
        }
    }

    /// Read-only view of the peer density map (for metrics / tests).
    #[allow(dead_code)] // public API for diagnostics and tests
    pub fn peer_info(&self) -> &HashMap<SocketAddr, PeerChainInfo> {
        &self.peer_info
    }

    // ── State transitions ────────────────────────────────────────────────────

    /// Evaluate state transitions based on current conditions.
    ///
    /// Returns `Some(new_state)` if a transition occurred, `None` if unchanged.
    ///
    /// # Arguments
    /// - `active_blp_count`: number of active big ledger peers
    /// - `all_chainsync_idle`: whether all ChainSync clients are idle
    /// - `tip_age_secs`: age of our chain tip in seconds
    /// - `immutable_tip_slot`: current immutable tip slot (for within-window check)
    pub fn evaluate(
        &mut self,
        active_blp_count: usize,
        all_chainsync_idle: bool,
        tip_age_secs: u64,
        immutable_tip_slot: u64,
    ) -> Option<GenesisSyncState> {
        if !self.enabled {
            return None;
        }

        let old_state = self.state;

        match self.state {
            GenesisSyncState::PreSyncing => {
                // Transition to Syncing when HAA (Honest Availability Assumption) is satisfied:
                // we have enough active big ledger peers.
                if active_blp_count >= self.config.min_active_blp {
                    self.state = GenesisSyncState::Syncing;
                    info!(
                        active_blp = active_blp_count,
                        min = self.config.min_active_blp,
                        "Genesis: HAA satisfied, transitioning to Syncing"
                    );
                }
            }
            GenesisSyncState::Syncing => {
                // HAA LOSS: if we drop below the minimum BLP count, regress.
                if active_blp_count < self.config.min_active_blp {
                    self.state = GenesisSyncState::PreSyncing;
                    self.remove_marker();
                    warn!(
                        active_blp = active_blp_count,
                        min = self.config.min_active_blp,
                        "Genesis: HAA lost, regressing to PreSyncing"
                    );
                } else if all_chainsync_idle
                    && self.all_peers_idling()
                    && tip_age_secs < self.config.max_caught_up_age_secs
                    && self.all_peers_within_window(immutable_tip_slot)
                {
                    // Transition to CaughtUp when:
                    // 1. All ChainSync clients are idle (both external heuristic
                    //    AND per-peer MsgAwaitReply tracking)
                    // 2. Our tip is fresh
                    // 3. All peers' tips are within the genesis window
                    self.state = GenesisSyncState::CaughtUp;
                    self.caught_up_since = Some(Instant::now());
                    self.write_marker();
                    info!(
                        tip_age_secs,
                        "Genesis: all peers idle and tip fresh, transitioning to CaughtUp"
                    );
                }
            }
            GenesisSyncState::CaughtUp => {
                // Regress to PreSyncing if tip becomes stale, but only after
                // the minimum dwell time has elapsed. Jitter is added to the
                // tip-age threshold (not dwell) matching Haskell's antiThunderingHerd.
                let dwell_ok = self
                    .caught_up_since
                    .map(|t| t.elapsed().as_secs() >= self.config.min_caught_up_dwell_secs)
                    .unwrap_or(true);

                if dwell_ok {
                    let threshold =
                        self.config.max_caught_up_age_secs + self.anti_thundering_herd_jitter_secs;
                    if tip_age_secs > threshold {
                        self.state = GenesisSyncState::PreSyncing;
                        self.caught_up_since = None;
                        self.remove_marker();
                        warn!(
                            tip_age_secs,
                            threshold, "Genesis: tip stale, regressing to PreSyncing"
                        );
                    }
                }
            }
        }

        if self.state != old_state {
            Some(self.state)
        } else {
            None
        }
    }

    /// Check whether all tracked peers have reported idling (MsgAwaitReply).
    /// Returns `true` if there are no peers or if every peer's `idling` flag
    /// is set. This provides a more accurate CaughtUp signal than the external
    /// time-based heuristic, matching Haskell's `csIdling` per-peer check.
    fn all_peers_idling(&self) -> bool {
        self.peer_info.is_empty() || self.peer_info.values().all(|info| info.idling)
    }

    /// Check whether all tracked peers have density windows that are within
    /// the genesis window relative to `immutable_tip_slot`.
    ///
    /// Returns `true` if there are no peers (vacuous truth) or if every peer's
    /// intersection slot is within `genesis_window_slots` of the immutable tip.
    fn all_peers_within_window(&self, immutable_tip_slot: u64) -> bool {
        self.peer_info.values().all(|info| {
            // The peer is "within window" if the immutable tip hasn't advanced
            // past the end of this peer's density window.
            let window_end = info
                .intersection_slot
                .saturating_add(self.config.genesis_window_slots);
            immutable_tip_slot <= window_end
        })
    }

    // ── LoE ─────────────────────────────────────────────────────────────────

    /// Compute the Limit on Eagerness slot.
    ///
    /// - **PreSyncing**: `Some(0)` — freeze immutable tip at genesis.
    /// - **Syncing**: `Some(min_intersection)` — minimum intersection slot
    ///   across all tracked peers, ensuring the immutable tip cannot pass
    ///   any peer's fork point.
    /// - **CaughtUp**: `None` — no constraint.
    pub fn compute_loe_slot(&self) -> Option<u64> {
        if !self.enabled {
            return None;
        }

        match self.state {
            GenesisSyncState::PreSyncing => Some(0),
            GenesisSyncState::Syncing => {
                if self.peer_info.is_empty() {
                    return Some(0);
                }
                let min_intersection = self
                    .peer_info
                    .values()
                    .map(|info| info.intersection_slot)
                    .min()
                    .unwrap_or(0);
                Some(min_intersection)
            }
            GenesisSyncState::CaughtUp => None,
        }
    }

    // ── GDD ─────────────────────────────────────────────────────────────────

    /// Run the Genesis Density Disconnector (GDD).
    ///
    /// Implements the 4-guard `densityDisconnect` algorithm from the Haskell
    /// `ouroboros-consensus` specification. All density comparisons use integer
    /// arithmetic — no floating point.
    ///
    /// For each ordered peer pair `(peer0, peer1)`, disconnect `peer0` if ALL
    /// four guards pass:
    ///
    /// 1. **Has signal**: `peer0` is idling, has blocks in window, or has a
    ///    block at/after the window boundary. Peers with zero information are
    ///    given the benefit of the doubt.
    /// 2. **Chains disagree**: the last block recorded in each peer's window
    ///    differs, indicating they are on separate forks.
    /// 3. **Meaningful comparison**: `peer1` offers more than `k` blocks
    ///    (clearly dominant), OR `peer0` has no potential remaining slots
    ///    (its bounds are tight).
    /// 4. **Density dominance**: `peer1.lower_bound >= peer0.upper_bound`
    ///    (accounting for idling — idling peers use `lower_bound` as their
    ///    ceiling since they have stopped sending blocks).
    ///
    /// Returns addresses of peers that should be disconnected.
    pub fn gdd_evaluate(&self) -> Vec<SocketAddr> {
        if !self.enabled || self.state != GenesisSyncState::Syncing {
            return Vec::new();
        }

        if self.peer_info.len() < 2 {
            return Vec::new();
        }

        let k = self.config.security_param_k;
        let sgen = self.config.genesis_window_slots;

        // Pre-compute per-peer density metrics using integer arithmetic only.
        struct PeerMetrics {
            addr: SocketAddr,
            idling: bool,
            blocks_in_window: u64,
            has_block_after: bool,
            lower_bound: u64,
            upper_bound: u64,
            offers_more_than_k: bool,
            last_block_in_window: Option<u64>,
        }

        let loe_intersection = self
            .peer_info
            .values()
            .map(|info| info.intersection_slot)
            .min()
            .unwrap_or(0);

        let metrics: Vec<PeerMetrics> = self
            .peer_info
            .iter()
            .map(|(addr, info)| {
                // The first slot after the genesis window for this peer.
                let first_slot_after_window = loe_intersection + 1 + sgen;

                // Count blocks strictly before the window boundary.
                let blocks_in_window = info.density_window.blocks_before(first_slot_after_window);

                // Does this peer have evidence of blocks at or beyond the boundary?
                let has_block_after = info
                    .latest_slot
                    .is_some_and(|s| s >= first_slot_after_window)
                    || info
                        .density_window
                        .has_block_at_or_after(first_slot_after_window);

                // Potential remaining slots: if the peer has blocks after the window,
                // it has no "unknown" slots. Otherwise, count the gap between the
                // last observed block and the window boundary.
                let head_slot = info.density_window.head_slot();
                let potential_slots = if has_block_after {
                    0
                } else {
                    match head_slot {
                        Some(hs) => first_slot_after_window.saturating_sub(hs + 1),
                        None => first_slot_after_window.saturating_sub(loe_intersection + 1),
                    }
                };

                let lower_bound = blocks_in_window;
                let upper_bound = lower_bound + potential_slots;
                let offers_more_than_k = info.density_window.total_block_count() > k;
                let last_block_in_window = info.density_window.head_slot();

                PeerMetrics {
                    addr: *addr,
                    idling: info.idling,
                    blocks_in_window,
                    has_block_after,
                    lower_bound,
                    upper_bound,
                    offers_more_than_k,
                    last_block_in_window,
                }
            })
            .collect();

        let mut to_disconnect = Vec::new();

        // Pairwise comparison: for each (peer0, peer1), check if peer0 should
        // be disconnected based on peer1's superior density.
        for peer0 in &metrics {
            // Skip peers already flagged.
            if to_disconnect.contains(&peer0.addr) {
                continue;
            }

            for peer1 in &metrics {
                if peer0.addr == peer1.addr {
                    continue;
                }

                // Guard 1: peer0 must have some signal (not completely silent).
                // A peer with no signal gets the benefit of the doubt.
                let has_signal =
                    peer0.idling || peer0.blocks_in_window > 0 || peer0.has_block_after;
                if !has_signal {
                    continue;
                }

                // Guard 2: chains must disagree — if both peers have the same
                // last block in the window, they are on the same fork.
                if peer0.last_block_in_window == peer1.last_block_in_window {
                    continue;
                }

                // Guard 3: meaningful comparison — either peer1 clearly offers
                // more than k blocks, or peer0's bounds are tight (no uncertainty).
                let meaningful =
                    peer1.offers_more_than_k || (peer0.lower_bound == peer0.upper_bound);
                if !meaningful {
                    continue;
                }

                // Guard 4: density dominance — peer1's lower bound must be >=
                // peer0's effective ceiling. Idling peers have completed their
                // window, so their effective ceiling is their lower_bound.
                let peer0_ceiling = if peer0.idling {
                    peer0.lower_bound
                } else {
                    peer0.upper_bound
                };
                if peer1.lower_bound >= peer0_ceiling {
                    debug!(
                        peer0 = %peer0.addr,
                        peer1 = %peer1.addr,
                        peer0_lower = peer0.lower_bound,
                        peer0_upper = peer0.upper_bound,
                        peer0_ceiling,
                        peer1_lower = peer1.lower_bound,
                        peer0_idling = peer0.idling,
                        "GDD: disconnecting sparse peer"
                    );
                    to_disconnect.push(peer0.addr);
                    break; // peer0 is flagged, move to next peer0
                }
            }
        }

        if !to_disconnect.is_empty() {
            info!(
                disconnecting = to_disconnect.len(),
                total_peers = self.peer_info.len(),
                "GDD: disconnecting peers with insufficient chain density"
            );
        }

        to_disconnect
    }

    // ── Marker file helpers ──────────────────────────────────────────────────

    /// Write the caught_up marker file.
    fn write_marker(&self) {
        if let Err(e) = std::fs::write(&self.config.marker_path, "caught_up") {
            warn!(
                path = %self.config.marker_path.display(),
                "Failed to write caught_up marker: {e}"
            );
        }
    }

    /// Remove the caught_up marker file.
    fn remove_marker(&self) {
        if self.config.marker_path.exists() {
            if let Err(e) = std::fs::remove_file(&self.config.marker_path) {
                warn!(
                    path = %self.config.marker_path.display(),
                    "Failed to remove caught_up marker: {e}"
                );
            }
        }
    }

    /// Override the jitter value (for deterministic testing).
    #[cfg(test)]
    fn set_jitter(&mut self, jitter: u64) {
        self.anti_thundering_herd_jitter_secs = jitter;
    }
}

// ── GSM Actor ───────────────────────────────────────────────────────────────

/// Run the GSM actor as a background task.
///
/// Owns the `GenesisStateMachine` and processes events from the sync pipeline.
/// Publishes state snapshots via a `watch` channel and emits GDD disconnect
/// actions via an `mpsc` channel.
///
/// # Arguments
/// - `config`: GSM configuration
/// - `enabled`: whether genesis mode is active
/// - `event_rx`: incoming events from sync pipeline producers
/// - `snapshot_tx`: outgoing state snapshots (watch channel)
/// - `action_tx`: outgoing GDD disconnect actions
pub async fn run_gsm_actor(
    config: GsmConfig,
    enabled: bool,
    mut event_rx: mpsc::Receiver<GsmEvent>,
    snapshot_tx: watch::Sender<GsmSnapshot>,
    action_tx: mpsc::Sender<GddAction>,
) {
    let gdd_interval_ms = config.gdd_rate_limit_ms;
    let mut gsm = GenesisStateMachine::new(config, enabled);

    // Publish initial snapshot.
    let initial_snapshot = GsmSnapshot {
        state: gsm.state(),
        loe_slot: gsm.compute_loe_slot(),
    };
    let _ = snapshot_tx.send(initial_snapshot);

    let mut gdd_interval = tokio::time::interval(Duration::from_millis(gdd_interval_ms));
    // Don't compensate for missed ticks — just skip them.
    gdd_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            event = event_rx.recv() => {
                let Some(event) = event else {
                    // Channel closed — all producers dropped. Shut down.
                    info!("GSM actor: event channel closed, shutting down");
                    break;
                };

                let mut state_changed = false;

                match event {
                    GsmEvent::PeerRegistered { addr, intersection_slot, tip_slot } => {
                        gsm.register_peer(addr, intersection_slot, tip_slot);
                        state_changed = true; // LoE may change
                    }
                    GsmEvent::PeerDisconnected { addr } => {
                        gsm.deregister_peer(&addr);
                        state_changed = true; // LoE may change
                    }
                    GsmEvent::BlockReceived { addr, slot } => {
                        gsm.record_block(&addr, slot);
                    }
                    GsmEvent::PeerTipUpdated { addr, tip_slot } => {
                        gsm.update_peer_tip(&addr, tip_slot);
                    }
                    GsmEvent::PeerIdling { addr } => {
                        gsm.set_peer_idling(&addr, true);
                    }
                    GsmEvent::PeerActive { addr } => {
                        gsm.set_peer_idling(&addr, false);
                    }
                    GsmEvent::SyncStatus {
                        active_blp_count,
                        all_chainsync_idle,
                        tip_age_secs,
                        immutable_tip_slot,
                    } => {
                        if gsm.evaluate(active_blp_count, all_chainsync_idle, tip_age_secs, immutable_tip_slot).is_some() {
                            state_changed = true;
                        }
                    }
                }

                if state_changed {
                    let snapshot = GsmSnapshot {
                        state: gsm.state(),
                        loe_slot: gsm.compute_loe_slot(),
                    };
                    let _ = snapshot_tx.send(snapshot);
                }
            }

            _ = gdd_interval.tick() => {
                // Periodic GDD evaluation.
                let disconnects = gsm.gdd_evaluate();
                for addr in &disconnects {
                    if action_tx.send(GddAction::DisconnectPeer(*addr)).await.is_err() {
                        warn!("GSM actor: action channel closed, stopping GDD");
                        return;
                    }
                    // Also deregister the peer locally so LoE updates.
                    gsm.deregister_peer(addr);
                }

                if !disconnects.is_empty() {
                    let snapshot = GsmSnapshot {
                        state: gsm.state(),
                        loe_slot: gsm.compute_loe_slot(),
                    };
                    let _ = snapshot_tx.send(snapshot);
                }
            }
        }
    }
}

// ── Big ledger peer identification ──────────────────────────────────────────

/// Identify big ledger peers from the stake distribution.
///
/// Sorts pools by active stake descending and accumulates until 90 % of total
/// active stake is covered. Pools in the top 90 % are "big ledger peers" (BLPs).
///
/// Returns `(big_ledger_pool_ids, remaining_pool_ids)`.
#[allow(dead_code)] // future use: Genesis peer selection will use BLP classification
pub fn identify_big_ledger_peers(pool_stakes: &[(Vec<u8>, u64)]) -> (Vec<Vec<u8>>, Vec<Vec<u8>>) {
    if pool_stakes.is_empty() {
        return (Vec::new(), Vec::new());
    }

    let total_stake: u64 = pool_stakes.iter().map(|(_, s)| s).sum();
    let threshold = (total_stake as f64 * 0.9) as u64;

    let mut sorted: Vec<_> = pool_stakes.to_vec();
    sorted.sort_by(|a, b| b.1.cmp(&a.1)); // descending by stake

    let mut accumulated = 0u64;
    let mut big_ledger = Vec::new();
    let mut remaining = Vec::new();

    for (pool_id, stake) in sorted {
        if accumulated < threshold {
            accumulated += stake;
            big_ledger.push(pool_id);
        } else {
            remaining.push(pool_id);
        }
    }

    (big_ledger, remaining)
}

// ── Peer snapshot loader ────────────────────────────────────────────────────

/// Load peer snapshot from a JSON file.
///
/// Format: `[{"addr": "1.2.3.4", "port": 3001}, ...]`
#[allow(dead_code)] // future use: Genesis ledger peer snapshot loading
pub fn load_peer_snapshot(path: &std::path::Path) -> Result<Vec<SocketAddr>, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read peer snapshot file: {e}"))?;

    let entries: Vec<serde_json::Value> = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse peer snapshot JSON: {e}"))?;

    let mut peers = Vec::new();
    for entry in entries {
        if let (Some(addr), Some(port)) = (
            entry.get("addr").and_then(|v| v.as_str()),
            entry.get("port").and_then(|v| v.as_u64()),
        ) {
            if let Ok(socket_addr) = format!("{addr}:{port}").parse::<SocketAddr>() {
                peers.push(socket_addr);
            }
        }
    }

    Ok(peers)
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a GSM with test-friendly defaults.
    fn make_gsm(enabled: bool, marker_path: &str) -> GenesisStateMachine {
        let _ = std::fs::remove_file(marker_path);
        let config = GsmConfig {
            min_active_blp: 3,
            max_caught_up_age_secs: 600,
            min_caught_up_dwell_secs: 0, // no dwell for most tests
            anti_thundering_herd_max_secs: 0,
            genesis_window_slots: 1_000,
            gdd_rate_limit_ms: 100,
            security_param_k: 2160,
            marker_path: PathBuf::from(marker_path),
        };
        let mut gsm = GenesisStateMachine::new(config, enabled);
        gsm.set_jitter(0); // deterministic
        gsm
    }

    /// Helper: create a GSM with specific k and window size for GDD tests.
    fn make_gdd_gsm(k: u64, window: u64, marker_path: &str) -> GenesisStateMachine {
        let _ = std::fs::remove_file(marker_path);
        let config = GsmConfig {
            min_active_blp: 1,
            max_caught_up_age_secs: 600,
            min_caught_up_dwell_secs: 0,
            anti_thundering_herd_max_secs: 0,
            genesis_window_slots: window,
            gdd_rate_limit_ms: 100,
            security_param_k: k,
            marker_path: PathBuf::from(marker_path),
        };
        let mut gsm = GenesisStateMachine::new(config, true);
        gsm.set_jitter(0);
        // Move to Syncing state for GDD tests
        gsm.evaluate(5, false, 0, 0);
        assert_eq!(gsm.state(), GenesisSyncState::Syncing);
        gsm
    }

    // ── GDD Guard 1: peer with no signal not disconnected ───────────────────

    #[test]
    fn test_gdd_guard1_no_signal_not_disconnected() {
        // A peer with no blocks, not idling, and no blocks after the window
        // has no "signal" and should NOT be disconnected, even if another peer
        // has higher density.
        let mut gsm = make_gdd_gsm(10, 1_000, "/tmp/test_gdd_guard1_marker");

        let addr_dense: SocketAddr = "1.2.3.4:3001".parse().unwrap();
        let addr_silent: SocketAddr = "5.6.7.8:3001".parse().unwrap();

        gsm.register_peer(addr_dense, 0, 5000);
        gsm.register_peer(addr_silent, 0, 5000);

        // Dense peer: fill window with blocks
        for slot in 1..=500u64 {
            gsm.record_block(&addr_dense, slot);
        }
        // Silent peer: no blocks at all, not idling

        let to_disconnect = gsm.gdd_evaluate();
        assert!(
            !to_disconnect.contains(&addr_silent),
            "Silent peer (no signal) must NOT be disconnected"
        );
    }

    // ── GDD Guard 2: same-chain peers not disconnected ──────────────────────

    #[test]
    fn test_gdd_guard2_same_chain_not_disconnected() {
        // Two peers with identical block histories should NOT disconnect
        // each other, even if one is idling.
        let mut gsm = make_gdd_gsm(10, 1_000, "/tmp/test_gdd_guard2_marker");

        let addr_a: SocketAddr = "1.2.3.4:3001".parse().unwrap();
        let addr_b: SocketAddr = "5.6.7.8:3001".parse().unwrap();

        gsm.register_peer(addr_a, 0, 5000);
        gsm.register_peer(addr_b, 0, 5000);

        // Both peers have exactly the same blocks
        for slot in [100u64, 200, 300, 400, 500] {
            gsm.record_block(&addr_a, slot);
            gsm.record_block(&addr_b, slot);
        }
        // Make one peer idling
        gsm.set_peer_idling(&addr_a, true);

        let to_disconnect = gsm.gdd_evaluate();
        assert!(
            to_disconnect.is_empty(),
            "Same-chain peers must NOT be disconnected (guard 2)"
        );
    }

    // ── GDD Guard 3: small fork protection ──────────────────────────────────

    #[test]
    fn test_gdd_guard3_small_fork_protection() {
        // When peer1 doesn't offer more than k blocks AND peer0's bounds are
        // not tight (lower != upper), guard 3 blocks the disconnection.
        // This protects peers on small forks where comparison is not meaningful.
        let mut gsm = make_gdd_gsm(2160, 1_000, "/tmp/test_gdd_guard3_marker");

        let addr_sparse: SocketAddr = "1.2.3.4:3001".parse().unwrap();
        let addr_medium: SocketAddr = "5.6.7.8:3001".parse().unwrap();

        gsm.register_peer(addr_sparse, 0, 5000);
        gsm.register_peer(addr_medium, 0, 5000);

        // Sparse peer: a few blocks (different from medium), idling
        gsm.record_block(&addr_sparse, 10);
        gsm.record_block(&addr_sparse, 20);
        gsm.set_peer_idling(&addr_sparse, true);

        // Medium peer: more blocks but NOT more than k (2160)
        for slot in (100..=500).step_by(100) {
            gsm.record_block(&addr_medium, slot);
        }

        let to_disconnect = gsm.gdd_evaluate();
        assert!(
            !to_disconnect.contains(&addr_sparse),
            "Guard 3 should block disconnection when comparison is not meaningful"
        );
    }

    // ── GDD Guard 4 idling: idling peer dominated → disconnected ────────────

    #[test]
    fn test_gdd_guard4_idling_peer_dominated() {
        // An idling peer with low density should be disconnected when another
        // peer clearly dominates with more than k blocks. Idling peers use
        // lower_bound as their ceiling (they've stopped sending).
        let mut gsm = make_gdd_gsm(10, 1_000, "/tmp/test_gdd_guard4_idle_marker");

        let addr_idling: SocketAddr = "1.2.3.4:3001".parse().unwrap();
        let addr_dense: SocketAddr = "5.6.7.8:3001".parse().unwrap();

        gsm.register_peer(addr_idling, 0, 5000);
        gsm.register_peer(addr_dense, 0, 5000);

        // Idling peer: sparse chain, different blocks from dense peer
        gsm.record_block(&addr_idling, 5);
        gsm.record_block(&addr_idling, 15);
        gsm.set_peer_idling(&addr_idling, true);

        // Dense peer: fills window with many blocks (> k = 10)
        for slot in 1..=500u64 {
            gsm.record_block(&addr_dense, slot);
        }

        let to_disconnect = gsm.gdd_evaluate();
        assert!(
            to_disconnect.contains(&addr_idling),
            "Idling sparse peer should be disconnected when dominated"
        );
        assert!(
            !to_disconnect.contains(&addr_dense),
            "Dense peer should NOT be disconnected"
        );
    }

    // ── GDD Guard 4 non-idling: benefit of the doubt (upper_bound) ──────────

    #[test]
    fn test_gdd_guard4_non_idling_benefit_of_doubt() {
        // A non-idling peer gets the benefit of the doubt — its upper_bound
        // (which includes potential_slots) is used as the ceiling, making it
        // harder to disconnect.
        let mut gsm = make_gdd_gsm(10, 1_000, "/tmp/test_gdd_guard4_nonidl_marker");

        let addr_slow: SocketAddr = "1.2.3.4:3001".parse().unwrap();
        let addr_dense: SocketAddr = "5.6.7.8:3001".parse().unwrap();

        gsm.register_peer(addr_slow, 0, 5000);
        gsm.register_peer(addr_dense, 0, 5000);

        // Slow peer: only a few blocks, NOT idling (still sending).
        // Its upper_bound should include potential_slots for unobserved suffix,
        // making it high enough to avoid disconnection.
        gsm.record_block(&addr_slow, 10);
        gsm.record_block(&addr_slow, 20);
        // NOT idling — upper_bound = lower_bound + potential_slots = 2 + (1001 - 21) = 2 + 980 = 982

        // Dense peer: many blocks (> k = 10)
        for slot in 100..=500u64 {
            gsm.record_block(&addr_dense, slot);
        }
        // Dense peer lower_bound = 401, which is < slow peer upper_bound (982)
        // So guard 4 should NOT pass for the slow peer.

        let to_disconnect = gsm.gdd_evaluate();
        assert!(
            !to_disconnect.contains(&addr_slow),
            "Non-idling peer should get benefit of the doubt (upper_bound includes potential)"
        );
    }

    // ── State: PreSyncing → Syncing on HAA ──────────────────────────────────

    #[test]
    fn test_state_presyncing_to_syncing() {
        let mut gsm = make_gsm(true, "/tmp/test_state_pre_sync_marker");
        assert_eq!(gsm.state(), GenesisSyncState::PreSyncing);

        // Not enough BLPs
        assert_eq!(gsm.evaluate(2, false, 0, 0), None);
        assert_eq!(gsm.state(), GenesisSyncState::PreSyncing);

        // Enough BLPs
        let result = gsm.evaluate(3, false, 0, 0);
        assert_eq!(result, Some(GenesisSyncState::Syncing));
        assert_eq!(gsm.state(), GenesisSyncState::Syncing);
    }

    // ── State: Syncing → PreSyncing on HAA loss ─────────────────────────────

    #[test]
    fn test_state_syncing_to_presyncing_haa_loss() {
        let mut gsm = make_gsm(true, "/tmp/test_state_haa_loss_marker");
        assert_eq!(gsm.state(), GenesisSyncState::PreSyncing);

        // Get to Syncing
        gsm.evaluate(5, false, 0, 0);
        assert_eq!(gsm.state(), GenesisSyncState::Syncing);

        // Drop below minimum BLPs → regress to PreSyncing
        let result = gsm.evaluate(2, false, 0, 0);
        assert_eq!(result, Some(GenesisSyncState::PreSyncing));
        assert_eq!(gsm.state(), GenesisSyncState::PreSyncing);
    }

    // ── State: Syncing → CaughtUp ───────────────────────────────────────────

    #[test]
    fn test_state_syncing_to_caught_up() {
        let marker = PathBuf::from("/tmp/test_state_sync_caught_marker");
        let _ = std::fs::remove_file(&marker);
        let config = GsmConfig {
            min_active_blp: 1,
            max_caught_up_age_secs: 600,
            min_caught_up_dwell_secs: 0,
            anti_thundering_herd_max_secs: 0,
            genesis_window_slots: 1_000,
            gdd_rate_limit_ms: 100,
            security_param_k: 2160,
            marker_path: marker.clone(),
        };
        let mut gsm = GenesisStateMachine::new(config, true);
        gsm.set_jitter(0);

        gsm.evaluate(5, false, 0, 0); // → Syncing
        assert_eq!(gsm.state(), GenesisSyncState::Syncing);

        // Not idle yet
        assert_eq!(gsm.evaluate(5, false, 100, 0), None);

        // Idle but tip too old
        assert_eq!(gsm.evaluate(5, true, 700, 0), None);

        // Idle and tip fresh
        let result = gsm.evaluate(5, true, 100, 0);
        assert_eq!(result, Some(GenesisSyncState::CaughtUp));
        assert_eq!(gsm.state(), GenesisSyncState::CaughtUp);

        assert!(marker.exists());
        let _ = std::fs::remove_file(&marker);
    }

    // ── State: CaughtUp no regression during dwell ──────────────────────────

    #[test]
    fn test_state_caught_up_no_regression_during_dwell() {
        let marker = PathBuf::from("/tmp/test_state_dwell_marker");
        let _ = std::fs::remove_file(&marker);
        let config = GsmConfig {
            min_active_blp: 1,
            max_caught_up_age_secs: 600,
            min_caught_up_dwell_secs: 3600, // 1 hour dwell
            anti_thundering_herd_max_secs: 0,
            genesis_window_slots: 1_000,
            gdd_rate_limit_ms: 100,
            security_param_k: 2160,
            marker_path: marker.clone(),
        };
        let mut gsm = GenesisStateMachine::new(config, true);
        gsm.set_jitter(0);

        // Get to CaughtUp
        gsm.evaluate(5, false, 0, 0); // → Syncing
        gsm.evaluate(5, true, 100, 0); // → CaughtUp
        assert_eq!(gsm.state(), GenesisSyncState::CaughtUp);

        // Tip goes stale — but dwell period has not elapsed (just entered CaughtUp)
        let result = gsm.evaluate(5, false, 1300, 0);
        assert_eq!(result, None, "Should NOT regress during dwell period");
        assert_eq!(gsm.state(), GenesisSyncState::CaughtUp);

        let _ = std::fs::remove_file(&marker);
    }

    // ── LoE: PreSyncing freezes (Some(0)) ───────────────────────────────────

    #[test]
    fn test_loe_presyncing_freezes() {
        let gsm = make_gsm(true, "/tmp/test_loe_pre_freeze_marker");
        assert_eq!(gsm.state(), GenesisSyncState::PreSyncing);
        assert_eq!(gsm.compute_loe_slot(), Some(0));
    }

    // ── LoE: Syncing uses min intersection ──────────────────────────────────

    #[test]
    fn test_loe_syncing_min_intersection() {
        let mut gsm = make_gsm(true, "/tmp/test_loe_sync_inter_marker");
        gsm.evaluate(5, false, 0, 0); // → Syncing
        assert_eq!(gsm.state(), GenesisSyncState::Syncing);

        let addr_a: SocketAddr = "1.2.3.4:3001".parse().unwrap();
        let addr_b: SocketAddr = "5.6.7.8:3001".parse().unwrap();

        gsm.register_peer(addr_a, 500, 2000);
        gsm.register_peer(addr_b, 800, 2000);

        // Min intersection is 500
        assert_eq!(gsm.compute_loe_slot(), Some(500));
    }

    // ── LoE: CaughtUp no constraint (None) ──────────────────────────────────

    #[test]
    fn test_loe_caught_up_no_constraint() {
        let marker = PathBuf::from("/tmp/test_loe_caught_none_marker");
        std::fs::write(&marker, "caught_up").unwrap();
        let config = GsmConfig {
            marker_path: marker.clone(),
            ..Default::default()
        };
        let gsm = GenesisStateMachine::new(config, true);
        assert_eq!(gsm.state(), GenesisSyncState::CaughtUp);
        assert_eq!(gsm.compute_loe_slot(), None);
        let _ = std::fs::remove_file(&marker);
    }

    // ── LoE: advances after peer deregister ─────────────────────────────────

    #[test]
    fn test_loe_advances_after_deregister() {
        let mut gsm = make_gsm(true, "/tmp/test_loe_advance_marker");
        gsm.evaluate(5, false, 0, 0); // → Syncing

        let addr_low: SocketAddr = "1.2.3.4:3001".parse().unwrap();
        let addr_high: SocketAddr = "5.6.7.8:3001".parse().unwrap();

        gsm.register_peer(addr_low, 100, 2000);
        gsm.register_peer(addr_high, 500, 2000);

        // LoE is capped at the low peer's intersection
        assert_eq!(gsm.compute_loe_slot(), Some(100));

        // Deregister the low peer → LoE advances to 500
        gsm.deregister_peer(&addr_low);
        assert_eq!(gsm.compute_loe_slot(), Some(500));
    }

    // ── Actor: state transitions via events ─────────────────────────────────

    #[tokio::test]
    async fn test_actor_state_transitions() {
        let config = GsmConfig {
            min_active_blp: 2,
            max_caught_up_age_secs: 600,
            min_caught_up_dwell_secs: 0,
            anti_thundering_herd_max_secs: 0,
            genesis_window_slots: 1_000,
            gdd_rate_limit_ms: 10_000, // long interval so GDD tick doesn't interfere
            security_param_k: 2160,
            marker_path: PathBuf::from("/tmp/test_actor_transitions_marker"),
        };
        let _ = std::fs::remove_file(&config.marker_path);

        let (event_tx, event_rx) = mpsc::channel(64);
        let (snapshot_tx, mut snapshot_rx) = watch::channel(GsmSnapshot {
            state: GenesisSyncState::PreSyncing,
            loe_slot: Some(0),
        });
        let (action_tx, _action_rx) = mpsc::channel(64);

        // Spawn the actor
        let handle = tokio::spawn(run_gsm_actor(
            config,
            true,
            event_rx,
            snapshot_tx,
            action_tx,
        ));

        // Wait for initial snapshot to be published
        snapshot_rx.changed().await.unwrap();
        let snap = *snapshot_rx.borrow();
        assert_eq!(snap.state, GenesisSyncState::PreSyncing);
        assert_eq!(snap.loe_slot, Some(0));

        // Send SyncStatus with enough BLPs → should transition to Syncing
        event_tx
            .send(GsmEvent::SyncStatus {
                active_blp_count: 5,
                all_chainsync_idle: false,
                tip_age_secs: 0,
                immutable_tip_slot: 0,
            })
            .await
            .unwrap();

        snapshot_rx.changed().await.unwrap();
        let snap = *snapshot_rx.borrow();
        assert_eq!(snap.state, GenesisSyncState::Syncing);

        // Send SyncStatus that meets CaughtUp criteria
        event_tx
            .send(GsmEvent::SyncStatus {
                active_blp_count: 5,
                all_chainsync_idle: true,
                tip_age_secs: 100,
                immutable_tip_slot: 0,
            })
            .await
            .unwrap();

        snapshot_rx.changed().await.unwrap();
        let snap = *snapshot_rx.borrow();
        assert_eq!(snap.state, GenesisSyncState::CaughtUp);
        assert_eq!(snap.loe_slot, None);

        // Shut down actor
        drop(event_tx);
        let _ = handle.await;
        let _ = std::fs::remove_file("/tmp/test_actor_transitions_marker");
    }

    // ── Actor: GDD disconnects sparse peer ──────────────────────────────────

    #[tokio::test]
    async fn test_actor_gdd_disconnects_sparse_peer() {
        let config = GsmConfig {
            min_active_blp: 1,
            max_caught_up_age_secs: 600,
            min_caught_up_dwell_secs: 0,
            anti_thundering_herd_max_secs: 0,
            genesis_window_slots: 1_000,
            gdd_rate_limit_ms: 50, // fast GDD ticks for test
            security_param_k: 10,  // low k so guard 3 is easy to satisfy
            marker_path: PathBuf::from("/tmp/test_actor_gdd_marker"),
        };
        let _ = std::fs::remove_file(&config.marker_path);

        let (event_tx, event_rx) = mpsc::channel(256);
        let (snapshot_tx, mut snapshot_rx) = watch::channel(GsmSnapshot {
            state: GenesisSyncState::PreSyncing,
            loe_slot: Some(0),
        });
        let (action_tx, mut action_rx) = mpsc::channel(64);

        let handle = tokio::spawn(run_gsm_actor(
            config,
            true,
            event_rx,
            snapshot_tx,
            action_tx,
        ));

        // Wait for initial snapshot
        snapshot_rx.changed().await.unwrap();

        // Transition to Syncing
        event_tx
            .send(GsmEvent::SyncStatus {
                active_blp_count: 5,
                all_chainsync_idle: false,
                tip_age_secs: 0,
                immutable_tip_slot: 0,
            })
            .await
            .unwrap();
        snapshot_rx.changed().await.unwrap();

        let addr_dense: SocketAddr = "1.2.3.4:3001".parse().unwrap();
        let addr_sparse: SocketAddr = "5.6.7.8:3001".parse().unwrap();

        // Register peers
        event_tx
            .send(GsmEvent::PeerRegistered {
                addr: addr_dense,
                intersection_slot: 0,
                tip_slot: 5000,
            })
            .await
            .unwrap();
        event_tx
            .send(GsmEvent::PeerRegistered {
                addr: addr_sparse,
                intersection_slot: 0,
                tip_slot: 5000,
            })
            .await
            .unwrap();

        // Feed blocks to dense peer (> k = 10 blocks for guard 3)
        for slot in 1..=500u64 {
            event_tx
                .send(GsmEvent::BlockReceived {
                    addr: addr_dense,
                    slot,
                })
                .await
                .unwrap();
        }

        // Sparse peer: just a few blocks on a different chain, then idling
        event_tx
            .send(GsmEvent::BlockReceived {
                addr: addr_sparse,
                slot: 5,
            })
            .await
            .unwrap();
        event_tx
            .send(GsmEvent::BlockReceived {
                addr: addr_sparse,
                slot: 15,
            })
            .await
            .unwrap();
        event_tx
            .send(GsmEvent::PeerIdling { addr: addr_sparse })
            .await
            .unwrap();

        // Wait for a GDD tick to fire and produce a disconnect action
        let action = tokio::time::timeout(Duration::from_secs(2), action_rx.recv())
            .await
            .expect("timeout waiting for GDD action")
            .expect("action channel closed");

        match action {
            GddAction::DisconnectPeer(addr) => {
                assert_eq!(addr, addr_sparse, "GDD should disconnect the sparse peer");
            }
        }

        // Shut down
        drop(event_tx);
        let _ = handle.await;
        let _ = std::fs::remove_file("/tmp/test_actor_gdd_marker");
    }

    // ── Existing tests adapted to new API ───────────────────────────────────

    #[test]
    fn test_gsm_disabled_stays_caught_up() {
        let mut gsm = make_gsm(false, "/tmp/test_gsm_disabled_marker");
        assert_eq!(gsm.state(), GenesisSyncState::CaughtUp);
        let result = gsm.evaluate(0, false, 9999, 0);
        assert_eq!(result, None);
        assert_eq!(gsm.state(), GenesisSyncState::CaughtUp);
    }

    #[test]
    fn test_gsm_marker_fast_restart() {
        let marker_path = PathBuf::from("/tmp/test_gsm_fast_restart_marker");
        std::fs::write(&marker_path, "caught_up").unwrap();
        let config = GsmConfig {
            marker_path: marker_path.clone(),
            ..Default::default()
        };
        let gsm = GenesisStateMachine::new(config, true);
        assert_eq!(gsm.state(), GenesisSyncState::CaughtUp);
        let _ = std::fs::remove_file(&marker_path);
    }

    #[test]
    fn test_register_deregister_peer() {
        let mut gsm = make_gsm(true, "/tmp/test_register_marker2");
        gsm.evaluate(5, false, 0, 0); // → Syncing

        let addr: SocketAddr = "1.2.3.4:3001".parse().unwrap();
        assert_eq!(gsm.peer_info().len(), 0);

        gsm.register_peer(addr, 500, 1500);
        assert_eq!(gsm.peer_info().len(), 1);
        assert_eq!(gsm.peer_info()[&addr].intersection_slot, 500);

        gsm.deregister_peer(&addr);
        assert_eq!(gsm.peer_info().len(), 0);
    }

    #[test]
    fn test_record_block_updates_density() {
        let mut gsm = make_gsm(true, "/tmp/test_record_density_marker2");
        gsm.evaluate(5, false, 0, 0); // → Syncing

        let addr: SocketAddr = "1.2.3.4:3001".parse().unwrap();
        gsm.register_peer(addr, 0, 2000);

        assert_eq!(gsm.peer_info()[&addr].blocks_in_window(), 0);
        gsm.record_block(&addr, 100);
        gsm.record_block(&addr, 200);
        gsm.record_block(&addr, 300);
        assert_eq!(gsm.peer_info()[&addr].blocks_in_window(), 3);

        // Blocks outside window should not count (window_size=1000, intersection=0)
        gsm.record_block(&addr, 1001); // just outside
        assert_eq!(gsm.peer_info()[&addr].blocks_in_window(), 3);

        // But latest_slot should still be updated
        assert_eq!(gsm.peer_info()[&addr].latest_slot, Some(1001));
    }

    #[test]
    fn test_gdd_no_disconnect_when_caught_up() {
        let marker = PathBuf::from("/tmp/test_gdd_disabled_marker2");
        std::fs::write(&marker, "caught_up").unwrap();
        let config = GsmConfig {
            genesis_window_slots: 1_000,
            marker_path: marker.clone(),
            ..Default::default()
        };
        let mut gsm = GenesisStateMachine::new(config, true);
        assert_eq!(gsm.state(), GenesisSyncState::CaughtUp);

        let addr_a: SocketAddr = "1.2.3.4:3001".parse().unwrap();
        let addr_b: SocketAddr = "5.6.7.8:3001".parse().unwrap();
        gsm.register_peer(addr_a, 0, 1000);
        gsm.register_peer(addr_b, 0, 1000);

        for slot in 1..=900u64 {
            gsm.record_block(&addr_a, slot);
        }
        let to_disconnect = gsm.gdd_evaluate();
        assert!(to_disconnect.is_empty(), "GDD inactive in CaughtUp");
        let _ = std::fs::remove_file(&marker);
    }

    #[test]
    fn test_gdd_single_peer_never_disconnects() {
        let mut gsm = make_gdd_gsm(10, 1_000, "/tmp/test_gdd_single_marker2");

        let addr: SocketAddr = "1.2.3.4:3001".parse().unwrap();
        gsm.register_peer(addr, 0, 1000);
        let to_disconnect = gsm.gdd_evaluate();
        assert!(
            to_disconnect.is_empty(),
            "Single peer must not be disconnected"
        );
    }

    // ── Big ledger peer identification ───────────────────────────────────────

    #[test]
    fn test_identify_big_ledger_peers() {
        let pools = vec![
            (vec![1], 1000), // 50%
            (vec![2], 500),  // 25%
            (vec![3], 300),  // 15%
            (vec![4], 100),  // 5%
            (vec![5], 100),  // 5%
        ];

        let (big, remaining) = identify_big_ledger_peers(&pools);
        assert!(big.len() >= 2, "Should have at least 2 big ledger peers");
        assert!(!remaining.is_empty(), "Should have remaining small pools");
    }

    #[test]
    fn test_identify_big_ledger_peers_empty() {
        let (big, remaining) = identify_big_ledger_peers(&[]);
        assert!(big.is_empty());
        assert!(remaining.is_empty());
    }

    // ── Peer snapshot loader ─────────────────────────────────────────────────

    #[test]
    fn test_load_peer_snapshot() {
        let dir = std::env::temp_dir();
        let path = dir.join("test_peer_snapshot.json");
        std::fs::write(
            &path,
            r#"[{"addr": "1.2.3.4", "port": 3001}, {"addr": "5.6.7.8", "port": 3002}]"#,
        )
        .unwrap();

        let peers = load_peer_snapshot(&path).unwrap();
        assert_eq!(peers.len(), 2);
        assert_eq!(peers[0].to_string(), "1.2.3.4:3001");
        assert_eq!(peers[1].to_string(), "5.6.7.8:3002");

        let _ = std::fs::remove_file(&path);
    }
}
