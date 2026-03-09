use std::path::Path;
use std::sync::Arc;
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;
use tokio::sync::RwLock;
use torsten_mempool::Mempool;
use torsten_primitives::hash::Hash32;
use tracing::{debug, error, info, warn};

use crate::multiplexer::Segment;
use crate::n2n_server::BlockProvider;
use crate::query_handler::{ProtocolParamsSnapshot, QueryHandler, QueryResult};

/// Trait for validating transactions before mempool admission.
/// Implementors should perform full Phase-1 and Phase-2 (Plutus) validation.
pub trait TxValidator: Send + Sync + 'static {
    /// Validate a transaction. Returns Ok(()) if valid, or an error string.
    fn validate_tx(&self, era_id: u16, tx_bytes: &[u8]) -> Result<(), String>;
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

/// Per-client LocalChainSync cursor state
struct ChainSyncCursor {
    /// Current cursor slot (blocks after this slot will be served)
    cursor_slot: u64,
    /// Whether the client has found an intersection
    has_intersection: bool,
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
) -> Result<Option<Segment>, N2CServerError> {
    match segment.protocol_id {
        MINI_PROTOCOL_HANDSHAKE => handle_handshake(&segment.payload),
        MINI_PROTOCOL_STATE_QUERY => handle_state_query(&segment.payload, query_handler).await,
        MINI_PROTOCOL_TX_SUBMISSION => {
            handle_tx_submission(&segment.payload, mempool, tx_validator)
        }
        MINI_PROTOCOL_TX_MONITOR => {
            handle_tx_monitor(&segment.payload, mempool, query_handler).await
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
    let network_magic = parse_handshake_magic(payload).unwrap_or(764824073); // mainnet default
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
                && (best.is_none() || logical as u16 > best.unwrap().0)
            {
                best = Some((logical as u16, wire_v));
            }
        }
        // Skip the value (params)
        decoder.skip().ok()?;
    }
    best
}

/// Handle LocalTxSubmission messages
///
/// Protocol flow:
///   Client: MsgSubmitTx(era_id, tx_cbor) → Server: MsgAcceptTx | MsgRejectTx(reason)
///   Client: MsgDone → (end)
///
/// Message tags:
///   0: MsgSubmitTx [0, [era_id, tagged_tx_bytes]]
///   1: MsgAcceptTx [1]
///   2: MsgRejectTx [2, reason]
///   3: MsgDone     [3]
fn handle_tx_submission(
    payload: &[u8],
    mempool: &Arc<Mempool>,
    tx_validator: &Option<Arc<dyn TxValidator>>,
) -> Result<Option<Segment>, N2CServerError> {
    let mut decoder = minicbor::Decoder::new(payload);

    let msg_tag = match decoder.array() {
        Ok(Some(len)) if len >= 1 => decoder
            .u32()
            .map_err(|e| N2CServerError::Protocol(format!("bad tx submission msg tag: {e}")))?,
        Ok(None) => decoder
            .u32()
            .map_err(|e| N2CServerError::Protocol(format!("bad tx submission msg tag: {e}")))?,
        _ => {
            return Err(N2CServerError::Protocol(
                "invalid tx submission message".into(),
            ))
        }
    };

    match msg_tag {
        0 => {
            // MsgSubmitTx: [0, [era_id, tx_bytes]]
            debug!("LocalTxSubmission: MsgSubmitTx");

            // Extract the raw transaction bytes from the submission
            // The payload after tag 0 is [era_id, tx_cbor]
            let tx_data = extract_submitted_tx(&mut decoder);

            match tx_data {
                Some((era_id, tx_bytes)) => {
                    let tx_size = tx_bytes.len();

                    // Run Phase-1/Phase-2 validation if a validator is available
                    if let Some(validator) = tx_validator {
                        if let Err(e) = validator.validate_tx(era_id, &tx_bytes) {
                            warn!("Transaction validation failed: {e}");
                            return encode_tx_reject(&e);
                        }
                    }

                    // Parse the full transaction
                    match torsten_serialization::decode_transaction(era_id, &tx_bytes) {
                        Ok(tx) => {
                            let tx_hash = tx.hash;

                            match mempool.add_tx(tx_hash, tx, tx_size) {
                                Ok(torsten_mempool::MempoolAddResult::Added) => {
                                    info!("Transaction accepted into mempool: {tx_hash}");
                                    encode_tx_accept()
                                }
                                Ok(torsten_mempool::MempoolAddResult::AlreadyExists) => {
                                    debug!("Transaction already in mempool: {tx_hash}");
                                    encode_tx_accept()
                                }
                                Err(e) => {
                                    warn!("Transaction rejected: {e}");
                                    encode_tx_reject(&e.to_string())
                                }
                            }
                        }
                        Err(e) => {
                            warn!("Failed to decode transaction: {e}");
                            encode_tx_reject(&format!("Failed to decode transaction: {e}"))
                        }
                    }
                }
                None => {
                    warn!("Failed to extract transaction from submission");
                    encode_tx_reject("Failed to decode submitted transaction")
                }
            }
        }
        3 => {
            // MsgDone
            debug!("LocalTxSubmission: MsgDone");
            Ok(None)
        }
        other => {
            warn!("Unknown LocalTxSubmission message tag: {other}");
            Ok(None)
        }
    }
}

/// Handle LocalTxMonitor messages
///
/// Protocol flow:
///   Client: MsgAcquire           → Server: MsgAcquired(slot_no)
///   Client: MsgHasTx(tx_id)      → Server: MsgHasTxReply(bool)
///   Client: MsgNextTx            → Server: MsgNextTxReply(maybe_tx)
///   Client: MsgGetSizes          → Server: MsgGetSizesReply(sizes)
///   Client: MsgRelease           → (back to idle)
///   Client: MsgDone              → (end)
///
/// Message tags:
///   0: MsgAcquire     [0]
///   1: MsgAcquired    [1, slot_no]
///   2: MsgRelease     [2]
///   3: MsgDone        [3]
///   4: MsgHasTx       [4, tx_id_bytes]
///   5: MsgHasTxReply  [5, bool]
///   6: MsgNextTx      [6]
///   7: MsgNextTxReply [7, null | [era_id, tx_bytes]]
///   8: MsgGetSizes    [8]
///   9: MsgGetSizesReply [9, [capacity, size, num_txs]]
async fn handle_tx_monitor(
    payload: &[u8],
    mempool: &Arc<Mempool>,
    query_handler: &Arc<RwLock<QueryHandler>>,
) -> Result<Option<Segment>, N2CServerError> {
    let mut decoder = minicbor::Decoder::new(payload);

    let msg_tag = match decoder.array() {
        Ok(Some(len)) if len >= 1 => decoder
            .u32()
            .map_err(|e| N2CServerError::Protocol(format!("bad tx monitor msg tag: {e}")))?,
        Ok(None) => decoder
            .u32()
            .map_err(|e| N2CServerError::Protocol(format!("bad tx monitor msg tag: {e}")))?,
        _ => {
            return Err(N2CServerError::Protocol(
                "invalid tx monitor message".into(),
            ))
        }
    };

    match msg_tag {
        0 => {
            // MsgAcquire → MsgAcquired(slot_no)
            let tip_slot = {
                let handler = query_handler.read().await;
                handler.state().tip.point.slot().map(|s| s.0).unwrap_or(0)
            };
            debug!(tip_slot, "LocalTxMonitor: MsgAcquire");
            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);
            enc.array(2)
                .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
            enc.u32(1)
                .map_err(|e| N2CServerError::Protocol(e.to_string()))?; // MsgAcquired
            enc.u64(tip_slot)
                .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
            Ok(Some(Segment {
                transmission_time: 0,
                protocol_id: MINI_PROTOCOL_TX_MONITOR,
                is_responder: true,
                payload: buf,
            }))
        }
        2 => {
            // MsgRelease
            debug!("LocalTxMonitor: MsgRelease");
            Ok(None)
        }
        3 => {
            // MsgDone
            debug!("LocalTxMonitor: MsgDone");
            Ok(None)
        }
        4 => {
            // MsgHasTx(tx_id) → MsgHasTxReply(bool)
            let tx_id_bytes = decoder.bytes().unwrap_or(&[]);
            let has_tx = if tx_id_bytes.len() == 32 {
                let tx_hash = Hash32::from_bytes(tx_id_bytes.try_into().unwrap());
                let exists = mempool.contains(&tx_hash);
                debug!("LocalTxMonitor: MsgHasTx {} → {exists}", tx_hash.to_hex());
                exists
            } else {
                debug!("LocalTxMonitor: MsgHasTx with invalid tx_id length");
                false
            };

            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);
            enc.array(2)
                .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
            enc.u32(5)
                .map_err(|e| N2CServerError::Protocol(e.to_string()))?; // MsgHasTxReply
            enc.bool(has_tx)
                .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
            Ok(Some(Segment {
                transmission_time: 0,
                protocol_id: MINI_PROTOCOL_TX_MONITOR,
                is_responder: true,
                payload: buf,
            }))
        }
        6 => {
            // MsgNextTx → MsgNextTxReply(null | [era_id, tx_bytes])
            debug!("LocalTxMonitor: MsgNextTx");
            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);

            // Try to return the first transaction from the mempool
            if let Some(tx_hash) = mempool.first_tx_hash() {
                if let Some(tx_cbor) = mempool.get_tx_cbor(&tx_hash) {
                    debug!("LocalTxMonitor: MsgNextTxReply with tx {}", tx_hash);
                    // MsgNextTxReply [7, [era_id, tx_bytes]]
                    enc.array(2)
                        .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
                    enc.u32(7)
                        .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
                    enc.array(2)
                        .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
                    enc.u32(6)
                        .map_err(|e| N2CServerError::Protocol(e.to_string()))?; // era 6 = Conway
                    enc.bytes(&tx_cbor)
                        .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
                } else {
                    // Tx exists but no CBOR — return null
                    enc.array(2)
                        .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
                    enc.u32(7)
                        .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
                    enc.null()
                        .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
                }
            } else {
                // Empty mempool — return null
                enc.array(2)
                    .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
                enc.u32(7)
                    .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
                enc.null()
                    .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
            }
            Ok(Some(Segment {
                transmission_time: 0,
                protocol_id: MINI_PROTOCOL_TX_MONITOR,
                is_responder: true,
                payload: buf,
            }))
        }
        8 => {
            // MsgGetSizes → MsgGetSizesReply([capacity, size, num_txs])
            let num_txs = mempool.len() as u64;
            let size_bytes = mempool.total_bytes() as u64;
            let capacity = mempool.capacity() as u64;
            debug!(
                "LocalTxMonitor: MsgGetSizes → cap={capacity}, size={size_bytes}, txs={num_txs}"
            );

            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);
            enc.array(2)
                .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
            enc.u32(9)
                .map_err(|e| N2CServerError::Protocol(e.to_string()))?; // MsgGetSizesReply
            enc.array(3)
                .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
            enc.u64(capacity)
                .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
            enc.u64(size_bytes)
                .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
            enc.u64(num_txs)
                .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
            Ok(Some(Segment {
                transmission_time: 0,
                protocol_id: MINI_PROTOCOL_TX_MONITOR,
                is_responder: true,
                payload: buf,
            }))
        }
        other => {
            warn!("Unknown LocalTxMonitor message tag: {other}");
            Ok(None)
        }
    }
}

/// Handle LocalChainSync messages
///
/// Protocol flow:
///   Client: MsgFindIntersect(points) → Server: MsgIntersectFound(point, tip) | MsgIntersectNotFound(tip)
///   Client: MsgRequestNext            → Server: MsgRollForward(block, tip) | MsgRollBackward(point, tip) | MsgAwaitReply
///   Client: MsgDone                   → (end)
///
/// Message tags:
///   0: MsgRequestNext
///   1: MsgAwaitReply
///   2: MsgRollForward    [2, wrapped_header, tip]
///   3: MsgRollBackward   [3, point, tip]
///   4: MsgFindIntersect  [4, [point, ...]]
///   5: MsgIntersectFound [5, point, tip]
///   6: MsgIntersectNotFound [6, tip]
///   7: MsgDone
async fn handle_local_chainsync(
    payload: &[u8],
    query_handler: &Arc<RwLock<QueryHandler>>,
    block_provider: &Option<Arc<dyn BlockProvider>>,
    cursor: &mut ChainSyncCursor,
) -> Result<Option<Segment>, N2CServerError> {
    let mut decoder = minicbor::Decoder::new(payload);

    let msg_tag = match decoder.array() {
        Ok(Some(len)) if len >= 1 => decoder
            .u32()
            .map_err(|e| N2CServerError::Protocol(format!("bad chainsync msg tag: {e}")))?,
        Ok(None) => decoder
            .u32()
            .map_err(|e| N2CServerError::Protocol(format!("bad chainsync msg tag: {e}")))?,
        _ => return Err(N2CServerError::Protocol("invalid chainsync message".into())),
    };

    match msg_tag {
        0 => {
            // MsgRequestNext → MsgRollForward or MsgAwaitReply
            if let Some(provider) = block_provider {
                if cursor.has_intersection {
                    if let Some((slot, _hash, cbor)) =
                        provider.get_next_block_after_slot(cursor.cursor_slot)
                    {
                        // Serve the next block
                        debug!(slot, "LocalChainSync: MsgRollForward");
                        cursor.cursor_slot = slot;

                        let (tip_slot, tip_hash, tip_block_no) = provider.get_tip();

                        // Extract era tag from block CBOR: [era_tag, ...]
                        let era_id = {
                            let mut d = minicbor::Decoder::new(&cbor);
                            d.array().ok();
                            d.u32().unwrap_or(6) // default Conway if parse fails
                        };

                        let mut buf = Vec::new();
                        let mut enc = minicbor::Encoder::new(&mut buf);
                        // MsgRollForward [2, [era_id, tagged(24, block_cbor)], tip]
                        enc.array(3)
                            .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
                        enc.u32(2)
                            .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
                        // Wrapped block: [era_id, tag(24) block_bytes]
                        enc.array(2)
                            .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
                        enc.u32(era_id)
                            .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
                        enc.tag(minicbor::data::Tag::new(24))
                            .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
                        enc.bytes(&cbor)
                            .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
                        // tip
                        let tip_h = Hash32::from_bytes(tip_hash);
                        encode_tip(&mut enc, tip_slot, &tip_h, tip_block_no)?;

                        return Ok(Some(Segment {
                            transmission_time: 0,
                            protocol_id: MINI_PROTOCOL_CHAINSYNC,
                            is_responder: true,
                            payload: buf,
                        }));
                    }
                }
            }

            // No blocks available or no block provider — await
            debug!("LocalChainSync: MsgRequestNext → MsgAwaitReply");
            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);
            enc.array(1)
                .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
            enc.u32(1)
                .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
            Ok(Some(Segment {
                transmission_time: 0,
                protocol_id: MINI_PROTOCOL_CHAINSYNC,
                is_responder: true,
                payload: buf,
            }))
        }
        4 => {
            // MsgFindIntersect(points) → MsgIntersectFound(point, tip) or MsgIntersectNotFound(tip)
            debug!("LocalChainSync: MsgFindIntersect");
            let handler = query_handler.read().await;
            let state = handler.state();
            let tip_slot = state.tip.point.slot().map(|s| s.0).unwrap_or(0);
            let tip_hash = state.tip.point.hash().copied().unwrap_or(Hash32::ZERO);
            let tip_block_no = state.block_number.0;

            // Try to find an intersection with the client's points
            let found_point = if let Some(provider) = block_provider {
                // Check each client point against our chain
                parse_client_points_with_provider(&mut decoder, provider)
            } else {
                // Fallback: check if any point matches our current tip
                parse_client_points(&mut decoder, tip_slot, &tip_hash)
            };

            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);

            if let Some((slot, hash)) = found_point {
                debug!(slot, "LocalChainSync: MsgIntersectFound");
                cursor.cursor_slot = slot;
                cursor.has_intersection = true;
                // MsgIntersectFound [5, point, tip]
                enc.array(3)
                    .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
                enc.u32(5)
                    .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
                encode_point(&mut enc, slot, &hash)?;
                encode_tip(&mut enc, tip_slot, &tip_hash, tip_block_no)?;
            } else {
                debug!("LocalChainSync: MsgIntersectNotFound");
                cursor.has_intersection = false;
                // MsgIntersectNotFound [6, tip]
                enc.array(2)
                    .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
                enc.u32(6)
                    .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
                encode_tip(&mut enc, tip_slot, &tip_hash, tip_block_no)?;
            }

            Ok(Some(Segment {
                transmission_time: 0,
                protocol_id: MINI_PROTOCOL_CHAINSYNC,
                is_responder: true,
                payload: buf,
            }))
        }
        7 => {
            // MsgDone
            debug!("LocalChainSync: MsgDone");
            cursor.has_intersection = false;
            Ok(None)
        }
        other => {
            warn!("Unknown LocalChainSync message tag: {other}");
            Ok(None)
        }
    }
}

/// Parse client points from MsgFindIntersect and check if any match our tip
fn parse_client_points(
    decoder: &mut minicbor::Decoder,
    tip_slot: u64,
    tip_hash: &Hash32,
) -> Option<(u64, Hash32)> {
    let arr_len = decoder.array().ok()??;
    for _ in 0..arr_len {
        // Each point is either [slot, hash] or "origin" (encoded as array of 0 elements)
        if let Ok(Some(point_len)) = decoder.array() {
            if point_len == 2 {
                if let (Ok(slot), Ok(hash_bytes)) = (decoder.u64(), decoder.bytes()) {
                    if slot == tip_slot && hash_bytes.len() == 32 {
                        let point_hash = Hash32::from_bytes(hash_bytes.try_into().unwrap());
                        if point_hash == *tip_hash {
                            return Some((slot, point_hash));
                        }
                    }
                    continue;
                }
            } else if point_len == 0 {
                // Origin point
                continue;
            }
        }
        // Skip malformed point
        let _ = decoder.skip();
    }
    None
}

/// Parse client points and check if any exist on our chain (via block provider).
/// Returns the first matching point (highest priority = first in list).
fn parse_client_points_with_provider(
    decoder: &mut minicbor::Decoder,
    provider: &Arc<dyn BlockProvider>,
) -> Option<(u64, Hash32)> {
    let arr_len = decoder.array().ok()??;
    for _ in 0..arr_len {
        if let Ok(Some(point_len)) = decoder.array() {
            if point_len == 2 {
                if let (Ok(slot), Ok(hash_bytes)) = (decoder.u64(), decoder.bytes()) {
                    if hash_bytes.len() == 32 {
                        let mut hash_arr = [0u8; 32];
                        hash_arr.copy_from_slice(hash_bytes);
                        // Check if this block exists on our chain
                        if provider.has_block(&hash_arr) {
                            let point_hash = Hash32::from_bytes(hash_arr);
                            return Some((slot, point_hash));
                        }
                    }
                    continue;
                }
            } else if point_len == 0 {
                // Origin point — always matches
                return Some((0, Hash32::ZERO));
            }
        }
        let _ = decoder.skip();
    }
    None
}

/// Encode a point as [slot, hash]
fn encode_point(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    slot: u64,
    hash: &Hash32,
) -> Result<(), N2CServerError> {
    enc.array(2)
        .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
    enc.u64(slot)
        .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
    enc.bytes(hash.as_bytes())
        .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
    Ok(())
}

/// Encode tip as [[slot, hash], block_no]
fn encode_tip(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    slot: u64,
    hash: &Hash32,
    block_no: u64,
) -> Result<(), N2CServerError> {
    enc.array(2)
        .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
    encode_point(enc, slot, hash)?;
    enc.u64(block_no)
        .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
    Ok(())
}

/// Extract transaction CBOR bytes from a MsgSubmitTx payload
fn extract_submitted_tx(decoder: &mut minicbor::Decoder) -> Option<(u16, Vec<u8>)> {
    // The structure after the tag is: [era_id, tx_bytes]
    // era_id is a u16, tx_bytes is CBOR bytes
    let _ = decoder.array().ok()?;
    let era_id = decoder.u32().ok()? as u16;
    // The tx is encoded as a CBOR byte string containing the serialized transaction
    let tx_bytes = decoder.bytes().ok()?;
    Some((era_id, tx_bytes.to_vec()))
}

/// Encode MsgAcceptTx response: [1]
fn encode_tx_accept() -> Result<Option<Segment>, N2CServerError> {
    let mut buf = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut buf);
    enc.array(1)
        .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
    enc.u32(1)
        .map_err(|e| N2CServerError::Protocol(e.to_string()))?;

    Ok(Some(Segment {
        transmission_time: 0,
        protocol_id: MINI_PROTOCOL_TX_SUBMISSION,
        is_responder: true,
        payload: buf,
    }))
}

/// Encode MsgRejectTx response: [2, [reason_tag, reason_text]]
fn encode_tx_reject(reason: &str) -> Result<Option<Segment>, N2CServerError> {
    let mut buf = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut buf);
    enc.array(2)
        .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
    enc.u32(2)
        .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
    // Rejection reason as an array with a text description
    enc.array(1)
        .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
    enc.str(reason)
        .map_err(|e| N2CServerError::Protocol(e.to_string()))?;

    Ok(Some(Segment {
        transmission_time: 0,
        protocol_id: MINI_PROTOCOL_TX_SUBMISSION,
        is_responder: true,
        payload: buf,
    }))
}

/// Handle LocalStateQuery messages
///
/// Protocol flow:
///   Client: MsgAcquire(point) → Server: MsgAcquired
///   Client: MsgQuery(query)   → Server: MsgResult(result)
///   Client: MsgRelease        → (back to idle)
///   Client: MsgDone           → (end)
async fn handle_state_query(
    payload: &[u8],
    query_handler: &Arc<RwLock<QueryHandler>>,
) -> Result<Option<Segment>, N2CServerError> {
    let mut decoder = minicbor::Decoder::new(payload);

    // Parse the CBOR message tag
    let msg_tag = match decoder.array() {
        Ok(Some(len)) if len >= 1 => decoder
            .u32()
            .map_err(|e| N2CServerError::Protocol(format!("bad msg tag: {e}")))?,
        Ok(None) => {
            // Indefinite length array
            decoder
                .u32()
                .map_err(|e| N2CServerError::Protocol(format!("bad msg tag: {e}")))?
        }
        _ => {
            return Err(N2CServerError::Protocol(
                "invalid state query message".into(),
            ))
        }
    };

    match msg_tag {
        0 => {
            // MsgAcquire(point)
            debug!("LocalStateQuery: MsgAcquire");
            // Respond with MsgAcquired [1]
            let mut resp = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut resp);
            enc.array(1)
                .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
            enc.u32(1)
                .map_err(|e| N2CServerError::Protocol(e.to_string()))?; // MsgAcquired
            Ok(Some(Segment {
                transmission_time: 0,
                protocol_id: MINI_PROTOCOL_STATE_QUERY,
                is_responder: true,
                payload: resp,
            }))
        }
        3 => {
            // MsgQuery(query)
            debug!("LocalStateQuery: MsgQuery");
            let handler = query_handler.read().await;
            let result = handler.handle_query_cbor(payload);
            let response_cbor = encode_query_result(&result);

            Ok(Some(Segment {
                transmission_time: 0,
                protocol_id: MINI_PROTOCOL_STATE_QUERY,
                is_responder: true,
                payload: response_cbor,
            }))
        }
        5 => {
            // MsgReAcquire(point)
            debug!("LocalStateQuery: MsgReAcquire");
            // Respond with MsgAcquired [1]
            let mut resp = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut resp);
            enc.array(1)
                .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
            enc.u32(1)
                .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
            Ok(Some(Segment {
                transmission_time: 0,
                protocol_id: MINI_PROTOCOL_STATE_QUERY,
                is_responder: true,
                payload: resp,
            }))
        }
        7 => {
            // MsgRelease
            debug!("LocalStateQuery: MsgRelease");
            Ok(None)
        }
        9 => {
            // MsgDone
            debug!("LocalStateQuery: MsgDone");
            Ok(None)
        }
        other => {
            warn!("Unknown LocalStateQuery message tag: {other}");
            Ok(None)
        }
    }
}

/// Encode a QueryResult into a MsgResult CBOR response
/// Encode a UTxO output in PostAlonzo format (CBOR map with integer keys).
///
/// Format: {0: address_bytes, 1: value, 2?: datum_option, 3?: script_ref}
/// Value: coin (integer) or [coin, {policy_id -> {asset_name -> quantity}}]
fn encode_utxo_output(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    utxo: &crate::query_handler::UtxoSnapshot,
) {
    let has_datum = utxo.datum_hash.is_some();
    let field_count = 2 + has_datum as u64; // address + value + optional datum
    enc.map(field_count).ok();

    // 0: address (raw bytes)
    enc.u32(0).ok();
    enc.bytes(&utxo.address_bytes).ok();

    // 1: value
    enc.u32(1).ok();
    if utxo.multi_asset.is_empty() {
        // Coin-only: encode as plain integer
        enc.u64(utxo.lovelace).ok();
    } else {
        // Multi-asset: [coin, {policy_id -> {asset_name -> quantity}}]
        enc.array(2).ok();
        enc.u64(utxo.lovelace).ok();
        enc.map(utxo.multi_asset.len() as u64).ok();
        for (policy_id, assets) in &utxo.multi_asset {
            enc.bytes(policy_id).ok();
            enc.map(assets.len() as u64).ok();
            for (asset_name, quantity) in assets {
                enc.bytes(asset_name).ok();
                enc.u64(*quantity).ok();
            }
        }
    }

    // 2: datum_option (if present)
    if let Some(ref datum_hash) = utxo.datum_hash {
        enc.u32(2).ok();
        // DatumOption::Hash variant: [0, datum_hash]
        enc.array(2).ok();
        enc.u32(0).ok();
        enc.bytes(datum_hash).ok();
    }
}

/// Encode protocol parameters as a positional CBOR array(31) per Haskell ConwayPParams.
///
/// The Haskell reference uses `encCBOR` which encodes PParams as a flat positional array,
/// NOT a map. Field order matches `eraPParams @ConwayEra`:
///   [0] txFeePerByte, [1] txFeeFixed, [2] maxBBSize, [3] maxTxSize,
///   [4] maxBHSize, [5] keyDeposit, [6] poolDeposit, [7] eMax, [8] nOpt,
///   [9] a0, [10] rho, [11] tau, [12] protocolVersion,
///   [13] minPoolCost, [14] coinsPerUTxOByte, [15] costModels,
///   [16] prices, [17] maxTxExUnits, [18] maxBlockExUnits,
///   [19] maxValSize, [20] collateralPercentage, [21] maxCollateralInputs,
///   [22] poolVotingThresholds(5), [23] drepVotingThresholds(10),
///   [24] committeeMinSize, [25] committeeMaxTermLength, [26] govActionLifetime,
///   [27] govActionDeposit, [28] drepDeposit, [29] drepActivity,
///   [30] minFeeRefScriptCostPerByte
fn encode_protocol_params_cbor(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    pp: &ProtocolParamsSnapshot,
) {
    enc.array(31).ok();

    // [0] txFeePerByte (min_fee_a)
    enc.u64(pp.min_fee_a).ok();
    // [1] txFeeFixed (min_fee_b)
    enc.u64(pp.min_fee_b).ok();
    // [2] maxBlockBodySize
    enc.u64(pp.max_block_body_size).ok();
    // [3] maxTxSize
    enc.u64(pp.max_tx_size).ok();
    // [4] maxBlockHeaderSize
    enc.u64(pp.max_block_header_size).ok();
    // [5] keyDeposit
    enc.u64(pp.key_deposit).ok();
    // [6] poolDeposit
    enc.u64(pp.pool_deposit).ok();
    // [7] eMax
    enc.u64(pp.e_max).ok();
    // [8] nOpt
    enc.u64(pp.n_opt).ok();

    // [9] a0 (rational as tag 30)
    encode_tagged_rational(enc, pp.a0_num, pp.a0_den);
    // [10] rho
    encode_tagged_rational(enc, pp.rho_num, pp.rho_den);
    // [11] tau
    encode_tagged_rational(enc, pp.tau_num, pp.tau_den);

    // [12] protocolVersion [major, minor]
    enc.array(2).ok();
    enc.u64(pp.protocol_version_major).ok();
    enc.u64(pp.protocol_version_minor).ok();

    // [13] minPoolCost
    enc.u64(pp.min_pool_cost).ok();
    // [14] coinsPerUTxOByte
    enc.u64(pp.ada_per_utxo_byte).ok();

    // [15] costModels (map: {0: [v1], 1: [v2], 2: [v3]})
    {
        let cm_count = pp.cost_models_v1.is_some() as u64
            + pp.cost_models_v2.is_some() as u64
            + pp.cost_models_v3.is_some() as u64;
        enc.map(cm_count).ok();
        if let Some(ref v1) = pp.cost_models_v1 {
            enc.u32(0).ok();
            enc.array(v1.len() as u64).ok();
            for cost in v1 {
                enc.i64(*cost).ok();
            }
        }
        if let Some(ref v2) = pp.cost_models_v2 {
            enc.u32(1).ok();
            enc.array(v2.len() as u64).ok();
            for cost in v2 {
                enc.i64(*cost).ok();
            }
        }
        if let Some(ref v3) = pp.cost_models_v3 {
            enc.u32(2).ok();
            enc.array(v3.len() as u64).ok();
            for cost in v3 {
                enc.i64(*cost).ok();
            }
        }
    }

    // [16] prices [mem_price, step_price] as tagged rationals
    enc.array(2).ok();
    encode_tagged_rational(enc, pp.execution_costs_mem_num, pp.execution_costs_mem_den);
    encode_tagged_rational(
        enc,
        pp.execution_costs_step_num,
        pp.execution_costs_step_den,
    );

    // [17] maxTxExUnits [mem, steps]
    enc.array(2).ok();
    enc.u64(pp.max_tx_ex_mem).ok();
    enc.u64(pp.max_tx_ex_steps).ok();

    // [18] maxBlockExUnits [mem, steps]
    enc.array(2).ok();
    enc.u64(pp.max_block_ex_mem).ok();
    enc.u64(pp.max_block_ex_steps).ok();

    // [19] maxValSize
    enc.u64(pp.max_val_size).ok();
    // [20] collateralPercentage
    enc.u64(pp.collateral_percentage).ok();
    // [21] maxCollateralInputs
    enc.u64(pp.max_collateral_inputs).ok();

    // [22] poolVotingThresholds (5 tagged rationals)
    enc.array(5).ok();
    encode_tagged_rational(
        enc,
        pp.pvt_motion_no_confidence_num,
        pp.pvt_motion_no_confidence_den,
    );
    encode_tagged_rational(
        enc,
        pp.pvt_committee_normal_num,
        pp.pvt_committee_normal_den,
    );
    encode_tagged_rational(
        enc,
        pp.pvt_committee_no_confidence_num,
        pp.pvt_committee_no_confidence_den,
    );
    encode_tagged_rational(enc, pp.pvt_hard_fork_num, pp.pvt_hard_fork_den);
    encode_tagged_rational(
        enc,
        pp.pvt_pp_security_group_num,
        pp.pvt_pp_security_group_den,
    );

    // [23] drepVotingThresholds (10 tagged rationals)
    enc.array(10).ok();
    encode_tagged_rational(enc, pp.dvt_no_confidence_num, pp.dvt_no_confidence_den);
    encode_tagged_rational(
        enc,
        pp.dvt_committee_normal_num,
        pp.dvt_committee_normal_den,
    );
    encode_tagged_rational(
        enc,
        pp.dvt_committee_no_confidence_num,
        pp.dvt_committee_no_confidence_den,
    );
    encode_tagged_rational(enc, pp.dvt_constitution_num, pp.dvt_constitution_den);
    encode_tagged_rational(enc, pp.dvt_hard_fork_num, pp.dvt_hard_fork_den);
    encode_tagged_rational(
        enc,
        pp.dvt_pp_network_group_num,
        pp.dvt_pp_network_group_den,
    );
    encode_tagged_rational(
        enc,
        pp.dvt_pp_economic_group_num,
        pp.dvt_pp_economic_group_den,
    );
    encode_tagged_rational(
        enc,
        pp.dvt_pp_technical_group_num,
        pp.dvt_pp_technical_group_den,
    );
    encode_tagged_rational(enc, pp.dvt_pp_gov_group_num, pp.dvt_pp_gov_group_den);
    encode_tagged_rational(
        enc,
        pp.dvt_treasury_withdrawal_num,
        pp.dvt_treasury_withdrawal_den,
    );

    // [24] committeeMinSize
    enc.u64(pp.committee_min_size).ok();
    // [25] committeeMaxTermLength
    enc.u64(pp.committee_max_term_length).ok();
    // [26] govActionLifetime
    enc.u64(pp.gov_action_lifetime).ok();
    // [27] govActionDeposit
    enc.u64(pp.gov_action_deposit).ok();
    // [28] drepDeposit
    enc.u64(pp.drep_deposit).ok();
    // [29] drepActivity
    enc.u64(pp.drep_activity).ok();

    // [30] minFeeRefScriptCostPerByte
    encode_tagged_rational(enc, pp.min_fee_ref_script_cost_per_byte, 1);
}

/// Helper to encode a tagged rational number: tag(30)[numerator, denominator]
fn encode_tagged_rational(enc: &mut minicbor::Encoder<&mut Vec<u8>>, num: u64, den: u64) {
    enc.tag(minicbor::data::Tag::new(30)).ok();
    enc.array(2).ok();
    enc.u64(num).ok();
    enc.u64(den).ok();
}

/// Encode a GovAction as a CBOR sum type tag.
/// We encode a simplified version since we only have the action type string.
fn encode_gov_action_tag(enc: &mut minicbor::Encoder<&mut Vec<u8>>, action_type: &str) {
    match action_type {
        "ParameterChange" => {
            // [0, prev_action_id, params, policy_hash]
            enc.array(4).ok();
            enc.u32(0).ok();
            enc.null().ok(); // prev action id
            enc.map(0).ok(); // empty params update
            enc.null().ok(); // policy hash
        }
        "HardForkInitiation" => {
            // [1, prev_action_id, protocol_version]
            enc.array(3).ok();
            enc.u32(1).ok();
            enc.null().ok();
            enc.array(2).ok();
            enc.u64(0).ok();
            enc.u64(0).ok();
        }
        "TreasuryWithdrawals" => {
            // [2, withdrawals_map, policy_hash]
            enc.array(3).ok();
            enc.u32(2).ok();
            enc.map(0).ok();
            enc.null().ok();
        }
        "NoConfidence" => {
            // [3, prev_action_id]
            enc.array(2).ok();
            enc.u32(3).ok();
            enc.null().ok();
        }
        "UpdateCommittee" => {
            // [4, prev_action_id, remove_set, add_map, quorum]
            enc.array(5).ok();
            enc.u32(4).ok();
            enc.null().ok();
            enc.tag(minicbor::data::Tag::new(258)).ok();
            enc.array(0).ok();
            enc.map(0).ok();
            encode_tagged_rational(enc, 2, 3);
        }
        "NewConstitution" => {
            // [5, prev_action_id, constitution]
            enc.array(3).ok();
            enc.u32(5).ok();
            enc.null().ok();
            enc.array(2).ok();
            enc.array(2).ok();
            enc.str("").ok();
            enc.bytes(&[0u8; 32]).ok();
            enc.null().ok();
        }
        _ => {
            // [6]
            enc.array(1).ok();
            enc.u32(6).ok();
        }
    }
}

fn encode_relay_cbor(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    relay: &crate::query_handler::RelaySnapshot,
) {
    use crate::query_handler::RelaySnapshot;
    match relay {
        RelaySnapshot::SingleHostAddr { port, ipv4, ipv6 } => {
            enc.array(4).ok();
            enc.u32(0).ok();
            match port {
                Some(p) => {
                    enc.u16(*p).ok();
                }
                None => {
                    enc.null().ok();
                }
            }
            match ipv4 {
                Some(ip) => {
                    enc.bytes(ip).ok();
                }
                None => {
                    enc.null().ok();
                }
            }
            match ipv6 {
                Some(ip) => {
                    enc.bytes(ip).ok();
                }
                None => {
                    enc.null().ok();
                }
            }
        }
        RelaySnapshot::SingleHostName { port, dns_name } => {
            enc.array(3).ok();
            enc.u32(1).ok();
            match port {
                Some(p) => {
                    enc.u16(*p).ok();
                }
                None => {
                    enc.null().ok();
                }
            }
            enc.str(dns_name).ok();
        }
        RelaySnapshot::MultiHostName { dns_name } => {
            enc.array(2).ok();
            enc.u32(2).ok();
            enc.str(dns_name).ok();
        }
    }
}

/// Parse an ISO-8601 UTC timestamp to (year, dayOfYear, picosecondsOfDay).
/// Input format: "2022-04-01T00:00:00Z" or similar.
fn parse_utctime(s: &str) -> (u64, u64, u64) {
    // Try to parse "YYYY-MM-DDThh:mm:ssZ"
    let s = s.trim_end_matches('Z');
    let parts: Vec<&str> = s.split('T').collect();
    if parts.len() != 2 {
        return (2017, 266, 0); // fallback: mainnet system start
    }
    let date_parts: Vec<u64> = parts[0].split('-').filter_map(|p| p.parse().ok()).collect();
    let time_parts: Vec<u64> = parts[1].split(':').filter_map(|p| p.parse().ok()).collect();

    if date_parts.len() < 3 || time_parts.len() < 3 {
        return (2017, 266, 0);
    }

    let (year, month, day) = (date_parts[0], date_parts[1], date_parts[2]);

    // Calculate day of year
    let days_in_months: [u64; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let is_leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
    let mut day_of_year = day;
    for (i, &days) in days_in_months.iter().enumerate().take((month - 1) as usize) {
        day_of_year += days;
        if i == 1 && is_leap {
            day_of_year += 1;
        }
    }

    // Picoseconds of day
    let picos = (time_parts[0] * 3600 + time_parts[1] * 60 + time_parts[2]) * 1_000_000_000_000;

    (year, day_of_year, picos)
}

/// Encode legacy Shelley PParams as array(18) (N2C V16-V20 legacy format).
fn encode_shelley_pparams(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    pp: &crate::query_handler::ShelleyPParamsSnapshot,
) {
    enc.array(18).ok();
    enc.u64(pp.min_fee_a).ok(); // [0] txFeePerByte
    enc.u64(pp.min_fee_b).ok(); // [1] txFeeFixed
    enc.u32(pp.max_block_body_size).ok(); // [2] maxBBSize
    enc.u32(pp.max_tx_size).ok(); // [3] maxTxSize
    enc.u16(pp.max_block_header_size).ok(); // [4] maxBHSize
    enc.u64(pp.key_deposit).ok(); // [5] keyDeposit
    enc.u64(pp.pool_deposit).ok(); // [6] poolDeposit
    enc.u32(pp.e_max).ok(); // [7] eMax
    enc.u16(pp.n_opt).ok(); // [8] nOpt
    encode_tagged_rational(enc, pp.a0_num, pp.a0_den); // [9] a0
    encode_tagged_rational(enc, pp.rho_num, pp.rho_den); // [10] rho
    encode_tagged_rational(enc, pp.tau_num, pp.tau_den); // [11] tau
    encode_tagged_rational(enc, pp.d_num, pp.d_den); // [12] d (decentralization)
                                                     // [13] extraEntropy: NeutralNonce = [0]
    enc.array(1).ok();
    enc.u32(0).ok();
    // [14] protocolVersion major
    enc.u64(pp.protocol_version_major).ok();
    // [15] protocolVersion minor
    enc.u64(pp.protocol_version_minor).ok();
    // [16] minUTxOValue
    enc.u64(pp.min_utxo_value).ok();
    // [17] minPoolCost
    enc.u64(pp.min_pool_cost).ok();
}

fn encode_query_result(result: &QueryResult) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut buf);

    // MsgResult [4, result]
    // For BlockQuery (era-specific) results: [4, [result]]  (HFC success wrapper)
    // For QueryAnytime/QueryHardFork results: [4, result]   (no wrapper)
    enc.array(2).ok();
    enc.u32(4).ok(); // MsgResult tag

    // Determine if this is a BlockQuery result that needs HFC wrapping.
    // QueryAnytime (CurrentEra, SystemStart) and QueryHardFork (ChainBlockNo, ChainTip)
    // do NOT get the HFC wrapper. Only BlockQuery (Shelley/Conway) results DO.
    let needs_hfc_wrapper = !matches!(
        result,
        QueryResult::CurrentEra(_)
            | QueryResult::SystemStart(_)
            | QueryResult::ChainBlockNo(_)
            | QueryResult::ChainTip { .. }
    );

    if needs_hfc_wrapper {
        enc.array(1).ok(); // HFC success wrapper: array(1) = Right
    }

    match result {
        QueryResult::EpochNo(epoch) => {
            enc.u64(*epoch).ok();
        }
        QueryResult::ChainTip {
            slot,
            hash,
            block_no,
        } => {
            enc.array(2).ok();
            // Point: [slot, hash]
            enc.array(2).ok();
            enc.u64(*slot).ok();
            enc.bytes(hash).ok();
            // Block number
            enc.u64(*block_no).ok();
        }
        QueryResult::CurrentEra(era) => {
            enc.u32(*era).ok();
        }
        QueryResult::SystemStart(time_str) => {
            enc.str(time_str).ok();
        }
        QueryResult::ChainBlockNo(block_no) => {
            enc.u64(*block_no).ok();
        }
        QueryResult::ProtocolParams(pp) => {
            encode_protocol_params_cbor(&mut enc, pp);
        }
        QueryResult::StakeDistribution(pools) => {
            // Wire format: Map<pool_hash(28), IndividualPoolStake>
            // IndividualPoolStake: array(2) [tag(30)[num,den], vrf_hash(32)]
            enc.map(pools.len() as u64).ok();
            for pool in pools {
                enc.bytes(&pool.pool_id).ok();
                enc.array(2).ok();
                // Stake fraction as tagged rational
                let total = pool.total_active_stake.max(1); // avoid div by zero
                encode_tagged_rational(&mut enc, pool.stake, total);
                enc.bytes(&pool.vrf_keyhash).ok();
            }
        }
        QueryResult::GovState(gov) => {
            // ConwayGovState = array(7):
            //   [0] Proposals, [1] Committee, [2] Constitution,
            //   [3] curPParams, [4] prevPParams, [5] FuturePParams,
            //   [6] DRepPulsingState
            enc.array(7).ok();

            // [0] Proposals = array(2) [roots, values]
            enc.array(2).ok();
            // roots = array(4) of StrictMaybe GovPurposeId (empty for now)
            enc.array(4).ok();
            for _ in 0..4 {
                enc.array(0).ok(); // StrictMaybe Nothing = array(0)
            }
            // values = array(n) of GovActionState
            enc.array(gov.proposals.len() as u64).ok();
            for p in &gov.proposals {
                // GovActionState = array(7)
                //   [0] gasId, [1] committeeVotes, [2] drepVotes,
                //   [3] spoVotes, [4] procedure, [5] proposedIn, [6] expiresAfter
                enc.array(7).ok();
                // [0] GovActionId = array(2) [tx_hash, action_index]
                enc.array(2).ok();
                enc.bytes(&p.tx_id).ok();
                enc.u32(p.action_index).ok();
                // [1] committeeVotes = Map<Credential, Vote> (empty for now)
                enc.map(0).ok();
                // [2] drepVotes = Map<Credential, Vote> (empty for now)
                enc.map(0).ok();
                // [3] spoVotes = Map<Credential, Vote> (empty for now)
                enc.map(0).ok();
                // [4] ProposalProcedure = array(4) [deposit, return_addr, gov_action, anchor]
                enc.array(4).ok();
                enc.u64(p.deposit).ok();
                enc.bytes(&p.return_addr).ok();
                // gov_action = sum type tagged by action type
                encode_gov_action_tag(&mut enc, &p.action_type);
                // anchor = array(2) [url, hash]
                enc.array(2).ok();
                enc.str(&p.anchor_url).ok();
                enc.bytes(&p.anchor_hash).ok();
                // [5] proposedIn (EpochNo)
                enc.u64(p.proposed_epoch).ok();
                // [6] expiresAfter (EpochNo)
                enc.u64(p.expires_epoch).ok();
            }

            // [1] Committee = StrictMaybe(array(2) [Map<ColdCred,EpochNo>, UnitInterval])
            if gov.committee.members.is_empty() && gov.committee.threshold.is_none() {
                enc.array(0).ok(); // StrictMaybe Nothing
            } else {
                enc.array(1).ok(); // StrictMaybe Just
                enc.array(2).ok();
                // Map<ColdCredential, EpochNo>
                enc.map(gov.committee.members.len() as u64).ok();
                for m in &gov.committee.members {
                    // Key: Credential [type, hash]
                    enc.array(2).ok();
                    enc.u8(m.cold_credential_type).ok();
                    enc.bytes(&m.cold_credential).ok();
                    // Value: expiry epoch
                    enc.u64(m.expiry_epoch.unwrap_or(0)).ok();
                }
                // UnitInterval (quorum threshold)
                if let Some((num, den)) = gov.committee.threshold {
                    encode_tagged_rational(&mut enc, num, den);
                } else {
                    encode_tagged_rational(&mut enc, 2, 3); // default 2/3
                }
            }

            // [2] Constitution = array(2) [Anchor, StrictMaybe ScriptHash]
            enc.array(2).ok();
            // Anchor = array(2) [url, hash]
            enc.array(2).ok();
            enc.str(&gov.constitution_url).ok();
            enc.bytes(&gov.constitution_hash).ok();
            // StrictMaybe ScriptHash (null-encoded: null=Nothing, bytes=Just)
            if let Some(ref script) = gov.constitution_script {
                enc.bytes(script).ok();
            } else {
                enc.null().ok();
            }

            // [3] curPParams = array(31)
            encode_protocol_params_cbor(&mut enc, &gov.cur_pparams);

            // [4] prevPParams = array(31)
            encode_protocol_params_cbor(&mut enc, &gov.prev_pparams);

            // [5] FuturePParams = Sum: [0] = NoPParamsUpdate
            enc.array(1).ok();
            enc.u32(0).ok();

            // [6] DRepPulsingState = DRComplete: array(2) [PulsingSnapshot(4), RatifyState(4)]
            enc.array(2).ok();
            // PulsingSnapshot = array(4) [Map<DRep,Coin>, Map<Credential,Vote>, Map<GASId,Gas>, Map<Pool,IndivPoolStake>]
            enc.array(4).ok();
            enc.map(0).ok(); // drep stake distribution
            enc.map(0).ok(); // drep votes (credential→vote)
            enc.map(0).ok(); // proposals map
            enc.map(0).ok(); // pool stake distribution
                             // RatifyState = array(4) [enacted, expired, delayed_flag, future_pparams]
            enc.array(4).ok();
            // enacted proposals (tag(258) set)
            enc.tag(minicbor::data::Tag::new(258)).ok();
            enc.array(0).ok();
            // expired proposals (tag(258) set)
            enc.tag(minicbor::data::Tag::new(258)).ok();
            enc.array(0).ok();
            // delayed flag (bool)
            enc.bool(false).ok();
            // future pparams: [0] = NoPParamsUpdate
            enc.array(1).ok();
            enc.u32(0).ok();
        }
        QueryResult::DRepState(dreps) => {
            // Wire format: Map<Credential, DRepState>
            //   Credential: [0|1, hash(28)]
            //   DRepState: array(4) [expiry, maybe_anchor, deposit, tag(258)[delegators]]
            enc.map(dreps.len() as u64).ok();
            for drep in dreps {
                // Key: Credential
                enc.array(2).ok();
                enc.u8(drep.credential_type).ok();
                enc.bytes(&drep.credential_hash).ok();
                // Value: DRepState array(4)
                enc.array(4).ok();
                // [0] drepExpiry (EpochNo)
                enc.u64(drep.expiry_epoch).ok();
                // [1] drepAnchor (StrictMaybe Anchor)
                if let (Some(url), Some(hash)) = (&drep.anchor_url, &drep.anchor_hash) {
                    enc.array(1).ok(); // SJust
                    enc.array(2).ok(); // Anchor
                    enc.str(url).ok();
                    enc.bytes(hash).ok();
                } else {
                    enc.array(0).ok(); // SNothing
                }
                // [2] drepDeposit (Coin)
                enc.u64(drep.deposit).ok();
                // [3] drepDelegs: tag(258) Set of Credential
                enc.tag(minicbor::data::Tag::new(258)).ok();
                enc.array(drep.delegator_hashes.len() as u64).ok();
                for dh in &drep.delegator_hashes {
                    enc.array(2).ok();
                    enc.u8(0).ok(); // KeyHashObj
                    enc.bytes(dh).ok();
                }
            }
        }
        QueryResult::CommitteeState(committee) => {
            // Wire format: array(3) [map_members, maybe_threshold, epoch]
            enc.array(3).ok();
            // [0] Map<ColdCredential, CommitteeMemberState>
            enc.map(committee.members.len() as u64).ok();
            for member in &committee.members {
                // Key: Credential [type, hash(28)]
                enc.array(2).ok();
                enc.u8(member.cold_credential_type).ok();
                enc.bytes(&member.cold_credential).ok();
                // Value: CommitteeMemberState array(4)
                enc.array(4).ok();
                // [0] HotCredAuthStatus (Sum type)
                match member.hot_status {
                    0 => {
                        // MemberAuthorized: [0, credential]
                        enc.array(2).ok();
                        enc.u32(0).ok();
                        if let Some(hot) = &member.hot_credential {
                            enc.array(2).ok();
                            enc.u8(0).ok(); // KeyHashObj
                            enc.bytes(hot).ok();
                        }
                    }
                    1 => {
                        // MemberNotAuthorized: [1]
                        enc.array(1).ok();
                        enc.u32(1).ok();
                    }
                    _ => {
                        // MemberResigned: [2, maybe_anchor]
                        enc.array(2).ok();
                        enc.u32(2).ok();
                        enc.array(0).ok(); // SNothing anchor
                    }
                }
                // [1] MemberStatus enum (0=Active, 1=Expired, 2=Unrecognized)
                enc.u8(member.member_status).ok();
                // [2] Maybe EpochNo (expiration)
                if let Some(exp) = member.expiry_epoch {
                    enc.array(1).ok();
                    enc.u64(exp).ok();
                } else {
                    enc.array(0).ok();
                }
                // [3] NextEpochChange: NoChangeExpected [2]
                enc.array(1).ok();
                enc.u32(2).ok();
            }
            // [1] Maybe UnitInterval (threshold)
            if let Some((num, den)) = committee.threshold {
                enc.array(1).ok();
                encode_tagged_rational(&mut enc, num, den);
            } else {
                enc.array(0).ok();
            }
            // [2] Current epoch
            enc.u64(committee.current_epoch).ok();
        }
        QueryResult::UtxoByAddress(utxos) => {
            // Cardano wire format: Map<[tx_hash, index], TransactionOutput>
            enc.map(utxos.len() as u64).ok();
            for utxo in utxos {
                // Key: [tx_hash, index]
                enc.array(2).ok();
                enc.bytes(&utxo.tx_hash).ok();
                enc.u32(utxo.output_index).ok();

                // Value: PostAlonzo TransactionOutput as CBOR map {0: addr, 1: value, ...}
                encode_utxo_output(&mut enc, utxo);
            }
        }
        QueryResult::StakeAddressInfo(addrs) => {
            // Wire format: array(2) [delegations_map, rewards_map]
            // delegations_map: Map<Credential, pool_hash(28)>
            // rewards_map: Map<Credential, Coin>
            // Credential: [0, hash(28)] for KeyHash
            let delegated: Vec<_> = addrs
                .iter()
                .filter(|a| a.delegated_pool.is_some())
                .collect();
            enc.array(2).ok();
            // Delegations map
            enc.map(delegated.len() as u64).ok();
            for addr in &delegated {
                // Credential key
                enc.array(2).ok();
                enc.u32(0).ok(); // KeyHashObj
                enc.bytes(&addr.credential_hash).ok();
                // Pool hash value
                enc.bytes(addr.delegated_pool.as_ref().unwrap()).ok();
            }
            // Rewards map
            enc.map(addrs.len() as u64).ok();
            for addr in addrs {
                // Credential key
                enc.array(2).ok();
                enc.u32(0).ok(); // KeyHashObj
                enc.bytes(&addr.credential_hash).ok();
                // Reward balance value
                enc.u64(addr.reward_balance).ok();
            }
        }
        QueryResult::StakeSnapshots(snapshots) => {
            // Wire format: array(4) [pool_map, mark_total, set_total, go_total]
            // pool_map: Map<pool_hash(28), array(3) [mark_stake, set_stake, go_stake]>
            enc.array(4).ok();
            enc.map(snapshots.pools.len() as u64).ok();
            for pool in &snapshots.pools {
                enc.bytes(&pool.pool_id).ok();
                enc.array(3).ok();
                enc.u64(pool.mark_stake).ok();
                enc.u64(pool.set_stake).ok();
                enc.u64(pool.go_stake).ok();
            }
            // Totals (NonZero Coin — must be >= 1)
            enc.u64(snapshots.total_mark_stake.max(1)).ok();
            enc.u64(snapshots.total_set_stake.max(1)).ok();
            enc.u64(snapshots.total_go_stake.max(1)).ok();
        }
        QueryResult::StakePools(pool_ids) => {
            // Wire format: tag(258) Set<KeyHash StakePool>
            enc.tag(minicbor::data::Tag::new(258)).ok();
            enc.array(pool_ids.len() as u64).ok();
            for pid in pool_ids {
                enc.bytes(pid).ok();
            }
        }
        QueryResult::PoolParams(params) => {
            // Wire format: Map<pool_hash(28), PoolParams>
            // PoolParams is a CDDL record (positional fields, no array wrapper):
            //   operator(hash28), vrf_keyhash(hash32), pledge(coin), cost(coin),
            //   margin(unit_interval), reward_account(bytes), owners(set<hash28>),
            //   relays([*relay]), metadata(nullable [url, hash])
            enc.map(params.len() as u64).ok();
            for pool in params {
                // Key: pool hash
                enc.bytes(&pool.pool_id).ok();
                // Value: positional PoolParams fields (9 items, NOT wrapped in array)
                // Per CDDL: pool_params = (operator, vrf_keyhash, pledge, cost, margin,
                //            reward_account, pool_owners, relays, pool_metadata)
                // When used as a map value in GetStakePoolParams result, each value
                // is encoded as a 9-element array.
                enc.array(9).ok();
                enc.bytes(&pool.pool_id).ok(); // operator
                enc.bytes(&pool.vrf_keyhash).ok();
                enc.u64(pool.pledge).ok();
                enc.u64(pool.cost).ok();
                // margin as tagged rational
                encode_tagged_rational(&mut enc, pool.margin_num, pool.margin_den);
                enc.bytes(&pool.reward_account).ok();
                // owners as set (tag 258)
                enc.tag(minicbor::data::Tag::new(258)).ok();
                enc.array(pool.owners.len() as u64).ok();
                for owner in &pool.owners {
                    enc.bytes(owner).ok();
                }
                // relays
                enc.array(pool.relays.len() as u64).ok();
                for relay in &pool.relays {
                    encode_relay_cbor(&mut enc, relay);
                }
                // metadata
                if let Some(url) = &pool.metadata_url {
                    enc.array(2).ok();
                    enc.str(url).ok();
                    if let Some(hash) = &pool.metadata_hash {
                        enc.bytes(hash).ok();
                    } else {
                        enc.bytes(&[0u8; 32]).ok();
                    }
                } else {
                    enc.null().ok();
                }
            }
        }
        QueryResult::AccountState { treasury, reserves } => {
            // Account state: [treasury, reserves]
            enc.array(2).ok();
            enc.u64(*treasury).ok();
            enc.u64(*reserves).ok();
        }
        QueryResult::GenesisConfig(gc) => {
            // CompactGenesis: array(15) matching ShelleyGenesis CBOR wire format
            enc.array(15).ok();

            // [0] systemStart: UTCTime = array(3) [year, dayOfYear, picosecondsOfDay]
            let (year, day_of_year, picos) = parse_utctime(&gc.system_start);
            enc.array(3).ok();
            enc.u64(year).ok();
            enc.u64(day_of_year).ok();
            enc.u64(picos).ok();

            // [1] networkMagic: u32
            enc.u32(gc.network_magic).ok();

            // [2] networkId: 0=Testnet, 1=Mainnet
            enc.u8(gc.network_id).ok();

            // [3] activeSlotsCoeff: [num, den] (NO tag(30))
            enc.array(2).ok();
            enc.u64(gc.active_slots_coeff_num).ok();
            enc.u64(gc.active_slots_coeff_den).ok();

            // [4] securityParam: u64
            enc.u64(gc.security_param).ok();

            // [5] epochLength: u64
            enc.u64(gc.epoch_length).ok();

            // [6] slotsPerKESPeriod: u64
            enc.u64(gc.slots_per_kes_period).ok();

            // [7] maxKESEvolutions: u64
            enc.u64(gc.max_kes_evolutions).ok();

            // [8] slotLength: Fixed E6 integer (microseconds)
            enc.u64(gc.slot_length_micros).ok();

            // [9] updateQuorum: u64
            enc.u64(gc.update_quorum).ok();

            // [10] maxLovelaceSupply: u64
            enc.u64(gc.max_lovelace_supply).ok();

            // [11] protocolParams: legacy Shelley PParams array(18)
            encode_shelley_pparams(&mut enc, &gc.protocol_params);

            // [12] genDelegs: Map<hash28 → array(2)[hash28, hash32]>
            enc.map(gc.gen_delegs.len() as u64).ok();
            for (genesis_hash, delegate_hash, vrf_hash) in &gc.gen_delegs {
                enc.bytes(genesis_hash).ok();
                enc.array(2).ok();
                enc.bytes(delegate_hash).ok();
                enc.bytes(vrf_hash).ok();
            }

            // [13] initialFunds: empty map (CompactGenesis)
            enc.map(0).ok();

            // [14] staking: array(2) [empty_map, empty_map] (CompactGenesis)
            enc.array(2).ok();
            enc.map(0).ok();
            enc.map(0).ok();
        }
        QueryResult::NonMyopicMemberRewards(rewards) => {
            // Map from stake_amount → map from pool_id → reward
            enc.map(rewards.len() as u64).ok();
            for entry in rewards {
                enc.u64(entry.stake_amount).ok();
                enc.map(entry.pool_rewards.len() as u64).ok();
                for (pool_id, reward) in &entry.pool_rewards {
                    enc.bytes(pool_id).ok();
                    enc.u64(*reward).ok();
                }
            }
        }
        QueryResult::ProposedPParamsUpdates => {
            // Empty map — Conway era uses governance proposals instead of PP updates
            enc.map(0).ok();
        }
        QueryResult::Constitution {
            url,
            data_hash,
            script_hash,
        } => {
            // Constitution = array(2) [Anchor, StrictMaybe ScriptHash]
            enc.array(2).ok();
            // Anchor = array(2) [url, hash]
            enc.array(2).ok();
            enc.str(url).ok();
            enc.bytes(data_hash).ok();
            // StrictMaybe ScriptHash (null-encoded)
            if let Some(script) = script_hash {
                enc.bytes(script).ok();
            } else {
                enc.null().ok();
            }
        }
        QueryResult::Error(msg) => {
            enc.str(msg).ok();
        }
    }

    buf
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let result = QueryResult::GovState(GovStateSnapshot {
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
        });
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

        // MsgAcquire: [0]
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(1).unwrap();
        enc.u32(0).unwrap();

        let result = handle_tx_monitor(&payload, &mempool, &handler)
            .await
            .unwrap();
        assert!(result.is_some());

        let segment = result.unwrap();
        assert_eq!(segment.protocol_id, MINI_PROTOCOL_TX_MONITOR);
        assert!(segment.is_responder);

        // Verify MsgAcquired [1, slot_no]
        let mut decoder = minicbor::Decoder::new(&segment.payload);
        let _ = decoder.array();
        assert_eq!(decoder.u32().unwrap(), 1); // MsgAcquired
        let _slot = decoder.u64().unwrap();
    }

    #[tokio::test]
    async fn test_handle_tx_monitor_has_tx() {
        let mempool = Arc::new(Mempool::new(torsten_mempool::MempoolConfig::default()));
        let handler = Arc::new(RwLock::new(QueryHandler::new()));
        let tx_hash = Hash32::from_bytes([0xAA; 32]);
        let tx = torsten_primitives::transaction::Transaction::empty_with_hash(tx_hash);
        mempool.add_tx(tx_hash, tx, 100).unwrap();

        // MsgHasTx: [4, tx_id_bytes]
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(2).unwrap();
        enc.u32(4).unwrap();
        enc.bytes(tx_hash.as_bytes()).unwrap();

        let result = handle_tx_monitor(&payload, &mempool, &handler)
            .await
            .unwrap();
        assert!(result.is_some());

        let segment = result.unwrap();
        let mut decoder = minicbor::Decoder::new(&segment.payload);
        let _ = decoder.array();
        assert_eq!(decoder.u32().unwrap(), 5); // MsgHasTxReply
        assert!(decoder.bool().unwrap()); // tx exists
    }

    #[tokio::test]
    async fn test_handle_tx_monitor_has_tx_missing() {
        let mempool = Arc::new(Mempool::new(torsten_mempool::MempoolConfig::default()));
        let handler = Arc::new(RwLock::new(QueryHandler::new()));

        // MsgHasTx for non-existent tx
        let tx_hash = Hash32::from_bytes([0xBB; 32]);
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(2).unwrap();
        enc.u32(4).unwrap();
        enc.bytes(tx_hash.as_bytes()).unwrap();

        let result = handle_tx_monitor(&payload, &mempool, &handler)
            .await
            .unwrap();
        assert!(result.is_some());

        let segment = result.unwrap();
        let mut decoder = minicbor::Decoder::new(&segment.payload);
        let _ = decoder.array();
        assert_eq!(decoder.u32().unwrap(), 5); // MsgHasTxReply
        assert!(!decoder.bool().unwrap()); // tx does not exist
    }

    #[tokio::test]
    async fn test_handle_tx_monitor_get_sizes() {
        let mempool = Arc::new(Mempool::new(torsten_mempool::MempoolConfig::default()));
        let handler = Arc::new(RwLock::new(QueryHandler::new()));
        let tx_hash = Hash32::from_bytes([0xAA; 32]);
        let tx = torsten_primitives::transaction::Transaction::empty_with_hash(tx_hash);
        mempool.add_tx(tx_hash, tx, 500).unwrap();

        // MsgGetSizes: [8]
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(1).unwrap();
        enc.u32(8).unwrap();

        let result = handle_tx_monitor(&payload, &mempool, &handler)
            .await
            .unwrap();
        assert!(result.is_some());

        let segment = result.unwrap();
        let mut decoder = minicbor::Decoder::new(&segment.payload);
        let _ = decoder.array();
        assert_eq!(decoder.u32().unwrap(), 9); // MsgGetSizesReply
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

        // MsgNextTx: [6]
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(1).unwrap();
        enc.u32(6).unwrap();

        let result = handle_tx_monitor(&payload, &mempool, &handler)
            .await
            .unwrap();
        assert!(result.is_some());

        let segment = result.unwrap();
        let mut decoder = minicbor::Decoder::new(&segment.payload);
        let _ = decoder.array();
        assert_eq!(decoder.u32().unwrap(), 7); // MsgNextTxReply
        assert!(decoder.null().is_ok()); // no tx available
    }

    #[tokio::test]
    async fn test_handle_tx_monitor_done() {
        let mempool = Arc::new(Mempool::new(torsten_mempool::MempoolConfig::default()));
        let handler = Arc::new(RwLock::new(QueryHandler::new()));

        // MsgDone: [3]
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(1).unwrap();
        enc.u32(3).unwrap();

        let result = handle_tx_monitor(&payload, &mempool, &handler)
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
        use crate::n2n_server::BlockProvider;

        struct MockBlockProvider;

        impl BlockProvider for MockBlockProvider {
            fn get_block(&self, _hash: &[u8; 32]) -> Option<Vec<u8>> {
                None
            }
            fn has_block(&self, hash: &[u8; 32]) -> bool {
                // Only recognize our test hash
                *hash == [0xbb; 32]
            }
            fn get_tip(&self) -> (u64, [u8; 32], u64) {
                (200, [0xcc; 32], 10)
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

    #[test]
    fn test_parse_utctime() {
        // Preview testnet: 2022-04-01T00:00:00Z → (2022, 91, 0)
        let (y, d, p) = parse_utctime("2022-04-01T00:00:00Z");
        assert_eq!(y, 2022);
        assert_eq!(d, 91); // April 1 = day 91 in non-leap year
        assert_eq!(p, 0);

        // Mainnet: 2017-09-23T21:44:51Z
        let (y, d, p) = parse_utctime("2017-09-23T21:44:51Z");
        assert_eq!(y, 2017);
        assert_eq!(d, 266); // Sep 23 = day 266
        assert_eq!(p, (21 * 3600 + 44 * 60 + 51) * 1_000_000_000_000);

        // Leap year: 2024-03-01T00:00:00Z
        let (y, d, _) = parse_utctime("2024-03-01T00:00:00Z");
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
}
