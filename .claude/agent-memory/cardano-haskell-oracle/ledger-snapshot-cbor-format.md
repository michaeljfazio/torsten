---
name: ledger-snapshot-cbor-format
description: Complete CBOR format of ExtLedgerState disk snapshot (state file), as written by ouroboros-consensus and included in Mithril ancillary archives
type: reference
---

# ExtLedgerState Disk Snapshot CBOR Format

Verified against golden test files and source code. All findings confirmed with:
- `ouroboros-consensus-cardano/golden/cardano/disk/ExtLedgerState_Conway`
- `ouroboros-consensus-cardano/golden/cardano/disk/LedgerState_Conway`
- `ouroboros-consensus-cardano/golden/cardano/disk/ChainDepState_Conway`
- `ouroboros-consensus-cardano/golden/cardano/disk/AnnTip_Conway`
- `ouroboros-consensus-cardano/golden/cardano/disk/LedgerTables_Conway`

## Key Source Files

- `ouroboros-consensus/src/ouroboros-consensus/Ouroboros/Consensus/Ledger/Extended.hs` — `encodeDiskExtLedgerState`, `encodeExtLedgerState`
- `ouroboros-consensus/src/ouroboros-consensus/Ouroboros/Consensus/Storage/LedgerDB/Snapshots.hs` — `writeExtLedgerState`, `encodeL`, `snapshotEncodingVersion1`
- `ouroboros-consensus/src/ouroboros-consensus/Ouroboros/Consensus/HardFork/Combinator/Serialisation/Common.hs` — `encodeTelescope`, `encodeNS`, `encodeCurrent`, `encodePast`
- `ouroboros-consensus/src/ouroboros-consensus/Ouroboros/Consensus/HardFork/Combinator/Serialisation/SerialiseDisk.hs` — `EncodeDisk` instances for HFC
- `ouroboros-consensus-cardano/src/shelley/Ouroboros/Consensus/Shelley/Ledger/Ledger.hs` — `encodeShelleyLedgerState`, `serialisationFormatVersion2`
- `ouroboros-consensus-protocol/src/ouroboros-consensus-protocol/Ouroboros/Consensus/Protocol/Praos.hs` — `Serialise PraosState`
- `eras/shelley/impl/src/Cardano/Ledger/Shelley/LedgerState/Types.hs` — `EncCBOR NewEpochState`, `EncCBOR EpochState`, `EncCBOR LedgerState`, `EncCBOR UTxOState`

## Outer State File Format (on disk, in Mithril ancillary)

The `<snapshot_dir>/<slotno>/state` file contains:

```
array(2) [
  uint8(1),          -- snapshotEncodingVersion1 (from encodeVersion)
  <ExtLedgerState>   -- encodeDiskExtLedgerState result
]
```

The version wrapper comes from:
```haskell
encodeL encodeLedger l = encodeVersion snapshotEncodingVersion1 (encodeLedger l)
encodeVersion (VersionNumber 1) enc = encodeListLen 2 <> encode (VersionNumber 1) <> enc
```

The golden test files do NOT include this wrapper (they encode ExtLedgerState directly).

## ExtLedgerState Structure

```
array(2) [
  <LedgerState telescope>,   -- encodeDisk cfg ledgerState
  <HeaderState>              -- encodeHeaderState'
]
```

Source: `encodeExtLedgerState` in `Extended.hs`

## HeaderState Structure

```
array(2) [
  <WithOrigin(AnnTip)>,      -- encodeWithOrigin encodeAnnTip' headerStateTip
  <ChainDepState telescope>  -- encodeChainDepState headerStateChainDep
]
```

Source: `encodeHeaderState` in `HeaderValidation.hs`

### WithOrigin encoding
- `Origin` → `[]` (empty CBOR array, 0x80)
- `NotOrigin x` → `[x]` (1-element CBOR array)

### AnnTip for HFC (HardForkBlock)
Uses `encodeNS`:
```
array(2) [
  uint8(era_index),    -- 0=Byron, 1=Shelley, 2=Allegra, 3=Mary, 4=Alonzo, 5=Babbage, 6=Conway
  <era_specific_tip>   -- defaultEncodeAnnTip for Shelley-based eras
]
```

Shelley AnnTip (`defaultEncodeAnnTip`): **IMPORTANT - different order than ShelleyTip!**
```
array(3) [
  uint64(annTipSlotNo),   -- SlotNo
  bytes(32)(annTipHash),  -- HeaderHash
  uint64(annTipBlockNo)   -- BlockNo
]
```
**Order: slot, hash, blockNo**

### ChainDepState for Cardano (HardForkChainDepState)
Uses `encodeTelescope` — encodes as array where length encodes the era:
```
array(N) [                        -- N = era_index + 1 (e.g., N=7 for Conway)
  <Past1>,                        -- Byron era (past)
  <Past2>,                        -- Shelley era (past)
  ...                             -- each past era
  <Current(PraosState)>           -- current (last) era
]
```

Past encoding:
```
array(2) [
  <Bound_start>,   -- pastStart
  <Bound_end>      -- pastEnd
]
```

Current encoding:
```
array(2) [
  <Bound_start>,   -- currentStart
  <PraosState>     -- currentState (versioned)
]
```

Bound encoding:
```
array(3) [
  integer(relativeTime),  -- RelativeTime = Pico (picoseconds as integer)
  uint64(slotNo),         -- SlotNo
  uint64(epochNo)         -- EpochNo
]
```

### PraosState (version-wrapped)
```
array(2) [
  uint8(0),         -- encodeVersion 0
  array(8) [        -- PraosState fields
    <WithOrigin(SlotNo)>,         -- praosStateLastSlot
    map{bytes(28) -> uint64},     -- praosStateOCertCounters (KeyHash -> Word64)
    <Nonce>,                      -- praosStateEvolvingNonce
    <Nonce>,                      -- praosStateCandidateNonce
    <Nonce>,                      -- praosStateEpochNonce
    <Nonce>,                      -- praosStatePreviousEpochNonce
    <Nonce>,                      -- praosStateLabNonce
    <Nonce>                       -- praosStateLastEpochBlockNonce
  ]
]
```

Nonce encoding:
- `NeutralNonce` → `[0]` (array of tag 0)
- `Nonce hash32` → `[1, bytes(32)]`

## LedgerState for Cardano (HFC Telescope)

Encoded as telescope with `encodeTelescope`:
```
array(N) [               -- N = era_index + 1
  <Past1>,               -- Byron era past
  <Past2>,               -- Shelley era past
  ...
  <Current>              -- Conway era current
]
```

Current element:
```
array(2) [
  <Bound_start>,               -- currentStart
  <ShelleyLedgerState_versioned>  -- currentState
]
```

### ShelleyLedgerState (version-wrapped)

```
array(2) [
  uint8(2),              -- serialisationFormatVersion2
  array(4) [             -- ShelleyLedgerState fields
    <WithOrigin(ShelleyTip)>,                       -- shelleyLedgerTip
    <NewEpochState>,                                -- shelleyLedgerState
    uint32(shelleyAfterVoting),                     -- shelleyLedgerTransition
    <StrictMaybe(PerasRoundNo)>                     -- shelleyLedgerLatestPerasCertRound
  ]
]
```

**StrictMaybe encoding** (same as WithOrigin):
- `SNothing` → `[]`
- `SJust x` → `[x]`

**ShelleyTip encoding** (NOT same as AnnTip):
```
array(3) [
  uint64(shelleyTipSlotNo),   -- SlotNo
  uint64(shelleyTipBlockNo),  -- BlockNo
  bytes(32)(shelleyTipHash)   -- HeaderHash
]
```
**Order: slot, blockNo, hash** (blockNo before hash, opposite of AnnTip!)

## NewEpochState (Conway era)

```
array(7) [
  uint64(nesEL),                 -- EpochNo
  map{bytes(28)->uint64}(nesBprev),  -- BlocksMade (pool_id -> block_count)
  map{bytes(28)->uint64}(nesBcur),   -- BlocksMade
  <EpochState>,                  -- nesEs
  <StrictMaybe(PulsingRewUpdate)>, -- nesRu
  <PoolDistr>,                   -- nesPd
  null/undefined                 -- stashedAVVMAddresses (= () for Conway)
]
```

**stashedAVVMAddresses for Conway** = `()` in Haskell, encodes as CBOR `null` (0xf6)

## EpochState

```
array(4) [
  <ChainAccountState>,   -- esChainAccountState
  <LedgerState>,         -- esLState (encoded before snapshots for sharing)
  <SnapShots>,           -- esSnapshots
  <NonMyopic>            -- esNonMyopic
]
```

**ChainAccountState**: `array(2)[uint64(reserves), uint64(treasury)]`

## LedgerState (Shelley)

**CRITICAL**: CertState is encoded BEFORE UTxOState for sharing optimization.
```
array(2) [
  <CertState>,    -- lsCertState (FIRST for sharing)
  <UTxOState>     -- lsUTxOState (SECOND)
]
```

## UTxOState (Conway)

```
array(6) [
  map{...}(utxosUtxo),         -- UTxO (encoded with encodeMap encodeMemPack encodeMemPack)
  uint64(utxosDeposited),      -- Coin (deposits)
  uint64(utxosFees),           -- Coin (fees)
  <ConwayGovState>,            -- utxosGovState (array(7))
  <InstantStake>,              -- utxosInstantStake
  uint64(utxosDonation)        -- Coin (donations)
]
```

## CertState (Conway)

```
array(3) [
  <DState>,   -- certDState
  <PState>,   -- certPState
  <VState>    -- certVState
]
```

## PoolDistr (Conway V21+ format)

```
array(2) [
  map{bytes(28) -> <IndividualPoolStake>},  -- pool_map
  uint64(total_active_stake)                -- total stake
]
```

IndividualPoolStake:
```
array(3) [
  tag(30)[array(2)[numerator, denominator]],  -- pStake (Rational)
  uint64(compact_coin),                       -- pStakeVrf (or compact stake)
  bytes(32)                                   -- VRF verification key hash
]
```

Note: Pre-V21 GetPoolDistr used a different type without the total_active_stake field.

## Tables (tvar) File Format

The `<snapshot_dir>/<slotno>/tables/tvar` file:

```
toCBOR(WithOrigin(SlotNo)) || valuesMKEncoder(hint, values)
```

Where `valuesMKEncoder` = `array(1)[encodeTablesWithHint(hint, values)]`

For Conway era, `encodeTablesWithHint`:
```
encodeMap encodeMemPack encodeMemPack utxo_map
= map{TxIn_bytes -> TxOut_bytes}
```

TxIn MemPack encoding (TablesCodecVersion1, cardano-node 10.7+):
```
bytes(34) = txid(32) || be_uint16(txix)
```

Note: Prior to TablesCodecVersion1 (pre-10.7), no version was tracked in the `meta` file and TxIn was encoded differently.

TxOut is MemPack-encoded (compact binary format, NOT CBOR).

The `valuesMKDecoder` reads: `decodeListLenOf 1 >> decodeTablesWithHint`

## Meta File Format

The `<snapshot_dir>/<slotno>/meta` file is JSON:
```json
{
  "backend": "utxohd-mem",
  "checksum": <uint32_crc>,
  "tablesCodecVersion": 1
}
```

Backend values: `"utxohd-mem"`, `"utxohd-lmdb"`, `"utxohd-lsm"`

## V1 vs V2 LedgerDB

- **V1 (in-memory/LMDB)**: Used by Mithril ancillary snapshots. State file + tables/tvar file. `state` has the ExtLedgerState, `tables/` has the UTxO tables.
- **V2 (LSM)**: Future/experimental. Different on-disk format.

Mithril ancillary uses V1 in-memory backend (`utxohd-mem`).

## Snapshot Directory Layout

```
<cardano_db_dir>/ledger/<slotno>/
  ├── state       -- CBOR: array(2)[uint8(1), ExtLedgerState]
  ├── tables/
  │   └── tvar    -- CBOR: WithOrigin(SlotNo) || array(1)[map{TxIn->TxOut}]
  └── meta        -- JSON: {backend, checksum, tablesCodecVersion}
```

## Era Index Table (CardanoEras)

For `encodeNS` and `encodeTelescope`:
- 0 = Byron
- 1 = Shelley
- 2 = Allegra
- 3 = Mary
- 4 = Alonzo
- 5 = Babbage
- 6 = Conway
- 7 = Dijkstra (future)
