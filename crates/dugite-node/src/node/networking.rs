//! Node-level networking orchestration.
//!
//! This module bridges the new dugite-network crate's protocol primitives
//! with the node's high-level connection management. It defines the types
//! and orchestration logic that are specific to the node's needs (topology
//! management, peer lifecycle, block fetch coordination, N2C server, etc.)
//! without polluting the network protocol crate with node concerns.
//!
//! ## Architecture
//! ```text
//! dugite-network (protocol primitives)
//!   ├── Bearer (TCP/Unix transport)
//!   ├── Mux (multiplexer)
//!   ├── Protocols (ChainSync, BlockFetch, etc.)
//!   └── PeerManager (basic cold/warm/hot lifecycle)
//!
//! dugite-node::networking (this module, node-level orchestration)
//!   ├── NodePeerManager (wraps PeerManager + connection tracking)
//!   ├── NodeServer (TCP/Unix listener orchestration)
//!   ├── PeerConnection (per-peer protocol bundle)
//!   └── SyncClient (pipelined ChainSync + BlockFetch coordination)
//! ```

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;

use dugite_network::connection::state::{
    ConnectionManagerCounters, ConnectionState, DataFlow, Provenance,
};
use dugite_network::peer::manager::MAX_COLD_PEER_FAILURES;
use dugite_network::{PeerManager, PeerSource, PeerState};

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
    /// Per-group diffusion mode override. None = use node-level default.
    pub diffusion_mode: Option<DiffusionMode>,
    /// Whether peers in this group are behind a firewall (inbound-only).
    pub behind_firewall: bool,
    /// Whether peers in this group can be shared via the peer sharing protocol.
    pub advertise: bool,
}

// ─── Announcement Types ──────────────────────────────────────────────────────

// RollbackAnnouncement is defined in dugite-network alongside BlockAnnouncement
// and re-exported as `dugite_network::RollbackAnnouncement`.  The node crate
// uses that type directly for broadcast channels shared between sync and the
// ChainSync/LocalChainSync servers.

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
/// Adds connection state tracking (matching Haskell's `ConnectionManager`),
/// big ledger peer classification, local root group management, diffusion
/// mode, rate limiting, and other node-specific peer management concerns.
///
/// Each connection is tracked via a `ConnectionState` from the network crate,
/// enabling correct `ConnectionManagerCounters` computation that matches
/// the Haskell `connectionStateToCounters` behaviour.
pub struct NodePeerManager {
    /// The underlying protocol-level peer manager.
    pub inner: PeerManager,
    /// Configuration.
    pub config: PeerManagerConfig,
    /// Per-connection state machine (Haskell ConnectionManager state).
    ///
    /// Tracks the lifecycle state of each connection. Used to compute
    /// `ConnectionManagerCounters` via `to_counters()`.
    conn_states: HashMap<SocketAddr, ConnectionState>,
    /// Our own listen address (to prevent self-connections).
    local_addr: Option<SocketAddr>,
    /// Configured local root peer groups.
    local_root_groups: Vec<LocalRootGroupInfo>,
    /// Set of big ledger peer addresses.
    big_ledger_peers: std::collections::HashSet<SocketAddr>,
    /// Optional GSM event sender — when set, `peer_disconnected()` emits
    /// `GsmEvent::PeerDisconnected` so the GSM actor can update its peer
    /// tracking (LoE, GDD). `None` when GSM is not wired (e.g. in tests).
    gsm_event_tx: Option<tokio::sync::mpsc::Sender<crate::gsm::GsmEvent>>,
}

impl NodePeerManager {
    /// Create a new node peer manager with the given configuration.
    pub fn new(config: PeerManagerConfig) -> Self {
        Self {
            inner: PeerManager::new(),
            config,
            conn_states: HashMap::new(),
            local_addr: None,
            local_root_groups: Vec::new(),
            big_ledger_peers: std::collections::HashSet::new(),
            gsm_event_tx: None,
        }
    }

    /// Set the GSM event sender so `peer_disconnected()` emits events.
    pub fn set_gsm_event_tx(&mut self, tx: tokio::sync::mpsc::Sender<crate::gsm::GsmEvent>) {
        self.gsm_event_tx = Some(tx);
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
    ///
    /// Sets the connection state to `OutboundIdle(Duplex)` or
    /// `InboundIdle(Duplex)` depending on direction. All N2N P2P
    /// connections negotiate `Duplex` data flow.
    pub fn peer_connected(&mut self, addr: &SocketAddr, direction: ConnectionDirection) {
        if self.inner.get_peer(addr).is_none() {
            let source = match direction {
                ConnectionDirection::Inbound => PeerSource::PeerSharing,
                ConnectionDirection::Outbound => PeerSource::Topology,
            };
            self.inner.add_peer(*addr, source);
        }
        self.inner.promote_to_warm(addr);
        let state = match direction {
            ConnectionDirection::Outbound => ConnectionState::OutboundIdle(DataFlow::Duplex),
            ConnectionDirection::Inbound => ConnectionState::InboundIdle(DataFlow::Duplex),
        };
        self.conn_states.insert(*addr, state);
    }

    /// Mark a peer as disconnected.
    ///
    /// Demotes the peer to cold and removes connection state. Also emits
    /// `GsmEvent::PeerDisconnected` to the GSM actor (if wired) so that
    /// the LoE and GDD peer tracking are updated.
    pub fn peer_disconnected(&mut self, addr: &SocketAddr) {
        self.inner.demote_to_cold(addr);
        self.conn_states.remove(addr);
        // Notify the GSM actor so it can deregister the peer from density tracking.
        if let Some(ref tx) = self.gsm_event_tx {
            if let Err(e) = tx.try_send(crate::gsm::GsmEvent::PeerDisconnected { addr: *addr }) {
                tracing::debug!(%addr, "GSM PeerDisconnected event dropped: {e}");
            }
        }
    }

    /// Record a connection failure.
    ///
    /// Applies exponential backoff via `PeerInfo::record_failure()`. For
    /// non-root peers (Ledger, PeerSharing), permanently removes the peer
    /// after `MAX_COLD_PEER_FAILURES` consecutive failures, matching Haskell's
    /// `policyMaxConnectionRetries = 5` forget policy.
    ///
    /// Topology and Dns peers are never forgotten — they are retried
    /// indefinitely at the 160s backoff cap (matching Haskell local/public
    /// root peer behaviour).
    pub fn peer_failed(&mut self, addr: &SocketAddr) {
        // Check whether this failure pushes the peer over the forget threshold.
        // We read failure_count *before* calling record_failure() (+1 below).
        let should_forget = self.inner.get_peer(addr).is_some_and(|p| {
            p.failure_count + 1 >= MAX_COLD_PEER_FAILURES
                && !matches!(p.source, PeerSource::Topology | PeerSource::Dns)
        });

        if let Some(peer) = self.inner.get_peer_mut(addr) {
            peer.record_failure();
        }

        self.conn_states.remove(addr);

        if should_forget {
            // Non-root peer exceeded max retries — remove from known set entirely.
            // It will only re-appear if re-discovered via ledger or peer sharing.
            self.inner.remove_peer(addr);
        } else {
            self.inner.demote_to_cold(addr);
        }
    }

    /// Mark a connection as duplex (both initiator and responder active).
    ///
    /// Called during simultaneous open detection — an inbound connection arrives
    /// while we already have an outbound connection to the same peer. The
    /// connection transitions to `DuplexConn` matching Haskell's `DuplexState`.
    #[allow(dead_code)] // Will be used when full simultaneous-open handling is implemented
    pub fn mark_peer_duplex(&mut self, addr: &SocketAddr) {
        if self.conn_states.contains_key(addr) {
            self.conn_states.insert(*addr, ConnectionState::DuplexConn);
        }
    }

    /// Transition an outbound idle connection to active (initiator protocols running).
    ///
    /// Called when promoting warm → hot on an outbound connection.
    /// `OutboundIdle(Duplex)` → `OutboundDup`, `OutboundIdle(Unidirectional)` → `OutboundUni`.
    pub fn mark_outbound_active(&mut self, addr: &SocketAddr) {
        if let Some(state) = self.conn_states.get(addr) {
            let new_state = match state {
                ConnectionState::OutboundIdle(DataFlow::Duplex) => ConnectionState::OutboundDup,
                ConnectionState::OutboundIdle(DataFlow::Unidirectional) => {
                    ConnectionState::OutboundUni
                }
                // Already active or duplex — leave unchanged.
                _ => return,
            };
            self.conn_states.insert(*addr, new_state);
        }
    }

    /// Transition an active outbound connection back to idle.
    ///
    /// Called when demoting hot → warm on an outbound connection.
    /// `OutboundDup` → `OutboundIdle(Duplex)`, `OutboundUni` → `OutboundIdle(Unidirectional)`.
    pub fn mark_outbound_idle(&mut self, addr: &SocketAddr) {
        if let Some(state) = self.conn_states.get(addr) {
            let new_state = match state {
                ConnectionState::OutboundDup => ConnectionState::OutboundIdle(DataFlow::Duplex),
                ConnectionState::OutboundUni => {
                    ConnectionState::OutboundIdle(DataFlow::Unidirectional)
                }
                _ => return,
            };
            self.conn_states.insert(*addr, new_state);
        }
    }

    /// Transition an inbound idle connection to active (responder protocols running).
    ///
    /// Called when promoting warm → hot on an inbound connection.
    /// `InboundIdle(df)` → `InboundState(df)`.
    pub fn mark_inbound_active(&mut self, addr: &SocketAddr) {
        if let Some(state) = self.conn_states.get(addr) {
            let new_state = match state {
                ConnectionState::InboundIdle(df) => ConnectionState::InboundState(*df),
                _ => return,
            };
            self.conn_states.insert(*addr, new_state);
        }
    }

    /// Transition an active inbound connection back to idle.
    ///
    /// Called when demoting hot → warm on an inbound connection.
    /// `InboundState(df)` → `InboundIdle(df)`.
    pub fn mark_inbound_idle(&mut self, addr: &SocketAddr) {
        if let Some(state) = self.conn_states.get(addr) {
            let new_state = match state {
                ConnectionState::InboundState(df) => ConnectionState::InboundIdle(*df),
                _ => return,
            };
            self.conn_states.insert(*addr, new_state);
        }
    }

    /// Transition a connection to terminating state.
    ///
    /// Called before `conn.shutdown()` during demotion to cold or cleanup.
    /// The connection will be removed via `peer_disconnected()` after shutdown.
    pub fn mark_terminating(&mut self, addr: &SocketAddr) {
        if self.conn_states.contains_key(addr) {
            self.conn_states
                .insert(*addr, ConnectionState::TerminatingConn);
        }
    }

    /// Check if a connection is inbound (for directing state transitions).
    pub fn is_inbound(&self, addr: &SocketAddr) -> bool {
        self.conn_states
            .get(addr)
            .and_then(|s| s.provenance())
            .is_some_and(|p| p == Provenance::Inbound)
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

    /// Collect current EWMA latency values (ms) for all connected peers
    /// (warm or hot) that have at least one RTT measurement.
    pub fn connected_peer_latencies(&self) -> Vec<f64> {
        use dugite_network::peer::PeerState;
        self.inner
            .peers_in_state(PeerState::Warm)
            .iter()
            .chain(self.inner.peers_in_state(PeerState::Hot).iter())
            .filter_map(|addr| self.inner.get_peer(addr).and_then(|p| p.latency_ms))
            .collect()
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
    /// Count outbound connections (including DuplexConn, which counts as both).
    pub fn outbound_peer_count(&self) -> usize {
        self.conn_states
            .values()
            .filter(|s| {
                matches!(
                    s,
                    ConnectionState::OutboundIdle(_)
                        | ConnectionState::OutboundUni
                        | ConnectionState::OutboundDup
                        | ConnectionState::DuplexConn
                )
            })
            .count()
    }
    /// Count inbound connections (including DuplexConn, which counts as both).
    pub fn inbound_peer_count(&self) -> usize {
        self.conn_states
            .values()
            .filter(|s| {
                matches!(
                    s,
                    ConnectionState::InboundIdle(_)
                        | ConnectionState::InboundState(_)
                        | ConnectionState::DuplexConn
                )
            })
            .count()
    }
    /// Count duplex connections (negotiated Duplex DataFlow or in DuplexConn state).
    pub fn duplex_peer_count(&self) -> usize {
        self.conn_states
            .values()
            .filter(|s| s.data_flow() == Some(DataFlow::Duplex))
            .count()
    }

    /// Compute aggregated connection manager counters matching Haskell's
    /// `ConnectionManagerCounters`.
    pub fn connection_manager_counters(&self) -> ConnectionManagerCounters {
        self.conn_states.values().map(|s| s.to_counters()).sum()
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
        self.conn_states.keys().copied().collect()
    }

    /// Get the category of a peer.
    #[allow(dead_code)] // used by networking rewrite
    pub fn peer_category(&self, addr: &SocketAddr) -> Option<PeerCategory> {
        self.inner.get_peer(addr)?;
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
        self.conn_states
            .iter()
            .find(|(addr, state)| {
                addr.ip() == ip
                    && matches!(
                        state,
                        ConnectionState::InboundIdle(DataFlow::Duplex)
                            | ConnectionState::InboundState(DataFlow::Duplex)
                            | ConnectionState::DuplexConn
                    )
            })
            .map(|(addr, _)| *addr)
    }

    /// Get the effective diffusion mode for a specific peer.
    ///
    /// If the peer belongs to a local root group with an explicit diffusion mode,
    /// that override is used. Otherwise, falls back to the node-level config.
    #[allow(dead_code)] // will be used by P2P governor handshake logic
    pub fn effective_diffusion_mode(&self, addr: &SocketAddr) -> DiffusionMode {
        for group in &self.local_root_groups {
            if group.addrs.contains(addr) {
                if let Some(mode) = group.diffusion_mode {
                    return mode;
                }
            }
        }
        self.config.diffusion_mode
    }

    /// Whether a peer is behind a firewall (should not initiate outbound connections).
    #[allow(dead_code)] // will be used by P2P governor connection logic
    pub fn is_behind_firewall(&self, addr: &SocketAddr) -> bool {
        self.local_root_groups
            .iter()
            .any(|g| g.behind_firewall && g.addrs.contains(addr))
    }

    /// Whether a peer can be shared via the PeerSharing protocol.
    /// Returns false for peers in local root groups with advertise=false.
    #[allow(dead_code)] // will be used by PeerSharing protocol
    pub fn is_advertisable(&self, addr: &SocketAddr) -> bool {
        for group in &self.local_root_groups {
            if group.addrs.contains(addr) {
                return group.advertise;
            }
        }
        true // non-topology peers are advertisable by default
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;

    #[test]
    fn test_effective_diffusion_mode_per_group() {
        let mut pm = NodePeerManager::new(PeerManagerConfig {
            diffusion_mode: DiffusionMode::InitiatorAndResponder,
            ..PeerManagerConfig::default()
        });

        let relay: SocketAddr = "1.2.3.4:3001".parse().unwrap();
        let bp_relay: SocketAddr = "10.0.0.1:3001".parse().unwrap();
        let unknown: SocketAddr = "8.8.8.8:3001".parse().unwrap();

        pm.add_local_root_group(LocalRootGroupInfo {
            name: "relays".into(),
            addrs: vec![relay],
            hot_valency: 1,
            warm_valency: 1,
            diffusion_mode: None,
            behind_firewall: false,
            advertise: true,
        });
        pm.add_local_root_group(LocalRootGroupInfo {
            name: "bp-relays".into(),
            addrs: vec![bp_relay],
            hot_valency: 1,
            warm_valency: 1,
            diffusion_mode: Some(DiffusionMode::InitiatorOnly),
            behind_firewall: false,
            advertise: false,
        });

        // Relay inherits node-level default.
        assert_eq!(
            pm.effective_diffusion_mode(&relay),
            DiffusionMode::InitiatorAndResponder
        );
        // BP relay uses per-group override.
        assert_eq!(
            pm.effective_diffusion_mode(&bp_relay),
            DiffusionMode::InitiatorOnly
        );
        // Unknown peer falls back to node-level.
        assert_eq!(
            pm.effective_diffusion_mode(&unknown),
            DiffusionMode::InitiatorAndResponder
        );
    }

    #[test]
    fn test_behind_firewall_and_advertise() {
        let mut pm = NodePeerManager::new(PeerManagerConfig::default());
        let fw_addr: SocketAddr = "10.0.0.1:3001".parse().unwrap();
        let normal_addr: SocketAddr = "1.2.3.4:3001".parse().unwrap();
        let unknown_addr: SocketAddr = "8.8.8.8:3001".parse().unwrap();

        pm.add_local_root_group(LocalRootGroupInfo {
            name: "firewall-group".into(),
            addrs: vec![fw_addr],
            hot_valency: 1,
            warm_valency: 1,
            diffusion_mode: None,
            behind_firewall: true,
            advertise: false,
        });
        pm.add_local_root_group(LocalRootGroupInfo {
            name: "normal-group".into(),
            addrs: vec![normal_addr],
            hot_valency: 1,
            warm_valency: 1,
            diffusion_mode: None,
            behind_firewall: false,
            advertise: true,
        });

        assert!(pm.is_behind_firewall(&fw_addr));
        assert!(!pm.is_behind_firewall(&normal_addr));
        assert!(!pm.is_behind_firewall(&unknown_addr));

        assert!(!pm.is_advertisable(&fw_addr));
        assert!(pm.is_advertisable(&normal_addr));
        assert!(pm.is_advertisable(&unknown_addr)); // unknown defaults to true
    }
}
