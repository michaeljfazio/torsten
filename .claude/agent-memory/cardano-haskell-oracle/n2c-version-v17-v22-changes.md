---
name: N2C Version Changes V17-V22
description: Detailed wire format and query tag changes for N2C protocol versions V17 through V22, including PParams encoding change at V21 and GetStakeDistribution2
type: reference
---

# N2C Protocol Versions V17–V22 Changes

## Version Mapping Table

| NtC Version  | Shelley Version      | CardanoNtC Pattern     | Golden Dir            |
|:-------------|:---------------------|:-----------------------|:----------------------|
| V16          | ShelleyNtC8          | CardanoNtCVersion12    | QueryVersion3/V16     |
| V17          | ShelleyNtC9          | CardanoNtCVersion13    | QueryVersion3/V16     |
| V18          | ShelleyNtC10         | CardanoNtCVersion14    | QueryVersion3/V16     |
| V19          | ShelleyNtC11         | CardanoNtCVersion15    | QueryVersion3/V16     |
| V20          | ShelleyNtC12         | CardanoNtCVersion16    | QueryVersion3/V17     |
| V21          | ShelleyNtC13         | CardanoNtCVersion17    | QueryVersion3/V17     |
| V22          | ShelleyNtC14         | CardanoNtCVersion18    | QueryVersion3/V17     |
| V23          | ShelleyNtC15         | CardanoNtCVersion19    | QueryVersion3/V17     |

Source files:
- `ouroboros-network/cardano-diffusion/api/lib/Cardano/Network/NodeToClient/Version.hs`
- `ouroboros-consensus/ouroboros-consensus-cardano/src/shelley/Ouroboros/Consensus/Shelley/Ledger/NetworkProtocolVersion.hs`
- `ouroboros-consensus/ouroboros-consensus-cardano/src/ouroboros-consensus-cardano/Ouroboros/Consensus/Cardano/Node.hs`

## Query Tag Changes Per Version

All tags from `ouroboros-consensus/ouroboros-consensus-cardano/src/shelley/Ouroboros/Consensus/Shelley/Ledger/Query.hs`

### V16 (ShelleyNtC8) — baseline Conway queries
New tags added (tags 22-30):
- Tag 22: `GetStakeDelegDeposits` — `Set StakingCredential -> Map StakingCredential Coin`
- Tag 23: `GetConstitution` — `Constitution era`
- Tag 24: `GetGovState` — `GovState era` (ConwayGovState)
- Tag 25: `GetDRepState` — `Set DRepCredential -> Map DRepCredential DRepState`
- Tag 26: `GetDRepStakeDistr` — `Set DRep -> Map DRep Coin`
- Tag 27: `GetCommitteeMembersState` — `Set ColdCredential -> Set HotCredential -> Set MemberStatus -> CommitteeMembersState`
- Tag 28: `GetFilteredVoteDelegatees` — `Set StakingCredential -> Map StakingCredential DRep`
- Tag 29: `GetAccountState` — `ChainAccountState` (treasury + reserves)
- Tag 30: `GetSPOStakeDistr` — `Set (KeyHash StakePool) -> Map (KeyHash StakePool) Coin`

Deprecated (still supported until V20 cutoff):
- Tag 4: `GetProposedPParamsUpdates` deprecated at V20 (< v12 check)
- Tag 5: `GetStakeDistribution` deprecated at V21 (< v13 check)
- Tag 21: `GetPoolDistr` deprecated at V21 (< v13 check)

### V17 (ShelleyNtC9) — GetProposals + GetRatifyState
New tags:
- Tag 31: `GetProposals` — `Set GovActionId -> Seq GovActionState era`
- Tag 32: `GetRatifyState` — `RatifyState era`

### V18 (ShelleyNtC10) — GetFuturePParams
New tag:
- Tag 33: `GetFuturePParams` — `Maybe (PParams era)`

### V19 (ShelleyNtC11) — GetBigLedgerPeerSnapshot
New tag:
- Tag 34 (1-element form): `GetLedgerPeerSnapshot` (BigLedgerPeers only) — `SomeLedgerPeerSnapshot`

Query wire: `[1, 34]` (array(1)[34])

### V20 (ShelleyNtC12) — QueryStakePoolDefaultVote + TxMonitor extension
New tags:
- Tag 35: `QueryStakePoolDefaultVote` — `KeyHash StakePool -> DefaultVote`

`GetProposedPParamsUpdates` (tag 4) is no longer accepted (version gate: `< v12`)

LocalTxMonitor changes: `MsgGetMeasures` and `MsgReplyGetMeasures` added (separate mini-protocol concern, not LSQ tags).

### V21 (ShelleyNtC13) — NEW PParams + CompactGenesis encoding + new queries
**PParams encoding CHANGES** (see below for details).
**CompactGenesis (ShelleyGenesis) encoding CHANGES** (see below).

New tags:
- Tag 36: `GetPoolDistr2` — `Maybe (Set (KeyHash StakePool)) -> SL.PoolDistr` (new ledger PoolDistr type)
- Tag 37: `GetStakeDistribution2` — `SL.PoolDistr` (new ledger PoolDistr type)
- Tag 38: `GetMaxMajorProtocolVersion` — `MaxMajorProtVer` (encoded as a plain integer)

`GetStakeDistribution` (tag 5) and `GetPoolDistr` (tag 21) are now rejected (version gate: `< v13`).

### V22 (ShelleyNtC14) — GetBigLedgerPeerSnapshot SRV support
Changed behavior:
- Tag 34 (2-element form): `GetLedgerPeerSnapshot peerKind` — now accepts `AllLedgerPeers | BigLedgerPeers`

Query wire: `[2, 34, peerKindTag]` where peerKindTag=0 (All) or 1 (Big)
Previously: `[1, 34]` was the only form (implicitly BigLedgerPeers).
The old `[1, 34]` form still decodes (backward compat) as `GetLedgerPeerSnapshot' False BigLedgerPeers`.

### V23 (ShelleyNtC15) — GetDRepDelegations
New tag:
- Tag 39: `GetDRepDelegations` — `Set DRep -> Map DRep (Set (Credential Staking))`

`GetLedgerPeerSnapshot' True peerKind` requires >= v15; `GetLedgerPeerSnapshot' False BigLedgerPeers` (the V19 form) requires only >= v11.

## PParams Encoding Change at V21

### The Problem
In Shelley, Allegra, Mary, Alonzo, and Babbage eras, `ProtVer` was encoded as **two separate flat integers** within the PParams array. This is the "legacy" encoding.

Starting at `ShelleyNodeToClientVersion13` (NtC V21), `ProtVer` encodes as a **single nested 2-element array**: `array(2)[major, minor]`.

### Technical Details
`ProtVer` type:
```haskell
-- libs/cardano-ledger-core/src/Cardano/Ledger/BaseTypes.hs
data ProtVer = ProtVer { pvMajor :: !Version, pvMinor :: !Natural }
  deriving (EncCBOR) via (CBORGroup ProtVer)   -- encodes as array(2)[major, minor]
  deriving (DecCBOR) via (CBORGroup ProtVer)

instance ToCBOR ProtVer where
  toCBOR ProtVer{..} = toCBOR (pvMajor, pvMinor)  -- same: array(2)[major, minor]

instance EncCBORGroup ProtVer where
  encCBORGroup (ProtVer x y) = encCBOR x <> encCBOR y
  listLen _ = 2
```

`CBORGroup` wraps in a list: `encCBOR (CBORGroup x) = encodeListLen (listLen x) <> encCBORGroup x`
So `encCBOR (ProtVer major minor) = array(2)[major, minor]`.

The new `PParams` EncCBOR instance (in `libs/cardano-ledger-core/src/Cardano/Ledger/Core/PParams.hs`):
```haskell
instance EraPParams era => EncCBOR (PParams era) where
  encCBOR pp =
    encodeListLen (fromIntegral (length (eraPParams @era)))
      <> F.foldMap' toEnc (eraPParams @era)
    where
      toEnc PParam{ppLens} = encCBOR $ pp ^. ppLens
```

Each field uses `encCBOR`, so `ProtVer` becomes `array(2)[major, minor]` — one entry in the outer array.

The **legacy** encoding (pre-V21) from `Query/LegacyPParams.hs`:
```haskell
-- Babbage era:
!> To (pvMajor bppProtocolVersion)   -- flat integer
!> To (pvMinor bppProtocolVersion)   -- flat integer
```
These are two separate flat entries. So BabbagePParams had 23 fields (two for ProtVer), legacy; 22 fields (one ProtVer as array(2)) new.

### Era-by-Era Field Count Change

| Era      | Fields (legacy, pre-V21) | Fields (new, V21+) | ProtVer encoding (new) |
|:---------|:-------------------------|:-------------------|:-----------------------|
| Shelley  | 18                       | 17                 | array(2)[major, minor] |
| Allegra  | 18                       | 17                 | array(2)[major, minor] |
| Mary     | 18                       | 17                 | array(2)[major, minor] |
| Alonzo   | 25                       | 24                 | array(2)[major, minor] |
| Babbage  | 23                       | 22                 | array(2)[major, minor] |
| Conway   | 31                       | 31                 | NO CHANGE (always array(2)) |

Conway PParams was always new (EncCBOR via the new framework) since `LegacyPParams ConwayEra` just passes through to `toCBOR . unLegacyPParams`.

### Golden Test Evidence
V16 Allegra EmptyPParams: `81 92 00 ...` = HFC[array(18)[...]] with flat ProtVer
V17 Allegra EmptyPParams: `81 91 00 ...` = HFC[array(17)[...]] with ProtVer as array(2)

### CompactGenesis Encoding Change
`GetGenesisConfig` (tag 11) response also changed at V21:
- Pre-V21: uses `LegacyShelleyGenesis` which uses legacy PParams inside `ShelleyGenesis`
- V21+: uses standard `CompactGenesis` ToCBOR which uses the new PParams encoding

`activeSlotsCoeff` in LegacyShelleyGenesis uses `unboundRational` (no tag 30); the new encoding may differ.

## GetStakeDistribution2 (Tag 37)

### Old Type: GetStakeDistribution (tag 5)
Returns the old `PoolDistr c` from `Ouroboros.Consensus.Shelley.Ledger.Query.Types`:
```haskell
-- Query/Types.hs — copy of old ledger type
data IndividualPoolStake c = IndividualPoolStake
  { individualPoolStake    :: !Rational          -- pool stake fraction
  , individualPoolStakeVrf :: !(Hash HASH (VRF.VerKeyVRF (VRF c)))  -- 32-byte vrf hash
  }
-- EncCBOR: array(2)[rational, hash_bytes]

newtype PoolDistr c = PoolDistr
  { unPoolDistr :: Map (KeyHash StakePool) (IndividualPoolStake c) }
-- EncCBOR: map of pool_hash → IndividualPoolStake
```

Wire format: `{ pool_keyhash_bytes → [stake_rational, vrf_hash_bytes], ... }`
- `pool_keyhash_bytes`: 28-byte Blake2b-224 of pool cold VKey
- `stake_rational`: tag(30)[numerator, denominator]
- `vrf_hash_bytes`: 32-byte Blake2b-256 of VRF verification key

### New Type: GetStakeDistribution2 (tag 37)
Returns `SL.PoolDistr` from `libs/cardano-ledger-core/src/Cardano/Ledger/State/PoolDistr.hs`:
```haskell
data IndividualPoolStake = IndividualPoolStake
  { individualPoolStake      :: !Rational              -- pool stake fraction
  , individualTotalPoolStake :: !(CompactForm Coin)    -- absolute lovelace (uint64)
  , individualPoolStakeVrf   :: !(VRFVerKeyHash StakePoolVRF)  -- 32-byte vrf hash
  }
-- EncCBOR: array(3)[rational, compact_coin, vrf_hash_bytes]

data PoolDistr = PoolDistr
  { unPoolDistr        :: !(Map (KeyHash StakePool) IndividualPoolStake)
  , pdTotalActiveStake :: !(NonZero Coin)   -- NonZero Coin = newtype Coin = integer
  }
-- EncCBOR (via Rec):
--   encode (Rec PoolDistr !> To distr !> To total)
--   = array(2)[distr_map, total_coin_integer]
```

Wire format for GetStakeDistribution2 response (inside HFC success wrapper):
```
array(2)[
  { pool_keyhash_bytes → array(3)[stake_rational, compact_lovelace_uint64, vrf_hash_bytes], ... },
  total_active_stake_lovelace_integer
]
```
- `pool_keyhash_bytes`: 28-byte Blake2b-224 of pool cold VKey (same as before)
- `stake_rational`: tag(30)[numerator, denominator]
- `compact_lovelace_uint64`: `CompactForm Coin` = compact unsigned integer (lovelace for this pool)
- `vrf_hash_bytes`: 32-byte Blake2b-256 (same hash, different phantom type but identical bytes)
- `total_active_stake_lovelace_integer`: `NonZero Coin` newtype = plain integer (lovelace)

### GetPoolDistr2 (Tag 36)
Same response type as GetStakeDistribution2 (`SL.PoolDistr`) but filtered:
- Query wire: `[2, 36, maybe_set_pool_ids]`
- If `Nothing`: returns all pools (same as GetStakeDistribution2)
- If `Just set`: returns only the listed pool IDs

## Key Differences Summary

| Feature | V17 | V18 | V19 | V20 | V21 | V22 | V23 |
|:--------|:----|:----|:----|:----|:----|:----|:----|
| GetProposals (31) | Added | — | — | — | — | — | — |
| GetRatifyState (32) | Added | — | — | — | — | — | — |
| GetFuturePParams (33) | — | Added | — | — | — | — | — |
| GetBigLedgerPeerSnapshot (34) | — | — | Added | — | — | Extended | — |
| QueryStakePoolDefaultVote (35) | — | — | — | Added | — | — | — |
| GetProposedPParamsUpdates (4) | — | — | — | Removed | — | — | — |
| GetPoolDistr2 (36) | — | — | — | — | Added | — | — |
| GetStakeDistribution2 (37) | — | — | — | — | Added | — | — |
| GetMaxMajorProtVersion (38) | — | — | — | — | Added | — | — |
| GetStakeDistribution (5) | — | — | — | — | Removed | — | — |
| GetPoolDistr (21) | — | — | — | — | Removed | — | — |
| PParams encoding | — | — | — | — | CHANGED | — | — |
| CompactGenesis encoding | — | — | — | — | CHANGED | — | — |
| GetDRepDelegations (39) | — | — | — | — | — | — | Added |

## Rust/Dugite Implementation Notes

### Version Negotiation
Dugite currently supports V16-V17. To add V21 support:
1. Add `NodeToClientV_21` to the negotiation set
2. Gate `GetPoolDistr2`, `GetStakeDistribution2`, `GetMaxMajorProtocolVersion` on V21+
3. Switch PParams encoding based on negotiated version
4. Gate `GetStakeDistribution` (tag 5) and `GetPoolDistr` (tag 21) off for V21+ clients

### PParams Encoding Switch
When responding to `GetCurrentPParams`:
- V16-V20: use legacy encoding (ProtVer as two flat integers)
- V21+: use new encoding (ProtVer as `array(2)[major, minor]`)

The simplest approach: have two serialization paths for Shelley/Allegra/Mary/Alonzo/Babbage PParams,
selected by the negotiated client version. Conway PParams always uses the new path.

### New PoolDistr Wire Format
For GetStakeDistribution2 (tag 37) and GetPoolDistr2 (tag 36):
```rust
// Outer: array(2)[pool_map, total_active_stake]
// Each entry in pool_map: pool_keyhash(28 bytes) -> array(3)[rational, compact_coin, vrf_hash]
// total_active_stake: plain u64 lovelace integer
```

The key difference from tag 5:
- `IndividualPoolStake` now has 3 fields (was 2) — added `individualTotalPoolStake` (CompactForm Coin = u64)
- The whole response is wrapped in `array(2)[map, total]` instead of just a plain map
