# Quick Start

This guide walks you through building Torsten and syncing against the Cardano preview testnet.

## 1. Build

```bash
git clone https://github.com/michaeljfazio/torsten.git
cd torsten
cargo build --release
```

## 2. Create Configuration Files

Create a `config-preview.json`:

```json
{
  "Network": "Testnet",
  "NetworkMagic": 2
}
```

Create a `topology-preview.json` with preview testnet relays:

```json
{
  "bootstrapPeers": [
    {
      "address": "preview-node.play.dev.cardano.org",
      "port": 3001
    }
  ],
  "localRoots": [{ "accessPoints": [], "advertise": false, "valency": 1 }],
  "publicRoots": [{ "accessPoints": [], "advertise": false }],
  "useLedgerAfterSlot": 102729600
}
```

> **Tip:** You can also download the official topology directly from the [Cardano Operations Book](https://book.world.dev.cardano.org/environments/preview/topology.json).

## 3. Run the Node

```bash
./target/release/torsten-node run \
  --config config-preview.json \
  --topology topology-preview.json \
  --database-path ./db-preview \
  --socket-path ./node-preview.sock \
  --host-addr 0.0.0.0 \
  --port 3001
```

The node will:
1. Load the configuration and topology
2. Connect to the preview testnet bootstrap peers
3. Perform the N2N handshake (protocol version V14+)
4. Begin syncing blocks

Progress is logged every 5 seconds, showing:
- Current slot and block number
- Epoch number
- UTxO count
- Sync percentage
- Blocks-per-second throughput

## 4. Query the Node

Once the node is running, you can query it using the CLI via the Unix domain socket:

```bash
# Query the current tip
./target/release/torsten-cli query tip \
  --socket-path ./node-preview.sock \
  --testnet-magic 2
```

Example output:

```json
{
    "slot": 73429851,
    "hash": "a1b2c3d4...",
    "block": 2847392,
    "epoch": 170,
    "era": "Conway",
    "syncProgress": "95.42"
}
```

## 5. Fast Sync with Mithril (Optional)

To significantly reduce initial sync time, you can import a Mithril-certified snapshot before starting the node:

```bash
./target/release/torsten-node mithril-import \
  --network-magic 2 \
  --database-path ./db-preview
```

This downloads the latest snapshot from the Mithril aggregator, extracts it, and imports all blocks into the database. After the import completes, start the node normally and it will resume syncing from where the snapshot left off.

## Next Steps

- [Configuration](./running/configuration.md) -- Detailed configuration options
- [Networks](./running/networks.md) -- Connecting to mainnet, preview, or preprod
- [Monitoring](./running/monitoring.md) -- Prometheus metrics endpoint
- [CLI Reference](./cli/overview.md) -- Full CLI command reference
