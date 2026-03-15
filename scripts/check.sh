#!/usr/bin/env bash
# Run all pre-commit checks: format, clippy, test.
#
# Usage: ./scripts/check.sh

set -euo pipefail
cd "$(dirname "$0")/.."

echo "=== Format Check ==="
cargo fmt --all -- --check
echo "PASS"

echo ""
echo "=== Clippy ==="
cargo clippy --all-targets -- -D warnings
echo "PASS"

echo ""
echo "=== Tests ==="
cargo test --all
echo "PASS"

echo ""
echo "All checks passed!"
