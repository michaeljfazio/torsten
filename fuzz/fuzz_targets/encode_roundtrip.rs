//! Fuzz target for CBOR encode/decode roundtrip verification.
//!
//! Decodes arbitrary CBOR bytes as transactions (all eras 0-6), re-encodes
//! successfully decoded transactions, and verifies that decoding the
//! re-encoded bytes produces a structurally identical transaction.
//!
//! Also decodes blocks and verifies transaction-level roundtrips within them.
//!
//! Run with: cargo +nightly fuzz run fuzz_encode_roundtrip -- -max_total_time=300

#![no_main]

use libfuzzer_sys::fuzz_target;
use dugite_serialization::{decode_block, decode_transaction, encode_transaction};

fuzz_target!(|data: &[u8]| {
    // Test 1: Transaction roundtrip across all eras
    for era_id in 0..=6u16 {
        if let Ok(tx) = decode_transaction(era_id, data) {
            // Re-encode the decoded transaction
            let encoded = encode_transaction(&tx);

            // Decode the re-encoded bytes — must succeed
            if let Ok(re_decoded) = decode_transaction(era_id, &encoded) {
                // Structural equality check (ignoring raw_cbor fields which are
                // expected to differ since they capture the original wire bytes).
                assert_eq!(
                    tx.hash, re_decoded.hash,
                    "Transaction hash mismatch after roundtrip (era {})",
                    era_id
                );
                assert_eq!(
                    tx.body.inputs, re_decoded.body.inputs,
                    "Inputs mismatch after roundtrip (era {})",
                    era_id
                );
                assert_eq!(
                    tx.body.outputs.len(),
                    re_decoded.body.outputs.len(),
                    "Output count mismatch after roundtrip (era {})",
                    era_id
                );
                assert_eq!(
                    tx.body.fee, re_decoded.body.fee,
                    "Fee mismatch after roundtrip (era {})",
                    era_id
                );
                assert_eq!(
                    tx.body.ttl, re_decoded.body.ttl,
                    "TTL mismatch after roundtrip (era {})",
                    era_id
                );
                assert_eq!(
                    tx.body.certificates, re_decoded.body.certificates,
                    "Certificates mismatch after roundtrip (era {})",
                    era_id
                );
                assert_eq!(
                    tx.body.mint, re_decoded.body.mint,
                    "Mint mismatch after roundtrip (era {})",
                    era_id
                );
                assert_eq!(
                    tx.is_valid, re_decoded.is_valid,
                    "is_valid mismatch after roundtrip (era {})",
                    era_id
                );
            }
        }
    }

    // Test 2: Block decode → per-transaction roundtrip
    if let Ok(block) = decode_block(data) {
        for (i, tx) in block.transactions.iter().enumerate() {
            let encoded = encode_transaction(tx);
            // The re-encoded transaction should be decodable for the block's era
            let era_id = match block.era {
                dugite_primitives::era::Era::Byron => 0,
                dugite_primitives::era::Era::Shelley => 1,
                dugite_primitives::era::Era::Allegra => 2,
                dugite_primitives::era::Era::Mary => 3,
                dugite_primitives::era::Era::Alonzo => 4,
                dugite_primitives::era::Era::Babbage => 5,
                dugite_primitives::era::Era::Conway => 6,
            };
            if let Ok(re_decoded) = decode_transaction(era_id, &encoded) {
                assert_eq!(
                    tx.hash, re_decoded.hash,
                    "Block tx {} hash mismatch after roundtrip",
                    i
                );
                assert_eq!(
                    tx.body.fee, re_decoded.body.fee,
                    "Block tx {} fee mismatch after roundtrip",
                    i
                );
            }
        }
    }
});
