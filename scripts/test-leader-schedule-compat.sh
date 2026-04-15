#!/usr/bin/env bash
#
# test-leader-schedule-compat.sh — N2C compatibility harness for the
# `query leadership-schedule --current` command (issue #408).
#
# What this does
# --------------
# Runs `cardano-cli query leadership-schedule --current` against a node
# listening on the given --socket-path, then diffs the result (sorted by
# slotNumber) against the frozen golden vector at:
#
#   tests/golden/leadership-schedule/preview-epoch-1268/haskell-current.json
#
# That golden was captured on 2026-04-15 from a synced cardano-node 10.6.2
# running the SAND pool VRF key. Any node implementing the N2C server
# correctly must produce byte-identical output for the same inputs.
#
# How to run
# ----------
# Against cardano-node (self-consistency check — the harness should PASS
# because the golden was captured against cardano-node in the first place):
#
#     ./scripts/test-leader-schedule-compat.sh \
#         --socket-path ./haskell-node.sock
#
# Against dugite-node (the actual compat test — requires #403 to be fixed
# so dugite-node can reach a stable listening socket):
#
#     ./scripts/test-leader-schedule-compat.sh \
#         --socket-path ./node.sock
#
# TODO(#408): once #403 lands and dugite-node's N2C listener is stable,
# wire this script into CI against a dugite-node socket. Today we can
# only validate the harness itself by running it against cardano-node.
#
# Scope
# -----
# Deliberately narrow: one query, one pool, one epoch's worth of golden
# data. A more general compat framework is tracked in #409 and will be
# built on top of this script.

set -euo pipefail

cd "$(dirname "$0")/.."

# ---- defaults ---------------------------------------------------------------

SOCKET_PATH=""
GENESIS="config/shelley-genesis.json"
TESTNET_MAGIC=2
VRF_SKEY="${HOME}/Downloads/forTorst/vrf.skey"
GOLDEN="tests/golden/leadership-schedule/preview-epoch-1268/haskell-current.json"

# Frozen fixture inputs — see golden README for provenance. These are not
# configurable on purpose: changing either invalidates the golden vector.
POOL_ID="da71550ba75cbd51635ac8a30fb960aef9b6ffc4193fd3764da1b88e"

# ---- arg parsing ------------------------------------------------------------

usage() {
    cat <<EOF
Usage: $0 --socket-path PATH [options]

Required:
  --socket-path PATH      Unix socket exposing the node's N2C server.

Options:
  --genesis PATH          Shelley genesis JSON (default: $GENESIS).
  --testnet-magic N       Network magic (default: $TESTNET_MAGIC, preview).
  --vrf-skey PATH         VRF signing key for the pool under test.
                          Default: $VRF_SKEY
  --golden PATH           Golden vector to diff against.
                          Default: $GOLDEN
  -h, --help              Show this help.
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --socket-path)    SOCKET_PATH="$2"; shift 2 ;;
        --genesis)        GENESIS="$2"; shift 2 ;;
        --testnet-magic)  TESTNET_MAGIC="$2"; shift 2 ;;
        --vrf-skey)       VRF_SKEY="$2"; shift 2 ;;
        --golden)         GOLDEN="$2"; shift 2 ;;
        -h|--help)        usage; exit 0 ;;
        *)
            echo "error: unknown argument: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

if [[ -z "$SOCKET_PATH" ]]; then
    echo "error: --socket-path is required" >&2
    usage >&2
    exit 2
fi

# ---- preflight --------------------------------------------------------------

fail_precheck() {
    echo "error: $*" >&2
    exit 2
}

command -v cardano-cli >/dev/null 2>&1 \
    || fail_precheck "cardano-cli not found on PATH"
command -v jq >/dev/null 2>&1 \
    || fail_precheck "jq not found on PATH"
command -v diff >/dev/null 2>&1 \
    || fail_precheck "diff not found on PATH"

[[ -S "$SOCKET_PATH" ]] \
    || fail_precheck "socket does not exist or is not a socket: $SOCKET_PATH"
[[ -r "$GENESIS" ]] \
    || fail_precheck "genesis not readable: $GENESIS"
[[ -r "$VRF_SKEY" ]] \
    || fail_precheck "VRF signing key not readable: $VRF_SKEY"
[[ -r "$GOLDEN" ]] \
    || fail_precheck "golden vector not readable: $GOLDEN"

# ---- run the query ----------------------------------------------------------

TMP_DIR="$(mktemp -d -t leader-sched-compat.XXXXXX)"
trap 'rm -rf "$TMP_DIR"' EXIT

ACTUAL_RAW="$TMP_DIR/actual.json"
ACTUAL_NORM="$TMP_DIR/actual.normalized.json"
GOLDEN_NORM="$TMP_DIR/golden.normalized.json"

echo "==> querying leadership-schedule --current"
echo "    socket:  $SOCKET_PATH"
echo "    magic:   $TESTNET_MAGIC"
echo "    genesis: $GENESIS"
echo "    pool:    $POOL_ID"
echo "    golden:  $GOLDEN"

# cardano-cli prints to stdout on success and a mix of stdout+stderr on
# failure. We capture both so mux disconnects / decode errors propagate
# with context rather than disappearing into /dev/null.
if ! CARDANO_NODE_SOCKET_PATH="$SOCKET_PATH" cardano-cli conway query leadership-schedule \
        --testnet-magic "$TESTNET_MAGIC" \
        --genesis "$GENESIS" \
        --stake-pool-id "$POOL_ID" \
        --vrf-signing-key-file "$VRF_SKEY" \
        --current \
        --out-file "$ACTUAL_RAW" \
        >"$TMP_DIR/cli.stdout" 2>"$TMP_DIR/cli.stderr"; then
    echo "FAIL: cardano-cli returned nonzero" >&2
    echo "---- stdout ----" >&2
    cat "$TMP_DIR/cli.stdout" >&2 || true
    echo "---- stderr ----" >&2
    cat "$TMP_DIR/cli.stderr" >&2 || true
    exit 1
fi

if [[ ! -s "$ACTUAL_RAW" ]]; then
    echo "FAIL: cardano-cli produced empty output" >&2
    exit 1
fi

# ---- normalize & diff -------------------------------------------------------

# Sort by slotNumber so the harness is insensitive to ordering. Both files
# should already be sorted ascending, but normalizing defensively here means
# an accidental reorder in either source doesn't mask a real divergence.
jq -S 'sort_by(.slotNumber)' "$ACTUAL_RAW" >"$ACTUAL_NORM"
jq -S 'sort_by(.slotNumber)' "$GOLDEN"     >"$GOLDEN_NORM"

if diff -u "$GOLDEN_NORM" "$ACTUAL_NORM" >"$TMP_DIR/diff"; then
    echo "PASS: leader schedule matches golden ($(jq 'length' "$ACTUAL_NORM") slots)"
    exit 0
fi

echo "FAIL: leader schedule diverges from golden" >&2
echo "---- diff (golden → actual) ----" >&2
cat "$TMP_DIR/diff" >&2
exit 1
