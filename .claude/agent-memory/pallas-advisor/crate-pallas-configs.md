---
name: crate-pallas-configs
description: Genesis file parsing capabilities of pallas-configs across all eras; comparison to dugite-node genesis.rs
type: reference
---

# pallas-configs (v1.0.0-alpha.5)

## Overview

Description: "Config structs and utilities matching the Haskell implementation". Parses Cardano genesis JSON files for Byron, Shelley, Alonzo, and Conway eras. Version 1.0.0-alpha.5.

## Module Structure

```
pallas_configs::
  byron::    // Byron genesis: GenesisFile, BlockVersionData, ProtocolConsts
  shelley::  // Shelley genesis: GenesisFile, ProtocolParams, Pool, Staking
  alonzo::   // Alonzo genesis: cost models, execution unit prices
  conway::   // Conway genesis: governance params, voting thresholds
  cost_models:: // Cost model utilities
```

## Features

- `json` (default): enables serde + serde_json
- `default` includes `json`

## Dependencies

- pallas-addresses, pallas-crypto, pallas-primitives (all =1.0.0-alpha.5)
- serde, serde_json (optional, enabled by default feature)
- base64 0.22.0
- serde_with 3.7.0
- num-rational 0.4.2

## Byron Genesis (`pallas_configs::byron`)

```rust
pub struct GenesisFile {
    pub avvm_distr: HashMap<String, String>,    // genesis UTxOs from Cardano Paper Wallets
    pub block_version_data: BlockVersionData,
    pub fts_seed: Option<String>,
    pub protocol_consts: ProtocolConsts,
    pub start_time: u64,                         // Unix timestamp
    pub boot_stakeholders: HashMap<String, BootStakeWeight>,
    pub heavy_delegation: HashMap<String, HeavyDelegation>,
    pub non_avvm_balances: HashMap<String, String>, // genesis UTxOs (addresses → lovelace)
    pub vss_certs: Option<HashMap<String, VssCert>>,
}

pub struct BlockVersionData {
    pub script_version: u16,
    pub heavy_del_thd: u64,
    pub max_block_size: u64,
    pub max_header_size: u64,
    pub max_proposal_size: u64,
    pub max_tx_size: u64,
    pub mpc_thd: u64,
    pub slot_duration: u64,           // milliseconds per slot
    pub softfork_rule: SoftForkRule,
    pub tx_fee_policy: TxFeePolicy,
    pub unlock_stake_epoch: u64,
    pub update_implicit: u64,
    pub update_proposal_thd: u64,
    pub update_vote_thd: u64,
}

pub struct ProtocolConsts {
    pub k: usize,            // security parameter
    pub protocol_magic: u32,
    pub vss_max_ttl: Option<u32>,
    pub vss_min_ttl: Option<u32>,
}

pub struct TxFeePolicy {
    pub multiplier: u64,
    pub summand: u64,
}
```

## Shelley Genesis (`pallas_configs::shelley`)

```rust
pub struct GenesisFile {
    // ... network configuration
    // ... protocol params via ProtocolParams struct
    // ... staking via Staking struct
    // ... gen delegates
}

pub struct ProtocolParams {
    pub protocol_version: ProtocolVersion,
    pub max_tx_size: u64,
    pub max_block_body_size: u64,
    pub max_block_header_size: u64,
    pub key_deposit: u64,
    pub min_utxo_value: u64,
    pub min_fee_a: u64,
    pub min_fee_b: u64,
    pub pool_deposit: u64,
    pub n_opt: u64,           // desired number of pools
    pub min_pool_cost: u64,
    pub e_max: u64,           // max epoch for pool retirement
    pub extra_entropy: ExtraEntropy,
    pub decentralisation_param: f64,
    pub rho: f64,             // monetary expansion
    pub tau: f64,             // treasury growth
    pub a0: f64,              // pool pledge influence
}

// Public functions:
pub fn from_file(path: &Path) -> Result<GenesisFile, io::Error>
pub fn shelley_utxos(config: &GenesisFile) -> Vec<GenesisUtxo>
```

## Alonzo Genesis (`pallas_configs::alonzo`)

Contains Alonzo-specific additions: cost models, execution unit prices, collateral params, etc.

## Conway Genesis (`pallas_configs::conway`)

```rust
pub struct GenesisFile {
    pub pool_voting_thresholds: PoolVotingThresholds,
    pub d_rep_voting_thresholds: DRepVotingThresholds,
    pub committee_min_size: u64,
    pub committee_max_term_length: u32,
    pub gov_action_lifetime: u32,
    pub gov_action_deposit: u64,
    pub d_rep_deposit: u64,
    pub d_rep_activity: u32,
    pub min_fee_ref_script_cost_per_byte: u64,
    pub plutus_v3_cost_model: Vec<i64>,
    pub constitution: Constitution,
    pub committee: Committee,
}

pub struct PoolVotingThresholds {
    // 5 f32 threshold values: motion_no_confidence, committee_normal,
    // committee_no_confidence, hard_fork, pp_security_group
}

pub struct DRepVotingThresholds {
    // 10 f32 threshold values covering all governance action types
}

pub struct Committee {
    pub members: HashMap<String, u64>,   // credential → epoch
    pub threshold: Fraction,
}
```

## What pallas-configs Provides That dugite-node Doesn't

1. **Structured Byron genesis parsing** — dugite's genesis.rs may not parse all Byron genesis fields
2. **Shelley UTxO extraction helper** — `shelley_utxos()` function
3. **Conway governance parameter parsing** — committee, DRep thresholds with proper Fraction type
4. **Cost model utilities** — via `cost_models` submodule

## What dugite-node/genesis.rs Has Beyond pallas-configs

Need to verify, but dugite likely:
- Reads genesis files and extracts specific fields for ledger initialization
- Handles the eras needed for its use cases
- May parse fields pallas-configs omits (or vice versa)

## Adoption Recommendation

**ADOPT** with low risk. pallas-configs is straightforward JSON deserialization with no complex logic. It would replace ad-hoc parsing in `dugite-node/src/genesis.rs`. The main consideration is ensuring field coverage (dugite may need fields pallas-configs doesn't expose).

**Key benefit**: The `shelley_utxos()` function and structured protocol parameter types that match what pallas-validate's `Environment` struct expects (reducing type conversion friction if also adopting pallas-validate).

**Risk**: Alpha API instability. Field names in the genesis structs could change between alpha versions. The serde_json deserialization is tolerant of extra fields by default, so new fields in genesis files won't break it.
