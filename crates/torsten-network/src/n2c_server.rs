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
const MINI_PROTOCOL_TX_MONITOR: u16 = 12;

/// Node-to-Client server that listens on a Unix domain socket.
pub struct N2CServer {
    query_handler: Arc<RwLock<QueryHandler>>,
    mempool: Arc<Mempool>,
    tx_validator: Option<Arc<dyn TxValidator>>,
}

impl N2CServer {
    pub fn new(query_handler: Arc<RwLock<QueryHandler>>, mempool: Arc<Mempool>) -> Self {
        N2CServer {
            query_handler,
            mempool,
            tx_validator: None,
        }
    }

    /// Set a transaction validator for Phase-1/Phase-2 validation before mempool admission
    pub fn set_tx_validator(&mut self, validator: Arc<dyn TxValidator>) {
        self.tx_validator = Some(validator);
    }

    /// Start listening on the given Unix socket path.
    /// This runs indefinitely, accepting connections and spawning tasks for each.
    pub async fn listen(&self, socket_path: &Path) -> Result<(), N2CServerError> {
        // Remove existing socket file if present
        if socket_path.exists() {
            std::fs::remove_file(socket_path)?;
        }

        let listener = UnixListener::bind(socket_path)?;
        info!("N2C server listening on {}", socket_path.display());

        loop {
            match listener.accept().await {
                Ok((stream, _addr)) => {
                    info!("N2C client connected");
                    let handler = self.query_handler.clone();
                    let mempool = self.mempool.clone();
                    let validator = self.tx_validator.clone();
                    tokio::spawn(async move {
                        if let Err(e) =
                            handle_n2c_connection(stream, handler, mempool, validator).await
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
    }
}

/// Handle a single N2C client connection
async fn handle_n2c_connection(
    mut stream: tokio::net::UnixStream,
    query_handler: Arc<RwLock<QueryHandler>>,
    mempool: Arc<Mempool>,
    tx_validator: Option<Arc<dyn TxValidator>>,
) -> Result<(), N2CServerError> {
    let mut buf = vec![0u8; 65536];

    loop {
        let n = stream.read(&mut buf).await?;
        if n == 0 {
            return Ok(()); // Client disconnected
        }

        // Parse multiplexer segments from the received data
        let mut offset = 0;
        while offset < n {
            let remaining = &buf[offset..n];
            if remaining.len() < 8 {
                break; // Need more data for a complete header
            }

            match Segment::decode(remaining) {
                Ok((segment, consumed)) => {
                    offset += consumed;

                    // Process the segment
                    let response =
                        process_segment(&segment, &query_handler, &mempool, &tx_validator).await?;
                    if let Some(resp_segment) = response {
                        let encoded = resp_segment.encode();
                        stream.write_all(&encoded).await?;
                    }
                }
                Err(_) => {
                    break; // Incomplete segment
                }
            }
        }
    }
}

/// Process a single multiplexer segment and optionally return a response
async fn process_segment(
    segment: &Segment,
    query_handler: &Arc<RwLock<QueryHandler>>,
    mempool: &Arc<Mempool>,
    tx_validator: &Option<Arc<dyn TxValidator>>,
) -> Result<Option<Segment>, N2CServerError> {
    match segment.protocol_id {
        MINI_PROTOCOL_HANDSHAKE => handle_handshake(&segment.payload),
        MINI_PROTOCOL_STATE_QUERY => handle_state_query(&segment.payload, query_handler).await,
        MINI_PROTOCOL_TX_SUBMISSION => {
            handle_tx_submission(&segment.payload, mempool, tx_validator)
        }
        MINI_PROTOCOL_TX_MONITOR => handle_tx_monitor(&segment.payload, mempool),
        MINI_PROTOCOL_CHAINSYNC => handle_local_chainsync(&segment.payload, query_handler).await,
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
    let version = parse_highest_version(payload).unwrap_or(16);

    debug!("N2C handshake: accepting version {version}, magic {network_magic}");

    // Encode accept response: [1, version, [magic, false]]
    encoder
        .array(3)
        .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
    encoder
        .u32(1)
        .map_err(|e| N2CServerError::Protocol(e.to_string()))?; // MsgAcceptVersion
    encoder
        .u32(version as u32)
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

/// Parse the highest proposed version number
fn parse_highest_version(payload: &[u8]) -> Option<u16> {
    let mut decoder = minicbor::Decoder::new(payload);
    decoder.array().ok()?;
    decoder.u32().ok()?; // msg type
    let map_len = decoder.map().ok()??;
    let mut highest = 0u16;
    for _ in 0..map_len {
        if let Ok(v) = decoder.u32() {
            if v as u16 > highest && v <= 17 {
                highest = v as u16;
            }
        }
        // Skip the value (params)
        decoder.skip().ok()?;
    }
    if highest > 0 {
        Some(highest)
    } else {
        None
    }
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
fn handle_tx_monitor(
    payload: &[u8],
    mempool: &Arc<Mempool>,
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
            debug!("LocalTxMonitor: MsgAcquire");
            // Return current slot (0 if unknown — will be updated when blocks arrive)
            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);
            enc.array(2)
                .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
            enc.u32(1)
                .map_err(|e| N2CServerError::Protocol(e.to_string()))?; // MsgAcquired
            enc.u64(0)
                .map_err(|e| N2CServerError::Protocol(e.to_string()))?; // slot_no placeholder
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
            // MsgRequestNext → MsgAwaitReply [1]
            // For now, always respond with MsgAwaitReply since we don't push blocks to clients
            debug!("LocalChainSync: MsgRequestNext → MsgAwaitReply");
            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);
            enc.array(1)
                .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
            enc.u32(1)
                .map_err(|e| N2CServerError::Protocol(e.to_string()))?; // MsgAwaitReply
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

            // Try to parse the client's points and find one that matches our tip
            // (basic implementation — checks if any point matches our current tip)
            let found_point = parse_client_points(&mut decoder, tip_slot, &tip_hash);

            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);

            if let Some((slot, hash)) = found_point {
                debug!(slot, "LocalChainSync: MsgIntersectFound");
                // MsgIntersectFound [5, point, tip]
                enc.array(3)
                    .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
                enc.u32(5)
                    .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
                // point: [slot, hash]
                encode_point(&mut enc, slot, &hash)?;
                // tip: [point, block_no]
                encode_tip(&mut enc, tip_slot, &tip_hash, tip_block_no)?;
            } else {
                debug!("LocalChainSync: MsgIntersectNotFound");
                // MsgIntersectNotFound [6, tip]
                enc.array(2)
                    .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
                enc.u32(6)
                    .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
                // tip: [point, block_no]
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
/// Encode protocol parameters as CBOR map with integer keys per Cardano spec.
///
/// Conway era protocol params use keys 0-33:
///   0: min_fee_a, 1: min_fee_b, 2: max_block_body_size, 3: max_tx_size,
///   4: max_block_header_size, 5: key_deposit, 6: pool_deposit, 7: e_max,
///   8: n_opt, 9: a0, 10: rho, 11: tau, 16: min_pool_cost, 17: ada_per_utxo_byte,
///   18: cost_models, 19: execution_costs, 20: max_tx_ex_units, 21: max_block_ex_units,
///   22: max_val_size, 23: collateral_percentage, 24: max_collateral_inputs,
///   25: protocol_version, 26: pool_voting_thresholds, 27: drep_voting_thresholds,
///   28: committee_min_size, 29: committee_max_term_length, 30: gov_action_lifetime,
///   31: gov_action_deposit, 32: drep_deposit, 33: drep_activity,
///   34: min_fee_ref_script_cost_per_byte
fn encode_protocol_params_cbor(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    pp: &ProtocolParamsSnapshot,
) {
    // Count entries: base 24 fields + optional cost model entries
    let mut count = 24u64;
    if pp.cost_models_v1.is_some() || pp.cost_models_v2.is_some() || pp.cost_models_v3.is_some() {
        count += 1; // key 18
    }
    enc.map(count).ok();

    // 0: min_fee_a
    enc.u32(0).ok();
    enc.u64(pp.min_fee_a).ok();

    // 1: min_fee_b
    enc.u32(1).ok();
    enc.u64(pp.min_fee_b).ok();

    // 2: max_block_body_size
    enc.u32(2).ok();
    enc.u64(pp.max_block_body_size).ok();

    // 3: max_tx_size
    enc.u32(3).ok();
    enc.u64(pp.max_tx_size).ok();

    // 4: max_block_header_size
    enc.u32(4).ok();
    enc.u64(pp.max_block_header_size).ok();

    // 5: key_deposit
    enc.u32(5).ok();
    enc.u64(pp.key_deposit).ok();

    // 6: pool_deposit
    enc.u32(6).ok();
    enc.u64(pp.pool_deposit).ok();

    // 7: e_max
    enc.u32(7).ok();
    enc.u64(pp.e_max).ok();

    // 8: n_opt
    enc.u32(8).ok();
    enc.u64(pp.n_opt).ok();

    // 9: a0 (rational)
    enc.u32(9).ok();
    enc.tag(minicbor::data::Tag::new(30)).ok();
    enc.array(2).ok();
    enc.u64(pp.a0_num).ok();
    enc.u64(pp.a0_den).ok();

    // 10: rho (rational)
    enc.u32(10).ok();
    enc.tag(minicbor::data::Tag::new(30)).ok();
    enc.array(2).ok();
    enc.u64(pp.rho_num).ok();
    enc.u64(pp.rho_den).ok();

    // 11: tau (rational)
    enc.u32(11).ok();
    enc.tag(minicbor::data::Tag::new(30)).ok();
    enc.array(2).ok();
    enc.u64(pp.tau_num).ok();
    enc.u64(pp.tau_den).ok();

    // 16: min_pool_cost
    enc.u32(16).ok();
    enc.u64(pp.min_pool_cost).ok();

    // 17: ada_per_utxo_byte
    enc.u32(17).ok();
    enc.u64(pp.ada_per_utxo_byte).ok();

    // 18: cost_models (map: {0: [v1], 1: [v2], 2: [v3]})
    if pp.cost_models_v1.is_some() || pp.cost_models_v2.is_some() || pp.cost_models_v3.is_some() {
        enc.u32(18).ok();
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

    // 19: execution_costs [mem_price, step_price] as tagged rationals
    enc.u32(19).ok();
    enc.array(2).ok();
    enc.tag(minicbor::data::Tag::new(30)).ok();
    enc.array(2).ok();
    enc.u64(pp.execution_costs_mem_num).ok();
    enc.u64(pp.execution_costs_mem_den).ok();
    enc.tag(minicbor::data::Tag::new(30)).ok();
    enc.array(2).ok();
    enc.u64(pp.execution_costs_step_num).ok();
    enc.u64(pp.execution_costs_step_den).ok();

    // 20: max_tx_ex_units [mem, steps]
    enc.u32(20).ok();
    enc.array(2).ok();
    enc.u64(pp.max_tx_ex_mem).ok();
    enc.u64(pp.max_tx_ex_steps).ok();

    // 21: max_block_ex_units [mem, steps]
    enc.u32(21).ok();
    enc.array(2).ok();
    enc.u64(pp.max_block_ex_mem).ok();
    enc.u64(pp.max_block_ex_steps).ok();

    // 22: max_val_size
    enc.u32(22).ok();
    enc.u64(pp.max_val_size).ok();

    // 23: collateral_percentage
    enc.u32(23).ok();
    enc.u64(pp.collateral_percentage).ok();

    // 24: max_collateral_inputs
    enc.u32(24).ok();
    enc.u64(pp.max_collateral_inputs).ok();

    // 25: protocol_version [major, minor]
    enc.u32(25).ok();
    enc.array(2).ok();
    enc.u64(pp.protocol_version_major).ok();
    enc.u64(pp.protocol_version_minor).ok();

    // 26: pool_voting_thresholds [pvt_committee, pvt_hard_fork, ...]
    enc.u32(26).ok();
    enc.array(5).ok();
    // motion_no_confidence
    enc.tag(minicbor::data::Tag::new(30)).ok();
    enc.array(2).ok();
    enc.u64(pp.pvt_committee_num).ok();
    enc.u64(pp.pvt_committee_den).ok();
    // committee_normal
    enc.tag(minicbor::data::Tag::new(30)).ok();
    enc.array(2).ok();
    enc.u64(pp.pvt_committee_num).ok();
    enc.u64(pp.pvt_committee_den).ok();
    // committee_no_confidence
    enc.tag(minicbor::data::Tag::new(30)).ok();
    enc.array(2).ok();
    enc.u64(pp.pvt_committee_num).ok();
    enc.u64(pp.pvt_committee_den).ok();
    // hard_fork_initiation
    enc.tag(minicbor::data::Tag::new(30)).ok();
    enc.array(2).ok();
    enc.u64(pp.pvt_hard_fork_num).ok();
    enc.u64(pp.pvt_hard_fork_den).ok();
    // pp_security_group (use pvt_committee as placeholder)
    enc.tag(minicbor::data::Tag::new(30)).ok();
    enc.array(2).ok();
    enc.u64(pp.pvt_committee_num).ok();
    enc.u64(pp.pvt_committee_den).ok();

    // 27: drep_voting_thresholds [dvt_*, ...]
    enc.u32(27).ok();
    enc.array(10).ok();
    // motion_no_confidence
    enc.tag(minicbor::data::Tag::new(30)).ok();
    enc.array(2).ok();
    enc.u64(pp.dvt_no_confidence_num).ok();
    enc.u64(pp.dvt_no_confidence_den).ok();
    // committee_normal
    enc.tag(minicbor::data::Tag::new(30)).ok();
    enc.array(2).ok();
    enc.u64(pp.dvt_committee_normal_num).ok();
    enc.u64(pp.dvt_committee_normal_den).ok();
    // committee_no_confidence
    enc.tag(minicbor::data::Tag::new(30)).ok();
    enc.array(2).ok();
    enc.u64(pp.dvt_committee_no_confidence_num).ok();
    enc.u64(pp.dvt_committee_no_confidence_den).ok();
    // update_constitution
    enc.tag(minicbor::data::Tag::new(30)).ok();
    enc.array(2).ok();
    enc.u64(pp.dvt_constitution_num).ok();
    enc.u64(pp.dvt_constitution_den).ok();
    // hard_fork_initiation
    enc.tag(minicbor::data::Tag::new(30)).ok();
    enc.array(2).ok();
    enc.u64(pp.dvt_hard_fork_num).ok();
    enc.u64(pp.dvt_hard_fork_den).ok();
    // pp_network_group
    enc.tag(minicbor::data::Tag::new(30)).ok();
    enc.array(2).ok();
    enc.u64(pp.dvt_p_param_change_num).ok();
    enc.u64(pp.dvt_p_param_change_den).ok();
    // pp_economic_group
    enc.tag(minicbor::data::Tag::new(30)).ok();
    enc.array(2).ok();
    enc.u64(pp.dvt_p_param_change_num).ok();
    enc.u64(pp.dvt_p_param_change_den).ok();
    // pp_technical_group
    enc.tag(minicbor::data::Tag::new(30)).ok();
    enc.array(2).ok();
    enc.u64(pp.dvt_p_param_change_num).ok();
    enc.u64(pp.dvt_p_param_change_den).ok();
    // pp_governance_group
    enc.tag(minicbor::data::Tag::new(30)).ok();
    enc.array(2).ok();
    enc.u64(pp.dvt_p_param_change_num).ok();
    enc.u64(pp.dvt_p_param_change_den).ok();
    // treasury_withdrawal
    enc.tag(minicbor::data::Tag::new(30)).ok();
    enc.array(2).ok();
    enc.u64(pp.dvt_treasury_withdrawal_num).ok();
    enc.u64(pp.dvt_treasury_withdrawal_den).ok();

    // 28: committee_min_size
    enc.u32(28).ok();
    enc.u64(pp.committee_min_size).ok();

    // 29: committee_max_term_length
    enc.u32(29).ok();
    enc.u64(pp.committee_max_term_length).ok();

    // 30: gov_action_lifetime
    enc.u32(30).ok();
    enc.u64(pp.gov_action_lifetime).ok();

    // 31: gov_action_deposit
    enc.u32(31).ok();
    enc.u64(pp.gov_action_deposit).ok();

    // 32: drep_deposit
    enc.u32(32).ok();
    enc.u64(pp.drep_deposit).ok();

    // 33: drep_activity
    enc.u32(33).ok();
    enc.u64(pp.drep_activity).ok();
}

fn encode_query_result(result: &QueryResult) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut buf);

    // MsgResult [4, result]
    enc.array(2).ok();
    enc.u32(4).ok(); // MsgResult tag

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
            enc.map(pools.len() as u64).ok();
            for pool in pools {
                enc.bytes(&pool.pool_id).ok();
                enc.array(3).ok();
                enc.u64(pool.stake).ok();
                enc.u64(pool.pledge).ok();
                enc.u64(pool.cost).ok();
            }
        }
        QueryResult::GovState(gov) => {
            enc.map(4).ok();
            enc.str("drep_count").ok();
            enc.u64(gov.drep_count as u64).ok();
            enc.str("committee_member_count").ok();
            enc.u64(gov.committee_member_count as u64).ok();
            enc.str("treasury").ok();
            enc.u64(gov.treasury).ok();
            enc.str("proposals").ok();
            enc.array(gov.proposals.len() as u64).ok();
            for p in &gov.proposals {
                enc.map(6).ok();
                enc.str("tx_id").ok();
                enc.bytes(&p.tx_id).ok();
                enc.str("action_index").ok();
                enc.u32(p.action_index).ok();
                enc.str("action_type").ok();
                enc.str(&p.action_type).ok();
                enc.str("yes_votes").ok();
                enc.u64(p.yes_votes).ok();
                enc.str("no_votes").ok();
                enc.u64(p.no_votes).ok();
                enc.str("abstain_votes").ok();
                enc.u64(p.abstain_votes).ok();
            }
        }
        QueryResult::DRepState(dreps) => {
            enc.array(dreps.len() as u64).ok();
            for drep in dreps {
                enc.map(4).ok();
                enc.str("credential").ok();
                enc.bytes(&drep.credential_hash).ok();
                enc.str("deposit").ok();
                enc.u64(drep.deposit).ok();
                enc.str("anchor_url").ok();
                if let Some(url) = &drep.anchor_url {
                    enc.str(url).ok();
                } else {
                    enc.null().ok();
                }
                enc.str("registered_epoch").ok();
                enc.u64(drep.registered_epoch).ok();
            }
        }
        QueryResult::CommitteeState(committee) => {
            enc.map(2).ok();
            enc.str("members").ok();
            enc.array(committee.members.len() as u64).ok();
            for member in &committee.members {
                enc.map(2).ok();
                enc.str("cold").ok();
                enc.bytes(&member.cold_credential).ok();
                enc.str("hot").ok();
                enc.bytes(&member.hot_credential).ok();
            }
            enc.str("resigned").ok();
            enc.array(committee.resigned.len() as u64).ok();
            for cred in &committee.resigned {
                enc.bytes(cred).ok();
            }
        }
        QueryResult::UtxoByAddress(utxos) => {
            enc.array(utxos.len() as u64).ok();
            for utxo in utxos {
                enc.map(5).ok();
                enc.str("tx_hash").ok();
                enc.bytes(&utxo.tx_hash).ok();
                enc.str("output_index").ok();
                enc.u32(utxo.output_index).ok();
                enc.str("address").ok();
                enc.str(&utxo.address).ok();
                enc.str("lovelace").ok();
                enc.u64(utxo.lovelace).ok();
                enc.str("has_datum").ok();
                enc.bool(utxo.has_datum).ok();
            }
        }
        QueryResult::StakeAddressInfo(addrs) => {
            enc.array(addrs.len() as u64).ok();
            for addr in addrs {
                enc.map(3).ok();
                enc.str("credential").ok();
                enc.bytes(&addr.credential_hash).ok();
                enc.str("delegated_pool").ok();
                if let Some(pool) = &addr.delegated_pool {
                    enc.bytes(pool).ok();
                } else {
                    enc.null().ok();
                }
                enc.str("reward_balance").ok();
                enc.u64(addr.reward_balance).ok();
            }
        }
        QueryResult::StakeSnapshots(snapshots) => {
            enc.map(4).ok();
            enc.str("pools").ok();
            enc.array(snapshots.pools.len() as u64).ok();
            for pool in &snapshots.pools {
                enc.map(4).ok();
                enc.str("pool_id").ok();
                enc.bytes(&pool.pool_id).ok();
                enc.str("mark_stake").ok();
                enc.u64(pool.mark_stake).ok();
                enc.str("set_stake").ok();
                enc.u64(pool.set_stake).ok();
                enc.str("go_stake").ok();
                enc.u64(pool.go_stake).ok();
            }
            enc.str("total_mark_stake").ok();
            enc.u64(snapshots.total_mark_stake).ok();
            enc.str("total_set_stake").ok();
            enc.u64(snapshots.total_set_stake).ok();
            enc.str("total_go_stake").ok();
            enc.u64(snapshots.total_go_stake).ok();
        }
        QueryResult::PoolParams(params) => {
            enc.array(params.len() as u64).ok();
            for pool in params {
                enc.map(7).ok();
                enc.str("pool_id").ok();
                enc.bytes(&pool.pool_id).ok();
                enc.str("vrf_keyhash").ok();
                enc.bytes(&pool.vrf_keyhash).ok();
                enc.str("pledge").ok();
                enc.u64(pool.pledge).ok();
                enc.str("cost").ok();
                enc.u64(pool.cost).ok();
                enc.str("margin_num").ok();
                enc.u64(pool.margin_num).ok();
                enc.str("margin_den").ok();
                enc.u64(pool.margin_den).ok();
                enc.str("relays").ok();
                enc.array(pool.relays.len() as u64).ok();
                for relay in &pool.relays {
                    enc.str(relay).ok();
                }
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
        // Encode a handshake proposal: [0, {1: [764824073, false], 16: [764824073, false]}]
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(0).unwrap(); // MsgProposeVersions
        enc.map(2).unwrap();
        enc.u32(1).unwrap();
        enc.array(2).unwrap();
        enc.u64(764824073).unwrap();
        enc.bool(false).unwrap();
        enc.u32(16).unwrap();
        enc.array(2).unwrap();
        enc.u64(764824073).unwrap();
        enc.bool(false).unwrap();

        assert_eq!(parse_highest_version(&buf), Some(16));
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

        // Verify we can decode the CBOR map
        let mut decoder = minicbor::Decoder::new(&cbor);
        let _ = decoder.array();
        assert_eq!(decoder.u32().unwrap(), 4); // MsgResult
                                               // Result is a CBOR map with integer keys
        let map_len = decoder.map().unwrap().unwrap();
        assert!(map_len >= 24); // At least 24 entries
    }

    #[test]
    fn test_encode_query_result_gov_state() {
        use crate::query_handler::{GovStateSnapshot, ProposalSnapshot};

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
            }],
            drep_count: 10,
            committee_member_count: 3,
            treasury: 5_000_000_000_000,
        });
        let cbor = encode_query_result(&result);
        assert!(!cbor.is_empty());

        // Verify the outer structure
        let mut decoder = minicbor::Decoder::new(&cbor);
        let _ = decoder.array();
        assert_eq!(decoder.u32().unwrap(), 4); // MsgResult
    }

    #[test]
    fn test_encode_query_result_drep_state() {
        use crate::query_handler::DRepSnapshot;

        let result = QueryResult::DRepState(vec![DRepSnapshot {
            credential_hash: vec![0xdd; 32],
            deposit: 500_000_000,
            anchor_url: Some("https://example.com".to_string()),
            registered_epoch: 42,
        }]);
        let cbor = encode_query_result(&result);
        assert!(!cbor.is_empty());
    }

    #[test]
    fn test_encode_query_result_committee_state() {
        use crate::query_handler::{CommitteeMemberSnapshot, CommitteeSnapshot};

        let result = QueryResult::CommitteeState(CommitteeSnapshot {
            members: vec![CommitteeMemberSnapshot {
                cold_credential: vec![0x01; 32],
                hot_credential: vec![0x02; 32],
            }],
            resigned: vec![vec![0x03; 32]],
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
            pledge: 500_000_000,
            cost: 340_000_000,
            margin_num: 1,
            margin_den: 100,
        }]);
        let cbor = encode_query_result(&result);

        // Verify encoding: [4, map{pool_id => [stake, pledge, cost]}]
        let mut decoder = minicbor::Decoder::new(&cbor);
        let _ = decoder.array();
        assert_eq!(decoder.u32().unwrap(), 4);
        let map_len = decoder.map().unwrap().unwrap();
        assert_eq!(map_len, 1);
        let pool_id = decoder.bytes().unwrap();
        assert_eq!(pool_id, vec![0xaa; 28]);
        let _ = decoder.array();
        assert_eq!(decoder.u64().unwrap(), 1_000_000_000); // stake
        assert_eq!(decoder.u64().unwrap(), 500_000_000); // pledge
        assert_eq!(decoder.u64().unwrap(), 340_000_000); // cost
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

    #[test]
    fn test_handle_tx_monitor_acquire() {
        let mempool = Arc::new(Mempool::new(torsten_mempool::MempoolConfig::default()));

        // MsgAcquire: [0]
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(1).unwrap();
        enc.u32(0).unwrap();

        let result = handle_tx_monitor(&payload, &mempool).unwrap();
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

    #[test]
    fn test_handle_tx_monitor_has_tx() {
        let mempool = Arc::new(Mempool::new(torsten_mempool::MempoolConfig::default()));
        let tx_hash = Hash32::from_bytes([0xAA; 32]);
        let tx = torsten_primitives::transaction::Transaction::empty_with_hash(tx_hash);
        mempool.add_tx(tx_hash, tx, 100).unwrap();

        // MsgHasTx: [4, tx_id_bytes]
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(2).unwrap();
        enc.u32(4).unwrap();
        enc.bytes(tx_hash.as_bytes()).unwrap();

        let result = handle_tx_monitor(&payload, &mempool).unwrap();
        assert!(result.is_some());

        let segment = result.unwrap();
        let mut decoder = minicbor::Decoder::new(&segment.payload);
        let _ = decoder.array();
        assert_eq!(decoder.u32().unwrap(), 5); // MsgHasTxReply
        assert!(decoder.bool().unwrap()); // tx exists
    }

    #[test]
    fn test_handle_tx_monitor_has_tx_missing() {
        let mempool = Arc::new(Mempool::new(torsten_mempool::MempoolConfig::default()));

        // MsgHasTx for non-existent tx
        let tx_hash = Hash32::from_bytes([0xBB; 32]);
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(2).unwrap();
        enc.u32(4).unwrap();
        enc.bytes(tx_hash.as_bytes()).unwrap();

        let result = handle_tx_monitor(&payload, &mempool).unwrap();
        assert!(result.is_some());

        let segment = result.unwrap();
        let mut decoder = minicbor::Decoder::new(&segment.payload);
        let _ = decoder.array();
        assert_eq!(decoder.u32().unwrap(), 5); // MsgHasTxReply
        assert!(!decoder.bool().unwrap()); // tx does not exist
    }

    #[test]
    fn test_handle_tx_monitor_get_sizes() {
        let mempool = Arc::new(Mempool::new(torsten_mempool::MempoolConfig::default()));
        let tx_hash = Hash32::from_bytes([0xAA; 32]);
        let tx = torsten_primitives::transaction::Transaction::empty_with_hash(tx_hash);
        mempool.add_tx(tx_hash, tx, 500).unwrap();

        // MsgGetSizes: [8]
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(1).unwrap();
        enc.u32(8).unwrap();

        let result = handle_tx_monitor(&payload, &mempool).unwrap();
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

    #[test]
    fn test_handle_tx_monitor_next_tx() {
        let mempool = Arc::new(Mempool::new(torsten_mempool::MempoolConfig::default()));

        // MsgNextTx: [6]
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(1).unwrap();
        enc.u32(6).unwrap();

        let result = handle_tx_monitor(&payload, &mempool).unwrap();
        assert!(result.is_some());

        let segment = result.unwrap();
        let mut decoder = minicbor::Decoder::new(&segment.payload);
        let _ = decoder.array();
        assert_eq!(decoder.u32().unwrap(), 7); // MsgNextTxReply
        assert!(decoder.null().is_ok()); // no tx available
    }

    #[test]
    fn test_handle_tx_monitor_done() {
        let mempool = Arc::new(Mempool::new(torsten_mempool::MempoolConfig::default()));

        // MsgDone: [3]
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(1).unwrap();
        enc.u32(3).unwrap();

        let result = handle_tx_monitor(&payload, &mempool).unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_handle_local_chainsync_request_next() {
        let handler = Arc::new(RwLock::new(QueryHandler::new()));

        // MsgRequestNext: [0]
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(1).unwrap();
        enc.u32(0).unwrap();

        let result = handle_local_chainsync(&payload, &handler).await.unwrap();
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

        // MsgFindIntersect: [4, [[12345, hash]]]
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(2).unwrap();
        enc.u32(4).unwrap(); // MsgFindIntersect
        enc.array(1).unwrap(); // one point
        enc.array(2).unwrap(); // point: [slot, hash]
        enc.u64(12345).unwrap();
        enc.bytes(&[0xaa; 32]).unwrap();

        let result = handle_local_chainsync(&payload, &handler).await.unwrap();
        assert!(result.is_some());

        let segment = result.unwrap();
        let mut decoder = minicbor::Decoder::new(&segment.payload);
        let _ = decoder.array();
        assert_eq!(decoder.u32().unwrap(), 6); // MsgIntersectNotFound
    }

    #[tokio::test]
    async fn test_handle_local_chainsync_done() {
        let handler = Arc::new(RwLock::new(QueryHandler::new()));

        // MsgDone: [7]
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(1).unwrap();
        enc.u32(7).unwrap();

        let result = handle_local_chainsync(&payload, &handler).await.unwrap();
        assert!(result.is_none());
    }
}
