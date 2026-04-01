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
1. Compare `BlockNo` of tips: candidate must have strictly HIGHER BlockNo to win (returns `ShouldSwitch(Longer)`).
2. On TIE (same BlockNo): delegate to `PraosTiebreakerView` via `comparePraos`.
3. If candidate is SHORTER: `ShouldNotSwitch GT` — stay on current chain.

Key property: **ties NEVER cause a switch** — `ShouldNotSwitch` is returned on equality. The incumbent always wins on a tie.

## Praos Tiebreaker (`comparePraos`) — Exact Logic

Source: `ouroboros-consensus-protocol/.../Ouroboros/Consensus/Protocol/Praos/Common.hs`

`PraosTiebreakerView` fields: `ptvSlotNo`, `ptvIssuer` (VKey), `ptvIssueNo` (Word64 = opcert counter), `ptvTieBreakVRF`.

`pTieBreakVRFValue` = `certifiedOutput . hbVrfRes . headerBody` — the raw OutputVRF (NOT range-extended, NOT hashed). This is the single `hbVrfRes` VRF certified output from the header.

`pHeaderIssueNo` = `SL.ocertN . hbOCert . headerBody` — the OCert counter.

`comparePraos tiebreakerFlavor ours cand` uses a 4-tuple match:
```
(issueNoArmed, compare(issueNo), vrfArmed, compare(Down . ptvTieBreakVRF))
```

- `issueNoArmed` = `ptvSlotNo ours == ptvSlotNo cand AND ptvIssuer ours == ptvIssuer cand`
  (same slot AND same issuer pool — implies VRFs are also identical since VRF is deterministic)
- `vrfArmed`:
  - `UnrestrictedVRFTiebreaker` (pre-Conway eras): always True
  - `RestrictedVRFTiebreaker maxDist` (Conway): True only if `|ptvSlotNo ours - ptvSlotNo cand| <= maxDist`

Decision table:
```
(issueNoArmed=T, issueNo: LT) → ShouldSwitch(HigherOCert)    -- candidate has higher opcert counter
(issueNoArmed=T, issueNo: GT) → ShouldNotSwitch GT            -- ours has higher opcert counter
(issueNoArmed=T, issueNo: EQ, vrfArmed=T, VRF: LT) → ShouldSwitch(VRFTiebreak) -- cand has LOWER VRF (better)
(issueNoArmed=T, issueNo: EQ, vrfArmed=T, VRF: GT) → ShouldNotSwitch GT
(issueNoArmed=T, issueNo: EQ, vrfArmed=T, VRF: EQ) → ShouldNotSwitch EQ
(issueNoArmed=T, issueNo: EQ, vrfArmed=F)           → ShouldNotSwitch EQ  -- no tiebreaker armed
(issueNoArmed=F, vrfArmed=T,  VRF: LT) → ShouldSwitch(VRFTiebreak)        -- different issuer/slot, cand lower
(issueNoArmed=F, vrfArmed=T,  VRF: GT) → ShouldNotSwitch GT
(issueNoArmed=F, vrfArmed=T,  VRF: EQ) → ShouldNotSwitch EQ
(issueNoArmed=F, vrfArmed=F)            → ShouldNotSwitch EQ  -- Conway + slots far apart: no tiebreaker
```

**Lower VRF value wins** (`Down . ptvTieBreakVRF` so LT in Down ordering = GT in raw = candidate has lower raw value).

## VRFTiebreakerFlavor per Era

Source: `ouroboros-consensus-cardano/.../Ouroboros/Consensus/Shelley/Ledger/Config.hs`

```haskell
shelleyVRFTiebreakerFlavor
  | isBeforeConway (Proxy @era) = UnrestrictedVRFTiebreaker
  | otherwise                   = RestrictedVRFTiebreaker 5
```

- **Shelley through Babbage** (pre-Conway): `UnrestrictedVRFTiebreaker` — VRF tiebreaker always applies regardless of slot distance
- **Conway** and later: `RestrictedVRFTiebreaker 5` — VRF tiebreaker only applies when slots differ by ≤ 5

Rationale for restriction: if a block in slot S+6 couldn't build on a block in slot S (same block number), the issuer of S+6 had ample time to see the S block and mint on top of it. If they didn't, we don't reward that behavior with a VRF win.

## Hash-Based Tiebreaking

**There is NO hash-based tiebreaking anywhere.** The only tiebreaker fields are:
1. Opcert issue number (only same-issuer, same-slot case)
2. VRF output value (lower wins)

Block hash is never consulted for chain preference.

## Incumbent Wins on Tie — Exact Mechanism

In `preferAnchoredCandidate`:
```haskell
(_ :> ourTip, _ :> theirTip) ->
  case preferCandidate (projectChainOrderConfig cfg)
        (selectView cfg ourTip) (selectView cfg theirTip) of
    ShouldSwitch r    -> ShouldSwitch (Right r)
    ShouldNotSwitch o -> ShouldNotSwitch o    -- <-- stays on current chain
```

In `ChainSel.hs` constructPreferableCandidates:
```haskell
[ (chain, reason)
| chain <- fragments
, ShouldSwitch reason <- [preferAnchoredCandidate bcfg weights curChain (Diff.getSuffix chain)]
]
```
List comprehension: only chains where `ShouldSwitch` was returned are kept as candidates. `ShouldNotSwitch` (including EQ) is filtered out — the current chain is not replaced.

This is the "incumbent wins" rule: the node never switches to an equally-preferred chain, even if the Praos Ord instance would rank the candidate higher (e.g., `RestrictedVRFTiebreaker` with a far-apart slot case — candidate has lower VRF but is `ShouldNotSwitch EQ` not `ShouldSwitch`).

## Candidate Sorting When Multiple Compete

`sortCandidates` = `sortBy (flip (compareChainDiffs bcfg weights curChain) \`on\` fst)` — descending order (best first).

`compareChainDiffs` delegates to `compareAnchoredFragments` which uses the `Ord (SelectView p)` instance:
```haskell
compare = mconcat [compare \`on\` svBlockNo, compare \`on\` svTiebreakerView]
```

The `Ord (PraosTiebreakerView c)` instance uses `comparePraos UnrestrictedVRFTiebreaker` (always unrestricted for sorting, regardless of the flavor used for preference). This is important: the sorted order differs from the preference order when using `RestrictedVRFTiebreaker`.

`chainSelection'` tries candidates in sorted order, validating each one; it picks the first (best-sorted) valid candidate that `preferAnchoredCandidate` confirms is still preferred over whatever chain is current at that moment.

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
