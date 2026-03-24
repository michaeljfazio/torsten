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
                        let tx_hash_obj = torsten_primitives::Hash::from_bytes(*tx_hash);
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
