//! `torsten-lsm` — A pure Rust LSM-tree engine.
//!
//! Drop-in replacement for `cardano-lsm` with additional features:
//! - **Write-ahead log (WAL)** for crash recovery (no data loss on ungraceful shutdown)
//! - **Lazy levelling compaction** (T=4 size ratio, matching Haskell lsm-tree)
//! - **Blocked bloom filters** (cache-line aligned, ~1% FPR at 10 bits/key)
//! - **Fence pointer indexes** for binary search over SSTable pages
//! - **LRU block cache** for SSTable page caching
//! - **Persistent snapshots** via hard links (zero-copy)
//! - **CRC32 checksums** on WAL entries and SSTable pages
//! - **Exclusive session lock** to prevent concurrent access corruption
//!
//! ## Quick start
//!
//! ```no_run
//! use torsten_lsm::{LsmTree, LsmConfig, Key, Value};
//!
//! let config = LsmConfig {
//!     memtable_size: 64 * 1024 * 1024,
//!     block_cache_size: 256 * 1024 * 1024,
//!     bloom_filter_bits_per_key: 10,
//!     ..LsmConfig::default()
//! };
//!
//! let mut tree = LsmTree::open("./my-db", config).unwrap();
//! tree.insert(&Key::from([1, 2, 3]), &Value::from([4, 5, 6])).unwrap();
//! let val = tree.get(&Key::from([1, 2, 3])).unwrap();
//! assert!(val.is_some());
//! ```

pub mod bloom;
pub mod cache;
pub mod compaction;
pub mod config;
pub mod error;
pub mod fence;
pub mod key;
pub mod level;
pub mod memtable;
pub mod merge;
pub mod run;
pub mod session_lock;
pub mod snapshot;
pub mod sstable;
pub mod tree;
pub mod value;
pub mod wal;

// Re-export public API types
pub use config::LsmConfig;
pub use error::Error;
pub use key::Key;
pub use tree::{LsmTree, RangeIter};
pub use value::Value;
