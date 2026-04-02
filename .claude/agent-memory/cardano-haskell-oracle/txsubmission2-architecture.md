---
name: TxSubmission2 Architecture Deep-Dive
description: Complete Haskell TxSubmission2 implementation details - connection model, governor lifecycle, V1/V2 inbound, outbound, mempool sync, peer temperatures, decision logic
type: reference
---

# TxSubmission2 Architecture (Haskell Reference)

## Connection Model

### One Connection Per Peer, Bidirectional
- Haskell uses `InitiatorAndResponderMode` for P2P connections
- Single TCP connection per peer, multiplexed via network-mux
- Both initiator AND responder mini-protocols run on the SAME connection
- Each peer gets its own independent TxSubmission2 session (both directions)
- NOT one shared session — each direction is independent

### MuxMode: InitiatorAndResponder
- `initiatorAndResponder` function in `Ouroboros.Consensus.Network.NodeToNode`
- Wraps each mini-protocol with `InitiatorAndResponderProtocol`
- Both `MiniProtocolCb` for initiator context and responder context are provided
- This means: on any single connection, BOTH client AND server run for TxSubmission2

### Mini-Protocol Temperature Classification
- **Hot protocols** (started on warm→hot promotion): ChainSync, BlockFetch, **TxSubmission2**
- **Established protocols** (started on cold→warm): KeepAlive, PeerSharing
- **Warm protocols**: currently empty (reserved for tip-sample)
- TxSubmission2 is a **HOT** protocol — only runs when peer is in "hot" (active) state

### Key Constants
- Mini-protocol ID: 4 (txSubmissionMiniProtocolNum)
- `miniProtocolStart = StartOnDemand` (not StartEagerly)
- Mini-protocols only start when the governor promotes peer to hot

## Peer Lifecycle & TxSubmission2

### Governor State Machine: cold → warm → hot
1. **Cold→Warm** (`establishPeerConnection`): Opens TCP connection, runs handshake, starts KeepAlive
2. **Warm→Hot** (`activatePeerConnection`): Starts HOT protocols (ChainSync, BlockFetch, TxSubmission2)
3. **Hot→Warm** (`deactivatePeerConnection`): Terminates HOT protocols, keeps connection alive
4. **Warm→Cold** (`closePeerConnection`): Closes entire connection

### When TxSubmission2 Starts
- Only when governor promotes peer to hot (active)
- NOT immediately upon connection establishment
- Governor decides based on target counts (e.g., `targetNumberOfActivePeers`)

### When TxSubmission2 Ends
- Governor demotes peer from hot→warm: HOT protocols terminated
- Connection error/timeout: entire connection dies
- Protocol violation (exception thrown): terminates the mini-protocol thread
- If a single mini-protocol dies, the MUX terminates the entire connection

### Reconnection
- Governor manages reconnection with backoff
- After disconnection, peer goes cold → must re-promote through warm → hot
- New TxSubmission2 sessions start fresh (no state carried over)

## Roles: Client vs Server

### Outbound Connection (WE initiated)
- We run as **Initiator** for all mini-protocols
- For TxSubmission2 specifically:
  - Our **Client** (Outbound/Initiator) = `txSubmissionOutbound` — we advertise OUR mempool
  - Our **Server** (Inbound/Responder, via InitiatorAndResponder) = `txSubmissionInboundV2` — we pull THEIR mempool

### Inbound Connection (THEY initiated)
- We run as **Responder** for all mini-protocols
- For TxSubmission2 specifically:
  - Our **Server** (Inbound/Responder) = `txSubmissionInboundV2` — we pull THEIR mempool
  - Our **Client** (via InitiatorAndResponder) = `txSubmissionOutbound` — we advertise OUR mempool

### Summary
- On EVERY connection (regardless of who initiated), BOTH directions run
- Client role = outbound = advertise mempool (respond to requests)
- Server role = inbound = pull transactions (make requests)

## V1 Inbound Server (Legacy)

### Key Constants
- `maxTxIdsToRequest = 3` (requests 3 txids at a time)
- `maxTxToRequest = 2` (requests 2 tx bodies at a time)
- `maxUnacked` = from TxDecisionPolicy (default 100)

### Init Delay
- `defaultTxSubmissionInitDelay = TxSubmissionInitDelay 60` (60 seconds!)
- Applied via `threadDelay delay` before any protocol processing
- Purpose: avoid requesting txs during initial sync

### Protocol Flow (V1)
1. Wait for init delay (60 seconds)
2. Enter `serverIdle` — if no available txids, send blocking MsgRequestTxIds
3. When txids arrive, filter already-in-mempool ones
4. Send MsgRequestTxs for up to 2 tx bodies
5. Pipeline: request more txids while collecting tx replies
6. Add received txs to mempool in FIFO order

## V2 Inbound Server (Current)

### Architecture
- Central `decisionLogicThread` runs every 5ms (`_DECISION_LOOP_DELAY = 0.005`)
- `drainRejectionThread` runs every 1s, drains score every 7s
- Each peer has `PeerTxState`; shared `SharedTxState` across all peers
- Communication via `TxChannels` (MVar per peer)

### Decision Logic
- `makeDecisions` called on active peers every 5ms
- `filterActivePeers` determines which peers can download or acknowledge
- `pickTxsToDownload` distributes txids to download among peers
- Peers ordered by rejection score (less useful peers sorted last)

### Per-Peer State (PeerTxState)
- `unacknowledgedTxIds`: FIFO of txids not yet acked
- `availableTxIds`: txids we can request (Map txid SizeInBytes)
- `requestedTxIdsInflight`: count of requested-but-not-replied txids
- `requestedTxsInflightSize`: bytes in-flight
- `requestedTxsInflight`: Set of requested txid
- `unknownTxs`: txids requested but not received
- `downloadedTxs`: txs downloaded but not yet sent to mempool
- `score` / `scoreTs`: reputation scoring

### Shared State (SharedTxState)
- `peerTxStates`: Map peeraddr (PeerTxState txid tx)
- `inflightTxs`: Map txid Int (multiplicities)
- `bufferedTxs`: downloaded txs (or Nothing for already-in-mempool)
- `referenceCounts`: reference counting for lifecycle management
- `timedTxs`: TTL for buffered txs
- `inSubmissionToMempoolTxs`: txs being submitted to mempool

### withPeer Bracket
- Registers peer in SharedTxState on entry
- Unregisters + cleans up reference counts on exit
- Provides `PeerTxAPI`: readTxDecision, handleReceivedTxIds, handleReceivedTxs, submitTxToMempool

### Mempool Submission
- Uses `TxMempoolSem` (TSem with initial count 1) — serializes mempool access
- Only one tx submitted to mempool at a time across all peers
- `submitTxToMempool` acquires semaphore, calls mempoolAddTxs, updates shared state

## Outbound Client (txSubmissionOutbound)

### Behavior
- Maintains `unackedSeq` of (txid, idx) pairs
- `maxUnacked` = from TxDecisionPolicy (default 100 via maxUnacknowledgedTxIds)
- Uses `TxSubmissionMempoolReader` to get mempool snapshots
- `mempoolGetSnapshot` returns `MempoolSnapshot` with `mempoolTxIdsAfter`, `mempoolLookupTx`, `mempoolHasTx`

### Protocol Handling
- Blocking request: blocks in STM until new txs appear in mempool (with `check`)
- Non-blocking request: immediately returns available txs from snapshot
- Uses `ControlMessageSTM` for termination signal (governor demotion)
- When `timeoutWithControlMessage` returns Nothing → sends MsgDone, session ends

## Default TxDecisionPolicy (V2)

```
maxNumTxIdsToRequest   = 12
maxUnacknowledgedTxIds = 100
txsSizeInflightPerPeer = 6 * max_TX_SIZE (6 * 65540 = 393240 bytes)
txInflightMultiplicity = 2  (download from 2 peers simultaneously)
bufferedTxsMinLifetime = some default (time before buffered tx can expire)
scoreRate / scoreMax   = rejection recovery window
```

## Mempool Sync with Ledger

### implSyncWithLedger (consensus Mempool)
- Called by background thread whenever ChainDB tip changes
- Revalidates all mempool txs against new ledger state
- Invalid txs (e.g., inputs consumed by new block) are removed
- NOT explicit removal on block arrival — revalidation-based
- STM atomic: reads tip, if changed, revalidates and updates

### When Txs Leave Mempool
1. Block arrives that includes the tx → next revalidation removes it
2. Tx becomes invalid due to ledger state change → revalidation removes it
3. Explicit `removeTxsEvenIfValid` (rare, used by forging thread)
4. Txs are NOT removed when served to a peer

## Connection Stability

### What Causes Disconnection
- Protocol errors (throw exceptions in mini-protocol thread → MUX kills connection)
- Ingress queue overflow (maximumIngressQueue exceeded)
- Timeout violations (timeLimitsTxSubmission2)
- Governor demotion (controlMessage → sends MsgDone → clean shutdown)
- KeepAlive timeout (separate protocol, but kills whole connection)

### Expected TxSubmission2 Lifetime
- Indefinite while peer remains "hot"
- Client blocks in STM when mempool is empty (blocking MsgRequestTxIds)
- Server blocks waiting for decisions from decision logic thread
- Session ends cleanly when governor demotes peer (MsgDone sent)

### Auto-Reconnection
- Governor handles: after disconnect, peer goes to cold
- Churn mechanism may re-establish and re-promote
- TxSubmission2 starts fresh on new connection

## Key Differences from Dugite

### Dugite Issues Identified
1. **No governor**: Dugite doesn't have cold/warm/hot peer states
2. **No central decision logic**: Each peer session is independent (no SharedTxState)
3. **MAX_TX_IDS_REQUEST = 100**: Haskell V1 uses 3, V2 uses 12
4. **No init delay**: Haskell defaults to 60s delay before starting server
5. **Independent connections**: Dugite may create separate connections instead of reusing single duplex
6. **No ControlMessage**: No clean shutdown signal from governor
