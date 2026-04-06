//! Decoder for the Haskell `PraosState` snapshot format.
//!
//! The Haskell node serialises the Praos consensus state as a versioned wrapper:
//!
//! ```text
//! PraosState_versioned = array(2) [
//!   version = 0,
//!   PraosState = array(7|8) [
//!     [0] lastSlot:              WithOrigin(SlotNo)  — [] or [slot]
//!     [1] oCertCounters:         map(bytes(28) → uint)
//!     [2] evolvingNonce:         Nonce  — [0] or [1, bytes(32)]
//!     [3] candidateNonce:        Nonce
//!     [4] epochNonce:            Nonce
//!     [5] previousEpochNonce:    Nonce  — ONLY present in array(8)
//!     [5|6] labNonce:            Nonce  — index 5 in array(7), 6 in array(8)
//!     [6|7] lastEpochBlockNonce: Nonce  — index 6 in array(7), 7 in array(8)
//!   ]
//! ]
//! ```
//!
//! The optional `previousEpochNonce` field was added between Babbage and Conway
//! serialisers in the Haskell codebase; we need to handle both layouts.

use crate::error::SerializationError;
use crate::haskell_snapshot::cbor_utils::{
    decode_array_len, decode_hash28, decode_map_len, decode_nonce, decode_uint, skip_cbor_value,
};
use crate::haskell_snapshot::types::HaskellPraosState;
use dugite_primitives::time::SlotNo;
use std::collections::HashMap;

/// Decode a versioned `PraosState` snapshot produced by the Haskell node.
///
/// Returns `(praos_state, bytes_consumed)`.  The function consumes **all**
/// bytes in `data` that belong to this value; the caller can verify that
/// `consumed == data.len()` if the fixture is expected to contain exactly one
/// top-level value.
pub fn decode_praos_state(data: &[u8]) -> Result<(HaskellPraosState, usize), SerializationError> {
    let mut off = 0;

    // ── outer versioned wrapper: array(2) [version, PraosState] ──────────────
    let (outer_len, n) = decode_array_len(&data[off..])?;
    off += n;
    if outer_len != 2 {
        return Err(SerializationError::CborDecode(format!(
            "PraosState outer array: expected 2 elements, got {outer_len}"
        )));
    }

    // version must be 0
    let (version, n) = decode_uint(&data[off..])?;
    off += n;
    if version != 0 {
        return Err(SerializationError::CborDecode(format!(
            "PraosState version: expected 0, got {version}"
        )));
    }

    // ── inner PraosState: array(7) or array(8) ────────────────────────────────
    let (inner_len, n) = decode_array_len(&data[off..])?;
    off += n;
    if inner_len != 7 && inner_len != 8 {
        return Err(SerializationError::CborDecode(format!(
            "PraosState inner array: expected 7 or 8 elements, got {inner_len}"
        )));
    }
    // Whether a `previousEpochNonce` field is present before labNonce.
    let has_previous_epoch_nonce = inner_len == 8;

    // ── [0] lastSlot: WithOrigin(SlotNo) ─────────────────────────────────────
    //
    // Haskell encodes `WithOrigin SlotNo` identically to `Nonce`:
    //   array(1) [0]          → Origin
    //   array(2) [1, slotNo]  → At slotNo
    let (arr_len, n) = decode_array_len(&data[off..])?;
    off += n;
    let (tag, n) = decode_uint(&data[off..])?;
    off += n;
    let last_slot = match (arr_len, tag) {
        (1, 0) => None,
        (2, 1) => {
            let (slot, n) = decode_uint(&data[off..])?;
            off += n;
            Some(SlotNo(slot))
        }
        _ => {
            return Err(SerializationError::CborDecode(format!(
                "lastSlot WithOrigin: expected array(1)[0] or array(2)[1, slot], \
                 got array({arr_len})[tag={tag}]"
            )))
        }
    };

    // ── [1] oCertCounters: map(bytes(28) → uint) ──────────────────────────────
    let (map_len_opt, n) = decode_map_len(&data[off..])?;
    off += n;

    let mut opcert_counters = HashMap::new();

    match map_len_opt {
        Some(map_len) => {
            // Definite-length map.
            for _ in 0..map_len {
                let (key, n) = decode_hash28(&data[off..])?;
                off += n;
                let (val, n) = decode_uint(&data[off..])?;
                off += n;
                opcert_counters.insert(key, val);
            }
        }
        None => {
            // Indefinite-length map: consume until break code 0xff.
            while off < data.len() && data[off] != 0xff {
                let (key, n) = decode_hash28(&data[off..])?;
                off += n;
                let (val, n) = decode_uint(&data[off..])?;
                off += n;
                opcert_counters.insert(key, val);
            }
            // Consume the break byte.
            if off >= data.len() {
                return Err(SerializationError::CborDecode(
                    "oCertCounters: unexpected end of input (missing break byte)".into(),
                ));
            }
            off += 1; // skip 0xff
        }
    }

    // ── [2] evolvingNonce ─────────────────────────────────────────────────────
    let (evolving_nonce, n) = decode_nonce(&data[off..])?;
    off += n;

    // ── [3] candidateNonce ────────────────────────────────────────────────────
    let (candidate_nonce, n) = decode_nonce(&data[off..])?;
    off += n;

    // ── [4] epochNonce ────────────────────────────────────────────────────────
    let (epoch_nonce, n) = decode_nonce(&data[off..])?;
    off += n;

    // ── [5] previousEpochNonce (array(8) only) — skip it ─────────────────────
    if has_previous_epoch_nonce {
        let n = skip_cbor_value(&data[off..])?;
        off += n;
    }

    // ── [5|6] labNonce ────────────────────────────────────────────────────────
    let (lab_nonce, n) = decode_nonce(&data[off..])?;
    off += n;

    // ── [6|7] lastEpochBlockNonce ─────────────────────────────────────────────
    let (last_epoch_block_nonce, n) = decode_nonce(&data[off..])?;
    off += n;

    Ok((
        HaskellPraosState {
            last_slot,
            opcert_counters,
            evolving_nonce,
            candidate_nonce,
            epoch_nonce,
            lab_nonce,
            last_epoch_block_nonce,
        },
        off,
    ))
}
