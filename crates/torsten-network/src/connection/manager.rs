//! ConnectionManager — core lifecycle manager for all peer connections.
//!
//! Manages:
//! - Inbound TCP connection acceptance with rate limiting
//! - Outbound TCP connection establishment
//! - N2C Unix socket listener
//! - Connection deduplication and simultaneous open detection
//! - Connection limits (max inbound, max outbound, per-IP rate)

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use super::handler::ConnectionHandler;
use super::state::{ConnectionState, DataFlow, Provenance};

/// Inbound idle timeout — connections in `InboundIdle` for longer than this
/// are transitioned to `TerminatingConn` and closed.
///
/// Matches Haskell `serverProtocolIdleTimeout = 300s` from
/// `Ouroboros.Network.Server2`.
const INBOUND_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// Connection manager configuration.
#[derive(Debug, Clone)]
pub struct ConnectionManagerConfig {
    /// Maximum inbound connections.
    pub max_inbound: usize,
    /// Maximum outbound connections.
    pub max_outbound: usize,
    /// Maximum connection attempts per IP per minute.
    pub per_ip_rate_limit: usize,
    /// Network magic for handshake validation.
    pub network_magic: u64,
    /// Whether to enable peer sharing.
    pub peer_sharing: bool,
}

impl Default for ConnectionManagerConfig {
    fn default() -> Self {
        Self {
            max_inbound: 100,
            max_outbound: 20,
            per_ip_rate_limit: 5,
            network_magic: 2, // Preview testnet
            peer_sharing: true,
        }
    }
}

/// Tracks a single connection's state and handler.
struct ConnectionEntry {
    /// Current connection state.
    state: ConnectionState,
    /// Protocol handler for this connection (used by connection orchestration).
    #[allow(dead_code)]
    handler: ConnectionHandler,
    /// When this connection entered `InboundIdle` state.
    ///
    /// Set when `inbound_negotiated()` transitions to `InboundIdle`, cleared
    /// when `inbound_activity()` transitions to `InboundState`. Used by
    /// `check_inbound_idle_timeouts()` to enforce the 5-minute idle limit.
    idle_since: Option<Instant>,
}

/// ConnectionManager — central lifecycle manager for all connections.
pub struct ConnectionManager {
    /// Configuration.
    config: ConnectionManagerConfig,
    /// Active connections, keyed by peer address.
    connections: Arc<Mutex<HashMap<SocketAddr, ConnectionEntry>>>,
}

impl ConnectionManager {
    /// Create a new connection manager.
    pub fn new(config: ConnectionManagerConfig) -> Self {
        Self {
            config,
            connections: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Reserve an outbound connection slot.
    ///
    /// Returns `Ok(())` if a slot is available, `Err` if max_outbound reached
    /// or a connection to this peer already exists.
    pub async fn reserve_outbound(
        &self,
        addr: SocketAddr,
    ) -> Result<(), crate::error::ConnectionError> {
        let mut conns = self.connections.lock().await;

        // Check for existing connection
        if conns.contains_key(&addr) {
            return Err(crate::error::ConnectionError::SimultaneousOpenConflict);
        }

        // Check outbound limit
        let outbound_count = conns
            .values()
            .filter(|e| {
                matches!(
                    e.state,
                    ConnectionState::ReservedOutbound
                        | ConnectionState::OutboundIdle(_)
                        | ConnectionState::OutboundUni
                        | ConnectionState::OutboundDup
                        | ConnectionState::UnnegotiatedConn(Provenance::Outbound)
                        | ConnectionState::DuplexConn
                )
            })
            .count();

        if outbound_count >= self.config.max_outbound {
            return Err(crate::error::ConnectionError::MaxConnectionsReached);
        }

        conns.insert(
            addr,
            ConnectionEntry {
                state: ConnectionState::ReservedOutbound,
                handler: ConnectionHandler::new(),
                idle_since: None,
            },
        );

        Ok(())
    }

    /// Record that an outbound connection completed handshake.
    pub async fn outbound_connected(&self, addr: SocketAddr, duplex: bool) {
        let mut conns = self.connections.lock().await;
        if let Some(entry) = conns.get_mut(&addr) {
            entry.state = ConnectionState::OutboundIdle(if duplex {
                DataFlow::Duplex
            } else {
                DataFlow::Unidirectional
            });
        }
    }

    /// Accept an inbound connection.
    ///
    /// Returns `Ok(())` if the connection is accepted, `Err` if limits reached.
    pub async fn accept_inbound(
        &self,
        addr: SocketAddr,
    ) -> Result<(), crate::error::ConnectionError> {
        let mut conns = self.connections.lock().await;

        // Check for existing connection (simultaneous open)
        if let Some(existing) = conns.get(&addr) {
            if existing.state == ConnectionState::ReservedOutbound {
                // Simultaneous open — we already have an outbound attempt.
                // The Haskell algorithm uses address comparison to resolve.
                return Err(crate::error::ConnectionError::SimultaneousOpenConflict);
            }
            // Already connected
            return Err(crate::error::ConnectionError::ForbiddenConnection);
        }

        // Check inbound limit
        let inbound_count = conns
            .values()
            .filter(|e| {
                matches!(
                    e.state,
                    ConnectionState::InboundIdle(_)
                        | ConnectionState::InboundState(_)
                        | ConnectionState::UnnegotiatedConn(Provenance::Inbound)
                        | ConnectionState::DuplexConn
                )
            })
            .count();

        if inbound_count >= self.config.max_inbound {
            return Err(crate::error::ConnectionError::MaxConnectionsReached);
        }

        conns.insert(
            addr,
            ConnectionEntry {
                state: ConnectionState::UnnegotiatedConn(Provenance::Inbound),
                handler: ConnectionHandler::new(),
                idle_since: None,
            },
        );

        Ok(())
    }

    /// Record that an inbound connection completed handshake.
    ///
    /// Transitions to `InboundIdle` and starts the idle timeout clock.
    pub async fn inbound_negotiated(&self, addr: SocketAddr, duplex: bool) {
        let mut conns = self.connections.lock().await;
        if let Some(entry) = conns.get_mut(&addr) {
            entry.state = ConnectionState::InboundIdle(if duplex {
                DataFlow::Duplex
            } else {
                DataFlow::Unidirectional
            });
            entry.idle_since = Some(Instant::now());
        }
    }

    /// Record mini-protocol activity on an inbound connection.
    ///
    /// If the connection is in `InboundIdle`, transitions to `InboundState`
    /// and cancels the idle timeout. This prevents active connections from
    /// being prematurely closed.
    pub async fn inbound_activity(&self, addr: SocketAddr) {
        let mut conns = self.connections.lock().await;
        if let Some(entry) = conns.get_mut(&addr) {
            if let ConnectionState::InboundIdle(df) = entry.state {
                entry.state = ConnectionState::InboundState(df);
                entry.idle_since = None;
            }
        }
    }

    /// Check for inbound idle timeouts and return addresses to terminate.
    ///
    /// Sweeps all connections in `InboundIdle` state. Any that have been idle
    /// longer than [`INBOUND_IDLE_TIMEOUT`] (5 minutes) are transitioned to
    /// `TerminatingConn` and their addresses returned for the caller to close.
    ///
    /// Matches Haskell `serverProtocolIdleTimeout` from `Ouroboros.Network.Server2`.
    pub async fn check_inbound_idle_timeouts(&self) -> Vec<SocketAddr> {
        let mut conns = self.connections.lock().await;
        let now = Instant::now();
        let mut to_terminate = Vec::new();

        for (addr, entry) in conns.iter_mut() {
            if matches!(entry.state, ConnectionState::InboundIdle(_)) {
                if let Some(since) = entry.idle_since {
                    if now.duration_since(since) >= INBOUND_IDLE_TIMEOUT {
                        entry.state = ConnectionState::TerminatingConn;
                        entry.idle_since = None;
                        to_terminate.push(*addr);
                    }
                }
            }
        }

        to_terminate
    }

    /// Remove a connection (disconnected).
    pub async fn remove_connection(&self, addr: &SocketAddr) {
        let mut conns = self.connections.lock().await;
        conns.remove(addr);
    }

    /// Get current connection count.
    pub async fn connection_count(&self) -> usize {
        self.connections.lock().await.len()
    }

    /// Get all connected peer addresses.
    pub async fn connected_peers(&self) -> Vec<SocketAddr> {
        self.connections.lock().await.keys().copied().collect()
    }

    /// Get the configuration.
    pub fn config(&self) -> &ConnectionManagerConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn test_addr(port: u16) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4)), port)
    }

    #[tokio::test]
    async fn reserve_and_connect_outbound() {
        let cm = ConnectionManager::new(ConnectionManagerConfig::default());

        cm.reserve_outbound(test_addr(3001)).await.unwrap();
        assert_eq!(cm.connection_count().await, 1);

        cm.outbound_connected(test_addr(3001), true).await;
        assert_eq!(cm.connection_count().await, 1);
    }

    #[tokio::test]
    async fn rejects_duplicate_outbound() {
        let cm = ConnectionManager::new(ConnectionManagerConfig::default());

        cm.reserve_outbound(test_addr(3001)).await.unwrap();
        let result = cm.reserve_outbound(test_addr(3001)).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn respects_outbound_limit() {
        let config = ConnectionManagerConfig {
            max_outbound: 2,
            ..Default::default()
        };
        let cm = ConnectionManager::new(config);

        cm.reserve_outbound(test_addr(3001)).await.unwrap();
        cm.reserve_outbound(test_addr(3002)).await.unwrap();

        let result = cm.reserve_outbound(test_addr(3003)).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn accept_inbound() {
        let cm = ConnectionManager::new(ConnectionManagerConfig::default());

        cm.accept_inbound(test_addr(3001)).await.unwrap();
        cm.inbound_negotiated(test_addr(3001), true).await;
        assert_eq!(cm.connection_count().await, 1);
    }

    #[tokio::test]
    async fn remove_connection() {
        let cm = ConnectionManager::new(ConnectionManagerConfig::default());

        cm.accept_inbound(test_addr(3001)).await.unwrap();
        assert_eq!(cm.connection_count().await, 1);

        cm.remove_connection(&test_addr(3001)).await;
        assert_eq!(cm.connection_count().await, 0);
    }

    #[tokio::test]
    async fn simultaneous_open_detected() {
        let cm = ConnectionManager::new(ConnectionManagerConfig::default());

        // Reserve outbound
        cm.reserve_outbound(test_addr(3001)).await.unwrap();

        // Try to accept inbound from same address
        let result = cm.accept_inbound(test_addr(3001)).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn inbound_idle_timeout_terminates() {
        let cm = ConnectionManager::new(ConnectionManagerConfig::default());
        let addr = test_addr(3001);

        cm.accept_inbound(addr).await.unwrap();
        cm.inbound_negotiated(addr, true).await;

        // Immediately after negotiation — no timeout yet.
        let expired = cm.check_inbound_idle_timeouts().await;
        assert!(expired.is_empty(), "should not timeout immediately");

        // Manually set idle_since to 6 minutes ago to simulate time passing
        // (avoids dependency on tokio::time::pause which requires "test-util").
        {
            let mut conns = cm.connections.lock().await;
            let entry = conns.get_mut(&addr).unwrap();
            entry.idle_since = Some(Instant::now() - Duration::from_secs(360));
        }

        let expired = cm.check_inbound_idle_timeouts().await;
        assert_eq!(expired, vec![addr], "should timeout after 5+ minutes idle");
    }

    #[tokio::test]
    async fn inbound_activity_cancels_timeout() {
        let cm = ConnectionManager::new(ConnectionManagerConfig::default());
        let addr = test_addr(3001);

        cm.accept_inbound(addr).await.unwrap();
        cm.inbound_negotiated(addr, true).await;

        // Simulate 3 minutes of idle time.
        {
            let mut conns = cm.connections.lock().await;
            let entry = conns.get_mut(&addr).unwrap();
            entry.idle_since = Some(Instant::now() - Duration::from_secs(180));
        }

        // Mini-protocol activity resets the timer.
        cm.inbound_activity(addr).await;

        // Even after more time, should NOT timeout because activity occurred
        // (state is now InboundState, not InboundIdle).
        let expired = cm.check_inbound_idle_timeouts().await;
        assert!(
            expired.is_empty(),
            "activity should cancel the idle timeout"
        );
    }

    #[tokio::test]
    async fn inbound_idle_no_false_positives_on_outbound() {
        let cm = ConnectionManager::new(ConnectionManagerConfig::default());

        // Outbound connection in idle state — should NOT be affected by
        // inbound idle timeout sweep.
        let addr = test_addr(3001);
        cm.reserve_outbound(addr).await.unwrap();
        cm.outbound_connected(addr, true).await;

        // Manually set idle_since (shouldn't happen in practice for outbound,
        // but verifies the sweep only targets InboundIdle).
        {
            let mut conns = cm.connections.lock().await;
            let entry = conns.get_mut(&addr).unwrap();
            entry.idle_since = Some(Instant::now() - Duration::from_secs(600));
        }

        let expired = cm.check_inbound_idle_timeouts().await;
        assert!(
            expired.is_empty(),
            "outbound connections should not be affected by inbound idle timeout"
        );
    }
}
