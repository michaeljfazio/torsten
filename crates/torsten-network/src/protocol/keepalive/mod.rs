//! KeepAlive mini-protocol codec and message types.
//!
//! The simplest Ouroboros mini-protocol — proves the pattern for all others.
//!
//! ## State machine
//! ```text
//! StClient ──MsgKeepAlive(cookie)──► StServer
//! StServer ──MsgKeepAliveResponse(cookie)──► StClient
//! StClient ──MsgDone──► StDone
//! ```
//!
//! ## Wire format (CBOR)
//! - `MsgKeepAlive` = `[0, cookie]`  (cookie is u16)
//! - `MsgKeepAliveResponse` = `[1, cookie]`
//! - `MsgDone` = `[2]`

pub mod client;
pub mod server;

use minicbor::{Decoder, Encoder};

/// KeepAlive protocol state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepAliveState {
    /// Client has agency — can send MsgKeepAlive or MsgDone.
    StClient,
    /// Server has agency — must respond with MsgKeepAliveResponse.
    StServer,
    /// Terminal state — protocol is done.
    StDone,
}

/// KeepAlive protocol messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeepAliveMessage {
    /// Ping with a cookie value (client → server).
    MsgKeepAlive(u16),
    /// Pong echoing the cookie (server → client).
    MsgKeepAliveResponse(u16),
    /// Graceful shutdown (client → server).
    MsgDone,
}

// CBOR message tags
const TAG_KEEP_ALIVE: u64 = 0;
const TAG_KEEP_ALIVE_RESPONSE: u64 = 1;
const TAG_DONE: u64 = 2;

/// Encode a KeepAlive message as CBOR.
pub fn encode_message(msg: &KeepAliveMessage) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut enc = Encoder::new(&mut buf);
    match msg {
        KeepAliveMessage::MsgKeepAlive(cookie) => {
            enc.array(2).expect("infallible");
            enc.u64(TAG_KEEP_ALIVE).expect("infallible");
            enc.u16(*cookie).expect("infallible");
        }
        KeepAliveMessage::MsgKeepAliveResponse(cookie) => {
            enc.array(2).expect("infallible");
            enc.u64(TAG_KEEP_ALIVE_RESPONSE).expect("infallible");
            enc.u16(*cookie).expect("infallible");
        }
        KeepAliveMessage::MsgDone => {
            enc.array(1).expect("infallible");
            enc.u64(TAG_DONE).expect("infallible");
        }
    }
    buf
}

/// Decode a KeepAlive message from CBOR bytes.
pub fn decode_message(data: &[u8]) -> Result<KeepAliveMessage, String> {
    let mut dec = Decoder::new(data);
    let _arr_len = dec.array().map_err(|e| e.to_string())?;
    let tag = dec.u64().map_err(|e| e.to_string())?;
    match tag {
        TAG_KEEP_ALIVE => {
            let cookie = dec.u16().map_err(|e| e.to_string())?;
            Ok(KeepAliveMessage::MsgKeepAlive(cookie))
        }
        TAG_KEEP_ALIVE_RESPONSE => {
            let cookie = dec.u16().map_err(|e| e.to_string())?;
            Ok(KeepAliveMessage::MsgKeepAliveResponse(cookie))
        }
        TAG_DONE => Ok(KeepAliveMessage::MsgDone),
        _ => Err(format!("unknown KeepAlive message tag: {tag}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msg_keep_alive_roundtrip() {
        let msg = KeepAliveMessage::MsgKeepAlive(42);
        let encoded = encode_message(&msg);
        let decoded = decode_message(&encoded).unwrap();
        assert_eq!(msg, decoded);
    }

    #[test]
    fn msg_keep_alive_response_roundtrip() {
        let msg = KeepAliveMessage::MsgKeepAliveResponse(1234);
        let encoded = encode_message(&msg);
        let decoded = decode_message(&encoded).unwrap();
        assert_eq!(msg, decoded);
    }

    #[test]
    fn msg_done_roundtrip() {
        let msg = KeepAliveMessage::MsgDone;
        let encoded = encode_message(&msg);
        let decoded = decode_message(&encoded).unwrap();
        assert_eq!(msg, decoded);
    }

    #[test]
    fn msg_keep_alive_wire_format() {
        // MsgKeepAlive(0) should be: [0, 0] = 0x82 0x00 0x00
        let encoded = encode_message(&KeepAliveMessage::MsgKeepAlive(0));
        assert_eq!(encoded, vec![0x82, 0x00, 0x00]);
    }

    #[test]
    fn msg_done_wire_format() {
        // MsgDone = [2] = 0x81 0x02
        let encoded = encode_message(&KeepAliveMessage::MsgDone);
        assert_eq!(encoded, vec![0x81, 0x02]);
    }

    #[test]
    fn max_cookie_value() {
        let msg = KeepAliveMessage::MsgKeepAlive(u16::MAX);
        let encoded = encode_message(&msg);
        let decoded = decode_message(&encoded).unwrap();
        assert_eq!(decoded, KeepAliveMessage::MsgKeepAlive(u16::MAX));
    }
}
