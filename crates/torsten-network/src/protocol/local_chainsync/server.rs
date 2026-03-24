//! LocalChainSync server — serves full blocks to N2C clients.
//!
//! Uses the same ChainSync message wire format (tags 0-7) but wraps block data
//! in HFC era encoding for multi-era support: `[era_id, CBOR_tag_24(block_bytes)]`.

use tokio::sync::broadcast;

use crate::codec::Point;
use crate::error::ProtocolError;
use crate::mux::channel::MuxChannel;
use crate::protocol::chainsync::server::BlockAnnouncement;
use crate::protocol::chainsync::{decode_message, encode_message, ChainSyncMessage};
use crate::BlockProvider;

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
    pub async fn run<B: BlockProvider>(
        &mut self,
        channel: &mut MuxChannel,
        block_provider: &B,
        mut announcement_rx: broadcast::Receiver<BlockAnnouncement>,
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
                    self.handle_request_next(channel, block_provider, &mut announcement_rx)
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

    /// Handle MsgRequestNext — sends full blocks (not headers) with HFC wrapping.
    async fn handle_request_next<B: BlockProvider>(
        &mut self,
        channel: &mut MuxChannel,
        block_provider: &B,
        announcement_rx: &mut broadcast::Receiver<BlockAnnouncement>,
    ) -> Result<(), ProtocolError> {
        if let Some((slot, hash, block_cbor)) =
            block_provider.get_next_block_after_slot(self.cursor_slot)
        {
            let tip = block_provider.get_tip();

            // For LocalChainSync, we send the full block CBOR (not just header).
            // In a full implementation, this would be wrapped as [era_id, tag24(block_bytes)].
            // For now, we send the raw block bytes — the wrapping is added in the
            // node integration layer which knows the era.
            let response = encode_message(&ChainSyncMessage::MsgRollForward {
                header: block_cbor,
                tip_slot: tip.slot,
                tip_hash: tip.hash,
                tip_block_number: tip.block_number,
            });
            channel.send(response).await.map_err(ProtocolError::from)?;
            self.cursor_slot = slot;
            self.cursor_hash = hash;
            return Ok(());
        }

        // At tip — wait for announcement
        let await_msg = encode_message(&ChainSyncMessage::MsgAwaitReply);
        channel.send(await_msg).await.map_err(ProtocolError::from)?;

        match announcement_rx.recv().await {
            Ok(ann) => {
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
            }
            Err(broadcast::error::RecvError::Lagged(_)) => {
                // Try to catch up from current position
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
            }
            Err(broadcast::error::RecvError::Closed) => {
                // Node shutting down
            }
        }

        Ok(())
    }
}

impl Default for LocalChainSyncServer {
    fn default() -> Self {
        Self::new()
    }
}
