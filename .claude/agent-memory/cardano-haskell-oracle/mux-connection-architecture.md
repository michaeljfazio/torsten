---
name: Mux Connection Architecture
description: Complete N2N connection model - single TCP per peer, mux threading, protocol temperatures, startup sequence, connection manager state machine, timeouts
type: reference
---

## Connection Model: ONE TCP per Peer
- Single TCP connection carries ALL mini-protocols (ChainSync, BlockFetch, TxSubmission2, KeepAlive, PeerSharing)
- Multiplexed via network-mux SDU framing (8-byte header: timestamp u32, protocol_num u16, direction u8, length u16)
- SDU payload max = 0xFFFF (65535 bytes); large messages split across multiple SDUs
- Handshake runs on protocol 0 BEFORE mux starts, using same socket bearer
- `InitiatorResponderMode` = both directions on same connection (N2N P2P)

## Mux Threading Model
- `Mux.run` spawns 3 core threads via JobPool:
  1. **muxer** (egress): single thread reads from EgressQueue (TBQueue 100), chops data into SDU-sized chunks, batches up to 100 SDUs or SO_SNDBUF bytes, writes to bearer. Has egressInterval pacing.
  2. **demuxer** (ingress): single thread reads SDUs from bearer, dispatches to per-protocol IngressQueue based on (protocol_num, direction). Direction is FLIPPED on receive.
  3. **monitor**: manages protocol thread lifecycle, starts on-demand protocols when data arrives
- Each mini-protocol instance runs in its OWN Haskell thread (forked via JobPool)
- Per-protocol ingress queues (STM TVar with byte count + Builder) provide isolation
- Wanton (egress buffer per protocol) provides non-blocking fair scheduling: each protocol can have at most one entry in EgressQueue at a time, ensuring no starvation
- Soft egress buffer limit: 0x3FFFF (262143 bytes) per protocol before send blocks

## Temperature-Based Protocol Lifecycle
- Protocols grouped into three temperatures:
  - **Hot**: ChainSync (2), BlockFetch (3), TxSubmission2 (4), [Peras Cert (16), Peras Vote (17)]
  - **Warm**: (empty currently, reserved for tip-sample)
  - **Established**: KeepAlive (8), PeerSharing (10)
- Hot protocols use `StartOnDemand` (responder started when first data arrives)
- KeepAlive uses `StartOnDemandAny` (started when ANY on-demand protocol receives data)
- PeerSharing uses `StartOnDemand`
- Initiator side always started with `StartEagerly` via `startProtocols`

## Connection Lifecycle (PeerStateActions)
1. **Cold -> Warm** (`establishPeerConnection`):
   - `acquireOutboundConnection` -> TCP connect -> handshake
   - After handshake: `Mux.new` creates mux, `Mux.run` starts muxer+demuxer
   - Hot ControlMessage set to `Terminate`, Warm+Established set to `Continue`
   - `startProtocols SingWarm` and `startProtocols SingEstablished` called
   - Peer monitoring loop forked
2. **Warm -> Hot** (`activatePeerConnection`):
   - Hot ControlMessage set to `Continue`, Warm set to `Quiesce`
   - `startProtocols SingHot` starts ChainSync, BlockFetch, TxSubmission2 initiators eagerly
3. **Hot -> Warm** (`deactivatePeerConnection`):
   - Hot ControlMessage set to `Terminate`, Warm set to `Continue`
   - Wait up to `spsDeactivateTimeout` for hot protocols to finish
   - If timeout: Mux.stop -> connection closed
4. **Warm -> Cold** (`closePeerConnection`):
   - All ControlMessages set to `Terminate`

## Connection Manager State Machine (AbstractState)
- UnknownConnectionSt -> ReservedOutboundSt -> UnnegotiatedSt -> OutboundUniSt/OutboundDupSt -> DuplexSt
- InboundIdleSt -> InboundSt -> DuplexSt
- DuplexSt = fully bidirectional (both initiator and responder active)
- TerminatingSt -> TerminatedSt
- **Simultaneous open**: both inbound+outbound to same peer -> DuplexState
  - Decided by bit-15 convention on DataFlow negotiation
  - Lower address keeps outbound, higher keeps inbound (or vice versa based on negotiation)

## Protocol Timeouts (from source)
- **Handshake SDU**: 10s per SDU read/write
- **Normal SDU**: 30s per SDU read (minimum 17kbps speed)
- **ChainSync StIdle**: 3373s (defaultChainSyncIdleTimeout) - NOT a disconnect, just the IG idle timeout
- **ChainSync StMustReply**: untrusted=uniform(135s-269s) per state, trusted=waitForever
- **ChainSync StCanAwait**: 10s (shortWait)
- **ChainSync StIntersect**: 10s (shortWait)
- **TxSubmission2 StInit**: waitForever
- **TxSubmission2 StIdle**: waitForever
- **TxSubmission2 StTxIds Blocking**: waitForever
- **TxSubmission2 StTxIds NonBlocking**: 10s (shortWait)
- **TxSubmission2 StTxs**: 10s (shortWait)
- **KeepAlive StClient**: 97s
- **KeepAlive StServer**: 60s
- **Protocol idle timeout (OutboundDupSt)**: 5s (defaultProtocolIdleTimeout)

## BlockFetch Decision Logic
- `blockFetchLogic` runs in its own thread, iterating every 10ms (Praos) or 40ms (Genesis)
- Reads candidate chains (from ChainSync), current chain, downloaded blocks, peer GSV metrics
- Makes fetch decisions per peer: which block ranges to fetch from which peer
- `maxInFlightReqsPerPeer` = blockFetchPipeliningMax = 100
- `maxConcurrencyBulkSync` = 1 (one peer at a time during bulk sync)
- `maxConcurrencyDeadline` = 1
- ChainSync and BlockFetch run CONCURRENTLY on same connection
- ChainSync feeds candidate chain headers -> BlockFetch reads them via STM -> BlockFetch decides what to fetch
- No blocking between them; coordination is entirely through shared STM state

## Mini-Protocol Numbers
- Handshake: 0 (not in mux bundle)
- DeltaQ: 1 (reserved)
- ChainSync: 2
- BlockFetch: 3
- TxSubmission2: 4
- KeepAlive: 8
- PeerSharing: 10
- Peras Cert Diffusion: 16
- Peras Vote Diffusion: 17

## Ingress Queue Limits
- ChainSync: highMark(300) * 1400 * 1.1 = 462,000 bytes
- BlockFetch: max(10*2MB, 100*88KB) * 1.1 = 22,020,000 bytes (~21MB)
- TxSubmission2: maxUnacked * (44 + 65536) * 1.1
- KeepAlive: 1280 * 1.1 = 1408 bytes
- PeerSharing: 4 * 1440 = 5760 bytes
