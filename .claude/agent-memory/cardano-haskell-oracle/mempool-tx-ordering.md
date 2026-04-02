---
name: Mempool Transaction Ordering and Chained Tx Handling
description: Complete deep-dive into Haskell mempool ordering (FIFO with TicketNo), chained tx validation (virtual ledger state), TxSubmission2 serving order, block production selection, and revalidation on new blocks. Critical for Dugite conformance.
type: reference
---

## Mempool Internal Ordering: FIFO via TicketNo (NOT fee-density)

File: `ouroboros-consensus/src/.../Mempool/TxSeq.hs`

- `TxSeq` is a **StrictFingerTree** indexed by `TicketNo` (monotonically increasing Word64)
- Each tx gets the **next** TicketNo on admission: `nextTicketNo = succ isLastTicketNo`
- **No fee-based ordering**. Purely insertion order.
- `snapshotTxs` returns txs **oldest to newest** (ascending TicketNo)
- `snapshotTxsAfter` splits by TicketNo, returns remainder

## TxSubmission2 Outbound (Serving txs to peers): FIFO

File: `ouroboros-network/lib/.../TxSubmission/Outbound.hs`

- `txSubmissionOutbound` tracks a `lastIdx` (TicketNo cursor)
- On each MsgRequestTxIds: calls `mempoolTxIdsAfter lastIdx` → gets txs AFTER the cursor
- Takes `take (fromIntegral reqNo)` from those → **oldest-first** (ascending TicketNo)
- No fee-density sorting, no dependency-awareness
- Window: `maxUnacked` = 10 (from `defaultTxDecisionPolicy.maxUnacknowledgedTxIds`)

## Block Production: FIFO prefix, NOT fee-sorted

File: `ouroboros-consensus/.../NodeKernel.hs` lines 708-710

```haskell
let (txs, txssz) = snapshotTake mempoolSnapshot $
      blockCapacityTxMeasure (configLedger cfg) tickedLedgerState
```

- `snapshotTake` calls `TxSeq.splitAfterTxSize` — takes **longest prefix** by total size
- This is FIFO order (ascending TicketNo), NOT fee-density sorted
- Block producer takes as many txs as fit from the front of the sequence

## Chained Tx Admission: Virtual Ledger State

File: `ouroboros-consensus/src/.../Mempool/Impl/Common.hs`

Key field: `isLedgerState :: TickedLedgerState blk DiffMK`

- This is the **ticked ledger state after applying ALL mempool txs** sequentially
- New tx validated via `applyTx cfg wti isSlotNo tx st` where `st` includes pending tx diffs
- `st = applyMempoolDiffs values (getTransactionKeySets tx) (isLedgerState is)`
- This effectively creates: **on-chain UTxO + all pending mempool tx outputs - all pending mempool tx inputs**
- A chained tx (spending output of pending tx A) WILL be accepted if tx A is already in mempool
- The diffs are accumulated: `prependMempoolDiffs isLedgerState st'`

## Revalidation on New Block (pureSyncWithLedger)

File: `ouroboros-consensus/src/.../Mempool/Update.hs`

- `implSyncWithLedger` triggered by background watcher when ChainDB tip changes
- Calls `pureSyncWithLedger` → `revalidateTxsFor` → `reapplyTxs`
- `reapplyTxs` processes txs **in original order** (ascending TicketNo)
- Each tx reapplied against: fresh ticked ledger state + diffs from previous valid txs
- If tx A was in a block AND tx B (child) is in mempool:
  - tx B is **revalidated** (NOT automatically removed)
  - tx A's output is now on-chain, so tx B's input is valid → tx B survives
- If tx A was in a **different** block than expected, or the chain forked:
  - tx B revalidated against new ledger state
  - If parent output no longer exists → tx B fails revalidation → removed
- `TraceMempoolRemoveTxs` emitted for removed txs

## Key Policy Constants (defaultTxDecisionPolicy)

File: `ouroboros-network/lib/.../TxSubmission/Inbound/V2/Policy.hs`

```
maxNumTxIdsToRequest   = 3      -- max txids requested at once
maxUnacknowledgedTxIds = 10     -- max unacked txids (window size)
txsSizeInflightPerPeer = 393,240 (65,540 * 6) -- max bytes inflight per peer
txInflightMultiplicity = 2      -- download same tx from max 2 peers
bufferedTxsMinLifetime = 2s     -- keep buffered txs 2s to avoid re-download
```

## Dugite Divergences Identified

1. **Block production uses fee-density sorting** — Haskell uses FIFO prefix
2. **TxSubmission2 server serves from `snapshot.tx_hashes`** (FIFO order from VecDeque) — this is correct-ish but uses no TicketNo cursor
3. **No TicketNo-based cursor** for incremental tx serving — Dugite filters by inflight set instead
4. **No `isLedgerState` equivalent** — Dugite uses `virtual_utxo` DashMap instead of sequentially-applied ledger diffs
