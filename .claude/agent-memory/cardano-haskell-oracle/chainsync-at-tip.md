# ChainSync Protocol At-Tip Behavior

## Key Finding: Connection stays open, NO disconnect/reconnect

The ChainSync protocol uses **MsgAwaitReply** to keep the connection open and idle when the server has no more blocks. The client does NOT disconnect and reconnect.

## Protocol State Machine (from Type.hs)

```
StIdle ──MsgRequestNext──→ StNext(StCanAwait)
                              │
                              ├──MsgRollForward──→ StIdle (immediate response)
                              ├──MsgRollBackward──→ StIdle (immediate response)
                              └──MsgAwaitReply──→ StNext(StMustReply)
                                                     │
                                                     ├──MsgRollForward──→ StIdle (after wait)
                                                     └──MsgRollBackward──→ StIdle (after wait)
```

- **StCanAwait**: Server CAN send AwaitReply, RollForward, or RollBackward
- **StMustReply**: Server MUST send RollForward or RollBackward (no more AwaitReply)

## Server Side (ChainSync/Server.hs + Consensus Server)

`handleRequestNext` (Consensus MiniProtocol.ChainSync.Server):
1. Call `followerInstruction` (non-blocking)
2. If `Just update` → return `Left (sendNext ...)` → immediate RollForward/RollBackward
3. If `Nothing` → return `Right (blocking action)` → triggers MsgAwaitReply, then blocks on `followerInstructionBlocking`

The `Right` branch triggers the protocol framework to:
1. Send MsgAwaitReply to the client
2. Block on `followerInstructionBlocking` (uses `blockUntilJust` in STM, retries until chain changes)
3. When a new block arrives in ChainDB, the STM retry fires and the server sends the update

## Client Side Behavior

### Non-Pipelined (ChainSync/Client.hs)
`SendMsgRequestNext` takes two args:
- `m ()` — action run when MsgAwaitReply is received (the "waiting" callback)
- `ClientStNext` — handlers for RollForward/RollBackward

On MsgAwaitReply:
1. Execute the `stAwait` callback
2. Enter a second `Await` state, waiting for the server's eventual RollForward/RollBackward

### Pipelined (ClientPipelined.hs)
For pipelined requests, MsgAwaitReply handling:
1. Execute the `await` callback
2. Enter ReceiverAwait for the follow-up response (MustReply phase)
3. The pipelined receiver automatically handles the two-phase response

### Consensus Client (MiniProtocol.ChainSync.Client)
The `onMsgAwaitReply` callback (line ~1397):
1. Check historicity (Genesis safety check)
2. Call `idlingStart idling` → sets `csIdling = True` in ChainSyncState
3. Pause the LoP (Limits of Patience) bucket
4. Notify CSJ (ChainSync Jumping) governor

When RollForward/RollBackward received after AwaitReply:
1. Call `idlingStop idling` → sets `csIdling = False`
2. Resume LoP bucket
3. Continue normal processing

## Pipeline Decision at Tip

`pipelineDecisionLowHighMark` (PipelineDecision.hs):
- When `n == Zero` and `clientTipBlockNo == serverTipBlockNo` → **Request** (non-pipelined)
- When `n == Zero` and tips differ → **Pipeline**
- When `n > 0` and caught up → **Collect**

This means: when at tip, the client uses non-pipelined `SendMsgRequestNext` (not `SendMsgRequestNextPipelined`). This is the natural "catch-up to at-tip" transition.

Default pipeline marks: lowMark=200, highMark=300

## CBOR Wire Format

| Message | Tag | Encoding |
|---------|-----|----------|
| MsgRequestNext | 0 | [1, 0] |
| MsgAwaitReply | 1 | [1, 1] |
| MsgRollForward | 2 | [3, 2, header, tip] |
| MsgRollBackward | 3 | [3, 3, point, tip] |
| MsgFindIntersect | 4 | [2, 4, [points]] |
| MsgIntersectFound | 5 | [3, 5, point, tip] |
| MsgIntersectNotFound | 6 | [2, 6, tip] |
| MsgDone | 7 | [1, 7] |

## GSM Integration
- `csIdling` flag on ChainSyncState is used by the Genesis State Machine (GSM)
- GSM transitions to `CaughtUp` when peers are idling (among other conditions)
- This affects block production eligibility and peer management

## Key Files
- Protocol types: `ouroboros-network/protocols/lib/.../ChainSync/Type.hs`
- Server: `ouroboros-consensus/.../MiniProtocol/ChainSync/Server.hs`
- Client: `ouroboros-consensus/.../MiniProtocol/ChainSync/Client.hs`
- Pipeline decision: `ouroboros-network/protocols/lib/.../ChainSync/PipelineDecision.hs`
- ChainDB Follower: `ouroboros-consensus/.../Storage/ChainDB/Impl/Follower.hs`
- GSM State: `ouroboros-consensus/.../Node/GsmState.hs`
- ChainSync State: `ouroboros-consensus/.../MiniProtocol/ChainSync/Client/State.hs`
