---
name: RUPD Timing and Data Flow
description: Complete trace of reward update computation timing, snapshot usage, fee flow, and epoch-0 behavior in Haskell cardano-ledger
type: reference
---

## RUPD Timing Windows

- `randomnessStabilisationWindow` (sr) = `ceiling(4k/f)` — NOT `ceiling(3*(1/f - 1))`
- `stabilityWindow` = `ceiling(3k/f)` (for PParam updates, not rewards)
- Preview (k=432, f=0.05): sr = 34560, stabilityWindow = 25920
- RUPD fires in TICK (every block): `startAfterSlot = epochFirst + sr`, `endSlot = epochFirst + 2*sr`
- `RewardsTooEarly`: slot <= startAfterSlot → return SNothing
- `RewardsJustRight`: startAfterSlot < slot <= endSlot → pulse
- `RewardsTooLate`: slot > endSlot → force complete

## TICK/RUPD Environment Extraction (Tick.hs bheadTransition)

```
nes0 = (NewEpochState _ bprev _ es _ _ _)   -- state BEFORE NEWEPOCH
nes1 = validatingTickTransition nes0 slot     -- fires NEWEPOCH if epoch changed
RUPD gets: RupdEnv bprev es (from nes0), nesRu nes1 (from after NEWEPOCH)
```
Key: `bprev` and `es` come from the pre-NEWEPOCH state. `nesRu` from post-NEWEPOCH (which is SNothing after boundary).

## NEWEPOCH Ordering (Conway)

1. `applyRUpd` (complete pulser, apply to EpochState) — es0 → es1
2. `EPOCH` rule (contains SNAP → POOLREAP → RATIFY → HARDFORK) — es1 → es2
3. `nesBprev = bcur`, `nesBcur = empty`, `nesRu = SNothing`
4. `nesPd = ssStakeMarkPoolDistr(esSnapshots(es0))` — from BEFORE applyRUpd

## SNAP Rule Captures (Snap.hs)

```
ssFee = utxosFees (from current LedgerState, AFTER applyRUpd)
ssStakeMark = new mark snapshot (computed from current instant stake)
ssStakeSet = old ssStakeMark
ssStakeGo = old ssStakeSet
```

## startStep Data Sources (PulsingReward.hs)

- `ssStakeGo(esSnapshots(es))` — the go snapshot at time of pulsing
- `ssFee(esSnapshots(es))` — fee field at time of pulsing
- `b = nesBprev` — blocks from PREVIOUS epoch (captured at boundary)
- `casReserves(esChainAccountState(es))` — current reserves
- `eta | d >= 0.8 = 1 | otherwise = blocksMade/expectedBlocks`
- `deltaR1 = floor(min(1,eta) * rho * reserves)`
- `rPot = ssFee + deltaR1`
- `deltaT1 = floor(tau * rPot)`
- `_R = rPot - deltaT1`

## Fee Flow Lifecycle

1. Tx fees → utxosFees (accumulated during block processing)
2. At boundary: applyRUpd subtracts `deltaF = -ssFee` from utxosFees
3. SNAP captures remaining utxosFees into new ssFee
4. Next epoch's RUPD reads ssFee for rPot calculation
5. Net effect: ssFee = one epoch's worth of fees

## Epoch 0 Behavior (Preview)

- nesBprev = empty, esSnapshots = emptySnapShots (ssFee=0, ssStakeGo=empty)
- d=1.0 → eta=1 (d>=0.8 guard)
- reserves = 15,000,000,000,000,000 (45T - 30T Byron UTxOs)
- deltaR1 = floor(0.003 * 15T) = 45,000,000,000,000
- rPot = 0 + 45T = 45,000,000,000,000
- deltaT1 = floor(0.2 * 45T) = 9,000,000,000,000 ← matches Koios exactly
- No pools → _R all goes to deltaR2, rs=empty
- Epoch 0 fees (87,558) NOT included in first RUPD (ssFee=0)

## Dugite Divergence

- Uses `snapshots.set` instead of Haskell's `ssStakeGo` — 1 epoch ahead
- Block count from snapshot instead of `nesBprev`
- Fee reset via zeroing instead of `deltaF` subtraction
- Works for epoch 0 (all empty) but structurally divergent for later epochs
