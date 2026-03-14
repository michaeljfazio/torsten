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
    /// Encode a segment to bytes.
    ///
    /// If the payload exceeds MAX_SEGMENT_PAYLOAD (65535 bytes), it is
    /// automatically chunked into multiple consecutive wire segments per
    /// the Ouroboros multiplexer specification.
    pub fn encode(&self) -> Vec<u8> {
        let mut protocol_word = self.protocol_id;
        if self.is_responder {
            protocol_word |= 0x8000;
        }
        let proto_bytes = protocol_word.to_be_bytes();
        let time_bytes = self.transmission_time.to_be_bytes();

        if self.payload.len() <= MAX_SEGMENT_PAYLOAD {
            let mut buf = Vec::with_capacity(SEGMENT_HEADER_SIZE + self.payload.len());
            buf.extend_from_slice(&time_bytes);
            buf.extend_from_slice(&proto_bytes);
            buf.extend_from_slice(&(self.payload.len() as u16).to_be_bytes());
            buf.extend_from_slice(&self.payload);
            return buf;
        }

        // Chunk payload into multiple segments of at most MAX_SEGMENT_PAYLOAD bytes
        let num_chunks = self.payload.len().div_ceil(MAX_SEGMENT_PAYLOAD);
        let mut buf = Vec::with_capacity(num_chunks * SEGMENT_HEADER_SIZE + self.payload.len());
        for chunk in self.payload.chunks(MAX_SEGMENT_PAYLOAD) {
            buf.extend_from_slice(&time_bytes);
            buf.extend_from_slice(&proto_bytes);
            buf.extend_from_slice(&(chunk.len() as u16).to_be_bytes());
            buf.extend_from_slice(chunk);
        }
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

    #[test]
    fn test_small_payload_no_chunking() {
        let payload = vec![0xAB; 100];
        let segment = Segment {
            transmission_time: 42,
            protocol_id: 2,
            is_responder: true,
            payload: payload.clone(),
        };

        let encoded = segment.encode();
        // Single segment: 8-byte header + 100 bytes payload
        assert_eq!(encoded.len(), SEGMENT_HEADER_SIZE + 100);

        let (decoded, consumed) = Segment::decode(&encoded).unwrap();
        assert_eq!(consumed, encoded.len());
        assert_eq!(decoded.payload, payload);
    }

    #[test]
    fn test_max_payload_no_chunking() {
        let payload = vec![0xCD; MAX_SEGMENT_PAYLOAD];
        let segment = Segment {
            transmission_time: 0,
            protocol_id: 3,
            is_responder: false,
            payload: payload.clone(),
        };

        let encoded = segment.encode();
        // Single segment: 8-byte header + 65535 bytes
        assert_eq!(encoded.len(), SEGMENT_HEADER_SIZE + MAX_SEGMENT_PAYLOAD);

        let (decoded, consumed) = Segment::decode(&encoded).unwrap();
        assert_eq!(consumed, encoded.len());
        assert_eq!(decoded.payload, payload);
    }

    #[test]
    fn test_oversized_payload_is_chunked() {
        let payload_size = MAX_SEGMENT_PAYLOAD + 100;
        let payload = vec![0xEF; payload_size];
        let segment = Segment {
            transmission_time: 7,
            protocol_id: 5,
            is_responder: true,
            payload: payload.clone(),
        };

        let encoded = segment.encode();
        // Should produce 2 chunks: 65535 + 100
        let expected_len = 2 * SEGMENT_HEADER_SIZE + MAX_SEGMENT_PAYLOAD + 100;
        assert_eq!(encoded.len(), expected_len);

        // Decode first chunk
        let (chunk1, consumed1) = Segment::decode(&encoded).unwrap();
        assert_eq!(chunk1.payload.len(), MAX_SEGMENT_PAYLOAD);
        assert_eq!(chunk1.protocol_id, 5);
        assert!(chunk1.is_responder);
        assert_eq!(chunk1.transmission_time, 7);

        // Decode second chunk
        let (chunk2, consumed2) = Segment::decode(&encoded[consumed1..]).unwrap();
        assert_eq!(chunk2.payload.len(), 100);
        assert_eq!(chunk2.protocol_id, 5);
        assert!(chunk2.is_responder);

        // Reassemble
        let mut reassembled = chunk1.payload;
        reassembled.extend_from_slice(&chunk2.payload);
        assert_eq!(reassembled, payload);

        assert_eq!(consumed1 + consumed2, encoded.len());
    }

    #[test]
    fn test_exact_multiple_of_max_payload() {
        let payload = vec![0x11; MAX_SEGMENT_PAYLOAD * 3];
        let segment = Segment {
            transmission_time: 0,
            protocol_id: 2,
            is_responder: false,
            payload: payload.clone(),
        };

        let encoded = segment.encode();
        assert_eq!(
            encoded.len(),
            3 * (SEGMENT_HEADER_SIZE + MAX_SEGMENT_PAYLOAD)
        );

        // Decode all 3 chunks and reassemble
        let mut offset = 0;
        let mut reassembled = Vec::new();
        for i in 0..3 {
            let (chunk, consumed) = Segment::decode(&encoded[offset..]).unwrap();
            assert_eq!(chunk.payload.len(), MAX_SEGMENT_PAYLOAD, "chunk {i}");
            reassembled.extend_from_slice(&chunk.payload);
            offset += consumed;
        }
        assert_eq!(reassembled, payload);
        assert_eq!(offset, encoded.len());
    }

    #[test]
    fn test_large_payload_chunking_preserves_protocol_id() {
        // Verify protocol ID is the same in every chunk
        let payload_size = MAX_SEGMENT_PAYLOAD * 2 + 500;
        let payload = vec![0xFE; payload_size];
        let segment = Segment {
            transmission_time: 99,
            protocol_id: 7, // arbitrary protocol ID
            is_responder: false,
            payload: payload.clone(),
        };

        let encoded = segment.encode();
        let mut offset = 0;
        let mut chunk_count = 0;
        let mut reassembled = Vec::new();

        while offset < encoded.len() {
            let (chunk, consumed) = Segment::decode(&encoded[offset..]).unwrap();
            assert_eq!(chunk.protocol_id, 7, "chunk {chunk_count} protocol_id");
            assert_eq!(chunk.transmission_time, 99, "chunk {chunk_count} timestamp");
            assert!(!chunk.is_responder, "chunk {chunk_count} responder flag");
            reassembled.extend_from_slice(&chunk.payload);
            offset += consumed;
            chunk_count += 1;
        }

        assert_eq!(chunk_count, 3); // 65535 + 65535 + 500
        assert_eq!(reassembled, payload);
    }

    #[test]
    fn test_responder_flag_bit15_encoding() {
        // Verify bit 15 is correctly set in the wire format
        let segment = Segment {
            transmission_time: 0,
            protocol_id: 5,
            is_responder: true,
            payload: vec![0x42],
        };

        let encoded = segment.encode();
        // Protocol word is at bytes 4-5 (big-endian)
        let protocol_word = u16::from_be_bytes([encoded[4], encoded[5]]);
        assert_eq!(
            protocol_word & 0x8000,
            0x8000,
            "Bit 15 must be set for responder"
        );
        assert_eq!(protocol_word & 0x7FFF, 5, "Protocol ID must be preserved");

        // Non-responder should have bit 15 clear
        let segment2 = Segment {
            transmission_time: 0,
            protocol_id: 5,
            is_responder: false,
            payload: vec![0x42],
        };
        let encoded2 = segment2.encode();
        let protocol_word2 = u16::from_be_bytes([encoded2[4], encoded2[5]]);
        assert_eq!(
            protocol_word2 & 0x8000,
            0,
            "Bit 15 must be clear for initiator"
        );
    }

    #[test]
    fn test_exact_boundary_payload_65535() {
        // Exactly MAX_SEGMENT_PAYLOAD bytes should NOT be chunked
        let payload = vec![0xAA; MAX_SEGMENT_PAYLOAD];
        let segment = Segment {
            transmission_time: 0,
            protocol_id: 1,
            is_responder: false,
            payload: payload.clone(),
        };

        let encoded = segment.encode();
        // Should be exactly one segment
        assert_eq!(encoded.len(), SEGMENT_HEADER_SIZE + MAX_SEGMENT_PAYLOAD);

        // Verify the length field in the header
        let payload_len = u16::from_be_bytes([encoded[6], encoded[7]]);
        assert_eq!(payload_len, MAX_SEGMENT_PAYLOAD as u16);

        let (decoded, consumed) = Segment::decode(&encoded).unwrap();
        assert_eq!(consumed, encoded.len());
        assert_eq!(decoded.payload.len(), MAX_SEGMENT_PAYLOAD);
    }

    #[test]
    fn test_boundary_plus_one_triggers_chunking() {
        // MAX_SEGMENT_PAYLOAD + 1 should be chunked into 2 segments
        let payload = vec![0xBB; MAX_SEGMENT_PAYLOAD + 1];
        let segment = Segment {
            transmission_time: 0,
            protocol_id: 2,
            is_responder: false,
            payload: payload.clone(),
        };

        let encoded = segment.encode();
        // Two segments: first with 65535 bytes, second with 1 byte
        let expected_len = 2 * SEGMENT_HEADER_SIZE + MAX_SEGMENT_PAYLOAD + 1;
        assert_eq!(encoded.len(), expected_len);

        let (chunk1, consumed1) = Segment::decode(&encoded).unwrap();
        assert_eq!(chunk1.payload.len(), MAX_SEGMENT_PAYLOAD);

        let (chunk2, consumed2) = Segment::decode(&encoded[consumed1..]).unwrap();
        assert_eq!(chunk2.payload.len(), 1);
        assert_eq!(consumed1 + consumed2, encoded.len());
    }

    #[test]
    fn test_decode_truncated_payload() {
        // Header says 100 bytes but only 50 bytes of payload present
        let mut data = vec![0u8; SEGMENT_HEADER_SIZE + 50];
        // Set payload length to 100 in header
        let len_bytes = 100u16.to_be_bytes();
        data[6] = len_bytes[0];
        data[7] = len_bytes[1];

        let result = Segment::decode(&data);
        assert!(result.is_err());
    }

    #[test]
    fn test_all_protocol_ids_roundtrip() {
        // Verify various protocol IDs survive encode/decode
        for proto_id in [0u16, 1, 2, 3, 4, 5, 6, 9, 0x7FFF] {
            let segment = Segment {
                transmission_time: 0,
                protocol_id: proto_id,
                is_responder: false,
                payload: vec![0x01],
            };

            let encoded = segment.encode();
            let (decoded, _) = Segment::decode(&encoded).unwrap();
            assert_eq!(
                decoded.protocol_id, proto_id,
                "Protocol ID {proto_id} roundtrip failed"
            );
        }
    }
}
