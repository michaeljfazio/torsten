//! Unix domain socket bearer for N2C (node-to-client) connections.
//!
//! SDU payload size: 32,768 bytes (matching Haskell pipe bearer).
//! Used for local connections from cardano-cli and other tools via the node socket.

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use super::Bearer;
use crate::error::BearerError;

/// Unix socket SDU payload size (bytes). Matches Haskell's pipe bearer SDU size.
pub const UNIX_SDU_SIZE: usize = 32_768;

/// Unix socket batch size (bytes). Same as SDU size for local connections.
pub const UNIX_BATCH_SIZE: usize = 32_768;

/// Unix domain socket bearer for N2C connections.
pub struct UnixBearer {
    stream: UnixStream,
}

impl UnixBearer {
    /// Create a new Unix bearer wrapping an existing stream.
    pub fn new(stream: UnixStream) -> Self {
        Self { stream }
    }

    /// Consume this bearer and return the underlying `UnixStream`.
    pub fn into_stream(self) -> UnixStream {
        self.stream
    }
}

#[async_trait::async_trait]
impl Bearer for UnixBearer {
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
        UNIX_SDU_SIZE
    }

    fn batch_size(&self) -> usize {
        UNIX_BATCH_SIZE
    }

    fn split(
        self,
    ) -> (
        Box<dyn super::BearerReader + Send>,
        Box<dyn super::BearerWriter + Send>,
    ) {
        let (read_half, write_half) = self.stream.into_split();
        (
            Box::new(UnixBearerReader(read_half)),
            Box::new(UnixBearerWriter(write_half)),
        )
    }
}

/// Read half of a split Unix bearer.
struct UnixBearerReader(tokio::net::unix::OwnedReadHalf);

#[async_trait::async_trait]
impl super::BearerReader for UnixBearerReader {
    async fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), BearerError> {
        use tokio::io::AsyncReadExt;
        self.0.read_exact(buf).await.map_err(BearerError::from)?;
        Ok(())
    }
}

/// Write half of a split Unix bearer.
struct UnixBearerWriter(tokio::net::unix::OwnedWriteHalf);

#[async_trait::async_trait]
impl super::BearerWriter for UnixBearerWriter {
    async fn write_all(&mut self, buf: &[u8]) -> Result<(), BearerError> {
        use tokio::io::AsyncWriteExt;
        self.0.write_all(buf).await.map_err(BearerError::from)
    }
    async fn flush(&mut self) -> Result<(), BearerError> {
        use tokio::io::AsyncWriteExt;
        self.0.flush().await.map_err(BearerError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bearer::Bearer;

    // ─── Constant verification ───────────────────────────────────────────────

    #[test]
    fn sdu_size_matches_haskell_pipe_bearer() {
        // Haskell uses 32768 for local/pipe bearers (N2C connections).
        assert_eq!(UNIX_SDU_SIZE, 32_768);
    }

    #[test]
    fn batch_size_matches_sdu_size() {
        // For Unix sockets, batch size equals SDU size (no coalescing needed).
        assert_eq!(UNIX_BATCH_SIZE, 32_768);
        assert_eq!(UNIX_BATCH_SIZE, UNIX_SDU_SIZE);
    }

    // ─── Connection lifecycle tests ──────���───────────────────────────────────

    #[tokio::test]
    async fn connect_and_read_write() {
        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("test.sock");

        let listener = tokio::net::UnixListener::bind(&sock_path).unwrap();

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 5];
            tokio::io::AsyncReadExt::read_exact(&mut stream, &mut buf)
                .await
                .unwrap();
            tokio::io::AsyncWriteExt::write_all(&mut stream, &buf)
                .await
                .unwrap();
        });

        let stream = UnixStream::connect(&sock_path).await.unwrap();
        let mut bearer = UnixBearer::new(stream);

        // Verify sizes.
        assert_eq!(bearer.sdu_size(), UNIX_SDU_SIZE);
        assert_eq!(bearer.batch_size(), UNIX_BATCH_SIZE);

        // Write and read.
        bearer.write_all(b"hello").await.unwrap();
        bearer.flush().await.unwrap();

        let mut buf = [0u8; 5];
        bearer.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hello");

        // Drop bearer to clean up (close() may fail with NotConnected on
        // Unix sockets depending on OS implementation).
        drop(bearer);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn bearer_split_concurrent_io() {
        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("split.sock");

        let listener = tokio::net::UnixListener::bind(&sock_path).unwrap();

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

        let stream = UnixStream::connect(&sock_path).await.unwrap();
        let bearer = UnixBearer::new(stream);
        let (mut reader, mut writer) = bearer.split();

        writer.write_all(b"unix").await.unwrap();
        writer.flush().await.unwrap();

        let mut buf = [0u8; 4];
        reader.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"unix");

        server.await.unwrap();
    }

    #[tokio::test]
    async fn read_on_closed_connection_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("closed.sock");

        let listener = tokio::net::UnixListener::bind(&sock_path).unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            drop(stream);
        });

        let stream = UnixStream::connect(&sock_path).await.unwrap();
        let mut bearer = UnixBearer::new(stream);

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let mut buf = [0u8; 1];
        let result = bearer.read_exact(&mut buf).await;
        assert!(result.is_err(), "read on closed unix socket should fail");

        server.await.unwrap();
    }

    #[tokio::test]
    async fn into_stream_returns_underlying_stream() {
        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("into.sock");

        let listener = tokio::net::UnixListener::bind(&sock_path).unwrap();

        let _server = tokio::spawn(async move {
            let _ = listener.accept().await;
        });

        let stream = UnixStream::connect(&sock_path).await.unwrap();
        let bearer = UnixBearer::new(stream);
        let _stream = bearer.into_stream();
        // If into_stream() returns without panic, the stream is valid.
    }
}
