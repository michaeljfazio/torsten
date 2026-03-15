---
name: capability_gaps
description: Known capability gaps and partial implementations in Torsten
type: project
---

# Torsten Capability Gaps

**Why:** Track what's not implemented or partially implemented to inform roadmap.

**How to apply:** When asked what needs work, consult this list and classify priority.

## HIGH Priority Gaps

### Reward Calculation Cross-Validation (NOT DONE)
- All 19 reward tests cover Rat arithmetic primitives only (GCD, overflow safety)
- Zero tests validate maxPool' formula output against known-good values
- No end-to-end reward calculation test with real epoch data
- Risk: same class as the Alonzo VRF nonce_vrf bug — formula looks correct, tests pass, but could be wrong for real inputs
- Fix: use Koios historical epoch data (pool_history + epoch_params) as ground truth; Koios MCP is available on preview

### Byron Ledger Validation (NOT IMPLEMENTED)
- `ByronLedger` struct is a 12-line stub with no transaction validation
- Byron-era blocks pass through `apply_block` without UTxO rule enforcement
- Impact: genesis-from-Byron sync accumulates incorrect UTxO state; Mithril users unaffected
- File: `crates/torsten-ledger/src/eras/byron.rs`

### Ouroboros Genesis LoE Enforcement (PARTIAL)
- `loe_limit()` method exists and computes the constraint
- NOT wired into block application pipeline — blocks applied eagerly regardless
- GDD, HAA state tracked; peer-selection not differentiated for genesis mode
- File: `crates/torsten-node/src/gsm.rs`

## MEDIUM Priority Gaps

### cardano-lsm Fork Dependency (PARTIAL)
- Using `michaeljfazio/cardano-lsm-rust.git` branch `fix/index-based-get`
- Fix submitted upstream as PR #2; not yet merged to crates.io
- Risk: not a stable tagged release; no semver guarantees
- Cargo.toml: `cardano-lsm = { git = "...", branch = "fix/index-based-get" }`

### Plutus Cost Models (PARTIAL)
- V1/V2/V3 cost models stored and passed to uplc
- Cost model CBOR encoding/decoding from protocol parameters is implemented
- Not validated against cardano-node's eval results for complex scripts

## COMPLETED (recent)

### Governance Compliance Bugs (FIXED 2026-03-14)
- 6 governance bugs fixed: ratification logic, voting thresholds, committee handling, proposal lifecycle
- 45 unit tests added across governance.rs; now has 45 dedicated tests
- Matches Haskell cardano-ledger spec for CIP-1694 ratification

## LOW Priority Gaps

### Ouroboros Genesis Peer Selection (NOT IMPLEMENTED)
- Standard P2P governor policy used in all GSM states
- Full Genesis would need dedicated BLP-prioritized peer selection during PreSyncing/Syncing

### CDDL Compliance Verification (NOT DONE)
- No automated CDDL conformance testing against official Cardano CDDL specs
- Wire format correctness verified through integration tests with real cardano-node peers

### Lightweight Checkpointing (NOT IMPLEMENTED)
- Genesis spec calls for lightweight checkpoints to speed up trustless bootstrap
- Only relevant for --consensus-mode genesis (not default Mithril path)
