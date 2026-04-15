#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# n2c-compat-test.sh — N2C compatibility regression suite (issue #409)
# ─────────────────────────────────────────────────────────────────────────────
#
# Runs the full `cardano-cli query` surface against a running dugite-node AND
# a running cardano-node (Haskell), captures JSON outputs, normalizes them,
# and diffs. Exits non-zero if any query diverges.
#
# PREREQUISITES
#   - cardano-cli on PATH (the upstream Haskell CLI)
#   - jq on PATH
#   - A running dugite-node exposing its N2C socket (DUGITE_SOCKET)
#   - A running cardano-node exposing its N2C socket (HASKELL_SOCKET)
#   - Both nodes synced to (roughly) the same tip on the same network
#
#   The harness never starts or stops nodes — it only reads from their
#   sockets. Nothing is written outside OUT_DIR.
#
# USAGE
#   scripts/n2c-compat-test.sh \
#       --dugite-socket  ./node.sock \
#       --haskell-socket ./cardano-node.sock \
#       --network-magic  2 \
#       --out-dir        logs/n2c-compat/$(date +%Y%m%dT%H%M%SZ)
#
#   scripts/n2c-compat-test.sh --only tip
#   scripts/n2c-compat-test.sh --skip ledger-state --skip utxo-whole
#   DUGITE_SOCKET=./node.sock HASKELL_SOCKET=./cn.sock scripts/n2c-compat-test.sh
#
# OUTPUT LAYOUT
#   <OUT_DIR>/
#     dugite/<query>.json        captured stdout from dugite
#     dugite/<query>.err         captured stderr
#     dugite/<query>.exit        captured exit code
#     haskell/<query>.json       captured stdout from cardano-node
#     haskell/<query>.err        captured stderr
#     haskell/<query>.exit       captured exit code
#     diffs/<query>.diff         unified diff of normalized JSON (if any)
#     report.md                  human-readable report
#     report.json                machine-readable report (one record per query)
#
# EXIT CODES
#   0 — every query matched (and both sides succeeded)
#   1 — at least one query diverged, errored, or timed out
#
# FILING FAILURES AS ISSUES
#   report.md lists each failing query with a link to its .diff file. The
#   report is designed so each row can become a standalone bug report:
#   copy the query name, both exit codes, and the diff hunk into a new
#   issue titled "N2C compat: <query> diverges".
#
# COMPATIBILITY
#   POSIX bash, works on macOS bash 3.2+ and Linux bash 5+.
#   No associative arrays — parallel indexed arrays instead.
# ─────────────────────────────────────────────────────────────────────────────

set -euo pipefail

# ── Defaults (overridable via env or flags) ──────────────────────────────────
DUGITE_SOCKET="${DUGITE_SOCKET:-./node.sock}"
HASKELL_SOCKET="${HASKELL_SOCKET:-./cardano-node.sock}"
NETWORK_MAGIC="${NETWORK_MAGIC:-2}"
OUT_DIR="${OUT_DIR:-}"
POOL_ID="${POOL_ID:-da71550ba75cbd51635ac8a30fb960aef9b6ffc4193fd3764da1b88e}"
STAKE_ADDR="${STAKE_ADDR:-}"
TX_IN="${TX_IN:-}"
ADDRESS="${ADDRESS:-}"
QUERY_TIMEOUT="${QUERY_TIMEOUT:-60}"

ONLY_QUERY=""
SKIP_QUERIES=""  # space-separated list

# ── Usage ────────────────────────────────────────────────────────────────────
usage() {
    cat <<'EOF'
n2c-compat-test.sh — N2C compatibility regression suite (#409)

Runs every `cardano-cli query` subcommand against a dugite-node N2C socket
AND a cardano-node N2C socket, normalizes outputs with jq, and diffs them.

USAGE
    scripts/n2c-compat-test.sh [flags]

FLAGS
    --dugite-socket  <path>   Path to dugite-node socket        [default: ./node.sock]
    --haskell-socket <path>   Path to cardano-node socket       [default: ./cardano-node.sock]
    --network-magic  <int>    Network magic (2=preview)         [default: 2]
    --out-dir        <path>   Output directory (timestamped if omitted)
    --pool-id        <hex>    Pool ID for pool-state/params/stake-snapshot
                              [default: SAND preview pool]
    --stake-addr     <bech>   Stake address for stake-address-info
    --tx-in          <txin>   tx-hash#ix for `utxo --tx-in`
    --address        <addr>   Address for `utxo --address`
    --only           <name>   Run only this query (repeatable is not supported;
                              pass comma-separated names if multiple)
    --skip           <name>   Skip this query (may be repeated)
    --timeout        <secs>   Per-query timeout                 [default: 60]
    -h, --help                Show this help and exit

ENVIRONMENT
    Any of the flags above may also be set as UPPER_SNAKE env vars:
    DUGITE_SOCKET, HASKELL_SOCKET, NETWORK_MAGIC, OUT_DIR, POOL_ID,
    STAKE_ADDR, TX_IN, ADDRESS, QUERY_TIMEOUT.

EXAMPLES
    # Full run with defaults
    scripts/n2c-compat-test.sh

    # Single query
    scripts/n2c-compat-test.sh --only tip

    # Skip the heavy ones
    scripts/n2c-compat-test.sh --skip ledger-state --skip utxo-whole

EXIT CODES
    0  all queries matched
    1  any query diverged, errored, or timed out
EOF
}

# ── Argument parsing ─────────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
    case "$1" in
        --dugite-socket)  DUGITE_SOCKET="$2"; shift 2 ;;
        --haskell-socket) HASKELL_SOCKET="$2"; shift 2 ;;
        --network-magic)  NETWORK_MAGIC="$2"; shift 2 ;;
        --out-dir)        OUT_DIR="$2"; shift 2 ;;
        --pool-id)        POOL_ID="$2"; shift 2 ;;
        --stake-addr)     STAKE_ADDR="$2"; shift 2 ;;
        --tx-in)          TX_IN="$2"; shift 2 ;;
        --address)        ADDRESS="$2"; shift 2 ;;
        --only)           ONLY_QUERY="$2"; shift 2 ;;
        --skip)           SKIP_QUERIES="$SKIP_QUERIES $2"; shift 2 ;;
        --timeout)        QUERY_TIMEOUT="$2"; shift 2 ;;
        -h|--help)        usage; exit 0 ;;
        *)
            echo "error: unknown argument: $1" >&2
            echo "run '$0 --help' for usage" >&2
            exit 2
            ;;
    esac
done

# ── Pre-flight checks ────────────────────────────────────────────────────────
die() {
    echo "error: $*" >&2
    exit 2
}

command -v cardano-cli >/dev/null 2>&1 || die "cardano-cli not found on PATH"
command -v jq          >/dev/null 2>&1 || die "jq not found on PATH"
command -v diff        >/dev/null 2>&1 || die "diff not found on PATH"

if [[ ! -S "$DUGITE_SOCKET" ]]; then
    die "dugite socket not found or not a socket: $DUGITE_SOCKET
    (is dugite-node running? pass --dugite-socket or set DUGITE_SOCKET)"
fi

if [[ ! -S "$HASKELL_SOCKET" ]]; then
    die "haskell cardano-node socket not found or not a socket: $HASKELL_SOCKET
    (is cardano-node running? pass --haskell-socket or set HASKELL_SOCKET)"
fi

# Derive TESTNET_ARG from magic (mainnet uses --mainnet)
if [[ "$NETWORK_MAGIC" == "764824073" ]]; then
    # shellcheck disable=SC2034
    TESTNET_ARG=(--mainnet)
else
    TESTNET_ARG=(--testnet-magic "$NETWORK_MAGIC")
fi

# Default OUT_DIR to a timestamped directory if not supplied
if [[ -z "$OUT_DIR" ]]; then
    OUT_DIR="logs/n2c-compat/$(date -u +%Y%m%dT%H%M%SZ)"
fi

mkdir -p "$OUT_DIR/dugite" "$OUT_DIR/haskell" "$OUT_DIR/diffs"

# ── Logging helpers ──────────────────────────────────────────────────────────
LOG_FILE="$OUT_DIR/run.log"

log() {
    local ts
    ts=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
    echo "[$ts] $*" | tee -a "$LOG_FILE"
}

log_section() {
    echo ""                                                        | tee -a "$LOG_FILE"
    echo "════════════════════════════════════════════════════════" | tee -a "$LOG_FILE"
    echo "  $*"                                                     | tee -a "$LOG_FILE"
    echo "════════════════════════════════════════════════════════" | tee -a "$LOG_FILE"
}

# ── Portable per-command timeout ─────────────────────────────────────────────
# macOS bash 3.2 does not ship `timeout`. Fall back to a subshell + sleep kill.
run_with_timeout() {
    local secs="$1"; shift
    if command -v timeout >/dev/null 2>&1; then
        timeout "$secs" "$@"
        return $?
    fi
    if command -v gtimeout >/dev/null 2>&1; then
        gtimeout "$secs" "$@"
        return $?
    fi
    # Manual fallback (no coreutils timeout available)
    "$@" &
    local pid=$!
    ( sleep "$secs" && kill -TERM "$pid" 2>/dev/null ) &
    local watcher=$!
    local rc=0
    wait "$pid" 2>/dev/null || rc=$?
    kill "$watcher" 2>/dev/null || true
    wait "$watcher" 2>/dev/null || true
    return "$rc"
}

# ── Query dispatch ───────────────────────────────────────────────────────────
# Parallel indexed arrays (bash 3.2 compatible — no associative arrays).
#   QUERY_NAMES[i]   stable short name (used for filenames and CLI flags)
#   QUERY_LABELS[i]  human-friendly label for the report
#
# The actual cardano-cli invocation for each name is in build_query_cmd().
QUERY_NAMES=(
    "tip"
    "protocol-parameters"
    "stake-snapshot-all"
    "stake-snapshot-pool"
    "stake-pools"
    "stake-distribution"
    "stake-address-info"
    "pool-state"
    "pool-params"
    "ledger-state"
    "protocol-state"
    "ref-script-size"
    "utxo-whole"
    "utxo-address"
    "utxo-tx-in"
    "kes-period-info"
    "leadership-schedule-current"
    "leadership-schedule-next"
    "slot-number"
    "era-history"
    "gov-committee-state"
    "gov-drep-state"
    "gov-drep-stake-distribution"
    "gov-gov-state"
    "gov-proposals"
    "constitution"
    "treasury"
)

QUERY_LABELS=(
    "query tip"
    "query protocol-parameters"
    "query stake-snapshot (all pools)"
    "query stake-snapshot --stake-pool-id"
    "query stake-pools"
    "query stake-distribution"
    "query stake-address-info"
    "query pool-state"
    "query pool-params"
    "query ledger-state"
    "query protocol-state"
    "query ref-script-size"
    "query utxo --whole-utxo"
    "query utxo --address"
    "query utxo --tx-in"
    "query kes-period-info"
    "query leadership-schedule --current"
    "query leadership-schedule --next"
    "query slot-number"
    "query era-history"
    "query governance committee-state"
    "query governance drep-state"
    "query governance drep-stake-distribution"
    "query governance gov-state"
    "query governance proposals"
    "query constitution"
    "query treasury"
)

# Return the cardano-cli argv for query $1 on stdout as one-argument-per-line.
# The caller reads it with `mapfile`/`while read`. Unsupported queries (missing
# parameter) print "SKIP: <reason>" on stdout and return 1.
build_query_cmd() {
    local name="$1"
    case "$name" in
        tip)
            printf '%s\n' query tip "${TESTNET_ARG[@]}" --output-json
            ;;
        protocol-parameters)
            printf '%s\n' query protocol-parameters "${TESTNET_ARG[@]}" --output-json
            ;;
        stake-snapshot-all)
            printf '%s\n' query stake-snapshot "${TESTNET_ARG[@]}" --all-stake-pools --output-json
            ;;
        stake-snapshot-pool)
            if [[ -z "$POOL_ID" ]]; then
                echo "SKIP: --pool-id not set"
                return 1
            fi
            printf '%s\n' query stake-snapshot "${TESTNET_ARG[@]}" --stake-pool-id "$POOL_ID" --output-json
            ;;
        stake-pools)
            printf '%s\n' query stake-pools "${TESTNET_ARG[@]}" --output-json
            ;;
        stake-distribution)
            printf '%s\n' query stake-distribution "${TESTNET_ARG[@]}" --output-json
            ;;
        stake-address-info)
            if [[ -z "$STAKE_ADDR" ]]; then
                echo "SKIP: --stake-addr not set"
                return 1
            fi
            printf '%s\n' query stake-address-info "${TESTNET_ARG[@]}" --address "$STAKE_ADDR" --output-json
            ;;
        pool-state)
            if [[ -z "$POOL_ID" ]]; then
                echo "SKIP: --pool-id not set"
                return 1
            fi
            printf '%s\n' query pool-state "${TESTNET_ARG[@]}" --stake-pool-id "$POOL_ID" --output-json
            ;;
        pool-params)
            if [[ -z "$POOL_ID" ]]; then
                echo "SKIP: --pool-id not set"
                return 1
            fi
            printf '%s\n' query pool-params "${TESTNET_ARG[@]}" --stake-pool-id "$POOL_ID" --output-json
            ;;
        ledger-state)
            printf '%s\n' query ledger-state "${TESTNET_ARG[@]}" --output-json
            ;;
        protocol-state)
            printf '%s\n' query protocol-state "${TESTNET_ARG[@]}" --output-json
            ;;
        ref-script-size)
            if [[ -z "$TX_IN" ]]; then
                echo "SKIP: --tx-in not set"
                return 1
            fi
            printf '%s\n' query ref-script-size "${TESTNET_ARG[@]}" --tx-in "$TX_IN" --output-json
            ;;
        utxo-whole)
            printf '%s\n' query utxo "${TESTNET_ARG[@]}" --whole-utxo --output-json
            ;;
        utxo-address)
            if [[ -z "$ADDRESS" ]]; then
                echo "SKIP: --address not set"
                return 1
            fi
            printf '%s\n' query utxo "${TESTNET_ARG[@]}" --address "$ADDRESS" --output-json
            ;;
        utxo-tx-in)
            if [[ -z "$TX_IN" ]]; then
                echo "SKIP: --tx-in not set"
                return 1
            fi
            printf '%s\n' query utxo "${TESTNET_ARG[@]}" --tx-in "$TX_IN" --output-json
            ;;
        kes-period-info)
            # KES period info requires an opcert file — skip unless user supplied
            # one via the OPCERT env var. This keeps the harness read-only by default.
            if [[ -z "${OPCERT:-}" ]]; then
                echo "SKIP: OPCERT env var not set (path to opcert file)"
                return 1
            fi
            printf '%s\n' query kes-period-info "${TESTNET_ARG[@]}" --op-cert-file "$OPCERT" --output-json
            ;;
        leadership-schedule-current)
            if [[ -z "${VRF_SKEY:-}" || -z "${POOL_VRF_VKEY:-}" || -z "${GENESIS_FILE:-}" ]]; then
                echo "SKIP: VRF_SKEY/POOL_VRF_VKEY/GENESIS_FILE env vars not set"
                return 1
            fi
            printf '%s\n' query leadership-schedule "${TESTNET_ARG[@]}" \
                --genesis "$GENESIS_FILE" \
                --stake-pool-id "$POOL_ID" \
                --vrf-signing-key-file "$VRF_SKEY" \
                --current --output-json
            ;;
        leadership-schedule-next)
            if [[ -z "${VRF_SKEY:-}" || -z "${POOL_VRF_VKEY:-}" || -z "${GENESIS_FILE:-}" ]]; then
                echo "SKIP: VRF_SKEY/POOL_VRF_VKEY/GENESIS_FILE env vars not set"
                return 1
            fi
            printf '%s\n' query leadership-schedule "${TESTNET_ARG[@]}" \
                --genesis "$GENESIS_FILE" \
                --stake-pool-id "$POOL_ID" \
                --vrf-signing-key-file "$VRF_SKEY" \
                --next --output-json
            ;;
        slot-number)
            # Use a deterministic UTC so both sides see the same input.
            local utc
            utc="${SLOT_NUMBER_UTC:-$(date -u +%Y-%m-%dT%H:%M:%SZ)}"
            printf '%s\n' query slot-number "${TESTNET_ARG[@]}" "$utc"
            ;;
        era-history)
            printf '%s\n' query era-history "${TESTNET_ARG[@]}"
            ;;
        gov-committee-state)
            printf '%s\n' query governance committee-state "${TESTNET_ARG[@]}" --output-json
            ;;
        gov-drep-state)
            printf '%s\n' query governance drep-state "${TESTNET_ARG[@]}" --all-dreps --output-json
            ;;
        gov-drep-stake-distribution)
            printf '%s\n' query governance drep-stake-distribution "${TESTNET_ARG[@]}" --all-dreps --output-json
            ;;
        gov-gov-state)
            printf '%s\n' query governance gov-state "${TESTNET_ARG[@]}" --output-json
            ;;
        gov-proposals)
            printf '%s\n' query governance proposals "${TESTNET_ARG[@]}" --all-proposals --output-json
            ;;
        constitution)
            printf '%s\n' query constitution "${TESTNET_ARG[@]}" --output-json
            ;;
        treasury)
            printf '%s\n' query treasury "${TESTNET_ARG[@]}" --output-json
            ;;
        *)
            echo "SKIP: unknown query name: $name"
            return 1
            ;;
    esac
    return 0
}

# ── Query execution ──────────────────────────────────────────────────────────
# Runs one query against one socket. Writes <out_base>.json/.err/.exit.
# Args: socket name out_base
run_one() {
    local socket="$1"
    local name="$2"
    local out_base="$3"

    local json_out="${out_base}.json"
    local err_out="${out_base}.err"
    local exit_out="${out_base}.exit"

    local cmd_lines
    if ! cmd_lines=$(build_query_cmd "$name" 2>&1); then
        # build_query_cmd signalled SKIP
        echo "$cmd_lines" > "$err_out"
        echo "skipped" > "$exit_out"
        : > "$json_out"
        return 0
    fi

    # Build argv from the printed lines.
    local -a argv
    argv=()
    while IFS= read -r line; do
        argv+=("$line")
    done <<EOF_ARGS
$cmd_lines
EOF_ARGS

    local rc=0
    CARDANO_NODE_SOCKET_PATH="$socket" \
        run_with_timeout "$QUERY_TIMEOUT" cardano-cli "${argv[@]}" \
        > "$json_out" 2> "$err_out" || rc=$?
    echo "$rc" > "$exit_out"
}

# ── JSON normalization & diff ────────────────────────────────────────────────
# jq -S sorts object keys at every depth. If the file isn't JSON (e.g. slot-number
# prints a bare integer), fall back to a textual compare.
normalize_and_diff() {
    local name="$1"
    local dugite_json="$OUT_DIR/dugite/${name}.json"
    local haskell_json="$OUT_DIR/haskell/${name}.json"
    local diff_out="$OUT_DIR/diffs/${name}.diff"

    local dugite_norm="$OUT_DIR/diffs/${name}.dugite.norm"
    local haskell_norm="$OUT_DIR/diffs/${name}.haskell.norm"

    if jq -S . "$dugite_json" > "$dugite_norm" 2>/dev/null \
       && jq -S . "$haskell_json" > "$haskell_norm" 2>/dev/null; then
        : # both are valid JSON, compare normalized
    else
        # Fall back to trimmed textual compare
        sed -e 's/[[:space:]]*$//' "$dugite_json"  > "$dugite_norm"
        sed -e 's/[[:space:]]*$//' "$haskell_json" > "$haskell_norm"
    fi

    if diff -u "$haskell_norm" "$dugite_norm" > "$diff_out" 2>&1; then
        rm -f "$diff_out"
        return 0
    else
        return 1
    fi
}

# ── Per-query driver ─────────────────────────────────────────────────────────
# Populates RESULT_STATUS[i], RESULT_DUGITE_EXIT[i], RESULT_HASKELL_EXIT[i],
# RESULT_DIFF[i] in parallel indexed arrays (3.2-compatible).
RESULT_STATUS=()
RESULT_DUGITE_EXIT=()
RESULT_HASKELL_EXIT=()
RESULT_DIFF=()

should_skip() {
    local name="$1"
    if [[ -n "$ONLY_QUERY" && "$ONLY_QUERY" != "$name" ]]; then
        return 0
    fi
    local s
    for s in $SKIP_QUERIES; do
        [[ "$s" == "$name" ]] && return 0
    done
    return 1
}

drive_query() {
    local idx="$1"
    local name="${QUERY_NAMES[$idx]}"
    local label="${QUERY_LABELS[$idx]}"

    if should_skip "$name"; then
        log "SKIP  [$name] ($label)"
        RESULT_STATUS[idx]="skipped"
        RESULT_DUGITE_EXIT[idx]="-"
        RESULT_HASKELL_EXIT[idx]="-"
        RESULT_DIFF[idx]=""
        return 0
    fi

    log "RUN   [$name] ($label)"

    run_one "$DUGITE_SOCKET"  "$name" "$OUT_DIR/dugite/${name}"
    run_one "$HASKELL_SOCKET" "$name" "$OUT_DIR/haskell/${name}"

    local dugite_exit haskell_exit
    dugite_exit=$(cat "$OUT_DIR/dugite/${name}.exit")
    haskell_exit=$(cat "$OUT_DIR/haskell/${name}.exit")

    RESULT_DUGITE_EXIT[idx]="$dugite_exit"
    RESULT_HASKELL_EXIT[idx]="$haskell_exit"
    RESULT_DIFF[idx]=""

    if [[ "$dugite_exit" == "skipped" || "$haskell_exit" == "skipped" ]]; then
        log "      SKIPPED (build_query_cmd returned SKIP)"
        RESULT_STATUS[idx]="skipped"
        return 0
    fi

    if [[ "$dugite_exit" != "0" || "$haskell_exit" != "0" ]]; then
        log "      ERROR (dugite=$dugite_exit haskell=$haskell_exit)"
        RESULT_STATUS[idx]="error"
        return 1
    fi

    if normalize_and_diff "$name"; then
        log "      PASS"
        RESULT_STATUS[idx]="pass"
        return 0
    else
        log "      DIFF (see diffs/${name}.diff)"
        RESULT_STATUS[idx]="diff"
        RESULT_DIFF[idx]="diffs/${name}.diff"
        return 1
    fi
}

# ── Report writers ───────────────────────────────────────────────────────────
write_reports() {
    local report_md="$OUT_DIR/report.md"
    local report_json="$OUT_DIR/report.json"
    local total=${#QUERY_NAMES[@]}
    local pass=0 fail=0 skipped=0 errored=0
    local i status

    for i in "${!QUERY_NAMES[@]}"; do
        status="${RESULT_STATUS[$i]:-unknown}"
        case "$status" in
            pass)    pass=$((pass + 1)) ;;
            diff)    fail=$((fail + 1)) ;;
            error)   errored=$((errored + 1)) ;;
            skipped) skipped=$((skipped + 1)) ;;
        esac
    done

    {
        echo "# N2C compatibility regression report"
        echo ""
        echo "- Run date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
        echo "- Dugite socket: \`$DUGITE_SOCKET\`"
        echo "- Haskell socket: \`$HASKELL_SOCKET\`"
        echo "- Network magic: $NETWORK_MAGIC"
        echo "- Out dir: \`$OUT_DIR\`"
        echo ""
        echo "## Summary"
        echo ""
        echo "| total | pass | diff | error | skipped |"
        echo "|-------|------|------|-------|---------|"
        echo "| $total | $pass | $fail | $errored | $skipped |"
        echo ""
        echo "## Per-query results"
        echo ""
        echo "| query | dugite exit | haskell exit | status | diff |"
        echo "|-------|-------------|--------------|--------|------|"
        for i in "${!QUERY_NAMES[@]}"; do
            local name="${QUERY_NAMES[$i]}"
            local de="${RESULT_DUGITE_EXIT[$i]:--}"
            local he="${RESULT_HASKELL_EXIT[$i]:--}"
            local st="${RESULT_STATUS[$i]:-unknown}"
            local diff_link="${RESULT_DIFF[$i]:-}"
            local diff_cell="—"
            if [[ -n "$diff_link" ]]; then
                diff_cell="[\`$diff_link\`]($diff_link)"
            fi
            echo "| \`$name\` | $de | $he | $st | $diff_cell |"
        done
        echo ""
        if (( fail > 0 || errored > 0 )); then
            echo "## Failing queries (file one issue per row)"
            echo ""
            for i in "${!QUERY_NAMES[@]}"; do
                local st="${RESULT_STATUS[$i]:-unknown}"
                if [[ "$st" == "diff" || "$st" == "error" ]]; then
                    local name="${QUERY_NAMES[$i]}"
                    local label="${QUERY_LABELS[$i]}"
                    echo "- **\`$name\`** ($label) — dugite=${RESULT_DUGITE_EXIT[$i]} haskell=${RESULT_HASKELL_EXIT[$i]} — \`dugite/${name}.json\` vs \`haskell/${name}.json\`"
                fi
            done
            echo ""
            echo "Each row above can become a standalone sub-issue titled"
            echo "\"N2C compat: <query> diverges\" with the captured JSON and diff attached."
        fi
    } > "$report_md"

    # report.json — hand-rolled to avoid needing a second jq pass
    {
        printf '{\n'
        printf '  "run_utc": "%s",\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
        printf '  "dugite_socket": "%s",\n' "$DUGITE_SOCKET"
        printf '  "haskell_socket": "%s",\n' "$HASKELL_SOCKET"
        printf '  "network_magic": %s,\n' "$NETWORK_MAGIC"
        printf '  "out_dir": "%s",\n' "$OUT_DIR"
        printf '  "summary": { "total": %d, "pass": %d, "diff": %d, "error": %d, "skipped": %d },\n' \
            "$total" "$pass" "$fail" "$errored" "$skipped"
        printf '  "queries": [\n'
        local first=1
        for i in "${!QUERY_NAMES[@]}"; do
            local name="${QUERY_NAMES[$i]}"
            local label="${QUERY_LABELS[$i]}"
            local de="${RESULT_DUGITE_EXIT[$i]:--}"
            local he="${RESULT_HASKELL_EXIT[$i]:--}"
            local st="${RESULT_STATUS[$i]:-unknown}"
            local df="${RESULT_DIFF[$i]:-}"
            if (( first == 1 )); then
                first=0
            else
                printf ',\n'
            fi
            printf '    { "name": "%s", "label": "%s", "dugite_exit": "%s", "haskell_exit": "%s", "status": "%s", "diff_file": "%s" }' \
                "$name" "$label" "$de" "$he" "$st" "$df"
        done
        printf '\n  ]\n'
        printf '}\n'
    } > "$report_json"

    # Re-normalize the JSON report with jq so humans can re-read it
    if jq . "$report_json" > "$report_json.tmp" 2>/dev/null; then
        mv "$report_json.tmp" "$report_json"
    else
        rm -f "$report_json.tmp"
    fi

    log_section "REPORT"
    log "Wrote $report_md"
    log "Wrote $report_json"
    log "Summary: total=$total pass=$pass diff=$fail error=$errored skipped=$skipped"

    if (( fail > 0 || errored > 0 )); then
        return 1
    fi
    return 0
}

# ── Main ────────────────────────────────────────────────────────────────────
main() {
    log_section "N2C COMPAT REGRESSION SUITE (#409)"
    log "dugite socket:  $DUGITE_SOCKET"
    log "haskell socket: $HASKELL_SOCKET"
    log "network magic:  $NETWORK_MAGIC"
    log "out dir:        $OUT_DIR"
    log "pool id:        $POOL_ID"
    [[ -n "$STAKE_ADDR" ]] && log "stake addr:     $STAKE_ADDR"
    [[ -n "$TX_IN"      ]] && log "tx in:          $TX_IN"
    [[ -n "$ADDRESS"    ]] && log "address:        $ADDRESS"
    [[ -n "$ONLY_QUERY" ]] && log "only:           $ONLY_QUERY"
    [[ -n "$SKIP_QUERIES" ]] && log "skip:          $SKIP_QUERIES"
    log "timeout/query:  ${QUERY_TIMEOUT}s"

    # Iterate queries
    local i
    for i in "${!QUERY_NAMES[@]}"; do
        drive_query "$i" || true
    done

    if write_reports; then
        log "RESULT: all queries passed or skipped"
        exit 0
    else
        log "RESULT: divergences or errors detected — see $OUT_DIR/report.md"
        exit 1
    fi
}

main "$@"
