//! Pipelined ChainSync client for high-throughput header fetching.
//!
//! The standard pallas ChainSync client sends one MsgRequestNext and waits
//! for the response before sending the next (~300ms RTT per header). This
//! module implements Ouroboros ChainSync pipelining: sending N MsgRequestNext
//! messages at once, then batch-reading N responses. This eliminates the
//! per-header round-trip latency and can improve header throughput by 10-50x.

use pallas_network::facades::{KeepAliveHandle, KeepAliveLoop};
use pallas_network::miniprotocols::chainsync::{HeaderContent, Message, Tip};
use pallas_network::miniprotocols::handshake;
use pallas_network::miniprotocols::keepalive;
use pallas_network::miniprotocols::Point as PallasPoint;
use pallas_network::miniprotocols::{
    PROTOCOL_N2N_BLOCK_FETCH, PROTOCOL_N2N_CHAIN_SYNC, PROTOCOL_N2N_HANDSHAKE,
    PROTOCOL_N2N_KEEP_ALIVE, PROTOCOL_N2N_PEER_SHARING, PROTOCOL_N2N_TX_SUBMISSION,
};
use pallas_network::multiplexer::{Bearer, ChannelBuffer, Plexer, RunningPlexer};
use pallas_traverse::MultiEraHeader;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::ToSocketAddrs;
use tracing::{debug, trace, warn};

use crate::client::{ClientError, EbbInfo, HeaderBatchResult, HeaderInfo};
use torsten_primitives::block::{Point, Tip as TorstenTip};
use torsten_primitives::hash::Hash32;
use torsten_primitives::time::SlotNo;

/// Pipeline depth: how many MsgRequestNext to send before reading responses.
/// Higher values reduce RTT impact but use more server-side buffering.
/// Cardano-node uses a pipeline depth of ~100.
const DEFAULT_PIPELINE_DEPTH: usize = 100;

/// A ChainSync client that pipelines MsgRequestNext for high throughput.
///
/// Instead of the serial send-wait-receive pattern (1 header per RTT),
/// this client sends multiple MsgRequestNext messages before reading any
/// responses, achieving near line-rate header throughput.
pub struct PipelinedPeerClient {
    cs_buf: ChannelBuffer,
    bf_client: pallas_network::miniprotocols::blockfetch::Client,
    _keepalive: KeepAliveHandle,
    _plexer: RunningPlexer,
    /// Keep PeerSharing channel alive so the demuxer doesn't crash when the
    /// remote peer sends PeerSharing messages (~60s after connection).
    _peersharing_channel: pallas_network::multiplexer::AgentChannel,
    remote_addr: SocketAddr,
    /// Number of outstanding (sent but not yet received) pipelined requests.
    in_flight: usize,
    /// TxSubmission2 channel (available for taking by a background tx fetcher)
    txsub_channel: Option<pallas_network::multiplexer::AgentChannel>,
    /// Byron epoch length in absolute slots (10 * k). Used for correct
    /// slot computation on non-mainnet networks. 0 = use pallas defaults.
    byron_epoch_length: u64,
    /// Timeout for AwaitReply (MustReply) state. When the server is at tip
    /// and we're waiting for the next block, this controls how long to wait
    /// before declaring the connection stale. Haskell uses a randomized
    /// 135-911s for non-trustable peers; we default to 90s.
    await_reply_timeout: Duration,
}

impl PipelinedPeerClient {
    /// Connect to a remote Cardano node and set up a pipelined ChainSync channel.
    pub async fn connect(
        addr: impl ToSocketAddrs + std::fmt::Display + Copy,
        network_magic: u64,
    ) -> Result<Self, ClientError> {
        debug!("pipelined client: connecting to {addr}");

        // Connect manually so we can configure TCP keepalive before handing
        // the stream to the pallas Bearer.
        let stream = tokio::net::TcpStream::connect(addr)
            .await
            .map_err(|e| ClientError::Connection(format!("pipelined connect: {e}")))?;
        if let Err(e) = crate::tcp::configure_tcp_keepalive(&stream) {
            warn!("pipelined client: failed to set TCP keepalive: {e}");
        }
        let bearer = Bearer::Tcp(stream);

        let mut plexer = Plexer::new(bearer);

        let hs_channel = plexer.subscribe_client(PROTOCOL_N2N_HANDSHAKE);
        let cs_channel = plexer.subscribe_client(PROTOCOL_N2N_CHAIN_SYNC);
        let bf_channel = plexer.subscribe_client(PROTOCOL_N2N_BLOCK_FETCH);
        let txsub_channel = plexer.subscribe_client(PROTOCOL_N2N_TX_SUBMISSION);
        let peersharing_channel = plexer.subscribe_client(PROTOCOL_N2N_PEER_SHARING);
        let ka_channel = plexer.subscribe_client(PROTOCOL_N2N_KEEP_ALIVE);

        let plexer = plexer.spawn();

        // Handshake
        let mut hs_client = handshake::Client::new(hs_channel);
        let versions = handshake::n2n::VersionTable::v7_and_above(network_magic);
        let handshake_result = hs_client
            .handshake(versions)
            .await
            .map_err(|e| ClientError::Handshake(format!("pipelined handshake: {e}")))?;

        if let handshake::Confirmation::Rejected(reason) = handshake_result {
            return Err(ClientError::Handshake(format!(
                "pipelined handshake rejected: {reason:?}"
            )));
        }

        // Keepalive
        let ka_client = keepalive::Client::new(ka_channel);
        let keepalive = KeepAliveLoop::client(ka_client, Duration::from_secs(16)).spawn();

        // Use raw ChannelBuffer for ChainSync (bypass state machine)
        let cs_buf = ChannelBuffer::new(cs_channel);
        let bf_client = pallas_network::miniprotocols::blockfetch::Client::new(bf_channel);

        let remote_addr = format!("{addr}")
            .parse()
            .unwrap_or_else(|_| std::net::SocketAddr::from(([0, 0, 0, 0], 0)));

        debug!("pipelined client: connected to {remote_addr}");

        Ok(PipelinedPeerClient {
            cs_buf,
            bf_client,
            _keepalive: keepalive,
            _plexer: plexer,
            _peersharing_channel: peersharing_channel,
            remote_addr,
            in_flight: 0,
            txsub_channel: Some(txsub_channel),
            byron_epoch_length: 0,
            await_reply_timeout: Duration::from_secs(
                crate::tcp::TimeoutConfig::default().await_reply_timeout_secs,
            ),
        })
    }

    /// Find intersection with the remote chain.
    pub async fn find_intersect(
        &mut self,
        points: Vec<Point>,
    ) -> Result<(Option<Point>, TorstenTip), ClientError> {
        let pallas_points: Vec<PallasPoint> = points.iter().map(torsten_to_pallas_point).collect();

        // Send MsgFindIntersect
        let msg = Message::<HeaderContent>::FindIntersect(pallas_points);
        self.cs_buf
            .send_msg_chunks(&msg)
            .await
            .map_err(|e| ClientError::ChainSync(format!("send FindIntersect: {e}")))?;

        // Receive response
        let response: Message<HeaderContent> = self
            .cs_buf
            .recv_full_msg()
            .await
            .map_err(|e| ClientError::ChainSync(format!("recv intersect: {e}")))?;

        match response {
            Message::IntersectFound(point, tip) => {
                let t_point = pallas_to_torsten_point(&point);
                let t_tip = pallas_to_torsten_tip(&tip);
                debug!("pipelined: intersected at {t_point}");
                Ok((Some(t_point), t_tip))
            }
            Message::IntersectNotFound(tip) => {
                warn!("pipelined: no intersection found");
                Ok((None, pallas_to_torsten_tip(&tip)))
            }
            _ => Err(ClientError::ChainSync(
                "unexpected response to FindIntersect".into(),
            )),
        }
    }

    /// Request headers using pipelined MsgRequestNext messages.
    ///
    /// Sends `pipeline_depth` MsgRequestNext messages at once, then reads
    /// responses. This eliminates the per-header RTT latency. Returns up to
    /// `batch_size` headers (may send more requests than batch_size to keep
    /// the pipeline full).
    pub async fn request_headers_pipelined(
        &mut self,
        batch_size: usize,
    ) -> Result<HeaderBatchResult, ClientError> {
        self.request_headers_pipelined_with_depth(batch_size, DEFAULT_PIPELINE_DEPTH)
            .await
    }

    /// Request headers with a configurable pipeline depth.
    ///
    /// Sends exactly `pipeline_depth` MsgRequestNext messages, then reads all
    /// responses (up to `batch_size`). Does NOT refill the pipeline during
    /// reading — this ensures in_flight reaches 0 before returning, preventing
    /// channel buffer backpressure during the caller's block fetch phase.
    ///
    /// Byron Epoch Boundary Blocks (EBBs) are detected in the stream and their
    /// hashes are recorded in the returned `EbbInfo` list so the ledger apply
    /// loop can advance the tip through them before applying the next real block.
    pub async fn request_headers_pipelined_with_depth(
        &mut self,
        batch_size: usize,
        pipeline_depth: usize,
    ) -> Result<HeaderBatchResult, ClientError> {
        let mut headers = Vec::with_capacity(batch_size);
        // EBB hashes encountered in this batch; each entry is finalised once
        // the hash of the following real block is known.
        let mut ebb_hashes: Vec<EbbInfo> = Vec::new();
        // Hash of the most recently seen EBB; held until the next real block
        // is decoded so `next_block_hash` can be filled in.
        let mut pending_ebb_hash: Option<[u8; 32]> = None;
        let mut latest_tip = None;

        // Send pipeline of MsgRequestNext messages
        // Cap at batch_size to avoid requesting more than we'll consume
        let initial_send = pipeline_depth.min(batch_size);
        for _ in self.in_flight..initial_send {
            self.send_request_next().await?;
        }

        // Read all responses without refilling — ensures in_flight → 0
        while headers.len() < batch_size {
            if self.in_flight == 0 {
                break;
            }

            let response: Message<HeaderContent> = self
                .cs_buf
                .recv_full_msg()
                .await
                .map_err(|e| ClientError::ChainSync(format!("recv pipelined: {e}")))?;
            self.in_flight -= 1;

            match response {
                Message::RollForward(header, tip) => {
                    latest_tip = Some(pallas_to_torsten_tip(&tip));

                    match decode_header_info(&header, self.byron_epoch_length) {
                        Ok(Some(DecodedHeader::Block(info))) => {
                            trace!(
                                slot = info.slot,
                                block_no = info.block_no,
                                "pipelined header received"
                            );
                            // Finalise any pending EBB with this block's hash.
                            if let Some(ebb_hash) = pending_ebb_hash.take() {
                                ebb_hashes.push(EbbInfo {
                                    ebb_hash,
                                    next_block_hash: info.hash,
                                });
                            }
                            headers.push(info);
                        }
                        Ok(Some(DecodedHeader::Ebb(ebb_hash))) => {
                            // Record EBB hash; next real block will finalise it.
                            trace!(
                                ebb_hash = %hex::encode(ebb_hash),
                                "pipelined: recorded EBB hash for tip advance"
                            );
                            pending_ebb_hash = Some(ebb_hash);
                        }
                        Ok(None) => {
                            // EBB decode returned None (hash size mismatch, etc.)
                            trace!("pipelined: EBB skipped (could not decode hash)");
                        }
                        Err(e) => {
                            return Err(ClientError::BlockDecode(format!(
                                "pipelined header decode: {e}"
                            )));
                        }
                    }
                }
                Message::RollBackward(point, tip) => {
                    let torsten_tip = pallas_to_torsten_tip(&tip);
                    let rollback_point = pallas_to_torsten_point(&point);
                    // After a rollback, any previously collected headers are
                    // invalid (they were before the rollback). Clear them and
                    // continue reading the remaining in-flight responses, which
                    // contain the post-rollback headers.
                    headers.clear();
                    ebb_hashes.clear();
                    pending_ebb_hash = None;
                    latest_tip = Some(torsten_tip.clone());
                    // If no more in-flight, return the rollback
                    if self.in_flight == 0 {
                        return Ok(HeaderBatchResult::RollBackward(rollback_point, torsten_tip));
                    }
                    // Otherwise continue the loop — remaining in-flight
                    // responses will be post-rollback RollForward headers
                }
                Message::AwaitReply => {
                    // Server is at tip, no more blocks available right now.
                    // After AwaitReply, the server enters MustReply state and
                    // will send RollForward/RollBackward when a block arrives.
                    // Use a randomized timeout matching Haskell's behavior for
                    // non-trustable peers (135-269s uniform random). This avoids
                    // all connections timing out simultaneously and provides
                    // statistical detection of stale peers without being overly
                    // aggressive (the previous 30s caused constant reconnects).
                    let timeout = self.randomized_await_timeout();
                    let wait_response: Message<HeaderContent> =
                        tokio::time::timeout(timeout, self.cs_buf.recv_full_msg())
                            .await
                            .map_err(|_| {
                                ClientError::ChainSync(format!(
                                    "AwaitReply timeout: no block received in {}s, \
                                     connection may be stale",
                                    timeout.as_secs()
                                ))
                            })?
                            .map_err(|e| ClientError::ChainSync(format!("recv must-reply: {e}")))?;
                    // This response consumed one more in-flight
                    // (the MustReply is implicit, not counted separately)

                    match wait_response {
                        Message::RollForward(header, tip) => {
                            latest_tip = Some(pallas_to_torsten_tip(&tip));
                            match decode_header_info(&header, self.byron_epoch_length) {
                                Ok(Some(DecodedHeader::Block(info))) => {
                                    if let Some(ebb_hash) = pending_ebb_hash.take() {
                                        ebb_hashes.push(EbbInfo {
                                            ebb_hash,
                                            next_block_hash: info.hash,
                                        });
                                    }
                                    headers.push(info);
                                }
                                Ok(Some(DecodedHeader::Ebb(ebb_hash))) => {
                                    // An EBB arrived at tip — record it but the
                                    // next_block_hash is unknown until the next
                                    // RollForward arrives in a subsequent call.
                                    // We do not set pending_ebb_hash here because
                                    // this function returns immediately after the
                                    // AwaitReply handler; the EBB will be re-sent
                                    // by the server on the next request.
                                    let _ebb_at_tip = ebb_hash; // acknowledged but deferred
                                }
                                Ok(None) | Err(_) => {}
                            }
                            // Do NOT drain remaining in-flight. The server will
                            // respond to them one-by-one as new blocks arrive.
                            // Subsequent calls with reduced depth will read from
                            // the existing in-flight pipeline without sending new
                            // requests (matching Haskell's transition from pipelined
                            // to non-pipelined mode at tip).
                        }
                        Message::RollBackward(point, tip) => {
                            let torsten_tip = pallas_to_torsten_tip(&tip);
                            if !headers.is_empty() {
                                return Ok(HeaderBatchResult::HeadersAndRollback {
                                    headers,
                                    tip: torsten_tip.clone(),
                                    rollback_point: pallas_to_torsten_point(&point),
                                    rollback_tip: torsten_tip,
                                    ebb_hashes,
                                });
                            }
                            return Ok(HeaderBatchResult::RollBackward(
                                pallas_to_torsten_point(&point),
                                torsten_tip,
                            ));
                        }
                        other => {
                            debug!("Pipelined ChainSync: non-block response after AwaitReply: {other:?}");
                        }
                    }

                    if headers.is_empty() {
                        return Ok(HeaderBatchResult::Await);
                    }
                    // We have headers AND the server sent AwaitReply — signal
                    // the caller that we've caught up to the tip.
                    let tip = latest_tip.ok_or_else(|| {
                        ClientError::ChainSync("got headers at tip but no tip".into())
                    })?;
                    return Ok(HeaderBatchResult::HeadersAtTip(headers, tip, ebb_hashes));
                }
                _ => {
                    return Err(ClientError::ChainSync(format!(
                        "unexpected message in pipelined response: {response:?}"
                    )));
                }
            }
        }

        match latest_tip {
            Some(tip) => Ok(HeaderBatchResult::Headers(headers, tip, ebb_hashes)),
            None if headers.is_empty() => Ok(HeaderBatchResult::Await),
            None => Err(ClientError::ChainSync("got headers but no tip".into())),
        }
    }

    /// Send a single MsgRequestNext and increment in-flight counter.
    async fn send_request_next(&mut self) -> Result<(), ClientError> {
        let msg = Message::<HeaderContent>::RequestNext;
        self.cs_buf
            .send_msg_chunks(&msg)
            .await
            .map_err(|e| ClientError::ChainSync(format!("send RequestNext: {e}")))?;
        self.in_flight += 1;
        Ok(())
    }

    /// Access the blockfetch client for fetching full blocks.
    pub fn blockfetch(&mut self) -> &mut pallas_network::miniprotocols::blockfetch::Client {
        &mut self.bf_client
    }

    /// Remote address of this connection.
    pub fn remote_addr(&self) -> SocketAddr {
        self.remote_addr
    }

    /// Set the Byron epoch length for correct slot computation on non-mainnet
    /// networks. Value should be `10 * security_param` (10 * k).
    pub fn set_byron_epoch_length(&mut self, len: u64) {
        self.byron_epoch_length = len;
    }

    /// Set the AwaitReply timeout for the MustReply state.
    /// Controls how long to wait for the next block when the peer is at tip.
    pub fn set_await_reply_timeout(&mut self, timeout: Duration) {
        self.await_reply_timeout = timeout;
    }

    /// Generate a randomized AwaitReply timeout for non-trustable peers,
    /// matching Haskell's behavior.
    ///
    /// Haskell's `chainSyncTimeouts` for non-trustable peers uses a random
    /// uniform distribution between ~135s and ~269s (based on the probability
    /// that no block arrives in that interval being < 10^-3 to 10^-6).
    /// The ouroboros-network code uses `minChainSyncTimeout=601` and
    /// `maxChainSyncTimeout=911` for the raw protocol timeout, but the
    /// practical MustReply timeout is shorter at 135-269s.
    ///
    /// We use the 135-269s range which better matches the at-tip behavior.
    fn randomized_await_timeout(&self) -> Duration {
        use std::hash::{Hash, Hasher};
        // Use a simple hash-based pseudo-random since we don't need
        // cryptographic randomness — just variation between connections.
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.remote_addr.hash(&mut hasher);
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .hash(&mut hasher);
        let hash = hasher.finish();
        // Range: 135-269 seconds (134 second spread)
        let secs = 135 + (hash % 135);
        Duration::from_secs(secs)
    }

    /// Take the TxSubmission2 agent channel for use by a background tx fetcher.
    ///
    /// Returns `None` if the channel has already been taken.
    pub fn take_txsub_channel(&mut self) -> Option<pallas_network::multiplexer::AgentChannel> {
        self.txsub_channel.take()
    }

    /// Abort the connection.
    pub async fn abort(self) {
        self._plexer.abort().await;
    }

    /// Construct a `PipelinedPeerClient` from raw connection components.
    ///
    /// Used by [`crate::duplex::DuplexPeerConnection::into_pipelined`] to
    /// re-wrap an already-established full-duplex connection so that the same
    /// `chain_sync_loop` code can drive it for pipelined ChainSync.
    ///
    /// No `txsub_channel` is supplied here because a full-duplex connection
    /// uses the TxSubmission2 protocol in *server* mode (subscribe_server),
    /// not client mode.  The caller must not attempt to spawn a TxSubmission2
    /// client on this connection.
    #[allow(clippy::too_many_arguments)] // inherent to unwrapping a connection struct
    pub(crate) fn from_duplex_parts(
        cs_buf: ChannelBuffer,
        bf_client: pallas_network::miniprotocols::blockfetch::Client,
        keepalive: KeepAliveHandle,
        plexer: RunningPlexer,
        peersharing_channel: pallas_network::multiplexer::AgentChannel,
        remote_addr: std::net::SocketAddr,
        in_flight: usize,
        byron_epoch_length: u64,
    ) -> Self {
        PipelinedPeerClient {
            cs_buf,
            bf_client,
            _keepalive: keepalive,
            _plexer: plexer,
            _peersharing_channel: peersharing_channel,
            remote_addr,
            in_flight,
            // No TxSubmission2 client channel — the duplex connection uses
            // subscribe_server for TX_SUBMISSION (responder mode), so there
            // is no initiator channel to expose here.
            txsub_channel: None,
            byron_epoch_length,
            await_reply_timeout: Duration::from_secs(
                crate::tcp::TimeoutConfig::default().await_reply_timeout_secs,
            ),
        }
    }
}

/// Result of decoding a single ChainSync header.
enum DecodedHeader {
    /// A regular block header with full metadata.
    Block(HeaderInfo),
    /// A Byron Epoch Boundary Block — no slot, no transactions, but its hash
    /// is the `prev_hash` of the first real block of the next epoch.
    Ebb([u8; 32]),
}

/// Decode header metadata from a ChainSync HeaderContent.
///
/// Returns `DecodedHeader::Ebb` for Byron Epoch Boundary Blocks (variant 0,
/// subtag 0): these carry no transactions and are never fetched via BlockFetch,
/// but their hashes must be recorded so the ledger tip can be advanced through
/// them before the next real block is applied.
///
/// Returns `DecodedHeader::Block` for all regular headers.
///
/// `byron_epoch_length`: the number of absolute slots per Byron epoch
/// (= 10 * k). Required for correct slot computation on non-mainnet
/// networks where pallas's hardcoded mainnet values would be wrong.
/// Pass 0 to use pallas defaults (correct for mainnet).
fn decode_header_info(
    header: &HeaderContent,
    byron_epoch_length: u64,
) -> Result<Option<DecodedHeader>, String> {
    let subtag = header.byron_prefix.map(|(st, _)| st);

    // EBBs: Byron variant 0 with subtag 0.  They have no slot and carry no
    // transactions, but their hash is the `prev_hash` of the next real block.
    // Record the hash so the apply loop can advance the ledger tip through them.
    if header.variant == 0 && subtag == Some(0) {
        match MultiEraHeader::decode(header.variant, subtag, &header.cbor) {
            Ok(ebb_header) => {
                let hash_vec = ebb_header.hash().to_vec();
                if hash_vec.len() == 32 {
                    let mut ebb_hash = [0u8; 32];
                    ebb_hash.copy_from_slice(&hash_vec);
                    return Ok(Some(DecodedHeader::Ebb(ebb_hash)));
                }
                // Hash size mismatch — treat as skipped EBB (non-fatal).
                return Ok(None);
            }
            Err(_) => {
                // Decode failure on an EBB header is non-fatal: we skip it.
                // If the subsequent real block can't be decoded either, that
                // error will surface normally.
                return Ok(None);
            }
        }
    }

    let multi_era_header = MultiEraHeader::decode(header.variant, subtag, &header.cbor)
        .map_err(|e| format!("header decode: {e}"))?;

    // For Byron headers on non-mainnet networks, compute slot from the
    // raw (epoch, relative_slot) using the actual Byron epoch length.
    // Pallas uses hardcoded mainnet genesis values which produce wrong
    // absolute slots on testnets with different k values.
    let slot = if header.variant == 0 && byron_epoch_length > 0 {
        if let Some(byron) = multi_era_header.as_byron() {
            let epoch = byron.consensus_data.0.epoch;
            let rel_slot = byron.consensus_data.0.slot;
            epoch * byron_epoch_length + rel_slot
        } else {
            multi_era_header.slot()
        }
    } else {
        multi_era_header.slot()
    };

    let hash = multi_era_header.hash();
    let block_no = multi_era_header.number();

    let mut hash_bytes = [0u8; 32];
    let hash_vec = hash.to_vec();
    hash_bytes.copy_from_slice(&hash_vec);

    Ok(Some(DecodedHeader::Block(HeaderInfo {
        slot,
        hash: hash_bytes,
        block_no,
    })))
}

// Point/Tip conversion utilities

fn torsten_to_pallas_point(point: &Point) -> PallasPoint {
    match point {
        Point::Origin => PallasPoint::Origin,
        Point::Specific(slot, hash) => PallasPoint::Specific(slot.0, hash.as_bytes().to_vec()),
    }
}

fn pallas_to_torsten_point(point: &PallasPoint) -> Point {
    match point {
        PallasPoint::Origin => Point::Origin,
        PallasPoint::Specific(slot, hash) => {
            let mut hash_bytes = [0u8; 32];
            if hash.len() == 32 {
                hash_bytes.copy_from_slice(hash);
            }
            Point::Specific(SlotNo(*slot), Hash32::from_bytes(hash_bytes))
        }
    }
}

fn pallas_to_torsten_tip(tip: &Tip) -> TorstenTip {
    TorstenTip {
        point: pallas_to_torsten_point(&tip.0),
        block_number: torsten_primitives::time::BlockNo(tip.1),
    }
}
