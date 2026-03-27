---
name: Fork snapshot stall cascade fix
description: Six-bug cascade triggered by fork ledger snapshot on restart — root causes and fixes
type: project
---

A restart after unclean shutdown can leave the node permanently stalled if
the last saved snapshot has a tip on a fork chain.  Six bugs compose into
the failure:

**RC1 (primary): Fork snapshot** — Torsten snapshots at volatile ledger tip
(can be a fork block).  Haskell only snapshots at the ImmutableDB-confirmed
anchor.

**RC2: Intersection candidates** — When `chain_diverged=true`, the code sent
only 8 deep-historical sparse points instead of including the ImmutableDB tip,
causing a 97K-slot / ~4980-block rollback to a deep historical point.

**RC3: Deep rollback** — Rollback of ~4980 blocks exceeds the DiffSeq k=432
window; slow path (snapshot reload + replay) runs.

**RC4: All snapshots are fork snapshots** — Both retained epoch snapshots were
also saved on the fork; `find_best_snapshot_for_rollback` returns `None`.

**RC5: reset_ledger_and_replay corrupts UTxO store** — Re-attached the stale
fork UTxO store (2.9M fork UTxOs) before genesis replay, permanently
corrupting state.  apply_block fails silently forever after.

**RC-E: LSM replay broken for ImmutableDB blocks** — `replay_from_lsm` used
`get_block_by_number()` (VolatileDB only, empty after restart).  Should use
`get_next_block_after_slot()` which queries both ImmutableDB and VolatileDB.

## Fixes applied (commit 1ff9cbce)

1. **Fix 2 (first)**: `reset_ledger_and_replay` drops the stale fork UTxO
   store instead of re-attaching it.  Adds post-replay snapshot save.

2. **Fix 1**: On startup, if snapshot slot <= ImmutableDB tip slot, verify
   the hash matches the canonical ImmutableDB block.  Hash mismatch = fork
   snapshot = discard and fall back to genesis.

3. **Fix 3**: `chain_diverged` branch now offers ImmutableDB tip first, then
   deep historical points.  `get_immutable_tip_point() -> Option<Point>` added
   to `ChainDB`.

4. **Fix 4**: `replay_from_lsm` replaced block-number loop with
   `get_next_block_after_slot()` slot-based loop.

5. **Fix 5**: Deep rollback with no canonical snapshot found — warn and return
   instead of calling `reset_ledger_and_replay`.

6. **Fix 6**: `find_best_snapshot_for_rollback` takes optional `chain_db`,
   verifies each candidate via `is_snapshot_canonical()`.

## Key invariant

`is_snapshot_canonical(snap_slot, tip_point, chain_db)`: if snap_slot <=
imm_tip_slot, check ImmutableDB hash at that slot matches.  If snap_slot >
imm_tip_slot (volatile region), provisionally accept.

**Why:** Matches Haskell's snapshot-at-ImmutableDB-anchor invariant.  A fork
snapshot as base state corrupts every subsequent apply_block permanently.
