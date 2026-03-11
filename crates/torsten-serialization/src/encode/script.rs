use crate::cbor::*;
use torsten_primitives::hash::Hash32;
use torsten_primitives::transaction::*;

/// Encode a script reference
pub(crate) fn encode_script_ref(script_ref: &ScriptRef) -> Vec<u8> {
    let mut buf = encode_array_header(2);
    match script_ref {
        ScriptRef::NativeScript(ns) => {
            buf.extend(encode_uint(0));
            buf.extend(encode_native_script(ns));
        }
        ScriptRef::PlutusV1(script) => {
            buf.extend(encode_uint(1));
            buf.extend(encode_bytes(script));
        }
        ScriptRef::PlutusV2(script) => {
            buf.extend(encode_uint(2));
            buf.extend(encode_bytes(script));
        }
        ScriptRef::PlutusV3(script) => {
            buf.extend(encode_uint(3));
            buf.extend(encode_bytes(script));
        }
    }
    buf
}

/// Encode a native script
pub fn encode_native_script(script: &NativeScript) -> Vec<u8> {
    match script {
        NativeScript::ScriptPubkey(hash) => {
            let mut buf = encode_array_header(2);
            buf.extend(encode_uint(0));
            // Native script key hashes are 28 bytes (AddrKeyhash) on the wire
            // Our type stores them padded to Hash32, so truncate back to 28
            buf.extend(encode_bytes(&hash.as_ref()[..28]));
            buf
        }
        NativeScript::ScriptAll(scripts) => {
            let mut buf = encode_array_header(2);
            buf.extend(encode_uint(1));
            buf.extend(encode_array_header(scripts.len()));
            for s in scripts {
                buf.extend(encode_native_script(s));
            }
            buf
        }
        NativeScript::ScriptAny(scripts) => {
            let mut buf = encode_array_header(2);
            buf.extend(encode_uint(2));
            buf.extend(encode_array_header(scripts.len()));
            for s in scripts {
                buf.extend(encode_native_script(s));
            }
            buf
        }
        NativeScript::ScriptNOfK(n, scripts) => {
            let mut buf = encode_array_header(3);
            buf.extend(encode_uint(3));
            buf.extend(encode_uint(*n as u64));
            buf.extend(encode_array_header(scripts.len()));
            for s in scripts {
                buf.extend(encode_native_script(s));
            }
            buf
        }
        NativeScript::InvalidBefore(slot) => {
            let mut buf = encode_array_header(2);
            buf.extend(encode_uint(4));
            buf.extend(encode_uint(slot.0));
            buf
        }
        NativeScript::InvalidHereafter(slot) => {
            let mut buf = encode_array_header(2);
            buf.extend(encode_uint(5));
            buf.extend(encode_uint(slot.0));
            buf
        }
    }
}

/// Encode a redeemer tag
pub(crate) fn encode_redeemer_tag(tag: &RedeemerTag) -> Vec<u8> {
    encode_uint(match tag {
        RedeemerTag::Spend => 0,
        RedeemerTag::Mint => 1,
        RedeemerTag::Cert => 2,
        RedeemerTag::Reward => 3,
        RedeemerTag::Vote => 4,
        RedeemerTag::Propose => 5,
    })
}

/// Encode a redeemer [tag, index, data, ex_units]
pub(crate) fn encode_redeemer(redeemer: &Redeemer) -> Vec<u8> {
    let mut buf = encode_array_header(4);
    buf.extend(encode_redeemer_tag(&redeemer.tag));
    buf.extend(encode_uint(redeemer.index as u64));
    buf.extend(encode_plutus_data(&redeemer.data));
    buf.extend(encode_array_header(2));
    buf.extend(encode_uint(redeemer.ex_units.mem));
    buf.extend(encode_uint(redeemer.ex_units.steps));
    buf
}

/// Encode a VKey witness [vkey, signature]
pub(crate) fn encode_vkey_witness(w: &VKeyWitness) -> Vec<u8> {
    let mut buf = encode_array_header(2);
    buf.extend(encode_bytes(&w.vkey));
    buf.extend(encode_bytes(&w.signature));
    buf
}

/// Encode a bootstrap witness [vkey, signature, chain_code, attributes]
pub(crate) fn encode_bootstrap_witness(w: &BootstrapWitness) -> Vec<u8> {
    let mut buf = encode_array_header(4);
    buf.extend(encode_bytes(&w.vkey));
    buf.extend(encode_bytes(&w.signature));
    buf.extend(encode_bytes(&w.chain_code));
    buf.extend(encode_bytes(&w.attributes));
    buf
}

/// Encode a metadata map: {label: metadatum}
pub(crate) fn encode_metadata_map(
    metadata: &std::collections::BTreeMap<u64, TransactionMetadatum>,
) -> Vec<u8> {
    let mut buf = encode_map_header(metadata.len());
    for (label, value) in metadata {
        buf.extend(encode_uint(*label));
        buf.extend(encode_metadatum(value));
    }
    buf
}

/// Compute the script data hash for transaction integrity verification.
///
/// Per Cardano ledger spec, this is:
///   blake2b_256(redeemers_cbor || datums_cbor || language_views_cbor)
///
/// Where:
/// - redeemers_cbor = CBOR encoding of the redeemers list
/// - datums_cbor = CBOR encoding of plutus datums (tag 258 array), or empty if no datums
/// - language_views_cbor = CBOR encoding of cost models for languages used in the tx
pub fn compute_script_data_hash(
    redeemers: &[Redeemer],
    plutus_data: &[PlutusData],
    cost_models: &CostModels,
    has_v1: bool,
    has_v2: bool,
    has_v3: bool,
) -> Hash32 {
    let mut preimage = Vec::new();

    // 1. Encode redeemers as a CBOR array
    let mut redeemers_buf = encode_array_header(redeemers.len());
    for r in redeemers {
        redeemers_buf.extend(encode_redeemer(r));
    }
    preimage.extend(&redeemers_buf);

    // 2. Encode datums (if any) as #6.258([d1, d2, ...])
    if !plutus_data.is_empty() {
        let mut datums_buf = encode_tag(258);
        datums_buf.extend(encode_array_header(plutus_data.len()));
        for d in plutus_data {
            datums_buf.extend(encode_plutus_data(d));
        }
        preimage.extend(&datums_buf);
    }

    // 3. Encode language views (cost models for languages used in the transaction)
    preimage.extend(encode_language_views(cost_models, has_v1, has_v2, has_v3));

    torsten_primitives::hash::blake2b_256(&preimage)
}

/// Encode cost models as "language views" for script data hash computation.
///
/// Per the Haskell cardano-ledger implementation:
/// - PlutusV1: key = bstr(0x00) "double-bagged", value = bstr(indef_array(...))
/// - PlutusV2: key = uint(1), value = array(...)
/// - PlutusV3: key = uint(2), value = array(...)
///
/// Entries are sorted by short-lex order on key bytes:
/// V2 (0x01, 1 byte) < V3 (0x02, 1 byte) < V1 (0x41 0x00, 2 bytes)
///
/// Only includes cost models for languages actually used in the transaction.
pub(crate) fn encode_language_views(
    cost_models: &CostModels,
    has_v1: bool,
    has_v2: bool,
    has_v3: bool,
) -> Vec<u8> {
    // Collect (key_bytes, value_bytes) pairs
    let mut entries: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();

    if has_v1 {
        if let Some(v1) = &cost_models.plutus_v1 {
            // PlutusV1 key: "double-bagged" — serialize(serialize(0)) = bstr(0x00) = [0x41, 0x00]
            let key = encode_bytes(&encode_uint(0));
            // PlutusV1 value: bstr wrapping indefinite-length CBOR array
            let mut indef_arr = vec![0x9Fu8]; // indefinite-length array start
            for cost in v1 {
                indef_arr.extend(encode_int(*cost as i128));
            }
            indef_arr.push(0xFF); // break
            let value = encode_bytes(&indef_arr);
            entries.push((key, value));
        }
    }
    if has_v2 {
        if let Some(v2) = &cost_models.plutus_v2 {
            // PlutusV2 key: raw CBOR uint 1
            let key = encode_uint(1);
            // PlutusV2 value: definite-length CBOR array (raw, not byte-wrapped)
            let mut value = encode_array_header(v2.len());
            for cost in v2 {
                value.extend(encode_int(*cost as i128));
            }
            entries.push((key, value));
        }
    }
    if has_v3 {
        if let Some(v3) = &cost_models.plutus_v3 {
            // PlutusV3 key: raw CBOR uint 2
            let key = encode_uint(2);
            // PlutusV3 value: definite-length CBOR array (raw, not byte-wrapped)
            let mut value = encode_array_header(v3.len());
            for cost in v3 {
                value.extend(encode_int(*cost as i128));
            }
            entries.push((key, value));
        }
    }

    if entries.is_empty() {
        return encode_map_header(0);
    }

    // Sort by short-lex order on key bytes (shorter keys first, ties broken lexicographically)
    entries.sort_by(|(a, _), (b, _)| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));

    let mut buf = encode_map_header(entries.len());
    for (key, value) in entries {
        buf.extend(key);
        buf.extend(value);
    }
    buf
}
