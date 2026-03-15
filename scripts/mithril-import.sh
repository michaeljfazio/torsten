#!/usr/bin/env bash
# Import a Mithril snapshot for any network.
#
# Usage:
#   ./scripts/mithril-import.sh preview    # Preview testnet
#   ./scripts/mithril-import.sh preprod    # Preprod testnet
#   ./scripts/mithril-import.sh mainnet    # Mainnet

set -euo pipefail
cd "$(dirname "$0")/.."

NETWORK="${1:-preview}"
BIN=./target/release/torsten-node

if [[ ! -x "$BIN" ]]; then
    echo "Binary not found. Building..."
    cargo build --release
fi

case "$NETWORK" in
    preview)  MAGIC=2;          DB_PATH=./db-preview  ;;
    preprod)  MAGIC=1;          DB_PATH=./db-preprod  ;;
    mainnet)  MAGIC=764824073;  DB_PATH=./db-mainnet  ;;
    *)
        echo "Unknown network: $NETWORK"
        echo "Usage: $0 [preview|preprod|mainnet]"
        exit 1
        ;;
esac

echo "Importing Mithril snapshot for $NETWORK (magic=$MAGIC)..."
echo "Database: $DB_PATH"
"$BIN" mithril-import --network-magic "$MAGIC" --database-path "$DB_PATH"
echo "Import complete."
