//! Core LsmTree implementation wiring all components together.
//!
//! Orchestrates: memtable → WAL → flush → runs → compaction → snapshots.
//!
//! ## Thread safety
//!
//! - The public API uses `&self` for reads and `&mut self` for writes.
//! - Internal `BlockCache` uses `parking_lot::Mutex` for safe interior mutation
//!   during reads (cache inserts on cache miss).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use parking_lot::Mutex;

use crate::cache::BlockCache;
use crate::compaction::{compact_level, find_compaction_level, CompactParams};
use crate::config::LsmConfig;
use crate::error::{Error, Result};
use crate::key::Key;
use crate::level::Level;
use crate::memtable::MemTable;
use crate::run::Run;
use crate::session_lock::SessionLock;
use crate::snapshot::{self, SnapshotMetadata};
use crate::value::Value;
use crate::wal::{self, WalOp, WalWriter};

/// Manifest file name.
const MANIFEST_FILE: &str = "MANIFEST";

/// An LSM-tree database.
///
/// Provides a key-value store with:
/// - O(1) writes (append to memtable + WAL)
/// - O(log N) reads (memtable + bloom/fence/page lookup)
/// - Crash recovery via WAL
/// - Persistent snapshots via hard links
/// - Bounded write amplification via lazy levelling compaction
pub struct LsmTree {
    /// Database root directory.
    db_path: PathBuf,
    /// Active runs directory.
    active_dir: PathBuf,
    /// Configuration.
    config: LsmConfig,
    /// In-memory sorted write buffer.
    memtable: MemTable,
    /// Write-ahead log writer.
    wal: WalWriter,
    /// On-disk sorted runs, keyed by run ID.
    runs: HashMap<u64, Run>,
    /// Level structure (which runs at which level).
    levels: Vec<Level>,
    /// Next run ID to allocate.
    next_run_id: u64,
    /// Block cache for SSTable pages (interior mutability for reads).
    cache: Mutex<BlockCache>,
    /// Exclusive session lock.
    _lock: SessionLock,
}

impl LsmTree {
    /// Open or create an LSM tree database at the given path.
    pub fn open(path: impl AsRef<Path>, config: LsmConfig) -> Result<Self> {
        let db_path = path.as_ref().to_path_buf();
        fs::create_dir_all(&db_path)?;

        let active_dir = db_path.join("active");
        fs::create_dir_all(&active_dir)?;

        let wal_dir = db_path.join("wal");

        // Acquire exclusive lock
        let lock = SessionLock::acquire(&db_path)?;

        // Load manifest or start fresh
        let (mut levels, next_run_id, runs) =
            load_manifest(&db_path, &active_dir, config.page_size)?;

        // Replay WAL to recover memtable
        let mut memtable = MemTable::new();
        let wal_ops = wal::replay_wal(&wal_dir)?;
        for op in wal_ops {
            match op {
                WalOp::Insert(key, value) => memtable.insert(key, value),
                WalOp::Delete(key) => memtable.delete(key),
            }
        }

        // Start WAL writer
        let wal = WalWriter::new(&wal_dir, config.wal_segment_size, config.wal_enabled)?;

        // Ensure at least one level exists
        if levels.is_empty() {
            levels.push(Level::new(0));
        }

        let cache = Mutex::new(BlockCache::new(config.cache_capacity()));

        Ok(LsmTree {
            db_path,
            active_dir,
            config,
            memtable,
            wal,
            runs,
            levels,
            next_run_id,
            cache,
            _lock: lock,
        })
    }

    /// Open an LSM tree from a named snapshot.
    ///
    /// The snapshot's runs are restored to the active directory, and the tree
    /// is opened with the state captured at snapshot time.
    pub fn open_snapshot(path: impl AsRef<Path>, name: &str) -> Result<Self> {
        let db_path = path.as_ref().to_path_buf();
        fs::create_dir_all(&db_path)?;

        let active_dir = db_path.join("active");
        let snapshots_dir = snapshot::snapshot_dir(&db_path);

        // Acquire lock
        let lock = SessionLock::acquire(&db_path)?;

        // Clean active directory (we're replacing it with snapshot state)
        if active_dir.exists() {
            fs::remove_dir_all(&active_dir)?;
        }
        fs::create_dir_all(&active_dir)?;

        // Open snapshot
        let metadata = snapshot::open_snapshot(&snapshots_dir, &active_dir, name)?;

        // Reconstruct levels and runs
        let config = LsmConfig::default();
        let mut levels: Vec<Level> = Vec::new();
        let mut runs = HashMap::new();

        for &(level_num, run_id) in &metadata.runs {
            while levels.len() <= level_num {
                levels.push(Level::new(levels.len()));
            }
            levels[level_num].add_run(run_id);

            let run = Run::open(&active_dir, run_id, config.page_size)?;
            runs.insert(run_id, run);
        }

        if levels.is_empty() {
            levels.push(Level::new(0));
        }

        let wal_dir = db_path.join("wal");
        let wal = WalWriter::new(&wal_dir, config.wal_segment_size, config.wal_enabled)?;
        let cache = Mutex::new(BlockCache::new(config.cache_capacity()));

        Ok(LsmTree {
            db_path,
            active_dir,
            config,
            memtable: MemTable::new(),
            wal,
            runs,
            levels,
            next_run_id: metadata.next_run_id,
            cache,
            _lock: lock,
        })
    }

    /// Look up a key. Returns `Some(value)` if found, `None` if not found.
    ///
    /// Search order: memtable → levels (newest to oldest). The first match
    /// wins. A tombstone in the memtable or a newer run shadows older values.
    pub fn get(&self, key: &Key) -> Result<Option<Value>> {
        // Check memtable first
        if let Some(entry) = self.memtable.get(key) {
            return Ok(entry.clone()); // None = tombstone (key deleted)
        }

        // Check runs from newest to oldest (higher levels first within each level,
        // but actually we check from level 0 upward, newest run first within each level)
        let mut cache = self.cache.lock();
        for level in &self.levels {
            // Within a level, check runs from newest (last) to oldest (first)
            for &run_id in level.run_ids.iter().rev() {
                if let Some(run) = self.runs.get(&run_id) {
                    match run.get_with_cache_insert(key, &mut cache)? {
                        Some(Some(value)) => return Ok(Some(value)),
                        Some(None) => return Ok(None), // tombstone
                        None => continue,
                    }
                }
            }
        }

        Ok(None)
    }

    /// Insert a key-value pair.
    ///
    /// The write is logged to the WAL first, then applied to the memtable.
    /// If the memtable exceeds the configured size, it is flushed to disk.
    pub fn insert(&mut self, key: &Key, value: &Value) -> Result<()> {
        // WAL first (crash safety)
        self.wal.log_insert(key, value)?;

        // Apply to memtable
        self.memtable.insert(key.clone(), value.clone());

        // Check if memtable needs flushing
        if self.memtable.approx_bytes() >= self.config.memtable_size {
            self.flush_memtable()?;
        }

        Ok(())
    }

    /// Delete a key (write a tombstone).
    ///
    /// The tombstone is logged to the WAL and applied to the memtable.
    /// Tombstones are cleaned up during compaction at the last level.
    pub fn delete(&mut self, key: &Key) -> Result<()> {
        self.wal.log_delete(key)?;
        self.memtable.delete(key.clone());

        if self.memtable.approx_bytes() >= self.config.memtable_size {
            self.flush_memtable()?;
        }

        Ok(())
    }

    /// Iterate over entries in the key range [from, to] (inclusive both ends).
    ///
    /// Merges results from the memtable and all runs, deduplicating by key
    /// (newest value wins). Tombstones are filtered out.
    ///
    /// Returns a collected Vec because the merged result requires processing
    /// all sources. For the UTxO use case, ranges are rare (full scans at
    /// epoch boundaries) so this is acceptable.
    pub fn range(&self, from: &Key, to: &Key) -> RangeIter {
        // Collect entries from memtable
        let mut all_entries: crate::merge::MergeInputs = Vec::new();

        // Memtable has highest sequence number
        let mem_entries: Vec<(Key, Option<Value>)> = self
            .memtable
            .range(from, to)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        let mut seq = 0usize;
        // Add run entries from oldest to newest (lower seq = older)
        for level in self.levels.iter().rev() {
            for &run_id in &level.run_ids {
                if let Some(run) = self.runs.get(&run_id) {
                    if let Ok(entries) = run.scan_range(from, to) {
                        all_entries.push((seq, entries));
                        seq += 1;
                    }
                }
            }
        }

        // Memtable gets highest sequence number (newest)
        if !mem_entries.is_empty() {
            all_entries.push((seq, mem_entries));
        }

        // Merge all entries, keeping newest values and filtering tombstones
        let merged = crate::merge::merge_entries(all_entries, true);

        // Convert to (Key, Value) pairs (tombstones already filtered)
        let result: Vec<(Key, Value)> = merged
            .into_iter()
            .filter_map(|(k, v)| v.map(|val| (k, val)))
            .collect();

        RangeIter {
            inner: result.into_iter(),
        }
    }

    /// Save a persistent snapshot of the current tree state.
    pub fn save_snapshot(&mut self, name: &str, label: &str) -> Result<()> {
        // Flush memtable first to ensure all data is on disk
        if !self.memtable.is_empty() {
            self.flush_memtable()?;
        }

        let snapshots_dir = snapshot::snapshot_dir(&self.db_path);

        // Build metadata
        let mut runs_meta = Vec::new();
        for level in &self.levels {
            for &run_id in &level.run_ids {
                runs_meta.push((level.number, run_id));
            }
        }

        let metadata = SnapshotMetadata {
            label: label.to_string(),
            runs: runs_meta,
            next_run_id: self.next_run_id,
        };

        snapshot::save_snapshot(&self.active_dir, &snapshots_dir, name, label, &metadata)
    }

    /// Delete a named snapshot.
    pub fn delete_snapshot(&self, name: &str) -> Result<()> {
        let snapshots_dir = snapshot::snapshot_dir(&self.db_path);
        snapshot::delete_snapshot(&snapshots_dir, name)
    }

    /// Flush the memtable to disk as a new sorted run.
    pub fn flush(&mut self) -> Result<()> {
        if !self.memtable.is_empty() {
            self.flush_memtable()?;
        }
        Ok(())
    }

    /// Internal: flush the memtable to a new run and trigger compaction if needed.
    fn flush_memtable(&mut self) -> Result<()> {
        let entries = self.memtable.drain();
        if entries.is_empty() {
            return Ok(());
        }

        // Write a new run
        let run_id = self.next_run_id;
        self.next_run_id += 1;

        let run = Run::write(
            &self.active_dir,
            run_id,
            &entries,
            self.config.page_size,
            self.config.bloom_filter_bits_per_key,
        )?;

        // Add to level 0
        self.levels[0].add_run(run_id);
        self.runs.insert(run_id, run);

        // Clear the WAL (memtable data is now safely on disk)
        self.wal.clear()?;

        // Save manifest
        self.save_manifest()?;

        // Check for compaction
        self.maybe_compact()?;

        Ok(())
    }

    /// Run compaction if any level exceeds its threshold.
    fn maybe_compact(&mut self) -> Result<()> {
        // Keep compacting until no level exceeds its threshold
        while let Some(level_idx) = find_compaction_level(&self.levels, self.config.size_ratio) {
            let mut cache = self.cache.lock();
            let mut params = CompactParams {
                active_dir: &self.active_dir,
                levels: &mut self.levels,
                runs: &mut self.runs,
                next_run_id: &mut self.next_run_id,
                page_size: self.config.page_size,
                bloom_bits_per_key: self.config.bloom_filter_bits_per_key,
                cache: &mut cache,
            };
            compact_level(&mut params, level_idx)?;
            drop(cache);

            // Save manifest after each compaction
            self.save_manifest()?;
        }
        Ok(())
    }

    /// Save the manifest (level structure + run IDs + next_run_id).
    fn save_manifest(&self) -> Result<()> {
        let metadata = SnapshotMetadata {
            label: "manifest".to_string(),
            runs: self
                .levels
                .iter()
                .flat_map(|l| l.run_ids.iter().map(move |&id| (l.number, id)))
                .collect(),
            next_run_id: self.next_run_id,
        };

        let bytes = metadata.to_bytes()?;
        let tmp_path = self.db_path.join(format!("{MANIFEST_FILE}.tmp"));
        let manifest_path = self.db_path.join(MANIFEST_FILE);

        fs::write(&tmp_path, &bytes)?;
        fs::rename(&tmp_path, &manifest_path)?;

        Ok(())
    }
}

/// Load the manifest and reconstruct levels/runs, or return empty state if no manifest.
fn load_manifest(
    db_path: &Path,
    active_dir: &Path,
    page_size: usize,
) -> Result<(Vec<Level>, u64, HashMap<u64, Run>)> {
    let manifest_path = db_path.join(MANIFEST_FILE);

    if !manifest_path.exists() {
        return Ok((Vec::new(), 0, HashMap::new()));
    }

    let bytes = fs::read(&manifest_path)?;
    let metadata = SnapshotMetadata::from_bytes(&bytes)
        .map_err(|e| Error::Manifest(format!("failed to load manifest: {e}")))?;

    let mut levels: Vec<Level> = Vec::new();
    let mut runs = HashMap::new();

    for &(level_num, run_id) in &metadata.runs {
        while levels.len() <= level_num {
            levels.push(Level::new(levels.len()));
        }
        levels[level_num].add_run(run_id);

        // Try to open the run — if files are missing, skip it (corrupt state)
        match Run::open(active_dir, run_id, page_size) {
            Ok(run) => {
                runs.insert(run_id, run);
            }
            Err(e) => {
                // Run files missing — remove from level and continue
                eprintln!("Warning: failed to open run {run_id}: {e}");
                if let Some(level) = levels.get_mut(level_num) {
                    level.remove_run(run_id);
                }
            }
        }
    }

    Ok((levels, metadata.next_run_id, runs))
}

/// Iterator over a range of key-value pairs.
pub struct RangeIter {
    inner: std::vec::IntoIter<(Key, Value)>,
}

impl Iterator for RangeIter {
    type Item = (Key, Value);

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl ExactSizeIterator for RangeIter {}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_tree() -> (tempfile::TempDir, LsmTree) {
        let dir = tempfile::tempdir().unwrap();
        let config = LsmConfig {
            memtable_size: 4096, // Small for testing flush behavior
            wal_enabled: true,
            ..LsmConfig::default()
        };
        let tree = LsmTree::open(dir.path(), config).unwrap();
        (dir, tree)
    }

    #[test]
    fn test_basic_insert_get() {
        let (_dir, mut tree) = temp_tree();

        tree.insert(&Key::from([1, 2, 3]), &Value::from([10, 20, 30]))
            .unwrap();

        let result = tree.get(&Key::from([1, 2, 3])).unwrap();
        assert_eq!(result.unwrap().as_ref(), &[10, 20, 30]);

        // Key not found
        assert!(tree.get(&Key::from([4, 5, 6])).unwrap().is_none());
    }

    #[test]
    fn test_delete() {
        let (_dir, mut tree) = temp_tree();

        tree.insert(&Key::from([1]), &Value::from([10])).unwrap();
        assert!(tree.get(&Key::from([1])).unwrap().is_some());

        tree.delete(&Key::from([1])).unwrap();
        assert!(tree.get(&Key::from([1])).unwrap().is_none());
    }

    #[test]
    fn test_overwrite() {
        let (_dir, mut tree) = temp_tree();

        tree.insert(&Key::from([1]), &Value::from([10])).unwrap();
        tree.insert(&Key::from([1]), &Value::from([20])).unwrap();

        let result = tree.get(&Key::from([1])).unwrap().unwrap();
        assert_eq!(result.as_ref(), &[20]);
    }

    #[test]
    fn test_range_scan() {
        let (_dir, mut tree) = temp_tree();

        for i in 0u8..10 {
            tree.insert(&Key::from([i]), &Value::from([i * 10]))
                .unwrap();
        }

        let results: Vec<_> = tree.range(&Key::from([3]), &Key::from([7])).collect();
        assert_eq!(results.len(), 5);
        assert_eq!(results[0].0.as_ref(), &[3]);
        assert_eq!(results[4].0.as_ref(), &[7]);
    }

    #[test]
    fn test_flush_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let config = LsmConfig {
            memtable_size: 1024, // Very small to force flushes
            wal_enabled: true,
            ..LsmConfig::default()
        };
        let mut tree = LsmTree::open(dir.path(), config).unwrap();

        // Insert enough data to trigger a flush
        for i in 0u16..100 {
            let key = Key::from(i.to_be_bytes());
            let value = Value::from(vec![i as u8; 50]);
            tree.insert(&key, &value).unwrap();
        }

        // All entries should still be readable
        for i in 0u16..100 {
            let key = Key::from(i.to_be_bytes());
            let result = tree.get(&key).unwrap();
            assert!(result.is_some(), "key {i} not found");
        }
    }

    #[test]
    fn test_wal_crash_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().to_path_buf();

        // Write some data and close without flushing
        {
            let config = LsmConfig {
                memtable_size: 1024 * 1024, // Large memtable (won't flush)
                wal_enabled: true,
                ..LsmConfig::default()
            };
            let mut tree = LsmTree::open(&db_path, config).unwrap();
            tree.insert(&Key::from([1, 2, 3]), &Value::from([10, 20, 30]))
                .unwrap();
            tree.insert(&Key::from([4, 5, 6]), &Value::from([40, 50, 60]))
                .unwrap();
            // Drop without flush — simulates crash
        }

        // Reopen — WAL should recover the data
        {
            let config = LsmConfig {
                memtable_size: 1024 * 1024,
                wal_enabled: true,
                ..LsmConfig::default()
            };
            let tree = LsmTree::open(&db_path, config).unwrap();
            let v1 = tree.get(&Key::from([1, 2, 3])).unwrap();
            assert_eq!(v1.unwrap().as_ref(), &[10, 20, 30]);
            let v2 = tree.get(&Key::from([4, 5, 6])).unwrap();
            assert_eq!(v2.unwrap().as_ref(), &[40, 50, 60]);
        }
    }

    #[test]
    fn test_snapshot_save_and_restore() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().to_path_buf();

        // Write data and save snapshot
        {
            let config = LsmConfig::default();
            let mut tree = LsmTree::open(&db_path, config).unwrap();
            tree.insert(&Key::from([1]), &Value::from([10])).unwrap();
            tree.insert(&Key::from([2]), &Value::from([20])).unwrap();
            tree.save_snapshot("snap1", "test").unwrap();
        }

        // Reopen from snapshot (in a new directory to avoid lock conflict)
        let dir2 = tempfile::tempdir().unwrap();
        let db_path2 = dir2.path().to_path_buf();
        fs::create_dir_all(&db_path2).unwrap();

        // Copy snapshots directory
        let src_snaps = snapshot::snapshot_dir(&db_path);
        let dst_snaps = snapshot::snapshot_dir(&db_path2);
        copy_dir_recursive(&src_snaps, &dst_snaps).unwrap();

        {
            let tree = LsmTree::open_snapshot(&db_path2, "snap1").unwrap();
            let v1 = tree.get(&Key::from([1])).unwrap();
            assert_eq!(v1.unwrap().as_ref(), &[10]);
            let v2 = tree.get(&Key::from([2])).unwrap();
            assert_eq!(v2.unwrap().as_ref(), &[20]);
        }
    }

    #[test]
    fn test_reopen_with_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().to_path_buf();

        // Write data, flush, close
        {
            let config = LsmConfig {
                memtable_size: 512, // Small to force flush
                wal_enabled: true,
                ..LsmConfig::default()
            };
            let mut tree = LsmTree::open(&db_path, config).unwrap();
            for i in 0u16..50 {
                tree.insert(&Key::from(i.to_be_bytes()), &Value::from(vec![i as u8; 30]))
                    .unwrap();
            }
            tree.flush().unwrap();
        }

        // Reopen and verify data persists
        {
            let config = LsmConfig::default();
            let tree = LsmTree::open(&db_path, config).unwrap();
            for i in 0u16..50 {
                let result = tree.get(&Key::from(i.to_be_bytes())).unwrap();
                assert!(result.is_some(), "key {i} not found after reopen");
            }
        }
    }

    #[test]
    fn test_delete_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let config = LsmConfig::default();
        let mut tree = LsmTree::open(dir.path(), config).unwrap();
        tree.insert(&Key::from([1]), &Value::from([10])).unwrap();
        tree.save_snapshot("snap1", "test").unwrap();

        tree.delete_snapshot("snap1").unwrap();

        // Deleting again should succeed (idempotent)
        tree.delete_snapshot("snap1").unwrap();
    }

    #[test]
    fn test_large_values() {
        let (_dir, mut tree) = temp_tree();

        // Insert a large value (close to page size limit)
        let large_value = Value::from(vec![42u8; 3000]);
        tree.insert(&Key::from([1]), &large_value).unwrap();

        let result = tree.get(&Key::from([1])).unwrap().unwrap();
        assert_eq!(result.len(), 3000);
        assert_eq!(result.as_ref()[0], 42);
    }

    #[test]
    fn test_empty_range() {
        let (_dir, mut tree) = temp_tree();
        tree.insert(&Key::from([5]), &Value::from([50])).unwrap();

        // Range with no matching keys
        let results: Vec<_> = tree.range(&Key::from([10]), &Key::from([20])).collect();
        assert!(results.is_empty());
    }

    #[test]
    fn test_compaction_triggered() {
        let dir = tempfile::tempdir().unwrap();
        let config = LsmConfig {
            memtable_size: 256, // Very small — forces frequent flushes
            size_ratio: 2,      // Compact at 2 runs per level
            wal_enabled: false, // Skip WAL for speed
            ..LsmConfig::default()
        };
        let mut tree = LsmTree::open(dir.path(), config).unwrap();

        // Insert enough data to trigger multiple flushes and compaction
        for i in 0u16..200 {
            let key = Key::from(i.to_be_bytes());
            let value = Value::from(vec![i as u8; 20]);
            tree.insert(&key, &value).unwrap();
        }

        // All data should still be readable after compaction
        for i in 0u16..200 {
            let key = Key::from(i.to_be_bytes());
            let result = tree.get(&key).unwrap();
            assert!(result.is_some(), "key {i} not found after compaction");
        }
    }

    /// Recursive directory copy helper for tests.
    fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
        if !src.exists() {
            return Ok(());
        }
        fs::create_dir_all(dst)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            let src_path = entry.path();
            let dst_path = dst.join(entry.file_name());
            if src_path.is_dir() {
                copy_dir_recursive(&src_path, &dst_path)?;
            } else {
                fs::copy(&src_path, &dst_path)?;
            }
        }
        Ok(())
    }

    #[test]
    fn test_stress_mixed_operations() {
        // Stress test: interleave inserts, deletes, overwrites, reads
        let dir = tempfile::tempdir().unwrap();
        let config = LsmConfig {
            memtable_size: 512,
            size_ratio: 2,
            wal_enabled: true,
            ..LsmConfig::default()
        };
        let mut tree = LsmTree::open(dir.path(), config).unwrap();

        let mut expected: std::collections::HashMap<Vec<u8>, Vec<u8>> =
            std::collections::HashMap::new();

        // Phase 1: insert 500 keys
        for i in 0u16..500 {
            let key_bytes = i.to_be_bytes().to_vec();
            let val_bytes = vec![i as u8; 20];
            tree.insert(
                &Key::from(key_bytes.clone()),
                &Value::from(val_bytes.clone()),
            )
            .unwrap();
            expected.insert(key_bytes, val_bytes);
        }

        // Phase 2: overwrite every 3rd key
        for i in (0u16..500).step_by(3) {
            let key_bytes = i.to_be_bytes().to_vec();
            let val_bytes = vec![0xFF; 20];
            tree.insert(
                &Key::from(key_bytes.clone()),
                &Value::from(val_bytes.clone()),
            )
            .unwrap();
            expected.insert(key_bytes, val_bytes);
        }

        // Phase 3: delete every 5th key
        for i in (0u16..500).step_by(5) {
            let key_bytes = i.to_be_bytes().to_vec();
            tree.delete(&Key::from(key_bytes.clone())).unwrap();
            expected.remove(&key_bytes);
        }

        // Verify all expected keys
        for (key_bytes, val_bytes) in &expected {
            let result = tree.get(&Key::from(key_bytes.clone())).unwrap();
            assert!(result.is_some(), "key {:?} not found", key_bytes);
            assert_eq!(result.unwrap().as_ref(), val_bytes.as_slice());
        }

        // Verify deleted keys are gone
        for i in (0u16..500).step_by(5) {
            let key_bytes = i.to_be_bytes().to_vec();
            if !expected.contains_key(&key_bytes) {
                let result = tree.get(&Key::from(key_bytes)).unwrap();
                assert!(result.is_none());
            }
        }
    }

    #[test]
    fn test_range_across_memtable_and_disk() {
        // Ensure range scan correctly merges memtable and on-disk data
        let dir = tempfile::tempdir().unwrap();
        let config = LsmConfig {
            memtable_size: 256,
            wal_enabled: false,
            ..LsmConfig::default()
        };
        let mut tree = LsmTree::open(dir.path(), config).unwrap();

        // Insert enough to force some flushes
        for i in 0u16..100 {
            let key = Key::from(i.to_be_bytes());
            let value = Value::from(vec![i as u8; 20]);
            tree.insert(&key, &value).unwrap();
        }

        // Full range scan
        let start = Key::from(0u16.to_be_bytes());
        let end = Key::from(99u16.to_be_bytes());
        let results: Vec<_> = tree.range(&start, &end).collect();

        // Should have all 100 entries, sorted
        assert_eq!(results.len(), 100);
        for (i, (key, _)) in results.iter().enumerate() {
            let expected_key = Key::from((i as u16).to_be_bytes());
            assert_eq!(key, &expected_key);
        }
    }

    #[test]
    fn test_concurrent_read_during_flush() {
        // Verify that reads work correctly while flushes happen
        let dir = tempfile::tempdir().unwrap();
        let config = LsmConfig {
            memtable_size: 128, // Very small to cause many flushes
            wal_enabled: false,
            ..LsmConfig::default()
        };
        let mut tree = LsmTree::open(dir.path(), config).unwrap();

        for i in 0u16..300 {
            let key = Key::from(i.to_be_bytes());
            let value = Value::from(vec![i as u8; 10]);
            tree.insert(&key, &value).unwrap();

            // Read back a random earlier key
            if i > 0 {
                let check_key = Key::from((i / 2).to_be_bytes());
                let result = tree.get(&check_key).unwrap();
                assert!(
                    result.is_some(),
                    "key {} not found after inserting key {}",
                    i / 2,
                    i
                );
            }
        }
    }
}

/// Property-based tests using proptest.
#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    /// Strategy for generating LSM keys (1-64 bytes).
    fn arb_key() -> impl Strategy<Value = Key> {
        prop::collection::vec(any::<u8>(), 1..64).prop_map(Key::new)
    }

    /// Strategy for generating LSM values (1-256 bytes).
    fn arb_value() -> impl Strategy<Value = Value> {
        prop::collection::vec(any::<u8>(), 1..256).prop_map(Value::new)
    }

    /// Strategy for generating insert/delete operations.
    #[derive(Debug, Clone)]
    enum Op {
        Insert(Key, Value),
        Delete(Key),
    }

    fn arb_op() -> impl Strategy<Value = Op> {
        prop_oneof![
            3 => (arb_key(), arb_value()).prop_map(|(k, v)| Op::Insert(k, v)),
            1 => arb_key().prop_map(Op::Delete),
        ]
    }

    proptest! {
        /// Property: every inserted key can be read back unless subsequently deleted.
        #[test]
        fn prop_insert_then_get(
            entries in prop::collection::vec((arb_key(), arb_value()), 1..50)
        ) {
            let dir = tempfile::tempdir().unwrap();
            let config = LsmConfig {
                memtable_size: 256,
                wal_enabled: false,
                ..LsmConfig::default()
            };
            let mut tree = LsmTree::open(dir.path(), config).unwrap();

            for (key, value) in &entries {
                tree.insert(key, value).unwrap();
            }

            // The last write for each key should be readable
            let mut expected: std::collections::HashMap<Vec<u8>, Vec<u8>> =
                std::collections::HashMap::new();
            for (key, value) in &entries {
                expected.insert(key.as_bytes().to_vec(), value.as_bytes().to_vec());
            }

            for (key_bytes, val_bytes) in &expected {
                let result = tree.get(&Key::new(key_bytes.clone())).unwrap();
                prop_assert!(result.is_some());
                let got = result.unwrap();
                prop_assert_eq!(got.as_bytes(), val_bytes.as_slice());
            }
        }

        /// Property: deleting a key makes it unreadable.
        #[test]
        fn prop_delete_removes_key(
            key in arb_key(),
            value in arb_value()
        ) {
            let dir = tempfile::tempdir().unwrap();
            let config = LsmConfig {
                memtable_size: 4096,
                wal_enabled: false,
                ..LsmConfig::default()
            };
            let mut tree = LsmTree::open(dir.path(), config).unwrap();

            tree.insert(&key, &value).unwrap();
            prop_assert!(tree.get(&key).unwrap().is_some());

            tree.delete(&key).unwrap();
            prop_assert!(tree.get(&key).unwrap().is_none());
        }

        /// Property: range scan returns sorted, deduplicated results.
        #[test]
        fn prop_range_sorted(
            entries in prop::collection::vec((arb_key(), arb_value()), 1..30)
        ) {
            let dir = tempfile::tempdir().unwrap();
            let config = LsmConfig {
                memtable_size: 256,
                wal_enabled: false,
                ..LsmConfig::default()
            };
            let mut tree = LsmTree::open(dir.path(), config).unwrap();

            for (key, value) in &entries {
                tree.insert(key, value).unwrap();
            }

            let start = Key::from([0u8; 0]);
            let end = Key::from([0xFFu8; 64]);
            let results: Vec<_> = tree.range(&start, &end).collect();

            // Verify sorted order
            for window in results.windows(2) {
                prop_assert!(window[0].0 <= window[1].0);
            }

            // Verify no duplicate keys
            for window in results.windows(2) {
                prop_assert!(window[0].0 != window[1].0);
            }
        }

        /// Property: applying a random sequence of operations yields a consistent state.
        #[test]
        fn prop_random_ops_consistent(
            ops in prop::collection::vec(arb_op(), 1..100)
        ) {
            let dir = tempfile::tempdir().unwrap();
            let config = LsmConfig {
                memtable_size: 256,
                wal_enabled: false,
                ..LsmConfig::default()
            };
            let mut tree = LsmTree::open(dir.path(), config).unwrap();

            // Track expected state
            let mut expected: std::collections::HashMap<Vec<u8>, Vec<u8>> =
                std::collections::HashMap::new();

            for op in &ops {
                match op {
                    Op::Insert(key, value) => {
                        tree.insert(key, value).unwrap();
                        expected.insert(key.as_bytes().to_vec(), value.as_bytes().to_vec());
                    }
                    Op::Delete(key) => {
                        tree.delete(key).unwrap();
                        expected.remove(key.as_bytes());
                    }
                }
            }

            // Verify state matches expected
            for (key_bytes, val_bytes) in &expected {
                let result = tree.get(&Key::new(key_bytes.clone())).unwrap();
                prop_assert!(result.is_some(), "key {:?} missing", key_bytes);
                let got = result.unwrap();
                prop_assert_eq!(got.as_bytes(), val_bytes.as_slice());
            }
        }

        /// Property: WAL recovery restores identical state after crash.
        #[test]
        fn prop_wal_recovery(
            entries in prop::collection::vec((arb_key(), arb_value()), 1..20)
        ) {
            let dir = tempfile::tempdir().unwrap();
            let db_path = dir.path().to_path_buf();

            // Write without flushing (WAL only)
            {
                let config = LsmConfig {
                    memtable_size: 100 * 1024 * 1024, // Large — no flush
                    wal_enabled: true,
                    ..LsmConfig::default()
                };
                let mut tree = LsmTree::open(&db_path, config).unwrap();
                for (key, value) in &entries {
                    tree.insert(key, value).unwrap();
                }
                // Drop = simulated crash
            }

            // Recover
            let config = LsmConfig {
                memtable_size: 100 * 1024 * 1024,
                wal_enabled: true,
                ..LsmConfig::default()
            };
            let tree = LsmTree::open(&db_path, config).unwrap();

            // Last-write-wins expected state
            let mut expected: std::collections::HashMap<Vec<u8>, Vec<u8>> =
                std::collections::HashMap::new();
            for (key, value) in &entries {
                expected.insert(key.as_bytes().to_vec(), value.as_bytes().to_vec());
            }

            for (key_bytes, val_bytes) in &expected {
                let result = tree.get(&Key::new(key_bytes.clone())).unwrap();
                prop_assert!(result.is_some(), "WAL recovery lost key {:?}", key_bytes);
                let got = result.unwrap();
                prop_assert_eq!(got.as_bytes(), val_bytes.as_slice());
            }
        }

        /// Property: page encode/decode roundtrip preserves entries exactly.
        #[test]
        fn prop_page_roundtrip(
            entries in prop::collection::vec(
                (arb_key(), prop::option::of(arb_value())),
                1..10
            )
        ) {
            use crate::sstable::page::Page;
            // Sort entries by key (pages require sorted entries)
            let mut sorted = entries;
            sorted.sort_by(|a, b| a.0.cmp(&b.0));
            sorted.dedup_by(|a, b| a.0 == b.0);

            let page = Page { entries: sorted.clone() };

            // Only test if entries fit in a page
            let total_size: usize = sorted.iter()
                .map(|(k, v)| Page::entry_size(k, v))
                .sum();
            if total_size <= 4096 - 8 {
                let encoded = page.encode(4096).unwrap();
                let decoded = Page::decode(&encoded).unwrap();
                prop_assert_eq!(decoded.entries.len(), sorted.len());
                for (i, (dk, dv)) in decoded.entries.iter().enumerate() {
                    prop_assert_eq!(dk, &sorted[i].0);
                    prop_assert_eq!(dv, &sorted[i].1);
                }
            }
        }

        /// Property: bloom filter has zero false negatives.
        #[test]
        fn prop_bloom_no_false_negatives(
            keys in prop::collection::vec(arb_key(), 1..100)
        ) {
            let mut bloom = crate::bloom::BloomFilter::new(keys.len(), 10);
            for key in &keys {
                bloom.insert(key);
            }
            for key in &keys {
                prop_assert!(bloom.may_contain(key));
            }
        }
    }
}

/// High-volume stress tests simulating Cardano UTxO workloads.
///
/// These tests use 36-byte keys (matching Cardano TransactionInput encoding:
/// 32-byte tx hash + 4-byte index BE) and ~200-byte values (matching bincode-
/// serialized TransactionOutput). They exercise compaction, merge, WAL recovery,
/// and range scans at a scale closer to production (tens of thousands of entries).
#[cfg(test)]
mod stress_tests {
    use super::*;
    use std::collections::HashMap as StdHashMap;

    /// Generate a Cardano-like UTxO key: 32 bytes of tx hash + 4 bytes of index.
    fn utxo_key(tx_hash_seed: u32, index: u32) -> Key {
        let mut buf = [0u8; 36];
        // Simulate a tx hash by repeating the seed
        let seed_bytes = tx_hash_seed.to_be_bytes();
        for i in 0..8 {
            buf[i * 4..(i + 1) * 4].copy_from_slice(&seed_bytes);
        }
        buf[32..36].copy_from_slice(&index.to_be_bytes());
        Key::from(buf)
    }

    /// Generate a Cardano-like UTxO value (~200 bytes).
    fn utxo_value(seed: u32) -> Value {
        let mut buf = vec![0u8; 200];
        let seed_bytes = seed.to_be_bytes();
        for chunk in buf.chunks_mut(4) {
            let len = chunk.len().min(4);
            chunk[..len].copy_from_slice(&seed_bytes[..len]);
        }
        Value::from(buf)
    }

    /// Simulate a Cardano block application: consume N inputs, produce M outputs.
    /// Returns the new UTxO entries created.
    fn apply_block(
        tree: &mut LsmTree,
        block_num: u32,
        inputs_to_consume: &[(Key, Value)],
        num_outputs: u32,
    ) -> Vec<(Key, Value)> {
        // Delete consumed inputs
        for (key, _) in inputs_to_consume {
            tree.delete(key).unwrap();
        }

        // Create new outputs
        let mut new_entries = Vec::new();
        for idx in 0..num_outputs {
            let key = utxo_key(block_num, idx);
            let value = utxo_value(block_num * 1000 + idx);
            tree.insert(&key, &value).unwrap();
            new_entries.push((key, value));
        }

        new_entries
    }

    /// 50K-entry UTxO stress test with interleaved inserts, deletes, and compactions.
    ///
    /// Simulates 5,000 blocks each consuming 2 UTxOs and producing 3 UTxOs,
    /// resulting in a net growth of ~5,000 live UTxO entries with ~10,000 tombstones
    /// that must be correctly handled during compaction.
    #[test]
    fn test_utxo_workload_50k_entries() {
        let dir = tempfile::tempdir().unwrap();
        let config = LsmConfig {
            memtable_size: 256 * 1024, // 256 KB — forces frequent flushes
            block_cache_size: 4 * 1024 * 1024,
            bloom_filter_bits_per_key: 10,
            size_ratio: 4,
            wal_enabled: false, // WAL tested separately
            page_size: 4096,
            ..LsmConfig::default()
        };
        let mut tree = LsmTree::open(dir.path(), config).unwrap();

        // Track the live UTxO set as ground truth
        let mut live_utxos: StdHashMap<Vec<u8>, Vec<u8>> = StdHashMap::new();

        // Genesis: create 100 initial UTxOs
        for i in 0u32..100 {
            let key = utxo_key(0, i);
            let value = utxo_value(i);
            tree.insert(&key, &value).unwrap();
            live_utxos.insert(key.as_bytes().to_vec(), value.as_bytes().to_vec());
        }

        // Simulate 5,000 blocks
        for block in 1u32..=5000 {
            // Pick 2 random UTxOs to consume (use deterministic selection)
            let live_keys: Vec<Vec<u8>> = live_utxos.keys().cloned().collect();
            let mut to_consume = Vec::new();

            if live_keys.len() >= 2 {
                let idx1 = (block as usize * 7) % live_keys.len();
                let idx2 = (block as usize * 13 + 3) % live_keys.len();
                // Avoid consuming same UTxO twice
                let idx2 = if idx2 == idx1 {
                    (idx2 + 1) % live_keys.len()
                } else {
                    idx2
                };

                for &idx in &[idx1, idx2] {
                    let key_bytes = &live_keys[idx];
                    let val_bytes = live_utxos.get(key_bytes).unwrap().clone();
                    to_consume.push((Key::new(key_bytes.clone()), Value::new(val_bytes)));
                }
            }

            // Apply block: consume inputs, produce 3 outputs
            let new_entries = apply_block(&mut tree, block, &to_consume, 3);

            // Update ground truth
            for (key, _) in &to_consume {
                live_utxos.remove(key.as_bytes());
            }
            for (key, value) in &new_entries {
                live_utxos.insert(key.as_bytes().to_vec(), value.as_bytes().to_vec());
            }
        }

        // Verify: every live UTxO should be readable
        let mut missing = 0;
        let mut wrong = 0;
        for (key_bytes, expected_val) in &live_utxos {
            match tree.get(&Key::new(key_bytes.clone())).unwrap() {
                Some(val) => {
                    if val.as_bytes() != expected_val.as_slice() {
                        wrong += 1;
                    }
                }
                None => missing += 1,
            }
        }
        assert_eq!(
            missing,
            0,
            "{missing} live UTxOs missing from LSM tree (out of {})",
            live_utxos.len()
        );
        assert_eq!(
            wrong,
            0,
            "{wrong} live UTxOs have wrong values (out of {})",
            live_utxos.len()
        );

        // Verify: range scan returns correct count
        let start = Key::from([0u8; 0]);
        let end = Key::from([0xFFu8; 36]);
        let range_count = tree.range(&start, &end).count();
        assert_eq!(
            range_count,
            live_utxos.len(),
            "range scan returned {range_count} entries but expected {}",
            live_utxos.len()
        );

        // Final count check
        eprintln!(
            "Stress test passed: {} live UTxOs verified, range scan count matches",
            live_utxos.len()
        );
    }

    /// WAL crash recovery stress test: write data, simulate crash, verify recovery.
    #[test]
    fn test_wal_crash_recovery_stress() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().to_path_buf();
        let mut expected: StdHashMap<Vec<u8>, Vec<u8>> = StdHashMap::new();

        // Phase 1: write 1000 entries with WAL, don't flush (simulates crash)
        {
            let config = LsmConfig {
                memtable_size: 100 * 1024 * 1024, // Large — no auto-flush
                wal_enabled: true,
                ..LsmConfig::default()
            };
            let mut tree = LsmTree::open(&db_path, config).unwrap();

            for i in 0u32..1000 {
                let key = utxo_key(i, 0);
                let value = utxo_value(i);
                tree.insert(&key, &value).unwrap();
                expected.insert(key.as_bytes().to_vec(), value.as_bytes().to_vec());
            }
            // Drop without flush — simulates crash
        }

        // Phase 2: reopen and verify all data recovered
        {
            let config = LsmConfig {
                memtable_size: 100 * 1024 * 1024,
                wal_enabled: true,
                ..LsmConfig::default()
            };
            let tree = LsmTree::open(&db_path, config).unwrap();

            for (key_bytes, expected_val) in &expected {
                let result = tree.get(&Key::new(key_bytes.clone())).unwrap();
                assert!(
                    result.is_some(),
                    "WAL recovery lost key (first 4 bytes: {:?})",
                    &key_bytes[..4]
                );
                assert_eq!(result.unwrap().as_bytes(), expected_val.as_slice());
            }
        }
    }

    /// Snapshot consistency test: save snapshot mid-workload, verify it's frozen.
    #[test]
    fn test_snapshot_consistency_under_load() {
        let dir = tempfile::tempdir().unwrap();
        let config = LsmConfig {
            memtable_size: 4096,
            wal_enabled: false,
            ..LsmConfig::default()
        };
        let mut tree = LsmTree::open(dir.path(), config).unwrap();

        // Insert initial data
        for i in 0u32..500 {
            let key = utxo_key(i, 0);
            let value = utxo_value(i);
            tree.insert(&key, &value).unwrap();
        }

        // Save snapshot
        tree.save_snapshot("epoch-100", "epoch100").unwrap();

        // Continue modifying (these changes should NOT affect the snapshot)
        for i in 500u32..1000 {
            let key = utxo_key(i, 0);
            let value = utxo_value(i);
            tree.insert(&key, &value).unwrap();
        }
        // Delete some of the original entries
        for i in 0u32..100 {
            tree.delete(&utxo_key(i, 0)).unwrap();
        }

        // Open snapshot in a separate directory and verify it has exactly
        // the 500 entries from before the snapshot, not the modified state
        let dir2 = tempfile::tempdir().unwrap();
        let snap_src = dir.path().join("snapshots");
        let snap_dst = dir2.path().join("snapshots");
        copy_dir_recursive(&snap_src, &snap_dst).unwrap();

        let snap_tree = LsmTree::open_snapshot(dir2.path(), "epoch-100").unwrap();

        // Snapshot should have all 500 original entries
        for i in 0u32..500 {
            let key = utxo_key(i, 0);
            let result = snap_tree.get(&key).unwrap();
            assert!(
                result.is_some(),
                "snapshot missing key for tx_hash_seed={i}"
            );
        }

        // Snapshot should NOT have the 500 entries added after snapshot
        for i in 500u32..1000 {
            let key = utxo_key(i, 0);
            let result = snap_tree.get(&key).unwrap();
            assert!(
                result.is_none(),
                "snapshot should not contain post-snapshot key for tx_hash_seed={i}"
            );
        }
    }

    /// Recursive directory copy helper.
    fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
        if !src.exists() {
            return Ok(());
        }
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let src_path = entry.path();
            let dst_path = dst.join(entry.file_name());
            if src_path.is_dir() {
                copy_dir_recursive(&src_path, &dst_path)?;
            } else {
                std::fs::copy(&src_path, &dst_path)?;
            }
        }
        Ok(())
    }
}
