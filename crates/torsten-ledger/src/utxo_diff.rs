//! Per-block UTxO diff tracking for rollback support.
//!
//! Matches Haskell's `DiffMK` — tracks inserts and deletes for each block,
//! enabling rollback by unapplying diffs rather than restoring full snapshots.

use std::collections::VecDeque;
use torsten_primitives::hash::Hash32;
use torsten_primitives::time::SlotNo;
use torsten_primitives::transaction::{TransactionInput, TransactionOutput};

/// Per-block UTxO diff: tracks new UTxOs created and spent UTxOs consumed.
///
/// The `deletes` field stores the original `TransactionOutput` value alongside
/// the input, so the diff can be unapplied (re-inserting spent outputs on rollback).
#[derive(Debug, Clone)]
pub struct UtxoDiff {
    /// New UTxOs created by this block
    pub inserts: Vec<(TransactionInput, TransactionOutput)>,
    /// Spent UTxOs consumed by this block (value preserved for undo)
    pub deletes: Vec<(TransactionInput, TransactionOutput)>,
}

impl UtxoDiff {
    pub fn new() -> Self {
        UtxoDiff {
            inserts: Vec::new(),
            deletes: Vec::new(),
        }
    }

    /// Record a new UTxO insert (output created).
    pub fn record_insert(&mut self, input: TransactionInput, output: TransactionOutput) {
        self.inserts.push((input, output));
    }

    /// Record a UTxO deletion (input spent), preserving the original output for rollback.
    pub fn record_delete(&mut self, input: TransactionInput, output: TransactionOutput) {
        self.deletes.push((input, output));
    }

    /// Whether this diff has no changes.
    pub fn is_empty(&self) -> bool {
        self.inserts.is_empty() && self.deletes.is_empty()
    }
}

impl Default for UtxoDiff {
    fn default() -> Self {
        Self::new()
    }
}

/// Sequence of per-block UTxO diffs for the last k blocks.
///
/// Used for rollback: to undo n blocks, pop n diffs from the back
/// and unapply them (delete the inserts, re-insert the deletes).
#[derive(Debug, Clone)]
pub struct DiffSeq {
    diffs: VecDeque<(SlotNo, Hash32, UtxoDiff)>,
}

impl DiffSeq {
    pub fn new() -> Self {
        DiffSeq {
            diffs: VecDeque::new(),
        }
    }

    /// Append a new block's diff.
    pub fn push(&mut self, slot: SlotNo, hash: Hash32, diff: UtxoDiff) {
        self.diffs.push_back((slot, hash, diff));
    }

    /// Remove the last n diffs (for rollback). Returns them in reverse order
    /// (most recent first) for unapplying.
    pub fn rollback(&mut self, n: usize) -> Vec<(SlotNo, Hash32, UtxoDiff)> {
        let n = n.min(self.diffs.len());
        let mut result = Vec::with_capacity(n);
        for _ in 0..n {
            if let Some(entry) = self.diffs.pop_back() {
                result.push(entry);
            }
        }
        result
    }

    /// Drain diffs up to (and including) the given slot.
    /// Used when flushing to immutable — these diffs are no longer needed
    /// since the UTxO store has already been updated.
    pub fn flush_up_to(&mut self, slot: SlotNo) -> Vec<UtxoDiff> {
        let mut flushed = Vec::new();
        while let Some((s, _, _)) = self.diffs.front() {
            if *s <= slot {
                let (_, _, diff) = self.diffs.pop_front().unwrap();
                flushed.push(diff);
            } else {
                break;
            }
        }
        flushed
    }

    /// Number of diffs stored.
    pub fn len(&self) -> usize {
        self.diffs.len()
    }

    /// Whether the sequence is empty.
    pub fn is_empty(&self) -> bool {
        self.diffs.is_empty()
    }

    /// Clear all diffs.
    pub fn clear(&mut self) {
        self.diffs.clear();
    }
}

impl Default for DiffSeq {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use torsten_primitives::address::Address;
    use torsten_primitives::hash::Hash32;
    use torsten_primitives::transaction::OutputDatum;
    use torsten_primitives::value::Value;

    fn make_input(hash_byte: u8, index: u32) -> TransactionInput {
        TransactionInput {
            transaction_id: Hash32::from_bytes([hash_byte; 32]),
            index,
        }
    }

    fn make_output(lovelace: u64) -> TransactionOutput {
        TransactionOutput {
            address: Address::Byron(torsten_primitives::address::ByronAddress {
                payload: vec![0u8; 32],
            }),
            value: Value::lovelace(lovelace),
            datum: OutputDatum::None,
            script_ref: None,
            raw_cbor: None,
        }
    }

    #[test]
    fn test_utxo_diff_basic() {
        let mut diff = UtxoDiff::new();
        assert!(diff.is_empty());

        diff.record_insert(make_input(1, 0), make_output(1_000_000));
        diff.record_delete(make_input(2, 0), make_output(2_000_000));

        assert!(!diff.is_empty());
        assert_eq!(diff.inserts.len(), 1);
        assert_eq!(diff.deletes.len(), 1);
    }

    #[test]
    fn test_diff_seq_push_and_rollback() {
        let mut seq = DiffSeq::new();

        for i in 0..5 {
            let mut diff = UtxoDiff::new();
            diff.record_insert(make_input(i, 0), make_output(i as u64 * 1_000_000));
            seq.push(SlotNo(i as u64 + 1), Hash32::from_bytes([i; 32]), diff);
        }
        assert_eq!(seq.len(), 5);

        // Rollback last 2
        let rolled_back = seq.rollback(2);
        assert_eq!(rolled_back.len(), 2);
        assert_eq!(seq.len(), 3);

        // Most recent first
        assert_eq!(rolled_back[0].0, SlotNo(5));
        assert_eq!(rolled_back[1].0, SlotNo(4));
    }

    #[test]
    fn test_diff_seq_flush_up_to() {
        let mut seq = DiffSeq::new();
        for i in 1..=5u64 {
            let diff = UtxoDiff::new();
            seq.push(SlotNo(i * 10), Hash32::from_bytes([i as u8; 32]), diff);
        }

        // Flush up to slot 30 (should drain slots 10, 20, 30)
        let flushed = seq.flush_up_to(SlotNo(30));
        assert_eq!(flushed.len(), 3);
        assert_eq!(seq.len(), 2);
    }

    #[test]
    fn test_diff_seq_rollback_more_than_available() {
        let mut seq = DiffSeq::new();
        seq.push(SlotNo(1), Hash32::ZERO, UtxoDiff::new());
        let rolled_back = seq.rollback(10);
        assert_eq!(rolled_back.len(), 1);
        assert!(seq.is_empty());
    }
}
