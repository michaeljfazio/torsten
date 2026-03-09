//! Peer manager for P2P networking.
//!
//! Manages outbound and inbound peer connections following cardano-node's
//! peer management model with cold/warm/hot peer sets.
//!
//! Supports both **InitiatorOnly** and **InitiatorAndResponder** (bidirectional)
//! diffusion modes, matching the Haskell cardano-node behavior.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tracing::{debug, info};

/// Diffusion mode matching cardano-node
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DiffusionMode {
    /// Connect outbound only (typical non-relay nodes)
    InitiatorOnly,
    /// Both initiate and accept connections (relay nodes, stake pool nodes)
    #[default]
    InitiatorAndResponder,
}

/// Peer temperature classification (matching cardano-node)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PeerTemperature {
    /// Known but not connected
    Cold,
    /// Connected but not actively syncing
    Warm,
    /// Actively syncing/exchanging data
    Hot,
}

/// Peer source — how we learned about this peer
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerSource {
    /// From topology file (local roots, bootstrap peers)
    Config,
    /// From peer sharing protocol (gossip)
    PeerSharing,
    /// From ledger-based peer discovery
    Ledger,
}

/// Tracked peer state
#[derive(Debug, Clone)]
pub struct PeerInfo {
    pub address: SocketAddr,
    pub temperature: PeerTemperature,
    pub source: PeerSource,
    pub last_connected: Option<Instant>,
    pub last_failed: Option<Instant>,
    pub failure_count: u32,
    pub is_trustable: bool,
    pub advertise: bool,
    /// Negotiated protocol version (if connected)
    pub version: Option<u32>,
    /// Remote tip slot (if known)
    pub remote_tip_slot: Option<u64>,
    /// Connection direction
    pub is_initiator: Option<bool>,
    /// Performance metrics for adaptive peer selection
    pub performance: PeerPerformance,
}

/// Performance metrics tracked per peer for adaptive selection
#[derive(Debug, Clone)]
pub struct PeerPerformance {
    /// Exponentially weighted moving average of handshake RTT in milliseconds
    pub avg_handshake_rtt_ms: Option<f64>,
    /// Exponentially weighted moving average of block fetch time in milliseconds
    pub avg_block_fetch_ms: Option<f64>,
    /// Total bytes received from this peer
    pub bytes_received: u64,
    /// Total blocks successfully fetched from this peer
    pub blocks_fetched: u64,
    /// Number of successful interactions (connects, fetches)
    pub success_count: u64,
    /// Timestamp of last successful block fetch
    pub last_good_fetch: Option<Instant>,
    /// Computed reputation score (0.0 = worst, 1.0 = best)
    pub reputation: f64,
}

impl Default for PeerPerformance {
    fn default() -> Self {
        PeerPerformance {
            avg_handshake_rtt_ms: None,
            avg_block_fetch_ms: None,
            bytes_received: 0,
            blocks_fetched: 0,
            success_count: 0,
            last_good_fetch: None,
            reputation: 0.5, // Neutral starting reputation
        }
    }
}

/// EWMA smoothing factor (higher = more weight on recent observations)
const EWMA_ALPHA: f64 = 0.3;

impl PeerPerformance {
    /// Record a handshake latency measurement
    pub fn record_handshake_rtt(&mut self, rtt_ms: f64) {
        self.avg_handshake_rtt_ms = Some(match self.avg_handshake_rtt_ms {
            Some(avg) => avg * (1.0 - EWMA_ALPHA) + rtt_ms * EWMA_ALPHA,
            None => rtt_ms,
        });
        self.success_count += 1;
    }

    /// Record a block fetch latency measurement (per block)
    pub fn record_block_fetch(&mut self, fetch_ms: f64, block_count: u64, bytes: u64) {
        let per_block_ms = if block_count > 0 {
            fetch_ms / block_count as f64
        } else {
            fetch_ms
        };
        self.avg_block_fetch_ms = Some(match self.avg_block_fetch_ms {
            Some(avg) => avg * (1.0 - EWMA_ALPHA) + per_block_ms * EWMA_ALPHA,
            None => per_block_ms,
        });
        self.blocks_fetched += block_count;
        self.bytes_received += bytes;
        self.success_count += 1;
        self.last_good_fetch = Some(Instant::now());
    }

    /// Compute the reputation score based on all metrics.
    /// Returns a value in [0.0, 1.0] where higher is better.
    pub fn compute_reputation(&mut self, failure_count: u32) -> f64 {
        let mut score = 0.5_f64;

        // Latency component: lower handshake RTT is better
        // Baseline: 200ms is average, <50ms is excellent, >1000ms is poor
        if let Some(rtt) = self.avg_handshake_rtt_ms {
            let latency_score = (1.0 - (rtt / 2000.0).min(1.0)).max(0.0);
            score += 0.15 * (latency_score - 0.5);
        }

        // Block fetch speed: lower per-block time is better
        // Baseline: 50ms/block is average
        if let Some(fetch_ms) = self.avg_block_fetch_ms {
            let fetch_score = (1.0 - (fetch_ms / 200.0).min(1.0)).max(0.0);
            score += 0.2 * (fetch_score - 0.5);
        }

        // Volume component: peers that have served more blocks are more reliable
        let volume_score = (self.blocks_fetched as f64 / 10000.0).min(1.0);
        score += 0.1 * (volume_score - 0.5);

        // Reliability: ratio of successes vs failures
        let total = self.success_count + failure_count as u64;
        if total > 0 {
            let reliability = self.success_count as f64 / total as f64;
            score += 0.15 * (reliability - 0.5);
        }

        // Recency: peers with recent activity are preferred
        if let Some(last_fetch) = self.last_good_fetch {
            let minutes_ago = last_fetch.elapsed().as_secs_f64() / 60.0;
            let recency_score = (1.0 - (minutes_ago / 60.0).min(1.0)).max(0.0);
            score += 0.1 * (recency_score - 0.5);
        }

        self.reputation = score.clamp(0.0, 1.0);
        self.reputation
    }
}

impl PeerInfo {
    pub fn new(address: SocketAddr, source: PeerSource) -> Self {
        PeerInfo {
            address,
            temperature: PeerTemperature::Cold,
            source,
            last_connected: None,
            last_failed: None,
            failure_count: 0,
            is_trustable: false,
            advertise: false,
            version: None,
            remote_tip_slot: None,
            is_initiator: None,
            performance: PeerPerformance::default(),
        }
    }

    /// Whether this peer should be retried after failure
    pub fn should_retry(&self) -> bool {
        match self.last_failed {
            None => true,
            Some(t) => {
                // Exponential backoff: 5s, 10s, 20s, 40s, 60s max
                let delay = Duration::from_secs(
                    5u64.saturating_mul(2u64.saturating_pow(self.failure_count.min(4)))
                        .min(60),
                );
                t.elapsed() >= delay
            }
        }
    }

    /// Compute a selection score for ranking peers.
    /// Higher score = more preferred for connection.
    pub fn selection_score(&self) -> f64 {
        let mut score = self.performance.reputation;

        // Bonus for trustable peers (from topology)
        if self.is_trustable {
            score += 0.2;
        }

        // Bonus for config-sourced peers (most reliable)
        match self.source {
            PeerSource::Config => score += 0.1,
            PeerSource::PeerSharing => {}
            PeerSource::Ledger => score += 0.05,
        }

        // Penalty for failure history
        if self.failure_count > 0 {
            score -= (self.failure_count as f64 * 0.05).min(0.3);
        }

        // Bonus for peers with known recent tip
        if let Some(tip_slot) = self.remote_tip_slot {
            if tip_slot > 0 {
                score += 0.05;
            }
        }

        score
    }
}

/// Configuration for the peer manager
#[derive(Debug, Clone)]
pub struct PeerManagerConfig {
    /// Target number of hot (actively syncing) peers
    pub target_hot_peers: usize,
    /// Target number of warm (connected, not syncing) peers
    pub target_warm_peers: usize,
    /// Target number of known peers (including cold)
    pub target_known_peers: usize,
    /// Maximum inbound connections to accept
    pub max_inbound_peers: usize,
    /// Whether to enable peer sharing
    pub peer_sharing_enabled: bool,
    /// Diffusion mode
    pub diffusion_mode: DiffusionMode,
    /// How often to churn peer connections (seconds)
    pub churn_interval_secs: u64,
}

impl Default for PeerManagerConfig {
    fn default() -> Self {
        PeerManagerConfig {
            target_hot_peers: 20,
            target_warm_peers: 20,
            target_known_peers: 100,
            max_inbound_peers: 100,
            peer_sharing_enabled: true,
            diffusion_mode: DiffusionMode::InitiatorAndResponder,
            churn_interval_secs: 300,
        }
    }
}

/// Events emitted by the peer manager
#[derive(Debug)]
pub enum PeerManagerEvent {
    /// Should connect to this peer
    Connect(SocketAddr),
    /// Should disconnect from this peer
    Disconnect(SocketAddr),
    /// Should promote warm peer to hot (start syncing)
    PromoteToHot(SocketAddr),
    /// Should demote hot peer to warm (stop syncing)
    DemoteToWarm(SocketAddr),
}

/// The peer manager tracks all known peers and drives connection decisions.
pub struct PeerManager {
    config: PeerManagerConfig,
    peers: HashMap<SocketAddr, PeerInfo>,
    hot_peers: HashSet<SocketAddr>,
    warm_peers: HashSet<SocketAddr>,
    cold_peers: HashSet<SocketAddr>,
    inbound_count: usize,
}

impl PeerManager {
    pub fn new(config: PeerManagerConfig) -> Self {
        PeerManager {
            config,
            peers: HashMap::new(),
            hot_peers: HashSet::new(),
            warm_peers: HashSet::new(),
            cold_peers: HashSet::new(),
            inbound_count: 0,
        }
    }

    /// Add a peer from the topology/config
    pub fn add_config_peer(&mut self, addr: SocketAddr, trustable: bool, advertise: bool) {
        let mut info = PeerInfo::new(addr, PeerSource::Config);
        info.is_trustable = trustable;
        info.advertise = advertise;
        self.cold_peers.insert(addr);
        self.peers.insert(addr, info);
    }

    /// Add a peer discovered from the ledger (SPO relay registrations)
    pub fn add_ledger_peer(&mut self, addr: SocketAddr) {
        if self.peers.contains_key(&addr) {
            return; // Already known
        }
        if self.peers.len() >= self.config.target_known_peers {
            return; // At capacity
        }
        let info = PeerInfo::new(addr, PeerSource::Ledger);
        self.cold_peers.insert(addr);
        self.peers.insert(addr, info);
        debug!(%addr, "Discovered peer from ledger");
    }

    /// Add a peer discovered via peer sharing
    pub fn add_shared_peer(&mut self, addr: SocketAddr) {
        if self.peers.contains_key(&addr) {
            return; // Already known
        }
        if self.peers.len() >= self.config.target_known_peers {
            return; // At capacity
        }
        let info = PeerInfo::new(addr, PeerSource::PeerSharing);
        self.cold_peers.insert(addr);
        self.peers.insert(addr, info);
        debug!(%addr, "Discovered peer via sharing");
    }

    /// Mark a peer as successfully connected (warm)
    pub fn peer_connected(&mut self, addr: &SocketAddr, version: u32, is_initiator: bool) {
        if let Some(info) = self.peers.get_mut(addr) {
            info.temperature = PeerTemperature::Warm;
            info.last_connected = Some(Instant::now());
            info.failure_count = 0;
            info.version = Some(version);
            info.is_initiator = Some(is_initiator);
            self.cold_peers.remove(addr);
            self.warm_peers.insert(*addr);
            if !is_initiator {
                self.inbound_count += 1;
            }
            info!(%addr, version, is_initiator, "Peer connected (warm)");
        }
    }

    /// Promote a warm peer to hot (start syncing)
    pub fn promote_to_hot(&mut self, addr: &SocketAddr) {
        if let Some(info) = self.peers.get_mut(addr) {
            if info.temperature == PeerTemperature::Warm {
                info.temperature = PeerTemperature::Hot;
                self.warm_peers.remove(addr);
                self.hot_peers.insert(*addr);
                debug!(%addr, "Peer promoted to hot");
            }
        }
    }

    /// Demote a hot peer to warm (stop syncing)
    pub fn demote_to_warm(&mut self, addr: &SocketAddr) {
        if let Some(info) = self.peers.get_mut(addr) {
            if info.temperature == PeerTemperature::Hot {
                info.temperature = PeerTemperature::Warm;
                self.hot_peers.remove(addr);
                self.warm_peers.insert(*addr);
                debug!(%addr, "Peer demoted to warm");
            }
        }
    }

    /// Mark a peer as disconnected
    pub fn peer_disconnected(&mut self, addr: &SocketAddr) {
        if let Some(info) = self.peers.get_mut(addr) {
            if info.is_initiator == Some(false) {
                self.inbound_count = self.inbound_count.saturating_sub(1);
            }
            info.temperature = PeerTemperature::Cold;
            info.version = None;
            info.is_initiator = None;
            self.hot_peers.remove(addr);
            self.warm_peers.remove(addr);
            self.cold_peers.insert(*addr);
        }
    }

    /// Mark a peer as failed (connection attempt failed)
    pub fn peer_failed(&mut self, addr: &SocketAddr) {
        if let Some(info) = self.peers.get_mut(addr) {
            info.last_failed = Some(Instant::now());
            info.failure_count += 1;
            info.temperature = PeerTemperature::Cold;
            info.version = None;
            info.is_initiator = None;
            self.hot_peers.remove(addr);
            self.warm_peers.remove(addr);
            self.cold_peers.insert(*addr);
        }
    }

    /// Update a peer's remote tip
    pub fn update_tip(&mut self, addr: &SocketAddr, tip_slot: u64) {
        if let Some(info) = self.peers.get_mut(addr) {
            info.remote_tip_slot = Some(tip_slot);
        }
    }

    /// Check if we should accept an inbound connection
    pub fn should_accept_inbound(&self) -> bool {
        self.config.diffusion_mode == DiffusionMode::InitiatorAndResponder
            && self.inbound_count < self.config.max_inbound_peers
    }

    /// Get peers that should be connected to (cold peers that need promotion),
    /// ranked by selection score (highest first).
    pub fn peers_to_connect(&self) -> Vec<SocketAddr> {
        let connected = self.hot_peers.len() + self.warm_peers.len();
        let target = self.config.target_hot_peers + self.config.target_warm_peers;
        if connected >= target {
            return vec![];
        }

        let needed = target - connected;
        let mut candidates: Vec<_> = self
            .cold_peers
            .iter()
            .filter_map(|addr| {
                self.peers.get(addr).and_then(|p| {
                    if p.should_retry() {
                        Some((*addr, p.selection_score()))
                    } else {
                        None
                    }
                })
            })
            .collect();

        // Sort by score descending (best peers first)
        candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        candidates
            .into_iter()
            .map(|(addr, _)| addr)
            .take(needed)
            .collect()
    }

    /// Get warm peers that should be promoted to hot, ranked by selection score.
    pub fn peers_to_promote(&self) -> Vec<SocketAddr> {
        if self.hot_peers.len() >= self.config.target_hot_peers {
            return vec![];
        }
        let needed = self.config.target_hot_peers - self.hot_peers.len();
        let mut candidates: Vec<_> = self
            .warm_peers
            .iter()
            .filter_map(|addr| self.peers.get(addr).map(|p| (*addr, p.selection_score())))
            .collect();

        candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        candidates
            .into_iter()
            .map(|(addr, _)| addr)
            .take(needed)
            .collect()
    }

    /// Record a handshake latency measurement for a peer
    pub fn record_handshake_rtt(&mut self, addr: &SocketAddr, rtt_ms: f64) {
        if let Some(info) = self.peers.get_mut(addr) {
            info.performance.record_handshake_rtt(rtt_ms);
            debug!(%addr, rtt_ms, "Recorded handshake RTT");
        }
    }

    /// Record a block fetch performance measurement for a peer
    pub fn record_block_fetch(
        &mut self,
        addr: &SocketAddr,
        fetch_ms: f64,
        block_count: u64,
        bytes: u64,
    ) {
        if let Some(info) = self.peers.get_mut(addr) {
            info.performance
                .record_block_fetch(fetch_ms, block_count, bytes);
        }
    }

    /// Recompute reputation scores for all peers.
    /// Applies time-based decay: failure counts halve after 5 minutes of no failures.
    pub fn recompute_reputations(&mut self) {
        for info in self.peers.values_mut() {
            // Decay failure count if last failure was > 5 minutes ago
            if info.failure_count > 0 {
                if let Some(last_failed) = info.last_failed {
                    let decay_interval = Duration::from_secs(300); // 5 minutes
                    let elapsed = last_failed.elapsed();
                    let decay_steps = (elapsed.as_secs() / decay_interval.as_secs()) as u32;
                    if decay_steps > 0 {
                        info.failure_count = info.failure_count.saturating_sub(decay_steps);
                    }
                }
            }
            info.performance.compute_reputation(info.failure_count);
        }
    }

    /// Number of currently hot (actively syncing) peers.
    pub fn hot_peer_count(&self) -> usize {
        self.hot_peers.len()
    }

    /// Get the best N peers by reputation for block fetching.
    /// Returns addresses of hot peers sorted by reputation (best first).
    pub fn best_peers_for_fetch(&self, count: usize) -> Vec<SocketAddr> {
        let mut candidates: Vec<_> = self
            .hot_peers
            .iter()
            .filter_map(|addr| self.peers.get(addr).map(|p| (*addr, p.selection_score())))
            .collect();

        candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        candidates
            .into_iter()
            .map(|(addr, _)| addr)
            .take(count)
            .collect()
    }

    /// Get the peer with the worst reputation among hot peers,
    /// used for demotion during churn.
    pub fn worst_hot_peer(&self) -> Option<SocketAddr> {
        self.hot_peers
            .iter()
            .filter_map(|addr| self.peers.get(addr).map(|p| (*addr, p.selection_score())))
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(addr, _)| addr)
    }

    /// Get performance info for a peer (for display/metrics)
    pub fn peer_performance(&self, addr: &SocketAddr) -> Option<&PeerPerformance> {
        self.peers.get(addr).map(|p| &p.performance)
    }

    /// Get the list of hot peer addresses
    pub fn hot_peer_addrs(&self) -> Vec<SocketAddr> {
        self.hot_peers.iter().copied().collect()
    }

    /// Get the list of all connected peer addresses
    pub fn connected_peer_addrs(&self) -> Vec<SocketAddr> {
        self.hot_peers
            .iter()
            .chain(self.warm_peers.iter())
            .copied()
            .collect()
    }

    /// Get peer addresses to share with a requesting peer
    pub fn peers_for_sharing(&self, max_count: usize) -> Vec<SocketAddr> {
        if !self.config.peer_sharing_enabled {
            return vec![];
        }
        self.peers
            .iter()
            .filter(|(_, info)| info.advertise && info.temperature != PeerTemperature::Cold)
            .map(|(addr, _)| *addr)
            .take(max_count)
            .collect()
    }

    /// Get the diffusion mode
    pub fn diffusion_mode(&self) -> DiffusionMode {
        self.config.diffusion_mode
    }

    /// Get statistics
    pub fn stats(&self) -> PeerManagerStats {
        let hot_reputations: Vec<f64> = self
            .hot_peers
            .iter()
            .filter_map(|addr| self.peers.get(addr))
            .map(|p| p.performance.reputation)
            .collect();

        let avg_hot_reputation = if hot_reputations.is_empty() {
            0.0
        } else {
            hot_reputations.iter().sum::<f64>() / hot_reputations.len() as f64
        };

        let best_fetch_latency_ms = self
            .hot_peers
            .iter()
            .filter_map(|addr| self.peers.get(addr))
            .filter_map(|p| p.performance.avg_block_fetch_ms)
            .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        PeerManagerStats {
            known_peers: self.peers.len(),
            cold_peers: self.cold_peers.len(),
            warm_peers: self.warm_peers.len(),
            hot_peers: self.hot_peers.len(),
            inbound_count: self.inbound_count,
            avg_hot_reputation,
            best_fetch_latency_ms,
        }
    }
}

/// Statistics for monitoring
#[derive(Debug, Clone)]
pub struct PeerManagerStats {
    pub known_peers: usize,
    pub cold_peers: usize,
    pub warm_peers: usize,
    pub hot_peers: usize,
    pub inbound_count: usize,
    /// Average reputation of hot peers
    pub avg_hot_reputation: f64,
    /// Best block fetch latency across hot peers (ms)
    pub best_fetch_latency_ms: Option<f64>,
}

impl std::fmt::Display for PeerManagerStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "peers: {} known ({} cold, {} warm, {} hot), {} inbound, avg_rep={:.2}",
            self.known_peers,
            self.cold_peers,
            self.warm_peers,
            self.hot_peers,
            self.inbound_count,
            self.avg_hot_reputation
        )?;
        if let Some(lat) = self.best_fetch_latency_ms {
            write!(f, ", best_fetch={lat:.0}ms")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_addr(port: u16) -> SocketAddr {
        format!("127.0.0.1:{port}").parse().unwrap()
    }

    #[test]
    fn test_add_config_peer() {
        let mut pm = PeerManager::new(PeerManagerConfig::default());
        let addr = test_addr(3001);
        pm.add_config_peer(addr, true, false);

        assert_eq!(pm.peers.len(), 1);
        assert!(pm.cold_peers.contains(&addr));
        assert_eq!(pm.peers[&addr].source, PeerSource::Config);
        assert!(pm.peers[&addr].is_trustable);
    }

    #[test]
    fn test_peer_lifecycle() {
        let mut pm = PeerManager::new(PeerManagerConfig::default());
        let addr = test_addr(3001);
        pm.add_config_peer(addr, false, false);

        // Connect
        pm.peer_connected(&addr, 14, true);
        assert!(pm.warm_peers.contains(&addr));
        assert!(!pm.cold_peers.contains(&addr));

        // Promote to hot
        pm.promote_to_hot(&addr);
        assert!(pm.hot_peers.contains(&addr));
        assert!(!pm.warm_peers.contains(&addr));

        // Demote to warm
        pm.demote_to_warm(&addr);
        assert!(pm.warm_peers.contains(&addr));
        assert!(!pm.hot_peers.contains(&addr));

        // Disconnect
        pm.peer_disconnected(&addr);
        assert!(pm.cold_peers.contains(&addr));
        assert!(!pm.warm_peers.contains(&addr));
    }

    #[test]
    fn test_peers_to_connect() {
        let config = PeerManagerConfig {
            target_hot_peers: 2,
            target_warm_peers: 2,
            ..PeerManagerConfig::default()
        };
        let mut pm = PeerManager::new(config);

        for i in 0..5 {
            pm.add_config_peer(test_addr(3000 + i), false, false);
        }

        let to_connect = pm.peers_to_connect();
        assert_eq!(to_connect.len(), 4); // target_hot(2) + target_warm(2)
    }

    #[test]
    fn test_peers_to_promote() {
        let config = PeerManagerConfig {
            target_hot_peers: 2,
            ..PeerManagerConfig::default()
        };
        let mut pm = PeerManager::new(config);

        let a1 = test_addr(3001);
        let a2 = test_addr(3002);
        let a3 = test_addr(3003);
        pm.add_config_peer(a1, false, false);
        pm.add_config_peer(a2, false, false);
        pm.add_config_peer(a3, false, false);
        pm.peer_connected(&a1, 14, true);
        pm.peer_connected(&a2, 14, true);
        pm.peer_connected(&a3, 14, true);

        let to_promote = pm.peers_to_promote();
        assert_eq!(to_promote.len(), 2); // target_hot = 2
    }

    #[test]
    fn test_inbound_acceptance() {
        let config = PeerManagerConfig {
            max_inbound_peers: 2,
            ..PeerManagerConfig::default()
        };
        let mut pm = PeerManager::new(config);
        assert!(pm.should_accept_inbound());

        let a1 = test_addr(3001);
        let a2 = test_addr(3002);
        pm.add_config_peer(a1, false, false);
        pm.add_config_peer(a2, false, false);
        pm.peer_connected(&a1, 14, false); // inbound
        assert!(pm.should_accept_inbound());
        pm.peer_connected(&a2, 14, false); // inbound
        assert!(!pm.should_accept_inbound()); // at max
    }

    #[test]
    fn test_initiator_only_rejects_inbound() {
        let config = PeerManagerConfig {
            diffusion_mode: DiffusionMode::InitiatorOnly,
            ..PeerManagerConfig::default()
        };
        let pm = PeerManager::new(config);
        assert!(!pm.should_accept_inbound());
    }

    #[test]
    fn test_peer_sharing() {
        let mut pm = PeerManager::new(PeerManagerConfig::default());
        let a1 = test_addr(3001);
        let a2 = test_addr(3002);
        pm.add_config_peer(a1, false, true); // advertise=true
        pm.add_config_peer(a2, false, false); // advertise=false
        pm.peer_connected(&a1, 14, true);

        let shared = pm.peers_for_sharing(10);
        assert_eq!(shared.len(), 1);
        assert_eq!(shared[0], a1);
    }

    #[test]
    fn test_peer_failure_backoff() {
        let mut pm = PeerManager::new(PeerManagerConfig::default());
        let addr = test_addr(3001);
        pm.add_config_peer(addr, false, false);

        // First failure
        pm.peer_failed(&addr);
        assert!(!pm.peers[&addr].should_retry()); // Just failed, shouldn't retry yet

        // After enough time, should retry
        // (Can't easily test time-based behavior in unit tests without mocking)
    }

    #[test]
    fn test_stats() {
        let mut pm = PeerManager::new(PeerManagerConfig::default());
        let a1 = test_addr(3001);
        let a2 = test_addr(3002);
        let a3 = test_addr(3003);
        pm.add_config_peer(a1, false, false);
        pm.add_config_peer(a2, false, false);
        pm.add_config_peer(a3, false, false);
        pm.peer_connected(&a1, 14, true);
        pm.promote_to_hot(&a1);

        let stats = pm.stats();
        assert_eq!(stats.known_peers, 3);
        assert_eq!(stats.cold_peers, 2);
        assert_eq!(stats.warm_peers, 0);
        assert_eq!(stats.hot_peers, 1);
    }

    #[test]
    fn test_add_shared_peer_dedup() {
        let mut pm = PeerManager::new(PeerManagerConfig::default());
        let addr = test_addr(3001);
        pm.add_config_peer(addr, false, false);
        pm.add_shared_peer(addr); // Already known
        assert_eq!(pm.peers.len(), 1);
    }

    #[test]
    fn test_add_shared_peer_capacity() {
        let config = PeerManagerConfig {
            target_known_peers: 2,
            ..PeerManagerConfig::default()
        };
        let mut pm = PeerManager::new(config);
        pm.add_config_peer(test_addr(3001), false, false);
        pm.add_config_peer(test_addr(3002), false, false);
        pm.add_shared_peer(test_addr(3003)); // At capacity
        assert_eq!(pm.peers.len(), 2);
    }

    #[test]
    fn test_add_ledger_peer() {
        let mut pm = PeerManager::new(PeerManagerConfig::default());
        let addr = test_addr(3001);
        pm.add_ledger_peer(addr);

        assert_eq!(pm.peers.len(), 1);
        assert!(pm.cold_peers.contains(&addr));
        assert_eq!(pm.peers[&addr].source, PeerSource::Ledger);
    }

    #[test]
    fn test_add_ledger_peer_dedup() {
        let mut pm = PeerManager::new(PeerManagerConfig::default());
        let addr = test_addr(3001);
        pm.add_config_peer(addr, false, false);
        pm.add_ledger_peer(addr); // Already known from config
        assert_eq!(pm.peers.len(), 1);
        assert_eq!(pm.peers[&addr].source, PeerSource::Config); // Source unchanged
    }

    #[test]
    fn test_add_ledger_peer_capacity() {
        let config = PeerManagerConfig {
            target_known_peers: 2,
            ..PeerManagerConfig::default()
        };
        let mut pm = PeerManager::new(config);
        pm.add_ledger_peer(test_addr(3001));
        pm.add_ledger_peer(test_addr(3002));
        pm.add_ledger_peer(test_addr(3003)); // At capacity
        assert_eq!(pm.peers.len(), 2);
    }

    #[test]
    fn test_peer_performance_handshake_rtt() {
        let mut perf = PeerPerformance::default();
        perf.record_handshake_rtt(100.0);
        assert_eq!(perf.avg_handshake_rtt_ms, Some(100.0));
        assert_eq!(perf.success_count, 1);

        // EWMA: new avg = 100 * 0.7 + 50 * 0.3 = 85.0
        perf.record_handshake_rtt(50.0);
        assert!((perf.avg_handshake_rtt_ms.unwrap() - 85.0).abs() < 0.1);
    }

    #[test]
    fn test_peer_performance_block_fetch() {
        let mut perf = PeerPerformance::default();
        perf.record_block_fetch(500.0, 10, 1024 * 100);
        // 500ms / 10 blocks = 50ms per block
        assert!((perf.avg_block_fetch_ms.unwrap() - 50.0).abs() < 0.1);
        assert_eq!(perf.blocks_fetched, 10);
        assert_eq!(perf.bytes_received, 102400);
        assert!(perf.last_good_fetch.is_some());
    }

    #[test]
    fn test_reputation_scoring() {
        let mut perf = PeerPerformance::default();
        // Low latency, fast fetches, lots of blocks
        perf.record_handshake_rtt(30.0);
        perf.record_block_fetch(200.0, 100, 1024 * 1024);
        perf.record_block_fetch(180.0, 100, 1024 * 1024);
        let score = perf.compute_reputation(0);
        assert!(
            score > 0.5,
            "Good peer should have above-average reputation: {score}"
        );

        // Poor peer: high latency, slow fetches, failures
        let mut bad_perf = PeerPerformance::default();
        bad_perf.record_handshake_rtt(1500.0);
        bad_perf.record_block_fetch(5000.0, 10, 1024);
        let bad_score = bad_perf.compute_reputation(5);
        assert!(
            bad_score < score,
            "Bad peer should have lower reputation: {bad_score} < {score}"
        );
    }

    #[test]
    fn test_selection_score_trustable_bonus() {
        let mut pm = PeerManager::new(PeerManagerConfig::default());
        let trusted = test_addr(3001);
        let untrusted = test_addr(3002);
        pm.add_config_peer(trusted, true, false);
        pm.add_config_peer(untrusted, false, false);

        let trusted_score = pm.peers[&trusted].selection_score();
        let untrusted_score = pm.peers[&untrusted].selection_score();
        assert!(
            trusted_score > untrusted_score,
            "Trustable peer should rank higher: {trusted_score} > {untrusted_score}"
        );
    }

    #[test]
    fn test_ranked_peer_selection() {
        let config = PeerManagerConfig {
            target_hot_peers: 1,
            target_warm_peers: 1,
            ..PeerManagerConfig::default()
        };
        let mut pm = PeerManager::new(config);

        // Add a trustable config peer and a regular peer
        let trustable = test_addr(3001);
        let regular = test_addr(3002);
        pm.add_config_peer(trustable, true, false);
        pm.add_config_peer(regular, false, false);

        let to_connect = pm.peers_to_connect();
        assert_eq!(to_connect.len(), 2);
        // Trustable peer should be first (higher score)
        assert_eq!(to_connect[0], trustable);
    }

    #[test]
    fn test_ranked_promotion() {
        let config = PeerManagerConfig {
            target_hot_peers: 1,
            ..PeerManagerConfig::default()
        };
        let mut pm = PeerManager::new(config);

        let fast_peer = test_addr(3001);
        let slow_peer = test_addr(3002);
        pm.add_config_peer(fast_peer, true, false);
        pm.add_config_peer(slow_peer, false, false);
        pm.peer_connected(&fast_peer, 14, true);
        pm.peer_connected(&slow_peer, 14, true);

        // Give fast peer better latency
        pm.record_handshake_rtt(&fast_peer, 20.0);
        pm.record_handshake_rtt(&slow_peer, 800.0);
        pm.recompute_reputations();

        let to_promote = pm.peers_to_promote();
        assert_eq!(to_promote.len(), 1);
        assert_eq!(to_promote[0], fast_peer);
    }

    #[test]
    fn test_best_peers_for_fetch() {
        let mut pm = PeerManager::new(PeerManagerConfig::default());
        let fast = test_addr(3001);
        let slow = test_addr(3002);
        pm.add_config_peer(fast, true, false);
        pm.add_config_peer(slow, false, false);
        pm.peer_connected(&fast, 14, true);
        pm.peer_connected(&slow, 14, true);
        pm.promote_to_hot(&fast);
        pm.promote_to_hot(&slow);

        pm.record_block_fetch(&fast, 100.0, 50, 50000);
        pm.record_block_fetch(&slow, 2000.0, 50, 50000);
        pm.recompute_reputations();

        let best = pm.best_peers_for_fetch(2);
        assert_eq!(best.len(), 2);
        assert_eq!(best[0], fast);
    }

    #[test]
    fn test_worst_hot_peer() {
        let mut pm = PeerManager::new(PeerManagerConfig::default());
        let good = test_addr(3001);
        let bad = test_addr(3002);
        pm.add_config_peer(good, true, false);
        pm.add_config_peer(bad, false, false);
        pm.peer_connected(&good, 14, true);
        pm.peer_connected(&bad, 14, true);
        pm.promote_to_hot(&good);
        pm.promote_to_hot(&bad);

        pm.record_handshake_rtt(&good, 20.0);
        pm.record_handshake_rtt(&bad, 1500.0);
        pm.recompute_reputations();

        let worst = pm.worst_hot_peer();
        assert_eq!(worst, Some(bad));
    }

    #[test]
    fn test_stats_includes_performance() {
        let mut pm = PeerManager::new(PeerManagerConfig::default());
        let addr = test_addr(3001);
        pm.add_config_peer(addr, false, false);
        pm.peer_connected(&addr, 14, true);
        pm.promote_to_hot(&addr);
        pm.record_block_fetch(&addr, 100.0, 10, 10000);
        pm.recompute_reputations();

        let stats = pm.stats();
        assert!(stats.avg_hot_reputation > 0.0);
        assert!(stats.best_fetch_latency_ms.is_some());
    }
}
