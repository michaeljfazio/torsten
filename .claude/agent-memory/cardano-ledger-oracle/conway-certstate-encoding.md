---
name: Conway CertState/DState/PState/VState CBOR encoding
description: Complete encoding for Conway CertState and all sub-states, including StakePoolState vs StakePoolParams distinction
type: reference
---

# Conway CertState CBOR Encoding

Source files:
- ConwayCertState: `eras/conway/impl/src/Cardano/Ledger/Conway/State/CertState.hs`
- VState: `eras/conway/impl/src/Cardano/Ledger/Conway/State/VState.hs`
- DState/PState: `libs/cardano-ledger-core/src/Cardano/Ledger/State/CertState.hs`
- StakePoolState/StakePoolParams: `libs/cardano-ledger-core/src/Cardano/Ledger/State/StakePool.hs`

## ConwayCertState = array(3)

Encoder:
```haskell
encodeListLen 3
  <> encCBOR conwayCertVState
  <> encCBOR conwayCertPState
  <> encCBOR conwayCertDState
```

```
array(3)
  [0] conwayCertVState :: VState   (array(3))
  [1] conwayCertPState :: PState   (array(4))
  [2] conwayCertDState :: DState   (array(4))
```

CONFIRMED: VState is encoded FIRST. Order: V, P, D.

## VState = array(3)

Uses `Rec (VState @era) !> To vsDReps !> To vsCommitteeState !> To vsNumDormantEpochs`

```
array(3)
  [0] vsDReps             :: Map (Credential DRepRole) DRepState
  [1] vsCommitteeState    :: CommitteeState  (map credential -> authorization)
  [2] vsNumDormantEpochs  :: EpochNo         (u64)
```

## PState = array(4)

Encoder: `encodeListLen 4 <> encCBOR a <> encCBOR b <> encCBOR c <> encCBOR d`
where fields are (a=psVRFKeyHashes, b=psStakePools, c=psFutureStakePoolParams, d=psRetiring)

```
array(4)
  [0] psVRFKeyHashes          :: Map (VRFVerKeyHash StakePoolVRF) (NonZero Word64)
  [1] psStakePools            :: Map (KeyHash StakePool) StakePoolState  (NEW type!)
  [2] psFutureStakePoolParams :: Map (KeyHash StakePool) StakePoolParams (old registration params)
  [3] psRetiring              :: Map (KeyHash StakePool) EpochNo
```

CRITICAL DISTINCTION:
- psStakePools maps pool hash -> StakePoolState (10 fields, includes delegators set + deposit)
- psFutureStakePoolParams maps pool hash -> StakePoolParams (9 fields via CBORGroup, no deposit/delegators)
These are different types!

## StakePoolState = array(10)

Uses `Rec StakePoolState !> To ... (10 fields)`

```
array(10)
  [0] spsVrf          :: VRFVerKeyHash StakePoolVRF  (32 bytes)
  [1] spsPledge       :: Coin
  [2] spsCost         :: Coin
  [3] spsMargin       :: UnitInterval
  [4] spsAccountId    :: AccountId (= Credential Staking)
  [5] spsOwners       :: Set (KeyHash Staking)
  [6] spsRelays       :: StrictSeq StakePoolRelay
  [7] spsMetadata     :: StrictMaybe PoolMetadata
  [8] spsDeposit      :: CompactForm Coin
  [9] spsDelegators   :: Set (Credential Staking)
```

This is the NEW UTxO-HD era type with embedded deposit and delegator tracking.

## StakePoolParams = array(9) via CBORGroup

Used for psFutureStakePoolParams. Encoded via EncCBORGroup (listLen=9, fields encoded without header).
When encoded via CBORGroup as a standalone value: `array(9) [id, vrf, pledge, cost, margin, accountAddress, owners, relays, metadata_or_null]`

```
[0] sppId             :: KeyHash StakePool   (28 bytes)
[1] sppVrf            :: VRFVerKeyHash       (32 bytes)
[2] sppPledge         :: Coin
[3] sppCost           :: Coin
[4] sppMargin         :: UnitInterval
[5] sppAccountAddress :: AccountAddress (= Credential Staking)
[6] sppOwners         :: Set (KeyHash Staking)
[7] sppRelays         :: StrictSeq StakePoolRelay
[8] sppMetadata       :: null | PoolMetadata (encodeNullStrictMaybe)
```

Note: metadata uses encodeNullStrictMaybe (CBOR null for Nothing, not array(0)).

## DState = array(4)

Encoder: `encodeListLen 4 <> encCBOR dsAccounts <> encCBOR dsFutureGenDelegs <> encCBOR dsGenDelegs <> encCBOR dsIRewards`

```
array(4)
  [0] dsAccounts         :: Accounts era  (Conway: ConwayAccounts = map credential->ConwayAccountState)
  [1] dsFutureGenDelegs  :: Map FutureGenDeleg GenDelegPair
  [2] dsGenDelegs        :: GenDelegs     (map keyhash -> GenDelegPair)
  [3] dsIRewards         :: InstantaneousRewards (array(4))
```

## InstantaneousRewards = array(4)

```
array(4)
  [0] iRReserves    :: Map (Credential Staking) Coin
  [1] iRTreasury    :: Map (Credential Staking) Coin
  [2] deltaReserves :: DeltaCoin  (integer, may be negative)
  [3] deltaTreasury :: DeltaCoin
```
