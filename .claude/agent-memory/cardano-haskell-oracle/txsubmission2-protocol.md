---
name: TxSubmission2 Protocol Roles and Wire Format
description: Complete analysis of TxSubmission2 roles (client=outbound=serves mempool, server=inbound=pulls mempool), MsgInit flow, direction bits, init delay, and the critical role inversion bug in Torsten
type: reference
---

## TxSubmission2 Protocol Roles (COUNTERINTUITIVE)

The protocol uses "client" and "server" in a COUNTERINTUITIVE way:

- **Client** (outbound, typed-protocol initiator) = the one that **SERVES** mempool transactions
  - Sends MsgInit as first message
  - RECEIVES MsgRequestTxIds from server, REPLIES with MsgReplyTxIds
  - RECEIVES MsgRequestTxs from server, REPLIES with MsgReplyTxs
  - Can send MsgDone during a blocking MsgRequestTxIds to terminate
  - Haskell: `txSubmissionOutbound` in `Ouroboros.Network.TxSubmission.Outbound`
  - Wired as `hTxSubmissionClient` / `aTxSubmission2Client` (initiator callback)

- **Server** (inbound, typed-protocol responder) = the one that **PULLS** transactions from the remote
  - RECEIVES MsgInit from client (waits for it)
  - SENDS MsgRequestTxIds (blocking or non-blocking), RECEIVES MsgReplyTxIds
  - SENDS MsgRequestTxs, RECEIVES MsgReplyTxs
  - Adds received transactions to local mempool
  - Haskell: `txSubmissionInboundV2` in `Ouroboros.Network.TxSubmission.Inbound.V2`
  - Wired as `hTxSubmissionServer` / `aTxSubmission2Server` (responder callback)

## State Agency (from Type.hs)

```
StInit     = ClientAgency   -- Client sends MsgInit
StIdle     = ServerAgency   -- Server sends MsgRequestTxIds or MsgRequestTxs
StTxIds b  = ClientAgency   -- Client sends MsgReplyTxIds or MsgDone
StTxs      = ClientAgency   -- Client sends MsgReplyTxs
StDone     = NobodyAgency
```

## MsgInit: NOT Bidirectional

- Only the Client (initiator/outbound) sends MsgInit
- The Server (responder/inbound) awaits MsgInit, does NOT send one back
- There is exactly ONE MsgInit per protocol direction on a duplex connection

## Duplex Connections (InitiatorAndResponderMode)

On a single TCP connection, BOTH roles run simultaneously:
- subscribe_client(4): runs as Client — serves OUR mempool to the remote
- subscribe_server(4): runs as Server — pulls THEIR mempool into ours

## Multiplexer Direction Bits (protocol ID 4)

- Initiator/Client sends with bit-15 = 0 (raw protocol num = 4)
- Responder/Server sends with bit-15 = 1 (protocol num | 0x8000 = 0x8004)
- Demuxer FLIPS direction: data with bit-15=0 → dispatched to local responder; bit-15=1 → local initiator
- pallas: subscribe_client(P) sends on P, receives on P|0x8000 (CORRECT mapping)

## Init Delay (60 seconds default)

- `defaultTxSubmissionInitDelay = TxSubmissionInitDelay 60` (seconds)
- Applied by the Server (responder/inbound) at the very start, before reading MsgInit
- Production nodes sleep 60s before processing any TxSubmission2 on each connection
- Your timeout for waiting for the first MsgRequestTxIds from a Haskell peer must be > 60s

## Wire Format (CBOR tags)

```
MsgInit          = [6]
MsgRequestTxIds  = [0, bool(blocking), u16(ack_count), u16(req_count)]
MsgReplyTxIds    = [1, indef_array[[txid, u32(size)], ...]]  -- indef array!
MsgRequestTxs    = [2, indef_array[txid, ...]]               -- indef array!
MsgReplyTxs      = [3, indef_array[tx_cbor, ...]]            -- indef array!
MsgDone          = [4]
```

Note: Haskell uses indefinite-length arrays (encodeListLenIndef + encodeBreak) for the list payloads.

## Time Limits

| State                     | Timeout       |
|---------------------------|---------------|
| StInit                    | waitForever   |
| StIdle                    | waitForever   |
| StTxIds StBlocking        | waitForever   |
| StTxIds StNonBlocking     | shortWait     |
| StTxs                     | shortWait     |

## Torsten Bug (Confirmed 2026-03-20)

Both `serve_tx_submission` and `pull_tx_submission` in duplex.rs have their roles INVERTED:
- The responder (subscribe_server) acts as the Client (waits for MsgRequestTxIds, replies)
- The initiator (subscribe_client) acts as the Server (sends MsgRequestTxIds, receives)
- Additionally, both sides exchange MsgInit bidirectionally (wrong — only Client sends MsgInit)
- TXSUB_INIT_TIMEOUT is 30s, which is too short given 60s Haskell default init delay

## Key Files

- Protocol types: `ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/TxSubmission2/Type.hs`
- Client (outbound): `ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/TxSubmission2/Client.hs`
- Server (inbound): `ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/TxSubmission2/Server.hs`
- Codec (CBOR): `ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/TxSubmission2/Codec.hs`
- txSubmissionOutbound: `ouroboros-network/lib/Ouroboros/Network/TxSubmission/Outbound.hs`
- txSubmissionInboundV2: `ouroboros-network/lib/Ouroboros/Network/TxSubmission/Inbound/V2.hs`
- Wiring: `ouroboros-consensus-diffusion/.../Ouroboros/Consensus/Network/NodeToNode.hs`
- Mux codec (direction bit): `network-mux/src/Network/Mux/Codec.hs`
- Mux demuxer (direction flip): `network-mux/src/Network/Mux/Ingress.hs`
