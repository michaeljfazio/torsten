---
name: cascade-failure-treasury-committee
description: Root cause and fix for the slot 107229218 cascade failure — TreasuryValueMismatch and UnelectedCommitteeMember hard-returns causing UTxO corruption and network fork
type: project
---

## Incident Summary

Block 4119211 (slot 107229218, preview testnet) caused Dugite to fork from the network.

**tx 26b1e945** had 15 Phase-1 errors:
- 12x InputNotFound for `f82ae6af#0–#11`
- CollateralNotFound for `f82ae6af#11`
- CollateralMismatch declared:1394819/computed:0
- InsufficientCollateral

The node logged "Phase-1 validation divergence on confirmed block — trusting on-chain consensus" and applied the block, but the UTxO set was already corrupted. The node then forged a block on this corrupted state, which was rejected by the entire network.

## Root Cause: TreasuryValueMismatch Cascade

A prior transaction in the chain contained `treasury_value` (Conway field 19) that disagreed with `state.treasury`. The original code hard-returned `Err(LedgerError::BlockTxValidationFailed { TreasuryValueMismatch })` from `apply_block`. This caused:

1. `apply_block` aborts WITHOUT inserting the block's outputs (`f82ae6af#0–#11`) into the UTxO store
2. The sync loop `break`s on the error
3. On next batch, the gap-bridge replays in `ApplyOnly` mode (skipping the treasury check) — but the outputs remain missing
4. At-tip `ValidateAll` mode fires the same error on reconnect
5. `f82ae6af` outputs never appear in the UTxO store
6. `26b1e945` (which spends them) gets InputNotFound — even though it was confirmed on-chain

The same pattern existed for `UnelectedCommitteeMember` (CommitteeHotAuth cert for a cold credential not in our committee_expiration map).

## Fix (committed d3443a2)

**apply.rs**: Changed both `TreasuryValueMismatch` and `UnelectedCommitteeMember` from hard `return Err(...)` to `warn!() + self-correct + fall through`:

```rust
// TreasuryValueMismatch: was return Err, now:
warn!("TreasuryValueMismatch on confirmed block — trusting on-chain consensus");
self.treasury = declared_treasury; // self-correct

// UnelectedCommitteeMember: was return Err, now:
warn!("UnelectedCommitteeMember on confirmed block — trusting on-chain consensus");
// fall through — process cert normally
```

**sync.rs**: Slow-path rollback was also broken — it re-attached the pre-rollback UTxO store (which contained stale entries from rolled-back blocks). Fixed to open a fresh UTxO store from the "ledger" LSM snapshot.

## Key Invariant

For confirmed on-chain blocks, `apply_block` MUST NEVER hard-return `Err` for ledger-state-divergence checks (treasury, committee membership). These checks are valid for mempool admission (reject new txs) but for confirmed blocks, on-chain consensus is authoritative. The correct response is: log at WARN, self-correct our state, and fall through.

The treasury check is gated by `mode == BlockValidationMode::ValidateAll` — it does NOT fire during bulk replay (ApplyOnly). Both regression tests correctly use ValidateAll to exercise the failure path.

## Regression Tests (committed 519ad41)

Four tests in `crates/dugite-ledger/src/state/tests.rs`:
- `test_treasury_mismatch_does_not_abort_apply_block`
- `test_treasury_mismatch_no_cascade_in_downstream_block`
- `test_unelected_committee_member_does_not_abort_apply_block`
- `test_unelected_committee_member_no_cascade_in_downstream_block`

## Why: Impact

Without this fix, any treasury tracking divergence (reward rounding, missed treasury donation, or UTxO gap in a prior era) would permanently corrupt the UTxO set from that point forward, causing the node to fork from the network and potentially forge blocks on a minority chain.
