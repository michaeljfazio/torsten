#!/usr/bin/env bash
# Run a relay + block producer pair on the Cardano preview testnet.
#
# Relay  : port 3001, metrics 12798, db ./db-preview-relay, socket ./relay.sock
# BP     : port 3002, metrics 12799, db ./db-preview,       socket ./bp.sock
#
# Logs are written to ./logs/relay.log and ./logs/bp.log
# Tail them with:
#   tail -f logs/relay.log
#   tail -f logs/bp.log
#
# Monitor with:
#   ./scripts/dual-node-monitor.sh
#
# Usage: ./scripts/run-dual-node.sh [--no-build]

set -euo pipefail
cd "$(dirname "$0")/.."

SKIP_BUILD=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --no-build) SKIP_BUILD=1; shift ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

BIN=./target/release/dugite-node
KEY_DIR=./keys
LOG_DIR=./logs

# --- Prerequisites -----------------------------------------------------------

if [[ "$SKIP_BUILD" -eq 0 && ! -x "$BIN" ]]; then
    echo "Binary not found. Building..."
    cargo build --release
fi

for f in kes.skey vrf.skey opcert.cert; do
    if [[ ! -f "$KEY_DIR/$f" ]]; then
        echo "ERROR: Missing key: $KEY_DIR/$f"
        exit 1
    fi
done

mkdir -p "$LOG_DIR"

# --- Relay -------------------------------------------------------------------

RELAY_LOG="$LOG_DIR/relay.log"
RELAY_PID_FILE="/tmp/dugite-relay.pid"

echo "Starting relay node..."
echo "  Port    : 3001"
echo "  DB      : ./db-preview-relay"
echo "  Socket  : ./relay.sock"
echo "  Metrics : http://localhost:12798/metrics"
echo "  Log     : $RELAY_LOG"

"$BIN" run \
    --config config/relay-config.json \
    --topology config/relay-topology.json \
    --database-path ./db-preview-relay \
    --socket-path ./relay.sock \
    --host-addr 0.0.0.0 \
    --port 3001 \
    > "$RELAY_LOG" 2>&1 &

RELAY_PID=$!
echo "$RELAY_PID" > "$RELAY_PID_FILE"
echo "  PID: $RELAY_PID"

# --- Block Producer ----------------------------------------------------------

BP_LOG="$LOG_DIR/bp.log"
BP_PID_FILE="/tmp/dugite-bp.pid"

echo ""
echo "Starting block producer..."
echo "  Port    : 3002"
echo "  DB      : ./db-preview"
echo "  Socket  : ./bp.sock"
echo "  Metrics : http://localhost:12799/metrics"
echo "  Log     : $BP_LOG"

"$BIN" run \
    --config config/bp-config.json \
    --topology config/bp-topology.json \
    --database-path ./db-preview \
    --socket-path ./bp.sock \
    --host-addr 0.0.0.0 \
    --port 3002 \
    --shelley-kes-key "$KEY_DIR/kes.skey" \
    --shelley-vrf-key "$KEY_DIR/vrf.skey" \
    --shelley-operational-certificate "$KEY_DIR/opcert.cert" \
    > "$BP_LOG" 2>&1 &

BP_PID=$!
echo "$BP_PID" > "$BP_PID_FILE"
echo "  PID: $BP_PID"

# --- Summary -----------------------------------------------------------------

echo ""
echo "Both nodes running. To monitor:"
echo "  tail -f $RELAY_LOG          # relay logs"
echo "  tail -f $BP_LOG             # bp logs"
echo "  ./scripts/dual-node-monitor.sh  # metrics dashboard"
echo ""
echo "To stop:"
echo "  kill \$(cat $RELAY_PID_FILE) \$(cat $BP_PID_FILE)"
echo ""

# Wait for either process to exit and report
wait -n "$RELAY_PID" "$BP_PID" 2>/dev/null || true

if ! kill -0 "$RELAY_PID" 2>/dev/null; then
    echo "RELAY exited unexpectedly (PID $RELAY_PID). Check $RELAY_LOG"
fi
if ! kill -0 "$BP_PID" 2>/dev/null; then
    echo "BP exited unexpectedly (PID $BP_PID). Check $BP_LOG"
fi

# Keep script alive — kill it to stop both nodes
echo "Script waiting. Press Ctrl+C or kill $$ to stop both nodes."
trap "echo 'Stopping...'; kill $RELAY_PID $BP_PID 2>/dev/null; exit 0" INT TERM

while kill -0 "$RELAY_PID" 2>/dev/null && kill -0 "$BP_PID" 2>/dev/null; do
    sleep 5
done
