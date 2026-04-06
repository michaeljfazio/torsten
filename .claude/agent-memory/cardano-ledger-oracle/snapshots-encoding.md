---
name: SnapShots CBOR encoding — new array(2) and old array(3) formats
description: SnapShots and SnapShot encoding, including backward compat with old 3-field format, and new StakePoolSnapShot array(10)
type: reference
---

# SnapShots CBOR Encoding

Source: `libs/cardano-ledger-core/src/Cardano/Ledger/State/SnapShots.hs`

## SnapShots = array(4)

Encoder:
```haskell
encodeListLen 4
  <> encCBOR ssStakeMark
  -- ssStakeMarkPoolDistr intentionally NOT serialized (redundant, recomputed on decode)
  <> encCBOR ssStakeSet
  <> encCBOR ssStakeGo
  <> encCBOR ssFee
```

```
array(4)
  [0] ssStakeMark  :: SnapShot    (mark snapshot)
  [1] ssStakeSet   :: SnapShot    (set snapshot)
  [2] ssStakeGo    :: SnapShot    (go snapshot)
  [3] ssFee        :: Coin        (fee snapshot)
```

`ssStakeMarkPoolDistr` is NOT serialized — it is recomputed from `ssStakeMark` on decode
via `calculatePoolDistr ssStakeMark`.

## SnapShot

The decoder branches on the list length:

### New format: array(2)
```
array(2)
  [0] ssActiveStake        :: ActiveStake  (VMap credential -> StakeWithDelegation)
  [1] ssStakePoolsSnapShot :: VMap (KeyHash StakePool) StakePoolSnapShot
```
`ssTotalActiveStake` is NOT serialized (recomputed from ssActiveStake on decode via `sumAllActiveStake`).

### Old format: array(3) — backward compat decode only
```
array(3)
  [0] Stake        :: old Stake type (VMap credential -> CompactForm Coin)
  [1] Delegations  :: VMap (Credential Staking) (KeyHash StakePool)
  [2] StakePoolsSnapShot :: VMap (KeyHash StakePool) StakePoolSnapShot
```
On decode, old format is converted to new ActiveStake format by merging stake+delegations.

## StakePoolSnapShot = array(10)

This is a DERIVED snapshot type (different from StakePoolState!), computed at snapshot time.

```
array(10)
  [0] spssStake                  :: CompactForm Coin
  [1] spssStakeRatio             :: Rational
  [2] spssSelfDelegatedOwners    :: Set (KeyHash Staking)
  [3] spssSelfDelegatedOwnersStake :: Coin
  [4] spssVrf                    :: VRFVerKeyHash StakePoolVRF
  [5] spssPledge                 :: Coin
  [6] spssCost                   :: Coin
  [7] spssMargin                 :: UnitInterval
  [8] spssNumDelegators          :: Int
  [9] spssAccountId              :: AccountId (= Credential Staking)
```

## Key Architectural Note

In Conway/UTxO-HD era, there are TWO different pool-related types:
1. `StakePoolState` (10 fields) — in PState.psStakePools — the live state with deposit+delegators
2. `StakePoolSnapShot` (10 fields) — in SnapShot.ssStakePoolsSnapShot — derived for reward calc
3. `StakePoolParams` (9 fields via CBORGroup) — in PState.psFutureStakePoolParams — registration data

These are all distinct types encoded differently. Old code used PoolParams (= StakePoolParams) in snapshots;
new code uses StakePoolSnapShot.
