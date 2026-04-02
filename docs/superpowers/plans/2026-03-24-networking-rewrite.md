# Networking Layer Rewrite Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace all pallas-network dependencies in dugite-network with a custom Ouroboros networking implementation aligned to the Haskell cardano-node reference.

**Architecture:** Four-layer architecture (Bearer → Mux → Protocols → Connection Manager), each independently testable. The public trait API (`BlockProvider`, `TxValidator`, `MempoolProvider`, `UtxoQueryProvider`, `ConnectionMetrics`) is preserved so dugite-node adapter code remains stable. All CBOR encoding uses minicbor directly.

**Tech Stack:** Rust, tokio (async runtime), minicbor (CBOR), bytes, tokio-util (CancellationToken), hickory-resolver (DNS), socket2 (TCP options)

**Spec:** `docs/superpowers/specs/2026-03-24-networking-rewrite-design.md`

---

## File Structure

```
crates/dugite-network/
├── Cargo.toml                               # Remove pallas-network/traverse/crypto, add minicbor workspace
├── src/
│   ├── lib.rs                               # Re-exports, public trait definitions
│   ├── error.rs                             # NetworkError, BearerError, MuxError, HandshakeError, ProtocolError, ConnectionError
│   ├── codec.rs                             # CBOR helpers: encode_point, decode_point, encode_tip, decode_tip, etc.
│   ├── metrics.rs                           # Prometheus metric definitions
│   ├── bearer/
│   │   ├── mod.rs                           # Bearer trait definition
│   │   ├── tcp.rs                           # TcpBearer: wraps TcpStream, sdu_size=12288, batch_size=131072
│   │   └── unix.rs                          # UnixBearer: wraps UnixStream, sdu_size=32768
│   ├── mux/
│   │   ├── mod.rs                           # Mux struct: spawn(), subscribe(), shutdown()
│   │   ├── segment.rs                       # SDU header: encode_header(), decode_header(), Direction enum
│   │   ├── egress.rs                        # EgressTask: round-robin fairness, segmentation, batching
│   │   ├── ingress.rs                       # IngressTask: demux, direction flip, byte-limit enforcement
│   │   └── channel.rs                       # MuxChannel: send(), recv(), try_recv(), CBOR boundary detection
│   ├── handshake/
│   │   ├── mod.rs                           # run_handshake_client(), run_handshake_server(), HandshakeResult
│   │   ├── n2n.rs                           # N2N version table (V14-V15), encode/decode version data
│   │   └── n2c.rs                           # N2C version table (V16-V23), bit-15 encoding
│   ├── protocol/
│   │   ├── mod.rs                           # Agency enum, shared Point/Tip codec, protocol ID constants
│   │   ├── chainsync/
│   │   │   ├── mod.rs                       # ChainSyncState enum, message encode/decode
│   │   │   ├── client.rs                    # PipelinedChainSyncClient: intersection, low/high mark pipelining
│   │   │   └── server.rs                    # ChainSyncServer: per-peer cursor, header extraction, announcement wait
│   │   ├── blockfetch/
│   │   │   ├── mod.rs                       # BlockFetchState enum, message encode/decode
│   │   │   ├── client.rs                    # BlockFetchClient: range requests, batch streaming
│   │   │   └── server.rs                    # BlockFetchServer: range validation, block streaming
│   │   ├── txsubmission/
│   │   │   ├── mod.rs                       # TxSubmissionState enum, message encode/decode
│   │   │   ├── client.rs                    # TxSubmissionClient: announce txs, reply to requests
│   │   │   └── server.rs                    # TxSubmissionServer: request tx IDs, fetch txs, validate
│   │   ├── keepalive/
│   │   │   ├── mod.rs                       # KeepAlive message encode/decode
│   │   │   ├── client.rs                    # KeepAliveClient: periodic ping, RTT measurement
│   │   │   └── server.rs                    # KeepAliveServer: echo cookie
│   │   ├── peersharing/
│   │   │   ├── mod.rs                       # PeerSharing message encode/decode
│   │   │   ├── client.rs                    # PeerSharingClient: request peers
│   │   │   └── server.rs                    # PeerSharingServer: filter and return routable peers
│   │   ├── local_chainsync/
│   │   │   └── server.rs                    # LocalChainSyncServer: full blocks, HFC era wrapping
│   │   ├── local_tx_submission/
│   │   │   └── server.rs                    # LocalTxSubmissionServer: validate + mempool add
│   │   ├── local_state_query/
│   │   │   ├── mod.rs                       # Acquire/Release state machine, MsgAcquire target parsing
│   │   │   ├── server.rs                    # Query dispatch, HFC wrapping
│   │   │   └── encoding.rs                  # Query-specific CBOR (pparams, utxo, stake dist, governance)
│   │   └── local_tx_monitor/
│   │       └── server.rs                    # Snapshot-based mempool monitoring
│   ├── connection/
│   │   ├── mod.rs                           # ConnectionManager: accept_inbound(), connect_outbound()
│   │   ├── manager.rs                       # Connection lifecycle, dedup map, simultaneous open
│   │   ├── state.rs                         # ConnectionState enum, transitions
│   │   └── handler.rs                       # ConnectionHandler: start/stop protocol tasks by temperature
│   └── peer/
│       ├── mod.rs                           # Re-exports
│       ├── manager.rs                       # PeerManager: peer state, reputation, failure decay
│       ├── governor.rs                      # Governor: target-driven promotion/demotion
│       ├── discovery.rs                     # DNS, ledger-based, peer sharing discovery
│       └── selection.rs                     # Peer selection algorithms, address filtering
```

**Files modified in dugite-node (integration):**
- `crates/dugite-node/src/node/mod.rs` — update network construction to use ConnectionManager API
- `crates/dugite-node/src/node/sync.rs` — update sync client to use new PipelinedChainSyncClient
- `crates/dugite-node/src/node/serve.rs` — trait implementations preserved, minor signature adjustments

**Files modified in workspace root:**
- `Cargo.toml` — remove `pallas-network` from workspace dependencies

---

## Task 1: Scaffold and Dependencies

**Files:**
- Modify: `crates/dugite-network/Cargo.toml`
- Create: `crates/dugite-network/src/error.rs`
- Create: `crates/dugite-network/src/codec.rs`
- Modify: `crates/dugite-network/src/lib.rs`

- [ ] **Step 1: Create a feature branch**

```bash
git checkout -b feature/networking-rewrite
git push -u origin feature/networking-rewrite
```

- [ ] **Step 2: Update Cargo.toml — remove pallas, keep everything else**

In `crates/dugite-network/Cargo.toml`, remove these three dependencies:
```toml
# REMOVE:
pallas-network = { workspace = true }
pallas-crypto = { workspace = true }
pallas-traverse = { workspace = true }
```

Keep all other dependencies. Ensure `minicbor = { workspace = true }` is present (it should already be). Ensure `bytes = { workspace = true }` is present. Add if missing:
```toml
tokio-util = { workspace = true }
```

- [ ] **Step 3: Delete all existing source files except lib.rs**

```bash
# From crates/dugite-network/src/
# Delete everything except lib.rs
rm -rf bearer/ mux/ handshake/ protocol/ connection/ peer/
rm -f bandwidth.rs client.rs dns.rs duplex.rs governor.rs n2c_client.rs
rm -f n2n_server.rs peer_manager.rs peer.rs pipelined.rs server.rs tcp.rs
rm -rf miniprotocols/ multiplexer/ n2c/ query_handler/
```

- [ ] **Step 4: Write error.rs**

Create `crates/dugite-network/src/error.rs`:

```rust
use std::fmt;
use std::io;

/// Top-level network error.
#[derive(Debug)]
pub enum NetworkError {
    Bearer(BearerError),
    Mux(MuxError),
    Handshake(HandshakeError),
    Protocol(ProtocolError),
    Connection(ConnectionError),
}

impl fmt::Display for NetworkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bearer(e) => write!(f, "bearer: {e}"),
            Self::Mux(e) => write!(f, "mux: {e}"),
            Self::Handshake(e) => write!(f, "handshake: {e}"),
            Self::Protocol(e) => write!(f, "protocol: {e}"),
            Self::Connection(e) => write!(f, "connection: {e}"),
        }
    }
}

impl std::error::Error for NetworkError {}

impl From<BearerError> for NetworkError {
    fn from(e: BearerError) -> Self { Self::Bearer(e) }
}
impl From<MuxError> for NetworkError {
    fn from(e: MuxError) -> Self { Self::Mux(e) }
}
impl From<HandshakeError> for NetworkError {
    fn from(e: HandshakeError) -> Self { Self::Handshake(e) }
}
impl From<ProtocolError> for NetworkError {
    fn from(e: ProtocolError) -> Self { Self::Protocol(e) }
}
impl From<ConnectionError> for NetworkError {
    fn from(e: ConnectionError) -> Self { Self::Connection(e) }
}

/// Bearer (transport) errors.
#[derive(Debug)]
pub enum BearerError {
    Io(io::Error),
    ConnectionReset,
    Timeout,
}

impl fmt::Display for BearerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::ConnectionReset => write!(f, "connection reset"),
            Self::Timeout => write!(f, "timeout"),
        }
    }
}

impl std::error::Error for BearerError {}

impl From<io::Error> for BearerError {
    fn from(e: io::Error) -> Self {
        match e.kind() {
            io::ErrorKind::ConnectionReset | io::ErrorKind::BrokenPipe => Self::ConnectionReset,
            io::ErrorKind::TimedOut => Self::Timeout,
            _ => Self::Io(e),
        }
    }
}

/// Multiplexer errors.
#[derive(Debug)]
pub enum MuxError {
    InvalidHeader {
        protocol_id: u16,
        payload_len: u16,
    },
    IngressQueueOverrun {
        protocol_id: u16,
        bytes: usize,
        limit: usize,
    },
    UnknownProtocol(u16),
    BearerClosed,
    ChannelClosed,
    Bearer(BearerError),
}

impl fmt::Display for MuxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidHeader { protocol_id, payload_len } => {
                write!(f, "invalid SDU header: protocol={protocol_id}, len={payload_len}")
            }
            Self::IngressQueueOverrun { protocol_id, bytes, limit } => {
                write!(f, "ingress queue overrun: protocol={protocol_id}, {bytes}/{limit} bytes")
            }
            Self::UnknownProtocol(id) => write!(f, "unknown protocol ID: {id}"),
            Self::BearerClosed => write!(f, "bearer closed"),
            Self::ChannelClosed => write!(f, "channel closed"),
            Self::Bearer(e) => write!(f, "bearer: {e}"),
        }
    }
}

impl std::error::Error for MuxError {}

impl From<BearerError> for MuxError {
    fn from(e: BearerError) -> Self { Self::Bearer(e) }
}

/// Handshake errors.
#[derive(Debug)]
pub enum HandshakeError {
    NetworkMagicMismatch { ours: u64, theirs: u64 },
    VersionMismatch { ours: Vec<u16>, theirs: Vec<u16> },
    DecodeError(String),
    Refused { version: u16, reason: String },
    Timeout,
    Mux(MuxError),
}

impl fmt::Display for HandshakeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NetworkMagicMismatch { ours, theirs } => {
                write!(f, "network magic mismatch: ours={ours}, theirs={theirs}")
            }
            Self::VersionMismatch { ours, theirs } => {
                write!(f, "no common version: ours={ours:?}, theirs={theirs:?}")
            }
            Self::DecodeError(msg) => write!(f, "handshake decode error: {msg}"),
            Self::Refused { version, reason } => {
                write!(f, "handshake refused (v{version}): {reason}")
            }
            Self::Timeout => write!(f, "handshake timeout"),
            Self::Mux(e) => write!(f, "mux: {e}"),
        }
    }
}

impl std::error::Error for HandshakeError {}

impl From<MuxError> for HandshakeError {
    fn from(e: MuxError) -> Self { Self::Mux(e) }
}

/// Protocol-level errors (state machine violations, bad CBOR).
#[derive(Debug)]
pub enum ProtocolError {
    AgencyViolation {
        protocol: &'static str,
        state: String,
        received_tag: u8,
    },
    InvalidMessage {
        protocol: &'static str,
        tag: u8,
        reason: String,
    },
    CborDecode {
        protocol: &'static str,
        reason: String,
    },
    StateViolation {
        protocol: &'static str,
        expected: String,
        actual: String,
    },
    Mux(MuxError),
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AgencyViolation { protocol, state, received_tag } => {
                write!(f, "{protocol}: agency violation in state {state}, received tag {received_tag}")
            }
            Self::InvalidMessage { protocol, tag, reason } => {
                write!(f, "{protocol}: invalid message tag {tag}: {reason}")
            }
            Self::CborDecode { protocol, reason } => {
                write!(f, "{protocol}: CBOR decode error: {reason}")
            }
            Self::StateViolation { protocol, expected, actual } => {
                write!(f, "{protocol}: expected state {expected}, got {actual}")
            }
            Self::Mux(e) => write!(f, "mux: {e}"),
        }
    }
}

impl std::error::Error for ProtocolError {}

impl From<MuxError> for ProtocolError {
    fn from(e: MuxError) -> Self { Self::Mux(e) }
}

/// Connection manager errors.
#[derive(Debug)]
pub enum ConnectionError {
    MaxConnectionsReached,
    RateLimited(std::net::SocketAddr),
    SimultaneousOpenConflict,
    HandshakeFailed(HandshakeError),
    ConnectTimeout,
    ForbiddenConnection,
}

impl fmt::Display for ConnectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MaxConnectionsReached => write!(f, "max connections reached"),
            Self::RateLimited(addr) => write!(f, "rate limited: {addr}"),
            Self::SimultaneousOpenConflict => write!(f, "simultaneous open conflict"),
            Self::HandshakeFailed(e) => write!(f, "handshake failed: {e}"),
            Self::ConnectTimeout => write!(f, "connect timeout"),
            Self::ForbiddenConnection => write!(f, "forbidden connection (unidirectional reuse)"),
        }
    }
}

impl std::error::Error for ConnectionError {}

impl From<HandshakeError> for ConnectionError {
    fn from(e: HandshakeError) -> Self { Self::HandshakeFailed(e) }
}
```

- [ ] **Step 5: Write codec.rs — shared CBOR helpers**

Create `crates/dugite-network/src/codec.rs`:

```rust
//! Shared CBOR encoding/decoding helpers for Ouroboros wire format.
//!
//! All encoding matches the Haskell cardano-node EncCBOR/DecCBOR instances.

use bytes::{Bytes, BytesMut, BufMut};
use minicbor::{Encoder, Decoder, encode::Write};

/// A chain point: either the Origin (genesis) or a specific (slot, hash).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Point {
    Origin,
    Specific(u64, [u8; 32]),
}

/// Encode a Point as CBOR.
/// Origin = [] (empty array), Specific = [slot, hash_bytes]
pub fn encode_point(enc: &mut Encoder<&mut Vec<u8>>, point: &Point) -> Result<(), minicbor::encode::Error<std::io::Error>> {
    match point {
        Point::Origin => {
            enc.array(0)?;
        }
        Point::Specific(slot, hash) => {
            enc.array(2)?;
            enc.u64(*slot)?;
            enc.bytes(hash)?;
        }
    }
    Ok(())
}

/// Decode a Point from CBOR.
pub fn decode_point(dec: &mut Decoder<'_>) -> Result<Point, minicbor::decode::Error> {
    let len = dec.array()?;
    match len {
        Some(0) => Ok(Point::Origin),
        Some(2) => {
            let slot = dec.u64()?;
            let hash_bytes = dec.bytes()?;
            if hash_bytes.len() != 32 {
                return Err(minicbor::decode::Error::message("point hash must be 32 bytes"));
            }
            let mut hash = [0u8; 32];
            hash.copy_from_slice(hash_bytes);
            Ok(Point::Specific(slot, hash))
        }
        _ => Err(minicbor::decode::Error::message("invalid point array length")),
    }
}

/// Encode a Tip as CBOR: [[slot, hash], block_number] matching Haskell Tip encoding.
/// Outer array(2): first element is a Point [slot, hash], second is block_number.
pub fn encode_tip(enc: &mut Encoder<&mut Vec<u8>>, slot: u64, hash: &[u8; 32], block_number: u64) -> Result<(), minicbor::encode::Error<std::io::Error>> {
    enc.array(2)?;
    // Point
    enc.array(2)?;
    enc.u64(slot)?;
    enc.bytes(hash)?;
    // Block number
    enc.u64(block_number)?;
    Ok(())
}

/// Decode a Tip from CBOR. Returns (slot, hash, block_number).
pub fn decode_tip(dec: &mut Decoder<'_>) -> Result<(u64, [u8; 32], u64), minicbor::decode::Error> {
    dec.array()?; // outer array(2)
    // Point
    dec.array()?; // inner array(2)
    let slot = dec.u64()?;
    let hash_bytes = dec.bytes()?;
    if hash_bytes.len() != 32 {
        return Err(minicbor::decode::Error::message("tip hash must be 32 bytes"));
    }
    let mut hash = [0u8; 32];
    hash.copy_from_slice(hash_bytes);
    let block_number = dec.u64()?;
    Ok((slot, hash, block_number))
}

/// Try to find a complete CBOR value in a byte buffer.
/// Returns Some(consumed_bytes) if a complete value was found, None if incomplete.
/// This is used by MuxChannel for message boundary detection.
pub fn try_decode_cbor_boundary(data: &[u8]) -> Option<usize> {
    if data.is_empty() {
        return None;
    }
    let mut dec = Decoder::new(data);
    // Try to skip one complete CBOR value
    match skip_cbor_value(&mut dec) {
        Ok(()) => Some(dec.position()),
        Err(_) => None,
    }
}

/// Skip one complete CBOR data item (recursively for containers).
fn skip_cbor_value(dec: &mut Decoder<'_>) -> Result<(), minicbor::decode::Error> {
    use minicbor::data::Type;
    match dec.datatype()? {
        Type::U8 | Type::U16 | Type::U32 | Type::U64 => { dec.u64()?; }
        Type::I8 | Type::I16 | Type::I32 | Type::I64 => { dec.i64()?; }
        Type::Bool => { dec.bool()?; }
        Type::Null => { dec.null()?; }
        Type::Bytes => { dec.bytes()?; }
        Type::BytesIndef => { dec.bytes()?; }
        Type::String => { dec.str()?; }
        Type::StringIndef => { dec.str()?; }
        Type::Tag => {
            dec.tag()?;
            skip_cbor_value(dec)?;
        }
        Type::Array => {
            if let Some(len) = dec.array()? {
                for _ in 0..len {
                    skip_cbor_value(dec)?;
                }
            } else {
                // Indefinite-length array
                loop {
                    if dec.datatype()? == Type::Break {
                        dec.skip()?;
                        break;
                    }
                    skip_cbor_value(dec)?;
                }
            }
        }
        Type::Map => {
            if let Some(len) = dec.map()? {
                for _ in 0..len {
                    skip_cbor_value(dec)?; // key
                    skip_cbor_value(dec)?; // value
                }
            } else {
                // Indefinite-length map
                loop {
                    if dec.datatype()? == Type::Break {
                        dec.skip()?;
                        break;
                    }
                    skip_cbor_value(dec)?; // key
                    skip_cbor_value(dec)?; // value
                }
            }
        }
        Type::Simple => { dec.simple()?; }
        Type::Float16 | Type::Float32 | Type::Float64 => { dec.f64()?; }
        Type::Undefined => { dec.undefined()?; }
        t => return Err(minicbor::decode::Error::message(format!("unsupported CBOR type: {t:?}"))),
    }
    Ok(())
}
```

- [ ] **Step 6: Write minimal lib.rs stub**

Replace `crates/dugite-network/src/lib.rs` with:

```rust
//! Ouroboros network protocol implementation for the Dugite Cardano node.
//!
//! Four-layer architecture:
//! - Layer 1: Bearer (TCP, Unix socket transport)
//! - Layer 2: Multiplexer (SDU framing, fairness, demux)
//! - Layer 3: Mini-protocols (ChainSync, BlockFetch, TxSubmission2, etc.)
//! - Layer 4: Connection Manager (lifecycle, peer management)

pub mod error;
pub mod codec;
pub mod bearer;
pub mod mux;

// Placeholder modules — uncomment as implemented:
// pub mod handshake;
// pub mod protocol;
// pub mod connection;
// pub mod peer;
// pub mod metrics;

pub use error::*;

// Re-export MempoolProvider from primitives (used by TxSubmission2, LocalTxSubmission, LocalTxMonitor)
pub use dugite_primitives::mempool::MempoolProvider;

// ─── Public Traits ───
// These are the integration boundary with dugite-node.
// The node crate implements these traits and passes them to the network layer.

/// Provides block data from ChainDB for N2N server protocols.
pub trait BlockProvider: Send + Sync + 'static {
    /// Get raw block CBOR by hash.
    fn get_block(&self, hash: &[u8; 32]) -> Option<Vec<u8>>;
    /// Check if a block exists.
    fn has_block(&self, hash: &[u8; 32]) -> bool;
    /// Get current chain tip info.
    fn get_tip(&self) -> TipInfo;
    /// Get the next block after a given slot. Returns (slot, hash, cbor).
    fn get_next_block_after_slot(&self, after_slot: u64) -> Option<(u64, [u8; 32], Vec<u8>)>;
}

/// Chain tip information.
#[derive(Debug, Clone)]
pub struct TipInfo {
    pub slot: u64,
    pub hash: [u8; 32],
    pub block_number: u64,
}

/// Validates transactions before mempool admission.
pub trait TxValidator: Send + Sync + 'static {
    fn validate_tx(&self, era_id: u16, tx_bytes: &[u8]) -> Result<(), TxValidationError>;
}

/// Transaction validation errors returned to N2C clients.
#[derive(Debug, Clone)]
pub enum TxValidationError {
    DeserializationFailed { reason: String },
    InputNotFound { tx_hash: String, index: u32 },
    InsufficientFunds { required: u64, available: u64 },
    FeeTooSmall { minimum: u64, actual: u64 },
    ScriptFailed { reason: String },
    InvalidEra(u16),
    MempoolFull,
    LedgerStateUnavailable,
    Other(String),
}

/// Provides UTxO lookups for LocalStateQuery.
pub trait UtxoQueryProvider: Send + Sync {
    fn utxos_at_address_bytes(&self, addr_bytes: &[u8]) -> Vec<UtxoSnapshot>;
    fn utxos_by_tx_inputs(&self, _inputs: &[(Vec<u8>, u32)]) -> Vec<UtxoSnapshot> {
        vec![]
    }
}

/// UTxO snapshot for query responses.
#[derive(Debug, Clone)]
pub struct UtxoSnapshot {
    pub tx_hash: Vec<u8>,
    pub output_index: u32,
    pub address: Vec<u8>,
    pub value: u64,
    pub datum: Option<Vec<u8>>,
    pub script_ref: Option<Vec<u8>>,
    pub multi_assets: Vec<(Vec<u8>, Vec<(Vec<u8>, u64)>)>,
}

/// Metrics bridge for connection events.
pub trait ConnectionMetrics: Send + Sync + 'static {
    fn on_connect(&self);
    fn on_disconnect(&self);
    fn on_error(&self, label: &str);
}
```

- [ ] **Step 7: Write codec tests**

Add to the bottom of `codec.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_origin_roundtrip() {
        let mut buf = Vec::new();
        let mut enc = Encoder::new(&mut buf);
        encode_point(&mut enc, &Point::Origin).unwrap();
        let mut dec = Decoder::new(&buf);
        assert_eq!(decode_point(&mut dec).unwrap(), Point::Origin);
    }

    #[test]
    fn point_specific_roundtrip() {
        let hash = [0xAB; 32];
        let point = Point::Specific(42000, hash);
        let mut buf = Vec::new();
        let mut enc = Encoder::new(&mut buf);
        encode_point(&mut enc, &point).unwrap();
        let mut dec = Decoder::new(&buf);
        assert_eq!(decode_point(&mut dec).unwrap(), point);
    }

    #[test]
    fn tip_roundtrip() {
        let hash = [0xCD; 32];
        let mut buf = Vec::new();
        let mut enc = Encoder::new(&mut buf);
        encode_tip(&mut enc, 100, &hash, 50).unwrap();
        let mut dec = Decoder::new(&buf);
        let (slot, h, bn) = decode_tip(&mut dec).unwrap();
        assert_eq!(slot, 100);
        assert_eq!(h, hash);
        assert_eq!(bn, 50);
    }

    #[test]
    fn cbor_boundary_detection() {
        // A complete CBOR array [0] = 0x81 0x00
        let complete = vec![0x81, 0x00];
        assert_eq!(try_decode_cbor_boundary(&complete), Some(2));

        // Incomplete CBOR array (array of 2 with only 1 element)
        let incomplete = vec![0x82, 0x00];
        assert_eq!(try_decode_cbor_boundary(&incomplete), None);

        // Empty
        assert_eq!(try_decode_cbor_boundary(&[]), None);
    }
}
```

- [ ] **Step 8: Verify the crate compiles (it won't build fully yet, but syntax should be clean)**

Run: `cargo check -p dugite-network 2>&1 | head -20`
Expected: May have errors about missing modules — that's fine at this stage. The error.rs and codec.rs should be syntactically valid.

- [ ] **Step 8: Commit scaffold**

```bash
git add -A crates/dugite-network/
git commit -m "feat(network): scaffold networking rewrite — error types, codec, public traits

Remove pallas-network/traverse/crypto dependencies. Define the unified
error hierarchy (NetworkError, BearerError, MuxError, HandshakeError,
ProtocolError, ConnectionError) and shared CBOR codec helpers. Preserve
public trait API (BlockProvider, TxValidator, UtxoQueryProvider,
ConnectionMetrics) for dugite-node compatibility."
```

---

## Task 2: Bearer Layer

**Files:**
- Create: `crates/dugite-network/src/bearer/mod.rs`
- Create: `crates/dugite-network/src/bearer/tcp.rs`
- Create: `crates/dugite-network/src/bearer/unix.rs`
- Test: `crates/dugite-network/src/bearer/mod.rs` (inline tests)

- [ ] **Step 1: Write bearer trait and mock bearer**

Create `crates/dugite-network/src/bearer/mod.rs`:

```rust
//! Transport abstraction layer.
//!
//! The Bearer trait provides async read/write over TCP or Unix sockets.
//! A mock bearer is provided for testing with pre-recorded byte sequences.

pub mod tcp;
pub mod unix;

use crate::error::BearerError;

/// Abstract async transport. One bearer per connection.
#[async_trait::async_trait]
pub trait Bearer: Send + 'static {
    /// Read exactly `buf.len()` bytes.
    async fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), BearerError>;
    /// Write all bytes.
    async fn write_all(&mut self, buf: &[u8]) -> Result<(), BearerError>;
    /// Flush buffered data.
    async fn flush(&mut self) -> Result<(), BearerError>;
    /// Graceful shutdown.
    async fn close(&mut self) -> Result<(), BearerError>;
    /// Max SDU payload size for this bearer type.
    fn sdu_size(&self) -> usize;
    /// Max bytes per write batch.
    fn batch_size(&self) -> usize;
}

/// Mock bearer for testing. Feeds pre-recorded data and captures writes.
#[cfg(test)]
pub struct MockBearer {
    read_data: std::collections::VecDeque<u8>,
    write_data: Vec<u8>,
    sdu_size: usize,
    batch_size: usize,
}

#[cfg(test)]
impl MockBearer {
    pub fn new(read_data: Vec<u8>, sdu_size: usize, batch_size: usize) -> Self {
        Self {
            read_data: read_data.into(),
            write_data: Vec::new(),
            sdu_size,
            batch_size,
        }
    }

    pub fn written(&self) -> &[u8] {
        &self.write_data
    }
}

#[cfg(test)]
#[async_trait::async_trait]
impl Bearer for MockBearer {
    async fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), BearerError> {
        if self.read_data.len() < buf.len() {
            return Err(BearerError::ConnectionReset);
        }
        for byte in buf.iter_mut() {
            *byte = self.read_data.pop_front().unwrap();
        }
        Ok(())
    }

    async fn write_all(&mut self, buf: &[u8]) -> Result<(), BearerError> {
        self.write_data.extend_from_slice(buf);
        Ok(())
    }

    async fn flush(&mut self) -> Result<(), BearerError> {
        Ok(())
    }

    async fn close(&mut self) -> Result<(), BearerError> {
        Ok(())
    }

    fn sdu_size(&self) -> usize {
        self.sdu_size
    }

    fn batch_size(&self) -> usize {
        self.batch_size
    }
}
```

- [ ] **Step 2: Write TCP bearer**

Create `crates/dugite-network/src/bearer/tcp.rs`:

```rust
//! TCP bearer implementation.
//!
//! SDU payload size: 12,288 bytes (matching Haskell `makeSocketBearer`).
//! Batch size: 131,072 bytes.
//! TCP_NODELAY=false (Nagle enabled — mux egress batching handles coalescing).
//! SO_KEEPALIVE=true with 60s interval.

use tokio::net::TcpStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use socket2::{Socket, TcpKeepalive};
use std::time::Duration;

use crate::error::BearerError;
use super::Bearer;

/// TCP SDU payload size (bytes). Matches Haskell's `SDUSize 12_288`.
pub const TCP_SDU_SIZE: usize = 12_288;

/// TCP write batch size (bytes). Matches Haskell's batch of 131,072.
pub const TCP_BATCH_SIZE: usize = 131_072;

/// TCP read buffer size. Matches Haskell's readBufferSize.
pub const TCP_READ_BUFFER_SIZE: usize = 131_072;

/// TCP keepalive interval.
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(60);

pub struct TcpBearer {
    stream: TcpStream,
}

impl TcpBearer {
    /// Create a new TCP bearer from an existing stream.
    /// Configures TCP_NODELAY=false and SO_KEEPALIVE.
    pub fn new(stream: TcpStream) -> Result<Self, BearerError> {
        // Configure socket options via socket2
        let std_stream = stream.into_std().map_err(BearerError::Io)?;
        let socket = Socket::from(std_stream);

        // Nagle enabled (TCP_NODELAY=false) — mux batching handles coalescing
        socket.set_nodelay(false).map_err(|e| BearerError::Io(e.into()))?;

        // TCP keepalive
        let keepalive = TcpKeepalive::new().with_time(KEEPALIVE_INTERVAL);
        socket.set_tcp_keepalive(&keepalive).map_err(|e| BearerError::Io(e.into()))?;

        let std_stream: std::net::TcpStream = socket.into();
        std_stream.set_nonblocking(true).map_err(BearerError::Io)?;
        let stream = TcpStream::from_std(std_stream).map_err(BearerError::Io)?;

        Ok(Self { stream })
    }

    /// Connect to a remote address and return a configured bearer.
    pub async fn connect(addr: std::net::SocketAddr) -> Result<Self, BearerError> {
        let stream = TcpStream::connect(addr).await.map_err(BearerError::Io)?;
        Self::new(stream)
    }

    /// Get the underlying stream (for split operations if needed).
    pub fn into_stream(self) -> TcpStream {
        self.stream
    }
}

#[async_trait::async_trait]
impl Bearer for TcpBearer {
    async fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), BearerError> {
        self.stream.read_exact(buf).await.map_err(BearerError::from)
    }

    async fn write_all(&mut self, buf: &[u8]) -> Result<(), BearerError> {
        self.stream.write_all(buf).await.map_err(BearerError::from)
    }

    async fn flush(&mut self) -> Result<(), BearerError> {
        self.stream.flush().await.map_err(BearerError::from)
    }

    async fn close(&mut self) -> Result<(), BearerError> {
        self.stream.shutdown().await.map_err(BearerError::from)
    }

    fn sdu_size(&self) -> usize {
        TCP_SDU_SIZE
    }

    fn batch_size(&self) -> usize {
        TCP_BATCH_SIZE
    }
}
```

- [ ] **Step 3: Write Unix bearer**

Create `crates/dugite-network/src/bearer/unix.rs`:

```rust
//! Unix domain socket bearer for N2C connections.
//!
//! SDU payload size: 32,768 bytes (matching Haskell pipe bearer).

use tokio::net::UnixStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::error::BearerError;
use super::Bearer;

/// Unix socket SDU payload size (bytes).
pub const UNIX_SDU_SIZE: usize = 32_768;

/// Unix socket batch size (bytes).
pub const UNIX_BATCH_SIZE: usize = 32_768;

pub struct UnixBearer {
    stream: UnixStream,
}

impl UnixBearer {
    pub fn new(stream: UnixStream) -> Self {
        Self { stream }
    }

    pub fn into_stream(self) -> UnixStream {
        self.stream
    }
}

#[async_trait::async_trait]
impl Bearer for UnixBearer {
    async fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), BearerError> {
        self.stream.read_exact(buf).await.map_err(BearerError::from)
    }

    async fn write_all(&mut self, buf: &[u8]) -> Result<(), BearerError> {
        self.stream.write_all(buf).await.map_err(BearerError::from)
    }

    async fn flush(&mut self) -> Result<(), BearerError> {
        self.stream.flush().await.map_err(BearerError::from)
    }

    async fn close(&mut self) -> Result<(), BearerError> {
        self.stream.shutdown().await.map_err(BearerError::from)
    }

    fn sdu_size(&self) -> usize {
        UNIX_SDU_SIZE
    }

    fn batch_size(&self) -> usize {
        UNIX_BATCH_SIZE
    }
}
```

- [ ] **Step 4: Verify bearer layer compiles**

Run: `cargo check -p dugite-network`
Expected: Should compile (bearer module has no dependencies on deleted code).

- [ ] **Step 5: Commit bearer layer**

```bash
git add crates/dugite-network/src/bearer/
git commit -m "feat(network): add Bearer trait with TCP and Unix implementations

TCP bearer: SDU size 12,288 bytes (matching Haskell), TCP_NODELAY=false,
SO_KEEPALIVE=60s, 131KB read buffer. Unix bearer: SDU size 32,768 bytes.
Mock bearer provided for testing."
```

---

## Task 3: Multiplexer — SDU Segment Encoding

**Files:**
- Create: `crates/dugite-network/src/mux/mod.rs`
- Create: `crates/dugite-network/src/mux/segment.rs`
- Test: inline in `segment.rs`

- [ ] **Step 1: Write segment.rs with tests first**

Create `crates/dugite-network/src/mux/segment.rs`:

```rust
//! SDU (Segment Data Unit) header encoding/decoding.
//!
//! Wire format (8 bytes):
//! ```text
//! Bytes 0-3: transmission_time  u32 BE (microseconds, monotonic)
//! Bytes 4-5: protocol_and_dir   u16 BE (bit 15 = direction, bits 0-14 = protocol number)
//! Bytes 6-7: payload_length     u16 BE
//! ```
//!
//! Reference: ouroboros-network/network-mux/src/Network/Mux/Codec.hs

/// SDU header size in bytes.
pub const HEADER_SIZE: usize = 8;

/// Direction of a mux segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// Sent by the TCP connection initiator (bit 15 = 0).
    InitiatorDir,
    /// Sent by the TCP connection responder (bit 15 = 1).
    ResponderDir,
}

impl Direction {
    /// Flip direction (used on ingress — remote's InitiatorDir becomes our ResponderDir).
    pub fn flip(self) -> Self {
        match self {
            Self::InitiatorDir => Self::ResponderDir,
            Self::ResponderDir => Self::InitiatorDir,
        }
    }
}

/// Decoded SDU header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SduHeader {
    pub timestamp: u32,
    pub protocol_id: u16,
    pub direction: Direction,
    pub payload_length: u16,
}

/// Encode an SDU header into 8 bytes.
pub fn encode_header(header: &SduHeader) -> [u8; HEADER_SIZE] {
    let mut buf = [0u8; HEADER_SIZE];

    // Bytes 0-3: timestamp (u32 BE)
    buf[0..4].copy_from_slice(&header.timestamp.to_be_bytes());

    // Bytes 4-5: direction bit (15) | protocol number (0-14)
    let dir_bit: u16 = match header.direction {
        Direction::InitiatorDir => 0,
        Direction::ResponderDir => 0x8000,
    };
    let protocol_and_dir = dir_bit | (header.protocol_id & 0x7FFF);
    buf[4..6].copy_from_slice(&protocol_and_dir.to_be_bytes());

    // Bytes 6-7: payload length (u16 BE)
    buf[6..8].copy_from_slice(&header.payload_length.to_be_bytes());

    buf
}

/// Decode an SDU header from 8 bytes.
pub fn decode_header(buf: &[u8; HEADER_SIZE]) -> SduHeader {
    let timestamp = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
    let protocol_and_dir = u16::from_be_bytes([buf[4], buf[5]]);
    let payload_length = u16::from_be_bytes([buf[6], buf[7]]);

    let direction = if protocol_and_dir & 0x8000 != 0 {
        Direction::ResponderDir
    } else {
        Direction::InitiatorDir
    };
    let protocol_id = protocol_and_dir & 0x7FFF;

    SduHeader {
        timestamp,
        protocol_id,
        direction,
        payload_length,
    }
}

/// Get the current monotonic timestamp in microseconds (low 32 bits).
pub fn current_timestamp() -> u32 {
    use std::time::Instant;
    // Use a lazy-initialized epoch so timestamps are relative and fit in u32
    use std::sync::OnceLock;
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    let epoch = EPOCH.get_or_init(Instant::now);
    epoch.elapsed().as_micros() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_roundtrip_initiator() {
        let header = SduHeader {
            timestamp: 12345678,
            protocol_id: 2, // ChainSync
            direction: Direction::InitiatorDir,
            payload_length: 1024,
        };
        let encoded = encode_header(&header);
        let decoded = decode_header(&encoded);
        assert_eq!(header, decoded);
    }

    #[test]
    fn header_roundtrip_responder() {
        let header = SduHeader {
            timestamp: 0xDEADBEEF,
            protocol_id: 3, // BlockFetch
            direction: Direction::ResponderDir,
            payload_length: 12288,
        };
        let encoded = encode_header(&header);
        let decoded = decode_header(&encoded);
        assert_eq!(header, decoded);
    }

    #[test]
    fn direction_bit_encoding() {
        // InitiatorDir: bit 15 = 0, protocol 2 → 0x0002
        let header = SduHeader {
            timestamp: 0,
            protocol_id: 2,
            direction: Direction::InitiatorDir,
            payload_length: 0,
        };
        let buf = encode_header(&header);
        assert_eq!(buf[4..6], [0x00, 0x02]);

        // ResponderDir: bit 15 = 1, protocol 2 → 0x8002
        let header = SduHeader {
            timestamp: 0,
            protocol_id: 2,
            direction: Direction::ResponderDir,
            payload_length: 0,
        };
        let buf = encode_header(&header);
        assert_eq!(buf[4..6], [0x80, 0x02]);
    }

    #[test]
    fn direction_flip() {
        assert_eq!(Direction::InitiatorDir.flip(), Direction::ResponderDir);
        assert_eq!(Direction::ResponderDir.flip(), Direction::InitiatorDir);
    }

    #[test]
    fn max_protocol_id() {
        // Protocol ID uses bits 0-14, max = 0x7FFF = 32767
        let header = SduHeader {
            timestamp: 0,
            protocol_id: 0x7FFF,
            direction: Direction::InitiatorDir,
            payload_length: 0,
        };
        let decoded = decode_header(&encode_header(&header));
        assert_eq!(decoded.protocol_id, 0x7FFF);
    }

    #[test]
    fn max_payload_length() {
        let header = SduHeader {
            timestamp: 0,
            protocol_id: 0,
            direction: Direction::InitiatorDir,
            payload_length: u16::MAX,
        };
        let decoded = decode_header(&encode_header(&header));
        assert_eq!(decoded.payload_length, u16::MAX);
    }

    #[test]
    fn zero_payload_length() {
        let header = SduHeader {
            timestamp: 0,
            protocol_id: 8, // KeepAlive
            direction: Direction::ResponderDir,
            payload_length: 0,
        };
        let decoded = decode_header(&encode_header(&header));
        assert_eq!(decoded.payload_length, 0);
    }

    #[test]
    fn all_protocol_ids() {
        // Verify roundtrip for all known Ouroboros protocol IDs
        for &pid in &[0u16, 2, 3, 4, 5, 6, 7, 8, 9, 10] {
            for dir in [Direction::InitiatorDir, Direction::ResponderDir] {
                let header = SduHeader {
                    timestamp: 42,
                    protocol_id: pid,
                    direction: dir,
                    payload_length: 100,
                };
                let decoded = decode_header(&encode_header(&header));
                assert_eq!(decoded.protocol_id, pid, "protocol {pid} roundtrip failed");
                assert_eq!(decoded.direction, dir, "direction roundtrip failed for protocol {pid}");
            }
        }
    }
}
```

- [ ] **Step 2: Write mux/mod.rs stub**

Create `crates/dugite-network/src/mux/mod.rs`:

```rust
//! Ouroboros multiplexer.
//!
//! Multiplexes multiple mini-protocols over a single bearer (TCP or Unix socket).
//! Matches the Haskell network-mux architecture.

pub mod segment;

// These modules are added in Task 4:
// pub mod egress;
// pub mod ingress;
// pub mod channel;

pub use segment::{Direction, SduHeader, HEADER_SIZE};
```

- [ ] **Step 3: Run segment tests**

Run: `cargo test -p dugite-network -- segment::tests`
Expected: All tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/dugite-network/src/mux/
git commit -m "feat(network): add SDU segment encoding/decoding with tests

8-byte header: timestamp(u32) + direction_bit|protocol_id(u16) + payload_len(u16).
Direction bit: 0=InitiatorDir, 1=ResponderDir. Flipped on ingress.
All known Ouroboros protocol IDs verified in roundtrip tests."
```

---

## Task 4: Multiplexer — MuxChannel, Egress, and Ingress

**Files:**
- Create: `crates/dugite-network/src/mux/channel.rs`
- Create: `crates/dugite-network/src/mux/egress.rs`
- Create: `crates/dugite-network/src/mux/ingress.rs`
- Update: `crates/dugite-network/src/mux/mod.rs`

This is a large task. Each file implements one mux component. The test for the full mux lifecycle (egress+ingress+channel working together) is at the end.

- [ ] **Step 1: Write MuxChannel**

Create `crates/dugite-network/src/mux/channel.rs` — the per-protocol handle. The channel provides `send()` (writes complete CBOR messages to egress) and `recv()` (reads from ingress buffer, detects CBOR boundaries).

Key details:
- `send()` sends complete messages via `mpsc::Sender<Bytes>` to egress task (bounded capacity 32)
- `recv()` reads bytes from `mpsc::Receiver<Bytes>` and accumulates in an internal buffer
- Uses `codec::try_decode_cbor_boundary()` to detect when a complete CBOR message has arrived
- `try_recv()` is non-blocking variant for TxSubmission2
- **Buffer limit:** The internal reassembly buffer has a maximum size matching the protocol's ingress queue byte limit. If the buffer exceeds this limit without producing a complete CBOR message, return `MuxError::IngressQueueOverrun`. This prevents unbounded memory growth from malformed data.

- [ ] **Step 2: Write EgressTask**

Create `crates/dugite-network/src/mux/egress.rs` — the egress queue and writer.

Key details:
- Receives messages from all protocol channels via a shared `mpsc::Receiver<(u16, Direction, Bytes)>`
- Segments messages into `sdu_size` chunks with proper SDU headers
- Round-robin fairness: if a message exceeds one SDU, write one chunk and re-enqueue remainder
- Batches up to 100 SDUs or `batch_size` bytes per `write_all` call
- Writes to bearer and flushes

- [ ] **Step 3: Write IngressTask**

Create `crates/dugite-network/src/mux/ingress.rs` — the ingress demuxer.

Key details:
- Reads SDU headers (8 bytes) then payload from bearer
- Flips direction bit
- Dispatches payload to per-`(protocol_id, direction)` channel via `mpsc::Sender<Bytes>`
- Tracks byte count per channel; if exceeding limit, returns `IngressQueueOverrun`
- Silently discards data for protocol ID 1 (reserved)

- [ ] **Step 4: Update mux/mod.rs with Mux struct**

The `Mux` struct owns the bearer, spawns ingress/egress/control tasks, and provides `subscribe()` to get `MuxChannel` handles.

```rust
pub struct Mux {
    // ... internal state
}

impl Mux {
    pub fn new<B: Bearer>(bearer: B, is_initiator: bool) -> Self;
    pub fn subscribe(&mut self, protocol_id: u16, direction: Direction, ingress_limit: usize) -> MuxChannel;
    pub async fn run(self) -> Result<(), MuxError>;  // Spawns tasks, blocks until shutdown
}
```

- [ ] **Step 5: Write mux integration test**

Test the full mux lifecycle: create two mock bearers connected back-to-back, create a mux on each side, subscribe ChainSync channels, send a message from one side, verify it arrives on the other side with correct direction flipping.

- [ ] **Step 6: Run all mux tests**

Run: `cargo test -p dugite-network -- mux`
Expected: All pass.

- [ ] **Step 7: Commit**

```bash
git add crates/dugite-network/src/mux/
git commit -m "feat(network): implement Ouroboros multiplexer

MuxChannel with CBOR boundary detection, EgressTask with round-robin
fairness and batched writes, IngressTask with direction bit flipping
and per-protocol byte limits. SDU size 12,288 for TCP (matching Haskell).
Protocol ID 1 silently discarded (reserved)."
```

---

## Task 5: Handshake Protocol

**Files:**
- Create: `crates/dugite-network/src/handshake/mod.rs`
- Create: `crates/dugite-network/src/handshake/n2n.rs`
- Create: `crates/dugite-network/src/handshake/n2c.rs`

- [ ] **Step 1: Write N2N version data codec (n2n.rs)**

Encode/decode V14-V15 version data: `[network_magic, initiator_only, peer_sharing, query]`. Include acceptance logic (magic must match, initiator_only takes min, peer_sharing AND, query OR). Tests for all combinations.

- [ ] **Step 2: Write N2C version data codec (n2c.rs)**

Encode/decode V16-V23 version data: `[network_magic, query]`. Bit-15 version encoding (V16=32784, ..., V23=32791). Tests verifying wire integers.

- [ ] **Step 3: Write handshake state machine (mod.rs)**

`run_handshake_client()`: sends MsgProposeVersions, receives MsgAcceptVersion/MsgRefuse/MsgQueryReply. Detects simultaneous open (receives tag 0 instead of 1/2/3). Returns `HandshakeResult { version, version_data, simultaneous_open: bool }`.

`run_handshake_server()`: receives MsgProposeVersions, validates magic, selects highest common version, sends MsgAcceptVersion or MsgRefuse.

- [ ] **Step 4: Write handshake tests**

Test: version negotiation (propose V14+V15, accept V15), magic mismatch (refuse), query mode, N2C bit-15 encoding roundtrip, simultaneous open detection.

- [ ] **Step 5: Run tests and commit**

Run: `cargo test -p dugite-network -- handshake`

```bash
git add crates/dugite-network/src/handshake/
git commit -m "feat(network): implement Ouroboros handshake protocol

N2N V14-V15 and N2C V16-V23 version negotiation. Acceptance logic
matches Haskell (magic exact, initiator_only min, peer_sharing AND,
query OR). Simultaneous open detection. Query mode support."
```

---

## Task 6: Protocol Module Foundation + KeepAlive

**Files:**
- Create: `crates/dugite-network/src/protocol/mod.rs`
- Create: `crates/dugite-network/src/protocol/keepalive/mod.rs`
- Create: `crates/dugite-network/src/protocol/keepalive/client.rs`
- Create: `crates/dugite-network/src/protocol/keepalive/server.rs`

Start with KeepAlive because it's the simplest protocol — proves the pattern for all others.

- [ ] **Step 1: Write protocol/mod.rs with constants and shared types**

Protocol ID constants, protocol state trait pattern, Point/Tip re-exports from codec.

```rust
pub const PROTOCOL_N2N_HANDSHAKE: u16 = 0;
pub const PROTOCOL_N2N_CHAINSYNC: u16 = 2;
pub const PROTOCOL_N2N_BLOCKFETCH: u16 = 3;
pub const PROTOCOL_N2N_TXSUBMISSION: u16 = 4;
pub const PROTOCOL_N2C_CHAINSYNC: u16 = 5;
pub const PROTOCOL_N2C_TXSUBMISSION: u16 = 6;
pub const PROTOCOL_N2C_STATEQUERY: u16 = 7;
pub const PROTOCOL_N2N_KEEPALIVE: u16 = 8;
pub const PROTOCOL_N2C_TXMONITOR: u16 = 9;
pub const PROTOCOL_N2N_PEERSHARING: u16 = 10;
// Protocol ID 1 is reserved (unused).
```

- [ ] **Step 2: Write KeepAlive message codec (keepalive/mod.rs)**

Encode/decode: `MsgKeepAlive [0, cookie]`, `MsgKeepAliveResponse [1, cookie]`, `MsgDone [2]`. State enum: `StClient`, `StServer`, `StDone`. Tests for each message roundtrip.

- [ ] **Step 3: Write KeepAlive client (keepalive/client.rs)**

Periodic ping loop: send `MsgKeepAlive(cookie)`, receive `MsgKeepAliveResponse(cookie)`, verify cookie match, measure RTT. Configurable interval (default 30s). Runs until cancellation token fires, then sends `MsgDone`.

- [ ] **Step 4: Write KeepAlive server (keepalive/server.rs)**

Loop: receive `MsgKeepAlive(cookie)` or `MsgDone`, echo `MsgKeepAliveResponse(cookie)`. Exit on `MsgDone`.

- [ ] **Step 5: Run tests and commit**

Run: `cargo test -p dugite-network -- keepalive`

```bash
git add crates/dugite-network/src/protocol/
git commit -m "feat(network): add KeepAlive mini-protocol (client + server)

Cookie-based ping/pong with RTT measurement. Configurable interval
(default 30s). Graceful shutdown via MsgDone on CancellationToken."
```

---

## Task 7: ChainSync Protocol

**Files:**
- Create: `crates/dugite-network/src/protocol/chainsync/mod.rs`
- Create: `crates/dugite-network/src/protocol/chainsync/client.rs`
- Create: `crates/dugite-network/src/protocol/chainsync/server.rs`

The most complex and performance-critical protocol.

- [ ] **Step 1: Write ChainSync message codec (chainsync/mod.rs)**

State enum: `StIdle`, `StCanAwait`, `StMustReply`, `StIntersect`, `StDone`. Encode/decode all 8 message types (tags 0-7). Tests for each message roundtrip, including Origin point encoding.

- [ ] **Step 2: Write pipelined ChainSync client (chainsync/client.rs)**

`PipelinedChainSyncClient`:
- `find_intersection(points)` → sends MsgFindIntersect, receives MsgIntersectFound/NotFound
- `run_pipelined(callback)` → the main sync loop with low_mark=200, high_mark=300 pipelining
- EBB handling: detect Byron EBBs, track pending EBB hashes
- At-tip detection: switch to non-pipelined on MsgAwaitReply
- Cancellation via CancellationToken

This is the core sync engine. The callback receives decoded headers and the client manages the pipeline depth.

- [ ] **Step 3: Write ChainSync server (chainsync/server.rs)**

`ChainSyncServer`:
- Per-peer cursor tracking (slot + hash)
- `handle_find_intersect(points, block_provider)` → walk points, find best match
- `handle_request_next(block_provider, announcement_rx)` → serve next header or wait for announcement
- Header extraction from full block CBOR
- StMustReply: wait on broadcast channel with randomized timeout (135-911s)

- [ ] **Step 4: Write tests**

Test intersection with multiple points, pipelining state transitions (verify outstanding count), at-tip detection, rollback handling, header extraction.

- [ ] **Step 5: Run tests and commit**

Run: `cargo test -p dugite-network -- chainsync`

```bash
git add crates/dugite-network/src/protocol/chainsync/
git commit -m "feat(network): implement ChainSync mini-protocol

Pipelined client with low_mark=200/high_mark=300 matching Haskell.
Server with per-peer cursor, header extraction, broadcast-based
announcement wait with randomized timeout. EBB handling for Byron era."
```

---

## Task 8: BlockFetch Protocol

**Files:**
- Create: `crates/dugite-network/src/protocol/blockfetch/mod.rs`
- Create: `crates/dugite-network/src/protocol/blockfetch/client.rs`
- Create: `crates/dugite-network/src/protocol/blockfetch/server.rs`

- [ ] **Step 1: Write BlockFetch codec (blockfetch/mod.rs)**

State enum: `BFIdle`, `BFBusy`, `BFStreaming`, `BFDone`. Messages: MsgRequestRange [0], MsgClientDone [1], MsgStartBatch [2], MsgNoBlocks [3], MsgBlock [4], MsgBatchDone [5]. Roundtrip tests.

- [ ] **Step 2: Write BlockFetch client (blockfetch/client.rs)**

`BlockFetchClient`:
- `fetch_range(from_point, to_point)` → sends MsgRequestRange, receives batch
- Streams blocks via callback as they arrive (MsgBlock)
- Supports batch-level pipelining (multiple outstanding ranges)

- [ ] **Step 3: Write BlockFetch server (blockfetch/server.rs)**

`BlockFetchServer`:
- Validates range (max 2160 slots, max 100 blocks)
- Looks up from_hash via BlockProvider, iterates forward to to_hash
- Streams: MsgStartBatch → MsgBlock (per block) → MsgBatchDone
- MsgNoBlocks if range unavailable

- [ ] **Step 4: Run tests and commit**

Run: `cargo test -p dugite-network -- blockfetch`

```bash
git add crates/dugite-network/src/protocol/blockfetch/
git commit -m "feat(network): implement BlockFetch mini-protocol

Client with batch-level pipelining, streaming block delivery.
Server with range validation (max 2160 slots, 100 blocks),
sequential block streaming via BlockProvider."
```

---

## Task 9: TxSubmission2 Protocol

**Files:**
- Create: `crates/dugite-network/src/protocol/txsubmission/mod.rs`
- Create: `crates/dugite-network/src/protocol/txsubmission/client.rs`
- Create: `crates/dugite-network/src/protocol/txsubmission/server.rs`

- [ ] **Step 1: Write TxSubmission2 codec (txsubmission/mod.rs)**

State enum: `StInit`, `StIdle`, `StTxIds(Blocking)`, `StTxIds(NonBlocking)`, `StTxs`, `StDone`. Messages: MsgInit [6], MsgRequestTxIds [0], MsgReplyTxIds [1], MsgRequestTxs [2], MsgReplyTxs [3], MsgDone [4]. Agency: server has agency in StIdle (inverted). Roundtrip tests.

- [ ] **Step 2: Write TxSubmission2 client (txsubmission/client.rs)**

The client side (we announce our txs to remote):
- Wait for MsgInit acknowledgment
- Respond to MsgRequestTxIds with tx IDs from mempool
- Respond to MsgRequestTxs with full tx CBOR
- Send MsgDone when no more txs and in blocking state

- [ ] **Step 3: Write TxSubmission2 server (txsubmission/server.rs)**

The server side (we request txs from remote):
- Send MsgInit
- First MsgRequestTxIds: **blocking=false, ack_count=0** (critical correctness fix)
- Track FIFO of unacknowledged tx IDs
- Use `HashSet<[u8; 32]>` for inflight dedup
- Validate received txs via TxValidator, add to mempool
- `blocking=true` only when zero unacknowledged remain

- [ ] **Step 4: Test flow control rules**

Test: first request must be non-blocking, blocking only when unacked=0, MsgDone only in blocking state, FIFO acknowledgment semantics.

- [ ] **Step 5: Run tests and commit**

Run: `cargo test -p dugite-network -- txsubmission`

```bash
git add crates/dugite-network/src/protocol/txsubmission/
git commit -m "feat(network): implement TxSubmission2 mini-protocol

Pull-based protocol with correct inverted agency. First MsgRequestTxIds
is non-blocking (fixing critical bug from old implementation). HashSet
inflight dedup (O(1) vs old O(n^2)). FIFO acknowledgment tracking."
```

---

## Task 10: PeerSharing Protocol

**Files:**
- Create: `crates/dugite-network/src/protocol/peersharing/mod.rs`
- Create: `crates/dugite-network/src/protocol/peersharing/client.rs`
- Create: `crates/dugite-network/src/protocol/peersharing/server.rs`

- [ ] **Step 1: Write PeerSharing codec, client, server, and address filtering**

Codec: MsgShareRequest [0, amount], MsgSharePeers [1, [*addr]], MsgDone [2].
Address encoding: `[0, ipv4, port]` / `[1, w0, w1, w2, w3, port]`. No hostname variant.
Server filters non-routable addresses (RFC1918, CGNAT, ULA, loopback, etc.).
Client: periodic request from hot peers with peer_sharing enabled.

- [ ] **Step 2: Test address filtering**

Test all non-routable ranges are rejected: 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, 100.64.0.0/10, 127.0.0.0/8, fc00::/7, fe80::/10, ::1, 0.0.0.0.

- [ ] **Step 3: Run tests and commit**

Run: `cargo test -p dugite-network -- peersharing`

```bash
git add crates/dugite-network/src/protocol/peersharing/
git commit -m "feat(network): implement PeerSharing mini-protocol

IPv4/IPv6 address exchange. Address filtering rejects RFC1918, CGNAT
(100.64.0.0/10), IPv6 ULA (fc00::/7), loopback, link-local."
```

---

## Task 11a: N2C LocalChainSync + LocalTxSubmission

**Files:**
- Create: `crates/dugite-network/src/protocol/local_chainsync/server.rs`
- Create: `crates/dugite-network/src/protocol/local_tx_submission/server.rs`

- [ ] **Step 1: Write LocalChainSync server**

Same state machine as N2N ChainSync but sends full blocks wrapped as `[era_id, CBOR_tag_24(block_bytes)]`. Per-client cursor tracking. No pipelining needed. Ingress queue effectively unlimited (0xFFFFFFFF).

- [ ] **Step 2: Write LocalTxSubmission server**

MsgSubmitTx [0, era_id, tx_bytes] → validate via TxValidator → add to mempool via MempoolProvider → MsgAcceptTx [1] or MsgRejectTx [2, [era_id, reject_reason]]. Error encoding must match Haskell's `ApplyTxErr` format. Requires both `TxValidator` and `MempoolProvider` trait objects.

- [ ] **Step 3: Run tests and commit**

Run: `cargo test -p dugite-network -- local_chainsync local_tx_submission`

```bash
git add crates/dugite-network/src/protocol/local_chainsync/
git add crates/dugite-network/src/protocol/local_tx_submission/
git commit -m "feat(network): implement LocalChainSync and LocalTxSubmission

LocalChainSync sends full blocks with HFC era wrapping.
LocalTxSubmission validates via TxValidator, adds to mempool."
```

---

## Task 11b: N2C LocalTxMonitor

**Files:**
- Create: `crates/dugite-network/src/protocol/local_tx_monitor/server.rs`

- [ ] **Step 1: Write LocalTxMonitor server**

Snapshot-based mempool monitoring. Requires `MempoolProvider`.

Key implementation details:
- MsgAcquire [1] from StIdle: capture mempool snapshot (tx list, sizes), respond with MsgAcquired [2, slot_no]
- MsgNextTx [5]: yield next tx from snapshot that hasn't been yielded yet. Track yielded set per snapshot.
- MsgHasTx [7, tx_id]: check if tx_id is in the snapshot
- MsgGetSizes [9]: return [capacity, size, count] from snapshot
- MsgGetMeasures [11]: only available when negotiated N2C version >= V20 (wire 32788). Return [count, {*text => [size, capacity]}]
- MsgAwaitAcquire [1] from StAcquired: block until mempool snapshot differs from current, then auto-acquire the new snapshot
- MsgRelease [3]: release snapshot, return to StIdle
- MsgDone [0]: terminate

- [ ] **Step 2: Test snapshot semantics**

Test: acquire captures snapshot, subsequent queries see consistent state even if mempool changes, MsgAwaitAcquire blocks until new snapshot, MsgGetMeasures rejected if version < V20.

- [ ] **Step 3: Run tests and commit**

Run: `cargo test -p dugite-network -- local_tx_monitor`

```bash
git add crates/dugite-network/src/protocol/local_tx_monitor/
git commit -m "feat(network): implement LocalTxMonitor

Snapshot-based mempool monitoring. MsgGetMeasures gated on V20+.
MsgAwaitAcquire blocks until new snapshot available."
```

---

## Task 11c: N2C LocalStateQuery

**Files:**
- Create: `crates/dugite-network/src/protocol/local_state_query/mod.rs`
- Create: `crates/dugite-network/src/protocol/local_state_query/server.rs`
- Create: `crates/dugite-network/src/protocol/local_state_query/encoding.rs`

This is the most complex N2C protocol. Much of the query encoding logic can be ported from the existing `n2c/query/` code (which was already custom, not pallas-dependent).

- [ ] **Step 1: Write acquire/release state machine (mod.rs)**

State enum: StIdle, StAcquiring, StAcquired, StQuerying, StDone.

Acquire target parsing:
- `[0, point]` → SpecificPoint: validate between immutable and volatile tip
- `[8]` → VolatileTip: always succeeds
- `[10]` → ImmutableTip: always succeeds (V16+)
- Failure: `[2, 0]` = PointTooOld, `[2, 1]` = PointNotOnChain

MsgReAcquire ([6]/[9]/[11]): atomic release + acquire. If new acquisition fails, old state is also lost.

- [ ] **Step 2: Write query dispatch (server.rs)**

Route all 39 Shelley BlockQuery tags (0-38) to encoding functions. Results wrapped in HFC `QueryIfCurrent` success envelope `[1, result]` for BlockQuery results. QueryAnytime and QueryHardFork results unwrapped.

Snapshot under lock: acquire read lock, clone needed data, release lock, compute response on the clone.

- [ ] **Step 3: Write query-specific CBOR encoding (encoding.rs)**

Port encoding logic from existing files:
- `n2c/query/encoding.rs` → protocol params, UTxO, stake distribution
- `n2c/query/governance.rs` → governance queries (constitution, committee, proposals, DReps)
- `n2c/query/ledger.rs` → epoch state, reward provenance, pool info
- `n2c/query/protocol.rs` → era history, genesis config, system start
- `n2c/query/debug.rs` → debug state summaries (tags 8, 12, 13)

These files don't depend on pallas-network and can be largely ported as-is.

- [ ] **Step 4: Test acquire semantics and query responses**

Test: VolatileTip always acquires, SpecificPoint fails with correct error, MsgReAcquire releases old state, query response wrapping.

- [ ] **Step 5: Run tests and commit**

Run: `cargo test -p dugite-network -- local_state_query`

```bash
git add crates/dugite-network/src/protocol/local_state_query/
git commit -m "feat(network): implement LocalStateQuery

39 BlockQuery tags (0-38) with HFC wrapping. Proper acquire/release
with point validation (PointTooOld, PointNotOnChain). MsgReAcquire
atomic release+acquire. Snapshot-under-lock for lock-free query."
```

---

## Task 12: Peer Manager & Governor

**Files:**
- Create: `crates/dugite-network/src/peer/mod.rs`
- Create: `crates/dugite-network/src/peer/manager.rs`
- Create: `crates/dugite-network/src/peer/governor.rs`
- Create: `crates/dugite-network/src/peer/discovery.rs`
- Create: `crates/dugite-network/src/peer/selection.rs`

- [ ] **Step 1: Write PeerManager (peer/manager.rs)**

PeerInfo struct with cold/warm/hot state, EWMA latency, reputation, failure count with decay timer. Methods: add_peer, remove_peer, promote, demote, record_failure, record_success, update_latency. Background task halves failure_count every 5 minutes.

- [ ] **Step 2: Write Governor (peer/governor.rs)**

PeerTargets struct matching Haskell. Decision loop: compare current state vs targets, emit promotion/demotion/discovery actions. Churn rotation every 10-20 minutes.

- [ ] **Step 3: Write Discovery (peer/discovery.rs)**

DNS resolution (SRV + A/AAAA via hickory-resolver), ledger-based peer discovery (SPO relays from pool_params), peer sharing integration. Big ledger peer tracking.

- [ ] **Step 4: Write Selection (peer/selection.rs)**

Address filtering (`is_routable`): reject all non-routable ranges. Peer selection score: combine latency, reputation, failure penalty. Eviction: lowest-score cold peer when table full.

- [ ] **Step 5: Run tests and commit**

Run: `cargo test -p dugite-network -- peer`

```bash
git add crates/dugite-network/src/peer/
git commit -m "feat(network): implement Peer Manager and Governor

Cold/warm/hot peer lifecycle, EWMA latency, reputation scoring with
5-minute failure decay. Target-driven governor with churn rotation.
DNS, ledger-based, and peer sharing discovery. Address filtering
includes CGNAT and IPv6 ULA rejection."
```

---

## Task 13: Block Fetch Decision Logic

**Files:**
- Create: `crates/dugite-network/src/protocol/blockfetch/decision.rs`
- Modify: `crates/dugite-network/src/protocol/blockfetch/mod.rs`

The block fetch decision logic sits between ChainSync (which receives headers) and BlockFetch (which downloads blocks). It decides which peer to fetch each block range from.

- [ ] **Step 1: Write block fetch decision engine**

Create `crates/dugite-network/src/protocol/blockfetch/decision.rs`:

Key responsibilities:
- Maintain a download queue of block ranges needed (populated from ChainSync headers)
- Select the peer with lowest latency that has advertised the block (via ChainSync tip)
- Distribute ranges across multiple peers for parallel fetching
- Track in-flight ranges per peer, respect `blockFetchPipeliningMax` (default 100)
- On fetch failure: retry the range on an alternative peer
- On rollback: remove invalidated ranges from the queue

Public API:
```rust
pub struct BlockFetchDecision { ... }

impl BlockFetchDecision {
    pub fn new(max_in_flight: usize) -> Self;
    pub fn add_range(&mut self, from: Point, to: Point);
    pub fn select_peer(&self, peers: &[PeerFetchState]) -> Option<(SocketAddr, Point, Point)>;
    pub fn mark_completed(&mut self, peer: SocketAddr, range: (Point, Point));
    pub fn mark_failed(&mut self, peer: SocketAddr, range: (Point, Point));
    pub fn rollback_to(&mut self, point: &Point);
}
```

- [ ] **Step 2: Test peer selection and retry**

Test: ranges distributed across peers by latency, failed range retried on different peer, rollback removes invalidated ranges, in-flight limit respected.

- [ ] **Step 3: Run tests and commit**

Run: `cargo test -p dugite-network -- blockfetch::decision`

```bash
git add crates/dugite-network/src/protocol/blockfetch/decision.rs
git add crates/dugite-network/src/protocol/blockfetch/mod.rs
git commit -m "feat(network): add block fetch decision logic

Latency-based peer selection, parallel range distribution, retry on
failure, rollback handling. Respects blockFetchPipeliningMax (100)."
```

---

## Task 14: Connection Manager

**Files:**
- Create: `crates/dugite-network/src/connection/mod.rs`
- Create: `crates/dugite-network/src/connection/manager.rs`
- Create: `crates/dugite-network/src/connection/state.rs`
- Create: `crates/dugite-network/src/connection/handler.rs`

- [ ] **Step 1: Write ConnectionState (connection/state.rs)**

State enum: ReservedOutbound, UnnegotiatedConn(Provenance), OutboundIdle(DataFlow), InboundIdle(DataFlow), DuplexConn, Closed. Transition validation methods.

- [ ] **Step 2: Write ConnectionHandler (connection/handler.rs)**

Manages protocol tasks for a single connection. Methods: start_warm_protocols (KeepAlive), start_hot_protocols (ChainSync + BlockFetch + TxSubmission2 + PeerSharing), stop_hot_protocols (send MsgDone), stop_all (shutdown). Uses CancellationToken for graceful shutdown with 5s timeout.

- [ ] **Step 3: Write ConnectionManager (connection/manager.rs)**

Core lifecycle manager:
- `accept_inbound(stream)` → rate limit, start mux, handshake, validate magic
- `connect_outbound(addr)` → reserve slot, TCP connect with timeout, start mux, handshake
- Concurrent connection dedup via `HashMap<SocketAddr, ConnectionState>` under Mutex
- Simultaneous open: detect existing inbound, wait for its handshake, reuse if duplex
- Connection limits: max_inbound=100, max_outbound=20, per_ip_rate=5/min
- Exposes N2N listener (TCP) and N2C listener (Unix socket) as spawnable tasks

- [ ] **Step 4: Write connection/mod.rs**

Public API: `ConnectionManager::new(config, block_provider, tx_validator, mempool, utxo_provider, metrics)`. Re-exports.

- [ ] **Step 5: Run tests and commit**

Run: `cargo test -p dugite-network -- connection`

```bash
git add crates/dugite-network/src/connection/
git commit -m "feat(network): implement Connection Manager

Full connection lifecycle with simultaneous open handling (reuse inbound
connection matching Haskell algorithm). Rate limiting, connection limits,
warm/hot protocol orchestration with CancellationToken shutdown."
```

---

## Task 15: Wire Up lib.rs and Metrics

**Files:**
- Update: `crates/dugite-network/src/lib.rs`
- Create: `crates/dugite-network/src/metrics.rs`

- [ ] **Step 1: Update lib.rs to export all modules**

Uncomment all module declarations. Re-export key public types: ConnectionManager, PeerManager, Governor, PipelinedChainSyncClient, MuxChannel, etc.

- [ ] **Step 2: Write metrics.rs**

Prometheus metric definitions for connection, protocol, mux, and latency metrics. Use the existing pattern from dugite-node's metrics.

- [ ] **Step 3: Verify the full crate compiles**

Run: `cargo check -p dugite-network`
Expected: Clean compilation with zero warnings.

- [ ] **Step 4: Run all tests**

Run: `cargo test -p dugite-network`
Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/dugite-network/src/lib.rs crates/dugite-network/src/metrics.rs
git commit -m "feat(network): wire up all modules, add Prometheus metrics

Export complete public API. Prometheus metrics for connections, protocols,
mux, and latency on port 12798."
```

---

## Task 16: Integrate with dugite-node

**Files:**
- Modify: `crates/dugite-node/src/node/mod.rs`
- Modify: `crates/dugite-node/src/node/sync.rs`
- Modify: `crates/dugite-node/src/node/serve.rs`
- Modify: `Cargo.toml` (workspace root)

- [ ] **Step 1: Update serve.rs trait implementations**

The trait signatures are preserved, so the adapter implementations (ChainDBBlockProvider, LedgerTxValidator, LedgerUtxoProvider, ConnectionMetrics) should largely remain the same. Adjust any import paths that changed.

- [ ] **Step 2: Update node/mod.rs — network construction**

Replace the old `N2NServer::with_config()` and `N2CServer::new()` construction with the new `ConnectionManager::new(config, providers)` API. Wire up block announcement broadcast channels.

- [ ] **Step 3: Update node/sync.rs — sync client**

Replace `PipelinedPeerClient` usage with the new `PipelinedChainSyncClient`. The new client is constructed from a `MuxChannel` obtained via the ConnectionManager. Update the `chain_sync_loop` to use the new client API.

- [ ] **Step 4: Remove pallas-network from workspace Cargo.toml**

Remove `pallas-network` from `[workspace.dependencies]` if no other crate uses it (confirmed: only dugite-network used it).

- [ ] **Step 5: Build the full workspace**

Run: `cargo build --all-targets`
Expected: Clean build with zero warnings.

- [ ] **Step 6: Run all workspace tests**

Run: `cargo test --all`
Expected: All tests pass.

- [ ] **Step 7: Run clippy**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: Clean.

- [ ] **Step 8: Run fmt check**

Run: `cargo fmt --all -- --check`
Expected: Clean.

- [ ] **Step 9: Commit**

```bash
git add crates/dugite-network/ crates/dugite-node/ Cargo.toml Cargo.lock
git commit -m "feat(network): integrate new networking layer with dugite-node

Update node construction to use ConnectionManager API. Adapt sync loop
to new PipelinedChainSyncClient. Remove pallas-network workspace dep.
All tests pass, clippy clean, fmt clean."
```

---

## Task 17: Conformance Test Capture and Validation

**Files:**
- Create: `crates/dugite-network/tests/conformance/`

- [ ] **Step 1: Capture wire traces from cardano-node**

Run cardano-node on preview testnet with tcpdump capturing port 3001. Extract per-protocol message sequences using a dissector script. Save as test fixtures.

- [ ] **Step 2: Write conformance test harness**

Test framework that loads `.cbor` trace files, replays them through our encoder/decoder, and verifies byte-for-byte match for outbound messages and successful decode for inbound messages.

- [ ] **Step 3: Run conformance tests**

Run: `cargo test -p dugite-network -- conformance`
Expected: All pass.

- [ ] **Step 4: Commit**

```bash
git add crates/dugite-network/tests/
git commit -m "test(network): add protocol conformance test suite

Wire trace replay against captured cardano-node sessions. Verifies
byte-for-byte encoding correctness for all mini-protocols."
```

---

## Task 18: Live Testnet Validation

- [ ] **Step 1: Build release binary**

Run: `cargo build --release`

- [ ] **Step 2: Run on preview testnet**

```bash
./target/release/dugite-node run \
  --config config/preview-config.json \
  --topology config/preview-topology.json \
  --database-path ./db-preview \
  --socket-path ./node.sock \
  --host-addr 0.0.0.0 --port 3001
```

Verify: handshake completes, ChainSync starts, blocks are fetched, sync progresses.

- [ ] **Step 3: Test N2C via cardano-cli**

```bash
cardano-cli query tip --socket-path ./node.sock --testnet-magic 2
cardano-cli query protocol-parameters --socket-path ./node.sock --testnet-magic 2
```

Verify: responses match expected format.

- [ ] **Step 4: Verify Prometheus metrics**

```bash
curl -s http://localhost:12798/metrics | grep dugite_peers
curl -s http://localhost:12798/metrics | grep dugite_chainsync
```

Verify: metrics are being reported.

- [ ] **Step 5: Commit any fixes and create PR**

```bash
git push origin feature/networking-rewrite
```

Create PR against main with full description of the rewrite.

---

## Dependency Graph

```
Task 1 (Scaffold) → Task 2 (Bearer) → Task 3 (Segment) → Task 4 (Mux)
                                                              │
                                          ┌───────────────────┼───────────────────┐
                                          │                   │                   │
                                     Task 5 (Handshake)  Task 6 (KeepAlive)      ...
                                          │                   │
                    ┌─────────────────────┼───────────────────┼──────────────┐
                    │                     │                   │              │
               Task 7 (ChainSync)   Task 8 (BlockFetch)  Task 9 (TxSub2) Task 10 (PeerSharing)
                    │                     │                   │              │
               Task 11a (N2C basic) Task 11b (TxMon)    Task 11c (LSQ)  Task 12 (Peers)
                    │                     │                   │              │
                    │                Task 13 (BF Decision)    │              │
                    │                     │                   │              │
                    └─────────────────────┼───────────────────┼──────────────┘
                                          │                   │
                                     Task 14 (Conn Mgr)──────┘
                                          │
                                     Task 15 (lib.rs + Metrics)
                                          │
                                     Task 16 (Node Integration)
                                          │
                                     Task 17 (Conformance Tests)
                                          │
                                     Task 18 (Live Testnet)
```

**Parallelism:** Tasks 5-12 can be executed in **parallel** once Tasks 1-4 are complete (they all depend on the Mux layer but not on each other). Tasks 11a/11b/11c can also run in parallel. Task 13 depends on Tasks 7+8. Task 14 depends on all protocol implementations. Tasks 15-18 are sequential.
