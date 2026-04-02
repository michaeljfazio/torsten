//! Level management for the LSM tree.
//!
//! Each level holds a collection of sorted runs. The compaction strategy
//! determines when runs at a level should be merged into the next level.

/// A level in the LSM tree, containing zero or more sorted runs.
#[derive(Debug)]
pub struct Level {
    /// Level number (0 = flush target, higher = older/larger data).
    pub number: usize,
    /// Run IDs at this level, ordered from oldest to newest.
    pub run_ids: Vec<u64>,
}

impl Level {
    /// Create a new empty level.
    pub fn new(number: usize) -> Self {
        Level {
            number,
            run_ids: Vec::new(),
        }
    }

    /// Add a run to this level (appended as newest).
    pub fn add_run(&mut self, run_id: u64) {
        self.run_ids.push(run_id);
    }

    /// Remove a run from this level.
    pub fn remove_run(&mut self, run_id: u64) {
        self.run_ids.retain(|&id| id != run_id);
    }

    /// Number of runs at this level.
    pub fn num_runs(&self) -> usize {
        self.run_ids.len()
    }

    /// Whether this level has no runs.
    pub fn is_empty(&self) -> bool {
        self.run_ids.is_empty()
    }
}
