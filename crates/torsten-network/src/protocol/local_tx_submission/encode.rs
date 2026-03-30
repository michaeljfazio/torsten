//! CBOR encoding for `ApplyTxErr` — structured rejection reasons matching Haskell cardano-node.
//!
//! When a transaction is rejected via LocalTxSubmission, the Haskell node sends a structured
//! CBOR encoding of `ApplyTxErr` containing era-specific predicate failures. This module
//! encodes `TxValidationError` into the same wire format so that `cardano-cli` and other
//! standard Cardano tools can parse rejection reasons.
//!
//! ## Wire format
//!
//! The `ApplyTxErr` payload (inside `MsgRejectTx = [2, payload]`) is:
//! ```text
//! [[era_id, [failure_0, failure_1, ...]]]
//! ```
//!
//! Each failure is nested three levels deep for Conway UTxO errors:
//! ```text
//! ConwayLedgerPredFailure(tag=1) → ConwayUtxowPredFailure(tag=0) → ConwayUtxoPredFailure(tag=N)
//! ```
//!
//! ## References
//!
//! - `cardano-ledger/eras/conway/impl/src/Cardano/Ledger/Conway/Rules/Utxo.hs`
//! - `cardano-ledger/eras/conway/impl/src/Cardano/Ledger/Conway/Rules/Utxow.hs`
//! - `cardano-ledger/eras/conway/impl/src/Cardano/Ledger/Conway/Rules/Ledger.hs`

use minicbor::Encoder;

use crate::TxValidationError;

/// CBOR tag 258 — marks a CBOR array as a mathematical set (sorted, no duplicates).
/// Required by Conway-era encoding for sets of TxIn, KeyHash, ScriptHash, etc.
const CBOR_TAG_SET: u64 = 258;

/// Encode a `TxValidationError` into the `ApplyTxErr` CBOR payload.
///
/// The returned bytes represent the full `ApplyTxErr` structure:
/// `[[era_id, [failure_0, failure_1, ...]]]`
///
/// This is appended directly after the `[2, ...]` MsgRejectTx tag in the server.
pub fn encode_apply_tx_err(error: &TxValidationError, era_id: u16) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut enc = Encoder::new(&mut buf);

    // Collect all failures (flatten Multiple variant)
    let errors = flatten_errors(error);

    // Outer HFC wrapper: array(1) containing the era-tagged payload
    enc.array(1).expect("infallible");

    // Era-tagged payload: [era_id, [failure_0, failure_1, ...]]
    enc.array(2).expect("infallible");
    enc.u16(era_id).expect("infallible");

    // Array of ConwayLedgerPredFailure items
    enc.array(errors.len() as u64).expect("infallible");
    for err in &errors {
        encode_conway_ledger_pred_failure(&mut enc, err);
    }

    buf
}

/// Flatten a `TxValidationError` into a list of individual errors.
/// The `Multiple` variant is recursively expanded.
fn flatten_errors(error: &TxValidationError) -> Vec<&TxValidationError> {
    match error {
        TxValidationError::Multiple(errors) => errors.iter().flat_map(flatten_errors).collect(),
        other => vec![other],
    }
}

/// Encode a single `TxValidationError` as a `ConwayLedgerPredFailure`.
///
/// Most validation errors map to:
///   `ConwayLedgerPredFailure::ConwayUtxowFailure(tag=1)`
///     → `ConwayUtxowPredFailure::UtxoFailure(tag=0)`
///       → `ConwayUtxoPredFailure(tag=N, fields...)`
///
/// Witness-level errors skip the Utxo layer:
///   `ConwayLedgerPredFailure::ConwayUtxowFailure(tag=1)`
///     → `ConwayUtxowPredFailure(tag=N, fields...)`
///
/// Unmapped errors fall back to:
///   `ConwayLedgerPredFailure::ConwayMempoolFailure(tag=7, text)`
fn encode_conway_ledger_pred_failure(enc: &mut Encoder<&mut Vec<u8>>, err: &TxValidationError) {
    match err {
        // ── UTxO-level failures: Ledger(1) → Utxow(0) → Utxo(tag) ──

        // Tag 1: BadInputsUTxO — set of missing/bad inputs
        TxValidationError::InputNotFound { input }
        | TxValidationError::DuplicateInput { input } => {
            if let Some((tx_hash, tx_ix)) = parse_tx_input(input) {
                encode_utxo_failure(enc, 1, |enc| {
                    // tag(258) array(1)[ [tx_hash_bytes, tx_ix] ]
                    enc.tag(minicbor::data::Tag::new(CBOR_TAG_SET))
                        .expect("infallible");
                    enc.array(1).expect("infallible");
                    enc.array(2).expect("infallible");
                    enc.bytes(&tx_hash).expect("infallible");
                    enc.u32(tx_ix).expect("infallible");
                });
            } else {
                encode_mempool_fallback(enc, &format!("{err:?}"));
            }
        }

        // Tag 2: OutsideValidityIntervalUTxO — [validity_interval, current_slot]
        TxValidationError::TtlExpired { current_slot, ttl } => {
            encode_utxo_failure(enc, 2, |enc| {
                // ValidityInterval: array(2)[ SNothing (lower), SJust ttl (upper) ]
                enc.array(2).expect("infallible");
                // SNothing = array(0)
                enc.array(0).expect("infallible");
                // SJust ttl = array(1)[ ttl ]
                enc.array(1).expect("infallible");
                enc.u64(*ttl).expect("infallible");
                // current_slot
                enc.u64(*current_slot).expect("infallible");
            });
        }
        TxValidationError::NotYetValid {
            current_slot,
            valid_from,
        } => {
            encode_utxo_failure(enc, 2, |enc| {
                // ValidityInterval: array(2)[ SJust valid_from (lower), SNothing (upper) ]
                enc.array(2).expect("infallible");
                // SJust valid_from = array(1)[ valid_from ]
                enc.array(1).expect("infallible");
                enc.u64(*valid_from).expect("infallible");
                // SNothing = array(0)
                enc.array(0).expect("infallible");
                // current_slot
                enc.u64(*current_slot).expect("infallible");
            });
        }

        // Tag 3: MaxTxSizeUTxO — [supplied (actual), expected (max)] (no swap)
        TxValidationError::TxTooLarge { maximum, actual } => {
            encode_utxo_failure(enc, 3, |enc| {
                enc.u64(*actual).expect("infallible");
                enc.u64(*maximum).expect("infallible");
            });
        }

        // Tag 4: InputSetEmptyUTxO — no fields
        TxValidationError::NoInputs => {
            encode_utxo_failure(enc, 4, |_enc| {});
        }

        // Tag 5: FeeTooSmallUTxO — [expected (min), supplied (actual)] (swapped)
        TxValidationError::FeeTooSmall { minimum, actual } => {
            encode_utxo_failure(enc, 5, |enc| {
                enc.u64(*minimum).expect("infallible");
                enc.u64(*actual).expect("infallible");
            });
        }

        // Tag 6: ValueNotConservedUTxO — [consumed, produced] (no swap)
        // consumed = sum of input values, produced = outputs + fee
        TxValidationError::ValueNotConserved {
            inputs,
            outputs,
            fee,
        } => {
            let consumed = *inputs;
            let produced = outputs.saturating_add(*fee);
            encode_utxo_failure(enc, 6, |enc| {
                // Coin values encoded as uint (ADA-only)
                enc.u64(consumed).expect("infallible");
                enc.u64(produced).expect("infallible");
            });
        }

        // Tag 12: InsufficientCollateral — [balance_delta (DeltaCoin), required (Coin)]
        // DeltaCoin is a signed integer; Coin is unsigned.
        TxValidationError::CollateralMismatch { declared, computed } => {
            encode_utxo_failure(enc, 20, |enc| {
                // IncorrectTotalCollateralField: [delta_coin_int, declared_coin_uint]
                // delta = computed - declared (signed)
                let delta = (*computed as i64) - (*declared as i64);
                enc.i64(delta).expect("infallible");
                enc.u64(*declared).expect("infallible");
            });
        }

        // Tag 18: TooManyCollateralInputs — [max_allowed, actual_count] (swapped)
        TxValidationError::TooManyCollateralInputs { max, actual } => {
            encode_utxo_failure(enc, 18, |enc| {
                enc.u64(*max).expect("infallible");
                enc.u64(*actual).expect("infallible");
            });
        }

        // Tag 19: NoCollateralInputs — no fields
        // Note: We can't distinguish "no collateral" from "collateral not found" in
        // the current TxValidationError enum. CollateralNotFound falls through to mempool.

        // Tag 22: BabbageNonDisjointRefInputs
        TxValidationError::ReferenceInputOverlapsInput { input } => {
            if let Some((tx_hash, tx_ix)) = parse_tx_input(input) {
                encode_utxo_failure(enc, 22, |enc| {
                    // NonEmpty set of overlapping TxIn
                    enc.tag(minicbor::data::Tag::new(CBOR_TAG_SET))
                        .expect("infallible");
                    enc.array(1).expect("infallible");
                    enc.array(2).expect("infallible");
                    enc.bytes(&tx_hash).expect("infallible");
                    enc.u32(tx_ix).expect("infallible");
                });
            } else {
                encode_mempool_fallback(enc, &format!("{err:?}"));
            }
        }

        // ── Witness-level failures: Ledger(1) → Utxow(tag) ──

        // Utxow tag 1: InvalidWitnessesUTXOW — [vkey_bytes...]
        TxValidationError::InvalidWitnessSignature { vkey } => {
            if let Some(vkey_bytes) = parse_hex_bytes(vkey) {
                encode_utxow_failure(enc, 1, |enc| {
                    enc.array(1).expect("infallible");
                    enc.bytes(&vkey_bytes).expect("infallible");
                });
            } else {
                encode_mempool_fallback(enc, &format!("{err:?}"));
            }
        }

        // Utxow tag 2: MissingVKeyWitnessesUTXOW — tag(258) set of keyhash bytes(28)
        TxValidationError::MissingInputWitness { credential }
        | TxValidationError::MissingCertificateWitness { credential }
        | TxValidationError::MissingWithdrawalWitness { credential } => {
            if let Some(keyhash) = parse_hex_bytes(credential) {
                encode_utxow_failure(enc, 2, |enc| {
                    enc.tag(minicbor::data::Tag::new(CBOR_TAG_SET))
                        .expect("infallible");
                    enc.array(1).expect("infallible");
                    enc.bytes(&keyhash).expect("infallible");
                });
            } else {
                encode_mempool_fallback(enc, &format!("{err:?}"));
            }
        }

        // Utxow tag 3: MissingScriptWitnessesUTXOW — tag(258) set of script hashes
        TxValidationError::MissingScriptWitness { credential }
        | TxValidationError::MissingWithdrawalScriptWitness { credential } => {
            if let Some(script_hash) = parse_hex_bytes(credential) {
                encode_utxow_failure(enc, 3, |enc| {
                    enc.tag(minicbor::data::Tag::new(CBOR_TAG_SET))
                        .expect("infallible");
                    enc.array(1).expect("infallible");
                    enc.bytes(&script_hash).expect("infallible");
                });
            } else {
                encode_mempool_fallback(enc, &format!("{err:?}"));
            }
        }

        // Utxow tag 5: MissingTxBodyMetadataHash
        TxValidationError::AuxiliaryDataWithoutHash => {
            // We don't have the expected hash, but the tag structure is [5, hash_bytes].
            // Fall back to mempool since we lack the actual metadata hash.
            encode_mempool_fallback(
                enc,
                "AuxiliaryDataWithoutHash: auxiliary data present but no hash in tx body",
            );
        }

        // Utxow tag 6: MissingTxMetadata
        TxValidationError::AuxiliaryDataHashWithoutData => {
            // We don't have the declared hash. Fall back to mempool.
            encode_mempool_fallback(
                enc,
                "AuxiliaryDataHashWithoutData: metadata hash declared but no auxiliary data",
            );
        }

        // Utxow tag 13: PPViewHashesDontMatch — [supplied_hash_or_null, expected_hash_or_null]
        TxValidationError::ScriptDataHashMismatch { expected, actual } => {
            encode_utxow_failure(enc, 13, |enc| {
                // supplied (actual from tx) — StrictMaybe encoding
                if let Some(hash_bytes) = parse_hex_bytes(actual) {
                    enc.array(1).expect("infallible");
                    enc.bytes(&hash_bytes).expect("infallible");
                } else {
                    enc.array(0).expect("infallible");
                }
                // expected (computed from script context) — StrictMaybe encoding
                if let Some(hash_bytes) = parse_hex_bytes(expected) {
                    enc.array(1).expect("infallible");
                    enc.bytes(&hash_bytes).expect("infallible");
                } else {
                    enc.array(0).expect("infallible");
                }
            });
        }

        // Utxow tag 13: PPViewHashesDontMatch — unexpected hash present
        TxValidationError::UnexpectedScriptDataHash => {
            encode_utxow_failure(enc, 13, |enc| {
                // supplied = SJust (some hash, but we don't have it — encode as present-but-unknown)
                // expected = SNothing
                // Since we lack the actual hash bytes, fall back:
                enc.array(0).expect("infallible"); // supplied unknown
                enc.array(0).expect("infallible"); // expected nothing
            });
        }

        // Utxow tag 13: PPViewHashesDontMatch — required hash missing
        TxValidationError::MissingScriptDataHash => {
            encode_utxow_failure(enc, 13, |enc| {
                // supplied = SNothing (tx didn't include hash)
                enc.array(0).expect("infallible");
                // expected = SJust (some hash, but we don't have bytes)
                enc.array(0).expect("infallible");
            });
        }

        // ── Ledger-level failures ──

        // Ledger tag 5: ConwayTreasuryValueMismatch (swapped: [expected, supplied])
        // Note: This variant currently maps to ScriptFailed in serve.rs, so it won't
        // reach here. But if TxValidationError is extended, this handles it.

        // ── Fallback for all unmapped variants ──
        // ConwayMempoolFailure (Ledger tag 7): [7, "descriptive text"]
        _ => {
            encode_mempool_fallback(enc, &format!("{err:?}"));
        }
    }
}

// ── Encoding helpers ──

/// Encode a `ConwayUtxoPredFailure` wrapped in the full three-level nesting:
/// `[1, [0, [tag, fields...]]]`
///
/// The closure `encode_fields` writes the fields for the specific `ConwayUtxoPredFailure`
/// variant. The tag and surrounding arrays are handled by this function.
fn encode_utxo_failure(
    enc: &mut Encoder<&mut Vec<u8>>,
    utxo_tag: u8,
    encode_fields: impl FnOnce(&mut Encoder<&mut Vec<u8>>),
) {
    // Count the fields that will be written by the closure.
    // We use a temporary buffer to determine the count.
    let mut field_buf = Vec::new();
    let mut field_enc = Encoder::new(&mut field_buf);
    encode_fields(&mut field_enc);

    // Count top-level CBOR items in field_buf
    let field_count = count_cbor_items(&field_buf);

    // ConwayLedgerPredFailure: array(2)[1, utxow_payload]
    enc.array(2).expect("infallible");
    enc.u8(1).expect("infallible"); // tag 1: ConwayUtxowFailure

    // ConwayUtxowPredFailure: array(2)[0, utxo_payload]
    enc.array(2).expect("infallible");
    enc.u8(0).expect("infallible"); // tag 0: UtxoFailure

    // ConwayUtxoPredFailure: array(N+1)[utxo_tag, fields...]
    enc.array((field_count + 1) as u64).expect("infallible");
    enc.u8(utxo_tag).expect("infallible");

    // Write the pre-encoded field bytes directly
    let writer = enc.writer_mut();
    writer.extend_from_slice(&field_buf);
}

/// Encode a `ConwayUtxowPredFailure` wrapped in the Ledger nesting:
/// `[1, [tag, fields...]]`
///
/// Used for witness-level errors that don't go through the Utxo layer.
fn encode_utxow_failure(
    enc: &mut Encoder<&mut Vec<u8>>,
    utxow_tag: u8,
    encode_fields: impl FnOnce(&mut Encoder<&mut Vec<u8>>),
) {
    // Count fields via temporary buffer
    let mut field_buf = Vec::new();
    let mut field_enc = Encoder::new(&mut field_buf);
    encode_fields(&mut field_enc);
    let field_count = count_cbor_items(&field_buf);

    // ConwayLedgerPredFailure: array(2)[1, utxow_payload]
    enc.array(2).expect("infallible");
    enc.u8(1).expect("infallible"); // tag 1: ConwayUtxowFailure

    // ConwayUtxowPredFailure: array(N+1)[utxow_tag, fields...]
    enc.array((field_count + 1) as u64).expect("infallible");
    enc.u8(utxow_tag).expect("infallible");

    // Write the pre-encoded field bytes directly
    let writer = enc.writer_mut();
    writer.extend_from_slice(&field_buf);
}

/// Encode a `ConwayMempoolFailure` (Ledger tag 7) with a text description.
/// Used as fallback for error variants that can't be mapped to structured CBOR.
fn encode_mempool_fallback(enc: &mut Encoder<&mut Vec<u8>>, text: &str) {
    // ConwayLedgerPredFailure: array(2)[7, text]
    enc.array(2).expect("infallible");
    enc.u8(7).expect("infallible"); // tag 7: ConwayMempoolFailure
    enc.str(text).expect("infallible");
}

// ── Parsing helpers ──

/// Parse a transaction input string in the format `"hex_txhash#index"` into
/// a 32-byte hash and output index.
///
/// Returns `None` if the format is invalid.
fn parse_tx_input(s: &str) -> Option<([u8; 32], u32)> {
    let (hash_hex, idx_str) = s.rsplit_once('#')?;
    let idx: u32 = idx_str.parse().ok()?;
    let hash_bytes = parse_hex_bytes(hash_hex)?;
    if hash_bytes.len() != 32 {
        return None;
    }
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&hash_bytes);
    Some((hash, idx))
}

/// Parse a hex string into raw bytes. Returns `None` if the string has odd length
/// or contains non-hex characters.
fn parse_hex_bytes(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// Count the number of top-level CBOR data items in a byte buffer.
/// Used to determine array lengths for the `Sum` encoding pattern.
fn count_cbor_items(buf: &[u8]) -> usize {
    if buf.is_empty() {
        return 0;
    }
    let mut dec = minicbor::Decoder::new(buf);
    let mut count = 0;
    while dec.position() < buf.len() {
        if dec.skip().is_err() {
            break;
        }
        count += 1;
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use minicbor::Decoder;

    // ── Helper: decode the outer structure and return the inner failure CBOR ──

    /// Decode `[[era_id, [failure_0, ...]]]` and return era_id + number of failures.
    fn decode_outer(bytes: &[u8]) -> (u16, u64) {
        let mut dec = Decoder::new(bytes);
        let outer_len = dec.array().unwrap().unwrap();
        assert_eq!(outer_len, 1, "outer HFC wrapper must be array(1)");

        let inner_len = dec.array().unwrap().unwrap();
        assert_eq!(inner_len, 2, "inner must be array(2) [era_id, failures]");

        let era_id = dec.u16().unwrap();
        let n_failures = dec.array().unwrap().unwrap();
        (era_id, n_failures)
    }

    /// Decode a ConwayLedgerPredFailure and return (ledger_tag, remaining_decoder_position).
    fn decode_ledger_tag(dec: &mut Decoder<'_>) -> u8 {
        let _arr = dec.array().unwrap();
        dec.u8().unwrap()
    }

    /// Decode ConwayUtxowPredFailure tag from within a Ledger(1) wrapper.
    fn decode_utxow_tag(dec: &mut Decoder<'_>) -> u8 {
        let _arr = dec.array().unwrap();
        dec.u8().unwrap()
    }

    /// Decode ConwayUtxoPredFailure tag from within Utxow(0) wrapper.
    fn decode_utxo_tag(dec: &mut Decoder<'_>) -> u8 {
        let _arr = dec.array().unwrap();
        dec.u8().unwrap()
    }

    // ── Parsing tests ──

    #[test]
    fn test_parse_tx_input_valid() {
        let hash_hex = "a".repeat(64); // 32 bytes of 0xaa
        let input = format!("{hash_hex}#42");
        let (hash, idx) = parse_tx_input(&input).unwrap();
        assert_eq!(idx, 42);
        assert_eq!(hash, [0xaa; 32]);
    }

    #[test]
    fn test_parse_tx_input_invalid_no_hash() {
        assert!(parse_tx_input("#3").is_none());
    }

    #[test]
    fn test_parse_tx_input_invalid_no_index() {
        let hash_hex = "a".repeat(64);
        assert!(parse_tx_input(&hash_hex).is_none());
    }

    #[test]
    fn test_parse_tx_input_invalid_index() {
        let hash_hex = "a".repeat(64);
        let input = format!("{hash_hex}#abc");
        assert!(parse_tx_input(&input).is_none());
    }

    #[test]
    fn test_parse_tx_input_wrong_hash_length() {
        let input = "abcd#0"; // 2 bytes, not 32
        assert!(parse_tx_input(input).is_none());
    }

    #[test]
    fn test_parse_hex_bytes_valid() {
        let bytes = parse_hex_bytes("deadbeef").unwrap();
        assert_eq!(bytes, vec![0xde, 0xad, 0xbe, 0xef]);
    }

    #[test]
    fn test_parse_hex_bytes_odd_length() {
        assert!(parse_hex_bytes("abc").is_none());
    }

    #[test]
    fn test_parse_hex_bytes_invalid_chars() {
        assert!(parse_hex_bytes("zzzz").is_none());
    }

    // ── Encoding tests ──

    #[test]
    fn test_encode_no_inputs() {
        let err = TxValidationError::NoInputs;
        let bytes = encode_apply_tx_err(&err, 6);
        let (era_id, n_failures) = decode_outer(&bytes);
        assert_eq!(era_id, 6);
        assert_eq!(n_failures, 1);

        // Navigate into the failure
        let mut dec = Decoder::new(&bytes);
        dec.array().unwrap(); // outer [[...]]
        dec.array().unwrap(); // [era_id, [...]]
        dec.u16().unwrap(); // era_id
        dec.array().unwrap(); // failures array

        let ledger_tag = decode_ledger_tag(&mut dec);
        assert_eq!(ledger_tag, 1, "ConwayUtxowFailure");

        let utxow_tag = decode_utxow_tag(&mut dec);
        assert_eq!(utxow_tag, 0, "UtxoFailure");

        let utxo_tag = decode_utxo_tag(&mut dec);
        assert_eq!(utxo_tag, 4, "InputSetEmptyUTxO");
    }

    #[test]
    fn test_encode_fee_too_small() {
        let err = TxValidationError::FeeTooSmall {
            minimum: 200_000,
            actual: 170_000,
        };
        let bytes = encode_apply_tx_err(&err, 6);
        let (era_id, n_failures) = decode_outer(&bytes);
        assert_eq!(era_id, 6);
        assert_eq!(n_failures, 1);

        let mut dec = Decoder::new(&bytes);
        dec.array().unwrap();
        dec.array().unwrap();
        dec.u16().unwrap();
        dec.array().unwrap();

        assert_eq!(decode_ledger_tag(&mut dec), 1);
        assert_eq!(decode_utxow_tag(&mut dec), 0);

        // ConwayUtxoPredFailure: array(3)[5, min_fee, actual_fee]
        let arr_len = dec.array().unwrap().unwrap();
        assert_eq!(arr_len, 3);
        let tag = dec.u8().unwrap();
        assert_eq!(tag, 5, "FeeTooSmallUTxO");
        let min_fee = dec.u64().unwrap();
        let actual_fee = dec.u64().unwrap();
        assert_eq!(min_fee, 200_000, "minimum fee first (swapped)");
        assert_eq!(actual_fee, 170_000, "actual fee second");
    }

    #[test]
    fn test_encode_value_not_conserved() {
        let err = TxValidationError::ValueNotConserved {
            inputs: 5_000_000,
            outputs: 4_500_000,
            fee: 200_000,
        };
        let bytes = encode_apply_tx_err(&err, 6);

        let mut dec = Decoder::new(&bytes);
        dec.array().unwrap();
        dec.array().unwrap();
        dec.u16().unwrap();
        dec.array().unwrap();

        assert_eq!(decode_ledger_tag(&mut dec), 1);
        assert_eq!(decode_utxow_tag(&mut dec), 0);

        let arr_len = dec.array().unwrap().unwrap();
        assert_eq!(arr_len, 3);
        let tag = dec.u8().unwrap();
        assert_eq!(tag, 6, "ValueNotConservedUTxO");
        let consumed = dec.u64().unwrap();
        let produced = dec.u64().unwrap();
        assert_eq!(consumed, 5_000_000, "consumed = inputs");
        assert_eq!(produced, 4_700_000, "produced = outputs + fee");
    }

    #[test]
    fn test_encode_tx_too_large() {
        let err = TxValidationError::TxTooLarge {
            maximum: 16_384,
            actual: 20_000,
        };
        let bytes = encode_apply_tx_err(&err, 6);

        let mut dec = Decoder::new(&bytes);
        dec.array().unwrap();
        dec.array().unwrap();
        dec.u16().unwrap();
        dec.array().unwrap();
        assert_eq!(decode_ledger_tag(&mut dec), 1);
        assert_eq!(decode_utxow_tag(&mut dec), 0);

        let arr_len = dec.array().unwrap().unwrap();
        assert_eq!(arr_len, 3);
        let tag = dec.u8().unwrap();
        assert_eq!(tag, 3, "MaxTxSizeUTxO");
        let supplied = dec.u64().unwrap();
        let expected = dec.u64().unwrap();
        assert_eq!(supplied, 20_000, "actual (supplied) first");
        assert_eq!(expected, 16_384, "maximum (expected) second");
    }

    #[test]
    fn test_encode_ttl_expired() {
        let err = TxValidationError::TtlExpired {
            current_slot: 1000,
            ttl: 500,
        };
        let bytes = encode_apply_tx_err(&err, 6);

        let mut dec = Decoder::new(&bytes);
        dec.array().unwrap();
        dec.array().unwrap();
        dec.u16().unwrap();
        dec.array().unwrap();
        assert_eq!(decode_ledger_tag(&mut dec), 1);
        assert_eq!(decode_utxow_tag(&mut dec), 0);

        let arr_len = dec.array().unwrap().unwrap();
        assert_eq!(arr_len, 3);
        let tag = dec.u8().unwrap();
        assert_eq!(tag, 2, "OutsideValidityIntervalUTxO");

        // ValidityInterval: array(2)[ SNothing, SJust(ttl) ]
        let vi_len = dec.array().unwrap().unwrap();
        assert_eq!(vi_len, 2);
        // SNothing (lower bound) = array(0)
        let lower_len = dec.array().unwrap().unwrap();
        assert_eq!(lower_len, 0, "no lower bound for TtlExpired");
        // SJust(ttl) (upper bound) = array(1)[ttl]
        let upper_len = dec.array().unwrap().unwrap();
        assert_eq!(upper_len, 1);
        let ttl = dec.u64().unwrap();
        assert_eq!(ttl, 500);

        // current_slot
        let current = dec.u64().unwrap();
        assert_eq!(current, 1000);
    }

    #[test]
    fn test_encode_not_yet_valid() {
        let err = TxValidationError::NotYetValid {
            current_slot: 100,
            valid_from: 500,
        };
        let bytes = encode_apply_tx_err(&err, 6);

        let mut dec = Decoder::new(&bytes);
        dec.array().unwrap();
        dec.array().unwrap();
        dec.u16().unwrap();
        dec.array().unwrap();
        assert_eq!(decode_ledger_tag(&mut dec), 1);
        assert_eq!(decode_utxow_tag(&mut dec), 0);

        let arr_len = dec.array().unwrap().unwrap();
        assert_eq!(arr_len, 3);
        let tag = dec.u8().unwrap();
        assert_eq!(tag, 2, "OutsideValidityIntervalUTxO");

        // ValidityInterval: array(2)[ SJust(valid_from), SNothing ]
        let vi_len = dec.array().unwrap().unwrap();
        assert_eq!(vi_len, 2);
        // SJust(valid_from) = array(1)[valid_from]
        let lower_len = dec.array().unwrap().unwrap();
        assert_eq!(lower_len, 1);
        let valid_from = dec.u64().unwrap();
        assert_eq!(valid_from, 500);
        // SNothing (upper) = array(0)
        let upper_len = dec.array().unwrap().unwrap();
        assert_eq!(upper_len, 0, "no upper bound for NotYetValid");

        let current = dec.u64().unwrap();
        assert_eq!(current, 100);
    }

    #[test]
    fn test_encode_bad_inputs() {
        let hash_hex = "ab".repeat(32); // 32 bytes
        let input = format!("{hash_hex}#7");
        let err = TxValidationError::InputNotFound { input };
        let bytes = encode_apply_tx_err(&err, 6);

        let mut dec = Decoder::new(&bytes);
        dec.array().unwrap();
        dec.array().unwrap();
        dec.u16().unwrap();
        dec.array().unwrap();
        assert_eq!(decode_ledger_tag(&mut dec), 1);
        assert_eq!(decode_utxow_tag(&mut dec), 0);

        let arr_len = dec.array().unwrap().unwrap();
        assert_eq!(arr_len, 2);
        let tag = dec.u8().unwrap();
        assert_eq!(tag, 1, "BadInputsUTxO");

        // tag(258) followed by array(1)[ [hash, idx] ]
        let cbor_tag = dec.tag().unwrap();
        assert_eq!(cbor_tag.as_u64(), 258);
        let set_len = dec.array().unwrap().unwrap();
        assert_eq!(set_len, 1);
        let txin_len = dec.array().unwrap().unwrap();
        assert_eq!(txin_len, 2);
        let tx_hash = dec.bytes().unwrap();
        assert_eq!(tx_hash, vec![0xab; 32]);
        let tx_ix = dec.u32().unwrap();
        assert_eq!(tx_ix, 7);
    }

    #[test]
    fn test_encode_missing_vkey_witness() {
        let credential_hex = "cd".repeat(28); // 28-byte keyhash
        let err = TxValidationError::MissingInputWitness {
            credential: credential_hex,
        };
        let bytes = encode_apply_tx_err(&err, 6);

        let mut dec = Decoder::new(&bytes);
        dec.array().unwrap();
        dec.array().unwrap();
        dec.u16().unwrap();
        dec.array().unwrap();

        // Ledger tag 1: ConwayUtxowFailure
        assert_eq!(decode_ledger_tag(&mut dec), 1);

        // Utxow tag 2: MissingVKeyWitnessesUTXOW (NOT Utxo tag 0)
        let arr_len = dec.array().unwrap().unwrap();
        assert_eq!(arr_len, 2);
        let utxow_tag = dec.u8().unwrap();
        assert_eq!(utxow_tag, 2, "MissingVKeyWitnessesUTXOW");

        // tag(258) set of keyhash bytes
        let cbor_tag = dec.tag().unwrap();
        assert_eq!(cbor_tag.as_u64(), 258);
        let set_len = dec.array().unwrap().unwrap();
        assert_eq!(set_len, 1);
        let keyhash = dec.bytes().unwrap();
        assert_eq!(keyhash, vec![0xcd; 28]);
    }

    #[test]
    fn test_encode_too_many_collateral_inputs() {
        let err = TxValidationError::TooManyCollateralInputs { max: 3, actual: 5 };
        let bytes = encode_apply_tx_err(&err, 6);

        let mut dec = Decoder::new(&bytes);
        dec.array().unwrap();
        dec.array().unwrap();
        dec.u16().unwrap();
        dec.array().unwrap();
        assert_eq!(decode_ledger_tag(&mut dec), 1);
        assert_eq!(decode_utxow_tag(&mut dec), 0);

        let arr_len = dec.array().unwrap().unwrap();
        assert_eq!(arr_len, 3);
        let tag = dec.u8().unwrap();
        assert_eq!(tag, 18, "TooManyCollateralInputs");
        let max_allowed = dec.u64().unwrap();
        let actual_count = dec.u64().unwrap();
        assert_eq!(max_allowed, 3, "max first (swapped)");
        assert_eq!(actual_count, 5, "actual second");
    }

    #[test]
    fn test_encode_multiple_errors() {
        let err = TxValidationError::Multiple(vec![
            TxValidationError::NoInputs,
            TxValidationError::FeeTooSmall {
                minimum: 200_000,
                actual: 100_000,
            },
        ]);
        let bytes = encode_apply_tx_err(&err, 6);
        let (era_id, n_failures) = decode_outer(&bytes);
        assert_eq!(era_id, 6);
        assert_eq!(n_failures, 2, "two flattened failures");
    }

    #[test]
    fn test_encode_fallback() {
        let err = TxValidationError::Other("something unexpected".to_string());
        let bytes = encode_apply_tx_err(&err, 6);

        let mut dec = Decoder::new(&bytes);
        dec.array().unwrap();
        dec.array().unwrap();
        dec.u16().unwrap();
        dec.array().unwrap();

        // Ledger tag 7: ConwayMempoolFailure
        let arr_len = dec.array().unwrap().unwrap();
        assert_eq!(arr_len, 2);
        let tag = dec.u8().unwrap();
        assert_eq!(tag, 7, "ConwayMempoolFailure");
        let text = dec.str().unwrap();
        assert!(text.contains("something unexpected"));
    }

    #[test]
    fn test_encode_script_data_hash_mismatch() {
        let expected_hex = "aa".repeat(32);
        let actual_hex = "bb".repeat(32);
        let err = TxValidationError::ScriptDataHashMismatch {
            expected: expected_hex,
            actual: actual_hex,
        };
        let bytes = encode_apply_tx_err(&err, 6);

        let mut dec = Decoder::new(&bytes);
        dec.array().unwrap();
        dec.array().unwrap();
        dec.u16().unwrap();
        dec.array().unwrap();
        assert_eq!(decode_ledger_tag(&mut dec), 1);

        // Utxow tag 13: PPViewHashesDontMatch
        let arr_len = dec.array().unwrap().unwrap();
        assert_eq!(arr_len, 3);
        let utxow_tag = dec.u8().unwrap();
        assert_eq!(utxow_tag, 13, "PPViewHashesDontMatch");

        // supplied (actual): SJust(hash)
        let s_len = dec.array().unwrap().unwrap();
        assert_eq!(s_len, 1);
        let actual_bytes = dec.bytes().unwrap();
        assert_eq!(actual_bytes, vec![0xbb; 32]);

        // expected: SJust(hash)
        let e_len = dec.array().unwrap().unwrap();
        assert_eq!(e_len, 1);
        let expected_bytes = dec.bytes().unwrap();
        assert_eq!(expected_bytes, vec![0xaa; 32]);
    }

    #[test]
    fn test_encode_collateral_mismatch() {
        let err = TxValidationError::CollateralMismatch {
            declared: 5_000_000,
            computed: 4_800_000,
        };
        let bytes = encode_apply_tx_err(&err, 6);

        let mut dec = Decoder::new(&bytes);
        dec.array().unwrap();
        dec.array().unwrap();
        dec.u16().unwrap();
        dec.array().unwrap();
        assert_eq!(decode_ledger_tag(&mut dec), 1);
        assert_eq!(decode_utxow_tag(&mut dec), 0);

        let arr_len = dec.array().unwrap().unwrap();
        assert_eq!(arr_len, 3);
        let tag = dec.u8().unwrap();
        assert_eq!(tag, 20, "IncorrectTotalCollateralField");

        // delta = computed - declared = 4_800_000 - 5_000_000 = -200_000
        let delta = dec.i64().unwrap();
        assert_eq!(delta, -200_000);
        let declared = dec.u64().unwrap();
        assert_eq!(declared, 5_000_000);
    }

    #[test]
    fn test_encode_reference_input_overlaps() {
        let hash_hex = "ff".repeat(32);
        let input = format!("{hash_hex}#0");
        let err = TxValidationError::ReferenceInputOverlapsInput { input };
        let bytes = encode_apply_tx_err(&err, 6);

        let mut dec = Decoder::new(&bytes);
        dec.array().unwrap();
        dec.array().unwrap();
        dec.u16().unwrap();
        dec.array().unwrap();
        assert_eq!(decode_ledger_tag(&mut dec), 1);
        assert_eq!(decode_utxow_tag(&mut dec), 0);

        let arr_len = dec.array().unwrap().unwrap();
        assert_eq!(arr_len, 2);
        let tag = dec.u8().unwrap();
        assert_eq!(tag, 22, "BabbageNonDisjointRefInputs");
    }

    /// Test that the encoder output can be decoded by the existing client decoder.
    /// This validates encoder↔decoder compatibility.
    #[test]
    fn test_roundtrip_through_decoder() {
        let err = TxValidationError::FeeTooSmall {
            minimum: 200_000,
            actual: 170_000,
        };
        let apply_tx_err = encode_apply_tx_err(&err, 6);

        // Wrap in MsgRejectTx-like structure that decode_reject_reason expects.
        // The decoder expects to start AFTER the [2, ...] tag, at the ApplyTxErr payload.
        // Looking at n2c_client.rs:1155, it calls array() twice then reads era_idx.
        // The first array() enters the outer [[...]], the second enters [era_id, [...]].
        let mut dec = Decoder::new(&apply_tx_err);

        // Replicate decode_reject_reason logic
        let _ = dec.array().unwrap(); // outer array(1)
        let _ = dec.array().unwrap(); // [era_id, failures]
        let era_idx = dec.u8().unwrap();
        assert_eq!(era_idx, 6);

        let n_errors = dec.array().unwrap().unwrap();
        assert_eq!(n_errors, 1);

        // Decode the ConwayLedgerPredFailure
        let _ = dec.array().unwrap(); // failure array
        let ledger_tag = dec.u8().unwrap();
        assert_eq!(ledger_tag, 1); // ConwayUtxowFailure

        let _ = dec.array().unwrap(); // Utxow array
        let utxow_tag = dec.u8().unwrap();
        assert_eq!(utxow_tag, 0); // UtxoFailure

        let _ = dec.array().unwrap(); // Utxo array
        let utxo_tag = dec.u8().unwrap();
        assert_eq!(utxo_tag, 5); // FeeTooSmallUTxO

        let min_fee = dec.u64().unwrap();
        let actual_fee = dec.u64().unwrap();
        assert_eq!(min_fee, 200_000);
        assert_eq!(actual_fee, 170_000);
    }
}
