---
name: ConwayGovState CBOR encoding complete reference
description: Full wire format for GetGovState tag 24 response — ConwayGovState array(7), DRepPulsingState, PulsingSnapshot, RatifyState, EnactState, FuturePParams, Proposals
type: reference
---

# ConwayGovState CBOR Encoding (GetGovState tag 24)

Source files:
- ConwayGovState: `cardano-ledger/eras/conway/impl/src/Cardano/Ledger/Conway/Governance.hs`
- DRepPulsingState, PulsingSnapshot: `...Governance/DRepPulser.hs`
- RatifyState, EnactState: `...Governance/Internal.hs`
- FuturePParams: `libs/cardano-ledger-core/src/Cardano/Ledger/State/Governance.hs`
- Proposals: `...Governance/Proposals.hs`
- GovRelation: `...Governance/Procedures.hs`
- Binary.Coders: `libs/cardano-ledger-binary/src/Cardano/Ledger/Binary/Encoding/Coders.hs`

## Encoding Rules from Binary.Coders

- `Rec Constructor !> To field1 !> To field2 ...` => `array(N) [field1, field2, ...]` where N = count of `To` fields
- `Sum Constructor tag !> To field1 ...` => `array(N+1) [tag, field1, ...]` where N = count of `To` fields
- StrictMaybe: `SNothing` = `array(0)`, `SJust x` = `array(1) [x]`
- Set (version >= 9): `tag(258) array(N) [...]` (or indefinite for N > 23)
- Seq / StrictSeq: `array(N) [...]` (no tag; indefinite for N > 23)
- Map: `map(N) { key: value, ... }`

## Top Level: ConwayGovState = array(7)

```
array(7)
  [0] Proposals
  [1] StrictMaybe Committee
  [2] Constitution
  [3] PParams (curPParams) — array(31)
  [4] PParams (prevPParams) — array(31)
  [5] FuturePParams — Sum type
  [6] DRepPulsingState — always serialized as DRComplete
```

## [0] Proposals

`encCBOR` of `Proposals` uses `encCBOR (roots, pProps)` which is a Haskell tuple:
```
array(2)
  [0] GovRelation StrictMaybe = array(4)
      [0] StrictMaybe (GovPurposeId 'PParamUpdate) — root
      [1] StrictMaybe (GovPurposeId 'HardFork) — root
      [2] StrictMaybe (GovPurposeId 'Committee) — root
      [3] StrictMaybe (GovPurposeId 'Constitution) — root
      Each: array(0) for SNothing, array(1) [GovActionId] for SJust
      GovActionId = array(2) [tx_hash_32bytes, gov_action_ix_u16]
  [1] OMap = array(N) [GovActionState, ...]  (list of proposals, insertion order)
```

## [5] FuturePParams — Sum type

```
NoPParamsUpdate      = array(1) [0]
DefinitePParamsUpdate= array(2) [1, PParams]
PotentialPParamsUpdate= array(2) [2, Maybe_PParams]
```

Where `Maybe PParams` = CBOR `null` for Nothing or `PParams` for Just.

## [6] DRepPulsingState

IMPORTANT: Both `DRComplete` and `DRPulsing` serialize as `DRComplete`!
The `DRPulsing` case finishes the pulser first, then writes as `DRComplete`.

Uses `Rec DRComplete`:
```
array(2)
  [0] PulsingSnapshot
  [1] RatifyState
```

## PulsingSnapshot = array(4)

```
array(4)
  [0] psProposals   — StrictSeq GovActionState = array(N) [GovActionState, ...]
  [1] psDRepDistr   — Map DRep (CompactForm Coin) = map(N) { drep: compact_coin, ... }
  [2] psDRepState   — Map (Credential DRepRole) DRepState = map(N) { credential: drep_state, ... }
  [3] psPoolDistr   — Map (KeyHash StakePool) (CompactForm Coin) = map(N) { pool_hash: compact_coin, ... }
```

Note field ordering: proposals FIRST, then drep_distr, drep_state, pool_distr.

## RatifyState = array(4)

```
array(4)
  [0] rsEnactState  — EnactState (array(7))
  [1] rsEnacted     — Seq GovActionState = array(N) [...] (NOT tag 258, it's Seq not Set)
  [2] rsExpired     — Set GovActionId = tag(258) array(N) [GovActionId, ...]
  [3] rsDelayed     — Bool
```

IMPORTANT: NO FuturePParams field in RatifyState! The FuturePParams is at ConwayGovState[5].

## EnactState = array(7)

```
array(7)
  [0] ensCommittee       — StrictMaybe Committee
  [1] ensConstitution    — Constitution
  [2] ensCurPParams      — PParams (array(31))
  [3] ensPrevPParams     — PParams (array(31))
  [4] ensTreasury        — Coin (integer)
  [5] ensWithdrawals     — Map (Credential Staking) Coin
  [6] ensPrevGovActionIds — GovRelation StrictMaybe = array(4) [...]
```

## Dugite Bugs Found (2026-03-17)

1. **PulsingSnapshot field order wrong**: Dugite encodes [drep_distr, map, map, pool_distr].
   Haskell is [proposals, drep_distr, drep_state, pool_distr].
2. **PulsingSnapshot field types wrong**: Field [0] should be StrictSeq(GovActionState), not map.
   Field [2] should be Map<Credential,DRepState>, not map.
3. **RatifyState structure completely wrong**: Dugite encodes [enacted, expired, delayed, future_pparams].
   Haskell is [EnactState(array(7)), enacted_seq, expired_set, delayed_bool].
   - First field should be EnactState (the entire enact state), not enacted proposals
   - NO FuturePParams in RatifyState
4. **rsEnacted uses tag(258)**: Wrong. `rsEnacted` is `Seq` not `Set`. Should be plain array, no tag.
5. **rsExpired encoding is OK**: Uses tag(258), which is correct for Set.
