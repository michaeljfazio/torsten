//! Shared CBOR encoding/decoding helpers for Ouroboros wire format.
//!
//! All encoding matches the Haskell cardano-node EncCBOR/DecCBOR instances.
//! - Point = `[]` for Origin, `[slot, hash]` for Specific.
//! - Tip = `[[slot, hash], block_number]`.
//!
//! Also provides [`try_decode_cbor_boundary`] for detecting complete CBOR values
//! in byte buffers, used by the multiplexer for message boundary detection.

use minicbor::{Decoder, Encoder};

/// A chain point: either the Origin (genesis) or a specific (slot, block header hash).
///
/// Wire format matches Haskell's `Point` type:
/// - Origin: empty CBOR array `[]`
/// - Specific: two-element CBOR array `[slot, hash_bytes]`
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Point {
    /// The genesis/origin point (before any blocks).
    Origin,
    /// A specific point identified by slot number and 32-byte block header hash.
    Specific(u64, [u8; 32]),
}

/// Encode a [`Point`] as CBOR.
///
/// Origin encodes as an empty array `[]`. Specific encodes as `[slot, hash_bytes]`.
/// Encoding to a `Vec<u8>` is infallible — unwrap is safe here.
pub fn encode_point(enc: &mut Encoder<&mut Vec<u8>>, point: &Point) {
    match point {
        Point::Origin => {
            enc.array(0).expect("infallible");
        }
        Point::Specific(slot, hash) => {
            enc.array(2).expect("infallible");
            enc.u64(*slot).expect("infallible");
            enc.bytes(hash).expect("infallible");
        }
    }
}

/// Decode a [`Point`] from CBOR.
///
/// Expects an array of length 0 (Origin) or 2 (Specific with slot + 32-byte hash).
pub fn decode_point(dec: &mut Decoder<'_>) -> Result<Point, minicbor::decode::Error> {
    let len = dec.array()?;
    match len {
        Some(0) => Ok(Point::Origin),
        Some(2) => {
            let slot = dec.u64()?;
            let hash_bytes = dec.bytes()?;
            if hash_bytes.len() != 32 {
                return Err(minicbor::decode::Error::message(
                    "point hash must be 32 bytes",
                ));
            }
            let mut hash = [0u8; 32];
            hash.copy_from_slice(hash_bytes);
            Ok(Point::Specific(slot, hash))
        }
        _ => Err(minicbor::decode::Error::message(
            "invalid point array length",
        )),
    }
}

/// Encode a chain tip as CBOR: `[[slot, hash], block_number]`.
///
/// Matches the Haskell `Tip` encoding where the point is nested as a sub-array.
/// Encoding to a `Vec<u8>` is infallible — unwrap is safe here.
pub fn encode_tip(enc: &mut Encoder<&mut Vec<u8>>, slot: u64, hash: &[u8; 32], block_number: u64) {
    // Outer array: [point, block_number]
    enc.array(2).expect("infallible");
    // Inner point array: [slot, hash]
    enc.array(2).expect("infallible");
    enc.u64(slot).expect("infallible");
    enc.bytes(hash).expect("infallible");
    // Block number follows the point
    enc.u64(block_number).expect("infallible");
}

/// Decode a chain tip from CBOR. Returns `(slot, hash, block_number)`.
///
/// Expects the Haskell `Tip` encoding: `[[slot, hash_bytes], block_number]`.
pub fn decode_tip(dec: &mut Decoder<'_>) -> Result<(u64, [u8; 32], u64), minicbor::decode::Error> {
    // Outer array: [point, block_number]
    dec.array()?;
    // Inner point: either [slot, hash] for Specific, or [] for Origin.
    let arr_len = dec.array()?;
    if arr_len == Some(0) {
        // Origin tip — peer is at genesis with no blocks.
        let block_number = dec.u64()?;
        return Ok((0, [0u8; 32], block_number));
    }
    let slot = dec.u64()?;
    let hash_bytes = dec.bytes()?;
    if hash_bytes.len() != 32 {
        return Err(minicbor::decode::Error::message(
            "tip hash must be 32 bytes",
        ));
    }
    let mut hash = [0u8; 32];
    hash.copy_from_slice(hash_bytes);
    let block_number = dec.u64()?;
    Ok((slot, hash, block_number))
}

/// Try to find a complete CBOR value at the start of a byte buffer.
///
/// Returns `Some(consumed_bytes)` if a complete CBOR data item was found starting
/// at position 0, or `None` if the buffer contains an incomplete item (needs more data).
///
/// This is used by MuxChannel for message boundary detection — the multiplexer
/// may deliver partial CBOR across SDU segments, so we need to detect when a
/// complete message has been assembled before dispatching it.
pub fn try_decode_cbor_boundary(data: &[u8]) -> Option<usize> {
    if data.is_empty() {
        return None;
    }
    let mut dec = Decoder::new(data);
    match skip_cbor_value(&mut dec) {
        Ok(()) => Some(dec.position()),
        Err(_) => None,
    }
}

/// Skip one complete CBOR data item, recursively descending into containers.
///
/// This doesn't allocate or interpret values — it just advances the decoder
/// past a complete item, which is exactly what we need for boundary detection.
fn skip_cbor_value(dec: &mut Decoder<'_>) -> Result<(), minicbor::decode::Error> {
    use minicbor::data::Type;
    match dec.datatype()? {
        // Unsigned integers
        Type::U8 | Type::U16 | Type::U32 | Type::U64 => {
            dec.u64()?;
        }
        // Signed integers
        Type::I8 | Type::I16 | Type::I32 | Type::I64 => {
            dec.i64()?;
        }
        Type::Bool => {
            dec.bool()?;
        }
        Type::Null => {
            dec.null()?;
        }
        // Byte strings (definite and indefinite length)
        Type::Bytes | Type::BytesIndef => {
            dec.bytes()?;
        }
        // Text strings (definite and indefinite length)
        Type::String | Type::StringIndef => {
            dec.str()?;
        }
        // CBOR tags: skip the tag then recursively skip the tagged value
        Type::Tag => {
            dec.tag()?;
            skip_cbor_value(dec)?;
        }
        // Definite-length arrays
        Type::Array => {
            let len = dec.array()?.expect("definite array");
            for _ in 0..len {
                skip_cbor_value(dec)?;
            }
        }
        // Indefinite-length arrays: read until Break marker
        Type::ArrayIndef => {
            dec.array()?; // consume the indefinite-length header
            loop {
                if dec.datatype()? == Type::Break {
                    dec.skip()?;
                    break;
                }
                skip_cbor_value(dec)?;
            }
        }
        // Definite-length maps
        Type::Map => {
            let len = dec.map()?.expect("definite map");
            for _ in 0..len {
                skip_cbor_value(dec)?; // key
                skip_cbor_value(dec)?; // value
            }
        }
        // Indefinite-length maps: read until Break marker
        Type::MapIndef => {
            dec.map()?; // consume the indefinite-length header
            loop {
                if dec.datatype()? == Type::Break {
                    dec.skip()?;
                    break;
                }
                skip_cbor_value(dec)?; // key
                skip_cbor_value(dec)?; // value
            }
        }
        // Simple values (other than bool/null/undefined)
        Type::Simple => {
            dec.simple()?;
        }
        // Floating point numbers (minicbor 0.25: F16, F32, F64)
        Type::F16 | Type::F32 | Type::F64 => {
            dec.f64()?;
        }
        // Large integers that span multiple CBOR bytes
        Type::Int => {
            dec.i64()?;
        }
        Type::Undefined => {
            dec.undefined()?;
        }
        // Break is handled within indefinite containers above
        Type::Break => {
            dec.skip()?;
        }
        t => {
            return Err(minicbor::decode::Error::message(format!(
                "unsupported CBOR type: {t:?}"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_origin_roundtrip() {
        let mut buf = Vec::new();
        let mut enc = Encoder::new(&mut buf);
        encode_point(&mut enc, &Point::Origin);
        let mut dec = Decoder::new(&buf);
        assert_eq!(decode_point(&mut dec).unwrap(), Point::Origin);
    }

    #[test]
    fn point_specific_roundtrip() {
        let hash = [0xAB; 32];
        let point = Point::Specific(42000, hash);
        let mut buf = Vec::new();
        let mut enc = Encoder::new(&mut buf);
        encode_point(&mut enc, &point);
        let mut dec = Decoder::new(&buf);
        assert_eq!(decode_point(&mut dec).unwrap(), point);
    }

    #[test]
    fn tip_roundtrip() {
        let hash = [0xCD; 32];
        let mut buf = Vec::new();
        let mut enc = Encoder::new(&mut buf);
        encode_tip(&mut enc, 100, &hash, 50);
        let mut dec = Decoder::new(&buf);
        let (slot, h, bn) = decode_tip(&mut dec).unwrap();
        assert_eq!(slot, 100);
        assert_eq!(h, hash);
        assert_eq!(bn, 50);
    }

    #[test]
    fn cbor_boundary_complete_array() {
        // A complete CBOR array [0] = 0x81 0x00
        let complete = vec![0x81, 0x00];
        assert_eq!(try_decode_cbor_boundary(&complete), Some(2));
    }

    #[test]
    fn cbor_boundary_incomplete_array() {
        // Incomplete CBOR array (array of 2 with only 1 element)
        let incomplete = vec![0x82, 0x00];
        assert_eq!(try_decode_cbor_boundary(&incomplete), None);
    }

    #[test]
    fn cbor_boundary_empty() {
        assert_eq!(try_decode_cbor_boundary(&[]), None);
    }

    #[test]
    fn point_origin_wire_format() {
        // Origin should encode as a single byte: 0x80 (empty array)
        let mut buf = Vec::new();
        let mut enc = Encoder::new(&mut buf);
        encode_point(&mut enc, &Point::Origin);
        assert_eq!(buf, vec![0x80]);
    }

    #[test]
    fn decode_point_rejects_wrong_hash_length() {
        // Build a point with a 16-byte hash (invalid)
        let mut buf = Vec::new();
        let mut enc = Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u64(100).unwrap();
        enc.bytes(&[0xAA; 16]).unwrap();
        let mut dec = Decoder::new(&buf);
        assert!(decode_point(&mut dec).is_err());
    }
}
