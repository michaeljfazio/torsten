---
name: crate-pallas-traverse
description: pallas-traverse MultiEraBlock/Tx/Output traversal API, all public types and modules
type: reference
---

# pallas-traverse (v1.0.0-alpha.5)

## Overview

Description: "Utilities to traverse over multi-era block data". The primary era-agnostic API for working with Cardano blocks and transactions across all eras.

## Core Public Types

### Enums (era-polymorphic wrappers)

```rust
pub enum Era {
    Byron, Shelley, Allegra, Mary, Alonzo, Babbage, Conway
}

pub enum Feature {
    TimeLocks, MultiAssets, Staking, SmartContracts,
    CIP31,    // Reference inputs
    CIP32,    // Inline datums
    CIP33,    // Reference scripts
    CIP1694,  // Governance (Conway)
}

pub enum MultiEraBlock<'b>      // Block across all eras
pub enum MultiEraTx<'b>         // Transaction across all eras
pub enum MultiEraHeader<'b>     // Block header across all eras
pub enum MultiEraValue<'b>      // Currency value (ADA-only or multi-asset)
pub enum MultiEraOutput<'b>     // Transaction output
pub enum MultiEraInput<'b>      // Transaction input
pub enum MultiEraCert<'b>       // Certificate (or NotApplicable)
pub enum MultiEraRedeemer<'b>   // Plutus redeemer
pub enum MultiEraMeta<'b>       // Transaction metadata
pub enum MultiEraPolicyAssets<'b>  // Native assets by policy
pub enum MultiEraAsset<'b>      // Individual native asset
pub enum MultiEraWithdrawals<'b>   // Staking withdrawals
pub enum MultiEraUpdate<'b>     // Protocol parameter update
pub enum MultiEraProposal<'b>   // Governance proposal (Conway)
pub enum MultiEraGovAction<'b>  // Governance action (Conway)
pub enum MultiEraSigners<'b>    // Required signers
pub enum Error                  // CBOR parsing + validation errors
```

### Structs

```rust
pub struct OutputRef { tx_hash: Hash<32>, index: u64 }  // UTxO reference
```

### Traits

```rust
pub trait ComputeHash<const HASH_SIZE: usize> {
    fn compute_hash(&self) -> Hash<HASH_SIZE>;
}

pub trait OriginalHash<const HASH_SIZE: usize> {
    fn original_hash(&self) -> Hash<HASH_SIZE>;
}
```

## MultiEraBlock API (key methods)

```rust
impl MultiEraBlock<'_> {
    pub fn decode(data: &[u8]) -> Result<MultiEraBlock, Error>
    pub fn era(&self) -> Era
    pub fn hash(&self) -> Hash<32>
    pub fn slot(&self) -> u64
    pub fn number(&self) -> u64
    pub fn tx_count(&self) -> usize
    pub fn txs(&self) -> Vec<MultiEraTx>
    pub fn header(&self) -> MultiEraHeader
    pub fn issuer_vkey(&self) -> Option<&[u8]>  // block producer vkey
}
```

## MultiEraTx API (key methods)

```rust
impl MultiEraTx<'_> {
    pub fn hash(&self) -> Hash<32>
    pub fn inputs(&self) -> Vec<MultiEraInput>
    pub fn outputs(&self) -> Vec<MultiEraOutput>
    pub fn fee(&self) -> Option<u64>
    pub fn mints(&self) -> Vec<MultiEraPolicyAssets>
    pub fn certs(&self) -> Vec<MultiEraCert>
    pub fn withdrawals(&self) -> MultiEraWithdrawals
    pub fn metadata(&self) -> MultiEraMeta
    pub fn redeemers(&self) -> Vec<MultiEraRedeemer>
    pub fn required_signers(&self) -> MultiEraSigners
    pub fn is_valid(&self) -> bool               // phase-2 collateral flag
    pub fn as_babbage(&self) -> Option<&BabbageTransaction>
    pub fn as_conway(&self) -> Option<&ConwayTransaction>
    // etc for each era
}
```

## MultiEraHeader API (key methods)

```rust
impl MultiEraHeader<'_> {
    pub fn slot(&self) -> u64
    pub fn hash(&self) -> Hash<32>
    pub fn previous_hash(&self) -> Option<Hash<32>>
    pub fn cbor(&self) -> &[u8]       // raw CBOR for hashing
    pub fn era(&self) -> Era
    pub fn issuer_vkey(&self) -> Option<&[u8]>
}
```

## Sub-modules (26 total)

- `assets`, `auxiliary_data`, `blocks`, `certificates`, `eras`, `fees`
- `governance`, `hashes`, `headers`, `inputs`, `metadata`, `outputs`
- `probing`, `redeemers`, `signers`, `sizing`, `time`, `transactions`
- `updates`, `values`, `withdrawals`, `witnesses`
- `wellknown` — network-specific constants (epoch length, slot duration, magic numbers)

## Fees Module

Provides fee calculation helpers. Likely implements the linear fee formula (`a * size + b`).

## Wellknown Module

Contains network-specific constants for mainnet, preview, preprod. Used in `dugite-network/src/pipelined.rs` for Byron epoch length.

## What Dugite Uses from pallas-traverse

From `dugite-serialization/src/multi_era.rs`:
- `MultiEraBlock as PallasBlock` — primary block deserialization
- `MultiEraTx as PallasTx` — transaction traversal
- `MultiEraInput as PallasInput`
- `MultiEraOutput as PallasOutput`
- `MultiEraCert` — certificate extraction
- `MultiEraWithdrawals`
- `MultiEraMeta`
- `MultiEraSigners`
- `Era as PallasEra` — era detection

From `dugite-network/src/pipelined.rs`:
- `MultiEraHeader` — for decoding block headers during pipelined chainsync

## Notes

- All types use `'b` lifetime tied to the underlying CBOR bytes (zero-copy design)
- Decoding is lazy where possible
- The `probing` module allows era detection without full decode
- `MultiEraBlock::decode()` is the primary entry point for block parsing
