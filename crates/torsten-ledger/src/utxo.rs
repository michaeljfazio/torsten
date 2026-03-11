use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use torsten_primitives::address::Address;
use torsten_primitives::hash::TransactionHash;
use torsten_primitives::transaction::{TransactionInput, TransactionOutput};
use torsten_primitives::value::Lovelace;

/// The UTxO set: maps transaction inputs to their unspent outputs.
/// Uses HashMap for O(1) amortized lookups (vs BTreeMap O(log n)).
/// Maintains a secondary index by address for O(1) address queries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UtxoSet {
    utxos: HashMap<TransactionInput, TransactionOutput>,
    /// Secondary index: address → set of TransactionInputs at that address.
    /// Skipped during serialization and rebuilt on load via `rebuild_address_index()`.
    #[serde(skip)]
    address_index: HashMap<Address, Vec<TransactionInput>>,
    /// When false, address index operations are skipped (for fast replay).
    /// Call `rebuild_address_index()` after re-enabling.
    #[serde(skip)]
    indexing_enabled: bool,
}

impl UtxoSet {
    pub fn new() -> Self {
        UtxoSet {
            utxos: HashMap::new(),
            address_index: HashMap::new(),
            indexing_enabled: true,
        }
    }

    /// Enable or disable address index maintenance.
    /// When disabled, `insert()` and `remove()` skip address index updates.
    /// Call `rebuild_address_index()` after re-enabling.
    pub fn set_indexing_enabled(&mut self, enabled: bool) {
        self.indexing_enabled = enabled;
    }

    /// Rebuild the address index from the UTxO map.
    /// Must be called after deserialization since the index is not serialized.
    pub fn rebuild_address_index(&mut self) {
        self.address_index.clear();
        for (input, output) in &self.utxos {
            self.address_index
                .entry(output.address.clone())
                .or_default()
                .push(input.clone());
        }
    }

    pub fn len(&self) -> usize {
        self.utxos.len()
    }

    pub fn is_empty(&self) -> bool {
        self.utxos.is_empty()
    }

    /// Look up a UTxO by input reference
    pub fn lookup(&self, input: &TransactionInput) -> Option<&TransactionOutput> {
        self.utxos.get(input)
    }

    /// Insert a new UTxO
    pub fn insert(&mut self, input: TransactionInput, output: TransactionOutput) {
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
        self.utxos.contains_key(input)
    }

    /// Apply a transaction: consume inputs, produce outputs
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

    /// Rollback a transaction: restore inputs, remove outputs
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

    /// Calculate total ADA in the UTxO set
    pub fn total_lovelace(&self) -> Lovelace {
        self.utxos.values().fold(Lovelace(0), |acc, output| {
            Lovelace(acc.0 + output.value.coin.0)
        })
    }

    /// Get all UTxOs at a specific address (O(1) lookup via secondary index)
    pub fn utxos_at_address(
        &self,
        address: &Address,
    ) -> Vec<(&TransactionInput, &TransactionOutput)> {
        match self.address_index.get(address) {
            Some(inputs) => inputs
                .iter()
                .filter_map(|input| self.utxos.get_key_value(input))
                .collect(),
            None => Vec::new(),
        }
    }

    /// Iterator over all UTxOs
    pub fn iter(&self) -> impl Iterator<Item = (&TransactionInput, &TransactionOutput)> {
        self.utxos.iter()
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
    use torsten_primitives::address::Address;
    use torsten_primitives::hash::Hash32;
    use torsten_primitives::transaction::OutputDatum;
    use torsten_primitives::value::Value;

    fn make_output(lovelace: u64) -> TransactionOutput {
        TransactionOutput {
            address: Address::Byron(torsten_primitives::address::ByronAddress {
                payload: vec![0u8; 32],
            }),
            value: Value::lovelace(lovelace),
            datum: OutputDatum::None,
            script_ref: None,
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

        let addr_a = Address::Byron(torsten_primitives::address::ByronAddress {
            payload: vec![1u8; 32],
        });
        let addr_b = Address::Byron(torsten_primitives::address::ByronAddress {
            payload: vec![2u8; 32],
        });

        let make_output_with_addr = |lovelace: u64, addr: &Address| TransactionOutput {
            address: addr.clone(),
            value: Value::lovelace(lovelace),
            datum: OutputDatum::None,
            script_ref: None,
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
        let addr = Address::Byron(torsten_primitives::address::ByronAddress {
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
}
