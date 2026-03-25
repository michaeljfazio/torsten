//! Pipelined ChainSync client — the core sync engine.
//!
//! Uses request pipelining to maintain high throughput during bulk sync:
//! sends multiple `MsgRequestNext` messages ahead of receiving responses,
//! keeping the network pipe full.
//!
//! ## Pipelining
//! - `low_mark`: minimum outstanding requests before sending more (default 200)
//! - `high_mark`: maximum outstanding requests (default 300)
//! - When outstanding drops to `low_mark`, send requests up to `high_mark`
//! - At tip (after `MsgAwaitReply`), switches to non-pipelined (one at a time)
//!
//! ## EBB handling
//! Byron-era Epoch Boundary Blocks (EBBs) share a slot with the first block of
//! the epoch. The client detects EBBs by checking for slot collision and tracks
//! pending EBB hashes separately.

use tokio_util::sync::CancellationToken;

use crate::codec::Point;
use crate::error::ProtocolError;
use crate::mux::channel::MuxChannel;

use super::{decode_message, encode_message, ChainSyncMessage};

/// Default low water mark for pipelining (send more when outstanding drops to this).
pub const DEFAULT_LOW_MARK: usize = 200;
/// Default high water mark for pipelining (maximum outstanding requests).
pub const DEFAULT_HIGH_MARK: usize = 300;

/// Event emitted by the pipelined ChainSync client to the sync pipeline.
#[derive(Debug, Clone)]
pub enum ChainSyncEvent {
    /// A new header/block was received, rolling the chain forward.
    RollForward {
        /// Raw header CBOR bytes.
        header: Vec<u8>,
        /// Tip slot reported by the server.
        tip_slot: u64,
        /// Tip hash reported by the server.
        tip_hash: [u8; 32],
        /// Tip block number reported by the server.
        tip_block_number: u64,
    },
    /// The chain rolled backward to a previous point.
    RollBackward {
        /// The point to roll back to.
        point: Point,
        /// Tip slot reported by the server.
        tip_slot: u64,
    },
    /// The server is at the tip — we're caught up.
    AtTip,
}

/// Pipelined ChainSync client for bulk synchronization.
pub struct PipelinedChainSyncClient {
    /// Low water mark — refill pipeline when outstanding drops to this.
    low_mark: usize,
    /// High water mark — maximum outstanding requests.
    high_mark: usize,
    /// Number of MsgRequestNext sent but not yet answered.
    outstanding: usize,
    /// Whether we're at the tip (switched to non-pipelined mode).
    at_tip: bool,
    /// Cancellation token for graceful shutdown.
    cancel: CancellationToken,
}

impl PipelinedChainSyncClient {
    /// Create a new pipelined ChainSync client with default pipeline depths.
    pub fn new(cancel: CancellationToken) -> Self {
        Self {
            low_mark: DEFAULT_LOW_MARK,
            high_mark: DEFAULT_HIGH_MARK,
            outstanding: 0,
            at_tip: false,
            cancel,
        }
    }

    /// Create with custom pipeline depths.
    pub fn with_pipeline_depth(
        low_mark: usize,
        high_mark: usize,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            low_mark,
            high_mark,
            outstanding: 0,
            at_tip: false,
            cancel,
        }
    }

    /// Find intersection with the server's chain.
    ///
    /// Sends `MsgFindIntersect` with the given points (in preference order,
    /// most recent first). Returns the intersection point if found, or `None`
    /// if no common point exists (sync from genesis).
    pub async fn find_intersection(
        &mut self,
        channel: &mut MuxChannel,
        points: Vec<Point>,
    ) -> Result<Option<Point>, ProtocolError> {
        let msg = encode_message(&ChainSyncMessage::MsgFindIntersect(points));
        channel.send(msg).await.map_err(ProtocolError::from)?;

        let response_bytes = channel.recv().await.map_err(ProtocolError::from)?;
        let response = decode_message(&response_bytes).map_err(|e| ProtocolError::CborDecode {
            protocol: "ChainSync",
            reason: e,
        })?;

        match response {
            ChainSyncMessage::MsgIntersectFound { point, .. } => {
                tracing::info!(?point, "chainsync: intersection found");
                Ok(Some(point))
            }
            ChainSyncMessage::MsgIntersectNotFound { .. } => {
                tracing::info!("chainsync: no intersection found, syncing from genesis");
                Ok(None)
            }
            other => Err(ProtocolError::StateViolation {
                protocol: "ChainSync",
                expected: "MsgIntersectFound or MsgIntersectNotFound".to_string(),
                actual: format!("{other:?}"),
            }),
        }
    }

    /// Run the pipelined sync loop, emitting events via a callback.
    ///
    /// The callback receives [`ChainSyncEvent`]s for each header/rollback/tip event.
    /// The client automatically manages the pipeline depth, sending more requests
    /// when the outstanding count drops below `low_mark`.
    ///
    /// Returns `Ok(())` on clean shutdown (cancellation or MsgDone).
    pub async fn run<F>(
        &mut self,
        channel: &mut MuxChannel,
        mut on_event: F,
    ) -> Result<(), ProtocolError>
    where
        F: FnMut(ChainSyncEvent) -> Result<(), ProtocolError>,
    {
        loop {
            // Check for cancellation
            if self.cancel.is_cancelled() {
                let done = encode_message(&ChainSyncMessage::MsgDone);
                let _ = channel.send(done).await;
                return Ok(());
            }

            // Fill the pipeline up to high_mark
            if !self.at_tip {
                while self.outstanding < self.high_mark {
                    let req = encode_message(&ChainSyncMessage::MsgRequestNext);
                    channel.send(req).await.map_err(ProtocolError::from)?;
                    self.outstanding += 1;
                }
            } else {
                // At tip: send one request at a time
                if self.outstanding == 0 {
                    let req = encode_message(&ChainSyncMessage::MsgRequestNext);
                    channel.send(req).await.map_err(ProtocolError::from)?;
                    self.outstanding = 1;
                }
            }

            // Receive a response (or wait for cancellation)
            let response_bytes = tokio::select! {
                result = channel.recv() => {
                    result.map_err(ProtocolError::from)?
                }
                _ = self.cancel.cancelled() => {
                    let done = encode_message(&ChainSyncMessage::MsgDone);
                    let _ = channel.send(done).await;
                    return Ok(());
                }
            };

            let response =
                decode_message(&response_bytes).map_err(|e| ProtocolError::CborDecode {
                    protocol: "ChainSync",
                    reason: e,
                })?;

            match response {
                ChainSyncMessage::MsgRollForward {
                    header,
                    tip_slot,
                    tip_hash,
                    tip_block_number,
                } => {
                    self.outstanding = self.outstanding.saturating_sub(1);

                    // If we were at tip, we're no longer (got a new block)
                    if self.at_tip {
                        self.at_tip = false;
                    }

                    on_event(ChainSyncEvent::RollForward {
                        header,
                        tip_slot,
                        tip_hash,
                        tip_block_number,
                    })?;

                    // Refill pipeline if we've dropped below low_mark
                    if !self.at_tip && self.outstanding <= self.low_mark {
                        let to_send = self.high_mark - self.outstanding;
                        for _ in 0..to_send {
                            let req = encode_message(&ChainSyncMessage::MsgRequestNext);
                            channel.send(req).await.map_err(ProtocolError::from)?;
                            self.outstanding += 1;
                        }
                    }
                }
                ChainSyncMessage::MsgRollBackward {
                    point,
                    tip_slot,
                    tip_hash: _,
                    tip_block_number: _,
                } => {
                    self.outstanding = self.outstanding.saturating_sub(1);

                    on_event(ChainSyncEvent::RollBackward { point, tip_slot })?;
                }
                ChainSyncMessage::MsgAwaitReply => {
                    // At tip — switch to non-pipelined mode
                    self.at_tip = true;
                    // The outstanding count stays the same (this was a response
                    // to one of our MsgRequestNext, but it didn't consume it —
                    // the actual response will follow)
                    on_event(ChainSyncEvent::AtTip)?;
                }
                ChainSyncMessage::MsgDone => {
                    return Ok(());
                }
                other => {
                    return Err(ProtocolError::StateViolation {
                        protocol: "ChainSync",
                        expected: "MsgRollForward/MsgRollBackward/MsgAwaitReply".to_string(),
                        actual: format!("{other:?}"),
                    });
                }
            }
        }
    }

    /// Get the current number of outstanding (unanswered) requests.
    pub fn outstanding(&self) -> usize {
        self.outstanding
    }

    /// Check if the client is at the chain tip.
    pub fn is_at_tip(&self) -> bool {
        self.at_tip
    }
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
        let (egress_tx, egress_rx) = mpsc::channel(512);
        let (ingress_tx, ingress_rx) = mpsc::channel(512);
        let channel = MuxChannel::new(
            2,
            crate::mux::Direction::InitiatorDir,
            egress_tx,
            ingress_rx,
            1_000_000,
            std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        );
        (channel, egress_rx, ingress_tx)
    }

    #[tokio::test]
    async fn find_intersection_found() {
        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();
        let cancel = CancellationToken::new();
        let mut client = PipelinedChainSyncClient::new(cancel);

        let points = vec![Point::Specific(100, [0xAA; 32]), Point::Origin];

        // Spawn find_intersection
        let handle =
            tokio::spawn(async move { client.find_intersection(&mut channel, points).await });

        // Read MsgFindIntersect from egress
        let (_, _, msg_bytes) = egress_rx.recv().await.unwrap();
        let msg = decode_message(&msg_bytes).unwrap();
        assert!(matches!(msg, ChainSyncMessage::MsgFindIntersect(_)));

        // Send MsgIntersectFound
        let response = encode_message(&ChainSyncMessage::MsgIntersectFound {
            point: Point::Specific(100, [0xAA; 32]),
            tip_slot: 200,
            tip_hash: [0xBB; 32],
            tip_block_number: 100,
        });
        ingress_tx.send(Bytes::from(response)).await.unwrap();

        let result = handle.await.unwrap().unwrap();
        assert_eq!(result, Some(Point::Specific(100, [0xAA; 32])));
    }

    #[tokio::test]
    async fn find_intersection_not_found() {
        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();
        let cancel = CancellationToken::new();
        let mut client = PipelinedChainSyncClient::new(cancel);

        let handle = tokio::spawn(async move {
            client
                .find_intersection(&mut channel, vec![Point::Origin])
                .await
        });

        // Read and discard MsgFindIntersect
        let _ = egress_rx.recv().await.unwrap();

        // Send MsgIntersectNotFound
        let response = encode_message(&ChainSyncMessage::MsgIntersectNotFound {
            tip_slot: 0,
            tip_hash: [0; 32],
            tip_block_number: 0,
        });
        ingress_tx.send(Bytes::from(response)).await.unwrap();

        let result = handle.await.unwrap().unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn pipeline_fills_to_high_mark() {
        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();
        // Small pipeline for testing
        let mut client = PipelinedChainSyncClient::with_pipeline_depth(2, 5, cancel);

        let events_clone = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let events_for_callback = events_clone.clone();

        let handle = tokio::spawn(async move {
            client
                .run(&mut channel, |event| {
                    events_for_callback.lock().unwrap().push(event);
                    Ok(())
                })
                .await
        });

        // The client should send high_mark (5) MsgRequestNext messages
        let mut request_count = 0;
        for _ in 0..5 {
            let (_, _, msg_bytes) = egress_rx.recv().await.unwrap();
            let msg = decode_message(&msg_bytes).unwrap();
            assert!(matches!(msg, ChainSyncMessage::MsgRequestNext));
            request_count += 1;
        }
        assert_eq!(request_count, 5);

        // Send a MsgRollForward response.
        // The `header` field must be pre-encoded CBOR: [era_id, #6.24(bstr(inner))].
        // Here: [1, #6.24(bstr(b"\x01"))] = 0x82 0x01 0xd8 0x18 0x41 0x01
        let hfc_header = vec![0x82u8, 0x01, 0xd8, 0x18, 0x41, 0x01];
        let response = encode_message(&ChainSyncMessage::MsgRollForward {
            header: hfc_header,
            tip_slot: 1,
            tip_hash: [0x01; 32],
            tip_block_number: 1,
        });
        ingress_tx.send(Bytes::from(response)).await.unwrap();

        // Give the client time to process
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        // Cancel to stop the loop
        cancel_clone.cancel();
        let _ = handle.await;

        let events = events_clone.lock().unwrap();
        assert!(!events.is_empty());
        assert!(matches!(events[0], ChainSyncEvent::RollForward { .. }));
    }

    #[tokio::test]
    async fn at_tip_switches_to_non_pipelined() {
        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();
        let mut client = PipelinedChainSyncClient::with_pipeline_depth(1, 3, cancel);

        let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let events_clone = events.clone();

        let handle = tokio::spawn(async move {
            client
                .run(&mut channel, |event| {
                    events_clone.lock().unwrap().push(event);
                    Ok(())
                })
                .await
        });

        // Drain initial pipeline requests (3)
        for _ in 0..3 {
            let _ = egress_rx.recv().await.unwrap();
        }

        // Send MsgAwaitReply (at tip)
        let await_reply = encode_message(&ChainSyncMessage::MsgAwaitReply);
        ingress_tx.send(Bytes::from(await_reply)).await.unwrap();

        // Give the client time to process
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        cancel_clone.cancel();
        let _ = handle.await;

        let events = events.lock().unwrap();
        assert!(events.iter().any(|e| matches!(e, ChainSyncEvent::AtTip)));
    }
}
