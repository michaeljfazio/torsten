---
name: reward-iteration-deep-dive
description: How startStep iterates over pools for reward calculation — GO snapshot iteration, BlocksMade filter, zero-reward conditions, genesis pool warm-up timeline
type: reference
---

## Reward Calculation Iteration Pattern

### startStep iterates over GO snapshot pool params, NOT BlocksMade

File: `eras/shelley/impl/src/Cardano/Ledger/Shelley/LedgerState/PulsingReward.hs`

```haskell
let SnapShot activeStake totalActiveStake stakePoolSnapShots = ssStakeGo ss
    allPoolInfo = VMap.mapWithKey mkPoolRewardInfoCurry stakePoolSnapShots
    blockProducingPoolInfo = VMap.mapMaybe (either (const Nothing) Just) allPoolInfo
```

- Iterates every pool in GO's `ssStakePoolsSnapShot`
- `mkPoolRewardInfo` then checks if pool is in BlocksMade
- `Left StakeShare` = no blocks (ranking only), `Right PoolRewardInfo` = has blocks (rewards)
- `blockProducingPoolInfo` filters to only Right values
- Pools in BlocksMade but NOT in GO snapshot are never visited

### BlocksMade source: nesBprev (previous epoch's blocks)

TICK passes `nesBprev` to RUPD environment. At epoch N boundary, NEWEPOCH sets `nesBprev = bcur` (epoch N-1 blocks).

### Genesis Pool Warm-Up Timeline

| Boundary | Mark | Set | Go | First reward-eligible? |
|----------|------|-----|-----|----------------------|
| Genesis | Has pools | empty | empty | No |
| 0->1 | New | Genesis | empty | No |
| 1->2 | New | Epoch1 | Genesis | YES |

Genesis pools first in GO at epoch 2. First rewards for epoch 1 blocks, computed in epoch 2, applied at 2->3 boundary.

### Zero-Reward Conditions (even with blocks + stake)

1. **Pledge failure in GO snapshot**: `pledge > selfDelegatedOwnersStake` -> maxP=0
2. **sigma=0**: GO snapshot stake is zero -> apparent performance = 0
3. **Total _R = 0**: no fees + no deltaR1
4. **Pool not in GO snapshot**: structural, never visited by startStep
5. **Floor rounds to 0**: appPerf * maxP < 1 lovelace
6. **Pre-Babbage: reward account unregistered**: leader reward filtered (proto <= 6 only)

### No active_epoch_no or minimum age check

StakePoolState has no epoch fields. The 2-epoch warm-up is purely structural (mark->set->go rotation).
