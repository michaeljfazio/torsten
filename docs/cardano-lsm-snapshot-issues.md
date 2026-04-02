# cardano-lsm: Snapshot Lifecycle Issues

> **RESOLVED (March 2026):** These issues are no longer relevant. Dugite's storage architecture was redesigned to match cardano-node: block storage now uses append-only chunk files (ImmutableDB) and an in-memory HashMap (VolatileDB), eliminating the cardano-lsm snapshot-from-snapshot lifecycle entirely. cardano-lsm is now used only for the on-disk UTxO set (UTxO-HD), where the snapshot lifecycle is simpler and does not exhibit these problems. This document is retained for historical reference.

## Important Caveat

**We may not be using cardano-lsm as intended.** The issues described below arise from our specific usage pattern — using `save_snapshot` / `open_snapshot` as a general-purpose persistence and recovery mechanism across process restarts. While the Haskell `lsm-tree` library (which cardano-lsm is based on) documents that "tables are ephemeral" and snapshots are the intended persistence mechanism, it's possible that cardano-lsm-rust has different design assumptions or that we're using the API incorrectly in some subtle way.

The suggested fixes in this document are our best guesses at what might resolve the issues, but they may be unnecessary or misguided if the root cause is simply that we're using the library incorrectly. We're writing this up to share our findings and get your perspective — whether that's "yes, these are bugs that should be fixed", "here's the correct way to do what you're trying to do", or something else entirely.

## Context

We're using `cardano-lsm` as the storage backend for [Dugite](https://github.com/michaeljfazio/dugite), a Rust implementation of the Cardano node. Our LSM tree stores all blockchain data — blocks, slot/hash indexes, and tip metadata — in a single `LsmTree`. The tree accumulates millions of entries over hours of sync (the Cardano preview testnet alone has ~4M blocks).

### How cardano-node Handles Persistence (For Reference)

In the Haskell cardano-node, chain storage is split into three layers:

1. **ImmutableDB** — finalized blocks in append-only chunk files (inherently durable on disk)
2. **VolatileDB** — recent blocks that may still roll back
3. **LedgerDB** — periodic snapshots of ledger state for fast restart

On restart, cardano-node loads the most recent valid ledger snapshot and replays only the small tail of blocks after it. This is fast because the ImmutableDB chunk files are always durable — they're append-only files that survive process crashes without any special snapshot mechanism.

The Haskell `lsm-tree` library is being developed as a future on-disk backend for the **UTxO set** (via UTxO-HD), not for block storage. Block storage remains in the ImmutableDB. The `lsm-tree` documentation explicitly states that tables are ephemeral and `saveSnapshot` / `openTableFromSnapshot` is the intended persistence mechanism.

### How Dugite Uses cardano-lsm

Our architecture differs from cardano-node: we use a single `LsmTree` for **block storage** (fulfilling the ImmutableDB + VolatileDB role), with separate bincode serialization for ledger state. This means our LSM tree contains millions of block entries that take hours to sync, and losing the tree means re-syncing from genesis.

Since `LsmTree::open()` creates an ephemeral tree and there is no WAL, we rely on `save_snapshot` / `open_snapshot` as the persistence mechanism — consistent with how the Haskell `lsm-tree` documents its snapshot API.

### Why We Need Snapshot Rotation

A naive approach of always saving to the same snapshot name is unsafe — if the process crashes mid-`save_snapshot`, the snapshot directory could be left in a partially-written state, corrupting the only recovery point. We use a write-ahead rotation pattern:

1. `save_snapshot("latest_tmp")` — write to a temporary name
2. Rename `latest` → `previous` — preserve the last known-good snapshot as backup
3. Rename `latest_tmp` → `latest` — atomically promote the new snapshot
4. Remove `previous` — clean up

This ensures that at any point during the persist, at least one valid snapshot exists for recovery. On startup, we try `latest` first, fall back to `previous`, then `latest_tmp`.

This rotation scheme is our own invention — if cardano-lsm has built-in crash safety guarantees for snapshots that make this unnecessary, we'd be happy to simplify.

### Why This Creates a "Snapshot-from-Snapshot" Pattern

After restart, the tree is loaded via `open_snapshot("latest")`. The node then continues syncing new blocks into this tree. When it's time to persist again, we call `save_snapshot("latest_tmp")` — creating a **new snapshot from a tree that was itself loaded from a snapshot**. This "snapshot-from-snapshot" cycle repeats indefinitely across node restarts.

This is where the issues below emerge. They make the snapshot-from-snapshot cycle unsafe, meaning a node that has been restarted even once can silently lose data on the next persist or compaction.

---

## What We've Observed

We've encountered data loss and snapshot corruption during the snapshot-from-snapshot lifecycle. We've attempted to diagnose the root cause and have identified two potential issues, but **we want to be upfront that our diagnosis may be incomplete or incorrect**. We noticed that `open_snapshot()` sets `no_delete = true` on loaded `SsTableHandle`s, which should prevent Drop from deleting snapshot files — so the path-aliasing problem we describe in Issue 1 may not actually occur as described, or may manifest differently than we think. We're presenting our observations and theories so you can evaluate them with full knowledge of the library's internals.

---

## Issue 1: Possible SsTableHandle Path Aliasing After Directory Rename

### What We Observed

After the snapshot rotation cycle (save to `latest_tmp`, rename `latest_tmp → latest`), subsequent attempts to open from the `latest` snapshot sometimes find missing or corrupted SSTable files.

### Our Theory

`SsTableHandle` stores a file path (e.g., `snapshots/latest/00001.blobs`). After `open_snapshot("latest")`, handles point into `snapshots/latest/`. When we later rotate `latest_tmp → latest`, the directory name `snapshots/latest/` now refers to different files. We theorized that when old handles are dropped, their `Drop` implementation deletes files at the stale path — which now points to the new snapshot's files.

### Uncertainty

We noticed that `open_snapshot()` sets `no_delete = true` on loaded handles, which should prevent this. It's possible that:
- The `no_delete` flag fully prevents this, and the corruption we're seeing has a different root cause
- The flag prevents deletion but the handles still hold stale path references that cause read errors
- There's an interaction with `save_snapshot` (which creates hard-linked handles with `no_delete = false`) that we're not accounting for
- Our directory rotation itself is the problem — perhaps cardano-lsm expects the caller to use `delete_snapshot` rather than external directory manipulation

We don't fully understand the interplay between `no_delete`, refcounting, and hard-linking across the save/open/rotate cycle, so this theory may be wrong.

### Reproduction (As We Understand It)

```rust
// 1. Open from snapshot
let tree = LsmTree::open_snapshot(path, "latest").unwrap();
// tree holds handles → snapshots/latest/ (no_delete=true)

// 2. Do some work, then persist
tree.save_snapshot("latest_tmp", "marker").unwrap();

// 3. Rotate directories (our persist pattern)
fs::rename("snapshots/latest", "snapshots/previous");
fs::rename("snapshots/latest_tmp", "snapshots/latest");
fs::remove_dir_all("snapshots/previous");

// 4. Drop the tree
drop(tree);
// Do handles with no_delete=true safely no-op here?
// Or does something else go wrong?

// 5. Try to recover
let tree2 = LsmTree::open_snapshot(path, "latest");
// Sometimes fails or returns incomplete data
```

---

## Issue 2: PersistentSnapshot::create Renumbers SSTables, Possibly Breaking Compaction Order

### What We Observed

After loading a snapshot and calling `compact_all()`, keys that were written in the most recent session before the snapshot are sometimes lost — the tree appears to regress to an earlier state. Specifically, our `meta:tip` key (which tracks the chain tip) reverts to a value from a previous session.

### Our Theory

`PersistentSnapshot::create` serializes the tree's levels into the snapshot by iterating levels `L0` through `L_max` and assigning new sequential run numbers starting from 1. Since **L0 contains the newest data** (recently flushed memtable) and higher levels contain older compacted data, after renumbering:

- L0's SSTable (newest data) gets `run_number = 1`
- L_max's SSTable (oldest data) gets `run_number = N`

When `compact_all()` is later called on the restored tree, compaction processes runs in ascending `run_number` order. If compaction resolves key conflicts by keeping the value from the higher run number (assuming higher = newer), the older data wins and the newer data is discarded.

### Context: Why We Call compact_all() After open_snapshot()

We added `compact_all()` after `open_snapshot()` as an attempted workaround for Issue 1 — to move SSTable data from `snapshots/` into `active/` so that handles no longer point into the rotatable snapshot directory. This workaround may have been unnecessary (if `no_delete=true` already prevents Issue 1), and it may be the actual cause of the data loss we're seeing.

### Reproduction

```rust
// Run 1: Create tree, add blocks at slots 100 and 200, persist
let tree = LsmTree::open(path, config).unwrap();
tree.insert(b"slot:100", block1).unwrap();
tree.insert(b"slot:200", block2).unwrap();
tree.insert(b"meta:tip", tip_200).unwrap();
tree.compact_all().unwrap();
tree.save_snapshot("latest", "test").unwrap();
// Snapshot has 1 SSTable (compacted) with slots 100, 200, tip=200

// Run 2: Load snapshot, add block at slot 300, persist
let tree = LsmTree::open_snapshot(path, "latest").unwrap();
tree.insert(b"slot:300", block3).unwrap();
tree.insert(b"meta:tip", tip_300).unwrap();
// After memtable flush: L0 has slot:300+tip=300 (newest), L1 has slots 100+200+tip=200 (oldest)
tree.save_snapshot("latest", "test").unwrap();
// PersistentSnapshot::create assigns:
//   L0 SSTable (newest) → run_number 1
//   L1 SSTable (oldest) → run_number 2

// Run 3: Load snapshot, compact
let tree = LsmTree::open_snapshot(path, "latest").unwrap();
tree.compact_all().unwrap();
// Compaction merges run 1 and run 2
// For meta:tip key: keeps run 2's value (tip=200) — newer value lost
// Result: tip = 200, slot 300's block is inaccessible
```

### Uncertainty

Again, we may be wrong about the compaction order semantics. It's possible that:
- Compaction correctly handles the renumbered SSTables and the data loss has a different cause
- The level assignment during `open_snapshot` preserves enough ordering information that `compact_all` works correctly
- We shouldn't be calling `compact_all()` after `open_snapshot()` at all (and wouldn't need to if Issue 1 doesn't actually occur)

---

## Combined Impact (As We Experience It)

In practice, we see this as a Cardano node operator:

1. **First run**: Node syncs for hours, persists, shuts down. Everything works.
2. **Second run**: Node restarts from snapshot, syncs more blocks, persists again.
3. **Third run**: Node restarts and either (a) finds corrupt/missing snapshot files, or (b) loads successfully but after compaction the chain tip has regressed, losing blocks from the second run.

Our attempted workarounds for one issue seem to trigger the other:

- **Without `compact_all()` after open**: We see snapshot file corruption after rotation
- **With `compact_all()` after open**: We see data loss (tip regression) from incorrect compaction merge order

But as noted above, we're not confident in our diagnosis of either issue. The `no_delete` flag may mean Issue 1 isn't real, in which case the `compact_all()` workaround was unnecessary, and Issue 2 would also not apply.

## What We're Hoping For

We'd greatly appreciate any of the following:

1. **"You're using it wrong"** — If `save_snapshot` / `open_snapshot` aren't meant for this lifecycle, what is the intended approach for durable persistence across process restarts? Is there a different API or pattern we should be using? Should we not be doing directory rotation, and instead use `delete_snapshot` for cleanup?

2. **"These are bugs, here's a fix"** — If the snapshot-from-snapshot pattern is supposed to work, we're happy to test fixes or contribute patches.

3. **"Here's a workaround"** — If there's a way to use the existing API that avoids both issues (something we haven't thought of), that would unblock us immediately.

4. **"Your diagnosis is wrong, here's what's actually happening"** — We'd be equally grateful for this. We've been debugging from the outside without deep knowledge of the library internals, and there's a good chance we've misidentified the root cause.

We're very happy with cardano-lsm's performance and design otherwise — it handles our 4M+ block workload well, and the compaction strategy is excellent. We just need to solve the persistence lifecycle to make the node production-ready.

## Environment

- `cardano-lsm` version: latest from git
- Platform: macOS (darwin) and Linux
- Rust: stable

## Test Case

We have a self-contained test (`test_snapshot_from_snapshot_is_self_contained` in our codebase) that reproduces the data loss. Happy to share it or adapt it for cardano-lsm's test suite.
