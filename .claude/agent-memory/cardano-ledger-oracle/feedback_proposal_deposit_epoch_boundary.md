---
name: Conway proposal deposit epoch boundary — authoritative corrections
description: Corrects prior speculation that returnProposalDeposits could route active proposal deposits to treasury; documents verified source facts about proposal removal, expiry check, and deposit accounting
type: feedback
---

The following facts are verified from cardano-ledger source. Prior speculation that `returnProposalDeposits` could route active (non-expired, non-enacted) proposal deposits to treasury when their return credential is unregistered was WRONG. Do not repeat that claim.

## Verified Facts

**1. `returnProposalDeposits` scope**
`returnProposalDeposits` in `eras/conway/impl/src/Cardano/Ledger/Conway/Rules/Epoch.hs` iterates ONLY `allRemovedGovActions = expiredActions ∪ enactedActions ∪ removedDueToEnactment`. It does NOT sweep the active proposals map. Active non-expired non-enacted non-sibling-of-enacted proposals are completely untouched at the epoch boundary, regardless of their return credential registration state.

**2. Only write paths to `cgsProposalsL` at epoch boundary**
The only ways a proposal is removed from `cgsProposalsL` at an epoch boundary are via `proposalsApplyEnactment` in `Proposals.hs` (lines 492–554), which takes `rsEnacted` and `rsExpired` from the RATIFY pulser output and applies:
- (a) `proposalsRemoveWithDescendants expiredGais`
- (b) per-enacted sibling removal via `proposalsRemoveWithDescendants siblings`
- (c) the enacted action itself via `OMap.extractKeys`

There is no other write to `cgsProposalsL` at boundary.

**3. `reCurrentEpoch` in `RatifyEnv`**
`reCurrentEpoch` equals the epoch in which the pulser RUNS, not the epoch it is consumed in. The pulser is created at the (N-1)→N boundary via `setFreshDRepPulsingState eNo = N`, setting `dpCurrentEpoch = N` (DRepPulser/Governance.hs line 505). When consumed at N→N+1, `reCurrentEpoch` is still N.

**4. Expiry check — off-by-one**
`Ratify.hs` line 357: `gasExpiresAfter < reCurrentEpoch`. A proposal with `gasExpiresAfter = E` expires at the (E+1)→(E+2) boundary, NOT (E)→(E+1). Concretely: a proposal with `gasExpiresAfter = 735` is NOT expired at 735→736 (because `735 < 735` is false). It expires at 736→737.

**5. `totalObligation`/`obligationGovState` accounting**
`obligationGovState` reads `deposits.proposal` directly from the `cgsProposalsL` OMap via `foldMap' gasDeposit $ proposalsActions`. Deposits.proposal and the proposals map are one-to-one; there is no separate accounting table.

**6. No silent proposal drops**
There is NO submission-time predicate failure path by which a tx lands on-chain but its proposal is silently dropped. GOV rule predicate failures (including `ProposalProcedureNetworkIdMismatch`, `ProposalReturnAccountDoesNotExist`) fail the entire LEDGER transition, invalidating the whole tx. If the tx would have consumed inputs, those inputs remain unconsumed.

**7. Deposits.proposal vs treasury divergence diagnosis**
If two implementations agree on totalStake, reserves, activeStake, and epochFees at epoch E but disagree on `deposits.proposal` vs `treasury` by exactly the amount of one proposal deposit, the ONLY self-consistent explanation is a **chain ingestion divergence** — one implementation applied a tx to its chain that the other did not. This is not a governance rule bug.

**Why:** User corrected speculation from an earlier consultation; all facts above are source-verified.

**How to apply:** When answering any question about proposal deposit lifecycle, epoch boundary cleanup, expiry semantics, or `totalObligation` accounting in Conway, apply these facts. Do not speculate that active proposals can have deposits routed to treasury at epoch boundary.
