//! LocalTxSubmission server — validates and accepts transactions from N2C clients.
//!
//! Receives `MsgSubmitTx` with era ID and raw tx CBOR, validates via `TxValidator`,
//! and responds with `MsgAcceptTx` or `MsgRejectTx`.

use minicbor::{Decoder, Encoder};

use crate::error::ProtocolError;
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
            let msg_bytes = channel.recv().await.map_err(ProtocolError::from)?;
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
                            let reason = format!("{e:?}");
                            tracing::debug!(era_id, reason = %reason, "local tx rejected");

                            // Send MsgRejectTx = [2, [era_id, [reason]]]
                            let mut buf = Vec::new();
                            let mut enc = Encoder::new(&mut buf);
                            enc.array(2).expect("infallible");
                            enc.u64(TAG_REJECT_TX).expect("infallible");
                            enc.array(2).expect("infallible");
                            enc.u16(era_id).expect("infallible");
                            enc.array(1).expect("infallible");
                            enc.str(&reason).expect("infallible");
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

        // Should get MsgRejectTx
        let (_, _, resp) = egress_rx.recv().await.unwrap();
        let mut dec = Decoder::new(&resp);
        dec.array().unwrap();
        assert_eq!(dec.u64().unwrap(), TAG_REJECT_TX);

        // Send MsgDone
        ingress_tx.send(Bytes::from(encode_done())).await.unwrap();

        let stats = handle.await.unwrap().unwrap();
        assert_eq!(stats.submitted, 1);
        assert_eq!(stats.accepted, 0);
        assert_eq!(stats.rejected, 1);
    }
}
