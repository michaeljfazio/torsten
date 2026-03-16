//! SSTable reader: reads and decodes pages from SSTable data files.
//!
//! Supports both normal-size pages and oversized "jumbo" pages. The page
//! header's `data_end` field (u32) tells the reader how much data is in
//! each page. For normal pages, `data_end <= page_size`. For jumbo pages,
//! `data_end > page_size` and the reader reads the additional bytes.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::sstable::page::{Page, PAGE_HEADER_SIZE};

/// Reader for an SSTable data file.
pub struct SsTableReader {
    path: PathBuf,
    page_size: usize,
}

#[allow(dead_code)]
impl SsTableReader {
    /// Open an SSTable data file for reading.
    pub fn open(path: &Path, page_size: usize) -> Self {
        SsTableReader {
            path: path.to_path_buf(),
            page_size,
        }
    }

    /// Read and decode a single page at the given byte offset.
    ///
    /// First reads the header to determine actual page size, then reads
    /// the full page (which may be larger than `page_size` for jumbo pages).
    pub fn read_page(&self, offset: u64) -> Result<Page> {
        let mut file = File::open(&self.path)?;
        file.seek(SeekFrom::Start(offset))?;

        // Read at least one page_size worth of data
        let mut buf = vec![0u8; self.page_size];
        file.read_exact(&mut buf)?;

        // Peek at the data_end field (u32 LE at offset 2) to check for jumbo pages
        if buf.len() >= PAGE_HEADER_SIZE {
            let data_end = u32::from_le_bytes([buf[2], buf[3], buf[4], buf[5]]) as usize;

            // If data extends beyond initial read, read the rest
            if data_end > buf.len() {
                let additional = data_end - buf.len();
                let mut extra = vec![0u8; additional];
                file.read_exact(&mut extra)?;
                buf.extend_from_slice(&extra);
            }
        }

        Page::decode(&buf)
    }

    /// Read and decode a page by page index.
    ///
    /// NOTE: This only works correctly for SSTables where all pages are
    /// the same size (no jumbo pages). Use `read_page` with explicit
    /// offsets from the fence index for mixed-size pages.
    pub fn read_page_by_index(&self, page_index: u32) -> Result<Page> {
        let offset = page_index as u64 * self.page_size as u64;
        self.read_page(offset)
    }

    /// Read all pages from the SSTable file sequentially.
    ///
    /// Handles variable-size pages by reading headers to determine each
    /// page's actual size.
    pub fn read_all_pages(&self) -> Result<Vec<Page>> {
        let mut file = File::open(&self.path)?;
        let file_len = file.metadata()?.len();
        let mut pages = Vec::new();
        let mut offset = 0u64;

        while offset < file_len {
            file.seek(SeekFrom::Start(offset))?;

            // Read minimum page_size
            let remaining = (file_len - offset) as usize;
            let read_size = remaining.min(self.page_size);
            let mut buf = vec![0u8; read_size];
            file.read_exact(&mut buf)?;

            if buf.len() < PAGE_HEADER_SIZE {
                break; // Not enough data for a page header
            }

            // Check data_end for jumbo page detection
            let data_end = u32::from_le_bytes([buf[2], buf[3], buf[4], buf[5]]) as usize;

            if data_end > buf.len() {
                // Jumbo page — read additional data
                let additional = data_end - buf.len();
                let mut extra = vec![0u8; additional];
                file.read_exact(&mut extra)?;
                buf.extend_from_slice(&extra);
            }

            pages.push(Page::decode(&buf)?);

            // Advance to next page boundary (rounded up to page_size alignment)
            let actual_size = buf.len().max(self.page_size);
            let aligned_size = actual_size.div_ceil(self.page_size) * self.page_size;
            offset += aligned_size as u64;
        }

        Ok(pages)
    }

    /// Get the path to the data file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::key::Key;
    use crate::sstable::writer::write_sstable;
    use crate::value::Value;

    #[test]
    fn test_read_page() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.data");

        let entries: Vec<(Key, Option<Value>)> = vec![
            (Key::from([1, 2, 3]), Some(Value::from([10, 20]))),
            (Key::from([4, 5, 6]), Some(Value::from([30, 40]))),
        ];

        let result = write_sstable(&path, &entries, 4096, 10).unwrap();
        assert_eq!(result.page_count, 1);

        let reader = SsTableReader::open(&path, 4096);
        let page = reader.read_page(0).unwrap();
        assert_eq!(page.entries.len(), 2);
        assert_eq!(page.entries[0].0.as_ref(), &[1, 2, 3]);
    }

    #[test]
    fn test_read_all_pages() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.data");

        // Create enough entries to span multiple pages
        let mut entries = Vec::new();
        for i in 0u16..500 {
            let key = Key::from(i.to_be_bytes());
            let value = Value::from(vec![i as u8; 200]);
            entries.push((key, Some(value)));
        }

        let result = write_sstable(&path, &entries, 4096, 10).unwrap();
        assert!(result.page_count > 1);

        let reader = SsTableReader::open(&path, 4096);
        let pages = reader.read_all_pages().unwrap();
        assert_eq!(pages.len(), result.page_count);

        // Verify total entries across all pages
        let total: usize = pages.iter().map(|p| p.entries.len()).sum();
        assert_eq!(total, 500);
    }

    #[test]
    fn test_read_oversized_entry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.data");

        // Write an SSTable with a mix of normal and oversized entries
        let entries: Vec<(Key, Option<Value>)> = vec![
            (Key::from([1]), Some(Value::from(vec![10u8; 100]))),
            (Key::from([2]), Some(Value::from(vec![20u8; 10_000]))), // 10KB jumbo
            (Key::from([3]), Some(Value::from(vec![30u8; 100]))),
        ];

        write_sstable(&path, &entries, 4096, 10).unwrap();

        let reader = SsTableReader::open(&path, 4096);
        let pages = reader.read_all_pages().unwrap();

        // Verify all entries are recoverable
        let total: usize = pages.iter().map(|p| p.entries.len()).sum();
        assert_eq!(total, 3);

        // Find the page with the large entry
        let large_entry = pages
            .iter()
            .flat_map(|p| p.entries.iter())
            .find(|(k, _)| k.as_ref() == [2])
            .unwrap();
        assert_eq!(large_entry.1.as_ref().unwrap().len(), 10_000);
    }
}
