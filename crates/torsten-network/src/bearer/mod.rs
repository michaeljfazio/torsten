//! Transport abstraction layer for Ouroboros connections.
//!
//! The [`Bearer`] trait provides async read/write over TCP or Unix sockets.
//! Each bearer type defines its own SDU payload size and batch size matching
//! the Haskell reference implementation.
//!
//! Implementations:
//! - [`tcp::TcpBearer`] — TCP transport (N2N), SDU size 12,288 bytes
//! - [`unix::UnixBearer`] — Unix domain socket transport (N2C), SDU size 32,768 bytes
//! - [`MockBearer`] — Test-only bearer with pre-recorded byte sequences

pub mod tcp;
pub mod unix;

use crate::error::BearerError;

/// Abstract async transport. One bearer per connection.
///
/// The bearer provides a simple read/write interface that the multiplexer
/// uses to send and receive SDU segments. Each bearer type configures its
/// own SDU payload size and batch size to match Haskell's bearer parameters.
#[async_trait::async_trait]
pub trait Bearer: Send + 'static {
    /// Read exactly `buf.len()` bytes from the transport.
    /// Returns `BearerError::ConnectionReset` on EOF.
    async fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), BearerError>;

    /// Write all bytes to the transport.
    async fn write_all(&mut self, buf: &[u8]) -> Result<(), BearerError>;

    /// Flush any buffered data to the underlying transport.
    async fn flush(&mut self) -> Result<(), BearerError>;

    /// Gracefully shut down the transport (send EOF).
    async fn close(&mut self) -> Result<(), BearerError>;

    /// Maximum SDU payload size for this bearer type (bytes).
    /// TCP = 12,288 (matching Haskell `SDUSize`), Unix = 32,768.
    fn sdu_size(&self) -> usize;

    /// Maximum bytes per write batch. The egress task accumulates up to this
    /// many bytes before calling `write_all` + `flush`.
    fn batch_size(&self) -> usize;
}

/// Mock bearer for testing. Feeds pre-recorded data on reads and captures writes.
///
/// Useful for unit testing the multiplexer and protocol layers without
/// requiring actual network connections.
#[cfg(test)]
pub struct MockBearer {
    /// Bytes available for reading (consumed front-to-back).
    read_data: std::collections::VecDeque<u8>,
    /// Bytes that have been written.
    write_data: Vec<u8>,
    /// Configured SDU payload size.
    sdu_size: usize,
    /// Configured batch size.
    batch_size: usize,
}

#[cfg(test)]
impl MockBearer {
    /// Create a new mock bearer with pre-loaded read data.
    pub fn new(read_data: Vec<u8>, sdu_size: usize, batch_size: usize) -> Self {
        Self {
            read_data: read_data.into(),
            write_data: Vec::new(),
            sdu_size,
            batch_size,
        }
    }

    /// Get a reference to all bytes that have been written to this bearer.
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
            *byte = self.read_data.pop_front().expect("checked length above");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_bearer_read_exact() {
        let mut bearer = MockBearer::new(vec![1, 2, 3, 4], 12288, 131072);
        let mut buf = [0u8; 4];
        bearer.read_exact(&mut buf).await.unwrap();
        assert_eq!(buf, [1, 2, 3, 4]);
    }

    #[tokio::test]
    async fn mock_bearer_read_past_end() {
        let mut bearer = MockBearer::new(vec![1, 2], 12288, 131072);
        let mut buf = [0u8; 4];
        let result = bearer.read_exact(&mut buf).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn mock_bearer_write_captures() {
        let mut bearer = MockBearer::new(vec![], 12288, 131072);
        bearer.write_all(&[10, 20, 30]).await.unwrap();
        bearer.write_all(&[40, 50]).await.unwrap();
        assert_eq!(bearer.written(), &[10, 20, 30, 40, 50]);
    }

    #[tokio::test]
    async fn mock_bearer_sdu_and_batch_sizes() {
        let bearer = MockBearer::new(vec![], 12288, 131072);
        assert_eq!(bearer.sdu_size(), 12288);
        assert_eq!(bearer.batch_size(), 131072);
    }
}
