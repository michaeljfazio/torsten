---
name: capability_gaps
description: Known capability gaps and partial implementations in Dugite
type: project
---

# Dugite Capability Gaps

**Why:** Track what's not implemented or partially implemented to inform roadmap.

**How to apply:** When asked what needs work, consult this list and classify priority.

## HIGH Priority Gaps

### Reward Cross-Validation Against Historical Data (NOT DONE)
- Formula tests cover Rat arithmetic primitives and synthetic maxPool unit tests only
- Zero tests validate actual per-epoch reward output against known-good historical values
- Koios MCP available: koios_pool_history + koios_epoch_params can give ground truth
- Risk: same class as the Alonzo VRF nonce_vrf bug — formula looks correct, tests pass, but could produce wrong per-delegator amounts for real inputs
- RUPD timing change (2026-03-15) makes this even more urgent — rewards now land 1 epoch later; distribution amounts could have regressed

### Testnet Re-Validation After RUPD Change (NOT DONE)
- The deferred RUPD timing change alters when rewards land in reward accounts
- Validator run #9 was against the old (immediate) reward code
- Need a new full sync run from Mithril snapshot to verify 0 validation errors still hold
- Affects: LocalStateQuery GetRewardAccountBalance, snapshot content, treasury accounting

### Byron Ledger Validation (DONE — 2026-03-18 confirmed)
- `validate_byron_tx` and `apply_byron_block` fully implemented (834 lines, 9 tests)
- Rules: min 1 input, all inputs present, ADA-only outputs, value conservation, min fee
- `ByronApplyMode::ValidateAll` (live blocks) and `ApplyOnly` (replay) both implemented
- File: `crates/dugite-ledger/src/eras/byron.rs`

### Ouroboros Genesis LoE Enforcement (PARTIAL)
- `loe_limit()` method exists and computes the constraint
- NOT wired into block application pipeline — blocks applied eagerly regardless
- GDD, HAA state tracked; peer-selection not differentiated for genesis mode
- File: `crates/dugite-node/src/gsm.rs`

## MEDIUM Priority Gaps

### dugite-lsm Mainnet Scale Testing (PARTIAL)
- Validated at preview testnet scale (~3M UTxOs) — correct
- Not tested at mainnet scale (20M+ UTxOs, compaction under concurrent flush, WAL crash recovery)
- No benchmark against prior cardano-lsm baseline (target: ~10,600 blocks/s ImmutableDB replay)

### Plutus Cost Models (PARTIAL)
- V1/V2/V3 cost models stored and passed to uplc
- Not validated against cardano-node's eval results for complex scripts

## COMPLETED (recent)

### Governance Compliance Bugs (FIXED 2026-03-14)
- 6 governance bugs fixed: ratification logic, voting thresholds, committee handling, proposal lifecycle
- 45 unit tests added across governance.rs

### dugite-lsm (DONE 2026-03-15)
- Custom pure Rust LSM engine replacing cardano-lsm
- 1,830+ tests pass; preview testnet: 2,938,963 correct UTxOs

### RUPD Timing Alignment (DONE 2026-03-15)
- Rewards deferred 1 epoch matching Haskell's PendingRewardUpdate
- Reward formulas confirmed correct vs Koios epoch 1235 data

### PageOverflow Fix (DONE 2026-03-15)
- Large values (13KB+ inline datums) handled via jumbo pages

## LOW Priority Gaps

### Ouroboros Genesis Peer Selection (NOT IMPLEMENTED)
- Standard P2P governor policy used in all GSM states
- Full Genesis would need BLP-prioritized peer selection during PreSyncing/Syncing

### CDDL Compliance Verification (NOT DONE)
- No automated CDDL conformance testing against official Cardano CDDL specs
- Wire format correctness verified through integration tests only
