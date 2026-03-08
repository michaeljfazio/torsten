# Queries

Torsten CLI provides a comprehensive set of queries against a running node via the N2C (Node-to-Client) protocol over a Unix domain socket.

## Chain Tip

Query the current chain tip:

```bash
torsten-cli query tip --socket-path ./node.sock
```

For testnets:

```bash
torsten-cli query tip --socket-path ./node.sock --testnet-magic 2
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
torsten-cli query utxo \
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
torsten-cli query protocol-parameters \
  --socket-path ./node.sock

# Save to file
torsten-cli query protocol-parameters \
  --socket-path ./node.sock \
  --out-file protocol-params.json
```

The output is a JSON object containing all active protocol parameters, including fee settings, execution unit limits, and governance thresholds.

## Stake Distribution

Query the stake distribution across all registered pools:

```bash
torsten-cli query stake-distribution \
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
torsten-cli query stake-address-info \
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
torsten-cli query stake-pools \
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
torsten-cli query pool-params \
  --socket-path ./node.sock \
  --stake-pool-id pool1abc...
```

## Stake Snapshots

Query the mark/set/go stake snapshots:

```bash
torsten-cli query stake-snapshot \
  --socket-path ./node.sock

# Filter by pool
torsten-cli query stake-snapshot \
  --socket-path ./node.sock \
  --stake-pool-id pool1abc...
```

## Governance State (Conway)

Query the overall governance state:

```bash
torsten-cli query gov-state --socket-path ./node.sock
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
torsten-cli query drep-state --socket-path ./node.sock

# Specific DRep by key hash
torsten-cli query drep-state \
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
torsten-cli query committee-state --socket-path ./node.sock
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
torsten-cli query tx-mempool info --socket-path ./node.sock

# Check if a specific transaction is in the mempool
torsten-cli query tx-mempool has-tx \
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

## Leadership Schedule

Compute the leader schedule for a stake pool:

```bash
torsten-cli query leadership-schedule \
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
