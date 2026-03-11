use pallas_network::facades::PeerClient;
use pallas_network::miniprotocols::chainsync::NextResponse;
use pallas_network::miniprotocols::chainsync::Tip as PallasTip;
use pallas_network::miniprotocols::Point as PallasPoint;
use pallas_traverse::MultiEraHeader;
use std::net::SocketAddr;
use std::sync::Arc;
use thiserror::Error;
use tokio::net::ToSocketAddrs;
use tokio::sync::Mutex;
use torsten_primitives::block::{Block, Point, Tip};
use torsten_primitives::hash::Hash32;
use torsten_primitives::time::{BlockNo, SlotNo};
use torsten_serialization::multi_era::decode_block_with_byron_epoch_length;
use tracing::{debug, info, warn};

#[derive(Error, Debug)]
pub enum ClientError {
    #[error("connection failed: {0}")]
    Connection(String),
    #[error("handshake failed: {0}")]
    Handshake(String),
    #[error("chainsync error: {0}")]
    ChainSync(String),
    #[error("blockfetch error: {0}")]
    BlockFetch(String),
    #[error("block decode error: {0}")]
    BlockDecode(String),
    #[error("peer disconnected")]
    Disconnected,
}

/// Response from chain sync protocol
#[derive(Debug)]
pub enum ChainSyncEvent {
    /// A new block was received (roll forward)
    RollForward(Box<Block>, Tip),
    /// Chain rollback to a previous point
    RollBackward(Point, Tip),
    /// Caught up to tip, awaiting new blocks
    Await,
}

/// A client connection to a remote Cardano node (node-to-node protocol).
pub struct NodeToNodeClient {
    peer: PeerClient,
    remote_addr: SocketAddr,
    /// Byron epoch length in absolute slots (10 * k). Used for correct
    /// slot computation on non-mainnet networks. 0 = use pallas defaults.
    byron_epoch_length: u64,
}

impl NodeToNodeClient {
    /// Connect to a remote Cardano node via TCP.
    pub async fn connect(
        addr: impl ToSocketAddrs + std::fmt::Display + Copy,
        network_magic: u64,
    ) -> Result<Self, ClientError> {
        info!("connecting to peer {addr}");

        let peer = PeerClient::connect(addr, network_magic)
            .await
            .map_err(|e| ClientError::Connection(format!("{addr}: {e}")))?;

        // Resolve the address for display purposes
        let remote_addr = format!("{addr}")
            .parse()
            .unwrap_or_else(|_| std::net::SocketAddr::from(([0, 0, 0, 0], 0)));

        info!("connected to peer {remote_addr}");

        Ok(NodeToNodeClient {
            peer,
            remote_addr,
            byron_epoch_length: 0,
        })
    }

    /// Set the Byron epoch length for correct slot computation on non-mainnet
    /// networks. Value should be `10 * security_param` (10 * k).
    pub fn set_byron_epoch_length(&mut self, len: u64) {
        self.byron_epoch_length = len;
    }

    /// Find an intersection point with the remote peer's chain.
    /// Returns the intersection point and the remote tip.
    pub async fn find_intersect(
        &mut self,
        points: Vec<Point>,
    ) -> Result<(Option<Point>, Tip), ClientError> {
        let pallas_points: Vec<PallasPoint> = points.iter().map(torsten_point_to_pallas).collect();

        let (intersect, tip) = self
            .peer
            .chainsync()
            .find_intersect(pallas_points)
            .await
            .map_err(|e| ClientError::ChainSync(format!("find_intersect: {e}")))?;

        let torsten_intersect = intersect.map(|p| pallas_point_to_torsten(&p));
        let torsten_tip = pallas_tip_to_torsten(&tip);

        if let Some(ref p) = torsten_intersect {
            debug!("intersected at {p}");
        } else {
            warn!("no intersection found");
        }

        Ok((torsten_intersect, torsten_tip))
    }

    /// Request the next chain sync event.
    ///
    /// N2N chainsync delivers headers. When we receive a header, we immediately
    /// fetch the full block via blockfetch and return it as a RollForward event.
    pub async fn request_next(&mut self) -> Result<ChainSyncEvent, ClientError> {
        let response = self
            .peer
            .chainsync()
            .request_or_await_next()
            .await
            .map_err(|e| ClientError::ChainSync(format!("request_next: {e}")))?;

        match response {
            NextResponse::RollForward(header, tip) => {
                let torsten_tip = pallas_tip_to_torsten(&tip);

                // Parse the header to extract slot + hash for blockfetch
                let subtag = header.byron_prefix.map(|(st, _)| st);
                let multi_era_header = MultiEraHeader::decode(header.variant, subtag, &header.cbor)
                    .map_err(|e| ClientError::BlockDecode(format!("header decode: {e}")))?;

                let slot = multi_era_header.slot();
                let block_no = multi_era_header.number();
                let hash = multi_era_header.hash();

                let block_point = PallasPoint::Specific(slot, hash.to_vec());

                // Fetch the full block via blockfetch
                let bodies = self
                    .peer
                    .blockfetch()
                    .fetch_range((block_point.clone(), block_point))
                    .await
                    .map_err(|e| ClientError::BlockFetch(format!("fetch block: {e}")))?;

                let cbor = bodies
                    .into_iter()
                    .next()
                    .ok_or_else(|| ClientError::BlockFetch("no block returned".into()))?;

                let block = decode_block_with_byron_epoch_length(&cbor, self.byron_epoch_length)
                    .map_err(|e| ClientError::BlockDecode(format!("{e}")))?;

                debug!(slot, block_no, txs = block.tx_count(), "roll forward");
                Ok(ChainSyncEvent::RollForward(Box::new(block), torsten_tip))
            }
            NextResponse::RollBackward(point, tip) => {
                let torsten_point = pallas_point_to_torsten(&point);
                let torsten_tip = pallas_tip_to_torsten(&tip);
                warn!("rollback to {torsten_point}");
                Ok(ChainSyncEvent::RollBackward(torsten_point, torsten_tip))
            }
            NextResponse::Await => {
                info!("caught up to chain tip, awaiting new blocks");
                Ok(ChainSyncEvent::Await)
            }
        }
    }

    /// Request a batch of blocks from chain sync.
    ///
    /// Uses sub-batching: collects headers in groups of `sub_batch_size`,
    /// then immediately fetches and decodes those blocks before collecting
    /// the next group. This reduces memory pressure and returns events
    /// incrementally. Block decoding happens on blocking threads.
    pub async fn request_next_batch(
        &mut self,
        batch_size: usize,
    ) -> Result<Vec<ChainSyncEvent>, ClientError> {
        let sub_batch_size = 100; // Fetch blocks every 100 headers
        let mut events = Vec::with_capacity(batch_size);
        let mut pending_points: Vec<PallasPoint> = Vec::new();
        let mut latest_tip = None;
        let mut headers_collected = 0;
        let mut done = false;

        for _ in 0..batch_size {
            if done {
                break;
            }

            let response = self
                .peer
                .chainsync()
                .request_or_await_next()
                .await
                .map_err(|e| ClientError::ChainSync(format!("request_next: {e}")))?;

            match response {
                NextResponse::RollForward(header, tip) => {
                    latest_tip = Some(tip);
                    let subtag = header.byron_prefix.map(|(st, _)| st);
                    let multi_era_header =
                        MultiEraHeader::decode(header.variant, subtag, &header.cbor)
                            .map_err(|e| ClientError::BlockDecode(format!("header decode: {e}")))?;

                    let slot = multi_era_header.slot();
                    let hash = multi_era_header.hash();
                    pending_points.push(PallasPoint::Specific(slot, hash.to_vec()));
                    headers_collected += 1;

                    // Flush sub-batch when we hit the threshold
                    if headers_collected % sub_batch_size == 0 && !pending_points.is_empty() {
                        let tip_ref = latest_tip.as_ref().expect("tip set by prior RollForward");
                        let fetched = self
                            .fetch_and_decode_range(&pending_points, tip_ref)
                            .await?;
                        events.extend(fetched);
                        pending_points.clear();
                    }
                }
                NextResponse::RollBackward(point, tip) => {
                    // Flush any pending blocks before the rollback
                    if !pending_points.is_empty() {
                        let tip_ref = latest_tip.as_ref().expect("tip set by prior RollForward");
                        let fetched = self
                            .fetch_and_decode_range(&pending_points, tip_ref)
                            .await?;
                        events.extend(fetched);
                        pending_points.clear();
                    }
                    let torsten_point = pallas_point_to_torsten(&point);
                    let torsten_tip = pallas_tip_to_torsten(&tip);
                    warn!("rollback to {torsten_point}");
                    events.push(ChainSyncEvent::RollBackward(torsten_point, torsten_tip));
                    done = true;
                }
                NextResponse::Await => {
                    // Flush pending blocks, then signal await
                    if !pending_points.is_empty() {
                        let tip_ref = latest_tip.as_ref().expect("tip set by prior RollForward");
                        let fetched = self
                            .fetch_and_decode_range(&pending_points, tip_ref)
                            .await?;
                        events.extend(fetched);
                        pending_points.clear();
                    }
                    events.push(ChainSyncEvent::Await);
                    done = true;
                }
            }
        }

        // Flush remaining pending blocks
        if !pending_points.is_empty() {
            let tip_ref = latest_tip.as_ref().expect("tip set by prior RollForward");
            let fetched = self
                .fetch_and_decode_range(&pending_points, tip_ref)
                .await?;
            events.extend(fetched);
        }

        Ok(events)
    }

    /// Fetch a range of blocks and decode them into ChainSyncEvents.
    /// Block decoding is performed on a blocking thread pool to avoid
    /// stalling the async runtime with CPU-intensive CBOR parsing.
    async fn fetch_and_decode_range(
        &mut self,
        points: &[PallasPoint],
        tip: &PallasTip,
    ) -> Result<Vec<ChainSyncEvent>, ClientError> {
        let first = points.first().expect("points non-empty").clone();
        let last = points.last().expect("points non-empty").clone();

        let bodies = self
            .peer
            .blockfetch()
            .fetch_range((first, last))
            .await
            .map_err(|e| ClientError::BlockFetch(format!("fetch range: {e}")))?;

        let torsten_tip = pallas_tip_to_torsten(tip);

        // Decode blocks on a blocking thread to avoid stalling the async runtime
        let bel = self.byron_epoch_length;
        let decoded = tokio::task::spawn_blocking(move || {
            let mut events = Vec::with_capacity(bodies.len());
            for cbor in bodies {
                let block = decode_block_with_byron_epoch_length(&cbor, bel)
                    .map_err(|e| ClientError::BlockDecode(format!("{e}")))?;
                events.push(ChainSyncEvent::RollForward(
                    Box::new(block),
                    torsten_tip.clone(),
                ));
            }
            Ok::<_, ClientError>(events)
        })
        .await
        .map_err(|e| ClientError::BlockDecode(format!("decode task failed: {e}")))??;

        Ok(decoded)
    }

    /// Fetch a range of blocks from the peer using the block-fetch protocol.
    pub async fn fetch_block_range(
        &mut self,
        from: &Point,
        to: &Point,
    ) -> Result<Vec<Block>, ClientError> {
        let pallas_from = torsten_point_to_pallas(from);
        let pallas_to = torsten_point_to_pallas(to);

        let bodies = self
            .peer
            .blockfetch()
            .fetch_range((pallas_from, pallas_to))
            .await
            .map_err(|e| ClientError::BlockFetch(format!("fetch_range: {e}")))?;

        let mut blocks = Vec::with_capacity(bodies.len());
        for body in bodies {
            let block = decode_block_with_byron_epoch_length(&body, self.byron_epoch_length)
                .map_err(|e| ClientError::BlockDecode(format!("{e}")))?;
            blocks.push(block);
        }

        debug!("fetched {} blocks", blocks.len());
        Ok(blocks)
    }

    /// Get the remote peer's address.
    pub fn remote_addr(&self) -> &SocketAddr {
        &self.remote_addr
    }

    /// Request a batch of headers only (no block fetching).
    /// Returns header metadata that can be used for parallel block fetching.
    pub async fn request_headers_batch(
        &mut self,
        batch_size: usize,
    ) -> Result<HeaderBatchResult, ClientError> {
        let mut headers = Vec::with_capacity(batch_size);
        let mut latest_tip = None;

        for _ in 0..batch_size {
            let response = self
                .peer
                .chainsync()
                .request_or_await_next()
                .await
                .map_err(|e| ClientError::ChainSync(format!("request_next: {e}")))?;

            match response {
                NextResponse::RollForward(header, tip) => {
                    latest_tip = Some(pallas_tip_to_torsten(&tip));
                    let subtag = header.byron_prefix.map(|(st, _)| st);

                    // Skip EBBs: they contain no transactions
                    if header.variant == 0 && subtag == Some(0) {
                        continue;
                    }

                    let multi_era_header =
                        MultiEraHeader::decode(header.variant, subtag, &header.cbor)
                            .map_err(|e| ClientError::BlockDecode(format!("header decode: {e}")))?;

                    // For Byron headers on non-mainnet, compute slot from raw
                    // (epoch, rel_slot) using actual Byron epoch length
                    let slot = if header.variant == 0 && self.byron_epoch_length > 0 {
                        if let Some(byron) = multi_era_header.as_byron() {
                            let epoch = byron.consensus_data.0.epoch;
                            let rel_slot = byron.consensus_data.0.slot;
                            epoch * self.byron_epoch_length + rel_slot
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
                    headers.push(HeaderInfo {
                        slot,
                        hash: hash_bytes,
                        block_no,
                    });
                }
                NextResponse::RollBackward(point, tip) => {
                    let torsten_tip = pallas_tip_to_torsten(&tip);
                    if !headers.is_empty() {
                        return Ok(HeaderBatchResult::HeadersAndRollback {
                            headers,
                            tip: torsten_tip.clone(),
                            rollback_point: pallas_point_to_torsten(&point),
                            rollback_tip: torsten_tip,
                        });
                    }
                    return Ok(HeaderBatchResult::RollBackward(
                        pallas_point_to_torsten(&point),
                        torsten_tip,
                    ));
                }
                NextResponse::Await => {
                    if !headers.is_empty() {
                        return Ok(HeaderBatchResult::Headers(
                            headers,
                            latest_tip.expect("tip set by prior RollForward"),
                        ));
                    }
                    return Ok(HeaderBatchResult::Await);
                }
            }
        }

        Ok(HeaderBatchResult::Headers(
            headers,
            latest_tip.expect("tip set by prior RollForward"),
        ))
    }

    /// Fetch blocks by a list of points using the blockfetch protocol.
    /// Each block is fetched individually by its exact point (slot + hash),
    /// ensuring hash-verified correctness regardless of which peer serves it.
    pub async fn fetch_blocks_by_points(
        &mut self,
        points: &[HeaderInfo],
    ) -> Result<Vec<Block>, ClientError> {
        if points.is_empty() {
            return Ok(vec![]);
        }

        let mut all_bodies = Vec::with_capacity(points.len());

        // Try fetching all points as one range (from first to last) for efficiency.
        // Falls back to individual point fetches if the range request fails (e.g.
        // cross-era ranges, peer doesn't support the range, etc.).
        let first = PallasPoint::Specific(points[0].slot, points[0].hash.to_vec());
        let last_pt = points.last().expect("points non-empty (checked above)");
        let last = PallasPoint::Specific(last_pt.slot, last_pt.hash.to_vec());

        let range_result = self.peer.blockfetch().fetch_range((first, last)).await;

        match range_result {
            Ok(bodies) if bodies.len() == points.len() => {
                all_bodies = bodies;
            }
            Ok(bodies) => {
                // Peer returned different number of blocks — fall back to individual fetches
                tracing::warn!(
                    expected = points.len(),
                    got = bodies.len(),
                    peer = %self.remote_addr,
                    "Block count mismatch from range fetch, falling back to individual fetches"
                );
                self.fetch_individual_points(points, &mut all_bodies)
                    .await?;
            }
            Err(e) => {
                // Range fetch failed (NoBlocks, cross-era range, etc.)
                // Fall back to individual point fetches
                tracing::warn!(
                    first_slot = points[0].slot,
                    last_slot = last_pt.slot,
                    num_points = points.len(),
                    peer = %self.remote_addr,
                    error = %e,
                    "Range fetch failed, falling back to individual point fetches"
                );
                self.fetch_individual_points(points, &mut all_bodies)
                    .await?;
            }
        }

        // Decode on blocking thread and verify block hashes
        let expected_points: Vec<(u64, [u8; 32])> =
            points.iter().map(|p| (p.slot, p.hash)).collect();
        let bel = self.byron_epoch_length;
        let decoded = tokio::task::spawn_blocking(move || {
            let mut blocks = Vec::with_capacity(all_bodies.len());
            for (i, cbor) in all_bodies.into_iter().enumerate() {
                let block = decode_block_with_byron_epoch_length(&cbor, bel)
                    .map_err(|e| ClientError::BlockDecode(format!("{e}")))?;

                // Verify block hash matches expected header hash
                if i < expected_points.len() {
                    let (expected_slot, expected_hash) = expected_points[i];
                    let actual_hash = block.hash().as_bytes();
                    if *actual_hash != expected_hash {
                        return Err(ClientError::BlockDecode(format!(
                            "Block hash mismatch at slot {expected_slot}: expected {:02x?}, got {:02x?}",
                            &expected_hash[..8],
                            &actual_hash[..8]
                        )));
                    }
                }

                blocks.push(block);
            }
            Ok::<_, ClientError>(blocks)
        })
        .await
        .map_err(|e| ClientError::BlockDecode(format!("decode task: {e}")))??;

        Ok(decoded)
    }

    /// Fetch blocks one at a time by their exact point (slot + hash).
    /// Slower than range fetch but works across era boundaries and when
    /// the peer doesn't support the requested range.
    async fn fetch_individual_points(
        &mut self,
        points: &[HeaderInfo],
        out: &mut Vec<Vec<u8>>,
    ) -> Result<(), ClientError> {
        for point in points {
            let p = PallasPoint::Specific(point.slot, point.hash.to_vec());
            let single = self
                .peer
                .blockfetch()
                .fetch_range((p.clone(), p))
                .await
                .map_err(|e| {
                    ClientError::BlockFetch(format!("single fetch slot {}: {e}", point.slot))
                })?;
            if let Some(body) = single.into_iter().next() {
                out.push(body);
            } else {
                return Err(ClientError::BlockFetch(format!(
                    "block not found at slot {}",
                    point.slot
                )));
            }
        }
        Ok(())
    }

    /// Disconnect from the peer.
    pub async fn disconnect(self) {
        info!("disconnecting from peer {}", self.remote_addr);
        self.peer.abort().await;
    }
}

/// Metadata about a block header, used for parallel block fetching.
#[derive(Debug, Clone)]
pub struct HeaderInfo {
    pub slot: u64,
    pub hash: [u8; 32],
    pub block_no: u64,
}

/// Result of a header-only batch request.
#[derive(Debug)]
pub enum HeaderBatchResult {
    /// Got a batch of headers
    Headers(Vec<HeaderInfo>, Tip),
    /// Got a rollback
    RollBackward(Point, Tip),
    /// Got headers followed by a rollback in the same batch
    HeadersAndRollback {
        headers: Vec<HeaderInfo>,
        tip: Tip,
        rollback_point: Point,
        rollback_tip: Tip,
    },
    /// Caught up to tip
    Await,
}

/// A pool of peer connections for concurrent block fetching.
/// Headers are collected from a primary peer via ChainSync,
/// then blocks are fetched from multiple peers in parallel.
pub struct BlockFetchPool {
    fetchers: Vec<Arc<Mutex<NodeToNodeClient>>>,
}

impl Default for BlockFetchPool {
    fn default() -> Self {
        Self::new()
    }
}

impl BlockFetchPool {
    /// Create a new pool with no fetchers.
    pub fn new() -> Self {
        BlockFetchPool {
            fetchers: Vec::new(),
        }
    }

    /// Add a connected peer to the pool.
    pub fn add_fetcher(&mut self, client: NodeToNodeClient) {
        self.fetchers.push(Arc::new(Mutex::new(client)));
    }

    /// Number of fetchers in the pool.
    pub fn len(&self) -> usize {
        self.fetchers.len()
    }

    /// Returns true if the pool has no fetchers.
    pub fn is_empty(&self) -> bool {
        self.fetchers.is_empty()
    }

    /// Fetch blocks for the given headers using all fetchers in parallel.
    /// Headers are split into chunks and fetched concurrently across fetchers,
    /// then results are reassembled in order. This achieves near-linear speedup
    /// with the number of fetchers.
    pub async fn fetch_blocks_concurrent(
        &self,
        headers: &[HeaderInfo],
    ) -> Result<Vec<Block>, ClientError> {
        if headers.is_empty() {
            return Ok(vec![]);
        }

        let num_fetchers = self.fetchers.len();
        if num_fetchers == 0 {
            return Err(ClientError::Connection("no fetchers available".into()));
        }

        // Split headers into chunks, one per fetcher
        let chunk_size = headers.len().div_ceil(num_fetchers);
        let chunks: Vec<Vec<HeaderInfo>> = headers.chunks(chunk_size).map(|c| c.to_vec()).collect();

        // Fetch each chunk from a different fetcher in parallel
        let mut handles = Vec::with_capacity(chunks.len());
        for (i, chunk) in chunks.into_iter().enumerate() {
            let fetcher = self.fetchers[i].clone();
            handles.push(tokio::spawn(async move {
                let mut client = fetcher.lock().await;
                client.fetch_blocks_by_points(&chunk).await
            }));
        }

        // Collect results in order, retrying failed chunks on other fetchers
        let mut all_blocks = Vec::with_capacity(headers.len());
        let mut failed_chunks: Vec<(usize, Vec<HeaderInfo>)> = Vec::new();

        for (i, handle) in handles.into_iter().enumerate() {
            match handle.await {
                Ok(Ok(blocks)) => all_blocks.extend(blocks),
                Ok(Err(e)) => {
                    tracing::warn!("Fetcher {i} failed: {e}, will retry on another fetcher");
                    let start = i * chunk_size;
                    let end = (start + chunk_size).min(headers.len());
                    failed_chunks.push((i, headers[start..end].to_vec()));
                }
                Err(e) => {
                    tracing::warn!("Fetcher {i} task panicked: {e}, will retry on another fetcher");
                    let start = i * chunk_size;
                    let end = (start + chunk_size).min(headers.len());
                    failed_chunks.push((i, headers[start..end].to_vec()));
                }
            }
        }

        // Retry failed chunks on the first available fetcher (round-robin)
        for (failed_idx, chunk) in failed_chunks {
            let fallback_idx = (failed_idx + 1) % num_fetchers;
            let fetcher = self.fetchers[fallback_idx].clone();
            let mut client = fetcher.lock().await;
            let blocks = client.fetch_blocks_by_points(&chunk).await.map_err(|e| {
                ClientError::BlockFetch(format!("retry on fetcher {fallback_idx}: {e}"))
            })?;
            all_blocks.extend(blocks);
        }

        Ok(all_blocks)
    }

    /// Disconnect all fetchers.
    pub async fn disconnect_all(self) {
        for fetcher in self.fetchers {
            if let Ok(client) = Arc::try_unwrap(fetcher) {
                client.into_inner().disconnect().await;
            }
        }
    }
}

// Conversion utilities between torsten and pallas Point/Tip types

fn torsten_point_to_pallas(point: &Point) -> PallasPoint {
    match point {
        Point::Origin => PallasPoint::Origin,
        Point::Specific(slot, hash) => PallasPoint::Specific(slot.0, hash.to_vec()),
    }
}

fn pallas_point_to_torsten(point: &PallasPoint) -> Point {
    match point {
        PallasPoint::Origin => Point::Origin,
        PallasPoint::Specific(slot, hash) => {
            let mut bytes = [0u8; 32];
            if hash.len() == 32 {
                bytes.copy_from_slice(hash);
            }
            Point::Specific(SlotNo(*slot), Hash32::from_bytes(bytes))
        }
    }
}

fn pallas_tip_to_torsten(tip: &PallasTip) -> Tip {
    Tip {
        point: pallas_point_to_torsten(&tip.0),
        block_number: BlockNo(tip.1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_point_origin_roundtrip() {
        let point = Point::Origin;
        let pallas = torsten_point_to_pallas(&point);
        let back = pallas_point_to_torsten(&pallas);
        assert_eq!(point, back);
    }

    #[test]
    fn test_point_specific_roundtrip() {
        let hash = Hash32::from_bytes([0xab; 32]);
        let point = Point::Specific(SlotNo(12345), hash);
        let pallas = torsten_point_to_pallas(&point);
        let back = pallas_point_to_torsten(&pallas);
        assert_eq!(point, back);
    }

    #[test]
    fn test_pallas_tip_conversion() {
        let pallas_tip = PallasTip(PallasPoint::Specific(100, vec![0u8; 32]), 42);
        let tip = pallas_tip_to_torsten(&pallas_tip);
        assert_eq!(tip.block_number, BlockNo(42));
        assert_eq!(tip.point.slot(), Some(SlotNo(100)));
    }

    #[test]
    fn test_pallas_origin_tip() {
        let pallas_tip = PallasTip(PallasPoint::Origin, 0);
        let tip = pallas_tip_to_torsten(&pallas_tip);
        assert_eq!(tip, Tip::origin());
    }
}
