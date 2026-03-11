use crate::cbor::*;
use torsten_primitives::hash::{blake2b_256, Hash32};
use torsten_primitives::transaction::*;

use super::certificate::encode_certificate;
use super::governance::{encode_proposal_procedure, encode_voting_procedures};
use super::script::{
    encode_bootstrap_witness, encode_metadata_map, encode_native_script, encode_redeemer,
    encode_script_ref, encode_vkey_witness,
};
use super::value::{encode_mint, encode_value};

/// Encode a transaction output (Babbage/Conway post-Alonzo map format).
///
/// Map with keys: 0=address, 1=value, 2=datum_option, 3=script_ref
pub fn encode_transaction_output(output: &TransactionOutput) -> Vec<u8> {
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
        OutputDatum::InlineDatum(data) => {
            buf.extend(encode_uint(2));
            // [1, #6.24(cbor_encoded_data)]
            buf.extend(encode_array_header(2));
            buf.extend(encode_uint(1));
            // Tag 24 (CBOR-encoded data item)
            buf.extend(encode_tag(24));
            let encoded_data = encode_plutus_data(data);
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
pub fn encode_witness_set(ws: &TransactionWitnessSet) -> Vec<u8> {
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
        buf.extend(encode_array_header(ws.redeemers.len()));
        for r in &ws.redeemers {
            buf.extend(encode_redeemer(r));
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

/// Encode a transaction body as CBOR map.
///
/// Required keys: 0=inputs, 1=outputs, 2=fee
/// Optional keys: 3=ttl, 4=certs, 5=withdrawals, 7=aux_data_hash, 8=validity_start,
///                9=mint, 11=script_data_hash, 13=collateral, 14=required_signers,
///                15=network_id, 16=collateral_return, 17=total_collateral,
///                18=reference_inputs, 19=voting_procedures, 20=proposal_procedures,
///                21=treasury_value, 22=donation
pub fn encode_transaction_body(body: &TransactionBody) -> Vec<u8> {
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

    // 0: inputs (set of [tx_hash, index])
    buf.extend(encode_uint(0));
    buf.extend(encode_array_header(body.inputs.len()));
    for input in &body.inputs {
        buf.extend(encode_tx_input(input));
    }

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
    if !body.certificates.is_empty() {
        buf.extend(encode_uint(4));
        buf.extend(encode_array_header(body.certificates.len()));
        for cert in &body.certificates {
            buf.extend(encode_certificate(cert));
        }
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
    if !body.collateral.is_empty() {
        buf.extend(encode_uint(13));
        buf.extend(encode_array_header(body.collateral.len()));
        for input in &body.collateral {
            buf.extend(encode_tx_input(input));
        }
    }

    // 14: required_signers
    if !body.required_signers.is_empty() {
        buf.extend(encode_uint(14));
        buf.extend(encode_array_header(body.required_signers.len()));
        for hash in &body.required_signers {
            buf.extend(encode_hash32(hash));
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
    if !body.reference_inputs.is_empty() {
        buf.extend(encode_uint(18));
        buf.extend(encode_array_header(body.reference_inputs.len()));
        for input in &body.reference_inputs {
            buf.extend(encode_tx_input(input));
        }
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
pub fn encode_transaction(tx: &Transaction) -> Vec<u8> {
    let mut buf = encode_array_header(4);
    buf.extend(encode_transaction_body(&tx.body));
    buf.extend(encode_witness_set(&tx.witness_set));
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
