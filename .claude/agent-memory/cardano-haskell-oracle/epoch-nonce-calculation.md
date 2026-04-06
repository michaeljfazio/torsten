# Epoch Nonce Calculation (Praos) - Definitive Reference

## Source Files (ouroboros-consensus main branch, cardano-ledger master branch)
- PraosState + tickChainDepState + reupdateChainDepState: `ouroboros-consensus-protocol/src/ouroboros-consensus-protocol/Ouroboros/Consensus/Protocol/Praos.hs`
- VRF nonce/leader domain separation: `ouroboros-consensus-protocol/src/ouroboros-consensus-protocol/Ouroboros/Consensus/Protocol/Praos/VRF.hs`
- Nonce type + ⭒ operator: `cardano-ledger/libs/cardano-ledger-core/src/Cardano/Ledger/BaseTypes.hs`
- prevHashToNonce: `cardano-ledger/libs/cardano-protocol-tpraos/src/Cardano/Protocol/TPraos/BHeader.hs`
- TPraos ChainDepState + initialChainDepState: `cardano-ledger/libs/cardano-protocol-tpraos/src/Cardano/Protocol/TPraos/API.hs`
- TPraos TICKN rule: `cardano-ledger/libs/cardano-protocol-tpraos/src/Cardano/Protocol/TPraos/Rules/Tickn.hs`
- TPraos PrtclState: `cardano-ledger/libs/cardano-protocol-tpraos/src/Cardano/Protocol/TPraos/Rules/Prtcl.hs`
- genesisHashToPraosNonce: `cardano-node/cardano-node/src/Cardano/Node/Protocol/Shelley.hs`
- Byron→Shelley translation: `ouroboros-consensus-cardano/src/ouroboros-consensus-cardano/Ouroboros/Consensus/Cardano/CanHardFork.hs`
- TPraos→Praos translation: `ouroboros-consensus-protocol/src/.../Protocol/Praos.hs` (TranslateProto instance, line 755)
- ProtocolInfo construction: `ouroboros-consensus-cardano/src/shelley/Ouroboros/Consensus/Shelley/Node/TPraos.hs`

## PraosState — Version History (CRITICAL FOR CBOR ENCODING)

### Version on main branch (unreleased as of 2026-04-06)
8 fields, `encodeListLen 8` (array(8)).  Added `praosStatePreviousEpochNonce` for Peras in commit
`5598d9fbbb67` (2025-10-29, "Store previous epoch nonce in PraosState").

### Version in released ouroboros-consensus-protocol-0.13.0.0 (shipped with cardano-node 10.6.2 / 10.7.0)
7 fields, `encodeListLen 7` (array(7)).  NO `praosStatePreviousEpochNonce` field.

**cardano-cli 10.15 uses the released 7-field / array(7) encoding.
Sending array(8) causes the client to reject DebugChainDepState responses.**

```haskell
-- Released 0.13.0.0 (cardano-node 10.6.2):
data PraosState = PraosState
  { praosStateLastSlot              :: !(WithOrigin SlotNo)
  , praosStateOCertCounters         :: !(Map (KeyHash BlockIssuer) Word64)
  , praosStateEvolvingNonce         :: !Nonce    -- eta_v, updated every block, never reset
  , praosStateCandidateNonce        :: !Nonce    -- eta_c, frozen 4k/f before epoch end
  , praosStateEpochNonce            :: !Nonce    -- eta_0, recomputed at epoch boundary
  -- NO previousEpochNonce here
  , praosStateLabNonce              :: !Nonce    -- hash of parent block of last applied block
  , praosStateLastEpochBlockNonce   :: !Nonce    -- labNonce snapshot at epoch boundary
  }

-- main branch (unreleased, for Peras):
data PraosState = PraosState
  { praosStateLastSlot              :: !(WithOrigin SlotNo)
  , praosStateOCertCounters         :: !(Map (KeyHash BlockIssuer) Word64)
  , praosStateEvolvingNonce         :: !Nonce
  , praosStateCandidateNonce        :: !Nonce
  , praosStateEpochNonce            :: !Nonce
  , praosStatePreviousEpochNonce    :: !Nonce    -- added for Peras
  , praosStateLabNonce              :: !Nonce
  , praosStateLastEpochBlockNonce   :: !Nonce
  }
```

Field order in the released 7-field array(7):
  [0] lastSlot, [1] ocertCounters, [2] evolvingNonce, [3] candidateNonce,
  [4] epochNonce, [5] labNonce, [6] lastEpochBlockNonce

## TPraos ChainDepState (Shelley through Alonzo)
```haskell
data ChainDepState = ChainDepState
  { csProtocol :: !PrtclState       -- PrtclState ocertCounters evolvingNonce candidateNonce
  , csTickn    :: !TicknState        -- TicknState epochNonce prevHashNonce
  , csLabNonce :: !Nonce
  }
```

## Initial Values

### initialNonce = Blake2b-256 of raw Shelley genesis JSON file bytes
```haskell
-- cardano-node/Protocol/Shelley.hs:
genesisHashToPraosNonce (GenesisHash h) = Nonce (Crypto.castHash h)
-- GenesisHash = Crypto.hashWith id genesisBytes  -- raw file content hash
```

### initialChainDepState (API.hs:388-405)
```haskell
initialChainDepState initNonce genDelegs = ChainDepState
  { csProtocol = PrtclState ocertIssueNos initNonce initNonce
  , csTickn    = TicknState initNonce NeutralNonce
  , csLabNonce = NeutralNonce
  }
```

| Field | Initial Value |
|-------|--------------|
| evolvingNonce | initNonce (genesis file hash) |
| candidateNonce | initNonce (genesis file hash) |
| epochNonce | initNonce (genesis file hash) |
| labNonce | NeutralNonce |
| lastEpochBlockNonce (prevHashNonce) | NeutralNonce |
| previousEpochNonce | N/A in TPraos; at TPraos→Praos transition = epochNonce |

### Byron→Shelley HFC translation (CanHardFork.hs:359-384)
Same pattern: evolving=nonce, candidate=nonce, epochNonce=nonce, prevHashNonce=NeutralNonce, labNonce=NeutralNonce

## The ⭒ Operator
```haskell
Nonce a ⭒ Nonce b = Nonce (Blake2b_256(a_bytes || b_bytes))
x ⭒ NeutralNonce = x
NeutralNonce ⭒ x = x
```

## Per-Block Update: Praos (reupdateChainDepState)
```
eta = vrfNonceValue(vrf_output) = Nonce(Blake2b_256(Blake2b_256("N" || raw_vrf_output_bytes)))
newEvolvingNonce = evolvingNonce ⭒ eta
candidateNonce = if slot + randomnessStabilisationWindow < firstSlotNextEpoch
                 then newEvolvingNonce
                 else candidateNonce  -- FROZEN
labNonce = prevHashToNonce(block.prevHash)
         -- GenesisHash → NeutralNonce
         -- BlockHash h → Nonce(castHash h)  -- just type reinterpret, no rehash
```

## Epoch Boundary: Praos (tickChainDepState)
```
epochNonce = candidateNonce ⭒ lastEpochBlockNonce     -- TWO terms only
previousEpochNonce = old epochNonce
lastEpochBlockNonce = labNonce                         -- snapshot for next transition
-- evolvingNonce and candidateNonce CARRY FORWARD unchanged
```

## Epoch Boundary: TPraos (TICKN rule, Tickn.hs:89-99)
```
epochNonce = candidateNonce ⭒ prevHashNonce ⭒ extraEntropy   -- THREE terms
prevHashNonce = csLabNonce                                    -- snapshot
```
extraEntropy = from protocolParams.extraEntropy (NeutralNonce on all real networks)

## Stability Windows — ERA-DEPENDENT

The candidate nonce freeze window differs by era:

| Era | Protocol | Field used | Formula | Preview (k=432,f=0.05) | Mainnet (k=2160,f=0.05) |
|-----|----------|-----------|---------|------------------------|--------------------------|
| Shelley-Alonzo | TPraos | Globals.stabilityWindow | ceiling(3k/f) | 25920 | 129600 |
| Babbage | Praos | praosRandomnessStabilisationWindow (OVERRIDDEN) | ceiling(3k/f) | 25920 | 129600 |
| Conway+ | Praos | praosRandomnessStabilisationWindow (default) | ceiling(4k/f) | 34560 | 172800 |

Source: ouroboros-consensus-cardano/.../Consensus/Cardano/Node.hs
- Default praosParams (line 694-699): uses computeRandomnessStabilisationWindow = 4k/f
- partialConsensusConfigBabbage (line 792-802): OVERRIDES to computeStabilityWindow = 3k/f
  Comment: "For Praos in Babbage (just as in all TPraos eras) we use the smaller (3k/f vs 4k/f)
  stability window here for backwards-compatibility. See erratum 17.3 in the Shelley ledger specs."
- partialConsensusConfigConway (line 821): uses default praosParams = 4k/f

TPraos UPDN rule (Updn.hs line 64): `sp <- liftSTS $ asks stabilityWindow` reads Globals.stabilityWindow = 3k/f
Praos reupdateChainDepState (Praos.hs line 503): uses PraosParams.praosRandomnessStabilisationWindow

IMPORTANT: Dugite currently uses 4k/f unconditionally. Correct for Conway, WRONG for Babbage and earlier.

## Candidate Nonce Semantics: TRACK EARLY, FREEZE LATE

The candidate nonce UPDATES (tracks evolving) early in epoch, and FREEZES near epoch end:
- slot + window < firstSlotNextEpoch → candidate = newEvolvingNonce (TRACKING)
- slot + window >= firstSlotNextEpoch → candidate = old candidate (FROZEN)

Freeze point = firstSlotNextEpoch - window. Before that: candidate tracks. After that: candidate frozen.

## prevHashToNonce
```haskell
prevHashToNonce GenesisHash    = NeutralNonce
prevHashToNonce (BlockHash ph) = Nonce (castHash ph)  -- type cast only, no rehashing
```
