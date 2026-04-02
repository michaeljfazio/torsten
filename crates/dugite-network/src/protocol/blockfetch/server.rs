//! BlockFetch server — serves block ranges to requesting peers.
//!
//! Handles `MsgRequestRange(from, to)` by looking up blocks via [`BlockProvider`]
//! and streaming them as `MsgStartBatch` → `MsgBlock` × N → `MsgBatchDone`.
//! Sends `MsgNoBlocks` if the requested range is unavailable.
//!
//! ## HFC wrapping (N2N wire format)
//!
//! The Haskell N2N BlockFetch encoding is derived from the SerialiseNodeToNode
//! instance for HardForkBlock, which calls:
//!
//!   `encodeNodeToNode ccfg _ = wrapCBORinCBOR (encodeDiskHfcBlock ccfg)`
//!
//! where `wrapCBORinCBOR enc x = Serialise.encode (tag(24) bstr(enc(x)))`.
//!
//! The `encodeDiskHfcBlock` for Cardano is a **custom** override (not the generic
//! `encodeNS`) that emits `[era_word, block_body]` — identical to the on-disk
//! storage format.  The mapping is:
//!   - Byron EBB         → [0, body]
//!   - Byron regular     → [1, body]
//!   - Shelley           → [2, body]
//!   - Allegra           → [3, body]
//!   - Mary              → [4, body]
//!   - Alonzo            → [5, body]
//!   - Babbage           → [6, body]
//!   - Conway            → [7, body]
//!   - Dijkstra          → [8, body]  (future era)
//!
//! Therefore the complete MsgBlock wire encoding is:
//!
//! ```text
//!   [2,                                  ← array(2)
//!     word(4),                           ← MsgBlock tag
//!     #6.24(bstr( [era_word, body] ))    ← tag(24) wrapping raw stored CBOR
//!   ]
//! ```
//!
//! Since Dugite stores blocks in the same `[era_word, body]` layout that
//! `encodeDiskHfcBlock` produces, the stored bytes need NO structural
//! transformation — they are placed verbatim inside tag(24).
//!
//! ## Range validation
//! - Maximum blocks per batch: 100 (prevents memory exhaustion)

use std::io::Write as _;

use minicbor::Encoder;

use crate::codec::Point;
use crate::error::ProtocolError;
use crate::mux::channel::MuxChannel;
use crate::protocol::CBOR_TAG_EMBEDDED;
use crate::BlockProvider;

use super::{decode_message, encode_message, BlockFetchMessage, TAG_BLOCK};

/// Safety limit on blocks pre-collected per batch response.
///
/// This is a **block count** limit, not a slot count limit.  The slot range
/// `(from_slot, to_slot)` defines the range boundaries, but the `limit`
/// parameter passed to `get_blocks_in_range()` caps the number of actual
/// blocks returned from the iterator.
///
/// The Haskell BlockFetch client expects ALL blocks between from_point and
/// to_point to be served in a single batch.  Sending fewer triggers
/// `BlockFetchProtocolFailureTooFewBlocks`.  Typical ranges are 10–200
/// blocks; in dense chain regions they can reach ~500.  We set a generous
/// upper bound to prevent unbounded memory use from malicious requests.
pub const MAX_BLOCKS_PER_BATCH: usize = 2000;

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
                    tracing::debug!(
                        from = ?from,
                        to = ?to,
                        "blockfetch server: received MsgRequestRange"
                    );
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
        // Validate range — extract from_slot for iteration.
        let from_slot = match from {
            Point::Origin => 0,
            Point::Specific(slot, _) => *slot,
        };
        let to_slot = match to {
            Point::Origin => 0,
            Point::Specific(slot, _) => *slot,
        };

        // Basic range validity (no slot span limit — Haskell doesn't have one,
        // and the MAX_BLOCKS_PER_BATCH cap prevents memory exhaustion).
        if to_slot < from_slot {
            let no_blocks = encode_message(&BlockFetchMessage::MsgNoBlocks);
            channel.send(no_blocks).await.map_err(ProtocolError::from)?;
            return Ok(());
        }

        // Verify we have the starting block.
        let have_from = match from {
            Point::Origin => true,
            Point::Specific(_, hash) => block_provider.has_block(hash),
        };

        if !have_from {
            let no_blocks = encode_message(&BlockFetchMessage::MsgNoBlocks);
            channel.send(no_blocks).await.map_err(ProtocolError::from)?;
            return Ok(());
        }

        // Collect blocks in [from_slot, to_slot] via a single batched call.
        // ChainDBBlockProvider overrides get_blocks_in_range() to acquire
        // chain_db.read() in chunks of 50 blocks, preventing the per-block
        // block_in_place / blocking_read pattern from exhausting the tokio
        // async worker thread pool when the Haskell relay is syncing rapidly
        // with pipelined batch requests.
        let blocks = block_provider.get_blocks_in_range(from_slot, to_slot, MAX_BLOCKS_PER_BATCH);

        if blocks.is_empty() {
            let no_blocks = encode_message(&BlockFetchMessage::MsgNoBlocks);
            channel.send(no_blocks).await.map_err(ProtocolError::from)?;
            return Ok(());
        }

        // Stream: MsgStartBatch → MsgBlock × N → MsgBatchDone.
        let start = encode_message(&BlockFetchMessage::MsgStartBatch);
        channel.send(start).await.map_err(ProtocolError::from)?;

        for (slot, hash, block_cbor) in &blocks {
            tracing::debug!(
                slot,
                hash = hex::encode(hash),
                cbor_len = block_cbor.len(),
                first_bytes = hex::encode(&block_cbor[..block_cbor.len().min(16)]),
                "blockfetch server: serving block"
            );
            // Encode MsgBlock: [4, tag(24) bstr(stored_block_cbor)].
            // The stored CBOR format [era_word, body] is identical to what
            // Haskell's encodeDiskHfcBlock produces, so it goes verbatim
            // inside the CBOR-in-CBOR tag(24) wrapper.
            let block_msg = Self::encode_hfc_msg_block(block_cbor).map_err(|reason| {
                ProtocolError::CborDecode {
                    protocol: "BlockFetch",
                    reason: format!("HFC wrapping failed: {reason}"),
                }
            })?;
            channel.send(block_msg).await.map_err(ProtocolError::from)?;
        }

        let done = encode_message(&BlockFetchMessage::MsgBatchDone);
        channel.send(done).await.map_err(ProtocolError::from)?;

        tracing::debug!(
            block_count = blocks.len(),
            from_slot,
            to_slot,
            first_hash = blocks
                .first()
                .map(|(_, h, _)| hex::encode(h))
                .unwrap_or_default(),
            "blockfetch server: served batch"
        );

        Ok(())
    }

    /// Encode a single block as an HFC-wrapped `MsgBlock` message.
    ///
    /// ## Wire format
    ///
    /// The Haskell N2N `SerialiseNodeToNode` instance for `HardForkBlock` is:
    ///
    /// ```haskell
    /// encodeNodeToNode ccfg _ = wrapCBORinCBOR (encodeDiskHfcBlock ccfg)
    /// ```
    ///
    /// `wrapCBORinCBOR` serialises the value and wraps it in CBOR tag(24):
    ///
    /// ```text
    /// tag(24) bstr( encodeDiskHfcBlock_output )
    /// ```
    ///
    /// The Cardano-specific `encodeDiskHfcBlock` override produces the same
    /// `[era_word, block_body]` layout used for on-disk storage (NOT the
    /// generic 0-based NS index produced by `encodeNS`).  Therefore the
    /// stored block CBOR bytes can be placed **verbatim** inside tag(24)
    /// without any structural transformation.
    ///
    /// The resulting `MsgBlock` wire encoding is:
    ///
    /// ```text
    /// array(2) [
    ///   word(4),                          -- MsgBlock tag
    ///   tag(24) bstr( stored_block_cbor ) -- CBOR-in-CBOR
    /// ]
    /// ```
    fn encode_hfc_msg_block(block_cbor: &[u8]) -> Result<Vec<u8>, String> {
        // Pre-allocate: 1 (array(2)) + 1 (word 4) + 2 (tag 24) + varint (len) + payload.
        let mut buf = Vec::with_capacity(8 + block_cbor.len());
        let mut enc = Encoder::new(&mut buf);

        enc.array(2).map_err(|e| format!("MsgBlock array: {e}"))?;
        enc.u64(TAG_BLOCK)
            .map_err(|e| format!("MsgBlock tag: {e}"))?;
        // tag(24) wraps the complete stored-format CBOR bytes verbatim.
        enc.tag(minicbor::data::Tag::new(CBOR_TAG_EMBEDDED))
            .map_err(|e| format!("tag(24): {e}"))?;
        enc.bytes(block_cbor)
            .map_err(|e| format!("block bstr: {e}"))?;
        enc.writer_mut()
            .flush()
            .map_err(|e| format!("flush: {e}"))?;

        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TipInfo;
    use bytes::Bytes;
    use minicbor::Decoder;
    use tokio::sync::mpsc;

    /// Build a minimal storage-format block CBOR for testing.
    ///
    /// Layout (matching Haskell `encodeDiskHfcBlock` for Shelley+ eras):
    /// `[era_tag, [header_cbor, [], [], null, []]]`
    fn make_storage_block(era_tag: u64, header_bytes: &[u8]) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut enc = Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u64(era_tag).unwrap();
        enc.array(5).unwrap();
        enc.bytes(header_bytes).unwrap(); // header
        enc.array(0).unwrap(); // tx_bodies
        enc.array(0).unwrap(); // tx_witnesses
        enc.null().unwrap(); // aux_data
        enc.array(0).unwrap(); // invalid_txs
        buf
    }

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
            std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        );
        (channel, egress_rx, ingress_tx)
    }

    #[tokio::test]
    async fn serves_block_range_with_hfc_wrapping() {
        // Use Conway storage-format blocks (era_tag=7).
        let block_a = make_storage_block(7, &[0xAA, 0xBB]);
        let block_b = make_storage_block(7, &[0xCC, 0xDD]);
        let block_c = make_storage_block(7, &[0xEE, 0xFF]);

        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();
        let provider = MockBlockProvider {
            blocks: vec![
                (10, [0x01; 32], block_a),
                (20, [0x02; 32], block_b),
                (30, [0x03; 32], block_c),
            ],
        };

        let handle =
            tokio::spawn(async move { BlockFetchServer::run(&mut channel, &provider).await });

        // Request range from slot 10 to slot 30.
        let req = encode_message(&BlockFetchMessage::MsgRequestRange {
            from: Point::Specific(10, [0x01; 32]),
            to: Point::Specific(30, [0x03; 32]),
        });
        ingress_tx.send(Bytes::from(req)).await.unwrap();

        // Should receive: MsgStartBatch → MsgBlock × 3 → MsgBatchDone.
        let (_, _, start) = egress_rx.recv().await.unwrap();
        assert!(matches!(
            decode_message(&start).unwrap(),
            BlockFetchMessage::MsgStartBatch
        ));

        // The server HFC-wraps each block. The decoder extracts the inner
        // block body from [hfc_index, tag24(body)] and returns it.
        for _ in 0..3 {
            let (_, _, block) = egress_rx.recv().await.unwrap();
            let msg = decode_message(&block).unwrap();
            assert!(
                matches!(msg, BlockFetchMessage::MsgBlock(_)),
                "expected MsgBlock, got {msg:?}"
            );
        }

        let (_, _, done_msg) = egress_rx.recv().await.unwrap();
        assert!(matches!(
            decode_message(&done_msg).unwrap(),
            BlockFetchMessage::MsgBatchDone
        ));

        // Send MsgClientDone.
        let client_done = encode_message(&BlockFetchMessage::MsgClientDone);
        ingress_tx.send(Bytes::from(client_done)).await.unwrap();
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn msgblock_wire_format_is_tag24_cbor_in_cbor() {
        // Verify the exact Haskell-compatible wire format:
        //   array(2) [ word(4), tag(24) bstr(stored_block_cbor) ]
        //
        // The Haskell SerialiseNodeToNode instance for HardForkBlock is:
        //   encodeNodeToNode ccfg _ = wrapCBORinCBOR (encodeDiskHfcBlock ccfg)
        //
        // wrapCBORinCBOR places the encodeDiskHfcBlock output (which already
        // has the [era_word, body] layout) inside tag(24).  There is NO
        // intermediate HFC array([hfc_index, ...]) layer.
        let stored_cbor = make_storage_block(7, &[0x01, 0x02]); // Conway era_tag=7
        let wire_bytes = BlockFetchServer::encode_hfc_msg_block(&stored_cbor).unwrap();

        let mut dec = Decoder::new(&wire_bytes);
        let arr = dec.array().unwrap();
        assert_eq!(arr, Some(2), "outer array must have length 2");
        assert_eq!(
            dec.u64().unwrap(),
            TAG_BLOCK,
            "first element must be MsgBlock tag (4)"
        );

        // Second element MUST be tag(24) — the CBOR-in-CBOR wrapper.
        let tag = dec.tag().unwrap();
        assert_eq!(
            tag.as_u64(),
            24,
            "second element must be tag(24) (CBOR-in-CBOR), not an array"
        );

        // The bstr payload must be the original stored CBOR verbatim.
        let payload = dec.bytes().unwrap();
        assert_eq!(
            payload,
            stored_cbor.as_slice(),
            "tag(24) payload must be the verbatim stored block CBOR"
        );
    }

    #[tokio::test]
    async fn no_blocks_when_range_missing() {
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

    #[tokio::test]
    async fn batch_range_uses_single_lock_acquisition() {
        // Verify that get_blocks_in_range() returns all blocks in a contiguous
        // slot range, exercising the default trait implementation which delegates
        // to get_block_at_or_after_slot / get_next_block_after_slot.
        let blocks: Vec<_> = (0..5u64)
            .map(|i| {
                let mut h = [0u8; 32];
                h[0] = i as u8;
                (i * 10 + 10, h, make_storage_block(7, &[i as u8]))
            })
            .collect();

        let provider = MockBlockProvider {
            blocks: blocks.clone(),
        };

        // Use the default trait implementation.
        let result = provider.get_blocks_in_range(10, 50, 100);
        assert_eq!(result.len(), 5, "should return all 5 blocks in range");
        for (i, (slot, hash, _cbor)) in result.iter().enumerate() {
            assert_eq!(*slot, (i as u64) * 10 + 10);
            assert_eq!(hash[0], i as u8);
        }

        // Partial range.
        let partial = provider.get_blocks_in_range(20, 40, 100);
        assert_eq!(partial.len(), 3, "should return blocks at slots 20, 30, 40");

        // Limit enforcement.
        let limited = provider.get_blocks_in_range(10, 50, 2);
        assert_eq!(limited.len(), 2, "limit should cap at 2 blocks");

        // Empty range.
        let empty = provider.get_blocks_in_range(100, 200, 100);
        assert_eq!(empty.len(), 0, "no blocks in range should return empty vec");
    }
}
