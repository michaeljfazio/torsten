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
use super::networking::{EbbInfo, RollbackAnnouncement};
use torsten_consensus::praos::BlockIssuerInfo;
use torsten_consensus::ValidationMode;
use torsten_ledger::BlockValidationMode;
use torsten_network::codec::Point as CodecPoint;
use torsten_network::protocol::chainsync::{
    decode_message as cs_decode, encode_message as cs_encode, ChainSyncMessage,
};
use torsten_network::MuxChannel;
use torsten_primitives::block::Point;

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
    blocks: &[torsten_primitives::block::Block],
    expected_byron_hash: Option<&torsten_primitives::hash::Hash32>,
    expected_shelley_hash: Option<&torsten_primitives::hash::Hash32>,
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
    if first_block.era == torsten_primitives::era::Era::Byron {
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
        blocks: &[torsten_primitives::block::Block],
    ) -> Result<()> {
        validate_genesis_blocks(
            blocks,
            self.expected_byron_genesis_hash.as_ref(),
            self.expected_shelley_genesis_hash.as_ref(),
        )
    }

    /// Compute the current absolute slot number from wall-clock time.
    ///
    /// This correctly accounts for the Byron era on chains like mainnet and
    /// preprod, where the first N epochs use 20-second Byron slots before the
    /// Shelley hard fork switches to 1-second slots.
    ///
    /// The calculation:
    ///   1. Compute the total number of Byron slots that preceded Shelley:
    ///      `shelley_transition_epoch × byron_epoch_length`
    ///   2. Compute the wall-clock time at which Shelley began:
    ///      `chain_start + (shelley_transition_epoch × byron_epoch_length × byron_slot_ms)`
    ///   3. Compute elapsed Shelley slots from that point forward:
    ///      `(now - shelley_start) / shelley_slot_ms`
    ///   4. Total wall-clock slot = Byron slots + Shelley slots.
    ///
    /// For preview/sanchonet (no Byron era), `byron_epoch_length` is 0 and the
    /// result degenerates to the simple case used previously.
    pub fn current_wall_clock_slot(&self) -> Option<torsten_primitives::time::SlotNo> {
        let genesis = self.shelley_genesis.as_ref()?;
        let chain_start = chrono::DateTime::parse_from_rfc3339(&genesis.system_start)
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .ok()?;
        let shelley_slot_ms = (genesis.slot_length * 1000) as i64;
        if shelley_slot_ms == 0 {
            return None;
        }

        let now = chrono::Utc::now();

        // Determine how many Shelley-era slots follow the Byron era.
        // For networks without a Byron era (preview, sanchonet), byron_epoch_length
        // is 0 and the transition_epoch is also 0, so this whole block is a no-op.
        let byron_epoch_len = self.byron_epoch_length;
        let shelley_transition_epoch =
            super::epoch::shelley_transition_epoch_for_magic(self.network_magic);

        let (byron_total_slots, shelley_start_ms_offset): (u64, i64) =
            if byron_epoch_len > 0 && shelley_transition_epoch > 0 {
                // Total Byron slots = number of Byron epochs × slots per Byron epoch.
                let total_byron_slots = shelley_transition_epoch * byron_epoch_len;
                // Duration of the Byron era in milliseconds.
                let byron_duration_ms =
                    (total_byron_slots as i64).saturating_mul(self.byron_slot_duration_ms as i64);
                (total_byron_slots, byron_duration_ms)
            } else {
                (0, 0)
            };

        // Wall-clock time at which Shelley era began (= chain start + Byron era duration).
        let shelley_start = chain_start + chrono::Duration::milliseconds(shelley_start_ms_offset);

        // Elapsed milliseconds since Shelley start.
        let shelley_elapsed_ms = now.signed_duration_since(shelley_start).num_milliseconds();
        if shelley_elapsed_ms < 0 {
            // Wall clock is before Shelley era started — still in Byron era.
            // Fall back to computing Byron slot from chain start.
            let elapsed_ms = now.signed_duration_since(chain_start).num_milliseconds();
            if elapsed_ms < 0 {
                return None;
            }
            let byron_slot_ms = self.byron_slot_duration_ms as i64;
            if byron_slot_ms == 0 {
                return None;
            }
            return Some(torsten_primitives::time::SlotNo(
                (elapsed_ms / byron_slot_ms) as u64,
            ));
        }

        // Shelley-era slot count = elapsed ms / shelley slot duration.
        let shelley_slots = (shelley_elapsed_ms / shelley_slot_ms) as u64;

        // Absolute slot = Byron slots that preceded Shelley + Shelley slots elapsed.
        Some(torsten_primitives::time::SlotNo(
            byron_total_slots + shelley_slots,
        ))
    }

    /// Notify connected N2N peers of a chain rollback by sending MsgRollBackward.
    #[allow(dead_code)] // retained for networking rewrite
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
    #[allow(deprecated, dead_code)] // retained for networking rewrite
    async fn reset_ledger_and_replay(&self, target_slot: u64) {
        {
            let mut ls = self.ledger_state.write().await;
            let utxo_store = ls.utxo_set.detach_store();
            *ls = torsten_ledger::LedgerState::new(ls.protocol_params.clone());
            if let Some(store) = utxo_store {
                ls.attach_utxo_store(store);
            }
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
            let result = tokio::task::spawn_blocking(move || {
                let replay_start = std::time::Instant::now();
                let mut replayed = 0u64;
                let mut last_log = std::time::Instant::now();

                // Disable address index during replay for speed (rebuilt on
                // reattach after we're done).
                {
                    let mut ls = ledger_state.blocking_write();
                    ls.utxo_set.set_indexing_enabled(false);
                    ls.utxo_set.set_wal_enabled(false);
                }

                let result = crate::mithril::replay_from_chunk_files(
                    &immutable_dir,
                    |cbor| {
                        match torsten_serialization::multi_era::decode_block_minimal_with_byron_epoch_length(
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
                                    let speed = if elapsed > 0 { replayed / elapsed } else { replayed };
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
                    ls.utxo_set.set_indexing_enabled(true);
                    ls.utxo_set.set_wal_enabled(true);
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
    #[allow(dead_code)] // retained for networking rewrite
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

        // 2. Attempt the fast diff-based rollback path.
        //
        // Inspect the DiffSeq to determine whether all blocks that need to be
        // undone are covered by the in-memory diff window.  A diff is "after the
        // rollback point" if its slot is strictly greater than `rollback_slot`.
        //
        // The DiffSeq stores diffs in chronological order (oldest at front).
        // We count from the back (most-recent) until we find a diff whose slot
        // is <= rollback_slot — that's the new tip after rollback.
        let fast_path_used =
            {
                let mut ls = self.ledger_state.write().await;

                // Count how many trailing diffs are after the rollback point.
                // Also locate the new tip (the diff just at or before rollback_slot).
                let diffs_to_undo = ls
                    .diff_seq
                    .diffs
                    .iter()
                    .rev()
                    .take_while(|(slot, _hash, _diff)| slot.0 > rollback_slot)
                    .count();

                // The diff window is valid for the fast path when:
                //   (a) the DiffSeq is non-empty (at least some history is available), AND
                //   (b) all blocks after the rollback point are covered (i.e., the oldest
                //       diff we still have is at or before rollback_slot, meaning the
                //       ledger's state before those diffs is correctly represented
                //       by the remaining DiffSeq + underlying UTxO store).
                //
                // If diffs_to_undo == 0 the ledger is already at or before the rollback
                // point (handled above by the no-op check, but guard here for safety).
                //
                // If diffs_to_undo == ls.diff_seq.len() it means EVERY diff in the window
                // is after the rollback point, implying the diff window doesn't reach
                // far enough back to cover the rollback — fall back to slow path.
                let diff_total = ls.diff_seq.len();
                let window_covers_rollback = diffs_to_undo > 0 && diffs_to_undo < diff_total;

                if window_covers_rollback {
                    // Determine the new ledger tip: the most-recent diff that is AT or
                    // BEFORE the rollback slot becomes the new head.
                    let new_tip = ls.diff_seq.diffs.iter().rev().nth(diffs_to_undo).map(
                        |(slot, hash, _diff)| torsten_primitives::block::Tip {
                            point: torsten_primitives::block::Point::Specific(*slot, *hash),
                            block_number: torsten_primitives::time::BlockNo(0), // approximate; refreshed on next apply
                        },
                    );

                    if let Some(tip) = new_tip {
                        let rolled = ls.rollback_blocks_to_point(diffs_to_undo, tip);
                        info!(
                            rollback_slot,
                            diffs_undone = rolled,
                            "Fast diff-based rollback succeeded"
                        );
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            };

        if fast_path_used {
            // Fast path completed — skip snapshot reload.
        } else {
            // 3. Slow path: reload from snapshot and replay to rollback point.
            //
            // Find the best ledger snapshot at or before the rollback point.
            // Try epoch-numbered snapshots first (newest that's <= rollback_slot),
            // then fall back to the latest snapshot.
            let best_snapshot = self.find_best_snapshot_for_rollback(rollback_slot);

            if let Some(snapshot_path) = best_snapshot {
                match torsten_ledger::LedgerState::load_snapshot(&snapshot_path) {
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
                            match torsten_ledger::utxo_store::UtxoStore::open_from_snapshot(
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
                            let _ = ls.utxo_set.detach_store();
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
                            match db.get_next_block_after_slot(torsten_primitives::time::SlotNo(
                                current_slot,
                            )) {
                                Ok(Some((next_slot, _hash, cbor))) => {
                                    if next_slot.0 > rollback_slot {
                                        break;
                                    }
                                    // Minimal decode: rollback replay uses ApplyOnly
                                    // mode, so witness-set data is never read.
                                    match torsten_serialization::multi_era::decode_block_minimal_with_byron_epoch_length(&cbor, self.byron_epoch_length) {
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
                warn!("No suitable ledger snapshot found for rollback to slot {rollback_slot}, resetting ledger state");
                self.reset_ledger_and_replay(rollback_slot).await;
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
                if torsten_ledger::validation::validate_transaction(
                    &tx,
                    &ledger.utxo_set,
                    &ledger.protocol_params,
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
        mut blocks: Vec<torsten_primitives::block::Block>,
        tip: &torsten_primitives::block::Tip,
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
            let set_snapshot = ls.snapshots.set.as_ref();
            let total_active_stake: u64 = if let Some(snap) = set_snapshot {
                snap.pool_stake.values().map(|s| s.0).sum()
            } else {
                // During early sync, no snapshots exist yet — skip leader eligibility
                0
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
                //   1. Node restarts, replays immutable blocks → nonce_established=true
                //   2. First live block is the first block of epoch E+1
                //   3. Old code used epoch E nonce → VRF failure → batch rejected
                //   4. Ledger never advanced → epoch E+1 nonce never computed → stuck
                let epoch_nonce = ls.epoch_nonce_for_slot(block.slot().0);
                let mut header_with_nonce = block.header.clone();
                header_with_nonce.epoch_nonce = epoch_nonce;

                // Look up pool registration for VRF key binding and leader eligibility.
                // Uses "set" snapshot for stake (per Praos spec), falls back to current
                // pool_params for VRF key binding if snapshot is not available.
                let pool_id = torsten_primitives::hash::blake2b_224(&block.header.issuer_vkey);
                let issuer_info = if !block.header.issuer_vkey.is_empty() {
                    // Try set snapshot first (correct per spec)
                    let pool_reg = set_snapshot
                        .and_then(|snap| snap.pool_params.get(&pool_id))
                        .or_else(|| ls.pool_params.get(&pool_id));

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
                    ls.protocol_params.max_block_body_size,
                    ls.protocol_params.max_block_header_size,
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
                    mode,
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
                        Some(torsten_storage::AddBlockResult::AdoptedAsTip)
                        | Some(torsten_storage::AddBlockResult::StoredNotAdopted)
                        | Some(torsten_storage::AddBlockResult::AlreadyKnown) => {
                            // Block stored — proceed to ledger apply below.
                        }
                        Some(torsten_storage::AddBlockResult::Invalid(reason)) => {
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
        let loe_limit: Option<u64> = {
            let gsm = self.gsm.read().await;
            gsm.loe_limit(std::slice::from_ref(&tip.point))
        };

        // Now apply blocks to ledger — storage is confirmed
        let mut applied_count: u64 = 0;
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
                            db.get_next_block_after_slot(torsten_primitives::time::SlotNo(
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
                                match torsten_serialization::multi_era::decode_block_minimal_with_byron_epoch_length(&cbor, self.byron_epoch_length) {
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
                        use torsten_primitives::hash::Hash32;
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
                if let Err(e) = ls.apply_block(block, ledger_mode) {
                    error!(
                        slot = block.slot().0,
                        block_no = block.block_number().0,
                        hash = %block.hash().to_hex(),
                        "Failed to apply block to ledger: {e} — skipping remaining blocks in batch"
                    );
                    break;
                }
                applied_count += 1;
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
                                // TODO(subsystem-4): advance LedgerSeq anchor here.
                                // For now this is a no-op; the existing flush_to_immutable
                                // path handles immutable promotion.
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
                    if !ls.utxo_set.contains(input)
                        && self.mempool.lookup_virtual_utxo(input).is_none()
                    {
                        return false;
                    }
                }
                true
            });
            drop(ls);
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
                tx.send(torsten_network::BlockAnnouncement {
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
                self.epoch_transitions_observed = self
                    .epoch_transitions_observed
                    .saturating_add(epochs_crossed);
                self.live_epoch_transitions =
                    self.live_epoch_transitions.saturating_add(epochs_crossed);

                // Finalize immutable chunk at epoch boundary and persist
                {
                    let mut db = self.chain_db.write().await;
                    if let Err(e) = db.finalize_immutable_chunk() {
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
                        ledger.pool_params.keys().copied().collect();
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
                        ledger.protocol_params.max_block_body_size,
                        ledger.protocol_params.max_block_ex_units.mem,
                        ledger.protocol_params.max_block_ex_units.steps,
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
                        let new_params = ledger.protocol_params.clone();
                        let current_slot = ledger.tip.point.slot().map(|s| s.0).unwrap_or(0);
                        let slot_config = ledger.slot_config;
                        let utxo_ref = &ledger.utxo_set;
                        let evicted = self.mempool.revalidate_all(|tx| {
                            let tx_size = tx.raw_cbor.as_ref().map(|b| b.len() as u64).unwrap_or(0);
                            torsten_ledger::validation::validate_transaction(
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
                self.metrics.set_utxo_count(ls.utxo_set.len() as u64);
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
                    let duplex_count = pm.duplex_peer_count() as u64;
                    let outbound_count = pm.outbound_peer_count() as u64;
                    let inbound_count_cm = pm.inbound_peer_count() as u64;
                    // full_duplex: same as duplex for now (no separate full-duplex tracking yet).
                    self.metrics
                        .conn_full_duplex
                        .store(duplex_count, std::sync::atomic::Ordering::Relaxed);
                    self.metrics
                        .conn_duplex
                        .store(duplex_count, std::sync::atomic::Ordering::Relaxed);
                    // unidirectional: outbound connections that are not duplex.
                    self.metrics.conn_unidirectional.store(
                        outbound_count.saturating_sub(duplex_count),
                        std::sync::atomic::Ordering::Relaxed,
                    );
                    self.metrics
                        .conn_inbound
                        .store(inbound_count_cm, std::sync::atomic::Ordering::Relaxed);
                    self.metrics
                        .conn_outbound
                        .store(outbound_count, std::sync::atomic::Ordering::Relaxed);
                    // terminating: always 0 for now (no connection teardown tracking).
                    self.metrics
                        .conn_terminating
                        .store(0, std::sync::atomic::Ordering::Relaxed);
                }
                self.metrics.delegation_count.store(
                    ls.delegations.len() as u64,
                    std::sync::atomic::Ordering::Relaxed,
                );
                self.metrics
                    .treasury_lovelace
                    .store(ls.treasury.0, std::sync::atomic::Ordering::Relaxed);
                // Report only active DReps (active=true) to match what external
                // tools like Koios expose.  Inactive DReps remain registered in
                // `self.dreps` (they can reactivate) but are excluded from voting
                // power and from the count that operators care about.
                self.metrics.drep_count.store(
                    ls.governance.active_drep_count() as u64,
                    std::sync::atomic::Ordering::Relaxed,
                );
                self.metrics.proposal_count.store(
                    ls.governance.proposals.len() as u64,
                    std::sync::atomic::Ordering::Relaxed,
                );
                self.metrics.pool_count.store(
                    ls.pool_params.len() as u64,
                    std::sync::atomic::Ordering::Relaxed,
                );
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
                        utxos = ls.utxo_set.len(),
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
    // Node::enable_strict_verification, Node::extract_slot_from_wrapped_header) were
    // deleted as part of the networking layer rewrite. The new connection lifecycle
    // manager (connection_lifecycle.rs) handles per-peer ChainSync/BlockFetch tasks.
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
                // After replay the ledger's epoch is the authoritative count of
                // transitions that have been processed (snapshot + any new blocks
                // replayed here).  Assign directly rather than accumulating so
                // that if epoch_transitions_observed was primed from a snapshot
                // epoch we don't double-count.
                let replay_epoch = self.ledger_state.read().await.epoch.0;
                if replay_epoch > 0 {
                    self.epoch_transitions_observed = replay_epoch as u32;
                    info!(
                        epoch = replay_epoch,
                        epoch_transitions_observed = self.epoch_transitions_observed,
                        "Replay epoch transitions counted"
                    );
                }
                // Don't return — fall through to LSM replay check below.
                // Chunk files from Mithril may not cover blocks that were
                // previously synced by Torsten and flushed to ImmutableDB.
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
        let replay_limit: u64 = std::env::var("TORSTEN_REPLAY_LIMIT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(u64::MAX);

        if blocks_behind > replay_limit {
            warn!(
                blocks_behind,
                replay_limit,
                db_tip_slot,
                ledger_slot,
                "Skipping ledger replay: gap exceeds TORSTEN_REPLAY_LIMIT. \
                 Set TORSTEN_REPLAY_LIMIT to a higher value or remove it to replay all blocks."
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
        // After replay the ledger's epoch is the authoritative count of transitions
        // processed (snapshot + any new blocks replayed here).  Assign directly
        // rather than accumulating to avoid double-counting with the snapshot epoch
        // that was already primed into epoch_transitions_observed in Node::new().
        let replay_epoch = self.ledger_state.read().await.epoch.0;
        if replay_epoch > 0 {
            self.epoch_transitions_observed = replay_epoch as u32;
            info!(
                epoch = replay_epoch,
                epoch_transitions_observed = self.epoch_transitions_observed,
                "LSM replay epoch transitions counted"
            );
        }
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
                    utxos = ls.utxo_set.len(),
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
                ls.utxo_set.set_indexing_enabled(false);
                ls.utxo_set.set_wal_enabled(false); // WAL disabled during replay for speed
                ls.needs_stake_rebuild = false;
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
                match torsten_serialization::multi_era::decode_block_minimal_with_byron_epoch_length(
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
                            let utxos = ls_guard.utxo_set.len();
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
                    // Update metrics with final replay state
                    let ls = ledger_state.blocking_read();
                    let slot = ls.tip.point.slot().map(|s| s.0).unwrap_or(0);
                    metrics.set_slot(slot);
                    metrics.set_block_number(ls.tip.block_number.0);
                    metrics.set_epoch(ls.epoch.0);
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
                ls.utxo_set.set_wal_enabled(true); // Re-enable WAL after replay
                ls.utxo_set.set_indexing_enabled(true);
                ls.utxo_set.rebuild_address_index();
                // Rebuild the live stake distribution from the full UTxO set so that
                // live queries and the next epoch transition have correct values.
                // needs_stake_rebuild=true causes every subsequent epoch boundary to
                // rebuild stake_distribution instead of using incremental tracking.
                ls.needs_stake_rebuild = true;
                ls.rebuild_stake_distribution();
                // After rebuilding stake_distribution from the full UTxO set, recompute
                // pool_stake for all existing mark/set/go snapshots. This corrects any
                // drift that accumulated during replay when needs_stake_rebuild=false caused
                // epoch-boundary pool_stake to be computed from the incremental stake_map
                // rather than a full UTxO scan. recompute_snapshot_pool_stakes() uses both
                // the rebuilt stake_distribution and current reward_accounts, matching the
                // Haskell semantics for snapshot pool_stake (UTxO stake + reward balance).
                // Without this call, the saved final snapshot may have pool_stake=0 for
                // pools whose delegators' UTxOs were present during replay but missed by
                // the incremental tracking (e.g., due to UTxO apply failures on out-of-order
                // inputs) — causing incorrect leader eligibility and empty rewards after
                // two epoch transitions.
                ls.recompute_snapshot_pool_stakes();
                debug!("Rebuilt address index, stake distribution, and snapshot pool stakes after chunk replay");
            }

            // Save final snapshot (write lock to flush UTxO store — no WAL)
            {
                let mut ls = ledger_state.blocking_write();
                if let Err(e) = ls.save_utxo_snapshot() {
                    error!("Failed to save UTxO store after replay: {e}");
                }
                if let Err(e) = ls.save_snapshot(&snapshot_path) {
                    error!("Failed to save ledger snapshot after replay: {e}");
                }
            }

            result
        })
        .await;

        if let Err(e) = result {
            error!("Chunk-file replay task panicked: {e}");
        }
    }

    /// Fallback replay: read blocks from LSM tree by block number.
    async fn replay_from_lsm(
        &mut self,
        db_tip: torsten_primitives::block::Tip,
        shutdown_rx: watch::Receiver<bool>,
    ) {
        let start = std::time::Instant::now();
        let mut replayed = 0u64;
        let mut last_log = std::time::Instant::now();
        let snapshot_path = self.database_path.join("ledger-snapshot.bin");

        let start_block_no = {
            let mut ls = self.ledger_state.write().await;
            ls.utxo_set.set_indexing_enabled(false);
            ls.utxo_set.set_wal_enabled(false); // WAL disabled during replay for speed
            ls.needs_stake_rebuild = false;
            ls.tip.block_number.0 + 1
        };
        let end_block_no = db_tip.block_number.0;

        for block_no in start_block_no..=end_block_no {
            // Check shutdown every 1000 blocks
            if block_no.is_multiple_of(1000) && *shutdown_rx.borrow() {
                info!(
                    block_no,
                    "Shutdown requested during LSM replay, saving snapshot"
                );
                let ls = self.ledger_state.write().await;
                if let Err(e) = ls.save_snapshot(&snapshot_path) {
                    warn!("Failed to save snapshot on shutdown: {e}");
                }
                break;
            }

            let block_data = {
                let db = self.chain_db.read().await;
                db.get_block_by_number(torsten_primitives::time::BlockNo(block_no))
            };

            match block_data {
                Ok(Some((slot, _hash, cbor))) => {
                    // Minimal decode: LSM replay always uses ApplyOnly mode;
                    // witness-set fields are never accessed.
                    match torsten_serialization::multi_era::decode_block_minimal_with_byron_epoch_length(
                        &cbor,
                        self.byron_epoch_length,
                    ) {
                        Ok(block) => {
                            let mut ls = self.ledger_state.write().await;
                            if let Err(e) = ls.apply_block(&block, BlockValidationMode::ApplyOnly) {
                                warn!(
                                    "Replay       ledger apply failed at slot {} block {}: {e}",
                                    slot.0, block_no
                                );
                            }
                            replayed += 1;
                            self.snapshot_policy.record_blocks(1);

                            if last_log.elapsed().as_secs() >= 5 {
                                let elapsed = start.elapsed().as_secs_f64();
                                let speed = replayed as f64 / elapsed;
                                let pct = if end_block_no > 0 {
                                    block_no as f64 / end_block_no as f64 * 100.0
                                } else {
                                    0.0
                                };
                                // Update Prometheus metric so TUI/monitoring can track replay progress
                                self.metrics.set_sync_progress(pct);
                                self.metrics.set_slot(slot.0);
                                self.metrics.set_block_number(block_no);
                                self.metrics.set_epoch(ls.epoch.0);
                                info!(
                                    progress = format_args!("{pct:>6.2}%"),
                                    block = block_no,
                                    total = end_block_no,
                                    slot = slot.0,
                                    speed = format_args!("{speed:.0} blk/s"),
                                    utxos = ls.utxo_set.len(),
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
                                if let Err(e) = ls.save_snapshot(&snapshot_path) {
                                    warn!("Failed to save ledger snapshot during replay: {e}");
                                }
                                self.snapshot_policy.snapshot_taken();
                            }
                        }
                        Err(e) => {
                            warn!(block_no, "Failed to decode block during replay: {e}");
                        }
                    }
                }
                Ok(None) => {
                    warn!(block_no, "Block not found in ChainDB during replay");
                    break;
                }
                Err(e) => {
                    warn!(block_no, "Failed to read from ChainDB during replay: {e}");
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
            ls.utxo_set.set_wal_enabled(true);
            ls.utxo_set.set_indexing_enabled(true);
            ls.utxo_set.rebuild_address_index();
            // Rebuild the live stake distribution from the full UTxO set.
            // needs_stake_rebuild=true causes every subsequent live epoch boundary
            // to rebuild stake_distribution instead of using incremental tracking.
            ls.needs_stake_rebuild = true;
            ls.rebuild_stake_distribution();
            // Recompute pool_stake for all mark/set/go snapshots using the freshly
            // rebuilt stake_distribution and current reward_accounts. This corrects
            // drift accumulated during replay when needs_stake_rebuild=false caused
            // epoch-boundary pool_stake computations to use the incremental stake_map.
            // See the chunk replay path comment above for the detailed rationale.
            ls.recompute_snapshot_pool_stakes();
            debug!("Rebuilt address index, stake distribution, and snapshot pool stakes after LSM replay");
        }

        // Save final snapshot after replay (write lock to flush UTxO store — no WAL)
        {
            let mut ls = self.ledger_state.write().await;
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
/// 2. Full block CBOR — from Torsten peers
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
    let hash = torsten_primitives::hash::blake2b_256(inner);
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

/// Convert a `torsten_primitives::block::Point` to a network `codec::Point`.
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

/// Convert a network `codec::Point` to a `torsten_primitives::block::Point`.
fn from_codec_point(p: &CodecPoint) -> Point {
    match p {
        CodecPoint::Origin => Point::Origin,
        CodecPoint::Specific(slot, hash) => Point::Specific(
            torsten_primitives::time::SlotNo(*slot),
            torsten_primitives::hash::Hash32::from_bytes(*hash),
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
    chain_db: Arc<RwLock<torsten_storage::ChainDB>>,
    ledger_state: Arc<RwLock<torsten_ledger::LedgerState>>,
    byron_epoch_length: u64,
    cancel: CancellationToken,
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
            db.get_next_block_after_slot(torsten_primitives::time::SlotNo(ledger_slot))
        {
            if let Ok(block) =
                torsten_serialization::multi_era::decode_block_minimal_with_byron_epoch_length(
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
        // ChainDB has contaminated blocks — offer only deep historical
        // points from older ImmutableDB chunks.
        let db = chain_db.read().await;
        for (slot, hash) in db.get_immutable_historical_points(8) {
            let p = Point::Specific(torsten_primitives::time::SlotNo(slot), hash);
            if !known_points.contains(&p) {
                known_points.push(p);
            }
        }
        // If no historical points found, fall back to ledger tip.
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
    // Phase 2: Find intersection with MsgFindIntersect
    // ═══════════════════════════════════════════════════════════════════════

    let codec_points: Vec<CodecPoint> = known_points.iter().map(to_codec_point).collect();

    // Send MsgFindIntersect.
    let find_msg = cs_encode(&ChainSyncMessage::MsgFindIntersect(
        codec_points
            .iter()
            .map(|p| match p {
                CodecPoint::Origin => torsten_network::codec::Point::Origin,
                CodecPoint::Specific(s, h) => torsten_network::codec::Point::Specific(*s, *h),
            })
            .collect(),
    ));
    channel
        .send(find_msg)
        .await
        .map_err(|e| anyhow::anyhow!("ChainSync MsgFindIntersect send failed: {e}"))?;

    // Receive MsgIntersectFound or MsgIntersectNotFound.
    let response = channel
        .recv()
        .await
        .map_err(|e| anyhow::anyhow!("ChainSync intersection response recv failed: {e}"))?;
    let intersect_msg = cs_decode(&response)
        .map_err(|e| anyhow::anyhow!("ChainSync intersection decode failed: {e}"))?;

    let intersection = match intersect_msg {
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
            Some(point)
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
                "ChainSync no intersection — syncing from Origin",
            );
            None
        }
        other => {
            return Err(anyhow::anyhow!(
                "ChainSync unexpected response to MsgFindIntersect: {other:?}"
            ));
        }
    };

    // Initialize candidate chain state for this peer.
    {
        let mut chains = candidate_chains.write().await;
        chains.insert(
            peer_addr,
            CandidateChainState {
                tip_slot: intersection
                    .as_ref()
                    .map(|p| match p {
                        CodecPoint::Specific(s, _) => *s,
                        CodecPoint::Origin => 0,
                    })
                    .unwrap_or(0),
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

    // ═══════════════════════════════════════════════════════════════════════
    // Phase 3: Pipeline headers with MsgRequestNext
    // ═══════════════════════════════════════════════════════════════════════
    //
    // Send a burst of MsgRequestNext up to high_mark, then refill when
    // outstanding drops to low_mark. This matches the Haskell pipelined
    // ChainSync client behavior.

    // Pipeline depth: configurable via TORSTEN_PIPELINE_DEPTH env var (default: 300).
    let high_mark: usize = std::env::var("TORSTEN_PIPELINE_DEPTH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300);
    let low_mark: usize = high_mark * 2 / 3; // refill at ~67%
    let mut outstanding: usize = 0;
    let mut at_tip = false;
    let mut headers_received: u64 = 0;

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

                        info!(
                            %peer_addr,
                            rollback_point = %prim_point,
                            tip_slot,
                            tip_block_number,
                            "ChainSync rollback",
                        );

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
                        if !at_tip {
                            at_tip = true;
                            info!(
                                %peer_addr,
                                headers_received,
                                "ChainSync at tip — awaiting new blocks",
                            );
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
            torsten_primitives::time::SlotNo(42),
            torsten_primitives::hash::Hash32::from_bytes([0xAB; 32]),
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
}
