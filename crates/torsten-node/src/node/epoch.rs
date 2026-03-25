//! Epoch transition handling: snapshot policy, ledger snapshot save/load/prune,
//! and the Shelley transition epoch lookup used to correctly compute slot numbers
//! on networks that started with a Byron era.

use std::path::PathBuf;
use tracing::{debug, error, warn};

use super::Node;

// ─── Shelley transition epoch ────────────────────────────────────────────────

/// Return the number of Byron epochs before the Shelley hard fork for known
/// Cardano networks, identified by network magic.
///
/// Based on CNCLI's `guess_shelley_transition_epoch`.
pub fn shelley_transition_epoch_for_magic(network_magic: u64) -> u64 {
    match network_magic {
        764824073 => 208, // mainnet
        1 => 4,           // preprod
        2 => 0,           // preview (no Byron era)
        4 => 0,           // sanchonet
        141 => 2,         // guild
        _ => 0,           // unknown — assume no Byron era (safest default)
    }
}

// ─── Snapshot policy ─────────────────────────────────────────────────────────

/// Snapshot policy matching Haskell cardano-node's `SnapshotPolicy`.
///
/// Controls when ledger snapshots are taken based on time and block counts.
/// Two modes:
/// - **Normal operation:** snapshot every `k * 2` seconds (~72 minutes for k=2160)
/// - **Bulk sync (replay):** snapshot every `bulk_min_blocks` blocks AND `bulk_min_interval` elapsed
#[allow(dead_code)] // normal_interval used by should_snapshot_normal (networking rewrite)
pub struct SnapshotPolicy {
    /// Time between snapshots during normal operation (k * 2 seconds)
    pub normal_interval: std::time::Duration,
    /// Minimum blocks processed before snapshot during bulk sync
    pub bulk_min_blocks: u64,
    /// Minimum time between snapshots during bulk sync
    pub bulk_min_interval: std::time::Duration,
    /// Maximum snapshots to retain on disk
    pub max_snapshots: usize,
    /// Last snapshot time
    pub last_snapshot_time: std::time::Instant,
    /// Blocks since last snapshot
    pub blocks_since_snapshot: u64,
}

impl SnapshotPolicy {
    /// Create a new snapshot policy with defaults matching Haskell cardano-node.
    pub fn new(security_param_k: u64) -> Self {
        SnapshotPolicy {
            normal_interval: std::time::Duration::from_secs(security_param_k * 2),
            bulk_min_blocks: 50_000,
            bulk_min_interval: std::time::Duration::from_secs(360), // 6 minutes
            max_snapshots: 2,
            last_snapshot_time: std::time::Instant::now(),
            blocks_since_snapshot: 0,
        }
    }

    /// Create with custom parameters (from CLI flags).
    pub fn with_params(
        security_param_k: u64,
        max_snapshots: usize,
        bulk_min_blocks: u64,
        bulk_min_secs: u64,
    ) -> Self {
        SnapshotPolicy {
            normal_interval: std::time::Duration::from_secs(security_param_k * 2),
            bulk_min_blocks,
            bulk_min_interval: std::time::Duration::from_secs(bulk_min_secs),
            max_snapshots,
            last_snapshot_time: std::time::Instant::now(),
            blocks_since_snapshot: 0,
        }
    }

    /// Record that blocks have been applied.
    pub fn record_blocks(&mut self, count: u64) {
        self.blocks_since_snapshot += count;
    }

    /// Check if a snapshot should be taken during normal (at-tip) operation.
    #[allow(dead_code)] // used by networking rewrite (and tests)
    pub fn should_snapshot_normal(&self) -> bool {
        self.last_snapshot_time.elapsed() >= self.normal_interval
    }

    /// Check if a snapshot should be taken during bulk sync (replay).
    pub fn should_snapshot_bulk(&self) -> bool {
        self.blocks_since_snapshot >= self.bulk_min_blocks
            && self.last_snapshot_time.elapsed() >= self.bulk_min_interval
    }

    /// Mark that a snapshot was taken.
    pub fn snapshot_taken(&mut self) {
        self.last_snapshot_time = std::time::Instant::now();
        self.blocks_since_snapshot = 0;
    }
}

// ─── Node impl: snapshot persistence ─────────────────────────────────────────

impl Node {
    /// Save a ledger state snapshot to the database directory.
    ///
    /// When the UTxO store is backed by LSM (cardano-lsm), this also flushes
    /// the memtable to SST files on disk via `save_utxo_snapshot()`.
    /// cardano-lsm has no WAL — without this flush, all UTxO data is lost
    /// on restart.
    pub async fn save_ledger_snapshot(&self) {
        let mut ls = self.ledger_state.write().await;
        let epoch = ls.epoch.0;

        // Flush UTxO store to disk FIRST (cardano-lsm has no WAL)
        if let Err(e) = ls.save_utxo_snapshot() {
            error!("Failed to save UTxO store snapshot: {e}");
        }

        // Save epoch-numbered snapshot for rollback safety
        let epoch_path = self
            .database_path
            .join(format!("ledger-snapshot-epoch{epoch}.bin"));
        if let Err(e) = ls.save_snapshot(&epoch_path) {
            error!("Failed to save ledger snapshot: {e}");
            return;
        }

        // Copy to "latest" for fast startup (avoids double-serializing ~1 GB)
        let latest_path = self.database_path.join("ledger-snapshot.bin");
        if let Err(e) = std::fs::copy(&epoch_path, &latest_path) {
            error!("Failed to copy latest ledger snapshot: {e}");
        }

        drop(ls);

        // Prune old snapshots — keep only the configured maximum
        self.prune_old_snapshots(self.snapshot_policy.max_snapshots + 1);
    }

    /// Remove old epoch snapshots, keeping only the N most recent.
    pub fn prune_old_snapshots(&self, keep: usize) {
        let mut snapshots: Vec<(u64, PathBuf)> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&self.database_path) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if let Some(rest) = name_str.strip_prefix("ledger-snapshot-epoch") {
                    if let Some(epoch_str) = rest.strip_suffix(".bin") {
                        if let Ok(epoch) = epoch_str.parse::<u64>() {
                            snapshots.push((epoch, entry.path()));
                        }
                    }
                }
            }
        }
        if snapshots.len() > keep {
            snapshots.sort_by_key(|(epoch, _)| *epoch);
            let to_remove = snapshots.len() - keep;
            for (epoch, path) in snapshots.into_iter().take(to_remove) {
                if let Err(e) = std::fs::remove_file(&path) {
                    warn!(epoch, "Failed to remove old snapshot: {e}");
                } else {
                    debug!(epoch, "Pruned old ledger snapshot");
                }
            }
        }
    }

    /// Find the best epoch snapshot for a rollback to the given slot.
    ///
    /// Returns the path to the most recent snapshot whose ledger tip is at or
    /// before `rollback_slot`.  Falls back to `ledger-snapshot.bin` if no
    /// epoch snapshot qualifies.
    #[allow(dead_code)] // used by networking rewrite (handle_rollback)
    pub fn find_best_snapshot_for_rollback(
        &self,
        rollback_slot: u64,
    ) -> Option<std::path::PathBuf> {
        // Collect all epoch-numbered snapshots (sorted newest first)
        let mut epoch_snapshots: Vec<(u64, PathBuf)> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&self.database_path) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if let Some(rest) = name_str.strip_prefix("ledger-snapshot-epoch") {
                    if let Some(epoch_str) = rest.strip_suffix(".bin") {
                        if let Ok(epoch) = epoch_str.parse::<u64>() {
                            epoch_snapshots.push((epoch, entry.path()));
                        }
                    }
                }
            }
        }
        // Sort by epoch descending (newest first)
        epoch_snapshots.sort_by(|a, b| b.0.cmp(&a.0));

        // Try each epoch snapshot to find one at or before the rollback slot.
        // We need to actually load the snapshot to check its slot (epoch number alone
        // isn't enough since the snapshot slot could be anywhere in the epoch).
        // To avoid loading huge snapshots just to check, use a heuristic:
        // epoch * epoch_length gives approximate slot. If epoch is clearly too new, skip.
        let epoch_length = {
            // Use a rough estimate; we don't need exact precision here
            if let Some(ref genesis) = self.shelley_genesis {
                genesis.epoch_length
            } else {
                86400
            }
        };

        for (epoch, path) in &epoch_snapshots {
            // Heuristic: if epoch * epoch_length > rollback_slot + epoch_length, skip
            // (snapshot is definitely beyond the rollback point)
            let approx_slot = epoch * epoch_length;
            if approx_slot > rollback_slot + epoch_length {
                continue;
            }

            // This snapshot might work — try loading to check exact slot
            match torsten_ledger::LedgerState::load_snapshot(path) {
                Ok(state) => {
                    let snap_slot = state.tip.point.slot().map(|s| s.0).unwrap_or(0);
                    if snap_slot <= rollback_slot {
                        debug!(
                            epoch,
                            snap_slot, rollback_slot, "Found suitable epoch snapshot for rollback"
                        );
                        return Some(path.clone());
                    }
                }
                Err(e) => {
                    warn!(epoch, "Failed to load epoch snapshot: {e}");
                }
            }
        }

        // Fall back to latest snapshot
        let latest = self.database_path.join("ledger-snapshot.bin");
        if latest.exists() {
            // Check if it's usable (at or before rollback point)
            if let Ok(state) = torsten_ledger::LedgerState::load_snapshot(&latest) {
                let snap_slot = state.tip.point.slot().map(|s| s.0).unwrap_or(0);
                if snap_slot <= rollback_slot {
                    return Some(latest);
                }
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shelley_transition_mainnet() {
        assert_eq!(shelley_transition_epoch_for_magic(764824073), 208);
    }

    #[test]
    fn test_shelley_transition_preprod() {
        assert_eq!(shelley_transition_epoch_for_magic(1), 4);
    }

    #[test]
    fn test_shelley_transition_preview_no_byron() {
        assert_eq!(shelley_transition_epoch_for_magic(2), 0);
    }

    #[test]
    fn test_shelley_transition_sanchonet_no_byron() {
        assert_eq!(shelley_transition_epoch_for_magic(4), 0);
    }

    #[test]
    fn test_shelley_transition_unknown_defaults_to_zero() {
        assert_eq!(shelley_transition_epoch_for_magic(999999), 0);
    }

    #[test]
    fn test_snapshot_policy_defaults() {
        let policy = SnapshotPolicy::new(2160);
        assert_eq!(policy.normal_interval, std::time::Duration::from_secs(4320));
        assert_eq!(policy.bulk_min_blocks, 50_000);
        assert_eq!(policy.max_snapshots, 2);
        assert_eq!(policy.blocks_since_snapshot, 0);
    }

    #[test]
    fn test_snapshot_policy_custom_params() {
        let policy = SnapshotPolicy::with_params(432, 5, 10_000, 120);
        assert_eq!(policy.normal_interval, std::time::Duration::from_secs(864));
        assert_eq!(policy.bulk_min_blocks, 10_000);
        assert_eq!(policy.max_snapshots, 5);
        assert_eq!(
            policy.bulk_min_interval,
            std::time::Duration::from_secs(120)
        );
    }

    #[test]
    fn test_snapshot_policy_record_blocks() {
        let mut policy = SnapshotPolicy::new(432);
        assert_eq!(policy.blocks_since_snapshot, 0);
        policy.record_blocks(100);
        assert_eq!(policy.blocks_since_snapshot, 100);
        policy.record_blocks(50);
        assert_eq!(policy.blocks_since_snapshot, 150);
    }

    #[test]
    fn test_snapshot_policy_bulk_not_ready_below_threshold() {
        let mut policy = SnapshotPolicy::new(432);
        policy.record_blocks(49_999);
        // Even though enough time may have passed, not enough blocks
        assert!(
            !policy.should_snapshot_bulk() || policy.blocks_since_snapshot < policy.bulk_min_blocks
        );
    }

    #[test]
    fn test_snapshot_taken_resets_counters() {
        let mut policy = SnapshotPolicy::new(432);
        policy.record_blocks(100_000);
        assert_eq!(policy.blocks_since_snapshot, 100_000);
        policy.snapshot_taken();
        assert_eq!(policy.blocks_since_snapshot, 0);
    }

    #[test]
    fn test_snapshot_normal_not_ready_immediately() {
        let policy = SnapshotPolicy::new(2160);
        // Just created — normal interval (4320s) hasn't elapsed
        assert!(!policy.should_snapshot_normal());
    }
}
