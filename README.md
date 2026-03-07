# Torsten

A Cardano node implementation written in Rust, aiming for 100% compatibility with [cardano-node](https://github.com/IntersectMBO/cardano-node).

[![CI](https://github.com/michaeljfazio/torsten/actions/workflows/ci.yml/badge.svg)](https://github.com/michaeljfazio/torsten/actions/workflows/ci.yml)

## Architecture

Torsten is organized as a 10-crate Cargo workspace:

| Crate | Description |
|-------|-------------|
| `torsten-primitives` | Core types: hashes, blocks, transactions, addresses, values, protocol parameters (Byron–Conway) |
| `torsten-crypto` | Ed25519 keys, VRF, KES, text envelope format |
| `torsten-serialization` | CBOR encoding/decoding for Cardano wire format via pallas |
| `torsten-network` | Ouroboros mini-protocols (ChainSync, BlockFetch, TxSubmission, KeepAlive), N2N client |
| `torsten-consensus` | Ouroboros Praos, chain selection, epoch transitions, slot leader checks |
| `torsten-ledger` | UTxO set, transaction validation, ledger state, certificate processing, native script evaluation |
| `torsten-mempool` | Thread-safe transaction mempool |
| `torsten-storage` | ChainDB (ImmutableDB via RocksDB + VolatileDB in-memory) |
| `torsten-node` | Main binary, config, topology, chain sync loop |
| `torsten-cli` | cardano-cli compatible CLI |

## Building

```bash
cargo build --release
```

## Running

```bash
# Run with default settings (mainnet)
cargo run --release --bin torsten-node -- \
  --config config.json \
  --topology topology.json \
  --database-path ./db \
  --socket-path ./node.sock \
  --host-addr 0.0.0.0 \
  --port 3001
```

### Testnet

To run on the Cardano preview testnet, use a topology file pointing to testnet relays and set the appropriate network magic in your config.

## Testing

```bash
cargo test --all
```

## Development

Zero-warning policy enforced — all code must compile with `cargo clippy -- -D warnings` and pass `cargo fmt --check`.

## Status

**Work in progress.** Core chain sync and block storage are functional. The node can connect to Cardano peers and synchronize blocks. Major areas under active development:

- [ ] Full Plutus script validation
- [ ] VRF/KES cryptographic verification
- [ ] Complete epoch transition logic
- [ ] Node-to-client (N2C) protocol for local queries
- [ ] Conway governance actions
- [ ] Full cardano-cli command parity

## License

MIT
