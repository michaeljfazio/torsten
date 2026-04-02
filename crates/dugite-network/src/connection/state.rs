//! Connection state machine.
//!
//! Tracks the lifecycle state of each peer connection, matching the
//! Haskell `ConnectionState` from `ouroboros-network`.
//!
//! The connection manager counters follow the Haskell `ConnectionManagerCounters`
//! from `Ouroboros.Network.ConnectionManager.Core`, where counters are additive
//! (not mutually exclusive). Notably, a `DuplexConn` increments `full_duplex`,
//! `duplex`, `inbound`, AND `outbound` simultaneously.

use std::ops::{Add, AddAssign};

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
///
/// Matches the Haskell `ConnectionState` from `ouroboros-network`.
/// Each state maps to specific `ConnectionManagerCounters` via `to_counters()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    /// Outbound connection slot reserved, TCP connect in progress.
    ReservedOutbound,
    /// TCP connected but handshake not yet complete.
    UnnegotiatedConn(Provenance),
    /// Outbound connection with unidirectional data flow, initiator protocols active.
    /// Haskell: `OutboundUniState`.
    OutboundUni,
    /// Outbound connection with duplex data flow, initiator protocols active.
    /// Haskell: `OutboundDupState`.
    OutboundDup,
    /// Handshake complete, outbound idle (keepalive only).
    /// Haskell: `OutboundIdleState DataFlow`.
    OutboundIdle(DataFlow),
    /// Handshake complete, inbound idle (keepalive only, timeout pending).
    /// Haskell: `InboundIdleState DataFlow`.
    InboundIdle(DataFlow),
    /// Inbound connection with at least one responder protocol running.
    /// Haskell: `InboundState DataFlow`.
    InboundState(DataFlow),
    /// Full duplex connection — both initiator and responder protocols active.
    /// Haskell: `DuplexState`.
    DuplexConn,
    /// Connection teardown in progress.
    /// Haskell: `TerminatingState`.
    TerminatingConn,
    /// Connection closed.
    /// Haskell: `TerminatedState`.
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
                | ConnectionState::TerminatingConn
                | ConnectionState::Closed
        )
    }

    /// Return the provenance (direction) of this connection, if applicable.
    ///
    /// `DuplexConn` counts as both inbound and outbound in Haskell's counter
    /// model, so it returns `None` here — use `to_counters()` for accurate
    /// direction counting.
    pub fn provenance(&self) -> Option<Provenance> {
        match self {
            Self::ReservedOutbound => Some(Provenance::Outbound),
            Self::UnnegotiatedConn(p) => Some(*p),
            Self::OutboundUni | Self::OutboundDup | Self::OutboundIdle(_) => {
                Some(Provenance::Outbound)
            }
            Self::InboundIdle(_) | Self::InboundState(_) => Some(Provenance::Inbound),
            // DuplexConn is both directions; TerminatingConn/Closed have no direction.
            Self::DuplexConn | Self::TerminatingConn | Self::Closed => None,
        }
    }

    /// Return the negotiated data flow, if applicable.
    pub fn data_flow(&self) -> Option<DataFlow> {
        match self {
            Self::OutboundUni | Self::OutboundIdle(DataFlow::Unidirectional) => {
                Some(DataFlow::Unidirectional)
            }
            Self::OutboundDup | Self::OutboundIdle(DataFlow::Duplex) => Some(DataFlow::Duplex),
            Self::InboundIdle(df) | Self::InboundState(df) => Some(*df),
            Self::DuplexConn => Some(DataFlow::Duplex),
            _ => None,
        }
    }

    /// Compute the `ConnectionManagerCounters` contribution of this state.
    ///
    /// Exactly mirrors Haskell's `connectionStateToCounters` from
    /// `Ouroboros.Network.ConnectionManager.Core`. Counters are additive —
    /// a single `DuplexConn` contributes to `full_duplex`, `duplex`,
    /// `inbound`, AND `outbound` simultaneously.
    pub fn to_counters(&self) -> ConnectionManagerCounters {
        match self {
            Self::ReservedOutbound => ConnectionManagerCounters::default(),

            Self::UnnegotiatedConn(Provenance::Inbound) => ConnectionManagerCounters {
                inbound: 1,
                ..Default::default()
            },
            Self::UnnegotiatedConn(Provenance::Outbound) => ConnectionManagerCounters {
                outbound: 1,
                ..Default::default()
            },

            Self::OutboundUni => ConnectionManagerCounters {
                unidirectional: 1,
                outbound: 1,
                ..Default::default()
            },
            Self::OutboundDup => ConnectionManagerCounters {
                duplex: 1,
                outbound: 1,
                ..Default::default()
            },

            Self::OutboundIdle(DataFlow::Unidirectional) => ConnectionManagerCounters {
                unidirectional: 1,
                outbound: 1,
                ..Default::default()
            },
            Self::OutboundIdle(DataFlow::Duplex) => ConnectionManagerCounters {
                duplex: 1,
                outbound: 1,
                ..Default::default()
            },

            Self::InboundIdle(DataFlow::Unidirectional) => ConnectionManagerCounters {
                unidirectional: 1,
                inbound: 1,
                ..Default::default()
            },
            Self::InboundIdle(DataFlow::Duplex) => ConnectionManagerCounters {
                duplex: 1,
                inbound: 1,
                ..Default::default()
            },

            Self::InboundState(DataFlow::Unidirectional) => ConnectionManagerCounters {
                unidirectional: 1,
                inbound: 1,
                ..Default::default()
            },
            Self::InboundState(DataFlow::Duplex) => ConnectionManagerCounters {
                duplex: 1,
                inbound: 1,
                ..Default::default()
            },

            Self::DuplexConn => ConnectionManagerCounters {
                full_duplex: 1,
                duplex: 1,
                inbound: 1,
                outbound: 1,
                ..Default::default()
            },

            Self::TerminatingConn => ConnectionManagerCounters {
                terminating: 1,
                ..Default::default()
            },

            Self::Closed => ConnectionManagerCounters::default(),
        }
    }
}

/// Aggregated connection manager counters.
///
/// Matches Haskell's `ConnectionManagerCounters` from
/// `Ouroboros.Network.ConnectionManager.Core`. Counters are additive —
/// `inbound + outbound` can exceed total connections because `DuplexConn`
/// contributes to both.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConnectionManagerCounters {
    /// Connections in full duplex state (both initiator and responder active).
    pub full_duplex: u64,
    /// Connections negotiated with `Duplex` data flow (superset of `full_duplex`).
    pub duplex: u64,
    /// Connections negotiated with `Unidirectional` data flow.
    pub unidirectional: u64,
    /// Connections where the remote peer initiated TCP (includes DuplexConn).
    pub inbound: u64,
    /// Connections where we initiated TCP (includes DuplexConn).
    pub outbound: u64,
    /// Connections in teardown.
    pub terminating: u64,
}

impl Add for ConnectionManagerCounters {
    type Output = Self;

    fn add(self, rhs: Self) -> Self {
        ConnectionManagerCounters {
            full_duplex: self.full_duplex + rhs.full_duplex,
            duplex: self.duplex + rhs.duplex,
            unidirectional: self.unidirectional + rhs.unidirectional,
            inbound: self.inbound + rhs.inbound,
            outbound: self.outbound + rhs.outbound,
            terminating: self.terminating + rhs.terminating,
        }
    }
}

impl AddAssign for ConnectionManagerCounters {
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

impl std::iter::Sum for ConnectionManagerCounters {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Self::default(), |acc, c| acc + c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── ConnectionState existing tests ─────────────────────────────────

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
    fn cannot_promote_from_active() {
        // Active states (OutboundDup, OutboundUni, InboundState) are already
        // running protocols — they should not be promoted again.
        assert!(!ConnectionState::OutboundDup.can_promote_to_hot());
        assert!(!ConnectionState::OutboundUni.can_promote_to_hot());
        assert!(!ConnectionState::InboundState(DataFlow::Duplex).can_promote_to_hot());
        assert!(!ConnectionState::TerminatingConn.can_promote_to_hot());
    }

    #[test]
    fn negotiated_states() {
        assert!(ConnectionState::OutboundIdle(DataFlow::Duplex).is_negotiated());
        assert!(ConnectionState::DuplexConn.is_negotiated());
        assert!(ConnectionState::OutboundDup.is_negotiated());
        assert!(ConnectionState::OutboundUni.is_negotiated());
        assert!(ConnectionState::InboundState(DataFlow::Duplex).is_negotiated());
        assert!(!ConnectionState::ReservedOutbound.is_negotiated());
        assert!(!ConnectionState::UnnegotiatedConn(Provenance::Inbound).is_negotiated());
        assert!(!ConnectionState::TerminatingConn.is_negotiated());
        assert!(!ConnectionState::Closed.is_negotiated());
    }

    // ─── Provenance tests ───────────────────────────────────────────────

    #[test]
    fn provenance_outbound_states() {
        assert_eq!(
            ConnectionState::ReservedOutbound.provenance(),
            Some(Provenance::Outbound)
        );
        assert_eq!(
            ConnectionState::UnnegotiatedConn(Provenance::Outbound).provenance(),
            Some(Provenance::Outbound)
        );
        assert_eq!(
            ConnectionState::OutboundUni.provenance(),
            Some(Provenance::Outbound)
        );
        assert_eq!(
            ConnectionState::OutboundDup.provenance(),
            Some(Provenance::Outbound)
        );
        assert_eq!(
            ConnectionState::OutboundIdle(DataFlow::Duplex).provenance(),
            Some(Provenance::Outbound)
        );
    }

    #[test]
    fn provenance_inbound_states() {
        assert_eq!(
            ConnectionState::UnnegotiatedConn(Provenance::Inbound).provenance(),
            Some(Provenance::Inbound)
        );
        assert_eq!(
            ConnectionState::InboundIdle(DataFlow::Duplex).provenance(),
            Some(Provenance::Inbound)
        );
        assert_eq!(
            ConnectionState::InboundState(DataFlow::Duplex).provenance(),
            Some(Provenance::Inbound)
        );
    }

    #[test]
    fn provenance_none_for_duplex_and_terminal() {
        assert_eq!(ConnectionState::DuplexConn.provenance(), None);
        assert_eq!(ConnectionState::TerminatingConn.provenance(), None);
        assert_eq!(ConnectionState::Closed.provenance(), None);
    }

    // ─── to_counters() tests ────────────────────────────────────────────

    #[test]
    fn counters_reserved_outbound() {
        let c = ConnectionState::ReservedOutbound.to_counters();
        assert_eq!(c, ConnectionManagerCounters::default());
    }

    #[test]
    fn counters_unnegotiated_inbound() {
        let c = ConnectionState::UnnegotiatedConn(Provenance::Inbound).to_counters();
        assert_eq!(c.inbound, 1);
        assert_eq!(c.outbound, 0);
        assert_eq!(c.duplex, 0);
    }

    #[test]
    fn counters_unnegotiated_outbound() {
        let c = ConnectionState::UnnegotiatedConn(Provenance::Outbound).to_counters();
        assert_eq!(c.outbound, 1);
        assert_eq!(c.inbound, 0);
        assert_eq!(c.duplex, 0);
    }

    #[test]
    fn counters_outbound_uni() {
        let c = ConnectionState::OutboundUni.to_counters();
        assert_eq!(c.unidirectional, 1);
        assert_eq!(c.outbound, 1);
        assert_eq!(c.duplex, 0);
        assert_eq!(c.full_duplex, 0);
    }

    #[test]
    fn counters_outbound_dup() {
        let c = ConnectionState::OutboundDup.to_counters();
        assert_eq!(c.duplex, 1);
        assert_eq!(c.outbound, 1);
        assert_eq!(c.unidirectional, 0);
        assert_eq!(c.full_duplex, 0);
    }

    #[test]
    fn counters_outbound_idle_unidirectional() {
        let c = ConnectionState::OutboundIdle(DataFlow::Unidirectional).to_counters();
        assert_eq!(c.unidirectional, 1);
        assert_eq!(c.outbound, 1);
        assert_eq!(c.duplex, 0);
    }

    #[test]
    fn counters_outbound_idle_duplex() {
        let c = ConnectionState::OutboundIdle(DataFlow::Duplex).to_counters();
        assert_eq!(c.duplex, 1);
        assert_eq!(c.outbound, 1);
        assert_eq!(c.unidirectional, 0);
    }

    #[test]
    fn counters_inbound_idle_unidirectional() {
        let c = ConnectionState::InboundIdle(DataFlow::Unidirectional).to_counters();
        assert_eq!(c.unidirectional, 1);
        assert_eq!(c.inbound, 1);
        assert_eq!(c.duplex, 0);
    }

    #[test]
    fn counters_inbound_idle_duplex() {
        let c = ConnectionState::InboundIdle(DataFlow::Duplex).to_counters();
        assert_eq!(c.duplex, 1);
        assert_eq!(c.inbound, 1);
        assert_eq!(c.unidirectional, 0);
    }

    #[test]
    fn counters_inbound_state_unidirectional() {
        let c = ConnectionState::InboundState(DataFlow::Unidirectional).to_counters();
        assert_eq!(c.unidirectional, 1);
        assert_eq!(c.inbound, 1);
        assert_eq!(c.duplex, 0);
    }

    #[test]
    fn counters_inbound_state_duplex() {
        let c = ConnectionState::InboundState(DataFlow::Duplex).to_counters();
        assert_eq!(c.duplex, 1);
        assert_eq!(c.inbound, 1);
        assert_eq!(c.unidirectional, 0);
    }

    #[test]
    fn counters_duplex_conn_all_four() {
        // DuplexConn increments ALL FOUR: full_duplex, duplex, inbound, outbound.
        // This matches Haskell's DuplexState counter behaviour.
        let c = ConnectionState::DuplexConn.to_counters();
        assert_eq!(c.full_duplex, 1);
        assert_eq!(c.duplex, 1);
        assert_eq!(c.inbound, 1);
        assert_eq!(c.outbound, 1);
        assert_eq!(c.unidirectional, 0);
        assert_eq!(c.terminating, 0);
    }

    #[test]
    fn counters_terminating() {
        let c = ConnectionState::TerminatingConn.to_counters();
        assert_eq!(c.terminating, 1);
        assert_eq!(c.full_duplex, 0);
        assert_eq!(c.duplex, 0);
        assert_eq!(c.inbound, 0);
        assert_eq!(c.outbound, 0);
    }

    #[test]
    fn counters_closed() {
        let c = ConnectionState::Closed.to_counters();
        assert_eq!(c, ConnectionManagerCounters::default());
    }

    // ─── ConnectionManagerCounters aggregation ──────────────────────────

    #[test]
    fn counters_addition() {
        let a = ConnectionManagerCounters {
            full_duplex: 1,
            duplex: 2,
            unidirectional: 0,
            inbound: 3,
            outbound: 2,
            terminating: 0,
        };
        let b = ConnectionManagerCounters {
            full_duplex: 0,
            duplex: 1,
            unidirectional: 1,
            inbound: 1,
            outbound: 1,
            terminating: 1,
        };
        let sum = a + b;
        assert_eq!(sum.full_duplex, 1);
        assert_eq!(sum.duplex, 3);
        assert_eq!(sum.unidirectional, 1);
        assert_eq!(sum.inbound, 4);
        assert_eq!(sum.outbound, 3);
        assert_eq!(sum.terminating, 1);
    }

    #[test]
    fn counters_sum_iterator() {
        // Simulate 3 outbound duplex idle + 2 inbound duplex idle + 1 DuplexConn.
        let states = [
            ConnectionState::OutboundIdle(DataFlow::Duplex),
            ConnectionState::OutboundIdle(DataFlow::Duplex),
            ConnectionState::OutboundIdle(DataFlow::Duplex),
            ConnectionState::InboundIdle(DataFlow::Duplex),
            ConnectionState::InboundIdle(DataFlow::Duplex),
            ConnectionState::DuplexConn,
        ];
        let total: ConnectionManagerCounters = states.iter().map(|s| s.to_counters()).sum();

        // DuplexConn counts as both inbound and outbound.
        assert_eq!(total.full_duplex, 1);
        assert_eq!(total.duplex, 6); // all 6 are duplex
        assert_eq!(total.outbound, 4); // 3 outbound idle + 1 duplex
        assert_eq!(total.inbound, 3); // 2 inbound idle + 1 duplex
        assert_eq!(total.unidirectional, 0);
        assert_eq!(total.terminating, 0);
    }

    // ─── data_flow() tests ──────────────────────────────────────────────

    #[test]
    fn data_flow_values() {
        assert_eq!(
            ConnectionState::OutboundUni.data_flow(),
            Some(DataFlow::Unidirectional)
        );
        assert_eq!(
            ConnectionState::OutboundDup.data_flow(),
            Some(DataFlow::Duplex)
        );
        assert_eq!(
            ConnectionState::OutboundIdle(DataFlow::Duplex).data_flow(),
            Some(DataFlow::Duplex)
        );
        assert_eq!(
            ConnectionState::InboundIdle(DataFlow::Unidirectional).data_flow(),
            Some(DataFlow::Unidirectional)
        );
        assert_eq!(
            ConnectionState::InboundState(DataFlow::Duplex).data_flow(),
            Some(DataFlow::Duplex)
        );
        assert_eq!(
            ConnectionState::DuplexConn.data_flow(),
            Some(DataFlow::Duplex)
        );
        assert_eq!(ConnectionState::ReservedOutbound.data_flow(), None);
        assert_eq!(ConnectionState::TerminatingConn.data_flow(), None);
        assert_eq!(ConnectionState::Closed.data_flow(), None);
    }
}
