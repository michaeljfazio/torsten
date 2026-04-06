---
name: Conway Accounts and ConwayAccountState CBOR encoding
description: Per-account encoding for Conway DState accounts — array(4) with nullable pool/drep delegations
type: reference
---

# Conway Accounts CBOR Encoding

Source: `eras/conway/impl/src/Cardano/Ledger/Conway/State/Account.hs`

## ConwayAccounts

`ConwayAccounts` is a newtype over `Map (Credential Staking) (ConwayAccountState era)`.
It derives `EncCBOR` directly — encoded as a standard CBOR map.

```
map(N) {
  credential_staking => ConwayAccountState,
  ...
}
```

## ConwayAccountState = array(4)

Encoder:
```haskell
encodeListLen 4
  <> encCBOR casBalance
  <> encCBOR casDeposit
  <> encodeNullStrictMaybe encCBOR casStakePoolDelegation
  <> encodeNullStrictMaybe encCBOR casDRepDelegation
```

```
array(4)
  [0] casBalance              :: CompactForm Coin   (u64 lovelace)
  [1] casDeposit              :: CompactForm Coin   (u64 lovelace, stake credential deposit)
  [2] casStakePoolDelegation  :: null | KeyHash StakePool  (encodeNullStrictMaybe)
  [3] casDRepDelegation       :: null | DRep               (encodeNullStrictMaybe)
```

Note: Fields [2] and [3] use `encodeNullStrictMaybe`:
- SNothing => CBOR `null` (0xf6)
- SJust x  => encCBOR x (NOT wrapped in array — just the raw value)

## ConwayAccountState variants (internal, not in CBOR)

Internally stored as a sum type for efficiency:
- `CASNoDelegation balance deposit`
- `CASStakePool balance deposit poolHash`
- `CASDRep balance deposit dRep`
- `CASStakePoolAndDRep balance deposit poolHash dRep`

But all four variants encode identically to array(4) with nullable fields [2] and [3].

## DRep encoding (field [3])

DRep is encoded as a discriminated type:
- `DRepCredential cred` => Sum tag 0, cred
- `DRepAlwaysAbstain` => Sum tag 1 (no fields)
- `DRepAlwaysNoConfidence` => Sum tag 2 (no fields)
