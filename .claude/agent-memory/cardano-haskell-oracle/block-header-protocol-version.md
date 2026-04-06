---
name: Block Header Protocol Version
description: How cardano-node stamps ProtVer in forged block headers — source location, flow, per-release values, config override, and distinction from on-chain protocol version
type: reference
---

# Block Header Protocol Version in Forged Blocks

## Source of Truth

Single hardcoded line in cardano-node:
**`cardano-node/src/Cardano/Node/Protocol/Cardano.hs`**

```haskell
, Consensus.cardanoProtocolVersion = if npcExperimentalHardForksEnabled
                                     then ProtVer (natVersion @11) 0
                                     else ProtVer (natVersion @10) 8
```

(master branch, as of 2026-04-06; latest release 10.6.3 uses 10,7 — see table below)

## Per-Release Values (Normal Mode, `ExperimentalHardForksEnabled: false`)

| Release   | ProtVer (major, minor) | Notes                      |
|-----------|------------------------|----------------------------|
| 10.1.4    | 10, 2                  |                            |
| 10.2.1    | 10, 3                  |                            |
| 10.3.1    | 10, 3                  |                            |
| 10.4.1    | 10, 3                  |                            |
| 10.5.0    | 10, 3                  |                            |
| 10.5.1    | 10, 3                  |                            |
| 10.5.2    | 10, 3                  |                            |
| 10.5.3    | 10, 6                  | Plomin-era update          |
| 10.5.4    | 10, 6                  |                            |
| 10.6.0    | 10, 3                  | (reverted; odd)            |
| 10.6.1    | 10, 7                  |                            |
| 10.6.2    | 10, 7                  | Experimental branch added  |
| 10.6.3    | 10, 7                  |                            |
| 10.7.0    | 10, 8                  |                            |
| master    | 10, 8                  |                            |

Experimental mode (`ExperimentalHardForksEnabled: true`): always `ProtVer 11 0` from 10.6.2 onward.

## Config-Based Override

`ExperimentalHardForksEnabled` is a boolean field in the node config JSON:
```json
{ "ExperimentalHardForksEnabled": true }
```
Parsed in `cardano-node/src/Cardano/Node/Configuration/POM.hs` via `v .:? "ExperimentalHardForksEnabled"` (default: `false`). Old name `TestEnableDevelopmentHardForkEras` also accepted. This is the ONLY runtime override path; the version itself is not configurable as a raw number.

## Call Chain (Forging Path)

1. `mkSomeConsensusProtocolCardano` in `Cardano/Node/Protocol/Cardano.hs`
   → sets `Consensus.cardanoProtocolVersion :: ProtVer`

2. `protocolInfoCardano` in `ouroboros-consensus-cardano/src/ouroboros-consensus-cardano/Ouroboros/Consensus/Cardano/Node.hs`
   → extracts `cardanoProtocolVersion`
   → derives `maxMajorProtVer = MaxMajorProtVer $ pvMajor cardanoProtocolVersion`
   → passes `cardanoProtocolVersion` into `Shelley.mkShelleyBlockConfig` for ALL eras (Shelley, Allegra, Mary, Alonzo, Babbage, Conway, Dijkstra)

3. `mkShelleyBlockConfig` in `ouroboros-consensus-cardano/src/shelley/Ouroboros/Consensus/Shelley/Ledger/Config.hs`
   → stores it as `shelleyProtocolVersion :: SL.ProtVer` in `BlockConfig (ShelleyBlock proto era)`

4. `forgeShelleyBlock` in `ouroboros-consensus-cardano/src/shelley/Ouroboros/Consensus/Shelley/Ledger/Forge.hs`
   → reads `protocolVersion = shelleyProtocolVersion $ configBlock cfg`
   → passes it to `mkHeader` and to `SL.bBodySize protocolVersion body`
   → stamped directly in the block header body

## Two Consequences of cardanoProtocolVersion

1. **Public signaling**: The `ProtVer` value is stamped in every forged block header as an upgrade signal. SPOs advertise that they are running software capable of handling up to this protocol version.

2. **Obsolete node check** (`envelopeChecks` in `ouroboros-consensus-cardano/src/shelley/Ouroboros/Consensus/Shelley/Protocol/Praos.hs`):
   ```haskell
   unless (m <= maxpv) $ throwError (ObsoleteNode m maxpv)
   ```
   Where `maxpv = pvMajor cardanoProtocolVersion` and `m = pvMajor (lvProtocolVersion lv)` is the current ledger protocol version. If the on-chain protocol version's MAJOR component exceeds the node's configured maximum, the node rejects ALL incoming block headers as `ObsoleteNode`. This forces operators to upgrade.

## Distinction from On-Chain Protocol Version

| Property              | `cardanoProtocolVersion` (node config) | `ProtocolParams.protocolVersion` (on-chain) |
|-----------------------|----------------------------------------|---------------------------------------------|
| Where set             | Hardcoded in `Cardano.hs` source       | Updated by on-chain governance votes        |
| What it controls      | What the node stamps in blocks it forges; max version node will accept | The active ledger rule set; used for hard fork triggers |
| Who can change it     | Software update + node operator restart | Conway governance (HardForkInitiation action) |
| Current mainnet value | 10,8 (10.7.0 nodes)                    | 10,0 (post-Plomin, before intra-era bump)   |
| Checked where         | `envelopeChecks` during header validation | Ledger rules (e.g., transaction validation, era transitions) |

The on-chain `ProtVer` is what the ledger currently enforces. The node's `cardanoProtocolVersion` is the node's declaration of what it supports. They are deliberately decoupled: the node can advertise support for a higher version before on-chain governance enacts it, and can serve as an upgrade signal.
