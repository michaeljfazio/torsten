//! N2N (node-to-node) handshake version data codec.
//!
//! Supports protocol versions V14 and V15, matching cardano-node 10.x.
//!
//! Version data wire format (CBOR array):
//! ```text
//! [network_magic, initiator_only_diffusion_mode, peer_sharing, query]
//! ```
//!
//! Acceptance rules (matching Haskell `acceptableVersion`):
//! - `network_magic`: must match exactly
//! - `initiator_only`: min(ours, theirs) — degrade to unidirectional if either side requests it
//! - `peer_sharing`: AND(ours, theirs) — both must opt in
//! - `query`: OR(ours, theirs) — either side can request query mode

use minicbor::{Decoder, Encoder};

/// N2N protocol version numbers.
pub const N2N_V14: u16 = 14;
pub const N2N_V15: u16 = 15;

/// All N2N versions we support, in preference order (highest first).
pub const N2N_VERSIONS: &[u16] = &[N2N_V15, N2N_V14];

/// N2N version data exchanged during handshake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct N2NVersionData {
    /// Network magic identifying the Cardano network (e.g., 764824073 for mainnet, 2 for preview).
    pub network_magic: u64,
    /// If true, this node only initiates connections (never accepts inbound).
    pub initiator_only: bool,
    /// Whether this node supports peer sharing.
    pub peer_sharing: bool,
    /// Whether this is a query-only connection (no block sync).
    pub query: bool,
}

impl N2NVersionData {
    /// Create version data for a full (non-query) node connection.
    pub fn new(network_magic: u64, peer_sharing: bool) -> Self {
        Self {
            network_magic,
            initiator_only: false,
            peer_sharing,
            query: false,
        }
    }

    /// Encode as CBOR: `[network_magic, initiator_only, peer_sharing, query]`.
    pub fn encode(&self, enc: &mut Encoder<&mut Vec<u8>>) {
        enc.array(4).expect("infallible");
        enc.u64(self.network_magic).expect("infallible");
        enc.bool(self.initiator_only).expect("infallible");
        // peer_sharing: 0 = disabled, 1 = enabled
        enc.u8(if self.peer_sharing { 1 } else { 0 })
            .expect("infallible");
        enc.bool(self.query).expect("infallible");
    }

    /// Decode from CBOR.
    pub fn decode(dec: &mut Decoder<'_>) -> Result<Self, minicbor::decode::Error> {
        let len = dec.array()?;
        if len != Some(4) {
            return Err(minicbor::decode::Error::message(
                "N2N version data must be array(4)",
            ));
        }
        let network_magic = dec.u64()?;
        let initiator_only = dec.bool()?;
        let peer_sharing_val = dec.u8()?;
        let query = dec.bool()?;
        Ok(Self {
            network_magic,
            initiator_only,
            peer_sharing: peer_sharing_val != 0,
            query,
        })
    }

    /// Compute accepted version data from our data and the remote's.
    ///
    /// Returns `None` if network magic doesn't match.
    /// Otherwise applies the Haskell acceptance rules:
    /// - `initiator_only`: min(ours, theirs)
    /// - `peer_sharing`: AND
    /// - `query`: OR
    pub fn accept(&self, theirs: &Self) -> Option<Self> {
        if self.network_magic != theirs.network_magic {
            return None;
        }
        Some(Self {
            network_magic: self.network_magic,
            // min(ours, theirs) — if either side is initiator-only, use initiator-only
            initiator_only: self.initiator_only || theirs.initiator_only,
            // AND — both must opt in for peer sharing
            peer_sharing: self.peer_sharing && theirs.peer_sharing,
            // OR — either side can request query mode
            query: self.query || theirs.query,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let data = N2NVersionData {
            network_magic: 2,
            initiator_only: false,
            peer_sharing: true,
            query: false,
        };
        let mut buf = Vec::new();
        let mut enc = Encoder::new(&mut buf);
        data.encode(&mut enc);
        let mut dec = Decoder::new(&buf);
        let decoded = N2NVersionData::decode(&mut dec).unwrap();
        assert_eq!(data, decoded);
    }

    #[test]
    fn accept_matching_magic() {
        let ours = N2NVersionData::new(2, true);
        let theirs = N2NVersionData {
            network_magic: 2,
            initiator_only: false,
            peer_sharing: true,
            query: true,
        };
        let accepted = ours.accept(&theirs).unwrap();
        assert_eq!(accepted.network_magic, 2);
        assert!(!accepted.initiator_only); // min(false, false) = false
        assert!(accepted.peer_sharing); // AND(true, true)
        assert!(accepted.query); // OR(false, true)
    }

    #[test]
    fn accept_mismatched_magic() {
        let ours = N2NVersionData::new(2, true);
        let theirs = N2NVersionData::new(764824073, true);
        assert!(ours.accept(&theirs).is_none());
    }

    #[test]
    fn accept_initiator_only_takes_min() {
        let ours = N2NVersionData {
            network_magic: 2,
            initiator_only: true,
            peer_sharing: false,
            query: false,
        };
        let theirs = N2NVersionData::new(2, false);
        let accepted = ours.accept(&theirs).unwrap();
        // min means "degrade to initiator_only if either side sets it"
        assert!(accepted.initiator_only);
    }

    #[test]
    fn accept_peer_sharing_and() {
        let ours = N2NVersionData::new(2, true);
        let theirs = N2NVersionData::new(2, false);
        let accepted = ours.accept(&theirs).unwrap();
        assert!(!accepted.peer_sharing); // AND(true, false) = false
    }
}
