# CLAUDE.md — Development Instructions for Torsten

## Project Goal
Implement a 100% compatible Cardano node in Rust. Target full compatibility with cardano-node (Haskell).

## Development Methodology: Ralph Loop
Follow the Ralph autonomous development loop:
1. **Assess** — Evaluate current state, identify highest-impact gaps
2. **Implement** — Build the next feature/fix
3. **Test** — Run `cargo test --all`, ensure zero failures
4. **Verify** — Run `cargo clippy --all-targets -- -D warnings` and `cargo fmt --all -- --check`
5. **Commit** — Commit and push to remote with descriptive message
6. **Repeat** — Continue to the next iteration

## Hard Requirements
- **Zero warnings** — All code must compile with `RUSTFLAGS="-D warnings"`
- **Clippy clean** — `cargo clippy --all-targets -- -D warnings` must pass
- **Formatted** — `cargo fmt --all -- --check` must pass
- **Tests pass** — All tests must pass before committing
- **CI green** — GitHub Actions pipeline must be passing
- **Commit regularly** — Push changes to remote after each successful iteration

## Priority Roadmap (in order)
1. ~~Core types and primitives~~ ✅
2. ~~CBOR serialization via pallas~~ ✅
3. ~~Network client (N2N chain sync)~~ ✅
4. ~~Ledger: UTxO, validation, certificates, native scripts~~ ✅
5. ~~Upgrade pallas to 1.x~~ ✅ — Pallas 1.0.0-alpha.5 for N2N V14+
6. ~~Storage: rollback support~~ ✅ — ChainDB rollback, volatile→immutable flush
7. ~~Consensus: structural validation~~ ✅ — KES period, VRF output, opcert checks (crypto VRF/KES pending)
8. ~~Epoch transitions~~ ✅ — Stake snapshots, reward calculation/distribution, fee tracking
9. ~~Node-to-Client protocol~~ ✅ — Unix socket server, local state query handler, N2C handshake
10. ~~Plutus script execution~~ ✅ — uplc CEK machine for Plutus V1/V2/V3, Phase-2 validation, LocalTxSubmission validation
11. ~~Conway governance~~ ✅ — DRep reg/vote/delegation, committee, proposals, ratification, treasury withdrawals
12. ~~Relay node compliance~~ ✅ — Pipelined ChainSync (~40x throughput), ledger-based peer discovery, adaptive peer selection, N2N server
13. ~~CLI parity~~ ✅ — 33+ subcommands: address, transaction, query, key, stake, pool, node, governance
14. ~~Performance~~ ✅ — HashMap UTxO/ledger lookups, batched volatile writes, O(n) reward distribution, zero-copy block storage
15. **Integration testing** — Run against testnet/mainnet, verify block sync to tip

## Architecture
See README.md for the 10-crate workspace structure.

## Key Patterns
- Use pallas crates for Cardano wire-format compatibility
- `Transaction.hash` field is set during deserialization from `pallas tx.hash()`
- `ChainSyncEvent::RollForward` uses `Box<Block>` to avoid large enum variant size
- Invalid transactions (`is_valid: false`): collateral consumed, collateral_return added, regular inputs/outputs skipped
- N2N server uses `BlockProvider` trait for storage abstraction
- N2C server uses `TxValidator` trait for Phase-1/Phase-2 tx validation before mempool admission
- Batch block storage: `add_blocks_batch()` for single immutable flush per batch
- Ledger-based peer discovery: extracts SPO relay addresses from `pool_params` when past `useLedgerAfterSlot`
- `PoolRegistration` stores relay info (SingleHostAddr, SingleHostName, MultiHostName)
- Epoch transitions use mark/set/go snapshot model with reward distribution from "go" snapshot
- Governance ratification: DRep/SPO/CC voting thresholds vary by action type (CIP-1694)
