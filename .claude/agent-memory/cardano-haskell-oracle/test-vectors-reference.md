---
name: test-vectors-reference
description: Comprehensive catalog of test vectors from Cardano Haskell repos for conformance testing
type: reference
---

# Cardano Haskell Test Vector Catalog

## 1. cardano-ledger — Ledger Test Vectors

### CDDL Specs (normative wire format definitions)
- `eras/conway/impl/cddl/data/conway.cddl` — Conway era CDDL
- `eras/babbage/impl/cddl/data/babbage.cddl` — Babbage era CDDL
- `eras/alonzo/impl/cddl/data/alonzo.cddl` — Alonzo era CDDL
- `eras/shelley/impl/cddl/data/shelley.cddl` — Shelley era CDDL
- `eras/mary/impl/cddl/data/mary.cddl` — Mary era CDDL
- `eras/allegra/impl/cddl/data/allegra.cddl` — Allegra era CDDL
- Auto-generated from HuddleSpec.hs via generate-cddl tool

### Golden PParams JSON (per-era, JSON roundtrip tests)
- `eras/conway/impl/golden/pparams.json` — Conway PParams (31 fields)
- `eras/conway/impl/golden/pparams-update.json` — Conway PParamsUpdate (map form)
- `eras/babbage/impl/golden/pparams.json` / `pparams-update.json`
- `eras/alonzo/impl/golden/pparams.json` / `pparams-update.json`
- `eras/shelley/impl/golden/pparams.json` / `pparams-update.json`
- **Format**: JSON, arbitrary test values (not real mainnet params)
- **Test**: `goldenJsonPParamsSpec` roundtrips JSON encode/decode

### Golden Translation CBOR (era transition tests)
- `eras/conway/impl/golden/translations.cbor` — Conway translation golden
- `eras/babbage/impl/golden/translations.cbor`
- `eras/alonzo/impl/golden/translations.cbor`

### Alonzo Block/Tx Golden CBOR
- `eras/alonzo/test-suite/golden/block.cbor` — 1865 bytes, raw CBOR binary
- `eras/alonzo/test-suite/golden/tx.cbor` — 865 bytes, raw CBOR binary
- `eras/alonzo/test-suite/golden/hex-block-node-issue-4228.cbor` — regression test
- **Test**: `Test.Cardano.Ledger.Alonzo.Golden` decodes and validates

### Alonzo Golden JSON
- `eras/alonzo/test-suite/golden/FailureDescription.json`
- `eras/alonzo/test-suite/golden/IsValid.json`
- `eras/alonzo/test-suite/golden/TagMismatchDescription.json`
- `eras/alonzo/test-suite/golden/ValidityInterval.json`
- `eras/alonzo/test-suite/golden/mainnet-alonzo-genesis.json`

### Byron CBOR Golden (raw binary files, no extension)
- `eras/byron/ledger/impl/golden/cbor/` — extensive per-type golden CBOR
  - block/, common/, delegation/, mempoolpayload/, slotting/, ssc/, update/, utxo/
  - Each file is raw CBOR binary

### Shelley Golden
- `eras/shelley/test-suite/test/Golden/ShelleyGenesis` — Shelley genesis JSON
- `eras/shelley/impl/golden/pparams.json`
- Address golden tests in `Test.Cardano.Ledger.Shelley.Serialisation.Golden.Address`
  - Contains hex-encoded address test vectors inline in Haskell

### Non-Integral Golden (VRF math)
- `libs/non-integral/reference/golden_tests.txt` — 10.6MB, input triples
  - Format: `sigma f certNat` (3 space-separated integers per line)
- `libs/non-integral/reference/golden_tests_result.txt` — expected outputs
  - Format: `result ln_result exp_result exp1_result comparison bool` per line
  - Fields: exp/ln computation results, GT/LT comparison, 0/1 leader check
- **How to use**: test `ln'`, `exp'`, `(***)`, `taylorExpCmp` functions
- C reference impl: `libs/non-integral/reference/non_integral.c`

### Conway ImpTests (most comprehensive ledger tests — but Haskell-only)
- `eras/conway/impl/testlib/Test/Cardano/Ledger/Conway/Imp/*.hs`
  - BbodySpec, CertsSpec, DelegSpec, EnactSpec, EpochSpec, GovCertSpec
  - GovSpec, HardForkSpec, LedgerSpec, RatifySpec, UtxoSpec, UtxosSpec, UtxowSpec
- Not serialized vectors — require running Haskell test framework

### Conformance Tests (Agda spec vs Haskell impl)
- `libs/cardano-ledger-conformance/` — Tests Haskell impl against Agda formal spec
  - Per-rule: Cert, Certs, Deleg, Enact, Epoch, Gov, GovCert, Ledger, Ledgers, NewEpoch, Pool, Ratify, Utxo, Utxow

### Shelley Chain Examples (reward calculation reference)
- `eras/shelley/test-suite/test/Test/Cardano/Ledger/Shelley/Examples/TwoPools.hs`
  - THE canonical reference for reward calculation
  - Constructs 2 pools with different params, computes rewards over epochs
  - Not serialized, but contains hardcoded expected reward amounts

## 2. ouroboros-consensus — 1620 Golden CBOR Files

### Location
- `ouroboros-consensus-cardano/golden/` directory

### Structure
```
golden/
  byron/
    ByronNodeToNodeVersion2/       — N2N blocks, headers, txs
    QueryVersion2/ByronNodeToClientVersion1/  — N2C queries/results
    QueryVersion3/ByronNodeToClientVersion1/
    disk/                          — disk persistence format
  cardano/
    CardanoNodeToNodeVersion2/     — HFC-wrapped N2N blocks/headers/txs
    QueryVersion2/CardanoNodeToClientVersion{12-15}/  — N2C queries/results
    QueryVersion3/CardanoNodeToClientVersion{16-19}/
    disk/                          — HFC disk persistence
  shelley/
    ShelleyNodeToNodeVersion1/     — Shelley-only N2N
    QueryVersion2/ShelleyNodeToClientVersion{8-11}/
    QueryVersion3/ShelleyNodeToClientVersion{12-15}/
    disk/
```

### Format
- All files are **raw CBOR binary** (no hex encoding, no headers)
- Each file = one CBOR-encoded value

### Key Files for Conformance
**N2C Query/Result vectors** (CardanoNodeToClientVersion19):
- `Query_Conway_GetCurrentPParams`: `820082068103`
- `Query_Conway_GetLedgerTip`: `820082068100`
- `Query_Conway_GetEpochNo`: `820082068101`
- `Result_Conway_EmptyPParams`: full array(31) Conway PParams CBOR
- `Result_Conway_EpochNo`: `810a`
- `Result_EraMismatchByron`: `828201675368656c6c65798200654279726f6e`
- `Result_HardFork`: full EraHistory encoding

**N2N Block vectors** (CardanoNodeToNodeVersion2):
- `Block_Conway`, `Block_Babbage`, etc. — full HFC-wrapped blocks
- `Header_Conway`, `Header_Babbage`, etc.
- `GenTx_Conway`, `GenTxId_Conway`, etc.

**Disk format vectors**:
- `LedgerState_Conway`, `ChainDepState_Conway`, `ExtLedgerState_Conway`
- `AnnTip_Conway`, `HeaderHash_Conway`, `LedgerTables_Conway`

### Test Generator
- `ouroboros-consensus-cardano/test/cardano-test/Test/Consensus/Cardano/Golden.hs`
- Uses `Test.Util.Serialisation.Golden.goldenTest_all`
- Examples from `Test.Consensus.Cardano.Examples`

### CDDL Specs (consensus-level wire formats)
- `ouroboros-consensus-cardano/cddl/base.cddl` — HFC telescope, NS types
- `ouroboros-consensus-cardano/cddl/node-to-node/blockfetch/block.cddl` — N2N block wrapping
- `ouroboros-consensus-cardano/cddl/node-to-node/chainsync/header.cddl` — N2N header wrapping
- `ouroboros-consensus-cardano/cddl/disk/ledger/stateFile.cddl` — snapshot format
- `ouroboros-consensus-cardano/cddl/disk/ledger/praos.cddl` — PraosState
- `ouroboros-consensus-cardano/cddl/disk/ledger/ledgerstate.cddl` — telescope ledger state
- `ouroboros-consensus-cardano/cddl/disk/ledger/headerstate.cddl` — header state

## 3. ouroboros-network — Protocol CDDL Specs

### Location
- `cardano-diffusion/protocols/cddl/specs/`

### Files
- `handshake-node-to-node-v14.cddl` — N2N handshake
- `handshake-node-to-client.cddl` — N2C handshake (versions 32784-32791)
- `chain-sync.cddl` — ChainSync mini-protocol
- `block-fetch.cddl` — BlockFetch mini-protocol
- `tx-submission2.cddl` — TxSubmission2 mini-protocol
- `keep-alive.cddl` — KeepAlive mini-protocol
- `peer-sharing-v14.cddl` — PeerSharing mini-protocol
- `local-state-query.cddl` — LocalStateQuery mini-protocol
- `local-tx-submission.cddl` — LocalTxSubmission mini-protocol
- `local-tx-monitor.cddl` — LocalTxMonitor mini-protocol
- `node-to-node-version-data-v14.cddl` — N2N version data
- `network.base.cddl` — Base types (block, header, tip, point as `any`)

## 4. plutus — UPLC Conformance Tests (999 test cases)

### Location
- `plutus-conformance/test-cases/uplc/evaluation/`

### Format
- `<test>.uplc` — UPLC program in textual format
- `<test>.uplc.expected` — Expected evaluation result (UPLC text or "evaluation failure")
- `<test>.uplc.budget.expected` — Expected budget `({cpu: N | mem: M})`

### Categories
- `builtin/semantics/` — 797 tests for all Plutus builtins
- `builtin/constant/` — 97 tests for constant encoding
- `builtin/interleaving/` — 18 tests
- `term/` — 69 tests for term evaluation (app, case, constr, force, etc.)
- `example/` — 13 end-to-end examples (factorial, fibonacci, etc.)

## 5. ouroboros-consensus — Nonce Computation Tests

### Location
- `docs/agda-spec/src/Spec/hs-src/test/TickNonceSpec.hs`
- `docs/agda-spec/src/Spec/hs-src/test/UpdateNonceSpec.hs`

### Content
- Simple unit tests with hardcoded nonce values
- TickNonce: η₀=3, ηh=4, ηc=2, ηph=5
  - Signal=False: state unchanged
  - Signal=True: η₀ = 3+2+1 = 6, ηh = ηph = 5
- UpdateNonce: stability window tests
