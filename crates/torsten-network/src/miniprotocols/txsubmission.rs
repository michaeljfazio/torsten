//! TxSubmission2 mini-protocol (N2N transaction submission).
//!
//! In N2N TxSubmission2, when we connect to a peer (initiator), we are the
//! **server** — we request transaction IDs and bodies from the peer. The peer
//! (responder/client) advertises transactions from their mempool.
//!
//! Protocol flow:
//! 1. Both sides send MsgInit (bidirectional initialization)
//! 2. Server sends MsgRequestTxIds → Client replies with MsgReplyTxIds
//! 3. Server sends MsgRequestTxs → Client replies with MsgReplyTxs
//! 4. Server sends MsgDone to close

use pallas_network::multiplexer::{AgentChannel, MAX_SEGMENT_PAYLOAD_LENGTH};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::time::{timeout, Duration};
use torsten_mempool::Mempool;
use torsten_primitives::hash::{Hash32, TransactionHash};
use tracing::{debug, info, trace};

use crate::n2c::TxValidator;

/// Maximum number of tx IDs to request per batch
const MAX_TX_IDS_REQUEST: u16 = 100;

/// Timeout for receiving a response from the peer
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);

/// N2N TxSubmission2 client (initiator/server role).
///
/// Requests transactions from a connected peer and adds them to the local mempool
/// after validation.
pub struct TxSubmissionClient {
    channel: AgentChannel,
    /// Reassembly buffer for multi-chunk messages
    recv_buf: Vec<u8>,
    /// Set of tx IDs we have already requested from this peer (to avoid duplicates)
    known_tx_ids: HashSet<TransactionHash>,
    /// Number of tx IDs received but not yet acknowledged
    pending_ack: u16,
}

impl TxSubmissionClient {
    /// Create a new TxSubmission2 client from a multiplexer agent channel.
    pub fn new(channel: AgentChannel) -> Self {
        TxSubmissionClient {
            channel,
            recv_buf: Vec::new(),
            known_tx_ids: HashSet::new(),
            pending_ack: 0,
        }
    }

    /// Send raw CBOR payload through the channel, splitting into chunks if needed.
    async fn send_raw(&mut self, payload: &[u8]) -> Result<(), TxSubmissionError> {
        for chunk in payload.chunks(MAX_SEGMENT_PAYLOAD_LENGTH) {
            self.channel
                .enqueue_chunk(chunk.to_vec())
                .await
                .map_err(|e| TxSubmissionError::Channel(e.to_string()))?;
        }
        Ok(())
    }

    /// Receive a full CBOR message, reassembling chunks.
    ///
    /// Reads chunks until we have a complete CBOR value by checking decode success.
    async fn recv_raw(&mut self, wait: Duration) -> Result<Vec<u8>, TxSubmissionError> {
        self.recv_buf.clear();
        loop {
            let chunk = timeout(wait, self.channel.dequeue_chunk())
                .await
                .map_err(|_| TxSubmissionError::Timeout("waiting for response".into()))?
                .map_err(|e| TxSubmissionError::Channel(e.to_string()))?;
            self.recv_buf.extend_from_slice(&chunk);

            // Try to decode — if successful, we have a complete message
            let mut probe = minicbor::Decoder::new(&self.recv_buf);
            if probe.skip().is_ok() {
                return Ok(std::mem::take(&mut self.recv_buf));
            }
            // Otherwise keep reading chunks
        }
    }

    /// Run the TxSubmission2 protocol, continuously fetching transactions from the peer.
    ///
    /// Returns when the peer sends MsgDone or an error occurs.
    pub async fn run(
        &mut self,
        mempool: Arc<Mempool>,
        tx_validator: Option<Arc<dyn TxValidator>>,
    ) -> Result<TxSubmissionStats, TxSubmissionError> {
        let mut stats = TxSubmissionStats::default();

        // Step 1: Send MsgInit
        self.send_init().await?;

        // Step 2: Wait for peer's MsgInit
        self.recv_init().await?;

        debug!("TxSubmission2 client: init handshake complete");

        // Main loop: request tx IDs, filter, request bodies, validate, add to mempool
        loop {
            // Request tx IDs from peer (non-blocking first)
            let tx_ids = self
                .request_tx_ids(false, self.pending_ack, MAX_TX_IDS_REQUEST)
                .await?;
            self.pending_ack = 0;

            if tx_ids.is_empty() {
                // No transactions available — do a blocking request
                debug!("TxSubmission2: no txs available, sending blocking request");
                let tx_ids = self.request_tx_ids(true, 0, MAX_TX_IDS_REQUEST).await?;

                if tx_ids.is_empty() {
                    debug!("TxSubmission2: peer has no transactions, closing");
                    break;
                }

                self.process_tx_ids(&tx_ids, &mempool, tx_validator.as_deref(), &mut stats)
                    .await?;
            } else {
                self.process_tx_ids(&tx_ids, &mempool, tx_validator.as_deref(), &mut stats)
                    .await?;
            }
        }

        Ok(stats)
    }

    /// Process a batch of tx IDs: filter, request bodies, validate, add to mempool.
    async fn process_tx_ids(
        &mut self,
        tx_ids: &[(TransactionHash, u32)],
        mempool: &Mempool,
        tx_validator: Option<&dyn TxValidator>,
        stats: &mut TxSubmissionStats,
    ) -> Result<(), TxSubmissionError> {
        // Filter out tx IDs we already have in mempool or have seen from this peer
        let new_tx_ids: Vec<TransactionHash> = tx_ids
            .iter()
            .filter(|(hash, _)| !mempool.contains(hash) && !self.known_tx_ids.contains(hash))
            .map(|(hash, _)| *hash)
            .collect();

        // Track all tx IDs from this batch
        for (hash, _) in tx_ids {
            self.known_tx_ids.insert(*hash);
        }
        self.pending_ack += tx_ids.len() as u16;

        if new_tx_ids.is_empty() {
            trace!("TxSubmission2: all {} tx IDs already known", tx_ids.len());
            return Ok(());
        }

        debug!(
            new = new_tx_ids.len(),
            total = tx_ids.len(),
            "TxSubmission2: requesting new transactions"
        );

        // Request full transaction bodies
        let tx_bodies = self.request_txs(&new_tx_ids).await?;
        stats.received += tx_bodies.len() as u64;

        // Validate and add each transaction to mempool
        for (i, tx_cbor) in tx_bodies.iter().enumerate() {
            let tx_hash = if i < new_tx_ids.len() {
                new_tx_ids[i]
            } else {
                continue;
            };

            // Try decoding across eras (Conway=6 first, then backwards)
            let decoded = self.try_decode_and_add(tx_hash, tx_cbor, mempool, tx_validator, stats);
            if !decoded {
                debug!(hash = %tx_hash, "TxSubmission2: failed to decode tx in any era");
                stats.rejected += 1;
            }
        }

        Ok(())
    }

    /// Try to decode a transaction in multiple eras and add to mempool.
    fn try_decode_and_add(
        &self,
        tx_hash: TransactionHash,
        tx_cbor: &[u8],
        mempool: &Mempool,
        tx_validator: Option<&dyn TxValidator>,
        stats: &mut TxSubmissionStats,
    ) -> bool {
        for era in [6u16, 5, 4, 3, 2] {
            // Validate if validator available
            if let Some(validator) = tx_validator {
                if let Err(e) = validator.validate_tx(era, tx_cbor) {
                    if era == 6 {
                        debug!(
                            hash = %tx_hash,
                            "TxSubmission2: validation failed (era {era}): {e}"
                        );
                    }
                    continue;
                }
            }

            match torsten_serialization::decode_transaction(era, tx_cbor) {
                Ok(tx) => {
                    let tx_size = tx_cbor.len();
                    let fee = tx.body.fee;
                    match mempool.add_tx_with_fee(tx_hash, tx, tx_size, fee) {
                        Ok(torsten_mempool::MempoolAddResult::Added) => {
                            info!(hash = %tx_hash, "TxSubmission2: tx added to mempool");
                            stats.accepted += 1;
                        }
                        Ok(torsten_mempool::MempoolAddResult::AlreadyExists) => {
                            trace!(hash = %tx_hash, "TxSubmission2: tx already in mempool");
                            stats.duplicate += 1;
                        }
                        Err(e) => {
                            debug!(hash = %tx_hash, "TxSubmission2: mempool rejected: {e}");
                            stats.rejected += 1;
                        }
                    }
                    return true;
                }
                Err(_) => continue,
            }
        }
        false
    }

    /// Send MsgInit [6]
    async fn send_init(&mut self) -> Result<(), TxSubmissionError> {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(1).map_err(cbor_err)?;
        enc.u32(6).map_err(cbor_err)?;
        self.send_raw(&buf).await
    }

    /// Receive MsgInit [6] from peer
    async fn recv_init(&mut self) -> Result<(), TxSubmissionError> {
        let payload = self.recv_raw(RESPONSE_TIMEOUT).await?;
        let mut decoder = minicbor::Decoder::new(&payload);
        let _arr = decoder.array().map_err(cbor_err)?;
        let tag = decoder.u32().map_err(cbor_err)?;
        if tag != 6 {
            return Err(TxSubmissionError::Protocol(format!(
                "expected MsgInit (6), got {tag}"
            )));
        }
        Ok(())
    }

    /// Send MsgRequestTxIds and receive MsgReplyTxIds.
    async fn request_tx_ids(
        &mut self,
        blocking: bool,
        ack_count: u16,
        req_count: u16,
    ) -> Result<Vec<(TransactionHash, u32)>, TxSubmissionError> {
        // Send MsgRequestTxIds: [0, blocking, ack_count, req_count]
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(4).map_err(cbor_err)?;
        enc.u32(0).map_err(cbor_err)?;
        enc.bool(blocking).map_err(cbor_err)?;
        enc.u16(ack_count).map_err(cbor_err)?;
        enc.u16(req_count).map_err(cbor_err)?;
        self.send_raw(&buf).await?;

        // Receive response
        let wait_time = if blocking {
            Duration::from_secs(300) // Blocking requests can take a long time
        } else {
            RESPONSE_TIMEOUT
        };
        let payload = self.recv_raw(wait_time).await?;
        let mut decoder = minicbor::Decoder::new(&payload);
        let _arr = decoder.array().map_err(cbor_err)?;
        let tag = decoder.u32().map_err(cbor_err)?;

        match tag {
            // MsgReplyTxIds: [1, [[tx_id, size], ...]]
            1 => {
                let items_len = decoder.array().map_err(cbor_err)?.unwrap_or(0);
                let mut result = Vec::with_capacity(items_len as usize);
                for _ in 0..items_len {
                    let _inner = decoder.array().map_err(cbor_err)?;
                    let tx_hash_bytes = decoder.bytes().map_err(cbor_err)?;
                    let size = decoder.u32().map_err(cbor_err)?;

                    if tx_hash_bytes.len() == 32 {
                        // Safety: length is checked to be exactly 32 by the enclosing `if`
                        let hash_arr: [u8; 32] = tx_hash_bytes.try_into().expect("32-byte slice");
                        let tx_hash = Hash32::from_bytes(hash_arr);
                        result.push((tx_hash, size));
                    }
                }
                trace!(count = result.len(), "TxSubmission2: received tx ids");
                Ok(result)
            }
            // MsgDone: [5]
            5 => {
                debug!("TxSubmission2: peer sent MsgDone");
                Ok(vec![])
            }
            other => Err(TxSubmissionError::Protocol(format!(
                "expected MsgReplyTxIds (1) or MsgDone (5), got {other}"
            ))),
        }
    }

    /// Send MsgRequestTxs and receive MsgReplyTxs.
    async fn request_txs(
        &mut self,
        tx_ids: &[TransactionHash],
    ) -> Result<Vec<Vec<u8>>, TxSubmissionError> {
        if tx_ids.is_empty() {
            return Ok(vec![]);
        }

        // Send MsgRequestTxs: [2, [tx_id, ...]]
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).map_err(cbor_err)?;
        enc.u32(2).map_err(cbor_err)?;
        enc.array(tx_ids.len() as u64).map_err(cbor_err)?;
        for tx_id in tx_ids {
            enc.bytes(tx_id.as_bytes()).map_err(cbor_err)?;
        }
        self.send_raw(&buf).await?;

        // Receive MsgReplyTxs: [3, [tx_cbor, ...]]
        let payload = self.recv_raw(RESPONSE_TIMEOUT).await?;
        let mut decoder = minicbor::Decoder::new(&payload);
        let _arr = decoder.array().map_err(cbor_err)?;
        let tag = decoder.u32().map_err(cbor_err)?;

        if tag != 3 {
            return Err(TxSubmissionError::Protocol(format!(
                "expected MsgReplyTxs (3), got {tag}"
            )));
        }

        let items_len = decoder.array().map_err(cbor_err)?.unwrap_or(0);
        let mut result = Vec::with_capacity(items_len as usize);
        for _ in 0..items_len {
            let tx_cbor = decoder.bytes().map_err(cbor_err)?;
            result.push(tx_cbor.to_vec());
        }

        debug!(count = result.len(), "TxSubmission2: received tx bodies");
        Ok(result)
    }
}

/// Statistics from a TxSubmission2 session.
#[derive(Debug, Default, Clone)]
pub struct TxSubmissionStats {
    /// Number of transactions received from peer
    pub received: u64,
    /// Number of transactions accepted into mempool
    pub accepted: u64,
    /// Number of transactions rejected (validation failure)
    pub rejected: u64,
    /// Number of duplicate transactions (already in mempool)
    pub duplicate: u64,
}

/// Errors from the TxSubmission2 client.
#[derive(Debug, thiserror::Error)]
pub enum TxSubmissionError {
    #[error("CBOR error: {0}")]
    Cbor(String),
    #[error("Channel error: {0}")]
    Channel(String),
    #[error("Protocol error: {0}")]
    Protocol(String),
    #[error("Timeout: {0}")]
    Timeout(String),
}

fn cbor_err<T: std::fmt::Display>(e: T) -> TxSubmissionError {
    TxSubmissionError::Cbor(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stats_default() {
        let stats = TxSubmissionStats::default();
        assert_eq!(stats.received, 0);
        assert_eq!(stats.accepted, 0);
        assert_eq!(stats.rejected, 0);
        assert_eq!(stats.duplicate, 0);
    }
}
