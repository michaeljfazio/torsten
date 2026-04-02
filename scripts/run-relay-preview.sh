#!/usr/bin/env bash
# Run Dugite as a relay node on the Cardano preview testnet.
#
# Usage: ./scripts/run-relay-preview.sh [--log FILE]
#
# Prerequisites:
#   - Build: cargo build --release
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

BIN=./target/release/dugite-node

if [[ ! -x "$BIN" ]]; then
    echo "Binary not found. Building..."
    cargo build --release
fi

# Import Mithril snapshot if database is empty
if [[ ! -d "./db-preview/immutable" ]]; then
    echo "Database empty. Importing Mithril snapshot..."
    "$BIN" mithril-import --network-magic 2 --database-path ./db-preview
fi

CMD=(
    "$BIN" run
    --config config/preview-config.json
    --topology config/preview-topology.json
    --database-path ./db-preview
    --socket-path ./node.sock
    --host-addr 0.0.0.0
    --port 3001
)

echo "Starting Dugite relay (preview testnet)..."
echo "Database:  ./db-preview"
echo "Socket:    ./node.sock"
echo "Metrics:   http://localhost:12798/metrics"

if [[ -n "$LOGFILE" ]]; then
    echo "Logging to: $LOGFILE"
    "${CMD[@]}" 2>&1 | tee "$LOGFILE"
else
    "${CMD[@]}"
fi
