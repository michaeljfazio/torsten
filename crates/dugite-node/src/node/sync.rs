//! Block sync loop, forward-block processing, rollback handling, and ledger replay.
//!
//! This module contains the core pipelined ChainSync state machine that drives
//! block ingestion from upstream peers, as well as the ledger replay path used
//! after a Mithril snapshot import.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::{watch, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use super::connection_lifecycle::{CandidateChainState, PendingHeader};
use super::networking::EbbInfo;
use dugite_consensus::praos::BlockIssuerInfo;
use dugite_consensus::ValidationMode;
use dugite_ledger::BlockValidationMode;
use dugite_network::codec::Point as CodecPoint;
use dugite_network::protocol::chainsync::{
    decode_message as cs_decode, encode_message as cs_encode, ChainSyncMessage,
};
use dugite_network::MuxChannel;
use dugite_network::RollbackAnnouncement;
use dugite_primitives::block::Point;

use super::epoch::SnapshotPolicy;
use super::Node;

// ─── Genesis block validation (free function, used by tests) ─────────────────

/// Validate genesis blocks against expected hashes from the configuration.
///
/// When syncing from genesis (Origin), the first blocks received are the genesis
/// blocks for the chain. For Byron-era networks (mainnet, preprod), the first
/// block is a Byron Epoch Boundary Block (EBB) whose hash must match the
/// expected Byron genesis hash. For networks that start directly in the Shelley
/// era (preview), the first block's prev_hash should match the expected Shelley
/// genesis hash.
///
/// This validation is crucial to ensure we are syncing the correct chain and
/// not connecting to a peer serving a different network's blocks.
#[allow(dead_code)] // retained for networking rewrite; also used in tests
pub fn validate_genesis_blocks(
    blocks: &[dugite_primitives::block::Block],
    expected_byron_hash: Option<&dugite_primitives::hash::Hash32>,
    expected_shelley_hash: Option<&dugite_primitives::hash::Hash32>,
) -> Result<()> {
    if blocks.is_empty() {
        return Ok(());
    }

    let first_block = &blocks[0];

    // Only validate if we're starting from genesis (block 0 at slot 0).
    // If ChainDB already has blocks, genesis was validated on a prior run.
    if first_block.block_number().0 != 0 {
        debug!(
            "Skipping genesis validation — not syncing from genesis (block={})",
            first_block.block_number().0,
        );
        return Ok(());
    }

    // For Byron-era chains, the first block is the Byron EBB (block 0, slot 0).
    // Its hash must match the expected Byron genesis hash.
    if first_block.era == dugite_primitives::era::Era::Byron {
        if let Some(expected) = expected_byron_hash {
            let actual = first_block.hash();
            if actual != expected {
                return Err(anyhow::anyhow!(
                    "Byron genesis block hash mismatch: expected {}, got {} — \
                     this chain does not match the configured genesis. \
                     Check that you are connecting to the correct network.",
                    expected.to_hex(),
                    actual.to_hex()
                ));
            }
            debug!("Byron genesis block validated: {}", actual.to_hex());
        } else {
            warn!("No Byron genesis hash configured — skipping Byron genesis block validation");
        }
    }

    // For Shelley-first chains (e.g., preview testnet), the first block may be
    // a Shelley-era block. Its prev_hash points to the Shelley genesis hash.
    if first_block.era.is_shelley_based() && first_block.block_number().0 == 0 {
        if let Some(expected) = expected_shelley_hash {
            let prev_hash = first_block.prev_hash();
            if prev_hash != expected {
                return Err(anyhow::anyhow!(
                    "Shelley genesis hash mismatch: expected {}, but first block's \
                     prev_hash is {} — this chain does not match the configured genesis. \
                     Check that you are connecting to the correct network.",
                    expected.to_hex(),
                    prev_hash.to_hex()
                ));
            }
            debug!("Shelley genesis ref validated: {}", expected.to_hex());
        } else {
            warn!("No Shelley genesis hash configured — skipping Shelley genesis block validation");
        }
    }

    Ok(())
}

// ─── Node impl: sync loop ─────────────────────────────────────────────────────

impl Node {
    /// Validate genesis blocks against expected hashes from the configuration.
    #[allow(dead_code)] // retained for networking rewrite
    pub(crate) fn validate_genesis_blocks(
        &self,
        blocks: &[dugite_primitives::block::Block],
    ) -> Result<()> {
        validate_genesis_blocks(
            blocks,
            self.expected_byron_genesis_hash.as_ref(),
            self.expected_shelley_genesis_hash.as_ref(),
        )
    }

    /// Compute the current absolute slot number from wall-clock time.
    ///
    /// Uses the HFC era history state machine to correctly account for all
    /// era transitions (Byron→Shelley→...→Conway) with their respective
    /// slot durations.
    pub async fn current_wall_clock_slot(&self) -> Option<dugite_primitives::time::SlotNo> {
        let genesis = self.shelley_genesis.as_ref()?;
        let system_start = dugite_primitives::time::SystemStart {
            utc_time: chrono::DateTime::parse_from_rfc3339(&genesis.system_start)
                .ok()?
                .with_timezone(&chrono::Utc),
        };
        let eh = self.era_history.read().await;
        eh.wallclock_to_slot(chrono::Utc::now(), &system_start).ok()
    }

    /// Notify connected N2N/N2C peers of a chain rollback by broadcasting a
    /// `RollbackAnnouncement`.  Both the N2N ChainSync server and the N2C
    /// LocalChainSync server subscribe to this channel and translate the
    /// announcement into `MsgRollBackward` messages for their downstream peers.
    pub async fn notify_rollback(&self, rollback_point: &Point) {
        if let Some(ref tx) = self.rollback_announcement_tx {
            let rb_slot = rollback_point.slot().map(|s| s.0).unwrap_or(0);
            let rb_hash = rollback_point
                .hash()
                .map(|h| {
                    let bytes: &[u8] = h.as_ref();
                    let mut arr = [0u8; 32];
                    arr.copy_from_slice(bytes);
                    arr
                })
                .unwrap_or([0u8; 32]);

            let _ = tx.send(RollbackAnnouncement {
                slot: rb_slot,
                hash: rb_hash,
            });
        }
    }

    /// Reset the ledger state to genesis and replay ImmutableDB blocks up to
    /// `target_slot`.  Used when no suitable snapshot is available for
    /// rollback (or the snapshot failed to load).
    ///
    /// Uses sequential chunk iteration (same as startup replay) for high
    /// throughput — `get_next_block_after_slot()` is too slow for millions
    /// of blocks because it scans chunk metadata on every call.
    ///
    /// # Deprecation
    ///
    /// This function represents the "genesis replay" path that the Haskell
    /// architecture explicitly avoids.  In the new architecture (Subsystems
    /// 1–6), rollback is handled by `LedgerSeq::rollback()` which is O(k)
    /// and never requires replaying from genesis.  This function will be
    /// removed once `startup::recover_ledger_seq()` is fully wired in as the
    /// primary recovery path.
    ///
    /// TODO(subsystem-4): Replace callers with `LedgerSeq::rollback()` via
    /// the new startup recovery sequence.  Track in the migration plan under
    /// "Phase 5: Remove old fork recovery".
    ///
    /// Tracked in: https://github.com/dugite-project/dugite/issues/TODO
    #[allow(deprecated, dead_code)] // retained for networking rewrite
    async fn reset_ledger_and_replay(&self, target_slot: u64) {
        {
            let mut ls = self.ledger_state.write().await;
            // Drop the stale fork UTxO store — do NOT re-attach it.
            //
            // When called after a deep rollback or fork-snapshot recovery, the
            // UTxO store was built against a fork chain.  Re-attaching it
            // permanently corrupts state: genesis-replayed UTxOs coexist with
            // stale fork UTxOs, causing every subsequent apply_block to fail
            // silently (inputs not found, duplicate outputs, 0 blocks applied
            // forever).
            //
            // The genesis replay that follows builds a correct in-memory UTxO
            // set from scratch.  A fresh LSM snapshot is saved after replay
            // completes so subsequent restarts can use the canonical store.
            let _stale_store = ls.utxo.utxo_set.detach_store(); // drops the stale fork store
            *ls = dugite_ledger::LedgerState::new(ls.epochs.protocol_params.clone());
            // No re-attach: replay proceeds with a clean in-memory UTxO set only.
        }

        // Replay ImmutableDB blocks from genesis up to target_slot so the
        // ledger tip matches the rollback/intersection point.  Without this
        // replay, the ledger stays at genesis and incoming blocks from the
        // peer won't connect.
        if target_slot > 0 {
            let immutable_dir = self.database_path.join("immutable");
            if !immutable_dir.is_dir() {
                warn!("Rollback: no immutable directory found for replay");
                return;
            }

            // Run the replay on a blocking thread to avoid starving the
            // async runtime — chunk I/O is synchronous and CPU-bound.
            let ledger_state = self.ledger_state.clone();
            let bel = self.byron_epoch_length;
            let database_path = self.database_path.clone();
            let result = tokio::task::spawn_blocking(move || {
                let replay_start = std::time::Instant::now();
                let mut replayed = 0u64;
                let mut last_log = std::time::Instant::now();

                // Disable address index during replay for speed (rebuilt on
                // reattach after we're done).
                {
                    let mut ls = ledger_state.blocking_write();
                    ls.utxo.utxo_set.set_indexing_enabled(false);
                    ls.utxo.utxo_set.set_wal_enabled(false);
                }

                let result = crate::mithril::replay_from_chunk_files(
                    &immutable_dir,
                    |cbor| {
                        match dugite_serialization::multi_era::decode_block_minimal_with_byron_epoch_length(
                            cbor, bel,
                        ) {
                            Ok(block) => {
                                // Stop once we've passed the target slot.
                                if block.slot().0 > target_slot {
                                    return Err(anyhow::anyhow!("reached target slot"));
                                }
                                let mut ls = ledger_state.blocking_write();
                                if let Err(e) =
                                    ls.apply_block(&block, BlockValidationMode::ApplyOnly)
                                {
                                    // Non-fatal: some early blocks may not connect
                                    // when the UTxO store is from a later state.
                                    tracing::warn!(
                                        slot = block.slot().0,
                                        "Rollback: replay apply skipped: {e}"
                                    );
                                }
                                replayed += 1;
                                if last_log.elapsed().as_secs() >= 5 {
                                    let elapsed = replay_start.elapsed().as_secs();
                                    let speed = replayed.checked_div(elapsed).unwrap_or(replayed);
                                    tracing::info!(
                                        replayed,
                                        slot = block.slot().0,
                                        target_slot,
                                        speed = format_args!("{speed} blk/s"),
                                        "Rollback: replay progress",
                                    );
                                    last_log = std::time::Instant::now();
                                }
                                Ok(())
                            }
                            Err(e) => {
                                tracing::warn!("Rollback: replay decode error: {e}");
                                Ok(()) // skip bad block, continue
                            }
                        }
                    },
                );

                // Re-enable indexing.
                {
                    let mut ls = ledger_state.blocking_write();
                    ls.utxo.utxo_set.set_indexing_enabled(true);
                    ls.utxo.utxo_set.set_wal_enabled(true);
                }

                // Save a fresh canonical snapshot so the next restart can load
                // it instead of re-running genesis replay again.  This is
                // especially important because we just dropped the stale fork
                // UTxO store — without saving here the LSM store on disk still
                // reflects the fork chain, and the next restart would have to
                // do another full genesis replay.
                {
                    let mut ls = ledger_state.blocking_write();
                    let snapshot_path = database_path.join("ledger-snapshot.bin");
                    let snap_slot = ls.tip.point.slot().map(|s| s.0).unwrap_or(0);
                    if let Err(e) = ls.save_utxo_snapshot() {
                        tracing::warn!(snap_slot, "reset_ledger_and_replay: failed to save UTxO snapshot: {e}");
                    }
                    if let Err(e) = ls.save_snapshot(&snapshot_path) {
                        tracing::warn!(snap_slot, "reset_ledger_and_replay: failed to save ledger snapshot: {e}");
                    } else {
                        tracing::info!(
                            snap_slot,
                            "reset_ledger_and_replay: post-reset snapshot saved — next restart \
                             will skip genesis replay"
                        );
                    }
                }

                let elapsed = replay_start.elapsed().as_secs_f64();
                tracing::info!(
                    replayed,
                    target_slot,
                    elapsed_secs = format!("{elapsed:.1}"),
                    "Rollback: replay complete"
                );

                // The "reached target slot" error is expected and not a real failure.
                match result {
                    Ok(_) => Ok(replayed),
                    Err(e) if e.to_string().contains("reached target slot") => Ok(replayed),
                    Err(e) => Err(e),
                }
            })
            .await;

            if let Err(e) = result {
                error!("Rollback: replay task failed: {e}");
            }
        }
    }

    /// Handle a chain rollback: roll back ChainDB and restore ledger UTxO state
    /// to the rollback point.
    ///
    /// # Future direction (Phase 4 migration)
    ///
    /// Once `LedgerSeq` is fully integrated as the authoritative ledger state
    /// store, rollback will be delegated to `LedgerSeq::rollback(n)`.  That
    /// path is O(k) by design and never triggers a genesis replay.  The
    /// current DiffSeq fast path is the precursor to that design.
    ///
    /// TODO(subsystem-4): Delegate to `LedgerSeq::rollback()` once the seq is
    /// maintained as the primary ledger state representation.
    ///
    /// The current fast-path (DiffSeq) is the precursor to this. When LedgerSeq
    /// is the authoritative state store, replace the diff-based rollback with:
    ///   seq.rollback(rollback_slot)
    ///
    /// Tracked in: https://github.com/dugite-project/dugite/issues/TODO
    ///
    /// # Fast path — diff-based rollback
    ///
    /// When the rollback target is within the in-memory `DiffSeq` window (i.e.
    /// the rolled-back blocks were applied during this session and their per-block
    /// UTxO diffs are still held in memory), the ledger is restored by unapplying
    /// the diffs directly:
    ///
    ///   1. Identify which blocks in the DiffSeq are *after* the rollback point.
    ///   2. Call `rollback_blocks_to_point(n, new_tip)` to invert their UTxO
    ///      changes (remove inserted UTxOs, re-insert deleted UTxOs).
    ///   3. Update the ledger tip to the rollback point.
    ///
    /// This is O(txs in rolled-back blocks) and requires no I/O, making it
    /// ideal for the common micro-fork case (1-block chain reorganisation).
    ///
    /// # Slow path — snapshot reload + replay
    ///
    /// When the target is outside the diff window (e.g. after a node restart
    /// that cleared the in-memory diffs, or a deep rollback beyond k blocks),
    /// the ledger is rebuilt from the best available snapshot followed by
    /// replaying ImmutableDB blocks up to the rollback point.
    /// # Architecture note — live rollback path
    ///
    /// At tip, rollbacks are driven by peer ChainSync messages.  The flow is:
    ///
    /// 1. A peer sends `MsgRollBackward` to our `chainsync_client_task`.
    /// 2. The client trims the `candidate_chains` pending-header list.
    /// 3. The peer then re-sends the fork blocks via `MsgRollForward`.
    /// 4. `process_forward_blocks` (called from `apply_fetched_block`) applies
    ///    them; if they don't connect to the ledger tip the mismatch is detected
    ///    and this function is called to realign.
    ///
    /// This function is also called after `SwitchedToFork` is returned from
    /// `ChainSelQueue` when the VolatileDB chain has diverged from the ledger.
    ///
    /// TODO (Task 7): wire this directly into the MsgRollBackward handler in
    /// `chainsync_client_task` via a shared rollback channel.
    #[allow(dead_code)] // TODO (Task 7): call from MsgRollBackward dispatch path
    pub async fn handle_rollback(&self, rollback_point: &Point) {
        let rollback_slot = rollback_point.slot().map(|s| s.0).unwrap_or(0);

        // Count every rollback event for observability, even no-ops.
        self.metrics
            .rollback_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        // If the rollback point is at or beyond our ledger tip, it's a no-op.
        // This commonly happens after reconnection when the server confirms
        // the intersection by sending a RollBackward to the same point.
        {
            let ls = self.ledger_state.read().await;
            let ledger_slot = ls.tip.point.slot().map(|s| s.0).unwrap_or(0);
            if rollback_slot >= ledger_slot {
                debug!(
                    rollback_slot,
                    ledger_slot, "Rollback point is at or ahead of ledger tip, skipping"
                );
                return;
            }
        }

        // 1. Roll back ChainDB (removes volatile blocks after the rollback point).
        {
            let mut db = self.chain_db.write().await;
            if let Err(e) = db.rollback_to_point(rollback_point) {
                error!("ChainDB rollback failed: {e}");
                return;
            }
        }

        // 2. Full-state rollback via snapshot + replay.
        //
        // The previous fast-path (DiffSeq) only restored UTxO changes — nonce
        // accumulators, delegation state, reward accounts, governance, and other
        // LedgerState fields were NOT rolled back.  This caused incorrect epoch
        // nonces and VRF leader check failures after any rollback.
        //
        // Issue #308: We now always use the full-state restoration path:
        // find the best snapshot before the rollback point, load it, and replay
        // blocks forward.  This is O(snapshot_interval) but guarantees ALL
        // state fields are correctly restored.
        //
        // When LedgerSeq is fully wired as the authoritative state store,
        // rollback will be delegated to LedgerSeq::rollback(n) + tip_state()
        // which is O(checkpoint_interval) by design and restores all fields.
        {
            // 3. Slow path: reload from snapshot and replay to rollback point.
            //
            // Find the best ledger snapshot at or before the rollback point.
            // Try epoch-numbered snapshots first (newest that's <= rollback_slot),
            // then fall back to the latest snapshot.
            // Pass the ChainDB to verify that candidate snapshots are on the
            // canonical chain — fork snapshots must not be used as rollback
            // base states (they would corrupt UTxO state permanently).
            let best_snapshot = {
                let db = self.chain_db.read().await;
                self.find_best_snapshot_for_rollback(rollback_slot, Some(&*db))
            };

            if let Some(snapshot_path) = best_snapshot {
                match dugite_ledger::LedgerState::load_snapshot(&snapshot_path) {
                    Ok(snapshot_state) => {
                        let snapshot_slot =
                            snapshot_state.tip.point.slot().map(|s| s.0).unwrap_or(0);

                        // ─────────────────────────────────────────────────────
                        // CRITICAL: UTxO store must be rebuilt from the snapshot,
                        // NOT reused from the pre-rollback state.
                        //
                        // The previous approach (detach + re-attach the live store)
                        // was fundamentally broken:
                        //
                        //   1. The live store contains UTxOs from blocks BEYOND the
                        //      rollback point (the blocks we just rolled back).
                        //   2. Re-attaching it and then replaying snapshot→rollback
                        //      re-inserts outputs but never removes the stale outputs
                        //      from the rolled-back blocks.
                        //   3. The UTxO store permanently diverges from the canonical
                        //      chain: stale UTxOs (from rolled-back blocks) remain
                        //      forever because they are not tracked in any diff.
                        //   4. On subsequent blocks, inputs spending those stale UTxOs
                        //      succeed in our store when they should fail (double-spend
                        //      from our node's perspective) — or conversely, legitimate
                        //      inputs from blocks we haven't applied yet appear missing
                        //      because the live store's diff context is wrong.
                        //
                        // The CORRECT approach:
                        //   - If we have an LSM UTxO snapshot ("ledger") saved at or
                        //     near the ledger snapshot point, restore the UTxO store
                        //     from that snapshot.  It reflects the exact UTxO set at
                        //     the snapshot slot — no stale entries.
                        //   - Then replay ApplyOnly from snapshot_slot → rollback_slot
                        //     to add the blocks we need to re-apply.
                        //
                        // The "ledger" UTxO snapshot is written by save_utxo_snapshot()
                        // at the same time as each ledger snapshot, so they are always
                        // in sync.
                        //
                        // If no UTxO snapshot exists (e.g., in-memory mode or very
                        // first run), fall back to reset_ledger_and_replay which does
                        // a full genesis replay — expensive but correct.
                        // ─────────────────────────────────────────────────────
                        let utxo_store_path = self.database_path.join("utxo-store");
                        let restored_utxo_store = if utxo_store_path.exists() {
                            // Try to open the UTxO store from the saved "ledger" LSM snapshot.
                            // This snapshot reflects the exact UTxO set at snapshot_slot.
                            match dugite_ledger::utxo_store::UtxoStore::open_from_snapshot(
                                &utxo_store_path,
                                "ledger",
                            ) {
                                Ok(mut store) => {
                                    // Rebuild the address index and count from the restored snapshot.
                                    store.count_entries();
                                    store.set_indexing_enabled(true);
                                    store.rebuild_address_index();
                                    info!(
                                        snapshot_slot,
                                        utxos = store.len(),
                                        "UTxO store restored from LSM snapshot for rollback"
                                    );
                                    Some(store)
                                }
                                Err(e) => {
                                    warn!(
                                        "Failed to open UTxO store from snapshot for rollback: {e} \
                                         — falling back to full reset+replay"
                                    );
                                    None
                                }
                            }
                        } else {
                            None // No on-disk UTxO store; in-memory mode uses bincode snapshot
                        };

                        let mut ls = self.ledger_state.write().await;

                        if let Some(store) = restored_utxo_store {
                            // LSM mode: replace ledger state and attach the correct UTxO store.
                            *ls = snapshot_state;
                            ls.attach_utxo_store(store);
                        } else if utxo_store_path.exists() {
                            // LSM store path exists but snapshot open failed — do full reset.
                            drop(ls);
                            error!(
                                rollback_slot,
                                "UTxO store snapshot unavailable for rollback — \
                                 performing full genesis reset+replay to ensure correctness"
                            );
                            self.reset_ledger_and_replay(rollback_slot).await;
                            // reset_ledger_and_replay acquires the write lock internally;
                            // we must not hold `ls` here.  Skip the replay below.
                            // Note: this path is handled by returning early from the else branch.
                            // The goto-style logic is unavoidable here without restructuring —
                            // set a flag and fall through to the mempool cleanup below.
                            //
                            // For simplicity, we just fall through; the ledger state
                            // will be at rollback_slot after reset_ledger_and_replay.
                            // The replay in the block below will be a no-op since
                            // reset_ledger_and_replay already replayed to rollback_slot.
                            return; // exit handle_rollback entirely; caller will reconnect
                        } else {
                            // Pure in-memory mode (no UTxO store file): the bincode snapshot
                            // contains all UTxOs.  Detach the current (stale) store so the
                            // snapshot's in-memory UTxOs take precedence.
                            let _ = ls.utxo.utxo_set.detach_store();
                            *ls = snapshot_state;
                        }

                        let replay_from = snapshot_slot;

                        // Replay blocks from snapshot tip to rollback point.
                        // In-memory UTxOs from the snapshot are correct at snapshot_slot;
                        // LSM store has been restored from its matching snapshot.
                        // ApplyOnly mode correctly inserts all outputs without re-running
                        // validation, ensuring the UTxO set is canonical at rollback_slot.
                        let db = self.chain_db.read().await;
                        let mut current_slot = replay_from;
                        let mut replayed = 0u64;
                        while current_slot < rollback_slot {
                            match db.get_next_block_after_slot(dugite_primitives::time::SlotNo(
                                current_slot,
                            )) {
                                Ok(Some((next_slot, _hash, cbor))) => {
                                    if next_slot.0 > rollback_slot {
                                        break;
                                    }
                                    // Minimal decode: rollback replay uses ApplyOnly
                                    // mode, so witness-set data is never read.
                                    match dugite_serialization::multi_era::decode_block_minimal_with_byron_epoch_length(&cbor, self.byron_epoch_length) {
                                        Ok(block) => {
                                            if let Err(e) = ls.apply_block(&block, BlockValidationMode::ApplyOnly) {
                                                error!(
                                                    slot = next_slot.0,
                                                    "Ledger apply failed during rollback replay: {e} — aborting replay"
                                                );
                                                break;
                                            }
                                            replayed += 1;
                                            current_slot = next_slot.0;
                                        }
                                        Err(e) => {
                                            warn!("Failed to decode block during replay: {e}");
                                            break;
                                        }
                                    }
                                }
                                Ok(None) => break,
                                Err(e) => {
                                    warn!("Failed to read block during replay: {e}");
                                    break;
                                }
                            }
                        }
                        info!(
                            snapshot_slot,
                            rollback_slot,
                            replayed,
                            snapshot = %snapshot_path.display(),
                            "Ledger state restored from snapshot and replayed to rollback point"
                        );
                    }
                    Err(e) => {
                        error!("Failed to load ledger snapshot for rollback: {e}");
                        self.reset_ledger_and_replay(rollback_slot).await;
                    }
                }
            } else {
                // No suitable ledger snapshot found for rollback.
                //
                // Per Ouroboros, a rollback of more than k=2160 blocks should never
                // happen in normal operation.  Calling reset_ledger_and_replay() is
                // dangerous when the UTxO store on disk is from a fork chain — doing
                // so re-attaches the stale fork store (now fixed in Fix 2) but also
                // triggers a multi-hour genesis replay that masks the root cause.
                //
                // When no canonical snapshot exists before the rollback target, the
                // most likely cause is that ALL retained epoch snapshots were saved on
                // a fork chain (RC4 in the cascade analysis).  In this case:
                //   1. Fix 1 (fork snapshot detection at startup) should have prevented
                //      the fork snapshot from loading in the first place.
                //   2. Fix 3 (ImmutableDB tip as intersection candidate) should have
                //      prevented the 97K-slot rollback from occurring at all.
                //
                // If we somehow reach here without a canonical snapshot, warn loudly
                // and return without corrupting state.  The ChainSync task will
                // disconnect; on reconnect, the corrected intersection logic (Fix 3)
                // will negotiate from the ImmutableDB tip instead.
                //
                // Note: reset_ledger_and_replay() itself is now safer (Fix 2) but
                // a full genesis replay is still extremely expensive and masks bugs.
                // Prefer the "warn and disconnect" path here so the operator can
                // diagnose the root cause (usually: restart the node to trigger Fix 1).
                let ledger_slot = self
                    .ledger_state
                    .read()
                    .await
                    .tip
                    .point
                    .slot()
                    .map(|s| s.0)
                    .unwrap_or(0);
                warn!(
                    rollback_slot,
                    ledger_slot,
                    "Deep rollback: no canonical snapshot found before rollback target. \
                     Refusing genesis reset to prevent state corruption. \
                     This node likely started with a fork snapshot — restart the node \
                     to trigger fork-snapshot detection and canonical replay. \
                     ChainSync will disconnect and retry from the ImmutableDB tip."
                );
                // Do NOT call reset_ledger_and_replay here.
                return;
            }
        }

        // ── Phase 4: Update chain fragment on rollback ───────────────────────
        //
        // Roll back the chain fragment to the rollback point so that the
        // fragment stays in sync with the ChainDB.  Downstream ChainSync peers
        // that are following our chain will be sent a MsgRollBackward by the
        // `notify_rollback` call below; the fragment must reflect the new tip
        // before that happens so that subsequent `find_intersect` queries
        // return correct results.
        {
            let mut fragment = self.chain_fragment.write().await;
            fragment.rollback_to(rollback_point);
        }

        // 4. Re-validate mempool transactions against the rolled-back ledger state.
        // Drain all pending txs, then re-validate each against the updated UTxO set.
        let pending_txs = self.mempool.drain_all();
        let pending_count = pending_txs.len();
        if pending_count > 0 {
            let ledger = self.ledger_state.read().await;
            let current_slot = ledger.tip.point.slot().map(|s| s.0).unwrap_or(0);
            let slot_config = ledger.slot_config;
            let mut revalidated = 0u64;
            for tx in pending_txs {
                let tx_size = tx.raw_cbor.as_ref().map(|b| b.len() as u64).unwrap_or(0);
                if dugite_ledger::validation::validate_transaction(
                    &tx,
                    &ledger.utxo.utxo_set,
                    &ledger.epochs.protocol_params,
                    current_slot,
                    tx_size,
                    Some(&slot_config),
                )
                .is_ok()
                {
                    let hash = tx.hash;
                    let size = tx.raw_cbor.as_ref().map(|b| b.len()).unwrap_or(0);
                    let fee = tx.body.fee;
                    let _ = self.mempool.add_tx_with_fee(hash, tx, size, fee);
                    revalidated += 1;
                }
            }
            info!(
                total = pending_count,
                revalidated, "Re-validated mempool txs after rollback"
            );
        }

        // 5. Notify peers
        self.notify_rollback(rollback_point).await;
    }

    /// Process a batch of forward blocks: store in ChainDB, apply to ledger, validate, log progress.
    ///
    /// Returns the number of blocks successfully applied to the ledger (0 if the first block
    /// failed connectivity, indicating a state divergence that the caller should handle).
    #[allow(clippy::too_many_arguments, dead_code)] // retained for networking rewrite
    pub async fn process_forward_blocks(
        &mut self,
        mut blocks: Vec<dugite_primitives::block::Block>,
        tip: &dugite_primitives::block::Tip,
        ebb_hashes: &[EbbInfo],
        blocks_received: &mut u64,
        blocks_since_last_log: &mut u64,
        last_snapshot_epoch: &mut u64,
        last_log_time: &mut std::time::Instant,
        last_query_update: &mut std::time::Instant,
    ) -> u64 {
        if blocks.is_empty() {
            return 0;
        }

        // Genesis block validation: on the very first batch of blocks received
        // during initial sync, verify that the genesis block hash matches the
        // expected hash from the configuration. This prevents syncing from a
        // chain with a different genesis (wrong network).
        if !self.genesis_validated {
            if let Err(e) = self.validate_genesis_blocks(&blocks) {
                error!("Genesis block validation failed: {e}");
                return 0;
            }
            self.genesis_validated = true;
        }

        // Validate ALL block headers BEFORE storing.
        // Two-phase validation matching Haskell's cardano-node:
        //
        // During initial sync (non-strict), use Replay mode — skip all cryptographic
        // verification (VRF, KES, opcert Ed25519). This matches Haskell's
        // `reupdateChainDepState` behavior for blocks from the immutable chain.
        // Historical blocks are validated by hash-chain connectivity.
        //
        // At tip (strict), use Full mode with parallel crypto verification via rayon.
        // This matches Haskell's `updateChainDepState` for new network blocks.
        let strict = self.consensus.strict_verification();
        let mode = if strict {
            ValidationMode::Full
        } else {
            ValidationMode::Replay
        };
        {
            // Read ledger state once for the whole batch
            let ls = self.ledger_state.read().await;

            // Per Praos spec, leader eligibility uses the "set" snapshot
            // (stake distribution from the previous epoch boundary).
            // Fall back to current pool_params if snapshots aren't available yet.
            let set_snapshot = ls.epochs.snapshots.set.as_ref();
            let total_active_stake: u64 = if let Some(snap) = set_snapshot {
                snap.pool_stake.values().map(|s| s.0).sum()
            } else {
                // During early sync, no snapshots exist yet — skip leader eligibility
                0
            };

            // Build overlay context for BFT schedule validation.
            // Only needed when d > 0 and protocol version < 7 (pre-Babbage).
            // For Babbage+ (proto >= 7), d is always 0 and overlay is skipped.
            let overlay_ctx = if ls.epochs.protocol_params.protocol_version_major < 7
                && ls.epochs.protocol_params.d.numerator > 0
                && !ls.genesis_delegates.is_empty()
            {
                let epoch = ls.epoch_of_slot(blocks.first().map(|b| b.slot().0).unwrap_or(0));
                let first_slot = ls.first_slot_of_epoch(epoch);
                let genesis_keys: std::collections::BTreeSet<dugite_primitives::hash::Hash28> =
                    ls.genesis_delegates.keys().copied().collect();
                Some(dugite_consensus::overlay::OverlayContext {
                    genesis_delegates: ls.genesis_delegates.clone(),
                    genesis_keys,
                    d: (
                        ls.epochs.protocol_params.d.numerator,
                        ls.epochs.protocol_params.d.denominator,
                    ),
                    first_slot_of_epoch: first_slot,
                })
            } else {
                None
            };

            // Phase 1: Sequential structural validation + state updates.
            // Uses Replay mode during sync (skip crypto) or Full mode at tip.
            // Opcert counter tracking and structural checks always run.
            for block in &blocks {
                if !block.era.is_shelley_based() {
                    continue;
                }

                // Populate epoch_nonce — the wire format does not include the nonce;
                // it must be injected from ledger state before VRF verification.
                //
                // A single `epoch_nonce` snapshot at batch-start is WRONG when the
                // batch spans an epoch boundary: the first block of the new epoch must
                // be validated with the NEW epoch's nonce (computed by the TICKN rule),
                // not the old one.  `epoch_nonce_for_slot` pre-computes the correct
                // nonce for any block that crosses into the immediately-next epoch,
                // mirroring the TICKN logic in `process_epoch_transition` without
                // mutating any state.  This fixes the "stale nonce after restart"
                // VRF failure that permanently blocked epoch transitions:
                //
                //   1. Node restarts, replays immutable blocks → strict verification on
                //   2. First live block is the first block of epoch E+1
                //   3. Old code used epoch E nonce → VRF failure → batch rejected
                //   4. Ledger never advanced → epoch E+1 nonce never computed → stuck
                let epoch_nonce = ls.epoch_nonce_for_slot(block.slot().0);
                let mut header_with_nonce = block.header.clone();
                header_with_nonce.epoch_nonce = epoch_nonce;

                // Look up pool registration for VRF key binding and leader eligibility.
                // Uses "set" snapshot for stake (per Praos spec), falls back to current
                // pool_params for VRF key binding if snapshot is not available.
                let pool_id = dugite_primitives::hash::blake2b_224(&block.header.issuer_vkey);
                let issuer_info = if !block.header.issuer_vkey.is_empty() {
                    // Try set snapshot first (correct per spec)
                    let pool_reg = set_snapshot
                        .and_then(|snap| snap.pool_params.get(&pool_id))
                        .or_else(|| ls.certs.pool_params.get(&pool_id));

                    pool_reg.map(|reg| {
                        if total_active_stake == 0 {
                            // No snapshot data — just do VRF key binding, skip leader check.
                            // Use 1/1 (= 100%) so the pool passes the threshold trivially.
                            return BlockIssuerInfo {
                                vrf_keyhash: reg.vrf_keyhash,
                                pool_stake: 1,
                                total_active_stake: 1,
                            };
                        }
                        let pool_stake = set_snapshot
                            .and_then(|snap| snap.pool_stake.get(&pool_id))
                            .map(|s| s.0)
                            .unwrap_or(0);
                        BlockIssuerInfo {
                            vrf_keyhash: reg.vrf_keyhash,
                            pool_stake,
                            total_active_stake,
                        }
                    })
                } else {
                    None
                };

                // Envelope checks (Haskell's `envelopeChecks`): body size and
                // optional header size against protocol parameter limits.
                // These are always fatal — no strict/non-strict bypass.
                if let Err(e) = self.consensus.validate_envelope(
                    block.slot(),
                    block.header.body_size,
                    None, // header CBOR size not available during ChainSync header processing
                    ls.epochs.protocol_params.max_block_body_size,
                    ls.epochs.protocol_params.max_block_header_size,
                ) {
                    error!(
                        slot = block.slot().0,
                        block_no = block.block_number().0,
                        "Envelope check failed: {e} — rejecting batch"
                    );
                    return 0;
                }

                if let Err(e) = self.consensus.validate_header_full(
                    &header_with_nonce,
                    block.slot(),
                    issuer_info.as_ref(),
                    overlay_ctx.as_ref(),
                    mode,
                    Some(ls.epochs.protocol_params.protocol_version_major),
                ) {
                    if strict {
                        error!(
                            slot = block.slot().0,
                            block_no = block.block_number().0,
                            "Consensus validation failed (strict): {e} — rejecting batch"
                        );
                        return 0;
                    }
                    warn!(
                        slot = block.slot().0,
                        block_no = block.block_number().0,
                        "Consensus validation: {e}"
                    );
                }
            }
        }

        let batch_count = blocks.len() as u64;

        // Build ChainDB batch data, taking ownership of raw_cbor to avoid cloning
        let db_batch: Vec<_> = blocks
            .iter_mut()
            .map(|block| {
                (
                    *block.hash(),
                    block.slot(),
                    block.block_number(),
                    *block.prev_hash(),
                    block.raw_cbor.take().unwrap_or_default(),
                )
            })
            .collect();

        // Refuse new blocks when disk space is fatally low to protect data integrity.
        // The node stays alive so it can still serve queries.
        if *self.disk_space_rx.borrow() == crate::disk_monitor::DiskSpaceLevel::Fatal {
            error!(
                "Disk space critically low — refusing to store {} blocks to protect data integrity",
                blocks.len()
            );
            return 0;
        }

        // ── Phase 3: Store blocks to ChainDB FIRST, then apply to ledger ───
        //
        // At tip (strict mode), submit each block through the ChainSelQueue.
        // The queue writes to VolatileDB sequentially, matching the Haskell
        // `addBlockRunner` pattern.  For bulk sync (non-strict), keep the
        // existing batch write path for performance — the queue would be too
        // slow for 4M blocks during fast sync.
        //
        // In both cases, the ledger apply continues directly below (chain
        // selection is not yet fully live in the queue runner), and the
        // chain fragment is updated for each successfully stored block.
        if strict {
            // Live blocks at tip: route through ChainSelQueue for
            // Haskell-compatible sequential processing.
            if let Some(ref handle) = self.chain_sel_handle {
                for (hash, slot, block_no, prev_hash, cbor) in db_batch {
                    match handle
                        .submit_block(hash, slot, block_no, prev_hash, cbor)
                        .await
                    {
                        Some(dugite_storage::AddBlockResult::AdoptedAsTip)
                        | Some(dugite_storage::AddBlockResult::StoredNotAdopted)
                        | Some(dugite_storage::AddBlockResult::AlreadyKnown) => {
                            // Block stored — proceed to ledger apply below.
                        }
                        Some(dugite_storage::AddBlockResult::SwitchedToFork {
                            intersection_hash,
                            rollback,
                            apply,
                        }) => {
                            // Chain selection switched to a longer fork.  The
                            // VolatileDB is already on the new chain; log the
                            // event.  Phase 3 will wire the full ledger rollback
                            // + replay here.
                            info!(
                                intersection = %intersection_hash.to_hex(),
                                slot = slot.0,
                                rollback_count = rollback.len(),
                                apply_count = apply.len(),
                                "Chain selection: fork switch during sync (Phase 3 pending)"
                            );
                            // Count fork switches for observability.
                            self.metrics
                                .rollback_count
                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            // Block is stored; continue to the ledger apply below.
                        }
                        Some(dugite_storage::AddBlockResult::Invalid(reason)) => {
                            error!(
                                slot = slot.0,
                                reason,
                                "FATAL: ChainSelQueue rejected live block — halting to prevent divergence"
                            );
                            return 0;
                        }
                        None => {
                            error!("FATAL: ChainSelQueue runner exited unexpectedly");
                            return 0;
                        }
                    }
                }
            } else {
                // Fallback: no handle, use batch write.
                let mut db = self.chain_db.write().await;
                if let Err(e) = db.add_blocks_batch(db_batch) {
                    error!(
                        "FATAL: Failed to store block batch: {e} — halting to prevent state divergence"
                    );
                    return 0;
                }
            }
        } else {
            // Bulk sync: keep the fast batch path.  ChainSelQueue overhead
            // (one round-trip per block through an async channel) would
            // reduce throughput from ~10K blk/s to ~1K blk/s or worse.
            let mut db = self.chain_db.write().await;
            if let Err(e) = db.add_blocks_batch(db_batch) {
                error!(
                    "FATAL: Failed to store block batch: {e} — halting to prevent state divergence"
                );
                return 0;
            }
        }

        // Compute the Limit on Eagerness (LoE) slot ceiling ONCE here, before
        // acquiring any other locks.  This value is used in two places:
        //
        // 1. The ledger apply loop below — blocks with slot > loe_slot are
        //    skipped so the ledger state cannot advance past the LoE boundary.
        //    They remain in VolatileDB and will be applied when the GSM later
        //    transitions to CaughtUp and the constraint is lifted.
        //
        // 2. The volatile→immutable flush at the end of this function, which
        //    similarly must not promote blocks beyond the LoE slot.
        //
        // In Praos mode (genesis disabled) the GSM starts in CaughtUp and
        // loe_limit() always returns None, so both paths take the fast branch
        // with zero overhead.
        let loe_limit: Option<u64> = self.gsm_snapshot_rx.borrow().loe_slot;

        // Now apply blocks to ledger — storage is confirmed
        let mut applied_count: u64 = 0;
        let mut collected_deltas: Vec<dugite_ledger::ledger_seq::LedgerDelta> = Vec::new();
        {
            let mut ls = self.ledger_state.write().await;
            let ledger_slot = ls.tip.point.slot().map(|s| s.0).unwrap_or(0);
            if !blocks.is_empty() {
                debug!(
                    batch_size = blocks.len(),
                    ledger_slot,
                    first_slot = blocks[0].slot().0,
                    first_block = blocks[0].block_number().0,
                    first_prev_hash = %blocks[0].prev_hash().to_hex(),
                    ledger_tip_hash = %ls.tip.point.hash().map(|h| h.to_hex()).unwrap_or_default(),
                    "Applying block batch to ledger"
                );
            }

            // Gap bridging: if the first unskipped block doesn't connect to the
            // ledger tip, try to replay intermediate blocks from ChainDB storage.
            // This handles the case where ChainDB is ahead of the ledger (e.g.,
            // after a crash mid-batch, or when blocks were stored but ledger
            // apply failed in a previous iteration).
            if let Some(first_new) = blocks.iter().find(|b| b.slot().0 > ledger_slot) {
                let ledger_tip_hash = ls.tip.point.hash().cloned();
                let first_prev = first_new.prev_hash();
                if ledger_tip_hash.as_ref() != Some(first_prev) {
                    debug!(
                        "Gap detected (ledger slot={}, first block slot={}) — bridging from ChainDB",
                        ledger_slot, first_new.slot().0,
                    );
                    let mut bridge_slot = ledger_slot;
                    let target_slot = first_new.slot().0;
                    let mut bridged = 0u64;
                    let mut bridge_failed = false;
                    loop {
                        let block_data = {
                            let db = self.chain_db.read().await;
                            db.get_next_block_after_slot(dugite_primitives::time::SlotNo(
                                bridge_slot,
                            ))
                        };
                        match block_data {
                            Ok(Some((next_slot, _hash, cbor))) => {
                                if next_slot.0 >= target_slot {
                                    break; // Reached the incoming batch
                                }
                                // Minimal decode: gap-bridge replay uses ApplyOnly
                                // mode, so witness-set data is never read.
                                match dugite_serialization::multi_era::decode_block_minimal_with_byron_epoch_length(&cbor, self.byron_epoch_length) {
                                    Ok(block) => {
                                        // Verify the block connects to the ledger tip
                                        // before applying.  ImmutableDB may contain
                                        // contaminated blocks from a prior fork that
                                        // was flushed on shutdown — skip those.
                                        let current_tip = ls.tip.point.hash().cloned();
                                        if current_tip.as_ref() != Some(block.prev_hash()) {
                                            debug!(
                                                slot = next_slot.0,
                                                expected = current_tip.map(|h| h.to_hex()).unwrap_or_default(),
                                                got = block.prev_hash().to_hex(),
                                                "Gap bridge: skipping non-connecting block (likely fork contamination)"
                                            );
                                            bridge_slot = next_slot.0;
                                            continue; // Skip, try next block
                                        }
                                        if let Err(e) = ls.apply_block(&block, BlockValidationMode::ApplyOnly) {
                                            warn!(
                                                slot = next_slot.0,
                                                "Gap bridge apply failed: {e} — \
                                                 ChainDB may have blocks from a different fork"
                                            );
                                            bridge_failed = true;
                                            break;
                                        }
                                        bridged += 1;
                                        bridge_slot = next_slot.0;
                                    }
                                    Err(e) => {
                                        warn!(slot = next_slot.0, error = %e, "Gap bridge decode failed");
                                        bridge_slot = next_slot.0;
                                    }
                                }
                            }
                            _ => break,
                        }
                    }
                    if bridged > 0 {
                        debug!("Bridged {bridged} blocks from ChainDB storage");
                    }
                    if bridge_failed {
                        // Gap bridge found a block that decoded but failed
                        // apply (not just a prev_hash mismatch).  Clear
                        // volatile and retry.
                        {
                            let mut db = self.chain_db.write().await;
                            let removed = db.volatile_block_count();
                            db.clear_volatile();
                            warn!(
                                removed,
                                "Gap bridge failed — cleared volatile DB. Re-syncing."
                            );
                        }
                        return 0;
                    }
                }
            }

            let ledger_slot = ls.tip.point.slot().map(|s| s.0).unwrap_or(0);
            let ledger_tip_hash = ls.tip.point.hash().cloned();
            for block in &blocks {
                // Skip blocks the ledger has already applied (e.g. replaying from origin).
                // After a rollback/fork, a block at the same slot but with a different
                // prev_hash must NOT be skipped — it belongs to the new fork.
                if block.slot().0 <= ledger_slot {
                    let is_fork_block = ledger_tip_hash
                        .as_ref()
                        .is_some_and(|tip_hash| tip_hash == block.prev_hash());
                    if !is_fork_block {
                        continue;
                    }
                }

                // LoE guard: when the Genesis State Machine is in PreSyncing or
                // Syncing state, do not apply blocks whose slot exceeds the LoE
                // ceiling.  Those blocks are already in VolatileDB (stored above)
                // and will be applied once the GSM transitions to CaughtUp.
                //
                // Because blocks are delivered in slot order, the first block that
                // exceeds the ceiling means all subsequent ones will too — break
                // rather than continue so we don't scan the rest of the batch.
                if let Some(loe_slot) = loe_limit {
                    if block.slot().0 > loe_slot {
                        debug!(
                            slot = block.slot().0,
                            loe_slot,
                            "LoE: deferring ledger application of blocks beyond LoE ceiling"
                        );
                        break;
                    }
                }

                // Byron EBB bridge: before applying a block, check if its
                // prev_hash references a Byron Epoch Boundary Block (EBB)
                // rather than the current ledger tip.  EBBs carry no transactions
                // and are never fetched via BlockFetch, so they are not in `blocks`.
                // Their hashes are tracked in `ebb_hashes` and used here to advance
                // the ledger tip before applying the block that follows the EBB.
                //
                // This handles mainnet Byron epochs 0-207: each epoch boundary
                // produces one EBB whose hash becomes the prev_hash of the first
                // real block of the next epoch.
                let current_tip_hash = ls.tip.point.hash().cloned();
                if current_tip_hash.as_ref() != Some(block.prev_hash()) {
                    // Check if this block's prev_hash matches any EBB in the batch.
                    let ebb_match = ebb_hashes
                        .iter()
                        .find(|ebb| ebb.hash == *block.prev_hash().as_bytes());
                    if let Some(ebb) = ebb_match {
                        use dugite_primitives::hash::Hash32;
                        let ebb_hash = Hash32::from_bytes(ebb.hash);
                        debug!(
                            ebb_hash = %ebb_hash.to_hex(),
                            block_slot = block.slot().0,
                            block_no = block.block_number().0,
                            "Advancing ledger tip through Byron EBB before block application"
                        );
                        if let Err(e) = ls.advance_past_ebb(ebb_hash) {
                            warn!(
                                slot = block.slot().0,
                                "EBB advance failed: {e} — skipping block"
                            );
                            break;
                        }
                    }
                }

                let ledger_mode = if strict || self.validate_all_blocks {
                    BlockValidationMode::ValidateAll
                } else {
                    BlockValidationMode::ApplyOnly
                };
                match ls.apply_block_with_delta(block, ledger_mode) {
                    Ok(delta) => {
                        collected_deltas.push(delta);
                    }
                    Err(e) => {
                        error!(
                            slot = block.slot().0,
                            block_no = block.block_number().0,
                            hash = %block.hash().to_hex(),
                            "Failed to apply block to ledger: {e} — skipping remaining blocks in batch"
                        );
                        break;
                    }
                }
                // Consume pending era transition and propagate to the HFC state machine.
                if let Some((prev_era, new_era, epoch)) = ls.pending_era_transition.take() {
                    let mut eh = self.era_history.write().await;
                    if eh.current_era() < new_era {
                        eh.record_era_transition(new_era, epoch.0);
                        tracing::info!(
                            prev = %prev_era,
                            new = %new_era,
                            epoch = epoch.0,
                            "Era transition recorded in HFC era history",
                        );
                    }
                }
                applied_count += 1;
            }
        }

        // Push collected deltas to LedgerSeq (after releasing ledger_state lock).
        if !collected_deltas.is_empty() {
            let mut seq = self.ledger_seq.write().await;
            for delta in collected_deltas {
                seq.push(delta);
            }
        }

        // ── Phase 3: Update chain fragment for all applied blocks ────────────
        //
        // After the ledger apply loop, update the chain fragment with the
        // headers of blocks that were successfully applied.  This keeps the
        // fragment in sync with the selected chain so:
        //   1. ChainSync servers can find correct intersection points for
        //      downstream peers.
        //   2. The background copy-to-immutable can compare fragment.length()
        //      against k to decide when to flush to ImmutableDB.
        //
        // We use `applied_count` to only add headers for blocks that were
        // actually applied to the ledger, not the full batch (some may have
        // been skipped due to LoE or failed applies).
        if applied_count > 0 {
            let mut fragment = self.chain_fragment.write().await;
            let skip = blocks.len().saturating_sub(applied_count as usize);
            for block in blocks.iter().skip(skip) {
                fragment.push(block.header.clone());
            }
        }

        // ── Phase 5: Background maintenance operations ────────────────────────
        //
        // After updating the fragment, run the three background operations
        // that keep storage healthy.  These mirror Haskell's Background.hs:
        //
        // 1. copy_to_immutable — if fragment.len() > k, copy the oldest
        //    volatile block to ImmutableDB and advance the ledger anchor.
        // 2. gc_scheduler — remove expired volatile entries (60s delay).
        // 3. bg_snapshot_scheduler — take a ledger snapshot if warranted.
        //
        // We run these for EVERY batch (not just at tip) so the ImmutableDB
        // advances steadily during bulk sync and the GC queue drains promptly.
        // The copy-to-immutable check is O(1) (compare two integers) so the
        // overhead is negligible even during 10K blk/s bulk sync.
        if applied_count > 0 {
            // --- copy-to-immutable & GC ---
            // Get fragment metadata (oldest hash/slot/block_no) BEFORE
            // acquiring the ChainDB write lock, to avoid holding two locks.
            let fragment_info = {
                let frag = self.chain_fragment.read().await;
                if frag.length() > 0 {
                    // Oldest header (front of the deque)
                    frag.oldest_header()
                        .map(|h| (frag.length(), h.header_hash, h.slot, h.block_number))
                } else {
                    None
                }
            };

            if let Some((frag_len, oldest_hash, oldest_slot, oldest_block_no)) = fragment_info {
                let now = std::time::Instant::now();

                // Run copy-to-immutable + GC under a single ChainDB write lock.
                let copied = {
                    let mut db = self.chain_db.write().await;
                    // copy_to_immutable: moves oldest block if frag_len > k.
                    let copied = self
                        .copy_to_immutable
                        .run_once(
                            &mut db,
                            frag_len,
                            oldest_hash,
                            oldest_slot,
                            oldest_block_no,
                            &mut |_slot, _hash, _block_no| {
                                // LedgerSeq anchor advance and DiffSeq flush happen
                                // after this callback returns (below), since the
                                // callback is sync but our locks are async.
                            },
                        )
                        .unwrap_or_else(|e| {
                            warn!(error = %e, "background: copy-to-immutable failed");
                            None
                        });

                    // gc_scheduler: remove blocks past their 60s GC delay.
                    self.gc_scheduler.run_pending(&mut db, now);

                    copied
                };

                // If a block was copied, schedule it for GC after GC_DELAY.
                if let Some((gc_slot, gc_hash)) = copied {
                    self.gc_scheduler.schedule(gc_slot, gc_hash, now);
                    // The fragment's oldest header was promoted — pop it.
                    let mut frag = self.chain_fragment.write().await;
                    frag.pop_oldest();

                    // Advance LedgerSeq anchor — the oldest volatile delta
                    // is now immutable and can be absorbed into the anchor.
                    self.ledger_seq.write().await.advance_anchor();

                    // Flush DiffSeq entries for the now-immutable block.
                    // These diffs can never be rolled back, so keeping them
                    // wastes memory. Combined with push_bounded in apply_block,
                    // this ensures DiffSeq stays at most k entries.
                    let mut ls = self.ledger_state.write().await;
                    ls.utxo.diff_seq.flush_up_to(gc_slot);
                }
            }

            // --- snapshot scheduler ---
            // Check whether a snapshot should be taken.  Use the last applied
            // block's epoch number to detect epoch-boundary triggers.
            let last_applied = blocks.iter().rev().take(applied_count as usize).next();
            if let Some(last_block) = last_applied {
                let current_epoch = {
                    let ls = self.ledger_state.read().await;
                    ls.epoch
                };
                let block_no = last_block.block_number();
                // Clone self.bg_snapshot_scheduler to satisfy borrow checker
                // (can't have mut borrow of self.bg_snapshot_scheduler while
                //  also borrowing self.save_ledger_snapshot).  Use a flag
                // pattern to avoid the double-borrow.
                let should_snapshot = {
                    self.bg_snapshot_scheduler
                        .maybe_snapshot_check(current_epoch, block_no)
                };
                if should_snapshot {
                    self.save_ledger_snapshot().await;
                    self.bg_snapshot_scheduler
                        .record_snapshot_taken(current_epoch);
                }
            }
        }

        // Revalidate mempool transactions against the updated ledger state.
        //
        // Per the Haskell spec (pureSyncWithLedger → revalidateTxsFor → reapplyTxs),
        // ALL mempool txs are re-validated sequentially in FIFO order against the
        // new ticked ledger state. This naturally handles:
        //   - Confirmed txs (double-spend → removed)
        //   - Consumed input conflicts
        //   - TTL expiry
        //   - Cascading child removal (parent removed → child's input missing)
        //   - Any other validation rule changes
        if !self.mempool.is_empty() {
            // First remove confirmed txs by hash (fast path).
            let confirmed_hashes: Vec<_> = blocks
                .iter()
                .flat_map(|b| b.transactions.iter().map(|tx| tx.hash))
                .collect();
            if !confirmed_hashes.is_empty() {
                self.mempool.remove_txs(&confirmed_hashes);
            }

            // Full revalidation against the updated ledger state.
            // Build a set of consumed inputs for a quick first-pass check,
            // plus check TTL against the new tip slot.
            let consumed_inputs: std::collections::HashSet<_> = blocks
                .iter()
                .flat_map(|b| b.transactions.iter())
                .flat_map(|tx| tx.body.inputs.iter().cloned())
                .collect();
            let current_slot = blocks.last().map(|b| b.slot());

            // Also check if the tx's inputs exist in the on-chain UTxO set.
            // This catches chained txs whose parents were removed: their inputs
            // no longer exist in the UTxO set and mempool virtual UTxO.
            let ls = self.ledger_state.read().await;
            self.mempool.revalidate_all(|tx| {
                // Reject if any input was consumed by the new block
                if tx
                    .body
                    .inputs
                    .iter()
                    .any(|input| consumed_inputs.contains(input))
                {
                    return false;
                }
                // Reject if TTL has expired (half-open: slot >= ttl means expired)
                if let (Some(ttl), Some(slot)) = (tx.body.ttl, current_slot) {
                    if slot.0 >= ttl.0 {
                        return false;
                    }
                }
                // Reject if any input is not in on-chain UTxO or mempool virtual UTxO.
                // This catches orphaned chained txs whose parents were removed.
                for input in &tx.body.inputs {
                    if !ls.utxo.utxo_set.contains(input)
                        && self.mempool.lookup_virtual_utxo(input).is_none()
                    {
                        return false;
                    }
                }
                true
            });
            drop(ls);

            // Update mempool metrics immediately after revalidation so Prometheus
            // reflects tx removals (confirmed txs, TTL expiry, etc.) without
            // waiting for the periodic 5-second metric refresh.
            self.metrics.set_mempool_count(self.mempool.len() as u64);
            self.metrics.mempool_bytes.store(
                self.mempool.total_bytes() as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
        }

        if let Some(last_block) = blocks.last() {
            self.consensus.update_tip(last_block.tip());
        }

        // Flush finalized blocks from VolatileDB to ImmutableDB.
        //
        // Uses the same `loe_limit` computed before the ledger apply section.
        // When LoE is active the immutable tip cannot advance past the LoE
        // ceiling; blocks beyond that slot remain in VolatileDB (and were not
        // applied to the ledger above) until the GSM reaches CaughtUp.
        // In Praos mode (genesis disabled) `loe_limit` is always None.
        //
        // Flush finalized blocks from VolatileDB to ImmutableDB, then GC.
        //
        // This is split into batches of at most FLUSH_BATCH_SIZE blocks per
        // write-lock acquisition. Between batches we yield to the async
        // runtime so that other tasks (e.g. ChainSync server responding to
        // MsgFindIntersect on inbound N2N connections) can acquire read locks.
        // Without batching, the flush can hold the write lock for >10s during
        // bulk sync, causing Haskell peers to time out their ChainSync idle
        // timeout and drop the connection.
        const FLUSH_BATCH_SIZE: u64 = 50;
        loop {
            tokio::task::yield_now().await;
            let mut db = self.chain_db.write().await;
            let flush_result = match loe_limit {
                None => db.flush_to_immutable_batch(FLUSH_BATCH_SIZE),
                Some(loe_slot) => db.flush_to_immutable_loe_batch(loe_slot, FLUSH_BATCH_SIZE),
            };
            match flush_result {
                Ok(0) => break, // No more to flush
                Ok(_flushed) => {
                    // More blocks may remain — release lock and re-acquire.
                    drop(db);
                    continue;
                }
                Err(e) => {
                    warn!(error = %e, "Failed to flush blocks to immutable storage");
                    break;
                }
            }
        }
        {
            let mut db = self.chain_db.write().await;
            // GC orphaned fork blocks whose 60-second delay has expired.
            db.gc_volatile();
        }

        let tx_count: u64 = blocks.iter().map(|b| b.transactions.len() as u64).sum();

        *blocks_received += batch_count;
        *blocks_since_last_log += batch_count;
        self.snapshot_policy.record_blocks(batch_count);
        self.metrics.add_blocks_received(batch_count);
        self.metrics.record_block_received();
        self.metrics.record_roll_forward();
        self.metrics.add_blocks_applied(batch_count);
        self.metrics
            .transactions_received
            .fetch_add(tx_count, std::sync::atomic::Ordering::Relaxed);
        self.metrics
            .transactions_validated
            .fetch_add(tx_count, std::sync::atomic::Ordering::Relaxed);

        let last_block = blocks
            .last()
            // Safety: function returns early if blocks.is_empty()
            .expect("blocks is non-empty (checked at function entry)");
        let slot = last_block.slot().0;
        let block_no = last_block.block_number().0;
        self.metrics.set_slot(slot);
        self.metrics.set_block_number(block_no);

        // Log each new block when following the tip (individual blocks matter at tip)
        // and announce to connected downstream peers so they receive new blocks
        if strict {
            for block in &blocks {
                let hash_hex = block.hash().to_hex();
                info!(
                    era = %block.era,
                    slot = block.slot().0,
                    block = block.block_number().0,
                    txs = block.transactions.len(),
                    hash = %hash_hex,
                    "New block",
                );
            }

            // Announce the latest block to all connected N2N peers
            // This enables relay behavior: downstream peers waiting at tip (MsgAwaitReply)
            // will receive MsgRollForward for blocks we synced from upstream
            if let Some(ref tx) = self.block_announcement_tx {
                let mut hash_bytes = [0u8; 32];
                hash_bytes.copy_from_slice(last_block.hash().as_ref());
                tx.send(dugite_network::BlockAnnouncement {
                    slot,
                    hash: hash_bytes,
                    block_number: block_no,
                })
                .ok();
            }
        }

        {
            let current_epoch = self.ledger_state.read().await.epoch.0;
            if current_epoch > *last_snapshot_epoch {
                // Count ALL epoch transitions (batches may span multiple epochs)
                let epochs_crossed = (current_epoch - *last_snapshot_epoch) as u32;
                info!(
                    epoch = current_epoch,
                    crossed = epochs_crossed,
                    "Epoch transition",
                );
                self.live_epoch_transitions =
                    self.live_epoch_transitions.saturating_add(epochs_crossed);

                // Finalize immutable chunk at epoch boundary and persist.
                // Pass the new epoch's parameters for Haskell-compatible
                // chunk naming and primary index generation.
                {
                    let (next_epoch_length, next_epoch_first_slot) = {
                        let eh = self.era_history.read().await;
                        let epoch_no = dugite_primitives::EpochNo(current_epoch);
                        let length = eh.epoch_size(epoch_no).unwrap_or(432_000);
                        let first_slot = eh.epoch_first_slot(epoch_no).map(|s| s.0).unwrap_or(0);
                        (length, first_slot)
                    };
                    let mut db = self.chain_db.write().await;
                    if let Err(e) = db.finalize_immutable_chunk(
                        current_epoch,
                        next_epoch_length,
                        next_epoch_first_slot,
                    ) {
                        warn!(error = %e, "Failed to finalize immutable chunk at epoch transition");
                    }
                    match db.persist() {
                        Ok(()) => info!(
                            epoch = current_epoch,
                            "ChainDB persisted at epoch transition"
                        ),
                        Err(e) => {
                            warn!(error = %e, "Failed to persist ChainDB at epoch transition")
                        }
                    }
                }
                if self.snapshot_policy.should_snapshot_normal() {
                    self.save_ledger_snapshot().await;
                    self.snapshot_policy.snapshot_taken();
                }
                *last_snapshot_epoch = current_epoch;

                // Single read acquisition to cover both opcert pruning and
                // epoch-boundary mempool revalidation.  Combining these two
                // read-lock acquisitions into one eliminates the unlock/relock
                // round-trip and reduces contention with any concurrent writer
                // (e.g. the ledger-apply path above).
                //
                // The guard is held for the duration of the mempool closure
                // because the closure borrows `utxo_set` from it directly —
                // avoiding a potentially large clone of the UTxO map.
                {
                    let ledger = self.ledger_state.read().await;

                    // Prune opcert counters to only keep active pools (prevents
                    // unbounded growth as pools retire over epochs).
                    let active_pools: std::collections::HashSet<_> =
                        ledger.certs.pool_params.keys().copied().collect();
                    self.consensus.prune_opcert_counters(&active_pools);

                    // Update mempool capacity limits from the new epoch's protocol params.
                    //
                    // Haskell cardano-node sets mempool capacity to 2x the block's
                    // resource limits (`blockCapacityTxMeasure`).  Protocol params can
                    // change via governance actions at epoch boundaries, so we
                    // recalculate capacity here to stay in sync with the current limits.
                    // This must happen BEFORE revalidation so eviction uses the updated
                    // bounds when computing whether a tx still fits.
                    self.mempool.update_capacity_from_params(
                        ledger.epochs.protocol_params.max_block_body_size,
                        ledger.epochs.protocol_params.max_block_ex_units.mem,
                        ledger.epochs.protocol_params.max_block_ex_units.steps,
                    );

                    // Revalidate all mempool transactions against the new epoch's
                    // protocol parameters.  Protocol parameters can change at epoch
                    // boundaries (fee structure, max tx size, execution unit prices,
                    // etc.), so transactions that were valid in the previous epoch may
                    // now violate the new rules.  This mirrors Haskell cardano-node's
                    // epoch-boundary revalidation and is critical for block producers:
                    // forging a block with transactions that violate the new parameters
                    // would produce an invalid block.
                    if !self.mempool.is_empty() {
                        // Snapshot the scalar fields we need for the closure — these are
                        // cheap copies (params and slot_config are both small structs).
                        // We borrow utxo_set directly from the read-guard so we avoid
                        // cloning the potentially large UTxO map.
                        let new_params = ledger.epochs.protocol_params.clone();
                        let current_slot = ledger.tip.point.slot().map(|s| s.0).unwrap_or(0);
                        let slot_config = ledger.slot_config;
                        let utxo_ref = &ledger.utxo.utxo_set;
                        let evicted = self.mempool.revalidate_all(|tx| {
                            let tx_size = tx.raw_cbor.as_ref().map(|b| b.len() as u64).unwrap_or(0);
                            dugite_ledger::validation::validate_transaction(
                                tx,
                                utxo_ref,
                                &new_params,
                                current_slot,
                                tx_size,
                                Some(&slot_config),
                            )
                            .is_ok()
                        });
                        if !evicted.is_empty() {
                            info!(
                                epoch = current_epoch,
                                evicted = evicted.len(),
                                remaining = self.mempool.len(),
                                "Epoch boundary: evicted mempool transactions that violate new protocol parameters",
                            );
                        } else {
                            debug!(
                                epoch = current_epoch,
                                "Epoch boundary: all mempool transactions valid under new protocol parameters",
                            );
                        }
                    }
                }
            }
        }

        let elapsed = last_log_time.elapsed();
        if elapsed.as_secs() >= 5 || *blocks_received <= 5 {
            let tip_slot = tip.point.slot().map(|s| s.0).unwrap_or(0);
            let tip_block = tip.block_number.0;
            let progress = if tip_slot > 0 {
                (slot as f64 / tip_slot as f64 * 100.0).min(100.0)
            } else {
                0.0
            };
            let blocks_per_sec = if elapsed.as_secs_f64() > 0.0 {
                *blocks_since_last_log as f64 / elapsed.as_secs_f64()
            } else {
                0.0
            };
            let blocks_remaining = tip_block.saturating_sub(block_no);
            {
                let ls = self.ledger_state.read().await;
                self.metrics.set_epoch(ls.epoch.0);
                self.metrics.set_utxo_count(ls.utxo.utxo_set.len() as u64);
                self.metrics.set_sync_progress(progress);
                self.metrics.set_mempool_count(self.mempool.len() as u64);
                self.metrics.set_mempool_max(self.mempool.capacity() as u64);
                self.metrics.mempool_bytes.store(
                    self.mempool.total_bytes() as u64,
                    std::sync::atomic::Ordering::Relaxed,
                );
                {
                    let pm = self.peer_manager.read().await;
                    // Connected = warm + hot (both have live TCP connections).
                    self.metrics.peers_connected.store(
                        (pm.warm_peer_count() + pm.hot_peer_count()) as u64,
                        std::sync::atomic::Ordering::Relaxed,
                    );
                    self.metrics.peers_outbound.store(
                        pm.outbound_peer_count() as u64,
                        std::sync::atomic::Ordering::Relaxed,
                    );
                    let inbound_count = pm.inbound_peer_count() as u64;
                    self.metrics
                        .peers_inbound
                        .store(inbound_count, std::sync::atomic::Ordering::Relaxed);
                    // Duplex = peers with explicit duplex flag set (bidirectional
                    // mini-protocol bundles via InitiatorAndResponder diffusion mode).
                    self.metrics.peers_duplex.store(
                        pm.duplex_peer_count() as u64,
                        std::sync::atomic::Ordering::Relaxed,
                    );
                    self.metrics.peers_cold.store(
                        pm.cold_peer_count() as u64,
                        std::sync::atomic::Ordering::Relaxed,
                    );
                    self.metrics.peers_warm.store(
                        pm.warm_peer_count() as u64,
                        std::sync::atomic::Ordering::Relaxed,
                    );
                    self.metrics.peers_hot.store(
                        pm.hot_peer_count() as u64,
                        std::sync::atomic::Ordering::Relaxed,
                    );

                    // Connection manager counters (Haskell ConnectionManagerCounters compat).
                    // Uses per-connection state machine to compute overlapping counters
                    // matching Haskell's connectionStateToCounters exactly.
                    let cm_counters = pm.connection_manager_counters();
                    self.metrics.conn_full_duplex.store(
                        cm_counters.full_duplex,
                        std::sync::atomic::Ordering::Relaxed,
                    );
                    self.metrics
                        .conn_duplex
                        .store(cm_counters.duplex, std::sync::atomic::Ordering::Relaxed);
                    self.metrics.conn_unidirectional.store(
                        cm_counters.unidirectional,
                        std::sync::atomic::Ordering::Relaxed,
                    );
                    self.metrics
                        .conn_inbound
                        .store(cm_counters.inbound, std::sync::atomic::Ordering::Relaxed);
                    self.metrics
                        .conn_outbound
                        .store(cm_counters.outbound, std::sync::atomic::Ordering::Relaxed);
                    self.metrics.conn_terminating.store(
                        cm_counters.terminating,
                        std::sync::atomic::Ordering::Relaxed,
                    );
                }
                self.metrics
                    .set_governance_snapshot(&super::governance_snapshot_from_ledger(&ls));
                // Store tip slot time for dynamic tip_age computation
                let sc = &ls.slot_config;
                let slot_time_ms =
                    sc.zero_time + slot.saturating_sub(sc.zero_slot) * sc.slot_length as u64;
                self.metrics.set_tip_slot_time_ms(slot_time_ms);
                // Update chainsync idle time
                self.metrics.update_chainsync_idle();
                // Only show sync progress when catching up, not when following the tip
                if blocks_remaining > 0 {
                    info!(
                        progress = format_args!("{progress:.2}%"),
                        epoch = ls.epoch.0,
                        block = block_no,
                        tip = tip_block,
                        remaining = blocks_remaining,
                        speed = format_args!("{} blk/s", blocks_per_sec as u64),
                        utxos = ls.utxo.utxo_set.len(),
                        "Syncing",
                    );
                }
            }
            *last_log_time = std::time::Instant::now();
            *blocks_since_last_log = 0;
            if last_query_update.elapsed().as_secs() >= 30 {
                self.update_query_state().await;
                // Recompute peer reputations periodically
                self.peer_manager.write().await.recompute_reputations();
                *last_query_update = std::time::Instant::now();
            }
        }

        applied_count
    }

    // NOTE: chain_sync_loop and its helper methods (Node::validate_genesis_blocks,
    // Node::extract_slot_from_wrapped_header) were deleted as part of the networking
    // layer rewrite. enable_strict_verification() logic now lives in Node::run()
    // (after replay) and the epoch transition path in apply_blocks_batch().
    // The new connection lifecycle manager (connection_lifecycle.rs) handles
    // per-peer ChainSync/BlockFetch tasks.
    // The free function validate_genesis_blocks() is retained for tests.
    // process_forward_blocks() is retained as the block application entry point.

    /// Replay blocks from local storage to catch the ledger up to the chain tip.
    ///
    /// After a Mithril snapshot import, ChainDB contains millions of blocks
    /// but the ledger state starts from genesis. This replays blocks locally
    /// (no network needed).
    ///
    /// Two replay modes:
    /// 1. **Chunk file replay** (fast path): If `immutable/` exists in the
    ///    database directory (left by Mithril import), reads blocks sequentially
    ///    from chunk files. This is ~100x faster than LSM lookups because chunk
    ///    files are laid out sequentially on disk.
    /// 2. **LSM replay** (fallback): Reads blocks by block number from the LSM tree.
    ///    Slower due to random I/O but works when chunk files aren't available.
    pub async fn replay_ledger_from_storage(&mut self, shutdown_rx: watch::Receiver<bool>) {
        // Migrate legacy immutable-replay/ to immutable/ (backwards compat)
        let legacy_dir = self.database_path.join("immutable-replay");
        let immutable_dir = self.database_path.join("immutable");
        if legacy_dir.is_dir() && !immutable_dir.is_dir() {
            debug!("Migrating legacy immutable-replay/ to immutable/");
            if let Err(e) = std::fs::rename(&legacy_dir, &immutable_dir) {
                warn!("Failed to migrate immutable-replay/ to immutable/: {e}");
            }
        }

        // Check for chunk files — ImmutableDB provides permanent historical
        // block storage from Mithril. Chunk files are NOT deleted after replay.
        let chunk_dir = if immutable_dir.is_dir() {
            Some(immutable_dir)
        } else if legacy_dir.is_dir() {
            Some(legacy_dir)
        } else {
            None
        };
        if let Some(ref dir) = chunk_dir {
            let ledger_slot = {
                let ls = self.ledger_state.read().await;
                ls.tip.point.slot().map(|s| s.0).unwrap_or(0)
            };
            // Only replay if the ledger hasn't caught up to the immutable tip
            let imm_tip_slot = self
                .chain_db
                .read()
                .await
                .get_tip()
                .point
                .slot()
                .map(|s| s.0)
                .unwrap_or(0);
            if ledger_slot < imm_tip_slot {
                info!(
                    ledger_slot,
                    immutable_tip_slot = imm_tip_slot,
                    "Replaying ledger from chunk files",
                );
                self.replay_from_chunk_files(dir, shutdown_rx.clone()).await;
                // Don't return — fall through to LSM replay check below.
                // Chunk files from Mithril may not cover blocks that were
                // previously synced by Dugite and flushed to ImmutableDB.
                // The LSM replay path handles those remaining blocks.
            }
        }

        let db_tip = self.chain_db.read().await.get_tip();
        let ledger_slot = {
            let ls = self.ledger_state.read().await;
            ls.tip.point.slot().map(|s| s.0).unwrap_or(0)
        };
        let db_tip_slot = db_tip.point.slot().map(|s| s.0).unwrap_or(0);

        if db_tip_slot <= ledger_slot {
            return; // Ledger is already caught up
        }

        let blocks_behind = db_tip.block_number.0.saturating_sub({
            let ls = self.ledger_state.read().await;
            ls.tip.block_number.0
        });

        // Check if the user wants to limit replay via environment variable.
        let replay_limit: u64 = std::env::var("DUGITE_REPLAY_LIMIT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(u64::MAX);

        if blocks_behind > replay_limit {
            warn!(
                blocks_behind,
                replay_limit,
                db_tip_slot,
                ledger_slot,
                "Skipping ledger replay: gap exceeds DUGITE_REPLAY_LIMIT. \
                 Set DUGITE_REPLAY_LIMIT to a higher value or remove it to replay all blocks."
            );
            return;
        }

        if blocks_behind > 100_000 {
            info!(blocks_behind, "Replaying blocks (time-based snapshots)",);
        }

        info!(
            ledger_slot,
            db_tip_slot, blocks_behind, "Replaying ledger from ChainDB (LSM mode)",
        );
        self.replay_from_lsm(db_tip, shutdown_rx).await;
    }

    /// Fast replay: read blocks sequentially from chunk files.
    ///
    /// Runs in a blocking thread since chunk file I/O and ledger application
    /// are CPU-bound synchronous work.
    async fn replay_from_chunk_files(
        &self,
        replay_dir: &std::path::Path,
        shutdown_rx: watch::Receiver<bool>,
    ) {
        let ledger_state = self.ledger_state.clone();
        let snapshot_path = self.database_path.join("ledger-snapshot.bin");
        let replay_dir = replay_dir.to_path_buf();
        let bel = self.byron_epoch_length;
        let metrics = self.metrics.clone();

        let security_param = self
            .shelley_genesis
            .as_ref()
            .map(|g| g.security_param)
            .unwrap_or(2160);
        let imm_tip_slot = self
            .chain_db
            .read()
            .await
            .get_tip()
            .point
            .slot()
            .map(|s| s.0)
            .unwrap_or(0);
        let result = tokio::task::spawn_blocking(move || {
            let start = std::time::Instant::now();
            let mut replayed = 0u64;
            let mut skipped = 0u64;
            let mut last_log = std::time::Instant::now();
            let mut snapshot_policy = SnapshotPolicy::new(security_param);

            // Get ledger tip slot so we can skip blocks already applied.
            let ledger_tip_slot = {
                let ls = ledger_state.blocking_read();
                info!(
                    ledger_tip_slot = ls.tip.point.slot().map(|s| s.0).unwrap_or(0),
                    utxos = ls.utxo.utxo_set.len(),
                    "Chunk replay starting",
                );
                ls.tip.point.slot().map(|s| s.0).unwrap_or(0)
            };

            // Disable address index and full stake rebuild during replay.
            // Address index is never queried during replay, and the O(n)
            // retain per remove is expensive. Both are rebuilt at the end.
            // Incremental stake tracking is correct during sequential replay.
            {
                let mut ls = ledger_state.blocking_write();
                ls.utxo.utxo_set.set_indexing_enabled(false);
                ls.utxo.utxo_set.set_wal_enabled(false); // WAL disabled during replay for speed
                ls.epochs.needs_stake_rebuild = false;
            }

            let result = crate::mithril::replay_from_chunk_files(&replay_dir, |cbor| {
                // Check shutdown every 1000 blocks
                if replayed.is_multiple_of(1000) && *shutdown_rx.borrow() {
                    info!("Shutdown requested during chunk replay at block {replayed}");
                    return Err(anyhow::anyhow!("shutdown requested"));
                }

                // Minimal decode: chunk replay always uses ApplyOnly mode.
                // Skipping witness-set parsing is the primary replay speedup:
                // the witness set (vkey witnesses, scripts, redeemers, Plutus
                // data) is the largest per-tx allocation and is never read by
                // the ledger during ApplyOnly block application.
                match dugite_serialization::multi_era::decode_block_minimal_with_byron_epoch_length(
                    cbor, bel,
                ) {
                    Ok(block) => {
                        // Skip blocks already applied (at or before the ledger tip).
                        // Use strict < so that genesis block (slot 0) is NOT skipped
                        // when the ledger starts fresh (tip slot = 0).
                        if ledger_tip_slot > 0 && block.slot().0 <= ledger_tip_slot {
                            skipped += 1;
                            return Ok(());
                        }

                        let mut ls_guard = ledger_state.blocking_write();
                        if let Err(e) =
                            ls_guard.apply_block(&block, BlockValidationMode::ApplyOnly)
                        {
                            warn!(slot = block.slot().0, error = %e, "Ledger apply failed during replay");
                        }
                        replayed += 1;
                        snapshot_policy.record_blocks(1);

                        if last_log.elapsed().as_secs() >= 5 {
                            let elapsed = start.elapsed().as_secs_f64();
                            let speed = replayed as f64 / elapsed;
                            let slot = ls_guard.tip.point.slot().map(|s| s.0).unwrap_or(0);
                            let utxos = ls_guard.utxo.utxo_set.len();
                            let pct = if imm_tip_slot > 0 {
                                slot as f64 / imm_tip_slot as f64 * 100.0
                            } else {
                                0.0
                            };
                            // Update Prometheus metric so TUI/monitoring can track replay progress
                            metrics.set_sync_progress(pct);
                            metrics.set_slot(slot);
                            metrics.set_block_number(ls_guard.tip.block_number.0);
                            metrics.set_epoch(ls_guard.epoch.0);
                            info!(
                                progress = format_args!("{pct:>6.2}%"),
                                blocks = replayed,
                                slot,
                                speed = format_args!("{speed:.0} blk/s"),
                                utxos,
                                "Replay",
                            );
                            last_log = std::time::Instant::now();
                        }

                        if snapshot_policy.should_snapshot_bulk() {
                            // Recompute snapshot pool_stake using the current incremental
                            // stake_distribution before saving. This ensures mid-replay
                            // bulk snapshots have correct pool_stake values even though
                            // needs_stake_rebuild=false (no full UTxO scan at epoch boundaries).
                            // The incremental stake_map is correct at this point since it is
                            // maintained per-block during replay.
                            ls_guard.recompute_snapshot_pool_stakes();
                            if let Err(e) = ls_guard.save_snapshot(&snapshot_path) {
                                warn!("Failed to save ledger snapshot during replay: {e}");
                            }
                            snapshot_policy.snapshot_taken();
                        }
                    }
                    Err(e) => {
                        warn!("Failed to decode block during chunk replay: {e}");
                    }
                }
                Ok(())
            });

            match &result {
                Ok(total) => {
                    let elapsed = start.elapsed().as_secs_f64();
                    let speed = if elapsed > 0.0 {
                        replayed as f64 / elapsed
                    } else {
                        0.0
                    };
                    info!(
                        "Replay       complete ({} blocks in {}s, {} applied, {} skipped, {} blk/s)",
                        total, elapsed as u64, replayed, skipped, speed as u64,
                    );
                    // Update metrics with final replay state — include all
                    // counters so governance/state metrics are available
                    // immediately without requiring a node restart (#329).
                    let ls = ledger_state.blocking_read();
                    let slot = ls.tip.point.slot().map(|s| s.0).unwrap_or(0);
                    metrics.set_slot(slot);
                    metrics.set_block_number(ls.tip.block_number.0);
                    metrics.set_epoch(ls.epoch.0);
                    metrics.set_utxo_count(ls.utxo.utxo_set.len() as u64);
                    metrics
                        .set_governance_snapshot(&super::governance_snapshot_from_ledger(&ls));
                }
                Err(e) => {
                    // "shutdown requested" is not an error — it's a normal
                    // interruption when Ctrl+C is pressed during replay.
                    let msg = e.to_string();
                    if msg.contains("shutdown") {
                        warn!("Chunk-file replay interrupted: {e}");
                    } else {
                        error!("Chunk-file replay failed: {e}");
                    }
                }
            }

            // Re-enable address indexing and rebuild the index
            {
                let mut ls = ledger_state.blocking_write();
                ls.utxo.utxo_set.set_wal_enabled(true); // Re-enable WAL after replay
                ls.utxo.utxo_set.set_indexing_enabled(true);
                info!("Post-replay: rebuilding address index");
                ls.utxo.utxo_set.rebuild_address_index();
                // Rebuild stake distribution from the full UTxO set to correct any
                // residual state from the pre-replay snapshot. After this single
                // rebuild, incremental tracking is accurate and needs_stake_rebuild
                // self-disables at the next epoch boundary.
                info!("Post-replay: rebuilding stake distribution");
                ls.epochs.needs_stake_rebuild = true;
                ls.rebuild_stake_distribution();
                // Recompute pool_stake for all mark/set/go snapshots using the
                // freshly rebuilt stake_distribution and current reward_accounts.
                info!("Post-replay: recomputing snapshot pool stakes");
                ls.recompute_snapshot_pool_stakes();
                debug!("Rebuilt address index, stake distribution, and snapshot pool stakes after chunk replay");
            }

            // Save final snapshot (write lock to flush UTxO store — no WAL)
            {
                let mut ls = ledger_state.blocking_write();
                info!("Post-replay: saving UTxO snapshot");
                if let Err(e) = ls.save_utxo_snapshot() {
                    error!("Failed to save UTxO store after replay: {e}");
                }
                info!(
                    "Post-replay: saving ledger snapshot to {}",
                    snapshot_path.display()
                );
                if let Err(e) = ls.save_snapshot(&snapshot_path) {
                    error!("Failed to save ledger snapshot after replay: {e}");
                }
            }
            info!("Post-replay: initialization complete");

            result
        })
        .await;

        if let Err(e) = result {
            error!("Chunk-file replay task panicked: {e}");
        }
    }

    /// Fallback replay: read blocks from ChainDB using slot-based iteration.
    ///
    /// Uses `get_next_block_after_slot()` which queries both ImmutableDB and
    /// VolatileDB, making it correct after restart even when blocks have been
    /// flushed from the VolatileDB WAL into ImmutableDB chunk files.
    ///
    /// The previous implementation used `get_block_by_number()` (block-number
    /// index) which only queried the VolatileDB in-memory index.  After a
    /// clean restart the VolatileDB WAL is empty, so any blocks that had been
    /// flushed to ImmutableDB were invisible to the replay — resulting in
    /// "Block not found in ChainDB during replay block_no=NNNN" and 0 blocks
    /// applied, leaving the ledger stuck at the fork snapshot tip.
    async fn replay_from_lsm(
        &mut self,
        db_tip: dugite_primitives::block::Tip,
        shutdown_rx: watch::Receiver<bool>,
    ) {
        let start = std::time::Instant::now();
        let mut replayed = 0u64;
        let mut last_log = std::time::Instant::now();
        let snapshot_path = self.database_path.join("ledger-snapshot.bin");

        // Determine the slot range to replay: from the current ledger tip to
        // the ChainDB tip.  Use slots rather than block numbers — block numbers
        // are only indexed in the VolatileDB (which is empty after restart),
        // but slot-based lookup (get_next_block_after_slot) queries both
        // ImmutableDB and VolatileDB and so works correctly at all times.
        let (start_slot, end_slot) = {
            let mut ls = self.ledger_state.write().await;
            ls.utxo.utxo_set.set_indexing_enabled(false);
            ls.utxo.utxo_set.set_wal_enabled(false); // WAL disabled during replay for speed
            ls.epochs.needs_stake_rebuild = false;
            let start = ls.tip.point.slot().map(|s| s.0).unwrap_or(0);
            let end = db_tip.point.slot().map(|s| s.0).unwrap_or(0);
            (start, end)
        };

        if start_slot >= end_slot {
            info!(
                start_slot,
                end_slot, "LSM replay: nothing to replay (ledger tip >= ChainDB tip)"
            );
        } else {
            info!(
                ledger_slot = start_slot,
                db_tip_slot = end_slot,
                blocks_behind = {
                    // Rough estimate — block_number not available until we replay
                    db_tip
                        .block_number
                        .0
                        .saturating_sub(self.ledger_state.read().await.tip.block_number.0)
                },
                "Replaying ledger from ChainDB (slot-based)",
            );
        }

        let mut current_slot = start_slot;
        loop {
            // Check shutdown every 1000 blocks
            if replayed.is_multiple_of(1000) && replayed > 0 && *shutdown_rx.borrow() {
                info!(
                    replayed,
                    current_slot, "Shutdown requested during LSM replay, saving snapshot"
                );
                let mut ls = self.ledger_state.write().await;
                ls.consensus.opcert_counters = self.consensus.opcert_counters().clone();
                if let Err(e) = ls.save_snapshot(&snapshot_path) {
                    warn!("Failed to save snapshot on shutdown: {e}");
                }
                break;
            }

            let block_data = {
                let db = self.chain_db.read().await;
                db.get_next_block_after_slot(dugite_primitives::time::SlotNo(current_slot))
            };

            match block_data {
                Ok(Some((next_slot, _hash, cbor))) => {
                    // Stop once we have replayed up to and including the target slot.
                    if next_slot.0 > end_slot {
                        break;
                    }

                    // Minimal decode: LSM replay always uses ApplyOnly mode;
                    // witness-set fields are never accessed.
                    match dugite_serialization::multi_era::decode_block_minimal_with_byron_epoch_length(
                        &cbor,
                        self.byron_epoch_length,
                    ) {
                        Ok(block) => {
                            let mut ls = self.ledger_state.write().await;
                            let block_no = ls.tip.block_number.0 + 1;
                            if let Err(e) = ls.apply_block(&block, BlockValidationMode::ApplyOnly) {
                                warn!(
                                    slot = next_slot.0,
                                    "Replay ledger apply failed: {e}"
                                );
                            }
                            replayed += 1;
                            current_slot = next_slot.0;
                            self.snapshot_policy.record_blocks(1);

                            if last_log.elapsed().as_secs() >= 5 {
                                let elapsed = start.elapsed().as_secs_f64();
                                let speed = replayed as f64 / elapsed;
                                let pct = if end_slot > start_slot {
                                    (next_slot.0 - start_slot) as f64
                                        / (end_slot - start_slot) as f64
                                        * 100.0
                                } else {
                                    100.0
                                };
                                // Update Prometheus metric so TUI/monitoring can track replay progress
                                self.metrics.set_sync_progress(pct);
                                self.metrics.set_slot(next_slot.0);
                                self.metrics.set_block_number(block_no);
                                self.metrics.set_epoch(ls.epoch.0);
                                info!(
                                    progress = format_args!("{pct:>6.2}%"),
                                    slot = next_slot.0,
                                    end_slot,
                                    speed = format_args!("{speed:.0} blk/s"),
                                    utxos = ls.utxo.utxo_set.len(),
                                    "Replay",
                                );
                                last_log = std::time::Instant::now();
                            }

                            if self.snapshot_policy.should_snapshot_bulk() {
                                // Recompute snapshot pool_stake before saving. During LSM
                                // replay with needs_stake_rebuild=false, epoch boundaries
                                // use the incremental stake_map rather than a full UTxO scan.
                                // Calling recompute_snapshot_pool_stakes() here ensures the
                                // bulk snapshot has correct pool_stake values using the
                                // current incremental stake_distribution.
                                ls.recompute_snapshot_pool_stakes();
                                ls.consensus.opcert_counters = self.consensus.opcert_counters().clone();
                                if let Err(e) = ls.save_snapshot(&snapshot_path) {
                                    warn!("Failed to save ledger snapshot during replay: {e}");
                                }
                                self.snapshot_policy.snapshot_taken();
                            }
                        }
                        Err(e) => {
                            warn!(slot = next_slot.0, "Failed to decode block during replay: {e}");
                            // Advance past the undecodable slot to avoid an infinite loop.
                            current_slot = next_slot.0;
                        }
                    }
                }
                Ok(None) => {
                    // No more blocks after current_slot — replay complete.
                    break;
                }
                Err(e) => {
                    warn!(
                        current_slot,
                        "Failed to read from ChainDB during replay: {e}"
                    );
                    break;
                }
            }
        }

        let elapsed = start.elapsed().as_secs_f64();
        let speed = if elapsed > 0.0 {
            replayed as f64 / elapsed
        } else {
            0.0
        };
        info!(
            blocks = replayed,
            elapsed_secs = elapsed as u64,
            speed = format_args!("{} blk/s", speed as u64),
            "Replay complete",
        );

        // Update metrics with final replay state so they reflect the true
        // ledger position immediately (the progress ticker only fires every
        // 5 seconds and may miss the final state for short replays).
        {
            let ls = self.ledger_state.read().await;
            let slot = ls.tip.point.slot().map(|s| s.0).unwrap_or(0);
            self.metrics.set_slot(slot);
            self.metrics.set_block_number(ls.tip.block_number.0);
            self.metrics.set_epoch(ls.epoch.0);
        }

        // Re-enable WAL and address indexing after replay
        {
            let mut ls = self.ledger_state.write().await;
            ls.utxo.utxo_set.set_wal_enabled(true);
            ls.utxo.utxo_set.set_indexing_enabled(true);
            ls.utxo.utxo_set.rebuild_address_index();
            // Rebuild stake distribution from the full UTxO set to correct any
            // residual state from the pre-replay snapshot. After this single
            // rebuild, incremental tracking is accurate and needs_stake_rebuild
            // self-disables at the next epoch boundary.
            ls.epochs.needs_stake_rebuild = true;
            ls.rebuild_stake_distribution();
            // Recompute pool_stake for all mark/set/go snapshots.
            ls.recompute_snapshot_pool_stakes();
            debug!("Rebuilt address index, stake distribution, and snapshot pool stakes after LSM replay");
        }

        // Save final snapshot after replay (write lock to flush UTxO store — no WAL)
        {
            let mut ls = self.ledger_state.write().await;
            ls.consensus.opcert_counters = self.consensus.opcert_counters().clone();
            if let Err(e) = ls.save_utxo_snapshot() {
                error!("Failed to save UTxO store after replay: {e}");
            }
            if let Err(e) = ls.save_snapshot(&snapshot_path) {
                error!("Failed to save ledger snapshot after replay: {e}");
            }
        }
    }
}

// ─── Per-Peer ChainSync Client Task ──────────────────────────────────────────

/// Extract the block header hash (Blake2b-256) from a raw header CBOR.
///
/// The ChainSync MsgRollForward delivers header CBOR that may be either:
/// 1. HFC-wrapped: `[era_tag, tag24(header_bytes)]` — from Haskell peers
/// 2. Full block CBOR — from Dugite peers
///
/// In both cases, the block header hash is `blake2b_256(header_cbor)`.
/// For HFC-wrapped headers, the hash is computed over the entire wrapped
/// envelope, matching how Haskell computes it.
fn extract_hash_from_header(header_cbor: &[u8]) -> [u8; 32] {
    // The N2N ChainSync protocol sends headers as HFC-wrapped:
    //   [era_tag, tag24(inner_header_bytes)]
    // The Cardano block hash is blake2b_256 of the INNER header bytes
    // (the bytes inside tag24), NOT the outer wrapper. Hashing the wrapper
    // produces wrong hashes that BlockFetch cannot find.
    let inner = unwrap_hfc_header(header_cbor).unwrap_or(header_cbor);
    let hash = dugite_primitives::hash::blake2b_256(inner);
    let mut arr = [0u8; 32];
    arr.copy_from_slice(hash.as_ref());
    arr
}

/// Unwrap an HFC-wrapped header to get the inner header bytes.
///
/// N2N ChainSync headers are wrapped as `[era_tag, tag24(inner_bytes)]`.
/// Returns the inner bytes, or `None` if the CBOR is not in HFC format.
fn unwrap_hfc_header(header_cbor: &[u8]) -> Option<&[u8]> {
    use minicbor::Decoder;
    let mut dec = Decoder::new(header_cbor);
    let arr_len = dec.array().ok()?;
    if arr_len != Some(2) {
        return None;
    }
    let _era_tag = dec.u64().ok()?;
    let tag = dec.tag().ok()?;
    if tag != minicbor::data::Tag::new(24) {
        return None;
    }
    dec.bytes().ok()
}

/// Extract the slot number from a wrapped header CBOR.
///
/// The N2N ChainSync protocol sends headers as `[era_tag, tag24(header_bytes)]`.
/// For Shelley+ eras, the header body contains the slot as the second field.
/// For Byron, the slot is in a different position. This function attempts a
/// best-effort extraction without full deserialization.
///
/// Returns `None` if the header CBOR cannot be parsed.
/// Extract slot from a raw (unwrapped) header CBOR.
///
/// The `header_cbor` is the raw Shelley+ header bytes AFTER HFC unwrapping
/// (i.e., the bytes inside `tag24` from `[era_id, tag24(bytes)]`).
///
/// For Shelley+ headers: `[header_body, body_signature]` where
/// `header_body = [block_number, slot, ...]`.
fn extract_slot_from_wrapped_header(header_cbor: &[u8]) -> Option<u64> {
    use minicbor::Decoder;

    let mut dec = Decoder::new(header_cbor);

    // First, try to parse as a raw Shelley+ header (already unwrapped).
    // Structure: array(2+) [header_body, body_signature, ...]
    // header_body: array(N) [block_number, slot, ...]
    if let Ok(Some(_outer_len)) = dec.array() {
        // Try header_body array
        if let Ok(Some(_body_len)) = dec.array() {
            let _block_number = dec.u64().ok()?;
            let slot = dec.u64().ok()?;
            return Some(slot);
        }
    }

    // If that fails, try the HFC-wrapped format [era_tag, tag24(inner)]
    // (for compatibility with the old chain_sync_loop code path).
    let mut dec = Decoder::new(header_cbor);
    let arr_len = dec.array().ok()?;
    if arr_len == Some(2) {
        let era_tag = dec.u64().ok()?;
        let tag = dec.tag().ok()?;
        if tag == minicbor::data::Tag::new(24) {
            let inner_bytes = dec.bytes().ok()?;
            let mut inner = Decoder::new(inner_bytes);

            if era_tag == 0 {
                // Byron — complex, return None for now
                return None;
            } else {
                // Shelley+ inner
                let _ = inner.array().ok()?;
                let _ = inner.array().ok()?;
                let _block_number = inner.u64().ok()?;
                let slot = inner.u64().ok()?;
                return Some(slot);
            }
        }
    }

    None
}

/// Convert a `dugite_primitives::block::Point` to a network `codec::Point`.
fn to_codec_point(p: &Point) -> CodecPoint {
    match p {
        Point::Origin => CodecPoint::Origin,
        Point::Specific(slot, hash) => {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(hash.as_ref());
            CodecPoint::Specific(slot.0, arr)
        }
    }
}

/// Convert a network `codec::Point` to a `dugite_primitives::block::Point`.
fn from_codec_point(p: &CodecPoint) -> Point {
    match p {
        CodecPoint::Origin => Point::Origin,
        CodecPoint::Specific(slot, hash) => Point::Specific(
            dugite_primitives::time::SlotNo(*slot),
            dugite_primitives::hash::Hash32::from_bytes(*hash),
        ),
    }
}

/// Per-peer ChainSync client task.
///
/// Runs on a single MuxChannel, receives headers, and updates shared
/// candidate chain state. Does NOT fetch blocks — that's the
/// BlockFetch decision task's responsibility.
///
/// This matches the Haskell architecture where ChainSync and BlockFetch
/// run as independent threads sharing state via STM.
///
/// # Lifecycle
///
/// Called by `ConnectionLifecycleManager::make_chainsync_task()` when a peer
/// is promoted to Hot. Runs until the cancellation token is triggered (peer
/// demotion/disconnect), a protocol error occurs, or the bearer closes.
///
/// On exit (regardless of reason), the peer's candidate chain entry is
/// removed from the shared map.
///
/// # Protocol Flow
///
/// 1. **Build known points** — Walk backwards through volatile chain and
///    ledger state to build intersection candidates.
/// 2. **Find intersection** — Send `MsgFindIntersect` with the known points.
/// 3. **Pipeline headers** — Send a burst of `MsgRequestNext` up to `high_mark`,
///    then refill when outstanding drops to `low_mark`.
/// 4. **Update state** — For each `MsgRollForward`, add a `PendingHeader` to
///    the shared `candidate_chains` map. For `MsgRollBackward`, trim headers
///    after the rollback point.
#[allow(clippy::too_many_arguments)]
pub async fn chainsync_client_task(
    mut channel: MuxChannel,
    peer_addr: SocketAddr,
    candidate_chains: Arc<RwLock<HashMap<SocketAddr, CandidateChainState>>>,
    chain_db: Arc<RwLock<dugite_storage::ChainDB>>,
    ledger_state: Arc<RwLock<dugite_ledger::LedgerState>>,
    byron_epoch_length: u64,
    // Ouroboros security parameter k (number of blocks before finality).
    // Mainnet: 2160, Preview: 432.  Rollbacks deeper than k blocks indicate
    // a dishonest peer and result in peer disconnection, matching Haskell's
    // `terminateAfterDrain RolledBackPastIntersection`.
    security_param: u64,
    // Active slots coefficient from Shelley genesis (0.05 on mainnet/preview).
    // Used to scale the rollback depth threshold from blocks to slots:
    // with coeff=0.05, ~20 slots per block, so k blocks ≈ k*20 slots.
    active_slots_coeff: f64,
    metrics: Arc<crate::metrics::NodeMetrics>,
    cancel: CancellationToken,
    // GSM event sender — emits PeerRegistered, BlockReceived, PeerTipUpdated,
    // PeerActive, PeerIdling events to the GSM actor. Uses try_send (non-blocking).
    gsm_event_tx: tokio::sync::mpsc::Sender<crate::gsm::GsmEvent>,
) -> Result<()> {
    // ═══════════════════════════════════════════════════════════════════════
    // Phase 1: Build known points for intersection
    // ═══════════════════════════════════════════════════════════════════════
    //
    // Walk backwards through the volatile chain and ledger state to collect
    // historical points. This gives the peer multiple candidates for finding
    // a common chain prefix, which is critical for recovery after forging
    // (our local tip may be a freshly-forged block the peer hasn't seen).

    let (chain_tip, chain_points) = {
        let db = chain_db.read().await;
        let tip = db.get_tip().point;
        let points = db.get_chain_points(10);
        (tip, points)
    };

    let ledger_tip = ledger_state.read().await.tip.point.clone();
    let ledger_slot = ledger_tip.slot().map(|s| s.0).unwrap_or(0);
    let chain_slot = chain_tip.slot().map(|s| s.0).unwrap_or(0);

    // Detect fork divergence: check if blocks after the ledger tip in
    // ChainDB actually connect. If not, the ImmutableDB (or volatile)
    // contains orphan fork blocks — we must exclude them from the
    // intersection offer.
    let mut use_chain_tip = chain_slot > ledger_slot;
    let mut chain_diverged = false;
    if chain_slot >= ledger_slot && ledger_tip != Point::Origin {
        let db = chain_db.read().await;
        if let Ok(Some((_next_slot, _hash, cbor))) =
            db.get_next_block_after_slot(dugite_primitives::time::SlotNo(ledger_slot))
        {
            if let Ok(block) =
                dugite_serialization::multi_era::decode_block_minimal_with_byron_epoch_length(
                    &cbor,
                    byron_epoch_length,
                )
            {
                let ledger_hash = ledger_tip.hash();
                if ledger_hash.is_some_and(|h| h != block.prev_hash()) {
                    warn!(
                        %peer_addr,
                        "ChainDB fork divergence detected: blocks after ledger tip \
                         do not connect. Using ledger tip only for intersection.",
                    );
                    use_chain_tip = false;
                    chain_diverged = true;
                }
            }
        }
    }

    // Build the known_points list, including chain history for robustness.
    let mut known_points = Vec::new();
    if use_chain_tip {
        // ChainDB leads: include all chain ancestry points first.
        for p in &chain_points {
            if *p != Point::Origin && !known_points.contains(p) {
                known_points.push(p.clone());
            }
        }
        // Include ledger tip if it wasn't already covered by chain walk.
        if ledger_tip != Point::Origin && !known_points.contains(&ledger_tip) {
            known_points.push(ledger_tip.clone());
        }
    } else if chain_diverged {
        // ChainDB volatile blocks do not connect to the ledger tip (fork
        // divergence).  We must offer only *canonical* intersection points
        // from the ImmutableDB.  The ImmutableDB tip is the single most
        // important candidate: it is finalized, on the canonical chain, and
        // guaranteed to be known by all peers.
        //
        // Haskell behaviour after startup with an empty VolatileDB: the node
        // offers exactly [ImmutableDB tip] as the intersection candidate,
        // since that is the LedgerDB anchor point.
        //
        // Without this fix, the code sent only 8 deep-historical sparse points
        // (e.g. slot 107857439) instead of the ImmutableDB tip (107957082),
        // causing a 97K-slot / ~4980-block rollback on every restart after a
        // fork snapshot.
        let db = chain_db.read().await;

        // 1. ImmutableDB tip — always offered first (canonical, finalized anchor).
        if let Some(imm_tip) = db.get_immutable_tip_point() {
            if imm_tip != Point::Origin && !known_points.contains(&imm_tip) {
                known_points.push(imm_tip);
            }
        }

        // 2. Deep historical sparse points from older ImmutableDB chunks
        //    (fallback for peers that have rolled back past the current imm tip).
        for (slot, hash) in db.get_immutable_historical_points(8) {
            let p = Point::Specific(dugite_primitives::time::SlotNo(slot), hash);
            if !known_points.contains(&p) {
                known_points.push(p);
            }
        }

        // 3. If no ImmutableDB points found at all, fall back to ledger tip
        //    (last-chance candidate; peer will reject if it is on a fork, which
        //    is harmless — we'll fall back to Origin and resync from genesis).
        if known_points.is_empty() && ledger_tip != Point::Origin {
            known_points.push(ledger_tip.clone());
        }
    } else {
        // Ledger leads or tips are equal: offer ledger tip first, then
        // chain ancestry.
        if ledger_tip != Point::Origin {
            known_points.push(ledger_tip.clone());
        }
        for p in &chain_points {
            if *p != Point::Origin && !known_points.contains(p) {
                known_points.push(p.clone());
            }
        }
    }
    known_points.push(Point::Origin);

    info!(
        %peer_addr,
        chain_tip = %chain_tip,
        ledger_tip = %ledger_tip,
        known_points_count = known_points.len(),
        use_chain_tip,
        "ChainSync intersection candidates",
    );
    for (i, p) in known_points.iter().enumerate() {
        debug!(%peer_addr, idx = i, point = %p, "known_point");
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Phase 2: Find intersection with MsgFindIntersect (with retry)
    // ═══════════════════════════════════════════════════════════════════════
    //
    // Try progressively deeper points if the peer rejects our initial set.
    // This handles peers on a different fork or peers that have pruned recent
    // history.  Retry attempts use deeper ImmutableDB historical points
    // before falling back to Origin.

    /// Send MsgFindIntersect with the given points and return the result.
    async fn try_find_intersect(
        channel: &mut MuxChannel,
        peer_addr: SocketAddr,
        points: &[CodecPoint],
    ) -> Result<Option<CodecPoint>, anyhow::Error> {
        let find_msg = cs_encode(&ChainSyncMessage::MsgFindIntersect(
            points
                .iter()
                .map(|p| match p {
                    CodecPoint::Origin => dugite_network::codec::Point::Origin,
                    CodecPoint::Specific(s, h) => dugite_network::codec::Point::Specific(*s, *h),
                })
                .collect(),
        ));
        channel
            .send(find_msg)
            .await
            .map_err(|e| anyhow::anyhow!("ChainSync MsgFindIntersect send failed: {e}"))?;

        let response = channel
            .recv()
            .await
            .map_err(|e| anyhow::anyhow!("ChainSync intersection response recv failed: {e}"))?;
        let intersect_msg = cs_decode(&response)
            .map_err(|e| anyhow::anyhow!("ChainSync intersection decode failed: {e}"))?;

        match intersect_msg {
            ChainSyncMessage::MsgIntersectFound {
                point,
                tip_slot,
                tip_block_number,
                ..
            } => {
                let prim_point = from_codec_point(&point);
                info!(
                    %peer_addr,
                    point = %prim_point,
                    tip_slot,
                    tip_block_number,
                    "ChainSync intersection found",
                );
                Ok(Some(point))
            }
            ChainSyncMessage::MsgIntersectNotFound {
                tip_slot,
                tip_block_number,
                ..
            } => {
                info!(
                    %peer_addr,
                    tip_slot,
                    tip_block_number,
                    "ChainSync MsgIntersectNotFound",
                );
                Ok(None)
            }
            other => Err(anyhow::anyhow!(
                "ChainSync unexpected response to MsgFindIntersect: {other:?}"
            )),
        }
    }

    // Attempt 1: use the known_points we built above.
    let codec_points: Vec<CodecPoint> = known_points.iter().map(to_codec_point).collect();
    let mut intersection = try_find_intersect(&mut channel, peer_addr, &codec_points).await?;

    // Retry with progressively deeper ImmutableDB points if not found.
    if intersection.is_none() {
        let retry_depths: &[usize] = &[16, 64, 256];
        for (attempt, &depth) in retry_depths.iter().enumerate() {
            let db = chain_db.read().await;
            let deep_points: Vec<CodecPoint> = db
                .get_immutable_historical_points(depth)
                .iter()
                .map(|(slot, hash)| CodecPoint::Specific(*slot, hash.0))
                .collect();
            drop(db);

            if deep_points.is_empty() {
                // No deeper points available — fall through to Origin.
                break;
            }

            warn!(
                %peer_addr,
                attempt = attempt + 2,
                depth,
                points = deep_points.len(),
                "ChainSync intersection retry with deeper points",
            );

            // Include Origin as final fallback in the retry set.
            let mut retry_points = deep_points;
            retry_points.push(CodecPoint::Origin);

            if let Some(found) = try_find_intersect(&mut channel, peer_addr, &retry_points).await? {
                intersection = Some(found);
                break;
            }
        }

        if intersection.is_none() {
            info!(
                %peer_addr,
                "ChainSync no intersection after retries — syncing from Origin",
            );
        }
    }

    // Initialize candidate chain state for this peer.
    let intersection_slot = intersection
        .as_ref()
        .map(|p| match p {
            CodecPoint::Specific(s, _) => *s,
            CodecPoint::Origin => 0,
        })
        .unwrap_or(0);
    {
        let mut chains = candidate_chains.write().await;
        chains.insert(
            peer_addr,
            CandidateChainState {
                tip_slot: intersection_slot,
                tip_hash: intersection
                    .as_ref()
                    .map(|p| match p {
                        CodecPoint::Specific(_, h) => *h,
                        CodecPoint::Origin => [0u8; 32],
                    })
                    .unwrap_or([0u8; 32]),
                tip_block_number: 0,
                pending_headers: Vec::new(),
            },
        );
    }

    // Emit PeerRegistered to the GSM actor after successful intersection.
    // The tip_slot here is 0 (we haven't received any headers yet); the
    // GSM will update it as PeerTipUpdated events arrive.
    if let Err(e) = gsm_event_tx.try_send(crate::gsm::GsmEvent::PeerRegistered {
        addr: peer_addr,
        intersection_slot,
        tip_slot: intersection_slot,
    }) {
        debug!(%peer_addr, "GSM PeerRegistered event dropped: {e}");
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Phase 3: Pipeline headers with MsgRequestNext
    // ═══════════════════════════════════════════════════════════════════════
    //
    // Send a burst of MsgRequestNext up to high_mark, then refill when
    // outstanding drops to low_mark. This matches the Haskell pipelined
    // ChainSync client behavior.

    // Pipeline depth: configurable via DUGITE_PIPELINE_DEPTH env var (default: 300).
    let high_mark: usize = std::env::var("DUGITE_PIPELINE_DEPTH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300);
    let low_mark: usize = high_mark * 2 / 3; // refill at ~67%
    let mut outstanding: usize = 0;
    let mut at_tip = false;
    let mut headers_received: u64 = 0;
    // The first MsgRollBackward after intersection is expected protocol
    // behavior — the server rolls the client back to the agreed intersection
    // point before sending new headers. Skip the depth check for it.
    let mut initial_rollback = true;

    // Send initial pipeline burst.
    for _ in 0..high_mark {
        let req = cs_encode(&ChainSyncMessage::MsgRequestNext);
        channel
            .send(req)
            .await
            .map_err(|e| anyhow::anyhow!("ChainSync initial pipeline send failed: {e}"))?;
        outstanding += 1;
    }

    debug!(
        %peer_addr,
        high_mark,
        low_mark,
        "ChainSync pipeline started",
    );

    // Main loop: receive responses and update candidate_chains.
    loop {
        // Check for cancellation before each recv.
        tokio::select! {
            biased;

            _ = cancel.cancelled() => {
                debug!(%peer_addr, "ChainSync task cancelled");
                break;
            }

            result = channel.recv() => {
                let data = result.map_err(|e| {
                    anyhow::anyhow!("ChainSync recv failed: {e}")
                })?;

                let msg = cs_decode(&data).map_err(|e| {
                    anyhow::anyhow!("ChainSync decode failed: {e}")
                })?;

                match msg {
                    ChainSyncMessage::MsgRollForward {
                        header,
                        tip_slot,
                        tip_hash,
                        tip_block_number,
                    } => {
                        outstanding = outstanding.saturating_sub(1);
                        if at_tip {
                            at_tip = false;
                        }
                        headers_received += 1;

                        // Extract slot and hash from the header CBOR.
                        // The hash is blake2b_256 of the raw header bytes.
                        let hash = extract_hash_from_header(&header);
                        let slot = extract_slot_from_wrapped_header(&header)
                            .unwrap_or(tip_slot);

                        // Update candidate chain state.
                        {
                            let mut chains = candidate_chains.write().await;
                            let entry = chains.entry(peer_addr).or_insert_with(|| {
                                CandidateChainState {
                                    tip_slot: 0,
                                    tip_hash: [0u8; 32],
                                    tip_block_number: 0,
                                    pending_headers: Vec::new(),
                                }
                            });
                            entry.tip_slot = tip_slot;
                            entry.tip_hash = tip_hash;
                            entry.tip_block_number = tip_block_number;
                            entry.pending_headers.push(PendingHeader {
                                slot,
                                hash,
                                header_cbor: header,
                            });

                            // Prune headers that the ledger has already applied.
                            //
                            // We only drop headers whose slot is at or below the
                            // ledger tip — these have already been fetched and applied.
                            // Headers above the ledger tip are retained even if there
                            // are thousands, because BlockFetch needs them to bridge
                            // the gap (e.g. after fork divergence on restart when the
                            // volatile chain doesn't connect to the ledger snapshot).
                            //
                            // Hard cap at 10_000 as an absolute safety valve against
                            // unbounded growth during very long catch-ups.
                            let applied_slot = {
                                let ls = ledger_state.read().await;
                                ls.tip.point.slot().map(|s| s.0).unwrap_or(0)
                            };
                            entry.pending_headers.retain(|h| h.slot > applied_slot);
                            const HARD_CAP: usize = 10_000;
                            if entry.pending_headers.len() > HARD_CAP {
                                let excess = entry.pending_headers.len() - HARD_CAP;
                                entry.pending_headers.drain(..excess);
                            }
                        }

                        // Emit GSM events: BlockReceived, PeerTipUpdated, PeerActive.
                        // All use try_send — if the channel is full, the event is
                        // dropped silently (the periodic SyncStatus ensures convergence).
                        if let Err(e) = gsm_event_tx.try_send(crate::gsm::GsmEvent::BlockReceived {
                            addr: peer_addr,
                            slot,
                        }) {
                            debug!(%peer_addr, "GSM BlockReceived event dropped: {e}");
                        }
                        if let Err(e) = gsm_event_tx.try_send(crate::gsm::GsmEvent::PeerTipUpdated {
                            addr: peer_addr,
                            tip_slot,
                        }) {
                            debug!(%peer_addr, "GSM PeerTipUpdated event dropped: {e}");
                        }
                        if let Err(e) = gsm_event_tx.try_send(crate::gsm::GsmEvent::PeerActive {
                            addr: peer_addr,
                        }) {
                            debug!(%peer_addr, "GSM PeerActive event dropped: {e}");
                        }

                        // Log progress periodically.
                        if headers_received.is_multiple_of(10_000) {
                            debug!(
                                %peer_addr,
                                headers_received,
                                slot,
                                tip_slot,
                                tip_block_number,
                                outstanding,
                                "ChainSync header progress",
                            );
                        }

                        // Refill pipeline when outstanding drops below low_mark.
                        if !at_tip && outstanding <= low_mark {
                            let to_send = high_mark - outstanding;
                            for _ in 0..to_send {
                                let req = cs_encode(&ChainSyncMessage::MsgRequestNext);
                                channel.send(req).await.map_err(|e| {
                                    anyhow::anyhow!("ChainSync pipeline refill failed: {e}")
                                })?;
                                outstanding += 1;
                            }
                        }
                    }

                    ChainSyncMessage::MsgRollBackward {
                        point,
                        tip_slot,
                        tip_hash,
                        tip_block_number,
                    } => {
                        outstanding = outstanding.saturating_sub(1);

                        let rollback_slot = match &point {
                            CodecPoint::Origin => 0,
                            CodecPoint::Specific(s, _) => *s,
                        };
                        let prim_point = from_codec_point(&point);

                        // ── k-block rollback limit (Haskell: terminateAfterDrain) ──
                        //
                        // The first MsgRollBackward after intersection is expected
                        // protocol behavior — the server rolls the client back to
                        // the agreed intersection point. Skip the depth check for it.
                        //
                        // For subsequent rollbacks, compute depth from the ChainDB
                        // tip (NOT the ledger tip, which can diverge after Mithril
                        // import or snapshot restore). The threshold accounts for
                        // active_slots_coeff: with coeff=0.05, ~20 slots per block
                        // on average, so k blocks ≈ k*20 slots. We use 2x that as
                        // a safety margin.
                        //
                        // Haskell's `attemptRollback` returns `Nothing` when the
                        // rollback point is before the anchored fragment's anchor
                        // (deeper than k blocks), causing `ChainSyncClient` to call
                        // `terminateAfterDrain RolledBackPastIntersection`.
                        let is_initial = initial_rollback;
                        {
                            if initial_rollback {
                                initial_rollback = false;
                                debug!(
                                    %peer_addr,
                                    rollback_slot,
                                    "Skipping rollback depth check for initial \
                                     post-intersection rollback",
                                );
                            } else {
                                let chain_tip_slot = chain_db
                                    .read()
                                    .await
                                    .get_tip()
                                    .point
                                    .slot()
                                    .map(|s| s.0)
                                    .unwrap_or(0);

                                if chain_tip_slot > rollback_slot {
                                    let depth_slots = chain_tip_slot - rollback_slot;
                                    // Scale by active_slots_coeff: e.g. 0.05 → 20 slots/block.
                                    // Use 2x safety margin → k * (1/coeff) * 2.
                                    let slots_per_block =
                                        (1.0 / active_slots_coeff).ceil() as u64;
                                    let threshold_slots = security_param
                                        .saturating_mul(slots_per_block)
                                        .saturating_mul(2);
                                    if depth_slots > threshold_slots {
                                        warn!(
                                            %peer_addr,
                                            depth_slots,
                                            threshold_slots,
                                            security_param,
                                            chain_tip_slot,
                                            rollback_slot,
                                            "MsgRollBackward exceeds k-block limit — \
                                             disconnecting peer (matches Haskell \
                                             terminateAfterDrain RolledBackPastIntersection)"
                                        );
                                        // Return an error to drop this connection.
                                        // The PeerManager will record the failure and
                                        // apply a reputation penalty.
                                        return Err(anyhow::anyhow!(
                                            "Peer {peer_addr} requested rollback of \
                                             {depth_slots} slots (> {threshold_slots} \
                                             threshold, k={security_param})"
                                        ));
                                    }
                                }
                            }
                        }

                        info!(
                            %peer_addr,
                            rollback_point = %prim_point,
                            tip_slot,
                            tip_block_number,
                            "ChainSync rollback",
                        );

                        // Count non-initial rollbacks for observability.
                        // The first MsgRollBackward after intersection is
                        // expected protocol behavior — not a real fork.
                        if !is_initial {
                            metrics
                                .rollback_count
                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        }

                        // Remove headers after the rollback point.
                        {
                            let mut chains = candidate_chains.write().await;
                            if let Some(entry) = chains.get_mut(&peer_addr) {
                                entry.pending_headers.retain(|h| h.slot <= rollback_slot);
                                entry.tip_slot = tip_slot;
                                entry.tip_hash = tip_hash;
                                entry.tip_block_number = tip_block_number;
                            }
                        }

                        // Refill pipeline after rollback.
                        if !at_tip && outstanding <= low_mark {
                            let to_send = high_mark - outstanding;
                            debug!(
                                %peer_addr,
                                outstanding,
                                low_mark,
                                to_send,
                                "ChainSync refilling pipeline after rollback",
                            );
                            for _ in 0..to_send {
                                let req = cs_encode(&ChainSyncMessage::MsgRequestNext);
                                channel.send(req).await.map_err(|e| {
                                    anyhow::anyhow!("ChainSync pipeline refill failed: {e}")
                                })?;
                                outstanding += 1;
                            }
                            debug!(%peer_addr, outstanding, "ChainSync pipeline refilled");
                        } else {
                            debug!(
                                %peer_addr,
                                at_tip,
                                outstanding,
                                low_mark,
                                "ChainSync NOT refilling after rollback",
                            );
                        }
                    }

                    ChainSyncMessage::MsgAwaitReply => {
                        // At tip: the server has no new blocks right now.
                        // Do NOT decrement outstanding — MsgAwaitReply doesn't
                        // consume a request. The server will eventually respond
                        // with MsgRollForward or MsgRollBackward.
                        //
                        // Emit PeerIdling to the GSM actor so the GDD knows
                        // this peer has stopped sending blocks.
                        if let Err(e) = gsm_event_tx.try_send(crate::gsm::GsmEvent::PeerIdling {
                            addr: peer_addr,
                        }) {
                            debug!(%peer_addr, "GSM PeerIdling event dropped: {e}");
                        }
                        if !at_tip {
                            at_tip = true;
                            // Rate-limit "at tip" logging to at most once per
                            // 60 seconds globally across all peers.
                            //
                            // Rationale: when an inbound peer (e.g. a Haskell
                            // node syncing the full chain from Dugite) sends
                            // rapid MsgRollForward+MsgAwaitReply pairs, the
                            // at_tip flag toggles false→true on every single
                            // block — up to 1.2 million times in 10 minutes.
                            // Each log event at INFO floods the log file,
                            // filling 120MB in under 10 minutes and causing
                            // measurable I/O contention that stalls the main
                            // sync loop.
                            //
                            // Use compare_exchange (not load+store) so that
                            // concurrent tasks racing on the same 60-second
                            // window don't all win and each log once. Only the
                            // task that successfully stores wins.
                            static LAST_LOG: std::sync::atomic::AtomicU64 =
                                std::sync::atomic::AtomicU64::new(0);
                            let now_secs = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs();
                            let prev = LAST_LOG.load(std::sync::atomic::Ordering::Relaxed);
                            if now_secs.saturating_sub(prev) >= 60
                                && LAST_LOG
                                    .compare_exchange(
                                        prev,
                                        now_secs,
                                        std::sync::atomic::Ordering::Relaxed,
                                        std::sync::atomic::Ordering::Relaxed,
                                    )
                                    .is_ok()
                            {
                                // Emit at DEBUG — "at tip waiting for new block"
                                // is normal steady-state and does not warrant INFO.
                                debug!(
                                    %peer_addr,
                                    headers_received,
                                    "ChainSync at tip — awaiting new blocks",
                                );
                            }
                        }
                    }

                    ChainSyncMessage::MsgDone => {
                        info!(%peer_addr, "ChainSync server sent MsgDone");
                        break;
                    }

                    other => {
                        warn!(
                            %peer_addr,
                            "ChainSync unexpected message: {other:?}",
                        );
                    }
                }
            }
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Phase 4: Cleanup — remove this peer's candidate chain on exit
    // ═══════════════════════════════════════════════════════════════════════

    {
        let mut chains = candidate_chains.write().await;
        chains.remove(&peer_addr);
    }

    info!(
        %peer_addr,
        headers_received,
        "ChainSync task exiting",
    );

    Ok(())
}

#[cfg(test)]
mod chainsync_task_tests {
    use super::*;

    /// Verify extract_hash_from_header produces a 32-byte array.
    #[test]
    fn test_extract_hash_from_header() {
        let header = vec![0x82, 0x01, 0x02]; // arbitrary CBOR
        let hash = extract_hash_from_header(&header);
        // Should be a valid 32-byte blake2b-256 hash.
        assert_eq!(hash.len(), 32);
        // Same input should produce the same hash (deterministic).
        assert_eq!(hash, extract_hash_from_header(&header));
    }

    /// Verify extract_slot_from_wrapped_header returns None for invalid CBOR.
    #[test]
    fn test_extract_slot_invalid_cbor() {
        assert_eq!(extract_slot_from_wrapped_header(&[]), None);
        assert_eq!(extract_slot_from_wrapped_header(&[0x00]), None);
    }

    /// Verify to_codec_point / from_codec_point round-trip.
    #[test]
    fn test_point_roundtrip() {
        let origin = Point::Origin;
        assert_eq!(from_codec_point(&to_codec_point(&origin)), origin);

        let specific = Point::Specific(
            dugite_primitives::time::SlotNo(42),
            dugite_primitives::hash::Hash32::from_bytes([0xAB; 32]),
        );
        assert_eq!(from_codec_point(&to_codec_point(&specific)), specific);
    }

    /// Verify that extract_slot_from_wrapped_header correctly parses a
    /// Shelley+ wrapped header: [era_tag, tag24(header_bytes)].
    #[test]
    fn test_extract_slot_shelley_header() {
        use minicbor::Encoder;

        // Build a fake Shelley wrapped header:
        // Outer: array(2) [era_tag=1, tag24(inner_bytes)]
        // Inner: array(2) [array(N) [block_number=100, slot=12345, ...], signature]
        let mut inner_buf = Vec::new();
        let mut inner_enc = Encoder::new(&mut inner_buf);
        inner_enc.array(2).unwrap(); // outer: [header_body, signature]
        inner_enc.array(3).unwrap(); // header_body: [block_number, slot, prev_hash]
        inner_enc.u64(100).unwrap(); // block_number
        inner_enc.u64(12345).unwrap(); // slot
        inner_enc.bytes(&[0u8; 32]).unwrap(); // prev_hash (placeholder)
        inner_enc.bytes(&[0u8; 64]).unwrap(); // signature (placeholder)

        let mut buf = Vec::new();
        let mut enc = Encoder::new(&mut buf);
        enc.array(2).unwrap(); // [era_tag, tag24(inner)]
        enc.u64(1).unwrap(); // Shelley era tag
        enc.tag(minicbor::data::Tag::new(24)).unwrap();
        enc.bytes(&inner_buf).unwrap();

        assert_eq!(extract_slot_from_wrapped_header(&buf), Some(12345));
    }

    // -----------------------------------------------------------------------
    // k-block rollback limit logic tests
    // -----------------------------------------------------------------------
    //
    // The chainsync_client_task checks:
    //   depth_slots = ledger_slot - rollback_slot
    //   if depth_slots > security_param * 2 → disconnect peer
    //
    // These tests exercise the threshold arithmetic directly so we can verify
    // the boundary conditions without spinning up a full peer connection.

    /// Compute the rollback depth in slots and compare against the threshold.
    /// Returns `true` if the rollback EXCEEDS the k-limit (should disconnect).
    fn rollback_exceeds_k_limit(ledger_slot: u64, rollback_slot: u64, security_param: u64) -> bool {
        if ledger_slot > rollback_slot {
            let depth_slots = ledger_slot - rollback_slot;
            let threshold = security_param.saturating_mul(2);
            depth_slots > threshold
        } else {
            false
        }
    }

    /// A shallow rollback (1 slot) must never trigger the limit.
    #[test]
    fn test_k_rollback_shallow_ok() {
        // Mainnet k=2160
        assert!(!rollback_exceeds_k_limit(1000, 999, 2160));
        // Preview k=432
        assert!(!rollback_exceeds_k_limit(1000, 999, 432));
    }

    /// Rollback to exactly the threshold boundary: depth == 2k (not over).
    #[test]
    fn test_k_rollback_at_boundary_ok() {
        let k: u64 = 432; // preview
        let ledger_slot = k * 2; // exactly at threshold
        let rollback_slot = 0;
        // depth = 2k, threshold = 2k → NOT > → ok
        assert!(!rollback_exceeds_k_limit(ledger_slot, rollback_slot, k));
    }

    /// Rollback one slot beyond the threshold must trigger the limit.
    #[test]
    fn test_k_rollback_one_over_limit() {
        let k: u64 = 432;
        let ledger_slot = k * 2 + 1; // one over
        let rollback_slot = 0;
        // depth = 2k+1 > threshold=2k → must disconnect
        assert!(rollback_exceeds_k_limit(ledger_slot, rollback_slot, k));
    }

    /// Rolling back to the same slot as the ledger tip is never an error.
    #[test]
    fn test_k_rollback_same_slot_ok() {
        assert!(!rollback_exceeds_k_limit(1000, 1000, 432));
    }

    /// Rolling back to a LATER slot (peer confusion / no-op) is not an error.
    #[test]
    fn test_k_rollback_ahead_of_ledger_ok() {
        // rollback_slot > ledger_slot should not trigger the limit.
        assert!(!rollback_exceeds_k_limit(1000, 2000, 432));
    }

    /// Mainnet k=2160: a 5000-slot deep rollback exceeds 2*2160=4320.
    #[test]
    fn test_k_rollback_mainnet_deep_exceeds() {
        assert!(rollback_exceeds_k_limit(10_000, 5_000, 2160));
    }

    /// Mainnet k=2160: a 4000-slot rollback is within 2*2160=4320.
    #[test]
    fn test_k_rollback_mainnet_within_limit() {
        assert!(!rollback_exceeds_k_limit(10_000, 6_001, 2160));
    }
}
