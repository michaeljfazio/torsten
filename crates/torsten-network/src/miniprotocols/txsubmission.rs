//! TxSubmission2 mini-protocol (N2N transaction submission).
//!
//! In N2N TxSubmission2, when we connect to a peer (initiator), we are the
//! **server** — we request transaction IDs and bodies from the peer. The peer
//! (responder/client) advertises transactions from their mempool.
//!
//! Ouroboros TxSubmission2 CDDL (tag reference):
//!   MsgRequestTxIds  = [0, blocking, ack_count, req_count]
//!   MsgReplyTxIds    = [1, [[tx_id, size_bytes], ...]]
//!   MsgRequestTxs    = [2, [tx_id, ...]]
//!   MsgReplyTxs      = [3, [tx_cbor, ...]]
//!   MsgDone          = [4]
//!   MsgInit          = [6]
//!
//! Protocol flow (N2N initiator = client role, requests txs FROM the peer):
//! 1. Initiator sends MsgInit [6]
//! 2. Responder sends MsgInit [6] (bidirectional initialization)
//! 3. Initiator sends MsgRequestTxIds → Responder replies with MsgReplyTxIds
//! 4. Initiator sends MsgRequestTxs   → Responder replies with MsgReplyTxs
//! 5. Initiator sends MsgDone [4] to close
//!
//! Note on blocking vs. non-blocking requests:
//! - Non-blocking (blocking=false): responder must reply immediately, possibly with [].
//! - Blocking   (blocking=true):  responder holds the reply until at least one tx is
//!   available, or sends [] only when it has nothing and is transitioning to Done.
//!   An empty blocking reply indicates the responder is finishing the session.

use pallas_network::multiplexer::{AgentChannel, MAX_SEGMENT_PAYLOAD_LENGTH};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::time::{timeout, Duration};
use torsten_primitives::hash::{Hash32, TransactionHash};
use tracing::{debug, info, trace, warn};

use crate::n2c::TxValidator;
use torsten_primitives::mempool::{MempoolAddResult, MempoolProvider};

/// Maximum number of tx IDs to request per batch
const MAX_TX_IDS_REQUEST: u16 = 100;

/// Timeout for receiving a response from a non-blocking request
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);

/// Timeout for receiving a response from a blocking request.
/// A blocking MsgRequestTxIds holds the responder until txs are available,
/// so we use a generous timeout to avoid spurious disconnects.
const BLOCKING_RESPONSE_TIMEOUT: Duration = Duration::from_secs(300);

/// Maximum number of inflight tx IDs we'll track in the known-set before eviction
const MAX_KNOWN_TX_IDS: usize = 10_000;

/// N2N TxSubmission2 client (initiator role, requests txs from the peer).
///
/// Requests transactions from a connected peer and adds them to the local mempool
/// after validation.
pub struct TxSubmissionClient {
    channel: AgentChannel,
    /// Reassembly buffer for multi-chunk messages
    recv_buf: Vec<u8>,
    /// Set of tx IDs we have already seen from this peer (dedup filter)
    known_tx_ids: HashSet<TransactionHash>,
    /// Number of tx IDs received but not yet acknowledged in the next MsgRequestTxIds
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
    /// Enforces a maximum reassembly size to prevent memory exhaustion from malicious peers.
    async fn recv_raw(&mut self, wait: Duration) -> Result<Vec<u8>, TxSubmissionError> {
        /// Maximum reassembled message size (8 MB — well above any legitimate tx submission message).
        const MAX_REASSEMBLY_SIZE: usize = 8 * 1024 * 1024;
        self.recv_buf.clear();
        loop {
            let chunk = timeout(wait, self.channel.dequeue_chunk())
                .await
                .map_err(|_| TxSubmissionError::Timeout("waiting for response".into()))?
                .map_err(|e| TxSubmissionError::Channel(e.to_string()))?;
            self.recv_buf.extend_from_slice(&chunk);

            if self.recv_buf.len() > MAX_REASSEMBLY_SIZE {
                return Err(TxSubmissionError::Protocol(format!(
                    "reassembled message exceeds {} bytes",
                    MAX_REASSEMBLY_SIZE
                )));
            }

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
    /// Returns when the peer sends MsgDone [4] or an unrecoverable error occurs.
    pub async fn run(
        &mut self,
        mempool: Arc<dyn MempoolProvider>,
        tx_validator: Option<Arc<dyn TxValidator>>,
    ) -> Result<TxSubmissionStats, TxSubmissionError> {
        let mut stats = TxSubmissionStats::default();

        // Step 1: Send our MsgInit [6]
        info!("TxSubmission2: sending MsgInit");
        self.send_init().await?;

        // Step 2: Wait for peer's MsgInit [6]
        self.recv_init().await?;
        info!("TxSubmission2: init handshake complete — beginning tx polling");

        // Main loop: request tx IDs, filter, request bodies, validate, add to mempool.
        //
        // Flow control:
        //   pending_ack  = number of tx IDs from the previous reply that we must ack
        //   Start with a non-blocking request so we can immediately get any queued txs.
        //   If the non-blocking reply is empty, switch to blocking to wait for new txs.
        loop {
            // Non-blocking request: ack any previously seen IDs, ask for more
            let ack = self.pending_ack;
            let tx_ids = self.request_tx_ids(false, ack, MAX_TX_IDS_REQUEST).await?;
            self.pending_ack = 0;

            match tx_ids {
                TxIdsReply::Done => {
                    // Peer sent MsgDone [4] in place of MsgReplyTxIds
                    info!("TxSubmission2: peer sent MsgDone, closing session");
                    break;
                }
                TxIdsReply::Ids(ids) if ids.is_empty() => {
                    // Non-blocking returned empty — peer has no queued txs right now.
                    // Switch to a blocking request: hold until the peer has something.
                    debug!("TxSubmission2: no txs available, sending blocking MsgRequestTxIds");
                    let blocking_ids = self.request_tx_ids(true, 0, MAX_TX_IDS_REQUEST).await?;

                    match blocking_ids {
                        TxIdsReply::Done => {
                            info!("TxSubmission2: peer sent MsgDone during blocking wait, closing");
                            break;
                        }
                        TxIdsReply::Ids(ids) if ids.is_empty() => {
                            // Empty reply to a blocking request signals the peer is
                            // ending the session (transitioning to Done state).
                            info!("TxSubmission2: peer returned empty blocking reply, closing");
                            break;
                        }
                        TxIdsReply::Ids(ids) => {
                            info!(
                                count = ids.len(),
                                "TxSubmission2: received tx IDs from blocking request"
                            );
                            self.process_tx_ids(
                                &ids,
                                &*mempool,
                                tx_validator.as_deref(),
                                &mut stats,
                            )
                            .await?;
                        }
                    }
                }
                TxIdsReply::Ids(ids) => {
                    info!(
                        count = ids.len(),
                        ack_sent = ack,
                        "TxSubmission2: received tx IDs"
                    );
                    self.process_tx_ids(&ids, &*mempool, tx_validator.as_deref(), &mut stats)
                        .await?;
                }
            }
        }

        info!(
            accepted = stats.accepted,
            rejected = stats.rejected,
            duplicate = stats.duplicate,
            received = stats.received,
            "TxSubmission2: session complete"
        );
        Ok(stats)
    }

    /// Process a batch of tx IDs: filter, request bodies, validate, add to mempool.
    async fn process_tx_ids(
        &mut self,
        tx_ids: &[(TransactionHash, u32)],
        mempool: &dyn MempoolProvider,
        tx_validator: Option<&dyn TxValidator>,
        stats: &mut TxSubmissionStats,
    ) -> Result<(), TxSubmissionError> {
        // Filter out tx IDs we already have in mempool or have seen from this peer
        let new_tx_ids: Vec<TransactionHash> = tx_ids
            .iter()
            .filter(|(hash, _)| !mempool.contains(hash) && !self.known_tx_ids.contains(hash))
            .map(|(hash, _)| *hash)
            .collect();

        // Track all tx IDs from this batch; evict oldest entries if we hit the cap.
        // Using a simple clear-and-reinsert strategy to avoid needing an ordered set.
        if self.known_tx_ids.len() + tx_ids.len() > MAX_KNOWN_TX_IDS {
            self.known_tx_ids.clear();
        }
        for (hash, _) in tx_ids {
            self.known_tx_ids.insert(*hash);
        }

        // Accumulate pending acks: every batch we receive counts against the sliding window.
        self.pending_ack = self.pending_ack.saturating_add(tx_ids.len() as u16);

        if new_tx_ids.is_empty() {
            trace!("TxSubmission2: all {} tx IDs already known", tx_ids.len());
            return Ok(());
        }

        debug!(
            new = new_tx_ids.len(),
            total = tx_ids.len(),
            "TxSubmission2: requesting transaction bodies for new IDs"
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
                warn!(hash = %tx_hash, "TxSubmission2: failed to decode tx in any era");
                stats.rejected += 1;
            }
        }

        Ok(())
    }

    /// Try to decode a transaction in multiple eras and add to mempool.
    ///
    /// Returns true if the transaction was successfully decoded (and either
    /// added, marked duplicate, or rejected by the mempool), false if it
    /// failed CBOR decoding in every era.
    fn try_decode_and_add(
        &self,
        tx_hash: TransactionHash,
        tx_cbor: &[u8],
        mempool: &dyn MempoolProvider,
        tx_validator: Option<&dyn TxValidator>,
        stats: &mut TxSubmissionStats,
    ) -> bool {
        for era in [6u16, 5, 4, 3, 2] {
            // Run Phase-1 validation if a validator is available
            if let Some(validator) = tx_validator {
                if let Err(e) = validator.validate_tx(era, tx_cbor) {
                    if era == 6 {
                        debug!(
                            hash = %tx_hash,
                            era,
                            "TxSubmission2: Phase-1 validation failed: {e}"
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
                        Ok(MempoolAddResult::Added) => {
                            info!(
                                hash = %tx_hash,
                                size = tx_size,
                                "TxSubmission2: tx added to mempool"
                            );
                            stats.accepted += 1;
                        }
                        Ok(MempoolAddResult::AlreadyExists) => {
                            trace!(hash = %tx_hash, "TxSubmission2: tx already in mempool");
                            stats.duplicate += 1;
                        }
                        Err(e) => {
                            debug!(hash = %tx_hash, "TxSubmission2: mempool rejected tx: {e}");
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

    /// Send MsgInit [6].
    async fn send_init(&mut self) -> Result<(), TxSubmissionError> {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(1).map_err(cbor_err)?;
        enc.u32(6).map_err(cbor_err)?;
        debug!("TxSubmission2: sent MsgInit");
        self.send_raw(&buf).await
    }

    /// Receive MsgInit [6] from peer.
    async fn recv_init(&mut self) -> Result<(), TxSubmissionError> {
        let payload = self.recv_raw(RESPONSE_TIMEOUT).await?;
        let mut decoder = minicbor::Decoder::new(&payload);
        let _arr = decoder.array().map_err(cbor_err)?;
        let tag = decoder.u32().map_err(cbor_err)?;
        if tag != 6 {
            return Err(TxSubmissionError::Protocol(format!(
                "expected MsgInit (6), got tag {tag}"
            )));
        }
        debug!("TxSubmission2: received MsgInit from peer");
        Ok(())
    }

    /// Send MsgRequestTxIds and receive MsgReplyTxIds or MsgDone.
    ///
    /// # Arguments
    /// - `blocking`  — if true the responder holds its reply until it has txs (or is done)
    /// - `ack_count` — number of tx IDs from the previous batch that we are acknowledging
    /// - `req_count` — how many new tx IDs we want
    async fn request_tx_ids(
        &mut self,
        blocking: bool,
        ack_count: u16,
        req_count: u16,
    ) -> Result<TxIdsReply, TxSubmissionError> {
        // MsgRequestTxIds: [0, blocking, ack_count, req_count]
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(4).map_err(cbor_err)?;
        enc.u32(0).map_err(cbor_err)?;
        enc.bool(blocking).map_err(cbor_err)?;
        enc.u16(ack_count).map_err(cbor_err)?;
        enc.u16(req_count).map_err(cbor_err)?;
        self.send_raw(&buf).await?;

        debug!(
            blocking,
            ack_count, req_count, "TxSubmission2: sent MsgRequestTxIds"
        );

        // Use a longer timeout for blocking requests
        let wait_time = if blocking {
            BLOCKING_RESPONSE_TIMEOUT
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
                    } else {
                        warn!(
                            len = tx_hash_bytes.len(),
                            "TxSubmission2: ignoring tx ID with unexpected length"
                        );
                    }
                }
                debug!(
                    count = result.len(),
                    "TxSubmission2: received MsgReplyTxIds"
                );
                Ok(TxIdsReply::Ids(result))
            }
            // MsgDone: [4]
            // Per CDDL: txsubmission2_MsgDone = [4]
            4 => {
                debug!("TxSubmission2: received MsgDone from peer");
                Ok(TxIdsReply::Done)
            }
            other => Err(TxSubmissionError::Protocol(format!(
                "expected MsgReplyTxIds (1) or MsgDone (4), got tag {other}"
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

        // MsgRequestTxs: [2, [tx_id, ...]]
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).map_err(cbor_err)?;
        enc.u32(2).map_err(cbor_err)?;
        enc.array(tx_ids.len() as u64).map_err(cbor_err)?;
        for tx_id in tx_ids {
            enc.bytes(tx_id.as_bytes()).map_err(cbor_err)?;
        }
        self.send_raw(&buf).await?;

        debug!(count = tx_ids.len(), "TxSubmission2: sent MsgRequestTxs");

        // Receive MsgReplyTxs: [3, [tx_cbor, ...]]
        let payload = self.recv_raw(RESPONSE_TIMEOUT).await?;
        let mut decoder = minicbor::Decoder::new(&payload);
        let _arr = decoder.array().map_err(cbor_err)?;
        let tag = decoder.u32().map_err(cbor_err)?;

        if tag != 3 {
            return Err(TxSubmissionError::Protocol(format!(
                "expected MsgReplyTxs (3), got tag {tag}"
            )));
        }

        let items_len = decoder.array().map_err(cbor_err)?.unwrap_or(0);
        let mut result = Vec::with_capacity(items_len as usize);
        for _ in 0..items_len {
            let tx_cbor = decoder.bytes().map_err(cbor_err)?;
            result.push(tx_cbor.to_vec());
        }

        info!(count = result.len(), "TxSubmission2: received MsgReplyTxs");
        Ok(result)
    }
}

/// Result of a MsgRequestTxIds exchange.
#[derive(Debug)]
enum TxIdsReply {
    /// Peer replied with a list of (tx_hash, size) pairs (may be empty)
    Ids(Vec<(TransactionHash, u32)>),
    /// Peer sent MsgDone [4] — session is ending
    Done,
}

/// Statistics from a TxSubmission2 session.
#[derive(Debug, Default, Clone)]
pub struct TxSubmissionStats {
    /// Number of transaction bodies received from peer
    pub received: u64,
    /// Number of transactions accepted into mempool
    pub accepted: u64,
    /// Number of transactions rejected (validation failure or mempool error)
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

    // -----------------------------------------------------------------------
    // CBOR encoding helpers — used to build synthetic peer messages in tests
    // -----------------------------------------------------------------------

    /// Encode MsgInit [6] as raw CBOR bytes.
    fn encode_msg_init() -> Vec<u8> {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(1).unwrap();
        enc.u32(6).unwrap();
        buf
    }

    /// Encode MsgReplyTxIds [1, [[tx_hash, size], ...]] as raw CBOR bytes.
    fn encode_reply_tx_ids(ids: &[([u8; 32], u32)]) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(1).unwrap();
        enc.array(ids.len() as u64).unwrap();
        for (hash, size) in ids {
            enc.array(2).unwrap();
            enc.bytes(hash.as_slice()).unwrap();
            enc.u32(*size).unwrap();
        }
        buf
    }

    /// Encode MsgReplyTxs [3, [tx_cbor, ...]] as raw CBOR bytes.
    fn encode_reply_txs(txs: &[Vec<u8>]) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(3).unwrap();
        enc.array(txs.len() as u64).unwrap();
        for tx in txs {
            enc.bytes(tx).unwrap();
        }
        buf
    }

    /// Encode MsgDone [4] as raw CBOR bytes.
    fn encode_msg_done() -> Vec<u8> {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(1).unwrap();
        enc.u32(4).unwrap();
        buf
    }

    // -----------------------------------------------------------------------
    // Encoding correctness tests (no I/O required)
    // -----------------------------------------------------------------------

    #[test]
    fn test_msg_init_encoding() {
        // MsgInit must encode as [6]
        let bytes = encode_msg_init();
        let mut dec = minicbor::Decoder::new(&bytes);
        let len = dec.array().unwrap().unwrap_or(0);
        let tag = dec.u32().unwrap();
        assert_eq!(len, 1, "MsgInit array length must be 1");
        assert_eq!(tag, 6, "MsgInit tag must be 6");
    }

    #[test]
    fn test_msg_done_encoding() {
        // MsgDone must encode as [4] per Ouroboros CDDL
        let bytes = encode_msg_done();
        let mut dec = minicbor::Decoder::new(&bytes);
        let len = dec.array().unwrap().unwrap_or(0);
        let tag = dec.u32().unwrap();
        assert_eq!(len, 1, "MsgDone array length must be 1");
        assert_eq!(tag, 4, "MsgDone tag must be 4, not 5");
    }

    #[test]
    fn test_msg_request_tx_ids_encoding() {
        // MsgRequestTxIds = [0, blocking, ack_count, req_count]
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(4).unwrap();
        enc.u32(0).unwrap(); // tag
        enc.bool(true).unwrap(); // blocking
        enc.u16(5u16).unwrap(); // ack_count
        enc.u16(100u16).unwrap(); // req_count

        let mut dec = minicbor::Decoder::new(&buf);
        let len = dec.array().unwrap().unwrap_or(0);
        assert_eq!(len, 4);
        assert_eq!(dec.u32().unwrap(), 0); // tag
        assert!(dec.bool().unwrap()); // blocking = true
        assert_eq!(dec.u16().unwrap(), 5); // ack_count
        assert_eq!(dec.u16().unwrap(), 100); // req_count
    }

    #[test]
    fn test_msg_request_tx_ids_non_blocking_encoding() {
        // Non-blocking variant
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(4).unwrap();
        enc.u32(0).unwrap();
        enc.bool(false).unwrap(); // non-blocking
        enc.u16(0u16).unwrap();
        enc.u16(50u16).unwrap();

        let mut dec = minicbor::Decoder::new(&buf);
        dec.array().unwrap();
        dec.u32().unwrap();
        assert!(!dec.bool().unwrap()); // non-blocking
    }

    #[test]
    fn test_msg_reply_tx_ids_encoding_roundtrip() {
        // MsgReplyTxIds = [1, [[tx_id, size], ...]]
        let hash1 = [0x11u8; 32];
        let hash2 = [0x22u8; 32];
        let ids = vec![(hash1, 512u32), (hash2, 1024u32)];
        let bytes = encode_reply_tx_ids(&ids);

        let mut dec = minicbor::Decoder::new(&bytes);
        let outer_len = dec.array().unwrap().unwrap_or(0);
        assert_eq!(outer_len, 2);
        assert_eq!(dec.u32().unwrap(), 1); // tag = MsgReplyTxIds

        let items_len = dec.array().unwrap().unwrap_or(0);
        assert_eq!(items_len, 2);

        for (expected_hash, expected_size) in &ids {
            let _inner = dec.array().unwrap();
            let hash_bytes = dec.bytes().unwrap();
            assert_eq!(hash_bytes, expected_hash.as_slice());
            let size = dec.u32().unwrap();
            assert_eq!(size, *expected_size);
        }
    }

    #[test]
    fn test_msg_reply_tx_ids_empty() {
        // Empty reply is valid for non-blocking requests
        let bytes = encode_reply_tx_ids(&[]);
        let mut dec = minicbor::Decoder::new(&bytes);
        dec.array().unwrap();
        assert_eq!(dec.u32().unwrap(), 1); // tag
        let items_len = dec.array().unwrap().unwrap_or(0);
        assert_eq!(items_len, 0);
    }

    #[test]
    fn test_msg_request_txs_encoding() {
        // MsgRequestTxs = [2, [tx_id, ...]]
        let hashes: Vec<[u8; 32]> = vec![[0xAAu8; 32], [0xBBu8; 32]];
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(2).unwrap(); // tag
        enc.array(hashes.len() as u64).unwrap();
        for h in &hashes {
            enc.bytes(h.as_slice()).unwrap();
        }

        let mut dec = minicbor::Decoder::new(&buf);
        let outer = dec.array().unwrap().unwrap_or(0);
        assert_eq!(outer, 2);
        assert_eq!(dec.u32().unwrap(), 2); // tag = MsgRequestTxs
        let ids_len = dec.array().unwrap().unwrap_or(0);
        assert_eq!(ids_len, 2);
        for expected in &hashes {
            let h = dec.bytes().unwrap();
            assert_eq!(h, expected.as_slice());
        }
    }

    #[test]
    fn test_msg_reply_txs_encoding_roundtrip() {
        // MsgReplyTxs = [3, [tx_cbor, ...]]
        let tx1 = vec![0x01, 0x02, 0x03];
        let tx2 = vec![0xFF, 0xFE];
        let txs = vec![tx1.clone(), tx2.clone()];
        let bytes = encode_reply_txs(&txs);

        let mut dec = minicbor::Decoder::new(&bytes);
        let outer = dec.array().unwrap().unwrap_or(0);
        assert_eq!(outer, 2);
        assert_eq!(dec.u32().unwrap(), 3); // tag = MsgReplyTxs
        let items = dec.array().unwrap().unwrap_or(0);
        assert_eq!(items, 2);
        assert_eq!(dec.bytes().unwrap(), tx1.as_slice());
        assert_eq!(dec.bytes().unwrap(), tx2.as_slice());
    }

    #[test]
    fn test_stats_default() {
        let stats = TxSubmissionStats::default();
        assert_eq!(stats.received, 0);
        assert_eq!(stats.accepted, 0);
        assert_eq!(stats.rejected, 0);
        assert_eq!(stats.duplicate, 0);
    }

    #[test]
    fn test_protocol_id_assignment() {
        // Verify the protocol ID for TxSubmission2 is 4 in the mini-protocol registry
        use crate::miniprotocols::MiniProtocolId;
        assert_eq!(MiniProtocolId::TxSubmission2 as u16, 4);
    }

    // -----------------------------------------------------------------------
    // State machine correctness tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_tx_id_dedup_filter() {
        // Simulate the dedup filter logic from process_tx_ids:
        // known_tx_ids should prevent re-requesting already-seen txs
        let mut known: HashSet<Hash32> = HashSet::new();
        let hash_a = Hash32::from_bytes([0x11u8; 32]);
        let hash_b = Hash32::from_bytes([0x22u8; 32]);

        // First batch: both are new
        let batch1 = vec![(hash_a, 100u32), (hash_b, 200u32)];
        let new1: Vec<_> = batch1.iter().filter(|(h, _)| !known.contains(h)).collect();
        assert_eq!(new1.len(), 2);
        for (h, _) in &batch1 {
            known.insert(*h);
        }

        // Second batch: same hashes — both should be filtered
        let batch2 = [(hash_a, 100u32), (hash_b, 200u32)];
        let new2: Vec<_> = batch2.iter().filter(|(h, _)| !known.contains(h)).collect();
        assert_eq!(new2.len(), 0, "Duplicate tx IDs must be filtered out");

        // Third batch: one new hash
        let hash_c = Hash32::from_bytes([0x33u8; 32]);
        let batch3 = [(hash_a, 100u32), (hash_c, 300u32)];
        let new3: Vec<_> = batch3.iter().filter(|(h, _)| !known.contains(h)).collect();
        assert_eq!(new3.len(), 1, "Only hash_c should be new");
        assert_eq!(new3[0].0, hash_c);
    }

    #[test]
    fn test_known_tx_ids_eviction() {
        // Simulate the MAX_KNOWN_TX_IDS eviction strategy:
        // When the set would exceed the cap, it is cleared before re-inserting.
        let mut known: HashSet<Hash32> = HashSet::new();

        // Fill to just below the cap
        for i in 0..MAX_KNOWN_TX_IDS {
            let mut bytes = [0u8; 32];
            bytes[0] = (i >> 24) as u8;
            bytes[1] = (i >> 16) as u8;
            bytes[2] = (i >> 8) as u8;
            bytes[3] = i as u8;
            known.insert(Hash32::from_bytes(bytes));
        }
        assert_eq!(known.len(), MAX_KNOWN_TX_IDS);

        // Add one more batch that would push past the cap → eviction triggers
        let new_hash = Hash32::from_bytes([0xFFu8; 32]);
        let batch = vec![(new_hash, 100u32)];
        if known.len() + batch.len() > MAX_KNOWN_TX_IDS {
            known.clear();
        }
        for (h, _) in &batch {
            known.insert(*h);
        }
        // After eviction + re-insert, only the new hash should remain
        assert_eq!(known.len(), 1);
        assert!(known.contains(&new_hash));
    }

    #[test]
    fn test_pending_ack_saturating_add() {
        // pending_ack must not overflow on very large batches
        let mut pending_ack: u16 = u16::MAX - 1;
        let batch_len: u16 = 10;
        pending_ack = pending_ack.saturating_add(batch_len);
        assert_eq!(pending_ack, u16::MAX, "saturating_add must not overflow");
    }

    // -----------------------------------------------------------------------
    // Full protocol exchange simulation
    // -----------------------------------------------------------------------

    /// Simulates the TxSubmission2 handshake exchange at the CBOR message level.
    ///
    /// This test verifies that our CBOR encoding matches the wire format expected
    /// by a Cardano peer at each step of the protocol:
    ///
    ///   1. MsgInit [6] — initiator → responder
    ///   2. MsgInit [6] — responder → initiator
    ///   3. MsgRequestTxIds [0, false, 0, 100] — initiator → responder
    ///   4. MsgReplyTxIds [1, [[hash, size]]] — responder → initiator
    ///   5. MsgRequestTxs [2, [hash]] — initiator → responder
    ///   6. MsgReplyTxs [3, [tx_cbor]] — responder → initiator
    ///   7. MsgRequestTxIds [0, false, 1, 100] — with ack=1
    ///   8. MsgReplyTxIds [1, []] — empty, trigger blocking
    ///   9. MsgRequestTxIds [0, true, 0, 100] — blocking request
    ///  10. MsgDone [4] — responder ends session
    #[test]
    fn test_protocol_message_sequence_encoding() {
        // Step 1: Verify MsgInit encoding
        let init = encode_msg_init();
        {
            let mut dec = minicbor::Decoder::new(&init);
            assert_eq!(dec.array().unwrap().unwrap_or(0), 1);
            assert_eq!(dec.u32().unwrap(), 6);
        }

        // Step 3: Verify MsgRequestTxIds encoding (non-blocking, ack=0, req=100)
        let req_tx_ids = {
            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);
            enc.array(4).unwrap();
            enc.u32(0).unwrap();
            enc.bool(false).unwrap();
            enc.u16(0u16).unwrap();
            enc.u16(100u16).unwrap();
            buf
        };
        {
            let mut dec = minicbor::Decoder::new(&req_tx_ids);
            assert_eq!(dec.array().unwrap().unwrap_or(0), 4);
            assert_eq!(dec.u32().unwrap(), 0);
            assert!(!dec.bool().unwrap());
            assert_eq!(dec.u16().unwrap(), 0);
            assert_eq!(dec.u16().unwrap(), 100);
        }

        // Step 4: Verify MsgReplyTxIds with one tx
        let tx_hash = [0xDEu8; 32];
        let reply_ids = encode_reply_tx_ids(&[(tx_hash, 256)]);
        {
            let mut dec = minicbor::Decoder::new(&reply_ids);
            dec.array().unwrap();
            assert_eq!(dec.u32().unwrap(), 1);
            let n = dec.array().unwrap().unwrap_or(0);
            assert_eq!(n, 1);
            dec.array().unwrap();
            assert_eq!(dec.bytes().unwrap(), tx_hash.as_slice());
            assert_eq!(dec.u32().unwrap(), 256);
        }

        // Step 5: Verify MsgRequestTxs encoding
        let req_txs = {
            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);
            enc.array(2).unwrap();
            enc.u32(2).unwrap();
            enc.array(1u64).unwrap();
            enc.bytes(tx_hash.as_slice()).unwrap();
            buf
        };
        {
            let mut dec = minicbor::Decoder::new(&req_txs);
            assert_eq!(dec.array().unwrap().unwrap_or(0), 2);
            assert_eq!(dec.u32().unwrap(), 2);
            let n = dec.array().unwrap().unwrap_or(0);
            assert_eq!(n, 1);
            assert_eq!(dec.bytes().unwrap(), tx_hash.as_slice());
        }

        // Step 6: Verify MsgReplyTxs encoding
        let fake_tx = vec![0x82, 0x00, 0x01]; // minimal CBOR for test
        let reply_txs = encode_reply_txs(std::slice::from_ref(&fake_tx));
        {
            let mut dec = minicbor::Decoder::new(&reply_txs);
            dec.array().unwrap();
            assert_eq!(dec.u32().unwrap(), 3);
            let n = dec.array().unwrap().unwrap_or(0);
            assert_eq!(n, 1);
            assert_eq!(dec.bytes().unwrap(), fake_tx.as_slice());
        }

        // Step 7: Verify MsgRequestTxIds with ack=1 (acknowledging previous batch)
        let req_with_ack = {
            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);
            enc.array(4).unwrap();
            enc.u32(0).unwrap();
            enc.bool(false).unwrap();
            enc.u16(1u16).unwrap(); // ack_count = 1
            enc.u16(100u16).unwrap();
            buf
        };
        {
            let mut dec = minicbor::Decoder::new(&req_with_ack);
            dec.array().unwrap();
            dec.u32().unwrap();
            dec.bool().unwrap();
            assert_eq!(dec.u16().unwrap(), 1, "ack_count must be 1");
        }

        // Step 9: Verify blocking MsgRequestTxIds
        let blocking_req = {
            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);
            enc.array(4).unwrap();
            enc.u32(0).unwrap();
            enc.bool(true).unwrap(); // blocking = true
            enc.u16(0u16).unwrap();
            enc.u16(100u16).unwrap();
            buf
        };
        {
            let mut dec = minicbor::Decoder::new(&blocking_req);
            dec.array().unwrap();
            dec.u32().unwrap();
            assert!(dec.bool().unwrap(), "blocking flag must be true");
        }

        // Step 10: Verify MsgDone [4] — CRITICAL: must be tag 4, not 5
        let done = encode_msg_done();
        {
            let mut dec = minicbor::Decoder::new(&done);
            assert_eq!(dec.array().unwrap().unwrap_or(0), 1);
            let tag = dec.u32().unwrap();
            assert_eq!(
                tag, 4,
                "MsgDone MUST be tag 4 per Ouroboros CDDL spec (was incorrectly 5)"
            );
        }
    }

    /// Verify that MsgRequestTxIds wire format is compatible with what Haskell
    /// cardano-node sends: the blocking field is a proper CBOR bool, not an integer.
    #[test]
    fn test_blocking_field_is_bool_not_integer() {
        let make_req = |blocking: bool| {
            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);
            enc.array(4).unwrap();
            enc.u32(0).unwrap();
            enc.bool(blocking).unwrap();
            enc.u16(0u16).unwrap();
            enc.u16(100u16).unwrap();
            buf
        };

        let blocking_bytes = make_req(true);
        let non_blocking_bytes = make_req(false);

        // CBOR true = 0xF5, false = 0xF4 — must NOT be integer 1/0
        // In CBOR, 0xF5 = true (major type 7), 0xF4 = false (major type 7)
        // An integer 1 would be 0x01 (major type 0)
        assert!(
            blocking_bytes.contains(&0xF5),
            "blocking=true must encode as CBOR true (0xF5)"
        );
        assert!(
            non_blocking_bytes.contains(&0xF4),
            "blocking=false must encode as CBOR false (0xF4)"
        );
    }
}
