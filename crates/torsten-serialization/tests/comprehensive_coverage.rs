//! Comprehensive CBOR serialization coverage tests.
//!
//! This file targets code paths that were not covered by the existing unit tests
//! in `cbor.rs`, `encode/mod.rs`, and other sub-modules.  Each test group is
//! documented with the exact function or branch it exercises.
//!
//! Coverage goals
//! --------------
//! - `encode_uint`: 8-byte (> 4 GiB) path
//! - `encode_int`: two-byte / four-byte / eight-byte negative paths + boundaries
//! - `encode_bytes`: empty, exactly-24-byte, 256-byte, 65536-byte boundaries
//! - `encode_text`: empty, 24-byte, 256-byte boundaries
//! - `encode_array_header` / `encode_map_header`: 65536-entry path
//! - `encode_bool`, `encode_null`
//! - `encode_tag`: small, medium (< 256), large (< 65536), huge (< 2^32)
//! - `encode_hash28` (28-byte wire format)
//! - `decode_hash32`: short-bytestring path, error paths
//! - `encode_metadatum`: nested List / nested Map
//! - `encode_plutus_data`: Map variant, nested Constr
//! - `encode_tx_input`: index > 0
//! - `encode_native_script`: all six variants
//! - `encode_redeemer_tag`: all six tags
//! - `encode_vkey_witness` / `encode_bootstrap_witness`
//! - `encode_operational_cert` / `encode_vrf_result` / `encode_protocol_version`
//! - `encode_block_header` (with real KES sig bytes)
//! - `compute_block_body_hash` with non-empty transactions
//! - `encode_relay`: all three relay variants
//! - `encode_credential` (both key and script)
//! - `encode_anchor`, `encode_rational`
//! - `encode_certificate`: full range of certificate types
//! - `encode_drep`: key and script hash variants
//! - `encode_voter`: all three voter types
//! - `encode_gov_action`: HardForkInitiation, UpdateCommittee, NewConstitution
//! - `encode_voting_procedures` map (multi-voter)
//! - `encode_transaction_output`: legacy format, inline datum (with and without raw CBOR)
//! - `encode_auxiliary_data`: with scripts (tag 259 path)
//! - `encode_transaction_body`: all optional fields present
//! - `encode_mint` with positive and negative quantities
//! - `encode_multi_asset` field ordering
//! - `encode_protocol_param_update`: all remaining key ranges (0–30)
//! - `SerializationError`: display strings and `From<minicbor::decode::Error>`
//! - `encode_point`: slot boundary values
//! - `decode_transaction`: unknown era id returns error

use std::collections::BTreeMap;
use torsten_primitives::address::{Address, EnterpriseAddress};
use torsten_primitives::block::{Block, BlockHeader, OperationalCert, ProtocolVersion, VrfOutput};
use torsten_primitives::credentials::Credential;
use torsten_primitives::era::Era;
use torsten_primitives::hash::{Hash28, Hash32};
use torsten_primitives::network::NetworkId;
use torsten_primitives::time::{BlockNo, SlotNo};
use torsten_primitives::transaction::*;
use torsten_primitives::value::{AssetName, Lovelace, Value};
use torsten_serialization::cbor::*;
use torsten_serialization::encode::*;
use torsten_serialization::{decode_transaction, SerializationError};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a minimal valid enterprise address for testing.
fn test_addr() -> Address {
    Address::Enterprise(EnterpriseAddress {
        network: NetworkId::Mainnet,
        payment: Credential::VerificationKey(Hash28::from_bytes([0u8; 28])),
    })
}

/// Build a minimal valid transaction body (3 required fields only).
fn minimal_body() -> TransactionBody {
    TransactionBody {
        inputs: vec![TransactionInput {
            transaction_id: Hash32::ZERO,
            index: 0,
        }],
        outputs: vec![TransactionOutput {
            address: test_addr(),
            value: Value::lovelace(1_000_000),
            datum: OutputDatum::None,
            script_ref: None,
            is_legacy: false,
            raw_cbor: None,
        }],
        fee: Lovelace(200_000),
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

/// Build a minimal empty witness set.
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

/// Build a minimal valid Transaction.
fn minimal_tx() -> Transaction {
    Transaction {
        hash: Hash32::ZERO,
        era: torsten_primitives::era::Era::Conway,
        body: minimal_body(),
        witness_set: empty_witness_set(),
        is_valid: true,
        auxiliary_data: None,
        raw_cbor: None,
        raw_body_cbor: None,
        raw_witness_cbor: None,
    }
}

/// Build a minimal BlockHeader for encode tests.
fn test_header() -> BlockHeader {
    BlockHeader {
        header_hash: Hash32::ZERO,
        prev_hash: Hash32::from_bytes([1u8; 32]),
        issuer_vkey: vec![0u8; 32],
        vrf_vkey: vec![0u8; 32],
        vrf_result: VrfOutput {
            output: vec![0u8; 64],
            proof: vec![0u8; 80],
        },
        nonce_vrf_output: vec![],
        nonce_vrf_proof: vec![],
        block_number: BlockNo(1),
        slot: SlotNo(1000),
        epoch_nonce: Hash32::ZERO,
        body_size: 256,
        body_hash: Hash32::ZERO,
        operational_cert: OperationalCert {
            hot_vkey: vec![0u8; 32],
            sequence_number: 0,
            kes_period: 100,
            sigma: vec![0u8; 64],
        },
        protocol_version: ProtocolVersion { major: 9, minor: 0 },
        kes_signature: vec![],
    }
}

// ===========================================================================
// encode_uint — boundary tests
// ===========================================================================

#[test]
fn test_encode_uint_boundary_23() {
    // 23 is the last value that fits in a 1-byte tiny uint
    assert_eq!(encode_uint(23), vec![0x17]);
}

#[test]
fn test_encode_uint_boundary_24_to_255() {
    // 24 triggers the 1-byte additional info path
    assert_eq!(encode_uint(24), vec![0x18, 0x18]);
    assert_eq!(encode_uint(255), vec![0x18, 0xFF]);
}

#[test]
fn test_encode_uint_boundary_256_to_65535() {
    assert_eq!(encode_uint(256), vec![0x19, 0x01, 0x00]);
    assert_eq!(encode_uint(65535), vec![0x19, 0xFF, 0xFF]);
}

#[test]
fn test_encode_uint_boundary_65536_to_4294967295() {
    assert_eq!(encode_uint(65536), vec![0x1A, 0x00, 0x01, 0x00, 0x00]);
    assert_eq!(
        encode_uint(4_294_967_295),
        vec![0x1A, 0xFF, 0xFF, 0xFF, 0xFF]
    );
}

#[test]
fn test_encode_uint_8_byte_path() {
    // Values >= 4_294_967_296 must use the 8-byte major-type path (0x1B)
    let v = 4_294_967_296u64;
    let encoded = encode_uint(v);
    assert_eq!(encoded[0], 0x1B);
    assert_eq!(encoded.len(), 9);
    assert_eq!(u64::from_be_bytes(encoded[1..].try_into().unwrap()), v);

    // max u64
    let max = u64::MAX;
    let enc_max = encode_uint(max);
    assert_eq!(enc_max[0], 0x1B);
    assert_eq!(u64::from_be_bytes(enc_max[1..].try_into().unwrap()), max);
}

// ===========================================================================
// encode_int — negative boundary tests
// ===========================================================================

#[test]
fn test_encode_int_zero() {
    assert_eq!(encode_int(0), vec![0x00]);
}

#[test]
fn test_encode_int_negative_1_to_24() {
    // -1 → 0x20, -24 → 0x37
    assert_eq!(encode_int(-1), vec![0x20]);
    assert_eq!(encode_int(-24), vec![0x37]);
}

#[test]
fn test_encode_int_negative_25_to_256() {
    // abs_val 24 → one extra byte (0x38 prefix)
    let enc = encode_int(-25);
    assert_eq!(enc[0], 0x38);
    assert_eq!(enc[1], 24); // abs_val - 1 = 24
}

#[test]
fn test_encode_int_negative_two_byte_boundary() {
    // abs_val = 255 (last value for 0x38 path)
    let enc = encode_int(-256);
    assert_eq!(enc[0], 0x38);
    assert_eq!(enc[1], 255);
}

#[test]
fn test_encode_int_negative_three_byte_boundary() {
    // abs_val = 256 → triggers 0x39 path
    let enc = encode_int(-257);
    assert_eq!(enc[0], 0x39);
    assert_eq!(u16::from_be_bytes([enc[1], enc[2]]), 256u16);
}

#[test]
fn test_encode_int_negative_five_byte_boundary() {
    // abs_val = 65536 → triggers 0x3A path
    let enc = encode_int(-65537);
    assert_eq!(enc[0], 0x3A);
    assert_eq!(u32::from_be_bytes([enc[1], enc[2], enc[3], enc[4]]), 65536);
}

#[test]
fn test_encode_int_negative_nine_byte_boundary() {
    // abs_val = 4_294_967_296 → triggers 0x3B path
    let v: i128 = -4_294_967_297;
    let enc = encode_int(v);
    assert_eq!(enc[0], 0x3B);
    let abs_val = (-1 - v) as u64;
    assert_eq!(u64::from_be_bytes(enc[1..].try_into().unwrap()), abs_val);
}

// ===========================================================================
// encode_bytes — boundary tests
// ===========================================================================

#[test]
fn test_encode_bytes_empty() {
    // Empty bytes → 0x40 (bstr(0))
    let enc = encode_bytes(&[]);
    assert_eq!(enc, vec![0x40]);
}

#[test]
fn test_encode_bytes_23_bytes() {
    // 23 bytes fit in 1-byte header
    let data = vec![0xAA; 23];
    let enc = encode_bytes(&data);
    assert_eq!(enc[0], 0x40 | 23u8);
    assert_eq!(&enc[1..], data.as_slice());
}

#[test]
fn test_encode_bytes_24_bytes_triggers_extra_byte() {
    // 24 bytes needs 0x58 header with 1-byte length
    let data = vec![0xBB; 24];
    let enc = encode_bytes(&data);
    assert_eq!(enc[0], 0x58);
    assert_eq!(enc[1], 24);
    assert_eq!(&enc[2..], data.as_slice());
}

#[test]
fn test_encode_bytes_255_bytes() {
    let data = vec![0xCC; 255];
    let enc = encode_bytes(&data);
    assert_eq!(enc[0], 0x58);
    assert_eq!(enc[1], 255);
    assert_eq!(enc.len(), 257);
}

#[test]
fn test_encode_bytes_256_bytes_triggers_two_byte_length() {
    let data = vec![0xDD; 256];
    let enc = encode_bytes(&data);
    assert_eq!(enc[0], 0x59);
    assert_eq!(u16::from_be_bytes([enc[1], enc[2]]), 256u16);
    assert_eq!(enc.len(), 259);
}

#[test]
fn test_encode_bytes_65536_bytes_triggers_four_byte_length() {
    let data = vec![0u8; 65536];
    let enc = encode_bytes(&data);
    assert_eq!(enc[0], 0x5A);
    assert_eq!(
        u32::from_be_bytes([enc[1], enc[2], enc[3], enc[4]]),
        65536u32
    );
    assert_eq!(enc.len(), 5 + 65536);
}

// ===========================================================================
// encode_text — boundary tests
// ===========================================================================

#[test]
fn test_encode_text_empty() {
    let enc = encode_text("");
    assert_eq!(enc, vec![0x60]);
}

#[test]
fn test_encode_text_23_chars() {
    let text = "a".repeat(23);
    let enc = encode_text(&text);
    assert_eq!(enc[0], 0x60 | 23u8);
    assert_eq!(&enc[1..], text.as_bytes());
}

#[test]
fn test_encode_text_24_chars_triggers_extra_byte() {
    let text = "b".repeat(24);
    let enc = encode_text(&text);
    assert_eq!(enc[0], 0x78);
    assert_eq!(enc[1], 24);
    assert_eq!(&enc[2..], text.as_bytes());
}

#[test]
fn test_encode_text_256_chars_triggers_two_byte_length() {
    let text = "c".repeat(256);
    let enc = encode_text(&text);
    assert_eq!(enc[0], 0x79);
    assert_eq!(u16::from_be_bytes([enc[1], enc[2]]), 256u16);
}

// ===========================================================================
// encode_array_header / encode_map_header — large-count paths
// ===========================================================================

#[test]
fn test_encode_array_header_65536() {
    let enc = encode_array_header(65536);
    assert_eq!(enc[0], 0x9A);
    assert_eq!(
        u32::from_be_bytes([enc[1], enc[2], enc[3], enc[4]]),
        65536u32
    );
}

#[test]
fn test_encode_map_header_255() {
    let enc = encode_map_header(255);
    assert_eq!(enc[0], 0xB8);
    assert_eq!(enc[1], 255);
}

#[test]
fn test_encode_map_header_256() {
    let enc = encode_map_header(256);
    assert_eq!(enc[0], 0xB9);
    assert_eq!(u16::from_be_bytes([enc[1], enc[2]]), 256u16);
}

#[test]
fn test_encode_map_header_65536() {
    let enc = encode_map_header(65536);
    assert_eq!(enc[0], 0xBA);
    assert_eq!(
        u32::from_be_bytes([enc[1], enc[2], enc[3], enc[4]]),
        65536u32
    );
}

// ===========================================================================
// encode_bool / encode_null / encode_tag
// ===========================================================================

#[test]
fn test_encode_bool_true() {
    assert_eq!(encode_bool(true), vec![0xF5]);
}

#[test]
fn test_encode_bool_false() {
    assert_eq!(encode_bool(false), vec![0xF4]);
}

#[test]
fn test_encode_null() {
    assert_eq!(encode_null(), vec![0xF6]);
}

#[test]
fn test_encode_tag_small() {
    // Tags 0..=23 fit in 1 byte (0xC0 | tag)
    assert_eq!(encode_tag(0), vec![0xC0]);
    assert_eq!(encode_tag(23), vec![0xD7]);
}

#[test]
fn test_encode_tag_one_extra_byte() {
    // Tag 24..=255 → 0xD8 <tag>
    let enc = encode_tag(24);
    assert_eq!(enc, vec![0xD8, 24]);

    let enc255 = encode_tag(255);
    assert_eq!(enc255, vec![0xD8, 255]);
}

#[test]
fn test_encode_tag_two_extra_bytes() {
    // Tag 256..=65535 → 0xD9 <hi> <lo>
    let enc = encode_tag(256);
    assert_eq!(enc[0], 0xD9);
    assert_eq!(u16::from_be_bytes([enc[1], enc[2]]), 256u16);
}

#[test]
fn test_encode_tag_four_extra_bytes() {
    // Tag 65536..=2^32-1 → 0xDA <4 bytes>
    let enc = encode_tag(65536);
    assert_eq!(enc[0], 0xDA);
    assert_eq!(
        u32::from_be_bytes([enc[1], enc[2], enc[3], enc[4]]),
        65536u32
    );
}

#[test]
fn test_encode_tag_258_set_tag() {
    // Tag 258 is the Cardano canonical set tag
    let enc = encode_tag(258);
    assert_eq!(enc[0], 0xD9);
    assert_eq!(u16::from_be_bytes([enc[1], enc[2]]), 258u16);
}

#[test]
fn test_encode_tag_30_rational_tag() {
    // Tag 30 is used for rational numbers (fits in 1-byte additional)
    let enc = encode_tag(30);
    assert_eq!(enc, vec![0xD8, 30]);
}

// ===========================================================================
// encode_hash28 — correct wire format
// ===========================================================================

#[test]
fn test_encode_hash28_structure() {
    let h = Hash28::from_bytes([0xAB; 28]);
    let enc = encode_hash28(&h);
    // Header: 0x58 0x1C (bstr with 1-byte length = 28)
    assert_eq!(enc[0], 0x58);
    assert_eq!(enc[1], 28);
    assert_eq!(enc.len(), 30);
    assert_eq!(&enc[2..], h.as_bytes());
}

#[test]
fn test_encode_hash28_all_zeros() {
    let enc = encode_hash28(&Hash28::from_bytes([0u8; 28]));
    assert_eq!(enc[2..], [0u8; 28]);
}

// ===========================================================================
// decode_hash32 — short-bytestring path and error paths
// ===========================================================================

#[test]
fn test_decode_hash32_short_bstr_path() {
    // The short-bytestring path uses the 0x40..0x5F major type range.
    // A 32-byte bytestring fits exactly in the range 0x40 | 32 = 0x60 — but
    // 0x60 is a text string prefix.  In practice Cardano never uses the 1-byte
    // compact form for 32-byte hashes (it always uses 0x58 0x20), so we only
    // exercise the branch via a hand-crafted buffer where the length byte is
    // embedded in the first byte (0x40 | len).
    //
    // Note: 0x40 | 32 = 0x60 which is the CBOR text-string major type; the
    // `decode_hash32` branch only fires when the byte is in 0x40..0x5F.  The
    // largest length that fits is 31 (0x5F = 0x40 | 31).  32-byte hashes
    // therefore *cannot* be encoded in this compact form.  We verify the error
    // path instead when the compact length != 32.
    let mut buf = vec![0x5E]; // 0x40 | 30 → 30-byte bstr
    buf.extend_from_slice(&[0u8; 30]);
    let result = decode_hash32(&buf);
    assert!(
        result.is_err(),
        "compact bstr with length != 32 should return Err"
    );
}

#[test]
fn test_decode_hash32_short_buf_returns_error() {
    // Buffer too short (< 2 bytes)
    assert!(decode_hash32(&[]).is_err());
    assert!(decode_hash32(&[0x58]).is_err());
}

#[test]
fn test_decode_hash32_wrong_length_returns_error() {
    // 0x58 header with length 16 instead of 32
    let mut buf = vec![0x58, 16];
    buf.extend_from_slice(&[0u8; 16]);
    assert!(decode_hash32(&buf).is_err());
}

#[test]
fn test_decode_hash32_wrong_major_type_returns_error() {
    // Integer type (0x01) is not a bytestring
    let result = decode_hash32(&[0x01]);
    assert!(result.is_err());
}

#[test]
fn test_decode_hash32_roundtrip() {
    let original = Hash32::from_bytes([0xDE; 32]);
    let encoded = encode_hash32(&original);
    let (decoded, consumed) = decode_hash32(&encoded).unwrap();
    assert_eq!(decoded, original);
    assert_eq!(consumed, 34); // 2-byte header + 32 bytes
}

#[test]
fn test_decode_hash32_with_trailing_bytes() {
    let original = Hash32::from_bytes([0x42; 32]);
    let mut encoded = encode_hash32(&original);
    encoded.extend_from_slice(&[0xFF, 0xFF]); // trailing garbage
    let (decoded, consumed) = decode_hash32(&encoded).unwrap();
    assert_eq!(decoded, original);
    assert_eq!(consumed, 34); // only 34 bytes consumed, not the trailing bytes
}

// ===========================================================================
// encode_point — boundary slot values
// ===========================================================================

#[test]
fn test_encode_point_max_slot() {
    let point = torsten_primitives::block::Point::Specific(SlotNo(u64::MAX), Hash32::ZERO);
    let enc = encode_point(&point);
    assert_eq!(enc[0], 0x82); // array of 2
                              // Slot u64::MAX uses the 8-byte path
    assert_eq!(enc[1], 0x1B);
    let slot_val = u64::from_be_bytes(enc[2..10].try_into().unwrap());
    assert_eq!(slot_val, u64::MAX);
}

#[test]
fn test_encode_point_slot_zero() {
    let point = torsten_primitives::block::Point::Specific(SlotNo(0), Hash32::ZERO);
    let enc = encode_point(&point);
    assert_eq!(enc[0], 0x82);
    assert_eq!(enc[1], 0x00); // uint 0 is 1 byte
}

// ===========================================================================
// encode_metadatum — nested / recursive variants
// ===========================================================================

#[test]
fn test_encode_metadatum_nested_list() {
    let inner = TransactionMetadatum::List(vec![
        TransactionMetadatum::Int(1),
        TransactionMetadatum::Int(2),
    ]);
    let outer = TransactionMetadatum::List(vec![inner]);
    let enc = encode_metadatum(&outer);
    // outer array(1), inner array(2), 1, 2
    assert_eq!(enc[0], 0x81); // outer: array(1)
    assert_eq!(enc[1], 0x82); // inner: array(2)
    assert_eq!(enc[2], 0x01);
    assert_eq!(enc[3], 0x02);
}

#[test]
fn test_encode_metadatum_bytes() {
    let m = TransactionMetadatum::Bytes(vec![0xDE, 0xAD, 0xBE, 0xEF]);
    let enc = encode_metadatum(&m);
    assert_eq!(enc[0], 0x44); // bstr(4)
    assert_eq!(&enc[1..], &[0xDE, 0xAD, 0xBE, 0xEF]);
}

#[test]
fn test_encode_metadatum_nested_map() {
    let meta = TransactionMetadatum::Map(vec![
        (
            TransactionMetadatum::Int(0),
            TransactionMetadatum::Text("version".to_string()),
        ),
        (
            TransactionMetadatum::Int(1),
            TransactionMetadatum::Bytes(vec![0xFF]),
        ),
    ]);
    let enc = encode_metadatum(&meta);
    assert_eq!(enc[0], 0xA2); // map(2)
    assert_eq!(enc[1], 0x00); // key: int 0
}

#[test]
fn test_encode_metadatum_large_negative_int() {
    // -1000 → encode_int(-1000)
    let m = TransactionMetadatum::Int(-1000);
    let enc = encode_metadatum(&m);
    let reference = encode_int(-1000);
    assert_eq!(enc, reference);
}

// ===========================================================================
// encode_plutus_data — Map variant and nested Constr
// ===========================================================================

#[test]
fn test_encode_plutus_data_map_variant() {
    let data = PlutusData::Map(vec![
        (PlutusData::Integer(1), PlutusData::Bytes(vec![0xAA])),
        (PlutusData::Integer(2), PlutusData::Bytes(vec![0xBB])),
    ]);
    let enc = encode_plutus_data(&data);
    assert_eq!(enc[0], 0xA2); // map(2)
    assert_eq!(enc[1], 0x01); // key: int 1
    assert_eq!(enc[2], 0x41); // bstr(1)
    assert_eq!(enc[3], 0xAA);
}

#[test]
fn test_encode_plutus_data_empty_map() {
    let data = PlutusData::Map(vec![]);
    let enc = encode_plutus_data(&data);
    assert_eq!(enc, vec![0xA0]); // empty map
}

#[test]
fn test_encode_plutus_data_nested_constr() {
    // Constructor 1 containing constructor 0 with integer 42
    let inner = PlutusData::Constr(0, vec![PlutusData::Integer(42)]);
    let outer = PlutusData::Constr(1, vec![inner]);
    let enc = encode_plutus_data(&outer);
    // Outer: tag 122 (121 + 1), array(1)
    assert_eq!(enc[0], 0xD8);
    assert_eq!(enc[1], 122); // tag 122
    assert_eq!(enc[2], 0x81); // array(1)
                              // Inner: tag 121 (121 + 0), array(1), int 42
    assert_eq!(enc[3], 0xD8);
    assert_eq!(enc[4], 121);
    assert_eq!(enc[5], 0x81); // array(1)
    assert_eq!(enc[6], 0x18);
    assert_eq!(enc[7], 42);
}

#[test]
fn test_encode_plutus_data_constructor_6_is_tag_127() {
    // The last constructor in the 121..=127 range
    let data = PlutusData::Constr(6, vec![]);
    let enc = encode_plutus_data(&data);
    assert_eq!(enc[0], 0xD8);
    assert_eq!(enc[1], 127); // 121 + 6
    assert_eq!(enc[2], 0x80); // empty array
}

#[test]
fn test_encode_plutus_data_constructor_127_is_tag_1400() {
    // Constructor 127 maps to tag 1280 + (127 - 7) = 1400
    let data = PlutusData::Constr(127, vec![]);
    let enc = encode_plutus_data(&data);
    assert_eq!(enc[0], 0xD9); // 2-byte tag
    let tag_val = u16::from_be_bytes([enc[1], enc[2]]);
    assert_eq!(tag_val, 1280 + 120); // 1400
}

// ===========================================================================
// encode_tx_input — non-zero index
// ===========================================================================

#[test]
fn test_encode_tx_input_nonzero_index() {
    let input = TransactionInput {
        transaction_id: Hash32::from_bytes([0xAB; 32]),
        index: 7,
    };
    let enc = encode_tx_input(&input);
    // array(2), hash, index=7
    assert_eq!(enc[0], 0x82);
    // After the 34-byte hash: uint 7
    assert_eq!(enc[34 + 1], 0x07);
}

// ===========================================================================
// encode_native_script — all six variants
// ===========================================================================

#[test]
fn test_encode_native_script_any() {
    let script = NativeScript::ScriptAny(vec![
        NativeScript::ScriptPubkey(Hash32::ZERO),
        NativeScript::ScriptPubkey(Hash32::from_bytes([1u8; 32])),
    ]);
    let enc = encode_native_script(&script);
    assert_eq!(enc[0], 0x82); // array(2)
    assert_eq!(enc[1], 0x02); // type 2 (any)
    assert_eq!(enc[2], 0x82); // inner array(2)
}

#[test]
fn test_encode_native_script_n_of_k() {
    let script = NativeScript::ScriptNOfK(
        2,
        vec![
            NativeScript::ScriptPubkey(Hash32::ZERO),
            NativeScript::ScriptPubkey(Hash32::ZERO),
            NativeScript::ScriptPubkey(Hash32::ZERO),
        ],
    );
    let enc = encode_native_script(&script);
    assert_eq!(enc[0], 0x83); // array(3)
    assert_eq!(enc[1], 0x03); // type 3 (n_of_k)
    assert_eq!(enc[2], 0x02); // n = 2
    assert_eq!(enc[3], 0x83); // inner array(3)
}

#[test]
fn test_encode_native_script_invalid_before() {
    let script = NativeScript::InvalidBefore(SlotNo(1000));
    let enc = encode_native_script(&script);
    assert_eq!(enc[0], 0x82); // array(2)
    assert_eq!(enc[1], 0x04); // type 4 (invalid_before)
                              // slot 1000 = 0x19 0x03 0xE8
    assert_eq!(enc[2], 0x19);
    assert_eq!(u16::from_be_bytes([enc[3], enc[4]]), 1000u16);
}

#[test]
fn test_encode_native_script_invalid_hereafter() {
    let script = NativeScript::InvalidHereafter(SlotNo(99_999_999));
    let enc = encode_native_script(&script);
    assert_eq!(enc[0], 0x82);
    assert_eq!(enc[1], 0x05); // type 5 (invalid_hereafter)
}

#[test]
fn test_encode_native_script_empty_all() {
    // ScriptAll with zero children is valid CDDL
    let script = NativeScript::ScriptAll(vec![]);
    let enc = encode_native_script(&script);
    assert_eq!(enc[0], 0x82);
    assert_eq!(enc[1], 0x01);
    assert_eq!(enc[2], 0x80); // empty inner array
}

#[test]
fn test_encode_native_script_pubkey_truncates_to_28_bytes() {
    // ScriptPubkey stores Hash32 but emits only the first 28 bytes on wire.
    let hash = Hash32::from_bytes([0xAA; 32]);
    let enc = encode_native_script(&NativeScript::ScriptPubkey(hash));
    assert_eq!(enc.len(), 4 + 28); // array(2) + type(1) + bstr_header(2) + 28 bytes
    assert_eq!(enc[3], 0x1C); // length 28 = 0x1C
                              // All bytes should be 0xAA
    assert!(enc[4..].iter().all(|&b| b == 0xAA));
}

// ===========================================================================
// encode_redeemer — all tag values (exercised via encode_witness_set)
// ===========================================================================

/// Build a witness set with a single redeemer of the given tag, encode it,
/// and return the CBOR bytes so the caller can inspect the redeemer array.
fn witness_set_with_redeemer(tag: RedeemerTag, index: u32) -> Vec<u8> {
    let ws = TransactionWitnessSet {
        redeemers: vec![Redeemer {
            tag,
            index,
            data: PlutusData::Integer(0),
            ex_units: ExUnits { mem: 0, steps: 0 },
        }],
        ..empty_witness_set()
    };
    encode_witness_set(&ws)
}

#[test]
fn test_encode_redeemer_spend_tag() {
    // Conway map format:
    //   map(1) { 5: map(1) { array(2)[tag, index] => array(2)[data, ex_units] } }
    let enc = witness_set_with_redeemer(RedeemerTag::Spend, 0);
    let mut dec = minicbor::Decoder::new(&enc);
    dec.map().unwrap(); // outer witness_set map(1)
    assert_eq!(dec.u64().unwrap(), 5); // key 5 = redeemers
    dec.map().unwrap(); // redeemers map(1)
    dec.array().unwrap(); // key: array(2) [tag, index]
    assert_eq!(dec.u64().unwrap(), 0); // Spend = 0
}

#[test]
fn test_encode_redeemer_mint_tag() {
    // Conway map format: key array(2) starts with Mint tag = 1
    let enc = witness_set_with_redeemer(RedeemerTag::Mint, 0);
    let mut dec = minicbor::Decoder::new(&enc);
    dec.map().unwrap(); // witness_set map(1)
    dec.u64().unwrap(); // key 5
    dec.map().unwrap(); // redeemers map(1)
    dec.array().unwrap(); // key: array(2) [tag, index]
    assert_eq!(dec.u64().unwrap(), 1); // Mint = 1
}

#[test]
fn test_encode_redeemer_cert_tag() {
    // Conway map format: key array(2) starts with Cert tag = 2
    let enc = witness_set_with_redeemer(RedeemerTag::Cert, 0);
    let mut dec = minicbor::Decoder::new(&enc);
    dec.map().unwrap();
    dec.u64().unwrap();
    dec.map().unwrap(); // redeemers map(1)
    dec.array().unwrap(); // key: array(2) [tag, index]
    assert_eq!(dec.u64().unwrap(), 2); // Cert = 2
}

#[test]
fn test_encode_redeemer_reward_tag() {
    // Conway map format: key array(2) starts with Reward tag = 3
    let enc = witness_set_with_redeemer(RedeemerTag::Reward, 0);
    let mut dec = minicbor::Decoder::new(&enc);
    dec.map().unwrap();
    dec.u64().unwrap();
    dec.map().unwrap(); // redeemers map(1)
    dec.array().unwrap(); // key: array(2) [tag, index]
    assert_eq!(dec.u64().unwrap(), 3); // Reward = 3
}

#[test]
fn test_encode_redeemer_vote_tag() {
    // Conway map format: key array(2) starts with Vote tag = 4
    let enc = witness_set_with_redeemer(RedeemerTag::Vote, 0);
    let mut dec = minicbor::Decoder::new(&enc);
    dec.map().unwrap();
    dec.u64().unwrap();
    dec.map().unwrap(); // redeemers map(1)
    dec.array().unwrap(); // key: array(2) [tag, index]
    assert_eq!(dec.u64().unwrap(), 4); // Vote = 4
}

#[test]
fn test_encode_redeemer_propose_tag() {
    // Conway map format: key array(2) starts with Propose tag = 5
    let enc = witness_set_with_redeemer(RedeemerTag::Propose, 0);
    let mut dec = minicbor::Decoder::new(&enc);
    dec.map().unwrap();
    dec.u64().unwrap();
    dec.map().unwrap(); // redeemers map(1)
    dec.array().unwrap(); // key: array(2) [tag, index]
    assert_eq!(dec.u64().unwrap(), 5); // Propose = 5
}

#[test]
fn test_encode_redeemer_ex_units_encoding() {
    // Conway map format:
    //   map(1) { 5: map(1) { array(2)[tag, index] => array(2)[data, ex_units] } }
    // Key:   array(2) [ Spend=0, index=3 ]
    // Value: array(2) [ data, array(2)[mem, steps] ]
    let ws = TransactionWitnessSet {
        redeemers: vec![Redeemer {
            tag: RedeemerTag::Spend,
            index: 3,
            data: PlutusData::Bytes(vec![]),
            ex_units: ExUnits {
                mem: 1_000_000,
                steps: 500_000_000,
            },
        }],
        ..empty_witness_set()
    };
    let enc = encode_witness_set(&ws);
    let mut dec = minicbor::Decoder::new(&enc);
    dec.map().unwrap(); // witness_set map(1)
    dec.u64().unwrap(); // key 5 = redeemers
    dec.map().unwrap(); // redeemers map(1)
                        // Key: array(2) [tag, index]
    let key_arr = dec.array().unwrap().unwrap();
    assert_eq!(key_arr, 2);
    assert_eq!(dec.u64().unwrap(), 0); // Spend = 0
    assert_eq!(dec.u64().unwrap(), 3); // index = 3
                                       // Value: array(2) [data, ex_units_array]
    let val_arr = dec.array().unwrap().unwrap();
    assert_eq!(val_arr, 2);
    let _ = dec.bytes().unwrap(); // empty bytes datum
    let ex_arr = dec.array().unwrap().unwrap();
    assert_eq!(ex_arr, 2);
    assert_eq!(dec.u64().unwrap(), 1_000_000);
    assert_eq!(dec.u64().unwrap(), 500_000_000);
}

// ===========================================================================
// encode_vkey_witness / encode_bootstrap_witness
// ===========================================================================

#[test]
fn test_encode_vkey_witness_structure() {
    let ws = TransactionWitnessSet {
        vkey_witnesses: vec![VKeyWitness {
            vkey: vec![0xAA; 32],
            signature: vec![0xBB; 64],
        }],
        ..empty_witness_set()
    };
    let enc = encode_witness_set(&ws);
    // map(1) { 0: array(1) [ array(2) [bstr(32), bstr(64)] ] }
    assert_eq!(enc[0], 0xA1); // map(1)
    assert_eq!(enc[1], 0x00); // key 0
    assert_eq!(enc[2], 0x81); // array(1)
    assert_eq!(enc[3], 0x82); // array(2) for the single vkey witness
    assert_eq!(enc[4], 0x58); // bstr with 1-byte length prefix
    assert_eq!(enc[5], 32); // 32-byte vkey
}

#[test]
fn test_encode_bootstrap_witness_structure() {
    let ws = TransactionWitnessSet {
        bootstrap_witnesses: vec![BootstrapWitness {
            vkey: vec![0x11; 32],
            signature: vec![0x22; 64],
            chain_code: vec![0x33; 32],
            attributes: vec![0x44; 4],
        }],
        ..empty_witness_set()
    };
    let enc = encode_witness_set(&ws);
    // map(1) { 2: array(1) [ array(4) [...] ] }
    assert_eq!(enc[0], 0xA1); // map(1)
    assert_eq!(enc[1], 0x02); // key 2 (bootstrap)
    assert_eq!(enc[2], 0x81); // array(1)
    assert_eq!(enc[3], 0x84); // array(4) for the bootstrap witness
}

// ===========================================================================
// encode_operational_cert / encode_vrf_result / encode_protocol_version
// ===========================================================================

#[test]
fn test_encode_operational_cert_structure() {
    let cert = OperationalCert {
        hot_vkey: vec![0xAA; 32],
        sequence_number: 7,
        kes_period: 300,
        sigma: vec![0xBB; 64],
    };
    let enc = encode_operational_cert(&cert);
    let mut dec = minicbor::Decoder::new(&enc);
    let arr = dec.array().unwrap().unwrap();
    assert_eq!(arr, 4);
    assert_eq!(dec.bytes().unwrap().len(), 32); // hot_vkey
    assert_eq!(dec.u64().unwrap(), 7); // sequence_number
    assert_eq!(dec.u64().unwrap(), 300); // kes_period
    assert_eq!(dec.bytes().unwrap().len(), 64); // sigma
}

#[test]
fn test_encode_vrf_result_structure() {
    let vrf = VrfOutput {
        output: vec![0x11; 64],
        proof: vec![0x22; 80],
    };
    let enc = encode_vrf_result(&vrf);
    let mut dec = minicbor::Decoder::new(&enc);
    let arr = dec.array().unwrap().unwrap();
    assert_eq!(arr, 2);
    assert_eq!(dec.bytes().unwrap().len(), 64);
    assert_eq!(dec.bytes().unwrap().len(), 80);
}

#[test]
fn test_encode_protocol_version_structure() {
    let pv = ProtocolVersion {
        major: 10,
        minor: 1,
    };
    let enc = encode_protocol_version(&pv);
    let mut dec = minicbor::Decoder::new(&enc);
    let arr = dec.array().unwrap().unwrap();
    assert_eq!(arr, 2);
    assert_eq!(dec.u64().unwrap(), 10);
    assert_eq!(dec.u64().unwrap(), 1);
}

#[test]
fn test_encode_block_header_with_kes_sig() {
    let header = test_header();
    let kes_sig = vec![0xCC; 448];
    let enc = encode_block_header(&header, &kes_sig);
    let mut dec = minicbor::Decoder::new(&enc);
    let arr = dec.array().unwrap().unwrap();
    assert_eq!(arr, 2); // [header_body, kes_sig]
                        // Skip header_body (it's an array)
    let _inner_arr = dec.array().unwrap().unwrap();
    // Skip all 10 fields of the header body
    for _ in 0..10 {
        dec.skip().unwrap();
    }
    // Now read the KES signature
    let sig_bytes = dec.bytes().unwrap();
    assert_eq!(sig_bytes.len(), 448);
    assert!(sig_bytes.iter().all(|&b| b == 0xCC));
}

// ===========================================================================
// compute_block_body_hash — with non-empty transactions
// ===========================================================================

#[test]
fn test_compute_block_body_hash_single_tx() {
    let tx = minimal_tx();
    let hash1 = compute_block_body_hash(std::slice::from_ref(&tx));
    let hash2 = compute_block_body_hash(std::slice::from_ref(&tx));
    // Hash must be deterministic
    assert_eq!(hash1, hash2);
    assert_ne!(hash1, Hash32::ZERO);
}

#[test]
fn test_compute_block_body_hash_different_for_different_fee() {
    let mut tx1 = minimal_tx();
    let mut tx2 = minimal_tx();
    tx1.body.fee = Lovelace(100_000);
    tx2.body.fee = Lovelace(200_000);
    let hash1 = compute_block_body_hash(&[tx1]);
    let hash2 = compute_block_body_hash(&[tx2]);
    assert_ne!(hash1, hash2);
}

#[test]
fn test_compute_block_body_hash_invalid_tx_affects_hash() {
    let valid_tx = minimal_tx();
    let mut invalid_tx = minimal_tx();
    invalid_tx.is_valid = false;
    let hash_valid = compute_block_body_hash(std::slice::from_ref(&valid_tx));
    let hash_invalid = compute_block_body_hash(&[invalid_tx]);
    // The invalid-tx list hash (h4) changes, so the combined hash differs
    assert_ne!(hash_valid, hash_invalid);
}

#[test]
fn test_compute_block_body_hash_empty_vs_one_tx() {
    let hash_empty = compute_block_body_hash(&[]);
    let hash_one = compute_block_body_hash(&[minimal_tx()]);
    assert_ne!(hash_empty, hash_one);
}

// ===========================================================================
// encode_relay — all three variants
// ===========================================================================

#[test]
fn test_encode_relay_single_host_addr_all_fields() {
    let relay = Relay::SingleHostAddr {
        port: Some(3001),
        ipv4: Some([127, 0, 0, 1]),
        ipv6: None,
    };
    let enc = encode_certificate(&Certificate::PoolRegistration(PoolParams {
        operator: Hash28::from_bytes([0x01; 28]),
        vrf_keyhash: Hash32::from_bytes([0x02; 32]),
        pledge: Lovelace(1),
        cost: Lovelace(1),
        margin: Rational {
            numerator: 1,
            denominator: 10,
        },
        reward_account: vec![0xE0; 29],
        pool_owners: vec![],
        relays: vec![relay],
        pool_metadata: None,
    }));
    // array(10) for PoolRegistration
    assert_eq!(enc[0], 0x8A);
}

#[test]
fn test_encode_relay_single_host_name() {
    let relay = Relay::SingleHostName {
        port: Some(3001),
        dns_name: "relay.example.com".to_string(),
    };
    let enc = encode_certificate(&Certificate::PoolRegistration(PoolParams {
        operator: Hash28::from_bytes([0x01; 28]),
        vrf_keyhash: Hash32::from_bytes([0x02; 32]),
        pledge: Lovelace(1),
        cost: Lovelace(1),
        margin: Rational {
            numerator: 1,
            denominator: 10,
        },
        reward_account: vec![0xE0; 29],
        pool_owners: vec![],
        relays: vec![relay],
        pool_metadata: None,
    }));
    assert_eq!(enc[0], 0x8A); // PoolRegistration
}

#[test]
fn test_encode_relay_multi_host_name() {
    let relay = Relay::MultiHostName {
        dns_name: "multi.example.com".to_string(),
    };
    // Verify relay encoding directly through the pool params path
    let cert = Certificate::PoolRegistration(PoolParams {
        operator: Hash28::from_bytes([0x01; 28]),
        vrf_keyhash: Hash32::from_bytes([0x02; 32]),
        pledge: Lovelace(1),
        cost: Lovelace(1),
        margin: Rational {
            numerator: 1,
            denominator: 10,
        },
        reward_account: vec![0xE0; 29],
        pool_owners: vec![],
        relays: vec![relay],
        pool_metadata: None,
    });
    let enc = encode_certificate(&cert);
    assert_eq!(enc[0], 0x8A);
}

// ===========================================================================
// encode_credential — both types
// ===========================================================================

#[test]
fn test_encode_credential_verification_key() {
    let cert =
        Certificate::StakeRegistration(Credential::VerificationKey(Hash28::from_bytes([0xAA; 28])));
    let enc = encode_certificate(&cert);
    let mut dec = minicbor::Decoder::new(&enc);
    dec.array().unwrap(); // outer array(2)
    dec.u64().unwrap(); // tag 0
                        // Credential: array(2) [0, hash]
    let cred_arr = dec.array().unwrap().unwrap();
    assert_eq!(cred_arr, 2);
    assert_eq!(dec.u64().unwrap(), 0); // VerificationKey = 0
}

#[test]
fn test_encode_credential_script() {
    let cert = Certificate::StakeDelegation {
        credential: Credential::Script(Hash28::from_bytes([0xBB; 28])),
        pool_hash: Hash28::from_bytes([0xCC; 28]),
    };
    let enc = encode_certificate(&cert);
    let mut dec = minicbor::Decoder::new(&enc);
    dec.array().unwrap(); // outer array(3)
    dec.u64().unwrap(); // tag 2
                        // Credential: array(2) [1, hash]
    let cred_arr = dec.array().unwrap().unwrap();
    assert_eq!(cred_arr, 2);
    assert_eq!(dec.u64().unwrap(), 1); // Script = 1
}

// ===========================================================================
// encode_anchor / encode_rational
// ===========================================================================

#[test]
fn test_encode_anchor_structure() {
    let cert = Certificate::CommitteeColdResign {
        cold_credential: Credential::VerificationKey(Hash28::from_bytes([0u8; 28])),
        anchor: Some(Anchor {
            url: "https://example.com/meta.json".to_string(),
            data_hash: Hash32::from_bytes([0xAA; 32]),
        }),
    };
    let enc = encode_certificate(&cert);
    // array(3) [15, cred, anchor]
    let mut dec = minicbor::Decoder::new(&enc);
    let arr = dec.array().unwrap().unwrap();
    assert_eq!(arr, 3);
    dec.u64().unwrap(); // 15
    dec.skip().unwrap(); // cred
    let anchor_arr = dec.array().unwrap().unwrap();
    assert_eq!(anchor_arr, 2); // [url, data_hash]
    let url = dec.str().unwrap();
    assert_eq!(url, "https://example.com/meta.json");
}

#[test]
fn test_encode_rational_structure() {
    // Rational is encoded as tag(30) [numerator, denominator]
    let cert = Certificate::RegDRep {
        credential: Credential::VerificationKey(Hash28::from_bytes([0u8; 28])),
        deposit: Lovelace(500_000_000),
        anchor: None,
    };
    let _ = encode_certificate(&cert); // smoke test only

    // Check directly that pool registration with margin encodes a rational
    let cert2 = Certificate::PoolRegistration(PoolParams {
        operator: Hash28::from_bytes([0u8; 28]),
        vrf_keyhash: Hash32::ZERO,
        pledge: Lovelace(0),
        cost: Lovelace(340_000_000),
        margin: Rational {
            numerator: 5,
            denominator: 100,
        },
        reward_account: vec![0xE0; 29],
        pool_owners: vec![],
        relays: vec![],
        pool_metadata: None,
    });
    let enc2 = encode_certificate(&cert2);
    // Parse past the fixed fields to find the rational tag 30
    let mut dec = minicbor::Decoder::new(&enc2);
    dec.array().unwrap(); // outer
    dec.u64().unwrap(); // tag 3
    dec.bytes().unwrap(); // operator (28 bytes)
    dec.bytes().unwrap(); // vrf_keyhash (32 bytes)
    dec.u64().unwrap(); // pledge
    dec.u64().unwrap(); // cost
    let rational_tag = dec.tag().unwrap();
    assert_eq!(rational_tag, minicbor::data::Tag::new(30));
    let rat_arr = dec.array().unwrap().unwrap();
    assert_eq!(rat_arr, 2);
    assert_eq!(dec.u64().unwrap(), 5); // numerator
    assert_eq!(dec.u64().unwrap(), 100); // denominator
}

// ===========================================================================
// Certificate — remaining types
// ===========================================================================

#[test]
fn test_encode_certificate_conway_stake_deregistration() {
    let cert = Certificate::ConwayStakeDeregistration {
        credential: Credential::VerificationKey(Hash28::from_bytes([0xAA; 28])),
        refund: Lovelace(2_000_000),
    };
    let enc = encode_certificate(&cert);
    let mut dec = minicbor::Decoder::new(&enc);
    let arr = dec.array().unwrap().unwrap();
    assert_eq!(arr, 3);
    assert_eq!(dec.u64().unwrap(), 8); // Conway UnReg tag
    dec.skip().unwrap(); // credential
    assert_eq!(dec.u64().unwrap(), 2_000_000);
}

#[test]
fn test_encode_certificate_stake_vote_delegation() {
    let cert = Certificate::StakeVoteDelegation {
        credential: Credential::VerificationKey(Hash28::from_bytes([0xBB; 28])),
        pool_hash: Hash28::from_bytes([0xCC; 28]),
        drep: DRep::NoConfidence,
    };
    let enc = encode_certificate(&cert);
    let mut dec = minicbor::Decoder::new(&enc);
    let arr = dec.array().unwrap().unwrap();
    assert_eq!(arr, 4);
    assert_eq!(dec.u64().unwrap(), 10); // StakeVoteDelegation tag
}

#[test]
fn test_encode_certificate_reg_stake_deleg() {
    let cert = Certificate::RegStakeDeleg {
        credential: Credential::Script(Hash28::from_bytes([0xDD; 28])),
        pool_hash: Hash28::from_bytes([0xEE; 28]),
        deposit: Lovelace(1_000_000),
    };
    let enc = encode_certificate(&cert);
    let mut dec = minicbor::Decoder::new(&enc);
    let arr = dec.array().unwrap().unwrap();
    assert_eq!(arr, 4);
    assert_eq!(dec.u64().unwrap(), 11);
}

#[test]
fn test_encode_certificate_vote_reg_deleg() {
    let cert = Certificate::VoteRegDeleg {
        credential: Credential::VerificationKey(Hash28::from_bytes([0x01; 28])),
        drep: DRep::Abstain,
        deposit: Lovelace(2_000_000),
    };
    let enc = encode_certificate(&cert);
    let mut dec = minicbor::Decoder::new(&enc);
    let arr = dec.array().unwrap().unwrap();
    assert_eq!(arr, 4);
    assert_eq!(dec.u64().unwrap(), 12);
}

#[test]
fn test_encode_certificate_reg_stake_vote_deleg() {
    let cert = Certificate::RegStakeVoteDeleg {
        credential: Credential::VerificationKey(Hash28::from_bytes([0x02; 28])),
        pool_hash: Hash28::from_bytes([0x03; 28]),
        drep: DRep::KeyHash(Hash32::from_bytes([0x04; 32])),
        deposit: Lovelace(3_000_000),
    };
    let enc = encode_certificate(&cert);
    let mut dec = minicbor::Decoder::new(&enc);
    let arr = dec.array().unwrap().unwrap();
    assert_eq!(arr, 5);
    assert_eq!(dec.u64().unwrap(), 13);
}

#[test]
fn test_encode_certificate_committee_hot_auth() {
    let cert = Certificate::CommitteeHotAuth {
        cold_credential: Credential::VerificationKey(Hash28::from_bytes([0x10; 28])),
        hot_credential: Credential::Script(Hash28::from_bytes([0x20; 28])),
    };
    let enc = encode_certificate(&cert);
    let mut dec = minicbor::Decoder::new(&enc);
    let arr = dec.array().unwrap().unwrap();
    assert_eq!(arr, 3);
    assert_eq!(dec.u64().unwrap(), 14);
}

#[test]
fn test_encode_certificate_unregdrep() {
    let cert = Certificate::UnregDRep {
        credential: Credential::VerificationKey(Hash28::from_bytes([0x30; 28])),
        refund: Lovelace(500_000_000),
    };
    let enc = encode_certificate(&cert);
    let mut dec = minicbor::Decoder::new(&enc);
    let arr = dec.array().unwrap().unwrap();
    assert_eq!(arr, 3);
    assert_eq!(dec.u64().unwrap(), 17);
}

#[test]
fn test_encode_certificate_updatedrep_with_anchor() {
    let cert = Certificate::UpdateDRep {
        credential: Credential::VerificationKey(Hash28::from_bytes([0x40; 28])),
        anchor: Some(Anchor {
            url: "https://drep.example.com".to_string(),
            data_hash: Hash32::ZERO,
        }),
    };
    let enc = encode_certificate(&cert);
    let mut dec = minicbor::Decoder::new(&enc);
    let arr = dec.array().unwrap().unwrap();
    assert_eq!(arr, 3);
    assert_eq!(dec.u64().unwrap(), 18);
    dec.skip().unwrap(); // credential
    let anchor_arr = dec.array().unwrap().unwrap();
    assert_eq!(anchor_arr, 2); // anchor present
}

#[test]
fn test_encode_certificate_genesis_key_delegation() {
    let cert = Certificate::GenesisKeyDelegation {
        genesis_hash: Hash32::from_bytes([0xAA; 32]),
        genesis_delegate_hash: Hash32::from_bytes([0xBB; 32]),
        vrf_keyhash: Hash32::from_bytes([0xCC; 32]),
    };
    let enc = encode_certificate(&cert);
    let mut dec = minicbor::Decoder::new(&enc);
    let arr = dec.array().unwrap().unwrap();
    assert_eq!(arr, 4);
    assert_eq!(dec.u64().unwrap(), 5);
}

#[test]
fn test_encode_certificate_move_instantaneous_rewards_to_treasury() {
    let cert = Certificate::MoveInstantaneousRewards {
        source: MIRSource::Reserves,
        target: MIRTarget::OtherAccountingPot(100_000_000),
    };
    let enc = encode_certificate(&cert);
    let mut dec = minicbor::Decoder::new(&enc);
    let outer = dec.array().unwrap().unwrap();
    assert_eq!(outer, 2);
    assert_eq!(dec.u64().unwrap(), 6);
    let mir_arr = dec.array().unwrap().unwrap();
    assert_eq!(mir_arr, 2);
    assert_eq!(dec.u64().unwrap(), 0); // Reserves = 0
    assert_eq!(dec.u64().unwrap(), 100_000_000); // OtherAccountingPot coin
}

#[test]
fn test_encode_certificate_mir_to_stake_credentials() {
    let cred = Credential::VerificationKey(Hash28::from_bytes([0x99; 28]));
    let cert = Certificate::MoveInstantaneousRewards {
        source: MIRSource::Treasury,
        target: MIRTarget::StakeCredentials(vec![(cred, 50_000_000)]),
    };
    let enc = encode_certificate(&cert);
    let mut dec = minicbor::Decoder::new(&enc);
    dec.array().unwrap(); // outer array(2)
    assert_eq!(dec.u64().unwrap(), 6);
    let mir = dec.array().unwrap().unwrap();
    assert_eq!(mir, 2);
    assert_eq!(dec.u64().unwrap(), 1); // Treasury = 1
    let map_len = dec.map().unwrap().unwrap();
    assert_eq!(map_len, 1); // one credential
}

// ===========================================================================
// encode_drep — key and script hash variants
// ===========================================================================

#[test]
fn test_encode_drep_key_hash() {
    let cert = Certificate::VoteDelegation {
        credential: Credential::VerificationKey(Hash28::from_bytes([0u8; 28])),
        drep: DRep::KeyHash(Hash32::from_bytes([0xAA; 32])),
    };
    let enc = encode_certificate(&cert);
    let mut dec = minicbor::Decoder::new(&enc);
    dec.array().unwrap(); // outer
    dec.u64().unwrap(); // tag 9
    dec.skip().unwrap(); // credential
    let drep_arr = dec.array().unwrap().unwrap();
    assert_eq!(drep_arr, 2); // [0, hash]
    assert_eq!(dec.u64().unwrap(), 0); // KeyHash = 0
    assert_eq!(dec.bytes().unwrap().len(), 32);
}

#[test]
fn test_encode_drep_script_hash() {
    let cert = Certificate::VoteDelegation {
        credential: Credential::VerificationKey(Hash28::from_bytes([0u8; 28])),
        drep: DRep::ScriptHash(Hash28::from_bytes([0xBB; 28])),
    };
    let enc = encode_certificate(&cert);
    let mut dec = minicbor::Decoder::new(&enc);
    dec.array().unwrap();
    dec.u64().unwrap(); // 9
    dec.skip().unwrap();
    let drep_arr = dec.array().unwrap().unwrap();
    assert_eq!(drep_arr, 2); // [1, hash]
    assert_eq!(dec.u64().unwrap(), 1); // ScriptHash = 1
    assert_eq!(dec.bytes().unwrap().len(), 28);
}

// ===========================================================================
// encode_voter — all three voter types
// ===========================================================================

fn make_voting_procedures_encoded(voter: Voter, action_id: GovActionId) -> Vec<u8> {
    let mut procedures = BTreeMap::new();
    let mut votes = BTreeMap::new();
    votes.insert(
        action_id,
        VotingProcedure {
            vote: Vote::Yes,
            anchor: None,
        },
    );
    procedures.insert(voter, votes);
    let mut body = minimal_body();
    body.voting_procedures = procedures;
    encode_transaction_body(&body)
}

#[test]
fn test_encode_voter_constitutional_committee_vkey() {
    let voter =
        Voter::ConstitutionalCommittee(Credential::VerificationKey(Hash28::from_bytes([0xAA; 28])));
    let action_id = GovActionId {
        transaction_id: Hash32::ZERO,
        action_index: 0,
    };
    let enc = make_voting_procedures_encoded(voter, action_id);
    // Just verify it encodes without panic and is valid CBOR
    let mut dec = minicbor::Decoder::new(&enc);
    assert!(dec.map().is_ok());
}

#[test]
fn test_encode_voter_drep_script() {
    let voter = Voter::DRep(Credential::Script(Hash28::from_bytes([0xBB; 28])));
    let action_id = GovActionId {
        transaction_id: Hash32::from_bytes([0xCC; 32]),
        action_index: 1,
    };
    let enc = make_voting_procedures_encoded(voter, action_id);
    let mut dec = minicbor::Decoder::new(&enc);
    assert!(dec.map().is_ok());
}

#[test]
fn test_encode_voter_stake_pool() {
    let voter = Voter::StakePool(Hash32::from_bytes([0xDD; 32]));
    let action_id = GovActionId {
        transaction_id: Hash32::from_bytes([0xEE; 32]),
        action_index: 2,
    };
    let enc = make_voting_procedures_encoded(voter, action_id);
    let mut dec = minicbor::Decoder::new(&enc);
    assert!(dec.map().is_ok());
}

// ===========================================================================
// encode_gov_action — remaining variants (tested via encode_transaction_body)
// ===========================================================================

/// Encode a gov action through a proposal_procedure inside a transaction body,
/// then return the CBOR of the action itself by navigating to key 20.
fn encode_action_via_body(action: GovAction) -> Vec<u8> {
    let pp = ProposalProcedure {
        deposit: Lovelace(100_000_000_000),
        return_addr: vec![0xE0; 29],
        gov_action: action,
        anchor: Anchor {
            url: "https://example.com".to_string(),
            data_hash: Hash32::ZERO,
        },
    };
    let mut body = minimal_body();
    body.proposal_procedures = vec![pp];
    encode_transaction_body(&body)
}

#[test]
fn test_encode_gov_action_hard_fork_initiation() {
    let enc = encode_action_via_body(GovAction::HardForkInitiation {
        prev_action_id: None,
        protocol_version: (10, 0),
    });

    // Navigate to key 20 (proposal_procedures) in the map
    let mut dec = minicbor::Decoder::new(&enc);
    let map_len = dec.map().unwrap().unwrap();
    // Find key 20
    for _ in 0..map_len {
        let k = dec.u64().unwrap();
        if k == 20 {
            let pp_arr_len = dec.array().unwrap().unwrap();
            assert_eq!(pp_arr_len, 1);
            let pp_arr = dec.array().unwrap().unwrap();
            assert_eq!(pp_arr, 4); // [deposit, return_addr, gov_action, anchor]
            dec.skip().unwrap(); // deposit
            dec.skip().unwrap(); // return_addr
                                 // gov_action: array(3) [tag=1, prev_id=null, version=[10,0]]
            let ga_len = dec.array().unwrap().unwrap();
            assert_eq!(ga_len, 3);
            assert_eq!(dec.u64().unwrap(), 1); // HardForkInitiation tag = 1
            dec.null().unwrap(); // prev_id = null
            let version_arr = dec.array().unwrap().unwrap();
            assert_eq!(version_arr, 2);
            assert_eq!(dec.u64().unwrap(), 10);
            assert_eq!(dec.u64().unwrap(), 0);
            return;
        }
        dec.skip().unwrap(); // skip value
    }
    panic!("key 20 not found in encoded body");
}

#[test]
fn test_encode_gov_action_update_committee() {
    let cred_remove = Credential::VerificationKey(Hash28::from_bytes([0x01; 28]));
    let cred_add = Credential::Script(Hash28::from_bytes([0x02; 28]));
    let mut to_add = BTreeMap::new();
    to_add.insert(cred_add, 500u64);

    let enc = encode_action_via_body(GovAction::UpdateCommittee {
        prev_action_id: Some(GovActionId {
            transaction_id: Hash32::from_bytes([0x11; 32]),
            action_index: 0,
        }),
        members_to_remove: vec![cred_remove],
        members_to_add: to_add,
        threshold: Rational {
            numerator: 2,
            denominator: 3,
        },
    });

    let mut dec = minicbor::Decoder::new(&enc);
    let map_len = dec.map().unwrap().unwrap();
    for _ in 0..map_len {
        let k = dec.u64().unwrap();
        if k == 20 {
            dec.array().unwrap(); // outer array(1)
            dec.array().unwrap(); // pp array(4)
            dec.skip().unwrap(); // deposit
            dec.skip().unwrap(); // return_addr
                                 // gov_action: array(5) [tag, prev_id, remove, add, threshold]
            let ga_len = dec.array().unwrap().unwrap();
            assert_eq!(ga_len, 5);
            assert_eq!(dec.u64().unwrap(), 4); // UpdateCommittee tag = 4
            let prev_arr = dec.array().unwrap().unwrap();
            assert_eq!(prev_arr, 2); // prev_id is array(2)
            dec.skip().unwrap(); // tx_id
            dec.u64().unwrap(); // action_index
            let remove_len = dec.array().unwrap().unwrap();
            assert_eq!(remove_len, 1); // one credential to remove
            dec.skip().unwrap(); // skip the credential
            let add_len = dec.map().unwrap().unwrap();
            assert_eq!(add_len, 1); // one credential to add
            return;
        }
        dec.skip().unwrap();
    }
    panic!("key 20 not found");
}

#[test]
fn test_encode_gov_action_new_constitution_with_script() {
    let enc = encode_action_via_body(GovAction::NewConstitution {
        prev_action_id: None,
        constitution: Constitution {
            anchor: Anchor {
                url: "https://constitution.example.com".to_string(),
                data_hash: Hash32::from_bytes([0xAA; 32]),
            },
            script_hash: Some(Hash28::from_bytes([0xBB; 28])),
        },
    });

    let mut dec = minicbor::Decoder::new(&enc);
    let map_len = dec.map().unwrap().unwrap();
    for _ in 0..map_len {
        let k = dec.u64().unwrap();
        if k == 20 {
            dec.array().unwrap(); // array(1) of proposals
            dec.array().unwrap(); // pp: array(4)
            dec.skip().unwrap(); // deposit
            dec.skip().unwrap(); // return_addr
            let arr = dec.array().unwrap().unwrap();
            assert_eq!(arr, 3); // [tag, prev_id, constitution]
            assert_eq!(dec.u64().unwrap(), 5); // NewConstitution tag = 5
            dec.null().unwrap(); // prev_id = null
            let constitution_arr = dec.array().unwrap().unwrap();
            assert_eq!(constitution_arr, 2); // [anchor, script_hash_or_null]
            let anchor_arr = dec.array().unwrap().unwrap();
            assert_eq!(anchor_arr, 2); // anchor: [url, hash]
            let url = dec.str().unwrap();
            assert_eq!(url, "https://constitution.example.com");
            dec.skip().unwrap(); // anchor hash
            let script_hash_bytes = dec.bytes().unwrap();
            assert_eq!(script_hash_bytes.len(), 28); // script hash present
            return;
        }
        dec.skip().unwrap();
    }
    panic!("key 20 not found");
}

#[test]
fn test_encode_gov_action_new_constitution_no_script() {
    let enc = encode_action_via_body(GovAction::NewConstitution {
        prev_action_id: None,
        constitution: Constitution {
            anchor: Anchor {
                url: "https://constitution.example.com".to_string(),
                data_hash: Hash32::ZERO,
            },
            script_hash: None,
        },
    });

    let mut dec = minicbor::Decoder::new(&enc);
    let map_len = dec.map().unwrap().unwrap();
    for _ in 0..map_len {
        let k = dec.u64().unwrap();
        if k == 20 {
            dec.array().unwrap();
            dec.array().unwrap();
            dec.skip().unwrap();
            dec.skip().unwrap();
            dec.array().unwrap(); // gov_action
            dec.u64().unwrap(); // tag 5
            dec.null().unwrap(); // prev_id
            dec.array().unwrap(); // constitution array(2)
            dec.skip().unwrap(); // anchor
            dec.null().unwrap(); // script_hash = null
            return;
        }
        dec.skip().unwrap();
    }
    panic!("key 20 not found");
}

// ===========================================================================
// encode_transaction_output — legacy format and inline datum
// ===========================================================================

#[test]
fn test_encode_tx_output_legacy_with_datum_hash() {
    let output = TransactionOutput {
        address: test_addr(),
        value: Value::lovelace(2_000_000),
        datum: OutputDatum::DatumHash(Hash32::from_bytes([0xAA; 32])),
        script_ref: None,
        is_legacy: true,
        raw_cbor: None,
    };
    let enc = encode_transaction_output(&output);
    // Legacy format: array(3) [address, value, datum_hash]
    let mut dec = minicbor::Decoder::new(&enc);
    let arr = dec.array().unwrap().unwrap();
    assert_eq!(arr, 3);
    dec.skip().unwrap(); // address bytes
    dec.u64().unwrap(); // value (ADA only)
    let hash_bytes = dec.bytes().unwrap();
    assert_eq!(hash_bytes.len(), 32);
}

#[test]
fn test_encode_tx_output_legacy_without_datum() {
    let output = TransactionOutput {
        address: test_addr(),
        value: Value::lovelace(1_000_000),
        datum: OutputDatum::None,
        script_ref: None,
        is_legacy: true,
        raw_cbor: None,
    };
    let enc = encode_transaction_output(&output);
    // Legacy format without datum: array(2) [address, value]
    let mut dec = minicbor::Decoder::new(&enc);
    let arr = dec.array().unwrap().unwrap();
    assert_eq!(arr, 2);
}

#[test]
fn test_encode_tx_output_inline_datum_fresh_encode() {
    // InlineDatum without raw_cbor — falls back to encode_plutus_data
    let output = TransactionOutput {
        address: test_addr(),
        value: Value::lovelace(1_000_000),
        datum: OutputDatum::InlineDatum {
            data: PlutusData::Integer(42),
            raw_cbor: None,
        },
        script_ref: None,
        is_legacy: false,
        raw_cbor: None,
    };
    let enc = encode_transaction_output(&output);
    // Post-Alonzo map with keys 0,1,2: {0: addr, 1: value, 2: datum}
    let mut dec = minicbor::Decoder::new(&enc);
    let map_len = dec.map().unwrap().unwrap();
    assert_eq!(map_len, 3); // address + value + datum
    dec.u64().unwrap(); // key 0
    dec.skip().unwrap(); // address
    dec.u64().unwrap(); // key 1
    dec.skip().unwrap(); // value
    assert_eq!(dec.u64().unwrap(), 2); // key 2 = datum option
    let datum_arr = dec.array().unwrap().unwrap();
    assert_eq!(datum_arr, 2); // [1, tag(24)(encoded_data)]
    assert_eq!(dec.u64().unwrap(), 1); // inline datum indicator = 1
    let tag = dec.tag().unwrap();
    assert_eq!(tag, minicbor::data::Tag::new(24)); // CBOR-encoded data tag
}

#[test]
fn test_encode_tx_output_inline_datum_uses_raw_cbor_when_present() {
    // When raw_cbor is provided it must be used verbatim (not re-encoded)
    let raw = vec![0x18, 0x2A]; // encode_int(42) = [0x18, 0x2A]
    let output = TransactionOutput {
        address: test_addr(),
        value: Value::lovelace(500_000),
        datum: OutputDatum::InlineDatum {
            data: PlutusData::Integer(99), // intentionally different from raw_cbor
            raw_cbor: Some(raw.clone()),
        },
        script_ref: None,
        is_legacy: false,
        raw_cbor: None,
    };
    let enc = encode_transaction_output(&output);
    // Decode to find the embedded CBOR bytes
    let mut dec = minicbor::Decoder::new(&enc);
    dec.map().unwrap(); // map header
    dec.u64().unwrap(); // key 0
    dec.skip().unwrap();
    dec.u64().unwrap(); // key 1
    dec.skip().unwrap();
    dec.u64().unwrap(); // key 2
    dec.array().unwrap(); // datum array
    dec.u64().unwrap(); // 1 (inline)
    dec.tag().unwrap(); // tag 24
    let embedded = dec.bytes().unwrap();
    assert_eq!(embedded, raw.as_slice()); // must match raw_cbor exactly
}

#[test]
fn test_encode_tx_output_with_script_ref() {
    let output = TransactionOutput {
        address: test_addr(),
        value: Value::lovelace(1_000_000),
        datum: OutputDatum::None,
        script_ref: Some(ScriptRef::PlutusV2(vec![0xDE, 0xAD])),
        is_legacy: false,
        raw_cbor: None,
    };
    let enc = encode_transaction_output(&output);
    let mut dec = minicbor::Decoder::new(&enc);
    let map_len = dec.map().unwrap().unwrap();
    assert_eq!(map_len, 3); // address + value + script_ref
    dec.u64().unwrap(); // key 0
    dec.skip().unwrap();
    dec.u64().unwrap(); // key 1
    dec.skip().unwrap();
    assert_eq!(dec.u64().unwrap(), 3); // key 3 = script_ref
    let tag = dec.tag().unwrap();
    assert_eq!(tag, minicbor::data::Tag::new(24)); // tag 24 wraps script_ref
}

// ===========================================================================
// encode_auxiliary_data — with scripts (tag 259 path)
// ===========================================================================

#[test]
fn test_encode_auxiliary_data_with_native_scripts() {
    let mut metadata = BTreeMap::new();
    metadata.insert(721u64, TransactionMetadatum::Text("NFT".to_string()));

    let aux = AuxiliaryData {
        metadata,
        native_scripts: vec![NativeScript::InvalidBefore(SlotNo(1000))],
        plutus_v1_scripts: vec![],
        plutus_v2_scripts: vec![],
        plutus_v3_scripts: vec![],
        raw_cbor: None,
    };

    let enc = encode_auxiliary_data(&aux);
    // Should start with tag 259
    let mut dec = minicbor::Decoder::new(&enc);
    let tag = dec.tag().unwrap();
    assert_eq!(tag, minicbor::data::Tag::new(259));
    let map_len = dec.map().unwrap().unwrap();
    assert_eq!(map_len, 2); // metadata + native_scripts
    assert_eq!(dec.u64().unwrap(), 0); // key 0 = metadata
    dec.skip().unwrap();
    assert_eq!(dec.u64().unwrap(), 1); // key 1 = native_scripts
}

#[test]
fn test_encode_auxiliary_data_with_plutus_scripts() {
    let aux = AuxiliaryData {
        metadata: BTreeMap::new(),
        native_scripts: vec![],
        plutus_v1_scripts: vec![vec![0x01, 0x02]],
        plutus_v2_scripts: vec![vec![0x03, 0x04]],
        plutus_v3_scripts: vec![vec![0x05, 0x06]],
        raw_cbor: None,
    };

    let enc = encode_auxiliary_data(&aux);
    let mut dec = minicbor::Decoder::new(&enc);
    let tag = dec.tag().unwrap();
    assert_eq!(tag, minicbor::data::Tag::new(259));
    let map_len = dec.map().unwrap().unwrap();
    assert_eq!(map_len, 3); // v1 + v2 + v3 (no metadata)
    assert_eq!(dec.u64().unwrap(), 2); // key 2 = plutus_v1
    dec.skip().unwrap();
    assert_eq!(dec.u64().unwrap(), 3); // key 3 = plutus_v2
    dec.skip().unwrap();
    assert_eq!(dec.u64().unwrap(), 4); // key 4 = plutus_v3
}

// ===========================================================================
// encode_transaction_body — all optional fields
// ===========================================================================

#[test]
fn test_encode_transaction_body_with_all_optional_fields() {
    let mut body = minimal_body();
    body.ttl = Some(SlotNo(10_000));
    body.certificates = vec![Certificate::StakeRegistration(Credential::VerificationKey(
        Hash28::from_bytes([0u8; 28]),
    ))];
    body.withdrawals.insert(vec![0xE1; 29], Lovelace(500_000));
    body.auxiliary_data_hash = Some(Hash32::from_bytes([0xAA; 32]));
    body.validity_interval_start = Some(SlotNo(100));
    body.mint.insert(Hash28::from_bytes([0x01; 28]), {
        let mut m = BTreeMap::new();
        m.insert(AssetName(b"Token".to_vec()), 1i64);
        m
    });
    body.script_data_hash = Some(Hash32::from_bytes([0xBB; 32]));
    body.collateral = vec![TransactionInput {
        transaction_id: Hash32::ZERO,
        index: 0,
    }];
    body.required_signers = vec![Hash32::from_bytes([0xCC; 32])];
    body.network_id = Some(1);
    body.collateral_return = Some(TransactionOutput {
        address: test_addr(),
        value: Value::lovelace(5_000_000),
        datum: OutputDatum::None,
        script_ref: None,
        is_legacy: false,
        raw_cbor: None,
    });
    body.total_collateral = Some(Lovelace(500_000));
    body.reference_inputs = vec![TransactionInput {
        transaction_id: Hash32::ZERO,
        index: 1,
    }];

    let enc = encode_transaction_body(&body);
    let mut dec = minicbor::Decoder::new(&enc);
    let map_len = dec.map().unwrap().unwrap();
    // Required: 3 (inputs, outputs, fee) + optional count above = 3 + 13 = 16
    assert_eq!(map_len, 16);
}

// ===========================================================================
// encode_mint — positive and negative quantities
// ===========================================================================

#[test]
fn test_encode_transaction_body_mint_negative_quantity() {
    // Burning tokens uses negative mint quantities
    let mut body = minimal_body();
    body.mint.insert(Hash28::from_bytes([0x01; 28]), {
        let mut m = BTreeMap::new();
        m.insert(AssetName(b"Burn".to_vec()), -100i64);
        m
    });

    let enc = encode_transaction_body(&body);
    let mut dec = minicbor::Decoder::new(&enc);
    let map_len = dec.map().unwrap().unwrap();
    // 3 required + 1 mint
    assert_eq!(map_len, 4);

    // Navigate to key 9 (mint)
    dec.u64().unwrap(); // key 0
    dec.skip().unwrap();
    dec.u64().unwrap(); // key 1
    dec.skip().unwrap();
    dec.u64().unwrap(); // key 2
    dec.skip().unwrap();
    assert_eq!(dec.u64().unwrap(), 9); // mint key

    let ma_map = dec.map().unwrap().unwrap();
    assert_eq!(ma_map, 1); // one policy
    dec.skip().unwrap(); // policy_id bytes
    let inner = dec.map().unwrap().unwrap();
    assert_eq!(inner, 1); // one asset
    dec.skip().unwrap(); // asset name
    let qty = dec.i64().unwrap();
    assert_eq!(qty, -100); // negative quantity for burn
}

// ===========================================================================
// encode_multi_asset — deterministic ordering (BTreeMap)
// ===========================================================================

#[test]
fn test_encode_multi_asset_ordering_is_deterministic() {
    let policy_a = Hash28::from_bytes([0x01; 28]);
    let policy_b = Hash28::from_bytes([0x02; 28]);
    let mut v = Value::lovelace(1_000_000);
    v.multi_asset.insert(policy_b, {
        let mut m = BTreeMap::new();
        m.insert(AssetName(b"B".to_vec()), 2);
        m
    });
    v.multi_asset.insert(policy_a, {
        let mut m = BTreeMap::new();
        m.insert(AssetName(b"A".to_vec()), 1);
        m
    });

    let enc = encode_value(&v);
    // Decode and verify policy_a (0x01...) comes before policy_b (0x02...)
    let mut dec = minicbor::Decoder::new(&enc);
    dec.array().unwrap(); // [coin, multiasset]
    dec.u64().unwrap(); // coin
    dec.map().unwrap(); // 2-entry map
    let first_policy = dec.bytes().unwrap();
    assert_eq!(first_policy[0], 0x01, "policy_a should sort first");
}

// ===========================================================================
// Protocol parameter encoding — tested through the governance action path
// since encode_protocol_param_update is not exported publicly.
//
// The ParameterChange GovAction encodes a PPU inside a proposal_procedure,
// so we can inspect the PPU encoding by decoding the full body CBOR.
// ===========================================================================

/// Encode a ProtocolParamUpdate by embedding it in a ParameterChange proposal
/// inside a transaction body, then extract just the PPU bytes by navigating
/// the CBOR to key 20 → proposals[0] → gov_action → ppu.
fn encode_ppu_via_body(ppu: ProtocolParamUpdate) -> Vec<u8> {
    let pp = ProposalProcedure {
        deposit: Lovelace(100_000_000_000),
        return_addr: vec![0xE0; 29],
        gov_action: GovAction::ParameterChange {
            prev_action_id: None,
            protocol_param_update: Box::new(ppu),
            policy_hash: None,
        },
        anchor: Anchor {
            url: "https://example.com".to_string(),
            data_hash: Hash32::ZERO,
        },
    };
    let mut body = minimal_body();
    body.proposal_procedures = vec![pp];
    let body_enc = encode_transaction_body(&body);

    // Extract the PPU map bytes by decoding the body
    let mut dec = minicbor::Decoder::new(&body_enc);
    let map_len = dec.map().unwrap().unwrap();
    for _ in 0..map_len {
        let k = dec.u64().unwrap();
        if k == 20 {
            dec.array().unwrap(); // array(1) proposals
            dec.array().unwrap(); // pp array(4)
            dec.skip().unwrap(); // deposit
            dec.skip().unwrap(); // return_addr
                                 // gov_action: array(4) [tag=0, null, ppu, null]
            dec.array().unwrap();
            dec.u64().unwrap(); // tag 0
            dec.null().unwrap(); // prev_id null
                                 // The rest of the raw bytes from this position are the PPU map
            let ppu_start = dec.position();
            let ppu_bytes = body_enc[ppu_start..].to_vec();
            return ppu_bytes;
        }
        dec.skip().unwrap();
    }
    panic!("key 20 not found in body");
}

#[test]
fn test_encode_protocol_param_update_all_simple_numeric_keys() {
    let ppu = ProtocolParamUpdate {
        min_fee_a: Some(44),                        // key 0
        min_fee_b: Some(155381),                    // key 1
        max_block_body_size: Some(90112),           // key 2
        max_tx_size: Some(16384),                   // key 3
        max_block_header_size: Some(1100),          // key 4
        key_deposit: Some(Lovelace(2_000_000)),     // key 5
        pool_deposit: Some(Lovelace(500_000_000)),  // key 6
        e_max: Some(18),                            // key 7
        n_opt: Some(500),                           // key 8
        min_pool_cost: Some(Lovelace(340_000_000)), // key 13
        ada_per_utxo_byte: Some(Lovelace(4310)),    // key 14
        ..Default::default()
    };

    let ppu_bytes = encode_ppu_via_body(ppu);
    let mut dec = minicbor::Decoder::new(&ppu_bytes);
    let map_len = dec.map().unwrap().unwrap();
    assert_eq!(map_len, 11); // 11 fields set

    // Keys must be in ascending order
    let mut prev_key = 0u64;
    for _ in 0..11 {
        let k = dec.u64().unwrap();
        assert!(
            k >= prev_key,
            "keys must be in non-decreasing order: got {k} after {prev_key}"
        );
        prev_key = k;
        dec.skip().unwrap(); // skip value
    }
}

#[test]
fn test_encode_protocol_param_update_min_fee_ref_script_cost_per_byte() {
    // Key 30: the min_fee_ref_script_cost_per_byte field (u64, encoded as tag(30) rational)
    let ppu = ProtocolParamUpdate {
        min_fee_ref_script_cost_per_byte: Some(15), // stored as u64, wire-encoded as rational
        ..Default::default()
    };

    let ppu_bytes = encode_ppu_via_body(ppu);
    let mut dec = minicbor::Decoder::new(&ppu_bytes);
    let map_len = dec.map().unwrap().unwrap();
    assert_eq!(map_len, 1);
    assert_eq!(dec.u64().unwrap(), 30); // key 30
    let tag = dec.tag().unwrap();
    assert_eq!(tag, minicbor::data::Tag::new(30)); // rational tag
}

#[test]
fn test_encode_protocol_param_update_governance_group_keys() {
    // Keys 22 (pool voting thresholds) and 23 (drep voting thresholds)
    // are emitted when any threshold in the group is set.
    let ppu = ProtocolParamUpdate {
        pvt_motion_no_confidence: Some(Rational {
            numerator: 51,
            denominator: 100,
        }),
        dvt_no_confidence: Some(Rational {
            numerator: 67,
            denominator: 100,
        }),
        ..Default::default()
    };

    let ppu_bytes = encode_ppu_via_body(ppu);
    let mut dec = minicbor::Decoder::new(&ppu_bytes);
    let map_len = dec.map().unwrap().unwrap();
    assert_eq!(map_len, 2); // keys 22 and 23
    assert_eq!(dec.u64().unwrap(), 22); // pool_voting_thresholds group
    dec.skip().unwrap();
    assert_eq!(dec.u64().unwrap(), 23); // drep_voting_thresholds group
}

#[test]
fn test_encode_protocol_param_update_empty_roundtrip() {
    // Empty PPU should encode as empty map
    let ppu_bytes = encode_ppu_via_body(ProtocolParamUpdate::default());
    let mut dec = minicbor::Decoder::new(&ppu_bytes);
    let map_len = dec.map().unwrap().unwrap();
    assert_eq!(map_len, 0);
}

// ===========================================================================
// SerializationError — display and From impls
// ===========================================================================

#[test]
fn test_serialization_error_display_cbor_encode() {
    let e = SerializationError::CborEncode("test encode error".to_string());
    assert!(e.to_string().contains("CBOR encoding error"));
    assert!(e.to_string().contains("test encode error"));
}

#[test]
fn test_serialization_error_display_cbor_decode() {
    let e = SerializationError::CborDecode("bad data".to_string());
    assert!(e.to_string().contains("CBOR decoding error"));
    assert!(e.to_string().contains("bad data"));
}

#[test]
fn test_serialization_error_display_invalid_data() {
    let e = SerializationError::InvalidData("missing field".to_string());
    assert!(e.to_string().contains("Invalid data"));
    assert!(e.to_string().contains("missing field"));
}

#[test]
fn test_serialization_error_display_unexpected_tag() {
    let e = SerializationError::UnexpectedTag(999);
    assert!(e.to_string().contains("Unexpected CBOR tag"));
    assert!(e.to_string().contains("999"));
}

#[test]
fn test_serialization_error_display_missing_field() {
    let e = SerializationError::MissingField("transaction_id".to_string());
    assert!(e.to_string().contains("Missing required field"));
    assert!(e.to_string().contains("transaction_id"));
}

#[test]
fn test_serialization_error_display_invalid_length() {
    let e = SerializationError::InvalidLength {
        expected: 32,
        got: 28,
    };
    assert!(e.to_string().contains("Invalid length"));
    assert!(e.to_string().contains("32"));
    assert!(e.to_string().contains("28"));
}

#[test]
fn test_serialization_error_from_minicbor_decode_error() {
    // Trigger a real minicbor decode error and verify the conversion
    let bad_cbor = &[0xFF]; // CBOR break code outside indefinite structure
    let result = minicbor::decode::<u64>(bad_cbor);
    assert!(result.is_err());
    let err: SerializationError = result.unwrap_err().into();
    assert!(
        matches!(err, SerializationError::CborDecode(_)),
        "expected CborDecode variant"
    );
}

// ===========================================================================
// decode_transaction — unknown era error
// ===========================================================================

#[test]
fn test_decode_transaction_unknown_era_returns_error() {
    let dummy_cbor = vec![0x84, 0xA0, 0xA0, 0xF5, 0xF6]; // not real tx bytes
    let result = decode_transaction(99, &dummy_cbor);
    assert!(result.is_err(), "unknown era should return error");
    let err_str = format!("{}", result.unwrap_err());
    assert!(
        err_str.contains("unknown era id") || err_str.contains("CBOR decoding error"),
        "unexpected error message: {err_str}"
    );
}

#[test]
fn test_decode_transaction_invalid_cbor_shelley_returns_error() {
    let bad_cbor = vec![0xFF, 0xFF, 0xFF, 0xFF];
    let result = decode_transaction(1, &bad_cbor); // Shelley = 1
    assert!(result.is_err());
}

// ===========================================================================
// encode_block — era tags
// ===========================================================================

#[test]
fn test_encode_block_era_tags() {
    let eras = [
        (Era::Byron, 0u8),
        (Era::Shelley, 2),
        (Era::Allegra, 3),
        (Era::Mary, 4),
        (Era::Alonzo, 5),
        (Era::Babbage, 6),
        (Era::Conway, 7),
    ];

    for (era, expected_tag) in eras {
        let block = Block {
            header: test_header(),
            transactions: vec![],
            era,
            raw_cbor: None,
        };
        let enc = encode_block(&block, &[]);
        assert_eq!(enc[0], 0x82, "era {era:?} outer must be array(2)");
        // Second byte: era tag as uint
        if expected_tag < 24 {
            assert_eq!(
                enc[1], expected_tag,
                "era {era:?} should have tag {expected_tag}"
            );
        }
    }
}

#[test]
fn test_encode_block_invalid_tx_indices() {
    let mut tx_valid = minimal_tx();
    tx_valid.is_valid = true;
    let mut tx_invalid = minimal_tx();
    tx_invalid.is_valid = false;

    let block = Block {
        header: test_header(),
        transactions: vec![tx_valid, tx_invalid],
        era: Era::Conway,
        raw_cbor: None,
    };

    let enc = encode_block(&block, &[]);
    // Parse the block to find the invalid_txs array (5th element of inner array)
    let mut dec = minicbor::Decoder::new(&enc);
    dec.array().unwrap(); // outer array(2)
    dec.u64().unwrap(); // era tag 7
    let inner = dec.array().unwrap().unwrap();
    assert_eq!(inner, 5); // [header, bodies, witnesses, aux_data, invalid_txs]
    dec.skip().unwrap(); // header
    dec.skip().unwrap(); // tx_bodies
    dec.skip().unwrap(); // witness_sets
    dec.skip().unwrap(); // aux_data_map
    let invalid_arr = dec.array().unwrap().unwrap();
    assert_eq!(invalid_arr, 1); // one invalid tx at index 1
    assert_eq!(dec.u64().unwrap(), 1); // index of the invalid tx
}

// ===========================================================================
// Miscellaneous round-trip sanity checks
// ===========================================================================

#[test]
fn test_compute_transaction_hash_depends_on_inputs() {
    let mut body1 = minimal_body();
    let mut body2 = minimal_body();
    body1.inputs[0].transaction_id = Hash32::from_bytes([0xAA; 32]);
    body2.inputs[0].transaction_id = Hash32::from_bytes([0xBB; 32]);
    let hash1 = compute_transaction_hash(&body1);
    let hash2 = compute_transaction_hash(&body2);
    assert_ne!(hash1, hash2);
}

#[test]
fn test_compute_transaction_hash_depends_on_fee() {
    let mut body1 = minimal_body();
    let mut body2 = minimal_body();
    body1.fee = Lovelace(100_000);
    body2.fee = Lovelace(200_000);
    assert_ne!(
        compute_transaction_hash(&body1),
        compute_transaction_hash(&body2)
    );
}

#[test]
fn test_compute_transaction_hash_is_32_bytes() {
    let hash = compute_transaction_hash(&minimal_body());
    assert_ne!(hash, Hash32::ZERO);
    // Hash32 is always exactly 32 bytes
    assert_eq!(hash.as_bytes().len(), 32);
}

// ===========================================================================
// required_signers — must encode as 28-byte addr_keyhash per CDDL
// ===========================================================================

/// The CDDL spec defines:
///   required_signers = nonempty_set<addr_keyhash>
///   addr_keyhash      = hash28
///
/// Internally required_signers are stored as Hash32 (zero-padded from the
/// 28-byte pallas key hashes).  The encoder must strip the trailing 4 zero
/// bytes and emit exactly 28 bytes on the wire so that cardano-node and
/// cardano-cli can round-trip the transaction body without error.
#[test]
fn test_required_signers_encoded_as_28_bytes() {
    let mut body = minimal_body();
    // Build a padded Hash32 the same way pallas does: 28 real bytes followed
    // by 4 zero bytes.
    let mut raw = [0u8; 32];
    raw[..28].copy_from_slice(&[0xDE; 28]);
    // last 4 bytes remain 0x00 (zero-padding)
    body.required_signers = vec![Hash32::from_bytes(raw)];

    let enc = encode_transaction_body(&body);

    // Locate key 14 in the encoded map by scanning for the uint 14 (0x0E).
    // The map is definite-length, so navigate entry by entry.
    let mut dec = minicbor::Decoder::new(&enc);
    let map_len = dec.map().unwrap().unwrap() as usize;

    let mut found_key14 = false;
    for _ in 0..map_len {
        let key = dec.u64().unwrap();
        if key == 14 {
            found_key14 = true;
            // required_signers is encoded as an array of bstr values
            let arr_len = dec.array().unwrap().unwrap();
            assert_eq!(arr_len, 1, "expected exactly one required signer");
            let bytes = dec.bytes().unwrap();
            // Must be exactly 28 bytes — not 32
            assert_eq!(
                bytes.len(),
                28,
                "required_signer must be 28-byte addr_keyhash; got {} bytes",
                bytes.len()
            );
            // The first byte should be 0xDE (our sentinel value)
            assert_eq!(bytes[0], 0xDE);
            // The 28th byte (index 27) should also be 0xDE
            assert_eq!(bytes[27], 0xDE);
            break;
        } else {
            dec.skip().unwrap();
        }
    }
    assert!(
        found_key14,
        "key 14 (required_signers) not found in encoded body"
    );
}

/// Verify the raw CBOR header bytes for a required_signer entry: must be
/// `0x58 0x1C` (major type 2 / additional info 24 / length = 28 = 0x1C),
/// NOT `0x58 0x20` (length = 32) which was the previous incorrect encoding.
#[test]
fn test_required_signers_cbor_header_bytes() {
    let mut body = minimal_body();
    let mut raw = [0u8; 32];
    raw[..28].fill(0xAB);
    body.required_signers = vec![Hash32::from_bytes(raw)];

    let enc = encode_transaction_body(&body);

    // Scan the raw bytes for the CBOR bstr header of the required signer.
    // The sequence we're looking for is: 0x58 0x1C followed by 28 bytes of 0xAB.
    // The incorrect sequence would be: 0x58 0x20 followed by 28 bytes 0xAB + 4 bytes 0x00.
    let target_header: &[u8] = &[0x58, 28]; // 0x1C = 28
    let wrong_header: &[u8] = &[0x58, 32]; // 0x20 = 32

    let has_correct = enc.windows(target_header.len()).any(|w| w == target_header);

    assert!(
        has_correct,
        "encoded body must contain 0x58 0x1C header for 28-byte addr_keyhash"
    );
    // The 32-byte header (0x58 0x20) must NOT appear as a required_signer encoding.
    // Note: other fields (like tx hashes) legitimately use 0x58 0x20, so we check
    // that the 28-byte payload following our specific sentinel bytes is present.
    let _ = wrong_header; // documented for clarity; checked via sentinel pattern below
    let sentinel_with_correct: Vec<u8> = {
        let mut v = vec![0x58, 28];
        v.extend_from_slice(&[0xAB; 28]);
        v
    };
    let sentinel_with_wrong: Vec<u8> = {
        let mut v = vec![0x58, 32];
        v.extend_from_slice(&[0xAB; 28]);
        v.extend_from_slice(&[0x00; 4]);
        v
    };

    let correct_present = enc
        .windows(sentinel_with_correct.len())
        .any(|w| w == sentinel_with_correct.as_slice());
    let wrong_present = enc
        .windows(sentinel_with_wrong.len())
        .any(|w| w == sentinel_with_wrong.as_slice());

    assert!(
        correct_present,
        "28-byte addr_keyhash sentinel pattern not found in encoded body"
    );
    assert!(
        !wrong_present,
        "32-byte encoding of required_signer found — hash truncation to 28 bytes is broken"
    );
}
