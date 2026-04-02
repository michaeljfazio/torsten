---
name: crate-pallas-math
description: pallas-math fixed-point arithmetic, VRF leader check math, and comparison with dugite's ported implementation
type: reference
---

# pallas-math (v1.0.0-alpha.5)

## Overview

Description: "Mathematics functions for Cardano". Author: Andrew Westberg. Implements exact fixed-point arithmetic needed for Cardano's VRF leader election check.

## Module Structure

```
pallas_math::
  math::       // Abstract traits: FixedPrecision, FixedDecimal, ExpOrdering, ExpCmpOrdering
  math_dashu:: // Concrete implementation using dashu-int IBig/UBig
```

## Dependencies

- `dashu-int` 0.4.1 — big integer arithmetic
- `dashu-base` 0.4.1 — base utilities
- `regex` 1.10.5 — for string parsing
- `thiserror` 1.0.61 — error handling

## Public Types

### From `math` module

```rust
pub trait FixedPrecision: Sized + ... {
    fn new(precision: u64) -> Result<Self, Error>;
    fn from_str(string: &str, precision: u64) -> Result<Self, Error>;
    fn precision(&self) -> u64;
    fn exp(&self) -> Self;          // e^x using continued fractions
    fn ln(&self) -> Self;           // ln(x), domain: x > 0
    fn pow(&self, exponent: &Self) -> Self;  // x^y = exp(y * ln(x))
    fn exp_cmp(max_iterations: u64, bound: &Self, compare: &Self) -> ExpCmpOrdering;
    fn round(&self) -> Self;
    fn floor(&self) -> Self;
    fn ceil(&self) -> Self;
    fn trunc(&self) -> Self;
    // + arithmetic ops: neg, mul, div, sub
}

pub struct FixedDecimal { ... }  // Abstract type (maps to Decimal in dashu impl)

pub enum ExpOrdering { GT, LT, UNKNOWN }

pub struct ExpCmpOrdering {
    pub iters: u64,          // iterations taken
    pub estimation: String,  // debug: decimal estimation
    pub approx: ExpOrdering, // final comparison result
}

pub enum Error {
    RegexError(regex::Error),
    NulByte,
}
```

### From `math_dashu` module

```rust
pub struct Decimal {
    // Implements FixedPrecision using dashu_int IBig/UBig
    // 34-digit fixed-point precision (E34)
}

// Utility functions:
pub fn div(rop: &mut IBig, x: &IBig, y: &IBig)  // division with precision scaling
pub fn scale(rop: &mut IBig)                      // scale by precision multiplier
```

## Mathematical Operations

The library implements **transcendental functions** (exp, ln, pow) using:

1. **exp()**: Euler continued fraction expansion — converges for all x ≥ 0
2. **ln()**: Derived from exp via ln(1+x) using continued fraction (`lncf`)
3. **exp_cmp()**: Taylor series for exp() with error bounds for early comparison termination (taylorExpCmp)

**Precision**: E34 — 34 decimal digits of precision, matching Haskell's `FixedPoint E34`

## VRF Leader Check Algorithm

The library implements the exact arithmetic needed for Cardano's Ouroboros Praos leader election:

```
Haskell reference:
  φ(f) = 1 - (1-f)^σ     where σ = relative stake, f = active slot coefficient
  Leader check: VRF_output / certNatMax < φ(f)
```

This requires `exp_cmp(taylorExpCmp)` for efficient comparison without computing exact exp value.

## Dugite vs pallas-math

**Key decision**: Dugite did NOT adopt pallas-math. Instead, it ported the algorithms directly into `dugite-crypto` using `dashu-int` IBig directly.

### Why dugite ported instead of adopted:

From memory notes: "VRF math was ported FROM pallas-math into dugite-crypto using dashu directly"

Likely reasons:
1. Wanted to avoid adding pallas-math as a dependency
2. Needed to customize for dugite's specific VRF context types
3. The algorithms are well-understood and small enough to maintain directly

### Dugite's VRF implementation details (from project memory):

- Uses Euler continued fraction (NOT Taylor series) for ln(1+x) — matches Haskell's `lncf`
- Uses Taylor series for `taylorExpCmp` with error bounds for early termination
- 34-digit fixed-point arithmetic via `dashu-int` IBig
- TPraos vs Praos distinction: Shelley-Alonzo (proto < 7) uses raw 64-byte VRF output with certNatMax=2^512; Babbage/Conway (proto >= 7) uses Blake2b-256("L"||output) with certNatMax=2^256

## Adoption Recommendation

**IMPLEMENT FROM SCRATCH** (already done). Dugite's ported implementation is already working correctly. Re-introducing pallas-math as a dependency would add no benefit and could introduce version incompatibility risks. The algorithms are stable and well-tested in dugite.

**Monitoring note**: If pallas-math adds new functionality (e.g., reward calculation math, or new VRF modes for future protocol versions), re-evaluate. For now, maintain dugite's direct dashu implementation.
