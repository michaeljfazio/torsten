use dugite_primitives::address::Address;
use dugite_primitives::hash::TransactionHash;
use dugite_primitives::transaction::{TransactionInput, TransactionOutput};
use dugite_primitives::value::Lovelace;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::utxo_store::UtxoStore;

/// Default for `indexing_enabled` during serde deserialization.
/// Returns `true` so that address indexing is active by default after loading.
fn default_indexing_enabled() -> bool {
    true
}

// ---------------------------------------------------------------------------
// UtxoLookup trait — abstraction over UTxO lookup
// ---------------------------------------------------------------------------

/// Read-only UTxO lookup interface.
///
/// This trait allows the transaction validation pipeline to work against both
/// the authoritative on-chain `UtxoSet` and the `CompositeUtxoView` (which
/// overlays unconfirmed mempool outputs on top of the on-chain set).
///
/// All `validate_transaction_*` functions are generic over this trait so that
/// the mempool validator can pass a composite view without modifying the live
/// ledger state.
pub trait UtxoLookup {
    /// Look up a UTxO by spending input reference.
    fn lookup(&self, input: &TransactionInput) -> Option<TransactionOutput>;

    /// Check if a UTxO exists.
    fn contains(&self, input: &TransactionInput) -> bool {
        self.lookup(input).is_some()
    }
}

impl UtxoLookup for UtxoSet {
    fn lookup(&self, input: &TransactionInput) -> Option<TransactionOutput> {
        UtxoSet::lookup(self, input)
    }

    fn contains(&self, input: &TransactionInput) -> bool {
        UtxoSet::contains(self, input)
    }
}

// ---------------------------------------------------------------------------
// CompositeUtxoView — on-chain UTxO set + virtual mempool overlay
// ---------------------------------------------------------------------------

/// A read-only composite UTxO view that consults the on-chain `UtxoSet` first
/// and falls back to a virtual overlay (from the mempool) on a miss.
///
/// This allows chained (dependent) transactions to be validated against pending
/// mempool outputs without modifying the live ledger state, matching the
/// behaviour of Haskell cardano-node's virtual UTxO.
///
/// # Thread safety
/// `CompositeUtxoView` holds a shared reference to the on-chain `UtxoSet` and
/// an owned snapshot of the mempool's virtual UTxO entries.  Lookups are
/// read-only and therefore safe for concurrent use.
pub struct CompositeUtxoView<'a> {
    /// Primary: the authoritative on-chain UTxO set (behind a read-lock borrow).
    on_chain: &'a UtxoSet,
    /// Fallback: a snapshot of virtual UTxO entries from pending mempool txs.
    /// Keyed by `TransactionInput { transaction_id = mempool_tx_hash, index }`.
    virtual_utxo: HashMap<TransactionInput, TransactionOutput>,
}

impl<'a> CompositeUtxoView<'a> {
    /// Create a composite view from an on-chain `UtxoSet` reference and a
    /// virtual UTxO snapshot taken from the mempool.
    pub fn new(
        on_chain: &'a UtxoSet,
        virtual_utxo: HashMap<TransactionInput, TransactionOutput>,
    ) -> Self {
        CompositeUtxoView {
            on_chain,
            virtual_utxo,
        }
    }
}

impl<'a> UtxoLookup for CompositeUtxoView<'a> {
    /// Look up an input: check on-chain first, then the virtual overlay.
    fn lookup(&self, input: &TransactionInput) -> Option<TransactionOutput> {
        self.on_chain
            .lookup(input)
            .or_else(|| self.virtual_utxo.get(input).cloned())
    }

    fn contains(&self, input: &TransactionInput) -> bool {
        self.on_chain.contains(input) || self.virtual_utxo.contains_key(input)
    }
}

/// The UTxO set: maps transaction inputs to their unspent outputs.
///
/// Supports two backends:
/// - **In-memory** (`HashMap`): used for tests and legacy mode
/// - **On-disk** (`UtxoStore` backed by `dugite-lsm`): used in production,
///   dramatically reduces memory usage for large UTxO sets (mainnet ~20M entries)
///
/// When an `UtxoStore` is attached, all operations delegate to it and the
/// in-memory `utxos` HashMap is unused. The store is `#[serde(skip)]` — it is
/// managed separately during snapshot save/load.
#[derive(Debug, Serialize, Deserialize)]
pub struct UtxoSet {
    utxos: HashMap<TransactionInput, TransactionOutput>,
    /// Secondary index: address → set of TransactionInputs at that address.
    /// Skipped during serialization and rebuilt on load via `rebuild_address_index()`.
    #[serde(skip)]
    address_index: HashMap<Address, Vec<TransactionInput>>,
    /// When false, address index operations are skipped (for fast replay).
    /// Call `rebuild_address_index()` after re-enabling.
    #[serde(skip, default = "default_indexing_enabled")]
    indexing_enabled: bool,
    /// Optional LSM-tree backed store. When present, all operations delegate to it.
    /// The in-memory `utxos` HashMap is unused when this is set.
    #[serde(skip)]
    store: Option<UtxoStore>,
}

impl UtxoSet {
    pub fn new() -> Self {
        UtxoSet {
            utxos: HashMap::new(),
            address_index: HashMap::new(),
            indexing_enabled: true,
            store: None,
        }
    }

    /// Attach an LSM-backed UtxoStore. All subsequent operations will delegate to it.
    /// Any existing in-memory UTxOs are NOT migrated — this is intended for use when
    /// the store already contains the full UTxO set (e.g., after loading from snapshot).
    pub fn attach_store(&mut self, store: UtxoStore) {
        self.store = Some(store);
        // Clear in-memory data to free RAM
        self.utxos.clear();
        self.utxos.shrink_to_fit();
    }

    /// Detach and return the LSM-backed store, reverting to in-memory mode.
    pub fn detach_store(&mut self) -> Option<UtxoStore> {
        self.store.take()
    }

    /// Get a reference to the attached UtxoStore (if any).
    pub fn store(&self) -> Option<&UtxoStore> {
        self.store.as_ref()
    }

    /// Get a mutable reference to the attached UtxoStore (if any).
    pub fn store_mut(&mut self) -> Option<&mut UtxoStore> {
        self.store.as_mut()
    }

    /// Whether this UtxoSet is backed by an on-disk UtxoStore.
    pub fn has_store(&self) -> bool {
        self.store.is_some()
    }

    /// Enable or disable address index maintenance.
    /// When disabled, `insert()` and `remove()` skip address index updates.
    /// Call `rebuild_address_index()` after re-enabling.
    pub fn set_indexing_enabled(&mut self, enabled: bool) {
        self.indexing_enabled = enabled;
        if let Some(ref mut store) = self.store {
            store.set_indexing_enabled(enabled);
        }
    }

    /// Enable or disable the WAL on the underlying LSM store.
    pub fn set_wal_enabled(&mut self, enabled: bool) {
        if let Some(ref mut store) = self.store {
            store.set_wal_enabled(enabled);
        }
    }

    /// Rebuild the address index from the UTxO map.
    /// Must be called after deserialization since the index is not serialized.
    pub fn rebuild_address_index(&mut self) {
        if let Some(ref mut store) = self.store {
            store.rebuild_address_index();
            return;
        }
        self.address_index.clear();
        for (input, output) in &self.utxos {
            self.address_index
                .entry(output.address.clone())
                .or_default()
                .push(input.clone());
        }
    }

    pub fn len(&self) -> usize {
        if let Some(ref store) = self.store {
            return store.len();
        }
        self.utxos.len()
    }

    pub fn is_empty(&self) -> bool {
        if let Some(ref store) = self.store {
            return store.is_empty();
        }
        self.utxos.is_empty()
    }

    /// Number of addresses in the secondary index
    pub fn address_index_size(&self) -> usize {
        if let Some(ref store) = self.store {
            return store.address_index_size();
        }
        self.address_index.len()
    }

    /// Look up a UTxO by input reference.
    /// Returns an owned value (cloned from HashMap or deserialized from LSM).
    pub fn lookup(&self, input: &TransactionInput) -> Option<TransactionOutput> {
        if let Some(ref store) = self.store {
            return store.lookup(input);
        }
        self.utxos.get(input).cloned()
    }

    /// Insert a new UTxO
    pub fn insert(&mut self, input: TransactionInput, output: TransactionOutput) {
        if let Some(ref mut store) = self.store {
            store.insert(input, output);
            return;
        }
        if self.indexing_enabled {
            self.address_index
                .entry(output.address.clone())
                .or_default()
                .push(input.clone());
        }
        self.utxos.insert(input, output);
    }

    /// Remove a UTxO (mark as spent)
    pub fn remove(&mut self, input: &TransactionInput) -> Option<TransactionOutput> {
        if let Some(ref mut store) = self.store {
            return store.remove(input);
        }
        if let Some(output) = self.utxos.remove(input) {
            if self.indexing_enabled {
                if let Some(inputs) = self.address_index.get_mut(&output.address) {
                    inputs.retain(|i| i != input);
                    if inputs.is_empty() {
                        self.address_index.remove(&output.address);
                    }
                }
            }
            Some(output)
        } else {
            None
        }
    }

    /// Check if a UTxO exists
    pub fn contains(&self, input: &TransactionInput) -> bool {
        if let Some(ref store) = self.store {
            return store.contains(input);
        }
        self.utxos.contains_key(input)
    }

    /// Apply a transaction: consume inputs, produce outputs
    pub fn apply_transaction(
        &mut self,
        tx_hash: &TransactionHash,
        inputs: &[TransactionInput],
        outputs: &[TransactionOutput],
    ) -> Result<(), UtxoError> {
        if let Some(ref mut store) = self.store {
            return store.apply_transaction(tx_hash, inputs, outputs);
        }
        // Validate all inputs exist
        for input in inputs {
            if !self.contains(input) {
                return Err(UtxoError::InputNotFound(input.clone()));
            }
        }

        // Remove spent inputs
        for input in inputs {
            self.remove(input);
        }

        // Add new outputs
        for (idx, output) in outputs.iter().enumerate() {
            let new_input = TransactionInput {
                transaction_id: *tx_hash,
                index: idx as u32,
            };
            self.insert(new_input, output.clone());
        }

        Ok(())
    }

    /// Rollback a transaction: restore inputs, remove outputs
    pub fn rollback_transaction(
        &mut self,
        tx_hash: &TransactionHash,
        inputs: &[(TransactionInput, TransactionOutput)],
        output_count: usize,
    ) {
        if let Some(ref mut store) = self.store {
            store.rollback_transaction(tx_hash, inputs, output_count);
            return;
        }
        // Remove outputs that were added
        for idx in 0..output_count {
            let input = TransactionInput {
                transaction_id: *tx_hash,
                index: idx as u32,
            };
            self.remove(&input);
        }

        // Restore spent inputs
        for (input, output) in inputs {
            self.insert(input.clone(), output.clone());
        }
    }

    /// Calculate total ADA in the UTxO set
    pub fn total_lovelace(&self) -> Lovelace {
        if let Some(ref store) = self.store {
            return store.total_lovelace();
        }
        self.utxos.values().fold(Lovelace(0), |acc, output| {
            Lovelace(acc.0 + output.value.coin.0)
        })
    }

    /// Get all UTxOs at a specific address.
    /// Returns owned tuples (compatible with both in-memory and on-disk backends).
    pub fn utxos_at_address(
        &self,
        address: &Address,
    ) -> Vec<(TransactionInput, TransactionOutput)> {
        if let Some(ref store) = self.store {
            return store.utxos_at_address(address);
        }
        match self.address_index.get(address) {
            Some(inputs) => inputs
                .iter()
                .filter_map(|input| {
                    self.utxos
                        .get(input)
                        .map(|output| (input.clone(), output.clone()))
                })
                .collect(),
            None => Vec::new(),
        }
    }

    /// Iterate over all UTxOs.
    /// Returns owned tuples (compatible with both in-memory and on-disk backends).
    /// For on-disk stores, this performs a full LSM scan — use sparingly.
    ///
    /// Prefer [`scan_all`] for hot paths: this helper materialises the whole
    /// set in a single `Vec`, which at preview scale (~3M UTxOs) is multiple
    /// GB and was implicated in the #403 post-replay OOM hang. Retained for
    /// test callers that want a concrete collection.
    ///
    /// [`scan_all`]: Self::scan_all
    pub fn iter(&self) -> Vec<(TransactionInput, TransactionOutput)> {
        let mut out = Vec::with_capacity(self.len());
        self.scan_all(|input, output| out.push((input.clone(), output.clone())));
        out
    }

    /// Stream every live UTxO entry into a callback without materialising
    /// the full set in memory.
    ///
    /// For the in-memory backend this is a direct `HashMap` walk; for the
    /// LSM-backed store it delegates to
    /// [`UtxoStore::scan_all`](crate::utxo_store::UtxoStore::scan_all),
    /// which splits the key space into 256 chunks so peak memory stays
    /// bounded regardless of UTxO set size (#403).
    pub fn scan_all<F>(&self, mut f: F)
    where
        F: FnMut(&TransactionInput, &TransactionOutput),
    {
        if let Some(ref store) = self.store {
            store.scan_all(|input, output| f(&input, &output));
            return;
        }
        for (k, v) in &self.utxos {
            f(k, v);
        }
    }
}

impl Clone for UtxoSet {
    fn clone(&self) -> Self {
        UtxoSet {
            utxos: self.utxos.clone(),
            address_index: self.address_index.clone(),
            indexing_enabled: self.indexing_enabled,
            // UtxoStore is not cloneable — clone gets in-memory mode.
            // This is fine: LedgerState clones are only used in tests,
            // and production code shares via Arc<RwLock<LedgerState>>.
            store: None,
        }
    }
}

impl Default for UtxoSet {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum UtxoError {
    #[error("Input not found in UTxO set: {0}")]
    InputNotFound(TransactionInput),
    #[error("Duplicate output: {0}")]
    DuplicateOutput(TransactionInput),
}

#[cfg(test)]
mod tests {
    use super::*;
    use dugite_primitives::address::Address;
    use dugite_primitives::hash::Hash32;
    use dugite_primitives::transaction::OutputDatum;
    use dugite_primitives::value::Value;

    fn make_output(lovelace: u64) -> TransactionOutput {
        TransactionOutput {
            address: Address::Byron(dugite_primitives::address::ByronAddress {
                payload: vec![0u8; 32],
            }),
            value: Value::lovelace(lovelace),
            datum: OutputDatum::None,
            script_ref: None,
            is_legacy: false,
            raw_cbor: None,
        }
    }

    #[test]
    fn test_utxo_insert_lookup() {
        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::ZERO,
            index: 0,
        };
        let output = make_output(5_000_000);
        utxo_set.insert(input.clone(), output.clone());

        assert_eq!(utxo_set.len(), 1);
        assert!(utxo_set.contains(&input));
        assert_eq!(utxo_set.lookup(&input).unwrap().value.coin.0, 5_000_000);
    }

    #[test]
    fn test_utxo_remove() {
        let mut utxo_set = UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::ZERO,
            index: 0,
        };
        utxo_set.insert(input.clone(), make_output(1_000_000));
        assert_eq!(utxo_set.len(), 1);

        let removed = utxo_set.remove(&input);
        assert!(removed.is_some());
        assert_eq!(utxo_set.len(), 0);
    }

    #[test]
    fn test_apply_transaction() {
        let mut utxo_set = UtxoSet::new();

        // Create initial UTxO
        let genesis_input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        utxo_set.insert(genesis_input.clone(), make_output(10_000_000));

        // Apply a transaction spending the genesis UTxO
        let tx_hash = Hash32::from_bytes([2u8; 32]);
        let inputs = vec![genesis_input.clone()];
        let outputs = vec![make_output(7_000_000), make_output(3_000_000)];

        utxo_set
            .apply_transaction(&tx_hash, &inputs, &outputs)
            .unwrap();

        // Genesis UTxO should be spent
        assert!(!utxo_set.contains(&genesis_input));
        // New UTxOs should exist
        assert_eq!(utxo_set.len(), 2);

        let new_input_0 = TransactionInput {
            transaction_id: tx_hash,
            index: 0,
        };
        let new_input_1 = TransactionInput {
            transaction_id: tx_hash,
            index: 1,
        };
        assert_eq!(
            utxo_set.lookup(&new_input_0).unwrap().value.coin.0,
            7_000_000
        );
        assert_eq!(
            utxo_set.lookup(&new_input_1).unwrap().value.coin.0,
            3_000_000
        );
    }

    #[test]
    fn test_apply_transaction_missing_input() {
        let mut utxo_set = UtxoSet::new();
        let tx_hash = Hash32::from_bytes([2u8; 32]);
        let missing_input = TransactionInput {
            transaction_id: Hash32::from_bytes([99u8; 32]),
            index: 0,
        };

        let result = utxo_set.apply_transaction(&tx_hash, &[missing_input], &[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_total_lovelace() {
        let mut utxo_set = UtxoSet::new();
        utxo_set.insert(
            TransactionInput {
                transaction_id: Hash32::from_bytes([1u8; 32]),
                index: 0,
            },
            make_output(5_000_000),
        );
        utxo_set.insert(
            TransactionInput {
                transaction_id: Hash32::from_bytes([2u8; 32]),
                index: 0,
            },
            make_output(3_000_000),
        );

        assert_eq!(utxo_set.total_lovelace(), Lovelace(8_000_000));
    }

    #[test]
    fn test_rollback_transaction() {
        let mut utxo_set = UtxoSet::new();

        let genesis_input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        let genesis_output = make_output(10_000_000);
        utxo_set.insert(genesis_input.clone(), genesis_output.clone());

        // Apply
        let tx_hash = Hash32::from_bytes([2u8; 32]);
        let outputs = vec![make_output(7_000_000), make_output(3_000_000)];
        utxo_set
            .apply_transaction(&tx_hash, std::slice::from_ref(&genesis_input), &outputs)
            .unwrap();

        // Rollback
        utxo_set.rollback_transaction(
            &tx_hash,
            &[(genesis_input.clone(), genesis_output.clone())],
            2,
        );

        // Original UTxO should be restored
        assert!(utxo_set.contains(&genesis_input));
        assert_eq!(utxo_set.len(), 1);
        assert_eq!(
            utxo_set.lookup(&genesis_input).unwrap().value.coin.0,
            10_000_000
        );
    }

    #[test]
    fn test_address_index() {
        let mut utxo_set = UtxoSet::new();

        let addr_a = Address::Byron(dugite_primitives::address::ByronAddress {
            payload: vec![1u8; 32],
        });
        let addr_b = Address::Byron(dugite_primitives::address::ByronAddress {
            payload: vec![2u8; 32],
        });

        let make_output_with_addr = |lovelace: u64, addr: &Address| TransactionOutput {
            address: addr.clone(),
            value: Value::lovelace(lovelace),
            datum: OutputDatum::None,
            script_ref: None,
            is_legacy: false,
            raw_cbor: None,
        };

        // Insert UTxOs at different addresses
        let input_a1 = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        let input_a2 = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 1,
        };
        let input_b1 = TransactionInput {
            transaction_id: Hash32::from_bytes([2u8; 32]),
            index: 0,
        };

        utxo_set.insert(input_a1.clone(), make_output_with_addr(1_000_000, &addr_a));
        utxo_set.insert(input_a2.clone(), make_output_with_addr(2_000_000, &addr_a));
        utxo_set.insert(input_b1.clone(), make_output_with_addr(3_000_000, &addr_b));

        // Query by address
        let a_utxos = utxo_set.utxos_at_address(&addr_a);
        assert_eq!(a_utxos.len(), 2);
        let b_utxos = utxo_set.utxos_at_address(&addr_b);
        assert_eq!(b_utxos.len(), 1);

        // Remove one UTxO from addr_a
        utxo_set.remove(&input_a1);
        let a_utxos = utxo_set.utxos_at_address(&addr_a);
        assert_eq!(a_utxos.len(), 1);

        // Remove the last UTxO at addr_b — address entry should be cleaned up
        utxo_set.remove(&input_b1);
        let b_utxos = utxo_set.utxos_at_address(&addr_b);
        assert_eq!(b_utxos.len(), 0);
    }

    #[test]
    fn test_rebuild_address_index() {
        let mut utxo_set = UtxoSet::new();
        let addr = Address::Byron(dugite_primitives::address::ByronAddress {
            payload: vec![1u8; 32],
        });

        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        utxo_set.insert(
            input,
            TransactionOutput {
                address: addr.clone(),
                value: Value::lovelace(5_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            },
        );

        // Clear the index to simulate deserialization
        utxo_set.address_index.clear();
        assert_eq!(utxo_set.utxos_at_address(&addr).len(), 0);

        // Rebuild
        utxo_set.rebuild_address_index();
        assert_eq!(utxo_set.utxos_at_address(&addr).len(), 1);
    }

    #[test]
    fn test_with_store_backend() {
        let store = UtxoStore::new_temp().unwrap();
        let mut utxo_set = UtxoSet::new();
        utxo_set.attach_store(store);

        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([1u8; 32]),
            index: 0,
        };
        let output = make_output(5_000_000);
        utxo_set.insert(input.clone(), output);

        assert_eq!(utxo_set.len(), 1);
        assert!(utxo_set.contains(&input));
        assert_eq!(utxo_set.lookup(&input).unwrap().value.coin.0, 5_000_000);
        assert!(utxo_set.has_store());

        utxo_set.remove(&input);
        assert_eq!(utxo_set.len(), 0);
    }
}
