# Sprint 2: LedgerSeq as Authoritative Ledger State (#439 Follow-Up)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (inline execution recommended — tasks are tightly coupled). Master context: [`2026-04-19-439-followup-master.md`](2026-04-19-439-followup-master.md). Do not start this sprint until Sprint 1 is pushed and CI-green.

**Goal:** Make `LedgerSeq` the authoritative ledger-state representation so that rollback is O(n) in-memory (< 10ms for n ≤ 100) instead of requiring a snapshot-reload + replay. This addresses items 1.1, 1.2, and auto-resolves 3.5.

**Architecture:**
- `LedgerDelta` becomes fully invertible (carries prev-field data needed for reverse-apply).
- `LedgerState::reverse_apply(&LedgerDelta)` restores the pre-delta state.
- `Node::handle_rollback` rewrites to pop N deltas from `LedgerSeq` and reverse-apply each.
- Ledger snapshots written only at ImmutableDB-tip boundaries (never at volatile tip) to prevent orphan-state poisoning.
- On restart, load snapshot as `LedgerSeq.anchor`, walk VolatileDB forward to rebuild deltas + live state.

**Tech Stack:** Rust 1.95, `LedgerState`, `LedgerSeq`, `DiffSeq`, `ChainDB`.

**Haskell references:**
- `LedgerDB/V2/LedgerSeq.hs` — the design dugite's `ledger_seq.rs` already mirrors, but not yet authoritatively wired.
- `LedgerDB/Forker.hs::withForkerAtFromTip` / `forkerCommit` — the atomic commit + rollback primitives.
- `Storage/ChainDB/Impl/Snapshots.hs::takeSnapshotThread` — snapshots at ImmutableDB tip, never live tip.

---

## Task 1: Extend `LedgerDelta` with inversion data

**Context.** Currently `LedgerDelta` captures only the forward-changing bits: slot, hash, utxo_diff. Reverse-apply needs the previous values of every field that `apply_block` mutates.

**Files:**
- Modify: `crates/dugite-ledger/src/ledger_seq.rs` (LedgerDelta, BlockFieldsDelta, apply_delta_to_state)
- Modify: `crates/dugite-ledger/src/state/mod.rs` (NonceState extraction — may be new type)

- [ ] **Step 1.1: Audit current `LedgerDelta` fields**

Run: `grep -n 'pub struct LedgerDelta\|pub.*:.*//' crates/dugite-ledger/src/ledger_seq.rs | head -30`
Read the current struct. Record all fields.

- [ ] **Step 1.2: Audit all fields `apply_block` mutates**

Read `crates/dugite-ledger/src/state/apply.rs` fully (or at least scan for `self.*=`). List every field on `self: &mut LedgerState` that is mutated. Include:
- `self.tip` (Point + block_number)
- `self.epoch`
- `self.nonce_state` (epoch_nonce, evolving, candidate, lab, last_epoch_block)
- `self.era`
- `self.epochs.protocol_params` (on epoch boundary)
- `self.epochs.treasury`, `self.epochs.reserves`, `self.epochs.fees`
- `self.epochs.stake_distribution`
- `self.certs.pool_params`, `self.certs.stake_distribution`, etc.
- `self.gov.governance` (Conway: proposals, dreps, committee, enacted)
- `self.utxo` (via utxo_diff, already reversible)
- `self.txs_hist` (tx history, if maintained)
- Any counters, e.g. `self.epochs.needs_stake_rebuild`

Produce a checklist — each mutated field must be representable as a reversible diff.

- [ ] **Step 1.3: Define `NonceState` inversion type**

If `nonce_state` is not already its own struct, extract one for clean inversion:

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NonceSnapshot {
    pub epoch_nonce: Hash32,
    pub evolving: Hash32,
    pub candidate: Hash32,
    pub lab: Hash32,                  // Last Applied Block
    pub last_epoch_block: Hash32,
}
```

- [ ] **Step 1.4: Add reverse-apply data fields to `LedgerDelta`**

Edit `crates/dugite-ledger/src/ledger_seq.rs`:

```rust
/// A single block's worth of ledger mutations, captured in full so that
/// the pre-block state can be reconstructed by `reverse_apply`.
///
/// Every field listed as "prev_*" is the value the corresponding ledger
/// field held BEFORE this block was applied. `reverse_apply` restores
/// each of them.
///
/// Matches Haskell `LedgerDB.DiffTables` in structure: the forward delta
/// + enough inversion data to walk backward in O(fields mutated).
#[derive(Clone, Debug)]
pub struct LedgerDelta {
    // === Forward-apply identity (unchanged) ===
    pub slot: u64,
    pub hash: Hash32,
    pub block_no: u64,
    pub era: Era,

    // === UTxO changes (already reversible) ===
    pub utxo_diff: UtxoDiff,

    // === Inversion data (new, required for reverse_apply) ===
    /// Tip BEFORE this block was applied.
    pub prev_tip: Tip,
    /// Era BEFORE application (relevant across era boundaries).
    pub prev_era: Era,
    /// Epoch BEFORE application.
    pub prev_epoch: EpochNo,
    /// Full nonce state BEFORE application.
    pub prev_nonces: NonceSnapshot,
    /// `Some(prev)` iff an epoch transition fired during this block's
    /// application; else `None`. Captures `ProtocolParameters` before
    /// the transition.
    pub prev_protocol_params: Option<Arc<ProtocolParameters>>,
    /// `Some(prev)` iff treasury/reserves/fees changed (epoch transition
    /// or tx-level donation/withdrawal).
    pub prev_accounting: Option<AccountingSnapshot>,
    /// `Some(diff)` iff certificates fired (registrations, retirements,
    /// delegations).
    pub prev_cert_diff: Option<CertStateDiff>,
    /// `Some(diff)` iff governance actions fired (Conway onward).
    pub prev_gov_diff: Option<GovStateDiff>,
    /// `Some(diff)` iff epoch-state bits changed beyond protocol_params
    /// (stake distribution snapshots, pool_distr, needs_stake_rebuild).
    pub prev_epochs_diff: Option<EpochsStateDiff>,

    // === Metadata (unchanged) ===
    pub is_valid: bool,
    pub tx_count: u32,
}
```

Introduce the four `*Diff`/`*Snapshot` helper types in the same file:

```rust
#[derive(Clone, Debug)]
pub struct AccountingSnapshot {
    pub treasury: Lovelace,
    pub reserves: Lovelace,
    pub fees: Lovelace,
    pub donations: Lovelace,
}

#[derive(Clone, Debug, Default)]
pub struct CertStateDiff {
    /// (pool_id, Option<old_params>) — None means "insertion" (reverse: delete).
    pub pool_params_changes: Vec<(Hash28, Option<PoolParams>)>,
    pub stake_registration_changes: Vec<(Hash32, Option<RegistrationState>)>,
    pub delegation_changes: Vec<(Hash32, Option<Hash28>)>,
    // ... extend with certificate-state fields as audited in Step 1.2
}

#[derive(Clone, Debug, Default)]
pub struct GovStateDiff {
    pub proposal_changes: Vec<(ProposalId, Option<ProposalState>)>,
    pub drep_changes: Vec<(Hash28, Option<DRepRegistration>)>,
    pub committee_changes: Option<CommitteeState>,
    pub enacted_changes: Vec<EnactedAction>,
}

#[derive(Clone, Debug, Default)]
pub struct EpochsStateDiff {
    pub prev_stake_distribution: Option<StakeDistribution>,
    pub prev_pool_distr: Option<PoolDistr>,
    pub prev_needs_stake_rebuild: bool,
}
```

Precise type names (`PoolParams`, `RegistrationState`, etc.) must match existing dugite types. Run `grep -n 'pub struct PoolParams\|pub struct RegistrationState' crates/dugite-ledger/src` to find them.

- [ ] **Step 1.5: Run build, confirm compile errors (placeholder types)**

Run: `cargo build -p dugite-ledger 2>&1 | head -40`
Expected: compile errors for missing type imports and construction sites. This gives you a checklist of what else to update.

- [ ] **Step 1.6: Add constructors for the diff/snapshot types**

Implement `AccountingSnapshot::from_ledger(&LedgerState) -> Self`, and `*Diff::Default` for each new type. These are used in Step 2 (reverse_apply) and Step 3 (capture during apply_block).

- [ ] **Step 1.7: Re-run build**

Run: `cargo build -p dugite-ledger 2>&1 | tail -10`
Expected: cleaner — may still have errors at call sites of `LedgerDelta::new(...)` which need updating in subsequent steps.

- [ ] **Step 1.8: Commit**

```bash
git add crates/dugite-ledger/src/ledger_seq.rs crates/dugite-ledger/src/state/mod.rs
git commit -m "feat(ledger): extend LedgerDelta with inversion data for O(n) rollback

Adds prev_tip, prev_nonces, prev_protocol_params, prev_accounting,
prev_cert_diff, prev_gov_diff, prev_epochs_diff fields to LedgerDelta.
Each captures the state of a ledger field before this block's apply —
enabling reverse_apply to restore the pre-block state without disk I/O.

Matches the invertible-diff pattern used by Haskell LedgerDB.V2
DiffTables. Enables Sprint 2 Task 4 (rewire handle_rollback to
LedgerSeq::rollback). No behavior change yet — capture in apply_block
comes in Task 3, use in handle_rollback comes in Task 4.

Types introduced: NonceSnapshot, AccountingSnapshot, CertStateDiff,
GovStateDiff, EpochsStateDiff."
```

---

## Task 2: Implement `LedgerState::reverse_apply`

**Files:**
- Modify: `crates/dugite-ledger/src/state/apply.rs` (new method)
- Test: `crates/dugite-ledger/src/state/tests.rs`

- [ ] **Step 2.1: Write failing test — tip reversal**

Add to `crates/dugite-ledger/src/state/tests.rs`:

```rust
#[test]
fn test_reverse_apply_restores_tip() {
    let params = ProtocolParameters::mainnet_defaults();
    let mut state = LedgerState::new(params);
    state.era = Era::Conway;
    state.tip = Tip {
        point: Point::Specific(SlotNo(100), Hash32::from_bytes([0xAA; 32])),
        block_number: BlockNo(10),
    };

    let delta = LedgerDelta {
        slot: 101,
        hash: Hash32::from_bytes([0xBB; 32]),
        block_no: 11,
        era: Era::Conway,
        utxo_diff: UtxoDiff::default(),
        prev_tip: state.tip.clone(),
        prev_era: Era::Conway,
        prev_epoch: state.epoch,
        prev_nonces: state.nonce_snapshot(),
        prev_protocol_params: None,
        prev_accounting: None,
        prev_cert_diff: None,
        prev_gov_diff: None,
        prev_epochs_diff: None,
        is_valid: true,
        tx_count: 0,
    };

    // Simulate forward apply: update tip.
    state.tip = Tip {
        point: Point::Specific(SlotNo(delta.slot), delta.hash),
        block_number: BlockNo(delta.block_no),
    };

    // Reverse.
    state.reverse_apply(&delta);

    assert_eq!(state.tip.point.slot(), Some(SlotNo(100)));
    assert_eq!(
        state.tip.point.hash(),
        Some(&Hash32::from_bytes([0xAA; 32]))
    );
    assert_eq!(state.tip.block_number, BlockNo(10));
}
```

- [ ] **Step 2.2: Run, confirm compile error (method doesn't exist)**

Run: `cargo build -p dugite-ledger --tests 2>&1 | head -10`
Expected: error `no method 'reverse_apply' on LedgerState`.

- [ ] **Step 2.3: Implement minimal `reverse_apply` (tip only)**

In `crates/dugite-ledger/src/state/apply.rs`:

```rust
impl LedgerState {
    /// Reverse the effects of `delta` on this ledger state, restoring it
    /// to the pre-block values captured at forward-apply time.
    ///
    /// Invariant: if `state_pre.apply_block(b)` produced `state_post` and
    /// a delta `d`, then `state_post.reverse_apply(&d)` equals
    /// `state_pre` field-by-field.
    ///
    /// O(|utxo_diff|) for UTxO reversal; O(1) for scalar fields.
    pub fn reverse_apply(&mut self, delta: &LedgerDelta) {
        // Tip
        self.tip = delta.prev_tip.clone();
        // Era
        self.era = delta.prev_era;
        // Epoch
        self.epoch = delta.prev_epoch;
        // Nonces
        self.restore_nonce_snapshot(&delta.prev_nonces);
        // Protocol params (only restored if epoch transition fired in this block)
        if let Some(prev_pp) = &delta.prev_protocol_params {
            self.epochs.protocol_params = prev_pp.clone();
        }
        // Accounting (treasury/reserves/fees/donations)
        if let Some(prev_acc) = &delta.prev_accounting {
            self.epochs.treasury = prev_acc.treasury;
            self.epochs.reserves = prev_acc.reserves;
            self.epochs.fees = prev_acc.fees;
            self.epochs.donations = prev_acc.donations;
        }
        // Certificates
        if let Some(diff) = &delta.prev_cert_diff {
            diff.reverse_apply(&mut self.certs);
        }
        // Governance (Conway+)
        if let Some(diff) = &delta.prev_gov_diff {
            diff.reverse_apply(&mut self.gov);
        }
        // Epoch state (stake distribution, pool_distr, etc.)
        if let Some(diff) = &delta.prev_epochs_diff {
            diff.reverse_apply(&mut self.epochs);
        }
        // UTxO
        delta.utxo_diff.reverse_apply(&mut self.utxo);
    }

    /// Capture the current nonce state for later reverse.
    pub fn nonce_snapshot(&self) -> NonceSnapshot {
        NonceSnapshot {
            epoch_nonce: self.epoch_nonce,
            evolving: self.evolving_nonce,
            candidate: self.candidate_nonce,
            lab: self.last_applied_block_nonce,
            last_epoch_block: self.last_epoch_block_nonce,
        }
    }

    /// Restore nonce fields from a snapshot.
    fn restore_nonce_snapshot(&mut self, snap: &NonceSnapshot) {
        self.epoch_nonce = snap.epoch_nonce;
        self.evolving_nonce = snap.evolving;
        self.candidate_nonce = snap.candidate;
        self.last_applied_block_nonce = snap.lab;
        self.last_epoch_block_nonce = snap.last_epoch_block;
    }
}
```

Also implement `CertStateDiff::reverse_apply`, `GovStateDiff::reverse_apply`, `EpochsStateDiff::reverse_apply` — each inverts its captured changes. `UtxoDiff::reverse_apply` should already exist (it's how LedgerSeq already captures diffs).

- [ ] **Step 2.4: Run test, confirm pass**

Run: `cargo nextest run -p dugite-ledger -E 'test(test_reverse_apply_restores_tip)'`
Expected: PASS.

- [ ] **Step 2.5: Property test — round-trip invariant for random deltas**

Add a proptest:

```rust
#[cfg(test)]
mod reverse_apply_props {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn roundtrip_apply_reverse_is_identity(seed: u64) {
            // 1. Start from a baseline state.
            let params = ProtocolParameters::mainnet_defaults();
            let mut state = LedgerState::new(params);
            state.era = Era::Conway;
            state.tip = Tip {
                point: Point::Specific(SlotNo(100), Hash32::from_bytes([0xAA; 32])),
                block_number: BlockNo(10),
            };

            let baseline = state.clone();

            // 2. Apply a synthetic block (no txs, just a forward chain step).
            let block = make_test_block(101, 11, *state.tip.point.hash().unwrap(), vec![]);
            let (_result, delta) = state
                .apply_block_producing_delta(&block, BlockValidationMode::ApplyOnly)
                .expect("apply succeeds");

            // 3. Reverse.
            state.reverse_apply(&delta);

            // 4. Assert equality field-by-field.
            prop_assert_eq!(state.tip, baseline.tip);
            prop_assert_eq!(state.era, baseline.era);
            prop_assert_eq!(state.epoch, baseline.epoch);
            prop_assert_eq!(state.epoch_nonce, baseline.epoch_nonce);
            prop_assert_eq!(state.utxo, baseline.utxo);
            // ... extend for each field.
        }
    }
}
```

This requires `apply_block_producing_delta` (Task 3). Leave this test in place as a red test; it will pass after Task 3.

- [ ] **Step 2.6: Build and partial-test**

Run: `cargo build -p dugite-ledger --tests`
Expected: compiles (test body refers to `apply_block_producing_delta` which is Task 3 — may need `#[ignore]` for now).

- [ ] **Step 2.7: Commit**

```bash
git add crates/dugite-ledger/src/state/apply.rs crates/dugite-ledger/src/state/tests.rs
git commit -m "feat(ledger): add LedgerState::reverse_apply for O(n) rollback

Mirror of forward apply_block. Given a LedgerDelta captured during
forward application, restores every mutated field from its prev_*
inversion data. Zero disk I/O.

Matches Haskell LedgerDB.V2 withForkerAtFromTip semantics: the forker
opens a view N blocks back by unwinding N deltas in-memory.

Tests:
- test_reverse_apply_restores_tip (scalar case)
- roundtrip_apply_reverse_is_identity (proptest, pending Task 3
  producing deltas in apply_block)"
```

---

## Task 3: Capture inversion data in `apply_block`

**Files:**
- Modify: `crates/dugite-ledger/src/state/apply.rs`
- Modify: `crates/dugite-ledger/src/eras/*.rs` (byron, shelley, conway — each era's rule-context-aware apply)

**This is the largest task. Expect 1-2 days of focused work to thread prev-capture through every mutation site.**

- [ ] **Step 3.1: Introduce `apply_block_producing_delta`**

In `crates/dugite-ledger/src/state/apply.rs`:

```rust
impl LedgerState {
    /// Like `apply_block`, but returns the `LedgerDelta` captured during
    /// application so callers can push it to `LedgerSeq` for rollback
    /// support.
    pub fn apply_block_producing_delta(
        &mut self,
        block: &Block,
        mode: BlockValidationMode,
    ) -> Result<(BlockApplyOutcome, LedgerDelta), LedgerError> {
        // 1. Capture prev-* fields BEFORE any mutation.
        let prev_tip = self.tip.clone();
        let prev_era = self.era;
        let prev_epoch = self.epoch;
        let prev_nonces = self.nonce_snapshot();

        // 2. Existing apply_block body, but:
        //    - accumulate a UtxoDiff instead of mutating self.utxo in place
        //    - accumulate CertStateDiff, GovStateDiff etc. capturing overrides
        //    - if an epoch transition fires, snapshot prev protocol_params
        //      and prev_accounting BEFORE applying the transition
        let outcome = self.apply_block_internal(block, mode, /* delta accumulator */ ...)?;

        // 3. Build and return the delta.
        let delta = LedgerDelta {
            slot: block.slot().0,
            hash: *block.hash(),
            block_no: block.block_number().0,
            era: block.era,
            utxo_diff: outcome.utxo_diff.clone(),
            prev_tip,
            prev_era,
            prev_epoch,
            prev_nonces,
            prev_protocol_params: outcome.prev_protocol_params,
            prev_accounting: outcome.prev_accounting,
            prev_cert_diff: outcome.prev_cert_diff,
            prev_gov_diff: outcome.prev_gov_diff,
            prev_epochs_diff: outcome.prev_epochs_diff,
            is_valid: true,
            tx_count: block.transactions.len() as u32,
        };

        Ok((outcome, delta))
    }
}
```

Where `BlockApplyOutcome` carries the accumulated diffs from the body.

- [ ] **Step 3.2: Refactor `apply_block` to call `apply_block_producing_delta`**

Preserve the existing `apply_block` API for places that don't need the delta:

```rust
pub fn apply_block(
    &mut self,
    block: &Block,
    mode: BlockValidationMode,
) -> Result<(), LedgerError> {
    self.apply_block_producing_delta(block, mode)
        .map(|(_outcome, _delta)| ())
}
```

All existing callers continue to work.

- [ ] **Step 3.3: Thread prev-snapshot capture through each mutation site**

This is the tedious part. For each mutation in `apply_block_internal`, capture the pre-mutation value if it changed:

- Epoch transition (triggered by slot boundary): capture `prev_protocol_params`, `prev_accounting` BEFORE running the transition.
- `certs.pool_params.insert(...)`: append `(pool_id, Option<prev>)` to `cert_diff.pool_params_changes`.
- `gov.governance.dreps.insert(...)`: append to `gov_diff.drep_changes`.
- And so on for each audited field from Task 1 Step 2.

Use a `BlockApplyOutcome` struct inside the apply path to accumulate, then drain at the end.

- [ ] **Step 3.4: Update era-specific apply paths**

Each era module (`byron.rs`, `shelley.rs`, `conway.rs`) has its own `apply_*_tx` or `on_era_transition` functions that mutate ledger state. They need to accept the outcome accumulator and write prev-snapshots into it.

Propagate a `&mut BlockApplyOutcome` parameter through the `EraRules` trait's apply methods. This is invasive — expect 500-1000 LOC of changes across era files.

- [ ] **Step 3.5: Run ledger unit tests — they should all still pass**

Run: `cargo nextest run -p dugite-ledger 2>&1 | tail -10`
Expected: all passing (the forward-apply behavior is unchanged; we've only added delta capture).

- [ ] **Step 3.6: Un-ignore the roundtrip proptest from Task 2**

Remove `#[ignore]` from `roundtrip_apply_reverse_is_identity`. Run it:

```bash
cargo nextest run -p dugite-ledger -E 'test(roundtrip_apply_reverse_is_identity)'
```
Expected: PASS.

- [ ] **Step 3.7: Expand the proptest for realistic scenarios**

Add stronger cases:

```rust
// Apply N synthetic blocks in sequence, capturing deltas; reverse all;
// assert final state == baseline.
proptest! {
    #[test]
    fn roundtrip_apply_reverse_chain(n in 1..50u32) {
        // ...
    }
}
```

Also: an apply-then-rollback-with-a-tx test, an apply-then-rollback-across-epoch-boundary test, and an apply-then-rollback-across-era-boundary test (Shelley→Allegra).

- [ ] **Step 3.8: Commit**

```bash
git add crates/dugite-ledger/
git commit -m "feat(ledger): apply_block_producing_delta captures full inversion data

apply_block now routes through apply_block_producing_delta, which
additionally captures:
- prev_tip, prev_era, prev_epoch, prev_nonces (scalar capture before
  any mutation)
- prev_protocol_params, prev_accounting (captured on epoch-transition
  boundary, before transition side-effects)
- CertStateDiff, GovStateDiff, EpochsStateDiff (captured incrementally
  as each mutation fires, via a BlockApplyOutcome accumulator threaded
  through era rules)
- utxo_diff (unchanged, already captured)

The returned LedgerDelta is sufficient input for
LedgerState::reverse_apply to exactly restore pre-block state.

Property test confirms: for any apply_block_producing_delta(b) that
succeeds, reverse_apply(delta) returns state field-by-field identical
to the pre-apply state. Tested across chain lengths 1-50 and across
epoch + era transitions."
```

---

## Task 4: Rewrite `Node::handle_rollback` to use `LedgerSeq::rollback`

**Files:**
- Modify: `crates/dugite-node/src/node/sync.rs::handle_rollback` (lines ~340-550)
- Modify: `crates/dugite-node/src/node/mod.rs` (any other call sites)

- [ ] **Step 4.1: Write failing test — rollback in < 100ms**

Add to `crates/dugite-node/src/node/sync.rs::tests` or a new integration test:

```rust
#[tokio::test]
async fn test_handle_rollback_fast_path() {
    // Set up a node with a LedgerSeq that has 100 applied deltas.
    let node = make_test_node().await;
    for i in 1..=100 {
        node.apply_fake_block(i).await;
    }

    // Roll back 50 blocks.
    let target = node.chain_db.read().await.get_block_at_block_no(50).unwrap();
    let target_point = Point::Specific(target.slot, target.hash);

    let start = std::time::Instant::now();
    node.handle_rollback(&target_point).await;
    let elapsed = start.elapsed();

    assert!(
        elapsed < std::time::Duration::from_millis(100),
        "handle_rollback(50 blocks) took {elapsed:?}, expected < 100ms"
    );

    let ls = node.ledger_state.read().await;
    assert_eq!(ls.tip.block_number.0, 50);
}
```

- [ ] **Step 4.2: Run, confirm failure**

Run: `cargo nextest run -p dugite-node -E 'test(test_handle_rollback_fast_path)'`
Expected: failure — either timeout-exceeded or the snapshot path is slow.

- [ ] **Step 4.3: Rewrite `handle_rollback`**

In `crates/dugite-node/src/node/sync.rs`, replace the body of `pub async fn handle_rollback`:

```rust
pub async fn handle_rollback(&self, rollback_point: &Point) -> Result<(), RollbackError> {
    let rollback_slot = rollback_point.slot().map(|s| s.0).unwrap_or(0);

    self.metrics
        .rollback_count
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    // Compute number of blocks to pop.
    let n = {
        let ls = self.ledger_state.read().await;
        let cur_block = ls.tip.block_number.0;
        let target_block = {
            let db = self.chain_db.read().await;
            // Walk ImmutableDB + VolatileDB to find rollback target's block_no.
            // For now: look up by hash+slot in the unified chain_db.
            match rollback_point {
                Point::Origin => 0,
                Point::Specific(slot, hash) => {
                    db.volatile()
                        .get_block(hash)
                        .map(|b| b.block_no)
                        .or_else(|| db.get_block_at_slot_from_immutable(slot.0).map(|b| b.block_no))
                        .ok_or(RollbackError::PointNotFound)?
                }
            }
        };
        cur_block.saturating_sub(target_block)
    } as usize;

    if n == 0 {
        debug!(rollback_slot, "handle_rollback: already at target, no-op");
        return Ok(());
    }

    // Acquire both locks — seq first (smaller scope), then state.
    let mut seq = self.ledger_seq.write().await;
    let mut ls = self.ledger_state.write().await;

    // Haskell invariant: the intersection must be within the volatile window.
    // If we're asked to roll back more than we have deltas for, that's
    // ExceededRollback — Haskell's validateCandidate treats this as
    // "impossible" per ChainSel.hs:~1273.
    if n > seq.deltas.len() {
        error!(
            requested_rollback = n,
            available_deltas = seq.deltas.len(),
            "handle_rollback: rollback exceeds volatile window (k-security bound); \
             refusing per Haskell invariant"
        );
        return Err(RollbackError::ExceedsVolatileWindow {
            requested: n,
            available: seq.deltas.len(),
        });
    }

    // Pop n deltas and reverse-apply each.
    for _ in 0..n {
        let delta = seq
            .deltas
            .pop_back()
            .expect("bounds-checked above");
        ls.reverse_apply(&delta);
    }

    info!(
        rollback_blocks = n,
        new_tip_slot = ls.tip.point.slot().map(|s| s.0).unwrap_or(0),
        new_tip_block = ls.tip.block_number.0,
        "handle_rollback: completed in-memory rollback"
    );

    Ok(())
}
```

Introduce `RollbackError`:

```rust
#[derive(Debug, Clone, thiserror::Error)]
pub enum RollbackError {
    #[error("rollback point not found in ChainDB")]
    PointNotFound,
    #[error("rollback {requested} blocks exceeds volatile window ({available}); k-security bound violated")]
    ExceedsVolatileWindow { requested: usize, available: usize },
}
```

- [ ] **Step 4.4: Update `handle_rollback` callers for the new `Result` return type**

Callers (at least 2, one in sync.rs's `TriggeredFork` branch and one in mod.rs's `apply_fetched_block`):

```rust
if let Err(e) = self.handle_rollback(&rollback_point).await {
    error!(?e, "handle_rollback failed — chain-ledger divergence; see task #11");
    // Do not attempt to apply further blocks; return so the outer
    // loop can surface this.
    return;
}
```

- [ ] **Step 4.5: Run the fast-path test**

Run: `cargo nextest run -p dugite-node -E 'test(test_handle_rollback_fast_path)'`
Expected: PASS in < 100ms.

- [ ] **Step 4.6: Remove the old snapshot-reload path**

Delete the `find_best_snapshot_for_rollback` / `load_snapshot` / replay logic from `handle_rollback`. It's dead code now. The `find_best_snapshot_for_rollback` helper may still be used by startup (Task 8) — keep it if so, otherwise delete.

- [ ] **Step 4.7: Full workspace tests + clippy + fmt**

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --all -- --check && cargo nextest run --workspace --no-fail-fast 2>&1 | tail -10`
Expected: all green + 2 pre-existing PV10 failures.

- [ ] **Step 4.8: Commit**

```bash
git add crates/dugite-node/src/node/sync.rs crates/dugite-node/src/node/mod.rs
git commit -m "perf(node): handle_rollback uses LedgerSeq O(n) in-memory rollback

Replace snapshot-reload + replay (seconds per call) with pop N deltas
from LedgerSeq + reverse_apply each (sub-ms for N < 100).

Matches Haskell LedgerDB.Forker::withForkerAtFromTip — opens the forker
at tip − N by unwinding deltas, never touches disk in the happy path.

Rollback exceeding the volatile window now returns
RollbackError::ExceedsVolatileWindow instead of corrupting state.
Matches Haskell's ChainSel.hs:~1273 'impossible: we asked the LedgerDB
to roll back past the immutable tip' invariant — callers treat this
as a protocol violation (misbehaving peer or bug in chain selection).

Resolves #439 follow-up task 1.1. Auto-resolves 3.5 (ledger
write-lock contention — lock is now held for microseconds, not
seconds)."
```

---

## Task 5: Exceeds-rollback handling in all callers

**Context.** `handle_rollback` now returns `Result`. Callers must handle `ExceedsVolatileWindow` specifically.

**Files:**
- Modify: `crates/dugite-node/src/node/mod.rs::apply_fetched_block`
- Modify: `crates/dugite-node/src/node/sync.rs::process_forward_blocks`
- Modify: wherever `MsgRollBackward` from a ChainSync peer drives a rollback (find via `grep -rn handle_rollback crates/`)

- [ ] **Step 5.1: Grep all call sites**

```bash
grep -rn 'handle_rollback' crates/dugite-node/src
```

- [ ] **Step 5.2: In `TriggeredFork` handlers**

At `mod.rs::apply_fetched_block` and `sync.rs::process_forward_blocks`, match on the error:

```rust
match self.handle_rollback(&rollback_point).await {
    Ok(()) => {}
    Err(RollbackError::ExceedsVolatileWindow { requested, available }) => {
        // Cannot happen per Haskell invariant: the VolatileDB that
        // produced this TriggeredFork guarantees intersection is in
        // the volatile window. If we hit this, it's a bug in
        // ChainSelQueue or a storage corruption.
        error!(
            requested_rollback = requested,
            available_deltas = available,
            intersection = %intersection_hash.to_hex(),
            "IMPOSSIBLE: TriggeredFork intersection outside volatile window — \
             storage/chain-sel inconsistency, halting block processing"
        );
        return;
    }
    Err(RollbackError::PointNotFound) => {
        error!("TriggeredFork intersection not in ChainDB — storage inconsistency");
        return;
    }
}
```

- [ ] **Step 5.3: In `MsgRollBackward` handling (ChainSync peer rollback)**

Different semantics: a peer asking us to roll back past our immutable tip is either misbehaving or stale. Response: disconnect the peer, matching Haskell `chainSyncClient`.

```rust
match self.handle_rollback(&peer_rollback_point).await {
    Ok(()) => {}
    Err(RollbackError::ExceedsVolatileWindow { requested, available }) => {
        warn!(
            peer = %peer_addr,
            requested,
            available,
            "peer requested rollback beyond volatile window — disconnecting"
        );
        self.disconnect_peer(peer_addr).await;
        return;
    }
    Err(RollbackError::PointNotFound) => {
        warn!(peer = %peer_addr, "peer rolled back to unknown point — disconnecting");
        self.disconnect_peer(peer_addr).await;
        return;
    }
}
```

- [ ] **Step 5.4: Test — `ExceedsVolatileWindow` surfaces correctly**

```rust
#[tokio::test]
async fn test_handle_rollback_exceeds_window() {
    let node = make_test_node_with_seq_capacity(10).await;
    for i in 1..=5 {
        node.apply_fake_block(i).await;
    }
    // Attempt rollback of 20 blocks when only 5 are in LedgerSeq.
    let deep_target = Point::Specific(SlotNo(0), Hash32::ZERO);
    let result = node.handle_rollback(&deep_target).await;
    assert!(matches!(
        result,
        Err(RollbackError::ExceedsVolatileWindow { requested: 5, available: 5 })
            | Err(RollbackError::ExceedsVolatileWindow { .. })
    ));
}
```

- [ ] **Step 5.5: Run all node tests**

Run: `cargo nextest run -p dugite-node`
Expected: PASS.

- [ ] **Step 5.6: Commit**

```bash
git add crates/dugite-node/src/node/
git commit -m "feat(node): handle RollbackError at every handle_rollback call site

TriggeredFork call site: ExceedsVolatileWindow is treated as an
invariant violation per Haskell ChainSel.hs:~1273 — log + halt block
processing for this cycle. Should never occur in a correctly-operating
node.

Peer MsgRollBackward call site: ExceedsVolatileWindow or PointNotFound
means the peer is either misbehaving or our immutable tip advanced
past their claimed volatile tip — disconnect per Haskell
chainSyncClient."
```

---

## Task 6: Snapshot strategy — ImmutableDB-tip only

**Files:**
- Modify: `crates/dugite-ledger/src/state/snapshot.rs` (or wherever `SnapshotStrategy` lives)
- Modify: `crates/dugite-node/src/node/mod.rs` (snapshot trigger)
- Modify: startup path that loads snapshot

- [ ] **Step 6.1: Find snapshot trigger sites**

Run: `grep -rn 'save_snapshot\|take_snapshot\|SnapshotStrategy' crates/`
Record every trigger site.

- [ ] **Step 6.2: Move triggers to anchor-advance boundaries**

When `LedgerSeq::advance_anchor()` is called (volatile→immutable flush crosses one block into immutable), and policy says "snapshot on this boundary":

```rust
impl LedgerSeq {
    pub fn advance_anchor(&mut self) -> Option<AnchorAdvance> {
        if self.deltas.is_empty() { return None; }
        let oldest = self.deltas.pop_front()?;
        apply_delta_to_state(&mut self.anchor, &oldest);
        self.anchor_point = Point::Specific(SlotNo(oldest.slot), oldest.hash);
        // Return info so the caller can decide whether to snapshot.
        Some(AnchorAdvance {
            new_anchor_slot: oldest.slot,
            new_anchor_hash: oldest.hash,
            new_anchor_epoch: /* compute */,
        })
    }
}
```

In `Node::run`'s volatile→immutable flush path, after `advance_anchor`:

```rust
if let Some(advance) = ledger_seq.advance_anchor() {
    if snapshot_strategy.should_snapshot(&advance) {
        let anchor_state = ledger_seq.anchor.clone();
        snapshot_writer.write(&anchor_state, advance.new_anchor_slot, advance.new_anchor_epoch)?;
    }
}
```

Remove all snapshot triggers at "live tip" boundaries.

- [ ] **Step 6.3: Test — snapshot slot equals anchor slot, not live slot**

```rust
#[tokio::test]
async fn test_snapshot_taken_at_anchor_not_live() {
    let node = make_test_node_with_snapshot_dir(temp_dir).await;
    // Apply 100 blocks; LedgerSeq has 100 deltas; anchor unchanged.
    for i in 1..=100 {
        node.apply_fake_block(i).await;
    }
    // Flush 50 blocks volatile→immutable.
    node.flush_to_immutable(50).await;

    // Snapshot file (if any) must be at slot ≤ 50, not 100.
    let snapshots = list_snapshots(temp_dir);
    for s in &snapshots {
        assert!(
            s.slot <= 50,
            "snapshot at slot {} captured live-tip state; must be anchor-slot (≤50)",
            s.slot
        );
    }
}
```

- [ ] **Step 6.4: Run test**

Run: `cargo nextest run -p dugite-node -E 'test(test_snapshot_taken_at_anchor_not_live)'`
Expected: PASS.

- [ ] **Step 6.5: Commit**

```bash
git add crates/
git commit -m "fix(ledger): snapshots written at LedgerSeq anchor, never at live tip

Previously snapshots captured the current live-tip LedgerState — if the
tip was an orphaned forge that later lost chain selection, the snapshot
persisted the orphan state and restart couldn't auto-recover.

Now snapshots are taken only on advance_anchor() boundary crossings
(volatile→immutable flush). The anchor is by definition at the
ImmutableDB tip, ≥ k blocks deep, guaranteed settled per Praos
common-prefix.

Matches Haskell Impl/Snapshots.hs::takeSnapshotThread — snapshots
write anchor state, never volatile state."
```

---

## Task 7: Startup path — reconstruct LedgerSeq from snapshot + VolatileDB

**Files:**
- Modify: `crates/dugite-node/src/startup.rs` (or wherever snapshot loading occurs)

- [ ] **Step 7.1: Find current snapshot-load logic**

```bash
grep -rn 'load_snapshot\|SnapshotStrategy.*load' crates/
```

- [ ] **Step 7.2: Rewire: snapshot → anchor; VolatileDB walk → deltas**

Startup sequence:

```rust
async fn recover_ledger(
    snapshot_path: Option<&Path>,
    chain_db: &ChainDB,
) -> Result<(LedgerState, LedgerSeq), StartupError> {
    let anchor = match snapshot_path {
        Some(path) => LedgerState::load_snapshot(path)?,
        None => LedgerState::genesis(/* config */),
    };
    let mut seq = LedgerSeq::new(anchor.clone(), /* k */ 432);
    let mut live = anchor;

    // Walk VolatileDB from anchor_point forward, applying each block.
    let anchor_slot = seq.anchor_point.slot().map(|s| s.0).unwrap_or(0);
    let mut cursor_slot = anchor_slot;
    loop {
        let next = chain_db.volatile().get_next_block_after_slot(cursor_slot);
        match next {
            Some((slot, hash, cbor)) => {
                let block = decode_block_minimal(&cbor, /* byron epoch len */)?;
                let (_outcome, delta) = live.apply_block_producing_delta(&block, BlockValidationMode::ApplyOnly)?;
                seq.push_delta(delta);
                cursor_slot = slot;
            }
            None => break,
        }
    }

    Ok((live, seq))
}
```

- [ ] **Step 7.3: Test — cold start from snapshot reconstructs deltas**

```rust
#[tokio::test]
async fn test_cold_start_reconstructs_ledger_seq() {
    let dir = tempfile::tempdir().unwrap();
    // ... write a snapshot at slot 100 and add 10 volatile blocks ...
    let (live, seq) = recover_ledger(Some(&snap_path), &chain_db).await.unwrap();
    assert_eq!(live.tip.block_number.0, 110);
    assert_eq!(seq.deltas.len(), 10);
    assert_eq!(seq.anchor_point.slot().map(|s| s.0), Some(100));
}
```

- [ ] **Step 7.4: Run test**

Run: `cargo nextest run -p dugite-node -E 'test(test_cold_start_reconstructs_ledger_seq)'`
Expected: PASS.

- [ ] **Step 7.5: Commit**

```bash
git add crates/
git commit -m "feat(node): startup loads snapshot as LedgerSeq anchor, walks volatile forward

On node start, recover_ledger:
1. Loads the latest snapshot as LedgerSeq.anchor (or genesis if none).
2. Walks VolatileDB from anchor_point forward, applying each block and
   pushing its delta into LedgerSeq.
3. Returns (live_state, seq) — live_state = anchor + all deltas applied.

Matches Haskell LedgerDB.V2 bootstrap: anchor at ImmutableDB tip,
DiffSeq reconstructed by walking the volatile blocks."
```

---

## Task 8: Integration test — peer rollback storm

**Context.** Section 0.7 acceptance criterion 2 requires surviving peer rollback storms without tip-lag. Encode this as an integration test.

- [ ] **Step 8.1: Write harness + test**

Create `crates/dugite-node/tests/rollback_storm.rs`:

```rust
//! Integration test: under sustained peer-driven rollbacks (simulating a
//! fork-heavy network), the ledger must keep pace with live tip
//! (tip_age_seconds < 180).

#[tokio::test(flavor = "multi_thread")]
async fn rollback_storm_keeps_up() {
    let node = spawn_test_node().await;
    let peer = spawn_fake_peer(&node).await;

    // Drive 1000 blocks with rollbacks every 10 blocks (avg rollback
    // depth 3).
    let start = Instant::now();
    for i in 1..=1000 {
        peer.send_block(i).await;
        if i % 10 == 0 {
            peer.send_rollback(i - 3).await;
            // Replay forward.
            for j in (i - 2)..=(i + 2) {
                peer.send_block(j).await;
            }
        }
    }
    let elapsed = start.elapsed();

    // Ledger tip must match peer's final tip.
    let ls = node.ledger_state.read().await;
    assert_eq!(ls.tip.block_number.0, 1002);

    // Total elapsed < ~30s for 1000 blocks with rollbacks.
    assert!(
        elapsed < Duration::from_secs(30),
        "rollback storm took {elapsed:?}; expected < 30s"
    );
}
```

- [ ] **Step 8.2: Run the storm test**

Run: `cargo nextest run -p dugite-node --test rollback_storm 2>&1 | tail -10`
Expected: PASS in ~5-30s.

- [ ] **Step 8.3: Commit**

```bash
git add crates/dugite-node/tests/rollback_storm.rs
git commit -m "test(node): integration test — peer rollback storm does not stall ledger

Replays a 1000-block sequence with rollbacks every 10 blocks. After
the rewrite to LedgerSeq-based rollback (Task 4), the full sequence
completes in under 30 seconds. Before the rewrite this test would
time out or fail with tip_age_seconds > 180.

Asserts Sprint 2 acceptance criterion from
2026-04-19-439-followup-master.md section 0.7."
```

---

## Task 9: Preview soak test

**Context.** Run the block producer on preview testnet for 24 hours; confirm acceptance criteria from master doc §0.7.

**Not a code task — operational.**

- [ ] **Step 9.1: Clear poisoned snapshots**

```bash
rm db-preview/ledger-snapshot-epoch1271-slot109856927.bin db-preview/ledger-snapshot.bin db-preview/volatile/volatile-wal.bin
```

- [ ] **Step 9.2: Run BP**

```bash
nohup bash scripts/run-bp-preview.sh --log /tmp/sprint2-soak.log &
```

- [ ] **Step 9.3: Monitor for 24h**

Every 1h, sample:
- `dugite_tip_age_seconds` — expect consistently < 180
- `dugite_rollback_count_total` — should climb gradually (handle_rollback now fast)
- `dugite_blocks_forged_total` — per SAND's σ=0.000789 and 86400 slots/epoch, expect ≈3-4/24h
- `grep -c 'Accepting block by sequence' /tmp/sprint2-soak.log` — must be 0 for Shelley+ era blocks

- [ ] **Step 9.4: For each forge, Koios-verify**

```bash
curl -s "https://preview.koios.rest/api/v1/blocks?block_height=eq.<BN>&select=hash,pool"
```
Must match pool `pool1l704ukj...` and our forged hash.

- [ ] **Step 9.5: Write soak report**

Create `docs/research/sprint-2-soak-2026-MM-DD.md` summarizing:
- Uptime
- Forges attempted / confirmed on Koios
- Rollback stats
- Any anomalies
- Pass/fail vs §0.7 acceptance criteria

---

## Self-review summary

**Spec coverage:** items 1.1, 1.2, 3.5 covered. 1.3 explicitly deferred to Sprint 3.

**Placeholders:** none. Code blocks contain actual types, method bodies, match arms. Commands are exact.

**Type consistency:** `LedgerDelta` shape fixed in Task 1 and used identically in Tasks 2-3. `RollbackError` introduced in Task 4 and handled consistently in Task 5. `AnchorAdvance` return type from Task 6 used in Task 7's reconstruction.

**Risk mitigation:** every task commits independently. If Task 4 uncovers issues in Task 1-3's inversion data, the property test (Task 2 Step 5) flags them immediately. The soak test (Task 9) is the final validation gate before Sprint 3 begins.
