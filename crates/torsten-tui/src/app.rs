//! Application state and update logic.
//!
//! The `App` struct holds all state needed to render the TUI dashboard,
//! including the latest metrics snapshot, historical block-rate samples
//! for the sparkline, and UI navigation state.

use crate::metrics::MetricsSnapshot;
use crate::theme::{self, Theme, THEMES};
use std::collections::VecDeque;

/// Maximum number of sparkline samples to retain (one sample per poll interval).
const SPARKLINE_CAPACITY: usize = 60;

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
}

/// Full application state for the TUI dashboard.
pub struct App {
    /// Latest scraped metrics.
    pub metrics: MetricsSnapshot,
    /// Historical block-rate values for the sparkline widget.
    /// Each entry is blocks_applied delta since the previous sample.
    pub block_rate_history: VecDeque<u64>,
    /// Previous blocks_applied value (for computing deltas).
    prev_blocks_applied: u64,
    /// Whether this is the first metrics update (skip delta on first sample).
    first_update: bool,
    /// Currently focused panel.
    pub active_panel: ActivePanel,
    /// Whether the help overlay is visible.
    pub show_help: bool,
    /// Whether the application should exit.
    pub should_quit: bool,
    /// Index into [`THEMES`] for the currently active theme.
    pub theme_index: usize,
}

impl App {
    /// Create a new App with default (empty) state.
    pub fn new() -> Self {
        Self {
            metrics: MetricsSnapshot::default(),
            block_rate_history: VecDeque::with_capacity(SPARKLINE_CAPACITY),
            prev_blocks_applied: 0,
            first_update: true,
            active_panel: ActivePanel::Chain,
            show_help: false,
            should_quit: false,
            theme_index: 0,
        }
    }

    /// Update the app state with a new metrics snapshot.
    ///
    /// Computes the block-rate delta and pushes it onto the sparkline history.
    pub fn update_metrics(&mut self, snapshot: MetricsSnapshot) {
        let current_blocks = snapshot.get_u64("torsten_blocks_applied_total");

        if self.first_update {
            // First sample: no delta to compute, just record the baseline.
            self.first_update = false;
            self.prev_blocks_applied = current_blocks;
        } else {
            // Compute blocks applied since last poll.
            let delta = current_blocks.saturating_sub(self.prev_blocks_applied);
            self.prev_blocks_applied = current_blocks;

            // Push to sparkline history, evicting oldest if at capacity.
            if self.block_rate_history.len() >= SPARKLINE_CAPACITY {
                self.block_rate_history.pop_front();
            }
            self.block_rate_history.push_back(delta);
        }

        self.metrics = snapshot;
    }

    /// Cycle to the next panel.
    pub fn next_panel(&mut self) {
        self.active_panel = self.active_panel.next();
    }

    /// Toggle the help overlay.
    pub fn toggle_help(&mut self) {
        self.show_help = !self.show_help;
    }

    /// Return a reference to the currently active theme.
    pub fn current_theme(&self) -> &'static Theme {
        &THEMES[self.theme_index]
    }

    /// Cycle to the next theme, wrapping around after the last.
    pub fn cycle_theme(&mut self) {
        self.theme_index = theme::cycle_theme(self.theme_index);
    }

    /// Set the theme by index. Clamps to valid range.
    pub fn set_theme(&mut self, index: usize) {
        self.theme_index = index.min(THEMES.len() - 1);
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
            connected: true,
            error: None,
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
    fn test_theme_cycling() {
        let mut app = App::new();
        assert_eq!(app.theme_index, 0);
        assert_eq!(app.current_theme().name, "Default");

        app.cycle_theme();
        assert_eq!(app.theme_index, 1);
        assert_eq!(app.current_theme().name, "Monokai");

        // Cycle through all themes and back to start
        for _ in 0..6 {
            app.cycle_theme();
        }
        assert_eq!(app.theme_index, 0);
        assert_eq!(app.current_theme().name, "Default");
    }

    #[test]
    fn test_set_theme() {
        let mut app = App::new();
        app.set_theme(4);
        assert_eq!(app.current_theme().name, "Nord");

        // Clamp to max
        app.set_theme(100);
        assert_eq!(app.theme_index, 6);
    }
}
