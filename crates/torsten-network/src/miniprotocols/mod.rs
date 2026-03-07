pub mod blockfetch;
pub mod chainsync;
pub mod handshake;
pub mod keepalive;
pub mod localstatequery;
pub mod localtxsubmission;
pub mod txsubmission;

/// Mini-protocol IDs as used by the Ouroboros multiplexer
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MiniProtocolId {
    Handshake = 0,
    ChainSync = 2,
    BlockFetch = 3,
    TxSubmission2 = 6,
    KeepAlive = 8,
    LocalChainSync = 5,
    LocalTxSubmission = 9,
    LocalStateQuery = 7,
    LocalTxMonitor = 12,
}

impl MiniProtocolId {
    pub fn from_u16(value: u16) -> Option<Self> {
        match value {
            0 => Some(MiniProtocolId::Handshake),
            2 => Some(MiniProtocolId::ChainSync),
            3 => Some(MiniProtocolId::BlockFetch),
            5 => Some(MiniProtocolId::LocalChainSync),
            6 => Some(MiniProtocolId::TxSubmission2),
            7 => Some(MiniProtocolId::LocalStateQuery),
            8 => Some(MiniProtocolId::KeepAlive),
            9 => Some(MiniProtocolId::LocalTxSubmission),
            12 => Some(MiniProtocolId::LocalTxMonitor),
            _ => None,
        }
    }
}

/// Direction of a mini-protocol message
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Message from initiator to responder
    InitiatorToResponder,
    /// Message from responder to initiator
    ResponderToInitiator,
}

/// Agency: who has the right to send the next message
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agency {
    Client,
    Server,
    Nobody,
}
