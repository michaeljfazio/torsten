# Conway PParams CBOR Encoding Reference

## Key Distinction: PParams vs PParamsUpdate
- **PParams** (GetCurrentPParams query response): CBOR **array** of length 31, fields in fixed order
- **PParamsUpdate** (governance proposals): CBOR **map** with integer keys 0-33, only changed fields present

## Source Files
- EncCBOR PParams: `cardano-ledger/libs/cardano-ledger-core/src/Cardano/Ledger/Core/PParams.hs`
- Conway fields: `cardano-ledger/eras/conway/impl/src/Cardano/Ledger/Conway/PParams.hs`
- CDDL spec: `cardano-ledger/eras/conway/impl/cddl/data/conway.cddl`
- Rational encoding: `cardano-ledger/libs/cardano-ledger-binary/src/Cardano/Ledger/Binary/Plain.hs`
- CostModels: `cardano-ledger/libs/cardano-ledger-core/src/Cardano/Ledger/Plutus/CostModels.hs`
- ExUnits/Prices: `cardano-ledger/libs/cardano-ledger-core/src/Cardano/Ledger/Plutus/ExUnits.hs`

## PParams Array Order (31 fields)
| Idx | Field | CBOR Type | ppuTag (map key) |
|-----|-------|-----------|------------------|
| 0 | txFeePerByte | uint | 0 |
| 1 | txFeeFixed | uint | 1 |
| 2 | maxBBSize | uint | 2 |
| 3 | maxTxSize | uint | 3 |
| 4 | maxBHSize | uint | 4 |
| 5 | keyDeposit | uint | 5 |
| 6 | poolDeposit | uint | 6 |
| 7 | eMax | uint | 7 |
| 8 | nOpt | uint | 8 |
| 9 | a0 | Tag(30)[num,den] | 9 |
| 10 | rho | Tag(30)[num,den] | 10 |
| 11 | tau | Tag(30)[num,den] | 11 |
| 12 | protocolVersion | [major,minor] | N/A (no update in Conway) |
| 13 | minPoolCost | uint | 16 |
| 14 | coinsPerUTxOByte | uint | 17 |
| 15 | costModels | map{0:[i64],1:[i64],2:[i64]} | 18 |
| 16 | prices | [Tag30,Tag30] | 19 |
| 17 | maxTxExUnits | [mem,steps] | 20 |
| 18 | maxBlockExUnits | [mem,steps] | 21 |
| 19 | maxValSize | uint | 22 |
| 20 | collateralPercentage | uint | 23 |
| 21 | maxCollateralInputs | uint | 24 |
| 22 | poolVotingThresholds | array(5) of Tag30 | 25 |
| 23 | drepVotingThresholds | array(10) of Tag30 | 26 |
| 24 | committeeMinSize | uint | 27 |
| 25 | committeeMaxTermLength | uint | 28 |
| 26 | govActionLifetime | uint | 29 |
| 27 | govActionDeposit | uint | 30 |
| 28 | drepDeposit | uint | 31 |
| 29 | drepActivity | uint | 32 |
| 30 | minFeeRefScriptCostPerByte | Tag(30)[num,den] | 33 |

## Note: Array index != ppuTag
Keys 12-15 were Shelley's ppD/extraEntropy/protVer/minUTxOValue.
Babbage removed ppD(12) and extraEntropy(13), so array positions shifted
but ppuTag numbers in PParamsUpdate map stayed the same.

## Nested Type Encodings
- **Rational**: `Tag(30) [numerator: uint, denominator: positive_uint]`
- **ExUnits**: `[mem: uint, steps: uint]`
- **Prices**: `[mem_price: Tag30[n,d], step_price: Tag30[n,d]]`
- **CostModels**: `{0: [i64...], 1: [i64...], 2: [i64...]}` (PlutusV1=0, V2=1, V3=2)
- **ProtocolVersion**: `[major: uint, minor: uint]`
- **PoolVotingThresholds**: array(5) Tag30 rationals: [motionNoConfidence, committeeNormal, committeeNoConfidence, hardForkInitiation, ppSecurityGroup]
- **DRepVotingThresholds**: array(10) Tag30 rationals: [motionNoConfidence, committeeNormal, committeeNoConfidence, updateConstitution, hardForkInitiation, ppNetworkGroup, ppEconomicGroup, ppTechnicalGroup, ppGovGroup, treasuryWithdrawal]

## Known Dugite Bugs (as of 2026-03-09)
1. encode_protocol_params_cbor uses map encoding, should be array(31)
2. DRep voting thresholds reuse dvt_p_param_change for all 4 PP group thresholds
3. ProtocolParamsSnapshot missing separate dvt_pp_network/economic/technical/governance fields
