---
name: CBOR Golden Test Vectors
description: Locations and decoded structure of official CBOR golden test vectors for Cardano protocol validation
type: reference
---

## Sources of CBOR Golden Test Vectors

### 1. ouroboros-consensus (official Haskell node)

Repository: `IntersectMBO/ouroboros-consensus`
Path: `ouroboros-consensus-cardano/golden/cardano/QueryVersion3/CardanoNodeToClientVersion19/`

These are the authoritative golden files produced by the Haskell cardano-node test suite.

**Key facts about OC golden file format:**
- Files store the HFC-wrapped result payload (array(1)[result]) — NOT the MsgResult [4, ...] wrapper
- Binary CBOR files, download via `curl -sL <raw.githubusercontent.com URL>` (NOT via GitHub API which mangles binary)
- Files containing indefinite-length CBOR arrays (0x9F...0xFF) cannot be decoded with a simple recursive decoder

**Downloaded into dugite at:** `tests/golden/n2c/` (HFC-wrapped, no MsgResult) and `tests/golden/n2c/oc/` (OC-specific goldens with real on-chain data)

**Key Conway query tag numbers:**
- 0 = GetLedgerTip
- 1 = GetEpochNo
- 3 = GetCurrentPParams
- 7 = GetStakeDistribution
- 11 = GetGenesisConfig
- 34 = GetBigLedgerPeerSnapshot (takes parameter flag=1)
- 37 = GetStakeDistribution2 (PoolDistr2 with total_active_stake)
- 38 = GetMaxMajorProtocolVersion

**Key Conway result structures:**
- EpochNo: `81 0A` = array(1)[10]
- MaxMajorProtocolVersion: `81 0D` = array(1)[13]
- LedgerTip: `81 82 09 58 20 <hash32>` = array(1)[array(2)[slot=9, bytes(32)]]
- SlotNo: `18 2A` = plain uint(42) (no wrapper)
- EmptyPParams: `81 98 1F ...` = array(1)[array(31)[...fields...]] — 145 bytes total

**EmptyPParams field order (ConwayPParams EncCBOR):**
```
[0]  txFeePerByte       [1]  txFeeFixed         [2]  maxBlockBodySize
[3]  maxTxSize (=2048)  [4]  maxBlockHeaderSize [5]  keyDeposit
[6]  poolDeposit        [7]  eMax               [8]  nOpt (=100)
[9]  a0 tag(30)         [10] rho tag(30)        [11] tau tag(30)
[12] protocolVersion [9,0]   [13] minPoolCost    [14] adaPerUTxOByte
[15] costModels {}      [16] prices [tag30,tag30]
[17] maxTxExUnits [0,0] [18] maxBlockExUnits [0,0]
[19] maxValueSize       [20] collateralPct (=150) [21] maxCollateral (=5)
[22] pvt array(5)       [23] dvt array(10)
[24] committeeMinSize   [25] committeeMaxTermLength
[26] govActionLifetime  [27] govActionDeposit
[28] drepDeposit        [29] drepActivity
[30] minFeeRefScriptCostPerByte tag(30)
```

### 2. Cardano Blueprint

Repository: `cardano-scaling/cardano-blueprint`
Path: `src/network/node-to-node/handshake/test-data/test-0` through `test-4`

Files contain hex-ASCII strings (NOT raw binary). Decode with `xxd -r -p`.

**Downloaded into dugite at:** `tests/golden/handshake/blueprint_test_0` through `_4`

**Handshake test vectors (decoded):**
- test-0: `8200a0` = [0, {}] — MsgProposeVersions with empty version table
- test-1: `820283020d617b` = [2, [2, 13, "{"]] — MsgRefuse RefuseReasonRefused
- test-2: `8200a10e8400f401f4` = [0, {14: [0, false, 1, false]}] — v14 propose
- test-3: `8200a20d8401f501f40e8402f501f4` = [0, {13: [...], 14: [...]}] — v13+v14 propose
- test-4: `83010e8401f401f4` = [1, 14, [1, false, 1, false]] — MsgAcceptVersion v14

**CDDL reference:** `src/network/node-to-node/handshake/messages.cddl`
- nodeToNodeVersionData = [networkMagic, initiatorOnlyDiffusionMode, peerSharing(0..1), query]
- versionNumbers: 13, 14 (in Blueprint CDDL — dugite uses 14, 15, 16)

## Key CBOR Encoding Rules

| Structure | Encoding | Hex prefix |
|-----------|----------|------------|
| HFC success wrapper | array(1) | 0x81 |
| Conway PParams | array(31) | 0x98 0x1F |
| Rational (UnitInterval) | tag(30)[num, den] | 0xD8 0x1E 0x82 |
| CBOR Set | tag(258)[array] | 0xD9 0x01 0x02 |
| Embedded CBOR | tag(24)[bytes] | 0xD8 0x18 |
| Credential | array(2)[0|1, hash28] | 0x82 0x00/01 0x58 0x1C |
| Point (Specific) | array(2)[slot, hash32] | 0x82 ... 0x58 0x20 |
| Point (Origin) | array(0) | 0x80 |
| ADA-only Value | plain uint | 0x1A (for 2M lovelace) |
| Multi-asset Value | array(2)[coin, map] | 0x82 ... |
| MsgResult | array(2)[4, result] | 0x82 0x04 ... |

## Special Cases

**GetNonMyopicMemberRewards query keys** are `Either Lovelace Credential`:
- Left lovelace: `[0, coin]` (stake amount query, NOT a credential)
- Right credential: `[1, Credential]` where Credential = `[type, hash28]`

**GetStakeDistribution2 (tag 37) vs GetStakeDistribution (tag 7):**
- tag 37 returns `PoolDistr2 = array(2)[pool_map, total_active_stake]`
- IndividualPoolStake (v2) = `array(3)[tag(30)rational, compact_lovelace, vrf_hash32]` (3 elements)
- tag 7 returns just `Map<pool_hash, IndividualPoolStake>` where `IndividualPoolStake = array(2)[rational, vrf_hash32]`

**GetBigLedgerPeerSnapshot (tag 34)** inner data uses indefinite-length CBOR arrays (0x9F...0xFF).
Simple recursive decoders cannot handle this. Only check outer fixed-length structure in tests.

**OC golden files vs our MsgResult-wrapped fixtures:**
- OC goldens: `tests/golden/n2c/oc/` — HFC-wrapped only, no MsgResult
- Our fixtures: `tests/golden/n2c/` — include full MsgResult `[4, [result]]` wrapper
- Tests in `n2c_encoding.rs` generate/verify our own fixtures
- Tests in `cbor_golden.rs` verify OC-sourced goldens (for structural assertions)
