//! In-memory sorted write buffer backed by a BTreeMap.
//!
//! All writes (inserts and deletes) go to the memtable first. When the
//! approximate byte size exceeds the configured threshold, the memtable is
//! frozen and flushed to disk as a new sorted run (SSTable).
//!
//! Tombstones are represented as `None` values — they must survive until
//! compaction at the last level to ensure deleted keys are properly removed
//! from older runs.

use std::collections::BTreeMap;

use crate::key::Key;
use crate::value::Value;

/// Sorted in-memory write buffer.
pub struct MemTable {
    /// Sorted map of key → optional value (None = tombstone).
    entries: BTreeMap<Key, Option<Value>>,
    /// Approximate size in bytes of all keys and values.
    approx_bytes: usize,
}

impl MemTable {
    /// Create a new empty memtable.
    pub fn new() -> Self {
        MemTable {
            entries: BTreeMap::new(),
            approx_bytes: 0,
        }
    }

    /// Insert a key-value pair. Overwrites any existing entry for this key.
    pub fn insert(&mut self, key: Key, value: Value) {
        let new_value_len = value.len();
        let key_len = key.len();
        if let Some(old) = self.entries.insert(key, Some(value)) {
            // Key already existed — subtract old value size, add new value size
            let old_value_len = old.as_ref().map_or(0, |v| v.len());
            self.approx_bytes = self.approx_bytes.saturating_sub(old_value_len);
            self.approx_bytes += new_value_len;
        } else {
            // New key
            self.approx_bytes += key_len + new_value_len;
        }
    }

    /// Mark a key as deleted (tombstone).
    pub fn delete(&mut self, key: Key) {
        let key_len = key.len();
        if let Some(old) = self.entries.insert(key, None) {
            // Replace existing entry — subtract old value size
            let removed = old.as_ref().map_or(0, |v| v.len());
            self.approx_bytes = self.approx_bytes.saturating_sub(removed);
        } else {
            // New tombstone entry — only the key contributes to size
            self.approx_bytes += key_len;
        }
    }

    /// Look up a key. Returns `Some(Some(value))` for a live entry,
    /// `Some(None)` for a tombstone, or `None` if the key is not in the memtable.
    pub fn get(&self, key: &Key) -> Option<&Option<Value>> {
        self.entries.get(key)
    }

    /// Number of entries (including tombstones).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the memtable is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Approximate size in bytes of all stored data.
    pub fn approx_bytes(&self) -> usize {
        self.approx_bytes
    }

    /// Drain all entries from the memtable in sorted order.
    /// Resets the memtable to empty.
    pub fn drain(&mut self) -> Vec<(Key, Option<Value>)> {
        self.approx_bytes = 0;
        std::mem::take(&mut self.entries).into_iter().collect()
    }

    /// Iterate over entries in sorted key order. Includes tombstones.
    pub fn iter(&self) -> impl Iterator<Item = (&Key, &Option<Value>)> {
        self.entries.iter()
    }

    /// Iterate over entries in the given key range (inclusive start, exclusive end).
    /// Includes tombstones.
    pub fn range(&self, from: &Key, to: &Key) -> impl Iterator<Item = (&Key, &Option<Value>)> {
        use std::ops::Bound;
        self.entries
            .range((Bound::Included(from.clone()), Bound::Included(to.clone())))
    }
}

impl Default for MemTable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_and_get() {
        let mut mt = MemTable::new();
        let key = Key::from([1, 2, 3]);
        let val = Value::from([4, 5, 6]);
        mt.insert(key.clone(), val.clone());

        assert_eq!(mt.len(), 1);
        assert!(!mt.is_empty());

        let found = mt.get(&key).unwrap();
        assert_eq!(found.as_ref().unwrap().as_ref(), &[4, 5, 6]);
    }

    #[test]
    fn test_delete_tombstone() {
        let mut mt = MemTable::new();
        let key = Key::from([1, 2, 3]);
        let val = Value::from([4, 5, 6]);

        mt.insert(key.clone(), val);
        mt.delete(key.clone());

        let found = mt.get(&key).unwrap();
        assert!(found.is_none()); // tombstone
    }

    #[test]
    fn test_drain_sorted() {
        let mut mt = MemTable::new();
        mt.insert(Key::from([3]), Value::from([30]));
        mt.insert(Key::from([1]), Value::from([10]));
        mt.insert(Key::from([2]), Value::from([20]));

        let entries = mt.drain();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].0.as_ref(), &[1]);
        assert_eq!(entries[1].0.as_ref(), &[2]);
        assert_eq!(entries[2].0.as_ref(), &[3]);
        assert!(mt.is_empty());
    }

    #[test]
    fn test_overwrite() {
        let mut mt = MemTable::new();
        let key = Key::from([1]);
        mt.insert(key.clone(), Value::from([10]));
        mt.insert(key.clone(), Value::from([20]));

        assert_eq!(mt.len(), 1);
        let found = mt.get(&key).unwrap().as_ref().unwrap();
        assert_eq!(found.as_ref(), &[20]);
    }

    #[test]
    fn test_range() {
        let mut mt = MemTable::new();
        for i in 0u8..10 {
            mt.insert(Key::from([i]), Value::from([i * 10]));
        }

        let range: Vec<_> = mt.range(&Key::from([3]), &Key::from([6])).collect();
        assert_eq!(range.len(), 4); // 3, 4, 5, 6 (inclusive both ends)
    }

    #[test]
    fn test_approx_bytes() {
        let mut mt = MemTable::new();
        assert_eq!(mt.approx_bytes(), 0);

        mt.insert(Key::from([1, 2, 3]), Value::from([4, 5]));
        assert!(mt.approx_bytes() > 0);
    }
}
