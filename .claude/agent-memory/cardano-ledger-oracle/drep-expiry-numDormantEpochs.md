---
name: DRep expiry and vsNumDormantEpochs mechanics
description: Exact Haskell implementation of computeDRepExpiry, computeDRepExpiryVersioned, updateNumDormantEpochs, and the expiry check in Ratify — including the delta-based formula and bug history
type: reference
---

## Source files
- `eras/conway/impl/src/Cardano/Ledger/Conway/Rules/GovCert.hs` — computeDRepExpiry, computeDRepExpiryVersioned, ConwayRegDRep/ConwayUpdateDRep handlers
- `eras/conway/impl/src/Cardano/Ledger/Conway/Rules/Epoch.hs` — updateNumDormantEpochs
- `eras/conway/impl/src/Cardano/Ledger/Conway/Rules/Ratify.hs` — `reCurrentEpoch > drepExpiry drepState` check

## computeDRepExpiry (post-PV10, normal path)

```haskell
computeDRepExpiry :: EpochInterval -> EpochNo -> EpochNo -> EpochNo
computeDRepExpiry ppDRepActivity currentEpoch =
  binOpEpochNo (-) (addEpochInterval currentEpoch ppDRepActivity)
-- i.e.: expiry = (currentEpoch + drepActivity) - numDormantEpochs
```

`binOpEpochNo op e1 e2 = EpochNo (op (unEpochNo e1) (unEpochNo e2))`

## computeDRepExpiryVersioned

```haskell
computeDRepExpiryVersioned pp currentEpoch numDormantEpochs
  | hardforkConwayBootstrapPhase (pp ^. ppProtocolVersionL) =
      addEpochInterval currentEpoch (pp ^. ppDRepActivityL)   -- PV < 10: ignores dormant!
  | otherwise =
      computeDRepExpiry (pp ^. ppDRepActivityL) currentEpoch numDormantEpochs
```

PV < 10 (Conway bootstrap): expiry = currentEpoch + drepActivity (no dormant correction)
PV >= 10: expiry = currentEpoch + drepActivity - numDormantEpochs (at registration time)

## Registration handler (ConwayRegDRep)

Uses `computeDRepExpiryVersioned` — takes CURRENT numDormantEpochs at registration time.

## UpdateDRep handler (ConwayUpdateDRep / vote refresh)

Uses `computeDRepExpiry` directly (not versioned) — always applies dormant correction:
```haskell
drepExpiryL .~ computeDRepExpiry ppDRepActivity cgceCurrentEpoch
                  (certState ^. certVStateL . vsNumDormantEpochsL)
```

## updateNumDormantEpochs (called in EPOCH transition)

```haskell
updateNumDormantEpochs :: EpochNo -> Proposals era -> VState era -> VState era
updateNumDormantEpochs currentEpoch ps vState =
  if null $ OMap.filter ((currentEpoch <=) . gasExpiresAfter) $ ps ^. pPropsL
    then vState & vsNumDormantEpochsL %~ succ
    else vState
```

A dormant epoch = at the epoch boundary, there are zero proposals whose `gasExpiresAfter >= currentEpoch`
(i.e., zero live proposals to vote on).
Called with `eNo` = the NEW epoch number (signal to EPOCH rule).
vsNumDormantEpochs is CUMULATIVE since Conway genesis — never reset.

## Expiry check in Ratify (drepAccepted)

```haskell
| reCurrentEpoch > drepExpiry drepState -> (yes, tot)  -- expired
```

`reCurrentEpoch` is the CURRENT epoch (the epoch being ratified in, NOT epoch+1).
A DRep is expired when `currentEpoch > drepExpiry` (strictly greater than).

## KEY INSIGHT: expiry stores an absolute epoch, NOT an offset

expiry = registrationEpoch + drepActivity - numDormantEpochsAtRegistration

At a later epoch E, the DRep is expired when:
  E > (registrationEpoch + drepActivity - numDormantEpochsAtRegistration)

The delta that matters is:
  (numDormantEpochsNow - numDormantEpochsAtRegistration)

Haskell does NOT update existing DRep expiry values when vsNumDormantEpochs is incremented.
The dormant correction is baked in at registration/vote time only.

## Rust bug pattern (overcorrection)

Wrong: elapsed = currentEpoch - registrationEpoch - totalDormantEpochs
Right: expiry = registrationEpoch + drepActivity - dormantAtRegistration
       expired = currentEpoch > expiry

Or equivalently:
  activeEpochsElapsed = (currentEpoch - registrationEpoch) - (dormantNow - dormantAtRegistration)
  expired = activeEpochsElapsed > drepActivity

The total dormant count should NOT be subtracted — only the INCREMENTAL dormant epochs
since registration.

## Concrete example (from user question)

- Registration: epoch=100, numDormantEpochs=50, drepActivity=20
- expiry = 100 + 20 - 50 = 70  (stored in drepExpiry)
- At epoch=160, numDormantEpochs=55
- Check: 160 > 70? YES => expired

But the DRep registered only 60 epochs ago, with 5 new dormant epochs since registration.
Active epochs elapsed = 60 - 5 = 55, which exceeds drepActivity=20 => also expired.

If numDormantEpochs had been 50 at epoch 160 (no new dormant), expiry=70, 160 > 70 => still expired.
The expiry is always a fixed stored value; Haskell does not retroactively adjust it.
