//! Trait abstraction for mempool operations.
//!
//! Defined in `dugite-primitives` so that both `dugite-mempool` (impl) and
//! `dugite-network` (consumer) can depend on it without creating a direct
//! coupling between the two crates.

use std::fmt;

use crate::hash::TransactionHash;
use crate::transaction::Transaction;
use crate::value::Lovelace;

/// Result of adding a transaction to the mempool.
#[derive(Debug)]
pub enum MempoolAddResult {
    /// Transaction was added successfully.
    Added,
    /// Transaction already exists in the mempool.
    AlreadyExists,
}

/// Error adding a transaction to the mempool.
#[derive(Debug)]
pub struct MempoolAddError(pub String);

impl fmt::Display for MempoolAddError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Snapshot of the mempool state for queries.
#[derive(Debug, Clone)]
pub struct MempoolSnapshot {
    pub tx_count: usize,
    pub total_bytes: usize,
    pub tx_hashes: Vec<TransactionHash>,
}

/// Trait abstracting mempool operations.
///
/// This allows crates like `dugite-network` to interact with a mempool without
/// depending on the concrete `dugite-mempool` crate, keeping the coupling loose.
pub trait MempoolProvider: Send + Sync + 'static {
    /// Add a transaction to the mempool.
    fn add_tx(
        &self,
        tx_hash: TransactionHash,
        tx: Transaction,
        size_bytes: usize,
    ) -> Result<MempoolAddResult, MempoolAddError>;

    /// Add a transaction with explicit fee for priority ordering.
    fn add_tx_with_fee(
        &self,
        tx_hash: TransactionHash,
        tx: Transaction,
        size_bytes: usize,
        fee: Lovelace,
    ) -> Result<MempoolAddResult, MempoolAddError>;

    /// Check if a transaction is in the mempool.
    fn contains(&self, tx_hash: &TransactionHash) -> bool;

    /// Get a transaction by hash.
    fn get_tx(&self, tx_hash: &TransactionHash) -> Option<Transaction>;

    /// Get a transaction's size in bytes.
    fn get_tx_size(&self, tx_hash: &TransactionHash) -> Option<usize>;

    /// Get a transaction's raw CBOR bytes.
    fn get_tx_cbor(&self, tx_hash: &TransactionHash) -> Option<Vec<u8>>;

    /// Get all transaction hashes in FIFO order.
    fn tx_hashes_ordered(&self) -> Vec<TransactionHash>;

    /// Number of transactions in the mempool.
    fn len(&self) -> usize;

    /// Whether the mempool is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Total bytes used by transactions in the mempool.
    fn total_bytes(&self) -> usize;

    /// Maximum number of transactions the mempool can hold.
    fn capacity(&self) -> usize;

    /// Snapshot of current mempool state.
    fn snapshot(&self) -> MempoolSnapshot;
}
