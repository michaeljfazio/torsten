//! LocalTxSubmission server — validates and accepts transactions from N2C clients.
//!
//! Receives `MsgSubmitTx` with era ID and raw tx CBOR, validates via `TxValidator`,
//! and responds with `MsgAcceptTx` or `MsgRejectTx`.

use minicbor::{Decoder, Encoder};

use crate::error::{MuxError, ProtocolError};
use crate::mux::channel::MuxChannel;
use crate::TxValidator;

// CBOR message tags for LocalTxSubmission
const TAG_SUBMIT_TX: u64 = 0;
const TAG_ACCEPT_TX: u64 = 1;
const TAG_REJECT_TX: u64 = 2;
const TAG_DONE: u64 = 3;

/// LocalTxSubmission server statistics.
#[derive(Debug, Clone, Default)]
pub struct LocalTxSubmissionStats {
    /// Number of transactions submitted.
    pub submitted: u64,
    /// Number of transactions accepted.
    pub accepted: u64,
    /// Number of transactions rejected.
    pub rejected: u64,
}

/// LocalTxSubmission server that validates transactions from N2C clients.
pub struct LocalTxSubmissionServer;

impl LocalTxSubmissionServer {
    /// Run the LocalTxSubmission server loop.
    ///
    /// `on_accepted` is called with `(era_id, tx_bytes)` for each accepted transaction.
    /// The caller is responsible for adding the tx to the mempool.
    pub async fn run<V, F>(
        channel: &mut MuxChannel,
        validator: &V,
        mut on_accepted: F,
    ) -> Result<LocalTxSubmissionStats, ProtocolError>
    where
        V: TxValidator,
        F: FnMut(u16, Vec<u8>) + Send,
    {
        let mut stats = LocalTxSubmissionStats::default();

        loop {
            let msg_bytes = match channel.recv().await {
                Ok(b) => b,
                // Client closed the connection without sending MsgDone — treat as
                // a graceful disconnect and return the accumulated stats so that
                // the node can update its metrics correctly.
                Err(e) if matches!(e, MuxError::BearerClosed | MuxError::ChannelClosed) => {
                    tracing::debug!(?stats, "local tx submission: client disconnected");
                    return Ok(stats);
                }
                Err(e) => return Err(ProtocolError::from(e)),
            };
            let mut dec = Decoder::new(&msg_bytes);

            let _arr_len = dec.array().map_err(|e| ProtocolError::CborDecode {
                protocol: "LocalTxSubmission",
                reason: e.to_string(),
            })?;
            let tag = dec.u64().map_err(|e| ProtocolError::CborDecode {
                protocol: "LocalTxSubmission",
                reason: e.to_string(),
            })?;

            match tag {
                TAG_SUBMIT_TX => {
                    stats.submitted += 1;

                    // Decode [era_id, tx_bytes]
                    let _inner_arr = dec.array().map_err(|e| ProtocolError::CborDecode {
                        protocol: "LocalTxSubmission",
                        reason: e.to_string(),
                    })?;
                    let era_id = dec.u16().map_err(|e| ProtocolError::CborDecode {
                        protocol: "LocalTxSubmission",
                        reason: e.to_string(),
                    })?;
                    // The tx may be wrapped in CBOR tag 24 (wrapCBORinCBOR).
                    // Consume the tag if present, then read the raw bytes.
                    let pos = dec.position();
                    if let Ok(tag) = dec.tag() {
                        if tag.as_u64() != 24 {
                            dec.set_position(pos); // not tag 24, rewind
                        }
                        // tag 24 consumed, bytes follow
                    } else {
                        dec.set_position(pos); // no tag, rewind
                    }
                    let tx_bytes = dec
                        .bytes()
                        .map_err(|e| ProtocolError::CborDecode {
                            protocol: "LocalTxSubmission",
                            reason: e.to_string(),
                        })?
                        .to_vec();

                    // Validate via TxValidator
                    match validator.validate_tx(era_id, &tx_bytes) {
                        Ok(()) => {
                            stats.accepted += 1;
                            on_accepted(era_id, tx_bytes);

                            // Send MsgAcceptTx
                            let mut buf = Vec::new();
                            let mut enc = Encoder::new(&mut buf);
                            enc.array(1).expect("infallible");
                            enc.u64(TAG_ACCEPT_TX).expect("infallible");
                            channel.send(buf).await.map_err(ProtocolError::from)?;
                        }
                        Err(e) => {
                            stats.rejected += 1;
                            tracing::debug!(era_id, reason = %format!("{e:?}"), "local tx rejected");

                            // Send MsgRejectTx = [2, ApplyTxErr]
                            // where ApplyTxErr = [[era_id, [failure_0, ...]]]
                            // encoded as structured CBOR matching Haskell cardano-node.
                            let apply_tx_err = super::encode::encode_apply_tx_err(&e, era_id);
                            let mut buf = Vec::new();
                            let mut enc = Encoder::new(&mut buf);
                            enc.array(2).expect("infallible");
                            enc.u64(TAG_REJECT_TX).expect("infallible");
                            let writer = enc.writer_mut();
                            writer.extend_from_slice(&apply_tx_err);
                            channel.send(buf).await.map_err(ProtocolError::from)?;
                        }
                    }
                }
                TAG_DONE => {
                    tracing::debug!(?stats, "local tx submission: client done");
                    return Ok(stats);
                }
                _ => {
                    return Err(ProtocolError::InvalidMessage {
                        protocol: "LocalTxSubmission",
                        tag: tag as u8,
                        reason: format!("unexpected message tag: {tag}"),
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TxValidationError;
    use bytes::Bytes;
    use tokio::sync::mpsc;

    struct AcceptAllValidator;
    impl TxValidator for AcceptAllValidator {
        fn validate_tx(&self, _era_id: u16, _tx_bytes: &[u8]) -> Result<(), TxValidationError> {
            Ok(())
        }
    }

    struct RejectAllValidator;
    impl TxValidator for RejectAllValidator {
        fn validate_tx(&self, _era_id: u16, _tx_bytes: &[u8]) -> Result<(), TxValidationError> {
            Err(TxValidationError::Other("test rejection".to_string()))
        }
    }

    fn make_test_channel() -> (
        MuxChannel,
        mpsc::Receiver<(u16, crate::mux::Direction, Bytes)>,
        mpsc::Sender<Bytes>,
    ) {
        let (egress_tx, egress_rx) = mpsc::channel(64);
        let (ingress_tx, ingress_rx) = mpsc::channel(64);
        let channel = MuxChannel::new(
            6,
            crate::mux::Direction::ResponderDir,
            egress_tx,
            ingress_rx,
            1_000_000,
            std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        );
        (channel, egress_rx, ingress_tx)
    }

    /// Encode a MsgSubmitTx for testing.
    fn encode_submit_tx(era_id: u16, tx_bytes: &[u8]) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut enc = Encoder::new(&mut buf);
        enc.array(2).expect("infallible");
        enc.u64(TAG_SUBMIT_TX).expect("infallible");
        enc.array(2).expect("infallible");
        enc.u16(era_id).expect("infallible");
        enc.bytes(tx_bytes).expect("infallible");
        buf
    }

    /// Encode a MsgDone for testing.
    fn encode_done() -> Vec<u8> {
        let mut buf = Vec::new();
        let mut enc = Encoder::new(&mut buf);
        enc.array(1).expect("infallible");
        enc.u64(TAG_DONE).expect("infallible");
        buf
    }

    #[tokio::test]
    async fn accepts_valid_tx() {
        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();
        let validator = AcceptAllValidator;
        let accepted = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let accepted_clone = accepted.clone();

        let handle = tokio::spawn(async move {
            LocalTxSubmissionServer::run(&mut channel, &validator, move |era, tx| {
                accepted_clone.lock().unwrap().push((era, tx));
            })
            .await
        });

        // Submit a tx
        let submit = encode_submit_tx(6, &[0xDE, 0xAD]);
        ingress_tx.send(Bytes::from(submit)).await.unwrap();

        // Should get MsgAcceptTx
        let (_, _, resp) = egress_rx.recv().await.unwrap();
        let mut dec = Decoder::new(&resp);
        dec.array().unwrap();
        assert_eq!(dec.u64().unwrap(), TAG_ACCEPT_TX);

        // Send MsgDone
        ingress_tx.send(Bytes::from(encode_done())).await.unwrap();

        let stats = handle.await.unwrap().unwrap();
        assert_eq!(stats.submitted, 1);
        assert_eq!(stats.accepted, 1);
        assert_eq!(stats.rejected, 0);

        let accepted = accepted.lock().unwrap();
        assert_eq!(accepted.len(), 1);
        assert_eq!(accepted[0].0, 6); // era
        assert_eq!(accepted[0].1, vec![0xDE, 0xAD]); // tx bytes
    }

    #[tokio::test]
    async fn rejects_invalid_tx() {
        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();
        let validator = RejectAllValidator;

        let handle = tokio::spawn(async move {
            LocalTxSubmissionServer::run(&mut channel, &validator, |_, _| {}).await
        });

        let submit = encode_submit_tx(6, &[0xBA, 0xAD]);
        ingress_tx.send(Bytes::from(submit)).await.unwrap();

        // Should get MsgRejectTx with structured CBOR
        let (_, _, resp) = egress_rx.recv().await.unwrap();
        let mut dec = Decoder::new(&resp);
        dec.array().unwrap();
        assert_eq!(dec.u64().unwrap(), TAG_REJECT_TX);

        // Verify ApplyTxErr structure: [[era_id, [failure_0, ...]]]
        let outer_len = dec.array().unwrap().unwrap();
        assert_eq!(outer_len, 1, "outer HFC wrapper must be array(1)");
        let inner_len = dec.array().unwrap().unwrap();
        assert_eq!(inner_len, 2, "inner must be [era_id, failures]");
        let era_id = dec.u16().unwrap();
        assert_eq!(era_id, 6);
        let n_failures = dec.array().unwrap().unwrap();
        assert_eq!(n_failures, 1, "one rejection failure");

        // The failure should be ConwayMempoolFailure (tag 7) since RejectAllValidator
        // returns Other("test rejection") which falls through to mempool fallback.
        let failure_len = dec.array().unwrap().unwrap();
        assert_eq!(failure_len, 2);
        let ledger_tag = dec.u8().unwrap();
        assert_eq!(ledger_tag, 7, "ConwayMempoolFailure");
        let text = dec.str().unwrap();
        assert!(text.contains("test rejection"));

        // Send MsgDone
        ingress_tx.send(Bytes::from(encode_done())).await.unwrap();

        let stats = handle.await.unwrap().unwrap();
        assert_eq!(stats.submitted, 1);
        assert_eq!(stats.accepted, 0);
        assert_eq!(stats.rejected, 1);
    }

    #[tokio::test]
    async fn rejects_with_structured_fee_error() {
        // Test that a specific validation error produces correct structured CBOR
        struct FeeTooSmallValidator;
        impl TxValidator for FeeTooSmallValidator {
            fn validate_tx(&self, _era_id: u16, _tx_bytes: &[u8]) -> Result<(), TxValidationError> {
                Err(TxValidationError::FeeTooSmall {
                    minimum: 200_000,
                    actual: 170_000,
                })
            }
        }

        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();
        let validator = FeeTooSmallValidator;

        let handle = tokio::spawn(async move {
            LocalTxSubmissionServer::run(&mut channel, &validator, |_, _| {}).await
        });

        let submit = encode_submit_tx(6, &[0xDE, 0xAD]);
        ingress_tx.send(Bytes::from(submit)).await.unwrap();

        let (_, _, resp) = egress_rx.recv().await.unwrap();
        let mut dec = Decoder::new(&resp);
        dec.array().unwrap(); // MsgRejectTx
        assert_eq!(dec.u64().unwrap(), TAG_REJECT_TX);

        // ApplyTxErr outer wrapper
        dec.array().unwrap(); // [[...]]
        dec.array().unwrap(); // [era_id, [...]]
        assert_eq!(dec.u16().unwrap(), 6);
        dec.array().unwrap(); // failures

        // ConwayLedgerPredFailure(1) → ConwayUtxowPredFailure(0) → ConwayUtxoPredFailure(5)
        dec.array().unwrap();
        assert_eq!(dec.u8().unwrap(), 1, "ConwayUtxowFailure");
        dec.array().unwrap();
        assert_eq!(dec.u8().unwrap(), 0, "UtxoFailure");
        let arr_len = dec.array().unwrap().unwrap();
        assert_eq!(arr_len, 3);
        assert_eq!(dec.u8().unwrap(), 5, "FeeTooSmallUTxO");
        assert_eq!(dec.u64().unwrap(), 200_000, "min fee first");
        assert_eq!(dec.u64().unwrap(), 170_000, "actual fee second");

        ingress_tx.send(Bytes::from(encode_done())).await.unwrap();
        let stats = handle.await.unwrap().unwrap();
        assert_eq!(stats.rejected, 1);
    }
}
