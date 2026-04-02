//! LedgerSeq: Haskell-compatible anchored sequence of ledger state deltas.
//!
//! Matches Haskell's `LedgerDB.V2.LedgerSeq` — a single full anchor state at the
//! immutable tip plus a window of per-block deltas covering the volatile chain.
//! This enables O(1) rollback and O(checkpoint_interval) state reconstruction.
//!
//! # Architecture
//!
//! The design follows Haskell's V2 DiffTables approach because full `LedgerState`
//! clones are prohibitively large (~40–80 MB each for epoch snapshots alone).
//! Storing k full copies would require ~17–34 GB on preview and ~86–173 GB on
//! mainnet, which is infeasible.
//!
//! Instead:
//! - **Anchor**: One full `LedgerState` at the immutable tip.  Saved to disk on
//!   snapshot.  Rebuilt from the latest on-disk snapshot + ImmutableDB replay on
//!   restart.
//! - **Volatile deltas**: Per-block `LedgerDelta` recording ALL state changes,
//!   not just UTxO (also delegations, rewards, pools, governance, nonces, epoch
//!   transitions, protocol parameters).
//! - **Checkpoints**: Full `LedgerState` snapshots stored in memory every
//!   `checkpoint_interval` blocks (default 100).  Limits reconstruction cost to
//!   at most `checkpoint_interval` delta applications.
//!
//! # Memory budget (preview testnet, k=2160)
//!
//! | Component          | Count | Per-item size | Total     |
//! |--------------------|-------|---------------|-----------|
//! | Anchor             | 1     | ~80 MB        | ~80 MB    |
//! | Deltas             | 2160  | ~5–50 KB      | ~2–20 MB  |
//! | Checkpoints (k/100)| ~22   | ~80 MB        | ~1.76 GB  |
//!
//! Checkpoints dominate.  If memory pressure is a concern the checkpoint interval
//! can be increased (e.g. 500) at the cost of slower state reconstruction.
//!
//! # Rollback
//!
//! `rollback(n)` is O(1): it drops the trailing n deltas from the VecDeque and
//! removes any checkpoints that no longer have backing deltas.  The reconstructed
//! state for the new tip is obtained via `tip_state()`.
//!
//! # Haskell reference
//!
//! `ouroboros-consensus:LedgerDB/V2/LedgerSeq.hs` — `LedgerSeq`, `prune`,
//! `rollbackN`, `extend`.

use crate::state::{
    DRepRegistration, EpochSnapshots, LedgerState, PoolRegistration, ProposalState,
    StakeDistributionState,
};
use crate::utxo_diff::UtxoDiff;
use dugite_primitives::block::Point;
use dugite_primitives::hash::{Hash28, Hash32};
use dugite_primitives::protocol_params::ProtocolParameters;
use dugite_primitives::time::{BlockNo, EpochNo, SlotNo};
use dugite_primitives::transaction::{
    Anchor, Constitution, DRep, GovActionId, Rational, Voter, VotingProcedure,
};
use dugite_primitives::value::Lovelace;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;

// ─────────────────────────────────────────────────────────────────────────────
// LedgerDelta: all state changes produced by a single block
// ─────────────────────────────────────────────────────────────────────────────

/// All state changes produced by applying a single block to the ledger.
///
/// This must capture EVERY mutable field in `LedgerState` so that any
/// historical state within the volatile window can be exactly reconstructed
/// by replaying deltas forward from the nearest checkpoint.
///
/// `LedgerDelta` is intentionally flat — there is no nesting of "apply this
/// sub-delta then that one".  Each variant in each sub-list is self-contained.
///
/// # Forward-only semantics
///
/// Deltas are applied in the FORWARD direction (oldest → newest) during
/// reconstruction.  They are NOT unapplied during rollback — rollback simply
/// discards the trailing deltas and reconstructs the tip from scratch.  This
/// design avoids the complexity and subtle bugs of "inverse diff" logic.
#[derive(Debug, Clone)]
pub struct LedgerDelta {
    /// Block slot number.
    pub slot: SlotNo,
    /// Block header hash.
    pub hash: Hash32,
    /// Block number.
    pub block_no: BlockNo,

    /// UTxO changes: inserts (new outputs) and deletes (consumed outputs).
    pub utxo_diff: UtxoDiff,

    /// Delegation state changes produced by certificates in this block.
    pub delegation_changes: Vec<DelegationChange>,

    /// Pool state changes produced by certificates in this block.
    pub pool_changes: Vec<PoolChange>,

    /// Reward account state changes.
    pub reward_changes: Vec<RewardChange>,

    /// Governance state changes (DRep, vote delegation, proposals, votes,
    /// committee, ratification).
    pub governance_changes: Vec<GovernanceChange>,

    /// Epoch transition data if this block crossed an epoch boundary.
    /// Contains the full set of scalar field changes made during the
    /// transition (treasury, reserves, snapshots, protocol params, etc.).
    pub epoch_transition: Option<EpochTransitionDelta>,

    /// Scalar nonce / block production field updates for this block.
    pub block_fields: BlockFieldsDelta,
}

impl LedgerDelta {
    /// Create an empty delta for the given block header.
    pub fn new(slot: SlotNo, hash: Hash32, block_no: BlockNo) -> Self {
        LedgerDelta {
            slot,
            hash,
            block_no,
            utxo_diff: UtxoDiff::new(),
            delegation_changes: Vec::new(),
            pool_changes: Vec::new(),
            reward_changes: Vec::new(),
            governance_changes: Vec::new(),
            epoch_transition: None,
            block_fields: BlockFieldsDelta::default(),
        }
    }
}

// ─── Delegation ────────────────────────────────────────────────────────────

/// A change to the delegation map or pointer map.
#[derive(Debug, Clone)]
pub enum DelegationChange {
    /// New stake credential registered (added to reward_accounts with deposit).
    /// `pointer` is `Some` when registered via a certificate that also creates
    /// a pointer entry (Shelley StakeRegistration at a specific (slot, tx, cert)).
    Register {
        credential_hash: Hash32,
        is_script: bool,
        pointer: Option<dugite_primitives::credentials::Pointer>,
    },
    /// Stake credential deregistered (removed from delegations, reward_accounts).
    Deregister {
        credential_hash: Hash32,
        pointer: Option<dugite_primitives::credentials::Pointer>,
    },
    /// Delegation set or updated (credential → pool).
    Delegate {
        credential_hash: Hash32,
        pool_id: Hash28,
    },
    /// Delegation removed (e.g. stake address deregistered without re-delegation).
    Undelegate { credential_hash: Hash32 },
}

// ─── Pool ──────────────────────────────────────────────────────────────────

/// A change to the pool registration or retirement state.
#[derive(Debug, Clone)]
pub enum PoolChange {
    /// New pool registered (first-time registration, takes effect at epoch N+2).
    Register { params: PoolRegistration },
    /// Existing pool re-registered (parameters queued as future_pool_params).
    Reregister { params: PoolRegistration },
    /// Pool retirement announced for a future epoch.
    Retire { pool_id: Hash28, epoch: EpochNo },
    /// Pending retirement cancelled (re-registration before retirement epoch).
    CancelRetirement { pool_id: Hash28 },
}

// ─── Rewards ───────────────────────────────────────────────────────────────

/// A change to a reward account balance.
#[derive(Debug, Clone)]
pub enum RewardChange {
    /// Fee credited to reward account (from withdrawal certificate or deposit refund).
    Credit {
        credential_hash: Hash32,
        amount: Lovelace,
    },
    /// Reward withdrawn (balance reduced by withdrawal amount).
    Withdraw {
        credential_hash: Hash32,
        amount: Lovelace,
    },
    /// Reward account created (deposit held, initial balance 0).
    Create { credential_hash: Hash32 },
    /// Reward account destroyed.
    Destroy { credential_hash: Hash32 },
}

// ─── Governance ────────────────────────────────────────────────────────────

/// A change to Conway governance state.
#[derive(Debug, Clone)]
pub enum GovernanceChange {
    // DRep lifecycle
    DRepRegister {
        credential_hash: Hash32,
        registration: DRepRegistration,
        is_script: bool,
    },
    DRepUpdate {
        credential_hash: Hash32,
        anchor: Option<Anchor>,
        last_active_epoch: EpochNo,
    },
    DRepUnregister {
        credential_hash: Hash32,
    },

    // Vote delegation
    VoteDelegate {
        credential_hash: Hash32,
        drep: DRep,
    },
    VoteUndelegate {
        credential_hash: Hash32,
    },

    // Constitutional committee
    CommitteeHotAuth {
        cold_credential_hash: Hash32,
        hot_credential_hash: Hash32,
        cold_is_script: bool,
        hot_is_script: bool,
    },
    CommitteeResign {
        cold_credential_hash: Hash32,
        anchor: Option<Anchor>,
        is_script: bool,
    },

    // Governance proposals
    ProposeAction {
        action_id: GovActionId,
        proposal: ProposalState,
    },

    // Votes
    CastVote {
        action_id: GovActionId,
        voter: Voter,
        procedure: VotingProcedure,
    },

    // Ratification outcomes (applied at epoch boundary)
    Enacted {
        action_id: GovActionId,
        proposal: ProposalState,
    },
    Expired {
        action_id: GovActionId,
    },

    // Constitutional updates
    SetConstitution {
        constitution: Constitution,
    },
    SetNoConfidence {
        no_confidence: bool,
    },
    SetCommitteeThreshold {
        threshold: Option<Rational>,
    },

    // Governance action counters
    IncrementDRepCount,
    IncrementProposalCount,
}

// ─── Epoch transition ──────────────────────────────────────────────────────

/// All scalar and collection changes made during an epoch transition.
///
/// When `process_epoch_transition()` runs, every field it touches is captured
/// here so that the transition can be replayed during state reconstruction.
/// This avoids having to re-run the full epoch transition logic (which is
/// expensive and stateful) during delta application.
#[derive(Debug, Clone)]
pub struct EpochTransitionDelta {
    /// The epoch number after the transition.
    pub new_epoch: EpochNo,
    /// New treasury balance.
    pub treasury: Lovelace,
    /// New reserves balance.
    pub reserves: Lovelace,
    /// Updated epoch snapshots (mark/set/go rotation).
    pub snapshots: EpochSnapshots,
    /// New protocol parameters (after PPUP/governance ratification).
    pub protocol_params: ProtocolParameters,
    /// Previous protocol parameters (swap during PPUP).
    pub prev_protocol_params: ProtocolParameters,
    /// Updated prev_d value.
    pub prev_d: f64,
    /// Updated prev_protocol_version_major.
    pub prev_protocol_version_major: u64,
    /// Cleared pending PP updates (pre-Conway).
    pub pending_pp_updates_cleared: bool,
    /// Epoch nonce updated at the transition.
    pub epoch_nonce: Hash32,
    /// New last_epoch_block_nonce.
    pub last_epoch_block_nonce: Hash32,
    /// Reward credits applied to individual accounts.
    pub reward_credits: HashMap<Hash32, Lovelace>,
    /// Pool retirements processed: pools removed.
    pub pools_retired: Vec<Hash28>,
    /// Future pool params promoted to pool_params.
    pub future_params_promoted: Vec<(Hash28, PoolRegistration)>,
    /// DRep active flags updated (credential_hash → new active state).
    pub drep_activity_updates: HashMap<Hash32, bool>,
    /// Last ratified and expired proposals (for GetRatifyState).
    pub last_ratified: Vec<(GovActionId, ProposalState)>,
    pub last_expired: Vec<GovActionId>,
    pub last_ratify_delayed: bool,
    /// Constitution set during this transition.
    pub new_constitution: Option<Constitution>,
    /// No-confidence state updated.
    pub no_confidence: Option<bool>,
    /// Committee threshold updated.
    pub committee_threshold: Option<Option<Rational>>,
    /// Proposals enacted by governance actions: proposals removed from
    /// active set.
    pub proposals_enacted: Vec<GovActionId>,
    /// Proposals expired: removed from active set.
    pub proposals_expired: Vec<GovActionId>,
    /// Enacted protocol param update IDs.
    pub enacted_pparam_update: Option<Option<GovActionId>>,
    pub enacted_hard_fork: Option<Option<GovActionId>>,
    pub enacted_committee: Option<Option<GovActionId>>,
    pub enacted_constitution: Option<Option<GovActionId>>,
    /// Post-transition stake distribution rebuild result.
    pub stake_distribution: StakeDistributionState,
    /// Delegation changes during transition (e.g. retiring pool delegator moves).
    pub delegation_changes: Vec<DelegationChange>,
}

// ─── Per-block scalar fields ───────────────────────────────────────────────

/// Scalar and nonce fields updated by each individual block.
///
/// These fields are updated by every block (not just epoch transitions) and
/// must be captured so that the exact historical state can be reconstructed.
#[derive(Debug, Clone)]
pub struct BlockFieldsDelta {
    /// Fee accumulated in this block (added to epoch_fees).
    pub fees_collected: Lovelace,
    /// The pool that produced this block (pool_id whose block count to
    /// increment by 1).  `None` for Byron blocks / blocks with no VRF proof.
    pub pool_block_increment: Option<Hash28>,
    /// Total epoch_block_count after this block.
    pub epoch_block_count: u64,
    /// Updated evolving_nonce (post-block).
    pub evolving_nonce: Hash32,
    /// Updated candidate_nonce (post-block; may be same as pre-block if
    /// the randomness stabilisation window has passed).
    pub candidate_nonce: Hash32,
    /// Updated lab_nonce (= prev_hash of this block).
    pub lab_nonce: Hash32,
    /// epoch_fees running total after this block.
    pub epoch_fees: Lovelace,
}

impl Default for BlockFieldsDelta {
    fn default() -> Self {
        BlockFieldsDelta {
            fees_collected: Lovelace(0),
            pool_block_increment: None,
            epoch_block_count: 0,
            evolving_nonce: Hash32::ZERO,
            candidate_nonce: Hash32::ZERO,
            lab_nonce: Hash32::ZERO,
            epoch_fees: Lovelace(0),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// LedgerSeq
// ─────────────────────────────────────────────────────────────────────────────

/// Anchored sequence of ledger state deltas.
///
/// Implements Haskell's `LedgerDB.V2.LedgerSeq` (DiffTables variant):
/// one full anchor state at the immutable tip plus a window of per-block
/// deltas covering the volatile chain.
///
/// # Invariants
///
/// 1. `deltas.len() <= k` at all times (enforced by `push`).
/// 2. `checkpoints` keys are indices into the current `deltas` window, i.e.
///    every key `i` satisfies `i < deltas.len()`.
/// 3. The state at delta index `i` equals the anchor with deltas `[0..=i]`
///    applied.
/// 4. `anchor_point` matches `anchor.tip.point`.
pub struct LedgerSeq {
    /// Full ledger state at the immutable tip (the anchor point).
    ///
    /// This is the only full state ever saved to disk.  All volatile states
    /// are derived by applying deltas forward from the anchor.
    anchor: Box<LedgerState>,

    /// The chain point corresponding to the anchor state.
    anchor_point: Point,

    /// Per-block deltas in chronological order (oldest at front, newest at back).
    ///
    /// `deltas[0]` is the first block applied after the anchor.
    /// `deltas[deltas.len()-1]` is the tip delta.
    deltas: VecDeque<LedgerDelta>,

    /// Full `LedgerState` checkpoints stored in memory every
    /// `checkpoint_interval` deltas.
    ///
    /// Key: delta index after which the checkpoint was taken.  E.g. if
    /// `checkpoint_interval = 100` then checkpoints exist at indices
    /// 99, 199, 299, …  (the state produced by applying deltas `[0..=99]`
    /// is stored at key `99`).
    ///
    /// During reconstruction of the state at delta index `i`, the nearest
    /// checkpoint at index `j <= i` is loaded and then deltas `[j+1..=i]`
    /// are applied.
    checkpoints: BTreeMap<usize, Box<LedgerState>>,

    /// Number of deltas between consecutive checkpoints.
    checkpoint_interval: usize,

    /// Security parameter k: maximum number of volatile deltas retained.
    k: u64,
}

impl LedgerSeq {
    /// Create a new `LedgerSeq` anchored at the given state.
    ///
    /// # Parameters
    ///
    /// - `anchor`: Full ledger state at the immutable tip.  Ownership is
    ///   transferred; the caller should not hold another copy.
    /// - `k`: Security parameter (number of blocks for rollback window).
    ///   The volatile window will hold at most `k` deltas before the anchor
    ///   is advanced.
    /// - `checkpoint_interval`: How often to store a full checkpoint in
    ///   memory.  Default 100; increasing reduces memory at the cost of
    ///   slower reconstruction.
    pub fn new(anchor: LedgerState, k: u64, checkpoint_interval: usize) -> Self {
        let anchor_point = anchor.tip.point.clone();
        LedgerSeq {
            anchor: Box::new(anchor),
            anchor_point,
            deltas: VecDeque::new(),
            checkpoints: BTreeMap::new(),
            checkpoint_interval,
            k,
        }
    }

    /// Create a `LedgerSeq` with default settings (checkpoint every 100 blocks).
    pub fn with_defaults(anchor: LedgerState, k: u64) -> Self {
        Self::new(anchor, k, 100)
    }

    // ── Accessors ────────────────────────────────────────────────────────────

    /// Current anchor point (immutable tip).
    pub fn anchor_point(&self) -> &Point {
        &self.anchor_point
    }

    /// Number of volatile deltas currently held.
    pub fn len(&self) -> usize {
        self.deltas.len()
    }

    /// Whether the volatile window is empty (chain tip == anchor).
    pub fn is_empty(&self) -> bool {
        self.deltas.is_empty()
    }

    /// Maximum rollback depth: number of blocks that can be rolled back
    /// without losing state.  Equals `deltas.len()`.
    pub fn max_rollback(&self) -> usize {
        self.deltas.len()
    }

    /// The chain point at the current tip (newest delta, or anchor if empty).
    pub fn tip_point(&self) -> Point {
        if let Some(d) = self.deltas.back() {
            // Reconstruct a Specific point from the tip delta
            dugite_primitives::block::Point::Specific(d.slot, d.hash)
        } else {
            self.anchor_point.clone()
        }
    }

    /// Reference to the raw anchor state (the immutable tip).
    ///
    /// Callers needing the volatile-tip state should use `tip_state()`.
    pub fn anchor_state(&self) -> &LedgerState {
        &self.anchor
    }

    // ── State reconstruction ─────────────────────────────────────────────────

    /// Reconstruct the ledger state at the current tip by applying deltas
    /// from the nearest checkpoint.
    ///
    /// Cost: O(`checkpoint_interval`) delta applications — at most
    /// `checkpoint_interval` blocks regardless of how many deltas exist.
    pub fn tip_state(&self) -> LedgerState {
        if self.deltas.is_empty() {
            return (*self.anchor).clone();
        }
        self.state_at_index(self.deltas.len() - 1)
    }

    /// Reconstruct the ledger state after applying the first `index + 1`
    /// deltas (0-indexed).
    ///
    /// Returns `None` if `index >= deltas.len()`.
    pub fn state_at_index(&self, index: usize) -> LedgerState {
        debug_assert!(
            index < self.deltas.len(),
            "state_at_index: index {} out of bounds (deltas.len()={})",
            index,
            self.deltas.len()
        );

        // Find the nearest checkpoint at or before `index`.
        let (start_index, base_state) = match self.checkpoints.range(..=index).next_back() {
            Some((&cp_idx, cp_state)) => {
                // Apply deltas from cp_idx+1 through index.
                (cp_idx + 1, (**cp_state).clone())
            }
            None => {
                // No checkpoint before this index — start from anchor.
                (0, (*self.anchor).clone())
            }
        };

        // Apply deltas [start_index..=index].
        let mut state = base_state;
        for i in start_index..=index {
            let delta = &self.deltas[i];
            apply_delta_to_state(&mut state, delta);
        }
        state
    }

    /// Reconstruct the ledger state at a specific chain point within the
    /// volatile window.
    ///
    /// Returns `None` if the point is not found in the volatile window.
    ///
    /// Cost: O(`checkpoint_interval`) delta applications.
    pub fn state_at(&self, slot: SlotNo, hash: &Hash32) -> Option<LedgerState> {
        // Search deltas from newest to oldest for a matching point.
        let idx = self
            .deltas
            .iter()
            .enumerate()
            .rev()
            .find(|(_, d)| d.slot == slot && &d.hash == hash)
            .map(|(i, _)| i)?;

        Some(self.state_at_index(idx))
    }

    // ── Mutation ────────────────────────────────────────────────────────────

    /// Push a new block's delta onto the volatile window.
    ///
    /// If the number of deltas would exceed `k`, `advance_anchor` is called
    /// first to move the oldest delta into the anchor.
    ///
    /// After appending, if the new delta's index is a multiple of
    /// `checkpoint_interval - 1` (i.e. every N blocks), a full checkpoint
    /// is stored.
    pub fn push(&mut self, delta: LedgerDelta) {
        // Enforce the k-block volatile window by advancing the anchor when full.
        while self.deltas.len() >= self.k as usize {
            self.advance_anchor();
        }

        self.deltas.push_back(delta);
        let new_idx = self.deltas.len() - 1;

        // Store a checkpoint every `checkpoint_interval` blocks.
        // Checkpoint is taken at indices checkpoint_interval-1, 2*(checkpoint_interval)-1, …
        if (new_idx + 1).is_multiple_of(self.checkpoint_interval) {
            let cp = Box::new(self.state_at_index(new_idx));
            self.checkpoints.insert(new_idx, cp);
        }
    }

    /// Roll back `n` blocks.
    ///
    /// Removes the last `n` deltas from the volatile window and invalidates
    /// any checkpoints that pointed into the removed range.
    ///
    /// # Panics
    ///
    /// Panics in debug builds if `n > deltas.len()`.  In release builds
    /// it silently clamps to `deltas.len()`.
    ///
    /// Cost: O(n) for deque truncation + O(checkpoints pruned).
    pub fn rollback(&mut self, n: usize) {
        let n = n.min(self.deltas.len());
        let new_len = self.deltas.len() - n;

        // Trim deltas.
        self.deltas.truncate(new_len);

        // Drop checkpoints that pointed into the removed range.
        // A checkpoint at index `i` is valid only if `i < new_len`.
        self.checkpoints.retain(|&idx, _| idx < new_len);
    }

    /// Advance the anchor: apply the oldest delta to the anchor state, pop
    /// it from the deque, and re-index the remaining checkpoints.
    ///
    /// Called automatically by `push` when `deltas.len() >= k`, and
    /// explicitly when the immutable tip advances (copy-to-immutable).
    ///
    /// Cost: O(checkpoint_interval) for the anchor update + O(checkpoints)
    /// for re-indexing.
    pub fn advance_anchor(&mut self) {
        if self.deltas.is_empty() {
            return;
        }

        // Apply the oldest delta to the anchor.
        let oldest = self.deltas.pop_front().unwrap();
        apply_delta_to_state(&mut self.anchor, &oldest);
        self.anchor_point = dugite_primitives::block::Point::Specific(oldest.slot, oldest.hash);

        // Re-index checkpoints: every stored index shifts down by 1.
        // Checkpoints that were at index 0 (= the delta we just consumed)
        // are now part of the anchor — drop them.
        let old_checkpoints = std::mem::take(&mut self.checkpoints);
        self.checkpoints = old_checkpoints
            .into_iter()
            .filter_map(|(idx, state)| {
                if idx == 0 {
                    // This checkpoint was for the delta we just absorbed —
                    // it is now redundant (the anchor IS that state).
                    None
                } else {
                    Some((idx - 1, state))
                }
            })
            .collect();
    }

    /// Replace the anchor with a new full state (e.g. after loading a
    /// snapshot from disk).  Clears all volatile deltas and checkpoints.
    pub fn reset_anchor(&mut self, new_anchor: LedgerState) {
        self.anchor_point = new_anchor.tip.point.clone();
        *self.anchor = new_anchor;
        self.deltas.clear();
        self.checkpoints.clear();
    }

    /// Return a reference to all deltas (oldest first).  Used by the
    /// startup recovery path to replay volatile blocks.
    pub fn deltas(&self) -> &VecDeque<LedgerDelta> {
        &self.deltas
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Delta application
// ─────────────────────────────────────────────────────────────────────────────

/// Apply a single `LedgerDelta` to a `LedgerState` in-place.
///
/// This is the forward-direction application used during state reconstruction.
/// It is NOT used during rollback (rollback simply discards deltas).
///
/// Every field that can be modified by a block must be handled here.
/// If a new delta variant is added and this function is not updated, the
/// compiler will warn only if the enum is `#[non_exhaustive]` — therefore
/// reviewers MUST audit this function when adding new delta variants.
pub fn apply_delta_to_state(state: &mut LedgerState, delta: &LedgerDelta) {
    // ── 1. UTxO changes ─────────────────────────────────────────────────────
    apply_utxo_diff(state, &delta.utxo_diff);

    // ── 2. Delegation changes ────────────────────────────────────────────────
    for change in &delta.delegation_changes {
        apply_delegation_change(state, change);
    }

    // ── 3. Pool changes ──────────────────────────────────────────────────────
    for change in &delta.pool_changes {
        apply_pool_change(state, change);
    }

    // ── 4. Reward account changes ────────────────────────────────────────────
    for change in &delta.reward_changes {
        apply_reward_change(state, change);
    }

    // ── 5. Governance changes ─────────────────────────────────────────────────
    for change in &delta.governance_changes {
        apply_governance_change(state, change);
    }

    // ── 6. Epoch transition ───────────────────────────────────────────────────
    if let Some(et) = &delta.epoch_transition {
        apply_epoch_transition_delta(state, et);
    }

    // ── 7. Per-block scalar / nonce updates ───────────────────────────────────
    apply_block_fields(state, &delta.block_fields);

    // Update tip to reflect this block.
    state.tip = dugite_primitives::block::Tip {
        point: dugite_primitives::block::Point::Specific(delta.slot, delta.hash),
        block_number: delta.block_no,
    };
}

// ── UTxO ─────────────────────────────────────────────────────────────────────

fn apply_utxo_diff(state: &mut LedgerState, diff: &UtxoDiff) {
    for (input, output) in &diff.inserts {
        state.utxo_set.insert(input.clone(), output.clone());
    }
    for (input, _output) in &diff.deletes {
        state.utxo_set.remove(input);
    }
}

// ── Delegation ───────────────────────────────────────────────────────────────

fn apply_delegation_change(state: &mut LedgerState, change: &DelegationChange) {
    match change {
        DelegationChange::Register {
            credential_hash,
            is_script,
            pointer,
        } => {
            // Ensure reward account exists (registered with 0 balance).
            Arc::make_mut(&mut state.reward_accounts)
                .entry(*credential_hash)
                .or_insert(Lovelace(0));
            if *is_script {
                state.script_stake_credentials.insert(*credential_hash);
            }
            if let Some(ptr) = pointer {
                state.pointer_map.insert(*ptr, *credential_hash);
            }
        }
        DelegationChange::Deregister {
            credential_hash,
            pointer,
        } => {
            Arc::make_mut(&mut state.delegations).remove(credential_hash);
            Arc::make_mut(&mut state.reward_accounts).remove(credential_hash);
            state.script_stake_credentials.remove(credential_hash);
            if let Some(ptr) = pointer {
                state.pointer_map.remove(ptr);
            }
        }
        DelegationChange::Delegate {
            credential_hash,
            pool_id,
        } => {
            Arc::make_mut(&mut state.delegations).insert(*credential_hash, *pool_id);
        }
        DelegationChange::Undelegate { credential_hash } => {
            Arc::make_mut(&mut state.delegations).remove(credential_hash);
        }
    }
}

// ── Pool ─────────────────────────────────────────────────────────────────────

fn apply_pool_change(state: &mut LedgerState, change: &PoolChange) {
    match change {
        PoolChange::Register { params } => {
            Arc::make_mut(&mut state.pool_params).insert(params.pool_id, params.clone());
        }
        PoolChange::Reregister { params } => {
            state
                .future_pool_params
                .insert(params.pool_id, params.clone());
        }
        PoolChange::Retire { pool_id, epoch } => {
            state.pending_retirements.insert(*pool_id, *epoch);
        }
        PoolChange::CancelRetirement { pool_id } => {
            state.pending_retirements.remove(pool_id);
        }
    }
}

// ── Rewards ──────────────────────────────────────────────────────────────────

fn apply_reward_change(state: &mut LedgerState, change: &RewardChange) {
    match change {
        RewardChange::Credit {
            credential_hash,
            amount,
        } => {
            let accounts = Arc::make_mut(&mut state.reward_accounts);
            let entry = accounts.entry(*credential_hash).or_insert(Lovelace(0));
            entry.0 = entry.0.saturating_add(amount.0);
        }
        RewardChange::Withdraw {
            credential_hash,
            amount,
        } => {
            let accounts = Arc::make_mut(&mut state.reward_accounts);
            if let Some(bal) = accounts.get_mut(credential_hash) {
                bal.0 = bal.0.saturating_sub(amount.0);
            }
        }
        RewardChange::Create { credential_hash } => {
            Arc::make_mut(&mut state.reward_accounts)
                .entry(*credential_hash)
                .or_insert(Lovelace(0));
        }
        RewardChange::Destroy { credential_hash } => {
            Arc::make_mut(&mut state.reward_accounts).remove(credential_hash);
        }
    }
}

// ── Governance ───────────────────────────────────────────────────────────────

fn apply_governance_change(state: &mut LedgerState, change: &GovernanceChange) {
    let gov = Arc::make_mut(&mut state.governance);
    match change {
        GovernanceChange::DRepRegister {
            credential_hash,
            registration,
            is_script,
        } => {
            gov.dreps.insert(*credential_hash, registration.clone());
            if *is_script {
                // Script DReps don't have a separate tracking set in
                // GovernanceState currently, but we note this for future use.
            }
            gov.drep_registration_count += 1;
        }
        GovernanceChange::DRepUpdate {
            credential_hash,
            anchor,
            last_active_epoch,
        } => {
            if let Some(drep) = gov.dreps.get_mut(credential_hash) {
                drep.anchor = anchor.clone();
                drep.last_active_epoch = *last_active_epoch;
                drep.active = true;
            }
        }
        GovernanceChange::DRepUnregister { credential_hash } => {
            gov.dreps.remove(credential_hash);
            gov.vote_delegations.retain(|_, d| {
                // Remove delegations to this DRep (key credential).
                // Note: DRep::KeyHash is matched by credential_hash.
                !matches!(d, DRep::KeyHash(h) if h == credential_hash)
            });
        }
        GovernanceChange::VoteDelegate {
            credential_hash,
            drep,
        } => {
            gov.vote_delegations.insert(*credential_hash, drep.clone());
        }
        GovernanceChange::VoteUndelegate { credential_hash } => {
            gov.vote_delegations.remove(credential_hash);
        }
        GovernanceChange::CommitteeHotAuth {
            cold_credential_hash,
            hot_credential_hash,
            cold_is_script,
            hot_is_script,
        } => {
            gov.committee_hot_keys
                .insert(*cold_credential_hash, *hot_credential_hash);
            gov.committee_resigned.remove(cold_credential_hash);
            if *cold_is_script {
                gov.script_committee_credentials
                    .insert(*cold_credential_hash);
            }
            if *hot_is_script {
                gov.script_committee_hot_credentials
                    .insert(*hot_credential_hash);
            }
        }
        GovernanceChange::CommitteeResign {
            cold_credential_hash,
            anchor,
            is_script,
        } => {
            gov.committee_resigned
                .insert(*cold_credential_hash, anchor.clone());
            gov.committee_hot_keys.remove(cold_credential_hash);
            if *is_script {
                gov.script_committee_credentials
                    .insert(*cold_credential_hash);
            }
        }
        GovernanceChange::ProposeAction {
            action_id,
            proposal,
        } => {
            gov.proposals.insert(action_id.clone(), proposal.clone());
            gov.proposal_count += 1;
        }
        GovernanceChange::CastVote {
            action_id,
            voter,
            procedure,
        } => {
            let votes = gov.votes_by_action.entry(action_id.clone()).or_default();
            // Replace existing vote from this voter or append.
            if let Some(entry) = votes.iter_mut().find(|(v, _)| v == voter) {
                entry.1 = procedure.clone();
            } else {
                votes.push((voter.clone(), procedure.clone()));
            }
        }
        GovernanceChange::Enacted {
            action_id,
            proposal,
        } => {
            gov.proposals.remove(action_id);
            gov.votes_by_action.remove(action_id);
            gov.last_ratified
                .push((action_id.clone(), proposal.clone()));
        }
        GovernanceChange::Expired { action_id } => {
            gov.proposals.remove(action_id);
            gov.votes_by_action.remove(action_id);
            gov.last_expired.push(action_id.clone());
        }
        GovernanceChange::SetConstitution { constitution } => {
            gov.constitution = Some(constitution.clone());
        }
        GovernanceChange::SetNoConfidence { no_confidence } => {
            gov.no_confidence = *no_confidence;
        }
        GovernanceChange::SetCommitteeThreshold { threshold } => {
            gov.committee_threshold = threshold.clone();
        }
        GovernanceChange::IncrementDRepCount => {
            gov.drep_registration_count += 1;
        }
        GovernanceChange::IncrementProposalCount => {
            gov.proposal_count += 1;
        }
    }
}

// ── Epoch transition ──────────────────────────────────────────────────────────

fn apply_epoch_transition_delta(state: &mut LedgerState, et: &EpochTransitionDelta) {
    state.epoch = et.new_epoch;
    state.treasury = et.treasury;
    state.reserves = et.reserves;
    state.snapshots = et.snapshots.clone();
    state.protocol_params = et.protocol_params.clone();
    state.prev_protocol_params = et.prev_protocol_params.clone();
    state.prev_d = et.prev_d;
    state.prev_protocol_version_major = et.prev_protocol_version_major;
    state.epoch_nonce = et.epoch_nonce;
    state.last_epoch_block_nonce = et.last_epoch_block_nonce;
    state.stake_distribution = et.stake_distribution.clone();

    if et.pending_pp_updates_cleared {
        state.pending_pp_updates.clear();
        state.future_pp_updates.clear();
    }

    // Apply reward credits.
    {
        let accounts = Arc::make_mut(&mut state.reward_accounts);
        for (cred, amount) in &et.reward_credits {
            let bal = accounts.entry(*cred).or_insert(Lovelace(0));
            bal.0 = bal.0.saturating_add(amount.0);
        }
    }

    // Remove retired pools.
    {
        let pools = Arc::make_mut(&mut state.pool_params);
        for pool_id in &et.pools_retired {
            pools.remove(pool_id);
            state.future_pool_params.remove(pool_id);
        }
    }
    // Clean up pending retirements for the epoch just processed.
    state.pending_retirements.retain(|_, ep| *ep > et.new_epoch);

    // Promote future pool params.
    {
        let pools = Arc::make_mut(&mut state.pool_params);
        for (pool_id, params) in &et.future_params_promoted {
            pools.insert(*pool_id, params.clone());
            state.future_pool_params.remove(pool_id);
        }
    }

    // Update DRep activity flags.
    {
        let gov = Arc::make_mut(&mut state.governance);
        for (cred, active) in &et.drep_activity_updates {
            if let Some(drep) = gov.dreps.get_mut(cred) {
                drep.active = *active;
            }
        }
        gov.last_ratified = et.last_ratified.clone();
        gov.last_expired = et.last_expired.clone();
        gov.last_ratify_delayed = et.last_ratify_delayed;

        if let Some(c) = &et.new_constitution {
            gov.constitution = Some(c.clone());
        }
        if let Some(nc) = et.no_confidence {
            gov.no_confidence = nc;
        }
        if let Some(thresh) = &et.committee_threshold {
            gov.committee_threshold = thresh.clone();
        }
        for action_id in &et.proposals_enacted {
            gov.proposals.remove(action_id);
            gov.votes_by_action.remove(action_id);
        }
        for action_id in &et.proposals_expired {
            gov.proposals.remove(action_id);
            gov.votes_by_action.remove(action_id);
        }
        if let Some(v) = &et.enacted_pparam_update {
            gov.enacted_pparam_update = v.clone();
        }
        if let Some(v) = &et.enacted_hard_fork {
            gov.enacted_hard_fork = v.clone();
        }
        if let Some(v) = &et.enacted_committee {
            gov.enacted_committee = v.clone();
        }
        if let Some(v) = &et.enacted_constitution {
            gov.enacted_constitution = v.clone();
        }
    }

    // Apply transition-level delegation changes (e.g. retiring pool movers).
    for change in &et.delegation_changes {
        apply_delegation_change(state, change);
    }

    // Reset per-epoch counters.
    state.epoch_fees = Lovelace(0);
    Arc::make_mut(&mut state.epoch_blocks_by_pool).clear();
    state.epoch_block_count = 0;
}

// ── Per-block scalar / nonce fields ──────────────────────────────────────────

fn apply_block_fields(state: &mut LedgerState, fields: &BlockFieldsDelta) {
    // epoch_fees and epoch_block_count are already in the delta as the
    // running totals AFTER this block so we can just assign.
    state.epoch_fees = fields.epoch_fees;
    state.epoch_block_count = fields.epoch_block_count;
    state.evolving_nonce = fields.evolving_nonce;
    state.candidate_nonce = fields.candidate_nonce;
    state.lab_nonce = fields.lab_nonce;

    if let Some(pool_id) = fields.pool_block_increment {
        *Arc::make_mut(&mut state.epoch_blocks_by_pool)
            .entry(pool_id)
            .or_insert(0) += 1;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Snapshot helpers (stubs for Task 1.4 / 1.5)
// ─────────────────────────────────────────────────────────────────────────────

/// Reasons a `LedgerSeq` operation can fail.
#[derive(Debug)]
pub enum LedgerSeqError {
    /// Rollback depth exceeds the volatile window.
    RollbackExceedsWindow { requested: usize, available: usize },
    /// The given point is not in the volatile window.
    PointNotFound,
}

impl std::fmt::Display for LedgerSeqError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LedgerSeqError::RollbackExceedsWindow {
                requested,
                available,
            } => write!(
                f,
                "rollback depth {requested} exceeds volatile window ({available} available)"
            ),
            LedgerSeqError::PointNotFound => write!(f, "point not found in volatile window"),
        }
    }
}

impl std::error::Error for LedgerSeqError {}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::LedgerState;
    use dugite_primitives::protocol_params::ProtocolParameters;
    use dugite_primitives::time::{BlockNo, SlotNo};
    use dugite_primitives::value::Lovelace;

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn make_anchor() -> LedgerState {
        LedgerState::new(ProtocolParameters::mainnet_defaults())
    }

    fn make_hash(b: u8) -> Hash32 {
        Hash32::from_bytes([b; 32])
    }

    /// Build a minimal delta that records some fee collection so the
    /// resulting state is distinguishable from the anchor.
    fn make_delta(slot: u64, hash_byte: u8, fees: u64) -> LedgerDelta {
        let mut delta = LedgerDelta::new(SlotNo(slot), make_hash(hash_byte), BlockNo(slot));
        delta.block_fields = BlockFieldsDelta {
            fees_collected: Lovelace(fees),
            epoch_fees: Lovelace(fees), // Running total in this simple test
            epoch_block_count: slot,
            evolving_nonce: make_hash(hash_byte),
            candidate_nonce: make_hash(hash_byte),
            lab_nonce: make_hash(hash_byte),
            pool_block_increment: None,
        };
        delta
    }

    // ── Construction ──────────────────────────────────────────────────────────

    #[test]
    fn test_new_empty() {
        let anchor = make_anchor();
        let seq = LedgerSeq::with_defaults(anchor, 10);
        assert!(seq.is_empty());
        assert_eq!(seq.len(), 0);
        assert_eq!(seq.max_rollback(), 0);
    }

    // ── Push ─────────────────────────────────────────────────────────────────

    #[test]
    fn test_push_single_delta() {
        let anchor = make_anchor();
        let mut seq = LedgerSeq::with_defaults(anchor, 10);

        seq.push(make_delta(1, 1, 1_000_000));

        assert_eq!(seq.len(), 1);
        assert_eq!(seq.max_rollback(), 1);
    }

    #[test]
    fn test_push_multiple_deltas() {
        let anchor = make_anchor();
        let mut seq = LedgerSeq::with_defaults(anchor, 10);

        for i in 1u8..=5 {
            seq.push(make_delta(i as u64, i, i as u64 * 1_000_000));
        }

        assert_eq!(seq.len(), 5);
        assert_eq!(seq.max_rollback(), 5);
    }

    #[test]
    fn test_push_beyond_k_advances_anchor() {
        let anchor = make_anchor();
        let k = 5u64;
        let mut seq = LedgerSeq::with_defaults(anchor, k);

        // Push k+2 deltas — the anchor should advance twice.
        for i in 1u8..=(k as u8 + 2) {
            seq.push(make_delta(i as u64, i, i as u64 * 1_000_000));
        }

        // volatile window should be exactly k
        assert_eq!(seq.len(), k as usize);
        // anchor point should have advanced to delta[1] (0-indexed)
        assert!(matches!(
            seq.anchor_point(),
            dugite_primitives::block::Point::Specific(_, _)
        ));
    }

    // ── Rollback ──────────────────────────────────────────────────────────────

    #[test]
    fn test_rollback_zero_is_noop() {
        let anchor = make_anchor();
        let mut seq = LedgerSeq::with_defaults(anchor, 10);
        seq.push(make_delta(1, 1, 1_000_000));
        seq.push(make_delta(2, 2, 2_000_000));

        seq.rollback(0);
        assert_eq!(seq.len(), 2);
    }

    #[test]
    fn test_rollback_one() {
        let anchor = make_anchor();
        let mut seq = LedgerSeq::with_defaults(anchor, 10);
        seq.push(make_delta(1, 1, 1_000_000));
        seq.push(make_delta(2, 2, 2_000_000));

        seq.rollback(1);
        assert_eq!(seq.len(), 1);
        assert_eq!(seq.max_rollback(), 1);
    }

    #[test]
    fn test_rollback_all() {
        let anchor = make_anchor();
        let mut seq = LedgerSeq::with_defaults(anchor, 10);
        for i in 1u8..=5 {
            seq.push(make_delta(i as u64, i, 1_000_000));
        }

        seq.rollback(5);
        assert!(seq.is_empty());
        assert_eq!(seq.max_rollback(), 0);
    }

    #[test]
    fn test_rollback_clamps_to_available() {
        // rollback(n > len) should not panic — it clamps.
        let anchor = make_anchor();
        let mut seq = LedgerSeq::with_defaults(anchor, 10);
        seq.push(make_delta(1, 1, 0));
        seq.push(make_delta(2, 2, 0));

        seq.rollback(100); // more than available
        assert!(seq.is_empty());
    }

    #[test]
    fn test_rollback_invalidates_checkpoints() {
        let anchor = make_anchor();
        // checkpoint_interval = 3 → checkpoint at index 2 (deltas[0..=2])
        let mut seq = LedgerSeq::new(anchor, 20, 3);

        for i in 1u8..=6 {
            seq.push(make_delta(i as u64, i, 0));
        }
        // Checkpoints should exist at indices 2 and 5.
        assert!(seq.checkpoints.contains_key(&2));
        assert!(seq.checkpoints.contains_key(&5));

        // Roll back to len=3 (remove deltas 3,4,5).
        seq.rollback(3);
        assert_eq!(seq.len(), 3);
        // Checkpoint at index 5 should be gone; checkpoint at index 2 remains.
        assert!(!seq.checkpoints.contains_key(&5));
        assert!(seq.checkpoints.contains_key(&2));
    }

    // ── Advance anchor ────────────────────────────────────────────────────────

    #[test]
    fn test_advance_anchor_empty_is_noop() {
        let anchor = make_anchor();
        let mut seq = LedgerSeq::with_defaults(anchor, 10);
        seq.advance_anchor(); // should not panic
        assert!(seq.is_empty());
    }

    #[test]
    fn test_advance_anchor_updates_anchor_state() {
        let anchor = make_anchor();
        let mut seq = LedgerSeq::with_defaults(anchor, 10);

        // Push one delta that changes epoch_fees.
        seq.push(make_delta(1, 1, 5_000_000));

        let pre_advance_tip = seq.tip_state();
        assert_eq!(pre_advance_tip.epoch_fees.0, 5_000_000);

        seq.advance_anchor();
        assert!(seq.is_empty());

        // Anchor itself should now reflect the fee change.
        assert_eq!(seq.anchor.epoch_fees.0, 5_000_000);
    }

    #[test]
    fn test_advance_anchor_reindexes_checkpoints() {
        let anchor = make_anchor();
        // checkpoint_interval = 2 → checkpoint at index 1, 3.
        let mut seq = LedgerSeq::new(anchor, 20, 2);

        for i in 1u8..=4 {
            seq.push(make_delta(i as u64, i, 0));
        }
        // Checkpoints at indices 1 and 3.
        assert!(seq.checkpoints.contains_key(&1));
        assert!(seq.checkpoints.contains_key(&3));

        // Advance anchor once — oldest delta is consumed, indices shift by 1.
        seq.advance_anchor();
        assert_eq!(seq.len(), 3);

        // Checkpoint that was at index 1 is now at index 0;
        // checkpoint that was at index 3 is now at index 2.
        assert!(seq.checkpoints.contains_key(&0));
        assert!(seq.checkpoints.contains_key(&2));
        // Old indices gone.
        assert!(!seq.checkpoints.contains_key(&1));
        assert!(!seq.checkpoints.contains_key(&3));
    }

    // ── max_rollback boundary ─────────────────────────────────────────────────

    #[test]
    fn test_max_rollback_boundary() {
        let anchor = make_anchor();
        let k = 5u64;
        let mut seq = LedgerSeq::with_defaults(anchor, k);

        // Fill to exactly k.
        for i in 1u8..=(k as u8) {
            seq.push(make_delta(i as u64, i, 0));
        }
        assert_eq!(seq.max_rollback(), k as usize);

        // Push one more — anchor advances, len stays at k.
        seq.push(make_delta(k + 1, (k + 1) as u8, 0));
        assert_eq!(seq.max_rollback(), k as usize);
    }

    // ── State reconstruction ──────────────────────────────────────────────────

    #[test]
    fn test_tip_state_matches_sequential_application() {
        let anchor = make_anchor();
        let mut seq = LedgerSeq::with_defaults(anchor.clone(), 100);

        // Push 5 deltas, each adding 1_000_000 to epoch_fees.
        // After 5 deltas, running epoch_fees in BlockFieldsDelta = 5_000_000.
        let mut running_fees = 0u64;
        for i in 1u8..=5 {
            running_fees += 1_000_000;
            let mut delta = make_delta(i as u64, i, 1_000_000);
            delta.block_fields.epoch_fees = Lovelace(running_fees);
            seq.push(delta);
        }

        let tip = seq.tip_state();
        assert_eq!(tip.epoch_fees.0, 5_000_000);
    }

    #[test]
    fn test_state_at_returns_correct_intermediate_state() {
        let anchor = make_anchor();
        let mut seq = LedgerSeq::with_defaults(anchor, 100);

        let mut running = 0u64;
        for i in 1u8..=5 {
            running += 1_000_000;
            let mut delta = make_delta(i as u64, i, 1_000_000);
            delta.block_fields.epoch_fees = Lovelace(running);
            seq.push(delta);
        }

        // State at slot=3 (hash=[3;32]) should have epoch_fees = 3_000_000.
        let state = seq
            .state_at(SlotNo(3), &make_hash(3))
            .expect("slot 3 should be in window");
        assert_eq!(state.epoch_fees.0, 3_000_000);
    }

    #[test]
    fn test_state_at_returns_none_for_unknown_point() {
        let anchor = make_anchor();
        let mut seq = LedgerSeq::with_defaults(anchor, 100);
        seq.push(make_delta(1, 1, 0));

        let result = seq.state_at(SlotNo(99), &make_hash(99));
        assert!(result.is_none());
    }

    // ── Checkpoint creation ───────────────────────────────────────────────────

    #[test]
    fn test_checkpoint_created_at_correct_interval() {
        let anchor = make_anchor();
        let mut seq = LedgerSeq::new(anchor, 200, 5);

        // No checkpoints before we hit the interval.
        for i in 1u8..=4 {
            seq.push(make_delta(i as u64, i, 0));
        }
        assert!(seq.checkpoints.is_empty());

        // 5th push → checkpoint at index 4.
        seq.push(make_delta(5, 5, 0));
        assert_eq!(seq.checkpoints.len(), 1);
        assert!(seq.checkpoints.contains_key(&4));
    }

    #[test]
    fn test_checkpoint_reconstruction_consistent_with_sequential() {
        let anchor = make_anchor();
        let mut seq = LedgerSeq::new(anchor, 200, 3);

        // Push 9 deltas; checkpoints at index 2, 5, 8.
        let mut running = 0u64;
        for i in 1u8..=9 {
            running += 1_000_000;
            let mut delta = make_delta(i as u64, i, 1_000_000);
            delta.block_fields.epoch_fees = Lovelace(running);
            seq.push(delta);
        }

        // Verify checkpoint at index 2 has epoch_fees = 3_000_000.
        let cp_state = seq.checkpoints.get(&2).expect("checkpoint at 2");
        assert_eq!(cp_state.epoch_fees.0, 3_000_000);

        // Verify checkpoint at index 5 has epoch_fees = 6_000_000.
        let cp_state = seq.checkpoints.get(&5).expect("checkpoint at 5");
        assert_eq!(cp_state.epoch_fees.0, 6_000_000);

        // Verify tip (index 8) has epoch_fees = 9_000_000.
        let tip = seq.tip_state();
        assert_eq!(tip.epoch_fees.0, 9_000_000);
    }

    // ── Push / rollback cycle ─────────────────────────────────────────────────

    #[test]
    fn test_push_rollback_reapply_cycle() {
        let anchor = make_anchor();
        let mut seq = LedgerSeq::with_defaults(anchor, 20);

        // Push 5 deltas.
        let mut running = 0u64;
        for i in 1u8..=5 {
            running += 1_000_000;
            let mut delta = make_delta(i as u64, i, 1_000_000);
            delta.block_fields.epoch_fees = Lovelace(running);
            seq.push(delta);
        }
        assert_eq!(seq.tip_state().epoch_fees.0, 5_000_000);

        // Roll back 2.
        seq.rollback(2);
        assert_eq!(seq.len(), 3);
        assert_eq!(seq.tip_state().epoch_fees.0, 3_000_000);

        // Reapply 3 different deltas (fork scenario).
        let mut running = 3_000_000u64;
        for i in 10u8..=12 {
            running += 500_000;
            let mut delta = make_delta(i as u64, i, 500_000);
            delta.block_fields.epoch_fees = Lovelace(running);
            seq.push(delta);
        }
        assert_eq!(seq.len(), 6);
        assert_eq!(seq.tip_state().epoch_fees.0, 4_500_000);
    }

    // ── Reset anchor ──────────────────────────────────────────────────────────

    #[test]
    fn test_reset_anchor_clears_volatile() {
        let anchor = make_anchor();
        let mut seq = LedgerSeq::with_defaults(anchor, 10);
        for i in 1u8..=5 {
            seq.push(make_delta(i as u64, i, 0));
        }
        assert!(!seq.is_empty());

        let new_anchor = make_anchor();
        seq.reset_anchor(new_anchor);
        assert!(seq.is_empty());
        assert!(seq.checkpoints.is_empty());
    }
}
