#!/usr/bin/env bash
set -euo pipefail

# End-to-end integration test orchestration script
#
# Usage: tests/e2e/run.sh [--offline|--smoke|--full] [--keep-node]
#
#   --offline    Tier 0 only (no node, CI-safe)
#   --smoke      Tier 0 + query tip (default)
#   --full       All tiers including transactions
#   --keep-node  Don't stop node after tests

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

MODE="smoke"
KEEP_NODE=false

for arg in "$@"; do
    case "$arg" in
        --offline) MODE="offline" ;;
        --smoke)   MODE="smoke" ;;
        --full)    MODE="full" ;;
        --keep-node) KEEP_NODE=true ;;
        *)
            echo "Unknown argument: $arg"
            echo "Usage: $0 [--offline|--smoke|--full] [--keep-node]"
            exit 1
            ;;
    esac
done

echo "=== Torsten E2E Tests (mode: $MODE) ==="

# Step 1: Build release binaries
echo "--- Building release binaries..."
cd "$PROJECT_ROOT"
cargo build --release -p torsten-cli -p torsten-node 2>&1

export TORSTEN_CLI_PATH="$PROJECT_ROOT/target/release/torsten-cli"
export TORSTEN_NODE_PATH="$PROJECT_ROOT/target/release/torsten-node"

echo "CLI:  $TORSTEN_CLI_PATH"
echo "Node: $TORSTEN_NODE_PATH"

# Step 2: Run Tier 0 (always)
echo ""
echo "--- Running Tier 0: Offline tests..."
cargo test -p torsten-integration-tests -- tier0 --test-threads=4 2>&1
TIER0_EXIT=$?

if [ "$MODE" = "offline" ]; then
    echo ""
    if [ $TIER0_EXIT -eq 0 ]; then
        echo "=== ALL OFFLINE TESTS PASSED ==="
    else
        echo "=== OFFLINE TESTS FAILED ==="
    fi
    exit $TIER0_EXIT
fi

# Step 3: Check/configure node socket
SOCKET="${TORSTEN_INTEGRATION_SOCKET:-$PROJECT_ROOT/node.sock}"
export TORSTEN_INTEGRATION_SOCKET="$SOCKET"

NODE_PID=""
if ! "$TORSTEN_CLI_PATH" query tip --socket-path "$SOCKET" &>/dev/null; then
    echo ""
    echo "--- Node not running. Checking for config..."

    CONFIG="${TORSTEN_CONFIG:-$PROJECT_ROOT/config/preview-config.json}"
    TOPOLOGY="${TORSTEN_TOPOLOGY:-$PROJECT_ROOT/config/preview-topology.json}"
    DB_PATH="${TORSTEN_DB_PATH:-$PROJECT_ROOT/db-preview}"

    if [ ! -f "$CONFIG" ]; then
        echo "ERROR: No node running and config file not found: $CONFIG"
        echo "Set TORSTEN_INTEGRATION_SOCKET to a running node, or provide config files."
        exit 1
    fi

    echo "--- Starting node..."
    "$TORSTEN_NODE_PATH" run \
        --config "$CONFIG" \
        --topology "$TOPOLOGY" \
        --database-path "$DB_PATH" \
        --socket-path "$SOCKET" \
        --host-addr 0.0.0.0 --port 3001 &
    NODE_PID=$!
    echo "Node PID: $NODE_PID"

    # Wait for readiness
    echo "--- Waiting for node to be ready..."
    for i in $(seq 1 60); do
        if "$TORSTEN_CLI_PATH" query tip --socket-path "$SOCKET" &>/dev/null; then
            echo "Node ready after ${i}s"
            break
        fi
        if [ $i -eq 60 ]; then
            echo "ERROR: Node did not become ready within 60s"
            kill "$NODE_PID" 2>/dev/null || true
            exit 1
        fi
        sleep 1
    done
fi

cleanup() {
    if [ -n "$NODE_PID" ] && [ "$KEEP_NODE" = false ]; then
        echo "--- Stopping node (PID $NODE_PID)..."
        kill "$NODE_PID" 2>/dev/null || true
        wait "$NODE_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

# Step 4: Run Tier 1
echo ""
echo "--- Running Tier 1: Query tests..."
cargo test -p torsten-integration-tests -- tier1 --test-threads=2 2>&1
TIER1_EXIT=$?

if [ "$MODE" = "smoke" ]; then
    echo ""
    TOTAL_EXIT=$(( TIER0_EXIT + TIER1_EXIT ))
    if [ $TOTAL_EXIT -eq 0 ]; then
        echo "=== SMOKE TESTS PASSED ==="
    else
        echo "=== SMOKE TESTS FAILED ==="
    fi
    exit $TOTAL_EXIT
fi

# Step 5: Run Tier 2 (full mode only)
if [ "$MODE" = "full" ]; then
    if [ -n "${TORSTEN_TEST_KEYS:-}" ]; then
        export TORSTEN_TEST_KEYS
        echo ""
        echo "--- Running Tier 2: Transaction tests..."
        cargo test -p torsten-integration-tests -- tier2 --test-threads=1 2>&1
        TIER2_EXIT=$?
    else
        echo ""
        echo "--- SKIP Tier 2: TORSTEN_TEST_KEYS not set"
        TIER2_EXIT=0
    fi

    echo ""
    TOTAL_EXIT=$(( TIER0_EXIT + TIER1_EXIT + TIER2_EXIT ))
    if [ $TOTAL_EXIT -eq 0 ]; then
        echo "=== ALL E2E TESTS PASSED ==="
    else
        echo "=== E2E TESTS FAILED ==="
    fi
    exit $TOTAL_EXIT
fi
