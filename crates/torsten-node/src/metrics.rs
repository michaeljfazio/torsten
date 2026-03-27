use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tracing::{error, info};

/// Duration in seconds after which the node is considered "stalled" if no blocks received
/// and sync progress is below 99%.
const STALLED_THRESHOLD_SECS: u64 = 300; // 5 minutes

/// Tracks process CPU utilization between samples.
///
/// Computes CPU percentage by comparing cumulative process CPU time (user +
/// kernel) across two wall-clock samples.  The percentage is relative to one
/// logical CPU core, so values > 100 are possible on multi-threaded workloads.
///
/// Platform notes:
/// - Linux: reads `/proc/self/stat` fields 14 (utime) and 15 (stime) in clock
///   ticks, then divides by the tick frequency obtained from `libc::sysconf`.
/// - macOS: shell out to `ps -o pcpu= -p <pid>` which reports instantaneous
///   CPU% directly — cheap enough for a 5-second polling window.
/// - Other platforms: returns 0.0 (no-op, no external dependencies needed).
struct CpuTracker {
    /// Wall-clock time of the previous sample.
    last_wall: std::time::Instant,
    /// Cumulative CPU ticks (utime + stime) at the previous sample.
    /// Stored as 0 on non-Linux platforms (unused).
    last_cpu_ticks: u64,
    /// CPU percentage from the most recent interval (updated on each `sample()`).
    last_pct: f64,
    /// Cumulative CPU seconds (utime + stime / CLK_TCK) updated on each sample.
    /// Used to expose `torsten_cpu_seconds_total`.
    cumulative_cpu_secs: f64,
}

impl CpuTracker {
    fn new() -> Self {
        Self {
            last_wall: std::time::Instant::now(),
            last_cpu_ticks: read_cpu_ticks_linux(),
            last_pct: 0.0,
            cumulative_cpu_secs: 0.0,
        }
    }

    /// Sample current CPU usage.
    ///
    /// Returns the CPU percentage consumed since the previous call (0.0–100.0+
    /// per core).  Also updates `self.cumulative_cpu_secs`.
    fn sample(&mut self) -> f64 {
        let pct = sample_cpu_pct_impl(
            &mut self.last_wall,
            &mut self.last_cpu_ticks,
            &mut self.cumulative_cpu_secs,
        );
        self.last_pct = pct;
        pct
    }
}

// ---------------------------------------------------------------------------
// Linux implementation — /proc/self/stat
// ---------------------------------------------------------------------------

/// Read the sum of `utime + stime` (in clock ticks) from `/proc/self/stat`.
/// Returns 0 on non-Linux platforms or if the file cannot be parsed.
#[cfg(target_os = "linux")]
fn read_cpu_ticks_linux() -> u64 {
    // /proc/self/stat is a single space-separated line; fields are 1-indexed
    // in the proc(5) man page.  Fields 14 (utime) and 15 (stime) are the
    // user-mode and kernel-mode CPU times in clock ticks.
    //
    // Field 2 (comm) can contain spaces inside parentheses, so we locate the
    // closing ')' and split from there to get the remaining positional fields.
    std::fs::read_to_string("/proc/self/stat")
        .ok()
        .and_then(|s| {
            // Skip past the closing ')' of the comm field.
            let after_comm = s.find(')')? + 1;
            let rest = s[after_comm..].trim_start();
            // Remaining fields are whitespace-separated; 0-indexed from here:
            //   0 = state (field 3)
            //   ...
            //  11 = utime (field 14)
            //  12 = stime (field 15)
            let fields: Vec<&str> = rest.split_whitespace().collect();
            let utime: u64 = fields.get(11)?.parse().ok()?;
            let stime: u64 = fields.get(12)?.parse().ok()?;
            Some(utime + stime)
        })
        .unwrap_or(0)
}

#[cfg(not(target_os = "linux"))]
fn read_cpu_ticks_linux() -> u64 {
    0
}

/// Return the number of clock ticks per second (`_SC_CLK_TCK`).
///
/// 100 is the correct value on virtually all Linux systems but we read the
/// actual kernel-reported value to be accurate.  The result is cached after
/// the first call via a `std::sync::OnceLock`.
#[cfg(target_os = "linux")]
fn clk_tck() -> u64 {
    static CLK_TCK: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *CLK_TCK.get_or_init(|| {
        // SAFETY: sysconf is always safe to call with _SC_CLK_TCK.
        let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        if ticks > 0 {
            ticks as u64
        } else {
            100
        }
    })
}

// ---------------------------------------------------------------------------
// Platform-dispatched sampling
// ---------------------------------------------------------------------------

/// Compute the CPU percentage consumed since the last call and update
/// the cumulative CPU seconds counter.
///
/// On Linux: delta_ticks / clk_tck / elapsed_wall_secs * 100.
/// On macOS: one `ps -o pcpu=` shell-out per call.
/// Elsewhere: 0.0 always.
#[cfg(target_os = "linux")]
fn sample_cpu_pct_impl(
    last_wall: &mut std::time::Instant,
    last_ticks: &mut u64,
    cumulative_secs: &mut f64,
) -> f64 {
    let now_wall = std::time::Instant::now();
    let elapsed_wall = now_wall.duration_since(*last_wall).as_secs_f64();

    let current_ticks = read_cpu_ticks_linux();
    let tck = clk_tck();

    // Guard against clock going backwards or zero elapsed time.
    if elapsed_wall < 0.001 || tck == 0 {
        return 0.0;
    }

    let delta_ticks = current_ticks.saturating_sub(*last_ticks);
    let delta_cpu_secs = delta_ticks as f64 / tck as f64;

    *cumulative_secs += delta_cpu_secs;
    *last_wall = now_wall;
    *last_ticks = current_ticks;

    // Clamp to a sane ceiling (400% = 4 fully-loaded cores).
    (delta_cpu_secs / elapsed_wall * 100.0).clamp(0.0, 400.0)
}

#[cfg(target_os = "macos")]
fn sample_cpu_pct_impl(
    last_wall: &mut std::time::Instant,
    _last_ticks: &mut u64,
    cumulative_secs: &mut f64,
) -> f64 {
    // `ps -o pcpu=` emits the CPU% since process start (not since last call),
    // which is what we want for the gauge display.  The shell-out takes ~5 ms
    // on macOS; acceptable for a monitoring interval of >= 2 seconds.
    let pct = std::process::Command::new("ps")
        .args(["-o", "pcpu=", "-p", &std::process::id().to_string()])
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .and_then(|s| s.trim().parse::<f64>().ok())
        .unwrap_or(0.0);

    // Update cumulative seconds from the elapsed wall time and the current %.
    let now_wall = std::time::Instant::now();
    let elapsed_wall = now_wall.duration_since(*last_wall).as_secs_f64();
    *last_wall = now_wall;
    // Approximate: pct is since-start average, so delta = pct/100 * elapsed_wall.
    *cumulative_secs += (pct / 100.0) * elapsed_wall;

    pct.clamp(0.0, 400.0)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn sample_cpu_pct_impl(
    _last_wall: &mut std::time::Instant,
    _last_ticks: &mut u64,
    _cumulative_secs: &mut f64,
) -> f64 {
    0.0
}

/// Sync progress threshold (as percentage * 100) at or above which the node is "healthy".
const SYNCED_THRESHOLD: u64 = 9990; // 99.9% (stored as pct * 100)

/// Fixed histogram bucket boundaries (in milliseconds) for latency tracking.
const LATENCY_BUCKETS_MS: &[f64] = &[
    1.0, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1000.0, 2500.0, 5000.0, 10000.0,
];

/// Prometheus-style histogram with fixed buckets.
#[derive(Debug)]
pub struct Histogram {
    /// Count of observations in each bucket (cumulative upper bound).
    buckets: Vec<AtomicU64>,
    /// Total count of observations.
    count: AtomicU64,
    /// Sum of all observed values (stored as f64 bits for atomicity).
    sum_bits: AtomicU64,
}

impl Histogram {
    fn new() -> Self {
        Histogram {
            buckets: (0..LATENCY_BUCKETS_MS.len())
                .map(|_| AtomicU64::new(0))
                .collect(),
            count: AtomicU64::new(0),
            sum_bits: AtomicU64::new(f64::to_bits(0.0)),
        }
    }

    /// Record an observation (value in milliseconds).
    /// Increments the first bucket whose upper bound >= value_ms.
    pub fn observe(&self, value_ms: f64) {
        for (i, &bound) in LATENCY_BUCKETS_MS.iter().enumerate() {
            if value_ms <= bound {
                self.buckets[i].fetch_add(1, Ordering::Relaxed);
                break;
            }
        }
        self.count.fetch_add(1, Ordering::Relaxed);
        // Approximate sum update — relaxed ordering is fine for metrics
        loop {
            let old_bits = self.sum_bits.load(Ordering::Relaxed);
            let old_sum = f64::from_bits(old_bits);
            let new_sum = old_sum + value_ms;
            if self
                .sum_bits
                .compare_exchange_weak(
                    old_bits,
                    f64::to_bits(new_sum),
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                break;
            }
        }
    }

    /// Format as Prometheus histogram lines.
    fn to_prometheus(&self, name: &str, help: &str) -> String {
        let mut out = String::new();
        out.push_str(&format!("# HELP {name} {help}\n# TYPE {name} histogram\n"));
        let mut cumulative = 0u64;
        for (i, &bound) in LATENCY_BUCKETS_MS.iter().enumerate() {
            cumulative += self.buckets[i].load(Ordering::Relaxed);
            out.push_str(&format!("{name}_bucket{{le=\"{bound}\"}} {cumulative}\n"));
        }
        let total = self.count.load(Ordering::Relaxed);
        out.push_str(&format!("{name}_bucket{{le=\"+Inf\"}} {total}\n"));
        let sum = f64::from_bits(self.sum_bits.load(Ordering::Relaxed));
        out.push_str(&format!("{name}_sum {sum}\n"));
        out.push_str(&format!("{name}_count {total}\n"));
        out
    }
}

/// Get the current resident set size (RSS) of this process in bytes.
fn get_resident_memory_bytes() -> u64 {
    get_resident_memory_bytes_impl()
}

#[cfg(target_os = "linux")]
fn get_resident_memory_bytes_impl() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmRSS:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse::<u64>().ok())
        })
        .map(|kb| kb * 1024)
        .unwrap_or(0)
}

#[cfg(target_os = "macos")]
fn get_resident_memory_bytes_impl() -> u64 {
    std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(|kb| kb * 1024)
        .unwrap_or(0)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn get_resident_memory_bytes_impl() -> u64 {
    0
}

/// Get total system physical memory in bytes.
///
/// Used to compute the process RSS fraction for the TUI memory bar.
/// Returns 0 on unsupported platforms (bar will be hidden).
fn get_total_memory_bytes() -> u64 {
    get_total_memory_bytes_impl()
}

#[cfg(target_os = "linux")]
fn get_total_memory_bytes_impl() -> u64 {
    // /proc/meminfo line: "MemTotal:       16384000 kB"
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("MemTotal:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse::<u64>().ok())
        })
        .map(|kb| kb * 1024)
        .unwrap_or(0)
}

#[cfg(target_os = "macos")]
fn get_total_memory_bytes_impl() -> u64 {
    // sysctl hw.memsize returns total physical RAM as a string integer.
    std::process::Command::new("sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn get_total_memory_bytes_impl() -> u64 {
    0
}

/// Node metrics for monitoring
pub struct NodeMetrics {
    pub blocks_received: AtomicU64,
    pub blocks_applied: AtomicU64,
    pub transactions_received: AtomicU64,
    pub transactions_validated: AtomicU64,
    pub transactions_rejected: AtomicU64,
    pub peers_connected: AtomicU64,
    pub peers_outbound: AtomicU64,
    pub peers_inbound: AtomicU64,
    pub peers_duplex: AtomicU64,
    pub peers_cold: AtomicU64,
    pub peers_warm: AtomicU64,
    pub peers_hot: AtomicU64,
    // Connection manager counters (Haskell ConnectionManagerCounters compat)
    pub conn_full_duplex: AtomicU64,
    pub conn_duplex: AtomicU64,
    pub conn_unidirectional: AtomicU64,
    pub conn_inbound: AtomicU64,
    pub conn_outbound: AtomicU64,
    pub conn_terminating: AtomicU64,
    pub sync_progress_pct: AtomicU64,
    pub slot_number: AtomicU64,
    pub block_number: AtomicU64,
    pub epoch_number: AtomicU64,
    pub utxo_count: AtomicU64,
    pub mempool_tx_count: AtomicU64,
    pub mempool_tx_max: AtomicU64,
    pub mempool_bytes: AtomicU64,
    pub rollback_count: AtomicU64,
    pub blocks_forged: AtomicU64,
    pub delegation_count: AtomicU64,
    pub treasury_lovelace: AtomicU64,
    pub drep_count: AtomicU64,
    pub proposal_count: AtomicU64,
    pub pool_count: AtomicU64,
    pub disk_total_bytes: AtomicU64,
    pub disk_used_bytes: AtomicU64,
    pub disk_available_bytes: AtomicU64,
    // Block production metrics
    pub leader_checks_total: AtomicU64,
    pub leader_checks_not_elected: AtomicU64,
    pub forge_failures: AtomicU64,
    pub blocks_announced: AtomicU64,
    // Protocol error metrics
    pub n2n_connections_total: AtomicU64,
    pub n2c_connections_total: AtomicU64,
    pub n2n_connections_active: AtomicU64,
    pub n2c_connections_active: AtomicU64,
    // N2C LocalTxSubmission counters (from torsten-cli submit-tx)
    pub n2c_txs_submitted: AtomicU64,
    pub n2c_txs_accepted: AtomicU64,
    pub n2c_txs_rejected: AtomicU64,
    /// Per-protocol-error-type counts (label → count).
    protocol_errors: std::sync::Mutex<HashMap<String, u64>>,
    /// Peer handshake RTT histogram (milliseconds)
    pub peer_handshake_rtt_ms: Histogram,
    /// Block fetch latency histogram (milliseconds per block)
    pub peer_block_fetch_ms: Histogram,
    /// Node uptime in seconds
    startup_instant: std::time::Instant,
    /// Per-validation-error-type rejection counts (label → count).
    validation_errors: std::sync::Mutex<HashMap<String, u64>>,
    /// Epoch milliseconds when the last block was received (0 = never)
    pub last_block_received_at: AtomicU64,
    /// Epoch millis of last RollForward event (for chainsync_idle calculation)
    pub last_roll_forward_at: AtomicU64,
    /// Duration of last ledger replay in seconds (stored as f64 bits)
    pub replay_duration_secs: AtomicU64,
    /// Tip age in seconds (wall_clock - slot_to_time(tip_slot))
    pub tip_age_secs: AtomicU64,
    /// POSIX time of the tip slot in milliseconds (for dynamic tip_age computation).
    pub tip_slot_time_ms: AtomicU64,
    /// Seconds since last RollForward event
    pub chainsync_idle_secs: AtomicU64,
    /// CPU tracker for process CPU utilization measurement.
    cpu_tracker: std::sync::Mutex<CpuTracker>,
    /// Peak resident memory observed since node start, in bytes.
    /// Exposed as `torsten_mem_peak_bytes` Prometheus gauge.
    peak_mem_bytes: AtomicU64,
    /// Network magic number (764824073=mainnet, 2=preview, 1=preprod).
    pub network_magic: AtomicU64,
    /// 1 when running as a block producer (forge credentials loaded), 0 for relay.
    ///
    /// Exposed as `torsten_is_block_producer` gauge so the TUI can show the
    /// correct role label without inspecting CLI arguments directly.
    pub is_block_producer: AtomicU64,
    /// Hex-encoded pool ID (28-byte Blake2b-224 of the cold verification key).
    ///
    /// Empty string when running as a relay.  Emitted as a Prometheus info metric
    /// with a `pool_id` label so operators can identify the producing pool at a
    /// glance in the TUI without opening the logs.
    pool_id_hex: std::sync::Mutex<String>,
    /// When true, emit additional `cardano_node_metrics_*` aliases alongside the
    /// native `torsten_*` metrics.  Allows existing cardano-node Grafana dashboards
    /// to work without modification.  Controlled by `--compat-metrics` CLI flag.
    compat_metrics: std::sync::atomic::AtomicBool,
}

impl NodeMetrics {
    pub fn new() -> Self {
        NodeMetrics {
            blocks_received: AtomicU64::new(0),
            blocks_applied: AtomicU64::new(0),
            transactions_received: AtomicU64::new(0),
            transactions_validated: AtomicU64::new(0),
            transactions_rejected: AtomicU64::new(0),
            peers_connected: AtomicU64::new(0),
            peers_outbound: AtomicU64::new(0),
            peers_inbound: AtomicU64::new(0),
            peers_duplex: AtomicU64::new(0),
            peers_cold: AtomicU64::new(0),
            peers_warm: AtomicU64::new(0),
            peers_hot: AtomicU64::new(0),
            conn_full_duplex: AtomicU64::new(0),
            conn_duplex: AtomicU64::new(0),
            conn_unidirectional: AtomicU64::new(0),
            conn_inbound: AtomicU64::new(0),
            conn_outbound: AtomicU64::new(0),
            conn_terminating: AtomicU64::new(0),
            sync_progress_pct: AtomicU64::new(0),
            slot_number: AtomicU64::new(0),
            block_number: AtomicU64::new(0),
            epoch_number: AtomicU64::new(0),
            utxo_count: AtomicU64::new(0),
            mempool_tx_count: AtomicU64::new(0),
            mempool_tx_max: AtomicU64::new(0),
            mempool_bytes: AtomicU64::new(0),
            rollback_count: AtomicU64::new(0),
            blocks_forged: AtomicU64::new(0),
            delegation_count: AtomicU64::new(0),
            treasury_lovelace: AtomicU64::new(0),
            drep_count: AtomicU64::new(0),
            proposal_count: AtomicU64::new(0),
            pool_count: AtomicU64::new(0),
            disk_total_bytes: AtomicU64::new(0),
            disk_used_bytes: AtomicU64::new(0),
            disk_available_bytes: AtomicU64::new(0),
            leader_checks_total: AtomicU64::new(0),
            leader_checks_not_elected: AtomicU64::new(0),
            forge_failures: AtomicU64::new(0),
            blocks_announced: AtomicU64::new(0),
            n2n_connections_total: AtomicU64::new(0),
            n2c_connections_total: AtomicU64::new(0),
            n2n_connections_active: AtomicU64::new(0),
            n2c_connections_active: AtomicU64::new(0),
            n2c_txs_submitted: AtomicU64::new(0),
            n2c_txs_accepted: AtomicU64::new(0),
            n2c_txs_rejected: AtomicU64::new(0),
            protocol_errors: std::sync::Mutex::new(HashMap::new()),
            peer_handshake_rtt_ms: Histogram::new(),
            peer_block_fetch_ms: Histogram::new(),
            startup_instant: std::time::Instant::now(),
            validation_errors: std::sync::Mutex::new(HashMap::new()),
            last_block_received_at: AtomicU64::new(0),
            last_roll_forward_at: AtomicU64::new(0),
            replay_duration_secs: AtomicU64::new(0),
            tip_age_secs: AtomicU64::new(0),
            tip_slot_time_ms: AtomicU64::new(0),
            chainsync_idle_secs: AtomicU64::new(0),
            cpu_tracker: std::sync::Mutex::new(CpuTracker::new()),
            peak_mem_bytes: AtomicU64::new(0),
            network_magic: AtomicU64::new(0),
            is_block_producer: AtomicU64::new(0),
            pool_id_hex: std::sync::Mutex::new(String::new()),
            compat_metrics: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Enable or disable `cardano_node_metrics_*` compatibility aliases.
    ///
    /// When enabled, `to_prometheus()` emits a second set of metric lines using
    /// the naming convention used by cardano-node (Haskell), so existing Grafana
    /// dashboards built for cardano-node continue to work without modification.
    pub fn set_compat_metrics(&self, enabled: bool) {
        self.compat_metrics
            .store(enabled, std::sync::atomic::Ordering::Relaxed);
    }

    /// Record a transaction validation error by type.
    pub fn record_validation_error(&self, error_type: &str) {
        if let Ok(mut map) = self.validation_errors.lock() {
            *map.entry(error_type.to_string()).or_insert(0) += 1;
        }
    }

    /// Record a protocol-level error by label (e.g. "n2n_handshake_failed").
    #[allow(dead_code)] // used by networking rewrite
    pub fn record_protocol_error(&self, label: &str) {
        if let Ok(mut map) = self.protocol_errors.lock() {
            *map.entry(label.to_string()).or_insert(0) += 1;
        }
    }

    /// Record a peer handshake latency observation.
    pub fn record_handshake_rtt(&self, rtt_ms: f64) {
        self.peer_handshake_rtt_ms.observe(rtt_ms);
    }

    /// Record a per-block fetch latency observation.
    pub fn record_block_fetch_latency(&self, ms_per_block: f64) {
        self.peer_block_fetch_ms.observe(ms_per_block);
    }

    pub fn add_blocks_received(&self, count: u64) {
        self.blocks_received.fetch_add(count, Ordering::Relaxed);
    }

    pub fn add_blocks_applied(&self, count: u64) {
        self.blocks_applied.fetch_add(count, Ordering::Relaxed);
    }

    pub fn set_slot(&self, slot: u64) {
        self.slot_number.store(slot, Ordering::Relaxed);
    }

    pub fn set_block_number(&self, block_no: u64) {
        self.block_number.store(block_no, Ordering::Relaxed);
    }

    pub fn set_epoch(&self, epoch: u64) {
        self.epoch_number.store(epoch, Ordering::Relaxed);
    }

    pub fn set_sync_progress(&self, pct: f64) {
        self.sync_progress_pct
            .store((pct * 100.0) as u64, Ordering::Relaxed);
    }

    pub fn set_utxo_count(&self, count: u64) {
        self.utxo_count.store(count, Ordering::Relaxed);
    }

    pub fn set_mempool_count(&self, count: u64) {
        self.mempool_tx_count.store(count, Ordering::Relaxed);
    }

    pub fn set_mempool_max(&self, max: u64) {
        self.mempool_tx_max.store(max, Ordering::Relaxed);
    }

    pub fn set_disk_available_bytes(&self, bytes: u64) {
        self.disk_available_bytes.store(bytes, Ordering::Relaxed);
    }

    pub fn set_disk_total_bytes(&self, bytes: u64) {
        self.disk_total_bytes.store(bytes, Ordering::Relaxed);
    }

    pub fn set_disk_used_bytes(&self, bytes: u64) {
        self.disk_used_bytes.store(bytes, Ordering::Relaxed);
    }

    /// Record that a block was just received (updates timestamp to now).
    pub fn record_block_received(&self) {
        let now_millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.last_block_received_at
            .store(now_millis, Ordering::Relaxed);
    }

    /// Returns the node health status: "healthy", "syncing", or "stalled".
    ///
    /// - "healthy": sync_progress >= 99.9%
    /// - "stalled": last block received > 5 minutes ago AND sync_progress < 99%
    /// - "syncing": everything else (actively catching up)
    pub fn health_status(&self) -> &'static str {
        let sync_pct = self.sync_progress_pct.load(Ordering::Relaxed);

        // Fully synced
        if sync_pct >= SYNCED_THRESHOLD {
            return "healthy";
        }

        // Check for stalled condition
        let last_block_ms = self.last_block_received_at.load(Ordering::Relaxed);
        if last_block_ms > 0 && sync_pct < 9900 {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let elapsed_secs = now_ms.saturating_sub(last_block_ms) / 1000;
            if elapsed_secs > STALLED_THRESHOLD_SECS {
                return "stalled";
            }
        }

        "syncing"
    }

    /// Returns the ISO 8601 timestamp of the last block received, or None if no block received yet.
    pub fn last_block_received_iso(&self) -> Option<String> {
        let ms = self.last_block_received_at.load(Ordering::Relaxed);
        if ms == 0 {
            return None;
        }
        let secs = (ms / 1000) as i64;
        let nanos = ((ms % 1000) * 1_000_000) as u32;
        let dt = chrono::DateTime::from_timestamp(secs, nanos)?;
        Some(dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
    }

    /// Returns uptime in seconds since node startup.
    pub fn uptime_seconds(&self) -> u64 {
        self.startup_instant.elapsed().as_secs()
    }

    /// Check if the node is ready (sync_progress >= 99.9%).
    /// Used for Kubernetes readiness probes.
    pub fn is_ready(&self) -> bool {
        self.sync_progress_pct.load(Ordering::Relaxed) >= SYNCED_THRESHOLD
    }

    /// Record a RollForward event timestamp for chainsync_idle tracking.
    pub fn record_roll_forward(&self) {
        let now_millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.last_roll_forward_at
            .store(now_millis, Ordering::Relaxed);
    }

    /// Set the tip slot time in milliseconds (POSIX). Tip age is computed dynamically.
    pub fn set_tip_slot_time_ms(&self, slot_time_ms: u64) {
        self.tip_slot_time_ms.store(slot_time_ms, Ordering::Relaxed);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let age = now_ms.saturating_sub(slot_time_ms) / 1000;
        self.tip_age_secs.store(age, Ordering::Relaxed);
    }

    /// Set the replay duration in seconds.
    pub fn set_replay_duration_secs(&self, secs: u64) {
        self.replay_duration_secs.store(secs, Ordering::Relaxed);
    }

    /// Set the network magic number.
    pub fn set_network_magic(&self, magic: u64) {
        self.network_magic.store(magic, Ordering::Relaxed);
    }

    /// Record block producer mode.
    ///
    /// Call once during node startup when forge credentials are loaded.
    /// Sets `torsten_is_block_producer` to 1 and stores the pool ID hex string
    /// so the TUI can display the role and abbreviated pool identifier.
    pub fn set_block_producer(&self, pool_id_hex: &str) {
        self.is_block_producer.store(1, Ordering::Relaxed);
        if let Ok(mut guard) = self.pool_id_hex.lock() {
            *guard = pool_id_hex.to_string();
        }
    }

    /// Compute and store the chainsync idle time.
    pub fn update_chainsync_idle(&self) {
        let last_rf = self.last_roll_forward_at.load(Ordering::Relaxed);
        if last_rf > 0 {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let idle_secs = now_ms.saturating_sub(last_rf) / 1000;
            self.chainsync_idle_secs.store(idle_secs, Ordering::Relaxed);
        }
    }

    /// Format metrics as Prometheus exposition format
    pub(crate) fn to_prometheus(&self) -> String {
        // Recompute tip_age dynamically for freshness
        let slot_time_ms = self.tip_slot_time_ms.load(Ordering::Relaxed);
        if slot_time_ms > 0 {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            self.tip_age_secs.store(
                now_ms.saturating_sub(slot_time_ms) / 1000,
                Ordering::Relaxed,
            );
        }
        // Sample the CPU tracker on every scrape.  This computes the
        // percentage CPU consumed since the previous scrape interval and
        // accumulates cumulative seconds.  Both values are emitted below.
        let (cpu_pct, cpu_secs_total) = if let Ok(mut tracker) = self.cpu_tracker.lock() {
            let pct = tracker.sample();
            (pct, tracker.cumulative_cpu_secs)
        } else {
            (0.0, 0.0)
        };

        let mut out = String::with_capacity(2048);

        // Counters (monotonically increasing totals)
        let counters: &[(&str, &str, &AtomicU64)] = &[
            (
                "torsten_blocks_received_total",
                "Total blocks received from peers",
                &self.blocks_received,
            ),
            (
                "torsten_blocks_applied_total",
                "Total blocks applied to ledger",
                &self.blocks_applied,
            ),
            (
                "torsten_transactions_received_total",
                "Total transactions received",
                &self.transactions_received,
            ),
            (
                "torsten_transactions_validated_total",
                "Total transactions validated",
                &self.transactions_validated,
            ),
            (
                "torsten_transactions_rejected_total",
                "Total transactions rejected",
                &self.transactions_rejected,
            ),
            (
                "torsten_rollback_count_total",
                "Total number of chain rollbacks",
                &self.rollback_count,
            ),
            (
                "torsten_blocks_forged_total",
                "Total blocks forged by this node",
                &self.blocks_forged,
            ),
            (
                "torsten_leader_checks_total",
                "Total VRF leader checks performed",
                &self.leader_checks_total,
            ),
            (
                "torsten_leader_checks_not_elected_total",
                "Leader checks where node was not elected",
                &self.leader_checks_not_elected,
            ),
            (
                "torsten_forge_failures_total",
                "Block forge attempts that failed",
                &self.forge_failures,
            ),
            (
                "torsten_blocks_announced_total",
                "Blocks successfully announced to peers",
                &self.blocks_announced,
            ),
            (
                "torsten_n2n_connections_total",
                "Total N2N connections accepted",
                &self.n2n_connections_total,
            ),
            (
                "torsten_n2c_connections_total",
                "Total N2C connections accepted",
                &self.n2c_connections_total,
            ),
        ];

        // Gauges (can go up and down)
        let gauges: &[(&str, &str, &AtomicU64)] = &[
            (
                "torsten_peers_connected",
                "Number of connected peers",
                &self.peers_connected,
            ),
            (
                "torsten_peers_outbound",
                "Outbound peer connections (initiated by us)",
                &self.peers_outbound,
            ),
            (
                "torsten_peers_inbound",
                "Inbound peer connections (initiated by remote)",
                &self.peers_inbound,
            ),
            (
                "torsten_peers_duplex",
                "Duplex (bidirectional) peer connections",
                &self.peers_duplex,
            ),
            (
                "torsten_peers_cold",
                "Number of cold (known but unconnected) peers",
                &self.peers_cold,
            ),
            (
                "torsten_peers_warm",
                "Number of warm (connected, not syncing) peers",
                &self.peers_warm,
            ),
            (
                "torsten_peers_hot",
                "Number of hot (actively syncing) peers",
                &self.peers_hot,
            ),
            (
                "torsten_conn_full_duplex",
                "Connections in full duplex state (both sides active)",
                &self.conn_full_duplex,
            ),
            (
                "torsten_conn_duplex",
                "Connections negotiated as Duplex (InitiatorAndResponder)",
                &self.conn_duplex,
            ),
            (
                "torsten_conn_unidirectional",
                "Connections negotiated as Unidirectional (InitiatorOnly)",
                &self.conn_unidirectional,
            ),
            (
                "torsten_conn_inbound",
                "Inbound connections (remote initiated)",
                &self.conn_inbound,
            ),
            (
                "torsten_conn_outbound",
                "Outbound connections (locally initiated)",
                &self.conn_outbound,
            ),
            (
                "torsten_conn_terminating",
                "Connections currently being torn down",
                &self.conn_terminating,
            ),
            (
                "torsten_sync_progress_percent",
                "Chain sync progress (0-10000, divide by 100 for %)",
                &self.sync_progress_pct,
            ),
            (
                "torsten_slot_number",
                "Current slot number",
                &self.slot_number,
            ),
            (
                "torsten_block_number",
                "Current block number",
                &self.block_number,
            ),
            (
                "torsten_epoch_number",
                "Current epoch number",
                &self.epoch_number,
            ),
            (
                "torsten_utxo_count",
                "Number of entries in the UTxO set",
                &self.utxo_count,
            ),
            (
                "torsten_mempool_tx_count",
                "Number of transactions in the mempool",
                &self.mempool_tx_count,
            ),
            (
                "torsten_mempool_tx_max",
                "Maximum transaction capacity of the mempool",
                &self.mempool_tx_max,
            ),
            (
                "torsten_mempool_bytes",
                "Size of mempool in bytes",
                &self.mempool_bytes,
            ),
            (
                "torsten_delegation_count",
                "Number of active stake delegations",
                &self.delegation_count,
            ),
            (
                "torsten_treasury_lovelace",
                "Total lovelace in the treasury",
                &self.treasury_lovelace,
            ),
            (
                "torsten_drep_count",
                "Number of registered DReps",
                &self.drep_count,
            ),
            (
                "torsten_proposal_count",
                "Number of active governance proposals",
                &self.proposal_count,
            ),
            (
                "torsten_pool_count",
                "Number of registered stake pools",
                &self.pool_count,
            ),
            (
                "torsten_disk_total_bytes",
                "Total disk space in bytes on the database volume",
                &self.disk_total_bytes,
            ),
            (
                "torsten_disk_used_bytes",
                "Used disk space in bytes on the database volume",
                &self.disk_used_bytes,
            ),
            (
                "torsten_disk_available_bytes",
                "Available disk space in bytes on the database volume",
                &self.disk_available_bytes,
            ),
            (
                "torsten_n2n_connections_active",
                "Currently active N2N connections",
                &self.n2n_connections_active,
            ),
            (
                "torsten_n2c_connections_active",
                "Currently active N2C connections",
                &self.n2c_connections_active,
            ),
            (
                "torsten_n2c_txs_submitted_total",
                "Total transactions submitted via N2C LocalTxSubmission",
                &self.n2c_txs_submitted,
            ),
            (
                "torsten_n2c_txs_accepted_total",
                "Transactions accepted via N2C LocalTxSubmission",
                &self.n2c_txs_accepted,
            ),
            (
                "torsten_n2c_txs_rejected_total",
                "Transactions rejected via N2C LocalTxSubmission",
                &self.n2c_txs_rejected,
            ),
            (
                "torsten_tip_age_seconds",
                "Seconds since the tip slot time",
                &self.tip_age_secs,
            ),
            (
                "torsten_chainsync_idle_seconds",
                "Seconds since last ChainSync RollForward event",
                &self.chainsync_idle_secs,
            ),
            (
                "torsten_ledger_replay_duration_seconds",
                "Duration of last ledger replay in seconds",
                &self.replay_duration_secs,
            ),
            (
                "torsten_network_magic",
                "Network magic number (764824073=mainnet, 2=preview, 1=preprod)",
                &self.network_magic,
            ),
            (
                "torsten_is_block_producer",
                "1 when running as a block producer (forge credentials loaded), 0 for relay",
                &self.is_block_producer,
            ),
        ];

        for (name, help, value) in counters {
            out.push_str(&format!(
                "# HELP {name} {help}\n# TYPE {name} counter\n{name} {}\n",
                value.load(Ordering::Relaxed)
            ));
        }

        for (name, help, value) in gauges {
            out.push_str(&format!(
                "# HELP {name} {help}\n# TYPE {name} gauge\n{name} {}\n",
                value.load(Ordering::Relaxed)
            ));
        }

        // Uptime gauge
        let uptime_secs = self.startup_instant.elapsed().as_secs();
        out.push_str(&format!(
            "# HELP torsten_uptime_seconds Time since node startup\n# TYPE torsten_uptime_seconds gauge\ntorsten_uptime_seconds {uptime_secs}\n"
        ));

        // Pool ID info metric — emitted only when running as a block producer.
        //
        // Uses a Prometheus info metric pattern: a gauge permanently set to 1
        // with a `pool_id` label carrying the hex-encoded pool identifier.
        // The TUI reads `torsten_pool_id_info` and parses the label value from
        // the metrics text to display an abbreviated pool ID in the Node panel.
        if self.is_block_producer.load(Ordering::Relaxed) == 1 {
            if let Ok(guard) = self.pool_id_hex.lock() {
                if !guard.is_empty() {
                    out.push_str(&format!(
                        "# HELP torsten_pool_id_info Block producer pool identity\n\
                         # TYPE torsten_pool_id_info gauge\n\
                         torsten_pool_id_info{{pool_id=\"{}\"}} 1\n",
                        *guard
                    ));
                }
            }
        }

        // CPU metrics — emitted as both a gauge (current %) and a counter
        // (cumulative seconds of CPU time consumed since node start).
        //
        // `torsten_cpu_percent`: instantaneous CPU utilisation (user + kernel)
        //   relative to one logical core.  >100 is possible on multi-threaded
        //   workloads.
        // `torsten_cpu_seconds_total`: monotonically increasing counter of
        //   wall-adjusted CPU seconds consumed since node start.
        out.push_str(&format!(
            "# HELP torsten_cpu_percent Process CPU utilisation as a percentage of one core\n\
             # TYPE torsten_cpu_percent gauge\n\
             torsten_cpu_percent {cpu_pct:.3}\n"
        ));
        out.push_str(&format!(
            "# HELP torsten_cpu_seconds_total Cumulative CPU time consumed by the process in seconds\n\
             # TYPE torsten_cpu_seconds_total counter\n\
             torsten_cpu_seconds_total {cpu_secs_total:.6}\n"
        ));

        // Resident memory gauge
        let rss = get_resident_memory_bytes();
        out.push_str(&format!(
            "# HELP torsten_mem_resident_bytes Resident set size in bytes\n# TYPE torsten_mem_resident_bytes gauge\ntorsten_mem_resident_bytes {rss}\n"
        ));

        // Total system physical memory gauge — used by the TUI memory bar to
        // show RSS as a percentage of total RAM rather than a raw byte value.
        let mem_total = get_total_memory_bytes();
        if mem_total > 0 {
            out.push_str(&format!(
                "# HELP torsten_mem_total_bytes Total physical memory on this host in bytes\n\
                 # TYPE torsten_mem_total_bytes gauge\n\
                 torsten_mem_total_bytes {mem_total}\n"
            ));
        }

        // Track peak RSS (monotonically increasing high-water mark).
        let _ = self.peak_mem_bytes.fetch_max(rss, Ordering::Relaxed);
        let peak = self.peak_mem_bytes.load(Ordering::Relaxed);
        out.push_str(&format!(
            "# HELP torsten_mem_peak_bytes Peak resident set size in bytes\n# TYPE torsten_mem_peak_bytes gauge\ntorsten_mem_peak_bytes {peak}\n"
        ));

        // Validation error breakdown
        if let Ok(errors) = self.validation_errors.lock() {
            if !errors.is_empty() {
                out.push_str("# HELP torsten_validation_errors_total Transaction validation errors by type\n");
                out.push_str("# TYPE torsten_validation_errors_total counter\n");
                let mut sorted: Vec<_> = errors.iter().collect();
                sorted.sort_by_key(|(k, _)| (*k).clone());
                for (error_type, count) in sorted {
                    out.push_str(&format!(
                        "torsten_validation_errors_total{{error=\"{error_type}\"}} {count}\n"
                    ));
                }
            }
        }

        // Protocol error breakdown
        if let Ok(errors) = self.protocol_errors.lock() {
            if !errors.is_empty() {
                out.push_str("# HELP torsten_protocol_errors_total Protocol errors by type\n");
                out.push_str("# TYPE torsten_protocol_errors_total counter\n");
                let mut sorted: Vec<_> = errors.iter().collect();
                sorted.sort_by_key(|(k, _)| (*k).clone());
                for (error_type, count) in sorted {
                    out.push_str(&format!(
                        "torsten_protocol_errors_total{{error=\"{error_type}\"}} {count}\n"
                    ));
                }
            }
        }

        // Histograms
        out.push_str(&self.peer_handshake_rtt_ms.to_prometheus(
            "torsten_peer_handshake_rtt_ms",
            "Peer handshake round-trip time in milliseconds",
        ));
        out.push_str(&self.peer_block_fetch_ms.to_prometheus(
            "torsten_peer_block_fetch_ms",
            "Per-block fetch latency in milliseconds",
        ));

        // cardano-node compatibility aliases.
        //
        // When --compat-metrics is set, emit a second set of metric lines using
        // the `cardano_node_metrics_*` naming convention.  This allows operators
        // to reuse existing cardano-node Grafana dashboards without modification.
        //
        // Naming rules follow the cardano-node EKG metric export convention:
        //   - Integer gauges use the `_int` suffix.
        //   - The density metric is a real-valued fraction in [0, 1].
        //   - forge metrics use the full EKG path as the metric name.
        //
        // NOTE: We emit only GAUGE lines (no # TYPE or # HELP declarations) for
        // the compat names because Prometheus rejects duplicate TYPE declarations
        // when the same name appears twice, and the compat names are aliases, not
        // independent metrics.  Prometheus will infer the type as "untyped" for
        // lines without a TYPE header, which is harmless for dashboard queries.
        if self.compat_metrics.load(Ordering::Relaxed) {
            // slotNum_int — current slot number
            out.push_str(&format!(
                "cardano_node_metrics_slotNum_int {}\n",
                self.slot_number.load(Ordering::Relaxed)
            ));

            // blockNum_int — current block number
            out.push_str(&format!(
                "cardano_node_metrics_blockNum_int {}\n",
                self.block_number.load(Ordering::Relaxed)
            ));

            // epoch_int — current epoch number
            out.push_str(&format!(
                "cardano_node_metrics_epoch_int {}\n",
                self.epoch_number.load(Ordering::Relaxed)
            ));

            // connectedPeers_int — total connected peers
            out.push_str(&format!(
                "cardano_node_metrics_connectedPeers_int {}\n",
                self.peers_connected.load(Ordering::Relaxed)
            ));

            // utxoSize_int — UTxO set size
            out.push_str(&format!(
                "cardano_node_metrics_utxoSize_int {}\n",
                self.utxo_count.load(Ordering::Relaxed)
            ));

            // txsInMempool_int — mempool transaction count
            out.push_str(&format!(
                "cardano_node_metrics_txsInMempool_int {}\n",
                self.mempool_tx_count.load(Ordering::Relaxed)
            ));

            // mempoolBytes_int — mempool size in bytes
            out.push_str(&format!(
                "cardano_node_metrics_mempoolBytes_int {}\n",
                self.mempool_bytes.load(Ordering::Relaxed)
            ));

            // Forge_forge_adopted_int — blocks forged and adopted
            out.push_str(&format!(
                "cardano_node_metrics_Forge_forge_adopted_int {}\n",
                self.blocks_forged.load(Ordering::Relaxed)
            ));

            // density_real — chain density as a fraction in [0, 1].
            //
            // torsten stores sync progress as (percentage * 100), i.e. 0–10000
            // for 0%–100%.  Divide by 10000 to produce the [0, 1] density
            // fraction that cardano-node's EKG dashboard panel expects.
            let density = self.sync_progress_pct.load(Ordering::Relaxed) as f64 / 10000.0;
            out.push_str(&format!("cardano_node_metrics_density_real {density:.6}\n"));
        }

        out
    }
}

impl Default for NodeMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Start an HTTP metrics server on the given port.
///
/// Responds to any request with Prometheus-format metrics.
///
/// On node restart the previous process may have left the port in `TIME_WAIT`
/// or another process may still be exiting.  Rather than failing immediately,
/// we retry binding up to 5 times with a 1-second delay between attempts.
/// This prevents the common "address already in use" startup failure when
/// restarting the node quickly after a crash.
pub async fn start_metrics_server(
    port: u16,
    metrics: Arc<NodeMetrics>,
) -> Result<(), std::io::Error> {
    let addr = format!("0.0.0.0:{port}");

    // Attempt to bind with retries to handle brief port-in-use windows on
    // fast restarts (TIME_WAIT, or a previous instance still shutting down).
    const MAX_RETRIES: u32 = 5;
    const RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(1);

    let listener = {
        let mut last_err = None;
        let mut bound = None;
        for attempt in 1..=MAX_RETRIES {
            match TcpListener::bind(&addr).await {
                Ok(l) => {
                    bound = Some(l);
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                    if attempt < MAX_RETRIES {
                        tracing::warn!(
                            port,
                            attempt,
                            max = MAX_RETRIES,
                            "Metrics port in use, retrying in 1s"
                        );
                        tokio::time::sleep(RETRY_DELAY).await;
                    }
                    last_err = Some(e);
                }
                Err(e) => {
                    // Non-retryable error (permission denied, etc.)
                    error!("Failed to start metrics server on {addr}: {e}");
                    return Err(e);
                }
            }
        }
        match bound {
            Some(l) => {
                info!(
                    url = format_args!("http://{addr}/metrics"),
                    "Metrics server started"
                );
                l
            }
            None => {
                let e = last_err.unwrap();
                error!(
                    "Failed to start metrics server on {addr} after {MAX_RETRIES} attempts: {e}"
                );
                return Err(e);
            }
        }
    };

    loop {
        let (mut stream, _) = match listener.accept().await {
            Ok(conn) => conn,
            Err(e) => {
                error!("Metrics server accept error: {e}");
                continue;
            }
        };

        // Read the request to determine the path
        let mut buf = [0u8; 1024];
        let n = tokio::io::AsyncReadExt::read(&mut stream, &mut buf)
            .await
            .unwrap_or(0);
        let request = std::str::from_utf8(&buf[..n]).unwrap_or("");

        let response = if request.starts_with("GET /ready") {
            // Kubernetes readiness probe: 200 if synced, 503 if not
            if metrics.is_ready() {
                let body = r#"{"ready":true}"#;
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
            } else {
                let sync_pct = metrics.sync_progress_pct.load(Ordering::Relaxed) as f64 / 100.0;
                let body = format!("{{\"ready\":false,\"sync_progress\":{sync_pct:.2}}}");
                format!(
                    "HTTP/1.1 503 Service Unavailable\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
            }
        } else if request.starts_with("GET /health") {
            let status = metrics.health_status();
            let uptime = metrics.uptime_seconds();
            let slot = metrics.slot_number.load(Ordering::Relaxed);
            let block = metrics.block_number.load(Ordering::Relaxed);
            let epoch = metrics.epoch_number.load(Ordering::Relaxed);
            let sync_pct = metrics.sync_progress_pct.load(Ordering::Relaxed) as f64 / 100.0;
            let peers = metrics.peers_connected.load(Ordering::Relaxed);
            let last_block_ts = metrics.last_block_received_iso();
            let last_block_json = match &last_block_ts {
                Some(ts) => format!("\"{}\"", ts),
                None => "null".to_string(),
            };
            let body = format!(
                "{{\"status\":\"{status}\",\"uptime_seconds\":{uptime},\"slot_number\":{slot},\"block_number\":{block},\"epoch_number\":{epoch},\"sync_progress\":{sync_pct:.2},\"peers_connected\":{peers},\"last_block_received_at\":{last_block_json}}}"
            );
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
        } else {
            let body = metrics.to_prometheus();
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
        };

        if let Err(e) = stream.write_all(response.as_bytes()).await {
            error!("Metrics server write error: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics() {
        let metrics = NodeMetrics::new();
        assert_eq!(metrics.blocks_applied.load(Ordering::Relaxed), 0);

        metrics.add_blocks_applied(2);
        assert_eq!(metrics.blocks_applied.load(Ordering::Relaxed), 2);

        metrics.set_slot(12345);
        assert_eq!(metrics.slot_number.load(Ordering::Relaxed), 12345);
    }

    #[test]
    fn test_prometheus_output() {
        let metrics = NodeMetrics::new();
        metrics.set_slot(99999);
        metrics.set_epoch(42);
        metrics.add_blocks_applied(100);

        let output = metrics.to_prometheus();
        assert!(output.contains("torsten_slot_number 99999"));
        assert!(output.contains("torsten_epoch_number 42"));
        assert!(output.contains("torsten_blocks_applied_total 100"));
        assert!(output.contains("# HELP"));
        // Verify correct metric types
        assert!(output.contains("# TYPE torsten_blocks_applied_total counter"));
        assert!(output.contains("# TYPE torsten_slot_number gauge"));
        assert!(output.contains("# TYPE torsten_rollback_count_total counter"));
        assert!(output.contains("# TYPE torsten_peers_connected gauge"));
    }

    #[test]
    fn test_compat_metrics_disabled_by_default() {
        // With the default NodeMetrics no compat aliases should appear.
        let metrics = NodeMetrics::new();
        metrics.set_slot(42);
        let output = metrics.to_prometheus();
        assert!(
            !output.contains("cardano_node_metrics_"),
            "compat aliases must not appear when compat_metrics is false"
        );
    }

    #[test]
    fn test_compat_metrics_enabled() {
        let metrics = NodeMetrics::new();
        metrics.set_compat_metrics(true);

        // Set known values so we can assert exact alias output.
        metrics.set_slot(100_000);
        metrics.set_block_number(50_000);
        metrics.set_epoch(410);
        metrics.peers_connected.store(8, Ordering::Relaxed);
        metrics.set_utxo_count(23_000_000);
        metrics.set_mempool_count(7);
        metrics.mempool_bytes.store(14_336, Ordering::Relaxed);
        metrics.blocks_forged.store(3, Ordering::Relaxed);
        // 50% sync stored as 5000 (pct * 100)
        metrics.sync_progress_pct.store(5000, Ordering::Relaxed);

        let output = metrics.to_prometheus();

        // Each alias must be present with the correct value.
        assert!(
            output.contains("cardano_node_metrics_slotNum_int 100000"),
            "slotNum_int alias missing or wrong"
        );
        assert!(
            output.contains("cardano_node_metrics_blockNum_int 50000"),
            "blockNum_int alias missing or wrong"
        );
        assert!(
            output.contains("cardano_node_metrics_epoch_int 410"),
            "epoch_int alias missing or wrong"
        );
        assert!(
            output.contains("cardano_node_metrics_connectedPeers_int 8"),
            "connectedPeers_int alias missing or wrong"
        );
        assert!(
            output.contains("cardano_node_metrics_utxoSize_int 23000000"),
            "utxoSize_int alias missing or wrong"
        );
        assert!(
            output.contains("cardano_node_metrics_txsInMempool_int 7"),
            "txsInMempool_int alias missing or wrong"
        );
        assert!(
            output.contains("cardano_node_metrics_mempoolBytes_int 14336"),
            "mempoolBytes_int alias missing or wrong"
        );
        assert!(
            output.contains("cardano_node_metrics_Forge_forge_adopted_int 3"),
            "Forge_forge_adopted_int alias missing or wrong"
        );
        // 5000 / 10000 = 0.5 density
        assert!(
            output.contains("cardano_node_metrics_density_real 0.500000"),
            "density_real alias missing or wrong"
        );

        // Native torsten metrics must still be present alongside compat aliases.
        assert!(
            output.contains("torsten_slot_number 100000"),
            "native torsten_slot_number must still be present"
        );
    }

    #[test]
    fn test_compat_metrics_can_be_toggled() {
        // Verify that set_compat_metrics can be called multiple times and takes effect.
        let metrics = NodeMetrics::new();
        metrics.set_slot(1);

        // Off initially
        let out1 = metrics.to_prometheus();
        assert!(!out1.contains("cardano_node_metrics_"));

        // Enable
        metrics.set_compat_metrics(true);
        let out2 = metrics.to_prometheus();
        assert!(out2.contains("cardano_node_metrics_slotNum_int 1"));

        // Disable again
        metrics.set_compat_metrics(false);
        let out3 = metrics.to_prometheus();
        assert!(!out3.contains("cardano_node_metrics_"));
    }

    #[test]
    fn test_compat_density_real_zero_and_full() {
        let metrics = NodeMetrics::new();
        metrics.set_compat_metrics(true);

        // 0% sync
        metrics.sync_progress_pct.store(0, Ordering::Relaxed);
        let out = metrics.to_prometheus();
        assert!(
            out.contains("cardano_node_metrics_density_real 0.000000"),
            "density_real must be 0.0 at 0% sync"
        );

        // 100% sync stored as 10000
        metrics.sync_progress_pct.store(10000, Ordering::Relaxed);
        let out = metrics.to_prometheus();
        assert!(
            out.contains("cardano_node_metrics_density_real 1.000000"),
            "density_real must be 1.0 at 100% sync"
        );
    }

    #[test]
    fn test_histogram_observe() {
        let h = Histogram::new();
        h.observe(5.0); // → bucket le=5
        h.observe(50.0); // → bucket le=50
        h.observe(500.0); // → bucket le=500

        assert_eq!(h.count.load(Ordering::Relaxed), 3);
        let sum = f64::from_bits(h.sum_bits.load(Ordering::Relaxed));
        assert!((sum - 555.0).abs() < 0.01);

        // Each observation lands in exactly one bucket
        assert_eq!(h.buckets[1].load(Ordering::Relaxed), 1); // le=5.0
        assert_eq!(h.buckets[4].load(Ordering::Relaxed), 1); // le=50.0
        assert_eq!(h.buckets[7].load(Ordering::Relaxed), 1); // le=500.0

        // Verify cumulative output via prometheus format
        let output = h.to_prometheus("test", "test");
        assert!(output.contains("test_bucket{le=\"5\"} 1"));
        assert!(output.contains("test_bucket{le=\"50\"} 2")); // cumulative: 5 + 50
        assert!(output.contains("test_bucket{le=\"500\"} 3")); // cumulative: all three
        assert!(output.contains("test_bucket{le=\"+Inf\"} 3"));
    }

    #[test]
    fn test_histogram_prometheus_format() {
        let h = Histogram::new();
        h.observe(10.0);
        h.observe(100.0);

        let output = h.to_prometheus("test_latency", "Test latency");
        assert!(output.contains("# TYPE test_latency histogram"));
        assert!(output.contains("test_latency_bucket{le=\"10\"} 1"));
        assert!(output.contains("test_latency_bucket{le=\"100\"} 2"));
        assert!(output.contains("test_latency_bucket{le=\"+Inf\"} 2"));
        assert!(output.contains("test_latency_sum 110"));
        assert!(output.contains("test_latency_count 2"));
    }

    #[test]
    fn test_prometheus_output_includes_histograms() {
        let metrics = NodeMetrics::new();
        metrics.record_handshake_rtt(50.0);
        metrics.record_block_fetch_latency(25.0);

        let output = metrics.to_prometheus();
        assert!(output.contains("torsten_peer_handshake_rtt_ms_bucket"));
        assert!(output.contains("torsten_peer_block_fetch_ms_bucket"));
        assert!(output.contains("torsten_uptime_seconds"));
    }

    #[test]
    fn test_handshake_rtt_records_to_histogram() {
        let metrics = NodeMetrics::new();
        metrics.record_handshake_rtt(42.0);
        metrics.record_handshake_rtt(150.0);
        let output = metrics.to_prometheus();
        assert!(output.contains("torsten_peer_handshake_rtt_ms_count 2"));
        // 42ms lands in le=50 bucket, 150ms lands in le=250 bucket
        assert!(output.contains("peer_handshake_rtt_ms_bucket{le=\"50\"} 1"));
        assert!(output.contains("peer_handshake_rtt_ms_bucket{le=\"250\"} 2"));
    }

    #[test]
    fn test_block_fetch_latency_records_to_histogram() {
        let metrics = NodeMetrics::new();
        metrics.record_block_fetch_latency(25.0);
        metrics.record_block_fetch_latency(300.0);
        let output = metrics.to_prometheus();
        assert!(output.contains("torsten_peer_block_fetch_ms_count 2"));
        assert!(output.contains("peer_block_fetch_ms_bucket{le=\"25\"} 1"));
        assert!(output.contains("peer_block_fetch_ms_bucket{le=\"500\"} 2"));
    }

    #[test]
    fn test_prometheus_output_includes_cpu_metrics() {
        // Verify that the two CPU-related metrics are always present in the
        // Prometheus output, even when the measured value is zero (which is the
        // case on platforms without a sampling implementation or immediately
        // after node start before any meaningful CPU has been consumed).
        let metrics = NodeMetrics::new();
        let output = metrics.to_prometheus();

        // Both metrics must be present with correct types.
        assert!(
            output.contains("# TYPE torsten_cpu_percent gauge"),
            "torsten_cpu_percent gauge TYPE declaration missing"
        );
        assert!(
            output.contains("# TYPE torsten_cpu_seconds_total counter"),
            "torsten_cpu_seconds_total counter TYPE declaration missing"
        );
        // The gauge line must exist (value may be 0.000 on first call).
        assert!(
            output.contains("torsten_cpu_percent "),
            "torsten_cpu_percent value line missing"
        );
        assert!(
            output.contains("torsten_cpu_seconds_total "),
            "torsten_cpu_seconds_total value line missing"
        );
        // The resident-memory metric should still be present alongside CPU.
        assert!(output.contains("torsten_mem_resident_bytes "));
    }

    #[test]
    fn test_cpu_tracker_cumulative_seconds_non_negative() {
        // After two sample() calls the cumulative CPU seconds must be >= 0.
        let mut tracker = CpuTracker::new();
        let _pct1 = tracker.sample();
        let _pct2 = tracker.sample();
        assert!(
            tracker.cumulative_cpu_secs >= 0.0,
            "cumulative CPU seconds must be non-negative, got {}",
            tracker.cumulative_cpu_secs
        );
    }

    #[test]
    fn test_health_status_healthy() {
        let metrics = NodeMetrics::new();
        // 99.9% = 9990 (stored as pct * 100)
        metrics.sync_progress_pct.store(9990, Ordering::Relaxed);
        assert_eq!(metrics.health_status(), "healthy");

        // Above threshold is also healthy
        metrics.sync_progress_pct.store(10000, Ordering::Relaxed);
        assert_eq!(metrics.health_status(), "healthy");
    }

    #[test]
    fn test_health_status_syncing() {
        let metrics = NodeMetrics::new();
        // 50% sync, recently received a block
        metrics.sync_progress_pct.store(5000, Ordering::Relaxed);
        metrics.record_block_received();
        assert_eq!(metrics.health_status(), "syncing");
    }

    #[test]
    fn test_health_status_stalled() {
        let metrics = NodeMetrics::new();
        // Below 99% and last block was > 5 minutes ago
        metrics.sync_progress_pct.store(5000, Ordering::Relaxed);
        let five_min_ago_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
            - (STALLED_THRESHOLD_SECS + 10) * 1000;
        metrics
            .last_block_received_at
            .store(five_min_ago_ms, Ordering::Relaxed);
        assert_eq!(metrics.health_status(), "stalled");
    }

    #[test]
    fn test_health_status_not_stalled_when_synced() {
        let metrics = NodeMetrics::new();
        // Even if last block was long ago, if we're synced we're healthy
        metrics.sync_progress_pct.store(9990, Ordering::Relaxed);
        let old_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
            - 600_000; // 10 minutes ago
        metrics
            .last_block_received_at
            .store(old_ms, Ordering::Relaxed);
        assert_eq!(metrics.health_status(), "healthy");
    }

    #[test]
    fn test_readiness_check() {
        let metrics = NodeMetrics::new();

        // Not ready at 0%
        assert!(!metrics.is_ready());

        // Not ready at 99%
        metrics.sync_progress_pct.store(9900, Ordering::Relaxed);
        assert!(!metrics.is_ready());

        // Ready at 99.9%
        metrics.sync_progress_pct.store(9990, Ordering::Relaxed);
        assert!(metrics.is_ready());

        // Ready at 100%
        metrics.sync_progress_pct.store(10000, Ordering::Relaxed);
        assert!(metrics.is_ready());
    }

    #[test]
    fn test_last_block_received_iso() {
        let metrics = NodeMetrics::new();

        // No block received yet
        assert!(metrics.last_block_received_iso().is_none());

        // Record a block
        metrics.record_block_received();
        let iso = metrics.last_block_received_iso();
        assert!(iso.is_some());
        let ts = iso.unwrap();
        // Should be a valid ISO 8601 string containing 'T' and 'Z'
        assert!(ts.contains('T'));
        assert!(ts.contains('Z'));
    }

    #[test]
    fn test_record_block_received_updates_timestamp() {
        let metrics = NodeMetrics::new();
        assert_eq!(metrics.last_block_received_at.load(Ordering::Relaxed), 0);
        metrics.record_block_received();
        let ts = metrics.last_block_received_at.load(Ordering::Relaxed);
        assert!(ts > 0);
        // Should be within the last second
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        assert!(now_ms - ts < 1000);
    }
}
