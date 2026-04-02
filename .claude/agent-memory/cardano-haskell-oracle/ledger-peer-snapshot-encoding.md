---
name: LedgerPeerSnapshot CBOR Encoding
description: GetLedgerPeerSnapshot (tag 34) wire format — V2 (big peers) and V23 (big/all peers) encoding, relay access point CBOR, rational encoding details
type: reference
---

## Source Files
- Type + CBOR: `ouroboros-network/api/lib/Ouroboros/Network/PeerSelection/LedgerPeers/Type.hs`
- Relay CBOR: `ouroboros-network/api/lib/Ouroboros/Network/PeerSelection/RelayAccessPoint.hs`
- Query wiring: `ouroboros-consensus-cardano/src/shelley/Ouroboros/Consensus/Shelley/Ledger/Query.hs`
- Version mapping: `ouroboros-consensus-cardano/src/shelley/Ouroboros/Consensus/Shelley/Ledger/NetworkProtocolVersion.hs`

## Query Encoding (tag 34)
- Pre-V23 (NtC V19-V22 / Shelley V11-V14): `array(1)[34]` — BigLedgerPeers only
- V23+ (NtC V23 / Shelley V15): `array(2)[34, peer_kind]` where peer_kind: 0=AllLedgerPeers, 1=BigLedgerPeers

## Result Encoding — 3 versions, determined by internal version byte

### Common wrapper: `array(2)[version_u8, payload]`

### Version 1 (V2 type, BigLedgerPeers only, NtC V19-V22, SRV domains filtered if < V22):
```
array(2)[
  1,                    # version byte
  array(2)[
    WithOrigin,         # slot: [1] for Origin, [2, slot_u64] for At
    toCBOR pools        # ToCBOR [(AccPoolStake, (PoolStake, NonEmpty relay))]
                        # = indefinite array of array(2)[rational, array(2)[rational, indef_array_of_relay]]
  ]
]
```
Wait — the pools use `toCBOR pools` where pools is `[(AccPoolStake, (PoolStake, NonEmpty LedgerRelayAccessPoint))]`.
- List = indefinite array
- Each element = tuple(2) = `array(2)[accPoolStake, innerTuple]`
- accPoolStake = Rational = `array(2)[numerator_int, denominator_int]`
- innerTuple = tuple(2) = `array(2)[poolStake, relays]`
- poolStake = Rational = `array(2)[numerator_int, denominator_int]`
- relays = NonEmpty (encoded as list) = indefinite array of relay

### Version 2 (BigPeerSnapshotV23, NtC V23):
```
array(2)[
  2,                    # version byte
  array(3)[
    Point,              # [1, 0] for genesis; [3, 1, slot_u64, hash_sbs] for block
    network_magic_u32,
    big_stake_pools     # indefinite array of array(3)[accPoolStake_rational, poolStake_rational, relays_indef_array]
  ]
]
```

### Version 3 (AllPeerSnapshotV23, NtC V23):
```
array(2)[
  3,                    # version byte
  array(3)[
    Point,
    network_magic_u32,
    all_stake_pools     # indefinite array of array(2)[poolStake_rational, relays_indef_array]
  ]
]
```

## Relay Access Point CBOR (LedgerRelayAccessPoint)
- DNS domain: `array(3)[0, port_integer, domain_bytes]`
- IPv4: `array(3)[1, port_integer, ipv4_as_list_of_4_ints]`
- IPv6: `array(3)[2, port_integer, ipv6_as_tuple_of_4_ints]`
- SRV domain: `array(2)[3, domain_bytes]`

Note: port is encoded as `toCBOR . toInteger` = Integer (not u16!)
IPv4: `toCBOR (IP.fromIPv4 ipv4)` = `[Int]` (4 octets as Ints)
IPv6: `toCBOR (IP.fromIPv6 ip6)` = `(Int, Int, Int, Int)` = array(4) of 4 Ints (32-bit words)

## Rational Encoding
`array(2)[numerator_integer, denominator_integer]` — NO tag 30

## Dugite Current Implementation Issues
- Uses `u64` for stakes instead of `Rational` (numerator/denominator)
- Uses definite-length array for pools instead of indefinite
- Missing WithOrigin wrapper for V1, Point for V2/V3
- Missing network_magic for V2/V3
- Doesn't compute accumulated stake as Rational
- Doesn't recompute relative stake (total normalization)
- Doesn't filter big ledger peers (top 90% by stake)
