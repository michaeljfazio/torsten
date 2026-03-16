use std::net::SocketAddr;
use std::time::Instant;
use torsten_primitives::block::Tip;

/// Peer connection state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerState {
    /// Connection not yet established
    Connecting,
    /// Handshake in progress
    Handshaking,
    /// Fully connected and operational
    Connected,
    /// Being disconnected
    Disconnecting,
    /// Disconnected
    Disconnected,
}

/// Information about a connected peer
#[derive(Debug, Clone)]
pub struct PeerConnection {
    pub address: SocketAddr,
    pub state: PeerState,
    pub remote_tip: Option<Tip>,
    pub negotiated_version: Option<u32>,
    pub connected_at: Option<Instant>,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub is_initiator: bool,
}

impl PeerConnection {
    pub fn new_outbound(address: SocketAddr) -> Self {
        PeerConnection {
            address,
            state: PeerState::Connecting,
            remote_tip: None,
            negotiated_version: None,
            connected_at: None,
            bytes_sent: 0,
            bytes_received: 0,
            is_initiator: true,
        }
    }

    pub fn new_inbound(address: SocketAddr) -> Self {
        PeerConnection {
            address,
            state: PeerState::Connecting,
            remote_tip: None,
            negotiated_version: None,
            connected_at: None,
            bytes_sent: 0,
            bytes_received: 0,
            is_initiator: false,
        }
    }

    pub fn is_connected(&self) -> bool {
        self.state == PeerState::Connected
    }

    pub fn uptime_secs(&self) -> Option<u64> {
        self.connected_at.map(|t| t.elapsed().as_secs())
    }
}

/// Peer selection policy
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PeerSelectionConfig {
    pub target_active_peers: usize,
    pub target_established_peers: usize,
    pub target_known_peers: usize,
    pub max_inbound_peers: usize,
}

impl Default for PeerSelectionConfig {
    fn default() -> Self {
        PeerSelectionConfig {
            target_active_peers: 20,
            target_established_peers: 40,
            target_known_peers: 100,
            max_inbound_peers: 100,
        }
    }
}
