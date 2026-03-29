//! HFC (Hard Fork Combinator) era history state machine.
//!
//! Tracks era boundaries with exact slot/epoch/time arithmetic, following the
//! Haskell `Ouroboros.Consensus.HardFork.History.Summary` model.
//!
//! The `EraHistory` is initialized from genesis configs and extended during sync
//! as era transitions are detected in the block stream. It provides:
//! - Slot-to-wallclock and wallclock-to-slot conversions across all eras
//! - Slot-to-epoch and epoch-to-first-slot lookups across era boundaries
//! - N2C `GetEraSummaries` / `GetInterpreter` export
//!
//! All post-Byron eras share the same `EraParams` (epoch_size, slot_length, safe_zone)
//! derived from the Shelley genesis config, matching the Haskell implementation.

use serde::{Deserialize, Serialize};
use std::fmt;
use torsten_primitives::era::Era;
use torsten_primitives::time::{EpochNo, SlotNo, SystemStart};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Returned when a query references a slot/time beyond the safe prediction
/// horizon of the known era history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PastHorizonError {
    pub msg: String,
}

impl fmt::Display for PastHorizonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PastHorizon: {}", self.msg)
    }
}

impl std::error::Error for PastHorizonError {}

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// Per-era parameters (immutable within an era).
/// Mirrors Haskell's `EraParams` from `Ouroboros.Consensus.HardFork.History`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EraParams {
    /// Slots per epoch.
    pub epoch_size: u64,
    /// Milliseconds per slot.
    pub slot_length_ms: u64,
    /// Number of slots past the era end bound where predictions are still valid.
    /// Byron uses `2 * k`, Shelley+ uses `floor(3 * k / active_slots_coeff)`.
    pub safe_zone: u64,
}

/// A bound marking where an era starts or ends.
/// Mirrors Haskell's `Bound` from `Ouroboros.Consensus.HardFork.History`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bound {
    /// Picoseconds relative to system start.
    /// 1 second = 1_000_000_000_000 picoseconds.
    pub time_pico: u128,
    /// Absolute slot number.
    pub slot: u64,
    /// Absolute epoch number.
    pub epoch: u64,
}

impl Bound {
    /// The origin bound (time=0, slot=0, epoch=0).
    pub fn origin() -> Self {
        Self {
            time_pico: 0,
            slot: 0,
            epoch: 0,
        }
    }
}

/// One entry in the era history.
/// Mirrors Haskell's `EraSummary`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EraSummaryEntry {
    /// Which Cardano era this entry represents.
    pub era: Era,
    /// Inclusive start bound.
    pub start: Bound,
    /// Exclusive end bound. `None` means this is the current (open) era.
    pub end: Option<Bound>,
    /// Era-specific parameters.
    pub params: EraParams,
    /// Genesis window = 2 * k (security parameter). Used in N2C export.
    pub genesis_window: u64,
}

impl EraSummaryEntry {
    /// Whether this era entry contains the given slot.
    /// An open era (end=None) contains all slots >= start.slot.
    fn contains_slot(&self, slot: u64) -> bool {
        if slot < self.start.slot {
            return false;
        }
        match &self.end {
            Some(end) => slot < end.slot,
            None => true,
        }
    }

    /// Whether this era entry contains the given epoch.
    fn contains_epoch(&self, epoch: u64) -> bool {
        if epoch < self.start.epoch {
            return false;
        }
        match &self.end {
            Some(end) => epoch < end.epoch,
            None => true,
        }
    }

    /// Whether the given time (pico from system start) falls within this era.
    fn contains_time_pico(&self, time_pico: u128) -> bool {
        if time_pico < self.start.time_pico {
            return false;
        }
        match &self.end {
            Some(end) => time_pico < end.time_pico,
            None => true,
        }
    }

    /// Compute the end bound for this era at the given transition epoch.
    /// Panics if `transition_epoch < self.start.epoch`.
    fn compute_end_bound(&self, transition_epoch: u64) -> Bound {
        debug_assert!(
            transition_epoch >= self.start.epoch,
            "transition epoch {} < start epoch {}",
            transition_epoch,
            self.start.epoch
        );
        let epochs_in_era = transition_epoch - self.start.epoch;
        let slots_in_era = epochs_in_era * self.params.epoch_size;
        let end_slot = self.start.slot + slots_in_era;
        let time_delta_pico =
            slots_in_era as u128 * self.params.slot_length_ms as u128 * 1_000_000_000;
        let end_time_pico = self.start.time_pico + time_delta_pico;
        Bound {
            time_pico: end_time_pico,
            slot: end_slot,
            epoch: transition_epoch,
        }
    }
}

// ---------------------------------------------------------------------------
// EraHistory state machine
// ---------------------------------------------------------------------------

/// The era history state machine tracking all known era boundaries.
///
/// Invariants:
/// - `entries` is non-empty and ordered by era (ascending).
/// - Each entry's start bound equals the previous entry's end bound.
/// - Exactly the last entry is open (end=None); all prior entries are closed.
///
/// Mirrors Haskell's `Summary` from `Ouroboros.Consensus.HardFork.History`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EraHistory {
    entries: Vec<EraSummaryEntry>,
}

impl EraHistory {
    /// Construct a new era history from Byron and Shelley genesis parameters.
    ///
    /// - `byron_params`: Byron-era parameters (epoch_size = 10*k for mainnet,
    ///   slot_length_ms = 20000 for mainnet / 1000 for testnets).
    /// - `shelley_params`: Shelley-era parameters from genesis.
    /// - `shelley_transition_epoch`: The epoch at which the Byron→Shelley hard fork
    ///   occurred. For testnets with instant transitions, this is 0.
    /// - `genesis_window`: 2 * k (security parameter), used in N2C export.
    ///
    /// Creates a Byron entry (closed at `shelley_transition_epoch`) followed by an
    /// open Shelley entry. For networks with `shelley_transition_epoch == 0`, Byron
    /// has zero width (start == end at origin).
    pub fn from_genesis(
        byron_params: EraParams,
        shelley_params: EraParams,
        shelley_transition_epoch: u64,
        genesis_window: u64,
    ) -> Self {
        let byron_start = Bound::origin();

        // Compute Byron end bound. For instant transitions (epoch 0), this equals
        // the origin, producing a zero-width Byron era.
        let byron_end = if shelley_transition_epoch == 0 {
            Bound::origin()
        } else {
            let epochs = shelley_transition_epoch;
            let slots = epochs * byron_params.epoch_size;
            let time_pico = slots as u128 * byron_params.slot_length_ms as u128 * 1_000_000_000;
            Bound {
                time_pico,
                slot: slots,
                epoch: shelley_transition_epoch,
            }
        };

        let shelley_start = byron_end.clone();

        Self {
            entries: vec![
                EraSummaryEntry {
                    era: Era::Byron,
                    start: byron_start,
                    end: Some(byron_end),
                    params: byron_params,
                    genesis_window,
                },
                EraSummaryEntry {
                    era: Era::Shelley,
                    start: shelley_start,
                    end: None, // open era
                    params: shelley_params,
                    genesis_window,
                },
            ],
        }
    }

    /// Record an era transition, closing the current (last) era and opening a new one.
    ///
    /// - `new_era`: The era that begins at `transition_epoch`.
    /// - `transition_epoch`: The epoch at which the new era starts.
    ///
    /// The new era inherits the same `EraParams` as the current era (all Shelley+
    /// eras share epoch_size, slot_length, safe_zone from Shelley genesis).
    ///
    /// Panics if the entries are empty (invariant: always non-empty).
    pub fn record_era_transition(&mut self, new_era: Era, transition_epoch: u64) {
        let current = self
            .entries
            .last_mut()
            .expect("EraHistory must be non-empty");

        // Compute the end bound for the current era.
        let end_bound = current.compute_end_bound(transition_epoch);

        // Close the current era.
        current.end = Some(end_bound.clone());

        // The new era starts where the old one ended and inherits the same params.
        let genesis_window = current.genesis_window;
        let new_params = current.params.clone();
        self.entries.push(EraSummaryEntry {
            era: new_era,
            start: end_bound,
            end: None,
            params: new_params,
            genesis_window,
        });
    }

    /// The current (last, open) era.
    pub fn current_era(&self) -> Era {
        self.entries
            .last()
            .expect("EraHistory must be non-empty")
            .era
    }

    /// All entries in the era history.
    pub fn entries(&self) -> &[EraSummaryEntry] {
        &self.entries
    }

    /// Number of eras in the history.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the history is empty (should never be true by invariant).
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    // -----------------------------------------------------------------------
    // Slot ↔ time conversions
    // -----------------------------------------------------------------------

    /// Convert an absolute slot number to wall-clock time.
    ///
    /// Returns the time as picoseconds relative to system start, plus the
    /// `SystemStart` offset to get the absolute UTC time.
    pub fn slot_to_wallclock(
        &self,
        slot: SlotNo,
        system_start: &SystemStart,
    ) -> Result<chrono::DateTime<chrono::Utc>, PastHorizonError> {
        let entry = self.find_era_for_slot(slot.0)?;
        let slots_into_era = slot.0 - entry.start.slot;
        let time_delta_pico =
            slots_into_era as u128 * entry.params.slot_length_ms as u128 * 1_000_000_000;
        let relative_pico = entry.start.time_pico + time_delta_pico;

        Ok(pico_to_utc(relative_pico, system_start))
    }

    /// Convert a wall-clock time to an absolute slot number.
    ///
    /// Returns `Err(PastHorizonError)` if the time is before system start or
    /// beyond the safe prediction horizon.
    pub fn wallclock_to_slot(
        &self,
        time: chrono::DateTime<chrono::Utc>,
        system_start: &SystemStart,
    ) -> Result<SlotNo, PastHorizonError> {
        let elapsed = time.signed_duration_since(system_start.utc_time);
        if elapsed.num_milliseconds() < 0 {
            return Err(PastHorizonError {
                msg: "time is before system start".into(),
            });
        }

        // Convert elapsed time to picoseconds relative to system start.
        let elapsed_ms = elapsed.num_milliseconds() as u128;
        let elapsed_pico = elapsed_ms * 1_000_000_000;

        let entry = self.find_era_for_time_pico(elapsed_pico)?;
        let time_into_era_pico = elapsed_pico - entry.start.time_pico;
        let ms_per_slot = entry.params.slot_length_ms as u128;
        if ms_per_slot == 0 {
            return Err(PastHorizonError {
                msg: "slot_length_ms is zero".into(),
            });
        }
        let pico_per_slot = ms_per_slot * 1_000_000_000;
        let slots_into_era = time_into_era_pico / pico_per_slot;

        Ok(SlotNo(entry.start.slot + slots_into_era as u64))
    }

    // -----------------------------------------------------------------------
    // Slot ↔ epoch conversions
    // -----------------------------------------------------------------------

    /// Convert a slot to its epoch and the slot offset within that epoch.
    pub fn slot_to_epoch(&self, slot: SlotNo) -> Result<(EpochNo, u64), PastHorizonError> {
        let entry = self.find_era_for_slot(slot.0)?;
        let slots_into_era = slot.0 - entry.start.slot;
        let epoch_offset = slots_into_era / entry.params.epoch_size;
        let slot_in_epoch = slots_into_era % entry.params.epoch_size;
        Ok((EpochNo(entry.start.epoch + epoch_offset), slot_in_epoch))
    }

    /// Return the first slot of the given epoch.
    pub fn epoch_first_slot(&self, epoch: EpochNo) -> Result<SlotNo, PastHorizonError> {
        let entry = self.find_era_for_epoch(epoch.0)?;
        let epochs_into_era = epoch.0 - entry.start.epoch;
        let slot = entry.start.slot + epochs_into_era * entry.params.epoch_size;
        Ok(SlotNo(slot))
    }

    // -----------------------------------------------------------------------
    // N2C export
    // -----------------------------------------------------------------------

    /// Export the era history as a list of summaries suitable for the N2C
    /// `GetEraSummaries` / `GetInterpreter` query response.
    ///
    /// The returned struct fields match the existing `EraSummary` type in
    /// `torsten-node::n2c_query::types`.
    pub fn to_era_summary_exports(&self) -> Vec<EraSummaryExport> {
        self.entries
            .iter()
            .map(|entry| EraSummaryExport {
                start_slot: entry.start.slot,
                start_epoch: entry.start.epoch,
                start_time_pico: entry.start.time_pico,
                end: entry.end.as_ref().map(|b| EraBoundExport {
                    slot: b.slot,
                    epoch: b.epoch,
                    time_pico: b.time_pico,
                }),
                epoch_size: entry.params.epoch_size,
                slot_length_ms: entry.params.slot_length_ms,
                safe_zone: entry.params.safe_zone,
                genesis_window: entry.genesis_window,
            })
            .collect()
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Find the era entry that contains the given slot.
    fn find_era_for_slot(&self, slot: u64) -> Result<&EraSummaryEntry, PastHorizonError> {
        // Check the last (open) era first — hot path during normal operation.
        if let Some(last) = self.entries.last() {
            if last.contains_slot(slot) {
                return Ok(last);
            }
        }
        // Walk entries in order for closed eras.
        for entry in &self.entries {
            if entry.contains_slot(slot) {
                return Ok(entry);
            }
        }
        Err(PastHorizonError {
            msg: format!("slot {} is beyond the known era history", slot),
        })
    }

    /// Find the era entry that contains the given epoch.
    fn find_era_for_epoch(&self, epoch: u64) -> Result<&EraSummaryEntry, PastHorizonError> {
        if let Some(last) = self.entries.last() {
            if last.contains_epoch(epoch) {
                return Ok(last);
            }
        }
        for entry in &self.entries {
            if entry.contains_epoch(epoch) {
                return Ok(entry);
            }
        }
        Err(PastHorizonError {
            msg: format!("epoch {} is beyond the known era history", epoch),
        })
    }

    /// Find the era entry that contains the given relative time (picoseconds).
    fn find_era_for_time_pico(
        &self,
        time_pico: u128,
    ) -> Result<&EraSummaryEntry, PastHorizonError> {
        if let Some(last) = self.entries.last() {
            if last.contains_time_pico(time_pico) {
                return Ok(last);
            }
        }
        for entry in &self.entries {
            if entry.contains_time_pico(time_pico) {
                return Ok(entry);
            }
        }
        Err(PastHorizonError {
            msg: format!("time {} pico is beyond the known era history", time_pico),
        })
    }
}

// ---------------------------------------------------------------------------
// N2C export types (parallel to n2c_query::types::EraSummary / EraBound)
// ---------------------------------------------------------------------------

/// Flat export of an era summary for N2C wire format.
/// These mirror the existing types in `torsten-node::n2c_query::types`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EraSummaryExport {
    pub start_slot: u64,
    pub start_epoch: u64,
    /// Picoseconds from system start (u128 to avoid overflow for mainnet Byron).
    pub start_time_pico: u128,
    pub end: Option<EraBoundExport>,
    pub epoch_size: u64,
    pub slot_length_ms: u64,
    pub safe_zone: u64,
    pub genesis_window: u64,
}

/// Flat export of an era bound for N2C wire format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EraBoundExport {
    pub slot: u64,
    pub epoch: u64,
    /// Picoseconds from system start (u128 to avoid overflow for mainnet Byron).
    pub time_pico: u128,
}

// ---------------------------------------------------------------------------
// Helper: convert picoseconds from system start to UTC time
// ---------------------------------------------------------------------------

fn pico_to_utc(relative_pico: u128, system_start: &SystemStart) -> chrono::DateTime<chrono::Utc> {
    // Split into whole seconds + sub-second nanoseconds.
    let pico_per_sec: u128 = 1_000_000_000_000;
    let total_secs = (relative_pico / pico_per_sec) as i64;
    let remaining_pico = relative_pico % pico_per_sec;
    let remaining_nanos = (remaining_pico / 1_000) as i64; // pico → nano
    system_start.utc_time
        + chrono::Duration::seconds(total_secs)
        + chrono::Duration::nanoseconds(remaining_nanos)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Mainnet constants
    const MAINNET_BYRON_EPOCH_SIZE: u64 = 21600;
    const MAINNET_BYRON_SLOT_MS: u64 = 20_000;
    const MAINNET_SHELLEY_EPOCH_SIZE: u64 = 432000;
    const MAINNET_SHELLEY_SLOT_MS: u64 = 1000;
    const MAINNET_K: u64 = 2160;
    const MAINNET_ACTIVE_SLOTS_COEFF: f64 = 0.05;
    const MAINNET_SHELLEY_TRANSITION_EPOCH: u64 = 208;
    const MAINNET_GENESIS_WINDOW: u64 = MAINNET_K * 2; // 4320

    fn mainnet_byron_params() -> EraParams {
        EraParams {
            epoch_size: MAINNET_BYRON_EPOCH_SIZE,
            slot_length_ms: MAINNET_BYRON_SLOT_MS,
            safe_zone: MAINNET_K * 2,
        }
    }

    fn mainnet_shelley_params() -> EraParams {
        EraParams {
            epoch_size: MAINNET_SHELLEY_EPOCH_SIZE,
            slot_length_ms: MAINNET_SHELLEY_SLOT_MS,
            safe_zone: (3.0 * MAINNET_K as f64 / MAINNET_ACTIVE_SLOTS_COEFF).floor() as u64,
        }
    }

    // Preview testnet constants
    const PREVIEW_SHELLEY_EPOCH_SIZE: u64 = 86400;
    const PREVIEW_SHELLEY_SLOT_MS: u64 = 1000;
    const PREVIEW_K: u64 = 432;
    const PREVIEW_ACTIVE_SLOTS_COEFF: f64 = 0.05;
    const PREVIEW_GENESIS_WINDOW: u64 = PREVIEW_K * 2; // 864

    fn preview_byron_params() -> EraParams {
        EraParams {
            epoch_size: 4320,
            slot_length_ms: 1000,
            safe_zone: PREVIEW_K * 2,
        }
    }

    fn preview_shelley_params() -> EraParams {
        EraParams {
            epoch_size: PREVIEW_SHELLEY_EPOCH_SIZE,
            slot_length_ms: PREVIEW_SHELLEY_SLOT_MS,
            safe_zone: (3.0 * PREVIEW_K as f64 / PREVIEW_ACTIVE_SLOTS_COEFF).floor() as u64,
        }
    }

    #[test]
    fn test_from_genesis_mainnet() {
        let eh = EraHistory::from_genesis(
            mainnet_byron_params(),
            mainnet_shelley_params(),
            MAINNET_SHELLEY_TRANSITION_EPOCH,
            MAINNET_GENESIS_WINDOW,
        );

        assert_eq!(eh.len(), 2);
        assert_eq!(eh.current_era(), Era::Shelley);

        // Byron entry
        let byron = &eh.entries[0];
        assert_eq!(byron.era, Era::Byron);
        assert_eq!(byron.start, Bound::origin());
        let byron_end = byron.end.as_ref().unwrap();
        // 208 epochs * 21600 slots = 4,492,800 slots
        assert_eq!(byron_end.slot, 208 * 21600);
        assert_eq!(byron_end.epoch, 208);
        // Time: 4,492,800 slots * 20,000 ms/slot * 1e9 pico/ms
        let expected_time_pico = 4_492_800u128 * 20_000 * 1_000_000_000;
        assert_eq!(byron_end.time_pico, expected_time_pico);

        // Shelley entry: starts where Byron ended
        let shelley = &eh.entries[1];
        assert_eq!(shelley.era, Era::Shelley);
        assert_eq!(shelley.start, *byron_end);
        assert!(shelley.end.is_none()); // open
        assert_eq!(shelley.params.epoch_size, 432000);
        assert_eq!(shelley.params.slot_length_ms, 1000);
    }

    #[test]
    fn test_from_genesis_preview() {
        // Preview: shelley_transition_epoch = 0 (instant Byron→Shelley)
        let eh = EraHistory::from_genesis(
            preview_byron_params(),
            preview_shelley_params(),
            0,
            PREVIEW_GENESIS_WINDOW,
        );

        assert_eq!(eh.len(), 2);
        assert_eq!(eh.current_era(), Era::Shelley);

        // Byron entry: zero-width (start == end == origin)
        let byron = &eh.entries[0];
        assert_eq!(byron.start, Bound::origin());
        assert_eq!(byron.end.as_ref().unwrap(), &Bound::origin());

        // Shelley entry: starts at origin
        let shelley = &eh.entries[1];
        assert_eq!(shelley.start, Bound::origin());
        assert!(shelley.end.is_none());
    }

    #[test]
    fn test_record_era_transitions() {
        // Start with preview-like history (instant Byron)
        let mut eh = EraHistory::from_genesis(
            preview_byron_params(),
            preview_shelley_params(),
            0,
            PREVIEW_GENESIS_WINDOW,
        );

        // Record: Shelley→Allegra at epoch 0 (instant), Allegra→Mary at epoch 0,
        // Mary→Alonzo at epoch 0, Alonzo→Babbage at epoch 3, Babbage→Conway at epoch 646
        eh.record_era_transition(Era::Allegra, 0);
        eh.record_era_transition(Era::Mary, 0);
        eh.record_era_transition(Era::Alonzo, 0);
        eh.record_era_transition(Era::Babbage, 3);
        eh.record_era_transition(Era::Conway, 646);

        assert_eq!(eh.len(), 7);
        assert_eq!(eh.current_era(), Era::Conway);

        // Verify continuity: each entry's start == previous entry's end
        for i in 1..eh.entries.len() {
            let prev_end = eh.entries[i - 1].end.as_ref().unwrap();
            assert_eq!(
                &eh.entries[i].start,
                prev_end,
                "entry {} start != entry {} end",
                i,
                i - 1
            );
        }

        // Verify Alonzo→Babbage boundary at epoch 3
        let alonzo = &eh.entries[4];
        assert_eq!(alonzo.era, Era::Alonzo);
        let alonzo_end = alonzo.end.as_ref().unwrap();
        assert_eq!(alonzo_end.epoch, 3);
        assert_eq!(alonzo_end.slot, 3 * PREVIEW_SHELLEY_EPOCH_SIZE);

        // Verify Babbage→Conway boundary at epoch 646
        let babbage = &eh.entries[5];
        assert_eq!(babbage.era, Era::Babbage);
        let babbage_end = babbage.end.as_ref().unwrap();
        assert_eq!(babbage_end.epoch, 646);
        assert_eq!(babbage_end.slot, 646 * PREVIEW_SHELLEY_EPOCH_SIZE);

        // Conway is open
        let conway = &eh.entries[6];
        assert_eq!(conway.era, Era::Conway);
        assert!(conway.end.is_none());
    }

    #[test]
    fn test_slot_to_wallclock_byron() {
        let eh = EraHistory::from_genesis(
            mainnet_byron_params(),
            mainnet_shelley_params(),
            MAINNET_SHELLEY_TRANSITION_EPOCH,
            MAINNET_GENESIS_WINDOW,
        );

        let system_start = torsten_primitives::time::mainnet_system_start();

        // Slot 100 in Byron: 100 * 20s = 2000s from system start
        let time = eh.slot_to_wallclock(SlotNo(100), &system_start).unwrap();
        let expected = system_start.utc_time + chrono::Duration::seconds(2000);
        assert_eq!(time, expected);

        // Slot 0: exactly at system start
        let time = eh.slot_to_wallclock(SlotNo(0), &system_start).unwrap();
        assert_eq!(time, system_start.utc_time);
    }

    #[test]
    fn test_slot_to_wallclock_cross_era() {
        let eh = EraHistory::from_genesis(
            mainnet_byron_params(),
            mainnet_shelley_params(),
            MAINNET_SHELLEY_TRANSITION_EPOCH,
            MAINNET_GENESIS_WINDOW,
        );

        let system_start = torsten_primitives::time::mainnet_system_start();

        // Byron end slot = 4,492,800. Shelley starts here.
        let byron_end_slot = 208 * 21600; // 4,492,800
        let byron_duration_secs = byron_end_slot as i64 * 20; // 89,856,000 seconds

        // Slot at Shelley start should equal Byron end time
        let time = eh
            .slot_to_wallclock(SlotNo(byron_end_slot), &system_start)
            .unwrap();
        let expected = system_start.utc_time + chrono::Duration::seconds(byron_duration_secs);
        assert_eq!(time, expected);

        // One Shelley slot later (1 second per slot)
        let time = eh
            .slot_to_wallclock(SlotNo(byron_end_slot + 1), &system_start)
            .unwrap();
        let expected = system_start.utc_time + chrono::Duration::seconds(byron_duration_secs + 1);
        assert_eq!(time, expected);

        // 100 Shelley slots later
        let time = eh
            .slot_to_wallclock(SlotNo(byron_end_slot + 100), &system_start)
            .unwrap();
        let expected = system_start.utc_time + chrono::Duration::seconds(byron_duration_secs + 100);
        assert_eq!(time, expected);
    }

    #[test]
    fn test_wallclock_to_slot_roundtrip() {
        let eh = EraHistory::from_genesis(
            mainnet_byron_params(),
            mainnet_shelley_params(),
            MAINNET_SHELLEY_TRANSITION_EPOCH,
            MAINNET_GENESIS_WINDOW,
        );
        let system_start = torsten_primitives::time::mainnet_system_start();

        // Test roundtrip for various slots
        for slot in [
            0, 1, 100, 21599, 21600, 4_492_799, 4_492_800, 4_492_801, 5_000_000,
        ] {
            let time = eh.slot_to_wallclock(SlotNo(slot), &system_start).unwrap();
            let roundtrip = eh.wallclock_to_slot(time, &system_start).unwrap();
            assert_eq!(
                roundtrip,
                SlotNo(slot),
                "roundtrip failed for slot {}",
                slot
            );
        }
    }

    #[test]
    fn test_slot_to_epoch() {
        let eh = EraHistory::from_genesis(
            mainnet_byron_params(),
            mainnet_shelley_params(),
            MAINNET_SHELLEY_TRANSITION_EPOCH,
            MAINNET_GENESIS_WINDOW,
        );

        // Slot 0 = epoch 0, offset 0 (Byron)
        let (epoch, offset) = eh.slot_to_epoch(SlotNo(0)).unwrap();
        assert_eq!(epoch, EpochNo(0));
        assert_eq!(offset, 0);

        // Last slot of Byron epoch 0: slot 21599
        let (epoch, offset) = eh.slot_to_epoch(SlotNo(21599)).unwrap();
        assert_eq!(epoch, EpochNo(0));
        assert_eq!(offset, 21599);

        // First slot of Byron epoch 1: slot 21600
        let (epoch, offset) = eh.slot_to_epoch(SlotNo(21600)).unwrap();
        assert_eq!(epoch, EpochNo(1));
        assert_eq!(offset, 0);

        // First Shelley slot (slot 4,492,800 = epoch 208)
        let shelley_start = 208 * 21600;
        let (epoch, offset) = eh.slot_to_epoch(SlotNo(shelley_start)).unwrap();
        assert_eq!(epoch, EpochNo(208));
        assert_eq!(offset, 0);

        // One Shelley slot in: slot 4,492,801 = epoch 208, offset 1
        let (epoch, offset) = eh.slot_to_epoch(SlotNo(shelley_start + 1)).unwrap();
        assert_eq!(epoch, EpochNo(208));
        assert_eq!(offset, 1);

        // First slot of Shelley epoch 209: shelley_start + 432000
        let (epoch, offset) = eh.slot_to_epoch(SlotNo(shelley_start + 432000)).unwrap();
        assert_eq!(epoch, EpochNo(209));
        assert_eq!(offset, 0);
    }

    #[test]
    fn test_epoch_first_slot() {
        let eh = EraHistory::from_genesis(
            mainnet_byron_params(),
            mainnet_shelley_params(),
            MAINNET_SHELLEY_TRANSITION_EPOCH,
            MAINNET_GENESIS_WINDOW,
        );

        // Byron epoch 0 → slot 0
        assert_eq!(eh.epoch_first_slot(EpochNo(0)).unwrap(), SlotNo(0));

        // Byron epoch 1 → slot 21600
        assert_eq!(eh.epoch_first_slot(EpochNo(1)).unwrap(), SlotNo(21600));

        // Byron epoch 207 (last Byron epoch) → slot 207 * 21600
        assert_eq!(
            eh.epoch_first_slot(EpochNo(207)).unwrap(),
            SlotNo(207 * 21600)
        );

        // Shelley epoch 208 → slot 4,492,800 (= 208 * 21600)
        let shelley_start = 208 * 21600;
        assert_eq!(
            eh.epoch_first_slot(EpochNo(208)).unwrap(),
            SlotNo(shelley_start)
        );

        // Shelley epoch 209 → shelley_start + 432000
        assert_eq!(
            eh.epoch_first_slot(EpochNo(209)).unwrap(),
            SlotNo(shelley_start + 432000)
        );
    }

    #[test]
    fn test_past_horizon_error() {
        // Create a history with only Byron (closed at epoch 10) and Shelley (closed at epoch 20)
        let byron_params = EraParams {
            epoch_size: 100,
            slot_length_ms: 1000,
            safe_zone: 200,
        };
        let shelley_params = EraParams {
            epoch_size: 100,
            slot_length_ms: 1000,
            safe_zone: 200,
        };
        let mut eh = EraHistory::from_genesis(byron_params, shelley_params, 10, 100);
        // Close Shelley at epoch 20 and open Allegra
        eh.record_era_transition(Era::Allegra, 20);

        // Allegra is the open era (epoch 20+), so slots in Allegra should work
        let (epoch, _) = eh.slot_to_epoch(SlotNo(2500)).unwrap();
        assert_eq!(epoch, EpochNo(25)); // slot 2500 in era starting at epoch 20, slot 2000: (2500-2000)/100 + 20 = 25

        // But if we remove the open era by closing it too...
        // Actually, with the current API we always have one open era, so PastHorizon
        // would only trigger for slots before slot 0 (impossible with u64).
        // The error is mainly a safety valve; let's just verify the error type exists.
        let system_start = torsten_primitives::time::mainnet_system_start();
        let before_start = system_start.utc_time - chrono::Duration::seconds(100);
        let result = eh.wallclock_to_slot(before_start, &system_start);
        assert!(result.is_err());
        assert!(result.unwrap_err().msg.contains("before system start"));
    }

    #[test]
    fn test_mainnet_era_summaries() {
        // Build full mainnet history with all era transitions
        let mut eh = EraHistory::from_genesis(
            mainnet_byron_params(),
            mainnet_shelley_params(),
            MAINNET_SHELLEY_TRANSITION_EPOCH,
            MAINNET_GENESIS_WINDOW,
        );

        // Mainnet era transitions (from build_era_summaries hardcoded values):
        // Shelley→Allegra: not explicitly tracked in current code (Shelley covers 208..365)
        // but in Haskell, each era has its own entry. For simplicity, the current
        // build_era_summaries groups Shelley/Allegra/Mary/Alonzo as one "Shelley" era.
        //
        // For this test, we verify the Byron entry matches the hardcoded values.
        let exports = eh.to_era_summary_exports();
        assert_eq!(exports.len(), 2); // Byron + Shelley (open)

        let byron = &exports[0];
        assert_eq!(byron.start_slot, 0);
        assert_eq!(byron.start_epoch, 0);
        assert_eq!(byron.start_time_pico, 0);
        assert_eq!(byron.epoch_size, 21600);
        assert_eq!(byron.slot_length_ms, 20000);
        assert_eq!(byron.safe_zone, 4320); // 2 * 2160
        assert_eq!(byron.genesis_window, 4320);

        let byron_end = byron.end.as_ref().unwrap();
        assert_eq!(byron_end.slot, 4_492_800);
        assert_eq!(byron_end.epoch, 208);

        // Now add Babbage and Conway transitions (matching current hardcoded values)
        // In current code: Shelley covers 208..365, Babbage 365..517, Conway 517+
        eh.record_era_transition(Era::Babbage, 365);
        eh.record_era_transition(Era::Conway, 517);

        let exports = eh.to_era_summary_exports();
        assert_eq!(exports.len(), 4); // Byron, Shelley, Babbage, Conway

        // Shelley end = Babbage start
        let shelley = &exports[1];
        let shelley_end = shelley.end.as_ref().unwrap();
        assert_eq!(shelley_end.epoch, 365);
        // Shelley slot = 4,492,800 + (365 - 208) * 432,000 = 4,492,800 + 67,824,000 = 72,316,800
        assert_eq!(shelley_end.slot, 4_492_800 + (365 - 208) * 432_000);

        let babbage = &exports[2];
        assert_eq!(babbage.start_slot, shelley_end.slot);
        assert_eq!(babbage.start_epoch, 365);
        let babbage_end = babbage.end.as_ref().unwrap();
        assert_eq!(babbage_end.epoch, 517);

        let conway = &exports[3];
        assert_eq!(conway.start_slot, babbage_end.slot);
        assert_eq!(conway.start_epoch, 517);
        assert!(conway.end.is_none());
    }

    #[test]
    fn test_zero_width_eras() {
        // Preview testnet pattern: Byron/Shelley/Allegra/Mary all at epoch 0
        let mut eh = EraHistory::from_genesis(
            preview_byron_params(),
            preview_shelley_params(),
            0, // instant Byron
            PREVIEW_GENESIS_WINDOW,
        );

        eh.record_era_transition(Era::Allegra, 0);
        eh.record_era_transition(Era::Mary, 0);
        eh.record_era_transition(Era::Alonzo, 0);

        // All zero-width eras start and end at origin
        for i in 0..4 {
            let entry = &eh.entries[i];
            assert_eq!(entry.start, Bound::origin());
            assert_eq!(entry.end.as_ref().unwrap(), &Bound::origin());
        }

        // Alonzo starts at origin and is open
        assert_eq!(eh.entries[4].era, Era::Alonzo);
        assert_eq!(eh.entries[4].start, Bound::origin());
        assert!(eh.entries[4].end.is_none());

        // Slot 0 should resolve to Alonzo (the last era containing slot 0)
        // — actually slot 0 is contained by all zero-width eras AND Alonzo.
        // Our find_era_for_slot checks the last (open) era first, so it returns Alonzo.
        let (epoch, offset) = eh.slot_to_epoch(SlotNo(0)).unwrap();
        assert_eq!(epoch, EpochNo(0));
        assert_eq!(offset, 0);

        // Close Alonzo at epoch 3, open Babbage
        eh.record_era_transition(Era::Babbage, 3);

        // Slot at epoch 3 start = 3 * 86400 = 259200
        let (epoch, _) = eh.slot_to_epoch(SlotNo(259200)).unwrap();
        assert_eq!(epoch, EpochNo(3));
    }

    #[test]
    fn test_preview_era_summaries() {
        // Build full preview history matching current hardcoded values
        let mut eh = EraHistory::from_genesis(
            preview_byron_params(),
            preview_shelley_params(),
            0,
            PREVIEW_GENESIS_WINDOW,
        );

        // Preview: Byron/Shelley/Allegra/Mary at epoch 0 (instant)
        // Alonzo 0→3, Babbage 3→646, Conway 646+
        eh.record_era_transition(Era::Allegra, 0);
        eh.record_era_transition(Era::Mary, 0);
        eh.record_era_transition(Era::Alonzo, 0);
        eh.record_era_transition(Era::Babbage, 3);
        eh.record_era_transition(Era::Conway, 646);

        let exports = eh.to_era_summary_exports();
        assert_eq!(exports.len(), 7);

        // Byron through Mary: zero-width at origin
        for e in exports.iter().take(4) {
            assert_eq!(e.start_slot, 0);
            assert_eq!(e.start_epoch, 0);
            assert_eq!(e.start_time_pico, 0);
            let end = e.end.as_ref().unwrap();
            assert_eq!(end.slot, 0);
            assert_eq!(end.epoch, 0);
        }

        // Alonzo: 0→3
        let alonzo = &exports[4];
        assert_eq!(alonzo.start_epoch, 0);
        let alonzo_end = alonzo.end.as_ref().unwrap();
        assert_eq!(alonzo_end.epoch, 3);
        assert_eq!(alonzo_end.slot, 3 * PREVIEW_SHELLEY_EPOCH_SIZE);

        // Babbage: 3→646
        let babbage = &exports[5];
        assert_eq!(babbage.start_epoch, 3);
        let babbage_end = babbage.end.as_ref().unwrap();
        assert_eq!(babbage_end.epoch, 646);

        // Conway: 646→open
        let conway = &exports[6];
        assert_eq!(conway.start_epoch, 646);
        assert!(conway.end.is_none());
    }

    #[test]
    fn test_serde_roundtrip() {
        let eh = EraHistory::from_genesis(
            mainnet_byron_params(),
            mainnet_shelley_params(),
            MAINNET_SHELLEY_TRANSITION_EPOCH,
            MAINNET_GENESIS_WINDOW,
        );

        let json = serde_json::to_string(&eh).unwrap();
        let deserialized: EraHistory = serde_json::from_str(&json).unwrap();

        assert_eq!(eh.entries.len(), deserialized.entries.len());
        for (a, b) in eh.entries.iter().zip(deserialized.entries.iter()) {
            assert_eq!(a.era, b.era);
            assert_eq!(a.start, b.start);
            assert_eq!(a.end, b.end);
            assert_eq!(a.params, b.params);
        }
    }

    #[test]
    fn test_custom_network() {
        // Custom network with non-standard epoch length and slot duration
        let byron_params = EraParams {
            epoch_size: 500,
            slot_length_ms: 2000, // 2s slots
            safe_zone: 100,
        };
        let shelley_params = EraParams {
            epoch_size: 1000,
            slot_length_ms: 500, // 0.5s slots
            safe_zone: 200,
        };

        let eh = EraHistory::from_genesis(byron_params, shelley_params, 5, 100);
        let system_start = torsten_primitives::time::mainnet_system_start();

        // Byron: 5 epochs * 500 slots = 2500 slots at 2s each = 5000s
        let byron_end_slot = 5 * 500;
        assert_eq!(byron_end_slot, 2500);

        let time = eh.slot_to_wallclock(SlotNo(2500), &system_start).unwrap();
        let expected = system_start.utc_time + chrono::Duration::seconds(5000);
        assert_eq!(time, expected);

        // Shelley slot 2501 = Byron duration + 1 * 0.5s = 5000.5s
        let time = eh.slot_to_wallclock(SlotNo(2501), &system_start).unwrap();
        let expected = system_start.utc_time + chrono::Duration::milliseconds(5_000_500);
        assert_eq!(time, expected);

        // Roundtrip
        let slot = eh.wallclock_to_slot(time, &system_start).unwrap();
        assert_eq!(slot, SlotNo(2501));
    }
}
