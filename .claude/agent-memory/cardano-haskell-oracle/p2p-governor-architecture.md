---
name: P2P Governor Architecture
description: Complete P2P Peer Selection Governor architecture from ouroboros-network - state machine, targets, churn, promotion/demotion logic, big ledger peers, connection manager
type: reference
---

# P2P Governor Architecture Reference

## Source Files (ouroboros-network repo, main branch)

### Core Governor
- `ouroboros-network/lib/Ouroboros/Network/PeerSelection/Governor.hs` — main loop (peerSelectionGovernor, peerSelectionGovernorLoop)
- `ouroboros-network/lib/Ouroboros/Network/PeerSelection/Governor/Types.hs` — PeerSelectionState, PeerSelectionTargets, PeerSelectionPolicy, PeerSelectionActions
- `ouroboros-network/lib/Ouroboros/Network/PeerSelection/Governor/KnownPeers.hs` — cold peer discovery, peer share requests
- `ouroboros-network/lib/Ouroboros/Network/PeerSelection/Governor/EstablishedPeers.hs` — cold→warm promotion, warm→cold demotion
- `ouroboros-network/lib/Ouroboros/Network/PeerSelection/Governor/ActivePeers.hs` — warm→hot promotion, hot→warm demotion
- `ouroboros-network/lib/Ouroboros/Network/PeerSelection/Governor/BigLedgerPeers.hs` — big ledger peer management
- `ouroboros-network/lib/Ouroboros/Network/PeerSelection/Governor/RootPeers.hs` — public root peer fetching
- `ouroboros-network/lib/Ouroboros/Network/PeerSelection/Governor/Monitor.hs` — connection monitoring, target/local roots changes

### State Data Structures
- `ouroboros-network/lib/Ouroboros/Network/PeerSelection/State/KnownPeers.hs` — KnownPeers, KnownPeerInfo (failCount, tepid, sharing)
- `ouroboros-network/lib/Ouroboros/Network/PeerSelection/State/EstablishedPeers.hs` — EstablishedPeers (connections, peer share times, activate times)
- `ouroboros-network/lib/Ouroboros/Network/PeerSelection/State/LocalRootPeers.hs` — LocalRootPeers (groups with HotValency/WarmValency)
- `ouroboros-network/lib/Ouroboros/Network/PeerSelection/PublicRootPeers.hs` — PublicRootPeers (ledger, bigLedger, extra)
- `ouroboros-network/lib/Ouroboros/Network/PeerSelection/Types.hs` — PeerStatus, PeerSource

### Churn
- `ouroboros-network/lib/Ouroboros/Network/PeerSelection/Churn.hs` — generic churn loop
- `cardano-diffusion/lib/Cardano/Network/PeerSelection/Churn.hs` — Cardano-specific churn with mode awareness

### Connection Manager
- `ouroboros-network/framework/lib/Ouroboros/Network/ConnectionManager/Core.hs` — connection state machine, pruning
- `ouroboros-network/framework/lib/Ouroboros/Network/ConnectionManager/State.hs` — ConnectionState constructors
- `ouroboros-network/framework/lib/Ouroboros/Network/ConnectionManager/Types.hs` — AbstractState, OperationResult, DataFlow, Provenance

### Inbound Governor
- `ouroboros-network/framework/lib/Ouroboros/Network/InboundGovernor.hs` — inbound connection management, maturity

### Bridge (Governor → ConnectionManager)
- `ouroboros-network/lib/Ouroboros/Network/PeerSelection/PeerStateActions.hs` — maps governor decisions to actual connection operations

### Peer Sharing
- `ouroboros-network/lib/Ouroboros/Network/PeerSharing.hs` — PeerSharingController, PeerSharingRegistry, computePeerSharingPeers

### Cardano-specific
- `cardano-diffusion/lib/Cardano/Network/PeerSelection/Governor/PeerSelectionState.hs` — ExtraState (ledgerStateJudgement, bootstrapPeersFlag, etc.)
- `cardano-diffusion/lib/Cardano/Network/PeerSelection/Governor/Monitor.hs` — bootstrap monitoring, LSJ transitions, target mode switching
- `cardano-diffusion/lib/Cardano/Network/PeerSelection/ExtraRootPeers.hs` — ExtraPeers (publicConfigPeers, bootstrapPeers)
- `cardano-diffusion/lib/Cardano/Network/Diffusion/Configuration.hs` — default targets, connection limits
- `ouroboros-network/lib/Ouroboros/Network/Diffusion/Configuration.hs` — Praos default targets, churn intervals
- `ouroboros-network/lib/Ouroboros/Network/Diffusion/Policies.hs` — timeout constants, policy functions

## Key Constants

### Default Targets (Praos Relay)
- rootPeers=60, knownPeers=150, establishedPeers=30, activePeers=20
- knownBigLedger=15, establishedBigLedger=10, activeBigLedger=5

### Default Targets (Praos BlockProducer)
- rootPeers=100, knownPeers=100, establishedPeers=30, activePeers=20
- knownBigLedger=15, establishedBigLedger=10, activeBigLedger=5

### Default Targets (Sync Mode - cardano-diffusion)
- rootPeers=0, knownPeers=150, establishedPeers=10, activePeers=5
- knownBigLedger=100, establishedBigLedger=40, activeBigLedger=30

### Churn Intervals
- deadlineChurnInterval=3300s (55min), bulkChurnInterval=900s (15min)
- Churn removes ~20% of peers per cycle: max(0, v - max(u, max(1, v/5)))

### Policy Timeouts
- findPublicRootTimeout=5s, peerShareRetryTime=900s (15min)
- peerShareBatchWaitTime=3s, peerShareOverallTimeout=10s
- peerShareActivationDelay=300s (5min), maxConnectionRetries=5
- clearFailCountDelay=120s (2min)

### Connection Limits
- acceptedConnectionsHardLimit=512, softLimit=384, delay=5s
- deactivateTimeout=300s (5min), closeConnectionTimeout=120s (2min)
- churnEstablishConnectionTimeout=60s

### Peer Sharing
- stickyTime=823s, maxPeersPerResponse=10
- maxInProgressPeerShareReqs=2

### Big Ledger Peers
- bigLedgerPeerQuota=0.9 (top 90% of stake)
- Backoff: exponential 2^n, capped at 2^8 (~4min)

### Cold Peer Retry
- baseColdPeerRetryDiffTime=5s, maxColdPeerRetryBackoff=2^5
- Random fuzz ±2s added

### Inbound Governor
- inboundMaturePeerDelay=15min (900s)
- inactionTimeout=31.4s
