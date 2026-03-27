#!/usr/bin/env bash
# Run the Haskell cardano-node as a relay on the Cardano preview testnet,
# peering exclusively with a local Torsten node.
#
# Usage: ./scripts/run-haskell-relay-preview.sh [--db PATH] [--log FILE]
#
# Ports (non-conflicting with Torsten defaults):
#   Node listen:       3002   (Torsten uses 3001)
#   Prometheus legacy: 12799  (Torsten uses 12798)
#   Prometheus tracer: 12797  (haskell-preview-config.json PrometheusSimple suffix)
#   Socket:            ./haskell-node.sock
#
# Networking:
#   - Single peer: Torsten at 127.0.0.1:3001
#   - P2P peer sharing disabled (PeerSharing: false in config)
#   - Ledger peer discovery disabled (useLedgerAfterSlot: -1 in topology)
#   - No public roots; no bootstrap peers
#
# Consensus:
#   - PraosMode (ConsensusMode: PraosMode in config)
#   - ExperimentalProtocolsEnabled: false

set -euo pipefail
cd "$(dirname "$0")/.."

DATABASE_PATH=./db-haskell
LOGFILE=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        --db)  DATABASE_PATH="$2"; shift 2 ;;
        --log) LOGFILE="$2";       shift 2 ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

BIN=cardano-node

if ! command -v "$BIN" &>/dev/null; then
    echo "cardano-node not found in PATH. Install it or add it to PATH."
    exit 1
fi

mkdir -p "$DATABASE_PATH"

CMD=(
    "$BIN" run
    --config           config/haskell-preview-config.json
    --topology         config/haskell-topology.json
    --database-path    "$DATABASE_PATH"
    --socket-path      ./haskell-node.sock
    --host-addr        0.0.0.0
    --port             3002
)

echo "Starting Haskell cardano-node relay (preview testnet)..."
echo "Peer:      127.0.0.1:3001  (Torsten)"
echo "Database:  $DATABASE_PATH"
echo "Socket:    ./haskell-node.sock"
echo "Port:      3002"
echo "Metrics:   http://127.0.0.1:12799/metrics (legacy) / http://127.0.0.1:12797/metrics (tracer)"

if [[ -n "$LOGFILE" ]]; then
    echo "Logging to: $LOGFILE"
    "${CMD[@]}" 2>&1 | tee "$LOGFILE"
else
    "${CMD[@]}"
fi
