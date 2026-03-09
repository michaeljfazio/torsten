#!/usr/bin/env bash
# Benchmark different pipeline depths against the Cardano Preview testnet.
#
# Usage: ./scripts/benchmark-pipeline-depth.sh [duration_seconds]
#
# Each configuration syncs for the specified duration (default: 120s),
# then the blocks/sec throughput is extracted from the logs.

set -euo pipefail

DURATION=${1:-120}
BINARY="./target/release/torsten-node"
CONFIG="./config/preview-config.json"
TOPOLOGY="./config/preview-topology.json"
SOCKET="/tmp/torsten-bench.socket"
BASE_DB_DIR="/tmp/torsten-bench"

# Pipeline depths to test
DEPTHS=(25 50 100 150 200 300 500)

# Header batch sizes to test (paired with pipeline depth)
BATCH_SIZE=500

echo "=== Pipeline Depth Benchmark ==="
echo "Duration per run: ${DURATION}s"
echo "Testing depths: ${DEPTHS[*]}"
echo ""

results=()

for depth in "${DEPTHS[@]}"; do
    DB_DIR="${BASE_DB_DIR}/depth-${depth}"
    LOG_FILE="/tmp/torsten-bench-depth-${depth}.log"

    echo "--- Testing pipeline depth: ${depth} ---"

    # Clean DB for a fresh sync each time
    rm -rf "${DB_DIR}"
    mkdir -p "${DB_DIR}"
    rm -f "${SOCKET}"

    # First do a mithril import to skip to a known point
    echo "  Importing Mithril snapshot..."
    ${BINARY} mithril-import \
        --network-magic 2 \
        --database-path "${DB_DIR}" \
        --temp-dir /tmp/torsten-mithril 2>&1 | tail -1

    # Run the node with the specified pipeline depth
    echo "  Syncing with depth=${depth} for ${DURATION}s..."
    TORSTEN_PIPELINE_DEPTH=${depth} \
    TORSTEN_HEADER_BATCH_SIZE=${BATCH_SIZE} \
    timeout "${DURATION}" ${BINARY} run \
        --config "${CONFIG}" \
        --topology "${TOPOLOGY}" \
        --database-path "${DB_DIR}" \
        --socket-path "${SOCKET}" \
        2>&1 | tee "${LOG_FILE}" | grep "blocks/s" | tail -5

    # Extract the average blocks/sec from the last few log lines
    avg_bps=$(grep "blocks/s" "${LOG_FILE}" | tail -5 | \
        sed 's/.*| \([0-9.]*\) blocks\/s.*/\1/' | \
        awk '{sum += $1; n++} END {if (n > 0) printf "%.1f", sum/n; else print "0"}')

    # Get the final block number
    final_block=$(grep "block " "${LOG_FILE}" | tail -1 | \
        sed 's/.*block \([0-9]*\)\/.*/\1/' 2>/dev/null || echo "0")

    echo "  Result: depth=${depth} => ${avg_bps} blocks/sec (reached block ${final_block})"
    results+=("${depth}:${avg_bps}:${final_block}")
    echo ""
done

echo ""
echo "=== Summary ==="
echo "Depth | Blocks/sec | Final Block"
echo "------|------------|------------"
for r in "${results[@]}"; do
    IFS=':' read -r d bps fb <<< "$r"
    printf "%5s | %10s | %s\n" "$d" "$bps" "$fb"
done

# Find the best depth
best=""
best_bps=0
for r in "${results[@]}"; do
    IFS=':' read -r d bps fb <<< "$r"
    if (( $(echo "$bps > $best_bps" | bc -l) )); then
        best_bps=$bps
        best=$d
    fi
done
echo ""
echo "Optimal pipeline depth: ${best} (${best_bps} blocks/sec)"
