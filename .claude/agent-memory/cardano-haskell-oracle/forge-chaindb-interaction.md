---
name: forge-chaindb-interaction
description: How forged blocks enter ChainDB, chain selection for competing blocks, rollback mechanism, forge/sync concurrency
type: reference
---

# Forged Block → ChainDB Interaction

## Key Files
- `ouroboros-consensus-diffusion/src/.../Consensus/NodeKernel.hs` — `forkBlockForging`, `go`, `mkCurrentBlockContext`
- `ouroboros-consensus/src/.../Storage/ChainDB/Impl/ChainSel.hs` — `addBlockAsync`, `chainSelSync`, `chainSelectionForBlock`, `switchTo`, `chainSelection`
- `ouroboros-consensus/src/.../Storage/ChainDB/Impl/Background.hs` — `addBlockRunner` (single background thread)
- `ouroboros-consensus/src/.../Storage/LedgerDB/Forker.hs` — `validate`, `switch` (rollback + reapply)
- `ouroboros-consensus/src/.../Protocol/Praos/Common.hs` — `comparePraos`, `PraosTiebreakerView`

## Flow
1. Forged block submitted via `ChainDB.addBlockAsync chainDB noPunish newBlock` (line 750 of NodeKernel.hs)
2. Block enters `cdbChainSelQueue` (TBQueue) — same queue used by blocks from peers
3. Single `addBlockRunner` thread processes queue sequentially
4. Block stored in VolatileDB first (`VolatileDB.putBlock`), then `chainSelectionForBlock` triggered
5. `blockProcessed` TMVar filled with new tip (or not-adopted result)
6. Forge loop blocks on `atomically $ ChainDB.blockProcessed result`

## Forged blocks not adopted
- Check: `mbCurTip /= SuccesfullyAddedBlock (blockPoint newBlock)`
- If not adopted and not invalid: `TraceDidntAdoptBlock` trace, exitEarly
- If invalid: `TraceForgedInvalidBlock`, removes txs from mempool, exitEarly
- No special handling — forged blocks treated identically to peer blocks in ChainDB

## Chain selection tiebreaker (same slot)
- `mkCurrentBlockContext` EQ case: forges alternative block with same blockNo, same predecessor
- Chain selection: `comparePraos` → compare OCert issue number, then VRF tiebreaker
- Lower VRF output wins (`Down` ordering)
- RestrictedVRFTiebreaker (Conway): only compare VRF if slot distance <= maxDist

## Rollback mechanism
- NOT a full chain replay from genesis
- LedgerDB keeps last k ledger states (LedgerSeq/AnchoredSeq)
- `switch` function: `forkerAtFromTip rr numRollbacks` → rollback to intersection, then apply new blocks
- `applyThenPushMany` applies blocks one-by-one on the forker
- Previously validated blocks (in `prevApplied` set) use `Reapply` (skip validation); new blocks use `Apply` (full validation)
- Max rollback depth: k (SecurityParam, 2160 on mainnet) — same limit for both local and peer blocks

## Forge/ChainSync concurrency
- Forging runs in its own thread (`forkBlockForging`), triggered by `knownSlotWatcher` on every slot
- ChainSync runs independently in per-peer threads
- Both submit blocks to the same ChainSelQueue (TBQueue)
- Single `addBlockRunner` serializes all chain selection — no concurrent chain selection
- Forging does NOT stop ChainSync; they are fully concurrent but serialized at the queue level
