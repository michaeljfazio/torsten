//! Block sync loop, forward-block processing, rollback handling, and ledger replay.
//!
//! This module contains the core pipelined ChainSync state machine that drives
//! block ingestion from upstream peers, as well as the ledger replay path used
//! after a Mithril snapshot import.

use anyhow::Result;
use tokio::sync::watch;
use tracing::{debug, error, info, warn};

use torsten_consensus::praos::BlockIssuerInfo;
use torsten_consensus::ValidationMode;
use torsten_ledger::BlockValidationMode;
use torsten_network::{
    BlockFetchPool, ChainSyncEvent, EbbInfo, HeaderBatchResult, NodeToNodeClient,
    PipelinedPeerClient,
};
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

    /// Enable strict verification mode.
    ///
    /// The epoch nonce is immediately valid after replay because the nonce
    /// computation correctly accumulates VRF contributions from every block
    /// during replay (using the era-correct nonce_vrf_output field).  There is
    /// no "warming up" period — `nonce_established` is always `true`.
    ///
    /// Stake snapshots still need 3 epoch transitions to fully rotate through
    /// mark→set→go with correctly rebuilt stake distributions, so VRF leader
    /// eligibility failures remain non-fatal until `snapshots_established`.
    pub async fn enable_strict_verification(&mut self) {
        self.consensus.set_strict_verification(true);
        // The epoch nonce is valid after a full replay from genesis (where VRF
        // contributions are accumulated for every block). But after loading a
        // snapshot — especially one whose epoch was corrected (wrong epoch_length
        // baked into the snapshot) — the nonce may be stale.  Only mark it as
        // established once at least 1 epoch transition has been observed (replay
        // or live), which ensures nonce was built from VRF contributions.
        self.consensus.nonce_established = self.epoch_transitions_observed >= 1;
        // Stake snapshots need 3 LIVE (post-replay) epoch transitions to fully
        // rotate with correct rebuilt stake distributions.  Replay-built snapshots
        // may have slightly different stake values than the canonical chain (due
        // to reward calculation differences or UTxO apply mismatches), so VRF
        // leader eligibility failures remain non-fatal until live transitions
        // have rotated fresh snapshots through mark→set→go.
        self.consensus.snapshots_established = self.live_epoch_transitions >= 3;
        if !self.consensus.snapshots_established {
            debug!(
                total_transitions = self.epoch_transitions_observed,
                live_transitions = self.live_epoch_transitions,
                "VRF leader check non-fatal: stake snapshots not yet established (need 3 live epoch transitions)"
            );
        }
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
    pub async fn notify_rollback(&self, rollback_point: &Point) {
        if let Some(ref tx) = self.rollback_announcement_tx {
            let (tip_slot, tip_hash, tip_block_number) = {
                let db = self.chain_db.read().await;
                let tip = db.get_tip();
                let slot = tip.point.slot().map(|s| s.0).unwrap_or(0);
                let hash = tip
                    .point
                    .hash()
                    .map(|h| {
                        let bytes: &[u8] = h.as_ref();
                        let mut arr = [0u8; 32];
                        arr.copy_from_slice(bytes);
                        arr
                    })
                    .unwrap_or([0u8; 32]);
                (slot, hash, tip.block_number.0)
            };

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

            let _ = tx.send(torsten_network::RollbackAnnouncement {
                slot: rb_slot,
                hash: rb_hash,
                tip_slot,
                tip_hash,
                tip_block_number,
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
                                    tracing::debug!(
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

                        // Restore from snapshot and replay forward to rollback point.
                        // Detach the UtxoStore before replacing state so it survives
                        // the replacement (bincode snapshot has utxos=0 in LSM mode).
                        let mut ls = self.ledger_state.write().await;
                        let utxo_store = ls.utxo_set.detach_store();
                        *ls = snapshot_state;
                        if let Some(store) = utxo_store {
                            ls.attach_utxo_store(store);
                        }
                        let replay_from = snapshot_slot;

                        // Replay blocks from snapshot tip to rollback point.
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
                        debug!(
                            snapshot_slot,
                            rollback_slot,
                            replayed,
                            snapshot = %snapshot_path.display(),
                            "Ledger state restored from snapshot and replayed"
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
    #[allow(clippy::too_many_arguments)]
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

        // Store blocks to ChainDB FIRST, then apply to ledger.
        // This ordering ensures the ledger never advances past what's persisted in storage,
        // preventing state divergence if storage fails.
        {
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
                        // ChainDB has blocks from a different fork that don't connect
                        // to the ledger. Clear volatile blocks and let the network
                        // re-sync from the ledger tip.
                        warn!(
                            "Gap bridge failed due to fork divergence. \
                             Clearing stale volatile blocks and re-syncing from ledger tip."
                        );
                        {
                            let mut db = self.chain_db.write().await;
                            db.clear_volatile();
                        }
                        // Return 0 to signal that no blocks were applied from this batch.
                        // The caller will reconnect with a fresh intersection.
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
                        .find(|ebb| ebb.next_block_hash == *block.hash().as_bytes());
                    if let Some(ebb) = ebb_match {
                        use torsten_primitives::hash::Hash32;
                        let ebb_hash = Hash32::from_bytes(ebb.ebb_hash);
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
                    self.metrics
                        .transactions_rejected
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    break;
                }
                applied_count += 1;
            }
        }

        // Remove confirmed transactions from mempool, then full revalidation
        if !self.mempool.is_empty() {
            let confirmed_hashes: Vec<_> = blocks
                .iter()
                .flat_map(|b| b.transactions.iter().map(|tx| tx.hash))
                .collect();
            if !confirmed_hashes.is_empty() {
                self.mempool.remove_txs(&confirmed_hashes);
            }

            // Full revalidation: check each remaining tx for input conflicts,
            // TTL expiry, and any other invalidity in one pass.
            let consumed_inputs: std::collections::HashSet<_> = blocks
                .iter()
                .flat_map(|b| b.transactions.iter())
                .flat_map(|tx| tx.body.inputs.iter().cloned())
                .collect();
            let current_slot = blocks.last().map(|b| b.slot());
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
                // Reject if TTL has expired
                if let (Some(ttl), Some(slot)) = (tx.body.ttl, current_slot) {
                    if slot.0 > ttl.0 {
                        return false;
                    }
                }
                true
            });
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
        {
            let mut db = self.chain_db.write().await;
            let flush_result = match loe_limit {
                None => {
                    // No LoE constraint — flush normally (Praos mode or CaughtUp).
                    db.flush_to_immutable()
                }
                Some(loe_slot) => {
                    // LoE active: cap the immutable tip at the peer common prefix.
                    // Blocks beyond loe_slot remain in VolatileDB until the GSM
                    // transitions to CaughtUp and the constraint is lifted.
                    debug!(
                        loe_slot,
                        "LoE active: capping immutable flush at peer tip slot"
                    );
                    db.flush_to_immutable_loe(loe_slot)
                }
            };
            if let Err(e) = flush_result {
                warn!(error = %e, "Failed to flush blocks to immutable storage");
            }
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
                self.metrics.mempool_bytes.store(
                    self.mempool.total_bytes() as u64,
                    std::sync::atomic::Ordering::Relaxed,
                );
                {
                    let pm = self.peer_manager.read().await;
                    self.metrics.peers_connected.store(
                        pm.hot_peer_count() as u64,
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
                    // Duplex = outbound + inbound: total peers with bidirectional
                    // mini-protocol bundles (InitiatorAndResponder diffusion mode).
                    self.metrics.peers_duplex.store(
                        pm.outbound_peer_count() as u64 + inbound_count,
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

    /// Run the pipelined ChainSync loop with a connected peer.
    pub async fn chain_sync_loop(
        &mut self,
        client: &mut NodeToNodeClient,
        pipelined_client: Option<PipelinedPeerClient>,
        fetch_pool: BlockFetchPool,
        mut shutdown_rx: watch::Receiver<bool>,
        peer_addr: std::net::SocketAddr,
    ) -> Result<()> {
        let mut pipelined = pipelined_client;
        // Find intersection with our current chain.
        //
        // We walk backwards through the volatile chain to collect multiple
        // historical points, not just the tip.  This is critical for
        // recovery after forging: if the local tip is a freshly-forged block
        // that the peer hasn't seen (slot battle), the ancestor blocks give
        // the peer a known common point to intersect on.  Without this, the
        // intersection would fall all the way to Origin causing a death loop.
        let chain_tip = self.chain_db.read().await.get_tip().point;
        let ledger_tip = self.ledger_state.read().await.tip.point.clone();
        let ledger_slot = ledger_tip.slot().map(|s| s.0).unwrap_or(0);
        let chain_slot = chain_tip.slot().map(|s| s.0).unwrap_or(0);

        // Collect historical chain points by walking backwards through the
        // volatile DB via prev_hash links (tip → parent → grandparent …).
        // This gives the peer enough ancestry to find a common block even
        // when the tip is an orphaned/forged block.
        let chain_points = {
            let db = self.chain_db.read().await;
            db.get_chain_points(10)
        };

        // When ChainDB is ahead of the ledger, check if its chain connects.
        // If not (fork divergence), prefer the ledger tip for intersection.
        let mut use_chain_tip = chain_slot > ledger_slot;
        let mut chain_diverged = false;
        if use_chain_tip && ledger_tip != Point::Origin {
            let db = self.chain_db.read().await;
            if let Ok(Some((_next_slot, _hash, cbor))) =
                db.get_next_block_after_slot(torsten_primitives::time::SlotNo(ledger_slot))
            {
                if let Ok(block) =
                    torsten_serialization::multi_era::decode_block_minimal_with_byron_epoch_length(
                        &cbor,
                        self.byron_epoch_length,
                    )
                {
                    let ledger_hash = ledger_tip.hash();
                    if ledger_hash.is_some_and(|h| h != block.prev_hash()) {
                        warn!(
                            "ChainDB fork divergence detected: ChainDB blocks after ledger tip \
                             do not connect (expected prev_hash={}, got {}). \
                             Using ledger tip only for intersection (excluding contaminated ChainDB points).",
                            ledger_hash.map(|h| h.to_hex()).unwrap_or_default(),
                            block.prev_hash().to_hex()
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
            // ChainDB has contaminated blocks (e.g., orphan fork flushed to
            // ImmutableDB). Exclude all ChainDB chain_points — they may
            // cause the peer to intersect at a block our ledger can't reach.
            // Only offer the ledger tip so the peer sends blocks from there.
            if ledger_tip != Point::Origin {
                known_points.push(ledger_tip.clone());
            }
        } else {
            // Ledger leads or tips are equal: offer ledger tip first, then
            // chain ancestry (which may include ancestors of a forged block
            // that are known to the peer even when the tip is not).
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

        // Log the full set of known_points for diagnosing intersection failures.
        // This is essential for understanding why a peer might reject our points
        // (e.g., stale immutable tip, forged block on orphaned fork).
        info!(
            chain_tip = %chain_tip,
            ledger_tip = %ledger_tip,
            chain_points_count = chain_points.len(),
            known_points_count = known_points.len(),
            use_chain_tip,
            "Intersection candidates",
        );
        for (i, p) in known_points.iter().enumerate() {
            debug!(idx = i, point = %p, "known_point");
        }

        // Find intersection: use pipelined client if available, otherwise serial client
        let (intersect, remote_tip) = if let Some(ref mut pc) = pipelined {
            pc.find_intersect(known_points.clone()).await?
        } else {
            client.find_intersect(known_points).await?
        };

        match &intersect {
            Some(point) => info!(point = %point, "Sync intersection found"),
            None => info!("Sync starting from Origin"),
        }
        info!(remote_tip = %remote_tip, "Remote tip");

        // ── Fork recovery ──────────────────────────────────────────────
        // If the intersection fell all the way to Origin but we have a
        // non-trivial ledger tip, the local chain has diverged from the
        // canonical one (e.g. a forged block that lost a slot battle was
        // persisted in the ledger snapshot).  Syncing from genesis with a
        // stale ledger is doomed to fail (blocks won't connect).
        //
        // Recovery: roll back ChainDB and the ledger state to the best
        // snapshot before the divergence, then retry the intersection.
        // This lets the node re-adopt the canonical chain without a full
        // wipe.
        //
        // Note: find_intersect returns Some(Point::Origin) when Origin is
        // in our known_points (it always is), so we must check for Origin
        // explicitly rather than checking for None.
        // ── Fork recovery ──────────────────────────────────────────────
        //
        // Two cases require fork recovery:
        //
        // (A) Intersection fell to Origin despite having a non-trivial
        //     ledger tip — no peer recognizes any of our chain points.
        //     Clear volatile, reset ledger, and reconnect.
        //
        // (B) Intersection is at a canonical block BEHIND the ledger tip —
        //     the local chain diverged (e.g. a slot battle where our
        //     forged block lost).  Reset the ledger and replay ImmutableDB
        //     blocks up to the intersection slot, then continue syncing.
        //
        // Note: find_intersect returns Some(Point::Origin) when Origin is
        // in our known_points (it always is), so we must check for Origin
        // explicitly rather than checking for None.
        let intersect_at_origin = intersect.as_ref().is_none_or(|p| *p == Point::Origin);
        let intersect_slot = intersect
            .as_ref()
            .and_then(|p| p.slot())
            .map(|s| s.0)
            .unwrap_or(0);

        // Case (A): no intersection found at all — multi-peer retry before
        // full reset.  A single peer returning Origin does NOT prove our chain
        // has diverged — the peer may be stale, on a minority fork, or have a
        // corrupted volatile DB.  Only after ORIGIN_INTERSECT_THRESHOLD
        // consecutive peers all return Origin do we perform the expensive full
        // ledger reset.
        const ORIGIN_INTERSECT_THRESHOLD: u32 = 3;

        if intersect_at_origin && ledger_slot > 0 {
            self.consecutive_origin_intersections =
                self.consecutive_origin_intersections.saturating_add(1);

            if self.consecutive_origin_intersections < ORIGIN_INTERSECT_THRESHOLD {
                warn!(
                    ledger_slot,
                    consecutive = self.consecutive_origin_intersections,
                    threshold = ORIGIN_INTERSECT_THRESHOLD,
                    "Fork recovery: peer returned Origin intersection ({}/{}). \
                     Trying a different peer before triggering full reset.",
                    self.consecutive_origin_intersections,
                    ORIGIN_INTERSECT_THRESHOLD,
                );
                return Err(anyhow::anyhow!(
                    "fork recovery: peer returned Origin intersection ({}/{}), \
                     retrying with different peer",
                    self.consecutive_origin_intersections,
                    ORIGIN_INTERSECT_THRESHOLD,
                ));
            }

            // All N peers returned Origin — genuinely diverged.
            warn!(
                ledger_slot,
                consecutive = self.consecutive_origin_intersections,
                "Fork recovery: {} consecutive peers returned Origin — \
                 our chain has diverged. Resetting ledger and reconnecting.",
                self.consecutive_origin_intersections,
            );
            self.consecutive_origin_intersections = 0;

            {
                let mut db = self.chain_db.write().await;
                db.clear_volatile();
            }

            // Reset epoch transition counters so that the subsequent replay
            // from genesis produces the correct epoch count.
            self.epoch_transitions_observed = 0;
            self.live_epoch_transitions = 0;

            self.handle_rollback(&Point::Origin).await;
            self.consensus.set_strict_verification(false);

            return Err(anyhow::anyhow!(
                "fork recovery: reset diverged ledger state, reconnecting"
            ));
        }

        // Non-Origin intersection found — our chain is canonical.
        // Reset the consecutive-Origin counter.
        if !intersect_at_origin && self.consecutive_origin_intersections > 0 {
            debug!(
                cleared = self.consecutive_origin_intersections,
                "Fork recovery: non-Origin intersection, resetting Origin counter"
            );
            self.consecutive_origin_intersections = 0;
        }

        // Case (B): intersection behind ledger tip — targeted replay.
        if intersect_slot > 0 && intersect_slot < ledger_slot {
            warn!(
                intersect_slot,
                ledger_slot,
                "Fork recovery: intersection is behind ledger tip — \
                 replaying ImmutableDB up to intersection."
            );

            {
                let mut db = self.chain_db.write().await;
                db.clear_volatile();
            }

            // Reset ledger and replay up to the intersection slot.
            // Detach the LSM UTxO store so the replay uses a fast
            // in-memory UtxoSet (~30K blk/s vs ~300 blk/s with LSM).
            // The store is reattached after replay completes.
            let immutable_dir = self.database_path.join("immutable");
            if immutable_dir.is_dir() {
                let ledger_state = self.ledger_state.clone();
                let bel = self.byron_epoch_length;
                let target = intersect_slot;

                let saved_utxo_store = {
                    let mut ls = ledger_state.write().await;
                    let utxo_store = ls.utxo_set.detach_store();
                    *ls = torsten_ledger::LedgerState::new(ls.protocol_params.clone());
                    utxo_store
                };

                info!(
                    target_slot = target,
                    "Fork recovery: replaying from genesis to intersection"
                );

                let replay_result = tokio::task::spawn_blocking(move || {
                    let start = std::time::Instant::now();
                    let mut replayed = 0u64;
                    let mut last_log = std::time::Instant::now();

                    // Hold write lock for the entire replay to avoid
                    // per-block lock contention with N2C/N2N servers.
                    let mut ls = ledger_state.blocking_write();

                    let result = crate::mithril::replay_from_chunk_files(
                        &immutable_dir,
                        |cbor| {
                            match torsten_serialization::multi_era::decode_block_minimal_with_byron_epoch_length(cbor, bel) {
                                Ok(block) => {
                                    if block.slot().0 > target {
                                        return Err(anyhow::anyhow!("reached target slot"));
                                    }
                                    if let Err(e) = ls.apply_block(&block, BlockValidationMode::ApplyOnly) {
                                        tracing::debug!(slot = block.slot().0, "Fork recovery: apply skipped: {e}");
                                    }
                                    replayed += 1;
                                    if last_log.elapsed().as_secs() >= 5 {
                                        let elapsed = start.elapsed().as_secs();
                                        let speed = if elapsed > 0 { replayed / elapsed } else { replayed };
                                        tracing::info!(
                                            replayed,
                                            slot = block.slot().0,
                                            target_slot = target,
                                            speed = format_args!("{speed} blk/s"),
                                            "Fork recovery: replay progress",
                                        );
                                        last_log = std::time::Instant::now();
                                    }
                                    Ok(())
                                }
                                Err(e) => {
                                    tracing::warn!("Fork recovery: decode error: {e}");
                                    Ok(())
                                }
                            }
                        },
                    );

                    drop(ls);

                    let elapsed = start.elapsed().as_secs_f64();
                    tracing::info!(
                        replayed,
                        target_slot = target,
                        elapsed_secs = format!("{elapsed:.1}"),
                        "Fork recovery: replay complete"
                    );

                    match result {
                        Ok(_) | Err(_) => replayed,
                    }
                })
                .await;

                // Reattach the LSM UTxO store.  The in-memory UtxoSet
                // built during replay has the correct canonical state;
                // the LSM store will be reconciled on the next snapshot.
                if let Some(store) = saved_utxo_store {
                    let mut ls = self.ledger_state.write().await;
                    ls.attach_utxo_store(store);
                }

                match replay_result {
                    Ok(n) => info!(blocks = n, "Fork recovery: finished"),
                    Err(e) => error!("Fork recovery: replay task failed: {e}"),
                }
            }

            self.consensus.set_strict_verification(false);
        }

        // Genesis State Machine: register this peer with the intersection slot so
        // that the GDD can anchor its density window correctly.  `register_peer`
        // is a no-op when genesis mode is disabled (GSM in CaughtUp state).
        {
            let intersection_slot = intersect
                .as_ref()
                .and_then(|p| p.slot())
                .map(|s| s.0)
                .unwrap_or(0);
            let remote_tip_slot = remote_tip.point.slot().map(|s| s.0).unwrap_or(0);
            self.gsm
                .write()
                .await
                .register_peer(peer_addr, intersection_slot, remote_tip_slot);
        }

        // Stale peer detection: if the remote tip is significantly behind the
        // current wall-clock slot, this peer is likely stale or stuck. Disconnect
        // and let the outer loop try a different peer. This handles the case where
        // the node reconnects after sleep/hibernate and reaches a stale peer.
        if let Some(wall_clock) = self.current_wall_clock_slot() {
            let remote_tip_slot = remote_tip.point.slot().map(|s| s.0).unwrap_or(0);
            let lag_slots = wall_clock.0.saturating_sub(remote_tip_slot);
            // Allow 120 slots (2 minutes) of lag for normal network propagation
            if lag_slots > 600 {
                warn!(
                    remote_tip_slot,
                    wall_clock_slot = wall_clock.0,
                    lag_slots,
                    "Peer tip is {} slots behind wall clock, skipping stale peer",
                    lag_slots
                );
                return Err(anyhow::anyhow!(
                    "peer tip is {lag_slots} slots behind wall clock (stale)"
                ));
            }
        }

        let use_pool = !fetch_pool.is_empty();
        let use_pipelined = pipelined.is_some();
        // Pipeline depth configurable via TORSTEN_PIPELINE_DEPTH env var (default: 150)
        // Benchmarked optimal: 150 yields ~275 blocks/sec vs ~151 at depth 100
        let max_pipeline_depth: usize = std::env::var("TORSTEN_PIPELINE_DEPTH")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(150);
        // When at tip, reduce to 1 to avoid sending many MsgRequestNext that
        // each need a new block (~20s) before the server responds.
        let mut pipeline_depth = max_pipeline_depth;
        if use_pipelined {
            info!(
                depth = max_pipeline_depth,
                fetchers = fetch_pool.len(),
                "Sync mode: pipelined",
            );
        } else if use_pool {
            info!(fetchers = fetch_pool.len(), "Sync mode: multi-peer");
        }

        let mut blocks_received: u64 = 0;
        let mut consecutive_apply_failures: u32 = 0;
        let mut last_snapshot_epoch: u64 = self.ledger_state.read().await.epoch.0;
        let mut last_log_time = std::time::Instant::now();
        let mut last_query_update = std::time::Instant::now();
        let mut blocks_since_last_log: u64 = 0;
        // Header batch size configurable via TORSTEN_HEADER_BATCH_SIZE env var
        let header_batch_size: usize = std::env::var("TORSTEN_HEADER_BATCH_SIZE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(if use_pipelined || use_pool { 500 } else { 100 });

        // Slot ticker for block production: fires every slot_length seconds
        let slot_length_secs = self
            .shelley_genesis
            .as_ref()
            .map(|g| g.slot_length)
            .unwrap_or(1);
        let mut forge_ticker =
            tokio::time::interval(tokio::time::Duration::from_secs(slot_length_secs));
        forge_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // Track the last slot we checked for forging to avoid duplicate checks
        let mut last_forge_slot: u64 = 0;

        // Pipeline decoupling: when both a pipelined client and fetch pool are
        // available, spawn a separate task for header/block fetching. This allows
        // network I/O to stay saturated while the main task processes blocks.
        // The fetch task sends blocks through a bounded channel; the main task
        // consumes and applies them. This matches cardano-node's architecture
        // where block download and ledger application run on separate threads.
        if use_pipelined && use_pool {
            /// Messages from the block fetch pipeline to the processing loop.
            enum PipelineMsg {
                /// A batch of blocks fetched from the network.
                Batch {
                    blocks: Vec<torsten_primitives::block::Block>,
                    tip: torsten_primitives::block::Tip,
                    fetch_ms: f64,
                    header_count: u64,
                    /// Byron EBB hashes encountered while collecting the headers
                    /// for this batch.  Each entry identifies an EBB whose hash
                    /// is the `prev_hash` of `next_block_hash`, allowing the
                    /// ledger apply loop to advance the tip through the EBB
                    /// before applying the block that references it.
                    ebb_hashes: Vec<torsten_network::EbbInfo>,
                },
                /// Chain rollback — process any preceding blocks, then rollback.
                Rollback(Point),
                /// Caught up to chain tip — enable strict verification.
                AtTip,
                /// Fetch error — abort the pipeline.
                FetchError(String),
            }

            let mut pc = pipelined
                .take()
                .expect("use_pipelined implies pipelined is Some");
            let (depth_tx, depth_rx) = tokio::sync::watch::channel(max_pipeline_depth);
            // Bounded channel: 4 batches of buffering allows network to stay
            // saturated while CPU catches up on block processing.
            let (block_tx, mut block_rx) = tokio::sync::mpsc::channel::<PipelineMsg>(4);
            let fetch_shutdown = shutdown_rx.clone();

            let fetch_handle = tokio::spawn(async move {
                loop {
                    if *fetch_shutdown.borrow() {
                        break;
                    }
                    let depth = *depth_rx.borrow();

                    let result = pc
                        .request_headers_pipelined_with_depth(header_batch_size, depth)
                        .await;
                    match result {
                        Ok(HeaderBatchResult::Headers(headers, tip, ebb_hashes)) => {
                            if headers.is_empty() {
                                continue;
                            }
                            debug!(
                                header_count = headers.len(),
                                first_slot = headers[0].slot,
                                last_slot = headers.last().expect("headers is non-empty").slot,
                                ebb_count = ebb_hashes.len(),
                                "Pipeline: headers received"
                            );
                            let fetch_start = std::time::Instant::now();
                            let header_count = headers.len() as u64;
                            match fetch_pool.fetch_blocks_concurrent(&headers).await {
                                Ok(result) => {
                                    let fetch_ms = fetch_start.elapsed().as_secs_f64() * 1000.0;
                                    if !result.failed_fetchers.is_empty() {
                                        warn!(
                                            failed = ?result.failed_fetchers,
                                            "Some fetchers failed during batch fetch"
                                        );
                                    }
                                    if block_tx
                                        .send(PipelineMsg::Batch {
                                            blocks: result.blocks,
                                            tip,
                                            fetch_ms,
                                            header_count,
                                            ebb_hashes,
                                        })
                                        .await
                                        .is_err()
                                    {
                                        break; // receiver dropped
                                    }
                                }
                                Err(e) => {
                                    let _ = block_tx
                                        .send(PipelineMsg::FetchError(format!("{e}")))
                                        .await;
                                    break;
                                }
                            }
                        }
                        Ok(HeaderBatchResult::HeadersAndRollback {
                            headers,
                            tip,
                            rollback_point,
                            ebb_hashes,
                            ..
                        }) => {
                            // Fetch blocks for headers before the rollback point
                            if !headers.is_empty() {
                                if let Ok(result) =
                                    fetch_pool.fetch_blocks_concurrent(&headers).await
                                {
                                    let _ = block_tx
                                        .send(PipelineMsg::Batch {
                                            blocks: result.blocks,
                                            tip,
                                            fetch_ms: 0.0,
                                            header_count: headers.len() as u64,
                                            ebb_hashes,
                                        })
                                        .await;
                                }
                            }
                            if block_tx
                                .send(PipelineMsg::Rollback(rollback_point))
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                        Ok(HeaderBatchResult::RollBackward(point, _)) => {
                            if block_tx.send(PipelineMsg::Rollback(point)).await.is_err() {
                                break;
                            }
                        }
                        Ok(HeaderBatchResult::HeadersAtTip(headers, tip, ebb_hashes)) => {
                            // We got headers AND caught up to tip. Fetch the
                            // blocks, send the batch, then signal AtTip.
                            if !headers.is_empty() {
                                debug!(
                                    header_count = headers.len(),
                                    first_slot = headers[0].slot,
                                    last_slot = headers.last().expect("non-empty").slot,
                                    ebb_count = ebb_hashes.len(),
                                    "Pipeline: headers at tip"
                                );
                                let fetch_start = std::time::Instant::now();
                                let header_count = headers.len() as u64;
                                match fetch_pool.fetch_blocks_concurrent(&headers).await {
                                    Ok(result) => {
                                        let fetch_ms = fetch_start.elapsed().as_secs_f64() * 1000.0;
                                        if !result.failed_fetchers.is_empty() {
                                            warn!(
                                                failed = ?result.failed_fetchers,
                                                "Some fetchers failed during at-tip batch fetch"
                                            );
                                        }
                                        if block_tx
                                            .send(PipelineMsg::Batch {
                                                blocks: result.blocks,
                                                tip,
                                                fetch_ms,
                                                header_count,
                                                ebb_hashes,
                                            })
                                            .await
                                            .is_err()
                                        {
                                            break;
                                        }
                                    }
                                    Err(e) => {
                                        let _ = block_tx
                                            .send(PipelineMsg::FetchError(format!("{e}")))
                                            .await;
                                        break;
                                    }
                                }
                            }
                            if block_tx.send(PipelineMsg::AtTip).await.is_err() {
                                break;
                            }
                        }
                        Ok(HeaderBatchResult::Await) => {
                            // Depth reduction is signaled by the main loop via
                            // the watch channel when it processes AtTip.
                            if block_tx.send(PipelineMsg::AtTip).await.is_err() {
                                break;
                            }
                        }
                        Err(e) => {
                            let _ = block_tx.send(PipelineMsg::FetchError(format!("{e}"))).await;
                            break;
                        }
                    }

                    // Connection stays open at tip — remaining in-flight
                    // requests drain naturally as new blocks arrive.
                }
            });

            // Processing loop: consume block batches from the pipeline channel.
            // Block processing and network fetching now run concurrently.
            loop {
                tokio::select! {
                    msg = block_rx.recv() => {
                        match msg {
                            Some(PipelineMsg::Batch { blocks, tip, fetch_ms, header_count, ebb_hashes }) => {
                                if header_count > 0 {
                                    self.metrics.record_block_fetch_latency(fetch_ms / header_count as f64);
                                }
                                self.peer_manager.write().await.record_block_fetch(
                                    &peer_addr, fetch_ms, header_count, 0,
                                );

                                // GDD density tracking: record every block slot for this
                                // peer so the genesis density comparator has accurate data.
                                // Collect slots before moving `blocks` into process_forward_blocks.
                                let batch_slots: Vec<u64> = blocks
                                    .iter()
                                    .map(|b| b.slot().0)
                                    .collect();
                                let remote_tip_slot = tip.point.slot().map(|s| s.0).unwrap_or(0);

                                let applied = self.process_forward_blocks(
                                    blocks, &tip, &ebb_hashes,
                                    &mut blocks_received,
                                    &mut blocks_since_last_log, &mut last_snapshot_epoch,
                                    &mut last_log_time, &mut last_query_update,
                                ).await;

                                // Update GSM per-peer density and run GDD evaluation.
                                // This is a no-op in Praos mode (genesis disabled).
                                if applied > 0 {
                                    consecutive_apply_failures = 0;
                                    let to_disconnect = {
                                        let mut gsm = self.gsm.write().await;
                                        for slot in &batch_slots {
                                            gsm.record_block(&peer_addr, *slot);
                                        }
                                        gsm.update_peer_tip(&peer_addr, remote_tip_slot);
                                        gsm.gdd_evaluate()
                                    };
                                    // Disconnect GDD-flagged peers via the peer manager.
                                    if !to_disconnect.is_empty() {
                                        let mut pm = self.peer_manager.write().await;
                                        for addr in &to_disconnect {
                                            warn!(
                                                %addr,
                                                "GDD: disconnecting peer with insufficient chain density"
                                            );
                                            pm.peer_failed(addr);
                                        }
                                    }
                                } else if header_count > 0 {
                                    consecutive_apply_failures += 1;
                                    if consecutive_apply_failures >= 5 {
                                        error!(
                                            consecutive_apply_failures,
                                            "Ledger state diverged from chain — \
                                             blocks do not connect. Triggering \
                                             reconnect to re-establish intersection."
                                        );
                                        break;
                                    }
                                }
                            }
                            Some(PipelineMsg::Rollback(point)) => {
                                warn!("Rollback to {point}");
                                self.handle_rollback(&point).await;
                            }
                            Some(PipelineMsg::AtTip) => {
                                if !self.consensus.strict_verification() {
                                    info!(blocks_applied = blocks_received, "Caught up to chain tip");
                                    self.enable_strict_verification().await;
                                }
                                self.update_query_state().await;
                                self.try_forge_block().await;
                                // Reduce pipeline depth to 1 at tip
                                let _ = depth_tx.send(1);
                            }
                            Some(PipelineMsg::FetchError(e)) => {
                                warn!("Block fetch pipeline error: {e}");
                                break;
                            }
                            None => {
                                // Channel closed — fetch task exited (stale or shutdown)
                                debug!("Fetch pipeline channel closed, ending sync loop");
                                break;
                            }
                        }
                    }
                    _ = forge_ticker.tick(), if self.block_producer.is_some() => {
                        if let Some(wc) = self.current_wall_clock_slot() {
                            if wc.0 > last_forge_slot {
                                last_forge_slot = wc.0;
                                self.try_forge_block().await;
                            }
                        }
                    }
                    _ = shutdown_rx.changed() => {
                        info!("Shutdown: stopping sync");
                        break;
                    }
                }
            }

            // Cleanup: close channel and abort fetch task
            drop(block_rx);
            fetch_handle.abort();
        } else {
            // Sequential mode: no pipeline decoupling (single peer or no fetch pool)
            loop {
                if *shutdown_rx.borrow() {
                    info!("Shutdown: stopping sync");
                    break;
                }

                if use_pipelined || use_pool {
                    // Pipelined/multi-peer mode without separate fetch pool
                    let header_future = async {
                        if let Some(ref mut pc) = pipelined {
                            pc.request_headers_pipelined_with_depth(
                                header_batch_size,
                                pipeline_depth,
                            )
                            .await
                        } else {
                            client.request_headers_batch(header_batch_size).await
                        }
                    };
                    tokio::select! {
                        result = header_future => {
                            match result {
                                Ok(batch_result) => {
                                    match batch_result {
                                        HeaderBatchResult::Headers(headers, tip, ebb_hashes) => {
                                            if headers.len() > 10 && pipeline_depth < max_pipeline_depth {
                                                pipeline_depth = max_pipeline_depth;
                                            }
                                            if !headers.is_empty() {
                                                debug!(
                                                    header_count = headers.len(),
                                                    first_slot = headers[0].slot,
                                                    first_block = headers[0].block_no,
                                                    last_slot = headers.last().expect("headers is non-empty").slot,
                                                    last_block = headers.last().expect("headers is non-empty").block_no,
                                                    ebb_count = ebb_hashes.len(),
                                                    "Headers received from pipelined client"
                                                );
                                            }
                                            let fetch_start = std::time::Instant::now();
                                            let header_count = headers.len() as u64;
                                            let blocks_result = if fetch_pool.is_empty() {
                                                client.fetch_blocks_by_points(&headers).await
                                            } else {
                                                match fetch_pool.fetch_blocks_concurrent(&headers).await {
                                                    Ok(result) => {
                                                        if !result.failed_fetchers.is_empty() {
                                                            warn!(
                                                                failed = ?result.failed_fetchers,
                                                                "Some fetchers failed during sequential fetch"
                                                            );
                                                        }
                                                        Ok(result.blocks)
                                                    }
                                                    Err(e) => {
                                                        warn!("Pool fetch failed, falling back to primary peer: {e}");
                                                        client.fetch_blocks_by_points(&headers).await
                                                    }
                                                }
                                            };
                                            match blocks_result {
                                                Ok(blocks) => {
                                                    let fetch_ms = fetch_start.elapsed().as_secs_f64() * 1000.0;
                                                    if header_count > 0 {
                                                        self.metrics.record_block_fetch_latency(fetch_ms / header_count as f64);
                                                    }
                                                    self.peer_manager.write().await.record_block_fetch(
                                                        &peer_addr, fetch_ms, header_count, 0,
                                                    );
                                                    // GDD: collect slots before moving blocks
                                                    let batch_slots: Vec<u64> = blocks
                                                        .iter()
                                                        .map(|b| b.slot().0)
                                                        .collect();
                                                    let remote_tip_slot =
                                                        tip.point.slot().map(|s| s.0).unwrap_or(0);
                                                    let applied = self.process_forward_blocks(blocks, &tip, &ebb_hashes, &mut blocks_received, &mut blocks_since_last_log, &mut last_snapshot_epoch, &mut last_log_time, &mut last_query_update).await;
                                                    if applied > 0 {
                                                        consecutive_apply_failures = 0;
                                                        // GDD evaluation (no-op in Praos mode)
                                                        let to_disconnect = {
                                                            let mut gsm = self.gsm.write().await;
                                                            for slot in &batch_slots {
                                                                gsm.record_block(&peer_addr, *slot);
                                                            }
                                                            gsm.update_peer_tip(&peer_addr, remote_tip_slot);
                                                            gsm.gdd_evaluate()
                                                        };
                                                        if !to_disconnect.is_empty() {
                                                            let mut pm = self.peer_manager.write().await;
                                                            for addr in &to_disconnect {
                                                                warn!(%addr, "GDD: disconnecting sparse peer");
                                                                pm.peer_failed(addr);
                                                            }
                                                        }
                                                    } else if header_count > 0 {
                                                        consecutive_apply_failures += 1;
                                                        if consecutive_apply_failures >= 5 {
                                                            error!(
                                                                consecutive_apply_failures,
                                                                "Ledger state diverged from chain — \
                                                                 blocks do not connect. Triggering \
                                                                 reconnect to re-establish intersection."
                                                            );
                                                            break;
                                                        }
                                                    }
                                                }
                                                Err(e) => { error!("Block fetch failed: {e}"); break; }
                                            }
                                        }
                                        HeaderBatchResult::HeadersAndRollback { headers, tip, rollback_point, ebb_hashes, .. } => {
                                            if !headers.is_empty() {
                                                match fetch_pool.fetch_blocks_concurrent(&headers).await {
                                                    Ok(result) => {
                                                        self.process_forward_blocks(result.blocks, &tip, &ebb_hashes, &mut blocks_received, &mut blocks_since_last_log, &mut last_snapshot_epoch, &mut last_log_time, &mut last_query_update).await;
                                                    }
                                                    Err(e) => { warn!("Pool fetch failed during rollback batch: {e}"); }
                                                }
                                            }
                                            warn!("Rollback to {rollback_point}");
                                            self.handle_rollback(&rollback_point).await;
                                        }
                                        HeaderBatchResult::RollBackward(point, _tip) => {
                                            warn!("Rollback to {point}");
                                            self.handle_rollback(&point).await;
                                        }
                                        HeaderBatchResult::HeadersAtTip(headers, tip, ebb_hashes) => {
                                            // Got headers AND caught up to tip
                                            if !headers.is_empty() {
                                                let blocks_result = if fetch_pool.is_empty() {
                                                    client.fetch_blocks_by_points(&headers).await
                                                } else {
                                                    match fetch_pool.fetch_blocks_concurrent(&headers).await {
                                                        Ok(result) => {
                                                            if !result.failed_fetchers.is_empty() {
                                                                warn!(
                                                                    failed = ?result.failed_fetchers,
                                                                    "Some fetchers failed during at-tip fetch"
                                                                );
                                                            }
                                                            Ok(result.blocks)
                                                        }
                                                        Err(e) => {
                                                            warn!("Pool fetch failed, falling back to primary peer: {e}");
                                                            client.fetch_blocks_by_points(&headers).await
                                                        }
                                                    }
                                                };
                                                if let Ok(blocks) = blocks_result {
                                                    self.process_forward_blocks(blocks, &tip, &ebb_hashes, &mut blocks_received, &mut blocks_since_last_log, &mut last_snapshot_epoch, &mut last_log_time, &mut last_query_update).await;
                                                }
                                            }
                                            if !self.consensus.strict_verification() {
                                                info!(blocks_applied = blocks_received, "Caught up to chain tip");
                                                self.enable_strict_verification().await;
                                            }
                                            self.update_query_state().await;
                                            self.try_forge_block().await;
                                            pipeline_depth = 1;
                                        }
                                        HeaderBatchResult::Await => {
                                            if !self.consensus.strict_verification() {
                                                info!(blocks_applied = blocks_received, "Caught up to chain tip");
                                                self.enable_strict_verification().await;
                                            }
                                            self.update_query_state().await;
                                            self.try_forge_block().await;
                                            pipeline_depth = 1;
                                        }
                                    }
                                    // Connection stays open at tip — remaining in-flight
                                    // requests drain naturally as new blocks arrive.
                                }
                                Err(e) => { error!("Chain sync error: {e}"); break; }
                            }
                        }
                        _ = forge_ticker.tick(), if self.block_producer.is_some() && pipeline_depth <= 1 => {
                            if let Some(wc) = self.current_wall_clock_slot() {
                                if wc.0 > last_forge_slot {
                                    last_forge_slot = wc.0;
                                    self.try_forge_block().await;
                                }
                            }
                        }
                        _ = shutdown_rx.changed() => {
                            info!("Shutdown: stopping sync");
                            break;
                        }
                    }
                } else {
                    // Single-peer mode: use request_next_batch (headers + blocks from same peer)
                    tokio::select! {
                        result = client.request_next_batch(header_batch_size) => {
                            match result {
                                Ok(events) => {
                                    let mut forward_blocks = Vec::new();
                                    let mut other_events = Vec::new();

                                    for event in events {
                                        match event {
                                            ChainSyncEvent::RollForward(block, tip) => {
                                                forward_blocks.push((*block, tip));
                                            }
                                            other => other_events.push(other),
                                        }
                                    }

                                    if !forward_blocks.is_empty() {
                                        let tip = forward_blocks
                                            .last()
                                            .expect("forward_blocks is non-empty (checked above)")
                                            .1
                                            .clone();
                                        let blocks: Vec<_> =
                                            forward_blocks.into_iter().map(|(b, _)| b).collect();
                                        // Single-peer serial mode does not use ChainSync header
                                        // batching, so no EBBs are tracked separately — the
                                        // serial client fetches full blocks which already handle
                                        // EBBs via the gap-bridge mechanism.
                                        self.process_forward_blocks(blocks, &tip, &[], &mut blocks_received, &mut blocks_since_last_log, &mut last_snapshot_epoch, &mut last_log_time, &mut last_query_update).await;
                                    }

                                    for event in other_events {
                                        match event {
                                            ChainSyncEvent::RollBackward(point, tip) => {
                                                warn!("Rollback to {point}, tip: {tip}");
                                                self.handle_rollback(&point).await;
                                            }
                                            ChainSyncEvent::Await => {
                                                if !self.consensus.strict_verification() {
                                                    info!(blocks_applied = blocks_received, "Caught up to chain tip");
                                                    self.enable_strict_verification().await;
                                                }
                                                self.update_query_state().await;
                                            }
                                            ChainSyncEvent::RollForward(..) => {
                                                warn!("Unexpected RollForward in other_events, skipping");
                                                continue;
                                            }
                                        }
                                    }
                                }
                                Err(e) => { error!("Chain sync error: {e}"); break; }
                            }
                        }
                        _ = forge_ticker.tick(), if self.block_producer.is_some() => {
                            if let Some(wc) = self.current_wall_clock_slot() {
                                if wc.0 > last_forge_slot {
                                    last_forge_slot = wc.0;
                                    self.try_forge_block().await;
                                }
                            }
                        }
                        _ = shutdown_rx.changed() => {
                            info!("Shutdown: stopping sync");
                            break;
                        }
                    }
                }
            }
            fetch_pool.disconnect_all().await;
        }

        // GSM cleanup: remove this peer from density tracking on disconnect.
        // This prevents the GDD from comparing against stale density windows
        // from a peer that is no longer connected.
        self.gsm.write().await.deregister_peer(&peer_addr);

        debug!("Chain sync stopped after {blocks_received} blocks");
        Ok(())
    }

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
                // After a full replay from genesis, the ledger has processed
                // all epoch transitions. Record this so that
                // enable_strict_verification() can set nonce_established=true
                // and the block producer can forge blocks at tip.
                let replay_epoch = self.ledger_state.read().await.epoch.0;
                if replay_epoch > 0 {
                    self.epoch_transitions_observed = self
                        .epoch_transitions_observed
                        .saturating_add(replay_epoch as u32);
                    info!(
                        epoch = replay_epoch,
                        epoch_transitions_observed = self.epoch_transitions_observed,
                        "Replay epoch transitions counted"
                    );
                }
                return;
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
        // Mirror the chunk-file replay path: record the epoch reached so that
        // enable_strict_verification() can set nonce_established=true.
        let replay_epoch = self.ledger_state.read().await.epoch.0;
        if replay_epoch > 0 {
            self.epoch_transitions_observed = self
                .epoch_transitions_observed
                .saturating_add(replay_epoch as u32);
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
