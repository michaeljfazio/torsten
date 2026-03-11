//! HFC (Hard Fork Combinator) telescope unwrapping.
//!
//! Haskell ledger state files wrap the era-specific state in multiple
//! layers of HFC encoding. This module peels off those layers to reach
//! the Conway-era NewEpochState payload.
//!
//! ## Structure
//!
//! The on-disk format is an `ExtLedgerState` (array(2)):
//!
//! ```text
//! ExtLedgerState: array(2)
//!   [0] LedgerState HFC Telescope
//!   [1] HeaderState HFC Telescope (skipped)
//! ```
//!
//! The LedgerState telescope for Conway (era index 6) is:
//!
//! ```text
//! array(7)                                          -- 1 + era_index(6)
//!   [0] past_era_0: array(2) [Bound_start, Bound_end]  -- Byron
//!   [1] past_era_1: array(2) [Bound_start, Bound_end]  -- Shelley
//!   [2] past_era_2: array(2) [Bound_start, Bound_end]  -- Allegra
//!   [3] past_era_3: array(2) [Bound_start, Bound_end]  -- Mary
//!   [4] past_era_4: array(2) [Bound_start, Bound_end]  -- Alonzo
//!   [5] past_era_5: array(2) [Bound_start, Bound_end]  -- Babbage
//!   [6] current_era: array(2) [Bound_start, NewEpochState]  -- Conway
//! ```
//!
//! Each `Bound` is: `array(3) [RelativeTime(rational_or_integer), SlotNo(u64), EpochNo(u64)]`

use crate::error::SerializationError;
use tracing::{debug, info};

/// Era names, indexed by HFC era index (0 = Byron, 6 = Conway).
const ERA_NAMES: &[&str] = &[
    "Byron", "Shelley", "Allegra", "Mary", "Alonzo", "Babbage", "Conway",
];

/// An epoch boundary extracted from the HFC telescope.
#[derive(Debug, Clone)]
pub struct EraBound {
    /// Slot number at the boundary.
    pub slot: u64,
    /// Epoch number at the boundary.
    pub epoch: u64,
}

/// Information returned after unwrapping the HFC telescope.
#[derive(Debug)]
#[allow(dead_code)]
pub struct TelescopeInfo {
    /// Era index of the current (innermost) era (e.g. 6 = Conway).
    pub era_index: u64,
    /// Epoch boundaries for each past era. Each entry is `(start_bound, end_bound)`.
    /// Length equals `era_index` (one per completed era).
    pub past_era_bounds: Vec<(EraBound, EraBound)>,
    /// Start bound of the current era.
    pub current_era_start: EraBound,
}

/// Unwrap the HFC telescope to position the decoder at the Conway-era
/// `NewEpochState`.
///
/// After a successful call the decoder is positioned at the first byte of
/// the `NewEpochState` CBOR value inside the current era's telescope entry.
///
/// Returns a [`TelescopeInfo`] containing the era index and the epoch
/// boundaries discovered while traversing the telescope.
pub fn unwrap_hfc_telescope(
    d: &mut minicbor::Decoder,
) -> Result<TelescopeInfo, SerializationError> {
    // ---- ExtLedgerState: array(2) [LedgerState telescope, HeaderState telescope] ----
    let ext_len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("ExtLedgerState: expected definite array".into())
    })?;
    if ext_len != 2 {
        return Err(SerializationError::CborDecode(format!(
            "ExtLedgerState: expected array(2), got array({ext_len})"
        )));
    }

    // ---- LedgerState HFC Telescope ----
    // The telescope is array(N) where N = 1 + era_index.
    // For Conway, N = 7 (eras 0..5 are past, era 6 is current).
    let telescope_len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("LedgerState telescope: expected definite array".into())
    })?;
    if telescope_len < 1 {
        return Err(SerializationError::CborDecode(
            "LedgerState telescope: empty array (no eras present)".into(),
        ));
    }
    let era_index = telescope_len - 1;

    let era_name = ERA_NAMES
        .get(era_index as usize)
        .copied()
        .unwrap_or("unknown");
    info!(
        "HFC telescope: {} past era(s), current era {} ({era_name})",
        era_index, era_index
    );

    // ---- Skip past eras (indices 0 .. era_index-1) ----
    // Each past era entry is: array(2) [Bound_start, Bound_end]
    let mut past_era_bounds = Vec::with_capacity(era_index as usize);
    for i in 0..era_index {
        let past_name = ERA_NAMES.get(i as usize).copied().unwrap_or("unknown");
        let (start, end) = decode_past_era_entry(d).map_err(|e| {
            SerializationError::CborDecode(format!("telescope past era {i} ({past_name}): {e}"))
        })?;
        debug!(
            "  past era {i} ({past_name}): slots {}..{}, epochs {}..{}",
            start.slot, end.slot, start.epoch, end.epoch
        );
        past_era_bounds.push((start, end));
    }

    // ---- Current era entry: array(2) [Bound_start, NewEpochState] ----
    let cur_len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode(format!(
            "telescope current era {era_index} ({era_name}): expected definite array"
        ))
    })?;
    if cur_len != 2 {
        return Err(SerializationError::CborDecode(format!(
            "telescope current era {era_index} ({era_name}): \
             expected array(2) [Bound, NewEpochState], got array({cur_len})"
        )));
    }

    // Decode the start Bound, then leave decoder at NewEpochState.
    let current_era_start = decode_bound(d).map_err(|e| {
        SerializationError::CborDecode(format!(
            "telescope current era {era_index} ({era_name}) start bound: {e}"
        ))
    })?;
    info!(
        "  current era {era_index} ({era_name}): start slot {}, epoch {}",
        current_era_start.slot, current_era_start.epoch
    );

    // Decoder is now positioned at the NewEpochState payload.
    // The caller will NOT call d.skip() on the HeaderState telescope --
    // the HeaderState is the second element of ExtLedgerState and sits
    // after the NewEpochState bytes that the caller will fully consume.

    Ok(TelescopeInfo {
        era_index,
        past_era_bounds,
        current_era_start,
    })
}

/// Decode a past-era telescope entry: `array(2) [Bound_start, Bound_end]`.
fn decode_past_era_entry(
    d: &mut minicbor::Decoder,
) -> Result<(EraBound, EraBound), SerializationError> {
    let len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("past era entry: expected definite array".into())
    })?;
    if len != 2 {
        return Err(SerializationError::CborDecode(format!(
            "past era entry: expected array(2), got array({len})"
        )));
    }
    let start = decode_bound(d)?;
    let end = decode_bound(d)?;
    Ok((start, end))
}

/// Decode a single HFC `Bound`: `array(3) [RelativeTime, SlotNo, EpochNo]`.
///
/// `RelativeTime` may be encoded as:
/// - a plain integer (0 at the start)
/// - a `Tag(30)` rational `[numerator, denominator]`
/// - a CBOR float / half-float
///
/// We skip RelativeTime (we only need slot and epoch) but must consume it
/// correctly regardless of encoding.
fn decode_bound(d: &mut minicbor::Decoder) -> Result<EraBound, SerializationError> {
    let len = d
        .array()?
        .ok_or_else(|| SerializationError::CborDecode("Bound: expected definite array".into()))?;
    if len != 3 {
        return Err(SerializationError::CborDecode(format!(
            "Bound: expected array(3), got array({len})"
        )));
    }

    // [0] RelativeTime -- skip it (may be integer, rational, or float)
    d.skip()?;

    // [1] SlotNo (u64)
    let slot = d.u64()?;

    // [2] EpochNo (u64)
    let epoch = d.u64()?;

    Ok(EraBound { slot, epoch })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: encode a Bound as array(3) [relative_time, slot, epoch].
    fn encode_bound(e: &mut minicbor::Encoder<&mut Vec<u8>>, slot: u64, epoch: u64) {
        e.array(3).unwrap();
        e.u64(0).unwrap(); // RelativeTime (placeholder)
        e.u64(slot).unwrap();
        e.u64(epoch).unwrap();
    }

    /// Build a minimal HFC telescope in CBOR (Conway = era 6) and verify
    /// that unwrapping positions the decoder at the payload.
    #[test]
    fn test_unwrap_conway_telescope() {
        let mut buf = Vec::new();
        let mut e = minicbor::Encoder::new(&mut buf);

        // ExtLedgerState: array(2)
        e.array(2).unwrap();

        // [0] LedgerState telescope: array(7) for Conway
        e.array(7).unwrap();

        // Past eras 0..5: each is array(2) [Bound_start, Bound_end]
        for era in 0..6u64 {
            e.array(2).unwrap();
            encode_bound(&mut e, era * 1000, era * 10);
            encode_bound(&mut e, (era + 1) * 1000, (era + 1) * 10);
        }

        // Current era (Conway): array(2) [Bound, payload]
        e.array(2).unwrap();
        encode_bound(&mut e, 6000, 60);

        // Payload: a simple integer as a stand-in for NewEpochState
        e.u64(42).unwrap();

        // [1] HeaderState telescope (just a dummy integer)
        e.u64(0).unwrap();

        // Now decode
        let mut decoder = minicbor::Decoder::new(&buf);
        let info = unwrap_hfc_telescope(&mut decoder).unwrap();

        assert_eq!(info.era_index, 6, "should detect Conway (era 6)");
        assert_eq!(info.past_era_bounds.len(), 6);

        // Verify past era boundaries
        for (i, (start, end)) in info.past_era_bounds.iter().enumerate() {
            let era = i as u64;
            assert_eq!(start.slot, era * 1000);
            assert_eq!(start.epoch, era * 10);
            assert_eq!(end.slot, (era + 1) * 1000);
            assert_eq!(end.epoch, (era + 1) * 10);
        }

        // Verify current era start
        assert_eq!(info.current_era_start.slot, 6000);
        assert_eq!(info.current_era_start.epoch, 60);

        // Decoder should now be at the payload
        let payload = decoder.u64().unwrap();
        assert_eq!(payload, 42);
    }

    /// Verify that a Babbage-era telescope (era index 5) is handled.
    #[test]
    fn test_unwrap_babbage_telescope() {
        let mut buf = Vec::new();
        let mut e = minicbor::Encoder::new(&mut buf);

        // ExtLedgerState: array(2)
        e.array(2).unwrap();

        // LedgerState telescope: array(6) for Babbage
        e.array(6).unwrap();

        // Past eras 0..4
        for era in 0..5u64 {
            e.array(2).unwrap();
            encode_bound(&mut e, era * 100, era);
            encode_bound(&mut e, (era + 1) * 100, era + 1);
        }

        // Current era (Babbage): array(2) [Bound, payload]
        e.array(2).unwrap();
        encode_bound(&mut e, 500, 5);
        e.u64(99).unwrap(); // payload

        // HeaderState (dummy)
        e.u64(0).unwrap();

        let mut decoder = minicbor::Decoder::new(&buf);
        let info = unwrap_hfc_telescope(&mut decoder).unwrap();

        assert_eq!(info.era_index, 5, "should detect Babbage (era 5)");
        assert_eq!(info.past_era_bounds.len(), 5);
        assert_eq!(info.current_era_start.slot, 500);
        assert_eq!(info.current_era_start.epoch, 5);

        let payload = decoder.u64().unwrap();
        assert_eq!(payload, 99);
    }

    /// A single-era telescope (era 0, e.g. Byron only) should work.
    #[test]
    fn test_unwrap_single_era_telescope() {
        let mut buf = Vec::new();
        let mut e = minicbor::Encoder::new(&mut buf);

        e.array(2).unwrap();

        // telescope: array(1) -- era 0 is current, no past eras
        e.array(1).unwrap();

        // current era: array(2) [Bound, payload]
        e.array(2).unwrap();
        encode_bound(&mut e, 0, 0);
        e.u64(7).unwrap(); // payload

        // HeaderState (dummy)
        e.u64(0).unwrap();

        let mut decoder = minicbor::Decoder::new(&buf);
        let info = unwrap_hfc_telescope(&mut decoder).unwrap();

        assert_eq!(info.era_index, 0);
        assert_eq!(info.past_era_bounds.len(), 0);
        assert_eq!(decoder.u64().unwrap(), 7);
    }

    /// Invalid ExtLedgerState length should produce a clear error.
    #[test]
    fn test_invalid_ext_ledger_state_length() {
        let mut buf = Vec::new();
        let mut e = minicbor::Encoder::new(&mut buf);
        e.array(3).unwrap();
        e.u64(0).unwrap();
        e.u64(0).unwrap();
        e.u64(0).unwrap();

        let mut decoder = minicbor::Decoder::new(&buf);
        let err = unwrap_hfc_telescope(&mut decoder).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("ExtLedgerState"), "error: {msg}");
        assert!(msg.contains("array(3)"), "error: {msg}");
    }
}
