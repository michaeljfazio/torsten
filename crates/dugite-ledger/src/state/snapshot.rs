//! Ledger state snapshot persistence: save, load, and UTxO store attachment.
//!
//! # Snapshot format
//!
//! All snapshots use bincode serialization of [`LedgerStateSnapshot`](super::snapshot_format::LedgerStateSnapshot).  The on-disk
//! layout is:
//!
//! ```text
//! [4 bytes]  magic  "DUGT"
//! [1 byte]   version (SNAPSHOT_VERSION)
//! [32 bytes] blake2b-256 checksum of the payload
//! [N bytes]  bincode payload (LedgerState)
//! ```
//!
//! Two legacy formats are also supported for backwards compatibility:
//! - **Legacy with checksum** – `DUGT` + 32-byte checksum + data (no version byte)
//! - **Legacy raw** – plain bincode with no header at all
//!
//! # Version policy
//!
//! Increment `SNAPSHOT_VERSION` whenever the serialized `LedgerState` layout
//! changes (adding, removing, or reordering fields).  Because bincode is
//! positional and not self-describing, structural changes break existing
//! snapshots.  This is acceptable — snapshots are an optimization, not critical
//! data.  The node can always reconstruct state from the chain.

use super::{LedgerError, LedgerState, MAX_SNAPSHOT_SIZE};
use std::path::Path;
use tracing::{debug, info, warn};

/// `std::io::Write` adapter that forwards every byte to an inner writer
/// and simultaneously feeds it into a blake2b-256 hasher.
///
/// Used by [`LedgerState::save_snapshot`] so that the snapshot digest can
/// be computed while `bincode::serialize_into` streams the payload to a
/// buffered file, without first materialising the whole payload in a
/// `Vec<u8>`.
struct HashingWriter<'a, W: std::io::Write> {
    inner: &'a mut W,
    hasher: dugite_primitives::hash::Blake2b256Hasher,
    bytes_written: u64,
}

impl<'a, W: std::io::Write> std::io::Write for HashingWriter<'a, W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        // Only hash bytes that were actually accepted by the underlying
        // writer — partial writes must not double-hash the prefix.
        self.hasher.update(&buf[..n]);
        self.bytes_written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

impl LedgerState {
    /// Current snapshot format version.
    ///
    /// **Migration policy:** Increment this when the serialized `LedgerState`
    /// layout changes (adding, removing, or reordering fields). When bumped:
    ///
    /// 1. Add a `migrate_vN_to_vM()` function that transforms the old data.
    /// 2. Update `load_snapshot()` to dispatch to the migration chain.
    /// 3. If bincode-level migration is infeasible (field layout changed too much),
    ///    the old snapshot will fail to deserialize and the node re-syncs from chain.
    ///
    /// Since bincode is field-order-dependent and not self-describing, structural
    /// changes (new/removed/reordered fields) will cause deserialization failures
    /// for older snapshots.  This is acceptable — snapshots are an optimization,
    /// not critical data.  The node can always reconstruct state from the chain.
    ///
    /// Increment when `GovernanceState`/`LedgerState` fields change.
    /// Bincode is positional — any field addition/reorder breaks old snapshots.
    pub(crate) const SNAPSHOT_VERSION: u8 = 14;

    /// Save ledger state snapshot to disk using bincode serialization.
    ///
    /// Format: `[4-byte magic "DUGT"][1-byte version][32-byte blake2b checksum][bincode data]`
    ///
    /// The write is atomic: data is written to a `.tmp` file and then renamed
    /// over the final path so that a crash mid-write does not produce a partial
    /// or corrupt snapshot file.
    ///
    /// The payload is streamed through a two-pass write:
    ///
    ///   1. First pass: `bincode::serialize_into` writes directly to the
    ///      temp file via a buffered writer while simultaneously feeding
    ///      every byte into an incremental blake2b hasher. No full
    ///      in-memory `Vec<u8>` copy of the snapshot is ever produced.
    ///   2. Second pass: seek back to the header slot and overwrite the
    ///      placeholder checksum with the computed hash.
    ///
    /// Prior to #403 this function allocated a single contiguous `Vec<u8>`
    /// via `bincode::serialize(&snapshot)` before writing it out. At
    /// preview scale (~3M UTxOs in the in-memory backend) that allocation
    /// was multiple GB and contributed materially to the post-replay OOM
    /// that killed dugite-node on 32 GB hosts.
    pub fn save_snapshot(&self, path: &Path) -> Result<(), LedgerError> {
        use dugite_primitives::hash::Blake2b256Hasher;
        use std::io::{Seek, SeekFrom, Write};

        let tmp_path = path.with_extension("tmp");

        // Build the serde-friendly snapshot view. For the LSM backend this is
        // cheap (empty `utxo_set`); for the in-memory backend it still clones
        // the UTxO `HashMap`, but that cost is bounded (#403 follow-up work
        // can thread a borrowing wrapper through bincode if needed).
        let snapshot = super::snapshot_format::LedgerStateSnapshot::from(self);

        let file = std::fs::File::create(&tmp_path)
            .map_err(|e| LedgerError::EpochTransition(format!("Failed to create snapshot: {e}")))?;
        let mut writer = std::io::BufWriter::with_capacity(1 << 20, file);

        // Header: "DUGT" (4) + version (1) + blake2b placeholder (32).
        // We fill the checksum slot with zeros, remember its offset, and
        // rewrite it once the payload has been streamed and hashed.
        writer.write_all(b"DUGT").map_err(|e| {
            LedgerError::EpochTransition(format!("Failed to write snapshot header: {e}"))
        })?;
        writer.write_all(&[Self::SNAPSHOT_VERSION]).map_err(|e| {
            LedgerError::EpochTransition(format!("Failed to write snapshot version: {e}"))
        })?;
        const CHECKSUM_LEN: usize = 32;
        let checksum_offset: u64 = 4 + 1;
        writer.write_all(&[0u8; CHECKSUM_LEN]).map_err(|e| {
            LedgerError::EpochTransition(format!("Failed to write checksum placeholder: {e}"))
        })?;

        // Stream the payload through a `HashingWriter` that forwards writes
        // to the buffered file while updating a blake2b-256 hasher in-line.
        // `bincode::serialize_into` never materialises the whole payload in
        // memory — it walks the struct and emits bytes incrementally.
        let mut hashing_writer = HashingWriter {
            inner: &mut writer,
            hasher: Blake2b256Hasher::new(),
            bytes_written: 0,
        };
        bincode::serialize_into(&mut hashing_writer, &snapshot).map_err(|e| {
            LedgerError::EpochTransition(format!("Failed to serialize ledger state: {e}"))
        })?;
        let payload_bytes = hashing_writer.bytes_written;
        let checksum = hashing_writer.hasher.finalize();

        // Rewrite the checksum placeholder now that the payload hash is
        // known.  Flush afterwards so the file on disk is consistent before
        // the atomic rename.
        writer.flush().map_err(|e| {
            LedgerError::EpochTransition(format!("Failed to flush snapshot payload: {e}"))
        })?;
        writer
            .seek(SeekFrom::Start(checksum_offset))
            .map_err(|e| LedgerError::EpochTransition(format!("Failed to seek snapshot: {e}")))?;
        writer.write_all(checksum.as_bytes()).map_err(|e| {
            LedgerError::EpochTransition(format!("Failed to write snapshot checksum: {e}"))
        })?;
        writer
            .flush()
            .map_err(|e| LedgerError::EpochTransition(format!("Failed to flush snapshot: {e}")))?;
        drop(writer);

        let total_bytes = 4 + 1 + CHECKSUM_LEN as u64 + payload_bytes;

        std::fs::rename(&tmp_path, path)
            .map_err(|e| LedgerError::EpochTransition(format!("Failed to rename snapshot: {e}")))?;
        info!(
            "Snapshot     saved (epoch={}, {} UTxOs, {:.1} MB)",
            self.epoch.0,
            self.utxo.utxo_set.len(),
            total_bytes as f64 / 1_048_576.0,
        );
        Ok(())
    }

    /// Load ledger state snapshot from disk.
    ///
    /// Rejects snapshots larger than [`MAX_SNAPSHOT_SIZE`] to prevent OOM.
    ///
    /// Supports three formats:
    /// - **Versioned (v1+):** `DUGT` + version byte + 32-byte checksum + data
    /// - **Legacy with checksum:** `DUGT` + 32-byte checksum + data (no version byte)
    /// - **Legacy raw:** plain bincode without any header
    pub fn load_snapshot(path: &Path) -> Result<Self, LedgerError> {
        let raw = std::fs::read(path)
            .map_err(|e| LedgerError::EpochTransition(format!("Failed to read snapshot: {e}")))?;

        // Reject oversized snapshot files to prevent OOM from malicious data
        if raw.len() > MAX_SNAPSHOT_SIZE {
            return Err(LedgerError::EpochTransition(format!(
                "Snapshot size {} exceeds maximum allowed size {}",
                raw.len(),
                MAX_SNAPSHOT_SIZE
            )));
        }

        let data = if raw.len() >= 37 && &raw[..4] == b"DUGT" {
            let fifth_byte = raw[4];
            if fifth_byte > 0 && fifth_byte < 128 {
                // Versioned format: DUGT + version(1) + checksum(32) + data
                let version = fifth_byte;
                if version > Self::SNAPSHOT_VERSION {
                    return Err(LedgerError::EpochTransition(format!(
                        "Unsupported snapshot version {version} (max supported: {}). \
                         Delete the snapshot to re-sync from chain.",
                        Self::SNAPSHOT_VERSION,
                    )));
                }
                if version < Self::SNAPSHOT_VERSION {
                    // Older version — attempt migration chain. For bincode-based
                    // snapshots, structural changes make cross-version deserialization
                    // impossible. Log clearly so the user knows to re-sync.
                    warn!(
                        snapshot_version = version,
                        current_version = Self::SNAPSHOT_VERSION,
                        "Snapshot version mismatch — snapshot may fail to load. \
                         Delete the snapshot file to re-sync from chain if this fails."
                    );
                }
                debug!(version, "Loading versioned snapshot");
                let stored_checksum = &raw[5..37];
                let payload = &raw[37..];
                let computed = dugite_primitives::hash::blake2b_256(payload);
                if computed.as_bytes() != stored_checksum {
                    return Err(LedgerError::EpochTransition(
                        "Snapshot checksum mismatch — file may be corrupted".to_string(),
                    ));
                }
                payload
            } else {
                // Legacy format with checksum but no version byte:
                // DUGT + checksum(32) + data (5th byte is part of blake2b hash)
                warn!("Loading legacy snapshot (no version byte) with checksum verification");
                let stored_checksum = &raw[4..36];
                let payload = &raw[36..];
                let computed = dugite_primitives::hash::blake2b_256(payload);
                if computed.as_bytes() != stored_checksum {
                    return Err(LedgerError::EpochTransition(
                        "Snapshot checksum mismatch — file may be corrupted".to_string(),
                    ));
                }
                payload
            }
        } else if raw.len() >= 36 && &raw[..4] == b"DUGT" {
            // Legacy format with checksum (exactly 36 bytes of header, rare edge case)
            warn!("Loading legacy snapshot (no version byte) with checksum verification");
            let stored_checksum = &raw[4..36];
            let payload = &raw[36..];
            let computed = dugite_primitives::hash::blake2b_256(payload);
            if computed.as_bytes() != stored_checksum {
                return Err(LedgerError::EpochTransition(
                    "Snapshot checksum mismatch — file may be corrupted".to_string(),
                ));
            }
            payload
        } else {
            // Legacy format: raw bincode without header (backwards compatible)
            warn!("Loading legacy snapshot without checksum verification");
            &raw
        };

        // Use bincode options with size limit as defense-in-depth against
        // malicious payloads that encode enormous internal allocations.
        // Must use with_fixint_encoding() to match bincode::serialize() defaults.
        use bincode::Options;
        let snapshot: super::snapshot_format::LedgerStateSnapshot = bincode::options()
            .with_fixint_encoding()
            .allow_trailing_bytes()
            .with_limit(MAX_SNAPSHOT_SIZE as u64)
            .deserialize(data)
            .map_err(|e| {
                LedgerError::EpochTransition(format!("Failed to deserialize ledger state: {e}"))
            })?;
        let mut state = LedgerState::from(snapshot);
        state.utxo.utxo_set.rebuild_address_index();
        // Re-enable indexing so subsequent insert/remove operations maintain the index.
        // The #[serde(skip)] on indexing_enabled defaults to false after deserialization.
        state.utxo.utxo_set.set_indexing_enabled(true);
        // After loading a snapshot, incremental stake tracking may have drifted.
        // Rebuild stake distribution from the full UTxO set, then recompute
        // pool_stake for all existing snapshots (mark/set/go).
        //
        // IMPORTANT: Only run if the UTxO set is non-empty. When using an LSM-backed
        // UTxO store, the store hasn't been attached yet at this point — the in-memory
        // set is empty. Running rebuild_stake_distribution on an empty set would wipe
        // all pool_stake values, causing block producers to see zero stake. The caller
        // (dugite-node) runs rebuild + recompute again AFTER attaching the LSM store.
        if !state.utxo.utxo_set.is_empty() {
            state.rebuild_stake_distribution();
            state.recompute_snapshot_pool_stakes();
        }
        // Trigger one full rebuild at the next epoch boundary to correct any drift
        // from the snapshot (which may have been saved with stale incremental state).
        // After that single rebuild, incremental tracking takes over.
        state.epochs.needs_stake_rebuild = true;
        // After loading a snapshot, the node is past genesis — RUPD should fire
        // at the next epoch boundary. Old snapshots without this field will
        // deserialize with rupd_ready=false (serde default), so set it here.
        state.epochs.snapshots.rupd_ready = true;
        // Migration: populate per-credential deposit maps from current protocol
        // parameters when loading snapshots written before per-credential deposit
        // tracking was added (version < 12). This is an approximation that is
        // correct for all networks where key_deposit/pool_deposit have never
        // changed via governance.
        if state.certs.stake_key_deposits.is_empty() && !state.certs.reward_accounts.is_empty() {
            let deposit = state.epochs.protocol_params.key_deposit.0;
            for cred_hash in state.certs.reward_accounts.keys() {
                state.certs.stake_key_deposits.insert(*cred_hash, deposit);
            }
            debug!(
                "Migrated {} stake key deposits from current key_deposit={}",
                state.certs.stake_key_deposits.len(),
                deposit,
            );
        }
        if state.certs.pool_deposits.is_empty() && !state.certs.pool_params.is_empty() {
            let deposit = state.epochs.protocol_params.pool_deposit.0;
            for pool_id in state.certs.pool_params.keys() {
                state.certs.pool_deposits.insert(*pool_id, deposit);
            }
            debug!(
                "Migrated {} pool deposits from current pool_deposit={}",
                state.certs.pool_deposits.len(),
                deposit,
            );
        }
        debug!(
            "Snapshot loaded from {} ({:.1} MB, {} UTxOs, epoch {})",
            path.display(),
            raw.len() as f64 / 1_048_576.0,
            state.utxo.utxo_set.len(),
            state.epoch.0,
        );
        Ok(state)
    }

    /// Save the attached UTxO store's LSM snapshot.
    ///
    /// Call this after `save_snapshot()` when using on-disk UTxO storage.
    /// Requires mutable access because `LsmTree::save_snapshot` is `&mut self`.
    pub fn save_utxo_snapshot(&mut self) -> Result<(), LedgerError> {
        if let Some(store) = self.utxo.utxo_set.store_mut() {
            // Delete any existing snapshot first to avoid "already exists" error
            let _ = store.delete_snapshot("ledger");
            store.save_snapshot("ledger").map_err(|e| {
                LedgerError::EpochTransition(format!("Failed to save UTxO store snapshot: {e}"))
            })?;
            debug!("UTxO store snapshot saved ({} entries)", store.len());
        }
        Ok(())
    }

    /// Attach an on-disk UTxO store to this ledger state.
    ///
    /// All subsequent UTxO operations will use the LSM-backed store.
    /// If the ledger has in-memory UTxOs (from bincode snapshot load),
    /// they are migrated to the store before attachment.
    pub fn attach_utxo_store(&mut self, mut store: crate::utxo_store::UtxoStore) {
        // Migrate any in-memory UTxOs to the store. `iter()` used to
        // materialise every entry into a throw-away `Vec` before the copy
        // loop; at preview scale (~3M UTxOs) that intermediate buffer was
        // multi-GB.  Stream the HashMap directly instead (#403).
        if !self.utxo.utxo_set.is_empty() && !self.utxo.utxo_set.has_store() {
            let count = self.utxo.utxo_set.len();
            tracing::info!("Migrating {} in-memory UTxOs to on-disk store", count);
            self.utxo.utxo_set.scan_all(|input, output| {
                store.insert(input.clone(), output.clone());
            });
        }
        store.set_indexing_enabled(true);
        store.rebuild_address_index();
        self.utxo.utxo_set.attach_store(store);
        tracing::info!("UTxO store attached ({} entries)", self.utxo.utxo_set.len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::LedgerStateSnapshot;
    use dugite_primitives::era::Era;
    use dugite_primitives::protocol_params::ProtocolParameters;
    use dugite_primitives::time::EpochNo;
    use dugite_primitives::value::Lovelace;

    // -----------------------------------------------------------------------
    // 1. Save/load roundtrip: verify that key fields survive serialisation
    // -----------------------------------------------------------------------

    /// Save a `LedgerState` with recognisable field values, load it back, and
    /// verify that `epoch`, `treasury`, and `era` are preserved exactly.
    #[test]
    fn test_save_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("roundtrip.bin");

        let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());
        state.epoch = EpochNo(7);
        state.epochs.treasury = Lovelace(42_000_000);
        state.era = Era::Conway;

        state.save_snapshot(&path).unwrap();
        let loaded = LedgerState::load_snapshot(&path).unwrap();

        assert_eq!(loaded.epoch, EpochNo(7), "epoch must survive roundtrip");
        assert_eq!(
            loaded.epochs.treasury,
            Lovelace(42_000_000),
            "treasury must survive roundtrip"
        );
        assert_eq!(loaded.era, Era::Conway, "era must survive roundtrip");
    }

    // -----------------------------------------------------------------------
    // 2. Magic bytes: first 4 bytes of the on-disk file must be b"DUGT"
    // -----------------------------------------------------------------------

    /// Save a snapshot and verify that the raw on-disk file starts with the
    /// expected `DUGT` magic word.
    #[test]
    fn test_magic_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("magic.bin");

        let state = LedgerState::new(ProtocolParameters::mainnet_defaults());
        state.save_snapshot(&path).unwrap();

        let raw = std::fs::read(&path).unwrap();
        assert!(raw.len() >= 4, "snapshot file must be at least 4 bytes");
        assert_eq!(&raw[..4], b"DUGT", "first 4 bytes must be magic word DUGT");
    }

    // -----------------------------------------------------------------------
    // 3. Checksum verification: stored checksum matches blake2b-256 of payload
    // -----------------------------------------------------------------------

    /// Save a snapshot, then manually re-derive the blake2b-256 checksum over
    /// the payload region (bytes 37..) and assert it equals the stored
    /// checksum (bytes 5..37).
    #[test]
    fn test_checksum_verification() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("checksum.bin");

        let state = LedgerState::new(ProtocolParameters::mainnet_defaults());
        state.save_snapshot(&path).unwrap();

        let raw = std::fs::read(&path).unwrap();

        // Header layout: DUGT(4) + version(1) + checksum(32) + payload(N)
        assert!(
            raw.len() > 37,
            "snapshot must be longer than the 37-byte header"
        );
        let stored_checksum = &raw[5..37];
        let payload = &raw[37..];
        let computed = dugite_primitives::hash::blake2b_256(payload);
        assert_eq!(
            computed.as_bytes(),
            stored_checksum,
            "stored checksum must equal blake2b-256(payload)"
        );
    }

    // -----------------------------------------------------------------------
    // 4. Corrupted data detected: flipping a payload byte must cause an error
    // -----------------------------------------------------------------------

    /// Save a snapshot, flip a single byte in the payload region, then attempt
    /// to load it — the checksum mismatch must produce a `LedgerError`.
    #[test]
    fn test_corrupted_data_detected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("corrupt.bin");

        let state = LedgerState::new(ProtocolParameters::mainnet_defaults());
        state.save_snapshot(&path).unwrap();

        // Flip a byte in the payload (byte 40 is well within the payload region).
        let mut raw = std::fs::read(&path).unwrap();
        assert!(
            raw.len() > 40,
            "snapshot must be long enough to corrupt byte 40"
        );
        raw[40] ^= 0xFF;
        std::fs::write(&path, &raw).unwrap();

        let result = LedgerState::load_snapshot(&path);
        assert!(
            result.is_err(),
            "loading a corrupted snapshot must return an error"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("checksum") || msg.contains("corrupt"),
            "error message must mention checksum or corruption, got: {msg}"
        );
    }

    // -----------------------------------------------------------------------
    // 5. Size limit enforcement: MAX_SNAPSHOT_SIZE constant and reject path
    // -----------------------------------------------------------------------

    /// Verify that `MAX_SNAPSHOT_SIZE` is 10 GiB and that a file whose raw
    /// length exceeds the limit is rejected before any deserialisation attempt.
    #[test]
    fn test_size_limit_enforcement() {
        // The constant must be exactly 10 GiB.
        assert_eq!(
            MAX_SNAPSHOT_SIZE,
            10 * 1024 * 1024 * 1024,
            "MAX_SNAPSHOT_SIZE must be 10 GiB"
        );

        // Write a tiny file whose first 8 bytes encode a length field larger
        // than MAX_SNAPSHOT_SIZE — the raw-bytes size check triggers first.
        // We achieve this by writing (MAX_SNAPSHOT_SIZE + 1) bytes so that
        // the check `raw.len() > MAX_SNAPSHOT_SIZE` fires immediately.
        //
        // Writing 10 GiB to disk is impractical in a unit test, so instead
        // we construct a file that *claims* (via its bincode length prefix)
        // to contain an enormous allocation.  The with_limit() guard inside
        // load_snapshot rejects it at the deserialization stage.
        let dir = tempfile::tempdir().unwrap();
        let malicious_path = dir.path().join("malicious.bin");

        // Raw bincode (no DUGT header, so raw path taken): a u64 length
        // that exceeds MAX_SNAPSHOT_SIZE.
        let huge_len: u64 = (MAX_SNAPSHOT_SIZE as u64) + 1;
        let mut payload = huge_len.to_le_bytes().to_vec();
        payload.extend_from_slice(&[0u8; 64]); // padding so the file exists
        std::fs::write(&malicious_path, &payload).unwrap();

        let result = LedgerState::load_snapshot(&malicious_path);
        // The file is < MAX_SNAPSHOT_SIZE bytes so the raw-size gate passes,
        // but bincode's with_limit() should reject the giant allocation.
        assert!(
            result.is_err(),
            "a snapshot claiming a huge allocation must be rejected"
        );
    }

    // -----------------------------------------------------------------------
    // 6. Legacy format loading: plain bincode (no DUGT header) must succeed
    // -----------------------------------------------------------------------

    /// Write a snapshot in the legacy raw-bincode format (no `DUGT` header at
    /// all) and verify that `load_snapshot` can still deserialise it.
    ///
    /// This exercises the third branch of `load_snapshot`: the plain-bincode
    /// fallback that exists for backwards compatibility with very old snapshots.
    #[test]
    fn test_legacy_format_loading() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy-raw.bin");

        let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());
        state.epoch = EpochNo(3);

        // Serialise via LedgerStateSnapshot (LedgerState no longer implements
        // Serialize directly), which uses the same bincode encoder that the
        // legacy path deserialises with.
        let snapshot = LedgerStateSnapshot::from(&state);
        let raw_bincode = bincode::serialize(&snapshot).unwrap();

        // The file must NOT start with `DUGT` so the legacy path is taken.
        // A plain bincode-serialised `LedgerStateSnapshot` starts with the u64
        // field count or the first field value, never with the ASCII string "DUGT".
        std::fs::write(&path, &raw_bincode).unwrap();

        let loaded = LedgerState::load_snapshot(&path).unwrap();
        assert_eq!(
            loaded.epoch,
            EpochNo(3),
            "epoch must be preserved through legacy raw-bincode load"
        );
    }

    // -----------------------------------------------------------------------
    // 7. Version byte in header: byte at position 4 must equal SNAPSHOT_VERSION
    // -----------------------------------------------------------------------

    /// Save a snapshot and assert that byte 4 (the version field) equals
    /// `SNAPSHOT_VERSION` (currently 14).
    #[test]
    fn test_version_in_header() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("version.bin");

        let state = LedgerState::new(ProtocolParameters::mainnet_defaults());
        state.save_snapshot(&path).unwrap();

        let raw = std::fs::read(&path).unwrap();
        assert!(raw.len() > 4, "snapshot must be longer than 4 bytes");
        assert_eq!(
            raw[4],
            LedgerState::SNAPSHOT_VERSION,
            "byte 4 must be SNAPSHOT_VERSION ({})",
            LedgerState::SNAPSHOT_VERSION
        );
    }

    // -----------------------------------------------------------------------
    // 8. Atomic write: the .tmp file must NOT exist after save completes
    // -----------------------------------------------------------------------

    /// Save a snapshot and verify that the `.tmp` staging file has been
    /// renamed away and does not exist on disk after `save_snapshot` returns.
    #[test]
    fn test_atomic_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("atomic.bin");
        let tmp_path = path.with_extension("tmp");

        let state = LedgerState::new(ProtocolParameters::mainnet_defaults());
        state.save_snapshot(&path).unwrap();

        // The final file must exist.
        assert!(
            path.exists(),
            "final snapshot file must exist after save_snapshot"
        );
        // The temporary staging file must have been renamed away.
        assert!(
            !tmp_path.exists(),
            ".tmp staging file must not exist after save_snapshot completes (atomic rename)"
        );
    }
}
