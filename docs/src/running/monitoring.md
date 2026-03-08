# Monitoring

Torsten exposes a Prometheus-compatible metrics endpoint for monitoring node health and sync progress.

## Metrics Endpoint

The metrics server runs on **port 12798** by default and responds to any HTTP request with Prometheus exposition format metrics:

```
http://localhost:12798/metrics
```

Example response:

```
# HELP torsten_blocks_received_total Total blocks received from peers
# TYPE torsten_blocks_received_total gauge
torsten_blocks_received_total 1523847

# HELP torsten_blocks_applied_total Total blocks applied to ledger
# TYPE torsten_blocks_applied_total gauge
torsten_blocks_applied_total 1523845

# HELP torsten_slot_number Current slot number
# TYPE torsten_slot_number gauge
torsten_slot_number 142857392

# HELP torsten_block_number Current block number
# TYPE torsten_block_number gauge
torsten_block_number 11283746

# HELP torsten_epoch_number Current epoch number
# TYPE torsten_epoch_number gauge
torsten_epoch_number 512

# HELP torsten_sync_progress_percent Chain sync progress (0-10000, divide by 100 for %)
# TYPE torsten_sync_progress_percent gauge
torsten_sync_progress_percent 9542

# HELP torsten_utxo_count Number of entries in the UTxO set
# TYPE torsten_utxo_count gauge
torsten_utxo_count 15234892

# HELP torsten_mempool_tx_count Number of transactions in the mempool
# TYPE torsten_mempool_tx_count gauge
torsten_mempool_tx_count 42

# HELP torsten_peers_connected Number of connected peers
# TYPE torsten_peers_connected gauge
torsten_peers_connected 8
```

## Available Metrics

| Metric | Type | Description |
|--------|------|-------------|
| `torsten_blocks_received_total` | gauge | Total blocks received from peers |
| `torsten_blocks_applied_total` | gauge | Total blocks successfully applied to the ledger |
| `torsten_transactions_received_total` | gauge | Total transactions received |
| `torsten_transactions_validated_total` | gauge | Total transactions validated |
| `torsten_transactions_rejected_total` | gauge | Total transactions rejected |
| `torsten_peers_connected` | gauge | Number of currently connected peers |
| `torsten_sync_progress_percent` | gauge | Chain sync progress (0-10000; divide by 100 for percentage) |
| `torsten_slot_number` | gauge | Current slot number |
| `torsten_block_number` | gauge | Current block number |
| `torsten_epoch_number` | gauge | Current epoch number |
| `torsten_utxo_count` | gauge | Number of entries in the UTxO set |
| `torsten_mempool_tx_count` | gauge | Number of transactions in the mempool |
| `torsten_mempool_bytes` | gauge | Size of the mempool in bytes |

## Prometheus Configuration

Add the Torsten node as a scrape target in your `prometheus.yml`:

```yaml
scrape_configs:
  - job_name: 'torsten'
    scrape_interval: 15s
    static_configs:
      - targets: ['localhost:12798']
        labels:
          network: 'mainnet'
          node: 'relay-1'
```

## Grafana Dashboard

You can create a Grafana dashboard to visualize Torsten metrics. Key panels to consider:

- **Sync Progress:** `torsten_sync_progress_percent / 100` (percentage)
- **Block Height:** `torsten_block_number`
- **Current Epoch:** `torsten_epoch_number`
- **Blocks/sec throughput:** `rate(torsten_blocks_applied_total[5m])`
- **UTxO Set Size:** `torsten_utxo_count`
- **Mempool Size:** `torsten_mempool_tx_count`
- **Connected Peers:** `torsten_peers_connected`
- **Transaction Rejection Rate:** `rate(torsten_transactions_rejected_total[5m])`

## Console Logging

In addition to the Prometheus endpoint, Torsten logs sync progress to the console every 5 seconds. The log output includes:

- Current slot and block number
- Epoch number
- UTxO count
- Sync percentage
- Blocks-per-second throughput

Example log line:

```
INFO torsten_node::node: slot=142857392 block=11283746 epoch=512 utxo=15234892 sync=95.42% speed=312 blk/s
```

Set the log level via the `RUST_LOG` environment variable:

```bash
RUST_LOG=info torsten-node run ...
```
