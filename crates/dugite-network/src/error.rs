//! Network error types for the Ouroboros protocol stack.
//!
//! Hierarchical error types mapping to each layer:
//! - [`BearerError`] — transport layer (TCP/Unix I/O)
//! - [`MuxError`] — multiplexer layer (SDU framing, queues)
//! - [`HandshakeError`] — version negotiation failures
//! - [`ProtocolError`] — mini-protocol state machine violations
//! - [`ConnectionError`] — connection lifecycle failures
//! - [`NetworkError`] — top-level wrapper

use std::fmt;
use std::io;

/// Top-level network error that wraps all layer-specific errors.
///
/// Each variant corresponds to one layer in the Ouroboros protocol stack.
/// Conversions via `From` allow `?` propagation from any layer.
#[derive(Debug)]
pub enum NetworkError {
    /// Transport-level error (TCP/Unix socket I/O).
    Bearer(BearerError),
    /// Multiplexer-level error (SDU framing, ingress queues).
    Mux(MuxError),
    /// Handshake negotiation failure.
    Handshake(HandshakeError),
    /// Mini-protocol state machine violation.
    Protocol(ProtocolError),
    /// Connection manager error (lifecycle, rate limiting).
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
    fn from(e: BearerError) -> Self {
        Self::Bearer(e)
    }
}
impl From<MuxError> for NetworkError {
    fn from(e: MuxError) -> Self {
        Self::Mux(e)
    }
}
impl From<HandshakeError> for NetworkError {
    fn from(e: HandshakeError) -> Self {
        Self::Handshake(e)
    }
}
impl From<ProtocolError> for NetworkError {
    fn from(e: ProtocolError) -> Self {
        Self::Protocol(e)
    }
}
impl From<ConnectionError> for NetworkError {
    fn from(e: ConnectionError) -> Self {
        Self::Connection(e)
    }
}

/// Bearer (transport) errors from TCP or Unix socket I/O.
///
/// Automatically classifies `io::Error` variants into semantic categories
/// (connection reset, timeout) via the `From<io::Error>` impl.
#[derive(Debug)]
pub enum BearerError {
    /// Generic I/O error that doesn't map to a specific category.
    Io(io::Error),
    /// Connection was reset by the remote peer (or pipe was broken).
    ConnectionReset,
    /// I/O operation timed out.
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

/// Multiplexer errors related to SDU framing, ingress queues, and bearer lifecycle.
#[derive(Debug)]
pub enum MuxError {
    /// Invalid SDU header received (bad protocol ID or payload length).
    InvalidHeader {
        /// The protocol ID from the SDU header.
        protocol_id: u16,
        /// The payload length from the SDU header.
        payload_len: u16,
    },
    /// Protocol ingress queue exceeded its byte limit.
    IngressQueueOverrun {
        /// The protocol whose queue overflowed.
        protocol_id: u16,
        /// Current queue size in bytes.
        bytes: usize,
        /// Maximum allowed queue size in bytes.
        limit: usize,
    },
    /// Received data for an unknown or unsubscribed protocol.
    UnknownProtocol(u16),
    /// The underlying bearer was closed (EOF or explicit close).
    BearerClosed,
    /// An internal tokio channel was closed unexpectedly.
    ChannelClosed,
    /// Bearer-level error propagated through the mux layer.
    Bearer(BearerError),
    /// No SDU data received within the per-SDU read deadline (30s).
    ///
    /// Matches Haskell's `sduTimeout` (30 seconds). When the remote peer
    /// stops sending data mid-connection (e.g. silent TCP failure or stalled
    /// peer), this timeout fires and the mux tears down the bearer.
    SduReadTimeout,
}

impl fmt::Display for MuxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidHeader {
                protocol_id,
                payload_len,
            } => {
                write!(
                    f,
                    "invalid SDU header: protocol={protocol_id}, len={payload_len}"
                )
            }
            Self::IngressQueueOverrun {
                protocol_id,
                bytes,
                limit,
            } => {
                write!(
                    f,
                    "ingress queue overrun: protocol={protocol_id}, {bytes}/{limit} bytes"
                )
            }
            Self::UnknownProtocol(id) => write!(f, "unknown protocol ID: {id}"),
            Self::BearerClosed => write!(f, "bearer closed"),
            Self::ChannelClosed => write!(f, "channel closed"),
            Self::Bearer(e) => write!(f, "bearer: {e}"),
            Self::SduReadTimeout => write!(f, "SDU read timeout (30s)"),
        }
    }
}

impl std::error::Error for MuxError {}

impl From<BearerError> for MuxError {
    fn from(e: BearerError) -> Self {
        Self::Bearer(e)
    }
}

/// Handshake protocol errors during version negotiation.
#[derive(Debug)]
pub enum HandshakeError {
    /// Network magic values do not match between peers.
    NetworkMagicMismatch {
        /// Our configured network magic.
        ours: u64,
        /// The remote peer's network magic.
        theirs: u64,
    },
    /// No common protocol version found between peers.
    VersionMismatch {
        /// Protocol versions we support.
        ours: Vec<u16>,
        /// Protocol versions the remote peer supports.
        theirs: Vec<u16>,
    },
    /// Failed to decode a handshake CBOR message.
    DecodeError(String),
    /// Remote peer explicitly refused the handshake.
    Refused {
        /// The protocol version that was refused.
        version: u16,
        /// Human-readable reason for refusal.
        reason: String,
    },
    /// Handshake timed out waiting for remote response.
    Timeout,
    /// Multiplexer error during handshake exchange.
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
    fn from(e: MuxError) -> Self {
        Self::Mux(e)
    }
}

/// Protocol-level errors for mini-protocol state machine violations and CBOR issues.
#[derive(Debug)]
pub enum ProtocolError {
    /// Received a message when it's not the remote peer's turn (agency violation).
    /// This indicates a protocol bug on one side of the connection.
    AgencyViolation {
        /// Name of the mini-protocol (e.g. "ChainSync", "BlockFetch").
        protocol: &'static str,
        /// Current state machine state as a human-readable string.
        state: String,
        /// The CBOR tag of the received message.
        received_tag: u8,
    },
    /// Received a message with an invalid or unexpected CBOR tag.
    InvalidMessage {
        /// Name of the mini-protocol.
        protocol: &'static str,
        /// The CBOR tag of the invalid message.
        tag: u8,
        /// Human-readable explanation of why it's invalid.
        reason: String,
    },
    /// Failed to decode CBOR payload within a protocol message.
    CborDecode {
        /// Name of the mini-protocol where decoding failed.
        protocol: &'static str,
        /// Human-readable description of the decode error.
        reason: String,
    },
    /// Protocol state machine is in an unexpected state for the operation.
    StateViolation {
        /// Name of the mini-protocol.
        protocol: &'static str,
        /// The expected state(s).
        expected: String,
        /// The actual state found.
        actual: String,
    },
    /// Remote peer exceeded protocol bounds (e.g. sent more tx IDs than requested).
    BoundsExceeded {
        /// Name of the mini-protocol.
        protocol: &'static str,
        /// Human-readable description of the violation.
        reason: String,
    },
    /// KeepAlive response timed out repeatedly, indicating an unresponsive peer.
    ///
    /// After `consecutive_failures` consecutive pong timeouts (default threshold: 3),
    /// the KeepAlive client returns this error to signal that the peer should be
    /// demoted and the connection closed. This provides faster failure detection
    /// than waiting for the mux to die via TCP timeout or idle detection.
    KeepAliveTimeout {
        /// Number of consecutive pong timeouts before giving up.
        consecutive_failures: u32,
    },
    /// Multiplexer error propagated through the protocol layer.
    Mux(MuxError),
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AgencyViolation {
                protocol,
                state,
                received_tag,
            } => {
                write!(
                    f,
                    "{protocol}: agency violation in state {state}, received tag {received_tag}"
                )
            }
            Self::InvalidMessage {
                protocol,
                tag,
                reason,
            } => {
                write!(f, "{protocol}: invalid message tag {tag}: {reason}")
            }
            Self::CborDecode { protocol, reason } => {
                write!(f, "{protocol}: CBOR decode error: {reason}")
            }
            Self::StateViolation {
                protocol,
                expected,
                actual,
            } => {
                write!(f, "{protocol}: expected state {expected}, got {actual}")
            }
            Self::BoundsExceeded { protocol, reason } => {
                write!(f, "{protocol}: bounds exceeded: {reason}")
            }
            Self::KeepAliveTimeout {
                consecutive_failures,
            } => {
                write!(
                    f,
                    "KeepAlive: {consecutive_failures} consecutive response timeouts"
                )
            }
            Self::Mux(e) => write!(f, "mux: {e}"),
        }
    }
}

impl std::error::Error for ProtocolError {}

impl From<MuxError> for ProtocolError {
    fn from(e: MuxError) -> Self {
        Self::Mux(e)
    }
}

/// Connection manager errors for lifecycle and policy enforcement.
#[derive(Debug)]
pub enum ConnectionError {
    /// Maximum number of concurrent connections has been reached.
    MaxConnectionsReached,
    /// Connection rate limited for this peer address (too many recent attempts).
    RateLimited(std::net::SocketAddr),
    /// Simultaneous open conflict could not be resolved by tie-breaking.
    SimultaneousOpenConflict,
    /// Handshake failed during connection setup.
    HandshakeFailed(HandshakeError),
    /// TCP connect timed out before connection was established.
    ConnectTimeout,
    /// Connection is forbidden by policy (e.g. unidirectional reuse attempted).
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
    fn from(e: HandshakeError) -> Self {
        Self::HandshakeFailed(e)
    }
}
