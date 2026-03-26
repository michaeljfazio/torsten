---
name: txsubmission2-wire-format
description: Exact CBOR wire format for TxSubmission2 MsgReplyTxIds and MsgReplyTxs on N2N, including HFC era-tag wrapping
type: reference
---

# TxSubmission2 N2N Wire Format

## Key Sources

- Protocol codec: `ouroboros-network/protocols/lib/Ouroboros/Network/Protocol/TxSubmission2/Codec.hs` — `encodeTxSubmission2`, `decodeTxSubmission2`
- Cardano codec instantiation: `ouroboros-consensus/ouroboros-consensus-diffusion/src/.../Consensus/Network/NodeToNode.hs` lines ~417-422
- HFC NS encoding: `ouroboros-consensus/src/.../HardFork/Combinator/Serialisation/Common.hs` — `encodeNS`
- Shelley GenTx/GenTxId serialisation: `ouroboros-consensus-cardano/src/shelley/.../Shelley/Node/Serialisation.hs`
- Shelley GenTx ToCBOR: `ouroboros-consensus-cardano/src/shelley/.../Shelley/Ledger/Mempool.hs` line ~250

## Era Indices (CardanoEras)

From `ouroboros-consensus-cardano/src/.../Consensus/Cardano/Block.hs`:
```
0 = ByronBlock
1 = ShelleyBlock TPraos ShelleyEra
2 = ShelleyBlock TPraos AllegraEra
3 = ShelleyBlock TPraos MaryEra
4 = ShelleyBlock TPraos AlonzoEra
5 = ShelleyBlock Praos BabbageEra
6 = ShelleyBlock Praos ConwayEra
7 = ShelleyBlock Praos DijkstraEra
```

## How txid/tx Are Encoded

Both `encodeTxId` and `encodeTx` are wired to `enc = encodeNodeToNode ccfg version` in `NodeToNode.hs`.

For `HardForkBlock xs`, `encodeNodeToNode (GenTxId blk)` dispatches through `dispatchEncoder` → `encodeNS`.

`encodeNS` encodes as:
```
[listLen 2, word8(era_index), <era_payload>]
```
i.e., a definite-length CBOR array of 2 elements: era index (uint8) + payload.

### GenTxId (txid) Payload

`SerialiseNodeToNode` for `GenTxId (ShelleyBlock proto era)` uses `toEraCBOR @era`:
- `toEraCBOR @era t = toPlainEncoding (eraProtVerLow @era) (encCBOR t)`
- For `ShelleyTxId`, this is the underlying `TxId` (= `SafeHash EraIndependentTxBody` = `Hash Blake2b_256 EraIndependentTxBody`)
- `Hash h a` encodes as raw CBOR bytes (`encCBOR h = encCBOR (ShortByteString)` = `encodeByteArray`)
- Result: **32 raw bytes as CBOR bytes primitive** (major type 2, 32 bytes)

So for Conway, GenTxId on the wire = `[6, bstr(32_bytes)]` (array(2)[word8(6), bytes(32)])

### GenTx (tx body) Payload

`SerialiseNodeToNode` for `GenTx (ShelleyBlock proto era)` uses `toCBOR`:
- `toCBOR (ShelleyTx _txid tx) = wrapCBORinCBOR toCBOR tx`
- `wrapCBORinCBOR enc x = encode (Serialised (toLazyByteString (enc x)))`
- `Serialised` encodes as: `tag(24) + bytes(cbor_bytes)` = CBOR tag 24 wrapping the serialized tx bytes

So for Conway, GenTx on the wire = `[6, #6.24(bstr(tx_bytes))]` (array(2)[word8(6), tag(24)(bytes(cbor))])

## Complete Message Wire Formats

### MsgInit
```
[1, 6]
```

### MsgRequestTxIds (server→client)
```
[4, blocking:bool, ack:word16, req:word16]
```
(Note: tag=0 in codec, but this is listLen 4 + key 0)

### MsgReplyTxIds (client→server, tag 1)
```
[2, 1, [_ *[ [era_idx, txid_payload], size:word32 ] ]]
```
- Each entry is `array(2)`: [HFC-wrapped txid, size_bytes]
- For Conway txid: `[6, bstr(32)]`

### MsgRequestTxs (server→client, tag 2)
```
[2, 2, [_ *[era_idx, txid_payload] ]]
```

### MsgReplyTxs (client→server, tag 3)
```
[2, 3, [_ *[era_idx, #6.24(bstr(tx_cbor_bytes))] ]]
```
- Each tx is `array(2)`: [era_idx, cbor_in_cbor_tx]
- For Conway tx: `[6, tag(24)(bstr(cbor_bytes_of_tx))]`

### MsgDone (client→server, tag 4)
```
[1, 4]
```

## Size Reporting in MsgReplyTxIds

The `SizeInBytes` reported alongside each txid in `MsgReplyTxIds` is the estimated wire size of
the full tx as it would appear in `MsgReplyTxs`. This includes the HFC envelope overhead (~3 bytes:
1 for array-of-2 header, 1 for era index word8, 1 for CBOR tag 24 header).

Haskell's `txWireSize` function returns `encodeNodeToNode` byte count. Mismatches larger than
`const_MAX_TX_SIZE_DISCREPANCY` (10 bytes in V2) will terminate the connection per V2 inbound code.

Torsten must include the HFC envelope in `SizeInBytes` when announcing txids.

## N2N Protocol vs N2C Protocol

**N2N TxSubmission2**: ERA-TAGGED (HFC wrapper, double-wrapping for tx bodies)
**N2C LocalTxSubmission**: NO era tagging, just raw tx CBOR (different serialisation path)
