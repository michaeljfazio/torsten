---
name: Conway PParams array(31) complete field order
description: All 31 Conway PParams fields in exact CBOR index order, verified from eraPParams list in Conway/PParams.hs
type: reference
---

# Conway PParams CBOR Encoding — array(31)

Source: `eras/conway/impl/src/Cardano/Ledger/Conway/PParams.hs`

Encoding mechanism: `encodeListLen (fromIntegral (length (eraPParams @era))) <> F.foldMap' toEnc (eraPParams @era)`
Source for mechanism: `libs/cardano-ledger-core/src/Cardano/Ledger/Core/PParams.hs` lines 187-189

The `eraPParams` list in `EraPParams ConwayEra` instance (lines 861-893):

```
Index  Field name               Haskell field          Type
  0    txFeePerByte             cppTxFeePerByte         CoinPerByte (= CompactForm Coin / byte)
  1    txFeeFixed               cppTxFeeFixed           CompactForm Coin
  2    maxBBSize                cppMaxBBSize            Word32
  3    maxTxSize                cppMaxTxSize            Word32
  4    maxBHSize                cppMaxBHSize            Word16
  5    keyDeposit               cppKeyDeposit           CompactForm Coin
  6    poolDeposit              cppPoolDeposit          CompactForm Coin
  7    eMax                     cppEMax                 EpochInterval (u32)
  8    nOpt                     cppNOpt                 Word16
  9    a0                       cppA0                   NonNegativeInterval (rational)
 10    rho                      cppRho                  UnitInterval (rational)
 11    tau                      cppTau                  UnitInterval (rational)
 12    protocolVersion          cppProtocolVersion      ProtVer = [major, minor]
 13    minPoolCost              cppMinPoolCost          CompactForm Coin
 14    coinsPerUTxOByte         cppCoinsPerUTxOByte     CoinPerByte
 15    costModels               cppCostModels           CostModels
 16    prices                   cppPrices               Prices [exUnitsMem, exUnitsSteps]
 17    maxTxExUnits             cppMaxTxExUnits         OrdExUnits [mem, steps]
 18    maxBlockExUnits          cppMaxBlockExUnits      OrdExUnits [mem, steps]
 19    maxValSize               cppMaxValSize           Word32
 20    collateralPercentage     cppCollateralPercentage Word16
 21    maxCollateralInputs      cppMaxCollateralInputs  Word16
 22    poolVotingThresholds     cppPoolVotingThresholds PoolVotingThresholds (array(5))
 23    dRepVotingThresholds     cppDRepVotingThresholds DRepVotingThresholds (array(10))
 24    committeeMinSize         cppCommitteeMinSize     Word16
 25    committeeMaxTermLength   cppCommitteeMaxTermLength EpochInterval (u32)
 26    govActionLifetime        cppGovActionLifetime    EpochInterval (u32)
 27    govActionDeposit         cppGovActionDeposit     CompactForm Coin
 28    dRepDeposit              cppDRepDeposit          CompactForm Coin
 29    dRepActivity             cppDRepActivity         EpochInterval (u32)
 30    minFeeRefScriptCostPerByte cppMinFeeRefScriptCostPerByte NonNegativeInterval (rational)
```

## Key Answers

- **Index 0**: txFeePerByte (NOT minFeeA/minFeeB — those are deprecated aliases)
  - `cppMinFeeA` and `cppMinFeeB` are deprecated aliases for cppTxFeePerByte and cppTxFeeFixed
- **Index 12**: protocolVersion (confirmed)
- **Governance thresholds**: poolVotingThresholds at index 22, dRepVotingThresholds at index 23
- **Index 30**: minFeeRefScriptCostPerByte (confirmed, 31st and final field)

## PoolVotingThresholds = array(5)
```
[0] pvtMotionNoConfidence
[1] pvtCommitteeNormal
[2] pvtCommitteeNoConfidence
[3] pvtHardForkInitiation
[4] pvtPPSecurityGroup
```

## DRepVotingThresholds = array(10)
```
[0] dvtMotionNoConfidence
[1] dvtCommitteeNormal
[2] dvtCommitteeNoConfidence
[3] dvtUpdateToConstitution
[4] dvtHardForkInitiation
[5] dvtPPNetworkGroup
[6] dvtPPEconomicGroup
[7] dvtPPTechnicalGroup
[8] dvtPPGovGroup
[9] dvtTreasuryWithdrawal
```

## protocolVersion encoding
`protocolVersion` (index 12) uses `HKDNoUpdate` — this means it cannot be set via PParamsUpdate
in Conway (governed by HFC instead). The field IS present in the serialized PParams (as ProtVer),
it just cannot be updated through governance. Encoder: standard `encCBOR` on ProtVer.

## Note on cppGovProtocolVersion
The ppName in eraPParams list is `ppGovProtocolVersion` for the protocol version entry (line 874).
The field itself is `cppProtocolVersion` in the struct. This is just an internal naming distinction.
