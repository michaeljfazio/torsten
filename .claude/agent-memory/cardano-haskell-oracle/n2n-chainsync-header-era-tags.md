---
name: N2N ChainSync Header Era Tag Encoding
description: Two distinct era numbering schemes — storage tags vs HFC NS indices — must not be confused in MsgRollForward
type: reference
---

## The Two Era Numbering Schemes

### Pallas/ImmutableDB Storage Era Tags
Used in block CBOR on disk and in BlockFetch wire format (`[era_tag, block_body]`):

| Era     | Storage tag |
|---------|-------------|
| Byron   | 0 or 1      |
| Shelley | 2           |
| Allegra | 3           |
| Mary    | 4           |
| Alonzo  | 5           |
| Babbage | 6           |
| Conway  | 7           |

Source: `pallas-traverse/src/era.rs` — `impl From<Era> for u16`.

### HFC NS Indices (N2N ChainSync MsgRollForward Header Wire Format)
Used in `[hfc_index, tag(24)(header_bytes)]` in ChainSync MsgRollForward:

| Era     | HFC NS index |
|---------|--------------|
| Byron   | 0            |
| Shelley | 1            |
| Allegra | 2            |
| Mary    | 3            |
| Alonzo  | 4            |
| Babbage | 5            |
| Conway  | 6            |

Source: 0-based position in `ProtocolCardano` type list in `ouroboros-consensus-cardano`. Encoded by `encodeNS` as `array(2)[era_index_u8, inner_encoding]`.

## The Bug Pattern

Passing the storage era tag directly as the HFC NS index:
- Conway storage tag=7, HFC NS index=6
- Sending `[7, ...]` causes Haskell's `decodeNS` to fail with "invalid index 7" (only 7 eras, indices 0-6 valid)
- Or if storage tag happens to match a valid-but-wrong HFC index, routes to wrong era decoder

## The "Expected 15, but found 10" Error

This error from `decodeRecordNamed "RecD"` means the wrong era decoder was selected:

- **TPraos BHBody** (Shelley-Alonzo, HFC indices 1-4): flat 9 + 4 (OCert) + 2 (ProtVer) = **15 fields** in array
- **Praos HeaderBody** (Babbage-Conway, HFC indices 5-6): **10-element array** with OCert/ProtVer as nested sub-arrays

The mismatch occurs when a Praos (10-field) header is decoded by the TPraos decoder (expects 15).

## Conway Header Body Structure (Praos, 10 fields)

```
array(2) [          -- HeaderRaw: [body, sig]
  array(10) [       -- HeaderBody
    block_number,   -- uint (BlockNo)
    slot,           -- uint (SlotNo)
    prev_hash,      -- bytes(32) | null
    issuer_vkey,    -- bytes(32)
    vrf_vkey,       -- bytes(32)
    vrf_result,     -- array(2)[bytes, bytes(80)]  (CertifiedVRF)
    body_size,      -- uint (Word32)
    body_hash,      -- bytes(32)
    array(4)[hot_vkey, seq_num, kes_period, sigma],  -- OCert (nested sub-array)
    array(2)[major, minor],                           -- ProtVer (nested sub-array)
  ],
  bytes(448)        -- KES signature
]
```

## TPraos Header Body Structure (Shelley-Alonzo, 15 flat fields)

```
array(15) [         -- BHBody (flat)
  block_number,     -- 1
  slot,             -- 2
  prev_hash,        -- 3
  issuer_vkey,      -- 4
  vrf_vkey,         -- 5
  bheaderEta,       -- 6 (CertifiedVRF for nonce)
  bheaderL,         -- 7 (CertifiedVRF for leader election)
  body_size,        -- 8
  body_hash,        -- 9
  hot_vkey,         -- 10 (OCert field 1, flat)
  seq_num,          -- 11 (OCert field 2, flat)
  kes_period,       -- 12 (OCert field 3, flat)
  sigma,            -- 13 (OCert field 4, flat)
  prot_major,       -- 14 (ProtVer field 1, flat)
  prot_minor,       -- 15 (ProtVer field 2, flat)
]
```

Key differences from Praos:
1. Two VRF outputs (nonce + leader) instead of one
2. OCert/ProtVer fields FLAT in parent array instead of nested sub-arrays
3. Total 15 fields vs 10 fields

## Fix Location

`crates/torsten-network/src/protocol/chainsync/server.rs`:
- `storage_era_tag_to_hfc_index()` — converts storage tag to HFC NS index
- `extract_header_for_chainsync()` — calls the conversion before encoding
