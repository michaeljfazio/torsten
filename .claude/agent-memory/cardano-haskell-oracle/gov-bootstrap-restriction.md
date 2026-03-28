---
name: Conway Bootstrap Governance Restriction
description: isBootstrapAction allows ParameterChange/HardForkInitiation/InfoAction during proto==9, NOT the opposite
type: reference
---

## isBootstrapAction — Haskell Source (Gov.hs line 642)

```haskell
isBootstrapAction :: GovAction era -> Bool
isBootstrapAction =
  \case
    ParameterChange {}    -> True   -- ALLOWED
    HardForkInitiation {} -> True   -- ALLOWED
    InfoAction            -> True   -- ALLOWED
    _                     -> False  -- NoConfidence, UpdateCommittee, NewConstitution, TreasuryWithdrawals all REJECTED
```

## Key Facts

- Introduced in commit b6282d5 ("Restrict allowed proposal types during bootstrap phase", Apr 2024)
- `HardForkInitiation` was in the ALLOWED set from the very first commit — it was never blocked
- The Plomin hard fork (proto 9→10) was a `HardForkInitiation` submitted during bootstrap — correctly accepted
- `hardforkConwayBootstrapPhase pv = pvMajor pv == natVersion @9` (Era.hs line 176)
- Located: `eras/conway/impl/src/Cardano/Ledger/Conway/Rules/Gov.hs`
- `hardforkConwayBootstrapPhase` defined in: `eras/conway/impl/src/Cardano/Ledger/Conway/Era.hs`

## Common Mistake

An earlier Torsten implementation had the sets inverted: it allowed `NoConfidence/UpdateCommittee/NewConstitution`
and rejected `ParameterChange/HardForkInitiation/TreasuryWithdrawals` — exactly backwards.

The correct check pattern:
```rust
let allowed = matches!(
    &proposal.gov_action,
    GovAction::ParameterChange { .. }
        | GovAction::HardForkInitiation { .. }
        | GovAction::InfoAction
);
if !allowed { /* reject */ }
```

## Voting Restrictions During Bootstrap

During bootstrap (proto 9), DRep votes are only allowed on `isBootstrapAction` proposals.
For all other voter types, all proposals can be voted on.
DRep thresholds are implicitly 0 during bootstrap (auto-pass for DRep dimension).
See `checkDisallowedVotes` in Gov.hs around line 393.
