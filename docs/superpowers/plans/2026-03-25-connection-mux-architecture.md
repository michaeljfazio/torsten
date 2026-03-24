# Connection Mux Architecture Refactor

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refactor the connection and protocol lifecycle to match the Haskell cardano-node architecture: ONE muxed TCP connection per peer, temperature-based protocol activation (Cold→Warm→Hot), all protocols as independent tokio tasks sharing the same mux, and a separate BlockFetch decision task.

**Architecture:** Each peer gets a single `PeerConnection` that owns a `Mux` with channels for all protocols. The Governor manages peer temperature transitions. At Warm, KeepAlive runs. At Hot, ChainSync + BlockFetch + TxSubmission2 are started as independent tasks on the same mux. A dedicated `BlockFetchDecisionTask` reads candidate chain state from ChainSync and issues fetch requests to per-peer BlockFetch tasks. The sync loop is replaced by independent per-peer ChainSync client tasks.

**Tech Stack:** Rust, tokio, torsten-network (Bearer, Mux, protocol clients/servers), tokio::sync (watch, mpsc, broadcast)

**Reference:** Haskell source analysis (ouroboros-network/network-mux, ouroboros-network/PeerStateActions, ouroboros-network/BlockFetch)

---

## Key Haskell Architecture Facts (from research)

These are NOT negotiable — our implementation MUST match:

1. **ONE TCP connection per peer** carrying ALL mini-protocols via mux
2. **3 mux threads**: egress (muxer), ingress (demuxer), monitor — we have reader+writer which is equivalent
3. **Per-protocol tasks**: each protocol runs in its own thread/task
4. **Temperature lifecycle**: Cold→Warm (KeepAlive starts) → Hot (ChainSync+BlockFetch+TxSubmission2 start)
5. **BlockFetch is independent**: a separate decision loop reads candidate chains (STM/watch) and issues fetch requests — it does NOT run inline with ChainSync
6. **TxSubmission2 MsgInit**: sent immediately when Hot protocols start
7. **SDU fair scheduling**: round-robin Wanton model — one SDU per protocol per round
8. **Protocol idle timeout**: ChainSync StIdle = 3373s, KeepAlive = 97s client / 60s server, SDU timeout = 30s
9. **Simultaneous open**: when both inbound+outbound exist, promote to Duplex on the existing connection

---

## File Structure

```
crates/torsten-node/src/node/
├── mod.rs                    # MODIFY: Remove dual-connection paths, use PeerConnection
├── peer_connection.rs        # CREATE: PeerConnection — owns one mux, all protocol channels
├── connection_lifecycle.rs   # CREATE: Cold→Warm→Hot transitions, protocol start/stop
├── block_fetch_logic.rs      # CREATE: Independent BlockFetch decision task
├── sync.rs                   # MODIFY: ChainSync becomes a per-peer task, not the main loop
├── networking.rs             # MODIFY: Remove types that duplicate network crate, add PeerConnection types

crates/torsten-network/src/
├── mux/mod.rs                # MODIFY: Verify SDU direction byte matches Haskell (byte 6, not bit 15)
├── mux/egress.rs             # MODIFY: Implement fair round-robin Wanton scheduling
```

---

## Task 1: Create PeerConnection — One Mux Per Peer

**Files:**
- Create: `crates/torsten-node/src/node/peer_connection.rs`

A `PeerConnection` encapsulates a single multiplexed TCP connection to one peer. It owns the mux and provides protocol channels. ALL protocols share the same underlying TCP stream.

- [ ] **Step 1: Define PeerConnection struct**

```rust
/// A single multiplexed connection to a peer.
///
/// Matches the Haskell ConnectionHandler architecture: one TCP stream,
/// one Mux instance, channels for all N2N protocols. Protocols are started
/// and stopped based on temperature (Warm/Hot) without creating new connections.
pub struct PeerConnection {
    /// Peer address.
    pub addr: SocketAddr,
    /// Negotiated protocol version (e.g., 15 for V15).
    pub version: u16,
    /// Network magic used in handshake.
    pub network_magic: u64,

    // Protocol channels — created during mux setup, consumed by protocol tasks
    pub chainsync_channel: Option<MuxChannel>,   // Protocol 2
    pub blockfetch_channel: Option<MuxChannel>,   // Protocol 3
    pub txsubmission_channel: Option<MuxChannel>, // Protocol 4
    pub keepalive_channel: Option<MuxChannel>,    // Protocol 8
    pub peersharing_channel: Option<MuxChannel>,  // Protocol 10

    /// Mux task handle — kept alive for the connection's lifetime.
    mux_handle: JoinHandle<Result<(), MuxError>>,
    /// Cancellation token for graceful shutdown of all protocols.
    cancel: CancellationToken,
    /// Handles for running protocol tasks (aborted on temperature changes).
    hot_tasks: Vec<JoinHandle<()>>,
    warm_tasks: Vec<JoinHandle<()>>,
}
```

- [ ] **Step 2: Implement PeerConnection::connect() — outbound**

Establishes TCP connection, creates bearer, sets up mux with ALL protocol channels, runs handshake. Returns a PeerConnection ready for Warm activation.

```rust
impl PeerConnection {
    pub async fn connect(
        addr: SocketAddr,
        network_magic: u64,
        peer_sharing: bool,
        timeout: Duration,
    ) -> Result<Self, NetworkError> {
        // 1. TCP connect with timeout
        let bearer = tokio::time::timeout(timeout, TcpBearer::connect(addr)).await??;

        // 2. Create Mux and subscribe ALL protocol channels upfront
        let mut mux = Mux::new(bearer, true); // we are initiator
        let mut hs_ch = mux.subscribe(PROTOCOL_HANDSHAKE, Direction::InitiatorDir, 65536);
        let cs_ch = mux.subscribe(PROTOCOL_N2N_CHAINSYNC, Direction::InitiatorDir, 1_048_576);
        let bf_ch = mux.subscribe(PROTOCOL_N2N_BLOCKFETCH, Direction::InitiatorDir, 4_194_304);
        let tx_ch = mux.subscribe(PROTOCOL_N2N_TXSUBMISSION, Direction::InitiatorDir, 65536);
        let ka_ch = mux.subscribe(PROTOCOL_N2N_KEEPALIVE, Direction::InitiatorDir, 65536);
        let ps_ch = mux.subscribe(PROTOCOL_N2N_PEERSHARING, Direction::InitiatorDir, 65536);

        // 3. Start the mux (runs reader + writer tasks)
        let mux_handle = tokio::spawn(async move { mux.run().await });

        // 4. Run N2N handshake
        let our_data = N2NVersionData::new(network_magic, peer_sharing);
        let hs_result = run_n2n_handshake_client(&mut hs_ch, &our_data).await?;

        Ok(Self {
            addr,
            version: hs_result.version,
            network_magic,
            chainsync_channel: Some(cs_ch),
            blockfetch_channel: Some(bf_ch),
            txsubmission_channel: Some(tx_ch),
            keepalive_channel: Some(ka_ch),
            peersharing_channel: Some(ps_ch),
            mux_handle,
            cancel: CancellationToken::new(),
            hot_tasks: Vec::new(),
            warm_tasks: Vec::new(),
        })
    }
}
```

- [ ] **Step 3: Implement accept() — inbound connections**

Similar to connect() but for inbound TCP connections from the N2N listener. Runs handshake as server.

- [ ] **Step 4: Implement start_warm_protocols() and start_hot_protocols()**

```rust
/// Start Warm-temperature protocols (KeepAlive, PeerSharing responder).
/// Called when transitioning Cold → Warm.
pub fn start_warm_protocols(&mut self, ...) {
    // KeepAlive client — send periodic pings
    let ka_ch = self.keepalive_channel.take().expect("channel not started");
    let cancel = self.cancel.clone();
    self.warm_tasks.push(tokio::spawn(async move {
        let client = KeepAliveClient::new(Duration::from_secs(90), cancel);
        let _ = client.run(&mut ka_ch).await;
    }));
}

/// Start Hot-temperature protocols (ChainSync, BlockFetch, TxSubmission2).
/// Called when transitioning Warm → Hot.
pub fn start_hot_protocols(&mut self, ...) {
    // ALL hot protocols start SIMULTANEOUSLY (matching Haskell StartEagerly)

    // ChainSync client — runs intersection finding + header streaming
    // BlockFetch client — channel given to the BlockFetchDecision task
    // TxSubmission2 — sends MsgInit immediately, then responds to server requests
}
```

- [ ] **Step 5: Implement stop_hot_protocols() and shutdown()**

Graceful shutdown with 5s timeout matching Haskell `spsDeactivateTimeout`.

- [ ] **Step 6: Add tests for PeerConnection lifecycle**

- [ ] **Step 7: Commit**

---

## Task 2: Create Connection Lifecycle Manager

**Files:**
- Create: `crates/torsten-node/src/node/connection_lifecycle.rs`

Manages the Cold→Warm→Hot temperature transitions for all peer connections. Replaces the current Governor connect tasks + sync loop dual-connection pattern.

- [ ] **Step 1: Define ConnectionLifecycleManager**

```rust
/// Manages per-peer connections and temperature transitions.
///
/// Matches Haskell PeerStateActions: Cold→Warm starts KeepAlive on the
/// single mux connection, Warm→Hot starts ChainSync+BlockFetch+TxSubmission2
/// on the SAME connection.
pub struct ConnectionLifecycleManager {
    /// Active connections indexed by peer address.
    connections: HashMap<SocketAddr, PeerConnection>,
    /// Block announcement broadcast for ChainSync servers.
    block_announcement_tx: broadcast::Sender<BlockAnnouncement>,
    /// Shared state for BlockFetch decision task.
    candidate_chains: Arc<RwLock<HashMap<SocketAddr, CandidateChain>>>,
}
```

- [ ] **Step 2: Implement promote_to_warm()**

Creates the TCP connection if not exists, runs handshake, starts KeepAlive.

- [ ] **Step 3: Implement promote_to_hot()**

On the EXISTING connection (no new TCP!), starts ChainSync + BlockFetch + TxSubmission2 as independent tasks. Sends TxSubmission2 MsgInit immediately.

- [ ] **Step 4: Implement demote_to_warm()**

Stops hot protocol tasks, keeps KeepAlive running on the same connection.

- [ ] **Step 5: Implement demote_to_cold()**

Shuts down all protocols and closes the TCP connection.

- [ ] **Step 6: Implement accept_inbound()**

Handles inbound connections from the N2N listener. Creates PeerConnection, starts warm protocols. If an outbound connection already exists to the same peer, promote to Duplex.

- [ ] **Step 7: Commit**

---

## Task 3: Create Independent BlockFetch Decision Task

**Files:**
- Create: `crates/torsten-node/src/node/block_fetch_logic.rs`

A dedicated tokio task that continuously reads candidate chain state from all ChainSync clients and decides which blocks to fetch from which peers. Matches Haskell's `blockFetchLogic` thread.

- [ ] **Step 1: Define CandidateChain and BlockFetchDecisionTask**

```rust
/// Candidate chain fragment from a peer's ChainSync.
pub struct CandidateChain {
    pub peer: SocketAddr,
    pub tip_slot: u64,
    pub tip_hash: [u8; 32],
    pub headers: Vec<HeaderInfo>,  // headers not yet fetched
}

/// Runs the block fetch decision loop.
/// Reads candidate_chains (updated by ChainSync tasks) every 10ms (Praos)
/// or 40ms (Genesis), decides which blocks to fetch, sends fetch requests
/// to per-peer BlockFetch channels.
pub struct BlockFetchDecisionTask { ... }
```

- [ ] **Step 2: Implement the decision loop**

Matching Haskell's `fetchLogicIteration`:
1. Wait for candidate chain state to change
2. For each peer: filter plausible candidates, remove already-fetched/in-flight blocks
3. Issue `FetchRequest` to the best peer's BlockFetch channel
4. Sleep for decision interval (10ms Praos / 40ms Genesis)

- [ ] **Step 3: Implement fetch request handling in BlockFetch client task**

Each peer's BlockFetch runs as an independent task that receives fetch requests from the decision task and calls `BlockFetchClient::fetch_range()`.

- [ ] **Step 4: Implement fetched block delivery**

Fetched blocks are sent to the main processing pipeline via an mpsc channel. The node's run loop receives and applies them to the ledger.

- [ ] **Step 5: Tests**

- [ ] **Step 6: Commit**

---

## Task 4: Refactor ChainSync as Per-Peer Independent Task

**Files:**
- Modify: `crates/torsten-node/src/node/sync.rs`

ChainSync becomes an independent per-peer tokio task that:
1. Finds intersection
2. Streams headers via pipelined MsgRequestNext
3. Updates the shared `candidate_chains` state (for BlockFetch decision task)
4. Does NOT fetch blocks itself — that's BlockFetch's job

- [ ] **Step 1: Extract ChainSync client task**

```rust
/// Per-peer ChainSync client task.
///
/// Runs on a single MuxChannel, receives headers, updates candidate chain
/// state. Does NOT fetch blocks — that's the BlockFetch decision task's job.
pub async fn chainsync_client_task(
    mut channel: MuxChannel,
    peer_addr: SocketAddr,
    known_points: Vec<Point>,
    candidate_chains: Arc<RwLock<HashMap<SocketAddr, CandidateChain>>>,
    cancel: CancellationToken,
) -> Result<()> {
    // 1. Find intersection
    // 2. Pipeline MsgRequestNext (high_mark=300)
    // 3. For each MsgRollForward: update candidate_chains
    // 4. For each MsgRollBackward: update candidate_chains
    // 5. MsgAwaitReply: at tip, slow down
}
```

- [ ] **Step 2: Remove sync loop from mod.rs run()**

The sync loop is replaced by per-peer ChainSync tasks spawned during Hot promotion. The run loop only processes fetched blocks from the block delivery channel.

- [ ] **Step 3: Move block processing to the main run loop**

The main run loop receives fetched+decoded blocks from a channel and applies them to the ledger. This replaces the inline sync loop.

- [ ] **Step 4: Preserve fork recovery logic**

Move fork recovery (origin intersection detection, deep rollback) into the ChainSync task. When a rollback is detected, the task updates candidate_chains and the main loop handles the ledger rollback.

- [ ] **Step 5: Tests**

- [ ] **Step 6: Commit**

---

## Task 5: Refactor mod.rs Run Loop

**Files:**
- Modify: `crates/torsten-node/src/node/mod.rs`

Remove the current dual-connection Governor+Sync pattern. Replace with:

- [ ] **Step 1: Replace Node struct fields**

```rust
pub struct Node {
    // ... existing fields ...
    /// Per-peer connection manager (one connection per peer).
    connection_manager: ConnectionLifecycleManager,
    /// Block fetch decision task handle.
    block_fetch_task: Option<JoinHandle<()>>,
    /// Channel for receiving fetched blocks from BlockFetch.
    fetched_blocks_rx: mpsc::Receiver<FetchedBlock>,
}
```

- [ ] **Step 2: Rewrite the Governor loop**

The Governor now only emits Connect/Disconnect/Promote/Demote events. The ConnectionLifecycleManager handles them:

```rust
for event in governor.evaluate(&peer_manager) {
    match event {
        GovernorEvent::Connect(addr) => {
            connection_manager.promote_to_warm(addr, ...).await;
        }
        GovernorEvent::Promote(addr) => {
            connection_manager.promote_to_hot(addr, ...).await;
        }
        GovernorEvent::Demote(addr) => {
            connection_manager.demote_to_warm(addr).await;
        }
        GovernorEvent::Disconnect(addr) => {
            connection_manager.demote_to_cold(addr).await;
        }
    }
}
```

- [ ] **Step 3: Rewrite the main run loop**

```rust
loop {
    tokio::select! {
        // Process fetched blocks
        Some(block) = fetched_blocks_rx.recv() => {
            self.apply_block(block).await;
        }
        // Governor evaluation (periodic)
        _ = governor_ticker.tick() => {
            let events = governor.evaluate(&peer_manager);
            for event in events { ... }
        }
        // Forge ticker
        _ = forge_ticker.tick(), if self.block_producer.is_some() => {
            self.try_forge_block().await;
        }
        // Shutdown
        _ = shutdown_rx.changed() => break,
    }
}
```

- [ ] **Step 4: Remove the sync connection path**

Delete the section in mod.rs that creates a SEPARATE connection for sync. ChainSync now runs on the same connection that the Governor manages.

- [ ] **Step 5: Full integration test**

- [ ] **Step 6: Commit**

---

## Task 6: Verify SDU Wire Format

**Files:**
- Modify: `crates/torsten-network/src/mux/segment.rs` (if needed)

- [ ] **Step 1: Verify SDU header format matches Haskell**

Haskell SDU header (8 bytes):
```
Bytes 0-3: RemoteClockModel (u32 BE, microseconds)
Bytes 4-5: MiniProtocolNum (u16 BE, protocol number)
Byte  6:   MiniProtocolDir (0x00 = Initiator, 0x01 = Responder)
Bytes 7-8: WRONG — actually it's:
  Bytes 4-5: protocol_and_dir (u16 BE, bit 15 = direction, bits 0-14 = protocol)
  Bytes 6-7: payload_length (u16 BE)
```

Actually check the Haskell source carefully — there's a discrepancy in the research about whether direction is bit 15 of bytes 4-5 or a separate byte 6. Verify against the actual wire captures we already have.

Our captures show: `0000 000f` at bytes 4-7 for protocol 0, InitiatorDir, payload 15. If direction were a separate byte, it would be `0000 00 000f`. Our current encoding (bit 15) produces `0000 000f` which matches. So our encoding IS correct.

- [ ] **Step 2: Verify against Haskell wire capture**

Compare our SDU bytes with the Haskell capture from the tcpdump session. Confirm byte-for-byte match.

- [ ] **Step 3: Commit (only if changes needed)**

---

## Task 7: Workspace Build + Test + Testnet Verification

- [ ] **Step 1: Full workspace build**: `cargo build --all-targets`
- [ ] **Step 2: All tests pass**: `cargo test --all`
- [ ] **Step 3: Clippy clean**: `cargo clippy --all-targets -- -D warnings`
- [ ] **Step 4: Format clean**: `cargo fmt --all -- --check`
- [ ] **Step 5: Run node on preview testnet**:
  - Verify peer connections succeed (ONE per peer)
  - Verify ChainSync receives headers
  - Verify BlockFetch downloads blocks
  - Verify blocks are applied to ledger
  - Verify N2C queries work via torsten-cli
  - Verify Prometheus metrics on port 12798
- [ ] **Step 6: Commit and push**

---

## Risk Assessment

| Task | Risk | Effort | Notes |
|------|------|--------|-------|
| 1. PeerConnection | Medium | Large | New abstraction, clean design |
| 2. ConnectionLifecycle | Medium | Large | Replaces Governor connect logic |
| 3. BlockFetchDecision | High | Large | New concurrent architecture |
| 4. ChainSync refactor | High | Large | Fundamental change to sync flow |
| 5. mod.rs refactor | High | Large | Touches the main run loop |
| 6. SDU verify | Low | Small | Likely just confirmation |
| 7. Verification | Medium | Medium | Runtime testing |

**Total estimated effort**: Large refactor (~2000-3000 lines changed across 6 files).

**Critical path**: Tasks 1→2→4→5 (PeerConnection must exist before lifecycle, ChainSync must be refactored before mod.rs). Task 3 (BlockFetch) can be done in parallel with Task 4.
