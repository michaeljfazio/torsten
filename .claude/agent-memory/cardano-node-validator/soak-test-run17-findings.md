---
name: Soak Test Run #17 Findings (2026-03-27)
description: Transaction submission soak test findings — panics, crash, stall bugs discovered and fixed
type: project
---

## Soak Test Run #17 — 2026-03-27

### Test Configuration
- Network: Preview testnet (magic=2), Conway era
- Address: `addr_test1qzultc5n46dl02n6v5f4zwhgf623m99ngda8cegynr7aaqtvnhgsytsr9zrwaz79h74dhlm4s3hy8dd9v0mzcye6l90setqluv`
- Socket: `./node.sock`, DB: `./db-preview`
- Duration: ~02:09 UTC to ~04:00 UTC (~110 minutes)
- Cycles submitted: 43 total (cycles 0–43, batches 1–3)
- Block producer: pool `6954ec11cf7097a693721104139b96c54e7f3e2a8f9e7577630f7856` (Sandstone [SAND])

### Bug #1 FIXED: blocking_read() Panic in connection_lifecycle.rs
- **Symptom**: `thread 'tokio-rt-worker' panicked at crates/dugite-node/src/node/connection_lifecycle.rs:753:69: Cannot block the current thread from within a runtime.`
- **Root cause**: `chain_db.blocking_read()` called inside async task while holding `candidate_chains.read().await` lock
- **Fix**: Move `chain_db.read().await` BEFORE acquiring `candidate_chains` lock; commit `6fa4886b`
- **File**: `crates/dugite-node/src/node/connection_lifecycle.rs` lines ~755-778

### Bug #2 FOUND (NEW): Node Crash After ~21 Minutes at Tip
- **Symptom**: Node PID 3328 died at 03:57 UTC; auto-restarted as PID 9737
- **Evidence**: Log file truncated and restarted; new startup at 03:57:06 UTC
- **Node uptime before crash**: ~21 minutes (started 03:36, crashed ~03:57)
- **No panic/error logged before crash**: process died without leaving a trace in the log
- **Possible cause**: OOM kill, signal, or panic with `panic=abort` (no unwinding, no log)
- **Status**: OPEN — needs investigation (check system logs, add crash handler)
- **Impact**: All pending mempool transactions (cycles 34–38) had TTLs expire during crash recovery

### Bug #3 FOUND (NEW): 10-Minute Sync Stall After Restart
- **Symptom**: After crash recovery, node got stuck at block 4141228 for ~10 minutes despite repeatedly fetching blocks from peers
- **Root cause**: Blocks were arriving in `pending_blocks` buffer out of order; the first connecting block never arrived until a timeout/retry cycle drained the full buffer
- **Mechanism**: BlockFetch workers drain `pending_headers` via `std::mem::take`; if any peer sends a subset of headers, the remaining headers are gone. The `pending_blocks` HashMap at line 1977 (`mod.rs`) buffers blocks keyed by prev_hash; the chain only drains when the FIRST connecting block arrives
- **Recovery**: After 10 minutes the buffer resolved itself (likely a timeout caused re-send)
- **Pattern logged**: Fetching slots 107927055 → 107927515 → 107927762 repeatedly without applying
- **Fix needed**: `pending_blocks` buffer should be drained by sequence number fallback, not just by hash; or headers should not be fully drained until confirmed applied

### Operational Findings
- **LSM lock contention**: First node restart required killing PID 47816 (holding LSM lock); stale lock files block startup
- **UTxO snapshot coverage**: After crash, node's LSM snapshot was at block 4141182; UTxOs from blocks 4141183+ not available until replayed. Submit txs spending UTxOs from blocks well before the snapshot (100+ blocks back)
- **Tx confirmation time**: ~20-60 seconds from submit to on-chain confirmation at tip
- **TTL recommendation**: Use TTL = current_slot + 600 (10 min window) for all soak transactions
- **dugite-cli query utxo bug**: Shows 0 lovelace for all UTxOs and only displays first tx hash; use Koios for UTxO discovery

### Confirmation Statistics (Koios-verified)
- Confirmed on-chain: 35+ transactions (cycles 0–33, batch1, batch2, batch3, cycles 34b–38b)
- TTL-expired (not confirmed): 4 batch2 txs, restart-verify tx, cycles 34–38 (first attempt)
- Rejected at node: ~5 txs (InputNotFound due to UTxO snapshot gap, ValueNotConserved once)
- Average confirmation time: ~25 seconds (fastest: ~15s, slowest: ~120s during stall)

### Why: The test validates end-to-end tx submission pipeline — from N2C LocalTxSubmission → mempool → N2N TxSubmission2 → on-chain inclusion. All three paths worked correctly; issues were in sync/recovery behavior.

### How to apply: When running soak tests post-crash, wait for the node's UTxO store to be fully attached (log line "UTxO store attached (N entries)") before submitting txs. Use UTxOs from blocks significantly older than the snapshot point.
