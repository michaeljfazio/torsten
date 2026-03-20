#!/bin/bash
# Stress test: submit 5000 valid transactions to local torsten-node as fast as possible.
#
# Strategy:
#   - Use 50 UTxO chains (from existing 101 UTxOs), 100 chained txs per chain
#   - Each tx sends (input - fee) back to our own address
#   - Build all txs offline first (chaining outputs), then submit in rapid-fire
#
# Requirements: cardano-cli, torsten-cli, jq

set -uo pipefail

CLI="./target/release/torsten-cli"
CCLI="cardano-cli"
SOCKET="./node.sock"
MAGIC=2
ADDR=$(cat ./keys/preview-test/payment.addr)
SKEY="./keys/preview-test/payment.skey"
CHAINS=50        # Number of parallel UTxO chains
CHAIN_LEN=100   # Transactions per chain
TOTAL=$((CHAINS * CHAIN_LEN))
FEE=180000       # Conservative fee (44 * ~500 bytes + 155381 ≈ 177381, round up)
WORK_DIR="/tmp/stress-test-5k"

echo "=== Torsten 5K TX Stress Test ==="
echo "Chains: $CHAINS x $CHAIN_LEN = $TOTAL transactions"
echo "Address: $ADDR"
echo ""

# Clean up previous run
rm -rf "$WORK_DIR"
mkdir -p "$WORK_DIR/tx"

# Get current slot for TTL
TIP_SLOT=$($CLI query tip --socket-path $SOCKET --testnet-magic $MAGIC 2>/dev/null | python3 -c "import sys,json; print(json.load(sys.stdin)['slot'])")
TTL=$((TIP_SLOT + 86400))  # 1 epoch TTL
echo "Current slot: $TIP_SLOT, TTL: $TTL"

# Get UTxOs
echo "Fetching UTxOs..."
$CLI query utxo --socket-path $SOCKET --testnet-magic $MAGIC --address "$ADDR" 2>/dev/null | \
    grep "lovelace" | awk '{print $1, $2, $3}' > "$WORK_DIR/utxos.txt"

UTXO_COUNT=$(wc -l < "$WORK_DIR/utxos.txt" | tr -d ' ')
echo "Available UTxOs: $UTXO_COUNT"

if [ "$UTXO_COUNT" -lt "$CHAINS" ]; then
    echo "ERROR: Need at least $CHAINS UTxOs, have $UTXO_COUNT"
    exit 1
fi

# Phase 1: Build all transaction chains offline
echo ""
echo "=== Phase 1: Building $TOTAL transactions offline ==="
BUILD_START=$(date +%s)

chain_idx=0
while IFS=' ' read -r txhash txix amount; do
    if [ "$chain_idx" -ge "$CHAINS" ]; then
        break
    fi

    # Build a chain of CHAIN_LEN transactions from this UTxO
    current_txhash="$txhash"
    current_txix="$txix"
    current_amount="$amount"

    for i in $(seq 0 $((CHAIN_LEN - 1))); do
        tx_num=$((chain_idx * CHAIN_LEN + i))
        tx_file="$WORK_DIR/tx/tx_${tx_num}"

        output_amount=$((current_amount - FEE))
        if [ "$output_amount" -lt 1000000 ]; then
            echo "WARNING: Chain $chain_idx exhausted at tx $i (output=$output_amount)"
            break
        fi

        # Build transaction body
        $CCLI conway transaction build-raw \
            --tx-in "${current_txhash}#${current_txix}" \
            --tx-out "${ADDR}+${output_amount}" \
            --fee "$FEE" \
            --invalid-hereafter "$TTL" \
            --out-file "${tx_file}.raw" 2>/dev/null

        # Sign
        $CCLI conway transaction sign \
            --tx-body-file "${tx_file}.raw" \
            --signing-key-file "$SKEY" \
            --out-file "${tx_file}.signed" 2>/dev/null

        # Get the tx hash for chaining the next tx
        next_txhash=$($CCLI conway transaction txid --tx-file "${tx_file}.signed" 2>/dev/null)

        # Next tx in chain consumes output 0 of this tx
        current_txhash="$next_txhash"
        current_txix=0
        current_amount="$output_amount"
    done

    chain_idx=$((chain_idx + 1))
    if [ $((chain_idx % 10)) -eq 0 ]; then
        echo "  Built chain $chain_idx/$CHAINS ($(( chain_idx * CHAIN_LEN )) txs)"
    fi
done < "$WORK_DIR/utxos.txt"

BUILD_END=$(date +%s)
BUILT_COUNT=$(ls "$WORK_DIR/tx/"*.signed 2>/dev/null | wc -l | tr -d ' ')
echo "Built $BUILT_COUNT transactions in $((BUILD_END - BUILD_START))s"

# Phase 2: Submit all transactions as fast as possible
echo ""
echo "=== Phase 2: Submitting $BUILT_COUNT transactions ==="
echo "Start: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
SUBMIT_START=$(date +%s%N)

submitted=0
failed=0
for tx_file in $(ls "$WORK_DIR/tx/"*.signed | sort -V); do
    result=$($CLI transaction submit --socket-path $SOCKET --testnet-magic $MAGIC --tx-file "$tx_file" 2>&1) || true
    if echo "$result" | grep -qi "success\|submitted\|accepted"; then
        submitted=$((submitted + 1))
    elif [ -z "$result" ]; then
        # Empty output usually means success
        submitted=$((submitted + 1))
    else
        failed=$((failed + 1))
        if [ "$failed" -le 5 ]; then
            echo "  FAIL tx $(basename $tx_file): $result"
        fi
    fi

    total=$((submitted + failed))
    if [ $((total % 500)) -eq 0 ]; then
        elapsed_ms=$(( ($(date +%s%N) - SUBMIT_START) / 1000000 ))
        rate=$((total * 1000 / (elapsed_ms + 1)))
        echo "  Progress: $total/$BUILT_COUNT (submitted=$submitted, failed=$failed, ${rate} tx/s)"
    fi
done

SUBMIT_END_NS=$(date +%s%N)
ELAPSED_MS=$(( (SUBMIT_END_NS - SUBMIT_START) / 1000000 ))
RATE=$((submitted * 1000 / (ELAPSED_MS + 1)))

echo ""
echo "=== Results ==="
echo "End: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo "Total built:     $BUILT_COUNT"
echo "Submitted OK:    $submitted"
echo "Failed:          $failed"
echo "Elapsed:         ${ELAPSED_MS}ms"
echo "Rate:            ${RATE} tx/s"
echo ""

# Check mempool
sleep 1
echo "=== Post-submission mempool ==="
curl -s http://localhost:12798/metrics | grep "mempool" | grep -v "^#"
