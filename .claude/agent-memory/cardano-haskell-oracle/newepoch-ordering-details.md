---
name: NEWEPOCH Ordering Details
description: Exact ordering of applyRUpd/SNAP/POOLREAP/block rotation/nesRu reset in NEWEPOCH, bprev source for RUPD, protocol update timing, incrBlocks d value
type: reference
---

## NEWEPOCH Full Operation Order (Shelley & Conway identical structure)

1. **applyRUpd** (complete pulser if Pulsing, apply RewardUpdate to EpochState)
   - rewards credited to accounts, deltaF subtracted from fees, treasury/reserves adjusted
2. **MIR** (Shelley only; no-op in Conway)
3. **EPOCH** sub-rule, which internally runs:
   a. **SNAP** (rotate mark→set→go, capture new mark from instant stake, ssFee=utxosFees)
   b. **POOLREAP** (retire pools, return deposits)
   c. **UPEC** (Shelley) / **RATIFY+HARDFORK** (Conway) — apply protocol parameter updates
   d. Recompute `utxosDeposited`, set `prevPParams = old curPParams`, `curPParams = pp'`
4. **Record update** (pure, all at once as return value):
   - `nesEL = eNo`
   - `nesBprev = bcur` (old nesBcur, blocks from the epoch that just ended)
   - `nesBcur = BlocksMade mempty`
   - `nesEs = es_final` (from step 3)
   - `nesRu = SNothing`
   - `nesPd = ssStakeMarkPoolDistr(esSnapshots(es_original))` (from BEFORE applyRUpd!)

## bprev for RUPD: Where It Comes From

In TICK (bheadTransition), `bprev` and `es` are destructured from `nes0` (pre-NEWEPOCH).
- First slot of epoch N: bprev = blocks from epoch N-2 (but RUPD returns SNothing — too early)
- All subsequent slots in epoch N: nes0 has post-NEWEPOCH state, so bprev = blocks from epoch N-1
- Net effect: RUPD during epoch N always computes rewards using blocks from epoch N-1

## Protocol Parameter Update Timing (Shelley PPUP)

- Two voting periods per epoch, split by `tooLate = firstSlotNextEpoch - 2*stabilityWindow`
- Before tooLate: proposals target current epoch → sgsCurProposals
- After tooLate: proposals target next epoch → sgsFutureProposals
- At epoch boundary: futureProposals promoted to curProposals, old curProposals applied via UPEC
- **Proposals targeting epoch E take effect at the (E-1)→E boundary**
  (they were in sgsCurProposals during epoch E-1, applied by UPEC at that boundary)

## incrBlocks d Value

- Uses `curPParams` from the post-TICK state (BbodyEnv receives pp from CHAIN rule)
- For epoch N: this is the curPParams set at the (N-1)→N boundary
- Conway: d=0 always, so isOverlaySlot always returns False, all blocks counted
