# Leadership-schedule golden vectors — preview epoch 1268

Reference `--current` leader schedules for SAND pool on the preview testnet,
captured 2026-04-15 against a synced `cardano-node 10.6.2` via Mithril snapshot.

## Provenance

Both files were produced by issuing `query leadership-schedule --current`
against the **same running cardano-node 10.6.2 socket**, using identical
inputs:

| Parameter | Value |
|---|---|
| Network | preview (`--testnet-magic 2`) |
| Pool ID | `da71550ba75cbd51635ac8a30fb960aef9b6ffc4193fd3764da1b88e` (SAND) |
| Epoch | 1268 |
| Current slot | 109,576,792 |
| VRF key | `forTorst/vrf.skey` (not in repo — operator key) |
| Pool active stake (`stakeSet`) | 2,167,067,591,783 lovelace |
| Total active stake (`stakeSet`) | 1,259,333,994,152,147 lovelace |
| Active-slots coefficient `f` | 0.05 |

`haskell-current.json` was produced by `cardano-cli 10.15.0.0`.

`dugite-current.json` was produced by `dugite-cli` (v1.1.0-alpha) with the
`--pool-stake-override` / `--total-active-stake-override` flags, reading the
chain-dependent state (epoch nonce) from the same cardano-node socket.

## What these vectors prove

Both CLIs computed byte-identical 6-slot schedules from identical inputs.
That validates `dugite_consensus::compute_leader_schedule` (VRF leader check,
probability threshold math, slot iteration) against the Haskell reference
implementation at the **computation** level.

## What they do NOT prove

Neither file was produced by running a query against **dugite-node's N2C
server**. Full N2C server wire-compatibility requires `cardano-cli query
leadership-schedule --socket-path <dugite-node.sock>` — tracked in issue #408.
Use `haskell-current.json` as the expected output when that test runs.

## Expected schedule

```
slotNumber    slotTime
109555931     2026-04-15T00:12:11Z
109557282     2026-04-15T00:34:42Z
109562337     2026-04-15T01:58:57Z
109562981     2026-04-15T02:09:41Z
109581540     2026-04-15T07:19:00Z
109626565     2026-04-15T19:49:25Z
```
