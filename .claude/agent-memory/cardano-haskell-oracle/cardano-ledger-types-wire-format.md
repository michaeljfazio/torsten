---
name: Cardano Ledger Types and Wire Format Reference
description: Comprehensive reference for all hash types, key types, address formats, value types, scripts, datums, protocol parameters, and CBOR encoding in cardano-ledger
type: reference
---

# Cardano Ledger: Complete Types and Wire Format Reference

Sourced from IntersectMBO/cardano-ledger master branch, 2026-03-28.

---

## 1. Hash Types

### Hashing Algorithms

Defined in `libs/cardano-ledger-core/src/Cardano/Ledger/Hashes.hs`:

```haskell
type HASH    = Hash.Blake2b_256   -- 32 bytes: used for everything EXCEPT addresses
type ADDRHASH = Hash.Blake2b_224  -- 28 bytes: used for address credential hashes
```

**`standardHashSize` = 32** (Blake2b-256 output)
**`standardAddrHashSize` = 28** (Blake2b-224 output)

### Hash Type Inventory

| Haskell Type | Algorithm | Bytes | What it hashes |
|---|---|---|---|
| `KeyHash r` | Blake2b-224 | 28 | Ed25519 VerKeyDSIGN bytes |
| `ScriptHash` | Blake2b-224 | 28 | prefix_byte \|\| script_bytes |
| `SafeHash i` (used as TxId, DataHash, etc.) | Blake2b-256 | 32 | serialized bytes |
| `TxId` = `SafeHash EraIndependentTxBody` | Blake2b-256 | 32 | tx body CBOR bytes |
| `DataHash` = `SafeHash EraIndependentData` | Blake2b-256 | 32 | Plutus Data CBOR |
| `TxAuxDataHash` | Blake2b-256 | 32 | aux data bytes |
| `HashHeader` = `Hash HASH EraIndependentBlockHeader` | Blake2b-256 | 32 | block header bytes |
| `VRFVerKeyHash r` | Blake2b-256 | 32 | VRF VerKey bytes |
| `GenDelegPair.genDelegKeyHash` | Blake2b-224 | 28 | genesis delegate VKey |
| `GenDelegPair.genDelegVrfHash` | Blake2b-256 | 32 | VRF verification key |

### KeyHash Roles (phantom types)

```haskell
type data KeyRole
  = GenesisRole        -- genesis key
  | GenesisDelegate    -- genesis delegate key
  | Payment            -- payment credential key
  | Staking            -- staking credential key
  | StakePool          -- pool cold key (PoolId = KeyHash StakePool)
  | BlockIssuer        -- block issuer key
  | Witness            -- witness key (coercible from any role)
  | DRepRole           -- DRep key
  | HotCommitteeRole   -- committee hot key
  | ColdCommitteeRole  -- committee cold key
  | Guard              -- guard key
```

`PoolId` is NOT a separate newtype — it is `KeyHash StakePool` which is 28 bytes (Blake2b-224 of Ed25519 cold key).

### Script Hash Computation

```haskell
hashScript :: forall era. EraScript era => Script era -> ScriptHash
hashScript =
  ScriptHash . Hash.castHash . Hash.hashWith
    (\x -> scriptPrefixTag @era x <> originalBytes x)
```

Script prefix tags (the single byte prepended before hashing):

| Script type | Prefix byte |
|---|---|
| Native (MultiSig/Timelock) | `0x00` (`nativeMultiSigTag = "\x00"`) |
| Plutus V1 | `0x01` |
| Plutus V2 | `0x02` |
| Plutus V3 | `0x03` |
| Plutus V4 | `0x04` |

The hash algorithm for ScriptHash is **Blake2b-224** (ADDRHASH) applied to `prefix_byte || original_script_bytes`.

Result is 28 bytes stored as `ScriptHash (Hash ADDRHASH EraIndependentScript)`.

### Byron AddressHash

`AddressHash` = Blake2b-224 of the canonical CBOR encoding of `(AddrType, AddrSpendingData, Attributes AddrAttributes)`.

---

## 2. Key Types

### Signing/Verification Keys

`DSIGN = Ed25519DSIGN` (defined in `Keys/Internal.hs`)

```haskell
type DSIGN = DSIGN.Ed25519DSIGN

newtype VKey (kd :: KeyRole) = VKey { unVKey :: DSIGN.VerKeyDSIGN DSIGN }
-- VerKeyDSIGN Ed25519DSIGN = 32-byte Ed25519 public key
```

**Wire format**: VKey serializes as raw 32 bytes (Ed25519 public key). `DSIGN.encodeVerKeyDSIGN` / `DSIGN.decodeVerKeyDSIGN`.

`SignKeyDSIGN Ed25519DSIGN` = 64-byte Ed25519 private key (or 32-byte seed depending on implementation).

`SignedDSIGN DSIGN a` = 64-byte Ed25519 signature.

### VRF Keys

`VerKeyVRF` and `SignKeyVRF` — from `cardano-crypto-class`. Cardano uses `PraosVRF` = ECVRF-ED25519-SHA512-Elligator2.

`VRFVerKeyHash r` = `Hash HASH (VRF.VerKeyVRF v)` = Blake2b-256 of the raw VRF verification key bytes (32 bytes result).

### KES Keys

`Sum6KES` — depth-6 sum composition of Ed25519 key pairs. Total periods = 2^6 = 64.

`VerKeyKES` = 32 bytes (Ed25519 public key at current period).
`SignKeyKES` = 612 bytes for Sum6KES (period counter + forward-secure key material).
`SigKES` = Sum6KES signature.

### Key Hashing

```haskell
hashKey :: VKey kd -> KeyHash kd
hashKey (VKey vk) = KeyHash $ DSIGN.hashVerKeyDSIGN vk
-- DSIGN.hashVerKeyDSIGN uses Blake2b-224 (ADDRHASH)
-- Result: 28 bytes
```

---

## 3. Address Types

Source: `libs/cardano-ledger-core/src/Cardano/Ledger/Address.hs`

### Addr Type

```haskell
data Addr
  = Addr Network (Credential Payment) StakeReference
  | AddrBootstrap BootstrapAddress
```

```haskell
data Network = Testnet | Mainnet  -- Testnet=0, Mainnet=1
```

```haskell
data Credential (kr :: KeyRole)
  = ScriptHashObj !ScriptHash   -- 28-byte script hash
  | KeyHashObj !(KeyHash kr)    -- 28-byte key hash
```

```haskell
data StakeReference
  = StakeRefBase !(Credential Staking)   -- base address
  | StakeRefPtr !Ptr                     -- pointer address
  | StakeRefNull                         -- enterprise address
```

### Address Header Byte

The header byte encodes address type and network:

```
bit 7 (0x80): always 0 for Shelley addresses (Byron starts with 0x82)
bit 6 (0x40): NOT base address (1 = enterprise or pointer)
bit 5 (0x20): enterprise address (when bit 6 is set) OR stake cred is script (when bit 6 is clear)
bit 4 (0x10): payment cred is script
bit 0 (0x01): network id (0=Testnet, 1=Mainnet)
```

Address type encodings:

| Type | Header bits [7:0] | Notes |
|---|---|---|
| Base, key-pay, key-stake | `0b0000_000n` | `0x00` / `0x01` |
| Base, script-pay, key-stake | `0b0001_000n` | `0x10` / `0x11` |
| Base, key-pay, script-stake | `0b0010_000n` | `0x20` / `0x21` |
| Base, script-pay, script-stake | `0b0011_000n` | `0x30` / `0x31` |
| Pointer, key-pay | `0b0100_000n` | `0x40` / `0x41` |
| Pointer, script-pay | `0b0101_000n` | `0x50` / `0x51` |
| Enterprise, key-pay | `0b0110_000n` | `0x60` / `0x61` |
| Enterprise, script-pay | `0b0111_000n` | `0x70` / `0x71` |
| Reward/Account, key-stake | `0b1110_000n` | `0xE0` / `0xE1` |
| Reward/Account, script-stake | `0b1111_000n` | `0xF0` / `0xF1` |
| Byron | `0x82` | always — CBOR list len 2 |

Where `n` = 0 for testnet, 1 for mainnet.

### Address Binary Layout

```
Base address:       [header 1B] [payment_cred 28B] [stake_cred 28B]    = 57 bytes total
Enterprise address: [header 1B] [payment_cred 28B]                      = 29 bytes
Pointer address:    [header 1B] [payment_cred 28B] [variable-len ptr]   = 29+ bytes
Reward/Account:     [header 1B] [stake_cred 28B]                        = 29 bytes
```

Payment and stake credential bytes are the raw 28-byte hash (either KeyHash or ScriptHash, both are 28 bytes via ADDRHASH).

### Pointer Address Encoding

`Ptr = Ptr SlotNo32 TxIx CertIx`

Each field is encoded as a **variable-length big-endian Word64** (CBOR-style but NOT actual CBOR — it is a custom 7-bit encoding):
- Each byte: high bit = more bytes follow (1) or last byte (0); low 7 bits = data
- The bytes are big-endian (most significant group first, matching the word64ToWord7s function)

### Reward/Account Address

```haskell
data AccountAddress = AccountAddress
  { aaNetworkId :: !Network
  , aaId :: !AccountId  -- wraps Credential Staking
  }
-- header: 0xE0 (key) or 0xF0 (script) | network_bit
-- body:   28-byte credential hash
```

### Byron Address

Full CBOR-encoded Byron `Address` type — `[CRC32, payload_bytes]` where `payload_bytes` is CBOR of `(AddressHash, Attributes, AddrType)`.

`AddressHash` = Blake2b-224 of canonical CBOR of `Address'`.

---

## 4. Value Types

### Coin

```haskell
newtype Coin = Coin { unCoin :: Integer }
```

- CBOR: encoded as a **CBOR uint** (major type 0). Uses `ToCBOR` which calls `encodeInteger`.
- `CompactForm Coin` = `CompactCoin Word64` — serializes as uint64.
- On-wire in transactions: always `Word64` range (non-negative), but type is `Integer`.

### MaryValue (Mary/Alonzo/Babbage/Conway era multi-asset value)

```haskell
data MaryValue = MaryValue !Coin !MultiAsset
```

**CBOR encoding**:
```
-- ADA only (MultiAsset is empty):
  encCBOR coin   -- plain integer (uint)

-- Multi-asset:
  [coin, {policy_id => {asset_name => amount}}]
  -- array(2)[uint, map]
```

The `Rec` encoder produces `array(2)[coin, multi_asset_map]`.

### MultiAsset

```haskell
newtype MultiAsset = MultiAsset (Map PolicyID (Map AssetName Integer))
```

**PolicyID** = `ScriptHash` = 28 bytes, encoded as CBOR bytes(28).

**AssetName** = up to 32 bytes, encoded as CBOR bytes (0–32 bytes).

**Amount** = Integer (can be negative for minting/burning; bounded to Word64 range for UTxO values).

**MultiAsset CBOR** = `map { bytes(28) => map { bytes(0..32) => int } }`.

Canonical form requirements (Conway, decoder version ≥ 9):
- Maps must not contain zero amounts.
- Maps must not be empty.

### MaryValue vs ConwayValue

There is NO separate `AlonzoValue` or `ConwayValue` type — all post-Mary eras use `MaryValue` (aliased as `Value` for those eras). Only Shelley uses a pure `Coin` value type.

---

## 5. Script Types

### Native Scripts (Shelley: MultiSig; Allegra+: Timelock)

**Shelley MultiSig** — encoded inline (no version prefix needed since the whole `Script` type is just a `MultiSig`):

CBOR tags (array with discriminant):
```
[0, keyhash_28b]           -- RequireSignature
[1, [scripts...]]          -- RequireAllOf
[2, [scripts...]]          -- RequireAnyOf
[3, n, [scripts...]]       -- RequireMOf
```

**Allegra Timelock** — same tags 0-3, plus:
```
[4, slot_no]               -- RequireTimeStart (invalid before slot)
[5, slot_no]               -- RequireTimeExpire (invalid at/after slot)
```

Source: `eras/allegra/impl/src/Cardano/Ledger/Allegra/Scripts.hs`

### AlonzoScript (Alonzo/Babbage/Conway era)

CBOR uses discriminant tags in a 2-element array (`[tag, body]`):
```
[0, timelock_script]       -- NativeScript (Timelock)
[1, plutus_binary_bytes]   -- PlutusScript V1
[2, plutus_binary_bytes]   -- PlutusScript V2
[3, plutus_binary_bytes]   -- PlutusScript V3
[4, plutus_binary_bytes]   -- PlutusScript V4
```

Source: `eras/alonzo/impl/src/Cardano/Ledger/Alonzo/Scripts.hs`, `encodeScript` function.

`PlutusScript` at era level wraps a `Plutus l` which CBOR-encodes as `[language_enum, bytes]`:
```
[0, bytes]   -- PlutusV1 (language enum = 0)
[1, bytes]   -- PlutusV2 (language enum = 1)
[2, bytes]   -- PlutusV3 (language enum = 2)
[3, bytes]   -- PlutusV4 (language enum = 3)
```

Note: `plutusLanguageTag` (for hashing) and the CBOR language enum differ:
- Language enum 0 = PlutusV1 (for Plutus CBOR encoding)
- Language enum 1 = PlutusV2
- But hash prefix byte: V1=0x01, V2=0x02, V3=0x03, V4=0x04

`PlutusBinary` encodes as CBOR `bytes(...)` — the raw flat UPLC bytes.

### Script CBOR in Transaction Witness Set

Scripts in the witness set use `AlonzoScript` CBOR, wrapped with the era tag discriminant described above.

---

## 6. Datum Types

Source: `libs/cardano-ledger-core/src/Cardano/Ledger/Plutus/Data.hs`

### Data / PlutusData

```haskell
newtype PlutusData era = PlutusData PV1.Data
-- Encoded using codec-serialise (PV1.Data serialization)

newtype Data era = MkData (MemoBytes (PlutusData era))
-- Preserves original bytes for hashing
```

**CBOR**: Uses Plutus's own CBOR encoding for `PV1.Data` (which uses CBOR tags 121-127 for constructors, 6:121-6:127, etc.).

### DataHash (a.k.a. DatumHash)

```haskell
type DataHash = SafeHash EraIndependentData
-- = Hash Blake2b_256 EraIndependentData
-- = 32 bytes
```

Computed as: `Blake2b-256(original_cbor_bytes_of_Data)`.

### BinaryData (inline datum storage)

```haskell
newtype BinaryData era = BinaryData ShortByteString
```

**CBOR encoding**: `tag(24) bytes(...)` — CBOR tag 24 wraps the raw bytes of the Plutus data.

```haskell
instance EncCBOR (BinaryData era) where
  encCBOR (BinaryData sbs) = encodeTag 24 <> encCBOR sbs
```

### Datum (in TxOut)

```haskell
data Datum era
  = NoDatum
  | DatumHash !DataHash   -- hash only
  | Datum !(BinaryData era)  -- inline data
```

**CBOR** (in Babbage/Conway TxOut):
```
[0, datahash_bytes]       -- DatumHash (hash reference)
[1, tag(24) bytes]        -- Datum (inline, BinaryData encoding)
NoDatum                   -- omitted entirely from TxOut
```

---

## 7. Protocol Parameters (Conway Era)

Source: `eras/conway/impl/src/Cardano/Ledger/Conway/PParams.hs`

### Full PParams CBOR (Identity form — used in blocks and genesis)

**Encoding: positional array**, length = number of fields in `eraPParams @ConwayEra`.

For Conway, `eraPParams` contains 31 entries (keys 0-16 from Shelley/Alonzo/Babbage + 8 Conway additions, but key 12/13 are excluded in Conway, and key 14 is handled differently). Actually the exact array length is determined by `length (eraPParams @era)`.

**Conway `eraPParams` list order and array positions**:

| Array index | PParam key (update map) | Field name | Type |
|---|---|---|---|
| 0 | 0 | txFeePerByte (minFeeA) | CoinPerByte (compact Coin) |
| 1 | 1 | txFeeFixed (minFeeB) | CompactForm Coin |
| 2 | 2 | maxBlockBodySize | Word32 |
| 3 | 3 | maxTxSize | Word32 |
| 4 | 4 | maxBlockHeaderSize | Word16 |
| 5 | 5 | stakeAddressDeposit (keyDeposit) | CompactForm Coin |
| 6 | 6 | stakePoolDeposit (poolDeposit) | CompactForm Coin |
| 7 | 7 | poolRetireMaxEpoch (eMax) | EpochInterval (Word32) |
| 8 | 8 | stakePoolTargetNum (nOpt) | Word16 |
| 9 | 9 | poolPledgeInfluence (a0) | NonNegativeInterval (tag 30 rational) |
| 10 | 10 | monetaryExpansion (rho) | UnitInterval (tag 30 rational) |
| 11 | 11 | treasuryCut (tau) | UnitInterval (tag 30 rational) |
| 12 | 16 | minPoolCost | CompactForm Coin |
| 13 | 17 | utxoCostPerByte (coinsPerUTxOByte) | CoinPerByte (compact Coin) |
| 14 | 18 | costModels | CostModels |
| 15 | 19 | executionUnitPrices (prices) | Prices |
| 16 | 20 | maxTxExecutionUnits | OrdExUnits |
| 17 | 21 | maxBlockExecutionUnits | OrdExUnits |
| 18 | 22 | maxValueSize | Word32 |
| 19 | 23 | collateralPercentage | Word16 |
| 20 | 24 | maxCollateralInputs | Word16 |
| 21 | 25 | poolVotingThresholds | PoolVotingThresholds (array(5)) |
| 22 | 26 | dRepVotingThresholds | DRepVotingThresholds (array(10)) |
| 23 | 27 | committeeMinSize | Word16 |
| 24 | 28 | committeeMaxTermLength | EpochInterval (Word32) |
| 25 | 29 | govActionLifetime | EpochInterval (Word32) |
| 26 | 30 | govActionDeposit | CompactForm Coin |
| 27 | 31 | dRepDeposit | CompactForm Coin |
| 28 | 32 | dRepActivity | EpochInterval (Word32) |
| 29 | 33 | minFeeRefScriptCostPerByte | NonNegativeInterval (tag 30 rational) |
| 30 | n/a | protocolVersion | ProtVer (array(2)[major,minor]) |

**Note**: Conway PParams array has **31 elements** (index 0–30). Keys 12, 13 (d, extraEntropy) and 14 (protocolVersion as updatable) are removed in Conway. Key 15 (minUTxOValue) was removed in Babbage. Keys 16–33 remain.

**Important**: `protocolVersion` in Conway is NOT updatable (no ppuTag), but IS included in the full PParams array at position 30 via `ppGovProtocolVersion`.

### PParamsUpdate CBOR (StrictMaybe form — used in proposals and update certs)

**Encoding: sparse map** with integer keys. Only present (SJust) fields are included.

Map key = the `ppuTag` integer from the table above (0–33). Missing fields are simply absent from the map.

```
{ 0: uint,       -- txFeePerByte (if present)
  1: uint,       -- txFeeFixed
  2: uint,       -- maxBlockBodySize
  ...
  33: tag(30)[n,d],  -- minFeeRefScriptCostPerByte
}
```

### Nested Type Encodings

**`PoolVotingThresholds`** = `array(5)` of 5 UnitIntervals:
```
[motionNoConfidence, committeeNormal, committeeNoConfidence, hardForkInitiation, ppSecurityGroup]
```

**`DRepVotingThresholds`** = `array(10)` of 10 UnitIntervals:
```
[motionNoConfidence, committeeNormal, committeeNoConfidence, updateToConstitution,
 hardForkInitiation, ppNetworkGroup, ppEconomicGroup, ppTechnicalGroup, ppGovGroup,
 treasuryWithdrawal]
```

**`UnitInterval`** / **`NonNegativeInterval`** / **`PositiveUnitInterval`** = CBOR tag 30 rational:
```
tag(30) array(2) [numerator_int, denominator_int]
```

**`Prices`** (Alonzo) = `array(2)` of two `NonNegativeInterval`:
```
[prMem_rational, prSteps_rational]
```

**`ExUnits`** / **`OrdExUnits`** = `array(2)`:
```
[exUnitsMem_uint, exUnitsSteps_uint]
```

**`EpochInterval`** = single `uint` (Word32).

**`ProtVer`** = `array(2)` of `[major_uint, minor_uint]`.

**`CoinPerByte`** = compact coin (single `uint`, same as `CompactForm Coin`).

**`CompactForm Coin`** = `uint` (Word64 in practice).

---

## 8. Wire Format Details

### Tag 258 (Sets)

CBOR tag 258 is used for sets/lists where elements must be **canonically sorted**. Used for:
- Pool owners (set of KeyHash)
- Required signers
- Pool relays (as sets in some places)

Canonical order = CBOR lexicographic order of the encoded elements.

### Tag 24 (Embedded CBOR)

`tag(24) bytes(...)` — wraps CBOR-encoded data as an opaque byte string. Used for:
- Inline datums (BinaryData)
- GetCBOR query wrapping
- N2C block delivery

### Tag 30 (Rational Numbers)

```
tag(30) array(2) [numerator, denominator]
```

Used for all `BoundedRational` types: `UnitInterval`, `PositiveUnitInterval`, `PositiveInterval`, `NonNegativeInterval`.

Encode: `encodeRatioWithTag encodeInteger r` = `tag(30) [2] [numerator, denominator]`
Decode: `decodeRationalWithTag` — requires tag 30, then array of exactly 2 integers.

### Tags 121–127 and 1280+ (Plutus Data Constructors)

Plutus `Data` uses:
- `Constr 0 [...]` = tag 121, array of fields
- `Constr 1 [...]` = tag 122
- ...
- `Constr 6 [...]` = tag 127
- `Constr 7+ [...]` = tag 1280+, then array(2)[constructor_index, fields]
- `Map [(k,v)]` = CBOR map
- `List [...]` = CBOR array
- `I n` = CBOR integer
- `B bs` = CBOR bytes

### Credential CBOR

```haskell
instance Typeable kr => ToCBOR (Credential kr) where
  toCBOR = \case
    KeyHashObj kh    -> [2] 0 hash_bytes_28
    ScriptHashObj hs -> [2] 1 hash_bytes_28
```

`array(2)[discriminant_uint, hash_bytes]`

- 0 = key hash
- 1 = script hash

### TxIn CBOR

```
array(2) [tx_id_bytes_32, index_uint]
```

`TxId` = `SafeHash EraIndependentTxBody` = `Hash Blake2b_256` = 32 bytes, encoded as CBOR `bytes(32)`.

`TxIx` = `Word16`, encoded as `uint`.

### Addr CBOR

Addresses encode as CBOR `bytes(...)` — the raw binary address encoding (header byte + credential bytes).

```haskell
instance EncCBOR Addr where
  encCBOR = encCBOR . B.runPut . putAddr
  -- Wraps binary address as CBOR bytes()
```

### AccountAddress (Reward/Stake Address) CBOR

Same pattern: `bytes(29)` — 1 header byte + 28 credential bytes.

### Definite vs Indefinite Length

The ledger uses **definite length** encoding for all arrays and maps (e.g., `encodeListLen n`), NOT indefinite-length `0x9F`/`0xFF` CBOR. However, the DECODER must accept both in many places.

### Map Key Ordering (Canonical)

Maps in CBOR are not required to be ordered for correctness, but canonical encoding uses sorted keys. The `CanonicalMaps` used in `MultiAsset` enforce canonical order (ascending by key).

---

## 9. Genesis Configurations

### ShelleyGenesis

Source: `eras/shelley/impl/src/Cardano/Ledger/Shelley/Genesis.hs`

```haskell
data ShelleyGenesis = ShelleyGenesis
  { sgSystemStart         :: !UTCTime                          -- "systemStart"
  , sgNetworkMagic        :: !Word32                           -- "networkMagic"
  , sgNetworkId           :: !Network                          -- "networkId"
  , sgActiveSlotsCoeff    :: !PositiveUnitInterval             -- "activeSlotsCoeff" (f = active slot coefficient)
  , sgSecurityParam       :: !(NonZero Word64)                 -- "securityParam" (k)
  , sgEpochLength         :: !EpochSize                        -- "epochLength" (slots per epoch)
  , sgSlotsPerKESPeriod   :: !Word64                           -- "slotsPerKESPeriod"
  , sgMaxKESEvolutions    :: !Word64                           -- "maxKESEvolutions"
  , sgSlotLength          :: !NominalDiffTimeMicro             -- "slotLength" (in seconds, e.g. 1.0)
  , sgUpdateQuorum        :: !Word64                           -- "updateQuorum"
  , sgMaxLovelaceSupply   :: !Word64                           -- "maxLovelaceSupply"
  , sgProtocolParams      :: !(PParams ShelleyEra)             -- "protocolParams"
  , sgGenDelegs           :: !(Map (KeyHash GenesisRole) GenDelegPair)  -- "genDelegs"
  , sgInitialFunds        :: LM.ListMap Addr Coin              -- "initialFunds"
  , sgStaking             :: ShelleyGenesisStaking             -- "staking"
  }
```

JSON fields are camelCase. `activeSlotsCoeff` encodes as decimal (e.g., `0.05`). `slotLength` encodes as seconds decimal (e.g., `1`).

### AlonzoGenesis

Source: `eras/alonzo/impl/src/Cardano/Ledger/Alonzo/Genesis.hs`

Fields (JSON):
- `lovelacePerUTxOWord` / `coinsPerUTxOWord` — CoinPerWord
- `costModels` — map of language to cost model arrays
- `executionPrices` / `executionUnitPrices` — Prices
- `maxTxExecutionUnits` — ExUnits
- `maxBlockExecutionUnits` — ExUnits
- `maxValueSize` — Word32
- `collateralPercentage` — Word16
- `maxCollateralInputs` — Word16

### ConwayGenesis

Source: `eras/conway/impl/src/Cardano/Ledger/Conway/Genesis.hs`

```haskell
data ConwayGenesis = ConwayGenesis
  { cgUpgradePParams  :: !(UpgradeConwayPParams Identity)  -- all 9 new Conway PP
  , cgConstitution    :: !(Constitution ConwayEra)          -- "constitution"
  , cgCommittee       :: !(Committee ConwayEra)             -- "committee"
  , cgDelegs          :: ListMap (Credential Staking) Delegatee  -- "delegs" (optional)
  , cgInitialDReps    :: ListMap (Credential DRepRole) DRepState  -- "initialDReps" (optional)
  }
```

The `cgUpgradePParams` fields appear inline in JSON (not nested): poolVotingThresholds, dRepVotingThresholds, committeeMinSize, committeeMaxTermLength, govActionLifetime, govActionDeposit, dRepDeposit, dRepActivity, minFeeRefScriptCostPerByte, plus PlutusV3 cost model.

---

## 10. Reward Accounts and Withdrawals

### AccountAddress (formerly RewardAccount)

```haskell
data AccountAddress = AccountAddress
  { aaNetworkId :: !Network
  , aaId :: !AccountId  -- = Credential Staking
  }
```

Pattern synonym `RewardAccount` still exists for backward compat. Wire format = `bytes(29)`.

### Withdrawals

```haskell
newtype Withdrawals = Withdrawals { unWithdrawals :: Map AccountAddress Coin }
-- CBOR: map { bytes(29) => uint }
```

The `AccountAddress` is the CBOR bytes of the reward address (not a nested CBOR structure — it's the raw address binary).

### MIR (Move Instantaneous Rewards) — Removed in Conway

In pre-Conway eras, MIR certificates used:
```haskell
data MIRPot = ReservesMIR | TreasuryMIR
data MIRTarget = StakeAddressesMIR (Map (Credential Staking) DeltaCoin)
               | SendToOppositePotMIR Coin
```

MIR is completely removed in Conway — replaced by `TreasuryWithdrawal` governance actions.

---

## 11. Key CBOR Encoding Summary Table

| Type | CBOR encoding |
|---|---|
| `Hash Blake2b_256 a` (32B) | `bytes(32)` |
| `Hash Blake2b_224 a` (28B) | `bytes(28)` |
| `SafeHash i` (32B) | `bytes(32)` |
| `TxId` | `bytes(32)` |
| `ScriptHash` | `bytes(28)` |
| `KeyHash r` | `bytes(28)` |
| `VKey kd` | `bytes(32)` (raw Ed25519 pubkey) |
| `Coin` | `uint` (integer) |
| `CompactForm Coin` | `uint` (Word64) |
| `CoinPerByte` | `uint` (Word64, same as CompactForm Coin) |
| `TxIn` | `array(2)[bytes(32), uint]` |
| `Addr` | `bytes(...)` |
| `AccountAddress` | `bytes(29)` |
| `Credential r` | `array(2)[0_or_1, bytes(28)]` |
| `Withdrawals` | `map{bytes(29) => uint}` |
| `UnitInterval` | `tag(30) array(2)[int, int]` |
| `EpochInterval` | `uint` (Word32) |
| `ProtVer` | `array(2)[uint, uint]` |
| `MaryValue` (ADA-only) | `uint` |
| `MaryValue` (multi-asset) | `array(2)[uint, map]` |
| `MultiAsset` | `map{bytes(28) => map{bytes(0..32) => int}}` |
| `PolicyID` | `bytes(28)` (same as `ScriptHash`) |
| `AssetName` | `bytes(0..32)` |
| `DataHash` | `bytes(32)` |
| `BinaryData` | `tag(24) bytes(...)` |
| `Datum DatumHash` | `array(2)[0, bytes(32)]` |
| `Datum Datum` | `array(2)[1, tag(24) bytes(...)]` |
| `Plutus l` | `array(2)[language_enum, bytes(...)]` |
| `PParams ConwayEra` | `array(31)[...]` positional |
| `PParamsUpdate ConwayEra` | `map{uint => ...}` sparse |
| `PoolVotingThresholds` | `array(5)[rational×5]` |
| `DRepVotingThresholds` | `array(10)[rational×10]` |
| `Rational`/`BoundedRatio` | `tag(30) array(2)[int, int]` |
| `GenDelegPair` | `array(2)[bytes(28), bytes(32)]` |

---

## 12. Rust Translation Notes for Torsten

### Hash type mismatches
- `KeyHash`/`ScriptHash`/`PolicyID` = 28 bytes (Blake2b-224). Use `[u8; 28]` or a newtype.
- `TxId`/`DataHash`/`SafeHash` = 32 bytes (Blake2b-256). Use `[u8; 32]` or a newtype.
- **Never** use a single `Hash<32>` for both — the ledger distinguishes them clearly.

### Script hashing
```rust
// ScriptHash = Blake2b-224(prefix_byte || original_bytes)
// prefix 0x00 = native, 0x01 = PlutusV1, 0x02 = PlutusV2, 0x03 = PlutusV3
fn hash_script(prefix: u8, script_bytes: &[u8]) -> [u8; 28] {
    let mut input = vec![prefix];
    input.extend_from_slice(script_bytes);
    blake2b_224(&input)
}
```

### Address header decoding
```rust
const HEADER_BYRON: u8 = 0x82;
fn is_byron(h: u8) -> bool { h == HEADER_BYRON }
fn network(h: u8) -> Network { if h & 1 == 1 { Mainnet } else { Testnet } }
fn pay_is_script(h: u8) -> bool { h & 0x10 != 0 }
fn is_enterprise(h: u8) -> bool { h & 0x40 != 0 && h & 0x20 != 0 }
fn is_pointer(h: u8) -> bool { h & 0x40 != 0 && h & 0x20 == 0 }
fn is_base(h: u8) -> bool { h & 0x40 == 0 }
fn stake_is_script(h: u8) -> bool { is_base(h) && h & 0x20 != 0 }
```

### Rational CBOR
```rust
// tag(30) array(2)[numerator, denominator]
// Torsten must encode/decode this for UnitInterval, NonNegativeInterval, etc.
fn encode_rational(n: i64, d: i64) -> Vec<u8> {
    // d6_1e = tag(30), 82 = array(2), then two ints
}
```

### PParams CBOR
- `PParams` = `array(31)` with positional fields — see table above
- `PParamsUpdate` = sparse `map{uint => value}` — absent fields = no change
- The array length for Conway is exactly 31 (not 34 as one might expect from 0-33 map keys — keys 12, 13, 15 are absent in Conway; key 14 is moved to non-updatable position 30)

### Pallas 28-byte hashes
From CLAUDE.md: "Pallas 28-byte hash types (DRep keys, pool voter keys, required signers) must be padded to 32 bytes — do not use `Hash<32>::from()` directly on 28-byte hashes."

### CompactForm Coin vs Coin
- In PParams fields tagged as `CompactForm Coin` or `CoinPerByte`: encode as plain `uint`.
- In TxOut values: Coin encodes as `uint`.
- In MultiAsset amounts: encode as CBOR integer (may be negative for minting).
