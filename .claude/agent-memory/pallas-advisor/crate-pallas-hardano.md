---
name: crate-pallas-hardano
description: pallas-hardano ImmutableDB storage format reading; overlap with dugite's Mithril import
type: reference
---

# pallas-hardano (v1.0.0-alpha.5)

## Overview

Description: "Interoperability with implementation-specific artifacts of the Haskell Cardano node". Provides reading of the on-disk storage format used by `cardano-node`.

## Module Structure

```
pallas_hardano::
  display::    // (purpose unclear from surface inspection)
  storage::    // Haskell-compatible ImmutableDB storage reading
    immutable:: // Chunk file reading
```

## Storage Module (`pallas_hardano::storage::immutable`)

### Public Types

```rust
pub type Point = // blockchain position (Origin or Specific(slot, hash))
pub type Block = Vec<u8>          // raw block bytes
pub type ChunkName = String       // chunk file identifier (e.g., "00000")
pub type ChunkNameSack = Vec<ChunkName>
pub enum Error { ... }            // storage errors
```

### Public Functions

```rust
pub fn read_blocks(dir: &Path) -> impl Iterator<Item = Result<Block>>
    // Iterator over ALL blocks from genesis

pub fn read_blocks_from_point(dir: &Path, point: &Point)
    -> impl Iterator<Item = Result<Block>>
    // Iterator from specific chain point onward (binary search to find chunk)

pub fn get_tip(dir: &Path) -> Result<Point>
    // Get the latest block's Point (slot + hash)
```

### How It Works

1. **File format**: `.chunk` files numbered sequentially (00000.chunk, 00001.chunk, etc.) in a directory
2. **Immutability**: Last chunk files may not be truly immutable — code accounts for this
3. **Block locating**: Binary search on chunk files by comparing first block slot numbers
4. **Block iteration**: Sequential reads within chunks after locating start point
5. **Decoding**: Uses `MultiEraBlock::decode()` from pallas-traverse for era-agnostic parsing

### File Naming

Chunks are numbered sequentially. The binary search locates the correct chunk by reading the first block of each chunk and comparing slot numbers.

## What pallas-hardano Provides vs dugite Mithril Import

### pallas-hardano provides:
- Reading cardano-node's chunk/primary/secondary file format
- Iterating blocks from arbitrary points
- Getting the current tip

### dugite Mithril import ALSO does (from dugite-node/src/mithril.rs):
- Downloads Mithril snapshots (tar.zst archives)
- Digest verification: SHA256(beacon_hash || file_digests) over sorted chunk/primary/secondary files
- Beacon hash: SHA256(network_name || epoch_BE || immutable_file_number_BE)
- Secondary index parsing: 56-byte entries, NO header, big-endian
- CRC32 block checksum verification from secondary index entries
- Memory-mapped I/O (memmap2) for chunk file parsing
- Bulk import into cardano-lsm via ChainDB::open_for_bulk_import()

### Key difference in parsing approach:
- **pallas-hardano** uses `MultiEraBlock::decode()` — parses block content
- **dugite Mithril** uses secondary index (56-byte entries) to locate blocks by position, then verifies CRC32 checksums before importing raw bytes

Dugite's Mithril import is more complete — it handles the full snapshot workflow including download, digest verification, CRC32 checking, and bulk import. pallas-hardano only handles the file reading/iteration portion.

## Overlap Assessment

**Partial overlap** with dugite's Mithril import path for the chunk file parsing step. However, dugite's implementation is optimized differently (memory-mapped I/O, bulk import, deferred compaction) and handles the full snapshot lifecycle.

## Adoption Recommendation

**IMPLEMENT FROM SCRATCH** (already done). Dugite's Mithril import is more sophisticated than what pallas-hardano provides. The chunk file parsing could theoretically use pallas-hardano, but:

1. Dugite uses secondary index for CRC32 verification (pallas-hardano doesn't)
2. Dugite uses memmap2 for performance (pallas-hardano uses standard file I/O)
3. Dugite's bulk import path is optimized for the 4M+ block case
4. Adding pallas-hardano dependency would save only a small portion of code

**Future consideration**: If pallas-hardano adds secondary index parsing with CRC32 verification, re-evaluate. For now, dugite's implementation is superior for the Mithril use case.
