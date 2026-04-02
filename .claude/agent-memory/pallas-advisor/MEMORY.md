# Pallas Advisor Memory Index

Last comprehensive research: 2026-03-13 against pallas v1.0.0-alpha.5

## Ecosystem Overview
- [ecosystem-overview.md](ecosystem-overview.md) — All 14 crates, version history (v1.0.0-alpha.5 = Feb 28 2026, latest stable = v0.35.0), dual 0.33.x/1.x versions in Cargo.lock, alpha.4→alpha.5 changes

## Crate Details
- [crate-pallas-validate.md](crate-pallas-validate.md) — Phase-1 validation per era (Byron→Conway), Environment struct, ValidationError hierarchy, test corpus, gaps (no CIP-0112, no cert state, Conway gov incomplete)
- [crate-pallas-configs.md](crate-pallas-configs.md) — Genesis file parsing: Byron (ProtocolConsts, TxFeePolicy), Shelley (ProtocolParams, shelley_utxos()), Conway (DRep/pool thresholds, committee, constitution)
- [crate-pallas-crypto.md](crate-pallas-crypto.md) — Hash<N>/Hasher, Sum6Kes (612 bytes, 63 evolutions), KesSk/KesSig traits, zeroize-on-drop caveat, 28-byte padding problem
- [crate-pallas-network.md](crate-pallas-network.md) — Multiplexer (Bearer/Plexer/ChannelBuffer), all mini-protocols, N2N V7-V14, N2C V1-V16, NO pipelining, dugite divergences documented
- [crate-pallas-traverse.md](crate-pallas-traverse.md) — MultiEraBlock/Tx/Header/Output/Input/Cert API, Era/Feature enums, OutputRef, ComputeHash/OriginalHash traits
- [crate-pallas-math.md](crate-pallas-math.md) — FixedDecimal E34, FixedPrecision trait, exp()/ln()/exp_cmp(), Euler continued fraction, dashu-int backend; dugite ported algorithms directly
- [crate-pallas-hardano.md](crate-pallas-hardano.md) — ImmutableDB chunk file reading, read_blocks/read_blocks_from_point/get_tip; dugite's Mithril import is more complete
- [crate-pallas-txbuilder.md](crate-pallas-txbuilder.md) — Conway-era builder pattern (BuildConway, StagingTransaction, BuiltTransaction); relevant for dugite-cli transaction commands
- [crate-pallas-misc.md](crate-pallas-misc.md) — pallas-addresses (Address/ShelleyAddress/StakeAddress), pallas-primitives (era types, Hash<N>, Coin, ExUnits), pallas-codec (Fragment trait, MaybeIndefArray, KeyValuePairs, Nullable, CborWrap, Set<T>), pallas-utxorpc (ignore)

## Integration & Gaps
- [dugite-integration.md](dugite-integration.md) — Exact workspace deps, per-crate pallas usage, 6 known workarounds (pipelining, 28-byte padding, KES lifecycle, Byron epoch length, DatumOption rename, Nullable→Option)
- [gaps-and-recommendations.md](gaps-and-recommendations.md) — ADOPT pallas-validate (HIGH) + pallas-configs (MEDIUM); pallas-math/hardano already ported; pallas-txbuilder for CLI (LOW); pallas-utxorpc ignore; dugite superiority areas listed
