//! LocalTxMonitor server — snapshot-based mempool monitoring.
//!
//! Captures a mempool snapshot on MsgAcquire. All subsequent queries
//! (MsgNextTx, MsgHasTx, MsgGetSizes) operate on that frozen snapshot.

use minicbor::{Decoder, Encoder};
use std::collections::HashSet;

use crate::error::ProtocolError;
use crate::mux::channel::MuxChannel;
use crate::MempoolProvider;

// Message tags
const TAG_DONE: u64 = 0;
const TAG_ACQUIRE: u64 = 1; // Also MsgAwaitAcquire (same tag, different state)
const TAG_ACQUIRED: u64 = 2;
const TAG_RELEASE: u64 = 3;
const TAG_NEXT_TX: u64 = 5;
const TAG_REPLY_NEXT_TX: u64 = 6;
const TAG_HAS_TX: u64 = 7;
const TAG_REPLY_HAS_TX: u64 = 8;
const TAG_GET_SIZES: u64 = 9;
const TAG_REPLY_GET_SIZES: u64 = 10;

/// Snapshot of mempool state at a point in time.
struct MonitorSnapshot {
    /// Transaction hashes in the snapshot.
    tx_hashes: Vec<[u8; 32]>,
    /// Set for O(1) membership checks.
    tx_set: HashSet<[u8; 32]>,
    /// Total bytes of all transactions.
    total_bytes: usize,
    /// Mempool capacity in transactions.
    capacity: usize,
    /// Index of next tx to yield for MsgNextTx.
    next_tx_index: usize,
    /// Slot number when snapshot was taken.
    /// Slot when snapshot was taken (used in MsgAcquired response).
    #[allow(dead_code)]
    slot: u64,
}

/// LocalTxMonitor server.
pub struct LocalTxMonitorServer;

impl LocalTxMonitorServer {
    /// Run the LocalTxMonitor server loop.
    ///
    /// `current_slot` provides the current slot number for MsgAcquired responses.
    pub async fn run<M: MempoolProvider>(
        channel: &mut MuxChannel,
        mempool: &M,
        current_slot: impl Fn() -> u64 + Send,
    ) -> Result<(), ProtocolError> {
        let mut snapshot: Option<MonitorSnapshot> = None;

        loop {
            let msg_bytes = channel.recv().await.map_err(ProtocolError::from)?;
            let mut dec = Decoder::new(&msg_bytes);

            let _arr_len = dec.array().map_err(|e| ProtocolError::CborDecode {
                protocol: "LocalTxMonitor",
                reason: e.to_string(),
            })?;
            let tag = dec.u64().map_err(|e| ProtocolError::CborDecode {
                protocol: "LocalTxMonitor",
                reason: e.to_string(),
            })?;

            match tag {
                TAG_DONE => {
                    return Ok(());
                }
                TAG_ACQUIRE => {
                    // MsgAcquire (from StIdle) or MsgAwaitAcquire (from StAcquired)
                    // Both capture a new snapshot.
                    let snap = mempool.snapshot();
                    let slot = current_slot();

                    // Convert tx hashes to [u8; 32] arrays
                    let tx_hashes: Vec<[u8; 32]> = snap
                        .tx_hashes
                        .iter()
                        .map(|h| {
                            let mut arr = [0u8; 32];
                            arr.copy_from_slice(h.as_ref());
                            arr
                        })
                        .collect();
                    let tx_set: HashSet<[u8; 32]> = tx_hashes.iter().copied().collect();

                    snapshot = Some(MonitorSnapshot {
                        tx_hashes,
                        tx_set,
                        total_bytes: snap.total_bytes,
                        capacity: mempool.capacity(),
                        next_tx_index: 0,
                        slot,
                    });

                    // Send MsgAcquired(slot)
                    let mut buf = Vec::new();
                    let mut enc = Encoder::new(&mut buf);
                    enc.array(2).expect("infallible");
                    enc.u64(TAG_ACQUIRED).expect("infallible");
                    enc.u64(slot).expect("infallible");
                    channel.send(buf).await.map_err(ProtocolError::from)?;
                }
                TAG_RELEASE => {
                    // Release the snapshot, return to StIdle
                    snapshot = None;
                }
                TAG_NEXT_TX => {
                    let snap = snapshot.as_mut().ok_or(ProtocolError::StateViolation {
                        protocol: "LocalTxMonitor",
                        expected: "StAcquired".to_string(),
                        actual: "StIdle (no snapshot)".to_string(),
                    })?;

                    let mut buf = Vec::new();
                    let mut enc = Encoder::new(&mut buf);

                    if snap.next_tx_index < snap.tx_hashes.len() {
                        let tx_hash = &snap.tx_hashes[snap.next_tx_index];
                        snap.next_tx_index += 1;

                        // Try to get the tx CBOR from mempool
                        let tx_hash_obj = dugite_primitives::Hash::from_bytes(*tx_hash);
                        if let Some(tx_cbor) = mempool.get_tx_cbor(&tx_hash_obj) {
                            // MsgReplyNextTx with tx = [6, [era_id, tx_bytes]]
                            enc.array(2).expect("infallible");
                            enc.u64(TAG_REPLY_NEXT_TX).expect("infallible");
                            enc.bytes(&tx_cbor).expect("infallible");
                        } else {
                            // Tx was removed from mempool since snapshot — skip
                            enc.array(1).expect("infallible");
                            enc.u64(TAG_REPLY_NEXT_TX).expect("infallible");
                        }
                    } else {
                        // No more transactions — MsgReplyNextTx with no tx
                        enc.array(1).expect("infallible");
                        enc.u64(TAG_REPLY_NEXT_TX).expect("infallible");
                    }
                    channel.send(buf).await.map_err(ProtocolError::from)?;
                }
                TAG_HAS_TX => {
                    let snap = snapshot.as_ref().ok_or(ProtocolError::StateViolation {
                        protocol: "LocalTxMonitor",
                        expected: "StAcquired".to_string(),
                        actual: "StIdle (no snapshot)".to_string(),
                    })?;

                    let tx_id_bytes = dec.bytes().map_err(|e| ProtocolError::CborDecode {
                        protocol: "LocalTxMonitor",
                        reason: e.to_string(),
                    })?;
                    let mut tx_id = [0u8; 32];
                    if tx_id_bytes.len() == 32 {
                        tx_id.copy_from_slice(tx_id_bytes);
                    }

                    let has = snap.tx_set.contains(&tx_id);
                    tracing::debug!(
                        queried_txid = %hex::encode(tx_id),
                        snapshot_size = snap.tx_hashes.len(),
                        result = has,
                        "LocalTxMonitor has-tx query"
                    );

                    // MsgReplyHasTx(bool)
                    let mut buf = Vec::new();
                    let mut enc = Encoder::new(&mut buf);
                    enc.array(2).expect("infallible");
                    enc.u64(TAG_REPLY_HAS_TX).expect("infallible");
                    enc.bool(has).expect("infallible");
                    channel.send(buf).await.map_err(ProtocolError::from)?;
                }
                TAG_GET_SIZES => {
                    let snap = snapshot.as_ref().ok_or(ProtocolError::StateViolation {
                        protocol: "LocalTxMonitor",
                        expected: "StAcquired".to_string(),
                        actual: "StIdle (no snapshot)".to_string(),
                    })?;

                    // MsgReplyGetSizes = [10, [capacity, size, count]]
                    let mut buf = Vec::new();
                    let mut enc = Encoder::new(&mut buf);
                    enc.array(2).expect("infallible");
                    enc.u64(TAG_REPLY_GET_SIZES).expect("infallible");
                    enc.array(3).expect("infallible");
                    enc.u64(snap.capacity as u64).expect("infallible");
                    enc.u64(snap.total_bytes as u64).expect("infallible");
                    enc.u64(snap.tx_hashes.len() as u64).expect("infallible");
                    channel.send(buf).await.map_err(ProtocolError::from)?;
                }
                _ => {
                    return Err(ProtocolError::InvalidMessage {
                        protocol: "LocalTxMonitor",
                        tag: tag as u8,
                        reason: format!("unexpected message tag: {tag}"),
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
    use dugite_primitives::hash::Hash;
    use dugite_primitives::mempool::{
        MempoolAddError, MempoolAddResult, MempoolProvider, MempoolSnapshot,
    };
    use dugite_primitives::transaction::Transaction;
    use dugite_primitives::value::Lovelace;
    use std::collections::HashMap;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    // ─── Test infrastructure ─────────────────────────────────────────────────

    /// Mock mempool for testing. Stores tx hashes, CBOR bytes, and sizes.
    struct MockMempool {
        /// Transaction data: hash → (cbor_bytes, size).
        txs: HashMap<[u8; 32], (Vec<u8>, usize)>,
        /// Capacity in transactions.
        cap: usize,
    }

    impl MockMempool {
        fn new(cap: usize) -> Self {
            Self {
                txs: HashMap::new(),
                cap,
            }
        }

        fn with_txs(txs: Vec<([u8; 32], Vec<u8>)>, cap: usize) -> Self {
            let mut map = HashMap::new();
            for (hash, cbor) in txs {
                let size = cbor.len();
                map.insert(hash, (cbor, size));
            }
            Self { txs: map, cap }
        }
    }

    impl MempoolProvider for MockMempool {
        fn add_tx(
            &self,
            _: Hash<32>,
            _: Transaction,
            _: usize,
        ) -> Result<MempoolAddResult, MempoolAddError> {
            Ok(MempoolAddResult::Added)
        }
        fn add_tx_with_fee(
            &self,
            _: Hash<32>,
            _: Transaction,
            _: usize,
            _: Lovelace,
        ) -> Result<MempoolAddResult, MempoolAddError> {
            Ok(MempoolAddResult::Added)
        }
        fn contains(&self, tx_hash: &Hash<32>) -> bool {
            self.txs.contains_key(&tx_hash.0)
        }
        fn get_tx(&self, _: &Hash<32>) -> Option<Transaction> {
            None
        }
        fn get_tx_size(&self, tx_hash: &Hash<32>) -> Option<usize> {
            self.txs.get(&tx_hash.0).map(|(_, s)| *s)
        }
        fn get_tx_cbor(&self, tx_hash: &Hash<32>) -> Option<Vec<u8>> {
            self.txs.get(&tx_hash.0).map(|(cbor, _)| cbor.clone())
        }
        fn tx_hashes_ordered(&self) -> Vec<Hash<32>> {
            self.txs.keys().map(|k| Hash::from_bytes(*k)).collect()
        }
        fn len(&self) -> usize {
            self.txs.len()
        }
        fn total_bytes(&self) -> usize {
            self.txs.values().map(|(_, s)| s).sum()
        }
        fn capacity(&self) -> usize {
            self.cap
        }
        fn snapshot(&self) -> MempoolSnapshot {
            MempoolSnapshot {
                tx_count: self.txs.len(),
                total_bytes: self.total_bytes(),
                tx_hashes: self.txs.keys().map(|k| Hash::from_bytes(*k)).collect(),
            }
        }
    }

    /// Create a test MuxChannel with egress receiver and ingress sender.
    fn make_test_channel() -> (
        MuxChannel,
        mpsc::Receiver<(u16, crate::mux::Direction, Bytes)>,
        mpsc::Sender<Bytes>,
    ) {
        let (egress_tx, egress_rx) = mpsc::channel(64);
        let (ingress_tx, ingress_rx) = mpsc::channel(64);
        let channel = MuxChannel::new(
            9, // LocalTxMonitor protocol ID
            crate::mux::Direction::ResponderDir,
            egress_tx,
            ingress_rx,
            1_000_000,
            Arc::new(AtomicUsize::new(0)),
        );
        (channel, egress_rx, ingress_tx)
    }

    /// CBOR-encode a message with the given tag and no extra fields.
    fn encode_tag_only(tag: u64) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut enc = Encoder::new(&mut buf);
        enc.array(1).unwrap();
        enc.u64(tag).unwrap();
        buf
    }

    /// CBOR-encode MsgHasTx with a tx hash.
    fn encode_has_tx(tx_hash: &[u8; 32]) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut enc = Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u64(TAG_HAS_TX).unwrap();
        enc.bytes(tx_hash).unwrap();
        buf
    }

    /// Decode the tag from a CBOR response.
    fn decode_tag(data: &[u8]) -> u64 {
        let mut dec = Decoder::new(data);
        dec.array().unwrap();
        dec.u64().unwrap()
    }

    /// Send bytes through ingress channel.
    async fn send_raw(ingress_tx: &mpsc::Sender<Bytes>, data: Vec<u8>) {
        ingress_tx.send(Bytes::from(data)).await.unwrap();
    }

    /// Receive response bytes from egress channel.
    async fn recv_raw(
        egress_rx: &mut mpsc::Receiver<(u16, crate::mux::Direction, Bytes)>,
    ) -> Vec<u8> {
        let (_, _, bytes) = egress_rx.recv().await.unwrap();
        bytes.to_vec()
    }

    // ─── MsgAcquire / MsgAcquired tests ──────────────────────────────────────

    #[tokio::test]
    async fn acquire_returns_acquired_with_slot() {
        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();
        let mempool = MockMempool::new(100);
        let current_slot = 42u64;

        let handle = tokio::spawn(async move {
            LocalTxMonitorServer::run(&mut channel, &mempool, || current_slot).await
        });

        // Send MsgAcquire (tag 1).
        send_raw(&ingress_tx, encode_tag_only(TAG_ACQUIRE)).await;

        // Should receive MsgAcquired(slot).
        let resp = recv_raw(&mut egress_rx).await;
        let mut dec = Decoder::new(&resp);
        dec.array().unwrap();
        let tag = dec.u64().unwrap();
        assert_eq!(tag, TAG_ACQUIRED);
        let slot = dec.u64().unwrap();
        assert_eq!(slot, 42);

        // Send MsgDone.
        send_raw(&ingress_tx, encode_tag_only(TAG_DONE)).await;
        handle.await.unwrap().unwrap();
    }

    // ─── MsgHasTx tests ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn has_tx_returns_true_for_present_tx() {
        let tx_hash = [0xAA; 32];
        let mempool = MockMempool::with_txs(vec![(tx_hash, vec![0x01, 0x02])], 100);
        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();

        let handle =
            tokio::spawn(
                async move { LocalTxMonitorServer::run(&mut channel, &mempool, || 10).await },
            );

        // Acquire snapshot.
        send_raw(&ingress_tx, encode_tag_only(TAG_ACQUIRE)).await;
        let _ = recv_raw(&mut egress_rx).await;

        // Query HasTx for a tx that exists.
        send_raw(&ingress_tx, encode_has_tx(&tx_hash)).await;
        let resp = recv_raw(&mut egress_rx).await;

        let mut dec = Decoder::new(&resp);
        dec.array().unwrap();
        let tag = dec.u64().unwrap();
        assert_eq!(tag, TAG_REPLY_HAS_TX);
        let has = dec.bool().unwrap();
        assert!(has, "should report tx as present");

        send_raw(&ingress_tx, encode_tag_only(TAG_DONE)).await;
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn has_tx_returns_false_for_absent_tx() {
        let mempool = MockMempool::with_txs(vec![([0xAA; 32], vec![0x01])], 100);
        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();

        let handle =
            tokio::spawn(
                async move { LocalTxMonitorServer::run(&mut channel, &mempool, || 10).await },
            );

        // Acquire.
        send_raw(&ingress_tx, encode_tag_only(TAG_ACQUIRE)).await;
        let _ = recv_raw(&mut egress_rx).await;

        // Query HasTx for a tx that does NOT exist.
        send_raw(&ingress_tx, encode_has_tx(&[0xBB; 32])).await;
        let resp = recv_raw(&mut egress_rx).await;

        let mut dec = Decoder::new(&resp);
        dec.array().unwrap();
        assert_eq!(dec.u64().unwrap(), TAG_REPLY_HAS_TX);
        assert!(!dec.bool().unwrap(), "should report tx as absent");

        send_raw(&ingress_tx, encode_tag_only(TAG_DONE)).await;
        handle.await.unwrap().unwrap();
    }

    // ─── MsgNextTx tests ────────────────────────────────────────────────────

    #[tokio::test]
    async fn next_tx_iterates_all_then_empty() {
        let tx1_hash = [0x01; 32];
        let tx2_hash = [0x02; 32];
        let tx1_cbor = vec![0xAA, 0xBB];
        let tx2_cbor = vec![0xCC, 0xDD];

        let mempool = MockMempool::with_txs(
            vec![(tx1_hash, tx1_cbor.clone()), (tx2_hash, tx2_cbor.clone())],
            100,
        );
        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();

        let handle =
            tokio::spawn(
                async move { LocalTxMonitorServer::run(&mut channel, &mempool, || 10).await },
            );

        // Acquire.
        send_raw(&ingress_tx, encode_tag_only(TAG_ACQUIRE)).await;
        let _ = recv_raw(&mut egress_rx).await;

        // Iterate: should get 2 transactions, then empty.
        let mut received_cbors = Vec::new();
        for _ in 0..2 {
            send_raw(&ingress_tx, encode_tag_only(TAG_NEXT_TX)).await;
            let resp = recv_raw(&mut egress_rx).await;
            let mut dec = Decoder::new(&resp);
            let arr_len = dec.array().unwrap();
            let tag = dec.u64().unwrap();
            assert_eq!(tag, TAG_REPLY_NEXT_TX);
            if arr_len == Some(2) {
                // Has tx CBOR.
                let cbor = dec.bytes().unwrap().to_vec();
                received_cbors.push(cbor);
            }
        }
        assert_eq!(received_cbors.len(), 2, "should receive 2 transactions");

        // Third NextTx → empty (array len 1, no tx body).
        send_raw(&ingress_tx, encode_tag_only(TAG_NEXT_TX)).await;
        let resp = recv_raw(&mut egress_rx).await;
        let mut dec = Decoder::new(&resp);
        let arr_len = dec.array().unwrap();
        assert_eq!(dec.u64().unwrap(), TAG_REPLY_NEXT_TX);
        assert_eq!(arr_len, Some(1), "empty NextTx reply should be array(1)");

        send_raw(&ingress_tx, encode_tag_only(TAG_DONE)).await;
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn next_tx_empty_mempool() {
        let mempool = MockMempool::new(100);
        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();

        let handle =
            tokio::spawn(
                async move { LocalTxMonitorServer::run(&mut channel, &mempool, || 10).await },
            );

        // Acquire.
        send_raw(&ingress_tx, encode_tag_only(TAG_ACQUIRE)).await;
        let _ = recv_raw(&mut egress_rx).await;

        // NextTx on empty snapshot → immediately empty.
        send_raw(&ingress_tx, encode_tag_only(TAG_NEXT_TX)).await;
        let resp = recv_raw(&mut egress_rx).await;
        let mut dec = Decoder::new(&resp);
        let arr_len = dec.array().unwrap();
        assert_eq!(dec.u64().unwrap(), TAG_REPLY_NEXT_TX);
        assert_eq!(arr_len, Some(1), "empty mempool NextTx = array(1)");

        send_raw(&ingress_tx, encode_tag_only(TAG_DONE)).await;
        handle.await.unwrap().unwrap();
    }

    // ─── MsgGetSizes tests ──────────────────────────────────────────────────

    #[tokio::test]
    async fn get_sizes_reports_correct_values() {
        // Create mempool with known sizes.
        let mempool = MockMempool::with_txs(
            vec![
                ([0x01; 32], vec![0; 100]), // 100 bytes
                ([0x02; 32], vec![0; 200]), // 200 bytes
            ],
            500,
        );
        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();

        let handle =
            tokio::spawn(
                async move { LocalTxMonitorServer::run(&mut channel, &mempool, || 10).await },
            );

        // Acquire.
        send_raw(&ingress_tx, encode_tag_only(TAG_ACQUIRE)).await;
        let _ = recv_raw(&mut egress_rx).await;

        // GetSizes.
        send_raw(&ingress_tx, encode_tag_only(TAG_GET_SIZES)).await;
        let resp = recv_raw(&mut egress_rx).await;

        // Response: [10, [capacity, total_bytes, tx_count]]
        let mut dec = Decoder::new(&resp);
        dec.array().unwrap(); // outer
        assert_eq!(dec.u64().unwrap(), TAG_REPLY_GET_SIZES);
        dec.array().unwrap(); // inner [capacity, size, count]
        let capacity = dec.u64().unwrap();
        let total_bytes = dec.u64().unwrap();
        let tx_count = dec.u64().unwrap();

        assert_eq!(capacity, 500);
        assert_eq!(total_bytes, 300); // 100 + 200
        assert_eq!(tx_count, 2);

        send_raw(&ingress_tx, encode_tag_only(TAG_DONE)).await;
        handle.await.unwrap().unwrap();
    }

    // ─── MsgRelease tests ───────────────────────────────────────────────────

    #[tokio::test]
    async fn release_drops_snapshot_and_allows_reacquire() {
        let mempool = MockMempool::with_txs(vec![([0x01; 32], vec![0xAA])], 100);
        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();

        let handle =
            tokio::spawn(
                async move { LocalTxMonitorServer::run(&mut channel, &mempool, || 10).await },
            );

        // Acquire.
        send_raw(&ingress_tx, encode_tag_only(TAG_ACQUIRE)).await;
        let _ = recv_raw(&mut egress_rx).await;

        // Release.
        send_raw(&ingress_tx, encode_tag_only(TAG_RELEASE)).await;

        // Re-acquire should succeed.
        send_raw(&ingress_tx, encode_tag_only(TAG_ACQUIRE)).await;
        let resp = recv_raw(&mut egress_rx).await;
        assert_eq!(decode_tag(&resp), TAG_ACQUIRED);

        send_raw(&ingress_tx, encode_tag_only(TAG_DONE)).await;
        handle.await.unwrap().unwrap();
    }

    // ─── MsgAwaitAcquire (re-snapshot) test ─────────────────────────────────

    #[tokio::test]
    async fn await_acquire_from_acquired_state() {
        // MsgAwaitAcquire (tag 1) sent from StAcquired should capture a new snapshot.
        // Both MsgAcquire and MsgAwaitAcquire share tag 1.
        let mempool = MockMempool::with_txs(vec![([0x01; 32], vec![0xAA])], 100);
        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();
        let slot = Arc::new(std::sync::atomic::AtomicU64::new(10));
        let slot_ref = slot.clone();

        let handle = tokio::spawn(async move {
            LocalTxMonitorServer::run(&mut channel, &mempool, move || {
                slot_ref.load(std::sync::atomic::Ordering::SeqCst)
            })
            .await
        });

        // First acquire at slot 10.
        send_raw(&ingress_tx, encode_tag_only(TAG_ACQUIRE)).await;
        let resp = recv_raw(&mut egress_rx).await;
        let mut dec = Decoder::new(&resp);
        dec.array().unwrap();
        assert_eq!(dec.u64().unwrap(), TAG_ACQUIRED);
        assert_eq!(dec.u64().unwrap(), 10);

        // Advance slot and re-acquire (MsgAwaitAcquire = same tag 1).
        slot.store(20, std::sync::atomic::Ordering::SeqCst);
        send_raw(&ingress_tx, encode_tag_only(TAG_ACQUIRE)).await;
        let resp = recv_raw(&mut egress_rx).await;
        let mut dec = Decoder::new(&resp);
        dec.array().unwrap();
        assert_eq!(dec.u64().unwrap(), TAG_ACQUIRED);
        assert_eq!(dec.u64().unwrap(), 20, "re-acquire should use new slot");

        send_raw(&ingress_tx, encode_tag_only(TAG_DONE)).await;
        handle.await.unwrap().unwrap();
    }

    // ─── State violation tests ──────────────────────────────────────────────

    #[tokio::test]
    async fn next_tx_without_acquire_returns_state_violation() {
        let mempool = MockMempool::new(100);
        let (mut channel, _egress_rx, ingress_tx) = make_test_channel();

        let handle =
            tokio::spawn(
                async move { LocalTxMonitorServer::run(&mut channel, &mempool, || 10).await },
            );

        // Send MsgNextTx without acquiring first.
        send_raw(&ingress_tx, encode_tag_only(TAG_NEXT_TX)).await;

        let result = handle.await.unwrap();
        assert!(result.is_err(), "NextTx without Acquire should error");
        match result.unwrap_err() {
            ProtocolError::StateViolation { protocol, .. } => {
                assert_eq!(protocol, "LocalTxMonitor");
            }
            other => panic!("expected StateViolation, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn has_tx_without_acquire_returns_state_violation() {
        let mempool = MockMempool::new(100);
        let (mut channel, _egress_rx, ingress_tx) = make_test_channel();

        let handle =
            tokio::spawn(
                async move { LocalTxMonitorServer::run(&mut channel, &mempool, || 10).await },
            );

        // Send MsgHasTx without acquiring first.
        send_raw(&ingress_tx, encode_has_tx(&[0xAA; 32])).await;

        let result = handle.await.unwrap();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ProtocolError::StateViolation { .. }
        ));
    }

    #[tokio::test]
    async fn get_sizes_without_acquire_returns_state_violation() {
        let mempool = MockMempool::new(100);
        let (mut channel, _egress_rx, ingress_tx) = make_test_channel();

        let handle =
            tokio::spawn(
                async move { LocalTxMonitorServer::run(&mut channel, &mempool, || 10).await },
            );

        // Send MsgGetSizes without acquiring first.
        send_raw(&ingress_tx, encode_tag_only(TAG_GET_SIZES)).await;

        let result = handle.await.unwrap();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ProtocolError::StateViolation { .. }
        ));
    }

    // ─── Unknown tag test ───────────────────────────────────────────────────

    #[tokio::test]
    async fn unknown_tag_returns_invalid_message() {
        let mempool = MockMempool::new(100);
        let (mut channel, _egress_rx, ingress_tx) = make_test_channel();

        let handle =
            tokio::spawn(
                async move { LocalTxMonitorServer::run(&mut channel, &mempool, || 10).await },
            );

        // Send a message with an unknown tag (99).
        send_raw(&ingress_tx, encode_tag_only(99)).await;

        let result = handle.await.unwrap();
        assert!(result.is_err());
        match result.unwrap_err() {
            ProtocolError::InvalidMessage { protocol, tag, .. } => {
                assert_eq!(protocol, "LocalTxMonitor");
                assert_eq!(tag, 99);
            }
            other => panic!("expected InvalidMessage, got {other:?}"),
        }
    }

    // ─── MsgDone test ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn msg_done_terminates_cleanly() {
        let mempool = MockMempool::new(100);
        let (mut channel, _egress_rx, ingress_tx) = make_test_channel();

        let handle =
            tokio::spawn(
                async move { LocalTxMonitorServer::run(&mut channel, &mempool, || 10).await },
            );

        send_raw(&ingress_tx, encode_tag_only(TAG_DONE)).await;
        let result = handle.await.unwrap();
        assert!(result.is_ok());
    }

    // ─── Snapshot isolation test ─────────────────────────────────────────────

    #[tokio::test]
    async fn snapshot_is_frozen_after_acquire() {
        // The snapshot should reflect the mempool state at the time of MsgAcquire,
        // not any changes made afterwards. Since our mock doesn't mutate, we verify
        // that iteration returns exactly the expected number of transactions.
        let mempool = MockMempool::with_txs(
            vec![
                ([0x01; 32], vec![0xAA]),
                ([0x02; 32], vec![0xBB]),
                ([0x03; 32], vec![0xCC]),
            ],
            100,
        );
        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();

        let handle =
            tokio::spawn(
                async move { LocalTxMonitorServer::run(&mut channel, &mempool, || 10).await },
            );

        // Acquire.
        send_raw(&ingress_tx, encode_tag_only(TAG_ACQUIRE)).await;
        let _ = recv_raw(&mut egress_rx).await;

        // GetSizes should report 3 txs.
        send_raw(&ingress_tx, encode_tag_only(TAG_GET_SIZES)).await;
        let resp = recv_raw(&mut egress_rx).await;
        let mut dec = Decoder::new(&resp);
        dec.array().unwrap();
        assert_eq!(dec.u64().unwrap(), TAG_REPLY_GET_SIZES);
        dec.array().unwrap();
        let _capacity = dec.u64().unwrap();
        let _total_bytes = dec.u64().unwrap();
        let tx_count = dec.u64().unwrap();
        assert_eq!(tx_count, 3);

        // Iterate all 3 txs via NextTx.
        let mut count = 0;
        for _ in 0..4 {
            send_raw(&ingress_tx, encode_tag_only(TAG_NEXT_TX)).await;
            let resp = recv_raw(&mut egress_rx).await;
            let mut dec = Decoder::new(&resp);
            let arr_len = dec.array().unwrap();
            assert_eq!(dec.u64().unwrap(), TAG_REPLY_NEXT_TX);
            if arr_len == Some(2) {
                count += 1;
            }
        }
        assert_eq!(count, 3, "should iterate exactly 3 transactions");

        send_raw(&ingress_tx, encode_tag_only(TAG_DONE)).await;
        handle.await.unwrap().unwrap();
    }

    // ─── Full protocol walk test ─────────────────────────────────────────────

    #[tokio::test]
    async fn full_protocol_walk() {
        // Complete protocol state machine walk:
        // StIdle → MsgAcquire → StAcquired → HasTx → NextTx → GetSizes → Release → StIdle → Done
        let tx_hash = [0x42; 32];
        let tx_cbor = vec![0xDE, 0xAD];
        let mempool = MockMempool::with_txs(vec![(tx_hash, tx_cbor)], 200);
        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();

        let handle =
            tokio::spawn(
                async move { LocalTxMonitorServer::run(&mut channel, &mempool, || 55).await },
            );

        // 1. Acquire
        send_raw(&ingress_tx, encode_tag_only(TAG_ACQUIRE)).await;
        let resp = recv_raw(&mut egress_rx).await;
        assert_eq!(decode_tag(&resp), TAG_ACQUIRED);

        // 2. HasTx (true)
        send_raw(&ingress_tx, encode_has_tx(&tx_hash)).await;
        let resp = recv_raw(&mut egress_rx).await;
        let mut dec = Decoder::new(&resp);
        dec.array().unwrap();
        assert_eq!(dec.u64().unwrap(), TAG_REPLY_HAS_TX);
        assert!(dec.bool().unwrap());

        // 3. HasTx (false — different hash)
        send_raw(&ingress_tx, encode_has_tx(&[0xFF; 32])).await;
        let resp = recv_raw(&mut egress_rx).await;
        let mut dec = Decoder::new(&resp);
        dec.array().unwrap();
        assert_eq!(dec.u64().unwrap(), TAG_REPLY_HAS_TX);
        assert!(!dec.bool().unwrap());

        // 4. NextTx (should get the tx)
        send_raw(&ingress_tx, encode_tag_only(TAG_NEXT_TX)).await;
        let resp = recv_raw(&mut egress_rx).await;
        let mut dec = Decoder::new(&resp);
        let arr_len = dec.array().unwrap();
        assert_eq!(dec.u64().unwrap(), TAG_REPLY_NEXT_TX);
        assert_eq!(arr_len, Some(2), "should have tx body");

        // 5. NextTx (no more txs)
        send_raw(&ingress_tx, encode_tag_only(TAG_NEXT_TX)).await;
        let resp = recv_raw(&mut egress_rx).await;
        let mut dec = Decoder::new(&resp);
        let arr_len = dec.array().unwrap();
        assert_eq!(dec.u64().unwrap(), TAG_REPLY_NEXT_TX);
        assert_eq!(arr_len, Some(1), "should be empty");

        // 6. GetSizes
        send_raw(&ingress_tx, encode_tag_only(TAG_GET_SIZES)).await;
        let resp = recv_raw(&mut egress_rx).await;
        assert_eq!(decode_tag(&resp), TAG_REPLY_GET_SIZES);

        // 7. Release
        send_raw(&ingress_tx, encode_tag_only(TAG_RELEASE)).await;

        // 8. Done
        send_raw(&ingress_tx, encode_tag_only(TAG_DONE)).await;
        handle.await.unwrap().unwrap();
    }
}
