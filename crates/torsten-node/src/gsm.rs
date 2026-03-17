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
//! `loe_limit()` is wired into `process_forward_blocks()` in `node.rs`.
//! When the GSM is in PreSyncing or Syncing state, `flush_to_immutable_loe()`
//! is called with the peer tip slot as the ceiling so that the immutable tip
//! cannot advance past the common prefix of all candidate chains.  When the
//! GSM reaches CaughtUp, `loe_limit()` returns `None` and the normal
//! unconstrained `flush_to_immutable()` is used instead.
//!
//! ## GDD (Genesis Density Disconnector)
//!
//! During Syncing state the GSM maintains a per-peer `DensityWindow` tracking
//! how many blocks each peer's chain contains within the genesis window
//! `(intersection_slot, intersection_slot + 3k/f]`.  On each GDD evaluation
//! any peer whose density upper-bound is dominated by another peer's
//! density lower-bound is flagged for disconnection.
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

use torsten_consensus::DensityWindow;
use torsten_primitives::block::Point;
use tracing::{debug, info, warn};

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

/// Configuration for the Genesis State Machine.
#[derive(Debug, Clone)]
pub struct GsmConfig {
    /// Minimum active big ledger peers to transition PreSyncing → Syncing
    pub min_active_blp: usize,
    /// Maximum tip age (seconds) before CaughtUp → PreSyncing regression
    pub max_tip_age_secs: u64,
    /// Genesis window size in slots (`3k/f`).
    ///
    /// The GDD compares block density across peers within this window,
    /// anchored at the intersection point.  Defaults to 129_600 slots
    /// (~36 hours at 1-second slot intervals, matching mainnet `k=2160, f=0.05`).
    pub genesis_window_slots: u64,
    /// Path for the caught_up marker file
    pub marker_path: PathBuf,
}

impl Default for GsmConfig {
    fn default() -> Self {
        GsmConfig {
            min_active_blp: 5,
            max_tip_age_secs: 1200, // 20 minutes
            // 3 * 2160 / 0.05 = 129_600 slots (mainnet default)
            genesis_window_slots: 129_600,
            marker_path: PathBuf::from("caught_up.marker"),
        }
    }
}

// ── Per-peer chain density tracking ─────────────────────────────────────────

/// Live density information tracked per peer for GDD comparison.
///
/// The GDD requires knowing how many blocks a peer's chain places within the
/// genesis window `(I, I + s]` where `I` is the fork intersection slot.  This
/// struct combines the sliding `DensityWindow` (which records block slots) with
/// the peer's reported tip slot for staleness detection.
#[derive(Debug, Clone)]
pub struct PeerChainInfo {
    /// Density window tracking blocks in the genesis window for this peer.
    pub density_window: DensityWindow,
    /// Most recent tip slot reported by this peer.
    pub tip_slot: u64,
}

impl PeerChainInfo {
    /// Create a new info record with a fresh density window.
    pub fn new(intersection_slot: u64, window_size: u64, tip_slot: u64) -> Self {
        PeerChainInfo {
            density_window: DensityWindow::new(intersection_slot, window_size),
            tip_slot,
        }
    }

    /// Number of blocks this peer has within the genesis window.
    pub fn blocks_in_window(&self) -> u64 {
        self.density_window.block_count()
    }

    /// Record a block arriving at `slot` from this peer.
    pub fn record_block(&mut self, slot: u64) {
        self.density_window.record_block(slot);
    }

    /// Update the peer's tip slot (called when a new header is received).
    pub fn update_tip(&mut self, slot: u64) {
        if slot > self.tip_slot {
            self.tip_slot = slot;
        }
    }
}

// ── GenesisStateMachine ──────────────────────────────────────────────────────

/// The Genesis State Machine.
///
/// Tracks sync state and enforces transitions based on peer availability
/// and tip freshness. The GSM also manages:
/// - **LoE (Limit on Eagerness)**: constrains block application during sync
/// - **GDD (Genesis Density Disconnector)**: disconnects sparse-chain peers
pub struct GenesisStateMachine {
    config: GsmConfig,
    state: GenesisSyncState,
    /// Whether genesis mode is enabled (opt-in via --consensus-mode genesis)
    enabled: bool,
    /// Per-peer chain density information, keyed by peer socket address.
    ///
    /// Only populated when `enabled` and in `Syncing` state. Each entry
    /// holds the peer's `DensityWindow` anchored at the current intersection
    /// slot. Entries are inserted on first contact and removed on disconnect.
    peer_info: HashMap<SocketAddr, PeerChainInfo>,
    /// The intersection slot shared by all candidate chains.
    ///
    /// Set when the sync loop resolves `find_intersect()`. All density
    /// windows are anchored at this slot.
    pub intersection_slot: u64,
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

        GenesisStateMachine {
            config,
            state: initial_state,
            enabled,
            peer_info: HashMap::new(),
            intersection_slot: 0,
        }
    }

    /// Current sync state
    pub fn state(&self) -> GenesisSyncState {
        self.state
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
        self.intersection_slot = intersection_slot;
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

    /// Read-only view of the peer density map (for metrics / tests).
    #[allow(dead_code)] // exposed for diagnostics and test introspection
    pub fn peer_info(&self) -> &HashMap<SocketAddr, PeerChainInfo> {
        &self.peer_info
    }

    // ── State transitions ────────────────────────────────────────────────────

    /// Evaluate state transitions based on current conditions.
    ///
    /// Returns `Some(new_state)` if a transition occurred, `None` if unchanged.
    pub fn evaluate(
        &mut self,
        active_blp_count: usize,
        all_chainsync_idle: bool,
        tip_age_secs: u64,
    ) -> Option<GenesisSyncState> {
        if !self.enabled {
            return None;
        }

        let old_state = self.state;

        match self.state {
            GenesisSyncState::PreSyncing => {
                // Transition to Syncing when we have enough active BLPs (HAA satisfied)
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
                // Transition to CaughtUp when all ChainSync clients are idle
                // and the tip is fresh enough.
                if all_chainsync_idle && tip_age_secs < self.config.max_tip_age_secs {
                    self.state = GenesisSyncState::CaughtUp;
                    self.write_marker();
                    info!(
                        tip_age_secs,
                        "Genesis: all peers idle and tip fresh, transitioning to CaughtUp"
                    );
                }
            }
            GenesisSyncState::CaughtUp => {
                // Regress to PreSyncing if tip becomes stale
                if tip_age_secs > self.config.max_tip_age_secs {
                    self.state = GenesisSyncState::PreSyncing;
                    self.remove_marker();
                    warn!(
                        tip_age_secs,
                        max = self.config.max_tip_age_secs,
                        "Genesis: tip stale, regressing to PreSyncing"
                    );
                }
            }
        }

        if self.state != old_state {
            Some(self.state)
        } else {
            None
        }
    }

    // ── LoE ─────────────────────────────────────────────────────────────────

    /// Check the Limit on Eagerness constraint.
    ///
    /// Returns the maximum immutable tip slot that block application should
    /// advance to. Returns `None` if there is no constraint (CaughtUp state).
    pub fn loe_limit(&self, candidate_tips: &[Point]) -> Option<u64> {
        if !self.enabled {
            return None; // No constraint
        }

        match self.state {
            GenesisSyncState::PreSyncing => {
                // Don't apply any blocks — anchor at genesis
                Some(0)
            }
            GenesisSyncState::Syncing => {
                // Don't advance immutable tip past common prefix of all candidate chains
                if candidate_tips.is_empty() {
                    return Some(0);
                }
                // Common prefix = minimum tip slot across all candidates
                let min_slot = candidate_tips
                    .iter()
                    .filter_map(|p| p.slot())
                    .map(|s| s.0)
                    .min()
                    .unwrap_or(0);
                Some(min_slot)
            }
            GenesisSyncState::CaughtUp => {
                None // No constraint
            }
        }
    }

    // ── GDD ─────────────────────────────────────────────────────────────────

    /// Run the Genesis Density Disconnector (GDD).
    ///
    /// During Syncing state, compares chain density across all known peers
    /// within the genesis window. A peer is flagged for disconnection when its
    /// density *upper bound* is dominated by another peer's density *lower bound*.
    ///
    /// ## Density bounds
    ///
    /// The GDD uses conservative bounds to avoid false positives from network
    /// jitter:
    ///
    /// - **Lower bound**: `blocks_in_window / window_size` — minimum guaranteed
    ///   density based on blocks actually observed.
    /// - **Upper bound**: `(blocks_in_window + slack) / window_size` where
    ///   `slack = blocks_in_window * 0.10 + 1` — allows 10 % extra for blocks
    ///   still in flight plus one block of absolute tolerance.
    ///
    /// A peer is disconnected when `upper_bound(peer) <= max_lower_bound` where
    /// `max_lower_bound` is the highest lower bound across all peers. This means
    /// the peer's best-case density still cannot beat the best-observed peer.
    ///
    /// Returns addresses of peers that should be disconnected.
    pub fn gdd_evaluate(&self) -> Vec<SocketAddr> {
        if !self.enabled || self.state != GenesisSyncState::Syncing {
            return Vec::new();
        }

        if self.peer_info.len() < 2 {
            return Vec::new();
        }

        let window = self.config.genesis_window_slots as f64;

        // Compute density bounds per peer.
        let densities: Vec<(SocketAddr, f64, f64)> = self
            .peer_info
            .iter()
            .map(|(addr, info)| {
                let blocks = info.blocks_in_window() as f64;
                // Lower bound: confirmed blocks / full window
                let lower = blocks / window;
                // Upper bound: allow 10 % slack for in-flight blocks, plus one
                // absolute block of tolerance.
                let slack = blocks * 0.10 + 1.0;
                let upper = (blocks + slack) / window;
                (*addr, lower, upper)
            })
            .collect();

        // Find the best-observed density lower bound.
        let max_lower = densities
            .iter()
            .map(|(_, lower, _)| *lower)
            .fold(f64::NEG_INFINITY, f64::max);

        // Disconnect peers whose upper bound is ≤ the best lower bound.
        let mut to_disconnect = Vec::new();
        for (addr, _lower, upper) in &densities {
            if *upper <= max_lower {
                debug!(
                    %addr,
                    upper,
                    max_lower,
                    blocks = self.peer_info.get(addr).map(|i| i.blocks_in_window()).unwrap_or(0),
                    "GDD: disconnecting sparse peer"
                );
                to_disconnect.push(*addr);
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

    /// Write the caught_up marker file
    fn write_marker(&self) {
        if let Err(e) = std::fs::write(&self.config.marker_path, "caught_up") {
            warn!(
                path = %self.config.marker_path.display(),
                "Failed to write caught_up marker: {e}"
            );
        }
    }

    /// Remove the caught_up marker file
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

// ── Peer snapshot loader ─────────────────────────────────────────────────────

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

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use torsten_primitives::time::SlotNo;

    fn make_gsm(enabled: bool, marker_path: &str) -> GenesisStateMachine {
        let _ = std::fs::remove_file(marker_path);
        let config = GsmConfig {
            min_active_blp: 3,
            max_tip_age_secs: 600,
            genesis_window_slots: 1_000,
            marker_path: PathBuf::from(marker_path),
        };
        GenesisStateMachine::new(config, enabled)
    }

    #[test]
    fn test_gsm_disabled_stays_caught_up() {
        let mut gsm = make_gsm(false, "/tmp/test_gsm_disabled_marker");
        assert_eq!(gsm.state(), GenesisSyncState::CaughtUp);
        let result = gsm.evaluate(0, false, 9999);
        assert_eq!(result, None);
        assert_eq!(gsm.state(), GenesisSyncState::CaughtUp);
    }

    #[test]
    fn test_gsm_presyncing_to_syncing() {
        let mut gsm = make_gsm(true, "/tmp/test_gsm_pre_sync_marker");
        assert_eq!(gsm.state(), GenesisSyncState::PreSyncing);

        // Not enough BLPs
        assert_eq!(gsm.evaluate(2, false, 0), None);
        assert_eq!(gsm.state(), GenesisSyncState::PreSyncing);

        // Enough BLPs
        let result = gsm.evaluate(3, false, 0);
        assert_eq!(result, Some(GenesisSyncState::Syncing));
        assert_eq!(gsm.state(), GenesisSyncState::Syncing);
    }

    #[test]
    fn test_gsm_syncing_to_caught_up() {
        let marker = PathBuf::from("/tmp/test_gsm_sync_caught_marker");
        let _ = std::fs::remove_file(&marker);
        let config = GsmConfig {
            min_active_blp: 1,
            max_tip_age_secs: 600,
            genesis_window_slots: 1_000,
            marker_path: marker.clone(),
        };
        let mut gsm = GenesisStateMachine::new(config, true);

        gsm.evaluate(5, false, 0); // → Syncing
        assert_eq!(gsm.state(), GenesisSyncState::Syncing);

        // Not idle yet
        assert_eq!(gsm.evaluate(5, false, 100), None);

        // Idle but tip too old
        assert_eq!(gsm.evaluate(5, true, 700), None);

        // Idle and tip fresh
        let result = gsm.evaluate(5, true, 100);
        assert_eq!(result, Some(GenesisSyncState::CaughtUp));
        assert_eq!(gsm.state(), GenesisSyncState::CaughtUp);

        assert!(marker.exists());
        let _ = std::fs::remove_file(&marker);
    }

    #[test]
    fn test_gsm_caught_up_regression() {
        let marker = PathBuf::from("/tmp/test_gsm_regression_marker");
        let _ = std::fs::remove_file(&marker);
        let config = GsmConfig {
            min_active_blp: 1,
            max_tip_age_secs: 600,
            genesis_window_slots: 1_000,
            marker_path: marker.clone(),
        };
        let mut gsm = GenesisStateMachine::new(config, true);

        gsm.evaluate(5, false, 0); // → Syncing
        gsm.evaluate(5, true, 100); // → CaughtUp
        assert_eq!(gsm.state(), GenesisSyncState::CaughtUp);

        let result = gsm.evaluate(5, false, 1300);
        assert_eq!(result, Some(GenesisSyncState::PreSyncing));
        let _ = std::fs::remove_file(&marker);
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

    // ── LoE tests ────────────────────────────────────────────────────────────

    #[test]
    fn test_loe_presyncing_blocks_all() {
        let gsm = make_gsm(true, "/tmp/test_loe_pre_marker");
        assert_eq!(gsm.state(), GenesisSyncState::PreSyncing);
        let limit = gsm.loe_limit(&[]);
        assert_eq!(limit, Some(0));
    }

    #[test]
    fn test_loe_syncing_constrains_to_common_prefix() {
        let mut gsm = make_gsm(true, "/tmp/test_loe_sync_marker");
        gsm.evaluate(5, false, 0); // → Syncing

        let tips = vec![
            Point::Specific(SlotNo(1000), torsten_primitives::hash::Hash32::ZERO),
            Point::Specific(SlotNo(800), torsten_primitives::hash::Hash32::ZERO),
            Point::Specific(SlotNo(1200), torsten_primitives::hash::Hash32::ZERO),
        ];
        let limit = gsm.loe_limit(&tips);
        assert_eq!(limit, Some(800));
    }

    #[test]
    fn test_loe_caught_up_no_constraint() {
        let marker = PathBuf::from("/tmp/test_loe_caught_marker");
        std::fs::write(&marker, "caught_up").unwrap();
        let config = GsmConfig {
            marker_path: marker.clone(),
            ..Default::default()
        };
        let gsm = GenesisStateMachine::new(config, true);
        assert_eq!(gsm.state(), GenesisSyncState::CaughtUp);
        assert_eq!(gsm.loe_limit(&[]), None);
        let _ = std::fs::remove_file(&marker);
    }

    // ── GDD tests ────────────────────────────────────────────────────────────

    #[test]
    fn test_gdd_disconnects_sparse_peers() {
        let mut gsm = make_gsm(true, "/tmp/test_gdd_marker");
        gsm.evaluate(5, false, 0); // → Syncing

        let addr_good: SocketAddr = "1.2.3.4:3001".parse().unwrap();
        let addr_bad: SocketAddr = "5.6.7.8:3001".parse().unwrap();

        // Register peers at intersection 0
        gsm.register_peer(addr_good, 0, 1000);
        gsm.register_peer(addr_bad, 0, 1000);

        // Good peer: 900 dense blocks in window of 1000
        for slot in 1..=900u64 {
            gsm.record_block(&addr_good, slot);
        }
        // Bad peer: only 10 sparse blocks
        for slot in 1..=10u64 {
            gsm.record_block(&addr_bad, slot);
        }

        let to_disconnect = gsm.gdd_evaluate();
        assert!(
            to_disconnect.contains(&addr_bad),
            "Sparse peer should be disconnected"
        );
        assert!(
            !to_disconnect.contains(&addr_good),
            "Dense peer should NOT be disconnected"
        );
    }

    #[test]
    fn test_gdd_no_disconnect_when_caught_up() {
        let marker = PathBuf::from("/tmp/test_gdd_disabled_marker");
        std::fs::write(&marker, "caught_up").unwrap();
        let config = GsmConfig {
            marker_path: marker.clone(),
            genesis_window_slots: 1_000,
            ..Default::default()
        };
        let mut gsm = GenesisStateMachine::new(config, true);
        assert_eq!(gsm.state(), GenesisSyncState::CaughtUp);

        let addr_a: SocketAddr = "1.2.3.4:3001".parse().unwrap();
        let addr_b: SocketAddr = "5.6.7.8:3001".parse().unwrap();
        gsm.register_peer(addr_a, 0, 1000);
        gsm.register_peer(addr_b, 0, 1000);

        // Even with a sparse peer, GDD should not fire when CaughtUp
        for slot in 1..=900u64 {
            gsm.record_block(&addr_a, slot);
        }
        let to_disconnect = gsm.gdd_evaluate();
        assert!(to_disconnect.is_empty(), "GDD inactive in CaughtUp");
        let _ = std::fs::remove_file(&marker);
    }

    #[test]
    fn test_gdd_single_peer_never_disconnects() {
        let mut gsm = make_gsm(true, "/tmp/test_gdd_single_marker");
        gsm.evaluate(5, false, 0); // → Syncing

        let addr: SocketAddr = "1.2.3.4:3001".parse().unwrap();
        gsm.register_peer(addr, 0, 1000);
        // No matter how sparse, with only 1 peer there's no comparison to make
        let to_disconnect = gsm.gdd_evaluate();
        assert!(
            to_disconnect.is_empty(),
            "Single peer must not be disconnected"
        );
    }

    #[test]
    fn test_gdd_disabled_when_not_enabled() {
        // GSM disabled (praos mode)
        let config = GsmConfig {
            marker_path: PathBuf::from("/tmp/test_gdd_not_enabled_marker"),
            genesis_window_slots: 1_000,
            ..Default::default()
        };
        let mut gsm = GenesisStateMachine::new(config, false);
        let addr_a: SocketAddr = "1.2.3.4:3001".parse().unwrap();
        let addr_b: SocketAddr = "5.6.7.8:3001".parse().unwrap();
        gsm.register_peer(addr_a, 0, 1000);
        gsm.register_peer(addr_b, 0, 1000);
        // Dense vs sparse — but genesis is disabled
        for slot in 1..=900u64 {
            gsm.record_block(&addr_a, slot);
        }
        assert!(
            gsm.gdd_evaluate().is_empty(),
            "GDD must be inactive when genesis mode is disabled"
        );
    }

    #[test]
    fn test_gdd_equal_density_no_disconnect() {
        let mut gsm = make_gsm(true, "/tmp/test_gdd_equal_marker");
        gsm.evaluate(5, false, 0); // → Syncing

        let addr_a: SocketAddr = "1.2.3.4:3001".parse().unwrap();
        let addr_b: SocketAddr = "5.6.7.8:3001".parse().unwrap();
        gsm.register_peer(addr_a, 0, 1000);
        gsm.register_peer(addr_b, 0, 1000);

        // Both peers have identical density
        for slot in [100u64, 200, 300, 400, 500].iter() {
            gsm.record_block(&addr_a, *slot);
            gsm.record_block(&addr_b, *slot);
        }
        let to_disconnect = gsm.gdd_evaluate();
        assert!(
            to_disconnect.is_empty(),
            "Equal-density peers must not be disconnected"
        );
    }

    #[test]
    fn test_register_deregister_peer() {
        let mut gsm = make_gsm(true, "/tmp/test_register_marker");
        gsm.evaluate(5, false, 0); // → Syncing

        let addr: SocketAddr = "1.2.3.4:3001".parse().unwrap();
        assert_eq!(gsm.peer_info().len(), 0);

        gsm.register_peer(addr, 500, 1500);
        assert_eq!(gsm.peer_info().len(), 1);
        assert_eq!(gsm.intersection_slot, 500);

        gsm.deregister_peer(&addr);
        assert_eq!(gsm.peer_info().len(), 0);
    }

    #[test]
    fn test_record_block_updates_density() {
        let mut gsm = make_gsm(true, "/tmp/test_record_density_marker");
        gsm.evaluate(5, false, 0); // → Syncing

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
        // 90% threshold = 1800. Pools 1+2+3 = 1800 (cumulative).
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
