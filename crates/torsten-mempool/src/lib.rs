use dashmap::DashMap;
use parking_lot::RwLock;
use std::collections::VecDeque;
use torsten_primitives::hash::TransactionHash;
use torsten_primitives::time::SlotNo;
use torsten_primitives::transaction::Transaction;
use torsten_primitives::value::Lovelace;
use tracing::{debug, info, trace, warn};

/// Configuration for the mempool
#[derive(Debug, Clone)]
pub struct MempoolConfig {
    /// Maximum number of transactions in the mempool
    pub max_transactions: usize,
    /// Maximum total size in bytes
    pub max_bytes: usize,
}

impl Default for MempoolConfig {
    fn default() -> Self {
        MempoolConfig {
            max_transactions: 16_384,
            max_bytes: 512 * 1024 * 1024, // 512 MB
        }
    }
}

/// Transaction entry in the mempool
#[derive(Debug, Clone)]
struct MempoolEntry {
    tx: Transaction,
    tx_hash: TransactionHash,
    size_bytes: usize,
    fee: Lovelace,
    arrival_order: u64,
}

/// The transaction mempool
///
/// Holds unconfirmed transactions waiting to be included in blocks.
/// Transactions are validated before admission and removed when
/// included in a block or when they expire.
pub struct Mempool {
    /// Transactions indexed by hash
    txs: DashMap<TransactionHash, MempoolEntry>,
    /// FIFO order for fair processing
    order: RwLock<VecDeque<TransactionHash>>,
    /// Running counter for arrival order
    counter: RwLock<u64>,
    /// Current total size
    total_bytes: RwLock<usize>,
    /// Configuration
    config: MempoolConfig,
}

#[derive(Debug, thiserror::Error)]
pub enum MempoolError {
    #[error("Transaction already in mempool: {0}")]
    AlreadyExists(TransactionHash),
    #[error("Mempool is full (max {max} transactions)")]
    Full { max: usize },
    #[error("Transaction too large: {size} bytes")]
    TooLarge { size: usize },
    #[error("Validation error: {0}")]
    ValidationFailed(String),
}

/// Result of adding a transaction
#[derive(Debug)]
pub enum MempoolAddResult {
    Added,
    AlreadyExists,
}

impl Mempool {
    pub fn new(config: MempoolConfig) -> Self {
        Mempool {
            txs: DashMap::new(),
            order: RwLock::new(VecDeque::new()),
            counter: RwLock::new(0),
            total_bytes: RwLock::new(0),
            config,
        }
    }

    /// Add a transaction to the mempool.
    /// `fee` is the transaction fee (from tx body) used for priority ordering.
    pub fn add_tx(
        &self,
        tx_hash: TransactionHash,
        tx: Transaction,
        size_bytes: usize,
    ) -> Result<MempoolAddResult, MempoolError> {
        self.add_tx_with_fee(tx_hash, tx, size_bytes, Lovelace(0))
    }

    /// Add a transaction with explicit fee for priority ordering
    pub fn add_tx_with_fee(
        &self,
        tx_hash: TransactionHash,
        tx: Transaction,
        size_bytes: usize,
        fee: Lovelace,
    ) -> Result<MempoolAddResult, MempoolError> {
        // Check if already exists
        if self.txs.contains_key(&tx_hash) {
            trace!(hash = %tx_hash.to_hex(), "Mempool: tx already exists");
            return Ok(MempoolAddResult::AlreadyExists);
        }

        // Check capacity
        if self.txs.len() >= self.config.max_transactions {
            warn!(
                max = self.config.max_transactions,
                "Mempool: full, rejecting tx"
            );
            return Err(MempoolError::Full {
                max: self.config.max_transactions,
            });
        }

        let total = *self.total_bytes.read();
        if total + size_bytes > self.config.max_bytes {
            warn!(
                size_bytes,
                total,
                max = self.config.max_bytes,
                "Mempool: tx too large, rejecting"
            );
            return Err(MempoolError::TooLarge { size: size_bytes });
        }

        let arrival_order = {
            let mut counter = self.counter.write();
            *counter += 1;
            *counter
        };

        let entry = MempoolEntry {
            tx,
            tx_hash,
            size_bytes,
            fee,
            arrival_order,
        };

        self.txs.insert(tx_hash, entry);
        self.order.write().push_back(tx_hash);
        *self.total_bytes.write() += size_bytes;

        debug!(
            hash = %tx_hash.to_hex(),
            size_bytes,
            total_txs = self.txs.len(),
            "Mempool: transaction added"
        );

        Ok(MempoolAddResult::Added)
    }

    /// Remove a transaction (when included in a block)
    pub fn remove_tx(&self, tx_hash: &TransactionHash) -> Option<Transaction> {
        if let Some((_, entry)) = self.txs.remove(tx_hash) {
            self.order.write().retain(|h| h != tx_hash);
            *self.total_bytes.write() -= entry.size_bytes;
            debug!(
                hash = %tx_hash.to_hex(),
                remaining = self.txs.len(),
                "Mempool: transaction removed"
            );
            Some(entry.tx)
        } else {
            trace!(hash = %tx_hash.to_hex(), "Mempool: tx not found for removal");
            None
        }
    }

    /// Remove multiple transactions (batch removal after block)
    pub fn remove_txs(&self, tx_hashes: &[TransactionHash]) {
        if !tx_hashes.is_empty() {
            debug!(
                count = tx_hashes.len(),
                "Mempool: batch removing transactions"
            );
        }
        for hash in tx_hashes {
            self.remove_tx(hash);
        }
    }

    /// Get a transaction by hash
    pub fn get_tx(&self, tx_hash: &TransactionHash) -> Option<Transaction> {
        self.txs.get(tx_hash).map(|r| r.tx.clone())
    }

    /// Get a transaction's size in bytes
    pub fn get_tx_size(&self, tx_hash: &TransactionHash) -> Option<usize> {
        self.txs.get(tx_hash).map(|r| r.size_bytes)
    }

    /// Check if a transaction is in the mempool
    pub fn contains(&self, tx_hash: &TransactionHash) -> bool {
        self.txs.contains_key(tx_hash)
    }

    /// Get transactions for block production (up to max count/size).
    /// Transactions are sorted by fee density (fee/byte) descending to maximize block revenue.
    pub fn get_txs_for_block(&self, max_count: usize, max_size: usize) -> Vec<Transaction> {
        // Collect all entries with fee density for sorting
        let mut candidates: Vec<(Transaction, usize, u64)> = self
            .txs
            .iter()
            .map(|entry| {
                (
                    entry.tx.clone(),
                    entry.size_bytes,
                    entry
                        .fee
                        .0
                        .checked_div(entry.size_bytes as u64)
                        .unwrap_or(0),
                )
            })
            .collect();

        // Sort by fee density (fee/byte) descending
        candidates.sort_by(|a, b| b.2.cmp(&a.2));

        let mut result = Vec::new();
        let mut total_size = 0;

        for (tx, size, _) in candidates {
            if result.len() >= max_count {
                break;
            }
            if total_size + size > max_size {
                continue;
            }
            result.push(tx);
            total_size += size;
        }

        result
    }

    /// Number of transactions in the mempool
    pub fn len(&self) -> usize {
        self.txs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.txs.is_empty()
    }

    /// Total bytes used
    pub fn total_bytes(&self) -> usize {
        *self.total_bytes.read()
    }

    /// Maximum number of transactions the mempool can hold
    pub fn capacity(&self) -> usize {
        self.config.max_transactions
    }

    /// Get a transaction's raw CBOR bytes (for LocalTxMonitor protocol)
    pub fn get_tx_cbor(&self, tx_hash: &TransactionHash) -> Option<Vec<u8>> {
        self.txs
            .get(tx_hash)
            .and_then(|entry| entry.tx.raw_cbor.clone())
    }

    /// Get the first transaction hash in the mempool (for iteration)
    pub fn first_tx_hash(&self) -> Option<TransactionHash> {
        self.order.read().front().copied()
    }

    /// Get all transaction hashes in FIFO order (for TxMonitor snapshot cursor)
    pub fn tx_hashes_ordered(&self) -> Vec<TransactionHash> {
        self.order.read().iter().copied().collect()
    }

    /// Snapshot of current mempool state
    pub fn snapshot(&self) -> MempoolSnapshot {
        MempoolSnapshot {
            tx_count: self.txs.len(),
            total_bytes: *self.total_bytes.read(),
            tx_hashes: self.order.read().iter().copied().collect(),
        }
    }

    /// Evict transactions whose TTL has expired.
    /// Returns the number of evicted transactions.
    pub fn evict_expired(&self, current_slot: SlotNo) -> usize {
        let expired: Vec<TransactionHash> = self
            .txs
            .iter()
            .filter(|entry| {
                if let Some(ttl) = entry.tx.body.ttl {
                    current_slot.0 > ttl.0
                } else {
                    false
                }
            })
            .map(|entry| entry.tx_hash)
            .collect();

        let count = expired.len();
        for hash in &expired {
            self.remove_tx(hash);
        }
        if count > 0 {
            info!(
                evicted = count,
                slot = current_slot.0,
                "Mempool: evicted expired transactions"
            );
        }
        count
    }

    /// Get transactions for block production ordered by fee density (fee/byte, descending).
    /// Transactions with higher fee density are prioritized.
    pub fn get_txs_for_block_by_fee(&self, max_count: usize, max_size: usize) -> Vec<Transaction> {
        // Collect all entries with their fee density and arrival order
        let mut entries: Vec<(TransactionHash, u64, usize, u64)> = self
            .txs
            .iter()
            .map(|entry| {
                let fee_density = if entry.size_bytes > 0 {
                    entry.fee.0.saturating_mul(1000) / entry.size_bytes as u64 // fee per KB
                } else {
                    0
                };
                (
                    entry.tx_hash,
                    fee_density,
                    entry.size_bytes,
                    entry.arrival_order,
                )
            })
            .collect();

        // Sort by fee density descending, then by arrival order ascending (FIFO tiebreak)
        entries.sort_by(|a, b| b.1.cmp(&a.1).then(a.3.cmp(&b.3)));

        let mut result = Vec::new();
        let mut total_size = 0;

        for (hash, _, size, _) in entries {
            if result.len() >= max_count {
                break;
            }
            if total_size + size > max_size {
                continue;
            }
            if let Some(entry) = self.txs.get(&hash) {
                result.push(entry.tx.clone());
                total_size += size;
            }
        }

        result
    }

    /// Remove transactions that spend any of the given inputs (already consumed by a new block).
    /// Returns the hashes of removed transactions.
    pub fn revalidate_against_inputs(
        &self,
        consumed_inputs: &std::collections::HashSet<
            torsten_primitives::transaction::TransactionInput,
        >,
    ) -> Vec<TransactionHash> {
        let conflicting: Vec<TransactionHash> = self
            .txs
            .iter()
            .filter(|entry| {
                entry
                    .tx
                    .body
                    .inputs
                    .iter()
                    .any(|input| consumed_inputs.contains(input))
            })
            .map(|entry| entry.tx_hash)
            .collect();

        for hash in &conflicting {
            self.remove_tx(hash);
        }
        if !conflicting.is_empty() {
            debug!(
                removed = conflicting.len(),
                "Mempool: removed conflicting transactions after new block"
            );
        }
        conflicting
    }

    /// Clear all transactions
    pub fn clear(&self) {
        let count = self.txs.len();
        self.txs.clear();
        self.order.write().clear();
        *self.total_bytes.write() = 0;
        if count > 0 {
            info!(removed = count, "Mempool: cleared all transactions");
        }
    }
}

/// A snapshot of the mempool state (for queries)
#[derive(Debug, Clone)]
pub struct MempoolSnapshot {
    pub tx_count: usize,
    pub total_bytes: usize,
    pub tx_hashes: Vec<TransactionHash>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use torsten_primitives::address::{Address, ByronAddress};
    use torsten_primitives::hash::Hash32;
    use torsten_primitives::transaction::*;
    use torsten_primitives::value::Value;

    fn make_dummy_tx() -> Transaction {
        Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![TransactionInput {
                    transaction_id: Hash32::ZERO,
                    index: 0,
                }],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0; 32],
                    }),
                    value: Value::lovelace(1_000_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    raw_cbor: None,
                }],
                fee: Lovelace(200_000),
                ttl: None,
                certificates: vec![],
                withdrawals: BTreeMap::new(),
                auxiliary_data_hash: None,
                validity_interval_start: None,
                mint: BTreeMap::new(),
                script_data_hash: None,
                collateral: vec![],
                required_signers: vec![],
                network_id: None,
                collateral_return: None,
                total_collateral: None,
                reference_inputs: vec![],
                update: None,
                voting_procedures: BTreeMap::new(),
                proposal_procedures: vec![],
                treasury_value: None,
                donation: None,
            },
            witness_set: TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![],
                plutus_v3_scripts: vec![],
                plutus_data: vec![],
                redeemers: vec![],
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        }
    }

    #[test]
    fn test_add_and_get() {
        let mempool = Mempool::new(MempoolConfig::default());
        let tx = make_dummy_tx();
        let hash = Hash32::from_bytes([1u8; 32]);

        let result = mempool.add_tx(hash, tx.clone(), 500).unwrap();
        assert!(matches!(result, MempoolAddResult::Added));
        assert_eq!(mempool.len(), 1);
        assert!(mempool.contains(&hash));

        let retrieved = mempool.get_tx(&hash).unwrap();
        assert_eq!(retrieved.body.fee, tx.body.fee);
    }

    #[test]
    fn test_duplicate_add() {
        let mempool = Mempool::new(MempoolConfig::default());
        let tx = make_dummy_tx();
        let hash = Hash32::from_bytes([1u8; 32]);

        mempool.add_tx(hash, tx.clone(), 500).unwrap();
        let result = mempool.add_tx(hash, tx, 500).unwrap();
        assert!(matches!(result, MempoolAddResult::AlreadyExists));
        assert_eq!(mempool.len(), 1);
    }

    #[test]
    fn test_remove() {
        let mempool = Mempool::new(MempoolConfig::default());
        let tx = make_dummy_tx();
        let hash = Hash32::from_bytes([1u8; 32]);

        mempool.add_tx(hash, tx, 500).unwrap();
        let removed = mempool.remove_tx(&hash);
        assert!(removed.is_some());
        assert_eq!(mempool.len(), 0);
        assert_eq!(mempool.total_bytes(), 0);
    }

    #[test]
    fn test_capacity_limit() {
        let config = MempoolConfig {
            max_transactions: 2,
            max_bytes: 1024 * 1024,
        };
        let mempool = Mempool::new(config);

        mempool
            .add_tx(Hash32::from_bytes([1u8; 32]), make_dummy_tx(), 100)
            .unwrap();
        mempool
            .add_tx(Hash32::from_bytes([2u8; 32]), make_dummy_tx(), 100)
            .unwrap();
        let result = mempool.add_tx(Hash32::from_bytes([3u8; 32]), make_dummy_tx(), 100);
        assert!(result.is_err());
    }

    #[test]
    fn test_get_txs_for_block() {
        let mempool = Mempool::new(MempoolConfig::default());

        for i in 1..=5u8 {
            mempool
                .add_tx(Hash32::from_bytes([i; 32]), make_dummy_tx(), 200)
                .unwrap();
        }

        let txs = mempool.get_txs_for_block(3, 1000);
        assert_eq!(txs.len(), 3);
    }

    #[test]
    fn test_snapshot() {
        let mempool = Mempool::new(MempoolConfig::default());
        mempool
            .add_tx(Hash32::from_bytes([1u8; 32]), make_dummy_tx(), 500)
            .unwrap();
        mempool
            .add_tx(Hash32::from_bytes([2u8; 32]), make_dummy_tx(), 300)
            .unwrap();

        let snap = mempool.snapshot();
        assert_eq!(snap.tx_count, 2);
        assert_eq!(snap.total_bytes, 800);
        assert_eq!(snap.tx_hashes.len(), 2);
    }

    #[test]
    fn test_clear() {
        let mempool = Mempool::new(MempoolConfig::default());
        mempool
            .add_tx(Hash32::from_bytes([1u8; 32]), make_dummy_tx(), 500)
            .unwrap();
        mempool
            .add_tx(Hash32::from_bytes([2u8; 32]), make_dummy_tx(), 300)
            .unwrap();

        mempool.clear();
        assert!(mempool.is_empty());
        assert_eq!(mempool.total_bytes(), 0);
    }

    fn make_tx_with_ttl(ttl: Option<SlotNo>) -> Transaction {
        let mut tx = make_dummy_tx();
        tx.body.ttl = ttl;
        tx
    }

    fn make_tx_with_fee(fee: u64) -> Transaction {
        let mut tx = make_dummy_tx();
        tx.body.fee = Lovelace(fee);
        tx
    }

    fn make_tx_with_input(tx_id: [u8; 32], index: u32) -> Transaction {
        let mut tx = make_dummy_tx();
        tx.body.inputs = vec![TransactionInput {
            transaction_id: Hash32::from_bytes(tx_id),
            index,
        }];
        tx
    }

    #[test]
    fn test_evict_expired_ttl() {
        let mempool = Mempool::new(MempoolConfig::default());

        // Add tx with TTL at slot 100
        mempool
            .add_tx(
                Hash32::from_bytes([1u8; 32]),
                make_tx_with_ttl(Some(SlotNo(100))),
                200,
            )
            .unwrap();
        // Add tx with no TTL (never expires)
        mempool
            .add_tx(Hash32::from_bytes([2u8; 32]), make_tx_with_ttl(None), 200)
            .unwrap();
        // Add tx with TTL at slot 200
        mempool
            .add_tx(
                Hash32::from_bytes([3u8; 32]),
                make_tx_with_ttl(Some(SlotNo(200))),
                200,
            )
            .unwrap();

        assert_eq!(mempool.len(), 3);

        // At slot 50, nothing should be evicted
        assert_eq!(mempool.evict_expired(SlotNo(50)), 0);
        assert_eq!(mempool.len(), 3);

        // At slot 101, tx with TTL=100 should be evicted
        assert_eq!(mempool.evict_expired(SlotNo(101)), 1);
        assert_eq!(mempool.len(), 2);
        assert!(!mempool.contains(&Hash32::from_bytes([1u8; 32])));

        // At slot 201, tx with TTL=200 should be evicted
        assert_eq!(mempool.evict_expired(SlotNo(201)), 1);
        assert_eq!(mempool.len(), 1);
        // No-TTL tx remains
        assert!(mempool.contains(&Hash32::from_bytes([2u8; 32])));
    }

    #[test]
    fn test_fee_based_priority() {
        let mempool = Mempool::new(MempoolConfig::default());

        // Add 3 txs with different fees, same size
        let size = 500;
        mempool
            .add_tx_with_fee(
                Hash32::from_bytes([1u8; 32]),
                make_tx_with_fee(100_000),
                size,
                Lovelace(100_000),
            )
            .unwrap();
        mempool
            .add_tx_with_fee(
                Hash32::from_bytes([2u8; 32]),
                make_tx_with_fee(300_000),
                size,
                Lovelace(300_000),
            )
            .unwrap();
        mempool
            .add_tx_with_fee(
                Hash32::from_bytes([3u8; 32]),
                make_tx_with_fee(200_000),
                size,
                Lovelace(200_000),
            )
            .unwrap();

        let txs = mempool.get_txs_for_block_by_fee(3, 100_000);
        assert_eq!(txs.len(), 3);
        // Highest fee first
        assert_eq!(txs[0].body.fee.0, 300_000);
        assert_eq!(txs[1].body.fee.0, 200_000);
        assert_eq!(txs[2].body.fee.0, 100_000);
    }

    #[test]
    fn test_revalidate_against_inputs() {
        let mempool = Mempool::new(MempoolConfig::default());

        let input_a = TransactionInput {
            transaction_id: Hash32::from_bytes([10u8; 32]),
            index: 0,
        };
        let input_b = TransactionInput {
            transaction_id: Hash32::from_bytes([20u8; 32]),
            index: 0,
        };

        // tx1 spends input_a
        let mut tx1 = make_tx_with_input([10u8; 32], 0);
        tx1.body.inputs = vec![input_a.clone()];
        mempool
            .add_tx(Hash32::from_bytes([1u8; 32]), tx1, 200)
            .unwrap();

        // tx2 spends input_b
        let mut tx2 = make_tx_with_input([20u8; 32], 0);
        tx2.body.inputs = vec![input_b.clone()];
        mempool
            .add_tx(Hash32::from_bytes([2u8; 32]), tx2, 200)
            .unwrap();

        // tx3 spends a different input
        mempool
            .add_tx(
                Hash32::from_bytes([3u8; 32]),
                make_tx_with_input([30u8; 32], 0),
                200,
            )
            .unwrap();

        assert_eq!(mempool.len(), 3);

        // A new block consumed input_a
        let mut consumed = std::collections::HashSet::new();
        consumed.insert(input_a);
        let removed = mempool.revalidate_against_inputs(&consumed);

        assert_eq!(removed.len(), 1);
        assert_eq!(mempool.len(), 2);
        assert!(!mempool.contains(&Hash32::from_bytes([1u8; 32])));
        assert!(mempool.contains(&Hash32::from_bytes([2u8; 32])));
        assert!(mempool.contains(&Hash32::from_bytes([3u8; 32])));
    }

    #[test]
    fn test_add_tx_with_fee() {
        let mempool = Mempool::new(MempoolConfig::default());
        let tx = make_tx_with_fee(500_000);
        let hash = Hash32::from_bytes([1u8; 32]);

        let result = mempool
            .add_tx_with_fee(hash, tx, 1000, Lovelace(500_000))
            .unwrap();
        assert!(matches!(result, MempoolAddResult::Added));
        assert_eq!(mempool.len(), 1);
    }

    #[test]
    fn test_get_txs_for_block_fee_priority() {
        let mempool = Mempool::new(MempoolConfig::default());

        // Add 3 transactions with different fee densities
        // tx1: low fee density (100_000 fee / 500 bytes = 200 per byte)
        let mut tx1 = make_dummy_tx();
        tx1.body.fee = Lovelace(100_000);
        let h1 = Hash32::from_bytes([1u8; 32]);
        mempool
            .add_tx_with_fee(h1, tx1, 500, Lovelace(100_000))
            .unwrap();

        // tx2: high fee density (500_000 fee / 500 bytes = 1000 per byte)
        let mut tx2 = make_dummy_tx();
        tx2.body.fee = Lovelace(500_000);
        let h2 = Hash32::from_bytes([2u8; 32]);
        mempool
            .add_tx_with_fee(h2, tx2, 500, Lovelace(500_000))
            .unwrap();

        // tx3: medium fee density (200_000 fee / 500 bytes = 400 per byte)
        let mut tx3 = make_dummy_tx();
        tx3.body.fee = Lovelace(200_000);
        let h3 = Hash32::from_bytes([3u8; 32]);
        mempool
            .add_tx_with_fee(h3, tx3, 500, Lovelace(200_000))
            .unwrap();

        // Get txs — should be sorted by fee density: tx2, tx3, tx1
        let txs = mempool.get_txs_for_block(10, 100_000);
        assert_eq!(txs.len(), 3);
        assert_eq!(txs[0].body.fee, Lovelace(500_000)); // highest fee density
        assert_eq!(txs[1].body.fee, Lovelace(200_000)); // medium
        assert_eq!(txs[2].body.fee, Lovelace(100_000)); // lowest

        // With size limit, only highest-fee txs should be included
        let txs = mempool.get_txs_for_block(10, 1000);
        assert_eq!(txs.len(), 2); // only room for 2 x 500 bytes
        assert_eq!(txs[0].body.fee, Lovelace(500_000)); // highest priority first
        assert_eq!(txs[1].body.fee, Lovelace(200_000)); // second highest
    }

    #[test]
    fn test_fee_density_no_overflow_near_u64_max() {
        // A fee near u64::MAX would overflow when multiplied by 1000
        // without saturating_mul. This test ensures no panic occurs.
        let mempool = Mempool::new(MempoolConfig::default());

        let huge_fee = u64::MAX - 1; // 18_446_744_073_709_551_614
        let tx = make_tx_with_fee(huge_fee);
        let hash = Hash32::from_bytes([1u8; 32]);
        mempool
            .add_tx_with_fee(hash, tx, 1000, Lovelace(huge_fee))
            .unwrap();

        // A normal-fee tx for comparison
        let tx2 = make_tx_with_fee(200_000);
        let hash2 = Hash32::from_bytes([2u8; 32]);
        mempool
            .add_tx_with_fee(hash2, tx2, 1000, Lovelace(200_000))
            .unwrap();

        // Should not panic and should return both transactions
        let txs = mempool.get_txs_for_block_by_fee(10, 100_000);
        assert_eq!(txs.len(), 2);
        // The huge-fee tx should come first (higher fee density)
        assert_eq!(txs[0].body.fee.0, huge_fee);
        assert_eq!(txs[1].body.fee.0, 200_000);
    }
}
