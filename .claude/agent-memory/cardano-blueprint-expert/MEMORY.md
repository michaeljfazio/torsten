# Agent Memory Index — Cardano Blueprint Expert

## Blueprint Documentation Research (2026-03-13)

Comprehensive deep-dive into https://github.com/cardano-scaling/cardano-blueprint and https://cardano-scaling.github.io/cardano-blueprint.

| File | Type | Description |
|------|------|-------------|
| [blueprint-overview.md](blueprint-overview.md) | reference | Project structure, repo layout, all source file paths, project mission, current state |
| [blueprint-consensus.md](blueprint-consensus.md) | reference | Consensus layer — chain validity (BFT/PBFT), chain selection (k=2160, 3k/f), tie-breakers, Genesis, forging, era/protocol table |
| [blueprint-ledger.md](blueprint-ledger.md) | reference | Ledger rules — block structure, state transition, multi-phase validity, static/dynamic checks, fee calculation, Conway EraRule BBODY/LEDGERS/CERTS/GOV/UTXOW flowcharts |
| [blueprint-network.md](blueprint-network.md) | reference | All mini-protocols — mux packet format, Handshake CDDL (v13/v14), ChainSync/BlockFetch/TxSubmission2/KeepAlive state machines and CDDL |
| [blueprint-serialization.md](blueprint-serialization.md) | reference | Wire format — CBOR tag 24, era dispatch (ns7/telescope7), base.cddl types, LSQ encoding, UTCTime encoding |
| [blueprint-governance.md](blueprint-governance.md) | reference | Governance — what IS documented (EraRule GOV/GOVERT), what is NOT (thresholds, ratification), authoritative sources |
| [blueprint-test-vectors.md](blueprint-test-vectors.md) | reference | Test data — Conway ledger conformance vectors (vectors.tar.gz), handshake test data, LSQ examples, fee worked example |
| [blueprint-plutus.md](blueprint-plutus.md) | reference | Plutus/UPLC — syntax, de Bruijn, CEK machine full transition table, cost model overview |
| [blueprint-storage.md](blueprint-storage.md) | reference | Storage — ChainDB directory structure (immutable/volatile/ledger), index formats, Mithril integration |
| [blueprint-gaps.md](blueprint-gaps.md) | reference | All known gaps/stubs/TODOs with fallback authoritative sources for each topic |
| [formal-spec-nonce.md](formal-spec-nonce.md) | reference | TICKN/UPDN/PRTCL STS rules, nonce derivation, stability windows (3k/f vs 4k/f), Conway erratum, prevHashToNonce, vrfNonceValue, initialChainDepState |
