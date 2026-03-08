# Networks

Torsten can connect to any Cardano network. Each network is identified by a unique magic number used during the N2N handshake.

## Network Magic Values

| Network | Magic | Description |
|---------|-------|-------------|
| **Mainnet** | `764824073` | The production Cardano network |
| **Preview** | `2` | Fast-moving testnet for early feature testing |
| **Preprod** | `1` | Stable testnet that mirrors mainnet behavior |

## Connecting to Mainnet

Create a `config-mainnet.json`:

```json
{
  "Network": "Mainnet",
  "NetworkMagic": 764824073
}
```

Create a `topology-mainnet.json`:

```json
{
  "bootstrapPeers": [
    { "address": "backbone.cardano.iog.io", "port": 3001 },
    { "address": "backbone.mainnet.cardanofoundation.org", "port": 3001 },
    { "address": "backbone.mainnet.emurgornd.com", "port": 3001 }
  ],
  "localRoots": [{ "accessPoints": [], "advertise": false, "valency": 1 }],
  "publicRoots": [{ "accessPoints": [], "advertise": false }],
  "useLedgerAfterSlot": 177724800
}
```

Run the node:

```bash
torsten-node run \
  --config config-mainnet.json \
  --topology topology-mainnet.json \
  --database-path ./db-mainnet \
  --socket-path ./node-mainnet.sock \
  --host-addr 0.0.0.0 \
  --port 3001
```

> **Tip:** For a faster initial mainnet sync, consider using [Mithril snapshot import](./mithril.md) first.

## Connecting to Preview Testnet

Create a `config-preview.json`:

```json
{
  "Network": "Testnet",
  "NetworkMagic": 2
}
```

Create a `topology-preview.json`:

```json
{
  "bootstrapPeers": [
    { "address": "preview-node.play.dev.cardano.org", "port": 3001 }
  ],
  "localRoots": [{ "accessPoints": [], "advertise": false, "valency": 1 }],
  "publicRoots": [{ "accessPoints": [], "advertise": false }],
  "useLedgerAfterSlot": 102729600
}
```

Run the node:

```bash
torsten-node run \
  --config config-preview.json \
  --topology topology-preview.json \
  --database-path ./db-preview \
  --socket-path ./node-preview.sock \
  --host-addr 0.0.0.0 \
  --port 3001
```

## Connecting to Preprod Testnet

Create a `config-preprod.json`:

```json
{
  "Network": "Testnet",
  "NetworkMagic": 1
}
```

Create a `topology-preprod.json`:

```json
{
  "bootstrapPeers": [
    { "address": "preprod-node.play.dev.cardano.org", "port": 3001 }
  ],
  "localRoots": [{ "accessPoints": [], "advertise": false, "valency": 1 }],
  "publicRoots": [{ "accessPoints": [], "advertise": false }],
  "useLedgerAfterSlot": 76924800
}
```

Run the node:

```bash
torsten-node run \
  --config config-preprod.json \
  --topology topology-preprod.json \
  --database-path ./db-preprod \
  --socket-path ./node-preprod.sock \
  --host-addr 0.0.0.0 \
  --port 3001
```

## Official Configuration Files

Official configuration and topology files for each network are maintained in the Cardano Operations Book:

- **Preview:** [book.world.dev.cardano.org/environments/preview/](https://book.world.dev.cardano.org/environments/preview/)
- **Preprod:** [book.world.dev.cardano.org/environments/preprod/](https://book.world.dev.cardano.org/environments/preprod/)
- **Mainnet:** [book.world.dev.cardano.org/environments/mainnet/](https://book.world.dev.cardano.org/environments/mainnet/)

These include the full genesis files (Byron, Shelley, Alonzo, Conway) required for complete protocol parameter initialization.

## Using the CLI with Different Networks

When querying a node connected to a testnet, pass the `--testnet-magic` flag to the CLI:

```bash
# Preview
torsten-cli query tip --socket-path ./node-preview.sock --testnet-magic 2

# Preprod
torsten-cli query tip --socket-path ./node-preprod.sock --testnet-magic 1

# Mainnet (default, --testnet-magic not needed)
torsten-cli query tip --socket-path ./node-mainnet.sock
```

## Multiple Nodes

You can run multiple Torsten instances on the same machine by using different ports, database paths, and socket paths:

```bash
# Preview on port 3001
torsten-node run --port 3001 --database-path ./db-preview --socket-path ./preview.sock ...

# Preprod on port 3002
torsten-node run --port 3002 --database-path ./db-preprod --socket-path ./preprod.sock ...
```
