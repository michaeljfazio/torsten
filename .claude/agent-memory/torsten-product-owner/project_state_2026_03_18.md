---
name: project_state_2026_03_18
description: Comprehensive gap and compliance analysis as of 2026-03-18 — soak test on preview testnet as BP, epoch 1239, ~1019 ADA delegated
type: project
---

# Torsten State Assessment — 2026-03-18 (Soak Test Analysis)

**Why:** Tracks current capability and gap state during live soak test on preview testnet as block producer.

**How to apply:** Use when asked about current state, compliance level, or what to work on next.

---

## Session Context

- Branch: main, clean working tree
- Version: 0.4.3-alpha
- 13 crates in workspace
- Soak test: preview testnet, epoch 1239, ~1019 ADA delegated, running as block producer
- Most recent major commits: fork recovery + epoch numbering + zero-stake BP fixes (feaaf69)
- N2N: V14/V15/V16 advertised; N2C: V16-V22 supported

## Protocol Compliance — Summary

### Consensus (Ouroboros Praos): ~90% complete
- Praos leader check: exact 34-digit fixed-point arithmetic (IBig via dashu), lncf, taylorExpCmp — DONE
- VRF V1/V2 (TPraos vs Praos): DONE (certNatMax split at proto >= 7)
- VRF/KES header validation: DONE (validate_header_full)
- Fork recovery: DONE (feaaf69) — 5 new tests
- Ouroboros Genesis density-based chain selection: PARTIALLY wired (loe_limit() used in sync flush)
- Genesis BLP-prioritized peer selection: NOT IMPLEMENTED (uses standard P2P governor)
- Epoch transitions: DONE (mark/set/go, RUPD deferred timing)

### N2N Mini-Protocols: ~95% complete
- ChainSync: DONE (pipelined depth 300)
- BlockFetch: DONE (4 fetchers)
- TxSubmission2: DONE (3 critical bugs fixed)
- KeepAlive: DONE
- PeerSharing: DONE (protocol handler, privacy filter: outbound-only)
- N2N V14/V15/V16: DONE

### N2C Mini-Protocols: ~97% complete
- LocalChainSync: DONE
- LocalStateQuery: ALL 38 tags (0-38) IMPLEMENTED — verified in query_handler/mod.rs:416-508
- LocalTxSubmission: DONE
- LocalTxMonitor: DONE
- N2C V16-V22 (bit-15 encoding): DONE
- CBOR encoding: 25 golden fixture files, 77+ encoding tests

### Block Validation: ~88% complete
- Phase-1 (structural): DONE — 31 conformance vectors, 89+ unit tests
- Byron ledger: DONE (validate_byron_tx, apply_byron_block — NOT the stub we thought)
- Phase-2 Plutus: DONE (uplc CEK machine, V1/V2/V3 scripts)
  - Known divergence: marginal budget cases (~0.1% difference vs Haskell) handled via warn-and-trust
  - ValidationTagMismatch: enforced for invalid (is_valid=false) blocks claiming scripts pass
- Phase-1 false positive: InvalidMint on within-block UTxO dependencies — FIXED (PR #98)
  - Tests: test_within_block_ref_script_minting_policy_visible in state/tests.rs:9867
- Phase-1 divergence on confirmed blocks: WARN and continue (non-fatal, per spec intent)
- Reward calculation: formula correct (Koios cross-validated epoch 1235), RUPD deferred correctly

### Governance (CIP-1694): ~95% complete
- DRep/SPO/CC voting thresholds: DONE
- Ratification pipeline: DONE (6 bugs fixed, 45 tests)
- Committee: DONE (UnelectedCommitteeMember check, no_confidence state)
- One known gap: hot_credential_type defaults to 0 (KeyHash) — script-based CC hot keys not tracked
  (query.rs:315 — TODO comment)

## Operational Readiness — Summary

### Block Producer: ~85% (soak-tested, known pre-existing issues resolved)
- Fork recovery: DONE (orphaned block, intersection contamination)
- Epoch numbering: FIXED (epoch 445→1239 on preview)
- Zero-stake: FIXED (premature rebuild_stake_distribution)
- Non-fatal VRF until 3 live epoch transitions: DONE
- Block announcement to relays: DONE

### Relay: ~90%
- Announced synced blocks to downstream N2N peers: DONE
- P2P peer manager: DONE

### Mithril import: DONE (4M blocks in ~2 min)

### Graceful shutdown: DONE (SIGINT/SIGTERM → 30s timeout → flush+snapshot)

### Prometheus metrics: DONE (port 12798, /metrics, /health, /ready endpoints)
- Note: metric names are `torsten_*` prefix (not `cardano_node_*`). Dashboards need updating.

### CLI: ~90% compatible with cardano-cli
- 33+ subcommands implemented
- Multi-asset coin selection: DONE (calculate_change handles token change)
- Missing: some advanced query flags; not all cardano-cli 10.x options exposed

## Known Gaps (as of 2026-03-18)

### HIGH Priority
1. **Reward cross-validation against on-chain data**: Unit tests cover formula primitives and Koios epoch 1235. No E2E validation of per-epoch reward amounts distributed to delegators.
2. **Testnet re-validation post-RUPD**: Need full sync from Mithril snapshot after 2026-03-15 RUPD change to confirm 0 validation errors.
3. **Phase-1 false positives under investigation**: Soak test log may contain InvalidMint warnings on confirmed blocks (treated as non-fatal, sync continues). Root cause may be race condition in UTxO overlay during high-volume blocks.

### MEDIUM Priority
4. **Prometheus metric naming**: Uses `torsten_*` prefix instead of `cardano_node_*`. Not compatible with existing Grafana dashboards for cardano-node.
5. **torsten-lsm at mainnet scale**: Validated at ~3M UTxOs (preview). Not tested at 20M+ (mainnet). Need compaction + WAL crash recovery benchmarks.
6. **CC hot key type tracking**: Defaults to KeyHash (0) for all hot keys. Script-based hot keys not tracked — acceptable for current governance state.
7. **Genesis BLP peer selection**: Standard P2P governor used in all GSM states. Proper BLP-prioritized peer selection not implemented.
8. **state/tests.rs monolith**: 7,842 lines, 200 tests, all in one file. Makes discovery/debugging harder.

### LOW Priority
9. **CDDL automated conformance testing**: No automated test against official Cardano CDDL spec files.
10. **Cost model cross-validation**: V1/V2/V3 cost models stored and passed to uplc but not validated against cardano-node's eval results for complex edge-case scripts.

## Test Coverage

| Area | Unit Tests | Property Tests | Conformance | Golden | E2E |
|---|---|---|---|---|---|
| Byron ledger | 9 | - | - | - | - |
| Shelley+ validation | 89 | Yes (proptest) | 31 vectors | - | - |
| Rewards/epoch | 20+ | - | 6 vectors | - | - |
| Governance | 45+ | - | 13 vectors | - | - |
| N2C encoding | - | - | - | 77+ tests/25 fixtures | - |
| VRF/KES | - | - | - | 8 files | - |
| ChainDB/storage | 36 | - | - | 5 blueprint | - |
| Block production | 5 forge tests | - | - | - | preview soak |

## Dependency Status (2026-03-18)

| Dependency | Version | Status |
|---|---|---|
| pallas-* (6 crates) | 1.0.0-alpha.5 | Pre-release alpha. No stable 1.0.0 yet. |
| uplc | git rev 6806d52 | Git pinned from aiken-lang/aiken. No crates.io release. |
| vrf_dalek | git rev 03ac038 | Git pinned from IOHK. No tagged release. |
| dashu-int/base | 0.4.1 | Stable. Latest is 0.4.1 (no newer). |
| tokio | 1.x | Stable, up to date. |
| ratatui | 0.29 | Latest stable as of 2026-03-18. |

## Mainnet Readiness Gap

Key items blocking mainnet production use:
1. Reward E2E validation at mainnet scale (not just preview)
2. torsten-lsm at 20M+ UTxO scale
3. Extended soak test (48+ hours) as both relay and BP without manual intervention
4. Prometheus metric naming alignment (or documented divergence)
5. Genesis BLP peer selection (security concern for bootstrap attacks)
6. Official security audit of VRF/KES code paths
</content>
</invoke>