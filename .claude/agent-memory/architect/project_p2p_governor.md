---
name: P2P Governor Upgrade Architecture
description: Five-phase design for upgrading PeerManager/Governor to full Ouroboros peer selection (issue #138)
type: project
---

Five-phase plan to upgrade `governor.rs` / `peer_manager.rs` to proper Ouroboros P2P governor behaviour.

**Why:** Issue #138 — current implementation is a functional scaffold. Two high-severity silent bugs exist: cold BLPs are never connected proactively, and per-group local-root valency is not enforced. The rest of the gaps cause suboptimal peer rotation and gossip growth.

**How to apply:** Use this plan when implementing #138. Start with Phase 1 (BLP connect + local-root valency) — it is the highest-impact and smallest change. Each phase is independently committable.

## Phase Summary

| Phase | Focus | Effort | Key Files |
|---|---|---|---|
| 1 | BLP connect events + local-root valency | 2-3 days | governor.rs, peer_manager.rs, config.rs, node/mod.rs |
| 2 | Formal state machine + transition timeouts | 4-5 days | peer_manager.rs, governor.rs, client.rs, node/mod.rs |
| 3 | Churn: randomised, no re-selection, local-root exempt | 2-3 days | governor.rs only |
| 4 | Governor-driven peer sharing requests | 3-4 days | governor.rs, peer_manager.rs, node/mod.rs |
| 5 | Known-peer target enforcement + BLP preemption during sync | 3-4 days | governor.rs, peer_manager.rs, node/mod.rs |

## Top Two Silent Bugs (Phase 1 Fixes)

1. `evaluate_blp()` in `governor.rs` never emits `GovernorEvent::Connect` for cold BLPs — it has a comment instead. Cold BLPs are never proactively connected during sync.
2. `localRoots` topology groups have a `valency` field (minimum connections) that is parsed in JSON but ignored. Only a boolean `is_local_root` flag is tracked per peer.

## Architecture Constraints

- All changes stay within `dugite-network`; `dugite-node` interacts only via `GovernorEvent` and `PeerManager` public API.
- `PeerManager` is `Arc<RwLock<..>>` — governor must hold write lock only for event application, never during I/O.
- Phase 4 peer-sharing work must be fire-and-forget spawned tasks, not inline awaits.

## Full Document

`docs/src/architecture/p2p-governor.md`
