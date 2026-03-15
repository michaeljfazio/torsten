//! Persistent snapshots via hard links and metadata files.
//!
//! A snapshot captures the current state of the LSM tree by:
//! 1. Hard-linking all active run files into a snapshot directory
//! 2. Writing a metadata file that records which runs and their level assignments
//!
//! Snapshots are zero-copy (hard links share the same data on disk). Runs that
//! are still referenced by a snapshot are not deleted during compaction.

use std::fs;
use std::path::{Path, PathBuf};

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use std::io::{Cursor, Write};

use crate::error::{Error, Result};
use crate::run::{run_bloom_path, run_data_path, run_index_path};

/// Metadata for a snapshot.
#[derive(Debug, Clone)]
pub struct SnapshotMetadata {
    /// Human-readable label.
    pub label: String,
    /// Run IDs included in this snapshot, with their level assignments.
    /// Vec of (level_number, run_id).
    pub runs: Vec<(usize, u64)>,
    /// The next_run_id at snapshot time (for restoring the counter).
    pub next_run_id: u64,
}

impl SnapshotMetadata {
    /// Serialize to bytes.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut buf = Vec::new();

        // Label: length-prefixed string
        let label_bytes = self.label.as_bytes();
        buf.write_u16::<LittleEndian>(label_bytes.len() as u16)?;
        buf.write_all(label_bytes)?;

        // next_run_id
        buf.write_u64::<LittleEndian>(self.next_run_id)?;

        // Runs: count then (level, run_id) pairs
        buf.write_u32::<LittleEndian>(self.runs.len() as u32)?;
        for &(level, run_id) in &self.runs {
            buf.write_u32::<LittleEndian>(level as u32)?;
            buf.write_u64::<LittleEndian>(run_id)?;
        }

        Ok(buf)
    }

    /// Deserialize from bytes.
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        let mut cursor = Cursor::new(data);

        let label_len = cursor.read_u16::<LittleEndian>()? as usize;
        let mut label_buf = vec![0u8; label_len];
        std::io::Read::read_exact(&mut cursor, &mut label_buf)?;
        let label = String::from_utf8(label_buf)
            .map_err(|e| Error::Manifest(format!("invalid snapshot label: {e}")))?;

        let next_run_id = cursor.read_u64::<LittleEndian>()?;

        let run_count = cursor.read_u32::<LittleEndian>()? as usize;
        let mut runs = Vec::with_capacity(run_count);
        for _ in 0..run_count {
            let level = cursor.read_u32::<LittleEndian>()? as usize;
            let run_id = cursor.read_u64::<LittleEndian>()?;
            runs.push((level, run_id));
        }

        Ok(SnapshotMetadata {
            label,
            runs,
            next_run_id,
        })
    }
}

/// Save a snapshot by hard-linking run files and writing metadata.
pub fn save_snapshot(
    active_dir: &Path,
    snapshots_dir: &Path,
    name: &str,
    label: &str,
    metadata: &SnapshotMetadata,
) -> Result<()> {
    let snap_dir = snapshots_dir.join(name);
    fs::create_dir_all(&snap_dir)?;

    // Hard-link all run files
    for &(_, run_id) in &metadata.runs {
        hard_link_run(active_dir, &snap_dir, run_id)?;
    }

    // Write metadata
    let meta_path = snap_dir.join("metadata.bin");
    let meta_bytes = metadata.to_bytes()?;
    fs::write(meta_path, meta_bytes)?;

    // Write the label as a convenience file
    fs::write(snap_dir.join("label.txt"), label)?;

    Ok(())
}

/// Load snapshot metadata.
pub fn load_snapshot_metadata(snapshots_dir: &Path, name: &str) -> Result<SnapshotMetadata> {
    let snap_dir = snapshots_dir.join(name);
    if !snap_dir.exists() {
        return Err(Error::SnapshotNotFound(name.to_string()));
    }

    let meta_path = snap_dir.join("metadata.bin");
    let meta_bytes = fs::read(&meta_path)?;
    SnapshotMetadata::from_bytes(&meta_bytes)
}

/// Delete a snapshot directory and all its contents.
pub fn delete_snapshot(snapshots_dir: &Path, name: &str) -> Result<()> {
    let snap_dir = snapshots_dir.join(name);
    if snap_dir.exists() {
        fs::remove_dir_all(&snap_dir)?;
    }
    Ok(())
}

/// Open a snapshot by copying its run files to the active directory.
///
/// Returns the snapshot metadata for level reconstruction.
pub fn open_snapshot(
    snapshots_dir: &Path,
    active_dir: &Path,
    name: &str,
) -> Result<SnapshotMetadata> {
    let snap_dir = snapshots_dir.join(name);
    if !snap_dir.exists() {
        return Err(Error::SnapshotNotFound(name.to_string()));
    }

    let metadata = load_snapshot_metadata(snapshots_dir, name)?;

    // Ensure active dir exists
    fs::create_dir_all(active_dir)?;

    // Hard-link (or copy) run files from snapshot to active
    for &(_, run_id) in &metadata.runs {
        hard_link_run(&snap_dir, active_dir, run_id)?;
    }

    Ok(metadata)
}

/// Hard-link the three files of a run from src_dir to dst_dir.
fn hard_link_run(src_dir: &Path, dst_dir: &Path, run_id: u64) -> Result<()> {
    let files = [
        (
            run_data_path(src_dir, run_id),
            run_data_path(dst_dir, run_id),
        ),
        (
            run_bloom_path(src_dir, run_id),
            run_bloom_path(dst_dir, run_id),
        ),
        (
            run_index_path(src_dir, run_id),
            run_index_path(dst_dir, run_id),
        ),
    ];

    for (src, dst) in &files {
        if src.exists() && !dst.exists() {
            // Try hard link first, fall back to copy (cross-device)
            if fs::hard_link(src, dst).is_err() {
                fs::copy(src, dst)?;
            }
        }
    }

    Ok(())
}

/// Get the snapshot directory path.
pub fn snapshot_dir(db_path: &Path) -> PathBuf {
    db_path.join("snapshots")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snapshot_metadata_roundtrip() {
        let meta = SnapshotMetadata {
            label: "test-snap".to_string(),
            runs: vec![(0, 1), (0, 2), (1, 3)],
            next_run_id: 4,
        };

        let bytes = meta.to_bytes().unwrap();
        let restored = SnapshotMetadata::from_bytes(&bytes).unwrap();
        assert_eq!(restored.label, "test-snap");
        assert_eq!(restored.runs, vec![(0, 1), (0, 2), (1, 3)]);
        assert_eq!(restored.next_run_id, 4);
    }

    #[test]
    fn test_save_and_load_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let active_dir = dir.path().join("active");
        let snaps_dir = dir.path().join("snapshots");
        fs::create_dir_all(&active_dir).unwrap();

        // Create dummy run files
        fs::write(run_data_path(&active_dir, 1), b"data1").unwrap();
        fs::write(run_bloom_path(&active_dir, 1), b"bloom1").unwrap();
        fs::write(run_index_path(&active_dir, 1), b"index1").unwrap();

        let meta = SnapshotMetadata {
            label: "test".to_string(),
            runs: vec![(0, 1)],
            next_run_id: 2,
        };

        save_snapshot(&active_dir, &snaps_dir, "snap1", "test", &meta).unwrap();

        let loaded = load_snapshot_metadata(&snaps_dir, "snap1").unwrap();
        assert_eq!(loaded.label, "test");
        assert_eq!(loaded.runs, vec![(0, 1)]);

        // Verify hard-linked files exist
        let snap_data = snaps_dir.join("snap1").join("run-000001.data");
        assert!(snap_data.exists());
    }

    #[test]
    fn test_delete_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let snaps_dir = dir.path().join("snapshots");
        let snap_dir = snaps_dir.join("snap1");
        fs::create_dir_all(&snap_dir).unwrap();
        fs::write(snap_dir.join("metadata.bin"), b"test").unwrap();

        delete_snapshot(&snaps_dir, "snap1").unwrap();
        assert!(!snap_dir.exists());
    }

    #[test]
    fn test_snapshot_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let snaps_dir = dir.path().join("snapshots");
        fs::create_dir_all(&snaps_dir).unwrap();

        let result = load_snapshot_metadata(&snaps_dir, "nonexistent");
        assert!(result.is_err());
    }
}
