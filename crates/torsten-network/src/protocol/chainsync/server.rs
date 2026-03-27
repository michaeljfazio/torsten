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

// Re-use shared HFC helpers from the protocol module.
use crate::protocol::{storage_era_tag_to_hfc_index, CBOR_TAG_EMBEDDED};

/// Extract the block header from raw HFC-wrapped block CBOR and encode it as
/// `[hfc_index, #6.24(bstr(header_cbor))]` ready for inlining into MsgRollForward.
///
/// # Era tag conversion
///
/// The block CBOR uses pallas storage era tags (Byron=0/1, Shelley=2, …,
/// Conway=7).  The N2N ChainSync `MsgRollForward` uses HFC NS indices
/// (Byron=0, Shelley=1, …, Conway=6) — one less than the storage tag for all
/// post-Byron eras.  This function converts between the two schemes so that
/// Haskell peers route the header to the correct era-specific decoder.
///
/// # Block layout
///
/// ```text
/// Shelley+ HFC layout: [storage_era_tag, [header, tx_bodies, tx_witnesses, aux_data, invalid_txs]]
/// Byron HFC layout:    [storage_era_tag, [header, body, extra]]
/// ```
///
/// Returns an error if the CBOR does not match the expected structure.  The
/// caller MUST propagate the error and MUST NOT fall back to sending the full
/// block — doing so would produce incorrect wire output.
pub fn extract_header_for_chainsync(block_cbor: &[u8]) -> Result<Vec<u8>, String> {
    let mut dec = Decoder::new(block_cbor);

    // Outer array: [storage_era_tag, block_body]
    dec.array()
        .map_err(|e| format!("block CBOR: expected outer array: {e}"))?;
    let storage_era_tag = dec
        .u64()
        .map_err(|e| format!("block CBOR: expected era_tag u64: {e}"))?;

    // Convert to HFC NS index (the value Haskell's encodeNS/decodeNS expects).
    let hfc_index = storage_era_tag_to_hfc_index(storage_era_tag)?;

    // Inner array: [header, ...]  — we only need the first element.
    dec.array().map_err(|e| {
        format!("block CBOR (storage_era={storage_era_tag}): expected inner block array: {e}")
    })?;

    // Capture the raw CBOR bytes of the header sub-value.
    let header_start = dec.position();
    dec.skip().map_err(|e| {
        format!("block CBOR (storage_era={storage_era_tag}): could not skip header: {e}")
    })?;
    let header_end = dec.position();
    let header_cbor = &block_cbor[header_start..header_end];

    // Encode the HFC-wrapped header: [hfc_index, #6.24(bstr(header_cbor))]
    //
    // This is the format expected by Haskell's `dispatchDecoder` in
    // `Ouroboros.Consensus.HardFork.Combinator.Serialisation.SerialiseNodeToNode`.
    // `encodeNS` produces `array(2)[era_index_u8, tag(24)(header_bytes)]`.
    let mut buf = Vec::with_capacity(8 + header_cbor.len());
    let mut enc = Encoder::new(&mut buf);
    enc.array(2)
        .map_err(|e| format!("encode hfc header: array: {e}"))?;
    enc.u8(hfc_index)
        .map_err(|e| format!("encode hfc header: hfc_index: {e}"))?;
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
    /// True when the cursor is at Origin — meaning no block has been served
    /// yet and we must include blocks at slot 0 (e.g. Byron genesis EBB).
    cursor_at_origin: bool,
}

impl ChainSyncServer {
    /// Create a new server with no cursor (must find intersection first).
    pub fn new() -> Self {
        Self {
            cursor_slot: 0,
            cursor_hash: [0; 32],
            cursor_initialized: false,
            cursor_at_origin: false,
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
                    self.cursor_at_origin = true;

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
                        self.cursor_at_origin = false;

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
        //
        // When cursor_at_origin is true, we use the inclusive `>=` lookup so
        // that blocks at slot 0 (e.g. Byron genesis EBB) are not skipped.
        // The strict `>` lookup would miss them since cursor_slot is 0.
        let next_block = if self.cursor_at_origin {
            block_provider.get_block_at_or_after_slot(0)
        } else {
            block_provider.get_next_block_after_slot(self.cursor_slot)
        };

        if let Some((slot, hash, block_cbor)) = next_block {
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
            self.cursor_at_origin = false;
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
    /// `storage_era_tag` is the era tag from the block's on-disk/wire CBOR (pallas
    /// convention: Byron=0/1, Shelley=2, ..., Conway=7).  The function converts it
    /// to the HFC NS index and encodes `[hfc_index_u8, #6.24(bstr(header_cbor))]`.
    ///
    /// `header_cbor` must be the **CBOR-encoded** form of the header element as
    /// it appears inside the block — i.e. the bytes captured by `dec.skip()`.
    /// For blocks built with `make_hfc_block`, pass `cbor_encode_bytes(raw_bytes)`.
    fn expected_hfc_header(storage_era_tag: u64, header_cbor: &[u8]) -> Vec<u8> {
        let hfc_index = storage_era_tag_to_hfc_index(storage_era_tag)
            .expect("test fixture used invalid storage era tag");
        let mut buf = Vec::new();
        let mut enc = Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u8(hfc_index).unwrap();
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
            std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        );
        (channel, egress_rx, ingress_tx)
    }

    // ─── unit tests for extract_header_for_chainsync ──────────────────────────

    #[test]
    fn extract_header_conway_block() {
        // Build a synthetic Conway (storage era_tag=7, HFC index=6) HFC block.
        // Pallas uses era_tag=7 for Conway blocks in ImmutableDB and block-fetch.
        // The ChainSync header wire format uses HFC NS index=6 for Conway.
        // In our test fixture the header element is a bstr; extraction captures
        // the full CBOR encoding of that element (bstr length prefix + bytes).
        let inner_header_bytes = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let block_cbor = make_hfc_block(7, &inner_header_bytes); // storage era_tag=7 (Conway)

        let hfc_header = extract_header_for_chainsync(&block_cbor)
            .expect("extraction should succeed for valid Conway block");

        // The extractor converts storage era_tag=7 → HFC index=6, then produces
        // [6_u8, #6.24(bstr(header_cbor))].  Pass the CBOR form of the element.
        let header_cbor = cbor_encode_bytes(&inner_header_bytes);
        let expected = expected_hfc_header(7, &header_cbor); // storage tag→hfc_index conversion
        assert_eq!(
            hfc_header, expected,
            "extracted HFC header does not match expected encoding"
        );

        // Verify the HFC index in the output is actually 6 (Conway).
        let mut dec = minicbor::Decoder::new(&hfc_header);
        dec.array().unwrap();
        let hfc_idx = dec.u8().unwrap();
        assert_eq!(hfc_idx, 6, "Conway blocks must use HFC NS index 6");
    }

    #[test]
    fn extract_header_babbage_block() {
        // Babbage (storage era_tag=6, HFC index=5) — distinct from Conway.
        let inner_header_bytes = vec![0xBA, 0xBB, 0xAA, 0xBE];
        let block_cbor = make_hfc_block(6, &inner_header_bytes); // storage era_tag=6 (Babbage)

        let hfc_header = extract_header_for_chainsync(&block_cbor)
            .expect("extraction should succeed for valid Babbage block");

        let header_cbor = cbor_encode_bytes(&inner_header_bytes);
        let expected = expected_hfc_header(6, &header_cbor);
        assert_eq!(hfc_header, expected);

        // Verify the HFC index in the output is actually 5 (Babbage).
        let mut dec = minicbor::Decoder::new(&hfc_header);
        dec.array().unwrap();
        let hfc_idx = dec.u8().unwrap();
        assert_eq!(hfc_idx, 5, "Babbage blocks must use HFC NS index 5");
    }

    #[test]
    fn extract_header_shelley_block() {
        // Shelley (storage era_tag=2, HFC index=1) — same structure, different era identifier.
        let inner_header_bytes = vec![0x01, 0x02, 0x03];
        let block_cbor = make_hfc_block(2, &inner_header_bytes);

        let hfc_header =
            extract_header_for_chainsync(&block_cbor).expect("Shelley extraction should succeed");

        let header_cbor = cbor_encode_bytes(&inner_header_bytes);
        let expected = expected_hfc_header(2, &header_cbor);
        assert_eq!(hfc_header, expected);

        // Verify the HFC index in the output is actually 1 (Shelley).
        let mut dec = minicbor::Decoder::new(&hfc_header);
        dec.array().unwrap();
        let hfc_idx = dec.u8().unwrap();
        assert_eq!(hfc_idx, 1, "Shelley blocks must use HFC NS index 1");
    }

    #[test]
    fn extract_header_larger_inner_header() {
        // Verify extraction with a larger (256-byte) inner header payload (Conway).
        let inner_header_bytes: Vec<u8> = (0u8..=255u8).collect();
        let block_cbor = make_hfc_block(7, &inner_header_bytes); // Conway storage era_tag=7

        let hfc_header = extract_header_for_chainsync(&block_cbor)
            .expect("extraction should succeed with large inner header");

        let header_cbor = cbor_encode_bytes(&inner_header_bytes);
        let expected = expected_hfc_header(7, &header_cbor); // Conway storage_era_tag=7
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
        // Use Conway storage era_tag=7 for realistic block CBOR.
        let block_cbor = make_hfc_block(7, &[0x01, 0x02]);
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
        // The server must extract the header and send [hfc_index, #6.24(bstr(hdr))].
        // Use Conway storage era_tag=7 (→ HFC index=6) for realistic fixtures.
        let inner_header_bytes_a = vec![0xAA, 0xBB];
        let inner_header_bytes_b = vec![0xCC, 0xDD];
        let block_a = make_hfc_block(7, &inner_header_bytes_a); // Conway storage_era_tag=7
        let block_b = make_hfc_block(7, &inner_header_bytes_b); // Conway storage_era_tag=7

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
            // Conway storage_era_tag=7 → HFC index=6, so expected = [6, tag24(bytes)].
            let header_cbor = cbor_encode_bytes(&inner_header_bytes_a);
            let expected = expected_hfc_header(7, &header_cbor); // storage_era_tag=7 (Conway)
            assert_eq!(
                header, expected,
                "server sent incorrect HFC-wrapped header; \
                 expected [hfc_index=6, #6.24(bstr(inner))], got {header:?}"
            );
            // Sanity-check: the header is strictly smaller than the full block CBOR.
            // (Full block includes tx_bodies, tx_witnesses, aux_data, invalid_txs.)
            let full_block = make_hfc_block(7, &inner_header_bytes_a);
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
