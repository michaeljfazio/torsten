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
| `torsten_blocks_received_total` | counter | Total blocks received from peers |
| `torsten_blocks_applied_total` | counter | Total blocks successfully applied to the ledger |
| `torsten_transactions_received_total` | counter | Total transactions received |
| `torsten_transactions_validated_total` | counter | Total transactions validated |
| `torsten_transactions_rejected_total` | counter | Total transactions rejected |
| `torsten_peers_connected` | gauge | Number of connected peers |
| `torsten_peers_cold` | gauge | Number of cold (known but unconnected) peers |
| `torsten_peers_warm` | gauge | Number of warm (connected, not syncing) peers |
| `torsten_peers_hot` | gauge | Number of hot (actively syncing) peers |
| `torsten_sync_progress_percent` | gauge | Chain sync progress (0-10000; divide by 100 for percentage) |
| `torsten_slot_number` | gauge | Current slot number |
| `torsten_block_number` | gauge | Current block number |
| `torsten_epoch_number` | gauge | Current epoch number |
| `torsten_utxo_count` | gauge | Number of entries in the UTxO set |
| `torsten_mempool_tx_count` | gauge | Number of transactions in the mempool |
| `torsten_mempool_bytes` | gauge | Size of the mempool in bytes |
| `torsten_rollback_count_total` | counter | Total number of chain rollbacks |
| `torsten_blocks_forged_total` | counter | Total blocks forged by this node |
| `torsten_delegation_count` | gauge | Number of active stake delegations |
| `torsten_treasury_lovelace` | gauge | Total lovelace in the treasury |
| `torsten_drep_count` | gauge | Number of registered DReps |
| `torsten_proposal_count` | gauge | Number of active governance proposals |
| `torsten_pool_count` | gauge | Number of registered stake pools |

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

Torsten ships with a pre-built Grafana dashboard at `config/grafana-dashboard.json`. The dashboard covers all node metrics organized into six sections:

- **Overview** -- Sync progress gauge, block height, epoch, slot, connected peers, blocks forged
- **Sync & Throughput** -- Sync progress over time, block apply/receive rate (blk/s), block height, rollbacks
- **Peers** -- Connected peer count over time, peer state breakdown (hot/warm/cold stacked)
- **Mempool & Transactions** -- Mempool tx count, mempool size (bytes), transaction rate (received/validated/rejected)
- **Ledger State** -- UTxO set size, stake delegations, treasury balance (ADA), registered stake pools
- **Governance** -- Registered DReps, active governance proposals
- **Block Production** -- Total blocks forged, block forge rate (blk/h)

### Importing the Dashboard

1. Open Grafana and go to **Dashboards > Import**
2. Click **Upload JSON file** and select `config/grafana-dashboard.json`
3. Select your Prometheus data source when prompted
4. Click **Import**

The dashboard includes an `instance` template variable so you can monitor multiple Torsten nodes (relays + block producer) from a single dashboard. It auto-refreshes every 30 seconds.

### Provisioning

To auto-provision the dashboard, copy it into your Grafana provisioning directory:

```bash
cp config/grafana-dashboard.json /etc/grafana/provisioning/dashboards/torsten.json
```

Add a dashboard provider in `/etc/grafana/provisioning/dashboards/torsten.yaml`:

```yaml
apiVersion: 1
providers:
  - name: Torsten
    folder: Cardano
    type: file
    options:
      path: /etc/grafana/provisioning/dashboards
      foldersFromFilesStructure: false
```

### Quick Start (macOS)

To quickly preview the dashboard locally with Homebrew:

```bash
# Install Prometheus and Grafana
brew install prometheus grafana

# Configure Prometheus to scrape Torsten
cat > /opt/homebrew/etc/prometheus.yml << 'EOF'
global:
  scrape_interval: 5s

scrape_configs:
  - job_name: torsten
    static_configs:
      - targets: ['localhost:12798']
EOF

# Provision the datasource
cat > "$(brew --prefix)/opt/grafana/share/grafana/conf/provisioning/datasources/torsten.yaml" << 'EOF'
apiVersion: 1
datasources:
  - name: Prometheus
    type: prometheus
    access: proxy
    url: http://localhost:9090
    isDefault: true
    uid: DS_PROMETHEUS
EOF

# Provision the dashboard
cat > "$(brew --prefix)/opt/grafana/share/grafana/conf/provisioning/dashboards/torsten.yaml" << 'EOF'
apiVersion: 1
providers:
  - name: Torsten
    folder: Cardano
    type: file
    options:
      path: /opt/homebrew/var/lib/grafana/dashboards
EOF

mkdir -p /opt/homebrew/var/lib/grafana/dashboards
sed 's/${DS_PROMETHEUS}/DS_PROMETHEUS/g' config/grafana-dashboard.json \
  > /opt/homebrew/var/lib/grafana/dashboards/torsten.json

# Start services
brew services start prometheus
brew services start grafana

# Open the dashboard (default login: admin/admin)
open "http://localhost:3000/d/torsten-node/torsten-node"
```

To stop:

```bash
brew services stop prometheus grafana
```

### Key Queries

| Panel | PromQL |
|-------|--------|
| Sync progress | `torsten_sync_progress_percent / 100` |
| Block throughput | `rate(torsten_blocks_applied_total[5m])` |
| Transaction rejection rate | `rate(torsten_transactions_rejected_total[5m])` |
| Treasury balance (ADA) | `torsten_treasury_lovelace / 1e6` |
| Block forge rate (per hour) | `rate(torsten_blocks_forged_total[1h]) * 3600` |

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
