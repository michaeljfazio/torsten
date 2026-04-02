use crate::cbor::*;
use dugite_primitives::hash::Hash32;
use dugite_primitives::transaction::*;

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

/// Encode a redeemer in Babbage array format: [tag, index, data, ex_units]
///
/// This is the pre-Conway array format. Conway transactions use map format
/// instead (see `encode_witness_set` in transaction.rs). Kept for compatibility
/// with pre-Conway era serialization and as a utility function.
#[allow(dead_code)]
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
/// When `raw_redeemers_cbor` and `raw_datums_cbor` are provided (from pallas
/// deserialization), they are used directly instead of re-encoding. This
/// preserves the original encoding format (map vs array for redeemers,
/// definite vs indefinite-length arrays for datums), which is essential
/// for matching the hash computed by the transaction builder.
///
/// Only the language views (cost models) are freshly encoded from protocol
/// parameters, matching what the Haskell cardano-ledger does.
#[allow(clippy::too_many_arguments)]
pub fn compute_script_data_hash(
    redeemers: &[Redeemer],
    plutus_data: &[PlutusData],
    cost_models: &CostModels,
    has_v1: bool,
    has_v2: bool,
    has_v3: bool,
    raw_redeemers_cbor: Option<&[u8]>,
    raw_datums_cbor: Option<&[u8]>,
) -> Hash32 {
    let mut preimage = Vec::new();

    // 1. Redeemers: use raw CBOR when available, otherwise re-encode.
    //
    // Conway uses map format for redeemers in the script data hash preimage:
    //   { [tag, index] => [data, ex_units], ... }
    // Empty redeemers are encoded as 0xa0 (empty map), not 0x80 (empty array),
    // matching Haskell's `hashScriptIntegrity` which uses `encodeRedeemers` always
    // producing a map in the Conway era.
    if let Some(raw) = raw_redeemers_cbor {
        preimage.extend_from_slice(raw);
    } else if redeemers.is_empty() {
        // Empty redeemers: use 0xa0 (empty map) for Conway compatibility.
        preimage.push(0xa0);
    } else {
        // Re-encode as Conway map format: { [tag, index] => [data, ex_units] }
        let mut redeemers_buf = encode_map_header(redeemers.len());
        for r in redeemers {
            redeemers_buf.extend(encode_array_header(2));
            redeemers_buf.extend(encode_redeemer_tag(&r.tag));
            redeemers_buf.extend(encode_uint(r.index as u64));
            redeemers_buf.extend(encode_array_header(2));
            redeemers_buf.extend(encode_plutus_data(&r.data));
            redeemers_buf.extend(encode_array_header(2));
            redeemers_buf.extend(encode_uint(r.ex_units.mem));
            redeemers_buf.extend(encode_uint(r.ex_units.steps));
        }
        preimage.extend(&redeemers_buf);
    }

    // 2. Datums: use raw CBOR when available, otherwise re-encode
    if let Some(raw) = raw_datums_cbor {
        preimage.extend_from_slice(raw);
    } else if !plutus_data.is_empty() {
        let mut datums_buf = encode_tag(258);
        datums_buf.extend(encode_array_header(plutus_data.len()));
        for d in plutus_data {
            datums_buf.extend(encode_plutus_data(d));
        }
        preimage.extend(&datums_buf);
    }

    // 3. Encode language views (cost models for languages used in the transaction)
    preimage.extend(encode_language_views(cost_models, has_v1, has_v2, has_v3));

    dugite_primitives::hash::blake2b_256(&preimage)
}

/// Compute script_data_hash by re-parsing raw transaction CBOR with pallas.
///
/// This uses pallas's native CBOR handling (KeepRaw for datums, Redeemers
/// enum for map/array format) to produce the exact hash that the Haskell
/// cardano-ledger computes. Returns None if the CBOR can't be parsed.
/// Compute script_data_hash using pallas's ScriptData module.
///
/// Re-parses the transaction CBOR with pallas to access KeepRaw fields,
/// builds a LanguageView from the cost models, and uses pallas's own
/// hash computation which matches the Haskell cardano-ledger exactly.
pub fn compute_script_data_hash_from_cbor(
    tx_cbor: &[u8],
    cost_models: &CostModels,
    has_v1: bool,
    has_v2: bool,
    has_v3: bool,
) -> Option<Hash32> {
    use pallas_codec::minicbor;
    // Try Conway format first (most common on current networks)
    let tx = minicbor::decode::<pallas_primitives::conway::Tx>(tx_cbor).ok()?;
    let ws = &tx.transaction_witness_set;

    if ws.redeemer.is_none() && ws.plutus_data.is_none() {
        return None;
    }

    let mut preimage = Vec::new();

    // 1. Redeemers: use KeepRaw raw_cbor() for exact original encoding
    if let Some(ref redeemers) = ws.redeemer {
        preimage.extend_from_slice(redeemers.raw_cbor());
    } else {
        preimage.push(0xa0); // empty map
    }

    // 2. Datums: use KeepRaw raw_cbor() for exact original encoding
    if let Some(ref datums) = ws.plutus_data {
        preimage.extend_from_slice(datums.raw_cbor());
    }

    // 3. Language views: use our multi-language encoding (handles V1+V2+V3)
    preimage.extend(encode_language_views(cost_models, has_v1, has_v2, has_v3));

    Some(dugite_primitives::hash::blake2b_256(&preimage))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_script_data_hash_from_real_tx_cbor() {
        // Real failing tx from preview testnet (Alonzo-style array redeemers in Conway era)
        // Expected script_data_hash from tx body field 0b:
        let expected_hex = "7482063745239a453494a4700d4e9e481c745603355fa31fdc5cee2ca0c20d3d";
        let expected = Hash32::from_hex(expected_hex).unwrap();

        // The full tx CBOR (from Koios)
        let tx_cbor_hex = include_str!("../../test_data/script_data_hash_test_tx.hex");
        let tx_cbor = hex::decode(tx_cbor_hex.trim()).unwrap();

        // Use preview cost models (V2 only - this tx uses V2 scripts)
        let cost_models = CostModels {
            plutus_v1: None,
            plutus_v2: Some(vec![
                100788, 420, 1, 1, 1000, 173, 0, 1, 1000, 59957, 4, 1, 11183, 32, 201305, 8356, 4,
                16000, 100, 16000, 100, 16000, 100, 16000, 100, 16000, 100, 16000, 100, 100, 100,
                16000, 100, 94375, 32, 132994, 32, 61462, 4, 72010, 178, 0, 1, 22151, 32, 91189,
                769, 4, 2, 85848, 228465, 122, 0, 1, 1, 1000, 42921, 4, 2, 24548, 29498, 38, 1,
                898148, 27279, 1, 51775, 558, 1, 39184, 1000, 60594, 1, 141895, 32, 83150, 32,
                15299, 32, 76049, 1, 13169, 4, 22100, 10, 28999, 74, 1, 28999, 74, 1, 43285, 552,
                1, 44749, 541, 1, 33852, 32, 68246, 32, 72362, 32, 7243, 32, 7391, 32, 11546, 32,
                85848, 228465, 122, 0, 1, 1, 90434, 519, 0, 1, 74433, 32, 85848, 228465, 122, 0, 1,
                1, 85848, 228465, 122, 0, 1, 1, 955506, 213312, 0, 2, 270652, 22588, 4, 1457325,
                64566, 4, 20467, 1, 4, 0, 141992, 32, 100788, 420, 1, 1, 81663, 32, 59498, 32,
                20142, 32, 24588, 32, 20744, 32, 25933, 32, 24623, 32, 43053543, 10, 53384111,
                14333, 10, 43574283, 26308, 10,
            ]),
            plutus_v3: None,
        };

        let result = compute_script_data_hash_from_cbor(&tx_cbor, &cost_models, false, true, false);

        assert_eq!(
            result,
            Some(expected),
            "Script data hash from real tx CBOR should match declared hash"
        );
    }

    #[test]
    fn test_script_data_hash_survives_reencode() {
        // Verify that decoding + re-encoding a tx preserves the script_data_hash
        let tx_cbor_hex = include_str!("../../test_data/script_data_hash_test_tx.hex");
        let tx_cbor = hex::decode(tx_cbor_hex.trim()).unwrap();

        // Decode with pallas and re-encode (simulating what tx.encode() does)
        let tx: pallas_primitives::conway::Tx = pallas_codec::minicbor::decode(&tx_cbor).unwrap();
        let reencoded = pallas_codec::minicbor::to_vec(&tx).unwrap();

        let cost_models = CostModels {
            plutus_v1: None,
            plutus_v2: Some(vec![
                100788, 420, 1, 1, 1000, 173, 0, 1, 1000, 59957, 4, 1, 11183, 32, 201305, 8356, 4,
                16000, 100, 16000, 100, 16000, 100, 16000, 100, 16000, 100, 16000, 100, 100, 100,
                16000, 100, 94375, 32, 132994, 32, 61462, 4, 72010, 178, 0, 1, 22151, 32, 91189,
                769, 4, 2, 85848, 228465, 122, 0, 1, 1, 1000, 42921, 4, 2, 24548, 29498, 38, 1,
                898148, 27279, 1, 51775, 558, 1, 39184, 1000, 60594, 1, 141895, 32, 83150, 32,
                15299, 32, 76049, 1, 13169, 4, 22100, 10, 28999, 74, 1, 28999, 74, 1, 43285, 552,
                1, 44749, 541, 1, 33852, 32, 68246, 32, 72362, 32, 7243, 32, 7391, 32, 11546, 32,
                85848, 228465, 122, 0, 1, 1, 90434, 519, 0, 1, 74433, 32, 85848, 228465, 122, 0, 1,
                1, 85848, 228465, 122, 0, 1, 1, 955506, 213312, 0, 2, 270652, 22588, 4, 1457325,
                64566, 4, 20467, 1, 4, 0, 141992, 32, 100788, 420, 1, 1, 81663, 32, 59498, 32,
                20142, 32, 24588, 32, 20744, 32, 25933, 32, 24623, 32, 43053543, 10, 53384111,
                14333, 10, 43574283, 26308, 10,
            ]),
            plutus_v3: None,
        };

        // Original CBOR should work
        let original =
            compute_script_data_hash_from_cbor(&tx_cbor, &cost_models, false, true, false);
        // Re-encoded CBOR should also work
        let from_reencoded =
            compute_script_data_hash_from_cbor(&reencoded, &cost_models, false, true, false);

        let expected_hex = "7482063745239a453494a4700d4e9e481c745603355fa31fdc5cee2ca0c20d3d";
        let expected = Hash32::from_hex(expected_hex).unwrap();

        assert_eq!(original, Some(expected), "Original CBOR hash mismatch");
        assert_eq!(
            from_reencoded,
            Some(expected),
            "Re-encoded CBOR hash mismatch"
        );
    }
}
