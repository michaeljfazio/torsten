//! SDU (Segment Data Unit) header encoding/decoding.
//!
//! Wire format (8 bytes, big-endian):
//! ```text
//! Bytes 0-3: transmission_time  u32 BE (microseconds, monotonic)
//! Bytes 4-5: protocol_and_dir   u16 BE (bit 15 = direction, bits 0-14 = protocol number)
//! Bytes 6-7: payload_length     u16 BE
//! ```
//!
//! Reference: `ouroboros-network/network-mux/src/Network/Mux/Codec.hs`
//!
//! Direction bit encoding matches the Haskell implementation:
//! - Initiator (TCP connection originator): bit 15 = 0
//! - Responder (TCP connection acceptor): bit 15 = 1
//!
//! On ingress, the direction bit is flipped — what the remote sends as "InitiatorDir"
//! is received as "ResponderDir" on our side, and vice versa.

/// SDU header size in bytes — fixed by the Ouroboros wire format.
pub const HEADER_SIZE: usize = 8;

/// Direction of a mux segment, determined by which peer initiated the TCP connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// Sent by the TCP connection initiator (bit 15 = 0).
    InitiatorDir,
    /// Sent by the TCP connection responder (bit 15 = 1).
    ResponderDir,
}

impl Direction {
    /// Flip direction (used on ingress — remote's InitiatorDir becomes our ResponderDir).
    pub fn flip(self) -> Self {
        match self {
            Self::InitiatorDir => Self::ResponderDir,
            Self::ResponderDir => Self::InitiatorDir,
        }
    }
}

/// Decoded SDU header — the 8-byte prefix on every multiplexed segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SduHeader {
    /// Monotonic timestamp in microseconds (low 32 bits).
    pub timestamp: u32,
    /// Protocol number (0-14 bits, max 32767). See `protocol/mod.rs` for ID constants.
    pub protocol_id: u16,
    /// Direction: InitiatorDir (bit 15 = 0) or ResponderDir (bit 15 = 1).
    pub direction: Direction,
    /// Length of the payload following this header (bytes).
    pub payload_length: u16,
}

/// Encode an SDU header into its 8-byte big-endian wire representation.
pub fn encode_header(header: &SduHeader) -> [u8; HEADER_SIZE] {
    let mut buf = [0u8; HEADER_SIZE];

    // Bytes 0-3: timestamp (u32 BE)
    buf[0..4].copy_from_slice(&header.timestamp.to_be_bytes());

    // Bytes 4-5: direction bit (15) | protocol number (0-14)
    let dir_bit: u16 = match header.direction {
        Direction::InitiatorDir => 0,
        Direction::ResponderDir => 0x8000,
    };
    let protocol_and_dir = dir_bit | (header.protocol_id & 0x7FFF);
    buf[4..6].copy_from_slice(&protocol_and_dir.to_be_bytes());

    // Bytes 6-7: payload length (u16 BE)
    buf[6..8].copy_from_slice(&header.payload_length.to_be_bytes());

    buf
}

/// Decode an SDU header from its 8-byte big-endian wire representation.
pub fn decode_header(buf: &[u8; HEADER_SIZE]) -> SduHeader {
    let timestamp = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
    let protocol_and_dir = u16::from_be_bytes([buf[4], buf[5]]);
    let payload_length = u16::from_be_bytes([buf[6], buf[7]]);

    let direction = if protocol_and_dir & 0x8000 != 0 {
        Direction::ResponderDir
    } else {
        Direction::InitiatorDir
    };
    let protocol_id = protocol_and_dir & 0x7FFF;

    SduHeader {
        timestamp,
        protocol_id,
        direction,
        payload_length,
    }
}

/// Get the current monotonic timestamp in microseconds (low 32 bits).
///
/// Uses a lazy-initialized epoch so timestamps are relative to process start
/// and fit within a u32 (wraps every ~71 minutes, which is fine — the timestamp
/// is only used for debugging/logging, not protocol correctness).
pub fn current_timestamp() -> u32 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    let epoch = EPOCH.get_or_init(Instant::now);
    epoch.elapsed().as_micros() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_roundtrip_initiator() {
        let header = SduHeader {
            timestamp: 12345678,
            protocol_id: 2, // ChainSync
            direction: Direction::InitiatorDir,
            payload_length: 1024,
        };
        let encoded = encode_header(&header);
        let decoded = decode_header(&encoded);
        assert_eq!(header, decoded);
    }

    #[test]
    fn header_roundtrip_responder() {
        let header = SduHeader {
            timestamp: 0xDEADBEEF,
            protocol_id: 3, // BlockFetch
            direction: Direction::ResponderDir,
            payload_length: 12288,
        };
        let encoded = encode_header(&header);
        let decoded = decode_header(&encoded);
        assert_eq!(header, decoded);
    }

    #[test]
    fn direction_bit_encoding() {
        // InitiatorDir: bit 15 = 0, protocol 2 → 0x0002
        let header = SduHeader {
            timestamp: 0,
            protocol_id: 2,
            direction: Direction::InitiatorDir,
            payload_length: 0,
        };
        let buf = encode_header(&header);
        assert_eq!(buf[4..6], [0x00, 0x02]);

        // ResponderDir: bit 15 = 1, protocol 2 → 0x8002
        let header = SduHeader {
            timestamp: 0,
            protocol_id: 2,
            direction: Direction::ResponderDir,
            payload_length: 0,
        };
        let buf = encode_header(&header);
        assert_eq!(buf[4..6], [0x80, 0x02]);
    }

    #[test]
    fn direction_flip() {
        assert_eq!(Direction::InitiatorDir.flip(), Direction::ResponderDir);
        assert_eq!(Direction::ResponderDir.flip(), Direction::InitiatorDir);
    }

    #[test]
    fn max_protocol_id() {
        // Protocol ID uses bits 0-14, max = 0x7FFF = 32767
        let header = SduHeader {
            timestamp: 0,
            protocol_id: 0x7FFF,
            direction: Direction::InitiatorDir,
            payload_length: 0,
        };
        let decoded = decode_header(&encode_header(&header));
        assert_eq!(decoded.protocol_id, 0x7FFF);
    }

    #[test]
    fn max_payload_length() {
        let header = SduHeader {
            timestamp: 0,
            protocol_id: 0,
            direction: Direction::InitiatorDir,
            payload_length: u16::MAX,
        };
        let decoded = decode_header(&encode_header(&header));
        assert_eq!(decoded.payload_length, u16::MAX);
    }

    #[test]
    fn zero_payload_length() {
        let header = SduHeader {
            timestamp: 0,
            protocol_id: 8, // KeepAlive
            direction: Direction::ResponderDir,
            payload_length: 0,
        };
        let decoded = decode_header(&encode_header(&header));
        assert_eq!(decoded.payload_length, 0);
    }

    #[test]
    fn all_known_protocol_ids_roundtrip() {
        // Verify roundtrip for all known Ouroboros protocol IDs
        for &pid in &[0u16, 2, 3, 4, 5, 6, 7, 8, 9, 10] {
            for dir in [Direction::InitiatorDir, Direction::ResponderDir] {
                let header = SduHeader {
                    timestamp: 42,
                    protocol_id: pid,
                    direction: dir,
                    payload_length: 100,
                };
                let decoded = decode_header(&encode_header(&header));
                assert_eq!(decoded.protocol_id, pid, "protocol {pid} roundtrip failed");
                assert_eq!(
                    decoded.direction, dir,
                    "direction roundtrip failed for protocol {pid}"
                );
            }
        }
    }

    #[test]
    fn timestamp_wraps_at_u32_max() {
        let header = SduHeader {
            timestamp: u32::MAX,
            protocol_id: 2,
            direction: Direction::InitiatorDir,
            payload_length: 100,
        };
        let decoded = decode_header(&encode_header(&header));
        assert_eq!(decoded.timestamp, u32::MAX);
    }
}
