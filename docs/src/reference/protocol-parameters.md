# Protocol Parameters Reference

Cardano protocol parameters control the behavior of the network, including fees, block sizes, staking mechanics, and governance. These parameters can be queried from a running node and updated through governance actions.

## Querying Parameters

```bash
dugite-cli query protocol-parameters \
  --socket-path ./node.sock \
  --out-file protocol-params.json
```

## Fee Parameters

| Parameter | JSON Key | Description | Mainnet Default |
|-----------|----------|-------------|-----------------|
| Min fee coefficient | `txFeePerByte` / `minFeeA` | Fee per byte of transaction size | 44 |
| Min fee constant | `txFeeFixed` / `minFeeB` | Fixed fee component | 155381 |
| Min UTxO value per byte | `utxoCostPerByte` / `adaPerUtxoByte` | Minimum lovelace per byte of UTxO | 4310 |

The transaction fee formula is:

```
fee = txFeePerByte * tx_size_in_bytes + txFeeFixed
```

## Block Size Parameters

| Parameter | JSON Key | Description | Mainnet Default |
|-----------|----------|-------------|-----------------|
| Max block body size | `maxBlockBodySize` | Maximum block body size in bytes | 90112 |
| Max transaction size | `maxTxSize` | Maximum transaction size in bytes | 16384 |
| Max block header size | `maxBlockHeaderSize` | Maximum block header size in bytes | 1100 |

## Staking Parameters

| Parameter | JSON Key | Description | Mainnet Default |
|-----------|----------|-------------|-----------------|
| Stake address deposit | `stakeAddressDeposit` / `keyDeposit` | Deposit for stake key registration (lovelace) | 2000000 |
| Pool deposit | `stakePoolDeposit` / `poolDeposit` | Deposit for pool registration (lovelace) | 500000000 |
| Pool retire max epoch | `poolRetireMaxEpoch` / `eMax` | Maximum future epochs for pool retirement | 18 |
| Pool target count | `stakePoolTargetNum` / `nOpt` | Target number of pools (k parameter) | 500 |
| Min pool cost | `minPoolCost` | Minimum fixed pool cost (lovelace) | 170000000 |

## Monetary Policy

| Parameter | Description |
|-----------|-------------|
| Monetary expansion (rho) | Rate of new ADA creation from reserves per epoch |
| Treasury cut (tau) | Fraction of rewards directed to the treasury |
| Pledge influence (a0) | How pledge affects reward calculations |

## Plutus Execution Parameters

| Parameter | JSON Key | Description | Mainnet Default |
|-----------|----------|-------------|-----------------|
| Max tx execution units | `maxTxExecutionUnits` | `{memory, steps}` per transaction | `{14000000, 10000000000}` |
| Max block execution units | `maxBlockExecutionUnits` | `{memory, steps}` per block | `{62000000, 40000000000}` |
| Max value size | `maxValueSize` | Maximum serialized value size in bytes | 5000 |
| Collateral percentage | `collateralPercentage` | Collateral % of total tx fee for Plutus txs | 150 |
| Max collateral inputs | `maxCollateralInputs` | Maximum collateral inputs per tx | 3 |

## Governance Parameters (Conway)

| Parameter | JSON Key | Description | Mainnet Default |
|-----------|----------|-------------|-----------------|
| DRep deposit | `drepDeposit` | Deposit for DRep registration (lovelace) | 500000000 |
| Gov action deposit | `govActionDeposit` | Deposit for governance action submission (lovelace) | 100000000000 |
| Gov action lifetime | `govActionLifetime` | Governance action expiry (epochs) | 6 |

### Voting Thresholds

Different governance action types require different voting thresholds from DReps, SPOs, and the Constitutional Committee:

| Action Type | DRep Threshold | SPO Threshold | CC Threshold |
|-------------|---------------|---------------|--------------|
| No Confidence | dvtMotionNoConfidence | pvtMotionNoConfidence | Required |
| Update Committee (normal) | dvtCommitteeNormal | pvtCommitteeNormal | N/A |
| Update Committee (no confidence) | dvtCommitteeNoConfidence | pvtCommitteeNoConfidence | N/A |
| New Constitution | dvtUpdateToConstitution | N/A | Required |
| Hard Fork Initiation | dvtHardForkInitiation | pvtHardForkInitiation | Required |
| Protocol Parameter Update (network) | dvtPPNetworkGroup | N/A | Required |
| Protocol Parameter Update (economic) | dvtPPEconomicGroup | pvtPPEconomicGroup | Required |
| Protocol Parameter Update (technical) | dvtPPTechnicalGroup | N/A | Required |
| Protocol Parameter Update (governance) | dvtPPGovGroup | N/A | Required |
| Treasury Withdrawal | dvtTreasuryWithdrawal | N/A | Required |

## CBOR Field Numbers

When encoding protocol parameter updates in governance actions, each parameter maps to a CBOR field number:

| CBOR Key | Parameter |
|----------|-----------|
| 0 | txFeePerByte / minFeeA |
| 1 | txFeeFixed / minFeeB |
| 2 | maxBlockBodySize |
| 3 | maxTxSize |
| 4 | maxBlockHeaderSize |
| 5 | stakeAddressDeposit / keyDeposit |
| 6 | stakePoolDeposit / poolDeposit |
| 7 | poolRetireMaxEpoch / eMax |
| 8 | stakePoolTargetNum / nOpt |
| 16 | minPoolCost |
| 17 | utxoCostPerByte / adaPerUtxoByte |
| 20 | maxTxExecutionUnits |
| 21 | maxBlockExecutionUnits |
| 22 | maxValueSize |
| 23 | collateralPercentage |
| 24 | maxCollateralInputs |
| 30 | drepDeposit |
| 31 | govActionDeposit |
| 32 | govActionLifetime |
