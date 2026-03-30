---
name: project_state_2026_03_14
description: Comprehensive assessment of Torsten capability and production readiness as of 2026-03-14
type: project
---

# Torsten State Assessment — 2026-03-14

**Why:** First comprehensive product-owner-level assessment to establish a baseline for roadmap planning.

**How to apply:** Use this as a reference when asked about current state, what's complete vs partial, and what to prioritize next.

## Test Suite Health
- All tests pass: 1,608 tests across all crates, 0 failures
- Clippy clean: zero warnings
- Conformance tests: 174 test vectors (UTXO, CERT, GOV, EPOCH) — all pass
- CI runs: build, fmt, clippy, test, offline integration tests, release binary builds

## Key Capability Ratings

### Sync: MOSTLY COMPLETE
- Mithril import works on preview/preprod/mainnet — 4M blocks in ~2 min
- Pipelined ChainSync (depth 300) with 4 parallel block fetchers
- Genesis sync from genesis block: works but no Ouroboros Genesis trustless bootstrap (LoE not enforced in pipeline)
- AVVM genesis UTxO seeding works (nonAvvmBalances + avvmDistr via Byron redeem addresses)
- Strict verification only activates after node catches up; during sync, VRF/KES failures are non-fatal

### Protocol Compliance: MOSTLY COMPLETE
- N2N: ChainSync, BlockFetch, TxSubmission2, KeepAlive, PeerSharing all implemented
- N2C: LocalStateQuery tags 0-38 all implemented, LocalTxSubmission, LocalTxMonitor, LocalChainSync
- N2N V14/V15 handshake, N2C V16-V22 with bit-15 encoding

### Ledger Validation: MOSTLY COMPLETE
- Phase-1: Full UTxO rules, fee validation (CIP-0112 tiered ref script fee), script data hash, conservation, native scripts, era gating
- Phase-2: Plutus V1/V2/V3 via uplc CEK machine (eval_phase_two_raw)
- ValidationTagMismatch: correctly detected and blocks rejected
- Certificate processing: staking, pool reg/retire, DRep, vote delegation, Conway certs
- Reward calculation: full maxPool' formula with a0, n_opt, Rat arithmetic (i128 overflow-safe)

### Consensus: MOSTLY COMPLETE
- VRF leader check: exact 34-digit dashu IBig arithmetic, Euler continued fraction ln, taylorExpCmp
- TPraos vs Praos distinction (proto < 7 vs >= 7) correctly handled
- KES: Sum6Kes, period validation, non-fatal during sync, fatal after catch-up
- Opcert: raw bytes signable, counter monotonicity, retirement pruning
- Chain selection: Byron (density) + Shelley+ (longest chain) with hash tiebreaker
- Block body hash verification

### Block Production: MOSTLY COMPLETE
- VRF proof generation (vrf_dalek ECVRF)
- KES signing (Sum6Kes depth-6)
- Opcert loading and signing
- Block forging (forge_block()) and announcement via broadcast channel
- Relay mode: synced blocks announced to downstream N2N peers

### Governance (CIP-1694): MOSTLY COMPLETE
- All 7 GovAction types implemented (ParameterChange, HardFork, NoConfidence, UpdateCommittee, NewConstitution, TreasuryWithdrawals, InfoAction)
- DRep/SPO/CC threshold logic with exact rational arithmetic
- Governance action enactment (treasury withdrawal, protocol param update, hard fork, committee changes)
- DRep activity tracking, no_confidence flag
- LocalStateQuery tags 23-32 all implemented for governance

### Operational Readiness: MOSTLY COMPLETE
- 18+ Prometheus metrics on port 12798
- Graceful shutdown (SIGTERM/SIGINT/Ctrl-C with watch channel)
- SIGHUP topology reload
- Disk space monitoring (10GB warning, 2GB critical, 500MB fatal)
- Snapshot save/load with TRSN magic + blake2b checksum + BufWriter
- VolatileDB WAL for crash recovery
- Config format compatible with cardano-node JSON configs

## Known Gaps (as of 2026-03-14)

1. **Ouroboros Genesis bootstrap**: LoE enforcement not wired into block pipeline; GDD and HAA state tracked but not enforced. Genesis mode is opt-in via --consensus-mode genesis.

2. **cardano-lsm fork dependency**: Using michaeljfazio/cardano-lsm-rust fix/index-based-get branch (O(n) SSTable get() fix submitted upstream as PR #2). Should migrate to upstream once merged.

3. **pallas version**: Using 1.0.0-alpha.5 (pre-release). Should track for stable release.

4. **Reward calculation accuracy**: Implemented against spec; not cross-validated against mainnet reward history. Could have subtle divergences in pledge-influence or n_opt saturation edge cases.

5. **Byron transaction validation**: ByronLedger struct is a stub with no actual transaction validation. Byron-era tx validation is skipped (blocks pass through without UTxO checks). This is acceptable for sync-from-Mithril (post-Byron) but breaks genesis-from-Byron scenarios needing full Byron ledger validation.

6. **uplc version**: Using 1.1 (not pegged to pallas ecosystem). Plutus V3 cost model support depends on uplc's implementation.

7. **Governance conformance testing**: governance.rs has 0 dedicated unit tests. The conformance test vectors cover basic cases but not threshold edge cases or complex multi-action ratification sequences.

## Risk Areas for Production

- Non-strict verification window: after Mithril import, nodes skip VRF/KES checks for 2-3 epochs — an adversarial peer could feed malformed blocks during this window
- cardano-lsm fork dependency: not on a stable tagged release
- Reward calculation not validated against historical mainnet data
- Byron ledger validation is a stub (blocks applied without full Byron UTxO rules)
