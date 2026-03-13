# Cardano Haskell Oracle - Agent Memory

## Key Reference Files
- Conway UTXO rules: `cardano-ledger/eras/conway/impl/src/Cardano/Ledger/Conway/Rules/Utxo.hs`
- Conway UTXOW rules: `...Rules/Utxow.hs`
- Conway UTXOS (Phase-2): `...Rules/Utxos.hs`
- Conway LEDGER: `...Rules/Ledger.hs` (order: CERTS→GOV→UTXOW)
- Conway BBODY: `...Rules/Bbody.hs`
- Conway CERTS: `...Rules/Certs.hs` (sequential L→R processing)
- Conway DELEG: `...Rules/Deleg.hs`
- Conway GovCert: `...Rules/GovCert.hs`
- Conway GOV: `...Rules/Gov.hs` (19 predicate failures for proposals/votes)
- Conway Ratify: `...Rules/Ratify.hs`
- Conway Enact: `...Rules/Enact.hs`
- Conway Epoch: `...Rules/Epoch.hs`
- Conway NewEpoch: `...Rules/NewEpoch.hs`
- Conway PParams: `...Conway/PParams.hs` — PParams=array(31), PParamsUpdate=map(keys 0-33)
- Shelley Rewards: `shelley/impl/src/.../Shelley/Rewards.hs`
- maxPool function: `libs/cardano-ledger-core/src/.../State/SnapShots.hs`
- Pool desirability: `shelley/impl/src/.../Shelley/PoolRank.hs`

## Critical: ln' uses continued fraction, NOT Taylor series
- See [nonintegral-ln-algorithm.md](nonintegral-ln-algorithm.md)
- Torsten's fp_ln uses Taylor series -> different truncation -> boundary disagreements
- Haskell uses exact Rational for sigma/f; Torsten uses f64 -> precision loss
- activeSlotLog precomputed once via ln', not per-block

## Pool Distribution for Leader Check
- `nesPd` = `ssStakeMarkPoolDistr(esSnapshots es0)`, set once at epoch boundary
- Mark snapshot (pre-rotation) = set snapshot (post-rotation); memoized, not recomputed mid-epoch
- Torsten bug: uses snapshots.set on-the-fly instead of memoized pool_distr
- See [pool-distr-leader-check.md](pool-distr-leader-check.md)

## Block Validation: apply vs reapply
- Replay from snapshot (LedgerDB init): `tickThenReapply` -> NO VRF/KES/opcert/ledger validation
- New blocks from network: `tickThenApply` -> FULL validation (unless previously applied in session)
- `reupdateChainDepState`: only updates nonces/counters, NO crypto checks
- `updateChainDepState`: validateKESSignature + validateVRFSignature + then reupdateChainDepState
- STS.ValidateNone: skips all STS predicate failures (no UTxO checks, no script execution)
- No "sync mode" flag; purely structural: ImmutableDB blocks trusted, network blocks untrusted
- See [block-validation-modes.md](block-validation-modes.md)

## ChainSync At-Tip Behavior
- Connection stays OPEN — no disconnect/reconnect cycle
- Server sends MsgAwaitReply (tag 1) when follower is at head of chain
- Server then blocks on `followerInstructionBlocking` (STM retry until chain changes)
- Client receives MsgAwaitReply, sets csIdling=true, pauses LoP bucket
- Client enters StNext(StMustReply) state, waits for eventual RollForward/RollBackward
- Pipeline decision at tip: non-pipelined Request (not Pipeline) when client==server block number
- Default pipeline marks: lowMark=200, highMark=300 (pipelineDecisionLowHighMark)
- See [chainsync-at-tip.md](chainsync-at-tip.md)

## Topic Files
- [pparams-group-classification.md](pparams-group-classification.md) - Conway PP group classification (Network/Economic/Technical/Gov/Security), threshold combination logic
- [conway-validation-rules.md](conway-validation-rules.md) - Complete validation rules, predicate failures, reward formula, epoch transition order
- [n2n-protocols.md](n2n-protocols.md) - N2N protocol reference: mini-protocol IDs, CBOR/CDDL encodings, version negotiation, time limits, queue limits
- [n2c-protocol-details.md](n2c-protocol-details.md) - N2C protocol: all 4 mini-protocols, message tags, 40 query types with CBOR tags, wire format
- [vrf-input-construction.md](vrf-input-construction.md) - VRF seed construction: TPraos vs Praos, mkInputVRF, domain separation, Torsten bugs
- [pparams-cbor-encoding.md](pparams-cbor-encoding.md) - PParams array(31) encoding, PParamsUpdate map encoding, nested types, field ordering
- [lsq-result-encoding.md](lsq-result-encoding.md) - MsgResult wire format, HFC success wrapper [result], era mismatch encoding
- [peer-sharing-protocol.md](peer-sharing-protocol.md) - PeerSharing mini-protocol: wire format, address encoding, policy constants, governor integration
- [gov-state-cbor-encoding.md](gov-state-cbor-encoding.md) - GetGovState (tag 24) response: ConwayGovState array(7), Proposals, GovActionState, Committee, Constitution, DRepPulsingState encoding
- [shelley-genesis-cbor.md](shelley-genesis-cbor.md) - GetGenesisConfig (tag 11): CompactGenesis array(15), UTCTime encoding, legacy vs new PParams, activeSlotsCoeff NO tag(30)
- [era-history-wire-format.md](era-history-wire-format.md) - GetInterpreter/GetEraHistory: query=[2,0,[2,2,[1,0]]], response=list of EraSummary (no HFC wrapper), Bound/EraParams/SafeZone encoding, RelativeTime=Pico integer, SlotLength=milliseconds
- [epoch-nonce-calculation.md](epoch-nonce-calculation.md) - Praos epoch nonce: PraosState fields, per-block update, epoch boundary computation, stability windows, Torsten bugs
- [vrf-leader-check.md](vrf-leader-check.md) - VRF leader eligibility: checkLeaderValue, taylorExpCmp, FixedPoint E34, certNat/certNatMax, exact algorithm
- [block-forging-flow.md](block-forging-flow.md) - Complete block forging: slot tick→leader check→tx selection→body hash→header→KES sign, all key files and Torsten body hash bug
- [utxo-hd-snapshot-format.md](utxo-hd-snapshot-format.md) - UTxO-HD in-memory backend snapshot: version wrapper array(2)[1,ext], HFC telescope, per-era version array(2)[2,...], NewEpochState array(7), EMPTY UTxO in state file, tables written separately
- [query-version2-wire-format.md](query-version2-wire-format.md) - QueryVersion2 three-level nesting: Query(tag 0-4) → HFC(tag 0-2) → NS(era_idx, shelley_tag), EitherMismatch wrapping rules, golden test hex values
- [ledger-peer-snapshot-encoding.md](ledger-peer-snapshot-encoding.md) - GetLedgerPeerSnapshot (tag 34): V1/V2/V3 wire format, relay CBOR, Rational encoding, big vs all peers
- [genesis-bootstrap-protocol.md](genesis-bootstrap-protocol.md) - Ouroboros Genesis bootstrap: GSM (PreSyncing/Syncing/CaughtUp), LoE/GDD, CSJ, bootstrap peers, FetchModeGenesis, peer targets, HAA, caught-up detection

## N2C Key Facts
- Shelley query CBOR tags: 40 queries (0-39), see n2c-protocol-details.md
- Hard-fork wrapping: QueryIfCurrent=[tag 0], QueryAnytime=[tag 1], QueryHardFork=[tag 2]
- N2C mini-protocol IDs: Handshake=0, ChainSync=5, TxSubmission=6, StateQuery=7, TxMonitor=12
- N2C ChainSync sends full blocks (not headers), wrapped as [era_id, CBOR_tag_24(block_bytes)]
- NodeToClientVersion V16-V19=QueryVersion2, V20-V23=QueryVersion3
- **V21 PParams change**: ProtVer encodes as array(2)[major,minor] instead of two flat ints (Shelley-Babbage only; Conway unchanged). Field count drops by 1 for each pre-Conway era.
- **V21 new queries**: GetPoolDistr2 (tag 36, new PoolDistr type), GetStakeDistribution2 (tag 37), GetMaxMajorProtVersion (tag 38)
- **V21 removed**: GetStakeDistribution (tag 5) and GetPoolDistr (tag 21) rejected for V21+ clients
- **GetStakeDistribution2** returns new SL.PoolDistr = array(2)[pool_map, total_active_stake_int]; IndividualPoolStake is array(3)[rational, compact_coin_u64, vrf_hash_32bytes]
- See [n2c-version-v17-v22-changes.md](n2c-version-v17-v22-changes.md) for full version change table

## ouroboros-network Repo Structure (main branch)
- Protocol types: `ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/<Proto>/Type.hs`
- Protocol codecs: `ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/<Proto>/Codec.hs`
- Cardano N2N versions: `cardano-diffusion/api/lib/Cardano/Network/NodeToNode/Version.hs`
- CDDL specs: `cardano-diffusion/protocols/cddl/specs/`
- Diffusion config: `cardano-diffusion/lib/Cardano/Network/Diffusion/Configuration.hs`
- KeepAlive client: `ouroboros-network/lib/Ouroboros/Network/KeepAlive.hs`

## Mempool Implementation
- See [mempool-implementation.md](mempool-implementation.md) - FIFO ordering (NOT fee-based), StrictFingerTree, dual-FIFO fairness, multi-dimensional capacity, revalidation, snapshotting

## P2P Governor
- See [p2p-governor-architecture.md](p2p-governor-architecture.md) - Complete governor architecture: state machine, targets, churn, promotion/demotion, big ledger peers, connection manager, all constants

## N2N Protocol Status (cardano-node 10.6.2)
- Active versions: V14 (Plomin HF, mandatory), V15 (SRV DNS)
- Versions 7-13 obsolete
- Mini-protocol IDs: Handshake=0, ChainSync=2, BlockFetch=3, TxSubmission=4, KeepAlive=8, PeerSharing=10
