pub mod blockfetch;
pub mod chainsync;
pub mod handshake;
pub mod keepalive;
pub mod localstatequery;
pub mod localtxsubmission;
pub mod peersharing;
pub mod txsubmission;

/// Mini-protocol IDs as used by the Ouroboros multiplexer
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(dead_code)]
pub enum MiniProtocolId {
    Handshake = 0,
    ChainSync = 2,
    BlockFetch = 3,
    TxSubmission2 = 4,
    KeepAlive = 8,
    PeerSharing = 10,
    LocalChainSync = 5,
    LocalTxSubmission = 6,
    LocalStateQuery = 7,
    LocalTxMonitor = 9,
}

impl MiniProtocolId {
    #[allow(dead_code)]
    pub fn from_u16(value: u16) -> Option<Self> {
        match value {
            0 => Some(MiniProtocolId::Handshake),
            2 => Some(MiniProtocolId::ChainSync),
            3 => Some(MiniProtocolId::BlockFetch),
            4 => Some(MiniProtocolId::TxSubmission2),
            5 => Some(MiniProtocolId::LocalChainSync),
            6 => Some(MiniProtocolId::LocalTxSubmission),
            7 => Some(MiniProtocolId::LocalStateQuery),
            8 => Some(MiniProtocolId::KeepAlive),
            9 => Some(MiniProtocolId::LocalTxMonitor),
            10 => Some(MiniProtocolId::PeerSharing),
            _ => None,
        }
    }
}

/// Direction of a mini-protocol message
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Direction {
    /// Message from initiator to responder
    InitiatorToResponder,
    /// Message from responder to initiator
    ResponderToInitiator,
}

/// Agency: who has the right to send the next message
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Agency {
    Client,
    Server,
    Nobody,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── MiniProtocolId discriminants ─────────────────────────────────────────

    #[test]
    fn test_mini_protocol_id_discriminants_match_ouroboros_spec() {
        // These must exactly match the Ouroboros wire protocol IDs.
        // Changing any of these will break compatibility with cardano-node.
        assert_eq!(MiniProtocolId::Handshake as u16, 0);
        assert_eq!(MiniProtocolId::ChainSync as u16, 2);
        assert_eq!(MiniProtocolId::BlockFetch as u16, 3);
        assert_eq!(MiniProtocolId::TxSubmission2 as u16, 4);
        assert_eq!(MiniProtocolId::LocalChainSync as u16, 5);
        assert_eq!(MiniProtocolId::LocalTxSubmission as u16, 6);
        assert_eq!(MiniProtocolId::LocalStateQuery as u16, 7);
        assert_eq!(MiniProtocolId::KeepAlive as u16, 8);
        assert_eq!(MiniProtocolId::LocalTxMonitor as u16, 9);
        assert_eq!(MiniProtocolId::PeerSharing as u16, 10);
    }

    // ── MiniProtocolId::from_u16 ─────────────────────────────────────────────

    #[test]
    fn test_from_u16_all_known_ids() {
        // Every defined protocol ID should round-trip through from_u16.
        let cases: &[(u16, MiniProtocolId)] = &[
            (0, MiniProtocolId::Handshake),
            (2, MiniProtocolId::ChainSync),
            (3, MiniProtocolId::BlockFetch),
            (4, MiniProtocolId::TxSubmission2),
            (5, MiniProtocolId::LocalChainSync),
            (6, MiniProtocolId::LocalTxSubmission),
            (7, MiniProtocolId::LocalStateQuery),
            (8, MiniProtocolId::KeepAlive),
            (9, MiniProtocolId::LocalTxMonitor),
            (10, MiniProtocolId::PeerSharing),
        ];
        for (raw, expected) in cases {
            let parsed = MiniProtocolId::from_u16(*raw);
            assert_eq!(
                parsed,
                Some(*expected),
                "from_u16({raw}) should parse to {expected:?}"
            );
        }
    }

    #[test]
    fn test_from_u16_unknown_ids_return_none() {
        // Unassigned IDs should return None rather than panic.
        let unknown_ids: &[u16] = &[1, 11, 255, 1000, u16::MAX];
        for id in unknown_ids {
            assert!(
                MiniProtocolId::from_u16(*id).is_none(),
                "from_u16({id}) should return None for unknown protocol ID"
            );
        }
    }

    #[test]
    fn test_from_u16_returns_none_for_id_1() {
        // Protocol ID 1 is explicitly unassigned in the Ouroboros spec.
        assert!(MiniProtocolId::from_u16(1).is_none());
    }
}
