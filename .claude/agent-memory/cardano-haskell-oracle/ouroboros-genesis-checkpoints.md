---
name: Ouroboros Genesis Lightweight Checkpointing
description: Complete architecture of LCP in cardano-node — CheckpointsMap, Genesis features (CSJ/GDD/LoE/LoP/GSM), deployment status, file locations
type: reference
---

## Lightweight Checkpointing (LCP)

LCP is NOT a separate subsystem — it is a simple `Map BlockNo (HeaderHash blk)` validated during header envelope validation. The term "lightweight checkpoint" means these are just (blockNo, hash) pairs — no ledger state, no chain data, just block number to expected hash mappings.

### Key Files
- **CheckpointsMap type**: `ouroboros-consensus/src/ouroboros-consensus/Ouroboros/Consensus/Config.hs` (lines 69-90)
- **validateIfCheckpoint**: `ouroboros-consensus/src/ouroboros-consensus/Ouroboros/Consensus/HeaderValidation.hs` (lines 420-430)
- **Checkpoint JSON parsing**: `cardano-node/src/Cardano/Node/Protocol/Checkpoints.hs`
- **NodeCheckpointsConfiguration**: `cardano-node/src/Cardano/Node/Types.hs` (lines 374-378)
- **CheckpointsFile/Hash**: config keys `CheckpointsFile`, `CheckpointsFileHash`
- **Mainnet checkpoints**: `cardano-node/configuration/cardano/mainnet-checkpoints.json` — 2,767 entries at 2,000-block intervals, block 2000 through ~5,534,000

### Key Types
```haskell
newtype CheckpointsMap blk = CheckpointsMap { unCheckpointsMap :: Map BlockNo (HeaderHash blk) }
data TopLevelConfig blk = TopLevelConfig { ..., topLevelConfigCheckpoints :: !(CheckpointsMap blk) }
```

### Validation Logic
- Called in `validateEnvelope` during header validation (every header received from peers)
- If header's blockNo is in the map AND hash doesn't match -> `CheckpointMismatch` error
- If header's blockNo is NOT in the map -> no check (passes)
- If hash matches -> passes

### Broader Ouroboros Genesis Architecture (5 components)
1. **CSJ (ChainSync Jumping)** — Download headers from one "dynamo" peer, periodically ask "jumpers" if they agree. Jump size = 2*2160 slots (Byron forecast range). Only 2 active header-downloading peers at a time.
2. **GDD (Genesis Density Disconnect)** — Disconnect peers with sparser chains in genesis window. Evaluated per-second rate-limited.
3. **LoE (Limit on Eagerness)** — Prevents selecting blocks past common intersection of all candidate fragments until density is confirmed.
4. **LoP (Limit on Patience)** — Token bucket limiting how long a peer can stall without sending headers. 100K tokens, 500/s leak rate.
5. **GSM (Genesis State Machine)** — Three states: PreSyncing, Syncing, CaughtUp. LoE disabled when CaughtUp.
6. **LCP (Lightweight Checkpoints)** — Static map validated during header envelope checks.

### Deployment Status (cardano-node 10.6.2)
- **Mainnet**: `ConsensusMode: PraosMode` — Genesis features DISABLED, but LCP checkpoints ARE loaded
- **Preview/Preprod**: `ConsensusMode: PraosMode` — No checkpoint files exist for testnets
- LCP validation happens regardless of ConsensusMode (it's part of header validation)
- CSJ/GDD/LoE/LoP only activate when `ConsensusMode: GenesisMode`
- Genesis mode is NOT yet enabled on mainnet (still PraosMode as of 10.6.2)

### Config JSON Keys
```json
{
  "CheckpointsFile": "mainnet-checkpoints.json",
  "CheckpointsFileHash": "3e6dee5bae7acc6d870187e72674b37c929be8c66e62a552cf6a876b1af31ade",
  "ConsensusMode": "PraosMode",
  "LowLevelGenesisOptions": { "EnableCSJ": true, "EnableLoEAndGDD": true, "EnableLoP": true, ... }
}
```
