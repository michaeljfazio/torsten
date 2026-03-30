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
pub mod n2c_client;

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
    ///
    /// Uses strict `>` comparison: only returns blocks with `slot > after_slot`.
    fn get_next_block_after_slot(&self, after_slot: u64) -> Option<(u64, [u8; 32], Vec<u8>)>;

    /// Get the first block at or after a given slot. Returns `(slot, hash, cbor)`.
    ///
    /// Uses `>=` comparison, so `get_block_at_or_after_slot(0)` includes blocks
    /// at slot 0 (e.g. Byron genesis EBB).  Used by ChainSync when the cursor is
    /// at Origin and we must serve the very first block on the chain.
    fn get_block_at_or_after_slot(&self, slot: u64) -> Option<(u64, [u8; 32], Vec<u8>)> {
        // Default: fall back to strict-after lookup.  This is correct when
        // slot > 0, but misses slot-0 blocks.  Implementations should override.
        if slot == 0 {
            self.get_next_block_after_slot(0)
        } else {
            self.get_next_block_after_slot(slot.saturating_sub(1))
        }
    }

    /// Collect multiple blocks in a contiguous slot range [`from_slot`, `to_slot`].
    ///
    /// Returns up to `limit` blocks as `(slot, hash, cbor)` tuples in ascending
    /// slot order.  Implementations SHOULD acquire the storage lock **once** for
    /// the entire batch rather than once per block — this is the primary purpose
    /// of this method.
    ///
    /// The default implementation delegates to [`get_block_at_or_after_slot`] and
    /// [`get_next_block_after_slot`] in a loop.  Concrete implementations backed
    /// by a real storage layer MUST override this with a single lock acquisition.
    fn get_blocks_in_range(
        &self,
        from_slot: u64,
        to_slot: u64,
        limit: usize,
    ) -> Vec<(u64, [u8; 32], Vec<u8>)> {
        let mut blocks = Vec::new();
        let mut current_slot = from_slot;
        let mut first = true;
        while current_slot <= to_slot && blocks.len() < limit {
            let next = if first {
                first = false;
                self.get_block_at_or_after_slot(current_slot)
            } else {
                self.get_next_block_after_slot(current_slot)
            };
            match next {
                Some((slot, hash, cbor)) if slot <= to_slot => {
                    current_slot = slot;
                    blocks.push((slot, hash, cbor));
                }
                _ => break,
            }
        }
        blocks
    }
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
/// Each variant maps to a specific failure reason from the ledger validation
/// pipeline, encoded in protocol responses to inform the submitting peer.
/// This enum mirrors the full set of `torsten_ledger::validation::ValidationError`
/// variants to enable lossless error propagation from ledger → network → client.
#[derive(Debug, Clone)]
pub enum TxValidationError {
    DecodeFailed {
        reason: String,
    },
    LedgerStateUnavailable,
    NoInputs,
    InputNotFound {
        input: String,
    },
    ValueNotConserved {
        inputs: u64,
        outputs: u64,
        fee: u64,
    },
    FeeTooSmall {
        minimum: u64,
        actual: u64,
    },
    OutputTooSmall {
        minimum: u64,
        actual: u64,
    },
    TxTooLarge {
        maximum: u64,
        actual: u64,
    },
    MissingRequiredSigner {
        signer: String,
    },
    MissingWitness {
        input: String,
    },
    TtlExpired {
        current_slot: u64,
        ttl: u64,
    },
    NotYetValid {
        current_slot: u64,
        valid_from: u64,
    },
    ScriptFailed {
        reason: String,
    },
    InsufficientCollateral,
    TooManyCollateralInputs {
        max: u64,
        actual: u64,
    },
    CollateralNotFound {
        input: String,
    },
    CollateralHasTokens {
        input: String,
    },
    CollateralMismatch {
        declared: u64,
        computed: u64,
    },
    ReferenceInputNotFound {
        input: String,
    },
    ReferenceInputOverlapsInput {
        input: String,
    },
    MultiAssetNotConserved {
        policy: String,
        input_side: i128,
        output_side: i128,
    },
    InvalidMint,
    ExUnitsExceeded,
    ScriptDataHashMismatch {
        expected: String,
        actual: String,
    },
    UnexpectedScriptDataHash,
    MissingScriptDataHash,
    DuplicateInput {
        input: String,
    },
    NativeScriptFailed,
    InvalidWitnessSignature {
        vkey: String,
    },
    NetworkMismatch {
        expected: String,
        actual: String,
    },
    AuxiliaryDataHashWithoutData,
    AuxiliaryDataWithoutHash,
    BlockExUnitsExceeded {
        resource: String,
        limit: u64,
        total: u64,
    },
    OutputValueTooLarge {
        maximum: u64,
        actual: u64,
    },
    MissingRawCbor,
    MissingSlotConfig,
    MissingSpendRedeemer {
        index: u32,
    },
    RedeemerIndexOutOfRange {
        tag: String,
        index: u32,
        max: u32,
    },
    MissingInputWitness {
        credential: String,
    },
    MissingScriptWitness {
        credential: String,
    },
    MissingWithdrawalWitness {
        credential: String,
    },
    MissingWithdrawalScriptWitness {
        credential: String,
    },
    MissingCertificateWitness {
        credential: String,
    },
    ValueOverflow,
    /// Multiple validation errors collected.
    Multiple(Vec<TxValidationError>),
    /// Catch-all for other validation failures.
    Other(String),
}

impl std::fmt::Display for TxValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for TxValidationError {}

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

    /// Return the entire UTxO set (GetUTxOWhole).
    /// Default implementation returns empty — override if the store supports it.
    fn utxos_all(&self) -> Vec<UtxoSnapshot> {
        vec![]
    }
}

/// A single asset within a multi-asset value: `(asset_name, quantity)`.
pub type AssetEntry = (Vec<u8>, u64);

/// A policy group within a multi-asset value: `(policy_id, assets)`.
pub type PolicyAssets = (Vec<u8>, Vec<AssetEntry>);

/// Multi-asset snapshot: `[(policy_id, [(asset_name, quantity)])]`.
pub type MultiAssetSnapshot = Vec<PolicyAssets>;

/// UTxO snapshot for query responses, containing all fields needed for CBOR encoding.
///
/// Field names match the old API to minimize node integration churn.
#[derive(Debug, Clone)]
pub struct UtxoSnapshot {
    /// Transaction hash (raw bytes, typically 32 bytes).
    pub tx_hash: Vec<u8>,
    /// Output index within the transaction.
    pub output_index: u32,
    /// Address bytes (raw Cardano address encoding).
    pub address_bytes: Vec<u8>,
    /// Lovelace value at this output.
    pub lovelace: u64,
    /// Multi-asset values: `[(policy_id, [(asset_name, quantity)])]`.
    pub multi_asset: MultiAssetSnapshot,
    /// Optional datum hash (32 bytes).
    pub datum_hash: Option<Vec<u8>>,
    /// Optional raw CBOR of the entire output (for script reference, inline datum, etc.).
    pub raw_cbor: Option<Vec<u8>>,
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

// ─── Convenience re-exports ───
// Key types re-exported at crate root for ergonomic imports.

pub use protocol::blockfetch::client::BlockFetchClient;
pub use protocol::blockfetch::decision::BlockFetchDecision;
pub use protocol::chainsync::client::{ChainSyncEvent, PipelinedChainSyncClient};
pub use protocol::chainsync::server::BlockAnnouncement;
pub use protocol::keepalive::client::KeepAliveClient;
pub use protocol::keepalive::server::KeepAliveServer;
pub use protocol::local_state_query::server::QueryHandler;
pub use protocol::peersharing::client::PeerSharingClient;
pub use protocol::txsubmission::client::{TxSource, TxSubmissionClient};
pub use protocol::txsubmission::server::TxSubmissionServer;
pub use protocol::txsubmission::TxIdAndSize;

pub use peer::manager::{PeerInfo, PeerManager, PeerSource, PeerState};
pub use peer::{Governor, GovernorConfig, PeerTargets};

pub use connection::manager::ConnectionManagerConfig;
pub use connection::{ConnectionHandler, ConnectionManager, ConnectionState};

pub use mux::channel::MuxChannel;
pub use mux::{Direction, Mux};

pub use bearer::tcp::TcpBearer;
pub use bearer::unix::UnixBearer;

pub use handshake::n2c::N2CVersionData;
pub use handshake::n2n::N2NVersionData;
pub use n2c_client::{N2CClient, TipResult};
