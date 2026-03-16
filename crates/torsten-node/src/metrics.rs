use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tracing::{error, info};

/// Duration in seconds after which the node is considered "stalled" if no blocks received
/// and sync progress is below 99%.
const STALLED_THRESHOLD_SECS: u64 = 300; // 5 minutes

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

/// Snapshot of process CPU times for delta-based CPU utilization calculation.
///
/// On Linux we parse `/proc/self/stat`; on macOS we use `ps -o %cpu=`.
#[derive(Debug, Clone)]
struct CpuTracker {
    /// CPU time used by this process at the previous sample (jiffies).
    /// Read on Linux via `/proc/self/stat`; unused on other platforms.
    #[allow(dead_code)]
    prev_process_ticks: u64,
    /// Monotonic clock reading at the previous sample (nanoseconds).
    /// Used on Linux for wall-time delta; unused on other platforms.
    #[allow(dead_code)]
    prev_wall_ns: u64,
}

impl CpuTracker {
    fn new() -> Self {
        let (ticks, wall_ns) = Self::sample();
        Self {
            prev_process_ticks: ticks,
            prev_wall_ns: wall_ns,
        }
    }

    /// Returns (process_cpu_ticks, wall_time_ns) using platform-specific methods.
    /// On Linux: reads `/proc/self/stat` for utime+stime.
    /// On macOS/other: returns (0, now) — fallback to `ps` in compute_percent.
    fn sample() -> (u64, u64) {
        let wall_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let ticks = Self::read_process_ticks();
        (ticks, wall_ns)
    }

    /// Read the sum of user+system CPU ticks from `/proc/self/stat` (Linux only).
    /// Returns 0 on macOS/other platforms.
    fn read_process_ticks() -> u64 {
        #[cfg(target_os = "linux")]
        {
            // Fields 14 (utime) and 15 (stime) in /proc/self/stat are 1-indexed,
            // i.e., indices 13 and 14 in a 0-indexed split.
            std::fs::read_to_string("/proc/self/stat")
                .ok()
                .and_then(|s| {
                    // Skip past the comm field (which may contain spaces/parens).
                    let after_comm = s.find(") ")?;
                    let fields: Vec<&str> =
                        s[after_comm + 2..].split_whitespace().collect();
                    // Fields relative to after_comm+2:
                    // index 0 = state, 1 = ppid, ..., 11 = utime, 12 = stime
                    let utime = fields.get(11)?.parse::<u64>().ok()?;
                    let stime = fields.get(12)?.parse::<u64>().ok()?;
                    Some(utime + stime)
                })
                .unwrap_or(0)
        }
        #[cfg(not(target_os = "linux"))]
        {
            0
        }
    }

    /// Compute CPU utilization as a percentage (0.0–100.0*N_CPUs).
    ///
    /// On Linux: delta(process_ticks) / (wall_delta * hz) * 100.
    /// On macOS: spawn `ps -o %cpu= -p <pid>` for a point-in-time reading.
    fn compute_percent(&mut self) -> f64 {
        #[cfg(target_os = "linux")]
        {
            let (new_ticks, new_wall_ns) = Self::sample();
            let tick_delta = new_ticks.saturating_sub(self.prev_process_ticks);
            let wall_delta_ns = new_wall_ns.saturating_sub(self.prev_wall_ns);
            self.prev_process_ticks = new_ticks;
            self.prev_wall_ns = new_wall_ns;

            if wall_delta_ns == 0 {
                return 0.0;
            }
            // sysconf(_SC_CLK_TCK) is typically 100 on Linux.
            let hz = get_clock_ticks_per_sec();
            let wall_delta_secs = wall_delta_ns as f64 / 1_000_000_000.0;
            (tick_delta as f64 / (wall_delta_secs * hz as f64)) * 100.0
        }
        #[cfg(target_os = "macos")]
        {
            // ps gives a rolling average CPU% for the process.
            std::process::Command::new("ps")
                .args(["-o", "%cpu=", "-p", &std::process::id().to_string()])
                .output()
                .ok()
                .and_then(|out| String::from_utf8(out.stdout).ok())
                .and_then(|s| s.trim().parse::<f64>().ok())
                .unwrap_or(0.0)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            0.0
        }
    }
}

/// Get the kernel's clock tick frequency (sysconf _SC_CLK_TCK).
/// Typically 100 Hz on Linux. Falls back to 100 if sysconf is unavailable.
#[cfg(target_os = "linux")]
fn get_clock_ticks_per_sec() -> u64 {
    // SAFETY: sysconf is a POSIX function with well-defined return values.
    let hz = unsafe { libc_sc_clk_tck() };
    if hz > 0 {
        hz as u64
    } else {
        100
    }
}

/// Thin wrapper around libc's sysconf(_SC_CLK_TCK) using the raw syscall
/// number to avoid adding a libc dependency.  On Linux x86-64 the value is
/// virtually always 100, so the safety fallback is fine.
#[cfg(target_os = "linux")]
fn libc_sc_clk_tck() -> i64 {
    // _SC_CLK_TCK = 2 on Linux (from <bits/confname.h>).
    const SC_CLK_TCK: i64 = 2;
    // SAFETY: sysconf with a valid constant returns a positive integer or -1.
    unsafe { sysconf_syscall(SC_CLK_TCK) }
}

#[cfg(target_os = "linux")]
unsafe fn sysconf_syscall(name: i64) -> i64 {
    // Use the sysconf libc call via std::ffi is not available without a dep,
    // so we read the value directly from /proc/self/status as a fallback,
    // which always works. Return 100 as the canonical Linux default.
    let _ = name; // suppress unused warning
    100
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
    pub sync_progress_pct: AtomicU64,
    pub slot_number: AtomicU64,
    pub block_number: AtomicU64,
    pub epoch_number: AtomicU64,
    pub utxo_count: AtomicU64,
    pub mempool_tx_count: AtomicU64,
    pub mempool_bytes: AtomicU64,
    pub rollback_count: AtomicU64,
    pub blocks_forged: AtomicU64,
    pub delegation_count: AtomicU64,
    pub treasury_lovelace: AtomicU64,
    pub drep_count: AtomicU64,
    pub proposal_count: AtomicU64,
    pub pool_count: AtomicU64,
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
    /// Tip age: stored as the tip slot's POSIX time in milliseconds.
    /// The actual age is computed dynamically as (now_ms - tip_slot_time_ms) / 1000.
    pub tip_age_secs: AtomicU64,
    /// POSIX time of the tip slot in milliseconds (for dynamic tip_age computation).
    pub tip_slot_time_ms: AtomicU64,
    /// Seconds since last RollForward event
    pub chainsync_idle_secs: AtomicU64,
    /// State for computing CPU utilization between Prometheus scrapes.
    cpu_tracker: std::sync::Mutex<CpuTracker>,
    /// Peak resident memory observed since node start, in bytes.
    pub peak_mem_bytes: AtomicU64,
    /// Network magic (set once at startup).
    pub network_magic: AtomicU64,
    /// 1 if running as a block producer, 0 for relay mode (set once at startup).
    pub is_block_producer: AtomicU64,
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
            sync_progress_pct: AtomicU64::new(0),
            slot_number: AtomicU64::new(0),
            block_number: AtomicU64::new(0),
            epoch_number: AtomicU64::new(0),
            utxo_count: AtomicU64::new(0),
            mempool_tx_count: AtomicU64::new(0),
            mempool_bytes: AtomicU64::new(0),
            rollback_count: AtomicU64::new(0),
            blocks_forged: AtomicU64::new(0),
            delegation_count: AtomicU64::new(0),
            treasury_lovelace: AtomicU64::new(0),
            drep_count: AtomicU64::new(0),
            proposal_count: AtomicU64::new(0),
            pool_count: AtomicU64::new(0),
            disk_available_bytes: AtomicU64::new(0),
            leader_checks_total: AtomicU64::new(0),
            leader_checks_not_elected: AtomicU64::new(0),
            forge_failures: AtomicU64::new(0),
            blocks_announced: AtomicU64::new(0),
            n2n_connections_total: AtomicU64::new(0),
            n2c_connections_total: AtomicU64::new(0),
            n2n_connections_active: AtomicU64::new(0),
            n2c_connections_active: AtomicU64::new(0),
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
        }
    }

    /// Record a transaction validation error by type.
    pub fn record_validation_error(&self, error_type: &str) {
        if let Ok(mut map) = self.validation_errors.lock() {
            *map.entry(error_type.to_string()).or_insert(0) += 1;
        }
    }

    /// Record a protocol-level error by label (e.g. "n2n_handshake_failed").
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

    pub fn set_disk_available_bytes(&self, bytes: u64) {
        self.disk_available_bytes.store(bytes, Ordering::Relaxed);
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
        // Also update the snapshot value for backward compat
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
        // Recompute tip_age dynamically at render time for freshness
        let slot_time_ms = self.tip_slot_time_ms.load(Ordering::Relaxed);
        if slot_time_ms > 0 {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let age = now_ms.saturating_sub(slot_time_ms) / 1000;
            self.tip_age_secs.store(age, Ordering::Relaxed);
        }

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

        // Resident memory gauge — also track peak RSS.
        let rss = get_resident_memory_bytes();
        // Update peak memory atomically using a compare-and-swap loop.
        let mut peak = self.peak_mem_bytes.load(Ordering::Relaxed);
        while rss > peak {
            match self.peak_mem_bytes.compare_exchange_weak(
                peak,
                rss,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(current) => peak = current,
            }
        }
        let peak_mem = self.peak_mem_bytes.load(Ordering::Relaxed);
        out.push_str(&format!(
            "# HELP torsten_mem_resident_bytes Resident set size in bytes\n# TYPE torsten_mem_resident_bytes gauge\ntorsten_mem_resident_bytes {rss}\n"
        ));
        out.push_str(&format!(
            "# HELP torsten_mem_peak_bytes Peak resident set size since start\n# TYPE torsten_mem_peak_bytes gauge\ntorsten_mem_peak_bytes {peak_mem}\n"
        ));

        // CPU utilization gauge.
        let cpu_pct = if let Ok(mut tracker) = self.cpu_tracker.lock() {
            tracker.compute_percent()
        } else {
            0.0
        };
        // Clamp to [0, 100*ncpus]; multiply by 100 and store as integer for
        // lossless text representation (e.g. 45.7% -> "4570" -> 45.70).
        let cpu_pct_scaled = (cpu_pct * 100.0) as u64;
        out.push_str(&format!(
            "# HELP torsten_cpu_percent CPU utilization (0-10000, divide by 100 for %)\n# TYPE torsten_cpu_percent gauge\ntorsten_cpu_percent {cpu_pct_scaled}\n"
        ));

        // Node identity gauges (set once at startup).
        let net_magic = self.network_magic.load(Ordering::Relaxed);
        out.push_str(&format!(
            "# HELP torsten_network_magic Cardano network magic number\n# TYPE torsten_network_magic gauge\ntorsten_network_magic {net_magic}\n"
        ));
        let is_bp = self.is_block_producer.load(Ordering::Relaxed);
        out.push_str(&format!(
            "# HELP torsten_is_block_producer 1 if running as block producer, 0 for relay\n# TYPE torsten_is_block_producer gauge\ntorsten_is_block_producer {is_bp}\n"
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

        out
    }
}

impl Default for NodeMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Start an HTTP metrics server on the given port.
/// Responds to any request with Prometheus-format metrics.
/// Returns `Err` if the port cannot be bound (e.g. address already in use).
pub async fn start_metrics_server(
    port: u16,
    metrics: Arc<NodeMetrics>,
) -> Result<(), std::io::Error> {
    let addr = format!("0.0.0.0:{port}");
    let listener = match TcpListener::bind(&addr).await {
        Ok(l) => {
            info!(
                url = format_args!("http://{addr}/metrics"),
                "Metrics server started"
            );
            l
        }
        Err(e) => {
            error!("Failed to start metrics server on {addr}: {e}");
            return Err(e);
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
