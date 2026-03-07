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
10. **Plutus script execution** — CEK machine for Plutus V1/V2/V3
11. **Conway governance** — DRep, voting, proposals, constitutional committee
12. **CLI parity** — Full cardano-cli compatible command set
13. **Performance** — Optimize sync speed, memory usage, database I/O
14. **Integration testing** — Run against testnet, verify block sync to tip

## Architecture
See README.md for the 10-crate workspace structure.

## Key Patterns
- Use pallas crates for Cardano wire-format compatibility
- `Transaction.hash` field is set during deserialization from `pallas tx.hash()`
- `ChainSyncEvent::RollForward` uses `Box<Block>` to avoid large enum variant size
- Invalid transactions (`is_valid: false`) are skipped during `apply_block`
