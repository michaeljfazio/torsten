# Node Commands

The `torsten-cli node` subcommands manage cold keys, KES keys, VRF keys, and operational certificates for block producer setup.

## key-gen

Generate a cold key pair and an operational certificate issue counter:

```bash
torsten-cli node key-gen \
  --cold-verification-key-file cold.vkey \
  --cold-signing-key-file cold.skey \
  --operational-certificate-counter-file opcert.counter
```

| Flag | Required | Description |
|------|----------|-------------|
| `--cold-verification-key-file` | Yes | Output path for the cold verification key |
| `--cold-signing-key-file` | Yes | Output path for the cold signing key |
| `--operational-certificate-counter-file` | Yes | Output path for the opcert issue counter |

The cold key identifies your stake pool. Keep the signing key offline (air-gapped) after initial setup.

## key-gen-kes

Generate a KES (Key Evolving Signature) key pair:

```bash
torsten-cli node key-gen-kes \
  --verification-key-file kes.vkey \
  --signing-key-file kes.skey
```

| Flag | Required | Description |
|------|----------|-------------|
| `--verification-key-file` | Yes | Output path for the KES verification key |
| `--signing-key-file` | Yes | Output path for the KES signing key |

KES keys are rotated periodically. Each key is valid for a limited number of KES periods (62 periods on mainnet, approximately 90 days total).

## key-gen-vrf

Generate a VRF (Verifiable Random Function) key pair:

```bash
torsten-cli node key-gen-vrf \
  --verification-key-file vrf.vkey \
  --signing-key-file vrf.skey
```

| Flag | Required | Description |
|------|----------|-------------|
| `--verification-key-file` | Yes | Output path for the VRF verification key |
| `--signing-key-file` | Yes | Output path for the VRF signing key |

VRF keys are used for slot leader election and do not need rotation.

## issue-op-cert

Issue an operational certificate binding the cold key to the current KES key:

```bash
torsten-cli node issue-op-cert \
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
| `--operational-certificate-counter-file` | Yes | Path to the opcert issue counter (incremented automatically) |
| `--kes-period` | Yes | Current KES period (`current_slot / slots_per_kes_period`) |
| `--out-file` | Yes | Output path for the operational certificate |

The opcert must be regenerated each time you rotate KES keys. The counter file is incremented each time to prevent replay attacks.

## new-counter

Create a new operational certificate issue counter (useful if the original counter is lost):

```bash
torsten-cli node new-counter \
  --cold-verification-key-file cold.vkey \
  --counter-value 5 \
  --operational-certificate-counter-file opcert.counter
```

| Flag | Required | Description |
|------|----------|-------------|
| `--cold-verification-key-file` | Yes | Path to the cold verification key |
| `--counter-value` | Yes | Counter value to set |
| `--operational-certificate-counter-file` | Yes | Output path for the counter file |
