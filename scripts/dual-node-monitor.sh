#!/usr/bin/env bash
# Dual Node Monitor — tracks BP + Relay metrics every 60s for 2 hours
#
# Relay  metrics: http://localhost:12798/metrics
# BP     metrics: http://localhost:12799/metrics
#
# Tracked metrics per node:
#   sync progress, peers, blocks received/applied/forged/announced,
#   tx received, tx validated, tx rejected, mempool size, leader checks
set -euo pipefail

LOG="/tmp/dual-node-monitor.log"
RELAY_METRICS="http://localhost:12798/metrics"
BP_METRICS="http://localhost:12799/metrics"
INTERVAL=60
DURATION=7200

get_m() {
  curl -s "$1" 2>/dev/null | grep "^$2 " | awk '{print $2}' | cut -d. -f1
}

log() {
  echo "$(date -u '+%H:%M:%S') $*" | tee -a "$LOG"
}

> "$LOG"
log "=== Dual Node Monitor Started (${DURATION}s, interval=${INTERVAL}s) ==="
log "=== Relay: $RELAY_METRICS | BP: $BP_METRICS ==="

START=$(date +%s)
CYCLE=0

while true; do
  ELAPSED=$(( $(date +%s) - START ))
  [ "$ELAPSED" -ge "$DURATION" ] && break

  # Relay metrics
  RL_SYNC=$(get_m "$RELAY_METRICS" "dugite_sync_progress_percent")
  RL_PEERS=$(get_m "$RELAY_METRICS" "dugite_peers_connected")
  RL_INBOUND=$(get_m "$RELAY_METRICS" "dugite_peers_inbound")
  RL_BLOCKS_RCV=$(get_m "$RELAY_METRICS" "dugite_blocks_received_total")
  RL_BLOCKS_APP=$(get_m "$RELAY_METRICS" "dugite_blocks_applied_total")
  RL_BLOCKS_ANN=$(get_m "$RELAY_METRICS" "dugite_blocks_announced_total")
  RL_TX_RECV=$(get_m "$RELAY_METRICS" "dugite_transactions_received_total")
  RL_TX_VAL=$(get_m "$RELAY_METRICS" "dugite_transactions_validated_total")
  RL_TX_REJ=$(get_m "$RELAY_METRICS" "dugite_transactions_rejected_total")
  RL_MEMPOOL=$(get_m "$RELAY_METRICS" "dugite_mempool_tx_count")
  RL_AGE=$(get_m "$RELAY_METRICS" "dugite_tip_age_seconds")
  RL_SLOT=$(get_m "$RELAY_METRICS" "dugite_slot_number")

  # BP metrics
  BP_SYNC=$(get_m "$BP_METRICS" "dugite_sync_progress_percent")
  BP_PEERS=$(get_m "$BP_METRICS" "dugite_peers_connected")
  BP_BLOCKS_RCV=$(get_m "$BP_METRICS" "dugite_blocks_received_total")
  BP_BLOCKS_APP=$(get_m "$BP_METRICS" "dugite_blocks_applied_total")
  BP_BLOCKS_FORGED=$(get_m "$BP_METRICS" "dugite_blocks_forged_total")
  BP_BLOCKS_ANN=$(get_m "$BP_METRICS" "dugite_blocks_announced_total")
  BP_LEADER=$(get_m "$BP_METRICS" "dugite_leader_checks_total")
  BP_TX_RECV=$(get_m "$BP_METRICS" "dugite_transactions_received_total")
  BP_TX_VAL=$(get_m "$BP_METRICS" "dugite_transactions_validated_total")
  BP_TX_REJ=$(get_m "$BP_METRICS" "dugite_transactions_rejected_total")
  BP_MEMPOOL=$(get_m "$BP_METRICS" "dugite_mempool_tx_count")
  BP_AGE=$(get_m "$BP_METRICS" "dugite_tip_age_seconds")
  BP_SLOT=$(get_m "$BP_METRICS" "dugite_slot_number")

  log "[${ELAPSED}s] RELAY: sync=${RL_SYNC}‱ slot=${RL_SLOT} age=${RL_AGE}s peers=${RL_PEERS}(in=${RL_INBOUND}) blk_rcv=${RL_BLOCKS_RCV} blk_app=${RL_BLOCKS_APP} blk_ann=${RL_BLOCKS_ANN} tx_recv=${RL_TX_RECV:-0} tx_val=${RL_TX_VAL:-0} tx_rej=${RL_TX_REJ:-0} mempool=${RL_MEMPOOL:-0}"
  log "[${ELAPSED}s] BP   : sync=${BP_SYNC}‱ slot=${BP_SLOT} age=${BP_AGE}s peers=${BP_PEERS} blk_rcv=${BP_BLOCKS_RCV} blk_app=${BP_BLOCKS_APP} blk_forged=${BP_BLOCKS_FORGED} blk_ann=${BP_BLOCKS_ANN} leader=${BP_LEADER} tx_recv=${BP_TX_RECV:-0} tx_val=${BP_TX_VAL:-0} tx_rej=${BP_TX_REJ:-0} mempool=${BP_MEMPOOL:-0}"

  # Highlight key events
  [ "${BP_BLOCKS_FORGED:-0}" -gt 0 ] && log "*** BP FORGED A BLOCK! total=${BP_BLOCKS_FORGED} ***"
  [ "${RL_BLOCKS_ANN:-0}" -gt "${BP_BLOCKS_RCV:-0}" ] && log "  (relay is ahead of BP by $((${RL_BLOCKS_ANN:-0} - ${BP_BLOCKS_RCV:-0})) blocks)"
  [ "${RL_TX_RECV:-0}" -gt 0 ]     && log "  Relay received txs: total=${RL_TX_RECV}"
  [ "${BP_TX_RECV:-0}" -gt 0 ]     && log "  BP received txs: total=${BP_TX_RECV}"
  [ "${RL_TX_REJ:-0}" -gt 0 ]      && log "  WARN: Relay rejected ${RL_TX_REJ} txs"
  [ "${BP_TX_REJ:-0}" -gt 0 ]      && log "  WARN: BP rejected ${BP_TX_REJ} txs"

  CYCLE=$((CYCLE + 1))
  sleep "$INTERVAL"
done

log "=== Monitor Complete ==="
