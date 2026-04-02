#!/usr/bin/env bash
# Build and submit a chain of N dependent transactions to a node.
#
# Usage:
#   ./submit-chained-txs.sh --target dugite --chain-length 10
#   ./submit-chained-txs.sh --target haskell --chain-length 10
#
# Prerequisites: cardano-cli (for offline tx building/signing)

set -euo pipefail
cd "$(dirname "$0")/../.."

CCLI="cardano-cli"
TARGET="dugite"
CHAIN_LEN=10
MAGIC=2
ADDR=$(cat ./keys/preview-test/payment.addr)
SKEY="./keys/preview-test/payment.skey"
FEE=200000
WORK_DIR="/tmp/chained-tx-test"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --target) TARGET="$2"; shift 2 ;;
        --chain-length) CHAIN_LEN="$2"; shift 2 ;;
        *) echo "Unknown: $1"; exit 1 ;;
    esac
done

if [[ "$TARGET" == "dugite" ]]; then
    SOCKET="./node.sock"
    QUERY_CLI="./target/release/dugite-cli"
    SUBMIT_CLI="./target/release/dugite-cli"
    echo "=== Submitting $CHAIN_LEN chained txs to DUGITE ==="
elif [[ "$TARGET" == "haskell" ]]; then
    SOCKET="./haskell-node.sock"
    QUERY_CLI="$CCLI conway"
    SUBMIT_CLI="$CCLI conway"
    echo "=== Submitting $CHAIN_LEN chained txs to HASKELL ==="
else
    echo "ERROR: --target must be 'dugite' or 'haskell'"
    exit 1
fi

if [[ ! -S "$SOCKET" ]]; then
    echo "ERROR: Socket not found at $SOCKET. Is the $TARGET node running?"
    exit 1
fi

rm -rf "$WORK_DIR"
mkdir -p "$WORK_DIR/tx"

# Get current slot for TTL
TIP_SLOT=$($QUERY_CLI query tip --socket-path "$SOCKET" --testnet-magic $MAGIC 2>/dev/null | \
    python3 -c "import sys,json; print(json.load(sys.stdin)['slot'])")
TTL=$((TIP_SLOT + 86400))
echo "Current slot: $TIP_SLOT, TTL: $TTL"

# Pick the first UTxO with enough ADA for the chain
MIN_ADA=$((FEE * CHAIN_LEN + 2000000))
echo "Fetching UTxOs (need >= $MIN_ADA lovelace)..."
UTXO_LINE=$($QUERY_CLI query utxo --socket-path "$SOCKET" --testnet-magic $MAGIC --address "$ADDR" 2>/dev/null | \
    grep "lovelace" | awk -v min=$MIN_ADA '$3 >= min {print $1, $2, $3; exit}')

if [[ -z "$UTXO_LINE" ]]; then
    echo "ERROR: No UTxO with enough ADA (need >= $MIN_ADA lovelace)"
    echo "Fund: $ADDR"
    exit 1
fi

read TXHASH TXIX AMOUNT <<< "$UTXO_LINE"
echo "Using UTxO: ${TXHASH}#${TXIX} ($AMOUNT lovelace)"

# Phase 1: Build entire chain offline
echo ""
echo "=== Phase 1: Building $CHAIN_LEN chained transactions ==="

current_txhash="$TXHASH"
current_txix="$TXIX"
current_amount="$AMOUNT"

for i in $(seq 0 $((CHAIN_LEN - 1))); do
    tx_file="$WORK_DIR/tx/tx_${i}"
    output_amount=$((current_amount - FEE))

    if [[ "$output_amount" -lt 1000000 ]]; then
        echo "Chain exhausted at tx $i (output=$output_amount < min_utxo)"
        CHAIN_LEN=$i
        break
    fi

    $CCLI conway transaction build-raw \
        --tx-in "${current_txhash}#${current_txix}" \
        --tx-out "${ADDR}+${output_amount}" \
        --fee "$FEE" \
        --invalid-hereafter "$TTL" \
        --out-file "${tx_file}.raw" 2>/dev/null

    $CCLI conway transaction sign \
        --tx-body-file "${tx_file}.raw" \
        --signing-key-file "$SKEY" \
        --out-file "${tx_file}.signed" 2>/dev/null

    next_txhash=$($CCLI conway transaction txid --tx-file "${tx_file}.signed" 2>/dev/null | python3 -c "import sys,json; d=sys.stdin.read().strip(); print(json.loads(d)['txhash'] if d.startswith('{') else d)")
    echo "  tx[$i]: ${next_txhash} (${output_amount} lovelace)"

    current_txhash="$next_txhash"
    current_txix=0
    current_amount="$output_amount"
done

# Phase 2: Submit in dependency order
echo ""
echo "=== Phase 2: Submitting $CHAIN_LEN transactions to $TARGET ==="
echo "Start: $(date -u +%Y-%m-%dT%H:%M:%SZ)"

submitted=0
failed=0
for i in $(seq 0 $((CHAIN_LEN - 1))); do
    tx_file="$WORK_DIR/tx/tx_${i}.signed"
    tx_hash=$($CCLI conway transaction txid --tx-file "$tx_file" 2>/dev/null | python3 -c "import sys,json; d=sys.stdin.read().strip(); print(json.loads(d)['txhash'] if d.startswith('{') else d)")
    ts=$(date -u +%Y-%m-%dT%H:%M:%SZ)

    result=$($SUBMIT_CLI transaction submit \
        --socket-path "$SOCKET" --testnet-magic $MAGIC --tx-file "$tx_file" 2>&1) || true

    if echo "$result" | grep -qi "success\|submitted\|accepted" || [[ -z "$result" ]]; then
        echo "  [$ts] tx[$i] $tx_hash → OK"
        submitted=$((submitted + 1))
    else
        echo "  [$ts] tx[$i] $tx_hash → FAIL: $result"
        failed=$((failed + 1))
    fi
done

echo ""
echo "=== Results ==="
echo "End: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo "Submitted: $submitted / $CHAIN_LEN"
echo "Failed:    $failed"

sleep 2
echo ""
echo "=== Mempool State ==="
if [[ "$TARGET" == "dugite" ]]; then
    curl -s http://localhost:12798/metrics 2>/dev/null | grep "mempool" | grep -v "^#" || echo "(metrics unavailable)"
else
    curl -s http://localhost:12799/metrics 2>/dev/null | grep -i "mempool" | grep -v "^#" || echo "(metrics unavailable)"
fi
