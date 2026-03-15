---
name: Conway Ratification Implementation Details
description: Complete CIP-1694 governance ratification algorithm from cardano-ledger-conway — threshold functions, enactment priority, committee expiry, DRep activity, parameter groups, treasury cap, delaying actions, prevActionId validation
type: reference
---

## Key Source Files
- Ratify rules: `eras/conway/impl/src/Cardano/Ledger/Conway/Rules/Ratify.hs`
- Enact rules: `...Rules/Enact.hs`
- Threshold functions + RatifyState: `...Governance/Internal.hs`
- GovAction types: `...Governance/Procedures.hs`
- DRepPulser: `...Governance/DRepPulser.hs`
- Epoch transition: `...Rules/Epoch.hs`
- DRep expiry: `...Rules/GovCert.hs` (computeDRepExpiry), `...Rules/Certs.hs` (updateVotingDRepExpiries, updateDormantDRepExpiry)
- PParams groups: `...Conway/PParams.hs` (THKD, PPGroups, modifiedPPGroups)
- Bootstrap phase: `...Conway/Era.hs` (hardforkConwayBootstrapPhase = pvMajor == 9)
- Safe division: `libs/cardano-ledger-core/src/.../BaseTypes/NonZero.hs` (%? returns 0 when denom is 0)

## Ratification Algorithm (ratifyTransition)
Processes proposals sequentially from reordered StrictSeq. For each proposal:
1. prevActionAsExpected (parent chain valid)
2. validCommitteeTerm (new members within maxTermLength)
3. NOT rsDelayed (no delaying action already enacted this epoch)
4. withdrawalCanWithdraw (treasury can cover withdrawals)
5. acceptedByEveryone (committee AND SPO AND DRep all accept)

If ALL pass: enact via ENACT rule, set rsDelayed if delayingAction, append to rsEnacted.
If ANY fail: skip, check if expired (gasExpiresAfter < reCurrentEpoch), add to rsExpired.
When sequence empty: set ensTreasury to Coin 0.

## Action Priority (reorderActions)
NoConfidence=0, UpdateCommittee=1, NewConstitution=2, HardForkInitiation=3, ParameterChange=4, TreasuryWithdrawals=5, InfoAction=6

## Delaying Actions
NoConfidence, HardForkInitiation, UpdateCommittee, NewConstitution are delaying.
ParameterChange, TreasuryWithdrawals, InfoAction are NOT delaying.
Once a delaying action is enacted, rsDelayed=true prevents ALL subsequent ratification this epoch.

## Committee Threshold
- NoConfidence/UpdateCommittee: NoVotingAllowed (committee cannot vote)
- InfoAction: NoVotingThreshold (can never ratify)
- Others: use committeeThreshold from Committee record
- If no committee (SNothing): NoVotingThreshold -> fails
- Committee min size check: activeCommitteeSize >= ppCommitteeMinSize (skipped during bootstrap phase)
- Active = registered + not resigned + not expired (currentEpoch <= validUntil)

## DRep Threshold
- During bootstrap (proto version 9): ALL DRep thresholds reset to 0 (def)
- After bootstrap: per-action thresholds from ppDRepVotingThresholds
- UpdateCommittee uses dvtCommitteeNormal when committee exists, dvtCommitteeNoConfidence otherwise
- ParameterChange: max threshold across all modified PP groups
- InfoAction: NoVotingThreshold

## SPO Threshold
- NoConfidence: pvtMotionNoConfidence
- UpdateCommittee: pvtCommitteeNormal/pvtCommitteeNoConfidence based on committee existence
- HardForkInitiation: pvtHardForkInitiation
- ParameterChange: pvtPPSecurityGroup ONLY if any modified param is SecurityGroup; else NoVotingAllowed
- NewConstitution, TreasuryWithdrawals: NoVotingAllowed
- InfoAction: NoVotingThreshold

## Default SPO Vote (post-bootstrap)
Determined by pool reward account's DRep delegation:
- DRepAlwaysAbstain -> DefaultAbstain (counts as abstain)
- DRepAlwaysNoConfidence -> DefaultNoConfidence (Yes on NoConfidence, No otherwise)
- Anything else -> DefaultNo
HardForkInitiation: non-voters ALWAYS count as No (even during bootstrap)

## DRep Accepted Ratio
yes/(yes+no) where abstains excluded from denominator:
- DRepAlwaysAbstain: excluded entirely
- DRepAlwaysNoConfidence: Yes on NoConfidence, No otherwise (always in denominator)
- Expired DReps: excluded entirely
- Unregistered DReps: excluded entirely
- Registered non-voting DReps: count as No (in denominator but not numerator)

## Committee Accepted Ratio
yes/(yes+no) excluding abstains:
- Expired members: treated as abstain (excluded)
- Unregistered members: treated as abstain (excluded)
- Resigned members: treated as abstain (excluded)
- Non-voting registered members: treated as No
- Zero threshold: short-circuit to accepted

## Treasury Withdrawals Cap
withdrawalCanWithdraw checks: sum of all withdrawal amounts <= ensTreasury
If exceeds, action is NOT ratified (stays in queue).

## Parameter Groups (THKD type-level tags)
Each PP field tagged with PPGroups DRepGroup StakePoolGroup:
- DRepGroup: NetworkGroup | EconomicGroup | TechnicalGroup | GovGroup
- StakePoolGroup: SecurityGroup | NoStakePoolGroup
modifiedPPGroups inspects which fields in PParamsUpdate are SJust.
DRep threshold = max of applicable group thresholds.
SPO threshold = pvtPPSecurityGroup if ANY field is SecurityGroup; else NoVotingAllowed.

## prevActionId Validation
withGovActionParent: checks gas's parent matches ensPrevGovActionIds for same purpose.
GovRelation has 4 fields: grPParamUpdate, grHardFork, grCommittee, grConstitution.
NoConfidence and UpdateCommittee share CommitteePurpose.
TreasuryWithdrawals and InfoAction have no parent chain.

## DRep Activity
computeDRepExpiry(activity, currentEpoch, numDormantEpochs) = addEpochInterval(currentEpoch, activity) - numDormantEpochs
Dormant epochs: counter incremented when no active proposals exist; reset when proposals appear.
updateDormantDRepExpiry: adds numDormantEpochs to all DRep expiries when proposals appear.
During bootstrap (v9): dormant epochs not subtracted from registration expiry.
