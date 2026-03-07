use crate::utxo::UtxoSet;
use torsten_primitives::block::{Block, Point, Tip};
use torsten_primitives::era::Era;
use torsten_primitives::hash::Hash32;
use torsten_primitives::protocol_params::ProtocolParameters;
use torsten_primitives::time::{BlockNo, EpochNo, SlotNo};
use torsten_primitives::value::Lovelace;
use std::collections::BTreeMap;

/// The complete ledger state
#[derive(Debug, Clone)]
pub struct LedgerState {
    /// Current UTxO set
    pub utxo_set: UtxoSet,
    /// Current tip of the chain
    pub tip: Tip,
    /// Current era
    pub era: Era,
    /// Current epoch
    pub epoch: EpochNo,
    /// Current protocol parameters
    pub protocol_params: ProtocolParameters,
    /// Stake distribution
    pub stake_distribution: StakeDistributionState,
    /// Treasury balance
    pub treasury: Lovelace,
    /// Reserves balance
    pub reserves: Lovelace,
    /// Delegation state
    pub delegations: BTreeMap<Hash32, Hash32>,
    /// Pool registrations
    pub pool_params: BTreeMap<Hash32, PoolRegistration>,
}

#[derive(Debug, Clone, Default)]
pub struct StakeDistributionState {
    pub stake_map: BTreeMap<Hash32, Lovelace>,
}

#[derive(Debug, Clone)]
pub struct PoolRegistration {
    pub pool_id: Hash32,
    pub vrf_keyhash: Hash32,
    pub pledge: Lovelace,
    pub cost: Lovelace,
    pub margin_numerator: u64,
    pub margin_denominator: u64,
}

impl LedgerState {
    pub fn new(params: ProtocolParameters) -> Self {
        LedgerState {
            utxo_set: UtxoSet::new(),
            tip: Tip::origin(),
            era: Era::Conway,
            epoch: EpochNo(0),
            protocol_params: params,
            stake_distribution: StakeDistributionState::default(),
            treasury: Lovelace(0),
            reserves: Lovelace(0),
            delegations: BTreeMap::new(),
            pool_params: BTreeMap::new(),
        }
    }

    /// Apply a block to the ledger state
    pub fn apply_block(&mut self, block: &Block) -> Result<(), LedgerError> {
        // Verify block connects to current tip
        if self.tip.point != Point::Origin {
            if let Some(tip_hash) = self.tip.point.hash() {
                if block.prev_hash() != tip_hash {
                    return Err(LedgerError::BlockDoesNotConnect {
                        expected: tip_hash.to_hex(),
                        got: block.prev_hash().to_hex(),
                    });
                }
            }
        }

        // Apply each transaction
        for tx in &block.transactions {
            let tx_hash = torsten_primitives::hash::blake2b_256(&[]); // placeholder
            self.utxo_set
                .apply_transaction(&tx_hash, &tx.body.inputs, &tx.body.outputs)
                .map_err(|e| LedgerError::UtxoError(e.to_string()))?;
        }

        // Update tip
        self.tip = block.tip();
        self.era = block.era;

        Ok(())
    }

    pub fn current_slot(&self) -> Option<SlotNo> {
        self.tip.point.slot()
    }

    pub fn current_block_number(&self) -> BlockNo {
        self.tip.block_number
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LedgerError {
    #[error("Block does not connect to tip: expected {expected}, got {got}")]
    BlockDoesNotConnect { expected: String, got: String },
    #[error("UTxO error: {0}")]
    UtxoError(String),
    #[error("Invalid transaction: {0}")]
    InvalidTransaction(String),
    #[error("Epoch transition error: {0}")]
    EpochTransition(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_ledger_state() {
        let params = ProtocolParameters::mainnet_defaults();
        let state = LedgerState::new(params);
        assert_eq!(state.tip, Tip::origin());
        assert!(state.utxo_set.is_empty());
        assert_eq!(state.epoch, EpochNo(0));
    }
}
