# Peer Metrics Instrumentation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire up peer handshake RTT and block fetch latency measurements so the dugite-monitor dashboard displays real-time peer performance data.

**Architecture:** The metrics infrastructure (histogram fields, recording methods, Prometheus output, monitor parsing) is already fully implemented but never called. We need to: (1) thread `Arc<NodeMetrics>` into `ConnectionLifecycleManager`, (2) add `Instant::now()` timing around handshakes and block fetches, (3) call the existing `record_handshake_rtt()` and `record_block_fetch_latency()` methods. No new metrics types or monitor changes needed.

**Tech Stack:** Rust, `std::time::Instant`, `Arc<NodeMetrics>`, existing `Histogram` type in `metrics.rs`

---

### Task 1: Thread metrics into ConnectionLifecycleManager

**Files:**
- Modify: `crates/dugite-node/src/node/connection_lifecycle.rs:121-240`
- Modify: `crates/dugite-node/src/node/mod.rs:1767-1781`

- [ ] **Step 1: Add metrics field to ConnectionLifecycleManager struct**

In `crates/dugite-node/src/node/connection_lifecycle.rs`, add the `metrics` field to the struct (around line 140, after `byron_epoch_length`):

```rust
/// Prometheus metrics for recording peer latencies.
metrics: Arc<crate::metrics::NodeMetrics>,
```

Add the import at the top of the file:
```rust
use crate::metrics::NodeMetrics;
```

- [ ] **Step 2: Add metrics parameter to `new()` constructor**

Update the `new()` method signature (around line 215) to accept `metrics: Arc<NodeMetrics>` as the last parameter, and store it in the struct initializer:

```rust
pub fn new(
    network_magic: u64,
    peer_sharing: bool,
    connect_timeout: Duration,
    candidate_chains: Arc<RwLock<HashMap<SocketAddr, CandidateChainState>>>,
    fetched_blocks_tx: mpsc::Sender<FetchedBlock>,
    block_announcement_tx: broadcast::Sender<BlockAnnouncement>,
    chain_db: Arc<RwLock<ChainDB>>,
    ledger_state: Arc<RwLock<LedgerState>>,
    byron_epoch_length: u64,
    metrics: Arc<NodeMetrics>,
) -> Self {
    Self {
        // ... existing fields ...
        metrics,
    }
}
```

- [ ] **Step 3: Pass metrics at the instantiation site in mod.rs**

In `crates/dugite-node/src/node/mod.rs`, at the `ConnectionLifecycleManager::new()` call (around line 1767-1780), add `self.metrics.clone()` as the last argument:

```rust
let mut lifecycle = ConnectionLifecycleManager::new(
    self.network_magic,
    true, // peer_sharing
    Duration::from_secs(5),
    candidate_chains.clone(),
    fetched_blocks_tx,
    block_ann_tx.clone(),
    self.chain_db.clone(),
    self.ledger_state.clone(),
    self.byron_epoch_length,
    self.metrics.clone(),  // <-- add this
);
```

- [ ] **Step 4: Build and verify compilation**

Run: `cargo build --release 2>&1 | tail -5`
Expected: `Finished` with no errors. There may be an `unused field` warning for `metrics` — that's fine, we'll use it in the next task.

- [ ] **Step 5: Commit**

```bash
git add crates/dugite-node/src/node/connection_lifecycle.rs crates/dugite-node/src/node/mod.rs
git commit -m "feat(metrics): thread NodeMetrics into ConnectionLifecycleManager"
```

---

### Task 2: Instrument handshake RTT

**Files:**
- Modify: `crates/dugite-node/src/node/connection_lifecycle.rs:254-284` (promote_to_warm)
- Modify: `crates/dugite-node/src/metrics.rs:536-539` (remove dead_code allow)

- [ ] **Step 1: Write a test that verifies histogram observation**

In `crates/dugite-node/src/metrics.rs`, find the existing test section (around line 1488) and add:

```rust
#[test]
fn test_handshake_rtt_records_to_histogram() {
    let metrics = NodeMetrics::new();
    metrics.record_handshake_rtt(42.0);
    metrics.record_handshake_rtt(150.0);
    let output = metrics.to_prometheus();
    assert!(output.contains("dugite_peer_handshake_rtt_ms_count 2"));
    // 42ms lands in le=50 bucket, 150ms lands in le=250 bucket
    assert!(output.contains("peer_handshake_rtt_ms_bucket{le=\"50\"} 1"));
    assert!(output.contains("peer_handshake_rtt_ms_bucket{le=\"250\"} 2"));
}
```

- [ ] **Step 2: Run the test**

Run: `cargo nextest run -p dugite-node -E 'test(test_handshake_rtt_records_to_histogram)'`
Expected: PASS (the `record_handshake_rtt` method already works)

- [ ] **Step 3: Add timing around handshake in promote_to_warm()**

In `crates/dugite-node/src/node/connection_lifecycle.rs`, in the `promote_to_warm()` method (around lines 263-272), wrap the `PeerConnection::connect()` call with timing and record the RTT:

```rust
info!(%addr, "promoting cold -> warm: connecting");

// Time the TCP connect + handshake for RTT measurement
let connect_start = std::time::Instant::now();

let mut conn = PeerConnection::connect(
    addr,
    self.network_magic,
    self.peer_sharing,
    Some(self.connect_timeout),
)
.await?;

// Record handshake RTT (includes TCP connect + mux setup + handshake exchange)
let rtt_ms = connect_start.elapsed().as_secs_f64() * 1000.0;
self.metrics.record_handshake_rtt(rtt_ms);

info!(%addr, rtt_ms = format_args!("{rtt_ms:.0}"), "cold -> warm complete");
```

Also add identical timing for inbound connections in `accept_inbound()` (around line 411 in connection_lifecycle.rs). Wrap the `PeerConnection::accept()` call (line 432) the same way:

```rust
let accept_start = std::time::Instant::now();
let mut conn =
    PeerConnection::accept(stream, addr, self.network_magic, self.peer_sharing).await?;
let rtt_ms = accept_start.elapsed().as_secs_f64() * 1000.0;
self.metrics.record_handshake_rtt(rtt_ms);
```

- [ ] **Step 4: Remove `#[allow(dead_code)]` from record_handshake_rtt**

In `crates/dugite-node/src/metrics.rs`:
- Remove the `#[allow(dead_code)]` annotation from `record_handshake_rtt` (around line 536)
- Remove the `#[allow(dead_code)]` annotation from `Histogram::observe()` (around line 225), since it's no longer dead code

- [ ] **Step 5: Build and verify**

Run: `cargo build --release 2>&1 | tail -5`
Expected: Clean build, no warnings about dead_code for `record_handshake_rtt`.

- [ ] **Step 6: Commit**

```bash
git add crates/dugite-node/src/node/connection_lifecycle.rs crates/dugite-node/src/metrics.rs
git commit -m "feat(metrics): instrument peer handshake RTT measurement"
```

---

### Task 3: Instrument block fetch latency

**Files:**
- Modify: `crates/dugite-node/src/node/connection_lifecycle.rs:638-800` (make_blockfetch_task)
- Modify: `crates/dugite-node/src/metrics.rs:541-544` (remove dead_code allow)

- [ ] **Step 1: Write a test for block fetch latency recording**

In `crates/dugite-node/src/metrics.rs`, add alongside the existing tests:

```rust
#[test]
fn test_block_fetch_latency_records_to_histogram() {
    let metrics = NodeMetrics::new();
    metrics.record_block_fetch_latency(25.0);
    metrics.record_block_fetch_latency(300.0);
    let output = metrics.to_prometheus();
    assert!(output.contains("dugite_peer_block_fetch_ms_count 2"));
    // 25ms lands in le=25 bucket, 300ms lands in le=500 bucket
    assert!(output.contains("peer_block_fetch_ms_bucket{le=\"25\"} 1"));
    assert!(output.contains("peer_block_fetch_ms_bucket{le=\"500\"} 2"));
}
```

- [ ] **Step 2: Run the test**

Run: `cargo nextest run -p dugite-node -E 'test(test_block_fetch_latency_records_to_histogram)'`
Expected: PASS

- [ ] **Step 3: Add timing around block fetch in make_blockfetch_task()**

In `crates/dugite-node/src/node/connection_lifecycle.rs`, inside the `make_blockfetch_task()` closure, the block fetch loop (around lines 742-780) fetches blocks one at a time. Add timing around the `fetch_range()` call:

```rust
// Before the block fetch call (around line 751):
let fetch_start = std::time::Instant::now();

// ... existing fetch_range() call ...

// After block is successfully received and decoded (around line 761):
let fetch_ms = fetch_start.elapsed().as_secs_f64() * 1000.0;
metrics_clone.record_block_fetch_latency(fetch_ms);
```

The `metrics_clone` needs to be captured in the closure. At the top of `make_blockfetch_task()`, clone the metrics Arc:

```rust
let metrics_clone = self.metrics.clone();
```

Then move it into the async block / closure that the task spawns.

- [ ] **Step 4: Remove `#[allow(dead_code)]` from record_block_fetch_latency**

In `crates/dugite-node/src/metrics.rs`, remove the `#[allow(dead_code)]` annotation from `record_block_fetch_latency` (around line 542).

- [ ] **Step 5: Build and verify**

Run: `cargo build --release 2>&1 | tail -5`
Expected: Clean build

- [ ] **Step 6: Commit**

```bash
git add crates/dugite-node/src/node/connection_lifecycle.rs crates/dugite-node/src/metrics.rs
git commit -m "feat(metrics): instrument block fetch latency measurement"
```

---

### Task 4: Verify end-to-end on live testnet

**Files:** No code changes — verification only

- [ ] **Step 1: Run all tests**

Run: `cargo nextest run --workspace`
Expected: All tests pass

- [ ] **Step 2: Run clippy and fmt**

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --all -- --check`
Expected: Clean

- [ ] **Step 3: Start the node and verify metrics populate**

```bash
pkill -f dugite-node; sleep 3
rm -f db-preview/utxo-store/lock node.sock
./scripts/run-bp-preview.sh --log dugite-bp.log &
sleep 45
curl -s http://localhost:12798/metrics | grep -E 'handshake_rtt|block_fetch_ms'
```

Expected: Histogram buckets should show non-zero counts after the node connects to peers:
```
dugite_peer_handshake_rtt_ms_bucket{le="250"} 3
dugite_peer_handshake_rtt_ms_bucket{le="500"} 7
dugite_peer_handshake_rtt_ms_count 8
```

- [ ] **Step 4: Verify dugite-monitor shows RTT data**

Run: `./target/release/dugite-monitor --socket-path ./node.sock`

The peer panel should now display RTT bands and percentiles (p50, p95) instead of showing zeros or "N/A".

- [ ] **Step 5: Final commit with any adjustments**

If any adjustments were needed during verification, commit them:
```bash
git add -A
git commit -m "fix(metrics): adjustments from live testnet verification"
```
