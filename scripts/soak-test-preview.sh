#!/usr/bin/env bash
# =============================================================================
# Torsten 12-Hour Soak Test — Preview Network Block Producer
# =============================================================================
#
# This script orchestrates a long-running soak test that:
#   1. Starts the node as a block producer on preview testnet
#   2. Monitors metrics, logs, and node health continuously
#   3. Restarts the node periodically (clean SIGTERM + hard SIGKILL cycles)
#   4. Submits valid and invalid transactions at intervals
#   5. Logs all observations to a structured report file
#
# Usage: ./scripts/soak-test-preview.sh [--duration HOURS] [--restart-interval MINUTES]
#
# Prerequisites:
#   - cargo build --release (binary at ./target/release/torsten-node)
#   - Keys in ./keys/preview-test/pool/
#   - Database in ./db-preview/ (will mithril-import if missing)
# =============================================================================

set -euo pipefail
cd "$(dirname "$0")/.."

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------
DURATION_HOURS=12
RESTART_INTERVAL_MIN=90
MONITOR_INTERVAL_SEC=30
TX_INTERVAL_MIN=15
METRICS_URL="http://localhost:12798/metrics"
SOCKET_PATH="./node.sock"
DB_PATH="./db-preview"
KEY_DIR="./keys/preview-test"
POOL_KEY_DIR="$KEY_DIR/pool"
BIN="./target/release/torsten-node"
CLI="./target/release/torsten-cli"
LOG_DIR="./logs/soak-test"
REPORT_FILE="$LOG_DIR/soak-report-$(date +%Y%m%d-%H%M%S).log"
NODE_LOG="$LOG_DIR/node.log"
NETWORK_MAGIC=2

# Parse CLI args
while [[ $# -gt 0 ]]; do
    case "$1" in
        --duration) DURATION_HOURS="$2"; shift 2 ;;
        --restart-interval) RESTART_INTERVAL_MIN="$2"; shift 2 ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

DURATION_SEC=$((DURATION_HOURS * 3600))
RESTART_INTERVAL_SEC=$((RESTART_INTERVAL_MIN * 60))
TX_INTERVAL_SEC=$((TX_INTERVAL_MIN * 60))

# ---------------------------------------------------------------------------
# Setup
# ---------------------------------------------------------------------------
mkdir -p "$LOG_DIR"

NODE_PID=""
START_TIME=$(date +%s)
RESTART_COUNT=0
TX_SUBMITTED=0
TX_VALID=0
TX_INVALID=0
ISSUES_FOUND=0
LAST_RESTART_TIME=$START_TIME
LAST_TX_TIME=$START_TIME
LAST_BLOCK_HEIGHT=0
STALL_COUNT=0

# Colours for terminal output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m' # No Colour

# ---------------------------------------------------------------------------
# Logging helpers
# ---------------------------------------------------------------------------
log() {
    local level="$1"; shift
    local ts
    ts=$(date '+%Y-%m-%d %H:%M:%S')
    local elapsed=$(( $(date +%s) - START_TIME ))
    local h=$((elapsed / 3600))
    local m=$(((elapsed % 3600) / 60))
    local s=$((elapsed % 60))
    local elapsed_fmt
    elapsed_fmt=$(printf "%02d:%02d:%02d" "$h" "$m" "$s")
    echo "[$ts] [+$elapsed_fmt] [$level] $*" >> "$REPORT_FILE"
    case "$level" in
        ERROR)   echo -e "${RED}[$elapsed_fmt] [$level] $*${NC}" ;;
        WARN)    echo -e "${YELLOW}[$elapsed_fmt] [$level] $*${NC}" ;;
        OK)      echo -e "${GREEN}[$elapsed_fmt] [$level] $*${NC}" ;;
        TX)      echo -e "${CYAN}[$elapsed_fmt] [$level] $*${NC}" ;;
        RESTART) echo -e "${BLUE}[$elapsed_fmt] [$level] $*${NC}" ;;
        *)       echo "[$elapsed_fmt] [$level] $*" ;;
    esac
}

log_section() {
    echo "" >> "$REPORT_FILE"
    echo "===== $1 =====" >> "$REPORT_FILE"
    echo "" >> "$REPORT_FILE"
    echo -e "\n${BLUE}===== $1 =====${NC}\n"
}

# ---------------------------------------------------------------------------
# Cleanup on exit
# ---------------------------------------------------------------------------
cleanup() {
    log INFO "Shutting down soak test..."
    if [[ -n "$NODE_PID" ]] && kill -0 "$NODE_PID" 2>/dev/null; then
        log INFO "Stopping node (PID $NODE_PID)..."
        kill -TERM "$NODE_PID" 2>/dev/null || true
        wait "$NODE_PID" 2>/dev/null || true
    fi
    print_summary
    log INFO "Soak test complete. Report: $REPORT_FILE"
}
trap cleanup EXIT INT TERM

# ---------------------------------------------------------------------------
# Node management
# ---------------------------------------------------------------------------
start_node() {
    log INFO "Starting torsten-node as block producer..."

    # Ensure no stale torsten-node processes are running
    local stale_pid
    stale_pid=$(pgrep -f "torsten-node run" || true)
    if [[ -n "$stale_pid" ]]; then
        log WARN "Killing stale torsten-node process ($stale_pid)"
        kill -KILL "$stale_pid" 2>/dev/null || true
        sleep 2
    fi

    # Remove stale UTxO store lock (safe because we verified no process is running)
    rm -f "$DB_PATH/utxo-store/lock"

    # Remove stale socket
    rm -f "$SOCKET_PATH"

    # Wait for metrics port to be released
    local port_wait=0
    while lsof -i :12798 >/dev/null 2>&1 && [[ $port_wait -lt 15 ]]; do
        sleep 1
        port_wait=$((port_wait + 1))
    done
    if [[ $port_wait -gt 0 ]]; then
        log INFO "Waited ${port_wait}s for metrics port to be released"
    fi

    # Rotate node log
    if [[ -f "$NODE_LOG" ]]; then
        mv "$NODE_LOG" "$NODE_LOG.$(date +%s)"
    fi

    "$BIN" run \
        --config config/preview-config.json \
        --topology config/preview-topology.json \
        --database-path "$DB_PATH" \
        --socket-path "$SOCKET_PATH" \
        --host-addr 0.0.0.0 \
        --port 3001 \
        --shelley-kes-key "$POOL_KEY_DIR/kes.skey" \
        --shelley-vrf-key "$POOL_KEY_DIR/vrf.skey" \
        --shelley-operational-certificate "$POOL_KEY_DIR/opcert.cert" \
        --log-output stdout --log-output file \
        --log-dir "$LOG_DIR" \
        --log-level info \
        --log-format text \
        --compat-metrics \
        > "$NODE_LOG" 2>&1 &

    NODE_PID=$!
    log OK "Node started with PID $NODE_PID"

    # Wait for socket to appear (up to 120s — UTxO store + address index rebuild
    # + chunk replay takes ~90s on preview with a full snapshot)
    local waited=0
    while [[ ! -S "$SOCKET_PATH" ]] && [[ $waited -lt 120 ]]; do
        sleep 1
        waited=$((waited + 1))
        # Check node hasn't crashed
        if ! kill -0 "$NODE_PID" 2>/dev/null; then
            log ERROR "Node crashed during startup! Last 20 lines of log:"
            tail -20 "$NODE_LOG" >> "$REPORT_FILE" 2>/dev/null
            tail -20 "$NODE_LOG" 2>/dev/null
            return 1
        fi
    done

    if [[ -S "$SOCKET_PATH" ]]; then
        log OK "Node socket ready after ${waited}s"
    else
        log WARN "Socket not ready after ${waited}s — node may still be initializing"
    fi

    LAST_RESTART_TIME=$(date +%s)
    return 0
}

stop_node_graceful() {
    if [[ -z "$NODE_PID" ]] || ! kill -0 "$NODE_PID" 2>/dev/null; then
        log WARN "Node not running (PID=$NODE_PID)"
        return 0
    fi
    log RESTART "Sending SIGTERM to node (PID $NODE_PID)..."
    kill -TERM "$NODE_PID" 2>/dev/null || true

    # Wait up to 30s for graceful shutdown
    local waited=0
    while kill -0 "$NODE_PID" 2>/dev/null && [[ $waited -lt 30 ]]; do
        sleep 1
        waited=$((waited + 1))
    done

    if kill -0 "$NODE_PID" 2>/dev/null; then
        log WARN "Node didn't stop gracefully after 30s, sending SIGKILL"
        kill -KILL "$NODE_PID" 2>/dev/null || true
        wait "$NODE_PID" 2>/dev/null || true
    else
        log OK "Node stopped gracefully after ${waited}s"
    fi

    # Clean up socket and lock
    rm -f "$SOCKET_PATH"
    rm -f "$DB_PATH/utxo-store/lock"
    NODE_PID=""
    sleep 5
}

stop_node_hard() {
    if [[ -z "$NODE_PID" ]] || ! kill -0 "$NODE_PID" 2>/dev/null; then
        return 0
    fi
    log RESTART "Hard-killing node (SIGKILL, PID $NODE_PID) to test crash recovery..."
    kill -KILL "$NODE_PID" 2>/dev/null || true
    wait "$NODE_PID" 2>/dev/null || true
    rm -f "$SOCKET_PATH"
    rm -f "$DB_PATH/utxo-store/lock"
    NODE_PID=""
    sleep 5
}

restart_node() {
    RESTART_COUNT=$((RESTART_COUNT + 1))
    log_section "RESTART #$RESTART_COUNT"

    # Alternate between graceful and hard restarts
    if (( RESTART_COUNT % 3 == 0 )); then
        stop_node_hard
        log RESTART "Recovery restart after SIGKILL (#$RESTART_COUNT)"
    else
        stop_node_graceful
        log RESTART "Graceful restart (#$RESTART_COUNT)"
    fi

    start_node
}

# ---------------------------------------------------------------------------
# Health checks
# ---------------------------------------------------------------------------
check_node_alive() {
    if [[ -z "$NODE_PID" ]] || ! kill -0 "$NODE_PID" 2>/dev/null; then
        log ERROR "Node process not running!"
        ISSUES_FOUND=$((ISSUES_FOUND + 1))
        return 1
    fi
    return 0
}

check_metrics() {
    local metrics
    if ! metrics=$(curl -s --max-time 5 "$METRICS_URL" 2>/dev/null); then
        log WARN "Metrics endpoint unreachable"
        return 1
    fi

    # Extract key metrics
    local sync_pct block_height slot_num epoch_num peer_count utxo_count
    local mempool_tx mempool_bytes blocks_applied blocks_forged rollbacks

    # Helper: extract a single metric value, defaulting to "?" on failure
    _m() { echo "$metrics" | grep "^$1 " | head -1 | awk '{print $2}'; }

    sync_pct=$(_m torsten_sync_progress_percent)
    block_height=$(_m torsten_block_number)
    slot_num=$(_m torsten_slot_number)
    epoch_num=$(_m torsten_epoch_number)
    peer_count=$(_m torsten_peers_connected)
    utxo_count=$(_m torsten_utxo_count)
    mempool_tx=$(_m torsten_mempool_tx_count)
    mempool_bytes=$(_m torsten_mempool_bytes)
    blocks_applied=$(_m torsten_blocks_applied_total)
    blocks_forged=$(_m torsten_blocks_forged_total)
    rollbacks=$(_m torsten_rollback_count_total)

    # Default empty to "?"
    : "${sync_pct:=?}" "${block_height:=?}" "${slot_num:=?}" "${epoch_num:=?}"
    : "${peer_count:=?}" "${utxo_count:=?}" "${mempool_tx:=?}" "${mempool_bytes:=?}"
    : "${blocks_applied:=?}" "${blocks_forged:=?}" "${rollbacks:=?}"

    # Get memory usage (RSS)
    local rss_bytes
    rss_bytes=$(_m torsten_mem_resident_bytes)
    : "${rss_bytes:=0}"
    local rss_mb="?"
    if [[ "$rss_bytes" != "0" && "$rss_bytes" != "?" ]]; then
        rss_mb=$(echo "$rss_bytes" | awk '{printf "%.0f", $1/1048576}')
    fi

    # Convert sync progress (0-10000) to percentage (0-100)
    local sync_display="$sync_pct"
    if [[ "$sync_pct" != "?" ]]; then
        sync_display=$(echo "$sync_pct" | awk '{printf "%.2f", $1/100.0}')
    fi

    log INFO "sync=${sync_display}% block=$block_height slot=$slot_num epoch=$epoch_num peers=$peer_count utxo=$utxo_count mempool=$mempool_tx/$mempool_bytes rss=${rss_mb}MB applied=$blocks_applied forged=$blocks_forged rollbacks=$rollbacks"

    # Check for stalls: block height not advancing
    if [[ "$block_height" != "?" ]]; then
        local bh
        bh=$(printf "%.0f" "$block_height" 2>/dev/null || echo "0")
        if [[ $bh -gt 0 && $LAST_BLOCK_HEIGHT -gt 0 && $bh -le $LAST_BLOCK_HEIGHT ]]; then
            STALL_COUNT=$((STALL_COUNT + 1))
            if (( STALL_COUNT >= 10 )); then
                log ERROR "Block height stalled at $bh for $((STALL_COUNT * MONITOR_INTERVAL_SEC))s!"
                ISSUES_FOUND=$((ISSUES_FOUND + 1))
            else
                log WARN "Block height unchanged at $bh (stall count: $STALL_COUNT)"
            fi
        else
            if (( STALL_COUNT >= 3 )); then
                log OK "Block height resumed advancing: $LAST_BLOCK_HEIGHT -> $bh"
            fi
            STALL_COUNT=0
        fi
        LAST_BLOCK_HEIGHT=$bh
    fi

    # Warn on low peer count
    if [[ "$peer_count" != "?" ]]; then
        local pc
        pc=$(printf "%.0f" "$peer_count" 2>/dev/null || echo "0")
        if (( pc < 3 )); then
            log WARN "Low peer count: $pc"
        fi
    fi

    # Warn on high memory (> 8GB — node normally uses ~5.5GB with full UTxO store)
    if [[ "$rss_mb" != "?" ]]; then
        if (( rss_mb > 8192 )); then
            log WARN "High memory usage: ${rss_mb}MB"
            ISSUES_FOUND=$((ISSUES_FOUND + 1))
        fi
    fi

    return 0
}

check_logs_for_errors() {
    # Check both stdout capture and tracing log files for error patterns
    local log_files=()
    [[ -f "$NODE_LOG" ]] && log_files+=("$NODE_LOG")
    # Also check the most recent tracing log file in LOG_DIR
    local latest_log
    latest_log=$(ls -t "$LOG_DIR"/torsten-*.log 2>/dev/null | head -1)
    [[ -n "$latest_log" ]] && log_files+=("$latest_log")

    if [[ ${#log_files[@]} -eq 0 ]]; then
        return 0
    fi

    local error_count=0
    local warn_count=0
    for lf in "${log_files[@]}"; do
        local ec wc
        ec=$(tail -200 "$lf" 2>/dev/null | grep -ci "panic\|fatal\|assertion failed\|stack overflow\|out of memory\|segfault" || true)
        error_count=$((error_count + ${ec:-0}))
        wc=$(tail -200 "$lf" 2>/dev/null | grep -ci "ERROR" || true)
        warn_count=$((warn_count + ${wc:-0}))
    done

    if (( error_count > 0 )); then
        log ERROR "Found $error_count critical errors in recent logs!"
        for lf in "${log_files[@]}"; do
            tail -200 "$lf" 2>/dev/null | grep -i "panic\|fatal\|assertion failed\|stack overflow\|out of memory\|segfault" | tail -5 >> "$REPORT_FILE" 2>/dev/null
            tail -200 "$lf" 2>/dev/null | grep -i "panic\|fatal\|assertion failed\|stack overflow\|out of memory\|segfault" | tail -5
        done
        ISSUES_FOUND=$((ISSUES_FOUND + 1))
    fi

    if (( warn_count > 20 )); then
        log WARN "High error rate in logs: $warn_count ERRORs in last 200 lines"
        for lf in "${log_files[@]}"; do
            tail -200 "$lf" 2>/dev/null | grep "ERROR" | sed 's/.*ERROR //' | sort | uniq -c | sort -rn | head -3 >> "$REPORT_FILE" 2>/dev/null
        done
    fi
}

query_tip() {
    # Query the node's tip via CLI
    if [[ ! -S "$SOCKET_PATH" ]]; then
        log WARN "Socket not available for query"
        return 1
    fi

    local tip_output
    if tip_output=$("$CLI" query tip --socket-path "$SOCKET_PATH" --testnet-magic $NETWORK_MAGIC 2>&1); then
        log OK "query tip: $tip_output"
        return 0
    else
        log WARN "query tip failed: $tip_output"
        return 1
    fi
}

# ---------------------------------------------------------------------------
# Transaction submission
# ---------------------------------------------------------------------------
query_utxos_raw() {
    # Query UTxOs at our payment address via CLI (tabular output)
    local addr
    addr=$(cat "$KEY_DIR/payment.addr")
    if [[ ! -S "$SOCKET_PATH" ]]; then
        return 1
    fi
    "$CLI" query utxo \
        --address "$addr" \
        --socket-path "$SOCKET_PATH" \
        --testnet-magic $NETWORK_MAGIC 2>/dev/null || return 1
}

# Parse tabular UTxO output → returns "txhash#index amount" lines
parse_utxos() {
    # Skip header lines (first 2), parse: TxHash TxIx Amount ...
    awk 'NR > 2 && NF >= 3 { print $1 "#" $2, $3 }'
}

# Get the first UTxO with > min_amount lovelace
# Outputs: TX_HASH#INDEX AMOUNT
get_first_utxo() {
    local min_amount="${1:-5000000}"
    local raw
    raw=$(query_utxos_raw) || return 1
    echo "$raw" | parse_utxos | while read -r utxo amount; do
        if [[ "$amount" =~ ^[0-9]+$ ]] && (( amount > min_amount )); then
            echo "$utxo $amount"
            return 0
        fi
    done
}

get_current_slot() {
    local tip
    tip=$("$CLI" query tip --socket-path "$SOCKET_PATH" --testnet-magic $NETWORK_MAGIC 2>/dev/null) || return 1
    echo "$tip" | python3 -c "import sys,json; print(json.load(sys.stdin).get('slot', 0))" 2>/dev/null || echo "0"
}

submit_valid_tx() {
    log_section "VALID TRANSACTION"
    TX_SUBMITTED=$((TX_SUBMITTED + 1))

    local addr
    addr=$(cat "$KEY_DIR/payment.addr")

    # Get first suitable UTxO
    local utxo_line
    utxo_line=$(get_first_utxo 5000000) || { log WARN "Cannot query UTxOs — skipping tx"; return 1; }
    if [[ -z "$utxo_line" ]]; then
        log WARN "No suitable UTxO found"
        return 1
    fi
    local first_utxo first_amount
    first_utxo=$(echo "$utxo_line" | awk '{print $1}')
    first_amount=$(echo "$utxo_line" | awk '{print $2}')

    log TX "Using UTxO: $first_utxo ($first_amount lovelace)"

    # Get current slot for TTL
    local current_slot
    current_slot=$(get_current_slot)
    local ttl=$((current_slot + 600))  # ~10 minutes from now

    # Send small amount to self (preserves funds)
    local send_amount=2000000  # 2 ADA
    local fee=200000
    local change=$((first_amount - send_amount - fee))
    local tx_raw="/tmp/soak-tx-$$.raw"
    local tx_signed="/tmp/soak-tx-$$.signed"

    if (( change < 1000000 )); then
        log WARN "Insufficient funds for tx (balance=$first_amount)"
        return 1
    fi

    "$CLI" transaction build-raw \
        --tx-in "$first_utxo" \
        --tx-out "$addr+$send_amount" \
        --tx-out "$addr+$change" \
        --fee "$fee" \
        --ttl "$ttl" \
        --out-file "$tx_raw" 2>&1 || { log WARN "build-raw failed"; rm -f "$tx_raw"; return 1; }

    # Sign
    "$CLI" transaction sign \
        --tx-body-file "$tx_raw" \
        --signing-key-file "$KEY_DIR/payment.skey" \
        --out-file "$tx_signed" 2>&1 || { log WARN "sign failed"; rm -f "$tx_raw" "$tx_signed"; return 1; }

    # Submit
    local submit_output
    if submit_output=$("$CLI" transaction submit \
        --tx-file "$tx_signed" \
        --socket-path "$SOCKET_PATH" \
        --testnet-magic $NETWORK_MAGIC 2>&1); then
        log OK "VALID TX ACCEPTED: $submit_output"
        TX_VALID=$((TX_VALID + 1))
    else
        log WARN "Valid tx rejected: $submit_output"
        ISSUES_FOUND=$((ISSUES_FOUND + 1))
    fi

    # Get tx hash for tracking
    local txid
    txid=$("$CLI" transaction txid --tx-file "$tx_signed" 2>/dev/null || echo "unknown")
    log TX "TxID: $txid"

    rm -f "$tx_raw" "$tx_signed"
    return 0
}

submit_valid_tx_with_metadata() {
    log_section "VALID TRANSACTION WITH METADATA"
    TX_SUBMITTED=$((TX_SUBMITTED + 1))

    local addr
    addr=$(cat "$KEY_DIR/payment.addr")

    local utxo_line
    utxo_line=$(get_first_utxo 5000000) || { log WARN "Cannot query UTxOs — skipping"; return 1; }
    if [[ -z "$utxo_line" ]]; then
        log WARN "No suitable UTxO for metadata tx"
        return 1
    fi
    local first_utxo first_amount
    first_utxo=$(echo "$utxo_line" | awk '{print $1}')
    first_amount=$(echo "$utxo_line" | awk '{print $2}')

    local current_slot
    current_slot=$(get_current_slot)
    local ttl=$((current_slot + 600))

    # Create metadata JSON
    local metadata_file="/tmp/soak-metadata-$$.json"
    cat > "$metadata_file" <<METAEOF
{
    "674": {
        "msg": ["Torsten soak test tx $(date +%H:%M:%S)", "restart #$RESTART_COUNT"]
    }
}
METAEOF

    local send_amount=2000000
    local tx_raw="/tmp/soak-meta-tx-$$.raw"
    local tx_signed="/tmp/soak-meta-tx-$$.signed"

    if "$CLI" transaction build \
        --tx-in "$first_utxo" \
        --tx-out "$addr+$send_amount" \
        --change-address "$addr" \
        --ttl "$ttl" \
        --metadata-json-file "$metadata_file" \
        --socket-path "$SOCKET_PATH" \
        --testnet-magic $NETWORK_MAGIC \
        --out-file "$tx_raw" 2>&1; then
        log TX "Metadata transaction built"
    else
        log WARN "Metadata tx build failed"
        rm -f "$tx_raw" "$tx_signed" "$metadata_file"
        return 1
    fi

    if "$CLI" transaction sign \
        --tx-body-file "$tx_raw" \
        --signing-key-file "$KEY_DIR/payment.skey" \
        --out-file "$tx_signed" 2>&1; then
        log TX "Metadata transaction signed"
    else
        log WARN "Metadata tx sign failed"
        rm -f "$tx_raw" "$tx_signed" "$metadata_file"
        return 1
    fi

    local submit_output
    if submit_output=$("$CLI" transaction submit \
        --tx-file "$tx_signed" \
        --socket-path "$SOCKET_PATH" \
        --testnet-magic $NETWORK_MAGIC 2>&1); then
        log OK "VALID METADATA TX ACCEPTED: $submit_output"
        TX_VALID=$((TX_VALID + 1))
    else
        log WARN "Metadata tx rejected: $submit_output"
        ISSUES_FOUND=$((ISSUES_FOUND + 1))
    fi

    rm -f "$tx_raw" "$tx_signed" "$metadata_file"
}

submit_invalid_expired_ttl() {
    log_section "INVALID TX: EXPIRED TTL"
    TX_SUBMITTED=$((TX_SUBMITTED + 1))

    local addr
    addr=$(cat "$KEY_DIR/payment.addr")

    local utxo_line
    utxo_line=$(get_first_utxo 5000000) || { log WARN "Cannot query UTxOs — skipping"; return 1; }
    if [[ -z "$utxo_line" ]]; then
        log WARN "No UTxO for expired TTL test"
        return 1
    fi
    local first_utxo first_amount
    first_utxo=$(echo "$utxo_line" | awk '{print $1}')
    first_amount=$(echo "$utxo_line" | awk '{print $2}')

    # TTL in the past
    local expired_ttl=100

    local send_amount=2000000
    local fee=200000
    local change=$((first_amount - send_amount - fee))
    local tx_raw="/tmp/soak-expired-$$.raw"
    local tx_signed="/tmp/soak-expired-$$.signed"

    "$CLI" transaction build-raw \
        --tx-in "$first_utxo" \
        --tx-out "$addr+$send_amount" \
        --tx-out "$addr+$change" \
        --fee "$fee" \
        --ttl "$expired_ttl" \
        --out-file "$tx_raw" 2>&1 || { log WARN "build failed"; rm -f "$tx_raw"; return 1; }

    "$CLI" transaction sign \
        --tx-body-file "$tx_raw" \
        --signing-key-file "$KEY_DIR/payment.skey" \
        --out-file "$tx_signed" 2>&1 || { log WARN "sign failed"; rm -f "$tx_raw" "$tx_signed"; return 1; }

    local submit_output
    if submit_output=$("$CLI" transaction submit \
        --tx-file "$tx_signed" \
        --socket-path "$SOCKET_PATH" \
        --testnet-magic $NETWORK_MAGIC 2>&1); then
        log ERROR "EXPIRED TTL TX WAS ACCEPTED (should have been rejected!)"
        ISSUES_FOUND=$((ISSUES_FOUND + 1))
    else
        log OK "Expired TTL tx correctly rejected: $submit_output"
        TX_INVALID=$((TX_INVALID + 1))
    fi

    rm -f "$tx_raw" "$tx_signed"
}

submit_invalid_insufficient_fee() {
    log_section "INVALID TX: INSUFFICIENT FEE"
    TX_SUBMITTED=$((TX_SUBMITTED + 1))

    local addr
    addr=$(cat "$KEY_DIR/payment.addr")

    local utxo_line
    utxo_line=$(get_first_utxo 5000000) || { log WARN "Cannot query UTxOs — skipping"; return 1; }
    if [[ -z "$utxo_line" ]]; then
        log WARN "No UTxO for low-fee test"
        return 1
    fi
    local first_utxo first_amount
    first_utxo=$(echo "$utxo_line" | awk '{print $1}')
    first_amount=$(echo "$utxo_line" | awk '{print $2}')

    local current_slot
    current_slot=$(get_current_slot)
    local ttl=$((current_slot + 600))

    # Absurdly low fee
    local fee=1
    local send_amount=2000000
    local change=$((first_amount - send_amount - fee))
    local tx_raw="/tmp/soak-lowfee-$$.raw"
    local tx_signed="/tmp/soak-lowfee-$$.signed"

    "$CLI" transaction build-raw \
        --tx-in "$first_utxo" \
        --tx-out "$addr+$send_amount" \
        --tx-out "$addr+$change" \
        --fee "$fee" \
        --ttl "$ttl" \
        --out-file "$tx_raw" 2>&1 || { rm -f "$tx_raw"; return 1; }

    "$CLI" transaction sign \
        --tx-body-file "$tx_raw" \
        --signing-key-file "$KEY_DIR/payment.skey" \
        --out-file "$tx_signed" 2>&1 || { rm -f "$tx_raw" "$tx_signed"; return 1; }

    local submit_output
    if submit_output=$("$CLI" transaction submit \
        --tx-file "$tx_signed" \
        --socket-path "$SOCKET_PATH" \
        --testnet-magic $NETWORK_MAGIC 2>&1); then
        log ERROR "LOW-FEE TX WAS ACCEPTED (should have been rejected!)"
        ISSUES_FOUND=$((ISSUES_FOUND + 1))
    else
        log OK "Insufficient fee tx correctly rejected: $submit_output"
        TX_INVALID=$((TX_INVALID + 1))
    fi

    rm -f "$tx_raw" "$tx_signed"
}

submit_invalid_spent_utxo() {
    log_section "INVALID TX: ALREADY-SPENT UTxO"
    TX_SUBMITTED=$((TX_SUBMITTED + 1))

    local addr
    addr=$(cat "$KEY_DIR/payment.addr")

    local current_slot
    current_slot=$(get_current_slot)
    local ttl=$((current_slot + 600))

    # Use a fabricated/already-spent UTxO
    local fake_utxo="0000000000000000000000000000000000000000000000000000000000000000#0"

    local tx_raw="/tmp/soak-spent-$$.raw"
    local tx_signed="/tmp/soak-spent-$$.signed"

    "$CLI" transaction build-raw \
        --tx-in "$fake_utxo" \
        --tx-out "$addr+2000000" \
        --fee 200000 \
        --ttl "$ttl" \
        --out-file "$tx_raw" 2>&1 || { rm -f "$tx_raw"; return 1; }

    "$CLI" transaction sign \
        --tx-body-file "$tx_raw" \
        --signing-key-file "$KEY_DIR/payment.skey" \
        --out-file "$tx_signed" 2>&1 || { rm -f "$tx_raw" "$tx_signed"; return 1; }

    local submit_output
    if submit_output=$("$CLI" transaction submit \
        --tx-file "$tx_signed" \
        --socket-path "$SOCKET_PATH" \
        --testnet-magic $NETWORK_MAGIC 2>&1); then
        log ERROR "FAKE UTxO TX WAS ACCEPTED (should have been rejected!)"
        ISSUES_FOUND=$((ISSUES_FOUND + 1))
    else
        log OK "Spent UTxO tx correctly rejected: $submit_output"
        TX_INVALID=$((TX_INVALID + 1))
    fi

    rm -f "$tx_raw" "$tx_signed"
}

submit_invalid_no_outputs() {
    log_section "INVALID TX: NO OUTPUTS"
    TX_SUBMITTED=$((TX_SUBMITTED + 1))

    local addr
    addr=$(cat "$KEY_DIR/payment.addr")

    local utxo_line
    utxo_line=$(get_first_utxo 5000000) || { log WARN "Cannot query UTxOs — skipping"; return 1; }
    if [[ -z "$utxo_line" ]]; then
        log WARN "No UTxO for no-outputs test"
        return 1
    fi
    local first_utxo
    first_utxo=$(echo "$utxo_line" | awk '{print $1}')

    local current_slot
    current_slot=$(get_current_slot)
    local ttl=$((current_slot + 600))

    local tx_raw="/tmp/soak-noout-$$.raw"
    local tx_signed="/tmp/soak-noout-$$.signed"

    # Build with no outputs — just input and fee
    "$CLI" transaction build-raw \
        --tx-in "$first_utxo" \
        --fee 200000 \
        --ttl "$ttl" \
        --out-file "$tx_raw" 2>&1 || { rm -f "$tx_raw"; return 1; }

    "$CLI" transaction sign \
        --tx-body-file "$tx_raw" \
        --signing-key-file "$KEY_DIR/payment.skey" \
        --out-file "$tx_signed" 2>&1 || { rm -f "$tx_raw" "$tx_signed"; return 1; }

    local submit_output
    if submit_output=$("$CLI" transaction submit \
        --tx-file "$tx_signed" \
        --socket-path "$SOCKET_PATH" \
        --testnet-magic $NETWORK_MAGIC 2>&1); then
        log ERROR "NO-OUTPUTS TX WAS ACCEPTED (should have been rejected!)"
        ISSUES_FOUND=$((ISSUES_FOUND + 1))
    else
        log OK "No-outputs tx correctly rejected: $submit_output"
        TX_INVALID=$((TX_INVALID + 1))
    fi

    rm -f "$tx_raw" "$tx_signed"
}

submit_invalid_value_not_conserved() {
    log_section "INVALID TX: VALUE NOT CONSERVED"
    TX_SUBMITTED=$((TX_SUBMITTED + 1))

    local addr
    addr=$(cat "$KEY_DIR/payment.addr")

    local utxo_line
    utxo_line=$(get_first_utxo 5000000) || { log WARN "Cannot query UTxOs — skipping"; return 1; }
    if [[ -z "$utxo_line" ]]; then
        log WARN "No UTxO for value-not-conserved test"
        return 1
    fi
    local first_utxo first_amount
    first_utxo=$(echo "$utxo_line" | awk '{print $1}')
    first_amount=$(echo "$utxo_line" | awk '{print $2}')

    local current_slot
    current_slot=$(get_current_slot)
    local ttl=$((current_slot + 600))

    # Output more than input (create value from nothing)
    local inflated=$((first_amount + 999999999999))
    local fee=200000

    local tx_raw="/tmp/soak-inflate-$$.raw"
    local tx_signed="/tmp/soak-inflate-$$.signed"

    "$CLI" transaction build-raw \
        --tx-in "$first_utxo" \
        --tx-out "$addr+$inflated" \
        --fee "$fee" \
        --ttl "$ttl" \
        --out-file "$tx_raw" 2>&1 || { rm -f "$tx_raw"; return 1; }

    "$CLI" transaction sign \
        --tx-body-file "$tx_raw" \
        --signing-key-file "$KEY_DIR/payment.skey" \
        --out-file "$tx_signed" 2>&1 || { rm -f "$tx_raw" "$tx_signed"; return 1; }

    local submit_output
    if submit_output=$("$CLI" transaction submit \
        --tx-file "$tx_signed" \
        --socket-path "$SOCKET_PATH" \
        --testnet-magic $NETWORK_MAGIC 2>&1); then
        log ERROR "VALUE INFLATION TX WAS ACCEPTED (should have been rejected!)"
        ISSUES_FOUND=$((ISSUES_FOUND + 1))
    else
        log OK "Value-not-conserved tx correctly rejected: $submit_output"
        TX_INVALID=$((TX_INVALID + 1))
    fi

    rm -f "$tx_raw" "$tx_signed"
}

# Picks a random transaction test to run
run_tx_cycle() {
    local cycle=$((RANDOM % 7))
    case $cycle in
        0) submit_valid_tx ;;
        1) submit_valid_tx_with_metadata ;;
        2) submit_invalid_expired_ttl ;;
        3) submit_invalid_insufficient_fee ;;
        4) submit_invalid_spent_utxo ;;
        5) submit_invalid_no_outputs ;;
        6) submit_invalid_value_not_conserved ;;
    esac
}

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
print_summary() {
    log_section "SOAK TEST SUMMARY"
    local end_time
    end_time=$(date +%s)
    local total_sec=$((end_time - START_TIME))
    local hours=$((total_sec / 3600))
    local mins=$(( (total_sec % 3600) / 60 ))

    log INFO "Duration: ${hours}h ${mins}m"
    log INFO "Restarts: $RESTART_COUNT"
    log INFO "Transactions submitted: $TX_SUBMITTED"
    log INFO "  Valid accepted: $TX_VALID"
    log INFO "  Invalid rejected (correct): $TX_INVALID"
    log INFO "Issues found: $ISSUES_FOUND"
    log INFO "Final block height: $LAST_BLOCK_HEIGHT"
    log INFO "Report file: $REPORT_FILE"
}

# ===========================================================================
# MAIN LOOP
# ===========================================================================
log_section "SOAK TEST START"
log INFO "Duration: ${DURATION_HOURS}h | Restart interval: ${RESTART_INTERVAL_MIN}min | TX interval: ${TX_INTERVAL_MIN}min"
log INFO "Database: $DB_PATH"
log INFO "Keys: $POOL_KEY_DIR"
log INFO "Metrics: $METRICS_URL"

# Verify prerequisites
if [[ ! -x "$BIN" ]]; then
    log ERROR "Binary not found: $BIN"
    exit 1
fi
if [[ ! -x "$CLI" ]]; then
    log ERROR "CLI not found: $CLI"
    exit 1
fi
for f in kes.skey vrf.skey opcert.cert; do
    if [[ ! -f "$POOL_KEY_DIR/$f" ]]; then
        log ERROR "Missing key: $POOL_KEY_DIR/$f"
        exit 1
    fi
done

# Kill any existing torsten-node process
existing_pid=$(pgrep -f "torsten-node run" || true)
if [[ -n "$existing_pid" ]]; then
    log WARN "Killing existing torsten-node process ($existing_pid)"
    kill -TERM "$existing_pid" 2>/dev/null || true
    sleep 3
    kill -KILL "$existing_pid" 2>/dev/null || true
    rm -f "$SOCKET_PATH"
    sleep 2
fi

# Start the node
start_node || { log ERROR "Failed to start node"; exit 1; }

# Wait briefly after socket readiness for initial peer connections
log INFO "Waiting 15s for peer connections..."
sleep 15

# Initial health check
check_metrics || true
query_tip || true

# Main monitoring loop
log_section "ENTERING MAIN LOOP"
while true; do
    now=$(date +%s)
    elapsed=$((now - START_TIME))

    # Check if test duration exceeded
    if (( elapsed >= DURATION_SEC )); then
        log OK "Soak test duration reached (${DURATION_HOURS}h). Completing..."
        break
    fi

    # Check node is alive
    if ! check_node_alive; then
        log ERROR "Node crashed — restarting immediately"
        start_node || { log ERROR "Failed to restart after crash"; sleep 30; continue; }
        sleep 15
        continue
    fi

    # Periodic health checks (all guarded — must not crash the soak test)
    check_metrics || true
    check_logs_for_errors || true

    # Periodic tip query (every 5 cycles)
    if (( (elapsed / MONITOR_INTERVAL_SEC) % 5 == 0 )); then
        query_tip || true
    fi

    # Periodic restart
    since_restart=$((now - LAST_RESTART_TIME))
    if (( since_restart >= RESTART_INTERVAL_SEC )); then
        restart_node || true
        sleep 30  # Let node settle after restart
        continue  # Re-enter loop to check health
    fi

    # Periodic transaction submission
    since_tx=$((now - LAST_TX_TIME))
    if (( since_tx >= TX_INTERVAL_SEC )); then
        run_tx_cycle || true
        LAST_TX_TIME=$(date +%s)
    fi

    sleep "$MONITOR_INTERVAL_SEC"
done

# Final cleanup happens in trap
exit 0
