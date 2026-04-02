//! Error types for the LSM-tree engine.

use std::io;

/// All errors that can occur in LSM-tree operations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// I/O error from the underlying filesystem.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    /// CRC32 checksum mismatch — data corruption detected.
    #[error("checksum mismatch: expected {expected:#010x}, got {actual:#010x}")]
    ChecksumMismatch { expected: u32, actual: u32 },

    /// WAL entry is corrupted or truncated.
    #[error("WAL corruption at offset {offset}: {detail}")]
    WalCorruption { offset: u64, detail: String },

    /// Page data exceeds the configured page size.
    #[error("page overflow: entry requires {needed} bytes, page has {available} bytes free")]
    PageOverflow { needed: usize, available: usize },

    /// Attempted to open a database that is already locked by another process.
    #[error("database is locked by another process: {0}")]
    DatabaseLocked(String),

    /// Snapshot not found.
    #[error("snapshot not found: {0}")]
    SnapshotNotFound(String),

    /// Manifest is missing or corrupted.
    #[error("manifest error: {0}")]
    Manifest(String),

    /// Key is too large to fit in a single page.
    #[error("key too large: {size} bytes (max {max})")]
    KeyTooLarge { size: usize, max: usize },

    /// Value is too large to fit in a single page.
    #[error("value too large: {size} bytes (max {max})")]
    ValueTooLarge { size: usize, max: usize },
}

pub type Result<T> = std::result::Result<T, Error>;
