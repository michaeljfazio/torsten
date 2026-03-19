# Monitoring

Torsten provides two complementary monitoring tools: a terminal dashboard (`torsten-monitor`) for quick at-a-glance status, and a Prometheus-compatible metrics endpoint for production alerting and dashboards.

## Terminal Dashboard (torsten-monitor)

`torsten-monitor` is a standalone binary that renders a real-time status dashboard in the terminal by polling the node's Prometheus endpoint. It requires no external infrastructure and works over SSH.

```bash
# Monitor a local node (default: http://localhost:12798/metrics)
torsten-monitor

# Monitor a remote node
torsten-monitor --metrics-url http://192.168.1.100:12798/metrics

# Custom refresh interval (default: 2 seconds)
torsten-monitor --refresh-interval 5
```

The dashboard displays four panels:

- **Chain Status** — sync progress, current slot/block/epoch, tip age, GSM state
- **Peers** — out/in/total connection counts, hot/warm/cold breakdown, EWMA latency
- **Performance** — block rate sparkline, replay throughput, transaction counts
- **Governance** — treasury balance, DRep count, active proposals, pool count

Color-coded health indicators (green/yellow/red) reflect tip age and sync progress. The block rate sparkline shows the last 30 data points so you can spot throughput trends at a glance.

Keyboard navigation: `q` to quit, `Tab` to cycle panels, `j`/`k` (vim-style) to scroll within a panel.

---

## Prometheus Metrics Endpoint

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

## Health Endpoint

The metrics server exposes a `/health` endpoint for monitoring node status:

```
GET http://localhost:12798/health
```

Returns JSON with three possible statuses:
- **healthy**: Sync progress >= 99.9%
- **syncing**: Actively catching up to chain tip
- **stalled**: No blocks received for > 5 minutes AND sync < 99%

```json
{
  "status": "healthy",
  "uptime_seconds": 3421,
  "slot": 142857392,
  "block": 11283746,
  "epoch": 512,
  "sync_progress": 99.95,
  "peers": 8,
  "last_block_received": "2026-03-14T12:34:56.789Z"
}
```

## Readiness Endpoint

For Kubernetes readiness probes:

```
GET http://localhost:12798/ready
```

Returns **200 OK** when `sync_progress >= 99.9%`, **503 Service Unavailable** otherwise:

```json
{"ready": true}
```

or:

```json
{"ready": false, "sync_progress": 75.42}
```

## Available Metrics

### Counters

| Metric | Description |
|--------|-------------|
| `torsten_blocks_received_total` | Total blocks received from peers |
| `torsten_blocks_applied_total` | Total blocks successfully applied to the ledger |
| `torsten_transactions_received_total` | Total transactions received |
| `torsten_transactions_validated_total` | Total transactions validated |
| `torsten_transactions_rejected_total` | Total transactions rejected |
| `torsten_rollback_count_total` | Total number of chain rollbacks |
| `torsten_blocks_forged_total` | Total blocks forged by this node |
| `torsten_leader_checks_total` | Total VRF leader checks performed |
| `torsten_leader_checks_not_elected_total` | Leader checks where node was not elected |
| `torsten_forge_failures_total` | Block forge attempts that failed |
| `torsten_blocks_announced_total` | Blocks successfully announced to peers |
| `torsten_n2n_connections_total` | Total N2N (peer-to-peer) connections accepted |
| `torsten_n2c_connections_total` | Total N2C (client) connections accepted |
| `torsten_validation_errors_total{error="..."}` | Transaction validation errors, broken down by error type |
| `torsten_protocol_errors_total{error="..."}` | Protocol-level errors by type (e.g. handshake failures, connection errors) |

### Gauges

| Metric | Description |
|--------|-------------|
| `torsten_peers_connected` | Number of connected peers |
| `torsten_peers_cold` | Number of cold (known but unconnected) peers |
| `torsten_peers_warm` | Number of warm (connected, not syncing) peers |
| `torsten_peers_hot` | Number of hot (actively syncing) peers |
| `torsten_sync_progress_percent` | Chain sync progress (0-10000; divide by 100 for percentage) |
| `torsten_slot_number` | Current slot number |
| `torsten_block_number` | Current block number |
| `torsten_epoch_number` | Current epoch number |
| `torsten_utxo_count` | Number of entries in the UTxO set |
| `torsten_mempool_tx_count` | Number of transactions in the mempool |
| `torsten_mempool_bytes` | Size of the mempool in bytes |
| `torsten_delegation_count` | Number of active stake delegations |
| `torsten_treasury_lovelace` | Total lovelace in the treasury |
| `torsten_drep_count` | Number of registered DReps |
| `torsten_proposal_count` | Number of active governance proposals |
| `torsten_pool_count` | Number of registered stake pools |
| `torsten_uptime_seconds` | Seconds since node startup |
| `torsten_disk_available_bytes` | Available disk space on the database volume |
| `torsten_n2n_connections_active` | Currently active N2N connections |
| `torsten_n2c_connections_active` | Currently active N2C connections |
| `torsten_tip_age_seconds` | Seconds since the tip slot time |
| `torsten_chainsync_idle_seconds` | Seconds since last ChainSync RollForward event |
| `torsten_ledger_replay_duration_seconds` | Duration of last ledger replay in seconds |
| `torsten_mem_resident_bytes` | Resident set size (RSS) in bytes |

### Histograms

| Metric | Buckets (ms) | Description |
|--------|-------------|-------------|
| `torsten_peer_handshake_rtt_ms` | 1, 5, 10, 25, 50, 100, 250, 500, 1000, 2500, 5000, 10000 | Peer N2N handshake round-trip time |
| `torsten_peer_block_fetch_ms` | (same) | Per-block fetch latency |

Histograms expose `_bucket`, `_count`, and `_sum` suffixes for standard Prometheus histogram queries.

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

Torsten ships with a pre-built Grafana dashboard at `config/grafana-dashboard.json`. The dashboard covers all node metrics organized into nine sections:

- **Overview** — Sync progress gauge, block height, epoch, slot, connected peers, blocks forged
- **Node Health** — Uptime, disk available (stat + time series)
- **Sync & Throughput** — Sync progress over time, block apply/receive rate (blk/s), block height, rollbacks
- **Peers** — Connected peer count over time, peer state breakdown (hot/warm/cold stacked)
- **Mempool & Transactions** — Mempool tx count, mempool size (bytes), transaction rate (received/validated/rejected)
- **Ledger State** — UTxO set size, stake delegations, treasury balance (ADA), registered stake pools
- **Governance** — Registered DReps, active governance proposals
- **Block Production** — Total blocks forged, block forge rate (blk/h)
- **Network Latency** — Handshake RTT and block fetch latency percentiles (p50/p95/p99), request counts
- **Validation Errors** — Error breakdown by type (stacked bars), error totals (bar chart)

### Quick Start (Docker)

The fastest way to start a local monitoring stack is with the included script:

```bash
# Start Prometheus + Grafana
./scripts/start-monitoring.sh

# Open the dashboard (admin/admin)
open http://localhost:3000/d/torsten-node/torsten-node

# Check status
./scripts/start-monitoring.sh status

# Stop
./scripts/start-monitoring.sh stop
```

The script starts Prometheus (port 9090) and Grafana (port 3000) as Docker containers, auto-configures the Prometheus datasource, and imports the Torsten dashboard. Prometheus data is persisted in `.monitoring-data/` so metrics survive restarts.

Environment variables for port customization:

| Variable | Default | Description |
|----------|---------|-------------|
| `PROMETHEUS_PORT` | 9090 | Prometheus web UI port |
| `GRAFANA_PORT` | 3000 | Grafana web UI port |
| `TORSTEN_METRICS_PORT` | 12798 | Port where Torsten exposes metrics |

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
| Handshake RTT p95 | `histogram_quantile(0.95, rate(torsten_peer_handshake_rtt_ms_bucket[5m]))` |
| Block fetch latency p95 | `histogram_quantile(0.95, rate(torsten_peer_block_fetch_ms_bucket[5m]))` |
| Validation errors by type | `rate(torsten_validation_errors_total[5m])` |
| Protocol errors by type | `rate(torsten_protocol_errors_total[5m])` |
| Leader election rate | `rate(torsten_leader_checks_total[5m])` |
| Active N2N connections | `torsten_n2n_connections_active` |
| Disk available | `torsten_disk_available_bytes` |

## Console Logging

In addition to the Prometheus endpoint, Torsten logs sync progress to the console every 5 seconds. The log output includes:

- Current slot and block number
- Epoch number
- UTxO count
- Sync percentage
- Blocks-per-second throughput

Example log line:

```
2026-03-12T12:34:56.789Z  INFO torsten_node::node: Syncing progress="95.42%" epoch=512 block=11283746 tip=11300000 remaining=16254 speed="312 blk/s" utxos=15234892
```

Log output can be directed to stdout, file, or systemd journal. See [Logging](./logging.md) for full details on output targets, file rotation, and log level configuration.
