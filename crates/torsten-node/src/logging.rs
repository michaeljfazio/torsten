use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer, Registry};

/// Log output target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogOutput {
    Stdout,
    File,
    Journald,
}

impl std::str::FromStr for LogOutput {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "stdout" => Ok(Self::Stdout),
            "file" => Ok(Self::File),
            "journald" | "journal" | "systemd" => Ok(Self::Journald),
            other => Err(format!(
                "unknown log output '{other}' (valid: stdout, file, journald)"
            )),
        }
    }
}

/// Log output format.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LogFormat {
    /// Human-readable text output
    #[default]
    Text,
    /// Structured JSON output (one JSON object per line)
    Json,
}

impl std::str::FromStr for LogFormat {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "text" | "plain" => Ok(Self::Text),
            "json" => Ok(Self::Json),
            other => Err(format!("unknown log format '{other}' (valid: text, json)")),
        }
    }
}

/// Log file rotation strategy.
#[derive(Debug, Clone, Copy, Default)]
pub enum LogRotation {
    #[default]
    Daily,
    Hourly,
    Never,
}

impl std::str::FromStr for LogRotation {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "daily" => Ok(Self::Daily),
            "hourly" => Ok(Self::Hourly),
            "never" | "none" => Ok(Self::Never),
            other => Err(format!(
                "unknown rotation '{other}' (valid: daily, hourly, never)"
            )),
        }
    }
}

/// Options for initializing the logging system.
pub struct LoggingOpts {
    pub outputs: Vec<LogOutput>,
    pub format: LogFormat,
    pub level: String,
    pub log_dir: String,
    pub rotation: LogRotation,
    pub no_color: bool,
    /// Number of days to retain log files (default: 7). Files older than this are deleted.
    /// Used by [`start_log_cleanup_task`] when the caller passes this value.
    #[allow(dead_code)]
    pub log_retention_days: u64,
}

/// Guard that must be held for the lifetime of the program.
/// Dropping this flushes any buffered log output (e.g., file writer).
pub struct LogGuard {
    _guards: Vec<tracing_appender::non_blocking::WorkerGuard>,
}

/// Initialize the logging system with the given options.
///
/// Returns a [`LogGuard`] that must be held until program exit to ensure
/// buffered output (file logs) is flushed.
pub fn init(opts: &LoggingOpts) -> anyhow::Result<LogGuard> {
    let mut guards: Vec<tracing_appender::non_blocking::WorkerGuard> = Vec::new();
    let mut layers: Vec<Box<dyn Layer<Registry> + Send + Sync>> = Vec::new();

    let outputs = if opts.outputs.is_empty() {
        vec![LogOutput::Stdout]
    } else {
        opts.outputs.clone()
    };

    for output in &outputs {
        match output {
            LogOutput::Stdout => {
                let ansi = !opts.no_color && atty_stdout();
                let filter = build_filter(&opts.level);
                match opts.format {
                    LogFormat::Text => {
                        let layer = tracing_subscriber::fmt::layer()
                            .compact()
                            .with_target(true)
                            .with_ansi(ansi)
                            .with_filter(filter);
                        layers.push(Box::new(layer));
                    }
                    LogFormat::Json => {
                        let layer = tracing_subscriber::fmt::layer()
                            .json()
                            .with_target(true)
                            .with_ansi(false)
                            .with_filter(filter);
                        layers.push(Box::new(layer));
                    }
                }
            }
            LogOutput::File => {
                std::fs::create_dir_all(&opts.log_dir)?;
                let file_appender = match opts.rotation {
                    LogRotation::Daily => {
                        tracing_appender::rolling::daily(&opts.log_dir, "torsten.log")
                    }
                    LogRotation::Hourly => {
                        tracing_appender::rolling::hourly(&opts.log_dir, "torsten.log")
                    }
                    LogRotation::Never => {
                        tracing_appender::rolling::never(&opts.log_dir, "torsten.log")
                    }
                };
                let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
                guards.push(guard);

                let filter = build_filter(&opts.level);
                match opts.format {
                    LogFormat::Text => {
                        let layer = tracing_subscriber::fmt::layer()
                            .compact()
                            .with_target(true)
                            .with_ansi(false)
                            .with_writer(non_blocking)
                            .with_filter(filter);
                        layers.push(Box::new(layer));
                    }
                    LogFormat::Json => {
                        let layer = tracing_subscriber::fmt::layer()
                            .json()
                            .with_target(true)
                            .with_ansi(false)
                            .with_writer(non_blocking)
                            .with_filter(filter);
                        layers.push(Box::new(layer));
                    }
                }
            }
            LogOutput::Journald => {
                #[cfg(feature = "journald")]
                {
                    let layer = tracing_journald::layer()
                        .map_err(|e| anyhow::anyhow!("Failed to connect to journald: {e}"))?
                        .with_filter(build_filter(&opts.level));
                    layers.push(Box::new(layer));
                }
                #[cfg(not(feature = "journald"))]
                {
                    anyhow::bail!(
                        "journald output requires the 'journald' feature (rebuild with --features journald)"
                    );
                }
            }
        }
    }

    Registry::default().with(layers).init();

    Ok(LogGuard { _guards: guards })
}

/// Delete `.log` files in `log_dir` that are older than `retention_days`.
///
/// Scans only immediate children of the directory (not recursive). Files that
/// cannot be inspected (e.g. permission errors) are silently skipped.
#[allow(dead_code)]
pub fn cleanup_old_logs(log_dir: &std::path::Path, retention_days: u64) {
    let cutoff =
        std::time::SystemTime::now() - std::time::Duration::from_secs(retention_days * 86400);

    let entries = match std::fs::read_dir(log_dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        // Only consider files with .log extension
        if path.extension().and_then(|e| e.to_str()) != Some("log") {
            continue;
        }
        let modified = match entry.metadata().and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(_) => continue,
        };
        if modified < cutoff {
            if let Err(e) = std::fs::remove_file(&path) {
                tracing::warn!(path = %path.display(), "Failed to remove old log file: {e}");
            } else {
                tracing::info!(path = %path.display(), "Removed old log file");
            }
        }
    }
}

/// Spawn a background task that periodically cleans up old log files.
/// Runs every hour alongside the disk monitor.
#[allow(dead_code)]
pub async fn start_log_cleanup_task(
    log_dir: std::path::PathBuf,
    retention_days: u64,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));

    loop {
        tokio::select! {
            _ = interval.tick() => {}
            _ = shutdown_rx.changed() => {
                tracing::debug!("Log cleanup task shutting down");
                return;
            }
        }
        cleanup_old_logs(&log_dir, retention_days);
    }
}

/// Build an `EnvFilter` from the given level string.
/// `RUST_LOG` env var takes priority if set.
fn build_filter(level: &str) -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level))
}

/// Check if stdout is a terminal (for auto-detecting color support).
fn atty_stdout() -> bool {
    std::io::IsTerminal::is_terminal(&std::io::stdout())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_output_from_str() {
        assert_eq!("stdout".parse::<LogOutput>().unwrap(), LogOutput::Stdout);
        assert_eq!("file".parse::<LogOutput>().unwrap(), LogOutput::File);
        assert_eq!(
            "journald".parse::<LogOutput>().unwrap(),
            LogOutput::Journald
        );
        assert_eq!("journal".parse::<LogOutput>().unwrap(), LogOutput::Journald);
        assert_eq!("systemd".parse::<LogOutput>().unwrap(), LogOutput::Journald);
        assert_eq!("STDOUT".parse::<LogOutput>().unwrap(), LogOutput::Stdout);
        assert!("invalid".parse::<LogOutput>().is_err());
    }

    #[test]
    fn test_log_format_from_str() {
        assert_eq!("text".parse::<LogFormat>().unwrap(), LogFormat::Text);
        assert_eq!("plain".parse::<LogFormat>().unwrap(), LogFormat::Text);
        assert_eq!("json".parse::<LogFormat>().unwrap(), LogFormat::Json);
        assert_eq!("JSON".parse::<LogFormat>().unwrap(), LogFormat::Json);
        assert!("invalid".parse::<LogFormat>().is_err());
    }

    #[test]
    fn test_log_rotation_from_str() {
        assert!(matches!(
            "daily".parse::<LogRotation>().unwrap(),
            LogRotation::Daily
        ));
        assert!(matches!(
            "hourly".parse::<LogRotation>().unwrap(),
            LogRotation::Hourly
        ));
        assert!(matches!(
            "never".parse::<LogRotation>().unwrap(),
            LogRotation::Never
        ));
        assert!(matches!(
            "none".parse::<LogRotation>().unwrap(),
            LogRotation::Never
        ));
        assert!("invalid".parse::<LogRotation>().is_err());
    }

    #[test]
    fn test_cleanup_old_logs_removes_expired() {
        let dir = std::env::temp_dir().join("torsten_log_cleanup_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Create a .log file and backdate its modification time
        let old_file = dir.join("torsten.2020-01-01.log");
        std::fs::write(&old_file, "old log data").unwrap();
        // Set modification time to 30 days ago.
        // We must drop the file handle before cleanup_old_logs reads metadata,
        // otherwise some platforms may report a stale mtime for the open file.
        let old_time = std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 86400);
        {
            let f = std::fs::File::options()
                .write(true)
                .open(&old_file)
                .unwrap();
            f.set_modified(old_time).unwrap();
            drop(f);
        }
        // Verify the mtime was actually persisted before proceeding.
        let actual_mtime = std::fs::metadata(&old_file).unwrap().modified().unwrap();
        assert!(
            actual_mtime
                < std::time::SystemTime::now() - std::time::Duration::from_secs(29 * 86400),
            "Filesystem did not persist backdated mtime"
        );

        // Create a recent .log file
        let new_file = dir.join("torsten.2026-03-14.log");
        std::fs::write(&new_file, "new log data").unwrap();

        // Create a non-log file (should not be deleted)
        let txt_file = dir.join("notes.txt");
        std::fs::write(&txt_file, "keep me").unwrap();

        cleanup_old_logs(&dir, 7);

        assert!(!old_file.exists(), "Old log file should have been deleted");
        assert!(new_file.exists(), "Recent log file should be kept");
        assert!(txt_file.exists(), "Non-log file should not be touched");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_cleanup_old_logs_nonexistent_dir() {
        // Should not panic on non-existent directory
        cleanup_old_logs(std::path::Path::new("/nonexistent/dir"), 7);
    }
}
