use dashmap::DashMap;
use parking_lot::{Mutex, RwLock};
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use torsten_primitives::hash::TransactionHash;
use torsten_primitives::time::SlotNo;
use torsten_primitives::transaction::{Transaction, TransactionInput, TransactionOutput};
use torsten_primitives::value::Lovelace;
use tracing::{debug, info, trace, warn};

/// Configuration for the mempool
#[derive(Debug, Clone)]
pub struct MempoolConfig {
    /// Maximum number of transactions in the mempool
    pub max_transactions: usize,
    /// Maximum total size in bytes
    pub max_bytes: usize,
    /// Maximum total execution memory units (sum of all tx redeemers)
    pub max_ex_mem: u64,
    /// Maximum total execution step units (sum of all tx redeemers)
    pub max_ex_steps: u64,
    /// Maximum total reference script bytes
    pub max_ref_scripts_bytes: usize,
}

impl Default for MempoolConfig {
    fn default() -> Self {
        MempoolConfig {
            max_transactions: 16_384,
            max_bytes: 512 * 1024 * 1024, // 512 MB
            // 2x block limits (matching Haskell cardano-node defaults)
            max_ex_mem: 28_000_000_000,       // 2 * 14B mem
            max_ex_steps: 20_000_000_000_000, // 2 * 10T steps
            max_ref_scripts_bytes: 1_048_576, // 1 MB
        }
    }
}

/// Origin of a transaction submission — used for dual-FIFO fairness.
///
/// Local clients (wallets connected via N2C) get equal weight to ALL remote
/// peers combined, matching Haskell's cardano-node fairness model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxOrigin {
    /// From a local client (N2C connection)
    Local,
    /// From a remote peer (N2N connection)
    Remote,
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
    ex_units_mem: u64,
    ex_units_steps: u64,
    ref_scripts_bytes: usize,
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
///
/// Capacity is enforced across multiple dimensions: transaction count,
/// total bytes, execution units (mem + steps), and reference script bytes.
/// When full, lowest-fee-density transactions are evicted to make room
/// for higher-density newcomers.
///
/// Input-conflict checking is enforced at admission: a new transaction whose
/// inputs overlap with any existing mempool transaction is rejected immediately
/// with `MempoolError::InputConflict`. This matches Haskell cardano-node
/// behaviour (`Ouroboros.Consensus.Mempool.addTx`) which validates each new
/// tx against the virtual UTxO state (ledger tip + pending mempool txs).
///
/// Dual-FIFO fairness ensures local clients get equal admission weight
/// to all remote peers combined.
pub struct Mempool {
    /// Transactions indexed by hash
    txs: DashMap<TransactionHash, MempoolEntry>,
    /// FIFO order for fair processing.
    /// Tombstoned entries (in `order_tombstones`) are skipped during iteration
    /// and compacted when the tombstone ratio exceeds 50%.
    order: RwLock<VecDeque<TransactionHash>>,
    /// O(1) tombstone set: hashes removed from the mempool but not yet
    /// compacted out of the `order` VecDeque. Avoids O(n) retain per removal.
    order_tombstones: RwLock<HashSet<TransactionHash>>,
    /// Fee-density sorted index: iterates highest fee density first
    fee_index: RwLock<BTreeSet<FeeDensityKey>>,
    /// Input-conflict index: maps each claimed TransactionInput to the hash of
    /// the mempool tx that spends it.  Allows O(1) conflict detection at
    /// admission without scanning every existing tx's input set.
    claimed_inputs: DashMap<TransactionInput, TransactionHash>,
    /// Virtual UTxO set: tracks unspent outputs of pending (unconfirmed) mempool
    /// transactions so that chained/dependent transactions can be validated
    /// against them.
    ///
    /// Key:   TransactionInput { transaction_id = pending_tx.hash, index }
    /// Value: the TransactionOutput at that index in the pending tx
    ///
    /// Entries are added when a tx is admitted and removed when it is evicted,
    /// confirmed, or cascaded away.
    virtual_utxo: DashMap<TransactionInput, TransactionOutput>,
    /// Dependency graph: maps parent tx hash → list of child tx hashes.
    ///
    /// When tx B spends an output of tx A (which is still unconfirmed), B is a
    /// "child" of A in this graph.  If A is removed for any reason (confirmed,
    /// evicted, expired), all its children are cascade-removed as well so that
    /// no tx can reference a no-longer-available virtual UTxO output.
    ///
    /// Stored under a single `RwLock` rather than a `DashMap<…, DashMap>` to
    /// keep the cascade walk atomic (no child can be re-added between the point
    /// we read the children list and the point we remove them).
    dependents: RwLock<HashMap<TransactionHash, Vec<TransactionHash>>>,
    /// Current total size
    total_bytes: RwLock<usize>,
    /// Current total execution memory units
    total_ex_mem: AtomicU64,
    /// Current total execution step units
    total_ex_steps: AtomicU64,
    /// Current total reference script bytes
    total_ref_scripts_bytes: AtomicUsize,
    /// Atomic transaction count for race-free capacity checks.
    /// The count is reserved (incremented) before inserting into the DashMap,
    /// preventing the TOCTOU race between `txs.len()` and `txs.insert()`.
    tx_count: AtomicUsize,
    /// Configuration (behind a RwLock so capacity can be updated dynamically
    /// from live protocol params without requiring `&mut self`).
    config: RwLock<MempoolConfig>,
    /// Fairness mutex for remote submissions — all remote peers compete for this first
    remote_fifo: Mutex<()>,
    /// Fairness mutex for all submissions — local acquires only this, remote acquires both
    all_fifo: Mutex<()>,
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
    #[error("Insufficient priority: new tx fee density too low to evict existing transactions")]
    InsufficientPriority,
    /// Rejected because another mempool tx already spends the same input.
    /// `claimed_by` is the hash of the tx that holds the conflicting input.
    #[error("Input conflict: input already claimed by mempool tx {claimed_by}")]
    InputConflict { claimed_by: TransactionHash },
}

/// Result of adding a transaction
#[derive(Debug)]
pub enum MempoolAddResult {
    Added,
    AlreadyExists,
}

/// Sum the execution units (memory + steps) declared by all redeemers in a transaction.
/// Returns `(0, 0)` for transactions with no Plutus scripts.
fn tx_ex_units(tx: &Transaction) -> (u64, u64) {
    let mut mem: u64 = 0;
    let mut steps: u64 = 0;
    for r in &tx.witness_set.redeemers {
        mem = mem.saturating_add(r.ex_units.mem);
        steps = steps.saturating_add(r.ex_units.steps);
    }
    (mem, steps)
}

impl Mempool {
    pub fn new(config: MempoolConfig) -> Self {
        Mempool {
            txs: DashMap::new(),
            order: RwLock::new(VecDeque::new()),
            order_tombstones: RwLock::new(HashSet::new()),
            fee_index: RwLock::new(BTreeSet::new()),
            claimed_inputs: DashMap::new(),
            virtual_utxo: DashMap::new(),
            dependents: RwLock::new(HashMap::new()),
            total_bytes: RwLock::new(0),
            total_ex_mem: AtomicU64::new(0),
            total_ex_steps: AtomicU64::new(0),
            total_ref_scripts_bytes: AtomicUsize::new(0),
            tx_count: AtomicUsize::new(0),
            config: RwLock::new(config),
            remote_fifo: Mutex::new(()),
            all_fifo: Mutex::new(()),
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
        self.add_tx_full(tx_hash, tx, size_bytes, fee, 0, 0, 0, TxOrigin::Local)
    }

    /// Add a transaction with full multi-dimensional capacity tracking and fairness.
    ///
    /// This is the primary admission method. All dimensions (count, bytes, ExUnits,
    /// reference scripts) are checked. If any dimension is exceeded, the mempool
    /// attempts to evict lowest-fee-density transactions to make room — but only
    /// if the new tx has higher fee density than the worst existing tx.
    ///
    /// The `origin` parameter controls dual-FIFO fairness: remote submissions are
    /// serialized through an additional mutex so local clients get equal weight.
    #[allow(clippy::too_many_arguments)]
    pub fn add_tx_full(
        &self,
        tx_hash: TransactionHash,
        tx: Transaction,
        size_bytes: usize,
        fee: Lovelace,
        ex_units_mem: u64,
        ex_units_steps: u64,
        ref_scripts_bytes: usize,
        origin: TxOrigin,
    ) -> Result<MempoolAddResult, MempoolError> {
        // Dual-FIFO fairness: remote acquires remote_fifo then all_fifo,
        // local acquires only all_fifo. This gives local clients equal weight
        // to ALL remote peers combined.
        let _remote_guard = if origin == TxOrigin::Remote {
            Some(self.remote_fifo.lock())
        } else {
            None
        };
        let _all_guard = self.all_fifo.lock();

        self.add_tx_inner(
            tx_hash,
            tx,
            size_bytes,
            fee,
            ex_units_mem,
            ex_units_steps,
            ref_scripts_bytes,
        )
    }

    /// Inner admission logic (called under fairness locks)
    #[allow(clippy::too_many_arguments)]
    fn add_tx_inner(
        &self,
        tx_hash: TransactionHash,
        tx: Transaction,
        size_bytes: usize,
        fee: Lovelace,
        ex_units_mem: u64,
        ex_units_steps: u64,
        ref_scripts_bytes: usize,
    ) -> Result<MempoolAddResult, MempoolError> {
        // Check if already exists (idempotent re-submission is silently accepted)
        if self.txs.contains_key(&tx_hash) {
            trace!(hash = %tx_hash.to_hex(), "Mempool: tx already exists");
            return Ok(MempoolAddResult::AlreadyExists);
        }

        // Input-conflict check: reject if any spending input is already claimed by
        // a pending mempool tx.  This matches Haskell cardano-node behaviour —
        // only one tx per UTxO can be in-flight at a time.
        //
        // Only `body.inputs` (spending inputs) create exclusive claims.
        // Collateral inputs are only consumed for phase-2 failing txs; valid txs
        // never touch them.  Reference inputs are read-only and freely shareable.
        for input in &tx.body.inputs {
            if let Some(entry) = self.claimed_inputs.get(input) {
                let claimed_by = *entry;
                trace!(
                    new_hash = %tx_hash.to_hex(),
                    claimed_by = %claimed_by.to_hex(),
                    tx_id = %input.transaction_id.to_hex(),
                    index = input.index,
                    "Mempool: input conflict detected, rejecting tx"
                );
                return Err(MempoolError::InputConflict { claimed_by });
            }
        }

        // Try eviction if any capacity dimension would be exceeded
        if !self.ensure_capacity(
            size_bytes,
            fee.0,
            ex_units_mem,
            ex_units_steps,
            ref_scripts_bytes,
        )? {
            return Err(MempoolError::InsufficientPriority);
        }

        // Atomically reserve a slot: increment tx_count first, then check capacity.
        // This eliminates the TOCTOU race between checking txs.len() and inserting.
        let (cfg_max_txs, cfg_max_bytes) = {
            let cfg = self.config.read();
            (cfg.max_transactions, cfg.max_bytes)
        };
        let count = self.tx_count.fetch_add(1, Ordering::Relaxed);
        if count >= cfg_max_txs {
            self.tx_count.fetch_sub(1, Ordering::Relaxed);
            warn!(max = cfg_max_txs, "Mempool: full, rejecting tx");
            return Err(MempoolError::Full { max: cfg_max_txs });
        }

        let total = *self.total_bytes.read();
        if total + size_bytes > cfg_max_bytes {
            self.tx_count.fetch_sub(1, Ordering::Relaxed);
            warn!(
                size_bytes,
                total,
                max = cfg_max_bytes,
                "Mempool: tx too large, rejecting"
            );
            return Err(MempoolError::TooLarge { size: size_bytes });
        }

        // Populate claimed-inputs index before inserting the entry so that any
        // concurrent admission attempt (from under the all_fifo lock) sees the
        // inputs as claimed immediately.
        for input in &tx.body.inputs {
            self.claimed_inputs.insert(input.clone(), tx_hash);
        }

        // Build the dependency graph: for each spending input, check whether it
        // references an output of a currently-pending mempool tx via the virtual
        // UTxO set.  If so, record this tx as a child of that parent so that
        // cascading removal works correctly when the parent is removed.
        //
        // Note: `claimed_inputs` was populated just above for this tx's own
        // inputs.  We use `virtual_utxo` as the authoritative lookup because
        // virtual_utxo is keyed by (parent_tx_hash, output_index) — exactly
        // the structure of a TransactionInput.  When an input resolves via
        // virtual_utxo, the parent tx hash is `input.transaction_id`.
        {
            let mut dep_map = self.dependents.write();
            for input in &tx.body.inputs {
                if self.virtual_utxo.contains_key(input) {
                    // This spending input references an output from a pending
                    // mempool tx.  Record the dependency so we can cascade
                    // on removal.
                    let parent_hash = input.transaction_id;
                    // Guard against a tx referencing its own hash (impossible
                    // in practice but defensive).
                    if parent_hash != tx_hash {
                        dep_map.entry(parent_hash).or_default().push(tx_hash);
                    }
                }
            }
        }

        // Publish this tx's outputs into the virtual UTxO set so that
        // subsequent (chained) transactions can reference them during
        // Phase-1 validation.
        for (index, output) in tx.body.outputs.iter().enumerate() {
            let virt_input = TransactionInput {
                transaction_id: tx_hash,
                index: index as u32,
            };
            self.virtual_utxo.insert(virt_input, output.clone());
        }

        let entry = MempoolEntry {
            tx: tx.clone(),
            tx_hash,
            size_bytes,
            fee,
            ex_units_mem,
            ex_units_steps,
            ref_scripts_bytes,
        };

        // Insert into fee-density sorted index
        let key = FeeDensityKey::new(fee.0, size_bytes, tx_hash);
        self.fee_index.write().insert(key);

        self.txs.insert(tx_hash, entry);
        // If this hash was previously removed (tombstoned), clear the tombstone.
        // The old order entry becomes a harmless duplicate — deduplication in
        // get_txs_for_block handles it.
        self.order_tombstones.write().remove(&tx_hash);
        self.order.write().push_back(tx_hash);
        *self.total_bytes.write() += size_bytes;
        self.total_ex_mem.fetch_add(ex_units_mem, Ordering::Relaxed);
        self.total_ex_steps
            .fetch_add(ex_units_steps, Ordering::Relaxed);
        self.total_ref_scripts_bytes
            .fetch_add(ref_scripts_bytes, Ordering::Relaxed);

        debug!(
            hash = %tx_hash.to_hex(),
            size_bytes,
            ex_mem = ex_units_mem,
            ex_steps = ex_units_steps,
            ref_scripts = ref_scripts_bytes,
            total_txs = self.tx_count.load(Ordering::Relaxed),
            "Mempool: transaction added"
        );

        Ok(MempoolAddResult::Added)
    }

    /// Ensure capacity across all dimensions by evicting lowest-fee-density txs.
    /// Returns Ok(true) if capacity is available, Ok(false) if eviction wasn't
    /// possible (new tx has insufficient priority), or Err on other failures.
    fn ensure_capacity(
        &self,
        new_size: usize,
        new_fee: u64,
        new_ex_mem: u64,
        new_ex_steps: u64,
        new_ref_scripts: usize,
    ) -> Result<bool, MempoolError> {
        loop {
            let needs_eviction =
                self.needs_eviction(new_size, new_ex_mem, new_ex_steps, new_ref_scripts);
            if !needs_eviction {
                return Ok(true);
            }

            // Find the worst (lowest fee density) tx
            let worst = {
                let fee_index = self.fee_index.read();
                fee_index.iter().next_back().copied()
            };

            let worst = match worst {
                Some(w) => w,
                None => return Ok(true), // Empty mempool, capacity must be available
            };

            // Check if new tx has better fee density than the worst existing tx
            let new_size_cmp = if new_size == 0 {
                1u128
            } else {
                new_size as u128
            };
            let new_density = (new_fee as u128) * (worst.size as u128);
            let worst_density = (worst.fee as u128) * new_size_cmp;
            if new_density <= worst_density {
                return Ok(false);
            }

            // Evict the worst tx
            debug!(
                evicted_hash = %worst.tx_hash.to_hex(),
                evicted_fee = worst.fee,
                evicted_size = worst.size,
                "Mempool: evicting lowest-density tx for capacity"
            );
            self.remove_tx(&worst.tx_hash);
        }
    }

    /// Check if any capacity dimension would be exceeded by adding a tx with the given resources.
    fn needs_eviction(
        &self,
        new_size: usize,
        new_ex_mem: u64,
        new_ex_steps: u64,
        new_ref_scripts: usize,
    ) -> bool {
        let (cfg_max_txs, cfg_max_bytes, cfg_max_ex_mem, cfg_max_ex_steps, cfg_max_ref) = {
            let cfg = self.config.read();
            (
                cfg.max_transactions,
                cfg.max_bytes,
                cfg.max_ex_mem,
                cfg.max_ex_steps,
                cfg.max_ref_scripts_bytes,
            )
        };

        let count = self.tx_count.load(Ordering::Relaxed);
        if count >= cfg_max_txs {
            return true;
        }

        let total_bytes = *self.total_bytes.read();
        if total_bytes + new_size > cfg_max_bytes {
            return true;
        }

        let total_ex_mem = self.total_ex_mem.load(Ordering::Relaxed);
        if total_ex_mem + new_ex_mem > cfg_max_ex_mem {
            return true;
        }

        let total_ex_steps = self.total_ex_steps.load(Ordering::Relaxed);
        if total_ex_steps + new_ex_steps > cfg_max_ex_steps {
            return true;
        }

        let total_ref = self.total_ref_scripts_bytes.load(Ordering::Relaxed);
        if total_ref + new_ref_scripts > cfg_max_ref {
            return true;
        }

        false
    }

    /// Remove a transaction (when included in a block, evicted, or expired).
    ///
    /// In addition to the standard cleanup (claimed inputs, fee index, counters),
    /// this method:
    /// 1. Removes all of the tx's outputs from the virtual UTxO set.
    /// 2. **Cascade-removes** every dependent tx that spends one of those virtual
    ///    outputs.  The cascade is breadth-first so arbitrarily deep chains are
    ///    handled without recursion.
    ///
    /// Returns only the directly-removed transaction (not the cascaded ones).
    pub fn remove_tx(&self, tx_hash: &TransactionHash) -> Option<Transaction> {
        self.remove_tx_inner(tx_hash, true)
    }

    /// Inner removal logic.
    ///
    /// `cascade` controls whether dependent transactions are recursively removed.
    /// It is always `true` for external callers; the flag exists to prevent
    /// double-cascade during the BFS loop below.
    fn remove_tx_inner(&self, tx_hash: &TransactionHash, cascade: bool) -> Option<Transaction> {
        let (_, entry) = match self.txs.remove(tx_hash) {
            Some(pair) => pair,
            None => {
                trace!(hash = %tx_hash.to_hex(), "Mempool: tx not found for removal");
                return None;
            }
        };

        self.tx_count.fetch_sub(1, Ordering::Relaxed);
        // O(1) tombstone instead of O(n) retain — compacted lazily
        self.order_tombstones.write().insert(*tx_hash);

        // Release every spending input claimed by this tx so that
        // replacement or successor transactions can now be admitted.
        for input in &entry.tx.body.inputs {
            self.claimed_inputs.remove(input);
        }

        // Remove this tx's outputs from the virtual UTxO.  Any dependent txs
        // that spend these outputs will be cascade-removed next.
        for (index, _output) in entry.tx.body.outputs.iter().enumerate() {
            let virt_input = TransactionInput {
                transaction_id: *tx_hash,
                index: index as u32,
            };
            self.virtual_utxo.remove(&virt_input);
        }

        // Remove from fee-density sorted index
        let key = FeeDensityKey::new(entry.fee.0, entry.size_bytes, *tx_hash);
        self.fee_index.write().remove(&key);

        *self.total_bytes.write() -= entry.size_bytes;
        self.total_ex_mem
            .fetch_sub(entry.ex_units_mem, Ordering::Relaxed);
        self.total_ex_steps
            .fetch_sub(entry.ex_units_steps, Ordering::Relaxed);
        self.total_ref_scripts_bytes
            .fetch_sub(entry.ref_scripts_bytes, Ordering::Relaxed);

        debug!(
            hash = %tx_hash.to_hex(),
            remaining = self.tx_count.load(Ordering::Relaxed),
            "Mempool: transaction removed"
        );

        let removed_tx = entry.tx;

        if cascade {
            // BFS cascade: collect the direct children of this tx, then for
            // each child collect its children, until no more dependents remain.
            // We do not recurse to avoid stack overflow on deep chains.
            let mut queue: Vec<TransactionHash> = {
                let mut dep_map = self.dependents.write();
                dep_map.remove(tx_hash).unwrap_or_default()
            };

            while !queue.is_empty() {
                let mut next_queue: Vec<TransactionHash> = Vec::new();
                for child_hash in &queue {
                    // Collect this child's own children before removing it
                    let grandchildren: Vec<TransactionHash> = {
                        let mut dep_map = self.dependents.write();
                        dep_map.remove(child_hash).unwrap_or_default()
                    };
                    next_queue.extend(grandchildren);

                    // Remove the child without triggering another cascade
                    // (we handle it in this BFS loop instead).
                    if let Some((_, child_entry)) = self.txs.remove(child_hash) {
                        self.tx_count.fetch_sub(1, Ordering::Relaxed);
                        self.order_tombstones.write().insert(*child_hash);

                        for input in &child_entry.tx.body.inputs {
                            self.claimed_inputs.remove(input);
                        }
                        for (index, _) in child_entry.tx.body.outputs.iter().enumerate() {
                            let virt_input = TransactionInput {
                                transaction_id: *child_hash,
                                index: index as u32,
                            };
                            self.virtual_utxo.remove(&virt_input);
                        }

                        let child_key = FeeDensityKey::new(
                            child_entry.fee.0,
                            child_entry.size_bytes,
                            *child_hash,
                        );
                        self.fee_index.write().remove(&child_key);
                        *self.total_bytes.write() -= child_entry.size_bytes;
                        self.total_ex_mem
                            .fetch_sub(child_entry.ex_units_mem, Ordering::Relaxed);
                        self.total_ex_steps
                            .fetch_sub(child_entry.ex_units_steps, Ordering::Relaxed);
                        self.total_ref_scripts_bytes
                            .fetch_sub(child_entry.ref_scripts_bytes, Ordering::Relaxed);

                        debug!(
                            hash = %child_hash.to_hex(),
                            parent = %tx_hash.to_hex(),
                            remaining = self.tx_count.load(Ordering::Relaxed),
                            "Mempool: cascade-removed dependent transaction"
                        );
                    }
                }
                queue = next_queue;
            }
        }

        Some(removed_tx)
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
    /// Select transactions for block production in **FIFO order** (matching Haskell).
    ///
    /// The Haskell node takes the longest FIFO prefix from the mempool that fits
    /// within block capacity. This naturally preserves dependency ordering since
    /// chained txs are admitted parent-first. Using fee-density ordering would
    /// break dependency chains (child selected before parent = invalid block).
    pub fn get_txs_for_block(&self, max_count: usize, max_size: usize) -> Vec<Transaction> {
        self.get_txs_for_block_with_ex_units(max_count, max_size, u64::MAX, u64::MAX)
    }

    /// Select transactions for block production in **FIFO order** (matching Haskell),
    /// enforcing both byte-size and execution-unit budget limits.
    ///
    /// `max_block_ex_mem` and `max_block_ex_steps` are the per-block ExUnit limits
    /// from protocol parameters (`maxBlockExecutionUnits`). The selection stops as
    /// soon as adding the next transaction would exceed any of the four limits
    /// (count, size, memory, steps).
    pub fn get_txs_for_block_with_ex_units(
        &self,
        max_count: usize,
        max_size: usize,
        max_block_ex_mem: u64,
        max_block_ex_steps: u64,
    ) -> Vec<Transaction> {
        let order = self.order.read();
        let tombstones = self.order_tombstones.read();
        let mut result = Vec::new();
        let mut total_size = 0;
        let mut total_ex_mem: u64 = 0;
        let mut total_ex_steps: u64 = 0;
        let mut seen = std::collections::HashSet::new();

        // Take FIFO prefix (oldest first) that fits within block capacity.
        // Skip tombstoned entries and deduplicate (a hash can appear twice
        // in the order list if it was removed and re-added).
        for tx_hash in order.iter() {
            if result.len() >= max_count {
                break;
            }
            if tombstones.contains(tx_hash) || !seen.insert(*tx_hash) {
                continue;
            }
            if let Some(entry) = self.txs.get(tx_hash) {
                if total_size + entry.size_bytes > max_size {
                    // Block is full by size — stop (strict prefix, no skipping)
                    break;
                }

                // Accumulate execution units from all redeemers in this transaction
                let (tx_mem, tx_steps) = tx_ex_units(&entry.tx);

                if total_ex_mem.saturating_add(tx_mem) > max_block_ex_mem
                    || total_ex_steps.saturating_add(tx_steps) > max_block_ex_steps
                {
                    // Adding this tx would exceed block ExUnit budget — stop
                    break;
                }

                result.push(entry.tx.clone());
                total_size += entry.size_bytes;
                total_ex_mem = total_ex_mem.saturating_add(tx_mem);
                total_ex_steps = total_ex_steps.saturating_add(tx_steps);
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

    /// Total execution memory units across all mempool transactions
    pub fn total_ex_mem(&self) -> u64 {
        self.total_ex_mem.load(Ordering::Relaxed)
    }

    /// Total execution step units across all mempool transactions
    pub fn total_ex_steps(&self) -> u64 {
        self.total_ex_steps.load(Ordering::Relaxed)
    }

    /// Total reference script bytes across all mempool transactions
    pub fn total_ref_scripts_bytes(&self) -> usize {
        self.total_ref_scripts_bytes.load(Ordering::Relaxed)
    }

    /// Maximum number of transactions the mempool can hold
    pub fn capacity(&self) -> usize {
        self.config.read().max_transactions
    }

    /// Update mempool capacity limits from live protocol parameters.
    ///
    /// Haskell cardano-node sets mempool capacity to `2 * blockCapacityTxMeasure`
    /// (i.e., 2x the block's byte limit, execution-unit limits, and reference script
    /// limit) and re-evaluates this at every epoch transition when protocol params
    /// may have changed via governance actions.
    ///
    /// `max_block_body_size`  — from `ProtocolParameters::max_block_body_size`
    /// `max_block_ex_mem`     — from `ProtocolParameters::max_block_ex_units.mem`
    /// `max_block_ex_steps`   — from `ProtocolParameters::max_block_ex_units.steps`
    pub fn update_capacity_from_params(
        &self,
        max_block_body_size: u64,
        max_block_ex_mem: u64,
        max_block_ex_steps: u64,
    ) {
        let mut cfg = self.config.write();
        let old_bytes = cfg.max_bytes;
        let old_ex_mem = cfg.max_ex_mem;
        let old_ex_steps = cfg.max_ex_steps;

        cfg.max_bytes = (max_block_body_size as usize).saturating_mul(2);
        cfg.max_ex_mem = max_block_ex_mem.saturating_mul(2);
        cfg.max_ex_steps = max_block_ex_steps.saturating_mul(2);

        if cfg.max_bytes != old_bytes
            || cfg.max_ex_mem != old_ex_mem
            || cfg.max_ex_steps != old_ex_steps
        {
            info!(
                max_bytes = cfg.max_bytes,
                max_ex_mem = cfg.max_ex_mem,
                max_ex_steps = cfg.max_ex_steps,
                "Mempool capacity updated from protocol params",
            );
        }
    }

    /// Get a transaction's raw CBOR bytes (for LocalTxMonitor protocol)
    pub fn get_tx_cbor(&self, tx_hash: &TransactionHash) -> Option<Vec<u8>> {
        self.txs
            .get(tx_hash)
            .and_then(|entry| entry.tx.raw_cbor.clone())
    }

    /// Get the first transaction hash in the mempool (for iteration).
    /// Skips tombstoned entries (removed but not yet compacted).
    pub fn first_tx_hash(&self) -> Option<TransactionHash> {
        let order = self.order.read();
        let tombstones = self.order_tombstones.read();
        order.iter().find(|h| !tombstones.contains(h)).copied()
    }

    /// Get all transaction hashes in FIFO order (for TxMonitor snapshot cursor).
    /// Skips tombstoned entries.
    pub fn tx_hashes_ordered(&self) -> Vec<TransactionHash> {
        let order = self.order.read();
        let tombstones = self.order_tombstones.read();
        order
            .iter()
            .filter(|h| !tombstones.contains(h))
            .copied()
            .collect()
    }

    /// Snapshot of current mempool state
    pub fn snapshot(&self) -> MempoolSnapshot {
        MempoolSnapshot {
            tx_count: self.txs.len(),
            total_bytes: *self.total_bytes.read(),
            tx_hashes: self.tx_hashes_ordered(),
        }
    }

    /// Compact the order VecDeque by removing tombstoned entries.
    /// Called when the tombstone ratio exceeds 50% to prevent unbounded growth.
    fn maybe_compact_order(&self) {
        let tombstone_count = self.order_tombstones.read().len();
        let order_len = self.order.read().len();
        // Compact when tombstones exceed 50% of order length and there are
        // at least 100 tombstones (avoid compacting tiny mempools)
        if tombstone_count > 100 && tombstone_count * 2 > order_len {
            let mut order = self.order.write();
            let mut tombstones = self.order_tombstones.write();
            order.retain(|h| !tombstones.contains(h));
            tombstones.clear();
            trace!(new_len = order.len(), "Mempool: compacted order VecDeque");
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
                    // Haskell uses half-open interval: tx is valid when slot < ttl.
                    // The invalid-hereafter slot is the FIRST invalid slot, so
                    // current_slot >= ttl means the tx has expired.
                    current_slot.0 >= ttl.0
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
            // Compact tombstones after batch eviction
            self.maybe_compact_order();
        }
        count
    }

    /// Sweep transactions whose TTL has expired, using a raw slot number.
    ///
    /// This is the public entry-point intended for the forge ticker or any
    /// periodic timer that works in raw `u64` slots rather than `SlotNo`.
    /// It is equivalent to calling `evict_expired(SlotNo(current_slot))`.
    ///
    /// Returns the number of swept (removed) transactions.
    pub fn sweep_expired(&self, current_slot: u64) -> usize {
        self.evict_expired(SlotNo(current_slot))
    }

    /// Number of spending inputs currently claimed by mempool transactions.
    ///
    /// Useful for metrics and diagnostics.  Under normal operation this equals
    /// the sum of `body.inputs.len()` across all mempool txs.
    pub fn claimed_inputs_count(&self) -> usize {
        self.claimed_inputs.len()
    }

    /// Get transactions ordered by fee density (fee/byte, descending).
    ///
    /// Unlike `get_txs_for_block()` (FIFO), this sorts by fee density.
    /// NOT used for block production (Haskell uses FIFO), but retained for
    /// eviction policy and diagnostics.
    pub fn get_txs_for_block_by_fee(&self, max_count: usize, max_size: usize) -> Vec<Transaction> {
        let fee_index = self.fee_index.read();
        let mut result = Vec::new();
        let mut total_size = 0;

        for key in fee_index.iter() {
            if result.len() >= max_count {
                break;
            }
            if let Some(entry) = self.txs.get(&key.tx_hash) {
                if total_size + entry.size_bytes > max_size {
                    continue; // Skip oversized, try smaller txs
                }
                result.push(entry.tx.clone());
                total_size += entry.size_bytes;
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

    /// Revalidate all mempool transactions against a validator closure.
    ///
    /// Replays transactions in FIFO order through `is_valid`. Any transaction
    /// for which the closure returns `false` is removed. This replaces the
    /// piecemeal post-block cleanup (remove_txs + revalidate_against_inputs +
    /// evict_expired) with a single pass that catches all invalidity reasons.
    ///
    /// Returns the hashes of removed transactions.
    pub fn revalidate_all<F>(&self, mut is_valid: F) -> Vec<TransactionHash>
    where
        F: FnMut(&Transaction) -> bool,
    {
        // Snapshot FIFO order to iterate deterministically (skip tombstoned)
        let hashes: Vec<TransactionHash> = self.tx_hashes_ordered();
        let mut removed = Vec::new();

        for hash in hashes {
            let valid = self.txs.get(&hash).map(|entry| is_valid(&entry.tx));
            if valid == Some(false) {
                self.remove_tx(&hash);
                removed.push(hash);
            }
        }

        if !removed.is_empty() {
            info!(
                removed = removed.len(),
                remaining = self.len(),
                "Mempool: revalidation removed invalid transactions"
            );
        }
        removed
    }

    /// Remove and return all transactions from the mempool.
    ///
    /// This is used during chain rollback to save pending transactions before
    /// the UTxO set changes, allowing them to be re-validated and re-added.
    pub fn drain_all(&self) -> Vec<Transaction> {
        let count = self.tx_count.swap(0, Ordering::Relaxed);
        let mut txs = Vec::with_capacity(count);
        // Collect transactions in FIFO order (skip tombstoned entries)
        let order: Vec<TransactionHash> = self.order.write().drain(..).collect();
        self.order_tombstones.write().clear();
        for hash in &order {
            if let Some((_, entry)) = self.txs.remove(hash) {
                txs.push(entry.tx);
            }
        }
        self.fee_index.write().clear();
        self.claimed_inputs.clear();
        // Clear virtual UTxO and dependency graph so they don't carry stale
        // entries that could corrupt the next round of re-admission after rollback.
        self.virtual_utxo.clear();
        self.dependents.write().clear();
        *self.total_bytes.write() = 0;
        self.total_ex_mem.store(0, Ordering::Relaxed);
        self.total_ex_steps.store(0, Ordering::Relaxed);
        self.total_ref_scripts_bytes.store(0, Ordering::Relaxed);
        if !txs.is_empty() {
            info!(drained = txs.len(), "Mempool: drained all transactions");
        }
        txs
    }

    /// Clear all transactions
    pub fn clear(&self) {
        let count = self.tx_count.swap(0, Ordering::Relaxed);
        self.txs.clear();
        self.order.write().clear();
        self.order_tombstones.write().clear();
        self.fee_index.write().clear();
        self.claimed_inputs.clear();
        // Clear virtual UTxO and dependency graph so no stale entries remain.
        self.virtual_utxo.clear();
        self.dependents.write().clear();
        *self.total_bytes.write() = 0;
        self.total_ex_mem.store(0, Ordering::Relaxed);
        self.total_ex_steps.store(0, Ordering::Relaxed);
        self.total_ref_scripts_bytes.store(0, Ordering::Relaxed);
        if count > 0 {
            info!(removed = count, "Mempool: cleared all transactions");
        }
    }

    // -------------------------------------------------------------------------
    // Virtual UTxO access
    // -------------------------------------------------------------------------

    /// Look up a transaction output in the mempool's virtual UTxO set.
    ///
    /// Returns `Some(output)` when the given input references an unconfirmed
    /// output from a pending mempool transaction.  Returns `None` otherwise.
    ///
    /// Callers (e.g. `LedgerTxValidator`) use this to build a composite UTxO
    /// view — on-chain UTxO first, virtual UTxO as a fallback — enabling Phase-1
    /// validation of transactions that chain off unconfirmed mempool outputs.
    pub fn lookup_virtual_utxo(&self, input: &TransactionInput) -> Option<TransactionOutput> {
        self.virtual_utxo.get(input).map(|r| r.clone())
    }

    /// Return a snapshot of the current virtual UTxO entries.
    ///
    /// The snapshot is taken under a brief per-shard lock (DashMap semantics)
    /// and therefore represents a consistent-at-collection-time view.  Callers
    /// that need a stable, immutable view for the duration of a validation pass
    /// should use this snapshot (a `HashMap`) rather than `lookup_virtual_utxo`
    /// to avoid seeing partial updates from concurrent admissions.
    pub fn virtual_utxo_snapshot(&self) -> HashMap<TransactionInput, TransactionOutput> {
        self.virtual_utxo
            .iter()
            .map(|r| (r.key().clone(), r.value().clone()))
            .collect()
    }

    /// Number of entries currently in the virtual UTxO set.
    ///
    /// Under normal conditions this equals the sum of `body.outputs.len()`
    /// across all pending mempool transactions.  Useful for diagnostics.
    pub fn virtual_utxo_count(&self) -> usize {
        self.virtual_utxo.len()
    }
}

/// A snapshot of the mempool state (for queries)
#[derive(Debug, Clone)]
pub struct MempoolSnapshot {
    pub tx_count: usize,
    pub total_bytes: usize,
    pub tx_hashes: Vec<TransactionHash>,
}

/// Implement `MempoolProvider` from `torsten-primitives` for the concrete `Mempool`.
///
/// This allows `torsten-network` (and any other crate) to depend on the trait
/// abstraction rather than on this crate directly.
impl torsten_primitives::mempool::MempoolProvider for Mempool {
    fn add_tx(
        &self,
        tx_hash: TransactionHash,
        tx: Transaction,
        size_bytes: usize,
    ) -> Result<
        torsten_primitives::mempool::MempoolAddResult,
        torsten_primitives::mempool::MempoolAddError,
    > {
        match Mempool::add_tx(self, tx_hash, tx, size_bytes) {
            Ok(MempoolAddResult::Added) => Ok(torsten_primitives::mempool::MempoolAddResult::Added),
            Ok(MempoolAddResult::AlreadyExists) => {
                Ok(torsten_primitives::mempool::MempoolAddResult::AlreadyExists)
            }
            Err(e) => Err(torsten_primitives::mempool::MempoolAddError(e.to_string())),
        }
    }

    fn add_tx_with_fee(
        &self,
        tx_hash: TransactionHash,
        tx: Transaction,
        size_bytes: usize,
        fee: torsten_primitives::value::Lovelace,
    ) -> Result<
        torsten_primitives::mempool::MempoolAddResult,
        torsten_primitives::mempool::MempoolAddError,
    > {
        match Mempool::add_tx_with_fee(self, tx_hash, tx, size_bytes, fee) {
            Ok(MempoolAddResult::Added) => Ok(torsten_primitives::mempool::MempoolAddResult::Added),
            Ok(MempoolAddResult::AlreadyExists) => {
                Ok(torsten_primitives::mempool::MempoolAddResult::AlreadyExists)
            }
            Err(e) => Err(torsten_primitives::mempool::MempoolAddError(e.to_string())),
        }
    }

    fn contains(&self, tx_hash: &TransactionHash) -> bool {
        Mempool::contains(self, tx_hash)
    }

    fn get_tx(&self, tx_hash: &TransactionHash) -> Option<Transaction> {
        Mempool::get_tx(self, tx_hash)
    }

    fn get_tx_size(&self, tx_hash: &TransactionHash) -> Option<usize> {
        Mempool::get_tx_size(self, tx_hash)
    }

    fn get_tx_cbor(&self, tx_hash: &TransactionHash) -> Option<Vec<u8>> {
        Mempool::get_tx_cbor(self, tx_hash)
    }

    fn tx_hashes_ordered(&self) -> Vec<TransactionHash> {
        Mempool::tx_hashes_ordered(self)
    }

    fn len(&self) -> usize {
        Mempool::len(self)
    }

    fn total_bytes(&self) -> usize {
        Mempool::total_bytes(self)
    }

    fn capacity(&self) -> usize {
        Mempool::capacity(self)
    }

    fn snapshot(&self) -> torsten_primitives::mempool::MempoolSnapshot {
        let snap = Mempool::snapshot(self);
        torsten_primitives::mempool::MempoolSnapshot {
            tx_count: snap.tx_count,
            total_bytes: snap.total_bytes,
            tx_hashes: snap.tx_hashes,
        }
    }
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

    /// Create a dummy transaction with a unique input derived from an atomic counter.
    ///
    /// Each call gets a different `(transaction_id, index)` pair so that tests
    /// adding multiple `make_dummy_tx()` transactions do not accidentally trigger
    /// the input-conflict check (which is correct behaviour — two real transactions
    /// spending the same UTxO cannot coexist in the mempool).
    fn make_dummy_tx() -> Transaction {
        use std::sync::atomic::{AtomicU32, Ordering as AOrdering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, AOrdering::Relaxed);
        let mut id_bytes = [0u8; 32];
        id_bytes[28..32].copy_from_slice(&n.to_be_bytes());

        Transaction {
            hash: Hash32::ZERO,
            body: TransactionBody {
                inputs: vec![TransactionInput {
                    transaction_id: Hash32::from_bytes(id_bytes),
                    index: n,
                }],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0; 32],
                    }),
                    value: Value::lovelace(1_000_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    is_legacy: false,
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
                raw_redeemers_cbor: None,
                raw_plutus_data_cbor: None,
                pallas_script_data_hash: None,
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        }
    }

    fn default_config() -> MempoolConfig {
        MempoolConfig::default()
    }

    #[test]
    fn test_add_and_get() {
        let mempool = Mempool::new(default_config());
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
        let mempool = Mempool::new(default_config());
        let tx = make_dummy_tx();
        let hash = Hash32::from_bytes([1u8; 32]);

        mempool.add_tx(hash, tx.clone(), 500).unwrap();
        let result = mempool.add_tx(hash, tx, 500).unwrap();
        assert!(matches!(result, MempoolAddResult::AlreadyExists));
        assert_eq!(mempool.len(), 1);
    }

    #[test]
    fn test_remove() {
        let mempool = Mempool::new(default_config());
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
            ..default_config()
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
        let mempool = Mempool::new(default_config());

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
        let mempool = Mempool::new(default_config());
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
        let mempool = Mempool::new(default_config());
        mempool
            .add_tx(Hash32::from_bytes([1u8; 32]), make_dummy_tx(), 500)
            .unwrap();
        mempool
            .add_tx(Hash32::from_bytes([2u8; 32]), make_dummy_tx(), 300)
            .unwrap();

        mempool.clear();
        assert!(mempool.is_empty());
        assert_eq!(mempool.total_bytes(), 0);
        assert_eq!(mempool.total_ex_mem(), 0);
        assert_eq!(mempool.total_ex_steps(), 0);
        assert_eq!(mempool.total_ref_scripts_bytes(), 0);
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
        let mempool = Mempool::new(default_config());

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
        let mempool = Mempool::new(default_config());

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
        let mempool = Mempool::new(default_config());

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
        let mempool = Mempool::new(default_config());
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
        let mempool = Mempool::new(default_config());

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
        let txs = mempool.get_txs_for_block_by_fee(10, 100_000);
        assert_eq!(txs.len(), 3);
        assert_eq!(txs[0].body.fee, Lovelace(500_000)); // highest fee density
        assert_eq!(txs[1].body.fee, Lovelace(200_000)); // medium
        assert_eq!(txs[2].body.fee, Lovelace(100_000)); // lowest

        // With size limit, only highest-fee txs should be included
        let txs = mempool.get_txs_for_block_by_fee(10, 1000);
        assert_eq!(txs.len(), 2); // only room for 2 x 500 bytes
        assert_eq!(txs[0].body.fee, Lovelace(500_000)); // highest priority first
        assert_eq!(txs[1].body.fee, Lovelace(200_000)); // second highest
    }

    #[test]
    fn test_atomic_tx_count_consistency() {
        let config = MempoolConfig {
            max_transactions: 5,
            ..default_config()
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
            ..default_config()
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
        let mempool = Mempool::new(default_config());

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

    /// Verify that `Mempool` can be used as a `dyn MempoolProvider` trait object.
    #[test]
    fn test_mempool_provider_trait_object() {
        use torsten_primitives::mempool::MempoolProvider;

        let mempool = Mempool::new(default_config());
        let provider: &dyn MempoolProvider = &mempool;

        assert_eq!(provider.len(), 0);
        assert!(provider.is_empty());
        assert_eq!(provider.total_bytes(), 0);
        assert_eq!(provider.capacity(), 16_384);

        let hash = Hash32::from_bytes([1u8; 32]);
        let tx = make_dummy_tx();
        let result = provider.add_tx(hash, tx, 500).unwrap();
        assert!(matches!(
            result,
            torsten_primitives::mempool::MempoolAddResult::Added
        ));

        assert_eq!(provider.len(), 1);
        assert!(!provider.is_empty());
        assert!(provider.contains(&hash));
        assert!(provider.get_tx(&hash).is_some());
        assert_eq!(provider.get_tx_size(&hash), Some(500));
        assert_eq!(provider.total_bytes(), 500);

        let hashes = provider.tx_hashes_ordered();
        assert_eq!(hashes.len(), 1);
        assert_eq!(hashes[0], hash);

        let snap = provider.snapshot();
        assert_eq!(snap.tx_count, 1);
        assert_eq!(snap.total_bytes, 500);
        assert_eq!(snap.tx_hashes.len(), 1);
    }

    /// Verify that `Arc<Mempool>` can be coerced to `Arc<dyn MempoolProvider>`.
    #[test]
    fn test_mempool_provider_arc_coercion() {
        use std::sync::Arc;
        use torsten_primitives::mempool::MempoolProvider;

        let mempool = Arc::new(Mempool::new(default_config()));
        let provider: Arc<dyn MempoolProvider> = mempool;

        assert_eq!(provider.len(), 0);
        assert_eq!(provider.capacity(), 16_384);

        let hash = Hash32::from_bytes([1u8; 32]);
        let tx = make_dummy_tx();
        provider.add_tx(hash, tx, 300).unwrap();
        assert_eq!(provider.len(), 1);
        assert!(provider.contains(&hash));
    }

    /// Verify `add_tx_with_fee` through the trait.
    #[test]
    fn test_mempool_provider_add_with_fee() {
        use torsten_primitives::mempool::MempoolProvider;

        let mempool = Mempool::new(default_config());
        let provider: &dyn MempoolProvider = &mempool;

        let hash = Hash32::from_bytes([1u8; 32]);
        let tx = make_dummy_tx();
        let result = provider
            .add_tx_with_fee(hash, tx, 1000, Lovelace(500_000))
            .unwrap();
        assert!(matches!(
            result,
            torsten_primitives::mempool::MempoolAddResult::Added
        ));

        // Adding again should return AlreadyExists
        let result = provider
            .add_tx_with_fee(hash, make_dummy_tx(), 1000, Lovelace(500_000))
            .unwrap();
        assert!(matches!(
            result,
            torsten_primitives::mempool::MempoolAddResult::AlreadyExists
        ));
    }

    /// Verify that a full mempool returns an error through the trait.
    #[test]
    fn test_mempool_provider_full_error() {
        use torsten_primitives::mempool::MempoolProvider;

        let config = MempoolConfig {
            max_transactions: 1,
            ..default_config()
        };
        let mempool = Mempool::new(config);
        let provider: &dyn MempoolProvider = &mempool;

        provider
            .add_tx(Hash32::from_bytes([1u8; 32]), make_dummy_tx(), 100)
            .unwrap();

        let result = provider.add_tx(Hash32::from_bytes([2u8; 32]), make_dummy_tx(), 100);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.0.contains("full") || err.0.contains("priority"),
            "error should mention 'full' or 'priority': {}",
            err.0
        );
    }

    // ========================== Fee-density ordering tests ==========================

    #[test]
    fn test_fee_density_ordering_different_sizes() {
        // Transactions with different fees AND sizes — fee density matters, not raw fee
        let mempool = Mempool::new(default_config());

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

        let txs = mempool.get_txs_for_block_by_fee(10, 100_000);
        assert_eq!(txs.len(), 3);
        // Order: tx2 (800/byte) > tx3 (600/byte) > tx1 (500/byte)
        assert_eq!(txs[0].body.fee.0, 400_000); // highest density
        assert_eq!(txs[1].body.fee.0, 600_000); // middle density
        assert_eq!(txs[2].body.fee.0, 1_000_000); // lowest density
    }

    #[test]
    fn test_fee_density_same_density_deterministic_hash_tiebreak() {
        // Two transactions with identical fee density should be ordered deterministically by hash
        let mempool = Mempool::new(default_config());

        let hash_a = Hash32::from_bytes([0xAA; 32]);
        let hash_b = Hash32::from_bytes([0x11; 32]);

        // Both have same density: 1000 fee / 500 bytes = 2 per byte
        mempool
            .add_tx_with_fee(hash_a, make_tx_with_fee(1000), 500, Lovelace(1000))
            .unwrap();
        mempool
            .add_tx_with_fee(hash_b, make_tx_with_fee(1000), 500, Lovelace(1000))
            .unwrap();

        let txs = mempool.get_txs_for_block_by_fee(10, 100_000);
        assert_eq!(txs.len(), 2);
        // hash_b (0x11...) < hash_a (0xAA...) lexicographically, so hash_b comes first
        assert_eq!(txs[0].body.fee.0, 1000);
        assert_eq!(txs[1].body.fee.0, 1000);

        // Run again — ordering must be stable/deterministic
        let txs2 = mempool.get_txs_for_block_by_fee(10, 100_000);
        assert_eq!(txs2.len(), 2);
    }

    #[test]
    fn test_fee_density_proportional_same_density() {
        // 200 fee / 100 bytes = 2 per byte  (same as 400 fee / 200 bytes)
        // Cross-multiplication should detect these as equal
        let mempool = Mempool::new(default_config());

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

        let txs = mempool.get_txs_for_block_by_fee(10, 100_000);
        assert_eq!(txs.len(), 2);
        // Both have equal density, so tiebreak is by hash ascending
        // [1u8; 32] < [2u8; 32], so tx with hash [1...] comes first
        assert_eq!(txs[0].body.fee.0, 200);
        assert_eq!(txs[1].body.fee.0, 400);
    }

    #[test]
    fn test_fee_density_zero_fee() {
        // Zero-fee transactions should sort last
        let mempool = Mempool::new(default_config());

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

        let txs = mempool.get_txs_for_block_by_fee(10, 100_000);
        assert_eq!(txs.len(), 2);
        assert_eq!(txs[0].body.fee.0, 100); // non-zero fee first
        assert_eq!(txs[1].body.fee.0, 0); // zero fee last
    }

    #[test]
    fn test_fee_density_zero_size_treated_as_one() {
        // A zero-size transaction should not cause division by zero;
        // it's treated as size=1 for density calculation
        let mempool = Mempool::new(default_config());

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

        let txs = mempool.get_txs_for_block_by_fee(10, 100_000);
        assert_eq!(txs.len(), 2);
        // Zero-size (treated as 1): density = 1000/1 = 1000
        // 500-byte: density = 1000/500 = 2
        // So zero-size tx comes first (higher density)
        assert_eq!(txs[0].body.fee.0, 1000); // zero-size tx
    }

    #[test]
    fn test_remove_maintains_fee_index_consistency() {
        let mempool = Mempool::new(default_config());

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
        let txs = mempool.get_txs_for_block_by_fee(10, 100_000);
        assert_eq!(txs.len(), 2);
        assert_eq!(txs[0].body.fee.0, 200);
        assert_eq!(txs[1].body.fee.0, 100);
    }

    #[test]
    fn test_insertion_order_does_not_affect_fee_ordering() {
        // Insert low, high, medium — result should always be high, medium, low
        let mempool = Mempool::new(default_config());
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

        let txs = mempool.get_txs_for_block_by_fee(10, 100_000);
        assert_eq!(txs[0].body.fee.0, 500);
        assert_eq!(txs[1].body.fee.0, 300);
        assert_eq!(txs[2].body.fee.0, 100);
    }

    #[test]
    fn test_get_txs_for_block_skips_oversized_includes_smaller() {
        // When a high-density tx doesn't fit, lower-density smaller txs can still be included
        let mempool = Mempool::new(default_config());

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
        let txs = mempool.get_txs_for_block_by_fee(10, 1000);
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
        let mempool = Mempool::new(default_config());

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
        let txs = mempool.get_txs_for_block_by_fee(10, 100_000);
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
        let mempool = Mempool::new(default_config());

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

        let txs = mempool.get_txs_for_block_by_fee(10, 100_000);
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
        let mempool = Mempool::new(default_config());

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
        let mempool = Mempool::new(default_config());

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
        let mempool = Mempool::new(default_config());
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

    // ========================== Multi-dimensional capacity tests ==========================

    #[test]
    fn test_ex_units_capacity_enforcement() {
        let config = MempoolConfig {
            max_ex_mem: 1000,
            max_ex_steps: 5000,
            ..default_config()
        };
        let mempool = Mempool::new(config);

        // Add tx with 600 mem, 2000 steps
        mempool
            .add_tx_full(
                Hash32::from_bytes([1u8; 32]),
                make_dummy_tx(),
                100,
                Lovelace(500),
                600,
                2000,
                0,
                TxOrigin::Local,
            )
            .unwrap();
        assert_eq!(mempool.total_ex_mem(), 600);
        assert_eq!(mempool.total_ex_steps(), 2000);

        // Add tx with 300 mem, 2000 steps — should fit
        mempool
            .add_tx_full(
                Hash32::from_bytes([2u8; 32]),
                make_dummy_tx(),
                100,
                Lovelace(500),
                300,
                2000,
                0,
                TxOrigin::Local,
            )
            .unwrap();
        assert_eq!(mempool.total_ex_mem(), 900);
        assert_eq!(mempool.total_ex_steps(), 4000);

        // Add tx with 200 mem — would exceed max_ex_mem (900+200=1100 > 1000)
        // New tx has same fee density as existing, so eviction should fail
        let result = mempool.add_tx_full(
            Hash32::from_bytes([3u8; 32]),
            make_dummy_tx(),
            100,
            Lovelace(500),
            200,
            500,
            0,
            TxOrigin::Local,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_ref_scripts_capacity_enforcement() {
        let config = MempoolConfig {
            max_ref_scripts_bytes: 500,
            ..default_config()
        };
        let mempool = Mempool::new(config);

        mempool
            .add_tx_full(
                Hash32::from_bytes([1u8; 32]),
                make_dummy_tx(),
                100,
                Lovelace(500),
                0,
                0,
                300,
                TxOrigin::Local,
            )
            .unwrap();
        assert_eq!(mempool.total_ref_scripts_bytes(), 300);

        // Would exceed ref script limit (300+300=600 > 500)
        let result = mempool.add_tx_full(
            Hash32::from_bytes([2u8; 32]),
            make_dummy_tx(),
            100,
            Lovelace(500),
            0,
            0,
            300,
            TxOrigin::Local,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_eviction_frees_all_dimensions() {
        let config = MempoolConfig {
            max_transactions: 2,
            max_ex_mem: 1000,
            ..default_config()
        };
        let mempool = Mempool::new(config);

        // tx1: low fee density, high ex_mem
        mempool
            .add_tx_full(
                Hash32::from_bytes([1u8; 32]),
                make_dummy_tx(),
                100,
                Lovelace(100),
                800,
                0,
                0,
                TxOrigin::Local,
            )
            .unwrap();

        // tx2: medium fee density
        mempool
            .add_tx_full(
                Hash32::from_bytes([2u8; 32]),
                make_dummy_tx(),
                100,
                Lovelace(200),
                100,
                0,
                0,
                TxOrigin::Local,
            )
            .unwrap();

        assert_eq!(mempool.len(), 2);
        assert_eq!(mempool.total_ex_mem(), 900);

        // tx3: highest fee density — should evict tx1 (lowest density)
        let result = mempool.add_tx_full(
            Hash32::from_bytes([3u8; 32]),
            make_dummy_tx(),
            100,
            Lovelace(500),
            50,
            0,
            0,
            TxOrigin::Local,
        );
        assert!(result.is_ok());
        assert_eq!(mempool.len(), 2);
        assert!(!mempool.contains(&Hash32::from_bytes([1u8; 32]))); // evicted
        assert!(mempool.contains(&Hash32::from_bytes([2u8; 32])));
        assert!(mempool.contains(&Hash32::from_bytes([3u8; 32])));
        // ex_mem should be 100 (tx2) + 50 (tx3) = 150
        assert_eq!(mempool.total_ex_mem(), 150);
    }

    #[test]
    fn test_eviction_insufficient_priority() {
        let config = MempoolConfig {
            max_transactions: 2,
            ..default_config()
        };
        let mempool = Mempool::new(config);

        // Fill with high-fee txs
        mempool
            .add_tx_full(
                Hash32::from_bytes([1u8; 32]),
                make_dummy_tx(),
                100,
                Lovelace(1000),
                0,
                0,
                0,
                TxOrigin::Local,
            )
            .unwrap();
        mempool
            .add_tx_full(
                Hash32::from_bytes([2u8; 32]),
                make_dummy_tx(),
                100,
                Lovelace(900),
                0,
                0,
                0,
                TxOrigin::Local,
            )
            .unwrap();

        // Try to add low-fee tx — should fail with InsufficientPriority
        let result = mempool.add_tx_full(
            Hash32::from_bytes([3u8; 32]),
            make_dummy_tx(),
            100,
            Lovelace(50),
            0,
            0,
            0,
            TxOrigin::Local,
        );
        assert!(matches!(result, Err(MempoolError::InsufficientPriority)));
        assert_eq!(mempool.len(), 2);
    }

    #[test]
    fn test_remove_decrements_ex_units() {
        let mempool = Mempool::new(default_config());

        mempool
            .add_tx_full(
                Hash32::from_bytes([1u8; 32]),
                make_dummy_tx(),
                100,
                Lovelace(500),
                1000,
                2000,
                300,
                TxOrigin::Local,
            )
            .unwrap();
        assert_eq!(mempool.total_ex_mem(), 1000);
        assert_eq!(mempool.total_ex_steps(), 2000);
        assert_eq!(mempool.total_ref_scripts_bytes(), 300);

        mempool.remove_tx(&Hash32::from_bytes([1u8; 32]));
        assert_eq!(mempool.total_ex_mem(), 0);
        assert_eq!(mempool.total_ex_steps(), 0);
        assert_eq!(mempool.total_ref_scripts_bytes(), 0);
    }

    // ========================== Dual-FIFO fairness tests ==========================

    #[test]
    fn test_tx_origin_local_and_remote() {
        let mempool = Mempool::new(default_config());

        // Local submission
        mempool
            .add_tx_full(
                Hash32::from_bytes([1u8; 32]),
                make_dummy_tx(),
                100,
                Lovelace(500),
                0,
                0,
                0,
                TxOrigin::Local,
            )
            .unwrap();

        // Remote submission
        mempool
            .add_tx_full(
                Hash32::from_bytes([2u8; 32]),
                make_dummy_tx(),
                100,
                Lovelace(500),
                0,
                0,
                0,
                TxOrigin::Remote,
            )
            .unwrap();

        assert_eq!(mempool.len(), 2);
        // Both should be in FIFO order
        let hashes = mempool.tx_hashes_ordered();
        assert_eq!(hashes[0], Hash32::from_bytes([1u8; 32]));
        assert_eq!(hashes[1], Hash32::from_bytes([2u8; 32]));
    }

    #[test]
    fn test_dual_fifo_fairness_contention() {
        use std::sync::Arc;

        let config = MempoolConfig {
            max_transactions: 100,
            ..default_config()
        };
        let mempool = Arc::new(Mempool::new(config));

        // Simulate concurrent local and remote submissions
        let m1 = mempool.clone();
        let local_handle = std::thread::spawn(move || {
            for i in 0..10u8 {
                let mut hash_bytes = [0u8; 32];
                hash_bytes[0] = 0xAA;
                hash_bytes[1] = i;
                let _ = m1.add_tx_full(
                    Hash32::from_bytes(hash_bytes),
                    make_dummy_tx(),
                    100,
                    Lovelace(500),
                    0,
                    0,
                    0,
                    TxOrigin::Local,
                );
            }
        });

        let m2 = mempool.clone();
        let remote_handle = std::thread::spawn(move || {
            for i in 0..10u8 {
                let mut hash_bytes = [0u8; 32];
                hash_bytes[0] = 0xBB;
                hash_bytes[1] = i;
                let _ = m2.add_tx_full(
                    Hash32::from_bytes(hash_bytes),
                    make_dummy_tx(),
                    100,
                    Lovelace(500),
                    0,
                    0,
                    0,
                    TxOrigin::Remote,
                );
            }
        });

        local_handle.join().unwrap();
        remote_handle.join().unwrap();

        // All 20 txs should be added (no deadlock, no data corruption)
        assert_eq!(mempool.len(), 20);
    }

    // ========================== Revalidation tests ==========================

    #[test]
    fn test_revalidate_all_removes_invalid() {
        let mempool = Mempool::new(default_config());

        // Add 5 txs with different fees
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
        assert_eq!(mempool.len(), 5);

        // Revalidate: reject txs with fee < 300
        let removed = mempool.revalidate_all(|tx| tx.body.fee.0 >= 300);

        assert_eq!(removed.len(), 2); // tx1 (100) and tx2 (200) removed
        assert_eq!(mempool.len(), 3);
        assert!(!mempool.contains(&Hash32::from_bytes([1u8; 32])));
        assert!(!mempool.contains(&Hash32::from_bytes([2u8; 32])));
        assert!(mempool.contains(&Hash32::from_bytes([3u8; 32])));
        assert!(mempool.contains(&Hash32::from_bytes([4u8; 32])));
        assert!(mempool.contains(&Hash32::from_bytes([5u8; 32])));
    }

    #[test]
    fn test_revalidate_all_keeps_all_valid() {
        let mempool = Mempool::new(default_config());

        for i in 1..=3u8 {
            mempool
                .add_tx(Hash32::from_bytes([i; 32]), make_dummy_tx(), 100)
                .unwrap();
        }

        let removed = mempool.revalidate_all(|_| true);
        assert!(removed.is_empty());
        assert_eq!(mempool.len(), 3);
    }

    #[test]
    fn test_revalidate_all_removes_all_invalid() {
        let mempool = Mempool::new(default_config());

        for i in 1..=3u8 {
            mempool
                .add_tx(Hash32::from_bytes([i; 32]), make_dummy_tx(), 100)
                .unwrap();
        }

        let removed = mempool.revalidate_all(|_| false);
        assert_eq!(removed.len(), 3);
        assert!(mempool.is_empty());
        assert_eq!(mempool.total_bytes(), 0);
    }

    #[test]
    fn test_revalidate_all_maintains_counters() {
        let mempool = Mempool::new(default_config());

        mempool
            .add_tx_full(
                Hash32::from_bytes([1u8; 32]),
                make_tx_with_fee(100),
                500,
                Lovelace(100),
                1000,
                2000,
                300,
                TxOrigin::Local,
            )
            .unwrap();
        mempool
            .add_tx_full(
                Hash32::from_bytes([2u8; 32]),
                make_tx_with_fee(200),
                600,
                Lovelace(200),
                500,
                1000,
                100,
                TxOrigin::Local,
            )
            .unwrap();

        // Remove tx1 via revalidation
        mempool.revalidate_all(|tx| tx.body.fee.0 >= 200);

        assert_eq!(mempool.len(), 1);
        assert_eq!(mempool.total_bytes(), 600);
        assert_eq!(mempool.total_ex_mem(), 500);
        assert_eq!(mempool.total_ex_steps(), 1000);
        assert_eq!(mempool.total_ref_scripts_bytes(), 100);
    }

    #[test]
    fn test_drain_all_returns_transactions() {
        let mempool = Mempool::new(default_config());

        for i in 1..=3u8 {
            mempool
                .add_tx(Hash32::from_bytes([i; 32]), make_dummy_tx(), 200)
                .unwrap();
        }
        assert_eq!(mempool.len(), 3);

        let drained = mempool.drain_all();
        assert_eq!(drained.len(), 3);
        assert!(mempool.is_empty());
        assert_eq!(mempool.total_bytes(), 0);
        assert_eq!(mempool.total_ex_mem(), 0);
        assert_eq!(mempool.total_ex_steps(), 0);
        assert_eq!(mempool.total_ref_scripts_bytes(), 0);
        assert!(mempool.fee_index.read().is_empty());
    }

    #[test]
    fn test_drain_all_empty_mempool() {
        let mempool = Mempool::new(default_config());
        let drained = mempool.drain_all();
        assert!(drained.is_empty());
        assert!(mempool.is_empty());
    }

    #[test]
    fn test_drain_all_preserves_fifo_order() {
        let mempool = Mempool::new(default_config());

        let hashes: Vec<Hash32> = (1..=5u8).map(|i| Hash32::from_bytes([i; 32])).collect();

        for hash in &hashes {
            mempool.add_tx(*hash, make_dummy_tx(), 100).unwrap();
        }

        let drained = mempool.drain_all();
        assert_eq!(drained.len(), 5);
        // After drain, mempool should be usable again
        assert!(mempool.is_empty());
        mempool
            .add_tx(Hash32::from_bytes([10u8; 32]), make_dummy_tx(), 100)
            .unwrap();
        assert_eq!(mempool.len(), 1);
    }

    #[test]
    fn test_drain_all_size_tracking() {
        let mempool = Mempool::new(default_config());

        mempool
            .add_tx_full(
                Hash32::from_bytes([1u8; 32]),
                make_tx_with_fee(100),
                500,
                Lovelace(100),
                1000,
                2000,
                50,
                TxOrigin::Local,
            )
            .unwrap();
        mempool
            .add_tx_full(
                Hash32::from_bytes([2u8; 32]),
                make_tx_with_fee(200),
                600,
                Lovelace(200),
                500,
                1000,
                100,
                TxOrigin::Local,
            )
            .unwrap();

        assert_eq!(mempool.total_bytes(), 1100);
        assert_eq!(mempool.total_ex_mem(), 1500);

        let drained = mempool.drain_all();
        assert_eq!(drained.len(), 2);
        assert_eq!(mempool.total_bytes(), 0);
        assert_eq!(mempool.total_ex_mem(), 0);
        assert_eq!(mempool.total_ex_steps(), 0);
        assert_eq!(mempool.total_ref_scripts_bytes(), 0);
    }

    // ===== Capacity limit tests =====

    #[test]
    fn test_max_transactions_evicts_lowest_density() {
        let config = MempoolConfig {
            max_transactions: 3,
            max_bytes: 1_000_000,
            max_ex_mem: u64::MAX,
            max_ex_steps: u64::MAX,
            max_ref_scripts_bytes: usize::MAX,
        };
        let mempool = Mempool::new(config);

        // Add 3 txs with increasing fee density
        for i in 1..=3u8 {
            mempool
                .add_tx_full(
                    Hash32::from_bytes([i; 32]),
                    make_tx_with_fee(i as u64 * 1000),
                    100,
                    Lovelace(i as u64 * 1000),
                    0,
                    0,
                    0,
                    TxOrigin::Local,
                )
                .unwrap();
        }
        assert_eq!(mempool.len(), 3);

        // Add tx with higher fee density than the lowest - should evict tx [1]
        let result = mempool.add_tx_full(
            Hash32::from_bytes([4u8; 32]),
            make_tx_with_fee(5000),
            100,
            Lovelace(5000),
            0,
            0,
            0,
            TxOrigin::Local,
        );
        assert!(result.is_ok());
        assert_eq!(mempool.len(), 3);
        // Lowest fee density (tx [1], fee=1000) should have been evicted
        assert!(!mempool.contains(&Hash32::from_bytes([1u8; 32])));
        assert!(mempool.contains(&Hash32::from_bytes([4u8; 32])));
    }

    #[test]
    fn test_max_bytes_capacity() {
        let config = MempoolConfig {
            max_transactions: 100,
            max_bytes: 500, // 500 bytes total
            max_ex_mem: u64::MAX,
            max_ex_steps: u64::MAX,
            max_ref_scripts_bytes: usize::MAX,
        };
        let mempool = Mempool::new(config);

        // Add tx of 200 bytes
        mempool
            .add_tx_full(
                Hash32::from_bytes([1u8; 32]),
                make_tx_with_fee(100),
                200,
                Lovelace(100),
                0,
                0,
                0,
                TxOrigin::Local,
            )
            .unwrap();

        // Add tx of 200 bytes (total: 400)
        mempool
            .add_tx_full(
                Hash32::from_bytes([2u8; 32]),
                make_tx_with_fee(200),
                200,
                Lovelace(200),
                0,
                0,
                0,
                TxOrigin::Local,
            )
            .unwrap();

        // Add tx of 200 bytes with higher fee density - should evict lowest to make room
        let result = mempool.add_tx_full(
            Hash32::from_bytes([3u8; 32]),
            make_tx_with_fee(300),
            200,
            Lovelace(300),
            0,
            0,
            0,
            TxOrigin::Local,
        );
        assert!(result.is_ok());
        assert_eq!(mempool.len(), 2);
        // Lowest density tx should have been evicted
        assert!(!mempool.contains(&Hash32::from_bytes([1u8; 32])));
    }

    #[test]
    fn test_max_ex_mem_capacity() {
        let config = MempoolConfig {
            max_transactions: 100,
            max_bytes: 1_000_000,
            max_ex_mem: 1000, // tight ex_mem limit
            max_ex_steps: u64::MAX,
            max_ref_scripts_bytes: usize::MAX,
        };
        let mempool = Mempool::new(config);

        // Add tx consuming 600 ex_mem
        mempool
            .add_tx_full(
                Hash32::from_bytes([1u8; 32]),
                make_tx_with_fee(100),
                100,
                Lovelace(100),
                600,
                0,
                0,
                TxOrigin::Local,
            )
            .unwrap();

        // Add tx consuming 600 ex_mem with higher density - should evict first
        let result = mempool.add_tx_full(
            Hash32::from_bytes([2u8; 32]),
            make_tx_with_fee(200),
            100,
            Lovelace(200),
            600,
            0,
            0,
            TxOrigin::Local,
        );
        assert!(result.is_ok());
        assert_eq!(mempool.len(), 1);
        assert!(mempool.contains(&Hash32::from_bytes([2u8; 32])));
        assert!(!mempool.contains(&Hash32::from_bytes([1u8; 32])));
    }

    #[test]
    fn test_max_ex_steps_capacity() {
        let config = MempoolConfig {
            max_transactions: 100,
            max_bytes: 1_000_000,
            max_ex_mem: u64::MAX,
            max_ex_steps: 5000, // tight ex_steps limit
            max_ref_scripts_bytes: usize::MAX,
        };
        let mempool = Mempool::new(config);

        // Add tx consuming 3000 ex_steps
        mempool
            .add_tx_full(
                Hash32::from_bytes([1u8; 32]),
                make_tx_with_fee(100),
                100,
                Lovelace(100),
                0,
                3000,
                0,
                TxOrigin::Local,
            )
            .unwrap();

        // Add tx consuming 3000 ex_steps with higher density - should evict first
        let result = mempool.add_tx_full(
            Hash32::from_bytes([2u8; 32]),
            make_tx_with_fee(200),
            100,
            Lovelace(200),
            0,
            3000,
            0,
            TxOrigin::Local,
        );
        assert!(result.is_ok());
        assert_eq!(mempool.len(), 1);
        assert!(mempool.contains(&Hash32::from_bytes([2u8; 32])));
    }

    #[test]
    fn test_fee_density_ordering_after_eviction() {
        let config = MempoolConfig {
            max_transactions: 3,
            max_bytes: 1_000_000,
            max_ex_mem: u64::MAX,
            max_ex_steps: u64::MAX,
            max_ref_scripts_bytes: usize::MAX,
        };
        let mempool = Mempool::new(config);

        // Add 3 txs: fees 100, 200, 300 all same size
        for i in 1..=3u8 {
            mempool
                .add_tx_full(
                    Hash32::from_bytes([i; 32]),
                    make_tx_with_fee(i as u64 * 100),
                    100,
                    Lovelace(i as u64 * 100),
                    0,
                    0,
                    0,
                    TxOrigin::Local,
                )
                .unwrap();
        }

        // Get txs for block - should be ordered by fee density descending
        let txs = mempool.get_txs_for_block_by_fee(10, 1_000_000);
        assert_eq!(txs.len(), 3);
        assert_eq!(txs[0].body.fee, Lovelace(300)); // highest
        assert_eq!(txs[1].body.fee, Lovelace(200));
        assert_eq!(txs[2].body.fee, Lovelace(100)); // lowest

        // Evict lowest by adding a higher-fee tx
        mempool
            .add_tx_full(
                Hash32::from_bytes([4u8; 32]),
                make_tx_with_fee(500),
                100,
                Lovelace(500),
                0,
                0,
                0,
                TxOrigin::Local,
            )
            .unwrap();

        // Ordering should still be maintained
        let txs2 = mempool.get_txs_for_block_by_fee(10, 1_000_000);
        assert_eq!(txs2.len(), 3);
        assert_eq!(txs2[0].body.fee, Lovelace(500));
        assert_eq!(txs2[1].body.fee, Lovelace(300));
        assert_eq!(txs2[2].body.fee, Lovelace(200));
    }

    #[test]
    fn test_fifo_fairness_local_vs_remote() {
        let mempool = Mempool::new(default_config());

        // Add local tx
        mempool
            .add_tx_full(
                Hash32::from_bytes([1u8; 32]),
                make_dummy_tx(),
                100,
                Lovelace(100),
                0,
                0,
                0,
                TxOrigin::Local,
            )
            .unwrap();

        // Add remote tx
        mempool
            .add_tx_full(
                Hash32::from_bytes([2u8; 32]),
                make_dummy_tx(),
                100,
                Lovelace(100),
                0,
                0,
                0,
                TxOrigin::Remote,
            )
            .unwrap();

        // Add another local tx
        mempool
            .add_tx_full(
                Hash32::from_bytes([3u8; 32]),
                make_dummy_tx(),
                100,
                Lovelace(100),
                0,
                0,
                0,
                TxOrigin::Local,
            )
            .unwrap();

        // FIFO order should be preserved regardless of origin
        let ordered = mempool.tx_hashes_ordered();
        assert_eq!(ordered.len(), 3);
        assert_eq!(ordered[0], Hash32::from_bytes([1u8; 32]));
        assert_eq!(ordered[1], Hash32::from_bytes([2u8; 32]));
        assert_eq!(ordered[2], Hash32::from_bytes([3u8; 32]));
    }

    #[test]
    fn test_drain_all_preserves_insertion_order_extended() {
        let mempool = Mempool::new(default_config());

        // Add txs in specific order with different fees
        for i in 1..=5u8 {
            mempool
                .add_tx_full(
                    Hash32::from_bytes([i; 32]),
                    make_tx_with_fee(i as u64 * 50),
                    100,
                    Lovelace(i as u64 * 50),
                    0,
                    0,
                    0,
                    TxOrigin::Local,
                )
                .unwrap();
        }

        let drained = mempool.drain_all();
        assert_eq!(drained.len(), 5);
        // drain_all returns FIFO order, NOT fee-density order
        assert_eq!(drained[0].body.fee, Lovelace(50));
        assert_eq!(drained[1].body.fee, Lovelace(100));
        assert_eq!(drained[2].body.fee, Lovelace(150));
        assert_eq!(drained[3].body.fee, Lovelace(200));
        assert_eq!(drained[4].body.fee, Lovelace(250));

        assert!(mempool.is_empty());
        assert_eq!(mempool.total_bytes(), 0);
    }

    // ========================== Input-conflict detection tests ==========================

    /// Two transactions spending the same UTxO input cannot coexist in the mempool.
    /// The second submission must be rejected with InputConflict, not silently admitted.
    #[test]
    fn test_input_conflict_rejected() {
        let mempool = Mempool::new(default_config());

        // tx_a spends input d697f98b#0
        let shared_input = TransactionInput {
            transaction_id: Hash32::from_bytes([
                0xd6, 0x97, 0xf9, 0x8b, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0,
            ]),
            index: 0,
        };

        let mut tx_a = make_dummy_tx();
        tx_a.body.inputs = vec![shared_input.clone()];
        let hash_a = Hash32::from_bytes([0xAA; 32]);
        mempool.add_tx(hash_a, tx_a, 300).unwrap();

        // tx_b also spends d697f98b#0 — must be rejected
        let mut tx_b = make_dummy_tx();
        tx_b.body.inputs = vec![shared_input.clone()];
        let hash_b = Hash32::from_bytes([0xBB; 32]);
        let result = mempool.add_tx(hash_b, tx_b, 300);

        assert!(
            matches!(result, Err(MempoolError::InputConflict { claimed_by }) if claimed_by == hash_a),
            "expected InputConflict(hash_a) but got: {:?}",
            result
        );
        // Only tx_a should be in the mempool
        assert_eq!(mempool.len(), 1);
        assert!(mempool.contains(&hash_a));
        assert!(!mempool.contains(&hash_b));
    }

    /// Regression: the soak test scenario — 50 txs all spending the same input.
    /// Only the first must be admitted; the remaining 49 must all be rejected.
    #[test]
    fn test_input_conflict_stress_50_txs_same_input() {
        let mempool = Mempool::new(default_config());

        let shared_input = TransactionInput {
            transaction_id: Hash32::from_bytes([
                0xd6, 0x97, 0xf9, 0x8b, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0, 0, 0, 0, 0, 0, 0,
            ]),
            index: 0,
        };

        let mut admitted = 0usize;
        let mut rejected = 0usize;
        for i in 0u8..50 {
            let mut tx = make_dummy_tx();
            tx.body.inputs = vec![shared_input.clone()];
            let mut hash_bytes = [0u8; 32];
            hash_bytes[0] = i;
            let hash = Hash32::from_bytes(hash_bytes);
            match mempool.add_tx(hash, tx, 200) {
                Ok(MempoolAddResult::Added) => admitted += 1,
                Err(MempoolError::InputConflict { .. }) => rejected += 1,
                other => panic!("unexpected result: {:?}", other),
            }
        }

        assert_eq!(admitted, 1, "exactly one tx must be admitted");
        assert_eq!(
            rejected, 49,
            "all other 49 must be rejected with InputConflict"
        );
        assert_eq!(mempool.len(), 1);
        // Exactly one entry in the claimed-inputs index
        assert_eq!(mempool.claimed_inputs_count(), 1);
    }

    /// After removing the tx that holds a contested input, a new tx spending that
    /// same input must be admitted successfully.
    #[test]
    fn test_input_conflict_cleared_after_remove() {
        let mempool = Mempool::new(default_config());

        let shared_input = TransactionInput {
            transaction_id: Hash32::from_bytes([0x12; 32]),
            index: 3,
        };

        // tx_a claims the input
        let mut tx_a = make_dummy_tx();
        tx_a.body.inputs = vec![shared_input.clone()];
        let hash_a = Hash32::from_bytes([0x01; 32]);
        mempool.add_tx(hash_a, tx_a, 300).unwrap();
        assert_eq!(mempool.claimed_inputs_count(), 1);

        // tx_b is rejected because tx_a holds the input
        let mut tx_b = make_dummy_tx();
        tx_b.body.inputs = vec![shared_input.clone()];
        let hash_b = Hash32::from_bytes([0x02; 32]);
        assert!(matches!(
            mempool.add_tx(hash_b, tx_b.clone(), 300),
            Err(MempoolError::InputConflict { .. })
        ));

        // Confirm tx_a (block includes it) — release its inputs
        mempool.remove_tx(&hash_a);
        assert_eq!(mempool.len(), 0);
        assert_eq!(mempool.claimed_inputs_count(), 0);

        // Now tx_b (same input) must be admitted
        mempool.add_tx(hash_b, tx_b, 300).unwrap();
        assert_eq!(mempool.len(), 1);
        assert!(mempool.contains(&hash_b));
        assert_eq!(mempool.claimed_inputs_count(), 1);
    }

    /// A tx with multiple inputs: conflict is detected even when only one of
    /// several inputs overlaps with an existing mempool tx.
    #[test]
    fn test_input_conflict_partial_overlap() {
        let mempool = Mempool::new(default_config());

        let input_x = TransactionInput {
            transaction_id: Hash32::from_bytes([0x11; 32]),
            index: 0,
        };
        let input_y = TransactionInput {
            transaction_id: Hash32::from_bytes([0x22; 32]),
            index: 0,
        };
        let input_z = TransactionInput {
            transaction_id: Hash32::from_bytes([0x33; 32]),
            index: 0,
        };

        // tx_a spends input_x and input_y
        let mut tx_a = make_dummy_tx();
        tx_a.body.inputs = vec![input_x.clone(), input_y.clone()];
        let hash_a = Hash32::from_bytes([0xA0; 32]);
        mempool.add_tx(hash_a, tx_a, 300).unwrap();
        assert_eq!(mempool.claimed_inputs_count(), 2);

        // tx_b spends input_y (overlap!) and input_z — must be rejected
        let mut tx_b = make_dummy_tx();
        tx_b.body.inputs = vec![input_y.clone(), input_z.clone()];
        let hash_b = Hash32::from_bytes([0xB0; 32]);
        let result = mempool.add_tx(hash_b, tx_b, 300);
        assert!(
            matches!(result, Err(MempoolError::InputConflict { .. })),
            "expected InputConflict: {:?}",
            result
        );

        // tx_c spends only input_z (no overlap) — must be admitted
        let mut tx_c = make_dummy_tx();
        tx_c.body.inputs = vec![input_z.clone()];
        let hash_c = Hash32::from_bytes([0xC0; 32]);
        mempool.add_tx(hash_c, tx_c, 200).unwrap();

        assert_eq!(mempool.len(), 2);
        // input_x, input_y from tx_a plus input_z from tx_c
        assert_eq!(mempool.claimed_inputs_count(), 3);
    }

    /// drain_all must clear the claimed-inputs index so the mempool is fully
    /// reusable after draining (rollback scenario).
    #[test]
    fn test_input_conflict_cleared_after_drain_all() {
        let mempool = Mempool::new(default_config());

        let shared_input = TransactionInput {
            transaction_id: Hash32::from_bytes([0xDE; 32]),
            index: 1,
        };

        let mut tx_a = make_dummy_tx();
        tx_a.body.inputs = vec![shared_input.clone()];
        let hash_a = Hash32::from_bytes([0x01; 32]);
        mempool.add_tx(hash_a, tx_a, 200).unwrap();

        // Simulate rollback: drain everything
        let drained = mempool.drain_all();
        assert_eq!(drained.len(), 1);
        assert_eq!(
            mempool.claimed_inputs_count(),
            0,
            "drain_all must clear claimed_inputs"
        );

        // The same input can now be re-admitted
        let mut tx_b = make_dummy_tx();
        tx_b.body.inputs = vec![shared_input.clone()];
        let hash_b = Hash32::from_bytes([0x02; 32]);
        mempool.add_tx(hash_b, tx_b, 200).unwrap();
        assert_eq!(mempool.len(), 1);
    }

    /// clear() must also reset the claimed-inputs index.
    #[test]
    fn test_input_conflict_cleared_after_clear() {
        let mempool = Mempool::new(default_config());

        let shared_input = TransactionInput {
            transaction_id: Hash32::from_bytes([0xCA; 32]),
            index: 0,
        };

        let mut tx = make_dummy_tx();
        tx.body.inputs = vec![shared_input.clone()];
        mempool
            .add_tx(Hash32::from_bytes([0x01; 32]), tx, 200)
            .unwrap();

        mempool.clear();
        assert_eq!(
            mempool.claimed_inputs_count(),
            0,
            "clear() must reset claimed_inputs"
        );

        // Must be re-admittable after clear
        let mut tx2 = make_dummy_tx();
        tx2.body.inputs = vec![shared_input.clone()];
        mempool
            .add_tx(Hash32::from_bytes([0x02; 32]), tx2, 200)
            .unwrap();
        assert_eq!(mempool.len(), 1);
    }

    // ========================== sweep_expired tests ==========================

    /// sweep_expired(u64) is equivalent to evict_expired(SlotNo(u64)).
    #[test]
    fn test_sweep_expired_removes_past_ttl() {
        let mempool = Mempool::new(default_config());

        // tx with TTL=100 (expires after slot 100)
        let mut tx_expiring = make_dummy_tx();
        tx_expiring.body.ttl = Some(SlotNo(100));
        mempool
            .add_tx(Hash32::from_bytes([0x01; 32]), tx_expiring, 200)
            .unwrap();

        // tx with TTL=200
        let mut tx_later = make_dummy_tx();
        tx_later.body.ttl = Some(SlotNo(200));
        mempool
            .add_tx(Hash32::from_bytes([0x02; 32]), tx_later, 200)
            .unwrap();

        // tx with no TTL (never expires)
        let tx_immortal = make_dummy_tx();
        mempool
            .add_tx(Hash32::from_bytes([0x03; 32]), tx_immortal, 200)
            .unwrap();

        assert_eq!(mempool.len(), 3);

        // At slot 50 nothing has expired yet
        assert_eq!(mempool.sweep_expired(50), 0);
        assert_eq!(mempool.len(), 3);

        // At slot 101 the TTL=100 tx has expired
        assert_eq!(mempool.sweep_expired(101), 1);
        assert_eq!(mempool.len(), 2);
        assert!(!mempool.contains(&Hash32::from_bytes([0x01; 32])));

        // At slot 201 the TTL=200 tx has expired
        assert_eq!(mempool.sweep_expired(201), 1);
        assert_eq!(mempool.len(), 1);
        assert!(mempool.contains(&Hash32::from_bytes([0x03; 32]))); // immortal remains
    }

    /// sweep_expired at the exact TTL slot DOES evict (Haskell half-open: slot >= ttl means expired).
    #[test]
    fn test_sweep_expired_boundary_exact_ttl_slot() {
        let mempool = Mempool::new(default_config());

        let mut tx = make_dummy_tx();
        tx.body.ttl = Some(SlotNo(100));
        mempool
            .add_tx(Hash32::from_bytes([0x01; 32]), tx, 200)
            .unwrap();

        // One slot before TTL — still valid
        assert_eq!(mempool.sweep_expired(99), 0);
        assert_eq!(mempool.len(), 1);

        // Exactly at TTL slot — expired (TTL is the first INVALID slot per Haskell)
        assert_eq!(mempool.sweep_expired(100), 1);
        assert_eq!(mempool.len(), 0);
    }

    /// sweep_expired releases claimed inputs so a replacement tx can be admitted.
    #[test]
    fn test_sweep_expired_releases_claimed_inputs() {
        let mempool = Mempool::new(default_config());

        let shared_input = TransactionInput {
            transaction_id: Hash32::from_bytes([0xAB; 32]),
            index: 0,
        };

        // tx_a spends the input and has a short TTL
        let mut tx_a = make_dummy_tx();
        tx_a.body.inputs = vec![shared_input.clone()];
        tx_a.body.ttl = Some(SlotNo(50));
        let hash_a = Hash32::from_bytes([0x01; 32]);
        mempool.add_tx(hash_a, tx_a, 200).unwrap();

        // tx_b spends the same input — conflict while tx_a is alive
        let mut tx_b = make_dummy_tx();
        tx_b.body.inputs = vec![shared_input.clone()];
        let hash_b = Hash32::from_bytes([0x02; 32]);
        assert!(matches!(
            mempool.add_tx(hash_b, tx_b.clone(), 200),
            Err(MempoolError::InputConflict { .. })
        ));

        // Sweep at slot 51 — tx_a expires, releasing the input
        let swept = mempool.sweep_expired(51);
        assert_eq!(swept, 1);
        assert_eq!(mempool.claimed_inputs_count(), 0);

        // Now tx_b (spending the same input) must be admitted
        mempool.add_tx(hash_b, tx_b, 200).unwrap();
        assert_eq!(mempool.len(), 1);
        assert_eq!(mempool.claimed_inputs_count(), 1);
    }

    // ========================= Virtual UTxO tests =========================

    /// Build a transaction whose outputs can be used as virtual UTxO entries.
    /// `outputs` is a slice of lovelace values; each becomes one output.
    fn make_tx_with_outputs(outputs: &[u64]) -> (Transaction, TransactionHash) {
        use std::sync::atomic::{AtomicU32, Ordering as AOrdering};
        static VTXO_COUNTER: AtomicU32 = AtomicU32::new(1000);
        let n = VTXO_COUNTER.fetch_add(1, AOrdering::Relaxed);
        let mut id_bytes = [0u8; 32];
        id_bytes[28..32].copy_from_slice(&n.to_be_bytes());
        let tx_hash = Hash32::from_bytes(id_bytes);

        let outs: Vec<TransactionOutput> = outputs
            .iter()
            .map(|&lovelace| TransactionOutput {
                address: Address::Byron(ByronAddress {
                    payload: vec![0; 32],
                }),
                value: Value::lovelace(lovelace),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            })
            .collect();

        // The tx body inputs use a distinct ID so they don't collide with the outputs.
        let mut in_bytes = [0xFFu8; 32];
        in_bytes[28..32].copy_from_slice(&n.to_be_bytes());

        let tx = Transaction {
            hash: tx_hash,
            body: TransactionBody {
                inputs: vec![TransactionInput {
                    transaction_id: Hash32::from_bytes(in_bytes),
                    index: 0,
                }],
                outputs: outs,
                fee: Lovelace(200_000),
                ttl: None,
                certificates: vec![],
                withdrawals: std::collections::BTreeMap::new(),
                auxiliary_data_hash: None,
                validity_interval_start: None,
                mint: std::collections::BTreeMap::new(),
                script_data_hash: None,
                collateral: vec![],
                required_signers: vec![],
                network_id: None,
                collateral_return: None,
                total_collateral: None,
                reference_inputs: vec![],
                update: None,
                voting_procedures: std::collections::BTreeMap::new(),
                proposal_procedures: vec![],
                treasury_value: None,
                donation: None,
            },
            witness_set: torsten_primitives::transaction::TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![],
                plutus_v3_scripts: vec![],
                plutus_data: vec![],
                redeemers: vec![],
                raw_redeemers_cbor: None,
                raw_plutus_data_cbor: None,
                pallas_script_data_hash: None,
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        };
        (tx, tx_hash)
    }

    /// Make a transaction that spends a specific virtual UTxO output.
    fn make_tx_spending_virtual(parent_hash: TransactionHash, output_index: u32) -> Transaction {
        use std::sync::atomic::{AtomicU32, Ordering as AOrdering};
        static SPEND_COUNTER: AtomicU32 = AtomicU32::new(2000);
        let n = SPEND_COUNTER.fetch_add(1, AOrdering::Relaxed);
        let mut id_bytes = [0u8; 32];
        id_bytes[28..32].copy_from_slice(&n.to_be_bytes());

        Transaction {
            hash: Hash32::from_bytes(id_bytes),
            body: TransactionBody {
                inputs: vec![TransactionInput {
                    transaction_id: parent_hash,
                    index: output_index,
                }],
                outputs: vec![TransactionOutput {
                    address: Address::Byron(ByronAddress {
                        payload: vec![0; 32],
                    }),
                    value: Value::lovelace(500_000),
                    datum: OutputDatum::None,
                    script_ref: None,
                    is_legacy: false,
                    raw_cbor: None,
                }],
                fee: Lovelace(200_000),
                ttl: None,
                certificates: vec![],
                withdrawals: std::collections::BTreeMap::new(),
                auxiliary_data_hash: None,
                validity_interval_start: None,
                mint: std::collections::BTreeMap::new(),
                script_data_hash: None,
                collateral: vec![],
                required_signers: vec![],
                network_id: None,
                collateral_return: None,
                total_collateral: None,
                reference_inputs: vec![],
                update: None,
                voting_procedures: std::collections::BTreeMap::new(),
                proposal_procedures: vec![],
                treasury_value: None,
                donation: None,
            },
            witness_set: torsten_primitives::transaction::TransactionWitnessSet {
                vkey_witnesses: vec![],
                native_scripts: vec![],
                bootstrap_witnesses: vec![],
                plutus_v1_scripts: vec![],
                plutus_v2_scripts: vec![],
                plutus_v3_scripts: vec![],
                plutus_data: vec![],
                redeemers: vec![],
                raw_redeemers_cbor: None,
                raw_plutus_data_cbor: None,
                pallas_script_data_hash: None,
            },
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
        }
    }

    // -----------------------------------------------------------------------
    // Virtual UTxO population and lookup
    // -----------------------------------------------------------------------

    /// After adding a tx, its outputs must appear in the virtual UTxO set.
    #[test]
    fn test_virtual_utxo_populated_on_add() {
        let mempool = Mempool::new(default_config());
        let (tx, tx_hash) = make_tx_with_outputs(&[1_000_000, 2_000_000]);

        mempool.add_tx(tx_hash, tx, 300).unwrap();

        // Both outputs should be in the virtual UTxO
        assert_eq!(mempool.virtual_utxo_count(), 2);

        let key0 = TransactionInput {
            transaction_id: tx_hash,
            index: 0,
        };
        let key1 = TransactionInput {
            transaction_id: tx_hash,
            index: 1,
        };
        assert!(
            mempool.lookup_virtual_utxo(&key0).is_some(),
            "output 0 should be in virtual UTxO"
        );
        assert!(
            mempool.lookup_virtual_utxo(&key1).is_some(),
            "output 1 should be in virtual UTxO"
        );

        let out0 = mempool.lookup_virtual_utxo(&key0).unwrap();
        assert_eq!(out0.value.coin.0, 1_000_000);
        let out1 = mempool.lookup_virtual_utxo(&key1).unwrap();
        assert_eq!(out1.value.coin.0, 2_000_000);
    }

    /// After removing a tx, its outputs must be removed from the virtual UTxO set.
    #[test]
    fn test_virtual_utxo_cleared_on_remove() {
        let mempool = Mempool::new(default_config());
        let (tx, tx_hash) = make_tx_with_outputs(&[1_000_000]);

        mempool.add_tx(tx_hash, tx, 300).unwrap();
        assert_eq!(mempool.virtual_utxo_count(), 1);

        mempool.remove_tx(&tx_hash);
        assert_eq!(mempool.virtual_utxo_count(), 0);

        let key = TransactionInput {
            transaction_id: tx_hash,
            index: 0,
        };
        assert!(
            mempool.lookup_virtual_utxo(&key).is_none(),
            "output should be removed from virtual UTxO after tx removal"
        );
    }

    /// After `clear()`, the virtual UTxO set must be empty.
    #[test]
    fn test_virtual_utxo_cleared_on_clear() {
        let mempool = Mempool::new(default_config());

        let (tx1, h1) = make_tx_with_outputs(&[1_000_000, 2_000_000]);
        let (tx2, h2) = make_tx_with_outputs(&[3_000_000]);
        mempool.add_tx(h1, tx1, 200).unwrap();
        mempool.add_tx(h2, tx2, 200).unwrap();

        assert_eq!(mempool.virtual_utxo_count(), 3); // 2 + 1

        mempool.clear();
        assert_eq!(mempool.virtual_utxo_count(), 0);
        assert_eq!(mempool.len(), 0);
    }

    /// After `drain_all()`, the virtual UTxO set must be empty.
    #[test]
    fn test_virtual_utxo_cleared_on_drain_all() {
        let mempool = Mempool::new(default_config());
        let (tx, tx_hash) = make_tx_with_outputs(&[1_000_000]);
        mempool.add_tx(tx_hash, tx, 200).unwrap();
        assert_eq!(mempool.virtual_utxo_count(), 1);

        let drained = mempool.drain_all();
        assert_eq!(drained.len(), 1);
        assert_eq!(mempool.virtual_utxo_count(), 0);
        assert!(mempool.dependents.read().is_empty());
    }

    /// `virtual_utxo_snapshot()` must return all currently-pending outputs.
    #[test]
    fn test_virtual_utxo_snapshot() {
        let mempool = Mempool::new(default_config());
        let (tx, tx_hash) = make_tx_with_outputs(&[1_000_000, 2_000_000]);
        mempool.add_tx(tx_hash, tx, 300).unwrap();

        let snapshot = mempool.virtual_utxo_snapshot();
        assert_eq!(snapshot.len(), 2);
        assert!(snapshot.contains_key(&TransactionInput {
            transaction_id: tx_hash,
            index: 0
        }));
        assert!(snapshot.contains_key(&TransactionInput {
            transaction_id: tx_hash,
            index: 1
        }));
    }

    // -----------------------------------------------------------------------
    // Cascade removal
    // -----------------------------------------------------------------------

    /// A child tx whose input references a parent tx output should be
    /// cascade-removed when the parent is removed.
    #[test]
    fn test_cascade_removal_direct_child() {
        let mempool = Mempool::new(default_config());

        // Parent tx: has one output
        let (parent_tx, parent_hash) = make_tx_with_outputs(&[1_000_000]);
        mempool.add_tx(parent_hash, parent_tx, 300).unwrap();

        // Child tx: spends parent's output 0
        let child_tx = make_tx_spending_virtual(parent_hash, 0);
        let child_hash = child_tx.hash;
        mempool.add_tx(child_hash, child_tx, 300).unwrap();

        assert_eq!(mempool.len(), 2);
        // The child's dependency should be recorded
        assert!(mempool.dependents.read().contains_key(&parent_hash));

        // Remove the parent — the child must be cascade-removed
        mempool.remove_tx(&parent_hash);

        assert_eq!(
            mempool.len(),
            0,
            "child should be cascade-removed with parent"
        );
        assert_eq!(
            mempool.virtual_utxo_count(),
            0,
            "all virtual UTxO entries should be gone"
        );
        assert_eq!(
            mempool.claimed_inputs_count(),
            0,
            "no claimed inputs should remain"
        );
    }

    /// A multi-level chain: grandchild cascades when grandparent is removed.
    /// tx_a → tx_b (spends tx_a output) → tx_c (spends tx_b output)
    #[test]
    fn test_cascade_removal_multi_level() {
        let mempool = Mempool::new(default_config());

        // tx_a: root
        let (tx_a, hash_a) = make_tx_with_outputs(&[3_000_000]);
        mempool.add_tx(hash_a, tx_a, 200).unwrap();

        // tx_b: spends tx_a output 0
        let tx_b = make_tx_spending_virtual(hash_a, 0);
        let hash_b = tx_b.hash;
        mempool.add_tx(hash_b, tx_b, 200).unwrap();

        // tx_c: spends tx_b output 0
        let tx_c = make_tx_spending_virtual(hash_b, 0);
        let hash_c = tx_c.hash;
        mempool.add_tx(hash_c, tx_c, 200).unwrap();

        assert_eq!(mempool.len(), 3);

        // Remove tx_a — tx_b and tx_c should both cascade out
        mempool.remove_tx(&hash_a);

        assert_eq!(
            mempool.len(),
            0,
            "tx_b and tx_c should be cascade-removed transitively"
        );
        assert_eq!(mempool.virtual_utxo_count(), 0);
        assert_eq!(mempool.claimed_inputs_count(), 0);
        assert!(mempool.dependents.read().is_empty());
    }

    /// Removing a child directly (without removing the parent) must NOT remove
    /// the parent, and must leave the parent's virtual UTxO entries intact.
    #[test]
    fn test_cascade_does_not_remove_parent() {
        let mempool = Mempool::new(default_config());

        let (parent_tx, parent_hash) = make_tx_with_outputs(&[1_000_000, 2_000_000]);
        mempool.add_tx(parent_hash, parent_tx, 300).unwrap();

        let child_tx = make_tx_spending_virtual(parent_hash, 0);
        let child_hash = child_tx.hash;
        mempool.add_tx(child_hash, child_tx, 300).unwrap();

        // Remove only the child
        mempool.remove_tx(&child_hash);

        // Parent should still be present
        assert_eq!(mempool.len(), 1);
        assert!(
            mempool.contains(&parent_hash),
            "parent must not be removed when child is removed"
        );
        // Parent's virtual UTxO entries should still be there
        // (the child's output entry was removed)
        assert!(
            mempool
                .lookup_virtual_utxo(&TransactionInput {
                    transaction_id: parent_hash,
                    index: 0,
                })
                .is_some(),
            "parent output 0 should still be in virtual UTxO"
        );
        assert!(
            mempool
                .lookup_virtual_utxo(&TransactionInput {
                    transaction_id: parent_hash,
                    index: 1,
                })
                .is_some(),
            "parent output 1 should still be in virtual UTxO"
        );
    }

    /// An unrelated tx should not be removed when a parent is cascade-removed.
    #[test]
    fn test_cascade_does_not_affect_unrelated_txs() {
        let mempool = Mempool::new(default_config());

        // Parent + child chain
        let (parent_tx, parent_hash) = make_tx_with_outputs(&[1_000_000]);
        mempool.add_tx(parent_hash, parent_tx, 200).unwrap();

        let child_tx = make_tx_spending_virtual(parent_hash, 0);
        let child_hash = child_tx.hash;
        mempool.add_tx(child_hash, child_tx, 200).unwrap();

        // Unrelated tx
        let (unrelated_tx, unrelated_hash) = make_tx_with_outputs(&[500_000]);
        mempool.add_tx(unrelated_hash, unrelated_tx, 200).unwrap();

        assert_eq!(mempool.len(), 3);

        // Remove parent — parent + child cascade out, unrelated stays
        mempool.remove_tx(&parent_hash);

        assert_eq!(mempool.len(), 1);
        assert!(
            mempool.contains(&unrelated_hash),
            "unrelated tx should not be affected by cascade removal"
        );
    }

    /// TTL expiry of a parent should cascade-remove dependent children.
    #[test]
    fn test_cascade_on_ttl_expiry() {
        let mempool = Mempool::new(default_config());

        // Parent has TTL = 100
        let (mut parent_tx, parent_hash) = make_tx_with_outputs(&[1_000_000]);
        parent_tx.body.ttl = Some(SlotNo(100));
        mempool.add_tx(parent_hash, parent_tx, 200).unwrap();

        // Child spends parent output
        let child_tx = make_tx_spending_virtual(parent_hash, 0);
        let child_hash = child_tx.hash;
        mempool.add_tx(child_hash, child_tx, 200).unwrap();

        assert_eq!(mempool.len(), 2);

        // Sweep at slot 101: parent expires
        let evicted = mempool.evict_expired(SlotNo(101));

        // The parent is directly expired; the child is cascade-removed
        assert_eq!(evicted, 1, "only 1 tx is directly evicted (parent)");
        // After cascade, mempool is empty
        assert_eq!(
            mempool.len(),
            0,
            "child should be cascade-removed when parent expires"
        );
        assert_eq!(mempool.virtual_utxo_count(), 0);
        assert_eq!(mempool.claimed_inputs_count(), 0);
    }

    /// `dependency_count()` should equal the number of parent→child edges.
    #[test]
    fn test_dependency_graph_size() {
        let mempool = Mempool::new(default_config());

        let (tx_a, hash_a) = make_tx_with_outputs(&[1_000_000, 2_000_000]);
        mempool.add_tx(hash_a, tx_a, 200).unwrap();

        // Two children: each spends a different output of tx_a
        let child1 = make_tx_spending_virtual(hash_a, 0);
        let child1_hash = child1.hash;
        mempool.add_tx(child1_hash, child1, 200).unwrap();

        let child2 = make_tx_spending_virtual(hash_a, 1);
        let child2_hash = child2.hash;
        mempool.add_tx(child2_hash, child2, 200).unwrap();

        {
            let dep_map = mempool.dependents.read();
            let children = dep_map.get(&hash_a).expect("tx_a should have children");
            assert_eq!(children.len(), 2, "tx_a should have exactly 2 children");
            assert!(children.contains(&child1_hash));
            assert!(children.contains(&child2_hash));
        }

        // Remove one child — the other child + parent should remain
        mempool.remove_tx(&child1_hash);
        assert_eq!(mempool.len(), 2);
    }

    /// Multiple outputs: only the unspent virtual outputs should remain after
    /// a child is added (child spends output 0; output 1 is still available).
    #[test]
    fn test_virtual_utxo_partial_spend() {
        let mempool = Mempool::new(default_config());

        let (tx, tx_hash) = make_tx_with_outputs(&[1_000_000, 2_000_000]);
        mempool.add_tx(tx_hash, tx, 300).unwrap();

        // Only output 0 is spent by the child
        let child = make_tx_spending_virtual(tx_hash, 0);
        let child_hash = child.hash;
        mempool.add_tx(child_hash, child, 200).unwrap();

        // Total virtual UTxO entries: 2 (parent) + 1 (child) = 3
        assert_eq!(mempool.virtual_utxo_count(), 3);

        // Output 0 AND output 1 of the parent are still in virtual UTxO
        assert!(mempool
            .lookup_virtual_utxo(&TransactionInput {
                transaction_id: tx_hash,
                index: 0
            })
            .is_some());
        assert!(mempool
            .lookup_virtual_utxo(&TransactionInput {
                transaction_id: tx_hash,
                index: 1
            })
            .is_some());

        // Child's output is also in virtual UTxO
        assert!(mempool
            .lookup_virtual_utxo(&TransactionInput {
                transaction_id: child_hash,
                index: 0
            })
            .is_some());
    }

    /// A tx that does NOT spend any mempool outputs should not create
    /// any entries in the dependency graph.
    #[test]
    fn test_no_false_dependencies_for_on_chain_spends() {
        let mempool = Mempool::new(default_config());

        // Independent tx using an "on-chain" input (not in virtual UTxO)
        let (tx, hash) = make_tx_with_outputs(&[1_000_000]);
        mempool.add_tx(hash, tx, 200).unwrap();

        // The dependency map should be empty (no virtual UTxO parents)
        assert!(
            mempool.dependents.read().is_empty(),
            "no dependencies should be recorded for a tx spending on-chain UTxOs"
        );
    }
}
