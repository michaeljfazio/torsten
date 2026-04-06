//! ConwayGovState decoder for Haskell ledger snapshots.
//!
//! Haskell serialises `ConwayGovState` as a flat CBOR `array(7)` in this
//! fixed positional order:
//!
//! ```text
//! ConwayGovState = array(7) [
//!   [0] proposals:        complex structure — captured as raw CBOR bytes
//!   [1] committee:        StrictMaybe(Committee) — captured as raw CBOR bytes
//!   [2] constitution:     array(2) [Anchor, ScriptHash]
//!                           Anchor     = array(2) [url_text, bytes(32)]
//!                           ScriptHash = bytes(28) — direct bytestring, NOT wrapped
//!   [3] curPParams:       array(31) — decoded via decode_pparams
//!   [4] prevPParams:      array(31) — decoded via decode_pparams
//!   [5] futurePParams:    tagged sum —
//!                           array(1)[0]              = NoPParamsUpdate
//!                           array(2)[1, pp]          = DefinitePParamsUpdate(pp)
//!                           array(2)[2, array(0)]    = PotentialPParamsUpdate(SNothing)
//!                           array(2)[2, array(1)[pp]]= PotentialPParamsUpdate(SJust(pp))
//!   [6] drepPulsingState: complex structure — captured as raw CBOR bytes
//! ]
//! ```
//!
//! `cur_pparams` and `prev_pparams` are decoded into `ProtocolParameters`
//! and stored directly on `HaskellGovState` so the top-level decoder can
//! copy them into `HaskellNewEpochState` without re-decoding.
//!
//! All other complex sub-structures are preserved verbatim as raw CBOR bytes
//! so they can be decoded on-demand or passed through to consumers that need
//! the full fidelity wire format.

use crate::error::SerializationError;
use dugite_primitives::hash::Hash28;
use dugite_primitives::protocol_params::ProtocolParameters;

use super::cbor_utils::{
    decode_array_len, decode_bytes, decode_hash32, decode_text, decode_uint, skip_cbor_value,
};
use super::pparams::decode_pparams;
use super::types::{HaskellConstitution, HaskellGovState};

// ── Public entry point ──────────────────────────────────────────────────────

/// Decode a `ConwayGovState` from a CBOR `array(7)`.
///
/// Returns `(HaskellGovState, bytes_consumed)`.
///
/// `HaskellGovState::cur_pparams` and `::prev_pparams` are fully decoded
/// from positions [3] and [4] of the array.  Positions [0], [1], and [6] are
/// preserved verbatim as raw CBOR byte vectors.
pub fn decode_govstate(data: &[u8]) -> Result<(HaskellGovState, usize), SerializationError> {
    let mut off = 0;

    // ── outer array(7) header ───────────────────────────────────────────────
    let (arr_len, n) = decode_array_len(&data[off..])?;
    off += n;
    if arr_len != 7 {
        return Err(SerializationError::CborDecode(format!(
            "ConwayGovState: expected array(7), got array({arr_len})"
        )));
    }

    // ── [0] proposals — capture raw CBOR bytes ──────────────────────────────
    // The proposals structure is complex (ordered map of governance action IDs
    // to GovAction/Vote bundles); we preserve it for on-demand decoding.
    let proposals_start = off;
    let proposals_size = skip_cbor_value(&data[off..])?;
    let proposals_raw = data[proposals_start..proposals_start + proposals_size].to_vec();
    off += proposals_size;

    // ── [1] committee — StrictMaybe(Committee), capture raw CBOR bytes ──────
    // Haskell StrictMaybe encoding:
    //   SNothing → array(0)  = 0x80
    //   SJust x  → array(1) [x]
    // We capture the inner content (the Committee value) if present, stripping
    // the StrictMaybe wrapper since the wrapper itself is redundant overhead.
    let committee_raw = decode_strict_maybe_raw(&data[off..])?;
    let committee_size = skip_cbor_value(&data[off..])?;
    off += committee_size;

    // ── [2] constitution — array(2) [Anchor, ScriptHash] ────────────────────
    let (constitution, n) = decode_constitution(&data[off..])?;
    off += n;

    // ── [3] curPParams — array(31) ──────────────────────────────────────────
    let (cur_pparams, n) = decode_pparams(&data[off..])?;
    off += n;

    // ── [4] prevPParams — array(31) ─────────────────────────────────────────
    let (prev_pparams, n) = decode_pparams(&data[off..])?;
    off += n;

    // ── [5] futurePParams — tagged sum ──────────────────────────────────────
    let ((future_pparams_tag, future_pparams), n) = decode_future_pparams(&data[off..])?;
    off += n;

    // ── [6] drepPulsingState — capture raw CBOR bytes ───────────────────────
    // The DRep pulsing state encodes the incremental reward calculation in
    // progress; it is large (~1.3 MB on preview) and decoded separately if
    // needed.
    let drep_start = off;
    let drep_size = skip_cbor_value(&data[off..])?;
    let drep_pulsing_raw = data[drep_start..drep_start + drep_size].to_vec();
    off += drep_size;

    Ok((
        HaskellGovState {
            proposals_raw,
            committee_raw,
            constitution,
            cur_pparams,
            prev_pparams,
            future_pparams_tag,
            future_pparams,
            drep_pulsing_raw,
        },
        off,
    ))
}

// ── Internal helpers ────────────────────────────────────────────────────────

/// Decode a Haskell `StrictMaybe T` where T is any CBOR value, returning the
/// inner raw CBOR bytes if `SJust`, or `None` if `SNothing`.
///
/// Encoding:
///   SNothing → `array(0)`   = `[0x80]`
///   SJust x  → `array(1) [x]`
///
/// The returned `Vec<u8>` is the raw CBOR of the inner value only (not
/// including the wrapping array header), so callers get the Committee bytes
/// directly rather than the StrictMaybe wrapper.
fn decode_strict_maybe_raw(data: &[u8]) -> Result<Option<Vec<u8>>, SerializationError> {
    let (arr_len, hdr) = decode_array_len(data)?;
    match arr_len {
        // SNothing — nothing to capture.
        0 => Ok(None),
        // SJust — the inner value starts immediately after the array header.
        1 => {
            let inner_size = skip_cbor_value(&data[hdr..])?;
            Ok(Some(data[hdr..hdr + inner_size].to_vec()))
        }
        n => Err(SerializationError::CborDecode(format!(
            "StrictMaybe: expected array(0) or array(1), got array({n})"
        ))),
    }
}

/// Decode a `Constitution` value:
/// ```text
/// array(2) [
///   Anchor     = array(2) [text_url, bytes(32)],
///   ScriptHash = bytes(28)          -- direct bytestring (not StrictMaybe)
/// ]
/// ```
///
/// The anchor hash is 32 bytes; the script hash is 28 bytes.  On-chain,
/// if no script hash guard is associated, the constitution can still carry
/// a default hash (the wire format in preview epoch 1259 always includes a
/// 28-byte script hash directly, not wrapped in a StrictMaybe).
///
/// Returns `(Option<HaskellConstitution>, bytes_consumed)`.  A `None` is
/// returned only if the outer array is empty; in practice this value is
/// always present after the Conway hard fork.
fn decode_constitution(
    data: &[u8],
) -> Result<(Option<HaskellConstitution>, usize), SerializationError> {
    let mut off = 0;

    let (arr_len, n) = decode_array_len(&data[off..])?;
    off += n;

    // An empty array encodes an absent constitution (pre-Conway or initial).
    if arr_len == 0 {
        return Ok((None, off));
    }

    if arr_len != 2 {
        return Err(SerializationError::CborDecode(format!(
            "Constitution: expected array(0) or array(2), got array({arr_len})"
        )));
    }

    // ── Anchor = array(2) [text_url, bytes(32)] ─────────────────────────────
    let (anchor_arr_len, n) = decode_array_len(&data[off..])?;
    off += n;
    if anchor_arr_len != 2 {
        return Err(SerializationError::CborDecode(format!(
            "Constitution Anchor: expected array(2), got array({anchor_arr_len})"
        )));
    }

    let (url_str, n) = decode_text(&data[off..])?;
    let anchor_url = url_str.to_owned();
    off += n;

    let (anchor_hash, n) = decode_hash32(&data[off..])?;
    off += n;

    // ── ScriptHash = bytes(28) ───────────────────────────────────────────────
    // In the Haskell wire format for Conway ConwayGovState, the script hash is
    // encoded as a direct bytestring — NOT wrapped in a StrictMaybe array.
    // Verified against preview epoch 1259 fixture.
    let script_hash = decode_optional_script_hash(&data[off..])?;
    let sh_size = skip_cbor_value(&data[off..])?;
    off += sh_size;

    Ok((
        Some(HaskellConstitution {
            anchor_url,
            anchor_hash,
            script_hash,
        }),
        off,
    ))
}

/// Decode the optional script hash field in a Constitution.
///
/// Haskell encodes this as a direct CBOR `bytes(28)` bytestring (not wrapped
/// in a StrictMaybe).  If the major type is not 2 (bytestring) the field is
/// treated as absent and `None` is returned — this guards against snapshots
/// from future protocol versions that might change the encoding.
fn decode_optional_script_hash(data: &[u8]) -> Result<Option<Hash28>, SerializationError> {
    if data.is_empty() {
        return Ok(None);
    }
    let major = data[0] >> 5;
    // Major type 2 = bytestring; 28 bytes → Hash28.
    if major != 2 {
        return Ok(None);
    }
    let (bytes, _) = decode_bytes(data)?;
    if bytes.len() != 28 {
        return Err(SerializationError::InvalidLength {
            expected: 28,
            got: bytes.len(),
        });
    }
    Ok(Some(Hash28::from_bytes(bytes.try_into().unwrap())))
}

/// Decode the `FuturePParams` tagged sum.
///
/// Haskell encoding (verified against preview epoch 1259):
/// ```text
/// NoPParamsUpdate              → array(1) [0]
/// DefinitePParamsUpdate pp     → array(2) [1, pp]
/// PotentialPParamsUpdate sm    → array(2) [2, StrictMaybe(pp)]
///   where sm = SNothing        → array(0)
///         sm = SJust pp        → array(1) [pp]
/// ```
///
/// Returns `((tag, Option<ProtocolParameters>), bytes_consumed)` where
/// `tag` encodes the variant:
///   - 0 = NoPParamsUpdate
///   - 1 = DefinitePParamsUpdate
///   - 2 = PotentialPParamsUpdate
fn decode_future_pparams(
    data: &[u8],
) -> Result<((u8, Option<ProtocolParameters>), usize), SerializationError> {
    let mut off = 0;

    let (arr_len, n) = decode_array_len(&data[off..])?;
    off += n;

    // Read the variant tag (always a uint).
    let (tag, n) = decode_uint(&data[off..])?;
    off += n;

    match (arr_len, tag) {
        // NoPParamsUpdate = array(1) [0]
        (1, 0) => Ok(((0, None), off)),

        // DefinitePParamsUpdate = array(2) [1, pp]
        (2, 1) => {
            let (pp, n) = decode_pparams(&data[off..])?;
            off += n;
            Ok(((1, Some(pp)), off))
        }

        // PotentialPParamsUpdate = array(2) [2, StrictMaybe(pp)]
        (2, 2) => {
            // Decode the inner StrictMaybe.
            let (inner_arr_len, n) = decode_array_len(&data[off..])?;
            off += n;
            match inner_arr_len {
                // SNothing — no future PParams queued.
                0 => Ok(((2, None), off)),
                // SJust pp — a potential future PParams update.
                1 => {
                    let (pp, n) = decode_pparams(&data[off..])?;
                    off += n;
                    Ok(((2, Some(pp)), off))
                }
                n => Err(SerializationError::CborDecode(format!(
                    "FuturePParams: PotentialPParamsUpdate StrictMaybe: \
                     expected array(0) or array(1), got array({n})"
                ))),
            }
        }

        _ => Err(SerializationError::CborDecode(format!(
            "FuturePParams: unexpected array({arr_len}) tag {tag}"
        ))),
    }
}
