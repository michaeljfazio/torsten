# N2C Protocol Implementation Details

## LocalStateQuery CBOR Message Tags
- 0: MsgAcquire (SpecificPoint) [0, point]
- 1: MsgAcquired [1]
- 2: MsgFailure [2, failure_code] (0=PointTooOld, 1=PointNotOnChain)
- 3: MsgQuery [3, query]
- 4: MsgResult [4, result]
- 5: MsgRelease [5]
- 6: MsgReAcquire (SpecificPoint) [6, point]
- 7: MsgDone [7]
- 8: MsgAcquire (VolatileTip) [8]
- 9: MsgReAcquire (VolatileTip) [9]
- 10: MsgAcquire (ImmutableTip) [10] (V2+)
- 11: MsgReAcquire (ImmutableTip) [11] (V2+)

## LocalTxMonitor CBOR Message Tags (from CDDL spec)
- 0: MsgDone [0]
- 1: MsgAcquire [1]
- 2: MsgAcquired [2, slotNo]
- 3: MsgRelease [3]
- 5: MsgNextTx [5]
- 6: MsgReplyNextTx [6] or [6, tx]
- 7: MsgHasTx [7, txId]
- 8: MsgReplyHasTx [8, bool]
- 9: MsgGetSizes [9]
- 10: MsgReplyGetSizes [10, [capacity, size, numTxs]]
- 11: MsgGetMeasures [11] (V2+)
- 12: MsgReplyGetMeasures [12, txCount, measureMap]

## LocalTxSubmission CBOR Message Tags
- 0: MsgSubmitTx [0, tx]
- 1: MsgAcceptTx [1]
- 2: MsgRejectTx [2, rejectReason]
- 3: MsgDone [3]

## LocalChainSync CBOR Message Tags (same as N2N ChainSync)
- 0: MsgRequestNext [0]
- 1: MsgAwaitReply [1]
- 2: MsgRollForward [2, block/header, tip]
- 3: MsgRollBackward [3, point, tip]
- 4: MsgFindIntersect [4, points]
- 5: MsgIntersectFound [5, point, tip]
- 6: MsgIntersectNotFound [6, tip]
- 7: MsgDone [7]

## Shelley BlockQuery CBOR Tags (era-specific, inside QueryIfCurrent wrapper)
0=GetLedgerTip, 1=GetEpochNo, 2=GetNonMyopicMemberRewards,
3=GetCurrentPParams, 4=GetProposedPParamsUpdates, 5=GetStakeDistribution,
6=GetUTxOByAddress, 7=GetUTxOWhole, 8=DebugEpochState, 9=GetCBOR,
10=GetFilteredDelegationsAndRewardAccounts, 11=GetGenesisConfig,
12=DebugNewEpochState, 13=DebugChainDepState, 14=GetRewardProvenance,
15=GetUTxOByTxIn, 16=GetStakePools, 17=GetStakePoolParams,
18=GetRewardInfoPools, 19=GetPoolState, 20=GetStakeSnapshots,
21=GetPoolDistr, 22=GetStakeDelegDeposits, 23=GetConstitution,
24=GetGovState, 25=GetDRepState, 26=GetDRepStakeDistr,
27=GetCommitteeMembersState, 28=GetFilteredVoteDelegatees,
29=GetAccountState, 30=GetSPOStakeDistr, 31=GetProposals,
32=GetRatifyState, 33=GetFuturePParams, 34=GetLedgerPeerSnapshot',
35=QueryStakePoolDefaultVote, 36=GetPoolDistr2, 37=GetStakeDistribution2,
38=GetMaxMajorProtocolVersion, 39=GetDRepDelegations

## Hard-Fork Query Wrapping
- QueryIfCurrent: [2-elem list, tag 0, dispatched_query] — era index implicit in dispatch
- QueryAnytime: [3-elem list, tag 1, encoded_query, era_index]
  - GetEraStart: [1-elem, tag 0]
- QueryHardFork: [2-elem list, tag 2, hf_query]
  - GetInterpreter: [1-elem, tag 0]
  - GetCurrentEra: [1-elem, tag 1]

## Outermost Query Type (Consensus Layer, NOT era-specific)
Source: Ouroboros.Consensus.Ledger.Query
- Tag 0: BlockQuery [2, tag=0, wrapped_block_query] — delegates to HardFork/era layer
- Tag 1: GetSystemStart [1, tag=1]
- Tag 2: GetChainBlockNo [1, tag=2] — requires QueryVersion2 (N2C v16+)
- Tag 3: GetChainPoint [1, tag=3] — requires QueryVersion2 (N2C v16+)
- Tag 4: DebugLedgerConfig [1, tag=4] — requires QueryVersion3 (N2C v20+)
These are NOT Shelley-level tags. They sit ABOVE the HardFork wrapping layer.

## N2C vs N2N ChainSync Key Differences
- N2N: MsgRollForward carries **headers** (for chain selection), blocks fetched separately via BlockFetch
- N2C: MsgRollForward carries **full serialized blocks** wrapped as [era_id, CBOR_tag_24(block_bytes)]
- Both use same message tags (0-7)
- N2C has no pipelining in the protocol spec (client must wait for response)
- Tip format identical: [slot, hash, block_no]

## NodeToClientVersion — Complete Version Table (ouroboros-network main, 2026-03-09)
Source: cardano-diffusion/api/lib/Cardano/Network/NodeToClient/Version.hs

| N2C Version | Wire Value | ShelleyNtCVersion | What Changed |
|-------------|------------|-------------------|--------------|
| V16 | 16 + bit15 = 32784 | ShelleyNtCv8 | Conway era; ImmutableTip + GetStakeDelegDeposits queries |
| V17 | 17 + bit15 = 32785 | ShelleyNtCv9 | GetProposals + GetRatifyState queries |
| V18 | 18 + bit15 = 32786 | ShelleyNtCv10 | GetFuturePParams query |
| V19 | 19 + bit15 = 32787 | ShelleyNtCv11 | GetBigLedgerPeerSnapshot query |
| V20 | 20 + bit15 = 32788 | ShelleyNtCv12 | QueryStakePoolDefaultVote; MsgGetMeasures in LocalTxMonitor |
| V21 | 21 + bit15 = 32789 | ShelleyNtCv13 | New codecs for PParams + CompactGenesis |
| V22 | 22 + bit15 = 32790 | ShelleyNtCv14 | SRV record support in GetBigLedgerPeerSnapshot |
| V23 | 23 + bit15 = 32791 | ShelleyNtCv15 | QueryDRepDelegations; LedgerPeerSnapshot includes block hash + NetworkMagic |

Latest released: V23 (all versions V16-V23 offered in handshake)
Versions < V16: removed/obsolete.
Dijkstra era: already wired in all CardanoNodeToClientVersion patterns (era 8 in HardFork list)

### NodeToClientVersionData (handshake payload)
Fields:
- `networkMagic :: NetworkMagic` (u32)
- `query :: Bool`

CBOR encoding: `TList [TInt(networkMagic), TBool(query)]`

### Version Wire Encoding (in handshake propose/accept)
- Version number = `version_number | (1 << 15)` — bit 15 set to distinguish N2C from N2N
- E.g. V16 = 16 | 0x8000 = 32784 = 0x8010
- Decoding: check bit 15 is set, then clear it to get version number

### Version Negotiation
- Network magic must match (reject otherwise)
- `query` field uses logical OR: `query = local.query || remote.query`

### QueryVersion mapping
- V16-V19: QueryVersion2 (supports GetChainBlockNo, GetChainPoint)
- V20-V23: QueryVersion3 (adds DebugLedgerConfig)

## ApplyTxErr (LocalTxSubmission rejection)
- Conway: ApplyTxError wraps nested rule failures
- Structure: ConwayLedgerPredFailure -> ConwayUtxowPredFailure -> ConwayUtxoPredFailure -> ConwayUtxosPredFailure
- Phase-2 specific: ConwayUtxosPredFailure::ValidationTagMismatch(IsValid, TagMismatchDescription)
- Encoded via era-specific EncCBOR instances from cardano-ledger
- The reject reason in MsgRejectTx is the full CBOR-encoded ApplyTxError
