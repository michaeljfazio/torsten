use std::sync::Arc;
use torsten_mempool::Mempool;
use tracing::{debug, info, warn};

use crate::multiplexer::Segment;

use super::{N2CServerError, TxValidator, MINI_PROTOCOL_TX_SUBMISSION};

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
pub(crate) fn handle_tx_submission(
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
                            return encode_tx_reject(&e.to_string());
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
