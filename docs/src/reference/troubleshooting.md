# Troubleshooting

Common issues and their solutions when running Dugite.

## Build Issues

### Compilation is slow

The initial build compiles all dependencies from source, which takes several minutes. Subsequent builds are much faster due to cargo caching.

For faster development iteration, use debug builds:

```bash
cargo build  # debug mode, faster compilation
```

Only use `--release` when running against a live network.

## Connection Issues

### Cannot connect to peers

**Symptoms:** Node starts but never receives blocks. Logs show connection failures.

**Possible causes:**

1. **Firewall blocking outbound connections on port 3001.** Ensure outbound TCP connections to port 3001 are allowed.

2. **Incorrect network magic.** Verify the `NetworkMagic` in your config matches the target network:
   - Mainnet: `764824073`
   - Preview: `2`
   - Preprod: `1`

3. **DNS resolution failure.** If topology uses hostnames, ensure DNS is working:
   ```bash
   nslookup preview-node.play.dev.cardano.org
   ```

4. **Stale topology.** Peer addresses may change. Download the latest topology from the [Cardano Operations Book](https://book.world.dev.cardano.org/).

### Handshake failures

**Error:** `Handshake failed: version mismatch`

This usually means the peer does not support the protocol version Dugite is requesting (V14+). Ensure you are connecting to an up-to-date cardano-node (version 10.x+).

## Socket Issues

### Cannot connect to node socket

**Error:** `Cannot connect to node socket './node.sock': No such file or directory`

**Solutions:**

1. **Node is not running.** Start the node first.

2. **Wrong socket path.** Verify the socket path matches what the node was started with:
   ```bash
   dugite-cli query tip --socket-path /path/to/actual/node.sock
   ```

3. **Permission denied.** Ensure the user running the CLI has read/write access to the socket file.

4. **Stale socket file.** If the node crashed, the socket file may remain. Delete it and restart:
   ```bash
   rm ./node.sock
   dugite-node run ...
   ```

### Socket permission denied

**Error:** `Permission denied (os error 13)`

The Unix socket file inherits the permissions of the process that created it. Ensure both the node and CLI processes run as the same user, or adjust the socket file permissions.

## Storage Issues

### Database corruption

**Symptoms:** Node crashes on startup with storage errors.

**Solution:** The safest approach is to delete the database and resync:

```bash
rm -rf ./db-path
dugite-node run ...
```

For faster recovery, use [Mithril snapshot import](../running/mithril.md):

```bash
rm -rf ./db-path
dugite-node mithril-import --network-magic 2 --database-path ./db-path
dugite-node run ...
```

### Disk space

Cardano databases grow continuously. Approximate sizes:

| Network | Database Size |
|---------|--------------|
| Mainnet | 90-140+ GB |
| Preview | 8-15+ GB |
| Preprod | 20-35+ GB |

Monitor disk usage and ensure adequate free space.

## Sync Issues

### Sync is slow

**Possible causes:**

1. **Single peer.** Dugite benefits from multiple peers for block fetching. Ensure your topology includes multiple bootstrap peers or enable ledger-based peer discovery.

2. **Network latency.** The ChainSync protocol has an inherent per-header RTT (~300ms). High-latency connections will reduce throughput.

3. **Slow disk.** Storage performance depends on disk I/O speed. SSDs are strongly recommended. On Linux, enable `io_uring` for improved UTxO storage performance: `cargo build --release --features io-uring`.

4. **CPU-bound during ledger validation.** Block processing includes UTxO validation and Plutus script execution. This is CPU-intensive during sync.

**Recommendation:** Use [Mithril snapshot import](../running/mithril.md) to bypass the initial sync bottleneck entirely.

### Sync stalls

**Symptoms:** Progress percentage stops increasing, no new blocks logged.

**Possible causes:**

1. **Peer disconnected.** The node will reconnect automatically with exponential backoff. Wait a few minutes.

2. **All peers at same height.** If all configured peers are also syncing, they may not have new blocks to serve. Add more peers to the topology.

3. **Resource exhaustion.** Check for out-of-memory or file descriptor limits.

## Memory Issues

### Out of memory

Dugite's memory usage depends on:
- UTxO set size (the largest memory consumer)
- Number of connected peers
- VolatileDB (last k=2160 blocks in memory)

For mainnet, expect memory usage of 8-16 GB depending on sync progress.

If running on a memory-constrained system, ensure adequate swap space is configured.

## Logging

### Increase log verbosity

Use the `RUST_LOG` environment variable:

```bash
# Debug all crates
RUST_LOG=debug dugite-node run ...

# Debug specific crate
RUST_LOG=dugite_network=debug dugite-node run ...

# Trace level (very verbose)
RUST_LOG=trace dugite-node run ...
```

### Log to file

Use the built-in file logging:

```bash
dugite-node run --log-output file --log-dir /var/log/dugite ...
```

Log files are rotated daily by default. See [Logging](../running/logging.md) for rotation options and multi-target output.

## SIGHUP Topology Reload

To update topology without restarting:

```bash
# Edit topology.json
kill -HUP $(pidof dugite-node)
```

The node will log that the topology was reloaded and update the peer manager with the new configuration.

## Block Producer Issues

### Block producer shows ZERO stake

**Cause:** Snapshot loaded before UTxO store was attached, corrupting `pool_stake` values.

**Fix:** Automatic on restart — `rebuild_stake_distribution` runs after UTxO store attachment.

**Verify:** Check the log for `"Block producer: pool stake in 'set' snapshot"` with a non-zero `pool_stake_lovelace` value.

### Node enters reconnection loop after forging

**Cause:** Forged block lost a slot battle and was persisted to ImmutableDB.

**Symptoms:** Log shows `"intersection fell to Origin"` or the node repeatedly reconnects to upstream peers.

**Fix:** The fork recovery mechanism now handles this automatically. If the issue persists, re-import from Mithril:

```bash
dugite-node mithril-import --network-magic <magic> --database-path <path>
```

See [Fork Recovery & ImmutableDB Contamination](../architecture/storage.md#fork-recovery--immutabledb-contamination) for details on how the recovery mechanism works.

## Epoch & State Issues

### Epoch number appears wrong (e.g., epoch 445 instead of 1239)

**Cause:** Snapshot saved with incorrect `epoch_length` defaults (mainnet 432000 instead of preview 86400).

**Fix:** Automatic correction on load — the epoch is recalculated from the tip slot using genesis parameters.

**Log message:** `"Snapshot epoch differs from computed epoch — correcting"`

### VRF verification failures after restart

**Cause:** Epoch nonce in snapshot may be stale if saved with wrong epoch boundaries, or the node is replaying blocks in non-strict mode.

**Fix:** VRF verification is non-fatal during non-strict (initial sync / replay) mode. Once the node reaches the chain tip it enables strict verification and the serialized `epoch_nonce` from the snapshot is used directly — matching Haskell's behavior.

## Getting Help

If you encounter an issue not covered here:

1. Check the [GitHub issues](https://github.com/michaeljfazio/dugite/issues)
2. Open a new issue with:
   - Dugite version (`dugite-node --version`)
   - Operating system
   - Configuration files (redact any sensitive info)
   - Relevant log output
   - Steps to reproduce
