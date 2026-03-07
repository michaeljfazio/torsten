use thiserror::Error;
use torsten_primitives::network::NetworkId;

#[derive(Error, Debug)]
pub enum HandshakeError {
    #[error("Version mismatch: {0}")]
    VersionMismatch(String),
    #[error("Refused: {0}")]
    Refused(String),
    #[error("Network magic mismatch: expected {expected}, got {got}")]
    NetworkMagicMismatch { expected: u64, got: u64 },
}

/// Supported node-to-node protocol versions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeToNodeVersion {
    V7 = 7,
    V8 = 8,
    V9 = 9,
    V10 = 10,
    V11 = 11,
    V12 = 12,
    V13 = 13,
    V14 = 14,
}

/// Supported node-to-client protocol versions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeToClientVersion {
    V9 = 9,
    V10 = 10,
    V11 = 11,
    V12 = 12,
    V13 = 13,
    V14 = 14,
    V15 = 15,
    V16 = 16,
    V17 = 17,
}

/// Version data exchanged during handshake
#[derive(Debug, Clone)]
pub struct VersionData {
    pub network_magic: u64,
    pub initiator_and_responder: bool,
    pub peer_sharing: PeerSharing,
    pub query: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerSharing {
    PeerSharingDisabled,
    PeerSharingEnabled,
}

/// Propose versions to the remote peer
pub fn propose_versions(network: NetworkId) -> Vec<(u32, VersionData)> {
    let magic = network.magic();
    vec![
        (
            13,
            VersionData {
                network_magic: magic,
                initiator_and_responder: true,
                peer_sharing: PeerSharing::PeerSharingDisabled,
                query: false,
            },
        ),
        (
            14,
            VersionData {
                network_magic: magic,
                initiator_and_responder: true,
                peer_sharing: PeerSharing::PeerSharingDisabled,
                query: false,
            },
        ),
    ]
}

/// Handshake state machine
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandshakeState {
    /// Initial state - client proposes versions
    StPropose,
    /// Server confirms or refuses
    StConfirm,
    /// Handshake complete
    StDone,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_propose_mainnet_versions() {
        let versions = propose_versions(NetworkId::Mainnet);
        assert!(!versions.is_empty());
        assert!(versions.iter().all(|(_, v)| v.network_magic == 764824073));
    }

    #[test]
    fn test_propose_testnet_versions() {
        let versions = propose_versions(NetworkId::Testnet);
        assert!(versions.iter().all(|(_, v)| v.network_magic == 1));
    }
}
