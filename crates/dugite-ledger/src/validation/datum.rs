//! Datum witness completeness validation (Rule 9c).
//!
//! This module implements two complementary datum-related Phase-1 rules that
//! together ensure the transaction's `plutus_data` witness set is exactly the
//! set required — no more and no fewer datums than necessary.
//!
//! ## Required datums (missing datum check)
//!
//! For every spending input whose UTxO carries a `DatumHash` (i.e. the hash
//! form of datum attachment — not an inline datum) AND whose address is
//! script-locked (payment credential is `Credential::Script`), the raw datum
//! bytes that hash to that value MUST be present in
//! `tx.witness_set.plutus_data`.
//!
//! Inputs that are NOT script-locked do not require a datum witness even if
//! their UTxO carries a `DatumHash` — only scripts need to inspect the datum.
//! Inputs with `OutputDatum::InlineDatum` are also exempt: the datum is
//! already embedded in the UTxO and does not need to be re-supplied.
//!
//! ## Extra datums (spurious datum check)
//!
//! Any datum in `tx.witness_set.plutus_data` whose blake2b-256 hash is NOT
//! in the "needed" set makes the transaction malformed.  The needed set is:
//!
//! - All `DatumHash` values from script-locked spending input UTxOs, plus
//! - All `DatumHash` values from transaction outputs (outputs that declare a
//!   datum hash are allowed to have the datum supplied by the witness set).
//!
//! The second bullet mirrors Haskell's `allowedSupplementalDatums` which
//! permits output datum hashes as an additional "allowed" set in addition to
//! the strictly required input datum hashes.
//!
//! ## Reference: Haskell ledger
//!
//! Cardano.Ledger.Alonzo.Rules.Utxow:
//! - `missingRequiredDatums` — inputs with DatumHash but no matching witness datum
//! - `notAllowedSupplementalDatums` — witness datums whose hash is not in
//!   `requiredDatums ∪ allowedSupplementalDatums`

use std::collections::HashSet;

use dugite_primitives::credentials::Credential;
use dugite_primitives::hash::{DatumHash, Hash32};
use dugite_primitives::transaction::{OutputDatum, Transaction};

use crate::utxo::UtxoLookup;

use super::ValidationError;

/// Hash a plutus datum by CBOR-encoding it and applying blake2b-256.
///
/// This mirrors Haskell's `hashData :: Data era -> DataHash (EraCrypto era)`
/// which is `blake2b_256(to_cbor(data))`.  We use our own CBOR encoder so
/// that the hash is computed from the canonical re-encoding rather than
/// from the original wire bytes.
///
/// For transactions received from the network the witness datums are always
/// parsed through pallas before being stored in `plutus_data`, so
/// re-encoding is safe.  Any encoding-detail differences (e.g.
/// indefinite-length arrays) would also exist in the on-chain UTxO datum hash
/// — but in practice datum hashes committed to UTxOs were produced by
/// cardano-cli/cardano-node which use the same canonical encoding we produce.
fn hash_plutus_datum(datum: &dugite_primitives::transaction::PlutusData) -> Hash32 {
    let cbor = dugite_serialization::encode_plutus_data(datum);
    dugite_primitives::hash::blake2b_256(&cbor)
}

/// Check datum witness completeness: Rule 9c.
///
/// Populates `errors` with:
/// - [`ValidationError::MissingDatumWitness`] for each script-locked input
///   whose UTxO carries a `DatumHash` with no matching entry in the witness
///   plutus_data.
/// - [`ValidationError::ExtraDatumWitness`] for each witness datum whose hash
///   is not in the needed set (required input datums ∪ allowed supplemental
///   output datums).
///
/// Called unconditionally from `run_phase1_rules` after input existence has
/// been confirmed (so UTxO lookups are safe).
pub(super) fn check_datum_witnesses(
    tx: &Transaction,
    utxo_set: &dyn UtxoLookup,
    errors: &mut Vec<ValidationError>,
) {
    // ------------------------------------------------------------------
    // Step 1 — Build the "needed" set of datum hashes.
    //
    // This is the union of two sub-sets:
    //   (a) required_datums: DatumHash from script-locked spending input UTxOs.
    //       These MUST have a matching witness datum.
    //   (b) allowed_supplemental: DatumHash declared on transaction outputs.
    //       Witness datums for these are allowed (but not required).
    //
    // `needed = required_datums ∪ allowed_supplemental`
    //
    // Any witness datum whose hash is outside `needed` is extraneous.
    // ------------------------------------------------------------------

    // (a) Required: datum hashes from script-locked spending input UTxOs.
    let mut required_datum_hashes: HashSet<DatumHash> = HashSet::new();

    for input in &tx.body.inputs {
        // If the UTxO is not found, Rule 2 (InputNotFound) will have already
        // fired.  We skip silently here to avoid duplicate/confusing errors.
        let Some(utxo) = utxo_set.lookup(input) else {
            continue;
        };

        // Only script-locked inputs need a datum witness.
        let is_script_locked = matches!(
            utxo.address.payment_credential(),
            Some(Credential::Script(_))
        );
        if !is_script_locked {
            continue;
        }

        // Only DatumHash outputs require a witness datum.
        // InlineDatum outputs embed the datum in the UTxO itself — no witness
        // needed.  OutputDatum::None means the script either doesn't need a
        // datum or uses the redeemer alone (e.g. minting policies executed via
        // a spending input do not require a datum).
        if let OutputDatum::DatumHash(hash) = &utxo.datum {
            required_datum_hashes.insert(*hash);
        }
    }

    // (b) Allowed supplemental: datum hashes from transaction outputs AND
    // reference inputs.  Cardano allows a transaction to supply datum
    // pre-images for:
    //   - outputs it creates (so future spenders have the datum bytes)
    //   - reference inputs it reads (the datum may be needed by scripts)
    //
    // These are optional and do not trigger MissingDatumWitness, but they
    // DO count toward the allowed set for the ExtraDatumWitness check.
    //
    // Haskell reference: `notAllowedSupplementalDatums` includes both
    // output datum hashes and reference input datum hashes in the
    // "allowed" set.
    let mut allowed_supplemental_hashes: HashSet<DatumHash> = HashSet::new();

    for output in &tx.body.outputs {
        if let OutputDatum::DatumHash(hash) = &output.datum {
            allowed_supplemental_hashes.insert(*hash);
        }
    }

    // Reference inputs: their UTxO datum hashes are also supplemental.
    for ref_input in &tx.body.reference_inputs {
        if let Some(utxo) = utxo_set.lookup(ref_input) {
            if let OutputDatum::DatumHash(hash) = &utxo.datum {
                allowed_supplemental_hashes.insert(*hash);
            }
        }
    }

    // Union: all datum hashes that are acceptable in the witness set.
    let needed: HashSet<DatumHash> = required_datum_hashes
        .iter()
        .chain(allowed_supplemental_hashes.iter())
        .copied()
        .collect();

    // ------------------------------------------------------------------
    // Step 2 — Hash each witness datum and build the "supplied" set.
    //
    // We compute the hash of every datum in plutus_data so we can:
    //   (a) check that required hashes are covered (missing datum check), and
    //   (b) check that no supplied hash is outside `needed` (extra datum check).
    // ------------------------------------------------------------------
    let supplied_hashes: HashSet<DatumHash> = tx
        .witness_set
        .plutus_data
        .iter()
        .map(hash_plutus_datum)
        .collect();

    // ------------------------------------------------------------------
    // Step 3 — Missing datum check.
    //
    // Every required datum hash MUST appear in the supplied set.
    // ------------------------------------------------------------------
    for hash in &required_datum_hashes {
        if !supplied_hashes.contains(hash) {
            errors.push(ValidationError::MissingDatumWitness(hash.to_hex()));
        }
    }

    // ------------------------------------------------------------------
    // Step 4 — Extra datum check.
    //
    // Every supplied datum hash MUST appear in the needed set.
    // ------------------------------------------------------------------
    for hash in &supplied_hashes {
        if !needed.contains(hash) {
            errors.push(ValidationError::ExtraDatumWitness(hash.to_hex()));
        }
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use dugite_primitives::address::{Address, BaseAddress};
    use dugite_primitives::credentials::Credential;
    use dugite_primitives::hash::{Hash28, Hash32};
    use dugite_primitives::network::NetworkId;
    use dugite_primitives::transaction::{
        OutputDatum, PlutusData, Transaction, TransactionInput, TransactionOutput,
    };
    use dugite_primitives::value::Value;

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    /// Build a UTxO set containing a single entry and return it along with the
    /// `TransactionInput` key that addresses that entry.
    fn make_utxo(output: TransactionOutput) -> (crate::utxo::UtxoSet, TransactionInput) {
        let mut utxo_set = crate::utxo::UtxoSet::new();
        let input = TransactionInput {
            transaction_id: Hash32::from_bytes([0xaau8; 32]),
            index: 0,
        };
        utxo_set.insert(input.clone(), output);
        (utxo_set, input)
    }

    /// Build a minimal `Transaction` whose only populated fields are those
    /// examined by `check_datum_witnesses`.
    fn make_tx(
        inputs: Vec<TransactionInput>,
        outputs: Vec<TransactionOutput>,
        reference_inputs: Vec<TransactionInput>,
        plutus_data: Vec<PlutusData>,
    ) -> Transaction {
        Transaction::empty_with_hash(Hash32::ZERO).with_parts(
            inputs,
            outputs,
            reference_inputs,
            plutus_data,
        )
    }

    // Helper method — attach fields to a `Transaction::empty_with_hash` return
    // value without requiring the full builder chain.
    trait WithParts {
        fn with_parts(
            self,
            inputs: Vec<TransactionInput>,
            outputs: Vec<TransactionOutput>,
            reference_inputs: Vec<TransactionInput>,
            plutus_data: Vec<PlutusData>,
        ) -> Self;
    }

    impl WithParts for Transaction {
        fn with_parts(
            mut self,
            inputs: Vec<TransactionInput>,
            outputs: Vec<TransactionOutput>,
            reference_inputs: Vec<TransactionInput>,
            plutus_data: Vec<PlutusData>,
        ) -> Self {
            self.body.inputs = inputs;
            self.body.outputs = outputs;
            self.body.reference_inputs = reference_inputs;
            self.witness_set.plutus_data = plutus_data;
            self
        }
    }

    /// Build a script-locked `TransactionOutput` with a `DatumHash` attachment.
    fn script_output_with_datum_hash(datum_hash: Hash32) -> TransactionOutput {
        TransactionOutput {
            address: Address::Base(BaseAddress {
                network: NetworkId::Testnet,
                payment: Credential::Script(Hash28::from_bytes([0xbbu8; 28])),
                stake: Credential::VerificationKey(Hash28::from_bytes([0xccu8; 28])),
            }),
            value: Value::lovelace(2_000_000),
            datum: OutputDatum::DatumHash(datum_hash),
            script_ref: None,
            is_legacy: false,
            raw_cbor: None,
        }
    }

    /// Build a script-locked `TransactionOutput` with an `InlineDatum`.
    fn script_output_with_inline_datum(data: PlutusData) -> TransactionOutput {
        TransactionOutput {
            address: Address::Base(BaseAddress {
                network: NetworkId::Testnet,
                payment: Credential::Script(Hash28::from_bytes([0xbbu8; 28])),
                stake: Credential::VerificationKey(Hash28::from_bytes([0xccu8; 28])),
            }),
            value: Value::lovelace(2_000_000),
            datum: OutputDatum::InlineDatum {
                data,
                raw_cbor: None,
            },
            script_ref: None,
            is_legacy: false,
            raw_cbor: None,
        }
    }

    /// Build a VKey-locked `TransactionOutput` (no datum).
    fn vkey_output_no_datum() -> TransactionOutput {
        TransactionOutput {
            address: Address::Base(BaseAddress {
                network: NetworkId::Testnet,
                payment: Credential::VerificationKey(Hash28::from_bytes([0xddu8; 28])),
                stake: Credential::VerificationKey(Hash28::from_bytes([0xeeu8; 28])),
            }),
            value: Value::lovelace(5_000_000),
            datum: OutputDatum::None,
            script_ref: None,
            is_legacy: false,
            raw_cbor: None,
        }
    }

    /// Compute the datum hash that `check_datum_witnesses` will compute for a
    /// given `PlutusData` (mirrors `hash_plutus_datum` in the production code).
    fn datum_hash_of(data: &PlutusData) -> Hash32 {
        hash_plutus_datum(data)
    }

    /// A simple, unique integer datum suitable for use in tests.
    fn int_datum(n: i128) -> PlutusData {
        PlutusData::Integer(n)
    }

    // -----------------------------------------------------------------------
    // Test 1 — script-locked input with matching datum witness → no error
    // -----------------------------------------------------------------------

    #[test]
    fn test_script_input_datum_present() {
        // Datum to witness
        let datum = int_datum(42);
        let hash = datum_hash_of(&datum);

        // UTxO: script-locked, DatumHash in output
        let utxo_output = script_output_with_datum_hash(hash);
        let (utxo_set, input) = make_utxo(utxo_output);

        let tx = make_tx(vec![input], vec![], vec![], vec![datum]);

        let mut errors: Vec<ValidationError> = vec![];
        check_datum_witnesses(&tx, &utxo_set, &mut errors);

        assert!(
            !errors
                .iter()
                .any(|e| matches!(e, ValidationError::MissingDatumWitness(_))),
            "expected no MissingDatumWitness, got: {errors:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 2 — script-locked input with missing datum witness → error
    // -----------------------------------------------------------------------

    #[test]
    fn test_script_input_datum_missing() {
        // Datum NOT supplied in witness
        let datum = int_datum(99);
        let hash = datum_hash_of(&datum);

        let utxo_output = script_output_with_datum_hash(hash);
        let (utxo_set, input) = make_utxo(utxo_output);

        // witness plutus_data is empty — no datum supplied
        let tx = make_tx(vec![input], vec![], vec![], vec![]);

        let mut errors: Vec<ValidationError> = vec![];
        check_datum_witnesses(&tx, &utxo_set, &mut errors);

        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::MissingDatumWitness(_))),
            "expected MissingDatumWitness, got: {errors:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 3 — inline datum: no witness entry needed
    // -----------------------------------------------------------------------

    #[test]
    fn test_inline_datum_no_witness_needed() {
        // Output carries an inline datum — no witness required
        let data = int_datum(7);
        let utxo_output = script_output_with_inline_datum(data);
        let (utxo_set, input) = make_utxo(utxo_output);

        // witness plutus_data is empty — none supplied
        let tx = make_tx(vec![input], vec![], vec![], vec![]);

        let mut errors: Vec<ValidationError> = vec![];
        check_datum_witnesses(&tx, &utxo_set, &mut errors);

        assert!(
            errors.is_empty(),
            "expected no errors for inline datum, got: {errors:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 4 — VKey-locked input: datum not required even without witness
    // -----------------------------------------------------------------------

    #[test]
    fn test_non_script_input_no_datum() {
        let utxo_output = vkey_output_no_datum();
        let (utxo_set, input) = make_utxo(utxo_output);

        let tx = make_tx(vec![input], vec![], vec![], vec![]);

        let mut errors: Vec<ValidationError> = vec![];
        check_datum_witnesses(&tx, &utxo_set, &mut errors);

        assert!(
            errors.is_empty(),
            "expected no errors for vkey-locked input, got: {errors:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 5 — datum in witness not referenced by any input/output → error
    // -----------------------------------------------------------------------

    #[test]
    fn test_extra_datum_is_hard_error() {
        // UTxO has no datum at all; the witness carries a spurious datum.
        let utxo_output = vkey_output_no_datum();
        let (utxo_set, input) = make_utxo(utxo_output);

        let spurious_datum = int_datum(123);
        let tx = make_tx(vec![input], vec![], vec![], vec![spurious_datum]);

        let mut errors: Vec<ValidationError> = vec![];
        check_datum_witnesses(&tx, &utxo_set, &mut errors);

        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::ExtraDatumWitness(_))),
            "expected ExtraDatumWitness, got: {errors:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 6 — output datum hash supplemental: witness is allowed, not extra
    // -----------------------------------------------------------------------

    #[test]
    fn test_output_datum_hash_supplemental() {
        // Transaction output declares a datum hash — the datum in the witness
        // is "supplemental" (allowed but not required).  Must NOT produce
        // ExtraDatumWitness.
        let datum = int_datum(55);
        let hash = datum_hash_of(&datum);

        // Input is VKey-locked (no datum witness required from input side)
        let utxo_output = vkey_output_no_datum();
        let (utxo_set, input) = make_utxo(utxo_output);

        // Transaction output carries that datum hash
        let tx_output = TransactionOutput {
            address: Address::Base(BaseAddress {
                network: NetworkId::Testnet,
                payment: Credential::VerificationKey(Hash28::from_bytes([0x11u8; 28])),
                stake: Credential::VerificationKey(Hash28::from_bytes([0x22u8; 28])),
            }),
            value: Value::lovelace(2_000_000),
            datum: OutputDatum::DatumHash(hash),
            script_ref: None,
            is_legacy: false,
            raw_cbor: None,
        };

        let tx = make_tx(vec![input], vec![tx_output], vec![], vec![datum]);

        let mut errors: Vec<ValidationError> = vec![];
        check_datum_witnesses(&tx, &utxo_set, &mut errors);

        assert!(
            !errors
                .iter()
                .any(|e| matches!(e, ValidationError::ExtraDatumWitness(_))),
            "expected no ExtraDatumWitness for supplemental output datum, got: {errors:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 7 — reference input datum cannot satisfy spending input requirement
    // -----------------------------------------------------------------------

    #[test]
    fn test_ref_input_datum_supplemental_only() {
        // Both the spending input UTxO and the reference input UTxO carry the
        // same DatumHash.  The datum is NOT in the witness.
        //
        // Because the spending input is script-locked, MissingDatumWitness must
        // still fire — the reference input datum hash only makes the witness
        // *allowed*, it does not satisfy the *required* set.
        let datum = int_datum(77);
        let hash = datum_hash_of(&datum);

        // Spending input: script-locked, DatumHash
        let spend_output = script_output_with_datum_hash(hash);
        let (mut utxo_set, spend_input) = make_utxo(spend_output);

        // Reference input: also carries same DatumHash (but lives at a
        // different input reference so it is distinct from the spending input)
        let ref_tx_input = TransactionInput {
            transaction_id: Hash32::from_bytes([0x55u8; 32]),
            index: 0,
        };
        let ref_output = TransactionOutput {
            address: Address::Base(BaseAddress {
                network: NetworkId::Testnet,
                payment: Credential::VerificationKey(Hash28::from_bytes([0x33u8; 28])),
                stake: Credential::VerificationKey(Hash28::from_bytes([0x44u8; 28])),
            }),
            value: Value::lovelace(2_000_000),
            datum: OutputDatum::DatumHash(hash),
            script_ref: None,
            is_legacy: false,
            raw_cbor: None,
        };
        utxo_set.insert(ref_tx_input.clone(), ref_output);

        // No witness datum supplied
        let tx = make_tx(
            vec![spend_input],
            vec![],
            vec![ref_tx_input],
            vec![], // datum NOT supplied
        );

        let mut errors: Vec<ValidationError> = vec![];
        check_datum_witnesses(&tx, &utxo_set, &mut errors);

        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::MissingDatumWitness(_))),
            "expected MissingDatumWitness even though ref-input carries the same hash, got: {errors:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 8 — two script inputs with different hashes; only one datum supplied
    // -----------------------------------------------------------------------

    #[test]
    fn test_multiple_script_inputs_one_missing() {
        let datum_a = int_datum(10);
        let hash_a = datum_hash_of(&datum_a);

        let datum_b = int_datum(20);
        let hash_b = datum_hash_of(&datum_b);

        // Two distinct UTxOs, both script-locked, each with a different DatumHash
        let output_a = script_output_with_datum_hash(hash_a);
        let output_b = script_output_with_datum_hash(hash_b);

        let mut utxo_set = crate::utxo::UtxoSet::new();
        let input_a = TransactionInput {
            transaction_id: Hash32::from_bytes([0x01u8; 32]),
            index: 0,
        };
        let input_b = TransactionInput {
            transaction_id: Hash32::from_bytes([0x02u8; 32]),
            index: 0,
        };
        utxo_set.insert(input_a.clone(), output_a);
        utxo_set.insert(input_b.clone(), output_b);

        // Only datum_a is in the witness; datum_b is absent
        let tx = make_tx(vec![input_a, input_b], vec![], vec![], vec![datum_a]);

        let mut errors: Vec<ValidationError> = vec![];
        check_datum_witnesses(&tx, &utxo_set, &mut errors);

        let missing: Vec<_> = errors
            .iter()
            .filter(|e| matches!(e, ValidationError::MissingDatumWitness(_)))
            .collect();

        assert_eq!(
            missing.len(),
            1,
            "expected exactly one MissingDatumWitness (for datum_b), got: {errors:?}"
        );

        // Confirm the missing hash is hash_b's hex representation
        if let ValidationError::MissingDatumWitness(hex) = missing[0] {
            assert_eq!(
                hex,
                &hash_b.to_hex(),
                "wrong hash in MissingDatumWitness error"
            );
        }
    }
}
