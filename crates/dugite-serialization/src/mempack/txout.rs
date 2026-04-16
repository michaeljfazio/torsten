//! MemPack TxOut decoder.
//!
//! The first byte of a MemPack TxOut blob selects the Haskell constructor variant,
//! as defined in `Cardano.Ledger.Alonzo.TxOut` (eras/alonzo/impl/src/Cardano/Ledger/Alonzo/TxOut.hs):
//!
//! | Tag | Variant                                | Fields                                        |
//! |-----|----------------------------------------|-----------------------------------------------|
//! |  0  | `TxOutCompact'`                        | CompactAddr + CompactValue                    |
//! |  1  | `TxOutCompactDH'`                      | CompactAddr + CompactValue + DataHash         |
//! |  2  | `TxOut_AddrHash28_AdaOnly`             | Credential Staking + Addr28Extra + Coin       |
//! |  3  | `TxOut_AddrHash28_AdaOnly_DataHash32`  | Credential Staking + Addr28Extra + Coin + DH  |
//! |  4  | `TxOutCompactDatum` (Babbage+)         | CompactAddr + CompactValue + Datum            |
//! |  5  | `TxOutCompactRefScript` (Babbage+)     | CompactAddr + CompactValue + Datum + Script   |
//!
//! ## Tags 2 and 3 — `Addr28Extra` packed form
//!
//! When a TxOut is an ADA-only output at a base address whose payment and stake
//! credentials are both 28-byte hashes, cardano-ledger uses a compact encoding:
//!
//! ```text
//! tag(1)
//!   Credential Staking           (1-byte tag + 28-byte hash = 29 bytes)
//!   Addr28Extra                  (32 bytes = 4 × Word64 native-endian)
//!   CompactForm Coin             (1 inner tag + VarLen Word64)
//!   [DataHash32 — tag 3 only]    (32 bytes = 4 × Word64 native-endian)
//! ```
//!
//! The `Addr28Extra` holds the payment hash28 plus a 4-bit metadata nibble
//! (network + payment-credential type). Port of the Haskell layout:
//!
//! * `Credential Staking` tag: `0` = `ScriptHashObj`, `1` = `KeyHashObj`
//!   (see `Cardano.Ledger.Credential`). **Note**: this tag convention is the
//!   opposite of the payment-cred bit inside `Addr28Extra`.
//!
//! * `Addr28Extra` = four `Word64` values `(w0, w1, w2, w3)` serialized via
//!   MemPack's native `packM @Word64`, which writes each word as a **native
//!   endian** 8-byte chunk. On all Cardano build targets (x86_64, aarch64) that
//!   is little-endian. The 28-byte payment hash is reconstructed from
//!   `PackedBytes28 w0 w1 w2 (w3 >> 32 :: Word32)` where each slot is written
//!   as **big-endian** bytes (see `Cardano.Crypto.PackedBytes.Internal`):
//!
//!   ```text
//!   payment_hash28 = be_u64(w0) ‖ be_u64(w1) ‖ be_u64(w2) ‖ be_u32(w3 >> 32)
//!   ```
//!
//!   The low 32 bits of `w3` carry the metadata:
//!   - bit 0 (`d.testBit 0`): `1` = `KeyHashObj`, `0` = `ScriptHashObj`
//!   - bit 1 (`d.testBit 1`): `1` = `Mainnet`,    `0` = `Testnet`
//!
//!   See `encodeAddress28` / `decodeAddress28` in `Cardano.Ledger.Alonzo.TxOut`.
//!
//! * `CompactForm Coin` is serialized as `packTagM 0 >> packM (VarLen c)` — an
//!   inner 1-byte tag (`0x00`) followed by a MemPack VarLen Word64. See the
//!   `MemPack (CompactForm Coin)` instance in `Cardano.Ledger.Coin`.
//!
//! * `DataHash32` has the same layout as `Addr28Extra` — 4×Word64 LE — but all
//!   four words form the full 32-byte datum hash (no metadata bits).
//!
//! References:
//! - `IntersectMBO/cardano-ledger/eras/alonzo/impl/src/Cardano/Ledger/Alonzo/TxOut.hs`
//!   (lines ~99-198: `Addr28Extra`, `DataHash32`, `AlonzoTxOut`, `decodeAddress28`,
//!   `MemPack AlonzoTxOut`)
//! - `IntersectMBO/cardano-ledger/libs/cardano-ledger-core/src/Cardano/Ledger/Coin.hs`
//!   (lines ~154-164: `instance MemPack (CompactForm Coin)`)
//! - `IntersectMBO/cardano-ledger/libs/cardano-ledger-core/src/Cardano/Ledger/Credential.hs`
//!   (lines ~99-112: `instance MemPack (Credential kr)`)
//! - `IntersectMBO/cardano-ledger/libs/cardano-ledger-core/src/Cardano/Ledger/Address.hs`
//!   (lines ~266-304: Shelley address header bit layout and `putAddr`)
//! - `IntersectMBO/cardano-base/cardano-crypto-class/src/Cardano/Crypto/PackedBytes/Internal.hs`
//!   (lines ~113-134: `MemPack (PackedBytes n)` — hash slots use big-endian
//!   `writeWord64BE`/`writeWord32BE`)

use crate::error::SerializationError;
use crate::mempack::compact::{decode_compact_addr, decode_compact_value, decode_varlen};

/// A decoded MemPack TxOut.
///
/// Fields are populated according to the tag variant. For every tag, `address`
/// holds a fully-formed Shelley/Byron address byte sequence that can be fed
/// directly into `dugite_primitives::address::Address::from_bytes`, and `coin`
/// holds the lovelace amount (possibly `0` for a multi-asset-only output).
#[derive(Debug, Clone)]
pub struct MemPackTxOut {
    /// MemPack constructor tag (0–5).
    pub tag: u8,
    /// Fully decoded Shelley (or Byron) address bytes, ready for
    /// `Address::from_bytes`.
    pub address: Vec<u8>,
    /// Lovelace amount.
    pub coin: u64,
    /// Raw multi-asset bytes (when CompactValue tag = 1).
    pub multi_asset: Option<Vec<u8>>,
    /// 32-byte datum hash (tags 1, 3).
    pub datum_hash: Option<[u8; 32]>,
    /// Inline datum bytes (tags 4, 5).
    pub datum: Option<Vec<u8>>,
    /// Reference script bytes (tag 5).
    pub script_ref: Option<Vec<u8>>,
    /// Opaque remaining bytes for variants we cannot fully split yet (tag 5
    /// multi-asset payloads, etc.). For tags 0–3 this is always `None`.
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

/// Intermediate result of decoding an `Addr28Extra + CompactCoin` payload.
struct Addr28Decoded {
    /// Fully assembled 57-byte Shelley base address.
    address: Vec<u8>,
    /// Lovelace amount from the `CompactForm Coin` VarLen.
    coin: u64,
    /// Total bytes consumed (29 cred + 32 addr28extra + `CompactCoin` length).
    consumed: usize,
}

/// Decode `Credential Staking + Addr28Extra + CompactForm Coin`, returning the
/// reconstructed Shelley base address, coin value, and total bytes consumed.
///
/// `data` must start with the `Credential Staking` tag byte (i.e. `&blob[1..]`
/// for a tag-2/tag-3 TxOut blob). This is shared by both tag-2 and tag-3
/// decoders because the prefix is identical.
fn decode_addr28_payload(data: &[u8]) -> Result<Addr28Decoded, SerializationError> {
    // Credential Staking: 1-byte tag (0 = ScriptHash, 1 = KeyHash) + 28-byte hash.
    if data.len() < 29 {
        return Err(SerializationError::CborDecode(
            "mempack_txout tag 2/3: truncated Credential Staking".into(),
        ));
    }
    let stake_cred_tag = data[0];
    let stake_hash: &[u8; 28] = data[1..29]
        .try_into()
        .expect("slice of length 28 fits [u8; 28]");

    // Addr28Extra: 4 × Word64 native-endian (= little-endian on x86_64/aarch64).
    if data.len() < 29 + 32 {
        return Err(SerializationError::CborDecode(
            "mempack_txout tag 2/3: truncated Addr28Extra".into(),
        ));
    }
    let ae = &data[29..29 + 32];
    let w0 = u64::from_le_bytes(ae[0..8].try_into().unwrap());
    let w1 = u64::from_le_bytes(ae[8..16].try_into().unwrap());
    let w2 = u64::from_le_bytes(ae[16..24].try_into().unwrap());
    let w3 = u64::from_le_bytes(ae[24..32].try_into().unwrap());

    // Payment hash28 = BE(w0) ‖ BE(w1) ‖ BE(w2) ‖ BE(w3 >> 32 as u32).
    let mut payment_hash = [0u8; 28];
    payment_hash[0..8].copy_from_slice(&w0.to_be_bytes());
    payment_hash[8..16].copy_from_slice(&w1.to_be_bytes());
    payment_hash[16..24].copy_from_slice(&w2.to_be_bytes());
    let w3_top: u32 = (w3 >> 32) as u32;
    payment_hash[24..28].copy_from_slice(&w3_top.to_be_bytes());

    // Metadata bits live in the low 32 bits of w3.
    let meta = w3 as u32;
    let payment_is_key = (meta & 0b01) != 0; // bit 0
    let is_mainnet = (meta & 0b10) != 0; // bit 1

    // Reconstruct the 57-byte Shelley base address: header(1) + pay28 + stake28.
    //
    // Shelley base-address header (see Cardano.Ledger.Address):
    //   bit 0: mainnet (1) vs testnet (0)
    //   bit 4: payCredIsScript
    //   bit 5: stakeCredIsScript
    //   bits 6-7: 0b00 (base address)
    //
    // The staking credential tag convention here is 0 = ScriptHashObj,
    // 1 = KeyHashObj (Credential MemPack instance).
    let stake_is_script = match stake_cred_tag {
        0 => true,
        1 => false,
        other => {
            return Err(SerializationError::CborDecode(format!(
                "mempack_txout tag 2/3: invalid Credential Staking tag {other} (expected 0 or 1)"
            )));
        }
    };

    let mut header: u8 = 0;
    if is_mainnet {
        header |= 0b0000_0001;
    }
    if !payment_is_key {
        header |= 0b0001_0000;
    }
    if stake_is_script {
        header |= 0b0010_0000;
    }

    let mut address = Vec::with_capacity(57);
    address.push(header);
    address.extend_from_slice(&payment_hash);
    address.extend_from_slice(stake_hash);
    debug_assert_eq!(address.len(), 57);

    // CompactForm Coin: inner tag byte (must be 0) + VarLen Word64.
    let coin_start = 29 + 32;
    if coin_start >= data.len() {
        return Err(SerializationError::CborDecode(
            "mempack_txout tag 2/3: truncated before CompactCoin".into(),
        ));
    }
    let inner_tag = data[coin_start];
    if inner_tag != 0 {
        return Err(SerializationError::CborDecode(format!(
            "mempack_txout tag 2/3: unexpected CompactCoin inner tag {inner_tag}"
        )));
    }
    let (coin, coin_varlen_bytes) = decode_varlen(&data[coin_start + 1..])?;
    let consumed = coin_start + 1 + coin_varlen_bytes;

    Ok(Addr28Decoded {
        address,
        coin,
        consumed,
    })
}

/// Tag 2: `TxOut_AddrHash28_AdaOnly` — Credential Staking + Addr28Extra + Coin.
///
/// Yields a fully-decoded 57-byte Shelley base address and the exact lovelace
/// amount (byte-for-byte compatible with what a tag-0 decode would produce for
/// the same logical UTxO).
fn decode_tag2(data: &[u8]) -> Result<(MemPackTxOut, usize), SerializationError> {
    // Skip outer tag byte.
    let decoded = decode_addr28_payload(&data[1..])?;
    let consumed = 1 + decoded.consumed;
    Ok((
        MemPackTxOut {
            tag: 2,
            address: decoded.address,
            coin: decoded.coin,
            multi_asset: None,
            datum_hash: None,
            datum: None,
            script_ref: None,
            opaque_tail: None,
        },
        consumed,
    ))
}

/// Tag 3: `TxOut_AddrHash28_AdaOnly_DataHash32` — tag 2 plus a trailing
/// `DataHash32` (32 raw bytes interpreted as 4 × Word64 little-endian, then
/// re-serialized big-endian to recover the original datum hash).
fn decode_tag3(data: &[u8]) -> Result<(MemPackTxOut, usize), SerializationError> {
    let decoded = decode_addr28_payload(&data[1..])?;
    let after_coin = 1 + decoded.consumed;

    if data.len() < after_coin + 32 {
        return Err(SerializationError::CborDecode(
            "mempack_txout tag 3: truncated DataHash32".into(),
        ));
    }
    let dh_slice = &data[after_coin..after_coin + 32];
    let dw0 = u64::from_le_bytes(dh_slice[0..8].try_into().unwrap());
    let dw1 = u64::from_le_bytes(dh_slice[8..16].try_into().unwrap());
    let dw2 = u64::from_le_bytes(dh_slice[16..24].try_into().unwrap());
    let dw3 = u64::from_le_bytes(dh_slice[24..32].try_into().unwrap());
    let mut datum_hash = [0u8; 32];
    datum_hash[0..8].copy_from_slice(&dw0.to_be_bytes());
    datum_hash[8..16].copy_from_slice(&dw1.to_be_bytes());
    datum_hash[16..24].copy_from_slice(&dw2.to_be_bytes());
    datum_hash[24..32].copy_from_slice(&dw3.to_be_bytes());

    Ok((
        MemPackTxOut {
            tag: 3,
            address: decoded.address,
            coin: decoded.coin,
            multi_asset: None,
            datum_hash: Some(datum_hash),
            datum: None,
            script_ref: None,
            opaque_tail: None,
        },
        after_coin + 32,
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
