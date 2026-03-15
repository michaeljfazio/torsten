#!/usr/bin/env bash
# Run Torsten as a relay node on the Cardano preprod testnet.
#
# Usage: ./scripts/run-relay-preprod.sh [--log FILE]

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

if [[ ! -x "$BIN" ]]; then
    echo "Binary not found. Building..."
    cargo build --release
fi

if [[ ! -d "./db-preprod/immutable" ]]; then
    echo "Database empty. Importing Mithril snapshot..."
    "$BIN" mithril-import --network-magic 1 --database-path ./db-preprod
fi

CMD=(
    "$BIN" run
    --config config/preprod-config.json
    --topology config/preprod-topology.json
    --database-path ./db-preprod
    --socket-path ./node.sock
    --host-addr 0.0.0.0
    --port 3001
)

echo "Starting Torsten relay (preprod testnet)..."
echo "Database:  ./db-preprod"
echo "Socket:    ./node.sock"
echo "Metrics:   http://localhost:12798/metrics"

if [[ -n "$LOGFILE" ]]; then
    echo "Logging to: $LOGFILE"
    "${CMD[@]}" 2>&1 | tee "$LOGFILE"
else
    "${CMD[@]}"
fi
