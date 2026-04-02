---
name: RUPD snapshot position fix — 2.16× treasury divergence
description: Root cause and fix for the systematic 2.16× treasury inflation vs Koios canonical data on preview testnet genesis replay
type: project
---

## Root Cause

The RUPD (Reward Update) was computed from `self.snapshots.go` (epoch-2-ago data) instead of `self.snapshots.set` (epoch-just-ended data, = old mark after rotation).

At the first two epoch boundaries (0→1 and 1→2), `go` is None (the initial EpochSnapshots has all three as None, and go doesn't get populated until 2 rotations have occurred). Using go caused the RUPD to be skipped entirely for the first two epochs, losing 2 full cycles of monetary expansion. Over 1241 preview epochs with compounding, this produced ~2.16× treasury inflation.

## Canonical Verification (Koios preview testnet)

```
initial_reserves = 45T - 30T (Byron genesis) = 15_000_000_000_000_000
expansion(0→1)  = floor(0.003 × 15T × 4320/4320) = 45_000_000_000_000
treasury(0→1)   = floor(0.2 × (expansion + fees)) ≈ 9_000_000_000_000 (with active pools)
Koios epoch 1 treasury = 9_000_000_000_000  ← exact match
```

In the no-pool test path, all expansion goes to treasury = 45B. In the real chain with active pools, ~36B is distributed to stakers and only the tau cut = 9B enters treasury.

## Snapshot Timeline (After Fix)

```
Boundary 0→1: rotate → go=None, set=None; RUPD: set=None → skipped; treasury=0
              build mark1(fees=epoch0_fees, blocks=epoch0_blocks)
Boundary 1→2: rotate → go=None, set=mark1; RUPD fires from set=mark1 → treasury > 0
              build mark2(fees=epoch1_fees, blocks=epoch1_blocks)
Boundary 2→3: rotate → go=mark1, set=mark2; RUPD fires from set=mark2 → treasury grows
              ...
```

## Fix

In `/Users/michaelfazio/Source/dugite/crates/dugite-ledger/src/state/epoch.rs` `process_epoch_transition`:
- Changed `if let Some(go_snapshot) = self.snapshots.go.clone()` → `if let Some(set_snapshot) = self.snapshots.set.clone()`
- Passes `&set_snapshot` to `calculate_rewards()` instead of `&go_snapshot`

In `/Users/michaelfazio/Source/dugite/crates/dugite-ledger/src/state/rewards.rs` `calculate_rewards`:
- Renamed parameter from `go_snapshot` to `rupd_snapshot` throughout
- All internal references updated accordingly

## Why

**Why:** The Haskell NEWEPOCH rule fires the RUPD at each boundary using the epoch-just-ended snapshot data. After the mark→set→go rotation, that data is in the SET position (old mark). Using the GO position means 2-epoch-stale data, and at genesis the GO is None for the first two epochs, causing the first two RUPDs to be silently skipped.

**How to apply:** When adding epoch transition logic or tracing RUPD issues — always verify which snapshot the RUPD uses. The RUPD should use `self.snapshots.set` (after rotation = epoch just ended), NOT `self.snapshots.go` (after rotation = epoch 2 epochs prior).

## Tests Updated

- `test_epoch_fees_not_double_counted_through_snapshot_chain`: updated assertions (RUPD now fires at 1→2, not 2→3)
- `test_treasury_accumulates_at_correct_rate_no_double_counting`: updated assertions (treasury=0 only after 0→1, non-zero from 1→2)
- New: `test_rupd_fires_at_first_epoch_canonical_treasury` — verifies 45B treasury at 1→2 (no-pool)
- New: `test_rupd_compounding_treasury_over_three_epochs` — verifies monotonic compounding

## Stale Snapshot Problem (March 2026)

After the fix was committed (commit `ffe3604`, 2026-03-20 01:37 +0800), the live node snapshot at `db-preview/ledger-snapshot.bin` still showed 14.08T treasury (2.163× Koios 6.51T) because:

1. The fix was committed at 01:37 but the node process was still running the OLD binary
2. The snapshot was written at 01:48 by the old-binary node process (still running pre-fix)
3. The binary was never recompiled and restarted before the snapshot was saved

**Diagnostic**: loaded the snapshot via `LedgerState::load_snapshot()` and compared treasury to the theoretical no-pool maximum (14.64T = 96.2% of that). This confirms the treasury was accumulating as if almost NO pool rewards were being distributed (because the OLD GO snapshot had empty/stale stake data, triggering the `total_active_stake == 0` early-return that dumps all rewards to treasury).

**Resolution**: Delete `ledger-snapshot.bin` and replay from genesis (or Mithril import). The code is correct; all 582 ledger unit tests pass.

**Key invariant**: After this fix, if treasury > 10T on preview testnet, the snapshot was written by the old binary. Canonical maximum treasury (with ALL expansion to treasury, no pools) is ~14.64T at epoch 1241. Canonical actual treasury is 6.51T. Any value between 10T and 14.64T is a stale-snapshot artefact.
