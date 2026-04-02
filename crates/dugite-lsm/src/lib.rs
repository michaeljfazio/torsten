#![allow(dead_code)] // Internal crate — methods may be used by future features
//! `dugite-lsm` — A pure Rust LSM-tree engine.
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
//! use dugite_lsm::{LsmTree, LsmConfig, Key, Value};
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

pub(crate) mod bloom;
pub(crate) mod cache;
pub(crate) mod compaction;
pub(crate) mod config;
pub(crate) mod error;
pub(crate) mod fence;
pub(crate) mod key;
pub(crate) mod level;
pub(crate) mod memtable;
pub(crate) mod merge;
pub(crate) mod run;
pub(crate) mod session_lock;
pub(crate) mod snapshot;
pub(crate) mod sstable;
pub(crate) mod tree;
pub(crate) mod value;
pub(crate) mod wal;

// Re-export public API types
pub use config::LsmConfig;
pub use error::Error;
pub use key::Key;
pub use tree::{LsmTree, RangeIter};
pub use value::Value;
