//! File-based exclusive session lock.
//!
//! Prevents multiple processes from opening the same LSM database
//! simultaneously, which would cause data corruption.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use fs2::FileExt;

use crate::error::{Error, Result};

/// An exclusive lock on a database directory.
///
/// The lock is held for the lifetime of this struct. Dropping it releases
/// the lock, allowing other processes to open the database.
pub struct SessionLock {
    _file: File,
    path: PathBuf,
}

#[allow(dead_code)]
impl SessionLock {
    /// Acquire an exclusive lock on the database at the given path.
    ///
    /// Creates a `lock` file in the directory and acquires an exclusive flock on it.
    /// Returns an error if another process already holds the lock.
    pub fn acquire(db_path: &Path) -> Result<Self> {
        let lock_path = db_path.join("lock");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)?;

        file.try_lock_exclusive()
            .map_err(|e| Error::DatabaseLocked(format!("{}: {e}", lock_path.display())))?;

        Ok(SessionLock {
            _file: file,
            path: lock_path,
        })
    }

    /// Get the path to the lock file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

// Lock is released when the File is dropped (flock is released on close).

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_acquire_lock() {
        let dir = tempfile::tempdir().unwrap();
        let lock = SessionLock::acquire(dir.path()).unwrap();
        assert!(lock.path().exists());
    }

    #[test]
    fn test_double_lock_fails() {
        let dir = tempfile::tempdir().unwrap();
        let _lock1 = SessionLock::acquire(dir.path()).unwrap();
        let result = SessionLock::acquire(dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn test_lock_released_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        {
            let _lock = SessionLock::acquire(dir.path()).unwrap();
        }
        // After drop, a new lock should succeed
        let _lock2 = SessionLock::acquire(dir.path()).unwrap();
    }
}
