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
use crate::query_handler::{QueryHandler, QueryResult};

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

/// Node-to-Client server that listens on a Unix domain socket.
pub struct N2CServer {
    query_handler: Arc<RwLock<QueryHandler>>,
    mempool: Arc<Mempool>,
}

impl N2CServer {
    pub fn new(query_handler: Arc<RwLock<QueryHandler>>, mempool: Arc<Mempool>) -> Self {
        N2CServer {
            query_handler,
            mempool,
        }
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
                    tokio::spawn(async move {
                        if let Err(e) = handle_n2c_connection(stream, handler, mempool).await {
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
                    let response = process_segment(&segment, &query_handler, &mempool).await?;
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
) -> Result<Option<Segment>, N2CServerError> {
    match segment.protocol_id {
        MINI_PROTOCOL_HANDSHAKE => handle_handshake(&segment.payload),
        MINI_PROTOCOL_STATE_QUERY => handle_state_query(&segment.payload, query_handler).await,
        MINI_PROTOCOL_TX_SUBMISSION => handle_tx_submission(&segment.payload, mempool),
        MINI_PROTOCOL_CHAINSYNC => {
            debug!("LocalChainSync message received (not yet implemented)");
            Ok(None)
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
            let tx_cbor = extract_submitted_tx(&mut decoder);

            match tx_cbor {
                Some(tx_bytes) => {
                    // Compute transaction hash from the CBOR bytes
                    let tx_hash_bytes = torsten_primitives::hash::blake2b_256(&tx_bytes);
                    let tx_hash = Hash32::from_bytes(*tx_hash_bytes.as_bytes());
                    let tx_size = tx_bytes.len();

                    // Create a minimal transaction for mempool storage
                    let tx = torsten_primitives::transaction::Transaction::empty_with_hash(tx_hash);

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

/// Extract transaction CBOR bytes from a MsgSubmitTx payload
fn extract_submitted_tx(decoder: &mut minicbor::Decoder) -> Option<Vec<u8>> {
    // The structure after the tag is: [era_id, tx_bytes]
    // era_id is a u16, tx_bytes is CBOR bytes
    let _ = decoder.array().ok()?;
    let _era_id = decoder.u32().ok()?;
    // The tx is encoded as a CBOR byte string containing the serialized transaction
    let tx_bytes = decoder.bytes().ok()?;
    Some(tx_bytes.to_vec())
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
        QueryResult::ProtocolParams(json) => {
            enc.str(json).ok();
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

    #[test]
    fn test_handle_tx_submission_accept() {
        let mempool = Arc::new(Mempool::new(torsten_mempool::MempoolConfig::default()));

        // Build MsgSubmitTx: [0, [6, tx_bytes]]
        let tx_bytes = vec![0xa0u8]; // minimal CBOR (empty map)
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(2).unwrap();
        enc.u32(0).unwrap(); // MsgSubmitTx
        enc.array(2).unwrap();
        enc.u32(6).unwrap(); // Conway era
        enc.bytes(&tx_bytes).unwrap();

        let result = handle_tx_submission(&payload, &mempool).unwrap();
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

        let tx_bytes = vec![0xa0u8];
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(2).unwrap();
        enc.u32(0).unwrap();
        enc.array(2).unwrap();
        enc.u32(6).unwrap();
        enc.bytes(&tx_bytes).unwrap();

        // Submit twice - both should accept
        let _ = handle_tx_submission(&payload, &mempool).unwrap();
        let result = handle_tx_submission(&payload, &mempool).unwrap();
        assert!(result.is_some());

        let segment = result.unwrap();
        let mut decoder = minicbor::Decoder::new(&segment.payload);
        let _ = decoder.array();
        assert_eq!(decoder.u32().unwrap(), 1); // Still accepted (AlreadyExists)
        assert_eq!(mempool.len(), 1);
    }

    #[test]
    fn test_handle_tx_submission_full_mempool() {
        let config = torsten_mempool::MempoolConfig {
            max_transactions: 1,
            max_bytes: 1024 * 1024,
        };
        let mempool = Arc::new(Mempool::new(config));

        // Fill the mempool
        let tx_bytes_1 = vec![0xa0u8];
        let mut payload1 = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload1);
        enc.array(2).unwrap();
        enc.u32(0).unwrap();
        enc.array(2).unwrap();
        enc.u32(6).unwrap();
        enc.bytes(&tx_bytes_1).unwrap();
        let _ = handle_tx_submission(&payload1, &mempool).unwrap();

        // Submit a different tx - should be rejected
        let tx_bytes_2 = vec![0xa1u8, 0x00, 0x01]; // different CBOR
        let mut payload2 = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload2);
        enc.array(2).unwrap();
        enc.u32(0).unwrap();
        enc.array(2).unwrap();
        enc.u32(6).unwrap();
        enc.bytes(&tx_bytes_2).unwrap();

        let result = handle_tx_submission(&payload2, &mempool).unwrap();
        assert!(result.is_some());

        let segment = result.unwrap();
        let mut decoder = minicbor::Decoder::new(&segment.payload);
        let _ = decoder.array();
        assert_eq!(decoder.u32().unwrap(), 2); // MsgRejectTx
    }

    #[test]
    fn test_encode_query_result_protocol_params() {
        let result = QueryResult::ProtocolParams("{\"min_fee_a\": 44}".to_string());
        let cbor = encode_query_result(&result);
        assert!(!cbor.is_empty());

        // Verify we can decode the string back
        let mut decoder = minicbor::Decoder::new(&cbor);
        let _ = decoder.array();
        assert_eq!(decoder.u32().unwrap(), 4); // MsgResult
        let json = decoder.str().unwrap();
        assert!(json.contains("min_fee_a"));
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

        // Build MsgDone: [3]
        let mut payload = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut payload);
        enc.array(1).unwrap();
        enc.u32(3).unwrap();

        let result = handle_tx_submission(&payload, &mempool).unwrap();
        assert!(result.is_none());
    }
}
