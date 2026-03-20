---
name: Block Forging Deep Dive
description: Complete analysis of Haskell block forging — header structure, VRF inputs, KES signing, body hash, opcert, tx selection, TPraos vs Praos differences
type: reference
---

# Block Forging Deep Dive

## Key Source Files

### ouroboros-consensus
- `ouroboros-consensus-diffusion/src/.../NodeKernel.hs` — Top-level forge loop: slot tick, leader check, mempool snapshot, forgeBlock, add to ChainDB
- `ouroboros-consensus-cardano/src/shelley/.../Ledger/Forge.hs` — `forgeShelleyBlock`: assembles body, calls mkHeader, wraps as ShelleyBlock
- `ouroboros-consensus-cardano/src/shelley/.../Protocol/Praos.hs` — Praos `mkHeader` impl, `verifyHeaderIntegrity`
- `ouroboros-consensus-cardano/src/shelley/.../Protocol/TPraos.hs` — TPraos `mkHeader` impl with two VRF calls
- `ouroboros-consensus-protocol/src/.../Protocol/Praos.hs` — `forgePraosFields`, `checkIsLeader`, `reupdateChainDepState` (nonce update)
- `ouroboros-consensus-protocol/src/.../Protocol/Praos/Header.hs` — Praos `HeaderBody` type, CBOR encoding
- `ouroboros-consensus-protocol/src/.../Protocol/Praos/VRF.hs` — `mkInputVRF`, `vrfLeaderValue`, `vrfNonceValue`, range extension

### cardano-ledger
- `libs/cardano-protocol-tpraos/src/.../TPraos/BHeader.hs` — TPraos `BHBody` type, `mkSeed`, `seedEta`/`seedL`, `checkLeaderValue`
- `libs/cardano-protocol-tpraos/src/.../TPraos/OCert.hs` — `OCert` type, `OCertSignable`, signable representation
- `eras/alonzo/impl/src/.../Alonzo/BlockBody/Internal.hs` — `AlonzoBlockBody`, `hashAlonzoSegWits` (4 components)
- `eras/shelley/impl/src/.../Shelley/BlockBody/Internal.hs` — `ShelleyBlockBody`, `hashShelleySegWits` (3 components)
- `libs/cardano-ledger-core/src/Cardano/Ledger/Core.hs` — `bBodySize` = serialize' + length

## TPraos vs Praos VRF Differences

### TPraos (Shelley through Alonzo, proto < 7)
- TWO separate VRF evaluations with different inputs:
  - `bheaderEta` (nonce VRF): `VRF.evalCertified(mkSeed(seedEta, slot, epochNonce), vrfKey)`
  - `bheaderL` (leader VRF): `VRF.evalCertified(mkSeed(seedL, slot, epochNonce), vrfKey)`
- `seedEta = mkNonceFromNumber 0` → `Nonce(blake2b_256(word64BE(0)))`
- `seedL = mkNonceFromNumber 1` → `Nonce(blake2b_256(word64BE(1)))`
- `mkSeed(ucNonce, slot, eNonce)` = `ucNonce XOR castHash(blake2b_256(word64BE(slot) ++ hashToBytes(eNonce)))`
- Leader check: raw VRF output bytes → `getOutputVRFNatural` → certNatMax = 2^(8*64) = 2^512
- Nonce contribution: `mkNonceFromOutputVRF(certifiedOutput(bheaderEta))`

### Praos (Babbage/Conway, proto >= 7)
- ONE VRF evaluation with unified input:
  - `hbVrfRes`: `VRF.evalCertified(mkInputVRF(slot, epochNonce), vrfKey)`
- `mkInputVRF(slot, epochNonce)` = `blake2b_256(word64BE(slot) ++ hashToBytes(epochNonce))`
  - NeutralNonce contributes empty bytes (just the slot)
- Range extension for leader check: `blake2b_256("L" ++ vrfOutputBytes)` → certNatMax = 2^256
- Range extension for nonce: `blake2b_256("N" ++ vrfOutputBytes)` → then `Nonce(blake2b_256(result))`
  - Double hash: first hash is range extension, second is nonce construction
- **Key difference**: Praos uses blake2b_256 hash of VRF output (32 bytes, certNatMax=2^256), TPraos uses raw VRF output (64 bytes, certNatMax=2^512)

## Header Body Structure (Praos — Babbage/Conway)

```
HeaderBody = array(N) where N = 10 + inline_fields_from_OCert + inline_fields_from_ProtVer
```

Fields in order (from `HeaderBody` CBOR encoding):
1. `hbBlockNo` — block number (uint)
2. `hbSlotNo` — slot number (uint)
3. `hbPrev` — previous block header hash (null for genesis, bytes(32) otherwise)
4. `hbVk` — issuer verification key = **cold VKey** (bytes(32))
5. `hbVrfVk` — VRF verification key (bytes(32))
6. `hbVrfRes` — certified VRF result = `[output(64 bytes), proof(80 bytes)]`
7. `hbBodySize` — body size in bytes (uint32)
8. `hbBodyHash` — block body hash (bytes(32))
9-12. `hbOCert` — operational certificate (INLINE as CBORGroup, 4 fields)
13-14. `hbProtVer` — protocol version (INLINE as CBORGroup, 2 fields: major, minor)

**Total CBOR array length**: **array(10)** for Praos — OCert and ProtVer are **nested** arrays

Both OCert and ProtVer derive `EncCBOR via (CBORGroup T)`, so `encCBOR` on them produces:
- OCert → `array(4)[kes_vkey, counter, kes_period, sigma]`
- ProtVer → `array(2)[major, minor]`

So the Praos HeaderBody = `array(10)[blockno, slot, prev, vkey, vrfvkey, vrfres, bodysize, bodyhash, array(4)[...], array(2)[...]]`

On the decode side, OCert uses `mapCoder unCBORGroup From` which unwraps the CBORGroup wrapper.

### TPraos Header Body (Shelley through Alonzo)

Encodes with INLINE groups (not nested):
1. block_number
2. slot
3. prev_hash
4. issuer_vkey (cold VKey)
5. vrf_vkey
6. **bheaderEta** — nonce VRF: `CertifiedVRF Nonce` = `[output, proof]`
7. **bheaderL** — leader VRF: `CertifiedVRF Natural` = `[output, proof]`
8. body_size
9. body_hash
10. opcert.kes_vkey (INLINE field 1 of 4)
11. opcert.counter (INLINE field 2)
12. opcert.kes_period (INLINE field 3)
13. opcert.sigma (INLINE field 4)
14. protver.major (INLINE field 1 of 2)
15. protver.minor (INLINE field 2)

**Total**: `array(15)` = 9 base + 4 (OCert inline) + 2 (ProtVer inline)

The encoding explicitly uses `encCBORGroup oc` and `encCBORGroup pv` which produce raw fields without array wrappers.

**CRITICAL**: TPraos = flat array(15) with inline OCert+ProtVer. Praos = array(10) with nested OCert+ProtVer sub-arrays.

## Full Header = [HeaderBody, KES_Signature]

```
Header = array(2) [header_body, kes_signature]
```

For both TPraos and Praos:
- `BHeaderRaw` / `HeaderRaw` = `array(2)[body, kes_sig]`
- The header is wrapped in `MemoBytes` which memoizes the CBOR encoding
- Header hash = blake2b_256 of the **entire serialized header** (both body + sig)

## What Gets KES-Signed

**The KES signature is over the CBOR-serialized header body.**

From `SignableRepresentation`:
- TPraos: `getSignableRepresentation bh = serialize' (pvMajor (bprotver bh)) bh`
- Praos: `getSignableRepresentation hb = serialize' (pvMajor (hbProtVer hb)) hb`

Both serialize the header body using `serialize'` with the current protocol version, producing raw bytes. The KES signature is computed over these bytes.

**KES period offset** = `(current_slot / slots_per_kes_period) - opcert_kes_start_period`

## Operational Certificate

```haskell
data OCert c = OCert
  { ocertVkHot :: VerKeyKES (KES c)    -- KES verification (hot) key
  , ocertN :: Word64                     -- Counter
  , ocertKESPeriod :: KESPeriod          -- Start KES period
  , ocertSigma :: SignedDSIGN DSIGN (OCertSignable c)  -- Cold key signature
  }
```

**OCert Signable** (raw bytes, NOT CBOR):
```
rawSerialiseVerKeyKES(vkHot) ++ word64BE(counter) ++ word64BE(kesPeriod)
```
This is 32 + 8 + 8 = 48 bytes for Sum6Kes (VerKeyKES = 32 bytes).

The signature `ocertSigma` is an Ed25519 signature by the **cold signing key** over these raw bytes.

**CBOR encoding**: As CBORGroup (inline), 4 fields: `[kes_vkey_bytes, counter, kes_period, sigma_bytes]`

## Block Body Hash

### Shelley/Allegra/Mary (3 components):
```
hashShelleySegWits bodies wits md =
  blake2b_256(blake2b_256(bodies) ++ blake2b_256(wits) ++ blake2b_256(md))
```

### Alonzo/Babbage/Conway (4 components):
```
hashAlonzoSegWits bodies wits auxData isValids =
  blake2b_256(blake2b_256(bodies) ++ blake2b_256(wits) ++ blake2b_256(auxData) ++ blake2b_256(isValids))
```

Each component is the CBOR-serialized form:
- `bodies` = CBOR array of transaction bodies (each body's `originalBytes`)
- `wits` = CBOR array of witness sets (each witness set's `originalBytes`)
- `auxData` = CBOR map of {tx_index: auxiliary_data} (only for txs that have aux data)
- `isValids` = CBOR-encoded list of indices of invalid transactions

**CRITICAL**: The body bytes used are the `originalBytes` (pre-encoded) of each component, NOT re-serialized. When forging, the pattern constructor `AlonzoBlockBody` calls `serialize` with `encodePreEncoded` on the original bytes.

## Body Size

`bBodySize protVer body = BS.length (serialize' (pvMajor protVer) (encCBORGroup body))`

This is the length of the CBOR encoding of ALL body components concatenated (without outer array wrapper since encCBORGroup is used). For Alonzo+: bodies ++ wits ++ auxData ++ isValids bytes concatenated.

## Block Number and Previous Hash

- `blockNo` = previous block's blockNo + 1 (passed in from NodeKernel)
- `prevHash` = hash of the **previous block's header** (not the full block)
  - Specifically: `getTipHash tickedLedger` → `ShelleyHash` → `HashHeader` → `PrevHash`
  - `HashHeader` wraps `Hash HASH EraIndependentBlockHeader` (blake2b_256 of header bytes)
  - For genesis: `GenesisHash` encoded as CBOR null

## issuer_vkey

**The pool's cold verification key** (Ed25519), NOT the KES hot key. The cold vkey is stored in `praosCanBeLeaderColdVerKey` / `bheaderVk` / `hbVk`. Pool ID = blake2b_224(cold_vkey).

## Protocol Version in Header

From `BlockConfig`: `shelleyProtocolVersion` — this is the **highest protocol version this node supports**, set at node configuration time (not from on-chain protocol parameters). For Conway it's typically `ProtVer 10 0`.

## VRF Key in Header

Yes, `hbVrfVk` / `bheaderVrfVk` is the **VRF verification (public) key**, derived from the VRF signing key via `VRF.deriveVerKeyVRF`.

## Transaction Selection from Mempool

1. `getSnapshotFor mempool currentSlot tickedLedgerState` — revalidates mempool txs against the ticked ledger state for the forging slot
2. `snapshotTake mempoolSnapshot (blockCapacityTxMeasure cfg tickedLedgerState)` — takes the greatest prefix of txs (FIFO order by TicketNo) that fits within the block capacity
3. **Ordering**: Strict FIFO — transactions are in insertion order (TicketNo). No sorting by fee or priority.
4. **Capacity**: `TxMeasure` typically includes byte size and (for Alonzo+) execution units. `splitAfterTxSize` accumulates until the measure exceeds the block capacity.
5. The capacity comes from protocol parameters: maxBlockBodySize, maxBlockExUnits.

## Epoch Nonce Computation

Maintained in `PraosState`:
- `praosStateEvolvingNonce`: updated every block with `evolvingNonce ⭒ vrfNonceValue(vrfRes)`
- `praosStateCandidateNonce`: snapshot of evolving nonce, frozen at `randomnessStabilisationWindow` before epoch end
- `praosStateEpochNonce`: set at epoch boundary = `candidateNonce ⭒ lastEpochBlockNonce`
- `praosStateLastEpochBlockNonce`: set at epoch boundary from `praosStateLabNonce`
- `praosStateLabNonce`: `prevHashToNonce(prevHash)` — nonce from hash of previous block header

**Epoch nonce transition** (at epoch boundary):
```
epochNonce_new = candidateNonce_prev ⭒ lastEpochBlockNonce_prev
lastEpochBlockNonce_new = labNonce_prev  (i.e., hash of last block's prev_hash)
```

The `⭒` operator: `NeutralNonce ⭒ x = x`, `x ⭒ NeutralNonce = x`, `Nonce a ⭒ Nonce b = Nonce(blake2b_256(a ++ b))`

**Randomness stabilisation window**: blocks within this window before epoch end do NOT update the candidateNonce, only the evolvingNonce. This ensures the epoch nonce is determined well before the epoch starts.
