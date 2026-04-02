//! Config file I/O — load, save, and backup Cardano node configuration files.
//!
//! The Cardano node configuration format is a flat JSON object (no nested
//! sections) where every key is a top-level string and values are booleans,
//! integers, or strings.  This module reads the file into a
//! [`serde_json::Value`] and exposes a typed view used by the TUI.
//!
//! # Backup strategy
//!
//! Before every save, the original file is copied to `<path>.bak`.  Only one
//! level of backup is kept — the previous `.bak` is silently overwritten.
//!
//! # Pretty-print format
//!
//! Saved files use 4-space indentation and a trailing newline, matching the
//! format used by the official Cardano config files.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Flat key-value entry (the TUI's working unit)
// ---------------------------------------------------------------------------

/// A single key-value pair extracted from the top-level JSON object.
///
/// The TUI works exclusively with this type — it never manipulates the raw
/// JSON `Value` tree directly after the initial parse.
#[derive(Debug, Clone)]
pub struct ConfigEntry {
    /// The JSON key exactly as found in the file.
    pub key: String,
    /// Current value as a JSON `Value`.
    pub value: Value,
    /// Whether this entry has been modified since the file was loaded.
    pub modified: bool,
}

impl ConfigEntry {
    /// Return the current value formatted as a concise display string.
    ///
    /// - Booleans render as `true` / `false`.
    /// - Numbers render without quotes.
    /// - Strings render without surrounding quotes.
    /// - Anything else renders as compact JSON.
    pub fn display_value(&self) -> String {
        match &self.value {
            Value::Bool(b) => b.to_string(),
            Value::Number(n) => n.to_string(),
            Value::String(s) => s.clone(),
            other => other.to_string(),
        }
    }

    /// Apply a string edit to this entry's value, coercing to the appropriate
    /// JSON type based on the existing value type.
    ///
    /// - Existing bool: parses "true"/"false".
    /// - Existing number: tries integer parse then float.
    /// - Existing string: stores as-is.
    /// - Other: stores as a JSON string.
    ///
    /// Returns `Err` if the parse fails.
    pub fn apply_edit(&mut self, raw: &str) -> Result<()> {
        let new_value = match &self.value {
            Value::Bool(_) => raw
                .parse::<bool>()
                .map(Value::Bool)
                .with_context(|| format!("'{raw}' is not a valid boolean"))?,
            Value::Number(_) => {
                if let Ok(i) = raw.parse::<i64>() {
                    Value::Number(serde_json::Number::from(i))
                } else if let Ok(f) = raw.parse::<f64>() {
                    Value::Number(
                        serde_json::Number::from_f64(f)
                            .with_context(|| format!("'{raw}' is not finite"))?,
                    )
                } else {
                    anyhow::bail!("'{raw}' is not a valid number")
                }
            }
            Value::String(_) => Value::String(raw.to_string()),
            _ => Value::String(raw.to_string()),
        };
        self.value = new_value;
        self.modified = true;
        Ok(())
    }

    /// Toggle a boolean value in place.
    ///
    /// Returns `Err` if the current value is not a boolean.
    pub fn toggle_bool(&mut self) -> Result<()> {
        match &self.value {
            Value::Bool(b) => {
                self.value = Value::Bool(!b);
                self.modified = true;
                Ok(())
            }
            _ => anyhow::bail!("cannot toggle non-boolean value"),
        }
    }

    /// Cycle an enum value forward through the provided list of choices.
    ///
    /// If the current value is not in `choices`, it is set to `choices[0]`.
    pub fn cycle_enum(&mut self, choices: &[&str]) {
        if choices.is_empty() {
            return;
        }
        let current = self.display_value();
        let next = choices
            .iter()
            .position(|c| *c == current.as_str())
            .map(|i| choices[(i + 1) % choices.len()])
            .unwrap_or(choices[0]);
        self.value = Value::String(next.to_string());
        self.modified = true;
    }
}

// ---------------------------------------------------------------------------
// Loaded config
// ---------------------------------------------------------------------------

/// The full config file loaded into memory as an ordered list of entries.
///
/// Order is preserved from the original file so that save round-trips produce
/// minimal diffs.
#[derive(Debug)]
pub struct LoadedConfig {
    /// Absolute path of the file on disk.
    pub path: PathBuf,
    /// All key-value entries in file order.
    pub entries: Vec<ConfigEntry>,
}

impl LoadedConfig {
    /// Return `true` if any entry has been modified since load (or last save).
    pub fn is_modified(&self) -> bool {
        self.entries.iter().any(|e| e.modified)
    }

    /// Clear the `modified` flag on every entry.
    pub fn mark_clean(&mut self) {
        for entry in &mut self.entries {
            entry.modified = false;
        }
    }
}

// ---------------------------------------------------------------------------
// Load
// ---------------------------------------------------------------------------

/// Load a Cardano node configuration JSON file from `path`.
///
/// The file must be a JSON object (`{...}`) at its top level.  All top-level
/// keys are extracted in iteration order (which, for `serde_json`, is
/// insertion/file order when using the `preserve_order` feature — but
/// standard `serde_json::Map` also iterates alphabetically in the absence of
/// that feature; either way every key is captured).
pub fn load_config(path: &Path) -> Result<LoadedConfig> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("reading config file '{}'", path.display()))?;

    let json: Value = serde_json::from_str(&raw)
        .with_context(|| format!("parsing config file '{}' as JSON", path.display()))?;

    let obj = json.as_object().with_context(|| {
        format!(
            "config file '{}' must be a JSON object at the top level",
            path.display()
        )
    })?;

    let entries = obj
        .iter()
        .map(|(k, v)| ConfigEntry {
            key: k.clone(),
            value: v.clone(),
            modified: false,
        })
        .collect();

    Ok(LoadedConfig {
        path: path.to_path_buf(),
        entries,
    })
}

// ---------------------------------------------------------------------------
// Save
// ---------------------------------------------------------------------------

/// Save `config` back to its original file path.
///
/// Steps:
/// 1. Copy the current file to `<path>.bak` (overwriting any existing backup).
/// 2. Reconstruct a JSON object from the entry list (preserving order).
/// 3. Pretty-print with 4-space indent and a trailing newline.
/// 4. Write atomically via a temp file in the same directory then rename.
///
/// If the backup or write fails, the original file is left untouched.
pub fn save_config(config: &mut LoadedConfig) -> Result<()> {
    let path = config.path.clone();

    // Step 1 — backup.
    backup_file(&path)?;

    // Step 2 — rebuild JSON object in entry order.
    let mut obj = serde_json::Map::new();
    for entry in &config.entries {
        obj.insert(entry.key.clone(), entry.value.clone());
    }
    let json = Value::Object(obj);

    // Step 3 — pretty-print.
    let mut out = serde_json::to_string_pretty(&json).context("serialising config to JSON")?;
    out.push('\n'); // trailing newline

    // Step 4 — atomic write via temp file in the same directory.
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp_path = dir.join(".dugite-config.tmp");
    fs::write(&tmp_path, &out)
        .with_context(|| format!("writing temp file '{}'", tmp_path.display()))?;
    fs::rename(&tmp_path, &path)
        .with_context(|| format!("renaming temp file to '{}'", path.display()))?;

    // Mark all entries clean now that the file is on disk.
    config.mark_clean();

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Copy `path` to `<path>.bak`, silently overwriting any existing backup.
fn backup_file(path: &Path) -> Result<()> {
    // If the file does not exist yet there is nothing to back up.
    if !path.exists() {
        return Ok(());
    }
    let bak = PathBuf::from(format!("{}.bak", path.display()));
    fs::copy(path, &bak).with_context(|| format!("creating backup '{}'", bak.display()))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_temp(content: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f.flush().unwrap();
        f
    }

    #[test]
    fn test_load_simple_config() {
        let f = write_temp(r#"{"EnableP2P": true, "NetworkMagic": 2}"#);
        let config = load_config(f.path()).unwrap();
        assert_eq!(config.entries.len(), 2);
        assert_eq!(config.entries[0].key, "EnableP2P");
        assert_eq!(config.entries[0].value, Value::Bool(true));
        assert_eq!(config.entries[1].key, "NetworkMagic");
        assert_eq!(config.entries[1].value, Value::Number(2.into()));
    }

    #[test]
    fn test_load_rejects_non_object() {
        let f = write_temp(r#"[1, 2, 3]"#);
        assert!(load_config(f.path()).is_err());
    }

    #[test]
    fn test_load_rejects_invalid_json() {
        let f = write_temp(r#"{invalid"#);
        assert!(load_config(f.path()).is_err());
    }

    #[test]
    fn test_display_value_formats() {
        let mut entry = ConfigEntry {
            key: "k".into(),
            value: Value::Bool(true),
            modified: false,
        };
        assert_eq!(entry.display_value(), "true");

        entry.value = Value::Number(42.into());
        assert_eq!(entry.display_value(), "42");

        entry.value = Value::String("hello".into());
        assert_eq!(entry.display_value(), "hello");
    }

    #[test]
    fn test_apply_edit_bool() {
        let mut entry = ConfigEntry {
            key: "k".into(),
            value: Value::Bool(true),
            modified: false,
        };
        entry.apply_edit("false").unwrap();
        assert_eq!(entry.value, Value::Bool(false));
        assert!(entry.modified);
    }

    #[test]
    fn test_apply_edit_number() {
        let mut entry = ConfigEntry {
            key: "k".into(),
            value: Value::Number(1.into()),
            modified: false,
        };
        entry.apply_edit("99").unwrap();
        assert_eq!(entry.value, Value::Number(99.into()));
        assert!(entry.modified);
    }

    #[test]
    fn test_apply_edit_string() {
        let mut entry = ConfigEntry {
            key: "k".into(),
            value: Value::String("old".into()),
            modified: false,
        };
        entry.apply_edit("new").unwrap();
        assert_eq!(entry.value, Value::String("new".into()));
        assert!(entry.modified);
    }

    #[test]
    fn test_toggle_bool() {
        let mut entry = ConfigEntry {
            key: "k".into(),
            value: Value::Bool(false),
            modified: false,
        };
        entry.toggle_bool().unwrap();
        assert_eq!(entry.value, Value::Bool(true));
        assert!(entry.modified);
    }

    #[test]
    fn test_toggle_bool_on_non_bool_errors() {
        let mut entry = ConfigEntry {
            key: "k".into(),
            value: Value::Number(1.into()),
            modified: false,
        };
        assert!(entry.toggle_bool().is_err());
    }

    #[test]
    fn test_cycle_enum() {
        let choices = ["A", "B", "C"];
        let mut entry = ConfigEntry {
            key: "k".into(),
            value: Value::String("A".into()),
            modified: false,
        };
        entry.cycle_enum(&choices);
        assert_eq!(entry.display_value(), "B");
        entry.cycle_enum(&choices);
        assert_eq!(entry.display_value(), "C");
        entry.cycle_enum(&choices);
        assert_eq!(entry.display_value(), "A"); // wraps
    }

    #[test]
    fn test_is_modified_and_mark_clean() {
        let f = write_temp(r#"{"k": true}"#);
        let mut config = load_config(f.path()).unwrap();
        assert!(!config.is_modified());
        config.entries[0].modified = true;
        assert!(config.is_modified());
        config.mark_clean();
        assert!(!config.is_modified());
    }

    #[test]
    fn test_save_roundtrip() {
        let f = write_temp(r#"{"EnableP2P": true, "NetworkMagic": 2}"#);
        let path = f.path().to_path_buf();
        // Keep the NamedTempFile alive but we need to persist it.
        let persist = f.into_temp_path();

        let mut config = load_config(&path).unwrap();
        config.entries[0].toggle_bool().unwrap(); // EnableP2P -> false
        save_config(&mut config).unwrap();

        // Reload and verify.
        let config2 = load_config(&path).unwrap();
        assert_eq!(config2.entries[0].value, Value::Bool(false));
        assert_eq!(config2.entries[1].value, Value::Number(2.into()));
        assert!(!config2.is_modified());

        // Backup should exist.
        let bak = PathBuf::from(format!("{}.bak", path.display()));
        assert!(bak.exists());

        // Cleanup.
        let _ = std::fs::remove_file(&bak);
        drop(persist);
    }
}
