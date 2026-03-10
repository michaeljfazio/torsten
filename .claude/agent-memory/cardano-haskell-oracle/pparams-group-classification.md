# Conway Protocol Parameter Group Classification

Source: `cardano-ledger/eras/conway/impl/src/Cardano/Ledger/Conway/PParams.hs` (ConwayPParams type)
Threshold logic: `cardano-ledger/eras/conway/impl/src/Cardano/Ledger/Conway/Governance/Internal.hs`

## Each param tagged with PPGroups DRepGroup StakePoolGroup

### NetworkGroup (dvtPPNetworkGroup)
- max_block_body_size (SecurityGroup)
- max_tx_size (SecurityGroup)
- max_block_header_size (SecurityGroup)
- max_tx_ex_units (NoStakePoolGroup)
- max_block_ex_units (SecurityGroup)
- max_val_size (SecurityGroup)
- max_collateral_inputs (NoStakePoolGroup)

### EconomicGroup (dvtPPEconomicGroup)
- min_fee_a/txFeePerByte (SecurityGroup)
- min_fee_b/txFeeFixed (SecurityGroup)
- key_deposit (NoStakePoolGroup)
- pool_deposit (NoStakePoolGroup)
- rho (NoStakePoolGroup)
- tau (NoStakePoolGroup)
- min_pool_cost (NoStakePoolGroup)
- ada_per_utxo_byte (SecurityGroup)
- execution_costs/prices (NoStakePoolGroup)
- min_fee_ref_script_cost_per_byte (SecurityGroup)

### TechnicalGroup (dvtPPTechnicalGroup)
- e_max (NoStakePoolGroup)
- n_opt (NoStakePoolGroup)
- a0 (NoStakePoolGroup)
- cost_models (NoStakePoolGroup)
- collateral_percentage (NoStakePoolGroup)

### GovGroup (dvtPPGovGroup)
- pool_voting_thresholds (NoStakePoolGroup)
- drep_voting_thresholds (NoStakePoolGroup)
- committee_min_size (NoStakePoolGroup)
- committee_max_term_length (NoStakePoolGroup)
- gov_action_lifetime (NoStakePoolGroup)
- gov_action_deposit (SecurityGroup)
- drep_deposit (NoStakePoolGroup)
- drep_activity (NoStakePoolGroup)

### Not updatable
- protocol_version (HKDNoUpdate — changed only via HardForkInitiation)

## Threshold Combination for Multi-Group Updates

### DRep threshold
`pparamsUpdateThreshold`: collects all DRepGroups from modified fields, takes MAX of their thresholds.

### SPO threshold
`votingStakePoolThresholdInternal`: if ANY modified param is SecurityGroup -> pvtPPSecurityGroup threshold.
If NO modified param is SecurityGroup -> NoVotingAllowed (auto-passes, SPOs cannot vote).

### Committee
Always applies its normal threshold for ParameterChange actions.
