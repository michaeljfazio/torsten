use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use torsten_primitives::hash::TransactionHash;
use torsten_primitives::transaction::{TransactionInput, TransactionOutput};
use torsten_primitives::value::Lovelace;

/// The UTxO set: maps transaction inputs to their unspent outputs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UtxoSet {
    utxos: BTreeMap<TransactionInput, TransactionOutput>,
}

impl UtxoSet {
    pub fn new() -> Self {
        UtxoSet {
            utxos: BTreeMap::new(),
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
        self.utxos.insert(input, output);
    }

    /// Remove a UTxO (mark as spent)
    pub fn remove(&mut self, input: &TransactionInput) -> Option<TransactionOutput> {
        self.utxos.remove(input)
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

    /// Get all UTxOs at a specific address
    pub fn utxos_at_address(
        &self,
        address: &torsten_primitives::address::Address,
    ) -> Vec<(&TransactionInput, &TransactionOutput)> {
        self.utxos
            .iter()
            .filter(|(_, output)| &output.address == address)
            .collect()
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

/// Concurrent UTxO set for use in the node (thread-safe)
#[derive(Debug)]
pub struct ConcurrentUtxoSet {
    utxos: DashMap<TransactionInput, TransactionOutput>,
}

impl ConcurrentUtxoSet {
    pub fn new() -> Self {
        ConcurrentUtxoSet {
            utxos: DashMap::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.utxos.len()
    }

    pub fn is_empty(&self) -> bool {
        self.utxos.is_empty()
    }

    pub fn lookup(&self, input: &TransactionInput) -> Option<TransactionOutput> {
        self.utxos.get(input).map(|r| r.value().clone())
    }

    pub fn insert(&self, input: TransactionInput, output: TransactionOutput) {
        self.utxos.insert(input, output);
    }

    pub fn remove(&self, input: &TransactionInput) -> Option<TransactionOutput> {
        self.utxos.remove(input).map(|(_, v)| v)
    }

    pub fn contains(&self, input: &TransactionInput) -> bool {
        self.utxos.contains_key(input)
    }
}

impl Default for ConcurrentUtxoSet {
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
    fn test_concurrent_utxo_set() {
        let utxo_set = ConcurrentUtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::ZERO,
            index: 0,
        };
        utxo_set.insert(input.clone(), make_output(1_000_000));

        assert_eq!(utxo_set.len(), 1);
        assert!(utxo_set.contains(&input));
        assert_eq!(utxo_set.lookup(&input).unwrap().value.coin.0, 1_000_000);

        utxo_set.remove(&input);
        assert!(utxo_set.is_empty());
    }
}
