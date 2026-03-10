#!/usr/bin/env bash
# Run Torsten as a block producer on the Cardano preview testnet.
#
# Usage: ./scripts/run-bp-preview.sh [--log FILE]
#
# Prerequisites:
#   - Build: cargo build --release
#   - Keys in ./keys/preview-test/pool/ (kes.skey, vrf.skey, opcert.cert, cold.skey)
#   - Database in ./db-preview/ (use mithril-import first if empty)

set -euo pipefail
cd "$(dirname "$0")/.."

LOGFILE=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --log) LOGFILE="$2"; shift 2 ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

BIN=./target/release/torsten-node
KEY_DIR=./keys/preview-test/pool

if [[ ! -x "$BIN" ]]; then
    echo "Binary not found. Building..."
    cargo build --release
fi

for f in kes.skey vrf.skey opcert.cert cold.skey; do
    if [[ ! -f "$KEY_DIR/$f" ]]; then
        echo "Missing key: $KEY_DIR/$f"
        exit 1
    fi
done

CMD=(
    "$BIN" run
    --config config/preview-config.json
    --topology config/preview-topology.json
    --database-path ./db-preview
    --socket-path ./node.sock
    --host-addr 0.0.0.0
    --port 3001
    --shelley-kes-key "$KEY_DIR/kes.skey"
    --shelley-vrf-key "$KEY_DIR/vrf.skey"
    --shelley-operational-certificate "$KEY_DIR/opcert.cert"
    --shelley-cold-key "$KEY_DIR/cold.skey"
)

echo "Starting Torsten block producer (preview testnet)..."
echo "Pool keys: $KEY_DIR"
echo "Database:  ./db-preview"
echo "Socket:    ./node.sock"
echo "Metrics:   http://localhost:12798/metrics"

if [[ -n "$LOGFILE" ]]; then
    echo "Logging to: $LOGFILE"
    "${CMD[@]}" 2>&1 | tee "$LOGFILE"
else
    "${CMD[@]}"
fi
