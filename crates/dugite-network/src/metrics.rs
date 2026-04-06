//! Prometheus metric definitions for the networking layer.
//!
//! These metrics are registered and updated by the connection manager,
//! protocol handlers, and mux tasks. Exposed on the node's Prometheus
//! endpoint (default port 12798).

/// Metric names used by the networking layer.
///
/// These are string constants matching the existing dugite-node Prometheus
/// metric names. The actual metric registration happens in dugite-node
/// since it owns the Prometheus registry.
pub mod names {
    /// Number of currently connected peers (gauge).
    pub const PEERS_CONNECTED: &str = "peers_connected";
    /// Total blocks received from all peers (counter).
    pub const BLOCKS_RECEIVED: &str = "blocks_received";
    /// Total transactions received from all peers (counter).
    pub const TRANSACTIONS_RECEIVED: &str = "transactions_received";
    /// Total transactions validated successfully (counter).
    pub const TRANSACTIONS_VALIDATED: &str = "transactions_validated";
    /// Total transactions rejected during validation (counter).
    pub const TRANSACTIONS_REJECTED: &str = "transactions_rejected";
    /// Number of rollback events (counter).
    pub const ROLLBACK_COUNT: &str = "rollback_count";
    /// Current mempool transaction count (gauge).
    pub const MEMPOOL_TX_COUNT: &str = "mempool_tx_count";
    /// Current mempool bytes (gauge).
    pub const MEMPOOL_BYTES: &str = "mempool_bytes";
}

#[cfg(test)]
mod tests {
    use super::names;

    #[test]
    fn metric_names_are_non_empty() {
        assert!(!names::PEERS_CONNECTED.is_empty());
        assert!(!names::BLOCKS_RECEIVED.is_empty());
        assert!(!names::TRANSACTIONS_RECEIVED.is_empty());
        assert!(!names::TRANSACTIONS_VALIDATED.is_empty());
        assert!(!names::TRANSACTIONS_REJECTED.is_empty());
        assert!(!names::ROLLBACK_COUNT.is_empty());
        assert!(!names::MEMPOOL_TX_COUNT.is_empty());
        assert!(!names::MEMPOOL_BYTES.is_empty());
    }

    #[test]
    fn metric_names_have_expected_values() {
        // Verify exact string values match what Prometheus expects.
        assert_eq!(names::PEERS_CONNECTED, "peers_connected");
        assert_eq!(names::BLOCKS_RECEIVED, "blocks_received");
        assert_eq!(names::TRANSACTIONS_RECEIVED, "transactions_received");
        assert_eq!(names::TRANSACTIONS_VALIDATED, "transactions_validated");
        assert_eq!(names::TRANSACTIONS_REJECTED, "transactions_rejected");
        assert_eq!(names::ROLLBACK_COUNT, "rollback_count");
        assert_eq!(names::MEMPOOL_TX_COUNT, "mempool_tx_count");
        assert_eq!(names::MEMPOOL_BYTES, "mempool_bytes");
    }

    #[test]
    fn metric_names_are_valid_prometheus_identifiers() {
        // Prometheus metric names must match [a-zA-Z_:][a-zA-Z0-9_:]*.
        let all_names = [
            names::PEERS_CONNECTED,
            names::BLOCKS_RECEIVED,
            names::TRANSACTIONS_RECEIVED,
            names::TRANSACTIONS_VALIDATED,
            names::TRANSACTIONS_REJECTED,
            names::ROLLBACK_COUNT,
            names::MEMPOOL_TX_COUNT,
            names::MEMPOOL_BYTES,
        ];

        for name in &all_names {
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':'),
                "metric name '{name}' contains invalid Prometheus characters"
            );
            let first = name.chars().next().unwrap();
            assert!(
                first.is_ascii_alphabetic() || first == '_' || first == ':',
                "metric name '{name}' must start with [a-zA-Z_:]"
            );
        }
    }

    #[test]
    fn metric_names_are_unique() {
        let all_names = [
            names::PEERS_CONNECTED,
            names::BLOCKS_RECEIVED,
            names::TRANSACTIONS_RECEIVED,
            names::TRANSACTIONS_VALIDATED,
            names::TRANSACTIONS_REJECTED,
            names::ROLLBACK_COUNT,
            names::MEMPOOL_TX_COUNT,
            names::MEMPOOL_BYTES,
        ];
        let set: std::collections::HashSet<&&str> = all_names.iter().collect();
        assert_eq!(set.len(), all_names.len(), "metric names must be unique");
    }
}
