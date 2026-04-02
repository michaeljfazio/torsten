# Pool Distribution for VRF Leader Eligibility

## Which Snapshot?
`nesPd` (top-level field of `NewEpochState`) = `ssStakeMarkPoolDistr(esSnapshots es0)` set once at epoch boundary.

This is the **mark** snapshot from the PREVIOUS epoch (pre-rotation), which equals the **set** snapshot AFTER SNAP rotation. The key point: it's memoized as `nesPd` and never recomputed mid-epoch.

## Code Path (Incoming Block Validation)
1. `protocolLedgerView` in `SupportsProtocol.hs` extracts `nesPd` from `NewEpochState`
2. Puts it in `Praos.LedgerView.lvPoolDistr`
3. `updateChainDepState` in `Praos.hs` calls `validateVRFSignature` with `tickedPraosStateLedgerView`
4. `validateVRFSignature` extracts `PoolDistr pd` from `lvPoolDistr`
5. Looks up `IndividualPoolStake sigma` by pool KeyHash in `pd`
6. `checkLeaderNatValue vrfLeaderVal sigma f` checks eligibility

## Key Files
- ouroboros-consensus: `Protocol/Praos.hs` lines 474-600 (updateChainDepState, validateVRFSignature)
- ouroboros-consensus: `Shelley/Ledger/SupportsProtocol.hs` lines 93-105 (protocolLedgerView)
- cardano-ledger: `Conway/Rules/NewEpoch.hs` line 182 (`pd' = ssStakeMarkPoolDistr (esSnapshots es0)`)
- cardano-ledger: `Shelley/Rules/NewEpoch.hs` lines 172-198 (same, with detailed comment)
- cardano-ledger: `State/SnapShots.hs` lines 341-347 (SnapShots data type)

## Dugite Bug
Uses `snapshots.set` computed on-the-fly per batch. Should memoize `pool_distr` on LedgerState at epoch boundary.
Also uses f64 for relative stake; Haskell uses exact Rational.
