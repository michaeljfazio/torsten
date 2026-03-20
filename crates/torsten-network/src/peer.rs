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
    /// Defaults matching cardano-node: Active=15, Established=40, Known=85
    fn default() -> Self {
        PeerSelectionConfig {
            target_active_peers: 15,
            target_established_peers: 40,
            target_known_peers: 85,
            max_inbound_peers: 100,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_addr(port: u16) -> SocketAddr {
        format!("127.0.0.1:{port}").parse().unwrap()
    }

    // ── PeerConnection constructors ──────────────────────────────────────────

    #[test]
    fn test_new_outbound_is_initiator() {
        // new_outbound should mark the connection as initiator-side.
        let conn = PeerConnection::new_outbound(make_addr(3001));
        assert!(conn.is_initiator, "Outbound connection must be initiator");
        assert_eq!(conn.state, PeerState::Connecting);
        assert_eq!(conn.address, make_addr(3001));
    }

    #[test]
    fn test_new_inbound_is_not_initiator() {
        // new_inbound should mark the connection as responder-side.
        let conn = PeerConnection::new_inbound(make_addr(3001));
        assert!(
            !conn.is_initiator,
            "Inbound connection must not be initiator"
        );
        assert_eq!(conn.state, PeerState::Connecting);
    }

    #[test]
    fn test_new_connection_starts_at_zero_bytes() {
        // All byte counters must be zero on construction.
        let conn = PeerConnection::new_outbound(make_addr(3001));
        assert_eq!(conn.bytes_sent, 0);
        assert_eq!(conn.bytes_received, 0);
    }

    #[test]
    fn test_new_connection_no_tip_or_version() {
        // remote_tip and negotiated_version start as None.
        let conn = PeerConnection::new_outbound(make_addr(3001));
        assert!(conn.remote_tip.is_none());
        assert!(conn.negotiated_version.is_none());
        assert!(conn.connected_at.is_none());
    }

    // ── is_connected ─────────────────────────────────────────────────────────

    #[test]
    fn test_is_connected_only_when_state_is_connected() {
        // is_connected must return true only for PeerState::Connected.
        let states_not_connected = [
            PeerState::Connecting,
            PeerState::Handshaking,
            PeerState::Disconnecting,
            PeerState::Disconnected,
        ];
        for state in &states_not_connected {
            let mut conn = PeerConnection::new_outbound(make_addr(3001));
            conn.state = *state;
            assert!(
                !conn.is_connected(),
                "State {state:?} must not be considered connected"
            );
        }

        let mut conn = PeerConnection::new_outbound(make_addr(3001));
        conn.state = PeerState::Connected;
        assert!(
            conn.is_connected(),
            "PeerState::Connected must report as connected"
        );
    }

    // ── uptime_secs ───────────────────────────────────────────────────────────

    #[test]
    fn test_uptime_secs_none_when_not_connected() {
        // uptime_secs returns None if connected_at is not set.
        let conn = PeerConnection::new_outbound(make_addr(3001));
        assert!(conn.uptime_secs().is_none());
    }

    #[test]
    fn test_uptime_secs_some_when_connected() {
        // uptime_secs returns Some(elapsed) when connected_at is set.
        let mut conn = PeerConnection::new_outbound(make_addr(3001));
        conn.connected_at = Some(Instant::now() - std::time::Duration::from_secs(5));
        let uptime = conn.uptime_secs().unwrap();
        // Should be at least 4 seconds (allow 1s tolerance for test timing)
        assert!(uptime >= 4, "Uptime should be at least 4s; got {uptime}");
    }

    // ── PeerSelectionConfig defaults ─────────────────────────────────────────

    #[test]
    fn test_peer_selection_config_defaults() {
        // Defaults should match cardano-node reference values.
        let config = PeerSelectionConfig::default();
        assert_eq!(config.target_active_peers, 15);
        assert_eq!(config.target_established_peers, 40);
        assert_eq!(config.target_known_peers, 85);
        assert_eq!(config.max_inbound_peers, 100);
    }
}
