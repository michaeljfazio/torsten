//! Ouroboros handshake protocol implementation.
//!
//! The handshake is the first mini-protocol exchanged on a new connection (protocol ID 0).
//! It negotiates the protocol version and version data before any other communication.
//!
//! ## Client flow
//! 1. Send `MsgProposeVersions` with our supported versions + version data
//! 2. Receive one of:
//!    - `MsgAcceptVersion(version, version_data)` — negotiation succeeded
//!    - `MsgRefuse(version, reason)` — rejected
//!    - `MsgQueryReply(versions)` — if remote was in query mode
//!    - `MsgProposeVersions` — simultaneous open detected
//!
//! ## Server flow
//! 1. Receive `MsgProposeVersions` with remote's supported versions
//! 2. Select highest common version, verify magic, send `MsgAcceptVersion` or `MsgRefuse`
//!
//! ## Wire format (CBOR)
//! - `MsgProposeVersions` = `[0, {version: version_data, ...}]`
//! - `MsgAcceptVersion` = `[1, version, version_data]`
//! - `MsgRefuse` = `[2, [version_mismatch_tag, ...]]`
//! - `MsgQueryReply` = `[3, {version: version_data, ...}]`

pub mod n2c;
pub mod n2n;

use minicbor::{Decoder, Encoder};
use std::collections::BTreeMap;

use crate::error::HandshakeError;
use crate::mux::channel::MuxChannel;

pub use n2c::N2CVersionData;
pub use n2n::N2NVersionData;

/// Result of a successful handshake.
#[derive(Debug, Clone)]
pub struct HandshakeResult {
    /// Negotiated protocol version.
    pub version: u16,
    /// Whether simultaneous open was detected (received MsgProposeVersions instead of MsgAccept).
    pub simultaneous_open: bool,
}

// ─── CBOR Message Tags ───

/// MsgProposeVersions tag (client → server).
const MSG_PROPOSE_VERSIONS: u64 = 0;
/// MsgAcceptVersion tag (server → client).
const MSG_ACCEPT_VERSION: u64 = 1;
/// MsgRefuse tag (server → client).
const MSG_REFUSE: u64 = 2;
/// MsgQueryReply tag (server → client, query mode only).
#[allow(dead_code)]
const MSG_QUERY_REPLY: u64 = 3;

/// Run the handshake as the client (initiator) for N2N connections.
///
/// Sends `MsgProposeVersions` with our version table, then waits for the server's response.
/// Returns the negotiated version and whether simultaneous open was detected.
pub async fn run_n2n_handshake_client(
    channel: &mut MuxChannel,
    our_data: &N2NVersionData,
) -> Result<HandshakeResult, HandshakeError> {
    // Build and send MsgProposeVersions
    let msg = encode_propose_versions_n2n(n2n::N2N_VERSIONS, our_data);
    channel.send(msg).await.map_err(HandshakeError::from)?;

    // Receive response
    let response = channel.recv().await.map_err(HandshakeError::from)?;

    decode_handshake_response(&response)
}

/// Run the handshake as the server (responder) for N2N connections.
///
/// Receives `MsgProposeVersions`, selects the highest common version, validates
/// magic, and sends `MsgAcceptVersion` or `MsgRefuse`.
pub async fn run_n2n_handshake_server(
    channel: &mut MuxChannel,
    our_data: &N2NVersionData,
) -> Result<HandshakeResult, HandshakeError> {
    // Receive MsgProposeVersions
    let proposal = channel.recv().await.map_err(HandshakeError::from)?;

    let remote_versions = decode_propose_versions_n2n(&proposal)?;

    // Find highest common version
    for &our_version in n2n::N2N_VERSIONS {
        if let Some(their_data) = remote_versions.get(&our_version) {
            // Check if we can accept this version
            if let Some(_accepted) = our_data.accept(their_data) {
                // Send MsgAcceptVersion
                let msg = encode_accept_version_n2n(our_version, our_data);
                channel.send(msg).await.map_err(HandshakeError::from)?;
                return Ok(HandshakeResult {
                    version: our_version,
                    simultaneous_open: false,
                });
            } else {
                // Magic mismatch — refuse
                let msg = encode_refuse(our_version, "network magic mismatch");
                channel.send(msg).await.map_err(HandshakeError::from)?;
                return Err(HandshakeError::NetworkMagicMismatch {
                    ours: our_data.network_magic,
                    theirs: their_data.network_magic,
                });
            }
        }
    }

    // No common version
    let our_versions: Vec<u16> = n2n::N2N_VERSIONS.to_vec();
    let their_versions: Vec<u16> = remote_versions.keys().copied().collect();
    let msg = encode_refuse(0, "no common version");
    let _ = channel.send(msg).await;
    Err(HandshakeError::VersionMismatch {
        ours: our_versions,
        theirs: their_versions,
    })
}

/// Run the handshake as the client (initiator) for N2C connections.
pub async fn run_n2c_handshake_client(
    channel: &mut MuxChannel,
    our_data: &N2CVersionData,
) -> Result<HandshakeResult, HandshakeError> {
    let msg = encode_propose_versions_n2c(n2c::N2C_VERSIONS, our_data);
    channel.send(msg).await.map_err(HandshakeError::from)?;

    let response = channel.recv().await.map_err(HandshakeError::from)?;

    // Decode, converting wire versions back to logical
    decode_handshake_response_n2c(&response)
}

/// Run the handshake as the server (responder) for N2C connections.
pub async fn run_n2c_handshake_server(
    channel: &mut MuxChannel,
    our_data: &N2CVersionData,
) -> Result<HandshakeResult, HandshakeError> {
    let proposal = channel.recv().await.map_err(HandshakeError::from)?;

    let remote_versions = decode_propose_versions_n2c(&proposal)?;

    // Find highest common version (N2C versions are already logical after decode)
    for &our_version in n2c::N2C_VERSIONS {
        if let Some(their_data) = remote_versions.get(&our_version) {
            if let Some(_accepted) = our_data.accept(their_data) {
                let msg = encode_accept_version_n2c(our_version, our_data);
                channel.send(msg).await.map_err(HandshakeError::from)?;
                return Ok(HandshakeResult {
                    version: our_version,
                    simultaneous_open: false,
                });
            } else {
                let msg = encode_refuse(
                    n2c::encode_n2c_version(our_version),
                    "network magic mismatch",
                );
                channel.send(msg).await.map_err(HandshakeError::from)?;
                return Err(HandshakeError::NetworkMagicMismatch {
                    ours: our_data.network_magic,
                    theirs: their_data.network_magic,
                });
            }
        }
    }

    let our_versions: Vec<u16> = n2c::N2C_VERSIONS.to_vec();
    let their_versions: Vec<u16> = remote_versions.keys().copied().collect();
    let msg = encode_refuse(0, "no common version");
    let _ = channel.send(msg).await;
    Err(HandshakeError::VersionMismatch {
        ours: our_versions,
        theirs: their_versions,
    })
}

// ─── Encoding helpers ───

/// Encode MsgProposeVersions for N2N: `[0, {version: version_data, ...}]`.
fn encode_propose_versions_n2n(versions: &[u16], data: &N2NVersionData) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut enc = Encoder::new(&mut buf);
    enc.array(2).expect("infallible");
    enc.u64(MSG_PROPOSE_VERSIONS).expect("infallible");
    enc.map(versions.len() as u64).expect("infallible");
    for &v in versions {
        enc.u16(v).expect("infallible");
        data.encode(&mut enc);
    }
    buf
}

/// Encode MsgProposeVersions for N2C with bit-15 wire encoding.
fn encode_propose_versions_n2c(versions: &[u16], data: &N2CVersionData) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut enc = Encoder::new(&mut buf);
    enc.array(2).expect("infallible");
    enc.u64(MSG_PROPOSE_VERSIONS).expect("infallible");
    enc.map(versions.len() as u64).expect("infallible");
    for &v in versions {
        enc.u16(n2c::encode_n2c_version(v)).expect("infallible");
        data.encode(&mut enc);
    }
    buf
}

/// Encode MsgAcceptVersion for N2N: `[1, version, version_data]`.
fn encode_accept_version_n2n(version: u16, data: &N2NVersionData) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut enc = Encoder::new(&mut buf);
    enc.array(3).expect("infallible");
    enc.u64(MSG_ACCEPT_VERSION).expect("infallible");
    enc.u16(version).expect("infallible");
    data.encode(&mut enc);
    buf
}

/// Encode MsgAcceptVersion for N2C with bit-15 wire encoding.
fn encode_accept_version_n2c(version: u16, data: &N2CVersionData) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut enc = Encoder::new(&mut buf);
    enc.array(3).expect("infallible");
    enc.u64(MSG_ACCEPT_VERSION).expect("infallible");
    enc.u16(n2c::encode_n2c_version(version))
        .expect("infallible");
    data.encode(&mut enc);
    buf
}

/// Encode MsgRefuse: `[2, [0, [version, reason_text]]]`.
/// Tag 0 = VersionMismatch in the Haskell encoding.
fn encode_refuse(version: u16, reason: &str) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut enc = Encoder::new(&mut buf);
    enc.array(2).expect("infallible");
    enc.u64(MSG_REFUSE).expect("infallible");
    // RefuseReason: [0, [version, reason]] for VersionMismatch
    enc.array(2).expect("infallible");
    enc.u8(0).expect("infallible");
    enc.array(2).expect("infallible");
    enc.u16(version).expect("infallible");
    enc.str(reason).expect("infallible");
    buf
}

// ─── Decoding helpers ───

/// Decode MsgProposeVersions for N2N. Returns a map of version → version_data.
fn decode_propose_versions_n2n(
    data: &[u8],
) -> Result<BTreeMap<u16, N2NVersionData>, HandshakeError> {
    let mut dec = Decoder::new(data);
    let _arr_len = dec
        .array()
        .map_err(|e| HandshakeError::DecodeError(e.to_string()))?;
    let tag = dec
        .u64()
        .map_err(|e| HandshakeError::DecodeError(e.to_string()))?;
    if tag != MSG_PROPOSE_VERSIONS {
        return Err(HandshakeError::DecodeError(format!(
            "expected MsgProposeVersions (tag 0), got {tag}"
        )));
    }

    let map_len = dec
        .map()
        .map_err(|e| HandshakeError::DecodeError(e.to_string()))?
        .ok_or_else(|| HandshakeError::DecodeError("indefinite map not supported".to_string()))?;

    let mut versions = BTreeMap::new();
    for _ in 0..map_len {
        let version = dec
            .u16()
            .map_err(|e| HandshakeError::DecodeError(e.to_string()))?;
        let version_data = N2NVersionData::decode(&mut dec)
            .map_err(|e| HandshakeError::DecodeError(e.to_string()))?;
        versions.insert(version, version_data);
    }
    Ok(versions)
}

/// Decode MsgProposeVersions for N2C. Converts wire versions (bit-15) to logical.
fn decode_propose_versions_n2c(
    data: &[u8],
) -> Result<BTreeMap<u16, N2CVersionData>, HandshakeError> {
    let mut dec = Decoder::new(data);
    let _arr_len = dec
        .array()
        .map_err(|e| HandshakeError::DecodeError(e.to_string()))?;
    let tag = dec
        .u64()
        .map_err(|e| HandshakeError::DecodeError(e.to_string()))?;
    if tag != MSG_PROPOSE_VERSIONS {
        return Err(HandshakeError::DecodeError(format!(
            "expected MsgProposeVersions (tag 0), got {tag}"
        )));
    }

    let map_len = dec
        .map()
        .map_err(|e| HandshakeError::DecodeError(e.to_string()))?
        .ok_or_else(|| HandshakeError::DecodeError("indefinite map not supported".to_string()))?;

    let mut versions = BTreeMap::new();
    for _ in 0..map_len {
        let wire_version = dec
            .u16()
            .map_err(|e| HandshakeError::DecodeError(e.to_string()))?;
        let logical_version = n2c::decode_n2c_version(wire_version);
        let version_data = N2CVersionData::decode(&mut dec)
            .map_err(|e| HandshakeError::DecodeError(e.to_string()))?;
        versions.insert(logical_version, version_data);
    }
    Ok(versions)
}

/// Decode a handshake response (MsgAcceptVersion, MsgRefuse, or MsgProposeVersions for N2N).
fn decode_handshake_response(data: &[u8]) -> Result<HandshakeResult, HandshakeError> {
    let mut dec = Decoder::new(data);
    let _arr_len = dec
        .array()
        .map_err(|e| HandshakeError::DecodeError(e.to_string()))?;
    let tag = dec
        .u64()
        .map_err(|e| HandshakeError::DecodeError(e.to_string()))?;

    match tag {
        MSG_ACCEPT_VERSION => {
            let version = dec
                .u16()
                .map_err(|e| HandshakeError::DecodeError(e.to_string()))?;
            // Skip version data (we don't need it after acceptance)
            let _ = N2NVersionData::decode(&mut dec);
            Ok(HandshakeResult {
                version,
                simultaneous_open: false,
            })
        }
        MSG_REFUSE => {
            // Decode RefuseReason
            let _reason_arr = dec
                .array()
                .map_err(|e| HandshakeError::DecodeError(e.to_string()))?;
            let reason_tag = dec
                .u8()
                .map_err(|e| HandshakeError::DecodeError(e.to_string()))?;
            let reason = match reason_tag {
                0 => {
                    // VersionMismatch: [version, reason_text]
                    let _inner = dec.array();
                    let v = dec.u16().unwrap_or(0);
                    let r = dec.str().unwrap_or("unknown");
                    format!("version mismatch (v{v}): {r}")
                }
                1 => "handshake decode error".to_string(),
                2 => "refused".to_string(),
                _ => format!("unknown refuse reason tag {reason_tag}"),
            };
            Err(HandshakeError::Refused { version: 0, reason })
        }
        MSG_PROPOSE_VERSIONS => {
            // Simultaneous open — the remote also sent MsgProposeVersions.
            // We need to negotiate from their proposal.
            // For now, return a marker; the caller handles version selection.
            Ok(HandshakeResult {
                version: 0, // Caller must re-negotiate
                simultaneous_open: true,
            })
        }
        _ => Err(HandshakeError::DecodeError(format!(
            "unexpected handshake message tag: {tag}"
        ))),
    }
}

/// Decode a handshake response for N2C (with bit-15 version decoding).
fn decode_handshake_response_n2c(data: &[u8]) -> Result<HandshakeResult, HandshakeError> {
    let mut dec = Decoder::new(data);
    let _arr_len = dec
        .array()
        .map_err(|e| HandshakeError::DecodeError(e.to_string()))?;
    let tag = dec
        .u64()
        .map_err(|e| HandshakeError::DecodeError(e.to_string()))?;

    match tag {
        MSG_ACCEPT_VERSION => {
            let wire_version = dec
                .u16()
                .map_err(|e| HandshakeError::DecodeError(e.to_string()))?;
            let version = n2c::decode_n2c_version(wire_version);
            let _ = N2CVersionData::decode(&mut dec);
            Ok(HandshakeResult {
                version,
                simultaneous_open: false,
            })
        }
        MSG_REFUSE => {
            // Same refuse format as N2N
            decode_handshake_response(data)
        }
        _ => Err(HandshakeError::DecodeError(format!(
            "unexpected N2C handshake message tag: {tag}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn n2n_propose_encode_decode_roundtrip() {
        let data = N2NVersionData::new(2, true);
        let encoded = encode_propose_versions_n2n(n2n::N2N_VERSIONS, &data);
        let decoded = decode_propose_versions_n2n(&encoded).unwrap();
        assert!(decoded.contains_key(&14));
        assert!(decoded.contains_key(&15));
        assert_eq!(decoded[&14].network_magic, 2);
        assert!(decoded[&15].peer_sharing);
    }

    #[test]
    fn n2n_accept_encode_decode() {
        let data = N2NVersionData::new(2, true);
        let encoded = encode_accept_version_n2n(15, &data);
        let result = decode_handshake_response(&encoded).unwrap();
        assert_eq!(result.version, 15);
        assert!(!result.simultaneous_open);
    }

    #[test]
    fn n2n_refuse_decode() {
        let encoded = encode_refuse(15, "bad magic");
        let result = decode_handshake_response(&encoded);
        assert!(result.is_err());
        if let Err(HandshakeError::Refused { reason, .. }) = result {
            assert!(reason.contains("bad magic"));
        }
    }

    #[test]
    fn n2c_propose_encode_decode_roundtrip() {
        let data = N2CVersionData::new(2);
        let encoded = encode_propose_versions_n2c(n2c::N2C_VERSIONS, &data);
        let decoded = decode_propose_versions_n2c(&encoded).unwrap();
        // All 8 N2C versions should be present (as logical versions)
        for &v in n2c::N2C_VERSIONS {
            assert!(decoded.contains_key(&v), "missing version {v}");
        }
        assert_eq!(decoded[&16].network_magic, 2);
    }

    #[test]
    fn n2c_accept_encode_decode() {
        let data = N2CVersionData::new(2);
        let encoded = encode_accept_version_n2c(22, &data);
        let result = decode_handshake_response_n2c(&encoded).unwrap();
        assert_eq!(result.version, 22); // logical, not wire
    }

    #[test]
    fn n2c_bit15_wire_format() {
        // Verify the wire format contains bit-15 encoded versions
        let data = N2CVersionData::new(2);
        let encoded = encode_propose_versions_n2c(&[n2c::N2C_V16], &data);
        // The encoded bytes should contain 32784 (V16 | 0x8000) as a CBOR integer
        let mut dec = Decoder::new(&encoded);
        dec.array().unwrap(); // outer array
        dec.u64().unwrap(); // tag 0
        let map_len = dec.map().unwrap().unwrap();
        assert_eq!(map_len, 1);
        let wire_version = dec.u16().unwrap();
        assert_eq!(wire_version, 32784); // 16 | 0x8000
    }

    #[test]
    fn simultaneous_open_detection() {
        // If we receive MsgProposeVersions (tag 0) instead of MsgAcceptVersion,
        // it means simultaneous open.
        let data = N2NVersionData::new(2, true);
        let proposal = encode_propose_versions_n2n(n2n::N2N_VERSIONS, &data);
        let result = decode_handshake_response(&proposal).unwrap();
        assert!(result.simultaneous_open);
    }
}
