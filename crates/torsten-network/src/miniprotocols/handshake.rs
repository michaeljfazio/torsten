use thiserror::Error;
use torsten_primitives::network::NetworkId;

#[derive(Error, Debug)]
#[allow(dead_code)]
pub enum HandshakeError {
    #[error("Version mismatch: {0}")]
    VersionMismatch(String),
    #[error("Refused: {0}")]
    Refused(String),
    #[error("Network magic mismatch: expected {expected}, got {got}")]
    NetworkMagicMismatch { expected: u64, got: u64 },
}

/// Supported node-to-node protocol versions.
///
/// V14 — Plomin hard-fork capability bits
/// V15 — SRV DNS peer-sharing support
/// V16 — Genesis light client / Cardano Node 10.x preferred version
///
/// No Genesis-specific logic is implemented for V16: it is accepted purely
/// so that peers running cardano-node 10.x can negotiate a shared version
/// with us.  Genesis-specific behaviour (e.g. the `query` mode extensions)
/// will be added when the Genesis protocol layer is implemented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum NodeToNodeVersion {
    V14 = 14,
    V15 = 15,
    /// Accepted for interoperability with cardano-node 10.x; no Genesis
    /// extensions are active.
    V16 = 16,
}

/// Supported node-to-client protocol versions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum NodeToClientVersion {
    V16 = 16,
    V17 = 17,
}

/// Version data exchanged during handshake
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct VersionData {
    pub network_magic: u64,
    pub initiator_and_responder: bool,
    pub peer_sharing: PeerSharing,
    pub query: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum PeerSharing {
    PeerSharingDisabled,
    PeerSharingEnabled,
}

/// Propose versions to the remote peer.
///
/// We advertise V14, V15, and V16 so that any peer running cardano-node ≥ 8.x
/// can negotiate a shared version.  V16 capabilities are identical to V15 for
/// now — Genesis-specific extensions will be wired in separately.
#[allow(dead_code)]
pub fn propose_versions(network: NetworkId) -> Vec<(u32, VersionData)> {
    let magic = network.magic();
    // Helper to build a standard VersionData for a given version number.
    // All three versions share the same capability flags.
    let vd = |_version: u32| VersionData {
        network_magic: magic,
        initiator_and_responder: true,
        peer_sharing: PeerSharing::PeerSharingEnabled,
        query: false,
    };
    vec![
        (NodeToNodeVersion::V14 as u32, vd(14)),
        (NodeToNodeVersion::V15 as u32, vd(15)),
        (NodeToNodeVersion::V16 as u32, vd(16)),
    ]
}

/// Handshake state machine
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::enum_variant_names, dead_code)]
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

    /// V14, V15 and V16 must all be present so we can negotiate with
    /// cardano-node peers that prefer any of those versions.
    #[test]
    fn test_propose_versions_includes_v14_v15_v16() {
        let versions = propose_versions(NetworkId::Mainnet);
        let version_numbers: Vec<u32> = versions.iter().map(|(v, _)| *v).collect();
        assert!(
            version_numbers.contains(&14),
            "V14 must be proposed: {version_numbers:?}"
        );
        assert!(
            version_numbers.contains(&15),
            "V15 must be proposed: {version_numbers:?}"
        );
        assert!(
            version_numbers.contains(&16),
            "V16 must be proposed for cardano-node 10.x interoperability: {version_numbers:?}"
        );
    }

    /// V16 must carry the same capability flags as V15 (no Genesis extensions yet).
    #[test]
    fn test_v16_capabilities_match_v15() {
        let versions = propose_versions(NetworkId::Mainnet);
        let v15 = versions
            .iter()
            .find(|(v, _)| *v == 15)
            .map(|(_, d)| d)
            .expect("V15 must be in proposed versions");
        let v16 = versions
            .iter()
            .find(|(v, _)| *v == 16)
            .map(|(_, d)| d)
            .expect("V16 must be in proposed versions");

        assert_eq!(v15.network_magic, v16.network_magic);
        assert_eq!(v15.initiator_and_responder, v16.initiator_and_responder);
        assert_eq!(v15.peer_sharing, v16.peer_sharing);
        assert_eq!(v15.query, v16.query);
    }

    /// NodeToNodeVersion::V16 discriminant must equal 16 so CBOR wire encoding
    /// produces the correct version number.
    #[test]
    fn test_node_to_node_version_discriminants() {
        assert_eq!(NodeToNodeVersion::V14 as u32, 14);
        assert_eq!(NodeToNodeVersion::V15 as u32, 15);
        assert_eq!(NodeToNodeVersion::V16 as u32, 16);
    }
}
