//! BlockFetch client — downloads block ranges from peers.
//!
//! Sends `MsgRequestRange(from, to)` and receives the batch response:
//! `MsgStartBatch` → `MsgBlock(data)` × N → `MsgBatchDone`, or `MsgNoBlocks`.
//!
//! Supports batch-level pipelining (multiple outstanding range requests).

use crate::codec::Point;
use crate::error::ProtocolError;
use crate::mux::channel::MuxChannel;

use super::{decode_message, encode_message, BlockFetchMessage};

/// BlockFetch client for downloading block ranges.
pub struct BlockFetchClient;

impl BlockFetchClient {
    /// Fetch a range of blocks from the remote peer.
    ///
    /// Sends `MsgRequestRange(from, to)` and streams blocks via callback.
    /// The callback receives raw block CBOR for each block in the range.
    ///
    /// Returns `Ok(block_count)` on success, or `Ok(0)` if the range is unavailable.
    pub async fn fetch_range<F>(
        channel: &mut MuxChannel,
        from: Point,
        to: Point,
        mut on_block: F,
    ) -> Result<usize, ProtocolError>
    where
        F: FnMut(Vec<u8>) -> Result<(), ProtocolError>,
    {
        // Send MsgRequestRange
        let req = encode_message(&BlockFetchMessage::MsgRequestRange {
            from: from.clone(),
            to: to.clone(),
        });
        tracing::debug!("blockfetch: sending MsgRequestRange");
        channel.send(req).await.map_err(ProtocolError::from)?;
        tracing::debug!("blockfetch: MsgRequestRange sent, waiting for response");

        // Receive MsgStartBatch or MsgNoBlocks
        let response_bytes = channel.recv().await.map_err(ProtocolError::from)?;
        tracing::debug!(
            bytes = response_bytes.len(),
            "blockfetch: received response"
        );
        let response = decode_message(&response_bytes).map_err(|e| ProtocolError::CborDecode {
            protocol: "BlockFetch",
            reason: e,
        })?;

        match response {
            BlockFetchMessage::MsgNoBlocks => {
                tracing::debug!("blockfetch: range not available (MsgNoBlocks)");
                return Ok(0);
            }
            BlockFetchMessage::MsgStartBatch => {
                tracing::debug!("blockfetch: MsgStartBatch received, streaming blocks");
            }
            other => {
                tracing::error!("blockfetch: unexpected response: {other:?}");
                return Err(ProtocolError::StateViolation {
                    protocol: "BlockFetch",
                    expected: "MsgStartBatch or MsgNoBlocks".to_string(),
                    actual: format!("{other:?}"),
                });
            }
        }

        // Receive blocks until MsgBatchDone
        let mut block_count = 0;
        loop {
            let block_bytes = channel.recv().await.map_err(ProtocolError::from)?;
            let msg = decode_message(&block_bytes).map_err(|e| ProtocolError::CborDecode {
                protocol: "BlockFetch",
                reason: e,
            })?;

            match msg {
                BlockFetchMessage::MsgBlock(data) => {
                    block_count += 1;
                    tracing::debug!(
                        block_count,
                        data_len = data.len(),
                        "blockfetch: MsgBlock received"
                    );
                    on_block(data)?;
                }
                BlockFetchMessage::MsgBatchDone => {
                    tracing::debug!(block_count, "blockfetch: batch complete");
                    return Ok(block_count);
                }
                other => {
                    return Err(ProtocolError::StateViolation {
                        protocol: "BlockFetch",
                        expected: "MsgBlock or MsgBatchDone".to_string(),
                        actual: format!("{other:?}"),
                    });
                }
            }
        }
    }

    /// Send MsgClientDone to terminate the BlockFetch protocol.
    pub async fn done(channel: &mut MuxChannel) -> Result<(), ProtocolError> {
        let msg = encode_message(&BlockFetchMessage::MsgClientDone);
        channel.send(msg).await.map_err(ProtocolError::from)
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
        let (egress_tx, egress_rx) = mpsc::channel(64);
        let (ingress_tx, ingress_rx) = mpsc::channel(64);
        let channel = MuxChannel::new(
            3, // BlockFetch protocol ID
            crate::mux::Direction::InitiatorDir,
            egress_tx,
            ingress_rx,
            1_000_000,
            std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        );
        (channel, egress_rx, ingress_tx)
    }

    #[tokio::test]
    async fn fetch_range_receives_blocks() {
        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();

        let handle = tokio::spawn(async move {
            let mut blocks = Vec::new();
            let count = BlockFetchClient::fetch_range(
                &mut channel,
                Point::Specific(10, [0x01; 32]),
                Point::Specific(20, [0x02; 32]),
                |block| {
                    blocks.push(block);
                    Ok(())
                },
            )
            .await
            .unwrap();
            (count, blocks)
        });

        // Read MsgRequestRange
        let (_, _, req_bytes) = egress_rx.recv().await.unwrap();
        let req = decode_message(&req_bytes).unwrap();
        assert!(matches!(req, BlockFetchMessage::MsgRequestRange { .. }));

        // Send MsgStartBatch → MsgBlock × 2 → MsgBatchDone
        ingress_tx
            .send(Bytes::from(encode_message(
                &BlockFetchMessage::MsgStartBatch,
            )))
            .await
            .unwrap();
        ingress_tx
            .send(Bytes::from(encode_message(&BlockFetchMessage::MsgBlock(
                vec![0xAA],
            ))))
            .await
            .unwrap();
        ingress_tx
            .send(Bytes::from(encode_message(&BlockFetchMessage::MsgBlock(
                vec![0xBB],
            ))))
            .await
            .unwrap();
        ingress_tx
            .send(Bytes::from(encode_message(
                &BlockFetchMessage::MsgBatchDone,
            )))
            .await
            .unwrap();

        let (count, blocks) = handle.await.unwrap();
        assert_eq!(count, 2);
        assert_eq!(blocks, vec![vec![0xAA], vec![0xBB]]);
    }

    #[tokio::test]
    async fn fetch_range_no_blocks() {
        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();

        let handle = tokio::spawn(async move {
            BlockFetchClient::fetch_range(&mut channel, Point::Origin, Point::Origin, |_| Ok(()))
                .await
        });

        // Read MsgRequestRange
        let _ = egress_rx.recv().await.unwrap();

        // Send MsgNoBlocks
        ingress_tx
            .send(Bytes::from(encode_message(&BlockFetchMessage::MsgNoBlocks)))
            .await
            .unwrap();

        let count = handle.await.unwrap().unwrap();
        assert_eq!(count, 0);
    }
}
