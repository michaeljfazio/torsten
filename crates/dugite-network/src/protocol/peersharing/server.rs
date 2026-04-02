//! PeerSharing server — serves filtered peer addresses to requesting peers.

use crate::error::ProtocolError;
use crate::mux::channel::MuxChannel;
use std::net::SocketAddr;

use super::{decode_message, encode_message, is_routable, PeerSharingMessage};

/// PeerSharing server that serves addresses filtered for routability.
pub struct PeerSharingServer;

impl PeerSharingServer {
    /// Run the PeerSharing server loop.
    ///
    /// `known_peers` is the set of routable peer addresses we can share.
    /// The server filters non-routable addresses before responding.
    pub async fn run(
        channel: &mut MuxChannel,
        known_peers: &[SocketAddr],
    ) -> Result<(), ProtocolError> {
        loop {
            let msg_bytes = channel.recv().await.map_err(ProtocolError::from)?;
            let msg = decode_message(&msg_bytes).map_err(|e| ProtocolError::CborDecode {
                protocol: "PeerSharing",
                reason: e,
            })?;

            match msg {
                PeerSharingMessage::MsgShareRequest(amount) => {
                    // Filter to only routable addresses and limit to requested amount
                    let peers: Vec<SocketAddr> = known_peers
                        .iter()
                        .filter(|addr| is_routable(&addr.ip()))
                        .take(amount as usize)
                        .copied()
                        .collect();

                    let reply = encode_message(&PeerSharingMessage::MsgSharePeers(peers));
                    channel.send(reply).await.map_err(ProtocolError::from)?;
                }
                PeerSharingMessage::MsgDone => {
                    return Ok(());
                }
                other => {
                    return Err(ProtocolError::AgencyViolation {
                        protocol: "PeerSharing",
                        state: "StIdle".to_string(),
                        received_tag: format!("{other:?}")
                            .as_bytes()
                            .first()
                            .copied()
                            .unwrap_or(0),
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
    use std::net::{IpAddr, Ipv4Addr};
    use tokio::sync::mpsc;

    fn make_test_channel() -> (
        MuxChannel,
        mpsc::Receiver<(u16, crate::mux::Direction, Bytes)>,
        mpsc::Sender<Bytes>,
    ) {
        let (egress_tx, egress_rx) = mpsc::channel(64);
        let (ingress_tx, ingress_rx) = mpsc::channel(64);
        let channel = MuxChannel::new(
            10,
            crate::mux::Direction::ResponderDir,
            egress_tx,
            ingress_rx,
            65536,
            std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        );
        (channel, egress_rx, ingress_tx)
    }

    #[tokio::test]
    async fn filters_non_routable_addresses() {
        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();

        let peers = vec![
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4)), 3001), // routable
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 3001), // RFC1918
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(8, 8, 4, 4)), 3001), // routable
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)), 3001), // RFC1918
        ];

        let handle =
            tokio::spawn(async move { PeerSharingServer::run(&mut channel, &peers).await });

        // Request 10 peers
        let req = encode_message(&PeerSharingMessage::MsgShareRequest(10));
        ingress_tx.send(Bytes::from(req)).await.unwrap();

        // Should only get the 2 routable addresses
        let (_, _, resp) = egress_rx.recv().await.unwrap();
        if let PeerSharingMessage::MsgSharePeers(addrs) = decode_message(&resp).unwrap() {
            assert_eq!(addrs.len(), 2);
            assert_eq!(addrs[0].ip(), IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4)));
            assert_eq!(addrs[1].ip(), IpAddr::V4(Ipv4Addr::new(8, 8, 4, 4)));
        } else {
            panic!("expected MsgSharePeers");
        }

        // Send MsgDone
        ingress_tx
            .send(Bytes::from(encode_message(&PeerSharingMessage::MsgDone)))
            .await
            .unwrap();
        handle.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn respects_amount_limit() {
        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();

        let peers: Vec<SocketAddr> = (1..=10u8)
            .map(|i| SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 2, 3, i)), 3001))
            .collect();

        let handle =
            tokio::spawn(async move { PeerSharingServer::run(&mut channel, &peers).await });

        // Request only 3
        let req = encode_message(&PeerSharingMessage::MsgShareRequest(3));
        ingress_tx.send(Bytes::from(req)).await.unwrap();

        let (_, _, resp) = egress_rx.recv().await.unwrap();
        if let PeerSharingMessage::MsgSharePeers(addrs) = decode_message(&resp).unwrap() {
            assert_eq!(addrs.len(), 3);
        } else {
            panic!("expected MsgSharePeers");
        }

        ingress_tx
            .send(Bytes::from(encode_message(&PeerSharingMessage::MsgDone)))
            .await
            .unwrap();
        handle.await.unwrap().unwrap();
    }
}
