---
name: crate-pallas-validate
description: Detailed API and capabilities of pallas-validate — Phase-1 and Phase-2 tx validation across all eras
type: reference
---

# pallas-validate (v1.0.0-alpha.5)

## Overview

Comprehensive Cardano transaction validation implementing LEDGER/LEDGERS inference rules. Version 1.0.0-alpha.5. Description: "Utilities for validating transactions". Apache-2.0.

## Module Structure

```
pallas_validate::
  phase1::          // Always compiled — structural + signature validation
    byron           // Byron era
    shelley_ma      // Shelley, Allegra, Mary eras
    alonzo          // Alonzo era
    babbage         // Babbage era
    conway          // Conway era
    validate_txs()  // batch — applies state changes cumulatively
    validate_tx()   // single tx — era-dispatch
  utils::           // Always compiled
    Environment     // validation context (slot, pparams, network)
    MultiEraProtocolParameters
    ValidationError / ValidationResult
    ByronError / ShelleyMAError / AlonzoError / PostAlonzoError
  phase2::          // Only with feature = "phase2"
    (Plutus script execution via pallas-uplc)
```

## Features

- Default: no features (phase1 only)
- `phase2`: enables Plutus script execution via `pallas-uplc = "0.1.0"` (optional dep)

## Public Entry Points

```rust
// Single transaction validation
pub fn validate_tx(
    tx: &MultiEraTx,
    tx_ix: usize,
    env: &Environment,
    utxos: &UTxOs,
) -> ValidationResult

// Multi-transaction batch (state carries across txs)
pub fn validate_txs(
    txs: &[MultiEraTx],
    env: &Environment,
    utxos: &UTxOs,
) -> ValidationResult
```

## Environment Struct

The `Environment` context required for all validation:
- `pparams: MultiEraProtocolParameters` — era-specific protocol params
- `magic: u32` — network magic (used for Byron witness signing)
- `block_slot: u64` — current block slot (for TTL checks)
- `network_id: u8` — 0 = testnet, 1 = mainnet (for address validation)
- `acnt: Option<AccountState>` — optional treasury/reserves (required for Shelley+)

## MultiEraProtocolParameters Variants

```rust
pub enum MultiEraProtocolParameters {
    Byron(ByronProtParams),
    Shelley(ShelleyProtParams),
    Alonzo(AlonzoProtParams),
    Babbage(BabbageProtParams),
    Conway(ConwayProtParams),
}
```

Each era has a corresponding struct with all relevant protocol parameters.

## Validation Rules Per Era

### Byron
- inputs not empty
- outputs not empty
- all inputs in UTxO set
- outputs have non-zero lovelace
- fee validation: `summand + multiplier * size`
- max tx size check
- Ed25519 witness verification per input

### Shelley/Allegra/Mary
- inputs not empty
- all inputs in UTxO set
- TTL not exceeded (upper bound only; no lower bound in Shelley)
- max tx size
- min lovelace per output (era-specific calculation)
- value preservation (inputs = outputs + fees + deposits - refunds + minting)
- min fee check
- network ID for output addresses
- metadata hash verification
- vkey witness verification
- native script validation (timelocks, signature requirements)
- minting policy check
- certificate validation (stake registration, delegation, pool operations, genesis delegations, MIR)
- Mary: multiasset NOT allowed in Shelley-era transaction check

### Alonzo
All Shelley/MA rules plus:
- collateral inputs validation (required when Plutus scripts present)
- max collateral count check
- collateral must be vkey-locked addresses
- collateral lovelace minimum
- collateral non-lovelace assets check
- tx execution units within max
- redeemer set matches Plutus script pointers
- datum hashes match witness set
- required signers validation
- Plutus language version support check
- script integrity hash validation
- script witness completeness (needed/unneeded scripts)

### Babbage (extends Alonzo)
- same as Alonzo
- reference inputs included in UTxO lookups
- separate `check_tx_outs_network_id` for output address network
- `check_input_scripts` for inline scripts
- `check_minting_policies` for minting policy matching

### Conway (extends Babbage)
- same as Babbage
- `check_all_ins_in_utxos` includes reference inputs and collateral
- (Note: governance certificate validation not yet explicitly visible in check names)

## Error Type Hierarchy

```rust
pub enum ValidationError {
    TxAndProtParamsDiffer,           // era mismatch
    PParamsByronDoesntNeedAccountState,
    EnvMissingAccountState,          // Shelley+ requires AccountState
    UnknownProtParams,
    Byron(ByronError),
    ShelleyMA(ShelleyMAError),
    Alonzo(AlonzoError),
    PostAlonzo(PostAlonzoError),     // Babbage + Conway share this
}
```

AlonzoError variants (representative):
`TxInsEmpty`, `InputNotInUTxO`, `CollateralNotInUTxO`, `BlockPrecedesValInt`, `BlockExceedsValInt`, `FeeBelowMin`, `CollateralMissing`, `TooManyCollaterals`, `CollateralNotVKeyLocked`, `CollateralMinLovelace`, `NonLovelaceCollateral`, `PreservationOfValue`, `MinLovelaceUnreached`, `MaxValSizeExceeded`, `OutputWrongNetworkID`, `TxWrongNetworkID`, `MaxTxSizeExceeded`, `TxExUnitsExceeded`, `RedeemerMissing`, `UnneededNativeScript`, `UnneededPlutusScript`, `UnneededRedeemer`, `UnneededDatum`, `ScriptWitnessMissing`, `DatumMissing`, `MetadataHash`, `ScriptIntegrityHash`, `VKWitnessMissing`, `VKWrongSignature`, `ReqSignerMissing`, `ReqSignerWrongSig`, `MintingLacksPolicy`

## Test Coverage

Tests in `pallas-validate/tests/`:
- `byron.rs` — positive and negative mainnet tx tests
- `shelley_ma.rs` — native scripts, metadata, minting, pool/staking certs
- `alonzo.rs` — Plutus script structure, collateral, redeemers, datums
- `babbage.rs` — reference scripts, Plutus V1/V2, collateral handling
- `conway.rs` — Conway era tests
- `common.rs` — shared test utilities

Uses real mainnet transaction examples (not synthetic).

## Known Limitations / Gaps

1. **No reference script fee calculation (CIP-0112)**: pallas-validate does not implement the 25KiB-tier reference script fee. Dugite has this implemented in `dugite-ledger`.
2. **No certificate state tracking**: The validation doesn't maintain cert state (who's registered, what pools exist) — it validates structural correctness but not cert ordering rules.
3. **No reward/withdrawal validation against actual stake state**: Withdrawal amounts not checked against actual reward account balances.
4. **Phase-2 status uncertain**: pallas-uplc 0.1.0 is the Plutus evaluator dependency — its maturity is unclear.
5. **No CIP-0112 ref script cost check**: Missing from the fee calculation.
6. **Conway governance cert validation**: Not visible in the phase1/conway.rs function list — may be incomplete for DRep/CC certs.
7. **Alpha stability**: All 1.x APIs are alpha and may break between releases.

### CRITICAL BUG: script_data_hash multi-language support is broken in pallas-validate

`check_script_data_hash` in `pallas-validate/src/phase1/conway.rs` calls `cost_model_for_tx()` which uses `itertools::max(tx_languages.iter())` — this picks only the **highest** language version and builds a single `LanguageView` from it. This is fundamentally wrong for multi-language transactions.

The Haskell spec (`getConwayScriptsNeeded` / `mkScriptIntegrity`) produces a **Set** of `LangDepView`, one per language used, and encodes them all as a map. A V1+V2 transaction needs TWO entries in the language views map.

The `pallas-primitives::conway::ScriptData` struct itself has the same design flaw — it holds `language_view: Option<LanguageView>` (singular), not a set.

Additionally, `tx_languages()` in pallas-validate does NOT do `scriptsNeeded ∩ scriptsProvided` intersection — it just checks presence of scripts in the witness set and reference inputs. This means it can include languages from scripts that are provided but not needed.

### CRITICAL BUG: pallas-validate Conway `tx_languages()` has a V1+V2+V3 bug

```rust
// From pallas-validate/src/phase1/conway.rs
} else {
    vec![Language::PlutusV1, Language::PlutusV2]  // BUG: drops V3!
}
```

When all three languages are present, the function returns only `[V1, V2]`, dropping V3 entirely. This is a hardcoded bug in the multi-language branch.

## Comparison to dugite-ledger

Dugite's `dugite-ledger` implements:
- Phase-1 validation (structural + signature checks) — overlaps with pallas-validate
- CIP-0112 reference script fee calculation (NOT in pallas-validate)
- UTxO set management
- Reward distribution
- Certificate state (DRep registration, pool registration, stake delegation)
- Governance ratification (DRep/SPO/CC voting)
- Ledger state snapshots

**pallas-validate covers**: Phase-1 structural checks, signature verification, fee minimums, value preservation, script witness completeness.

**pallas-validate does NOT cover**: Ledger state management, reward calculation, governance, certificate ordering.

## Adoption Recommendation

**ADAPT**: pallas-validate Phase-1 logic is well-tested against real transactions. Consider adopting for the validation checks themselves, while dugite-ledger retains state management. The main gap is CIP-0112 reference script fee and the cert state context needed for deeper validation.

The biggest value is Plutus Phase-2 via pallas-uplc once that crate matures — dugite does not yet have Plutus execution.
