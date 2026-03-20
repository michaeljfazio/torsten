//! Disk-space querying for the Resources panel.
//!
//! Provides [`DiskStats`] which captures the total, used, and free bytes on
//! the filesystem that contains the node's database directory.  The query is
//! performed synchronously via the OS `statvfs` (Unix) or
//! `GetDiskFreeSpaceEx` (Windows) syscalls — both complete in microseconds
//! and are safe to call on the render thread.
//!
//! When the path does not exist or the syscall fails (e.g. the database
//! directory has not yet been created) `None` is returned so the caller can
//! suppress the panel row cleanly rather than displaying misleading zeros.

/// Point-in-time snapshot of disk space for a single filesystem.
#[derive(Debug, Clone, Copy)]
pub struct DiskStats {
    /// Total capacity of the filesystem in bytes.
    pub total_bytes: u64,
    /// Number of bytes currently in use (total − free).
    pub used_bytes: u64,
    /// Number of bytes available to unprivileged processes.
    pub free_bytes: u64,
}

impl DiskStats {
    /// Usage as a fraction in `[0.0, 1.0]`.
    ///
    /// Returns 0.0 when `total_bytes` is zero to avoid division by zero.
    pub fn usage_ratio(&self) -> f64 {
        if self.total_bytes == 0 {
            0.0
        } else {
            self.used_bytes as f64 / self.total_bytes as f64
        }
    }
}

/// Query the filesystem that contains `path` and return a [`DiskStats`] snapshot.
///
/// Returns `None` when:
/// - `path` is empty (no `--db-path` was supplied).
/// - The syscall fails (permission denied, path does not exist, etc.).
pub fn query(path: &str) -> Option<DiskStats> {
    if path.is_empty() {
        return None;
    }
    query_impl(path)
}

// ---------------------------------------------------------------------------
// Unix implementation — statvfs(3)
// ---------------------------------------------------------------------------

#[cfg(unix)]
fn query_impl(path: &str) -> Option<DiskStats> {
    use std::ffi::CString;

    // Convert the path to a C string for the statvfs(2) syscall.
    let c_path = CString::new(path).ok()?;

    // SAFETY: `c_path` is a valid NUL-terminated path; `stat` is
    // zero-initialised before being passed to the kernel.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    let ret = unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) };
    if ret != 0 {
        return None;
    }

    // `f_blocks` / `f_bfree` / `f_bavail` are measured in `f_frsize`-byte
    // fragments.  We use `f_bavail` (blocks available to unprivileged
    // processes) for "free" so that the displayed value matches what `df`
    // reports for unprivileged users.
    let block_size = stat.f_frsize as u64;
    let total = stat.f_blocks as u64 * block_size;
    let free = stat.f_bavail as u64 * block_size;
    let used = total.saturating_sub(stat.f_bfree as u64 * block_size);

    Some(DiskStats {
        total_bytes: total,
        used_bytes: used,
        free_bytes: free,
    })
}

// ---------------------------------------------------------------------------
// Windows implementation — GetDiskFreeSpaceExA
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn query_impl(path: &str) -> Option<DiskStats> {
    use std::ffi::CString;

    let c_path = CString::new(path).ok()?;

    let mut free_bytes_caller: u64 = 0;
    let mut total_bytes: u64 = 0;
    let mut free_bytes_total: u64 = 0;

    // SAFETY: all pointers point to local u64 variables; the path is a valid
    // NUL-terminated string.
    let ok = unsafe {
        libc::GetDiskFreeSpaceExA(
            c_path.as_ptr(),
            &mut free_bytes_caller as *mut u64 as *mut _,
            &mut total_bytes as *mut u64 as *mut _,
            &mut free_bytes_total as *mut u64 as *mut _,
        )
    };
    if ok == 0 {
        return None;
    }

    Some(DiskStats {
        total_bytes,
        used_bytes: total_bytes.saturating_sub(free_bytes_total),
        free_bytes: free_bytes_caller,
    })
}

// ---------------------------------------------------------------------------
// Fallback stub for unsupported platforms
// ---------------------------------------------------------------------------

#[cfg(not(any(unix, windows)))]
fn query_impl(_path: &str) -> Option<DiskStats> {
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Empty path must always return None regardless of platform.
    #[test]
    fn test_empty_path_returns_none() {
        assert!(query("").is_none());
    }

    /// A non-existent path must return None (syscall fails gracefully).
    #[test]
    fn test_nonexistent_path_returns_none() {
        assert!(query("/this/path/does/not/exist/at/all/abc123").is_none());
    }

    /// The current working directory must always be on a readable filesystem.
    #[test]
    fn test_current_dir_returns_stats() {
        let stats = query(".");
        assert!(stats.is_some(), "expected disk stats for '.'");
        let s = stats.unwrap();
        assert!(s.total_bytes > 0, "total_bytes should be > 0");
        assert!(s.free_bytes <= s.total_bytes, "free should not exceed total");
        assert!(s.usage_ratio() <= 1.0, "usage ratio should not exceed 1.0");
    }

    /// `usage_ratio` must not panic when total_bytes is zero.
    #[test]
    fn test_usage_ratio_zero_total() {
        let s = DiskStats {
            total_bytes: 0,
            used_bytes: 0,
            free_bytes: 0,
        };
        assert_eq!(s.usage_ratio(), 0.0);
    }

    /// usage_ratio reflects used / total correctly.
    #[test]
    fn test_usage_ratio_partial() {
        let s = DiskStats {
            total_bytes: 1000,
            used_bytes: 333,
            free_bytes: 667,
        };
        // 333 / 1000 = 0.333; allow a tiny floating-point epsilon.
        let ratio = s.usage_ratio();
        assert!((ratio - 0.333).abs() < 1e-9);
    }
}
