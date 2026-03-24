//! Prometheus metric definitions for the networking layer.
//!
//! These metrics are registered and updated by the connection manager,
//! protocol handlers, and mux tasks. Exposed on the node's Prometheus
//! endpoint (default port 12798).

/// Metric names used by the networking layer.
///
/// These are string constants matching the existing torsten-node Prometheus
/// metric names. The actual metric registration happens in torsten-node
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
