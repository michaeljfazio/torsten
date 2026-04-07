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
///
/// Haskell's `BL.splitAt (fromIntegral sduSize) d` splits payload at
/// exactly this many bytes.  The 8-byte mux header is added separately.
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
        // Match Haskell cardano-node bearer configuration:
        // TCP_NODELAY=false (Nagle enabled — mux egress batching handles coalescing)
        // SO_KEEPALIVE with 60s interval
        //
        // Use socket2 for TCP option configuration, then convert back to tokio.
        // socket2 0.6 renamed set_nodelay() → set_tcp_nodelay().
        let std_stream = stream.into_std().map_err(BearerError::Io)?;
        let socket = Socket::from(std_stream);

        socket.set_tcp_nodelay(false).map_err(BearerError::Io)?;

        let keepalive = TcpKeepalive::new().with_time(KEEPALIVE_INTERVAL);
        socket
            .set_tcp_keepalive(&keepalive)
            .map_err(BearerError::Io)?;

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

    fn split(
        self,
    ) -> (
        Box<dyn super::BearerReader + Send>,
        Box<dyn super::BearerWriter + Send>,
    ) {
        let (read_half, write_half) = self.stream.into_split();
        (
            Box::new(TcpBearerReader(read_half)),
            Box::new(TcpBearerWriter(write_half)),
        )
    }
}

/// Read half of a split TCP bearer.
struct TcpBearerReader(tokio::net::tcp::OwnedReadHalf);

#[async_trait::async_trait]
impl super::BearerReader for TcpBearerReader {
    async fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), BearerError> {
        self.0.read_exact(buf).await.map_err(BearerError::from)?;
        Ok(())
    }
}

/// Write half of a split TCP bearer.
struct TcpBearerWriter(tokio::net::tcp::OwnedWriteHalf);

#[async_trait::async_trait]
impl super::BearerWriter for TcpBearerWriter {
    async fn write_all(&mut self, buf: &[u8]) -> Result<(), BearerError> {
        self.0.write_all(buf).await.map_err(BearerError::from)
    }
    async fn flush(&mut self) -> Result<(), BearerError> {
        self.0.flush().await.map_err(BearerError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bearer::Bearer;

    // ─── Constant verification ───────────────────────────────────────────────

    #[test]
    fn sdu_size_matches_haskell() {
        // Haskell cardano-node uses SDUSize 12288 for TCP bearers.
        assert_eq!(TCP_SDU_SIZE, 12_288);
    }

    #[test]
    fn batch_size_matches_haskell() {
        // Haskell cardano-node uses a batch size of 131072 for TCP write coalescing.
        assert_eq!(TCP_BATCH_SIZE, 131_072);
    }

    #[test]
    fn read_buffer_size_matches_haskell() {
        // Haskell's readBufferSize = 131072.
        assert_eq!(TCP_READ_BUFFER_SIZE, 131_072);
    }

    // ─── Connection lifecycle tests ──────────────────────────────────────────

    #[tokio::test]
    async fn connect_and_read_write() {
        // Create a TCP listener, connect a bearer, and verify data exchange.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            // Echo back whatever we read.
            let mut buf = [0u8; 5];
            tokio::io::AsyncReadExt::read_exact(&mut stream, &mut buf)
                .await
                .unwrap();
            tokio::io::AsyncWriteExt::write_all(&mut stream, &buf)
                .await
                .unwrap();
        });

        let mut bearer = TcpBearer::connect(addr).await.unwrap();

        // Verify SDU/batch sizes.
        assert_eq!(bearer.sdu_size(), TCP_SDU_SIZE);
        assert_eq!(bearer.batch_size(), TCP_BATCH_SIZE);

        // Write and read back.
        bearer.write_all(b"hello").await.unwrap();
        bearer.flush().await.unwrap();

        let mut buf = [0u8; 5];
        bearer.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hello");

        bearer.close().await.unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn bearer_split_concurrent_io() {
        // Verify that split() produces independent read and write halves
        // that can operate concurrently.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4];
            tokio::io::AsyncReadExt::read_exact(&mut stream, &mut buf)
                .await
                .unwrap();
            tokio::io::AsyncWriteExt::write_all(&mut stream, &buf)
                .await
                .unwrap();
        });

        let bearer = TcpBearer::connect(addr).await.unwrap();
        let (mut reader, mut writer) = bearer.split();

        // Write from one half, read from the other.
        writer.write_all(b"test").await.unwrap();
        writer.flush().await.unwrap();

        let mut buf = [0u8; 4];
        reader.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"test");

        server.await.unwrap();
    }

    #[tokio::test]
    async fn read_on_closed_connection_returns_error() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            // Close immediately.
            drop(stream);
        });

        let mut bearer = TcpBearer::connect(addr).await.unwrap();
        // Give the server time to close.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let mut buf = [0u8; 1];
        let result = bearer.read_exact(&mut buf).await;
        assert!(result.is_err(), "read on closed connection should fail");

        server.await.unwrap();
    }

    #[tokio::test]
    async fn new_configures_socket_options() {
        // Verify TcpBearer::new succeeds and configures the stream properly.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let _server = tokio::spawn(async move {
            let _ = listener.accept().await;
        });

        let stream = TcpStream::connect(addr).await.unwrap();
        let bearer = TcpBearer::new(stream);
        assert!(bearer.is_ok(), "TcpBearer::new should succeed");
    }

    #[tokio::test]
    async fn into_stream_returns_underlying_stream() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let _server = tokio::spawn(async move {
            let _ = listener.accept().await;
        });

        let bearer = TcpBearer::connect(addr).await.unwrap();
        let stream = bearer.into_stream();
        // Verify the stream is valid by checking peer addr.
        assert_eq!(stream.peer_addr().unwrap(), addr);
    }
}
