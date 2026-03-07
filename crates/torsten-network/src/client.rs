use pallas_network::facades::PeerClient;
use pallas_network::miniprotocols::chainsync::NextResponse;
use pallas_network::miniprotocols::chainsync::Tip as PallasTip;
use pallas_network::miniprotocols::Point as PallasPoint;
use pallas_traverse::MultiEraHeader;
use std::net::SocketAddr;
use thiserror::Error;
use tokio::net::ToSocketAddrs;
use torsten_primitives::block::{Block, Point, Tip};
use torsten_primitives::hash::Hash32;
use torsten_primitives::time::{BlockNo, SlotNo};
use torsten_serialization::multi_era::decode_block;
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
            .unwrap_or_else(|_| "0.0.0.0:0".parse().unwrap());

        info!("connected to peer {remote_addr}");

        Ok(NodeToNodeClient { peer, remote_addr })
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

                let block =
                    decode_block(&cbor).map_err(|e| ClientError::BlockDecode(format!("{e}")))?;

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

    /// Request a batch of blocks from chain sync, fetching them via blockfetch in ranges.
    /// This is much faster than fetching one block at a time during initial sync.
    /// Returns a vec of events (RollForward/RollBackward) or Await if caught up.
    pub async fn request_next_batch(
        &mut self,
        batch_size: usize,
    ) -> Result<Vec<ChainSyncEvent>, ClientError> {
        let mut events = Vec::new();
        let mut pending_points: Vec<PallasPoint> = Vec::new();
        let mut latest_tip = None;

        // Collect headers from chainsync
        for _ in 0..batch_size {
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
                }
                NextResponse::RollBackward(point, tip) => {
                    // Flush any pending blocks before the rollback
                    if !pending_points.is_empty() {
                        let tip_ref = latest_tip.as_ref().unwrap();
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
                    break;
                }
                NextResponse::Await => {
                    // Flush pending blocks, then signal await
                    if !pending_points.is_empty() {
                        let tip_ref = latest_tip.as_ref().unwrap();
                        let fetched = self
                            .fetch_and_decode_range(&pending_points, tip_ref)
                            .await?;
                        events.extend(fetched);
                        pending_points.clear();
                    }
                    events.push(ChainSyncEvent::Await);
                    break;
                }
            }
        }

        // Fetch all pending blocks in one range
        if !pending_points.is_empty() {
            let tip_ref = latest_tip.as_ref().unwrap();
            let fetched = self
                .fetch_and_decode_range(&pending_points, tip_ref)
                .await?;
            events.extend(fetched);
        }

        Ok(events)
    }

    /// Fetch a range of blocks and decode them into ChainSyncEvents
    async fn fetch_and_decode_range(
        &mut self,
        points: &[PallasPoint],
        tip: &PallasTip,
    ) -> Result<Vec<ChainSyncEvent>, ClientError> {
        let first = points.first().unwrap().clone();
        let last = points.last().unwrap().clone();

        let bodies = self
            .peer
            .blockfetch()
            .fetch_range((first, last))
            .await
            .map_err(|e| ClientError::BlockFetch(format!("fetch range: {e}")))?;

        let torsten_tip = pallas_tip_to_torsten(tip);
        let mut events = Vec::with_capacity(bodies.len());
        for cbor in bodies {
            let block =
                decode_block(&cbor).map_err(|e| ClientError::BlockDecode(format!("{e}")))?;
            events.push(ChainSyncEvent::RollForward(
                Box::new(block),
                torsten_tip.clone(),
            ));
        }
        Ok(events)
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
            let block =
                decode_block(&body).map_err(|e| ClientError::BlockDecode(format!("{e}")))?;
            blocks.push(block);
        }

        debug!("fetched {} blocks", blocks.len());
        Ok(blocks)
    }

    /// Get the remote peer's address.
    pub fn remote_addr(&self) -> &SocketAddr {
        &self.remote_addr
    }

    /// Disconnect from the peer.
    pub async fn disconnect(self) {
        info!("disconnecting from peer {}", self.remote_addr);
        self.peer.abort().await;
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
