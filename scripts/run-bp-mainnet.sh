#!/usr/bin/env bash
# Run Torsten as a block producer on Cardano mainnet.
#
# Usage: ./scripts/run-bp-mainnet.sh [--log FILE]
#
# Prerequisites:
#   - Build: cargo build --release
#   - Keys in ./keys/mainnet/pool/ (kes.skey, vrf.skey, opcert.cert, cold.skey)
#   - Database in ./db-mainnet/ (use mithril-import first if empty)

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
KEY_DIR=./keys/mainnet/pool

if [[ ! -x "$BIN" ]]; then
    echo "Binary not found. Building..."
    cargo build --release
fi

for f in kes.skey vrf.skey opcert.cert; do
    if [[ ! -f "$KEY_DIR/$f" ]]; then
        echo "Missing key: $KEY_DIR/$f"
        exit 1
    fi
done

CMD=(
    "$BIN" run
    --config config/mainnet-config.json
    --topology config/mainnet-topology.json
    --database-path ./db-mainnet
    --socket-path ./node.sock
    --host-addr 0.0.0.0
    --port 3001
    --shelley-kes-key "$KEY_DIR/kes.skey"
    --shelley-vrf-key "$KEY_DIR/vrf.skey"
    --shelley-operational-certificate "$KEY_DIR/opcert.cert"
)

echo "Starting Torsten block producer (mainnet)..."
echo "Pool keys: $KEY_DIR"
echo "Database:  ./db-mainnet"
echo "Socket:    ./node.sock"
echo "Metrics:   http://localhost:12798/metrics"

if [[ -n "$LOGFILE" ]]; then
    echo "Logging to: $LOGFILE"
    "${CMD[@]}" 2>&1 | tee "$LOGFILE"
else
    "${CMD[@]}"
fi
