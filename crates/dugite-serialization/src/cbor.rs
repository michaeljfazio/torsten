use crate::error::SerializationError;
use dugite_primitives::block::Point;
use dugite_primitives::hash::{Hash28, Hash32};
use dugite_primitives::transaction::{PlutusData, TransactionInput, TransactionMetadatum};

/// Encode a Hash32 to CBOR bytes
pub fn encode_hash32(hash: &Hash32) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.push(0x58); // byte string, 1-byte length
    buf.push(32);
    buf.extend_from_slice(hash.as_bytes());
    buf
}

/// Decode a Hash32 from CBOR bytes
pub fn decode_hash32(data: &[u8]) -> Result<(Hash32, usize), SerializationError> {
    if data.len() < 2 {
        return Err(SerializationError::InvalidLength {
            expected: 34,
            got: data.len(),
        });
    }
    match data[0] {
        0x58 => {
            let len = data[1] as usize;
            if len != 32 || data.len() < 2 + 32 {
                return Err(SerializationError::InvalidLength {
                    expected: 32,
                    got: len,
                });
            }
            let mut bytes = [0u8; 32];
            bytes.copy_from_slice(&data[2..34]);
            Ok((Hash32::from_bytes(bytes), 34))
        }
        // Short byte string (length embedded in first byte)
        b if (b & 0xe0) == 0x40 => {
            let len = (b & 0x1f) as usize;
            if len != 32 || data.len() < 1 + 32 {
                return Err(SerializationError::InvalidLength {
                    expected: 32,
                    got: len,
                });
            }
            let mut bytes = [0u8; 32];
            bytes.copy_from_slice(&data[1..33]);
            Ok((Hash32::from_bytes(bytes), 33))
        }
        _ => Err(SerializationError::CborDecode(format!(
            "Expected byte string, got {:#04x}",
            data[0]
        ))),
    }
}

/// Encode a Point to CBOR
pub fn encode_point(point: &Point) -> Vec<u8> {
    match point {
        Point::Origin => {
            // Origin is encoded as CBOR array with tag
            vec![0x82, 0x00, 0x80] // [0, []]
        }
        Point::Specific(slot, hash) => {
            let mut buf = Vec::new();
            buf.push(0x82); // array of 2
                            // Encode slot as unsigned integer
            buf.extend(encode_uint(slot.0));
            // Encode hash as byte string
            buf.extend(encode_hash32(hash));
            buf
        }
    }
}

/// Encode an unsigned integer to CBOR
pub fn encode_uint(value: u64) -> Vec<u8> {
    let mut buf = Vec::new();
    if value < 24 {
        buf.push(value as u8);
    } else if value < 256 {
        buf.push(0x18);
        buf.push(value as u8);
    } else if value < 65536 {
        buf.push(0x19);
        buf.extend_from_slice(&(value as u16).to_be_bytes());
    } else if value < 4294967296 {
        buf.push(0x1a);
        buf.extend_from_slice(&(value as u32).to_be_bytes());
    } else {
        buf.push(0x1b);
        buf.extend_from_slice(&value.to_be_bytes());
    }
    buf
}

/// Encode a signed integer to CBOR
pub fn encode_int(value: i128) -> Vec<u8> {
    if value >= 0 {
        encode_uint(value as u64)
    } else {
        let abs_val = (-1 - value) as u64;
        let mut buf = Vec::new();
        if abs_val < 24 {
            buf.push(0x20 | abs_val as u8);
        } else if abs_val < 256 {
            buf.push(0x38);
            buf.push(abs_val as u8);
        } else if abs_val < 65536 {
            buf.push(0x39);
            buf.extend_from_slice(&(abs_val as u16).to_be_bytes());
        } else if abs_val < 4294967296 {
            buf.push(0x3a);
            buf.extend_from_slice(&(abs_val as u32).to_be_bytes());
        } else {
            buf.push(0x3b);
            buf.extend_from_slice(&abs_val.to_be_bytes());
        }
        buf
    }
}

/// Encode a byte string to CBOR
pub fn encode_bytes(data: &[u8]) -> Vec<u8> {
    let mut buf = Vec::new();
    let len = data.len();
    if len < 24 {
        buf.push(0x40 | len as u8);
    } else if len < 256 {
        buf.push(0x58);
        buf.push(len as u8);
    } else if len < 65536 {
        buf.push(0x59);
        buf.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        buf.push(0x5a);
        buf.extend_from_slice(&(len as u32).to_be_bytes());
    }
    buf.extend_from_slice(data);
    buf
}

/// Encode a text string to CBOR
pub fn encode_text(text: &str) -> Vec<u8> {
    let data = text.as_bytes();
    let mut buf = Vec::new();
    let len = data.len();
    if len < 24 {
        buf.push(0x60 | len as u8);
    } else if len < 256 {
        buf.push(0x78);
        buf.push(len as u8);
    } else if len < 65536 {
        buf.push(0x79);
        buf.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        buf.push(0x7a);
        buf.extend_from_slice(&(len as u32).to_be_bytes());
    }
    buf.extend_from_slice(data);
    buf
}

/// Encode a CBOR array header
pub fn encode_array_header(len: usize) -> Vec<u8> {
    let mut buf = Vec::new();
    if len < 24 {
        buf.push(0x80 | len as u8);
    } else if len < 256 {
        buf.push(0x98);
        buf.push(len as u8);
    } else if len < 65536 {
        buf.push(0x99);
        buf.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        buf.push(0x9a);
        buf.extend_from_slice(&(len as u32).to_be_bytes());
    }
    buf
}

/// Encode a CBOR map header
pub fn encode_map_header(len: usize) -> Vec<u8> {
    let mut buf = Vec::new();
    if len < 24 {
        buf.push(0xa0 | len as u8);
    } else if len < 256 {
        buf.push(0xb8);
        buf.push(len as u8);
    } else if len < 65536 {
        buf.push(0xb9);
        buf.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        buf.push(0xba);
        buf.extend_from_slice(&(len as u32).to_be_bytes());
    }
    buf
}

/// Encode PlutusData to CBOR
pub fn encode_plutus_data(data: &PlutusData) -> Vec<u8> {
    match data {
        PlutusData::Constr(tag, fields) => {
            let mut buf = Vec::new();
            // Use CBOR tag 121 + constructor index for small constructors
            if *tag < 7 {
                let cbor_tag = 121 + tag;
                buf.push(0xd8); // tag (1-byte)
                buf.push(cbor_tag as u8);
            } else if *tag < 128 {
                let cbor_tag = 1280 + (tag - 7);
                buf.push(0xd9); // tag (2-byte)
                buf.extend_from_slice(&(cbor_tag as u16).to_be_bytes());
            } else {
                // General form: tag 102 wrapping [constructor, fields]
                buf.push(0xd8); // tag (1-byte)
                buf.push(0x66); // tag 102
                                // Encode as [constructor_index, [fields...]]
                let mut inner = encode_array_header(2);
                inner.extend(encode_uint(*tag));
                inner.extend(encode_array_header(fields.len()));
                for field in fields {
                    inner.extend(encode_plutus_data(field));
                }
                buf.extend(inner);
                return buf;
            }
            buf.extend(encode_array_header(fields.len()));
            for field in fields {
                buf.extend(encode_plutus_data(field));
            }
            buf
        }
        PlutusData::Map(entries) => {
            let mut buf = encode_map_header(entries.len());
            for (k, v) in entries {
                buf.extend(encode_plutus_data(k));
                buf.extend(encode_plutus_data(v));
            }
            buf
        }
        PlutusData::List(items) => {
            let mut buf = encode_array_header(items.len());
            for item in items {
                buf.extend(encode_plutus_data(item));
            }
            buf
        }
        PlutusData::Integer(n) => encode_int(*n),
        PlutusData::Bytes(b) => encode_bytes(b),
    }
}

/// Encode a TransactionInput to CBOR [tx_hash, index]
pub fn encode_tx_input(input: &TransactionInput) -> Vec<u8> {
    let mut buf = encode_array_header(2);
    buf.extend(encode_hash32(&input.transaction_id));
    buf.extend(encode_uint(input.index as u64));
    buf
}

/// Encode a Hash28 to CBOR bytes
pub fn encode_hash28(hash: &Hash28) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.push(0x58); // byte string, 1-byte length
    buf.push(28);
    buf.extend_from_slice(hash.as_bytes());
    buf
}

/// Encode a CBOR tag
pub fn encode_tag(tag: u64) -> Vec<u8> {
    let mut buf = Vec::new();
    if tag < 24 {
        buf.push(0xc0 | tag as u8);
    } else if tag < 256 {
        buf.push(0xd8);
        buf.push(tag as u8);
    } else if tag < 65536 {
        buf.push(0xd9);
        buf.extend_from_slice(&(tag as u16).to_be_bytes());
    } else {
        buf.push(0xda);
        buf.extend_from_slice(&(tag as u32).to_be_bytes());
    }
    buf
}

/// Encode a CBOR bool
pub fn encode_bool(value: bool) -> Vec<u8> {
    vec![if value { 0xf5 } else { 0xf4 }]
}

/// Encode CBOR null
pub fn encode_null() -> Vec<u8> {
    vec![0xf6]
}

/// Encode transaction metadata to CBOR
pub fn encode_metadatum(metadatum: &TransactionMetadatum) -> Vec<u8> {
    match metadatum {
        TransactionMetadatum::Int(n) => encode_int(*n),
        TransactionMetadatum::Bytes(b) => encode_bytes(b),
        TransactionMetadatum::Text(t) => encode_text(t),
        TransactionMetadatum::List(items) => {
            let mut buf = encode_array_header(items.len());
            for item in items {
                buf.extend(encode_metadatum(item));
            }
            buf
        }
        TransactionMetadatum::Map(entries) => {
            let mut buf = encode_map_header(entries.len());
            for (k, v) in entries {
                buf.extend(encode_metadatum(k));
                buf.extend(encode_metadatum(v));
            }
            buf
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dugite_primitives::time::SlotNo;

    #[test]
    fn test_encode_uint_small() {
        assert_eq!(encode_uint(0), vec![0x00]);
        assert_eq!(encode_uint(1), vec![0x01]);
        assert_eq!(encode_uint(23), vec![0x17]);
    }

    #[test]
    fn test_encode_uint_one_byte() {
        assert_eq!(encode_uint(24), vec![0x18, 0x18]);
        assert_eq!(encode_uint(255), vec![0x18, 0xff]);
    }

    #[test]
    fn test_encode_uint_two_bytes() {
        assert_eq!(encode_uint(256), vec![0x19, 0x01, 0x00]);
        assert_eq!(encode_uint(1000), vec![0x19, 0x03, 0xe8]);
    }

    #[test]
    fn test_encode_uint_four_bytes() {
        assert_eq!(encode_uint(1_000_000), vec![0x1a, 0x00, 0x0f, 0x42, 0x40]);
    }

    #[test]
    fn test_encode_negative_int() {
        assert_eq!(encode_int(-1), vec![0x20]);
        assert_eq!(encode_int(-10), vec![0x29]);
        assert_eq!(encode_int(-100), vec![0x38, 0x63]);
    }

    #[test]
    fn test_encode_bytes() {
        let data = vec![0x01, 0x02, 0x03];
        let encoded = encode_bytes(&data);
        assert_eq!(encoded[0], 0x43); // byte string of length 3
        assert_eq!(&encoded[1..], &data);
    }

    #[test]
    fn test_encode_text() {
        let encoded = encode_text("hello");
        assert_eq!(encoded[0], 0x65); // text string of length 5
        assert_eq!(&encoded[1..], b"hello");
    }

    #[test]
    fn test_encode_array_header() {
        assert_eq!(encode_array_header(0), vec![0x80]);
        assert_eq!(encode_array_header(3), vec![0x83]);
        assert_eq!(encode_array_header(24), vec![0x98, 0x18]);
    }

    #[test]
    fn test_encode_map_header() {
        assert_eq!(encode_map_header(0), vec![0xa0]);
        assert_eq!(encode_map_header(2), vec![0xa2]);
    }

    #[test]
    fn test_encode_hash32() {
        let hash = Hash32::ZERO;
        let encoded = encode_hash32(&hash);
        assert_eq!(encoded.len(), 34); // 2 byte header + 32 bytes
        assert_eq!(encoded[0], 0x58);
        assert_eq!(encoded[1], 32);
    }

    #[test]
    fn test_encode_point_origin() {
        let point = Point::Origin;
        let encoded = encode_point(&point);
        assert_eq!(encoded, vec![0x82, 0x00, 0x80]);
    }

    #[test]
    fn test_encode_point_specific() {
        let point = Point::Specific(SlotNo(100), Hash32::ZERO);
        let encoded = encode_point(&point);
        assert_eq!(encoded[0], 0x82); // array of 2
        assert_eq!(encoded[1], 0x18); // uint 100
        assert_eq!(encoded[2], 100);
    }

    #[test]
    fn test_encode_plutus_data_integer() {
        let data = PlutusData::Integer(42);
        let encoded = encode_plutus_data(&data);
        assert_eq!(encoded, vec![0x18, 42]);
    }

    #[test]
    fn test_encode_plutus_data_bytes() {
        let data = PlutusData::Bytes(vec![0xde, 0xad]);
        let encoded = encode_plutus_data(&data);
        assert_eq!(encoded, vec![0x42, 0xde, 0xad]);
    }

    #[test]
    fn test_encode_plutus_data_list() {
        let data = PlutusData::List(vec![PlutusData::Integer(1), PlutusData::Integer(2)]);
        let encoded = encode_plutus_data(&data);
        assert_eq!(encoded, vec![0x82, 0x01, 0x02]);
    }

    #[test]
    fn test_encode_plutus_data_constr() {
        let data = PlutusData::Constr(0, vec![PlutusData::Integer(1)]);
        let encoded = encode_plutus_data(&data);
        assert_eq!(encoded[0], 0xd8); // tag
        assert_eq!(encoded[1], 121); // constructor 0 = tag 121
        assert_eq!(encoded[2], 0x81); // array of 1
        assert_eq!(encoded[3], 0x01); // integer 1
    }

    #[test]
    fn test_encode_tx_input() {
        let input = TransactionInput {
            transaction_id: Hash32::ZERO,
            index: 0,
        };
        let encoded = encode_tx_input(&input);
        assert_eq!(encoded[0], 0x82); // array of 2
    }

    #[test]
    fn test_encode_metadatum_text() {
        let meta = TransactionMetadatum::Text("hello".to_string());
        let encoded = encode_metadatum(&meta);
        assert_eq!(encoded[0], 0x65);
        assert_eq!(&encoded[1..], b"hello");
    }

    #[test]
    fn test_encode_metadatum_int() {
        let meta = TransactionMetadatum::Int(42);
        let encoded = encode_metadatum(&meta);
        assert_eq!(encoded, vec![0x18, 42]);
    }

    #[test]
    fn test_encode_metadatum_map() {
        let meta = TransactionMetadatum::Map(vec![(
            TransactionMetadatum::Text("key".to_string()),
            TransactionMetadatum::Int(1),
        )]);
        let encoded = encode_metadatum(&meta);
        assert_eq!(encoded[0], 0xa1); // map of 1
    }

    #[test]
    fn test_encode_plutus_constr_small() {
        // Constructors 0-6 use CBOR tags 121-127
        for tag in 0..7u64 {
            let data = PlutusData::Constr(tag, vec![]);
            let encoded = encode_plutus_data(&data);
            assert_eq!(encoded[0], 0xd8);
            assert_eq!(encoded[1], (121 + tag) as u8);
            assert_eq!(encoded[2], 0x80); // empty array
        }
    }

    #[test]
    fn test_encode_plutus_constr_medium() {
        // Constructors 7-127 use CBOR tags 1280+
        let data = PlutusData::Constr(7, vec![PlutusData::Integer(1)]);
        let encoded = encode_plutus_data(&data);
        assert_eq!(encoded[0], 0xd9); // 2-byte tag
        let tag_val = u16::from_be_bytes([encoded[1], encoded[2]]);
        assert_eq!(tag_val, 1280); // 1280 + (7 - 7) = 1280
    }

    #[test]
    fn test_encode_plutus_constr_large_uses_tag_102() {
        // Constructors >= 128 must use CBOR tag 102 (NOT tag 258)
        let data = PlutusData::Constr(128, vec![PlutusData::Integer(99)]);
        let encoded = encode_plutus_data(&data);

        // Tag 102 = 0xd8 0x66
        assert_eq!(encoded[0], 0xd8, "should use 1-byte CBOR tag prefix");
        assert_eq!(encoded[1], 0x66, "should use tag 102 (0x66)");

        // After tag: array(2) with [constructor_index, fields_array]
        assert_eq!(encoded[2], 0x82); // array of 2

        // Constructor index 128 = 0x18 0x80
        assert_eq!(encoded[3], 0x18);
        assert_eq!(encoded[4], 128);

        // Fields array(1) with integer 99
        assert_eq!(encoded[5], 0x81);
        assert_eq!(encoded[6], 0x18);
        assert_eq!(encoded[7], 99);
    }

    #[test]
    fn test_encode_plutus_constr_256_tag_102() {
        let data = PlutusData::Constr(256, vec![]);
        let encoded = encode_plutus_data(&data);
        assert_eq!(encoded[0], 0xd8);
        assert_eq!(encoded[1], 0x66); // tag 102
        assert_eq!(encoded[2], 0x82); // array of 2
                                      // Constructor 256 = 0x19 0x01 0x00
        assert_eq!(encoded[3], 0x19);
        assert_eq!(encoded[4], 0x01);
        assert_eq!(encoded[5], 0x00);
        assert_eq!(encoded[6], 0x80); // empty fields array
    }
}
