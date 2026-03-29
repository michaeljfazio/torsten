---
name: DRep Pulser & Ratification Mechanism
description: Complete DRep pulser lifecycle: snapshot timing, pulse spreading, ratification flow, EPOCH/NEWEPOCH interaction
type: reference
---

## Key Files

- `eras/conway/impl/src/Cardano/Ledger/Conway/Governance/DRepPulser.hs` — `DRepPulser`, `PulsingSnapshot`, `DRepPulsingState`, `finishDRepPulser`, `computeDRepDistr`
- `eras/conway/impl/src/Cardano/Ledger/Conway/Governance.hs` — `setFreshDRepPulsingState` (lines 458–530), `predictFuturePParams`
- `eras/conway/impl/src/Cardano/Ledger/Conway/Rules/Epoch.hs` — `epochTransition` (the EPOCH rule body)
- `eras/conway/impl/src/Cardano/Ledger/Conway/Rules/NewEpoch.hs` — `newEpochTransition` (pulses on every block when not epoch boundary)
- `eras/conway/impl/src/Cardano/Ledger/Conway/Rules/Ratify.hs` — `ratifyTransition`, `dRepAcceptedRatio`, `spoAcceptedRatio`, `committeeAccepted`
- `eras/conway/impl/src/Cardano/Ledger/Conway/Governance/Internal.hs` — `RatifyEnv`, `RatifyState`, `reorderActions`, `actionPriority`

---

## 1. When is the Pulser Snapshot Taken?

The pulser is created at the **END of the EPOCH transition**, not at the start of the new epoch.

### Exact call sequence in `epochTransition`:

```
1. SNAP rule   (snapshots1)
2. POOLREAP rule
3. extractDRepPulsingState (consume OLD pulser → rsEnactState, rsEnacted, rsExpired)
4. apply enacted withdrawals
5. proposalsApplyEnactment (remove enacted/expired proposals)
6. govState1  ← apply enactment results
7. certState2, chainAccountState3, utxoState2 ← update all state
8. epochState2 ← conditionally run HARDFORK rule
9. setFreshDRepPulsingState eNo stakePoolDistr epochState2   ← CREATE NEW PULSER
```

The critical point: `setFreshDRepPulsingState` is called with `epochState2`, which is the **fully updated state after all epoch transitions have completed**. The snapshot is therefore taken from:
- Accounts (delegations, rewards) from the NEW epoch's starting state
- Proposals that survived enactment (newly submitted proposals from the ending epoch are included; enacted/expired ones are excluded)
- Instant stake from the new epoch's utxoState
- `stakePoolDistr` = `ssStakeMarkPoolDistr snapshots1` = mark snapshot from SNAP rule

### When is `setFreshDRepPulsingState` called?

It is called via `liftSTS` at the bottom of `epochTransition`. This runs inside the `ConwayNEWEPOCH` rule's epoch-boundary branch. Looking at `newEpochTransition`:

```haskell
-- NewEpoch: when eNo == succ eL (epoch boundary):
es2 <- trans @(EraRule "EPOCH" era) $ TRC ((), es1, eNo)
-- ...
-- The pool distribution is captured here from snapshots BEFORE EPOCH (es0)
let pd' = ssStakeMarkPoolDistr (esSnapshots es0)
pure $ nes { nesEs = es2, nesPd = pd', ... }
```

But inside `epochTransition`, `setFreshDRepPulsingState` uses:
```haskell
stakePoolDistr = ssStakeMarkPoolDistr snapshots1  -- from SNAP output
```

The stakePoolDistr passed to the pulser is `ssStakeMarkPoolDistr snapshots1` (the mark pool distribution AFTER the new SNAP run), NOT `nesPd` from `newEpochTransition`.

---

## 2. What Data is Snapshotted

All fields captured at epoch boundary via `setFreshDRepPulsingState`:

| Field | Source | Notes |
|-------|--------|-------|
| `dpPulseSize` | `floor (numAccounts / (4 * k))` | min 1; mainnet k=2160 |
| `dpAccounts` | `dState ^. accountsL` | Full accounts map (staking cred → AccountState) after epoch transition |
| `dpIndex` | `0` | Iteration cursor starts at 0 |
| `dpInstantStake` | `utxoState ^. instantStakeG` | Incremental stake snapshot from UTxO state |
| `dpStakePoolDistr` | `ssStakeMarkPoolDistr snapshots1` | Mark pool distribution (lazy!) |
| `dpDRepDistr` | `Map.empty` | Starts empty; built by pulsing |
| `dpDRepState` | `vsDReps vState` | Registered DRep credentials + expiry |
| `dpCurrentEpoch` | `epochNo` | The new epoch number |
| `dpCommitteeState` | `vsCommitteeState vState` | Committee hot/cold credential mapping |
| `dpEnactState` | `mkEnactState govState & ensTreasuryL .~ treasury` | Constitution, committee, PParams, prevGovActionIds + current treasury |
| `dpProposals` | `proposalsActions props` | All surviving proposals (as StrictSeq ordered by insertion) |
| `dpProposalDeposits` | `proposalsDeposits props` | Per-credential aggregate proposal deposit amounts |
| `dpGlobals` | `globals` | Protocol globals (k, active slot coeff, etc.) |
| `dpStakePools` | `epochState ^. epochStateStakePoolsL` | Pool params for SPO default vote computation |

**CRITICAL**: `dpStakePoolDistr` is **lazy** (intentional, see ADR-7). It references `ssStakeMarkPoolDistr` which is a thunk that will be forced only when RATIFY needs it. The DRep distribution (`dpDRepDistr`) starts empty and is built incrementally through pulsing.

---

## 3. How the Pulser Works (Spreading Computation Across the Epoch)

### Pulse size formula:
```haskell
pulseSize = max 1 (numAccounts / (4 * k))
```
On mainnet with k=2160, and ~1M accounts: `pulseSize ≈ 1,000,000 / 8,640 ≈ 116` accounts per block.

The intent: spread account-iteration over `4k` blocks (~the first 4k/f slots of the epoch before the stability window). This ensures the EnactState is available at least `6k/f` slots before epoch end.

### Per-block pulse: `newEpochTransition` (non-boundary blocks)

```haskell
-- When eNo /= succ eL (not epoch boundary): just pulse + predict
pure $
  nes
    & newEpochStateDRepPulsingStateL %~ pulseDRepPulsingState
    & newEpochStateGovStateL %~ predictFuturePParams
```

Each TICK (i.e., each block) calls `pulseDRepPulsingState`, which:
1. If `DRComplete` → no-op
2. If `DRPulsing` → call `pulse` on the `DRepPulser`, advancing `dpIndex` by `dpPulseSize`
3. If `done` after pulse → call `finishDRepPulser` → transitions to `DRComplete`

### `computeDRepDistr` (the per-chunk work)

For each `AccountState` in the chunk:
- **DRep delegation**: add `instantStake + proposalDeposit + balance` to `dpDRepDistr[dRep]`, but only if `dRep` is `AlwaysAbstain`, `AlwaysNoConfidence`, or a registered `DRepCredential`
- **Pool delegation**: add only `proposalDeposit` to `poolDistr[pool].individualTotalPoolStake`

Note: SPO stake+rewards are already in `poolDistr` from the SNAP rule; only proposal deposits need to be added by the pulser.

### `finishDRepPulser`: forced completion

Called when EPOCH rule needs the result. Processes all remaining accounts (`Map.drop dpIndex`) in one shot, then runs RATIFY:

```haskell
finishDRepPulser (DRPulsing DRepPulser{..}) =
  ( PulsingSnapshot dpProposals finalDRepDistr dpDRepState (Map.map individualTotalPoolStake finalStakePoolDistr)
  , ratifyState'
  )
  where
    leftOver = Map.drop dpIndex (dpAccounts ^. accountsMapL)
    (finalDRepDistr, finalStakePoolDistr) = computeDRepDistr ... leftOver
    ratifyEnv = RatifyEnv { reInstantStake = dpInstantStake
                          , reStakePoolDistr = finalStakePoolDistr
                          , reDRepDistr = finalDRepDistr
                          , reDRepState = dpDRepState
                          , reCurrentEpoch = dpCurrentEpoch
                          , reCommitteeState = dpCommitteeState
                          , reAccounts = dpAccounts
                          , reStakePools = dpStakePools
                          }
    ratifyState = RatifyState { rsEnactState = dpEnactState, rsEnacted = mempty
                              , rsExpired = mempty, rsDelayed = False }
    ratifyState' = runConwayRatify dpGlobals ratifyEnv ratifyState (RatifySignal dpProposals)
```

**RATIFY is run once, at forced completion**, using the epoch-boundary snapshot.

### RATIFY processes proposals in priority order

`runConwayRatify` calls `reorderActions` on the signal before processing:
```haskell
runConwayRatify globals ratifyEnv ratifyState (RatifySignal ratifySig) =
  applySTS (TRC (ratifyEnv, ratifyState, RatifySignal $ reorderActions ratifySig))

actionPriority:
  NoConfidence        = 0   (highest priority)
  UpdateCommittee     = 1
  NewConstitution     = 2
  HardForkInitiation  = 3
  ParameterChange     = 4
  TreasuryWithdrawals = 5
  InfoAction          = 6   (lowest priority)
```

---

## 4. When Ratification Results are Applied (Enacted)

At the **start of the next EPOCH transition**, the old pulser is consumed by `extractDRepPulsingState`:

```haskell
-- In epochTransition for epoch E+1:
let pulsingState = epochState0 ^. epochStateDRepPulsingStateL
    ratifyState@RatifyState{rsEnactState, rsEnacted, rsExpired} =
      extractDRepPulsingState pulsingState
```

`extractDRepPulsingState` forces completion if not already done (`finishDRepPulser`), returning `RatifyState`.

Then the enacted and expired results are applied:
```haskell
(newProposals, enactedActions, removedDueToEnactment, expiredActions) =
  proposalsApplyEnactment rsEnacted rsExpired (govState0 ^. proposalsGovStateL)
```

And the EnactState values are written to GovState:
```haskell
govState1 =
  govState0
    & cgsProposalsL .~ newProposals
    & cgsCommitteeL .~ ensCommittee        -- may be new committee
    & cgsConstitutionL .~ ensConstitution  -- may be new constitution
    & cgsCurPParamsL .~ nextEpochPParams govState0  -- may be new PParams
    & cgsPrevPParamsL .~ curPParams
    & cgsFuturePParamsL .~ PotentialPParamsUpdate Nothing
```

### Timeline Summary (epoch E → epoch E+1)

```
[Epoch E starts]
  → EPOCH rule runs (for epoch E transition → epoch E+1):
    1. SNAP:        take snapshots (mark/set/go rotate)
    2. POOLREAP:    remove retired pools
    3. extractDRepPulsingState: consume pulser created at epoch E-1 boundary
                                → get RatifyState for epoch E proposals
    4. applyEnactedWithdrawals: distribute treasury withdrawals
    5. proposalsApplyEnactment: remove enacted/expired proposals
    6. Apply EnactState → govState1 (new committee, constitution, pparams)
    7. HARDFORK:    if protocol version changed
    8. setFreshDRepPulsingState epochNo stakePoolDistr epochState2
                    → create NEW pulser for epoch E+1 proposals

[Epoch E+1 runs (blocks 1..N)]
  → Each TICK: pulseDRepPulsingState (advance dpIndex by pulseSize)
  → After ~4k blocks: pulser transitions to DRComplete automatically

[Epoch E+1 ends → Epoch E+2 EPOCH rule]
  → extractDRepPulsingState: consume E+1 pulser (may call finishDRepPulser if not complete)
  → Apply E+1 ratification results
```

**Net result**: Proposals submitted during epoch E are ratified using data snapshotted at the E→E+1 boundary. Results become effective at the E+1→E+2 boundary.

---

## 5. Key Types

### `DRepPulsingState era`
```haskell
data DRepPulsingState era
  = DRPulsing !(DRepPulser era Identity (RatifyState era))
  | DRComplete !(PulsingSnapshot era) !(RatifyState era)
```
Stored in `ConwayGovState.cgsDRepPulsingState`.

### `PulsingSnapshot era`
```haskell
data PulsingSnapshot era = PulsingSnapshot
  { psProposals :: !(StrictSeq (GovActionState era))  -- proposals at epoch boundary
  , psDRepDistr :: !(Map DRep (CompactForm Coin))      -- final DRep stake distribution
  , psDRepState :: !(Map (Credential DRepRole) DRepState)  -- registered DReps
  , psPoolDistr :: Map (KeyHash StakePool) (CompactForm Coin)  -- pool stake (total, not individual)
  }
```
Available from `DRComplete` after pulsing completes. This is what `GetDRepState` / `GetGovState` queries return.

### `RatifyEnv era`
```haskell
data RatifyEnv era = RatifyEnv
  { reInstantStake     :: !(InstantStake era)
  , reStakePoolDistr   :: PoolDistr
  , reDRepDistr        :: !(Map DRep (CompactForm Coin))
  , reDRepState        :: !(Map (Credential DRepRole) DRepState)
  , reCurrentEpoch     :: !EpochNo
  , reCommitteeState   :: !(CommitteeState era)
  , reAccounts         :: Accounts era
  , reStakePools       :: !(Map (KeyHash StakePool) StakePoolState)
  }
```

### `RatifyState era`
```haskell
data RatifyState era = RatifyState
  { rsEnactState :: !(EnactState era)   -- accumulated enactment (grows as proposals ratify)
  , rsEnacted    :: !(Seq (GovActionState era))  -- ratified proposals in order
  , rsExpired    :: !(Set GovActionId)  -- expired proposal IDs
  , rsDelayed    :: !Bool               -- True if a delaying action was enacted this pass
  }
```

---

## 6. Torsten Divergence — The Bug

Torsten evaluates ratification against **live ledger state** each epoch boundary instead of the **frozen epoch-boundary snapshot**.

### What must be fixed:

1. **At epoch boundary (EPOCH rule start)**: call `extractDRepPulsingState` on the PREVIOUS epoch's pulser to get ratification results. Do NOT re-run RATIFY with current live state.

2. **After applying epoch transitions**: call `setFreshDRepPulsingState` to create a NEW pulser with a snapshot of the post-transition state.

3. **During the epoch (each block/tick)**: advance the pulser by `pulseSize` accounts via `pulseDRepPulsingState`. This is called from `newEpochTransition` when `eNo != succ eL`.

4. **The DRep distribution** must be computed by iterating over `dpAccounts` (the snapshotted accounts map), not from live state. For each account: add `instantStake + proposalDeposit + reward_balance` to the delegated DRep's bucket.

5. **Pool distribution**: the pulser starts with `ssStakeMarkPoolDistr` (from SNAP) and only **adds proposal deposits** per pool per delegating account. It does NOT recompute pool stake from scratch.

6. **RATIFY input**: proposals passed to RATIFY must be `reorderActions(dpProposals)` — the snapshot proposals sorted by `actionPriority`, not the live proposals.

### Correct epoch-boundary sequence in Torsten:

```
epoch_transition(epoch_state, new_epoch_no):
  1. snapshots = run_snap(epoch_state)
  2. stake_pool_distr = snapshots.mark_pool_distr
  3. epoch_state = run_poolreap(epoch_state, new_epoch_no)
  4. ratify_state = extract_drep_pulsing_state(epoch_state.gov_state.drep_pulsing_state)
                   // This finishes pulsing if not yet complete
  5. apply enacted/expired from ratify_state to proposals
  6. apply new committee/constitution/pparams from ratify_state.enact_state
  7. epoch_state = run_hardfork_if_needed(epoch_state)
  8. epoch_state.gov_state.drep_pulsing_state =
       create_fresh_pulser(new_epoch_no, stake_pool_distr, epoch_state)
       // Snapshot is taken HERE from post-transition state

per_block_tick(new_epoch_state, block):
  if not epoch_boundary:
    new_epoch_state.gov_state.drep_pulsing_state =
      pulse_drep_pulsing_state(new_epoch_state.gov_state.drep_pulsing_state)
```
