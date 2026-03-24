//! BlockFetch server — serves block ranges to requesting peers.
//!
//! Handles `MsgRequestRange(from, to)` by looking up blocks via [`BlockProvider`]
//! and streaming them as `MsgStartBatch` → `MsgBlock` × N → `MsgBatchDone`.
//! Sends `MsgNoBlocks` if the requested range is unavailable.
//!
//! ## Range validation
//! - Maximum slot span: 2160 (one Cardano epoch)
//! - Maximum blocks per batch: 100 (prevents memory exhaustion)

use crate::codec::Point;
use crate::error::ProtocolError;
use crate::mux::channel::MuxChannel;
use crate::BlockProvider;

use super::{decode_message, encode_message, BlockFetchMessage};

/// Maximum slot span for a single range request.
pub const MAX_RANGE_SLOTS: u64 = 2160;
/// Maximum number of blocks per batch response.
pub const MAX_BLOCKS_PER_BATCH: usize = 100;

/// BlockFetch server that serves block ranges to peers.
pub struct BlockFetchServer;

impl BlockFetchServer {
    /// Run the BlockFetch server loop.
    ///
    /// Handles `MsgRequestRange` and `MsgClientDone`.
    pub async fn run<B: BlockProvider>(
        channel: &mut MuxChannel,
        block_provider: &B,
    ) -> Result<(), ProtocolError> {
        loop {
            let msg_bytes = channel.recv().await.map_err(ProtocolError::from)?;
            let msg = decode_message(&msg_bytes).map_err(|e| ProtocolError::CborDecode {
                protocol: "BlockFetch",
                reason: e,
            })?;

            match msg {
                BlockFetchMessage::MsgRequestRange { from, to } => {
                    Self::handle_request_range(channel, block_provider, &from, &to).await?;
                }
                BlockFetchMessage::MsgClientDone => {
                    tracing::debug!("blockfetch server: client sent MsgClientDone");
                    return Ok(());
                }
                other => {
                    return Err(ProtocolError::AgencyViolation {
                        protocol: "BlockFetch",
                        state: "BFIdle".to_string(),
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

    /// Handle a range request: validate range, look up blocks, stream response.
    async fn handle_request_range<B: BlockProvider>(
        channel: &mut MuxChannel,
        block_provider: &B,
        from: &Point,
        to: &Point,
    ) -> Result<(), ProtocolError> {
        // Validate range — extract from_slot for iteration
        let from_slot = match from {
            Point::Origin => 0,
            Point::Specific(slot, _) => *slot,
        };
        let to_slot = match to {
            Point::Origin => 0,
            Point::Specific(slot, _) => *slot,
        };

        // Check range validity
        if to_slot < from_slot || (to_slot - from_slot) > MAX_RANGE_SLOTS {
            let no_blocks = encode_message(&BlockFetchMessage::MsgNoBlocks);
            channel.send(no_blocks).await.map_err(ProtocolError::from)?;
            return Ok(());
        }

        // Verify we have the starting block
        let have_from = match from {
            Point::Origin => true,
            Point::Specific(_, hash) => block_provider.has_block(hash),
        };

        if !have_from {
            let no_blocks = encode_message(&BlockFetchMessage::MsgNoBlocks);
            channel.send(no_blocks).await.map_err(ProtocolError::from)?;
            return Ok(());
        }

        // Collect blocks in the range
        let mut blocks: Vec<Vec<u8>> = Vec::new();
        let mut current_slot = from_slot;

        while current_slot <= to_slot && blocks.len() < MAX_BLOCKS_PER_BATCH {
            if let Some((slot, _hash, cbor)) =
                block_provider.get_next_block_after_slot(current_slot.saturating_sub(1))
            {
                if slot > to_slot {
                    break;
                }
                blocks.push(cbor);
                current_slot = slot + 1;
            } else {
                break;
            }
        }

        if blocks.is_empty() {
            let no_blocks = encode_message(&BlockFetchMessage::MsgNoBlocks);
            channel.send(no_blocks).await.map_err(ProtocolError::from)?;
            return Ok(());
        }

        // Stream: MsgStartBatch → MsgBlock × N → MsgBatchDone
        let start = encode_message(&BlockFetchMessage::MsgStartBatch);
        channel.send(start).await.map_err(ProtocolError::from)?;

        for block_cbor in &blocks {
            let block_msg = encode_message(&BlockFetchMessage::MsgBlock(block_cbor.clone()));
            channel.send(block_msg).await.map_err(ProtocolError::from)?;
        }

        let done = encode_message(&BlockFetchMessage::MsgBatchDone);
        channel.send(done).await.map_err(ProtocolError::from)?;

        tracing::debug!(
            block_count = blocks.len(),
            from_slot,
            to_slot,
            "blockfetch server: served batch"
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TipInfo;
    use bytes::Bytes;
    use tokio::sync::mpsc;

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

    fn make_test_channel() -> (
        MuxChannel,
        mpsc::Receiver<(u16, crate::mux::Direction, Bytes)>,
        mpsc::Sender<Bytes>,
    ) {
        let (egress_tx, egress_rx) = mpsc::channel(64);
        let (ingress_tx, ingress_rx) = mpsc::channel(64);
        let channel = MuxChannel::new(
            3,
            crate::mux::Direction::ResponderDir,
            egress_tx,
            ingress_rx,
            1_000_000,
        );
        (channel, egress_rx, ingress_tx)
    }

    #[tokio::test]
    async fn serves_block_range() {
        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();
        let provider = MockBlockProvider {
            blocks: vec![
                (10, [0x01; 32], vec![0xAA]),
                (20, [0x02; 32], vec![0xBB]),
                (30, [0x03; 32], vec![0xCC]),
            ],
        };

        let handle =
            tokio::spawn(async move { BlockFetchServer::run(&mut channel, &provider).await });

        // Request range from slot 10 to slot 30
        let req = encode_message(&BlockFetchMessage::MsgRequestRange {
            from: Point::Specific(10, [0x01; 32]),
            to: Point::Specific(30, [0x03; 32]),
        });
        ingress_tx.send(Bytes::from(req)).await.unwrap();

        // Should receive: MsgStartBatch → MsgBlock × 3 → MsgBatchDone
        let (_, _, start) = egress_rx.recv().await.unwrap();
        assert!(matches!(
            decode_message(&start).unwrap(),
            BlockFetchMessage::MsgStartBatch
        ));

        for expected in [vec![0xAA], vec![0xBB], vec![0xCC]] {
            let (_, _, block) = egress_rx.recv().await.unwrap();
            if let BlockFetchMessage::MsgBlock(data) = decode_message(&block).unwrap() {
                assert_eq!(data, expected);
            } else {
                panic!("expected MsgBlock");
            }
        }

        let (_, _, done_msg) = egress_rx.recv().await.unwrap();
        assert!(matches!(
            decode_message(&done_msg).unwrap(),
            BlockFetchMessage::MsgBatchDone
        ));

        // Send MsgClientDone
        let client_done = encode_message(&BlockFetchMessage::MsgClientDone);
        ingress_tx.send(Bytes::from(client_done)).await.unwrap();
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn unknown_block_returns_no_blocks() {
        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();
        let provider = MockBlockProvider { blocks: vec![] };

        let handle =
            tokio::spawn(async move { BlockFetchServer::run(&mut channel, &provider).await });

        let req = encode_message(&BlockFetchMessage::MsgRequestRange {
            from: Point::Specific(999, [0xFF; 32]),
            to: Point::Specific(999, [0xFF; 32]),
        });
        ingress_tx.send(Bytes::from(req)).await.unwrap();

        let (_, _, resp) = egress_rx.recv().await.unwrap();
        assert!(matches!(
            decode_message(&resp).unwrap(),
            BlockFetchMessage::MsgNoBlocks
        ));

        let client_done = encode_message(&BlockFetchMessage::MsgClientDone);
        ingress_tx.send(Bytes::from(client_done)).await.unwrap();
        handle.await.unwrap().unwrap();
    }
}
