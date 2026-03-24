//! ChainSync server — serves headers to downstream peers.
//!
//! Maintains a per-peer cursor tracking the last block served. When the peer
//! sends `MsgRequestNext`, serves the next header from ChainDB. At the tip,
//! waits for a block announcement via a broadcast channel before responding.
//!
//! ## Block Announcement
//! When a new block is received (either from upstream sync or forged locally),
//! a broadcast is sent. The server listens for this broadcast to unblock peers
//! waiting at the tip (in `StMustReply` state).

use std::time::Duration;

use tokio::sync::broadcast;

use crate::codec::Point;
use crate::error::ProtocolError;
use crate::mux::channel::MuxChannel;
use crate::BlockProvider;

use super::{decode_message, encode_message, ChainSyncMessage};

/// Block announcement sent via broadcast channel when a new block arrives.
#[derive(Debug, Clone)]
pub struct BlockAnnouncement {
    /// Slot of the announced block.
    pub slot: u64,
    /// Hash of the announced block.
    pub hash: [u8; 32],
    /// Block number of the announced block.
    pub block_number: u64,
}

/// ChainSync server that serves headers to a single downstream peer.
pub struct ChainSyncServer {
    /// Current cursor: the last point served to this peer.
    cursor_slot: u64,
    cursor_hash: [u8; 32],
    /// Whether the cursor has been initialized (via intersection or genesis).
    cursor_initialized: bool,
}

impl ChainSyncServer {
    /// Create a new server with no cursor (must find intersection first).
    pub fn new() -> Self {
        Self {
            cursor_slot: 0,
            cursor_hash: [0; 32],
            cursor_initialized: false,
        }
    }

    /// Run the ChainSync server loop.
    ///
    /// Handles `MsgFindIntersect`, `MsgRequestNext`, and `MsgDone` from the client.
    /// Uses `block_provider` to look up blocks and `announcement_rx` to wait for
    /// new blocks at the tip.
    pub async fn run<B: BlockProvider>(
        &mut self,
        channel: &mut MuxChannel,
        block_provider: &B,
        mut announcement_rx: broadcast::Receiver<BlockAnnouncement>,
    ) -> Result<(), ProtocolError> {
        loop {
            let msg_bytes = channel.recv().await.map_err(ProtocolError::from)?;
            let msg = decode_message(&msg_bytes).map_err(|e| ProtocolError::CborDecode {
                protocol: "ChainSync",
                reason: e,
            })?;

            match msg {
                ChainSyncMessage::MsgFindIntersect(points) => {
                    self.handle_find_intersect(channel, block_provider, &points)
                        .await?;
                }
                ChainSyncMessage::MsgRequestNext => {
                    self.handle_request_next(channel, block_provider, &mut announcement_rx)
                        .await?;
                }
                ChainSyncMessage::MsgDone => {
                    tracing::debug!("chainsync server: client sent MsgDone");
                    return Ok(());
                }
                other => {
                    return Err(ProtocolError::AgencyViolation {
                        protocol: "ChainSync",
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

    /// Handle MsgFindIntersect: walk the client's points to find the best match.
    async fn handle_find_intersect<B: BlockProvider>(
        &mut self,
        channel: &mut MuxChannel,
        block_provider: &B,
        points: &[Point],
    ) -> Result<(), ProtocolError> {
        let tip = block_provider.get_tip();

        // Walk points in order (most recent first), find the first one we have
        for point in points {
            match point {
                Point::Origin => {
                    // We always have genesis
                    self.cursor_slot = 0;
                    self.cursor_hash = [0; 32];
                    self.cursor_initialized = true;

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
                        self.cursor_initialized = true;

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

        // No intersection found
        let response = encode_message(&ChainSyncMessage::MsgIntersectNotFound {
            tip_slot: tip.slot,
            tip_hash: tip.hash,
            tip_block_number: tip.block_number,
        });
        channel.send(response).await.map_err(ProtocolError::from)?;
        Ok(())
    }

    /// Handle MsgRequestNext: serve the next header or wait for announcement.
    async fn handle_request_next<B: BlockProvider>(
        &mut self,
        channel: &mut MuxChannel,
        block_provider: &B,
        announcement_rx: &mut broadcast::Receiver<BlockAnnouncement>,
    ) -> Result<(), ProtocolError> {
        // Try to find the next block after our cursor
        if let Some((slot, hash, block_cbor)) =
            block_provider.get_next_block_after_slot(self.cursor_slot)
        {
            // We have the next block — extract header and send MsgRollForward
            let tip = block_provider.get_tip();

            // For N2N ChainSync, we send just the header.
            // For simplicity, we send the raw block CBOR as the "header" —
            // the real header extraction happens in the node integration layer.
            let response = encode_message(&ChainSyncMessage::MsgRollForward {
                header: block_cbor,
                tip_slot: tip.slot,
                tip_hash: tip.hash,
                tip_block_number: tip.block_number,
            });
            channel.send(response).await.map_err(ProtocolError::from)?;

            // Advance cursor
            self.cursor_slot = slot;
            self.cursor_hash = hash;
            return Ok(());
        }

        // We're at the tip — send MsgAwaitReply and wait for announcement
        let await_msg = encode_message(&ChainSyncMessage::MsgAwaitReply);
        channel.send(await_msg).await.map_err(ProtocolError::from)?;

        // Wait for a block announcement with a randomized timeout (matching Haskell behavior).
        // Haskell uses a timeout range of 135-911 seconds.
        let timeout = Duration::from_secs(135);

        tokio::select! {
            announcement = announcement_rx.recv() => {
                match announcement {
                    Ok(ann) => {
                        // New block announced — fetch it and send MsgRollForward
                        if let Some(block_cbor) = block_provider.get_block(&ann.hash) {
                            let tip = block_provider.get_tip();
                            let response = encode_message(&ChainSyncMessage::MsgRollForward {
                                header: block_cbor,
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
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(n, "chainsync server: announcement channel lagged");
                        // Try to catch up from current tip
                        let tip = block_provider.get_tip();
                        if let Some(block_cbor) = block_provider.get_block(&tip.hash) {
                            let response = encode_message(&ChainSyncMessage::MsgRollForward {
                                header: block_cbor,
                                tip_slot: tip.slot,
                                tip_hash: tip.hash,
                                tip_block_number: tip.block_number,
                            });
                            channel.send(response).await.map_err(ProtocolError::from)?;
                            self.cursor_slot = tip.slot;
                            self.cursor_hash = tip.hash;
                        }
                        Ok(())
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        // Broadcast sender dropped — node is shutting down
                        Ok(())
                    }
                }
            }
            _ = tokio::time::sleep(timeout) => {
                // Timeout — check if there's a new block now
                if let Some((slot, hash, block_cbor)) =
                    block_provider.get_next_block_after_slot(self.cursor_slot)
                {
                    let tip = block_provider.get_tip();
                    let response = encode_message(&ChainSyncMessage::MsgRollForward {
                        header: block_cbor,
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
    }
}

impl Default for ChainSyncServer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TipInfo;
    use bytes::Bytes;
    use tokio::sync::mpsc;

    /// Mock block provider for testing.
    struct MockBlockProvider {
        blocks: Vec<(u64, [u8; 32], Vec<u8>)>, // (slot, hash, cbor)
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
            if let Some((slot, hash, _)) = self.blocks.last() {
                TipInfo {
                    slot: *slot,
                    hash: *hash,
                    block_number: self.blocks.len() as u64,
                }
            } else {
                TipInfo {
                    slot: 0,
                    hash: [0; 32],
                    block_number: 0,
                }
            }
        }

        fn get_next_block_after_slot(&self, after_slot: u64) -> Option<(u64, [u8; 32], Vec<u8>)> {
            self.blocks
                .iter()
                .find(|(s, _, _)| *s > after_slot)
                .cloned()
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
            2,
            crate::mux::Direction::ResponderDir,
            egress_tx,
            ingress_rx,
            1_000_000,
        );
        (channel, egress_rx, ingress_tx)
    }

    #[tokio::test]
    async fn find_intersect_with_known_block() {
        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();
        let provider = MockBlockProvider {
            blocks: vec![(100, [0xAA; 32], vec![0x01, 0x02])],
        };
        let (ann_tx, _) = broadcast::channel(16);

        let mut server = ChainSyncServer::new();

        let handle = tokio::spawn(async move {
            server
                .run(&mut channel, &provider, ann_tx.subscribe())
                .await
        });

        // Send MsgFindIntersect
        let find = encode_message(&ChainSyncMessage::MsgFindIntersect(vec![Point::Specific(
            100, [0xAA; 32],
        )]));
        ingress_tx.send(Bytes::from(find)).await.unwrap();

        // Read MsgIntersectFound
        let (_, _, resp) = egress_rx.recv().await.unwrap();
        let msg = decode_message(&resp).unwrap();
        assert!(matches!(msg, ChainSyncMessage::MsgIntersectFound { .. }));

        // Send MsgDone to clean up
        let done = encode_message(&ChainSyncMessage::MsgDone);
        ingress_tx.send(Bytes::from(done)).await.unwrap();

        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn request_next_serves_block() {
        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();
        let provider = MockBlockProvider {
            blocks: vec![(10, [0x01; 32], vec![0xAA]), (20, [0x02; 32], vec![0xBB])],
        };
        let (ann_tx, _) = broadcast::channel(16);

        let mut server = ChainSyncServer::new();

        let handle = tokio::spawn(async move {
            server
                .run(&mut channel, &provider, ann_tx.subscribe())
                .await
        });

        // Find intersection at origin (cursor_slot = 0)
        let find = encode_message(&ChainSyncMessage::MsgFindIntersect(vec![Point::Origin]));
        ingress_tx.send(Bytes::from(find)).await.unwrap();
        let _ = egress_rx.recv().await.unwrap(); // MsgIntersectFound

        // Request next — should get block at slot 10
        let req = encode_message(&ChainSyncMessage::MsgRequestNext);
        ingress_tx.send(Bytes::from(req)).await.unwrap();

        let (_, _, resp) = egress_rx.recv().await.unwrap();
        let msg = decode_message(&resp).unwrap();
        if let ChainSyncMessage::MsgRollForward { header, .. } = msg {
            assert_eq!(header, vec![0xAA]);
        } else {
            panic!("expected MsgRollForward, got {msg:?}");
        }

        // Send MsgDone
        let done = encode_message(&ChainSyncMessage::MsgDone);
        ingress_tx.send(Bytes::from(done)).await.unwrap();
        handle.await.unwrap().unwrap();
    }
}
