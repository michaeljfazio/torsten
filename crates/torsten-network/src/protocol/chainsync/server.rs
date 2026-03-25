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
//!
//! ## Header Extraction
//! N2N ChainSync sends only the block *header*, not the full block.  Headers are
//! 10–100× smaller than full blocks; sending full blocks wastes bandwidth and
//! produces CBOR that Haskell decoders cannot parse as a header.
//!
//! Shelley+ blocks are HFC-wrapped: `[era_tag, block_body]` where
//! `block_body = [header, tx_bodies, tx_witnesses, aux_data, invalid_txs]`.
//! We extract `header` (index 0 of `block_body`) and re-wrap as
//! `[era_tag, #6.24(bstr(header_cbor))]`.
//!
//! Byron blocks (era_tag = 0) use a different internal structure; they are
//! handled by a dedicated path that skips tag-24 wrapping.

use std::io::Write as _;
use std::time::Duration;

use minicbor::{Decoder, Encoder};
use tokio::sync::broadcast;

use crate::codec::Point;
use crate::error::ProtocolError;
use crate::mux::channel::MuxChannel;
use crate::BlockProvider;

use super::{decode_message, encode_message, ChainSyncMessage};

// CBOR tag number for embedded CBOR (RFC 7049 §2.4.4.1 / RFC 8949 §3.4.5.1).
const CBOR_TAG_EMBEDDED: u64 = 24;

/// Extract the block header from raw HFC-wrapped block CBOR and encode it as
/// `[era_id, #6.24(bstr(header_cbor))]` ready for inlining into MsgRollForward.
///
/// Shelley+ HFC layout: `[era_tag, [header, tx_bodies, tx_witnesses, aux_data, invalid_txs]]`
/// Byron HFC layout:    `[era_tag, [header, body, extra]]` (similar; header at index 0)
///
/// Returns an error if the CBOR does not match the expected structure.  The
/// caller MUST propagate the error and MUST NOT fall back to sending the full
/// block — doing so would produce incorrect wire output.
pub fn extract_header_for_chainsync(block_cbor: &[u8]) -> Result<Vec<u8>, String> {
    let mut dec = Decoder::new(block_cbor);

    // Outer array: [era_tag, block_body]
    dec.array()
        .map_err(|e| format!("block CBOR: expected outer array: {e}"))?;
    let era_tag = dec
        .u64()
        .map_err(|e| format!("block CBOR: expected era_tag u64: {e}"))?;

    // Inner array: [header, ...]  — we only need the first element.
    dec.array()
        .map_err(|e| format!("block CBOR (era {era_tag}): expected inner block array: {e}"))?;

    // Capture the raw CBOR bytes of the header sub-value.
    let header_start = dec.position();
    dec.skip()
        .map_err(|e| format!("block CBOR (era {era_tag}): could not skip header: {e}"))?;
    let header_end = dec.position();
    let header_cbor = &block_cbor[header_start..header_end];

    // Encode the HFC-wrapped header: [era_tag, #6.24(bstr(header_cbor))]
    //
    // The outer array [era_tag, ...] is the standard HFC wrapper used by
    // cardano-node for all ChainSync MsgRollForward header payloads.
    let mut buf = Vec::with_capacity(8 + header_cbor.len());
    let mut enc = Encoder::new(&mut buf);
    enc.array(2)
        .map_err(|e| format!("encode hfc header: array: {e}"))?;
    enc.u64(era_tag)
        .map_err(|e| format!("encode hfc header: era_tag: {e}"))?;
    enc.tag(minicbor::data::Tag::new(CBOR_TAG_EMBEDDED))
        .map_err(|e| format!("encode hfc header: tag24: {e}"))?;
    enc.bytes(header_cbor)
        .map_err(|e| format!("encode hfc header: bytes: {e}"))?;
    // Write any remaining encoder state.
    enc.writer_mut()
        .flush()
        .map_err(|e| format!("encode hfc header: flush: {e}"))?;

    Ok(buf)
}

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
        // Try to find the next block after our cursor.
        if let Some((slot, hash, block_cbor)) =
            block_provider.get_next_block_after_slot(self.cursor_slot)
        {
            // Extract the block header and encode it as the HFC-wrapped header
            // payload expected by N2N ChainSync: [era_id, #6.24(bstr(header_cbor))].
            let hfc_header = extract_header_for_chainsync(&block_cbor).map_err(|reason| {
                ProtocolError::CborDecode {
                    protocol: "ChainSync",
                    reason: format!("header extraction failed for block at slot {slot}: {reason}"),
                }
            })?;

            let tip = block_provider.get_tip();
            let response = encode_message(&ChainSyncMessage::MsgRollForward {
                header: hfc_header,
                tip_slot: tip.slot,
                tip_hash: tip.hash,
                tip_block_number: tip.block_number,
            });
            channel.send(response).await.map_err(ProtocolError::from)?;

            // Advance cursor to the block we just served.
            self.cursor_slot = slot;
            self.cursor_hash = hash;
            return Ok(());
        }

        // We're at the tip — send MsgAwaitReply and wait for announcement.
        let await_msg = encode_message(&ChainSyncMessage::MsgAwaitReply);
        channel.send(await_msg).await.map_err(ProtocolError::from)?;

        // Wait for a block announcement with a fixed timeout.
        // Haskell uses a timeout range of 135–911 seconds; we use 135s as the lower bound.
        let timeout = Duration::from_secs(135);

        tokio::select! {
            announcement = announcement_rx.recv() => {
                match announcement {
                    Ok(ann) => {
                        // New block announced — fetch and serve its header.
                        if let Some(block_cbor) = block_provider.get_block(&ann.hash) {
                            let hfc_header = extract_header_for_chainsync(&block_cbor)
                                .map_err(|reason| ProtocolError::CborDecode {
                                    protocol: "ChainSync",
                                    reason: format!(
                                        "header extraction failed for announced block \
                                         at slot {}: {reason}",
                                        ann.slot
                                    ),
                                })?;
                            let tip = block_provider.get_tip();
                            let response = encode_message(&ChainSyncMessage::MsgRollForward {
                                header: hfc_header,
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
                        tracing::warn!(
                            n,
                            "chainsync server: announcement channel lagged; catching up from tip"
                        );
                        // Try to catch up from the current tip.
                        let tip = block_provider.get_tip();
                        if let Some(block_cbor) = block_provider.get_block(&tip.hash) {
                            let hfc_header = extract_header_for_chainsync(&block_cbor)
                                .map_err(|reason| ProtocolError::CborDecode {
                                    protocol: "ChainSync",
                                    reason: format!(
                                        "header extraction failed for tip block \
                                         at slot {}: {reason}",
                                        tip.slot
                                    ),
                                })?;
                            let response = encode_message(&ChainSyncMessage::MsgRollForward {
                                header: hfc_header,
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
                        // Broadcast sender dropped — node is shutting down.
                        Ok(())
                    }
                }
            }
            _ = tokio::time::sleep(timeout) => {
                // Timeout — check whether a new block has arrived while we waited.
                if let Some((slot, hash, block_cbor)) =
                    block_provider.get_next_block_after_slot(self.cursor_slot)
                {
                    let hfc_header =
                        extract_header_for_chainsync(&block_cbor).map_err(|reason| {
                            ProtocolError::CborDecode {
                                protocol: "ChainSync",
                                reason: format!(
                                    "header extraction failed for timeout-polled block \
                                     at slot {slot}: {reason}"
                                ),
                            }
                        })?;
                    let tip = block_provider.get_tip();
                    let response = encode_message(&ChainSyncMessage::MsgRollForward {
                        header: hfc_header,
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
    use minicbor::Encoder;
    use tokio::sync::mpsc;

    // ─── helpers ──────────────────────────────────────────────────────────────

    /// Encode `value` as a CBOR bstr — returns the bytes used to store `value`
    /// inside the block's inner array.  This is what the header element looks
    /// like at wire level when we put raw bytes there for testing.
    fn cbor_encode_bytes(value: &[u8]) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut enc = Encoder::new(&mut buf);
        enc.bytes(value).unwrap();
        buf
    }

    /// Build a minimal valid HFC-wrapped block CBOR for testing.
    ///
    /// Layout: `[era_tag, [header_cbor, [], [], null, []]]`
    ///
    /// The header element is stored as a CBOR bstr containing `header_bytes`.
    /// `extract_header_for_chainsync` will capture the full CBOR encoding of
    /// that bstr (including the length prefix) as the header sub-value.
    fn make_hfc_block(era_tag: u64, header_bytes: &[u8]) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut enc = Encoder::new(&mut buf);
        // Outer: [era_tag, inner_array]
        enc.array(2).unwrap();
        enc.u64(era_tag).unwrap();
        // Inner block body array: [header, tx_bodies, tx_witnesses, aux_data, invalid_txs]
        enc.array(5).unwrap();
        enc.bytes(header_bytes).unwrap(); // header stored as a bstr
        enc.array(0).unwrap(); // tx_bodies: []
        enc.array(0).unwrap(); // tx_witnesses: []
        enc.null().unwrap(); // aux_data: null
        enc.array(0).unwrap(); // invalid_txs: []
        buf
    }

    /// Build the expected HFC-wrapped header bytes that `extract_header_for_chainsync`
    /// should produce.
    ///
    /// `header_cbor` must be the **CBOR-encoded** form of the header element as
    /// it appears inside the block — i.e. the bytes captured by `dec.skip()`.
    /// For blocks built with `make_hfc_block`, pass `cbor_encode_bytes(raw_bytes)`.
    fn expected_hfc_header(era_tag: u64, header_cbor: &[u8]) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut enc = Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u64(era_tag).unwrap();
        enc.tag(minicbor::data::Tag::new(24)).unwrap();
        // header_cbor is the raw CBOR of the header element; embed it as a bstr
        // (this is what tag24 wraps — the CBOR serialisation of the header).
        enc.bytes(header_cbor).unwrap();
        buf
    }

    /// Mock block provider that stores (slot, hash, block_cbor) triples.
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

    // ─── unit tests for extract_header_for_chainsync ──────────────────────────

    #[test]
    fn extract_header_conway_block() {
        // Build a synthetic Conway (era_tag=6) HFC block.
        // In our test fixture the header element is a bstr; extraction captures
        // the full CBOR encoding of that element (bstr length prefix + bytes).
        let inner_header_bytes = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let block_cbor = make_hfc_block(6, &inner_header_bytes);

        let hfc_header = extract_header_for_chainsync(&block_cbor)
            .expect("extraction should succeed for valid Conway block");

        // The extractor captures the raw CBOR of the header element and wraps it
        // in [6, #6.24(bstr(header_cbor))].  Pass the CBOR form of the element.
        let header_cbor = cbor_encode_bytes(&inner_header_bytes);
        let expected = expected_hfc_header(6, &header_cbor);
        assert_eq!(
            hfc_header, expected,
            "extracted HFC header does not match expected encoding"
        );
    }

    #[test]
    fn extract_header_shelley_block() {
        // Shelley (era_tag=2) — same structure, different era identifier.
        let inner_header_bytes = vec![0x01, 0x02, 0x03];
        let block_cbor = make_hfc_block(2, &inner_header_bytes);

        let hfc_header =
            extract_header_for_chainsync(&block_cbor).expect("Shelley extraction should succeed");

        let header_cbor = cbor_encode_bytes(&inner_header_bytes);
        let expected = expected_hfc_header(2, &header_cbor);
        assert_eq!(hfc_header, expected);
    }

    #[test]
    fn extract_header_larger_inner_header() {
        // Verify extraction with a larger (256-byte) inner header payload.
        let inner_header_bytes: Vec<u8> = (0u8..=255u8).collect();
        let block_cbor = make_hfc_block(6, &inner_header_bytes);

        let hfc_header = extract_header_for_chainsync(&block_cbor)
            .expect("extraction should succeed with large inner header");

        let header_cbor = cbor_encode_bytes(&inner_header_bytes);
        let expected = expected_hfc_header(6, &header_cbor);
        assert_eq!(hfc_header, expected);
    }

    #[test]
    fn extract_header_invalid_cbor_returns_error() {
        // Truncated / garbage input must return an Err, not panic.
        let result = extract_header_for_chainsync(&[0xFF, 0x00, 0x01]);
        assert!(
            result.is_err(),
            "expected Err for invalid CBOR, got Ok: {result:?}"
        );
    }

    #[test]
    fn extract_header_empty_input_returns_error() {
        let result = extract_header_for_chainsync(&[]);
        assert!(result.is_err(), "expected Err for empty input");
    }

    // ─── integration tests for ChainSync server ───────────────────────────────

    #[tokio::test]
    async fn find_intersect_with_known_block() {
        // Block CBOR does not need to be valid for FindIntersect (the server only
        // checks presence via has_block, not the CBOR content).
        let block_cbor = make_hfc_block(6, &[0x01, 0x02]);
        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();
        let provider = MockBlockProvider {
            blocks: vec![(100, [0xAA; 32], block_cbor)],
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
    async fn request_next_serves_header_not_full_block() {
        // The mock provider holds blocks whose CBOR is a valid HFC-wrapped block.
        // The server must extract the header and send [era_id, #6.24(bstr(hdr))].
        let inner_header_bytes_a = vec![0xAA, 0xBB];
        let inner_header_bytes_b = vec![0xCC, 0xDD];
        let block_a = make_hfc_block(6, &inner_header_bytes_a);
        let block_b = make_hfc_block(6, &inner_header_bytes_b);

        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();
        let provider = MockBlockProvider {
            blocks: vec![(10, [0x01; 32], block_a), (20, [0x02; 32], block_b)],
        };
        let (ann_tx, _) = broadcast::channel(16);

        let mut server = ChainSyncServer::new();

        let handle = tokio::spawn(async move {
            server
                .run(&mut channel, &provider, ann_tx.subscribe())
                .await
        });

        // Find intersection at origin (sets cursor_slot = 0).
        let find = encode_message(&ChainSyncMessage::MsgFindIntersect(vec![Point::Origin]));
        ingress_tx.send(Bytes::from(find)).await.unwrap();
        let _ = egress_rx.recv().await.unwrap(); // MsgIntersectFound

        // Request next — should get the header for the block at slot 10.
        let req = encode_message(&ChainSyncMessage::MsgRequestNext);
        ingress_tx.send(Bytes::from(req)).await.unwrap();

        let (_, _, resp) = egress_rx.recv().await.unwrap();
        let msg = decode_message(&resp).unwrap();
        if let ChainSyncMessage::MsgRollForward { header, .. } = msg {
            // The header field must be the HFC-wrapped header, not the full block.
            // In our fixture the header element is a bstr, so its CBOR encoding
            // is cbor_encode_bytes(inner_header_bytes_a).
            let header_cbor = cbor_encode_bytes(&inner_header_bytes_a);
            let expected = expected_hfc_header(6, &header_cbor);
            assert_eq!(
                header, expected,
                "server sent incorrect HFC-wrapped header; \
                 expected [era_id, #6.24(bstr(inner))], got {header:?}"
            );
            // Sanity-check: the header is strictly smaller than the full block CBOR.
            // (Full block includes tx_bodies, tx_witnesses, aux_data, invalid_txs.)
            let full_block = make_hfc_block(6, &inner_header_bytes_a);
            assert!(
                header.len() < full_block.len(),
                "header ({} bytes) should be smaller than full block ({} bytes)",
                header.len(),
                full_block.len()
            );
        } else {
            panic!("expected MsgRollForward, got {msg:?}");
        }

        // Send MsgDone
        let done = encode_message(&ChainSyncMessage::MsgDone);
        ingress_tx.send(Bytes::from(done)).await.unwrap();
        handle.await.unwrap().unwrap();
    }
}
