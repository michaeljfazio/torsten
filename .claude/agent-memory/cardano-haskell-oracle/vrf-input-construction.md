# VRF Input Construction: TPraos vs Praos

## Critical Difference: Two Protocols

### TPraos (Shelley/Allegra/Mary) — TWO VRF proofs per block
- File: `cardano-ledger/libs/cardano-protocol-tpraos/src/Cardano/Protocol/TPraos/BHeader.hs`
- BHBody has two VRF fields: `bheaderEta` (nonce) and `bheaderL` (leader)
- Uses `mkSeed` with domain separators `seedEta = mkNonceFromNumber 0` and `seedL = mkNonceFromNumber 1`
- mkSeed: hash(slot_be64 || epoch_nonce_bytes) XOR ucNonce

### Praos (Alonzo+) — ONE VRF proof per block, domain-separated post-hoc
- File: `ouroboros-consensus-protocol/src/.../Praos/VRF.hs`
- HeaderBody has single `hbVrfRes :: CertifiedVRF (VRF c) InputVRF`
- Uses `mkInputVRF` — NO domain separator, NO XOR
- Leader vs nonce values derived AFTER by hashing VRF output with "L" or "N" prefix

## mkInputVRF (Praos) — THE CORRECT ONE FOR CONWAY
```haskell
mkInputVRF (SlotNo slot) eNonce =
  InputVRF . Hash.castHash . Hash.hashWith id . runByteBuilder (8 + 32)
    $ BS.word64BE slot
      <> (case eNonce of
            NeutralNonce -> mempty
            Nonce h -> BS.byteStringCopy (Hash.hashToBytes h))
```
**Input = Blake2b-256(slot_u64_BE || epoch_nonce_32_bytes)**
- Slot is 8-byte big-endian u64 (NOT CBOR-encoded)
- Epoch nonce is raw 32 bytes (or empty if NeutralNonce)
- Result is 32-byte Blake2b-256 hash
- This hash IS the VRF input (signed by VRF key)

## Domain Separation (Praos) — applied to VRF OUTPUT, not input
```haskell
hashVRF _ use certVRF =
  let vrfOutputAsBytes = getOutputVRFBytes $ certifiedOutput certVRF
  in case use of
       SVRFLeader -> castHash $ hashWith id $ "L" <> vrfOutputAsBytes
       SVRFNonce  -> castHash $ hashWith id $ "N" <> vrfOutputAsBytes
```
- Leader value: Blake2b-256("L" || vrf_output_bytes)
- Nonce value: Blake2b-256("N" || vrf_output_bytes)

## Leader Check (Praos)
- `vrfLeaderValue`: hash with "L", convert to BoundedNatural, check < threshold
- `checkLeaderNatValue`: value/max < 1-(1-f)^sigma

## Nonce Contribution (Praos)
- `vrfNonceValue`: hash with "N", then hash AGAIN with Blake2b-256, wrap as Nonce
- Double hash: first is domain separation, second converts to Nonce type

## Torsten Bug (as of 2026-03-09)
Torsten's `vrf_input()` in both `slot_leader.rs` and `praos.rs` does:
```rust
data = epoch_nonce || slot_be64  // WRONG ORDER
```
Should be:
```rust
data = Blake2b256(slot_be64 || epoch_nonce)  // Correct: hash of (slot || nonce)
```
Two bugs:
1. Order is reversed (nonce||slot vs slot||nonce)
2. Missing Blake2b-256 hash — Haskell hashes the concatenation, Torsten uses raw concat
3. Missing domain-separated leader/nonce value extraction from VRF output
