//! Connection state machine.
//!
//! Tracks the lifecycle state of each peer connection, matching the
//! Haskell `ConnectionState` from `ouroboros-network`.

/// Provenance of a connection — who initiated it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provenance {
    /// We initiated the TCP connection.
    Outbound,
    /// The remote peer initiated the TCP connection.
    Inbound,
}

/// Data flow capability of a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataFlow {
    /// Unidirectional — only the initiator sends data.
    Unidirectional,
    /// Duplex — both sides can send data.
    Duplex,
}

/// Connection lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    /// Outbound connection slot reserved, TCP connect in progress.
    ReservedOutbound,
    /// TCP connected but handshake not yet complete.
    UnnegotiatedConn(Provenance),
    /// Handshake complete, outbound idle (keepalive only).
    OutboundIdle(DataFlow),
    /// Handshake complete, inbound idle (keepalive only).
    InboundIdle(DataFlow),
    /// Full duplex connection (both inbound and outbound active).
    DuplexConn,
    /// Connection closed.
    Closed,
}

impl ConnectionState {
    /// Check if this connection state allows promotion to hot (start sync protocols).
    pub fn can_promote_to_hot(&self) -> bool {
        matches!(
            self,
            ConnectionState::OutboundIdle(_)
                | ConnectionState::InboundIdle(_)
                | ConnectionState::DuplexConn
        )
    }

    /// Check if the connection is in a negotiated (post-handshake) state.
    pub fn is_negotiated(&self) -> bool {
        !matches!(
            self,
            ConnectionState::ReservedOutbound
                | ConnectionState::UnnegotiatedConn(_)
                | ConnectionState::Closed
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn can_promote_from_idle() {
        assert!(ConnectionState::OutboundIdle(DataFlow::Duplex).can_promote_to_hot());
        assert!(ConnectionState::InboundIdle(DataFlow::Duplex).can_promote_to_hot());
        assert!(ConnectionState::DuplexConn.can_promote_to_hot());
    }

    #[test]
    fn cannot_promote_from_unnegotiated() {
        assert!(!ConnectionState::ReservedOutbound.can_promote_to_hot());
        assert!(!ConnectionState::UnnegotiatedConn(Provenance::Outbound).can_promote_to_hot());
        assert!(!ConnectionState::Closed.can_promote_to_hot());
    }

    #[test]
    fn negotiated_states() {
        assert!(ConnectionState::OutboundIdle(DataFlow::Duplex).is_negotiated());
        assert!(ConnectionState::DuplexConn.is_negotiated());
        assert!(!ConnectionState::ReservedOutbound.is_negotiated());
        assert!(!ConnectionState::UnnegotiatedConn(Provenance::Inbound).is_negotiated());
    }
}
