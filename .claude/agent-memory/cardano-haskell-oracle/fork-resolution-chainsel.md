---
name: fork-resolution-chainsel
description: Complete chain selection and mini-fork resolution algorithm in ouroboros-consensus
type: reference
---

# Fork Resolution / Chain Selection in ouroboros-consensus

## Primary Files

- `ouroboros-consensus/src/ouroboros-consensus/Ouroboros/Consensus/Storage/ChainDB/Impl/ChainSel.hs` — main chain selection logic
- `ouroboros-consensus/src/ouroboros-consensus/Ouroboros/Consensus/Util/AnchoredFragment.hs` — `preferAnchoredCandidate`, `compareAnchoredFragments`
- `ouroboros-consensus/src/ouroboros-consensus/Ouroboros/Consensus/Protocol/Abstract.hs` — `ChainOrder`, `SelectView`, `preferCandidate`
- `ouroboros-consensus-protocol/src/ouroboros-consensus-protocol/Ouroboros/Consensus/Protocol/Praos/Common.hs` — `PraosTiebreakerView`, `comparePraos`
- `ouroboros-consensus/src/ouroboros-consensus/Ouroboros/Consensus/Storage/ChainDB/Impl/Paths.hs` — `isReachable`, `maximalCandidates`, `extendWithSuccessors`
- `ouroboros-consensus/src/ouroboros-consensus/Ouroboros/Consensus/MiniProtocol/ChainSync/Client.hs` — rollback handling
- `ouroboros-consensus/src/ouroboros-consensus/Ouroboros/Consensus/MiniProtocol/ChainSync/Client/State.hs` — `ChainSyncClientHandle`, `ChainSyncClientHandleCollection`
- `ouroboros-consensus/src/ouroboros-consensus/Ouroboros/Consensus/Storage/ChainDB/Impl/Follower.hs` — `TentativeChain` vs `SelectedChain`

## Chain Selection Rule

Primary rule (longest chain wins): `preferAnchoredCandidate` compares `SelectView`:
1. Compare `BlockNo` of tips: candidate must have strictly HIGHER BlockNo to win.
2. On TIE (same BlockNo): delegate to `PraosTiebreakerView`.

Praos tiebreaker (in `comparePraos`):
1. If same slot AND same issuer: prefer higher opcert issue number.
2. If slots differ by ≤ maxDist (RestrictedVRFTiebreaker) or always (UnrestrictedVRFTiebreaker): prefer LOWER VRF output (ptvTieBreakVRF, compared as `Down`).
3. Otherwise: no switch (stay on current chain).

Key property: **ties never cause a switch** — node always stays on current chain unless candidate is strictly preferred.

## addBlock Flow (fork arrival)

1. Block arrives → `addBlockAsync` → enqueued in `cdbChainSelQueue`.
2. Background thread: `chainSelSync` → `chainSelectionForBlock`.
3. Block is first written to VolatileDB (`VolatileDB.putBlock`).
4. `constructPreferableCandidates` builds `ChainDiff`s:
   - If block extends current tip (common case during sync): fast path, just extend.
   - If block is reachable via VolatileDB from current chain anchor (`isReachable`): builds a `ChainDiff(rollback, suffix)`.
   - Otherwise: unreachable (block not connected, ignore).
5. Filter candidates by `preferAnchoredCandidate` — only keep those strictly better than current.
6. Trim candidates to LoE (Limit on Eagerness) fragment if Genesis mode.
7. `chainSelection`: sort all candidates by `compareChainDiffs` (best first), validate each.

## Candidate Fragment Storage

- **One fragment per peer**: each ChainSync client maintains a `theirFrag :: AnchoredFragment (HeaderWithTime blk)` in `KnownIntersectionState`.
- Published via `setCandidate` into `ChainSyncClientHandle.cschState.csCandidate` (a `StrictTVar`).
- All handles collected in `ChainSyncClientHandleCollection` (both Map and Seq views, kept in sync).
- BlockFetch decision logic reads all candidates from this collection.
- ChainDB does NOT store candidates — it recomputes from VolatileDB successor map on demand.

## MsgRollBackward Handling (ChainSync client)

In `rollBackward`:
1. Calls `attemptRollback rollBackPoint (theirFrag, theirHeaderStateHistory)`.
2. `AF.rollback` truncates `theirFrag` to the given rollback point.
3. `HeaderStateHistory.rewind` rolls back the header state history.
4. If rollback point is NOT on the fragment (rolled back past k blocks from anchor): **disconnect** (`RolledBackPastIntersection` error).
5. Updates `mostRecentIntersection` if rollback point is on `ourFrag`.
6. Writes updated `theirFrag'` to `csCandidate` via `setCandidate`.
7. The ChainSync client DOES NOT trigger chain selection itself — that happens when downloaded blocks arrive via BlockFetch.

## Block Validation During Fork Resolution

`validateCandidate` in ChainSel.hs:
1. Calls `LedgerDB.validateFork rollback neHeaders onSuccess`.
2. Rolls back ledger state `rollback` blocks, then applies new headers one by one.
3. On `ValidateSuccessful`: `FullyValid` result.
4. On `ValidateLedgerError`: block is added to `cdbInvalid` map, returns `ValidPrefix` (truncated at last valid block).
5. The valid prefix may still be preferred and triggers a chain switch.
6. Block validation happens on HEADERS only (full blocks were already downloaded by BlockFetch). Validation includes: Praos VRF check, KES signature, opcert, ledger rules.

## Tentative Follower / Pipelining

- `cdbTentativeHeader :: StrictTVar m (StrictMaybe (Header blk))` — the "tentative" next block header.
- Used for diffusion pipelining: the next expected block header is announced to ChainSync servers before the block body is validated.
- `TentativeChain` followers see `curChain :> tentativeHeader`.
- `SelectedChain` followers see only validated, committed blocks.
- When a tentative block body turns out invalid: `clearTentativeHeader` resets `cdbTentativeHeader` to `SNothing`, rolls back tentative followers via `fhSwitchFork`.

## Key Invariants

- Current chain fragment `cdbChain` is at most `k` blocks long (immutable tip is the anchor).
- VolatileDB stores all non-finalized blocks; successor map is the data structure for candidate construction.
- Blocks older than the immutable tip are silently ignored (`olderThanImmTip`).
- Invalid blocks cached in `cdbInvalid` (fingerprinted map) — subsequent arrivals of same block immediately ignored.
- `switchTo` atomically updates `cdbChain` TVar, commits the LedgerDB forker, notifies all followers via `fhSwitchFork`.
