#!/bin/bash
# Soak test monitor for Torsten node
# Checks node health every 5 minutes for 4 hours
# Logs to /tmp/torsten-soak.log

METRICS_URL="http://localhost:12798/metrics"
LOG="/tmp/torsten-soak.log"
INTERVAL=300  # 5 minutes
DURATION=14400  # 4 hours
START=$(date +%s)
END=$((START + DURATION))
CHECK=0

echo "=== Torsten Soak Test Started ===" | tee "$LOG"
echo "Start: $(date)" | tee -a "$LOG"
echo "Duration: ${DURATION}s (4 hours)" | tee -a "$LOG"
echo "Interval: ${INTERVAL}s" | tee -a "$LOG"
echo "" | tee -a "$LOG"

prev_blocks=0
prev_slot=0

while [ $(date +%s) -lt $END ]; do
    CHECK=$((CHECK + 1))
    NOW=$(date '+%Y-%m-%d %H:%M:%S')
    ELAPSED=$(( $(date +%s) - START ))
    ELAPSED_MIN=$((ELAPSED / 60))

    # Fetch metrics
    METRICS=$(curl -s --max-time 5 "$METRICS_URL" 2>/dev/null)
    if [ -z "$METRICS" ]; then
        echo "[$NOW] CHECK #$CHECK (${ELAPSED_MIN}m) — FAIL: metrics unreachable" | tee -a "$LOG"
        sleep "$INTERVAL"
        continue
    fi

    # Parse key metrics
    blocks=$(echo "$METRICS" | grep '^torsten_blocks_applied_total ' | awk '{print $2}')
    slot=$(echo "$METRICS" | grep '^torsten_slot_number ' | awk '{print $2}')
    tip_age=$(echo "$METRICS" | grep '^torsten_tip_age_seconds ' | awk '{print $2}')
    epoch=$(echo "$METRICS" | grep '^torsten_epoch_number ' | awk '{print $2}')
    peers=$(echo "$METRICS" | grep '^torsten_peers_connected ' | awk '{print $2}')
    hot=$(echo "$METRICS" | grep '^torsten_peers_hot ' | awk '{print $2}')
    forged=$(echo "$METRICS" | grep '^torsten_blocks_forged_total ' | awk '{print $2}')
    leader_checks=$(echo "$METRICS" | grep '^torsten_leader_checks_total ' | awk '{print $2}')
    rollbacks=$(echo "$METRICS" | grep '^torsten_rollback_count_total ' | awk '{print $2}')
    mempool_tx=$(echo "$METRICS" | grep '^torsten_mempool_tx_count ' | awk '{print $2}')
    utxo=$(echo "$METRICS" | grep '^torsten_utxo_count ' | awk '{print $2}')
    announced=$(echo "$METRICS" | grep '^torsten_blocks_announced_total ' | awk '{print $2}')
    cpu=$(echo "$METRICS" | grep '^torsten_cpu_percent ' | awk '{print $2}')
    mem_gb=$(echo "$METRICS" | grep '^torsten_mem_resident_bytes ' | awk '{printf "%.1f", $2/1073741824}')
    sync=$(echo "$METRICS" | grep '^torsten_sync_progress_percent ' | awk '{printf "%.1f", $2/100}')
    n2n_in=$(echo "$METRICS" | grep '^torsten_peers_inbound ' | awk '{print $2}')
    tx_recv=$(echo "$METRICS" | grep '^torsten_transactions_received_total ' | awk '{print $2}')
    tx_valid=$(echo "$METRICS" | grep '^torsten_transactions_validated_total ' | awk '{print $2}')

    # Compute deltas
    new_blocks=$((${blocks:-0} - ${prev_blocks:-0}))
    new_slots=$((${slot:-0} - ${prev_slot:-0}))
    prev_blocks=${blocks:-0}
    prev_slot=${slot:-0}

    # Health assessment
    STATUS="OK"
    if [ "${peers:-0}" -eq 0 ]; then STATUS="WARN:no_peers"; fi
    if [ "${tip_age%.*}" -gt 600 ] 2>/dev/null && [ "$CHECK" -gt 2 ]; then STATUS="WARN:tip_behind(${tip_age}s)"; fi
    if [ "${tip_age%.*}" -gt 3600 ] 2>/dev/null && [ "$CHECK" -gt 4 ]; then STATUS="CRITICAL:tip_stale(${tip_age}s)"; fi

    echo "[$NOW] CHECK #$CHECK (${ELAPSED_MIN}m) — $STATUS" | tee -a "$LOG"
    echo "  Chain:  slot=${slot} epoch=${epoch} sync=${sync}% tip_age=${tip_age}s" | tee -a "$LOG"
    echo "  Blocks: applied=${blocks} (+${new_blocks}) forged=${forged} announced=${announced} rollbacks=${rollbacks}" | tee -a "$LOG"
    echo "  Forge:  leader_checks=${leader_checks}" | tee -a "$LOG"
    echo "  Peers:  connected=${peers} hot=${hot} inbound=${n2n_in}" | tee -a "$LOG"
    echo "  Txs:    mempool=${mempool_tx} received=${tx_recv} validated=${tx_valid}" | tee -a "$LOG"
    echo "  System: cpu=${cpu}% mem=${mem_gb}GB utxo=${utxo}" | tee -a "$LOG"
    echo "" | tee -a "$LOG"

    sleep "$INTERVAL"
done

echo "=== Torsten Soak Test Complete ===" | tee -a "$LOG"
echo "End: $(date)" | tee -a "$LOG"
echo "Total checks: $CHECK" | tee -a "$LOG"
echo "Final blocks_applied: $blocks" | tee -a "$LOG"
echo "Final tip_age: $tip_age" | tee -a "$LOG"
