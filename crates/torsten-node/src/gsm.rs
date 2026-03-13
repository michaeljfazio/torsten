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

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
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
#[allow(dead_code)]
pub struct GsmConfig {
    /// Minimum active big ledger peers to transition PreSyncing → Syncing
    pub min_active_blp: usize,
    /// Maximum tip age (seconds) before CaughtUp → PreSyncing regression
    pub max_tip_age_secs: u64,
    /// Genesis security parameter (window size in slots for GDD density comparison)
    pub genesis_window_slots: u64,
    /// Path for the caught_up marker file
    pub marker_path: PathBuf,
}

impl Default for GsmConfig {
    fn default() -> Self {
        GsmConfig {
            min_active_blp: 5,
            max_tip_age_secs: 1200,       // 20 minutes
            genesis_window_slots: 36_000, // ~10 hours at 1s slots
            marker_path: PathBuf::from("caught_up.marker"),
        }
    }
}

/// The Genesis State Machine.
///
/// Tracks sync state and enforces transitions based on peer availability
/// and tip freshness. The GSM also manages:
/// - **LoE (Limit on Eagerness)**: constrains block application during sync
/// - **GDD (Genesis Density Disconnector)**: disconnects sparse-chain peers
#[allow(dead_code)]
pub struct GenesisStateMachine {
    config: GsmConfig,
    state: GenesisSyncState,
    /// Whether genesis mode is enabled (opt-in via --consensus-mode genesis)
    enabled: bool,
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
        }
    }

    /// Current sync state
    pub fn state(&self) -> GenesisSyncState {
        self.state
    }

    /// Whether genesis mode is enabled
    #[allow(dead_code)]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

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
                // and there's no better candidate chain
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

    /// Check the Limit on Eagerness constraint.
    ///
    /// Returns the maximum immutable tip slot that block application should
    /// advance to. Returns `None` if there is no constraint (CaughtUp state).
    #[allow(dead_code)]
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

    /// Run the Genesis Density Disconnector (GDD).
    ///
    /// During Syncing state, compares chain density across peers within the
    /// genesis window. Peers whose density upper bound is ≤ another peer's
    /// lower bound are disconnected.
    ///
    /// Returns addresses of peers that should be disconnected.
    #[allow(dead_code)]
    pub fn gdd_evaluate(
        &self,
        peer_chain_lengths: &HashMap<SocketAddr, PeerChainInfo>,
    ) -> Vec<SocketAddr> {
        if !self.enabled || self.state != GenesisSyncState::Syncing {
            return Vec::new();
        }

        if peer_chain_lengths.len() < 2 {
            return Vec::new();
        }

        let mut to_disconnect = Vec::new();

        // For each peer, compute density bounds within genesis window
        let densities: Vec<(SocketAddr, f64, f64)> = peer_chain_lengths
            .iter()
            .map(|(addr, info)| {
                let window = self.config.genesis_window_slots as f64;
                // Lower bound: assume minimum blocks in window
                let lower = info.blocks_in_window as f64 / window;
                // Upper bound: actual observed + 10% margin for latency
                let upper = (info.blocks_in_window as f64 * 1.1) / window;
                (*addr, lower, upper)
            })
            .collect();

        // Find the maximum lower bound (best peer's guaranteed density)
        let max_lower = densities
            .iter()
            .map(|(_, lower, _)| *lower)
            .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap_or(0.0);

        // Disconnect peers whose upper bound is ≤ the max lower bound
        for (addr, _, upper) in &densities {
            if *upper > 0.0 && *upper <= max_lower {
                debug!(
                    %addr,
                    upper,
                    max_lower,
                    "GDD: disconnecting sparse peer"
                );
                to_disconnect.push(*addr);
            }
        }

        if !to_disconnect.is_empty() {
            info!(
                disconnecting = to_disconnect.len(),
                "GDD: disconnecting peers with insufficient chain density"
            );
        }

        to_disconnect
    }

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

/// Chain info tracked per peer for GDD density comparison.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PeerChainInfo {
    /// Number of blocks this peer's chain has within the genesis window
    pub blocks_in_window: u64,
    /// Peer's reported tip slot
    pub tip_slot: u64,
}

/// Identify big ledger peers from the stake distribution.
///
/// Sorts pools by active stake descending and accumulates until 90% of total
/// active stake is covered. Pools in the top 90% are "big ledger peers".
///
/// Returns (big_ledger_pool_ids, remaining_pool_ids).
#[allow(dead_code)]
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

/// Load peer snapshot from a JSON file.
///
/// Format: `[{"addr": "1.2.3.4", "port": 3001}, ...]`
#[allow(dead_code)]
pub fn load_peer_snapshot(path: &Path) -> Result<Vec<SocketAddr>, String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use torsten_primitives::time::SlotNo;

    #[test]
    fn test_gsm_disabled_stays_caught_up() {
        let config = GsmConfig {
            marker_path: PathBuf::from("/tmp/test_gsm_disabled_marker"),
            ..Default::default()
        };
        let mut gsm = GenesisStateMachine::new(config, false);

        assert_eq!(gsm.state(), GenesisSyncState::CaughtUp);
        // Should not transition even with bad conditions
        let result = gsm.evaluate(0, false, 9999);
        assert_eq!(result, None);
        assert_eq!(gsm.state(), GenesisSyncState::CaughtUp);
    }

    #[test]
    fn test_gsm_presyncing_to_syncing() {
        let config = GsmConfig {
            min_active_blp: 3,
            marker_path: PathBuf::from("/tmp/test_gsm_pre_sync_marker"),
            ..Default::default()
        };
        // Ensure no marker file
        let _ = std::fs::remove_file(&config.marker_path);
        let mut gsm = GenesisStateMachine::new(config, true);

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
        let config = GsmConfig {
            min_active_blp: 1,
            max_tip_age_secs: 600,
            marker_path: PathBuf::from("/tmp/test_gsm_sync_caught_marker"),
            ..Default::default()
        };
        let _ = std::fs::remove_file(&config.marker_path);
        let mut gsm = GenesisStateMachine::new(config.clone(), true);

        // Move to Syncing
        gsm.evaluate(5, false, 0);
        assert_eq!(gsm.state(), GenesisSyncState::Syncing);

        // Not idle yet
        assert_eq!(gsm.evaluate(5, false, 100), None);
        assert_eq!(gsm.state(), GenesisSyncState::Syncing);

        // Idle but tip too old
        assert_eq!(gsm.evaluate(5, true, 700), None);
        assert_eq!(gsm.state(), GenesisSyncState::Syncing);

        // Idle and tip fresh
        let result = gsm.evaluate(5, true, 100);
        assert_eq!(result, Some(GenesisSyncState::CaughtUp));
        assert_eq!(gsm.state(), GenesisSyncState::CaughtUp);

        // Marker file should exist
        assert!(config.marker_path.exists());
        let _ = std::fs::remove_file(&config.marker_path);
    }

    #[test]
    fn test_gsm_caught_up_regression() {
        let config = GsmConfig {
            min_active_blp: 1,
            max_tip_age_secs: 600,
            marker_path: PathBuf::from("/tmp/test_gsm_regression_marker"),
            ..Default::default()
        };
        let _ = std::fs::remove_file(&config.marker_path);
        let mut gsm = GenesisStateMachine::new(config.clone(), true);

        // Fast-track to CaughtUp
        gsm.evaluate(5, false, 0); // → Syncing
        gsm.evaluate(5, true, 100); // → CaughtUp
        assert_eq!(gsm.state(), GenesisSyncState::CaughtUp);

        // Tip becomes stale → regress
        let result = gsm.evaluate(5, false, 1300);
        assert_eq!(result, Some(GenesisSyncState::PreSyncing));
        assert_eq!(gsm.state(), GenesisSyncState::PreSyncing);

        let _ = std::fs::remove_file(&config.marker_path);
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
    fn test_loe_presyncing_blocks_all() {
        let config = GsmConfig {
            marker_path: PathBuf::from("/tmp/test_loe_pre_marker"),
            ..Default::default()
        };
        let _ = std::fs::remove_file(&config.marker_path);
        let gsm = GenesisStateMachine::new(config, true);
        assert_eq!(gsm.state(), GenesisSyncState::PreSyncing);

        let limit = gsm.loe_limit(&[]);
        assert_eq!(limit, Some(0)); // Block everything
    }

    #[test]
    fn test_loe_syncing_constrains_to_common_prefix() {
        let config = GsmConfig {
            min_active_blp: 1,
            marker_path: PathBuf::from("/tmp/test_loe_sync_marker"),
            ..Default::default()
        };
        let _ = std::fs::remove_file(&config.marker_path);
        let mut gsm = GenesisStateMachine::new(config, true);
        gsm.evaluate(5, false, 0); // → Syncing

        let tips = vec![
            Point::Specific(SlotNo(1000), torsten_primitives::hash::Hash32::ZERO),
            Point::Specific(SlotNo(800), torsten_primitives::hash::Hash32::ZERO),
            Point::Specific(SlotNo(1200), torsten_primitives::hash::Hash32::ZERO),
        ];
        let limit = gsm.loe_limit(&tips);
        assert_eq!(limit, Some(800)); // Min of all candidate tip slots
    }

    #[test]
    fn test_loe_caught_up_no_constraint() {
        let config = GsmConfig {
            marker_path: PathBuf::from("/tmp/test_loe_caught_marker"),
            ..Default::default()
        };
        let _ = std::fs::remove_file(&config.marker_path);
        std::fs::write(&config.marker_path, "caught_up").unwrap();
        let gsm = GenesisStateMachine::new(config.clone(), true);
        assert_eq!(gsm.state(), GenesisSyncState::CaughtUp);

        let limit = gsm.loe_limit(&[]);
        assert_eq!(limit, None); // No constraint
        let _ = std::fs::remove_file(&config.marker_path);
    }

    #[test]
    fn test_gdd_disconnects_sparse_peers() {
        let config = GsmConfig {
            min_active_blp: 1,
            genesis_window_slots: 1000,
            marker_path: PathBuf::from("/tmp/test_gdd_marker"),
            ..Default::default()
        };
        let _ = std::fs::remove_file(&config.marker_path);
        let mut gsm = GenesisStateMachine::new(config, true);
        gsm.evaluate(5, false, 0); // → Syncing

        let addr_good: SocketAddr = "1.2.3.4:3001".parse().unwrap();
        let addr_bad: SocketAddr = "5.6.7.8:3001".parse().unwrap();

        let mut peers = HashMap::new();
        peers.insert(
            addr_good,
            PeerChainInfo {
                blocks_in_window: 900,
                tip_slot: 1000,
            },
        );
        peers.insert(
            addr_bad,
            PeerChainInfo {
                blocks_in_window: 100, // Very sparse
                tip_slot: 1000,
            },
        );

        let to_disconnect = gsm.gdd_evaluate(&peers);
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
    fn test_gdd_disabled_when_caught_up() {
        let config = GsmConfig {
            marker_path: PathBuf::from("/tmp/test_gdd_disabled_marker"),
            ..Default::default()
        };
        let _ = std::fs::remove_file(&config.marker_path);
        std::fs::write(&config.marker_path, "caught_up").unwrap();
        let gsm = GenesisStateMachine::new(config.clone(), true);

        let mut peers = HashMap::new();
        peers.insert(
            "1.2.3.4:3001".parse().unwrap(),
            PeerChainInfo {
                blocks_in_window: 900,
                tip_slot: 1000,
            },
        );
        peers.insert(
            "5.6.7.8:3001".parse().unwrap(),
            PeerChainInfo {
                blocks_in_window: 10,
                tip_slot: 1000,
            },
        );

        let to_disconnect = gsm.gdd_evaluate(&peers);
        assert!(
            to_disconnect.is_empty(),
            "GDD should be inactive when CaughtUp"
        );
        let _ = std::fs::remove_file(&config.marker_path);
    }

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
        // 90% threshold = 1800. Pool 1 (1000) + Pool 2 (500) + Pool 3 (300) = 1800
        assert!(big.len() >= 2, "Should have at least 2 big ledger peers");
        assert!(
            !remaining.is_empty(),
            "Should have some remaining small pools"
        );
    }

    #[test]
    fn test_identify_big_ledger_peers_empty() {
        let (big, remaining) = identify_big_ledger_peers(&[]);
        assert!(big.is_empty());
        assert!(remaining.is_empty());
    }

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
