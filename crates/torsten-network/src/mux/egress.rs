//! Egress task — writes multiplexed SDU segments to the bearer.
//!
//! Receives complete protocol messages from all [`MuxChannel`]s, segments them
//! into SDU-sized chunks with proper headers, and writes them to the bearer in
//! batches for efficiency.
//!
//! ## Fairness
//! If a message exceeds one SDU, only one chunk is written per round. The remainder
//! is re-enqueued so that other protocols get a turn (round-robin fairness). This
//! prevents a large BlockFetch response from starving KeepAlive responses.
//!
//! ## Batching
//! Multiple SDUs are accumulated up to `batch_size` bytes before a single
//! `write_all()` + `flush()` call to the bearer, reducing syscall overhead.

use bytes::Bytes;
use tokio::sync::mpsc;

use crate::error::{BearerError, MuxError};
use crate::mux::segment::{current_timestamp, encode_header, Direction, SduHeader};

/// Maximum number of SDUs to accumulate in a single write batch.
const MAX_SDUS_PER_BATCH: usize = 100;

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
        // Pending remainder from a message that was too large for one SDU.
        // We re-process these before reading new messages (round-robin fairness).
        let mut pending: Vec<(u16, Direction, Bytes)> = Vec::new();
        // Batch buffer for accumulating multiple SDUs before a single write.
        let mut batch_buf: Vec<u8> = Vec::with_capacity(self.batch_size);

        loop {
            // First, process any pending remainders from previous round
            let mut next_pending: Vec<(u16, Direction, Bytes)> = Vec::new();

            for (protocol_id, direction, data) in pending.drain(..) {
                self.write_one_sdu(
                    protocol_id,
                    direction,
                    data,
                    &mut batch_buf,
                    &mut next_pending,
                );

                // Flush batch if it's getting large
                if batch_buf.len() >= self.batch_size || next_pending.len() >= MAX_SDUS_PER_BATCH {
                    write_fn(&batch_buf).await.map_err(MuxError::Bearer)?;
                    batch_buf.clear();
                }
            }

            pending = next_pending;

            // If we have pending remainders, try to drain more without blocking
            if !pending.is_empty() {
                // Flush what we have and continue the loop to process remainders
                if !batch_buf.is_empty() {
                    write_fn(&batch_buf).await.map_err(MuxError::Bearer)?;
                    batch_buf.clear();
                }
                continue;
            }

            // Read new messages — block on first, then drain non-blocking
            let first = self.rx.recv().await;
            match first {
                None => {
                    // Channel closed — flush any remaining data and exit
                    if !batch_buf.is_empty() {
                        write_fn(&batch_buf).await.map_err(MuxError::Bearer)?;
                    }
                    return Ok(());
                }
                Some((pid, dir, data)) => {
                    self.write_one_sdu(pid, dir, data, &mut batch_buf, &mut pending);
                }
            }

            // Drain any additional messages without blocking
            while let Ok((pid, dir, data)) = self.rx.try_recv() {
                self.write_one_sdu(pid, dir, data, &mut batch_buf, &mut pending);
                if batch_buf.len() >= self.batch_size {
                    break;
                }
            }

            // Flush the batch
            if !batch_buf.is_empty() {
                write_fn(&batch_buf).await.map_err(MuxError::Bearer)?;
                batch_buf.clear();
            }
        }
    }

    /// Write one SDU-worth of a message to the batch buffer.
    /// If the message is larger than `sdu_size`, only the first chunk is written
    /// and the remainder is pushed to `pending` for the next round (fairness).
    fn write_one_sdu(
        &self,
        protocol_id: u16,
        direction: Direction,
        data: Bytes,
        batch_buf: &mut Vec<u8>,
        pending: &mut Vec<(u16, Direction, Bytes)>,
    ) {
        let chunk_len = data.len().min(self.sdu_size);
        let header = SduHeader {
            timestamp: current_timestamp(),
            protocol_id,
            direction,
            payload_length: chunk_len as u16,
        };
        batch_buf.extend_from_slice(&encode_header(&header));
        batch_buf.extend_from_slice(&data[..chunk_len]);

        // If there's a remainder, enqueue it for the next round
        if chunk_len < data.len() {
            pending.push((protocol_id, direction, data.slice(chunk_len..)));
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
