//! Application state and update logic.
//!
//! The `App` struct holds all state needed to render the TUI dashboard,
//! including the latest metrics snapshot, historical block-rate samples
//! for the sparkline, and UI navigation state.

use crate::layout::LayoutMode;
use crate::metrics::MetricsSnapshot;
use std::collections::VecDeque;

/// Maximum number of sparkline samples to retain (one sample per poll interval).
const SPARKLINE_CAPACITY: usize = 60;

/// Push a value onto a ring buffer VecDeque, evicting the oldest if at capacity.
fn push_to_ring(ring: &mut VecDeque<u64>, value: u64) {
    if ring.len() >= SPARKLINE_CAPACITY {
        ring.pop_front();
    }
    ring.push_back(value);
}

/// Active panel for keyboard navigation (Tab cycling).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivePanel {
    Chain,
    Peers,
    Performance,
    Governance,
}

impl ActivePanel {
    /// Cycle to the next panel in order.
    pub fn next(self) -> Self {
        match self {
            ActivePanel::Chain => ActivePanel::Peers,
            ActivePanel::Peers => ActivePanel::Performance,
            ActivePanel::Performance => ActivePanel::Governance,
            ActivePanel::Governance => ActivePanel::Chain,
        }
    }

    /// Cycle to the previous panel in order.
    pub fn prev(self) -> Self {
        match self {
            ActivePanel::Chain => ActivePanel::Governance,
            ActivePanel::Peers => ActivePanel::Chain,
            ActivePanel::Performance => ActivePanel::Peers,
            ActivePanel::Governance => ActivePanel::Performance,
        }
    }

    /// Jump to a specific panel by 1-based index (1=Chain, 2=Peers, ...).
    /// Returns `None` if the index is out of range.
    pub fn from_index(idx: usize) -> Option<Self> {
        match idx {
            1 => Some(ActivePanel::Chain),
            2 => Some(ActivePanel::Peers),
            3 => Some(ActivePanel::Performance),
            4 => Some(ActivePanel::Governance),
            _ => None,
        }
    }
}

/// Full application state for the TUI dashboard.
pub struct App {
    /// Latest scraped metrics.
    pub metrics: MetricsSnapshot,
    /// Historical block-rate values for the sparkline widget.
    /// Each entry is blocks_applied delta since the previous sample.
    pub block_rate_history: VecDeque<u64>,
    /// Historical tx validation rate (delta per poll interval).
    pub tx_rate_history: VecDeque<u64>,
    /// Historical mempool depth snapshots (tx count per poll).
    pub mempool_depth_history: VecDeque<u64>,
    /// Historical memory usage snapshots (bytes per poll).
    pub memory_history: VecDeque<u64>,
    /// Previous blocks_applied value (for computing deltas).
    prev_blocks_applied: u64,
    /// Previous txs_validated value (for computing deltas).
    prev_txs_validated: u64,
    /// Whether this is the first metrics update (skip delta on first sample).
    first_update: bool,
    /// Currently focused panel.
    pub active_panel: ActivePanel,
    /// Whether the help overlay is visible.
    pub show_help: bool,
    /// Whether the application should exit.
    pub should_quit: bool,
    /// Manual layout mode override. `None` means auto-detect from terminal size.
    pub layout_mode: Option<LayoutMode>,
    /// Number of slots remaining in the current epoch.
    pub epoch_slots_remaining: u64,
    /// Epoch progress as a percentage (0.0 - 100.0).
    pub epoch_progress_pct: f64,
    /// Seconds remaining until the next epoch boundary.
    pub epoch_time_remaining_secs: u64,
    /// Slot position within the current epoch.
    pub slot_in_epoch: u64,
    /// Network-specific epoch length in slots. 0 = auto-detect from metrics.
    pub epoch_length_override: u64,
}

impl App {
    /// Create a new App with default (empty) state.
    pub fn new() -> Self {
        Self {
            metrics: MetricsSnapshot::default(),
            block_rate_history: VecDeque::with_capacity(SPARKLINE_CAPACITY),
            tx_rate_history: VecDeque::with_capacity(SPARKLINE_CAPACITY),
            mempool_depth_history: VecDeque::with_capacity(SPARKLINE_CAPACITY),
            memory_history: VecDeque::with_capacity(SPARKLINE_CAPACITY),
            prev_blocks_applied: 0,
            prev_txs_validated: 0,
            first_update: true,
            active_panel: ActivePanel::Chain,
            show_help: false,
            should_quit: false,
            layout_mode: None,
            epoch_slots_remaining: 0,
            epoch_progress_pct: 0.0,
            epoch_time_remaining_secs: 0,
            slot_in_epoch: 0,
            epoch_length_override: 0,
        }
    }

    /// Update the app state with a new metrics snapshot.
    ///
    /// Computes block-rate, tx-rate deltas and pushes snapshots onto
    /// the respective sparkline histories.
    pub fn update_metrics(&mut self, snapshot: MetricsSnapshot) {
        let current_blocks = snapshot.get_u64("torsten_blocks_applied_total");
        let current_txs = snapshot.get_u64("torsten_transactions_validated_total");
        let mempool_count = snapshot.get_u64("torsten_mempool_tx_count");
        let mem_bytes = snapshot.get_u64("torsten_mem_resident_bytes");

        if self.first_update {
            // First sample: no delta to compute, just record baselines.
            self.first_update = false;
            self.prev_blocks_applied = current_blocks;
            self.prev_txs_validated = current_txs;
        } else {
            // Compute blocks applied since last poll.
            let block_delta = current_blocks.saturating_sub(self.prev_blocks_applied);
            self.prev_blocks_applied = current_blocks;
            push_to_ring(&mut self.block_rate_history, block_delta);

            // Compute txs validated since last poll.
            let tx_delta = current_txs.saturating_sub(self.prev_txs_validated);
            self.prev_txs_validated = current_txs;
            push_to_ring(&mut self.tx_rate_history, tx_delta);

            // Snapshot mempool depth and memory usage.
            push_to_ring(&mut self.mempool_depth_history, mempool_count);
            push_to_ring(&mut self.memory_history, mem_bytes);
        }

        // Compute epoch progress from slot_number and epoch_length.
        // Use override if set, otherwise try the metric, otherwise fall back to 432,000.
        let epoch_length = if self.epoch_length_override > 0 {
            self.epoch_length_override
        } else {
            let metric_len = snapshot.get_u64("torsten_epoch_length");
            if metric_len > 0 {
                metric_len
            } else {
                432_000
            }
        };
        let slot = snapshot.get_u64("torsten_slot_number");
        let slot_in_epoch = slot % epoch_length;
        self.slot_in_epoch = slot_in_epoch;
        self.epoch_slots_remaining = epoch_length.saturating_sub(slot_in_epoch);
        // Each slot is 1 second on Cardano.
        self.epoch_time_remaining_secs = self.epoch_slots_remaining;
        self.epoch_progress_pct = if epoch_length > 0 {
            (slot_in_epoch as f64 / epoch_length as f64) * 100.0
        } else {
            0.0
        };

        self.metrics = snapshot;
    }

    /// Compute the current tx throughput (txs per second) based on the last
    /// sparkline sample and the poll interval.
    pub fn txs_per_second(&self, poll_interval_secs: f64) -> f64 {
        self.tx_rate_history.back().copied().unwrap_or(0) as f64 / poll_interval_secs
    }

    /// Get the effective epoch length (respecting overrides).
    pub fn epoch_length(&self) -> u64 {
        if self.epoch_length_override > 0 {
            self.epoch_length_override
        } else {
            let metric_len = self.metrics.get_u64("torsten_epoch_length");
            if metric_len > 0 {
                metric_len
            } else {
                432_000
            }
        }
    }

    /// Cycle to the next panel.
    pub fn next_panel(&mut self) {
        self.active_panel = self.active_panel.next();
    }

    /// Cycle to the previous panel.
    pub fn prev_panel(&mut self) {
        self.active_panel = self.active_panel.prev();
    }

    /// Jump to a specific panel by 1-based index (1-4).
    pub fn jump_to_panel(&mut self, idx: usize) {
        if let Some(panel) = ActivePanel::from_index(idx) {
            self.active_panel = panel;
        }
    }

    /// Toggle the help overlay.
    pub fn toggle_help(&mut self) {
        self.show_help = !self.show_help;
    }

    /// Cycle layout mode: Auto -> Full -> Standard -> Compact -> Auto.
    pub fn toggle_layout_mode(&mut self) {
        self.layout_mode = match self.layout_mode {
            None => Some(LayoutMode::Full),
            Some(LayoutMode::Full) => Some(LayoutMode::Standard),
            Some(LayoutMode::Standard) => Some(LayoutMode::Compact),
            Some(LayoutMode::Compact) => None,
        };
    }

    /// Determine the sync status label and associated color hint.
    ///
    /// Returns (label, is_synced, is_stalled):
    /// - "Synced" when progress >= 99.9%
    /// - "Stalled" when tip_age > 300s and progress < 99%
    /// - "Syncing XX.XX%" otherwise
    pub fn sync_status(&self) -> (&str, bool, bool) {
        let pct_raw = self.metrics.get("torsten_sync_progress_percent");
        let pct = pct_raw / 100.0;
        let tip_age = self.metrics.get_u64("torsten_tip_age_seconds");

        if pct >= 99.9 {
            ("Synced", true, false)
        } else if tip_age > 300 && pct < 99.0 {
            ("Stalled", false, true)
        } else {
            ("Syncing", false, false)
        }
    }

    /// Format sync progress as a percentage string.
    pub fn sync_progress_pct(&self) -> f64 {
        self.metrics.get("torsten_sync_progress_percent") / 100.0
    }

    /// Compute the current block rate (blocks per second) based on the last
    /// sparkline sample and the poll interval.
    pub fn blocks_per_second(&self, poll_interval_secs: f64) -> f64 {
        self.block_rate_history.back().copied().unwrap_or(0) as f64 / poll_interval_secs
    }

    /// Format a lovelace amount as a human-readable ADA string with suffix.
    /// e.g. 14_070_000_000_000 -> "14.07T"
    pub fn format_ada(lovelace: u64) -> String {
        let ada = lovelace as f64 / 1_000_000.0;
        if ada >= 1_000_000_000_000.0 {
            format!("{:.2}T", ada / 1_000_000_000_000.0)
        } else if ada >= 1_000_000_000.0 {
            format!("{:.2}B", ada / 1_000_000_000.0)
        } else if ada >= 1_000_000.0 {
            format!("{:.2}M", ada / 1_000_000.0)
        } else if ada >= 1_000.0 {
            format!("{:.2}K", ada / 1_000.0)
        } else {
            format!("{:.2}", ada)
        }
    }

    /// Format bytes as human-readable size.
    pub fn format_bytes(bytes: u64) -> String {
        if bytes >= 1_073_741_824 {
            format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
        } else if bytes >= 1_048_576 {
            format!("{:.1} MB", bytes as f64 / 1_048_576.0)
        } else if bytes >= 1_024 {
            format!("{:.1} KB", bytes as f64 / 1_024.0)
        } else {
            format!("{} B", bytes)
        }
    }

    /// Format a large number with comma separators.
    pub fn format_number(n: u64) -> String {
        let s = n.to_string();
        let mut result = String::with_capacity(s.len() + s.len() / 3);
        for (i, c) in s.chars().rev().enumerate() {
            if i > 0 && i % 3 == 0 {
                result.push(',');
            }
            result.push(c);
        }
        result.chars().rev().collect()
    }

    /// Format uptime as a human-readable duration.
    pub fn format_uptime(secs: u64) -> String {
        let days = secs / 86400;
        let hours = (secs % 86400) / 3600;
        let mins = (secs % 3600) / 60;
        if days > 0 {
            format!("{}d {}h {}m", days, hours, mins)
        } else if hours > 0 {
            format!("{}h {}m", hours, mins)
        } else {
            format!("{}m {}s", mins, secs % 60)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn make_snapshot(values: Vec<(&str, f64)>) -> MetricsSnapshot {
        MetricsSnapshot {
            values: values
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
            _labeled: std::collections::HashMap::new(),
            connected: true,
            _error: None,
        }
    }

    #[test]
    fn test_block_rate_history() {
        let mut app = App::new();

        // First update: no delta pushed
        app.update_metrics(make_snapshot(vec![("torsten_blocks_applied_total", 100.0)]));
        assert!(app.block_rate_history.is_empty());

        // Second update: delta = 150 - 100 = 50
        app.update_metrics(make_snapshot(vec![("torsten_blocks_applied_total", 150.0)]));
        assert_eq!(app.block_rate_history.len(), 1);
        assert_eq!(app.block_rate_history[0], 50);

        // Third update: delta = 200 - 150 = 50
        app.update_metrics(make_snapshot(vec![("torsten_blocks_applied_total", 200.0)]));
        assert_eq!(app.block_rate_history.len(), 2);
        assert_eq!(app.block_rate_history[1], 50);
    }

    #[test]
    fn test_sparkline_capacity() {
        let mut app = App::new();
        app.update_metrics(make_snapshot(vec![("torsten_blocks_applied_total", 0.0)]));

        // Fill beyond capacity
        for i in 1..=70 {
            app.update_metrics(make_snapshot(vec![(
                "torsten_blocks_applied_total",
                i as f64,
            )]));
        }
        assert_eq!(app.block_rate_history.len(), SPARKLINE_CAPACITY);
    }

    #[test]
    fn test_sync_status() {
        let mut app = App::new();

        // Synced
        app.metrics = make_snapshot(vec![
            ("torsten_sync_progress_percent", 9990.0),
            ("torsten_tip_age_seconds", 5.0),
        ]);
        let (label, synced, stalled) = app.sync_status();
        assert_eq!(label, "Synced");
        assert!(synced);
        assert!(!stalled);

        // Syncing
        app.metrics = make_snapshot(vec![
            ("torsten_sync_progress_percent", 5000.0),
            ("torsten_tip_age_seconds", 100.0),
        ]);
        let (label, synced, stalled) = app.sync_status();
        assert_eq!(label, "Syncing");
        assert!(!synced);
        assert!(!stalled);

        // Stalled
        app.metrics = make_snapshot(vec![
            ("torsten_sync_progress_percent", 5000.0),
            ("torsten_tip_age_seconds", 600.0),
        ]);
        let (label, synced, stalled) = app.sync_status();
        assert_eq!(label, "Stalled");
        assert!(!synced);
        assert!(stalled);
    }

    #[test]
    fn test_format_ada() {
        // 14.07T ADA = 14.07e18 lovelace
        assert_eq!(App::format_ada(14_070_000_000_000_000_000), "14.07T");
        // 14.07B ADA = 14.07e15 lovelace
        assert_eq!(App::format_ada(14_070_000_000_000_000), "14.07B");
        // 14.07M ADA = 14.07e12 lovelace
        assert_eq!(App::format_ada(14_070_000_000_000), "14.07M");
        assert_eq!(App::format_ada(1_000_000), "1.00");
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(App::format_bytes(6_076_211_200), "5.7 GB");
        assert_eq!(App::format_bytes(1_048_576), "1.0 MB");
        assert_eq!(App::format_bytes(1024), "1.0 KB");
        assert_eq!(App::format_bytes(500), "500 B");
    }

    #[test]
    fn test_format_number() {
        assert_eq!(App::format_number(4_109_330), "4,109,330");
        assert_eq!(App::format_number(0), "0");
        assert_eq!(App::format_number(999), "999");
        assert_eq!(App::format_number(1_000), "1,000");
    }

    #[test]
    fn test_format_uptime() {
        assert_eq!(App::format_uptime(90061), "1d 1h 1m");
        assert_eq!(App::format_uptime(3661), "1h 1m");
        assert_eq!(App::format_uptime(61), "1m 1s");
    }

    #[test]
    fn test_panel_cycling() {
        let mut app = App::new();
        assert_eq!(app.active_panel, ActivePanel::Chain);
        app.next_panel();
        assert_eq!(app.active_panel, ActivePanel::Peers);
        app.next_panel();
        assert_eq!(app.active_panel, ActivePanel::Performance);
        app.next_panel();
        assert_eq!(app.active_panel, ActivePanel::Governance);
        app.next_panel();
        assert_eq!(app.active_panel, ActivePanel::Chain);
    }

    #[test]
    fn test_help_toggle() {
        let mut app = App::new();
        assert!(!app.show_help);
        app.toggle_help();
        assert!(app.show_help);
        app.toggle_help();
        assert!(!app.show_help);
    }

    #[test]
    fn test_prev_panel_cycling() {
        let mut app = App::new();
        assert_eq!(app.active_panel, ActivePanel::Chain);
        app.prev_panel();
        assert_eq!(app.active_panel, ActivePanel::Governance);
        app.prev_panel();
        assert_eq!(app.active_panel, ActivePanel::Performance);
        app.prev_panel();
        assert_eq!(app.active_panel, ActivePanel::Peers);
        app.prev_panel();
        assert_eq!(app.active_panel, ActivePanel::Chain);
    }

    #[test]
    fn test_jump_to_panel() {
        let mut app = App::new();
        app.jump_to_panel(3);
        assert_eq!(app.active_panel, ActivePanel::Performance);
        app.jump_to_panel(1);
        assert_eq!(app.active_panel, ActivePanel::Chain);
        // Out-of-range index is ignored.
        app.jump_to_panel(5);
        assert_eq!(app.active_panel, ActivePanel::Chain);
        app.jump_to_panel(0);
        assert_eq!(app.active_panel, ActivePanel::Chain);
    }

    #[test]
    fn test_tx_rate_history() {
        let mut app = App::new();

        // First update: baselines recorded, no delta.
        app.update_metrics(make_snapshot(vec![
            ("torsten_blocks_applied_total", 100.0),
            ("torsten_transactions_validated_total", 50.0),
        ]));
        assert!(app.tx_rate_history.is_empty());

        // Second update: tx delta = 80 - 50 = 30.
        app.update_metrics(make_snapshot(vec![
            ("torsten_blocks_applied_total", 110.0),
            ("torsten_transactions_validated_total", 80.0),
        ]));
        assert_eq!(app.tx_rate_history.len(), 1);
        assert_eq!(app.tx_rate_history[0], 30);
    }

    #[test]
    fn test_mempool_and_memory_history() {
        let mut app = App::new();

        app.update_metrics(make_snapshot(vec![
            ("torsten_mempool_tx_count", 5.0),
            ("torsten_mem_resident_bytes", 1_000_000.0),
        ]));
        // First update: no snapshot pushed.
        assert!(app.mempool_depth_history.is_empty());
        assert!(app.memory_history.is_empty());

        app.update_metrics(make_snapshot(vec![
            ("torsten_mempool_tx_count", 8.0),
            ("torsten_mem_resident_bytes", 2_000_000.0),
        ]));
        assert_eq!(app.mempool_depth_history.len(), 1);
        assert_eq!(app.mempool_depth_history[0], 8);
        assert_eq!(app.memory_history[0], 2_000_000);
    }

    #[test]
    fn test_epoch_time_remaining() {
        let mut app = App::new();
        app.update_metrics(make_snapshot(vec![
            ("torsten_slot_number", 100_000.0),
            ("torsten_epoch_length", 432_000.0),
        ]));
        assert_eq!(app.slot_in_epoch, 100_000);
        assert_eq!(app.epoch_slots_remaining, 332_000);
        assert_eq!(app.epoch_time_remaining_secs, 332_000);
    }

    #[test]
    fn test_epoch_length_override() {
        let mut app = App::new();
        app.epoch_length_override = 86_400;
        app.update_metrics(make_snapshot(vec![("torsten_slot_number", 43_200.0)]));
        assert_eq!(app.slot_in_epoch, 43_200);
        assert_eq!(app.epoch_length(), 86_400);
        assert!((app.epoch_progress_pct - 50.0).abs() < 0.1);
    }

    #[test]
    fn test_txs_per_second() {
        let mut app = App::new();
        app.update_metrics(make_snapshot(vec![(
            "torsten_transactions_validated_total",
            0.0,
        )]));
        app.update_metrics(make_snapshot(vec![(
            "torsten_transactions_validated_total",
            20.0,
        )]));
        // 20 txs in 2 second poll = 10 tx/s.
        assert!((app.txs_per_second(2.0) - 10.0).abs() < 0.01);
    }
}
