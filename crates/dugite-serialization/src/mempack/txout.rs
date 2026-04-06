//! MemPack TxOut decoder.
//!
//! The first byte of a MemPack TxOut blob selects the Haskell constructor variant:
//!
//! | Tag | Variant                                | Fields                                |
//! |-----|----------------------------------------|---------------------------------------|
//! |  0  | `TxOutCompact`                         | CompactAddr + CompactValue            |
//! |  1  | `TxOutCompactDH`                       | CompactAddr + CompactValue + DataHash |
//! |  2  | `TxOut_AddrHash28_AdaOnly`             | Credential + Addr28Extra + Coin       |
//! |  3  | `TxOut_AddrHash28_AdaOnly_DataHash32`  | Credential + Addr28Extra + Coin + DH  |
//! |  4  | `TxOutCompactDatum`                    | CompactAddr + CompactValue + Datum    |
//! |  5  | `TxOutCompactRefScript`                | CompactAddr + CompactValue + Datum + Script |
//!
//! Tags 2 and 3 use a packed compact representation whose internal structure
//! (Addr28Extra + CompactForm Coin) is non-trivial.  This decoder stores the raw
//! credential info and opaque remaining bytes for those variants, letting the
//! consumer (typically `dugite-ledger`) reconstruct the full address and value.

use crate::error::SerializationError;
use crate::mempack::compact::{decode_compact_addr, decode_compact_value};

/// A decoded MemPack TxOut.
///
/// Fields are populated according to the tag variant.  For tags 2/3, address
/// bytes are the raw packed representation (credential type + 28-byte hash +
/// opaque Addr28Extra), and `coin` may be zero when it cannot be extracted from
/// the packed form.
#[derive(Debug, Clone)]
pub struct MemPackTxOut {
    /// MemPack constructor tag (0–5).
    pub tag: u8,
    /// Raw Cardano address bytes.
    ///
    /// For tags 0/1/4/5 this is the full decoded CompactAddr (header + credential
    /// hashes).  For tags 2/3 this contains `credential_type(1) + hash28(28)` only.
    pub address: Vec<u8>,
    /// Lovelace amount.  Zero when the coin cannot be extracted (tags 2/3).
    pub coin: u64,
    /// Raw multi-asset bytes (when CompactValue tag = 1).
    pub multi_asset: Option<Vec<u8>>,
    /// 32-byte datum hash (tags 1, 3).
    pub datum_hash: Option<[u8; 32]>,
    /// Inline datum bytes (tags 4, 5).
    pub datum: Option<Vec<u8>>,
    /// Reference script bytes (tag 5).
    pub script_ref: Option<Vec<u8>>,
    /// Opaque remaining bytes that could not be fully parsed.
    ///
    /// For tags 2/3 this holds the entire Addr28Extra + coin encoding after the
    /// first 29 bytes (credential type + hash28).  The consumer can decode this
    /// once the exact Haskell MemPack layout for `Addr28Extra` is known.
    pub opaque_tail: Option<Vec<u8>>,
}

/// Decode a MemPack TxOut from raw bytes.
///
/// The input is the value payload of a `tvar` map entry (already unwrapped from
/// its CBOR bytestring envelope).
///
/// Returns `(txout, bytes_consumed)`.  For a well-formed entry `bytes_consumed`
/// equals `data.len()`.
pub fn decode_mempack_txout(data: &[u8]) -> Result<(MemPackTxOut, usize), SerializationError> {
    if data.is_empty() {
        return Err(SerializationError::CborDecode(
            "mempack_txout: empty input".into(),
        ));
    }

    let tag = data[0];
    match tag {
        0 => decode_tag0(data),
        1 => decode_tag1(data),
        2 => decode_tag2(data),
        3 => decode_tag3(data),
        4 => decode_tag4(data),
        5 => decode_tag5(data),
        _ => Err(SerializationError::CborDecode(format!(
            "mempack_txout: unknown tag {tag}"
        ))),
    }
}

/// Tag 0: `TxOutCompact` — CompactAddr + CompactValue.
fn decode_tag0(data: &[u8]) -> Result<(MemPackTxOut, usize), SerializationError> {
    let mut off = 1; // skip tag byte

    // CompactAddr: VarLen(len) + raw_addr_bytes.
    let (address, addr_consumed) = decode_compact_addr(&data[off..])?;
    off += addr_consumed;

    // CompactValue: tag(0/1) + VarLen(coin) [+ multi-asset].
    let val = decode_compact_value(&data[off..], Some(data.len() - off))?;
    off += val.consumed;

    Ok((
        MemPackTxOut {
            tag: 0,
            address,
            coin: val.coin,
            multi_asset: val.multi_asset_raw,
            datum_hash: None,
            datum: None,
            script_ref: None,
            opaque_tail: None,
        },
        off,
    ))
}

/// Tag 1: `TxOutCompactDH` — CompactAddr + CompactValue + DataHash(32 bytes).
///
/// The datum hash is the last 32 bytes of the blob.
fn decode_tag1(data: &[u8]) -> Result<(MemPackTxOut, usize), SerializationError> {
    if data.len() < 34 {
        // tag(1) + at minimum some addr + value + 32-byte hash
        return Err(SerializationError::CborDecode(
            "mempack_txout tag 1: too short".into(),
        ));
    }

    let mut off = 1;

    let (address, addr_consumed) = decode_compact_addr(&data[off..])?;
    off += addr_consumed;

    // The datum hash occupies the last 32 bytes.  Everything between addr and
    // the hash is the CompactValue.
    let value_end = data.len() - 32;
    if off > value_end {
        return Err(SerializationError::CborDecode(
            "mempack_txout tag 1: not enough bytes for value + datum hash".into(),
        ));
    }

    let val = decode_compact_value(&data[off..], Some(value_end - off))?;

    let mut datum_hash_bytes = [0u8; 32];
    datum_hash_bytes.copy_from_slice(&data[value_end..value_end + 32]);

    Ok((
        MemPackTxOut {
            tag: 1,
            address,
            coin: val.coin,
            multi_asset: val.multi_asset_raw,
            datum_hash: Some(datum_hash_bytes),
            datum: None,
            script_ref: None,
            opaque_tail: None,
        },
        data.len(),
    ))
}

/// Tag 2: `TxOut_AddrHash28_AdaOnly` — packed compact form.
///
/// Layout: `tag(1) + credential_type(1) + payment_hash(28) + opaque_tail`.
///
/// The opaque tail contains the Addr28Extra (staking hash + metadata) and the
/// CompactForm Coin in a Haskell-specific packed encoding that varies in size.
/// We extract the credential info and store the rest for downstream decoding.
fn decode_tag2(data: &[u8]) -> Result<(MemPackTxOut, usize), SerializationError> {
    if data.len() < 30 {
        return Err(SerializationError::CborDecode(
            "mempack_txout tag 2: need at least 30 bytes".into(),
        ));
    }

    // Byte 1: credential type (0 = KeyHash, 1 = ScriptHash).
    // Bytes 2–29: 28-byte payment credential hash.
    let _cred_type = data[1];
    let address = data[1..30].to_vec(); // credential_type(1) + hash28(28)

    // Everything after the credential is opaque (Addr28Extra + CompactForm Coin).
    let opaque = if data.len() > 30 {
        Some(data[30..].to_vec())
    } else {
        None
    };

    Ok((
        MemPackTxOut {
            tag: 2,
            address,
            coin: 0, // Cannot reliably extract without full Addr28Extra decoding.
            multi_asset: None,
            datum_hash: None,
            datum: None,
            script_ref: None,
            opaque_tail: opaque,
        },
        data.len(),
    ))
}

/// Tag 3: `TxOut_AddrHash28_AdaOnly_DataHash32` — packed compact form with datum hash.
///
/// Same as tag 2 but the last 32 bytes are a datum hash.
fn decode_tag3(data: &[u8]) -> Result<(MemPackTxOut, usize), SerializationError> {
    if data.len() < 62 {
        // 30 (tag+cred+hash) + 32 (datum hash) minimum
        return Err(SerializationError::CborDecode(
            "mempack_txout tag 3: need at least 62 bytes".into(),
        ));
    }

    let address = data[1..30].to_vec();

    let hash_start = data.len() - 32;
    let mut datum_hash = [0u8; 32];
    datum_hash.copy_from_slice(&data[hash_start..]);

    let opaque = if hash_start > 30 {
        Some(data[30..hash_start].to_vec())
    } else {
        None
    };

    Ok((
        MemPackTxOut {
            tag: 3,
            address,
            coin: 0,
            multi_asset: None,
            datum_hash: Some(datum_hash),
            datum: None,
            script_ref: None,
            opaque_tail: opaque,
        },
        data.len(),
    ))
}

/// Tag 4: `TxOutCompactDatum` — CompactAddr + CompactValue + inline datum.
///
/// The datum occupies all remaining bytes after CompactAddr + CompactValue.
/// There is no explicit length prefix for the datum.
fn decode_tag4(data: &[u8]) -> Result<(MemPackTxOut, usize), SerializationError> {
    let mut off = 1;

    let (address, addr_consumed) = decode_compact_addr(&data[off..])?;
    off += addr_consumed;

    // For ADA-only (value tag 0): coin VarLen, then datum is the rest.
    // For multi-asset (value tag 1): coin VarLen + multi-asset, then datum.
    // Since multi-asset has no self-delimiting length in this context, for
    // multi-asset we store value + datum together as opaque remaining bytes.
    if off >= data.len() {
        return Err(SerializationError::CborDecode(
            "mempack_txout tag 4: no value data".into(),
        ));
    }

    let value_tag = data[off];
    if value_tag == 0 {
        // ADA-only: parse coin, rest is datum.
        let val = decode_compact_value(&data[off..], None)?;
        off += val.consumed;
        let datum = if off < data.len() {
            Some(data[off..].to_vec())
        } else {
            None
        };
        Ok((
            MemPackTxOut {
                tag: 4,
                address,
                coin: val.coin,
                multi_asset: None,
                datum_hash: None,
                datum,
                script_ref: None,
                opaque_tail: None,
            },
            data.len(),
        ))
    } else {
        // Multi-asset: the boundary between multi-asset and datum is ambiguous
        // without fully parsing the multi-asset structure.  Store all remaining
        // bytes as opaque tail; coin is extracted from the VarLen.
        let val = decode_compact_value(&data[off..], Some(data.len() - off))?;
        Ok((
            MemPackTxOut {
                tag: 4,
                address,
                coin: val.coin,
                multi_asset: val.multi_asset_raw,
                datum_hash: None,
                datum: None, // Datum is interleaved in multi_asset/opaque_tail.
                script_ref: None,
                opaque_tail: None,
            },
            data.len(),
        ))
    }
}

/// Tag 5: `TxOutCompactRefScript` — CompactAddr + CompactValue + Datum + Script.
///
/// Similar to tag 4 but with an additional reference script after the datum.
/// Without fully parsing each sub-field boundary, we store the address and coin,
/// and put the remaining bytes into opaque_tail.
fn decode_tag5(data: &[u8]) -> Result<(MemPackTxOut, usize), SerializationError> {
    let mut off = 1;

    let (address, addr_consumed) = decode_compact_addr(&data[off..])?;
    off += addr_consumed;

    if off >= data.len() {
        return Err(SerializationError::CborDecode(
            "mempack_txout tag 5: no value data".into(),
        ));
    }

    let value_tag = data[off];
    if value_tag == 0 {
        // ADA-only value.
        let val = decode_compact_value(&data[off..], None)?;
        off += val.consumed;
        // Everything remaining is datum + script (opaque).
        let opaque = if off < data.len() {
            Some(data[off..].to_vec())
        } else {
            None
        };
        Ok((
            MemPackTxOut {
                tag: 5,
                address,
                coin: val.coin,
                multi_asset: None,
                datum_hash: None,
                datum: None,
                script_ref: None,
                opaque_tail: opaque,
            },
            data.len(),
        ))
    } else {
        // Multi-asset: store coin + opaque remaining.
        let val = decode_compact_value(&data[off..], Some(data.len() - off))?;
        Ok((
            MemPackTxOut {
                tag: 5,
                address,
                coin: val.coin,
                multi_asset: val.multi_asset_raw,
                datum_hash: None,
                datum: None,
                script_ref: None,
                opaque_tail: None,
            },
            data.len(),
        ))
    }
}
