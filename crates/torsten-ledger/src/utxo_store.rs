//! LSM-tree backed UTxO set using `cardano-lsm`.
//!
//! Replaces the in-memory `HashMap<TransactionInput, TransactionOutput>` with an
//! on-disk LSM tree, dramatically reducing memory usage for large UTxO sets
//! (e.g., mainnet with ~20M entries). Matches Haskell cardano-node's UTxO-HD design.
//!
//! Key encoding: TransactionInput → 36 bytes (32-byte tx_hash + 4-byte index BE)
//! Value encoding: TransactionOutput → bincode serialization

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use cardano_lsm::{Key, LsmConfig, LsmTree, Value};
use torsten_primitives::address::Address;
use torsten_primitives::hash::TransactionHash;
use torsten_primitives::transaction::{TransactionInput, TransactionOutput};
use torsten_primitives::value::Lovelace;
use tracing::{debug, info, warn};

use crate::utxo::UtxoError;

/// LSM-tree backed UTxO set.
///
/// Stores UTxO entries on disk via `cardano-lsm`, with an in-memory address
/// index for efficient address-based queries (N2C LocalStateQuery).
pub struct UtxoStore {
    tree: LsmTree,
    path: PathBuf,
    /// Number of live UTxO entries (tracked in-memory for O(1) len()).
    count: usize,
    /// Secondary index: address → set of TransactionInputs at that address.
    /// Kept in-memory, rebuilt from LSM scan on startup.
    address_index: HashMap<Address, Vec<TransactionInput>>,
    /// When false, address index operations are skipped (for fast replay).
    indexing_enabled: bool,
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
    /// Open or create a UTxO store at the given path.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, UtxoStoreError> {
        let path = path.as_ref().to_path_buf();
        std::fs::create_dir_all(&path)?;
        let tree = LsmTree::open(&path, utxo_lsm_config())?;
        Ok(UtxoStore {
            tree,
            path,
            count: 0,
            address_index: HashMap::new(),
            indexing_enabled: true,
        })
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
        Ok(UtxoStore {
            tree,
            path,
            count: 0,
            address_index: HashMap::new(),
            indexing_enabled: true,
        })
    }

    /// Open a UTxO store from a persistent snapshot.
    pub fn open_from_snapshot(
        path: impl AsRef<Path>,
        snapshot_name: &str,
    ) -> Result<Self, UtxoStoreError> {
        let path = path.as_ref().to_path_buf();
        let tree = LsmTree::open_snapshot(&path, snapshot_name)?;
        Ok(UtxoStore {
            tree,
            path,
            count: 0, // Will be set by count_entries() or caller
            address_index: HashMap::new(),
            indexing_enabled: true,
        })
    }

    /// Create an in-memory UTxO store backed by a temporary directory.
    /// Useful for tests.
    pub fn new_temp() -> Result<Self, UtxoStoreError> {
        let dir = tempfile::tempdir().map_err(UtxoStoreError::Io)?;
        let path = dir.path().to_path_buf();
        let tree = LsmTree::open(&path, LsmConfig::default())?;
        // Leak the tempdir so the directory persists for the lifetime of the store
        std::mem::forget(dir);
        Ok(UtxoStore {
            tree,
            path,
            count: 0,
            address_index: HashMap::new(),
            indexing_enabled: true,
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
    pub fn insert(&mut self, input: TransactionInput, output: TransactionOutput) {
        let key = encode_key(&input);
        let value = encode_value(&output);
        if let Err(e) = self.tree.insert(&key, &value) {
            warn!("UtxoStore insert error: {e}");
            return;
        }
        if self.indexing_enabled {
            self.address_index
                .entry(output.address.clone())
                .or_default()
                .push(input);
        }
        self.count += 1;
    }

    /// Remove a UTxO (mark as spent). Returns the removed output if found.
    pub fn remove(&mut self, input: &TransactionInput) -> Option<TransactionOutput> {
        let key = encode_key(input);
        // Look up before delete to return the value and update address index
        let output = match self.tree.get(&key) {
            Ok(Some(value)) => decode_value(&value),
            _ => None,
        };
        if output.is_some() {
            if let Err(e) = self.tree.delete(&key) {
                warn!("UtxoStore delete error: {e}");
                return None;
            }
            self.count = self.count.saturating_sub(1);
            if self.indexing_enabled {
                if let Some(ref out) = output {
                    if let Some(inputs) = self.address_index.get_mut(&out.address) {
                        inputs.retain(|i| i != input);
                        if inputs.is_empty() {
                            self.address_index.remove(&out.address);
                        }
                    }
                }
            }
        }
        output
    }

    /// Check if a UTxO exists.
    pub fn contains(&self, input: &TransactionInput) -> bool {
        let key = encode_key(input);
        matches!(self.tree.get(&key), Ok(Some(_)))
    }

    /// Apply a transaction: consume inputs, produce outputs.
    pub fn apply_transaction(
        &mut self,
        tx_hash: &TransactionHash,
        inputs: &[TransactionInput],
        outputs: &[TransactionOutput],
    ) -> Result<(), UtxoError> {
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
                .push(input);
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
    Lsm(#[from] cardano_lsm::Error),
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
            raw_cbor: None,
        }
    }

    fn make_output_with_addr(lovelace: u64, addr: &Address) -> TransactionOutput {
        TransactionOutput {
            address: addr.clone(),
            value: TxValue::lovelace(lovelace),
            datum: OutputDatum::None,
            script_ref: None,
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
}
