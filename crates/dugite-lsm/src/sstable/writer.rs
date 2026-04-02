//! SSTable writer: flushes sorted entries to page-aligned files.
//!
//! Takes a sorted iterator of `(Key, Option<Value>)` entries and writes them
//! into pages. Normal entries share pages (default 65536 bytes), but oversized
//! entries (e.g., Cardano UTxOs with large inline datums) get their own
//! jumbo pages that are rounded up to the page_size alignment.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use crate::bloom::BloomFilter;
use crate::error::Result;
use crate::fence::FenceIndex;
use crate::key::Key;
use crate::sstable::page::{Page, PAGE_HEADER_SIZE};
use crate::value::Value;

/// Result of writing an SSTable: the fence index, bloom filter, and entry count.
pub struct WriteResult {
    pub fence: FenceIndex,
    pub bloom: BloomFilter,
    pub entry_count: usize,
    pub page_count: usize,
}

/// Write a sorted run of entries to an SSTable data file.
///
/// The entries MUST be sorted by key. Duplicate keys should have been resolved
/// before calling this function (newest value wins).
///
/// Returns the fence index and bloom filter for the written data.
pub fn write_sstable(
    path: &Path,
    entries: &[(Key, Option<Value>)],
    page_size: usize,
    bloom_bits_per_key: usize,
) -> Result<WriteResult> {
    let mut file = BufWriter::new(File::create(path)?);
    let mut fence = FenceIndex::new();
    let mut bloom = BloomFilter::new(entries.len(), bloom_bits_per_key);
    let data_capacity = page_size - PAGE_HEADER_SIZE;

    let mut page_entries: Vec<(Key, Option<Value>)> = Vec::new();
    let mut page_data_size: usize = 0;
    let mut page_offset: u64 = 0;
    let mut page_count: usize = 0;

    for (key, value) in entries {
        let entry_size = Page::entry_size(key, value);

        // If adding this entry would exceed the current page, flush first
        if page_data_size + entry_size > data_capacity && !page_entries.is_empty() {
            // Flush current page (normal size)
            let page = Page {
                entries: std::mem::take(&mut page_entries),
            };
            fence.add(page.entries[0].0.clone(), page_offset);
            let encoded = page.encode(page_size)?;
            let written_size = encoded.len();
            file.write_all(&encoded)?;
            page_offset += written_size as u64;
            page_count += 1;
            page_data_size = 0;
        }

        // Add ALL keys to bloom filter (including tombstones) so that
        // lookups for deleted keys correctly find the tombstone entry.
        bloom.insert(key);

        page_entries.push((key.clone(), value.clone()));
        page_data_size += entry_size;
    }

    // Flush final page
    if !page_entries.is_empty() {
        let page = Page {
            entries: std::mem::take(&mut page_entries),
        };
        fence.add(page.entries[0].0.clone(), page_offset);
        // Page::encode handles oversized entries by producing a larger page
        let encoded = page.encode(page_size)?;
        file.write_all(&encoded)?;
        page_count += 1;
    }

    file.flush()?;

    Ok(WriteResult {
        fence,
        bloom,
        entry_count: entries.len(),
        page_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_sstable_basic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.data");

        let entries: Vec<(Key, Option<Value>)> = vec![
            (Key::from([1]), Some(Value::from([10]))),
            (Key::from([2]), Some(Value::from([20]))),
            (Key::from([3]), Some(Value::from([30]))),
        ];

        let result = write_sstable(&path, &entries, 4096, 10).unwrap();
        assert_eq!(result.entry_count, 3);
        assert_eq!(result.page_count, 1); // Small entries fit in one page
        assert_eq!(result.fence.len(), 1);

        // Verify bloom filter
        assert!(result.bloom.may_contain(&Key::from([1])));
        assert!(result.bloom.may_contain(&Key::from([2])));
        assert!(result.bloom.may_contain(&Key::from([3])));
    }

    #[test]
    fn test_write_sstable_multiple_pages() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.data");

        // Create entries large enough to span multiple pages
        let mut entries = Vec::new();
        for i in 0u16..500 {
            let key = Key::from(i.to_be_bytes());
            let value = Value::from(vec![i as u8; 200]);
            entries.push((key, Some(value)));
        }

        let result = write_sstable(&path, &entries, 4096, 10).unwrap();
        assert_eq!(result.entry_count, 500);
        assert!(result.page_count > 1);
        assert_eq!(result.fence.len(), result.page_count);
    }

    #[test]
    fn test_write_sstable_with_tombstones() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.data");

        let entries: Vec<(Key, Option<Value>)> = vec![
            (Key::from([1]), Some(Value::from([10]))),
            (Key::from([2]), None), // tombstone
            (Key::from([3]), Some(Value::from([30]))),
        ];

        let result = write_sstable(&path, &entries, 4096, 10).unwrap();
        assert_eq!(result.entry_count, 3);
        // Tombstones ARE in the bloom filter (needed for correct lookup)
        assert!(result.bloom.may_contain(&Key::from([2])));
    }

    #[test]
    fn test_write_sstable_oversized_entry() {
        // Simulate a Cardano UTxO with a large inline datum (~13KB)
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.data");

        let entries: Vec<(Key, Option<Value>)> = vec![
            (Key::from([1]), Some(Value::from(vec![10u8; 100]))),
            (Key::from([2]), Some(Value::from(vec![20u8; 13_300]))), // 13.3KB
            (Key::from([3]), Some(Value::from(vec![30u8; 100]))),
        ];

        // Even with 4096-byte pages, oversized entries should succeed
        let result = write_sstable(&path, &entries, 4096, 10).unwrap();
        assert_eq!(result.entry_count, 3);
        assert!(result.bloom.may_contain(&Key::from([2])));
    }
}
