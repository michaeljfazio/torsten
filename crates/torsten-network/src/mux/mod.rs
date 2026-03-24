//! Ouroboros multiplexer.
//!
//! Multiplexes multiple mini-protocols over a single bearer (TCP or Unix socket).
//! Matches the Haskell `network-mux` architecture with SDU framing, direction bits,
//! and per-protocol channels.
//!
//! ## Architecture
//! - [`segment`] — SDU header encoding/decoding (8-byte wire format)
//! - [`channel`] — Per-protocol MuxChannel with CBOR boundary detection
//! - [`egress`] — Outbound message segmentation with round-robin fairness
//! - [`ingress`] — Inbound SDU demuxing with direction bit flipping
//!
//! ## Usage
//! ```ignore
//! let mut mux = Mux::new(bearer, true /* is_initiator */);
//! let chainsync_ch = mux.subscribe(PROTOCOL_N2N_CHAINSYNC, Direction::InitiatorDir, 65536);
//! let blockfetch_ch = mux.subscribe(PROTOCOL_N2N_BLOCKFETCH, Direction::InitiatorDir, 65536);
//! mux.run().await?;
//! ```

pub mod channel;
pub mod egress;
pub mod ingress;
pub mod segment;

use std::collections::HashMap;

use bytes::Bytes;
use tokio::sync::mpsc;

use crate::bearer::Bearer;
use crate::error::MuxError;

pub use channel::MuxChannel;
pub use segment::{Direction, SduHeader, HEADER_SIZE};

/// Default ingress channel capacity (number of byte chunks buffered).
const INGRESS_CHANNEL_CAPACITY: usize = 64;

/// Ouroboros multiplexer. Owns the bearer and coordinates ingress/egress tasks.
///
/// Subscribe protocol channels before calling `run()`. Once running, the mux
/// spawns ingress and egress tasks and blocks until the bearer is closed or
/// an error occurs.
pub struct Mux<B: Bearer> {
    /// The underlying transport (TCP or Unix socket).
    bearer: Option<B>,
    /// Whether we initiated the TCP connection (determines direction semantics).
    is_initiator: bool,
    /// Shared egress sender — cloned into each MuxChannel.
    egress_tx: mpsc::Sender<(u16, Direction, Bytes)>,
    /// Egress receiver — consumed by the egress task.
    egress_rx: Option<mpsc::Receiver<(u16, Direction, Bytes)>>,
    /// Per-protocol ingress senders — consumed by the ingress task.
    ingress_routes: HashMap<(u16, Direction), ingress::IngressRoute>,
}

impl<B: Bearer> Mux<B> {
    /// Create a new multiplexer over the given bearer.
    ///
    /// `is_initiator` is `true` if we initiated the TCP connection (client side),
    /// `false` if we accepted it (server side). This determines the direction
    /// semantics for the SDU direction bit.
    pub fn new(bearer: B, is_initiator: bool) -> Self {
        let (egress_tx, egress_rx) = mpsc::channel(channel::EGRESS_CHANNEL_CAPACITY * 8);
        Self {
            bearer: Some(bearer),
            is_initiator,
            egress_tx,
            egress_rx: Some(egress_rx),
            ingress_routes: HashMap::new(),
        }
    }

    /// Subscribe a protocol channel on this multiplexer.
    ///
    /// Must be called before `run()`. Returns a [`MuxChannel`] that the protocol
    /// implementation uses to send and receive CBOR messages.
    ///
    /// - `protocol_id`: Ouroboros protocol number (e.g., 2 for ChainSync)
    /// - `direction`: InitiatorDir or ResponderDir
    /// - `ingress_limit`: maximum bytes buffered for this channel before overrun
    pub fn subscribe(
        &mut self,
        protocol_id: u16,
        direction: Direction,
        ingress_limit: usize,
    ) -> MuxChannel {
        let (ingress_tx, ingress_rx) = mpsc::channel(INGRESS_CHANNEL_CAPACITY);

        // Register the ingress route for the ingress task to dispatch to.
        // The direction here is the LOCAL direction — what we'll see after
        // the ingress task flips the remote's direction bit.
        self.ingress_routes.insert(
            (protocol_id, direction),
            ingress::IngressRoute {
                tx: ingress_tx,
                limit: ingress_limit,
                buffered: 0,
            },
        );

        MuxChannel::new(
            protocol_id,
            direction,
            self.egress_tx.clone(),
            ingress_rx,
            ingress_limit,
        )
    }

    /// Run the multiplexer. Spawns ingress and egress tasks and blocks until
    /// the bearer is closed or an error occurs.
    ///
    /// Returns `Ok(())` on clean shutdown, `Err(MuxError)` on failure.
    pub async fn run(mut self) -> Result<(), MuxError> {
        let bearer = self.bearer.take().expect("bearer already consumed");
        let egress_rx = self.egress_rx.take().expect("egress_rx already consumed");
        let routes = std::mem::take(&mut self.ingress_routes);

        let sdu_size = bearer.sdu_size();
        let batch_size = bearer.batch_size();

        // Split the bearer into read and write halves using shared mutex.
        // This is simpler than requiring Bearer to implement split() and
        // works for our use case since egress and ingress don't contend heavily.
        let bearer = std::sync::Arc::new(tokio::sync::Mutex::new(bearer));
        let bearer_read = bearer.clone();
        let bearer_write = bearer;

        // Spawn the egress task
        let egress_task = egress::EgressTask::new(egress_rx, sdu_size, batch_size);
        let egress_handle = tokio::spawn(async move {
            egress_task
                .run(move |data: &[u8]| {
                    let bearer = bearer_write.clone();
                    let data = data.to_vec();
                    Box::pin(async move {
                        let mut b = bearer.lock().await;
                        b.write_all(&data).await?;
                        b.flush().await?;
                        Ok(())
                    })
                })
                .await
        });

        // Spawn the ingress task
        let ingress_task = ingress::IngressTask::new(routes);
        let ingress_handle = tokio::spawn(async move {
            ingress_task
                .run(move |n: usize| {
                    let bearer = bearer_read.clone();
                    Box::pin(async move {
                        let mut b = bearer.lock().await;
                        let mut buf = vec![0u8; n];
                        b.read_exact(&mut buf).await?;
                        Ok(buf)
                    })
                })
                .await
        });

        // Wait for either task to complete. If one fails, the other will
        // eventually fail too (bearer closed / channel dropped).
        tokio::select! {
            result = egress_handle => {
                match result {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(e)) => Err(e),
                    Err(_join_err) => Err(MuxError::ChannelClosed),
                }
            }
            result = ingress_handle => {
                match result {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(e)) => Err(e),
                    Err(_join_err) => Err(MuxError::ChannelClosed),
                }
            }
        }
    }

    /// Whether we are the TCP connection initiator.
    pub fn is_initiator(&self) -> bool {
        self.is_initiator
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bearer::MockBearer;
    use crate::mux::segment::encode_header;

    /// Build a mock bearer pre-loaded with raw SDU data for ingress testing.
    fn build_mock_bearer_with_sdus(sdus: Vec<(u16, Direction, Vec<u8>)>) -> MockBearer {
        let mut wire_data = Vec::new();
        for (pid, dir, payload) in sdus {
            let header = segment::SduHeader {
                timestamp: 0,
                protocol_id: pid,
                direction: dir,
                payload_length: payload.len() as u16,
            };
            wire_data.extend_from_slice(&encode_header(&header));
            wire_data.extend_from_slice(&payload);
        }
        MockBearer::new(wire_data, 12288, 131072)
    }

    #[tokio::test]
    async fn mux_subscribe_and_send() {
        // Create a mux with a mock bearer (no ingress data — it will EOF immediately)
        let bearer = MockBearer::new(vec![], 12288, 131072);
        let mut mux = Mux::new(bearer, true);

        let ch = mux.subscribe(2, Direction::InitiatorDir, 65536);

        // Send a message through the channel
        ch.send(vec![0x82, 0x01, 0x02]).await.unwrap();

        // Verify the channel properties
        assert_eq!(ch.protocol_id(), 2);
        assert_eq!(ch.direction(), Direction::InitiatorDir);
    }

    #[tokio::test]
    async fn mux_full_lifecycle() {
        // Build a bearer with one SDU for protocol 2 (sent by remote as InitiatorDir)
        let bearer =
            build_mock_bearer_with_sdus(vec![(2, Direction::InitiatorDir, vec![0x82, 0x01, 0x02])]);

        let mut mux = Mux::new(bearer, false); // we are responder

        // Subscribe protocol 2, ResponderDir (after flip from remote's InitiatorDir)
        let mut ch = mux.subscribe(2, Direction::ResponderDir, 65536);

        // Run the mux in the background
        let mux_handle = tokio::spawn(async move { mux.run().await });

        // Receive the message from the channel
        let msg = ch.recv().await.unwrap();
        assert_eq!(msg, vec![0x82, 0x01, 0x02]);

        // The mux should complete after EOF on the bearer
        let result = mux_handle.await.unwrap();
        // It's OK if this returns an error due to the bearer closing
        let _ = result;
    }
}
