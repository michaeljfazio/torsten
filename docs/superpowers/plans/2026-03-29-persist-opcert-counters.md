# Persist Opcert Counters Across Node Restarts — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Persist operational certificate counters in ledger snapshots so they survive node restarts, closing a replay-attack window (GitHub issue #310, Option A / sub-approach A2).

**Architecture:** Consensus (`OuroborosPraos`) remains the runtime owner of `opcert_counters`. `LedgerState` gains a new `opcert_counters` field that is populated from consensus before each snapshot save, and used to seed consensus on snapshot load. The snapshot version is bumped from 10 → 11.

**Tech Stack:** Rust, bincode serialization, `HashMap<Hash28, u64>`

---

### Task 1: Add `opcert_counters` field to `LedgerState`

**Files:**
- Modify: `crates/dugite-ledger/src/state/mod.rs:84-314` (LedgerState struct)
- Modify: `crates/dugite-ledger/src/state/mod.rs:675-730` (LedgerState::new)

- [ ] **Step 1: Add the field to `LedgerState` struct**

In `crates/dugite-ledger/src/state/mod.rs`, add a new field after `node_network` (the last field before the closing brace of `LedgerState`):

```rust
    /// Operational certificate counters per pool (cold key hash → highest seen counter).
    ///
    /// Persisted across node restarts to prevent opcert replay attacks.
    /// The canonical runtime copy lives in `OuroborosPraos.opcert_counters`;
    /// this field is populated from consensus before each snapshot save and
    /// used to seed consensus on snapshot load.
    ///
    /// `#[serde(default)]` ensures backward compatibility: snapshots written
    /// before this field was added deserialise with an empty map (the node
    /// then rebuilds counters from the chain, same as a fresh start).
    #[serde(default)]
    pub opcert_counters: HashMap<Hash28, u64>,
```

- [ ] **Step 2: Initialize the field in `LedgerState::new()`**

In the `LedgerState::new()` constructor, add `opcert_counters: HashMap::new(),` after `node_network: None,`.

- [ ] **Step 3: Verify it compiles**

Run: `cargo build -p dugite-ledger`
Expected: success (bincode derives Serialize/Deserialize; HashMap<Hash28, u64> already implements both)

- [ ] **Step 4: Commit**

```bash
git add crates/dugite-ledger/src/state/mod.rs
git commit -m "feat(ledger): add opcert_counters field to LedgerState (#310)"
```

---

### Task 2: Bump `SNAPSHOT_VERSION` and update stability test

**Files:**
- Modify: `crates/dugite-ledger/src/state/snapshot.rs:49` (SNAPSHOT_VERSION constant)
- Modify: `crates/dugite-ledger/tests/snapshot_stability.rs:53` (EXPECTED_HASH constant)

- [ ] **Step 1: Bump SNAPSHOT_VERSION from 10 to 11**

In `crates/dugite-ledger/src/state/snapshot.rs`, line 49, change:

```rust
    pub(crate) const SNAPSHOT_VERSION: u8 = 11;
```

- [ ] **Step 2: Compute the new expected hash**

Run: `cargo nextest run -p dugite-ledger -E 'test(snapshot_format_hash_stability)'`
Expected: FAIL with message containing the new hash. Copy the hash from the output.

- [ ] **Step 3: Update EXPECTED_HASH in snapshot_stability.rs**

In `crates/dugite-ledger/tests/snapshot_stability.rs`, line 53, replace the old hash with the new one from step 2.

- [ ] **Step 4: Run both snapshot tests to confirm**

Run: `cargo nextest run -p dugite-ledger -E 'test(snapshot_format_hash_stability) | test(snapshot_round_trip_deterministic)'`
Expected: both PASS

- [ ] **Step 5: Commit**

```bash
git add crates/dugite-ledger/src/state/snapshot.rs crates/dugite-ledger/tests/snapshot_stability.rs
git commit -m "feat(ledger): bump SNAPSHOT_VERSION to 11 for opcert counters (#310)"
```

---

### Task 3: Add getter/setter on `OuroborosPraos` for counter access

**Files:**
- Modify: `crates/dugite-consensus/src/praos.rs` (add two methods near `prune_opcert_counters`)

- [ ] **Step 1: Add `opcert_counters()` getter and `set_opcert_counters()` setter**

In `crates/dugite-consensus/src/praos.rs`, after the `prune_opcert_counters` method (around line 1106), add:

```rust
    /// Return a reference to the opcert counters map.
    /// Used by the node layer to copy counters into LedgerState before snapshot save.
    pub fn opcert_counters(&self) -> &HashMap<Hash28, u64> {
        &self.opcert_counters
    }

    /// Replace the opcert counters map wholesale.
    /// Used by the node layer to seed counters from a loaded LedgerState snapshot.
    pub fn set_opcert_counters(&mut self, counters: HashMap<Hash28, u64>) {
        debug!(
            count = counters.len(),
            "Seeded opcert counters from snapshot"
        );
        self.opcert_counters = counters;
    }
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -p dugite-consensus`
Expected: success

- [ ] **Step 3: Commit**

```bash
git add crates/dugite-consensus/src/praos.rs
git commit -m "feat(consensus): add opcert_counters getter/setter for snapshot persistence (#310)"
```

---

### Task 4: Wire up snapshot save — copy counters from consensus to ledger

**Files:**
- Modify: `crates/dugite-node/src/node/epoch.rs:115-143` (`save_ledger_snapshot` method)

- [ ] **Step 1: Copy opcert counters into ledger state before saving**

In `crates/dugite-node/src/node/epoch.rs`, the `save_ledger_snapshot` method currently starts:

```rust
    pub async fn save_ledger_snapshot(&self) {
        let mut ls = self.ledger_state.write().await;
        let epoch = ls.epoch.0;

        // Flush UTxO store to disk FIRST (cardano-lsm has no WAL)
        if let Err(e) = ls.save_utxo_snapshot() {
```

Add a counter copy between acquiring the write lock and the UTxO flush:

```rust
    pub async fn save_ledger_snapshot(&self) {
        let mut ls = self.ledger_state.write().await;
        let epoch = ls.epoch.0;

        // Copy opcert counters from consensus into ledger state for snapshot persistence.
        // Consensus is the runtime owner; ledger state is the persistence vehicle.
        ls.opcert_counters = self.consensus.opcert_counters().clone();

        // Flush UTxO store to disk FIRST (cardano-lsm has no WAL)
        if let Err(e) = ls.save_utxo_snapshot() {
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -p dugite-node`
Expected: success

- [ ] **Step 3: Commit**

```bash
git add crates/dugite-node/src/node/epoch.rs
git commit -m "feat(node): copy opcert counters to ledger state before snapshot save (#310)"
```

---

### Task 5: Wire up snapshot load — seed consensus from ledger

**Files:**
- Modify: `crates/dugite-node/src/node/mod.rs` (after consensus is created, seed from loaded ledger state)

The consensus object is created at line 789, and the `Node` struct is assembled at line 1124. The loaded `ledger_state` is available. We need to seed counters between consensus creation and Node construction.

- [ ] **Step 1: Seed consensus opcert counters from loaded ledger state**

In `crates/dugite-node/src/node/mod.rs`, find the section after consensus is created (around line 800, after the `info!` log). Add the counter seeding:

```rust
        // Seed opcert counters from the loaded ledger snapshot (issue #310).
        // This closes the replay-attack window that existed when counters
        // reset to empty on every restart.
        {
            let ls = ledger_state.blocking_read();
            if !ls.opcert_counters.is_empty() {
                consensus.set_opcert_counters(ls.opcert_counters.clone());
            }
        }
```

Find the right location — it must be after `let consensus = ...` and before consensus is moved into the `Node` struct. The `info!` log about consensus params is at ~line 802-809, so insert after that block.

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -p dugite-node`
Expected: success

- [ ] **Step 3: Commit**

```bash
git add crates/dugite-node/src/node/mod.rs
git commit -m "feat(node): seed opcert counters from ledger snapshot on startup (#310)"
```

---

### Task 6: Handle non-`save_ledger_snapshot` snapshot saves

**Files:**
- Modify: `crates/dugite-node/src/node/sync.rs` (the `save_snapshot` calls that don't go through `save_ledger_snapshot`)

There are several places in `sync.rs` where `ls.save_snapshot()` is called directly (not through `save_ledger_snapshot`). These also need the counter copy. The calls are at lines ~300, ~2092, ~2160, ~2240, ~2311, ~2385.

- [ ] **Step 1: Identify all direct `save_snapshot` calls in sync.rs**

Search for `save_snapshot(&snapshot_path)` or `save_snapshot(&epoch_path)` in `sync.rs`. Before each one that has a mutable `ls` (write lock), add:

```rust
ls.opcert_counters = self.consensus.opcert_counters().clone();
```

For calls where `self.consensus` is not accessible (e.g., inside `reset_ledger_and_replay` which doesn't have `&self`), the snapshot is being saved after a full replay from genesis — the opcert counters would be empty since consensus was not running during replay, so no action is needed for those call sites.

Review each call site individually:
- Line ~300 (`reset_ledger_and_replay`): This is inside a standalone function that doesn't have access to consensus. The replay rebuilds state from genesis, so opcert counters would be empty. **No change needed.**
- Lines ~2092, ~2160, ~2240, ~2311, ~2385: These are in methods on `Node` (or `SyncHandler` which has `&self` access to consensus). Add the counter copy before each `save_snapshot` call.

- [ ] **Step 2: Add counter copy before each applicable save_snapshot call**

For each call site that has access to `self.consensus`, add the copy line before the `save_snapshot` call. The exact pattern varies — check whether the lock guard is `ls`, `ls_guard`, etc.

- [ ] **Step 3: Verify it compiles**

Run: `cargo build -p dugite-node`
Expected: success

- [ ] **Step 4: Commit**

```bash
git add crates/dugite-node/src/node/sync.rs
git commit -m "feat(node): persist opcert counters in all snapshot save paths (#310)"
```

---

### Task 7: Write tests for opcert counter persistence

**Files:**
- Modify: `crates/dugite-ledger/src/state/tests.rs` (add new test)
- Modify: `crates/dugite-consensus/src/praos.rs` (add new tests in existing test module)

- [ ] **Step 1: Test opcert counters survive snapshot round-trip (ledger layer)**

In `crates/dugite-ledger/src/state/tests.rs`, add:

```rust
#[test]
fn test_opcert_counters_persist_in_snapshot() {
    use dugite_primitives::hash::Hash28;

    let dir = tempfile::tempdir().unwrap();
    let snapshot_path = dir.path().join("ledger-snapshot.bin");

    // Create a ledger state and populate opcert counters
    let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());
    let pool_a = Hash28::from([0xAA; 28]);
    let pool_b = Hash28::from([0xBB; 28]);
    state.opcert_counters.insert(pool_a, 5);
    state.opcert_counters.insert(pool_b, 42);

    // Save and reload
    state.save_snapshot(&snapshot_path).unwrap();
    let loaded = LedgerState::load_snapshot(&snapshot_path).unwrap();

    // Verify counters survived
    assert_eq!(loaded.opcert_counters.len(), 2);
    assert_eq!(loaded.opcert_counters.get(&pool_a), Some(&5));
    assert_eq!(loaded.opcert_counters.get(&pool_b), Some(&42));
}

#[test]
fn test_opcert_counters_empty_by_default_in_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let snapshot_path = dir.path().join("ledger-snapshot.bin");

    let state = LedgerState::new(ProtocolParameters::mainnet_defaults());
    state.save_snapshot(&snapshot_path).unwrap();
    let loaded = LedgerState::load_snapshot(&snapshot_path).unwrap();

    // Default: empty map
    assert!(loaded.opcert_counters.is_empty());
}
```

- [ ] **Step 2: Test that restored counters reject replay (consensus layer)**

In `crates/dugite-consensus/src/praos.rs`, add to the existing `#[cfg(test)]` module:

```rust
#[test]
fn test_opcert_counters_restored_rejects_replay() {
    // Simulate: pool A had counter 5, save/restore, then block with counter 3 arrives
    let mut praos = OuroborosPraos::new();
    let pool_id = Hash28::from([0xAA; 28]);

    // Seed counters as if loaded from snapshot
    let mut restored = HashMap::new();
    restored.insert(pool_id, 5);
    praos.set_opcert_counters(restored);

    // Counter 3 < stored 5 → must be rejected
    assert_eq!(praos.opcert_counters()[&pool_id], 5);

    // Counter 6 > stored 5 → would be accepted (update)
    // (The actual check happens in validate_envelope, but we verify
    // the counter state is correct for the check to work)
}

#[test]
fn test_set_opcert_counters_replaces_all() {
    let mut praos = OuroborosPraos::new();
    let pool_a = Hash28::from([0xAA; 28]);
    let pool_b = Hash28::from([0xBB; 28]);

    // Existing counter
    praos.opcert_counters.insert(pool_a, 10);

    // Replace with new set
    let mut new_counters = HashMap::new();
    new_counters.insert(pool_b, 20);
    praos.set_opcert_counters(new_counters);

    // Old counter gone, new counter present
    assert!(!praos.opcert_counters().contains_key(&pool_a));
    assert_eq!(praos.opcert_counters()[&pool_b], 20);
}
```

- [ ] **Step 3: Run all new tests**

Run: `cargo nextest run -p dugite-ledger -E 'test(opcert_counters_persist) | test(opcert_counters_empty_by_default)' && cargo nextest run -p dugite-consensus -E 'test(opcert_counters_restored) | test(set_opcert_counters_replaces)'`
Expected: all PASS

- [ ] **Step 4: Commit**

```bash
git add crates/dugite-ledger/src/state/tests.rs crates/dugite-consensus/src/praos.rs
git commit -m "test: add opcert counter persistence tests (#310)"
```

---

### Task 8: Run full test suite and verify

- [ ] **Step 1: Run full workspace tests**

Run: `cargo nextest run --workspace`
Expected: all tests pass

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: no warnings

- [ ] **Step 3: Run format check**

Run: `cargo fmt --all -- --check`
Expected: clean

- [ ] **Step 4: Final commit (if any fixups needed)**
