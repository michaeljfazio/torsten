use std::sync::Arc;
use torsten_primitives::mempool::{MempoolAddResult, MempoolProvider};
use tracing::{debug, warn};

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
    mempool: &Arc<dyn MempoolProvider>,
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
                                Ok(MempoolAddResult::Added) => {
                                    debug!("Transaction accepted into mempool: {tx_hash}");
                                    encode_tx_accept()
                                }
                                Ok(MempoolAddResult::AlreadyExists) => {
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

/// Extract transaction CBOR bytes from a MsgSubmitTx payload.
///
/// Cardano-node encodes `MsgSubmitTx [0, tx]` where `tx` is `GenTx (HardForkBlock xs)`.
/// `GenTx` is encoded by `encodeNS` as `[2, era_index, payload]`:
///   - list of length 2
///   - era_index: uint (0=Byron, 1=Shelley, …, 6=Conway)
///   - payload: `wrapCBORinCBOR toCBOR tx` = CBOR tag 24 wrapping a byte-string
///     that contains the serialized transaction CBOR
///
/// Wire format for a Conway tx submission:
///   `[0, [2, 6, tag(24, bstr(<conway_tx_cbor>))]]`
///
/// The caller has already consumed `[0, ...]` (the outer MsgSubmitTx wrapper);
/// this function decodes the inner `GenTx` value starting at the NS array.
fn extract_submitted_tx(decoder: &mut minicbor::Decoder) -> Option<(u16, Vec<u8>)> {
    // Decode the NS (N-ary sum) wrapper: [2, era_index, payload]
    // encodeNS always produces a 2-element list: [era_index, era_payload]
    let arr_len = decoder.array().ok()??;
    if arr_len != 2 {
        return None;
    }
    let era_id = decoder.u32().ok()? as u16;

    // The payload is `wrapCBORinCBOR toCBOR tx` which encodes as CBOR tag 24
    // wrapping a byte string containing the serialized transaction.
    // Per ouroboros-network Block.hs: `encode (Serialised bs)` =
    //   `encodeTag 24 <> encodeBytes (toStrict bs)`
    let tag = decoder.tag().ok()?;
    if tag != minicbor::data::Tag::new(24) {
        // Not CBOR-in-CBOR; fall back to reading raw bytes (non-standard clients)
        // Re-decode position has advanced past the tag — cannot recover here, fail.
        return None;
    }
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

/// Encode MsgRejectTx response.
///
/// Wire format: `[2, <reject>]`
/// where `<reject>` = `HardForkApplyTxErr` encoded by `dispatchEncoderErr` +
/// `encodeEitherMismatch` (HFC enabled, Right branch = `encodeListLen 1 <> enc a`):
///   array(1) header (length signals Right/Left) followed by the sole element `enc a`
/// and `enc a` = `encodeNS` = `[era_idx, <applyTxErr_cbor>]` (array of 2):
///   `[2, era_idx, <applyTxErr_cbor>]`
///
/// Full bytes: `82 02 81 82 06 <applyTxErr_cbor>`
/// where `era_idx` = 6 for Conway and `<applyTxErr_cbor>` =
/// `toEraCBOR (ApplyTxError [predicateFailures])`.
///
/// `ApplyTxError` for Conway is a newtype over `[PredicateFailure ...]`.
/// We encode an empty list `[]` as a placeholder (no detailed failure CBOR).
/// This produces a parseable but minimal rejection that cardano-cli can process.
///
/// Note: `reason` is logged but not transmitted on-wire (CBOR encoding would
/// require reproducing the full Conway predicate-failure type hierarchy).
fn encode_tx_reject(reason: &str) -> Result<Option<Segment>, N2CServerError> {
    // Log the human-readable reason for diagnostics
    tracing::debug!("LocalTxSubmission: MsgRejectTx reason={reason}");

    // Conway era index in the HFC NP list:
    // Byron=0, Shelley=1, Allegra=2, Mary=3, Alonzo=4, Babbage=5, Conway=6
    const CONWAY_ERA_IDX: u8 = 6;

    // ApplyTxError for Conway = CBOR array of predicate failures.
    // Encode as empty array [] — minimal valid encoding.
    // cardano-cli will display "Transaction rejected" with no detailed reason.
    let apply_tx_err_cbor: &[u8] = &[0x80]; // CBOR [] (empty array)

    let mut buf = Vec::new();
    {
        let mut enc = minicbor::Encoder::new(&mut buf);

        // Outer: MsgRejectTx = [2, <reject>]
        enc.array(2)
            .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
        enc.u32(2)
            .map_err(|e| N2CServerError::Protocol(e.to_string()))?;

        // encodeEitherMismatch HFC-enabled, Right branch:
        //   `encodeListLen 1 <> enc a`
        // This is a CBOR array of length 1 whose sole element is `enc a`.
        // The decoder reads the length (1 = Right, 2 = Left/mismatch).
        enc.array(1)
            .map_err(|e| N2CServerError::Protocol(e.to_string()))?;

        // encodeNS = [era_idx, <applyTxErr_cbor>] (list-of-2: index then payload)
        // This is the sole element of the length-1 array above.
        enc.array(2)
            .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
        enc.u8(CONWAY_ERA_IDX)
            .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
    } // drop enc, releasing the mutable borrow on buf

    // Emit the pre-encoded ApplyTxError CBOR bytes directly
    buf.extend_from_slice(apply_tx_err_cbor);

    Ok(Some(Segment {
        transmission_time: 0,
        protocol_id: MINI_PROTOCOL_TX_SUBMISSION,
        is_responder: true,
        payload: buf,
    }))
}
