//! Node-level networking orchestration.
//!
//! This module bridges the new torsten-network crate's protocol primitives
//! with the node's high-level connection management. It defines the types
//! and orchestration logic that are specific to the node's needs (topology
//! management, peer lifecycle, block fetch coordination, N2C server, etc.)
//! without polluting the network protocol crate with node concerns.
//!
//! ## Architecture
//! ```text
//! torsten-network (protocol primitives)
//!   ├── Bearer (TCP/Unix transport)
//!   ├── Mux (multiplexer)
//!   ├── Protocols (ChainSync, BlockFetch, etc.)
//!   └── PeerManager (basic cold/warm/hot lifecycle)
//!
//! torsten-node::networking (this module, node-level orchestration)
//!   ├── NodePeerManager (wraps PeerManager + connection tracking)
//!   ├── NodeServer (TCP/Unix listener orchestration)
//!   ├── PeerConnection (per-peer protocol bundle)
//!   └── SyncClient (pipelined ChainSync + BlockFetch coordination)
//! ```

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use torsten_network::{PeerManager, PeerSource, PeerState};

// ─── Configuration Types ─────────────────────────────────────────────────────

/// Diffusion mode — whether the node accepts inbound connections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // InitiatorOnly variant for networking rewrite
pub enum DiffusionMode {
    /// Only initiate outbound connections (relay behind NAT).
    InitiatorOnly,
    /// Both initiate outbound and accept inbound connections.
    InitiatorAndResponder,
}

/// Network timeout configuration.
#[derive(Debug, Clone)]
#[allow(dead_code)] // used by networking rewrite
pub struct TimeoutConfig {
    /// Timeout for TCP connection establishment.
    pub connect_timeout: Duration,
    /// Timeout for handshake negotiation.
    pub handshake_timeout: Duration,
    /// KeepAlive ping interval.
    pub keepalive_interval: Duration,
    /// Timeout before closing an idle connection at tip.
    pub await_reply_timeout: Duration,
}

impl Default for TimeoutConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            handshake_timeout: Duration::from_secs(30),
            keepalive_interval: Duration::from_secs(30),
            await_reply_timeout: Duration::from_secs(135),
        }
    }
}

/// Configuration for the node's peer management.
#[derive(Debug, Clone)]
#[allow(dead_code)] // fields used by networking rewrite
pub struct PeerManagerConfig {
    /// Diffusion mode (InitiatorOnly or InitiatorAndResponder).
    pub diffusion_mode: DiffusionMode,
    /// Whether peer sharing is enabled.
    pub peer_sharing_enabled: bool,
    /// Target number of hot (active) peers.
    pub target_hot_peers: usize,
    /// Target number of warm (established but not active) peers.
    pub target_warm_peers: usize,
    /// Target number of known (cold + warm + hot) peers.
    pub target_known_peers: usize,
    /// Network magic for handshake validation.
    pub network_magic: u64,
}

impl Default for PeerManagerConfig {
    fn default() -> Self {
        Self {
            diffusion_mode: DiffusionMode::InitiatorAndResponder,
            peer_sharing_enabled: true,
            target_hot_peers: 5,
            target_warm_peers: 10,
            target_known_peers: 100,
            network_magic: 2,
        }
    }
}

/// Direction of a peer connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionDirection {
    Outbound,
    Inbound,
}

/// Category of a peer for big ledger peer tracking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // used by networking rewrite
pub enum PeerCategory {
    Normal,
    BigLedgerPeer,
    LocalRoot,
}

/// A local root peer group from the topology configuration.
#[derive(Debug, Clone)]
#[allow(dead_code)] // fields used by networking rewrite
pub struct LocalRootGroupInfo {
    /// Name of the group (for logging/display).
    pub name: String,
    /// Resolved addresses of peers in this group.
    pub addrs: Vec<SocketAddr>,
    /// Target number of hot peers in this group.
    pub hot_valency: usize,
    /// Target number of warm peers in this group.
    pub warm_valency: usize,
}

// ─── Announcement Types ──────────────────────────────────────────────────────

/// Rollback announcement sent via broadcast channel when the chain rolls back.
#[derive(Debug, Clone)]
#[allow(dead_code)] // fields used by networking rewrite
pub struct RollbackAnnouncement {
    /// The point to roll back to.
    pub slot: u64,
    pub hash: [u8; 32],
}

// ─── Sync Client Types ───────────────────────────────────────────────────────

/// Information about a Byron Epoch Boundary Block (EBB).
///
/// Byron EBBs share a slot with the first block of the epoch and need
/// special handling during sync.
#[derive(Debug, Clone)]
#[allow(dead_code)] // used by networking rewrite
pub struct EbbInfo {
    /// Slot of the EBB (same as the first block of the epoch).
    pub slot: u64,
    /// Hash of the EBB.
    pub hash: [u8; 32],
    /// Epoch number this EBB marks the boundary of.
    pub epoch: u64,
}

/// Result from a pipelined header batch request.
#[derive(Debug)]
#[allow(dead_code)] // used by networking rewrite
pub enum HeaderBatchResult {
    /// A batch of headers was received.
    Headers(Vec<HeaderInfo>),
    /// The chain rolled backward to a point.
    RollBack { slot: u64, hash: [u8; 32] },
    /// We're at the chain tip — waiting for new blocks.
    Await,
}

/// Information about a received block header.
#[derive(Debug, Clone)]
#[allow(dead_code)] // used by networking rewrite
pub struct HeaderInfo {
    /// Raw header CBOR bytes.
    pub header: Vec<u8>,
    /// Slot number.
    pub slot: u64,
    /// Block header hash.
    pub hash: [u8; 32],
    /// Block number (height).
    pub block_number: u64,
    /// Tip slot reported by the server.
    pub tip_slot: u64,
}

// ─── Error Types ─────────────────────────────────────────────────────────────

/// Errors from N2N client operations.
#[derive(Debug)]
#[allow(dead_code)] // used by networking rewrite
pub enum ClientError {
    /// TCP connection failed.
    Connection(String),
    /// Handshake negotiation failed.
    Handshake(String),
    /// Protocol error during operation.
    Protocol(String),
    /// Connection timed out.
    Timeout,
    /// Connection was closed by the remote peer.
    Closed,
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connection(e) => write!(f, "connection: {e}"),
            Self::Handshake(e) => write!(f, "handshake: {e}"),
            Self::Protocol(e) => write!(f, "protocol: {e}"),
            Self::Timeout => write!(f, "timeout"),
            Self::Closed => write!(f, "connection closed"),
        }
    }
}

impl std::error::Error for ClientError {}

/// Errors from duplex peer connection operations.
#[derive(Debug)]
#[allow(dead_code)] // used by networking rewrite
pub enum DuplexError {
    /// The underlying client connection failed.
    Connection(ClientError),
    /// The peer was disconnected.
    Disconnected,
}

impl std::fmt::Display for DuplexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connection(e) => write!(f, "duplex: {e}"),
            Self::Disconnected => write!(f, "duplex: peer disconnected"),
        }
    }
}

impl std::error::Error for DuplexError {}

// ─── Node Peer Manager ───────────────────────────────────────────────────────

/// Node-level peer manager wrapping the network crate's PeerManager.
///
/// Adds connection direction tracking, big ledger peer classification,
/// local root group management, diffusion mode, rate limiting, and
/// other node-specific peer management concerns.
pub struct NodePeerManager {
    /// The underlying protocol-level peer manager.
    pub inner: PeerManager,
    /// Configuration.
    pub config: PeerManagerConfig,
    /// Connection direction tracking for connected peers.
    connections: HashMap<SocketAddr, ConnectionDirection>,
    /// Whether each connection is duplex.
    duplex: HashMap<SocketAddr, bool>,
    /// Last connection attempt times (for rate limiting).
    last_attempt: HashMap<SocketAddr, Instant>,
    /// Our own listen address (to prevent self-connections).
    local_addr: Option<SocketAddr>,
    /// Configured local root peer groups.
    local_root_groups: Vec<LocalRootGroupInfo>,
    /// Set of big ledger peer addresses.
    big_ledger_peers: std::collections::HashSet<SocketAddr>,
}

impl NodePeerManager {
    /// Create a new node peer manager with the given configuration.
    pub fn new(config: PeerManagerConfig) -> Self {
        Self {
            inner: PeerManager::new(),
            config,
            connections: HashMap::new(),
            duplex: HashMap::new(),
            last_attempt: HashMap::new(),
            local_addr: None,
            local_root_groups: Vec::new(),
            big_ledger_peers: std::collections::HashSet::new(),
        }
    }

    /// Set our own listen address.
    pub fn set_local_addr(&mut self, addr: SocketAddr) {
        self.local_addr = Some(addr);
    }

    /// Get the diffusion mode.
    pub fn diffusion_mode(&self) -> DiffusionMode {
        self.config.diffusion_mode
    }

    /// Add a peer from the topology configuration.
    pub fn add_config_peer(&mut self, addr: SocketAddr) {
        if self.local_addr == Some(addr) {
            return;
        }
        self.inner.add_peer(addr, PeerSource::Topology);
    }

    /// Add a local root peer group.
    pub fn add_local_root_group(&mut self, group: LocalRootGroupInfo) {
        for &addr in &group.addrs {
            self.add_config_peer(addr);
        }
        self.local_root_groups.push(group);
    }

    /// Get the configured local root groups.
    pub fn local_root_groups(&self) -> &[LocalRootGroupInfo] {
        &self.local_root_groups
    }

    /// Add a peer discovered from ledger state.
    pub fn add_ledger_peer(&mut self, addr: SocketAddr) {
        if self.local_addr == Some(addr) {
            return;
        }
        self.inner.add_peer(addr, PeerSource::Ledger);
    }

    /// Add a peer received via PeerSharing.
    #[allow(dead_code)] // used by networking rewrite
    pub fn add_shared_peer(&mut self, addr: SocketAddr) {
        if self.local_addr == Some(addr) {
            return;
        }
        if addr.ip().is_loopback() || addr.ip().is_unspecified() {
            return;
        }
        self.inner.add_peer(addr, PeerSource::PeerSharing);
    }

    /// Mark a peer as a big ledger peer.
    pub fn add_big_ledger_peer(&mut self, addr: SocketAddr) {
        self.big_ledger_peers.insert(addr);
    }

    /// Mark a peer as connected.
    pub fn peer_connected(&mut self, addr: &SocketAddr, direction: ConnectionDirection) {
        if !self.inner.get_peer(addr).is_some() {
            let source = match direction {
                ConnectionDirection::Inbound => PeerSource::PeerSharing,
                ConnectionDirection::Outbound => PeerSource::Topology,
            };
            self.inner.add_peer(*addr, source);
        }
        self.inner.promote_to_warm(addr);
        self.connections.insert(*addr, direction);
        self.duplex.insert(*addr, false);
    }

    /// Mark a peer as disconnected.
    pub fn peer_disconnected(&mut self, addr: &SocketAddr) {
        self.inner.demote_to_cold(addr);
        self.connections.remove(addr);
        self.duplex.remove(addr);
    }

    /// Record a connection failure.
    pub fn peer_failed(&mut self, addr: &SocketAddr) {
        if let Some(peer) = self.inner.get_peer_mut(addr) {
            peer.record_failure();
        }
        self.inner.demote_to_cold(addr);
        self.connections.remove(addr);
        self.duplex.remove(addr);
        self.last_attempt.insert(*addr, Instant::now());
    }

    /// Mark a connection as duplex.
    #[allow(dead_code)] // used by networking rewrite
    pub fn mark_peer_duplex(&mut self, addr: &SocketAddr) {
        self.duplex.insert(*addr, true);
    }

    /// Record a handshake RTT measurement.
    #[allow(dead_code)] // used by networking rewrite
    pub fn record_handshake_rtt(&mut self, addr: &SocketAddr, rtt_ms: f64) {
        if let Some(peer) = self.inner.get_peer_mut(addr) {
            peer.update_latency(rtt_ms);
        }
    }

    /// Record blocks fetched from a peer.
    #[allow(dead_code)] // used by networking rewrite
    pub fn record_block_fetch(&mut self, addr: &SocketAddr, blocks: usize) {
        if let Some(peer) = self.inner.get_peer_mut(addr) {
            peer.record_success();
            let _ = blocks; // future: track per-peer block counts
        }
    }

    /// Recompute reputation scores for all peers.
    pub fn recompute_reputations(&mut self) {
        self.inner.decay_all_failures();
    }

    // ─── Counting ───

    pub fn cold_peer_count(&self) -> usize {
        self.inner.count_by_state(PeerState::Cold)
    }
    pub fn warm_peer_count(&self) -> usize {
        self.inner.count_by_state(PeerState::Warm)
    }
    pub fn hot_peer_count(&self) -> usize {
        self.inner.count_by_state(PeerState::Hot)
    }
    pub fn outbound_peer_count(&self) -> usize {
        self.connections
            .values()
            .filter(|d| **d == ConnectionDirection::Outbound)
            .count()
    }
    pub fn inbound_peer_count(&self) -> usize {
        self.connections
            .values()
            .filter(|d| **d == ConnectionDirection::Inbound)
            .count()
    }
    pub fn duplex_peer_count(&self) -> usize {
        self.duplex.values().filter(|&&d| d).count()
    }
    pub fn active_big_ledger_peer_count(&self) -> usize {
        self.big_ledger_peers
            .iter()
            .filter(|addr| {
                self.inner
                    .get_peer(addr)
                    .is_some_and(|p| p.state != PeerState::Cold)
            })
            .count()
    }

    /// Get connected peer addresses.
    pub fn connected_peer_addrs(&self) -> Vec<SocketAddr> {
        self.connections.keys().copied().collect()
    }

    /// Get the category of a peer.
    #[allow(dead_code)] // used by networking rewrite
    pub fn peer_category(&self, addr: &SocketAddr) -> Option<PeerCategory> {
        if !self.inner.get_peer(addr).is_some() {
            return None;
        }
        for group in &self.local_root_groups {
            if group.addrs.contains(addr) {
                return Some(PeerCategory::LocalRoot);
            }
        }
        if self.big_ledger_peers.contains(addr) {
            return Some(PeerCategory::BigLedgerPeer);
        }
        Some(PeerCategory::Normal)
    }

    /// Find an inbound duplex connection from the same IP.
    #[allow(dead_code)] // used by networking rewrite
    pub fn find_inbound_duplex_by_ip(&self, ip: std::net::IpAddr) -> Option<SocketAddr> {
        self.connections
            .iter()
            .find(|(addr, dir)| {
                addr.ip() == ip
                    && **dir == ConnectionDirection::Inbound
                    && self.duplex.get(addr).copied().unwrap_or(false)
            })
            .map(|(addr, _)| *addr)
    }

    /// Check whether a connection attempt should be made to this peer.
    #[allow(dead_code)] // used by networking rewrite
    pub fn should_attempt_connection(&self, addr: &SocketAddr) -> bool {
        if let Some(last) = self.last_attempt.get(addr) {
            if last.elapsed() < Duration::from_secs(30) {
                return false;
            }
        }
        true
    }

    /// Get peers eligible for new outbound connections.
    #[allow(dead_code)] // used by networking rewrite
    pub fn peers_to_connect(&self, count: usize) -> Vec<SocketAddr> {
        self.inner
            .peers_in_state(PeerState::Cold)
            .into_iter()
            .filter(|addr| self.should_attempt_connection(addr))
            .filter(|addr| self.local_addr.as_ref() != Some(addr))
            .take(count)
            .collect()
    }

    /// Summary statistics.
    pub fn stats(&self) -> PeerManagerStats {
        PeerManagerStats {
            cold: self.cold_peer_count(),
            warm: self.warm_peer_count(),
            hot: self.hot_peer_count(),
            outbound: self.outbound_peer_count(),
            inbound: self.inbound_peer_count(),
            duplex: self.duplex_peer_count(),
            big_ledger: self.active_big_ledger_peer_count(),
        }
    }
}

/// Summary statistics from the node peer manager.
#[derive(Debug, Clone, Default)]
pub struct PeerManagerStats {
    pub cold: usize,
    pub warm: usize,
    pub hot: usize,
    pub outbound: usize,
    pub inbound: usize,
    pub duplex: usize,
    pub big_ledger: usize,
}

impl std::fmt::Display for PeerManagerStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "cold={} warm={} hot={} out={} in={} duplex={} blp={}",
            self.cold,
            self.warm,
            self.hot,
            self.outbound,
            self.inbound,
            self.duplex,
            self.big_ledger,
        )
    }
}
