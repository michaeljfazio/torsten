#!/usr/bin/env bash
# =============================================================================
# Submit 100 Transactions to Dugite Node (Preview Testnet)
# =============================================================================
# Self-to-self transactions, each consuming a separate UTxO.
# Uses dugite-cli for build-raw, sign, and submit.
# =============================================================================
set -euo pipefail
cd "$(dirname "$0")/.."

CLI="./target/release/dugite-cli"
SOCKET="./node.sock"
MAGIC=2
KEY_DIR="./keys/preview-test"
ADDR=$(cat "$KEY_DIR/payment.addr")
SKEY="$KEY_DIR/payment.skey"
FEE=200000
SEND=2000000        # 2 ADA to self
TX_COUNT=100
TMP_DIR=$(mktemp -d)

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

trap 'rm -rf "$TMP_DIR"' EXIT

echo "================================================================"
echo " Submitting $TX_COUNT transactions to Dugite node"
echo " Address: $ADDR"
echo " Socket:  $SOCKET"
echo " Fee:     $FEE lovelace"
echo "================================================================"
echo ""

# Get current slot from Prometheus metrics (N2C query tip can lag behind)
CURRENT_SLOT=$(curl -s http://localhost:12798/metrics | grep '^dugite_slot_number ' | awk '{print int($2)}')
if [[ -z "$CURRENT_SLOT" || "$CURRENT_SLOT" == "0" ]]; then
    # Fallback to query tip
    CURRENT_SLOT=$("$CLI" query tip --socket-path "$SOCKET" --testnet-magic $MAGIC 2>/dev/null | grep '"slot"' | head -1 | sed 's/[^0-9]//g')
fi
TTL=$((CURRENT_SLOT + 3600))  # ~60 minutes from now
echo "Current slot: $CURRENT_SLOT, TTL: $TTL"
echo ""

# Collect UTxOs into an array (skip header lines)
mapfile -t UTXO_LINES < <("$CLI" query utxo \
    --address "$ADDR" \
    --socket-path "$SOCKET" \
    --testnet-magic $MAGIC 2>/dev/null \
    | tail -n +3 \
    | awk '{print $1 "#" $2, $3}' \
    | sort -k2 -n -r)

AVAILABLE=${#UTXO_LINES[@]}
echo "Available UTxOs: $AVAILABLE"

if (( AVAILABLE < TX_COUNT )); then
    echo -e "${YELLOW}WARNING: Only $AVAILABLE UTxOs available, will submit $AVAILABLE transactions${NC}"
    TX_COUNT=$AVAILABLE
fi
echo ""

SUBMITTED=0
ACCEPTED=0
REJECTED=0

for i in $(seq 1 "$TX_COUNT"); do
    IDX=$((i - 1))
    LINE="${UTXO_LINES[$IDX]}"
    UTXO_REF=$(echo "$LINE" | awk '{print $1}')
    AMOUNT=$(echo "$LINE" | awk '{print $2}')

    CHANGE=$((AMOUNT - SEND - FEE))
    if (( CHANGE < 1000000 )); then
        # Not enough for min UTxO, send everything minus fee as single output
        SEND_ALL=$((AMOUNT - FEE))
        if (( SEND_ALL < 1000000 )); then
            echo -e "${YELLOW}[$i/$TX_COUNT] SKIP — UTxO too small ($AMOUNT lovelace)${NC}"
            continue
        fi
        TX_OUT_ARGS="--tx-out ${ADDR}+${SEND_ALL}"
    else
        TX_OUT_ARGS="--tx-out ${ADDR}+${SEND} --tx-out ${ADDR}+${CHANGE}"
    fi

    RAW="$TMP_DIR/tx-${i}.raw"
    SIGNED="$TMP_DIR/tx-${i}.signed"

    # Build
    if ! "$CLI" transaction build-raw \
        --tx-in "$UTXO_REF" \
        $TX_OUT_ARGS \
        --fee "$FEE" \
        --ttl "$TTL" \
        --out-file "$RAW" 2>/dev/null; then
        echo -e "${RED}[$i/$TX_COUNT] FAIL build-raw${NC}"
        ((REJECTED++)) || true
        continue
    fi

    # Sign
    if ! "$CLI" transaction sign \
        --tx-body-file "$RAW" \
        --signing-key-file "$SKEY" \
        --out-file "$SIGNED" 2>/dev/null; then
        echo -e "${RED}[$i/$TX_COUNT] FAIL sign${NC}"
        ((REJECTED++)) || true
        continue
    fi

    # Submit
    SUBMIT_OUT=$("$CLI" transaction submit \
        --tx-file "$SIGNED" \
        --socket-path "$SOCKET" \
        --testnet-magic $MAGIC 2>&1) && RC=0 || RC=$?

    TXID=$("$CLI" transaction txid --tx-file "$SIGNED" 2>/dev/null || echo "?")
    ((SUBMITTED++)) || true

    if [[ $RC -eq 0 ]]; then
        ((ACCEPTED++)) || true
        echo -e "${GREEN}[$i/$TX_COUNT] OK  ${TXID:0:16}... ($AMOUNT -> $SEND + change, fee=$FEE)${NC}"
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

# Check mempool after submission
echo ""
echo "Mempool status:"
curl -s http://localhost:12798/metrics 2>/dev/null | grep -E 'mempool_tx_count|mempool_bytes|transactions_' || echo "(metrics unavailable)"
