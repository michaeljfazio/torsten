# Cardano Mini-Protocol Reference

This document is the definitive implementation reference for every Cardano
mini-protocol used in node-to-node (N2N) and node-to-client (N2C)
communication. It covers the complete state machine, exact CBOR wire format,
timing constraints, flow-control rules, and every protocol-error condition for
each protocol. The information is derived directly from the Haskell source in
the `IntersectMBO/ouroboros-network` repository.

---

## Connection Model and Multiplexer

All mini-protocols share a single TCP connection per peer, multiplexed by the
`network-mux` layer using 8-byte SDU headers:

```
  Bytes  Field
  -----  -----
   0-3   timestamp (u32, microseconds, used for RTT measurement)
   4-5   mini_protocol_num (u16)
     6   flags (bit 0 = direction: 0=initiator, 1=responder)
   7-8   payload_length (u16, max 65535)
```

Large messages are fragmented across multiple SDUs transparently. Handshake
(protocol 0) runs on the raw socket before the mux is started.

**Key invariant**: if any single mini-protocol thread throws an exception, the
entire mux — and therefore the entire TCP connection — is torn down. Protocol
errors are fatal to the connection, not just to the affected mini-protocol.

**Sources:**
- `ouroboros-network/network-mux/src/Network/Mux/Types.hs`
- `ouroboros-network/network-mux/src/Network/Mux/Egress.hs`

---

## Shared Encoding Primitives

These types are used identically across all protocols.

### Point
A `Point` identifies a position on the chain by slot and header hash.

```
; CBOR encoding (Haskell: encodePoint / decodePoint)
point = []                         ; Origin — empty definite-length list
      / [slot_no, header_hash]     ; At(slot, hash) — definite-length list of 2

slot_no     = uint     ; word64
header_hash = bstr     ; 32 bytes (Blake2b-256 of header)
```

Source: `ouroboros-network/ouroboros-network/api/lib/Ouroboros/Network/Block.hs`

### Tip
A `Tip` is the chain tip as seen by the server. It is a `(Point, BlockNo)` pair.

```
; N2N ChainSync / N2C LocalChainSync
tip = [slot_no, header_hash, block_no]   ; At(pt, blockno)
    / [0]                                ; TipGenesis (Origin point, blockno=0)

block_no = uint   ; word64
```

Source: `ouroboros-network/ouroboros-network/api/lib/Ouroboros/Network/Block.hs`
(`encodeTip` / `decodeTip`)

### Byte and Time Limit Constants
These constants appear in state-machine timeout and size-limit tables throughout
this document.

| Constant       | Value       | Source in Codec/Limits.hs  |
|----------------|-------------|----------------------------|
| `smallByteLimit` | 65535 bytes | `Protocol/Limits.hs:smallByteLimit` |
| `largeByteLimit` | 2 500 000 bytes | `Protocol/Limits.hs:largeByteLimit` |
| `shortWait`      | 10 seconds  | `Protocol/Limits.hs:shortWait`      |
| `longWait`       | 60 seconds  | `Protocol/Limits.hs:longWait`       |
| `waitForever`    | no timeout  | `Protocol/Limits.hs:waitForever` (= `Nothing`) |

Source: `ouroboros-network/ouroboros-network/api/lib/Ouroboros/Network/Protocol/Limits.hs`

---

## N2N Mini-Protocol IDs

| Protocol        | ID |
|-----------------|----|
| Handshake       |  0 |
| DeltaQ          |  1 (reserved, never used) |
| ChainSync       |  2 |
| BlockFetch      |  3 |
| TxSubmission2   |  4 |
| KeepAlive       |  8 |
| PeerSharing     | 10 |
| Peras Cert      | 16 (future) |
| Peras Vote      | 17 (future) |

## N2C Mini-Protocol IDs

| Protocol          | ID |
|-------------------|----|
| Handshake         |  0 |
| LocalChainSync    |  5 |
| LocalTxSubmission |  6 |
| LocalStateQuery   |  7 |
| LocalTxMonitor    |  9 |

---

## Protocol Temperatures (N2N)

Protocol temperature determines when each N2N mini-protocol is started during
the peer lifecycle (`cold → warm → hot`).

| Temperature    | Protocols                                    | Started when               |
|----------------|----------------------------------------------|----------------------------|
| Established    | KeepAlive (8), PeerSharing (10)              | On cold→warm promotion     |
| Warm           | (none currently)                             | —                          |
| Hot            | ChainSync (2), BlockFetch (3), TxSubmission2 (4) | On warm→hot promotion  |

Hot protocols use `StartOnDemand` for the responder side (they wait for the
first inbound byte). Initiator sides are started eagerly by `startProtocols`.

Source: `ouroboros-network/cardano-diffusion/lib/Cardano/Network/Diffusion/Peer/`

---

## N2N Protocol 0: Handshake

### Identity
- **Protocol ID:** 0 (runs on raw socket bearer before mux starts)
- **Direction:** Initiator sends `MsgProposeVersions`, responder replies
- **Versions:** V14 (Plomin HF, mandatory since 2025-01-29), V15 (SRV DNS)

### State Machine

```
StPropose  (ClientAgency)  -- initiator has agency
    │
    │ MsgProposeVersions
    ▼
StConfirm  (ServerAgency)  -- server chooses version
    │
    ├─── MsgAcceptVersion ──→ StDone
    ├─── MsgRefuse        ──→ StDone
    └─── MsgQueryReply    ──→ StDone
```

| State       | Agency    | Meaning |
|-------------|-----------|---------|
| `StPropose` | Client    | Initiator must send its version list |
| `StConfirm` | Server    | Server must accept, refuse, or query |
| `StDone`    | Nobody    | Terminal |

**Terminal state:** `StDone` — connection is closed after handshake completes (for N2N; the mux then starts).

### Wire Format

Source: `ouroboros-network/ouroboros-network/framework/lib/Ouroboros/Network/Protocol/Handshake/Codec.hs`
and `cardano-diffusion/protocols/cddl/specs/handshake-node-to-node-v14.cddl`

```
; Every handshake message is a definite-length CBOR array.
MsgProposeVersions = [0, versionTable]
MsgAcceptVersion   = [1, versionNumber, versionData]
MsgRefuse          = [2, refuseReason]
MsgQueryReply      = [3, versionTable]

; versionTable is a CBOR definite-length MAP (not an array).
; Keys are encoded in ascending order.
versionTable = { * versionNumber => versionData }

; N2N version numbers (V14=14, V15=15, V16=16, ...)
; Note: N2N does NOT set bit-15. Only N2C uses bit-15.
versionNumber = 14 / 15 / 16

; Version data for V14/V15: 4-element array
versionData_v14 = [networkMagic, initiatorOnly, peerSharing, query]
; Version data for V16+: 5-element array (adds perasSupport)
versionData_v16 = [networkMagic, initiatorOnly, peerSharing, query, perasSupport]

networkMagic = uint .size 4   ; word32 (mainnet=764824073, preview=2, preprod=1)
initiatorOnly = bool           ; true=InitiatorOnly, false=InitiatorAndResponder
peerSharing   = 0 / 1          ; 0=Disabled, 1=Enabled
query         = bool
perasSupport  = bool

refuseReason
  = [0, [* versionNumber]]           ; VersionMismatch
  / [1, versionNumber, tstr]         ; HandshakeDecodeError
  / [2, versionNumber, tstr]         ; Refused
```

### Version Negotiation Rules

Source: `cardano-diffusion/api/lib/Cardano/Network/NodeToNode/Version.hs`

- The responder picks the **highest** version number that appears in both
  the initiator's and responder's version tables.
- If no common version: `MsgRefuse` with `VersionMismatch`.
- `networkMagic` must match exactly; mismatch → `MsgRefuse` with `Refused`.
- `initiatorOnlyDiffusionMode` = `min(local, remote)` — more restrictive wins
  (i.e., `InitiatorOnly` if either side is).
- `peerSharing` = `local <> remote` (Semigroup): **both** must be Enabled for
  Enabled; any Disabled results in Disabled. `InitiatorOnly` nodes
  automatically have Disabled.
- `query` = `local || remote` (logical OR).

### MsgQueryReply Semantics

When the initiator sends `MsgProposeVersions` with `query=true`, the responder
**must** reply with `MsgQueryReply` (a copy of its own version table) and then
close the connection. This is used by cardano-cli for version probing. The mux
never starts in this case.

### Timeout

Handshake SDU read/write: 10 seconds per SDU. There is no per-state
timeout beyond this; the handshake exchange must complete within one
SDU read cycle on each side.

---

## N2N Protocol 2: ChainSync

### Identity
- **Protocol ID:** 2
- **Temperature:** Hot (started on warm→hot promotion)
- **Direction:** N2N ChainSync streams **block headers** only (not full blocks).
  Full blocks are fetched via BlockFetch.
- **Versions:** All N2N versions (V7+)

### State Machine

Source: `ouroboros-network/ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/ChainSync/Type.hs`

```
StIdle       (ClientAgency)  -- client requests next update or intersect
    │
    ├─── MsgRequestNext      ──→ StNext(StCanAwait)
    ├─── MsgFindIntersect    ──→ StIntersect
    └─── MsgDone             ──→ StDone

StNext(StCanAwait)  (ServerAgency)  -- server can immediately reply or defer
    │
    ├─── MsgAwaitReply       ──→ StNext(StMustReply)
    ├─── MsgRollForward      ──→ StIdle
    └─── MsgRollBackward     ──→ StIdle

StNext(StMustReply)  (ServerAgency)  -- server MUST reply (already sent await)
    │
    ├─── MsgRollForward      ──→ StIdle
    └─── MsgRollBackward     ──→ StIdle

StIntersect  (ServerAgency)  -- server searching for intersection
    │
    ├─── MsgIntersectFound   ──→ StIdle
    └─── MsgIntersectNotFound ─→ StIdle

StDone (NobodyAgency)
```

**Critical invariant:** `MsgAwaitReply` is only valid in state `StNext(StCanAwait)`.
The server transitions to `StNext(StMustReply)` after sending it. Sending
`MsgAwaitReply` when the client sent a non-blocking variant (`Pipeline` rather
than `Request`) or when the server has already sent `MsgAwaitReply` this round
is a **protocol error** (`ProtocolErrorRequestNonBlocking`). The typed-protocol
framework enforces this at compile time; a Rust implementation must enforce it
at runtime by tracking which sub-state of `StNext` is current.

### Wire Format

Source: `ouroboros-network/ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/ChainSync/Codec.hs`
and `cardano-diffusion/protocols/cddl/specs/chain-sync.cddl`

```
MsgRequestNext        = [0]
MsgAwaitReply         = [1]
MsgRollForward        = [2, header, tip]
MsgRollBackward       = [3, point, tip]
MsgFindIntersect      = [4, points]
MsgIntersectFound     = [5, point, tip]
MsgIntersectNotFound  = [6, tip]
MsgDone               = [7]

; points is a DEFINITE-length array (not indefinite)
points = [* point]
```

**N2N header encoding** in `MsgRollForward`: For the `CardanoBlock` HFC block
type, the header is wrapped as:

```
header = [era_index, serialised_header_bytes]
```

where `era_index` is 0=Byron, 1=Shelley, ..., 6=Conway, 7=Dijkstra (see
TxSubmission2 section for full table), and `serialised_header_bytes` is
`tag(24)(bstr(cbor_encoded_header))` — CBOR-in-CBOR wrapping via
`wrapCBORinCBOR`.

Source: `ouroboros-consensus/ouroboros-consensus-cardano/src/shelley/Ouroboros/Consensus/Shelley/Node/Serialisation.hs`

### Pipelining

Source: `ouroboros-network/ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/ChainSync/PipelineDecision.hs`

ChainSync uses the `pipelineDecisionLowHighMark` strategy with default marks
`lowMark=200, highMark=300` (Dugite uses configurable depth via
`DUGITE_PIPELINE_DEPTH`, default 300).

```
pipelineDecisionLowHighMark :: Word16 -> Word16 -> MkPipelineDecision
```

Decision logic (given `n` outstanding requests, `clientTip`, `serverTip`):
- `n=0, clientTip == serverTip` → `Request` (non-pipelined, triggers await semantics)
- `n=0, clientTip < serverTip` → `Pipeline`
- `n>0, clientTip + n >= serverTip` → `Collect` (we're caught up, stop pipelining)
- `n >= highMark` → `Collect` (high-water: drain before adding more)
- `n < lowMark` → `CollectOrPipeline` (can collect or pipeline)
- `n >= lowMark` → `Collect` (above low mark in high state)

**When `n=0` and `clientTip == serverTip`:** the client sends a non-pipelined
`Request`, the server is at its tip and sends `MsgAwaitReply` (valid because
the client sent a blocking request). This is the "at tip" steady state.

### Timing

Source: `ouroboros-network/cardano-diffusion/protocols/lib/Cardano/Network/Protocol/ChainSync/Codec/TimeLimits.hs`

| State                  | Trusted peer  | Untrusted peer        |
|------------------------|---------------|-----------------------|
| `StIdle`               | 3373 s        | 3373 s (configurable via `ChainSyncIdleTimeout`) |
| `StNext(StCanAwait)`   | 10 s (shortWait) | 10 s                 |
| `StNext(StMustReply)`  | waitForever   | uniform random 601–911 s |
| `StIntersect`          | 10 s          | 10 s                  |

The random range for untrusted `StMustReply` corresponds to streak-of-empty-slots
probabilities between 99.9% and 99.9999% at `f=0.05`.

Default `ChainSyncIdleTimeout` = 3373 seconds.
Source: `cardano-diffusion/lib/Cardano/Network/Diffusion/Configuration.hs:defaultChainSyncIdleTimeout`

### Ingress Queue Limit

`highMark × 1400 bytes × 1.1 safety factor`

With `highMark=300`: approximately 462 000 bytes.

---

## N2N Protocol 3: BlockFetch

### Identity
- **Protocol ID:** 3
- **Temperature:** Hot
- **Purpose:** Bulk download of full block bodies, driven by the BlockFetch
  decision logic after ChainSync supplies candidate chain headers.

### State Machine

Source: `ouroboros-network/ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/BlockFetch/Type.hs`

```
BFIdle      (ClientAgency)  -- client decides what to fetch
    │
    ├─── MsgRequestRange  ──→ BFBusy
    └─── MsgClientDone    ──→ BFDone

BFBusy      (ServerAgency)  -- server preparing batch
    │
    ├─── MsgStartBatch    ──→ BFStreaming
    └─── MsgNoBlocks      ──→ BFIdle

BFStreaming  (ServerAgency)  -- server streaming blocks
    │
    ├─── MsgBlock         ──→ BFStreaming  (self-loop, one block per message)
    └─── MsgBatchDone     ──→ BFIdle

BFDone (NobodyAgency)
```

| State        | Agency  |
|--------------|---------|
| `BFIdle`     | Client  |
| `BFBusy`     | Server  |
| `BFStreaming` | Server |
| `BFDone`     | Nobody  |

### Wire Format

Source: `ouroboros-network/ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/BlockFetch/Codec.hs`
and `cardano-diffusion/protocols/cddl/specs/block-fetch.cddl`

```
MsgRequestRange = [0, lower_point, upper_point]
MsgClientDone   = [1]
MsgStartBatch   = [2]
MsgNoBlocks     = [3]
MsgBlock        = [4, block]
MsgBatchDone    = [5]
```

**`MsgRequestRange`:** Both `lower_point` and `upper_point` are inclusive
(the range spans from lower to upper, both included). Each point uses the
standard `point` encoding (`[]` for Origin, `[slot, hash]` for specific).

**Block encoding in `MsgBlock`:** For `CardanoBlock`, the block is encoded as:

```
block = [era_index, tag(24)(bstr(cbor_encoded_block))]
```

The full block (including header and body) is CBOR-serialized, then wrapped in
`tag(24)(bytes(cbor_bytes))` (CBOR-in-CBOR), then placed in a 2-element array
with the HFC era index.

Source: `ouroboros-consensus/ouroboros-consensus-cardano/src/shelley/Ouroboros/Consensus/Shelley/Node/Serialisation.hs`

### Timing

Source: `ouroboros-network/ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/BlockFetch/Codec.hs:timeLimitsBlockFetch`

| State         | Timeout     |
|---------------|-------------|
| `BFIdle`      | waitForever |
| `BFBusy`      | 60 s (longWait) |
| `BFStreaming` | 60 s (longWait) |

### Byte Limits

| State         | Limit              |
|---------------|--------------------|
| `BFIdle`      | 65535 bytes (smallByteLimit) |
| `BFBusy`      | 65535 bytes (smallByteLimit) |
| `BFStreaming` | 2 500 000 bytes (largeByteLimit) |

### BlockFetch Decision Loop

The `blockFetchLogic` thread runs continuously, waking every 10 ms (Praos) or
40 ms (Genesis). It reads candidate chains from ChainSync via STM, computes
which block ranges need to be fetched, and issues `MsgRequestRange` messages.

| Parameter                  | Default | Source |
|----------------------------|---------|--------|
| `maxInFlightReqsPerPeer`   | 100     | `blockFetchPipeliningMax` |
| `maxConcurrencyBulkSync`   | 1 peer  | `bfcMaxConcurrencyBulkSync` |
| `maxConcurrencyDeadline`   | 1 peer  | `bfcMaxConcurrencyDeadline` |
| Decision loop interval (Praos) | 10 ms | `bfcDecisionLoopIntervalPraos` |
| Decision loop interval (Genesis) | 40 ms | `bfcDecisionLoopIntervalGenesis` |

Source: `cardano-diffusion/lib/Cardano/Network/Diffusion/Configuration.hs:defaultBlockFetchConfiguration`

### Ingress Queue Limit

`max(10 × 2 097 154, 100 × 90 112) × 1.1` ≈ 22 MB.

---

## N2N Protocol 4: TxSubmission2

### Identity
- **Protocol ID:** 4
- **Temperature:** Hot
- **Direction:** **Inverted agency** — the server (inbound/receiver) has agency
  first. The server *requests* transactions; the client *replies* with them.
  This is the opposite of most protocols.
- **Versions:** All N2N versions. V2 logic (multi-peer decision loop) is
  enabled server-side when `TxSubmissionLogicV2` is configured; V1 is the
  current default in cardano-node.

### State Machine

Source: `ouroboros-network/ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/TxSubmission2/Type.hs`

```
StInit  (ClientAgency)   -- client must send MsgInit before anything else
    │
    │ MsgInit
    ▼
StIdle  (ServerAgency)   -- server has agency; requests txids or terminates
    │
    ├─── MsgRequestTxIds(blocking=true)    ──→ StTxIds(StBlocking)
    ├─── MsgRequestTxIds(blocking=false)   ──→ StTxIds(StNonBlocking)
    ├─── MsgRequestTxs                     ──→ StTxs
    └─── MsgDone                           ──→ StDone

StTxIds(StBlocking)   (ClientAgency)   -- client MUST reply, no timeout
    │
    └─── MsgReplyTxIds(NonEmpty list)  ──→ StIdle
         (BlockingReply: list must be non-empty)

StTxIds(StNonBlocking)  (ClientAgency)  -- client must reply within shortWait
    │
    └─── MsgReplyTxIds(possibly empty) ──→ StIdle

StTxs  (ClientAgency)  -- client must reply with requested tx bodies
    │
    └─── MsgReplyTxs(tx list)  ──→ StIdle

StDone (NobodyAgency)
```

**`MsgDone` constraint:** `MsgDone` can only be sent from `StIdle` (server side).
It is the server's prerogative to terminate, not the client's.

### Wire Format

Source: `ouroboros-network/ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/TxSubmission2/Codec.hs:encodeTxSubmission2`
and `cardano-diffusion/protocols/cddl/specs/tx-submission2.cddl`

```
MsgInit           = [6]

MsgRequestTxIds   = [0, blocking:bool, ack:word16, req:word16]
                  ; blocking=true  → StTxIds(StBlocking)
                  ; blocking=false → StTxIds(StNonBlocking)

MsgReplyTxIds     = [1, [_ *[txid, size:word32] ]]
                  ; INDEFINITE-length outer list (encodeListLenIndef)
                  ; Each inner entry is a DEFINITE-length array(2)

MsgRequestTxs     = [2, [_ *txid ]]
                  ; INDEFINITE-length list

MsgReplyTxs       = [3, [_ *tx ]]
                  ; INDEFINITE-length list

MsgDone           = [4]
```

**IMPORTANT:** Both `MsgReplyTxIds`, `MsgRequestTxs`, and `MsgReplyTxs` use
**indefinite-length** CBOR arrays (encoded with `encodeListLenIndef` and
terminated with `encodeBreak`). The codec explicitly requires this. Using
definite-length arrays is a decoding error.

**HFC era-tag wrapping for txids and txs:**

For the Cardano HFC instantiation, each `txid` and each `tx` is wrapped with
the era index before being placed into the list. The wrapping is done by
`encodeNS` in `ouroboros-consensus`:

```
; txid (GenTxId) encoding
txid = [era_index:uint8, bstr(32)]
     ; era_index: 0=Byron, 1=Shelley, 2=Allegra, 3=Mary, 4=Alonzo,
     ;            5=Babbage, 6=Conway, 7=Dijkstra
     ; payload:   32 raw bytes = Blake2b-256 hash of tx body (no CBOR tag)

; tx (GenTx) encoding
tx = [era_index:uint8, tag(24)(bstr(cbor_of_tx))]
   ; The transaction CBOR bytes are wrapped in CBOR tag 24 (embedded CBOR)
```

Example for Conway (era_index=6):
```
txid = [6, bstr(32_bytes_of_txhash)]
tx   = [6, #6.24(bstr(cbor_bytes_of_transaction))]
```

Source: `ouroboros-consensus/ouroboros-consensus-diffusion/src/.../Consensus/Network/NodeToNode.hs`
and `ouroboros-consensus/src/.../HardFork/Combinator/Serialisation/Common.hs:encodeNS`

### MsgReplyTxIds — Size Reporting

Each entry in `MsgReplyTxIds` carries a `SizeInBytes` (`word32`) alongside the
txid. This size **must** include the full HFC envelope overhead that the tx will
have in `MsgReplyTxs`. For Conway: 3 bytes overhead (1 byte array-of-2 header,
1 byte era_index word8, CBOR tag 24 header). Mismatches beyond the tolerance
threshold (`const_MAX_TX_SIZE_DISCREPANCY = 10 bytes` in V2 inbound) terminate
the connection.

### Blocking vs Non-Blocking Rules

In **blocking mode** (`MsgRequestTxIds(blocking=true)`):
- `req_count` must be >= 1
- `MsgReplyTxIds` reply must contain a non-empty list (`BlockingReply`)
- No timeout: the client MAY block indefinitely in STM waiting for new mempool entries

In **non-blocking mode** (`MsgRequestTxIds(blocking=false)`):
- At least one of `ack_count` or `req_count` must be non-zero
- `MsgReplyTxIds` reply may be empty (`NonBlockingReply []`)
- Timeout: `shortWait` (10 seconds)

**Acknowledgment semantics:** `ack_count` tells the client how many previously
announced txids can now be removed from the outbound window. The client
maintains a FIFO of `unacknowledgedTxIds`. When the server sends
`MsgRequestTxIds(ack=N, req=M)`, the client drops the first `N` entries from
the FIFO and adds up to `M` new txids from the mempool.

### Timing

Source: `ouroboros-network/ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/TxSubmission2/Codec.hs:timeLimitsTxSubmission2`

| State                     | Timeout     |
|---------------------------|-------------|
| `StInit`                  | waitForever |
| `StIdle`                  | waitForever |
| `StTxIds(StBlocking)`     | waitForever |
| `StTxIds(StNonBlocking)`  | 10 s (shortWait) |
| `StTxs`                   | 10 s (shortWait) |

### V1 Server Constants (current default)

| Parameter               | Value |
|-------------------------|-------|
| `maxTxIdsToRequest`     | 3     |
| `maxTxToRequest`        | 2     |
| `maxUnacknowledgedTxIds`| 100   |
| `txSubmissionInitDelay` | 60 s  |

The 60-second init delay is applied via `threadDelay` before the V1 server makes
its first `MsgRequestTxIds`. This intentionally avoids requesting transactions
during initial chain sync.

### V2 Server Constants (experimental)

| Parameter                    | Value           |
|------------------------------|-----------------|
| `maxNumTxIdsToRequest`       | 12              |
| `maxUnacknowledgedTxIds`     | 100             |
| `txsSizeInflightPerPeer`     | 6 × 65540 bytes |
| `txInflightMultiplicity`     | 2               |
| Decision loop delay          | 5 ms            |

Source: `ouroboros-network/ouroboros-network/lib/Ouroboros/Network/TxSubmission/Inbound/V2/`

### MsgInit Requirement

`MsgInit` (tag=6, one-element array `[6]`) must be the **very first message**
sent by the client (outbound side) after the mux connection is established for
the TxSubmission2 protocol. The server waits for `MsgInit` in `StInit` before
transitioning to `StIdle`. Sending any other message first is a protocol error.

### Ingress Queue Limit

`maxUnacknowledgedTxIds × (44 + 65536) × 1.1`

With `maxUnacknowledgedTxIds=100`: approximately 6 666 400 bytes.

---

## N2N Protocol 8: KeepAlive

### Identity
- **Protocol ID:** 8
- **Temperature:** Established (started on cold→warm, runs for entire connection lifetime)
- **Purpose:** Detects connection failure and measures round-trip time for
  GSV (Good-Spread-Variable) calculations used in BlockFetch prioritization.

### State Machine

Source: `ouroboros-network/ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/KeepAlive/Type.hs`

```
StClient  (ClientAgency)  -- client sends keep-alive request
    │
    ├─── MsgKeepAlive(cookie)  ──→ StServer
    └─── MsgDone               ──→ StDone

StServer  (ServerAgency)  -- server must respond with same cookie
    │
    └─── MsgKeepAliveResponse(cookie)  ──→ StClient

StDone (NobodyAgency)
```

### Wire Format

Source: `ouroboros-network/ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/KeepAlive/Codec.hs:codecKeepAlive_v2`

```
MsgKeepAlive         = [0, cookie:word16]
MsgKeepAliveResponse = [1, cookie:word16]
MsgDone              = [2]
```

**Cookie matching:** The server must echo back the exact `cookie` value sent by
the client. A mismatch raises `KeepAliveCookieMissmatch` (note: the Haskell
source has the typo "Missmatch" with double-s), which terminates the connection.

### Timing

Source: `ouroboros-network/ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/KeepAlive/Codec.hs:timeLimitsKeepAlive`

| State      | Timeout |
|------------|---------|
| `StClient` | 97 seconds |
| `StServer` | 60 seconds |

The asymmetry is intentional: the client side (97 s) is how long the client
waits before sending the next keep-alive; the server side (60 s) is how long
the server has to respond. The comment in source notes that `StServer` timeout
"should be 10s" (issue #2505) but is currently 60 s.

### Byte Limits

Both states: `smallByteLimit` (65535 bytes).

### Protocol Error Condition

`KeepAliveCookieMissmatch oldCookie receivedCookie` — thrown when
`MsgKeepAliveResponse` cookie does not match the outstanding request cookie.
This terminates the connection.

---

## N2N Protocol 10: PeerSharing

### Identity
- **Protocol ID:** 10
- **Temperature:** Established (started on cold→warm)
- **Purpose:** Exchange of peer addresses to assist in peer discovery. Only
  active when both sides negotiated `peerSharing=1` in Handshake.

### State Machine

Source: `ouroboros-network/ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/PeerSharing/Type.hs`

```
StIdle  (ClientAgency)  -- client requests peer addresses or terminates
    │
    ├─── MsgShareRequest(amount)  ──→ StBusy
    └─── MsgDone                  ──→ StDone

StBusy  (ServerAgency)  -- server must reply with peer list
    │
    └─── MsgSharePeers(addrs)  ──→ StIdle

StDone (NobodyAgency)
```

### Wire Format

Source: `ouroboros-network/cardano-diffusion/protocols/lib/Cardano/Network/Protocol/PeerSharing/Codec.hs`
and `cardano-diffusion/protocols/cddl/specs/peer-sharing-v14.cddl`

```
MsgShareRequest = [0, amount:word8]
MsgSharePeers   = [1, [* peerAddress]]
MsgDone         = [2]

; Peer address encoding (SockAddr)
peerAddress = [0, ipv4:word32, port:word16]
            ; IPv4: single u32 in network byte order, then port as word16
            / [1, word32, word32, word32, word32, port:word16]
            ; IPv6: four u32s (network byte order), then port as word16
```

**Protocol error condition:** If the server replies with more addresses than
`amount` requested, it is a protocol error. The client must request no more
than 255 peers (`word8` max).

### Timing

| State    | Timeout     |
|----------|-------------|
| `StIdle` | waitForever |
| `StBusy` | 60 s (longWait) |

### Server Address Selection Policy

The server only shares addresses for peers that satisfy all of:
- `knownPeerAdvertise = DoAdvertisePeer`
- `knownSuccessfulConnection = True`
- `knownPeerFailCount = 0`

Addresses are randomized using a hash with a salt that rotates every 823
seconds to prevent fingerprinting.

Source: `ouroboros-network/ouroboros-network/api/lib/Ouroboros/Network/PeerSelection/PeerSharing/Codec.hs`
and `ouroboros-network/ouroboros-network/lib/Ouroboros/Network/PeerSharing.hs`

### Key Policy Constants

| Constant                           | Value |
|------------------------------------|-------|
| `policyMaxInProgressPeerShareReqs` | 2     |
| `policyPeerShareRetryTime`         | 900 s |
| `policyPeerShareBatchWaitTime`     | 3 s   |
| `policyPeerShareOverallTimeout`    | 10 s  |
| `policyPeerShareActivationDelay`   | 300 s |
| `ps_POLICY_PEER_SHARE_STICKY_TIME` | 823 s (salt rotation) |
| `ps_POLICY_PEER_SHARE_MAX_PEERS`   | 10    |

Source: `ouroboros-network/ouroboros-network/lib/Ouroboros/Network/Diffusion/Policies.hs`

---

## N2C Protocol 0: Handshake (Node-to-Client)

### Identity
- **Protocol ID:** 0 (same as N2N, runs on raw socket before mux)
- **Direction:** Same as N2N: client proposes, server accepts or refuses
- **Versions:** V16 (=32784) through V23 (=32791)

### Wire Format

Source: CDDL: `cardano-diffusion/protocols/cddl/specs/handshake-node-to-client.cddl`
Codec: same `codecHandshake` function as N2N, parameterized on version number type.

```
; Messages are identical in structure to N2N handshake
MsgProposeVersions = [0, versionTable]
MsgAcceptVersion   = [1, versionNumber, nodeToClientVersionData]
MsgRefuse          = [2, refuseReason]
MsgQueryReply      = [3, versionTable]

; N2C version numbers have bit 15 set to distinguish from N2N
; V16=32784, V17=32785, V18=32786, V19=32787,
; V20=32788, V21=32789, V22=32790, V23=32791
versionNumber = 32784 / 32785 / 32786 / 32787 / 32788 / 32789 / 32790 / 32791

; Encoding: versionNumber_wire = logical_version | 0x8000
; Decoding: logical_version = wire_value & 0x7FFF (after verifying bit 15 is set)

; Version data (V16+): 2-element array
nodeToClientVersionData = [networkMagic:uint, query:bool]
```

The `versionTable` in `MsgProposeVersions` is a **definite-length CBOR map**
with entries sorted in **ascending key order**.

### Version Features

| N2C Version | Wire Value | What Changed |
|-------------|------------|--------------|
| V16 | 32784 | Conway era; ImmutableTip acquire; GetStakeDelegDeposits |
| V17 | 32785 | GetProposals, GetRatifyState |
| V18 | 32786 | GetFuturePParams |
| V19 | 32787 | GetBigLedgerPeerSnapshot |
| V20 | 32788 | QueryStakePoolDefaultVote; MsgGetMeasures in LocalTxMonitor |
| V21 | 32789 | New ProtVer codec for Shelley-Babbage; GetPoolDistr2, GetStakeDistribution2, GetMaxMajorProtVersion |
| V22 | 32790 | SRV records in GetBigLedgerPeerSnapshot |
| V23 | 32791 | GetDRepDelegations; LedgerPeerSnapshot includes block hash + NetworkMagic |

Source: `cardano-diffusion/api/lib/Cardano/Network/NodeToClient/Version.hs`

### Version Negotiation

Same rules as N2N:
- Highest common version wins.
- `networkMagic` must match.
- `query = local || remote` (logical OR).
- No `initiatorOnlyDiffusionMode` or `peerSharing` fields in N2C version data.

---

## N2C Protocol 5: LocalChainSync

### Identity
- **Protocol ID:** 5
- **Direction:** N2C clients receive **full serialized blocks** (not just
  headers). This is the key difference from N2N ChainSync.
- **Versions:** All N2C versions

### State Machine

Identical state machine to N2N ChainSync (same Type.hs). See that section for
the complete state machine diagram.

### Wire Format

Messages tags are identical to N2N ChainSync (0–7). The key difference is the
content of `MsgRollForward`.

**N2C `MsgRollForward` block encoding:**

```
; N2C LocalChainSync block payload in MsgRollForward
block = [era_id:uint, tag(24)(bstr(cbor_of_full_block))]
```

The entire block (header + body) is CBOR-encoded, wrapped in CBOR `tag(24)`
(embedded CBOR), and then paired with the era index in a 2-element array.

Era indices: same as TxSubmission2 (0=Byron through 7=Dijkstra).

This matches the same HFC wrapping used by BlockFetch `MsgBlock` in N2N.

### Differences from N2N ChainSync

| Aspect             | N2N ChainSync                    | N2C LocalChainSync         |
|--------------------|----------------------------------|----------------------------|
| Payload type       | Block headers only               | Full blocks                |
| Purpose            | Chain selection                  | Wallet / tool consumption  |
| Pipelining         | Yes (pipelineDecisionLowHighMark) | Typically none             |
| Source of blocks   | Server → client                  | Server → client            |

Source: `ouroboros-network/ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/ChainSync/Codec.hs` (same codec)

---

## N2C Protocol 6: LocalTxSubmission

### Identity
- **Protocol ID:** 6
- **Direction:** Client submits a single transaction; server accepts or rejects.
- **No HFC era-tag wrapping:** Unlike N2N TxSubmission2, N2C LocalTxSubmission
  sends raw transaction CBOR without any HFC era-index prefix.
- **Versions:** All N2C versions

### State Machine

Source: `ouroboros-network/ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/LocalTxSubmission/Type.hs`

```
StIdle  (ClientAgency)  -- client submits a transaction or terminates
    │
    ├─── MsgSubmitTx(tx)  ──→ StBusy
    └─── MsgDone          ──→ StDone

StBusy  (ServerAgency)  -- server validates and responds
    │
    ├─── MsgAcceptTx   ──→ StIdle
    └─── MsgRejectTx   ──→ StIdle

StDone (NobodyAgency)
```

**Blocking semantics:** After sending `MsgSubmitTx`, the client **must** wait
for `MsgAcceptTx` or `MsgRejectTx` before sending another transaction. This
protocol processes one transaction at a time. This is intentional: N2C is
only used by local trusted clients (wallets, CLI), so throughput is not a
concern.

### Wire Format

Source: `ouroboros-network/ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/LocalTxSubmission/Codec.hs:encodeLocalTxSubmission`
and `cardano-diffusion/protocols/cddl/specs/local-tx-submission.cddl`

```
MsgSubmitTx = [0, tx]
MsgAcceptTx = [1]
MsgRejectTx = [2, rejectReason]
MsgDone     = [3]
```

**Transaction encoding (`tx`):** Raw transaction CBOR, exactly as produced
by `toCBOR` on the ledger's `Tx` type. No HFC wrapper, no era tag, no
`tag(24)`. The server determines the era from the ledger state.

**Rejection reason (`rejectReason`):** The full `ApplyTxError` encoded via
the ledger's `EncCBOR` instance. For Conway, this is a nested structure of
`ConwayLedgerPredFailure` variants. The exact encoding is era-specific and
defined in `cardano-ledger`.

Source: `cardano-ledger/eras/conway/impl/src/Cardano/Ledger/Conway/Rules/`

---

## N2C Protocol 7: LocalStateQuery

### Identity
- **Protocol ID:** 7
- **Direction:** Client acquires a ledger state snapshot and submits queries;
  server responds with query results.
- **Versions:** All N2C versions. Some queries require specific minimum versions
  (see Shelley query tag table).

### State Machine

Source: `ouroboros-network/ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/LocalStateQuery/Type.hs`

```
StIdle      (ClientAgency)   -- client acquires a state or terminates
    │
    ├─── MsgAcquire(target)  ──→ StAcquiring
    └─── MsgDone             ──→ StDone

StAcquiring  (ServerAgency)  -- server acquiring the requested state
    │
    ├─── MsgAcquired         ──→ StAcquired
    └─── MsgFailure(reason)  ──→ StIdle

StAcquired   (ClientAgency)  -- client can query or release
    │
    ├─── MsgQuery(query)     ──→ StQuerying
    ├─── MsgRelease          ──→ StIdle
    └─── MsgReAcquire(target)──→ StAcquiring

StQuerying   (ServerAgency)  -- server computing query result
    │
    └─── MsgResult(result)   ──→ StAcquired

StDone (NobodyAgency)
```

**Re-acquire:** `MsgReAcquire` transitions from `StAcquired` directly back to
`StAcquiring`, allowing the client to acquire a new state without going through
`StIdle`. This avoids a round trip.

### Acquire Targets

Three targets exist for `MsgAcquire` and `MsgReAcquire`:

| Target           | CBOR           | Semantics                                        | Min Version |
|------------------|----------------|--------------------------------------------------|-------------|
| `SpecificPoint`  | `[0, point]`   | Acquire the state at a specific slot/hash point  | V8+ (any)   |
| `VolatileTip`    | `[8]`          | Acquire the current tip of the volatile chain    | V8+         |
| `ImmutableTip`   | `[10]`         | Acquire the tip of the immutable chain           | N2C V16+    |

For `MsgReAcquire`: tags are shifted by 3 → `SpecificPoint=[6, point]`,
`VolatileTip=[9]`, `ImmutableTip=[11]` (V16+).

`VolatileTip` and `ImmutableTip` cannot fail (they always succeed with
`MsgAcquired`). `SpecificPoint` can fail if the point is not in the volatile
chain window (yields `MsgFailure`).

### Acquire Failure Codes

```
AcquireFailurePointTooOld     = 0
AcquireFailurePointNotOnChain = 1
```

### Wire Format

Source: `ouroboros-network/ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/LocalStateQuery/Codec.hs:codecLocalStateQuery`
and `cardano-diffusion/protocols/cddl/specs/local-state-query.cddl`

```
; Acquire / Re-acquire
MsgAcquire(SpecificPoint pt)  = [0, point]
MsgAcquire(VolatileTip)       = [8]
MsgAcquire(ImmutableTip)      = [10]    ; V16+ only

MsgAcquired                   = [1]

MsgFailure(reason)            = [2, failure_code:uint]
                              ; 0=PointTooOld, 1=PointNotOnChain

MsgQuery(query)               = [3, query_encoding]
MsgResult(result)             = [4, result_encoding]
MsgRelease                    = [5]

MsgReAcquire(SpecificPoint pt)= [6, point]
MsgReAcquire(VolatileTip)     = [9]
MsgReAcquire(ImmutableTip)    = [11]    ; V16+ only

MsgDone                       = [7]
```

### Query Encoding (Three-Level HFC Wrapping)

Queries are wrapped in three layers. The outermost layer is the consensus-level
`Query` type (in `Ouroboros.Consensus.Ledger.Query`):

```
; Outermost consensus layer
query = [2, tag=0, wrapped_block_query]   ; BlockQuery — delegates to HFC
      / [1, tag=1]                         ; GetSystemStart
      / [1, tag=2]                         ; GetChainBlockNo (V16+ / QueryVersion2)
      / [1, tag=3]                         ; GetChainPoint  (V16+ / QueryVersion2)
      / [1, tag=4]                         ; DebugLedgerConfig (V20+ / QueryVersion3)
```

For `BlockQuery` (tag=0), the next layer is the HFC query:

```
; HFC (Hard Fork Combinator) layer
hfc_query = [2, tag=0, era_query]   ; QueryIfCurrent — query current era
          / [3, tag=1, era_query, era_index]   ; QueryAnytime
          / [2, tag=2, hf_specific]            ; QueryHardFork
```

For `QueryIfCurrent`, the era index is determined by dispatch; there is no
explicit era tag in the message. The `era_query` is the era-level query:

```
; Era-level query (Shelley BlockQuery tags)
; These are 1-element or 2-element arrays with a numeric tag
era_query = [1, tag=0]    ; GetLedgerTip
          / [1, tag=1]    ; GetEpochNo
          / [2, tag=2, ..] ; GetNonMyopicMemberRewards
          / [1, tag=3]    ; GetCurrentPParams
          ; ... (see full table below)
```

### Shelley BlockQuery Tag Table

| Tag | Query Name | Min N2C Version |
|-----|------------|-----------------|
|  0 | GetLedgerTip | V8 |
|  1 | GetEpochNo | V8 |
|  2 | GetNonMyopicMemberRewards | V8 |
|  3 | GetCurrentPParams | V8 |
|  4 | GetProposedPParamsUpdates | V8 |
|  5 | GetStakeDistribution | V8 (removed in V21) |
|  6 | GetUTxOByAddress | V8 |
|  7 | GetUTxOWhole | V8 |
|  8 | DebugEpochState | V8 |
|  9 | GetCBOR (wraps inner query in tag(24)) | V8 |
| 10 | GetFilteredDelegationsAndRewardAccounts | V8 |
| 11 | GetGenesisConfig | V8 |
| 12 | DebugNewEpochState | V8 |
| 13 | DebugChainDepState | V8 |
| 14 | GetRewardProvenance | V9 |
| 15 | GetUTxOByTxIn | V10 |
| 16 | GetStakePools | V11 |
| 17 | GetStakePoolParams | V11 |
| 18 | GetRewardInfoPools | V11 |
| 19 | GetPoolState | V11 |
| 20 | GetStakeSnapshots | V11 |
| 21 | GetPoolDistr | V11 (removed in V21) |
| 22 | GetStakeDelegDeposits | V16 |
| 23 | GetConstitution | V16 |
| 24 | GetGovState | V16 |
| 25 | GetDRepState | V16 |
| 26 | GetDRepStakeDistr | V16 |
| 27 | GetCommitteeMembersState | V16 |
| 28 | GetFilteredVoteDelegatees | V16 |
| 29 | GetAccountState | V16 |
| 30 | GetSPOStakeDistr | V16 |
| 31 | GetProposals | V17 |
| 32 | GetRatifyState | V17 |
| 33 | GetFuturePParams | V18 |
| 34 | GetLedgerPeerSnapshot | V19 |
| 35 | QueryStakePoolDefaultVote | V20 |
| 36 | GetPoolDistr2 | V21 |
| 37 | GetStakeDistribution2 | V21 |
| 38 | GetMaxMajorProtVersion | V21 |
| 39 | GetDRepDelegations | V23 |

Source: `cardano-diffusion/api/lib/Cardano/Network/NodeToClient/Version.hs` and
`ouroboros-consensus/ouroboros-consensus-cardano/src/unstable-cardano-tools/Cardano/Tools/DBAnalyser/Block/Cardano.hs`

### MsgResult Wrapping

For `QueryIfCurrent` queries, the result is wrapped in an `EitherMismatch`
type to indicate whether the query was applied to the correct era:

```
; QueryIfCurrent result encoding
result = [result_value]          ; Success: definite-length array(1) wrapping the value
       / [era_mismatch_info]     ; Era mismatch: see EraEraMismatch encoding
```

A successful `QueryIfCurrent` result is wrapped in a **1-element definite-length
array**. This is easy to miss and causes decoding failures if omitted.

`QueryAnytime` and `QueryHardFork` results are **not** wrapped in this extra
array.

---

## N2C Protocol 9: LocalTxMonitor

### Identity
- **Protocol ID:** 9
- **Direction:** Client monitors the node's mempool contents.
- **Versions:** All N2C versions. `MsgGetMeasures`/`MsgReplyGetMeasures`
  require N2C V20+.

### State Machine

Source: `ouroboros-network/ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/LocalTxMonitor/Type.hs`

```
StIdle      (ClientAgency)   -- client can acquire a snapshot or terminate
    │
    ├─── MsgAcquire  ──→ StAcquiring
    └─── MsgDone     ──→ StDone

StAcquiring  (ServerAgency)  -- server captures mempool snapshot
    │
    └─── MsgAcquired(slotNo)  ──→ StAcquired

StAcquired   (ClientAgency)  -- client queries snapshot or releases
    │
    ├─── MsgNextTx          ──→ StBusy(NextTx)
    ├─── MsgHasTx(txid)     ──→ StBusy(HasTx)
    ├─── MsgGetSizes        ──→ StBusy(GetSizes)
    ├─── MsgGetMeasures     ──→ StBusy(GetMeasures)   ; V20+ only
    ├─── MsgAwaitAcquire    ──→ StAcquiring           ; refresh snapshot
    └─── MsgRelease         ──→ StIdle

StBusy(NextTx)      (ServerAgency)
    └─── MsgReplyNextTx(maybe tx)  ──→ StAcquired

StBusy(HasTx)       (ServerAgency)
    └─── MsgReplyHasTx(bool)       ──→ StAcquired

StBusy(GetSizes)    (ServerAgency)
    └─── MsgReplyGetSizes(sizes)   ──→ StAcquired

StBusy(GetMeasures) (ServerAgency)   ; V20+
    └─── MsgReplyGetMeasures(m)    ──→ StAcquired

StDone (NobodyAgency)
```

**Snapshot semantics:** After `MsgAcquired`, the client holds a fixed snapshot
of the mempool as of the `slotNo` returned. The snapshot does not change even
if new transactions arrive or are removed. `MsgAwaitAcquire` refreshes the
snapshot without going through `StIdle`.

### Wire Format

Source: `ouroboros-network/ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/LocalTxMonitor/Codec.hs`
and `cardano-diffusion/protocols/cddl/specs/local-tx-monitor.cddl`

```
MsgDone             = [0]

MsgAcquire          = [1]          ; same tag for initial acquire from StIdle
MsgAwaitAcquire     = [1]          ; same tag for re-acquire from StAcquired

MsgAcquired         = [2, slotNo:word64]

MsgRelease          = [3]

MsgNextTx           = [5]          ; note: tag 4 is unused
MsgReplyNextTx      = [6]          ; no tx: empty mempool
                    / [6, tx]      ; with tx: next transaction in snapshot

MsgHasTx            = [7, txId]
MsgReplyHasTx       = [8, bool]

MsgGetSizes         = [9]
MsgReplyGetSizes    = [10, [capacityInBytes:word32,
                            sizeInBytes:word32,
                            numberOfTxs:word32]]

MsgGetMeasures      = [11]         ; V20+ only
MsgReplyGetMeasures = [12, txCount:word32, {* tstr => [integer, integer]}]
                    ; V20+ only
```

**Tag 4 is intentionally unused.** Tags jump from 3 (`MsgRelease`) to 5
(`MsgNextTx`).

**`MsgReplyNextTx`:** Uses the same tag (6) for both the no-tx and has-tx
cases, distinguished by array length: `[6]` (len=1) means no more txs;
`[6, tx]` (len=2) means a tx follows.

**`MsgAcquire` and `MsgAwaitAcquire`** use the same wire tag `[1]`. The
protocol state (`StIdle` vs `StAcquired`) determines which message is
being decoded. This is handled by the state token in the codec.

**Transaction encoding:** Same as LocalTxSubmission — raw CBOR with no HFC
wrapping.

**`txId` encoding:** Raw 32-byte Blake2b-256 hash as CBOR bytes primitive.

---

## Initialization Sequence

### N2N Connection Startup

After the TCP connection is established:

1. **Handshake** (protocol 0): Both sides send `MsgProposeVersions`
   simultaneously (simultaneous open). The one with the lower socket address
   keeps the outbound role; the other keeps the inbound. Each side processes
   the other's proposal and the higher-address side sends `MsgAcceptVersion`
   or `MsgRefuse`. The connection proceeds only if both sides determine the
   same version.

2. **Mux starts:** After successful handshake, the mux multiplexer and
   demultiplexer threads are started. Protocol threads are started based on
   peer temperature.

3. **Cold→Warm:** `KeepAlive` (8) and `PeerSharing` (10) initiator threads
   start eagerly.

4. **Warm→Hot:** `ChainSync` (2), `BlockFetch` (3), `TxSubmission2` (4)
   initiator threads start eagerly. Responder threads start on-demand (when
   first inbound bytes arrive).

5. **TxSubmission2 MsgInit:** The TxSubmission2 **client** (outbound side)
   must send `MsgInit` (`[6]`) as its very first message. Without this,
   the server stays in `StInit` indefinitely (waitForever timeout).

### N2C Connection Startup

1. **Handshake** (protocol 0): Same mechanism, but using N2C version numbers
   (with bit 15 set). The local client proposes; the node accepts.

2. **Mux starts:** All N2C mini-protocols start eagerly on both sides.

3. **No mandatory initial messages:** Unlike N2N TxSubmission2, no N2C
   protocol requires a mandatory initial message before the first
   client request. The client may begin with `MsgAcquire` (LocalStateQuery),
   `MsgSubmitTx` (LocalTxSubmission), or `MsgAcquire` (LocalTxMonitor)
   immediately.

---

## HFC Era Index Table

This table applies to all N2N protocols (ChainSync headers, BlockFetch blocks,
TxSubmission2 txids/txs) and N2C LocalChainSync blocks.

| Era Index | Era |
|-----------|-----|
| 0 | Byron |
| 1 | Shelley (TPraos) |
| 2 | Allegra (TPraos) |
| 3 | Mary (TPraos) |
| 4 | Alonzo (TPraos) |
| 5 | Babbage (Praos) |
| 6 | Conway (Praos) |
| 7 | Dijkstra (Praos, future) |

Source: `ouroboros-consensus/ouroboros-consensus-cardano/src/unstable-cardano-consensus/Ouroboros/Consensus/Cardano/Block.hs`

---

## Summary: Protocol Error Triggers

This table lists the most common protocol violations that terminate the
connection.

| Protocol        | Error Condition | Trigger |
|-----------------|-----------------|---------|
| Handshake       | `VersionMismatch`     | No common version in propose |
| Handshake       | `Refused`             | Magic mismatch, policy rejection |
| Handshake       | `HandshakeDecodeError`| Failed to decode version params |
| ChainSync       | Agency violation      | Client sends `MsgRollForward` (server-only message) |
| ChainSync       | `ProtocolErrorRequestNonBlocking` | Server sends `MsgAwaitReply` but `StNext(StMustReply)` was active (not `StCanAwait`) |
| BlockFetch      | Agency violation      | Client sends `MsgBlock` (server-only message) |
| TxSubmission2   | Protocol error        | Any message before `MsgInit` is processed |
| TxSubmission2   | `BlockingReply` empty | Server sends `MsgRequestTxIds(blocking=true)` and client replies with empty list |
| TxSubmission2   | Size mismatch         | Reported `SizeInBytes` deviates >10 bytes from actual tx wire size (V2 inbound) |
| KeepAlive       | `KeepAliveCookieMissmatch` | Response cookie != request cookie |
| PeerSharing     | Protocol error        | Server replies with more peers than requested |
| LocalStateQuery | `AcquireFailurePointTooOld` | `SpecificPoint` is outside the volatile window |
| LocalStateQuery | `AcquireFailurePointNotOnChain` | `SpecificPoint` not on the node's chain |
| LocalStateQuery | `ImmutableTip` on old version | Attempting `MsgAcquire(ImmutableTip)` before N2C V16 |
| Any             | Byte limit exceeded   | Ingress queue overflow (per-state byte limits) |
| Any             | Timeout exceeded      | Per-state timing limits (see per-protocol tables) |

---

## Source File Index

All files are in the `IntersectMBO/ouroboros-network` repository (main branch)
unless otherwise noted.

| Protocol / Topic | File |
|------------------|------|
| N2N Handshake Type | `ouroboros-network/framework/lib/Ouroboros/Network/Protocol/Handshake/Type.hs` |
| N2N Handshake Codec | `ouroboros-network/framework/lib/Ouroboros/Network/Protocol/Handshake/Codec.hs` |
| N2N Handshake CDDL | `cardano-diffusion/protocols/cddl/specs/handshake-node-to-node-v14.cddl` |
| N2C Handshake CDDL | `cardano-diffusion/protocols/cddl/specs/handshake-node-to-client.cddl` |
| N2N Version data v14 CDDL | `cardano-diffusion/protocols/cddl/specs/node-to-node-version-data-v14.cddl` |
| N2N Version data v16 CDDL | `cardano-diffusion/protocols/cddl/specs/node-to-node-version-data-v16.cddl` |
| N2C Version enum | `cardano-diffusion/api/lib/Cardano/Network/NodeToClient/Version.hs` |
| N2N Version enum | `cardano-diffusion/api/lib/Cardano/Network/NodeToNode/Version.hs` |
| ChainSync Type | `ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/ChainSync/Type.hs` |
| ChainSync Codec | `ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/ChainSync/Codec.hs` |
| ChainSync TimeLimits | `cardano-diffusion/protocols/lib/Cardano/Network/Protocol/ChainSync/Codec/TimeLimits.hs` |
| ChainSync CDDL | `cardano-diffusion/protocols/cddl/specs/chain-sync.cddl` |
| ChainSync Pipelining | `ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/ChainSync/PipelineDecision.hs` |
| BlockFetch Type | `ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/BlockFetch/Type.hs` |
| BlockFetch Codec | `ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/BlockFetch/Codec.hs` |
| BlockFetch CDDL | `cardano-diffusion/protocols/cddl/specs/block-fetch.cddl` |
| TxSubmission2 Type | `ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/TxSubmission2/Type.hs` |
| TxSubmission2 Codec | `ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/TxSubmission2/Codec.hs` |
| TxSubmission2 CDDL | `cardano-diffusion/protocols/cddl/specs/tx-submission2.cddl` |
| KeepAlive Type | `ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/KeepAlive/Type.hs` |
| KeepAlive Codec | `ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/KeepAlive/Codec.hs` |
| KeepAlive CDDL | `cardano-diffusion/protocols/cddl/specs/keep-alive.cddl` |
| PeerSharing Type | `ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/PeerSharing/Type.hs` |
| PeerSharing Codec (Cardano) | `cardano-diffusion/protocols/lib/Cardano/Network/Protocol/PeerSharing/Codec.hs` |
| PeerSharing CDDL | `cardano-diffusion/protocols/cddl/specs/peer-sharing-v14.cddl` |
| LocalStateQuery Type | `ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/LocalStateQuery/Type.hs` |
| LocalStateQuery Codec | `ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/LocalStateQuery/Codec.hs` |
| LocalStateQuery CDDL | `cardano-diffusion/protocols/cddl/specs/local-state-query.cddl` |
| LocalTxSubmission Type | `ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/LocalTxSubmission/Type.hs` |
| LocalTxSubmission Codec | `ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/LocalTxSubmission/Codec.hs` |
| LocalTxSubmission CDDL | `cardano-diffusion/protocols/cddl/specs/local-tx-submission.cddl` |
| LocalTxMonitor Type | `ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/LocalTxMonitor/Type.hs` |
| LocalTxMonitor Codec | `ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/LocalTxMonitor/Codec.hs` |
| LocalTxMonitor CDDL | `cardano-diffusion/protocols/cddl/specs/local-tx-monitor.cddl` |
| Protocol Limits (byte/time constants) | `ouroboros-network/api/lib/Ouroboros/Network/Protocol/Limits.hs` |
| Diffusion Configuration | `cardano-diffusion/lib/Cardano/Network/Diffusion/Configuration.hs` |
| Mux SDU framing | `ouroboros-network/network-mux/src/Network/Mux/Types.hs` |
| HFC era encoding (encodeNS) | `ouroboros-consensus` repo: `src/.../HardFork/Combinator/Serialisation/Common.hs` |
| network.base.cddl | `cardano-diffusion/protocols/cddl/specs/network.base.cddl` |
