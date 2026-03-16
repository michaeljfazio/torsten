//! Application state and update logic.
//!
//! The `App` struct holds all state needed to render the TUI dashboard,
//! including the latest metrics snapshot, computed epoch progress values,
//! RTT histogram buckets, and UI navigation state (active theme, etc).

use crate::metrics::MetricsSnapshot;
use crate::theme::{cycle_theme, THEMES};

/// Network name derived from the `torsten_network_magic` metric.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Network {
    Mainnet,
    Preview,
    Preprod,
    Guild,
    Unknown,
}

impl Network {
    /// Resolve from the raw magic integer scraped from Prometheus.
    pub fn from_magic(magic: u64) -> Self {
        match magic {
            764_824_073 => Network::Mainnet,
            2 => Network::Preview,
            1 => Network::Preprod,
            141 => Network::Guild,
            _ => Network::Unknown,
        }
    }

    /// Human-readable label.
    pub fn label(self) -> &'static str {
        match self {
            Network::Mainnet => "Mainnet",
            Network::Preview => "Preview",
            Network::Preprod => "Preprod",
            Network::Guild => "Guild",
            Network::Unknown => "Unknown",
        }
    }

    /// Return the slot-per-epoch length for the network.
    pub fn epoch_length(self) -> u64 {
        match self {
            // Preview: 1 day = 86,400 slots
            Network::Preview => 86_400,
            // Mainnet / Preprod: 5 days = 432,000 slots
            Network::Mainnet | Network::Preprod | Network::Guild | Network::Unknown => 432_000,
        }
    }
}

/// RTT histogram buckets derived from the `torsten_peer_handshake_rtt_ms` histogram.
///
/// Buckets are cumulative (standard Prometheus histogram); we convert to per-band counts.
#[derive(Debug, Clone, Default)]
pub struct RttBands {
    /// Peers with RTT 0–50 ms.
    pub band_0_50: u64,
    /// Peers with RTT 50–100 ms.
    pub band_50_100: u64,
    /// Peers with RTT 100–200 ms.
    pub band_100_200: u64,
    /// Peers with RTT 200 ms+.
    pub band_200_plus: u64,
    /// Lowest RTT observed (ms), or None if no samples.
    pub min_ms: Option<f64>,
    /// Average RTT across all samples (ms), or None if no samples.
    pub avg_ms: Option<f64>,
    /// Highest RTT observed (ms), or None if no samples.
    pub max_ms: Option<f64>,
}

impl RttBands {
    /// Parse RTT band counts from the raw Prometheus histogram metrics.
    ///
    /// The Prometheus histogram for `torsten_peer_handshake_rtt_ms` exposes:
    ///   - `torsten_peer_handshake_rtt_ms_bucket_le_50`  (cumulative count ≤ 50 ms)
    ///   - `torsten_peer_handshake_rtt_ms_bucket_le_100` (cumulative count ≤ 100 ms)
    ///   - `torsten_peer_handshake_rtt_ms_bucket_le_200` (cumulative count ≤ 200 ms)
    ///   - `torsten_peer_handshake_rtt_ms_count`         (total count)
    ///   - `torsten_peer_handshake_rtt_ms_sum`           (total sum ms)
    ///
    /// Note: the histogram parser in `metrics.rs` uses synthetic metric names
    /// (with label suffix stripped) for buckets emitted with `{le="..."}` labels.
    /// We rely on the labeled bucket values stored under the `histogram_buckets` map.
    pub fn from_snapshot(snap: &MetricsSnapshot) -> Self {
        // Cumulative counts from histogram buckets (stored with le-label suffix).
        let le50 = snap
            .histogram_buckets
            .get("torsten_peer_handshake_rtt_ms")
            .and_then(|buckets| buckets.get("50"))
            .copied()
            .unwrap_or(0.0) as u64;
        let le100 = snap
            .histogram_buckets
            .get("torsten_peer_handshake_rtt_ms")
            .and_then(|buckets| buckets.get("100"))
            .copied()
            .unwrap_or(0.0) as u64;
        let le200 = snap
            .histogram_buckets
            .get("torsten_peer_handshake_rtt_ms")
            .and_then(|buckets| buckets.get("200"))
            .copied()
            .unwrap_or(0.0) as u64;
        let total = snap.get_u64("torsten_peer_handshake_rtt_ms_count");
        let sum = snap.get("torsten_peer_handshake_rtt_ms_sum");

        // Per-band counts (convert from cumulative).
        let band_0_50 = le50;
        let band_50_100 = le100.saturating_sub(le50);
        let band_100_200 = le200.saturating_sub(le100);
        let band_200_plus = total.saturating_sub(le200);

        let avg_ms = if total > 0 {
            Some(sum / total as f64)
        } else {
            None
        };

        // Prefer explicit _min / _max gauges if the node publishes them.
        // Otherwise approximate from the lowest / highest populated bucket midpoints.
        let min_approx: Option<f64> = if band_0_50 > 0 {
            Some(25.0) // midpoint of 0-50ms
        } else if band_50_100 > 0 {
            Some(75.0)
        } else if band_100_200 > 0 {
            Some(150.0)
        } else if total > 0 {
            Some(200.0)
        } else {
            None
        };
        let max_approx: Option<f64> = if total > 0 && band_200_plus > 0 {
            Some(300.0) // representative for 200ms+
        } else if band_100_200 > 0 {
            Some(150.0)
        } else if band_50_100 > 0 {
            Some(75.0)
        } else if band_0_50 > 0 {
            Some(25.0)
        } else {
            None
        };
        let min_ms = snap
            .values
            .get("torsten_peer_handshake_rtt_ms_min")
            .copied()
            .or(min_approx);
        let max_ms = snap
            .values
            .get("torsten_peer_handshake_rtt_ms_max")
            .copied()
            .or(max_approx);

        RttBands {
            band_0_50,
            band_50_100,
            band_100_200,
            band_200_plus,
            min_ms,
            avg_ms,
            max_ms,
        }
    }
}

/// Full application state for the TUI dashboard.
pub struct App {
    /// Latest scraped metrics.
    pub metrics: MetricsSnapshot,
    /// Network inferred from `torsten_network_magic`.
    pub network: Network,
    /// Epoch slot position.
    pub slot_in_epoch: u64,
    /// Epoch progress 0.0–100.0.
    pub epoch_progress_pct: f64,
    /// Seconds remaining until the next epoch boundary.
    pub epoch_time_remaining_secs: u64,
    /// Manual epoch length override (0 = auto-detect from metrics/network).
    pub epoch_length_override: u64,
    /// RTT histogram bands from the last scrape.
    pub rtt_bands: RttBands,
    /// Index into `THEMES` for the active theme.
    pub theme_idx: usize,
    /// Whether the application should exit.
    pub should_quit: bool,
    /// Whether the help overlay is visible.
    pub show_help: bool,
}

impl App {
    /// Create a new App with default (empty) state.
    ///
    /// The default theme is Monokai (index 1 in the `THEMES` array) — a warm,
    /// high-contrast palette that reads well on most terminal emulators.
    pub fn new() -> Self {
        // Locate the Monokai theme index dynamically so that reordering THEMES
        // does not silently break the default.
        let monokai_idx = THEMES.iter().position(|t| t.name == "Monokai").unwrap_or(0);
        Self {
            metrics: MetricsSnapshot::default(),
            network: Network::Unknown,
            slot_in_epoch: 0,
            epoch_progress_pct: 0.0,
            epoch_time_remaining_secs: 0,
            epoch_length_override: 0,
            rtt_bands: RttBands::default(),
            theme_idx: monokai_idx,
            should_quit: false,
            show_help: false,
        }
    }

    /// Update the app state with a new metrics snapshot.
    pub fn update_metrics(&mut self, snapshot: MetricsSnapshot) {
        // Resolve network from magic.
        let magic = snapshot.get_u64("torsten_network_magic");
        if magic > 0 {
            self.network = Network::from_magic(magic);
        }

        // Compute epoch progress.
        let epoch_length = self.epoch_length_for_snapshot(&snapshot);
        let slot = snapshot.get_u64("torsten_slot_number");
        let slot_in_epoch = slot % epoch_length.max(1);
        self.slot_in_epoch = slot_in_epoch;
        self.epoch_slots_remaining_inner(epoch_length, slot_in_epoch);

        // Parse RTT histogram bands.
        self.rtt_bands = RttBands::from_snapshot(&snapshot);

        self.metrics = snapshot;
    }

    fn epoch_slots_remaining_inner(&mut self, epoch_length: u64, slot_in_epoch: u64) {
        let remaining = epoch_length.saturating_sub(slot_in_epoch);
        self.epoch_time_remaining_secs = remaining;
        self.epoch_progress_pct = if epoch_length > 0 {
            (slot_in_epoch as f64 / epoch_length as f64) * 100.0
        } else {
            0.0
        };
    }

    /// Determine epoch length for a snapshot (override > metric > network default > 432,000).
    fn epoch_length_for_snapshot(&self, snapshot: &MetricsSnapshot) -> u64 {
        if self.epoch_length_override > 0 {
            return self.epoch_length_override;
        }
        let metric = snapshot.get_u64("torsten_epoch_length");
        if metric > 0 {
            return metric;
        }
        let magic = snapshot.get_u64("torsten_network_magic");
        if magic > 0 {
            let n = Network::from_magic(magic);
            return n.epoch_length();
        }
        432_000
    }

    /// Get the effective epoch length using current state.
    pub fn epoch_length(&self) -> u64 {
        if self.epoch_length_override > 0 {
            return self.epoch_length_override;
        }
        let metric = self.metrics.get_u64("torsten_epoch_length");
        if metric > 0 {
            return metric;
        }
        self.network.epoch_length()
    }

    /// Get the current theme.
    pub fn theme(&self) -> &'static crate::theme::Theme {
        &THEMES[self.theme_idx]
    }

    /// Cycle to the next theme.
    pub fn cycle_theme(&mut self) {
        self.theme_idx = cycle_theme(self.theme_idx);
    }

    /// Toggle the help overlay.
    pub fn toggle_help(&mut self) {
        self.show_help = !self.show_help;
    }

    /// Determine whether the node is a block producer (metric 1.0) or relay (0.0).
    pub fn is_block_producer(&self) -> bool {
        self.metrics.get("torsten_is_block_producer") >= 1.0
    }

    /// Determine current sync status.
    ///
    /// Returns (label, is_synced, is_stalled).
    pub fn sync_status(&self) -> (&'static str, bool, bool) {
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

    /// Sync progress as a percentage (0.0–100.0).
    pub fn sync_progress_pct(&self) -> f64 {
        self.metrics.get("torsten_sync_progress_percent") / 100.0
    }

    /// Infer current era from the protocol major version metric.
    ///
    /// Protocol versions:
    ///   0-1: Byron, 2-3: Shelley, 4: Allegra, 5: Mary, 6: Alonzo, 7: Babbage, 8+: Conway
    pub fn current_era(&self) -> &'static str {
        let major = self.metrics.get_u64("torsten_protocol_major_version");
        // If the metric isn't exposed yet, fall back to epoch-based inference.
        if major == 0 {
            let epoch = self.metrics.get_u64("torsten_epoch_number");
            return match self.network {
                Network::Mainnet => {
                    if epoch >= 394 {
                        "Conway"
                    } else if epoch >= 365 {
                        "Babbage"
                    } else {
                        "Alonzo"
                    }
                }
                Network::Preview => {
                    if epoch >= 670 {
                        "Conway"
                    } else {
                        "Babbage"
                    }
                }
                Network::Preprod => {
                    if epoch >= 160 {
                        "Conway"
                    } else {
                        "Babbage"
                    }
                }
                _ => "Conway",
            };
        }
        match major {
            0 | 1 => "Byron",
            2 | 3 => "Shelley",
            4 => "Allegra",
            5 => "Mary",
            6 => "Alonzo",
            7 => "Babbage",
            _ => "Conway",
        }
    }

    // ---- Formatting helpers ----

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

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_snapshot(values: Vec<(&str, f64)>) -> MetricsSnapshot {
        MetricsSnapshot {
            values: values
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
            histogram_buckets: HashMap::new(),
            connected: true,
            error: None,
        }
    }

    #[test]
    fn test_network_from_magic() {
        assert_eq!(Network::from_magic(764_824_073), Network::Mainnet);
        assert_eq!(Network::from_magic(2), Network::Preview);
        assert_eq!(Network::from_magic(1), Network::Preprod);
        assert_eq!(Network::from_magic(0), Network::Unknown);
    }

    #[test]
    fn test_network_epoch_length() {
        assert_eq!(Network::Preview.epoch_length(), 86_400);
        assert_eq!(Network::Mainnet.epoch_length(), 432_000);
        assert_eq!(Network::Preprod.epoch_length(), 432_000);
    }

    #[test]
    fn test_epoch_progress_update() {
        let mut app = App::new();
        app.update_metrics(make_snapshot(vec![
            ("torsten_slot_number", 43_200.0),
            ("torsten_network_magic", 2.0), // Preview: 86,400 slots/epoch
        ]));
        assert_eq!(app.slot_in_epoch, 43_200);
        assert!((app.epoch_progress_pct - 50.0).abs() < 0.1);
    }

    #[test]
    fn test_epoch_length_override() {
        let mut app = App::new();
        app.epoch_length_override = 86_400;
        app.update_metrics(make_snapshot(vec![("torsten_slot_number", 43_200.0)]));
        assert_eq!(app.epoch_length(), 86_400);
    }

    #[test]
    fn test_is_block_producer() {
        let mut app = App::new();
        app.metrics = make_snapshot(vec![("torsten_is_block_producer", 1.0)]);
        assert!(app.is_block_producer());

        app.metrics = make_snapshot(vec![("torsten_is_block_producer", 0.0)]);
        assert!(!app.is_block_producer());
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
    fn test_theme_cycling() {
        let mut app = App::new();
        // Default theme is Monokai (looked up by name, not hardcoded index).
        assert_eq!(app.theme().name, "Monokai");
        let start_idx = app.theme_idx;
        app.cycle_theme();
        assert_ne!(app.theme_idx, start_idx);
        // Cycle through all themes and verify we return to the starting theme.
        let steps_remaining = crate::theme::THEMES.len() - 1;
        for _ in 0..steps_remaining {
            app.cycle_theme();
        }
        assert_eq!(
            app.theme_idx, start_idx,
            "cycling all themes should return to start"
        );
    }

    #[test]
    fn test_format_number() {
        assert_eq!(App::format_number(4_109_330), "4,109,330");
        assert_eq!(App::format_number(0), "0");
        assert_eq!(App::format_number(999), "999");
        assert_eq!(App::format_number(1_000), "1,000");
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(App::format_bytes(6_076_211_200), "5.7 GB");
        assert_eq!(App::format_bytes(1_048_576), "1.0 MB");
        assert_eq!(App::format_bytes(1024), "1.0 KB");
        assert_eq!(App::format_bytes(500), "500 B");
    }

    #[test]
    fn test_format_uptime() {
        assert_eq!(App::format_uptime(90061), "1d 1h 1m");
        assert_eq!(App::format_uptime(3661), "1h 1m");
        assert_eq!(App::format_uptime(61), "1m 1s");
    }

    #[test]
    fn test_current_era_from_protocol_version() {
        let mut app = App::new();
        app.metrics = make_snapshot(vec![("torsten_protocol_major_version", 9.0)]);
        assert_eq!(app.current_era(), "Conway");

        app.metrics = make_snapshot(vec![("torsten_protocol_major_version", 7.0)]);
        assert_eq!(app.current_era(), "Babbage");

        app.metrics = make_snapshot(vec![("torsten_protocol_major_version", 6.0)]);
        assert_eq!(app.current_era(), "Alonzo");
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
}
