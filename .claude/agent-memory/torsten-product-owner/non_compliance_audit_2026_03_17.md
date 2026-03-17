---
name: non_compliance_audit_2026_03_17
description: Comprehensive audit of all known Cardano standards non-compliance in Torsten as of 2026-03-17
type: project
---

# Torsten Non-Compliance Audit — 2026-03-17

**Why:** Full cross-source audit covering GitHub issues, agent memories, code review, and spec references.
**How to apply:** Use as the authoritative list of compliance debt when planning work. Update as items are fixed.

## CRITICAL (consensus divergence / data integrity risk)

### C-1: Block body hash computation (block production)
- **Spec:** Haskell `hashAlonzoSegWits` = Blake2b-256(hash(bodies)||hash(wits)||hash(auxdata)||hash(isvalid))
- **Torsten:** Computes Blake2b-256(CBOR_array(tx_bodies)) — single hash, wrong components
- **Crate:** torsten-node (forge.rs), torsten-serialization
- **Source:** agent-memory/cardano-haskell-oracle/block-forging-flow.md

### C-2: VRF input construction (epoch nonce stabilisation window era-dependency)
- **Spec:** Babbage uses 3k/f window; Conway uses 4k/f window (NOT 4k/f for all eras)
- **Torsten:** Uses 4k/f unconditionally — wrong for Babbage and all TPraos eras (Shelley through Alonzo)
- **Crate:** torsten-consensus (praos.rs)
- **Source:** agent-memory/cardano-haskell-oracle/epoch-nonce-calculation.md

### C-3: Pool distribution for leader check — not memoized
- **Spec:** `nesPd` = mark snapshot pool distribution, set ONCE at epoch boundary
- **Torsten:** Computes pool distribution from `snapshots.set` on-the-fly per batch
- **Impact:** Leader schedule diverges from Haskell mid-epoch when stake changes
- **Crate:** torsten-consensus / torsten-ledger
- **Source:** agent-memory/cardano-haskell-oracle/pool-distr-leader-check.md

### C-4: Relative stake uses f64 instead of exact Rational
- **Spec:** Haskell passes sigma as exact Rational (ratio of two Integer); active_slot_coeff = exact Rational from genesis (e.g., 1/20)
- **Torsten:** Uses f64 — cannot exactly represent 0.05 (binary rounding error at boundary cases)
- **Crate:** torsten-consensus (praos.rs PoolInfo.relative_stake: f64)
- **Source:** agent-memory/cardano-haskell-oracle/pool-distr-leader-check.md, nonintegral-ln-algorithm.md

## HIGH (protocol non-compliance / cardano-cli breakage)

### H-1: GetGovState (tag 24) — GetRatifyState (tag 32) standalone encoding wrong
- **Spec:** RatifyState = array(4) [EnactState(array(7)), enacted_seq, expired_set, delayed_bool]; first field IS the full EnactState
- **Torsten:** Encodes [enacted_seq, expired_seq, delayed, future_pparams] — completely wrong structure; missing EnactState, has spurious future_pparams field
- **Issue:** #117
- **Crate:** torsten-network (n2c/query/encoding.rs:1314, encode_ratify_state)
- **Source:** agent-memory/cardano-haskell-oracle/conway-gov-state-encoding-detailed.md

### H-2: GetGovState (tag 24) — DRepPulsingState PulsingSnapshot field order/types wrong
- **Spec:** PulsingSnapshot = array(4) [psProposals(StrictSeq), psDRepDistr(Map), psDRepState(Map), psPoolDistr(Map)]
- **Torsten (in GetGovState):** Fixed with correct field order as of ~2026-03-17 (encoding.rs:978-981 shows correct structure)
- **Note:** The standalone GetRatifyState path (H-1) still has the bug
- **Issue:** #117

### H-3: CommitteeMembersState (tag 27) — built from wrong map
- **Spec:** Committee members = all entries in `committee_expiration` map (includes MemberNotAuthorized)
- **Torsten:** Iterates only `committee_hot_keys` — members without hot key authorization are invisible
- **Crate:** torsten-node (node/query.rs:277-313)
- **Source:** agent-memory/network-lead/committee-state-encoding-bugs.md

### H-4: CommitteeMembersState — hot credential type hardcoded to KeyHashObj (0)
- **Spec:** Hot credential type must reflect actual type (0=KeyHash, 1=Script)
- **Torsten:** `encoding.rs:1055` always encodes `enc.u8(0)` regardless of actual type
- **Crate:** torsten-network (n2c/query/encoding.rs), torsten-ledger (state/mod.rs:204)
- **Source:** agent-memory/network-lead/committee-state-encoding-bugs.md

### H-5: GetLedgerPeerSnapshot (tag 34) — multiple encoding bugs
- **Spec:** Rational stake fractions (no tag 30), indefinite arrays for pool lists, WithOrigin wrapper for slot, network_magic in V2/V3 encoding
- **Torsten:** Uses u64 for stakes instead of Rational; definite arrays; missing WithOrigin; missing network_magic; doesn't filter big ledger peers (top 90% by stake); doesn't compute accumulated stake
- **Crate:** torsten-network (n2c/query/encoding.rs, encode_ledger_peer_snapshot)
- **Source:** agent-memory/cardano-haskell-oracle/ledger-peer-snapshot-encoding.md

### H-6: N2C version support — max version is V22, V23 not supported
- **Spec:** cardano-node 10.x offers V16-V23; V23 adds GetDRepDelegations (tag 39)
- **Torsten:** Offers V16-V22 (tag 39 GetDRepDelegations not implemented)
- **Crate:** torsten-network
- **Source:** agent-memory/cardano-haskell-oracle/n2c-protocol-details.md

### H-7: RUPD timing — rewards distributed 1 epoch early
- **Spec:** Haskell RUPD: startStep at epoch E, pulse during E+1, apply at E+2 boundary
- **Torsten:** Applies rewards immediately at epoch boundary — 1 epoch early
- **Impact:** Cumulative treasury/reserve divergence on long-running chains; staker balances 1 epoch out of sync
- **Crate:** torsten-ledger (state/epoch.rs)
- **Source:** agent-memory/ledger-lead/reward-formula-validation.md

### H-8: Plutus Phase-2 validation — not executed (scripts always pass)
- **Spec:** Transactions with Plutus scripts require CEK machine execution for script validation
- **Torsten:** Plutus evaluation via `uplc` crate is present but cost model validation against cardano-node not confirmed; complex scripts may diverge
- **Crate:** torsten-ledger (plutus.rs)
- **Source:** capability_gaps.md

### H-9: Byron era — no ledger validation (UTxO rules not enforced)
- **Note:** REVISED — Byron ledger validation IS implemented (byron.rs has full fee/conservation/input rules). The prior gap (capability_gaps.md) is outdated.
- **Status:** RESOLVED — ByronApplyMode::ValidateAll enforces all 5 Byron UTxO rules
- **Remaining gap:** Byron consensus validation (OBFT signatures) not verified — only UTxO rules are checked

### H-10: Nonce contribution window formula (ln' algorithm)
- **Spec:** Haskell uses Euler continued fraction for ln(1+x) via `lncf`; activeSlotLog precomputed once at epoch init
- **Torsten:** Uses Taylor series for ln — different fixed-point truncation causes boundary disagreements with Haskell
- **Crate:** torsten-crypto (vrf.rs) — the vrf.rs code uses continued fraction for lncf, but activeSlotLog uses f64 path
- **Source:** agent-memory/cardano-haskell-oracle/nonintegral-ln-algorithm.md

## MEDIUM (encoding correctness / partial functionality)

### M-1: CompactGenesis (GetGenesisConfig tag 11) — version-gated encoding not implemented
- **Spec:** V16-V20: legacy PParams array(18) with ProtVer as two flat ints; V21+: new PParams array(17) with ProtVer as array(2)
- **Torsten:** Single encoding path; unclear if it correctly handles version gating
- **Crate:** torsten-network
- **Source:** agent-memory/cardano-haskell-oracle/shelley-genesis-cbor.md, n2c-version-v17-v22-changes.md

### M-2: GetStakeDistribution / GetPoolDistr deprecated at V21 — not gated
- **Spec:** GetStakeDistribution (tag 5) and GetPoolDistr (tag 21) must be REJECTED for V21+ clients
- **Torsten:** Status unknown — likely still served to all clients
- **Crate:** torsten-network
- **Source:** agent-memory/cardano-haskell-oracle/n2c-version-v17-v22-changes.md

### M-3: GetProposedPParamsUpdates (tag 4) deprecated at V20
- **Spec:** Must be rejected for V20+ clients (version gate: < v12)
- **Torsten:** Likely not gated by negotiated version
- **Crate:** torsten-network

### M-4: CIP-0129 governance bech32 prefixes not implemented
- **Spec:** CIP-0129 defines bech32 prefixes: `drep`, `drep_script`, `cc_hot`, `cc_hot_script`, `cc_cold`, `cc_cold_script`, `gov_action`
- **Torsten:** Only `drep` is used; other prefixes missing from CLI
- **Issue:** #114
- **Crate:** torsten-cli (governance.rs)

### M-5: CLI output format diverges from cardano-cli (JSON field names/structure)
- **Spec:** cardano-cli has a well-defined JSON output schema per command
- **Torsten:** May have differences in field names, formatting, or structure
- **Issue:** #118
- **Crate:** torsten-cli

### M-6: CLI does not infer network name from magic number
- **Spec:** cardano-cli infers "mainnet"/"preview"/"preprod" from magic; Torsten requires explicit `--network`
- **Issue:** #120
- **Crate:** torsten-cli

### M-7: Transaction build (auto-balance) — ADA-only change calculation
- **Spec:** Change output must correctly return multi-asset tokens to the change address
- **Torsten:** `calculate_change` handles multi-asset structurally but auto-balance coin selection is ADA-centric
- **Crate:** torsten-cli (transaction.rs)
- **Source:** project_state_2026_03_16.md (item 5)

### M-8: DebugEpochState (tag 8), DebugNewEpochState (tag 12), DebugChainDepState (tag 13) — simplified summaries
- **Spec:** These tags return full serialized state (used by cncli for pool analysis)
- **Torsten:** Returns simplified state summaries, not full serialized CBOR
- **Crate:** torsten-network
- **Source:** MEMORY.md system-reminder

### M-9: Ouroboros Genesis LoE — block application not gated
- **Spec:** In Genesis mode, blocks beyond LoE slot should not be applied to ledger
- **Torsten:** LoE gates volatile-to-immutable flush (correct) but blocks are still applied to ledger regardless
- **Crate:** torsten-node (gsm.rs)
- **Source:** capability_gaps.md, consensus-lead/loe-enforcement.md

### M-10: PeerSharing — outbound-only privacy, no full P2P governor
- **Spec:** Full Ouroboros P2P governor with LedgerStateJudgement, big ledger peer promotion, genesis peer targets, churn
- **Torsten:** Simplified PeerManager with EWMA latency and reputation; no genesis-mode peer targets (40 BLP established, 30 active); no churn governor
- **Crate:** torsten-network
- **Source:** agent-memory/cardano-haskell-oracle/p2p-governor-architecture.md

### M-11: No CDDL conformance test suite
- **Spec:** Official Cardano CDDL specs for all 10 mini-protocols
- **Torsten:** Wire format verified through integration tests only; no automated CDDL validation
- **Crate:** torsten-serialization (cddl_conformance.rs tests are structural, not spec-verified)
- **Source:** capability_gaps.md

### M-12: Reward cross-validation against on-chain history not done
- **Spec:** Per-delegator reward amounts must exactly match cardano-node computation
- **Torsten:** Formula confirmed correct vs Koios epoch 1235 synthetic validation; zero tests against actual per-delegator on-chain amounts for real historical epochs
- **Crate:** torsten-ledger
- **Source:** capability_gaps.md

## LOW (cosmetic / partial support / future work)

### L-1: N2N V16 Genesis version present but CSJ / GDD not implemented
- **Spec:** Ouroboros Genesis requires ChainSync Jumping (CSJ) and Genesis Density Disconnector (GDD)
- **Torsten:** V16 in handshake but CSJ/GDD disabled; no bootstrap peer target switching
- **Issue:** #101
- **Crate:** torsten-network, torsten-node

### L-2: TxSubmission2 end-to-end tx propagation not verified
- **Spec:** Submitted transactions must propagate to the network via TxSubmission2
- **Torsten:** MsgReplyTxs path exists but E2E flow unverified
- **Issue:** #102
- **Crate:** torsten-network

### L-3: Mainnet 48-hour stability not verified
- **Issue:** #103

### L-4: MsgRejectTx (LocalTxSubmission) CBOR encoding not verified against spec
- **Spec:** [2, [1, [6, NonEmpty ConwayLedgerPredFailure]]] per HFC wrapping rules
- **Torsten:** Encoding present but not golden-tested against Haskell output for all predicate failure types
- **Crate:** torsten-network
- **Source:** agent-memory/cardano-haskell-oracle/msgrejecttx-wire-format.md

### L-5: Treasury value edge cases not validated on live chain
- **Spec:** Undistributed rewards accumulate in treasury; tau calculation must be exact
- **Issue:** #116

### L-6: Ouroboros Peras (CIP-0140) — not researched/planned
- **Issue:** #111

### L-7: Guild network pool registration not done
- **Issue:** #110
