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
