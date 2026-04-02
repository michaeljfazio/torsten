#!/usr/bin/env bash
# Query the node tip via the CLI.
#
# Usage: ./scripts/query-tip.sh [--socket-path PATH] [--testnet-magic N]

set -euo pipefail
cd "$(dirname "$0")/.."

SOCKET="${CARDANO_NODE_SOCKET_PATH:-./node.sock}"
MAGIC="${1:-2}"  # Default to preview

./target/release/dugite-cli query tip \
    --socket-path "$SOCKET" \
    --testnet-magic "$MAGIC"
