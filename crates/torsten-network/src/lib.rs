//! Ouroboros network protocol implementation for the Torsten Cardano node.
//!
//! Four-layer architecture:
//! - Layer 1: Bearer (TCP, Unix socket transport)
//! - Layer 2: Multiplexer (SDU framing, fairness, demux)
//! - Layer 3: Mini-protocols (ChainSync, BlockFetch, TxSubmission2, etc.)
//! - Layer 4: Connection Manager (lifecycle, peer management)

pub mod codec;
pub mod error;

pub mod bearer;

pub mod mux;

pub mod handshake;

pub mod protocol;

pub mod peer;

pub mod connection;

pub mod metrics;

pub use error::*;

// Re-export MempoolProvider from primitives (used by TxSubmission2, LocalTxSubmission, LocalTxMonitor).
pub use torsten_primitives::mempool::MempoolProvider;

// ─── Public Traits ───
// These are the integration boundary with torsten-node.
// The node crate implements these traits and passes them to the network layer.

/// Provides block data from ChainDB for N2N server protocols.
///
/// The node crate implements this trait over its ChainDB instance so that
/// the network layer can serve blocks to peers without depending on storage internals.
pub trait BlockProvider: Send + Sync + 'static {
    /// Get raw block CBOR by its 32-byte header hash.
    fn get_block(&self, hash: &[u8; 32]) -> Option<Vec<u8>>;

    /// Check if a block with the given header hash exists in the chain database.
    fn has_block(&self, hash: &[u8; 32]) -> bool;

    /// Get current chain tip information (slot, hash, block number).
    fn get_tip(&self) -> TipInfo;

    /// Get the next block after a given slot. Returns `(slot, hash, cbor)` if found.
    fn get_next_block_after_slot(&self, after_slot: u64) -> Option<(u64, [u8; 32], Vec<u8>)>;
}

/// Chain tip information returned by [`BlockProvider::get_tip`].
#[derive(Debug, Clone)]
pub struct TipInfo {
    /// Slot number of the tip block.
    pub slot: u64,
    /// 32-byte header hash of the tip block.
    pub hash: [u8; 32],
    /// Block number (height) of the tip block.
    pub block_number: u64,
}

/// Validates transactions before mempool admission.
///
/// The node crate implements this over its ledger state to perform Phase-1 and Phase-2
/// validation. Called by N2C LocalTxSubmission and N2N TxSubmission2 protocols.
pub trait TxValidator: Send + Sync + 'static {
    /// Validate a transaction given its era identifier and raw CBOR bytes.
    fn validate_tx(&self, era_id: u16, tx_bytes: &[u8]) -> Result<(), TxValidationError>;
}

/// Transaction validation errors returned to N2C/N2N clients.
///
/// Each variant maps to a specific failure reason that can be encoded
/// in the protocol response to inform the submitting peer.
#[derive(Debug, Clone)]
pub enum TxValidationError {
    /// Transaction CBOR could not be deserialized.
    DeserializationFailed {
        /// Human-readable description of the deserialization failure.
        reason: String,
    },
    /// A referenced UTxO input was not found in the current ledger state.
    InputNotFound {
        /// Hex-encoded hash of the transaction containing the missing input.
        tx_hash: String,
        /// Output index within that transaction.
        index: u32,
    },
    /// Transaction inputs do not cover the required outputs + fees.
    InsufficientFunds {
        /// Total lovelace required (outputs + fee).
        required: u64,
        /// Total lovelace available from inputs.
        available: u64,
    },
    /// Transaction fee is below the minimum required by protocol parameters.
    FeeTooSmall {
        /// Minimum fee required.
        minimum: u64,
        /// Fee actually specified in the transaction.
        actual: u64,
    },
    /// A Plutus script evaluation failed.
    ScriptFailed {
        /// Human-readable description of the script failure.
        reason: String,
    },
    /// Transaction is tagged with an unsupported era identifier.
    InvalidEra(u16),
    /// Mempool is full; cannot accept more transactions.
    MempoolFull,
    /// Ledger state is not available (e.g. during initial sync).
    LedgerStateUnavailable,
    /// Catch-all for other validation failures.
    Other(String),
}

/// Provides UTxO lookups for LocalStateQuery protocol responses.
///
/// The node crate implements this over its UTxO store so that the network layer
/// can answer UTxO queries from N2C clients without depending on ledger internals.
pub trait UtxoQueryProvider: Send + Sync {
    /// Look up all UTxOs at a given address (raw address bytes).
    fn utxos_at_address_bytes(&self, addr_bytes: &[u8]) -> Vec<UtxoSnapshot>;

    /// Look up UTxOs by specific transaction inputs (tx_hash, output_index).
    /// Default implementation returns empty — override if the store supports it.
    fn utxos_by_tx_inputs(&self, _inputs: &[(Vec<u8>, u32)]) -> Vec<UtxoSnapshot> {
        vec![]
    }
}

/// A single asset within a multi-asset value: `(asset_name, quantity)`.
pub type AssetEntry = (Vec<u8>, u64);

/// A policy group within a multi-asset value: `(policy_id, assets)`.
pub type PolicyAssets = (Vec<u8>, Vec<AssetEntry>);

/// UTxO snapshot for query responses, containing all fields needed for CBOR encoding.
#[derive(Debug, Clone)]
pub struct UtxoSnapshot {
    /// Transaction hash (raw bytes, typically 32 bytes).
    pub tx_hash: Vec<u8>,
    /// Output index within the transaction.
    pub output_index: u32,
    /// Address bytes (raw Cardano address encoding).
    pub address: Vec<u8>,
    /// Lovelace value at this output.
    pub value: u64,
    /// Optional datum hash or inline datum (CBOR-encoded).
    pub datum: Option<Vec<u8>>,
    /// Optional reference script (CBOR-encoded).
    pub script_ref: Option<Vec<u8>>,
    /// Multi-asset values: `[(policy_id, [(asset_name, quantity)])]`.
    pub multi_assets: Vec<PolicyAssets>,
}

/// Metrics bridge for connection events.
///
/// Implemented by the node layer to bridge protocol-level events to the
/// Prometheus metrics system (e.g. `peers_connected` gauge).
pub trait ConnectionMetrics: Send + Sync + 'static {
    /// Called when a new peer connection is established.
    fn on_connect(&self);
    /// Called when a peer connection is closed.
    fn on_disconnect(&self);
    /// Called when a connection-level error occurs.
    fn on_error(&self, label: &str);
}
