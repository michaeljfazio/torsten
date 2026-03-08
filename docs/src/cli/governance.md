# Governance

Torsten CLI supports Conway-era governance operations as defined in [CIP-1694](https://cips.cardano.org/cip/CIP-1694). This includes DRep management, voting, and governance action creation.

## DRep Operations

### Generate DRep Keys

```bash
torsten-cli governance drep key-gen \
  --signing-key-file drep.skey \
  --verification-key-file drep.vkey
```

### Get DRep ID

```bash
# Bech32 format (default)
torsten-cli governance drep id \
  --drep-verification-key-file drep.vkey

# Hex format
torsten-cli governance drep id \
  --drep-verification-key-file drep.vkey \
  --output-format hex
```

### DRep Registration

Create a DRep registration certificate:

```bash
torsten-cli governance drep registration-certificate \
  --drep-verification-key-file drep.vkey \
  --key-reg-deposit-amt 500000000 \
  --anchor-url "https://example.com/drep-metadata.json" \
  --anchor-data-hash "a1b2c3d4..." \
  --out-file drep-reg.cert
```

The `--key-reg-deposit-amt` should match the current DRep deposit parameter (currently 500 ADA = 500000000 lovelace on mainnet).

### DRep Retirement

```bash
torsten-cli governance drep retirement-certificate \
  --drep-verification-key-file drep.vkey \
  --deposit-amt 500000000 \
  --out-file drep-retire.cert
```

### DRep Update

Update DRep metadata:

```bash
torsten-cli governance drep update-certificate \
  --drep-verification-key-file drep.vkey \
  --anchor-url "https://example.com/drep-metadata-v2.json" \
  --anchor-data-hash "d4e5f6a7..." \
  --out-file drep-update.cert
```

## Voting

### Create a Vote

Votes can be cast by DReps, SPOs, or Constitutional Committee members:

**DRep vote:**

```bash
torsten-cli governance vote create \
  --governance-action-tx-id "a1b2c3d4..." \
  --governance-action-index 0 \
  --vote yes \
  --drep-verification-key-file drep.vkey \
  --out-file vote.json
```

**SPO vote:**

```bash
torsten-cli governance vote create \
  --governance-action-tx-id "a1b2c3d4..." \
  --governance-action-index 0 \
  --vote no \
  --cold-verification-key-file cold.vkey \
  --out-file vote.json
```

**Constitutional Committee vote:**

```bash
torsten-cli governance vote create \
  --governance-action-tx-id "a1b2c3d4..." \
  --governance-action-index 0 \
  --vote yes \
  --cc-hot-verification-key-file cc-hot.vkey \
  --out-file vote.json
```

### Vote Values

| Value | Description |
|-------|-------------|
| `yes` | Vote in favor |
| `no` | Vote against |
| `abstain` | Abstain from voting |

### Vote with Anchor

Attach rationale metadata to a vote:

```bash
torsten-cli governance vote create \
  --governance-action-tx-id "a1b2c3d4..." \
  --governance-action-index 0 \
  --vote yes \
  --drep-verification-key-file drep.vkey \
  --anchor-url "https://example.com/vote-rationale.json" \
  --anchor-data-hash "e5f6a7b8..." \
  --out-file vote.json
```

## Governance Actions

### Info Action

A governance action that carries no on-chain effect (used for signaling):

```bash
torsten-cli governance action create-info \
  --anchor-url "https://example.com/proposal.json" \
  --anchor-data-hash "a1b2c3d4..." \
  --deposit 100000000000 \
  --return-addr "addr_test1qz..." \
  --out-file info-action.json
```

### No Confidence Motion

Express no confidence in the current constitutional committee:

```bash
torsten-cli governance action create-no-confidence \
  --anchor-url "https://example.com/no-confidence.json" \
  --anchor-data-hash "a1b2c3d4..." \
  --deposit 100000000000 \
  --return-addr "addr_test1qz..." \
  --prev-governance-action-tx-id "d4e5f6a7..." \
  --prev-governance-action-index 0 \
  --out-file no-confidence.json
```

### New Constitution

Propose a new constitution:

```bash
torsten-cli governance action create-constitution \
  --anchor-url "https://example.com/constitution-proposal.json" \
  --anchor-data-hash "a1b2c3d4..." \
  --deposit 100000000000 \
  --return-addr "addr_test1qz..." \
  --constitution-url "https://example.com/constitution.txt" \
  --constitution-hash "e5f6a7b8..." \
  --constitution-script-hash "b8c9d0e1..." \
  --out-file new-constitution.json
```

### Hard Fork Initiation

Propose a protocol version change:

```bash
torsten-cli governance action create-hard-fork-initiation \
  --anchor-url "https://example.com/hardfork.json" \
  --anchor-data-hash "a1b2c3d4..." \
  --deposit 100000000000 \
  --return-addr "addr_test1qz..." \
  --protocol-major-version 10 \
  --protocol-minor-version 0 \
  --out-file hardfork.json
```

### Protocol Parameters Update

Propose changes to protocol parameters:

```bash
torsten-cli governance action create-protocol-parameters-update \
  --anchor-url "https://example.com/pp-update.json" \
  --anchor-data-hash "a1b2c3d4..." \
  --deposit 100000000000 \
  --return-addr "addr_test1qz..." \
  --protocol-parameters-update pp-changes.json \
  --out-file pp-update.json
```

The `pp-changes.json` file contains the parameter fields to change:

```json
{
  "txFeePerByte": 44,
  "txFeeFixed": 155381,
  "maxBlockBodySize": 90112,
  "maxTxSize": 16384
}
```

### Update Committee

Propose changes to the constitutional committee:

```bash
torsten-cli governance action create-update-committee \
  --anchor-url "https://example.com/committee-update.json" \
  --anchor-data-hash "a1b2c3d4..." \
  --deposit 100000000000 \
  --return-addr "addr_test1qz..." \
  --remove-cc-cold-verification-key-hash "old_member_hash" \
  --add-cc-cold-verification-key-hash "new_member_hash,500" \
  --threshold "2/3" \
  --out-file committee-update.json
```

The `--add-cc-cold-verification-key-hash` uses the format `key_hash,expiry_epoch`.

### Treasury Withdrawal

Propose a withdrawal from the treasury:

```bash
torsten-cli governance action create-treasury-withdrawal \
  --anchor-url "https://example.com/withdrawal.json" \
  --anchor-data-hash "a1b2c3d4..." \
  --deposit 100000000000 \
  --return-addr "addr_test1qz..." \
  --funds-receiving-stake-verification-key-file recipient.vkey \
  --transfer 50000000000 \
  --out-file treasury-withdrawal.json
```

## Hash Anchor Data

Compute the Blake2b-256 hash of an anchor data file:

```bash
# Binary file
torsten-cli governance action hash-anchor-data \
  --file-binary proposal.json

# Text file
torsten-cli governance action hash-anchor-data \
  --file-text proposal.txt
```

## Submitting Governance Actions

Governance actions and votes are submitted as part of transactions. Include the certificate or vote file when building the transaction:

```bash
# Submit a DRep registration
torsten-cli transaction build \
  --tx-in "abc123...#0" \
  --tx-out "addr_test1qz...+5000000" \
  --change-address "addr_test1qp..." \
  --fee 200000 \
  --certificate-file drep-reg.cert \
  --out-file tx.body

torsten-cli transaction sign \
  --tx-body-file tx.body \
  --signing-key-file payment.skey \
  --signing-key-file drep.skey \
  --out-file tx.signed

torsten-cli transaction submit \
  --tx-file tx.signed \
  --socket-path ./node.sock
```
