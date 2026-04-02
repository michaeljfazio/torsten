#!/bin/bash
# Stress test: 5000 valid transactions via dugite-cli only
#
# Strategy: 50 parallel UTxO chains x 100 chained txs each.
# Each tx sends (input - fee) back to our own address.
# All txs built offline first, then submitted in rapid fire.

set -uo pipefail

CLI="./target/release/dugite-cli"
SOCKET="./node.sock"
MAGIC=2
ADDR=$(cat ./keys/preview-test/payment.addr)
SKEY="./keys/preview-test/payment.skey"
CHAINS=40
CHAIN_LEN=100
TOTAL=$((CHAINS * CHAIN_LEN))
FEE=180000
WORK_DIR="/tmp/stress-test-dugite"

echo "=== Dugite 5K TX Stress Test (dugite-cli only) ==="
echo "Chains: $CHAINS x $CHAIN_LEN = $TOTAL transactions"
echo ""

rm -rf "$WORK_DIR"
mkdir -p "$WORK_DIR/tx"

# Get tip slot for TTL
TIP_SLOT=$($CLI query tip --socket-path $SOCKET --testnet-magic $MAGIC 2>/dev/null | python3 -c "import sys,json; print(json.load(sys.stdin)['slot'])")
TTL=$((TIP_SLOT + 86400))
echo "Tip slot: $TIP_SLOT, TTL: $TTL"

# Get UTxOs
$CLI query utxo --socket-path $SOCKET --testnet-magic $MAGIC --address "$ADDR" 2>/dev/null | \
    grep "lovelace" | awk '{print $1, $2, $3}' > "$WORK_DIR/utxos.txt"

UTXO_COUNT=$(wc -l < "$WORK_DIR/utxos.txt" | tr -d ' ')
echo "UTxOs available: $UTXO_COUNT (need $CHAINS)"

if [ "$UTXO_COUNT" -lt "$CHAINS" ]; then
    echo "ERROR: Not enough UTxOs"
    exit 1
fi

# ── Phase 1: Build all 5000 transactions offline ──
echo ""
echo "=== Phase 1: Building $TOTAL transactions ==="
BUILD_START=$(date +%s)

tx_count=0
chain_idx=0
while IFS=' ' read -r txhash txix amount; do
    [ "$chain_idx" -ge "$CHAINS" ] && break

    cur_hash="$txhash"
    cur_ix="$txix"
    cur_amt="$amount"

    for i in $(seq 0 $((CHAIN_LEN - 1))); do
        n=$((chain_idx * CHAIN_LEN + i))
        tf="$WORK_DIR/tx/tx_$(printf '%05d' $n)"

        out_amt=$((cur_amt - FEE))
        if [ "$out_amt" -lt 1000000 ]; then
            echo "  Chain $chain_idx exhausted at tx $i (only $out_amt left)"
            break
        fi

        # Build
        $CLI transaction build-raw \
            --tx-in "${cur_hash}#${cur_ix}" \
            --tx-out "${ADDR}+${out_amt}" \
            --fee "$FEE" \
            --ttl "$TTL" \
            --out-file "${tf}.raw" 2>/dev/null

        if [ ! -f "${tf}.raw" ]; then
            echo "  ERROR: build failed for chain $chain_idx tx $i"
            break
        fi

        # Sign
        $CLI transaction sign \
            --tx-body-file "${tf}.raw" \
            --signing-key-file "$SKEY" \
            --out-file "${tf}.signed" 2>/dev/null

        if [ ! -f "${tf}.signed" ]; then
            echo "  ERROR: sign failed for chain $chain_idx tx $i"
            break
        fi

        # Get tx hash for next in chain
        next_hash=$($CLI transaction txid --tx-file "${tf}.signed" 2>/dev/null)
        if [ -z "$next_hash" ]; then
            echo "  ERROR: txid failed for chain $chain_idx tx $i"
            break
        fi

        cur_hash="$next_hash"
        cur_ix=0
        cur_amt="$out_amt"
        tx_count=$((tx_count + 1))
    done

    chain_idx=$((chain_idx + 1))
    if [ $((chain_idx % 10)) -eq 0 ]; then
        echo "  Built chain $chain_idx/$CHAINS ($tx_count txs so far)"
    fi
done < "$WORK_DIR/utxos.txt"

BUILD_END=$(date +%s)
echo "Built $tx_count transactions in $((BUILD_END - BUILD_START))s"

if [ "$tx_count" -eq 0 ]; then
    echo "ERROR: No transactions built!"
    exit 1
fi

# ── Phase 2: Submit all transactions ──
echo ""
echo "=== Phase 2: Submitting $tx_count transactions ==="
echo "Start: $(date -u +%Y-%m-%dT%H:%M:%SZ)"

# Pre-submission metrics
PRE_MEMPOOL=$(curl -s http://localhost:12798/metrics | grep "^dugite_mempool_tx_count " | awk '{print $2}')
PRE_VALIDATED=$(curl -s http://localhost:12798/metrics | grep "^dugite_transactions_validated_total " | awk '{print $2}')
echo "Pre-submit mempool: $PRE_MEMPOOL txs"

SUBMIT_START=$(date +%s%N)
submitted=0
failed=0
first_errors=""

for tf in $(ls "$WORK_DIR/tx/"*.signed 2>/dev/null | sort); do
    result=$($CLI transaction submit --socket-path $SOCKET --testnet-magic $MAGIC --tx-file "$tf" 2>&1)
    rc=$?
    if [ $rc -eq 0 ]; then
        submitted=$((submitted + 1))
    else
        failed=$((failed + 1))
        if [ "$failed" -le 10 ]; then
            first_errors="${first_errors}\n  $(basename $tf): $result"
        fi
    fi

    total=$((submitted + failed))
    if [ $((total % 1000)) -eq 0 ]; then
        elapsed_ms=$(( ($(date +%s%N) - SUBMIT_START) / 1000000 ))
        rate=$((total * 1000 / (elapsed_ms + 1)))
        mempool=$(curl -s http://localhost:12798/metrics 2>/dev/null | grep "^dugite_mempool_tx_count " | awk '{print $2}')
        echo "  Progress: $total/$tx_count (ok=$submitted fail=$failed ${rate} tx/s mempool=$mempool)"
    fi
done

SUBMIT_END_NS=$(date +%s%N)
ELAPSED_MS=$(( (SUBMIT_END_NS - SUBMIT_START) / 1000000 ))
if [ "$ELAPSED_MS" -gt 0 ]; then
    RATE=$((submitted * 1000 / ELAPSED_MS))
else
    RATE=0
fi

echo ""
echo "=== Results ==="
echo "End: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo "Transactions built:  $tx_count"
echo "Submitted OK:        $submitted"
echo "Failed:              $failed"
echo "Elapsed:             ${ELAPSED_MS}ms ($((ELAPSED_MS / 1000))s)"
echo "Throughput:          ${RATE} tx/s"

if [ -n "$first_errors" ]; then
    echo ""
    echo "First errors:"
    echo -e "$first_errors"
fi

echo ""
echo "=== Post-submission metrics ==="
sleep 1
curl -s http://localhost:12798/metrics | grep -E "mempool|transactions_" | grep -v "^#"
