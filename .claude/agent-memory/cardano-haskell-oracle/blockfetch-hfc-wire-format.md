---
name: BlockFetch N2N HFC wire format
description: Exact CBOR structure for MsgBlock in N2N BlockFetch — critical distinction between full blocks and headers
type: reference
---

## BlockFetch MsgBlock Wire Format (N2N)

The Haskell `SerialiseNodeToNode` instance for `HardForkBlock` for **full blocks**:

```haskell
-- ouroboros-consensus/.../HardFork/Combinator/Serialisation/SerialiseNodeToNode.hs
instance SerialiseHFC xs => SerialiseNodeToNode (HardForkBlock xs) (HardForkBlock xs) where
  encodeNodeToNode ccfg _ = wrapCBORinCBOR (encodeDiskHfcBlock ccfg)
```

`wrapCBORinCBOR enc x = Serialise.encode (tag(24) bstr(enc(x)))`

`encodeDiskHfcBlock` for Cardano is a **custom override** (NOT the generic `encodeNS`):

```haskell
-- ouroboros-consensus-cardano/.../Cardano/Node.hs
BlockConway  blockConway  -> prependTag 7 $ encodeDisk ccfgConway  blockConway
-- prependTag tag payload = array(2)[word(tag), payload]
```

Era → tag mapping (matching on-disk storage format exactly):
- Byron EBB:  tag=0
- Byron block: tag=1
- Shelley: tag=2, Allegra: tag=3, Mary: tag=4, Alonzo: tag=5, Babbage: tag=6, Conway: tag=7, Dijkstra: tag=8

## Complete MsgBlock wire layout

```
array(2) [
  word(4),                            ← MsgBlock tag
  tag(24) bstr( [era_word, body] )    ← CBOR-in-CBOR, verbatim stored CBOR
]
```

The stored block CBOR `[era_word, block_body]` is placed **verbatim** inside `tag(24)`.
NO structural transformation needed. The `era_word` is NOT converted to a 0-based NS index.

## ChainSync MsgRollForward (headers) — DIFFERENT path

Headers use `dispatchEncoder` → generic `encodeNS`:
```haskell
instance SerialiseHFC xs => SerialiseNodeToNode (HardForkBlock xs) (Header (HardForkBlock xs)) where
  encodeNodeToNode = dispatchEncoder `after` (getOneEraHeader . getHardForkHeader)
```

`encodeNS` produces `array(2)[era_index_u8(0-based), per_era_header_encoding]`.
For Shelley+ headers: `[hfc_index, tag(24)(header_cbor)]` where hfc_index is 0-based (Conway=6).

## The bug Torsten had

Torsten was emitting `array(2)[word(4), array(2)[hfc_index, tag(24)(body_cbor)]]`
The `array(2)` HFC wrapper at position 2 caused `DeserialiseFailure 2 "expected tag"` because
the Haskell decoder expected `tag(24)` at byte offset 2, but saw `0x82` (array start).

## Key source files
- `ouroboros-consensus/.../HardFork/Combinator/Serialisation/SerialiseNodeToNode.hs` — SerialiseNodeToNode instances
- `ouroboros-consensus-cardano/.../Cardano/Node.hs` — Cardano-specific encodeDiskHfcBlock override
- `ouroboros-network/.../Block.hs` — wrapCBORinCBOR / unwrapCBORinCBOR / Serialise(Serialised a)
- `ouroboros-network/protocols/.../BlockFetch/Codec.hs` — codecBlockFetch, MsgBlock = `[2, word(4), encodeBlock block]`
