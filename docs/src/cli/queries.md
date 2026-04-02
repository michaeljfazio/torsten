# Queries

Dugite CLI provides a comprehensive set of queries against a running node via the N2C (Node-to-Client) protocol over a Unix domain socket.

## Chain Tip

Query the current chain tip:

```bash
dugite-cli query tip --socket-path ./node.sock
```

For testnets:

```bash
dugite-cli query tip --socket-path ./node.sock --testnet-magic 2
```

Output:

```json
{
    "slot": 73429851,
    "hash": "a1b2c3d4e5f6...",
    "block": 2847392,
    "epoch": 170,
    "era": "Conway",
    "syncProgress": "99.87"
}
```

## UTxO Query

Query UTxOs at a specific address:

```bash
dugite-cli query utxo \
  --address addr_test1qz... \
  --socket-path ./node.sock \
  --testnet-magic 2
```

Output:

```
TxHash#Ix                                                            Datum           Lovelace
------------------------------------------------------------------------------------------------
a1b2c3d4...#0                                                           no            5000000
e5f6a7b8...#1                                                          yes           10000000

Total UTxOs: 2
```

## Protocol Parameters

Query current protocol parameters:

```bash
# Print to stdout
dugite-cli query protocol-parameters \
  --socket-path ./node.sock

# Save to file
dugite-cli query protocol-parameters \
  --socket-path ./node.sock \
  --out-file protocol-params.json
```

The output is a JSON object containing all active protocol parameters, including fee settings, execution unit limits, and governance thresholds.

## Stake Distribution

Query the stake distribution across all registered pools:

```bash
dugite-cli query stake-distribution \
  --socket-path ./node.sock
```

Output:

```
PoolId                                                             Stake (lovelace)   Pledge (lovelace)
----------------------------------------------------------------------------------------------------------
pool1abc...                                                        15234892000000      500000000000
pool1def...                                                         8923451000000      250000000000

Total pools: 3200
```

## Stake Address Info

Query delegation and rewards for a stake address:

```bash
dugite-cli query stake-address-info \
  --address stake_test1uz... \
  --socket-path ./node.sock \
  --testnet-magic 2
```

Output:

```json
[
  {
    "address": "stake_test1uz...",
    "delegation": "pool1abc...",
    "rewardAccountBalance": 5234000
  }
]
```

## Stake Pools

List all registered stake pools with their parameters:

```bash
dugite-cli query stake-pools \
  --socket-path ./node.sock
```

Output:

```
PoolId                                                      Pledge (ADA)    Cost (ADA)   Margin
----------------------------------------------------------------------------------------------------
pool1abc...                                                   500.000000     340.000000    1.00%
pool1def...                                                   250.000000     340.000000    2.50%

Total pools: 3200
```

## Pool Parameters

Query detailed parameters for a specific pool:

```bash
dugite-cli query pool-params \
  --socket-path ./node.sock \
  --stake-pool-id pool1abc...
```

## Stake Snapshots

Query the mark/set/go stake snapshots:

```bash
dugite-cli query stake-snapshot \
  --socket-path ./node.sock

# Filter by pool
dugite-cli query stake-snapshot \
  --socket-path ./node.sock \
  --stake-pool-id pool1abc...
```

## Governance State (Conway)

Query the overall governance state:

```bash
dugite-cli query gov-state --socket-path ./node.sock
```

Output:

```
Governance State (Conway)
========================
Treasury:         1234567890 ADA
Registered DReps: 456
Committee Members: 7
Active Proposals: 12

Proposals:
Type                 TxId     Yes     No  Abstain
----------------------------------------------------
InfoAction           a1b2c3#0    42     3        5
TreasuryWithdrawals  d4e5f6#1    28    12        8
```

## DRep State (Conway)

Query registered DReps:

```bash
# All DReps
dugite-cli query drep-state --socket-path ./node.sock

# Specific DRep by key hash
dugite-cli query drep-state \
  --socket-path ./node.sock \
  --drep-key-hash a1b2c3d4...
```

Output:

```
DRep State (Conway)
===================
Total DReps: 456

Credential Hash                                                    Deposit (ADA)    Epoch
--------------------------------------------------------------------------------------------
a1b2c3d4...                                                                500      412
  Anchor: https://example.com/drep-metadata.json
```

## Committee State (Conway)

Query the constitutional committee:

```bash
dugite-cli query committee-state --socket-path ./node.sock
```

Output:

```
Constitutional Committee State (Conway)
=======================================
Active Members: 7
Resigned Members: 1

Cold Credential                                                    Hot Credential
--------------------------------------------------------------------------------------------------------------------------------------
a1b2c3d4...                                                        e5f6a7b8...

Resigned:
  d4e5f6a7...
```

## Transaction Mempool

Query the node's transaction mempool:

```bash
# Mempool info (size, capacity, tx count)
dugite-cli query tx-mempool info --socket-path ./node.sock

# Check if a specific transaction is in the mempool
dugite-cli query tx-mempool has-tx \
  --socket-path ./node.sock \
  --tx-id a1b2c3d4...
```

Info output:

```
Mempool snapshot at slot 73429851:
  Capacity:     2000000 bytes
  Size:         45320 bytes
  Transactions: 12
```

## Treasury

Query the treasury and reserves:

```bash
dugite-cli query treasury --socket-path ./node.sock
```

Output:

```
Account State
=============
Treasury: 1234567 ADA
Reserves: 9876543 ADA
```

## Constitution (Conway)

Query the current constitution:

```bash
dugite-cli query constitution --socket-path ./node.sock
```

Output:

```
Constitution
============
URL:         https://constitution.gov/hash.json
Data Hash:   a1b2c3d4e5f6...
Script Hash: none
```

## Ratification State (Conway)

Query the ratification state (enacted/expired proposals from the most recent epoch transition):

```bash
dugite-cli query ratify-state --socket-path ./node.sock
```

Output:

```
Ratification State
==================
Enacted proposals: 1
  a1b2c3d4e5f6...#0
Expired proposals: 2
  d4e5f6a7b8c9...#1
  e5f6a7b8c9d0...#0
Delayed:           false
```

## Slot Number

Convert a wall-clock time to a Cardano slot number:

```bash
dugite-cli query slot-number \
  --socket-path ./node.sock \
  --testnet-magic 2 \
  --utc-time "2026-03-20T12:00:00Z"
```

Output:

```
Slot: 73851200
```

This is useful for computing TTL values or verifying that a specific point in time falls within a given epoch.

## KES Period Info

Query KES period information for an operational certificate:

```bash
dugite-cli query kes-period-info \
  --socket-path ./node.sock \
  --op-cert-file opcert.cert
```

Output:

```
KES Period Info
===============
On-chain: yes
Operational certificate counter on-chain: 3
Certificate issue counter: 3

Current KES period: 418
Operational certificate start KES period: 418
KES max evolutions: 62
KES periods remaining: 62

Node start time: 2026-03-19T08:00:00Z
KES key expiry: 2026-09-14T08:00:00Z
```

Use this command to verify that a KES key is current and to determine when rotation is needed.

## Leadership Schedule

Compute the leader schedule for a stake pool:

```bash
dugite-cli query leadership-schedule \
  --vrf-signing-key-file vrf.skey \
  --epoch-nonce a1b2c3d4... \
  --epoch-start-slot 73000000 \
  --epoch-length 432000 \
  --relative-stake 0.001 \
  --active-slot-coeff 0.05
```

Output:

```
Computing leader schedule for epoch starting at slot 73000000...
Epoch length: 432000 slots
Relative stake: 0.001000
Active slot coefficient: 0.05

SlotNo       VRF Output (first 16 bytes)
--------------------------------------------------
73012345     a1b2c3d4e5f6a7b8...
73045678     d4e5f6a7b8c9d0e1...

Total leader slots: 2
Expected: ~22 (f=0.05, stake=0.001000)
```
