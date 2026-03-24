//! TxSubmission2 mini-protocol codec and message types.
//!
//! TxSubmission2 is a pull-based protocol with **inverted agency**: the server
//! (not the client) has agency in the Idle state and requests transaction IDs
//! and bodies from the client.
//!
//! ## State machine
//! ```text
//! StInit ──MsgInit──► StIdle
//! StIdle ──MsgRequestTxIds(blocking, ack, req)──► StTxIds
//! StTxIds ──MsgReplyTxIds([tx_id])──► StIdle
//! StIdle ──MsgRequestTxs([tx_id])──► StTxs
//! StTxs ──MsgReplyTxs([tx])──► StIdle
//! StTxIds(blocking) ──MsgDone──► StDone  (only when blocking and no txs)
//! ```
//!
//! ## Wire format (CBOR)
//! - `MsgRequestTxIds` = `[0, blocking, ack_count, req_count]`
//! - `MsgReplyTxIds` = `[1, [[tx_id, size_in_bytes]]]`
//! - `MsgRequestTxs` = `[2, [tx_id]]`
//! - `MsgReplyTxs` = `[3, [tx_bytes]]`
//! - `MsgDone` = `[4]`
//! - `MsgInit` = `[6]`

pub mod client;
pub mod server;

use minicbor::{Decoder, Encoder};

/// TxSubmission2 protocol state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxSubmissionState {
    /// Initial state — client sends MsgInit.
    StInit,
    /// Server has agency — requests tx IDs or txs.
    StIdle,
    /// Client must reply with tx IDs (blocking or non-blocking).
    StTxIds { blocking: bool },
    /// Client must reply with tx bodies.
    StTxs,
    /// Terminal state.
    StDone,
}

/// A transaction ID with its size (for flow control).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxIdAndSize {
    /// Transaction hash (32 bytes).
    pub tx_id: [u8; 32],
    /// Transaction size in bytes (used by the server for flow control).
    pub size_in_bytes: u32,
}

/// TxSubmission2 protocol messages.
#[derive(Debug, Clone)]
pub enum TxSubmissionMessage {
    /// Protocol initialization (client → server, tag 6).
    MsgInit,
    /// Server requests transaction IDs from the client (tag 0).
    MsgRequestTxIds {
        /// If true, the client must block until it has txs. If false, may return empty.
        blocking: bool,
        /// Number of previously received tx IDs to acknowledge (FIFO).
        ack_count: u16,
        /// Maximum number of tx IDs to return.
        req_count: u16,
    },
    /// Client replies with transaction IDs and sizes (tag 1).
    MsgReplyTxIds(Vec<TxIdAndSize>),
    /// Server requests full transaction bodies by ID (tag 2).
    MsgRequestTxs(Vec<[u8; 32]>),
    /// Client replies with full transaction CBOR bodies (tag 3).
    MsgReplyTxs(Vec<Vec<u8>>),
    /// Terminate the protocol (tag 4). Only valid in blocking StTxIds with no txs.
    MsgDone,
}

// CBOR message tags
const TAG_REQUEST_TX_IDS: u64 = 0;
const TAG_REPLY_TX_IDS: u64 = 1;
const TAG_REQUEST_TXS: u64 = 2;
const TAG_REPLY_TXS: u64 = 3;
const TAG_DONE: u64 = 4;
const TAG_INIT: u64 = 6;

/// Encode a TxSubmission2 message as CBOR.
pub fn encode_message(msg: &TxSubmissionMessage) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut enc = Encoder::new(&mut buf);
    match msg {
        TxSubmissionMessage::MsgInit => {
            enc.array(1).expect("infallible");
            enc.u64(TAG_INIT).expect("infallible");
        }
        TxSubmissionMessage::MsgRequestTxIds {
            blocking,
            ack_count,
            req_count,
        } => {
            enc.array(4).expect("infallible");
            enc.u64(TAG_REQUEST_TX_IDS).expect("infallible");
            enc.bool(*blocking).expect("infallible");
            enc.u16(*ack_count).expect("infallible");
            enc.u16(*req_count).expect("infallible");
        }
        TxSubmissionMessage::MsgReplyTxIds(ids) => {
            enc.array(2).expect("infallible");
            enc.u64(TAG_REPLY_TX_IDS).expect("infallible");
            enc.array(ids.len() as u64).expect("infallible");
            for id in ids {
                enc.array(2).expect("infallible");
                enc.bytes(&id.tx_id).expect("infallible");
                enc.u32(id.size_in_bytes).expect("infallible");
            }
        }
        TxSubmissionMessage::MsgRequestTxs(ids) => {
            enc.array(2).expect("infallible");
            enc.u64(TAG_REQUEST_TXS).expect("infallible");
            enc.array(ids.len() as u64).expect("infallible");
            for id in ids {
                enc.bytes(id).expect("infallible");
            }
        }
        TxSubmissionMessage::MsgReplyTxs(txs) => {
            enc.array(2).expect("infallible");
            enc.u64(TAG_REPLY_TXS).expect("infallible");
            enc.array(txs.len() as u64).expect("infallible");
            for tx in txs {
                enc.bytes(tx).expect("infallible");
            }
        }
        TxSubmissionMessage::MsgDone => {
            enc.array(1).expect("infallible");
            enc.u64(TAG_DONE).expect("infallible");
        }
    }
    buf
}

/// Decode a TxSubmission2 message from CBOR bytes.
pub fn decode_message(data: &[u8]) -> Result<TxSubmissionMessage, String> {
    let mut dec = Decoder::new(data);
    let _arr_len = dec.array().map_err(|e| e.to_string())?;
    let tag = dec.u64().map_err(|e| e.to_string())?;

    match tag {
        TAG_INIT => Ok(TxSubmissionMessage::MsgInit),
        TAG_REQUEST_TX_IDS => {
            let blocking = dec.bool().map_err(|e| e.to_string())?;
            let ack_count = dec.u16().map_err(|e| e.to_string())?;
            let req_count = dec.u16().map_err(|e| e.to_string())?;
            Ok(TxSubmissionMessage::MsgRequestTxIds {
                blocking,
                ack_count,
                req_count,
            })
        }
        TAG_REPLY_TX_IDS => {
            let len = dec
                .array()
                .map_err(|e| e.to_string())?
                .ok_or("indefinite array not supported")?;
            let mut ids = Vec::with_capacity(len as usize);
            for _ in 0..len {
                dec.array().map_err(|e| e.to_string())?;
                let tx_id_bytes = dec.bytes().map_err(|e| e.to_string())?;
                if tx_id_bytes.len() != 32 {
                    return Err(format!("tx_id must be 32 bytes, got {}", tx_id_bytes.len()));
                }
                let mut tx_id = [0u8; 32];
                tx_id.copy_from_slice(tx_id_bytes);
                let size = dec.u32().map_err(|e| e.to_string())?;
                ids.push(TxIdAndSize {
                    tx_id,
                    size_in_bytes: size,
                });
            }
            Ok(TxSubmissionMessage::MsgReplyTxIds(ids))
        }
        TAG_REQUEST_TXS => {
            let len = dec
                .array()
                .map_err(|e| e.to_string())?
                .ok_or("indefinite array not supported")?;
            let mut ids = Vec::with_capacity(len as usize);
            for _ in 0..len {
                let id_bytes = dec.bytes().map_err(|e| e.to_string())?;
                if id_bytes.len() != 32 {
                    return Err(format!("tx_id must be 32 bytes, got {}", id_bytes.len()));
                }
                let mut id = [0u8; 32];
                id.copy_from_slice(id_bytes);
                ids.push(id);
            }
            Ok(TxSubmissionMessage::MsgRequestTxs(ids))
        }
        TAG_REPLY_TXS => {
            let len = dec
                .array()
                .map_err(|e| e.to_string())?
                .ok_or("indefinite array not supported")?;
            let mut txs = Vec::with_capacity(len as usize);
            for _ in 0..len {
                let tx = dec.bytes().map_err(|e| e.to_string())?.to_vec();
                txs.push(tx);
            }
            Ok(TxSubmissionMessage::MsgReplyTxs(txs))
        }
        TAG_DONE => Ok(TxSubmissionMessage::MsgDone),
        _ => Err(format!("unknown TxSubmission2 message tag: {tag}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msg_init_roundtrip() {
        let encoded = encode_message(&TxSubmissionMessage::MsgInit);
        let decoded = decode_message(&encoded).unwrap();
        assert!(matches!(decoded, TxSubmissionMessage::MsgInit));
        // Wire format: [6] = 0x81 0x06
        assert_eq!(encoded, vec![0x81, 0x06]);
    }

    #[test]
    fn msg_request_tx_ids_roundtrip() {
        let msg = TxSubmissionMessage::MsgRequestTxIds {
            blocking: false,
            ack_count: 0,
            req_count: 10,
        };
        let encoded = encode_message(&msg);
        let decoded = decode_message(&encoded).unwrap();
        if let TxSubmissionMessage::MsgRequestTxIds {
            blocking,
            ack_count,
            req_count,
        } = decoded
        {
            assert!(!blocking);
            assert_eq!(ack_count, 0);
            assert_eq!(req_count, 10);
        } else {
            panic!("expected MsgRequestTxIds");
        }
    }

    #[test]
    fn msg_reply_tx_ids_roundtrip() {
        let msg = TxSubmissionMessage::MsgReplyTxIds(vec![
            TxIdAndSize {
                tx_id: [0xAA; 32],
                size_in_bytes: 256,
            },
            TxIdAndSize {
                tx_id: [0xBB; 32],
                size_in_bytes: 512,
            },
        ]);
        let encoded = encode_message(&msg);
        let decoded = decode_message(&encoded).unwrap();
        if let TxSubmissionMessage::MsgReplyTxIds(ids) = decoded {
            assert_eq!(ids.len(), 2);
            assert_eq!(ids[0].tx_id, [0xAA; 32]);
            assert_eq!(ids[0].size_in_bytes, 256);
            assert_eq!(ids[1].tx_id, [0xBB; 32]);
            assert_eq!(ids[1].size_in_bytes, 512);
        } else {
            panic!("expected MsgReplyTxIds");
        }
    }

    #[test]
    fn msg_request_txs_roundtrip() {
        let msg = TxSubmissionMessage::MsgRequestTxs(vec![[0xCC; 32], [0xDD; 32]]);
        let encoded = encode_message(&msg);
        let decoded = decode_message(&encoded).unwrap();
        if let TxSubmissionMessage::MsgRequestTxs(ids) = decoded {
            assert_eq!(ids, vec![[0xCC; 32], [0xDD; 32]]);
        } else {
            panic!("expected MsgRequestTxs");
        }
    }

    #[test]
    fn msg_reply_txs_roundtrip() {
        let msg = TxSubmissionMessage::MsgReplyTxs(vec![vec![0x01, 0x02, 0x03], vec![0x04, 0x05]]);
        let encoded = encode_message(&msg);
        let decoded = decode_message(&encoded).unwrap();
        if let TxSubmissionMessage::MsgReplyTxs(txs) = decoded {
            assert_eq!(txs, vec![vec![0x01, 0x02, 0x03], vec![0x04, 0x05]]);
        } else {
            panic!("expected MsgReplyTxs");
        }
    }

    #[test]
    fn msg_done_roundtrip() {
        let encoded = encode_message(&TxSubmissionMessage::MsgDone);
        let decoded = decode_message(&encoded).unwrap();
        assert!(matches!(decoded, TxSubmissionMessage::MsgDone));
        // Wire format: [4] = 0x81 0x04
        assert_eq!(encoded, vec![0x81, 0x04]);
    }

    #[test]
    fn first_request_must_be_non_blocking() {
        // The spec requires the first MsgRequestTxIds to be non-blocking with ack_count=0.
        // This is an encoding test — the server enforces this semantically.
        let msg = TxSubmissionMessage::MsgRequestTxIds {
            blocking: false,
            ack_count: 0,
            req_count: 3,
        };
        let encoded = encode_message(&msg);
        let decoded = decode_message(&encoded).unwrap();
        if let TxSubmissionMessage::MsgRequestTxIds {
            blocking,
            ack_count,
            ..
        } = decoded
        {
            assert!(!blocking, "first request must be non-blocking");
            assert_eq!(ack_count, 0, "first request must have ack_count=0");
        }
    }

    #[test]
    fn empty_reply_tx_ids() {
        let msg = TxSubmissionMessage::MsgReplyTxIds(vec![]);
        let encoded = encode_message(&msg);
        let decoded = decode_message(&encoded).unwrap();
        if let TxSubmissionMessage::MsgReplyTxIds(ids) = decoded {
            assert!(ids.is_empty());
        } else {
            panic!("expected MsgReplyTxIds");
        }
    }
}
