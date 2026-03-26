---
name: chaindb-architecture
description: Complete ChainDB storage and chain selection architecture from ouroboros-consensus - components, data flow, invariants, fork handling
type: reference
---

# ChainDB Architecture (ouroboros-consensus 1.0.0)

Base path: `ouroboros-consensus/src/ouroboros-consensus/Ouroboros/Consensus/Storage/`

## Core Components

### ChainDbEnv (ChainDB/Impl/Types.hs:271-361)
- `cdbImmutableDB` - append-only storage
- `cdbVolatileDB` - recent blocks (all forks)
- `cdbLedgerDB` - ledger states (last k)
- `cdbChain` - `StrictTVar (InternalChain blk)` = current chain fragment
- `cdbChainSelQueue` - TBQueue + MultiSet for async block addition
- `cdbInvalid` - Map of known-invalid block hashes
- `cdbLoE` - Limit on Eagerness (Genesis)

### InternalChain (Types.hs:239-242)
Two parallel AnchoredFragments: with and without wall-clock times.

## Key Invariants
1. `AF.anchorPoint cdbChain == ImmutableDB tip`
2. All headers in cdbChain have blocks in VolatileDB
3. cdbChain CAN be > k blocks (bg thread trims); getCurrentChain returns at most k
4. LedgerSeq tip == cdbChain tip; LedgerSeq anchor == immutable tip
5. Single addBlockRunner thread does ALL chain selection (serialized)

## VolatileDB (VolatileDB/Impl/State.hs:93-113, Types.hs)
- `ReverseIndex`: Map (HeaderHash) -> InternalBlockInfo (file, offset, size)
- `SuccessorsIndex`: Map (ChainHash) -> Set (HeaderHash) -- parent->children
- `currentMaxSlotNo`: cached MaxSlotNo across all files
- File-level GC: delete entire file only when ALL blocks < gcSlotNo
- Losing fork blocks stay until file-level GC removes them

## LedgerDB (LedgerDB/V2/LedgerSeq.hs)
- `LedgerSeq`: AnchoredSeq of StateRef (ledger state + table handle)
- Anchor = immutable tip state; elements = volatile states
- `Forker`: independent handle for evaluating forks, then atomic commit
- `validateFork`: rollback N, apply headers, return ValidateResult
- Rollback bounded by k; exceeding returns ValidateExceededRollBack

## Chain Selection Flow (ChainSel.hs)
1. addBlockAsync -> enqueue to ChainSelQueue (TBQueue)
2. addBlockRunner dequeues, calls chainSelSync
3. chainSelSync: reject old/duplicate/invalid, write to VolatileDB, chainSelectionForBlock
4. chainSelectionForBlock: constructPreferableCandidates (via SuccessorsIndex), preferAnchoredCandidate, validate, switchTo
5. switchTo: atomic STM (write cdbChain + forkerCommit + notify followers)

## Copy to ImmutableDB (Background.hs:148-214)
- Background thread watches for cdbChain longer than getCurrentChain
- Copies oldest blocks: VolatileDB.getKnownBlockComponent -> ImmutableDB.appendBlock -> removeFromChain
- Triggers: LedgerDB flush/snapshot, VolatileDB GC schedule
- GC has delay (gcDelay ~60s) + batching (gcInterval ~10s)

## Fork Handling (Paths.hs)
- `isReachable`: walks backwards through VolatileDB to find intersection with current chain
- `maximalCandidates`: enumerates all maximal paths from a point via SuccessorsIndex
- `extendWithSuccessors`: extends a ChainDiff with all successor chains
- Max fork depth: exactly k blocks
