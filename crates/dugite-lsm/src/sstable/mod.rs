//! SSTable (Sorted String Table) on-disk format.
//!
//! An SSTable consists of:
//! - A data file (`.data`): sequence of fixed-size pages containing sorted entries
//! - A bloom filter file (`.bloom`): probabilistic existence check
//! - A fence index file (`.index`): page-level binary search index

pub mod page;
pub mod reader;
pub mod writer;
