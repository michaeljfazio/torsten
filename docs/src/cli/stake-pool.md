# Stake Pool Commands

The `torsten-cli stake-pool` subcommands manage stake pool key generation, pool registration, and operational certificate issuance.

## key-gen

Generate pool cold keys and an operational certificate counter:

```bash
torsten-cli stake-pool key-gen \
  --cold-verification-key-file cold.vkey \
  --cold-signing-key-file cold.skey \
  --operational-certificate-counter-file opcert.counter
```

| Flag | Required | Description |
|------|----------|-------------|
| `--cold-verification-key-file` | Yes | Output path for the cold verification key |
| `--cold-signing-key-file` | Yes | Output path for the cold signing key |
| `--operational-certificate-counter-file` | Yes | Output path for the opcert issue counter |

## id

Get the pool ID (Blake2b-224 hash of the cold verification key):

```bash
torsten-cli stake-pool id \
  --cold-verification-key-file cold.vkey
```

| Flag | Required | Description |
|------|----------|-------------|
| `--cold-verification-key-file` | Yes | Path to the cold verification key |

## vrf-key-gen

Generate a VRF key pair:

```bash
torsten-cli stake-pool vrf-key-gen \
  --verification-key-file vrf.vkey \
  --signing-key-file vrf.skey
```

| Flag | Required | Description |
|------|----------|-------------|
| `--verification-key-file` | Yes | Output path for the VRF verification key |
| `--signing-key-file` | Yes | Output path for the VRF signing key |

## kes-key-gen

Generate a KES key pair:

```bash
torsten-cli stake-pool kes-key-gen \
  --verification-key-file kes.vkey \
  --signing-key-file kes.skey
```

| Flag | Required | Description |
|------|----------|-------------|
| `--verification-key-file` | Yes | Output path for the KES verification key |
| `--signing-key-file` | Yes | Output path for the KES signing key |

## issue-op-cert

Issue an operational certificate:

```bash
torsten-cli stake-pool issue-op-cert \
  --kes-verification-key-file kes.vkey \
  --cold-signing-key-file cold.skey \
  --operational-certificate-counter-file opcert.counter \
  --kes-period 400 \
  --out-file opcert.cert
```

| Flag | Required | Description |
|------|----------|-------------|
| `--kes-verification-key-file` | Yes | Path to the KES verification key |
| `--cold-signing-key-file` | Yes | Path to the cold signing key |
| `--operational-certificate-counter-file` | Yes | Path to the opcert issue counter |
| `--kes-period` | Yes | Current KES period |
| `--out-file` | Yes | Output path for the operational certificate |

## registration-certificate

Create a stake pool registration certificate:

```bash
torsten-cli stake-pool registration-certificate \
  --cold-verification-key-file cold.vkey \
  --vrf-verification-key-file vrf.vkey \
  --pledge 500000000 \
  --cost 340000000 \
  --margin 0.02 \
  --reward-account-verification-key-file stake.vkey \
  --pool-owner-verification-key-file stake.vkey \
  --single-host-pool-relay "relay.example.com:3001" \
  --metadata-url "https://example.com/pool-metadata.json" \
  --metadata-hash "a1b2c3d4..." \
  --out-file pool-reg.cert
```

| Flag | Required | Description |
|------|----------|-------------|
| `--cold-verification-key-file` | Yes | Path to the cold verification key |
| `--vrf-verification-key-file` | Yes | Path to the VRF verification key |
| `--pledge` | Yes | Pledge amount in lovelace |
| `--cost` | Yes | Fixed cost per epoch in lovelace |
| `--margin` | Yes | Pool margin (0.0 to 1.0) |
| `--reward-account-verification-key-file` | Yes | Stake key for the reward account |
| `--pool-owner-verification-key-file` | No | Pool owner stake key (can be repeated) |
| `--pool-relay-ipv4` | No | Relay IP address with port (e.g., `1.2.3.4:3001`) |
| `--single-host-pool-relay` | No | Relay DNS hostname with port (e.g., `relay.example.com:3001`) |
| `--multi-host-pool-relay` | No | Relay DNS SRV record (e.g., `_cardano._tcp.example.com`) |
| `--metadata-url` | No | URL to pool metadata JSON |
| `--metadata-hash` | No | Blake2b-256 hash of the metadata file (hex) |
| `--testnet` | No | Use testnet network ID for the reward account |
| `--out-file` | Yes | Output path for the certificate |

## metadata-hash

Compute the Blake2b-256 hash of a pool metadata file:

```bash
torsten-cli stake-pool metadata-hash \
  --pool-metadata-file pool-metadata.json
```

Output:

```
a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2
```

This hash is required when registering a pool. The metadata file must be served at the URL specified in the registration certificate and the hash must match. The file contents at that URL are checked by other nodes during pool discovery.

**Example pool metadata file:**

```json
{
  "name": "Sandstone Pool",
  "description": "A Cardano stake pool running Torsten",
  "ticker": "SAND",
  "homepage": "https://sandstone.io"
}
```

## retirement-certificate

Create a stake pool retirement certificate:

```bash
torsten-cli stake-pool retirement-certificate \
  --cold-verification-key-file cold.vkey \
  --epoch 500 \
  --out-file pool-retire.cert
```

| Flag | Required | Description |
|------|----------|-------------|
| `--cold-verification-key-file` | Yes | Path to the cold verification key |
| `--epoch` | Yes | Epoch at which the pool retires |
| `--out-file` | Yes | Output path for the certificate |

## Complete Pool Registration Workflow

```bash
# 1. Generate all keys
torsten-cli stake-pool key-gen \
  --cold-verification-key-file cold.vkey \
  --cold-signing-key-file cold.skey \
  --operational-certificate-counter-file opcert.counter

torsten-cli stake-pool vrf-key-gen \
  --verification-key-file vrf.vkey \
  --signing-key-file vrf.skey

torsten-cli stake-pool kes-key-gen \
  --verification-key-file kes.vkey \
  --signing-key-file kes.skey

# 2. Issue operational certificate
torsten-cli stake-pool issue-op-cert \
  --kes-verification-key-file kes.vkey \
  --cold-signing-key-file cold.skey \
  --operational-certificate-counter-file opcert.counter \
  --kes-period 400 \
  --out-file opcert.cert

# 3. Create registration certificate
torsten-cli stake-pool registration-certificate \
  --cold-verification-key-file cold.vkey \
  --vrf-verification-key-file vrf.vkey \
  --pledge 500000000 \
  --cost 340000000 \
  --margin 0.02 \
  --reward-account-verification-key-file stake.vkey \
  --pool-owner-verification-key-file stake.vkey \
  --single-host-pool-relay "relay.example.com:3001" \
  --metadata-url "https://example.com/pool.json" \
  --metadata-hash "a1b2c3..." \
  --out-file pool-reg.cert

# 4. Submit registration in a transaction
torsten-cli transaction build \
  --tx-in "abc123...#0" \
  --tx-out "addr_test1qz...+5000000" \
  --change-address "addr_test1qp..." \
  --fee 200000 \
  --certificate-file pool-reg.cert \
  --out-file tx.body

torsten-cli transaction sign \
  --tx-body-file tx.body \
  --signing-key-file payment.skey \
  --signing-key-file cold.skey \
  --signing-key-file stake.skey \
  --out-file tx.signed

torsten-cli transaction submit \
  --tx-file tx.signed \
  --socket-path ./node.sock
```
