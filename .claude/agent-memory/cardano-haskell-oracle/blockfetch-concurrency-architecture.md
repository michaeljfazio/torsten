---
name: BlockFetch Concurrency Architecture
description: Complete BlockFetch decision logic, concurrency limits, block ordering, ChainDB async processing, peer selection during bulk sync vs deadline mode
type: reference
---

## Key Source Files
- `ouroboros-network/lib/Ouroboros/Network/BlockFetch/Decision.hs` — FetchDecisionPolicy, fetchDecisions pipeline, fetchRequestDecisions
- `ouroboros-network/lib/Ouroboros/Network/BlockFetch/State.hs` — fetchLogicIterations main loop
- `ouroboros-network/lib/Ouroboros/Network/BlockFetch/Client.hs` — blockFetchClient per-peer download
- `ouroboros-network/lib/Ouroboros/Network/BlockFetch.hs` — BlockFetchConfiguration, blockFetchLogic entry
- `cardano-diffusion/lib/Cardano/Network/Diffusion/Configuration.hs` — defaultBlockFetchConfiguration
- `cardano-diffusion/lib/Cardano/Network/NodeToNode.hs` — defaultMiniProtocolParameters (blockFetchPipeliningMax=100)
- `ouroboros-consensus/src/.../MiniProtocol/BlockFetch/ClientInterface.hs` — mkBlockFetchConsensusInterface, readFetchMode (1000 slot threshold)
- `ouroboros-consensus/src/.../Storage/ChainDB/Impl/ChainSel.hs` — chainSelectionForBlock, async block processing
- `ouroboros-consensus/src/.../Storage/ChainDB/Impl.hs` — addBlockAsync, cdbChainSelQueue

## Default Configuration (cardano-node production)
- `bfcMaxConcurrencyBulkSync = 1` (library default=1, mainnet config=1)
- `bfcMaxConcurrencyDeadline = 2` (library default=1, mainnet config overrides to 2)
- `bfcMaxRequestsInflight = 100` (from blockFetchPipeliningMax)
- `bfcDecisionLoopIntervalPraos = 0.01s` (10ms)
- `bfcDecisionLoopIntervalGenesis = 0.04s` (40ms)
- `gbfcGracePeriod = 10s`

## BulkSync vs Deadline Mode Decision
- readFetchModeDefault: if chain tip < 1000 slots behind wall clock → Deadline, else → BulkSync
- Bootstrap peers mode: always BulkSync regardless of slot distance

## Key Architecture Facts
1. **BulkSync fetches from ONE peer** — maxConcurrencyBulkSync=1 means only 1 peer gets fetch requests
2. **BulkSync deduplicates** — filterNotAlreadyInFlightWithOtherPeers removes blocks already being fetched from another peer
3. **Deadline allows parallel** — filterNotAlreadyInFlightWithOtherPeers is a no-op in deadline mode; duplicate fetches allowed
4. **Blocks arrive per-block** — addFetchedBlock callback called for each block during streaming (not batched)
5. **Blocks go to ChainDB async** — addBlockAsync queues to cdbChainSelQueue, returns immediately
6. **ChainDB processes sequentially** — single background thread (chainSelSync) processes queue in order
7. **Out-of-order blocks supported** — ChainDB stores in VolatileDB, chain selection constructs candidates from successor graph
8. **Validation on chain selection** — validateCandidate applies blocks to ledger only when a better chain is found

## Decision Pipeline (fetchDecisions)
1. filterPlausibleCandidates — check candidate chains vs current
2. selectForkSuffixes — find suffix beyond current chain
3. filterNotAlreadyFetched — skip blocks already in ChainDB
4. filterNotAlreadyInFlightWithPeer — skip blocks this peer is already fetching
5. prioritisePeerChains — sort by chain quality + network metrics
6. filterNotAlreadyInFlightWithOtherPeers — BulkSync: deduplicate; Deadline: no-op
7. fetchRequestDecisions — enforce concurrency limits, produce FetchRequest per peer

## Block Flow: Fetch → Ledger
1. BlockFetch client streams blocks from peer
2. Per block: addFetchedBlock → addBlockAsync → queue on cdbChainSelQueue
3. chainSelSync thread: write to VolatileDB → trigger chainSelectionForBlock
4. chainSelectionForBlock: build candidate chains via successor graph
5. If better candidate found: validateCandidate applies blocks to ledger via ExtLedgerState
6. If valid: switchTo atomically updates current chain + ledger state
