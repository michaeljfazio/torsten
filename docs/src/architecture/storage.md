# Storage

Torsten's storage layer is implemented in the `torsten-storage` crate, centered around ChainDB -- a two-tier block storage system modeled after cardano-node's design.

## Storage Architecture

```mermaid
flowchart TD
    CDB[ChainDB] --> VOL[VolatileDB<br/>In-Memory BTreeMap<br/>Last k=2160 blocks]
    CDB --> IMM[ImmutableDB<br/>RocksDB with WriteBatch<br/>Permanent blocks]

    NEW[New Block] -->|add_block| VOL
    VOL -->|flush when > k blocks| IMM

    READ[Block Query] -->|1. check volatile| VOL
    READ -->|2. fallback to immutable| IMM

    ROLL[Rollback] -->|remove from volatile| VOL
```

## ChainDB

ChainDB is the unified interface for block storage. It manages two underlying databases:

- **VolatileDB** -- Recent blocks that may be rolled back
- **ImmutableDB** -- Permanent blocks that are considered final

### Block Lifecycle

1. New blocks arrive from peers and are added to the **VolatileDB**
2. Once a block is more than **k** slots deep (k=2160 for mainnet), it is flushed from the VolatileDB to the **ImmutableDB**
3. Flushed blocks are removed from the VolatileDB

### Block Queries

When querying for a block:
1. The VolatileDB is checked first (fast, in-memory)
2. If not found, the ImmutableDB is consulted (disk-based)

### Slot Range Queries

ChainDB supports querying blocks by slot range:
- VolatileDB uses `BTreeMap::range()` for efficient slot-based lookups
- ImmutableDB uses RocksDB iterators for slot range scanning
- Results from both databases are merged

## VolatileDB

The VolatileDB stores recent blocks in an in-memory `BTreeMap` indexed by slot number. This enables:

- **Fast reads** -- No disk I/O for recent blocks
- **Efficient rollback** -- Blocks can be removed without touching disk
- **Ordered iteration** -- BTreeMap provides natural slot ordering

The VolatileDB holds the last k=2160 blocks (the security parameter). Once a block is deeper than k, it is considered immutable and flushed to the ImmutableDB.

## ImmutableDB

The ImmutableDB uses [RocksDB](https://rocksdb.org/) for persistent block storage. Key design choices:

### WriteBatch

Blocks are written in batches using RocksDB's `WriteBatch` API. When the VolatileDB flushes blocks to the ImmutableDB, it creates a single WriteBatch containing all blocks to be flushed. This provides:

- **Atomicity** -- All blocks in the batch are written together or not at all
- **Performance** -- A single disk sync for multiple blocks
- **Consistency** -- No partial writes on crash

### Key Format

Blocks are stored with their slot number as the key, enabling efficient range scans for slot-based queries.

### Metadata

The ImmutableDB tracks:
- **Tip slot** -- The highest slot stored
- **Tip block number** -- The highest block number stored
- **Tip hash** -- The hash of the highest block

This metadata is persisted to enable tip recovery on restart.

## Tip Recovery

When the node restarts, it recovers its tip from persisted metadata:
1. The ImmutableDB tip is read from RocksDB metadata
2. The VolatileDB starts empty (in-memory state is lost on restart)
3. The node resumes syncing from the ImmutableDB tip

## Ledger State Snapshots

In addition to block storage, the node periodically saves ledger state snapshots to disk. This allows the node to recover its full state (UTxO set, stake distribution, protocol parameters) without replaying all blocks from genesis.

## Disk Layout

```
database-path/
  immutable/       # RocksDB database files
    000001.sst
    000002.sst
    ...
    MANIFEST-000001
    CURRENT
    LOG
```

The VolatileDB has no on-disk representation -- it exists only in memory.

## Performance Considerations

- **Batch size** -- The flush batch size balances memory usage (larger batches use more memory) against write efficiency (fewer disk syncs)
- **RocksDB tuning** -- The default RocksDB configuration works well for most cases. The `release` profile uses `opt-level = 3` and `lto = "thin"` for optimal RocksDB performance
- **Memory usage** -- The VolatileDB holds approximately k blocks in memory. At ~2160 blocks, this is typically a few hundred MB depending on block sizes
