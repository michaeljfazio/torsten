//! Low-level decoders for MemPack compact representations.
//!
//! ## VarLen encoding
//!
//! MemPack uses a 7-bit variable-length unsigned integer encoding (LSB first,
//! continuation bit in the MSB — identical to Protocol Buffers Base-128 varint):
//!
//! * If `byte & 0x80 == 0`: final byte, value = `byte & 0x7f`.
//! * If `byte & 0x80 != 0`: value |= `(byte & 0x7f) << (7 * position)`, continue
//!   with the next byte.
//!
//! ## CompactAddr
//!
//! ```text
//! VarLen(address_byte_length) + raw_address_bytes
//! ```
//!
//! ## CompactValue
//!
//! ```text
//! tag(0) + VarLen(lovelace)                 — ADA-only
//! tag(1) + VarLen(lovelace) + multi-asset   — multi-asset (opaque bytes for now)
//! ```

use crate::error::SerializationError;

/// Maximum number of bytes we allow for a single VarLen integer (10 bytes covers
/// the full u64 range: ceil(64/7) = 10).
const MAX_VARLEN_BYTES: usize = 10;

/// Decode a VarLen-encoded unsigned integer.
///
/// Returns `(value, bytes_consumed)`.
pub fn decode_varlen(data: &[u8]) -> Result<(u64, usize), SerializationError> {
    if data.is_empty() {
        return Err(SerializationError::CborDecode("varlen: empty input".into()));
    }

    let mut value: u64 = 0;
    for (i, &byte) in data.iter().take(MAX_VARLEN_BYTES).enumerate() {
        // Accumulate the 7 payload bits at the correct shift.
        value |= ((byte & 0x7f) as u64) << (7 * i);
        // If the continuation bit is clear, this was the last byte.
        if byte & 0x80 == 0 {
            return Ok((value, i + 1));
        }
    }

    Err(SerializationError::CborDecode(
        "varlen: exceeded maximum length without termination".into(),
    ))
}

/// Decode a CompactAddr: `VarLen(length) + raw_address_bytes`.
///
/// Returns `(address_bytes, total_bytes_consumed)`.
pub fn decode_compact_addr(data: &[u8]) -> Result<(Vec<u8>, usize), SerializationError> {
    let (addr_len, len_bytes) = decode_varlen(data)?;
    let addr_len = addr_len as usize;
    let total = len_bytes + addr_len;
    if data.len() < total {
        return Err(SerializationError::CborDecode(format!(
            "compact_addr: need {total} bytes, have {}",
            data.len()
        )));
    }
    let addr = data[len_bytes..total].to_vec();
    Ok((addr, total))
}

/// Result of decoding a CompactValue.
#[derive(Debug, Clone)]
pub struct CompactValueDecoded {
    /// Lovelace amount.
    pub coin: u64,
    /// For multi-asset values (tag 1), the raw multi-asset bytes that follow the
    /// coin VarLen.  For ADA-only values (tag 0) this is `None`.
    pub multi_asset_raw: Option<Vec<u8>>,
    /// Total bytes consumed from the input slice.
    pub consumed: usize,
}

/// Decode a CompactValue.
///
/// `remaining_len` is the number of bytes available for the *entire* CompactValue
/// plus any trailing data in the same TxOut blob.  When the value is ADA-only
/// (tag 0), the coin VarLen is the only field and we consume exactly those bytes.
/// When the value is multi-asset (tag 1), we consume `VarLen(coin)` and then
/// **all remaining bytes up to `total_remaining`** are stored as opaque
/// multi-asset data (the caller is responsible for further subdivision if needed).
///
/// If `total_remaining` is `None`, we consume only the coin VarLen (useful when
/// the caller knows the exact extent of the CompactValue independently).
pub fn decode_compact_value(
    data: &[u8],
    total_remaining: Option<usize>,
) -> Result<CompactValueDecoded, SerializationError> {
    if data.is_empty() {
        return Err(SerializationError::CborDecode(
            "compact_value: empty input".into(),
        ));
    }

    let tag = data[0];
    let mut off = 1usize;

    // Decode VarLen(coin).
    let (coin, n) = decode_varlen(&data[off..])?;
    off += n;

    match tag {
        0 => {
            // ADA-only.
            Ok(CompactValueDecoded {
                coin,
                multi_asset_raw: None,
                consumed: off,
            })
        }
        1 => {
            // Multi-asset. The bytes after the coin VarLen up to the end of the
            // allocated slice represent the multi-asset payload.
            let end = total_remaining.unwrap_or(off);
            let ma = if end > off {
                Some(data[off..end].to_vec())
            } else {
                None
            };
            Ok(CompactValueDecoded {
                coin,
                multi_asset_raw: ma,
                consumed: end,
            })
        }
        other => Err(SerializationError::CborDecode(format!(
            "compact_value: unknown tag {other}"
        ))),
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn test_decode_varlen_small() {
        assert_eq!(decode_varlen(&[0]).unwrap(), (0, 1));
        assert_eq!(decode_varlen(&[1]).unwrap(), (1, 1));
        assert_eq!(decode_varlen(&[29]).unwrap(), (29, 1));
        assert_eq!(decode_varlen(&[127]).unwrap(), (127, 1));
    }

    #[test]
    fn test_decode_varlen_multi_byte() {
        // 128 = 0x80 0x01
        assert_eq!(decode_varlen(&[0x80, 0x01]).unwrap(), (128, 2));
        // 150 = 0x96 0x01  (0x16 | (0x01 << 7) = 22 + 128 = 150)
        assert_eq!(decode_varlen(&[0x96, 0x01]).unwrap(), (150, 2));
        // 300 = 0xAC 0x02  (0x2C | (0x02 << 7) = 44 + 256 = 300)
        assert_eq!(decode_varlen(&[0xAC, 0x02]).unwrap(), (300, 2));
    }

    #[test]
    fn test_decode_varlen_three_bytes() {
        // 16384 = 0x80 0x80 0x01
        assert_eq!(decode_varlen(&[0x80, 0x80, 0x01]).unwrap(), (16384, 3));
    }

    #[test]
    fn test_decode_varlen_empty() {
        assert!(decode_varlen(&[]).is_err());
    }

    #[test]
    fn test_decode_compact_addr() {
        // addr_len = 29, then 29 bytes of address data.
        let mut data = vec![29u8]; // VarLen(29)
        data.extend_from_slice(&[0x60; 29]); // 29 dummy address bytes
        let (addr, consumed) = decode_compact_addr(&data).unwrap();
        assert_eq!(addr.len(), 29);
        assert_eq!(consumed, 30);
    }

    #[test]
    fn test_decode_compact_value_ada_only() {
        // tag=0, coin VarLen = 28398 (from real data: 0xee 0xdd 0x01)
        let data = [0x00, 0xee, 0xdd, 0x01];
        let result = decode_compact_value(&data, None).unwrap();
        assert_eq!(result.coin, 28398);
        assert!(result.multi_asset_raw.is_none());
        assert_eq!(result.consumed, 4);
    }

    #[test]
    fn test_decode_compact_value_multi_asset() {
        // tag=1, coin VarLen = 1579224 (d8 b1 60), then 5 bytes of multi-asset.
        let data = [0x01, 0xd8, 0xb1, 0x60, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE];
        let result = decode_compact_value(&data, Some(data.len())).unwrap();
        assert_eq!(result.coin, 1579224);
        let ma = result.multi_asset_raw.unwrap();
        assert_eq!(ma, &[0xAA, 0xBB, 0xCC, 0xDD, 0xEE]);
    }
}
