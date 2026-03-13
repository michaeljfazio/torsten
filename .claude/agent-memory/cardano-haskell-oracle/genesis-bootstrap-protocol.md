---
name: genesis-bootstrap-protocol
description: Ouroboros Genesis bootstrap implementation — GSM state machine, LoE/GDD, CSJ, bootstrap peers, peer targets, FetchMode, caught-up detection
type: reference
---

# Ouroboros Genesis Bootstrap Protocol Implementation

## Key Files

### GSM (Genesis State Machine)
- GsmState type: `ouroboros-consensus/src/.../Node/GsmState.hs` — PreSyncing | Syncing | CaughtUp
- GSM logic: `ouroboros-consensus-diffusion/src/.../Node/GSM.hs` — state transitions, marker file, durationUntilTooOld
- Genesis config: `ouroboros-consensus-diffusion/src/.../Node/Genesis.hs` — GenesisConfig, LoEAndGDDConfig, CSJConfig, LoP
- NodeKernel wiring: `ouroboros-consensus-diffusion/src/.../NodeKernel.hs` — lines 310-390

### Network Layer
- ConsensusMode: `cardano-diffusion/api/lib/Cardano/Network/ConsensusMode.hs` — GenesisMode | PraosMode
- LedgerStateJudgement: `cardano-diffusion/api/lib/Cardano/Network/LedgerStateJudgement.hs` — YoungEnough | TooOld
- Bootstrap peers: `cardano-diffusion/api/lib/Cardano/Network/PeerSelection/Bootstrap.hs` — UseBootstrapPeers, requiresBootstrapPeers
- FetchMode: `cardano-diffusion/api/lib/Cardano/Network/FetchMode.hs` — FetchModeGenesis | PraosFetchMode
- OutboundConnectionsState: `cardano-diffusion/lib/Cardano/Network/PeerSelection/Governor/Types.hs` — TrustedStateWithExternalPeers | UntrustedState
- Churn governor: `cardano-diffusion/lib/Cardano/Network/PeerSelection/Churn.hs`
- Sync targets: `cardano-diffusion/lib/Cardano/Network/Diffusion/Configuration.hs` — defaultSyncTargets

### Genesis-specific Components
- GDD (Genesis Density Disconnector): `ouroboros-consensus/src/.../Genesis/Governor.hs` — densityDisconnect, gddWatcher
- CSJ (ChainSync Jumping): `ouroboros-consensus/src/.../ChainSync/Client/Jumping.hs`
- Genesis block fetch: `ouroboros-network/lib/.../BlockFetch/Decision/Genesis.hs` — fetchDecisionsGenesisM
- LoE (Limit on Eagerness): configured via ChainDB.cdbsLoE, managed by setGetLoEFragment

### Topology
- TopologyP2P: `cardano-node/src/.../Configuration/TopologyP2P.hs` — bootstrapPeers field, trustable flag

## Constants
- maxCaughtUpAge: 20 minutes (1200s)
- defaultSyncTargets: 40 established BLPs, 30 active BLPs, 10 established, 5 active
- defaultNumBootstrapPeers: 30
- minNumberOfBigLedgerPeers (for HAA): 5
- CSJ default jump size: 2*2160 = 4320 slots (Byron forecast range)
- LoP bucket: 100K tokens, 500 tokens/sec leak rate
- GDD rate limit: 1.0s
- BlockFetch grace period: 10s
- Anti-thundering-herd: random 0-300s bonus on CaughtUp→PreSyncing transition
