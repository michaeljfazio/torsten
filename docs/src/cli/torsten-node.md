# torsten-node Reference

`torsten-node` is the main Torsten node binary. It supports two subcommands: `run` (start the node) and `mithril-import` (import a Mithril snapshot for fast initial sync).

## run

Start the Torsten node:

```bash
torsten-node run [OPTIONS]
```

### Options

| Flag | Default | Description |
|------|---------|-------------|
| `--config` | `config/mainnet-config.json` | Path to the node configuration file |
| `--topology` | `config/mainnet-topology.json` | Path to the topology file |
| `--database-path` | `db` | Path to the database directory |
| `--socket-path` | `node.sock` | Unix domain socket path for N2C (local client) connections |
| `--port` | `3001` | TCP port for N2N (node-to-node) connections |
| `--host-addr` | `0.0.0.0` | Host address to bind to |
| `--metrics-port` | `12798` | Prometheus metrics port (set to `0` to disable) |
| `--shelley-kes-key` | | Path to the KES signing key (enables block production) |
| `--shelley-vrf-key` | | Path to the VRF signing key (enables block production) |
| `--shelley-operational-certificate` | | Path to the operational certificate (enables block production) |
| `--log-output` | `stdout` | Log output target: `stdout`, `file`, or `journald`. Can be specified multiple times. |
| `--log-format` | `text` | Log format: `text` (human-readable) or `json` (structured). |
| `--log-level` | `info` | Log level (`trace`, `debug`, `info`, `warn`, `error`). Overridden by `RUST_LOG`. |
| `--log-dir` | `logs` | Directory for log files (used with `--log-output file`) |
| `--log-file-rotation` | `daily` | Log file rotation strategy: `daily`, `hourly`, or `never` |
| `--log-no-color` | `false` | Disable ANSI colors in stdout output |
| `--mempool-max-tx` | `16384` | Maximum number of transactions in the mempool |
| `--mempool-max-bytes` | `536870912` | Maximum mempool size in bytes (default 512 MB) |
| `--snapshot-max-retained` | `2` | Maximum number of ledger snapshots to retain on disk |
| `--snapshot-bulk-min-blocks` | `50000` | Minimum blocks between bulk-sync snapshots |
| `--snapshot-bulk-min-secs` | `360` | Minimum seconds between bulk-sync snapshots |
| `--storage-profile` | `high-memory` | Storage profile: `ultra-memory` (32GB), `high-memory` (16GB), `low-memory` (8GB), or `minimal` (4GB) |
| `--immutable-index-type` | | Override block index type: `in-memory` or `mmap` |
| `--utxo-backend` | | Override UTxO backend: `in-memory` or `lsm` |
| `--utxo-memtable-size-mb` | | Override LSM memtable size in MB |
| `--utxo-block-cache-size-mb` | | Override LSM block cache size in MB |
| `--utxo-bloom-filter-bits` | | Override LSM bloom filter bits per key |

### Relay Node (default)

Run as a relay node with no block production keys:

```bash
torsten-node run \
  --config config/preview-config.json \
  --topology config/preview-topology.json \
  --database-path ./db-preview \
  --socket-path ./node.sock \
  --host-addr 0.0.0.0 \
  --port 3001
```

### Block Producer

Run as a block producer by providing all three key/certificate paths:

```bash
torsten-node run \
  --config config/preview-config.json \
  --topology config/preview-topology.json \
  --database-path ./db-preview \
  --socket-path ./node.sock \
  --host-addr 0.0.0.0 \
  --port 3001 \
  --shelley-kes-key ./keys/kes.skey \
  --shelley-vrf-key ./keys/vrf.skey \
  --shelley-operational-certificate ./keys/opcert.cert
```

When all three block producer flags are provided, the node enters block production mode. The cold signing key is not needed at runtime -- the cold verification key is extracted from the operational certificate, matching cardano-node behavior.

If any of the three flags is missing, the node runs in relay-only mode.

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `TORSTEN_PIPELINE_DEPTH` | `150` | ChainSync pipeline depth (number of blocks requested ahead) |
| `RUST_LOG` | `info` | Log level filter (e.g., `debug`, `info`, `warn`, `torsten_node=debug`). Overrides `--log-level`. |

See [Logging](../running/logging.md) for details on output targets, file rotation, and per-crate filtering.

### Configuration File

The `--config` file follows the same JSON format as cardano-node. Key fields:

```json
{
  "Protocol": "Cardano",
  "RequiresNetworkMagic": "RequiresMagic",
  "ByronGenesisFile": "byron-genesis.json",
  "ShelleyGenesisFile": "shelley-genesis.json",
  "AlonzoGenesisFile": "alonzo-genesis.json",
  "ConwayGenesisFile": "conway-genesis.json"
}
```

Genesis file paths are resolved relative to the directory containing the config file.

### Metrics

When `--metrics-port` is non-zero, Prometheus metrics are served at `http://localhost:<port>/metrics`. See [Monitoring](../running/monitoring.md) for the full list of available metrics.

## mithril-import

Import a Mithril snapshot for fast initial sync. This downloads and verifies a certified snapshot from a Mithril aggregator, then imports all blocks into the local database.

```bash
torsten-node mithril-import [OPTIONS]
```

### Options

| Flag | Default | Description |
|------|---------|-------------|
| `--network-magic` | `764824073` | Network magic value |
| `--database-path` | `db` | Path to the database directory |
| `--temp-dir` | | Temporary directory for download and extraction (uses system temp if omitted) |
| `--log-output` | `stdout` | Log output target: `stdout`, `file`, or `journald`. Can be specified multiple times. |
| `--log-format` | `text` | Log format: `text` (human-readable) or `json` (structured). |
| `--log-level` | `info` | Log level (`trace`, `debug`, `info`, `warn`, `error`). Overridden by `RUST_LOG`. |
| `--log-dir` | `logs` | Directory for log files (used with `--log-output file`) |
| `--log-file-rotation` | `daily` | Log file rotation strategy: `daily`, `hourly`, or `never` |
| `--log-no-color` | `false` | Disable ANSI colors in stdout output |

### Network Magic Values

| Network | Magic |
|---------|-------|
| Mainnet | `764824073` |
| Preview | `2` |
| Preprod | `1` |

### Example: Preview Testnet

```bash
torsten-node mithril-import \
  --network-magic 2 \
  --database-path ./db-preview

# Then start the node to sync from the snapshot to tip
torsten-node run \
  --config config/preview-config.json \
  --topology config/preview-topology.json \
  --database-path ./db-preview \
  --socket-path ./node.sock
```

The import process:

1. Downloads the latest snapshot from the Mithril aggregator
2. Verifies the snapshot digest (SHA256)
3. Extracts and parses immutable chunk files
4. Imports blocks into ChainDB with CRC32 verification
5. Supports resume -- skips blocks already in the database

On preview testnet, importing ~4M blocks takes approximately 2 minutes.
