---
name: credential-type-discrimination
description: How credential_type (0=KeyHash, 1=Script) is tracked and served in N2C query responses
type: reference
---

## Problem

The ledger state stores credential-keyed maps using `Hash32` (a 32-byte padded hash) as keys.
This is done by `credential_to_hash(credential: &Credential) -> Hash32` which discards the
`Credential::VerificationKey` vs `Credential::Script` variant information.

This caused all N2C query responses that include `credential_type` to always emit `0` (KeyHash),
even for script credentials.

## Solution

Two `HashSet<Hash32>` fields track which credential hashes are of script type:

1. `LedgerState::script_stake_credentials` — stake credentials registered via:
   - `Certificate::StakeRegistration`
   - `Certificate::ConwayStakeRegistration`
   - `Certificate::RegStakeDeleg`
   - `Certificate::RegStakeVoteDeleg`
   - `Certificate::VoteRegDeleg`
   Removed on `StakeDeregistration` and `ConwayStakeDeregistration`.

2. `GovernanceState::script_committee_credentials` — committee cold credentials from:
   - `Certificate::CommitteeHotAuth` (cold_credential)
   - `Certificate::CommitteeColdResign` (cold_credential)

## DReps — special case

`DRepRegistration` already stores the full `Credential` enum (with type info), so DRep
`credential_type` can be derived directly: `drep.credential.is_script() as u8`.
No separate set needed for DReps.

## N2C query responses fixed (in crates/dugite-node/src/node.rs)

| Query | Field | Fix |
|-------|-------|-----|
| GetDRepState | `DRepSnapshot::credential_type` | `drep.credential.is_script() as u8` |
| GetCommitteeState | `CommitteeMemberSnapshot::cold_credential_type` | `governance.script_committee_credentials.contains(cold) as u8` |
| GetStakeDelegDeposits | `StakeDelegDepositEntry::credential_type` | `ls.script_stake_credentials.contains(cred_hash) as u8` |
| GetFilteredVoteDelegatees | `VoteDelegateeEntry::credential_type` | `ls.script_stake_credentials.contains(stake_cred) as u8` |

## Known limitation

Reward accounts added from sources other than stake registration certificates (pool reward
accounts, governance action refunds) do not have `Credential` enum context available — they
work from raw byte arrays. In practice, script credentials at these positions are extremely
rare. These paths are left as `0` (KeyHash) since the Credential type cannot be recovered.

## Snapshot version

This change added `script_stake_credentials` to `LedgerState`, bumping `SNAPSHOT_VERSION`
from 2 to 3. Existing snapshots are incompatible and must be regenerated.
