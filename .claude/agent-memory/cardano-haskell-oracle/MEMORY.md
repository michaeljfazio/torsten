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

## Conway Governance Ratification
- See [conway-ratification-details.md](conway-ratification-details.md) - Complete CIP-1694 ratification algorithm, threshold functions, enactment priority, committee expiry, DRep activity, parameter groups, treasury cap, delaying actions, prevActionId

## lsm-tree (UTxO-HD Storage Backend)
- See [lsm-tree-architecture.md](lsm-tree-architecture.md) — Complete architecture: lazy levelling merge, 4-file run format, page layout, bloom filters, fence pointers, incremental merge scheduler, NO WAL

## N2N Connection Architecture
- See [n2n-connection-architecture.md](n2n-connection-architecture.md) — MuxMode, DataFlow, bit-15 convention, protocol temperature, Hot/Warm/Cold, TxSubmission2 delay, error propagation
- See [mux-connection-architecture.md](mux-connection-architecture.md) — Complete mux deep-dive: single TCP per peer, 3 core threads (muxer/demuxer/monitor), SDU framing, temperature lifecycle (Cold→Warm→Hot), all timeout values, BlockFetch decision loop, ingress queue limits

## TxSubmission2 Architecture
- See [txsubmission2-architecture.md](txsubmission2-architecture.md) — Complete deep-dive: V1/V2 server, outbound client, governor lifecycle, decision logic, mempool sync, connection stability, Torsten gaps

## Mempool Tx Ordering & Chained Tx Deep-Dive
- See [mempool-tx-ordering.md](mempool-tx-ordering.md) — FIFO ordering (TicketNo), virtual ledger state for chained txs, TxSubmission2 serving order, block production prefix, revalidation logic, policy constants, Torsten divergences

## Reward Iteration & Zero-Reward Conditions
- See [reward-iteration-deep-dive.md](reward-iteration-deep-dive.md) — startStep iterates GO snapshot pool params (not BlocksMade), mkPoolRewardInfo checks BlocksMade for each, effective intersection requirement, genesis pool 2-epoch warm-up, 6 zero-reward conditions, no active_epoch_no check

## Monetary Expansion & RUPD
- See [monetary-expansion-rupd.md](monetary-expansion-rupd.md) — deltaR1, eta, expectedBlocks, block counting (incrBlocks/isOverlaySlot), snapshot system, TICK→RUPD data flow, Conway d=0 simplification, first-epoch behavior
- See [rupd-timing-data-flow.md](rupd-timing-data-flow.md) — COMPLETE trace: timing windows (sr=4k/f not 3*(1/f-1)), TICK/RUPD env extraction, NEWEPOCH ordering (applyRUpd BEFORE SNAP), fee lifecycle (deltaF=-ssFee), epoch 0 behavior, Torsten snapshot divergence
- See [newepoch-ordering-details.md](newepoch-ordering-details.md) — Exact NEWEPOCH operation order (applyRUpd→MIR→SNAP→POOLREAP→UPEC→record update), bprev source for RUPD (N-1 blocks), PP update timing (targeting epoch E applied at E-1→E boundary), incrBlocks uses post-TICK curPParams
- See [reward-update-accounting.md](reward-update-accounting.md) — Complete deltaR/deltaT/deltaF formulas, sign conventions, conservation invariant (deltaT+deltaR+sum(rs)+deltaF=0), undistributed→reserves (NOT treasury), applyRUpd exact application, no-blocks behavior, d>=0.8 semantics, Torsten divergences

## Mempool Revalidation After Block
- See [mempool-revalidation-after-block.md](mempool-revalidation-after-block.md) — Full revalidation of ALL remaining txs via revalidateTxsFor/reapplyTxs; triggered by STM watcher on ledger tip change (NOT direct ChainSel call); reapplyTx skips crypto, runs UTxO/TTL/script checks; atomic metrics update

## Fork Resolution / Chain Selection
- [fork-resolution-chainsel.md](fork-resolution-chainsel.md) — Complete ChainSel algorithm: addBlock flow, preferAnchoredCandidate, Praos tiebreaker, rollback, tentative follower, candidate storage

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
- [msgrejecttx-wire-format.md](msgrejecttx-wire-format.md) - MsgRejectTx full CBOR encoding: mini-protocol envelope, HFC wrapping, all Conway predicate failure types with tag numbers

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

## N2N Protocol Status (cardano-node 10.6.2)
- Active versions: V14 (Plomin HF, mandatory), V15 (SRV DNS)
- Versions 7-13 obsolete
- Mini-protocol IDs: Handshake=0, ChainSync=2, BlockFetch=3, TxSubmission=4, KeepAlive=8, PeerSharing=10

## Genesis Initialization & Ledger State
- See [genesis-initialization-ledger-state.md](genesis-initialization-ledger-state.md) — Byron→Shelley translation, genDelegs vs pool registrations, staking section, resetStakeDistribution, initial snapshots, preview testnet init sequence

## BlockFetch Concurrency Architecture
- See [blockfetch-concurrency-architecture.md](blockfetch-concurrency-architecture.md) — BulkSync=1 peer, Deadline=2 peers, decision pipeline, async ChainDB queue, out-of-order block support, per-block addFetchedBlock callback, 100 max in-flight per peer

## Test Vectors for Conformance
- See [test-vectors-reference.md](test-vectors-reference.md) — Full catalog of test vectors from all Haskell repos
- ouroboros-consensus golden: 1620 raw CBOR files (blocks, queries, results, disk state per era/version)
- cardano-ledger: CDDL specs, PParams JSON golden, Alonzo block/tx CBOR, non-integral VRF math vectors
- plutus: 999 UPLC conformance tests (program + expected result + budget)
- ouroboros-network: CDDL specs for all 10 mini-protocols

## BlockFetch / ChainSync Wire Format
- [blockfetch-hfc-wire-format.md](blockfetch-hfc-wire-format.md) — MsgBlock=tag(24) bstr(stored_cbor), NOT array[hfc_index,tag24(body)]; ChainSync headers use different encodeNS path

## SDU Segmentation Size — CRITICAL
- See [sdu-segmentation-critical.md](sdu-segmentation-critical.md) — SDUSize=12288 is PAYLOAD split point (NOT 12280); Haskell splits EXACTLY at sduSize bytes, NO -8 adjustment; ingress accepts any payload_length 0-65535
