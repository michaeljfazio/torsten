//! Low-level CBOR decoding utilities for Haskell ledger state snapshots.
//!
//! These functions consume CBOR bytes and return `(decoded_value, bytes_consumed)`.
//! They follow the existing pattern in `crate::cbor` but add array/map/rational
//! decoders needed for the Haskell ExtLedgerState format.

use crate::error::SerializationError;
use dugite_primitives::hash::{Hash28, Hash32};

/// Decode a CBOR unsigned integer (major type 0).
pub fn decode_uint(data: &[u8]) -> Result<(u64, usize), SerializationError> {
    if data.is_empty() {
        return Err(SerializationError::CborDecode("empty input".into()));
    }
    let major = data[0] >> 5;
    let info = data[0] & 0x1f;
    if major != 0 {
        return Err(SerializationError::CborDecode(format!(
            "expected uint (major 0), got major {major} at byte {:#04x}",
            data[0]
        )));
    }
    decode_uint_info(data, info)
}

/// Decode a CBOR integer that could be unsigned (major 0) or negative (major 1).
pub fn decode_int(data: &[u8]) -> Result<(i64, usize), SerializationError> {
    if data.is_empty() {
        return Err(SerializationError::CborDecode("empty input".into()));
    }
    let major = data[0] >> 5;
    let info = data[0] & 0x1f;
    match major {
        0 => {
            let (v, n) = decode_uint_info(data, info)?;
            Ok((v as i64, n))
        }
        1 => {
            let (v, n) = decode_uint_info(data, info)?;
            Ok((-1 - v as i64, n))
        }
        _ => Err(SerializationError::CborDecode(format!(
            "expected int, got major {major}"
        ))),
    }
}

/// Internal: decode the integer value given the already-checked major type byte and
/// `info` bits. `data` must still point at the initial header byte.
fn decode_uint_info(data: &[u8], info: u8) -> Result<(u64, usize), SerializationError> {
    match info {
        0..=23 => Ok((info as u64, 1)),
        24 => {
            if data.len() < 2 {
                return Err(eof());
            }
            Ok((data[1] as u64, 2))
        }
        25 => {
            if data.len() < 3 {
                return Err(eof());
            }
            Ok((u16::from_be_bytes([data[1], data[2]]) as u64, 3))
        }
        26 => {
            if data.len() < 5 {
                return Err(eof());
            }
            Ok((
                u32::from_be_bytes([data[1], data[2], data[3], data[4]]) as u64,
                5,
            ))
        }
        27 => {
            if data.len() < 9 {
                return Err(eof());
            }
            Ok((
                u64::from_be_bytes([
                    data[1], data[2], data[3], data[4], data[5], data[6], data[7], data[8],
                ]),
                9,
            ))
        }
        _ => Err(SerializationError::CborDecode(format!(
            "invalid additional info {info}"
        ))),
    }
}

/// Decode a CBOR bigint (tag 2 = positive bignum wrapping a bytestring).
/// Falls back to regular uint if no tag is present.
pub fn decode_bigint_or_uint(data: &[u8]) -> Result<(u64, usize), SerializationError> {
    if data.is_empty() {
        return Err(eof());
    }
    let major = data[0] >> 5;
    if major == 0 {
        return decode_uint(data);
    }
    // Tag 2 (positive bignum): 0xc2 + bytestring
    if data[0] == 0xc2 {
        let (bytes, n) = decode_bytes(&data[1..])?;
        let mut val = 0u64;
        for &b in bytes {
            val = val.checked_shl(8).unwrap_or(u64::MAX) | b as u64;
        }
        return Ok((val, 1 + n));
    }
    Err(SerializationError::CborDecode(format!(
        "expected uint or bigint, got {:#04x}",
        data[0]
    )))
}

/// Decode a CBOR array header, returning `(length, bytes_consumed)`.
pub fn decode_array_len(data: &[u8]) -> Result<(usize, usize), SerializationError> {
    if data.is_empty() {
        return Err(eof());
    }
    let major = data[0] >> 5;
    let info = data[0] & 0x1f;
    if major != 4 {
        return Err(SerializationError::CborDecode(format!(
            "expected array (major 4), got major {major} at byte {:#04x}",
            data[0]
        )));
    }
    let (len, consumed) = decode_uint_info(data, info)?;
    Ok((len as usize, consumed))
}

/// Decode a CBOR map header, returning `(Some(length), bytes_consumed)` for a
/// definite-length map or `(None, 1)` for an indefinite-length map.
pub fn decode_map_len(data: &[u8]) -> Result<(Option<usize>, usize), SerializationError> {
    if data.is_empty() {
        return Err(eof());
    }
    let major = data[0] >> 5;
    let info = data[0] & 0x1f;
    if major != 5 {
        return Err(SerializationError::CborDecode(format!(
            "expected map (major 5), got major {major} at byte {:#04x}",
            data[0]
        )));
    }
    // Indefinite-length map
    if info == 31 {
        return Ok((None, 1));
    }
    let (len, consumed) = decode_uint_info(data, info)?;
    Ok((Some(len as usize), consumed))
}

/// Decode a CBOR byte string, returning `(&[u8], bytes_consumed)`.
pub fn decode_bytes(data: &[u8]) -> Result<(&[u8], usize), SerializationError> {
    if data.is_empty() {
        return Err(eof());
    }
    let major = data[0] >> 5;
    let info = data[0] & 0x1f;
    if major != 2 {
        return Err(SerializationError::CborDecode(format!(
            "expected bytes (major 2), got major {major} at byte {:#04x}",
            data[0]
        )));
    }
    let (len, hdr) = decode_uint_info(data, info)?;
    let len = len as usize;
    if data.len() < hdr + len {
        return Err(eof());
    }
    Ok((&data[hdr..hdr + len], hdr + len))
}

/// Decode a CBOR text string, returning `(&str, bytes_consumed)`.
pub fn decode_text(data: &[u8]) -> Result<(&str, usize), SerializationError> {
    if data.is_empty() {
        return Err(eof());
    }
    let major = data[0] >> 5;
    let info = data[0] & 0x1f;
    if major != 3 {
        return Err(SerializationError::CborDecode(format!(
            "expected text (major 3), got major {major}",
        )));
    }
    let (len, hdr) = decode_uint_info(data, info)?;
    let len = len as usize;
    if data.len() < hdr + len {
        return Err(eof());
    }
    let s = std::str::from_utf8(&data[hdr..hdr + len])
        .map_err(|e| SerializationError::CborDecode(format!("invalid utf8: {e}")))?;
    Ok((s, hdr + len))
}

/// Decode a 28-byte CBOR bytestring into a `Hash28`.
pub fn decode_hash28(data: &[u8]) -> Result<(Hash28, usize), SerializationError> {
    let (bytes, n) = decode_bytes(data)?;
    if bytes.len() != 28 {
        return Err(SerializationError::InvalidLength {
            expected: 28,
            got: bytes.len(),
        });
    }
    Ok((Hash28::from_bytes(bytes.try_into().unwrap()), n))
}

/// Decode a 32-byte CBOR bytestring into a `Hash32`.
pub fn decode_hash32(data: &[u8]) -> Result<(Hash32, usize), SerializationError> {
    let (bytes, n) = decode_bytes(data)?;
    if bytes.len() != 32 {
        return Err(SerializationError::InvalidLength {
            expected: 32,
            got: bytes.len(),
        });
    }
    Ok((Hash32::from_bytes(bytes.try_into().unwrap()), n))
}

/// Decode a Haskell `Nonce`:
/// - `[0]` → `NeutralNonce` mapped to the zero `Hash32`
/// - `[1, bytes(32)]` → `Nonce` carrying the 32-byte hash
pub fn decode_nonce(data: &[u8]) -> Result<(Hash32, usize), SerializationError> {
    let (arr_len, mut off) = decode_array_len(data)?;
    let (tag, n) = decode_uint(&data[off..])?;
    off += n;
    match (arr_len, tag) {
        (1, 0) => Ok((Hash32::ZERO, off)),
        (2, 1) => {
            let (hash, n) = decode_hash32(&data[off..])?;
            off += n;
            Ok((hash, off))
        }
        _ => Err(SerializationError::CborDecode(format!(
            "invalid nonce: array({arr_len}), tag {tag}"
        ))),
    }
}

/// Decode a Haskell `Credential`:
/// - `[0, bytes(28)]` → KeyHash
/// - `[1, bytes(28)]` → ScriptHash
///
/// Returns `((tag, hash28), bytes_consumed)`.
pub fn decode_credential(data: &[u8]) -> Result<((u8, Hash28), usize), SerializationError> {
    let (arr_len, mut off) = decode_array_len(data)?;
    if arr_len != 2 {
        return Err(SerializationError::InvalidLength {
            expected: 2,
            got: arr_len,
        });
    }
    let (tag, n) = decode_uint(&data[off..])?;
    off += n;
    let (hash, n) = decode_hash28(&data[off..])?;
    off += n;
    Ok(((tag as u8, hash), off))
}

/// Decode a Haskell `WithOrigin<T>` array header:
/// - `[]` (array of 0) → `None` (Origin)
/// - `[v]` (array of 1) → `Some(1)` — caller must then decode the inner value
///
/// Returns `(inner_element_count, bytes_of_array_header_consumed)`.
pub fn decode_with_origin_len(data: &[u8]) -> Result<(Option<usize>, usize), SerializationError> {
    let (arr_len, off) = decode_array_len(data)?;
    match arr_len {
        0 => Ok((None, off)),
        1 => Ok((Some(1), off)),
        n => Err(SerializationError::CborDecode(format!(
            "WithOrigin: expected array(0) or array(1), got array({n})"
        ))),
    }
}

/// Decode a Haskell `Rational` encoded as either:
/// - CBOR tag 30 followed by `[numerator, denominator]`, or
/// - plain `[numerator, denominator]` (no tag).
///
/// Each integer is decoded with `decode_bigint_or_uint` to handle bignum encoding.
pub fn decode_rational(data: &[u8]) -> Result<((u64, u64), usize), SerializationError> {
    let mut off = 0;
    // Skip tag 30 (0xd8 0x1e) if present
    if data.len() >= 2 && data[0] == 0xd8 && data[1] == 0x1e {
        off += 2;
    }
    let (arr_len, n) = decode_array_len(&data[off..])?;
    off += n;
    if arr_len != 2 {
        return Err(SerializationError::InvalidLength {
            expected: 2,
            got: arr_len,
        });
    }
    let (num, n) = decode_bigint_or_uint(&data[off..])?;
    off += n;
    let (den, n) = decode_bigint_or_uint(&data[off..])?;
    off += n;
    Ok(((num, den), off))
}

/// Check whether the next byte is CBOR null (`0xf6`).
///
/// Returns `(true, 1)` if the byte is null (consuming it), or `(false, 0)` if not
/// (leaving the cursor unchanged so the caller can decode the actual value).
pub fn decode_null(data: &[u8]) -> Result<(bool, usize), SerializationError> {
    if data.is_empty() {
        return Err(eof());
    }
    if data[0] == 0xf6 {
        Ok((true, 1))
    } else {
        Ok((false, 0))
    }
}

/// Skip over any single CBOR value, returning the number of bytes consumed.
///
/// Used for fields we don't need to fully decode (e.g., NonMyopic, pulsingRewUpdate).
pub fn skip_cbor_value(data: &[u8]) -> Result<usize, SerializationError> {
    if data.is_empty() {
        return Err(eof());
    }
    let major = data[0] >> 5;
    let info = data[0] & 0x1f;
    match major {
        // Unsigned or negative integer
        0 | 1 => {
            let (_, n) = decode_uint_info(data, info)?;
            Ok(n)
        }
        // Byte string or text string
        2 | 3 => {
            let hdr_len = match info {
                0..=23 => 1usize,
                24 => 2,
                25 => 3,
                26 => 5,
                27 => 9,
                _ => {
                    return Err(SerializationError::CborDecode(
                        "invalid string length encoding".into(),
                    ))
                }
            };
            let payload_len = match info {
                0..=23 => info as usize,
                24 => {
                    if data.len() < 2 {
                        return Err(eof());
                    }
                    data[1] as usize
                }
                25 => {
                    if data.len() < 3 {
                        return Err(eof());
                    }
                    u16::from_be_bytes([data[1], data[2]]) as usize
                }
                26 => {
                    if data.len() < 5 {
                        return Err(eof());
                    }
                    u32::from_be_bytes([data[1], data[2], data[3], data[4]]) as usize
                }
                27 => {
                    if data.len() < 9 {
                        return Err(eof());
                    }
                    u64::from_be_bytes([
                        data[1], data[2], data[3], data[4], data[5], data[6], data[7], data[8],
                    ]) as usize
                }
                _ => {
                    return Err(SerializationError::CborDecode(
                        "invalid string length encoding".into(),
                    ))
                }
            };
            Ok(hdr_len + payload_len)
        }
        // Array
        4 => {
            if info == 31 {
                // Indefinite-length array
                let mut off = 1;
                while off < data.len() && data[off] != 0xff {
                    off += skip_cbor_value(&data[off..])?;
                }
                Ok(off + 1) // +1 for the break byte 0xff
            } else {
                let (count, mut off) = decode_uint_info(data, info)?;
                for _ in 0..count {
                    off += skip_cbor_value(&data[off..])?;
                }
                Ok(off)
            }
        }
        // Map
        5 => {
            if info == 31 {
                let mut off = 1;
                while off < data.len() && data[off] != 0xff {
                    off += skip_cbor_value(&data[off..])?; // key
                    off += skip_cbor_value(&data[off..])?; // value
                }
                Ok(off + 1) // +1 for the break byte 0xff
            } else {
                let (count, mut off) = decode_uint_info(data, info)?;
                for _ in 0..count {
                    off += skip_cbor_value(&data[off..])?; // key
                    off += skip_cbor_value(&data[off..])?; // value
                }
                Ok(off)
            }
        }
        // Tag: skip the tag header then skip the tagged value
        6 => {
            let (_, n) = decode_uint_info(data, info)?;
            let inner = skip_cbor_value(&data[n..])?;
            Ok(n + inner)
        }
        // Simple values and floats
        7 => match info {
            0..=23 => Ok(1), // simple value (null=22, true=21, false=20, etc.)
            24 => Ok(2),
            25 => Ok(3),  // float16
            26 => Ok(5),  // float32
            27 => Ok(9),  // float64
            31 => Ok(1),  // break code (should not appear at top level, but handle gracefully)
            _ => Err(SerializationError::CborDecode(
                "invalid simple/float encoding".into(),
            )),
        },
        _ => unreachable!("CBOR major type is 3 bits, range 0-7"),
    }
}

/// Construct an "unexpected end of input" error.
fn eof() -> SerializationError {
    SerializationError::CborDecode("unexpected end of input".into())
}
