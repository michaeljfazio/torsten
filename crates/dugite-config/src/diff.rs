//! Diff computation between the original and current config values.
//!
//! The diff view (`Ctrl+D`) shows only the parameters that have been changed
//! in the current editing session — the original (loaded) value on the left
//! and the new value on the right.
//!
//! # Data model
//!
//! [`DiffEntry`] pairs an original value with the current value for a single
//! key that has been modified.  [`compute_diff`] walks [`LoadedConfig`] and
//! collects all entries where `modified == true`, recording the original value
//! from the backup snapshot created at load time.
//!
//! Because the original value is not stored separately in [`ConfigEntry`] (the
//! entry is mutated in place by edits), the diff tracks it via a separate
//! [`OriginalValues`] snapshot that [`App`] captures at construction time.

use std::collections::HashMap;

use crate::config::ConfigEntry;

// ---------------------------------------------------------------------------
// Original-values snapshot
// ---------------------------------------------------------------------------

/// A map of key → original display value captured at config-load time.
///
/// This is built once when the [`App`] is constructed and never modified
/// again.  It is the ground truth for the "before" side of every diff.
#[derive(Debug, Default)]
pub struct OriginalValues(pub HashMap<String, String>);

impl OriginalValues {
    /// Build a snapshot from the loaded entries.
    pub fn from_entries(entries: &[ConfigEntry]) -> Self {
        let map = entries
            .iter()
            .map(|e| (e.key.clone(), e.display_value()))
            .collect();
        OriginalValues(map)
    }

    /// Return the original value for `key`, or `None` if not found.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }
}

// ---------------------------------------------------------------------------
// Diff entry
// ---------------------------------------------------------------------------

/// A single changed parameter in the diff view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffEntry {
    /// The JSON key.
    pub key: String,
    /// Value as it was when the file was loaded.
    pub original: String,
    /// Value after user edits.
    pub current: String,
}

// ---------------------------------------------------------------------------
// Compute diff
// ---------------------------------------------------------------------------

/// Collect all modified entries into a list of [`DiffEntry`]s.
///
/// Entries are returned in the same order as they appear in `entries`.
/// Only entries with `modified == true` are included.
pub fn compute_diff(entries: &[ConfigEntry], originals: &OriginalValues) -> Vec<DiffEntry> {
    entries
        .iter()
        .filter(|e| e.modified)
        .map(|e| {
            let original = originals.get(&e.key).unwrap_or("").to_string();
            DiffEntry {
                key: e.key.clone(),
                original,
                current: e.display_value(),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn make_entry(key: &str, value: Value, modified: bool) -> ConfigEntry {
        ConfigEntry {
            key: key.to_string(),
            value,
            modified,
        }
    }

    #[test]
    fn test_original_values_captures_at_load() {
        let entries = vec![
            make_entry("EnableP2P", Value::Bool(true), false),
            make_entry("NetworkMagic", Value::Number(2.into()), false),
        ];
        let snap = OriginalValues::from_entries(&entries);
        assert_eq!(snap.get("EnableP2P"), Some("true"));
        assert_eq!(snap.get("NetworkMagic"), Some("2"));
    }

    #[test]
    fn test_compute_diff_returns_only_modified() {
        let originals = {
            let entries = vec![
                make_entry("EnableP2P", Value::Bool(true), false),
                make_entry("MinSeverity", Value::String("Info".into()), false),
            ];
            OriginalValues::from_entries(&entries)
        };

        // Simulate in-memory edits.
        let current = vec![
            make_entry("EnableP2P", Value::Bool(false), true), // changed
            make_entry("MinSeverity", Value::String("Info".into()), false), // unchanged
        ];

        let diff = compute_diff(&current, &originals);
        assert_eq!(diff.len(), 1);
        assert_eq!(diff[0].key, "EnableP2P");
        assert_eq!(diff[0].original, "true");
        assert_eq!(diff[0].current, "false");
    }

    #[test]
    fn test_compute_diff_empty_when_no_changes() {
        let originals = {
            let entries = vec![make_entry("EnableP2P", Value::Bool(true), false)];
            OriginalValues::from_entries(&entries)
        };
        let current = vec![make_entry("EnableP2P", Value::Bool(true), false)];
        let diff = compute_diff(&current, &originals);
        assert!(diff.is_empty());
    }

    #[test]
    fn test_compute_diff_multiple_changes() {
        let originals = {
            let entries = vec![
                make_entry("EnableP2P", Value::Bool(true), false),
                make_entry("MinSeverity", Value::String("Info".into()), false),
                make_entry("NetworkMagic", Value::Number(2.into()), false),
            ];
            OriginalValues::from_entries(&entries)
        };

        let current = vec![
            make_entry("EnableP2P", Value::Bool(false), true),
            make_entry("MinSeverity", Value::String("Warning".into()), true),
            make_entry("NetworkMagic", Value::Number(2.into()), false), // not changed
        ];

        let diff = compute_diff(&current, &originals);
        assert_eq!(diff.len(), 2);
        assert_eq!(diff[0].key, "EnableP2P");
        assert_eq!(diff[1].key, "MinSeverity");
        assert_eq!(diff[1].original, "Info");
        assert_eq!(diff[1].current, "Warning");
    }

    #[test]
    fn test_diff_entry_equality() {
        let a = DiffEntry {
            key: "k".into(),
            original: "old".into(),
            current: "new".into(),
        };
        let b = a.clone();
        assert_eq!(a, b);
    }
}
