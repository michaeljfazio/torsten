---
name: NewEpochState/EpochState/LedgerState/UTxOState complete CBOR encoding
description: Verified field order and array sizes for all top-level ledger state types from cardano-ledger source
type: reference
---

# Complete Ledger State CBOR Encoding (Verified from Source)

Source file: `eras/shelley/impl/src/Cardano/Ledger/Shelley/LedgerState/Types.hs`

## NewEpochState = array(7)

Encoder: `encodeListLen 7 <> encCBOR e <> encCBOR bp <> encCBOR bc <> encCBOR es <> encCBOR ru <> encCBOR pd <> encCBOR av`

```
array(7)
  [0] nesEL           :: EpochNo           (u64 integer)
  [1] nesBprev        :: BlocksMade        (map keyhash->natural)
  [2] nesBcur         :: BlocksMade        (map keyhash->natural)
  [3] nesEs           :: EpochState        (array(4))
  [4] nesRu           :: StrictMaybe PulsingRewUpdate  (array(0)=SNothing, array(1)[x]=SJust)
  [5] nesPd           :: PoolDistr         (map + total)
  [6] stashedAVVMAddresses :: StashedAVVMAddresses era
                         Conway: encoded as () = array(0)
                         Shelley only: UTxO
```

CONFIRMED: nesPd (PoolDistr) is at index 5. stashedAVVM is index 6.

## EpochState = array(4)

Encoder uses `Rec EpochState !> To esChainAccountState !> To esLState !> To esSnapshots !> To esNonMyopic`

```
array(4)
  [0] esChainAccountState :: ChainAccountState  (array(2) [treasury, reserves])
  [1] esLState            :: LedgerState        (array(2))
  [2] esSnapshots         :: SnapShots          (array(4))
  [3] esNonMyopic         :: NonMyopic
```

NOTE: Field order in the Haskell data declaration is:
  esChainAccountState, esLState, esSnapshots, esNonMyopic
And the EncCBOR instance encodes them in THAT SAME ORDER.
Comment in source: "We get better sharing when encoding ledger state before snapshots"

## LedgerState = array(2)

Encoder: `encodeListLen 2 <> encCBOR lsCertState <> encCBOR lsUTxOState`

```
array(2)
  [0] lsCertState  :: CertState  (Conway: array(3))
  [1] lsUTxOState  :: UTxOState  (array(6))
```

CRITICAL: CertState is encoded FIRST even though the Haskell struct declares UTxOState first!
Comment in source: "encode delegation state first to improve sharing"

## UTxOState = array(6)

Encoder uses `Rec UTxOState !> E (encodeMap encodeMemPack encodeMemPack . unUTxO) utxosUtxo !> To utxosDeposited !> To utxosFees !> To utxosGovState !> To utxosInstantStake !> To utxosDonation`

```
array(6)
  [0] utxosUtxo          :: UTxO     (map with MemPack encoding — NOT standard encCBOR)
  [1] utxosDeposited     :: Coin     (integer)
  [2] utxosFees          :: Coin     (integer)
  [3] utxosGovState      :: GovState (Conway: ConwayGovState array(7))
  [4] utxosInstantStake  :: InstantStake  (ActiveStake VMap)
  [5] utxosDonation      :: Coin     (integer)
```

IMPORTANT: Field previously called `utxosStakeDistr` in older code is now `utxosInstantStake`.
The UTxO field uses MemPack encoding (encodeMap encodeMemPack encodeMemPack), NOT standard encCBOR.
There is NO UTxO-HD variant for ledger snapshots — this is the single CBOR encoding.

## ChainAccountState = array(2)

```
array(2)
  [0] casTreasury :: Coin
  [1] casReserves :: Coin
```

Source: `libs/cardano-ledger-core/src/Cardano/Ledger/State/ChainAccount.hs`
