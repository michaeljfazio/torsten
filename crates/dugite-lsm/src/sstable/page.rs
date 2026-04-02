//! Variable-size page format for SSTable storage.
//!
//! Each page has a minimum size of `page_size` (default 65536) but can be
//! larger for oversized entries. Layout:
//!
//! ```text
//! [entry_count: u16 LE]      offset 0
//! [data_end:    u32 LE]      offset 2  (byte offset where entries end)
//! [crc32:       u32 LE]      offset 6  (CRC of bytes [HEADER..data_end])
//! [entries...]               offset 10
//! [zero padding...]          offset data_end .. page boundary
//! ```
//!
//! Each entry within a page:
//! ```text
//! [key_len:     u16 LE]
//! [key_bytes:   key_len]
//! [tag:         u8]           0 = tombstone, 1 = value present
//! [value_len:   u32 LE]      only if tag == 1
//! [value_bytes: value_len]    only if tag == 1
//! ```
//!
//! Entries within a page are sorted by key for binary search.

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use std::io::{Cursor, Read, Write};

use crate::error::{Error, Result};
use crate::key::Key;
use crate::value::Value;

/// Header size in bytes: entry_count(2) + data_end(4) + crc32(4).
pub const PAGE_HEADER_SIZE: usize = 10;

/// Tag byte indicating a live value entry.
const TAG_VALUE: u8 = 1;
/// Tag byte indicating a tombstone (deletion marker).
const TAG_TOMBSTONE: u8 = 0;

/// A decoded page containing sorted entries.
#[derive(Debug, Clone)]
pub struct Page {
    /// Sorted entries: key -> optional value (None = tombstone).
    pub entries: Vec<(Key, Option<Value>)>,
}

impl Page {
    /// Calculate the on-disk size of an entry.
    /// key_len(2) + key + tag(1) + [value_len(4) + value if present]
    pub fn entry_size(key: &Key, value: &Option<Value>) -> usize {
        let mut size = 2 + key.len() + 1; // key_len + key + tag
        if let Some(v) = value {
            size += 4 + v.len(); // value_len(u32) + value
        }
        size
    }

    /// Encode this page into a buffer.
    ///
    /// The buffer is at least `min_page_size` bytes, but may be larger if the
    /// entries require more space. Returns the encoded bytes (which may exceed
    /// `min_page_size` for oversized entries).
    pub fn encode(&self, min_page_size: usize) -> Result<Vec<u8>> {
        let data_capacity = min_page_size.saturating_sub(PAGE_HEADER_SIZE);
        let mut data_buf = Vec::with_capacity(data_capacity);

        for (key, value) in &self.entries {
            encode_entry(&mut data_buf, key, value)?;
        }

        // Compute the actual page size needed (round up to min_page_size alignment)
        let needed = PAGE_HEADER_SIZE + data_buf.len();
        let actual_page_size = if needed <= min_page_size {
            min_page_size
        } else {
            // Round up to next multiple of min_page_size for alignment
            needed.div_ceil(min_page_size) * min_page_size
        };

        let data_end = (PAGE_HEADER_SIZE + data_buf.len()) as u32;
        let crc = crc32fast::hash(&data_buf);

        let mut page_buf = Vec::with_capacity(actual_page_size);
        page_buf.write_u16::<LittleEndian>(self.entries.len() as u16)?;
        page_buf.write_u32::<LittleEndian>(data_end)?;
        page_buf.write_u32::<LittleEndian>(crc)?;
        page_buf.extend_from_slice(&data_buf);
        // Zero-pad to actual page size
        page_buf.resize(actual_page_size, 0);

        Ok(page_buf)
    }

    /// Decode a page from a buffer.
    ///
    /// Verifies the CRC32 checksum and returns an error on mismatch.
    pub fn decode(buf: &[u8]) -> Result<Self> {
        if buf.len() < PAGE_HEADER_SIZE {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "page too small for header",
            )));
        }

        let mut cursor = Cursor::new(buf);
        let entry_count = cursor.read_u16::<LittleEndian>()? as usize;
        let data_end = cursor.read_u32::<LittleEndian>()? as usize;
        let expected_crc = cursor.read_u32::<LittleEndian>()?;

        // Validate data_end is within bounds
        if data_end > buf.len() || data_end < PAGE_HEADER_SIZE {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid data_end: {data_end}"),
            )));
        }

        // Verify CRC over the data region
        let data_region = &buf[PAGE_HEADER_SIZE..data_end];
        let actual_crc = crc32fast::hash(data_region);
        if actual_crc != expected_crc {
            return Err(Error::ChecksumMismatch {
                expected: expected_crc,
                actual: actual_crc,
            });
        }

        // Decode entries from the data region
        let mut data_cursor = Cursor::new(data_region);
        let mut entries = Vec::with_capacity(entry_count);
        for _ in 0..entry_count {
            let (key, value) = decode_entry(&mut data_cursor)?;
            entries.push((key, value));
        }

        Ok(Page { entries })
    }

    /// Binary search for a key within this page.
    /// Returns `Some(Some(value))` for a live entry, `Some(None)` for a tombstone,
    /// or `None` if the key is not in this page.
    pub fn search(&self, key: &Key) -> Option<&Option<Value>> {
        self.entries
            .binary_search_by(|(k, _)| k.cmp(key))
            .ok()
            .map(|idx| &self.entries[idx].1)
    }
}

/// Encode a single entry into a writer.
fn encode_entry<W: Write>(w: &mut W, key: &Key, value: &Option<Value>) -> Result<()> {
    w.write_u16::<LittleEndian>(key.len() as u16)?;
    w.write_all(key.as_ref())?;
    match value {
        Some(v) => {
            w.write_all(&[TAG_VALUE])?;
            w.write_u32::<LittleEndian>(v.len() as u32)?;
            w.write_all(v.as_ref())?;
        }
        None => {
            w.write_all(&[TAG_TOMBSTONE])?;
        }
    }
    Ok(())
}

/// Decode a single entry from a reader.
fn decode_entry<R: Read>(r: &mut R) -> Result<(Key, Option<Value>)> {
    let key_len = r.read_u16::<LittleEndian>()? as usize;
    let mut key_buf = vec![0u8; key_len];
    r.read_exact(&mut key_buf)?;

    let tag = r.read_u8()?;
    let value = if tag == TAG_VALUE {
        let value_len = r.read_u32::<LittleEndian>()? as usize;
        let mut value_buf = vec![0u8; value_len];
        r.read_exact(&mut value_buf)?;
        Some(Value::new(value_buf))
    } else {
        None
    };

    Ok((Key::new(key_buf), value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_page_roundtrip() {
        let page = Page {
            entries: vec![
                (Key::from([1, 2]), Some(Value::from([10, 20]))),
                (Key::from([3, 4]), Some(Value::from([30, 40]))),
                (Key::from([5, 6]), None), // tombstone
            ],
        };

        let encoded = page.encode(4096).unwrap();
        assert_eq!(encoded.len(), 4096);

        let decoded = Page::decode(&encoded).unwrap();
        assert_eq!(decoded.entries.len(), 3);
        assert_eq!(decoded.entries[0].0.as_ref(), &[1, 2]);
        assert_eq!(decoded.entries[0].1.as_ref().unwrap().as_ref(), &[10, 20]);
        assert_eq!(decoded.entries[2].0.as_ref(), &[5, 6]);
        assert!(decoded.entries[2].1.is_none());
    }

    #[test]
    fn test_page_crc_corruption() {
        let page = Page {
            entries: vec![(Key::from([1]), Some(Value::from([2])))],
        };
        let mut encoded = page.encode(4096).unwrap();
        // Corrupt a data byte
        encoded[PAGE_HEADER_SIZE] ^= 0xFF;
        assert!(Page::decode(&encoded).is_err());
    }

    #[test]
    fn test_page_binary_search() {
        let page = Page {
            entries: vec![
                (Key::from([1]), Some(Value::from([10]))),
                (Key::from([3]), Some(Value::from([30]))),
                (Key::from([5]), None),
                (Key::from([7]), Some(Value::from([70]))),
            ],
        };

        // Found value
        let result = page.search(&Key::from([3]));
        assert_eq!(result.unwrap().as_ref().unwrap().as_ref(), &[30]);

        // Found tombstone
        let result = page.search(&Key::from([5]));
        assert!(result.unwrap().is_none());

        // Not found
        assert!(page.search(&Key::from([4])).is_none());
    }

    #[test]
    fn test_entry_size() {
        // key_len(2) + key(3) + tag(1) + value_len(4) + value(5) = 15
        let key = Key::from([1, 2, 3]);
        let val = Some(Value::from([1, 2, 3, 4, 5]));
        assert_eq!(Page::entry_size(&key, &val), 15);

        // Tombstone: key_len(2) + key(3) + tag(1) = 6
        let tomb = None;
        assert_eq!(Page::entry_size(&key, &tomb), 6);
    }

    #[test]
    fn test_empty_page() {
        let page = Page { entries: vec![] };
        let encoded = page.encode(4096).unwrap();
        let decoded = Page::decode(&encoded).unwrap();
        assert!(decoded.entries.is_empty());
    }

    #[test]
    fn test_oversized_entry() {
        // A single entry larger than the normal page data capacity
        let key = Key::from([1, 2, 3]);
        let large_value = Value::from(vec![42u8; 8000]); // 8KB value
        let page = Page {
            entries: vec![(key.clone(), Some(large_value.clone()))],
        };

        // With 4096-byte pages, this entry won't fit in one page but
        // the encoder should produce an oversized page
        let encoded = page.encode(4096).unwrap();
        assert!(encoded.len() > 4096); // Should be rounded up
        assert_eq!(encoded.len() % 4096, 0); // Aligned to page_size

        let decoded = Page::decode(&encoded).unwrap();
        assert_eq!(decoded.entries.len(), 1);
        assert_eq!(decoded.entries[0].0.as_ref(), &[1, 2, 3]);
        assert_eq!(decoded.entries[0].1.as_ref().unwrap().len(), 8000);
    }

    #[test]
    fn test_very_large_entry() {
        // Simulate a Cardano transaction output with a large inline datum
        // (~13KB, matching what was seen on preview testnet)
        let key = Key::from([0xAB; 36]); // 36-byte Cardano UTxO key
        let large_value = Value::from(vec![0xCD; 13_300]); // ~13.3KB value
        let page = Page {
            entries: vec![(key.clone(), Some(large_value.clone()))],
        };

        let encoded = page.encode(4096).unwrap();
        let decoded = Page::decode(&encoded).unwrap();
        assert_eq!(decoded.entries.len(), 1);
        assert_eq!(decoded.entries[0].1.as_ref().unwrap().len(), 13_300);
    }
}
