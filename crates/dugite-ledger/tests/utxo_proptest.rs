//! Property-based tests for UTxO invariants (Properties 1–5).
//!
//! Each property is cross-validated against the Haskell cardano-ledger rules.
//! All properties use 256 test cases.
//!
//! # Haskell cross-validation notes
//!
//! ## Property 1 — Per-transaction ADA conservation
//!
//! Haskell's UTXO rule enforces `consumed == produced` where:
//!   - `consumed  = Σ(input.coin)  + Σ(withdrawals) + Σ(deposit_refunds)`
//!   - `produced  = Σ(output.coin) + fee + Σ(deposits_paid) + donation`
//!
//! Minting does NOT appear in the ADA conservation equation — it only affects
//! multi-asset token balances (Rule 3b), not lovelace. Each transaction is
//! balanced individually; the ledger never aggregates across transactions.
//!
//! For the ADA-only transactions generated here:
//!   `withdrawals = 0`, `deposit_refunds = 0`, `deposits_paid = 0`, `donation = 0`
//! so the equation simplifies to:
//!   `Σ(input.coin) == Σ(output.coin) + fee`
//!
//! ## Property 2 — Multi-asset conservation
//!
//! Haskell's Rule 3b: for every (policy_id, asset_name) pair,
//!   `Σ(inputs[policy][name]) + mint[policy][name] == Σ(outputs[policy][name])`
//!
//! For ADA-only transactions, all multi-asset maps are empty — the property is
//! trivially satisfied. This property establishes the baseline; extension to
//! minting transactions is left for Property 2 of the full 10-property suite.
//!
//! ## Property 3 — Minimum UTxO value enforcement
//!
//! Haskell's Rule 5 (Babbage/Conway formula):
//!   `min_coins = coinsPerUTxOByte * (160 + serialized_output_size)`
//!
//! The constant 160 is the `overhead` term in the Haskell implementation
//! (UTxO entry key overhead: TxIn = 32-byte hash + 4-byte index = 36 bytes,
//! plus 124 bytes for the output pointer and CBOR framing = 160 total).
//!
//! A simple ADA-only output is ~29 bytes serialized, giving:
//!   `min_coins = 4310 * (160 + 29) = 4310 * 189 = 814,590 lovelace ≈ 0.814 ADA`
//!
//! Our generator uses a minimum of 1,000,000 lovelace (1 ADA), which comfortably
//! satisfies the mainnet minimum.
//!
//! ## Property 4 — Rollback restores exact UTxO state
//!
//! Haskell's `DiffMK` rollback: to undo a block, the diff records both
//! `inserts` (outputs to remove) and `deletes` (inputs to restore). Rollback
//! applies the inverse: remove every inserted output, re-insert every spent input.
//!
//! `DiffSeq::rollback(n)` returns the last n diffs in reverse order (most
//! recent first). The caller is responsible for applying the undo to the UTxO set.
//!
//! ## Property 5 — DiffSeq flush_up_to behavior
//!
//! DiffSeq has no automatic capacity limit — eviction is caller-driven via
//! `flush_up_to(slot)`. After the call, all diffs with `slot <= flush_slot` are
//! removed; diffs with `slot > flush_slot` are retained unchanged.
//!
//! This matches Haskell's `forgetUpTo` semantics in the `Ouroboros.Consensus`
//! volatile DB implementation.

#[path = "strategies.rs"]
mod strategies;

use dugite_ledger::{DiffSeq, UtxoDiff, UtxoSet};
use dugite_primitives::address::{Address, ByronAddress};
use dugite_primitives::hash::Hash32;
use dugite_primitives::protocol_params::ProtocolParameters;
use dugite_primitives::time::SlotNo;
use dugite_primitives::transaction::{OutputDatum, TransactionInput, TransactionOutput};
use dugite_primitives::value::Value;
use proptest::prelude::*;
use strategies::{arb_utxo_set, build_simple_tx};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Create a minimal `TransactionOutput` with a specific lovelace amount and
/// a zero-filled Byron payload.  This is the same output shape used by the
/// strategy generators, making the size estimate consistent.
fn make_output(lovelace: u64) -> TransactionOutput {
    TransactionOutput {
        address: Address::Byron(ByronAddress {
            payload: vec![0u8; 32],
        }),
        value: Value::lovelace(lovelace),
        datum: OutputDatum::None,
        script_ref: None,
        is_legacy: false,
        raw_cbor: None,
    }
}

// ---------------------------------------------------------------------------
// Property 1: Per-transaction ADA conservation
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// For every balanced, ADA-only transaction:
    ///
    /// ```text
    /// Σ(consumed_input.coin) == Σ(output.coin) + fee
    /// ```
    ///
    /// This is the simplified form of the full Haskell conservation identity
    /// when withdrawals = 0, deposit_refunds = 0, deposits_paid = 0, donation = 0.
    ///
    /// We generate a UTxO set, pick 1–3 inputs, compute a valid output value
    /// from the sum of consumed inputs minus the fee, and assert the identity.
    /// Minting is NOT included — it does not appear in the ADA balance.
    #[test]
    fn prop_per_tx_ada_conservation(
        // Generate a UTxO set with 3..=5 entries.  Indices 0..n_inputs are consumed.
        (utxo_set, inputs) in arb_utxo_set(5),
        // Fee drawn from a realistic range; kept small so output_value stays positive.
        fee in 200_000u64..500_000u64,
        // How many inputs to consume (1, 2, or 3 of the 5 available).
        n_inputs in 1usize..=3usize,
    ) {
        // ── Step 1: Identify consumed inputs and sum their lovelace ──────────
        //
        // We consume the first `n_inputs` from the generated set, which are
        // guaranteed to have distinct transaction IDs by the arb_utxo_set
        // generator.
        let consumed_inputs: Vec<TransactionInput> = inputs.into_iter().take(n_inputs).collect();

        let consumed_total: u64 = consumed_inputs
            .iter()
            .filter_map(|inp| utxo_set.lookup(inp))
            .map(|out| out.value.coin.0)
            .sum();

        // ── Step 2: Ensure the fee does not exceed consumed lovelace ─────────
        //
        // When consumed_total <= fee the output value would underflow; skip.
        // This is expected in a small fraction of cases with low-value inputs.
        prop_assume!(consumed_total > fee);
        let output_value = consumed_total - fee;

        // ── Step 3: Build the balanced transaction ────────────────────────────
        //
        // build_simple_tx places exactly one output with `output_value` lovelace
        // and sets `body.fee = fee`.  No withdrawals, certificates, minting, or
        // deposits are present.
        let tx = build_simple_tx(consumed_inputs.clone(), output_value, fee);

        // ── Step 4: Assert the ADA conservation identity ─────────────────────
        //
        // LHS: sum of all consumed input coins
        // RHS: sum of all outputs (single output here) + fee
        let lhs = consumed_total;
        let rhs: u64 = tx.body.outputs.iter().map(|o| o.value.coin.0).sum::<u64>()
            + tx.body.fee.0;

        prop_assert_eq!(
            lhs, rhs,
            "ADA conservation violated: consumed={}, outputs+fee={}",
            lhs, rhs
        );

        // ── Step 5: Verify the Haskell full identity with zero extra terms ────
        //
        // For completeness: confirm that withdrawals, deposits, refunds, and
        // donations are all zero in the generated transaction, so the simplified
        // form is equivalent to the full Haskell formula.
        let withdrawals_total: u64 = tx.body.withdrawals.values().map(|l| l.0).sum();
        let donation = tx.body.donation.map_or(0, |l| l.0);
        // No certificate-based deposits/refunds in this simple transaction.

        // Full identity: consumed + withdrawals + refunds == produced + fee + deposits + donation
        // Rearranged: consumed + withdrawals == produced + fee + deposits - refunds + donation
        // With all extra terms == 0: consumed == produced + fee  (verified above)
        prop_assert_eq!(
            withdrawals_total, 0u64,
            "Expected zero withdrawals in generated simple tx"
        );
        prop_assert_eq!(donation, 0u64, "Expected zero donation in generated simple tx");
    }
}

// ---------------------------------------------------------------------------
// Property 2: Multi-asset conservation (ADA-only baseline)
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// For ADA-only transactions, the multi-asset map is empty on both the input
    /// and output sides, and no minting occurs.  The conservation identity
    ///
    /// ```text
    /// Σ(inputs[policy][name]) + mint[policy][name] == Σ(outputs[policy][name])
    /// ```
    ///
    /// is trivially satisfied for every (policy, name) pair (vacuously true over
    /// an empty set).
    ///
    /// # Haskell Rule 3b
    ///
    /// The Haskell ledger's `valueMismatch` check collects the multi-asset parts
    /// of `consumed` and `produced` maps and verifies they are equal after adding
    /// `mint` to the input side.  For ADA-only transactions the multi-asset map
    /// is `mempty`, so `consumed_ma + mint == produced_ma` becomes `0 + 0 == 0`.
    ///
    /// This test establishes that the generated transactions carry no unexpected
    /// multi-asset values — it is the baseline from which minting extension tests
    /// (Properties 7–10 of the full suite) build.
    #[test]
    fn prop_multi_asset_conservation(
        (utxo_set, inputs) in arb_utxo_set(3),
        fee in 200_000u64..500_000u64,
        n_inputs in 1usize..=2usize,
    ) {
        let consumed_inputs: Vec<TransactionInput> = inputs.into_iter().take(n_inputs).collect();

        let consumed_total: u64 = consumed_inputs
            .iter()
            .filter_map(|inp| utxo_set.lookup(inp))
            .map(|out| out.value.coin.0)
            .sum();

        prop_assume!(consumed_total > fee);
        let output_value = consumed_total - fee;

        let tx = build_simple_tx(consumed_inputs, output_value, fee);

        // ── Multi-asset assertion ─────────────────────────────────────────────

        // Verify: every input output has an empty multi-asset map.
        for inp in &tx.body.inputs {
            if let Some(out) = utxo_set.lookup(inp) {
                prop_assert!(
                    out.value.multi_asset.is_empty(),
                    "Input UTxO carries multi-assets; expected ADA-only"
                );
            }
        }

        // Verify: every transaction output has an empty multi-asset map.
        for out in &tx.body.outputs {
            prop_assert!(
                out.value.multi_asset.is_empty(),
                "Transaction output carries multi-assets; expected ADA-only"
            );
        }

        // Verify: the mint map is empty.
        prop_assert!(
            tx.body.mint.is_empty(),
            "Expected empty mint map in ADA-only transaction"
        );

        // With all multi-asset maps empty, Rule 3b is trivially satisfied:
        // for every (policy, name): 0 + 0 == 0.
        // No per-policy assertion needed — the empty-map checks above are sufficient.
    }
}

// ---------------------------------------------------------------------------
// Property 3: Minimum UTxO value enforcement
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Every UTxO entry generated by `arb_utxo_set` satisfies the Babbage/Conway
    /// minimum-value formula:
    ///
    /// ```text
    /// coin >= coinsPerUTxOByte * (160 + serialized_output_size)
    /// ```
    ///
    /// For a simple ADA-only output with a 32-byte Byron address payload, the
    /// serialized size is approximately 29 bytes (CBOR-encoded output record),
    /// giving a mainnet minimum of:
    ///
    /// ```text
    /// min_coins = 4310 * (160 + 29) = 4310 * 189 = 814,590 lovelace ≈ 0.814 ADA
    /// ```
    ///
    /// Our generator enforces a lower bound of 1,000,000 lovelace (1.0 ADA),
    /// which strictly exceeds the mainnet minimum for this output shape.
    ///
    /// # Haskell Rule 5
    ///
    /// Haskell's `outputTooSmallUTxO` predicate rejects outputs where
    /// `getValue txOut < minUTxOValue pp txOut`.  The Babbage `minUTxOValue`
    /// uses `utxoEntrySize` = 160 + `utxoEntrySize txOut`.  In Conway this is
    /// carried forward unchanged.
    #[test]
    fn prop_min_utxo_value_enforcement(
        (utxo_set, inputs) in arb_utxo_set(10),
    ) {
        let params = ProtocolParameters::mainnet_defaults();

        // Serialized size of a simple ADA-only output (no datum, no script ref,
        // 32-byte Byron address payload).  The CBOR encoding of this output is
        // approximately 29 bytes.  We use this conservative estimate to compute
        // the minimum; the generator always produces at least 1 ADA.
        let simple_output_size_bytes: u64 = 29;
        let min_coins = params.min_utxo_for_output_size(simple_output_size_bytes);

        for inp in &inputs {
            let output = utxo_set.lookup(inp).expect("Input missing from generated UTxO set");

            prop_assert!(
                output.value.coin >= min_coins,
                "UTxO {inp:?} has coin={} < min_utxo={}: violates minimum UTxO invariant",
                output.value.coin.0,
                min_coins.0,
            );

            // Additionally verify coin > 0 (trivially implied by the above, but
            // stated explicitly to document the Haskell `NoZeroedAdaInUTxO` rule
            // which was added in Babbage — a subset of the minUTxO check).
            prop_assert!(
                output.value.coin.0 > 0,
                "UTxO {inp:?} has zero coin — violates NoZeroedAdaInUTxO"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Property 4: Rollback restores exact UTxO state
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// After applying a transaction to a UTxO set and recording the diff,
    /// rolling back via `DiffSeq::rollback` + manual undo restores the UTxO
    /// set to its exact pre-application state.
    ///
    /// The test flow:
    /// 1. Generate a UTxO set with 3 entries (inputs A, B, C).
    /// 2. Snapshot the UTxO set (clone — tests use in-memory mode).
    /// 3. Apply a transaction: spend input A, add new output O.
    ///    Record the diff (`deletes = [(A, out_A)]`, `inserts = [(O_ref, out_O)]`).
    /// 4. Push the diff to a `DiffSeq`.
    /// 5. Pop the diff via `rollback(1)` and apply the undo:
    ///    - Remove the inserted output (O_ref).
    ///    - Re-insert the deleted input (A, out_A).
    /// 6. Assert: UTxO set equals the snapshot.
    ///
    /// # Haskell DiffMK semantics
    ///
    /// Haskell's `rewindTableKeySets` applies the inverse of each diff in
    /// reverse order.  The inverse of an `Insert(k, v)` is `Delete(k)` and the
    /// inverse of a `Delete(k, v)` is `Insert(k, v)`.  For a single block this
    /// is equivalent to what we do here.
    #[test]
    fn prop_rollback_restores_utxo(
        (utxo_set, inputs) in arb_utxo_set(3),
        // Destination output lovelace.  Kept well above min UTxO.
        output_lovelace in 1_000_000u64..3_000_000u64,
    ) {
        // ── Step 1: Snapshot pre-application state ───────────────────────────
        //
        // Clone the in-memory UtxoSet.  `UtxoSet::clone()` copies the HashMap
        // but leaves the LSM store as None — always in-memory for tests.
        let snapshot: Vec<(TransactionInput, TransactionOutput)> = utxo_set.iter();

        // ── Step 2: Choose the input to spend ────────────────────────────────
        let input_to_spend = inputs[0].clone();
        let original_output = utxo_set
            .lookup(&input_to_spend)
            .expect("Input must exist in generated UTxO set");

        // Ensure the fee is less than the consumed value so the output is valid.
        let fee = 200_000u64;
        let consumed_coin = original_output.value.coin.0;
        prop_assume!(consumed_coin > fee + output_lovelace);

        // ── Step 3: Apply the transaction manually ────────────────────────────
        //
        // We do not go through `validate_transaction` here — this tests the
        // UTxO diff mechanics, not the validation pipeline.
        let mut live_utxo: UtxoSet = {
            let mut u = UtxoSet::new();
            for (inp, out) in &snapshot {
                u.insert(inp.clone(), out.clone());
            }
            u
        };

        // New output reference: tx_hash derived from a deterministic seed.
        let tx_hash = Hash32::from_bytes([0xDDu8; 32]);
        let new_input = TransactionInput { transaction_id: tx_hash, index: 0 };
        let new_output = make_output(output_lovelace);

        // Build the diff before touching the live UTxO set.
        let mut diff = UtxoDiff::new();
        diff.record_delete(input_to_spend.clone(), original_output.clone());
        diff.record_insert(new_input.clone(), new_output.clone());

        // Apply: remove spent input, add new output.
        live_utxo.remove(&input_to_spend);
        live_utxo.insert(new_input.clone(), new_output);

        prop_assert!(!live_utxo.contains(&input_to_spend), "Spent input should be absent");
        prop_assert!(live_utxo.contains(&new_input), "New output should be present");

        // ── Step 4: Record in DiffSeq and rollback ───────────────────────────
        let mut diff_seq = DiffSeq::new();
        diff_seq.push(SlotNo(1000), tx_hash, diff);
        prop_assert_eq!(diff_seq.len(), 1);

        let rolled_back = diff_seq.rollback(1);
        prop_assert_eq!(rolled_back.len(), 1, "Exactly one diff should be returned");
        prop_assert!(diff_seq.is_empty(), "DiffSeq should be empty after full rollback");

        // ── Step 5: Apply the undo ────────────────────────────────────────────
        //
        // Undo the diff: delete every insert, re-insert every delete.
        let (_, _, undo_diff) = rolled_back.into_iter().next().unwrap();

        for (ins_inp, _) in &undo_diff.inserts {
            live_utxo.remove(ins_inp);
        }
        for (del_inp, del_out) in &undo_diff.deletes {
            live_utxo.insert(del_inp.clone(), del_out.clone());
        }

        // ── Step 6: Verify exact restoration ─────────────────────────────────
        //
        // The live UTxO must contain exactly the same entries as the snapshot —
        // same keys, same values.  We compare as sorted vectors for determinism.
        let mut restored: Vec<(TransactionInput, TransactionOutput)> = live_utxo.iter();
        let mut expected = snapshot;

        // Sort by (tx_id bytes, index) for a stable comparison.
        let sort_key = |e: &(TransactionInput, TransactionOutput)| {
            let id_bytes: [u8; 32] = *e.0.transaction_id.as_bytes();
            (id_bytes, e.0.index)
        };
        restored.sort_by_key(sort_key);
        expected.sort_by_key(sort_key);

        prop_assert_eq!(
            restored.len(), expected.len(),
            "UTxO set size mismatch after rollback: got {}, expected {}",
            restored.len(), expected.len()
        );

        for (got, exp) in restored.iter().zip(expected.iter()) {
            prop_assert!(
                got.0 == exp.0,
                "Input key mismatch after rollback: {:?} != {:?}", got.0, exp.0
            );
            prop_assert!(
                got.1.value.coin == exp.1.value.coin,
                "Output coin mismatch after rollback for input {:?}: {} != {}",
                got.0, got.1.value.coin.0, exp.1.value.coin.0
            );
        }

        prop_assert!(
            !live_utxo.contains(&new_input),
            "Rolled-back output should be absent from UTxO set"
        );
        prop_assert!(
            live_utxo.contains(&input_to_spend),
            "Rolled-back input should be restored in UTxO set"
        );
    }
}

// ---------------------------------------------------------------------------
// Property 5: DiffSeq flush_up_to behavior
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// `DiffSeq::flush_up_to(slot)` removes exactly the diffs whose slot
    /// is `<= slot` and leaves the rest unmodified.
    ///
    /// Test structure:
    /// 1. Push N diffs at slots [10, 20, 30, ..., N*10].
    /// 2. Choose a flush slot in [0, N*10] (may flush 0..N diffs).
    /// 3. Call `flush_up_to(flush_slot)`.
    /// 4. Assert:
    ///    - (a) `len()` equals the count of diffs with `slot > flush_slot`.
    ///    - (b) The remaining diffs in `diffs` all have `slot > flush_slot`.
    ///    - (c) The returned flushed list has the correct length.
    ///
    /// # Haskell `forgetUpTo` semantics
    ///
    /// In Ouroboros.Consensus the "flush" operation discards diffs that are
    /// below the immutability horizon.  Once flushed, those diffs can never
    /// be used for rollback — the chain has been committed to immutable storage.
    /// The postcondition is identical to what we verify here.
    #[test]
    fn prop_diff_seq_flush_behavior(
        // Number of diffs: 1..=10 (small for fast iteration)
        n_diffs in 1usize..=10usize,
        // Flush slot factor: 0..=n_diffs, so flush_slot = factor * 10.
        // - factor = 0        → flush nothing (slot < 10)
        // - factor = n_diffs  → flush everything
        // - 0 < factor < n_diffs → partial flush
        flush_factor in 0usize..=10usize,
    ) {
        // Clamp flush_factor to [0, n_diffs] to avoid flushing beyond the range.
        let flush_factor = flush_factor.min(n_diffs);
        let flush_slot = SlotNo((flush_factor as u64) * 10);

        // ── Step 1: Build the DiffSeq ─────────────────────────────────────────
        let mut diff_seq = DiffSeq::new();
        for i in 1..=(n_diffs as u64) {
            let slot = SlotNo(i * 10);
            // Give each diff a distinct hash from its slot number.
            let mut hash_bytes = [0u8; 32];
            hash_bytes[..8].copy_from_slice(&(i * 10).to_be_bytes());
            let hash = Hash32::from_bytes(hash_bytes);
            diff_seq.push(slot, hash, UtxoDiff::new());
        }
        prop_assert_eq!(diff_seq.len(), n_diffs);

        // ── Step 2: Record expected counts before flushing ───────────────────
        //
        // Diffs with slot <= flush_slot will be removed.
        // Diffs with slot >  flush_slot will remain.
        let expected_flushed_count = (1..=(n_diffs as u64))
            .filter(|i| SlotNo(i * 10) <= flush_slot)
            .count();
        let expected_remaining_count = n_diffs - expected_flushed_count;

        // ── Step 3: Flush ─────────────────────────────────────────────────────
        let flushed = diff_seq.flush_up_to(flush_slot);

        // ── Step 4: Assertions ────────────────────────────────────────────────

        // (a) Returned list has the correct length.
        prop_assert_eq!(
            flushed.len(), expected_flushed_count,
            "flush_up_to({}) returned {} diffs; expected {}",
            flush_slot.0, flushed.len(), expected_flushed_count
        );

        // (b) DiffSeq.len() equals the number of remaining diffs.
        prop_assert_eq!(
            diff_seq.len(), expected_remaining_count,
            "After flush_up_to({}): seq.len()={}, expected {}",
            flush_slot.0, diff_seq.len(), expected_remaining_count
        );

        // (c) Every remaining diff has slot > flush_slot.
        for (slot, _, _) in &diff_seq.diffs {
            prop_assert!(
                *slot > flush_slot,
                "Diff at slot {} should have been flushed (flush_slot={})",
                slot.0, flush_slot.0
            );
        }

        // (d) Flush-then-rollback: rolling back all remaining diffs leaves an
        //     empty sequence (boundary condition).
        let remaining_count = diff_seq.len();
        let rolled = diff_seq.rollback(remaining_count);
        prop_assert_eq!(
            rolled.len(), expected_remaining_count,
            "Rollback after flush should return exactly the remaining diffs"
        );
        prop_assert!(diff_seq.is_empty(), "DiffSeq should be empty after full rollback");
    }
}
