# Networking Layer Rewrite: Design Specification

**Date:** 2026-03-24
**Status:** Approved
**Scope:** Complete replacement of `crates/torsten-network/` internals, removing all `pallas-network` dependencies

## Motivation

The current networking layer depends on `pallas-network` for multiplexing, handshake, and several mini-protocol client implementations. This dependency is problematic:

1. **pallas-network is incomplete** — it does not fully implement all Ouroboros mini-protocols, requiring us to bypass its state machines (e.g., pipelined ChainSync uses raw `ChannelBuffer` instead of pallas's `chainsync::Client`)
2. **pallas-network is being replaced** — the pallas team considers `pallas-network` redundant and is building `pallas-network2` as a replacement, making the current API unstable
3. **Correctness gaps** — pallas uses an incorrect SDU payload size (65,535 vs Haskell's 12,288 for TCP), doesn't handle direction bit flipping correctly for duplex connections, and has other subtle wire-format differences from the Haskell reference
4. **Hybrid architecture** — the current codebase is part pallas, part custom (N2C server, TxSubmission2, PeerSharing are custom; handshake, multiplexer, BlockFetch, KeepAlive use pallas). This creates maintenance burden and inconsistent patterns.

A ground-up rewrite aligned to the Haskell reference implementation gives us full control, correct wire-format behavior, and no upstream dependency risk.

## Design Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Scope | Full rewrite, one shot | Clean break, no legacy shims or migration complexity |
| Pallas deps | Zero pallas imports in torsten-network | Eliminates version coupling; our primitives crate wraps what we need |
| Multiplexer | Exact wire format, idiomatic Rust internals | Match Haskell on the wire (12,288 SDU, direction bits), use tokio channels internally |
| State machines | Runtime enum with debug assertions | ChainSync pipelining makes type-states impractical; runtime checks are sufficient |
| Connection lifecycle | Full Haskell parity including simultaneous open | Production mainnet requires correct duplex and connection merging |
| Protocol versions | N2N V14-V15, N2C V16-V23 | Matches production cardano-node 10.x; skip experimental Peras (V16 N2N) |
| Testing | Unit + conformance wire traces + live testnet | Wire trace replay catches the subtle encoding bugs that have bitten us before |

## Architecture

Four clean layers, each independently testable:

```
Layer 4: Connection Manager  (lifecycle, simultaneous open, warm/hot promotion)
Layer 3: Mini-Protocols      (ChainSync, BlockFetch, TxSubmission2, etc.)
Layer 2: Multiplexer         (segmentation, fairness, ingress/egress queues)
Layer 1: Bearer              (TCP streams, Unix sockets)
```

Each layer only depends on the one below it. Mini-protocols are independent of each other. The connection manager orchestrates which protocols run on which connections.

## Module Structure

```
crates/torsten-network/src/
├── lib.rs                          # Public API: traits, re-exports
├── bearer/
│   ├── mod.rs                      # Bearer trait definition
│   ├── tcp.rs                      # TCP bearer (async read/write, keepalive)
│   └── unix.rs                     # Unix domain socket bearer (N2C)
├── mux/
│   ├── mod.rs                      # Mux public API
│   ├── segment.rs                  # SDU header encoding/decoding (8-byte wire format)
│   ├── egress.rs                   # Egress queue, round-robin fairness, batched writes
│   ├── ingress.rs                  # Ingress demuxer, per-protocol queues with byte limits
│   └── channel.rs                  # MuxChannel: typed per-protocol read/write handle
├── handshake/
│   ├── mod.rs                      # Handshake protocol (shared N2N/N2C logic)
│   ├── n2n.rs                      # N2N version table, version data codec (V14-V15)
│   └── n2c.rs                      # N2C version table, version data codec (V16-V23)
├── protocol/
│   ├── mod.rs                      # Shared protocol types (Agency, State enums)
│   ├── chainsync/
│   │   ├── mod.rs                  # ChainSync state machine, message codec
│   │   ├── client.rs              # Pipelined ChainSync client (outbound sync)
│   │   └── server.rs             # ChainSync server (serve headers to peers)
│   ├── blockfetch/
│   │   ├── mod.rs                  # BlockFetch state machine, message codec
│   │   ├── client.rs              # BlockFetch client (download block ranges)
│   │   └── server.rs             # BlockFetch server (serve blocks to peers)
│   ├── txsubmission/
│   │   ├── mod.rs                  # TxSubmission2 state machine, message codec
│   │   ├── client.rs              # TxSubmission2 client (announce/send txs)
│   │   └── server.rs             # TxSubmission2 server (request txs from peers)
│   ├── keepalive/
│   │   ├── mod.rs                  # KeepAlive codec
│   │   ├── client.rs              # KeepAlive sender (ping with cookie)
│   │   └── server.rs             # KeepAlive responder (echo cookie)
│   ├── peersharing/
│   │   ├── mod.rs                  # PeerSharing codec
│   │   ├── client.rs              # PeerSharing requester
│   │   └── server.rs             # PeerSharing responder
│   ├── local_chainsync/
│   │   └── server.rs             # N2C LocalChainSync (full blocks, not headers)
│   ├── local_tx_submission/
│   │   └── server.rs             # N2C LocalTxSubmission
│   ├── local_state_query/
│   │   ├── mod.rs                  # Acquire/release state machine
│   │   ├── server.rs             # Query dispatch
│   │   └── encoding.rs           # Query-specific CBOR encoding
│   └── local_tx_monitor/
│       └── server.rs             # N2C LocalTxMonitor with snapshot semantics
├── connection/
│   ├── mod.rs                      # ConnectionManager public API
│   ├── manager.rs                 # Connection lifecycle, simultaneous open detection
│   ├── state.rs                   # Connection state machine
│   └── handler.rs                 # Per-connection protocol orchestration
├── peer/
│   ├── mod.rs                      # PeerManager + Governor
│   ├── manager.rs                 # Peer state, reputation, EWMA latency
│   ├── governor.rs                # Target-driven promotion/demotion decisions
│   ├── discovery.rs               # DNS, ledger-based, peer sharing discovery
│   └── selection.rs               # Peer selection algorithms
├── codec.rs                        # CBOR encoding/decoding helpers (minicbor-based)
├── error.rs                        # Unified error types
├── metrics.rs                      # Prometheus metrics
└── tests/
    ├── conformance/               # Wire trace replay tests
    ├── mux_tests.rs               # Multiplexer unit tests
    ├── protocol_tests.rs          # Per-protocol state machine tests
    └── integration.rs             # Full connection lifecycle tests
```

## Layer 1: Bearer

Thin async transport abstraction.

### Bearer Trait

```rust
#[async_trait]
pub trait Bearer: Send + 'static {
    async fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), BearerError>;
    async fn write_all(&mut self, buf: &[u8]) -> Result<(), BearerError>;
    async fn flush(&mut self) -> Result<(), BearerError>;
    async fn close(&mut self) -> Result<(), BearerError>;
    fn sdu_size(&self) -> usize;
    fn batch_size(&self) -> usize;
}
```

### Bearer Types

| Bearer | SDU Size | Batch Size | Notes |
|---|---|---|---|
| TCP | 12,288 | 131,072 | `TCP_NODELAY=false`, `SO_KEEPALIVE=true` (60s), 131KB read buffer |
| Unix | 32,768 | 32,768 | For N2C connections |
| Mock | configurable | configurable | For testing — replay captured wire traces |

The trait abstraction enables conformance testing with pre-recorded SDU sequences via the mock bearer.

## Layer 2: Multiplexer

One mux instance per connection, managing all mini-protocols.

### SDU Wire Format

```
Bytes 0-3:  transmission_time  u32 BE  (microseconds, monotonic clock)
Bytes 4-5:  protocol_and_dir   u16 BE  (bit 15 = direction, bits 0-14 = protocol number)
Bytes 6-7:  payload_length     u16 BE  (0..=sdu_size)
Bytes 8..:  payload            [u8; payload_length]
```

**Direction bit semantics:**
- Bit 15 = 0: InitiatorDir (sent by TCP initiator)
- Bit 15 = 1: ResponderDir (sent by TCP responder)
- On ingress, the direction is **flipped** before dispatch — data sent as InitiatorDir by the remote is delivered to the local ResponderDir handler, and vice versa

### Internal Architecture

Three tokio tasks per mux:

1. **Ingress task** — reads SDUs from bearer, flips direction bit, dispatches payload to per-protocol ingress queues. Enforces per-protocol byte limits; exceeding tears down the connection (`IngressQueueOverRun`, matching Haskell).

2. **Egress task** — drains from shared egress queue in round-robin order. Each protocol can have at most one pending write at a time (fairness). Messages exceeding `sdu_size` are segmented: one chunk is written, the remainder re-enqueued at the back so other protocols get a turn. Batches up to 100 SDUs or `batch_size` bytes per `write_all` call.

3. **Control task** — monitors bearer errors, protocol violations, shutdown signals. Propagates to all channels and bearer.

### MuxChannel

Per-protocol handle provided to protocol handlers:

```rust
pub struct MuxChannel {
    protocol_id: u16,
    direction: Direction,
    egress_tx: mpsc::Sender<Bytes>,     // Send complete messages; egress task segments
    ingress_rx: mpsc::Receiver<Bytes>,  // Receive reassembled complete messages
}

impl MuxChannel {
    pub async fn send(&self, msg: &[u8]) -> Result<(), MuxError>;
    pub async fn recv(&mut self) -> Result<Bytes, MuxError>;
    pub fn try_recv(&mut self) -> Result<Option<Bytes>, MuxError>;
}
```

### Message Reassembly

The ingress task accumulates payload bytes per `(protocol_id, direction)` pair and detects message completeness by attempting CBOR decode after each segment.

### Ingress Queue Byte Limits

| Protocol | Limit | Derivation |
|---|---|---|
| ChainSync | 462,000 | `highMark(300) * 1400 * 1.1` |
| BlockFetch | 23,068,694 | `max(10*2097154, 100*90112) * 1.1` |
| TxSubmission2 | 65,536 | Bounded by max tx size |
| KeepAlive | 1,408 | `1280 * 1.1` |
| PeerSharing | 5,760 | `4 * 1440` |
| N2C protocols | 4,294,967,295 | Effectively unlimited (local) |

## Layer 3: Mini-Protocols

### Handshake (Protocol ID 0)

Runs through the mux as the first mini-protocol exchange.

**State machine:** `StPropose → StConfirm → StDone`

**Wire format:**
```
MsgProposeVersions = [0, { version_number => version_data }]
MsgAcceptVersion   = [1, version_number, version_data]
MsgRefuse          = [2, refuse_reason]
MsgQueryReply      = [3, { version_number => version_data }]

refuse_reason = [0, [*version_number]]           // VersionMismatch
              / [1, version_number, text]         // HandshakeDecodeError
              / [2, version_number, text]         // Refused
```

**N2N version data** (V14-V15, CBOR list of 4):
```
[network_magic: u32, initiator_only: bool, peer_sharing: 0|1, query: bool]
```

**Acceptance logic** (matching Haskell):
- `network_magic`: must match exactly — refuse if different
- `initiator_only`: take `min` (either side initiator-only → unidirectional)
- `peer_sharing`: AND (both must enable)
- `query`: OR (either side can request)

**N2C version data:** `[network_magic: u32, query: bool]`
N2C version numbers have bit 15 set: V16=32784, V17=32785, ..., V23=32791.

**Simultaneous open:** When outbound handshake receives tag 0 instead of tag 1/2/3, treat as simultaneous open. Compute version intersection locally. Resolve which connection survives by lexicographic comparison of `(IP, port)` — lower address initiator wins. Surviving connection becomes duplex.

### N2N ChainSync (Protocol ID 2)

**State machine:**

| State | Agency | Allowed Messages |
|---|---|---|
| StIdle | Client | MsgRequestNext [0], MsgFindIntersect [4], MsgDone [7] |
| StCanAwait | Server | MsgRollForward [2], MsgRollBackward [3], MsgAwaitReply [1] |
| StMustReply | Server | MsgRollForward [2], MsgRollBackward [3] |
| StIntersect | Server | MsgIntersectFound [5], MsgIntersectNotFound [6] |
| StDone | Nobody | — |

**Wire format:**
```
MsgRequestNext       = [0]
MsgAwaitReply        = [1]
MsgRollForward       = [2, header, tip]
MsgRollBackward      = [3, point, tip]
MsgFindIntersect     = [4, [*point]]
MsgIntersectFound    = [5, point, tip]
MsgIntersectNotFound = [6, tip]
MsgDone              = [7]
```

N2N sends **headers only** in MsgRollForward, wrapped as `[era_id, CBOR_tag_24(header_bytes)]`.

**Client pipelining** (matching Haskell's `pipelineDecisionLowHighMark`):
- `low_mark = 200`, `high_mark = 300`
- goLow phase: send MsgRequestNext without waiting until `outstanding >= high_mark`
- goHigh phase: collect responses until `outstanding <= low_mark`
- At tip (peer's tip == our latest received): switch to non-pipelined single request-wait

**Server:**
- Per-peer cursor (slot + hash)
- MsgFindIntersect: walk peer's point list, find best intersection via BlockProvider
- MsgRequestNext: serve next header after cursor, or enter StMustReply waiting for block announcement
- Rollback: send MsgRollBackward when chain rolled back past cursor

### N2N BlockFetch (Protocol ID 3)

**State machine:**

| State | Agency | Allowed Messages |
|---|---|---|
| BFIdle | Client | MsgRequestRange [0], MsgClientDone [1] |
| BFBusy | Server | MsgStartBatch [2], MsgNoBlocks [3] |
| BFStreaming | Server | MsgBlock [4], MsgBatchDone [5] |
| BFDone | Nobody | — |

**Wire format:**
```
MsgRequestRange = [0, point, point]
MsgClientDone   = [1]
MsgStartBatch   = [2]
MsgNoBlocks     = [3]
MsgBlock        = [4, block]
MsgBatchDone    = [5]
```

Blocks are sent as `[era_id, CBOR_tag_24(block_bytes)]`. Server streams blocks sequentially within a batch. Client supports batch-level pipelining.

Server enforces configurable max slot range AND max block count per range (fixing the Byron-era density issue from the review).

### N2N TxSubmission2 (Protocol ID 4)

Pull-based protocol — **server drives** (inverted agency).

**State machine:**

| State | Agency | Allowed Messages |
|---|---|---|
| StInit | Client | MsgInit [6] |
| StIdle | **Server** | MsgRequestTxIds [0], MsgRequestTxs [2] |
| StTxIds(Blocking) | Client | MsgReplyTxIds [1] (non-empty), MsgDone [4] |
| StTxIds(NonBlocking) | Client | MsgReplyTxIds [1] (may be empty) |
| StTxs | Client | MsgReplyTxs [3] |
| StDone | Nobody | — |

**Wire format:**
```
MsgInit         = [6]
MsgRequestTxIds = [0, blocking: bool, ack_count: u16, req_count: u16]
MsgReplyTxIds   = [1, [*(tx_id, size_in_bytes: u32)]]
MsgRequestTxs   = [2, [*tx_id]]
MsgReplyTxs     = [3, [*tx]]
MsgDone         = [4]
```

**Flow control rules:**
- First MsgRequestTxIds after MsgInit: `blocking=false`, `ack_count=0`
- `blocking=true` only valid when zero unacknowledged tx IDs remain
- MsgDone only valid in blocking state
- Server maintains FIFO of announced-but-unacknowledged tx IDs
- Use `HashSet<[u8; 32]>` for inflight dedup (O(1) vs current O(n))

### N2N KeepAlive (Protocol ID 8)

**State machine:** `StClient → StServer → StClient` (loop), `StClient → StDone`

**Wire format:**
```
MsgKeepAlive         = [0, cookie: u16]
MsgKeepAliveResponse = [1, cookie: u16]
MsgDone              = [2]
```

Cookie must be echoed exactly; mismatch = disconnect. Used for RTT measurement (EWMA latency). Default interval 30s. Runs when peer is warm or hot.

### N2N PeerSharing (Protocol ID 10)

**State machine:** `StIdle → StBusy → StIdle` (loop), `StIdle → StDone`

**Wire format:**
```
MsgShareRequest = [0, amount: u8]
MsgSharePeers   = [1, [*peer_address]]
MsgDone          = [2]

peer_address = [0, ipv4: u32, port: u16]
             / [1, w0: u32, w1: u32, w2: u32, w3: u32, port: u16]
```

Max request 255 peers. Server must not return more than requested. Only active when both sides negotiated `peer_sharing: Enabled` and connection is `InitiatorAndResponder`.

Address filtering: reject loopback, RFC1918, link-local, broadcast, unspecified, multicast, documentation ranges, CGNAT 100.64.0.0/10, IPv6 ULA fc00::/7.

### N2C LocalChainSync (Protocol ID 5)

Identical state machine to N2N ChainSync. Differences:
- Sends **full blocks** (not headers) in MsgRollForward
- Wrapped as `[era_id, CBOR_tag_24(block_bytes)]`
- No pipelining expected from N2C clients
- Ingress queue effectively unlimited

### N2C LocalTxSubmission (Protocol ID 6)

**State machine:** `StIdle → StBusy → StIdle`, `StIdle → StDone`

**Wire format:**
```
MsgSubmitTx = [0, era_id, tx_bytes]
MsgAcceptTx = [1]
MsgRejectTx = [2, [era_id, reject_reason]]
MsgDone     = [3]
```

Push-based. Validate via TxValidator, accept into mempool or reject with structured error matching Haskell's `ApplyTxErr` encoding.

### N2C LocalStateQuery (Protocol ID 7)

**State machine:**

| State | Agency | Allowed Messages |
|---|---|---|
| StIdle | Client | MsgAcquire [0]/[8]/[10], MsgDone [7] |
| StAcquiring | Server | MsgAcquired [1], MsgFailure [2] |
| StAcquired | Client | MsgQuery [3], MsgRelease [5], MsgReAcquire [6]/[9]/[11] |
| StQuerying | Server | MsgResult [4] |
| StDone | Nobody | — |

**Acquire targets:**
- `[0, point]` — SpecificPoint: validate exists between immutable and volatile tip, fail with `PointTooOld` (0) or `PointNotOnChain` (1) if not
- `[8]` — VolatileTip: always succeeds
- `[10]` — ImmutableTip: always succeeds (V16+)

**MsgReAcquire** releases current and acquires new atomically. If new acquisition fails, old state is also lost.

All 39 Shelley BlockQuery tags (0-38) supported. Results wrapped in HFC `QueryIfCurrent` success envelope `[1, result]`. QueryAnytime and QueryHardFork results unwrapped.

**Lock strategy:** Snapshot needed data under the lock and release immediately. Expensive queries operate on the snapshot, not under the lock.

### N2C LocalTxMonitor (Protocol ID 9)

**State machine:**

| State | Agency | Allowed Messages |
|---|---|---|
| StIdle | Client | MsgAcquire [1], MsgDone [0] |
| StAcquiring | Server | MsgAcquired [2] |
| StAcquired | Client | MsgNextTx [5], MsgHasTx [7], MsgGetSizes [9], MsgGetMeasures [11], MsgAwaitAcquire [1], MsgRelease [3] |
| StBusy(kind) | Server | MsgReplyNextTx [6], MsgReplyHasTx [8], MsgReplyGetSizes [10], MsgReplyGetMeasures [12] |
| StDone | Nobody | — |

**Snapshot semantics:**
- MsgAcquire captures mempool snapshot; all queries operate on it
- MsgAwaitAcquire (tag 1 from StAcquired) blocks until a new snapshot is available
- MsgGetMeasures only available when negotiated version >= N2C V20 (wire 32788)
- Track which txs have been yielded via MsgNextTx per snapshot

## Layer 4: Connection Manager

### Connection States

```
ReservedOutbound → UnnegotiatedConn(Outbound) → OutboundIdle(Uni|Duplex) ─┐
                                                                           ├→ DuplexConn
UnnegotiatedConn(Inbound) → InboundIdle(Uni|Duplex) ─────────────────────┘

Any state → Closed (on error/shutdown)
```

### Core Responsibilities

**Outbound:** Governor requests connection → reserve slot → TCP connect (10s timeout) → start mux → handshake (30s timeout) → validate magic → OutboundIdle → notify governor for promotion.

**Inbound:** TCP listener accepts → rate-limit check (per-IP token bucket) → global limit check → start mux → receive handshake → validate magic → InboundIdle → notify governor.

**Simultaneous open:** Detected when outbound handshake receives MsgProposeVersions (tag 0) instead of MsgAcceptVersion (tag 1). Resolution: connection where lower address `(IP, port)` is the initiator survives; other is closed. Surviving connection becomes DuplexConn.

### Connection Limits

| Limit | Default | Purpose |
|---|---|---|
| `max_inbound` | 100 | Total inbound connections |
| `max_outbound` | 20 | Total outbound connections |
| `per_ip_rate` | 5/min | Rate limit per source IP |
| `handshake_timeout` | 30s | Time to complete handshake |
| `connect_timeout` | 10s | TCP connect timeout |

### Protocol Orchestration

Connection handler starts/stops protocol tasks based on peer temperature:

| Temperature | Protocols |
|---|---|
| Cold | None |
| Warm | KeepAlive (client + server) |
| Hot | ChainSync + BlockFetch + TxSubmission2 + KeepAlive + PeerSharing (if negotiated) |

**Promotion warm → hot:** Start protocol client/server tasks with mux channel handles.
**Demotion hot → warm:** Send MsgDone on ChainSync, BlockFetch, TxSubmission2. KeepAlive continues.
**Demotion warm → cold:** Send KeepAlive MsgDone. Shut down mux. Close bearer.

### Shutdown Coordination

Each protocol task holds a `CancellationToken`. On demotion or error:
1. Cancel all protocol tasks
2. Tasks detect cancellation, send MsgDone if they have agency
3. Wait up to 5s for graceful shutdown
4. Force-close mux and bearer if tasks haven't exited

## Peer Manager & Governor

### Peer Manager

Pure state container. Per-peer state:

```rust
pub struct PeerInfo {
    address: SocketAddr,
    source: PeerSource,              // DNS, Topology, LedgerState, PeerSharing
    state: PeerTemperature,          // Cold, Warm, Hot
    negotiated_version: Option<u16>,
    diffusion_mode: Option<DiffusionMode>,
    peer_sharing: Option<PeerSharingMode>,
    latency: EwmaLatency,
    bytes_received: u64,
    blocks_served: u64,
    reputation: f64,                 // 0.0 - 1.0
    failure_count: u32,
    last_failure: Option<Instant>,
    failure_decay_timer: Instant,
    last_connected: Option<Instant>,
    last_activity: Option<Instant>,
    promoted_at: Option<Instant>,
}
```

**Failure count decay:** Background timer halves `failure_count` every 5 minutes for all peers.

**Address filtering:** Reject loopback, RFC1918, link-local, broadcast, unspecified, multicast, documentation ranges, CGNAT 100.64.0.0/10, IPv6 ULA fc00::/7.

### Governor

Periodic decision loop (1-2 second tick). Targets matching Haskell's `PeerSelectionTargets`:

```rust
pub struct PeerTargets {
    pub root_peers: usize,
    pub known_peers: usize,                    // default 100
    pub established_peers: usize,              // default 10
    pub active_peers: usize,                   // default 5
    pub known_big_ledger_peers: usize,         // default 100
    pub established_big_ledger_peers: usize,   // default 5
    pub active_big_ledger_peers: usize,        // default 3
}
```

**Decision loop per tick:**
1. Count peers by temperature
2. known < target → trigger discovery (DNS, ledger, peer sharing)
3. established < target → promote cold → warm
4. active < target → promote warm → hot
5. active > target → demote hot → warm
6. established > target → demote warm → cold
7. Churn: every 10-20 minutes, rotate a random hot peer to warm and promote a different warm peer

### Peer Discovery

1. **Topology file** — static peers, always re-promoted (root peers)
2. **DNS** — SRV and A/AAAA records from topology
3. **Ledger-based** — SPO relay addresses from `pool_params`, after `useLedgerAfterSlot`
4. **Peer sharing** — request from hot connections with peer sharing enabled

### Block Fetch Decision Logic

- Download queue of block ranges from ChainSync headers
- Select lowest-latency peer that has the block
- Distribute ranges across peers for parallel fetch
- Retry failed ranges on alternative peers
- Respect `blockFetchPipeliningMax` (default 100)

## Error Handling

### Error Hierarchy

```rust
pub enum NetworkError {
    Bearer(BearerError),          // I/O errors, timeouts
    Mux(MuxError),                // Header errors, queue overruns
    Handshake(HandshakeError),    // Magic mismatch, version mismatch, decode errors
    Protocol(ProtocolError),      // Agency violations, invalid messages, state violations
    Connection(ConnectionError),  // Limits, rate limiting, simultaneous open conflicts
}
```

### Error Severity

- `IngressQueueOverrun` → tear down connection (protocol violation)
- `AgencyViolation` → tear down connection (buggy/malicious peer)
- `NetworkMagicMismatch` → refuse and close
- `ConnectionReset` → clean up, notify governor
- `Timeout` → clean up, increment failure count

### Logging Levels

| Event | Level |
|---|---|
| Connection established/closed | `info!` |
| Handshake completed | `info!` |
| Reached tip / left tip | `info!` |
| Peer promoted/demoted | `info!` |
| Per-block MsgRollForward/MsgRequestNext | `debug!` |
| MsgRequestTxIds/MsgReplyTxIds | `debug!` |
| SDU encode/decode details | `trace!` |
| CBOR hex dumps | `trace!` |

## Metrics

Prometheus on port 12798.

**Connection:** `torsten_peers_{cold,warm,hot}`, `torsten_connections_{inbound,outbound}`, `torsten_handshakes_{completed,failed}_total`, `torsten_simultaneous_opens_total`

**Protocol:** `torsten_chainsync_headers_received_total`, `torsten_chainsync_pipeline_depth`, `torsten_blockfetch_blocks_received_total`, `torsten_blockfetch_bytes_received_total`, `torsten_txsubmission_txs_{announced,received}_total`, `torsten_peersharing_peers_received_total`

**Mux:** `torsten_mux_egress_batches_total`, `torsten_mux_ingress_queue_bytes` (per protocol)

**Latency:** `torsten_keepalive_rtt_seconds` (histogram)

## Testing Strategy

### Unit Tests

- **Mux:** SDU encode/decode roundtrip, direction bit flipping, segmentation, reassembly, fairness, batching, ingress limits
- **Protocol state machines:** every valid transition, agency checks, invalid message rejection, CBOR roundtrip for every message type
- **Handshake:** version negotiation, magic mismatch, query mode, simultaneous open, N2C bit-15 encoding
- **Peer manager:** temperature transitions, failure decay, reputation, address filtering, eviction

### Protocol Conformance Tests

Wire trace replay against captured cardano-node sessions.

**Capture methodology:** Run cardano-node on preview with tcpdump, extract per-protocol sequences with dissector script, store as test fixtures in `tests/conformance/traces/`.

**Fixtures:**
```
tests/conformance/traces/
├── n2n_handshake_v14.cbor
├── n2n_handshake_v15.cbor
├── n2n_chainsync_initial.cbor
├── n2n_chainsync_rollback.cbor
├── n2n_chainsync_await.cbor
├── n2n_blockfetch_range.cbor
├── n2n_txsubmission2_init.cbor
├── n2n_keepalive.cbor
├── n2n_peersharing.cbor
├── n2c_handshake_v22.cbor
├── n2c_statequery_tip.cbor
├── n2c_statequery_pparams.cbor
├── n2c_txsubmit_accept.cbor
├── n2c_txsubmit_reject.cbor
├── n2c_txmonitor_snapshot.cbor
└── n2c_chainsync_block.cbor
```

**Test structure:** For outbound messages, encode and compare byte-for-byte. For inbound messages, verify correct decoding.

### Integration Tests

- In-process: full mux with mock bearer, handshake + ChainSync against canned server
- Connection manager: simulate inbound + outbound, verify simultaneous open resolution
- Live testnet: connect to preview peers, sync 100 blocks, serve LocalStateQuery, submit test transaction

### Coverage Targets

| Component | Target |
|---|---|
| Mux segment encoding | 100% |
| Protocol CBOR codecs | 100% |
| State machine transitions | 100% |
| Handshake negotiation | All version combinations |
| Connection manager | All state transitions |
| Peer manager | Core operations |

## Migration

### Dependency Changes

**Remove from Cargo.toml:**
```toml
pallas-network = { workspace = true }
pallas-traverse = { workspace = true }
pallas-crypto = { workspace = true }
```

**Add:**
```toml
minicbor = { version = "0.25", features = ["std"] }
tokio-util = { version = "0.7", features = ["codec"] }
bytes = "1"
```

**Keep:**
```toml
torsten-primitives = { workspace = true }
torsten-serialization = { workspace = true }
torsten-crypto = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }
```

### torsten-node Integration

The public trait interface (`BlockProvider`, `TxValidator`, `MempoolProvider`, `UtxoQueryProvider`, `ConnectionMetrics`) is preserved. Changes in `torsten-node`:

- `node/mod.rs` — update construction to use `ConnectionManager` API
- `node/serve.rs` — trait implementations unchanged; passed via `ConnectionManager::new(config, providers)`
- `node/sync.rs` — pipelined client API changes from `PipelinedPeerClient` to `ConnectionManager::connect_outbound()` + ChainSync client

### Cutover Sequence

1. Delete all existing `src/` contents
2. Write new implementation
3. Update `Cargo.toml`
4. Update `torsten-node` integration
5. Remove `pallas-network` from workspace `Cargo.toml`
6. Build, test, verify on preview testnet

Work happens on a feature branch. Node is non-functional between steps 1 and 4.
