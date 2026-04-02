#!/usr/bin/env bash
# Start Dugite and Haskell cardano-node side by side on preview testnet.
#
# Dugite:  port 3001, prometheus 12798, socket ./node.sock
# Haskell:  port 3002, prometheus 12799, socket ./haskell-node.sock
#
# Both nodes peer with each other AND with the IOG bootstrap peer.
# Logs go to /tmp/dugite-investigation.log and /tmp/haskell-investigation.log

set -euo pipefail
cd "$(dirname "$0")/../.."

INVESTIGATION_DIR="scripts/chained-tx-investigation"
BIN=./target/release/dugite-node
KEY_DIR=./keys/preview-test/pool

# --- Preflight checks ---
if [[ ! -x "$BIN" ]]; then
    echo "Building dugite-node..."
    cargo build --release
fi

if ! command -v cardano-node &>/dev/null; then
    echo "ERROR: cardano-node not found in PATH"
    exit 1
fi

for f in kes.skey vrf.skey opcert.cert; do
    if [[ ! -f "$KEY_DIR/$f" ]]; then
        echo "ERROR: Missing key: $KEY_DIR/$f"
        exit 1
    fi
done

# --- Stop any existing instances ---
echo "Stopping any running instances..."
pkill -f "dugite-node run" 2>/dev/null || true
pkill -f "cardano-node run" 2>/dev/null || true
sleep 2

# Clean up stale sockets
rm -f ./node.sock ./haskell-node.sock

# --- Start Dugite (block producer) ---
echo ""
echo "=== Starting Dugite (port 3001, metrics 12798) ==="
RUST_LOG=info,dugite_network=debug "$BIN" run \
    --config config/preview-config.json \
    --topology "$INVESTIGATION_DIR/dugite-topology.json" \
    --database-path ./db-preview \
    --socket-path ./node.sock \
    --host-addr 0.0.0.0 \
    --port 3001 \
    --shelley-kes-key "$KEY_DIR/kes.skey" \
    --shelley-vrf-key "$KEY_DIR/vrf.skey" \
    --shelley-operational-certificate "$KEY_DIR/opcert.cert" \
    > /tmp/dugite-investigation.log 2>&1 &
DUGITE_PID=$!
echo "Dugite PID: $DUGITE_PID"
echo "Log: /tmp/dugite-investigation.log"

# --- Start Haskell node ---
echo ""
echo "=== Starting Haskell cardano-node (port 3002, metrics 12799) ==="
cardano-node run \
    --config "$INVESTIGATION_DIR/haskell-config.json" \
    --topology "$INVESTIGATION_DIR/haskell-topology.json" \
    --database-path ./db-preview-haskell/db/db \
    --socket-path ./haskell-node.sock \
    --host-addr 0.0.0.0 \
    --port 3002 \
    > /tmp/haskell-investigation.log 2>&1 &
HASKELL_PID=$!
echo "Haskell PID: $HASKELL_PID"
echo "Log: /tmp/haskell-investigation.log"

# --- Wait for sockets ---
echo ""
echo "Waiting for node sockets..."
for i in $(seq 1 60); do
    READY=0
    [[ -S ./node.sock ]] && READY=$((READY + 1))
    [[ -S ./haskell-node.sock ]] && READY=$((READY + 1))
    if [[ $READY -eq 2 ]]; then
        echo "Both sockets ready after ${i}s"
        break
    fi
    if ! kill -0 $DUGITE_PID 2>/dev/null; then
        echo "ERROR: Dugite died. Check /tmp/dugite-investigation.log"
        tail -20 /tmp/dugite-investigation.log
        exit 1
    fi
    if ! kill -0 $HASKELL_PID 2>/dev/null; then
        echo "ERROR: Haskell node died. Check /tmp/haskell-investigation.log"
        tail -20 /tmp/haskell-investigation.log
        exit 1
    fi
    sleep 1
done

echo ""
echo "=== Both nodes running ==="
echo ""
echo "Dugite:   PID=$DUGITE_PID  socket=./node.sock         log=/tmp/dugite-investigation.log"
echo "Haskell:   PID=$HASKELL_PID  socket=./haskell-node.sock  log=/tmp/haskell-investigation.log"
echo ""
echo "Metrics:"
echo "  Dugite:  http://localhost:12798/metrics"
echo "  Haskell:  http://localhost:12799/metrics"
echo ""
echo "Watch logs:"
echo "  tail -f /tmp/dugite-investigation.log | grep -i txsubmission"
echo "  tail -f /tmp/haskell-investigation.log | grep -i txsubmission"
echo ""
echo "Stop both:  kill $DUGITE_PID $HASKELL_PID"
echo ""
echo "Next: ./scripts/chained-tx-investigation/submit-chained-txs.sh --target dugite --chain-length 10"
