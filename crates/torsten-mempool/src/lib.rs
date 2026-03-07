use dashmap::DashMap;
use parking_lot::RwLock;
use std::collections::VecDeque;
use torsten_primitives::hash::TransactionHash;
use torsten_primitives::transaction::Transaction;
use torsten_primitives::value::Lovelace;

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
#[allow(dead_code)]
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

    /// Add a transaction to the mempool
    pub fn add_tx(
        &self,
        tx_hash: TransactionHash,
        tx: Transaction,
        size_bytes: usize,
    ) -> Result<MempoolAddResult, MempoolError> {
        // Check if already exists
        if self.txs.contains_key(&tx_hash) {
            return Ok(MempoolAddResult::AlreadyExists);
        }

        // Check capacity
        if self.txs.len() >= self.config.max_transactions {
            return Err(MempoolError::Full {
                max: self.config.max_transactions,
            });
        }

        let total = *self.total_bytes.read();
        if total + size_bytes > self.config.max_bytes {
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
            fee: Lovelace(0), // Would be computed from tx body
            arrival_order,
        };

        self.txs.insert(tx_hash, entry);
        self.order.write().push_back(tx_hash);
        *self.total_bytes.write() += size_bytes;

        Ok(MempoolAddResult::Added)
    }

    /// Remove a transaction (when included in a block)
    pub fn remove_tx(&self, tx_hash: &TransactionHash) -> Option<Transaction> {
        if let Some((_, entry)) = self.txs.remove(tx_hash) {
            self.order.write().retain(|h| h != tx_hash);
            *self.total_bytes.write() -= entry.size_bytes;
            Some(entry.tx)
        } else {
            None
        }
    }

    /// Remove multiple transactions (batch removal after block)
    pub fn remove_txs(&self, tx_hashes: &[TransactionHash]) {
        for hash in tx_hashes {
            self.remove_tx(hash);
        }
    }

    /// Get a transaction by hash
    pub fn get_tx(&self, tx_hash: &TransactionHash) -> Option<Transaction> {
        self.txs.get(tx_hash).map(|r| r.tx.clone())
    }

    /// Check if a transaction is in the mempool
    pub fn contains(&self, tx_hash: &TransactionHash) -> bool {
        self.txs.contains_key(tx_hash)
    }

    /// Get transactions for block production (up to max count/size)
    pub fn get_txs_for_block(&self, max_count: usize, max_size: usize) -> Vec<Transaction> {
        let order = self.order.read();
        let mut result = Vec::new();
        let mut total_size = 0;

        for hash in order.iter() {
            if result.len() >= max_count {
                break;
            }
            if let Some(entry) = self.txs.get(hash) {
                if total_size + entry.size_bytes > max_size {
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

    /// Snapshot of current mempool state
    pub fn snapshot(&self) -> MempoolSnapshot {
        MempoolSnapshot {
            tx_count: self.txs.len(),
            total_bytes: *self.total_bytes.read(),
            tx_hashes: self.order.read().iter().copied().collect(),
        }
    }

    /// Clear all transactions
    pub fn clear(&self) {
        self.txs.clear();
        self.order.write().clear();
        *self.total_bytes.write() = 0;
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
    use torsten_primitives::address::{Address, ByronAddress};
    use torsten_primitives::hash::Hash32;
    use torsten_primitives::transaction::*;
    use torsten_primitives::value::Value;
    use std::collections::BTreeMap;

    fn make_dummy_tx() -> Transaction {
        Transaction {
            body: TransactionBody {
                inputs: vec![TransactionInput {
                    transaction_id: Hash32::ZERO,
                    index: 0,
                }],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress { payload: vec![0; 32] }),
                    value: Value::lovelace(1_000_000),
                    datum: OutputDatum::None,
                    script_ref: None,
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

        mempool.add_tx(Hash32::from_bytes([1u8; 32]), make_dummy_tx(), 100).unwrap();
        mempool.add_tx(Hash32::from_bytes([2u8; 32]), make_dummy_tx(), 100).unwrap();
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
        mempool.add_tx(Hash32::from_bytes([1u8; 32]), make_dummy_tx(), 500).unwrap();
        mempool.add_tx(Hash32::from_bytes([2u8; 32]), make_dummy_tx(), 300).unwrap();

        let snap = mempool.snapshot();
        assert_eq!(snap.tx_count, 2);
        assert_eq!(snap.total_bytes, 800);
        assert_eq!(snap.tx_hashes.len(), 2);
    }

    #[test]
    fn test_clear() {
        let mempool = Mempool::new(MempoolConfig::default());
        mempool.add_tx(Hash32::from_bytes([1u8; 32]), make_dummy_tx(), 500).unwrap();
        mempool.add_tx(Hash32::from_bytes([2u8; 32]), make_dummy_tx(), 300).unwrap();

        mempool.clear();
        assert!(mempool.is_empty());
        assert_eq!(mempool.total_bytes(), 0);
    }
}
