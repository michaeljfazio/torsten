---
name: crate-pallas-network
description: pallas-network mini-protocols, multiplexer, facades, version tables, and where dugite diverges
type: reference
---

# pallas-network (v1.0.0-alpha.5)

## Overview

Description: "Ouroboros networking stack using async IO". Apache-2.0. Provides the full Ouroboros network stack using tokio async I/O.

## Module Structure

```
pallas_network::
  facades::         // High-level PeerClient, NodeClient, PeerServer, NodeServer
  miniprotocols::   // All Ouroboros mini-protocol implementations
    blockfetch::    // Block fetch protocol
    chainsync::     // Chain synchronization protocol
    handshake::     // Version negotiation
      n2n::         // N2N version data
      n2c::         // N2C version data
    keepalive::     // Connection keepalive
    localstate::    // Local state query (N2C)
      queries_v16:: // BlockQuery types
    localmsgnotification::  // Local message notification
    localmsgsubmission::    // Local message submission
    localtxsubmission::     // Local tx submission (N2C)
    peersharing::   // Peer exchange
    txmonitor::     // Transaction monitoring (N2C)
    txsubmission::  // Transaction submission (N2N)
  multiplexer::     // Bearer, Plexer, ChannelBuffer, AgentChannel
```

## Multiplexer (`pallas_network::multiplexer`)

```rust
pub struct Bearer { ... }       // TCP/Unix/Windows pipe transport; supports split into read/write halves
pub struct Plexer { ... }       // Demuxer + Muxer; manages protocol subscriptions
pub struct AgentChannel { ... } // Bidirectional channel for one protocol
pub struct ChannelBuffer { ... } // Handles fragmented messages; higher-level than AgentChannel
pub struct RunningPlexer { ... } // Spawned plexer tasks; can abort()
```

XOR 0x8000 distinguishes client/server direction on same protocol number.

## Facades (`pallas_network::facades`)

```rust
pub struct PeerClient { ... }   // N2N client (TCP)
pub struct NodeClient { ... }   // N2C client (Unix socket / Windows pipe)
pub struct DmqClient { ... }    // N2C DMQ client (CIP-0137)
pub struct PeerServer { ... }   // N2N server
pub struct NodeServer { ... }   // N2C server (Unix only)
pub struct DmqServer { ... }    // N2C DMQ server (Unix only)

pub enum KeepAliveLoop { Client, Server }
pub struct KeepAliveHandle { ... }  // tokio task handle

pub enum Error {
    PlexerFailure, ConnectFailure, HandshakeProtocol,
    KeepAliveClientLoop, KeepAliveServerLoop, IncompatibleVersion,
}
```

## Protocol Constants

```rust
PROTOCOL_N2N_HANDSHAKE       = 0
PROTOCOL_N2N_CHAIN_SYNC      = 2
PROTOCOL_N2N_BLOCK_FETCH     = 3
PROTOCOL_N2N_TX_SUBMISSION   = 4
PROTOCOL_N2N_KEEP_ALIVE      = 8
PROTOCOL_N2N_PEER_SHARING    = 10
PROTOCOL_N2C_HANDSHAKE       = 0 (same ID, different bearer)
PROTOCOL_N2C_CHAIN_SYNC      = 5
PROTOCOL_N2C_LOCAL_TX_SUBMIT = 6
PROTOCOL_N2C_LOCAL_STATE     = 7
PROTOCOL_N2C_TX_MONITOR      = 9
```

## N2N Handshake

```rust
// n2n::VersionData (NOT called N2NVersionData)
pub struct VersionData {
    pub network_magic: u64,
    pub initiator_only_diffusion_mode: bool,
    pub peer_sharing: Option<u8>,   // V11+
    pub query: Option<bool>,        // V11+
}

// Supported N2N versions in pallas: V7 through V14
// Factory methods:
// v7_and_above() — includes V7-V14
// v11_and_above() — includes V11-V14
// v7_to_v10() — V7-V10 only
```

**Gap**: Dugite uses N2N V14/V15. Pallas N2N only goes to V14. If dugite negotiates V15, it's doing so outside of pallas handshake helpers.

## N2C Handshake

```rust
// n2c::VersionData (NOT called N2CVersionData)
pub struct VersionData(NetworkMagic, Option<bool>);
// V1-V14: basic network magic only
// V15-V16: + optional boolean parameter
// DMQ V1: protocol variant 4097
```

**Gap**: Dugite supports N2C V16-V22 with bit-15 version encoding. Pallas N2C only defines up to V16. Dugite extends this significantly beyond what pallas provides.

## ChainSync Mini-Protocol

```rust
pub struct Client<O>(State, ChannelBuffer, PhantomData<O>);
pub type N2NClient = Client<HeaderContent>;  // for N2N
pub type N2CClient = Client<BlockContent>;   // for N2C

// Client methods:
pub fn new(channel: AgentChannel) -> Self
pub fn find_intersect(points: Vec<Point>) -> Result<(Option<Point>, Tip)>
pub fn send_find_intersect(points: Vec<Point>) -> Result<()>
pub fn recv_intersect_response() -> Result<(Option<Point>, Tip)>
pub fn send_request_next() -> Result<()>
pub fn recv_while_can_await() -> Result<NextResponse<O>>
pub fn recv_while_must_reply() -> Result<NextResponse<O>>
pub fn request_next() -> Result<NextResponse<O>>  // combined send+recv
pub fn request_or_await_next() -> Result<NextResponse<O>>
pub fn intersect_origin() -> Result<Tip>
pub fn intersect_tip() -> Result<(Point, Tip)>

pub enum NextResponse<CONTENT> {
    RollForward(CONTENT, Tip),
    RollBackward(Point, Tip),
    Await,
}
```

**CRITICAL GAP — NO PIPELINING**: pallas-network ChainSync Client enforces strict request-response ordering. No support for sending multiple MsgRequestNext before reading responses. Dugite bypasses this entirely with its `PipelinedPeerClient` which directly manipulates `ChannelBuffer` to send N requests before reading N responses.

## BlockFetch Mini-Protocol

```rust
pub struct Client { ... }
// Methods:
pub fn fetch_single(point: Point) -> Result<Body>   // Body = Vec<u8>
pub fn fetch_range(range: Range) -> Result<Vec<Body>>  // Range = (Point, Point)
pub fn request_range(range: Range) -> Result<HasBlocks>
pub fn recv_while_streaming() -> Result<Option<Body>>
pub fn send_done() -> Result<()>
```

**No pipelining here either** — strict sequential. Dugite uses `blockfetch::Client` directly (not bypassed), so this is fine for batch block fetching.

## LocalState Query (N2C)

```rust
// In queries_v16:
pub enum Request {
    LedgerQuery(LedgerQuery),
    GetSystemStart,
    GetChainBlockNo,
    GetChainPoint,
}

pub enum LedgerQuery {
    BlockQuery(Era, BlockQuery),
    HardForkQuery(HardForkQuery),
}

// BlockQuery has 32+ variants:
// GetLedgerTip, GetEpochNo, GetCurrentPParams, GetUTxOByAddress,
// GetUTxOWhole, GetUTxOByTxIn, GetStakePools, GetStakePoolParams,
// GetPoolState, GetPoolDistr, GetConstitution, GetGovState,
// GetDRepState, GetProposals, GetRatifyState, GetFuturePParams, ...
```

**Gap**: Pallas localstate queries_v16 only goes to v16. Dugite implements tags 0-38 (V17-V22 additions). Pallas would need extension for V17+ queries like GetStakeDistribution2 (tag 37), GetPoolDistr2 (tag 36), GetMaxMajorProtocolVersion (tag 38).

## TxSubmission Mini-Protocol (N2N)

```rust
pub type Client = GenericClient<EraTxId, EraTxBody>;
// Methods:
pub fn send_init() -> Result<()>
pub fn reply_tx_ids(ids: Vec<(TxId, u32)>) -> Result<()>
pub fn reply_txs(txs: Vec<TxBody>) -> Result<()>
pub fn next_request() -> Result<Request<TxId>>
pub fn send_done() -> Result<()>
```

## PeerSharing Mini-Protocol

Has client, protocol, server modules (re-exported). Provides peer exchange functionality.

## Dugite's Divergences from pallas-network

1. **Pipelined ChainSync**: Dugite's `PipelinedPeerClient` directly manipulates `ChannelBuffer` + `Plexer` raw channels to pipeline N requests without the pallas state machine. This is the most significant divergence.

2. **N2C server beyond V16**: Dugite implements LocalStateQuery tags 0-38 (V17-V22); pallas only defines queries up to ~V16.

3. **N2N server**: Pallas provides N2N server facades (PeerServer) but dugite implements its own complete N2N server for serving blocks to downstream peers.

4. **TxSubmission server**: Dugite has custom N2N TxSubmission server handling; pallas provides client-side only.

5. **N2C DMQ**: Pallas has DmqClient/DmqServer but dugite doesn't use CIP-0137 DMQ.

6. **N2N V15**: Pallas N2N goes to V14; dugite supports V14/V15.

## What Dugite DOES Use From pallas-network

- `PeerClient::connect()` facade for initial upstream connections
- `ChannelBuffer` directly for pipelined chainsync
- `blockfetch::Client` for fetching block bodies
- `handshake` module for N2N version negotiation (client side)
- `keepalive` module for connection maintenance
- Protocol ID constants (PROTOCOL_N2N_*)
- `Bearer`, `Plexer`, `RunningPlexer` for raw multiplexer access
- `chainsync::Message` types for direct CBOR encoding in pipeline
