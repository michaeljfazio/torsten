use crate::cbor::*;
use dugite_primitives::era::Era;
use dugite_primitives::hash::{blake2b_256, Hash32};
use dugite_primitives::transaction::*;

use super::certificate::encode_certificate;
use super::governance::{encode_proposal_procedure, encode_voting_procedures};
use super::script::{
    encode_bootstrap_witness, encode_metadata_map, encode_native_script, encode_redeemer_tag,
    encode_script_ref, encode_vkey_witness,
};
use super::value::{encode_mint, encode_value};

/// Encode a set-typed field as CBOR tag 258 with a sorted definite-length array.
///
/// Per Conway CDDL: `set<a> = #6.258([* a])`. The canonical encoding wraps the
/// items in tag 258. Items are sorted lexicographically by their CBOR encoding
/// to produce canonical ordering.
fn encode_tagged_set<T, F>(items: &[T], encode_item: F) -> Vec<u8>
where
    F: Fn(&T) -> Vec<u8>,
{
    // Encode each item individually so we can sort them.
    let mut encoded_items: Vec<Vec<u8>> = items.iter().map(encode_item).collect();
    // Canonical CBOR set ordering: lexicographic on the CBOR encoding bytes.
    encoded_items.sort();

    // tag(258) followed by definite-length array.
    let mut buf = encode_tag(258);
    buf.extend(encode_array_header(encoded_items.len()));
    for encoded in encoded_items {
        buf.extend(encoded);
    }
    buf
}

/// Encode a sequence as a plain definite-length array (pre-Conway eras).
///
/// Pre-Conway CDDL uses `[* item]` for inputs, certificates, collateral, and
/// reference inputs — NOT `set<item> = #6.258([* item])`. Items are encoded in
/// their original order without sorting, preserving the original transaction body.
fn encode_plain_array<T, F>(items: &[T], encode_item: F) -> Vec<u8>
where
    F: Fn(&T) -> Vec<u8>,
{
    let mut buf = encode_array_header(items.len());
    for item in items {
        buf.extend(encode_item(item));
    }
    buf
}

/// Encode a set-typed body field using the correct format for the given era.
///
/// - Conway: CBOR tag 258 with lexicographically sorted items
///   (`set<a> = #6.258([* a])` per Conway CDDL)
/// - Pre-Conway: plain definite-length array (`[* a]`)
fn encode_set_for_era<T, F>(era: Era, items: &[T], encode_item: F) -> Vec<u8>
where
    F: Fn(&T) -> Vec<u8>,
{
    if era == Era::Conway {
        encode_tagged_set(items, encode_item)
    } else {
        encode_plain_array(items, encode_item)
    }
}

/// Encode a transaction output.
///
/// Two wire-format variants exist:
///
/// **Legacy (Shelley/Allegra/Mary/Alonzo era) — `output.is_legacy = true`**
/// Encoded as a CBOR array: `[address, value]` or `[address, value, datum_hash]`.
/// Conway-era transactions may embed legacy-format outputs for simple change
/// outputs to preserve encoding compatibility with existing tooling.
///
/// **Post-Alonzo (Babbage/Conway era) — `output.is_legacy = false`**
/// Encoded as a CBOR map with optional keys: `{0: address, 1: value, ?2: datum_option, ?3: script_ref}`.
///
/// The `is_legacy` flag is stored in bincode so it survives LSM round-trips.
pub fn encode_transaction_output(output: &TransactionOutput) -> Vec<u8> {
    if output.is_legacy {
        return encode_legacy_transaction_output(output);
    }
    encode_post_alonzo_transaction_output(output)
}

/// Encode a legacy (Shelley-era array format) transaction output.
///
/// Wire format: `[address_bytes, value]` or `[address_bytes, value, datum_hash]`
fn encode_legacy_transaction_output(output: &TransactionOutput) -> Vec<u8> {
    let has_datum_hash = matches!(&output.datum, OutputDatum::DatumHash(_));
    let len = if has_datum_hash { 3 } else { 2 };

    let mut buf = encode_array_header(len);
    buf.extend(encode_bytes(&output.address.to_bytes()));
    buf.extend(encode_value(&output.value));
    if let OutputDatum::DatumHash(h) = &output.datum {
        buf.extend(encode_hash32(h));
    }
    buf
}

/// Encode a post-Alonzo (Babbage/Conway map format) transaction output.
///
/// Map with keys: 0=address, 1=value, 2=datum_option, 3=script_ref
fn encode_post_alonzo_transaction_output(output: &TransactionOutput) -> Vec<u8> {
    let mut count = 2; // address + value are always present
    if output.datum != OutputDatum::None {
        count += 1;
    }
    if output.script_ref.is_some() {
        count += 1;
    }

    let mut buf = encode_map_header(count);

    // 0: address
    buf.extend(encode_uint(0));
    buf.extend(encode_bytes(&output.address.to_bytes()));

    // 1: value
    buf.extend(encode_uint(1));
    buf.extend(encode_value(&output.value));

    // 2: datum_option
    match &output.datum {
        OutputDatum::None => {}
        OutputDatum::DatumHash(h) => {
            buf.extend(encode_uint(2));
            // [0, datum_hash]
            buf.extend(encode_array_header(2));
            buf.extend(encode_uint(0));
            buf.extend(encode_hash32(h));
        }
        OutputDatum::InlineDatum { data, raw_cbor } => {
            buf.extend(encode_uint(2));
            // [1, #6.24(cbor_encoded_data)]
            buf.extend(encode_array_header(2));
            buf.extend(encode_uint(1));
            // Tag 24 (CBOR-encoded data item). Use the preserved raw bytes when
            // available so that encoding details (indefinite-length arrays inside
            // Constr/List, etc.) are reproduced exactly. Falling back to a fresh
            // encode_plutus_data() call would produce definite-length arrays which
            // differ from what many Plutus script builders emit, causing datum hash
            // mismatches in script context construction.
            buf.extend(encode_tag(24));
            let encoded_data = raw_cbor
                .as_deref()
                .map(|r| r.to_vec())
                .unwrap_or_else(|| encode_plutus_data(data));
            buf.extend(encode_bytes(&encoded_data));
        }
    }

    // 3: script_ref
    if let Some(script_ref) = &output.script_ref {
        buf.extend(encode_uint(3));
        // Tag 24 (CBOR-encoded data item)
        buf.extend(encode_tag(24));
        let script_cbor = encode_script_ref(script_ref);
        buf.extend(encode_bytes(&script_cbor));
    }

    buf
}

/// Encode a transaction witness set as CBOR map.
///
/// Map keys: 0=vkeywitnesses, 1=native_scripts, 2=bootstrap_witnesses,
///           3=plutus_v1, 4=plutus_data, 5=redeemers, 6=plutus_v2, 7=plutus_v3
/// Encode a transaction witness set (Conway format by default).
///
/// This is a compatibility wrapper that always uses Conway-era encoding
/// (map format for redeemers). For era-specific encoding, use
/// `encode_witness_set_for_era`.
pub fn encode_witness_set(ws: &TransactionWitnessSet) -> Vec<u8> {
    encode_witness_set_for_era(ws, Era::Conway)
}

/// Encode a transaction witness set using era-specific encoding rules.
///
/// - Conway: redeemers use map format `{ [tag, index] => [data, ex_units] }`
///   (per Conway CDDL `nonempty_map<redeemer_key, redeemer_value>`)
/// - Pre-Conway (Alonzo/Babbage): redeemers use array format
///   `[* [tag, index, data, ex_units]]`
pub(super) fn encode_witness_set_for_era(ws: &TransactionWitnessSet, era: Era) -> Vec<u8> {
    let mut count = 0;
    if !ws.vkey_witnesses.is_empty() {
        count += 1;
    }
    if !ws.native_scripts.is_empty() {
        count += 1;
    }
    if !ws.bootstrap_witnesses.is_empty() {
        count += 1;
    }
    if !ws.plutus_v1_scripts.is_empty() {
        count += 1;
    }
    if !ws.plutus_data.is_empty() {
        count += 1;
    }
    if !ws.redeemers.is_empty() {
        count += 1;
    }
    if !ws.plutus_v2_scripts.is_empty() {
        count += 1;
    }
    if !ws.plutus_v3_scripts.is_empty() {
        count += 1;
    }

    let mut buf = encode_map_header(count);

    if !ws.vkey_witnesses.is_empty() {
        buf.extend(encode_uint(0));
        buf.extend(encode_array_header(ws.vkey_witnesses.len()));
        for w in &ws.vkey_witnesses {
            buf.extend(encode_vkey_witness(w));
        }
    }

    if !ws.native_scripts.is_empty() {
        buf.extend(encode_uint(1));
        buf.extend(encode_array_header(ws.native_scripts.len()));
        for s in &ws.native_scripts {
            buf.extend(encode_native_script(s));
        }
    }

    if !ws.bootstrap_witnesses.is_empty() {
        buf.extend(encode_uint(2));
        buf.extend(encode_array_header(ws.bootstrap_witnesses.len()));
        for w in &ws.bootstrap_witnesses {
            buf.extend(encode_bootstrap_witness(w));
        }
    }

    if !ws.plutus_v1_scripts.is_empty() {
        buf.extend(encode_uint(3));
        buf.extend(encode_array_header(ws.plutus_v1_scripts.len()));
        for s in &ws.plutus_v1_scripts {
            buf.extend(encode_bytes(s));
        }
    }

    if !ws.plutus_data.is_empty() {
        buf.extend(encode_uint(4));
        buf.extend(encode_array_header(ws.plutus_data.len()));
        for d in &ws.plutus_data {
            buf.extend(encode_plutus_data(d));
        }
    }

    if !ws.redeemers.is_empty() {
        buf.extend(encode_uint(5));
        if era == Era::Conway {
            // Conway map format: { [tag, index] => [data, ex_units], ... }
            // Per Conway CDDL: redeemers = nonempty_map<redeemer_key, redeemer_value>
            buf.extend(encode_map_header(ws.redeemers.len()));
            for r in &ws.redeemers {
                // Key: [tag, index]
                buf.extend(encode_array_header(2));
                buf.extend(encode_redeemer_tag(&r.tag));
                buf.extend(encode_uint(r.index as u64));
                // Value: [data, ex_units]
                buf.extend(encode_array_header(2));
                buf.extend(encode_plutus_data(&r.data));
                buf.extend(encode_array_header(2));
                buf.extend(encode_uint(r.ex_units.mem));
                buf.extend(encode_uint(r.ex_units.steps));
            }
        } else {
            // Pre-Conway (Alonzo/Babbage) array format:
            //   [* [tag, index, data, ex_units]]
            buf.extend(encode_array_header(ws.redeemers.len()));
            for r in &ws.redeemers {
                buf.extend(encode_array_header(4));
                buf.extend(encode_redeemer_tag(&r.tag));
                buf.extend(encode_uint(r.index as u64));
                buf.extend(encode_plutus_data(&r.data));
                buf.extend(encode_array_header(2));
                buf.extend(encode_uint(r.ex_units.mem));
                buf.extend(encode_uint(r.ex_units.steps));
            }
        }
    }

    if !ws.plutus_v2_scripts.is_empty() {
        buf.extend(encode_uint(6));
        buf.extend(encode_array_header(ws.plutus_v2_scripts.len()));
        for s in &ws.plutus_v2_scripts {
            buf.extend(encode_bytes(s));
        }
    }

    if !ws.plutus_v3_scripts.is_empty() {
        buf.extend(encode_uint(7));
        buf.extend(encode_array_header(ws.plutus_v3_scripts.len()));
        for s in &ws.plutus_v3_scripts {
            buf.extend(encode_bytes(s));
        }
    }

    buf
}

/// Encode auxiliary data.
///
/// If only metadata and no scripts: metadata map directly.
/// Otherwise: tag 259 with map {0: metadata, 1: native_scripts, 2: plutus_v1, 3: plutus_v2, 4: plutus_v3}
pub fn encode_auxiliary_data(aux: &AuxiliaryData) -> Vec<u8> {
    let has_scripts = !aux.native_scripts.is_empty()
        || !aux.plutus_v1_scripts.is_empty()
        || !aux.plutus_v2_scripts.is_empty()
        || !aux.plutus_v3_scripts.is_empty();

    if !has_scripts {
        // Simple metadata map
        return encode_metadata_map(&aux.metadata);
    }

    // Alonzo+ format: tag 259 { 0: metadata, 1: native_scripts, ... }
    let mut buf = encode_tag(259);
    let mut count = 0;
    if !aux.metadata.is_empty() {
        count += 1;
    }
    if !aux.native_scripts.is_empty() {
        count += 1;
    }
    if !aux.plutus_v1_scripts.is_empty() {
        count += 1;
    }
    if !aux.plutus_v2_scripts.is_empty() {
        count += 1;
    }
    if !aux.plutus_v3_scripts.is_empty() {
        count += 1;
    }

    buf.extend(encode_map_header(count));

    if !aux.metadata.is_empty() {
        buf.extend(encode_uint(0));
        buf.extend(encode_metadata_map(&aux.metadata));
    }
    if !aux.native_scripts.is_empty() {
        buf.extend(encode_uint(1));
        buf.extend(encode_array_header(aux.native_scripts.len()));
        for s in &aux.native_scripts {
            buf.extend(encode_native_script(s));
        }
    }
    if !aux.plutus_v1_scripts.is_empty() {
        buf.extend(encode_uint(2));
        buf.extend(encode_array_header(aux.plutus_v1_scripts.len()));
        for s in &aux.plutus_v1_scripts {
            buf.extend(encode_bytes(s));
        }
    }
    if !aux.plutus_v2_scripts.is_empty() {
        buf.extend(encode_uint(3));
        buf.extend(encode_array_header(aux.plutus_v2_scripts.len()));
        for s in &aux.plutus_v2_scripts {
            buf.extend(encode_bytes(s));
        }
    }
    if !aux.plutus_v3_scripts.is_empty() {
        buf.extend(encode_uint(4));
        buf.extend(encode_array_header(aux.plutus_v3_scripts.len()));
        for s in &aux.plutus_v3_scripts {
            buf.extend(encode_bytes(s));
        }
    }

    buf
}

/// Encode a transaction body as CBOR map (Conway format by default).
///
/// This is a compatibility wrapper that always uses Conway-era encoding
/// (tag 258 for set fields). For era-specific encoding, use
/// `encode_transaction_body_for_era`.
///
/// Required keys: 0=inputs, 1=outputs, 2=fee
/// Optional keys: 3=ttl, 4=certs, 5=withdrawals, 7=aux_data_hash, 8=validity_start,
///                9=mint, 11=script_data_hash, 13=collateral, 14=required_signers,
///                15=network_id, 16=collateral_return, 17=total_collateral,
///                18=reference_inputs, 19=voting_procedures, 20=proposal_procedures,
///                21=treasury_value, 22=donation
pub fn encode_transaction_body(body: &TransactionBody) -> Vec<u8> {
    encode_transaction_body_for_era(body, Era::Conway)
}

/// Encode a transaction body as CBOR map using era-specific encoding rules.
///
/// - Conway: inputs, certificates, collateral, and reference_inputs are
///   encoded as CBOR tag 258 sets (`#6.258([* item])`) with items sorted
///   lexicographically by their CBOR encoding.
/// - Pre-Conway: those fields are encoded as plain definite-length arrays.
pub(super) fn encode_transaction_body_for_era(body: &TransactionBody, era: Era) -> Vec<u8> {
    // Count fields
    let mut count = 3; // inputs, outputs, fee always present
    if body.ttl.is_some() {
        count += 1;
    }
    if !body.certificates.is_empty() {
        count += 1;
    }
    if !body.withdrawals.is_empty() {
        count += 1;
    }
    if body.auxiliary_data_hash.is_some() {
        count += 1;
    }
    if body.validity_interval_start.is_some() {
        count += 1;
    }
    if !body.mint.is_empty() {
        count += 1;
    }
    if body.script_data_hash.is_some() {
        count += 1;
    }
    if !body.collateral.is_empty() {
        count += 1;
    }
    if !body.required_signers.is_empty() {
        count += 1;
    }
    if body.network_id.is_some() {
        count += 1;
    }
    if body.collateral_return.is_some() {
        count += 1;
    }
    if body.total_collateral.is_some() {
        count += 1;
    }
    if !body.reference_inputs.is_empty() {
        count += 1;
    }
    if !body.voting_procedures.is_empty() {
        count += 1;
    }
    if !body.proposal_procedures.is_empty() {
        count += 1;
    }
    if body.treasury_value.is_some() {
        count += 1;
    }
    if body.donation.is_some() {
        count += 1;
    }

    let mut buf = encode_map_header(count);

    // 0: inputs
    // Conway CDDL: set<transaction_input> = #6.258([* transaction_input])
    // Pre-Conway CDDL: [* transaction_input]  (plain array, no tag 258)
    buf.extend(encode_uint(0));
    buf.extend(encode_set_for_era(era, &body.inputs, encode_tx_input));

    // 1: outputs
    buf.extend(encode_uint(1));
    buf.extend(encode_array_header(body.outputs.len()));
    for output in &body.outputs {
        buf.extend(encode_transaction_output(output));
    }

    // 2: fee
    buf.extend(encode_uint(2));
    buf.extend(encode_uint(body.fee.0));

    // 3: ttl
    if let Some(ttl) = body.ttl {
        buf.extend(encode_uint(3));
        buf.extend(encode_uint(ttl.0));
    }

    // 4: certificates
    // Conway CDDL: nonempty_oset<certificate> = #6.258([+ certificate])
    // Pre-Conway CDDL: [* certificate]  (plain array, no tag 258)
    if !body.certificates.is_empty() {
        buf.extend(encode_uint(4));
        buf.extend(encode_set_for_era(
            era,
            &body.certificates,
            encode_certificate,
        ));
    }

    // 5: withdrawals
    if !body.withdrawals.is_empty() {
        buf.extend(encode_uint(5));
        buf.extend(encode_map_header(body.withdrawals.len()));
        for (addr, amount) in &body.withdrawals {
            buf.extend(encode_bytes(addr));
            buf.extend(encode_uint(amount.0));
        }
    }

    // 7: auxiliary_data_hash
    if let Some(hash) = &body.auxiliary_data_hash {
        buf.extend(encode_uint(7));
        buf.extend(encode_hash32(hash));
    }

    // 8: validity_interval_start
    if let Some(start) = body.validity_interval_start {
        buf.extend(encode_uint(8));
        buf.extend(encode_uint(start.0));
    }

    // 9: mint
    if !body.mint.is_empty() {
        buf.extend(encode_uint(9));
        buf.extend(encode_mint(&body.mint));
    }

    // 11: script_data_hash
    if let Some(hash) = &body.script_data_hash {
        buf.extend(encode_uint(11));
        buf.extend(encode_hash32(hash));
    }

    // 13: collateral
    // Conway CDDL: set<transaction_input> = #6.258([* transaction_input])
    // Pre-Conway CDDL: [* transaction_input]  (plain array, no tag 258)
    if !body.collateral.is_empty() {
        buf.extend(encode_uint(13));
        buf.extend(encode_set_for_era(era, &body.collateral, encode_tx_input));
    }

    // 14: required_signers
    // CDDL: required_signers = nonempty_set<addr_keyhash> where addr_keyhash = hash28.
    // required_signers is stored internally as Hash32 (zero-padded from 28-byte pallas hashes),
    // so we emit only the first 28 bytes on the wire to match the CDDL spec.
    if !body.required_signers.is_empty() {
        buf.extend(encode_uint(14));
        buf.extend(encode_array_header(body.required_signers.len()));
        for hash in &body.required_signers {
            buf.extend(encode_bytes(&hash.as_bytes()[..28]));
        }
    }

    // 15: network_id
    if let Some(nid) = body.network_id {
        buf.extend(encode_uint(15));
        buf.extend(encode_uint(nid as u64));
    }

    // 16: collateral_return
    if let Some(output) = &body.collateral_return {
        buf.extend(encode_uint(16));
        buf.extend(encode_transaction_output(output));
    }

    // 17: total_collateral
    if let Some(total) = body.total_collateral {
        buf.extend(encode_uint(17));
        buf.extend(encode_uint(total.0));
    }

    // 18: reference_inputs
    // Conway CDDL: set<transaction_input> = #6.258([* transaction_input])
    // Pre-Conway (Babbage) CDDL: [* transaction_input]  (plain array, no tag 258)
    if !body.reference_inputs.is_empty() {
        buf.extend(encode_uint(18));
        buf.extend(encode_set_for_era(
            era,
            &body.reference_inputs,
            encode_tx_input,
        ));
    }

    // 19: voting_procedures
    if !body.voting_procedures.is_empty() {
        buf.extend(encode_uint(19));
        buf.extend(encode_voting_procedures(&body.voting_procedures));
    }

    // 20: proposal_procedures
    if !body.proposal_procedures.is_empty() {
        buf.extend(encode_uint(20));
        buf.extend(encode_array_header(body.proposal_procedures.len()));
        for pp in &body.proposal_procedures {
            buf.extend(encode_proposal_procedure(pp));
        }
    }

    // 21: treasury_value
    if let Some(treasury) = body.treasury_value {
        buf.extend(encode_uint(21));
        buf.extend(encode_uint(treasury.0));
    }

    // 22: donation
    if let Some(donation) = body.donation {
        buf.extend(encode_uint(22));
        buf.extend(encode_uint(donation.0));
    }

    buf
}

/// Encode a complete transaction: [body, witness_set, is_valid, auxiliary_data]
///
/// Uses era-aware encoding driven by `tx.era`:
/// - Conway: tag 258 for set fields in body; map format for redeemers
/// - Pre-Conway: plain arrays for set fields; array format for redeemers
pub fn encode_transaction(tx: &Transaction) -> Vec<u8> {
    let mut buf = encode_array_header(4);
    buf.extend(encode_transaction_body_for_era(&tx.body, tx.era));
    buf.extend(encode_witness_set_for_era(&tx.witness_set, tx.era));
    buf.extend(encode_bool(tx.is_valid));
    match &tx.auxiliary_data {
        Some(aux) => buf.extend(encode_auxiliary_data(aux)),
        None => buf.extend(encode_null()),
    }
    buf
}

/// Compute the transaction hash from the body encoding (blake2b-256 of CBOR body)
pub fn compute_transaction_hash(body: &TransactionBody) -> Hash32 {
    let body_cbor = encode_transaction_body(body);
    blake2b_256(&body_cbor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dugite_primitives::{
        address::{Address, EnterpriseAddress},
        credentials::Credential,
        hash::{Hash28, Hash32},
        network::NetworkId,
        time::SlotNo,
        transaction::{
            AuxiliaryData, ExUnits, NativeScript, OutputDatum, PlutusData, Redeemer, RedeemerTag,
            ScriptRef, TransactionBody, TransactionInput, TransactionMetadatum, TransactionOutput,
            TransactionWitnessSet, VKeyWitness,
        },
        value::{Lovelace, Value},
    };
    use std::collections::BTreeMap;

    // ── helpers ──────────────────────────────────────────────────────────────

    /// A simple enterprise address on Testnet backed by an all-zero key hash.
    fn test_address() -> Address {
        Address::Enterprise(EnterpriseAddress {
            network: NetworkId::Testnet,
            payment: Credential::VerificationKey(Hash28::ZERO),
        })
    }

    /// Build a minimal ADA-only Value.
    fn ada(lovelace: u64) -> Value {
        Value {
            coin: Lovelace(lovelace),
            multi_asset: BTreeMap::new(),
        }
    }

    /// A single dummy TransactionInput used wherever a body needs at least one input.
    fn dummy_input() -> TransactionInput {
        TransactionInput {
            transaction_id: Hash32::ZERO,
            index: 0,
        }
    }

    /// Build the minimal TransactionBody (inputs, outputs, fee).
    fn minimal_body() -> TransactionBody {
        TransactionBody {
            inputs: vec![dummy_input()],
            outputs: vec![TransactionOutput {
                address: test_address(),
                value: ada(1_000_000),
                datum: OutputDatum::None,
                script_ref: None,
                is_legacy: false,
                raw_cbor: None,
            }],
            fee: Lovelace(170_000),
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
        }
    }

    /// An empty witness set.
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

    // ── encode_transaction_output (legacy) ───────────────────────────────────

    /// Legacy ADA-only output: array(2) [address_bytes, coin]
    #[test]
    fn test_legacy_output_ada_only() {
        let output = TransactionOutput {
            address: test_address(),
            value: ada(2_000_000),
            datum: OutputDatum::None,
            script_ref: None,
            is_legacy: true,
            raw_cbor: None,
        };
        let encoded = encode_transaction_output(&output);
        // First byte: array(2) = 0x82
        assert_eq!(encoded[0], 0x82, "legacy ADA-only must be array(2)");
        // Total length: 1 (array hdr) + len(addr bytes) + len(value)
        // address bytes for enterprise testnet + zero key = 29 bytes → encode_bytes starts 0x58, 29
        assert_eq!(encoded[1], 0x58, "address bytes length-prefix");
        assert_eq!(encoded[2], 29, "enterprise address is 29 bytes");
    }

    /// Legacy output with datum hash: array(3) [address, value, datum_hash]
    #[test]
    fn test_legacy_output_with_datum_hash() {
        let h = Hash32::from_bytes([0xab; 32]);
        let output = TransactionOutput {
            address: test_address(),
            value: ada(1_000_000),
            datum: OutputDatum::DatumHash(h),
            script_ref: None,
            is_legacy: true,
            raw_cbor: None,
        };
        let encoded = encode_transaction_output(&output);
        // First byte: array(3) = 0x83
        assert_eq!(
            encoded[0], 0x83,
            "legacy output with datum hash must be array(3)"
        );
    }

    // ── encode_transaction_output (post-Alonzo) ──────────────────────────────

    /// Minimal post-Alonzo output: map(2) {0: address, 1: value}
    #[test]
    fn test_post_alonzo_output_minimal() {
        let output = TransactionOutput {
            address: test_address(),
            value: ada(1_000_000),
            datum: OutputDatum::None,
            script_ref: None,
            is_legacy: false,
            raw_cbor: None,
        };
        let encoded = encode_transaction_output(&output);
        // First byte: map(2) = 0xa2
        assert_eq!(
            encoded[0], 0xa2,
            "post-Alonzo minimal output must be map(2)"
        );
        // Key 0 must immediately follow
        assert_eq!(encoded[1], 0x00, "first map key must be 0 (address)");
    }

    /// Post-Alonzo output with datum hash: map(3) with key 2 → [0, hash]
    #[test]
    fn test_post_alonzo_output_with_datum_hash() {
        let h = Hash32::from_bytes([0xcd; 32]);
        let output = TransactionOutput {
            address: test_address(),
            value: ada(500_000),
            datum: OutputDatum::DatumHash(h),
            script_ref: None,
            is_legacy: false,
            raw_cbor: None,
        };
        let encoded = encode_transaction_output(&output);
        // map(3) = 0xa3
        assert_eq!(encoded[0], 0xa3, "output with datum hash must be map(3)");
    }

    /// Post-Alonzo output with inline datum: map(3) with key 2 → [1, tag(24)(bytes)]
    #[test]
    fn test_post_alonzo_output_with_inline_datum() {
        let output = TransactionOutput {
            address: test_address(),
            value: ada(500_000),
            datum: OutputDatum::InlineDatum {
                data: PlutusData::Integer(42),
                raw_cbor: None,
            },
            script_ref: None,
            is_legacy: false,
            raw_cbor: None,
        };
        let encoded = encode_transaction_output(&output);
        // map(3) = 0xa3
        assert_eq!(encoded[0], 0xa3, "output with inline datum must be map(3)");
        // Scan for key 2
        let key2_pos = encoded.iter().position(|&b| b == 0x02);
        assert!(
            key2_pos.is_some(),
            "map must contain key 2 for datum_option"
        );
    }

    /// Post-Alonzo output with inline datum uses raw_cbor when provided.
    #[test]
    fn test_post_alonzo_output_inline_datum_uses_raw_cbor() {
        // raw_cbor = CBOR for integer 99 = 0x18 0x63
        let raw = vec![0x18u8, 0x63u8];
        let output = TransactionOutput {
            address: test_address(),
            value: ada(500_000),
            datum: OutputDatum::InlineDatum {
                data: PlutusData::Integer(1), // ignored because raw_cbor is set
                raw_cbor: Some(raw.clone()),
            },
            script_ref: None,
            is_legacy: false,
            raw_cbor: None,
        };
        let encoded = encode_transaction_output(&output);
        // The raw bytes 0x18 0x63 must appear inside the encoding (inside the tag(24) bstr)
        let pos = encoded
            .windows(2)
            .position(|w| w == [0x18, 0x63])
            .expect("raw_cbor bytes must appear in encoding");
        let _ = pos; // just asserting presence
    }

    /// Post-Alonzo output with script_ref: map(3) with key 3
    #[test]
    fn test_post_alonzo_output_with_script_ref() {
        let output = TransactionOutput {
            address: test_address(),
            value: ada(1_000_000),
            datum: OutputDatum::None,
            script_ref: Some(ScriptRef::PlutusV2(vec![0xde, 0xad])),
            is_legacy: false,
            raw_cbor: None,
        };
        let encoded = encode_transaction_output(&output);
        // map(3) = 0xa3
        assert_eq!(encoded[0], 0xa3, "output with script_ref must be map(3)");
        // key 3 must appear
        assert!(
            encoded.contains(&0x03),
            "map must contain key 3 for script_ref"
        );
    }

    /// Post-Alonzo output with datum hash AND script_ref: map(4)
    #[test]
    fn test_post_alonzo_output_with_all_optional_fields() {
        let h = Hash32::from_bytes([0x01; 32]);
        let output = TransactionOutput {
            address: test_address(),
            value: ada(1_000_000),
            datum: OutputDatum::DatumHash(h),
            script_ref: Some(ScriptRef::PlutusV2(vec![0x01, 0x02])),
            is_legacy: false,
            raw_cbor: None,
        };
        let encoded = encode_transaction_output(&output);
        // map(4) = 0xa4
        assert_eq!(
            encoded[0], 0xa4,
            "output with datum + script_ref must be map(4)"
        );
    }

    // ── era-specific set encoding ────────────────────────────────────────────

    /// Conway body: inputs encoded with tag(258) prefix bytes 0xd9 0x01 0x02
    #[test]
    fn test_conway_inputs_use_tag_258() {
        let body = minimal_body();
        let encoded = encode_transaction_body_for_era(&body, Era::Conway);

        // After map header (0xa3) and key 0 (0x00), we should see tag(258) = 0xd9 0x01 0x02
        // Map header: 0xa3; key 0: 0x00 → positions 0, 1
        // Then immediately: 0xd9, 0x01, 0x02
        assert_eq!(encoded[0], 0xa3, "minimal body map(3)");
        assert_eq!(encoded[1], 0x00, "key 0 = inputs");
        assert_eq!(encoded[2], 0xd9, "tag prefix byte 1");
        assert_eq!(encoded[3], 0x01, "tag prefix byte 2");
        assert_eq!(encoded[4], 0x02, "tag prefix byte 3 — completes tag(258)");
    }

    /// Babbage body: inputs encoded as plain array (no tag 258)
    #[test]
    fn test_babbage_inputs_use_plain_array() {
        let body = minimal_body();
        let encoded = encode_transaction_body_for_era(&body, Era::Babbage);

        // After map(3) = 0xa3 and key 0 = 0x00, the array header for 1 input = 0x81
        assert_eq!(encoded[0], 0xa3, "minimal body map(3)");
        assert_eq!(encoded[1], 0x00, "key 0 = inputs");
        // No tag: next byte is array(1) = 0x81
        assert_eq!(encoded[2], 0x81, "plain array(1) for Babbage inputs");
    }

    // ── witness set redeemer format ──────────────────────────────────────────

    /// Conway witness set: redeemer at key 5 encoded as map (0xa1 for single redeemer)
    #[test]
    fn test_conway_witness_redeemers_map_format() {
        let mut ws = empty_witness_set();
        ws.redeemers.push(Redeemer {
            tag: RedeemerTag::Spend,
            index: 0,
            data: PlutusData::Integer(1),
            ex_units: ExUnits {
                mem: 1000,
                steps: 2000,
            },
        });
        let encoded = encode_witness_set_for_era(&ws, Era::Conway);
        // map(1) = 0xa1
        assert_eq!(encoded[0], 0xa1, "witness set with redeemers only: map(1)");
        // key 5 = 0x05
        assert_eq!(encoded[1], 0x05, "redeemer key must be 5");
        // Conway map: map(1) = 0xa1 for one redeemer
        assert_eq!(encoded[2], 0xa1, "Conway redeemers encoded as map(1)");
    }

    /// Babbage witness set: redeemer at key 5 encoded as array (0x81 for single redeemer)
    #[test]
    fn test_babbage_witness_redeemers_array_format() {
        let mut ws = empty_witness_set();
        ws.redeemers.push(Redeemer {
            tag: RedeemerTag::Spend,
            index: 0,
            data: PlutusData::Integer(1),
            ex_units: ExUnits {
                mem: 1000,
                steps: 2000,
            },
        });
        let encoded = encode_witness_set_for_era(&ws, Era::Babbage);
        // map(1) = 0xa1
        assert_eq!(encoded[0], 0xa1, "witness set with redeemers only: map(1)");
        assert_eq!(encoded[1], 0x05, "redeemer key must be 5");
        // Babbage array: array(1) = 0x81 for one redeemer
        assert_eq!(encoded[2], 0x81, "Babbage redeemers encoded as array(1)");
    }

    // ── transaction body ─────────────────────────────────────────────────────

    /// Minimal body has exactly 3 keys (0, 1, 2) → map(3)
    #[test]
    fn test_transaction_body_minimal_map3() {
        let body = minimal_body();
        let encoded = encode_transaction_body(&body);
        // map(3) = 0xa3
        assert_eq!(encoded[0], 0xa3, "minimal body must be map(3)");
    }

    /// Body with TTL gains key 3 → map(4)
    #[test]
    fn test_transaction_body_with_ttl() {
        let mut body = minimal_body();
        body.ttl = Some(SlotNo(999_999));
        let encoded = encode_transaction_body(&body);
        // map(4) = 0xa4
        assert_eq!(encoded[0], 0xa4, "body with TTL must be map(4)");
    }

    /// Body with validity_interval_start gains key 8 → map(4)
    #[test]
    fn test_transaction_body_with_validity_start() {
        let mut body = minimal_body();
        body.validity_interval_start = Some(SlotNo(100));
        let encoded = encode_transaction_body(&body);
        assert_eq!(encoded[0], 0xa4, "body with validity start must be map(4)");
    }

    // ── full transaction encoding ────────────────────────────────────────────

    /// Full transaction: array(4) = 0x84
    #[test]
    fn test_encode_transaction_array4() {
        let body = minimal_body();
        let tx = dugite_primitives::transaction::Transaction {
            hash: Hash32::ZERO,
            era: Era::Conway,
            body,
            witness_set: empty_witness_set(),
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
            raw_body_cbor: None,
            raw_witness_cbor: None,
        };
        let encoded = encode_transaction(&tx);
        // array(4) = 0x84
        assert_eq!(encoded[0], 0x84, "transaction must be array(4)");
    }

    /// is_valid=false encodes as CBOR false (0xf4)
    #[test]
    fn test_encode_transaction_is_valid_false() {
        let body = minimal_body();
        let tx = dugite_primitives::transaction::Transaction {
            hash: Hash32::ZERO,
            era: Era::Conway,
            body,
            witness_set: empty_witness_set(),
            is_valid: false,
            auxiliary_data: None,
            raw_cbor: None,
            raw_body_cbor: None,
            raw_witness_cbor: None,
        };
        let encoded = encode_transaction(&tx);
        // Scan for 0xf4 (CBOR false) — it should appear as the 3rd element
        assert!(
            encoded.contains(&0xf4),
            "is_valid=false must encode as CBOR false (0xf4)"
        );
        // Verify 0xf5 (true) is NOT present
        assert!(
            !encoded.contains(&0xf5),
            "is_valid=false must not contain CBOR true (0xf5)"
        );
    }

    /// is_valid=true encodes as CBOR true (0xf5)
    #[test]
    fn test_encode_transaction_is_valid_true() {
        let body = minimal_body();
        let tx = dugite_primitives::transaction::Transaction {
            hash: Hash32::ZERO,
            era: Era::Conway,
            body,
            witness_set: empty_witness_set(),
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
            raw_body_cbor: None,
            raw_witness_cbor: None,
        };
        let encoded = encode_transaction(&tx);
        assert!(
            encoded.contains(&0xf5),
            "is_valid=true must encode as CBOR true (0xf5)"
        );
    }

    /// Transaction without auxiliary data: last element is CBOR null (0xf6)
    #[test]
    fn test_encode_transaction_no_aux_data_null() {
        let body = minimal_body();
        let tx = dugite_primitives::transaction::Transaction {
            hash: Hash32::ZERO,
            era: Era::Conway,
            body,
            witness_set: empty_witness_set(),
            is_valid: true,
            auxiliary_data: None,
            raw_cbor: None,
            raw_body_cbor: None,
            raw_witness_cbor: None,
        };
        let encoded = encode_transaction(&tx);
        // Last byte must be 0xf6 (null)
        assert_eq!(
            *encoded.last().unwrap(),
            0xf6,
            "no auxiliary data must produce trailing null (0xf6)"
        );
    }

    // ── auxiliary data ───────────────────────────────────────────────────────

    /// Simple auxiliary data (metadata only, no scripts): plain metadata map — no tag
    #[test]
    fn test_aux_data_metadata_only_no_tag() {
        let mut metadata = BTreeMap::new();
        metadata.insert(674_u64, TransactionMetadatum::Text("msg".to_string()));
        let aux = AuxiliaryData {
            metadata,
            native_scripts: vec![],
            plutus_v1_scripts: vec![],
            plutus_v2_scripts: vec![],
            plutus_v3_scripts: vec![],
            raw_cbor: None,
        };
        let encoded = encode_auxiliary_data(&aux);
        // Simple metadata: map(1). No tag 259 prefix.
        // 0xd9 would be a 2-byte tag prefix — must NOT appear as first byte
        assert_ne!(
            encoded[0], 0xd9,
            "metadata-only aux data must not use tag 259 prefix"
        );
        // Must be a map header (major type 5 = 0xa0-0xbf)
        assert!(
            encoded[0] & 0xe0 == 0xa0,
            "metadata-only aux data must start with a map header"
        );
    }

    /// Auxiliary data with native scripts: tag(259) = 0xd9 0x01 0x03
    #[test]
    fn test_aux_data_with_scripts_uses_tag_259() {
        let aux = AuxiliaryData {
            metadata: BTreeMap::new(),
            native_scripts: vec![NativeScript::InvalidBefore(SlotNo(100))],
            plutus_v1_scripts: vec![],
            plutus_v2_scripts: vec![],
            plutus_v3_scripts: vec![],
            raw_cbor: None,
        };
        let encoded = encode_auxiliary_data(&aux);
        // tag(259) = 0xd9 0x01 0x03
        assert_eq!(encoded[0], 0xd9, "aux with scripts: tag byte 1 (0xd9)");
        assert_eq!(encoded[1], 0x01, "aux with scripts: tag byte 2 (0x01)");
        assert_eq!(
            encoded[2], 0x03,
            "aux with scripts: tag byte 3 (0x03 = 259)"
        );
    }

    // ── empty witness set ────────────────────────────────────────────────────

    /// Empty witness set: map(0) = 0xa0
    #[test]
    fn test_empty_witness_set() {
        let ws = empty_witness_set();
        let encoded = encode_witness_set(&ws);
        assert_eq!(
            encoded,
            vec![0xa0],
            "empty witness set must be map(0) = 0xa0"
        );
    }

    // ── witness set with vkeys ───────────────────────────────────────────────

    /// Witness set with one vkey: map(1) with key 0
    #[test]
    fn test_witness_set_with_vkeys_map1_key0() {
        let mut ws = empty_witness_set();
        ws.vkey_witnesses.push(VKeyWitness {
            vkey: vec![0u8; 32],
            signature: vec![0u8; 64],
        });
        let encoded = encode_witness_set(&ws);
        // map(1) = 0xa1
        assert_eq!(encoded[0], 0xa1, "witness set with vkeys must be map(1)");
        // key 0 for vkey_witnesses
        assert_eq!(encoded[1], 0x00, "vkey_witnesses map key must be 0");
    }

    // ── compute_transaction_hash ─────────────────────────────────────────────

    /// Hash must be deterministic (same body → same hash)
    #[test]
    fn test_compute_transaction_hash_deterministic() {
        let body = minimal_body();
        let h1 = compute_transaction_hash(&body);
        let h2 = compute_transaction_hash(&body);
        assert_eq!(h1, h2, "transaction hash must be deterministic");
    }

    /// Hash must be non-zero for a non-empty body
    #[test]
    fn test_compute_transaction_hash_non_zero() {
        let body = minimal_body();
        let h = compute_transaction_hash(&body);
        assert_ne!(
            h,
            Hash32::ZERO,
            "transaction hash of real body must be non-zero"
        );
    }

    /// Two different bodies must produce different hashes
    #[test]
    fn test_compute_transaction_hash_differs_for_different_bodies() {
        let body1 = minimal_body();
        let mut body2 = minimal_body();
        body2.fee = Lovelace(999_999);
        let h1 = compute_transaction_hash(&body1);
        let h2 = compute_transaction_hash(&body2);
        assert_ne!(h1, h2, "different bodies must produce different hashes");
    }
}
