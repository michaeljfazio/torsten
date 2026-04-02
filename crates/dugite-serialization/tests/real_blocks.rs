//! Tests that decode real block CBOR from the preview testnet.
//!
//! Each `.hex` file in `test_vectors/` contains the hex-encoded CBOR of a real block.
//! These tests verify that `decode_block()` produces correct results by cross-checking
//! against `pallas_traverse::MultiEraBlock`.

use dugite_primitives::era::Era;
use dugite_serialization::decode_block;
use pallas_traverse::MultiEraBlock as PallasBlock;

/// Load a test vector hex file and return raw CBOR bytes.
fn load_vector(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/tests/test_vectors/{}.hex",
        env!("CARGO_MANIFEST_DIR"),
        name
    );
    let hex_str = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read test vector {path}: {e}"));
    hex::decode(hex_str.trim()).unwrap_or_else(|e| panic!("Invalid hex in {path}: {e}"))
}

/// Decode with both dugite and pallas, compare key fields.
fn cross_check_block(name: &str, expected_era: Era) {
    let cbor = load_vector(name);

    // Decode with dugite
    let block = decode_block(&cbor).unwrap_or_else(|e| panic!("{name}: decode_block failed: {e}"));

    // Decode with pallas
    let pallas_block =
        PallasBlock::decode(&cbor).unwrap_or_else(|e| panic!("{name}: pallas decode failed: {e}"));

    // Era
    assert_eq!(block.era, expected_era, "{name}: era mismatch");

    // Slot
    let pallas_slot = pallas_block.slot();
    assert_eq!(block.header.slot.0, pallas_slot, "{name}: slot mismatch");

    // Block number
    let pallas_bn = pallas_block.number();
    assert_eq!(
        block.header.block_number.0, pallas_bn,
        "{name}: block_number mismatch"
    );

    // Block header hash
    let pallas_hash = pallas_block.hash().to_vec();
    assert_eq!(
        block.header.header_hash.as_bytes(),
        &pallas_hash[..],
        "{name}: header_hash mismatch"
    );

    // Transaction count
    let pallas_tx_count = pallas_block.tx_count();
    assert_eq!(
        block.transactions.len(),
        pallas_tx_count,
        "{name}: tx_count mismatch"
    );

    // Transaction hashes (if any)
    for (i, pallas_tx) in pallas_block.txs().iter().enumerate() {
        let pallas_tx_hash = pallas_tx.hash().to_vec();
        let dugite_tx_hash = block.transactions[i].hash.as_bytes();
        assert_eq!(
            dugite_tx_hash,
            &pallas_tx_hash[..],
            "{name}: tx[{i}] hash mismatch"
        );
    }
}

#[test]
fn test_shelley_block() {
    cross_check_block("shelley", Era::Shelley);
}

#[test]
fn test_mary_block() {
    cross_check_block("mary", Era::Mary);
}

#[test]
fn test_alonzo_block() {
    cross_check_block("alonzo", Era::Alonzo);
}

#[test]
fn test_babbage_block() {
    cross_check_block("babbage", Era::Babbage);
}

#[test]
fn test_conway_block() {
    cross_check_block("conway", Era::Conway);
}

#[test]
fn test_decode_block_invalid_cbor() {
    let bad_cbor = vec![0xff, 0xfe, 0xfd, 0xfc];
    assert!(decode_block(&bad_cbor).is_err());
}

#[test]
fn test_decode_block_empty() {
    assert!(decode_block(&[]).is_err());
}

#[test]
fn test_decode_block_truncated() {
    let cbor = load_vector("conway");
    // Truncate to half — should fail gracefully
    let truncated = &cbor[..cbor.len() / 2];
    assert!(decode_block(truncated).is_err());
}
