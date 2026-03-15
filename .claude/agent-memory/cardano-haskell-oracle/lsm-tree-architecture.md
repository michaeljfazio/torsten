---
name: lsm-tree-architecture
description: Complete architecture of the Haskell lsm-tree library used by cardano-node UTxO-HD — merge policy, on-disk format, bloom filters, fence pointers, snapshots, incremental merge scheduler, table handle API
type: reference
---

# lsm-tree Architecture (IntersectMBO/lsm-tree)

Repository: https://github.com/IntersectMBO/lsm-tree
Authors: Well-Typed LLP (Duncan Coutts, Joris Dral, Matthias Heinzel, et al.)
Used by: ouroboros-consensus LedgerDB V2 backend for UTxO-HD

## Core Architecture

**Lazy Levelling merge policy** (only policy supported). Size ratio T=4.
Write buffer = in-memory sorted map (Data.Map.Strict), default 20,000 entries.
Level 1 always tiering, last level always levelling, middle levels tiering.

### Files per run (4 files + checksums):
- `{n}.keyops` — sorted key/op pages (4096-byte aligned)
- `{n}.blobs` — concatenated blob values
- `{n}.filter` — blocked bloom filter
- `{n}.index` — fence pointer index (compact or ordinary)
- `{n}.checksums` — CRC-32C checksums

### Page format (4096 bytes):
1. Directory (8 bytes): N(keys), B(blobs), KO(key offset array position), spare
2. Blob reference bitmap (N bits, padded to 64-bit)
3. Operation type crumb map (2*N bits): 00=Insert, 10=Upsert, 01=Delete
4. Blob reference arrays: 64-bit offsets + 32-bit lengths
5. Key offsets (16-bit per key)
6. Value offsets (16-bit per key+1; N=1 special case: 16+32 bit)
7. Concatenated keys
8. Concatenated values (may overflow to subsequent pages for N=1)

Binary search within page for lookup. Max key size = 4096-44 = 4052 bytes.

### Index types:
- **Ordinary**: vector of last-key-per-page, binary search → PageSpan
- **Compact**: optimized for uniformly distributed keys (hashes), stores 8 bytes per page + clash map

### Bloom filter:
- Blocked bloom filter (Data.BloomFilter.Blocked)
- Salted hash, configurable FPR (default 0.001)
- Bulk query: hash all keys, check against all filters, produce (RunIx, KeyIx) pairs
- On-disk: header (format version 32-bit, hash count 32-bit, bit size 64-bit, salt 64-bit) + bit vector

### Lookup pipeline:
1. Check write buffer (Map.lookup)
2. bloomQueries: batch check keys against all run bloom filters
3. indexSearches: for positive bloom hits, search fence pointer index → PageSpan
4. Batch IO: submit all page reads as IOOps (page-aligned, 4096-byte reads)
5. intraPageLookups: binary search within loaded pages, resolve across runs

## Merge Policy Details

### Level sizing:
- Tiering: maxRunSize = B * T^(level-1)
- Levelling: maxRunSize = B * T^level (T× larger than tiering at same level)
- Default: B=20000, T=4 → Level 1: 20K, Level 2: 80K, Level 3: 320K, Level 4: 1.28M

### addRunToLevels algorithm:
When write buffer flushes → new run at level 1:
1. If level empty: create new level with merge of incoming runs
2. If tiering and incoming run too small: merge with it
3. If tiering and level full (T-1 resident runs): cascade all down to next level
4. If tiering and not full: add run as resident, start new merge
5. If levelling and existing run too large: promote to next level
6. If levelling: merge incoming with existing run

### Incremental merge scheduler:
- Each update supplies 1 NominalCredit to each level
- NominalCredits scaled to MergeCredits: `(nominalCredits * mergeDebt) / nominalDebt`
- Uses widening multiplication (128-bit intermediate via GHC primops)
- MergeCredits accumulated; batched when exceeding CreditThreshold (default: min(mergeBatchSize, writeBufferSize))
- Merge steps: read entries from k-way merge readers, write to RunBuilder
- Last-level merges can drop Deletes; mid-level must preserve them

## Entry Types

```
Entry v b = Insert v | InsertWithBlob v b | Upsert v | Delete
```

Resolution (combine): newer entry wins for Insert/Delete/InsertWithBlob; Upsert merges monoidally with older value.

## Snapshots

- Hard-link based: run files hard-linked from active/ to snapshots/{name}/
- Metadata file: TableConfig + SnapLevels (shape of levels, incoming merges, run numbers)
- Write buffer serialized as keyops+blobs files
- CRC-32C checksum files for integrity
- Restore: hard-link back from snapshot to active, rebuild in-memory state

## No WAL

**IMPORTANT**: lsm-tree does NOT have a write-ahead log. Writes go to the in-memory write buffer (Data.Map.Strict). Durability is achieved through snapshots. The write buffer is lost on crash — the caller (ouroboros-consensus) handles this by replaying blocks from the ImmutableDB since the last snapshot.

## Integration with ouroboros-consensus

- LedgerDB V2 backend (Ouroboros.Consensus.Storage.LedgerDB.V2)
- LedgerTablesHandle wraps lsm-tree Table operations
- UTxO stored as key=TxIn (TxId + TxIx), value=TxOut
- Snapshots taken periodically; on startup, replay from last snapshot
