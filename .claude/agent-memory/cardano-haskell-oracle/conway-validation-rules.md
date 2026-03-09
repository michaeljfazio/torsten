# Conway Era Validation Rules Reference

## Transaction Validation (LEDGER rule order)
1. CERTS (certificates processed first, sequentially left-to-right)
2. GOV (governance votes + proposals)
3. UTXOW (UTxO with witnesses)

## UTXO Predicate Failures (Phase-1)
- BadInputsUTxO, InputSetEmptyUTxO
- OutsideValidityIntervalUTxO
- MaxTxSizeUTxO
- FeeTooSmallUTxO (includes ref script tiered fees)
- ValueNotConservedUTxO
- WrongNetwork, WrongNetworkWithdrawal, WrongNetworkInTxBody
- OutputTooSmallUTxO, OutputBootAddrAttrsTooBig, OutputTooBigUTxO
- BabbageOutputTooSmallUTxO, BabbageNonDisjointRefInputs
- InsufficientCollateral, NoCollateralInputs, TooManyCollateralInputs
- CollateralContainsNonADA, IncorrectTotalCollateralField
- ExUnitsTooBigUTxO
- OutsideForecast (slot outside consensus range)

## UTXOW Predicate Failures (Witness)
- InvalidWitnessesUTXOW, MissingVKeyWitnessesUTXOW
- MissingScriptWitnessesUTXOW, ExtraneousScriptWitnessesUTXOW
- ScriptWitnessNotValidatingUTXOW
- MalformedScriptWitnesses, MalformedReferenceScripts
- MissingRedeemers, ExtraRedeemers
- PPViewHashesDontMatch (script integrity hash)
- MissingRequiredDatums, NotAllowedSupplementalDatums
- UnspendableUTxONoDatumHash
- MissingTxBodyMetadataHash, MissingTxMetadata, ConflictingMetadataHash, InvalidMetadata

## UTXOS (Phase-2 Script Validation)
- ValidationTagMismatch (isValid flag incorrect)
- CollectErrors (script inputs not locatable)

## BBODY (Block Body)
- WrongBlockBodySizeBBODY, InvalidBodyHashBBODY
- TooManyExUnits (block-level ExUnit budget)
- BodyRefScriptsSizeTooBig (per-block ref script limit)
- HeaderProtVerTooHigh

## CERTS
- WithdrawalsNotInRewardsCERTS (must withdraw full amount)
- Certificates processed sequentially left-to-right

## Delegation (DELEG)
- IncorrectDepositDELEG, StakeKeyRegisteredDELEG, StakeKeyNotRegisteredDELEG
- StakeKeyHasNonZeroAccountBalanceDELEG
- DelegateeDRepNotRegisteredDELEG, DelegateeStakePoolNotRegisteredDELEG

## GovCert
- ConwayDRepAlreadyRegistered, ConwayDRepNotRegistered
- ConwayDRepIncorrectDeposit, ConwayDRepIncorrectRefund
- ConwayCommitteeHasPreviouslyResigned, ConwayCommitteeIsUnknown

## GOV (Proposals/Votes)
- 19 predicate failures including:
  - DisallowedVoters, VotersDoNotExist, VotingOnExpiredGovAction
  - ProposalDepositIncorrect, ProposalReturnAccountDoesNotExist
  - InvalidPrevGovActionId, ProposalCantFollow (hardfork version)
  - InvalidGuardrailsScriptHash
  - DisallowedProposalDuringBootstrap, DisallowedVotesDuringBootstrap
  - ZeroTreasuryWithdrawals
  - ConflictingCommitteeUpdate, ExpirationEpochTooSmall

## Protocol Parameters (CBOR keys)
Keys 0-21 inherited from Babbage. Conway adds:
- 25: poolVotingThresholds (5 fields)
- 26: dRepVotingThresholds (10 fields)
- 27: committeeMinSize
- 28: committeeMaxTermLength
- 29: govActionLifetime
- 30: govActionDeposit
- 31: dRepDeposit
- 32: dRepActivity
- 33: minFeeRefScriptCostPerByte

## Reward Formula (maxPool)
```
z0 = 1 / nOpt
sigma' = min(sigma, z0)
p' = min(pledge_ratio, z0)
maxPool = floor(R / (1 + a0) * (sigma' + p' * a0 * ((sigma' - p' * ((z0 - sigma') / z0)) / z0)))
```

## Epoch Transition Order (EPOCH rule)
1. Snapshot rotation (SNAP)
2. Pool reaping (POOLREAP)
3. Governance ratification & enactment (extract DRep pulser)
4. Apply enacted withdrawals
5. Protocol param updates take effect (future→current→prev)
6. Committee state cleanup
7. Dormant epoch counter
8. Treasury donations + unclaimed deposits
9. Obligation recalculation
10. Hard fork check
11. DRep pulser refresh

## Epoch Transition Order (NEWEPOCH rule)
1. Complete pending reward pulsing
2. Apply reward updates to epoch state
3. Execute EPOCH rule
4. Calculate ADA pots snapshot
5. Update epoch label, reset block counts, clear reward state
