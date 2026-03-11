# Formal Ledger Specification Conformance Tests

This directory contains infrastructure for validating Torsten's ledger
implementation against the Cardano formal ledger specification.

## Architecture

```
Agda formal spec ──► Haskell (MAlonzo) ──► JSON test vectors ──► Rust test harness
     (source)           (compiled)            (intermediate)         (validation)
```

**Source**: The Cardano formal ledger specification is written in Agda at
[IntersectMBO/formal-ledger-specifications](https://github.com/IntersectMBO/formal-ledger-specifications)
(branch `conway-v1.0`).

**Compilation**: The Agda spec compiles to Haskell via MAlonzo, producing the
`cardano-ledger-executable-spec` library with step functions for each STS rule.

**Test vectors**: A Haskell generator (`generator/Main.hs`) calls the step
functions with various inputs and serializes the results as JSON. The current
vectors in `vectors/` are hand-crafted examples that demonstrate the schema.

**Validation**: The Rust crate `torsten-conformance` loads the JSON vectors,
converts them to Torsten types via adapter functions, runs the corresponding
ledger function, and compares the result against the expected output.

## Directory Structure

```
tests/conformance/
├── Cargo.toml              # Rust crate definition
├── README.md               # This file
├── src/
│   ├── lib.rs              # Crate root
│   ├── schema.rs           # Test vector JSON schema types
│   ├── adapters.rs         # Agda type ↔ Torsten type converters
│   └── runner.rs           # Test execution and comparison engine
├── tests/
│   └── conformance_tests.rs  # Integration test entry point
├── vectors/                # JSON test vectors
│   ├── utxo/               # UTXO rule vectors
│   └── cert/               # CERT rule vectors
└── generator/              # Haskell test vector generator (TODO)
    ├── Main.hs             # Generator scaffold
    └── cabal.project       # Cabal project file
```

## Running Tests

```bash
# Run all conformance tests
cargo test -p torsten-conformance

# Run with output
cargo test -p torsten-conformance -- --nocapture

# Run only UTXO tests
cargo test -p torsten-conformance conformance_utxo

# Run only CERT tests
cargo test -p torsten-conformance conformance_cert
```

## Supported Rules

| Rule   | Status       | Description |
|--------|-------------|-------------|
| UTXO   | 6 vectors   | Transaction validation against UTxO state |
| CERT   | 5 vectors   | Certificate processing (delegation, registration, DRep) |
| GOV    | Planned     | Governance actions (Conway era) |
| EPOCH  | Planned     | Epoch boundary transitions |

## Test Vector Schema

Each test vector is a JSON file with this structure:

```json
{
  "rule": "UTXO",
  "description": "Human-readable description",
  "environment": { ... },
  "input_state": { ... },
  "signal": { ... },
  "expected_output": {
    "type": "success",
    "state": { ... }
  }
}
```

For failure cases:

```json
{
  "expected_output": {
    "type": "failure",
    "errors": ["ValueNotConserved"]
  }
}
```

### Type Mapping

The Agda formal spec uses abstract types. The test vectors use simplified
representations that map to Torsten's concrete types:

| Agda Type     | JSON Representation          | Torsten Type              |
|---------------|------------------------------|---------------------------|
| TxId          | 64-char hex string           | `Hash32` (TransactionHash)|
| Addr          | Tagged enum (base/enterprise/reward/byron) | `Address` enum |
| Credential    | `{"type": "vkey", "hash": "..."}` | `Credential` enum   |
| Coin          | Integer (lovelace)           | `Lovelace(u64)`           |
| UTxO          | Array of `{tx_hash, index, output}` | `UtxoSet`          |
| PParams       | Subset of protocol params    | `ProtocolParameters`      |

## Regenerating Test Vectors (Haskell Generator)

The Haskell generator requires a Nix environment with the compiled Agda spec.
This is a complex build that requires ~30 minutes and significant disk space.

### Prerequisites

- [Nix](https://nixos.org/download.html) with flakes enabled
- ~10GB disk space for the Agda/GHC build cache

### Steps

```bash
# 1. Enter the formal spec Nix shell
nix develop github:IntersectMBO/formal-ledger-specifications/conway-v1.0

# 2. Navigate to the generator
cd tests/conformance/generator

# 3. Build and run
cabal run conformance-generator -- --output-dir ../vectors/
```

### Using the Reference Pattern

The formal-ledger-specifications repo includes a conformance example at
`conformance-example/test/UtxowSpec.hs` that demonstrates how to:

1. Import the compiled Agda step functions
2. Construct test inputs using QuickCheck generators
3. Call the step function and compare results
4. Serialize to/from the test vector format

## Adding New Rules

1. **Schema**: Add new types in `src/schema.rs` for the rule's environment,
   state, and signal types.

2. **Adapters**: Add conversion functions in `src/adapters.rs` to map between
   the test vector types and Torsten's types.

3. **Runner**: Add a `run_<rule>_test()` function in `src/runner.rs` and
   register it in the `run_test()` dispatch.

4. **Vectors**: Create JSON test vectors in `vectors/<rule>/`.

5. **Integration test**: Add a `conformance_<rule>_vectors()` test in
   `tests/conformance_tests.rs`.

## Limitations

- The hand-crafted vectors test basic scenarios. Full coverage requires the
  Haskell generator with property-based testing.
- UTXO validation skips witness verification (signatures/scripts) since test
  vectors use simplified types without cryptographic material.
- The CERT runner simulates certificate application directly rather than going
  through `LedgerState::process_certificate` (which is not public API at the
  UTxO level).
- Error matching uses category names rather than exact error types, since the
  formal spec's error types are more abstract than Torsten's.
