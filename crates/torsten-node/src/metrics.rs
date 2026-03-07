use std::sync::atomic::{AtomicU64, Ordering};

/// Node metrics for monitoring
#[allow(dead_code)]
pub struct NodeMetrics {
    pub blocks_received: AtomicU64,
    pub blocks_applied: AtomicU64,
    pub transactions_received: AtomicU64,
    pub transactions_validated: AtomicU64,
    pub transactions_rejected: AtomicU64,
    pub peers_connected: AtomicU64,
    pub sync_progress_pct: AtomicU64,
    pub slot_number: AtomicU64,
    pub epoch_number: AtomicU64,
    pub utxo_count: AtomicU64,
    pub mempool_tx_count: AtomicU64,
    pub mempool_bytes: AtomicU64,
}

#[allow(dead_code)]
impl NodeMetrics {
    pub fn new() -> Self {
        NodeMetrics {
            blocks_received: AtomicU64::new(0),
            blocks_applied: AtomicU64::new(0),
            transactions_received: AtomicU64::new(0),
            transactions_validated: AtomicU64::new(0),
            transactions_rejected: AtomicU64::new(0),
            peers_connected: AtomicU64::new(0),
            sync_progress_pct: AtomicU64::new(0),
            slot_number: AtomicU64::new(0),
            epoch_number: AtomicU64::new(0),
            utxo_count: AtomicU64::new(0),
            mempool_tx_count: AtomicU64::new(0),
            mempool_bytes: AtomicU64::new(0),
        }
    }

    pub fn inc_blocks_received(&self) {
        self.blocks_received.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_blocks_applied(&self) {
        self.blocks_applied.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_transactions_received(&self) {
        self.transactions_received.fetch_add(1, Ordering::Relaxed);
    }

    pub fn get_blocks_applied(&self) -> u64 {
        self.blocks_applied.load(Ordering::Relaxed)
    }

    pub fn set_slot(&self, slot: u64) {
        self.slot_number.store(slot, Ordering::Relaxed);
    }

    pub fn set_epoch(&self, epoch: u64) {
        self.epoch_number.store(epoch, Ordering::Relaxed);
    }

    pub fn set_sync_progress(&self, pct: f64) {
        self.sync_progress_pct
            .store((pct * 100.0) as u64, Ordering::Relaxed);
    }
}

impl Default for NodeMetrics {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics() {
        let metrics = NodeMetrics::new();
        assert_eq!(metrics.get_blocks_applied(), 0);

        metrics.inc_blocks_applied();
        metrics.inc_blocks_applied();
        assert_eq!(metrics.get_blocks_applied(), 2);

        metrics.set_slot(12345);
        assert_eq!(metrics.slot_number.load(Ordering::Relaxed), 12345);
    }
}
