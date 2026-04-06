# Cardano Ledger Oracle Memory

## CBOR Structure Reference
- [NewEpochState/EpochState/LedgerState/UTxOState complete encoding](newepochstate-complete-encoding.md) — verified field order, exact array sizes, key warnings
- [Conway PParams array(31) field order](conway-pparams-field-order.md) — all 31 fields indexed 0-30, driven by eraPParams list
- [Conway CertState/DState/PState/VState encoding](conway-certstate-encoding.md) — array sizes, field order, StakePoolState vs StakePoolParams
- [SnapShots new vs old format](snapshots-encoding.md) — array(2) new format, array(3) old, StakePoolSnapShot array(10)
- [ConwayGovState encoding](conway-gov-state-encoding-detailed.md) — array(7), nested types
- [Conway Accounts/ConwayAccountState encoding](conway-accounts-encoding.md) — per-account array(4) with nullable delegations
