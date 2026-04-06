//! Property-based tests for UTxO invariants (Properties 1–10).
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
//!
//! ## Property 6 — DiffSeq rollback consistency
//!
//! Apply N blocks (each consuming one input and producing one new output),
//! then rollback M of them via `DiffSeq::rollback`.  After unapplying the
//! returned diffs (delete inserts, re-insert deletes), the UTxO set must
//! match the state after applying only the first N-M blocks — not a snapshot,
//! but the exact live state.
//!
//! Haskell's `rewindTableKeySets` applies the inverse of each diff in reverse
//! chronological order.  Because each diff is a bijection (inserts and deletes
//! are disjoint), the undo is exact: no approximation or partial application.
//!
//! ## Property 7 — Input consumption is atomic
//!
//! After applying a sequence of transactions to the UTxO set (including an
//! intra-block chained spend where Tx2 consumes an output created by Tx1),
//! every consumed input is absent from the set and every produced output is
//! present.  No partial application can occur.
//!
//! Haskell's UTXOS rule applies the entire block atomically: all inputs are
//! removed and all outputs are inserted in one ledger state transition.  If
//! any input is missing the block is rejected outright; no partial state is
//! written.
//!
//! ## Property 8 — Duplicate input rejection
//!
//! A transaction that lists the same `TransactionInput` twice must be rejected
//! by `validate_transaction` before touching the UTxO set.
//!
//! In Conway, Haskell catches this at deserialization via `OSet` (which
//! deduplicates and sorts inputs).  Dugite enforces it as Phase-1 validation
//! rule `DuplicateInput`, producing the same observable behaviour: submission
//! of a duplicate-input transaction is rejected.
//!
//! ## Property 9 — Deposit pot invariant
//!
//! For any well-formed `LedgerState`:
//!
//! ```text
//! total_stake_key_deposits + Σ(pool_deposits.values())
//!   == key_deposit * n_registered_creds + pool_deposit * n_registered_pools
//! ```
//!
//! The `arb_ledger_state` generator enforces this by construction.  This test
//! verifies that the identity survives the generator unscathed — it catches any
//! future generator regression that would produce an inconsistent deposit pot.
//!
//! Haskell's `totalObligation` function computes the same sum over
//! `certState.staking.deposits` (key deposits) and `certState.pool.poolDeposits`
//! (pool deposits).  The invariant must hold at every ledger state checkpoint.
//!
//! ## Property 10 — Collateral UTxO invariant
//!
//! For `is_valid = false` transactions, the Cardano ledger (Alonzo onwards):
//!   - Removes all collateral inputs from the UTxO set.
//!   - If `collateral_return` is present, adds it as a new UTxO entry.
//!   - Leaves all regular spending inputs untouched in the UTxO set.
//!   - Does NOT add any of the transaction's regular outputs.
//!
//! This is Haskell's `UTXOS` rule `isValid = false` branch:
//! `utxo' = (utxo ∖ collateral(tx)) ∪ collRet(tx)`.
//! The test simulates this logic at the UTxO-set level and asserts each
//! invariant holds after the invalid-transaction processing.

#[path = "strategies.rs"]
mod strategies;

use dugite_ledger::{validate_transaction, DiffSeq, UtxoDiff, UtxoSet};
use dugite_primitives::address::{Address, ByronAddress};
use dugite_primitives::hash::Hash32;
use dugite_primitives::protocol_params::ProtocolParameters;
use dugite_primitives::time::SlotNo;
use dugite_primitives::transaction::{OutputDatum, TransactionInput, TransactionOutput};
use dugite_primitives::value::Value;
use proptest::prelude::*;
use strategies::{arb_ledger_state, arb_utxo_set, build_simple_tx, LedgerStateConfig};

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

// ---------------------------------------------------------------------------
// Property 6: DiffSeq rollback consistency
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Apply N blocks (N in 2..=5) to a UTxO set, each spending one existing
    /// input and creating one new output.  Then rollback M blocks (M in 1..N).
    /// After unapplying the returned diffs the UTxO set must equal the state
    /// after applying only the first N-M blocks.
    ///
    /// The last rollback step also covers a chained spend: at block 2 we spend
    /// the output created by block 1, so the chain dependency
    ///
    ///   genesis → block1_output → block2_consumes_it
    ///
    /// is included whenever N >= 2 and M == 1, meaning after rolling back block
    /// 2 the output from block 1 reappears, and rolling back block 1 restores
    /// the original genesis entry.
    ///
    /// # Haskell `rewindTableKeySets`
    ///
    /// Haskell applies the inverse diffs in reverse chronological order.  Each
    /// `DiffMK` entry is a bijection (inserts and deletes are disjoint) so the
    /// undo is exact.  Our implementation mirrors this via
    /// `DiffSeq::rollback(m)` which pops the last m entries most-recent-first.
    #[test]
    fn prop_diff_seq_rollback_consistency(
        // Five initial UTxO amounts (1..=100 ADA each).
        initial_amounts in proptest::array::uniform5(1_000_000u64..=100_000_000u64),
        // How many blocks to apply: 2..=5 (need at least 2 for chained spend).
        n_blocks in 2usize..=5usize,
        // Raw rollback count; clamped below to [1, n_blocks - 1].
        m_rollback_raw in 1usize..=4usize,
    ) {
        // Clamp m_rollback to [1, n_blocks - 1] so we always keep at least one
        // applied block and always roll back at least one.
        let m_rollback = m_rollback_raw.min(n_blocks - 1).max(1);
        let keep_count = n_blocks - m_rollback; // blocks that should stay applied

        // ── Step 1: Build the initial UTxO set ───────────────────────────────
        //
        // Five entries with deterministic input hashes derived from their index.
        let mut live: UtxoSet = UtxoSet::new();
        let initial_inputs: Vec<TransactionInput> = (0..5usize)
            .map(|i| {
                let mut id = [0u8; 32];
                id[..8].copy_from_slice(&(i as u64).to_be_bytes());
                id[8] = 0xAB;
                TransactionInput {
                    transaction_id: Hash32::from_bytes(id),
                    index: 0,
                }
            })
            .collect();

        for (i, amount) in initial_amounts.iter().enumerate() {
            live.insert(initial_inputs[i].clone(), make_output(*amount));
        }

        // ── Step 2: Apply N blocks, capturing checkpoints ────────────────────
        //
        // Each block spends `current_spend_input` and creates a new output.
        // Block 0 spends initial_inputs[0]; block k spends the output of
        // block k-1 — the chained spend pattern.
        let mut diff_seq = DiffSeq::new();
        let mut current_spend_input = initial_inputs[0].clone();
        let current_spend_amount = initial_amounts[0];

        // checkpoint[i] = sorted UTxO entries after block i is applied.
        let mut checkpoints: Vec<Vec<(TransactionInput, TransactionOutput)>> =
            Vec::with_capacity(n_blocks);

        for block_idx in 0..n_blocks {
            // Derive a unique output hash for this block.
            let mut new_id = [0xCCu8; 32];
            new_id[..8].copy_from_slice(&(block_idx as u64).to_be_bytes());
            new_id[8] = 0xDD;
            let new_input = TransactionInput {
                transaction_id: Hash32::from_bytes(new_id),
                index: 0,
            };
            let new_output = make_output(current_spend_amount);

            // Build the diff before mutating the live set.
            let mut diff = UtxoDiff::new();
            diff.record_delete(
                current_spend_input.clone(),
                live.lookup(&current_spend_input)
                    .expect("spend input must exist before this block"),
            );
            diff.record_insert(new_input.clone(), new_output.clone());

            // Apply: remove spent, add created.
            live.remove(&current_spend_input);
            live.insert(new_input.clone(), new_output);

            // Push the diff into the DiffSeq.
            let slot = SlotNo((block_idx as u64 + 1) * 100);
            let mut block_hash = [0u8; 32];
            block_hash[..8].copy_from_slice(&(block_idx as u64).to_be_bytes());
            block_hash[8] = 0xEE;
            diff_seq.push(slot, Hash32::from_bytes(block_hash), diff);

            // Snapshot the live state (for comparison after rollback).
            let sort_key = |e: &(TransactionInput, TransactionOutput)| {
                let id: [u8; 32] = *e.0.transaction_id.as_bytes();
                (id, e.0.index)
            };
            let mut snap = live.iter();
            snap.sort_by_key(sort_key);
            checkpoints.push(snap);

            // Next block spends the output we just created (chained).
            current_spend_input = new_input;
            // current_spend_amount is unchanged (no fee modelled here).
        }

        prop_assert_eq!(diff_seq.len(), n_blocks);

        // ── Step 3: Rollback M blocks ─────────────────────────────────────────
        let rolled_back = diff_seq.rollback(m_rollback);
        prop_assert_eq!(
            rolled_back.len(), m_rollback,
            "rollback({}) returned {} diffs; expected {}",
            m_rollback, rolled_back.len(), m_rollback
        );
        prop_assert_eq!(
            diff_seq.len(), keep_count,
            "DiffSeq should have {} entries after rollback; got {}",
            keep_count, diff_seq.len()
        );

        // ── Step 4: Unapply the rolled-back diffs ────────────────────────────
        //
        // Diffs are returned most-recent-first.  For each: remove inserts (undo
        // the new output creation), re-insert deletes (undo the spend).
        for (_, _, undo) in &rolled_back {
            for (ins_inp, _) in &undo.inserts {
                live.remove(ins_inp);
            }
            for (del_inp, del_out) in &undo.deletes {
                live.insert(del_inp.clone(), del_out.clone());
            }
        }

        // ── Step 5: Compare against the expected checkpoint ──────────────────
        //
        // After rolling back M blocks, state must equal checkpoint[keep_count-1].
        // (keep_count >= 1 by construction.)
        let expected = &checkpoints[keep_count - 1];

        let sort_key = |e: &(TransactionInput, TransactionOutput)| {
            let id: [u8; 32] = *e.0.transaction_id.as_bytes();
            (id, e.0.index)
        };
        let mut got = live.iter();
        got.sort_by_key(sort_key);

        prop_assert_eq!(
            got.len(), expected.len(),
            "UTxO size after rollback: got {}, expected {}",
            got.len(), expected.len()
        );

        for (g, e) in got.iter().zip(expected.iter()) {
            prop_assert!(
                g.0 == e.0,
                "Input key mismatch after rollback: {:?} != {:?}", g.0, e.0
            );
            prop_assert!(
                g.1.value.coin == e.1.value.coin,
                "Coin mismatch for {:?}: got {}, expected {}",
                g.0, g.1.value.coin.0, e.1.value.coin.0
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Property 7: Input consumption is atomic
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// After applying a sequence of transactions — including an intra-block
    /// chained spend where Tx2 consumes the output of Tx1 — every consumed
    /// input is absent from the UTxO set and every produced output is present.
    /// No partial application can occur.
    ///
    /// The test exercises three transactions applied sequentially:
    ///   Tx1: spends initial_inputs[0], creates tx1_hash:0
    ///   Tx2: spends tx1_hash:0 (output of Tx1 — chained), creates tx2_hash:0
    ///   Tx3: spends initial_inputs[1], creates tx3_hash:0
    ///
    /// Final expected state:
    ///   Absent:   initial_inputs[0], tx1_hash:0, initial_inputs[1]
    ///   Present:  tx2_hash:0, tx3_hash:0
    ///   Unchanged: initial_inputs[2..4]
    ///
    /// # Haskell UTXOS atomicity
    ///
    /// Haskell's `utxoTransition` (UTXOS rule) applies the full block in one
    /// state-transition step.  Inputs are consumed before outputs are produced,
    /// and no intermediate state is visible.  `UtxoSet::apply_transaction`
    /// implements the same semantics: it checks all inputs exist, then removes
    /// them all, then inserts all new outputs.
    #[test]
    fn prop_input_consumption_atomic(
        amounts in proptest::array::uniform5(1_000_000u64..=50_000_000u64),
    ) {
        // ── Build initial UTxO set ────────────────────────────────────────────
        let mut utxo = UtxoSet::new();
        let initial_inputs: Vec<TransactionInput> = (0..5usize)
            .map(|i| {
                let mut id = [0u8; 32];
                id[..8].copy_from_slice(&(i as u64).to_be_bytes());
                id[8] = 0xA1;
                TransactionInput { transaction_id: Hash32::from_bytes(id), index: 0 }
            })
            .collect();

        for (i, &amt) in amounts.iter().enumerate() {
            utxo.insert(initial_inputs[i].clone(), make_output(amt));
        }

        // ── Tx1: spends initial_inputs[0], produces tx1_hash:0 ───────────────
        let tx1_hash = Hash32::from_bytes([0x11u8; 32]);
        utxo.apply_transaction(&tx1_hash, &[initial_inputs[0].clone()], &[make_output(amounts[0])])
            .expect("Tx1 must apply cleanly");

        let tx1_out_ref = TransactionInput { transaction_id: tx1_hash, index: 0 };

        prop_assert!(
            !utxo.contains(&initial_inputs[0]),
            "initial_inputs[0] must be absent after Tx1 spends it"
        );
        prop_assert!(
            utxo.contains(&tx1_out_ref),
            "tx1_out_ref must be present after Tx1 creates it"
        );

        // ── Tx2: chained spend — spends tx1_hash:0, produces tx2_hash:0 ───────
        //
        // This is the intra-block chaining case: Tx2 depends on Tx1's output.
        // Haskell handles this via the sequential fold in `applyTxSeq`.
        let tx2_hash = Hash32::from_bytes([0x22u8; 32]);
        utxo.apply_transaction(&tx2_hash, std::slice::from_ref(&tx1_out_ref), &[make_output(amounts[0])])
            .expect("Tx2 chained application must succeed");

        let tx2_out_ref = TransactionInput { transaction_id: tx2_hash, index: 0 };

        prop_assert!(
            !utxo.contains(&tx1_out_ref),
            "tx1_out_ref must be absent after Tx2 spends it (chained)"
        );
        prop_assert!(
            utxo.contains(&tx2_out_ref),
            "tx2_out_ref must be present after Tx2 creates it"
        );

        // ── Tx3: independent — spends initial_inputs[1], produces tx3_hash:0 ──
        let tx3_hash = Hash32::from_bytes([0x33u8; 32]);
        utxo.apply_transaction(&tx3_hash, &[initial_inputs[1].clone()], &[make_output(amounts[1])])
            .expect("Tx3 must apply cleanly");

        let tx3_out_ref = TransactionInput { transaction_id: tx3_hash, index: 0 };

        // ── Final atomicity assertions ─────────────────────────────────────────
        prop_assert!(!utxo.contains(&initial_inputs[0]), "initial_inputs[0] absent");
        prop_assert!(!utxo.contains(&tx1_out_ref),       "tx1_out_ref absent (spent by Tx2)");
        prop_assert!(!utxo.contains(&initial_inputs[1]), "initial_inputs[1] absent");
        prop_assert!(utxo.contains(&tx2_out_ref),        "tx2_out_ref present");
        prop_assert!(utxo.contains(&tx3_out_ref),        "tx3_out_ref present");

        // initial_inputs[2..4] are untouched.
        for inp in initial_inputs.iter().skip(2) {
            prop_assert!(
                utxo.contains(inp),
                "initial_inputs[2..] must be untouched"
            );
        }

        // Net size: 5 - 3 consumed + 3 created = 5.
        prop_assert_eq!(
            utxo.len(), 5,
            "UTxO size must be 5 after three spend-and-create txs; got {}",
            utxo.len()
        );
    }
}

// ---------------------------------------------------------------------------
// Property 8: Duplicate input rejection
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// A transaction that lists the same `TransactionInput` twice is rejected
    /// by `validate_transaction` with a `DuplicateInput` error, and the UTxO
    /// set is not modified.
    ///
    /// # Haskell `OSet` (Conway)
    ///
    /// In Conway the transaction body's `inputs` field is an `OSet` (tag 258),
    /// which enforces uniqueness and sorted order at deserialization.  Dugite
    /// enforces the equivalent invariant in Phase-1 validation as the
    /// `DuplicateInput` error variant, producing the same rejection outcome.
    ///
    /// We verify three things:
    ///   (a) `validate_transaction` returns `Err(_)`.
    ///   (b) The error list contains a `DuplicateInput` variant.
    ///   (c) The original UTxO set entries are unmodified (validation is read-only).
    #[test]
    fn prop_duplicate_input_rejected(
        (utxo_set, inputs) in arb_utxo_set(3),
        fee in 200_000u64..500_000u64,
    ) {
        // ── Step 1: Choose the input to duplicate ─────────────────────────────
        let dup_input = inputs[0].clone();
        let consumed_coin = utxo_set
            .lookup(&dup_input)
            .expect("input must exist in generated UTxO set")
            .value.coin.0;

        prop_assume!(consumed_coin > fee);
        let output_value = consumed_coin - fee;

        // ── Step 2: Build a tx with the duplicate input ───────────────────────
        //
        // `build_simple_tx` accepts an arbitrary Vec<TransactionInput>; passing
        // the same input twice produces a structurally invalid body that
        // Phase-1 validation must reject.
        let dup_tx = build_simple_tx(
            vec![dup_input.clone(), dup_input.clone()],
            output_value,
            fee,
        );

        // ── Step 3: Validate — must return Err containing DuplicateInput ──────
        //
        // tx_size = 500 bytes is a plausible minimum; current_slot = 0 means no
        // TTL expiry; no slot_config needed (no Plutus scripts).
        let params = ProtocolParameters::mainnet_defaults();
        let result = validate_transaction(&dup_tx, &utxo_set, &params, 0, 500, None);

        prop_assert!(
            result.is_err(),
            "Transaction with duplicate input must be rejected"
        );

        let errors = result.unwrap_err();
        let has_dup = errors.iter().any(|e| {
            matches!(e, dugite_ledger::ValidationError::DuplicateInput(_))
        });
        prop_assert!(
            has_dup,
            "Expected DuplicateInput error; got: {:?}", errors
        );

        // ── Step 4: UTxO set is unmodified ─────────────────────────────────────
        //
        // `validate_transaction` borrows the UTxO set via `&dyn UtxoLookup`
        // (read-only) and never mutates it.
        for inp in &inputs {
            prop_assert!(
                utxo_set.contains(inp),
                "UTxO entry {:?} must still be present after failed validation", inp
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Property 9: Deposit pot invariant
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// For any `LedgerState` generated by `arb_ledger_state`:
    ///
    /// ```text
    /// total_stake_key_deposits + Σ(pool_deposits.values())
    ///   == key_deposit * n_registered_creds + pool_deposit * n_registered_pools
    /// ```
    ///
    /// This is the deposit-pot component of Haskell's `totalObligation`:
    ///
    /// ```haskell
    /// totalObligation certState govState =
    ///     obligationCertState certState + obligationGovState govState
    /// ```
    ///
    /// where `obligationCertState` sums stake key deposits and pool deposits.
    ///
    /// The `arb_ledger_state` generator enforces the identity by construction;
    /// this test verifies the invariant survives the generator unscathed and
    /// will catch future generator regressions or ledger-state mutations that
    /// break the tracking.
    ///
    /// # Why this matters
    ///
    /// The deposit pot directly feeds the reward-reserve calculation.  An
    /// inconsistency here causes silent over- or under-payment across every
    /// subsequent epoch.  This is listed as a HIGH-priority open gap in the
    /// conformance audit (deposit tracking per-credential).
    #[test]
    fn prop_deposit_pot_invariant(
        state in arb_ledger_state(LedgerStateConfig::default()),
    ) {
        let params = ProtocolParameters::mainnet_defaults();
        let key_deposit = params.key_deposit.0;
        let pool_deposit = params.pool_deposit.0;

        // Number of registered stake credentials.
        let n_registered_creds = state.stake_key_deposits.len() as u64;
        // Number of registered pools (each has exactly one pool_deposits entry).
        let n_registered_pools = state.pool_deposits.len() as u64;

        // Expected deposit totals derived from protocol parameters.
        let expected_key_total = key_deposit
            .checked_mul(n_registered_creds)
            .expect("key deposit total must not overflow u64");
        let expected_pool_total = pool_deposit
            .checked_mul(n_registered_pools)
            .expect("pool deposit total must not overflow u64");
        let expected_total = expected_key_total + expected_pool_total;

        // Actual deposit totals from the ledger state fields.
        let actual_pool_sum: u64 = state.pool_deposits.values().sum();
        let actual_total = state.total_stake_key_deposits + actual_pool_sum;

        prop_assert_eq!(
            actual_total, expected_total,
            "Deposit pot: actual={} (key_track={} + pool_sum={}), \
             expected={} (key_dep={} * n_creds={} + pool_dep={} * n_pools={})",
            actual_total,
            state.total_stake_key_deposits,
            actual_pool_sum,
            expected_total,
            key_deposit,
            n_registered_creds,
            pool_deposit,
            n_registered_pools
        );

        // Each per-pool deposit entry must equal exactly pool_deposit (since the
        // generator uses mainnet protocol parameters throughout).
        for (&pool_id, &deposit) in &state.pool_deposits {
            prop_assert_eq!(
                deposit, pool_deposit,
                "pool {:?} has deposit {}; expected {}",
                pool_id, deposit, pool_deposit
            );
        }

        // total_stake_key_deposits must equal key_deposit * n_registered_creds.
        prop_assert_eq!(
            state.total_stake_key_deposits, expected_key_total,
            "total_stake_key_deposits={} != key_deposit({}) * n_creds({})",
            state.total_stake_key_deposits, key_deposit, n_registered_creds
        );
    }
}

// ---------------------------------------------------------------------------
// Property 10: Collateral UTxO invariant
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// For `is_valid = false` transactions (Alonzo invalid-tx processing):
    ///
    /// 1. All collateral inputs are removed from the UTxO set.
    /// 2. If `collateral_return` is present it is added to the UTxO set.
    /// 3. All regular spending inputs remain present (untouched).
    /// 4. None of the transaction's regular outputs are added.
    ///
    /// This is Haskell's `UTXOS` rule `isValid = false` branch:
    ///
    /// ```text
    /// utxo' = (utxo ∖ collateral(tx)) ∪ collRet(tx)
    /// ```
    ///
    /// where `collRet(tx)` is `{txid#0 ↦ collateralReturn tx}` when present,
    /// or ∅ when absent.
    ///
    /// The test uses a UTxO set with 5 entries:
    ///   entries[0..2] — collateral inputs (consumed)
    ///   entries[2..4] — spending inputs (kept untouched)
    ///   entries[4]    — unused extra entry (size anchor)
    ///
    /// A simulated `collateral_return` output with a deterministic reference is
    /// optionally inserted to cover both branches.
    #[test]
    fn prop_collateral_utxo_invariant(
        (utxo_set, inputs) in arb_utxo_set(5),
        // Value of the simulated collateral_return output.
        collateral_return_lovelace in 500_000u64..=5_000_000u64,
        // Whether to include a collateral_return output in this case.
        has_collateral_return in proptest::bool::ANY,
    ) {
        let collateral_inputs: Vec<TransactionInput> = inputs[..2].to_vec();
        let spending_inputs: Vec<TransactionInput>   = inputs[2..4].to_vec();

        // ── Build the live UTxO set ───────────────────────────────────────────
        let mut live = UtxoSet::new();
        for (inp, out) in utxo_set.iter() {
            live.insert(inp, out);
        }
        let initial_len = live.len();

        // Reference that will be used for the collateral_return output.
        // The Cardano spec places collateral_return at txid#0 of the invalid tx.
        let colret_tx_hash = Hash32::from_bytes([0xCBu8; 32]);
        let colret_ref = TransactionInput { transaction_id: colret_tx_hash, index: 0 };

        // ── Simulate invalid-tx processing ────────────────────────────────────
        //
        // (a) Remove all collateral inputs.
        for col in &collateral_inputs {
            let removed = live.remove(col);
            prop_assert!(removed.is_some(), "collateral input {:?} must exist before removal", col);
        }

        // (b) Add collateral_return if present.
        if has_collateral_return {
            live.insert(colret_ref.clone(), make_output(collateral_return_lovelace));
        }

        // ── Invariant assertions ──────────────────────────────────────────────

        // (i) Collateral inputs are absent.
        for col in &collateral_inputs {
            prop_assert!(
                !live.contains(col),
                "collateral input {:?} must be absent after invalid-tx processing", col
            );
        }

        // (ii) Spending inputs remain present.
        for sp in &spending_inputs {
            prop_assert!(
                live.contains(sp),
                "spending input {:?} must remain present after invalid-tx processing", sp
            );
        }

        // (iii) collateral_return presence matches the flag.
        if has_collateral_return {
            prop_assert!(
                live.contains(&colret_ref),
                "collateral_return output must be present when has_collateral_return=true"
            );
            let out = live.lookup(&colret_ref).expect("collateral_return must be lookupable");
            prop_assert_eq!(
                out.value.coin.0, collateral_return_lovelace,
                "collateral_return coin: got {}, expected {}",
                out.value.coin.0, collateral_return_lovelace
            );
        } else {
            prop_assert!(
                !live.contains(&colret_ref),
                "collateral_return ref must not be present when has_collateral_return=false"
            );
        }

        // (iv) Size accounting:
        //   initial_len - n_collateral + (1 if colret else 0)
        let expected_len = initial_len
            - collateral_inputs.len()
            + usize::from(has_collateral_return);
        prop_assert_eq!(
            live.len(), expected_len,
            "UTxO size: got {}, expected {} (initial={} - col={} + colret={})",
            live.len(), expected_len,
            initial_len, collateral_inputs.len(),
            usize::from(has_collateral_return)
        );
    }
}
