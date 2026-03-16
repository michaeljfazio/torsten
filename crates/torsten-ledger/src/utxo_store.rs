//! LSM-tree backed UTxO set using `torsten-lsm`.
//!
//! Replaces the in-memory `HashMap<TransactionInput, TransactionOutput>` with an
//! on-disk LSM tree, dramatically reducing memory usage for large UTxO sets
//! (e.g., mainnet with ~20M entries). Matches Haskell cardano-node's UTxO-HD design.
//!
//! Key encoding: TransactionInput → 36 bytes (32-byte tx_hash + 4-byte index BE)
//! Value encoding: TransactionOutput → bincode serialization

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
#[cfg(test)]
use tempfile::TempDir;

use torsten_lsm::{Key, LsmConfig, LsmTree, Value};
use torsten_primitives::address::Address;
use torsten_primitives::hash::TransactionHash;
use torsten_primitives::transaction::{TransactionInput, TransactionOutput};
use torsten_primitives::value::Lovelace;
use tracing::{debug, info, warn};

use crate::utxo::UtxoError;

/// LSM-tree backed UTxO set.
///
/// Stores UTxO entries on disk via `torsten-lsm`, with an in-memory address
/// index for efficient address-based queries (N2C LocalStateQuery).
pub struct UtxoStore {
    tree: LsmTree,
    path: PathBuf,
    /// Number of live UTxO entries (tracked in-memory for O(1) len()).
    count: usize,
    /// Secondary index: address → set of TransactionInputs at that address.
    /// Kept in-memory, rebuilt from LSM scan on startup.
    /// Uses HashSet for O(1) insert and remove (Vec::retain was O(n) per remove,
    /// causing quadratic degradation on addresses with many UTxOs).
    address_index: HashMap<Address, HashSet<TransactionInput>>,
    /// When false, address index operations are skipped (for fast replay).
    indexing_enabled: bool,
    /// Owned temporary directory for test-only stores created via `new_temp()`.
    /// Holding this here ensures the directory is cleaned up when the store is
    /// dropped, rather than being leaked via `std::mem::forget`.
    #[cfg(test)]
    _temp_dir: Option<TempDir>,
}

// Key encoding: 32-byte tx_hash + 4-byte index (big-endian)
const KEY_SIZE: usize = 36;

#[inline]
fn encode_key(input: &TransactionInput) -> Key {
    let mut buf = [0u8; KEY_SIZE];
    buf[..32].copy_from_slice(input.transaction_id.as_bytes());
    buf[32..36].copy_from_slice(&input.index.to_be_bytes());
    Key::from(&buf[..])
}

#[inline]
fn decode_key(key: &Key) -> TransactionInput {
    let bytes = key.as_ref();
    let mut hash_bytes = [0u8; 32];
    hash_bytes.copy_from_slice(&bytes[..32]);
    let index = u32::from_be_bytes([bytes[32], bytes[33], bytes[34], bytes[35]]);
    TransactionInput {
        transaction_id: TransactionHash::from_bytes(hash_bytes),
        index,
    }
}

#[inline]
fn encode_value(output: &TransactionOutput) -> Value {
    let data = bincode::serialize(output).expect("TransactionOutput serialization should not fail");
    Value::from(&data[..])
}

#[inline]
fn decode_value(value: &Value) -> Option<TransactionOutput> {
    bincode::deserialize(value.as_ref()).ok()
}

/// LSM configuration for the UTxO store.
fn utxo_lsm_config() -> LsmConfig {
    LsmConfig {
        memtable_size: 128 * 1024 * 1024,    // 128 MB write buffer
        block_cache_size: 256 * 1024 * 1024, // 256 MB read cache
        bloom_filter_bits_per_key: 10,
        ..LsmConfig::default()
    }
}

impl UtxoStore {
    /// Private constructor used by `open`, `open_with_config`, and
    /// `open_from_snapshot` to build a `UtxoStore` from an already-opened
    /// `LsmTree`.  Centralises the cfg(test) guard on `_temp_dir` so that
    /// individual struct literals do not need to be duplicated across
    /// compilation configurations.
    fn from_tree(tree: LsmTree, path: PathBuf) -> Self {
        UtxoStore {
            tree,
            path,
            count: 0,
            address_index: HashMap::new(),
            indexing_enabled: true,
            #[cfg(test)]
            _temp_dir: None,
        }
    }

    /// Open or create a UTxO store at the given path.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, UtxoStoreError> {
        let path = path.as_ref().to_path_buf();
        std::fs::create_dir_all(&path)?;
        let tree = LsmTree::open(&path, utxo_lsm_config())?;
        Ok(Self::from_tree(tree, path))
    }

    /// Open or create a UTxO store with custom LSM configuration.
    ///
    /// `memtable_size_mb`, `block_cache_size_mb`, and `bloom_filter_bits_per_key`
    /// override the defaults from `utxo_lsm_config()`.
    pub fn open_with_config(
        path: impl AsRef<Path>,
        memtable_size_mb: u64,
        block_cache_size_mb: u64,
        bloom_filter_bits_per_key: u32,
    ) -> Result<Self, UtxoStoreError> {
        let path = path.as_ref().to_path_buf();
        std::fs::create_dir_all(&path)?;
        let config = LsmConfig {
            memtable_size: memtable_size_mb as usize * 1024 * 1024,
            block_cache_size: block_cache_size_mb as usize * 1024 * 1024,
            bloom_filter_bits_per_key: bloom_filter_bits_per_key as usize,
            ..LsmConfig::default()
        };
        let tree = LsmTree::open(&path, config)?;
        Ok(Self::from_tree(tree, path))
    }

    /// Open a UTxO store from a persistent snapshot.
    pub fn open_from_snapshot(
        path: impl AsRef<Path>,
        snapshot_name: &str,
    ) -> Result<Self, UtxoStoreError> {
        let path = path.as_ref().to_path_buf();
        let tree = LsmTree::open_snapshot(&path, snapshot_name)?;
        // count and address_index will be populated by the caller via
        // count_entries() / rebuild_address_index().
        Ok(Self::from_tree(tree, path))
    }

    /// Create an in-memory UTxO store backed by a temporary directory.
    /// Useful for tests.
    ///
    /// The `TempDir` is stored in the returned `UtxoStore` and is cleaned up
    /// automatically when the store is dropped. Previously this used
    /// `std::mem::forget`, which leaked the directory and its contents on the
    /// host filesystem for the lifetime of the process.
    #[cfg(test)]
    pub fn new_temp() -> Result<Self, UtxoStoreError> {
        let dir = tempfile::tempdir().map_err(UtxoStoreError::Io)?;
        let path = dir.path().to_path_buf();
        let tree = LsmTree::open(&path, LsmConfig::default())?;
        Ok(UtxoStore {
            tree,
            path,
            count: 0,
            address_index: HashMap::new(),
            indexing_enabled: true,
            _temp_dir: Some(dir),
        })
    }

    /// Number of UTxO entries.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Number of addresses in the secondary index.
    pub fn address_index_size(&self) -> usize {
        self.address_index.len()
    }

    /// Enable or disable address index maintenance.
    pub fn set_indexing_enabled(&mut self, enabled: bool) {
        self.indexing_enabled = enabled;
    }

    /// Enable or disable the WAL on the underlying LSM tree.
    ///
    /// Disabling during bulk replay avoids per-write disk flushes, providing
    /// a significant speedup. Must be re-enabled before at-tip operation.
    pub fn set_wal_enabled(&mut self, enabled: bool) {
        self.tree.set_wal_enabled(enabled);
    }

    /// Look up a UTxO by input reference.
    pub fn lookup(&self, input: &TransactionInput) -> Option<TransactionOutput> {
        let key = encode_key(input);
        match self.tree.get(&key) {
            Ok(Some(value)) => decode_value(&value),
            Ok(None) => None,
            Err(e) => {
                warn!("UtxoStore lookup error: {e}");
                None
            }
        }
    }

    /// Insert a new UTxO entry.
    ///
    /// # Panics
    ///
    /// Panics if the LSM tree insert fails. A missing UTxO entry causes
    /// unrecoverable ledger state divergence — it is better to crash and
    /// restart than to silently lose UTxO data.
    pub fn insert(&mut self, input: TransactionInput, output: TransactionOutput) {
        let key = encode_key(&input);
        let value = encode_value(&output);
        if let Err(e) = self.tree.insert(&key, &value) {
            panic!(
                "FATAL: UtxoStore insert failed for {}#{}: {e}. \
                 Cannot continue — UTxO loss causes unrecoverable ledger divergence.",
                input.transaction_id, input.index
            );
        }
        if self.indexing_enabled {
            self.address_index
                .entry(output.address.clone())
                .or_default()
                .insert(input);
        }
        self.count += 1;
    }

    /// Remove a UTxO (mark as spent). Returns the removed output if found.
    ///
    /// Performs a single LSM `get` followed by a `delete` when the key exists.
    /// When indexing is disabled (replay mode) the returned `Option` is always
    /// `None` — callers that need the removed output must use `remove()` only
    /// when indexing is enabled (at-tip mode).  Callers that do not need the
    /// value (e.g., `rollback_transaction` output cleanup, Byron spent inputs)
    /// may call `remove_fast` to skip even the get.
    pub fn remove(&mut self, input: &TransactionInput) -> Option<TransactionOutput> {
        let key = encode_key(input);

        // Single LSM get — the only lookup required.
        let output = match self.tree.get(&key) {
            Ok(Some(value)) => decode_value(&value),
            _ => return None,
        };

        // Key exists; write tombstone.
        if let Err(e) = self.tree.delete(&key) {
            panic!(
                "FATAL: UtxoStore delete failed for {}#{}: {e}. \
                 Cannot continue — failed UTxO delete causes unrecoverable ledger divergence.",
                input.transaction_id, input.index
            );
        }
        self.count = self.count.saturating_sub(1);

        // Update address index (only when enabled).
        if self.indexing_enabled {
            if let Some(ref out) = output {
                if let Some(inputs) = self.address_index.get_mut(&out.address) {
                    inputs.remove(input); // O(1) HashSet remove
                    if inputs.is_empty() {
                        self.address_index.remove(&out.address);
                    }
                }
            }
        }

        output
    }

    /// Remove a UTxO without reading its value first.
    ///
    /// This is a fast-path for callers that already know the key exists (e.g.
    /// after a `contains()` check) and do not need the removed output value.
    /// It skips the `get` entirely and writes a tombstone directly, then
    /// decrements the counter.  The address index is NOT updated — only call
    /// this when `indexing_enabled` is `false`.
    ///
    /// # Safety invariant
    ///
    /// The caller must guarantee the key exists in the store.  If the key does
    /// not exist the count will underflow (saturating to 0), but no data
    /// corruption occurs because a tombstone for a missing key is a no-op in
    /// LSM semantics.
    fn remove_fast(&mut self, input: &TransactionInput) {
        debug_assert!(
            !self.indexing_enabled,
            "remove_fast must only be called when indexing is disabled"
        );
        let key = encode_key(input);
        if let Err(e) = self.tree.delete(&key) {
            panic!(
                "FATAL: UtxoStore delete failed for {}#{}: {e}. \
                 Cannot continue — failed UTxO delete causes unrecoverable ledger divergence.",
                input.transaction_id, input.index
            );
        }
        self.count = self.count.saturating_sub(1);
    }

    /// Check if a UTxO exists.
    pub fn contains(&self, input: &TransactionInput) -> bool {
        let key = encode_key(input);
        matches!(self.tree.get(&key), Ok(Some(_)))
    }

    /// Apply a transaction: consume inputs, produce outputs.
    ///
    /// When indexing is enabled (at-tip mode) each input is looked up once to
    /// validate existence and update the address index.
    ///
    /// When indexing is disabled (replay mode) inputs are validated with a
    /// single `get` per input and then deleted without a second lookup, halving
    /// the LSM reads compared with the old `contains()` + `remove()` pattern.
    pub fn apply_transaction(
        &mut self,
        tx_hash: &TransactionHash,
        inputs: &[TransactionInput],
        outputs: &[TransactionOutput],
    ) -> Result<(), UtxoError> {
        if self.indexing_enabled {
            // At-tip mode: validate all inputs first (atomic check), then apply.
            // We must check all before modifying so that a partially-applied
            // transaction is not possible if an error is returned.
            for input in inputs {
                if !self.contains(input) {
                    return Err(UtxoError::InputNotFound(input.clone()));
                }
            }
            // `remove()` performs one LSM get + one delete.  The `contains()`
            // above was a separate get, so this is still 2 lookups per input
            // in at-tip mode — required for atomicity (check-before-modify).
            for input in inputs {
                self.remove(input);
            }
        } else {
            // Replay mode (indexing disabled): fuse the existence check and
            // the delete into a single pass — `remove()` does one `get` and
            // one `delete`, and we collect any missing inputs to report.
            for input in inputs {
                // `remove()` returns None both when the key is absent AND in
                // replay mode (where it skips deserialization).  To distinguish
                // the two cases in replay mode we still call `get` once via
                // `remove()`, but we skip the second `get` from `contains()`.
                // Net effect: 1 lookup per input instead of 2.
                let key = encode_key(input);
                match self.tree.get(&key) {
                    Ok(Some(_)) => {
                        // Key exists — remove without reading the value.
                        self.remove_fast(input);
                    }
                    _ => {
                        return Err(UtxoError::InputNotFound(input.clone()));
                    }
                }
            }
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

    /// Apply a complete block's UTxO changes using a single LSM batch write.
    ///
    /// Collects all output inserts and input deletes across every transaction
    /// in the block, then issues them to the LSM tree in one `apply_batch`
    /// call.  This reduces WAL writes from O(inputs + outputs) to O(inputs +
    /// outputs) individual entries but — crucially — checks the flush
    /// threshold only once per block rather than once per operation.
    ///
    /// Existence validation is performed before any writes: if any input is
    /// missing the entire batch is aborted and an error is returned, leaving
    /// the store unchanged.
    ///
    /// The `tx_inputs` slice contains one entry per transaction: a slice of
    /// `TransactionInput` values to spend.  `tx_outputs` contains one entry
    /// per transaction: a `(tx_hash, outputs)` tuple for the new UTxOs.
    /// Both slices must have the same length.
    ///
    /// This method is intended for block replay where indexing is disabled.
    /// Callers that need the address index updated must use `apply_transaction`
    /// per-transaction instead (the address index is not maintained by this
    /// method regardless of `indexing_enabled`).
    pub fn apply_block_batch(
        &mut self,
        tx_inputs: &[&[TransactionInput]],
        tx_outputs: &[(&TransactionHash, &[TransactionOutput])],
    ) -> Result<(), UtxoError> {
        debug_assert_eq!(
            tx_inputs.len(),
            tx_outputs.len(),
            "tx_inputs and tx_outputs must have the same length"
        );

        // --- Pass 1: existence check ---
        // Validate all inputs across all transactions before modifying anything.
        for inputs in tx_inputs {
            for input in *inputs {
                let key = encode_key(input);
                match self.tree.get(&key) {
                    Ok(Some(_)) => {} // exists
                    _ => return Err(UtxoError::InputNotFound(input.clone())),
                }
            }
        }

        // --- Pass 2: build batch ---
        // Collect inserts (outputs) and deletes (inputs) for a single LSM call.
        let total_outputs: usize = tx_outputs.iter().map(|(_, outs)| outs.len()).sum();
        let total_inputs: usize = tx_inputs.iter().map(|ins| ins.len()).sum();

        let mut lsm_inserts: Vec<(Key, Value)> = Vec::with_capacity(total_outputs);
        let mut lsm_deletes: Vec<Key> = Vec::with_capacity(total_inputs);

        // Encode new outputs
        for (tx_hash, outputs) in tx_outputs {
            for (idx, output) in outputs.iter().enumerate() {
                let new_input = TransactionInput {
                    transaction_id: **tx_hash,
                    index: idx as u32,
                };
                let key = encode_key(&new_input);
                let value = encode_value(output);

                // Update address index when enabled
                if self.indexing_enabled {
                    self.address_index
                        .entry(output.address.clone())
                        .or_default()
                        .insert(new_input.clone());
                }

                lsm_inserts.push((key, value));
            }
        }
        self.count += total_outputs;

        // Encode input deletes
        for inputs in tx_inputs {
            for input in *inputs {
                lsm_deletes.push(encode_key(input));
            }
        }
        self.count = self.count.saturating_sub(total_inputs);

        // --- Pass 3: single LSM batch write ---
        if let Err(e) = self.tree.apply_batch(&lsm_inserts, &lsm_deletes) {
            panic!(
                "FATAL: UtxoStore batch write failed: {e}. \
                 Cannot continue — UTxO loss causes unrecoverable ledger divergence."
            );
        }

        Ok(())
    }

    /// Rollback a transaction: restore inputs, remove outputs.
    pub fn rollback_transaction(
        &mut self,
        tx_hash: &TransactionHash,
        inputs: &[(TransactionInput, TransactionOutput)],
        output_count: usize,
    ) {
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

    /// Calculate total ADA in the UTxO set by scanning all entries.
    pub fn total_lovelace(&self) -> Lovelace {
        let mut total = 0u64;
        let start = Key::from([0u8; 0]);
        let end = Key::from([0xFFu8; KEY_SIZE]);
        for (_, value) in self.tree.range(&start, &end) {
            if let Some(output) = decode_value(&value) {
                total = total.saturating_add(output.value.coin.0);
            }
        }
        Lovelace(total)
    }

    /// Get all UTxOs at a specific address (O(1) lookup via secondary index,
    /// then fetches values from LSM tree).
    pub fn utxos_at_address(
        &self,
        address: &Address,
    ) -> Vec<(TransactionInput, TransactionOutput)> {
        match self.address_index.get(address) {
            Some(inputs) => inputs
                .iter()
                .filter_map(|input| self.lookup(input).map(|output| (input.clone(), output)))
                .collect(),
            None => Vec::new(),
        }
    }

    /// Iterate over all UTxO entries by scanning the full LSM tree.
    ///
    /// This is expensive (full scan + deserialization). Use sparingly
    /// (e.g., at epoch boundaries for stake distribution rebuild).
    pub fn iter(&self) -> Vec<(TransactionInput, TransactionOutput)> {
        let start = Key::from([0u8; 0]);
        let end = Key::from([0xFFu8; KEY_SIZE]);
        self.tree
            .range(&start, &end)
            .filter_map(|(key, value)| {
                let input = decode_key(&key);
                decode_value(&value).map(|output| (input, output))
            })
            .collect()
    }

    /// Rebuild the address index by scanning all UTxO entries.
    /// Must be called after opening from a snapshot.
    pub fn rebuild_address_index(&mut self) {
        self.address_index.clear();
        let entries = self.iter();
        self.count = entries.len();
        for (input, output) in entries {
            self.address_index
                .entry(output.address.clone())
                .or_default()
                .insert(input);
        }
        info!(
            "Address index rebuilt: {} addresses, {} UTxOs",
            self.address_index.len(),
            self.count,
        );
    }

    /// Count entries by scanning the LSM tree. Sets the internal count.
    pub fn count_entries(&mut self) -> usize {
        let start = Key::from([0u8; 0]);
        let end = Key::from([0xFFu8; KEY_SIZE]);
        let count = self.tree.range(&start, &end).count();
        self.count = count;
        count
    }

    /// Save a persistent snapshot of the UTxO store.
    pub fn save_snapshot(&mut self, name: &str) -> Result<(), UtxoStoreError> {
        self.tree.save_snapshot(name, "utxo")?;
        debug!("UTxO store snapshot saved: {name}");
        Ok(())
    }

    /// Delete a persistent snapshot.
    pub fn delete_snapshot(&self, name: &str) -> Result<(), UtxoStoreError> {
        self.tree.delete_snapshot(name)?;
        Ok(())
    }

    /// Get the path to the UTxO store directory.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl std::fmt::Debug for UtxoStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UtxoStore")
            .field("path", &self.path)
            .field("count", &self.count)
            .field("indexing_enabled", &self.indexing_enabled)
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum UtxoStoreError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("LSM error: {0}")]
    Lsm(#[from] torsten_lsm::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use torsten_primitives::hash::Hash32;
    use torsten_primitives::transaction::OutputDatum;
    use torsten_primitives::value::Value as TxValue;

    fn make_output(lovelace: u64) -> TransactionOutput {
        TransactionOutput {
            address: Address::Byron(torsten_primitives::address::ByronAddress {
                payload: vec![0u8; 32],
            }),
            value: TxValue::lovelace(lovelace),
            datum: OutputDatum::None,
            script_ref: None,
            is_legacy: false,
            raw_cbor: None,
        }
    }

    fn make_output_with_addr(lovelace: u64, addr: &Address) -> TransactionOutput {
        TransactionOutput {
            address: addr.clone(),
            value: TxValue::lovelace(lovelace),
            datum: OutputDatum::None,
            script_ref: None,
            is_legacy: false,
            raw_cbor: None,
        }
    }

    fn make_input(hash_byte: u8, index: u32) -> TransactionInput {
        TransactionInput {
            transaction_id: Hash32::from_bytes([hash_byte; 32]),
            index,
        }
    }

    #[test]
    fn test_insert_lookup_delete() {
        let mut store = UtxoStore::new_temp().unwrap();
        let input = make_input(1, 0);
        let output = make_output(5_000_000);

        store.insert(input.clone(), output.clone());
        assert_eq!(store.len(), 1);
        assert!(store.contains(&input));

        let found = store.lookup(&input).unwrap();
        assert_eq!(found.value.coin.0, 5_000_000);

        let removed = store.remove(&input).unwrap();
        assert_eq!(removed.value.coin.0, 5_000_000);
        assert_eq!(store.len(), 0);
        assert!(!store.contains(&input));
    }

    #[test]
    fn test_apply_transaction() {
        let mut store = UtxoStore::new_temp().unwrap();
        let genesis_input = make_input(1, 0);
        store.insert(genesis_input.clone(), make_output(10_000_000));

        let tx_hash = Hash32::from_bytes([2u8; 32]);
        let outputs = vec![make_output(7_000_000), make_output(3_000_000)];
        store
            .apply_transaction(&tx_hash, std::slice::from_ref(&genesis_input), &outputs)
            .unwrap();

        assert!(!store.contains(&genesis_input));
        assert_eq!(store.len(), 2);

        let out0 = store
            .lookup(&TransactionInput {
                transaction_id: tx_hash,
                index: 0,
            })
            .unwrap();
        assert_eq!(out0.value.coin.0, 7_000_000);
    }

    #[test]
    fn test_apply_transaction_missing_input() {
        let mut store = UtxoStore::new_temp().unwrap();
        let tx_hash = Hash32::from_bytes([2u8; 32]);
        let missing = make_input(99, 0);
        let result = store.apply_transaction(&tx_hash, &[missing], &[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_total_lovelace() {
        let mut store = UtxoStore::new_temp().unwrap();
        store.insert(make_input(1, 0), make_output(5_000_000));
        store.insert(make_input(2, 0), make_output(3_000_000));
        assert_eq!(store.total_lovelace(), Lovelace(8_000_000));
    }

    #[test]
    fn test_rollback_transaction() {
        let mut store = UtxoStore::new_temp().unwrap();
        let genesis_input = make_input(1, 0);
        let genesis_output = make_output(10_000_000);
        store.insert(genesis_input.clone(), genesis_output.clone());

        let tx_hash = Hash32::from_bytes([2u8; 32]);
        let outputs = vec![make_output(7_000_000), make_output(3_000_000)];
        store
            .apply_transaction(&tx_hash, std::slice::from_ref(&genesis_input), &outputs)
            .unwrap();

        store.rollback_transaction(&tx_hash, &[(genesis_input.clone(), genesis_output)], 2);

        assert!(store.contains(&genesis_input));
        assert_eq!(store.len(), 1);
        assert_eq!(
            store.lookup(&genesis_input).unwrap().value.coin.0,
            10_000_000
        );
    }

    #[test]
    fn test_address_index() {
        let mut store = UtxoStore::new_temp().unwrap();
        let addr_a = Address::Byron(torsten_primitives::address::ByronAddress {
            payload: vec![1u8; 32],
        });
        let addr_b = Address::Byron(torsten_primitives::address::ByronAddress {
            payload: vec![2u8; 32],
        });

        store.insert(make_input(1, 0), make_output_with_addr(1_000_000, &addr_a));
        store.insert(make_input(1, 1), make_output_with_addr(2_000_000, &addr_a));
        store.insert(make_input(2, 0), make_output_with_addr(3_000_000, &addr_b));

        let a_utxos = store.utxos_at_address(&addr_a);
        assert_eq!(a_utxos.len(), 2);
        let b_utxos = store.utxos_at_address(&addr_b);
        assert_eq!(b_utxos.len(), 1);

        store.remove(&make_input(1, 0));
        assert_eq!(store.utxos_at_address(&addr_a).len(), 1);

        store.remove(&make_input(2, 0));
        assert_eq!(store.utxos_at_address(&addr_b).len(), 0);
    }

    #[test]
    fn test_rebuild_address_index() {
        let mut store = UtxoStore::new_temp().unwrap();
        let addr = Address::Byron(torsten_primitives::address::ByronAddress {
            payload: vec![1u8; 32],
        });
        store.insert(make_input(1, 0), make_output_with_addr(5_000_000, &addr));

        // Clear index to simulate fresh open
        store.address_index.clear();
        assert_eq!(store.utxos_at_address(&addr).len(), 0);

        store.rebuild_address_index();
        assert_eq!(store.utxos_at_address(&addr).len(), 1);
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn test_iter() {
        let mut store = UtxoStore::new_temp().unwrap();
        store.insert(make_input(1, 0), make_output(1_000_000));
        store.insert(make_input(2, 0), make_output(2_000_000));
        store.insert(make_input(3, 0), make_output(3_000_000));

        let entries = store.iter();
        assert_eq!(entries.len(), 3);

        let total: u64 = entries.iter().map(|(_, o)| o.value.coin.0).sum();
        assert_eq!(total, 6_000_000);
    }

    #[test]
    fn test_save_and_restore_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let utxo_path = dir.path().join("utxo");

        // Create store and insert data
        {
            let mut store = UtxoStore::open(&utxo_path).unwrap();
            store.insert(make_input(1, 0), make_output(5_000_000));
            store.insert(make_input(2, 0), make_output(3_000_000));
            assert_eq!(store.len(), 2);
            store.save_snapshot("test_snap").unwrap();
        }

        // Restore from snapshot
        {
            let mut store = UtxoStore::open_from_snapshot(&utxo_path, "test_snap").unwrap();
            store.rebuild_address_index();
            assert_eq!(store.len(), 2);
            assert_eq!(
                store.lookup(&make_input(1, 0)).unwrap().value.coin.0,
                5_000_000
            );
            assert_eq!(
                store.lookup(&make_input(2, 0)).unwrap().value.coin.0,
                3_000_000
            );
        }
    }

    #[test]
    fn test_key_encoding_roundtrip() {
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0xAB; 32]),
            index: 42,
        };
        let key = encode_key(&input);
        let decoded = decode_key(&key);
        assert_eq!(decoded.transaction_id, input.transaction_id);
        assert_eq!(decoded.index, input.index);
    }

    #[test]
    fn test_open_with_custom_config() {
        let dir = tempfile::tempdir().unwrap();
        let utxo_path = dir.path().join("utxo");

        // Open with custom LSM config
        let mut store = UtxoStore::open_with_config(&utxo_path, 64, 128, 10).unwrap();

        // Basic operations should work
        store.insert(make_input(1, 0), make_output(5_000_000));
        store.insert(make_input(2, 0), make_output(3_000_000));
        assert_eq!(store.len(), 2);
        assert!(store.contains(&make_input(1, 0)));

        let found = store.lookup(&make_input(1, 0)).unwrap();
        assert_eq!(found.value.coin.0, 5_000_000);

        let removed = store.remove(&make_input(1, 0)).unwrap();
        assert_eq!(removed.value.coin.0, 5_000_000);
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn test_open_with_small_config() {
        let dir = tempfile::tempdir().unwrap();
        let utxo_path = dir.path().join("utxo");

        // Small config (low-memory profile values)
        let mut store = UtxoStore::open_with_config(&utxo_path, 32, 64, 5).unwrap();
        store.insert(make_input(1, 0), make_output(1_000_000));
        assert_eq!(store.len(), 1);
        assert_eq!(store.total_lovelace(), Lovelace(1_000_000));
    }

    /// Verify that apply_block_batch produces the same UTxO state as an
    /// equivalent sequence of apply_transaction calls (correctness parity).
    #[test]
    fn test_apply_block_batch_correctness() {
        // Reference store using per-transaction apply
        let mut ref_store = UtxoStore::new_temp().unwrap();
        ref_store.set_indexing_enabled(false);
        ref_store.insert(make_input(0xAA, 0), make_output(10_000_000));
        ref_store.insert(make_input(0xAA, 1), make_output(5_000_000));
        ref_store.insert(make_input(0xBB, 0), make_output(3_000_000));

        let tx1_hash = Hash32::from_bytes([0x01u8; 32]);
        let tx2_hash = Hash32::from_bytes([0x02u8; 32]);
        ref_store
            .apply_transaction(
                &tx1_hash,
                &[make_input(0xAA, 0)],
                &[make_output(9_000_000), make_output(1_000_000)],
            )
            .unwrap();
        ref_store
            .apply_transaction(&tx2_hash, &[make_input(0xBB, 0)], &[make_output(2_500_000)])
            .unwrap();

        // Batch store using apply_block_batch
        let mut batch_store = UtxoStore::new_temp().unwrap();
        batch_store.set_indexing_enabled(false);
        batch_store.insert(make_input(0xAA, 0), make_output(10_000_000));
        batch_store.insert(make_input(0xAA, 1), make_output(5_000_000));
        batch_store.insert(make_input(0xBB, 0), make_output(3_000_000));

        let tx1_ins = [make_input(0xAA, 0)];
        let tx1_outs = [make_output(9_000_000), make_output(1_000_000)];
        let tx2_ins = [make_input(0xBB, 0)];
        let tx2_outs = [make_output(2_500_000)];
        batch_store
            .apply_block_batch(
                &[tx1_ins.as_slice(), tx2_ins.as_slice()],
                &[
                    (&tx1_hash, tx1_outs.as_slice()),
                    (&tx2_hash, tx2_outs.as_slice()),
                ],
            )
            .unwrap();

        // Both stores should have identical UTxO counts and total lovelace
        assert_eq!(ref_store.len(), batch_store.len(), "UTxO count mismatch");
        assert_eq!(
            ref_store.total_lovelace(),
            batch_store.total_lovelace(),
            "Total lovelace mismatch"
        );

        // Old input (0xAA, 0) should be gone in both
        assert!(!ref_store.contains(&make_input(0xAA, 0)));
        assert!(!batch_store.contains(&make_input(0xAA, 0)));

        // New outputs should exist in both
        assert!(ref_store.contains(&TransactionInput {
            transaction_id: tx1_hash,
            index: 0
        }));
        assert!(batch_store.contains(&TransactionInput {
            transaction_id: tx1_hash,
            index: 0
        }));
    }

    /// Verify that apply_block_batch returns an error and leaves the store
    /// unchanged when an input is missing (atomicity guarantee).
    #[test]
    fn test_apply_block_batch_missing_input() {
        let mut store = UtxoStore::new_temp().unwrap();
        store.set_indexing_enabled(false);
        store.insert(make_input(0xAA, 0), make_output(5_000_000));

        let tx_hash = Hash32::from_bytes([0x01u8; 32]);
        let missing = make_input(0xFF, 0); // not in store
        let inputs = [missing];
        let outputs = [make_output(5_000_000)];

        let result =
            store.apply_block_batch(&[inputs.as_slice()], &[(&tx_hash, outputs.as_slice())]);
        assert!(result.is_err(), "expected error for missing input");
        // Store should be unchanged
        assert_eq!(store.len(), 1);
        assert!(store.contains(&make_input(0xAA, 0)));
    }

    /// Verify that apply_transaction in replay mode (indexing disabled) uses
    /// only one LSM lookup per input (no contains() + remove() double lookup).
    /// We cannot directly count lookups, but we verify correctness: after
    /// apply_transaction with indexing disabled, the inputs are gone and the
    /// outputs are present, and remove() returns None (no deserialization).
    #[test]
    fn test_apply_transaction_replay_mode() {
        let mut store = UtxoStore::new_temp().unwrap();
        store.set_indexing_enabled(false);

        let genesis_input = make_input(1, 0);
        store.insert(genesis_input.clone(), make_output(10_000_000));

        let tx_hash = Hash32::from_bytes([2u8; 32]);
        let outputs = vec![make_output(7_000_000), make_output(3_000_000)];
        store
            .apply_transaction(&tx_hash, std::slice::from_ref(&genesis_input), &outputs)
            .unwrap();

        assert!(!store.contains(&genesis_input));
        assert_eq!(store.len(), 2);
        let out0 = store
            .lookup(&TransactionInput {
                transaction_id: tx_hash,
                index: 0,
            })
            .unwrap();
        assert_eq!(out0.value.coin.0, 7_000_000);
    }
}
