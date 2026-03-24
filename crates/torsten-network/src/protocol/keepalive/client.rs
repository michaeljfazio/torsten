//! KeepAlive client — periodic ping with RTT measurement.
//!
//! Sends `MsgKeepAlive(cookie)` at a configurable interval, receives
//! `MsgKeepAliveResponse(cookie)`, verifies cookie match, and measures
//! round-trip time. On cancellation, sends `MsgDone` for graceful shutdown.

use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

use crate::error::ProtocolError;
use crate::mux::channel::MuxChannel;

use super::{decode_message, encode_message, KeepAliveMessage};

/// Default keepalive ping interval.
pub const DEFAULT_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);

/// KeepAlive client that periodically pings the remote peer.
pub struct KeepAliveClient {
    /// Interval between pings.
    interval: Duration,
    /// Cancellation token for graceful shutdown.
    cancel: CancellationToken,
}

impl KeepAliveClient {
    /// Create a new KeepAlive client with the given ping interval.
    pub fn new(interval: Duration, cancel: CancellationToken) -> Self {
        Self { interval, cancel }
    }

    /// Run the keepalive loop. Returns the last measured RTT on clean shutdown.
    ///
    /// Sends `MsgKeepAlive(cookie)`, waits for `MsgKeepAliveResponse(cookie)`,
    /// verifies the cookie matches, measures RTT, then sleeps for `interval`.
    /// On cancellation, sends `MsgDone` and returns.
    pub async fn run(&self, channel: &mut MuxChannel) -> Result<Option<Duration>, ProtocolError> {
        let mut cookie: u16 = 0;
        let mut last_rtt: Option<Duration> = None;

        loop {
            // Check for cancellation before sending
            if self.cancel.is_cancelled() {
                // Send MsgDone for graceful shutdown
                let msg = encode_message(&KeepAliveMessage::MsgDone);
                channel.send(msg).await.map_err(ProtocolError::from)?;
                return Ok(last_rtt);
            }

            // Send MsgKeepAlive with current cookie
            let start = Instant::now();
            let ping = encode_message(&KeepAliveMessage::MsgKeepAlive(cookie));
            channel.send(ping).await.map_err(ProtocolError::from)?;

            // Wait for response or cancellation
            tokio::select! {
                result = channel.recv() => {
                    let response_bytes = result.map_err(ProtocolError::from)?;
                    let response = decode_message(&response_bytes).map_err(|e| {
                        ProtocolError::CborDecode {
                            protocol: "KeepAlive",
                            reason: e,
                        }
                    })?;

                    match response {
                        KeepAliveMessage::MsgKeepAliveResponse(response_cookie) => {
                            if response_cookie != cookie {
                                return Err(ProtocolError::InvalidMessage {
                                    protocol: "KeepAlive",
                                    tag: 1,
                                    reason: format!(
                                        "cookie mismatch: sent {cookie}, received {response_cookie}"
                                    ),
                                });
                            }
                            last_rtt = Some(start.elapsed());
                            tracing::debug!(
                                cookie,
                                rtt_ms = start.elapsed().as_millis(),
                                "keepalive: pong received"
                            );
                        }
                        other => {
                            return Err(ProtocolError::StateViolation {
                                protocol: "KeepAlive",
                                expected: "MsgKeepAliveResponse".to_string(),
                                actual: format!("{other:?}"),
                            });
                        }
                    }
                }
                _ = self.cancel.cancelled() => {
                    // Cancelled while waiting for response — send MsgDone
                    let msg = encode_message(&KeepAliveMessage::MsgDone);
                    let _ = channel.send(msg).await;
                    return Ok(last_rtt);
                }
            }

            // Increment cookie for next ping (wraps at u16::MAX)
            cookie = cookie.wrapping_add(1);

            // Sleep for the interval, or exit if cancelled
            tokio::select! {
                _ = tokio::time::sleep(self.interval) => {}
                _ = self.cancel.cancelled() => {
                    let msg = encode_message(&KeepAliveMessage::MsgDone);
                    let _ = channel.send(msg).await;
                    return Ok(last_rtt);
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

    /// Create a test MuxChannel with controlled ingress/egress.
    fn make_test_channel() -> (
        MuxChannel,
        mpsc::Receiver<(u16, crate::mux::Direction, Bytes)>,
        mpsc::Sender<Bytes>,
    ) {
        let (egress_tx, egress_rx) = mpsc::channel(32);
        let (ingress_tx, ingress_rx) = mpsc::channel(32);
        let channel = MuxChannel::new(
            8, // KeepAlive protocol ID
            crate::mux::Direction::InitiatorDir,
            egress_tx,
            ingress_rx,
            65536,
        );
        (channel, egress_rx, ingress_tx)
    }

    #[tokio::test]
    async fn ping_pong_with_matching_cookie() {
        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();
        let cancel = CancellationToken::new();
        let client = KeepAliveClient::new(Duration::from_millis(10), cancel.clone());

        // Spawn the client
        let cancel_clone = cancel.clone();
        let handle = tokio::spawn(async move { client.run(&mut channel).await });

        // Read the ping from egress
        let (_, _, ping_bytes) = egress_rx.recv().await.unwrap();
        let ping = decode_message(&ping_bytes).unwrap();
        assert_eq!(ping, KeepAliveMessage::MsgKeepAlive(0));

        // Send matching pong via ingress
        let pong = encode_message(&KeepAliveMessage::MsgKeepAliveResponse(0));
        ingress_tx.send(Bytes::from(pong)).await.unwrap();

        // Wait a bit then cancel
        tokio::time::sleep(Duration::from_millis(20)).await;
        cancel_clone.cancel();

        let result = handle.await.unwrap();
        assert!(result.is_ok());
        // Should have measured an RTT
        assert!(result.unwrap().is_some());
    }

    #[tokio::test]
    async fn cancelled_before_first_ping() {
        let (mut channel, _egress_rx, _ingress_tx) = make_test_channel();
        let cancel = CancellationToken::new();
        cancel.cancel(); // Cancel immediately

        let client = KeepAliveClient::new(Duration::from_secs(30), cancel);
        let result = client.run(&mut channel).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), None); // No RTT measured
    }
}
