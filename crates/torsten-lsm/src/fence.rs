//! Fence pointer index for SSTable page lookup.
//!
//! Stores the first key of each page, enabling binary search to find which
//! page might contain a given key. This is the "compact" variant optimized
//! for fixed-length hash keys (like Cardano UTxO keys) but works with any
//! variable-length key.
//!
//! Serialization format:
//! ```text
//! [entry_count: u32 LE]
//! For each entry:
//!   [key_len: u16 LE] [key_bytes] [page_offset: u64 LE]
//! ```

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use std::io::{Cursor, Read, Write};

use crate::error::Result;
use crate::key::Key;

/// A fence pointer index entry: first key of a page and its byte offset.
#[derive(Debug, Clone)]
struct FenceEntry {
    /// First key in the referenced page.
    first_key: Key,
    /// Byte offset of the page in the SSTable data file.
    page_offset: u64,
}

/// Fence pointer index for fast page lookup.
#[derive(Debug, Clone)]
pub struct FenceIndex {
    entries: Vec<FenceEntry>,
}

impl FenceIndex {
    /// Create a new empty fence index.
    pub fn new() -> Self {
        FenceIndex {
            entries: Vec::new(),
        }
    }

    /// Add a fence pointer for a page.
    pub fn add(&mut self, first_key: Key, page_offset: u64) {
        self.entries.push(FenceEntry {
            first_key,
            page_offset,
        });
    }

    /// Number of pages indexed.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the index is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Find the page offset that might contain the given key.
    ///
    /// Returns `None` if the key is before all indexed pages. Otherwise returns
    /// the offset of the page whose key range includes the search key (i.e.,
    /// the last page whose first_key <= search key).
    pub fn find_page(&self, key: &Key) -> Option<u64> {
        if self.entries.is_empty() {
            return None;
        }

        // Binary search: find the rightmost entry where first_key <= key
        match self.entries.binary_search_by(|e| e.first_key.cmp(key)) {
            Ok(idx) => Some(self.entries[idx].page_offset),
            Err(0) => None, // Key is before the first page
            Err(idx) => Some(self.entries[idx - 1].page_offset),
        }
    }

    /// Return page offsets for all pages that may contain keys in [from, to].
    pub fn find_pages_in_range(&self, from: &Key, to: &Key) -> Vec<u64> {
        if self.entries.is_empty() {
            return Vec::new();
        }

        // Find the first page that might contain keys >= from
        let start_idx = match self.entries.binary_search_by(|e| e.first_key.cmp(from)) {
            Ok(idx) => idx,
            Err(0) => 0,
            Err(idx) => idx - 1,
        };

        // Find the last page that might contain keys <= to
        let end_idx = match self.entries.binary_search_by(|e| e.first_key.cmp(to)) {
            Ok(idx) => idx,
            Err(idx) => {
                if idx == 0 {
                    return Vec::new();
                }
                idx - 1
            }
        };

        (start_idx..=end_idx)
            .map(|i| self.entries[i].page_offset)
            .collect()
    }

    /// Return all page offsets in order.
    pub fn all_page_offsets(&self) -> Vec<u64> {
        self.entries.iter().map(|e| e.page_offset).collect()
    }

    /// Serialize the fence index to bytes.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut buf = Vec::new();
        buf.write_u32::<LittleEndian>(self.entries.len() as u32)?;
        for entry in &self.entries {
            buf.write_u16::<LittleEndian>(entry.first_key.len() as u16)?;
            buf.write_all(entry.first_key.as_ref())?;
            buf.write_u64::<LittleEndian>(entry.page_offset)?;
        }
        Ok(buf)
    }

    /// Deserialize a fence index from bytes.
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        let mut cursor = Cursor::new(data);
        let entry_count = cursor.read_u32::<LittleEndian>()? as usize;
        let mut entries = Vec::with_capacity(entry_count);
        for _ in 0..entry_count {
            let key_len = cursor.read_u16::<LittleEndian>()? as usize;
            let mut key_buf = vec![0u8; key_len];
            cursor.read_exact(&mut key_buf)?;
            let page_offset = cursor.read_u64::<LittleEndian>()?;
            entries.push(FenceEntry {
                first_key: Key::new(key_buf),
                page_offset,
            });
        }
        Ok(FenceIndex { entries })
    }
}

impl Default for FenceIndex {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fence_find_page() {
        let mut fence = FenceIndex::new();
        fence.add(Key::from([10]), 0);
        fence.add(Key::from([20]), 4096);
        fence.add(Key::from([30]), 8192);

        // Key before all pages
        assert_eq!(fence.find_page(&Key::from([5])), None);

        // Key in first page
        assert_eq!(fence.find_page(&Key::from([10])), Some(0));
        assert_eq!(fence.find_page(&Key::from([15])), Some(0));

        // Key in second page
        assert_eq!(fence.find_page(&Key::from([20])), Some(4096));
        assert_eq!(fence.find_page(&Key::from([25])), Some(4096));

        // Key in third page
        assert_eq!(fence.find_page(&Key::from([30])), Some(8192));
        assert_eq!(fence.find_page(&Key::from([99])), Some(8192));
    }

    #[test]
    fn test_fence_range() {
        let mut fence = FenceIndex::new();
        fence.add(Key::from([10]), 0);
        fence.add(Key::from([20]), 4096);
        fence.add(Key::from([30]), 8192);
        fence.add(Key::from([40]), 12288);

        let pages = fence.find_pages_in_range(&Key::from([15]), &Key::from([35]));
        assert_eq!(pages, vec![0, 4096, 8192]);
    }

    #[test]
    fn test_fence_serialization_roundtrip() {
        let mut fence = FenceIndex::new();
        fence.add(Key::from([1, 2, 3]), 0);
        fence.add(Key::from([4, 5, 6]), 4096);

        let bytes = fence.to_bytes().unwrap();
        let restored = FenceIndex::from_bytes(&bytes).unwrap();
        assert_eq!(restored.len(), 2);
        assert_eq!(restored.find_page(&Key::from([3, 0, 0])), Some(0));
    }

    #[test]
    fn test_fence_empty() {
        let fence = FenceIndex::new();
        assert!(fence.is_empty());
        assert_eq!(fence.find_page(&Key::from([1])), None);
    }
}
