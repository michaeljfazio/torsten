---
name: fork-switching-mechanism
description: Complete analysis of how cardano-node handles slot battles, fork switching, rollbacks, VolatileDB retention, and the immutable/volatile tip boundary
type: reference
---

# Fork Switching Mechanism in ouroboros-consensus

## Key Files
- ChainSel: `ouroboros-consensus/src/.../Storage/ChainDB/Impl/ChainSel.hs`
- Background: `ouroboros-consensus/src/.../Storage/ChainDB/Impl/Background.hs`
- Paths: `ouroboros-consensus/src/.../Storage/ChainDB/Impl/Paths.hs`
- Follower: `ouroboros-consensus/src/.../Storage/ChainDB/Impl/Follower.hs`
- Types: `ouroboros-consensus/src/.../Storage/ChainDB/Impl/Types.hs`
- Query: `ouroboros-consensus/src/.../Storage/ChainDB/Impl/Query.hs`
- Fragment.Diff: `ouroboros-consensus/src/.../Fragment/Diff.hs`
- VolatileDB API: `ouroboros-consensus/src/.../Storage/VolatileDB/API.hs`
- LedgerDB API: `ouroboros-consensus/src/.../Storage/LedgerDB/API.hs`
- SecurityParam: `ouroboros-consensus/src/.../Config/SecurityParam.hs`
- ChainSync Server: `ouroboros-consensus/src/.../MiniProtocol/ChainSync/Server.hs`

## Architecture Summary
- `cdbChain` (TVar): full internal chain, may temporarily exceed k blocks
- `getCurrentChain`: returns last k headers (volatile suffix), anchored at immutable tip
- VolatileDB: stores ALL recent blocks (including orphaned forks), on-disk files
- ImmutableDB: append-only chain of finalized blocks
- LedgerDB: maintains last k ledger states for rollback support
- `addBlockRunner`: single background thread processes ChainSelQueue sequentially
- `copyToImmutableDBRunner`: moves blocks from cdbChain prefix to ImmutableDB
- GC: delayed (60s default), removes VolatileDB blocks by slot number `<` threshold

## Fork Switching Flow
1. Block B' arrives via ChainSync client -> added to ChainSelQueue
2. addBlockRunner calls chainSelSync -> stores in VolatileDB via putBlock
3. chainSelectionForBlock constructs candidate ChainDiffs via Paths.isReachable
4. ChainDiff = { rollback: Word64, suffix: AnchoredFragment }
5. Candidates sorted by compareAnchoredFragments, validated via LedgerDB.validateFork
6. switchTo atomically: writes cdbChain, commits forker, updates followers
7. Trace event: SwitchedToAFork (rollback > 0) or AddedToCurrentChain (rollback == 0)

## Follower RollBack Delivery
- switchTo calls fhSwitchFork on each follower when rollback > 0
- switchFork computes orphaned suffix, sets FollowerInMem(RollBackTo intersection)
- Next instructionSTM call returns RollBack(pt) then transitions to RollForwardFrom(pt)
- ChainSync server delivers MsgRollBackward to downstream peers

## VolatileDB Retention
- Old fork blocks NOT immediately deleted; kept until GC
- GC driven by slot number: `garbageCollect slotNo` removes blocks with slot < slotNo
- GC delay: 60s default, interval: 10s batching
- Forged blocks that lose slot battles remain in VolatileDB until GC
- VolatileDB tracks successors map (predecessor -> set of successors) in memory

## Max Rollback = k
- SecurityParam = NonZero Word64, represents max rollback depth
- Mainnet: k=2160, Preview: k=2160 (same)
- LedgerDB maintains k ledger states; rollback > k fails with ValidateExceededRollBack
- Immutable tip = block at depth k from chain tip
- Only volatile (last k) blocks can participate in fork switching
