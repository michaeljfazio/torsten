# Stake Address Commands

The `dugite-cli stake-address` subcommands manage stake key generation, reward address construction, and certificate creation for staking operations.

## key-gen

Generate a stake key pair:

```bash
dugite-cli stake-address key-gen \
  --verification-key-file stake.vkey \
  --signing-key-file stake.skey
```

| Flag | Required | Description |
|------|----------|-------------|
| `--verification-key-file` | Yes | Output path for the stake verification key |
| `--signing-key-file` | Yes | Output path for the stake signing key |

## build

Build a stake (reward) address from a stake verification key:

```bash
dugite-cli stake-address build \
  --stake-verification-key-file stake.vkey \
  --network testnet
```

| Flag | Required | Default | Description |
|------|----------|---------|-------------|
| `--stake-verification-key-file` | Yes | | Path to the stake verification key |
| `--network` | No | `mainnet` | Network: `mainnet` or `testnet` |
| `--out-file` | No | | Output file (prints to stdout if omitted) |

## registration-certificate

Create a stake address registration certificate:

```bash
# Conway era (with deposit)
dugite-cli stake-address registration-certificate \
  --stake-verification-key-file stake.vkey \
  --key-reg-deposit-amt 2000000 \
  --out-file stake-reg.cert

# Legacy Shelley era (no deposit parameter)
dugite-cli stake-address registration-certificate \
  --stake-verification-key-file stake.vkey \
  --out-file stake-reg.cert
```

| Flag | Required | Description |
|------|----------|-------------|
| `--stake-verification-key-file` | Yes | Path to the stake verification key |
| `--key-reg-deposit-amt` | No | Deposit amount in lovelace (Conway era; omit for legacy Shelley cert) |
| `--out-file` | Yes | Output path for the certificate |

The deposit amount should match the current `stakeAddressDeposit` protocol parameter (typically 2 ADA = 2000000 lovelace).

## deregistration-certificate

Create a stake address deregistration certificate to reclaim the deposit:

```bash
dugite-cli stake-address deregistration-certificate \
  --stake-verification-key-file stake.vkey \
  --key-reg-deposit-amt 2000000 \
  --out-file stake-dereg.cert
```

| Flag | Required | Description |
|------|----------|-------------|
| `--stake-verification-key-file` | Yes | Path to the stake verification key |
| `--key-reg-deposit-amt` | No | Deposit refund amount (Conway era; omit for legacy Shelley cert) |
| `--out-file` | Yes | Output path for the certificate |

## delegation-certificate

Create a stake delegation certificate to delegate to a stake pool:

```bash
dugite-cli stake-address delegation-certificate \
  --stake-verification-key-file stake.vkey \
  --stake-pool-id pool1abc... \
  --out-file delegation.cert
```

| Flag | Required | Description |
|------|----------|-------------|
| `--stake-verification-key-file` | Yes | Path to the stake verification key |
| `--stake-pool-id` | Yes | Pool ID to delegate to (bech32 or hex) |
| `--out-file` | Yes | Output path for the certificate |

## vote-delegation-certificate

Create a vote delegation certificate (Conway era) to delegate voting power to a DRep:

```bash
# Delegate to a specific DRep
dugite-cli stake-address vote-delegation-certificate \
  --stake-verification-key-file stake.vkey \
  --drep-verification-key-file drep.vkey \
  --out-file vote-deleg.cert

# Delegate to always-abstain
dugite-cli stake-address vote-delegation-certificate \
  --stake-verification-key-file stake.vkey \
  --always-abstain \
  --out-file vote-deleg.cert

# Delegate to always-no-confidence
dugite-cli stake-address vote-delegation-certificate \
  --stake-verification-key-file stake.vkey \
  --always-no-confidence \
  --out-file vote-deleg.cert
```

| Flag | Required | Description |
|------|----------|-------------|
| `--stake-verification-key-file` | Yes | Path to the stake verification key |
| `--drep-verification-key-file` | No | DRep verification key file (mutually exclusive with --always-abstain/--always-no-confidence) |
| `--always-abstain` | No | Use the special always-abstain DRep |
| `--always-no-confidence` | No | Use the special always-no-confidence DRep |
| `--out-file` | Yes | Output path for the certificate |

## Complete Staking Workflow

```bash
# 1. Generate stake keys
dugite-cli stake-address key-gen \
  --verification-key-file stake.vkey \
  --signing-key-file stake.skey

# 2. Create registration certificate
dugite-cli stake-address registration-certificate \
  --stake-verification-key-file stake.vkey \
  --key-reg-deposit-amt 2000000 \
  --out-file stake-reg.cert

# 3. Create delegation certificate
dugite-cli stake-address delegation-certificate \
  --stake-verification-key-file stake.vkey \
  --stake-pool-id pool1abc... \
  --out-file delegation.cert

# 4. Submit both in a single transaction
dugite-cli transaction build \
  --tx-in "abc123...#0" \
  --tx-out "addr_test1qz...+5000000" \
  --change-address "addr_test1qp..." \
  --fee 200000 \
  --certificate-file stake-reg.cert \
  --certificate-file delegation.cert \
  --out-file tx.body

dugite-cli transaction sign \
  --tx-body-file tx.body \
  --signing-key-file payment.skey \
  --signing-key-file stake.skey \
  --out-file tx.signed

dugite-cli transaction submit \
  --tx-file tx.signed \
  --socket-path ./node.sock
```
