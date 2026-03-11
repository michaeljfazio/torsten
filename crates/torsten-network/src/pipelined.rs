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
use tracing::{debug, info, trace, warn};

use crate::client::{ClientError, HeaderBatchResult, HeaderInfo};
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
    /// True when the connection has stale in-flight requests that couldn't be
    /// drained (e.g. after AwaitReply with many pending MsgRequestNext).
    /// The caller should reconnect before using this client again.
    stale: bool,
    /// TxSubmission2 channel (available for taking by a background tx fetcher)
    txsub_channel: Option<pallas_network::multiplexer::AgentChannel>,
    /// Byron epoch length in absolute slots (10 * k). Used for correct
    /// slot computation on non-mainnet networks. 0 = use pallas defaults.
    byron_epoch_length: u64,
}

impl PipelinedPeerClient {
    /// Connect to a remote Cardano node and set up a pipelined ChainSync channel.
    pub async fn connect(
        addr: impl ToSocketAddrs + std::fmt::Display + Copy,
        network_magic: u64,
    ) -> Result<Self, ClientError> {
        info!("pipelined client: connecting to {addr}");

        let bearer = Bearer::connect_tcp(addr)
            .await
            .map_err(|e| ClientError::Connection(format!("pipelined connect: {e}")))?;

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
            .unwrap_or_else(|_| "0.0.0.0:0".parse().unwrap());

        info!("pipelined client: connected to {remote_addr}");

        Ok(PipelinedPeerClient {
            cs_buf,
            bf_client,
            _keepalive: keepalive,
            _plexer: plexer,
            _peersharing_channel: peersharing_channel,
            remote_addr,
            in_flight: 0,
            stale: false,
            txsub_channel: Some(txsub_channel),
            byron_epoch_length: 0,
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
    pub async fn request_headers_pipelined_with_depth(
        &mut self,
        batch_size: usize,
        pipeline_depth: usize,
    ) -> Result<HeaderBatchResult, ClientError> {
        let mut headers = Vec::with_capacity(batch_size);
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
                        Ok(Some(info)) => {
                            trace!(
                                slot = info.slot,
                                block_no = info.block_no,
                                "pipelined header received"
                            );
                            headers.push(info);
                        }
                        Ok(None) => {
                            // EBB skipped
                            trace!("skipping EBB header");
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
                    // Drain remaining in-flight (they'll also be AwaitReply or
                    // actual responses if blocks arrived while we were reading).
                    // After AwaitReply, the server enters MustReply state and
                    // will send RollForward/RollBackward when a block arrives.
                    // We need to wait for that response.
                    let wait_response: Message<HeaderContent> =
                        self.cs_buf
                            .recv_full_msg()
                            .await
                            .map_err(|e| ClientError::ChainSync(format!("recv must-reply: {e}")))?;
                    // This response consumed one more in-flight
                    // (the MustReply is implicit, not counted separately)

                    match wait_response {
                        Message::RollForward(header, tip) => {
                            latest_tip = Some(pallas_to_torsten_tip(&tip));
                            if let Ok(Some(info)) =
                                decode_header_info(&header, self.byron_epoch_length)
                            {
                                headers.push(info);
                            }
                            // Drain remaining in-flight
                            self.drain_in_flight().await;
                        }
                        Message::RollBackward(point, tip) => {
                            self.drain_in_flight().await;
                            let torsten_tip = pallas_to_torsten_tip(&tip);
                            if !headers.is_empty() {
                                return Ok(HeaderBatchResult::HeadersAndRollback {
                                    headers,
                                    tip: torsten_tip.clone(),
                                    rollback_point: pallas_to_torsten_point(&point),
                                    rollback_tip: torsten_tip,
                                });
                            }
                            return Ok(HeaderBatchResult::RollBackward(
                                pallas_to_torsten_point(&point),
                                torsten_tip,
                            ));
                        }
                        _ => {}
                    }

                    if headers.is_empty() {
                        return Ok(HeaderBatchResult::Await);
                    }
                    break;
                }
                _ => {
                    return Err(ClientError::ChainSync(format!(
                        "unexpected message in pipelined response: {response:?}"
                    )));
                }
            }
        }

        match latest_tip {
            Some(tip) => Ok(HeaderBatchResult::Headers(headers, tip)),
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

    /// Drain in-flight responses with a timeout.
    ///
    /// After AwaitReply, each remaining in-flight MsgRequestNext requires a new
    /// block to be produced (~20s each). With 150 in-flight, this would block
    /// for ~50 minutes. Instead, drain what we can within a short timeout and
    /// mark the client as stale so the caller can reconnect.
    async fn drain_in_flight(&mut self) {
        let deadline = tokio::time::Instant::now() + Duration::from_millis(500);
        while self.in_flight > 0 {
            match tokio::time::timeout_at(
                deadline,
                self.cs_buf.recv_full_msg::<Message<HeaderContent>>(),
            )
            .await
            {
                Ok(Ok(_)) => {
                    self.in_flight -= 1;
                }
                Ok(Err(e)) => {
                    debug!("drain in-flight error (expected): {e}");
                    self.in_flight = 0;
                    break;
                }
                Err(_) => {
                    // Timeout — remaining in-flight would block for too long
                    debug!(
                        remaining = self.in_flight,
                        "drain in-flight timeout, marking client stale"
                    );
                    self.in_flight = 0;
                    self.stale = true;
                    break;
                }
            }
        }
    }

    /// Access the blockfetch client for fetching full blocks.
    pub fn blockfetch(&mut self) -> &mut pallas_network::miniprotocols::blockfetch::Client {
        &mut self.bf_client
    }

    /// Remote address of this connection.
    pub fn remote_addr(&self) -> SocketAddr {
        self.remote_addr
    }

    /// Whether the connection has stale in-flight requests and should be
    /// reconnected before further use.
    pub fn is_stale(&self) -> bool {
        self.stale
    }

    /// Set the Byron epoch length for correct slot computation on non-mainnet
    /// networks. Value should be `10 * security_param` (10 * k).
    pub fn set_byron_epoch_length(&mut self, len: u64) {
        self.byron_epoch_length = len;
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
}

/// Decode header metadata from a ChainSync HeaderContent.
/// Returns None for Epoch Boundary Blocks (EBBs), which contain no
/// transactions and are not needed for ledger application.
///
/// `byron_epoch_length`: the number of absolute slots per Byron epoch
/// (= 10 * k). Required for correct slot computation on non-mainnet
/// networks where pallas's hardcoded mainnet values would be wrong.
/// Pass 0 to use pallas defaults (correct for mainnet).
fn decode_header_info(
    header: &HeaderContent,
    byron_epoch_length: u64,
) -> Result<Option<HeaderInfo>, String> {
    let subtag = header.byron_prefix.map(|(st, _)| st);

    // Skip EBBs: variant 0 with subtag 0
    if header.variant == 0 && subtag == Some(0) {
        return Ok(None);
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

    Ok(Some(HeaderInfo {
        slot,
        hash: hash_bytes,
        block_no,
    }))
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
