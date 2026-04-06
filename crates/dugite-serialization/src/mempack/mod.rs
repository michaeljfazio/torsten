//! MemPack UTxO decoder for Haskell ledger state `tables/tvar` files.
//!
//! The `tvar` file in a Mithril ancillary archive contains the UTxO set serialised as:
//!
//! ```text
//! array(1) [
//!   map(indefinite) {        // 0xbf … 0xff
//!     bytes(34) → bytes(N),  // MemPack TxIn → MemPack TxOut
//!     …
//!   }
//! ]
//! ```
//!
//! ## TxIn encoding (34-byte key)
//!
//! ```text
//! TxId (32 bytes, big-endian) ‖ TxIx (2 bytes, LITTLE-endian)
//! ```
//!
//! ## TxOut encoding (value blob)
//!
//! The first byte is a tag (0–5) selecting a Haskell constructor variant.
//! See [`txout::decode_mempack_txout`] for the per-variant layout.

pub mod compact;
pub mod txout;

#[cfg(test)]
mod tests;

use crate::error::SerializationError;
use crate::haskell_snapshot::cbor_utils::{decode_array_len, decode_bytes};
use dugite_primitives::hash::Hash;

/// A decoded MemPack TxIn.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MemPackTxIn {
    /// Transaction hash (32 bytes, big-endian).
    pub txid: Hash<32>,
    /// Output index (little-endian u16 on the wire).
    pub txix: u16,
}

/// Decode a MemPack TxIn from exactly 34 raw bytes.
///
/// Layout: `TxId(32 BE) || TxIx(2 LE)`.
pub fn decode_mempack_txin(data: &[u8]) -> Result<MemPackTxIn, SerializationError> {
    if data.len() != 34 {
        return Err(SerializationError::InvalidLength {
            expected: 34,
            got: data.len(),
        });
    }
    let mut txid_bytes = [0u8; 32];
    txid_bytes.copy_from_slice(&data[0..32]);
    // TxIx is little-endian (host-native on x86/ARM).
    let txix = u16::from_le_bytes([data[32], data[33]]);
    Ok(MemPackTxIn {
        txid: Hash::from_bytes(txid_bytes),
        txix,
    })
}

/// Iterator over entries in a `tvar` file.
///
/// Yields `(MemPackTxIn, MemPackTxOut)` pairs.  The iterator handles both
/// normal termination (break byte `0xff`) and truncated input (returns `None`
/// when not enough bytes remain for a complete entry).
pub struct TvarIterator<'a> {
    data: &'a [u8],
    offset: usize,
    finished: bool,
}

impl<'a> TvarIterator<'a> {
    /// Create a new iterator positioned after the `array(1)` and `map(indef)` headers.
    pub fn new(data: &'a [u8]) -> Result<Self, SerializationError> {
        if data.is_empty() {
            return Err(SerializationError::CborDecode("empty tvar input".into()));
        }

        let mut off = 0;

        // Decode array(1) header.
        let (arr_len, n) = decode_array_len(data)?;
        if arr_len != 1 {
            return Err(SerializationError::CborDecode(format!(
                "tvar: expected array(1), got array({arr_len})"
            )));
        }
        off += n;

        // Expect indefinite-length map (0xbf) or definite-length map.
        if off >= data.len() {
            return Err(SerializationError::CborDecode(
                "tvar: truncated before map header".into(),
            ));
        }
        let major = data[off] >> 5;
        if major != 5 {
            return Err(SerializationError::CborDecode(format!(
                "tvar: expected map (major 5), got major {major}"
            )));
        }
        // For indefinite map, header is 1 byte (0xbf).
        // For definite map, header varies. Skip accordingly.
        let info = data[off] & 0x1f;
        if info == 31 {
            // Indefinite map.
            off += 1;
        } else {
            // Definite-length map: skip the header (we'll just iterate until done).
            let hdr_size = match info {
                0..=23 => 1,
                24 => 2,
                25 => 3,
                26 => 5,
                27 => 9,
                _ => {
                    return Err(SerializationError::CborDecode(
                        "tvar: invalid map length encoding".into(),
                    ))
                }
            };
            off += hdr_size;
        }

        Ok(TvarIterator {
            data,
            offset: off,
            finished: false,
        })
    }

    /// Return the current byte offset into the underlying data.
    pub fn offset(&self) -> usize {
        self.offset
    }
}

impl<'a> Iterator for TvarIterator<'a> {
    type Item = Result<(MemPackTxIn, txout::MemPackTxOut), SerializationError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }

        let remaining = &self.data[self.offset..];

        // Check for end-of-data (truncated file) or break byte (0xff).
        if remaining.is_empty() {
            self.finished = true;
            return None;
        }
        if remaining[0] == 0xff {
            self.finished = true;
            return None;
        }

        // Decode CBOR bytes(34) key.
        let key_result = decode_bytes(remaining);
        let (key_bytes, key_consumed) = match key_result {
            Ok(v) => v,
            Err(_) => {
                // Not enough data for a complete CBOR bytes header — treat as truncation.
                self.finished = true;
                return None;
            }
        };

        // Decode TxIn from the 34-byte key.
        let txin = match decode_mempack_txin(key_bytes) {
            Ok(t) => t,
            Err(e) => {
                self.finished = true;
                return Some(Err(e));
            }
        };

        // Decode CBOR bytes(N) value.
        let val_start = self.offset + key_consumed;
        if val_start >= self.data.len() {
            self.finished = true;
            return None;
        }
        let val_result = decode_bytes(&self.data[val_start..]);
        let (val_bytes, val_consumed) = match val_result {
            Ok(v) => v,
            Err(_) => {
                // Truncated value — graceful stop.
                self.finished = true;
                return None;
            }
        };

        // Advance past key + value.
        self.offset = val_start + val_consumed;

        // Decode the MemPack TxOut from the value bytes.
        match txout::decode_mempack_txout(val_bytes) {
            Ok((txout, _consumed)) => Some(Ok((txin, txout))),
            Err(e) => Some(Err(e)),
        }
    }
}
