//! KeepAlive server — echoes cookie values back to the client.
//!
//! Receives `MsgKeepAlive(cookie)` and responds with `MsgKeepAliveResponse(cookie)`.
//! Exits cleanly on `MsgDone`.

use crate::error::ProtocolError;
use crate::mux::channel::MuxChannel;

use super::{decode_message, encode_message, KeepAliveMessage};

/// KeepAlive server that echoes ping cookies.
pub struct KeepAliveServer;

impl KeepAliveServer {
    /// Run the keepalive server loop.
    ///
    /// Receives messages from the client and responds:
    /// - `MsgKeepAlive(cookie)` → reply with `MsgKeepAliveResponse(cookie)`
    /// - `MsgDone` → exit cleanly
    ///
    /// Returns `Ok(ping_count)` on clean shutdown (MsgDone received).
    pub async fn run(channel: &mut MuxChannel) -> Result<u64, ProtocolError> {
        let mut ping_count: u64 = 0;

        loop {
            let msg_bytes = channel.recv().await.map_err(ProtocolError::from)?;
            let msg = decode_message(&msg_bytes).map_err(|e| ProtocolError::CborDecode {
                protocol: "KeepAlive",
                reason: e,
            })?;

            match msg {
                KeepAliveMessage::MsgKeepAlive(cookie) => {
                    // Echo the cookie back
                    let response = encode_message(&KeepAliveMessage::MsgKeepAliveResponse(cookie));
                    channel.send(response).await.map_err(ProtocolError::from)?;
                    ping_count += 1;
                    tracing::debug!(cookie, ping_count, "keepalive: echoed ping");
                }
                KeepAliveMessage::MsgDone => {
                    tracing::debug!(ping_count, "keepalive: client sent MsgDone");
                    return Ok(ping_count);
                }
                KeepAliveMessage::MsgKeepAliveResponse(cookie) => {
                    // Server should never receive a response — agency violation
                    return Err(ProtocolError::AgencyViolation {
                        protocol: "KeepAlive",
                        state: "StServer".to_string(),
                        received_tag: cookie as u8,
                    });
                }
            }
        }
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
        let (egress_tx, egress_rx) = mpsc::channel(32);
        let (ingress_tx, ingress_rx) = mpsc::channel(32);
        let channel = MuxChannel::new(
            8,
            crate::mux::Direction::ResponderDir,
            egress_tx,
            ingress_rx,
            65536,
            std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        );
        (channel, egress_rx, ingress_tx)
    }

    #[tokio::test]
    async fn echoes_cookie_and_exits_on_done() {
        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();

        // Spawn the server
        let handle = tokio::spawn(async move { KeepAliveServer::run(&mut channel).await });

        // Send MsgKeepAlive(42)
        let ping = encode_message(&KeepAliveMessage::MsgKeepAlive(42));
        ingress_tx.send(Bytes::from(ping)).await.unwrap();

        // Read the response
        let (_, _, response_bytes) = egress_rx.recv().await.unwrap();
        let response = decode_message(&response_bytes).unwrap();
        assert_eq!(response, KeepAliveMessage::MsgKeepAliveResponse(42));

        // Send MsgDone
        let done = encode_message(&KeepAliveMessage::MsgDone);
        ingress_tx.send(Bytes::from(done)).await.unwrap();

        let result = handle.await.unwrap();
        assert_eq!(result.unwrap(), 1); // 1 ping echoed
    }

    #[tokio::test]
    async fn multiple_pings() {
        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();

        let handle = tokio::spawn(async move { KeepAliveServer::run(&mut channel).await });

        // Send 3 pings
        for cookie in 0..3u16 {
            let ping = encode_message(&KeepAliveMessage::MsgKeepAlive(cookie));
            ingress_tx.send(Bytes::from(ping)).await.unwrap();
            let (_, _, resp) = egress_rx.recv().await.unwrap();
            let response = decode_message(&resp).unwrap();
            assert_eq!(response, KeepAliveMessage::MsgKeepAliveResponse(cookie));
        }

        // Send MsgDone
        let done = encode_message(&KeepAliveMessage::MsgDone);
        ingress_tx.send(Bytes::from(done)).await.unwrap();

        let result = handle.await.unwrap();
        assert_eq!(result.unwrap(), 3);
    }
}
