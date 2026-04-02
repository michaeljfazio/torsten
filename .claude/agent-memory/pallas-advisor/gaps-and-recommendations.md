---
name: pallas-gaps-and-recommendations
description: ADOPT/ADAPT/IMPLEMENT decisions for each pallas crate, with rationale and migration paths
type: reference
---

# Pallas Gaps and Adoption Recommendations

Last reviewed: 2026-03-13, pallas v1.0.0-alpha.5

## Summary Table

| Crate | Decision | Priority | Status |
|-------|----------|----------|--------|
| pallas-codec | ADOPT (already using) | — | Done |
| pallas-crypto | ADOPT (already using, kes feature) | — | Done |
| pallas-primitives | ADOPT (already using) | — | Done |
| pallas-traverse | ADOPT (already using) | — | Done |
| pallas-addresses | ADOPT (already using) | — | Done |
| pallas-network | ADAPT (already using + own extensions) | — | Done |
| pallas-validate | ADOPT Phase-1 as reference; ADOPT Phase-2 when uplc matures | HIGH | Not started |
| pallas-configs | ADOPT | MEDIUM | Not started |
| pallas-math | IMPLEMENT FROM SCRATCH | — | Done (ported to dugite-crypto) |
| pallas-hardano | IMPLEMENT FROM SCRATCH | — | Done (Mithril import superior) |
| pallas-txbuilder | EVALUATE for dugite-cli | LOW | Not started |
| pallas-utxorpc | IGNORE | N/A | — |

---

## HIGH PRIORITY: pallas-validate

### Decision: ADAPT

### What it provides
Comprehensive Phase-1 (structural + signature) validation across all eras:
- Byron: inputs/outputs/fees/witnesses
- Shelley/MA: TTL, value preservation, native scripts, certificates
- Alonzo: Plutus structure, collateral, redeemers, datums
- Babbage: Reference inputs/scripts inline scripts
- Conway: Full Conway structural checks

Phase-2 (Plutus script execution) via optional `pallas-uplc` dependency (feature `phase2`).

### What dugite would gain
1. **Phase-2 Plutus execution**: Dugite currently has NO Plutus script execution. pallas-validate's `phase2` feature via pallas-uplc would be the fastest path to Plutus support.
2. **Validation parity**: Ensures dugite's validation matches pallas reference implementation.
3. **Real-transaction test corpus**: pallas-validate has real mainnet tx test cases dugite can use.

### Gaps in pallas-validate vs dugite
1. **CIP-0112 reference script fee**: Not in pallas-validate. Dugite has this. Must keep dugite's implementation.
2. **Certificate ordering rules**: pallas-validate checks structure but not cert state (who's registered). Dugite's ledger state maintains this.
3. **Withdrawal balance checks**: pallas-validate doesn't check against actual reward account balances.
4. **Conway governance cert validation**: Incomplete (DRep cert structural checks not fully visible).

### Migration Path
1. Add `pallas-validate = "1.0.0-alpha.5"` to dugite-ledger's Cargo.toml
2. Create wrapper that maps dugite's `Environment` equivalent to pallas's `Environment`
3. Use pallas-validate for Phase-1 checks on new transactions entering the mempool
4. Keep dugite's additional checks (CIP-0112, cert state, withdrawal balance) as post-pallas checks
5. When pallas-uplc matures: enable `phase2` feature for Plutus execution

### Risks
- Alpha API: validation environment struct may change between alpha versions
- Conway completeness: governance cert validation may be incomplete
- pallas-uplc maturity: unknown — Plutus evaluator quality not assessed

---

## MEDIUM PRIORITY: pallas-configs

### Decision: ADOPT

### What it provides
Structured JSON deserialization for Byron, Shelley, Alonzo, and Conway genesis files. Matches Haskell implementation field names exactly.

### What dugite would gain
1. Replace ad-hoc genesis parsing in `dugite-node/src/genesis.rs`
2. Structured protocol parameter types that align with pallas-validate's `Environment`
3. `shelley_utxos()` helper function for genesis UTxO extraction
4. Correct Conway governance parameter parsing (DRep thresholds, committee, constitution)

### Migration Path
1. Add `pallas-configs = { version = "1.0.0-alpha.5", features = ["json"] }` to dugite-node
2. Replace `dugite-node/src/genesis.rs` parsing with pallas-configs structs
3. Map pallas-configs types to dugite-primitives protocol parameter types
4. Keep dugite's own types internally but use pallas-configs for file parsing

### Risks
- Genesis file field coverage: verify pallas-configs parses all fields dugite needs
- Alpha API: field names could change between alpha versions (low risk — JSON field names are stable by Cardano spec)
- Dependency weight: pallas-configs adds serde_with + num-rational; acceptable

---

## DONE (for reference): pallas-math

### Decision: IMPLEMENT FROM SCRATCH

### Rationale
- Algorithms ported directly to dugite-crypto using dashu-int
- VRF leader check requires: Euler continued fraction for ln(1+x), Taylor series for taylorExpCmp
- Dugite's implementation is verified correct (tested against actual leader schedule on testnet)
- Adding pallas-math dependency brings regex + additional dashu crates with no net benefit
- The mathematical algorithms are stable and well-understood

---

## DONE (for reference): pallas-hardano

### Decision: IMPLEMENT FROM SCRATCH

### Rationale
- Dugite's Mithril import is more comprehensive: download, digest verification, CRC32, bulk import
- pallas-hardano only provides chunk file iteration — a small subset
- Dugite uses memory-mapped I/O (memmap2) for performance; pallas-hardano uses standard I/O
- The 4M-block bulk import path is critical performance code that dugite has tuned

---

## LOW PRIORITY: pallas-txbuilder

### Decision: EVALUATE when implementing dugite-cli transaction commands

### What it provides
Conway-era transaction builder with fluent API. Handles inputs, outputs, scripts, redeemers, datums, minting, certificates.

### When to adopt
When `dugite-cli transaction build` command needs implementation. Use as starting point rather than building from scratch.

### Risks
- Conway-only: historical era transactions not supported
- Alpha API: likely less tested than other pallas crates
- Must add comprehensive integration tests against cardano-node

---

## IGNORE: pallas-utxorpc

### Rationale
UTxO RPC is a separate ecosystem (gRPC-based Cardano API). Not relevant to dugite's Ouroboros-native node implementation. Would only matter if dugite adds a UTxO RPC endpoint (not on roadmap).

---

## Dugite Capabilities That SURPASS Pallas

These are areas where dugite is more complete or advanced than the corresponding pallas functionality:

### 1. N2C Protocol (V17-V22)
Dugite implements LocalStateQuery tags 0-38, V17-V22 with bit-15 version encoding. Pallas only defines through V16. Dugite is ahead of pallas here.

### 2. Pipelined ChainSync
Dugite's `PipelinedPeerClient` achieves 10-50x header throughput vs pallas's serial ChainSync. Pallas has no pipelining support.

### 3. VRF Leader Check Math
Dugite's Euler continued fraction + taylorExpCmp implementation, using dashu-int directly, is tested against real testnet leader schedules. Pallas-math implements the same algorithms but dugite doesn't need to depend on it.

### 4. Mithril Snapshot Import
Dugite's full snapshot lifecycle (download, verify, CRC32, bulk import, deferred compaction) vastly exceeds pallas-hardano's read-only chunk iteration.

### 5. CIP-0112 Reference Script Fee
Dugite implements the 25KiB tier reference script fee calculation. pallas-validate does not.

### 6. Governance (CIP-1694)
Dugite has complete DRep/SPO/CC voting with exact rational arithmetic. pallas-validate's Conway checks are structural only.

---

## Version Upgrade Path

### When pallas v1.0.0-alpha.6 releases:
1. Check CHANGELOG for breaking changes in: pallas-codec utils (Nullable→Option patterns), pallas-network handshake version tables, pallas-primitives era types
2. The most breaking-change-prone areas are: pallas-network (version table additions), pallas-primitives (new Conway fields)
3. Test pipelined chainsync first — this is the most brittle pallas integration point

### When pallas v1.0.0 stable releases:
1. This will be the time to evaluate adopting pallas-validate and pallas-configs
2. Stable release implies API stability — much lower adoption risk
3. Check if pallas-uplc (phase2) is also stable at that point
