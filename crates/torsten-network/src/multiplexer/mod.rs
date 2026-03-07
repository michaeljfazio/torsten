use thiserror::Error;

/// Ouroboros network multiplexer
///
/// The multiplexer allows multiple mini-protocols to share a single
/// TCP or Unix domain socket connection. Each segment has a header
/// indicating which mini-protocol it belongs to.
///
/// Multiplexer segment header (8 bytes)
/// Format:
///   - transmission_time: u32 (4 bytes, network byte order)
///   - mini_protocol_id:  u16 (2 bytes, network byte order)
///     Bit 15 indicates direction (0=initiator, 1=responder)
///   - payload_length:    u16 (2 bytes, network byte order)
pub const SEGMENT_HEADER_SIZE: usize = 8;
pub const MAX_SEGMENT_PAYLOAD: usize = 65535;

#[derive(Error, Debug)]
pub enum MuxError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid segment header")]
    InvalidHeader,
    #[error("Payload too large: {0} bytes")]
    PayloadTooLarge(usize),
    #[error("Unknown mini-protocol: {0}")]
    UnknownProtocol(u16),
}

/// A multiplexer segment
#[derive(Debug, Clone)]
pub struct Segment {
    pub transmission_time: u32,
    pub protocol_id: u16,
    pub is_responder: bool,
    pub payload: Vec<u8>,
}

impl Segment {
    /// Encode a segment to bytes
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(SEGMENT_HEADER_SIZE + self.payload.len());
        buf.extend_from_slice(&self.transmission_time.to_be_bytes());

        let mut protocol_word = self.protocol_id;
        if self.is_responder {
            protocol_word |= 0x8000;
        }
        buf.extend_from_slice(&protocol_word.to_be_bytes());
        buf.extend_from_slice(&(self.payload.len() as u16).to_be_bytes());
        buf.extend_from_slice(&self.payload);
        buf
    }

    /// Decode a segment from bytes
    pub fn decode(data: &[u8]) -> Result<(Self, usize), MuxError> {
        if data.len() < SEGMENT_HEADER_SIZE {
            return Err(MuxError::InvalidHeader);
        }

        let transmission_time = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
        let protocol_word = u16::from_be_bytes([data[4], data[5]]);
        let payload_length = u16::from_be_bytes([data[6], data[7]]) as usize;

        let is_responder = (protocol_word & 0x8000) != 0;
        let protocol_id = protocol_word & 0x7FFF;

        let total_length = SEGMENT_HEADER_SIZE + payload_length;
        if data.len() < total_length {
            return Err(MuxError::InvalidHeader);
        }

        let payload = data[SEGMENT_HEADER_SIZE..total_length].to_vec();

        Ok((
            Segment {
                transmission_time,
                protocol_id,
                is_responder,
                payload,
            },
            total_length,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_segment_encode_decode_roundtrip() {
        let segment = Segment {
            transmission_time: 12345,
            protocol_id: 2, // ChainSync
            is_responder: false,
            payload: vec![0x01, 0x02, 0x03],
        };

        let encoded = segment.encode();
        assert_eq!(encoded.len(), SEGMENT_HEADER_SIZE + 3);

        let (decoded, consumed) = Segment::decode(&encoded).unwrap();
        assert_eq!(consumed, encoded.len());
        assert_eq!(decoded.transmission_time, 12345);
        assert_eq!(decoded.protocol_id, 2);
        assert!(!decoded.is_responder);
        assert_eq!(decoded.payload, vec![0x01, 0x02, 0x03]);
    }

    #[test]
    fn test_responder_flag() {
        let segment = Segment {
            transmission_time: 0,
            protocol_id: 3,
            is_responder: true,
            payload: vec![],
        };

        let encoded = segment.encode();
        let (decoded, _) = Segment::decode(&encoded).unwrap();
        assert!(decoded.is_responder);
        assert_eq!(decoded.protocol_id, 3);
    }

    #[test]
    fn test_decode_too_short() {
        let result = Segment::decode(&[0u8; 4]);
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_payload() {
        let segment = Segment {
            transmission_time: 0,
            protocol_id: 0,
            is_responder: false,
            payload: vec![],
        };

        let encoded = segment.encode();
        assert_eq!(encoded.len(), SEGMENT_HEADER_SIZE);

        let (decoded, consumed) = Segment::decode(&encoded).unwrap();
        assert_eq!(consumed, SEGMENT_HEADER_SIZE);
        assert!(decoded.payload.is_empty());
    }
}
