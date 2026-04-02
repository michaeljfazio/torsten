#!/usr/bin/env bash
# Run Dugite as a relay node on Cardano mainnet.
#
# Usage: ./scripts/run-relay-mainnet.sh [--log FILE]
#
# Prerequisites:
#   - Build: cargo build --release
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

BIN=./target/release/dugite-node

if [[ ! -x "$BIN" ]]; then
    echo "Binary not found. Building..."
    cargo build --release
fi

if [[ ! -d "./db-mainnet/immutable" ]]; then
    echo "Database empty. Importing Mithril snapshot (~35 GB, may take 30+ minutes)..."
    "$BIN" mithril-import --network-magic 764824073 --database-path ./db-mainnet
fi

CMD=(
    "$BIN" run
    --config config/mainnet-config.json
    --topology config/mainnet-topology.json
    --database-path ./db-mainnet
    --socket-path ./node.sock
    --host-addr 0.0.0.0
    --port 3001
)

echo "Starting Dugite relay (mainnet)..."
echo "Database:  ./db-mainnet"
echo "Socket:    ./node.sock"
echo "Metrics:   http://localhost:12798/metrics"

if [[ -n "$LOGFILE" ]]; then
    echo "Logging to: $LOGFILE"
    "${CMD[@]}" 2>&1 | tee "$LOGFILE"
else
    "${CMD[@]}"
fi
