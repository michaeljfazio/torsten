# Block Forging Flow - Haskell Reference

## Key Files
- Forge loop: `ouroboros-consensus-diffusion/src/.../NodeKernel.hs` (forkBlockForging)
- Praos checkIsLeader: `ouroboros-consensus-protocol/src/.../Protocol/Praos.hs`
- VRF input/leader value: `ouroboros-consensus-protocol/src/.../Protocol/Praos/VRF.hs`
- PraosCanBeLeader: `ouroboros-consensus-protocol/src/.../Protocol/Praos/Common.hs`
- Header body: `ouroboros-consensus-protocol/src/.../Protocol/Praos/Header.hs`
- forgePraosFields: in Praos.hs
- forgeShelleyBlock: `ouroboros-consensus-cardano/src/shelley/.../Shelley/Ledger/Forge.hs`
- mkHeader (Praos instance): `ouroboros-consensus-cardano/src/shelley/.../Shelley/Protocol/Praos.hs`
- OCert: `cardano-ledger/libs/cardano-protocol-tpraos/src/.../TPraos/OCert.hs`
- checkLeaderNatValue: `cardano-ledger/libs/cardano-protocol-tpraos/src/.../TPraos/BHeader.hs`
- Block body hash: `cardano-ledger/eras/alonzo/impl/src/.../Alonzo/BlockBody/Internal.hs`
- HotKey KES: `ouroboros-consensus-protocol/src/.../Protocol/Ledger/HotKey.hs`
- Mempool API: `ouroboros-consensus/src/.../Mempool/API.hs`

## VRF Input Construction (Praos)
mkInputVRF slot eNonce = Blake2b-256(slot_u64_BE || nonce_bytes)
- NeutralNonce: only slot bytes (8 bytes), no nonce
- Nonce h: slot_u64_BE (8 bytes) + hash_bytes (32 bytes) = 40 bytes
- Result is InputVRF (a Hash Blake2b_256)

## Block Body Hash (Alonzo+)
hashAlonzoSegWits: hash of 4 concatenated sub-hashes
- hashPart(txBodies) || hashPart(txWits) || hashPart(auxData) || hashPart(isValid)
- hashPart = Blake2b-256 of each serialized component
- Final = Blake2b-256 of the 4 concatenated 32-byte hashes = 128 bytes input

## VRF Domain Separation
- Leader: Blake2b-256("L" || vrf_output_bytes)
- Nonce: Blake2b-256("N" || vrf_output_bytes), then hashed AGAIN to make Nonce

## Header Body Fields (CBOR array, 10 fields)
1. hbBlockNo (BlockNo)
2. hbSlotNo (SlotNo)
3. hbPrev (PrevHash)
4. hbVk (cold VKey)
5. hbVrfVk (VRF verification key)
6. hbVrfRes (CertifiedVRF - [output, proof])
7. hbBodySize (Word32)
8. hbBodyHash (Hash)
9. hbOCert ([kes_vkey, counter, kes_period, sigma])
10. hbProtVer (ProtVer [major, minor])

## OCert Signable Bytes
kes_vkey_raw || counter_u64_BE || kes_period_u64_BE

## KES Signs
HeaderBody CBOR serialization (serialize' using protocol version)

## Dugite Divergence: body hash is WRONG
Dugite computes: Blake2b-256(CBOR_array(tx_bodies))
Haskell computes: Blake2b-256(hash(bodies) || hash(wits) || hash(auxdata) || hash(isvalid))
This is a critical bug for block production compatibility.
