# LocalStateQuery MsgResult Wire Format

## Protocol Layer
MsgResult = `[4, result_encoding]` (Codec.hs line 110-113)

## Result Encoding Chain (for BlockQuery / QueryIfCurrent)

### HardFork Success Wrapper (encodeEitherMismatch)
- **HardForkNodeToClientEnabled** (all current versions V16-V23):
  - Success: `[result]` (1-element CBOR array, no tag)
  - Era mismatch: `[[era_idx, era_name], [era_idx, era_name]]` (2-element array)
- **HardForkNodeToClientDisabled** (obsolete): raw result, no wrapper

### encodeQueryIfCurrentResult
No additional wrapping - pure delegation to era-specific encoder.

### Era-Specific (encodeShelleyResult)
Raw CBOR value per query type (toCBOR, toEraCBOR, etc.)

## Complete Example: GetEpochNo returning epoch 100
```
82 04 81 18 64
[4, [100]]
```

## QueryAnytime/QueryHardFork Results
NO success/failure wrapper - raw result directly:
```
[4, <raw_result>]
```

## Key Files
- Protocol codec: `ouroboros-network/protocols/lib/.../LocalStateQuery/Codec.hs`
- HFC result wrapping: `ouroboros-consensus/.../HardFork/Combinator/Serialisation/SerialiseNodeToClient.hs` (lines 494-568)
- Shelley result encoding: `ouroboros-consensus-cardano/src/shelley/.../Shelley/Ledger/Query.hs` (line 1020+)
- Query dispatch: `ouroboros-consensus/.../Ledger/Query.hs` (line 519+)

## Cardano Era Indices (for NS encoding)
0=Byron, 1=Shelley, 2=Allegra, 3=Mary, 4=Alonzo, 5=Babbage, 6=Conway, 7=Dijkstra

## GetStakeDistribution (tag 5, DEPRECATED < ShelleyV13)
- Result: consensus `PoolDistr` = newtype `Map(KeyHash StakePool, IndividualPoolStake)`
- Wire: bare CBOR map (no wrapper)
- Key: bytes(28) pool hash
- Value: array(2) [tag(30)[num,den], bytes(32) vrf_hash]
- IndividualPoolStake has only 2 fields (no CompactForm Coin)
- Conversion via `fromLedgerPoolDistr` drops totalPoolStake and totalActiveStake

## GetStakeDistribution2 (tag 37, >= ShelleyV13)
- Result: ledger `PoolDistr` = Rec with 2 fields
- Wire: array(2) [map, coin]
  - map key: bytes(28) pool hash
  - map value: array(3) [tag(30)[num,den], uint(compact_coin), bytes(32) vrf_hash]
  - coin: uint (pdTotalActiveStake, NonZero Coin = plain uint)

## GetDRepState (tag 25, >= ShelleyV8)
- Request: [2, 25, set_of_credentials]
- Result: Map(Credential DRepRole, DRepState)
- Wire: CBOR map
  - Key: array(2) [uint8(0=KeyHash|1=Script), bytes(28)]
  - Value: array(4) [uint(epoch), strict_maybe(anchor), uint(deposit), set(credentials)]
    - StrictMaybe: SNothing=array(0), SJust=array(1)[value]
    - Anchor: array(2) [text(url), bytes(32)(hash)]
    - Set(Credential Staking): each = array(2)[uint8, bytes(28)]

## Source files
- Consensus PoolDistr (old): Query/Types.hs lines 36-77
- Ledger PoolDistr (new): libs/cardano-ledger-core/src/.../State/PoolDistr.hs
- DRepState: libs/cardano-ledger-core/src/.../DRep.hs
- Credential CBOR: libs/cardano-ledger-core/src/.../Credential.hs lines 291-305
- StrictMaybe encoding: Encoder.hs lines 324-327 (array(0)/array(1), NOT null)
