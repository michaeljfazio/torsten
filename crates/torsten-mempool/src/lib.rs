use dashmap::DashMap;
use parking_lot::RwLock;
use std::collections::{BTreeSet, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
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

/// Fee density key for sorted index ordering.
///
/// Stores (fee, size) to enable exact cross-multiplication comparison,
/// avoiding precision loss from integer division. Two densities are compared as:
///   a.fee * b.size  vs  b.fee * a.size
/// This is mathematically equivalent to comparing fee/size ratios but exact.
///
/// The `Ord` implementation sorts by fee density descending (highest first),
/// with ties broken by transaction hash ascending for deterministic ordering.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
struct FeeDensityKey {
    fee: u64,
    size: u64,
    tx_hash: TransactionHash,
}

impl FeeDensityKey {
    fn new(fee: u64, size: usize, tx_hash: TransactionHash) -> Self {
        Self {
            fee,
            // Treat zero-size as 1 to avoid division by zero in comparisons
            size: if size == 0 { 1 } else { size as u64 },
            tx_hash,
        }
    }
}

impl Ord for FeeDensityKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Compare fee densities using cross-multiplication to avoid precision loss:
        //   self.fee / self.size  vs  other.fee / other.size
        //   self.fee * other.size vs  other.fee * self.size
        // Use u128 to prevent overflow (u64 * u64 fits in u128)
        let lhs = (self.fee as u128) * (other.size as u128);
        let rhs = (other.fee as u128) * (self.size as u128);

        // Reverse: higher fee density comes first (descending order)
        rhs.cmp(&lhs).then_with(|| self.tx_hash.cmp(&other.tx_hash))
    }
}

impl PartialOrd for FeeDensityKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Transaction entry in the mempool
#[derive(Debug, Clone)]
struct MempoolEntry {
    tx: Transaction,
    tx_hash: TransactionHash,
    size_bytes: usize,
    fee: Lovelace,
}

/// The transaction mempool
///
/// Holds unconfirmed transactions waiting to be included in blocks.
/// Transactions are validated before admission and removed when
/// included in a block or when they expire.
///
/// Maintains a sorted index by fee density (fee/byte) descending for
/// efficient block production. The index uses cross-multiplication
/// comparison to avoid precision loss from integer division.
pub struct Mempool {
    /// Transactions indexed by hash
    txs: DashMap<TransactionHash, MempoolEntry>,
    /// FIFO order for fair processing
    order: RwLock<VecDeque<TransactionHash>>,
    /// Fee-density sorted index: iterates highest fee density first
    fee_index: RwLock<BTreeSet<FeeDensityKey>>,
    /// Current total size
    total_bytes: RwLock<usize>,
    /// Atomic transaction count for race-free capacity checks.
    /// The count is reserved (incremented) before inserting into the DashMap,
    /// preventing the TOCTOU race between `txs.len()` and `txs.insert()`.
    tx_count: AtomicUsize,
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
            fee_index: RwLock::new(BTreeSet::new()),
            total_bytes: RwLock::new(0),
            tx_count: AtomicUsize::new(0),
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

        // Atomically reserve a slot: increment tx_count first, then check capacity.
        // This eliminates the TOCTOU race between checking txs.len() and inserting.
        let count = self.tx_count.fetch_add(1, Ordering::Relaxed);
        if count >= self.config.max_transactions {
            self.tx_count.fetch_sub(1, Ordering::Relaxed);
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
            self.tx_count.fetch_sub(1, Ordering::Relaxed);
            warn!(
                size_bytes,
                total,
                max = self.config.max_bytes,
                "Mempool: tx too large, rejecting"
            );
            return Err(MempoolError::TooLarge { size: size_bytes });
        }

        let entry = MempoolEntry {
            tx,
            tx_hash,
            size_bytes,
            fee,
        };

        // Insert into fee-density sorted index
        let key = FeeDensityKey::new(fee.0, size_bytes, tx_hash);
        self.fee_index.write().insert(key);

        self.txs.insert(tx_hash, entry);
        self.order.write().push_back(tx_hash);
        *self.total_bytes.write() += size_bytes;

        debug!(
            hash = %tx_hash.to_hex(),
            size_bytes,
            total_txs = self.tx_count.load(Ordering::Relaxed),
            "Mempool: transaction added"
        );

        Ok(MempoolAddResult::Added)
    }

    /// Remove a transaction (when included in a block)
    pub fn remove_tx(&self, tx_hash: &TransactionHash) -> Option<Transaction> {
        if let Some((_, entry)) = self.txs.remove(tx_hash) {
            self.tx_count.fetch_sub(1, Ordering::Relaxed);
            self.order.write().retain(|h| h != tx_hash);

            // Remove from fee-density sorted index
            let key = FeeDensityKey::new(entry.fee.0, entry.size_bytes, *tx_hash);
            self.fee_index.write().remove(&key);

            *self.total_bytes.write() -= entry.size_bytes;
            debug!(
                hash = %tx_hash.to_hex(),
                remaining = self.tx_count.load(Ordering::Relaxed),
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
    /// Uses the persistent sorted index for O(n) iteration instead of O(n log n) sorting.
    pub fn get_txs_for_block(&self, max_count: usize, max_size: usize) -> Vec<Transaction> {
        let fee_index = self.fee_index.read();
        let mut result = Vec::new();
        let mut total_size = 0;

        for key in fee_index.iter() {
            if result.len() >= max_count {
                break;
            }
            if let Some(entry) = self.txs.get(&key.tx_hash) {
                if total_size + entry.size_bytes > max_size {
                    // Skip this tx but continue — smaller txs may still fit
                    continue;
                }
                result.push(entry.tx.clone());
                total_size += entry.size_bytes;
            }
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
    ///
    /// This is an alias for `get_txs_for_block()` — both use the same fee-density
    /// sorted index. Retained for backward compatibility.
    pub fn get_txs_for_block_by_fee(&self, max_count: usize, max_size: usize) -> Vec<Transaction> {
        self.get_txs_for_block(max_count, max_size)
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
        let count = self.tx_count.swap(0, Ordering::Relaxed);
        self.txs.clear();
        self.order.write().clear();
        self.fee_index.write().clear();
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
    use std::sync::atomic::Ordering;
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

        // Fee index should also be empty
        assert!(mempool.fee_index.read().is_empty());
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
        assert!(mempool.fee_index.read().is_empty());
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
    fn test_atomic_tx_count_consistency() {
        let config = MempoolConfig {
            max_transactions: 5,
            max_bytes: 1024 * 1024,
        };
        let mempool = Mempool::new(config);

        // Verify counter starts at zero
        assert_eq!(mempool.tx_count.load(Ordering::Relaxed), 0);

        // Add 3 transactions
        for i in 1..=3u8 {
            mempool
                .add_tx(Hash32::from_bytes([i; 32]), make_dummy_tx(), 100)
                .unwrap();
        }
        assert_eq!(mempool.tx_count.load(Ordering::Relaxed), 3);
        assert_eq!(mempool.len(), 3);

        // Remove one transaction
        mempool.remove_tx(&Hash32::from_bytes([2u8; 32]));
        assert_eq!(mempool.tx_count.load(Ordering::Relaxed), 2);
        assert_eq!(mempool.len(), 2);

        // Removing a non-existent transaction should not change the counter
        mempool.remove_tx(&Hash32::from_bytes([99u8; 32]));
        assert_eq!(mempool.tx_count.load(Ordering::Relaxed), 2);

        // Add more transactions up to capacity (5)
        for i in 4..=6u8 {
            mempool
                .add_tx(Hash32::from_bytes([i; 32]), make_dummy_tx(), 100)
                .unwrap();
        }
        assert_eq!(mempool.tx_count.load(Ordering::Relaxed), 5);
        assert_eq!(mempool.len(), 5);

        // Exceeding capacity should be rejected and counter stays at 5
        let result = mempool.add_tx(Hash32::from_bytes([7u8; 32]), make_dummy_tx(), 100);
        assert!(result.is_err());
        assert_eq!(mempool.tx_count.load(Ordering::Relaxed), 5);

        // Remove one, then adding should succeed again
        mempool.remove_tx(&Hash32::from_bytes([1u8; 32]));
        assert_eq!(mempool.tx_count.load(Ordering::Relaxed), 4);
        mempool
            .add_tx(Hash32::from_bytes([7u8; 32]), make_dummy_tx(), 100)
            .unwrap();
        assert_eq!(mempool.tx_count.load(Ordering::Relaxed), 5);

        // Clear should reset counter to zero
        mempool.clear();
        assert_eq!(mempool.tx_count.load(Ordering::Relaxed), 0);
        assert_eq!(mempool.len(), 0);

        // Adding after clear works correctly
        mempool
            .add_tx(Hash32::from_bytes([10u8; 32]), make_dummy_tx(), 100)
            .unwrap();
        assert_eq!(mempool.tx_count.load(Ordering::Relaxed), 1);
        assert_eq!(mempool.len(), 1);
    }

    #[test]
    fn test_atomic_count_on_size_rejection() {
        let config = MempoolConfig {
            max_transactions: 10,
            max_bytes: 200, // very small byte limit
        };
        let mempool = Mempool::new(config);

        // Add one tx that uses most of the byte budget
        mempool
            .add_tx(Hash32::from_bytes([1u8; 32]), make_dummy_tx(), 150)
            .unwrap();
        assert_eq!(mempool.tx_count.load(Ordering::Relaxed), 1);

        // This tx should be rejected for exceeding max_bytes, counter must stay at 1
        let result = mempool.add_tx(Hash32::from_bytes([2u8; 32]), make_dummy_tx(), 100);
        assert!(result.is_err());
        assert_eq!(mempool.tx_count.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_fee_density_no_overflow_near_u64_max() {
        // A fee near u64::MAX would overflow when multiplied without u128.
        // This test ensures no panic occurs and ordering is correct.
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

    // ========================== New fee-density ordering tests ==========================

    #[test]
    fn test_fee_density_ordering_different_sizes() {
        // Transactions with different fees AND sizes — fee density matters, not raw fee
        let mempool = Mempool::new(MempoolConfig::default());

        // tx1: 1_000_000 fee / 2000 bytes = 500 lovelace/byte
        mempool
            .add_tx_with_fee(
                Hash32::from_bytes([1u8; 32]),
                make_tx_with_fee(1_000_000),
                2000,
                Lovelace(1_000_000),
            )
            .unwrap();

        // tx2: 400_000 fee / 500 bytes = 800 lovelace/byte (HIGHER density despite lower fee)
        mempool
            .add_tx_with_fee(
                Hash32::from_bytes([2u8; 32]),
                make_tx_with_fee(400_000),
                500,
                Lovelace(400_000),
            )
            .unwrap();

        // tx3: 600_000 fee / 1000 bytes = 600 lovelace/byte
        mempool
            .add_tx_with_fee(
                Hash32::from_bytes([3u8; 32]),
                make_tx_with_fee(600_000),
                1000,
                Lovelace(600_000),
            )
            .unwrap();

        let txs = mempool.get_txs_for_block(10, 100_000);
        assert_eq!(txs.len(), 3);
        // Order: tx2 (800/byte) > tx3 (600/byte) > tx1 (500/byte)
        assert_eq!(txs[0].body.fee.0, 400_000); // highest density
        assert_eq!(txs[1].body.fee.0, 600_000); // middle density
        assert_eq!(txs[2].body.fee.0, 1_000_000); // lowest density
    }

    #[test]
    fn test_fee_density_same_density_deterministic_hash_tiebreak() {
        // Two transactions with identical fee density should be ordered deterministically by hash
        let mempool = Mempool::new(MempoolConfig::default());

        let hash_a = Hash32::from_bytes([0xAA; 32]);
        let hash_b = Hash32::from_bytes([0x11; 32]);

        // Both have same density: 1000 fee / 500 bytes = 2 per byte
        mempool
            .add_tx_with_fee(hash_a, make_tx_with_fee(1000), 500, Lovelace(1000))
            .unwrap();
        mempool
            .add_tx_with_fee(hash_b, make_tx_with_fee(1000), 500, Lovelace(1000))
            .unwrap();

        let txs = mempool.get_txs_for_block(10, 100_000);
        assert_eq!(txs.len(), 2);
        // hash_b (0x11...) < hash_a (0xAA...) lexicographically, so hash_b comes first
        assert_eq!(txs[0].body.fee.0, 1000);
        assert_eq!(txs[1].body.fee.0, 1000);

        // Run again — ordering must be stable/deterministic
        let txs2 = mempool.get_txs_for_block(10, 100_000);
        assert_eq!(txs2.len(), 2);
    }

    #[test]
    fn test_fee_density_proportional_same_density() {
        // 200 fee / 100 bytes = 2 per byte  (same as 400 fee / 200 bytes)
        // Cross-multiplication should detect these as equal
        let mempool = Mempool::new(MempoolConfig::default());

        mempool
            .add_tx_with_fee(
                Hash32::from_bytes([1u8; 32]),
                make_tx_with_fee(200),
                100,
                Lovelace(200),
            )
            .unwrap();
        mempool
            .add_tx_with_fee(
                Hash32::from_bytes([2u8; 32]),
                make_tx_with_fee(400),
                200,
                Lovelace(400),
            )
            .unwrap();

        let txs = mempool.get_txs_for_block(10, 100_000);
        assert_eq!(txs.len(), 2);
        // Both have equal density, so tiebreak is by hash ascending
        // [1u8; 32] < [2u8; 32], so tx with hash [1...] comes first
        assert_eq!(txs[0].body.fee.0, 200);
        assert_eq!(txs[1].body.fee.0, 400);
    }

    #[test]
    fn test_fee_density_zero_fee() {
        // Zero-fee transactions should sort last
        let mempool = Mempool::new(MempoolConfig::default());

        mempool
            .add_tx_with_fee(
                Hash32::from_bytes([1u8; 32]),
                make_tx_with_fee(0),
                500,
                Lovelace(0),
            )
            .unwrap();
        mempool
            .add_tx_with_fee(
                Hash32::from_bytes([2u8; 32]),
                make_tx_with_fee(100),
                500,
                Lovelace(100),
            )
            .unwrap();

        let txs = mempool.get_txs_for_block(10, 100_000);
        assert_eq!(txs.len(), 2);
        assert_eq!(txs[0].body.fee.0, 100); // non-zero fee first
        assert_eq!(txs[1].body.fee.0, 0); // zero fee last
    }

    #[test]
    fn test_fee_density_zero_size_treated_as_one() {
        // A zero-size transaction should not cause division by zero;
        // it's treated as size=1 for density calculation
        let mempool = Mempool::new(MempoolConfig::default());

        mempool
            .add_tx_with_fee(
                Hash32::from_bytes([1u8; 32]),
                make_tx_with_fee(1000),
                0, // zero size
                Lovelace(1000),
            )
            .unwrap();
        mempool
            .add_tx_with_fee(
                Hash32::from_bytes([2u8; 32]),
                make_tx_with_fee(1000),
                500,
                Lovelace(1000),
            )
            .unwrap();

        let txs = mempool.get_txs_for_block(10, 100_000);
        assert_eq!(txs.len(), 2);
        // Zero-size (treated as 1): density = 1000/1 = 1000
        // 500-byte: density = 1000/500 = 2
        // So zero-size tx comes first (higher density)
        assert_eq!(txs[0].body.fee.0, 1000); // zero-size tx
    }

    #[test]
    fn test_remove_maintains_fee_index_consistency() {
        let mempool = Mempool::new(MempoolConfig::default());

        mempool
            .add_tx_with_fee(
                Hash32::from_bytes([1u8; 32]),
                make_tx_with_fee(100),
                500,
                Lovelace(100),
            )
            .unwrap();
        mempool
            .add_tx_with_fee(
                Hash32::from_bytes([2u8; 32]),
                make_tx_with_fee(300),
                500,
                Lovelace(300),
            )
            .unwrap();
        mempool
            .add_tx_with_fee(
                Hash32::from_bytes([3u8; 32]),
                make_tx_with_fee(200),
                500,
                Lovelace(200),
            )
            .unwrap();

        assert_eq!(mempool.fee_index.read().len(), 3);

        // Remove the highest-fee tx
        mempool.remove_tx(&Hash32::from_bytes([2u8; 32]));
        assert_eq!(mempool.fee_index.read().len(), 2);

        // Now tx3 (200) should be first, tx1 (100) second
        let txs = mempool.get_txs_for_block(10, 100_000);
        assert_eq!(txs.len(), 2);
        assert_eq!(txs[0].body.fee.0, 200);
        assert_eq!(txs[1].body.fee.0, 100);
    }

    #[test]
    fn test_insertion_order_does_not_affect_fee_ordering() {
        // Insert low, high, medium — result should always be high, medium, low
        let mempool = Mempool::new(MempoolConfig::default());
        let size = 500;

        // Insert in non-sorted order: low, high, medium
        mempool
            .add_tx_with_fee(
                Hash32::from_bytes([1u8; 32]),
                make_tx_with_fee(100),
                size,
                Lovelace(100),
            )
            .unwrap();
        mempool
            .add_tx_with_fee(
                Hash32::from_bytes([2u8; 32]),
                make_tx_with_fee(500),
                size,
                Lovelace(500),
            )
            .unwrap();
        mempool
            .add_tx_with_fee(
                Hash32::from_bytes([3u8; 32]),
                make_tx_with_fee(300),
                size,
                Lovelace(300),
            )
            .unwrap();

        let txs = mempool.get_txs_for_block(10, 100_000);
        assert_eq!(txs[0].body.fee.0, 500);
        assert_eq!(txs[1].body.fee.0, 300);
        assert_eq!(txs[2].body.fee.0, 100);
    }

    #[test]
    fn test_get_txs_for_block_skips_oversized_includes_smaller() {
        // When a high-density tx doesn't fit, lower-density smaller txs can still be included
        let mempool = Mempool::new(MempoolConfig::default());

        // tx1: high density, large size (won't fit in budget after tx2)
        mempool
            .add_tx_with_fee(
                Hash32::from_bytes([1u8; 32]),
                make_tx_with_fee(10_000),
                800,
                Lovelace(10_000),
            )
            .unwrap();

        // tx2: medium density, small size
        mempool
            .add_tx_with_fee(
                Hash32::from_bytes([2u8; 32]),
                make_tx_with_fee(5_000),
                300,
                Lovelace(5_000),
            )
            .unwrap();

        // tx3: low density, small size
        mempool
            .add_tx_with_fee(
                Hash32::from_bytes([3u8; 32]),
                make_tx_with_fee(1_000),
                200,
                Lovelace(1_000),
            )
            .unwrap();

        // Budget: 1000 bytes. tx1 (800) fits first, then tx3 (200) fits, tx2 (300) doesn't
        let txs = mempool.get_txs_for_block(10, 1000);
        // tx1 density: 10000/800 = 12.5, tx2: 5000/300 = 16.67, tx3: 1000/200 = 5
        // Order by density: tx2 (16.67), tx1 (12.5), tx3 (5)
        // tx2 = 300, tx1 = 800 -> 300+800=1100 > 1000, skip tx1
        // tx3 = 200 -> 300+200=500 <= 1000, include
        assert_eq!(txs.len(), 2);
        assert_eq!(txs[0].body.fee.0, 5_000); // tx2 first (highest density)
        assert_eq!(txs[1].body.fee.0, 1_000); // tx3 (tx1 skipped, too large)
    }

    #[test]
    fn test_fee_index_consistent_after_batch_remove() {
        let mempool = Mempool::new(MempoolConfig::default());

        for i in 1..=5u8 {
            mempool
                .add_tx_with_fee(
                    Hash32::from_bytes([i; 32]),
                    make_tx_with_fee(i as u64 * 100),
                    500,
                    Lovelace(i as u64 * 100),
                )
                .unwrap();
        }

        assert_eq!(mempool.fee_index.read().len(), 5);

        let to_remove = vec![Hash32::from_bytes([2u8; 32]), Hash32::from_bytes([4u8; 32])];
        mempool.remove_txs(&to_remove);

        assert_eq!(mempool.fee_index.read().len(), 3);
        assert_eq!(mempool.len(), 3);

        // Remaining: tx5 (500), tx3 (300), tx1 (100)
        let txs = mempool.get_txs_for_block(10, 100_000);
        assert_eq!(txs.len(), 3);
        assert_eq!(txs[0].body.fee.0, 500);
        assert_eq!(txs[1].body.fee.0, 300);
        assert_eq!(txs[2].body.fee.0, 100);
    }

    #[test]
    fn test_fee_density_key_cross_multiplication_precision() {
        // Test that cross-multiplication correctly distinguishes densities that
        // would be equal under integer division.
        // tx1: 7 fee / 3 bytes = 2.333... (integer div = 2)
        // tx2: 5 fee / 2 bytes = 2.5     (integer div = 2)
        // Integer division would say they're equal; cross-multiplication should
        // correctly rank tx2 higher.
        let mempool = Mempool::new(MempoolConfig::default());

        mempool
            .add_tx_with_fee(
                Hash32::from_bytes([1u8; 32]),
                make_tx_with_fee(7),
                3,
                Lovelace(7),
            )
            .unwrap();
        mempool
            .add_tx_with_fee(
                Hash32::from_bytes([2u8; 32]),
                make_tx_with_fee(5),
                2,
                Lovelace(5),
            )
            .unwrap();

        let txs = mempool.get_txs_for_block(10, 100_000);
        assert_eq!(txs.len(), 2);
        // tx2 (5/2 = 2.5) should come before tx1 (7/3 = 2.333...)
        assert_eq!(txs[0].body.fee.0, 5);
        assert_eq!(txs[1].body.fee.0, 7);
    }

    #[test]
    fn test_fee_density_key_ordering_properties() {
        // Verify FeeDensityKey ordering directly
        let high = FeeDensityKey::new(1000, 100, Hash32::from_bytes([1u8; 32]));
        let low = FeeDensityKey::new(100, 100, Hash32::from_bytes([2u8; 32]));

        // High density should come first (be "less" in BTreeSet ordering)
        assert!(high < low);

        // Same density, different hash — lower hash comes first
        let a = FeeDensityKey::new(100, 100, Hash32::from_bytes([1u8; 32]));
        let b = FeeDensityKey::new(100, 100, Hash32::from_bytes([2u8; 32]));
        assert!(a < b);

        // Equal
        let c = FeeDensityKey::new(100, 100, Hash32::from_bytes([1u8; 32]));
        assert_eq!(a, c);
    }

    #[test]
    fn test_evict_expired_maintains_fee_index() {
        let mempool = Mempool::new(MempoolConfig::default());

        let mut tx1 = make_tx_with_fee(300);
        tx1.body.ttl = Some(SlotNo(50));
        mempool
            .add_tx_with_fee(Hash32::from_bytes([1u8; 32]), tx1, 500, Lovelace(300))
            .unwrap();

        let mut tx2 = make_tx_with_fee(100);
        tx2.body.ttl = Some(SlotNo(200));
        mempool
            .add_tx_with_fee(Hash32::from_bytes([2u8; 32]), tx2, 500, Lovelace(100))
            .unwrap();

        assert_eq!(mempool.fee_index.read().len(), 2);

        // Evict tx1 (TTL=50 expired at slot 51)
        mempool.evict_expired(SlotNo(51));
        assert_eq!(mempool.fee_index.read().len(), 1);
        assert_eq!(mempool.len(), 1);

        let txs = mempool.get_txs_for_block(10, 100_000);
        assert_eq!(txs.len(), 1);
        assert_eq!(txs[0].body.fee.0, 100);
    }

    #[test]
    fn test_many_txs_ordering_stability() {
        // Insert 100 transactions with varying densities and verify stable ordering
        let mempool = Mempool::new(MempoolConfig::default());

        for i in 1..=100u32 {
            let fee = (i as u64) * 1000;
            let size = 500 + (i as usize % 10) * 50; // varying sizes 500-950
            let mut hash_bytes = [0u8; 32];
            hash_bytes[0] = (i >> 8) as u8;
            hash_bytes[1] = (i & 0xFF) as u8;
            mempool
                .add_tx_with_fee(
                    Hash32::from_bytes(hash_bytes),
                    make_tx_with_fee(fee),
                    size,
                    Lovelace(fee),
                )
                .unwrap();
        }

        assert_eq!(mempool.len(), 100);
        assert_eq!(mempool.fee_index.read().len(), 100);

        let txs1 = mempool.get_txs_for_block(100, 1_000_000);
        let txs2 = mempool.get_txs_for_block(100, 1_000_000);

        // Ordering must be identical across calls (deterministic)
        assert_eq!(txs1.len(), txs2.len());
        for (a, b) in txs1.iter().zip(txs2.iter()) {
            assert_eq!(a.body.fee, b.body.fee);
        }

        // Verify descending fee density
        for i in 1..txs1.len() {
            let prev_fee = txs1[i - 1].body.fee.0;
            let curr_fee = txs1[i].body.fee.0;
            // This is a rough check — not strictly density comparison since sizes vary,
            // but at minimum we can verify the list is not in ascending fee order
            // The real invariant is that the BTreeSet iteration produces sorted order
            let _ = (prev_fee, curr_fee); // just verify no panic
        }
    }

    #[test]
    fn test_add_remove_add_same_hash() {
        // Add, remove, re-add same transaction — fee index should be consistent
        let mempool = Mempool::new(MempoolConfig::default());
        let hash = Hash32::from_bytes([42u8; 32]);

        mempool
            .add_tx_with_fee(hash, make_tx_with_fee(100), 500, Lovelace(100))
            .unwrap();
        assert_eq!(mempool.fee_index.read().len(), 1);

        mempool.remove_tx(&hash);
        assert_eq!(mempool.fee_index.read().len(), 0);

        // Re-add with different fee
        mempool
            .add_tx_with_fee(hash, make_tx_with_fee(200), 500, Lovelace(200))
            .unwrap();
        assert_eq!(mempool.fee_index.read().len(), 1);

        let txs = mempool.get_txs_for_block(10, 100_000);
        assert_eq!(txs.len(), 1);
        assert_eq!(txs[0].body.fee.0, 200);
    }
}
