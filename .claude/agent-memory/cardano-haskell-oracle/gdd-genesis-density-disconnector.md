---
name: GDD Genesis Density Disconnector
description: Complete algorithm for the Genesis Density Disconnector (GDD) — density comparison, genesis window, LoE interaction, threading, disconnection criterion
type: reference
---

# Genesis Density Disconnector (GDD)

## Source Files

- **Main GDD logic**: `ouroboros-consensus/src/ouroboros-consensus/Ouroboros/Consensus/Genesis/Governor.hs`
- **Genesis window formula**: `cardano-ledger/eras/shelley/impl/src/Cardano/Ledger/Shelley/StabilityWindow.hs`
- **EraParams (eraGenesisWin field)**: `ouroboros-consensus/src/ouroboros-consensus/Ouroboros/Consensus/HardFork/History/EraParams.hs`
- **Shelley EraParams construction**: `ouroboros-consensus-cardano/src/shelley/Ouroboros/Consensus/Shelley/Ledger/Ledger.hs` — `shelleyEraParams` function
- **HardFork query** `slotToGenesisWindow`: `ouroboros-consensus/src/ouroboros-consensus/Ouroboros/Consensus/HardFork/History/Qry.hs`
- **NodeKernel GDD thread start**: `ouroboros-consensus-diffusion/src/ouroboros-consensus-diffusion/Ouroboros/Consensus/NodeKernel.hs`
- **Genesis config / LoEAndGDD config**: `ouroboros-consensus-diffusion/src/ouroboros-consensus-diffusion/Ouroboros/Consensus/Node/Genesis.hs`
- **ChainSyncState (csIdling, csLatestSlot)**: `ouroboros-consensus/src/ouroboros-consensus/Ouroboros/Consensus/MiniProtocol/ChainSync/Client/State.hs`
- **LoE ChainSel enforcement**: `ouroboros-consensus/src/ouroboros-consensus/Ouroboros/Consensus/Storage/ChainDB/Impl/ChainSel.hs`

## 1. Genesis Window Definition

The genesis window `sgen` is `ceiling(3k/f)` slots (NOT `4k/f`):

```haskell
-- cardano-ledger/eras/shelley/impl/src/Cardano/Ledger/Shelley/StabilityWindow.hs
computeStabilityWindow :: Word64 -> ActiveSlotCoeff -> Word64
computeStabilityWindow k asc =
  ceiling $ (3 * fromIntegral k) /. f
  where
    f = positiveUnitIntervalNonZeroRational . activeSlotVal $ asc
```

This is stored as `eraGenesisWin = GenesisWindow stabilityWindow` in `EraParams`.

NOTE: `4k/f` is the RANDOMNESS STABILISATION window (`computeRandomnessStabilisationWindow`). The GDD genesis window uses `3k/f`.

The default for tests uses `2k` (no `f` factor) — only real Cardano chains use `3k/f`.

### Per-era lookup

At runtime, the genesis window is looked up from the EraParams via HardFork.History query:
```haskell
slot = succWithOrigin $ AF.headSlot loeFrag   -- slot AFTER LoE tip
qry = qryFromExpr $ slotToGenesisWindow slot
summary = hardForkSummary (configLedger cfg) (ledgerState immutableLedgerSt)
msgen = eitherToMaybe $ runQuery qry summary
```
The query uses the slot immediately AFTER the LoE intersection, so when the LoE is at the last slot of an era, it correctly uses the genesis window of the NEXT era.

On mainnet with k=2160 and f=0.05: `ceiling(3 * 2160 / 0.05) = ceiling(129600) = 129600` slots.

## 2. GDD Threading Model

GDD runs in a **background thread** named `"NodeKernel.GDD"`, spawned via `forkLinkedWatcher` which calls `runWatcher`:

```haskell
-- NodeKernel.hs
forkLinkedWatcher registry "NodeKernel.GDD" $
  gddWatcher cfg (gddTracer tracers) chainDB
    (lgnkaGDDRateLimit lgArgs)  -- default 1.0 second
    (readTVar varGsmState)
    (cschcMap varChainSyncHandles)
    varLoEFragment
```

The `Watcher` pattern:
1. STM `blockUntilChanged` on the fingerprint (map of `(csLatestSlot, csIdling)` per peer)
2. Wakes up when ANY peer's `csLatestSlot` or `csIdling` changes
3. Also wakes up on GSM state transitions (PreSyncing/Syncing/CaughtUp)

**Rate limit**: After each GDD evaluation, a `threadDelay (rateLimit - elapsed)` is applied. Default is **1.0 second** (`defaultGDDRateLimit = 1.0`). So GDD is evaluated at most once per second.

GDD is triggered by:
- Any change to `csLatestSlot` of any peer (receives a new header)
- Any change to `csIdling` (peer sends MsgAwaitReply or starts sending again)
- GSM state change (PreSyncing/Syncing/CaughtUp)

## 3. GDD State Machine (GSM interaction)

```
PreSyncing  → GDD does NOTHING (LoE advancing doesn't help here)
Syncing     → GDD runs density comparison, updates LoE fragment, may disconnect peers
CaughtUp    → GDD does NOT run density comparison; just triggers chain selection
              (to process any LoE-postponed blocks that can now be adopted)
```

## 4. LoE Fragment Computation

`sharedCandidatePrefix` computes:
- `loeFrag`: fragment from immutable tip to the **earliest** intersection of curChain with ALL candidate fragments
- `candidateSuffixes`: for each peer, the fragment AFTER the LoE intersection

The LoE tip = intersection of all candidate fragments with the current chain. ChainSel is constrained to:
- Be on the same chain as the LoE tip
- Not extend more than `k` blocks beyond the LoE tip

## 5. Density Comparison Algorithm (`densityDisconnect`)

Function signature:
```haskell
densityDisconnect ::
  GenesisWindow ->
  SecurityParam ->
  Map peer (ChainSyncState blk) ->
  [(peer, AnchoredFragment (HeaderWithTime blk))] ->
  AnchoredFragment (HeaderWithTime blk) ->
  ([peer], [(peer, DensityBounds blk)])
```

### Per-peer bounds computation

For each peer, the genesis window starts at:
```haskell
firstSlotAfterGenesisWindow = succWithOrigin loeIntersectionSlot + SlotNo sgen
```
(i.e., the LoE intersection slot + `sgen` slots)

The candidate suffix is clipped to the genesis window:
```haskell
(clippedFragment, _) = AF.splitAtSlot firstSlotAfterGenesisWindow candidateSuffix
```

Then:
```haskell
-- Is the peer known to have a block AT OR AFTER the end of the genesis window?
hasBlockAfter =
  max (AF.headSlot candidateSuffix) latestSlot
    >= NotOrigin firstSlotAfterGenesisWindow

-- Trailing slots in genesis window not yet observed from this peer
potentialSlots =
  if hasBlockAfter
    then 0
    else unknownTrailingSlots

unknownTrailingSlots =
  firstSlotAfterGenesisWindow - succWithOrigin (AF.headSlot clippedFragment)

-- Definite count of blocks in genesis window
lowerBound = AF.length clippedFragment

-- Maximum possible density (optimistic)
upperBound = lowerBound + potentialSlots

-- Does this peer offer MORE than k blocks total after the intersection?
offersMoreThanK = AF.length candidateSuffix > k
```

**Arithmetic is exact integer arithmetic** — no floating point.

### Disconnection criterion (losingPeers)

Peer0 is disconnected if there EXISTS a peer1 such that ALL of:

1. `idling0 || not (AF.null frag0) || hasBlockAfter0`
   - peer0 must have declared something: either idling, sent ≥1 header, or has a block beyond the window.
   - Guards against disconnecting a peer that hasn't said anything yet (Note [Chain disagreement]).

2. `AF.lastPoint frag0 /= AF.lastPoint frag1`
   - The two peers disagree (their chains diverge after the LoE).

3. `offersMoreThanK || lb0 == ub0`
   - Peer1 offers more than k blocks total (it's a credible alternative), OR
   - Peer0's density is fully known (no trailing uncertainty: lowerBound == upperBound).
   - This avoids disconnecting honest peers near the tip when the node is almost caught up.

4. `lb1 >= (if idling0 then lb0 else ub0)`
   - If peer0 is idling: peer1's density ≥ peer0's density (lower bound comparison).
   - If peer0 is NOT idling: peer1's density ≥ peer0's UPPER bound (peer1 beats even peer0's best case).

**All comparisons are Word64 integer arithmetic** — no floating point anywhere.

### The "equal density" case

Note from the code: "Having the same density is enough to disconnect peer0, as the honest chain is expected to have a strictly higher density than all of the other chains. This matters to ChainSync jumping, where adversarial dynamo and objector could offer chains of equal density."

So `lb1 >= lb0` (not `lb1 > lb0`) for the idling case.

## 6. Note [Chain disagreement]

```
k: 1, sgen: 2
  0 1 2
G---1-2
```

If peer1 sent no headers yet and peer2 sent 2 headers:
- Both intersection at G
- peer2 density = 2; peer1 upper bound = 2

GDD defers disconnecting peer1 until it declares to have no more headers (idling=true), sends one header after intersection, or has a block beyond the genesis window. This prevents disconnecting an honest peer that happens to be slow to start sending.

## 7. LoE ChainSel Enforcement

In ChainSel (`chainSelSync`), when LoE is enabled:
```haskell
-- Trim candidate fragment to LoE:
-- - if candidate contains LoE tip: keep at most k blocks after LoE tip
-- - if candidate doesn't reach LoE tip: trim to LoE prefix
trimToLoE (LoEEnabled loe) diff = ...
  candSuffix trimmed to k blocks beyond LoE tip
```

This means even without GDD disconnecting anyone, the chain selection cannot advance more than k blocks beyond the LoE tip.

## 8. GDD Relaxation (No explicit "relaxation")

There is NO explicit "GDD relaxation" or "sufficient density" concept in the code. Instead:
- When GSM state = `CaughtUp`, GDD simply does NOT run; LoE is effectively disabled (`LoEDisabled` returned from `getLoEFragment` in `CaughtUp` state)
- GDD only runs in `Syncing` state
- When GDD disconnects peers, the LoE fragment naturally advances (fewer competing chains = later intersection)
- The `offersMoreThanK` guard serves as the "sufficient density" threshold: if no peer has offered more than k blocks, we don't yet have enough information to judge density

## 9. Configuration Parameters

From `defaultGenesisConfigFlags`:
- `gcfEnableLoEAndGDD = True` (default enabled)
- `gcfGDDRateLimit = Nothing` → defaults to `1.0` second
- `gcfEnableLoP = True` (Limit on Pipelining enabled)
- `gcfEnableCSJ = True` (ChainSync Jumping enabled)

The `HistoricityCutoff` is set to `3 * 2160 * 20 + 3600` seconds (~`3k/f` stability window for mainnet Shelley + 1hr safety margin). Headers older than this are rejected.

## 10. Full Data Flow

```
ChainSync client receives MsgRollForward(header)
  → updates csLatestSlot (BEFORE adding to candidate fragment)
  → STM fingerprint changes
  → Watcher unblocks in GDD thread (after STM commit)

GDD thread wakes up:
  1. Read GsmState (must be Syncing)
  2. Read all candidate fragments + csLatestSlot + csIdling
  3. Compute loeFrag = sharedCandidatePrefix
  4. Look up genesis window from HardFork history (at slot after LoE tip)
  5. densityDisconnect → compute losingPeers
  6. Kill losing peers via cschGDDKill
  7. Write new loeFrag to varLoEFrag TVar
  8. If LoE tip changed: triggerChainSelectionAsync
  9. Sleep up to 1 second (rate limit)

ChainSel:
  1. Reads loeFrag TVar
  2. Trims all candidates to k blocks beyond LoE tip
  3. Selects best valid candidate within LoE constraint
```
