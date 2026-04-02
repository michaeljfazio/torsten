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
