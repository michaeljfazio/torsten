# NewEpochState CBOR Encoding (Conway Era)

## Source Files
- NewEpochState/EpochState/LedgerState/UTxOState: `shelley/impl/src/.../Shelley/LedgerState/Types.hs`
- ConwayCertState: `conway/impl/src/.../Conway/State/CertState.hs`
- VState: `conway/impl/src/.../Conway/State/VState.hs`
- ConwayGovState: `conway/impl/src/.../Conway/Governance.hs`
- DRepPulsingState/PulsingSnapshot: `conway/impl/src/.../Conway/Governance/DRepPulser.hs`
- EnactState/RatifyState: `conway/impl/src/.../Conway/Governance/Internal.hs`
- Proposals: `conway/impl/src/.../Conway/Governance/Proposals.hs`
- SnapShots/SnapShot: `libs/cardano-ledger-core/src/.../State/SnapShots.hs`
- DState/PState/CommitteeState: `libs/cardano-ledger-core/src/.../State/CertState.hs`
- PoolDistr/IndividualPoolStake: `libs/cardano-ledger-core/src/.../State/PoolDistr.hs`
- ChainAccountState: `libs/cardano-ledger-core/src/.../State/ChainAccount.hs`
- ConwayAccountState/ConwayAccounts: `conway/impl/src/.../Conway/State/Account.hs`
- ConwayInstantStake: `conway/impl/src/.../Conway/State/Stake.hs`
- PulsingRewUpdate: `shelley/impl/src/.../Shelley/RewardUpdate.hs`
- NonMyopic: `shelley/impl/src/.../Shelley/PoolRank.hs`
- FuturePParams: `libs/cardano-ledger-core/src/.../State/Governance.hs`
- DRepState: `libs/cardano-ledger-core/src/.../DRep.hs`
- StakePoolState: `libs/cardano-ledger-core/src/.../State/StakePool.hs`

## HFC Telescope Wrapping (ouroboros-consensus)
- `SerialiseDisk.hs` + `Common.hs` in `ouroboros-consensus/HardFork/Combinator/Serialisation/`
- Telescope: `encodeListLen(1 + era_index)` + past_eras... + current_era
- Current era: `encodeListLen 2` + Bound(start) + state
- Past era: `encodeListLen 2` + Bound(start) + Bound(end)
- ExtLedgerState: `encodeListLen 2` + LedgerState + HeaderState
- For Conway (era index 6): telescope is `[7, past0, past1, past2, past3, past4, past5, current6]`

## Key Encoding Order (IMPORTANT: encode vs field order differs!)

### LedgerState: array(2)
0. CertState (encoded FIRST for sharing, despite being second field)
1. UTxOState

### ConwayCertState: array(3)
0. VState (NOT DState/PState! Conway encodes VState first)
1. PState
2. DState

### UTxOState: array(6)
0. UTxO (Map using encodeMemPack for keys and values)
1. Deposited (Coin)
2. Fees (Coin)
3. GovState (ConwayGovState)
4. InstantStake (ConwayInstantStake = Map<Credential,CompactCoin>)
5. Donation (Coin)
