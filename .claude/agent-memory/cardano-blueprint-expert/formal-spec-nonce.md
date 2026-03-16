---
name: Formal Spec Nonce Computation
description: TICKN/UPDN/PRTCL STS rules, nonce derivation, stability window constants, and Conway 4k/f change — from cardano-ledger formal spec and Haskell implementation
type: reference
---

## Source files

- Formal spec (LaTeX): `IntersectMBO/cardano-ledger` `eras/shelley/formal-spec/chain.tex` and `errata.tex`
- Haskell: `libs/cardano-protocol-tpraos/src/Cardano/Protocol/TPraos/Rules/Tickn.hs`, `Updn.hs`, `Prtcl.hs`
- Stability constants: `eras/shelley/impl/src/Cardano/Ledger/Shelley/StabilityWindow.hs`
- Consensus layer: `IntersectMBO/ouroboros-consensus` `ouroboros-consensus-protocol/.../Praos.hs`, `Praos/VRF.hs`
- Initial state: `libs/cardano-protocol-tpraos/src/Cardano/Protocol/TPraos/API.hs` `initialChainDepState`

## Nonces

Four nonces tracked in ChainDepState (TPraos era):
- eta_0: epoch nonce (used for VRF seed)
- eta_v: evolving nonce (accumulates every block)
- eta_c: candidate nonce (freezes at stability boundary)
- eta_h: seed from hash of previous epoch's last block header

In Praos era (Babbage/Conway), PraosState has:
- praosStateEpochNonce
- praosStateEvolvingNonce
- praosStateCandidateNonce
- praosStateLabNonce (hash of current block's prev header)
- praosStateLastEpochBlockNonce (labNonce carried forward at epoch boundary)

## Stability windows

- StabilityWindow = ceil(3k/f) — window for chain growth, protocol updates must be submitted 2*StabilityWindow slots before epoch boundary
- RandomnessStabilisationWindow = ceil(4k/f) — slots before epoch end where candidate nonce freezes

Both in `StabilityWindow.hs`. In Conway, `randomnessStabilisationWindow` = 4k/f (was 3k/f in original Shelley spec intent — see Erratum below).

## UPDN rule

Signal: slot s
State: (eta_v, eta_c)
Input: eta (block VRF nonce)

If s + Duration(stabilityWindow) < firstSlotNextEpoch:  — i.e. NOT yet in freeze zone
  eta_v' = eta_v XOR eta
  eta_c' = eta_v XOR eta   -- candidate stays in sync with evolving
Else:  — inside freeze zone
  eta_v' = eta_v XOR eta   -- evolving still accumulates
  eta_c' = eta_c            -- candidate frozen

Note: eta_c is set to eta_v (not eta_c) in Update-Both — ensures both nonces identical at start of each epoch even after freeze period discards entropy.

**Which window is used in UPDN:**
- Shelley spec as written: `StabilityWindow` (3k/f)
- Shelley implementation (buggy): `stabilityWindow` (3k/f) — same as spec
- Conway implementation: `randomnessStabilisationWindow` (4k/f) — changed for Genesis compatibility

In ouroboros-consensus Praos.hs `reupdateChainDepState`:
```haskell
if slot +* Duration praosRandomnessStabilisationWindow < firstSlotNextEpoch
  then newEvolvingNonce  -- not yet frozen
  else praosStateCandidateNonce cs  -- frozen
```

## TICKN rule

Signal: Bool (newEpoch)
Environment: (pp, eta_c, eta_ph)  -- candidate nonce and prev hash nonce
State: (eta_0, eta_h)

If newEpoch:
  eta_0' = eta_c XOR eta_h XOR eta_e   -- where eta_e = extraEntropy from pp
  eta_h' = eta_ph  -- update stored prev hash
Else:
  no change

In Haskell (Tickn.hs):
```haskell
if newEpoch
  then TicknState { ticknStateEpochNonce = etaC ⭒ etaH ⭒ extraEntropy
                  , ticknStatePrevHashNonce = etaPH }
  else st
```

## PRTCL rule

Calls UPDN then OVERLAY. Updates (cs, eta_v, eta_c).

## CHAIN rule call order

1. prtlSeqChecks (slot ordering, block number, prev hash)
2. TICK (calls NEWEPOCH then RUPD)
3. chainChecks (header/body sizes, protocol version)
4. TICKN (epoch nonce update using candidate nonce + prev hash)
5. PRTCL (calls UPDN then OVERLAY — updates evolving/candidate nonces and validates block)
6. BBODY (validates transactions)

Key: TICKN runs BEFORE PRTCL/UPDN. New epoch nonce computed first, then block's VRF nonce folded in via UPDN.

## prevHashToNonce

In BHeader.hs:
```haskell
prevHashToNonce :: PrevHash -> Nonce
prevHashToNonce GenesisHash = NeutralNonce
prevHashToNonce (BlockHash ph) = hashHeaderToNonce ph

hashHeaderToNonce :: HashHeader -> Nonce
hashHeaderToNonce (HashHeader h) = Nonce $ Hash.castHash h
```

The CHAIN rule computes:
`eta_ph = prevHashToNonce(lastAppliedHash(lab))`
where lab = last applied block. This is the hash of the last APPLIED block's header, passed to TICKN as eta_ph.

Note (Erratum in errata.tex): The implementation uses the penultimate block hash, not the last block hash of the previous epoch — this is intentional per the erratum.

## vrfNonceValue derivation (Praos/Babbage+)

From `Praos/VRF.hs`:
```haskell
vrfNonceValue p certVRF =
  Nonce . castHash . hashWith id . hashToBytes $
    castHash $ hashWith id $ "N" <> getOutputVRFBytes (certifiedOutput certVRF)
```

So: Nonce = Blake2b_256(Blake2b_256("N" || vrfOutput))
This is the double-hashing for range extension.

For TPraos (Shelley-Alonzo), the nonce is `mkNonceFromOutputVRF vrfOutput`:
```haskell
mkNonceFromOutputVRF = Nonce . castHash . hashWith id . getOutputVRFBytes
```
i.e. Blake2b_256(vrfOutput)

## VRF seed construction (mkSeed)

```haskell
mkSeed ucNonce (SlotNo slot) eNonce =
  Seed . XOR(ucNonce_bytes) . castHash . hashWith id $
    word64BE(slot) <> nonce_bytes(eNonce)
```

Two seeds used:
- seedEta = mkNonceFromNumber 0  -- for nonce VRF
- seedL   = mkNonceFromNumber 1  -- for leader VRF

## initialChainDepState

From `cardano-protocol-tpraos/src/Cardano/Protocol/TPraos/API.hs`:
```haskell
initialChainDepState initNonce genDelegs = ChainDepState {
  csProtocol = PrtclState ocertIssueNos initNonce initNonce,  -- eta_v = eta_c = initNonce
  csTickn = TicknState initNonce NeutralNonce,                 -- eta_0 = initNonce, eta_h = NeutralNonce
  csLabNonce = NeutralNonce
}
```

Where initNonce comes from the Shelley genesis config (sgNonce or similar).

## Nonce XOR/combine operator

```haskell
Nonce a ⭒ Nonce b = Nonce (castHash (hashWith id (hashToBytes a <> hashToBytes b)))
x ⭒ NeutralNonce = x
NeutralNonce ⭒ x = x
```

So seedOp is Blake2b_256(a_bytes || b_bytes), with NeutralNonce as identity.

## Erratum: Stability Windows (errata.tex section "Stability Windows")

The formal spec intended:
- UPDN uses RandomnessStabilisationWindow
- RUPD uses StabilityWindow

The implementation had it swapped (UPDN used StabilityWindow). This was not corrected in Shelley/Byron/Mary/Alonzo/Babbage but was fixed in Conway where UPDN now uses 4k/f (RandomnessStabilisationWindow). RUPD still uses 4k/f (which is fine per erratum — anything > StabilityWindow works).

## RUPD (reward update) timing

In Haskell (Rupd.hs): uses `randomnessStabilisationWindow`
- Reward update triggered when: s > firstSlotOfEpoch + RandomnessStabilisationWindow
- This is after the nonce freeze, so the reward calculation begins after the candidate nonce is frozen
