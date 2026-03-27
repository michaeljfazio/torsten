---
name: SDU Segmentation Size — Haskell vs Torsten
description: Critical: SDUSize value IS the split point; no -8 adjustment; Haskell uses splitAt(sduSize), NOT splitAt(sduSize-8)
type: reference
---

## CRITICAL FINDING: SDU Split Point in ouroboros-network

**Location**: `/tmp/ouroboros-network/network-mux/src/Network/Mux/Egress.hs:204`

```haskell
processSingleWanton :: MonadSTM m
                    => EgressQueue m
                    -> SDUSize
                    -> MiniProtocolNum
                    -> MiniProtocolDir
                    -> Wanton m
                    -> m SDU
processSingleWanton egressQueue (SDUSize sduSize)
                    mpc md wanton = do
    blob <- atomically $ do
      -- extract next SDU
      d <- readTVar (want wanton)
      let (frag, rest) = BL.splitAt (fromIntegral sduSize) d
      -- ... (re-enqueue if more remains)
```

**KEY**: Line 204 splits **EXACTLY at `sduSize`** bytes of **PAYLOAD ONLY**.

## SDUSize Value

**Location**: `/tmp/ouroboros-network/network-mux/src/Network/Mux/Bearer.hs:86`

```haskell
makeSocketBearer' egressInterval = MakeBearer $ pureBearer $ \sduTimeout fd rb ->
    socketAsBearer size batch rb sduTimeout egressInterval fd
  where
    size = SDUSize 12_288   -- THIS IS THE SDU SIZE
    batch = 131_072
```

**The SDUSize value for sockets is exactly 12_288 bytes.**

## Wire Format Structure

**Location**: `/tmp/ouroboros-network/network-mux/src/Network/Mux/Types.hs`

```haskell
msHeaderLength :: Int64
msHeaderLength = 8  -- 8-byte header (4 bytes timestamp + 2 bytes proto+dir + 2 bytes length)
```

**Location**: `/tmp/ouroboros-network/network-mux/src/Network/Mux/Codec.hs:45`

```haskell
encodeSDU sdu =
  let hdr = Bin.runPut enc in
  BL.append hdr $ msBlob sdu
  where
    enc = do
        Bin.putWord32be $ unRemoteClockModel $ msTimestamp sdu
        Bin.putWord16be $ putNumAndMode (msNum sdu) (msDir sdu)
        Bin.putWord16be $ fromIntegral $ BL.length $ msBlob sdu
```

The encoded SDU on the wire is: **8-byte header + payload**

Where payload_length field = `BL.length $ msBlob sdu` = the EXACT length of the blob, NOT including the header.

## What the Haskell Mux Does

1. **Egress**: Reads from a Wanton's data stream.
2. **Splits** at EXACTLY `sduSize` (12_288) bytes to extract one chunk of **payload**.
3. **Creates SDU** with header: timestamp(4) + proto_id+dir(2) + length(2) + payload.
4. **Wire format**: 8 bytes header + N bytes payload (where N ≤ 12_288).

## What the Ingress Path Does

**Location**: `/tmp/ouroboros-network/network-mux/src/Network/Mux/Bearer/Socket.hs:87-91`

```haskell
case Mx.decodeSDU hbuf of
     Left  e ->  throwIO e
     Right header@Mx.SDU { Mx.msHeader } -> do
         traceWith tracer $ Mx.TraceRecvHeaderEnd msHeader
         !blob <- recvLen' (fromIntegral $ Mx.mhLength msHeader) []
```

The receiver:
1. Decodes the 8-byte header to extract the `mhLength` field.
2. Reads **exactly** `fromIntegral $ Mx.mhLength msHeader` bytes of payload.
3. **NO VALIDATION** against `sduSize`.

The Haskell mux accepts ANY `mhLength` value in the received header, up to 65535 (u16 max).

## Critical Implication for Torsten

**Torsten's current bug**: The mux is reading the `payload_length` field from the header but may not be splitting correctly.

If Haskell sends payload_length = 12_288, Torsten MUST:
1. Accept it (no validation against SDUSize).
2. Read exactly 12_288 bytes of payload.
3. Strip the 8-byte header and deliver just the payload to the protocol handler.

**Torsten changed from 12_288 to 12_280 payload but error changed** from "expected word" to "expected list len or indef". This suggests:
- The size change was NOT the issue.
- The real problem is in how the split/chunking is being done or how protocol messages are being reconstructed.

## Summary

- **SDUSize = 12_288** = payload size per SDU (Haskell splitAt point).
- **Wire size = 8 (header) + payload** = total bytes on wire.
- **No -8 adjustment** in Haskell; the 8-byte header is NOT part of the payload chunk.
- **Ingress accepts any payload_length** from 0 to 65535; no upper bound check.
