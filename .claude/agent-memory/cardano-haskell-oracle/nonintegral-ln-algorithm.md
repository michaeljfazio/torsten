# NonIntegral ln' Algorithm Details

## Source Location
`libs/non-integral/src/Cardano/Ledger/NonIntegral.hs` in cardano-ledger repo

## Algorithm: Generalized Continued Fraction (NOT Taylor Series)

`ln'(x)` works in two steps:
1. `splitLn(x)` finds integer `n` with `e^n <= x < e^(n+1)`, computes `x' = x/e^n - 1`
2. `lncf(1000, x')` computes `ln(1+x')` via continued fraction

### Continued fraction for ln(1+x):
- a_1 = x, a_{2k} = a_{2k+1} = k^2 * x  (k >= 1)
- b_n = n (n >= 0)
- Convergence epsilon = 10^(-24)
- Max 1000 iterations (typically converges in 20-50)

### cf evaluator uses recurrence:
- A_n = b_n * A_{n-1} + a_n * A_{n-2}
- B_n = b_n * B_{n-1} + a_n * B_{n-2}
- convergent = A_n / B_n

## FixedPoint Type
```
data E34
instance HasResolution E34 where resolution _ = 10^34
type FixedPoint = Fixed E34  -- newtype over Integer
fpPrecision = 10^34
```

## activeSlotLog Precomputation
`ln'` called ONCE in `mkActiveSlotCoeff`:
```
unActiveSlotLog = floor(fpPrecision * ln'(1 - f))  -- stored as Integer
activeSlotLog f = fromIntegral(unActiveSlotLog f) / fpPrecision  -- retrieved per-block
```

## Leader Check Path (per-block)
`checkLeaderNatValue` only calls `taylorExpCmp` (exp comparison), not `ln'`.
The `c = activeSlotLog f` is the precomputed ln value.

## VRF Leader Value Range Extension
- Haskell hashes: Blake2b_256("L" || vrf_output_bytes) → 32 bytes
- certNatMax = 2^(8*32) = 2^256 (NOT 2^512)
- This matches Dugite's implementation

## Critical Precision Notes
- Haskell passes sigma as exact Rational (ratio of two Integer)
- Active slot coeff is exact Rational from genesis (e.g., 1/20 not 0.05)
- fromRational on Fixed E34 does exact integer division: floor(num/denom * 10^34)
- f64 cannot exactly represent 0.05 (binary: 0x3FA999999999999A ≈ 0.050000000000000003)

## Dugite Bug: Uses Taylor series instead of continued fraction
The Taylor series `ln(1+y) = y - y^2/2 + y^3/3 - ...` gives different fixed-point
truncation than the continued fraction, causing boundary-case disagreements.
Additionally, Dugite converts f64→fixed-point losing precision vs Haskell's
exact Rational→Fixed conversion.
