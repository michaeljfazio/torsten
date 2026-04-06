---
name: Mithril Snapshot and Ledger Initialization
description: What Mithril snapshots contain, how cardano-node initializes LedgerDB from them, exact directory layout, CBOR snapshot format, and replay logic
type: reference
---

# Mithril Snapshot Content and cardano-node Ledger Initialization

## What a Mithril Snapshot Actually Contains

### Two-Archive System (current Mithril protocol)

Mithril now produces **two separate archives** for a CardanoDB snapshot:

**1. Immutables archive** (`*.tar.zst`)
- All **completed** immutable chunk trios: `{N}.chunk`, `{N}.primary`, `{N}.secondary`
- Covers immutable file numbers 1..N (where N is the certified beacon)
- NO volatile, NO ledger state, NO VolatileDB files
- Mithril-certified (signed by the stake-weighted signer committee)

**2. Ancillary archive** (`*.ancillary.tar.zst`)
- The **last (in-progress) immutable trio**: chunk/primary/secondary for number N+1
- The **two most recent ledger state snapshots** from the `ledger/` directory
  - Format: UTxO-HD in-memory: directory `<slot>/` containing `meta`, `state`, `tables/tvar`
  - Format: Legacy (pre-UTxO-HD): single binary file named `<slot>`
- NOT Mithril-signed (ancillary = trust the aggregator, not the committee)

Source:
- `mithril-aggregator/src/services/snapshotter/compressed_archive_snapshotter.rs`
  `get_files_and_directories_for_ancillary_snapshot` takes last 2 ledger snapshots via `LedgerStateSnapshot::list_all_in_dir`
- `mithril-aggregator/src/artifact_builder/cardano_immutable_files_full.rs`
  `create_immutables_snapshot_archive` takes only completed immutables

### LedgerStateSnapshot Variants (Mithril entity)

```rust
enum LedgerStateSnapshot {
    Legacy { path, slot_number, filename },          // single file
    InMemory { path, slot_number, folder_name },     // directory with 3 files:
                                                     //   meta, state, tables/tvar
}
```

The `InMemory` variant corresponds to cardano-node's UTxO-HD in-memory backend (default in 10.x).

Source: `internal/cardano-node/mithril-cardano-node-internal-database/src/entities/ledger_state_snapshot.rs`

### Our Dugite Mithril Import (legacy API)

Our `mithril.rs` uses the **old `/artifact/snapshots` endpoint** which returns a SINGLE `.tar.zst` that:
- Contains the ImmutableDB chunk files (`immutable/` subdirectory)
- Does NOT contain ledger state (the ledger was included in older Mithril archives but was removed)

Our code explicitly states:
```
// Ledger state is not imported — the node rebuilds it via block replay.
```

We download, extract, move `immutable/` to `<database_path>/immutable/`, then delete stale
`utxo-store/` and `ledger-snapshot*.bin` files.

## How cardano-node Initializes LedgerDB

### Directory Layout

Given `--database-path /db`:

| Path | Contents |
|------|----------|
| `/db/immutable/` | ImmutableDB chunk files (`{N}.chunk`, `{N}.primary`, `{N}.secondary`) |
| `/db/volatile/` | VolatileDB (recent blocks, all forks) |
| `/db/ledger/` | LedgerDB snapshots directory |
| `/db/ledger/<slot>/` | UTxO-HD in-memory snapshot (dir) |
| `/db/ledger/<slot>/state` | ExtLedgerState CBOR (EmptyMK, no UTxO) |
| `/db/ledger/<slot>/tables` | UTxO table data (flat binary) |
| `/db/ledger/<slot>/tables/tvar` | UTxO tvar encoding |
| `/db/ledger/<slot>/meta` | JSON metadata: backend, checksum, tablesCodecVersion |
| `/db/ledger/<slot>` | (Legacy) single file = ExtLedgerState + UTxO (binary) |

Source: `ouroboros-consensus/.../Storage/ChainDB/Impl/Args.hs` lines 203, 207, 214:
```haskell
ImmutableDB.immHasFS = mkImmFS $ RelativeMountPoint "immutable"
VolatileDB.volHasFS  = mkVolFS $ RelativeMountPoint "volatile"
LedgerDB.lgrHasFS   = mkVolFS $ RelativeMountPoint "ledger"
```

`mkImmFS` uses `immutableDbPath srnDatabasePath` as root.
`mkVolFS` uses `nonImmutableDbPath srnDatabasePath` as root.
For `OnePathForAllDbs`, both are the same path.

### LedgerDB Initialization Flow

Source: `ouroboros-consensus/.../Storage/LedgerDB/API.hs` (`initialize` function)

```
ChainDB.openDBInternal
  → ImmutableDB.openDB         (opens immutable files)
  → VolatileDB.openDB          (opens volatile files)
  → LedgerDB.openDB            (the key step)
```

`LedgerDB.openDB` calls `initialize`:

```
initialize(stream=ImmutableDB.streamAPI, replayGoal=immutableDbTipPoint) =
  listSnapshots(ledger/)               // sort by slot descending
  → tryNewestFirst
      for each DiskSnapshot:
          if snapshot.slot > immTip.slot: SKIP (too recent, corrupt)
          initFromSnapshot(snapshot)
              → read ledger/<slot>/state  (CBOR ExtLedgerState EmptyMK)
              → read ledger/<slot>/tables (UTxO table)
              → return (initDb, anchorPoint=snapshot.slot)
          replayStartingWith(stream, from=anchorPoint, to=immTip)
              → for each block in ImmutableDB between anchorPoint and immTip:
                  initReapplyBlock cfg blk db
                      → tickThenReapply (NO validation, trust ImmutableDB)
          return (db, replayed_count)
      if no snapshots found:
          initFromGenesis
              → start with genesis ExtLedgerState
          replayStartingWith(stream, from=Origin, to=immTip)
              → replay ALL blocks from genesis
```

### Critical: initReapplyBlock = tickThenReapply (NOT tickThenApply)

During snapshot replay, blocks are replayed with `tickThenReapply`:
- **NO KES signature check**
- **NO VRF signature check**
- **NO opcert check**
- **NO ledger predicate failures** (STS.ValidateNone)
- Only updates slot-tick state and applies the block body (UTxO, certs, rewards, etc.)

This is safe because ImmutableDB blocks are assumed valid — they are k-final.

### snapshot slot vs ImmutableDB tip

The `DiskSnapshot.dsNumber` = slot number of the ledger state in that snapshot.

Consistency check: if `snapshot.slot > immutableDbTip.slot`, the snapshot is DISCARDED
(it refers to a block that hasn't been finalized into ImmutableDB — possible if node was
killed mid-copy-to-immutable). The next-oldest snapshot is tried.

### Protocol Parameters After Snapshot Start

**Answer: they come from the ledger state in the snapshot, not from genesis.**

The `ExtLedgerState blk EmptyMK` stored in `ledger/<slot>/state` contains the full
`NewEpochState` including `esLState.utxosGovState.cgsProposedPPUpdates` and the
current `curPParams` / `prevPParams`. Everything needed for protocol operation is
already in the ledger state.

Genesis configs (Shelley, Alonzo, Conway genesis) provide the **initial** protocol
parameters (before any PParams update proposals are applied). After the first
epoch boundaries, the ledger state itself tracks all protocol params.

### Snapshot Taking Policy

Default policy (ouroboros-consensus `defaultSnapshotPolicy`):
- Interval: `2 * k` slots (≈ 2 * 2160 = 4320 slots ≈ 72 minutes on mainnet)
- Keep: `DefaultNumOfDiskSnapshots` (2 snapshots retained)
- Triggered by background thread watching for copy-to-immutable events

### What Happens Without Any Ledger Snapshot

If the `ledger/` directory is empty (or all snapshots are too recent):
```
tryNewestFirst [] = initFromGenesis >> replayStartingWith(from=Origin, to=immTip)
```

This replays **all** blocks from genesis to the current ImmutableDB tip. For a
Mithril-imported chain with ~10M blocks on mainnet this would take many hours.

## Implication for Dugite After Mithril Import

Our current flow:
1. Download Mithril snapshot → extract only ImmutableDB chunk files
2. Delete old ledger snapshots and UTxO store
3. Node starts → no ledger snapshots → replays ALL blocks from genesis

This is correct but slow. To match Haskell behavior with ancillary snapshots:
1. Download both immutables and ancillary archives
2. Extract ancillary: place ledger state in `<db>/ledger/<slot>/`
   - `state` (CBOR) + `tables/tvar` + `meta` files
3. Node starts → finds ledger snapshot → replays only from snapshot slot to immTip
   - Typically 0-1 snapshot intervals worth of blocks (≤ ~4320 slots ≈ few minutes)

## Key Source Files

- `ouroboros-consensus/src/ouroboros-consensus/Ouroboros/Consensus/Storage/LedgerDB/API.hs`
  - `initialize`, `replayStartingWith`, `InitLog`, `InitDB`
- `ouroboros-consensus/src/ouroboros-consensus/Ouroboros/Consensus/Storage/LedgerDB/Snapshots.hs`
  - `DiskSnapshot`, `snapshotToDirPath`, `snapshotToStatePath`, `snapshotToTablesPath`
  - `readExtLedgerState`, `writeExtLedgerState`
  - `encodeL`, `decodeLBackwardsCompatible`
  - `snapshotEncodingVersion1 = 1`
- `ouroboros-consensus/src/ouroboros-consensus/Ouroboros/Consensus/Storage/LedgerDB/V2/InMemory.hs`
  - `writeSnapshot` (writes state + tables + meta)
  - `readSnapshot` (reads state + tables + meta)
  - `snapshotToTablesPath = snapshotToDirPath ds </> "tables"`
- `ouroboros-consensus/src/ouroboros-consensus/Ouroboros/Consensus/Storage/ChainDB/Impl.hs`
  - `openDBInternal` — wires ImmutableDB → LedgerDB initialization
- `ouroboros-consensus/src/ouroboros-consensus/Ouroboros/Consensus/Storage/ChainDB/Impl/Args.hs`
  - `RelativeMountPoint "immutable"`, `"volatile"`, `"ledger"`
- `ouroboros-consensus-diffusion/src/ouroboros-consensus-diffusion/Ouroboros/Consensus/Node.hs`
  - `NodeDatabasePaths`, `stdMkChainDbHasFS`, `immutableDbPath`, `nonImmutableDbPath`
- `input-output-hk/mithril: mithril-aggregator/src/services/snapshotter/compressed_archive_snapshotter.rs`
  - `get_files_and_directories_for_ancillary_snapshot`
- `input-output-hk/mithril: internal/cardano-node/mithril-cardano-node-internal-database/src/entities/ledger_state_snapshot.rs`
  - `LedgerStateSnapshot::Legacy`, `LedgerStateSnapshot::InMemory`
