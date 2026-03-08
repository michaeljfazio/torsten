# Torsten

A Cardano node implementation written in Rust, aiming for 100% compatibility with [cardano-node](https://github.com/IntersectMBO/cardano-node).

Built by [Sandstone Pool](https://www.sandstone.io/)

[![CI](https://github.com/michaeljfazio/torsten/actions/workflows/ci.yml/badge.svg)](https://github.com/michaeljfazio/torsten/actions/workflows/ci.yml)

## Architecture

Torsten is organized as a 10-crate Cargo workspace:

| Crate | Description |
|-------|-------------|
| `torsten-primitives` | Core types: hashes, blocks, transactions, addresses, values, protocol parameters (Byron–Conway) |
| `torsten-crypto` | Ed25519 keys, VRF, KES, text envelope format |
| `torsten-serialization` | CBOR encoding/decoding for Cardano wire format via pallas |
| `torsten-network` | Ouroboros mini-protocols (ChainSync, BlockFetch, TxSubmission, KeepAlive), N2N client, N2C server |
| `torsten-consensus` | Ouroboros Praos, chain selection, epoch transitions, slot leader checks |
| `torsten-ledger` | UTxO set, transaction validation, ledger state, certificate processing, native script evaluation, reward calculation |
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

### Cardano Preview Testnet

To sync against the Cardano preview testnet:

1. Create a `config-preview.json`:

```json
{
  "network": "preview",
  "network_magic": 2
}
```

2. Create a `topology-preview.json` with preview testnet relays:

```json
{
  "producers": [
    {
      "addr": "preview-node.play.dev.cardano.org",
      "port": 3001,
      "valency": 1
    }
  ]
}
```

3. Run the node:

```bash
cargo run --release --bin torsten-node -- \
  --config config-preview.json \
  --topology topology-preview.json \
  --database-path ./db-preview \
  --socket-path ./node-preview.sock \
  --host-addr 0.0.0.0 \
  --port 3001
```

The node will connect to the preview testnet, perform the N2N handshake, and begin syncing blocks. Progress is logged periodically showing slot, block number, UTxO count, epoch, and sync percentage.

#### Network Magic Values

| Network | Magic |
|---------|-------|
| Mainnet | `764824073` |
| Preview | `2` |
| Preprod | `1` |

## Testing

```bash
cargo test --all
```

## Development

Zero-warning policy enforced — all code must compile with `cargo clippy -- -D warnings` and pass `cargo fmt --check`.

## Status

**Work in progress.** The node can connect to Cardano peers via N2N, synchronize blocks in batches, apply them to the ledger state (UTxO tracking, certificate processing, reward calculation), and serve local state queries via N2C Unix socket.

### Completed

- [x] N2N chain sync with batch block fetching
- [x] Block storage (ImmutableDB + VolatileDB with rollback)
- [x] Ledger state: UTxO, certificates, stake delegation
- [x] Epoch transitions: mark/set/go snapshots, reward distribution
- [x] N2C Unix socket server with local state query handler
- [x] N2C client for CLI queries (query tip, epoch, era)
- [x] Native script evaluation
- [x] Pallas 1.0 integration (N2N V14+)
- [x] Conway governance: DRep registration, vote delegation, committee auth, proposals, voting
- [x] CLI: transaction build, sign, view, txid
- [x] CLI: key generation (payment, stake, DRep), address building
- [x] Transaction submission via LocalTxSubmission mini-protocol
- [x] Governance action ratification and enactment (CIP-1694 voting thresholds)
- [x] Operational certificate Ed25519 signature verification
- [x] VRF leader eligibility check (phi_f threshold)
- [x] N2C governance state queries (GovState, DRepState, CommitteeState, StakeDistribution)
- [x] CLI: governance and stake distribution queries via N2C
- [x] Protocol parameters query (live from node state)
- [x] UTxO query by address (N2C + CLI, with pluggable UtxoQueryProvider trait)
- [x] Stake address info query (delegation + reward balance)

### In Progress

- [ ] Full VRF proof verification (requires VRF library)
- [ ] Full KES signature verification (requires KES library)
- [ ] Full Plutus script validation (CEK machine)
- [ ] Full cardano-cli command parity
- [ ] Performance optimization for initial sync
- [ ] Wire UtxoQueryProvider to live ledger state for address-filtered UTxO queries

## License

MIT
