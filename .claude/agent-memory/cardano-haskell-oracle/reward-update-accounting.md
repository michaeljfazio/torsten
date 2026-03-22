---
name: reward-update-accounting
description: Complete RUPD/RewardUpdate accounting - deltaR/deltaT/deltaF formulas, sign conventions, conservation invariant, undistributed→reserves, applyRUpd, no-blocks behavior, d>=0.8 semantics
type: reference
---

## RewardUpdate Fields (cardano-ledger-shelley RewardUpdate.hs)

```haskell
data RewardUpdate = RewardUpdate
  { deltaT :: DeltaCoin  -- POSITIVE: treasury increase = floor(tau * rPot)
  , deltaR :: DeltaCoin  -- USUALLY NEGATIVE: -deltaR1 + deltaR2
  , rs     :: Map Cred (Set Reward)  -- per-account rewards
  , deltaF :: DeltaCoin  -- NEGATIVE: -ssFee (fees consumed)
  , nonMyopic :: NonMyopic
  }
```

## Construction (PulsingReward.hs startStep + completeRupd)

```
deltaR1 = floor(min(1, eta) * rho * reserves)
rPot = ssFee + deltaR1
deltaT1 = floor(tau * rPot)
_R = rPot - deltaT1
sum(rs) = total distributed rewards
deltaR2 = _R - sum(rs)         -- UNDISTRIBUTED goes back to reserves

deltaT = DeltaCoin deltaT1
deltaR = invert(toDeltaCoin deltaR1) <> toDeltaCoin deltaR2 = (-deltaR1 + deltaR2)
deltaF = invert(toDeltaCoin ssFee) = -ssFee
```

## Conservation Invariant (asserted in updateRewards)

`deltaT + deltaR + sum(rs) + deltaF = 0`

## applyRUpd (IncrementalStake.hs)

```
treasury_new = treasury + deltaT + unregistered_rewards_total
reserves_new = reserves + deltaR
fees_new = fees + deltaF (zeroes fee pot)
accounts += registered rewards from rs
```

## KEY FACTS:
- **Undistributed rewards → RESERVES (not treasury)** via deltaR2
- Treasury gets: tau_cut + unregistered rewards only
- Unregistered rewards = rewards for creds no longer in DState accounts
- deltaF IS applied (zeroes out utxosFeesL)
- When no pools produce blocks: _R goes entirely to deltaR2 → reserves
- When d >= 0.8: eta=1 (full expansion), apparent_perf=1 for any pool IN BlocksMade
- BFT/overlay blocks do NOT appear in BlocksMade → those pools get nothing

## Torsten divergences:
1. No unregistered→treasury pathway (gap)
2. Accounting is algebraically equivalent but uses different fields (net_reserve_decrease vs deltaR)
3. Early-return paths for zero total_stake/active_stake may have formula mismatch
