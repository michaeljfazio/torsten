---
name: ChainSync log flood from inbound Haskell syncer causes sync stall
description: Inbound Haskell node syncing from Dugite generates 1.2M+ "ChainSync at tip" INFO log messages per 10 minutes, creating I/O saturation that causes Dugite's own sync to stall after ~40 minutes
type: project
---

When a Haskell cardano-node connects to Dugite as an inbound peer and begins syncing the entire chain from scratch (e.g. from genesis), it generates a ChainSync server flood:

- Each header delivered by Dugite's ChainSync server logs: "ChainSync at tip — awaiting new blocks peer_addr=127.0.0.1:3002 headers_received=N"
- Rate: ~1.2M log messages per 10 minutes (~2000/sec)
- The log file fills at ~120MB/10min
- This I/O saturation causes Dugite's main sync loop to stall: no new "Chain extended" messages, stuck at same block for 10-15 minutes

**Why:** The INFO log event fires on EVERY header sent to the Haskell peer. When the Haskell node syncs from genesis, it requests 4M+ headers sequentially. The sync log at INFO level is too verbose.

**How to apply:**
1. The "ChainSync at tip" log should be at DEBUG level, not INFO
2. Or rate-limit the server-side logging (log every 10K headers, not every header)
3. Rotate logs with `tail -N` (not mv) to preserve the file descriptor — `mv` cuts the link and the running process writes to a deleted inode
4. The underlying stall is a symptom of log I/O overload, not a BlockFetch bug

**File to fix:** `crates/dugite-node/src/node/sync.rs` — find "ChainSync at tip" log and change INFO to DEBUG or add a counter modulo gate.

**Soak test observation (2026-03-27):** Node started at 05:28 UTC, first stall at ~06:04 (36 min), second at ~07:11 (40 min run). After fix deployment, log flood from 127.0.0.1:3002 stopped the log rotation from helping.
