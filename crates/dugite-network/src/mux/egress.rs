//! Egress task — writes multiplexed SDU segments to the bearer.
//!
//! Receives complete protocol messages from all [`MuxChannel`]s, segments them
//! into SDU-sized chunks with proper headers, and writes them to the bearer in
//! batches for efficiency.
//!
//! ## Per-protocol serialisation
//!
//! The Ouroboros mux delivers SDU payloads per-protocol in arrival order.
//! A receiver accumulates bytes from successive SDUs until a complete CBOR
//! message is formed.  If segments from two *different messages* on the
//! *same protocol* are interleaved, the receiver concatenates them and the
//! CBOR decoder sees corrupted input.
//!
//! Therefore: for each `(protocol_id, direction)` pair, a message's
//! continuation segments **must** be fully sent before any segment of the
//! *next* message on that same pair can start.  Messages from *different*
//! protocols may still be interleaved freely (fairness).
//!
//! ## Fairness
//!
//! Between continuation chunks of a large message, the egress serves one
//! chunk from every other protocol that has pending data (round-robin).
//! This prevents a large BlockFetch response from starving KeepAlive.
//!
//! ## Batching
//!
//! Multiple SDUs are accumulated up to `batch_size` bytes before a single
//! `write_all()` + `flush()` call to the bearer, reducing syscall overhead.

use bytes::Bytes;
use std::collections::{HashMap, VecDeque};
use tokio::sync::mpsc;

use crate::error::{BearerError, MuxError};
use crate::mux::segment::{current_timestamp, encode_header, Direction, SduHeader};

/// Maximum number of SDUs to accumulate in a single write batch.
const MAX_SDUS_PER_BATCH: usize = 100;

/// Key identifying a protocol channel: (protocol_id, direction).
type ChannelKey = (u16, Direction);

/// Egress task state. Created by the [`Mux`] and run as a spawned tokio task.
pub struct EgressTask {
    /// Receiver for outbound messages from all protocol channels.
    /// Each message is `(protocol_id, direction, complete_message_bytes)`.
    rx: mpsc::Receiver<(u16, Direction, Bytes)>,
    /// Maximum SDU payload size for this bearer (e.g., 12288 for TCP).
    sdu_size: usize,
    /// Maximum bytes per write batch (e.g., 131072 for TCP).
    batch_size: usize,
}

impl EgressTask {
    /// Create a new egress task.
    pub fn new(
        rx: mpsc::Receiver<(u16, Direction, Bytes)>,
        sdu_size: usize,
        batch_size: usize,
    ) -> Self {
        Self {
            rx,
            sdu_size,
            batch_size,
        }
    }

    /// Run the egress loop. Reads messages, segments into SDUs, batches writes.
    ///
    /// The `write_fn` is called with each batch of bytes to write to the bearer.
    /// Using a closure instead of a Bearer trait object allows the mux to split
    /// the bearer into separate read/write halves.
    ///
    /// Returns `Ok(())` when the channel is closed (clean shutdown).
    /// Returns `Err(MuxError)` on bearer write failure.
    pub async fn run<W>(mut self, mut write_fn: W) -> Result<(), MuxError>
    where
        W: FnMut(
                &[u8],
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = Result<(), BearerError>> + Send>,
            > + Send,
    {
        // Per-channel message queue.  Each entry is a complete message or a
        // continuation remainder.  For a given channel, only the FRONT item
        // may have segments on the wire — new messages are queued BEHIND the
        // current one so their bytes never interleave.
        let mut queues: HashMap<ChannelKey, VecDeque<Bytes>> = HashMap::new();

        // Batch buffer for accumulating multiple SDUs before a single write.
        let mut batch_buf: Vec<u8> = Vec::with_capacity(self.batch_size);

        loop {
            // ── Phase 1: write one SDU per channel that has data ─────────
            let mut made_progress = false;
            let keys: Vec<ChannelKey> = queues.keys().copied().collect();

            for key in &keys {
                let queue = match queues.get_mut(key) {
                    Some(q) if !q.is_empty() => q,
                    _ => continue,
                };

                // Take the front item (the in-flight message for this channel).
                let data = queue.pop_front().unwrap();
                let chunk_len = data.len().min(self.sdu_size);

                let header = SduHeader {
                    timestamp: current_timestamp(),
                    protocol_id: key.0,
                    direction: key.1,
                    payload_length: chunk_len as u16,
                };
                batch_buf.extend_from_slice(&encode_header(&header));
                batch_buf.extend_from_slice(&data[..chunk_len]);
                made_progress = true;

                // If there's a remainder, push it BACK TO THE FRONT so it
                // is sent before any queued successor message.
                if chunk_len < data.len() {
                    queue.push_front(data.slice(chunk_len..));
                }

                if batch_buf.len() >= self.batch_size {
                    write_fn(&batch_buf).await.map_err(MuxError::Bearer)?;
                    batch_buf.clear();
                }
            }

            // Remove empty queues.
            queues.retain(|_, q| !q.is_empty());

            // Flush whatever accumulated in this round.
            if !batch_buf.is_empty() {
                write_fn(&batch_buf).await.map_err(MuxError::Bearer)?;
                batch_buf.clear();
            }

            // If we made progress, loop again to send more continuation
            // chunks (or start the next queued message).
            if made_progress {
                // Non-blocking drain of any new messages that arrived while
                // we were writing.
                let mut sdu_count = 0;
                while let Ok((pid, dir, data)) = self.rx.try_recv() {
                    queues.entry((pid, dir)).or_default().push_back(data);
                    sdu_count += 1;
                    if sdu_count >= MAX_SDUS_PER_BATCH {
                        break;
                    }
                }
                continue;
            }

            // ── Phase 2: no pending data — block for the next message ────
            match self.rx.recv().await {
                None => return Ok(()),
                Some((pid, dir, data)) => {
                    queues.entry((pid, dir)).or_default().push_back(data);
                }
            }

            // Non-blocking drain of any additional messages.
            while let Ok((pid, dir, data)) = self.rx.try_recv() {
                queues.entry((pid, dir)).or_default().push_back(data);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mux::segment::{decode_header, HEADER_SIZE};
    use std::sync::{Arc, Mutex};

    /// Helper to run egress with a capturing write function.
    async fn run_egress_capturing(
        sdu_size: usize,
        messages: Vec<(u16, Direction, Vec<u8>)>,
    ) -> Vec<u8> {
        let (tx, rx) = mpsc::channel(64);
        let task = EgressTask::new(rx, sdu_size, 131072);

        // Send all messages
        for (pid, dir, data) in messages {
            tx.send((pid, dir, Bytes::from(data))).await.unwrap();
        }
        // Close the channel to signal shutdown
        drop(tx);

        let captured = Arc::new(Mutex::new(Vec::new()));
        let captured_clone = captured.clone();

        task.run(move |data: &[u8]| {
            let captured = captured_clone.clone();
            let data = data.to_vec();
            Box::pin(async move {
                captured.lock().unwrap().extend_from_slice(&data);
                Ok(())
            })
        })
        .await
        .unwrap();

        Arc::try_unwrap(captured).unwrap().into_inner().unwrap()
    }

    #[tokio::test]
    async fn single_message_fits_in_one_sdu() {
        let msg = vec![0x82, 0x01, 0x02]; // 3 bytes, well under SDU limit
        let captured =
            run_egress_capturing(12288, vec![(2, Direction::InitiatorDir, msg.clone())]).await;

        // Should be exactly one SDU: 8-byte header + 3-byte payload
        assert_eq!(captured.len(), HEADER_SIZE + 3);

        let header = decode_header(captured[..8].try_into().unwrap());
        assert_eq!(header.protocol_id, 2);
        assert_eq!(header.direction, Direction::InitiatorDir);
        assert_eq!(header.payload_length, 3);
        assert_eq!(&captured[8..], &msg);
    }

    #[tokio::test]
    async fn large_message_segmented_across_sdus() {
        // SDU size = 4 bytes, message = 10 bytes → should be split into 3 SDUs (4+4+2)
        let msg = vec![0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A];
        let captured =
            run_egress_capturing(4, vec![(3, Direction::ResponderDir, msg.clone())]).await;

        // Parse SDUs from the captured bytes
        let mut offset = 0;
        let mut chunks: Vec<Vec<u8>> = Vec::new();
        while offset < captured.len() {
            let header = decode_header(captured[offset..offset + 8].try_into().unwrap());
            offset += HEADER_SIZE;
            let payload = &captured[offset..offset + header.payload_length as usize];
            chunks.push(payload.to_vec());
            offset += header.payload_length as usize;

            assert_eq!(header.protocol_id, 3);
            assert_eq!(header.direction, Direction::ResponderDir);
        }

        // Reassembled should match original
        let reassembled: Vec<u8> = chunks.into_iter().flatten().collect();
        assert_eq!(reassembled, msg);
    }

    #[tokio::test]
    async fn multiple_protocols_interleaved() {
        let msg_a = vec![0x01; 6]; // protocol 2, 6 bytes
        let msg_b = vec![0x02; 3]; // protocol 3, 3 bytes
        let captured = run_egress_capturing(
            4,
            vec![
                (2, Direction::InitiatorDir, msg_a),
                (3, Direction::InitiatorDir, msg_b),
            ],
        )
        .await;

        // With SDU=4: msg_a needs 2 SDUs (4+2), msg_b needs 1 SDU (3)
        // Due to round-robin, we should see interleaved protocol IDs
        let mut offset = 0;
        let mut protocol_ids = Vec::new();
        while offset < captured.len() {
            let header = decode_header(captured[offset..offset + 8].try_into().unwrap());
            protocol_ids.push(header.protocol_id);
            offset += HEADER_SIZE + header.payload_length as usize;
        }

        // msg_a first chunk (pid=2), msg_b (pid=3), msg_a remainder (pid=2)
        // OR msg_a chunk (pid=2), msg_b chunk (pid=3), msg_a remainder (pid=2)
        // The exact interleaving depends on batching, but both protocols should appear
        assert!(protocol_ids.contains(&2));
        assert!(protocol_ids.contains(&3));
    }
}
