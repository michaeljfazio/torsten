//! Era-specific ledger transition logic.
//!
//! Each Cardano era introduces new ledger rules while maintaining
//! backward compatibility with previous eras. The `EraRules` trait
//! encapsulates all era-varying behavior, dispatched via `EraRulesImpl`.

pub mod byron;
pub mod common;
pub mod conway;
pub mod shelley;

// These will be added in later tasks:
// pub mod alonzo;
// pub mod babbage;

use std::collections::{HashMap, HashSet};

use dugite_primitives::block::BlockHeader;
use dugite_primitives::era::Era;
use dugite_primitives::hash::{Hash28, Hash32};
use dugite_primitives::network::NetworkId;
use dugite_primitives::time::EpochNo;
use dugite_primitives::transaction::Transaction;

use crate::plutus::SlotConfig;
use crate::state::substates::*;
use crate::state::{BlockValidationMode, LedgerError};
use crate::utxo_diff::UtxoDiff;
use dugite_primitives::block::Block;
use dugite_primitives::protocol_params::ProtocolParameters;

/// Read-only context available to all era rules.
/// Assembled by the orchestrator before dispatching.
#[allow(dead_code)]
pub struct RuleContext<'a> {
    pub params: &'a ProtocolParameters,
    pub current_slot: u64,
    pub current_epoch: EpochNo,
    pub era: Era,
    pub slot_config: Option<&'a SlotConfig>,
    pub node_network: Option<NetworkId>,
    pub genesis_delegates: &'a HashMap<Hash28, (Hash28, Hash32)>,
    pub update_quorum: u64,
    pub epoch_length: u64,
    pub shelley_transition_epoch: u64,
    pub byron_epoch_length: u64,
    pub stability_window: u64,
    pub randomness_stabilisation_window: u64,
}

/// Era-specific ledger rules.
///
/// Stateless strategy trait — implementations carry no mutable state.
/// All state lives in the component sub-states passed as parameters.
///
/// Methods accept multiple sub-state borrows by design — each sub-state
/// is an independently borrowable component, avoiding a single `&mut LedgerState`.
#[allow(dead_code, clippy::too_many_arguments)]
pub trait EraRules {
    /// Validate block body constraints (ExUnit budgets, ref script sizes).
    fn validate_block_body(
        &self,
        block: &Block,
        ctx: &RuleContext,
        utxo: &UtxoSubState,
    ) -> Result<(), LedgerError>;

    /// Apply a single valid transaction (IsValid=true path).
    ///
    /// Implements the full LEDGER rule pipeline for the era.
    fn apply_valid_tx(
        &self,
        tx: &Transaction,
        mode: BlockValidationMode,
        ctx: &RuleContext,
        utxo: &mut UtxoSubState,
        certs: &mut CertSubState,
        gov: &mut GovSubState,
        epochs: &mut EpochSubState,
    ) -> Result<UtxoDiff, LedgerError>;

    /// Apply an invalid transaction (IsValid=false, collateral consumption).
    fn apply_invalid_tx(
        &self,
        tx: &Transaction,
        mode: BlockValidationMode,
        ctx: &RuleContext,
        utxo: &mut UtxoSubState,
    ) -> Result<UtxoDiff, LedgerError>;

    /// Process an epoch boundary transition.
    fn process_epoch_transition(
        &self,
        new_epoch: EpochNo,
        ctx: &RuleContext,
        utxo: &mut UtxoSubState,
        certs: &mut CertSubState,
        gov: &mut GovSubState,
        epochs: &mut EpochSubState,
        consensus: &mut ConsensusSubState,
    ) -> Result<(), LedgerError>;

    /// Evolve nonce state after a block header.
    fn evolve_nonce(
        &self,
        header: &BlockHeader,
        ctx: &RuleContext,
        consensus: &mut ConsensusSubState,
    );

    /// Minimum fee for a transaction.
    fn min_fee(&self, tx: &Transaction, ctx: &RuleContext, utxo: &UtxoSubState) -> u64;

    /// Handle hard fork state transformations when entering this era.
    fn on_era_transition(
        &self,
        from_era: Era,
        ctx: &RuleContext,
        utxo: &mut UtxoSubState,
        certs: &mut CertSubState,
        gov: &mut GovSubState,
        consensus: &mut ConsensusSubState,
        epochs: &mut EpochSubState,
    ) -> Result<(), LedgerError>;

    /// Compute the set of required VKey witnesses for a transaction.
    fn required_witnesses(
        &self,
        tx: &Transaction,
        ctx: &RuleContext,
        utxo: &UtxoSubState,
        certs: &CertSubState,
        gov: &GovSubState,
    ) -> HashSet<Hash28>;
}
