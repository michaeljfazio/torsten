mod chain_sync;
mod state_query;
mod tx_monitor;
mod tx_submission;

use std::path::Path;
use std::sync::Arc;
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;
use tokio::sync::RwLock;
use torsten_mempool::Mempool;
use tracing::{debug, error, info, warn};

use crate::multiplexer::Segment;
use crate::n2n_server::BlockProvider;
use crate::query_handler::QueryHandler;

use chain_sync::{handle_local_chainsync, ChainSyncCursor};
use state_query::handle_state_query;
use tx_monitor::{handle_tx_monitor, TxMonitorCursor};
use tx_submission::handle_tx_submission;

/// Typed error returned by [`TxValidator::validate_tx`].
///
/// Each variant represents a distinct failure mode so that callers can
/// pattern-match and respond appropriately (e.g. return a specific reject
/// reason over the wire, apply different back-off policies, etc.).
#[derive(Debug, Error)]
pub enum TxValidationError {
    #[error("Failed to decode transaction: {reason}")]
    DecodeFailed { reason: String },
    #[error("Ledger state unavailable")]
    LedgerStateUnavailable,
    #[error("No inputs in transaction")]
    NoInputs,
    #[error("Input not found in UTxO set: {input}")]
    InputNotFound { input: String },
    #[error("Value not conserved: inputs={inputs}, outputs={outputs}, fee={fee}")]
    ValueNotConserved { inputs: u64, outputs: u64, fee: u64 },
    #[error("Fee too small: minimum={minimum}, actual={actual}")]
    FeeTooSmall { minimum: u64, actual: u64 },
    #[error("Output too small: minimum={minimum}, actual={actual}")]
    OutputTooSmall { minimum: u64, actual: u64 },
    #[error("Transaction too large: maximum={maximum}, actual={actual}")]
    TxTooLarge { maximum: u64, actual: u64 },
    #[error("Missing required signer: {signer}")]
    MissingRequiredSigner { signer: String },
    #[error("Missing witness for input: {input}")]
    MissingWitness { input: String },
    #[error("TTL expired: current_slot={current_slot}, ttl={ttl}")]
    TtlExpired { current_slot: u64, ttl: u64 },
    #[error("Transaction not yet valid: current_slot={current_slot}, valid_from={valid_from}")]
    NotYetValid { current_slot: u64, valid_from: u64 },
    #[error("Script validation failed: {reason}")]
    ScriptFailed { reason: String },
    #[error("Insufficient collateral")]
    InsufficientCollateral,
    #[error("Too many collateral inputs: max={max}, actual={actual}")]
    TooManyCollateralInputs { max: u64, actual: u64 },
    #[error("Collateral input not found in UTxO set: {input}")]
    CollateralNotFound { input: String },
    #[error("Collateral input contains tokens (must be pure ADA): {input}")]
    CollateralHasTokens { input: String },
    #[error("Collateral mismatch: total_collateral={declared}, effective={computed}")]
    CollateralMismatch { declared: u64, computed: u64 },
    #[error("Reference input not found in UTxO set: {input}")]
    ReferenceInputNotFound { input: String },
    #[error("Reference input overlaps with regular input: {input}")]
    ReferenceInputOverlapsInput { input: String },
    #[error("Multi-asset not conserved for policy {policy}: inputs+mint={input_side}, outputs={output_side}")]
    MultiAssetNotConserved {
        policy: String,
        input_side: i128,
        output_side: i128,
    },
    #[error("Negative minting without policy script")]
    InvalidMint,
    #[error("Max execution units exceeded")]
    ExUnitsExceeded,
    #[error("Script data hash mismatch: expected {expected}, got {actual}")]
    ScriptDataHashMismatch { expected: String, actual: String },
    #[error("Script data hash present but no scripts or redeemers")]
    UnexpectedScriptDataHash,
    #[error("Missing script data hash (required when scripts/redeemers present)")]
    MissingScriptDataHash,
    #[error("Duplicate input in transaction: {input}")]
    DuplicateInput { input: String },
    #[error("Native script validation failed")]
    NativeScriptFailed,
    #[error("Witness signature verification failed for vkey: {vkey}")]
    InvalidWitnessSignature { vkey: String },
    #[error("Output address network mismatch: expected {expected}, got {actual}")]
    NetworkMismatch { expected: String, actual: String },
    #[error("Auxiliary data hash declared but no auxiliary data present")]
    AuxiliaryDataHashWithoutData,
    #[error("Auxiliary data present but no auxiliary data hash in tx body")]
    AuxiliaryDataWithoutHash,
    #[error("Block execution units exceeded: {resource} limit={limit}, total={total}")]
    BlockExUnitsExceeded {
        resource: String,
        limit: u64,
        total: u64,
    },
    #[error("Output value too large: maximum={maximum}, actual={actual}")]
    OutputValueTooLarge { maximum: u64, actual: u64 },
    #[error("Plutus transaction missing raw CBOR for script evaluation")]
    MissingRawCbor,
    #[error("Plutus transaction missing slot configuration for script evaluation")]
    MissingSlotConfig,
    #[error("Script-locked input at index {index} has no matching Spend redeemer")]
    MissingSpendRedeemer { index: u32 },
    #[error("Redeemer index out of range: tag={tag}, index={index}, max={max}")]
    RedeemerIndexOutOfRange { tag: String, index: u32, max: usize },
    #[error("Missing VKey witness for input credential: {credential}")]
    MissingInputWitness { credential: String },
    #[error("Missing script witness for script-locked input: {credential}")]
    MissingScriptWitness { credential: String },
    #[error("Missing VKey witness for withdrawal credential: {credential}")]
    MissingWithdrawalWitness { credential: String },
    #[error("Missing script witness for script-locked withdrawal: {credential}")]
    MissingWithdrawalScriptWitness { credential: String },
    #[error("{}", format_multiple_errors(.0))]
    Multiple(Vec<TxValidationError>),
}

fn format_multiple_errors(errors: &[TxValidationError]) -> String {
    errors
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("; ")
}

impl TxValidationError {
    pub fn is_decode_error(&self) -> bool {
        matches!(self, TxValidationError::DecodeFailed { .. })
    }

    pub fn is_availability_error(&self) -> bool {
        matches!(self, TxValidationError::LedgerStateUnavailable)
    }

    pub fn errors(&self) -> Vec<&TxValidationError> {
        match self {
            TxValidationError::Multiple(errors) => errors.iter().collect(),
            other => vec![other],
        }
    }
}

/// Trait for validating transactions before mempool admission.
/// Implementors should perform full Phase-1 and Phase-2 (Plutus) validation.
pub trait TxValidator: Send + Sync + 'static {
    /// Validate a transaction.
    ///
    /// Returns `Ok(())` if the transaction passes all checks, or a typed
    /// [`TxValidationError`] describing the failure.
    fn validate_tx(&self, era_id: u16, tx_bytes: &[u8]) -> Result<(), TxValidationError>;
}

#[derive(Error, Debug)]
pub enum N2CServerError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Handshake failed: {0}")]
    HandshakeFailed(String),
    #[error("Protocol error: {0}")]
    Protocol(String),
}

/// N2C mini-protocol IDs
const MINI_PROTOCOL_HANDSHAKE: u16 = 0;
const MINI_PROTOCOL_CHAINSYNC: u16 = 5;
const MINI_PROTOCOL_TX_SUBMISSION: u16 = 6;
const MINI_PROTOCOL_STATE_QUERY: u16 = 7;
const MINI_PROTOCOL_TX_MONITOR: u16 = 9;

/// Node-to-Client server that listens on a Unix domain socket.
pub struct N2CServer {
    query_handler: Arc<RwLock<QueryHandler>>,
    mempool: Arc<Mempool>,
    tx_validator: Option<Arc<dyn TxValidator>>,
    block_provider: Option<Arc<dyn BlockProvider>>,
}

impl N2CServer {
    pub fn new(query_handler: Arc<RwLock<QueryHandler>>, mempool: Arc<Mempool>) -> Self {
        N2CServer {
            query_handler,
            mempool,
            tx_validator: None,
            block_provider: None,
        }
    }

    /// Set a transaction validator for Phase-1/Phase-2 validation before mempool admission
    pub fn set_tx_validator(&mut self, validator: Arc<dyn TxValidator>) {
        self.tx_validator = Some(validator);
    }

    /// Set a block provider for LocalChainSync block delivery
    pub fn set_block_provider(&mut self, provider: Arc<dyn BlockProvider>) {
        self.block_provider = Some(provider);
    }

    /// Start listening on the given Unix socket path.
    /// Accepts connections until the shutdown signal is received.
    pub async fn listen(
        &self,
        socket_path: &Path,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> Result<(), N2CServerError> {
        // Remove existing socket file if present
        if socket_path.exists() {
            std::fs::remove_file(socket_path)?;
        }

        let listener = UnixListener::bind(socket_path)?;
        info!("N2C server listening on {}", socket_path.display());

        loop {
            tokio::select! {
                result = listener.accept() => {
                    match result {
                        Ok((stream, _addr)) => {
                            info!("N2C client connected");
                            let handler = self.query_handler.clone();
                            let mempool = self.mempool.clone();
                            let validator = self.tx_validator.clone();
                            let block_provider = self.block_provider.clone();
                            tokio::spawn(async move {
                                if let Err(e) = handle_n2c_connection(
                                    stream,
                                    handler,
                                    mempool,
                                    validator,
                                    block_provider,
                                )
                                .await
                                {
                                    warn!("N2C connection error: {e}");
                                }
                                debug!("N2C client disconnected");
                            });
                        }
                        Err(e) => {
                            error!("Failed to accept N2C connection: {e}");
                        }
                    }
                }
                _ = shutdown_rx.changed() => {
                    info!("N2C server shutting down");
                    // Clean up socket file
                    let _ = std::fs::remove_file(socket_path);
                    return Ok(());
                }
            }
        }
    }
}

/// Handle a single N2C client connection
async fn handle_n2c_connection(
    mut stream: tokio::net::UnixStream,
    query_handler: Arc<RwLock<QueryHandler>>,
    mempool: Arc<Mempool>,
    tx_validator: Option<Arc<dyn TxValidator>>,
    block_provider: Option<Arc<dyn BlockProvider>>,
) -> Result<(), N2CServerError> {
    let mut read_buf = vec![0u8; 65536];
    // Persistent buffer for partial segments across reads
    let mut pending = Vec::new();
    let mut chainsync_cursor = ChainSyncCursor {
        cursor_slot: 0,
        has_intersection: false,
    };
    let mut tx_monitor_cursor = TxMonitorCursor {
        snapshot: Vec::new(),
        position: 0,
        acquired: false,
    };

    loop {
        let n = stream.read(&mut read_buf).await?;
        if n == 0 {
            return Ok(()); // Client disconnected
        }

        pending.extend_from_slice(&read_buf[..n]);

        // Parse as many complete segments as possible from the pending buffer
        let mut offset = 0;
        while offset < pending.len() {
            let remaining = &pending[offset..];
            if remaining.len() < 8 {
                break; // Need more data for a complete header
            }

            match Segment::decode(remaining) {
                Ok((segment, consumed)) => {
                    offset += consumed;

                    // Process the segment
                    let response = process_segment(
                        &segment,
                        &query_handler,
                        &mempool,
                        &tx_validator,
                        &block_provider,
                        &mut chainsync_cursor,
                        &mut tx_monitor_cursor,
                    )
                    .await?;
                    if let Some(resp_segment) = response {
                        let encoded = resp_segment.encode();
                        stream.write_all(&encoded).await?;
                    }
                }
                Err(_) => {
                    break; // Incomplete segment, wait for more data
                }
            }
        }

        // Remove consumed bytes from pending buffer
        if offset > 0 {
            pending.drain(..offset);
        }
    }
}

/// Process a single multiplexer segment and optionally return a response
async fn process_segment(
    segment: &Segment,
    query_handler: &Arc<RwLock<QueryHandler>>,
    mempool: &Arc<Mempool>,
    tx_validator: &Option<Arc<dyn TxValidator>>,
    block_provider: &Option<Arc<dyn BlockProvider>>,
    chainsync_cursor: &mut ChainSyncCursor,
    tx_monitor_cursor: &mut TxMonitorCursor,
) -> Result<Option<Segment>, N2CServerError> {
    match segment.protocol_id {
        MINI_PROTOCOL_HANDSHAKE => handle_handshake(&segment.payload),
        MINI_PROTOCOL_STATE_QUERY => handle_state_query(&segment.payload, query_handler).await,
        MINI_PROTOCOL_TX_SUBMISSION => {
            handle_tx_submission(&segment.payload, mempool, tx_validator)
        }
        MINI_PROTOCOL_TX_MONITOR => {
            handle_tx_monitor(&segment.payload, mempool, query_handler, tx_monitor_cursor).await
        }
        MINI_PROTOCOL_CHAINSYNC => {
            handle_local_chainsync(
                &segment.payload,
                query_handler,
                block_provider,
                chainsync_cursor,
            )
            .await
        }
        other => {
            debug!("Unknown N2C mini-protocol: {other}");
            Ok(None)
        }
    }
}

/// Handle N2C handshake
///
/// N2C handshake proposes versions. We accept the highest version we support.
/// The CBOR format is: [0, { version_number: params, ... }] for propose
/// We respond with: [1, version_number, params] for accept
fn handle_handshake(payload: &[u8]) -> Result<Option<Segment>, N2CServerError> {
    // Parse CBOR handshake proposal
    // The client sends [0, {version -> params}]
    // We need to find the highest version we support and accept it

    // For now, try to decode and accept a reasonable version
    // Simple handshake: accept version 16 (Conway) with network magic
    // Response: [1, version, [network_magic, false]]
    let mut response_buf = Vec::new();
    let mut encoder = minicbor::Encoder::new(&mut response_buf);

    // Try to parse the proposed versions to extract network magic
    let network_magic = match parse_handshake_magic(payload) {
        Some(magic) => magic,
        None => {
            warn!("N2C handshake: could not parse network magic from client proposal, rejecting");
            return Err(N2CServerError::HandshakeFailed(
                "could not parse network magic from handshake proposal".into(),
            ));
        }
    };
    let (version, wire_version) = parse_highest_version(payload).unwrap_or((16, 16));

    debug!(
        "N2C handshake: accepting version {version} (wire: {wire_version}), magic {network_magic}"
    );

    // Encode accept response: [1, wire_version, [magic, false]]
    // Use wire_version to preserve bit-15 encoding if client sent it
    encoder
        .array(3)
        .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
    encoder
        .u32(1)
        .map_err(|e| N2CServerError::Protocol(e.to_string()))?; // MsgAcceptVersion
    encoder
        .u32(wire_version)
        .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
    encoder
        .array(2)
        .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
    encoder
        .u64(network_magic)
        .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
    encoder
        .bool(false)
        .map_err(|e| N2CServerError::Protocol(e.to_string()))?; // query mode = false

    Ok(Some(Segment {
        transmission_time: 0,
        protocol_id: MINI_PROTOCOL_HANDSHAKE,
        is_responder: true,
        payload: response_buf,
    }))
}

/// Parse the network magic from a handshake proposal
fn parse_handshake_magic(payload: &[u8]) -> Option<u64> {
    let mut decoder = minicbor::Decoder::new(payload);
    // [0, { version: [magic, query] }]
    decoder.array().ok()?;
    decoder.u32().ok()?; // msg type = 0 (propose)
    let map_len = decoder.map().ok()?;
    if map_len == Some(0) {
        return None;
    }
    decoder.u32().ok()?; // first version number
                         // Value is either [magic, query] or just magic
    if let Ok(Some(_arr_len)) = decoder.array() {
        decoder.u64().ok()
    } else {
        None
    }
}

/// N2C version numbers have bit 15 set on the wire (Haskell convention).
/// V16 = 32784, V17 = 32785, etc. Strip bit 15 to get the logical version.
const N2C_VERSION_BIT: u32 = 1 << 15; // 0x8000

/// Maximum N2C version we support
const N2C_MAX_VERSION: u32 = 17;

/// Minimum N2C version we support
const N2C_MIN_VERSION: u32 = 16;

/// Parse the highest proposed version number.
/// Handles both raw version numbers (from torsten-cli) and
/// bit-15-encoded versions (from cardano-cli / Haskell clients).
/// Returns (logical_version, wire_version) where wire_version preserves
/// the original encoding for the accept response.
fn parse_highest_version(payload: &[u8]) -> Option<(u16, u32)> {
    let mut decoder = minicbor::Decoder::new(payload);
    decoder.array().ok()?;
    decoder.u32().ok()?; // msg type
    let map_len = decoder.map().ok()??;
    let mut best: Option<(u16, u32)> = None;
    for _ in 0..map_len {
        if let Ok(wire_v) = decoder.u32() {
            // Strip bit 15 if present to get logical version
            let logical = if wire_v & N2C_VERSION_BIT != 0 {
                wire_v & !N2C_VERSION_BIT
            } else {
                wire_v
            };
            if (N2C_MIN_VERSION..=N2C_MAX_VERSION).contains(&logical)
                && best.is_none_or(|(bv, _)| logical as u16 > bv)
            {
                best = Some((logical as u16, wire_v));
            }
        }
        // Skip the value (params)
        decoder.skip().ok()?;
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query_handler::ProtocolParamsSnapshot;
    use crate::query_handler::QueryResult;
    use state_query::encode_query_result;
    use torsten_primitives::hash::Hash32;

    #[test]
    fn test_parse_highest_version_basic() {
        // Propose versions 16 and 17 without bit 15 (torsten-cli style)
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(0).unwrap(); // MsgProposeVersions
        enc.map(2).unwrap();
        enc.u32(16).unwrap();
        enc.array(2).unwrap();
        enc.u64(764824073).unwrap();
        enc.bool(false).unwrap();
        enc.u32(17).unwrap();
        enc.array(2).unwrap();
        enc.u64(764824073).unwrap();
        enc.bool(false).unwrap();

        assert_eq!(parse_highest_version(&buf), Some((17, 17)));
    }

    #[test]
    fn test_parse_highest_version_bit15() {
        // Propose versions with bit 15 set (cardano-cli / Haskell style)
        // V16 = 32784, V17 = 32785
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(0).unwrap(); // MsgProposeVersions
        enc.map(2).unwrap();
        enc.u32(32784).unwrap(); // V16 with bit 15
        enc.array(2).unwrap();
        enc.u64(2).unwrap();
        enc.bool(false).unwrap();
        enc.u32(32785).unwrap(); // V17 with bit 15
        enc.array(2).unwrap();
        enc.u64(2).unwrap();
        enc.bool(false).unwrap();

        let result = parse_highest_version(&buf);
        assert_eq!(result, Some((17, 32785))); // logical 17, wire 32785
    }

    #[test]
    fn test_parse_highest_version_unsupported() {
        // Propose only old versions (below V16)
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(0).unwrap();
        enc.map(1).unwrap();
        enc.u32(1).unwrap();
        enc.array(2).unwrap();
        enc.u64(764824073).unwrap();
        enc.bool(false).unwrap();

        assert_eq!(parse_highest_version(&buf), None);
    }

    #[test]
    fn test_parse_handshake_magic() {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(0).unwrap();
        enc.map(1).unwrap();
        enc.u32(16).unwrap();
        enc.array(2).unwrap();
        enc.u64(1).unwrap(); // preview testnet magic
        enc.bool(false).unwrap();

        assert_eq!(parse_handshake_magic(&buf), Some(1));
    }

    #[test]
    fn test_encode_query_result_epoch() {
        let result = QueryResult::EpochNo(500);
        let cbor = encode_query_result(&result);
        assert!(!cbor.is_empty());
    }

    #[test]
    fn test_encode_query_result_chain_tip() {
        let result = QueryResult::ChainTip {
            slot: 12345,
            hash: vec![0u8; 32],
            block_no: 100,
        };
        let cbor = encode_query_result(&result);
        assert!(!cbor.is_empty());
    }

    /// Build a minimal valid Conway transaction CBOR for testing.
    ///
    /// Conway tx is a 4-element array: [body, witness_set, is_valid, auxiliary_data]
    /// Body is a map with: 0 -> inputs (array), 1 -> outputs (array), 2 -> fee (uint)
    fn build_test_tx_cbor(fee: u64) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        // Transaction: [body, witness_set, is_valid, null]
        enc.array(4).unwrap();
        // Body: {0: [], 1: [], 2: fee}
        enc.map(3).unwrap();
        enc.u32(0).unwrap();
        enc.array(0).unwrap(); // inputs (empty)
        enc.u32(1).unwrap();
        enc.array(0).unwrap(); // outputs (empty)
        enc.u32(2).unwrap();
        enc.u64(fee).unwrap(); // fee
                               // Witness set: {}
        enc.map(0).unwrap();
        // is_valid
        enc.bool(true).unwrap();
        // auxiliary_data
        enc.null().unwrap();
        buf
    }

    /// Build MsgSubmitTx payload: [0, [era_id, tx_bytes]]
    fn build_submit_payload(era_id: u32, tx_bytes: &[u8]) -> Vec<u8> {
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(2).unwrap();
        enc.u32(0).unwrap(); // MsgSubmitTx
        enc.array(2).unwrap();
        enc.u32(era_id).unwrap();
        enc.bytes(tx_bytes).unwrap();
        payload
    }

    #[test]
    fn test_handle_tx_submission_accept() {
        let mempool = Arc::new(Mempool::new(torsten_mempool::MempoolConfig::default()));
        let no_validator: Option<Arc<dyn TxValidator>> = None;

        let tx_bytes = build_test_tx_cbor(200_000);
        let payload = build_submit_payload(6, &tx_bytes); // Conway era

        let result = handle_tx_submission(&payload, &mempool, &no_validator).unwrap();
        assert!(result.is_some());

        let segment = result.unwrap();
        assert_eq!(segment.protocol_id, MINI_PROTOCOL_TX_SUBMISSION);
        assert!(segment.is_responder);

        // Verify MsgAcceptTx [1]
        let mut decoder = minicbor::Decoder::new(&segment.payload);
        let _ = decoder.array();
        let tag = decoder.u32().unwrap();
        assert_eq!(tag, 1); // MsgAcceptTx

        // Verify tx was added to mempool
        assert_eq!(mempool.len(), 1);
    }

    #[test]
    fn test_handle_tx_submission_duplicate() {
        let mempool = Arc::new(Mempool::new(torsten_mempool::MempoolConfig::default()));
        let no_validator: Option<Arc<dyn TxValidator>> = None;

        let tx_bytes = build_test_tx_cbor(200_000);
        let payload = build_submit_payload(6, &tx_bytes);

        // Submit twice - both should accept
        let _ = handle_tx_submission(&payload, &mempool, &no_validator).unwrap();
        let result = handle_tx_submission(&payload, &mempool, &no_validator).unwrap();
        assert!(result.is_some());

        let segment = result.unwrap();
        let mut decoder = minicbor::Decoder::new(&segment.payload);
        let _ = decoder.array();
        assert_eq!(decoder.u32().unwrap(), 1); // Still accepted (AlreadyExists)
        assert_eq!(mempool.len(), 1);
    }

    #[test]
    fn test_handle_tx_submission_invalid_cbor() {
        let mempool = Arc::new(Mempool::new(torsten_mempool::MempoolConfig::default()));
        let no_validator: Option<Arc<dyn TxValidator>> = None;

        let tx_bytes = vec![0xa0u8]; // not a valid transaction
        let payload = build_submit_payload(6, &tx_bytes);

        let result = handle_tx_submission(&payload, &mempool, &no_validator).unwrap();
        assert!(result.is_some());

        let segment = result.unwrap();
        let mut decoder = minicbor::Decoder::new(&segment.payload);
        let _ = decoder.array();
        assert_eq!(decoder.u32().unwrap(), 2); // MsgRejectTx
        assert_eq!(mempool.len(), 0);
    }

    #[test]
    fn test_handle_tx_submission_full_mempool() {
        let config = torsten_mempool::MempoolConfig {
            max_transactions: 1,
            max_bytes: 1024 * 1024,
        };
        let mempool = Arc::new(Mempool::new(config));
        let no_validator: Option<Arc<dyn TxValidator>> = None;

        // Fill the mempool
        let tx_bytes_1 = build_test_tx_cbor(100_000);
        let payload1 = build_submit_payload(6, &tx_bytes_1);
        let _ = handle_tx_submission(&payload1, &mempool, &no_validator).unwrap();

        // Submit a different tx - should be rejected (full)
        let tx_bytes_2 = build_test_tx_cbor(200_000);
        let payload2 = build_submit_payload(6, &tx_bytes_2);

        let result = handle_tx_submission(&payload2, &mempool, &no_validator).unwrap();
        assert!(result.is_some());

        let segment = result.unwrap();
        let mut decoder = minicbor::Decoder::new(&segment.payload);
        let _ = decoder.array();
        assert_eq!(decoder.u32().unwrap(), 2); // MsgRejectTx
    }

    #[test]
    fn test_encode_query_result_protocol_params() {
        let pp = ProtocolParamsSnapshot {
            min_fee_a: 44,
            min_fee_b: 155381,
            ..Default::default()
        };
        let result = QueryResult::ProtocolParams(Box::new(pp));
        let cbor = encode_query_result(&result);
        assert!(!cbor.is_empty());

        // Verify wire format: [4, [array(31, ...)]]
        let mut decoder = minicbor::Decoder::new(&cbor);
        let _ = decoder.array(); // outer [4, ...]
        assert_eq!(decoder.u32().unwrap(), 4); // MsgResult tag
                                               // HFC success wrapper: array(1)
        let hfc_len = decoder.array().unwrap().unwrap();
        assert_eq!(hfc_len, 1);
        // PParams: positional array(31)
        let arr_len = decoder.array().unwrap().unwrap();
        assert_eq!(arr_len, 31);
        // First element: txFeePerByte = 44
        assert_eq!(decoder.u64().unwrap(), 44);
        // Second element: txFeeFixed = 155381
        assert_eq!(decoder.u64().unwrap(), 155381);
    }

    #[test]
    fn test_encode_query_result_gov_state() {
        use crate::query_handler::{
            CommitteeSnapshot, GovStateSnapshot, ProposalSnapshot, ProtocolParamsSnapshot,
        };

        let result = QueryResult::GovState(Box::new(GovStateSnapshot {
            proposals: vec![ProposalSnapshot {
                tx_id: vec![0xaa; 32],
                action_index: 0,
                action_type: "InfoAction".to_string(),
                proposed_epoch: 100,
                expires_epoch: 106,
                yes_votes: 5,
                no_votes: 2,
                abstain_votes: 1,
                deposit: 100_000_000_000,
                return_addr: vec![0xbb; 29],
                anchor_url: "https://example.com/proposal".to_string(),
                anchor_hash: vec![0xcc; 32],
            }],
            committee: CommitteeSnapshot::default(),
            constitution_url: "https://constitution.example.com".to_string(),
            constitution_hash: vec![0xdd; 32],
            constitution_script: None,
            cur_pparams: Box::new(ProtocolParamsSnapshot::default()),
            prev_pparams: Box::new(ProtocolParamsSnapshot::default()),
            enacted_pparam_update: None,
            enacted_hard_fork: None,
            enacted_committee: None,
            enacted_constitution: None,
        }));
        let cbor = encode_query_result(&result);
        assert!(!cbor.is_empty());

        // Verify the outer structure: [4, [result]] (MsgResult with HFC wrapper)
        let mut decoder = minicbor::Decoder::new(&cbor);
        let _ = decoder.array();
        assert_eq!(decoder.u32().unwrap(), 4); // MsgResult
        let _ = decoder.array(); // HFC wrapper
                                 // Verify array(7) ConwayGovState
        assert_eq!(decoder.array().unwrap(), Some(7));
    }

    #[test]
    fn test_encode_query_result_drep_state() {
        use crate::query_handler::DRepSnapshot;

        let result = QueryResult::DRepState(vec![DRepSnapshot {
            credential_hash: vec![0xdd; 28],
            credential_type: 0,
            deposit: 500_000_000,
            anchor_url: Some("https://example.com".to_string()),
            anchor_hash: Some(vec![0xee; 32]),
            expiry_epoch: 62,
            delegator_hashes: Vec::new(),
        }]);
        let cbor = encode_query_result(&result);
        assert!(!cbor.is_empty());
    }

    #[test]
    fn test_encode_query_result_committee_state() {
        use crate::query_handler::{CommitteeMemberSnapshot, CommitteeSnapshot};

        let result = QueryResult::CommitteeState(CommitteeSnapshot {
            members: vec![CommitteeMemberSnapshot {
                cold_credential: vec![0x01; 28],
                cold_credential_type: 0,
                hot_status: 0,
                hot_credential: Some(vec![0x02; 28]),
                member_status: 0,
                expiry_epoch: Some(200),
            }],
            threshold: Some((2, 3)),
            current_epoch: 100,
        });
        let cbor = encode_query_result(&result);
        assert!(!cbor.is_empty());
    }

    #[test]
    fn test_encode_query_result_stake_distribution() {
        use crate::query_handler::StakePoolSnapshot;

        let result = QueryResult::StakeDistribution(vec![StakePoolSnapshot {
            pool_id: vec![0xaa; 28],
            stake: 1_000_000_000,
            vrf_keyhash: vec![0x11; 32],
            total_active_stake: 3_000_000_000,
        }]);
        let cbor = encode_query_result(&result);

        // Verify encoding: [4, [map{pool_id => [tag(30)[num,den], vrf_hash]}]]
        let mut decoder = minicbor::Decoder::new(&cbor);
        let _ = decoder.array();
        assert_eq!(decoder.u32().unwrap(), 4);
        // HFC success wrapper
        let hfc_len = decoder.array().unwrap().unwrap();
        assert_eq!(hfc_len, 1);
        let map_len = decoder.map().unwrap().unwrap();
        assert_eq!(map_len, 1);
        let pool_id = decoder.bytes().unwrap();
        assert_eq!(pool_id, vec![0xaa; 28]);
        let _ = decoder.array(); // array(2) for IndividualPoolStake
                                 // tag(30) rational fraction
        let tag = decoder.tag().unwrap();
        assert_eq!(tag.as_u64(), 30);
        let _ = decoder.array(); // [num, den]
        let num = decoder.u64().unwrap();
        let den = decoder.u64().unwrap();
        assert_eq!(num, 1_000_000_000);
        assert_eq!(den, 3_000_000_000);
        let vrf = decoder.bytes().unwrap();
        assert_eq!(vrf, vec![0x11; 32]);
    }

    #[test]
    fn test_handle_tx_submission_done() {
        let mempool = Arc::new(Mempool::new(torsten_mempool::MempoolConfig::default()));
        let no_validator: Option<Arc<dyn TxValidator>> = None;

        // Build MsgDone: [3]
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(1).unwrap();
        enc.u32(3).unwrap();

        let result = handle_tx_submission(&payload, &mempool, &no_validator).unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_handle_tx_monitor_acquire() {
        let mempool = Arc::new(Mempool::new(torsten_mempool::MempoolConfig::default()));
        let handler = Arc::new(RwLock::new(QueryHandler::new()));
        let mut cursor = TxMonitorCursor {
            snapshot: Vec::new(),
            position: 0,
            acquired: false,
        };

        // MsgAcquire: [1]
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(1).unwrap();
        enc.u32(1).unwrap();

        let result = handle_tx_monitor(&payload, &mempool, &handler, &mut cursor)
            .await
            .unwrap();
        assert!(result.is_some());

        let segment = result.unwrap();
        assert_eq!(segment.protocol_id, MINI_PROTOCOL_TX_MONITOR);
        assert!(segment.is_responder);

        // Verify MsgAcquired [2, slot_no]
        let mut decoder = minicbor::Decoder::new(&segment.payload);
        let _ = decoder.array();
        assert_eq!(decoder.u32().unwrap(), 2); // MsgAcquired
        let _slot = decoder.u64().unwrap();
    }

    #[tokio::test]
    async fn test_handle_tx_monitor_has_tx() {
        let mempool = Arc::new(Mempool::new(torsten_mempool::MempoolConfig::default()));
        let handler = Arc::new(RwLock::new(QueryHandler::new()));
        let mut cursor = TxMonitorCursor {
            snapshot: Vec::new(),
            position: 0,
            acquired: false,
        };
        let tx_hash = Hash32::from_bytes([0xAA; 32]);
        let tx = torsten_primitives::transaction::Transaction::empty_with_hash(tx_hash);
        mempool.add_tx(tx_hash, tx, 100).unwrap();

        // MsgHasTx: [7, tx_id_bytes]
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(2).unwrap();
        enc.u32(7).unwrap();
        enc.bytes(tx_hash.as_bytes()).unwrap();

        let result = handle_tx_monitor(&payload, &mempool, &handler, &mut cursor)
            .await
            .unwrap();
        assert!(result.is_some());

        let segment = result.unwrap();
        let mut decoder = minicbor::Decoder::new(&segment.payload);
        let _ = decoder.array();
        assert_eq!(decoder.u32().unwrap(), 8); // MsgReplyHasTx
        assert!(decoder.bool().unwrap()); // tx exists
    }

    #[tokio::test]
    async fn test_handle_tx_monitor_has_tx_missing() {
        let mempool = Arc::new(Mempool::new(torsten_mempool::MempoolConfig::default()));
        let handler = Arc::new(RwLock::new(QueryHandler::new()));
        let mut cursor = TxMonitorCursor {
            snapshot: Vec::new(),
            position: 0,
            acquired: false,
        };

        // MsgHasTx for non-existent tx
        let tx_hash = Hash32::from_bytes([0xBB; 32]);
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(2).unwrap();
        enc.u32(7).unwrap();
        enc.bytes(tx_hash.as_bytes()).unwrap();

        let result = handle_tx_monitor(&payload, &mempool, &handler, &mut cursor)
            .await
            .unwrap();
        assert!(result.is_some());

        let segment = result.unwrap();
        let mut decoder = minicbor::Decoder::new(&segment.payload);
        let _ = decoder.array();
        assert_eq!(decoder.u32().unwrap(), 8); // MsgReplyHasTx
        assert!(!decoder.bool().unwrap()); // tx does not exist
    }

    #[tokio::test]
    async fn test_handle_tx_monitor_get_sizes() {
        let mempool = Arc::new(Mempool::new(torsten_mempool::MempoolConfig::default()));
        let handler = Arc::new(RwLock::new(QueryHandler::new()));
        let mut cursor = TxMonitorCursor {
            snapshot: Vec::new(),
            position: 0,
            acquired: false,
        };
        let tx_hash = Hash32::from_bytes([0xAA; 32]);
        let tx = torsten_primitives::transaction::Transaction::empty_with_hash(tx_hash);
        mempool.add_tx(tx_hash, tx, 500).unwrap();

        // MsgGetSizes: [9]
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(1).unwrap();
        enc.u32(9).unwrap();

        let result = handle_tx_monitor(&payload, &mempool, &handler, &mut cursor)
            .await
            .unwrap();
        assert!(result.is_some());

        let segment = result.unwrap();
        let mut decoder = minicbor::Decoder::new(&segment.payload);
        let _ = decoder.array();
        assert_eq!(decoder.u32().unwrap(), 10); // MsgReplyGetSizes
        let _ = decoder.array();
        let capacity = decoder.u64().unwrap();
        let size = decoder.u64().unwrap();
        let num_txs = decoder.u64().unwrap();
        assert_eq!(capacity, 16384); // default max_transactions
        assert_eq!(size, 500);
        assert_eq!(num_txs, 1);
    }

    #[tokio::test]
    async fn test_handle_tx_monitor_next_tx() {
        let mempool = Arc::new(Mempool::new(torsten_mempool::MempoolConfig::default()));
        let handler = Arc::new(RwLock::new(QueryHandler::new()));
        let mut cursor = TxMonitorCursor {
            snapshot: Vec::new(),
            position: 0,
            acquired: false,
        };

        // MsgNextTx: [5]
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(1).unwrap();
        enc.u32(5).unwrap();

        let result = handle_tx_monitor(&payload, &mempool, &handler, &mut cursor)
            .await
            .unwrap();
        assert!(result.is_some());

        let segment = result.unwrap();
        let mut decoder = minicbor::Decoder::new(&segment.payload);
        let _ = decoder.array();
        assert_eq!(decoder.u32().unwrap(), 6); // MsgReplyNextTx
        assert!(decoder.null().is_ok()); // no tx available
    }

    #[tokio::test]
    async fn test_handle_tx_monitor_cursor_iteration() {
        let mempool = Arc::new(Mempool::new(torsten_mempool::MempoolConfig::default()));
        let handler = Arc::new(RwLock::new(QueryHandler::new()));
        let mut cursor = TxMonitorCursor {
            snapshot: Vec::new(),
            position: 0,
            acquired: false,
        };

        // Add 3 transactions to the mempool
        let h1 = Hash32::from_bytes([0x01; 32]);
        let h2 = Hash32::from_bytes([0x02; 32]);
        let h3 = Hash32::from_bytes([0x03; 32]);
        let mut tx1 = torsten_primitives::transaction::Transaction::empty_with_hash(h1);
        tx1.raw_cbor = Some(vec![0x01; 10]);
        let mut tx2 = torsten_primitives::transaction::Transaction::empty_with_hash(h2);
        tx2.raw_cbor = Some(vec![0x02; 10]);
        let mut tx3 = torsten_primitives::transaction::Transaction::empty_with_hash(h3);
        tx3.raw_cbor = Some(vec![0x03; 10]);
        mempool.add_tx(h1, tx1, 100).unwrap();
        mempool.add_tx(h2, tx2, 100).unwrap();
        mempool.add_tx(h3, tx3, 100).unwrap();

        // MsgAcquire to take a snapshot
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(1).unwrap();
        enc.u32(1).unwrap();
        let _ = handle_tx_monitor(&payload, &mempool, &handler, &mut cursor)
            .await
            .unwrap();
        assert!(cursor.acquired);
        assert_eq!(cursor.snapshot.len(), 3);
        assert_eq!(cursor.position, 0);

        // MsgNextTx should return tx1
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(1).unwrap();
        enc.u32(5).unwrap();
        let result = handle_tx_monitor(&payload, &mempool, &handler, &mut cursor)
            .await
            .unwrap()
            .unwrap();
        let mut d = minicbor::Decoder::new(&result.payload);
        let _ = d.array();
        assert_eq!(d.u32().unwrap(), 6); // MsgReplyNextTx
        let _ = d.array(); // [era, bytes] — not null, so a tx was returned
        assert_eq!(cursor.position, 1);

        // MsgNextTx should return tx2
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(1).unwrap();
        enc.u32(5).unwrap();
        let _ = handle_tx_monitor(&payload, &mempool, &handler, &mut cursor)
            .await
            .unwrap();
        assert_eq!(cursor.position, 2);

        // MsgNextTx should return tx3
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(1).unwrap();
        enc.u32(5).unwrap();
        let _ = handle_tx_monitor(&payload, &mempool, &handler, &mut cursor)
            .await
            .unwrap();
        assert_eq!(cursor.position, 3);

        // MsgNextTx should return null (end of snapshot)
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(1).unwrap();
        enc.u32(5).unwrap();
        let result = handle_tx_monitor(&payload, &mempool, &handler, &mut cursor)
            .await
            .unwrap()
            .unwrap();
        let mut d = minicbor::Decoder::new(&result.payload);
        let _ = d.array();
        assert_eq!(d.u32().unwrap(), 6);
        assert!(d.null().is_ok()); // null = end of snapshot
    }

    #[tokio::test]
    async fn test_handle_tx_monitor_done() {
        let mempool = Arc::new(Mempool::new(torsten_mempool::MempoolConfig::default()));
        let handler = Arc::new(RwLock::new(QueryHandler::new()));
        let mut cursor = TxMonitorCursor {
            snapshot: Vec::new(),
            position: 0,
            acquired: false,
        };

        // MsgDone: [0]
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(1).unwrap();
        enc.u32(0).unwrap();

        let result = handle_tx_monitor(&payload, &mempool, &handler, &mut cursor)
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_handle_local_chainsync_request_next_no_provider() {
        let handler = Arc::new(RwLock::new(QueryHandler::new()));
        let no_provider: Option<Arc<dyn BlockProvider>> = None;
        let mut cursor = ChainSyncCursor {
            cursor_slot: 0,
            has_intersection: false,
        };

        // MsgRequestNext: [0]
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(1).unwrap();
        enc.u32(0).unwrap();

        let result = handle_local_chainsync(&payload, &handler, &no_provider, &mut cursor)
            .await
            .unwrap();
        assert!(result.is_some());

        let segment = result.unwrap();
        assert_eq!(segment.protocol_id, MINI_PROTOCOL_CHAINSYNC);
        assert!(segment.is_responder);

        // Should be MsgAwaitReply [1]
        let mut decoder = minicbor::Decoder::new(&segment.payload);
        let _ = decoder.array();
        assert_eq!(decoder.u32().unwrap(), 1); // MsgAwaitReply
    }

    #[tokio::test]
    async fn test_handle_local_chainsync_find_intersect_not_found() {
        let handler = Arc::new(RwLock::new(QueryHandler::new()));
        let no_provider: Option<Arc<dyn BlockProvider>> = None;
        let mut cursor = ChainSyncCursor {
            cursor_slot: 0,
            has_intersection: false,
        };

        // MsgFindIntersect: [4, [[12345, hash]]]
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(2).unwrap();
        enc.u32(4).unwrap(); // MsgFindIntersect
        enc.array(1).unwrap(); // one point
        enc.array(2).unwrap(); // point: [slot, hash]
        enc.u64(12345).unwrap();
        enc.bytes(&[0xaa; 32]).unwrap();

        let result = handle_local_chainsync(&payload, &handler, &no_provider, &mut cursor)
            .await
            .unwrap();
        assert!(result.is_some());

        let segment = result.unwrap();
        let mut decoder = minicbor::Decoder::new(&segment.payload);
        let _ = decoder.array();
        assert_eq!(decoder.u32().unwrap(), 6); // MsgIntersectNotFound
        assert!(!cursor.has_intersection);
    }

    #[tokio::test]
    async fn test_handle_local_chainsync_done() {
        let handler = Arc::new(RwLock::new(QueryHandler::new()));
        let no_provider: Option<Arc<dyn BlockProvider>> = None;
        let mut cursor = ChainSyncCursor {
            cursor_slot: 100,
            has_intersection: true,
        };

        // MsgDone: [7]
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(1).unwrap();
        enc.u32(7).unwrap();

        let result = handle_local_chainsync(&payload, &handler, &no_provider, &mut cursor)
            .await
            .unwrap();
        assert!(result.is_none());
        assert!(!cursor.has_intersection);
    }

    #[tokio::test]
    async fn test_handle_local_chainsync_block_delivery() {
        use crate::n2n_server::{BlockProvider, TipInfo};

        struct MockBlockProvider;

        impl BlockProvider for MockBlockProvider {
            fn get_block(&self, _hash: &[u8; 32]) -> Option<Vec<u8>> {
                None
            }
            fn has_block(&self, hash: &[u8; 32]) -> bool {
                // Only recognize our test hash
                *hash == [0xbb; 32]
            }
            fn get_tip(&self) -> TipInfo {
                TipInfo {
                    slot: 200,
                    hash: [0xcc; 32],
                    block_number: 10,
                }
            }
            fn get_next_block_after_slot(
                &self,
                after_slot: u64,
            ) -> Option<(u64, [u8; 32], Vec<u8>)> {
                if after_slot < 100 {
                    // Return a fake block at slot 100
                    Some((100, [0xdd; 32], vec![0x82, 0x00, 0x80])) // minimal CBOR
                } else if after_slot < 200 {
                    Some((200, [0xee; 32], vec![0x82, 0x00, 0x80]))
                } else {
                    None
                }
            }
        }

        let handler = Arc::new(RwLock::new(QueryHandler::new()));
        let provider: Option<Arc<dyn BlockProvider>> = Some(Arc::new(MockBlockProvider));
        let mut cursor = ChainSyncCursor {
            cursor_slot: 0,
            has_intersection: false,
        };

        // Step 1: MsgFindIntersect with a known hash
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(2).unwrap();
        enc.u32(4).unwrap();
        enc.array(1).unwrap();
        enc.array(2).unwrap();
        enc.u64(50).unwrap();
        enc.bytes(&[0xbb; 32]).unwrap(); // Known hash

        let result = handle_local_chainsync(&payload, &handler, &provider, &mut cursor)
            .await
            .unwrap();
        let segment = result.unwrap();
        let mut decoder = minicbor::Decoder::new(&segment.payload);
        let _ = decoder.array();
        assert_eq!(decoder.u32().unwrap(), 5); // MsgIntersectFound
        assert!(cursor.has_intersection);
        assert_eq!(cursor.cursor_slot, 50);

        // Step 2: MsgRequestNext — should get MsgRollForward with block at slot 100
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(1).unwrap();
        enc.u32(0).unwrap();

        let result = handle_local_chainsync(&payload, &handler, &provider, &mut cursor)
            .await
            .unwrap();
        let segment = result.unwrap();
        let mut decoder = minicbor::Decoder::new(&segment.payload);
        let _ = decoder.array();
        assert_eq!(decoder.u32().unwrap(), 2); // MsgRollForward
        assert_eq!(cursor.cursor_slot, 100);

        // Step 3: MsgRequestNext — should get block at slot 200
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(1).unwrap();
        enc.u32(0).unwrap();

        let result = handle_local_chainsync(&payload, &handler, &provider, &mut cursor)
            .await
            .unwrap();
        let segment = result.unwrap();
        let mut decoder = minicbor::Decoder::new(&segment.payload);
        let _ = decoder.array();
        assert_eq!(decoder.u32().unwrap(), 2); // MsgRollForward
        assert_eq!(cursor.cursor_slot, 200);

        // Step 4: MsgRequestNext — should get MsgAwaitReply (no more blocks)
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(1).unwrap();
        enc.u32(0).unwrap();

        let result = handle_local_chainsync(&payload, &handler, &provider, &mut cursor)
            .await
            .unwrap();
        let segment = result.unwrap();
        let mut decoder = minicbor::Decoder::new(&segment.payload);
        let _ = decoder.array();
        assert_eq!(decoder.u32().unwrap(), 1); // MsgAwaitReply
    }

    #[tokio::test]
    async fn test_handle_local_chainsync_rollback() {
        use crate::n2n_server::{BlockProvider, TipInfo};
        use std::sync::atomic::{AtomicU64, Ordering};

        struct RollbackProvider {
            tip_slot: AtomicU64,
        }

        impl BlockProvider for RollbackProvider {
            fn get_block(&self, _hash: &[u8; 32]) -> Option<Vec<u8>> {
                None
            }
            fn has_block(&self, hash: &[u8; 32]) -> bool {
                *hash == [0xbb; 32]
            }
            fn get_tip(&self) -> TipInfo {
                let slot = self.tip_slot.load(Ordering::Relaxed);
                TipInfo {
                    slot,
                    hash: [0xcc; 32],
                    block_number: slot / 10,
                }
            }
            fn get_next_block_after_slot(
                &self,
                after_slot: u64,
            ) -> Option<(u64, [u8; 32], Vec<u8>)> {
                let tip = self.tip_slot.load(Ordering::Relaxed);
                if after_slot < tip {
                    Some((after_slot + 10, [0xdd; 32], vec![0x82, 0x00, 0x80]))
                } else {
                    None
                }
            }
        }

        let handler = Arc::new(RwLock::new(QueryHandler::new()));
        let provider_inner = Arc::new(RollbackProvider {
            tip_slot: AtomicU64::new(200),
        });
        let provider: Option<Arc<dyn BlockProvider>> =
            Some(provider_inner.clone() as Arc<dyn BlockProvider>);
        let mut cursor = ChainSyncCursor {
            cursor_slot: 150,
            has_intersection: true,
        };

        // Normal MsgRequestNext — should get MsgRollForward
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(1).unwrap();
        enc.u32(0).unwrap();

        let result = handle_local_chainsync(&payload, &handler, &provider, &mut cursor)
            .await
            .unwrap();
        let segment = result.unwrap();
        let mut decoder = minicbor::Decoder::new(&segment.payload);
        let _ = decoder.array();
        assert_eq!(decoder.u32().unwrap(), 2); // MsgRollForward
        assert_eq!(cursor.cursor_slot, 160);

        // Simulate rollback: tip moves back to 100
        provider_inner.tip_slot.store(100, Ordering::Relaxed);

        // MsgRequestNext — cursor at 160 > tip at 100, should get MsgRollBackward
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(1).unwrap();
        enc.u32(0).unwrap();

        let result = handle_local_chainsync(&payload, &handler, &provider, &mut cursor)
            .await
            .unwrap();
        let segment = result.unwrap();
        let mut decoder = minicbor::Decoder::new(&segment.payload);
        let _ = decoder.array();
        assert_eq!(decoder.u32().unwrap(), 3); // MsgRollBackward
        assert_eq!(cursor.cursor_slot, 100); // cursor moved back to tip
    }

    #[test]
    fn test_parse_utctime() {
        // Preview testnet: 2022-04-01T00:00:00Z → (2022, 91, 0)
        let (y, d, p) = state_query::parse_utctime_for_test("2022-04-01T00:00:00Z");
        assert_eq!(y, 2022);
        assert_eq!(d, 91); // April 1 = day 91 in non-leap year
        assert_eq!(p, 0);

        // Mainnet: 2017-09-23T21:44:51Z
        let (y, d, p) = state_query::parse_utctime_for_test("2017-09-23T21:44:51Z");
        assert_eq!(y, 2017);
        assert_eq!(d, 266); // Sep 23 = day 266
        assert_eq!(p, (21 * 3600 + 44 * 60 + 51) * 1_000_000_000_000);

        // Leap year: 2024-03-01T00:00:00Z
        let (y, d, _) = state_query::parse_utctime_for_test("2024-03-01T00:00:00Z");
        assert_eq!(y, 2024);
        assert_eq!(d, 61); // Jan(31) + Feb(29, leap) + 1 = 61
    }

    #[test]
    fn test_genesis_config_cbor_array15() {
        use crate::query_handler::{GenesisConfigSnapshot, ShelleyPParamsSnapshot};

        let gc = GenesisConfigSnapshot {
            system_start: "2022-04-01T00:00:00Z".to_string(),
            network_magic: 2,
            network_id: 0,
            active_slots_coeff_num: 1,
            active_slots_coeff_den: 20,
            security_param: 2160,
            epoch_length: 86400,
            slots_per_kes_period: 129600,
            max_kes_evolutions: 62,
            slot_length_micros: 1_000_000,
            update_quorum: 5,
            max_lovelace_supply: 45_000_000_000_000_000,
            protocol_params: ShelleyPParamsSnapshot {
                min_fee_a: 44,
                min_fee_b: 155381,
                max_block_body_size: 65536,
                max_tx_size: 16384,
                max_block_header_size: 1100,
                key_deposit: 2_000_000,
                pool_deposit: 500_000_000,
                e_max: 18,
                n_opt: 150,
                a0_num: 3,
                a0_den: 10,
                rho_num: 3,
                rho_den: 1000,
                tau_num: 2,
                tau_den: 10,
                d_num: 0,
                d_den: 1,
                protocol_version_major: 2,
                protocol_version_minor: 0,
                min_utxo_value: 1_000_000,
                min_pool_cost: 340_000_000,
            },
            gen_delegs: Vec::new(),
        };

        let result = QueryResult::GenesisConfig(Box::new(gc));
        let encoded = encode_query_result(&result);

        // Decode and verify it's array(15) inside the HFC wrapper
        let mut dec = minicbor::Decoder::new(&encoded);
        let _ = dec.array(); // [4, ...]
        assert_eq!(dec.u32().unwrap(), 4); // MsgResult
        let _ = dec.array(); // HFC success wrapper
        let len = dec.array().unwrap(); // CompactGenesis array
        assert_eq!(len, Some(15));

        // [0] UTCTime: array(3)
        assert_eq!(dec.array().unwrap(), Some(3));
        assert_eq!(dec.u64().unwrap(), 2022); // year
        assert_eq!(dec.u64().unwrap(), 91); // dayOfYear
        assert_eq!(dec.u64().unwrap(), 0); // picos

        // [1] networkMagic
        assert_eq!(dec.u32().unwrap(), 2);

        // [2] networkId
        assert_eq!(dec.u32().unwrap(), 0); // Testnet

        // [3] activeSlotsCoeff: [num, den] no tag
        assert_eq!(dec.array().unwrap(), Some(2));
        assert_eq!(dec.u64().unwrap(), 1);
        assert_eq!(dec.u64().unwrap(), 20);

        // [4] securityParam
        assert_eq!(dec.u64().unwrap(), 2160);

        // [5] epochLength
        assert_eq!(dec.u64().unwrap(), 86400);

        // [6] slotsPerKESPeriod
        assert_eq!(dec.u64().unwrap(), 129600);

        // [7] maxKESEvolutions
        assert_eq!(dec.u64().unwrap(), 62);

        // [8] slotLength (microseconds)
        assert_eq!(dec.u64().unwrap(), 1_000_000);

        // [9] updateQuorum
        assert_eq!(dec.u64().unwrap(), 5);

        // [10] maxLovelaceSupply
        assert_eq!(dec.u64().unwrap(), 45_000_000_000_000_000);

        // [11] protocolParams: array(18)
        assert_eq!(dec.array().unwrap(), Some(18));
        assert_eq!(dec.u64().unwrap(), 44); // minFeeA
        assert_eq!(dec.u64().unwrap(), 155381); // minFeeB

        // Skip remaining pparams fields
        for _ in 2..18 {
            dec.skip().unwrap();
        }

        // [12] genDelegs: empty map
        assert_eq!(dec.map().unwrap(), Some(0));

        // [13] initialFunds: empty map
        assert_eq!(dec.map().unwrap(), Some(0));

        // [14] staking: array(2) [empty_map, empty_map]
        assert_eq!(dec.array().unwrap(), Some(2));
        assert_eq!(dec.map().unwrap(), Some(0));
        assert_eq!(dec.map().unwrap(), Some(0));
    }

    #[test]
    fn test_encode_query_result_pool_distr() {
        use crate::query_handler::StakePoolSnapshot;
        let result = QueryResult::PoolDistr(vec![StakePoolSnapshot {
            pool_id: vec![0xaa; 28],
            stake: 1_000_000_000,
            vrf_keyhash: vec![0x11; 32],
            total_active_stake: 3_000_000_000,
        }]);
        let cbor = encode_query_result(&result);
        assert!(!cbor.is_empty());
        // [4, [1, map(1)]]
        let mut dec = minicbor::Decoder::new(&cbor);
        assert_eq!(dec.array().unwrap(), Some(2)); // MsgResult wrapper
        assert_eq!(dec.u32().unwrap(), 4); // MsgResult tag
        assert_eq!(dec.array().unwrap(), Some(1)); // HFC success wrapper
        assert_eq!(dec.map().unwrap(), Some(1)); // 1 pool
    }

    #[test]
    fn test_encode_query_result_stake_deleg_deposits() {
        use crate::query_handler::StakeDelegDepositEntry;
        let result = QueryResult::StakeDelegDeposits(vec![StakeDelegDepositEntry {
            credential_hash: vec![0xaa; 28],
            credential_type: 0,
            deposit: 2_000_000,
        }]);
        let cbor = encode_query_result(&result);
        assert!(!cbor.is_empty());
        let mut dec = minicbor::Decoder::new(&cbor);
        assert_eq!(dec.array().unwrap(), Some(2));
        assert_eq!(dec.u32().unwrap(), 4);
        assert_eq!(dec.array().unwrap(), Some(1));
        assert_eq!(dec.map().unwrap(), Some(1));
    }

    #[test]
    fn test_encode_query_result_drep_stake_distr() {
        use crate::query_handler::DRepStakeEntry;
        let result = QueryResult::DRepStakeDistr(vec![
            DRepStakeEntry {
                drep_type: 0,
                drep_hash: Some(vec![0xdd; 28]),
                stake: 500_000_000,
            },
            DRepStakeEntry {
                drep_type: 2,
                drep_hash: None,
                stake: 100_000_000,
            },
        ]);
        let cbor = encode_query_result(&result);
        assert!(!cbor.is_empty());
        let mut dec = minicbor::Decoder::new(&cbor);
        assert_eq!(dec.array().unwrap(), Some(2));
        assert_eq!(dec.u32().unwrap(), 4);
        assert_eq!(dec.array().unwrap(), Some(1));
        assert_eq!(dec.map().unwrap(), Some(2));
    }

    #[test]
    fn test_encode_query_result_filtered_vote_delegatees() {
        use crate::query_handler::VoteDelegateeEntry;
        let result = QueryResult::FilteredVoteDelegatees(vec![VoteDelegateeEntry {
            credential_hash: vec![0xaa; 28],
            credential_type: 0,
            drep_type: 0,
            drep_hash: Some(vec![0xdd; 28]),
        }]);
        let cbor = encode_query_result(&result);
        assert!(!cbor.is_empty());
        let mut dec = minicbor::Decoder::new(&cbor);
        assert_eq!(dec.array().unwrap(), Some(2));
        assert_eq!(dec.u32().unwrap(), 4);
        assert_eq!(dec.array().unwrap(), Some(1));
        assert_eq!(dec.map().unwrap(), Some(1));
    }

    // ---- TxValidationError tests ----

    #[test]
    fn test_tx_validation_error_display_decode_failed() {
        let err = TxValidationError::DecodeFailed {
            reason: "invalid CBOR".into(),
        };
        assert_eq!(
            err.to_string(),
            "Failed to decode transaction: invalid CBOR"
        );
    }

    #[test]
    fn test_tx_validation_error_display_ledger_unavailable() {
        let err = TxValidationError::LedgerStateUnavailable;
        assert_eq!(err.to_string(), "Ledger state unavailable");
    }

    #[test]
    fn test_tx_validation_error_display_no_inputs() {
        let err = TxValidationError::NoInputs;
        assert_eq!(err.to_string(), "No inputs in transaction");
    }

    #[test]
    fn test_tx_validation_error_display_fee_too_small() {
        let err = TxValidationError::FeeTooSmall {
            minimum: 200_000,
            actual: 100_000,
        };
        assert_eq!(
            err.to_string(),
            "Fee too small: minimum=200000, actual=100000"
        );
    }

    #[test]
    fn test_tx_validation_error_display_value_not_conserved() {
        let err = TxValidationError::ValueNotConserved {
            inputs: 5_000_000,
            outputs: 3_000_000,
            fee: 1_000_000,
        };
        assert_eq!(
            err.to_string(),
            "Value not conserved: inputs=5000000, outputs=3000000, fee=1000000"
        );
    }

    #[test]
    fn test_tx_validation_error_display_ttl_expired() {
        let err = TxValidationError::TtlExpired {
            current_slot: 1000,
            ttl: 500,
        };
        assert_eq!(err.to_string(), "TTL expired: current_slot=1000, ttl=500");
    }

    #[test]
    fn test_tx_validation_error_display_tx_too_large() {
        let err = TxValidationError::TxTooLarge {
            maximum: 16384,
            actual: 32000,
        };
        assert_eq!(
            err.to_string(),
            "Transaction too large: maximum=16384, actual=32000"
        );
    }

    #[test]
    fn test_tx_validation_error_display_multiple() {
        let err = TxValidationError::Multiple(vec![
            TxValidationError::NoInputs,
            TxValidationError::FeeTooSmall {
                minimum: 200_000,
                actual: 100_000,
            },
        ]);
        assert_eq!(
            err.to_string(),
            "No inputs in transaction; Fee too small: minimum=200000, actual=100000"
        );
    }

    #[test]
    fn test_tx_validation_error_is_decode_error() {
        let decode_err = TxValidationError::DecodeFailed {
            reason: "bad".into(),
        };
        assert!(decode_err.is_decode_error());
        assert!(!TxValidationError::NoInputs.is_decode_error());
    }

    #[test]
    fn test_tx_validation_error_is_availability_error() {
        assert!(TxValidationError::LedgerStateUnavailable.is_availability_error());
        assert!(!TxValidationError::NoInputs.is_availability_error());
    }

    #[test]
    fn test_tx_validation_error_errors_single() {
        let err = TxValidationError::NoInputs;
        let errors = err.errors();
        assert_eq!(errors.len(), 1);
        assert!(matches!(errors[0], TxValidationError::NoInputs));
    }

    #[test]
    fn test_tx_validation_error_errors_multiple() {
        let err = TxValidationError::Multiple(vec![
            TxValidationError::NoInputs,
            TxValidationError::InsufficientCollateral,
            TxValidationError::NativeScriptFailed,
        ]);
        let errors = err.errors();
        assert_eq!(errors.len(), 3);
        assert!(matches!(errors[0], TxValidationError::NoInputs));
        assert!(matches!(
            errors[1],
            TxValidationError::InsufficientCollateral
        ));
        assert!(matches!(errors[2], TxValidationError::NativeScriptFailed));
    }

    #[test]
    fn test_tx_validation_error_display_all_variants() {
        let variants: Vec<TxValidationError> = vec![
            TxValidationError::DecodeFailed {
                reason: "test".into(),
            },
            TxValidationError::LedgerStateUnavailable,
            TxValidationError::NoInputs,
            TxValidationError::InputNotFound {
                input: "tx#0".into(),
            },
            TxValidationError::ValueNotConserved {
                inputs: 1,
                outputs: 2,
                fee: 3,
            },
            TxValidationError::FeeTooSmall {
                minimum: 1,
                actual: 0,
            },
            TxValidationError::OutputTooSmall {
                minimum: 1,
                actual: 0,
            },
            TxValidationError::TxTooLarge {
                maximum: 1,
                actual: 2,
            },
            TxValidationError::MissingRequiredSigner {
                signer: "abc".into(),
            },
            TxValidationError::MissingWitness {
                input: "tx#1".into(),
            },
            TxValidationError::TtlExpired {
                current_slot: 10,
                ttl: 5,
            },
            TxValidationError::NotYetValid {
                current_slot: 5,
                valid_from: 10,
            },
            TxValidationError::ScriptFailed {
                reason: "eval error".into(),
            },
            TxValidationError::InsufficientCollateral,
            TxValidationError::TooManyCollateralInputs { max: 3, actual: 5 },
            TxValidationError::CollateralNotFound {
                input: "col#0".into(),
            },
            TxValidationError::CollateralHasTokens {
                input: "col#1".into(),
            },
            TxValidationError::CollateralMismatch {
                declared: 100,
                computed: 50,
            },
            TxValidationError::ReferenceInputNotFound {
                input: "ref#0".into(),
            },
            TxValidationError::ReferenceInputOverlapsInput {
                input: "ref#1".into(),
            },
            TxValidationError::MultiAssetNotConserved {
                policy: "abc".into(),
                input_side: 10,
                output_side: 20,
            },
            TxValidationError::InvalidMint,
            TxValidationError::ExUnitsExceeded,
            TxValidationError::ScriptDataHashMismatch {
                expected: "aaa".into(),
                actual: "bbb".into(),
            },
            TxValidationError::UnexpectedScriptDataHash,
            TxValidationError::MissingScriptDataHash,
            TxValidationError::DuplicateInput {
                input: "dup#0".into(),
            },
            TxValidationError::NativeScriptFailed,
            TxValidationError::InvalidWitnessSignature { vkey: "vk".into() },
            TxValidationError::NetworkMismatch {
                expected: "Testnet".into(),
                actual: "Mainnet".into(),
            },
            TxValidationError::AuxiliaryDataHashWithoutData,
            TxValidationError::AuxiliaryDataWithoutHash,
            TxValidationError::BlockExUnitsExceeded {
                resource: "mem".into(),
                limit: 100,
                total: 200,
            },
            TxValidationError::OutputValueTooLarge {
                maximum: 100,
                actual: 200,
            },
            TxValidationError::MissingRawCbor,
            TxValidationError::MissingSlotConfig,
            TxValidationError::MissingSpendRedeemer { index: 0 },
            TxValidationError::RedeemerIndexOutOfRange {
                tag: "Spend".into(),
                index: 5,
                max: 3,
            },
            TxValidationError::MissingInputWitness {
                credential: "cred#0".into(),
            },
            TxValidationError::MissingScriptWitness {
                credential: "script#0".into(),
            },
            TxValidationError::MissingWithdrawalWitness {
                credential: "wdrl#0".into(),
            },
            TxValidationError::MissingWithdrawalScriptWitness {
                credential: "wdrl_script#0".into(),
            },
            TxValidationError::Multiple(vec![TxValidationError::NoInputs]),
        ];
        for variant in &variants {
            let msg = variant.to_string();
            assert!(!msg.is_empty(), "Empty display for: {variant:?}");
        }
    }

    #[test]
    fn test_tx_validation_error_implements_std_error() {
        let err: Box<dyn std::error::Error> = Box::new(TxValidationError::NoInputs);
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn test_tx_validation_error_pattern_matching() {
        let err = TxValidationError::FeeTooSmall {
            minimum: 200_000,
            actual: 100_000,
        };
        match &err {
            TxValidationError::FeeTooSmall { minimum, actual } => {
                assert_eq!(*minimum, 200_000);
                assert_eq!(*actual, 100_000);
            }
            _ => panic!("Expected FeeTooSmall variant"),
        }
    }

    #[test]
    fn test_tx_validator_trait_with_typed_error() {
        struct RejectingValidator;
        impl TxValidator for RejectingValidator {
            fn validate_tx(&self, _era_id: u16, _tx_bytes: &[u8]) -> Result<(), TxValidationError> {
                Err(TxValidationError::NoInputs)
            }
        }
        let validator = RejectingValidator;
        let result = validator.validate_tx(6, &[]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, TxValidationError::NoInputs));
    }

    #[test]
    fn test_tx_validator_trait_accepting() {
        struct AcceptingValidator;
        impl TxValidator for AcceptingValidator {
            fn validate_tx(&self, _era_id: u16, _tx_bytes: &[u8]) -> Result<(), TxValidationError> {
                Ok(())
            }
        }
        let validator = AcceptingValidator;
        assert!(validator.validate_tx(6, &[0x80]).is_ok());
    }
}
