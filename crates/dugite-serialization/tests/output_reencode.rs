//! Tests that verify TransactionOutput re-encoding is byte-exact after an LSM round-trip.
//!
//! This is the critical invariant for Plutus script correctness: when a TransactionOutput is
//! deserialized from wire CBOR and then re-encoded (either directly or after being serialized
//! to/from bincode for the LSM store), the resulting CBOR must be byte-identical to the
//! original output bytes that pallas extracted via `output.encode()`.
//!
//! Why this matters:
//! - Plutus scripts hash the script context, which includes the UTxO output CBOR.
//!   A byte difference (e.g. indefinite-length vs definite-length array) changes the hash
//!   and causes script evaluation to fail.
//! - N2C `GetUTxO` responses must match what cardano-node would return for the same UTxO.
//!
//! The LSM store uses bincode serialization, which skips `raw_cbor` on `TransactionOutput`
//! (via `#[serde(skip)]`). After an LSM round-trip, `raw_cbor` is `None` and
//! `encode_transaction_output()` must fall back to re-encoding from the parsed fields.
//! For inline datums this re-encoding is only byte-exact if the raw datum CBOR bytes are
//! preserved inside `OutputDatum::InlineDatum.raw_cbor`.
//!
//! Test vectors are real Conway-era transactions from the Preview testnet (network magic 2),
//! fetched via Koios on 2026-03-15.

use dugite_primitives::transaction::OutputDatum;
use dugite_serialization::{decode_transaction, encode_transaction_output};
use pallas_traverse::MultiEraTx;

/// Simulate an LSM round-trip by serializing via bincode (which drops `raw_cbor` on
/// TransactionOutput) and deserializing back.
fn lsm_round_trip(
    output: &dugite_primitives::transaction::TransactionOutput,
) -> dugite_primitives::transaction::TransactionOutput {
    let bytes = bincode::serialize(output).expect("bincode serialize");
    let restored: dugite_primitives::transaction::TransactionOutput =
        bincode::deserialize(&bytes).expect("bincode deserialize");
    // The restored output has raw_cbor = None (serde(skip))
    assert!(
        restored.raw_cbor.is_none(),
        "raw_cbor must be None after LSM round-trip (serde skip)"
    );
    restored
}

// ---------------------------------------------------------------------------
// Test vectors — real Conway-era transactions from Preview testnet
// (fetched via Koios on 2026-03-15, epoch 1237)
// ---------------------------------------------------------------------------

/// Tx 5dea2b05... — 2 outputs: one with multi-asset + inline datum (indef-length),
/// one with plain ADA + no datum. Confirms indef-length inline datum bytes are preserved.
const TX_5DEA2B: &str = "84aa0083825820b51c6897649d809d06265e43c1939fa99de6deebfb11beffdb0bc3dff727ac2102825820b51c6897649d809d06265e43c1939fa99de6deebfb11beffdb0bc3dff727ac21008258201ddac0bec670e57a65b9af533a5fe8857392a7ba2b854e58558b7878c8ecf33b030182a300581d70036806576a451f316d3a3446597a90001cb9e1a951ce96328c7be5ca01821a005b8d80a3581c919d4c2c9455016289341b1a14dedf697687af31751170d56a31466ea1477453554e4441451a59682f03581ce757130df645e0af6f9428f51f690f9d603cecd7e39f78447c1af578a14001581ce9084d8d340581401f48dd1ceb8f88b78c33120164b7db3d465a4a07a14001028201d818479f0000000000ff82583900f97ccf4fe384174e63f2250a3c4a61f851112ed0b93caeb6aa84308e95d8feca512588e1a954ee5a8ae2beff32ea360590be694b5065d8581a2a24204a021a0006d291031a065f8551081a065f84250b58206abcaf879b12c98712caa98f85132661318fe3200f07517f9017a3e7475ee9520d818258201ddac0bec670e57a65b9af533a5fe8857392a7ba2b854e58558b7878c8ecf33b031082583900f97ccf4fe384174e63f2250a3c4a61f851112ed0b93caeb6aa84308e95d8feca512588e1a954ee5a8ae2beff32ea360590be694b5065d8581a2a20b701111a000a3bda12848258200f9a8e91d991410c83f72026358732af40ed2920cf3a4e9255259fb01296d231008258200f9a8e91d991410c83f72026358732af40ed2920cf3a4e9255259fb01296d23101825820b51c6897649d809d06265e43c1939fa99de6deebfb11beffdb0bc3dff727ac21018258209bd62c1e98645a0a5fa5c43d3131cb5c7dcd196d23210d940517aa5e73b3355300a200818258206e290ca92c2484c00955ecf4f6c2e539e2223365872a2018ba1cf761aa3683c85840a8521193212d42322f1edd4c88f9e96f644a798200e9f426f4f976227d05aa5e1334a3b9e21799460b94b3f9b857b023fa9633880d9d7d1fe05b0af8ad3144000582840001d87980821a0009a6ea1a0dc665e1840002d87980821a0003e0061a0509c523f5f6";

/// Tx 72489dcb... — 2 outputs: one with multi-asset + complex Constr inline datum
/// (nested Constr fields all using indef-length arrays), one with plain ADA.
/// This is the hardest case: deeply-nested indef-length Constr datums.
const TX_72489D: &str = "84a400d90102828258205373ce63fe7da9a749aa9449d2f1c3d2398c34dd9f71565ab428701791bc344a01825820efdc335b09497801f54b3129661ee6ac536715c03519eb8c921a86418fa26246000182a300581d7008248a1bcad3b73a31090dc5f458c920d8b0f8cf9f7c76d0a5b7212c01821a001e8480a1581c45df5f274b8950b512b08d10656864958659c4ecf3ffad092ef63024a14573555344721a000f4240028201d8185889d8799fd8799f581cb0fa78d409e29fc618ceeaa8a7d7b2639fc53ce282c209dbc080cc3cffd8799fd8799fd87a9f581c3494381b1e46838b161044c0db48890b07ac30c3a97b606141ba5d60ffd8799fd8799fd8799f581cb0fa78d409e29fc618ceeaa8a7d7b2639fc53ce282c209dbc080cc3cffffffffd87980ffd87e9f1a000f4240ffd87980ff82583900ee835f8e356d774f54b90598cc53f44cbf894b57ca714af06a1d9707b0fa78d409e29fc618ceeaa8a7d7b2639fc53ce282c209dbc080cc3c821a002672d9a1581c45df5f274b8950b512b08d10656864958659c4ecf3ffad092ef63024a14573555344721a001bbcbf021a0002e7f107582037e81053faf9b8176fa5bc67c169f33f9c53fb3282adea8d2155829a77ef766ea100d9010281825820a21b0766ba9d15e266663484c4c046dac69e4401ccc21be2c073bbb10c61dda658409463ad703703f75cce96caaa3907ef91177aa1913232bc9966036b95649bbe2fa525e813fe76e57cb036391024560d21751510744ff8ce850c0f18b3558d2e0bf5a11a034f6388a26b756e6c6f636b5f74696d651a065f94606b64657374696e6174696f6ea2727061796d656e745f63726564656e7469616ca16f566572696669636174696f6e4b657978386565383335663865333536643737346635346239303539386363353366343463626638393462353763613731346166303661316439373037707374616b655f63726564656e7469616ca166496e6c696e65a16f566572696669636174696f6e4b657978386230666137386434303965323966633631386365656161386137643762323633396663353363653238326332303964626330383063633363";

/// Tx ee50ed28... — 4 outputs, mix of multi-asset outputs with complex Constr inline datums
/// and one plain-ADA change output. Tests multi-output transactions.
const TX_EE50ED: &str = "84aa00838258205dea2b05335efeecec83f88e0916917b120fc6f8b467dcb04f9f3802c544bbab00825820b51c6897649d809d06265e43c1939fa99de6deebfb11beffdb0bc3dff727ac21018258205dea2b05335efeecec83f88e0916917b120fc6f8b467dcb04f9f3802c544bbab010184a300581d7077dc958c8b69ea789afa124a0bc0b15bc3af20bb1be37e914af82dcb01821a002dc6c0a1581ce757130df645e0af6f9428f51f690f9d603cecd7e39f78447c1af578a14001028201d818479f0000000000ffa300581d70b50cb52bb7bfe31c92d51f774189b5a1993a4367fe9724008c64639801821a002dc6c0a1581cc2ebb8db0851d4ec1b836753baf004d57b7e29f300923040cc25f9c1a14001028201d81858719f1a59682f03001b0000006ea5c6da3a1b00000001e80bcac11a6cc9e9c01a013caeeb9fc24d757ca36e6ec5a2af6619060ac3c24f5f3c3462365d16c284000000000000ff1b0000019cf1736f041b0000019cf1915ae89f1b00000002987be7f71b0000006ea5c6da3aff1a002dc6c0ffa300581d700c716cc2be8c60e2e517429d3fa276e82c95304f7d98e7ba64b82dab01821a002dc6c0a2581c919d4c2c9455016289341b1a14dedf697687af31751170d56a31466ea1477453554e4441451a59682f03581ce9084d8d340581401f48dd1ceb8f88b78c33120164b7db3d465a4a07a14001028201d8184a9f9f0000000000ff00ff82583900f97ccf4fe384174e63f2250a3c4a61f851112ed0b93caeb6aa84308e95d8feca512588e1a954ee5a8ae2beff32ea360590be694b5065d8581a2a1c3936021a0007e714031a065f8561081a065f84350b582072e8d5686c11f8aaf099500154ba1f8e2da97e94b3db3aa326b5bb455883dcad0d818258205dea2b05335efeecec83f88e0916917b120fc6f8b467dcb04f9f3802c544bbab011082583900f97ccf4fe384174e63f2250a3c4a61f851112ed0b93caeb6aa84308e95d8feca512588e1a954ee5a8ae2beff32ea360590be694b5065d8581a2a1845ac111a000bda9e1283825820e1d69ced45b1998deb9542fd92f4b724eeb5634a5d28a383e8c6ba5af0a824f10082582009f12f9f7518862789045dc2dbd51434d79085093ddde0f9412309d59466890c048258209bd62c1e98645a0a5fa5c43d3131cb5c7dcd196d23210d940517aa5e73b3355300a200818258206e290ca92c2484c00955ecf4f6c2e539e2223365872a2018ba1cf761aa3683c858403532e361b0d8c07c187c7f9760178f2bdd5372d87f8fdf217fb8ef07db527f5e114e75db88581b5f41960ea42c29140fa38d2981603c924312b024ef5bef700a05828400001a002dc6c0821a00196a8f1a1e3b37e7840002d87980821963401a007aed4af5f6";

// ---------------------------------------------------------------------------
// Core round-trip assertion
// ---------------------------------------------------------------------------

/// Assert that every output in a transaction encodes byte-identically to
/// what pallas produced via `output.encode()`, both:
/// (a) directly after deserialization (raw_cbor is set)
/// (b) after simulated LSM round-trip (raw_cbor is None — re-encoding path)
fn assert_outputs_reencode_exact(tx_cbor_hex: &str, tx_label: &str) {
    let cbor = hex::decode(tx_cbor_hex.trim()).expect("hex decode");

    // Decode with dugite
    let tx = decode_transaction(6, &cbor).expect("decode_transaction");

    // Decode with pallas to get reference output.encode() bytes
    let pallas_tx =
        MultiEraTx::decode_for_era(pallas_traverse::Era::Conway, &cbor).expect("pallas decode");
    let pallas_outputs: Vec<Vec<u8>> = pallas_tx.outputs().iter().map(|o| o.encode()).collect();

    assert_eq!(
        tx.body.outputs.len(),
        pallas_outputs.len(),
        "{tx_label}: output count mismatch"
    );

    for (i, (output, pallas_ref)) in tx
        .body
        .outputs
        .iter()
        .zip(pallas_outputs.iter())
        .enumerate()
    {
        // (a) Direct re-encode: raw_cbor is Some — path used when output is
        //     freshly deserialized and not yet persisted to LSM.
        let direct = encode_transaction_output(output);
        assert_eq!(
            &direct,
            pallas_ref,
            "{tx_label} output {i}: direct re-encode differs from pallas output.encode()\n\
             expected: {}\n\
             got:      {}",
            hex::encode(pallas_ref),
            hex::encode(&direct),
        );

        // (b) After LSM round-trip: raw_cbor becomes None (serde skip).
        //     The re-encoding path must still reproduce pallas bytes exactly.
        let restored = lsm_round_trip(output);
        let after_lsm = encode_transaction_output(&restored);
        assert_eq!(
            &after_lsm,
            pallas_ref,
            "{tx_label} output {i}: post-LSM re-encode differs from pallas output.encode()\n\
             expected: {}\n\
             got:      {}",
            hex::encode(pallas_ref),
            hex::encode(&after_lsm),
        );
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Verifies that the first inline datum in tx 5dea2b (a 5-element indef-length
/// array wrapped in tag(24)) is preserved byte-exactly through both direct
/// re-encoding and after LSM round-trip.
#[test]
fn test_output_reencode_simple_indef_datum() {
    assert_outputs_reencode_exact(TX_5DEA2B, "tx_5dea2b");
}

/// Verifies that deeply-nested Constr datums (all using indef-length arrays)
/// in tx 72489d are preserved byte-exactly. This is the hardest case: every
/// Constr alternative in the datum uses 0x9f..0xff rather than definite arrays.
#[test]
fn test_output_reencode_nested_constr_datum() {
    assert_outputs_reencode_exact(TX_72489D, "tx_72489d");
}

/// Verifies 4-output transaction with multiple inline datums of different shapes.
#[test]
fn test_output_reencode_multi_output_mixed_datums() {
    assert_outputs_reencode_exact(TX_EE50ED, "tx_ee50ed");
}

/// Verifies that `OutputDatum::InlineDatum.raw_cbor` is populated during
/// deserialization and that the specific indef-length datum bytes are preserved.
#[test]
fn test_inline_datum_raw_cbor_populated() {
    let cbor = hex::decode(TX_5DEA2B).expect("hex decode");
    let tx = decode_transaction(6, &cbor).expect("decode_transaction");

    // Output 0 has an inline datum: indef-length array [0,0,0,0,0]
    let output0 = &tx.body.outputs[0];
    match &output0.datum {
        OutputDatum::InlineDatum { raw_cbor, .. } => {
            let raw = raw_cbor
                .as_ref()
                .expect("raw_cbor must be populated after deserialization");
            // The original datum is: 9f 00 00 00 00 00 ff (indef-length array of 5 zeros)
            assert_eq!(
                raw.as_slice(),
                &[0x9f, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff],
                "inline datum raw_cbor should contain original indef-length bytes"
            );
        }
        other => panic!("expected InlineDatum, got: {other:?}"),
    }
}

/// Verifies that `raw_cbor` on `OutputDatum::InlineDatum` survives bincode
/// serialization/deserialization (i.e., it is NOT marked `#[serde(skip)]`).
#[test]
fn test_inline_datum_raw_cbor_survives_bincode() {
    let cbor = hex::decode(TX_5DEA2B).expect("hex decode");
    let tx = decode_transaction(6, &cbor).expect("decode_transaction");
    let output0 = &tx.body.outputs[0];

    // Verify raw_cbor is present before round-trip
    match &output0.datum {
        OutputDatum::InlineDatum { raw_cbor, .. } => {
            assert!(raw_cbor.is_some(), "raw_cbor should be Some before bincode");
        }
        other => panic!("expected InlineDatum, got: {other:?}"),
    }

    // Bincode round-trip (simulates LSM storage)
    let bytes = bincode::serialize(output0).expect("bincode serialize");
    let restored: dugite_primitives::transaction::TransactionOutput =
        bincode::deserialize(&bytes).expect("bincode deserialize");

    // After bincode: TransactionOutput.raw_cbor is None (serde skip),
    // but OutputDatum::InlineDatum.raw_cbor should be preserved.
    assert!(
        restored.raw_cbor.is_none(),
        "TransactionOutput.raw_cbor must be None after bincode (serde skip)"
    );
    match &restored.datum {
        OutputDatum::InlineDatum { raw_cbor, .. } => {
            let raw = raw_cbor
                .as_ref()
                .expect("OutputDatum::InlineDatum.raw_cbor must survive bincode");
            assert_eq!(
                raw.as_slice(),
                &[0x9f, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff],
                "datum raw_cbor must survive bincode with original indef-length bytes"
            );
        }
        other => panic!("expected InlineDatum after bincode, got: {other:?}"),
    }
}

/// Documents what the bug looked like before the fix: encoding a PlutusData::List
/// produces definite-length arrays (0x85 for 5 elements) rather than the
/// original indefinite-length encoding (0x9f..0xff). This test records the
/// specific byte difference that would break Plutus script context hashing.
#[test]
fn test_indef_array_encoding_difference_documented() {
    use dugite_primitives::transaction::PlutusData;
    use dugite_serialization::encode_plutus_data;

    // Five zeros as a Plutus List
    let datum = PlutusData::List(vec![
        PlutusData::Integer(0),
        PlutusData::Integer(0),
        PlutusData::Integer(0),
        PlutusData::Integer(0),
        PlutusData::Integer(0),
    ]);

    let our_encoding = encode_plutus_data(&datum);
    // Our fresh encoder produces definite-length: 0x85 (array(5)), then 5x 0x00
    assert_eq!(
        our_encoding,
        vec![0x85, 0x00, 0x00, 0x00, 0x00, 0x00],
        "encode_plutus_data produces definite-length array (6 bytes)"
    );

    // The original on-chain bytes use indefinite-length: 0x9f, 5x 0x00, 0xff (7 bytes)
    let original_on_chain: Vec<u8> = vec![0x9f, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff];

    // They differ — this is why raw_cbor must be preserved
    assert_ne!(
        our_encoding, original_on_chain,
        "fresh encoder and on-chain bytes differ: this is why raw_cbor must be preserved"
    );

    // The difference would produce a wrong tag(24) wrapper too:
    // Original: d8 18 47 9f 00 00 00 00 00 ff  (tag 24, bytes(7), 7 datum bytes)
    // Ours:     d8 18 46 85 00 00 00 00 00     (tag 24, bytes(6), 6 datum bytes)
    //                ^^ different length prefix!
}

/// Verifies that `collateral_return` outputs also get `raw_cbor` populated during
/// deserialization. TX_5DEA2B has a collateral_return (body key 16) which is a
/// plain ADA output. After our fix, `convert_output_with_cbor` is used for
/// collateral_return so `raw_cbor` is set and re-encoding is byte-exact.
#[test]
fn test_collateral_return_raw_cbor_preserved() {
    let cbor = hex::decode(TX_5DEA2B).expect("hex decode");
    let tx = decode_transaction(6, &cbor).expect("decode_transaction");

    // TX_5DEA2B has a collateral_return (body key 16 = 0x10)
    let cr = tx
        .body
        .collateral_return
        .as_ref()
        .expect("TX_5DEA2B must have collateral_return");

    // raw_cbor must be populated after deserialization
    assert!(
        cr.raw_cbor.is_some(),
        "collateral_return.raw_cbor must be Some after deserialization"
    );

    // Decode with pallas to get reference collateral_return bytes
    let pallas_tx =
        MultiEraTx::decode_for_era(pallas_traverse::Era::Conway, &cbor).expect("pallas decode");
    let pallas_cr_bytes = pallas_tx
        .collateral_return()
        .expect("pallas collateral_return")
        .encode();

    // Direct re-encode must match pallas
    let direct = encode_transaction_output(cr);
    assert_eq!(
        direct,
        pallas_cr_bytes,
        "collateral_return direct re-encode differs from pallas\n\
         expected: {}\n\
         got:      {}",
        hex::encode(&pallas_cr_bytes),
        hex::encode(&direct),
    );

    // After LSM round-trip, re-encode must still match
    let restored = lsm_round_trip(cr);
    let after_lsm = encode_transaction_output(&restored);
    assert_eq!(
        after_lsm,
        pallas_cr_bytes,
        "collateral_return post-LSM re-encode differs from pallas\n\
         expected: {}\n\
         got:      {}",
        hex::encode(&pallas_cr_bytes),
        hex::encode(&after_lsm),
    );
}
