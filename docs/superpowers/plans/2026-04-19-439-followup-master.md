# #439 Follow-Up Master Plan — Resumption Guide

> **For agentic workers:** This is the top-level index for resuming work on issue #439's follow-ups. Start here. The actual bite-sized tasks live in the three linked sprint plans. Use `superpowers:subagent-driven-development` or `superpowers:executing-plans` against each sprint plan in order.

**Goal:** Deliver the architectural + hygiene work uncovered during #439's investigation so dugite's block producer operates stably at live tip, with Haskell-aligned semantics for ledger rollback, snapshots, and chain selection.

**Architecture:** Three sequential sprints. Sprint 1 is mechanical cleanup (~1 week). Sprint 2 makes `LedgerSeq` the authoritative ledger state with O(n) in-memory rollback (~2 weeks). Sprint 3 unifies `ChainDB` + `LedgerState` + `LedgerSeq` into a single atomically-committed `ChainLedger` (~2 weeks). Each sprint produces a working, testable node.

**Tech Stack:** Rust 1.95, pallas-primitives 1.0.0-alpha.6, tokio, tracing, cardano-ledger 8+ conformance, Ouroboros Praos per `IntersectMBO/ouroboros-consensus`.

---

## 0. Context for fresh sessions

### 0.1 What #439 was, what is fixed

The filed bug: **forged blocks never landed on chain.** Three blocks forged by SAND (`pool1l704ukj3qtyxljsduccqkv3t24gh9hfqdarhreffw5na2uknf5k`, hex `ff9f5e5a5102c86fca0de6300b322b555172dd206f4771e5297527d5`) in epoch 1270 were all orphaned — Koios showed three different pools producing blocks at the expected heights.

Root cause was actually two stacked defects:

1. `apply_block` had a hash-mismatch bypass that accepted any block whose `block_number == tip.block_number + 1` regardless of `prev_hash`. This silently fused two chains' UTxO state whenever upstream delivered the next block of a competing chain, masking fork switches.
2. `SwitchedToFork` handlers in `sync.rs::process_forward_blocks` and `mod.rs::apply_fetched_block` were `Phase 3 pending` no-ops — chain selection's fork decision was never propagated to the ledger.

Commits landed so far:

| SHA | State | Purpose |
|---|---|---|
| `b9ac790b0` | pushed to main | Original #439 fix: `forged_is_tip` check, `ApplyOnly`-gated bypass, sync.rs `SwitchedToFork` → `handle_rollback`, 2 new metrics, subscriber-count logging |
| `cd3d03a92` | **local, needs push** | Follow-up: live-tip `SwitchedToFork` handler in `mod.rs::apply_fetched_block` |
| `3b86b75a4` | **local, needs push** | Haskell-aligned fork semantics: `SwitchPlan.intersection_slot`, unreachable-fork = `StoreButDontChange`, removed `Point::Origin` fallback |

To push the two local commits: refresh SSH agent (new terminal or `ssh-add`), then `git push origin main`. Remote URL already updated to `git@github.com:michaeljfazio/dugite.git`.

### 0.2 What was uncovered during investigation

Runtime validation on preview testnet exposed deeper architectural issues that don't manifest in unit tests. Summary:

| # | Item | Status | Sprint |
|---|---|---|---|
| 1.1 | `handle_rollback` uses snapshot-reload+replay (slow), should use `LedgerSeq` | Unfixed | 2 |
| 1.2 | Ledger snapshots captured at volatile tip, poison restart recovery | Unfixed | 2 |
| 1.3 | `ChainDB` and `LedgerState` updates not atomically coupled | Unfixed | 3 |
| 2.1 | Hash-mismatch bypass at live tip | Fixed `b9ac790b0` | — |
| 2.2 | `VolatileDB.switch_chain` fabricated anchors for unreachable forks | Fixed `3b86b75a4` | — |
| 2.3 | Missed `SwitchedToFork` handler in `apply_fetched_block` | Fixed `cd3d03a92` | — |
| 2.4 | `Point::Origin` fallback when intersection slot missing | Fixed `3b86b75a4` | — |
| 3.1 | `AdoptedAsTip` dead code, `StoredNotAdopted` overloaded | Unfixed | 1 |
| 3.2 | `ApplyOnly` bypass too broad — only Byron needs it | Unfixed | 1 |
| 3.3 | Leader election audit (confirmed correct, see 0.4) | Audit-only | 1 |
| 3.4 | `n2n_connections_active` gauge inaccurate | Unfixed | 1 |
| 3.5 | Ledger write-lock contention under rollback churn | Auto-fixed by 1.1 | 2 |
| 4.1 | Forge-2 orphaning cause | Investigated 2026-04-19, legitimate slot-battle loss | — |
| 4.2 | Two commits unpushed (SSH agent) | Admin, needs user action | — |

### 0.3 Haskell authoritative references (consulted this session)

All claims cross-checked against `IntersectMBO/ouroboros-consensus` via the cardano-haskell-oracle. Canonical citations:

| Concern | Module | Semantics |
|---|---|---|
| AddBlockResult shape | `Storage/ChainDB/API.hs:~515` | Carries the new tip `Point blk`, not a bespoke plan enum |
| Fork switch plan | `Storage/ChainDB/Impl/Paths.hs:~55` | `ChainDiff b = { getRollback :: Word64, getSuffix :: AnchoredFragment b }`; anchor carries `(SlotNo, HeaderHash, BlockNo)` |
| Reachability invariant | `Paths.hs::isReachable` | Walk VolatileDB backward; if exits window without finding current chain → `Nothing` → `StoreButDontChange` |
| Impossible rollback | `ChainSel.hs:~1273` | Comment: "impossible: we asked the LedgerDB to roll back past the immutable tip, which is impossible, since the candidates we construct must connect to the immutable tip" |
| Atomic commit | `ChainSel.hs::switchTo` (line ~896) | Single `atomically` block: `writeTVar cdbChain newChain; forkerCommit forker` |
| LedgerDB rollback | `LedgerDB/Forker.hs:~379` | `withForkerAtFromTip numRollbacks` + `applyThenPushMany` + `forkerCommit`. `Word64` rollback count only. |
| Ledger snapshot policy | `Impl/Snapshots.hs::takeSnapshotThread` | Snapshots written from the ImmutableDB-tip anchor state, never from volatile-tip live state |
| Header validation | `HeaderValidation.hs:~359 validateEnvelope` | `checkPrevHash'` strict equality, called unconditionally on both `ApplyVal` and `ReapplyVal` |
| Praos leader | `Protocol/Praos.hs:403-425 checkIsLeader` | Consults `lvPoolDistr`, sourced from `nesPd` = Mark snapshot taken at epoch E−1 boundary (i.e. pool registered in epoch N is leader-eligible from N+1, rewards from N+2) |
| Slot battle tiebreak | `Protocol/Praos/Common.hs::comparePraos` | Lower raw VRF output wins; Conway uses `RestrictedVRFTiebreaker maxDist`: only fires when `|Δslot| ≤ maxDist`, else EQ |
| Selection rule | `Ouroboros.Consensus.Protocol.TPraos` | Compare `svBlockNo` first (longest chain), then `PraosTiebreakerView` (VRF with Restricted flavor), then GDD/LoE |

### 0.4 Pallas authoritative references

Cross-checked via the pallas-advisor (pallas 1.0.0-alpha.6):

| Concern | Type/module | Observation |
|---|---|---|
| Raw byte preservation | `pallas-codec::utils::KeepRaw<'b, T>` | Holds `Cow<'b, [u8]>` + decoded `T`; `raw_cbor()` returns original slice. DerefMut clears raw — bug if used before hashing. |
| Block hash (Shelley+) | `pallas-traverse::hashes::OriginalHash<32>` | `Hasher::<256>::hash(self.raw_cbor())` — correct, uses original bytes |
| **Block hash (Byron)** | Same trait, Byron specialization | **Re-encodes: `Hasher::<256>::hash_cbor(&(tag, self))`**. Source of dugite's chunk-replay hash mismatches. Fix is upstream. |
| Dugite's `Block::hash()` | `crates/dugite-primitives/src/block.rs:153` | Returns pre-computed `header_hash` captured at decode time from `Hasher::hash(header_raw_cbor)`. Correct for Shelley+, broken for Byron. |

**Consequence:** the `ApplyOnly` sequence-number bypass in `apply.rs` is only justified for Byron blocks. Sprint 1 Task 1 narrows the bypass accordingly.

### 0.5 Current node state (for re-validation)

Pool: SAND (`pool1l704ukj3qtyxljsduccqkv3t24gh9hfqdarhreffw5na2uknf5k`)

Stake (epoch 1271):
- Go @ 1270: 1,009,459,156,006 lovelace
- Set @ 1271: 1,009,348,335,206 lovelace
- Mark @ 1272: 1,009,348,335,206 lovelace
- σ ≈ 0.000789
- Expected forges/epoch: 0.05 × 0.000789 × 86400 ≈ **3.4**

Preview params: `active_slot_coeff = 0.05`, `epoch_length = 86400 slots`, `k = 432`.

Database (`db-preview/`):
- `ledger-snapshot-epoch1271-slot109855480.bin` — clean (pre-orphan) snapshot
- `ledger-snapshot-epoch1271-slot109856927.bin` — poisoned, contains orphan state from forge-1 run
- `ledger-snapshot.bin` — latest; poisoned as of last run

Before re-validating, delete the poisoned snapshots manually until item 1.2 lands.

### 0.6 Sprint plan overview

**Sprint 1 — Hygiene and quick wins (~1 week)**
Plan: [`2026-04-19-sprint-1-hygiene.md`](2026-04-19-sprint-1-hygiene.md)
Blockers lifted: none (purely additive or narrowly scoped)
Tasks:
- Task 1: Narrow `ApplyOnly` bypass to Byron era (3.2)
- Task 2: Rename `AdoptedAsTip` → `AddedAsTip` and distinguish from `StoredAsFork` (3.1)
- Task 3: Fix `n2n_connections_active` gauge (3.4)
- Task 4: Audit leader election against Haskell (3.3) — documentation only
- Task 5: Audit/PR to pallas for Byron `OriginalHash` re-encode (3.2, upstream)
- Task 6: Add follow-up tests for 2.x correctness defects (2.1-2.4)

**Sprint 2 — LedgerSeq authoritative (~2 weeks)**
Plan: [`2026-04-19-sprint-2-ledger-seq-authoritative.md`](2026-04-19-sprint-2-ledger-seq-authoritative.md)
Blockers lifted: slow rollback, poisoned snapshot recovery (1.1, 1.2), write-lock contention (3.5)
Tasks:
- Task 1: Extend `LedgerDelta` with inversion data (prev_tip, prev_nonces, prev_*_diff)
- Task 2: Implement `LedgerState::reverse_apply(&LedgerDelta)`
- Task 3: Capture inversion data during `apply_block`
- Task 4: Rewrite `Node::handle_rollback` to use `LedgerSeq::rollback(n)`
- Task 5: `ExceedsRollback` error path (matches Haskell's `ExceededRollback`)
- Task 6: Property tests: apply→rollback round-trip equivalence
- Task 7: Snapshot strategy: write only on anchor-advance (ImmutableDB-tip boundary)
- Task 8: Startup path: load snapshot as anchor; walk VolatileDB forward
- Task 9: Integration test: induce peer rollback under load, verify < 10ms

**Sprint 3 — `ChainLedger` atomic commits (~2 weeks)**
Plan: [`2026-04-19-sprint-3-chain-ledger-atomic.md`](2026-04-19-sprint-3-chain-ledger-atomic.md)
Blockers lifted: ChainDB / LedgerState divergence window (1.3)
Tasks:
- Task 1: New `ChainLedger` struct wrapping chain_db + ledger_state + ledger_seq
- Task 2: `ChainLedger::apply_block`, `::rollback`, `::switch_fork` unified methods
- Task 3: Migrate `chain_sel_queue` from mutate-in-place to return-plan
- Task 4: Single critical section in `apply_fetched_block` and `process_forward_blocks`
- Task 5: Migrate forge path to `ChainLedger::try_forge`
- Task 6: Deadlock detection (loom or manual stress)
- Task 7: Concurrent-readers invariant tests

### 0.7 Verification criteria (end-to-end)

After Sprint 3, run this acceptance protocol:

1. **Clean-state sync** — `cargo build --release`; `rm db-preview/ledger-snapshot*.bin db-preview/volatile/volatile-wal.bin`; `scripts/run-bp-preview.sh --log /tmp/bp.log`; verify `tip_age_seconds < 180` within 15 min.
2. **Rollback storm resilience** — monitor for 2 hours; verify:
   - `tip_age_seconds` stays < 180 throughout
   - `Accepting block by sequence number` count remains 0 for non-Byron blocks
   - Lock-hold-time p99 < 10ms (needs histogram instrumentation from Sprint 2 Task 9)
3. **Forge + Koios-confirm** — run until first leader slot; verify the forged block appears on Koios `pool_blocks` for `pool1l704ukj...` within 2 minutes of forge log timestamp.
4. **Crash recovery** — SIGKILL the node mid-forge; restart; verify tip recovers to ImmutableDB tip within 30 seconds (not stuck at orphan).
5. **Full workspace tests** — `cargo nextest run --workspace --no-fail-fast` — 2 pre-existing PV10 failures expected (commit `9a631979e`), all else green.

### 0.8 Non-goals for this plan

- Protocol-parameter updates (governance): orthogonal.
- Mithril integration changes: orthogonal.
- N2C query layer: orthogonal.
- Performance tuning of BlockFetch pipeline: orthogonal (already measured at ~6 blocks/sec in live-tip catch-up).
- Any code in `crates/dugite-cli`, `crates/dugite-monitor`, `crates/dugite-config`, `crates/dugite-lsm`.

### 0.9 Preferred execution mode per sprint

- Sprint 1: Subagent-driven. Each task is < 1 day and independent.
- Sprint 2: Inline execution with checkpoints. Tasks are tightly coupled (delta shape → reverse_apply → rewire); review between them.
- Sprint 3: Inline, paired with loom/stress test results between steps. Deadlock risks mean reviews must be sharp.

---

## 1. Execution handoff

Start with Sprint 1: [`2026-04-19-sprint-1-hygiene.md`](2026-04-19-sprint-1-hygiene.md).

Do not begin Sprint 2 until Sprint 1 commits are pushed and CI is green.

Do not begin Sprint 3 until Sprint 2 soak-tests pass for 24h on preview.

After each sprint, return to this master doc to update Section 0.2's status table. This is the resumption guarantee — the master doc always reflects current truth.
