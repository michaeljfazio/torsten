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
///
/// Must be well under the 30-second `SDU_READ_TIMEOUT` to ensure pings (and
/// their pong responses) keep the bearer alive on both sides of the connection
/// when the node is at tip and no ChainSync/BlockFetch data is flowing.
/// Haskell cardano-node uses ~10 seconds.
pub const DEFAULT_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(10);

/// Timeout for waiting for a pong response after sending a ping.
///
/// If the remote peer does not respond within this window, the attempt is
/// counted as a failure. After `MAX_CONSECUTIVE_FAILURES` consecutive
/// timeouts, the KeepAlive client returns `ProtocolError::KeepAliveTimeout`
/// so the connection can be closed and the peer demoted.
const KEEPALIVE_RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum consecutive pong timeouts before escalating to connection close.
///
/// After this many consecutive unresponded pings, the peer is considered
/// unresponsive and the KeepAlive client exits with an error. The connection
/// lifecycle manager should close the connection and record a failure.
const MAX_CONSECUTIVE_FAILURES: u32 = 3;

/// KeepAlive client that periodically pings the remote peer.
pub struct KeepAliveClient {
    /// Interval between pings.
    interval: Duration,
    /// Cancellation token for graceful shutdown.
    cancel: CancellationToken,
    /// Optional sender for reporting each RTT measurement (in milliseconds).
    /// Fires on every successful pong so the caller can track current peer latency.
    rtt_tx: Option<tokio::sync::mpsc::Sender<f64>>,
}

impl KeepAliveClient {
    /// Create a new KeepAlive client with the given ping interval.
    pub fn new(interval: Duration, cancel: CancellationToken) -> Self {
        Self {
            interval,
            cancel,
            rtt_tx: None,
        }
    }

    /// Attach an RTT reporting channel. Each successful pong will send the
    /// round-trip time in milliseconds on this channel (non-blocking).
    pub fn with_rtt_sender(mut self, tx: tokio::sync::mpsc::Sender<f64>) -> Self {
        self.rtt_tx = Some(tx);
        self
    }

    /// Run the keepalive loop. Returns the last measured RTT on clean shutdown.
    ///
    /// Sends `MsgKeepAlive(cookie)`, waits for `MsgKeepAliveResponse(cookie)`,
    /// verifies the cookie matches, measures RTT, then sleeps for `interval`.
    /// On cancellation, sends `MsgDone` and returns.
    pub async fn run(&self, channel: &mut MuxChannel) -> Result<Option<Duration>, ProtocolError> {
        let mut cookie: u16 = 0;
        let mut last_rtt: Option<Duration> = None;
        let mut consecutive_failures: u32 = 0;

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

            // Wait for response, cancellation, or timeout
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
                            // Successful pong — reset failure counter.
                            consecutive_failures = 0;
                            let elapsed = start.elapsed();
                            last_rtt = Some(elapsed);
                            let rtt_ms = elapsed.as_secs_f64() * 1000.0;
                            tracing::debug!(
                                cookie,
                                rtt_ms = rtt_ms as u64,
                                "keepalive: pong received"
                            );
                            // Report RTT to the caller for live latency tracking.
                            if let Some(ref tx) = self.rtt_tx {
                                let _ = tx.try_send(rtt_ms);
                            }
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
                _ = tokio::time::sleep(KEEPALIVE_RESPONSE_TIMEOUT) => {
                    // No pong within the deadline — peer may be unresponsive.
                    consecutive_failures += 1;
                    tracing::warn!(
                        cookie,
                        consecutive_failures,
                        "keepalive: pong timeout ({consecutive_failures}/{MAX_CONSECUTIVE_FAILURES})",
                    );
                    if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                        return Err(ProtocolError::KeepAliveTimeout {
                            consecutive_failures,
                        });
                    }
                    // Continue — try next ping after the sleep interval.
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
            std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
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

    /// 3 consecutive pong timeouts should trigger KeepAliveTimeout error.
    #[tokio::test(start_paused = true)]
    async fn consecutive_timeouts_trigger_escalation() {
        let (mut channel, mut egress_rx, _ingress_tx) = make_test_channel();
        let cancel = CancellationToken::new();
        // Use short interval so the test is fast with paused time.
        let client = KeepAliveClient::new(Duration::from_millis(100), cancel);

        let handle = tokio::spawn(async move { client.run(&mut channel).await });

        // For each of the 3 expected pings, drain the ping from egress
        // but never send a pong — let the response timeout fire.
        for expected_cookie in 0..3u16 {
            // Read the outgoing MsgKeepAlive.
            let (_, _, ping_bytes) = egress_rx.recv().await.unwrap();
            let ping = decode_message(&ping_bytes).unwrap();
            assert_eq!(ping, KeepAliveMessage::MsgKeepAlive(expected_cookie));

            // Advance past the 30s response timeout + 100ms ping interval.
            tokio::time::advance(Duration::from_secs(31)).await;
        }

        let result = handle.await.unwrap();
        assert!(result.is_err());
        match result.unwrap_err() {
            ProtocolError::KeepAliveTimeout {
                consecutive_failures,
            } => {
                assert_eq!(consecutive_failures, 3);
            }
            other => panic!("expected KeepAliveTimeout, got: {other}"),
        }
    }

    /// A successful pong between timeouts resets the failure counter.
    #[tokio::test(start_paused = true)]
    async fn success_resets_failure_counter() {
        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();
        let client = KeepAliveClient::new(Duration::from_millis(100), cancel);

        let handle = tokio::spawn(async move { client.run(&mut channel).await });

        // Ping 0: timeout (failure 1)
        let (_, _, ping_bytes) = egress_rx.recv().await.unwrap();
        let _ = decode_message(&ping_bytes).unwrap();
        tokio::time::advance(Duration::from_secs(31)).await;

        // Ping 1: success (resets counter to 0)
        let (_, _, ping_bytes) = egress_rx.recv().await.unwrap();
        let _ = decode_message(&ping_bytes).unwrap();
        let pong = encode_message(&KeepAliveMessage::MsgKeepAliveResponse(1));
        ingress_tx.send(Bytes::from(pong)).await.unwrap();
        // Advance past the interval
        tokio::time::advance(Duration::from_millis(200)).await;

        // Ping 2: timeout (failure 1 again, not 2)
        let (_, _, ping_bytes) = egress_rx.recv().await.unwrap();
        let _ = decode_message(&ping_bytes).unwrap();
        tokio::time::advance(Duration::from_secs(31)).await;

        // Ping 3: timeout (failure 2, not 3 — because we reset)
        let (_, _, ping_bytes) = egress_rx.recv().await.unwrap();
        let _ = decode_message(&ping_bytes).unwrap();

        // Cancel before a third consecutive failure.
        cancel_clone.cancel();

        let result = handle.await.unwrap();
        // Should complete without error (cancelled, not timed out).
        assert!(result.is_ok());
    }
}
