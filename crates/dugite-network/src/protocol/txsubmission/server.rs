//! TxSubmission2 server — requests and validates transactions from remote peers.
//!
//! In TxSubmission2, the "server" drives the protocol by requesting tx IDs and
//! tx bodies from the remote peer (client). The server:
//!
//! 1. Receives `MsgInit` from the client
//! 2. Sends `MsgRequestTxIds(blocking=false, ack=0, req=N)` as the **first request**
//!    (CRITICAL: first request must be non-blocking with ack_count=0)
//! 3. Receives tx IDs, tracks them in a FIFO queue
//! 4. Requests full tx bodies for IDs not already known
//! 5. Passes received txs to a callback for validation/mempool admission
//! 6. Uses `blocking=true` only when all unacknowledged IDs have been processed

use std::collections::{HashSet, VecDeque};

use crate::error::ProtocolError;
use crate::mux::channel::MuxChannel;

use super::{decode_message, encode_message, TxIdAndSize, TxSubmissionMessage};

/// Maximum number of tx IDs to request in a single MsgRequestTxIds.
const MAX_TX_IDS_PER_REQUEST: u16 = 10;

/// Maximum unacknowledged tx IDs before the server stops requesting more.
/// Matches Haskell `maxUnacked` in `Ouroboros.Network.TxSubmission.Inbound`.
const MAX_UNACKED: usize = 100;

/// Maximum in-flight tx IDs (requested but not yet received).
/// Peers exceeding this limit are disconnected for protocol violation.
const MAX_INFLIGHT_TX_IDS: usize = 1000;

/// TxSubmission2 server that requests transactions from remote peers.
pub struct TxSubmissionServer;

impl TxSubmissionServer {
    /// Run the TxSubmission2 server protocol.
    ///
    /// Drives the protocol by requesting tx IDs and bodies from the remote peer.
    /// Each received transaction is passed to `on_tx` for validation and mempool
    /// admission. The callback returns `true` if the tx was accepted, `false` if rejected.
    pub async fn run<F>(
        channel: &mut MuxChannel,
        mut on_tx: F,
    ) -> Result<TxSubmissionStats, ProtocolError>
    where
        F: FnMut([u8; 32], Vec<u8>) -> bool + Send,
    {
        let mut stats = TxSubmissionStats::default();

        // Wait for MsgInit
        let init_bytes = channel.recv().await.map_err(ProtocolError::from)?;
        let init_msg = decode_message(&init_bytes).map_err(|e| ProtocolError::CborDecode {
            protocol: "TxSubmission2",
            reason: e,
        })?;
        if !matches!(init_msg, TxSubmissionMessage::MsgInit) {
            return Err(ProtocolError::StateViolation {
                protocol: "TxSubmission2",
                expected: "MsgInit".to_string(),
                actual: format!("{init_msg:?}"),
            });
        }

        // FIFO queue of unacknowledged tx IDs
        let mut unacked: VecDeque<TxIdAndSize> = VecDeque::new();
        // Set of tx IDs currently in-flight (requested but not yet received)
        let mut inflight: HashSet<[u8; 32]> = HashSet::new();
        loop {
            // Acknowledge all tx IDs that have been fully processed.
            // On the first iteration unacked is empty, so ack_count is 0.
            let ack_count = unacked.len().min(u16::MAX as usize) as u16;
            for _ in 0..ack_count {
                unacked.pop_front();
            }

            // Blocking mode rules (enforced by Haskell txSubmissionOutbound):
            //
            // - blocking=true when unacked is empty (including the FIRST request).
            //   The client uses STM retry to wait for mempool txs.
            // - blocking=false when unacked has items (pipelining more requests
            //   while waiting for tx bodies of previously announced IDs).
            //
            // Violating these invariants causes the Haskell peer to throw:
            // - ProtocolErrorRequestNonBlocking: non-blocking when unacked is empty
            // - ProtocolErrorRequestBlocking: blocking when unacked is non-empty
            let blocking = unacked.is_empty();

            // Dynamic request count: never request more IDs than the unacked
            // capacity allows, capping at MAX_TX_IDS_PER_REQUEST per round.
            let remaining_capacity = MAX_UNACKED.saturating_sub(unacked.len());
            let req_count = (remaining_capacity as u16).min(MAX_TX_IDS_PER_REQUEST);

            // Request more tx IDs
            tracing::debug!(
                blocking,
                ack_count,
                req_count,
                unacked = unacked.len(),
                inflight = inflight.len(),
                "txsubmission2 server: sending MsgRequestTxIds"
            );
            let req = encode_message(&TxSubmissionMessage::MsgRequestTxIds {
                blocking,
                ack_count,
                req_count,
            });
            channel.send(req).await.map_err(ProtocolError::from)?;

            // Receive reply
            let reply_bytes = channel.recv().await.map_err(ProtocolError::from)?;
            let reply = decode_message(&reply_bytes).map_err(|e| ProtocolError::CborDecode {
                protocol: "TxSubmission2",
                reason: e,
            })?;

            match reply {
                TxSubmissionMessage::MsgReplyTxIds(ids) => {
                    if ids.is_empty() && !blocking {
                        // Non-blocking with empty reply — peer has no txs right now
                        continue;
                    }

                    // ── Bounds check 1: reply must not exceed the requested count ──
                    // A well-behaved peer never sends more IDs than were requested.
                    if ids.len() > req_count as usize {
                        return Err(ProtocolError::BoundsExceeded {
                            protocol: "TxSubmission2",
                            reason: format!(
                                "peer sent {} tx IDs but only {} were requested",
                                ids.len(),
                                req_count
                            ),
                        });
                    }

                    // ── Bounds check 2: unacked queue must not overflow ──
                    // Defense-in-depth: even if reply size is within req_count, the
                    // total unacked count must stay within MAX_UNACKED.
                    if unacked.len() + ids.len() > MAX_UNACKED {
                        return Err(ProtocolError::BoundsExceeded {
                            protocol: "TxSubmission2",
                            reason: format!(
                                "unacked count would reach {} (max {})",
                                unacked.len() + ids.len(),
                                MAX_UNACKED
                            ),
                        });
                    }

                    // Track new tx IDs, dedup against inflight.
                    // to_fetch carries (era_id, tx_hash) pairs for MsgRequestTxs.
                    let mut to_fetch: Vec<(u8, [u8; 32])> = Vec::new();
                    for id in &ids {
                        if !inflight.contains(&id.tx_id) {
                            inflight.insert(id.tx_id);
                            to_fetch.push((id.era_id, id.tx_id));
                        }
                        unacked.push_back(id.clone());
                    }
                    stats.tx_ids_received += ids.len() as u64;

                    // ── Bounds check 3: inflight must not overflow ──
                    // Protects against a peer that advertises many unique tx IDs
                    // without ever providing the corresponding tx bodies.
                    if inflight.len() > MAX_INFLIGHT_TX_IDS {
                        return Err(ProtocolError::BoundsExceeded {
                            protocol: "TxSubmission2",
                            reason: format!(
                                "inflight tx IDs reached {} (max {})",
                                inflight.len(),
                                MAX_INFLIGHT_TX_IDS
                            ),
                        });
                    }

                    if to_fetch.is_empty() {
                        continue;
                    }

                    // Request full tx bodies — MsgRequestTxs carries (era_id, tx_hash) pairs.
                    let req_txs =
                        encode_message(&TxSubmissionMessage::MsgRequestTxs(to_fetch.clone()));
                    channel.send(req_txs).await.map_err(ProtocolError::from)?;

                    let txs_bytes = channel.recv().await.map_err(ProtocolError::from)?;
                    let txs_reply =
                        decode_message(&txs_bytes).map_err(|e| ProtocolError::CborDecode {
                            protocol: "TxSubmission2",
                            reason: e,
                        })?;

                    if let TxSubmissionMessage::MsgReplyTxs(txs) = txs_reply {
                        for (i, (_era_id, tx_bytes)) in txs.into_iter().enumerate() {
                            stats.txs_received += 1;

                            // tx_id is the hash portion of the (era_id, hash) pair.
                            let tx_id = if i < to_fetch.len() {
                                to_fetch[i].1
                            } else {
                                [0; 32]
                            };

                            // Pass raw tx bytes (era wrapper stripped) to the callback.
                            if on_tx(tx_id, tx_bytes) {
                                stats.txs_accepted += 1;
                            } else {
                                stats.txs_rejected += 1;
                            }

                            // Remove from inflight using the hash component.
                            if i < to_fetch.len() {
                                inflight.remove(&to_fetch[i].1);
                            }
                        }
                    }
                }
                TxSubmissionMessage::MsgDone => {
                    tracing::debug!(?stats, "txsubmission: client sent MsgDone");
                    return Ok(stats);
                }
                other => {
                    return Err(ProtocolError::StateViolation {
                        protocol: "TxSubmission2",
                        expected: "MsgReplyTxIds or MsgDone".to_string(),
                        actual: format!("{other:?}"),
                    });
                }
            }
        }
    }
}

/// Statistics from a TxSubmission2 session.
#[derive(Debug, Clone, Default)]
pub struct TxSubmissionStats {
    /// Number of tx IDs received from the remote peer.
    pub tx_ids_received: u64,
    /// Number of full transactions received.
    pub txs_received: u64,
    /// Number of transactions that passed validation.
    pub txs_accepted: u64,
    /// Number of transactions that failed validation.
    pub txs_rejected: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use tokio::sync::mpsc;

    fn make_test_channel() -> (
        MuxChannel,
        mpsc::Receiver<(u16, crate::mux::Direction, Bytes)>,
        mpsc::Sender<Bytes>,
    ) {
        let (egress_tx, egress_rx) = mpsc::channel(64);
        let (ingress_tx, ingress_rx) = mpsc::channel(64);
        let channel = MuxChannel::new(
            4,
            crate::mux::Direction::ResponderDir,
            egress_tx,
            ingress_rx,
            1_000_000,
            std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        );
        (channel, egress_rx, ingress_tx)
    }

    #[tokio::test]
    async fn server_blocking_mode_matches_haskell_invariants() {
        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();

        let handle = tokio::spawn(async move {
            TxSubmissionServer::run(&mut channel, |_tx_id, _tx_bytes| true).await
        });

        // Send MsgInit
        let init = encode_message(&TxSubmissionMessage::MsgInit);
        ingress_tx.send(Bytes::from(init)).await.unwrap();

        // Read first MsgRequestTxIds — MUST be blocking (unacked is empty).
        // The Haskell txSubmissionOutbound enforces:
        //   non-blocking when unacked is empty → ProtocolErrorRequestNonBlocking
        let (_, _, req) = egress_rx.recv().await.unwrap();
        if let TxSubmissionMessage::MsgRequestTxIds {
            blocking,
            ack_count,
            ..
        } = decode_message(&req).unwrap()
        {
            assert!(
                blocking,
                "first request must be blocking (unacked is empty)"
            );
            assert_eq!(ack_count, 0, "first request must have ack_count=0");
        } else {
            panic!("expected MsgRequestTxIds");
        }

        // Reply with empty (no txs) — client has nothing
        let reply = encode_message(&TxSubmissionMessage::MsgReplyTxIds(vec![]));
        ingress_tx.send(Bytes::from(reply)).await.unwrap();

        // Second request should also be blocking (still no unacked IDs).
        let (_, _, req2) = egress_rx.recv().await.unwrap();
        if let TxSubmissionMessage::MsgRequestTxIds { blocking, .. } =
            decode_message(&req2).unwrap()
        {
            assert!(
                blocking,
                "second request must be blocking when unacked is still empty"
            );
        }

        // Send MsgDone (allowed in blocking state with no txs)
        let done = encode_message(&TxSubmissionMessage::MsgDone);
        ingress_tx.send(Bytes::from(done)).await.unwrap();

        let result = handle.await.unwrap().unwrap();
        assert_eq!(result.tx_ids_received, 0);
    }

    #[tokio::test]
    async fn server_receives_and_validates_txs() {
        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();

        let accepted = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let accepted_clone = accepted.clone();

        let handle = tokio::spawn(async move {
            TxSubmissionServer::run(&mut channel, move |tx_id, tx_bytes| {
                accepted_clone.lock().unwrap().push((tx_id, tx_bytes));
                true // accept all
            })
            .await
        });

        // Send MsgInit
        ingress_tx
            .send(Bytes::from(encode_message(&TxSubmissionMessage::MsgInit)))
            .await
            .unwrap();

        // Read first request
        let _ = egress_rx.recv().await.unwrap();

        // Reply with one tx ID
        let reply = encode_message(&TxSubmissionMessage::MsgReplyTxIds(vec![TxIdAndSize {
            era_id: 6,
            tx_id: [0xAA; 32],
            size_in_bytes: 100,
        }]));
        ingress_tx.send(Bytes::from(reply)).await.unwrap();

        // Read MsgRequestTxs
        let (_, _, req_txs) = egress_rx.recv().await.unwrap();
        assert!(matches!(
            decode_message(&req_txs).unwrap(),
            TxSubmissionMessage::MsgRequestTxs(_)
        ));

        // Reply with tx body — (era_id, tx_cbor) tuple
        let reply_txs = encode_message(&TxSubmissionMessage::MsgReplyTxs(vec![(
            6u8,
            vec![0x01, 0x02],
        )]));
        ingress_tx.send(Bytes::from(reply_txs)).await.unwrap();

        // Read next request (should have ack_count > 0)
        let (_, _, req2) = egress_rx.recv().await.unwrap();
        if let TxSubmissionMessage::MsgRequestTxIds { ack_count, .. } =
            decode_message(&req2).unwrap()
        {
            assert!(ack_count > 0, "should acknowledge received tx IDs");
        }

        // Send MsgDone
        ingress_tx
            .send(Bytes::from(encode_message(&TxSubmissionMessage::MsgDone)))
            .await
            .unwrap();

        let stats = handle.await.unwrap().unwrap();
        assert_eq!(stats.tx_ids_received, 1);
        assert_eq!(stats.txs_received, 1);
        assert_eq!(stats.txs_accepted, 1);

        let accepted = accepted.lock().unwrap();
        assert_eq!(accepted.len(), 1);
        assert_eq!(accepted[0].0, [0xAA; 32]);
        assert_eq!(accepted[0].1, vec![0x01, 0x02]);
    }

    /// Helper: generate N unique TxIdAndSize entries with distinct tx_id bytes.
    fn make_tx_ids(n: usize, start_byte: u8) -> Vec<TxIdAndSize> {
        (0..n)
            .map(|i| {
                let mut id = [0u8; 32];
                // Use two bytes to support up to 65535 unique IDs.
                id[0] = start_byte;
                id[1] = (i >> 8) as u8;
                id[2] = (i & 0xFF) as u8;
                TxIdAndSize {
                    era_id: 6,
                    tx_id: id,
                    size_in_bytes: 100,
                }
            })
            .collect()
    }

    #[tokio::test]
    async fn server_rejects_reply_exceeding_request() {
        // Peer sends 11 tx IDs when only MAX_TX_IDS_PER_REQUEST (10) were requested.
        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();

        let handle = tokio::spawn(async move {
            TxSubmissionServer::run(&mut channel, |_tx_id, _tx_bytes| true).await
        });

        // MsgInit
        ingress_tx
            .send(Bytes::from(encode_message(&TxSubmissionMessage::MsgInit)))
            .await
            .unwrap();

        // Read first request
        let _ = egress_rx.recv().await.unwrap();

        // Reply with 11 tx IDs (exceeds the 10 requested)
        let too_many = make_tx_ids(11, 0xBB);
        let reply = encode_message(&TxSubmissionMessage::MsgReplyTxIds(too_many));
        ingress_tx.send(Bytes::from(reply)).await.unwrap();

        let result = handle.await.unwrap();
        assert!(result.is_err(), "server should reject oversized reply");
        let err = result.unwrap_err();
        assert!(
            matches!(err, ProtocolError::BoundsExceeded { .. }),
            "expected BoundsExceeded, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn server_normal_flow_within_bounds() {
        // Normal flow: peer sends exactly MAX_TX_IDS_PER_REQUEST IDs, all accepted.
        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();

        let handle = tokio::spawn(async move {
            TxSubmissionServer::run(&mut channel, |_tx_id, _tx_bytes| true).await
        });

        // MsgInit
        ingress_tx
            .send(Bytes::from(encode_message(&TxSubmissionMessage::MsgInit)))
            .await
            .unwrap();

        // Read first request
        let _ = egress_rx.recv().await.unwrap();

        // Reply with exactly 10 tx IDs (within bounds)
        let ids = make_tx_ids(MAX_TX_IDS_PER_REQUEST as usize, 0xCC);
        let reply = encode_message(&TxSubmissionMessage::MsgReplyTxIds(ids.clone()));
        ingress_tx.send(Bytes::from(reply)).await.unwrap();

        // Read MsgRequestTxs
        let (_, _, req_txs) = egress_rx.recv().await.unwrap();
        let req = decode_message(&req_txs).unwrap();
        let fetch_ids = match req {
            TxSubmissionMessage::MsgRequestTxs(ref f) => f.clone(),
            _ => panic!("expected MsgRequestTxs"),
        };
        assert_eq!(fetch_ids.len(), MAX_TX_IDS_PER_REQUEST as usize);

        // Reply with tx bodies
        let txs: Vec<(u8, Vec<u8>)> = fetch_ids
            .iter()
            .map(|(era, _)| (*era, vec![0x42]))
            .collect();
        let reply_txs = encode_message(&TxSubmissionMessage::MsgReplyTxs(txs));
        ingress_tx.send(Bytes::from(reply_txs)).await.unwrap();

        // Read next request (should ack the 10 IDs)
        let (_, _, req2) = egress_rx.recv().await.unwrap();
        if let TxSubmissionMessage::MsgRequestTxIds { ack_count, .. } =
            decode_message(&req2).unwrap()
        {
            assert_eq!(ack_count, MAX_TX_IDS_PER_REQUEST);
        } else {
            panic!("expected MsgRequestTxIds");
        }

        // Send MsgDone
        ingress_tx
            .send(Bytes::from(encode_message(&TxSubmissionMessage::MsgDone)))
            .await
            .unwrap();

        let stats = handle.await.unwrap().unwrap();
        assert_eq!(stats.tx_ids_received, MAX_TX_IDS_PER_REQUEST as u64);
        assert_eq!(stats.txs_received, MAX_TX_IDS_PER_REQUEST as u64);
        assert_eq!(stats.txs_accepted, MAX_TX_IDS_PER_REQUEST as u64);
    }
}
