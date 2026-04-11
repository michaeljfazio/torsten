# Sub-project 4 — Block-body & Witness Completion

**Parent:** [Ledger Completion Decomposition](2026-04-11-ledger-completion-decomposition.md)
**Date:** 2026-04-11
**Closes:** `state/apply.rs:193` (#377); `eras/common.rs:665-717`; `eras/alonzo.rs:212,549`; `eras/babbage.rs:52,547`; `eras/conway.rs:76,180-186,1522`
**Depends on:** Nothing — independent of sub-projects 1-3

---

## Problem

Six distinct rule holes in the block-/tx-validation path. They're grouped here because they share the same architectural driver: the era-rules trait was extracted in `2026-04-08-era-rules-trait-design.md`, but several rules were left as `Ok(())` stubs on the expectation that a follow-up would extract them from `validation/mod.rs`. That extraction never finished.

### Hole 1 — Block body size/hash equality (#377)

**File:** `state/apply.rs:185-210`

Current behavior: compares `header.body_size > max_block_body_size` and errors if exceeded. Haskell BBODY rule computes the actual serialized body size and checks `actual == header.body_size`. The current check is both the *wrong predicate* (inequality vs. equality) and at the *wrong layer* (max-block-body-size is a `chainChecks` header-level constraint, not a BBODY rule).

### Hole 2 — `validate_shelley_base` empty stub

**File:** `eras/common.rs:665-717`

Declared as the shared Phase-1 entry point for Shelley+ eras but is unimplemented. Every era rule impl still dispatches through `validation/mod.rs::validate_transaction_with_pools`. This means:
- Each era has a copy of the rule-dispatch logic (OK because the per-era rules differ)
- But the shared rules 1-10 (InputsExist, FeeSufficient, TTLValid, ValuePreserved, OutputTooSmall, OutputBootAddress, TxSizeLimit, NetworkMismatch, WitnessSetComplete, CollateralValid) are implemented once in `validation/phase1.rs` and called via `validate_transaction_with_pools`

The stub exists but isn't called. It's either a real gap (the extraction is needed for correctness) or dead code (extraction is cosmetic). We treat it as the latter: **delete the stub, document in the trait that era impls call `validation::phase1::validate_transaction_with_pools` for rules 1-10**. Rationale: the extraction doesn't change behavior; maintaining an unused stub is worse than acknowledging the current structure.

### Hole 3 — Block-level ExUnits budget (Alonzo/Babbage/Conway)

**Files:** `eras/alonzo.rs:549`, `eras/babbage.rs:547`, `eras/conway.rs:1522`

All three era impls of `validate_block_body` return `Ok(())` for the ExUnits budget. Haskell's `Alonzo/Rules/Bbody.hs` checks `sum tx.totExUnits ≤ pparams.maxBlockExUnits`. This is the block-level analogue of the per-tx ExUnits check that is implemented; both are required.

### Hole 4 — Babbage script-size limits

**File:** `eras/babbage.rs:52`

Babbage introduced reference scripts; the `max_script_size` (`ppMaxValSize` / `ppMaxScriptSize`) constraint isn't enforced. The per-tx `validate_block_body` returns `Ok(())`.

### Hole 5 — Conway ref-script-size

**File:** `eras/conway.rs:76`

Conway adds `maxRefScriptSizePerBlock` — the sum of inline and reference-script bytes across the *entire* block body must not exceed this limit. `eras/conway.rs:92` already implements the ref-script tiered fee for individual transactions (`BodyRefScriptsSizeTooBig`), but the block-wide cap isn't enforced. Worth re-reading — this item may already be done; verify first.

### Hole 6 — Alonzo Plutus witness rules

**File:** `eras/alonzo.rs:212`

Alonzo's `required_witnesses` implementation says "matches Shelley's witness logic for now, plus Plutus script requirements" but only the Shelley half is present. The Plutus side requires:
- Collateral input witnesses
- Datum witnesses for every input whose datum-hash is referenced
- Redeemer presence for every Plutus-script-locked input
- Required signers list (for Plutus scripts that use `ExtraSignatures`)

Babbage and Conway impls reuse this (`_ => shelley_required_witnesses(tx)`), so they inherit the gap for Plutus-v1 inputs. Fixing Alonzo fixes all three.

## Goal

Close all six holes so that:

1. `ValidateAll`-mode block application rejects any block whose serialized body bytes hash or length differs from the header claim.
2. The `validate_shelley_base` stub is either filled in or deleted with a comment explaining the decision.
3. Alonzo/Babbage/Conway reject blocks whose total ExUnits exceed `maxBlockExUnits`.
4. Babbage rejects transactions whose script bytes exceed `maxScriptSize` (or, if this param doesn't exist as a standalone, enforces via the existing value-size constraint).
5. Conway rejects blocks whose total reference-script bytes exceed `maxRefScriptSizePerBlock` (block-wide, distinct from the tx-level fee).
6. Alonzo (and thus Babbage/Conway) require datum/redeemer/collateral witnesses on every Plutus-locked input.

## Non-goals

- Phase-2 Plutus script execution (already implemented in `plutus.rs`).
- Reference script tiered fee formula (already correct per `validation/scripts.rs:363`, cross-verified by the correctness-bugs sub-project 2026-04-09).
- ExUnits budget *per-tx* (already enforced).
- Any changes to the `BlockValidator` trait shape.

## Design

### Hole 1 — Body size equality (#377)

**File:** `state/apply.rs`

Current `raw_cbor` stores the full block CBOR. Two approaches:

**A (chosen).** At block-parse time, record the body byte range within `raw_cbor`: `body_byte_start: usize, body_byte_len: usize`. The parser already walks the CBOR tree; capturing these offsets is ~10 LoC in `dugite-serialization`. Then the equality check is just `block.body_byte_len == block.header.body_size`.

**B.** Re-serialize `block.body` at validation time. More expensive (allocates) but avoids touching the parser.

Option A is cheap and correct. Go with A.

```rust
// state/apply.rs, replacing lines 185-217
if mode == BlockValidationMode::ValidateAll {
    let actual = block.body_byte_len as u64;
    let claimed = block.header.body_size;
    if actual != claimed {
        return Err(LedgerError::WrongBlockBodySize { actual, claimed });
    }
}
```

Delete the `max_block_body_size` comparison here entirely — it's a consensus-layer chainChecks concern, not a ledger BBODY concern. If consensus isn't already enforcing it, that's a separate issue for the consensus crate.

**Haskell reference:** `Shelley/Rules/Bbody.hs::validateBlockBodySize`.

### Hole 2 — Delete the `validate_shelley_base` stub

**File:** `eras/common.rs`

Delete the function. Replace with a single doc comment on the `BlockValidator::validate_transaction` trait method explaining that era impls dispatch to `validation::phase1::validate_transaction_with_pools` for rules 1-10, then apply era-specific rules.

Update the module-level table at `eras/common.rs:24` to remove the `validate_shelley_base` row.

If, during implementation, there turns out to be a real reason to share a wrapper (e.g., future rule additions), re-introduce as a thin dispatch function — but don't carry dead code.

### Hole 3 — Block-level ExUnits budget

**File:** `eras/common.rs` add `validate_block_ex_units_per_block`, called from each era's `validate_block_body`

```rust
/// Sum redeemer ExUnits across all txs in the block, reject if > maxBlockExUnits.
pub(crate) fn validate_block_ex_units_per_block(
    block: &Block,
    pparams: &ProtocolParameters,
) -> Result<(), LedgerError> {
    let max = pparams.max_block_ex_units;
    let mut total = ExUnits { mem: 0, steps: 0 };
    for tx in &block.body.transactions {
        if !tx.is_valid { continue; } // Phase-2-failing txs don't count
        if let Some(wit) = &tx.witness_set {
            for r in wit.redeemers.iter().flatten() {
                total.mem = total.mem.saturating_add(r.ex_units.mem);
                total.steps = total.steps.saturating_add(r.ex_units.steps);
            }
        }
    }
    if total.mem > max.mem || total.steps > max.steps {
        return Err(LedgerError::BlockExUnitsExceeded { total, max });
    }
    Ok(())
}
```

Call from `AlonzoRules::validate_block_body`, `BabbageRules::validate_block_body`, `ConwayRules::validate_block_body`. Replace the `Ok(())` stubs.

**Haskell reference:** `Alonzo/Rules/Bbody.hs::validateBlockExUnitsTotal`.

### Hole 4 — Babbage script-size limits

**File:** `eras/babbage.rs`

Per Haskell `Babbage/Rules/Utxo.hs::validateOutputTooBigUTxO`, the Babbage constraint is `sizeOf(serialize output) ≤ max_val_size`, enforced **per output**, not per-script-size. Verify that `validation/phase1.rs` already enforces `max_val_size`. If yes, the `babbage.rs:52` "for now we return Ok(())" comment is a misleading TODO — delete it with a note that the value-size check already fires via `validation/phase1.rs::check_output_value_size`. If no, implement the check in `phase1.rs` and route through.

Estimated: 10 LoC change plus a comment.

### Hole 5 — Conway block-wide ref-script size

**File:** `eras/conway.rs:76` + `eras/common.rs`

Re-read `eras/conway.rs:92` first. If it already sums ref-script bytes across the block body, this hole is phantom. If it only sums per-tx, add a block-wide sum:

```rust
// In ConwayRules::validate_block_body
let max = pparams.max_ref_script_size_per_block;
let mut total_ref_script_bytes: u64 = 0;
for tx in &block.body.transactions {
    if let Some(ref_scripts) = &tx.body.reference_scripts {
        for script in ref_scripts {
            total_ref_script_bytes += script.cbor_size() as u64;
        }
    }
}
if total_ref_script_bytes > max {
    return Err(LedgerError::BodyRefScriptsSizeTooBig { total: total_ref_script_bytes, max });
}
```

Delete the stub comment at line 76 once implemented.

### Hole 6 — Alonzo Plutus witness rules

**File:** `eras/alonzo.rs`

Add to `AlonzoRules::required_witnesses` (building on the Shelley base set):

1. **Collateral input witnesses** — for each input in `tx.body.collateral`, the payment key credential (if vkey-typed) must appear in required witnesses. Script-typed collateral is rejected by Phase-1 already.
2. **Datum witnesses** — for each Plutus-script-locked input, if the UTxO output has a datum *hash* (not inline), the witness set's `plutus_data` must contain a datum matching that hash.
3. **Redeemer presence** — for each Plutus-script-locked input, a redeemer with the matching `RedeemerTag::Spend` and input index must exist.
4. **Required signers** — all vkey hashes in `tx.body.required_signers` must appear in required witnesses (this exists in Shelley already; verify inclusion in Alonzo path).

Implementation: four new checks in a helper `validate_plutus_witnesses` called from `AlonzoRules::validate_transaction` after `validate_shelley_base-equivalent` runs. Each check walks the tx body and produces a witness requirement.

Babbage and Conway already call through to Alonzo's implementation for Plutus-v1; Babbage adds Plutus-v2 and reference-script handling (already implemented per the era-rules-trait spec). Conway adds Plutus-v3 (already implemented). So fixing Alonzo is sufficient.

**Haskell reference:** `Alonzo/Rules/Utxow.hs::missingRequiredDatums`, `hasExactSetOfRedeemers`, `requiredSignersAreWitnessed`.

### Validation

- **Unit test 1** — `test_bbody_size_mismatch_rejected`: hand-craft a block whose header.body_size differs from the actual body byte count; assert error.
- **Unit test 2** — `test_bbody_size_matches_accepted`: positive case.
- **Unit test 3** — `test_block_ex_units_over_budget_rejected`: block whose summed redeemer ExUnits exceed `maxBlockExUnits`; assert error.
- **Unit test 4** — `test_block_ex_units_skips_invalid_txs`: invalid txs (is_valid = false) are excluded from the sum.
- **Unit test 5** — `test_conway_ref_script_block_cap_rejected`: block whose reference-script bytes exceed the per-block cap.
- **Unit test 6** — `test_alonzo_missing_datum_witness_rejected`: Plutus input whose datum is absent from the witness set.
- **Unit test 7** — `test_alonzo_missing_redeemer_rejected`: Plutus input without a matching redeemer.
- **Unit test 8** — `test_alonzo_collateral_witness_required`: collateral input with unsigned payment credential.
- **Property test** — for arbitrary valid blocks, all six checks pass; for arbitrary mutations (flip body size, inflate ExUnits, drop a datum), the appropriate check fires.
- **Golden fixture** — a real preview block that contains Plutus transactions with datums; replay through dugite; assert identical acceptance as Haskell.

## Risk / tradeoffs

- **Body-byte-range threading.** Adding `body_byte_len` to `Block` touches `dugite-serialization` and every constructor of `Block` in tests. ~30 call sites. Tedious but mechanical.
- **ExUnits summation overflow.** `saturating_add` prevents panics, but a block whose summed ExUnits exceed `u64::MAX` would silently pass as `MAX`. Use `checked_add` instead and fail on overflow — matches Haskell's `Integer` semantics.
- **Babbage value-size vs script-size conflation.** If the current code already enforces value size but Haskell's Babbage has no separate `maxScriptSize`, the TODO at `babbage.rs:52` is misleading and should just be deleted with a note.
- **Datum witness check requires UTxO read.** `required_witnesses` currently only sees `tx` + `&ctx`. To check "this input's datum hash matches a witness-provided datum," we need `&UtxoSubState`. Either pass it through the call or move the check to `validate_transaction` where the borrow already exists.
- **Conway ref-script cap may already be implemented at line 92.** Re-read before writing code; don't duplicate.

## Order of operations

1. **Hole 1 first** — smallest blast radius. Add `body_byte_len` to `Block`. Update serialization. Add test, fix call sites.
2. **Hole 2** — delete the stub, update the comment table. Trivial.
3. **Hole 3** — add `validate_block_ex_units_per_block` in `common.rs`. Call from Alonzo/Babbage/Conway.
4. **Hole 5** — verify Conway state at line 92; add block-wide cap if missing.
5. **Hole 4** — verify Babbage state; delete misleading TODO or add check.
6. **Hole 6** — Alonzo Plutus witnesses. Biggest of the six. Do last so the simpler holes' test infra is already in place.
7. Clippy + fmt + nextest.

## Done when

- `rg -n 'TODO|FIXME' crates/dugite-ledger/src/state/apply.rs crates/dugite-ledger/src/eras/common.rs crates/dugite-ledger/src/eras/alonzo.rs crates/dugite-ledger/src/eras/babbage.rs` returns zero (ignoring sub-project-3 territory in conway.rs).
- All eight unit tests pass.
- Property test passes.
- Golden Plutus block from preview replays without error or acceptance delta.
- Clippy clean.
