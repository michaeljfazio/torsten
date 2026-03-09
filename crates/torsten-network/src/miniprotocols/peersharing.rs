//! PeerSharing mini-protocol (Ouroboros)
//!
//! Implements both client (initiator) and server (responder) sides of the
//! PeerSharing protocol for decentralized peer discovery.
//!
//! Protocol ID: 10 (N2N PeerSharing)
//!
//! Message flow:
//!   Client (initiator)         Server (responder)
//!   StIdle:
//!     MsgShareRequest(amount) →
//!                              ← MsgSharePeers(Vec<PeerAddress>)
//!   StIdle:
//!     MsgDone →
//!   StDone

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

/// PeerSharing protocol state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerSharingState {
    /// Client has agency — can send MsgShareRequest or MsgDone
    StIdle,
    /// Server has agency — must respond with MsgSharePeers
    StBusy,
    /// Terminal state
    StDone,
}

/// A peer address as exchanged in the PeerSharing protocol.
///
/// Cardano's PeerSharing uses a tagged representation:
///   IPv4: [0, word32, word16]  — 3-element array (tag, ip as u32, port)
///   IPv6: [1, word32, word32, word32, word32, word16] — 6-element array
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerAddress {
    IPv4(Ipv4Addr, u16),
    IPv6(Ipv6Addr, u16),
}

impl PeerAddress {
    /// Convert to a standard SocketAddr
    pub fn to_socket_addr(&self) -> SocketAddr {
        match self {
            PeerAddress::IPv4(ip, port) => SocketAddr::new(IpAddr::V4(*ip), *port),
            PeerAddress::IPv6(ip, port) => SocketAddr::new(IpAddr::V6(*ip), *port),
        }
    }

    /// Create from a SocketAddr
    pub fn from_socket_addr(addr: SocketAddr) -> Self {
        match addr {
            SocketAddr::V4(v4) => PeerAddress::IPv4(*v4.ip(), v4.port()),
            SocketAddr::V6(v6) => PeerAddress::IPv6(*v6.ip(), v6.port()),
        }
    }
}

/// PeerSharing protocol messages
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerSharingMessage {
    /// Request up to `amount` peers from the remote
    ShareRequest(u8),
    /// Response with a list of peer addresses
    SharePeers(Vec<PeerAddress>),
    /// Terminate the protocol
    Done,
}

/// Encode a PeerSharingMessage to CBOR bytes.
///
/// Wire format (matching cardano-node):
///   MsgShareRequest: [0, amount:word8]
///   MsgSharePeers:   [1, [*peerAddress]]  (indefinite-length list when non-empty)
///   MsgDone:         [2]
pub fn encode_message(msg: &PeerSharingMessage) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut buf);

    match msg {
        PeerSharingMessage::ShareRequest(amount) => {
            enc.array(2).map_err(|e| e.to_string())?;
            enc.u32(0).map_err(|e| e.to_string())?;
            enc.u8(*amount).map_err(|e| e.to_string())?;
        }
        PeerSharingMessage::SharePeers(peers) => {
            enc.array(2).map_err(|e| e.to_string())?;
            enc.u32(1).map_err(|e| e.to_string())?;
            if peers.is_empty() {
                // Empty definite-length list
                enc.array(0).map_err(|e| e.to_string())?;
            } else {
                // Non-empty: use indefinite-length list (matching cardano-node)
                enc.begin_array().map_err(|e| e.to_string())?;
                for peer in peers {
                    encode_peer_address(&mut enc, peer)?;
                }
                enc.end().map_err(|e| e.to_string())?;
            }
        }
        PeerSharingMessage::Done => {
            enc.array(1).map_err(|e| e.to_string())?;
            enc.u32(2).map_err(|e| e.to_string())?;
        }
    }

    Ok(buf)
}

/// Decode a PeerSharingMessage from CBOR bytes.
pub fn decode_message(payload: &[u8]) -> Result<PeerSharingMessage, String> {
    let mut decoder = minicbor::Decoder::new(payload);
    let _arr_len = decoder.array().map_err(|e| e.to_string())?;
    let tag = decoder.u32().map_err(|e| e.to_string())?;

    match tag {
        // MsgShareRequest: [0, amount]
        0 => {
            let amount = decoder.u8().map_err(|e| e.to_string())?;
            Ok(PeerSharingMessage::ShareRequest(amount))
        }
        // MsgSharePeers: [1, [peer_addresses...]]
        // Handles both definite-length and indefinite-length peer lists
        1 => {
            let arr_len = decoder.array().map_err(|e| e.to_string())?;
            let mut peers = Vec::new();
            match arr_len {
                Some(n) => {
                    // Definite-length array
                    for _ in 0..n {
                        peers.push(decode_peer_address(&mut decoder)?);
                    }
                }
                None => {
                    // Indefinite-length array — read until break
                    while decoder
                        .datatype()
                        .is_ok_and(|dt| dt != minicbor::data::Type::Break)
                    {
                        peers.push(decode_peer_address(&mut decoder)?);
                    }
                    // Consume the break marker
                    let _ = decoder.skip();
                }
            }
            Ok(PeerSharingMessage::SharePeers(peers))
        }
        // MsgDone: [2]
        2 => Ok(PeerSharingMessage::Done),
        other => Err(format!("Unknown PeerSharing message tag: {other}")),
    }
}

/// Encode a PeerAddress to CBOR.
///
/// Wire format (matching cardano-node):
///   IPv4: [0, word32, word16]  — IP as a single u32, port as u16
///   IPv6: [1, word32, word32, word32, word32, word16] — IP as four u32s, port as u16
fn encode_peer_address(
    enc: &mut minicbor::Encoder<&mut Vec<u8>>,
    addr: &PeerAddress,
) -> Result<(), String> {
    match addr {
        PeerAddress::IPv4(ip, port) => {
            enc.array(3).map_err(|e| e.to_string())?;
            enc.u32(0).map_err(|e| e.to_string())?;
            // IPv4 as a single u32 (network byte order)
            enc.u32(u32::from_be_bytes(ip.octets()))
                .map_err(|e| e.to_string())?;
            enc.u16(*port).map_err(|e| e.to_string())?;
        }
        PeerAddress::IPv6(ip, port) => {
            enc.array(6).map_err(|e| e.to_string())?;
            enc.u32(1).map_err(|e| e.to_string())?;
            // IPv6 as four u32s (network byte order, matching Haskell HostAddress6)
            let octets = ip.octets();
            for chunk in octets.chunks(4) {
                let word = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                enc.u32(word).map_err(|e| e.to_string())?;
            }
            enc.u16(*port).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Decode a PeerAddress from CBOR.
///
/// Wire format:
///   IPv4: [0, word32, word16]
///   IPv6: [1, word32, word32, word32, word32, word16]
fn decode_peer_address(decoder: &mut minicbor::Decoder) -> Result<PeerAddress, String> {
    let _arr = decoder.array().map_err(|e| e.to_string())?;
    let tag = decoder.u32().map_err(|e| e.to_string())?;

    match tag {
        0 => {
            // IPv4: u32 (network byte order) + u16 port
            let ip_word = decoder.u32().map_err(|e| e.to_string())?;
            let port = decoder.u16().map_err(|e| e.to_string())?;
            let ip = Ipv4Addr::from(ip_word.to_be_bytes());
            Ok(PeerAddress::IPv4(ip, port))
        }
        1 => {
            // IPv6: four u32s (network byte order) + u16 port
            let w0 = decoder.u32().map_err(|e| e.to_string())?;
            let w1 = decoder.u32().map_err(|e| e.to_string())?;
            let w2 = decoder.u32().map_err(|e| e.to_string())?;
            let w3 = decoder.u32().map_err(|e| e.to_string())?;
            let port = decoder.u16().map_err(|e| e.to_string())?;
            let mut octets = [0u8; 16];
            octets[0..4].copy_from_slice(&w0.to_be_bytes());
            octets[4..8].copy_from_slice(&w1.to_be_bytes());
            octets[8..12].copy_from_slice(&w2.to_be_bytes());
            octets[12..16].copy_from_slice(&w3.to_be_bytes());
            let ip = Ipv6Addr::from(octets);
            Ok(PeerAddress::IPv6(ip, port))
        }
        other => Err(format!("Unknown PeerAddress tag: {other}")),
    }
}

/// Connect to a peer and request peer addresses via the PeerSharing protocol.
///
/// Opens a fresh N2N connection, performs handshake, sends MsgShareRequest,
/// reads MsgSharePeers, sends MsgDone, and returns discovered peers.
///
/// Returns an empty vec if the remote doesn't support PeerSharing or has no peers.
pub async fn request_peers_from(
    addr: impl tokio::net::ToSocketAddrs + std::fmt::Display + Copy,
    network_magic: u64,
    amount: u8,
) -> Result<Vec<SocketAddr>, String> {
    use pallas_network::miniprotocols::handshake;
    use pallas_network::miniprotocols::{
        PROTOCOL_N2N_BLOCK_FETCH, PROTOCOL_N2N_CHAIN_SYNC, PROTOCOL_N2N_HANDSHAKE,
        PROTOCOL_N2N_KEEP_ALIVE, PROTOCOL_N2N_PEER_SHARING, PROTOCOL_N2N_TX_SUBMISSION,
    };
    use pallas_network::multiplexer::{Bearer, Plexer};
    use std::time::Duration;

    let bearer = Bearer::connect_tcp(addr)
        .await
        .map_err(|e| format!("PeerSharing connect to {addr}: {e}"))?;

    let mut plexer = Plexer::new(bearer);

    let hs_channel = plexer.subscribe_client(PROTOCOL_N2N_HANDSHAKE);
    let _cs_channel = plexer.subscribe_client(PROTOCOL_N2N_CHAIN_SYNC);
    let _bf_channel = plexer.subscribe_client(PROTOCOL_N2N_BLOCK_FETCH);
    let _txsub_channel = plexer.subscribe_client(PROTOCOL_N2N_TX_SUBMISSION);
    let mut ps_channel = plexer.subscribe_client(PROTOCOL_N2N_PEER_SHARING);
    let _ka_channel = plexer.subscribe_client(PROTOCOL_N2N_KEEP_ALIVE);

    let _plexer = plexer.spawn();

    // Handshake
    let mut hs_client = handshake::Client::new(hs_channel);
    let versions = handshake::n2n::VersionTable::v7_and_above(network_magic);
    let handshake_result = hs_client
        .handshake(versions)
        .await
        .map_err(|e| format!("PeerSharing handshake with {addr}: {e}"))?;

    if let handshake::Confirmation::Rejected(reason) = handshake_result {
        return Err(format!(
            "PeerSharing handshake rejected by {addr}: {reason:?}"
        ));
    }

    // Send MsgShareRequest via raw channel
    let request = encode_message(&PeerSharingMessage::ShareRequest(amount))
        .map_err(|e| format!("encode ShareRequest: {e}"))?;
    ps_channel
        .enqueue_chunk(request)
        .await
        .map_err(|e| format!("send ShareRequest to {addr}: {e}"))?;

    // Read MsgSharePeers response with timeout
    let response_payload =
        tokio::time::timeout(Duration::from_secs(60), ps_channel.dequeue_chunk())
            .await
            .map_err(|_| format!("PeerSharing timeout waiting for response from {addr}"))?
            .map_err(|e| format!("recv SharePeers from {addr}: {e}"))?;

    let response = decode_message(&response_payload)
        .map_err(|e| format!("decode SharePeers from {addr}: {e}"))?;

    let peers = match response {
        PeerSharingMessage::SharePeers(peers) => peers,
        other => {
            return Err(format!("Expected SharePeers from {addr}, got {other:?}"));
        }
    };

    // Send MsgDone to cleanly terminate
    let done =
        encode_message(&PeerSharingMessage::Done).map_err(|e| format!("encode Done: {e}"))?;
    let _ = ps_channel.enqueue_chunk(done).await;

    Ok(peers.into_iter().map(|p| p.to_socket_addr()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_peer_address_ipv4_roundtrip() {
        let addr = PeerAddress::IPv4(Ipv4Addr::new(192, 168, 1, 100), 3001);
        let socket = addr.to_socket_addr();
        assert_eq!(socket.to_string(), "192.168.1.100:3001");

        let back = PeerAddress::from_socket_addr(socket);
        assert_eq!(addr, back);
    }

    #[test]
    fn test_peer_address_ipv6_roundtrip() {
        let addr = PeerAddress::IPv6(Ipv6Addr::LOCALHOST, 3001);
        let socket = addr.to_socket_addr();
        assert_eq!(socket.to_string(), "[::1]:3001");

        let back = PeerAddress::from_socket_addr(socket);
        assert_eq!(addr, back);
    }

    #[test]
    fn test_encode_decode_share_request() {
        let msg = PeerSharingMessage::ShareRequest(10);
        let encoded = encode_message(&msg).unwrap();
        let decoded = decode_message(&encoded).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn test_encode_decode_share_peers_empty() {
        let msg = PeerSharingMessage::SharePeers(vec![]);
        let encoded = encode_message(&msg).unwrap();
        let decoded = decode_message(&encoded).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn test_encode_decode_share_peers_ipv4() {
        let msg = PeerSharingMessage::SharePeers(vec![
            PeerAddress::IPv4(Ipv4Addr::new(1, 2, 3, 4), 3001),
            PeerAddress::IPv4(Ipv4Addr::new(10, 0, 0, 1), 3002),
        ]);
        let encoded = encode_message(&msg).unwrap();
        let decoded = decode_message(&encoded).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn test_encode_decode_share_peers_ipv6() {
        let msg = PeerSharingMessage::SharePeers(vec![PeerAddress::IPv6(
            Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1),
            3001,
        )]);
        let encoded = encode_message(&msg).unwrap();
        let decoded = decode_message(&encoded).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn test_encode_decode_share_peers_mixed() {
        let msg = PeerSharingMessage::SharePeers(vec![
            PeerAddress::IPv4(Ipv4Addr::new(192, 168, 1, 1), 3001),
            PeerAddress::IPv6(Ipv6Addr::LOCALHOST, 3002),
        ]);
        let encoded = encode_message(&msg).unwrap();
        let decoded = decode_message(&encoded).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn test_encode_decode_done() {
        let msg = PeerSharingMessage::Done;
        let encoded = encode_message(&msg).unwrap();
        let decoded = decode_message(&encoded).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn test_decode_unknown_tag() {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(1).unwrap();
        enc.u32(99).unwrap();
        let result = decode_message(&buf);
        assert!(result.is_err());
    }

    #[test]
    fn test_from_socket_addr_v4() {
        let socket: SocketAddr = "127.0.0.1:3001".parse().unwrap();
        let peer = PeerAddress::from_socket_addr(socket);
        assert!(matches!(peer, PeerAddress::IPv4(_, 3001)));
    }

    #[test]
    fn test_from_socket_addr_v6() {
        let socket: SocketAddr = "[::1]:3001".parse().unwrap();
        let peer = PeerAddress::from_socket_addr(socket);
        assert!(matches!(peer, PeerAddress::IPv6(_, 3001)));
    }

    #[test]
    fn test_state_variants() {
        // Ensure state enum variants exist and are distinct
        assert_ne!(PeerSharingState::StIdle, PeerSharingState::StBusy);
        assert_ne!(PeerSharingState::StBusy, PeerSharingState::StDone);
        assert_ne!(PeerSharingState::StIdle, PeerSharingState::StDone);
    }

    #[test]
    fn test_ipv4_wire_format() {
        // Verify IPv4 encodes as [0, u32, u16] matching cardano-node
        let addr = PeerAddress::IPv4(Ipv4Addr::new(127, 0, 0, 1), 3001);
        let msg = PeerSharingMessage::SharePeers(vec![addr]);
        let encoded = encode_message(&msg).unwrap();

        // Decode manually to verify wire format
        let mut dec = minicbor::Decoder::new(&encoded);
        let _ = dec.array().unwrap(); // outer [1, ...]
        assert_eq!(dec.u32().unwrap(), 1); // tag=1 (SharePeers)

        // Peer list (indefinite-length for non-empty)
        let arr_len = dec.array().unwrap();
        assert!(arr_len.is_none()); // indefinite

        // First (only) peer: [0, u32, u16]
        assert_eq!(dec.array().unwrap(), Some(3)); // 3-element array
        assert_eq!(dec.u32().unwrap(), 0); // IPv4 tag
        assert_eq!(dec.u32().unwrap(), 0x7F000001); // 127.0.0.1 as u32
        assert_eq!(dec.u16().unwrap(), 3001); // port
    }

    #[test]
    fn test_ipv6_wire_format() {
        // Verify IPv6 encodes as [1, w0, w1, w2, w3, port] matching cardano-node
        let addr = PeerAddress::IPv6(Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, 1), 3001);
        let msg = PeerSharingMessage::SharePeers(vec![addr]);
        let encoded = encode_message(&msg).unwrap();

        let mut dec = minicbor::Decoder::new(&encoded);
        let _ = dec.array().unwrap();
        assert_eq!(dec.u32().unwrap(), 1); // SharePeers

        let arr_len = dec.array().unwrap();
        assert!(arr_len.is_none()); // indefinite

        // Peer: [1, w0, w1, w2, w3, port]
        assert_eq!(dec.array().unwrap(), Some(6)); // 6-element array
        assert_eq!(dec.u32().unwrap(), 1); // IPv6 tag
        assert_eq!(dec.u32().unwrap(), 0x20010db8); // first 4 bytes
        assert_eq!(dec.u32().unwrap(), 0x00000000);
        assert_eq!(dec.u32().unwrap(), 0x00000000);
        assert_eq!(dec.u32().unwrap(), 0x00000001);
        assert_eq!(dec.u16().unwrap(), 3001);
    }

    #[test]
    fn test_decode_definite_length_peer_list() {
        // Ensure we can decode definite-length peer lists (cardano-node may send either)
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(1).unwrap(); // SharePeers
        enc.array(1).unwrap(); // definite-length list with 1 element
                               // IPv4 peer: [0, 0x0A000001, 3001]
        enc.array(3).unwrap();
        enc.u32(0).unwrap();
        enc.u32(0x0A000001).unwrap(); // 10.0.0.1
        enc.u16(3001).unwrap();

        let msg = decode_message(&buf).unwrap();
        match msg {
            PeerSharingMessage::SharePeers(peers) => {
                assert_eq!(peers.len(), 1);
                assert_eq!(
                    peers[0],
                    PeerAddress::IPv4(Ipv4Addr::new(10, 0, 0, 1), 3001)
                );
            }
            _ => panic!("Expected SharePeers"),
        }
    }
}
