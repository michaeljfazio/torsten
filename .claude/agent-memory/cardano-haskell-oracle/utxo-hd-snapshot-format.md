# UTxO-HD In-Memory Backend Ledger State Snapshot Format

## Overview
cardano-node 10.x uses UTxO-HD with the in-memory backend by default.
Snapshots are written to a directory containing:
- `state` file — the ExtLedgerState (with EMPTY UTxO)
- `tables` file — the UTxO map encoded separately
- `meta` file — JSON: `{snapshotBackend: "UTxOHDMemSnapshot", snapshotChecksum, snapshotTablesCodecVersion: 1}`

## Key Insight: UTxO is EMPTY in the state file
The V2 InMemory backend stores `ExtLedgerState blk EmptyMK`.
The UTxO field inside `NewEpochState > EpochState > LedgerState > UTxOState > utxosUtxo`
is an **empty CBOR map** (0xa0). The actual UTxO is in the separate `tables` file.

## State File Wire Format (outermost to innermost)

### Layer 1: Version wrapper
```
array(2) [
  Word8(1),                    -- snapshotEncodingVersion1
  <ExtLedgerState encoding>
]
```
Source: `Ouroboros.Consensus.Storage.LedgerDB.Snapshots.encodeL`

### Layer 2: ExtLedgerState
```
array(2) [
  <LedgerState_HFC_telescope>,
  <HeaderState encoding>
]
```
Source: `Ouroboros.Consensus.Ledger.Extended.encodeExtLedgerState`

### Layer 3a: LedgerState HFC Telescope (Conway = era index 6)
Array length = 1 + era_index. For Conway: array(7).
Era index is NOT explicit — it's implied by array length.
```
array(7) [
  <Past_0>,  -- Byron
  <Past_1>,  -- Shelley
  <Past_2>,  -- Allegra
  <Past_3>,  -- Mary
  <Past_4>,  -- Alonzo
  <Past_5>,  -- Babbage
  <Current_6>  -- Conway (current era)
]
```

Each Past:
```
array(2) [Bound_start, Bound_end]
```

Current (era 6):
```
array(2) [Bound_start, <per-era LedgerState encoding>]
```

Source: `Ouroboros.Consensus.HardFork.Combinator.Serialisation.Common.encodeTelescope`
Source: `Ouroboros.Consensus.HardFork.Combinator.State.Instances.encodeCurrent/encodePast`

### Bound encoding
```
array(3) [RelativeTime, SlotNo, EpochNo]
```
(array(4) with PerasRoundNo if Peras enabled — not yet active)

### Layer 4: Per-era LedgerState (Shelley-family)
Another version wrapper:
```
array(2) [
  Word8(2),                    -- serialisationFormatVersion2
  array(3) [
    <WithOrigin ShelleyTip>,   -- shelleyLedgerTip
    <NewEpochState>,           -- shelleyLedgerState (toCBOR)
    Word32                     -- shelleyLedgerTransition (shelleyAfterVoting)
  ]
]
```
Source: `Ouroboros.Consensus.Shelley.Ledger.Ledger.encodeShelleyLedgerState`

WithOrigin encoding:
- Origin: array(0) []
- NotOrigin tip: array(1) [ array(3) [SlotNo, BlockNo, HeaderHash] ]

### Layer 5: NewEpochState (cardano-ledger)
```
array(7) [
  EpochNo,           -- nesEL
  BlocksMade,        -- nesBprev (map: KeyHash_Pool -> Natural)
  BlocksMade,        -- nesBcur
  EpochState,        -- nesEs
  StrictMaybe PulsingRewUpdate,  -- nesRu
  PoolDistr,         -- nesPd
  StashedAVVMAddresses  -- (empty in Conway)
]
```
Source: `Cardano.Ledger.Shelley.LedgerState.Types` EncCBOR instance

### Layer 6: EpochState
```
array(4) [
  ChainAccountState,  -- array(2) [treasury_coin, reserves_coin]
  LedgerState_ledger, -- array(2) [CertState, UTxOState]
  SnapShots,
  NonMyopic
]
```

### Layer 7: UTxOState (EMPTY UTxO in UTxO-HD)
```
array(6) [
  UTxO,              -- EMPTY MAP (0xa0) in UTxO-HD in-memory backend
  Coin,              -- utxosDeposited
  Coin,              -- utxosFees
  GovState,          -- utxosGovState (Conway: ConwayGovState)
  InstantStake,      -- utxosInstantStake
  Coin               -- utxosDonation
]
```

### Layer 3b: HeaderState encoding
```
array(2) [
  <WithOrigin AnnTip>,         -- headerStateTip
  <ChainDepState>              -- headerStateChainDep
]
```

For HFC, AnnTip is encoded as NS (n-ary sum):
```
array(2) [Word8(era_index), <per-era AnnTip>]
```

Per-era AnnTip (Shelley-family):
```
array(3) [SlotNo, HeaderHash, BlockNo]
```

ChainDepState for HFC is also encoded as a telescope (same structure as LedgerState telescope).

## Source Files
- Version wrapper: ouroboros-consensus/.../Storage/LedgerDB/Snapshots.hs
- ExtLedgerState: ouroboros-consensus/.../Ledger/Extended.hs
- Telescope: ouroboros-consensus/.../HardFork/Combinator/Serialisation/Common.hs
- Current/Past: ouroboros-consensus/.../HardFork/Combinator/State/Instances.hs
- Shelley LedgerState: ouroboros-consensus-cardano/src/shelley/.../Shelley/Ledger/Ledger.hs
- NewEpochState: cardano-ledger/eras/shelley/impl/src/.../Shelley/LedgerState/Types.hs
- V2 InMemory: ouroboros-consensus/.../Storage/LedgerDB/V2/InMemory.hs
- EncodeDisk HFC: ouroboros-consensus/.../HardFork/Combinator/Serialisation/SerialiseDisk.hs
