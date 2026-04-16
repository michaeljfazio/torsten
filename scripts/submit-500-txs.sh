#!/usr/bin/env bash
# =============================================================================
# Submit 500 Transactions to Dugite Node (Preview Testnet)
# =============================================================================
# Phase 1: Fan-out the large UTxO into 500+ small UTxOs via sequential fan-out
#           txs, waiting for each to confirm in a block before the next.
# Phase 2: Submit 500 self-to-self transactions, one per UTxO.
# =============================================================================
set -euo pipefail
cd "$(dirname "$0")/.."

CLI="./target/release/dugite-cli"
SOCKET="./node.sock"
MAGIC=2
KEY_DIR="./keys"
ADDR=$(cat "$KEY_DIR/payment.addr")
SKEY="$KEY_DIR/payment.skey"
FEE=200000           # 0.2 ADA per simple tx
OUTPUT_SIZE=2000000  # 2 ADA per fan-out output
TX_COUNT=500
TMP_DIR=$(mktemp -d)

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

trap 'rm -rf "$TMP_DIR"' EXIT

get_slot() {
    curl -s http://localhost:12798/metrics | grep '^dugite_slot_number ' | awk '{print int($2)}'
}

# Wait for mempool to drain (fan-out tx included in a block)
wait_for_block() {
    local target_txid="$1"
    local max_wait=120
    local waited=0
    echo -n "  Waiting for block inclusion"
    while (( waited < max_wait )); do
        sleep 5
        waited=$((waited + 5))
        echo -n "."
        # Check if the UTxO is now in the ledger state
        local check
        check=$("$CLI" query utxo \
            --address "$ADDR" \
            --socket-path "$SOCKET" \
            --testnet-magic $MAGIC 2>/dev/null \
            | grep "${target_txid:0:20}" || true)
        if [[ -n "$check" ]]; then
            echo -e " ${GREEN}confirmed (${waited}s)${NC}"
            return 0
        fi
    done
    echo -e " ${RED}timeout (${max_wait}s)${NC}"
    return 1
}

echo "================================================================"
echo " Submitting $TX_COUNT transactions to Dugite node"
echo " Address: $ADDR"
echo " Socket:  $SOCKET"
echo "================================================================"
echo ""

# ── Phase 1: Fan-out ─────────────────────────────────────────────────────
# Query current UTxOs
query_utxos() {
    "$CLI" query utxo \
        --address "$ADDR" \
        --socket-path "$SOCKET" \
        --testnet-magic $MAGIC 2>/dev/null \
        | tail -n +3 \
        | awk '{print $1 "#" $2, $3}' \
        | sort -k2 -n -r
}

mapfile -t UTXO_LINES < <(query_utxos)
AVAILABLE=${#UTXO_LINES[@]}
echo "Available UTxOs: $AVAILABLE"

if (( AVAILABLE >= TX_COUNT )); then
    echo -e "${GREEN}Already have enough UTxOs ($AVAILABLE >= $TX_COUNT)${NC}"
else
    # Find the largest UTxO
    BIGGEST_REF=$(echo "${UTXO_LINES[0]}" | awk '{print $1}')
    BIGGEST_AMT=$(echo "${UTXO_LINES[0]}" | awk '{print $2}')
    echo "Largest UTxO: $BIGGEST_REF = $BIGGEST_AMT lovelace ($(( BIGGEST_AMT / 1000000 )) ADA)"

    NEED=$((TX_COUNT - AVAILABLE + 1))  # +1 because we consume the big one
    echo ""
    echo -e "${CYAN}Phase 1: Fan-out — need ~$NEED more UTxOs${NC}"

    CURRENT_REF="$BIGGEST_REF"
    CURRENT_AMT=$BIGGEST_AMT
    OUTPUTS_PER_TX=150  # safe for tx size limit (~10KB)
    FANOUT_NUM=0

    while (( NEED > 0 && CURRENT_AMT > OUTPUT_SIZE * 2 )); do
        FANOUT_NUM=$((FANOUT_NUM + 1))
        N=$NEED
        if (( N > OUTPUTS_PER_TX )); then
            N=$OUTPUTS_PER_TX
        fi

        # Dynamic fee: ~180K base + ~3.2K per output
        FANOUT_FEE=$(( 180000 + 3200 * N ))

        TOTAL_OUT=$(( N * OUTPUT_SIZE ))
        CHANGE=$(( CURRENT_AMT - TOTAL_OUT - FANOUT_FEE ))

        # Reduce outputs if change too small
        while (( CHANGE > 0 && CHANGE < 1000000 && N > 1 )); do
            N=$((N - 1))
            FANOUT_FEE=$(( 180000 + 3200 * N ))
            TOTAL_OUT=$(( N * OUTPUT_SIZE ))
            CHANGE=$(( CURRENT_AMT - TOTAL_OUT - FANOUT_FEE ))
        done

        if (( N <= 0 || CHANGE < 0 )); then
            echo -e "${RED}Insufficient funds for more fan-out${NC}"
            break
        fi

        # Get current TTL
        SLOT=$(get_slot)
        TTL=$((SLOT + 7200))

        # Build tx-out args
        TX_OUT_ARGS=""
        for j in $(seq 1 "$N"); do
            TX_OUT_ARGS="$TX_OUT_ARGS --tx-out ${ADDR}+${OUTPUT_SIZE}"
        done
        if (( CHANGE >= 1000000 )); then
            TX_OUT_ARGS="$TX_OUT_ARGS --tx-out ${ADDR}+${CHANGE}"
        fi

        RAW="$TMP_DIR/fanout-${FANOUT_NUM}.raw"
        SIGNED="$TMP_DIR/fanout-${FANOUT_NUM}.signed"

        echo -n "  Fan-out tx $FANOUT_NUM: $N outputs (fee=${FANOUT_FEE}) ... "

        if ! eval "$CLI" transaction build-raw \
            --tx-in "$CURRENT_REF" \
            $TX_OUT_ARGS \
            --fee "$FANOUT_FEE" \
            --ttl "$TTL" \
            --out-file "$RAW" 2>"$TMP_DIR/err.txt"; then
            echo -e "${RED}FAIL build: $(cat "$TMP_DIR/err.txt")${NC}"
            break
        fi

        if ! "$CLI" transaction sign \
            --tx-body-file "$RAW" \
            --signing-key-file "$SKEY" \
            --out-file "$SIGNED" 2>/dev/null; then
            echo -e "${RED}FAIL sign${NC}"
            break
        fi

        TXID=$("$CLI" transaction txid --tx-file "$SIGNED" 2>/dev/null)

        if ! SUBMIT_OUT=$("$CLI" transaction submit \
            --tx-file "$SIGNED" \
            --socket-path "$SOCKET" \
            --testnet-magic $MAGIC 2>&1); then
            echo -e "${RED}FAIL submit: $SUBMIT_OUT${NC}"
            break
        fi

        echo -e "${GREEN}OK txid=${TXID:0:16}...${NC}"

        # Wait for block inclusion before next fan-out
        if ! wait_for_block "$TXID"; then
            echo -e "${RED}Fan-out tx not confirmed, aborting fan-out${NC}"
            break
        fi

        # Update for next iteration: change is the last output
        if (( CHANGE >= 1000000 )); then
            CURRENT_REF="${TXID}#${N}"
            CURRENT_AMT=$CHANGE
        else
            CURRENT_AMT=0
        fi

        NEED=$((NEED - N))
    done

    echo ""
    echo "Fan-out phase complete."

    # Re-query UTxOs
    mapfile -t UTXO_LINES < <(query_utxos)
    AVAILABLE=${#UTXO_LINES[@]}
    echo "Available UTxOs after fan-out: $AVAILABLE"
fi

if (( AVAILABLE < TX_COUNT )); then
    echo -e "${YELLOW}Adjusting TX_COUNT to $AVAILABLE${NC}"
    TX_COUNT=$AVAILABLE
fi

# ── Phase 2: Submit self-to-self transactions ────────────────────────────
SLOT=$(get_slot)
TTL=$((SLOT + 7200))
echo ""
echo -e "${CYAN}Phase 2: Submitting $TX_COUNT self-to-self transactions (TTL=$TTL)${NC}"
echo ""

SUBMITTED=0
ACCEPTED=0
REJECTED=0

for i in $(seq 1 "$TX_COUNT"); do
    IDX=$((i - 1))
    LINE="${UTXO_LINES[$IDX]}"
    UTXO_REF=$(echo "$LINE" | awk '{print $1}')
    AMOUNT=$(echo "$LINE" | awk '{print $2}')

    CHANGE=$((AMOUNT - FEE))
    if (( CHANGE < 1000000 )); then
        echo -e "${YELLOW}[$i/$TX_COUNT] SKIP — UTxO too small ($AMOUNT lovelace)${NC}"
        continue
    fi

    RAW="$TMP_DIR/tx-${i}.raw"
    SIGNED="$TMP_DIR/tx-${i}.signed"

    if ! "$CLI" transaction build-raw \
        --tx-in "$UTXO_REF" \
        --tx-out "${ADDR}+${CHANGE}" \
        --fee "$FEE" \
        --ttl "$TTL" \
        --out-file "$RAW" 2>/dev/null; then
        ((REJECTED++)) || true
        continue
    fi

    if ! "$CLI" transaction sign \
        --tx-body-file "$RAW" \
        --signing-key-file "$SKEY" \
        --out-file "$SIGNED" 2>/dev/null; then
        ((REJECTED++)) || true
        continue
    fi

    SUBMIT_OUT=$("$CLI" transaction submit \
        --tx-file "$SIGNED" \
        --socket-path "$SOCKET" \
        --testnet-magic $MAGIC 2>&1) && RC=0 || RC=$?

    TXID=$("$CLI" transaction txid --tx-file "$SIGNED" 2>/dev/null || echo "?")
    ((SUBMITTED++)) || true

    if [[ $RC -eq 0 ]]; then
        ((ACCEPTED++)) || true
        if (( i % 25 == 0 || i <= 5 || i == TX_COUNT )); then
            echo -e "${GREEN}[$i/$TX_COUNT] OK  ${TXID:0:16}... ($AMOUNT -> $CHANGE)${NC}"
        fi
    else
        ((REJECTED++)) || true
        echo -e "${RED}[$i/$TX_COUNT] REJ ${TXID:0:16}... — $SUBMIT_OUT${NC}"
    fi
done

echo ""
echo "================================================================"
echo " Results"
echo "================================================================"
echo " Submitted: $SUBMITTED"
echo -e " Accepted:  ${GREEN}$ACCEPTED${NC}"
echo -e " Rejected:  ${RED}$REJECTED${NC}"
echo "================================================================"
echo ""
echo "Mempool status:"
curl -s http://localhost:12798/metrics 2>/dev/null | grep -E 'mempool_tx_count|mempool_bytes|transactions_' || echo "(metrics unavailable)"
