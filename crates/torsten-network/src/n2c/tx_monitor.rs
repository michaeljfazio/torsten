use std::sync::Arc;
use tokio::sync::RwLock;
use torsten_mempool::Mempool;
use torsten_primitives::hash::Hash32;
use tracing::{debug, warn};

use crate::multiplexer::Segment;
use crate::query_handler::QueryHandler;

use super::{N2CServerError, MINI_PROTOCOL_TX_MONITOR};

/// Per-client LocalTxMonitor cursor state
pub(crate) struct TxMonitorCursor {
    /// Snapshot of mempool tx hashes taken at MsgAcquire
    pub(crate) snapshot: Vec<torsten_primitives::hash::TransactionHash>,
    /// Current position within the snapshot for NextTx iteration
    pub(crate) position: usize,
    /// Whether the client has acquired a mempool snapshot
    pub(crate) acquired: bool,
}

/// Handle LocalTxMonitor messages
///
/// Protocol flow:
///   Client: MsgAcquire           → Server: MsgAcquired(slot_no)
///   Client: MsgHasTx(tx_id)      → Server: MsgHasTxReply(bool)
///   Client: MsgNextTx            → Server: MsgNextTxReply(maybe_tx)
///   Client: MsgGetSizes          → Server: MsgGetSizesReply(sizes)
///   Client: MsgRelease           → (back to idle)
///   Client: MsgDone              → (end)
///
/// Message tags (CDDL spec):
///   0: MsgDone        [0]
///   1: MsgAcquire     [1]  (also MsgAwaitAcquire from StAcquired)
///   2: MsgAcquired    [2, slot_no]
///   3: MsgRelease     [3]
///   5: MsgNextTx      [5]
///   6: MsgReplyNextTx [6, null | [era_id, tx_bytes]]
///   7: MsgHasTx       [7, tx_id_bytes]
///   8: MsgReplyHasTx  [8, bool]
///   9: MsgGetSizes    [9]
///  10: MsgReplyGetSizes [10, [capacity, size, num_txs]]
pub(crate) async fn handle_tx_monitor(
    payload: &[u8],
    mempool: &Arc<Mempool>,
    query_handler: &Arc<RwLock<QueryHandler>>,
    cursor: &mut TxMonitorCursor,
) -> Result<Option<Segment>, N2CServerError> {
    let mut decoder = minicbor::Decoder::new(payload);

    let msg_tag = match decoder.array() {
        Ok(Some(len)) if len >= 1 => decoder
            .u32()
            .map_err(|e| N2CServerError::Protocol(format!("bad tx monitor msg tag: {e}")))?,
        Ok(None) => decoder
            .u32()
            .map_err(|e| N2CServerError::Protocol(format!("bad tx monitor msg tag: {e}")))?,
        _ => {
            return Err(N2CServerError::Protocol(
                "invalid tx monitor message".into(),
            ))
        }
    };

    match msg_tag {
        0 => {
            // MsgDone
            debug!("LocalTxMonitor: MsgDone");
            Ok(None)
        }
        1 => {
            // MsgAcquire / MsgAwaitAcquire → MsgAcquired(slot_no)
            // Take a snapshot of the mempool for cursor-based iteration
            cursor.snapshot = mempool.tx_hashes_ordered();
            cursor.position = 0;
            cursor.acquired = true;
            let tip_slot = {
                let handler = query_handler.read().await;
                handler.state().tip.point.slot().map(|s| s.0).unwrap_or(0)
            };
            debug!(
                tip_slot,
                snapshot_size = cursor.snapshot.len(),
                "LocalTxMonitor: MsgAcquire"
            );
            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);
            enc.array(2)
                .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
            enc.u32(2)
                .map_err(|e| N2CServerError::Protocol(e.to_string()))?; // MsgAcquired
            enc.u64(tip_slot)
                .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
            Ok(Some(Segment {
                transmission_time: 0,
                protocol_id: MINI_PROTOCOL_TX_MONITOR,
                is_responder: true,
                payload: buf,
            }))
        }
        3 => {
            // MsgRelease — clear the acquired snapshot
            cursor.snapshot.clear();
            cursor.position = 0;
            cursor.acquired = false;
            debug!("LocalTxMonitor: MsgRelease");
            Ok(None)
        }
        7 => {
            // MsgHasTx(tx_id) → MsgReplyHasTx(bool)
            let tx_id_bytes = decoder.bytes().unwrap_or(&[]);
            let has_tx = if tx_id_bytes.len() == 32 {
                // Safety: tx_id_bytes.len() == 32 is checked by the enclosing `if`
                let tx_hash = Hash32::from_bytes(tx_id_bytes.try_into().expect("32-byte slice"));
                let exists = mempool.contains(&tx_hash);
                debug!("LocalTxMonitor: MsgHasTx {} → {exists}", tx_hash.to_hex());
                exists
            } else {
                debug!("LocalTxMonitor: MsgHasTx with invalid tx_id length");
                false
            };

            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);
            enc.array(2)
                .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
            enc.u32(8)
                .map_err(|e| N2CServerError::Protocol(e.to_string()))?; // MsgReplyHasTx
            enc.bool(has_tx)
                .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
            Ok(Some(Segment {
                transmission_time: 0,
                protocol_id: MINI_PROTOCOL_TX_MONITOR,
                is_responder: true,
                payload: buf,
            }))
        }
        5 => {
            // MsgNextTx → MsgReplyNextTx(null | [era_id, tx_bytes])
            // Iterate through the snapshot taken at MsgAcquire
            debug!(
                position = cursor.position,
                snapshot_len = cursor.snapshot.len(),
                "LocalTxMonitor: MsgNextTx"
            );
            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);

            // Find the next tx in the snapshot that still exists in the mempool
            let mut found = false;
            while cursor.position < cursor.snapshot.len() {
                let tx_hash = cursor.snapshot[cursor.position];
                cursor.position += 1;
                if let Some(tx_cbor) = mempool.get_tx_cbor(&tx_hash) {
                    debug!("LocalTxMonitor: MsgReplyNextTx with tx {}", tx_hash);
                    enc.array(2)
                        .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
                    enc.u32(6)
                        .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
                    enc.array(2)
                        .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
                    enc.u32(6)
                        .map_err(|e| N2CServerError::Protocol(e.to_string()))?; // era 6 = Conway
                    enc.bytes(&tx_cbor)
                        .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
                    found = true;
                    break;
                }
                // Tx was removed from mempool since snapshot — skip it
            }

            if !found {
                // End of snapshot or empty — return null
                enc.array(2)
                    .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
                enc.u32(6)
                    .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
                enc.null()
                    .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
            }
            Ok(Some(Segment {
                transmission_time: 0,
                protocol_id: MINI_PROTOCOL_TX_MONITOR,
                is_responder: true,
                payload: buf,
            }))
        }
        9 => {
            // MsgGetSizes → MsgReplyGetSizes([capacity, size, num_txs])
            let num_txs = mempool.len() as u64;
            let size_bytes = mempool.total_bytes() as u64;
            let capacity = mempool.capacity() as u64;
            debug!(
                "LocalTxMonitor: MsgGetSizes → cap={capacity}, size={size_bytes}, txs={num_txs}"
            );

            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);
            enc.array(2)
                .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
            enc.u32(10)
                .map_err(|e| N2CServerError::Protocol(e.to_string()))?; // MsgReplyGetSizes
            enc.array(3)
                .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
            enc.u64(capacity)
                .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
            enc.u64(size_bytes)
                .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
            enc.u64(num_txs)
                .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
            Ok(Some(Segment {
                transmission_time: 0,
                protocol_id: MINI_PROTOCOL_TX_MONITOR,
                is_responder: true,
                payload: buf,
            }))
        }
        other => {
            warn!("Unknown LocalTxMonitor message tag: {other}");
            Ok(None)
        }
    }
}
