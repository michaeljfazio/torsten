//! PeerSharing mini-protocol codec, client, server, and address filtering.
//!
//! Allows peers to exchange routable addresses for peer discovery.
//!
//! ## Wire format
//! - `MsgShareRequest` = `[0, amount]`
//! - `MsgSharePeers` = `[1, [*addr]]`
//! - `MsgDone` = `[2]`
//!
//! ## Address encoding
//! - IPv4: `[0, ipv4_u32, port_u16]`
//! - IPv6: `[1, w0_u32, w1_u32, w2_u32, w3_u32, port_u16]`
//! - No hostname variant (only used for DNS relays, not peer sharing).
//!
//! ## Address filtering
//! The server filters out non-routable addresses before sharing:
//! RFC1918 (10/8, 172.16/12, 192.168/16), CGNAT (100.64/10), loopback,
//! IPv6 ULA (fc00::/7), link-local (fe80::/10), IPv6 loopback (::1).

pub mod client;
pub mod server;

use minicbor::{Decoder, Encoder};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

/// PeerSharing protocol messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerSharingMessage {
    /// Client requests up to `amount` peer addresses.
    MsgShareRequest(u8),
    /// Server replies with peer addresses.
    MsgSharePeers(Vec<SocketAddr>),
    /// Terminate the protocol.
    MsgDone,
}

// CBOR message tags
const TAG_SHARE_REQUEST: u64 = 0;
const TAG_SHARE_PEERS: u64 = 1;
const TAG_DONE: u64 = 2;

/// Encode a PeerSharing message as CBOR.
pub fn encode_message(msg: &PeerSharingMessage) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut enc = Encoder::new(&mut buf);
    match msg {
        PeerSharingMessage::MsgShareRequest(amount) => {
            enc.array(2).expect("infallible");
            enc.u64(TAG_SHARE_REQUEST).expect("infallible");
            enc.u8(*amount).expect("infallible");
        }
        PeerSharingMessage::MsgSharePeers(addrs) => {
            enc.array(2).expect("infallible");
            enc.u64(TAG_SHARE_PEERS).expect("infallible");
            enc.array(addrs.len() as u64).expect("infallible");
            for addr in addrs {
                encode_address(&mut enc, addr);
            }
        }
        PeerSharingMessage::MsgDone => {
            enc.array(1).expect("infallible");
            enc.u64(TAG_DONE).expect("infallible");
        }
    }
    buf
}

/// Decode a PeerSharing message from CBOR bytes.
pub fn decode_message(data: &[u8]) -> Result<PeerSharingMessage, String> {
    let mut dec = Decoder::new(data);
    let _arr_len = dec.array().map_err(|e| e.to_string())?;
    let tag = dec.u64().map_err(|e| e.to_string())?;

    match tag {
        TAG_SHARE_REQUEST => {
            let amount = dec.u8().map_err(|e| e.to_string())?;
            Ok(PeerSharingMessage::MsgShareRequest(amount))
        }
        TAG_SHARE_PEERS => {
            let len = dec
                .array()
                .map_err(|e| e.to_string())?
                .ok_or("indefinite array not supported")?;
            let mut addrs = Vec::with_capacity(len as usize);
            for _ in 0..len {
                addrs.push(decode_address(&mut dec)?);
            }
            Ok(PeerSharingMessage::MsgSharePeers(addrs))
        }
        TAG_DONE => Ok(PeerSharingMessage::MsgDone),
        _ => Err(format!("unknown PeerSharing message tag: {tag}")),
    }
}

/// Encode a socket address as CBOR: `[0, ipv4, port]` or `[1, w0, w1, w2, w3, port]`.
fn encode_address(enc: &mut Encoder<&mut Vec<u8>>, addr: &SocketAddr) {
    match addr.ip() {
        IpAddr::V4(ip) => {
            enc.array(3).expect("infallible");
            enc.u8(0).expect("infallible"); // IPv4 tag
            enc.u32(u32::from(ip)).expect("infallible");
            enc.u16(addr.port()).expect("infallible");
        }
        IpAddr::V6(ip) => {
            let segments = ip.segments();
            enc.array(6).expect("infallible");
            enc.u8(1).expect("infallible"); // IPv6 tag
                                            // Encode as 4 × u32 (pairs of 16-bit segments)
            enc.u32(((segments[0] as u32) << 16) | segments[1] as u32)
                .expect("infallible");
            enc.u32(((segments[2] as u32) << 16) | segments[3] as u32)
                .expect("infallible");
            enc.u32(((segments[4] as u32) << 16) | segments[5] as u32)
                .expect("infallible");
            enc.u32(((segments[6] as u32) << 16) | segments[7] as u32)
                .expect("infallible");
            enc.u16(addr.port()).expect("infallible");
        }
    }
}

/// Decode a socket address from CBOR.
fn decode_address(dec: &mut Decoder<'_>) -> Result<SocketAddr, String> {
    let _arr_len = dec.array().map_err(|e| e.to_string())?;
    let addr_type = dec.u8().map_err(|e| e.to_string())?;
    match addr_type {
        0 => {
            // IPv4: u32 + port
            let ip_u32 = dec.u32().map_err(|e| e.to_string())?;
            let port = dec.u16().map_err(|e| e.to_string())?;
            let ip = Ipv4Addr::from(ip_u32);
            Ok(SocketAddr::new(IpAddr::V4(ip), port))
        }
        1 => {
            // IPv6: 4 × u32 + port
            let w0 = dec.u32().map_err(|e| e.to_string())?;
            let w1 = dec.u32().map_err(|e| e.to_string())?;
            let w2 = dec.u32().map_err(|e| e.to_string())?;
            let w3 = dec.u32().map_err(|e| e.to_string())?;
            let port = dec.u16().map_err(|e| e.to_string())?;
            let ip = Ipv6Addr::new(
                (w0 >> 16) as u16,
                w0 as u16,
                (w1 >> 16) as u16,
                w1 as u16,
                (w2 >> 16) as u16,
                w2 as u16,
                (w3 >> 16) as u16,
                w3 as u16,
            );
            Ok(SocketAddr::new(IpAddr::V6(ip), port))
        }
        _ => Err(format!("unknown address type: {addr_type}")),
    }
}

/// Check if an IP address is routable (suitable for peer sharing).
///
/// Returns `false` for:
/// - RFC1918: 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16
/// - CGNAT: 100.64.0.0/10
/// - Loopback: 127.0.0.0/8
/// - Link-local: 169.254.0.0/16
/// - 0.0.0.0
/// - IPv6 ULA: fc00::/7
/// - IPv6 link-local: fe80::/10
/// - IPv6 loopback: ::1
/// - IPv6 unspecified: ::
pub fn is_routable(addr: &IpAddr) -> bool {
    match addr {
        IpAddr::V4(ip) => {
            let octets = ip.octets();
            // 10.0.0.0/8
            if octets[0] == 10 {
                return false;
            }
            // 172.16.0.0/12
            if octets[0] == 172 && (octets[1] & 0xF0) == 16 {
                return false;
            }
            // 192.168.0.0/16
            if octets[0] == 192 && octets[1] == 168 {
                return false;
            }
            // 100.64.0.0/10 (CGNAT)
            if octets[0] == 100 && (octets[1] & 0xC0) == 64 {
                return false;
            }
            // 127.0.0.0/8 (loopback)
            if octets[0] == 127 {
                return false;
            }
            // 169.254.0.0/16 (link-local)
            if octets[0] == 169 && octets[1] == 254 {
                return false;
            }
            // 0.0.0.0
            if ip.is_unspecified() {
                return false;
            }
            true
        }
        IpAddr::V6(ip) => {
            // ::1 (loopback)
            if ip.is_loopback() {
                return false;
            }
            // :: (unspecified)
            if ip.is_unspecified() {
                return false;
            }
            let segments = ip.segments();
            // fc00::/7 (ULA)
            if (segments[0] & 0xFE00) == 0xFC00 {
                return false;
            }
            // fe80::/10 (link-local)
            if (segments[0] & 0xFFC0) == 0xFE80 {
                return false;
            }
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msg_share_request_roundtrip() {
        let msg = PeerSharingMessage::MsgShareRequest(5);
        let encoded = encode_message(&msg);
        let decoded = decode_message(&encoded).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn msg_share_peers_ipv4_roundtrip() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4)), 3001);
        let msg = PeerSharingMessage::MsgSharePeers(vec![addr]);
        let encoded = encode_message(&msg);
        let decoded = decode_message(&encoded).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn msg_share_peers_ipv6_roundtrip() {
        let addr = SocketAddr::new(
            IpAddr::V6(Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, 1)),
            3001,
        );
        let msg = PeerSharingMessage::MsgSharePeers(vec![addr]);
        let encoded = encode_message(&msg);
        let decoded = decode_message(&encoded).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn msg_done_roundtrip() {
        let encoded = encode_message(&PeerSharingMessage::MsgDone);
        let decoded = decode_message(&encoded).unwrap();
        assert_eq!(decoded, PeerSharingMessage::MsgDone);
    }

    // ─── Address filtering tests ───

    #[test]
    fn routable_public_ipv4() {
        assert!(is_routable(&IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
        assert!(is_routable(&IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4))));
    }

    #[test]
    fn non_routable_rfc1918_10() {
        assert!(!is_routable(&IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
        assert!(!is_routable(&IpAddr::V4(Ipv4Addr::new(10, 255, 255, 255))));
    }

    #[test]
    fn non_routable_rfc1918_172() {
        assert!(!is_routable(&IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1))));
        assert!(!is_routable(&IpAddr::V4(Ipv4Addr::new(172, 31, 255, 255))));
        // 172.32.0.0 is routable
        assert!(is_routable(&IpAddr::V4(Ipv4Addr::new(172, 32, 0, 1))));
    }

    #[test]
    fn non_routable_rfc1918_192() {
        assert!(!is_routable(&IpAddr::V4(Ipv4Addr::new(192, 168, 0, 1))));
        assert!(!is_routable(&IpAddr::V4(Ipv4Addr::new(192, 168, 255, 255))));
    }

    #[test]
    fn non_routable_cgnat() {
        assert!(!is_routable(&IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1))));
        assert!(!is_routable(&IpAddr::V4(Ipv4Addr::new(100, 127, 255, 255))));
        // 100.128.0.0 is routable
        assert!(is_routable(&IpAddr::V4(Ipv4Addr::new(100, 128, 0, 1))));
    }

    #[test]
    fn non_routable_loopback() {
        assert!(!is_routable(&IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))));
        assert!(!is_routable(&IpAddr::V4(Ipv4Addr::LOCALHOST)));
    }

    #[test]
    fn non_routable_unspecified() {
        assert!(!is_routable(&IpAddr::V4(Ipv4Addr::UNSPECIFIED)));
        assert!(!is_routable(&IpAddr::V6(Ipv6Addr::UNSPECIFIED)));
    }

    #[test]
    fn non_routable_ipv6_ula() {
        // fc00::/7 — ULA addresses
        assert!(!is_routable(&IpAddr::V6(Ipv6Addr::new(
            0xFC00, 0, 0, 0, 0, 0, 0, 1
        ))));
        assert!(!is_routable(&IpAddr::V6(Ipv6Addr::new(
            0xFD00, 0, 0, 0, 0, 0, 0, 1
        ))));
    }

    #[test]
    fn non_routable_ipv6_link_local() {
        assert!(!is_routable(&IpAddr::V6(Ipv6Addr::new(
            0xFE80, 0, 0, 0, 0, 0, 0, 1
        ))));
    }

    #[test]
    fn non_routable_ipv6_loopback() {
        assert!(!is_routable(&IpAddr::V6(Ipv6Addr::LOCALHOST)));
    }

    #[test]
    fn routable_public_ipv6() {
        assert!(is_routable(&IpAddr::V6(Ipv6Addr::new(
            0x2001, 0x0db8, 0, 0, 0, 0, 0, 1
        ))));
    }
}
