# Mithril Snapshot Import

Syncing a Cardano node from genesis can take a very long time. Dugite supports importing [Mithril](https://mithril.network/)-certified snapshots of the immutable database to drastically reduce initial sync time.

## How It Works

Mithril is a stake-based threshold multi-signature scheme that produces certified snapshots of the Cardano immutable database. These snapshots are verified by Mithril signers (stake pool operators) and made available through Mithril aggregator endpoints.

The import process:

1. Queries the Mithril aggregator for the latest available snapshot
2. Downloads the snapshot archive (compressed with zstandard)
3. Extracts the cardano-node chunk files
4. Parses each block using the pallas CBOR decoder
5. Bulk-imports blocks into Dugite's ImmutableDB (append-only chunk files)

## Usage

```bash
dugite-node mithril-import \
  --network-magic <magic> \
  --database-path <path>
```

### Arguments

| Argument | Default | Description |
|----------|---------|-------------|
| `--network-magic` | `764824073` | Network magic (764824073=mainnet, 2=preview, 1=preprod) |
| `--database-path` | `db` | Path to the database directory |
| `--temp-dir` | system temp | Temporary directory for download and extraction |

### Examples

**Mainnet:**

```bash
dugite-node mithril-import \
  --network-magic 764824073 \
  --database-path ./db-mainnet
```

**Preview testnet:**

```bash
dugite-node mithril-import \
  --network-magic 2 \
  --database-path ./db-preview
```

**Preprod testnet:**

```bash
dugite-node mithril-import \
  --network-magic 1 \
  --database-path ./db-preprod
```

## Mithril Aggregator Endpoints

Dugite automatically selects the correct aggregator for each network:

| Network | Aggregator URL |
|---------|---------------|
| Mainnet | `https://aggregator.release-mainnet.api.mithril.network/aggregator` |
| Preview | `https://aggregator.pre-release-preview.api.mithril.network/aggregator` |
| Preprod | `https://aggregator.release-preprod.api.mithril.network/aggregator` |

## Resume Support

The import process supports resuming interrupted downloads and imports:

- If the snapshot archive has already been downloaded (same size), the download is skipped
- If the archive has already been extracted, extraction is skipped
- Blocks already present in the database are skipped during import

This means you can safely interrupt the import and restart it later.

## After Import

Once the import completes, start the node normally. It will detect the imported blocks and resume syncing from where the snapshot left off:

```bash
dugite-node run \
  --config config.json \
  --topology topology.json \
  --database-path ./db-mainnet \
  --socket-path ./node.sock \
  --host-addr 0.0.0.0 \
  --port 3001
```

## Disk Space Requirements

Mithril snapshots are large. Approximate sizes (which grow over time):

| Network | Compressed Archive | Extracted | Final DB |
|---------|-------------------|-----------|----------|
| Mainnet | ~60-90 GB | ~120-180 GB | ~90-140 GB |
| Preview | ~5-10 GB | ~10-20 GB | ~8-15 GB |
| Preprod | ~15-25 GB | ~30-50 GB | ~20-35 GB |

The temporary directory needs enough space for both the compressed archive and the extracted files. After import, temporary files are automatically cleaned up.

> **Note:** Ensure you have sufficient disk space before starting the import. The `--temp-dir` flag can be used to direct temporary files to a different volume if needed.
