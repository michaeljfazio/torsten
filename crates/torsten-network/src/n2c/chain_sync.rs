use std::sync::Arc;
use tokio::sync::RwLock;
use torsten_primitives::hash::Hash32;
use tracing::{debug, warn};

use crate::multiplexer::Segment;
use crate::n2n_server::BlockProvider;
use crate::query_handler::QueryHandler;

use super::{N2CServerError, MINI_PROTOCOL_CHAINSYNC};

/// Per-client LocalChainSync cursor state
pub(crate) struct ChainSyncCursor {
    /// Current cursor slot (blocks after this slot will be served)
    pub(crate) cursor_slot: u64,
    /// Whether the client has found an intersection
    pub(crate) has_intersection: bool,
}

/// Handle LocalChainSync messages
///
/// Protocol flow:
///   Client: MsgFindIntersect(points) → Server: MsgIntersectFound(point, tip) | MsgIntersectNotFound(tip)
///   Client: MsgRequestNext            → Server: MsgRollForward(block, tip) | MsgRollBackward(point, tip) | MsgAwaitReply
///   Client: MsgDone                   → (end)
///
/// Message tags:
///   0: MsgRequestNext
///   1: MsgAwaitReply
///   2: MsgRollForward    [2, wrapped_header, tip]
///   3: MsgRollBackward   [3, point, tip]
///   4: MsgFindIntersect  [4, [point, ...]]
///   5: MsgIntersectFound [5, point, tip]
///   6: MsgIntersectNotFound [6, tip]
///   7: MsgDone
pub(crate) async fn handle_local_chainsync(
    payload: &[u8],
    query_handler: &Arc<RwLock<QueryHandler>>,
    block_provider: &Option<Arc<dyn BlockProvider>>,
    cursor: &mut ChainSyncCursor,
) -> Result<Option<Segment>, N2CServerError> {
    let mut decoder = minicbor::Decoder::new(payload);

    let msg_tag = match decoder.array() {
        Ok(Some(len)) if len >= 1 => decoder
            .u32()
            .map_err(|e| N2CServerError::Protocol(format!("bad chainsync msg tag: {e}")))?,
        Ok(None) => decoder
            .u32()
            .map_err(|e| N2CServerError::Protocol(format!("bad chainsync msg tag: {e}")))?,
        _ => return Err(N2CServerError::Protocol("invalid chainsync message".into())),
    };

    match msg_tag {
        0 => {
            // MsgRequestNext → MsgRollForward, MsgRollBackward, or MsgAwaitReply
            if let Some(provider) = block_provider {
                if cursor.has_intersection {
                    // Check for rollback: if client cursor is ahead of the chain tip,
                    // a rollback has occurred and we need to notify the client
                    let tip = provider.get_tip();
                    if cursor.cursor_slot > tip.slot {
                        debug!(
                            cursor_slot = cursor.cursor_slot,
                            tip_slot = tip.slot,
                            "LocalChainSync: MsgRollBackward (chain rolled back)"
                        );
                        cursor.cursor_slot = tip.slot;

                        let mut buf = Vec::new();
                        let mut enc = minicbor::Encoder::new(&mut buf);
                        // MsgRollBackward [3, point, tip]
                        enc.array(3)
                            .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
                        enc.u32(3)
                            .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
                        let tip_h = Hash32::from_bytes(tip.hash);
                        encode_point(&mut enc, tip.slot, &tip_h)?;
                        encode_tip(&mut enc, tip.slot, &tip_h, tip.block_number)?;

                        return Ok(Some(Segment {
                            transmission_time: 0,
                            protocol_id: MINI_PROTOCOL_CHAINSYNC,
                            is_responder: true,
                            payload: buf,
                        }));
                    }

                    if let Some((slot, _hash, cbor)) =
                        provider.get_next_block_after_slot(cursor.cursor_slot)
                    {
                        // Serve the next block
                        debug!(slot, "LocalChainSync: MsgRollForward");
                        cursor.cursor_slot = slot;

                        let tip = provider.get_tip();

                        // Extract era tag from block CBOR: [era_tag, ...]
                        let era_id = {
                            let mut d = minicbor::Decoder::new(&cbor);
                            d.array().ok();
                            d.u32().unwrap_or(6) // default Conway if parse fails
                        };

                        let mut buf = Vec::new();
                        let mut enc = minicbor::Encoder::new(&mut buf);
                        // MsgRollForward [2, [era_id, tagged(24, block_cbor)], tip]
                        enc.array(3)
                            .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
                        enc.u32(2)
                            .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
                        // Wrapped block: [era_id, tag(24) block_bytes]
                        enc.array(2)
                            .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
                        enc.u32(era_id)
                            .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
                        enc.tag(minicbor::data::Tag::new(24))
                            .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
                        enc.bytes(&cbor)
                            .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
                        // tip
                        let tip_h = Hash32::from_bytes(tip.hash);
                        encode_tip(&mut enc, tip.slot, &tip_h, tip.block_number)?;

                        return Ok(Some(Segment {
                            transmission_time: 0,
                            protocol_id: MINI_PROTOCOL_CHAINSYNC,
                            is_responder: true,
                            payload: buf,
                        }));
                    }
                }
            }

            // No blocks available or no block provider — await
            debug!("LocalChainSync: MsgRequestNext → MsgAwaitReply");
            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);
            enc.array(1)
                .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
            enc.u32(1)
                .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
            Ok(Some(Segment {
                transmission_time: 0,
                protocol_id: MINI_PROTOCOL_CHAINSYNC,
                is_responder: true,
                payload: buf,
            }))
        }
        4 => {
            // MsgFindIntersect(points) → MsgIntersectFound(point, tip) or MsgIntersectNotFound(tip)
            debug!("LocalChainSync: MsgFindIntersect");
            let handler = query_handler.read().await;
            let state = handler.state();
            let tip_slot = state.tip.point.slot().map(|s| s.0).unwrap_or(0);
            let tip_hash = state.tip.point.hash().copied().unwrap_or(Hash32::ZERO);
            let tip_block_no = state.block_number.0;

            // Try to find an intersection with the client's points
            let found_point = if let Some(provider) = block_provider {
                // Check each client point against our chain
                parse_client_points_with_provider(&mut decoder, provider)
            } else {
                // Fallback: check if any point matches our current tip
                parse_client_points(&mut decoder, tip_slot, &tip_hash)
            };

            let mut buf = Vec::new();
            let mut enc = minicbor::Encoder::new(&mut buf);

            if let Some((slot, hash)) = found_point {
                debug!(slot, "LocalChainSync: MsgIntersectFound");
                cursor.cursor_slot = slot;
                cursor.has_intersection = true;
                // MsgIntersectFound [5, point, tip]
                enc.array(3)
                    .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
                enc.u32(5)
                    .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
                encode_point(&mut enc, slot, &hash)?;
                encode_tip(&mut enc, tip_slot, &tip_hash, tip_block_no)?;
            } else {
                debug!("LocalChainSync: MsgIntersectNotFound");
                cursor.has_intersection = false;
                // MsgIntersectNotFound [6, tip]
                enc.array(2)
                    .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
                enc.u32(6)
                    .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
                encode_tip(&mut enc, tip_slot, &tip_hash, tip_block_no)?;
            }

            Ok(Some(Segment {
                transmission_time: 0,
                protocol_id: MINI_PROTOCOL_CHAINSYNC,
                is_responder: true,
                payload: buf,
            }))
        }
        7 => {
            // MsgDone
            debug!("LocalChainSync: MsgDone");
            cursor.has_intersection = false;
            Ok(None)
        }
        other => {
            warn!("Unknown LocalChainSync message tag: {other}");
            Ok(None)
        }
    }
}

/// Parse client points from MsgFindIntersect and check if any match our tip
fn parse_client_points(
    decoder: &mut minicbor::Decoder,
    tip_slot: u64,
    tip_hash: &Hash32,
) -> Option<(u64, Hash32)> {
    let arr_len = decoder.array().ok()??;
    for _ in 0..arr_len {
        // Each point is either [slot, hash] or "origin" (encoded as array of 0 elements)
        if let Ok(Some(point_len)) = decoder.array() {
            if point_len == 2 {
                if let (Ok(slot), Ok(hash_bytes)) = (decoder.u64(), decoder.bytes()) {
                    if slot == tip_slot && hash_bytes.len() == 32 {
                        // Safety: hash_bytes.len() == 32 is checked by the enclosing `if`
                        let point_hash =
                            Hash32::from_bytes(hash_bytes.try_into().expect("32-byte slice"));
                        if point_hash == *tip_hash {
                            return Some((slot, point_hash));
                        }
                    }
                    continue;
                }
            } else if point_len == 0 {
                // Origin point
                continue;
            }
        }
        // Skip malformed point
        let _ = decoder.skip();
    }
    None
}

/// Parse client points and check if any exist on our chain (via block provider).
/// Returns the first matching point (highest priority = first in list).
fn parse_client_points_with_provider(
    decoder: &mut minicbor::Decoder,
    provider: &Arc<dyn BlockProvider>,
) -> Option<(u64, Hash32)> {
    let arr_len = decoder.array().ok()??;
    for _ in 0..arr_len {
        if let Ok(Some(point_len)) = decoder.array() {
            if point_len == 2 {
                if let (Ok(slot), Ok(hash_bytes)) = (decoder.u64(), decoder.bytes()) {
                    if hash_bytes.len() == 32 {
                        let mut hash_arr = [0u8; 32];
                        hash_arr.copy_from_slice(hash_bytes);
                        // Check if this block exists on our chain
                        if provider.has_block(&hash_arr) {
                            let point_hash = Hash32::from_bytes(hash_arr);
                            return Some((slot, point_hash));
                        }
                    }
                    continue;
                }
            } else if point_len == 0 {
                // Origin point — always matches
                return Some((0, Hash32::ZERO));
            }
        }
        let _ = decoder.skip();
    }
    None
}

/// Encode a point as [slot, hash]
pub(crate) fn encode_point(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    slot: u64,
    hash: &Hash32,
) -> Result<(), N2CServerError> {
    enc.array(2)
        .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
    enc.u64(slot)
        .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
    enc.bytes(hash.as_bytes())
        .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
    Ok(())
}

/// Encode tip as [[slot, hash], block_no]
pub(crate) fn encode_tip(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    slot: u64,
    hash: &Hash32,
    block_no: u64,
) -> Result<(), N2CServerError> {
    enc.array(2)
        .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
    encode_point(enc, slot, hash)?;
    enc.u64(block_no)
        .map_err(|e| N2CServerError::Protocol(e.to_string()))?;
    Ok(())
}
