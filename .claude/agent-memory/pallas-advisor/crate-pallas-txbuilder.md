---
name: crate-pallas-txbuilder
description: pallas-txbuilder transaction building API for Conway era; relevance to dugite-cli
type: reference
---

# pallas-txbuilder (v1.0.0-alpha.5)

## Overview

Description: "An ergonomic Cardano transaction builder". Provides a builder pattern API for constructing Conway-era Cardano transactions.

## Public API

```rust
pub struct BuildConway { ... }         // Main builder for Conway-era transactions
pub struct StagingTransaction { ... }  // Intermediate transaction state
pub struct BuiltTransaction { ... }    // Final constructed transaction
pub struct Input { ... }              // Transaction input
pub struct Output { ... }             // Transaction output
pub struct ExUnits { ... }            // Execution units for script costs
pub enum ScriptKind { ... }           // Script type (Native, PlutusV1, V2, V3)
pub struct Bytes(Vec<u8>)             // Byte array wrapper
pub struct Bytes32([u8; 32])          // 32-byte array wrapper

pub enum TxBuilderError {
    ScriptDecoding,
    DatumDecoding,
    HashLength,          // datum hashes must be 32 bytes
    RedeemerTarget,      // redeemer pointer resolution
    NetworkId,           // must be 0 (testnet) or 1 (mainnet)
    KeyDerivation,
    AssetNameLength,     // asset names ≤ 32 bytes
    EraCompatibility,    // era-specific constraints
}
```

## Builder Pattern

`BuildConway` provides a fluent API:
- Add inputs (UTxO references + optional redeemers)
- Add outputs (addresses + values + optional datums/scripts)
- Set fee
- Add minting
- Add certificates
- Add required signers
- Add metadata
- Build → `BuiltTransaction`

## Relevance to Dugite

Dugite has `dugite-cli` with 33+ subcommands. Some subcommands likely need transaction construction:
- `transaction build` — building unsigned transactions
- `transaction sign` — signing transactions
- `transaction submit` — submitting to node

`pallas-txbuilder` could replace hand-crafted CBOR encoding for transaction building in `dugite-cli`.

## Current Status in Dugite

NOT adopted. Dugite-cli builds transactions manually or is still being developed.

## Adoption Recommendation

**EVALUATE when implementing dugite-cli transaction building**. The builder provides ergonomic Conway transaction construction. Key considerations:

1. **Conway-only**: Builder targets Conway era. Historical era transactions would need other means.
2. **Alpha stability**: API may change between alpha versions.
3. **Fee calculation integration**: Builder needs correct fee calculation — ensure it uses pallas-traverse fee module.
4. **Plutus script integration**: When phase2 execution is needed, the builder's ExUnits handling would be relevant.

**Risk**: pallas-txbuilder is likely less tested than other pallas crates given its higher-level nature. Transaction building errors could produce malformed transactions that mainnet nodes reject.

**Recommendation**: Use as a starting point for dugite-cli's `transaction build` command, but add comprehensive integration tests against a live cardano-node before shipping.
