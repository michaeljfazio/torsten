use std::sync::Arc;
use torsten_primitives::mempool::{MempoolAddResult, MempoolProvider};
use tracing::{debug, warn};

use crate::multiplexer::Segment;

use super::{N2CServerError, TxValidationError, TxValidator, MINI_PROTOCOL_TX_SUBMISSION};

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
    rejection_counter: Option<&std::sync::atomic::AtomicU64>,
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

                    // Helper: increment rejection counter when present.
                    let count_rejection = || {
                        if let Some(ctr) = rejection_counter {
                            ctr.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        }
                    };

                    // Run Phase-1/Phase-2 validation if a validator is available
                    if let Some(validator) = tx_validator {
                        if let Err(e) = validator.validate_tx(era_id, &tx_bytes) {
                            warn!("Transaction validation failed: {e}");
                            count_rejection();
                            return encode_tx_reject(&e);
                        }
                    }

                    // Parse the full transaction
                    match torsten_serialization::decode_transaction(era_id, &tx_bytes) {
                        Ok(mut tx) => {
                            // Preserve the original CBOR bytes so TxSubmission2
                            // can re-transmit the exact wire-format tx to peers.
                            // Without this, get_tx_cbor() returns None and tx
                            // bodies are silently dropped from MsgReplyTxs.
                            tx.raw_cbor = Some(tx_bytes.to_vec());
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
                                    count_rejection();
                                    encode_tx_reject_str(&e.to_string())
                                }
                            }
                        }
                        Err(e) => {
                            warn!("Failed to decode transaction: {e}");
                            count_rejection();
                            encode_tx_reject_str(&format!("Failed to decode transaction: {e}"))
                        }
                    }
                }
                None => {
                    warn!("Failed to extract transaction from submission");
                    if let Some(ctr) = rejection_counter {
                        ctr.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    encode_tx_reject_str("Failed to decode submitted transaction")
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

// Conway era index in the HFC NP list:
// Byron=0, Shelley=1, Allegra=2, Mary=3, Alonzo=4, Babbage=5, Conway=6
const CONWAY_ERA_IDX: u8 = 6;

fn cbor_err(e: minicbor::encode::Error<std::convert::Infallible>) -> N2CServerError {
    N2CServerError::Protocol(format!("CBOR encode error: {e}"))
}

/// Build the MsgRejectTx outer envelope and HFC wrapper.
///
/// Wire format:
///   `[2, array(1)[ array(2)[era_idx, <apply_tx_err_cbor>] ]]`
///
/// - `[2, ...]` = MsgRejectTx
/// - `array(1)[...]` = HFC `encodeEitherMismatch` Right branch
/// - `[era_idx, ...]` = `encodeNS` era-indexed payload
fn build_reject_envelope(apply_tx_err_cbor: &[u8]) -> Result<Option<Segment>, N2CServerError> {
    let mut buf = Vec::with_capacity(16 + apply_tx_err_cbor.len());
    {
        let mut enc = minicbor::Encoder::new(&mut buf);
        // MsgRejectTx = [2, <reject>]
        enc.array(2).map_err(cbor_err)?;
        enc.u32(2).map_err(cbor_err)?;
        // HFC Right branch: array(1)
        enc.array(1).map_err(cbor_err)?;
        // encodeNS: [era_idx, <apply_tx_err>]
        enc.array(2).map_err(cbor_err)?;
        enc.u8(CONWAY_ERA_IDX).map_err(cbor_err)?;
    }
    buf.extend_from_slice(apply_tx_err_cbor);

    Ok(Some(Segment {
        transmission_time: 0,
        protocol_id: MINI_PROTOCOL_TX_SUBMISSION,
        is_responder: true,
        payload: buf,
    }))
}

/// Encode MsgRejectTx with detailed Conway predicate failure CBOR.
///
/// Maps each [`TxValidationError`] variant to the corresponding Haskell
/// `ConwayLedgerPredFailure` constructor, matching the exact `EncCBOR` output
/// of `cardano-ledger-conway`. Errors without a direct Haskell equivalent use
/// `ConwayMempoolFailure(Text)` (tag 7) which cardano-cli displays verbatim.
///
/// ## Conway predicate failure nesting
///
/// Most validation errors follow this path:
/// ```text
/// ApplyTxError [ ConwayLedgerPredFailure ]
///   └─ ConwayUtxowFailure (tag 1)
///        └─ UtxoFailure (tag 0)
///             └─ <ConwayUtxoPredFailure variant>
/// ```
fn encode_tx_reject(error: &TxValidationError) -> Result<Option<Segment>, N2CServerError> {
    tracing::debug!("LocalTxSubmission: MsgRejectTx error={error}");

    let mut apply_tx_err = Vec::new();
    {
        let mut enc = minicbor::Encoder::new(&mut apply_tx_err);
        // ApplyTxError = CBOR array of ConwayLedgerPredFailure
        let errors = error.errors();
        enc.array(errors.len() as u64).map_err(cbor_err)?;
        for e in &errors {
            encode_conway_ledger_failure(&mut enc, e)?;
        }
    }

    build_reject_envelope(&apply_tx_err)
}

/// Encode MsgRejectTx for string-only errors (decode failures, mempool errors).
///
/// Uses `ConwayMempoolFailure(Text)` which cardano-cli displays as-is.
fn encode_tx_reject_str(reason: &str) -> Result<Option<Segment>, N2CServerError> {
    tracing::debug!("LocalTxSubmission: MsgRejectTx reason={reason}");

    let mut apply_tx_err = Vec::new();
    {
        let mut enc = minicbor::Encoder::new(&mut apply_tx_err);
        // ApplyTxError with 1 failure
        enc.array(1).map_err(cbor_err)?;
        encode_mempool_failure_text(&mut enc, reason)?;
    }

    build_reject_envelope(&apply_tx_err)
}

/// Write the `ConwayUtxowFailure(UtxoFailure(...))` prefix (tags [1, [0, ...)]).
///
/// After calling this, write the inner `ConwayUtxoPredFailure` content.
fn encode_utxo_failure_prefix(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
) -> Result<(), N2CServerError> {
    // ConwayLedgerPredFailure::ConwayUtxowFailure = [1, <inner>]
    enc.array(2).map_err(cbor_err)?;
    enc.u8(1).map_err(cbor_err)?;
    // ConwayUtxowPredFailure::UtxoFailure = [0, <inner>]
    enc.array(2).map_err(cbor_err)?;
    enc.u8(0).map_err(cbor_err)?;
    Ok(())
}

/// Encode `ConwayLedgerPredFailure::ConwayMempoolFailure(Text)` (tag 7).
fn encode_mempool_failure_text(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    text: &str,
) -> Result<(), N2CServerError> {
    enc.array(2).map_err(cbor_err)?;
    enc.u8(7).map_err(cbor_err)?;
    enc.str(text).map_err(cbor_err)?;
    Ok(())
}

/// Encode a single [`TxValidationError`] as a Conway-era `ConwayLedgerPredFailure`.
///
/// Follows the Haskell `EncCBOR` convention: each sum constructor is encoded as
/// `[tag, field1, field2, ...]`. The `Mismatch` type uses `ToGroup` (fields
/// inlined directly, no sub-array). Some variants use `swapMismatch` which
/// reverses to `[tag, expected, supplied]` on wire.
///
/// ## Tag reference
///
/// ### ConwayLedgerPredFailure
/// | Tag | Constructor              |
/// |-----|--------------------------|
/// |   1 | ConwayUtxowFailure       |
/// |   7 | ConwayMempoolFailure     |
///
/// ### ConwayUtxoPredFailure
/// | Tag | Constructor                    | Encoding (S=swap)               |
/// |-----|--------------------------------|---------------------------------|
/// |   2 | OutsideValidityIntervalUTxO    | `[2, vi, slot]`                 |
/// |   3 | MaxTxSizeUTxO                  | `[3, supplied, expected]`       |
/// |   4 | InputSetEmptyUTxO              | `[4]`                           |
/// |   5 | FeeTooSmallUTxO                | `[5, expected, supplied]` (S)   |
/// |   6 | ValueNotConservedUTxO          | `[6, supplied, expected]`       |
/// |  18 | TooManyCollateralInputs        | `[18, expected, supplied]` (S)  |
/// |  19 | NoCollateralInputs             | `[19]`                          |
fn encode_conway_ledger_failure(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    error: &TxValidationError,
) -> Result<(), N2CServerError> {
    match error {
        // ── Properly encoded Conway UTxO predicate failures ──────────
        TxValidationError::NoInputs => {
            // InputSetEmptyUTxO = [4]
            encode_utxo_failure_prefix(enc)?;
            enc.array(1).map_err(cbor_err)?;
            enc.u8(4).map_err(cbor_err)?;
            Ok(())
        }

        TxValidationError::FeeTooSmall { minimum, actual } => {
            // FeeTooSmallUTxO uses ToGroup(swapMismatch): [5, expected, supplied]
            // swapMismatch reverses to (expected=minimum, supplied=actual)
            encode_utxo_failure_prefix(enc)?;
            enc.array(3).map_err(cbor_err)?;
            enc.u8(5).map_err(cbor_err)?;
            enc.u64(*minimum).map_err(cbor_err)?; // expected (swapped first)
            enc.u64(*actual).map_err(cbor_err)?; // supplied (swapped second)
            Ok(())
        }

        TxValidationError::TxTooLarge { maximum, actual } => {
            // MaxTxSizeUTxO uses ToGroup (no swap): [3, supplied, expected]
            encode_utxo_failure_prefix(enc)?;
            enc.array(3).map_err(cbor_err)?;
            enc.u8(3).map_err(cbor_err)?;
            enc.u64(*actual).map_err(cbor_err)?; // supplied
            enc.u64(*maximum).map_err(cbor_err)?; // expected
            Ok(())
        }

        TxValidationError::TtlExpired { current_slot, ttl } => {
            // OutsideValidityIntervalUTxO(ValidityInterval, SlotNo)
            // ValidityInterval = [invalidBefore=null, invalidHereafter=ttl]
            // = [2, [null, ttl], current_slot]
            encode_utxo_failure_prefix(enc)?;
            enc.array(3).map_err(cbor_err)?;
            enc.u8(2).map_err(cbor_err)?;
            // ValidityInterval: [SNothing, SJust ttl]
            enc.array(2).map_err(cbor_err)?;
            enc.null().map_err(cbor_err)?;
            enc.u64(*ttl).map_err(cbor_err)?;
            // SlotNo
            enc.u64(*current_slot).map_err(cbor_err)?;
            Ok(())
        }

        TxValidationError::NotYetValid {
            current_slot,
            valid_from,
        } => {
            // OutsideValidityIntervalUTxO(ValidityInterval, SlotNo)
            // ValidityInterval = [invalidBefore=valid_from, invalidHereafter=null]
            encode_utxo_failure_prefix(enc)?;
            enc.array(3).map_err(cbor_err)?;
            enc.u8(2).map_err(cbor_err)?;
            enc.array(2).map_err(cbor_err)?;
            enc.u64(*valid_from).map_err(cbor_err)?;
            enc.null().map_err(cbor_err)?;
            enc.u64(*current_slot).map_err(cbor_err)?;
            Ok(())
        }

        TxValidationError::ValueNotConserved {
            inputs,
            outputs,
            fee,
        } => {
            // ValueNotConservedUTxO uses ToGroup (no swap): [6, supplied, expected]
            // supplied=consumed=inputs, expected=produced=outputs+fee
            // ADA-only Value = plain Coin integer
            encode_utxo_failure_prefix(enc)?;
            enc.array(3).map_err(cbor_err)?;
            enc.u8(6).map_err(cbor_err)?;
            enc.u64(*inputs).map_err(cbor_err)?; // supplied (consumed)
            enc.u64(outputs.saturating_add(*fee)).map_err(cbor_err)?; // expected (produced)
            Ok(())
        }

        TxValidationError::TooManyCollateralInputs { max, actual } => {
            // TooManyCollateralInputs uses ToGroup(swapMismatch): [18, expected, supplied]
            encode_utxo_failure_prefix(enc)?;
            enc.array(3).map_err(cbor_err)?;
            enc.u8(18).map_err(cbor_err)?;
            enc.u64(*max).map_err(cbor_err)?; // expected (swapped first)
            enc.u64(*actual).map_err(cbor_err)?; // supplied (swapped second)
            Ok(())
        }

        TxValidationError::InsufficientCollateral => {
            // NoCollateralInputs = [19] (closest match without amount data)
            encode_utxo_failure_prefix(enc)?;
            enc.array(1).map_err(cbor_err)?;
            enc.u8(19).map_err(cbor_err)?;
            Ok(())
        }

        // ── Flatten Multiple variant ─────────────────────────────────
        TxValidationError::Multiple(errors) => {
            // Should not normally be reached (flattened via errors()),
            // but handle gracefully by encoding the first error.
            if let Some(first) = errors.first() {
                encode_conway_ledger_failure(enc, first)
            } else {
                encode_mempool_failure_text(enc, "Unknown validation error")
            }
        }

        // ── All other errors → ConwayMempoolFailure(Text) ───────────
        other => encode_mempool_failure_text(enc, &other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decode CBOR and verify basic structure of MsgRejectTx envelope.
    /// Returns the inner ApplyTxError bytes for further inspection.
    fn decode_reject_envelope(payload: &[u8]) -> Vec<u8> {
        let mut dec = minicbor::Decoder::new(payload);
        // [2, <reject>]
        let outer_len = dec.array().unwrap().unwrap();
        assert_eq!(outer_len, 2);
        assert_eq!(dec.u32().unwrap(), 2); // MsgRejectTx tag

        // HFC Right branch: array(1)
        let hfc_len = dec.array().unwrap().unwrap();
        assert_eq!(hfc_len, 1);

        // encodeNS: [era_idx, <apply_tx_err>]
        let ns_len = dec.array().unwrap().unwrap();
        assert_eq!(ns_len, 2);
        let era_idx = dec.u8().unwrap();
        assert_eq!(era_idx, CONWAY_ERA_IDX);

        // Return remaining bytes (the ApplyTxError CBOR)
        payload[dec.position()..].to_vec()
    }

    /// Verify the ApplyTxError is an array of the expected length.
    fn decode_apply_tx_err(cbor: &[u8]) -> (u64, usize) {
        let mut dec = minicbor::Decoder::new(cbor);
        let len = dec.array().unwrap().unwrap();
        (len, dec.position())
    }

    /// Decode a ConwayLedgerPredFailure and return (tag, remaining_position).
    fn decode_ledger_failure(cbor: &[u8], offset: usize) -> (u8, usize) {
        let mut dec = minicbor::Decoder::new(cbor);
        dec.set_position(offset);
        let _len = dec.array().unwrap().unwrap();
        let tag = dec.u8().unwrap();
        (tag, dec.position())
    }

    #[test]
    fn test_reject_no_inputs() {
        let err = TxValidationError::NoInputs;
        let result = encode_tx_reject(&err).unwrap().unwrap();
        let inner = decode_reject_envelope(&result.payload);
        let (count, pos) = decode_apply_tx_err(&inner);
        assert_eq!(count, 1);

        // ConwayLedgerPredFailure tag 1 = ConwayUtxowFailure
        let (tag, pos) = decode_ledger_failure(&inner, pos);
        assert_eq!(tag, 1);
        // ConwayUtxowPredFailure tag 0 = UtxoFailure
        let (tag, pos) = decode_ledger_failure(&inner, pos);
        assert_eq!(tag, 0);
        // ConwayUtxoPredFailure tag 4 = InputSetEmptyUTxO
        let mut dec = minicbor::Decoder::new(&inner);
        dec.set_position(pos);
        let len = dec.array().unwrap().unwrap();
        assert_eq!(len, 1);
        assert_eq!(dec.u8().unwrap(), 4);
    }

    #[test]
    fn test_reject_fee_too_small() {
        let err = TxValidationError::FeeTooSmall {
            minimum: 200_000,
            actual: 100_000,
        };
        let result = encode_tx_reject(&err).unwrap().unwrap();
        let inner = decode_reject_envelope(&result.payload);
        let (count, pos) = decode_apply_tx_err(&inner);
        assert_eq!(count, 1);

        // Navigate: ConwayUtxowFailure(1) → UtxoFailure(0) → FeeTooSmallUTxO(5)
        let (tag, pos) = decode_ledger_failure(&inner, pos);
        assert_eq!(tag, 1);
        let (tag, pos) = decode_ledger_failure(&inner, pos);
        assert_eq!(tag, 0);

        let mut dec = minicbor::Decoder::new(&inner);
        dec.set_position(pos);
        let len = dec.array().unwrap().unwrap();
        // ToGroup(swapMismatch): [5, expected, supplied] = array(3)
        assert_eq!(len, 3);
        assert_eq!(dec.u8().unwrap(), 5);
        assert_eq!(dec.u64().unwrap(), 200_000); // expected (minimum) — swapped first
        assert_eq!(dec.u64().unwrap(), 100_000); // supplied (actual) — swapped second
    }

    #[test]
    fn test_reject_tx_too_large() {
        let err = TxValidationError::TxTooLarge {
            maximum: 16_384,
            actual: 20_000,
        };
        let result = encode_tx_reject(&err).unwrap().unwrap();
        let inner = decode_reject_envelope(&result.payload);
        let (count, pos) = decode_apply_tx_err(&inner);
        assert_eq!(count, 1);

        let (tag, pos) = decode_ledger_failure(&inner, pos);
        assert_eq!(tag, 1); // ConwayUtxowFailure
        let (tag, pos) = decode_ledger_failure(&inner, pos);
        assert_eq!(tag, 0); // UtxoFailure

        let mut dec = minicbor::Decoder::new(&inner);
        dec.set_position(pos);
        // ToGroup (no swap): [3, supplied, expected] = array(3)
        assert_eq!(dec.array().unwrap().unwrap(), 3);
        assert_eq!(dec.u8().unwrap(), 3); // MaxTxSizeUTxO
        assert_eq!(dec.u64().unwrap(), 20_000); // supplied (actual)
        assert_eq!(dec.u64().unwrap(), 16_384); // expected (maximum)
    }

    #[test]
    fn test_reject_ttl_expired() {
        let err = TxValidationError::TtlExpired {
            current_slot: 1000,
            ttl: 500,
        };
        let result = encode_tx_reject(&err).unwrap().unwrap();
        let inner = decode_reject_envelope(&result.payload);
        let (_, pos) = decode_apply_tx_err(&inner);

        let (tag, pos) = decode_ledger_failure(&inner, pos);
        assert_eq!(tag, 1); // ConwayUtxowFailure
        let (tag, pos) = decode_ledger_failure(&inner, pos);
        assert_eq!(tag, 0); // UtxoFailure

        let mut dec = minicbor::Decoder::new(&inner);
        dec.set_position(pos);
        assert_eq!(dec.array().unwrap().unwrap(), 3); // [2, vi, slot]
        assert_eq!(dec.u8().unwrap(), 2); // OutsideValidityIntervalUTxO

        // ValidityInterval: [null, ttl]
        assert_eq!(dec.array().unwrap().unwrap(), 2);
        dec.null().unwrap(); // invalidBefore = SNothing
        assert_eq!(dec.u64().unwrap(), 500); // invalidHereafter = ttl

        // SlotNo
        assert_eq!(dec.u64().unwrap(), 1000);
    }

    #[test]
    fn test_reject_not_yet_valid() {
        let err = TxValidationError::NotYetValid {
            current_slot: 100,
            valid_from: 500,
        };
        let result = encode_tx_reject(&err).unwrap().unwrap();
        let inner = decode_reject_envelope(&result.payload);
        let (_, pos) = decode_apply_tx_err(&inner);

        let (tag, pos) = decode_ledger_failure(&inner, pos);
        assert_eq!(tag, 1);
        let (tag, pos) = decode_ledger_failure(&inner, pos);
        assert_eq!(tag, 0);

        let mut dec = minicbor::Decoder::new(&inner);
        dec.set_position(pos);
        assert_eq!(dec.array().unwrap().unwrap(), 3);
        assert_eq!(dec.u8().unwrap(), 2); // OutsideValidityIntervalUTxO

        // ValidityInterval: [valid_from, null]
        assert_eq!(dec.array().unwrap().unwrap(), 2);
        assert_eq!(dec.u64().unwrap(), 500); // invalidBefore = valid_from
        dec.null().unwrap(); // invalidHereafter = SNothing

        assert_eq!(dec.u64().unwrap(), 100); // current slot
    }

    #[test]
    fn test_reject_value_not_conserved() {
        let err = TxValidationError::ValueNotConserved {
            inputs: 5_000_000,
            outputs: 4_000_000,
            fee: 500_000,
        };
        let result = encode_tx_reject(&err).unwrap().unwrap();
        let inner = decode_reject_envelope(&result.payload);
        let (_, pos) = decode_apply_tx_err(&inner);

        let (tag, pos) = decode_ledger_failure(&inner, pos);
        assert_eq!(tag, 1);
        let (tag, pos) = decode_ledger_failure(&inner, pos);
        assert_eq!(tag, 0);

        let mut dec = minicbor::Decoder::new(&inner);
        dec.set_position(pos);
        // ToGroup (no swap): [6, supplied, expected] = array(3)
        assert_eq!(dec.array().unwrap().unwrap(), 3);
        assert_eq!(dec.u8().unwrap(), 6); // ValueNotConservedUTxO
        assert_eq!(dec.u64().unwrap(), 5_000_000); // supplied (consumed = inputs)
        assert_eq!(dec.u64().unwrap(), 4_500_000); // expected (produced = outputs + fee)
    }

    #[test]
    fn test_reject_too_many_collateral_inputs() {
        let err = TxValidationError::TooManyCollateralInputs { max: 3, actual: 5 };
        let result = encode_tx_reject(&err).unwrap().unwrap();
        let inner = decode_reject_envelope(&result.payload);
        let (_, pos) = decode_apply_tx_err(&inner);

        let (tag, pos) = decode_ledger_failure(&inner, pos);
        assert_eq!(tag, 1);
        let (tag, pos) = decode_ledger_failure(&inner, pos);
        assert_eq!(tag, 0);

        let mut dec = minicbor::Decoder::new(&inner);
        dec.set_position(pos);
        // ToGroup(swapMismatch): [18, expected, supplied] = array(3)
        assert_eq!(dec.array().unwrap().unwrap(), 3);
        assert_eq!(dec.u8().unwrap(), 18); // TooManyCollateralInputs
        assert_eq!(dec.u64().unwrap(), 3); // expected (max) — swapped first
        assert_eq!(dec.u64().unwrap(), 5); // supplied (actual) — swapped second
    }

    #[test]
    fn test_reject_insufficient_collateral() {
        let err = TxValidationError::InsufficientCollateral;
        let result = encode_tx_reject(&err).unwrap().unwrap();
        let inner = decode_reject_envelope(&result.payload);
        let (_, pos) = decode_apply_tx_err(&inner);

        let (tag, pos) = decode_ledger_failure(&inner, pos);
        assert_eq!(tag, 1);
        let (tag, pos) = decode_ledger_failure(&inner, pos);
        assert_eq!(tag, 0);

        let mut dec = minicbor::Decoder::new(&inner);
        dec.set_position(pos);
        assert_eq!(dec.array().unwrap().unwrap(), 1);
        assert_eq!(dec.u8().unwrap(), 19); // NoCollateralInputs
    }

    #[test]
    fn test_reject_fallback_uses_mempool_failure() {
        // Errors without direct Haskell equivalents use ConwayMempoolFailure
        let err = TxValidationError::InputNotFound {
            input: "abc123#0".into(),
        };
        let result = encode_tx_reject(&err).unwrap().unwrap();
        let inner = decode_reject_envelope(&result.payload);
        let (count, pos) = decode_apply_tx_err(&inner);
        assert_eq!(count, 1);

        // ConwayLedgerPredFailure tag 7 = ConwayMempoolFailure
        let mut dec = minicbor::Decoder::new(&inner);
        dec.set_position(pos);
        assert_eq!(dec.array().unwrap().unwrap(), 2);
        assert_eq!(dec.u8().unwrap(), 7);
        let text = dec.str().unwrap();
        assert!(text.contains("abc123#0"));
    }

    #[test]
    fn test_reject_str_encodes_as_mempool_failure() {
        let result = encode_tx_reject_str("Mempool is full").unwrap().unwrap();
        let inner = decode_reject_envelope(&result.payload);
        let (count, pos) = decode_apply_tx_err(&inner);
        assert_eq!(count, 1);

        let mut dec = minicbor::Decoder::new(&inner);
        dec.set_position(pos);
        assert_eq!(dec.array().unwrap().unwrap(), 2);
        assert_eq!(dec.u8().unwrap(), 7); // ConwayMempoolFailure
        assert_eq!(dec.str().unwrap(), "Mempool is full");
    }

    #[test]
    fn test_reject_multiple_errors() {
        let err = TxValidationError::Multiple(vec![
            TxValidationError::NoInputs,
            TxValidationError::FeeTooSmall {
                minimum: 200_000,
                actual: 100_000,
            },
        ]);
        let result = encode_tx_reject(&err).unwrap().unwrap();
        let inner = decode_reject_envelope(&result.payload);
        let (count, _) = decode_apply_tx_err(&inner);
        // Multiple errors → multiple predicate failures in the array
        assert_eq!(count, 2);
    }

    #[test]
    fn test_reject_envelope_structure() {
        // Verify exact bytes of a minimal rejection
        let err = TxValidationError::NoInputs;
        let result = encode_tx_reject(&err).unwrap().unwrap();
        let payload = &result.payload;

        // First bytes: 82 02 = [2, ...]
        assert_eq!(payload[0], 0x82); // array(2)
        assert_eq!(payload[1], 0x02); // MsgRejectTx tag

        // HFC wrapper: 81 = array(1)
        assert_eq!(payload[2], 0x81);

        // encodeNS: 82 06 = [6, ...]
        assert_eq!(payload[3], 0x82); // array(2)
        assert_eq!(payload[4], 0x06); // Conway era index

        assert_eq!(result.protocol_id, MINI_PROTOCOL_TX_SUBMISSION);
        assert!(result.is_responder);
    }
}
