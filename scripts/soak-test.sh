#!/usr/bin/env bash
# Torsten Node 3-Hour Transaction Submission Soak Test
# Runs every 10 minutes for 3 hours (18 cycles)
# Tests: single tx submission, batch submission (every 30 min), node restart (at 1 hour)
# All results logged to /tmp/soak-test-results.log

set -euo pipefail

# ── Configuration ───────────────────────────────────────────────────────────
TORSTEN_ROOT="/Users/michaelfazio/Source/torsten"
CLI="$TORSTEN_ROOT/target/release/torsten-cli"
NODE_BIN="$TORSTEN_ROOT/target/release/torsten-node"
SOCKET="$TORSTEN_ROOT/node.sock"
PAYMENT_ADDR="addr_test1qzultc5n46dl02n6v5f4zwhgf623m99ngda8cegynr7aaqtvnhgsytsr9zrwaz79h74dhlm4s3hy8dd9v0mzcye6l90setqluv"
PAYMENT_SKEY="$TORSTEN_ROOT/keys/preview-test/payment.skey"
METRICS_URL="http://localhost:12798/metrics"
KEY_DIR="$TORSTEN_ROOT/keys/preview-test/pool"
LOG_FILE="/tmp/soak-test-results.log"
NODE_LOG="$TORSTEN_ROOT/logs/torsten-node.log"
TESTNET_MAGIC=2
FEE=200000  # 0.2 ADA fixed fee
SOAK_CYCLE=10      # minutes between cycles
SOAK_DURATION=180  # total minutes (3 hours)
RESTART_AT_CYCLE=7 # cycle index (0-based) at which to do restart test (~70 min)

# ── State tracking ───────────────────────────────────────────────────────────
CYCLE=0
TOTAL_CYCLES=$(( SOAK_DURATION / SOAK_CYCLE ))
SUBMITTED=0
CONFIRMED=0
FAILED=0
REJECTED=0
TOTAL_CONFIRM_SECS=0
CONFIRM_COUNT=0

# Array of submitted tx hashes and their submission times (written to temp files)
TX_TRACKING_DIR="/tmp/soak-tx-tracking"
mkdir -p "$TX_TRACKING_DIR"

# ── Utility functions ────────────────────────────────────────────────────────

log() {
    local ts; ts=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
    echo "[$ts] $*" | tee -a "$LOG_FILE"
}

log_section() {
    echo "" | tee -a "$LOG_FILE"
    echo "════════════════════════════════════════════════════════════" | tee -a "$LOG_FILE"
    echo "  $*" | tee -a "$LOG_FILE"
    echo "════════════════════════════════════════════════════════════" | tee -a "$LOG_FILE"
}

get_metrics() {
    curl -sf "$METRICS_URL" 2>/dev/null || echo ""
}

metric_value() {
    local name="$1"
    get_metrics | grep "^torsten_${name} " | awk '{print $2}'
}

get_node_tip_slot() {
    "$CLI" query tip --socket-path "$SOCKET" --testnet-magic $TESTNET_MAGIC 2>/dev/null \
        | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('slot',0))" 2>/dev/null || echo "0"
}

get_node_tip_block() {
    "$CLI" query tip --socket-path "$SOCKET" --testnet-magic $TESTNET_MAGIC 2>/dev/null \
        | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('block',0))" 2>/dev/null || echo "0"
}

# Build and submit a single self-to-self transaction
# Args: $1=tx_in_hash, $2=tx_in_index, $3=input_value_lovelace, $4=tx_label
submit_tx() {
    local tx_in_hash="$1"
    local tx_in_ix="$2"
    local input_value="$3"
    local label="$4"
    local out_file="/tmp/soak-tx-${label}.raw"
    local signed_file="/tmp/soak-tx-${label}.signed"

    local change=$(( input_value - FEE ))
    if (( change <= 0 )); then
        log "  ERROR: input value $input_value too small for fee $FEE (label=$label)"
        return 1
    fi

    # Get TTL from node tip
    local tip_slot; tip_slot=$(get_node_tip_slot)
    if [[ "$tip_slot" == "0" ]]; then
        log "  ERROR: could not get tip slot for TTL"
        return 1
    fi
    local ttl=$(( tip_slot + 600 ))

    # Build
    if ! "$CLI" transaction build-raw \
        --tx-in "${tx_in_hash}#${tx_in_ix}" \
        --tx-out "${PAYMENT_ADDR}+${change}" \
        --fee $FEE \
        --ttl $ttl \
        --out-file "$out_file" 2>/tmp/soak-build-err.txt; then
        log "  ERROR: transaction build-raw failed: $(cat /tmp/soak-build-err.txt)"
        return 1
    fi

    # Sign
    if ! "$CLI" transaction sign \
        --tx-body-file "$out_file" \
        --signing-key-file "$PAYMENT_SKEY" \
        --out-file "$signed_file" 2>/tmp/soak-sign-err.txt; then
        log "  ERROR: transaction sign failed: $(cat /tmp/soak-sign-err.txt)"
        return 1
    fi

    # Get tx hash (from signed file)
    local tx_hash
    tx_hash=$("$CLI" transaction txid --tx-file "$signed_file" 2>/dev/null || echo "unknown")

    # Submit
    local submit_time; submit_time=$(date +%s)
    local submit_out
    if submit_out=$("$CLI" transaction submit \
        --tx-file "$signed_file" \
        --socket-path "$SOCKET" \
        --testnet-magic $TESTNET_MAGIC 2>&1); then
        log "  SUBMITTED: tx=$tx_hash in=${tx_in_hash}#${tx_in_ix} change=${change} ttl=$ttl"
        # Record for confirmation tracking
        echo "$tx_hash $submit_time" > "$TX_TRACKING_DIR/${label}.pending"
        SUBMITTED=$(( SUBMITTED + 1 ))
        echo "$tx_hash"
        return 0
    else
        log "  REJECTED: tx=$tx_hash error='$submit_out'"
        REJECTED=$(( REJECTED + 1 ))
        return 1
    fi
}

# Check confirmation of a pending tx via Koios tx_status endpoint
# Args: $1=tx_hash, $2=submit_timestamp
# Returns 0 if confirmed, 1 if still pending/unknown
check_confirmation() {
    local tx_hash="$1"
    local submit_ts="$2"
    local now; now=$(date +%s)

    # Query Koios tx_status
    local status_json
    status_json=$(curl -sf \
        "https://preview.koios.rest/api/v1/tx_status?select=tx_hash,num_confirmations" \
        -H "Content-Type: application/json" \
        -d "{\"_tx_hashes\":[\"$tx_hash\"]}" 2>/dev/null || echo "[]")

    local confirmations
    confirmations=$(echo "$status_json" | python3 -c "
import sys, json
data = json.load(sys.stdin)
if data and isinstance(data, list) and data[0]:
    print(data[0].get('num_confirmations', 0) or 0)
else:
    print(0)
" 2>/dev/null || echo "0")

    if (( confirmations > 0 )); then
        local elapsed=$(( now - submit_ts ))
        log "  CONFIRMED: tx=$tx_hash confirmations=$confirmations elapsed=${elapsed}s"
        CONFIRMED=$(( CONFIRMED + 1 ))
        TOTAL_CONFIRM_SECS=$(( TOTAL_CONFIRM_SECS + elapsed ))
        CONFIRM_COUNT=$(( CONFIRM_COUNT + 1 ))
        return 0
    else
        local elapsed=$(( now - submit_ts ))
        log "  PENDING: tx=$tx_hash elapsed=${elapsed}s confirmations=0"
        return 1
    fi
}

# Health check: metrics + tip comparison
health_check() {
    log_section "HEALTH CHECK — Cycle $CYCLE / $TOTAL_CYCLES"

    # Node metrics
    local slot; slot=$(metric_value "slot_number")
    local block; block=$(metric_value "block_number")
    local epoch; epoch=$(metric_value "epoch_number")
    local peers; peers=$(metric_value "peers_connected")
    local forged; forged=$(metric_value "blocks_forged_total")
    local applied; applied=$(metric_value "blocks_applied_total")
    local mempool_tx; mempool_tx=$(metric_value "mempool_tx_count")
    local sync; sync=$(metric_value "sync_progress_percent")
    local rollbacks; rollbacks=$(metric_value "rollback_count_total")
    local leader_checks; leader_checks=$(metric_value "leader_checks_total")
    local not_elected; not_elected=$(metric_value "leader_checks_not_elected_total")
    local tx_validated; tx_validated=$(metric_value "transactions_validated_total")
    local tx_rejected; tx_rejected=$(metric_value "transactions_rejected_total")

    log "  Metrics: slot=$slot block=$block epoch=$epoch peers=$peers forged=$forged applied=$applied"
    log "  Metrics: mempool_tx=$mempool_tx sync=$sync rollbacks=$rollbacks"
    log "  Metrics: leader_checks=$leader_checks not_elected=$not_elected"
    log "  Metrics: tx_validated=$tx_validated tx_rejected=$tx_rejected"

    # Node CLI tip
    local node_slot; node_slot=$(get_node_tip_slot)
    local node_block; node_block=$(get_node_tip_block)
    log "  NodeCLI: slot=$node_slot block=$node_block"

    # Log file size check
    if [[ -f "$NODE_LOG" ]]; then
        local log_size_mb; log_size_mb=$(du -m "$NODE_LOG" 2>/dev/null | awk '{print $1}')
        log "  NodeLog: size=${log_size_mb}MB path=$NODE_LOG"
        if (( log_size_mb > 100 )); then
            log "  WARNING: node log > 100MB"
        fi
    fi

    # Tip gap (seconds): Cardano preview produces 1 block per ~20s average
    # Each slot is 1 second; compare node slot vs Koios slot
    # We compare against last known koios slot (passed as arg)
    local koios_slot="${1:-0}"
    if [[ "$koios_slot" != "0" && "$node_slot" != "0" ]]; then
        local gap=$(( koios_slot - node_slot ))
        if (( gap < 0 )); then gap=$(( -gap )); fi
        log "  TipGap: ${gap}s (node=$node_slot koios=$koios_slot)"
        if (( gap > 120 )); then
            log "  WARNING: tip gap ${gap}s > 120s threshold"
        fi
    fi

    # Memory usage
    local mem_kb
    mem_kb=$(ps -o rss= -p 80373 2>/dev/null || echo "0")
    local mem_mb=$(( mem_kb / 1024 ))
    log "  Memory(PID=80373): ${mem_mb}MB RSS"

    # Cumulative soak stats
    local avg_confirm=0
    if (( CONFIRM_COUNT > 0 )); then
        avg_confirm=$(( TOTAL_CONFIRM_SECS / CONFIRM_COUNT ))
    fi
    log "  SoakStats: submitted=$SUBMITTED confirmed=$CONFIRMED failed=$FAILED rejected=$REJECTED avg_confirm=${avg_confirm}s"
}

# ── Soak test UTxO pool management ───────────────────────────────────────────
# We have 21 UTxOs at 2ADA each. We track which are spent (submitted but not yet
# confirmed on Koios) to avoid double-spend. Each cycle we pick the next unspent UTxO.

# These are populated at startup from Koios
declare -a UTXO_HASHES
declare -a UTXO_INDICES
declare -a UTXO_VALUES
declare -a UTXO_SPENT  # "0"=available, "1"=spent (in-flight or confirmed)
UTXO_COUNT=0
UTXO_PTR=0  # next UTxO to use

load_utxos() {
    log "Loading UTxOs from Koios..."
    # Parse from the statically known list (fetched at test start)
    # Format: hash index value
    local utxo_list=(
        "3fc57ced2a11c2d36cd97157724df472a7fa7fe5615c3c5afef3ea7a80eaf331 0 2000000"
        "13e3e959e83c97b6cb3399bad7b97f404f64b4cbfedfdea2b8600d8a31c5d3ec 0 2000000"
        "18c92acc409af916c15d0a428dd7c335ba0420b7a19f0eaf8242b0666b3b52e3 0 2000000"
        "54a36730d64b8fbf6cd1e0f59b28c919ec9ed7694265e407a83a45e1f91d9819 0 2000000"
        "d33bccf6c78055c5517202f6805b2d8be773e1c61db3eb244fff8d3f74c945df 0 2000000"
        "9f6a9d4e1fa989c45635486917da4309f860a5a0102671e40daa03755562b3f1 0 2000000"
        "5f30c2acc1eb8d8fa61d86be850a5d2690abc9571165858f02486b01106d3092 0 2000000"
        "8e9e58c575f973531c93011a5bf764b7698b6903e2b8ba097cc996cdb405abcc 0 2000000"
        "cc73e990b389fb7b7cbcbbc09661d94e48583a38e80eb343b4208b1081919dd0 0 2000000"
        "bac6834153cebaf76a3d6871399b65027ccc552029ad2ee0c12ca7bbf4c62021 0 2000000"
        "65ce3185d4958121af2a26b5c8abaa250dc9182bbed482e8b60f8813fd02d423 0 2000000"
        "2f2991bb21186d9c79624cf7e1cdd3439129251ba2f2a98adefe8af04854b224 0 2000000"
        "951ff73e8b8839fe79ff323fa0e1d4e5c284c2ae1364ac4515d4ddd831c39074 0 2000000"
        "5157dcd15fb1a5336fa44668dc56784e2db58af290bd240a0b514eb7bfa94adf 0 2000000"
        "36373a586ff94d889e215ea3260e98a6bde47f0c0b4093f1a0fde47923968d5a 0 2000000"
        "7d0e848745e65ecd4d5eb6a3cbeefd3f93c8106a1044c55455dbdd589599f3f1 0 2000000"
        "cfeb5091e871aee669a819a4207413d439734ef88c3a377c76194f839966b8b1 0 2000000"
        "8920a007a8989cac4f46454d01bfc36a65b023e34e4e7859746ed7437bb0cce0 0 2000000"
        "2d84ddb91a5fe4f8b9a22335f7a4837c4f5629564be5c2a36cc52da6f193c7ff 0 2000000"
        "1419f3f833773a1e82aab024d500deae7b84cbfc51c5273d08d056539d44d776 0 2000000"
    )

    # Also add: the output of each submitted tx becomes a new 1.8ADA UTxO
    # These will be discovered dynamically from Koios after confirmation.

    UTXO_COUNT=${#utxo_list[@]}
    for i in "${!utxo_list[@]}"; do
        read -r h ix v <<< "${utxo_list[$i]}"
        UTXO_HASHES[$i]="$h"
        UTXO_INDICES[$i]="$ix"
        UTXO_VALUES[$i]="$v"
        UTXO_SPENT[$i]="0"
    done
    log "Loaded $UTXO_COUNT UTxOs"
}

# Get next available UTxO index, refreshing from Koios if needed
next_utxo() {
    # First try to find an unspent one in our static list
    local attempts=0
    while (( attempts < UTXO_COUNT )); do
        local idx=$(( UTXO_PTR % UTXO_COUNT ))
        UTXO_PTR=$(( UTXO_PTR + 1 ))
        if [[ "${UTXO_SPENT[$idx]}" == "0" ]]; then
            echo "$idx"
            return 0
        fi
        attempts=$(( attempts + 1 ))
    done

    # All static UTxOs spent; try to refresh from Koios to get change outputs
    log "  All static UTxOs spent, refreshing from Koios..."
    refresh_utxos_from_koios
    # Try once more
    for i in "${!UTXO_HASHES[@]}"; do
        if [[ "${UTXO_SPENT[$i]}" == "0" ]]; then
            echo "$i"
            return 0
        fi
    done
    log "  ERROR: no spendable UTxOs available"
    return 1
}

# Re-query Koios for fresh UTxOs and add any new ones not already tracked
refresh_utxos_from_koios() {
    local raw_utxos
    raw_utxos=$(curl -sf \
        "https://preview.koios.rest/api/v1/address_utxos" \
        -H "Content-Type: application/json" \
        -d "{\"_addresses\":[\"$PAYMENT_ADDR\"]}" 2>/dev/null || echo "[]")

    local new_count=0
    while IFS= read -r line; do
        local h ix v
        h=$(echo "$line" | python3 -c "import sys,json; d=json.loads(sys.stdin.read()); print(d.get('tx_hash',''))" 2>/dev/null)
        ix=$(echo "$line" | python3 -c "import sys,json; d=json.loads(sys.stdin.read()); print(d.get('tx_index',0))" 2>/dev/null)
        v=$(echo "$line" | python3 -c "import sys,json; d=json.loads(sys.stdin.read()); print(d.get('value',0))" 2>/dev/null)
        [[ -z "$h" ]] && continue

        # Check if already tracked
        local found=0
        for i in "${!UTXO_HASHES[@]}"; do
            if [[ "${UTXO_HASHES[$i]}" == "$h" && "${UTXO_INDICES[$i]}" == "$ix" ]]; then
                found=1
                break
            fi
        done
        if [[ "$found" == "0" ]]; then
            UTXO_HASHES[$UTXO_COUNT]="$h"
            UTXO_INDICES[$UTXO_COUNT]="$ix"
            UTXO_VALUES[$UTXO_COUNT]="$v"
            UTXO_SPENT[$UTXO_COUNT]="0"
            UTXO_COUNT=$(( UTXO_COUNT + 1 ))
            new_count=$(( new_count + 1 ))
        fi
    done < <(echo "$raw_utxos" | python3 -c "
import sys,json
data = json.load(sys.stdin)
for item in data:
    print(json.dumps(item))
" 2>/dev/null)

    log "  Refreshed UTxOs from Koios: $new_count new, total=$UTXO_COUNT"
}

# Mark a UTxO as spent
mark_utxo_spent() {
    local tx_hash="$1"
    local tx_ix="$2"
    for i in "${!UTXO_HASHES[@]}"; do
        if [[ "${UTXO_HASHES[$i]}" == "$tx_hash" && "${UTXO_INDICES[$i]}" == "$tx_ix" ]]; then
            UTXO_SPENT[$i]="1"
            return 0
        fi
    done
}

# Add a newly created UTxO (change output from confirmed tx)
add_change_utxo() {
    local tx_hash="$1"
    local value="$2"
    UTXO_HASHES[$UTXO_COUNT]="$tx_hash"
    UTXO_INDICES[$UTXO_COUNT]="0"
    UTXO_VALUES[$UTXO_COUNT]="$value"
    UTXO_SPENT[$UTXO_COUNT]="0"
    UTXO_COUNT=$(( UTXO_COUNT + 1 ))
    log "  Added change UTxO: $tx_hash#0 value=$value total_utxos=$UTXO_COUNT"
}

# ── Node restart logic ────────────────────────────────────────────────────────
restart_node() {
    log_section "NODE RESTART TEST"
    log "  Killing existing torsten-node processes..."
    pkill -f torsten-node || true
    sleep 3

    # Verify stopped
    if pgrep -f torsten-node > /dev/null 2>&1; then
        log "  WARNING: processes still running after pkill, sending SIGKILL"
        pkill -9 -f torsten-node || true
        sleep 2
    fi

    # Remove socket
    rm -f "$SOCKET"
    log "  Removed socket $SOCKET"

    # Start node
    local restart_ts; restart_ts=$(date +%s)
    log "  Starting node at $(date -u +%Y-%m-%dT%H:%M:%SZ)..."
    "$NODE_BIN" run \
        --config "$TORSTEN_ROOT/config/preview-config.json" \
        --topology "$TORSTEN_ROOT/config/preview-topology.json" \
        --database-path "$TORSTEN_ROOT/db-preview" \
        --socket-path "$SOCKET" \
        --host-addr 0.0.0.0 --port 3001 \
        --shelley-kes-key "$KEY_DIR/kes.skey" \
        --shelley-vrf-key "$KEY_DIR/vrf.skey" \
        --shelley-operational-certificate "$KEY_DIR/opcert.cert" \
        >> "$NODE_LOG" 2>&1 &
    local new_pid=$!
    log "  Node started with PID=$new_pid"

    # Wait up to 30s for socket to appear and node to reach tip
    local waited=0
    while (( waited < 30 )); do
        if [[ -S "$SOCKET" ]]; then
            log "  Socket appeared after ${waited}s"
            break
        fi
        sleep 1
        waited=$(( waited + 1 ))
    done

    if [[ ! -S "$SOCKET" ]]; then
        log "  ERROR: socket did not appear within 30s after restart"
        FAILED=$(( FAILED + 1 ))
        return 1
    fi

    # Give node a few more seconds to connect peers and reach tip
    sleep 5

    # Verify tip
    local node_slot; node_slot=$(get_node_tip_slot)
    local elapsed=$(( $(date +%s) - restart_ts ))
    log "  Post-restart tip: slot=$node_slot elapsed=${elapsed}s"

    if [[ "$node_slot" != "0" ]]; then
        log "  RESTART SUCCESS: node responsive in ${elapsed}s"
    else
        log "  WARNING: node socket exists but tip query returned 0"
    fi

    # Submit a tx immediately after restart
    log "  Submitting post-restart tx..."
    local idx
    if idx=$(next_utxo 2>/dev/null); then
        local h="${UTXO_HASHES[$idx]}"
        local ix="${UTXO_INDICES[$idx]}"
        local v="${UTXO_VALUES[$idx]}"
        mark_utxo_spent "$h" "$ix"
        submit_tx "$h" "$ix" "$v" "restart-verify" || log "  Post-restart tx submission failed"
    else
        log "  No UTxOs available for post-restart tx"
    fi
}

# ── Pending tx confirmation checker ─────────────────────────────────────────
# Checks all .pending files older than 3 minutes
check_pending_confirmations() {
    local now; now=$(date +%s)
    local pending_count=0
    local checked=0

    for f in "$TX_TRACKING_DIR"/*.pending; do
        [[ -f "$f" ]] || continue
        pending_count=$(( pending_count + 1 ))
        read -r tx_hash submit_ts < "$f"
        local age=$(( now - submit_ts ))

        # Check if old enough (>= 3 min)
        if (( age >= 180 )); then
            checked=$(( checked + 1 ))
            if check_confirmation "$tx_hash" "$submit_ts"; then
                rm -f "$f"
                # Add change UTxO
                local change=$(( 2000000 - FEE ))
                add_change_utxo "$tx_hash" "$change"
            elif (( age >= 300 )); then
                # >5 min: mark as failed
                log "  TIMEOUT: tx=$tx_hash age=${age}s — marking as failed"
                FAILED=$(( FAILED + 1 ))
                rm -f "$f"
            fi
        fi
    done

    if (( pending_count > 0 )); then
        log "  Pending txs: total=$pending_count checked=$checked"
    fi
}

# ── Batch submission (every 30 minutes) ──────────────────────────────────────
submit_batch() {
    local batch_size="$1"
    log_section "BATCH SUBMISSION — $batch_size txs"

    local batch_hashes=()
    for b in $(seq 1 "$batch_size"); do
        local idx
        if idx=$(next_utxo 2>/dev/null); then
            local h="${UTXO_HASHES[$idx]}"
            local ix="${UTXO_INDICES[$idx]}"
            local v="${UTXO_VALUES[$idx]}"
            mark_utxo_spent "$h" "$ix"
            local batch_hash
            if batch_hash=$(submit_tx "$h" "$ix" "$v" "batch-${CYCLE}-${b}"); then
                batch_hashes+=("$batch_hash")
            fi
            sleep 0.5  # slight spacing to avoid mempool conflicts
        else
            log "  No UTxO available for batch tx $b"
        fi
    done

    log "  Batch submitted: ${#batch_hashes[@]}/$batch_size txs"
}

# ── Single-tx cycle ───────────────────────────────────────────────────────────
run_single_tx_cycle() {
    log_section "TX SUBMISSION CYCLE $CYCLE"

    local idx
    if ! idx=$(next_utxo 2>/dev/null); then
        log "  WARNING: no UTxOs available — skipping tx submission this cycle"
        FAILED=$(( FAILED + 1 ))
        return
    fi

    local h="${UTXO_HASHES[$idx]}"
    local ix="${UTXO_INDICES[$idx]}"
    local v="${UTXO_VALUES[$idx]}"
    mark_utxo_spent "$h" "$ix"

    submit_tx "$h" "$ix" "$v" "cycle-${CYCLE}" || log "  Cycle $CYCLE tx submission failed"
}

# ── Hourly report ────────────────────────────────────────────────────────────
hourly_report() {
    local hour="$1"
    log_section "=== HOURLY REPORT: HOUR $hour ==="
    local avg_confirm=0
    if (( CONFIRM_COUNT > 0 )); then
        avg_confirm=$(( TOTAL_CONFIRM_SECS / CONFIRM_COUNT ))
    fi
    log "  Hour $hour Summary:"
    log "    Submitted:   $SUBMITTED"
    log "    Confirmed:   $CONFIRMED"
    log "    Failed:      $FAILED"
    log "    Rejected:    $REJECTED"
    log "    Avg confirm: ${avg_confirm}s"

    # Current metrics snapshot
    local applied; applied=$(metric_value "blocks_applied_total")
    local forged; forged=$(metric_value "blocks_forged_total")
    local peers; peers=$(metric_value "peers_connected")
    local rollbacks; rollbacks=$(metric_value "rollback_count_total")
    log "    Blocks applied: $applied  Forged: $forged  Peers: $peers  Rollbacks: $rollbacks"

    # Recent log errors
    local error_count; error_count=$(grep -c " ERROR " "$NODE_LOG" 2>/dev/null || echo "0")
    local warn_count; warn_count=$(grep -c " WARN " "$NODE_LOG" 2>/dev/null || echo "0")
    log "    Log: ERRORs=$error_count WARNs=$warn_count"
}

# ── MAIN LOOP ────────────────────────────────────────────────────────────────

# Initialize
echo "" > "$LOG_FILE"  # start fresh log
log_section "TORSTEN NODE SOAK TEST — 3 HOURS"
log "Start time: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
log "Config: $SOAK_DURATION min total, $SOAK_CYCLE min cycles, $TOTAL_CYCLES total cycles"
log "Socket: $SOCKET"
log "Address: $PAYMENT_ADDR"

load_utxos

# Get initial Koios tip slot for gap tracking
KOIOS_SLOT=107921360  # from startup check

log_section "INITIAL STATE"
health_check "$KOIOS_SLOT"

SOAK_START_TS=$(date +%s)
NEXT_CYCLE_TS="$SOAK_START_TS"

while (( CYCLE < TOTAL_CYCLES )); do
    # Wait until it's time for this cycle
    local_now=$(date +%s)
    if (( local_now < NEXT_CYCLE_TS )); then
        local sleep_secs=$(( NEXT_CYCLE_TS - local_now ))
        log "Sleeping ${sleep_secs}s until cycle $CYCLE..."
        sleep "$sleep_secs"
    fi

    NEXT_CYCLE_TS=$(( NEXT_CYCLE_TS + SOAK_CYCLE * 60 ))
    local_now=$(date +%s)

    # Check pending confirmations first
    check_pending_confirmations

    # Health check (update Koios slot first)
    KOIOS_SLOT=$(curl -sf "https://preview.koios.rest/api/v1/tip" 2>/dev/null \
        | python3 -c "import sys,json; d=json.load(sys.stdin); print(d[0].get('abs_slot',0) if d else 0)" 2>/dev/null || echo "$KOIOS_SLOT")
    health_check "$KOIOS_SLOT"

    # Special: restart at cycle $RESTART_AT_CYCLE
    if (( CYCLE == RESTART_AT_CYCLE )); then
        restart_node
    fi

    # Batch submission every 30 minutes (every 3rd cycle starting from cycle 3)
    local elapsed_min=$(( (local_now - SOAK_START_TS) / 60 ))
    if (( elapsed_min > 0 && elapsed_min % 30 == 0 )); then
        submit_batch 4
    else
        run_single_tx_cycle
    fi

    # Hourly reports
    if (( (CYCLE + 1) % 6 == 0 )); then
        hourly_report $(( (CYCLE + 1) / 6 ))
    fi

    CYCLE=$(( CYCLE + 1 ))
done

# ── FINAL REPORT ─────────────────────────────────────────────────────────────
log_section "=== FINAL SOAK TEST REPORT ==="
SOAK_END_TS=$(date +%s)
TOTAL_DURATION=$(( SOAK_END_TS - SOAK_START_TS ))

# Check all remaining pending
sleep 60
check_pending_confirmations

log "Duration: ${TOTAL_DURATION}s ($(( TOTAL_DURATION / 60 ))m)"
log "Submitted:   $SUBMITTED"
log "Confirmed:   $CONFIRMED"
log "Failed:      $FAILED"
log "Rejected:    $REJECTED"
local avg_confirm=0
if (( CONFIRM_COUNT > 0 )); then
    avg_confirm=$(( TOTAL_CONFIRM_SECS / CONFIRM_COUNT ))
fi
log "Avg confirm: ${avg_confirm}s"

# Final metrics
local applied; applied=$(metric_value "blocks_applied_total")
local forged; forged=$(metric_value "blocks_forged_total")
local peers; peers=$(metric_value "peers_connected")
local rollbacks; rollbacks=$(metric_value "rollback_count_total")
local leader_checks; leader_checks=$(metric_value "leader_checks_total")
local not_elected; not_elected=$(metric_value "leader_checks_not_elected_total")
log "Final metrics: blocks_applied=$applied forged=$forged peers=$peers rollbacks=$rollbacks"
log "Final metrics: leader_checks=$leader_checks not_elected=$not_elected"

# Error summary from log
local error_lines; error_lines=$(grep " ERROR " "$NODE_LOG" 2>/dev/null | tail -20 || echo "none")
log "Recent ERROR lines from node log:"
echo "$error_lines" | tee -a "$LOG_FILE"

log "Soak test COMPLETE."
