//! PeerSharing client — requests peer addresses from hot peers.

use crate::error::ProtocolError;
use crate::mux::channel::MuxChannel;
use std::net::SocketAddr;

use super::{decode_message, encode_message, PeerSharingMessage};

/// PeerSharing client that requests addresses from a remote peer.
pub struct PeerSharingClient;

impl PeerSharingClient {
    /// Request peer addresses from the remote peer.
    ///
    /// Sends `MsgShareRequest(amount)` and returns the received addresses.
    pub async fn request_peers(
        channel: &mut MuxChannel,
        amount: u8,
    ) -> Result<Vec<SocketAddr>, ProtocolError> {
        let req = encode_message(&PeerSharingMessage::MsgShareRequest(amount));
        channel.send(req).await.map_err(ProtocolError::from)?;

        let response_bytes = channel.recv().await.map_err(ProtocolError::from)?;
        let response = decode_message(&response_bytes).map_err(|e| ProtocolError::CborDecode {
            protocol: "PeerSharing",
            reason: e,
        })?;

        match response {
            PeerSharingMessage::MsgSharePeers(addrs) => Ok(addrs),
            other => Err(ProtocolError::StateViolation {
                protocol: "PeerSharing",
                expected: "MsgSharePeers".to_string(),
                actual: format!("{other:?}"),
            }),
        }
    }

    /// Send MsgDone to terminate the PeerSharing protocol.
    pub async fn done(channel: &mut MuxChannel) -> Result<(), ProtocolError> {
        let msg = encode_message(&PeerSharingMessage::MsgDone);
        channel.send(msg).await.map_err(ProtocolError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::peersharing::{decode_message, encode_message, PeerSharingMessage};
    use bytes::Bytes;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    /// Create a test MuxChannel with egress receiver and ingress sender.
    fn make_test_channel() -> (
        MuxChannel,
        mpsc::Receiver<(u16, crate::mux::Direction, Bytes)>,
        mpsc::Sender<Bytes>,
    ) {
        let (egress_tx, egress_rx) = mpsc::channel(64);
        let (ingress_tx, ingress_rx) = mpsc::channel(64);
        let channel = MuxChannel::new(
            11, // PeerSharing protocol ID
            crate::mux::Direction::InitiatorDir,
            egress_tx,
            ingress_rx,
            1_000_000,
            Arc::new(AtomicUsize::new(0)),
        );
        (channel, egress_rx, ingress_tx)
    }

    #[tokio::test]
    async fn request_peers_ipv4() {
        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();

        let expected_addrs = vec![
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4)), 3001),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(5, 6, 7, 8)), 3002),
        ];

        let addrs_clone = expected_addrs.clone();
        let handle =
            tokio::spawn(async move { PeerSharingClient::request_peers(&mut channel, 5).await });

        // Read the MsgShareRequest from egress.
        let (_, _, req_bytes) = egress_rx.recv().await.unwrap();
        let req = decode_message(&req_bytes).unwrap();
        assert_eq!(req, PeerSharingMessage::MsgShareRequest(5));

        // Send MsgSharePeers response.
        let resp = encode_message(&PeerSharingMessage::MsgSharePeers(addrs_clone));
        ingress_tx.send(Bytes::from(resp)).await.unwrap();

        let result = handle.await.unwrap().unwrap();
        assert_eq!(result, expected_addrs);
    }

    #[tokio::test]
    async fn request_peers_ipv6() {
        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();

        let addr = SocketAddr::new(
            IpAddr::V6(Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, 1)),
            3001,
        );
        let expected = vec![addr];

        let handle =
            tokio::spawn(async move { PeerSharingClient::request_peers(&mut channel, 1).await });

        let (_, _, _req_bytes) = egress_rx.recv().await.unwrap();

        let resp = encode_message(&PeerSharingMessage::MsgSharePeers(vec![addr]));
        ingress_tx.send(Bytes::from(resp)).await.unwrap();

        let result = handle.await.unwrap().unwrap();
        assert_eq!(result, expected);
    }

    #[tokio::test]
    async fn request_peers_empty_response() {
        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();

        let handle =
            tokio::spawn(async move { PeerSharingClient::request_peers(&mut channel, 3).await });

        let (_, _, _req_bytes) = egress_rx.recv().await.unwrap();

        // Server responds with empty peer list.
        let resp = encode_message(&PeerSharingMessage::MsgSharePeers(vec![]));
        ingress_tx.send(Bytes::from(resp)).await.unwrap();

        let result = handle.await.unwrap().unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn request_peers_unexpected_response_returns_error() {
        // If server sends MsgDone instead of MsgSharePeers, it's a state violation.
        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();

        let handle =
            tokio::spawn(async move { PeerSharingClient::request_peers(&mut channel, 1).await });

        let (_, _, _req_bytes) = egress_rx.recv().await.unwrap();

        // Send MsgDone instead of MsgSharePeers.
        let resp = encode_message(&PeerSharingMessage::MsgDone);
        ingress_tx.send(Bytes::from(resp)).await.unwrap();

        let result = handle.await.unwrap();
        assert!(result.is_err(), "unexpected response should error");
        match result.unwrap_err() {
            ProtocolError::StateViolation { protocol, .. } => {
                assert_eq!(protocol, "PeerSharing");
            }
            other => panic!("expected StateViolation, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn request_peers_mixed_ipv4_ipv6() {
        let (mut channel, mut egress_rx, ingress_tx) = make_test_channel();

        let addrs = vec![
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)), 3001),
            SocketAddr::new(
                IpAddr::V6(Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, 1)),
                3002,
            ),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)), 3003),
        ];
        let expected = addrs.clone();

        let handle =
            tokio::spawn(async move { PeerSharingClient::request_peers(&mut channel, 10).await });

        let (_, _, _req_bytes) = egress_rx.recv().await.unwrap();
        let resp = encode_message(&PeerSharingMessage::MsgSharePeers(addrs));
        ingress_tx.send(Bytes::from(resp)).await.unwrap();

        let result = handle.await.unwrap().unwrap();
        assert_eq!(result, expected);
    }

    #[tokio::test]
    async fn done_sends_msg_done() {
        let (mut channel, mut egress_rx, _ingress_tx) = make_test_channel();

        PeerSharingClient::done(&mut channel).await.unwrap();

        let (_, _, bytes) = egress_rx.recv().await.unwrap();
        let msg = decode_message(&bytes).unwrap();
        assert_eq!(msg, PeerSharingMessage::MsgDone);
    }
}
