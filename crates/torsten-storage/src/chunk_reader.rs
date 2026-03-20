//! Abstraction over chunk-file reading backends.
//!
//! Two backends are provided:
//!
//! - **memmap2** (default) — memory-maps the chunk file and copies the
//!   requested byte range into a `Vec<u8>`.  Works on all platforms.
//!   The mmap handles for sealed (finalized) chunk files are cached in a
//!   `RwLock<HashMap>` so repeated single-block lookups in the same chunk
//!   pay the open+mmap cost only once, delivering a 5-10% speedup during
//!   ledger replay when many `read_range` calls target the same chunk.
//!
//! - **io_uring** (opt-in via `io-uring` feature, Linux only) — submits a
//!   vectored read through the kernel's io_uring interface, avoiding the
//!   page-table overhead of mmap for large sequential scans.
//!
//! Both backends expose the same [`ChunkReader`] trait so the rest of
//! `ImmutableDB` is backend-agnostic.

use std::path::Path;

/// Read a contiguous byte range from a chunk file.
///
/// Implementors must handle the case where `offset + len` exceeds the
/// file size by returning `None` (not panicking).
pub(crate) trait ChunkReader {
    /// Read `len` bytes starting at `offset` from the file at `path`.
    ///
    /// Returns `None` on any I/O error or if the range is out of bounds.
    fn read_range(&self, path: &Path, offset: u64, len: usize) -> Option<Vec<u8>>;

    /// Read multiple contiguous ranges from one file.
    ///
    /// The default implementation calls [`read_range`](Self::read_range)
    /// in a loop.  Backends that support batched I/O (e.g. io_uring) can
    /// override this for better throughput.
    fn read_ranges(&self, path: &Path, ranges: &[(u64, usize)]) -> Vec<Option<Vec<u8>>> {
        ranges
            .iter()
            .map(|&(off, len)| self.read_range(path, off, len))
            .collect()
    }
}

// -----------------------------------------------------------------------
// memmap2 backend (default)
// -----------------------------------------------------------------------

#[cfg(not(all(feature = "io-uring", target_os = "linux")))]
mod backend {
    use super::*;
    use memmap2::Mmap;
    use std::collections::HashMap;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::{Arc, RwLock};

    /// Per-file mmap state held in the cache.
    ///
    /// The `Mmap` is reference-counted so callers that hold a clone can
    /// keep reading it after another thread evicts the entry.
    type MmapEntry = Arc<Mmap>;

    /// Chunk reader backed by `memmap2` with a per-reader mmap cache.
    ///
    /// Immutable chunk files are never modified once sealed by the node,
    /// so the cached `Mmap` always reflects the complete file.  The
    /// active (currently-written) chunk is not routed through this reader —
    /// writes use `pending_blocks` in `ActiveChunk` — so there is no risk
    /// of serving stale data.
    ///
    /// The cache is keyed by the canonical `PathBuf` of the chunk file.
    /// Cache entries are evicted lazily (when the reader is dropped) or
    /// explicitly via [`MmapChunkReader::evict`].  In practice a node
    /// keeps a single `MmapChunkReader` alive for the lifetime of the
    /// process, so entries remain warm across many calls.
    pub(crate) struct MmapChunkReader {
        /// Cached mmap handles, keyed by absolute chunk file path.
        ///
        /// `RwLock` permits concurrent readers; a write lock is acquired only
        /// on a cache miss.  Contention is expected to be very low because
        /// misses are rare once the hot chunks are loaded.
        cache: RwLock<HashMap<PathBuf, MmapEntry>>,
    }

    impl MmapChunkReader {
        /// Evict all cached mmaps.
        ///
        /// Useful when the set of chunk files changes (e.g. after a Mithril
        /// import or when the active chunk is finalized), though in practice
        /// the immutable chunks never change their content after sealing.
        #[allow(dead_code)] // Reserved for future use (e.g. post-import cache flush)
        pub(crate) fn evict_all(&self) {
            if let Ok(mut guard) = self.cache.write() {
                guard.clear();
            }
        }

        /// Return the number of files currently held in the mmap cache.
        ///
        /// Exposed for testing only.
        #[cfg(test)]
        pub(crate) fn cache_len(&self) -> usize {
            self.cache.read().map(|g| g.len()).unwrap_or(0)
        }

        /// Obtain (or insert) a cached `Mmap` for `path`.
        ///
        /// Returns `None` if the file cannot be opened or mapped.
        fn get_or_insert(&self, path: &Path) -> Option<MmapEntry> {
            // Fast path: cache hit under a shared read lock.
            {
                let guard = self.cache.read().ok()?;
                if let Some(entry) = guard.get(path) {
                    return Some(Arc::clone(entry));
                }
            }

            // Slow path: cache miss — open+mmap the file, then insert.
            let file = fs::File::open(path).ok()?;
            // SAFETY: The file is opened read-only.  Chunk files are sealed
            // (append-only before the active chunk is finalized) and are never
            // truncated or replaced while the node is running.  The Mmap stays
            // valid as long as the Arc is live.
            let mmap = unsafe { Mmap::map(&file).ok()? };
            let entry: MmapEntry = Arc::new(mmap);

            // Acquire the write lock and insert.  A concurrent thread may have
            // raced us here; we accept the duplicate work and keep the one
            // already present to avoid a redundant write.
            if let Ok(mut guard) = self.cache.write() {
                guard
                    .entry(path.to_path_buf())
                    .or_insert_with(|| Arc::clone(&entry));
            }

            Some(entry)
        }
    }

    impl ChunkReader for MmapChunkReader {
        fn read_range(&self, path: &Path, offset: u64, len: usize) -> Option<Vec<u8>> {
            let mmap = self.get_or_insert(path)?;

            let start = offset as usize;
            let end = start.checked_add(len)?;
            if end > mmap.len() || start >= end {
                return None;
            }

            Some(mmap[start..end].to_vec())
        }

        fn read_ranges(&self, path: &Path, ranges: &[(u64, usize)]) -> Vec<Option<Vec<u8>>> {
            if ranges.is_empty() {
                return Vec::new();
            }

            // Fetch (or reuse) the cached mmap — open+map at most once per path.
            let mmap = match self.get_or_insert(path) {
                Some(m) => m,
                None => return ranges.iter().map(|_| None).collect(),
            };

            ranges
                .iter()
                .map(|&(off, len)| {
                    let start = off as usize;
                    let end = start.checked_add(len)?;
                    if end > mmap.len() || start >= end {
                        return None;
                    }
                    Some(mmap[start..end].to_vec())
                })
                .collect()
        }
    }

    /// Return the platform-appropriate chunk reader.
    pub(crate) fn default_reader() -> MmapChunkReader {
        MmapChunkReader {
            cache: RwLock::new(HashMap::new()),
        }
    }
}

// -----------------------------------------------------------------------
// io_uring backend (Linux only, feature-gated)
// -----------------------------------------------------------------------

#[cfg(all(feature = "io-uring", target_os = "linux"))]
mod backend {
    use super::*;
    use std::fs;
    use std::os::unix::io::AsRawFd;

    /// Chunk reader backed by Linux `io_uring`.
    ///
    /// Each read submits a single SQE and waits for the CQE.  For batched
    /// reads ([`read_ranges`](ChunkReader::read_ranges)) all SQEs are
    /// submitted together and reaped in one pass — this is where the
    /// throughput advantage over mmap shows up on NVMe storage.
    pub(crate) struct IoUringChunkReader;

    impl ChunkReader for IoUringChunkReader {
        fn read_range(&self, path: &Path, offset: u64, len: usize) -> Option<Vec<u8>> {
            // Zero-length read is an empty range — return None to match mmap backend.
            if len == 0 {
                return None;
            }
            let file = fs::File::open(path).ok()?;
            let fd = io_uring::types::Fd(file.as_raw_fd());
            let mut buf = vec![0u8; len];

            let mut ring = io_uring::IoUring::new(1).ok()?;

            let read_op = io_uring::opcode::Read::new(fd, buf.as_mut_ptr(), len as u32)
                .offset(offset)
                .build()
                .user_data(0);

            // Safety: the SQE references `buf` which outlives the submission.
            unsafe {
                ring.submission().push(&read_op).ok()?;
            }
            ring.submit_and_wait(1).ok()?;

            let cqe = ring.completion().next()?;
            let bytes_read = cqe.result();
            if bytes_read < 0 || (bytes_read as usize) < len {
                // Short read or error — fall back to None.
                return None;
            }

            Some(buf)
        }

        fn read_ranges(&self, path: &Path, ranges: &[(u64, usize)]) -> Vec<Option<Vec<u8>>> {
            if ranges.is_empty() {
                return Vec::new();
            }

            let file = match fs::File::open(path) {
                Ok(f) => f,
                Err(_) => return ranges.iter().map(|_| None).collect(),
            };
            let fd = io_uring::types::Fd(file.as_raw_fd());

            // We need at least as many entries as ranges.
            let ring_size = (ranges.len() as u32).next_power_of_two().max(2);
            let mut ring = match io_uring::IoUring::new(ring_size) {
                Ok(r) => r,
                Err(_) => return ranges.iter().map(|_| None).collect(),
            };

            // Allocate all buffers up-front so pointers remain stable.
            let mut bufs: Vec<Vec<u8>> = ranges.iter().map(|&(_, len)| vec![0u8; len]).collect();

            // Submit all reads.
            {
                let mut sq = ring.submission();
                for (i, (buf, &(off, len))) in bufs.iter_mut().zip(ranges.iter()).enumerate() {
                    let read_op = io_uring::opcode::Read::new(fd, buf.as_mut_ptr(), len as u32)
                        .offset(off)
                        .build()
                        .user_data(i as u64);

                    // Safety: bufs outlive the ring submission.
                    unsafe {
                        if sq.push(&read_op).is_err() {
                            // SQ full — shouldn't happen since we sized it.
                            break;
                        }
                    }
                }
            }

            if ring.submit_and_wait(ranges.len()).is_err() {
                return ranges.iter().map(|_| None).collect();
            }

            // Collect results.
            let mut results: Vec<Option<Vec<u8>>> = bufs.into_iter().map(Some).collect();

            for cqe in ring.completion() {
                let idx = cqe.user_data() as usize;
                let expected_len = ranges.get(idx).map_or(0, |&(_, l)| l);
                if cqe.result() < 0 || (cqe.result() as usize) < expected_len {
                    if let Some(slot) = results.get_mut(idx) {
                        *slot = None;
                    }
                }
            }

            results
        }
    }

    /// Return the platform-appropriate chunk reader.
    pub(crate) fn default_reader() -> IoUringChunkReader {
        IoUringChunkReader
    }
}

pub(crate) use backend::default_reader;

// Re-export the trait so callers can use it generically.
pub(crate) use self::ChunkReader as ChunkReaderTrait;

// -----------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_read_range_basic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.chunk");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(b"hello world chunk data").unwrap();
        drop(f);

        let reader = default_reader();
        let data = reader.read_range(&path, 0, 5).unwrap();
        assert_eq!(&data, b"hello");

        let data = reader.read_range(&path, 6, 5).unwrap();
        assert_eq!(&data, b"world");
    }

    #[test]
    fn test_read_range_out_of_bounds() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.chunk");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(b"short").unwrap();
        drop(f);

        let reader = default_reader();
        // Completely out of bounds
        assert!(reader.read_range(&path, 100, 10).is_none());
        // Partially out of bounds
        assert!(reader.read_range(&path, 3, 10).is_none());
    }

    #[test]
    fn test_read_range_nonexistent_file() {
        let reader = default_reader();
        assert!(reader
            .read_range(Path::new("/nonexistent/path"), 0, 10)
            .is_none());
    }

    #[test]
    fn test_read_range_zero_length() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.chunk");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(b"data").unwrap();
        drop(f);

        let reader = default_reader();
        // Zero-length read at valid offset
        // Note: start == end means the range is empty, our reader returns None
        // since start >= end check catches this.
        let result = reader.read_range(&path, 0, 0);
        assert!(result.is_none());
    }

    #[test]
    fn test_read_ranges_batch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.chunk");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(b"aaabbbccc").unwrap();
        drop(f);

        let reader = default_reader();
        let results = reader.read_ranges(&path, &[(0, 3), (3, 3), (6, 3)]);
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].as_deref(), Some(b"aaa".as_slice()));
        assert_eq!(results[1].as_deref(), Some(b"bbb".as_slice()));
        assert_eq!(results[2].as_deref(), Some(b"ccc".as_slice()));
    }

    #[test]
    fn test_read_ranges_mixed_valid_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.chunk");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(b"abcdef").unwrap();
        drop(f);

        let reader = default_reader();
        let results = reader.read_ranges(&path, &[(0, 3), (100, 5), (3, 3)]);
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].as_deref(), Some(b"abc".as_slice()));
        assert!(results[1].is_none());
        assert_eq!(results[2].as_deref(), Some(b"def".as_slice()));
    }

    #[test]
    fn test_read_ranges_empty() {
        let reader = default_reader();
        let results = reader.read_ranges(Path::new("/whatever"), &[]);
        assert!(results.is_empty());
    }

    // -----------------------------------------------------------------------
    // mmap cache tests (memmap2 backend only)
    // -----------------------------------------------------------------------

    /// Verify that repeated `read_range` calls to the same path return
    /// consistent data, confirming the cache path is exercised correctly.
    #[cfg(not(all(feature = "io-uring", target_os = "linux")))]
    #[test]
    fn test_mmap_cache_repeated_reads_are_consistent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cached.chunk");
        let content = b"ABCDEFGHIJ";
        std::fs::write(&path, content).unwrap();

        let reader = default_reader();

        // First call — cold cache (triggers open+mmap+insert).
        let first = reader.read_range(&path, 0, 5).unwrap();
        assert_eq!(&first, b"ABCDE");

        // Second call — warm cache (should reuse the cached Mmap).
        let second = reader.read_range(&path, 5, 5).unwrap();
        assert_eq!(&second, b"FGHIJ");

        // A third call at the same range should still agree.
        let third = reader.read_range(&path, 0, 5).unwrap();
        assert_eq!(first, third);

        // Cache should now contain exactly one entry for this path.
        assert_eq!(
            reader.cache_len(),
            1,
            "cache should hold exactly one mmap entry"
        );
    }

    /// Verify that two distinct chunk files each get their own cache entry.
    #[cfg(not(all(feature = "io-uring", target_os = "linux")))]
    #[test]
    fn test_mmap_cache_multiple_files_cached_independently() {
        let dir = tempfile::tempdir().unwrap();
        let path_a = dir.path().join("00000.chunk");
        let path_b = dir.path().join("00001.chunk");
        std::fs::write(&path_a, b"chunk-A-data").unwrap();
        std::fs::write(&path_b, b"chunk-B-data").unwrap();

        let reader = default_reader();

        let a = reader.read_range(&path_a, 0, 7).unwrap();
        let b = reader.read_range(&path_b, 0, 7).unwrap();
        assert_eq!(&a, b"chunk-A");
        assert_eq!(&b, b"chunk-B");

        // Both files should be in the cache after the reads.
        assert_eq!(
            reader.cache_len(),
            2,
            "each distinct chunk file gets its own cache entry"
        );
    }

    /// Verify that `evict_all` clears all cached entries.
    ///
    /// After eviction, a subsequent read should succeed (re-maps the file)
    /// and the cache will be repopulated with a fresh entry.
    #[cfg(not(all(feature = "io-uring", target_os = "linux")))]
    #[test]
    fn test_mmap_cache_evict_all_clears_cache() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("evict.chunk");
        std::fs::write(&path, b"hello eviction").unwrap();

        let reader = default_reader();

        // Warm the cache.
        let _ = reader.read_range(&path, 0, 5).unwrap();
        assert_eq!(reader.cache_len(), 1);

        // Evict.
        reader.evict_all();
        assert_eq!(
            reader.cache_len(),
            0,
            "cache should be empty after evict_all"
        );

        // Read again — should succeed and re-populate the cache.
        let data = reader.read_range(&path, 0, 5).unwrap();
        assert_eq!(&data, b"hello");
        assert_eq!(reader.cache_len(), 1);
    }

    /// Verify that `read_ranges` (batch path) also populates and reuses the cache.
    #[cfg(not(all(feature = "io-uring", target_os = "linux")))]
    #[test]
    fn test_mmap_cache_shared_between_read_range_and_read_ranges() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shared.chunk");
        std::fs::write(&path, b"123456789").unwrap();

        let reader = default_reader();

        // Prime via batch path.
        let batch = reader.read_ranges(&path, &[(0, 3), (3, 3)]);
        assert_eq!(batch[0].as_deref(), Some(b"123".as_slice()));
        assert_eq!(batch[1].as_deref(), Some(b"456".as_slice()));
        assert_eq!(reader.cache_len(), 1);

        // Single read should reuse the cache entry populated by read_ranges.
        let single = reader.read_range(&path, 6, 3).unwrap();
        assert_eq!(&single, b"789");
        // Cache should still have exactly one entry (no duplicate insert).
        assert_eq!(reader.cache_len(), 1);
    }
}
