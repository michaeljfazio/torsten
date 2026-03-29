---
name: Conway InstantStake pointer exclusion
description: ConwayInstantStake is a separate type with NO sisPtrStake field; pointer-addressed UTxOs are silently dropped at the Conway boundary
type: reference
---

# Conway InstantStake: Pointer Stake Exclusion

## The Core Answer

Conway uses a **completely different `InstantStake` type** (`ConwayInstantStake`) that has **no `sisPtrStake` field at all**. This is the entire mechanism.

## Type Family: EraStake

Defined in `libs/cardano-ledger-core/src/Cardano/Ledger/State/Stake.hs`:

```haskell
class EraStake era where
  type InstantStake era = (r :: Type) | r -> era
  instantStakeCredentialsL :: Lens' (InstantStake era) (Map (Credential Staking) (CompactForm Coin))
  addInstantStake    :: UTxO era -> InstantStake era -> InstantStake era
  deleteInstantStake :: UTxO era -> InstantStake era -> InstantStake era
  resolveInstantStake :: InstantStake era -> Accounts era -> ActiveStake
```

## Per-Era Instances

| Era | InstantStake type | Has sisPtrStake? |
|-----|-------------------|-----------------|
| Shelley | `ShelleyInstantStake ShelleyEra` | YES |
| Allegra | `ShelleyInstantStake AllegraEra` | YES |
| Mary | `ShelleyInstantStake MaryEra` | YES |
| Alonzo | `ShelleyInstantStake AlonzoEra` | YES |
| Babbage | `ShelleyInstantStake BabbageEra` | YES |
| **Conway** | **`ConwayInstantStake ConwayEra`** | **NO** |

## ShelleyInstantStake (Shelley→Babbage)

File: `eras/shelley/impl/src/Cardano/Ledger/Shelley/State/Stake.hs`

```haskell
data ShelleyInstantStake era = ShelleyInstantStake
  { sisCredentialStake :: !(Map.Map (Credential Staking) (CompactForm Coin))
  , sisPtrStake        :: !(Map.Map Ptr (CompactForm Coin))
  }
```

`addShelleyInstantStake` / `applyUTxOShelleyInstantStake`:
- `Addr _ _ (StakeRefPtr stakingPtr)` → goes into `sisPtrStake`
- `Addr _ _ (StakeRefBase stakingKeyHash)` → goes into `sisCredentialStake`

`resolveShelleyInstantStake`: after building `credentialStakeMap` from credentials+accounts, iterates
`sisPtrStake`, looks up each `Ptr` in `saPtrs sas` (pointer-to-credential map), resolves to
`StakeWithDelegation`, merges into the active stake map.

## ConwayInstantStake (Conway)

File: `eras/conway/impl/src/Cardano/Ledger/Conway/State/Stake.hs`

```haskell
newtype ConwayInstantStake era = ConwayInstantStake
  { cisCredentialStake :: Map.Map (Credential Staking) (CompactForm Coin)
  }
```

`applyUTxOConwayInstantStake`:
```haskell
accum ans@(ConwayInstantStake {cisCredentialStake}) out =
  let cc = out ^. compactCoinTxOutL
   in case out ^. addrTxOutL of
        Addr _ _ (StakeRefBase stakingKeyHash) ->
          ConwayInstantStake
            { cisCredentialStake = Map.alter (keepOrDeleteCompact cc) stakingKeyHash cisCredentialStake
            }
        _other -> ans   -- StakeRefPtr falls through to _other → SILENTLY DROPPED
```

`_other` matches `StakeRefPtr`, `StakeRefNull`, and bootstrap addresses. All are dropped. There is no
pointer map to store them in.

`resolveConwayInstantStake`:
```haskell
resolveConwayInstantStake instantStake accounts =
  ActiveStake $ VMap.fromMap $ resolveActiveInstantStakeCredentials instantStake accounts
```
No pointer resolution pass at all — there is nothing to resolve.

## What Happens at the HFC Boundary

When the HFC transitions the ledger from Babbage to Conway, the `UTxOState` is translated. As
part of that, the `ShelleyInstantStake BabbageEra` is translated to `ConwayInstantStake ConwayEra`.

The `DecShareCBOR` instance for `ConwayInstantStake` handles migration from on-disk
`ShelleyInstantStake` CBOR:

```haskell
instance DecShareCBOR (ConwayInstantStake era) where
  decShareCBOR credInterns = do
    peekTokenType >>= \case
      TypeListLen   -> toConwayInstantStake <$> decShareCBOR credInterns  -- array(2): Shelley format
      TypeListLen64 -> toConwayInstantStake <$> decShareCBOR credInterns
      TypeListLenIndef -> toConwayInstantStake <$> decShareCBOR credInterns
      _             -> ConwayInstantStake <$> decShareCBOR (credInterns, mempty)  -- map: Conway format
  where
    toConwayInstantStake :: ShelleyInstantStake era -> ConwayInstantStake era
    toConwayInstantStake = ConwayInstantStake . sisCredentialStake
```

`toConwayInstantStake` takes only `sisCredentialStake` and **discards `sisPtrStake`** entirely.
Any pre-Conway UTxOs with pointer addresses lose their instant stake at the moment of HFC
deserialization. The `sisPtrStake` map is permanently dropped.

## What Happens to New Conway Txs Spending Old Pointer-Addressed UTxOs

When a Conway tx spends a pointer-addressed UTxO:
- `deleteConwayInstantStake` is called on the consumed UTxO
- The `_other` branch fires → no-op, nothing to delete
- The instant stake map is unchanged (was never updated for it)

When a Conway tx creates a pointer-addressed output (not possible to create NEW pointer addresses in
Conway since `StakeRefPtr` is still a valid constructor, but no new registrations go to pointers):
- `addConwayInstantStake` is called on the new UTxO
- The `_other` branch fires → silently dropped

## SNAP Rule (unchanged — ShelleySNAP used by both Shelley and Conway)

File: `eras/shelley/impl/src/Cardano/Ledger/Shelley/Rules/Snap.hs`

Conway's Epoch rule delegates to `ShelleySNAP` (same rule, different type witnesses):
```haskell
instance (...) => Embed (ShelleySNAP era) (ConwayEPOCH era) where ...
```

`snapTransition` calls:
```haskell
snapShotFromInstantStake instantStake (certState ^. certDStateL) (certState ^. certPStateL)
```

`snapShotFromInstantStake` calls `resolveInstantStake instantStake $ dsAccounts dState`.

For Conway, `resolveInstantStake` dispatches to `resolveConwayInstantStake`, which does:
```haskell
ActiveStake $ VMap.fromMap $ resolveActiveInstantStakeCredentials instantStake accounts
```

`resolveActiveInstantStakeCredentials` operates only on `instantStakeCredentialsL` (the
credential map), merging with accounts via `Map.merge`. There is no pointer resolution pass.

## Summary: The 6-Part Answer

1. **YES — Conway uses a DIFFERENT InstantStake type.** `ConwayInstantStake` is a `newtype` with
   only `cisCredentialStake :: Map (Credential Staking) (CompactForm Coin)`. There is NO `sisPtrStake`.

2. **The type change, not the resolve function, is the primary mechanism.** `addConwayInstantStake`
   silently drops `StakeRefPtr` outputs via `_other -> ans`. The stake is never recorded.

3. **The SNAP rule is NOT changed.** Both Shelley and Conway use `ShelleySNAP`. The difference is
   purely in what `resolveInstantStake` does for each era — Conway's version has no pointer pass.

4. **`addConwayInstantStake` drops pointer-addressed UTxOs via `_other` fallthrough.** This is
   the transaction-level mechanism: new outputs with `StakeRefPtr` contribute zero to instant stake.

5. **At the HFC boundary, `sisPtrStake` is permanently discarded.** The `DecShareCBOR` migration
   (`toConwayInstantStake = ConwayInstantStake . sisCredentialStake`) extracts only credential stake
   and throws away the pointer map. Pre-Conway UTxOs' pointer stake is gone from the instant stake
   at the moment the first Conway block is processed.

6. **Pre-Conway pointer-addressed UTxOs still exist in the UTxO set.** Their value is unaffected.
   They can be spent. But their ADA does not appear in the pool stake distribution, meaning their
   stake does not count toward any pool's voting power or reward eligibility.

## Key Files

- `libs/cardano-ledger-core/src/Cardano/Ledger/State/Stake.hs` — `EraStake` class, `resolveActiveInstantStakeCredentials`
- `eras/shelley/impl/src/Cardano/Ledger/Shelley/State/Stake.hs` — `ShelleyInstantStake`, `addShelleyInstantStake`, `resolveShelleyInstantStake`
- `eras/conway/impl/src/Cardano/Ledger/Conway/State/Stake.hs` — `ConwayInstantStake`, `addConwayInstantStake`, `resolveConwayInstantStake`
- `eras/babbage/impl/src/Cardano/Ledger/Babbage/State/Stake.hs` — Babbage uses `ShelleyInstantStake`
- `eras/shelley/impl/src/Cardano/Ledger/Shelley/Rules/Snap.hs` — SNAP rule (shared by all eras including Conway)
- `libs/cardano-ledger-core/src/Cardano/Ledger/State/SnapShots.hs` — `snapShotFromInstantStake`, `calculatePoolDistr`
