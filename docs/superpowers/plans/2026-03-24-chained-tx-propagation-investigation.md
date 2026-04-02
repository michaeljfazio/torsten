# Chained Transaction Propagation Investigation

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Diagnose why chained transactions cause an infinite TxSubmission2 message exchange loop when propagating between Dugite and connected peers.

**Architecture:** Isolated two-node testbed — Dugite (block producer) and Haskell cardano-node peered exclusively with each other on preview testnet. A tcpdump sidecar container captures the TCP connection between them (macOS Docker Desktop does not expose container bridge interfaces to the host). Chained transactions are submitted to each node independently to observe propagation behavior differences.

**Tech Stack:** Docker Compose (network isolation), cardano-node 10.4.1 (Haskell), dugite-node (Rust), tcpdump sidecar container + tshark/Wireshark (pcap analysis), cardano-cli + dugite-cli (tx construction/submission), bash scripts.

**Prerequisites:**
- `cardano-cli` installed on host (for building/signing transactions offline)
- `dugite-cli` built (`cargo build --release`)
- Docker Desktop running
- Pool keys in `./keys/preview-test/pool/` (kes.skey, vrf.skey, opcert.cert)
- Payment keys in `./keys/preview-test/` (payment.addr, payment.skey)
- `tshark` installed on host for pcap analysis (or use Wireshark GUI)

**Preliminary Root Cause Hypothesis:** The N2N server's `handle_n2n_txsubmission` in `n2n_server.rs:1694` does not implement blocking semantics for `MsgRequestTxIds(blocking=true)` — it always replies immediately with whatever is in the mempool. The Ouroboros spec requires the server to HOLD the response until at least one new tx is available. This causes the client to receive an empty reply to a blocking request, interpret it as session-ending, and potentially re-establish — creating the observed "flurry that never ends."

---

### Task 1: Create Docker Compose for Isolated Two-Node Testbed

**Files:**
- Create: `scripts/chained-tx-investigation/docker-compose.yml`
- Create: `scripts/chained-tx-investigation/dugite-topology.json`
- Create: `scripts/chained-tx-investigation/haskell-topology.json`
- Create: `scripts/chained-tx-investigation/haskell-config.json`

- [ ] **Step 1: Create the investigation directory**

```bash
mkdir -p scripts/chained-tx-investigation
```

- [ ] **Step 2: Create Dugite topology (peers only with Haskell node)**

Create `scripts/chained-tx-investigation/dugite-topology.json`:
```json
{
  "bootstrapPeers": [],
  "localRoots": [
    {
      "accessPoints": [
        { "address": "haskell-node", "port": 3001 }
      ],
      "advertise": false,
      "trustable": true,
      "valency": 1
    }
  ],
  "publicRoots": [],
  "useLedgerAfterSlot": -1
}
```

`bootstrapPeers` and `publicRoots` are empty — no external peers. `useLedgerAfterSlot: -1` disables ledger peer discovery.

- [ ] **Step 3: Create Haskell node topology (peers only with Dugite)**

Create `scripts/chained-tx-investigation/haskell-topology.json`:
```json
{
  "bootstrapPeers": [],
  "localRoots": [
    {
      "accessPoints": [
        { "address": "dugite-node", "port": 3001 }
      ],
      "advertise": false,
      "trustable": true,
      "valency": 1
    }
  ],
  "publicRoots": [],
  "useLedgerAfterSlot": -1
}
```

- [ ] **Step 4: Create Haskell node config**

Create `scripts/chained-tx-investigation/haskell-config.json` with full preview config including trace options for tx submission debugging:

```json
{
  "AlonzoGenesisFile": "/config/preview-alonzo-genesis.json",
  "AlonzoGenesisHash": "7e94a15f55d1e82d10f09203fa1d40f8eede58fd8066542cf6566008068ed874",
  "ByronGenesisFile": "/config/preview-byron-genesis.json",
  "ByronGenesisHash": "81cf23542e33d64c541699926c2b5e6e9c286583f0c8a3fb5f22ea7b352dd174",
  "ConwayGenesisFile": "/config/preview-conway-genesis.json",
  "ShelleyGenesisFile": "/config/preview-shelley-genesis.json",
  "ShelleyGenesisHash": "363498d1024f84bb39d3fa9593ce391483cb40d479b87233f868d6e57c3a400d",
  "EnableP2P": true,
  "NetworkMagic": 2,
  "Protocol": "Cardano",
  "RequiresNetworkMagic": "RequiresMagic",
  "minSeverity": "Debug",
  "hasPrometheus": ["0.0.0.0", 12798],
  "setupScribes": [
    {
      "scKind": "StdoutSK",
      "scFormat": "ScJson",
      "scName": "stdout"
    }
  ],
  "defaultScribes": [["StdoutSK", "stdout"]],
  "TraceMempool": true,
  "TraceBlockFetchDecisions": true,
  "TraceTxSubmissionProtocol": true,
  "TraceTxInbound": true,
  "TraceTxOutbound": true,
  "TargetNumberOfActivePeers": 1,
  "TargetNumberOfEstablishedPeers": 1,
  "TargetNumberOfKnownPeers": 1,
  "TargetNumberOfRootPeers": 1
}
```

Note: Genesis file paths use absolute container paths matching the volume mounts in docker-compose.yml. Peer targets set to 1 since we only have one peer.

- [ ] **Step 5: Create docker-compose.yml**

Create `scripts/chained-tx-investigation/docker-compose.yml`.

Key design decisions:
- IPC volumes use **bind mounts** (not named volumes) so sockets are accessible from the host for tx submission
- A **tcpdump sidecar** container captures traffic on the shared network (required on macOS where Docker bridge interfaces aren't exposed to the host)
- Pcap output goes to a bind-mounted directory for host-side analysis

```yaml
networks:
  cardano-isolated:
    driver: bridge

services:
  dugite-node:
    build:
      context: ../..
      dockerfile: Dockerfile
    container_name: dugite-node
    hostname: dugite-node
    networks:
      - cardano-isolated
    ports:
      - "3001:3001"
      - "12798:12798"
    volumes:
      - dugite-db:/opt/dugite/db
      - ./ipc/dugite:/opt/dugite/ipc
      - ./dugite-topology.json:/opt/dugite/config/topology-override.json:ro
      - ../../keys/preview-test/pool:/opt/dugite/keys:ro
    environment:
      - RUST_LOG=info,dugite_network=debug
    command: >
      run
      --config /opt/dugite/config/preview-config.json
      --topology /opt/dugite/config/topology-override.json
      --database-path /opt/dugite/db
      --socket-path /opt/dugite/ipc/node.sock
      --host-addr 0.0.0.0
      --port 3001
      --shelley-kes-key /opt/dugite/keys/kes.skey
      --shelley-vrf-key /opt/dugite/keys/vrf.skey
      --shelley-operational-certificate /opt/dugite/keys/opcert.cert

  haskell-node:
    image: ghcr.io/intersectmbo/cardano-node:10.4.1
    container_name: haskell-node
    hostname: haskell-node
    networks:
      - cardano-isolated
    ports:
      - "3002:3001"
      - "12799:12798"
    volumes:
      - haskell-db:/data/db
      - ./ipc/haskell:/ipc
      - ./haskell-topology.json:/config/topology.json:ro
      - ./haskell-config.json:/config/config.json:ro
      - ../../config/preview-shelley-genesis.json:/config/preview-shelley-genesis.json:ro
      - ../../config/preview-byron-genesis.json:/config/preview-byron-genesis.json:ro
      - ../../config/preview-alonzo-genesis.json:/config/preview-alonzo-genesis.json:ro
      - ../../config/preview-conway-genesis.json:/config/preview-conway-genesis.json:ro
    environment:
      - CARDANO_NODE_SOCKET_PATH=/ipc/node.socket
    command: >
      run
      --config /config/config.json
      --topology /config/topology.json
      --database-path /data/db
      --socket-path /ipc/node.socket
      --host-addr 0.0.0.0
      --port 3001

  # Tcpdump sidecar — captures all N2N traffic between the two nodes.
  # Required on macOS where Docker Desktop runs containers in a Linux VM
  # and the bridge interface is not accessible from the host.
  tcpdump:
    image: nicolaka/netshoot
    container_name: tcpdump-sidecar
    networks:
      - cardano-isolated
    volumes:
      - ./captures:/captures
    command: >
      tcpdump -i any -w /captures/n2n-traffic.pcap
      tcp port 3001
    cap_add:
      - NET_ADMIN
      - NET_RAW
    depends_on:
      - dugite-node
      - haskell-node

volumes:
  dugite-db:
  haskell-db:
```

- [ ] **Step 6: Create IPC and captures directories**

```bash
mkdir -p scripts/chained-tx-investigation/ipc/dugite
mkdir -p scripts/chained-tx-investigation/ipc/haskell
mkdir -p scripts/chained-tx-investigation/captures
```

Add `.gitkeep` files so git tracks the empty directories:
```bash
touch scripts/chained-tx-investigation/ipc/dugite/.gitkeep
touch scripts/chained-tx-investigation/ipc/haskell/.gitkeep
touch scripts/chained-tx-investigation/captures/.gitkeep
```

- [ ] **Step 7: Verify docker-compose syntax**

```bash
cd scripts/chained-tx-investigation && docker compose config
```

Expected: Valid YAML, no errors.

- [ ] **Step 8: Commit infrastructure files**

```bash
git add scripts/chained-tx-investigation/
git commit -m "chore: add isolated two-node testbed for chained tx investigation"
```

---

### Task 2: Sync Both Nodes to Preview Tip

Both nodes need to be at the preview tip before we can submit transactions. Since they only peer with each other in isolation, we bootstrap them first:
- **Dugite**: Mithril snapshot import (~2 minutes for preview)
- **Haskell**: Start with a temporary topology that includes an IOG bootstrap peer. Preview testnet sync from genesis takes **4-8 hours**. Alternatively, use `mithril-client` CLI to download a Mithril snapshot directly into the Haskell DB volume.

- [ ] **Step 1: Create bootstrap script**

Create `scripts/chained-tx-investigation/bootstrap.sh`:
```bash
#!/usr/bin/env bash
# Bootstrap both nodes to preview tip, then switch to isolated peering.
#
# Dugite: Mithril snapshot import (~2 min)
# Haskell: Temporary IOG bootstrap peer (4-8 hours for preview sync)
#
# For faster Haskell bootstrap, manually download a Mithril snapshot:
#   mithril-client cardano-db download latest \
#     --download-dir ./ipc/haskell-db \
#     --genesis-verification-key $(wget -q -O - https://raw.githubusercontent.com/input-output-hk/mithril/main/mithril-infra/configuration/release-preprod/genesis.vkey)
set -euo pipefail
cd "$(dirname "$0")"

# Create IPC directories if they don't exist
mkdir -p ipc/dugite ipc/haskell captures

echo "=== Phase 1: Bootstrap Dugite with Mithril ==="
docker compose run --rm dugite-node mithril-import \
    --network-magic 2 \
    --database-path /opt/dugite/db

echo ""
echo "=== Phase 2: Bootstrap Haskell with IOG peer ==="
echo "Starting Haskell node with temporary IOG bootstrap topology..."
echo "NOTE: Preview testnet sync from genesis takes 4-8 hours."
echo ""

# Create temporary bootstrap topology for Haskell
cat > /tmp/haskell-bootstrap-topology.json <<'TOPO'
{
  "bootstrapPeers": [
    { "address": "preview-node.play.dev.cardano.org", "port": 3001 }
  ],
  "localRoots": [],
  "publicRoots": [
    {
      "accessPoints": [
        { "address": "preview-node.play.dev.cardano.org", "port": 3001 }
      ],
      "advertise": false
    }
  ],
  "useLedgerAfterSlot": 102729600
}
TOPO

# Start Haskell with bootstrap topology using docker run (not compose run)
docker run -d \
    --name haskell-bootstrap \
    --network chained-tx-investigation_cardano-isolated \
    -v "$(pwd)/ipc/haskell:/ipc" \
    -v haskell-db:/data/db \
    -v /tmp/haskell-bootstrap-topology.json:/config/topology.json:ro \
    -v "$(pwd)/haskell-config.json:/config/config.json:ro" \
    -v "$(cd ../.. && pwd)/config/preview-shelley-genesis.json:/config/preview-shelley-genesis.json:ro" \
    -v "$(cd ../.. && pwd)/config/preview-byron-genesis.json:/config/preview-byron-genesis.json:ro" \
    -v "$(cd ../.. && pwd)/config/preview-alonzo-genesis.json:/config/preview-alonzo-genesis.json:ro" \
    -v "$(cd ../.. && pwd)/config/preview-conway-genesis.json:/config/preview-conway-genesis.json:ro" \
    -p 3002:3001 -p 12799:12798 \
    ghcr.io/intersectmbo/cardano-node:10.4.1 \
    run --config /config/config.json --topology /config/topology.json \
    --database-path /data/db --socket-path /ipc/node.socket \
    --host-addr 0.0.0.0 --port 3001

echo ""
echo "Haskell node started. Monitor sync progress:"
echo "  docker logs -f haskell-bootstrap"
echo "  curl -s http://localhost:12799/metrics | grep cardano_node_metrics_blockNum"
echo ""
echo "Once synced, stop bootstrap and start isolated testbed:"
echo "  docker rm -f haskell-bootstrap"
echo "  ./start-isolated.sh"
```

- [ ] **Step 2: Create isolated start script**

Create `scripts/chained-tx-investigation/start-isolated.sh`:
```bash
#!/usr/bin/env bash
# Start both nodes in isolated mode (peering only with each other).
set -euo pipefail
cd "$(dirname "$0")"

# Stop any bootstrap containers
docker rm -f haskell-bootstrap 2>/dev/null || true
docker compose down 2>/dev/null || true

echo "=== Starting Isolated Two-Node Testbed ==="
docker compose up -d

echo ""
echo "Dugite N2N:     localhost:3001"
echo "Dugite metrics: http://localhost:12798/metrics"
echo "Dugite socket:  $(pwd)/ipc/dugite/node.sock"
echo ""
echo "Haskell N2N:     localhost:3002"
echo "Haskell metrics: http://localhost:12799/metrics"
echo "Haskell socket:  $(pwd)/ipc/haskell/node.socket"
echo ""
echo "Tcpdump capture: $(pwd)/captures/n2n-traffic.pcap"
echo ""
echo "Logs: docker compose logs -f"
```

- [ ] **Step 3: Make scripts executable and commit**

```bash
chmod +x scripts/chained-tx-investigation/bootstrap.sh
chmod +x scripts/chained-tx-investigation/start-isolated.sh
git add scripts/chained-tx-investigation/
git commit -m "chore: add bootstrap and isolated start scripts for tx investigation"
```

---

### Task 3: Create Chained Transaction Builder Script

Build a deterministic chain of N dependent transactions offline using `cardano-cli`, then submit them to a specific node via its Unix socket. Each tx spends the output of the previous tx.

**Files:**
- Create: `scripts/chained-tx-investigation/submit-chained-txs.sh`

- [ ] **Step 1: Create the chained tx submission script**

Create `scripts/chained-tx-investigation/submit-chained-txs.sh`:

```bash
#!/usr/bin/env bash
# Build and submit a chain of N dependent transactions to a node.
#
# Usage:
#   ./submit-chained-txs.sh --target dugite --chain-length 10
#   ./submit-chained-txs.sh --target haskell --chain-length 10
#
# Prerequisites: cardano-cli (for offline tx building/signing)
#
# The script builds all txs offline first (chaining outputs), then submits
# them in dependency order via the node's Unix socket.

set -euo pipefail
cd "$(dirname "$0")"
PROJECT_ROOT="../.."

CCLI="cardano-cli"
TARGET="dugite"
CHAIN_LEN=10
MAGIC=2
ADDR=$(cat "$PROJECT_ROOT/keys/preview-test/payment.addr")
SKEY="$PROJECT_ROOT/keys/preview-test/payment.skey"
FEE=200000
WORK_DIR="/tmp/chained-tx-test"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --target) TARGET="$2"; shift 2 ;;
        --chain-length) CHAIN_LEN="$2"; shift 2 ;;
        *) echo "Unknown: $1"; exit 1 ;;
    esac
done

# Determine socket path and query CLI based on target.
# Sockets are bind-mounted from the containers to ./ipc/<target>/
if [[ "$TARGET" == "dugite" ]]; then
    SOCKET="$(pwd)/ipc/dugite/node.sock"
    QUERY_CLI="$PROJECT_ROOT/target/release/dugite-cli"
    SUBMIT_CLI="$PROJECT_ROOT/target/release/dugite-cli"
    echo "=== Submitting $CHAIN_LEN chained txs to DUGITE ==="
elif [[ "$TARGET" == "haskell" ]]; then
    SOCKET="$(pwd)/ipc/haskell/node.socket"
    QUERY_CLI="$CCLI conway"
    SUBMIT_CLI="$CCLI conway"
    echo "=== Submitting $CHAIN_LEN chained txs to HASKELL ==="
else
    echo "ERROR: --target must be 'dugite' or 'haskell'"
    exit 1
fi

# Verify socket exists
if [[ ! -S "$SOCKET" ]]; then
    echo "ERROR: Socket not found at $SOCKET"
    echo "Is the $TARGET node running? Check: docker compose ps"
    exit 1
fi

rm -rf "$WORK_DIR"
mkdir -p "$WORK_DIR/tx"

# Get current slot for TTL
TIP_SLOT=$($QUERY_CLI query tip --socket-path "$SOCKET" --testnet-magic $MAGIC 2>/dev/null | \
    python3 -c "import sys,json; print(json.load(sys.stdin)['slot'])")
TTL=$((TIP_SLOT + 86400))
echo "Current slot: $TIP_SLOT, TTL: $TTL"

# Pick the first UTxO with enough ADA for the entire chain
MIN_ADA=$((FEE * CHAIN_LEN + 2000000))
echo "Fetching UTxOs (need >= $MIN_ADA lovelace)..."
UTXO_LINE=$($QUERY_CLI query utxo --socket-path "$SOCKET" --testnet-magic $MAGIC --address "$ADDR" 2>/dev/null | \
    grep "lovelace" | awk -v min=$MIN_ADA '$3 >= min {print $1, $2, $3; exit}')

if [[ -z "$UTXO_LINE" ]]; then
    echo "ERROR: No UTxO with enough ADA (need >= $MIN_ADA lovelace)"
    echo "Fund the address: $ADDR"
    exit 1
fi

read TXHASH TXIX AMOUNT <<< "$UTXO_LINE"
echo "Using UTxO: ${TXHASH}#${TXIX} ($AMOUNT lovelace)"

# Phase 1: Build entire chain offline using cardano-cli
echo ""
echo "=== Phase 1: Building $CHAIN_LEN chained transactions offline ==="

current_txhash="$TXHASH"
current_txix="$TXIX"
current_amount="$AMOUNT"

for i in $(seq 0 $((CHAIN_LEN - 1))); do
    tx_file="$WORK_DIR/tx/tx_${i}"
    output_amount=$((current_amount - FEE))

    if [[ "$output_amount" -lt 1000000 ]]; then
        echo "Chain exhausted at tx $i (output=$output_amount < min_utxo)"
        CHAIN_LEN=$i
        break
    fi

    $CCLI conway transaction build-raw \
        --tx-in "${current_txhash}#${current_txix}" \
        --tx-out "${ADDR}+${output_amount}" \
        --fee "$FEE" \
        --invalid-hereafter "$TTL" \
        --out-file "${tx_file}.raw" 2>/dev/null

    $CCLI conway transaction sign \
        --tx-body-file "${tx_file}.raw" \
        --signing-key-file "$SKEY" \
        --out-file "${tx_file}.signed" 2>/dev/null

    next_txhash=$($CCLI conway transaction txid --tx-file "${tx_file}.signed" 2>/dev/null)
    echo "  tx[$i]: ${next_txhash} (${output_amount} lovelace)"

    current_txhash="$next_txhash"
    current_txix=0
    current_amount="$output_amount"
done

# Phase 2: Submit in dependency order with timestamps
echo ""
echo "=== Phase 2: Submitting $CHAIN_LEN transactions to $TARGET ==="
echo "Start: $(date -u +%Y-%m-%dT%H:%M:%S.%3NZ)"

submitted=0
failed=0
for i in $(seq 0 $((CHAIN_LEN - 1))); do
    tx_file="$WORK_DIR/tx/tx_${i}.signed"
    tx_hash=$($CCLI conway transaction txid --tx-file "$tx_file" 2>/dev/null)
    ts=$(date -u +%Y-%m-%dT%H:%M:%S.%3NZ)

    result=$($SUBMIT_CLI transaction submit \
        --socket-path "$SOCKET" --testnet-magic $MAGIC --tx-file "$tx_file" 2>&1) || true

    if echo "$result" | grep -qi "success\|submitted\|accepted" || [[ -z "$result" ]]; then
        echo "  [$ts] tx[$i] $tx_hash → OK"
        submitted=$((submitted + 1))
    else
        echo "  [$ts] tx[$i] $tx_hash → FAIL: $result"
        failed=$((failed + 1))
    fi
done

echo ""
echo "=== Results ==="
echo "End: $(date -u +%Y-%m-%dT%H:%M:%S.%3NZ)"
echo "Submitted: $submitted / $CHAIN_LEN"
echo "Failed:    $failed"

# Check mempool metrics
sleep 2
echo ""
echo "=== Mempool State ==="
if [[ "$TARGET" == "dugite" ]]; then
    curl -s http://localhost:12798/metrics 2>/dev/null | grep "mempool" | grep -v "^#" || echo "(metrics unavailable)"
else
    curl -s http://localhost:12799/metrics 2>/dev/null | grep "mempool\|Mempool" | grep -v "^#" || echo "(metrics unavailable)"
fi
```

- [ ] **Step 2: Make executable and commit**

```bash
chmod +x scripts/chained-tx-investigation/submit-chained-txs.sh
git add scripts/chained-tx-investigation/submit-chained-txs.sh
git commit -m "chore: add chained tx submission script for propagation investigation"
```

---

### Task 4: Create Analysis Scripts

The tcpdump sidecar captures traffic continuously to `captures/n2n-traffic.pcap`. Analysis scripts run on the host using `tshark` to parse the pcap.

**Files:**
- Create: `scripts/chained-tx-investigation/analyze.sh`

- [ ] **Step 1: Create analysis script**

Create `scripts/chained-tx-investigation/analyze.sh`:
```bash
#!/usr/bin/env bash
# Analyze the captured pcap for TxSubmission2 message patterns.
#
# Looks for:
# - Message frequency (are messages repeating rapidly?)
# - Direction analysis (which node is sending more?)
# - Payload patterns (CBOR tags for MsgRequestTxIds/MsgReplyTxIds)
#
# Prerequisites: tshark installed (or use Wireshark GUI on the pcap directly)

set -euo pipefail
cd "$(dirname "$0")"

PCAP="captures/n2n-traffic.pcap"

if [[ ! -f "$PCAP" ]]; then
    echo "ERROR: No capture file at $PCAP"
    echo "The tcpdump sidecar should be writing here. Check:"
    echo "  docker compose logs tcpdump"
    exit 1
fi

echo "=== TxSubmission2 Traffic Analysis ==="
echo "File: $PCAP ($(du -h "$PCAP" | cut -f1))"
echo ""

# Get container IPs for labeling
DUGITE_IP=$(docker inspect dugite-node -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' 2>/dev/null || echo "unknown")
HASKELL_IP=$(docker inspect haskell-node -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' 2>/dev/null || echo "unknown")

echo "Dugite IP: $DUGITE_IP"
echo "Haskell IP: $HASKELL_IP"
echo ""

# Packet rate over time — look for sustained high rates indicating a loop
echo "--- Packet Rate (1-second intervals) ---"
echo "Sustained >10 pkt/s on port 3001 after tx submission indicates a loop."
tshark -r "$PCAP" -q -z io,stat,1,"tcp.port==3001" 2>/dev/null | head -40

echo ""
echo "--- TCP Conversation Summary ---"
tshark -r "$PCAP" -q -z conv,tcp 2>/dev/null | head -20

echo ""
echo "--- First 100 data packets (with CBOR payload for manual tag inspection) ---"
echo "CBOR tags: 0=MsgRequestTxIds, 1=MsgReplyTxIds, 2=MsgRequestTxs, 3=MsgReplyTxs, 4=MsgDone, 6=MsgInit"
echo ""
echo "TIME_REL SRC DST SRC_PORT DST_PORT LEN DATA_HEX(first 32 bytes)"
tshark -r "$PCAP" -T fields \
    -e frame.time_relative \
    -e ip.src -e ip.dst \
    -e tcp.srcport -e tcp.dstport \
    -e tcp.len \
    -e data.data \
    -Y "tcp.len > 0 && tcp.port == 3001" 2>/dev/null | \
    awk '{
        hex = substr($7, 1, 64);
        printf "%-12s %-16s %-16s %-6s %-6s %-6s %s\n", $1, $2, $3, $4, $5, $6, hex
    }' | head -100

echo ""
echo "=== Analysis Complete ==="
echo ""
echo "To inspect in Wireshark GUI: open $PCAP"
echo "To stop capture: docker compose stop tcpdump"
echo "To restart capture: docker compose start tcpdump"
```

- [ ] **Step 2: Make executable and commit**

```bash
chmod +x scripts/chained-tx-investigation/analyze.sh
git add scripts/chained-tx-investigation/
git commit -m "chore: add pcap analysis script for tx propagation investigation"
```

---

### Task 5: Run the Experiment

This task describes the steps to execute the investigation.

- [ ] **Step 1: Build Dugite**

```bash
cargo build --release
```

- [ ] **Step 2: Bootstrap both nodes**

```bash
cd scripts/chained-tx-investigation
./bootstrap.sh
```

Dugite bootstrap completes in ~2 minutes (Mithril). Haskell sync takes 4-8 hours. Monitor:

```bash
# Haskell sync progress
docker logs -f haskell-bootstrap 2>&1 | grep -i "block\|tip\|progress"

# Or check metrics
curl -s http://localhost:12799/metrics | grep cardano_node_metrics_blockNum
```

- [ ] **Step 3: Start isolated testbed (after both nodes are synced)**

```bash
docker rm -f haskell-bootstrap
./start-isolated.sh
```

Wait ~30 seconds, then verify both nodes are peered:
```bash
docker compose logs 2>&1 | grep -i "handshake\|connected\|peer"
```

The tcpdump sidecar starts automatically and writes to `captures/n2n-traffic.pcap`.

- [ ] **Step 4: Submit chained txs to Dugite**

```bash
./submit-chained-txs.sh --target dugite --chain-length 10
```

Immediately watch Dugite logs for the TxSubmission2 "flurry":
```bash
docker compose logs -f dugite-node 2>&1 | grep -i "txsubmission\|mempool"
```

**What to look for:** Rapid repeated `TxSubmission2: sending MsgReplyTxIds` or `TxSubmission2: received MsgRequestTxIds` logs after the txs are submitted. If messages continue at high rate (>1/sec) for more than 30 seconds after all txs should have been exchanged, the loop is confirmed.

- [ ] **Step 5: Wait 60 seconds, then submit chained txs to Haskell**

```bash
sleep 60
./submit-chained-txs.sh --target haskell --chain-length 10
```

Watch Haskell logs for comparison:
```bash
docker compose logs -f haskell-node 2>&1 | grep -i "TxSubmission\|Mempool\|TraceMempoolAdd"
```

- [ ] **Step 6: Let traffic accumulate for 5 minutes, then analyze**

```bash
# Wait for enough data
sleep 300

# Stop tcpdump to flush the pcap
docker compose stop tcpdump

# Analyze
./analyze.sh
```

- [ ] **Step 7: Document findings**

Look for these specific patterns in the capture:

1. **Infinite loop indicator**: Sustained >10 packets/second on port 3001 after tx submission period, with <100ms intervals between MsgRequestTxIds → MsgReplyTxIds pairs
2. **Blocking violation**: Dugite responding to `MsgRequestTxIds(blocking=true)` immediately with empty `MsgReplyTxIds` — visible as a CBOR payload starting with `82 01 80` (array(2), tag 1, array(0)) sent within milliseconds of receiving `84 00 f5` (array(4), tag 0, true)
3. **Re-advertisement**: Same tx ID bytes appearing in multiple MsgReplyTxIds from the same source IP
4. **Asymmetry**: Compare packet rates for Dugite→Haskell vs Haskell→Dugite direction. If Haskell→Dugite is quiet while Dugite→Haskell is noisy, it confirms the Haskell server correctly holds blocking requests while Dugite's doesn't.

---

### Task 6: Fix Blocking Semantics (If Confirmed)

**Files:**
- Modify: `crates/dugite-network/src/n2n_server.rs` — `handle_n2n_txsubmission` (tag 0 handler) and `PeerState` struct
- Modify: `crates/dugite-network/src/n2n_server.rs` — connection handler loop that calls `handle_n2n_txsubmission`

This task is contingent on Task 5 confirming the blocking semantics hypothesis.

**Architecture of the fix:**

The current `handle_n2n_txsubmission` is a synchronous message handler that always returns a response. For blocking semantics, we need:

1. When `blocking=true` and no txs available: return `Ok(None)` to signal "defer response"
2. Store the pending request parameters in `PeerState`
3. The connection handler loop (`handle_n2n_connection` or equivalent) must:
   - When `Ok(None)` is returned for TxSubmission2, NOT close the connection (distinguish from MsgDone)
   - Instead, `tokio::select!` between: (a) next incoming message from peer, (b) mempool notification channel
   - When a mempool notification arrives AND `peer_state.tx_pending_blocking_request.is_some()`, construct and send the deferred MsgReplyTxIds

This requires adding a `tokio::sync::broadcast::Receiver` or `tokio::sync::watch::Receiver` for mempool change notifications to the connection handler.

- [ ] **Step 1: Add `tx_pending_blocking_request` field to `PeerState`**

In `PeerState` struct (around line 500):

```rust
/// Pending blocking MsgRequestTxIds that we deferred because we had no txs.
/// Stores the requested count so we can fulfill it when new txs arrive.
/// None = no pending request. Some(req_count) = waiting for mempool notification.
tx_pending_blocking_request: Option<usize>,
```

Initialize to `None` in `PeerState::new()`.

- [ ] **Step 2: Write a test for blocking request deferral**

Add to existing tests in `n2n_server.rs`:
```rust
#[test]
fn test_txsubmission_blocking_request_empty_mempool_defers() {
    // When a peer sends MsgRequestTxIds(blocking=true) and we have no txs,
    // we must NOT immediately reply with empty MsgReplyTxIds.
    // Instead, return None to indicate the response is deferred.
    let peer_addr: SocketAddr = "127.0.0.1:3001".parse().unwrap();
    let mut peer_state = PeerState::new();
    peer_state.tx_submission_init_sent = true;
    let no_mempool: Option<Arc<dyn MempoolProvider>> = None;

    // MsgRequestTxIds: [0, true (blocking), 0, 3]
    let mut buf = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut buf);
    enc.array(4).unwrap();
    enc.u32(0).unwrap();
    enc.bool(true).unwrap(); // blocking = true
    enc.u32(0).unwrap();     // ack_count
    enc.u32(3).unwrap();     // req_count

    let result =
        handle_n2n_txsubmission(&buf, peer_addr, &mut peer_state, &no_mempool).unwrap();

    // Blocking request with no txs must defer (return None), not send empty reply
    assert!(
        result.is_none(),
        "blocking request with empty mempool must not reply immediately"
    );

    // Verify the pending request is stored
    assert_eq!(
        peer_state.tx_pending_blocking_request,
        Some(3),
        "deferred blocking request must store req_count"
    );
}

#[test]
fn test_txsubmission_non_blocking_request_empty_mempool_replies_immediately() {
    // Non-blocking requests must ALWAYS reply immediately, even with empty list.
    let peer_addr: SocketAddr = "127.0.0.1:3001".parse().unwrap();
    let mut peer_state = PeerState::new();
    peer_state.tx_submission_init_sent = true;
    let no_mempool: Option<Arc<dyn MempoolProvider>> = None;

    // MsgRequestTxIds: [0, false (non-blocking), 0, 3]
    let mut buf = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut buf);
    enc.array(4).unwrap();
    enc.u32(0).unwrap();
    enc.bool(false).unwrap(); // non-blocking
    enc.u32(0).unwrap();
    enc.u32(3).unwrap();

    let result =
        handle_n2n_txsubmission(&buf, peer_addr, &mut peer_state, &no_mempool).unwrap();

    // Non-blocking must reply immediately with empty MsgReplyTxIds
    assert!(
        result.is_some(),
        "non-blocking request must reply immediately even when empty"
    );
}
```

- [ ] **Step 3: Run tests to verify they fail**

```bash
cargo test -p dugite-network -- test_txsubmission_blocking_request_empty_mempool_defers -v
cargo test -p dugite-network -- test_txsubmission_non_blocking_request_empty_mempool_replies -v
```

Expected: First test FAILS (current code always returns Some). Second test PASSES (current behavior is correct for non-blocking).

- [ ] **Step 4: Implement blocking deferral in `handle_n2n_txsubmission`**

In the tag 0 handler (around line 1750), after computing `txs`:

```rust
// After line: let txs: Vec<_> = if let Some(mp) = mempool { ... };

if txs.is_empty() && blocking {
    // Ouroboros TxSubmission2 spec: blocking MsgRequestTxIds must hold
    // the response until at least one new tx is available. Return None
    // to signal the caller to defer and wait for a mempool notification.
    debug!(
        peer = %peer_addr,
        "TxSubmission2: blocking request with no txs, deferring response"
    );
    peer_state.tx_pending_blocking_request = Some(capped_req);
    return Ok(None);
}
```

- [ ] **Step 5: Add `fulfill_pending_blocking_request` helper**

Add a new function that constructs the deferred MsgReplyTxIds when the mempool has new txs:

```rust
/// Fulfill a previously deferred blocking MsgRequestTxIds.
///
/// Called by the connection handler when the mempool notifies of new txs
/// and `peer_state.tx_pending_blocking_request` is `Some(req_count)`.
/// Returns the MsgReplyTxIds segment to send to the peer.
fn fulfill_pending_blocking_request(
    peer_addr: SocketAddr,
    peer_state: &mut PeerState,
    mempool: &Option<Arc<dyn MempoolProvider>>,
) -> Result<Option<Segment>, N2NServerError> {
    let req_count = match peer_state.tx_pending_blocking_request.take() {
        Some(rc) => rc,
        None => return Ok(None),
    };

    // Same logic as the non-deferred path: get txs from mempool, filter inflight
    const MAX_TX_INFLIGHT: usize = 1000;
    let remaining_cap = MAX_TX_INFLIGHT.saturating_sub(peer_state.tx_inflight.len());
    let effective_count = req_count.min(remaining_cap);

    let txs: Vec<_> = if let Some(mp) = mempool {
        let snapshot = mp.snapshot();
        snapshot
            .tx_hashes
            .iter()
            .filter(|h| {
                let bytes = h.as_bytes();
                !peer_state.tx_inflight.iter().any(|inflight| inflight == bytes)
            })
            .take(effective_count)
            .filter_map(|h| mp.get_tx_size(h).map(|size| (*h.as_bytes(), size)))
            .collect()
    } else {
        vec![]
    };

    if txs.is_empty() {
        // Still no txs — re-defer
        peer_state.tx_pending_blocking_request = Some(req_count);
        return Ok(None);
    }

    // Track inflight
    for (tx_hash, _) in &txs {
        if peer_state.tx_inflight.len() < MAX_TX_INFLIGHT {
            peer_state.tx_inflight.push(*tx_hash);
        }
    }

    info!(
        peer = %peer_addr,
        count = txs.len(),
        "TxSubmission2: fulfilling deferred blocking MsgReplyTxIds"
    );

    // Build MsgReplyTxIds: [1, [[tx_id, size], ...]]
    let mut buf = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut buf);
    enc.array(2).map_err(|e| N2NServerError::Protocol(e.to_string()))?;
    enc.u32(1).map_err(|e| N2NServerError::Protocol(e.to_string()))?;
    enc.array(txs.len() as u64).map_err(|e| N2NServerError::Protocol(e.to_string()))?;
    for (tx_hash, size) in &txs {
        enc.array(2).map_err(|e| N2NServerError::Protocol(e.to_string()))?;
        enc.bytes(tx_hash).map_err(|e| N2NServerError::Protocol(e.to_string()))?;
        enc.u32(*size as u32).map_err(|e| N2NServerError::Protocol(e.to_string()))?;
    }

    Ok(Some(Segment {
        transmission_time: 0,
        protocol_id: MINI_PROTOCOL_TXSUBMISSION,
        is_responder: true,
        payload: buf,
    }))
}
```

- [ ] **Step 6: Wire mempool notifications into connection handler**

In the connection handler loop that dispatches to `handle_n2n_txsubmission`:

1. Add a `tokio::sync::watch::Receiver<u64>` parameter (mempool version counter)
2. In the `tokio::select!` loop, add a branch for mempool changes:

```rust
// In the connection handler's select! loop:
_ = mempool_watch.changed() => {
    // Mempool has new txs — check if any peer has a pending blocking request
    if peer_state.tx_pending_blocking_request.is_some() {
        if let Ok(Some(segment)) = fulfill_pending_blocking_request(
            peer_addr, &mut peer_state, &mempool
        ) {
            // Send the deferred MsgReplyTxIds
            send_segment(&mut writer, &segment).await?;
        }
    }
}
```

3. In the mempool crate, add a `tokio::sync::watch::Sender<u64>` that increments on every `add_tx` call. Expose it via `MempoolProvider::subscribe()` or pass it through node wiring.

- [ ] **Step 7: Distinguish deferred vs MsgDone in caller**

Currently `Ok(None)` from `handle_n2n_txsubmission` means "MsgDone — close the protocol." We need to distinguish:

Option A: Change the return type to an enum:
```rust
enum TxSubmissionResponse {
    Reply(Segment),    // Send this segment
    Deferred,          // Blocking request deferred, wait for mempool notification
    Done,              // Peer sent MsgDone, close protocol
}
```

Option B: Keep `Option<Segment>` but use `peer_state.tx_pending_blocking_request.is_some()` to distinguish at the call site. If `result.is_none()` AND `peer_state.tx_pending_blocking_request.is_some()`, it's deferred. Otherwise it's done.

Recommend Option B as it requires fewer changes.

- [ ] **Step 8: Run tests to verify they pass**

```bash
cargo test -p dugite-network -- test_txsubmission_blocking -v
```

Expected: Both new tests PASS.

- [ ] **Step 9: Run full test suite and lint**

```bash
cargo test --all
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

- [ ] **Step 10: Commit the fix**

```bash
git add crates/dugite-network/ crates/dugite-mempool/
git commit -m "$(cat <<'EOF'
fix: implement blocking semantics for TxSubmission2 MsgRequestTxIds

When a peer sends MsgRequestTxIds(blocking=true) and we have no new
transactions, defer the response instead of immediately replying with
an empty MsgReplyTxIds. The Ouroboros spec requires the server to hold
the response until at least one new tx is available.

Changes:
- PeerState gains tx_pending_blocking_request field
- handle_n2n_txsubmission returns None for deferred blocking requests
- New fulfill_pending_blocking_request() wakes deferred peers on mempool change
- Connection handler select! loop listens for mempool notifications
- Mempool exposes watch channel for change notifications

This fixes the infinite TxSubmission2 message exchange loop observed
when chained transactions are propagated between nodes.
EOF
)"
```

---

### Task 7: Re-run Experiment to Verify Fix

- [ ] **Step 1: Rebuild Dugite with fix**

```bash
cargo build --release
```

- [ ] **Step 2: Restart isolated testbed**

```bash
cd scripts/chained-tx-investigation
docker compose down
docker compose build dugite-node
rm -f captures/n2n-traffic.pcap
./start-isolated.sh
```

- [ ] **Step 3: Submit chained txs and capture**

```bash
# Wait for tcpdump sidecar to start
sleep 10

# Submit to Dugite
./submit-chained-txs.sh --target dugite --chain-length 10

# Wait, then submit to Haskell
sleep 60
./submit-chained-txs.sh --target haskell --chain-length 10

# Wait for traffic to settle
sleep 120
```

- [ ] **Step 4: Analyze and confirm the loop is gone**

```bash
docker compose stop tcpdump
./analyze.sh
```

Expected: After txs are exchanged, packet rate drops to near-zero. The blocking MsgRequestTxIds from the peer should NOT receive an immediate empty reply — instead, the connection should go quiet until new txs enter the mempool.

Compare the packet rate graph before and after the fix. Before: sustained >10 pkt/s. After: burst during tx exchange, then drops to <1 pkt/s.

- [ ] **Step 5: Clean up and commit**

```bash
docker compose down
git add scripts/chained-tx-investigation/
git commit -m "chore: verified chained tx propagation fix — loop eliminated"
```
