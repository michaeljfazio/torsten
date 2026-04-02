---
name: LoE enforcement wired into block pipeline
description: T3-4 — how the Limit on Eagerness constraint is plumbed from the GSM into flush_to_immutable
type: project
---

LoE is enforced by gating the volatile-to-immutable flush in `process_forward_blocks()`.

## Architecture

- `GenesisStateMachine::loe_limit(&[Point]) -> Option<u64>` — already existed in gsm.rs; returns None when CaughtUp (no constraint), Some(min_peer_tip_slot) when Syncing, Some(0) when PreSyncing.
- `ChainDB::flush_to_immutable_loe(loe_slot: u64)` — new method in dugite-storage; behaves like `flush_to_immutable()` but skips blocks with slot > loe_slot, leaving them in VolatileDB.
- `Node.gsm: Arc<RwLock<GenesisStateMachine>>` — added as a struct field; initialized in `Node::new()` so the background evaluation task (spawned in `run()`) and `process_forward_blocks()` share one instance.

## Integration Point

In `process_forward_blocks()` (node.rs ~line 3566), before flushing:
1. Read `self.gsm.read().await.loe_limit(&[tip.point.clone()])`
2. If `None` → call `flush_to_immutable()` (normal Praos path, zero overhead)
3. If `Some(loe_slot)` → call `flush_to_immutable_loe(loe_slot)` (Genesis path)

## Key Invariant

Blocks beyond the LoE slot remain in VolatileDB and are still applied to the ledger; only the immutable advancement is gated. When the GSM later transitions to CaughtUp, the next batch calls the unconstrained flush which drains the backlog.

## Tests Added

- `test_flush_to_immutable_loe_caps_flush` — verifies only blocks within the slot ceiling are moved
- `test_flush_to_immutable_loe_zero_blocks_all` — verifies PreSyncing (loe=0) blocks all immutable advancement

**Why:** PreSyncing/Syncing: loe_limit() always returns None when genesis is disabled (GSM starts in CaughtUp), so there is strictly zero overhead on the normal Praos hot path.
**How to apply:** Any future change to the volatile-to-immutable flush must preserve the LoE gating logic in process_forward_blocks.
