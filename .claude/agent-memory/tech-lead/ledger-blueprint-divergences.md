---
name: Cardano Blueprint divergences fixed 2026-03-16
description: Three critical spec divergences identified and fixed from Blueprint review
type: project
---

Three divergences from the Cardano Blueprint were identified and corrected:

## Fix 1: Reference script fee ceiling vs floor

**File:** `crates/torsten-ledger/src/validation/scripts.rs`

**Bug:** `calculate_ref_script_tiered_fee` used `(acc_num / acc_den) as u64` (floor).

**Fix:** Changed to `acc_num.div_ceil(acc_den) as u64`.

**Why:** The Cardano Blueprint `transaction-fee.md` explicitly says "you should take the ceiling as a last step" for the tiered reference script fee. Using floor causes a 1-lovelace undercount on transactions where the rational accumulation has a non-zero remainder.

**How to apply:** Any time the ref script fee formula is touched, remember that the FINAL step is ceiling, not floor. The intermediate accumulation remains exact rational arithmetic.

## Fix 2: Block-level totalRefScriptSize check

**File:** `crates/torsten-ledger/src/state/apply.rs`

**Bug:** Missing block-level aggregate ref script size check. Only per-tx fee calculation existed.

**Fix:** Added check in `apply_block` for Conway+ era (protocol >= 9), `ValidateAll` mode only:
- `MAX_REF_SCRIPT_SIZE_PER_BLOCK = 1024 * 1024` (1 MiB) — Haskell: `ppMaxRefScriptSizePerBlockG = L.to . const $ 1024 * 1024`
- `MAX_REF_SCRIPT_SIZE_PER_TX = 200 * 1024` (200 KiB) — documented but enforced via fee calculation
- Both are hardcoded constants in Haskell, NOT protocol parameters

**Why:** Haskell's `conwayBbodyTransition` enforces `BodyRefScriptsSizeTooBig` when the sum of `txNonDistinctRefScriptsSize` across all txs in the block exceeds the limit.

**Caution:** The task description incorrectly stated the per-block limit is 204800 bytes — that is the per-TRANSACTION limit. The per-block limit is 1 MiB (1048576 bytes).

## Fix 3: Chain selection tiebreaker (Praos)

**File:** `crates/torsten-consensus/src/chain_selection.rs`

**Bug:** Equal-length chains used header hash as tiebreaker (not spec-compliant).

**Fix:** New `prefer_chain_with_headers()` API implementing the full Blueprint spec:
1. Same pool (same issuer_vkey → pool_id via blake2b_224): **higher opcert counter wins**
2. Different pools: **lower VRF output value wins** (lexicographic byte comparison)
3. Conway (era == Era::Conway): VRF comparison only within `slot_window` slots; otherwise PreferCurrent to prevent late blocks from displacing established chain

**Why:** Spec from Blueprint `chainsel.md` — prevents geographic centralization incentives. Without the tiebreaker, arrival order (network latency) determines chain selection, incentivizing co-location.

**Backward compat:** `prefer_chain()` (hash-based) kept for callers without BlockHeader access.

## Related pre-existing fixes

- `torsten-serialization/src/multi_era.rs`: Fixed `KeepRaw<T>.as_ref()` compile error (uses `Deref` not `AsRef`; fixed to `x` with auto-deref). Also fixed `alonzo::AuxiliaryData` unresolved module (should be `PallasAux`).
- `crates/torsten-ledger/src/state/tests.rs`: Fixed `test_epoch_nonce_computation` expectation — `update_evolving_nonce` applies `blake2b_256` to the eta input before combining.
