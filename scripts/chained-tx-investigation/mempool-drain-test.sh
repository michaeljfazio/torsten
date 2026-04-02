#!/usr/bin/env bash
# Mempool drain test: fill the mempool with a mix of chained and non-chained
# transactions, then monitor how long it takes for the mempool to clear.
#
# Strategy:
#   - Use 50 UTxOs for 50 independent chains of 20 txs each = 1000 chained txs
#   - Use 50 UTxOs for 50 independent single txs = 50 non-chained txs
#   - Total: ~1050 transactions
#
# Usage: ./mempool-drain-test.sh [--chains N] [--chain-len N] [--singles N]

set -euo pipefail
cd "$(dirname "$0")/../.."

CCLI="cardano-cli"
SOCKET="./node.sock"
CLI="./target/release/dugite-cli"
MAGIC=2
ADDR=$(cat ./keys/preview-test/payment.addr)
SKEY="./keys/preview-test/payment.skey"
FEE=200000
WORK_DIR="/tmp/mempool-drain-test"
NUM_CHAINS=50
CHAIN_LEN=20
NUM_SINGLES=50

while [[ $# -gt 0 ]]; do
    case "$1" in
        --chains) NUM_CHAINS="$2"; shift 2 ;;
        --chain-len) CHAIN_LEN="$2"; shift 2 ;;
        --singles) NUM_SINGLES="$2"; shift 2 ;;
        *) echo "Unknown: $1"; exit 1 ;;
    esac
done

TOTAL=$((NUM_CHAINS * CHAIN_LEN + NUM_SINGLES))

if [[ ! -S "$SOCKET" ]]; then
    echo "ERROR: Socket not found at $SOCKET. Is the node running?"
    exit 1
fi

rm -rf "$WORK_DIR"
mkdir -p "$WORK_DIR/tx"

echo "============================================================"
echo "  Mempool Drain Test"
echo "============================================================"
echo "  Chained:     $NUM_CHAINS chains × $CHAIN_LEN txs = $((NUM_CHAINS * CHAIN_LEN))"
echo "  Non-chained: $NUM_SINGLES single txs"
echo "  Total:       $TOTAL transactions"
echo "============================================================"
echo ""

# Helper to extract tx hash from cardano-cli output (handles JSON format in 10.6.2)
txid() {
    $CCLI conway transaction txid --tx-file "$1" 2>/dev/null | \
        python3 -c "import sys,json; d=sys.stdin.read().strip(); print(json.loads(d)['txhash'] if d.startswith('{') else d)"
}

# Get current slot for TTL (1 epoch ahead)
TIP_SLOT=$($CLI query tip --socket-path $SOCKET --testnet-magic $MAGIC 2>/dev/null | \
    python3 -c "import sys,json; print(json.load(sys.stdin)['slot'])")
TTL=$((TIP_SLOT + 86400))
echo "Current slot: $TIP_SLOT, TTL: $TTL"

# Get UTxOs
echo "Fetching UTxOs..."
$CLI query utxo --socket-path $SOCKET --testnet-magic $MAGIC --address "$ADDR" 2>/dev/null | \
    grep "lovelace" | awk '$3 >= 5000000 {print $1, $2, $3}' > "$WORK_DIR/utxos.txt"

UTXO_COUNT=$(wc -l < "$WORK_DIR/utxos.txt" | tr -d ' ')
NEEDED=$((NUM_CHAINS + NUM_SINGLES))
echo "Available UTxOs (≥5 ADA): $UTXO_COUNT (need $NEEDED)"

if [[ "$UTXO_COUNT" -lt "$NEEDED" ]]; then
    echo "ERROR: Not enough UTxOs. Have $UTXO_COUNT, need $NEEDED"
    exit 1
fi

# ============================================================
# Phase 1: Build all transactions offline
# ============================================================
echo ""
echo "=== Phase 1: Building $TOTAL transactions offline ==="
BUILD_START=$(date +%s)

utxo_idx=0

# --- Build chained transactions ---
echo "  Building $NUM_CHAINS chains of $CHAIN_LEN..."
while IFS=' ' read -r txhash txix amount; do
    if [[ "$utxo_idx" -ge "$NUM_CHAINS" ]]; then
        break
    fi

    current_txhash="$txhash"
    current_txix="$txix"
    current_amount="$amount"

    for i in $(seq 0 $((CHAIN_LEN - 1))); do
        tx_num=$((utxo_idx * CHAIN_LEN + i))
        tx_file="$WORK_DIR/tx/chain_${tx_num}"
        output_amount=$((current_amount - FEE))

        if [[ "$output_amount" -lt 1000000 ]]; then
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

        next_txhash=$(txid "${tx_file}.signed")
        current_txhash="$next_txhash"
        current_txix=0
        current_amount="$output_amount"
    done

    utxo_idx=$((utxo_idx + 1))
    if [[ $((utxo_idx % 10)) -eq 0 ]]; then
        echo "    Built chain $utxo_idx/$NUM_CHAINS"
    fi
done < "$WORK_DIR/utxos.txt"
echo "    Built $utxo_idx chains"

# --- Build single (non-chained) transactions ---
echo "  Building $NUM_SINGLES single txs..."
single_idx=0
tail -n +$((NUM_CHAINS + 1)) "$WORK_DIR/utxos.txt" | while IFS=' ' read -r txhash txix amount; do
    if [[ "$single_idx" -ge "$NUM_SINGLES" ]]; then
        break
    fi

    tx_file="$WORK_DIR/tx/single_${single_idx}"
    output_amount=$((amount - FEE))

    if [[ "$output_amount" -lt 1000000 ]]; then
        single_idx=$((single_idx + 1))
        continue
    fi

    $CCLI conway transaction build-raw \
        --tx-in "${txhash}#${txix}" \
        --tx-out "${ADDR}+${output_amount}" \
        --fee "$FEE" \
        --invalid-hereafter "$TTL" \
        --out-file "${tx_file}.raw" 2>/dev/null

    $CCLI conway transaction sign \
        --tx-body-file "${tx_file}.raw" \
        --signing-key-file "$SKEY" \
        --out-file "${tx_file}.signed" 2>/dev/null

    single_idx=$((single_idx + 1))
done
echo "    Built singles"

BUILD_END=$(date +%s)
BUILT_COUNT=$(ls "$WORK_DIR/tx/"*.signed 2>/dev/null | wc -l | tr -d ' ')
echo "  Total built: $BUILT_COUNT transactions in $((BUILD_END - BUILD_START))s"

# ============================================================
# Phase 2: Submit all transactions as fast as possible
# ============================================================
echo ""
echo "=== Phase 2: Submitting $BUILT_COUNT transactions ==="

# Record pre-submission mempool state
PRE_MEMPOOL=$(curl -s http://localhost:12798/metrics | grep "^dugite_mempool_tx_count " | awk '{print $2}')
echo "  Pre-submission mempool: $PRE_MEMPOOL txs"

SUBMIT_START=$(date +%s%N)
SUBMIT_START_UTC=$(date -u +%Y-%m-%dT%H:%M:%SZ)
echo "  Start: $SUBMIT_START_UTC"

submitted=0
failed=0
> "$WORK_DIR/submitted_hashes.txt"  # Record all submitted tx hashes for Koios validation

# Submit chained txs first (in order within each chain)
for tx_file in $(ls "$WORK_DIR/tx/chain_"*.signed 2>/dev/null | sort -V); do
    result=$($CLI transaction submit \
        --socket-path "$SOCKET" --testnet-magic $MAGIC --tx-file "$tx_file" 2>&1) || true
    if echo "$result" | grep -qi "success\|submitted\|accepted" || [[ -z "$result" ]]; then
        submitted=$((submitted + 1))
        txid "$tx_file" >> "$WORK_DIR/submitted_hashes.txt"
    else
        failed=$((failed + 1))
        if [[ "$failed" -le 3 ]]; then
            echo "    FAIL $(basename $tx_file): $result"
        fi
    fi
done

# Submit single txs
for tx_file in $(ls "$WORK_DIR/tx/single_"*.signed 2>/dev/null | sort -V); do
    result=$($CLI transaction submit \
        --socket-path "$SOCKET" --testnet-magic $MAGIC --tx-file "$tx_file" 2>&1) || true
    if echo "$result" | grep -qi "success\|submitted\|accepted" || [[ -z "$result" ]]; then
        submitted=$((submitted + 1))
        txid "$tx_file" >> "$WORK_DIR/submitted_hashes.txt"
    else
        failed=$((failed + 1))
        if [[ "$failed" -le 3 ]]; then
            echo "    FAIL $(basename $tx_file): $result"
        fi
    fi
done

SUBMIT_END_NS=$(date +%s%N)
SUBMIT_ELAPSED_MS=$(( (SUBMIT_END_NS - SUBMIT_START) / 1000000 ))
SUBMIT_RATE=$((submitted * 1000 / (SUBMIT_ELAPSED_MS + 1)))
echo "  Submitted: $submitted  Failed: $failed  Time: ${SUBMIT_ELAPSED_MS}ms  Rate: ${SUBMIT_RATE} tx/s"

# ============================================================
# Phase 3: Monitor mempool drain
# ============================================================
echo ""
echo "=== Phase 3: Monitoring mempool drain ==="
echo "  Polling every 1s until mempool is empty or 600s timeout..."
echo ""
echo "  TIME          MEMPOOL_TXS  MEMPOOL_BYTES  DELTA"

DRAIN_START=$(date +%s)
prev_count=999999
drain_complete=0

for i in $(seq 1 600); do
    sleep 1
    metrics=$(curl -s http://localhost:12798/metrics 2>/dev/null)
    count=$(echo "$metrics" | grep "^dugite_mempool_tx_count " | awk '{print $2}')
    bytes=$(echo "$metrics" | grep "^dugite_mempool_bytes " | awk '{print $2}')
    count=${count:-0}
    bytes=${bytes:-0}

    delta=$((prev_count - count))
    elapsed=$(($(date +%s) - DRAIN_START))
    ts=$(date -u +%H:%M:%S)

    # Only print when count changes or every 10s
    if [[ "$count" != "$prev_count" ]] || [[ $((i % 10)) -eq 0 ]]; then
        printf "  %s  %4s txs  %8s bytes  -%s (elapsed: %ds)\n" "$ts" "$count" "$bytes" "$delta" "$elapsed"
    fi

    prev_count=$count

    if [[ "$count" == "0" ]]; then
        drain_complete=1
        break
    fi
done

DRAIN_END=$(date +%s)
DRAIN_ELAPSED=$((DRAIN_END - DRAIN_START))

echo ""
echo "============================================================"
echo "  Results"
echo "============================================================"
echo "  Transactions built:     $BUILT_COUNT"
echo "  Submitted successfully: $submitted"
echo "  Submit failures:        $failed"
echo "  Submit time:            ${SUBMIT_ELAPSED_MS}ms (${SUBMIT_RATE} tx/s)"
echo ""
if [[ "$drain_complete" == "1" ]]; then
    echo "  Mempool drain time:     ${DRAIN_ELAPSED}s"
    echo "  Drain rate:             $((submitted / (DRAIN_ELAPSED + 1))) tx/s"
else
    echo "  Mempool NOT drained after ${DRAIN_ELAPSED}s"
    echo "  Remaining txs:          $prev_count"
fi
echo "============================================================"

# ============================================================
# Phase 4: Koios cross-validation
# ============================================================
echo ""
echo "=== Phase 4: Koios cross-validation ==="
echo "  Waiting 60s for Koios indexing..."
sleep 60

KOIOS_API="https://preview.koios.rest/api/v1"
HASH_FILE="$WORK_DIR/submitted_hashes.txt"
TOTAL_HASHES=$(wc -l < "$HASH_FILE" | tr -d ' ')

echo "  Validating $TOTAL_HASHES transactions against Koios..."

confirmed=0
missing=0
koios_errors=0
batch_size=25

batch_start=0
while [[ $batch_start -lt $TOTAL_HASHES ]]; do
    batch_hashes=$(sed -n "$((batch_start + 1)),$((batch_start + batch_size))p" "$HASH_FILE")
    batch_count=$(echo "$batch_hashes" | wc -l | tr -d ' ')

    json_array=$(echo "$batch_hashes" | python3 -c "
import sys, json
hashes = [line.strip() for line in sys.stdin if line.strip()]
print(json.dumps({'_tx_hashes': hashes}))
")

    response=$(curl -s -X POST "$KOIOS_API/tx_status" \
        -H "Content-Type: application/json" \
        -d "$json_array" 2>/dev/null) || true

    if [[ -z "$response" ]] || echo "$response" | grep -q '"error"'; then
        koios_errors=$((koios_errors + batch_count))
    else
        batch_confirmed=$(echo "$response" | python3 -c "
import sys, json
try:
    data = json.load(sys.stdin)
    count = sum(1 for tx in data if tx.get('num_confirmations', 0) > 0)
    print(count)
except:
    print(0)
" 2>/dev/null)
        batch_confirmed=${batch_confirmed:-0}
        batch_missing=$((batch_count - batch_confirmed))
        confirmed=$((confirmed + batch_confirmed))
        missing=$((missing + batch_missing))
    fi

    batch_start=$((batch_start + batch_size))

    if [[ $((batch_start % 100)) -eq 0 ]] || [[ $batch_start -ge $TOTAL_HASHES ]]; then
        echo "    Checked $batch_start/$TOTAL_HASHES  (confirmed: $confirmed, missing: $missing, errors: $koios_errors)"
    fi

    sleep 0.5
done

echo ""
echo "============================================================"
echo "  Koios Cross-Validation"
echo "============================================================"
echo "  Total checked:  $TOTAL_HASHES"
echo "  Confirmed:      $confirmed"
echo "  Missing:        $missing"
echo "  Koios errors:   $koios_errors"
if [[ $TOTAL_HASHES -gt 0 ]]; then
    pct=$((confirmed * 100 / TOTAL_HASHES))
    echo "  Confirmation:   ${pct}%"
fi
echo "============================================================"
