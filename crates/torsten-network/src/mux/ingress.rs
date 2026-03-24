//! Ingress task — reads multiplexed SDU segments from the bearer and dispatches them.
//!
//! Reads 8-byte SDU headers followed by payloads from the bearer, flips the direction
//! bit (remote's InitiatorDir → our ResponderDir), and dispatches payloads to the
//! appropriate per-protocol channel.
//!
//! ## Protocol ID 1
//! Protocol ID 1 is reserved in the Ouroboros spec and never used. Data for this
//! protocol is silently discarded.
//!
//! ## Byte tracking
//! The ingress task tracks how many bytes are buffered per `(protocol_id, direction)`
//! channel. If a channel exceeds its configured limit, `IngressQueueOverrun` is returned.

use bytes::Bytes;
use std::collections::HashMap;
use tokio::sync::mpsc;

use crate::error::{BearerError, MuxError};
use crate::mux::segment::{decode_header, Direction, HEADER_SIZE};

/// Reserved protocol ID that is silently discarded on ingress.
const RESERVED_PROTOCOL_ID: u16 = 1;

/// Per-protocol ingress channel registration.
pub(crate) struct IngressRoute {
    /// Sender to deliver byte chunks to the protocol's MuxChannel.
    pub tx: mpsc::Sender<Bytes>,
    /// Maximum bytes allowed in this channel's queue before overrun.
    pub limit: usize,
    /// Current estimated bytes buffered (incremented on send, never decremented
    /// since we can't observe the receiver draining — this is a conservative estimate).
    pub buffered: usize,
}

/// Ingress task state. Created by the [`Mux`] and run as a spawned tokio task.
pub struct IngressTask {
    /// Registered protocol channels, keyed by `(protocol_id, direction)`.
    routes: HashMap<(u16, Direction), IngressRoute>,
}

impl IngressTask {
    /// Create a new ingress task with the given protocol routes.
    pub(crate) fn new(routes: HashMap<(u16, Direction), IngressRoute>) -> Self {
        Self { routes }
    }

    /// Run the ingress loop. Reads SDU headers + payloads from the bearer and
    /// dispatches to registered protocol channels.
    ///
    /// The `read_fn` is called to read exact byte counts from the bearer.
    ///
    /// Returns `Ok(())` when the bearer is cleanly closed (EOF).
    /// Returns `Err(MuxError)` on read errors or queue overruns.
    pub async fn run<R>(mut self, mut read_fn: R) -> Result<(), MuxError>
    where
        R: FnMut(
                usize,
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = Result<Vec<u8>, BearerError>> + Send>,
            > + Send,
    {
        loop {
            // Read the 8-byte SDU header
            let header_bytes = match read_fn(HEADER_SIZE).await {
                Ok(bytes) => bytes,
                Err(BearerError::ConnectionReset) => return Ok(()), // clean EOF
                Err(e) => return Err(MuxError::Bearer(e)),
            };

            let header = decode_header(header_bytes[..8].try_into().expect("read exact 8 bytes"));

            // Skip reserved protocol ID 1
            if header.protocol_id == RESERVED_PROTOCOL_ID {
                if header.payload_length > 0 {
                    // Read and discard the payload
                    let _ = read_fn(header.payload_length as usize)
                        .await
                        .map_err(MuxError::Bearer)?;
                }
                continue;
            }

            // Read the payload
            let payload = if header.payload_length > 0 {
                read_fn(header.payload_length as usize)
                    .await
                    .map_err(MuxError::Bearer)?
            } else {
                Vec::new()
            };

            // Flip direction: what the remote sent as InitiatorDir, we receive as ResponderDir
            let local_direction = header.direction.flip();
            let key = (header.protocol_id, local_direction);

            match self.routes.get_mut(&key) {
                Some(route) => {
                    // Check byte limit before sending
                    let new_buffered = route.buffered + payload.len();
                    if new_buffered > route.limit {
                        return Err(MuxError::IngressQueueOverrun {
                            protocol_id: header.protocol_id,
                            bytes: new_buffered,
                            limit: route.limit,
                        });
                    }

                    route.buffered = new_buffered;

                    if !payload.is_empty() {
                        // Send to protocol channel — if the receiver is dropped, the
                        // protocol has shut down; we just ignore the error and continue.
                        let _ = route.tx.send(Bytes::from(payload)).await;
                    }
                }
                None => {
                    // Unknown protocol — could log a warning, but for now just skip.
                    // This is more lenient than returning UnknownProtocol, since
                    // Haskell nodes may send data for protocols we haven't subscribed to.
                    tracing::debug!(
                        protocol_id = header.protocol_id,
                        direction = ?local_direction,
                        payload_len = header.payload_length,
                        "ingress: received data for unsubscribed protocol"
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a raw SDU (header + payload) for testing.
    fn build_sdu(protocol_id: u16, direction: Direction, payload: &[u8]) -> Vec<u8> {
        use crate::mux::segment::encode_header;
        let header = crate::mux::segment::SduHeader {
            timestamp: 0,
            protocol_id,
            direction,
            payload_length: payload.len() as u16,
        };
        let mut buf = encode_header(&header).to_vec();
        buf.extend_from_slice(payload);
        buf
    }

    #[tokio::test]
    async fn dispatches_to_correct_channel() {
        let (tx2, mut rx2) = mpsc::channel(32);
        let (tx3, mut rx3) = mpsc::channel(32);

        let mut routes = HashMap::new();
        // Protocol 2, ResponderDir (after flip from InitiatorDir)
        routes.insert(
            (2, Direction::ResponderDir),
            IngressRoute {
                tx: tx2,
                limit: 65536,
                buffered: 0,
            },
        );
        // Protocol 3, ResponderDir
        routes.insert(
            (3, Direction::ResponderDir),
            IngressRoute {
                tx: tx3,
                limit: 65536,
                buffered: 0,
            },
        );

        let task = IngressTask::new(routes);

        // Build raw wire data: protocol 2 message + protocol 3 message + EOF
        let mut wire_data = Vec::new();
        wire_data.extend_from_slice(&build_sdu(2, Direction::InitiatorDir, &[0x82, 0x01, 0x02]));
        wire_data.extend_from_slice(&build_sdu(
            3,
            Direction::InitiatorDir,
            &[0x83, 0x01, 0x02, 0x03],
        ));

        let wire_data = std::sync::Arc::new(std::sync::Mutex::new(wire_data));
        let offset = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let wire_data_clone = wire_data.clone();
        let offset_clone = offset.clone();

        task.run(move |n: usize| {
            let wire_data = wire_data_clone.clone();
            let offset = offset_clone.clone();
            Box::pin(async move {
                let data = wire_data.lock().unwrap();
                let off = offset.load(std::sync::atomic::Ordering::SeqCst);
                if off + n > data.len() {
                    return Err(BearerError::ConnectionReset);
                }
                let result = data[off..off + n].to_vec();
                offset.store(off + n, std::sync::atomic::Ordering::SeqCst);
                Ok(result)
            })
        })
        .await
        .unwrap();

        // Verify protocol 2 received its message
        let chunk2 = rx2.recv().await.unwrap();
        assert_eq!(chunk2.as_ref(), &[0x82, 0x01, 0x02]);

        // Verify protocol 3 received its message
        let chunk3 = rx3.recv().await.unwrap();
        assert_eq!(chunk3.as_ref(), &[0x83, 0x01, 0x02, 0x03]);
    }

    #[tokio::test]
    async fn reserved_protocol_id_discarded() {
        let (tx2, mut rx2) = mpsc::channel(32);

        let mut routes = HashMap::new();
        routes.insert(
            (2, Direction::ResponderDir),
            IngressRoute {
                tx: tx2,
                limit: 65536,
                buffered: 0,
            },
        );

        let task = IngressTask::new(routes);

        // Reserved protocol 1 message followed by valid protocol 2 message
        let mut wire_data = Vec::new();
        wire_data.extend_from_slice(&build_sdu(
            RESERVED_PROTOCOL_ID,
            Direction::InitiatorDir,
            &[0xFF, 0xFF],
        ));
        wire_data.extend_from_slice(&build_sdu(2, Direction::InitiatorDir, &[0x81, 0x01]));

        let wire_data = std::sync::Arc::new(std::sync::Mutex::new(wire_data));
        let offset = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let wire_data_clone = wire_data.clone();
        let offset_clone = offset.clone();

        task.run(move |n: usize| {
            let wire_data = wire_data_clone.clone();
            let offset = offset_clone.clone();
            Box::pin(async move {
                let data = wire_data.lock().unwrap();
                let off = offset.load(std::sync::atomic::Ordering::SeqCst);
                if off + n > data.len() {
                    return Err(BearerError::ConnectionReset);
                }
                let result = data[off..off + n].to_vec();
                offset.store(off + n, std::sync::atomic::Ordering::SeqCst);
                Ok(result)
            })
        })
        .await
        .unwrap();

        // Only protocol 2 should have received data
        let chunk = rx2.recv().await.unwrap();
        assert_eq!(chunk.as_ref(), &[0x81, 0x01]);
    }
}
