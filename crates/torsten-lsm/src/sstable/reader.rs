//! SSTable reader: reads and decodes pages from SSTable data files.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::sstable::page::Page;

/// Reader for an SSTable data file.
pub struct SsTableReader {
    path: PathBuf,
    page_size: usize,
}

impl SsTableReader {
    /// Open an SSTable data file for reading.
    pub fn open(path: &Path, page_size: usize) -> Self {
        SsTableReader {
            path: path.to_path_buf(),
            page_size,
        }
    }

    /// Read and decode a single page at the given byte offset.
    pub fn read_page(&self, offset: u64) -> Result<Page> {
        let mut file = File::open(&self.path)?;
        file.seek(SeekFrom::Start(offset))?;
        let mut buf = vec![0u8; self.page_size];
        file.read_exact(&mut buf)?;
        Page::decode(&buf)
    }

    /// Read and decode a page by page index (page_index * page_size).
    pub fn read_page_by_index(&self, page_index: u32) -> Result<Page> {
        let offset = page_index as u64 * self.page_size as u64;
        self.read_page(offset)
    }

    /// Read all pages from the SSTable file sequentially.
    /// Returns all decoded pages in order.
    pub fn read_all_pages(&self) -> Result<Vec<Page>> {
        let mut file = File::open(&self.path)?;
        let file_len = file.metadata()?.len();
        let mut pages = Vec::new();
        let mut offset = 0u64;

        while offset + self.page_size as u64 <= file_len {
            file.seek(SeekFrom::Start(offset))?;
            let mut buf = vec![0u8; self.page_size];
            file.read_exact(&mut buf)?;
            pages.push(Page::decode(&buf)?);
            offset += self.page_size as u64;
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
}
