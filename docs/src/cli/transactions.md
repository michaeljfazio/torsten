# Transactions

Dugite CLI supports the full transaction lifecycle: building, signing, submitting, and inspecting transactions.

## Building a Transaction

```bash
dugite-cli transaction build \
  --tx-in <tx_hash>#<index> \
  --tx-out <address>+<lovelace> \
  --change-address <address> \
  --fee <lovelace> \
  --out-file tx.body
```

### Arguments

| Argument | Description |
|----------|-------------|
| `--tx-in` | Transaction input in `tx_hash#index` format. Can be specified multiple times |
| `--tx-out` | Transaction output in `address+lovelace` format. Can be specified multiple times |
| `--change-address` | Address to receive change |
| `--fee` | Fee in lovelace (default: 200000) |
| `--ttl` | Time-to-live slot number (optional) |
| `--certificate-file` | Path to a certificate file to include (can be repeated) |
| `--withdrawal` | Withdrawal in `stake_address+lovelace` format (can be repeated) |
| `--metadata-json-file` | Path to a JSON metadata file (optional) |
| `--out-file` | Output file for the transaction body |

### Example: Simple ADA Transfer

```bash
dugite-cli transaction build \
  --tx-in "abc123...#0" \
  --tx-out "addr_test1qz...+5000000" \
  --change-address "addr_test1qp..." \
  --fee 200000 \
  --ttl 50000000 \
  --out-file tx.body
```

### Multi-Asset Outputs

To include native tokens in an output, use the extended format:

```
address+lovelace+"policy_id.asset_name quantity"
```

Example:

```bash
dugite-cli transaction build \
  --tx-in "abc123...#0" \
  --tx-out 'addr_test1qz...+2000000+"a1b2c3...d4e5f6.4d79546f6b656e 100"' \
  --change-address "addr_test1qp..." \
  --fee 200000 \
  --out-file tx.body
```

Multiple tokens can be separated with `+` inside the quoted string:

```
"policy1.asset1 100+policy2.asset2 50"
```

### Including Certificates

```bash
dugite-cli transaction build \
  --tx-in "abc123...#0" \
  --tx-out "addr_test1qz...+5000000" \
  --change-address "addr_test1qp..." \
  --fee 200000 \
  --certificate-file stake-reg.cert \
  --certificate-file stake-deleg.cert \
  --out-file tx.body
```

### Including Metadata

Create a metadata JSON file with integer keys:

```json
{
  "674": {
    "msg": ["Hello, Cardano!"]
  }
}
```

```bash
dugite-cli transaction build \
  --tx-in "abc123...#0" \
  --tx-out "addr_test1qz...+5000000" \
  --change-address "addr_test1qp..." \
  --fee 200000 \
  --metadata-json-file metadata.json \
  --out-file tx.body
```

## Signing a Transaction

```bash
dugite-cli transaction sign \
  --tx-body-file tx.body \
  --signing-key-file payment.skey \
  --out-file tx.signed
```

Multiple signing keys can be provided:

```bash
dugite-cli transaction sign \
  --tx-body-file tx.body \
  --signing-key-file payment.skey \
  --signing-key-file stake.skey \
  --out-file tx.signed
```

## Submitting a Transaction

```bash
dugite-cli transaction submit \
  --tx-file tx.signed \
  --socket-path ./node.sock
```

The node validates the transaction (Phase-1 and Phase-2 for Plutus transactions) and, if valid, adds it to the mempool for propagation.

## Viewing a Transaction

```bash
dugite-cli transaction view --tx-file tx.signed
```

Output includes:
- Transaction type
- CBOR size
- Transaction hash
- Number of inputs and outputs
- Fee
- TTL (if set)

## Transaction ID

Compute the transaction hash:

```bash
dugite-cli transaction txid --tx-file tx.body
```

Works with both transaction body files and signed transaction files.

## Calculate Minimum Fee

```bash
dugite-cli transaction calculate-min-fee \
  --tx-body-file tx.body \
  --witness-count 2 \
  --protocol-params-file protocol-params.json
```

The fee calculation accounts for:

- Base fee: `txFeeFixed + txFeePerByte * tx_size`
- Script execution: `executionUnitPrices * total_ExUnits` for any Plutus witnesses
- Reference script surcharge: CIP-0112 tiered fee for reference scripts (25KiB tiers, 1.2x multiplier per tier)

To get the current protocol parameters:

```bash
dugite-cli query protocol-parameters \
  --socket-path ./node.sock \
  --out-file protocol-params.json
```

## Calculate Minimum Required UTxO

Compute the minimum lovelace required for a transaction output to satisfy the `minUTxOValue` protocol parameter:

```bash
dugite-cli transaction calculate-min-required-utxo \
  --protocol-params-file protocol-params.json \
  --tx-out "addr_test1qz...+0+\"policy1.asset1 100\""
```

Output:

```
Minimum required lovelace: 1724100
```

This is particularly useful when constructing outputs that carry native tokens, since the minimum lovelace depends on the byte-size of the value bundle (number of policy IDs, asset names, and quantities).

## Creating Witnesses

For multi-signature workflows, you can create witnesses separately and assemble them:

### Create a Witness

```bash
dugite-cli transaction witness \
  --tx-body-file tx.body \
  --signing-key-file payment.skey \
  --out-file payment.witness
```

### Assemble a Transaction

```bash
dugite-cli transaction assemble \
  --tx-body-file tx.body \
  --witness-file payment.witness \
  --witness-file stake.witness \
  --out-file tx.signed
```

## Policy ID

Compute the policy ID (Blake2b-224 hash) of a native script:

```bash
dugite-cli transaction policyid --script-file policy.script
```

## Complete Workflow

```bash
# 1. Query UTxOs to find inputs
dugite-cli query utxo \
  --address addr_test1qz... \
  --socket-path ./node.sock \
  --testnet-magic 2

# 2. Get protocol parameters for fee calculation
dugite-cli query protocol-parameters \
  --socket-path ./node.sock \
  --testnet-magic 2 \
  --out-file pp.json

# 3. Build the transaction
dugite-cli transaction build \
  --tx-in "abc123...#0" \
  --tx-out "addr_test1qr...+5000000" \
  --change-address "addr_test1qz..." \
  --fee 200000 \
  --out-file tx.body

# 4. Calculate the exact fee
dugite-cli transaction calculate-min-fee \
  --tx-body-file tx.body \
  --witness-count 1 \
  --protocol-params-file pp.json

# 5. Rebuild with the correct fee (repeat step 3 with updated --fee)

# 6. Sign
dugite-cli transaction sign \
  --tx-body-file tx.body \
  --signing-key-file payment.skey \
  --out-file tx.signed

# 7. Submit
dugite-cli transaction submit \
  --tx-file tx.signed \
  --socket-path ./node.sock
```
