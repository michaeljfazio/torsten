//! Abstraction over chunk-file reading backends.
//!
//! Two backends are provided:
//!
//! - **memmap2** (default) — memory-maps the chunk file and copies the
//!   requested byte range into a `Vec<u8>`.  Works on all platforms.
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
    use std::fs;

    /// Chunk reader backed by `memmap2`.
    pub(crate) struct MmapChunkReader;

    impl ChunkReader for MmapChunkReader {
        fn read_range(&self, path: &Path, offset: u64, len: usize) -> Option<Vec<u8>> {
            let file = fs::File::open(path).ok()?;
            let mmap = unsafe { Mmap::map(&file).ok()? };

            let start = offset as usize;
            let end = start.checked_add(len)?;
            if end > mmap.len() || start >= end {
                return None;
            }

            Some(mmap[start..end].to_vec())
        }

        fn read_ranges(&self, path: &Path, ranges: &[(u64, usize)]) -> Vec<Option<Vec<u8>>> {
            // Open + mmap once, then serve all ranges from the mapping.
            let file = match std::fs::File::open(path) {
                Ok(f) => f,
                Err(_) => return ranges.iter().map(|_| None).collect(),
            };
            let mmap = match unsafe { Mmap::map(&file) } {
                Ok(m) => m,
                Err(_) => return ranges.iter().map(|_| None).collect(),
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
        MmapChunkReader
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
}
