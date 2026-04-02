//! K-way merge iterator for combining sorted runs.
//!
//! Merges multiple sorted iterators into a single sorted stream, with
//! deduplication: when the same key appears in multiple runs, only the
//! newest value (from the run with the highest sequence number) is kept.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use crate::key::Key;
use crate::value::Value;

/// A sequence-numbered sorted entry list for merging.
pub type MergeInput = (usize, Vec<(Key, Option<Value>)>);
/// Collection of merge inputs.
pub type MergeInputs = Vec<MergeInput>;
/// Iterator type for merge input entries.
type EntryIter = (usize, std::vec::IntoIter<(Key, Option<Value>)>);

/// An entry in the merge heap. Lower keys and higher sequence numbers have priority.
struct HeapEntry {
    key: Key,
    value: Option<Value>,
    /// Sequence number: higher = newer. Used for deduplication — when two entries
    /// have the same key, the one with the higher sequence number wins.
    seq: usize,
}

impl Eq for HeapEntry {}

impl PartialEq for HeapEntry {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key && self.seq == other.seq
    }
}

impl Ord for HeapEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse key ordering (min-heap by key), then prefer higher sequence (newer)
        other
            .key
            .cmp(&self.key)
            .then_with(|| self.seq.cmp(&other.seq))
    }
}

impl PartialOrd for HeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Merge multiple sorted entry lists into a single sorted, deduplicated list.
///
/// Each input list is a `(sequence_number, entries)` pair. Higher sequence
/// numbers represent newer data. For duplicate keys, the newest value wins.
///
/// If `drop_tombstones` is true, tombstones (None values) are removed from
/// the output. This should only be true when merging at the last level.
pub fn merge_entries(inputs: Vec<MergeInput>, drop_tombstones: bool) -> Vec<(Key, Option<Value>)> {
    let total_entries: usize = inputs.iter().map(|(_, v)| v.len()).sum();
    let mut result = Vec::with_capacity(total_entries);

    // Build the heap with the first entry from each input
    let mut heap = BinaryHeap::new();
    let mut iterators: Vec<EntryIter> = inputs
        .into_iter()
        .map(|(seq, entries)| (seq, entries.into_iter()))
        .collect();

    // Seed the heap
    for (idx, (seq, iter)) in iterators.iter_mut().enumerate() {
        if let Some((key, value)) = iter.next() {
            heap.push(HeapEntry {
                key,
                value,
                seq: *seq * 1_000_000 + idx, // Combine seq with idx for stable ordering
            });
        }
    }

    let mut last_key: Option<Key> = None;

    while let Some(entry) = heap.pop() {
        // Find which iterator this came from (encoded in the low bits of seq)
        let iter_idx = entry.seq % 1_000_000;
        let base_seq = entry.seq - iter_idx;

        // Advance the source iterator
        if let Some((key, value)) = iterators[iter_idx].1.next() {
            heap.push(HeapEntry {
                key,
                value,
                seq: base_seq + iter_idx,
            });
        }

        // Dedup: skip if same key as last emitted
        if let Some(ref last) = last_key {
            if last == &entry.key {
                continue;
            }
        }

        last_key = Some(entry.key.clone());

        // Optionally drop tombstones
        if drop_tombstones && entry.value.is_none() {
            continue;
        }

        result.push((entry.key, entry.value));
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merge_two_runs() {
        let run1 = vec![
            (Key::from([1]), Some(Value::from([10]))),
            (Key::from([3]), Some(Value::from([30]))),
            (Key::from([5]), Some(Value::from([50]))),
        ];
        let run2 = vec![
            (Key::from([2]), Some(Value::from([20]))),
            (Key::from([3]), Some(Value::from([33]))), // newer value for key 3
            (Key::from([4]), Some(Value::from([40]))),
        ];

        let merged = merge_entries(vec![(0, run1), (1, run2)], false);

        assert_eq!(merged.len(), 5);
        assert_eq!(merged[0].0.as_ref(), &[1]);
        assert_eq!(merged[1].0.as_ref(), &[2]);
        // Key 3 should have newer value (33) from run2 (seq=1)
        assert_eq!(merged[2].0.as_ref(), &[3]);
        assert_eq!(merged[2].1.as_ref().unwrap().as_ref(), &[33]);
        assert_eq!(merged[3].0.as_ref(), &[4]);
        assert_eq!(merged[4].0.as_ref(), &[5]);
    }

    #[test]
    fn test_merge_with_tombstones() {
        let run1 = vec![
            (Key::from([1]), Some(Value::from([10]))),
            (Key::from([2]), Some(Value::from([20]))),
        ];
        let run2 = vec![
            (Key::from([1]), None), // tombstone for key 1 (newer)
        ];

        // Keep tombstones
        let merged = merge_entries(vec![(0, run1.clone()), (1, run2.clone())], false);
        assert_eq!(merged.len(), 2);
        assert!(merged[0].1.is_none()); // tombstone wins for key 1

        // Drop tombstones
        let merged = merge_entries(vec![(0, run1), (1, run2)], true);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].0.as_ref(), &[2]); // only key 2 survives
    }

    #[test]
    fn test_merge_empty() {
        let merged = merge_entries(vec![], false);
        assert!(merged.is_empty());
    }

    #[test]
    fn test_merge_single_run() {
        let run = vec![
            (Key::from([1]), Some(Value::from([10]))),
            (Key::from([2]), Some(Value::from([20]))),
        ];
        let merged = merge_entries(vec![(0, run)], false);
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn test_merge_preserves_sort() {
        let run1 = vec![
            (Key::from([1]), Some(Value::from([10]))),
            (Key::from([5]), Some(Value::from([50]))),
            (Key::from([9]), Some(Value::from([90]))),
        ];
        let run2 = vec![
            (Key::from([2]), Some(Value::from([20]))),
            (Key::from([6]), Some(Value::from([60]))),
        ];
        let run3 = vec![
            (Key::from([3]), Some(Value::from([30]))),
            (Key::from([7]), Some(Value::from([70]))),
        ];

        let merged = merge_entries(vec![(0, run1), (1, run2), (2, run3)], false);
        // Verify sorted
        for w in merged.windows(2) {
            assert!(w[0].0 < w[1].0);
        }
    }
}
