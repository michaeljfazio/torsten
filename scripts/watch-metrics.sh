#!/usr/bin/env bash
# Watch Prometheus metrics in real-time (updates every 5 seconds).
#
# Usage: ./scripts/watch-metrics.sh [PORT]

PORT="${1:-12798}"
URL="http://localhost:$PORT/metrics"

echo "Watching metrics at $URL (Ctrl+C to stop)"
echo ""

while true; do
    clear
    echo "=== Dugite Metrics ($(date)) ==="
    echo ""
    curl -s "$URL" 2>/dev/null | grep "^dugite_" | sort || echo "Node not reachable at $URL"
    sleep 5
done
