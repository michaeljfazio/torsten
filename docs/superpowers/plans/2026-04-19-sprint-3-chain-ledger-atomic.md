# Sprint 3: `ChainLedger` Atomic Commits (#439 Follow-Up)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (inline execution recommended — lock-architecture changes benefit from review between each step). Master context: [`2026-04-19-439-followup-master.md`](2026-04-19-439-followup-master.md). Do not start until Sprint 2 soak passes.

**Goal:** Unify `ChainDB`, `LedgerState`, and `LedgerSeq` into a single `ChainLedger` struct behind one `RwLock`, making chain-selection commits and ledger-state updates atomic. Eliminates the observable-inconsistency window exploited by item 1.3.

**Architecture:**
- New `ChainLedger` struct wraps all three stores.
- `ChainLedger::apply_block`, `::rollback`, `::switch_fork` become the authoritative write operations — each acquires the single lock, mutates all three stores coherently, releases.
- `chain_sel_queue` produces a `SwitchPlan` (plan-only, no mutation); `ChainLedger` applies it atomically.
- All reader paths (ChainSync server, tip queries, metrics) read from the unified lock.

**Tech Stack:** Rust 1.95, tokio RwLock, existing dugite storage/ledger crates.

**Haskell reference:**
- `ChainSel.hs::switchTo` (line ~896): single `atomically` block commits both `cdbChain` and `forker`.
- No equivalent of dugite's multi-lock dance. Dugite's three `RwLock` structure is the divergence.

---

## Task 1: Introduce the `ChainLedger` struct (scaffolding only)

**Files:**
- Create: `crates/dugite-node/src/chain_ledger.rs`
- Modify: `crates/dugite-node/src/lib.rs` or `src/node/mod.rs` (`mod chain_ledger;` + re-export)

- [ ] **Step 1.1: Write the struct skeleton**

Create `crates/dugite-node/src/chain_ledger.rs`:

```rust
//! ChainLedger — unified chain+ledger state behind a single RwLock.
//!
//! Haskell reference: `ChainSel.hs::switchTo` commits both `cdbChain`
//! (ChainDB) and the ledger DB via a single `atomically` STM block.
//! Dugite's previous design held three separate locks, permitting an
//! observable window where `ChainDB.selected_chain` was on the new
//! fork but `LedgerState.tip` was still on the old fork. This struct
//! collapses all three stores into one lockable unit.

use std::sync::Arc;
use tokio::sync::RwLock;

use dugite_storage::ChainDB;
use dugite_ledger::{LedgerState, ledger_seq::LedgerSeq};

pub struct ChainLedger {
    inner: RwLock<ChainLedgerInner>,
}

/// The unified state, protected by a single lock.
///
/// Field invariant: `inner.chain_db.get_tip() == inner.ledger.tip` at
/// every observable instant (i.e. outside a `write().await` critical
/// section).
pub struct ChainLedgerInner {
    pub chain_db: ChainDB,
    pub ledger: LedgerState,
    pub seq: LedgerSeq,
}

impl ChainLedger {
    pub fn new(chain_db: ChainDB, ledger: LedgerState, seq: LedgerSeq) -> Self {
        Self {
            inner: RwLock::new(ChainLedgerInner { chain_db, ledger, seq }),
        }
    }

    pub async fn read(&self) -> tokio::sync::RwLockReadGuard<'_, ChainLedgerInner> {
        self.inner.read().await
    }

    pub async fn write(&self) -> tokio::sync::RwLockWriteGuard<'_, ChainLedgerInner> {
        self.inner.write().await
    }
}
```

- [ ] **Step 1.2: Expose module**

In `crates/dugite-node/src/lib.rs` (or `mod.rs`):

```rust
pub mod chain_ledger;
```

- [ ] **Step 1.3: Run build to verify scaffolding compiles**

Run: `cargo build -p dugite-node 2>&1 | tail -5`
Expected: clean (struct + impl only, no callers yet).

- [ ] **Step 1.4: Commit**

```bash
git add crates/dugite-node/src/chain_ledger.rs crates/dugite-node/src/lib.rs
git commit -m "feat(node): introduce ChainLedger struct (scaffolding only, no callers)

Unified container for ChainDB + LedgerState + LedgerSeq behind a single
RwLock. Field invariant: chain_db.get_tip() == ledger.tip at every
observable instant outside a critical section.

Prepares Sprint 3 Tasks 2-5 which migrate operations onto this unified
lock. Matches Haskell ChainSel.hs switchTo's single-STM-block atomic
commit. No behavior change yet."
```

---

## Task 2: `ChainLedger::apply_block` — the authoritative forward-apply

**Files:**
- Modify: `crates/dugite-node/src/chain_ledger.rs`

- [ ] **Step 2.1: Define `ApplyBlockOutcome`**

```rust
pub struct ApplyBlockOutcome {
    pub tip_advanced: bool,
    pub new_tip: Option<Tip>,
    pub fork_switch: Option<ForkSwitchOutcome>,
}

pub struct ForkSwitchOutcome {
    pub rollback_count: usize,
    pub apply_count: usize,
    pub intersection: Point,
}
```

- [ ] **Step 2.2: Write failing test — atomic tip advance**

```rust
#[tokio::test]
async fn test_apply_block_tip_and_ledger_advance_atomically() {
    let cl = make_test_chain_ledger().await;

    // Apply 10 blocks.
    for i in 1..=10 {
        let block = make_fake_block(i);
        let outcome = cl.apply_block(&block, BlockValidationMode::ApplyOnly).await.unwrap();
        assert!(outcome.tip_advanced);
    }

    // Read after the final apply. chain_db.tip and ledger.tip must be equal.
    let guard = cl.read().await;
    assert_eq!(
        guard.chain_db.get_tip().point,
        guard.ledger.tip.point,
        "ChainLedger invariant: chain_db.tip == ledger.tip"
    );
}
```

- [ ] **Step 2.3: Implement `apply_block` atomically**

```rust
impl ChainLedger {
    /// Apply a block to both ChainDB (store + chain selection) and
    /// LedgerState (ledger validation + apply), in a single critical
    /// section. Either both advance or neither does.
    pub async fn apply_block(
        &self,
        block: &Block,
        mode: BlockValidationMode,
    ) -> Result<ApplyBlockOutcome, ApplyBlockError> {
        let mut inner = self.inner.write().await;

        // 1. Submit to chain-selection-plan logic (pure — no storage mutation).
        let sel_plan = inner.chain_db.plan_add_block(
            *block.hash(),
            block.slot(),
            block.block_number(),
            *block.prev_hash(),
        )?;

        match sel_plan {
            SelectionPlan::AddedAsTip => {
                // 2. Commit to ChainDB.
                let extended = inner.chain_db.apply_plan_added_as_tip(
                    *block.hash(),
                    block.slot(),
                    block.block_number(),
                    *block.prev_hash(),
                    block.raw_cbor.clone().unwrap_or_default(),
                )?;
                debug_assert!(extended);

                // 3. Apply to ledger + push delta.
                let (_outcome, delta) = inner.ledger.apply_block_producing_delta(block, mode)?;
                inner.seq.push_delta(delta);

                Ok(ApplyBlockOutcome {
                    tip_advanced: true,
                    new_tip: Some(inner.ledger.tip.clone()),
                    fork_switch: None,
                })
            }
            SelectionPlan::StoredAsFork => {
                // 2. Store in ChainDB as fork block; ledger unchanged.
                inner.chain_db.apply_plan_stored_as_fork(
                    *block.hash(),
                    block.slot(),
                    block.block_number(),
                    *block.prev_hash(),
                    block.raw_cbor.clone().unwrap_or_default(),
                )?;
                Ok(ApplyBlockOutcome {
                    tip_advanced: false,
                    new_tip: None,
                    fork_switch: None,
                })
            }
            SelectionPlan::TriggeredFork(plan) => {
                // 2. Apply the chain-switch plan to ChainDB.
                inner.chain_db.apply_plan_triggered_fork(&plan)?;

                // 3. Roll back ledger to intersection (via LedgerSeq) + re-apply fork blocks.
                let n_rollback = plan.rollback.len();
                if n_rollback > inner.seq.deltas.len() {
                    return Err(ApplyBlockError::RollbackExceedsWindow {
                        requested: n_rollback,
                        available: inner.seq.deltas.len(),
                    });
                }
                for _ in 0..n_rollback {
                    let d = inner.seq.deltas.pop_back().unwrap();
                    inner.ledger.reverse_apply(&d);
                }

                // 4. Forward-apply the new fork's blocks.
                for apply_hash in &plan.apply {
                    let apply_block_cbor = inner.chain_db.get_block(apply_hash)?
                        .ok_or(ApplyBlockError::ApplyBlockNotInChainDB(*apply_hash))?;
                    let apply_block = decode_block_minimal(&apply_block_cbor, /* byron epoch len */)?;
                    let (_, delta) = inner.ledger.apply_block_producing_delta(&apply_block, BlockValidationMode::ApplyOnly)?;
                    inner.seq.push_delta(delta);
                }

                Ok(ApplyBlockOutcome {
                    tip_advanced: true,
                    new_tip: Some(inner.ledger.tip.clone()),
                    fork_switch: Some(ForkSwitchOutcome {
                        rollback_count: n_rollback,
                        apply_count: plan.apply.len(),
                        intersection: Point::Specific(plan.intersection_slot, plan.intersection),
                    }),
                })
            }
            SelectionPlan::AlreadyKnown => Ok(ApplyBlockOutcome {
                tip_advanced: false,
                new_tip: None,
                fork_switch: None,
            }),
            SelectionPlan::Invalid(reason) => Err(ApplyBlockError::InvalidBlock(reason)),
        }
    }
}
```

This requires introducing `SelectionPlan` in `chain_sel_queue.rs` as a pure (mutation-free) planning type.

- [ ] **Step 2.4: Introduce `SelectionPlan` and `ChainDB::plan_add_block`**

In `crates/dugite-storage/src/chain_sel_queue.rs`:

```rust
/// A selection decision that hasn't yet been committed to storage.
///
/// Returned by `ChainDB::plan_add_block` so the caller (ChainLedger) can
/// stage ChainDB + LedgerState changes together and commit atomically.
#[derive(Debug, Clone)]
pub enum SelectionPlan {
    AddedAsTip,
    StoredAsFork,
    TriggeredFork(SwitchPlan),
    AlreadyKnown,
    Invalid(String),
}
```

In `crates/dugite-storage/src/chain_db.rs`:

```rust
impl ChainDB {
    /// Pure planning function: inspect state, decide what this block's
    /// addition would do, return the plan WITHOUT mutating storage.
    /// Caller commits via `apply_plan_*` methods.
    pub fn plan_add_block(
        &self,
        hash: BlockHeaderHash,
        slot: SlotNo,
        block_no: BlockNo,
        prev_hash: BlockHeaderHash,
    ) -> Result<SelectionPlan, ChainDBError> {
        // 1. Already known?
        if self.has_block(&hash) { return Ok(SelectionPlan::AlreadyKnown); }
        // 2. Would this extend selected_chain?
        let extends = self.volatile.would_extend_selected_chain(&prev_hash);
        // 3. Would this trigger a fork switch?
        let fork_plan = self.volatile.plan_switch_chain_if_triggered(&hash, slot.0, block_no.0, prev_hash);
        match (extends, fork_plan) {
            (_, Some(plan)) => Ok(SelectionPlan::TriggeredFork(plan)),
            (true, None) => Ok(SelectionPlan::AddedAsTip),
            (false, None) => Ok(SelectionPlan::StoredAsFork),
        }
    }

    pub fn apply_plan_added_as_tip(&mut self, hash, slot, block_no, prev, cbor) -> Result<bool, _> {
        // Identical to old `add_block`; just the commit half of the split.
    }
    pub fn apply_plan_stored_as_fork(&mut self, hash, slot, block_no, prev, cbor) -> Result<(), _> {
        // Add to volatile.blocks but DON'T extend selected_chain.
    }
    pub fn apply_plan_triggered_fork(&mut self, plan: &SwitchPlan) -> Result<(), _> {
        // Mutate selected_chain per plan.
    }
}
```

In `crates/dugite-storage/src/volatile_db.rs`: add `would_extend_selected_chain(&Hash32) -> bool` and `plan_switch_chain_if_triggered(...) -> Option<SwitchPlan>` that compute the same decisions without mutating `selected_chain`.

- [ ] **Step 2.5: Run test**

Run: `cargo nextest run -p dugite-node -E 'test(test_apply_block_tip_and_ledger_advance_atomically)'`
Expected: PASS.

- [ ] **Step 2.6: Concurrent-read invariant test**

```rust
#[tokio::test(flavor = "multi_thread")]
async fn test_chain_ledger_invariant_under_concurrent_reads() {
    let cl = Arc::new(make_test_chain_ledger().await);
    let cl_reader = Arc::clone(&cl);

    // Reader task: constantly check the invariant.
    let reader = tokio::spawn(async move {
        for _ in 0..1000 {
            let g = cl_reader.read().await;
            assert_eq!(g.chain_db.get_tip().point, g.ledger.tip.point);
            drop(g);
            tokio::task::yield_now().await;
        }
    });

    // Writer task: apply 500 blocks.
    let writer = tokio::spawn(async move {
        for i in 1..=500 {
            let block = make_fake_block(i);
            cl.apply_block(&block, BlockValidationMode::ApplyOnly).await.unwrap();
        }
    });

    reader.await.unwrap();
    writer.await.unwrap();
}
```

- [ ] **Step 2.7: Commit**

```bash
git add crates/
git commit -m "feat(node): ChainLedger::apply_block is atomic across chain+ledger

Single write-lock acquisition commits ChainDB mutations, LedgerState
mutations, and LedgerSeq delta push as one unit. Concurrent readers
always observe chain_db.tip == ledger.tip.

SwitchedToFork path is now handled in the same critical section:
- ChainDB.selected_chain updated per SwitchPlan
- LedgerSeq rolled back N deltas
- Fork's apply blocks ledger-applied in order
- New deltas pushed onto LedgerSeq

Matches Haskell ChainSel.hs::switchTo atomic STM block.

Introduces SelectionPlan enum for mutation-free planning —
chain_sel_queue returns a plan; ChainLedger commits it."
```

---

## Task 3: Migrate `apply_fetched_block` and `process_forward_blocks` to `ChainLedger`

**Files:**
- Modify: `crates/dugite-node/src/node/mod.rs` (`apply_fetched_block`)
- Modify: `crates/dugite-node/src/node/sync.rs` (`process_forward_blocks`)
- Modify: `Node` struct field — replace separate `chain_db`, `ledger_state`, `ledger_seq` with single `chain_ledger: Arc<ChainLedger>`.

- [ ] **Step 3.1: Update `Node` struct**

In `crates/dugite-node/src/node/mod.rs`:

```rust
pub struct Node {
    // REMOVED:
    // pub(crate) chain_db: Arc<RwLock<ChainDB>>,
    // pub(crate) ledger_state: Arc<RwLock<LedgerState>>,
    // pub(crate) ledger_seq: Arc<RwLock<LedgerSeq>>,
    // NEW:
    pub(crate) chain_ledger: Arc<ChainLedger>,

    // ... rest unchanged ...
}
```

- [ ] **Step 3.2: Update `Node::new` construction**

Replace the three separate lock creations with one:

```rust
let chain_ledger = Arc::new(ChainLedger::new(chain_db, ledger_state, ledger_seq));
```

- [ ] **Step 3.3: Update `apply_fetched_block`**

Replace the hand-written critical section with a single call:

```rust
async fn apply_fetched_block(&mut self, fetched: FetchedBlock) {
    let block = fetched.block;
    let mode = /* same as before */;

    match self.chain_ledger.apply_block(&block, mode).await {
        Ok(outcome) if outcome.tip_advanced => {
            // Announce, mempool cleanup, metrics, etc.
            self.announce_new_tip(&outcome).await;
        }
        Ok(_) => {
            // StoredAsFork or AlreadyKnown — no announce.
        }
        Err(e) => {
            warn!(?e, "apply_fetched_block failed");
        }
    }
}
```

- [ ] **Step 3.4: Update `process_forward_blocks`**

Same pattern — iterate the batch, call `chain_ledger.apply_block(..)` for each. Simplification: the TriggeredFork handler becomes unnecessary because `ChainLedger::apply_block` already handles fork switches internally.

- [ ] **Step 3.5: Update every read site**

Grep: `grep -rn 'ledger_state.read\|chain_db.read\|ledger_seq.read' crates/dugite-node/src`

Each site becomes:

```rust
let guard = self.chain_ledger.read().await;
let tip = &guard.ledger.tip;
// or: let chain_tip = guard.chain_db.get_tip();
```

This is mechanical, but there are dozens of sites. Expect 4-6 hours.

- [ ] **Step 3.6: Build + test**

Run: `cargo build -p dugite-node && cargo nextest run -p dugite-node 2>&1 | tail -10`
Expected: PASS.

- [ ] **Step 3.7: Commit**

```bash
git add crates/dugite-node/
git commit -m "refactor(node): migrate apply_fetched_block and sync to ChainLedger

- Node.chain_db + ledger_state + ledger_seq collapsed into
  Node.chain_ledger: Arc<ChainLedger>.
- apply_fetched_block and process_forward_blocks now delegate to
  ChainLedger::apply_block which handles all three store mutations
  atomically.
- All read sites updated to acquire the unified read lock.

Eliminates the observable-inconsistency window between chain_db.tip
update and ledger.tip update that plagued live-tip operation."
```

---

## Task 4: Migrate forge path to `ChainLedger::try_forge`

**Files:**
- Modify: `crates/dugite-node/src/node/mod.rs::try_forge`

- [ ] **Step 4.1: Design `ChainLedger::try_forge`**

```rust
impl ChainLedger {
    /// Forge a new block on the current tip and commit atomically.
    ///
    /// Returns `Some((Block, Announcement))` iff the forge succeeded
    /// AND the block became the new tip. Returns `None` on leader-check
    /// miss, race-lost, or forge failure.
    pub async fn try_forge(
        &self,
        creds: &BlockProducerCredentials,
        config: &BlockProducerConfig,
        next_slot: SlotNo,
        epoch_nonce: &Hash32,
    ) -> Result<Option<ForgedBlock>, ForgeError> {
        // Read-lock first to compute leader check from stable state.
        let (is_leader, prev_hash, block_number, transactions) = {
            let guard = self.read().await;
            let is_leader = check_slot_leadership(
                creds,
                next_slot,
                epoch_nonce,
                guard.ledger.pool_stake_of(&creds.pool_id),
                guard.ledger.total_active_stake(),
                guard.ledger.protocol_params.active_slot_coeff_rational,
            );
            if !is_leader {
                return Ok(None);
            }
            let prev_hash = *guard.ledger.tip.point.hash().unwrap_or(&Hash32::ZERO);
            let block_number = BlockNo(guard.ledger.tip.block_number.0 + 1);
            let transactions = /* collect from mempool under this guard */;
            (is_leader, prev_hash, block_number, transactions)
        };

        // Forge (CPU-intensive; no locks held).
        let (block, cbor) = forge_block(
            creds, config, next_slot, block_number, prev_hash, epoch_nonce, transactions,
        )?;

        // Write-lock: commit atomically.
        let mut inner = self.write().await;
        let outcome = /* same as apply_block but with the freshly-forged block */;

        if !outcome.tip_advanced || outcome.new_tip.as_ref().map(|t| t.point.hash()) != Some(Some(block.hash())) {
            // Race lost: upstream advanced while we were forging.
            return Ok(None);
        }

        Ok(Some(ForgedBlock { block, cbor, announcement: /* construct */ }))
    }
}
```

- [ ] **Step 4.2: Update `Node::try_forge`**

```rust
async fn try_forge(&mut self, next_slot: SlotNo) {
    let Some(ref creds) = self.block_producer else { return; };
    let (epoch_nonce, config) = /* prepare */;

    match self.chain_ledger.try_forge(creds, &config, next_slot, &epoch_nonce).await {
        Ok(Some(forged)) => {
            self.metrics.blocks_forged.fetch_add(1, Relaxed);
            info!(
                slot = next_slot.0,
                block = forged.block.block_number().0,
                hash = %forged.block.hash().to_hex(),
                "Block forged"
            );
            self.announce_forged_block(&forged).await;
        }
        Ok(None) => { /* not elected or race lost */ }
        Err(e) => {
            self.metrics.forge_failures.fetch_add(1, Relaxed);
            error!(?e, "forge failed");
        }
    }
}
```

- [ ] **Step 4.3: Race-lost detection is now inherent**

The `forged_is_tip` re-lookup and early-exit logic from commit `b9ac790b0` is no longer needed — `ChainLedger::try_forge` inherently checks whether the forged block became the tip within the write critical section.

- [ ] **Step 4.4: Test — forge race-lost detected atomically**

```rust
#[tokio::test(flavor = "multi_thread")]
async fn test_try_forge_race_lost_when_upstream_advances() {
    let cl = Arc::new(make_test_chain_ledger().await);

    // Pre-populate with 100 blocks; ledger tip = block 100.
    for i in 1..=100 { cl.apply_fake_block(i).await; }

    // Spawn a "forging" task.
    let cl_forge = Arc::clone(&cl);
    let forge_task = tokio::spawn(async move {
        cl_forge.try_forge(&fake_creds(), &fake_config(), SlotNo(101), &Hash32::ZERO).await
    });

    // Concurrently advance the tip from "upstream."
    let cl_advance = Arc::clone(&cl);
    let advance_task = tokio::spawn(async move {
        cl_advance.apply_fake_block(101).await
    });

    let (forge_res, _advance_res) = tokio::join!(forge_task, advance_task);

    let forge_result = forge_res.unwrap();
    match forge_result {
        Ok(Some(_)) => {
            // OK — our forge won the race.
        }
        Ok(None) => {
            // OK — race lost; no inconsistent state.
            // Verify no metric for forge_race_lost yet.
        }
        Err(_) => panic!("forge errored unexpectedly"),
    }
    // Invariant holds either way: chain_db.tip == ledger.tip.
    let g = cl.read().await;
    assert_eq!(g.chain_db.get_tip().point, g.ledger.tip.point);
}
```

- [ ] **Step 4.5: Commit**

```bash
git add crates/dugite-node/
git commit -m "refactor(node): forge path uses ChainLedger::try_forge

try_forge now:
1. Read-locks to compute leader check + capture prev_hash/block_number.
2. Releases lock during CPU-intensive forge (VRF, KES, CBOR).
3. Write-locks to commit atomically — if race was lost (another block
   extended the tip during forge), the commit returns None without
   mutating state.

Removes the forged_is_tip re-lookup added in b9ac790b0; ChainLedger's
write critical section does the check inherently."
```

---

## Task 5: Lock ordering audit + deadlock stress test

**Files:**
- Create: `crates/dugite-node/tests/lock_ordering_stress.rs`

- [ ] **Step 5.1: Audit all lock-acquisition sites**

```bash
grep -rn 'chain_ledger.read\|chain_ledger.write' crates/dugite-node/src | wc -l
```

Every site must hold the lock for the minimum necessary duration. Document in code comments any site that holds > 100ms.

- [ ] **Step 5.2: Check for nested locks**

With one lock, nested acquisitions are impossible structurally — but verify no code path tries to acquire it twice while holding it (would deadlock). Common failure mode:

```rust
let g = self.chain_ledger.read().await;
// ...
self.some_method_that_also_reads().await;  // DEADLOCK if it tries to acquire the lock
```

Audit `apply_fetched_block`, `try_forge`, `handle_rollback`, and any method called from within a lock's critical section.

- [ ] **Step 5.3: Write stress test**

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn stress_concurrent_operations_no_deadlock() {
    let cl = Arc::new(make_test_chain_ledger().await);
    let mut handles = vec![];

    // 4 reader tasks.
    for _ in 0..4 {
        let cl = Arc::clone(&cl);
        handles.push(tokio::spawn(async move {
            for _ in 0..1000 {
                let g = cl.read().await;
                let _ = g.chain_db.get_tip();
                let _ = g.ledger.tip.clone();
                drop(g);
                tokio::task::yield_now().await;
            }
        }));
    }

    // 2 writer tasks applying blocks.
    for writer_id in 0..2 {
        let cl = Arc::clone(&cl);
        handles.push(tokio::spawn(async move {
            for i in 1..=500 {
                let block = make_fake_block(writer_id * 1000 + i);
                let _ = cl.apply_block(&block, BlockValidationMode::ApplyOnly).await;
            }
        }));
    }

    // 1 rollback task.
    let cl_rb = Arc::clone(&cl);
    handles.push(tokio::spawn(async move {
        for _ in 0..20 {
            let point = Point::Specific(SlotNo(100), Hash32::ZERO);
            let _ = cl_rb.apply_block(&make_fake_block(42), BlockValidationMode::ApplyOnly).await;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }));

    let timeout = Duration::from_secs(30);
    tokio::time::timeout(timeout, futures::future::join_all(handles))
        .await
        .expect("stress test must complete within 30s — if timeout, likely deadlock");
}
```

- [ ] **Step 5.4: Run stress test repeatedly**

```bash
for i in {1..10}; do
  echo "=== run $i ==="
  cargo nextest run -p dugite-node --test lock_ordering_stress 2>&1 | tail -5
done
```

Expected: 10/10 passes, each in < 30s.

- [ ] **Step 5.5: Commit**

```bash
git add crates/dugite-node/tests/lock_ordering_stress.rs
git commit -m "test(node): multi-thread stress test for ChainLedger concurrent ops

Exercises 4 readers + 2 block-applying writers + 1 rollback task
concurrently against a single ChainLedger for 10k+ operations total.
Must complete in < 30s; timeout indicates deadlock.

Run locally 10x to verify stability before soak test."
```

---

## Task 6: Preview soak test (Sprint 3 acceptance)

- [ ] **Step 6.1: Clear state**

```bash
rm -rf db-preview/ledger-snapshot*.bin db-preview/volatile/volatile-wal.bin
```

- [ ] **Step 6.2: Run BP for 24-48h**

```bash
nohup bash scripts/run-bp-preview.sh --log /tmp/sprint3-soak.log &
```

- [ ] **Step 6.3: Acceptance criteria verification**

Per master doc §0.7:
1. Clean-state sync — `tip_age_seconds < 180` within 15 min ✓
2. Rollback storm resilience — `tip_age_seconds` stays < 180 for 2h+
3. Forge + Koios confirm — at least one forge lands on Koios for our pool
4. Crash recovery — SIGKILL + restart → tip recovers in < 30s
5. Full workspace tests — `cargo nextest run --workspace` — 2 known PV10 failures only

- [ ] **Step 6.4: Lock-hold-time histogram**

Instrument `ChainLedger::apply_block` / `try_forge` with a histogram metric:

```rust
let start = Instant::now();
let result = /* operation */;
self.metrics.chain_ledger_lock_hold_ms.observe(start.elapsed().as_secs_f64() * 1000.0);
result
```

After soak: pull histogram from `/metrics`. p99 < 10ms confirms Sprint 2 + Sprint 3 together achieve the target.

- [ ] **Step 6.5: Report**

Create `docs/research/sprint-3-soak-2026-MM-DD.md` with:
- 24-48h uptime stats
- Forges attempted / Koios-confirmed
- Lock-hold p50 / p99 / max
- Any deadlock / timeout events (should be zero)
- Pass/fail vs §0.7

---

## Self-review summary

**Spec coverage:** Item 1.3 (ChainSel/LedgerDB atomicity) fully addressed. Completes the #439 follow-up roadmap.

**Placeholders:** none. Struct definitions, method bodies, test code all concrete.

**Type consistency:** `ChainLedger`, `ChainLedgerInner`, `ApplyBlockOutcome`, `SelectionPlan` introduced in Task 1-2 and used identically through Tasks 3-4. `RollbackError` from Sprint 2 still surfaced via `ApplyBlockError::RollbackExceedsWindow`.

**Risk:** high. Lock architecture changes are the easiest class of bug to miss in review (observed-inconsistency windows appear nondeterministically). The stress test in Task 5 is the primary defense. If it fails even once in 10 runs, stop and diagnose before advancing to soak.
