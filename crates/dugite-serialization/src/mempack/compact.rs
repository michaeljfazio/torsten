//! Low-level decoders for MemPack compact representations.
//!
//! ## VarLen encoding
//!
//! MemPack uses a 7-bit variable-length unsigned integer encoding that is
//! **MSB-first** (big-endian base-128), matching the Haskell `mempack` library
//! in `Data.MemPack` (see `packIntoCont7` / `unpack7BitVarLen` in
//! <https://github.com/lehins/mempack/blob/master/src/Data/MemPack.hs>).
//!
//! On the wire, the value is split into 7-bit groups starting with the most
//! significant group first. Every byte except the last has its top bit set
//! (continuation marker); the final byte has its top bit clear. Decoding shifts
//! the accumulator left by 7 and ORs in the next 7 payload bits each step:
//!
//! ```text
//! acc = 0
//! loop:
//!   b = next byte
//!   acc = (acc << 7) | (b & 0x7f)
//!   if b & 0x80 == 0 break
//! ```
//!
//! This is **NOT** protobuf-style LSB-first varint. The distinction matters:
//! for example `[0xee, 0xdd, 0x01]` decodes to `1_814_145` (MSB-first), not
//! `28_398` (LSB-first).
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

/// Maximum number of bytes we allow for a single VarLen integer. 10 bytes
/// covers the full u64 range (`ceil(64/7) = 10`).
const MAX_VARLEN_BYTES: usize = 10;

/// Decode a MemPack VarLen-encoded unsigned integer (MSB-first base-128).
///
/// Returns `(value, bytes_consumed)`. Errors if the encoding is truncated or
/// exceeds 10 bytes without a terminating byte.
pub fn decode_varlen(data: &[u8]) -> Result<(u64, usize), SerializationError> {
    if data.is_empty() {
        return Err(SerializationError::CborDecode("varlen: empty input".into()));
    }

    let mut acc: u64 = 0;
    for (i, &byte) in data.iter().take(MAX_VARLEN_BYTES).enumerate() {
        // Shift existing bits up by 7 and OR in the lower 7 bits of this byte.
        acc = (acc << 7) | ((byte & 0x7f) as u64);
        if byte & 0x80 == 0 {
            return Ok((acc, i + 1));
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
        // Single-byte encodings (no continuation bit): value = byte.
        assert_eq!(decode_varlen(&[0]).unwrap(), (0, 1));
        assert_eq!(decode_varlen(&[1]).unwrap(), (1, 1));
        assert_eq!(decode_varlen(&[29]).unwrap(), (29, 1));
        assert_eq!(decode_varlen(&[127]).unwrap(), (127, 1));
    }

    #[test]
    fn test_decode_varlen_multi_byte_msb_first() {
        // MSB-first: first byte is the most significant 7 bits.
        //
        // 128 = (1 << 7) | 0  →  [0x81, 0x00]
        assert_eq!(decode_varlen(&[0x81, 0x00]).unwrap(), (128, 2));
        // 150 = (1 << 7) | 22 →  [0x81, 0x16]
        assert_eq!(decode_varlen(&[0x81, 0x16]).unwrap(), (150, 2));
        // 300 = (2 << 7) | 44 →  [0x82, 0x2c]
        assert_eq!(decode_varlen(&[0x82, 0x2c]).unwrap(), (300, 2));
    }

    #[test]
    fn test_decode_varlen_three_bytes_fixture() {
        // 1_814_145 = 0xee 0xdd 0x01 (real coin value from preview tvar,
        // cross-checked against Koios: tx
        // 00002435e40d68a58b5130644c845c05fa8e36e3935a905f718e6fa611f0304a#2
        // → value 1_814_145).
        //
        //   0xee → acc = 0 << 7 | 0x6e = 110
        //   0xdd → acc = 110 << 7 | 0x5d = 14_173
        //   0x01 → acc = 14_173 << 7 | 0x01 = 1_814_145
        assert_eq!(decode_varlen(&[0xee, 0xdd, 0x01]).unwrap(), (1_814_145, 3));
    }

    #[test]
    fn test_decode_varlen_max_u64() {
        // u64::MAX = 2^64 - 1. In 7-bit MSB-first, that is 10 bytes:
        //   [0x81, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f]
        // The first byte encodes the single leading bit (2^63), subsequent
        // bytes contribute 7 bits each.
        let bytes = [0x81, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f];
        assert_eq!(decode_varlen(&bytes).unwrap(), (u64::MAX, 10));
    }

    #[test]
    fn test_decode_varlen_empty() {
        assert!(decode_varlen(&[]).is_err());
    }

    #[test]
    fn test_decode_varlen_truncated() {
        // Continuation bit set but no more bytes.
        assert!(decode_varlen(&[0x80]).is_err());
    }

    #[test]
    fn test_decode_compact_addr() {
        // addr_len = 29, then 29 bytes of address data.
        let mut data = vec![29u8]; // VarLen(29), single byte
        data.extend_from_slice(&[0x60; 29]); // 29 dummy address bytes
        let (addr, consumed) = decode_compact_addr(&data).unwrap();
        assert_eq!(addr.len(), 29);
        assert_eq!(consumed, 30);
    }

    #[test]
    fn test_decode_compact_value_ada_only() {
        // tag=0, coin VarLen = 1_814_145 (MSB-first [0xee, 0xdd, 0x01]).
        let data = [0x00, 0xee, 0xdd, 0x01];
        let result = decode_compact_value(&data, None).unwrap();
        assert_eq!(result.coin, 1_814_145);
        assert!(result.multi_asset_raw.is_none());
        assert_eq!(result.consumed, 4);
    }

    #[test]
    fn test_decode_compact_value_multi_asset() {
        // tag=1, coin VarLen (3 bytes, MSB-first), then 5 bytes of multi-asset.
        //
        //   0xd8 0xb1 0x60 → ((0x58 << 14) | (0x31 << 7) | 0x60)
        //                  =  1_450_144 + 6_272 + 96
        //                  wait — MSB-first:
        //     0xd8 (cont) → acc = 0x58 = 88
        //     0xb1 (cont) → acc = 88<<7 | 0x31 = 11_264 + 49 = 11_313
        //     0x60 (stop) → acc = 11_313<<7 | 0x60 = 1_448_064 + 96 = 1_448_160
        let data = [0x01, 0xd8, 0xb1, 0x60, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE];
        let result = decode_compact_value(&data, Some(data.len())).unwrap();
        assert_eq!(result.coin, 1_448_160);
        let ma = result.multi_asset_raw.unwrap();
        assert_eq!(ma, &[0xAA, 0xBB, 0xCC, 0xDD, 0xEE]);
    }
}
