#!/usr/bin/env bash
# Relay TX stress test
#
# Builds N chained transactions offline from each UTxO, then submits in rapid-fire.
# Tests: mempool ingestion rate, tx validation throughput, relay propagation.
#
# Usage: ./scripts/relay-stress-test.sh [--chains N] [--chain-len N]
set -uo pipefail
cd "$(dirname "$0")/.."

CLI="./target/release/dugite-cli"
SOCKET="./relay.sock"
MAGIC=2
ADDR=$(cat ./keys/payment.addr)
SKEY="./keys/payment.skey"
RELAY_METRICS="http://localhost:12798/metrics"
FEE=180000
WORK_DIR="/tmp/relay-stress-test"

CHAINS=30
CHAIN_LEN=20

while [[ $# -gt 0 ]]; do
    case "$1" in
        --chains)    CHAINS="$2";    shift 2 ;;
        --chain-len) CHAIN_LEN="$2"; shift 2 ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

TOTAL=$((CHAINS * CHAIN_LEN))

echo "=== Relay TX Stress Test ==="
echo "Chains: $CHAINS x $CHAIN_LEN = $TOTAL transactions"
echo "Socket: $SOCKET"
echo "Fee:    $FEE lovelace per tx"
echo ""

rm -rf "$WORK_DIR"
mkdir -p "$WORK_DIR/tx"

# Get current tip and TTL
TIP_SLOT=$($CLI query tip --socket-path "$SOCKET" --testnet-magic $MAGIC \
    | python3 -c "import sys,json; print(json.load(sys.stdin)['slot'])")
TTL=$((TIP_SLOT + 86400))
echo "Tip slot: $TIP_SLOT  TTL: $TTL"

# Fetch UTxOs
echo "Fetching UTxOs..."
$CLI query utxo --socket-path "$SOCKET" --testnet-magic $MAGIC --address "$ADDR" \
    | tail -n +3 \
    | awk '{print $1, $2, $3}' \
    | sort -k3 -n -r \
    > "$WORK_DIR/utxos.txt"

UTXO_COUNT=$(wc -l < "$WORK_DIR/utxos.txt" | tr -d ' ')
echo "Available UTxOs: $UTXO_COUNT"

if (( UTXO_COUNT < CHAINS )); then
    echo "WARNING: Only $UTXO_COUNT UTxOs; reducing chains to $UTXO_COUNT"
    CHAINS=$UTXO_COUNT
    TOTAL=$((CHAINS * CHAIN_LEN))
fi

# Pre-submission metrics snapshot
# n2c_txs_* = transactions submitted via dugite-cli (N2C LocalTxSubmission)
# transactions_* = transactions received/validated from P2P network peers
PRE_N2C_SUBMITTED=$(curl -s "$RELAY_METRICS" | grep "^dugite_n2c_txs_submitted_total " | awk '{print $2}')
PRE_N2C_ACCEPTED=$(curl -s  "$RELAY_METRICS" | grep "^dugite_n2c_txs_accepted_total "  | awk '{print $2}')
PRE_N2C_REJECTED=$(curl -s  "$RELAY_METRICS" | grep "^dugite_n2c_txs_rejected_total "  | awk '{print $2}')
PRE_MEMPOOL=$(curl -s        "$RELAY_METRICS" | grep "^dugite_mempool_tx_count "        | awk '{print $2}')
echo "Pre-test: n2c_submitted=${PRE_N2C_SUBMITTED:-0} n2c_accepted=${PRE_N2C_ACCEPTED:-0} n2c_rejected=${PRE_N2C_REJECTED:-0} mempool=${PRE_MEMPOOL:-0}"

# ── Phase 1: Build all transaction chains offline ──────────────────────────
echo ""
echo "=== Phase 1: Building $TOTAL transactions offline ==="
BUILD_START=$(date +%s)

chain_idx=0
total_built=0

while IFS=' ' read -r txhash txix amount; do
    (( chain_idx >= CHAINS )) && break

    cur_hash="$txhash"
    cur_ix="$txix"
    cur_amt="$amount"

    for i in $(seq 0 $((CHAIN_LEN - 1))); do
        tx_num=$((chain_idx * CHAIN_LEN + i))
        tf="$WORK_DIR/tx/tx_$(printf '%06d' "$tx_num")"

        out_amt=$((cur_amt - FEE))
        if (( out_amt < 1000000 )); then
            echo "  Chain $chain_idx: exhausted at tx $i (remaining=${cur_amt})"
            break
        fi

        # Build
        $CLI transaction build-raw \
            --tx-in "${cur_hash}#${cur_ix}" \
            --tx-out "${ADDR}+${out_amt}" \
            --fee "$FEE" \
            --ttl "$TTL" \
            --out-file "${tf}.raw" 2>/dev/null

        # Sign
        $CLI transaction sign \
            --tx-body-file "${tf}.raw" \
            --signing-key-file "$SKEY" \
            --out-file "${tf}.signed" 2>/dev/null

        # Get tx hash for next in chain
        next_hash=$($CLI transaction txid --tx-file "${tf}.signed" 2>/dev/null)
        if [[ -z "$next_hash" ]]; then
            echo "  Chain $chain_idx: txid failed at tx $i"
            break
        fi

        cur_hash="$next_hash"
        cur_ix=0
        cur_amt="$out_amt"
        total_built=$((total_built + 1))
    done

    chain_idx=$((chain_idx + 1))
    if (( chain_idx % 5 == 0 )); then
        echo "  Built chain $chain_idx/$CHAINS ($total_built txs)"
    fi
done < "$WORK_DIR/utxos.txt"

BUILD_END=$(date +%s)
echo "Built $total_built transactions in $((BUILD_END - BUILD_START))s"

if (( total_built == 0 )); then
    echo "ERROR: No transactions built"
    exit 1
fi

# ── Phase 2: Submit all transactions rapid-fire ───────────────────────────
echo ""
echo "=== Phase 2: Submitting $total_built transactions ==="
echo "Start: $(date -u +%Y-%m-%dT%H:%M:%SZ)"

SUBMIT_START_NS=$(date +%s%N)
submitted=0
failed=0
first_errors=""

for tf in $(ls "$WORK_DIR/tx/"*.signed 2>/dev/null | sort); do
    result=$($CLI transaction submit \
        --tx-file "$tf" \
        --socket-path "$SOCKET" \
        --testnet-magic $MAGIC 2>&1) && rc=0 || rc=$?

    if [[ $rc -eq 0 ]]; then
        submitted=$((submitted + 1))
    else
        failed=$((failed + 1))
        if (( failed <= 5 )); then
            first_errors+="  $(basename "$tf"): $result\n"
        fi
    fi

    total=$((submitted + failed))
    if (( total % 100 == 0 )); then
        elapsed_ms=$(( ($(date +%s%N) - SUBMIT_START_NS) / 1000000 ))
        rate=$(( total * 1000 / (elapsed_ms + 1) ))
        mempool=$(curl -s "$RELAY_METRICS" 2>/dev/null | grep "^dugite_mempool_tx_count " | awk '{print $2}')
        echo "  [$total/$total_built] submitted=$submitted failed=$failed rate=${rate}tx/s mempool=${mempool:-?}"
    fi
done

SUBMIT_END_NS=$(date +%s%N)
ELAPSED_MS=$(( (SUBMIT_END_NS - SUBMIT_START_NS) / 1000000 ))
RATE=$(( submitted * 1000 / (ELAPSED_MS + 1) ))

echo ""
echo "=== Results ==="
echo "End:          $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo "Built:        $total_built"
echo "Submitted OK: $submitted"
echo "Failed:       $failed"
echo "Elapsed:      ${ELAPSED_MS}ms ($(( ELAPSED_MS / 1000 ))s)"
echo "Rate:         ${RATE} tx/s"

if [[ -n "$first_errors" ]]; then
    echo ""
    echo "First errors:"
    printf "%b" "$first_errors"
fi

# ── Phase 3: Post-submission metrics ─────────────────────────────────────
echo ""
echo "=== Post-submission relay metrics ==="
sleep 2
POST_N2C_SUBMITTED=$(curl -s "$RELAY_METRICS" | grep "^dugite_n2c_txs_submitted_total " | awk '{print $2}')
POST_N2C_ACCEPTED=$(curl -s  "$RELAY_METRICS" | grep "^dugite_n2c_txs_accepted_total "  | awk '{print $2}')
POST_N2C_REJECTED=$(curl -s  "$RELAY_METRICS" | grep "^dugite_n2c_txs_rejected_total "  | awk '{print $2}')
POST_MEMPOOL=$(curl -s        "$RELAY_METRICS" | grep "^dugite_mempool_tx_count "        | awk '{print $2}')
POST_P2P_RECV=$(curl -s       "$RELAY_METRICS" | grep "^dugite_transactions_received_total "  | awk '{print $2}')
POST_P2P_VAL=$(curl -s        "$RELAY_METRICS" | grep "^dugite_transactions_validated_total " | awk '{print $2}')
POST_P2P_REJ=$(curl -s        "$RELAY_METRICS" | grep "^dugite_transactions_rejected_total "  | awk '{print $2}')

NEW_N2C_SUBMITTED=$(( ${POST_N2C_SUBMITTED:-0} - ${PRE_N2C_SUBMITTED:-0} ))
NEW_N2C_ACCEPTED=$(( ${POST_N2C_ACCEPTED:-0}  - ${PRE_N2C_ACCEPTED:-0}  ))
NEW_N2C_REJECTED=$(( ${POST_N2C_REJECTED:-0}  - ${PRE_N2C_REJECTED:-0}  ))

echo "N2C (dugite-cli submissions):"
echo "  submitted: ${POST_N2C_SUBMITTED:-0} (new: +${NEW_N2C_SUBMITTED})"
echo "  accepted:  ${POST_N2C_ACCEPTED:-0}  (new: +${NEW_N2C_ACCEPTED})"
echo "  rejected:  ${POST_N2C_REJECTED:-0}  (new: +${NEW_N2C_REJECTED})"
echo "P2P (from network peers):"
echo "  received:  ${POST_P2P_RECV:-0}"
echo "  validated: ${POST_P2P_VAL:-0}"
echo "  rejected:  ${POST_P2P_REJ:-0}"
echo "Mempool:   ${POST_MEMPOOL:-0} txs"
echo ""

if (( NEW_N2C_REJECTED > 0 )); then
    echo "WARN: $NEW_N2C_REJECTED N2C transactions rejected by relay"
fi
echo "Stress test complete."
