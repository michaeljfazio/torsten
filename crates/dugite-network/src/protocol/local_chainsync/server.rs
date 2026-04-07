//! LocalChainSync server — serves full blocks to N2C clients.
//!
//! Uses the same ChainSync message wire format (tags 0-7) but wraps block data
//! in `Serialised` encoding: `tag(24)(bytes(block_cbor))` (CBOR-in-CBOR).
//! This matches the Haskell `SerialiseNodeToClient` encoding for `Serialised blk`.

use minicbor::Encoder;
use tokio::sync::broadcast;

use crate::codec::Point;
use crate::error::ProtocolError;
use crate::mux::channel::MuxChannel;
use crate::protocol::chainsync::server::{BlockAnnouncement, RollbackAnnouncement};
use crate::protocol::chainsync::{decode_message, encode_message, ChainSyncMessage};
use crate::BlockProvider;

/// Wrap raw block CBOR in `Serialised` encoding: `tag(24)(bytes(block_cbor))`.
///
/// N2C LocalChainSync sends blocks as `Serialised (HardForkBlock xs)` which
/// uses CBOR-in-CBOR wrapping. The inner bytes are the full multi-era block
/// CBOR (including era tag) as stored in ChainDB.
fn wrap_serialised(block_cbor: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(block_cbor.len() + 10);
    let mut enc = Encoder::new(&mut buf);
    enc.tag(minicbor::data::Tag::new(24)).expect("infallible");
    enc.bytes(block_cbor).expect("infallible");
    buf
}

/// LocalChainSync server that serves full blocks to N2C clients.
///
/// Reuses the ChainSync message codec but wraps block bodies in HFC era encoding
/// instead of sending just headers.
pub struct LocalChainSyncServer {
    /// Current cursor: last slot served to this client.
    cursor_slot: u64,
    /// Current cursor: last hash served to this client.
    cursor_hash: [u8; 32],
}

impl LocalChainSyncServer {
    /// Create a new server with no cursor.
    pub fn new() -> Self {
        Self {
            cursor_slot: 0,
            cursor_hash: [0; 32],
        }
    }

    /// Run the LocalChainSync server loop.
    ///
    /// Accepts a `rollback_rx` receiver to propagate chain rollbacks to N2C
    /// clients via `MsgRollBackward`.
    pub async fn run<B: BlockProvider>(
        &mut self,
        channel: &mut MuxChannel,
        block_provider: &B,
        mut announcement_rx: broadcast::Receiver<BlockAnnouncement>,
        mut rollback_rx: broadcast::Receiver<RollbackAnnouncement>,
    ) -> Result<(), ProtocolError> {
        loop {
            let msg_bytes = channel.recv().await.map_err(ProtocolError::from)?;
            let msg = decode_message(&msg_bytes).map_err(|e| ProtocolError::CborDecode {
                protocol: "LocalChainSync",
                reason: e,
            })?;

            match msg {
                ChainSyncMessage::MsgFindIntersect(points) => {
                    self.handle_find_intersect(channel, block_provider, &points)
                        .await?;
                }
                ChainSyncMessage::MsgRequestNext => {
                    self.handle_request_next(
                        channel,
                        block_provider,
                        &mut announcement_rx,
                        &mut rollback_rx,
                    )
                    .await?;
                }
                ChainSyncMessage::MsgDone => {
                    tracing::debug!("local chainsync server: client sent MsgDone");
                    return Ok(());
                }
                other => {
                    return Err(ProtocolError::AgencyViolation {
                        protocol: "LocalChainSync",
                        state: "StIdle".to_string(),
                        received_tag: format!("{other:?}")
                            .as_bytes()
                            .first()
                            .copied()
                            .unwrap_or(0),
                    });
                }
            }
        }
    }

    /// Handle MsgFindIntersect — identical to N2N ChainSync.
    async fn handle_find_intersect<B: BlockProvider>(
        &mut self,
        channel: &mut MuxChannel,
        block_provider: &B,
        points: &[Point],
    ) -> Result<(), ProtocolError> {
        let tip = block_provider.get_tip();

        for point in points {
            match point {
                Point::Origin => {
                    self.cursor_slot = 0;
                    self.cursor_hash = [0; 32];
                    let response = encode_message(&ChainSyncMessage::MsgIntersectFound {
                        point: Point::Origin,
                        tip_slot: tip.slot,
                        tip_hash: tip.hash,
                        tip_block_number: tip.block_number,
                    });
                    channel.send(response).await.map_err(ProtocolError::from)?;
                    return Ok(());
                }
                Point::Specific(slot, hash) => {
                    if block_provider.has_block(hash) {
                        self.cursor_slot = *slot;
                        self.cursor_hash = *hash;
                        let response = encode_message(&ChainSyncMessage::MsgIntersectFound {
                            point: point.clone(),
                            tip_slot: tip.slot,
                            tip_hash: tip.hash,
                            tip_block_number: tip.block_number,
                        });
                        channel.send(response).await.map_err(ProtocolError::from)?;
                        return Ok(());
                    }
                }
            }
        }

        let response = encode_message(&ChainSyncMessage::MsgIntersectNotFound {
            tip_slot: tip.slot,
            tip_hash: tip.hash,
            tip_block_number: tip.block_number,
        });
        channel.send(response).await.map_err(ProtocolError::from)?;
        Ok(())
    }

    /// Handle MsgRequestNext — sends full blocks (not headers) with HFC wrapping,
    /// or sends MsgRollBackward if a rollback has occurred.
    async fn handle_request_next<B: BlockProvider>(
        &mut self,
        channel: &mut MuxChannel,
        block_provider: &B,
        announcement_rx: &mut broadcast::Receiver<BlockAnnouncement>,
        rollback_rx: &mut broadcast::Receiver<RollbackAnnouncement>,
    ) -> Result<(), ProtocolError> {
        // ── Check for pending rollbacks before serving ──────────────────────
        if let Some(rb) = Self::drain_rollback(rollback_rx) {
            if self.cursor_slot > rb.slot
                || (self.cursor_slot == rb.slot && self.cursor_hash != rb.hash)
            {
                return self.send_rollback(channel, block_provider, &rb).await;
            }
        }

        if let Some((slot, hash, block_cbor)) =
            block_provider.get_next_block_after_slot(self.cursor_slot)
        {
            let tip = block_provider.get_tip();

            // N2C sends full blocks wrapped in Serialised encoding: tag(24)(bytes(block_cbor)).
            let response = encode_message(&ChainSyncMessage::MsgRollForward {
                header: wrap_serialised(&block_cbor),
                tip_slot: tip.slot,
                tip_hash: tip.hash,
                tip_block_number: tip.block_number,
            });
            channel.send(response).await.map_err(ProtocolError::from)?;
            self.cursor_slot = slot;
            self.cursor_hash = hash;
            return Ok(());
        }

        // At tip — wait for announcement or rollback.
        let await_msg = encode_message(&ChainSyncMessage::MsgAwaitReply);
        channel.send(await_msg).await.map_err(ProtocolError::from)?;

        tokio::select! {
            // ── Rollback while waiting at tip ───────────────────────────────
            rollback = rollback_rx.recv() => {
                match rollback {
                    Ok(rb) => {
                        if self.cursor_slot > rb.slot
                            || (self.cursor_slot == rb.slot && self.cursor_hash != rb.hash)
                        {
                            self.send_rollback(channel, block_provider, &rb).await
                        } else {
                            // Cursor behind rollback — serve next block from new fork.
                            if let Some((slot, hash, block_cbor)) =
                                block_provider.get_next_block_after_slot(self.cursor_slot)
                            {
                                let tip = block_provider.get_tip();
                                let response = encode_message(&ChainSyncMessage::MsgRollForward {
                                    header: wrap_serialised(&block_cbor),
                                    tip_slot: tip.slot,
                                    tip_hash: tip.hash,
                                    tip_block_number: tip.block_number,
                                });
                                channel.send(response).await.map_err(ProtocolError::from)?;
                                self.cursor_slot = slot;
                                self.cursor_hash = hash;
                            }
                            Ok(())
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        let tip = block_provider.get_tip();
                        let rb = RollbackAnnouncement {
                            slot: tip.slot,
                            hash: tip.hash,
                        };
                        self.send_rollback(channel, block_provider, &rb).await
                    }
                    Err(broadcast::error::RecvError::Closed) => Ok(()),
                }
            }
            announcement = announcement_rx.recv() => {
                match announcement {
                    Ok(ann) => {
                        if let Some(block_cbor) = block_provider.get_block(&ann.hash) {
                            let tip = block_provider.get_tip();
                            let response = encode_message(&ChainSyncMessage::MsgRollForward {
                                header: wrap_serialised(&block_cbor),
                                tip_slot: tip.slot,
                                tip_hash: tip.hash,
                                tip_block_number: tip.block_number,
                            });
                            channel.send(response).await.map_err(ProtocolError::from)?;
                            self.cursor_slot = ann.slot;
                            self.cursor_hash = ann.hash;
                        }
                        Ok(())
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        // Try to catch up from current position.
                        if let Some((slot, hash, block_cbor)) =
                            block_provider.get_next_block_after_slot(self.cursor_slot)
                        {
                            let tip = block_provider.get_tip();
                            let response = encode_message(&ChainSyncMessage::MsgRollForward {
                                header: wrap_serialised(&block_cbor),
                                tip_slot: tip.slot,
                                tip_hash: tip.hash,
                                tip_block_number: tip.block_number,
                            });
                            channel.send(response).await.map_err(ProtocolError::from)?;
                            self.cursor_slot = slot;
                            self.cursor_hash = hash;
                        }
                        Ok(())
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        // Node shutting down.
                        Ok(())
                    }
                }
            }
        }
    }

    /// Drain any pending rollback announcements, returning the most recent one.
    fn drain_rollback(
        rollback_rx: &mut broadcast::Receiver<RollbackAnnouncement>,
    ) -> Option<RollbackAnnouncement> {
        let mut latest: Option<RollbackAnnouncement> = None;
        loop {
            match rollback_rx.try_recv() {
                Ok(rb) => latest = Some(rb),
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
        latest
    }

    /// Send `MsgRollBackward` to the N2C client and rewind the cursor.
    async fn send_rollback<B: BlockProvider>(
        &mut self,
        channel: &mut MuxChannel,
        block_provider: &B,
        rb: &RollbackAnnouncement,
    ) -> Result<(), ProtocolError> {
        let tip = block_provider.get_tip();
        let point = if rb.slot == 0 && rb.hash == [0u8; 32] {
            Point::Origin
        } else {
            Point::Specific(rb.slot, rb.hash)
        };

        tracing::info!(
            rollback_slot = rb.slot,
            cursor_slot = self.cursor_slot,
            "local chainsync server: sending MsgRollBackward to N2C client"
        );

        let response = encode_message(&ChainSyncMessage::MsgRollBackward {
            point,
            tip_slot: tip.slot,
            tip_hash: tip.hash,
            tip_block_number: tip.block_number,
        });
        channel.send(response).await.map_err(ProtocolError::from)?;

        // Rewind cursor to the rollback point.
        self.cursor_slot = rb.slot;
        self.cursor_hash = rb.hash;

        Ok(())
    }
}

impl Default for LocalChainSyncServer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::chainsync::server::{BlockAnnouncement, RollbackAnnouncement};
    use crate::protocol::chainsync::{decode_message, encode_message, ChainSyncMessage};
    use crate::TipInfo;
    use bytes::Bytes;
    use minicbor::Encoder;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;
    use tokio::sync::{broadcast, mpsc};

    // ─── Test infrastructure ─────────────────────────────────────────────────

    /// Create block CBOR as a CBOR bstr so the ChainSync codec decoder
    /// recognises the `header` field (which expects Array or Bytes CBOR type).
    ///
    /// LocalChainSync sends raw block CBOR in MsgRollForward.header. When
    /// decoded by `decode_message`, it must be a valid CBOR type (Array or
    /// Bytes). Using a bstr ensures the roundtrip works.
    fn make_block_cbor(payload: &[u8]) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut enc = Encoder::new(&mut buf);
        enc.bytes(payload).unwrap();
        buf
    }

    /// Mock block provider for LocalChainSync tests.
    ///
    /// Stores (slot, hash, block_cbor) tuples. The CBOR is raw block bytes —
    /// LocalChainSync passes them through without header extraction (unlike N2N
    /// ChainSync which extracts headers).
    struct MockBlockProvider {
        blocks: Vec<(u64, [u8; 32], Vec<u8>)>,
    }

    impl BlockProvider for MockBlockProvider {
        fn get_block(&self, hash: &[u8; 32]) -> Option<Vec<u8>> {
            self.blocks
                .iter()
                .find(|(_, h, _)| h == hash)
                .map(|(_, _, cbor)| cbor.clone())
        }

        fn has_block(&self, hash: &[u8; 32]) -> bool {
            self.blocks.iter().any(|(_, h, _)| h == hash)
        }

        fn get_tip(&self) -> TipInfo {
            self.blocks
                .last()
                .map(|(s, h, _)| TipInfo {
                    slot: *s,
                    hash: *h,
                    block_number: self.blocks.len() as u64,
                })
                .unwrap_or(TipInfo {
                    slot: 0,
                    hash: [0; 32],
                    block_number: 0,
                })
        }

        fn get_next_block_after_slot(&self, after_slot: u64) -> Option<(u64, [u8; 32], Vec<u8>)> {
            self.blocks
                .iter()
                .find(|(s, _, _)| *s > after_slot)
                .cloned()
        }
    }

    /// Create a test MuxChannel with egress receiver and ingress sender.
    fn make_test_channel() -> (
        crate::mux::channel::MuxChannel,
        mpsc::Receiver<(u16, crate::mux::Direction, Bytes)>,
        mpsc::Sender<Bytes>,
    ) {
        let (egress_tx, egress_rx) = mpsc::channel(64);
        let (ingress_tx, ingress_rx) = mpsc::channel(64);
        let channel = crate::mux::channel::MuxChannel::new(
            5, // LocalChainSync protocol ID
            crate::mux::Direction::ResponderDir,
            egress_tx,
            ingress_rx,
            1_000_000,
            Arc::new(AtomicUsize::new(0)),
        );
        (channel, egress_rx, ingress_tx)
    }

    /// Helper: spawn server, returning the join handle.
    fn spawn_server(
        mut channel: MuxChannel,
        provider: MockBlockProvider,
        ann_rx: broadcast::Receiver<BlockAnnouncement>,
        rb_rx: broadcast::Receiver<RollbackAnnouncement>,
    ) -> tokio::task::JoinHandle<Result<(), ProtocolError>> {
        tokio::spawn(async move {
            let mut server = LocalChainSyncServer::new();
            server.run(&mut channel, &provider, ann_rx, rb_rx).await
        })
    }

    /// Helper: decode egress message, stripping the mux header tuple.
    async fn recv_msg(
        egress_rx: &mut mpsc::Receiver<(u16, crate::mux::Direction, Bytes)>,
    ) -> ChainSyncMessage {
        let (_, _, bytes) = egress_rx.recv().await.expect("egress channel closed");
        decode_message(&bytes).expect("failed to decode ChainSync message")
    }

    /// Helper: send a ChainSync message through the ingress channel.
    async fn send_msg(ingress_tx: &mpsc::Sender<Bytes>, msg: &ChainSyncMessage) {
        let encoded = encode_message(msg);
        ingress_tx
            .send(Bytes::from(encoded))
            .await
            .expect("ingress channel closed");
    }

    // ─── FindIntersect tests ─────────────────────────────────────────────────

    #[tokio::test]
    async fn find_intersect_origin() {
        // Finding intersection at Origin should always succeed.
        let (channel, mut egress_rx, ingress_tx) = make_test_channel();
        let (ann_tx, _) = broadcast::channel(16);
        let (rb_tx, _) = broadcast::channel(16);

        let provider = MockBlockProvider {
            blocks: vec![(100, [0xAA; 32], make_block_cbor(&[0x01, 0x02]))],
        };

        let handle = spawn_server(channel, provider, ann_tx.subscribe(), rb_tx.subscribe());

        send_msg(
            &ingress_tx,
            &ChainSyncMessage::MsgFindIntersect(vec![Point::Origin]),
        )
        .await;

        let msg = recv_msg(&mut egress_rx).await;
        match msg {
            ChainSyncMessage::MsgIntersectFound {
                point,
                tip_slot,
                tip_hash,
                tip_block_number,
            } => {
                assert_eq!(point, Point::Origin);
                assert_eq!(tip_slot, 100);
                assert_eq!(tip_hash, [0xAA; 32]);
                assert_eq!(tip_block_number, 1);
            }
            other => panic!("expected MsgIntersectFound, got {other:?}"),
        }

        send_msg(&ingress_tx, &ChainSyncMessage::MsgDone).await;
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn find_intersect_specific_block() {
        // Finding intersection at a known specific block.
        let (channel, mut egress_rx, ingress_tx) = make_test_channel();
        let (ann_tx, _) = broadcast::channel(16);
        let (rb_tx, _) = broadcast::channel(16);

        let provider = MockBlockProvider {
            blocks: vec![
                (10, [0x01; 32], make_block_cbor(&[0xAA])),
                (20, [0x02; 32], make_block_cbor(&[0xBB])),
                (30, [0x03; 32], make_block_cbor(&[0xCC])),
            ],
        };

        let handle = spawn_server(channel, provider, ann_tx.subscribe(), rb_tx.subscribe());

        // Request intersection at the second block.
        send_msg(
            &ingress_tx,
            &ChainSyncMessage::MsgFindIntersect(vec![Point::Specific(20, [0x02; 32])]),
        )
        .await;

        let msg = recv_msg(&mut egress_rx).await;
        match msg {
            ChainSyncMessage::MsgIntersectFound {
                point,
                tip_slot,
                tip_hash,
                ..
            } => {
                assert_eq!(point, Point::Specific(20, [0x02; 32]));
                assert_eq!(tip_slot, 30);
                assert_eq!(tip_hash, [0x03; 32]);
            }
            other => panic!("expected MsgIntersectFound, got {other:?}"),
        }

        send_msg(&ingress_tx, &ChainSyncMessage::MsgDone).await;
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn find_intersect_not_found() {
        // When no requested points exist, server responds with MsgIntersectNotFound.
        let (channel, mut egress_rx, ingress_tx) = make_test_channel();
        let (ann_tx, _) = broadcast::channel(16);
        let (rb_tx, _) = broadcast::channel(16);

        let provider = MockBlockProvider {
            blocks: vec![(10, [0x01; 32], make_block_cbor(&[0xAA]))],
        };

        let handle = spawn_server(channel, provider, ann_tx.subscribe(), rb_tx.subscribe());

        // Request intersection at a nonexistent block.
        send_msg(
            &ingress_tx,
            &ChainSyncMessage::MsgFindIntersect(vec![Point::Specific(999, [0xFF; 32])]),
        )
        .await;

        let msg = recv_msg(&mut egress_rx).await;
        match msg {
            ChainSyncMessage::MsgIntersectNotFound {
                tip_slot, tip_hash, ..
            } => {
                assert_eq!(tip_slot, 10);
                assert_eq!(tip_hash, [0x01; 32]);
            }
            other => panic!("expected MsgIntersectNotFound, got {other:?}"),
        }

        send_msg(&ingress_tx, &ChainSyncMessage::MsgDone).await;
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn find_intersect_multiple_points_returns_first_match() {
        // When multiple points are provided, the server returns the first one that
        // exists in the chain (matching Haskell findIntersect behavior — clients
        // send points in reverse order, so the first match is the best intersection).
        let (channel, mut egress_rx, ingress_tx) = make_test_channel();
        let (ann_tx, _) = broadcast::channel(16);
        let (rb_tx, _) = broadcast::channel(16);

        let provider = MockBlockProvider {
            blocks: vec![
                (10, [0x01; 32], make_block_cbor(&[0xAA])),
                (20, [0x02; 32], make_block_cbor(&[0xBB])),
                (30, [0x03; 32], make_block_cbor(&[0xCC])),
            ],
        };

        let handle = spawn_server(channel, provider, ann_tx.subscribe(), rb_tx.subscribe());

        // Client sends points in reverse slot order (typical behavior).
        // First point (slot 30) exists, so it should be returned.
        send_msg(
            &ingress_tx,
            &ChainSyncMessage::MsgFindIntersect(vec![
                Point::Specific(30, [0x03; 32]),
                Point::Specific(10, [0x01; 32]),
                Point::Origin,
            ]),
        )
        .await;

        let msg = recv_msg(&mut egress_rx).await;
        match msg {
            ChainSyncMessage::MsgIntersectFound { point, .. } => {
                assert_eq!(
                    point,
                    Point::Specific(30, [0x03; 32]),
                    "should return the first matching point"
                );
            }
            other => panic!("expected MsgIntersectFound, got {other:?}"),
        }

        send_msg(&ingress_tx, &ChainSyncMessage::MsgDone).await;
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn find_intersect_falls_through_to_later_point() {
        // When the first point doesn't exist but a later one does, use the later one.
        let (channel, mut egress_rx, ingress_tx) = make_test_channel();
        let (ann_tx, _) = broadcast::channel(16);
        let (rb_tx, _) = broadcast::channel(16);

        let provider = MockBlockProvider {
            blocks: vec![
                (10, [0x01; 32], make_block_cbor(&[0xAA])),
                (20, [0x02; 32], make_block_cbor(&[0xBB])),
            ],
        };

        let handle = spawn_server(channel, provider, ann_tx.subscribe(), rb_tx.subscribe());

        // First point doesn't exist, second does.
        send_msg(
            &ingress_tx,
            &ChainSyncMessage::MsgFindIntersect(vec![
                Point::Specific(99, [0xFF; 32]),
                Point::Specific(10, [0x01; 32]),
            ]),
        )
        .await;

        let msg = recv_msg(&mut egress_rx).await;
        match msg {
            ChainSyncMessage::MsgIntersectFound { point, .. } => {
                assert_eq!(point, Point::Specific(10, [0x01; 32]));
            }
            other => panic!("expected MsgIntersectFound, got {other:?}"),
        }

        send_msg(&ingress_tx, &ChainSyncMessage::MsgDone).await;
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn find_intersect_empty_chain() {
        // With an empty chain, only Origin should match.
        let (channel, mut egress_rx, ingress_tx) = make_test_channel();
        let (ann_tx, _) = broadcast::channel(16);
        let (rb_tx, _) = broadcast::channel(16);

        let provider = MockBlockProvider { blocks: vec![] };

        let handle = spawn_server(channel, provider, ann_tx.subscribe(), rb_tx.subscribe());

        // Specific point on empty chain → not found.
        send_msg(
            &ingress_tx,
            &ChainSyncMessage::MsgFindIntersect(vec![Point::Specific(10, [0x01; 32])]),
        )
        .await;

        let msg = recv_msg(&mut egress_rx).await;
        assert!(
            matches!(msg, ChainSyncMessage::MsgIntersectNotFound { .. }),
            "specific point on empty chain should not be found"
        );

        send_msg(&ingress_tx, &ChainSyncMessage::MsgDone).await;
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn find_intersect_sets_cursor_for_subsequent_request_next() {
        // After FindIntersect, RequestNext should serve the block AFTER the
        // intersection point (not the intersection block itself).
        let (channel, mut egress_rx, ingress_tx) = make_test_channel();
        let (ann_tx, _) = broadcast::channel(16);
        let (rb_tx, _) = broadcast::channel(16);

        let block_b_cbor = make_block_cbor(&[0xBB, 0xCC, 0xDD]);
        let provider = MockBlockProvider {
            blocks: vec![
                (10, [0x01; 32], vec![0xAA]),
                (20, [0x02; 32], block_b_cbor.clone()),
                (30, [0x03; 32], make_block_cbor(&[0xEE])),
            ],
        };

        let handle = spawn_server(channel, provider, ann_tx.subscribe(), rb_tx.subscribe());

        // Set intersection at slot 10.
        send_msg(
            &ingress_tx,
            &ChainSyncMessage::MsgFindIntersect(vec![Point::Specific(10, [0x01; 32])]),
        )
        .await;
        let _ = recv_msg(&mut egress_rx).await; // MsgIntersectFound

        // RequestNext should serve block at slot 20 (next after cursor slot 10).
        send_msg(&ingress_tx, &ChainSyncMessage::MsgRequestNext).await;
        let msg = recv_msg(&mut egress_rx).await;
        match msg {
            ChainSyncMessage::MsgRollForward { header, .. } => {
                // LocalChainSync wraps blocks in Serialised encoding: tag(24)(bytes(cbor)).
                assert_eq!(header, wrap_serialised(&block_b_cbor));
            }
            other => panic!("expected MsgRollForward, got {other:?}"),
        }

        send_msg(&ingress_tx, &ChainSyncMessage::MsgDone).await;
        handle.await.unwrap().unwrap();
    }

    // ─── RequestNext tests ───────────────────────────────────────────────────

    #[tokio::test]
    async fn request_next_serves_sequential_blocks() {
        // After intersecting at Origin, RequestNext should serve blocks in order.
        let (channel, mut egress_rx, ingress_tx) = make_test_channel();
        let (ann_tx, _) = broadcast::channel(16);
        let (rb_tx, _) = broadcast::channel(16);

        let provider = MockBlockProvider {
            blocks: vec![
                (10, [0x01; 32], make_block_cbor(&[0x10])),
                (20, [0x02; 32], make_block_cbor(&[0x20])),
                (30, [0x03; 32], make_block_cbor(&[0x30])),
            ],
        };

        let handle = spawn_server(channel, provider, ann_tx.subscribe(), rb_tx.subscribe());

        // Intersect at Origin.
        send_msg(
            &ingress_tx,
            &ChainSyncMessage::MsgFindIntersect(vec![Point::Origin]),
        )
        .await;
        let _ = recv_msg(&mut egress_rx).await;

        // Serve all 3 blocks in order.
        let expected_cbor = [
            make_block_cbor(&[0x10]),
            make_block_cbor(&[0x20]),
            make_block_cbor(&[0x30]),
        ];
        for expected in &expected_cbor {
            send_msg(&ingress_tx, &ChainSyncMessage::MsgRequestNext).await;
            let msg = recv_msg(&mut egress_rx).await;
            match msg {
                ChainSyncMessage::MsgRollForward { header, .. } => {
                    assert_eq!(header, wrap_serialised(expected));
                }
                other => panic!("expected MsgRollForward, got {other:?}"),
            }
        }

        send_msg(&ingress_tx, &ChainSyncMessage::MsgDone).await;
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn request_next_sends_full_block_not_header() {
        // LocalChainSync (N2C) must send full block CBOR, not just the header.
        // This is the key difference from N2N ChainSync.
        let (channel, mut egress_rx, ingress_tx) = make_test_channel();
        let (ann_tx, _) = broadcast::channel(16);
        let (rb_tx, _) = broadcast::channel(16);

        let full_block = make_block_cbor(&[0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE]);
        let provider = MockBlockProvider {
            blocks: vec![(10, [0x01; 32], full_block.clone())],
        };

        let handle = spawn_server(channel, provider, ann_tx.subscribe(), rb_tx.subscribe());

        send_msg(
            &ingress_tx,
            &ChainSyncMessage::MsgFindIntersect(vec![Point::Origin]),
        )
        .await;
        let _ = recv_msg(&mut egress_rx).await;

        send_msg(&ingress_tx, &ChainSyncMessage::MsgRequestNext).await;
        let msg = recv_msg(&mut egress_rx).await;
        match msg {
            ChainSyncMessage::MsgRollForward { header, .. } => {
                assert_eq!(
                    header,
                    wrap_serialised(&full_block),
                    "LocalChainSync must send Serialised-wrapped full block CBOR"
                );
            }
            other => panic!("expected MsgRollForward, got {other:?}"),
        }

        send_msg(&ingress_tx, &ChainSyncMessage::MsgDone).await;
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn request_next_tip_info_matches_provider() {
        // The tip info in MsgRollForward must match the provider's current tip.
        let (channel, mut egress_rx, ingress_tx) = make_test_channel();
        let (ann_tx, _) = broadcast::channel(16);
        let (rb_tx, _) = broadcast::channel(16);

        let provider = MockBlockProvider {
            blocks: vec![
                (10, [0x01; 32], make_block_cbor(&[0xAA])),
                (20, [0x02; 32], make_block_cbor(&[0xBB])),
            ],
        };

        let handle = spawn_server(channel, provider, ann_tx.subscribe(), rb_tx.subscribe());

        send_msg(
            &ingress_tx,
            &ChainSyncMessage::MsgFindIntersect(vec![Point::Origin]),
        )
        .await;
        let _ = recv_msg(&mut egress_rx).await;

        send_msg(&ingress_tx, &ChainSyncMessage::MsgRequestNext).await;
        let msg = recv_msg(&mut egress_rx).await;
        match msg {
            ChainSyncMessage::MsgRollForward {
                tip_slot,
                tip_hash,
                tip_block_number,
                ..
            } => {
                assert_eq!(tip_slot, 20);
                assert_eq!(tip_hash, [0x02; 32]);
                assert_eq!(tip_block_number, 2);
            }
            other => panic!("expected MsgRollForward, got {other:?}"),
        }

        send_msg(&ingress_tx, &ChainSyncMessage::MsgDone).await;
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn request_next_at_tip_sends_await_reply() {
        // When there are no more blocks to serve, the server sends MsgAwaitReply,
        // then waits for a block announcement or rollback.
        let (channel, mut egress_rx, ingress_tx) = make_test_channel();
        let (ann_tx, _) = broadcast::channel(16);
        let (rb_tx, _) = broadcast::channel(16);

        let provider = MockBlockProvider {
            blocks: vec![(10, [0x01; 32], make_block_cbor(&[0xAA]))],
        };

        let handle = spawn_server(channel, provider, ann_tx.subscribe(), rb_tx.subscribe());

        send_msg(
            &ingress_tx,
            &ChainSyncMessage::MsgFindIntersect(vec![Point::Origin]),
        )
        .await;
        let _ = recv_msg(&mut egress_rx).await;

        // Serve the only block.
        send_msg(&ingress_tx, &ChainSyncMessage::MsgRequestNext).await;
        let _ = recv_msg(&mut egress_rx).await; // MsgRollForward

        // Next request — at tip, should get MsgAwaitReply.
        send_msg(&ingress_tx, &ChainSyncMessage::MsgRequestNext).await;
        let msg = recv_msg(&mut egress_rx).await;
        assert!(
            matches!(msg, ChainSyncMessage::MsgAwaitReply),
            "expected MsgAwaitReply at tip, got {msg:?}"
        );

        // Send announcement to unblock the server.
        ann_tx
            .send(BlockAnnouncement {
                slot: 20,
                hash: [0x02; 32],
                block_number: 2,
            })
            .unwrap();

        // The server won't be able to serve the block (provider doesn't have it),
        // but the select loop will complete. Drop channels to clean up.
        drop(ingress_tx);
        let _ = handle.await;
    }

    #[tokio::test]
    async fn request_next_announcement_serves_new_block() {
        // When waiting at tip and a block announcement arrives for a block
        // that the provider has, the server should send MsgRollForward.

        // Use a mutable provider so we can add a block after the server starts.
        type BlockList = Vec<(u64, [u8; 32], Vec<u8>)>;
        struct ArcBlockProvider {
            blocks: Arc<std::sync::Mutex<BlockList>>,
        }
        impl BlockProvider for ArcBlockProvider {
            fn get_block(&self, hash: &[u8; 32]) -> Option<Vec<u8>> {
                self.blocks
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|(_, h, _)| h == hash)
                    .map(|(_, _, c)| c.clone())
            }
            fn has_block(&self, hash: &[u8; 32]) -> bool {
                self.blocks
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|(_, h, _)| h == hash)
            }
            fn get_tip(&self) -> TipInfo {
                let b = self.blocks.lock().unwrap();
                b.last()
                    .map(|(s, h, _)| TipInfo {
                        slot: *s,
                        hash: *h,
                        block_number: b.len() as u64,
                    })
                    .unwrap_or(TipInfo {
                        slot: 0,
                        hash: [0; 32],
                        block_number: 0,
                    })
            }
            fn get_next_block_after_slot(
                &self,
                after_slot: u64,
            ) -> Option<(u64, [u8; 32], Vec<u8>)> {
                self.blocks
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|(s, _, _)| *s > after_slot)
                    .cloned()
            }
        }

        let blocks = Arc::new(std::sync::Mutex::new(vec![(
            10u64,
            [0x01u8; 32],
            make_block_cbor(&[0xAA]),
        )]));
        let blocks_ref = blocks.clone();
        let provider = ArcBlockProvider { blocks };

        let (ann_tx, _) = broadcast::channel(16);
        let (rb_tx, _) = broadcast::channel(16);

        let (egress_tx, mut egress_rx) = mpsc::channel(64);
        let (ingress_tx, ingress_rx) = mpsc::channel(64);
        let mut channel = crate::mux::channel::MuxChannel::new(
            5,
            crate::mux::Direction::ResponderDir,
            egress_tx,
            ingress_rx,
            1_000_000,
            Arc::new(AtomicUsize::new(0)),
        );

        let ann_rx = ann_tx.subscribe();
        let rb_rx = rb_tx.subscribe();

        let handle = tokio::spawn(async move {
            let mut server = LocalChainSyncServer::new();
            server.run(&mut channel, &provider, ann_rx, rb_rx).await
        });

        // Intersect at Origin.
        send_msg(
            &ingress_tx,
            &ChainSyncMessage::MsgFindIntersect(vec![Point::Origin]),
        )
        .await;
        let _ = recv_msg(&mut egress_rx).await;

        // Serve the initial block.
        send_msg(&ingress_tx, &ChainSyncMessage::MsgRequestNext).await;
        let _ = recv_msg(&mut egress_rx).await;

        // Request next at tip — triggers MsgAwaitReply.
        send_msg(&ingress_tx, &ChainSyncMessage::MsgRequestNext).await;
        let msg = recv_msg(&mut egress_rx).await;
        assert!(matches!(msg, ChainSyncMessage::MsgAwaitReply));

        // Add a new block and announce it.
        let new_block_cbor = make_block_cbor(&[0xBB, 0xCC]);
        blocks_ref
            .lock()
            .unwrap()
            .push((20, [0x02; 32], new_block_cbor.clone()));
        ann_tx
            .send(BlockAnnouncement {
                slot: 20,
                hash: [0x02; 32],
                block_number: 2,
            })
            .unwrap();

        // Server should respond with MsgRollForward for the new block.
        let msg = recv_msg(&mut egress_rx).await;
        match msg {
            ChainSyncMessage::MsgRollForward { header, .. } => {
                assert_eq!(header, wrap_serialised(&new_block_cbor));
            }
            other => panic!("expected MsgRollForward after announcement, got {other:?}"),
        }

        send_msg(&ingress_tx, &ChainSyncMessage::MsgDone).await;
        handle.await.unwrap().unwrap();
    }

    // ─── Rollback tests ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn rollback_before_cursor_sends_roll_backward() {
        // When a rollback occurs to a point before the cursor, the server must
        // send MsgRollBackward and rewind its cursor.
        let (channel, mut egress_rx, ingress_tx) = make_test_channel();
        let (ann_tx, _) = broadcast::channel(16);
        let (rb_tx, _) = broadcast::channel(16);

        let provider = MockBlockProvider {
            blocks: vec![
                (10, [0x01; 32], make_block_cbor(&[0x10])),
                (20, [0x02; 32], make_block_cbor(&[0x20])),
                (30, [0x03; 32], make_block_cbor(&[0x30])),
            ],
        };

        let handle = spawn_server(channel, provider, ann_tx.subscribe(), rb_tx.subscribe());

        // Intersect at Origin and serve all 3 blocks.
        send_msg(
            &ingress_tx,
            &ChainSyncMessage::MsgFindIntersect(vec![Point::Origin]),
        )
        .await;
        let _ = recv_msg(&mut egress_rx).await;

        for _ in 0..3 {
            send_msg(&ingress_tx, &ChainSyncMessage::MsgRequestNext).await;
            let msg = recv_msg(&mut egress_rx).await;
            assert!(matches!(msg, ChainSyncMessage::MsgRollForward { .. }));
        }

        // Cursor is now at slot 30. Rollback to slot 10.
        rb_tx
            .send(RollbackAnnouncement {
                slot: 10,
                hash: [0x01; 32],
            })
            .unwrap();

        // Next RequestNext should yield MsgRollBackward.
        send_msg(&ingress_tx, &ChainSyncMessage::MsgRequestNext).await;
        let msg = recv_msg(&mut egress_rx).await;
        match msg {
            ChainSyncMessage::MsgRollBackward { point, .. } => {
                assert_eq!(point, Point::Specific(10, [0x01; 32]));
            }
            other => panic!("expected MsgRollBackward, got {other:?}"),
        }

        send_msg(&ingress_tx, &ChainSyncMessage::MsgDone).await;
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn rollback_to_origin() {
        // Rollback to slot 0 with zero hash should produce MsgRollBackward(Origin).
        let (channel, mut egress_rx, ingress_tx) = make_test_channel();
        let (ann_tx, _) = broadcast::channel(16);
        let (rb_tx, _) = broadcast::channel(16);

        let provider = MockBlockProvider {
            blocks: vec![(10, [0x01; 32], make_block_cbor(&[0xAA]))],
        };

        let handle = spawn_server(channel, provider, ann_tx.subscribe(), rb_tx.subscribe());

        send_msg(
            &ingress_tx,
            &ChainSyncMessage::MsgFindIntersect(vec![Point::Origin]),
        )
        .await;
        let _ = recv_msg(&mut egress_rx).await;

        // Serve the block (cursor at slot 10).
        send_msg(&ingress_tx, &ChainSyncMessage::MsgRequestNext).await;
        let _ = recv_msg(&mut egress_rx).await;

        // Rollback to origin.
        rb_tx
            .send(RollbackAnnouncement {
                slot: 0,
                hash: [0u8; 32],
            })
            .unwrap();

        send_msg(&ingress_tx, &ChainSyncMessage::MsgRequestNext).await;
        let msg = recv_msg(&mut egress_rx).await;
        match msg {
            ChainSyncMessage::MsgRollBackward { point, .. } => {
                assert_eq!(
                    point,
                    Point::Origin,
                    "rollback to slot 0 + zero hash = Origin"
                );
            }
            other => panic!("expected MsgRollBackward(Origin), got {other:?}"),
        }

        send_msg(&ingress_tx, &ChainSyncMessage::MsgDone).await;
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn rollback_behind_cursor_no_rollback_sent() {
        // If the rollback point is ahead of the cursor, no MsgRollBackward should
        // be sent — the client hasn't seen the rolled-back blocks.
        let (channel, mut egress_rx, ingress_tx) = make_test_channel();
        let (ann_tx, _) = broadcast::channel(16);
        let (rb_tx, _) = broadcast::channel(16);

        let provider = MockBlockProvider {
            blocks: vec![
                (10, [0x01; 32], make_block_cbor(&[0x10])),
                (20, [0x02; 32], make_block_cbor(&[0x20])),
                (30, [0x03; 32], make_block_cbor(&[0x30])),
            ],
        };

        let handle = spawn_server(channel, provider, ann_tx.subscribe(), rb_tx.subscribe());

        send_msg(
            &ingress_tx,
            &ChainSyncMessage::MsgFindIntersect(vec![Point::Origin]),
        )
        .await;
        let _ = recv_msg(&mut egress_rx).await;

        // Only serve 1 block (cursor at slot 10).
        send_msg(&ingress_tx, &ChainSyncMessage::MsgRequestNext).await;
        let _ = recv_msg(&mut egress_rx).await;

        // Rollback to slot 20 (ahead of cursor at slot 10).
        rb_tx
            .send(RollbackAnnouncement {
                slot: 20,
                hash: [0x02; 32],
            })
            .unwrap();

        // Should get MsgRollForward for block at slot 20, not MsgRollBackward.
        send_msg(&ingress_tx, &ChainSyncMessage::MsgRequestNext).await;
        let msg = recv_msg(&mut egress_rx).await;
        assert!(
            matches!(msg, ChainSyncMessage::MsgRollForward { .. }),
            "rollback ahead of cursor should not trigger MsgRollBackward, got {msg:?}"
        );

        send_msg(&ingress_tx, &ChainSyncMessage::MsgDone).await;
        handle.await.unwrap().unwrap();
    }

    // ─── MsgDone tests ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn msg_done_terminates_server_cleanly() {
        let (channel, _egress_rx, ingress_tx) = make_test_channel();
        let (ann_tx, _) = broadcast::channel(16);
        let (rb_tx, _) = broadcast::channel(16);

        let provider = MockBlockProvider { blocks: vec![] };
        let handle = spawn_server(channel, provider, ann_tx.subscribe(), rb_tx.subscribe());

        send_msg(&ingress_tx, &ChainSyncMessage::MsgDone).await;
        let result = handle.await.unwrap();
        assert!(result.is_ok(), "MsgDone should terminate server cleanly");
    }

    // ─── drain_rollback tests ────────────────────────────────────────────────

    #[test]
    fn drain_rollback_returns_latest() {
        // drain_rollback should return the most recent rollback announcement
        // when multiple are buffered.
        let (tx, mut rx) = broadcast::channel(16);
        tx.send(RollbackAnnouncement {
            slot: 10,
            hash: [0x01; 32],
        })
        .unwrap();
        tx.send(RollbackAnnouncement {
            slot: 5,
            hash: [0x05; 32],
        })
        .unwrap();

        let result = LocalChainSyncServer::drain_rollback(&mut rx);
        assert!(result.is_some());
        let rb = result.unwrap();
        assert_eq!(rb.slot, 5, "should return the last (most recent) rollback");
        assert_eq!(rb.hash, [0x05; 32]);
    }

    #[test]
    fn drain_rollback_empty_returns_none() {
        let (_tx, mut rx) = broadcast::channel::<RollbackAnnouncement>(16);
        let result = LocalChainSyncServer::drain_rollback(&mut rx);
        assert!(result.is_none());
    }

    // ─── Default impl test ───────────────────────────────────────────────────

    #[test]
    fn default_creates_zero_cursor() {
        let server = LocalChainSyncServer::default();
        assert_eq!(server.cursor_slot, 0);
        assert_eq!(server.cursor_hash, [0; 32]);
    }
}
