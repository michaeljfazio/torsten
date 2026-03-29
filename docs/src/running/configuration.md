# Configuration

Torsten reads a JSON configuration file that controls network settings, genesis file paths, P2P parameters, and tracing options. The format is compatible with the cardano-node configuration format.

## Configuration File Format

The configuration file uses PascalCase keys (matching the cardano-node convention):

```json
{
  "Network": "Testnet",
  "NetworkMagic": 2,
  "EnableP2P": true,
  "DiffusionMode": "InitiatorAndResponder",
  "PeerSharing": null,
  "Protocol": {
    "RequiresNetworkMagic": "RequiresMagic"
  },
  "ShelleyGenesisFile": "shelley-genesis.json",
  "ByronGenesisFile": "byron-genesis.json",
  "AlonzoGenesisFile": "alonzo-genesis.json",
  "ConwayGenesisFile": "conway-genesis.json",
  "TargetNumberOfRootPeers": 60,
  "TargetNumberOfActivePeers": 15,
  "TargetNumberOfEstablishedPeers": 40,
  "TargetNumberOfKnownPeers": 85,
  "TargetNumberOfActiveBigLedgerPeers": 5,
  "TargetNumberOfEstablishedBigLedgerPeers": 10,
  "TargetNumberOfKnownBigLedgerPeers": 15,
  "MinSeverity": "Info",
  "TraceOptions": {
    "TraceBlockFetchClient": false,
    "TraceBlockFetchServer": false,
    "TraceChainDb": false,
    "TraceChainSyncClient": false,
    "TraceChainSyncServer": false,
    "TraceForge": false,
    "TraceMempool": false
  }
}
```

## Fields Reference

### Network Settings

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `Network` | string | `"Mainnet"` | Network identifier: `"Mainnet"` or `"Testnet"` |
| `NetworkMagic` | integer | auto | Network magic number. If omitted, derived from `Network` (764824073 for mainnet) |
| `EnableP2P` | boolean | `true` | Enable P2P networking mode. When `true` (the default), the peer governor manages peer connections with automatic churn, ledger-based discovery, and peer sharing. When `false`, the node uses only static topology connections |
| `DiffusionMode` | string | `"InitiatorAndResponder"` | Controls inbound connection acceptance. `"InitiatorAndResponder"` (default): relay mode, accepts inbound N2N connections. `"InitiatorOnly"`: block producer mode, outbound only (no listening port opened) |
| `PeerSharing` | boolean/null | `null` | Enable the peer sharing mini-protocol. When `null` (default), peer sharing is automatically disabled for block producers (when `--shelley-kes-key` is provided) and enabled for relays. Set explicitly to override |

### Protocol

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `Protocol.RequiresNetworkMagic` | string | `"RequiresMagic"` | Whether network magic is required in handshake |

### Genesis Files

Genesis file paths are resolved relative to the directory containing the configuration file. For example, if your config is at `/opt/cardano/config.json` and specifies `"ShelleyGenesisFile": "shelley-genesis.json"`, Torsten will look for `/opt/cardano/shelley-genesis.json`.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `ShelleyGenesisFile` | string | none | Path to Shelley genesis JSON |
| `ByronGenesisFile` | string | none | Path to Byron genesis JSON |
| `AlonzoGenesisFile` | string | none | Path to Alonzo genesis JSON |
| `ConwayGenesisFile` | string | none | Path to Conway genesis JSON |

> **Tip:** Genesis files for each network can be downloaded from the [Cardano Operations Book](https://book.world.dev.cardano.org/).

### P2P Parameters

These parameters control the P2P peer governor's target counts, matching the cardano-node defaults. The governor continuously works to maintain these targets by promoting/demoting peers and discovering new ones.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `TargetNumberOfRootPeers` | integer | 60 | Target number of root peers (bootstrap + local + public roots) |
| `TargetNumberOfActivePeers` | integer | 15 | Target number of active (hot) peers — fully syncing with ChainSync + BlockFetch |
| `TargetNumberOfEstablishedPeers` | integer | 40 | Target number of established (warm) peers — TCP connected, keepalive running |
| `TargetNumberOfKnownPeers` | integer | 85 | Target number of known (cold) peers in the peer table |
| `TargetNumberOfActiveBigLedgerPeers` | integer | 5 | Target number of active big ledger peers (high-stake SPOs, prioritised during sync) |
| `TargetNumberOfEstablishedBigLedgerPeers` | integer | 10 | Target number of established big ledger peers |
| `TargetNumberOfKnownBigLedgerPeers` | integer | 15 | Target number of known big ledger peers |

### Tracing

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `MinSeverity` | string | `"Info"` | Minimum log severity level |
| `TraceOptions.TraceBlockFetchClient` | boolean | `false` | Trace block fetch client activity |
| `TraceOptions.TraceBlockFetchServer` | boolean | `false` | Trace block fetch server activity |
| `TraceOptions.TraceChainDb` | boolean | `false` | Trace ChainDB operations |
| `TraceOptions.TraceChainSyncClient` | boolean | `false` | Trace chain sync client activity |
| `TraceOptions.TraceChainSyncServer` | boolean | `false` | Trace chain sync server activity |
| `TraceOptions.TraceForge` | boolean | `false` | Trace block forging |
| `TraceOptions.TraceMempool` | boolean | `false` | Trace mempool activity |

## Log Level Control

The log level can be set via CLI flag or environment variable:

```bash
# Via CLI flag
torsten-node run --log-level debug ...

# Via environment variable (takes priority over --log-level)
RUST_LOG=info torsten-node run ...

# Debug only for specific crates
RUST_LOG=torsten_network=debug,torsten_consensus=debug torsten-node run ...
```

Torsten supports multiple log output targets (stdout, file, journald) and file rotation. See [Logging](./logging.md) for full details on output configuration.

## Minimal Configuration

The smallest viable configuration file specifies only the network:

```json
{
  "Network": "Testnet",
  "NetworkMagic": 2
}
```

All other fields use sensible defaults. When no genesis files are specified, the node operates with built-in default parameters.

## Format Support

Torsten supports both JSON (`.json`) and TOML (`.toml`) configuration files. The format is determined by the file extension. JSON files use the cardano-node compatible PascalCase format shown above.

## Interactive Configuration Editor (torsten-config)

`torsten-config` is a standalone TUI tool for creating and editing Torsten configuration files interactively, without needing to remember field names or valid value ranges.

### Installation

Built as part of the standard workspace:

```bash
cargo build --release -p torsten-config
```

### Commands

```bash
# Interactively create a new configuration file
torsten-config init --out-file config.json

# Launch the interactive editor for an existing config file
torsten-config edit config.json

# Validate a configuration file and report errors
torsten-config validate config.json

# Get the value of a specific field
torsten-config get config.json TargetNumberOfActivePeers

# Set the value of a specific field
torsten-config set config.json TargetNumberOfActivePeers 30
```

### Interactive Editor Features

The editor provides a tree view of all configuration fields with inline editing:

- **Tree navigation** — expand/collapse sections, navigate fields with arrow keys
- **Inline editing** — press `Enter` on any field to edit its value in place
- **Type validation** — invalid values are rejected with an inline error message and a description of the expected type and range
- **Tuning hints** — contextual hints appear alongside each field explaining the impact of changes (for example, peer count targets and their effect on network connectivity)
- **Search/filter** — press `/` to search fields by name
- **Diff view** — press `d` to see a side-by-side diff of your changes before saving
- **Save/discard** — `Ctrl+S` to save, `Ctrl+Q` or `Esc` to discard

The editor validates the full configuration on save and will not write an invalid file.
