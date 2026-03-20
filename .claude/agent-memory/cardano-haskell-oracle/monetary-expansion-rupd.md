---
name: Monetary Expansion & RUPD Reward Calculation
description: Complete reference for deltaR1, eta, expectedBlocks, block counting, snapshot system, and the RUPD code path in cardano-ledger
type: reference
---

## Key Source Files
- `startStep` (the core RUPD calculation): `eras/shelley/impl/src/Cardano/Ledger/Shelley/LedgerState/PulsingReward.hs:89-212`
- `mkPoolRewardInfo` / `mkApparentPerformance`: `eras/shelley/impl/src/Cardano/Ledger/Shelley/Rewards.hs:86-394`
- `incrBlocks`: `eras/shelley/impl/src/Cardano/Ledger/Shelley/BlockBody/Internal.hs:289-296`
- `isOverlaySlot`: `libs/cardano-ledger-core/src/Cardano/Ledger/BHeaderView.hs:45-58`
- `BlocksMade`: `libs/cardano-ledger-core/src/Cardano/Ledger/BaseTypes.hs:869-874` (Map (KeyHash StakePool) Natural)
- `SnapShots`: `libs/cardano-ledger-core/src/Cardano/Ledger/State/SnapShots.hs:341-347`
- SNAP rule (rotation): `eras/shelley/impl/src/Cardano/Ledger/Shelley/Rules/Snap.hs:78-103`
- BBODY (block count): `eras/shelley/impl/src/Cardano/Ledger/Shelley/Rules/Bbody.hs:260-261`
- TICK rule (RUPD invocation): `eras/shelley/impl/src/Cardano/Ledger/Shelley/Rules/Tick.hs:260-279`
- RUPD rule: `eras/shelley/impl/src/Cardano/Ledger/Shelley/Rules/Rupd.hs:118-163`
- NewEpoch transition: `eras/shelley/impl/src/Cardano/Ledger/Shelley/Rules/NewEpoch.hs:151-198`
- Formal spec createRUpd: `eras/shelley/formal-spec/epoch.tex:1427-1458`

## Critical Formulas (from startStep in PulsingReward.hs)

### eta computation
```
d = unboundRational (pp ^. ppDG)  -- decentralization param
expectedBlocks = floor((1 - d) * activeSlotVal(asc) * slotsPerEpoch)
blocksMade = sum of all values in BlocksMade map
eta | d >= 0.8  = 1
    | otherwise = blocksMade / expectedBlocks   -- unsafe division
```
- `activeSlotVal(asc)` is the PositiveUnitInterval active slot coefficient (e.g., 0.05 on mainnet)
- In Conway (d=0): expectedBlocks = floor(f * slotsPerEpoch)

### deltaR1 (monetary expansion)
```
deltaR1 = rationalToCoinViaFloor(min(1, eta) * rho * reserves)
```
- `rho` = monetary expansion rate from protocol params
- `reserves` = current reserves (from ChainAccountState)
- Floor rounding

### Reward pot and treasury
```
rPot = ssFee(snapshots) + deltaR1        -- fees from "go" snapshot + expansion
deltaT1 = floor(tau * rPot)               -- treasury cut
R = rPot - deltaT1                        -- available for pool rewards
```

## Block Counting: incrBlocks

```haskell
incrBlocks isOverlay hk blocksMade
  | isOverlay = blocksMade        -- overlay blocks NOT counted
  | otherwise = Map.insertWith (+) hk 1 blocksMade
```
- Only non-overlay (decentralized) blocks increment the pool's count
- isOverlaySlot check: `step s < step (s + 1)` where `step x = ceiling(x * d)`
- When d=0 (full decentralization), isOverlaySlot always returns False → all blocks counted
- When d=1 (full federation), all slots are overlay → no blocks counted in BlocksMade

## Which BlocksMade feeds into RUPD?

1. TICK receives `nes0` with `nesBprev` (previous epoch's blocks)
2. TICK calls `validatingTickTransition` which may trigger NEWEPOCH
3. NEWEPOCH rotates: `nesBprev = bcur, nesBcur = empty`
4. BUT: TICK captured `bprev` from `nes0` BEFORE NEWEPOCH ran
5. RUPD receives `bprev` (from nes0) = `nesBprev` of the pre-transition state

**Key insight**: At epoch boundary, `bprev` from nes0 is the PREVIOUS-previous epoch blocks (wrong). But RUPD returns SNothing (RewardsTooEarly) at the epoch boundary. When RUPD actually fires (after stability window), `nesBprev` has already been rotated to hold the just-completed epoch's blocks. So the timing works out correctly.

## Mark/Set/Go Snapshot Feeding into RUPD

`startStep` uses `ssStakeGo ss` (the "go" snapshot):
```haskell
SnapShot activeStake totalActiveStake stakePoolSnapShots = ssStakeGo ss
```

SNAP rotation (each epoch boundary):
```
mark_new = current incremental stake
set_new  = old mark
go_new   = old set
fee_new  = current fees from UTxOState
```

So `ssStakeGo` = the snapshot from TWO epoch boundaries ago = the "set" from last epoch = the "mark" from two epochs ago.

The "go" snapshot's `ssFee` is the fee pot captured at the PREVIOUS epoch boundary (when set→go happened). But `ssFee` in SnapShots is captured at SNAP time from `utxosFees`, which means it's the fees accumulated during the epoch that just ended.

Actually: `ssFee = fees` where fees comes from `UTxOState _utxo _ fees _ _ _` at SNAP time. This is the current fees at the epoch boundary.

## Conway (d=0) Simplification

Since Conway has d=0:
- `isOverlaySlot` always returns False
- ALL blocks are counted in BlocksMade
- `expectedBlocks = floor(activeSlotCoeff * slotsPerEpoch)`
- eta guard: d >= 0.8 is False, so eta = blocksMade / expectedBlocks
- `mkApparentPerformance`: d < 0.8 branch → returns beta/sigma

## First Epochs (No Snapshots)

- Initial state: `nesBprev = BlocksMade empty, nesBcur = BlocksMade empty`
- All SnapShots start as `emptySnapShots` (empty stake, fee=0)
- With empty "go" snapshot: no pools → no rewards
- `blocksMade = 0` → eta = 0 → deltaR1 = 0
- First meaningful RUPD happens after 2 full epochs (when "go" has real data)
