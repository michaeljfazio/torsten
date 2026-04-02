---
name: mempool-revalidation-after-block
description: How Haskell cardano-node revalidates the mempool after a new block is applied
type: reference
---

## Summary

After a new block is adopted, the Haskell mempool does a FULL revalidation of ALL remaining
transactions against the new ledger state — not just a simple removal of confirmed txs.

## Trigger Path

1. `openMempool` (Init.hs) calls `forkSyncStateOnTipPointChange` which spawns a background
   `forkLinkedWatcher` thread named "Mempool.syncStateOnTipPointChange".
2. The watcher reads `getCurrentTip` (= `ledgerTipPoint . mldViewState`) in STM. This is an
   STM fingerprint: it retries/wakes whenever the LedgerDB tip changes.
3. ChainSel does NOT directly call any mempool function. The mempool learns about new blocks
   purely through the STM ledger tip change.
4. When the watcher fires, it calls `implSyncWithLedger`.

## implSyncWithLedger (Update.hs)

- Acquires the mempool `StrictTMVar` (`mpEnvStateVar`).
- In the same STM transaction, reads the new ledger state from the LedgerDB.
- If `pointHash(isTip is) == getTipHash(ls)` AND `isSlotNo is == slot`: tip didn't change,
  emits `TraceMempoolSyncNotNeeded`, returns immediately — NO revalidation.
- Otherwise: calls `pureSyncWithLedger`.

## pureSyncWithLedger (Update.hs)

Calls `revalidateTxsFor` with ALL remaining mempool transactions against the new ticked
ledger state.

```haskell
pureSyncWithLedger capacityOverride lcfg slot lstate values istate =
  let RevalidateTxsResult is' removed =
        revalidateTxsFor
          capacityOverride lcfg slot lstate values
          (isLastTicketNo istate)
          (TxSeq.toList $ isTxs istate)  -- ALL remaining txs
  in (is', if null removed then Nothing else Just $ TraceMempoolRemoveTxs ...)
```

## revalidateTxsFor (Impl/Common.hs)

The core function. Takes all mempool transactions and reapplies them to the new ledger state:

```haskell
revalidateTxsFor capacityOverride cfg slot st values lastTicketNo txTickets =
  let ReapplyTxsResult err val st' =
        reapplyTxs ComputeDiffs cfg slot theTxs ...
```

Uses `reapplyTxs` which folds `reapplyTx` over all transactions LEFT TO RIGHT:
- `reapplyTx` skips expensive crypto checks (signature verification) but runs ALL
  other ledger checks including: double-spend detection (UTxO inputs consumed),
  TTL/validity interval checks, script validity, min UTxO checks, etc.
- Any tx that fails becomes an `Invalidated blk` entry in `invalidatedTxs`.
- Valid txs become `validatedTxs` — their ordering is PRESERVED.
- The fold is left-to-right; a tx that depends on a prior tx that was removed will
  also fail because its inputs won't be present.

## applyTx vs reapplyTx distinction

- `applyTx`: full validation including crypto (used when adding NEW transactions)
- `reapplyTx`: skips expensive crypto, but checks double-spends, TTL, scripts, etc.
  (used during revalidation of previously-validated transactions)

## What gets caught in revalidation

1. Confirmed txs: their UTxO inputs are now consumed → double-spend → removed.
2. TTL-expired txs: validity interval check fails → removed.
3. Chained/orphaned txs: if tx A was confirmed and tx B spent tx A's outputs,
   tx B's inputs are no longer available → removed.
4. Any tx that fails a ledger rule against the new ledger state (fee changes, etc.)

## Metrics update

The new `InternalState` is written atomically to the `StrictTMVar`:
```
withTMVarAnd istate ... -> pure (Just (snapshotFromIS is'), is')
```
`isMempoolSize` computes from `TxSeq.toSize (isTxs is)` — O(1), reads cached totals.
The update is synchronous: the TMVar write and the revalidation happen in the same
critical section holding the mempool state lock.

`TraceMempoolRemoveTxs` is emitted after the update completes, listing each removed tx
and its rejection reason.

## Key Files

- `ouroboros-consensus/src/ouroboros-consensus/Ouroboros/Consensus/Mempool/Init.hs`
  — `openMempool`, `forkSyncStateOnTipPointChange`
- `ouroboros-consensus/src/ouroboros-consensus/Ouroboros/Consensus/Mempool/Update.hs`
  — `implSyncWithLedger`, `pureSyncWithLedger`, `pureRemoveTxs`, `revalidateTxsFor`
- `ouroboros-consensus/src/ouroboros-consensus/Ouroboros/Consensus/Mempool/Impl/Common.hs`
  — `revalidateTxsFor`, `InternalState`, `reapplyTxs`
- `ouroboros-consensus/src/ouroboros-consensus/Ouroboros/Consensus/Ledger/SupportsMempool.hs`
  — `reapplyTx`, `reapplyTxs`, `ReapplyTxsResult`, `Invalidated`

## Dugite Implications

1. Must do FULL revalidation of ALL mempool txs after every block, not just remove confirmed ones.
2. Revalidation uses `reapplyTx` semantics — skip crypto, run everything else.
3. The trigger is a background watcher on ledger tip change, not a direct call from block application.
4. Ordering of surviving txs is PRESERVED (same TicketNo order).
5. Metrics update is synchronous and atomic with the revalidation.
6. Short-circuit: if tip hash and slot are unchanged, skip revalidation entirely.
