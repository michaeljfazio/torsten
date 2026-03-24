//! TCP bearer implementation for N2N connections.
//!
//! SDU payload size: 12,288 bytes (matching Haskell `makeSocketBearer`).
//! Batch size: 131,072 bytes.
//! TCP_NODELAY=false (Nagle enabled — mux egress batching handles coalescing).
//! SO_KEEPALIVE=true with 60s interval.

use socket2::{Socket, TcpKeepalive};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::Bearer;
use crate::error::BearerError;

/// TCP SDU payload size (bytes). Matches Haskell's `SDUSize 12_288`.
pub const TCP_SDU_SIZE: usize = 12_288;

/// TCP write batch size (bytes). Matches Haskell's batch of 131,072.
pub const TCP_BATCH_SIZE: usize = 131_072;

/// TCP read buffer size. Matches Haskell's `readBufferSize`.
pub const TCP_READ_BUFFER_SIZE: usize = 131_072;

/// TCP keepalive interval — sends probes after this idle duration.
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(60);

/// TCP bearer wrapping a tokio `TcpStream` with Cardano-specific socket options.
pub struct TcpBearer {
    stream: TcpStream,
}

impl TcpBearer {
    /// Create a new TCP bearer from an existing stream.
    ///
    /// Configures:
    /// - `TCP_NODELAY=false` (Nagle enabled — mux batching handles coalescing)
    /// - `SO_KEEPALIVE=true` with 60s interval
    pub fn new(stream: TcpStream) -> Result<Self, BearerError> {
        // Convert to socket2 for advanced option configuration
        let std_stream = stream.into_std().map_err(BearerError::Io)?;
        let socket = Socket::from(std_stream);

        // Nagle enabled (TCP_NODELAY=false) — mux egress batching handles coalescing,
        // so we let Nagle coalesce small writes at the TCP level too.
        socket.set_nodelay(false).map_err(BearerError::Io)?;

        // TCP keepalive probes detect dead connections.
        let keepalive = TcpKeepalive::new().with_time(KEEPALIVE_INTERVAL);
        socket
            .set_tcp_keepalive(&keepalive)
            .map_err(BearerError::Io)?;

        // Convert back to tokio TcpStream
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

    /// Consume this bearer and return the underlying `TcpStream`.
    pub fn into_stream(self) -> TcpStream {
        self.stream
    }
}

#[async_trait::async_trait]
impl Bearer for TcpBearer {
    async fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), BearerError> {
        self.stream
            .read_exact(buf)
            .await
            .map_err(BearerError::from)?;
        Ok(())
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
