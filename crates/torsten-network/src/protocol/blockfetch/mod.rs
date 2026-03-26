//! BlockFetch mini-protocol codec and message types.
//!
//! BlockFetch downloads full blocks by range. The client requests a range
//! (from_point, to_point), and the server streams the blocks as a batch.
//!
//! ## State machine
//! ```text
//! BFIdle ──MsgRequestRange(from, to)──► BFBusy
//! BFBusy ──MsgStartBatch──► BFStreaming
//! BFBusy ──MsgNoBlocks──► BFIdle
//! BFStreaming ──MsgBlock(block)──► BFStreaming
//! BFStreaming ──MsgBatchDone──► BFIdle
//! BFIdle ──MsgClientDone──► BFDone
//! ```
//!
//! ## Wire format (CBOR)
//! - `MsgRequestRange` = `[0, from_point, to_point]`
//! - `MsgClientDone`   = `[1]`
//! - `MsgStartBatch`   = `[2]`
//! - `MsgNoBlocks`     = `[3]`
//! - `MsgBlock`        = `[4, tag(24) bstr(stored_block_cbor)]`  ← CBOR-in-CBOR
//! - `MsgBatchDone`    = `[5]`
//!
//! The block payload is always wrapped in CBOR tag(24).  The bstr content is
//! the raw Cardano HFC disk encoding: `[era_word, block_body]` (where
//! `era_word` is 0–8 matching Byron EBB through Dijkstra).  See the module
//! documentation on [`server`] for the full derivation.

pub mod client;
pub mod decision;
pub mod server;

use crate::codec::{self, Point};
use minicbor::{Decoder, Encoder};

/// BlockFetch protocol state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockFetchState {
    /// Client has agency — can request a range or send done.
    BFIdle,
    /// Server has agency — will start batch or report no blocks.
    BFBusy,
    /// Server streaming blocks — sends MsgBlock or MsgBatchDone.
    BFStreaming,
    /// Terminal state.
    BFDone,
}

/// BlockFetch protocol messages.
#[derive(Debug, Clone)]
pub enum BlockFetchMessage {
    /// Client requests a range of blocks (tag 0).
    MsgRequestRange { from: Point, to: Point },
    /// Client terminates the protocol (tag 1).
    MsgClientDone,
    /// Server starts streaming a batch of blocks (tag 2).
    MsgStartBatch,
    /// Server reports the requested range is unavailable (tag 3).
    MsgNoBlocks,
    /// Server sends a single block in the batch (tag 4).
    MsgBlock(Vec<u8>),
    /// Server signals end of the current batch (tag 5).
    MsgBatchDone,
}

// CBOR message tags
const TAG_REQUEST_RANGE: u64 = 0;
const TAG_CLIENT_DONE: u64 = 1;
const TAG_START_BATCH: u64 = 2;
const TAG_NO_BLOCKS: u64 = 3;
pub(crate) const TAG_BLOCK: u64 = 4;
const TAG_BATCH_DONE: u64 = 5;

/// Encode a BlockFetch message as CBOR.
pub fn encode_message(msg: &BlockFetchMessage) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut enc = Encoder::new(&mut buf);
    match msg {
        BlockFetchMessage::MsgRequestRange { from, to } => {
            enc.array(3).expect("infallible");
            enc.u64(TAG_REQUEST_RANGE).expect("infallible");
            codec::encode_point(&mut enc, from);
            codec::encode_point(&mut enc, to);
        }
        BlockFetchMessage::MsgClientDone => {
            enc.array(1).expect("infallible");
            enc.u64(TAG_CLIENT_DONE).expect("infallible");
        }
        BlockFetchMessage::MsgStartBatch => {
            enc.array(1).expect("infallible");
            enc.u64(TAG_START_BATCH).expect("infallible");
        }
        BlockFetchMessage::MsgNoBlocks => {
            enc.array(1).expect("infallible");
            enc.u64(TAG_NO_BLOCKS).expect("infallible");
        }
        BlockFetchMessage::MsgBlock(block) => {
            enc.array(2).expect("infallible");
            enc.u64(TAG_BLOCK).expect("infallible");
            enc.bytes(block).expect("infallible");
        }
        BlockFetchMessage::MsgBatchDone => {
            enc.array(1).expect("infallible");
            enc.u64(TAG_BATCH_DONE).expect("infallible");
        }
    }
    buf
}

/// Decode a BlockFetch message from CBOR bytes.
pub fn decode_message(data: &[u8]) -> Result<BlockFetchMessage, String> {
    let mut dec = Decoder::new(data);
    let _arr_len = dec.array().map_err(|e| e.to_string())?;
    let tag = dec.u64().map_err(|e| e.to_string())?;

    match tag {
        TAG_REQUEST_RANGE => {
            let from = codec::decode_point(&mut dec).map_err(|e| e.to_string())?;
            let to = codec::decode_point(&mut dec).map_err(|e| e.to_string())?;
            Ok(BlockFetchMessage::MsgRequestRange { from, to })
        }
        TAG_CLIENT_DONE => Ok(BlockFetchMessage::MsgClientDone),
        TAG_START_BATCH => Ok(BlockFetchMessage::MsgStartBatch),
        TAG_NO_BLOCKS => Ok(BlockFetchMessage::MsgNoBlocks),
        TAG_BLOCK => {
            // Haskell always sends: tag(24) bstr(stored_block_cbor)
            //
            // The `Serialise` instance for `Serialised a` in ouroboros-network:
            //   encode (Serialised bs) = encodeTag 24 <> encodeBytes bs
            //   decode = decodeTag (must be 24) <> Serialised <$> decodeBytes
            //
            // We unwrap tag(24) and return the raw stored-CBOR bytes
            // [era_word, block_body] as the MsgBlock payload.
            //
            // For robustness we also handle the legacy raw-bytes case
            // (direct bstr without tag), which may appear from older Torsten
            // peers or test harnesses.
            let block = match dec.datatype().map_err(|e| e.to_string())? {
                minicbor::data::Type::Tag => {
                    let tag = dec.tag().map_err(|e| e.to_string())?;
                    if tag.as_u64() != 24 {
                        return Err(format!(
                            "BlockFetch MsgBlock: expected tag(24) (CBOR-in-CBOR), got tag({})",
                            tag.as_u64()
                        ));
                    }
                    dec.bytes().map_err(|e| e.to_string())?.to_vec()
                }
                // Fallback: raw bytes without tag (legacy / test peers).
                minicbor::data::Type::Bytes | minicbor::data::Type::BytesIndef => {
                    dec.bytes().map_err(|e| e.to_string())?.to_vec()
                }
                other => {
                    return Err(format!(
                        "BlockFetch MsgBlock: expected tag(24) or bstr, got {other:?}"
                    ));
                }
            };
            Ok(BlockFetchMessage::MsgBlock(block))
        }
        TAG_BATCH_DONE => Ok(BlockFetchMessage::MsgBatchDone),
        _ => Err(format!("unknown BlockFetch message tag: {tag}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msg_request_range_roundtrip() {
        let msg = BlockFetchMessage::MsgRequestRange {
            from: Point::Specific(100, [0xAA; 32]),
            to: Point::Specific(200, [0xBB; 32]),
        };
        let encoded = encode_message(&msg);
        let decoded = decode_message(&encoded).unwrap();
        if let BlockFetchMessage::MsgRequestRange { from, to } = decoded {
            assert_eq!(from, Point::Specific(100, [0xAA; 32]));
            assert_eq!(to, Point::Specific(200, [0xBB; 32]));
        } else {
            panic!("expected MsgRequestRange");
        }
    }

    #[test]
    fn msg_client_done_roundtrip() {
        let encoded = encode_message(&BlockFetchMessage::MsgClientDone);
        let decoded = decode_message(&encoded).unwrap();
        assert!(matches!(decoded, BlockFetchMessage::MsgClientDone));
    }

    #[test]
    fn msg_start_batch_roundtrip() {
        let encoded = encode_message(&BlockFetchMessage::MsgStartBatch);
        let decoded = decode_message(&encoded).unwrap();
        assert!(matches!(decoded, BlockFetchMessage::MsgStartBatch));
    }

    #[test]
    fn msg_no_blocks_roundtrip() {
        let encoded = encode_message(&BlockFetchMessage::MsgNoBlocks);
        let decoded = decode_message(&encoded).unwrap();
        assert!(matches!(decoded, BlockFetchMessage::MsgNoBlocks));
    }

    #[test]
    fn msg_block_roundtrip() {
        let block = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let encoded = encode_message(&BlockFetchMessage::MsgBlock(block.clone()));
        let decoded = decode_message(&encoded).unwrap();
        if let BlockFetchMessage::MsgBlock(b) = decoded {
            assert_eq!(b, block);
        } else {
            panic!("expected MsgBlock");
        }
    }

    #[test]
    fn msg_batch_done_roundtrip() {
        let encoded = encode_message(&BlockFetchMessage::MsgBatchDone);
        let decoded = decode_message(&encoded).unwrap();
        assert!(matches!(decoded, BlockFetchMessage::MsgBatchDone));
    }

    #[test]
    fn msg_client_done_wire_format() {
        // [1] = 0x81 0x01
        let encoded = encode_message(&BlockFetchMessage::MsgClientDone);
        assert_eq!(encoded, vec![0x81, 0x01]);
    }
}
