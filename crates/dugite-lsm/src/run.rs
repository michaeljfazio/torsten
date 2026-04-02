//! Sorted run: an immutable on-disk SSTable with its bloom filter and fence index.
//!
//! A run is the atomic unit of the LSM tree. Each memtable flush or compaction
//! produces a new run. Runs are immutable once written — they are only deleted
//! when no longer referenced (after compaction or snapshot deletion).

use std::fs;
use std::path::{Path, PathBuf};

use crate::bloom::BloomFilter;
use crate::cache::{BlockCache, CacheKey};
use crate::error::Result;
use crate::fence::FenceIndex;
use crate::key::Key;
use crate::sstable::reader::SsTableReader;
use crate::sstable::writer::write_sstable;
use crate::value::Value;

/// A sorted run on disk.
pub struct Run {
    /// Unique identifier for this run (monotonically increasing).
    pub id: u64,
    /// Fence pointer index for page lookup.
    fence: FenceIndex,
    /// Bloom filter for probabilistic key check.
    bloom: BloomFilter,
    /// SSTable data file reader.
    reader: SsTableReader,
    /// Number of entries in this run.
    pub entry_count: usize,
    /// Number of pages in this run.
    pub page_count: usize,
    /// Page size in bytes.
    page_size: usize,
}

#[allow(dead_code)]
impl Run {
    /// Write a new sorted run from the given entries.
    ///
    /// Creates three files: `run-{id:06}.data`, `.bloom`, `.index`.
    pub fn write(
        dir: &Path,
        id: u64,
        entries: &[(Key, Option<Value>)],
        page_size: usize,
        bloom_bits_per_key: usize,
    ) -> Result<Self> {
        let data_path = run_data_path(dir, id);
        let bloom_path = run_bloom_path(dir, id);
        let index_path = run_index_path(dir, id);

        let result = write_sstable(&data_path, entries, page_size, bloom_bits_per_key)?;

        // Write bloom filter
        fs::write(&bloom_path, result.bloom.to_bytes())?;

        // Write fence index
        fs::write(&index_path, result.fence.to_bytes()?)?;

        let reader = SsTableReader::open(&data_path, page_size);

        Ok(Run {
            id,
            fence: result.fence,
            bloom: result.bloom,
            reader,
            entry_count: result.entry_count,
            page_count: result.page_count,
            page_size,
        })
    }

    /// Open an existing run from disk.
    pub fn open(dir: &Path, id: u64, page_size: usize) -> Result<Self> {
        let data_path = run_data_path(dir, id);
        let bloom_path = run_bloom_path(dir, id);
        let index_path = run_index_path(dir, id);

        let bloom_data = fs::read(&bloom_path)?;
        let bloom = BloomFilter::from_bytes(&bloom_data).ok_or_else(|| {
            crate::error::Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("corrupt bloom filter: {}", bloom_path.display()),
            ))
        })?;

        let index_data = fs::read(&index_path)?;
        let fence = FenceIndex::from_bytes(&index_data)?;

        let reader = SsTableReader::open(&data_path, page_size);
        let file_len = fs::metadata(&data_path)?.len();
        let page_count = (file_len / page_size as u64) as usize;

        // Count entries by reading all pages (only done at open time)
        let pages = reader.read_all_pages()?;
        let entry_count: usize = pages.iter().map(|p| p.entries.len()).sum();

        Ok(Run {
            id,
            fence,
            bloom,
            reader,
            entry_count,
            page_count,
            page_size,
        })
    }

    /// Look up a key in this run.
    ///
    /// Returns `Some(Some(value))` for a live entry, `Some(None)` for a tombstone,
    /// or `None` if the key is not in this run.
    pub fn get(&self, key: &Key, cache: Option<&mut BlockCache>) -> Result<Option<Option<Value>>> {
        // Check bloom filter first
        if !self.bloom.may_contain(key) {
            return Ok(None);
        }

        // Find the page via fence index
        let page_offset = match self.fence.find_page(key) {
            Some(offset) => offset,
            None => return Ok(None),
        };

        let page_index = (page_offset / self.page_size as u64) as u32;
        let cache_key = CacheKey {
            run_id: self.id,
            page_index,
        };

        // Check cache first
        if let Some(cache) = cache {
            if let Some(page) = cache.get(&cache_key) {
                return Ok(page.search(key).cloned());
            }
        }

        // Read from disk
        let page = self.reader.read_page(page_offset)?;
        let result = page.search(key).cloned();

        // We can't insert into cache here since we already consumed the
        // mutable reference above. The caller can handle caching separately.
        // For now, we accept the slight inefficiency.

        Ok(result)
    }

    /// Look up a key in this run with separate cache handling.
    pub fn get_with_cache_insert(
        &self,
        key: &Key,
        cache: &mut BlockCache,
    ) -> Result<Option<Option<Value>>> {
        // Check bloom filter first
        if !self.bloom.may_contain(key) {
            return Ok(None);
        }

        // Find the page via fence index
        let page_offset = match self.fence.find_page(key) {
            Some(offset) => offset,
            None => return Ok(None),
        };

        let page_index = (page_offset / self.page_size as u64) as u32;
        let cache_key = CacheKey {
            run_id: self.id,
            page_index,
        };

        // Check cache
        if let Some(page) = cache.get(&cache_key) {
            return Ok(page.search(key).cloned());
        }

        // Read from disk and cache
        let page = self.reader.read_page(page_offset)?;
        let result = page.search(key).cloned();
        cache.insert(cache_key, page);

        Ok(result)
    }

    /// Read all entries from this run in sorted order.
    pub fn scan_all(&self) -> Result<Vec<(Key, Option<Value>)>> {
        let pages = self.reader.read_all_pages()?;
        let mut entries = Vec::with_capacity(self.entry_count);
        for page in pages {
            entries.extend(page.entries);
        }
        Ok(entries)
    }

    /// Read entries in the given key range [from, to] (inclusive both ends).
    pub fn scan_range(&self, from: &Key, to: &Key) -> Result<Vec<(Key, Option<Value>)>> {
        let page_offsets = self.fence.find_pages_in_range(from, to);
        let mut entries = Vec::new();
        for offset in page_offsets {
            let page = self.reader.read_page(offset)?;
            for (key, value) in page.entries {
                if &key >= from && &key <= to {
                    entries.push((key, value));
                }
            }
        }
        Ok(entries)
    }

    /// Delete the run files from disk.
    pub fn delete_files(dir: &Path, id: u64) -> Result<()> {
        let _ = fs::remove_file(run_data_path(dir, id));
        let _ = fs::remove_file(run_bloom_path(dir, id));
        let _ = fs::remove_file(run_index_path(dir, id));
        Ok(())
    }
}

/// Generate the data file path for a run.
pub fn run_data_path(dir: &Path, id: u64) -> PathBuf {
    dir.join(format!("run-{id:06}.data"))
}

/// Generate the bloom filter file path for a run.
pub fn run_bloom_path(dir: &Path, id: u64) -> PathBuf {
    dir.join(format!("run-{id:06}.bloom"))
}

/// Generate the fence index file path for a run.
pub fn run_index_path(dir: &Path, id: u64) -> PathBuf {
    dir.join(format!("run-{id:06}.index"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_write_and_read() {
        let dir = tempfile::tempdir().unwrap();
        let active_dir = dir.path().join("active");
        fs::create_dir_all(&active_dir).unwrap();

        let entries: Vec<(Key, Option<Value>)> = vec![
            (Key::from([1, 0]), Some(Value::from(vec![10u8; 50]))),
            (Key::from([2, 0]), Some(Value::from(vec![20u8; 50]))),
            (Key::from([3, 0]), None), // tombstone
            (Key::from([4, 0]), Some(Value::from(vec![40u8; 50]))),
        ];

        let run = Run::write(&active_dir, 1, &entries, 4096, 10).unwrap();
        assert_eq!(run.entry_count, 4);
        assert_eq!(run.id, 1);

        // Point lookups
        let mut cache = BlockCache::new(100);
        let result = run
            .get_with_cache_insert(&Key::from([1, 0]), &mut cache)
            .unwrap();
        assert!(result.unwrap().is_some()); // live value

        let result = run
            .get_with_cache_insert(&Key::from([3, 0]), &mut cache)
            .unwrap();
        assert!(result.unwrap().is_none()); // tombstone

        let result = run
            .get_with_cache_insert(&Key::from([5, 0]), &mut cache)
            .unwrap();
        assert!(result.is_none()); // not found
    }

    #[test]
    fn test_run_scan_all() {
        let dir = tempfile::tempdir().unwrap();
        let active_dir = dir.path().join("active");
        fs::create_dir_all(&active_dir).unwrap();

        let entries: Vec<(Key, Option<Value>)> = vec![
            (Key::from([1]), Some(Value::from([10]))),
            (Key::from([2]), Some(Value::from([20]))),
            (Key::from([3]), Some(Value::from([30]))),
        ];

        let run = Run::write(&active_dir, 1, &entries, 4096, 10).unwrap();
        let scanned = run.scan_all().unwrap();
        assert_eq!(scanned.len(), 3);
    }

    #[test]
    fn test_run_open_existing() {
        let dir = tempfile::tempdir().unwrap();
        let active_dir = dir.path().join("active");
        fs::create_dir_all(&active_dir).unwrap();

        let entries: Vec<(Key, Option<Value>)> = vec![
            (Key::from([1]), Some(Value::from([10]))),
            (Key::from([2]), Some(Value::from([20]))),
        ];

        Run::write(&active_dir, 42, &entries, 4096, 10).unwrap();

        // Reopen
        let run = Run::open(&active_dir, 42, 4096).unwrap();
        assert_eq!(run.id, 42);
        assert_eq!(run.entry_count, 2);
    }

    #[test]
    fn test_run_scan_range() {
        let dir = tempfile::tempdir().unwrap();
        let active_dir = dir.path().join("active");
        fs::create_dir_all(&active_dir).unwrap();

        let entries: Vec<(Key, Option<Value>)> = (1u8..=10)
            .map(|i| (Key::from([i]), Some(Value::from([i * 10]))))
            .collect();

        let run = Run::write(&active_dir, 1, &entries, 4096, 10).unwrap();
        let range = run.scan_range(&Key::from([3]), &Key::from([7])).unwrap();
        assert_eq!(range.len(), 5); // 3, 4, 5, 6, 7
    }
}
