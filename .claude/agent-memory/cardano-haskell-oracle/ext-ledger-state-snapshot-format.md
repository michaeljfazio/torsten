---
name: ExtLedgerState Snapshot Format
description: Complete CBOR encoding of ouroboros-consensus ExtLedgerState disk snapshot (state file)
type: reference
---

# ExtLedgerState Snapshot Format (ouroboros-consensus main, 2026-04)

Source CDDL: `ouroboros-consensus-cardano/cddl/disk/ledger/stateFile.cddl`
Source code: `ouroboros-consensus/src/.../Storage/LedgerDB/Snapshots.hs`, `Ledger/Extended.hs`, etc.

## 1. Snapshot Wrapper (stateFile)

```
ledgerStateSnapshot = [snapshotEncodingVersion1, extLedgerState]
snapshotEncodingVersion1 = 1
```

`encodeVersion vn enc = array(2) [word8(vn), enc]`
(from `Ouroboros.Consensus.Util.Versioned`)

So the outer wrapper is: **array(2) [1, extLedgerState]**
Written by `writeExtLedgerState` → `encodeL encLedger` → `encodeVersion 1 (encodeLedger l)`

## 2. ExtLedgerState

`extLedgerState = [ledgerState, headerState]`

```haskell
-- Extended.hs
encodeExtLedgerState ... ExtLedgerState{ledgerState, headerState} =
    encodeListLen 2
    <> encodeLedgerState ledgerState
    <> encodeHeaderState' headerState
```

**array(2) [ledgerState, headerState]**

## 3. HeaderState

```
headerState = [withOrigin<headerStateTip>, headerStateChainDep]
```

```haskell
-- HeaderValidation.hs
encodeHeaderState encodeChainDepState encodeAnnTip' HeaderState{..} =
    encodeListLen 2
    <> encodeWithOrigin encodeAnnTip' headerStateTip  -- WithOrigin (AnnTip blk)
    <> encodeChainDepState headerStateChainDep
```

**array(2) [withOrigin<annTip>, chainDepState_telescope]**

- `withOrigin<v>` = `[] / [v]` (encodeMaybe = array(0) or array(1)[v])
- For Cardano HFC: `chainDepState` is a telescope over all eras

## 4. HFC Telescope (HardForkChainDepState / HardForkLedgerState)

CDDL for Conway era node (8 eras: Byron, Shelley, Allegra, Mary, Alonzo, Babbage, Conway, Dijkstra):

```
telescope8<...> =
  [pastEra, pastEra, pastEra, pastEra, pastEra, pastEra, currentEra<conway>]  -- Conway active
```

`encodeTelescope` logic:
```haskell
encodeTelescope es (HardForkState st) =
    encodeListLen (1 + fromIntegral ix)  -- ix = zero-based index of current era
    <> mconcat (past encodings) <> current encoding
```

- Past era = `encodePast Past{pastStart, pastEnd}` = **array(2) [bound, bound]**
- Current era = `encodeCurrent f Current{currentStart, currentState}` = **array(2) [bound, era_state]**
- For Conway (era index 6, 0-based): array length = **7**
- For Dijkstra (era index 7): array length = **8**

```
pastEra    = [bound, bound]
currentEra = [bound, era_state]
bound      = [relativeTime, slotno, epochno]  -- array(3)
             ; NOTE: if Peras is enabled, bound = array(4) [relTime, slotno, epochno, perasRoundNo]
relativeTime = int (picoseconds as integer)
slotno = word64
epochno = word64
```

IMPORTANT: `Bound` has a 4th optional field `boundPerasRound`. The CDDL says array(3) but current
Haskell code (Summary.hs) encodes 3 elements for `NoPerasEnabled` and 4 for `PerasEnabled`.
For all current Cardano era bounds (pre-Peras), it is always array(3).

## 5. HFC LedgerState Telescope (per-era Shelley ledger states)

```
ledgerState = telescope8<byron.ledgerstate, versionedShelleyLedgerState<shelley.ledgerstate>, ...>
```

The per-era Shelley ledger state wrapper:
```
versionedShelleyLedgerState<eraSt> = [shelleyVersion2, shelleyLedgerState<eraSt>]
shelleyVersion2 = 2
```

This is `encodeVersion 2 (encodeListLen 4 <> ...)` inside `currentEra`:
```
shelleyLedgerState<eraSt> = [withOrigin<shelleyTip>, eraSt, shelleyTransition, latestPerasCertRound]
shelleyTip = [slotno, blockno, hash]  -- array(3)
shelleyTransition = word32
latestPerasCertRound = [] / [roundno]  -- StrictMaybe PerasRoundNo
```

CRITICAL: The ShelleyLedgerState encodes **4 fields** (not 3):
1. `withOrigin<shelleyTip>` — tip
2. `NewEpochState` — full ledger state
3. `shelleyTransition` — word32 (afterVoting count)
4. `latestPerasCertRound` — StrictMaybe PerasRoundNo = [] or [roundno]

The data type has 5 fields but `shelleyLedgerTables` is NOT serialized (EmptyMK = phantom).

`ShelleyTip` = array(3) [slotno, blockno, hash]

## 6. PraosState (HFC ChainDepState for Shelley/Allegra/Mary/Alonzo/Babbage/Conway/Dijkstra)

CDDL:
```
versionedPraosState = [praosVersion, praosState]
praosVersion = 0
praosState = [withOrigin<slotno>, {* keyhash => word64}, nonce, nonce, nonce, nonce, nonce, nonce]
```

Haskell (Praos.hs, Serialise instance):
```haskell
encode PraosState{...} =
    encodeVersion 0 $   -- array(2) [0, array(8)[...]]
      encodeListLen 8
      <> toCBOR praosStateLastSlot           -- withOrigin<slotno>
      <> toCBOR praosStateOCertCounters      -- {* keyhash => word64}  (Map)
      <> toCBOR praosStateEvolvingNonce      -- nonce
      <> toCBOR praosStateCandidateNonce     -- nonce
      <> toCBOR praosStateEpochNonce         -- nonce
      <> toCBOR praosStatePreviousEpochNonce -- nonce  ← 6th field!
      <> toCBOR praosStateLabNonce           -- nonce
      <> toCBOR praosStateLastEpochBlockNonce-- nonce
```

**CORRECTION**: PraosState has **8 fields** (NOT 7). There is a `praosStatePreviousEpochNonce`
field between `praosStateEpochNonce` and `praosStateLabNonce`. Your memory of "array(7)" is WRONG.

Full field order in the array(8):
1. praosStateLastSlot (WithOrigin SlotNo)
2. praosStateOCertCounters (Map KeyHash Word64)
3. praosStateEvolvingNonce
4. praosStateCandidateNonce
5. praosStateEpochNonce
6. praosStatePreviousEpochNonce  ← THE MISSING FIELD
7. praosStateLabNonce
8. praosStateLastEpochBlockNonce

## 7. Nonce Encoding

```
nonce = [0] / [1, hash]
```

```haskell
-- cardano-ledger BaseTypes.hs
encCBOR NeutralNonce = encodeListLen 1 <> encCBOR (0 :: Word8)
encCBOR (Nonce n)    = encodeListLen 2 <> encCBOR (1 :: Word8) <> encCBOR n
```

- NeutralNonce: **array(1) [0]**  (NOT bare integer 0)
- Nonce hash:   **array(2) [1, bytes(32)]**

## 8. WithOrigin Encoding

```
withOrigin<v> = [] / [v]
```

Via `encodeWithOrigin f = encodeMaybe f . withOriginToMaybe`
Via `encodeMaybe`: Nothing → array(0) [], Just x → array(1) [x]

- Origin: **array(0) []**
- At v:   **array(1) [v]**

## 9. headerStateChainDep Telescope

For Cardano (8 eras), the chain dep state telescope for Conway looks like:
```
array(7) [
  array(2)[bound,bound],  -- Byron past (PBFTState)
  array(2)[bound,bound],  -- Shelley past (TPraosState)
  array(2)[bound,bound],  -- Allegra past (TPraosState)
  array(2)[bound,bound],  -- Mary past (TPraosState)
  array(2)[bound,bound],  -- Alonzo past (TPraosState)
  array(2)[bound,bound],  -- Babbage past (PraosState)
  array(2)[bound, versionedPraosState]  -- Conway current
]
```

The Conway `currentEra` contains the PraosState, which is then wrapped in `encodeVersion 0`:
```
array(2)[bound, array(2)[0, array(8)[...praosState fields...]]]
```

## Full Structure Summary (Conway active)

```
state file =
  array(2)[
    1,   -- snapshot version
    array(2)[  -- ExtLedgerState
      -- LedgerState telescope (length=7 for Conway)
      array(7)[
        byron_past,
        shelley_past,
        allegra_past,
        mary_past,
        alonzo_past,
        babbage_past,
        array(2)[bound, array(2)[2, array(4)[  -- Conway current, version 2
          withOrigin<shelleyTip>,
          NewEpochState_CBOR,
          word32,
          maybe<roundno>
        ]]]
      ],
      -- HeaderState
      array(2)[
        withOrigin<annTip>,  -- AnnTip telescope (ns8 encoding)
        -- ChainDepState telescope (length=7 for Conway)
        array(7)[
          byron_past,
          shelley_past,
          allegra_past,
          mary_past,
          alonzo_past,
          babbage_past,
          array(2)[bound, array(2)[0, array(8)[  -- Conway PraosState, version 0
            withOrigin<slotno>,
            {* keyhash => word64},
            nonce, nonce, nonce, nonce, nonce, nonce
          ]]]
        ]
      ]
    ]
  ]
```

## Key Bugs to Fix in Dugite

1. PraosState: must be **8 nonces** (has `praosStatePreviousEpochNonce` as field 6)
2. PraosState outer wrapper: `array(2)[0, array(8)[...]]` NOT bare array(8)
3. ShelleyLedgerState: **4 fields** not 3: tip, NewEpochState, transition, latestPerasCertRound
4. NeutralNonce: `array(1)[0]` not bare `0`
5. Telescope length: for Conway = 7 (6 past + 1 current), not 7 eras

## Source Files

- `ouroboros-consensus/src/.../Storage/LedgerDB/Snapshots.hs` — writeExtLedgerState, encodeL
- `ouroboros-consensus/src/.../Ledger/Extended.hs` — encodeExtLedgerState/encodeDiskExtLedgerState
- `ouroboros-consensus/src/.../HeaderValidation.hs` — encodeHeaderState
- `ouroboros-consensus/src/.../HardFork/Combinator/Serialisation/Common.hs` — encodeTelescope
- `ouroboros-consensus/src/.../HardFork/Combinator/State/Instances.hs` — encodeCurrent/encodePast
- `ouroboros-consensus/src/.../HardFork/History/Summary.hs` — Bound Serialise instance
- `ouroboros-consensus/src/.../Util/Versioned.hs` — encodeVersion
- `ouroboros-consensus/src/.../Util/CBOR.hs` — encodeWithOrigin (= encodeMaybe)
- `ouroboros-consensus-cardano/src/shelley/.../Shelley/Ledger/Ledger.hs` — encodeShelleyLedgerState
- `ouroboros-consensus-protocol/src/.../Protocol/Praos.hs` — PraosState Serialise
- `cardano-ledger/libs/cardano-ledger-core/src/.../BaseTypes.hs` — Nonce EncCBOR
- `ouroboros-consensus-cardano/cddl/disk/ledger/` — CDDL specs (authoritative)
