---
name: N2C Query Encoding Bugs (Tags 30, 33, 35)
description: Three N2C LocalStateQuery response encoding bugs found via Haskell source analysis - GetSPOStakeDistr, GetFuturePParams, QueryStakePoolDefaultVote
type: project
---

## Tag 30: GetSPOStakeDistr — Wrong result type
- Haskell result: `Map (KeyHash StakePool) Coin` = simple map { pool_hash(28) => integer }
- Torsten sends: `IndividualPoolStake` format with tag(30) rationals and VRF hashes
- Fix: New QueryResult variant with simple map encoding

## Tag 33: GetFuturePParams — Wrong Maybe encoding
- Haskell `Maybe` encoding (via `encodeMaybe`): Nothing=`array(0)`, Just=`array(1)[value]`
- Torsten sends: `array(1)[0]` for Nothing — this is the Sum encoding, NOT Maybe encoding
- Fix: Change to `enc.array(0)` with no contents

## Tag 35: QueryStakePoolDefaultVote — Two bugs
1. **Argument**: Haskell sends single `KeyHash` (28 bytes), Torsten expects `tag(258) Set`
2. **Response**: Haskell returns bare `word8` (0/1/2), Torsten returns `Map<PoolId, [vote]>`
3. **Semantics**: DefaultNo=0, DefaultAbstain=1, DefaultNoConfidence=2
   - AlwaysAbstain → 1 (correct)
   - AlwaysNoConfidence → 2 (Torsten had 0, wrong)
   - Specific DRep/undelegated → 0 (Torsten had 2, wrong)

## Key Haskell source locations
- Query GADT: `ouroboros-consensus-cardano/src/shelley/.../Query.hs`
- DefaultVote type: `cardano-ledger/eras/conway/impl/src/.../Conway/Governance.hs`
- FuturePParams type: `cardano-ledger/libs/cardano-ledger-core/src/.../State/Governance.hs`
- encodeMaybe: `cardano-base/cardano-binary/src/Cardano/Binary/ToCBOR.hs`
- Sum encoding (cardano-ledger-binary): `Encoding/Coders.hs` — Sum X tag = array(n+1)[tag, ...]
