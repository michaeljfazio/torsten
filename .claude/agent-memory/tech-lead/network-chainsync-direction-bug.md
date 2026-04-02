---
name: N2N ChainSync server direction bug
description: Root cause of Haskell cardano-node ChainSync terminating in <2ms — server never sends MsgInit to TxSubmission2, and the direction-unaware segment dispatch routes Haskell's responder-dir segments to wrong handlers
type: project
---

# N2N ChainSync/BlockFetch immediate termination from Haskell node

## Symptom
Haskell cardano-node connects to dugite, handshakes V15 InitiatorAndResponder, promotes to Hot,
but ChainSync exits in <2ms (`PeerHotDuration 0.001504s`), BlockFetch client also terminates
immediately (`BlockFetch.Client.ClientTerminating`).

## Root Cause (two-part)

### 1. Segment direction not checked — server dispatches by protocol_id only

`process_n2n_segment` (n2n_server.rs:748) matches solely on `segment.protocol_id`, never on
`segment.is_responder`.

In an N2N InitiatorAndResponder connection the Haskell node runs BOTH sides of several protocols:
- Haskell's ChainSync **Client** (initiator role) sends on protocol_id=2, `is_responder=false`
- Haskell's ChainSync **Server** (responder role) expects us to send on protocol_id=2, `is_responder=true`
- Haskell's BlockFetch **Client** (initiator) sends on protocol_id=3, `is_responder=false`
- Haskell's BlockFetch **Server** (responder) expects us to send on protocol_id=3, `is_responder=true`
- Haskell's TxSubmission2 **Server** (initiator role per TxSub2 protocol) sends MsgRequestTxIds on
  protocol_id=4, `is_responder=false` (because in TxSub2 the node-as-submitter is the initiator)

Our server treats ALL incoming segments on protocol_id=2 as client ChainSync requests.
This is fine for the inbound ChainSync direction (Haskell→us), but we never initiate
our own TxSubmission2 flow without waiting for MsgInit — which the Haskell node may not send
if it considers itself the server.

### 2. TxSubmission2 role inversion in InitiatorAndResponder mode

In N2N TxSubmission2:
- The **submitter** (client) is the initiator: sends MsgInit, then *Server* has agency
- In InitiatorAndResponder mode, each peer is BOTH submitter and receiver
- The Haskell node's TxSub2 **Server** (our side, receiving txs from Haskell) sends
  MsgRequestTxIds first — it does NOT send MsgInit first
- Our code waits for MsgInit (tag=6) to arrive on protocol_id=4 before responding

If the Haskell node's TxSub2 Server sends MsgRequestTxIds on protocol_id=4 with `is_responder=true`
(because it is the responder/server in that sub-protocol direction), and our server only sees
`protocol_id=4, is_responder=false` for the *other* direction's MsgInit — the roles are confused.

## The Critical Protocol Violation

After MsgAcceptVersion with `initiatorOnlyDiffusionMode=false`, the Haskell node expects dugite
to behave as a FULL peer — meaning dugite must proactively start sending on the protocols where
dugite is the initiator:
- TxSubmission2: dugite is the Server (receives txs); Haskell is the Client (sends txs)
  → Haskell waits for dugite to send MsgRequestTxIds immediately after handshake
  → dugite waits for Haskell to send MsgInit
  → DEADLOCK → timeout → Haskell disconnects

The <2ms clean termination is the Haskell node's `idleTimeout` or the BlockFetch client
noticing no data flows and giving up.

## What to Fix (do not implement without user direction)

1. In `handle_n2n_connection`, after a successful handshake, proactively send
   MsgRequestTxIds on TxSubmission2 (protocol_id=4, is_responder=true) WITHOUT waiting
   for MsgInit from the Haskell side.
2. Add direction-aware dispatch: segments arriving with `is_responder=true` are from
   the Haskell peer's server-side protocols (TxSub2 server → our consumer) and should
   be routed to our TxSub2 client-side handler, not re-dispatched to the server handler.
3. The current `handle_n2n_txsubmission` with tag=6 MsgInit handling is correct for
   connections where dugite is the *receiver* of txs (dugite's TxSub2 server).
   But in InitiatorAndResponder mode, dugite also runs as a TxSub2 *client* (submitter),
   sending txs to the Haskell peer — that path is entirely missing.

**Why:** Cardano N2N InitiatorAndResponder requires full bidirectional mini-protocol
operation. The current server only handles one direction per protocol, causing the
Haskell node to time out waiting for messages that dugite never sends.
