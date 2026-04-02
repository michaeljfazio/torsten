# VRF Leader Eligibility Check - Complete Algorithm

## File Locations
- `checkLeaderValue` / `checkLeaderNatValue` / `BoundedNatural`: `cardano-ledger/libs/cardano-protocol-tpraos/src/Cardano/Protocol/TPraos/BHeader.hs`
- `taylorExpCmp` / `CompareResult`: `cardano-ledger/libs/non-integral/src/Cardano/Ledger/NonIntegral.hs`
- `FixedPoint` / `ActiveSlotCoeff` / `activeSlotLog` / `fpPrecision`: `cardano-ledger/libs/cardano-ledger-core/src/Cardano/Ledger/BaseTypes.hs`
- `vrfLeaderValue` / `mkInputVRF`: `ouroboros-consensus/ouroboros-consensus-protocol/src/ouroboros-consensus-protocol/Ouroboros/Consensus/Protocol/Praos/VRF.hs`
- `meetsLeaderThreshold` / `checkIsLeader`: `ouroboros-consensus/ouroboros-consensus-protocol/src/ouroboros-consensus-protocol/Ouroboros/Consensus/Protocol/Praos.hs`

## Algorithm Summary

### Step 1: Domain Separation
- leader_hash = Blake2b-256("L" || raw_vrf_output)  [32 bytes]
- certNat = bytes_to_natural(leader_hash)  [big-endian]
- certNatMax = 2^(8*32) = 2^256

### Step 2: Compute recip_q (using FixedPoint = Fixed E34, 34 decimal places)
- recip_q = certNatMax / (certNatMax - certNat)
- This is 1/(1-p) where p = certNat/certNatMax

### Step 3: Compute x
- c = activeSlotLog(f) = ln(1-f) precomputed as integer/10^34
- x = -sigma * c = sigma * |ln(1-f)|

### Step 4: Taylor comparison
- taylorExpCmp(boundX=3, cmp=recip_q, x) compares recip_q against exp(x)
- ABOVE => certNat too big => NOT leader
- BELOW => certNat small enough => IS leader
- MaxReached => NOT leader (conservative)

### taylorExpCmp algorithm
```
go(maxN=1000, n=0, err=x, acc=1, divisor=1):
  divisor' = divisor + 1
  nextX = err           # first iteration: nextX = x
  err' = (err * x) / divisor'
  acc' = acc + nextX    # accumulates Taylor: 1 + x + x²/2! + x³/3! + ...
  errorTerm = |err' * boundX|
  if cmp >= acc' + errorTerm: ABOVE  (cmp is definitely above exp(x))
  if cmp < acc' - errorTerm: BELOW   (cmp is definitely below exp(x))
  else: recurse
```
- boundX=3 is the error bound multiplier
- errorTerm bounds the remaining Taylor series tail
- Converges fast for small x (typical sigma * |ln(0.95)| is small)

### FixedPoint Precision
- type FixedPoint = Fixed E34 (34 decimal places, resolution 10^34)
- fpPrecision = 10^34
- All arithmetic in exact fixed-point, no floating point

### Dugite Bug
Current Dugite uses f64 floating point (`vrf_output_to_fraction_full` + `powf`), which DOES NOT match Haskell's exact integer/fixed-point arithmetic. Edge cases near the threshold boundary will differ.

### Correct Rust Implementation Requirements
1. Convert 32-byte leader hash to BigUint (certNat)
2. certNatMax = BigUint::from(2u32).pow(256)
3. recip_q = Rational(certNatMax, certNatMax - certNat) as fixed-point
4. c = precompute ln(1-f) using continued fraction ln' at 34-digit precision
5. x = -sigma * c (where sigma is Rational)
6. taylorExpCmp(3, recip_q, x) using 34-digit fixed-point
7. Need a Rust fixed-point type with 34 decimal digits (or use num-bigint Rational)
