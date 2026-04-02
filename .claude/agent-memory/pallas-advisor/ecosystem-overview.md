---
name: pallas-ecosystem-overview
description: All pallas crates, version status, release history, workspace layout, and what's published vs experimental
type: reference
---

# Pallas Ecosystem Overview

Last researched: 2026-03-13 against v1.0.0-alpha.5

## Version Status

- **Current dugite version**: v1.0.0-alpha.5 (Feb 28, 2026)
- **Previous alpha**: v1.0.0-alpha.4 (Feb 4, 2026)
- **Latest stable**: v0.35.0 (Feb 10, 2026)
- **Prior stable**: v0.34.0 (Dec 16, 2025), v0.33.0 (Jul 13, 2025)
- **Older stable**: v0.32.1 (Jun 25, 2025), v0.18.5 (Jun 23, 2025)

The 1.x alpha series runs alongside the 0.x stable series. Both are actively released. The alpha series is targeting a 1.0 stable release but has been in alpha since April 2025.

**There is NO v1.0.0-alpha.6 or v1.0.0 stable yet** (as of 2026-03-13).

## Workspace Members (14 crates + 8 example projects)

All published on crates.io at v1.0.0-alpha.5:

| Crate | Description | Dugite Uses? |
|-------|-------------|---------------|
| pallas-codec | CBOR encode/decode via minicbor | YES |
| pallas-crypto | Ed25519, KES, VRF, Blake2b | YES (kes feature) |
| pallas-math | Fixed-point arithmetic, VRF math | NO (ported algorithms to dugite-crypto) |
| pallas-network | Ouroboros mini-protocols + multiplexer | YES |
| pallas-primitives | Block/tx types for Byron-Conway | YES |
| pallas-traverse | Era-agnostic MultiEraBlock/Tx traversal | YES |
| pallas-addresses | Address parsing/construction | YES |
| pallas-configs | Genesis file parsing | NO |
| pallas-validate | Phase-1/Phase-2 tx validation | NO |
| pallas-txbuilder | Transaction builder (Conway) | NO |
| pallas-hardano | Haskell node ImmutableDB interop | NO |
| pallas-utxorpc | UTxO RPC integration | NO |
| pallas (root) | Re-exports; umbrella crate | NO |
| pallas-network2 | Experimental rewrite (NOT published) | NO |

## Key Changes in alpha.4 → alpha.5 (Feb 2026)

- Fixed validation handling of non-Conway UTXOs within Conway transactions (#729)
- Corrected calculation of encoded array length in pallas-codec (#654)
- Introduced "responder behavior" in pallas-network (#732)
- 52 files changed total across 13 packages

## Key Changes in alpha.3 → alpha.4

- Bumped pallas-uplc dependency (phase2 feature in pallas-validate)

## alpha.1 Background (Apr 2025)

The 1.x series introduced breaking API changes from 0.x:
- `PseudoDatumOption` renamed to `DatumOption`
- `Nullable<T>` replaced by `Option<T>` in many contexts
- Hash types changed in some contexts (28-byte vs 32-byte)

## Cargo.lock Notes

Dugite's Cargo.lock shows BOTH 0.33.0 and 1.0.0-alpha.5 versions of several crates:
- pallas-addresses: 0.33.0 AND 1.0.0-alpha.5
- pallas-codec: 0.33.0 AND 1.0.0-alpha.5
- pallas-crypto: 0.33.0 AND 1.0.0-alpha.5
- pallas-primitives: 0.33.0 AND 1.0.0-alpha.5
- pallas-traverse: 0.33.0 AND 1.0.0-alpha.5
- pallas-network: 1.0.0-alpha.5 only

The 0.33.x versions are likely pulled in transitively by cardano-lsm or another dependency.

## Unpublished / Experimental

- **pallas-network2**: Listed in workspace but NOT published to crates.io. Appears to be an experimental rewrite of pallas-network. Not relevant for dugite adoption until stable.
- **pallas-bech32**: Appears in the repository; not confirmed published. Provides Bech32 encoding separate from pallas-addresses.

## Documentation & Stability

- All 1.0.0-alpha.x crates are labeled alpha — API breakage should be expected between alphas
- Test coverage exists for pallas-validate across all eras (real mainnet tx examples)
- pallas-math authored by Andrew Westberg (IOHK/Cardano Pool Tool background)
- The 0.x series has a separate changelog; 1.x alphas do NOT have a dedicated CHANGELOG section
