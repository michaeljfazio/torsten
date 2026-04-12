#!/bin/bash
# Soak test monitor for dual-node configuration (Haskell relay + Dugite BP)
# Checks both nodes every 5 minutes for 6 hours
# Logs to logs/soak-monitor.log

BP_METRICS="http://localhost:12799/metrics"
RELAY_METRICS="http://localhost:12798/metrics"
LOG="logs/soak-monitor.log"
INTERVAL=300  # 5 minutes
DURATION=21600  # 6 hours
START=$(date +%s)
END=$((START + DURATION))
CHECK=0

echo "=== Dual-Node Soak Test Started ===" | tee "$LOG"
echo "Start: $(date)" | tee -a "$LOG"
echo "Duration: ${DURATION}s (6 hours)" | tee -a "$LOG"
echo "Interval: ${INTERVAL}s" | tee -a "$LOG"
echo "BP metrics: $BP_METRICS" | tee -a "$LOG"
echo "Relay metrics: $RELAY_METRICS" | tee -a "$LOG"
echo "" | tee -a "$LOG"

prev_blocks=0
prev_slot=0
prev_forged=0
prev_leader=0

while [ $(date +%s) -lt $END ]; do
    CHECK=$((CHECK + 1))
    NOW=$(date '+%Y-%m-%d %H:%M:%S')
    ELAPSED=$(( $(date +%s) - START ))
    ELAPSED_MIN=$((ELAPSED / 60))
    ELAPSED_HR=$((ELAPSED / 3600))

    echo "--- CHECK #$CHECK at $NOW (${ELAPSED_HR}h ${ELAPSED_MIN}m) ---" | tee -a "$LOG"

    # Check processes
    BP_PID=$(pgrep -f "dugite-node run" 2>/dev/null | head -1 || echo "DEAD")
    RELAY_PID=$(pgrep -f "cardano-node run" 2>/dev/null | head -1 || echo "DEAD")
    echo "  Processes: bp=$BP_PID relay=$RELAY_PID" | tee -a "$LOG"

    if [ "$BP_PID" = "DEAD" ]; then
        echo "  !!! DUGITE BP IS DOWN !!!" | tee -a "$LOG"
    fi
    if [ "$RELAY_PID" = "DEAD" ]; then
        echo "  !!! HASKELL RELAY IS DOWN !!!" | tee -a "$LOG"
    fi

    # ── BP metrics ──
    BP_RAW=$(curl -s --max-time 5 "$BP_METRICS" 2>/dev/null)
    if [ -n "$BP_RAW" ]; then
        blocks=$(echo "$BP_RAW" | grep '^dugite_blocks_applied_total ' | awk '{print $2}')
        slot=$(echo "$BP_RAW" | grep '^dugite_slot_number ' | awk '{print $2}')
        block_no=$(echo "$BP_RAW" | grep '^dugite_block_number ' | awk '{print $2}')
        tip_age=$(echo "$BP_RAW" | grep '^dugite_tip_age_seconds ' | awk '{print $2}')
        epoch=$(echo "$BP_RAW" | grep '^dugite_epoch_number ' | awk '{print $2}')
        peers=$(echo "$BP_RAW" | grep '^dugite_peers_connected ' | awk '{print $2}')
        outbound=$(echo "$BP_RAW" | grep '^dugite_peers_outbound ' | awk '{print $2}')
        inbound=$(echo "$BP_RAW" | grep '^dugite_peers_inbound ' | awk '{print $2}')
        duplex=$(echo "$BP_RAW" | grep '^dugite_peers_duplex ' | awk '{print $2}')
        forged=$(echo "$BP_RAW" | grep '^dugite_blocks_forged_total ' | awk '{print $2}')
        forge_fail=$(echo "$BP_RAW" | grep '^dugite_forge_failures_total ' | awk '{print $2}')
        leader_checks=$(echo "$BP_RAW" | grep '^dugite_leader_checks_total ' | awk '{print $2}')
        announced=$(echo "$BP_RAW" | grep '^dugite_blocks_announced_total ' | awk '{print $2}')
        n2n_in=$(echo "$BP_RAW" | grep '^dugite_n2n_connections_total ' | awk '{print $2}')
        rollbacks=$(echo "$BP_RAW" | grep '^dugite_rollback_count_total ' | awk '{print $2}')
        mempool_tx=$(echo "$BP_RAW" | grep '^dugite_mempool_tx_count ' | awk '{print $2}')
        sync=$(echo "$BP_RAW" | grep '^dugite_sync_progress_percent ' | awk '{printf "%.1f", $2/100}')
        mem_gb=$(echo "$BP_RAW" | grep '^dugite_mem_resident_bytes ' | awk '{printf "%.1f", $2/1073741824}')

        # Deltas
        new_blocks=$((${blocks:-0} - ${prev_blocks:-0}))
        new_forged=$((${forged:-0} - ${prev_forged:-0}))
        new_leader=$((${leader_checks:-0} - ${prev_leader:-0}))
        prev_blocks=${blocks:-0}
        prev_forged=${forged:-0}
        prev_leader=${leader_checks:-0}

        # Health
        STATUS="OK"
        if [ "${peers:-0}" -eq 0 ]; then STATUS="WARN:no_peers"; fi
        if [ -n "$tip_age" ] && [ "${tip_age%.*}" -gt 600 ] 2>/dev/null && [ "$CHECK" -gt 2 ]; then STATUS="WARN:tip_behind(${tip_age}s)"; fi
        if [ -n "$tip_age" ] && [ "${tip_age%.*}" -gt 3600 ] 2>/dev/null && [ "$CHECK" -gt 4 ]; then STATUS="CRITICAL:tip_stale(${tip_age}s)"; fi

        echo "  BP Status: $STATUS" | tee -a "$LOG"
        echo "  BP Chain:  slot=${slot} block=${block_no} epoch=${epoch} sync=${sync}% tip_age=${tip_age}s" | tee -a "$LOG"
        echo "  BP Blocks: applied=${blocks}(+${new_blocks}) forged=${forged}(+${new_forged}) announced=${announced} rollbacks=${rollbacks}" | tee -a "$LOG"
        echo "  BP Forge:  leader_checks=${leader_checks}(+${new_leader}) forge_failures=${forge_fail}" | tee -a "$LOG"
        echo "  BP Peers:  total=${peers} out=${outbound} in=${inbound} duplex=${duplex} n2n_accepted=${n2n_in}" | tee -a "$LOG"
        echo "  BP System: mem=${mem_gb}GB mempool=${mempool_tx}" | tee -a "$LOG"
    else
        echo "  BP: metrics unreachable" | tee -a "$LOG"
    fi

    # ── Relay metrics (Haskell prometheus) ──
    RELAY_RAW=$(curl -s --max-time 5 "$RELAY_METRICS" 2>/dev/null)
    if [ -n "$RELAY_RAW" ]; then
        r_slot=$(echo "$RELAY_RAW" | grep 'cardano_node_metrics_slotNum_int' | grep -v '#' | awk '{print $2}')
        r_block=$(echo "$RELAY_RAW" | grep 'cardano_node_metrics_blockNum_int' | grep -v '#' | awk '{print $2}')
        r_peers=$(echo "$RELAY_RAW" | grep 'cardano_node_metrics_connectedPeers_int' | grep -v '#' | awk '{print $2}')
        r_epoch=$(echo "$RELAY_RAW" | grep 'cardano_node_metrics_epoch_int' | grep -v '#' | awk '{print $2}')
        echo "  Relay:  slot=${r_slot} block=${r_block} epoch=${r_epoch} peers=${r_peers}" | tee -a "$LOG"

        # Check if relay is caught up to BP
        if [ -n "$slot" ] && [ -n "$r_slot" ] && [ "${r_slot:-0}" -gt 0 ] && [ "${slot:-0}" -gt 0 ]; then
            SLOT_DIFF=$((${slot:-0} - ${r_slot:-0}))
            echo "  Gap:    relay_behind_bp=${SLOT_DIFF} slots" | tee -a "$LOG"
        fi
    else
        echo "  Relay: metrics unreachable (may still be starting/syncing)" | tee -a "$LOG"
    fi

    # Check BP log for errors
    if [ -f logs/bp-test.log ]; then
        BP_ERRORS=$(grep -c "ERROR\|PANIC\|panic\|FATAL" logs/bp-test.log 2>/dev/null || echo "0")
        echo "  BP Errors: $BP_ERRORS in log" | tee -a "$LOG"
    fi

    # Check relay log for errors and BP connection
    if [ -f logs/relay.log ]; then
        RELAY_BP_REFS=$(grep -c "127.0.0.1:3002" logs/relay.log 2>/dev/null || echo "0")
        echo "  Relay->BP: $RELAY_BP_REFS log references to BP address" | tee -a "$LOG"
    fi

    echo "" | tee -a "$LOG"
    sleep "$INTERVAL"
done

echo "=== Dual-Node Soak Test Complete ===" | tee -a "$LOG"
echo "End: $(date)" | tee -a "$LOG"
echo "Total checks: $CHECK" | tee -a "$LOG"
echo "" | tee -a "$LOG"

# Final summary
echo "=== FINAL SUMMARY ===" | tee -a "$LOG"
if BP_RAW=$(curl -s --max-time 5 "$BP_METRICS" 2>/dev/null); then
    forged=$(echo "$BP_RAW" | grep '^dugite_blocks_forged_total ' | awk '{print $2}')
    forge_fail=$(echo "$BP_RAW" | grep '^dugite_forge_failures_total ' | awk '{print $2}')
    leader_checks=$(echo "$BP_RAW" | grep '^dugite_leader_checks_total ' | awk '{print $2}')
    announced=$(echo "$BP_RAW" | grep '^dugite_blocks_announced_total ' | awk '{print $2}')
    n2n_in=$(echo "$BP_RAW" | grep '^dugite_n2n_connections_total ' | awk '{print $2}')
    echo "  Blocks forged: $forged" | tee -a "$LOG"
    echo "  Forge failures: $forge_fail" | tee -a "$LOG"
    echo "  Leader checks: $leader_checks" | tee -a "$LOG"
    echo "  Blocks announced: $announced" | tee -a "$LOG"
    echo "  N2N inbound connections: $n2n_in" | tee -a "$LOG"
fi
