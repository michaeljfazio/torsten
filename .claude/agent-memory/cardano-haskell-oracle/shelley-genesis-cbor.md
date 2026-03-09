# Shelley CompactGenesis CBOR Encoding

## Source Files
- ShelleyGenesis type: `cardano-ledger/eras/shelley/impl/src/Cardano/Ledger/Shelley/Genesis.hs`
- ShelleyPParams type: `cardano-ledger/eras/shelley/impl/src/Cardano/Ledger/Shelley/PParams.hs`
- CompactGenesis newtype: `ouroboros-consensus-cardano/src/shelley/Ouroboros/Consensus/Shelley/Ledger/Config.hs`
- GetGenesisConfig query: tag 11 in `ouroboros-consensus-cardano/src/shelley/Ouroboros/Consensus/Shelley/Ledger/Query.hs`
- Legacy encoding: `ouroboros-consensus-cardano/src/shelley/Ouroboros/Consensus/Shelley/Ledger/Query/LegacyShelleyGenesis.hs`
- Legacy PParams: `ouroboros-consensus-cardano/src/shelley/Ouroboros/Consensus/Shelley/Ledger/Query/LegacyPParams.hs`
- UTCTime encoding: `cardano-ledger-binary/src/Cardano/Ledger/Binary/Encoding/Encoder.hs` line 542
- Rational encoding: `cardano-ledger-binary/src/Cardano/Ledger/Binary/Encoding/Encoder.hs` line 290

## Version Split
- N2C V16-V20 (ShelleyNodeToClientVersion8-12): **legacy** encoding via LegacyShelleyGenesis
- N2C V21+ (ShelleyNodeToClientVersion13+): **new** encoding via standard ToCBOR ShelleyGenesis

## CompactGenesis = ShelleyGenesis with sgInitialFunds=empty, sgStaking=empty

## Top-Level: array(15)
Both legacy and new encode the 15 top-level fields identically (only PParams differs):

| Idx | Field               | Type                  | CBOR Encoding                           |
|-----|---------------------|-----------------------|-----------------------------------------|
| 0   | sgSystemStart       | UTCTime               | array(3) [year:int, dayOfYear:int, picoseconds:int] |
| 1   | sgNetworkMagic      | Word32                | uint                                    |
| 2   | sgNetworkId         | Network               | uint (Testnet=0, Mainnet=1)             |
| 3   | sgActiveSlotsCoeff  | PositiveUnitInterval  | array(2) [numerator:uint, denominator:uint] NO tag(30) |
| 4   | sgSecurityParam     | NonZero Word64        | uint                                    |
| 5   | sgEpochLength       | Word64 (unEpochSize)  | uint                                    |
| 6   | sgSlotsPerKESPeriod | Word64                | uint                                    |
| 7   | sgMaxKESEvolutions  | Word64                | uint                                    |
| 8   | sgSlotLength        | NominalDiffTimeMicro  | int (underlying Fixed E6 integer, e.g. 1000000 = 1 second) |
| 9   | sgUpdateQuorum      | Word64                | uint                                    |
| 10  | sgMaxLovelaceSupply | Word64                | uint                                    |
| 11  | sgProtocolParams    | PParams ShelleyEra    | see below (differs by version)          |
| 12  | sgGenDelegs         | Map KeyHash28 GenDelegPair | CBOR map                           |
| 13  | sgInitialFunds      | ListMap Addr Coin     | CBOR map (empty for CompactGenesis)     |
| 14  | sgStaking           | ShelleyGenesisStaking | array(2) [pools_map, stake_map] (both empty) |

## activeSlotsCoeff (field 3): NO TAG(30)
Forced to shelleyProtVer (version 2) via enforceEncodingVersion.
At version 2, encodeRatio uses encodeRatioNoTag: array(2) [numerator, denominator].
For 1/20: `[1, 20]`

## UTCTime (field 0): array(3)
```
encodeListLen 3
  <> encodeInteger year        -- Gregorian year
  <> encodeInt dayOfYear       -- 1-based day of year from toOrdinalDate
  <> encodeInteger picoseconds -- diffTimeToPicoseconds of day time
```

## NominalDiffTimeMicro (field 8)
Newtype over Micro (Fixed E6). EncCBOR derives from Integer.
Value is seconds * 10^6. So 1.0 second = CBOR integer 1000000.

## Legacy PParams (N2C V16-V20): array(18)
Fields in order, ProtVer split into major+minor:
| Idx | Field            | Type                    |
|-----|------------------|------------------------|
| 0   | txFeePerByte     | CoinPerByte (Word64)    |
| 1   | txFeeFixed       | CompactCoin (Word64)    |
| 2   | maxBBSize        | Word32                  |
| 3   | maxTxSize        | Word32                  |
| 4   | maxBHSize        | Word16                  |
| 5   | keyDeposit       | CompactCoin (Word64)    |
| 6   | poolDeposit      | CompactCoin (Word64)    |
| 7   | eMax             | EpochInterval (Word32)  |
| 8   | nOpt             | Word16                  |
| 9   | a0               | NonNegativeInterval (tag30 [num, den]) |
| 10  | rho              | UnitInterval (tag30 [num, den])        |
| 11  | tau              | UnitInterval (tag30 [num, den])        |
| 12  | d                | UnitInterval (tag30 [num, den])        |
| 13  | extraEntropy     | Nonce ([1] or [2, hash32])             |
| 14  | protocolVersion major | Version (uint)                     |
| 15  | protocolVersion minor | Natural (uint)                     |
| 16  | minUTxOValue     | CompactCoin (Word64)    |
| 17  | minPoolCost      | CompactCoin (Word64)    |

## New PParams (N2C V21+): array(17)
Standard eraPParams encoding via shelleyPParams list:
| Idx | Field            | Type                    |
|-----|------------------|------------------------|
| 0   | txFeePerByte     | CoinPerByte (Word64)    |
| 1   | txFeeFixed       | CompactCoin (Word64)    |
| 2   | maxBBSize        | Word32                  |
| 3   | maxTxSize        | Word32                  |
| 4   | maxBHSize        | Word16                  |
| 5   | keyDeposit       | CompactCoin (Word64)    |
| 6   | poolDeposit      | CompactCoin (Word64)    |
| 7   | eMax             | EpochInterval (Word32)  |
| 8   | nOpt             | Word16                  |
| 9   | a0               | NonNegativeInterval (tag30 [num, den]) |
| 10  | rho              | UnitInterval (tag30 [num, den])        |
| 11  | tau              | UnitInterval (tag30 [num, den])        |
| 12  | d                | UnitInterval (tag30 [num, den])        |
| 13  | extraEntropy     | Nonce                                  |
| 14  | protocolVersion  | ProtVer via CBORGroup = array(2) [major, minor] |
| 15  | minUTxOValue     | CompactCoin (Word64)    |
| 16  | minPoolCost      | CompactCoin (Word64)    |

## BoundedRational in PParams: tag(30) IS used
PParams encoding uses current protocol version (not forced to shelleyProtVer).
At version >= 9, BoundedRatio encodes as: tag(30) array(2) [numerator:Word64, denominator:Word64]

## GenDelegPair encoding
Map key: KeyHash GenesisRole = 28-byte hash (encCBOR = bytes)
Map value: array(2) [delegate_keyhash_28, vrf_hash_32]

## Nonce encoding
NeutralNonce: array(1) [0]
Nonce hash:   array(2) [1, hash_32_bytes]

## Torsten Bug (as of 2026-03-09)
Current encoding uses JSON-like map(5) with string keys. Must be CBOR array(15) per ShelleyGenesis.
