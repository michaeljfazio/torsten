# ConwayGovState CBOR Encoding (GetGovState Query, Shelley Tag 24)

## Query Wire Format
- Request: `[1, 24]` (list len 1, tag 24)
- Response: `toCBOR govState` — direct ConwayGovState encoding (no extra wrapping beyond HFC)

## ConwayGovState — CBOR Array(7)
File: `eras/conway/impl/src/Cardano/Ledger/Conway/Governance/Internal.hs`

Fields in order:
1. `cgsProposals` — Proposals
2. `cgsCommittee` — StrictMaybe(Committee) — list-based: [] for SNothing, [committee] for SJust
3. `cgsConstitution` — Constitution
4. `cgsCurPParams` — PParams (array of 31 fields)
5. `cgsPrevPParams` — PParams (array of 31 fields)
6. `cgsFuturePParams` — FuturePParams (tagged sum: 0=NoPParamsUpdate, 1=Definite+PP, 2=Potential+Maybe(PP))
7. `cgsDRepPulsingState` — DRepPulsingState (always encoded as DRComplete format)

## Proposals — CBOR Array(2) (tuple encoding)
File: `eras/conway/impl/src/Cardano/Ledger/Conway/Governance/Proposals.hs`

Encoded as 2-tuple `(roots, omap_props)`:
1. `roots` — GovRelation StrictMaybe (from toPrevGovActionIds)
2. `pProps` — OMap of GovActionState values

### GovRelation StrictMaybe — CBOR Array(4)
File: `eras/conway/impl/src/Cardano/Ledger/Conway/Governance/Procedures.hs`

Fields:
1. `grPParamUpdate` — StrictMaybe(GovPurposeId) — list-based: []/[gov_action_id]
2. `grHardFork` — StrictMaybe(GovPurposeId)
3. `grCommittee` — StrictMaybe(GovPurposeId)
4. `grConstitution` — StrictMaybe(GovPurposeId)

GovPurposeId is newtype over GovActionId.

### OMap Encoding — CBOR Array of values (keys derived from values via HasOKey)
File: `libs/cardano-data/src/Data/OMap/Strict.hs`

Encodes as `encodeStrictSeq` of GovActionState values. Keys not separately encoded.

## GovActionState — CBOR Array(7)
File: `eras/conway/impl/src/Cardano/Ledger/Conway/Governance/Procedures.hs`

Fields:
1. `gasId` — GovActionId (array(2): [TxId, GovActionIx(Word16)])
2. `gasCommitteeVotes` — Map(Credential HotCommitteeRole -> Vote)
3. `gasDRepVotes` — Map(Credential DRepRole -> Vote)
4. `gasStakePoolVotes` — Map(KeyHash StakePool -> Vote)
5. `gasProposalProcedure` — ProposalProcedure
6. `gasProposedIn` — EpochNo (Word64)
7. `gasExpiresAfter` — EpochNo (Word64)

## ProposalProcedure — CBOR Array(4)
1. `pProcDeposit` — Coin (Word64)
2. `pProcReturnAddr` — AccountAddress (raw bytes)
3. `pProcGovAction` — GovAction (tagged sum, see below)
4. `pProcAnchor` — Anchor

## GovAction — Tagged Sum (array with tag)
- Tag 0: ParameterChange [StrictMaybe(GovActionId), PParamsUpdate, StrictMaybe(ScriptHash)]
- Tag 1: HardForkInitiation [StrictMaybe(GovActionId), ProtVer]
- Tag 2: TreasuryWithdrawals [Map(RewardAccount->Coin), StrictMaybe(ScriptHash)]
- Tag 3: NoConfidence [StrictMaybe(GovActionId)]
- Tag 4: UpdateCommittee [StrictMaybe(GovActionId), Set(ColdCred), Map(ColdCred->EpochNo), UnitInterval]
- Tag 5: NewConstitution [StrictMaybe(GovActionId), Constitution]
- Tag 6: InfoAction (no additional fields)

## Committee — CBOR Array(2)
1. `committeeMembers` — Map(Credential ColdCommitteeRole -> EpochNo)
2. `committeeThreshold` — UnitInterval (tagged 30, [numerator, denominator])

## Constitution — CBOR Array(2)
1. `constitutionAnchor` — Anchor
2. `constitutionGuardrailsScriptHash` — null-encoded StrictMaybe(ScriptHash)
   NOTE: Uses `encodeNullStrictMaybe` (CBOR null for SNothing, raw hash for SJust)

## Anchor — CBOR Array(2)
1. `anchorUrl` — Url (text string)
2. `anchorDataHash` — SafeHash (32-byte hash)

## DRepPulsingState — always encoded as DRComplete — CBOR Array(2)
File: `eras/conway/impl/src/Cardano/Ledger/Conway/Governance/DRepPulser.hs`

Even if pulsing is in progress, it's completed before encoding.
1. PulsingSnapshot
2. RatifyState

### PulsingSnapshot — CBOR Array(4)
1. `psProposals` — StrictSeq(GovActionState)
2. `psDRepDistr` — Map(DRep -> CompactForm Coin)
3. `psDRepState` — Map(Credential DRepRole -> DRepState)
4. `psPoolDistr` — Map(KeyHash StakePool -> CompactForm Coin)

### RatifyState — CBOR Array(4)
1. `rsEnactState` — EnactState
2. `rsEnacted` — Seq(GovActionState)
3. `rsExpired` — Set(GovActionId)
4. `rsDelayed` — Bool

### EnactState — CBOR Array(7)
1. `ensCommittee` — StrictMaybe(Committee) — list-based encoding
2. `ensConstitution` — Constitution
3. `ensCurPParams` — PParams
4. `ensPrevPParams` — PParams
5. `ensTreasury` — Coin
6. `ensWithdrawals` — Map(Credential Staking -> Coin)
7. `ensPrevGovActionIds` — GovRelation StrictMaybe (array of 4)

## Vote Encoding
- VoteNo = 0, VoteYes = 1, Abstain = 2

## DRep Encoding (tagged sum)
- Tag 0: DRepKeyHash [KeyHash]
- Tag 1: DRepScriptHash [ScriptHash]
- Tag 2: DRepAlwaysAbstain (no payload)
- Tag 3: DRepAlwaysNoConfidence (no payload)

## DRepState — CBOR Array(4)
1. `drepExpiry` — EpochNo
2. `drepAnchor` — StrictMaybe(Anchor) — list-based encoding
3. `drepDeposit` — CompactForm Coin
4. `drepDelegs` — Set(Credential Staking)

## Voter Encoding (tagged sum)
- Tag 0: CommitteeVoter with KeyHashObj [Credential]
- Tag 1: CommitteeVoter with ScriptHashObj [Credential]
- Tag 2: DRepVoter with KeyHashObj [Credential]
- Tag 3: DRepVoter with ScriptHashObj [Credential]
- Tag 4: StakePoolVoter [KeyHash]

## StrictMaybe Encoding (DEFAULT — list-based)
- SNothing → encodeListLen 0 (empty CBOR array)
- SJust x → encodeListLen 1 <> encCBOR x (single-element array)
Note: Constitution's guardrails script hash uses NULL-based encoding instead (encodeNullStrictMaybe)

## Key Encoding Patterns
- `Rec` combinator → CBOR array with length prefix (encodeListLen n)
- `Sum` combinator → CBOR array with tag word: encodeListLen (n+1) <> encodeWord tag
- Tuples → CBOR array: encodeListLen n <> fields
- OMap → CBOR array of values only (keys reconstructed from values)
