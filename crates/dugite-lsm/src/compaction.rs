//! Lazy levelling compaction strategy.
//!
//! - Levels 0..n-1: tiering — up to T runs per level (T = size_ratio, default 4)
//! - Level n (last): levelling — at most 1 run (new data is merged with existing)
//!
//! When level i reaches T runs, all runs at level i are merged into a single
//! run and pushed to level i+1. If level i+1 is the last level, the merge
//! includes the existing run at that level (full merge / levelling).

use std::collections::HashMap;
use std::path::Path;

use crate::cache::BlockCache;
use crate::error::Result;
use crate::key::Key;
use crate::level::Level;
use crate::merge::merge_entries;
use crate::run::Run;
use crate::value::Value;

/// Merge input: sequence number and sorted entries.
type MergeInput = Vec<(usize, Vec<(Key, Option<Value>)>)>;

/// Check if any level needs compaction and return the level index if so.
///
/// A level needs compaction when it has >= `size_ratio` runs (tiering).
pub fn find_compaction_level(levels: &[Level], size_ratio: usize) -> Option<usize> {
    for (i, level) in levels.iter().enumerate() {
        if level.num_runs() >= size_ratio {
            return Some(i);
        }
    }
    None
}

/// Parameters for a compaction operation.
pub struct CompactParams<'a> {
    pub active_dir: &'a Path,
    pub levels: &'a mut Vec<Level>,
    pub runs: &'a mut HashMap<u64, Run>,
    pub next_run_id: &'a mut u64,
    pub page_size: usize,
    pub bloom_bits_per_key: usize,
    pub cache: &'a mut BlockCache,
}

/// Perform a compaction at the given level.
///
/// Merges all runs at `level_idx` into a single run, and places the result
/// at `level_idx + 1`. If the target level is the last level and already has
/// a run, that run is included in the merge (levelling behavior).
///
/// Returns the IDs of runs that were consumed (and should be deleted) and
/// the ID of the new merged run.
pub fn compact_level(params: &mut CompactParams<'_>, level_idx: usize) -> Result<(Vec<u64>, u64)> {
    let CompactParams {
        active_dir,
        levels,
        runs,
        next_run_id,
        page_size,
        bloom_bits_per_key,
        cache,
    } = params;

    // Ensure the target level exists
    let target_level = level_idx + 1;
    while levels.len() <= target_level {
        levels.push(Level::new(levels.len()));
    }

    // Collect run IDs to merge from the source level
    let source_run_ids: Vec<u64> = levels[level_idx].run_ids.clone();

    // Check if target level has runs to include (levelling on last level)
    let is_last_level = target_level == levels.len() - 1
        || (target_level + 1 < levels.len()
            && levels[target_level + 1..].iter().all(|l| l.is_empty()));
    let include_target = is_last_level && !levels[target_level].is_empty();
    let target_run_ids: Vec<u64> = if include_target {
        levels[target_level].run_ids.clone()
    } else {
        Vec::new()
    };

    // Gather all entries from runs to merge (source runs have lower sequence numbers)
    let mut merge_inputs: MergeInput = Vec::new();
    let mut consumed_ids: Vec<u64> = Vec::new();

    // Target level runs first (oldest, lowest sequence)
    for (seq, &run_id) in target_run_ids.iter().enumerate() {
        if let Some(run) = runs.get(&run_id) {
            let entries = run.scan_all()?;
            merge_inputs.push((seq, entries));
            consumed_ids.push(run_id);
        }
    }

    // Source level runs (newer, higher sequence)
    let base_seq = target_run_ids.len();
    for (seq, &run_id) in source_run_ids.iter().enumerate() {
        if let Some(run) = runs.get(&run_id) {
            let entries = run.scan_all()?;
            merge_inputs.push((base_seq + seq, entries));
            consumed_ids.push(run_id);
        }
    }

    // Determine if we should drop tombstones (only at the last level with no
    // older data below)
    let drop_tombstones = is_last_level && include_target;

    // Merge
    let merged = merge_entries(merge_inputs, drop_tombstones);

    // Write merged run
    let new_id = **next_run_id;
    **next_run_id += 1;

    let new_run = if merged.is_empty() {
        // Nothing to write — compaction eliminated all entries
        None
    } else {
        Some(Run::write(
            active_dir,
            new_id,
            &merged,
            *page_size,
            *bloom_bits_per_key,
        )?)
    };

    // Update level structures
    levels[level_idx].run_ids.clear();
    for &id in &target_run_ids {
        levels[target_level].remove_run(id);
    }
    if new_run.is_some() {
        levels[target_level].add_run(new_id);
    }

    // Remove consumed runs from the runs map and delete their files
    for &id in &consumed_ids {
        runs.remove(&id);
        cache.invalidate_run(id);
        Run::delete_files(active_dir, id)?;
    }

    // Insert new run
    if let Some(run) = new_run {
        runs.insert(new_id, run);
    }

    Ok((consumed_ids, new_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_find_compaction_level() {
        let mut levels = vec![Level::new(0), Level::new(1)];
        assert!(find_compaction_level(&levels, 4).is_none());

        // Add 4 runs to level 0
        for i in 0..4 {
            levels[0].add_run(i);
        }
        assert_eq!(find_compaction_level(&levels, 4), Some(0));
    }

    #[test]
    fn test_compact_level() {
        let dir = tempfile::tempdir().unwrap();
        let active_dir = dir.path().join("active");
        fs::create_dir_all(&active_dir).unwrap();

        let mut runs = HashMap::new();
        let mut levels = vec![Level::new(0)];
        let mut next_run_id = 0u64;
        let mut cache = BlockCache::new(100);

        // Create 4 runs at level 0
        for i in 0u8..4 {
            let entries: Vec<(Key, Option<Value>)> = vec![
                (Key::from([i, 0]), Some(Value::from(vec![i; 10]))),
                (Key::from([i, 1]), Some(Value::from(vec![i; 10]))),
            ];
            let run = Run::write(&active_dir, next_run_id, &entries, 4096, 10).unwrap();
            levels[0].add_run(next_run_id);
            runs.insert(next_run_id, run);
            next_run_id += 1;
        }

        assert_eq!(levels[0].num_runs(), 4);

        // Compact level 0
        let mut params = CompactParams {
            active_dir: &active_dir,
            levels: &mut levels,
            runs: &mut runs,
            next_run_id: &mut next_run_id,
            page_size: 4096,
            bloom_bits_per_key: 10,
            cache: &mut cache,
        };

        let (consumed, new_id) = compact_level(&mut params, 0).unwrap();

        assert_eq!(consumed.len(), 4);
        assert!(levels[0].is_empty());
        assert_eq!(levels[1].num_runs(), 1);

        // Verify the merged run contains all entries
        let merged_run = runs.get(&new_id).unwrap();
        assert_eq!(merged_run.entry_count, 8); // 4 runs * 2 entries
    }
}
