---
name: Haskell Mempool Implementation Details
description: Complete analysis of cardano-node mempool ordering, data structures, capacity, fairness, revalidation, and block production snapshotting
type: reference
---

## Key Files
- `ouroboros-consensus/src/.../Mempool/API.hs` — Public interface, Mempool record type, MempoolSnapshot, fairness docs
- `ouroboros-consensus/src/.../Mempool/TxSeq.hs` — StrictFingerTree data structure, TxTicket, splitting by size/ticket
- `ouroboros-consensus/src/.../Mempool/Impl/Common.hs` — InternalState (IS), validation, revalidation, snapshotFromIS
- `ouroboros-consensus/src/.../Mempool/Update.hs` — implAddTx (fairness MVars), doAddTx, pureTryAddTx, implSyncWithLedger
- `ouroboros-consensus/src/.../Mempool/Capacity.hs` — MempoolCapacityBytesOverride, computeMempoolCapacity (2x block capacity)
- `ouroboros-consensus/src/.../Mempool/Init.hs` — openMempool, background sync watcher thread
- `ouroboros-consensus/src/.../Mempool/Query.hs` — implGetSnapshotFor (forging snapshot)
- `ouroboros-consensus/src/.../Ledger/SupportsMempool.hs` — TxLimits class, TxMeasure, LedgerSupportsMempool class
- `ouroboros-consensus-cardano/src/shelley/.../Mempool.hs` — ConwayMeasure (bytes+ExUnits+refScripts), AlonzoMeasure
- `ouroboros-consensus-diffusion/src/.../NodeKernel.hs` — Block forging uses getSnapshotFor + snapshotTake

## Core Design: FIFO Ordering, NO Fee-Based Prioritization
- Mempool is a **strict FIFO list** (not fee-ordered)
- Transactions validated in insertion order against cumulative ledger state
- No fee/size ratio sorting, no priority queue
- Block production takes **longest valid prefix** that fits in block capacity

## Data Structure: StrictFingerTree
- `TxSeq sz tx = StrictFingerTree (TxSeqMeasure sz) (TxTicket sz tx)`
- Measure tracks: count, minTicket, maxTicket, cumulative size
- O(1) append, O(log n) split by ticket or by cumulative size
- Each tx gets monotonically increasing TicketNo (Word64)

## Capacity: Multi-dimensional Measure (Conway)
- ConwayMeasure = { byteSize: Word32, exUnits: {mem: Natural, steps: Natural}, refScriptsSize: Word32 }
- Default capacity = 2x block capacity (each dimension independently)
- Can override bytes only via MempoolCapacityBytesOverride (rounds up to whole blocks)
- NEW: DiffTimeMeasure tracks cumulative validation time (mempoolTimeoutCapacity)

## Fairness: Dual-FIFO MVar Design
- Two MVars: `remoteFifo` and `allFifo`
- Remote peers must acquire BOTH (remoteFifo then allFifo)
- Local clients only acquire `allFifo` (skip remoteFifo)
- Effect: local client = weight of ALL remote peers combined
- Per-tx granularity (not per-batch)

## Timeout Defense (post-Conway)
- mempoolTimeoutSoft: reject tx if validation takes longer (don't add to mempool)
- mempoolTimeoutHard: disconnect peer if validation exceeds this
- mempoolTimeoutCapacity: cumulative validation time limit for mempool fullness

## Block Production Snapshot
1. Forging thread calls `getSnapshotFor(slot, tickedLedgerState, readTables)`
2. If state matches cached, returns cached snapshot; otherwise revalidates all txs
3. `snapshotTake(blockCapacity)` uses `splitAfterTxSize` — O(log n) finger tree split
4. Returns longest prefix where cumulative measure <= block capacity (all dimensions)

## Revalidation (on new block)
- Background watcher thread detects tip change via STM
- `implSyncWithLedger` revalidates ALL mempool txs against new ledger state
- Invalid txs dropped, valid txs kept in original order with original ticket numbers
- Revalidation uses `reapplyTx` (cheaper than full `applyTx` — skips crypto checks)

## Transaction Dependencies
- Supported naturally: tx B spending tx A's output works if A is added first
- A's state changes are in the cumulative ledger state when B is validated
- No explicit dependency graph — ordering is purely insertion order

## No Eviction
- No eviction policy — mempool never removes valid transactions
- When capacity shrinks (protocol param change), existing txs stay until naturally invalidated
- `addTx` blocks (retries) when mempool is at capacity, until space freed by revalidation
