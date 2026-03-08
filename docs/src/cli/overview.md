# CLI Overview

Torsten provides `torsten-cli`, a cardano-cli compatible command-line interface for interacting with a running Torsten node and managing keys, transactions, and governance.

## Binary

```bash
torsten-cli [COMMAND] [OPTIONS]
```

## Command Groups

| Command | Description |
|---------|-------------|
| `address` | Address generation and manipulation |
| `key` | Payment and stake key generation |
| `transaction` | Transaction building, signing, and submission |
| `query` | Node queries (tip, UTxO, protocol parameters, etc.) |
| `stake-address` | Stake address registration, delegation, and vote delegation |
| `stake-pool` | Stake pool operations (retirement certificates) |
| `governance` | Conway governance (DRep, voting, proposals) |
| `node` | Node key operations (cold keys, KES, VRF, operational certificates) |

## Common Patterns

### Socket Path

Most commands that interact with a running node require `--socket-path` to specify the Unix domain socket:

```bash
torsten-cli query tip --socket-path ./node.sock
```

The default socket path is `node.sock` in the current directory.

### Testnet Magic

When querying a node on a testnet, pass the `--testnet-magic` flag:

```bash
torsten-cli query tip --socket-path ./node.sock --testnet-magic 2
```

For mainnet, `--testnet-magic` is not needed (defaults to mainnet magic 764824073).

### Text Envelope Format

Keys, certificates, and transactions are stored in the cardano-node "text envelope" JSON format:

```json
{
  "type": "PaymentSigningKeyShelley_ed25519",
  "description": "Payment Signing Key",
  "cborHex": "5820..."
}
```

This format is interchangeable with files produced by `cardano-cli`.

### Output Files

Commands that produce artifacts use `--out-file`:

```bash
torsten-cli transaction build ... --out-file tx.body
torsten-cli transaction sign ... --out-file tx.signed
```

## Help

Every command supports `--help`:

```bash
torsten-cli --help
torsten-cli transaction --help
torsten-cli transaction build --help
```
