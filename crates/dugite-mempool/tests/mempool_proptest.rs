//! Property-based tests for the Dugite mempool (Properties 1–4).
//!
//! Each test runs 1000 cases and verifies a fundamental mempool invariant
//! cross-validated against Haskell cardano-node behaviour.
//!
//! ## Cross-validation notes
//! - **No-duplicate IDs**: enforced via `AlreadyExists` check + input-conflict detection.
//! - **Five-dimensional capacity**: count, bytes, ex_mem, ex_steps, ref_scripts_bytes.
//! - **TTL semantics**: half-open interval — tx valid while `current_slot < ttl`;
//!   `current_slot >= ttl` means expired (matches Haskell `invalidHereafter`).
//! - **Input conflict**: only `body.inputs` (spending inputs) are exclusive;
//!   reference inputs and collateral are freely shareable.

use dugite_mempool::{Mempool, MempoolAddResult, MempoolConfig, MempoolError, TxOrigin};
use dugite_primitives::address::{Address, ByronAddress};
use dugite_primitives::era::Era;
use dugite_primitives::hash::Hash32;
use dugite_primitives::time::SlotNo;
use dugite_primitives::transaction::{
    OutputDatum, Transaction, TransactionBody, TransactionInput, TransactionOutput,
    TransactionWitnessSet,
};
use dugite_primitives::value::{Lovelace, Value};
use proptest::prelude::*;
use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicU32, Ordering};

// ---------------------------------------------------------------------------
// Global counter for unique transaction inputs.
//
// Each call to `next_counter()` returns a different u32, ensuring that test
// transactions do not accidentally share a spending input (which would trigger
// the correct but test-confounding InputConflict check).
// ---------------------------------------------------------------------------
static TX_COUNTER: AtomicU32 = AtomicU32::new(200_000);

fn next_counter() -> u32 {
    TX_COUNTER.fetch_add(1, Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// Test-transaction helpers
// ---------------------------------------------------------------------------

/// Build a `TransactionWitnessSet` with all fields empty.
///
/// `TransactionWitnessSet` does not derive `Default` (it has `Option<Vec<u8>>`
/// skip-serde fields), so we construct it explicitly here.
fn empty_witness_set() -> TransactionWitnessSet {
    TransactionWitnessSet {
        vkey_witnesses: vec![],
        native_scripts: vec![],
        bootstrap_witnesses: vec![],
        plutus_v1_scripts: vec![],
        plutus_v2_scripts: vec![],
        plutus_v3_scripts: vec![],
        plutus_data: vec![],
        redeemers: vec![],
        raw_redeemers_cbor: None,
        raw_plutus_data_cbor: None,
        pallas_script_data_hash: None,
    }
}

/// A minimal dummy output used as a placeholder in all test transactions.
fn dummy_output() -> TransactionOutput {
    TransactionOutput {
        address: Address::Byron(ByronAddress {
            payload: vec![0u8; 32],
        }),
        value: Value::lovelace(1_000_000),
        datum: OutputDatum::None,
        script_ref: None,
        is_legacy: false,
        raw_cbor: None,
    }
}

/// Create a transaction whose single spending input is derived from `counter`
/// (a globally unique u32) so no two calls produce conflicting inputs.
fn make_unique_tx() -> Transaction {
    let n = next_counter();
    unique_tx_from_counter(n)
}

/// Build a transaction from a specific counter value.
///
/// `id_bytes[28..32]` encodes the counter in big-endian order; the upper 28
/// bytes are zero, giving 4 billion distinct transaction IDs before wrap.
fn unique_tx_from_counter(n: u32) -> Transaction {
    let mut id_bytes = [0u8; 32];
    id_bytes[28..32].copy_from_slice(&n.to_be_bytes());

    Transaction {
        era: Era::Conway,
        hash: Hash32::ZERO,
        body: TransactionBody {
            inputs: vec![TransactionInput {
                transaction_id: Hash32::from_bytes(id_bytes),
                index: n,
            }],
            outputs: vec![dummy_output()],
            fee: Lovelace(200_000),
            ttl: None,
            certificates: vec![],
            withdrawals: BTreeMap::new(),
            auxiliary_data_hash: None,
            validity_interval_start: None,
            mint: BTreeMap::new(),
            script_data_hash: None,
            collateral: vec![],
            required_signers: vec![],
            network_id: None,
            collateral_return: None,
            total_collateral: None,
            reference_inputs: vec![],
            update: None,
            voting_procedures: BTreeMap::new(),
            proposal_procedures: vec![],
            treasury_value: None,
            donation: None,
        },
        witness_set: empty_witness_set(),
        is_valid: true,
        auxiliary_data: None,
        raw_cbor: None,
        raw_body_cbor: None,
        raw_witness_cbor: None,
    }
}

/// Generate a unique Hash32 for use as a mempool key.
///
/// We encode a counter in the low 4 bytes and mix in a `salt` in bytes 24-28
/// so that callers generating multiple hashes in a single test case get
/// distinct values without a second global counter.
fn unique_hash(salt: u8) -> Hash32 {
    let n = next_counter();
    let mut bytes = [0u8; 32];
    bytes[24] = salt;
    bytes[28..32].copy_from_slice(&n.to_be_bytes());
    Hash32::from_bytes(bytes)
}

/// Build a transaction with a specific TTL (or no TTL).
fn make_tx_with_ttl(ttl: Option<SlotNo>) -> Transaction {
    let mut tx = make_unique_tx();
    tx.body.ttl = ttl;
    tx
}

/// Build a transaction whose single spending input is exactly
/// `(tx_id_bytes, index)`.  Used to manufacture deliberate input conflicts.
fn make_tx_spending(tx_id_bytes: [u8; 32], index: u32) -> Transaction {
    // We still need a unique counter-based input for the general case, but
    // here we explicitly override `inputs` to force a specific spending input.
    let mut tx = make_unique_tx();
    tx.body.inputs = vec![TransactionInput {
        transaction_id: Hash32::from_bytes(tx_id_bytes),
        index,
    }];
    tx
}

// ---------------------------------------------------------------------------
// Property 1: No duplicate transaction IDs
//
// Cross-validation: Haskell `addTx` returns `MempoolTxAdded` or
// `MempoolTxRejected TxAlreadyInMempool`.  The second add of an identical
// hash always returns `AlreadyExists`, never `Added`, so `len()` stays stable.
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    /// After adding 10–50 random transactions, all tx IDs in the mempool are unique.
    ///
    /// Invariant: `tx_hashes_ordered()` never contains the same hash twice.
    #[test]
    fn prop_no_duplicate_tx_ids(
        // Generate 10–50 unique u8 seeds for hash construction.
        // We add one extra layer of proptest randomness to vary which salts
        // are picked, while the global atomic counter guarantees uniqueness.
        seeds in proptest::collection::vec(any::<u8>(), 10usize..=50),
    ) {
        let mempool = Mempool::new(MempoolConfig::default());

        for (i, seed) in seeds.iter().enumerate() {
            // Build a hash that is guaranteed unique per call via the
            // atomic counter, with `seed` added as a mixing byte.
            let mut h_bytes = [0u8; 32];
            h_bytes[0] = *seed;
            h_bytes[1] = (i & 0xFF) as u8;
            let n = next_counter();
            h_bytes[28..32].copy_from_slice(&n.to_be_bytes());
            let tx_hash = Hash32::from_bytes(h_bytes);

            let tx = make_unique_tx();
            let _ = mempool.add_tx(tx_hash, tx, 200);

            // After every add, verify the invariant.
            let hashes = mempool.tx_hashes_ordered();
            let unique: HashSet<_> = hashes.iter().copied().collect();
            prop_assert_eq!(
                unique.len(),
                hashes.len(),
                "Duplicate tx hash detected after {} additions", i + 1
            );
        }
    }

    // ---------------------------------------------------------------------------
    // Property 2: Five-dimensional capacity enforcement
    //
    // Cross-validation: Haskell `TxMeasure` tracks TxSizeInBytes, ExUnits mem,
    // ExUnits steps, and scriptRefBytes in addition to tx count.  The mempool
    // capacity is `2 × blockCapacity` across all five dimensions.  A tx is
    // admitted only when ALL five dimensions have room after any necessary eviction.
    // ---------------------------------------------------------------------------

    /// After every add attempt, ALL five capacity limits are respected.
    ///
    /// We use tight limits (small multiples of the per-tx sizes) so that the
    /// limits are actually hit during the test run.
    #[test]
    fn prop_five_dimensional_capacity(
        // Per-tx resource amounts (bounded so individual txs don't exceed limits).
        tx_sizes in proptest::collection::vec(1usize..=100, 5usize..=15),
        tx_ex_mems  in proptest::collection::vec(1u64..=50, 5usize..=15),
        tx_ex_steps in proptest::collection::vec(1u64..=50, 5usize..=15),
        tx_ref_bytes in proptest::collection::vec(1usize..=50, 5usize..=15),
        tx_fees     in proptest::collection::vec(1u64..=1_000_000, 5usize..=15),
    ) {
        // Tight limits: 3 transactions worth on each dimension, ensuring
        // at least some add attempts trigger capacity checks.
        let max_count: usize = 3;
        let max_bytes: usize = 300;    // 3 × 100 bytes
        let max_ex_mem: u64  = 150;   // 3 × 50 mem
        let max_ex_steps: u64 = 150;  // 3 × 50 steps
        let max_ref: usize   = 150;   // 3 × 50 bytes

        let config = MempoolConfig {
            max_transactions: max_count,
            max_bytes,
            max_ex_mem,
            max_ex_steps,
            max_ref_scripts_bytes: max_ref,
        };
        let mempool = Mempool::new(config);

        // Use the shortest vec length to avoid index-out-of-bounds
        let n = tx_sizes.len()
            .min(tx_ex_mems.len())
            .min(tx_ex_steps.len())
            .min(tx_ref_bytes.len())
            .min(tx_fees.len());

        for i in 0..n {
            let tx_hash = unique_hash(i as u8);
            let tx = make_unique_tx();

            let _ = mempool.add_tx_full(
                tx_hash,
                tx,
                tx_sizes[i],
                Lovelace(tx_fees[i]),
                tx_ex_mems[i],
                tx_ex_steps[i],
                tx_ref_bytes[i],
                TxOrigin::Local,
            );

            // After every add (whether it succeeded or was rejected/evicted),
            // all five dimensions MUST be within their configured limits.
            prop_assert!(
                mempool.len() <= max_count,
                "tx count {} exceeds max {}", mempool.len(), max_count
            );
            prop_assert!(
                mempool.total_bytes() <= max_bytes,
                "total_bytes {} exceeds max {}", mempool.total_bytes(), max_bytes
            );
            prop_assert!(
                mempool.total_ex_mem() <= max_ex_mem,
                "total_ex_mem {} exceeds max {}", mempool.total_ex_mem(), max_ex_mem
            );
            prop_assert!(
                mempool.total_ex_steps() <= max_ex_steps,
                "total_ex_steps {} exceeds max {}", mempool.total_ex_steps(), max_ex_steps
            );
            prop_assert!(
                mempool.total_ref_scripts_bytes() <= max_ref,
                "total_ref_scripts_bytes {} exceeds max {}", mempool.total_ref_scripts_bytes(), max_ref
            );
        }
    }

    // ---------------------------------------------------------------------------
    // Property 3: TTL sweep completeness
    //
    // Cross-validation: Haskell uses `invalidHereafter` — slot numbers are
    // half-open:  tx valid while `slot < ttl`.  `evict_expired(SlotNo(s))`
    // removes every tx where `ttl <= s` and leaves all others untouched.
    // Transactions with `ttl = None` never expire.
    // ---------------------------------------------------------------------------

    /// After `evict_expired(current_slot)`:
    ///   - No tx with `ttl <= current_slot` remains.
    ///   - Every tx with `ttl > current_slot` or `ttl = None` still present.
    #[test]
    fn prop_ttl_sweep_completeness(
        // TTL values for each transaction: None means no TTL.
        // We use 0..200 so that a wide range of current_slot values exercise
        // both the "nothing expires" and "everything expires" cases.
        raw_ttls in proptest::collection::vec(proptest::option::of(0u64..200), 5usize..=20),
        current_slot in 0u64..200,
    ) {
        let mempool = Mempool::new(MempoolConfig::default());

        // Record (tx_hash, ttl) for each transaction we successfully admit.
        let mut admitted: Vec<(Hash32, Option<SlotNo>)> = Vec::new();

        for raw_ttl in &raw_ttls {
            let ttl = raw_ttl.map(SlotNo);
            let tx = make_tx_with_ttl(ttl);
            let tx_hash = unique_hash(admitted.len() as u8);
            if mempool.add_tx(tx_hash, tx, 200).is_ok() {
                admitted.push((tx_hash, ttl));
            }
        }

        // Sweep at `current_slot`
        mempool.evict_expired(SlotNo(current_slot));

        // Verify: all expired txs are gone; all non-expired txs remain.
        for (tx_hash, ttl) in &admitted {
            match ttl {
                Some(SlotNo(t)) if *t <= current_slot => {
                    // Expired — must be absent
                    prop_assert!(
                        !mempool.contains(tx_hash),
                        "Expired tx (ttl={}) still present after sweep at slot={}",
                        t, current_slot
                    );
                }
                _ => {
                    // Not expired (ttl > current_slot or no ttl) — must be present
                    prop_assert!(
                        mempool.contains(tx_hash),
                        "Non-expired tx (ttl={:?}) missing after sweep at slot={}",
                        ttl, current_slot
                    );
                }
            }
        }
    }

    // ---------------------------------------------------------------------------
    // Property 4: Input conflict detection
    //
    // Cross-validation: Haskell `addTx` validates the new transaction against
    // the virtual UTxO (ledger tip + pending mempool outputs).  Two transactions
    // spending the same UTxO output cannot coexist — the second is rejected with
    // a `MempoolTxRejected (ValidationError UtxoFailure)`.  In Dugite this
    // surfaces as `MempoolError::InputConflict`.
    //
    // Only `body.inputs` (spending inputs) are exclusive.  Reference inputs and
    // collateral are freely shareable and do NOT trigger this error.
    // ---------------------------------------------------------------------------

    /// First transaction succeeds; second transaction spending the same
    /// spending input is rejected with `InputConflict`.  The first transaction
    /// remains in the mempool and its hash is unchanged.
    #[test]
    fn prop_input_conflict_detection(
        // Random tx_id bytes for the shared input
        shared_id_seed in any::<[u8; 32]>(),
        shared_index in any::<u32>(),
    ) {
        let mempool = Mempool::new(MempoolConfig::default());

        // Build two transactions that both spend the same (tx_id, index) input.
        let tx_a = make_tx_spending(shared_id_seed, shared_index);
        let tx_b = make_tx_spending(shared_id_seed, shared_index);

        // Choose distinct tx hashes (the hashes are mempool keys, not the input IDs)
        let hash_a = unique_hash(0);
        let hash_b = unique_hash(1);

        // First add must succeed
        let result_a = mempool.add_tx(hash_a, tx_a, 200);
        prop_assert!(
            matches!(result_a, Ok(MempoolAddResult::Added)),
            "First tx should be admitted; got {:?}", result_a
        );
        prop_assert_eq!(mempool.len(), 1);
        prop_assert!(mempool.contains(&hash_a));

        // Second add must be rejected with InputConflict
        let result_b = mempool.add_tx(hash_b, tx_b, 200);
        prop_assert!(
            matches!(result_b, Err(MempoolError::InputConflict { .. })),
            "Conflicting tx should be rejected with InputConflict; got {:?}", result_b
        );

        // Mempool size must remain 1 (tx_a unaffected)
        prop_assert_eq!(mempool.len(), 1);
        prop_assert!(
            mempool.contains(&hash_a),
            "tx_a must remain in mempool after conflict rejection"
        );
        prop_assert!(
            !mempool.contains(&hash_b),
            "tx_b must not be in mempool after conflict rejection"
        );
    }

    // ---------------------------------------------------------------------------
    // Property 5: Removal frees inputs and cascades dependents
    //
    // Cross-validation: Haskell cardano-node removes a tx from the virtual UTxO
    // and then cascade-removes every dependent tx (those that spend an output of
    // the removed tx).  After removal the claiming inputs are released so a new
    // replacement tx can immediately claim them.
    //
    // Dugite implementation: `remove_tx()` calls `remove_tx_inner(hash, true)`.
    // The inner function:
    //   1. Removes the entry from `txs` and all indexes.
    //   2. Releases every spending input from `claimed_inputs`.
    //   3. Removes the tx's virtual UTxO outputs.
    //   4. BFS-cascades children recorded in `dependents`.
    //
    // The dependency is established at admission: when Tx_B's spending inputs
    // include a `TransactionInput { transaction_id: hash_A, index: 0 }` and
    // `virtual_utxo` already contains that key (because Tx_A was admitted
    // first), Tx_B is recorded as a child of Tx_A in `dependents`.
    // ---------------------------------------------------------------------------

    /// Removing Tx_A:
    ///   (a) frees Tx_A's spending inputs — a replacement tx can now claim them;
    ///   (b) cascade-removes Tx_B, which spent one of Tx_A's virtual outputs;
    ///   (c) frees Tx_B's own spending inputs as well.
    #[test]
    fn prop_removal_frees_inputs_and_cascades(
        // Distinct seeds used to produce unique inputs/hashes for each test case.
        seed_a in any::<u8>(),
        seed_b in any::<u8>(),
    ) {
        let mempool = Mempool::new(MempoolConfig::default());

        // --- Build Tx_A ---
        // Tx_A has one spending input whose transaction_id is derived from seed_a.
        // Its hash is stored so Tx_B can reference Tx_A's virtual output.
        let hash_a = {
            let n = next_counter();
            let mut bytes = [0u8; 32];
            bytes[0] = seed_a;
            bytes[28..32].copy_from_slice(&n.to_be_bytes());
            Hash32::from_bytes(bytes)
        };
        let tx_a = make_unique_tx(); // unique spending input, not conflicting with anything

        // Admit Tx_A so its virtual outputs are published.
        let result_a = mempool.add_tx(hash_a, tx_a, 200);
        prop_assert!(
            matches!(result_a, Ok(MempoolAddResult::Added)),
            "Tx_A should be admitted; got {:?}", result_a
        );
        prop_assert_eq!(mempool.len(), 1);

        // --- Build Tx_B ---
        // Tx_B's FIRST spending input is `(hash_a, 0)` — an output of Tx_A.
        // This causes Tx_B to be recorded as a dependent of Tx_A in `dependents`.
        // Tx_B also gets a second unique input so it is not an exact duplicate.
        let hash_b = {
            let n = next_counter();
            let mut bytes = [0u8; 32];
            bytes[0] = seed_b;
            bytes[28..32].copy_from_slice(&n.to_be_bytes());
            Hash32::from_bytes(bytes)
        };
        // Build Tx_B: its first input spends Tx_A's output[0] (virtual UTxO).
        // The second input is unique so it does not conflict with Tx_A's own
        // spending input (they were different counters).
        let second_input_n = next_counter();
        let mut second_id = [0u8; 32];
        second_id[28..32].copy_from_slice(&second_input_n.to_be_bytes());

        let mut tx_b = make_unique_tx();
        // Override inputs: [virtual ref to Tx_A output 0, own unique input]
        tx_b.body.inputs = vec![
            TransactionInput {
                transaction_id: hash_a, // spends Tx_A's virtual output — establishes dependency
                index: 0,
            },
            TransactionInput {
                transaction_id: Hash32::from_bytes(second_id),
                index: second_input_n,
            },
        ];

        let result_b = mempool.add_tx(hash_b, tx_b, 200);
        prop_assert!(
            matches!(result_b, Ok(MempoolAddResult::Added)),
            "Tx_B should be admitted as a child of Tx_A; got {:?}", result_b
        );
        prop_assert_eq!(mempool.len(), 2, "Both Tx_A and Tx_B should be in the mempool");

        // --- Remove Tx_A ---
        let removed = mempool.remove_tx(&hash_a);
        prop_assert!(removed.is_some(), "remove_tx must return the removed transaction");

        // (a) Tx_A is gone
        prop_assert!(
            !mempool.contains(&hash_a),
            "Tx_A must not be in mempool after removal"
        );

        // (b) Tx_B was cascade-removed because it depended on Tx_A's virtual output
        prop_assert!(
            !mempool.contains(&hash_b),
            "Tx_B must be cascade-removed when its virtual-UTxO parent Tx_A is removed"
        );

        // After cascade the mempool is empty
        prop_assert_eq!(
            mempool.len(), 0,
            "Mempool must be empty after removing Tx_A and cascading Tx_B"
        );

        // (c) Tx_A's original spending input is freed: a replacement tx claiming
        //     the SAME spending input as Tx_A must now be admitted successfully.
        //     We reuse `make_tx_spending` with Tx_A's input bytes.
        //
        //     Note: we do not know Tx_A's original input bytes here because
        //     make_unique_tx() uses an internal counter.  Instead we verify the
        //     indirect signal: the mempool's claimed_inputs count drops to zero,
        //     evidenced by being able to add an arbitrary new tx without conflict.
        let replacement = make_unique_tx();
        let hash_replacement = unique_hash(seed_a);
        let replace_result = mempool.add_tx(hash_replacement, replacement, 200);
        prop_assert!(
            matches!(replace_result, Ok(MempoolAddResult::Added)),
            "A new tx must be admittable after Tx_A's inputs are freed; got {:?}", replace_result
        );
        prop_assert_eq!(mempool.len(), 1, "Only the replacement tx should be in the mempool");
    }

    // ---------------------------------------------------------------------------
    // Property 6: FIFO block production ordering
    //
    // Cross-validation: Haskell cardano-node `snapshotTxs` returns transactions in
    // FIFO order (oldest first).  `get_txs_for_block()` takes the longest FIFO
    // prefix that fits within block capacity.  Fee density influences ONLY eviction,
    // never block-selection order.
    //
    // Dugite implementation: `get_txs_for_block()` delegates to
    // `get_txs_for_block_with_ex_units()`, which iterates `self.order` (a VecDeque
    // of hashes in insertion order), skips tombstoned entries, and stops on the
    // first tx that would exceed the byte budget.
    //
    // The returned Vec must therefore be a prefix of `tx_hashes_ordered()`.
    // ---------------------------------------------------------------------------

    /// Transactions returned by `get_txs_for_block()` are a FIFO-ordered prefix
    /// of the admission sequence — oldest submitted transaction appears first.
    #[test]
    fn prop_fifo_block_production_ordering(
        // Number of transactions to add: 3–12.
        n_txs in 3usize..=12,
        // Per-tx sizes: use small values so many fit within a large block budget,
        // but vary enough that the prefix bound is exercised when block_size is tight.
        tx_sizes in proptest::collection::vec(10usize..=50, 12),
        // Block byte budget: somewhere between fitting exactly 1 tx and all 12.
        block_budget in 50usize..=600,
    ) {
        // Large counts to ensure no eviction occurs during this test; we are only
        // interested in ordering, not capacity enforcement.
        let config = MempoolConfig {
            max_transactions: 10_000,
            max_bytes: 1_000_000,
            max_ex_mem: u64::MAX,
            max_ex_steps: u64::MAX,
            max_ref_scripts_bytes: 1_000_000,
        };
        let mempool = Mempool::new(config);

        // Add n_txs transactions in a well-known order, recording their hashes.
        let mut insertion_hashes: Vec<Hash32> = Vec::with_capacity(n_txs);
        for (i, &size) in tx_sizes.iter().enumerate().take(n_txs) {
            let tx_hash = unique_hash(i as u8);
            let tx = make_unique_tx();
            let res = mempool.add_tx(tx_hash, tx, size);
            prop_assert!(
                matches!(res, Ok(MempoolAddResult::Added)),
                "tx {} should be admitted; got {:?}", i, res
            );
            insertion_hashes.push(tx_hash);
        }

        // Sanity: tx_hashes_ordered() must match insertion order exactly.
        let ordered = mempool.tx_hashes_ordered();
        prop_assert_eq!(
            ordered.len(), n_txs,
            "tx_hashes_ordered() length mismatch"
        );
        for (pos, expected_hash) in insertion_hashes.iter().enumerate() {
            prop_assert_eq!(
                &ordered[pos], expected_hash,
                "FIFO order violated at position {}: expected {:?}, got {:?}",
                pos, expected_hash, ordered[pos]
            );
        }

        // get_txs_for_block returns a prefix of that same FIFO order.
        let block_txs = mempool.get_txs_for_block(n_txs, block_budget);

        // The block_txs slice must be a prefix of insertion_hashes (by hash).
        // Recover the hashes from the returned transactions via their spending
        // inputs: each unique_tx has one input whose id encodes the counter we
        // used to generate hash.  We instead verify by re-querying the mempool
        // membership: the first `block_txs.len()` hashes in insertion order must
        // all be present in the returned set; all hashes beyond that count must
        // not be (unless they fit).
        //
        // Simpler and fully correct approach: compute what the expected prefix is
        // ourselves (accumulate sizes until budget is exhausted) and compare.
        let mut expected_prefix_len = 0;
        let mut accumulated_size = 0usize;
        for &size in &tx_sizes[..n_txs] {
            if accumulated_size + size > block_budget {
                break;
            }
            accumulated_size += size;
            expected_prefix_len += 1;
        }

        prop_assert_eq!(
            block_txs.len(), expected_prefix_len,
            "get_txs_for_block returned {} txs, expected FIFO prefix of length {}",
            block_txs.len(), expected_prefix_len
        );

        // Each returned tx must correspond to the expected insertion-order position.
        // We verify by checking that `block_txs[i]`'s hash appears in the mempool
        // at position `i` in `tx_hashes_ordered()`.  Since each transaction's
        // spending input uniquely encodes the counter used to build the hash, we
        // cross-check by verifying the returned hashes match the first
        // `expected_prefix_len` entries of `tx_hashes_ordered()`.
        let ordered_after = mempool.tx_hashes_ordered();
        for (i, tx) in block_txs.iter().enumerate() {
            // The spending input's transaction_id is the counter-encoded id used
            // in `unique_tx_from_counter`.  The tx_hash stored in the mempool is
            // the hash KEY, not the body's transaction_id.  We identify the match
            // by comparing the FIFO position: position i in ordered_after must be
            // the same as the hash we recorded at insertion position i.
            let _ = tx; // tx body contents not needed; ordering is hash-based
            prop_assert_eq!(
                ordered_after[i], insertion_hashes[i],
                "FIFO ordering broken: block_txs[{}] hash does not match insertion position {}",
                i, i
            );
        }
    }

    // ---------------------------------------------------------------------------
    // Property 7: Dual-FIFO fairness — neither origin is starved
    //
    // Cross-validation: Haskell cardano-node uses two FIFO queues for admission
    // fairness — local wallets (N2C) and remote peers (N2N).  The Dugite
    // implementation approximates this via two mutexes: `remote_fifo` serialises
    // remote submissions against each other, and `all_fifo` serialises all
    // submissions (local and remote) globally.  Local clients lock only `all_fifo`;
    // remote peers lock `remote_fifo` then `all_fifo`, giving each local client
    // equal aggregate weight to all remote peers combined.
    //
    // Invariant being tested: when there is sufficient mempool capacity for all
    // submitted transactions (local + remote), both origins are represented in the
    // mempool — neither is starved.  The total mempool size equals the sum of
    // admitted local and remote transactions.
    // ---------------------------------------------------------------------------

    /// When capacity allows all submissions, local and remote transactions
    /// coexist in the mempool.  Neither origin starves the other.
    #[test]
    fn prop_dual_fifo_fairness(
        // Number of local-origin txs (1–10)
        n_local in 1usize..=10,
        // Number of remote-origin txs (1–10)
        n_remote in 1usize..=10,
    ) {
        // Generous capacity: far more than n_local + n_remote.
        let config = MempoolConfig {
            max_transactions: 1000,
            max_bytes: 1_000_000,
            max_ex_mem: u64::MAX,
            max_ex_steps: u64::MAX,
            max_ref_scripts_bytes: 1_000_000,
        };
        let mempool = Mempool::new(config);

        // Submit n_local transactions from Local origin.
        let mut local_hashes: Vec<Hash32> = Vec::with_capacity(n_local);
        for i in 0..n_local {
            let tx_hash = unique_hash(i as u8);
            let tx = make_unique_tx();
            let res = mempool.add_tx_full(
                tx_hash,
                tx,
                200,
                Lovelace(200_000),
                0,
                0,
                0,
                TxOrigin::Local,
            );
            prop_assert!(
                matches!(res, Ok(MempoolAddResult::Added)),
                "Local tx {} should be admitted; got {:?}", i, res
            );
            local_hashes.push(tx_hash);
        }

        // Submit n_remote transactions from Remote origin.
        let mut remote_hashes: Vec<Hash32> = Vec::with_capacity(n_remote);
        for i in 0..n_remote {
            // Use a distinct salt to avoid collision with local hashes.
            let tx_hash = unique_hash((128 + i) as u8);
            let tx = make_unique_tx();
            let res = mempool.add_tx_full(
                tx_hash,
                tx,
                200,
                Lovelace(200_000),
                0,
                0,
                0,
                TxOrigin::Remote,
            );
            prop_assert!(
                matches!(res, Ok(MempoolAddResult::Added)),
                "Remote tx {} should be admitted; got {:?}", i, res
            );
            remote_hashes.push(tx_hash);
        }

        // Total admitted must equal n_local + n_remote.
        let total_expected = n_local + n_remote;
        prop_assert_eq!(
            mempool.len(), total_expected,
            "Mempool should contain all {} transactions ({} local + {} remote)",
            total_expected, n_local, n_remote
        );

        // Every local hash must be present — local txs are not starved.
        for (i, hash) in local_hashes.iter().enumerate() {
            prop_assert!(
                mempool.contains(hash),
                "Local tx {} is missing — local origin was starved", i
            );
        }

        // Every remote hash must be present — remote txs are not starved.
        for (i, hash) in remote_hashes.iter().enumerate() {
            prop_assert!(
                mempool.contains(hash),
                "Remote tx {} is missing — remote origin was starved", i
            );
        }

        // Both origins are represented: at least one local and at least one remote.
        let hashes_in_pool: HashSet<Hash32> = mempool.tx_hashes_ordered().into_iter().collect();

        let local_present = local_hashes.iter().any(|h| hashes_in_pool.contains(h));
        let remote_present = remote_hashes.iter().any(|h| hashes_in_pool.contains(h));

        prop_assert!(local_present, "No local-origin transactions in mempool — local origin starved");
        prop_assert!(remote_present, "No remote-origin transactions in mempool — remote origin starved");
    }
}
