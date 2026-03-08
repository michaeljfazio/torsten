# Configuration

Torsten reads a JSON configuration file that controls network settings, genesis file paths, P2P parameters, and tracing options. The format is compatible with the cardano-node configuration format.

## Configuration File Format

The configuration file uses PascalCase keys (matching the cardano-node convention):

```json
{
  "Network": "Testnet",
  "NetworkMagic": 2,
  "EnableP2P": true,
  "Protocol": {
    "RequiresNetworkMagic": "RequiresMagic"
  },
  "ShelleyGenesisFile": "shelley-genesis.json",
  "ByronGenesisFile": "byron-genesis.json",
  "AlonzoGenesisFile": "alonzo-genesis.json",
  "ConwayGenesisFile": "conway-genesis.json",
  "TargetNumberOfActivePeers": 20,
  "TargetNumberOfEstablishedPeers": 40,
  "TargetNumberOfKnownPeers": 100,
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
| `EnableP2P` | boolean | `true` | Enable P2P networking mode |

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

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `TargetNumberOfActivePeers` | integer | 20 | Target number of active (hot) peers |
| `TargetNumberOfEstablishedPeers` | integer | 40 | Target number of established (warm) peers |
| `TargetNumberOfKnownPeers` | integer | 100 | Target number of known (cold) peers |

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

Torsten uses the `RUST_LOG` environment variable for fine-grained log control:

```bash
# Default (info level)
RUST_LOG=info torsten-node run ...

# Debug level for all crates
RUST_LOG=debug torsten-node run ...

# Debug only for specific crates
RUST_LOG=torsten_network=debug,torsten_consensus=debug torsten-node run ...

# Trace level for detailed diagnostics
RUST_LOG=trace torsten-node run ...
```

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
