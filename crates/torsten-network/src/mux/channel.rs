//! Per-protocol multiplexer channel.
//!
//! A [`MuxChannel`] is the typed handle that a mini-protocol (e.g., ChainSync, BlockFetch)
//! uses to send and receive CBOR messages through the multiplexer. Each channel is bound
//! to a specific `(protocol_id, direction)` pair.
//!
//! ## Send path
//! `send()` writes a complete CBOR message to the egress queue via a bounded `mpsc` channel.
//! The egress task then segments it into SDU-sized chunks for the bearer.
//!
//! ## Receive path
//! `recv()` reads byte chunks delivered by the ingress task and accumulates them in an
//! internal buffer. It uses [`codec::try_decode_cbor_boundary`] to detect when a complete
//! CBOR message has been assembled, then returns the complete message bytes.

use bytes::Bytes;
use tokio::sync::mpsc;

use crate::codec::try_decode_cbor_boundary;
use crate::error::MuxError;
use crate::mux::segment::Direction;

/// Bounded capacity for the egress send channel (messages, not bytes).
/// This limits how many complete messages a protocol can enqueue before
/// back-pressure kicks in.
pub const EGRESS_CHANNEL_CAPACITY: usize = 32;

/// Per-protocol multiplexer channel handle.
///
/// Provides `send()` and `recv()` for a specific `(protocol_id, direction)` pair.
/// The protocol layer uses this to exchange CBOR messages through the mux.
pub struct MuxChannel {
    /// Protocol number this channel is bound to.
    pub(crate) protocol_id: u16,
    /// Direction this channel operates in.
    pub(crate) direction: Direction,
    /// Sender to egress task: (protocol_id, direction, message_bytes).
    pub(crate) egress_tx: mpsc::Sender<(u16, Direction, Bytes)>,
    /// Receiver from ingress task: raw byte chunks.
    pub(crate) ingress_rx: mpsc::Receiver<Bytes>,
    /// Reassembly buffer — accumulates byte chunks until a complete CBOR message is detected.
    reassembly_buf: Vec<u8>,
    /// Maximum size for the reassembly buffer (prevents unbounded growth from malformed data).
    ingress_limit: usize,
}

impl MuxChannel {
    /// Create a new mux channel.
    ///
    /// - `protocol_id`: the Ouroboros protocol number
    /// - `direction`: InitiatorDir or ResponderDir
    /// - `egress_tx`: sender to the shared egress queue
    /// - `ingress_rx`: receiver for byte chunks from the ingress demuxer
    /// - `ingress_limit`: maximum reassembly buffer size in bytes
    pub fn new(
        protocol_id: u16,
        direction: Direction,
        egress_tx: mpsc::Sender<(u16, Direction, Bytes)>,
        ingress_rx: mpsc::Receiver<Bytes>,
        ingress_limit: usize,
    ) -> Self {
        Self {
            protocol_id,
            direction,
            egress_tx,
            ingress_rx,
            reassembly_buf: Vec::new(),
            ingress_limit,
        }
    }

    /// Send a complete CBOR message through the multiplexer.
    ///
    /// The message is enqueued to the egress task, which will segment it into
    /// SDU-sized chunks and write them to the bearer. This may block if the
    /// egress channel is full (back-pressure from slow bearer writes).
    pub async fn send(&self, msg: Vec<u8>) -> Result<(), MuxError> {
        self.egress_tx
            .send((self.protocol_id, self.direction, Bytes::from(msg)))
            .await
            .map_err(|_| MuxError::ChannelClosed)
    }

    /// Receive a complete CBOR message from the multiplexer.
    ///
    /// Blocks until a complete CBOR data item has been assembled from ingress
    /// byte chunks. Uses CBOR boundary detection to handle messages that span
    /// multiple SDU segments.
    ///
    /// Returns `MuxError::BearerClosed` if the ingress channel is closed (connection gone).
    /// Returns `MuxError::IngressQueueOverrun` if the reassembly buffer exceeds the limit
    /// without producing a complete message (malformed data protection).
    pub async fn recv(&mut self) -> Result<Vec<u8>, MuxError> {
        loop {
            // Check if we already have a complete CBOR message in the buffer
            if let Some(boundary) = try_decode_cbor_boundary(&self.reassembly_buf) {
                let msg = self.reassembly_buf[..boundary].to_vec();
                // Remove consumed bytes from buffer, keeping any remainder
                self.reassembly_buf.drain(..boundary);
                return Ok(msg);
            }

            // Need more data — read next chunk from ingress
            match self.ingress_rx.recv().await {
                Some(chunk) => {
                    self.reassembly_buf.extend_from_slice(&chunk);
                    // Check buffer limit to prevent unbounded growth from malformed data
                    if self.reassembly_buf.len() > self.ingress_limit {
                        return Err(MuxError::IngressQueueOverrun {
                            protocol_id: self.protocol_id,
                            bytes: self.reassembly_buf.len(),
                            limit: self.ingress_limit,
                        });
                    }
                }
                None => return Err(MuxError::BearerClosed),
            }
        }
    }

    /// Non-blocking attempt to receive a complete CBOR message.
    ///
    /// Returns `Ok(Some(msg))` if a complete message is available,
    /// `Ok(None)` if no complete message is ready yet,
    /// `Err` on channel closure or buffer overflow.
    ///
    /// Used by TxSubmission2 for non-blocking tx ID polling.
    pub fn try_recv(&mut self) -> Result<Option<Vec<u8>>, MuxError> {
        // Drain any available chunks from the ingress channel
        loop {
            match self.ingress_rx.try_recv() {
                Ok(chunk) => {
                    self.reassembly_buf.extend_from_slice(&chunk);
                    if self.reassembly_buf.len() > self.ingress_limit {
                        return Err(MuxError::IngressQueueOverrun {
                            protocol_id: self.protocol_id,
                            bytes: self.reassembly_buf.len(),
                            limit: self.ingress_limit,
                        });
                    }
                }
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    // Channel closed — check if we have a complete message buffered first
                    if try_decode_cbor_boundary(&self.reassembly_buf).is_some() {
                        break;
                    }
                    return Err(MuxError::BearerClosed);
                }
            }
        }

        // Check if we have a complete CBOR message
        if let Some(boundary) = try_decode_cbor_boundary(&self.reassembly_buf) {
            let msg = self.reassembly_buf[..boundary].to_vec();
            self.reassembly_buf.drain(..boundary);
            Ok(Some(msg))
        } else {
            Ok(None)
        }
    }

    /// Get the protocol ID this channel is bound to.
    pub fn protocol_id(&self) -> u16 {
        self.protocol_id
    }

    /// Get the direction this channel operates in.
    pub fn direction(&self) -> Direction {
        self.direction
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn send_and_recv_single_message() {
        let (egress_tx, mut egress_rx) = mpsc::channel(32);
        let (ingress_tx, ingress_rx) = mpsc::channel(32);

        let mut channel = MuxChannel::new(2, Direction::InitiatorDir, egress_tx, ingress_rx, 65536);

        // Send a message
        let msg = vec![0x82, 0x01, 0x02]; // CBOR [1, 2]
        channel.send(msg.clone()).await.unwrap();

        // Verify it was sent to egress
        let (pid, dir, data) = egress_rx.recv().await.unwrap();
        assert_eq!(pid, 2);
        assert_eq!(dir, Direction::InitiatorDir);
        assert_eq!(data.as_ref(), &msg);

        // Simulate ingress delivering a complete CBOR message
        ingress_tx
            .send(Bytes::from(vec![0x83, 0x01, 0x02, 0x03]))
            .await
            .unwrap(); // [1, 2, 3]
        let received = channel.recv().await.unwrap();
        assert_eq!(received, vec![0x83, 0x01, 0x02, 0x03]);
    }

    #[tokio::test]
    async fn recv_reassembles_split_message() {
        let (egress_tx, _egress_rx) = mpsc::channel(32);
        let (ingress_tx, ingress_rx) = mpsc::channel(32);

        let mut channel = MuxChannel::new(2, Direction::InitiatorDir, egress_tx, ingress_rx, 65536);

        // CBOR array [1, 2] = 0x82 0x01 0x02, split across two chunks
        ingress_tx
            .send(Bytes::from(vec![0x82, 0x01]))
            .await
            .unwrap();
        ingress_tx.send(Bytes::from(vec![0x02])).await.unwrap();

        let received = channel.recv().await.unwrap();
        assert_eq!(received, vec![0x82, 0x01, 0x02]);
    }

    #[tokio::test]
    async fn recv_handles_multiple_messages_in_buffer() {
        let (egress_tx, _egress_rx) = mpsc::channel(32);
        let (ingress_tx, ingress_rx) = mpsc::channel(32);

        let mut channel = MuxChannel::new(2, Direction::InitiatorDir, egress_tx, ingress_rx, 65536);

        // Two complete CBOR messages back-to-back: [1] (0x81 0x01) and [2] (0x81 0x02)
        ingress_tx
            .send(Bytes::from(vec![0x81, 0x01, 0x81, 0x02]))
            .await
            .unwrap();

        let msg1 = channel.recv().await.unwrap();
        assert_eq!(msg1, vec![0x81, 0x01]);

        let msg2 = channel.recv().await.unwrap();
        assert_eq!(msg2, vec![0x81, 0x02]);
    }

    #[tokio::test]
    async fn ingress_limit_enforced() {
        let (egress_tx, _egress_rx) = mpsc::channel(32);
        let (ingress_tx, ingress_rx) = mpsc::channel(32);

        // Very small limit: 4 bytes
        let mut channel = MuxChannel::new(2, Direction::InitiatorDir, egress_tx, ingress_rx, 4);

        // Send 5 bytes of incomplete CBOR — should trigger overflow
        ingress_tx
            .send(Bytes::from(vec![0x85, 0x01, 0x02, 0x03, 0x04]))
            .await
            .unwrap();

        let result = channel.recv().await;
        assert!(result.is_err());
        if let Err(MuxError::IngressQueueOverrun {
            protocol_id,
            bytes,
            limit,
        }) = result
        {
            assert_eq!(protocol_id, 2);
            assert_eq!(bytes, 5);
            assert_eq!(limit, 4);
        } else {
            panic!("expected IngressQueueOverrun");
        }
    }

    #[tokio::test]
    async fn try_recv_returns_none_when_empty() {
        let (egress_tx, _egress_rx) = mpsc::channel(32);
        let (_ingress_tx, ingress_rx) = mpsc::channel(32);

        let mut channel = MuxChannel::new(2, Direction::InitiatorDir, egress_tx, ingress_rx, 65536);

        assert_eq!(channel.try_recv().unwrap(), None);
    }

    #[tokio::test]
    async fn try_recv_returns_complete_message() {
        let (egress_tx, _egress_rx) = mpsc::channel(32);
        let (ingress_tx, ingress_rx) = mpsc::channel(32);

        let mut channel = MuxChannel::new(2, Direction::InitiatorDir, egress_tx, ingress_rx, 65536);

        ingress_tx
            .send(Bytes::from(vec![0x81, 0x05]))
            .await
            .unwrap();
        // Small delay to let the channel buffer
        tokio::task::yield_now().await;

        let msg = channel.try_recv().unwrap();
        assert_eq!(msg, Some(vec![0x81, 0x05]));
    }
}
