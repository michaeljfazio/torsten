//! N2C (node-to-client) handshake version data codec.
//!
//! Supports protocol versions V16 through V23, matching cardano-node 10.x.
//!
//! ## Bit-15 Version Encoding
//! N2C protocol versions use bit-15 encoding on the wire: the version number
//! is `v | 0x8000` (bit 15 set). For example, V16 = 32784 on the wire.
//! This distinguishes N2C versions from N2N versions in the handshake.
//!
//! Version data wire format: `[network_magic, query]`

use minicbor::{Decoder, Encoder};

/// N2C protocol version numbers (logical values, before bit-15 encoding).
pub const N2C_V16: u16 = 16;
pub const N2C_V17: u16 = 17;
pub const N2C_V18: u16 = 18;
pub const N2C_V19: u16 = 19;
pub const N2C_V20: u16 = 20;
pub const N2C_V21: u16 = 21;
pub const N2C_V22: u16 = 22;
pub const N2C_V23: u16 = 23;

/// All N2C versions we support, in preference order (highest first).
pub const N2C_VERSIONS: &[u16] = &[
    N2C_V23, N2C_V22, N2C_V21, N2C_V20, N2C_V19, N2C_V18, N2C_V17, N2C_V16,
];

/// Encode a logical N2C version to its wire representation (bit-15 set).
pub fn encode_n2c_version(v: u16) -> u16 {
    v | 0x8000
}

/// Decode a wire N2C version to its logical version number (clear bit-15).
pub fn decode_n2c_version(wire: u16) -> u16 {
    wire & 0x7FFF
}

/// Check if a wire version number is an N2C version (bit-15 set).
pub fn is_n2c_version(wire: u16) -> bool {
    wire & 0x8000 != 0
}

/// N2C version data exchanged during handshake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct N2CVersionData {
    /// Network magic identifying the Cardano network.
    pub network_magic: u64,
    /// Whether this is a query-only connection.
    pub query: bool,
}

impl N2CVersionData {
    /// Create version data for a standard (non-query) N2C connection.
    pub fn new(network_magic: u64) -> Self {
        Self {
            network_magic,
            query: false,
        }
    }

    /// Encode as CBOR: `[network_magic, query]`.
    pub fn encode(&self, enc: &mut Encoder<&mut Vec<u8>>) {
        enc.array(2).expect("infallible");
        enc.u64(self.network_magic).expect("infallible");
        enc.bool(self.query).expect("infallible");
    }

    /// Decode from CBOR.
    ///
    /// Accepts both V16 legacy format `[network_magic]` (1 element, query defaults
    /// to false) and V17+ format `[network_magic, query]` (2 elements).
    pub fn decode(dec: &mut Decoder<'_>) -> Result<Self, minicbor::decode::Error> {
        let len = dec.array()?;
        match len {
            Some(1) => {
                // V16 legacy format: [network_magic] — query defaults to false
                let network_magic = dec.u64()?;
                Ok(Self {
                    network_magic,
                    query: false,
                })
            }
            Some(2) => {
                // V17+ format: [network_magic, query]
                let network_magic = dec.u64()?;
                let query = dec.bool()?;
                Ok(Self {
                    network_magic,
                    query,
                })
            }
            _ => Err(minicbor::decode::Error::message(
                "N2C version data must be array(1) or array(2)",
            )),
        }
    }

    /// Compute accepted version data.
    /// Returns `None` if network magic doesn't match.
    pub fn accept(&self, theirs: &Self) -> Option<Self> {
        if self.network_magic != theirs.network_magic {
            return None;
        }
        Some(Self {
            network_magic: self.network_magic,
            query: self.query || theirs.query,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bit15_encoding() {
        assert_eq!(encode_n2c_version(16), 32784); // 16 | 0x8000
        assert_eq!(encode_n2c_version(17), 32785);
        assert_eq!(encode_n2c_version(23), 32791);
    }

    #[test]
    fn bit15_decoding() {
        assert_eq!(decode_n2c_version(32784), 16);
        assert_eq!(decode_n2c_version(32791), 23);
    }

    #[test]
    fn is_n2c_detection() {
        assert!(is_n2c_version(32784)); // V16 on wire
        assert!(!is_n2c_version(14)); // N2N V14
        assert!(!is_n2c_version(15)); // N2N V15
    }

    #[test]
    fn roundtrip() {
        let data = N2CVersionData {
            network_magic: 764824073,
            query: true,
        };
        let mut buf = Vec::new();
        let mut enc = Encoder::new(&mut buf);
        data.encode(&mut enc);
        let mut dec = Decoder::new(&buf);
        let decoded = N2CVersionData::decode(&mut dec).unwrap();
        assert_eq!(data, decoded);
    }

    #[test]
    fn accept_matching_magic() {
        let ours = N2CVersionData::new(2);
        let theirs = N2CVersionData {
            network_magic: 2,
            query: true,
        };
        let accepted = ours.accept(&theirs).unwrap();
        assert_eq!(accepted.network_magic, 2);
        assert!(accepted.query); // OR(false, true)
    }

    #[test]
    fn accept_mismatched_magic() {
        let ours = N2CVersionData::new(2);
        let theirs = N2CVersionData::new(764824073);
        assert!(ours.accept(&theirs).is_none());
    }

    #[test]
    fn decode_v16_single_element_array() {
        // V16 legacy format: [network_magic] with no query field
        let mut buf = Vec::new();
        let mut enc = Encoder::new(&mut buf);
        enc.array(1).expect("infallible");
        enc.u64(764824073).expect("infallible");

        let mut dec = Decoder::new(&buf);
        let data = N2CVersionData::decode(&mut dec).unwrap();
        assert_eq!(data.network_magic, 764824073);
        assert!(!data.query); // defaults to false
    }

    #[test]
    fn decode_v16_two_element_still_works() {
        // Verify the existing 2-element format is unbroken
        let data = N2CVersionData {
            network_magic: 2,
            query: true,
        };
        let mut buf = Vec::new();
        let mut enc = Encoder::new(&mut buf);
        data.encode(&mut enc);
        let mut dec = Decoder::new(&buf);
        let decoded = N2CVersionData::decode(&mut dec).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn decode_v16_rejects_invalid_array_length() {
        // array(3) should be rejected
        let mut buf = Vec::new();
        let mut enc = Encoder::new(&mut buf);
        enc.array(3).expect("infallible");
        enc.u64(2).expect("infallible");
        enc.bool(false).expect("infallible");
        enc.bool(true).expect("infallible");

        let mut dec = Decoder::new(&buf);
        assert!(N2CVersionData::decode(&mut dec).is_err());
    }

    #[test]
    fn all_n2c_wire_versions() {
        // Verify all supported versions encode correctly
        for &v in N2C_VERSIONS {
            let wire = encode_n2c_version(v);
            assert!(is_n2c_version(wire));
            assert_eq!(decode_n2c_version(wire), v);
        }
    }
}
