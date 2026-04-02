//! TxSubmission2 client — announces and serves transactions to remote peers.
//!
//! In TxSubmission2, the "client" is the side that has transactions to offer.
//! The server drives the protocol by requesting tx IDs and tx bodies.
//!
//! The client:
//! 1. Sends `MsgInit` to initialize
//! 2. Waits for `MsgRequestTxIds` from the server
//! 3. Replies with tx IDs from the mempool
//! 4. Waits for `MsgRequestTxs` to send full tx bodies
//! 5. Sends `MsgDone` when in blocking state with no transactions

use std::sync::Arc;

use crate::error::ProtocolError;
use crate::mux::channel::MuxChannel;

use super::{decode_message, encode_message, TxIdAndSize, TxSubmissionMessage};

/// Trait for providing transactions to the TxSubmission2 client.
///
/// Implemented by the mempool layer.
pub trait TxSource: Send + Sync {
    /// Get pending transaction IDs with their sizes.
    /// Returns up to `max_count` tx IDs, acknowledging `ack_count` previously returned.
    fn get_tx_ids(&self, ack_count: u16, max_count: u16) -> Vec<TxIdAndSize>;

    /// Get full transaction CBOR by their IDs.
    ///
    /// Each element in `tx_ids` is `(era_id, tx_hash)` matching the HFC GenTxId
    /// envelope from `MsgRequestTxs`.  Returns `(era_id, tx_cbor)` pairs for
    /// `MsgReplyTxs`, preserving the era for the HFC GenTx envelope.
    fn get_txs(&self, tx_ids: &[(u8, [u8; 32])]) -> Vec<(u8, Vec<u8>)>;

    /// Check if there are any pending transactions.
    fn has_pending(&self) -> bool;

    /// Optional notification handle for event-driven wakeup.
    ///
    /// When `Some`, the client awaits this instead of polling every 500ms.
    /// The mempool fires `notify_waiters()` on each successful tx admission,
    /// providing zero-CPU-waste blocking behavior matching Haskell's STM retry.
    fn tx_notify(&self) -> Option<Arc<tokio::sync::Notify>> {
        None
    }
}

/// TxSubmission2 client that announces transactions to a remote peer.
pub struct TxSubmissionClient;

impl TxSubmissionClient {
    /// Run the TxSubmission2 client protocol.
    ///
    /// Sends `MsgInit`, then responds to server requests until `MsgDone`.
    pub async fn run<S: TxSource>(
        channel: &mut MuxChannel,
        source: &S,
    ) -> Result<(), ProtocolError> {
        // Send MsgInit
        let init = encode_message(&TxSubmissionMessage::MsgInit);
        channel.send(init).await.map_err(ProtocolError::from)?;
        tracing::debug!("txsubmission2 client: MsgInit sent, awaiting server requests");

        loop {
            // Wait for server request
            let msg_bytes = channel.recv().await.map_err(ProtocolError::from)?;
            let msg = decode_message(&msg_bytes).map_err(|e| ProtocolError::CborDecode {
                protocol: "TxSubmission2",
                reason: e,
            })?;

            match msg {
                TxSubmissionMessage::MsgRequestTxIds {
                    blocking,
                    ack_count,
                    req_count,
                } => {
                    let mut tx_ids = source.get_tx_ids(ack_count, req_count);
                    tracing::debug!(
                        blocking,
                        ack_count,
                        req_count,
                        yielded = tx_ids.len(),
                        "txsubmission2 client: MsgRequestTxIds received"
                    );

                    if tx_ids.is_empty() && blocking {
                        // Blocking mode with empty mempool: wait for txs to appear.
                        //
                        // The initial get_tx_ids(ack_count, req_count) already
                        // acknowledged previously-outstanding tx IDs.  Subsequent
                        // polls must NOT re-acknowledge (ack_count=0) but the
                        // outstanding set was already drained by the first call.
                        tracing::debug!("txsubmission2 client: blocking — waiting for mempool txs");
                        loop {
                            // Event-driven wakeup when the mempool provides a Notify
                            // handle; falls back to 500ms polling for TxSource impls
                            // that don't support notification (e.g. test mocks).
                            if let Some(notify) = source.tx_notify() {
                                notify.notified().await;
                            } else {
                                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                            }
                            // ack_count=0: the first call already acknowledged.
                            // req_count stays the same — peer wants up to this many.
                            tx_ids = source.get_tx_ids(0, req_count);
                            if !tx_ids.is_empty() {
                                tracing::info!(
                                    count = tx_ids.len(),
                                    "txsubmission2 client: mempool txs available, resuming"
                                );
                                break;
                            }
                        }
                    }

                    let reply = encode_message(&TxSubmissionMessage::MsgReplyTxIds(tx_ids));
                    channel.send(reply).await.map_err(ProtocolError::from)?;
                }
                TxSubmissionMessage::MsgRequestTxs(tx_ids) => {
                    let txs = source.get_txs(&tx_ids);
                    tracing::debug!(
                        requested = tx_ids.len(),
                        returned = txs.len(),
                        "txsubmission2 client: MsgRequestTxs received"
                    );
                    let reply = encode_message(&TxSubmissionMessage::MsgReplyTxs(txs));
                    channel.send(reply).await.map_err(ProtocolError::from)?;
                }
                other => {
                    return Err(ProtocolError::StateViolation {
                        protocol: "TxSubmission2",
                        expected: "MsgRequestTxIds or MsgRequestTxs".to_string(),
                        actual: format!("{other:?}"),
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use tokio::sync::mpsc;

    struct MockTxSource {
        tx_ids: Vec<TxIdAndSize>,
        txs: Vec<([u8; 32], Vec<u8>)>,
    }

    impl TxSource for MockTxSource {
        fn get_tx_ids(&self, _ack_count: u16, max_count: u16) -> Vec<TxIdAndSize> {
            self.tx_ids
                .iter()
                .take(max_count as usize)
                .cloned()
                .collect()
        }

        fn get_txs(&self, tx_ids: &[(u8, [u8; 32])]) -> Vec<(u8, Vec<u8>)> {
            tx_ids
                .iter()
                .filter_map(|(era_id, id)| {
                    self.txs
                        .iter()
                        .find(|(tid, _)| tid == id)
                        .map(|(_, data)| (*era_id, data.clone()))
                })
                .collect()
        }

        fn has_pending(&self) -> bool {
            !self.tx_ids.is_empty()
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
            4,
            crate::mux::Direction::InitiatorDir,
            egress_tx,
            ingress_rx,
            1_000_000,
            std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        );
        (channel, egress_rx, ingress_tx)
    }

    #[tokio::test]
    async fn client_sends_init_and_replies() {
        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();
        let source = MockTxSource {
            tx_ids: vec![TxIdAndSize {
                era_id: 6,
                tx_id: [0xAA; 32],
                size_in_bytes: 200,
            }],
            txs: vec![([0xAA; 32], vec![0x01, 0x02])],
        };

        let handle =
            tokio::spawn(async move { TxSubmissionClient::run(&mut channel, &source).await });

        // Read MsgInit
        let (_, _, init) = egress_rx.recv().await.unwrap();
        assert!(matches!(
            decode_message(&init).unwrap(),
            TxSubmissionMessage::MsgInit
        ));

        // Send MsgRequestTxIds (non-blocking, first request)
        let req = encode_message(&TxSubmissionMessage::MsgRequestTxIds {
            blocking: false,
            ack_count: 0,
            req_count: 10,
        });
        ingress_tx.send(Bytes::from(req)).await.unwrap();

        // Read MsgReplyTxIds
        let (_, _, reply) = egress_rx.recv().await.unwrap();
        if let TxSubmissionMessage::MsgReplyTxIds(ids) = decode_message(&reply).unwrap() {
            assert_eq!(ids.len(), 1);
            assert_eq!(ids[0].tx_id, [0xAA; 32]);
        } else {
            panic!("expected MsgReplyTxIds");
        }

        // Send MsgRequestTxs — each element is (era_id, tx_hash)
        let req_txs = encode_message(&TxSubmissionMessage::MsgRequestTxs(vec![(6u8, [0xAA; 32])]));
        ingress_tx.send(Bytes::from(req_txs)).await.unwrap();

        // Read MsgReplyTxs — each element is (era_id, tx_cbor)
        let (_, _, reply_txs) = egress_rx.recv().await.unwrap();
        if let TxSubmissionMessage::MsgReplyTxs(txs) = decode_message(&reply_txs).unwrap() {
            assert_eq!(txs, vec![(6u8, vec![0x01, 0x02])]);
        } else {
            panic!("expected MsgReplyTxs");
        }

        // We can't change the source mid-test, so just drop the channel to end
        drop(ingress_tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn client_blocks_when_blocking_with_no_txs() {
        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();
        let source = MockTxSource {
            tx_ids: vec![],
            txs: vec![],
        };

        let handle =
            tokio::spawn(async move { TxSubmissionClient::run(&mut channel, &source).await });

        // Read MsgInit
        let _ = egress_rx.recv().await.unwrap();

        // Send blocking MsgRequestTxIds
        let req = encode_message(&TxSubmissionMessage::MsgRequestTxIds {
            blocking: true,
            ack_count: 0,
            req_count: 10,
        });
        ingress_tx.send(Bytes::from(req)).await.unwrap();

        // Client should block (polling mempool) rather than sending MsgDone.
        // Verify no message arrives within 200ms.
        let result =
            tokio::time::timeout(std::time::Duration::from_millis(200), egress_rx.recv()).await;
        assert!(result.is_err(), "client should block, not send MsgDone");

        // Abort the client task (it's polling forever with empty mempool).
        handle.abort();
        let _ = handle.await;
    }

    /// Mock TxSource that supports Notify-based wakeup with shared tx_ids
    /// that can be populated externally after construction.
    struct NotifyMockTxSource {
        notify: Arc<tokio::sync::Notify>,
        /// Shared so the test can inject tx IDs while the source is in use.
        tx_ids: std::sync::Arc<std::sync::Mutex<Vec<TxIdAndSize>>>,
        txs: Vec<([u8; 32], Vec<u8>)>,
    }

    impl TxSource for NotifyMockTxSource {
        fn get_tx_ids(&self, _ack_count: u16, max_count: u16) -> Vec<TxIdAndSize> {
            self.tx_ids
                .lock()
                .unwrap()
                .iter()
                .take(max_count as usize)
                .cloned()
                .collect()
        }

        fn get_txs(&self, tx_ids: &[(u8, [u8; 32])]) -> Vec<(u8, Vec<u8>)> {
            tx_ids
                .iter()
                .filter_map(|(era_id, id)| {
                    self.txs
                        .iter()
                        .find(|(tid, _)| tid == id)
                        .map(|(_, data)| (*era_id, data.clone()))
                })
                .collect()
        }

        fn has_pending(&self) -> bool {
            !self.tx_ids.lock().unwrap().is_empty()
        }

        fn tx_notify(&self) -> Option<Arc<tokio::sync::Notify>> {
            Some(self.notify.clone())
        }
    }

    #[tokio::test]
    async fn client_wakes_on_notify_instead_of_polling() {
        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();
        let notify = Arc::new(tokio::sync::Notify::new());
        let shared_tx_ids: std::sync::Arc<std::sync::Mutex<Vec<TxIdAndSize>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let tx_ids_handle = shared_tx_ids.clone();

        let source = NotifyMockTxSource {
            notify: notify.clone(),
            tx_ids: shared_tx_ids,
            txs: vec![([0xDD; 32], vec![0x99])],
        };

        let handle =
            tokio::spawn(async move { TxSubmissionClient::run(&mut channel, &source).await });

        // Read MsgInit
        let _ = egress_rx.recv().await.unwrap();

        // Send blocking MsgRequestTxIds
        let req = encode_message(&TxSubmissionMessage::MsgRequestTxIds {
            blocking: true,
            ack_count: 0,
            req_count: 10,
        });
        ingress_tx.send(Bytes::from(req)).await.unwrap();

        // Client should be waiting on the notify (not polling).
        // Verify no message within 100ms.
        let timeout_result =
            tokio::time::timeout(std::time::Duration::from_millis(100), egress_rx.recv()).await;
        assert!(
            timeout_result.is_err(),
            "client should be waiting on notify"
        );

        // "Add a tx to the mempool" via the shared handle, then fire notify.
        tx_ids_handle.lock().unwrap().push(TxIdAndSize {
            era_id: 6,
            tx_id: [0xDD; 32],
            size_in_bytes: 200,
        });
        notify.notify_waiters();

        // The client should wake promptly and send MsgReplyTxIds within 100ms
        // (proving it used Notify, not 500ms polling).
        let reply_result =
            tokio::time::timeout(std::time::Duration::from_millis(100), egress_rx.recv()).await;
        assert!(
            reply_result.is_ok(),
            "client should wake from notify and reply promptly"
        );
        let (_, _, reply_bytes) = reply_result.unwrap().unwrap();
        if let TxSubmissionMessage::MsgReplyTxIds(ids) = decode_message(&reply_bytes).unwrap() {
            assert_eq!(ids.len(), 1);
            assert_eq!(ids[0].tx_id, [0xDD; 32]);
        } else {
            panic!("expected MsgReplyTxIds");
        }

        // Clean up
        handle.abort();
        let _ = handle.await;
    }
}
