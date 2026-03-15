//! LSM-tree configuration with sensible defaults.
//!
//! Defaults are tuned for Cardano UTxO workloads (~36-byte keys, ~200-byte values,
//! ~20M entries on mainnet). The size ratio of 4 matches the Haskell cardano-node's
//! lsm-tree library.

/// Configuration for an LSM-tree instance.
#[derive(Debug, Clone)]
pub struct LsmConfig {
    /// Maximum memtable size in bytes before flushing to disk.
    /// Default: 64 MB.
    pub memtable_size: usize,

    /// Block cache size in bytes for SSTable page caching.
    /// Default: 256 MB.
    pub block_cache_size: usize,

    /// Bloom filter bits per key. 10 bits gives ~1% false positive rate.
    /// Default: 10.
    pub bloom_filter_bits_per_key: usize,

    /// Size ratio between levels (T). Each level is T times larger than the
    /// previous. Matches Haskell's lsm-tree default of 4.
    /// Default: 4.
    pub size_ratio: usize,

    /// Whether the write-ahead log is enabled for crash recovery.
    /// Default: true.
    pub wal_enabled: bool,

    /// Maximum WAL segment size in bytes before rotation.
    /// Default: 64 MB.
    pub wal_segment_size: usize,

    /// SSTable page size in bytes. Must be a power of 2.
    /// Default: 4096.
    pub page_size: usize,
}

impl Default for LsmConfig {
    fn default() -> Self {
        LsmConfig {
            memtable_size: 64 * 1024 * 1024,     // 64 MB
            block_cache_size: 256 * 1024 * 1024, // 256 MB
            bloom_filter_bits_per_key: 10,
            size_ratio: 4,
            wal_enabled: true,
            wal_segment_size: 64 * 1024 * 1024, // 64 MB
            page_size: 4096,
        }
    }
}

impl LsmConfig {
    /// Maximum entry size (key + value) that fits in a single page.
    /// Accounts for page header (8 bytes) and entry overhead (5 bytes per entry).
    pub fn max_entry_size(&self) -> usize {
        // page_header(8) + key_len(2) + key + tag(1) + value_len(2) + value
        self.page_size.saturating_sub(8 + 5)
    }

    /// Maximum number of pages in the block cache.
    pub fn cache_capacity(&self) -> usize {
        self.block_cache_size / self.page_size
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = LsmConfig::default();
        assert_eq!(config.memtable_size, 64 * 1024 * 1024);
        assert_eq!(config.block_cache_size, 256 * 1024 * 1024);
        assert_eq!(config.bloom_filter_bits_per_key, 10);
        assert_eq!(config.size_ratio, 4);
        assert!(config.wal_enabled);
        assert_eq!(config.page_size, 4096);
    }

    #[test]
    fn test_max_entry_size() {
        let config = LsmConfig::default();
        assert_eq!(config.max_entry_size(), 4096 - 8 - 5);
    }

    #[test]
    fn test_cache_capacity() {
        let config = LsmConfig::default();
        assert_eq!(config.cache_capacity(), 256 * 1024 * 1024 / 4096);
    }
}
