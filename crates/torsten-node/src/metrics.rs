use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tracing::{error, info};

/// Node metrics for monitoring
pub struct NodeMetrics {
    pub blocks_received: AtomicU64,
    pub blocks_applied: AtomicU64,
    pub transactions_received: AtomicU64,
    pub transactions_validated: AtomicU64,
    pub transactions_rejected: AtomicU64,
    pub peers_connected: AtomicU64,
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
        }
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

    /// Format metrics as Prometheus exposition format
    pub(crate) fn to_prometheus(&self) -> String {
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
        ];

        // Gauges (can go up and down)
        let gauges: &[(&str, &str, &AtomicU64)] = &[
            (
                "torsten_peers_connected",
                "Number of connected peers",
                &self.peers_connected,
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
pub async fn start_metrics_server(port: u16, metrics: Arc<NodeMetrics>) {
    let addr = format!("0.0.0.0:{port}");
    let listener = match TcpListener::bind(&addr).await {
        Ok(l) => {
            info!("Metrics server listening on http://{addr}/metrics");
            l
        }
        Err(e) => {
            error!("Failed to start metrics server on {addr}: {e}");
            return;
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

        let body = metrics.to_prometheus();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );

        // Read the request (ignore content, just drain)
        let mut buf = [0u8; 1024];
        let _ = tokio::io::AsyncReadExt::read(&mut stream, &mut buf).await;

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
}
