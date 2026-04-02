# Haskell-Compatible ChainDB + LedgerDB Architecture

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace Dugite's ad-hoc fork handling with a proper Haskell-compatible storage architecture: VolatileDB for recent blocks (all forks), ImmutableDB for confirmed blocks, LedgerDB with last-k ledger states for O(1) rollback, chain selection via anchored fragment comparison, and background copy-to-immutable + GC threads.

**Architecture:** Follows ouroboros-consensus ChainDB exactly: blocks enter a ChainSelQueue and are processed sequentially by a single background thread (addBlockRunner). Each block is written to VolatileDB first, then chain selection runs to determine if the current chain should switch. The LedgerDB maintains an AnchoredSeq of the last k+1 ledger states so rollback is always O(k) with no snapshot loading or genesis replay. The immutable tip advances when a block is k-deep on the selected chain.

**Tech Stack:** Rust, tokio, bincode/serde, dugite-storage, dugite-ledger, dugite-consensus

**Sources (corroborated):**
- **Haskell Source**: ouroboros-consensus `ChainDB/Impl/ChainSel.hs`, `LedgerDB/V2/LedgerSeq.hs`, `Background.hs`, `NodeKernel.hs`
- **Technical Report**: "The Cardano Consensus and Storage Layer" (de Vries, Winant, Coutts) — TR §chaindb, §chainsel, §storage:components
- **Cardano Blueprint**: `src/storage/README.md`, `src/consensus/chainsel.md`
- **Dugite Audit**: Architect agent audit 2026-03-23 (5 identified problems)

---

## Scope

This plan covers **5 independent but related subsystems**. Each produces working, testable software:

1. **LedgerSeq** — Ring buffer of last-k ledger states (replaces DiffSeq for rollback)
2. **Chain Fragment** — Anchored fragment of last-k block headers (replaces `selected_chain`)
3. **Chain Selection** — Ouroboros Praos chain selection with VRF tiebreaking
4. **ChainSelQueue + addBlockRunner** — Sequential block processing thread
5. **Background Threads** — Copy-to-immutable, GC, snapshot scheduling

---

## Pre-Implementation: Key Invariants (from TR + Haskell source)

These MUST be maintained at all times:

1. **Current Chain Invariant**: The current chain is the best valid path through the VolatileDB, anchored at the immutable tip.
2. **Immutable Tip = k-deep**: A block moves to ImmutableDB only when it has k blocks after it on the selected chain.
3. **LedgerDB matches chain fragment**: The LedgerDB's tip state corresponds to the tip of the current chain fragment.
4. **VolatileDB is a tree**: All blocks in VolatileDB form a tree rooted at the immutable tip. Blocks not reachable from the immutable tip are garbage collected.
5. **Rollback ≤ k**: The maximum rollback depth is k blocks. If a candidate chain requires rolling back more than k blocks, it is rejected.
6. **Blocks enter VolatileDB only**: No block goes directly to ImmutableDB. Only the copy-to-immutable background thread moves blocks.
7. **Chain selection is symmetric**: Locally forged blocks follow the same addBlock path as peer blocks. No preferential treatment.

---

## Subsystem 1: LedgerSeq (Ring Buffer of Ledger States)

**Purpose:** Replace the current single-snapshot + DiffSeq approach with Haskell's LedgerSeq — an `AnchoredSeq` of up to k+1 complete ledger states. This enables O(1) rollback to any point within the last k blocks, covering ALL ledger state (not just UTxO diffs).

**Haskell reference:** `LedgerDB/V2/LedgerSeq.hs` — `LedgerSeq`, `prune`, `rollbackN`, `extend`

### File Structure

- Create: `crates/dugite-ledger/src/ledger_seq.rs`
- Modify: `crates/dugite-ledger/src/state/mod.rs` (add LedgerSeq integration)
- Modify: `crates/dugite-ledger/src/lib.rs` (pub mod ledger_seq)
- Test: `crates/dugite-ledger/tests/ledger_seq_tests.rs`

### Data Structure

The Haskell V2 LedgerDB uses a DiffTables approach: one full anchor state plus
per-block deltas. We follow the same pattern because full LedgerState clones
are ~40-80MB each (epoch snapshots alone are ~60MB with 3x StakeSnapshot),
making k full copies infeasible (k=432 → ~17-34GB on preview, ~86-173GB on mainnet).

**Architecture (matching Haskell V2 DiffTables):**
- **Anchor**: One full `LedgerState` at the immutable tip. Saved to disk.
- **Volatile deltas**: Per-block `LedgerDelta` recording ALL state changes
  (not just UTxO — also delegations, rewards, pool params, governance, epoch info).
- **Reconstruction**: To get the state at block N, apply deltas 1..N to the anchor.
- **Checkpoint optimization**: Store full snapshots at every 100 blocks within the
  volatile window, so reconstruction is bounded to 100 block re-applications max.

```rust
/// LedgerSeq: anchored sequence of ledger state deltas.
/// Matches Haskell's LedgerDB V2 DiffTables approach.
///
/// Memory model:
/// - 1 full anchor state (~80MB)
/// - k per-block deltas (~5-50KB each = ~2-20MB total for k=432)
/// - Checkpoints every 100 blocks (~4 checkpoints for k=432 = ~320MB)
/// Total: ~400MB on preview, ~500MB on mainnet — feasible.
pub struct LedgerSeq {
    /// Full ledger state at the immutable tip (the anchor).
    /// This is the only state saved to disk.
    anchor: Box<LedgerState>,
    anchor_point: Point,
    /// Per-block deltas for the volatile window, oldest first.
    /// Each delta records all changes made by applying one block.
    deltas: VecDeque<LedgerDelta>,
    /// Checkpointed full states at every N blocks (default 100).
    /// Key is the index into `deltas`. Enables O(100) reconstruction.
    checkpoints: BTreeMap<usize, Box<LedgerState>>,
    /// Interval between checkpoints (number of blocks).
    checkpoint_interval: usize,
    /// Security parameter k — max deltas length.
    k: u64,
}

/// All state changes from applying a single block.
/// This must capture EVERY field change in LedgerState, not just UTxO.
pub struct LedgerDelta {
    slot: SlotNo,
    hash: Hash32,
    block_no: BlockNo,
    /// UTxO changes (inserts and deletes)
    utxo_diff: UtxoDiff,
    /// Delegation changes (new delegations, removed delegations)
    delegation_changes: Vec<DelegationChange>,
    /// Pool parameter changes (registrations, retirements)
    pool_changes: Vec<PoolChange>,
    /// Reward account changes (new accounts, balance changes, removals)
    reward_changes: Vec<RewardChange>,
    /// Governance state changes (proposals, votes, DRep changes)
    governance_changes: Vec<GovernanceChange>,
    /// Epoch transition data (if this block crossed an epoch boundary)
    epoch_transition: Option<EpochTransitionDelta>,
    /// Protocol parameter changes
    pp_update: Option<ProtocolParamsDelta>,
}
```

### Key Operations

```rust
impl LedgerSeq {
    /// Get the current tip state by reconstructing from nearest checkpoint.
    /// O(checkpoint_interval) — at most 100 block re-applications.
    pub fn tip_state(&self) -> LedgerState;

    /// Roll back n blocks. Truncates deltas and invalidates checkpoints.
    /// O(1) — just truncate the VecDeque.
    pub fn rollback(&mut self, n: usize);

    /// Extend with a new block's delta.
    /// If deltas exceed k, advance_anchor is called automatically.
    pub fn push(&mut self, delta: LedgerDelta);

    /// Maximum rollback depth (= deltas.len())
    pub fn max_rollback(&self) -> usize;

    /// Advance the anchor: apply oldest delta to anchor, pop it.
    /// Called when the immutable tip advances.
    fn advance_anchor(&mut self);

    /// Get the state at a specific point by applying deltas from
    /// the nearest checkpoint. Used for queries and fork validation.
    pub fn state_at(&self, slot: u64, hash: &Hash32) -> Option<LedgerState>;

    /// Save anchor state to disk (for restart recovery).
    pub fn save_anchor_snapshot(&self, path: &Path) -> Result<()>;

    /// Restore from disk: load anchor + replay volatile blocks from WAL.
    /// On startup: (1) load anchor from latest snapshot,
    /// (2) replay volatile blocks from VolatileDB WAL to rebuild deltas.
    /// This replay is bounded by k blocks — NEVER genesis replay.
    pub fn restore(path: &Path, volatile_blocks: &[Block]) -> Result<Self>;
}
```

### Tasks

- [ ] Task 1.1: Define `LedgerSeq` struct and basic operations (push, rollback, tip)
- [ ] Task 1.2: Implement `advance_anchor` for immutable tip advancement
- [ ] Task 1.3: Implement `state_at` for point queries
- [ ] Task 1.4: Extract `NonUtxoLedgerState` from `LedgerState` (delegations, pools, rewards, governance)
- [ ] Task 1.5: Implement memory-efficient state sharing with Arc
- [ ] Task 1.6: Write comprehensive tests (rollback, push beyond k, advance_anchor)
- [ ] Task 1.7: Integrate LedgerSeq into Node — replace direct LedgerState usage

---

## Subsystem 2: Chain Fragment (Anchored Fragment)

**Purpose:** Replace the current `selected_chain: Vec<Hash32>` with Haskell's `AnchoredFragment` — a sequence of block headers anchored at the immutable tip. This is the authoritative representation of the current chain.

**Haskell reference:** `Fragment/Validated.hs`, `AnchoredFragment` from `ouroboros-network`

### File Structure

- Create: `crates/dugite-consensus/src/chain_fragment.rs` (pure data structure, no I/O — lives in consensus to avoid circular deps)
- Modify: `crates/dugite-consensus/src/lib.rs` (pub mod chain_fragment)
- Modify: `crates/dugite-storage/src/volatile_db.rs` (replace selected_chain)
- Test: `crates/dugite-consensus/tests/chain_fragment_tests.rs`

### Data Structure

```rust
/// Anchored fragment of block headers, matching Haskell's AnchoredFragment.
/// The anchor is the immutable tip point. The fragment contains headers
/// for the last k blocks on the selected chain.
pub struct ChainFragment {
    /// Anchor point (immutable tip)
    anchor: Point,
    /// Block headers in chronological order (oldest first)
    headers: VecDeque<BlockHeader>,
}
```

### Key Operations

```rust
impl ChainFragment {
    /// Tip of the fragment (last header, or anchor if empty)
    pub fn tip(&self) -> Point;
    /// Length (number of headers)
    pub fn length(&self) -> usize;
    /// Block number at tip
    pub fn tip_block_no(&self) -> BlockNo;
    /// Roll back to a point within the fragment
    pub fn rollback_to(&mut self, point: &Point) -> bool;
    /// Extend with a new header
    pub fn push(&mut self, header: BlockHeader);
    /// Find intersection with a list of points
    pub fn find_intersect(&self, points: &[Point]) -> Option<Point>;
    /// Compare preference with another fragment (chain selection)
    pub fn prefer_over(&self, other: &ChainFragment) -> bool;
    /// Get successor candidates from VolatileDB
    pub fn candidates_from_volatile(&self, volatile: &VolatileDB) -> Vec<ChainFragment>;
}
```

### Tasks

- [ ] Task 2.1: Define `ChainFragment` struct and basic operations
- [ ] Task 2.2: Implement `rollback_to` and `push`
- [ ] Task 2.3: Implement `find_intersect` for ChainSync server
- [ ] Task 2.4: Implement `prefer_over` using Praos chain selection rules
- [ ] Task 2.5: Implement `candidates_from_volatile` using successor index
- [ ] Task 2.6: Write tests (intersect, rollback, preference)
- [ ] Task 2.7: Integrate into VolatileDB — replace `selected_chain`

---

## Subsystem 3: Chain Selection

**Purpose:** Implement the Ouroboros Praos chain selection rule. When a new block arrives, determine whether the current chain should switch to a candidate chain containing the new block.

**Haskell reference:** `ChainSel.hs:chainSelectionForBlock`, `Praos/Common.hs:comparePraos`

### File Structure

- Create: `crates/dugite-consensus/src/chain_selection.rs`
- Modify: `crates/dugite-consensus/src/lib.rs`
- Test: `crates/dugite-consensus/tests/chain_selection_tests.rs`

### Algorithm (from TR §chainsel:addblock)

When block B arrives:
1. Write B to VolatileDB
2. Compute candidate chains: all maximal paths from immutable tip through VolatileDB that include B
3. For each candidate, compare with current chain using `prefer`:
   - Longer chain wins
   - Equal length: VRF tiebreaker (lower VRF value wins, within maxDist slots)
   - Equal length + same issuer: higher opcert counter wins
4. If a candidate is preferred AND validates successfully (ledger apply succeeds):
   - Switch to that candidate (rollback current chain to intersection, apply new blocks)
   - Update LedgerSeq accordingly
5. If no candidate is preferred: block stays in VolatileDB, no chain switch

### Tasks

- [ ] Task 3.1: Implement `chain_preference` function (longer chain, VRF tiebreak)
- [ ] Task 3.2: Implement `maximal_candidates` (find all candidate chains through VolatileDB)
- [ ] Task 3.3: Implement `validate_candidate` (apply candidate blocks to forked LedgerSeq)
- [ ] Task 3.4: Implement `switch_to` (rollback current chain, apply new chain)
- [ ] Task 3.5: Write tests with multi-fork scenarios
- [ ] Task 3.6: Integrate into addBlockRunner (Subsystem 4)

---

## Subsystem 4: ChainSelQueue + addBlockRunner

**Purpose:** Replace the current direct-apply approach with Haskell's sequential block processing. All blocks (peer and forged) enter a single queue, processed by one background thread.

**Haskell reference:** `ChainSel.hs:addBlockAsync`, `Background.hs:addBlockRunner`

### File Structure

- Create: `crates/dugite-storage/src/chain_sel_queue.rs`
- Modify: `crates/dugite-node/src/node/mod.rs` (new block flow)
- Modify: `crates/dugite-node/src/node/sync.rs` (submit to queue instead of direct apply)
- Modify: `crates/dugite-node/src/forge.rs` (submit to queue instead of direct apply)
- Test: `crates/dugite-storage/tests/chain_sel_queue_tests.rs`

### Architecture

```rust
/// Message to the chain selection thread
pub enum ChainSelMessage {
    /// Add a block (from peer or forge)
    AddBlock {
        hash: Hash32,
        slot: SlotNo,
        block_no: BlockNo,
        prev_hash: Hash32,
        cbor: Vec<u8>,
        /// Callback for the result
        result_tx: oneshot::Sender<AddBlockResult>,
    },
}

pub enum AddBlockResult {
    /// Block was adopted as the new chain tip
    AdoptedAsTip,
    /// Block was stored but a different chain was preferred
    StoredNotAdopted,
    /// Block was invalid
    Invalid(String),
    /// Block was already known
    AlreadyKnown,
}
```

### addBlockRunner Loop

```rust
async fn add_block_runner(
    mut rx: mpsc::Receiver<ChainSelMessage>,
    chain_db: Arc<RwLock<ChainDB>>,
    ledger_seq: Arc<RwLock<LedgerSeq>>,
    chain_fragment: Arc<RwLock<ChainFragment>>,
) {
    while let Some(msg) = rx.recv().await {
        match msg {
            ChainSelMessage::AddBlock { hash, slot, block_no, prev_hash, cbor, result_tx } => {
                // 1. Check: already in VolatileDB or ImmutableDB?
                // 2. Check: in invalid block cache?
                // 3. Write to VolatileDB
                // 4. Run chain selection for this block
                // 5. If adopted: update chain fragment + LedgerSeq
                // 6. Send result
                let result = process_add_block(...).await;
                let _ = result_tx.send(result);
            }
        }
    }
}
```

### Tasks

- [ ] Task 4.1: Define ChainSelQueue types and message protocol
- [ ] Task 4.2: Implement addBlockRunner loop with VolatileDB write
- [ ] Task 4.3: Wire chain selection (Subsystem 3) into addBlockRunner
- [ ] Task 4.4: Implement invalid block cache
- [ ] Task 4.5: Modify sync pipeline to submit blocks to queue
- [ ] Task 4.6: Modify forge pipeline to submit blocks to queue
- [ ] Task 4.7: Check forge adoption result (TraceDidntAdoptBlock)
- [ ] Task 4.8: Write integration tests

---

## Subsystem 5: Background Threads

**Purpose:** Implement the three background operations that maintain storage health: copy-to-immutable, garbage collection, and ledger snapshot scheduling.

**Haskell reference:** `Background.hs:copyToImmutableDB`, `garbageCollectBlocks`, `GcSchedule`

### Copy to Immutable

When the chain fragment grows beyond k headers:
1. Pop the oldest header from the fragment
2. Copy its block from VolatileDB to ImmutableDB
3. Advance the LedgerSeq anchor
4. Schedule the VolatileDB entry for GC (60s delay)

### Garbage Collection

After the 60s GC delay:
1. Remove the block from VolatileDB
2. Remove any other blocks with slot ≤ the copied block's slot (fork blocks at same height)

### Snapshot Scheduling

Save LedgerSeq anchor state (the immutable tip ledger state) to disk periodically:
- Every epoch boundary
- Every N blocks (configurable, default 2000)
- On graceful shutdown

### Tasks

- [ ] Task 5.1: Implement copy-to-immutable (fragment pop → ImmutableDB append → anchor advance)
- [ ] Task 5.2: Implement GC scheduler with 60s delay
- [ ] Task 5.3: Implement snapshot scheduling (anchor state to disk)
- [ ] Task 5.4: Wire background threads into node startup
- [ ] Task 5.5: Implement graceful shutdown (save snapshot, NO flush-all-volatile)
- [ ] Task 5.6: Write integration tests

---

## Migration Strategy

This is a major refactor. To avoid a "big bang" rewrite:

1. **Phase 1 (Subsystems 1-2):** Implement LedgerSeq and ChainFragment as NEW code alongside existing structures. Run both in parallel, comparing results.
2. **Phase 2 (Subsystem 3):** Add chain selection. Initially as advisory (log preferred chain, don't switch).
3. **Phase 3 (Subsystem 4):** Switch block flow to ChainSelQueue. This is the cutover point — disable old direct-apply path.
4. **Phase 4 (Subsystem 5):** Add background threads. Remove old flush_to_immutable calls.
5. **Phase 5:** Remove old code paths (reset_ledger_and_replay, do_fork_recovery genesis replay, etc.)

Each phase should be a separate commit/PR with its own tests passing.

---

## Subsystem 6: Startup Recovery + Mithril Bypass

**Purpose:** Define the startup sequence that rebuilds LedgerSeq from disk, and explicitly carve out the Mithril bulk-import path as the one exception to the "blocks enter VolatileDB only" rule.

### Startup Recovery (matching Haskell `openDBInternal`)

1. Open ImmutableDB; determine immutable tip `I`
2. Load LedgerSeq anchor snapshot from disk (latest snapshot at or before `I`)
3. If snapshot is behind `I`: replay ImmutableDB blocks from snapshot point to `I` (bounded, fast)
4. Open VolatileDB; reconstruct in-memory indices from WAL
5. Compute initial chain fragment: find the best valid path through VolatileDB anchored at `I`
6. Replay volatile blocks through LedgerSeq to rebuild deltas (at most k blocks)
7. Node is ready — no genesis replay ever needed

### Mithril Bypass

Mithril import writes millions of already-finalized blocks directly to ImmutableDB. This is the ONE exception to Invariant 6 ("blocks enter VolatileDB only"). The bypass is explicitly documented and justified:
- Mithril blocks are digest-verified and come from a trusted aggregator
- They are already-immutable (far beyond k depth)
- Routing through VolatileDB + chain selection would be catastrophically slow
- After import, the node rebuilds LedgerSeq from the ImmutableDB tip via the normal startup recovery path

### Chain Selection Header Validation

Before running full chain selection for a new block, perform tentative header validation (matching Haskell's `chainSelectionForBlock` prechecks):
1. Slot number > immutable tip slot (block is not immutable-age)
2. Block is not already in VolatileDB (dedup)
3. Block is not in the invalid block cache
4. Block's prev_hash exists in VolatileDB or is the immutable tip hash

Only after these checks pass does the block enter VolatileDB and trigger chain selection.

### Tasks

- [ ] Task 6.1: Implement startup recovery sequence (load anchor, replay to immutable tip)
- [ ] Task 6.2: Implement volatile block replay (WAL → LedgerSeq deltas)
- [ ] Task 6.3: Document Mithril bypass as explicit exception
- [ ] Task 6.4: Implement tentative header validation prechecks
- [ ] Task 6.5: Add invalid block cache
- [ ] Task 6.6: Write startup recovery integration tests

---

## Success Criteria

1. **No genesis replay ever** — rollback is always O(k), bounded by LedgerSeq
2. **No ImmutableDB contamination** — only copy-to-immutable writes to ImmutableDB
3. **Correct fork handling** — chain selection picks the Praos-preferred chain
4. **Forge adoption check** — forged blocks that aren't adopted are logged, not announced
5. **Graceful restart** — no replay needed if snapshot is within k blocks of immutable tip
6. **All existing tests pass** — no regression
7. **Verified with Haskell node** — blocks served via ChainSync accepted by Haskell node
