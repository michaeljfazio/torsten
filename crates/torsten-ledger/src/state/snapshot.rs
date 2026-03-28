//! Ledger state snapshot persistence: save, load, and UTxO store attachment.
//!
//! # Snapshot format
//!
//! All snapshots use bincode serialization of [`LedgerState`].  The on-disk
//! layout is:
//!
//! ```text
//! [4 bytes]  magic  "TRSN"
//! [1 byte]   version (SNAPSHOT_VERSION)
//! [32 bytes] blake2b-256 checksum of the payload
//! [N bytes]  bincode payload (LedgerState)
//! ```
//!
//! Two legacy formats are also supported for backwards compatibility:
//! - **Legacy with checksum** – `TRSN` + 32-byte checksum + data (no version byte)
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
    pub(crate) const SNAPSHOT_VERSION: u8 = 8;

    /// Save ledger state snapshot to disk using bincode serialization.
    ///
    /// Format: `[4-byte magic "TRSN"][1-byte version][32-byte blake2b checksum][bincode data]`
    ///
    /// The write is atomic: data is written to a `.tmp` file and then renamed
    /// over the final path so that a crash mid-write does not produce a partial
    /// or corrupt snapshot file.
    pub fn save_snapshot(&self, path: &Path) -> Result<(), LedgerError> {
        let tmp_path = path.with_extension("tmp");

        // Serialize the ledger state to bincode.
        let data = bincode::serialize(self).map_err(|e| {
            LedgerError::EpochTransition(format!("Failed to serialize ledger state: {e}"))
        })?;

        // Compute checksum over the serialized data
        let checksum = torsten_primitives::hash::blake2b_256(&data);

        // Write header + data using a single buffered write.
        // Header: "TRSN" (4 bytes) + version (1 byte) + blake2b checksum (32 bytes)
        use std::io::Write;
        let file = std::fs::File::create(&tmp_path)
            .map_err(|e| LedgerError::EpochTransition(format!("Failed to create snapshot: {e}")))?;
        let mut writer = std::io::BufWriter::with_capacity(1 << 20, file);
        writer.write_all(b"TRSN").map_err(|e| {
            LedgerError::EpochTransition(format!("Failed to write snapshot header: {e}"))
        })?;
        writer.write_all(&[Self::SNAPSHOT_VERSION]).map_err(|e| {
            LedgerError::EpochTransition(format!("Failed to write snapshot version: {e}"))
        })?;
        writer.write_all(checksum.as_bytes()).map_err(|e| {
            LedgerError::EpochTransition(format!("Failed to write snapshot checksum: {e}"))
        })?;
        writer.write_all(&data).map_err(|e| {
            LedgerError::EpochTransition(format!("Failed to write snapshot data: {e}"))
        })?;
        writer
            .flush()
            .map_err(|e| LedgerError::EpochTransition(format!("Failed to flush snapshot: {e}")))?;
        drop(writer);

        let total_bytes = 4 + 1 + 32 + data.len();

        std::fs::rename(&tmp_path, path)
            .map_err(|e| LedgerError::EpochTransition(format!("Failed to rename snapshot: {e}")))?;
        info!(
            "Snapshot     saved (epoch={}, {} UTxOs, {:.1} MB)",
            self.epoch.0,
            self.utxo_set.len(),
            total_bytes as f64 / 1_048_576.0,
        );
        Ok(())
    }

    /// Load ledger state snapshot from disk.
    ///
    /// Rejects snapshots larger than [`MAX_SNAPSHOT_SIZE`] to prevent OOM.
    ///
    /// Supports three formats:
    /// - **Versioned (v1+):** `TRSN` + version byte + 32-byte checksum + data
    /// - **Legacy with checksum:** `TRSN` + 32-byte checksum + data (no version byte)
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

        let data = if raw.len() >= 37 && &raw[..4] == b"TRSN" {
            let fifth_byte = raw[4];
            if fifth_byte > 0 && fifth_byte < 128 {
                // Versioned format: TRSN + version(1) + checksum(32) + data
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
                let computed = torsten_primitives::hash::blake2b_256(payload);
                if computed.as_bytes() != stored_checksum {
                    return Err(LedgerError::EpochTransition(
                        "Snapshot checksum mismatch — file may be corrupted".to_string(),
                    ));
                }
                payload
            } else {
                // Legacy format with checksum but no version byte:
                // TRSN + checksum(32) + data (5th byte is part of blake2b hash)
                warn!("Loading legacy snapshot (no version byte) with checksum verification");
                let stored_checksum = &raw[4..36];
                let payload = &raw[36..];
                let computed = torsten_primitives::hash::blake2b_256(payload);
                if computed.as_bytes() != stored_checksum {
                    return Err(LedgerError::EpochTransition(
                        "Snapshot checksum mismatch — file may be corrupted".to_string(),
                    ));
                }
                payload
            }
        } else if raw.len() >= 36 && &raw[..4] == b"TRSN" {
            // Legacy format with checksum (exactly 36 bytes of header, rare edge case)
            warn!("Loading legacy snapshot (no version byte) with checksum verification");
            let stored_checksum = &raw[4..36];
            let payload = &raw[36..];
            let computed = torsten_primitives::hash::blake2b_256(payload);
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
        let mut state: LedgerState = bincode::options()
            .with_fixint_encoding()
            .allow_trailing_bytes()
            .with_limit(MAX_SNAPSHOT_SIZE as u64)
            .deserialize(data)
            .map_err(|e| {
                LedgerError::EpochTransition(format!("Failed to deserialize ledger state: {e}"))
            })?;
        state.utxo_set.rebuild_address_index();
        // Re-enable indexing so subsequent insert/remove operations maintain the index.
        // The #[serde(skip)] on indexing_enabled defaults to false after deserialization.
        state.utxo_set.set_indexing_enabled(true);
        // After loading a snapshot, incremental stake tracking may have drifted.
        // Rebuild stake distribution from the full UTxO set, then recompute
        // pool_stake for all existing snapshots (mark/set/go).
        //
        // IMPORTANT: Only run if the UTxO set is non-empty. When using an LSM-backed
        // UTxO store, the store hasn't been attached yet at this point — the in-memory
        // set is empty. Running rebuild_stake_distribution on an empty set would wipe
        // all pool_stake values, causing block producers to see zero stake. The caller
        // (torsten-node) runs rebuild + recompute again AFTER attaching the LSM store.
        if !state.utxo_set.is_empty() {
            state.rebuild_stake_distribution();
            state.recompute_snapshot_pool_stakes();
        }
        // Trigger one full rebuild at the next epoch boundary to correct any drift
        // from the snapshot (which may have been saved with stale incremental state).
        // After that single rebuild, incremental tracking takes over.
        state.needs_stake_rebuild = true;
        // After loading a snapshot, the node is past genesis — RUPD should fire
        // at the next epoch boundary. Old snapshots without this field will
        // deserialize with rupd_ready=false (serde default), so set it here.
        state.snapshots.rupd_ready = true;
        debug!(
            "Snapshot loaded from {} ({:.1} MB, {} UTxOs, epoch {})",
            path.display(),
            raw.len() as f64 / 1_048_576.0,
            state.utxo_set.len(),
            state.epoch.0,
        );
        Ok(state)
    }

    /// Save the attached UTxO store's LSM snapshot.
    ///
    /// Call this after `save_snapshot()` when using on-disk UTxO storage.
    /// Requires mutable access because `LsmTree::save_snapshot` is `&mut self`.
    pub fn save_utxo_snapshot(&mut self) -> Result<(), LedgerError> {
        if let Some(store) = self.utxo_set.store_mut() {
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
        // Migrate any in-memory UTxOs to the store
        if !self.utxo_set.is_empty() && !self.utxo_set.has_store() {
            let count = self.utxo_set.len();
            tracing::info!("Migrating {} in-memory UTxOs to on-disk store", count);
            for (input, output) in self.utxo_set.iter() {
                store.insert(input, output);
            }
        }
        store.set_indexing_enabled(true);
        store.rebuild_address_index();
        self.utxo_set.attach_store(store);
        tracing::info!("UTxO store attached ({} entries)", self.utxo_set.len());
    }
}
