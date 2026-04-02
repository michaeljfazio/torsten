# Quick Start

This guide walks you through getting Dugite running on the Cardano preview testnet.

## 1. Install

**Option A: Pre-built binary** (fastest)

```bash
curl -LO https://github.com/michaeljfazio/dugite/releases/latest/download/dugite-x86_64-linux.tar.gz
tar xzf dugite-x86_64-linux.tar.gz
sudo mv dugite-node dugite-cli dugite-monitor dugite-config /usr/local/bin/
```

**Option B: Container image**

```bash
docker pull ghcr.io/michaeljfazio/dugite:latest
```

**Option C: Build from source**

```bash
git clone https://github.com/michaeljfazio/dugite.git
cd dugite
cargo build --release
```

## 2. Fast Sync with Mithril (Recommended)

Import a Mithril-certified snapshot to skip syncing millions of blocks from genesis:

```bash
dugite-node mithril-import \
  --network-magic 2 \
  --database-path ./db-preview
```

This downloads the latest snapshot from the Mithril aggregator, extracts it, and imports all blocks into the database. On preview testnet this takes approximately 9 minutes (downloading a ~2.7 GB snapshot containing ~4M blocks).

## 3. Run the Node

Dugite ships with configuration files for all networks. If you built from source, they are in the `config/` directory:

```bash
dugite-node run \
  --config config/preview-config.json \
  --topology config/preview-topology.json \
  --database-path ./db-preview \
  --socket-path ./node.sock \
  --host-addr 0.0.0.0 \
  --port 3001
```

Or with Docker:

```bash
docker run -d \
  --name dugite \
  -p 3001:3001 \
  -p 12798:12798 \
  -v dugite-data:/opt/dugite/db \
  ghcr.io/michaeljfazio/dugite:latest
```

The node will:
1. Load the configuration and genesis files
2. Replay imported blocks through the ledger (builds UTxO set, protocol params, delegations)
3. Connect to preview testnet peers
4. Sync remaining blocks to chain tip

Progress is logged every 5 seconds, showing sync percentage, blocks-per-second throughput, UTxO count, and epoch number. Logs go to stdout by default; add `--log-output file --log-dir /var/log/dugite` for file logging. See [Logging](./running/logging.md) for all options.

## 4. Query the Node

Once the node is running, query it using the CLI via the Unix domain socket:

```bash
# Query the current tip
dugite-cli query tip \
  --socket-path ./node.sock \
  --testnet-magic 2
```

Example output:

```json
{
    "slot": 106453897,
    "hash": "8498ccda...",
    "block": 4094745,
    "epoch": 1232,
    "era": "Conway",
    "syncProgress": "100.00"
}
```

```bash
# Query protocol parameters
dugite-cli query protocol-parameters \
  --socket-path ./node.sock \
  --testnet-magic 2

# Query mempool
dugite-cli query tx-mempool info \
  --socket-path ./node.sock \
  --testnet-magic 2
```

## 5. Check Metrics

Prometheus metrics are served on port 12798:

```bash
curl -s http://localhost:12798/metrics | grep sync_progress
# sync_progress_percent 10000
```

## Next Steps

- [Configuration](./running/configuration.md) — Detailed configuration options
- [Networks](./running/networks.md) — Connecting to mainnet, preview, or preprod
- [Mithril Import](./running/mithril.md) — Fast initial sync details
- [Monitoring](./running/monitoring.md) — Prometheus metrics endpoint
- [Kubernetes Deployment](./running/kubernetes.md) — Helm chart for production deployments
- [Relay Node](./running/relay.md) — Running relay nodes for a stake pool
- [Block Producer](./running/block-producer.md) — Running a stake pool
- [CLI Reference](./cli/overview.md) — Full CLI command reference
