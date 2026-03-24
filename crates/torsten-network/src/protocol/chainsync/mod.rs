//! ChainSync mini-protocol codec and message types.
//!
//! ChainSync is the core synchronization protocol — the client follows the server's
//! chain by receiving headers (N2N) or full blocks (N2C).
//!
//! ## State machine
//! ```text
//! StIdle ──MsgRequestNext──► StCanAwait
//! StCanAwait ──MsgRollForward(header, tip)──► StIdle
//! StCanAwait ──MsgRollBackward(point, tip)──► StIdle
//! StCanAwait ──MsgAwaitReply──► StMustReply
//! StMustReply ──MsgRollForward(header, tip)──► StIdle
//! StMustReply ──MsgRollBackward(point, tip)──► StIdle
//! StIdle ──MsgFindIntersect([points])──► StIntersect
//! StIntersect ──MsgIntersectFound(point, tip)──► StIdle
//! StIntersect ──MsgIntersectNotFound(tip)──► StIdle
//! StIdle ──MsgDone──► StDone
//! ```
//!
//! ## Wire format (CBOR)
//! - `MsgRequestNext` = `[0]`
//! - `MsgAwaitReply` = `[1]`
//! - `MsgRollForward` = `[2, header, tip]`
//! - `MsgRollBackward` = `[3, point, tip]`
//! - `MsgFindIntersect` = `[4, [*point]]`
//! - `MsgIntersectFound` = `[5, point, tip]`
//! - `MsgIntersectNotFound` = `[6, tip]`
//! - `MsgDone` = `[7]`

pub mod client;
pub mod server;

use crate::codec::{self, Point};
use minicbor::{Decoder, Encoder};

/// ChainSync protocol state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainSyncState {
    /// Client has agency — can send MsgRequestNext, MsgFindIntersect, or MsgDone.
    StIdle,
    /// Server has agency — can reply immediately or send MsgAwaitReply.
    StCanAwait,
    /// Server must reply (at tip, waiting for new block).
    StMustReply,
    /// Server has agency after MsgFindIntersect.
    StIntersect,
    /// Terminal state.
    StDone,
}

/// ChainSync protocol messages.
#[derive(Debug, Clone)]
pub enum ChainSyncMessage {
    /// Client requests the next header/block (tag 0).
    MsgRequestNext,
    /// Server signals it's at the tip, will reply when a new block arrives (tag 1).
    MsgAwaitReply,
    /// Server sends a new header/block rolling the chain forward (tag 2).
    /// Contains raw header CBOR and tip info (slot, hash, block_number).
    MsgRollForward {
        header: Vec<u8>,
        tip_slot: u64,
        tip_hash: [u8; 32],
        tip_block_number: u64,
    },
    /// Server rolls the chain backward to a previous point (tag 3).
    MsgRollBackward {
        point: Point,
        tip_slot: u64,
        tip_hash: [u8; 32],
        tip_block_number: u64,
    },
    /// Client requests intersection with a list of known points (tag 4).
    MsgFindIntersect(Vec<Point>),
    /// Server found an intersection point (tag 5).
    MsgIntersectFound {
        point: Point,
        tip_slot: u64,
        tip_hash: [u8; 32],
        tip_block_number: u64,
    },
    /// Server could not find any intersection (tag 6).
    MsgIntersectNotFound {
        tip_slot: u64,
        tip_hash: [u8; 32],
        tip_block_number: u64,
    },
    /// Client terminates the protocol (tag 7).
    MsgDone,
}

// CBOR message tags
const TAG_REQUEST_NEXT: u64 = 0;
const TAG_AWAIT_REPLY: u64 = 1;
const TAG_ROLL_FORWARD: u64 = 2;
const TAG_ROLL_BACKWARD: u64 = 3;
const TAG_FIND_INTERSECT: u64 = 4;
const TAG_INTERSECT_FOUND: u64 = 5;
const TAG_INTERSECT_NOT_FOUND: u64 = 6;
const TAG_DONE: u64 = 7;

/// Encode a ChainSync message as CBOR.
pub fn encode_message(msg: &ChainSyncMessage) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut enc = Encoder::new(&mut buf);
    match msg {
        ChainSyncMessage::MsgRequestNext => {
            enc.array(1).expect("infallible");
            enc.u64(TAG_REQUEST_NEXT).expect("infallible");
        }
        ChainSyncMessage::MsgAwaitReply => {
            enc.array(1).expect("infallible");
            enc.u64(TAG_AWAIT_REPLY).expect("infallible");
        }
        ChainSyncMessage::MsgRollForward {
            header,
            tip_slot,
            tip_hash,
            tip_block_number,
        } => {
            enc.array(3).expect("infallible");
            enc.u64(TAG_ROLL_FORWARD).expect("infallible");
            // Header is raw CBOR — write it as a bytes value
            enc.bytes(header).expect("infallible");
            codec::encode_tip(&mut enc, *tip_slot, tip_hash, *tip_block_number);
        }
        ChainSyncMessage::MsgRollBackward {
            point,
            tip_slot,
            tip_hash,
            tip_block_number,
        } => {
            enc.array(3).expect("infallible");
            enc.u64(TAG_ROLL_BACKWARD).expect("infallible");
            codec::encode_point(&mut enc, point);
            codec::encode_tip(&mut enc, *tip_slot, tip_hash, *tip_block_number);
        }
        ChainSyncMessage::MsgFindIntersect(points) => {
            enc.array(2).expect("infallible");
            enc.u64(TAG_FIND_INTERSECT).expect("infallible");
            enc.array(points.len() as u64).expect("infallible");
            for p in points {
                codec::encode_point(&mut enc, p);
            }
        }
        ChainSyncMessage::MsgIntersectFound {
            point,
            tip_slot,
            tip_hash,
            tip_block_number,
        } => {
            enc.array(3).expect("infallible");
            enc.u64(TAG_INTERSECT_FOUND).expect("infallible");
            codec::encode_point(&mut enc, point);
            codec::encode_tip(&mut enc, *tip_slot, tip_hash, *tip_block_number);
        }
        ChainSyncMessage::MsgIntersectNotFound {
            tip_slot,
            tip_hash,
            tip_block_number,
        } => {
            enc.array(2).expect("infallible");
            enc.u64(TAG_INTERSECT_NOT_FOUND).expect("infallible");
            codec::encode_tip(&mut enc, *tip_slot, tip_hash, *tip_block_number);
        }
        ChainSyncMessage::MsgDone => {
            enc.array(1).expect("infallible");
            enc.u64(TAG_DONE).expect("infallible");
        }
    }
    buf
}

/// Decode a ChainSync message from CBOR bytes.
pub fn decode_message(data: &[u8]) -> Result<ChainSyncMessage, String> {
    let mut dec = Decoder::new(data);
    let _arr_len = dec.array().map_err(|e| e.to_string())?;
    let tag = dec.u64().map_err(|e| e.to_string())?;

    match tag {
        TAG_REQUEST_NEXT => Ok(ChainSyncMessage::MsgRequestNext),
        TAG_AWAIT_REPLY => Ok(ChainSyncMessage::MsgAwaitReply),
        TAG_ROLL_FORWARD => {
            // The header is HFC-wrapped: [era_id, CBOR_tag_24(header_bytes)]
            // We need to handle both the HFC-wrapped format (from Haskell nodes)
            // and raw bytes (from Torsten-to-Torsten connections).
            //
            // Haskell sends: [2, [era_id, tag24(header_cbor)], tip]
            // We extract the raw header bytes from inside the tag24 wrapper.
            let header = match dec.datatype().map_err(|e| e.to_string())? {
                minicbor::data::Type::Bytes | minicbor::data::Type::BytesIndef => {
                    // Raw bytes (Torsten-to-Torsten or simple encoding)
                    dec.bytes().map_err(|e| e.to_string())?.to_vec()
                }
                minicbor::data::Type::Array => {
                    // HFC-wrapped: [era_id, tag24(header_bytes)]
                    let _arr = dec.array().map_err(|e| e.to_string())?;
                    let _era_id = dec.u64().map_err(|e| e.to_string())?;
                    // The header is typically wrapped in CBOR tag 24 (embedded CBOR)
                    match dec.datatype().map_err(|e| e.to_string())? {
                        minicbor::data::Type::Tag => {
                            let tag = dec.tag().map_err(|e| e.to_string())?;
                            if tag.as_u64() == 24 {
                                // tag24(header_bytes) — extract the inner bytes
                                dec.bytes().map_err(|e| e.to_string())?.to_vec()
                            } else {
                                // Unknown tag — try to read as bytes
                                dec.bytes().map_err(|e| e.to_string())?.to_vec()
                            }
                        }
                        minicbor::data::Type::Bytes | minicbor::data::Type::BytesIndef => {
                            // Plain bytes inside the array (no tag24 wrapper)
                            dec.bytes().map_err(|e| e.to_string())?.to_vec()
                        }
                        other => {
                            return Err(format!(
                                "ChainSync RollForward: unexpected type {other:?} inside HFC wrapper"
                            ));
                        }
                    }
                }
                other => {
                    return Err(format!(
                        "ChainSync RollForward: unexpected header type {other:?}, expected bytes or array"
                    ));
                }
            };
            let (tip_slot, tip_hash, tip_block_number) =
                codec::decode_tip(&mut dec).map_err(|e| e.to_string())?;
            Ok(ChainSyncMessage::MsgRollForward {
                header,
                tip_slot,
                tip_hash,
                tip_block_number,
            })
        }
        TAG_ROLL_BACKWARD => {
            let point = codec::decode_point(&mut dec).map_err(|e| e.to_string())?;
            let (tip_slot, tip_hash, tip_block_number) =
                codec::decode_tip(&mut dec).map_err(|e| e.to_string())?;
            Ok(ChainSyncMessage::MsgRollBackward {
                point,
                tip_slot,
                tip_hash,
                tip_block_number,
            })
        }
        TAG_FIND_INTERSECT => {
            let arr_len = dec
                .array()
                .map_err(|e| e.to_string())?
                .ok_or("indefinite array not supported")?;
            let mut points = Vec::with_capacity(arr_len as usize);
            for _ in 0..arr_len {
                points.push(codec::decode_point(&mut dec).map_err(|e| e.to_string())?);
            }
            Ok(ChainSyncMessage::MsgFindIntersect(points))
        }
        TAG_INTERSECT_FOUND => {
            let point = codec::decode_point(&mut dec).map_err(|e| e.to_string())?;
            let (tip_slot, tip_hash, tip_block_number) =
                codec::decode_tip(&mut dec).map_err(|e| e.to_string())?;
            Ok(ChainSyncMessage::MsgIntersectFound {
                point,
                tip_slot,
                tip_hash,
                tip_block_number,
            })
        }
        TAG_INTERSECT_NOT_FOUND => {
            let (tip_slot, tip_hash, tip_block_number) =
                codec::decode_tip(&mut dec).map_err(|e| e.to_string())?;
            Ok(ChainSyncMessage::MsgIntersectNotFound {
                tip_slot,
                tip_hash,
                tip_block_number,
            })
        }
        TAG_DONE => Ok(ChainSyncMessage::MsgDone),
        _ => Err(format!("unknown ChainSync message tag: {tag}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msg_request_next_roundtrip() {
        let msg = ChainSyncMessage::MsgRequestNext;
        let encoded = encode_message(&msg);
        let decoded = decode_message(&encoded).unwrap();
        assert!(matches!(decoded, ChainSyncMessage::MsgRequestNext));
    }

    #[test]
    fn msg_request_next_wire_format() {
        // [0] = 0x81 0x00
        let encoded = encode_message(&ChainSyncMessage::MsgRequestNext);
        assert_eq!(encoded, vec![0x81, 0x00]);
    }

    #[test]
    fn msg_await_reply_roundtrip() {
        let encoded = encode_message(&ChainSyncMessage::MsgAwaitReply);
        let decoded = decode_message(&encoded).unwrap();
        assert!(matches!(decoded, ChainSyncMessage::MsgAwaitReply));
    }

    #[test]
    fn msg_roll_forward_roundtrip() {
        let msg = ChainSyncMessage::MsgRollForward {
            header: vec![0xDE, 0xAD],
            tip_slot: 100,
            tip_hash: [0xAB; 32],
            tip_block_number: 50,
        };
        let encoded = encode_message(&msg);
        let decoded = decode_message(&encoded).unwrap();
        if let ChainSyncMessage::MsgRollForward {
            header,
            tip_slot,
            tip_hash,
            tip_block_number,
        } = decoded
        {
            assert_eq!(header, vec![0xDE, 0xAD]);
            assert_eq!(tip_slot, 100);
            assert_eq!(tip_hash, [0xAB; 32]);
            assert_eq!(tip_block_number, 50);
        } else {
            panic!("expected MsgRollForward");
        }
    }

    #[test]
    fn msg_roll_backward_with_origin() {
        let msg = ChainSyncMessage::MsgRollBackward {
            point: Point::Origin,
            tip_slot: 0,
            tip_hash: [0; 32],
            tip_block_number: 0,
        };
        let encoded = encode_message(&msg);
        let decoded = decode_message(&encoded).unwrap();
        if let ChainSyncMessage::MsgRollBackward { point, .. } = decoded {
            assert_eq!(point, Point::Origin);
        } else {
            panic!("expected MsgRollBackward");
        }
    }

    #[test]
    fn msg_find_intersect_roundtrip() {
        let points = vec![
            Point::Specific(1000, [0x11; 32]),
            Point::Specific(500, [0x22; 32]),
            Point::Origin,
        ];
        let msg = ChainSyncMessage::MsgFindIntersect(points.clone());
        let encoded = encode_message(&msg);
        let decoded = decode_message(&encoded).unwrap();
        if let ChainSyncMessage::MsgFindIntersect(decoded_points) = decoded {
            assert_eq!(decoded_points, points);
        } else {
            panic!("expected MsgFindIntersect");
        }
    }

    #[test]
    fn msg_intersect_found_roundtrip() {
        let msg = ChainSyncMessage::MsgIntersectFound {
            point: Point::Specific(42, [0xCC; 32]),
            tip_slot: 100,
            tip_hash: [0xDD; 32],
            tip_block_number: 50,
        };
        let encoded = encode_message(&msg);
        let decoded = decode_message(&encoded).unwrap();
        if let ChainSyncMessage::MsgIntersectFound {
            point, tip_slot, ..
        } = decoded
        {
            assert_eq!(point, Point::Specific(42, [0xCC; 32]));
            assert_eq!(tip_slot, 100);
        } else {
            panic!("expected MsgIntersectFound");
        }
    }

    #[test]
    fn msg_intersect_not_found_roundtrip() {
        let msg = ChainSyncMessage::MsgIntersectNotFound {
            tip_slot: 200,
            tip_hash: [0xEE; 32],
            tip_block_number: 100,
        };
        let encoded = encode_message(&msg);
        let decoded = decode_message(&encoded).unwrap();
        if let ChainSyncMessage::MsgIntersectNotFound {
            tip_slot,
            tip_block_number,
            ..
        } = decoded
        {
            assert_eq!(tip_slot, 200);
            assert_eq!(tip_block_number, 100);
        } else {
            panic!("expected MsgIntersectNotFound");
        }
    }

    #[test]
    fn msg_done_roundtrip() {
        let encoded = encode_message(&ChainSyncMessage::MsgDone);
        let decoded = decode_message(&encoded).unwrap();
        assert!(matches!(decoded, ChainSyncMessage::MsgDone));
    }
}
