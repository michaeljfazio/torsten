use crate::cbor::*;
use dugite_primitives::block::{Block, BlockHeader, OperationalCert, ProtocolVersion, VrfOutput};
use dugite_primitives::hash::{blake2b_256, Hash32};
use dugite_primitives::transaction::Transaction;

use super::transaction::{
    encode_auxiliary_data, encode_transaction_body_for_era, encode_witness_set_for_era,
};

/// Encode an operational certificate: [hot_vkey, sequence_number, kes_period, sigma]
pub fn encode_operational_cert(cert: &OperationalCert) -> Vec<u8> {
    let mut buf = encode_array_header(4);
    buf.extend(encode_bytes(&cert.hot_vkey));
    buf.extend(encode_uint(cert.sequence_number));
    buf.extend(encode_uint(cert.kes_period));
    buf.extend(encode_bytes(&cert.sigma));
    buf
}

/// Encode a VRF result: [output, proof]
pub fn encode_vrf_result(vrf: &VrfOutput) -> Vec<u8> {
    let mut buf = encode_array_header(2);
    buf.extend(encode_bytes(&vrf.output));
    buf.extend(encode_bytes(&vrf.proof));
    buf
}

/// Encode a protocol version: [major, minor]
pub fn encode_protocol_version(pv: &ProtocolVersion) -> Vec<u8> {
    let mut buf = encode_array_header(2);
    buf.extend(encode_uint(pv.major));
    buf.extend(encode_uint(pv.minor));
    buf
}

/// Encode a block header body (the part that gets signed by KES).
///
/// [block_number, slot, prev_hash, issuer_vkey, vrf_vkey, vrf_result,
///  body_size, body_hash, operational_cert, protocol_version]
pub fn encode_block_header_body(header: &BlockHeader) -> Vec<u8> {
    let mut buf = encode_array_header(10);
    buf.extend(encode_uint(header.block_number.0));
    buf.extend(encode_uint(header.slot.0));
    buf.extend(encode_hash32(&header.prev_hash));
    buf.extend(encode_bytes(&header.issuer_vkey));
    buf.extend(encode_bytes(&header.vrf_vkey));
    buf.extend(encode_vrf_result(&header.vrf_result));
    buf.extend(encode_uint(header.body_size));
    buf.extend(encode_hash32(&header.body_hash));
    buf.extend(encode_operational_cert(&header.operational_cert));
    buf.extend(encode_protocol_version(&header.protocol_version));
    buf
}

/// Encode a complete block header: [header_body, body_signature]
///
/// The `kes_signature` parameter is the KES signature over the header body.
pub fn encode_block_header(header: &BlockHeader, kes_signature: &[u8]) -> Vec<u8> {
    let mut buf = encode_array_header(2);
    buf.extend(encode_block_header_body(header));
    buf.extend(encode_bytes(kes_signature));
    buf
}

/// Encode a complete Shelley+ era block.
///
/// Block = [storage_era_tag, [header, tx_bodies, tx_witness_sets, aux_data_map, invalid_txs]]
///
/// Uses **pallas/ImmutableDB storage era tags**, which differ from the HFC NS
/// indices used in the N2N ChainSync header wire format:
///
/// | Era     | Storage tag (this fn) | HFC NS index (ChainSync header) |
/// |---------|-----------------------|---------------------------------|
/// | Byron   | 0                     | 0                               |
/// | Shelley | 2                     | 1                               |
/// | Allegra | 3                     | 2                               |
/// | Mary    | 4                     | 3                               |
/// | Alonzo  | 5                     | 4                               |
/// | Babbage | 6                     | 5                               |
/// | Conway  | 7                     | 6                               |
///
/// When serving headers over N2N ChainSync, `extract_header_for_chainsync` in
/// `dugite-network` converts the storage tag to the correct HFC NS index.
pub fn encode_block(block: &Block, kes_signature: &[u8]) -> Vec<u8> {
    let era_tag = match block.era {
        dugite_primitives::era::Era::Byron => 0u64,
        dugite_primitives::era::Era::Shelley => 2,
        dugite_primitives::era::Era::Allegra => 3,
        dugite_primitives::era::Era::Mary => 4,
        dugite_primitives::era::Era::Alonzo => 5,
        dugite_primitives::era::Era::Babbage => 6,
        dugite_primitives::era::Era::Conway => 7,
    };

    // Outer array: [era_tag, block_content]
    let mut buf = encode_array_header(2);
    buf.extend(encode_uint(era_tag));

    // Block content: [header, tx_bodies, tx_witness_sets, aux_data_map, invalid_txs]
    buf.extend(encode_array_header(5));

    // Header
    buf.extend(encode_block_header(&block.header, kes_signature));

    // Transaction bodies — prefer preserved raw CBOR from the original
    // transaction to avoid re-serialization mismatches that would invalidate
    // witness signatures (the tx hash is blake2b-256 of the body CBOR).
    buf.extend(encode_array_header(block.transactions.len()));
    for tx in &block.transactions {
        if let Some(raw) = &tx.raw_body_cbor {
            buf.extend_from_slice(raw);
        } else {
            buf.extend(encode_transaction_body_for_era(&tx.body, tx.era));
        }
    }

    // Transaction witness sets — prefer preserved raw CBOR to avoid encoding
    // differences (map vs array redeemers, definite vs indefinite lengths).
    buf.extend(encode_array_header(block.transactions.len()));
    for tx in &block.transactions {
        if let Some(raw) = &tx.raw_witness_cbor {
            buf.extend_from_slice(raw);
        } else {
            buf.extend(encode_witness_set_for_era(&tx.witness_set, tx.era));
        }
    }

    // Auxiliary data map: {tx_index: aux_data}
    // Prefer preserved raw CBOR to avoid re-encoding mismatches that would
    // cause ConflictingMetadataHash failures (the auxiliary_data_hash in the
    // body was computed from the original CBOR).
    let aux_entries: Vec<_> = block
        .transactions
        .iter()
        .enumerate()
        .filter_map(|(i, tx)| tx.auxiliary_data.as_ref().map(|aux| (i, aux)))
        .collect();
    buf.extend(encode_map_header(aux_entries.len()));
    for (idx, aux) in &aux_entries {
        buf.extend(encode_uint(*idx as u64));
        if let Some(raw) = &aux.raw_cbor {
            buf.extend_from_slice(raw);
        } else {
            buf.extend(encode_auxiliary_data(aux));
        }
    }

    // Invalid transactions (indices of txs with is_valid=false)
    let invalid_indices: Vec<_> = block
        .transactions
        .iter()
        .enumerate()
        .filter(|(_, tx)| !tx.is_valid)
        .map(|(i, _)| i)
        .collect();
    buf.extend(encode_array_header(invalid_indices.len()));
    for idx in &invalid_indices {
        buf.extend(encode_uint(*idx as u64));
    }

    buf
}

/// Compute the block body hash using the Alonzo+ segregated witness structure.
///
/// Per Haskell cardano-ledger, the block body hash is:
///   blake2b_256(h1 || h2 || h3 || h4)
/// where:
///   h1 = blake2b_256(CBOR array of transaction bodies)
///   h2 = blake2b_256(CBOR array of witness sets)
///   h3 = blake2b_256(CBOR map of {tx_index: auxiliary_data})
///   h4 = blake2b_256(CBOR array of invalid tx indices)
// NOTE: `transactions` is intentionally `&[Transaction]` rather than `&[&Transaction]`
// so callers can pass `block.transactions.as_slice()` directly.
pub fn compute_block_body_hash(transactions: &[Transaction]) -> Hash32 {
    // 1. Transaction bodies — prefer preserved raw CBOR from the original
    // transaction to ensure the body hash matches what the witnesses signed.
    let mut bodies_cbor = encode_array_header(transactions.len());
    for tx in transactions {
        if let Some(raw) = &tx.raw_body_cbor {
            bodies_cbor.extend_from_slice(raw);
        } else {
            bodies_cbor.extend(encode_transaction_body_for_era(&tx.body, tx.era));
        }
    }
    let h1 = blake2b_256(&bodies_cbor);

    // 2. Transaction witness sets — prefer preserved raw CBOR.
    let mut wits_cbor = encode_array_header(transactions.len());
    for tx in transactions {
        if let Some(raw) = &tx.raw_witness_cbor {
            wits_cbor.extend_from_slice(raw);
        } else {
            wits_cbor.extend(encode_witness_set_for_era(&tx.witness_set, tx.era));
        }
    }
    let h2 = blake2b_256(&wits_cbor);

    // 3. Auxiliary data map: {tx_index: aux_data} — prefer preserved raw CBOR
    // to avoid ConflictingMetadataHash from re-encoding differences.
    let aux_entries: Vec<_> = transactions
        .iter()
        .enumerate()
        .filter_map(|(i, tx)| tx.auxiliary_data.as_ref().map(|aux| (i, aux)))
        .collect();
    let mut aux_cbor = encode_map_header(aux_entries.len());
    for (idx, aux) in &aux_entries {
        aux_cbor.extend(encode_uint(*idx as u64));
        if let Some(raw) = &aux.raw_cbor {
            aux_cbor.extend_from_slice(raw);
        } else {
            aux_cbor.extend(encode_auxiliary_data(aux));
        }
    }
    let h3 = blake2b_256(&aux_cbor);

    // 4. Invalid transaction indices (txs with is_valid=false)
    let invalid_indices: Vec<_> = transactions
        .iter()
        .enumerate()
        .filter(|(_, tx)| !tx.is_valid)
        .map(|(i, _)| i)
        .collect();
    let mut isvalid_cbor = encode_array_header(invalid_indices.len());
    for idx in &invalid_indices {
        isvalid_cbor.extend(encode_uint(*idx as u64));
    }
    let h4 = blake2b_256(&isvalid_cbor);

    // Combine: blake2b_256(h1 || h2 || h3 || h4)
    let mut combined = Vec::with_capacity(128);
    combined.extend_from_slice(h1.as_bytes());
    combined.extend_from_slice(h2.as_bytes());
    combined.extend_from_slice(h3.as_bytes());
    combined.extend_from_slice(h4.as_bytes());
    blake2b_256(&combined)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cbor::encode_array_header;
    use dugite_primitives::block::{Block, BlockHeader, OperationalCert, ProtocolVersion, VrfOutput};
    use dugite_primitives::era::Era;
    use dugite_primitives::hash::Hash32;
    use dugite_primitives::time::{BlockNo, SlotNo};
    use dugite_primitives::transaction::Transaction;

    // -----------------------------------------------------------------------
    // Helper: build a minimal BlockHeader for encoder tests.
    // All byte-vectors use recognisable fill bytes so mismatches are easy to spot.
    // -----------------------------------------------------------------------
    fn make_header() -> BlockHeader {
        BlockHeader {
            header_hash: Hash32::from_bytes([0xaa; 32]),
            prev_hash: Hash32::from_bytes([0xbb; 32]),
            issuer_vkey: vec![0x01; 32],
            vrf_vkey: vec![0x02; 32],
            vrf_result: VrfOutput {
                output: vec![0x03; 32],
                proof: vec![0x04; 80],
            },
            block_number: BlockNo(42),
            slot: SlotNo(1000),
            epoch_nonce: Hash32::ZERO,
            body_size: 512,
            body_hash: Hash32::from_bytes([0xcc; 32]),
            operational_cert: OperationalCert {
                hot_vkey: vec![0x05; 32],
                sequence_number: 7,
                kes_period: 3,
                sigma: vec![0x06; 64],
            },
            protocol_version: ProtocolVersion { major: 9, minor: 0 },
            kes_signature: vec![0x07; 448],
            nonce_vrf_output: vec![],
            nonce_vrf_proof: vec![],
        }
    }

    // -----------------------------------------------------------------------
    // Test: encode_operational_cert  →  array(4)
    // -----------------------------------------------------------------------

    #[test]
    fn test_encode_operational_cert_is_array4() {
        let cert = OperationalCert {
            hot_vkey: vec![0xde; 32],
            sequence_number: 5,
            kes_period: 10,
            sigma: vec![0xad; 64],
        };
        let encoded = encode_operational_cert(&cert);

        // First byte must be array(4) = 0x84
        assert_eq!(
            encoded[0], 0x84,
            "operational cert must start with CBOR array(4) header 0x84"
        );
    }

    #[test]
    fn test_encode_operational_cert_sequence_number_and_kes_period() {
        // Use small values that appear unambiguously as single-byte CBOR uints.
        let cert = OperationalCert {
            hot_vkey: vec![0u8; 0],
            sequence_number: 0,
            kes_period: 1,
            sigma: vec![],
        };
        let encoded = encode_operational_cert(&cert);
        // array(4) | bytes(0) | uint(0) | uint(1) | bytes(0)
        // 0x84 0x40 0x00 0x01 0x40
        assert_eq!(encoded, vec![0x84, 0x40, 0x00, 0x01, 0x40]);
    }

    // -----------------------------------------------------------------------
    // Test: encode_vrf_result  →  array(2)
    // -----------------------------------------------------------------------

    #[test]
    fn test_encode_vrf_result_is_array2() {
        let vrf = VrfOutput {
            output: vec![0xab; 32],
            proof: vec![0xcd; 80],
        };
        let encoded = encode_vrf_result(&vrf);

        // First byte must be array(2) = 0x82
        assert_eq!(
            encoded[0], 0x82,
            "VRF result must start with CBOR array(2) header 0x82"
        );
    }

    #[test]
    fn test_encode_vrf_result_empty() {
        // Empty output and proof — useful baseline.
        let vrf = VrfOutput {
            output: vec![],
            proof: vec![],
        };
        let encoded = encode_vrf_result(&vrf);
        // array(2) | bytes(0) | bytes(0)  →  0x82 0x40 0x40
        assert_eq!(encoded, vec![0x82, 0x40, 0x40]);
    }

    #[test]
    fn test_encode_vrf_result_contains_both_fields() {
        let output_bytes = vec![0x11; 3];
        let proof_bytes = vec![0x22; 5];
        let vrf = VrfOutput {
            output: output_bytes.clone(),
            proof: proof_bytes.clone(),
        };
        let encoded = encode_vrf_result(&vrf);

        // Manually reconstruct expected bytes:
        // array(2) + bytes(3) + <output> + bytes(5) + <proof>
        let mut expected = vec![0x82_u8];
        expected.push(0x43); // bstr len 3
        expected.extend_from_slice(&output_bytes);
        expected.push(0x45); // bstr len 5
        expected.extend_from_slice(&proof_bytes);
        assert_eq!(encoded, expected);
    }

    // -----------------------------------------------------------------------
    // Test: encode_protocol_version  →  array(2) with major/minor
    // -----------------------------------------------------------------------

    #[test]
    fn test_encode_protocol_version_is_array2() {
        let pv = ProtocolVersion { major: 9, minor: 0 };
        let encoded = encode_protocol_version(&pv);
        assert_eq!(
            encoded[0], 0x82,
            "protocol version must start with CBOR array(2) header 0x82"
        );
    }

    #[test]
    fn test_encode_protocol_version_major_minor_values() {
        let pv = ProtocolVersion { major: 9, minor: 2 };
        let encoded = encode_protocol_version(&pv);
        // array(2) | uint(9) | uint(2)  →  0x82 0x09 0x02
        assert_eq!(encoded, vec![0x82, 0x09, 0x02]);
    }

    #[test]
    fn test_encode_protocol_version_large_minor() {
        // minor = 300 requires 3 CBOR bytes (0x19 0x01 0x2c)
        let pv = ProtocolVersion {
            major: 7,
            minor: 300,
        };
        let encoded = encode_protocol_version(&pv);
        assert_eq!(encoded[0], 0x82);
        assert_eq!(encoded[1], 0x07); // major = 7
        assert_eq!(encoded[2], 0x19); // uint 2-byte follows
        assert_eq!(encoded[3..5], [0x01, 0x2c]); // 300 in big-endian
    }

    // -----------------------------------------------------------------------
    // Test: encode_block_header_body  →  array(10)
    // -----------------------------------------------------------------------

    #[test]
    fn test_encode_block_header_body_starts_with_array10() {
        let header = make_header();
        let encoded = encode_block_header_body(&header);
        // array(10) = 0x8a
        assert_eq!(
            encoded[0], 0x8a,
            "header body must start with CBOR array(10) header 0x8a"
        );
    }

    #[test]
    fn test_encode_block_header_body_block_number_at_index1() {
        // block_number = 42  →  single-byte CBOR uint 0x18 0x2a
        let header = make_header();
        let encoded = encode_block_header_body(&header);
        // byte 0 is 0x8a (array header); byte 1 starts block_number
        assert_eq!(encoded[1], 0x18, "block_number uint prefix");
        assert_eq!(encoded[2], 42, "block_number value");
    }

    #[test]
    fn test_encode_block_header_body_slot_after_block_number() {
        // slot = 1000  →  0x19 0x03 0xe8
        let header = make_header();
        let encoded = encode_block_header_body(&header);
        // skip array(10): 1 byte, then block_number (2 bytes) → offset 3
        assert_eq!(encoded[3], 0x19, "slot uint prefix");
        let slot_val = u16::from_be_bytes([encoded[4], encoded[5]]);
        assert_eq!(slot_val, 1000, "slot value");
    }

    // -----------------------------------------------------------------------
    // Test: encode_block_header  →  array(2): [header_body, kes_sig]
    // -----------------------------------------------------------------------

    #[test]
    fn test_encode_block_header_starts_with_array2() {
        let header = make_header();
        let kes_sig = vec![0xff; 448];
        let encoded = encode_block_header(&header, &kes_sig);
        assert_eq!(
            encoded[0], 0x82,
            "block header must start with CBOR array(2) header 0x82"
        );
    }

    #[test]
    fn test_encode_block_header_body_is_first_element() {
        let header = make_header();
        let kes_sig = vec![0xee; 448];
        let encoded = encode_block_header(&header, &kes_sig);

        // Second byte (index 1) should be the start of the header body, which
        // itself starts with array(10) = 0x8a.
        assert_eq!(
            encoded[1], 0x8a,
            "second element of block header must be the header body array(10)"
        );
    }

    #[test]
    fn test_encode_block_header_kes_sig_is_second_element() {
        let header = make_header();
        let kes_sig = vec![0xab; 3];
        let header_body_len = encode_block_header_body(&header).len();

        let encoded = encode_block_header(&header, &kes_sig);

        // After outer array(2) header (1 byte) and the full header body,
        // the next bytes must encode the KES signature as a CBOR byte string.
        let sig_offset = 1 + header_body_len;
        // bytes(3) → 0x43
        assert_eq!(
            encoded[sig_offset], 0x43,
            "KES signature must be encoded as bytes(3)"
        );
        assert_eq!(&encoded[sig_offset + 1..sig_offset + 4], &[0xab; 3]);
    }

    // -----------------------------------------------------------------------
    // Test: encode_block  →  era tags + outer array(2) + inner array(5)
    // -----------------------------------------------------------------------

    fn make_block(era: Era) -> Block {
        Block {
            header: make_header(),
            transactions: vec![],
            era,
            raw_cbor: None,
        }
    }

    /// Helper: decode the era tag from the encoded block (the uint immediately
    /// after the outer array(2) header byte).
    fn decode_era_tag(encoded: &[u8]) -> u64 {
        // encoded[0] = 0x82 (array(2))
        // encoded[1] starts the era tag uint
        if encoded[1] < 0x18 {
            // small uint (0–23) inline
            encoded[1] as u64
        } else if encoded[1] == 0x18 {
            encoded[2] as u64
        } else {
            panic!("unexpected era tag encoding");
        }
    }

    #[test]
    fn test_encode_block_outer_array2() {
        let block = make_block(Era::Conway);
        let kes_sig = vec![];
        let encoded = encode_block(&block, &kes_sig);
        assert_eq!(
            encoded[0], 0x82,
            "block must start with CBOR outer array(2) header 0x82"
        );
    }

    #[test]
    fn test_encode_block_era_tag_shelley() {
        let encoded = encode_block(&make_block(Era::Shelley), &[]);
        assert_eq!(decode_era_tag(&encoded), 2, "Shelley era tag must be 2");
    }

    #[test]
    fn test_encode_block_era_tag_allegra() {
        let encoded = encode_block(&make_block(Era::Allegra), &[]);
        assert_eq!(decode_era_tag(&encoded), 3, "Allegra era tag must be 3");
    }

    #[test]
    fn test_encode_block_era_tag_mary() {
        let encoded = encode_block(&make_block(Era::Mary), &[]);
        assert_eq!(decode_era_tag(&encoded), 4, "Mary era tag must be 4");
    }

    #[test]
    fn test_encode_block_era_tag_alonzo() {
        let encoded = encode_block(&make_block(Era::Alonzo), &[]);
        assert_eq!(decode_era_tag(&encoded), 5, "Alonzo era tag must be 5");
    }

    #[test]
    fn test_encode_block_era_tag_babbage() {
        let encoded = encode_block(&make_block(Era::Babbage), &[]);
        assert_eq!(decode_era_tag(&encoded), 6, "Babbage era tag must be 6");
    }

    #[test]
    fn test_encode_block_era_tag_conway() {
        let encoded = encode_block(&make_block(Era::Conway), &[]);
        assert_eq!(decode_era_tag(&encoded), 7, "Conway era tag must be 7");
    }

    #[test]
    fn test_encode_block_inner_array5() {
        let block = make_block(Era::Conway);
        let encoded = encode_block(&block, &[]);

        // outer array(2)=0x82 | era-tag uint(7)=0x07 | inner starts here
        // era tag 7 is a single-byte uint, so inner array starts at offset 2.
        assert_eq!(
            encoded[2], 0x85,
            "inner block content must be CBOR array(5) header 0x85"
        );
    }

    #[test]
    fn test_encode_block_inner_array5_header_body_at_offset3() {
        let block = make_block(Era::Shelley);
        let encoded = encode_block(&block, &[]);

        // outer array(2)=0x82 | era-tag uint(2)=0x02 | inner array(5)=0x85
        // | block_header array(2)=0x82 | ...
        // Offsets: 0 0x82, 1 0x02, 2 0x85, 3 block_header
        assert_eq!(
            encoded[3], 0x82,
            "first element of inner array must be the block header array(2)"
        );
    }

    // -----------------------------------------------------------------------
    // Test: compute_block_body_hash
    // -----------------------------------------------------------------------

    #[test]
    fn test_compute_block_body_hash_is_deterministic() {
        // Same input must always produce the same hash.
        let txs: Vec<Transaction> = vec![];
        let h1 = compute_block_body_hash(&txs);
        let h2 = compute_block_body_hash(&txs);
        assert_eq!(h1, h2, "body hash must be deterministic");
    }

    #[test]
    fn test_compute_block_body_hash_empty_txs_is_32_bytes() {
        let txs: Vec<Transaction> = vec![];
        let hash = compute_block_body_hash(&txs);
        // Hash32 is always 32 bytes.
        assert_eq!(hash.as_bytes().len(), 32);
    }

    #[test]
    fn test_compute_block_body_hash_differs_for_different_inputs() {
        // Empty vs. one valid transaction must yield different hashes.
        let empty: Vec<Transaction> = vec![];
        let one_tx = vec![Transaction::empty_with_hash(Hash32::ZERO)];

        let hash_empty = compute_block_body_hash(&empty);
        let hash_one = compute_block_body_hash(&one_tx);
        assert_ne!(
            hash_empty, hash_one,
            "body hash must differ when transactions differ"
        );
    }

    #[test]
    fn test_compute_block_body_hash_invalid_tx_index_affects_hash() {
        // A block with one valid tx vs. one invalid tx must produce different hashes
        // because the invalid-tx-indices component (h4) changes.
        let mut valid_tx = Transaction::empty_with_hash(Hash32::ZERO);
        valid_tx.is_valid = true;

        let mut invalid_tx = Transaction::empty_with_hash(Hash32::ZERO);
        invalid_tx.is_valid = false;

        let hash_valid = compute_block_body_hash(&[valid_tx]);
        let hash_invalid = compute_block_body_hash(&[invalid_tx]);
        assert_ne!(
            hash_valid, hash_invalid,
            "invalid tx index must change the body hash"
        );
    }

    #[test]
    fn test_compute_block_body_hash_known_empty_value() {
        // Regression guard: encode the exact CBOR structures for an empty block
        // body and verify the hash matches what we compute ourselves step-by-step.
        //
        // h1 = blake2b_256(CBOR array(0))  i.e. blake2b_256(0x80)
        // h2 = blake2b_256(CBOR array(0))
        // h3 = blake2b_256(CBOR map(0))    i.e. blake2b_256(0xa0)
        // h4 = blake2b_256(CBOR array(0))
        use crate::cbor::encode_map_header;
        use dugite_primitives::hash::blake2b_256;

        let h1 = blake2b_256(&encode_array_header(0));
        let h2 = blake2b_256(&encode_array_header(0));
        let h3 = blake2b_256(&encode_map_header(0));
        let h4 = blake2b_256(&encode_array_header(0));

        let mut combined = Vec::with_capacity(128);
        combined.extend_from_slice(h1.as_bytes());
        combined.extend_from_slice(h2.as_bytes());
        combined.extend_from_slice(h3.as_bytes());
        combined.extend_from_slice(h4.as_bytes());
        let expected = blake2b_256(&combined);

        let actual = compute_block_body_hash(&[]);
        assert_eq!(
            actual, expected,
            "empty block body hash must match step-by-step computation"
        );
    }
}
