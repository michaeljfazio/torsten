use std::path::Path;
use std::sync::Arc;

use fs2::available_space;
use tokio::sync::watch;
use tracing::{error, info, warn};

use crate::metrics::NodeMetrics;

/// Disk space warning thresholds
const WARNING_BYTES: u64 = 10 * 1024 * 1024 * 1024; // 10 GB
const CRITICAL_BYTES: u64 = 2 * 1024 * 1024 * 1024; // 2 GB
const FATAL_BYTES: u64 = 500 * 1024 * 1024; // 500 MB

/// How often to check disk space (in seconds)
const CHECK_INTERVAL_SECS: u64 = 60;

/// Disk space severity levels
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskSpaceLevel {
    /// Plenty of space available
    Ok,
    /// Below 10 GB — operator should investigate
    Warning,
    /// Below 2 GB — node may soon be unable to store blocks
    Critical,
    /// Below 500 MB — node should refuse new blocks to protect data integrity
    Fatal,
}

impl std::fmt::Display for DiskSpaceLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiskSpaceLevel::Ok => write!(f, "ok"),
            DiskSpaceLevel::Warning => write!(f, "warning"),
            DiskSpaceLevel::Critical => write!(f, "critical"),
            DiskSpaceLevel::Fatal => write!(f, "fatal"),
        }
    }
}

/// Returns the available disk space in bytes for the filesystem containing `path`.
pub fn check_disk_space(path: &Path) -> std::io::Result<u64> {
    available_space(path)
}

/// Classify available bytes into a severity level.
pub fn classify_disk_space(available_bytes: u64) -> DiskSpaceLevel {
    if available_bytes < FATAL_BYTES {
        DiskSpaceLevel::Fatal
    } else if available_bytes < CRITICAL_BYTES {
        DiskSpaceLevel::Critical
    } else if available_bytes < WARNING_BYTES {
        DiskSpaceLevel::Warning
    } else {
        DiskSpaceLevel::Ok
    }
}

/// Format bytes as a human-readable string (e.g. "12.34 GB").
fn format_bytes(bytes: u64) -> String {
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else {
        format!("{:.2} MB", b / MB)
    }
}

/// Spawn a background task that periodically checks disk space on the database volume,
/// logs warnings at appropriate severity levels, and updates the Prometheus metric.
pub async fn start_disk_monitor(
    database_path: std::path::PathBuf,
    metrics: Arc<NodeMetrics>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(CHECK_INTERVAL_SECS));

    // Do the first check immediately
    loop {
        tokio::select! {
            _ = interval.tick() => {}
            _ = shutdown_rx.changed() => {
                info!("Disk monitor shutting down");
                return;
            }
        }

        match check_disk_space(&database_path) {
            Ok(available) => {
                metrics.set_disk_available_bytes(available);
                let level = classify_disk_space(available);
                let human = format_bytes(available);

                match level {
                    DiskSpaceLevel::Fatal => {
                        error!(
                            available_bytes = available,
                            "FATAL: Disk space critically low ({human}) — \
                             node should stop accepting new blocks to protect data integrity"
                        );
                    }
                    DiskSpaceLevel::Critical => {
                        error!(
                            available_bytes = available,
                            "CRITICAL: Disk space very low ({human}) — \
                             node may soon be unable to store blocks"
                        );
                    }
                    DiskSpaceLevel::Warning => {
                        warn!(
                            available_bytes = available,
                            "Disk space low ({human}) — consider freeing space or expanding volume"
                        );
                    }
                    DiskSpaceLevel::Ok => {
                        // Only log at debug level when things are healthy
                        tracing::debug!(
                            available_bytes = available,
                            "Disk space check: {human} available"
                        );
                    }
                }
            }
            Err(e) => {
                error!(
                    "Failed to check disk space on {}: {e}",
                    database_path.display()
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn test_check_disk_space_returns_reasonable_value() {
        // Check disk space on the current directory — should always succeed and
        // return a positive value on any system with a working filesystem.
        let available = check_disk_space(Path::new(".")).expect("check_disk_space should succeed");
        // Any modern OS should have at least 1 MB free on the root filesystem
        assert!(
            available > 1024 * 1024,
            "expected at least 1 MB free, got {available} bytes"
        );
    }

    #[test]
    fn test_check_disk_space_nonexistent_path() {
        let result = check_disk_space(Path::new("/nonexistent/path/that/does/not/exist"));
        assert!(result.is_err(), "should fail for nonexistent path");
    }

    #[test]
    fn test_classify_disk_space_ok() {
        // 20 GB — well above all thresholds
        let level = classify_disk_space(20 * 1024 * 1024 * 1024);
        assert_eq!(level, DiskSpaceLevel::Ok);
    }

    #[test]
    fn test_classify_disk_space_warning() {
        // 5 GB — below warning (10 GB), above critical (2 GB)
        let level = classify_disk_space(5 * 1024 * 1024 * 1024);
        assert_eq!(level, DiskSpaceLevel::Warning);
    }

    #[test]
    fn test_classify_disk_space_critical() {
        // 1 GB — below critical (2 GB), above fatal (500 MB)
        let level = classify_disk_space(1024 * 1024 * 1024);
        assert_eq!(level, DiskSpaceLevel::Critical);
    }

    #[test]
    fn test_classify_disk_space_fatal() {
        // 100 MB — below fatal (500 MB)
        let level = classify_disk_space(100 * 1024 * 1024);
        assert_eq!(level, DiskSpaceLevel::Fatal);
    }

    #[test]
    fn test_classify_disk_space_zero() {
        let level = classify_disk_space(0);
        assert_eq!(level, DiskSpaceLevel::Fatal);
    }

    #[test]
    fn test_classify_disk_space_boundary_warning() {
        // Exactly at the warning threshold — should be warning (strictly less than)
        let level = classify_disk_space(WARNING_BYTES);
        assert_eq!(level, DiskSpaceLevel::Ok);

        let level = classify_disk_space(WARNING_BYTES - 1);
        assert_eq!(level, DiskSpaceLevel::Warning);
    }

    #[test]
    fn test_classify_disk_space_boundary_critical() {
        let level = classify_disk_space(CRITICAL_BYTES);
        assert_eq!(level, DiskSpaceLevel::Warning);

        let level = classify_disk_space(CRITICAL_BYTES - 1);
        assert_eq!(level, DiskSpaceLevel::Critical);
    }

    #[test]
    fn test_classify_disk_space_boundary_fatal() {
        let level = classify_disk_space(FATAL_BYTES);
        assert_eq!(level, DiskSpaceLevel::Critical);

        let level = classify_disk_space(FATAL_BYTES - 1);
        assert_eq!(level, DiskSpaceLevel::Fatal);
    }

    #[test]
    fn test_format_bytes_gb() {
        let s = format_bytes(10 * 1024 * 1024 * 1024);
        assert_eq!(s, "10.00 GB");
    }

    #[test]
    fn test_format_bytes_mb() {
        let s = format_bytes(512 * 1024 * 1024);
        assert_eq!(s, "512.00 MB");
    }

    #[test]
    fn test_disk_space_level_display() {
        assert_eq!(DiskSpaceLevel::Ok.to_string(), "ok");
        assert_eq!(DiskSpaceLevel::Warning.to_string(), "warning");
        assert_eq!(DiskSpaceLevel::Critical.to_string(), "critical");
        assert_eq!(DiskSpaceLevel::Fatal.to_string(), "fatal");
    }

    #[test]
    fn test_metrics_integration() {
        let metrics = NodeMetrics::new();
        assert_eq!(metrics.disk_available_bytes.load(Ordering::Relaxed), 0);

        metrics.set_disk_available_bytes(42_000_000_000);
        assert_eq!(
            metrics.disk_available_bytes.load(Ordering::Relaxed),
            42_000_000_000
        );

        let output = metrics.to_prometheus();
        assert!(output.contains("torsten_disk_available_bytes 42000000000"));
        assert!(output.contains("# TYPE torsten_disk_available_bytes gauge"));
    }
}
