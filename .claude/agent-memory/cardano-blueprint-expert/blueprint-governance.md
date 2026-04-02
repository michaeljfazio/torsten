---
name: blueprint-governance
description: Cardano Blueprint governance (CIP-1694) documentation — what IS and IS NOT covered, DRep certs, GOV era rule details from block-validation.md
type: reference
---

# Cardano Blueprint — Governance (CIP-1694)

## What Blueprint Documents

The Blueprint does NOT have a dedicated governance section. Governance is documented **as part of the Conway block validation** in `src/ledger/block-validation.md` — specifically the EraRule GOV and EraRule GOVERT (governance certificates) sections.

## Governance in Conway Block Validation

### EraRule GOVERT (Governance Certificates)

Processed during CERTS rule. Handles:

**ConwayRegDRep** (DRep Registration):
- `Map.notMember cred vsDReps` — credential not already registered
- `deposit == ppDRepDeposit` — correct deposit amount
- Returns updated `dRepState`

**ConwayUnregDRep** (DRep Deregistration):
- `isJust mDRepState` — DRep is registered
- `failOnJust drepRefundMismatch` — refund amount must match
- Returns updated `dRepState`

**ConwayUpdateDRep** (DRep Metadata Update):
- `Map.member cred vsDReps` — DRep must be registered
- Returns updated `vsDReps`

**ConwayResignCommitteeColdKey** / **ConwayAuthCommitteeHotKey**:
Both call `checkAndOverwriteCommitteeMemberState`:
- `failOnJust coldCredResigned` — cold key must not already be resigned
- `isCurrentMember OR isPotentialFutureMember` — must be valid committee member
- Returns updated `vsCommitteeState`

### EraRule GOV (Governance Proposals and Votes)

Validates governance actions and votes in a transaction:

**Governance Proposal Validation**:
- `failOnJust badHardFork` — hard fork action must be valid
- `actionWellFormed` — well-formed check
- `refundAddress` — return address must be valid
- `nonRegisteredAccounts` — accounts must exist
- `pProcDeposit == expectedDeposit` — correct deposit for action type
- `pProcReturnAddr == expectedNetworkId` — correct network ID

**Per-action-type checks**:
- `TreasuryWithdrawals`: `mismatchedAccounts`, `checkPolicy` (guardrail script)
- `UpdateCommittee`: `Set.null conflicting` (no conflicting members), `Map.null invalidMembers`
- `ParameterChange`: `checkPolicy` (guardrail script)

**Vote Validation**:
- `ancestryCheck` — vote ancestry is valid
- `failOnNonEmpty unknownVoters` — all voters must be known
- `failOnNonEmpty unknownGovActionIds` — all action IDs must exist
- `checkBootstrapVotes` — bootstrap era voting constraints
- `checkVotesAreNotForExpiredActions` — actions must not be expired
- `checkVotersAreValid` — voters must be eligible

Returns `updatedProposalStates`

## What Blueprint Does NOT Document

The following governance topics are NOT covered in the Blueprint (as of 2026-03-13):

- DRep voting thresholds by governance action type
- SPO voting thresholds by governance action type
- Constitutional Committee voting thresholds
- Ratification rules and timing
- Enactment rules and priority
- DRep active_until_epoch calculation (registered_epoch + drep_activity)
- Governance action lifetime
- Treasury withdrawal mechanics
- No-confidence state and committee dissolution rules
- Hard fork initiation process
- Protocol parameter update groups (network/economic/technical/governance)
- Bootstrap phase governance restrictions
- CIP-1694 overview and governance design

## Authoritative Sources for Governance

For full CIP-1694 governance details, refer to:
- **CIP-1694**: https://github.com/cardano-foundation/CIPs/blob/master/CIP-1694/README.md
- **Conway formal spec**: https://intersectmbo.github.io/formal-ledger-specifications/conway-ledger.pdf
- **Haskell cardano-ledger Conway implementation**: https://github.com/IntersectMBO/cardano-ledger/tree/master/eras/conway

## Key Governance Constants (from Dugite project memory, not Blueprint)

From dugite project implementation (not from Blueprint directly):

- 4 DRep PP group thresholds: `dvt_pp_network_group`, `dvt_pp_economic_group`, `dvt_pp_technical_group`, `dvt_pp_gov_group`
- 5 SPO voting thresholds: `pvt_motion_no_confidence`, `pvt_committee_normal`, `pvt_committee_no_confidence`, `pvt_hard_fork`, `pvt_pp_security_group`
- `GovernanceState.no_confidence` flag tracks committee dissolved state
- `UpdateCommittee` uses different thresholds based on `no_confidence` state
- DRep `active_until_epoch = registered_epoch + drep_activity`
