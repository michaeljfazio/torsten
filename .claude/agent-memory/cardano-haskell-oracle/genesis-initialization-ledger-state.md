---
name: Genesis Initialization and Ledger State
description: How cardano-node initializes ledger state from genesis configs; Byron→Shelley translation, genDelegs, staking section, resetStakeDistribution, initial pool distribution
type: reference
---

## Key Source Files
- Translation: `cardano-ledger/eras/shelley/impl/src/Cardano/Ledger/Shelley/API/ByronTranslation.hs`
  - `translateToShelleyLedgerState` and `translateToShelleyLedgerStateFromUtxo`
- Initial state construction: `cardano-ledger/eras/shelley/impl/src/Cardano/Ledger/Shelley/Transition.hs`
  - `createInitialState`, `resetStakeDistribution`, `registerInitialFunds`, `registerInitialStakePools`, `shelleyRegisterInitialAccounts`
- ShelleyGenesis type: `cardano-ledger/eras/shelley/impl/src/Cardano/Ledger/Shelley/Genesis.hs`
- Translation context: `cardano-ledger/eras/shelley/impl/src/Cardano/Ledger/Shelley/Translation.hs`
  - `FromByronTranslationContext`, `toFromByronTranslationContext`
- HFC injection: `ouroboros-consensus/.../HardFork/Combinator/Embed/Nary.hs`
  - `injectInitialExtLedgerState` — starts Byron, then `State.extendToSlot (SlotNo 0)` to trigger era transitions
- Cardano node init: `ouroboros-consensus-cardano/.../Cardano/Node.hs`
  - `protocolInfoCardano` — assembles configs, triggers, and initial state
  - `initExtLedgerStateCardano` — applies `L.injectIntoTestState` to Shelley-based eras

## Critical Facts

### translateToShelleyLedgerStateFromUtxo (Byron→Shelley)
- Takes `FromByronTranslationContext` (genDelegs, PParams, maxLovelaceSupply)
- Creates NewEpochState with:
  - `nesEL = epochNo` (the epoch of the transition)
  - `nesBprev = BlocksMade Map.empty`, `nesBcur = BlocksMade Map.empty`
  - `esSnapshots = emptySnapShots` — ALL snapshots (mark/set/go) are EMPTY
  - `nesPd = def` — PoolDistr is EMPTY
  - `utxoShelley = translateUTxOByronToShelley utxoByron` — includes nonAvvmBalances
  - `utxosInstantStake = mempty` — EMPTY (Byron UTxOs have no staking credentials)
  - `dsAccounts = def` — EMPTY accounts
  - `dsGenDelegs = GenDelegs $ fbtcGenDelegs transCtxt` — genesis delegations ARE stored
  - `reserves = maxLovelaceSupply - sum(utxoShelley)` — remainder goes to reserves
  - `stashedAVVMAddresses` — AVVM UTxOs stashed for deletion at Shelley→Allegra boundary

### genDelegs: NOT pool registrations
- `dsGenDelegs` maps GenesisRole key hashes to (delegate, vrf) pairs
- These are for OBFT slot leadership during d>0 epochs, NOT stake pool registrations
- They do NOT create entries in `psStakePools`
- They do NOT create delegations
- They do NOT affect stake distribution or reward calculations

### staking section (ShelleyGenesisStaking)
- Contains `sgsPools :: ListMap (KeyHash StakePool) StakePoolParams` — pool registrations
- Contains `sgsStake :: ListMap (KeyHash Staking) (KeyHash StakePool)` — delegations
- On preview: NOT PRESENT (defaults to `emptyGenesisStaking`)
- On mainnet: PROTECTED by `protectMainnet` (cannot be non-empty for Mainnet network ID)
- Only used via `injectIntoTestState` = `shelleyRegisterInitialFundsThenStaking`

### resetStakeDistribution
- Called AFTER registerInitialFunds AND registerInitialStakePools AND registerInitialAccounts
- Creates `initSnapShot` from current UTxO instant stake + DState accounts + PState pool params
- Sets `ssStakeMark = initSnapShot`, `ssStakeMarkPoolDistr = calculatePoolDistr initSnapShot`, `nesPd = poolDistr`
- On preview: since staking section is empty, resetStakeDistribution produces EMPTY snapshots

### Preview testnet initialization sequence
1. Node starts with Byron genesis (nonAvvmBalances: 1 address with 30T, 7 with 0)
2. `injectInitialExtLedgerState` creates Byron state, then `extendToSlot (SlotNo 0)` triggers era transitions
3. Hard fork triggers use `CardanoTriggerHardForkAtDefaultVersion` (no TestXxxHardForkAtEpoch in config)
4. Byron→Shelley transition: `translateToShelleyLedgerState` creates state with EMPTY snapshots, EMPTY pool distribution, only genDelegs
5. `injectIntoTestState` is called but has NO effect because staking/initialFunds are empty on preview
6. First blocks (in Shelley era with d=1) are produced by OBFT genesis delegates (NOT stake pools)
7. Pool registrations and stake delegations happen via on-chain transactions in those first blocks
8. Pools only enter the stake distribution after epoch boundary snapshots capture them
