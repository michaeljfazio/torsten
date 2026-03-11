//! Property-based tests for CBOR serialization round-trips.
//!
//! Uses `proptest` to generate arbitrary inputs and verify that
//! encode-then-decode is the identity for all primitive CBOR types
//! and higher-level Cardano types.

use proptest::prelude::*;
use std::collections::BTreeMap;
use torsten_primitives::hash::{Hash28, Hash32};
use torsten_primitives::time::SlotNo;
use torsten_primitives::transaction::{PlutusData, TransactionInput, TransactionMetadatum};
use torsten_primitives::value::{AssetName, Lovelace, Value};
use torsten_serialization::cbor::*;
use torsten_serialization::encode::*;

// ---------------------------------------------------------------------------
// Proptest strategies for Cardano types
// ---------------------------------------------------------------------------

fn arb_hash32() -> impl Strategy<Value = Hash32> {
    prop::array::uniform32(any::<u8>()).prop_map(Hash32::from_bytes)
}

fn arb_hash28() -> impl Strategy<Value = Hash28> {
    prop::array::uniform28(any::<u8>()).prop_map(Hash28::from_bytes)
}

fn arb_asset_name() -> impl Strategy<Value = AssetName> {
    prop::collection::vec(any::<u8>(), 0..=32).prop_map(AssetName)
}

fn arb_value_ada_only() -> impl Strategy<Value = Value> {
    any::<u64>().prop_map(Value::lovelace)
}

fn arb_multi_asset() -> impl Strategy<Value = BTreeMap<Hash28, BTreeMap<AssetName, u64>>> {
    prop::collection::btree_map(
        arb_hash28(),
        prop::collection::btree_map(arb_asset_name(), any::<u64>(), 1..=3),
        1..=3,
    )
}

fn arb_value_multi_asset() -> impl Strategy<Value = Value> {
    (any::<u64>(), arb_multi_asset()).prop_map(|(coin, multi_asset)| Value {
        coin: Lovelace(coin),
        multi_asset,
    })
}

fn arb_value() -> impl Strategy<Value = Value> {
    prop_oneof![arb_value_ada_only(), arb_value_multi_asset(),]
}

fn arb_tx_input() -> impl Strategy<Value = TransactionInput> {
    (arb_hash32(), any::<u32>()).prop_map(|(transaction_id, index)| TransactionInput {
        transaction_id,
        index,
    })
}

/// Strategy for PlutusData with bounded recursion depth.
fn arb_plutus_data() -> impl Strategy<Value = PlutusData> {
    let leaf = prop_oneof![
        // Integer: use a range that fits in CBOR encoding (avoid i128 extremes)
        (-1_000_000_000i128..1_000_000_000i128).prop_map(PlutusData::Integer),
        prop::collection::vec(any::<u8>(), 0..=64).prop_map(PlutusData::Bytes),
    ];

    leaf.prop_recursive(
        3,  // depth
        32, // max nodes
        8,  // items per collection
        |inner| {
            prop_oneof![
                // List
                prop::collection::vec(inner.clone(), 0..=4).prop_map(PlutusData::List),
                // Map
                prop::collection::vec((inner.clone(), inner.clone()), 0..=3)
                    .prop_map(PlutusData::Map),
                // Constr (small constructors 0-6)
                (0..7u64, prop::collection::vec(inner.clone(), 0..=3))
                    .prop_map(|(tag, fields)| PlutusData::Constr(tag, fields)),
                // Constr (medium constructors 7-127)
                (7..128u64, prop::collection::vec(inner.clone(), 0..=2))
                    .prop_map(|(tag, fields)| PlutusData::Constr(tag, fields)),
                // Constr (large constructors >= 128 using tag 102)
                (128..256u64, prop::collection::vec(inner, 0..=2))
                    .prop_map(|(tag, fields)| PlutusData::Constr(tag, fields)),
            ]
        },
    )
}

/// Strategy for TransactionMetadatum with bounded recursion.
fn arb_metadatum() -> impl Strategy<Value = TransactionMetadatum> {
    let leaf = prop_oneof![
        (-1_000_000_000i128..1_000_000_000i128).prop_map(TransactionMetadatum::Int),
        prop::collection::vec(any::<u8>(), 0..=64).prop_map(TransactionMetadatum::Bytes),
        "[a-zA-Z0-9 ]{0,64}".prop_map(TransactionMetadatum::Text),
    ];

    leaf.prop_recursive(
        2,  // depth
        16, // max nodes
        4,  // items per collection
        |inner| {
            prop_oneof![
                prop::collection::vec(inner.clone(), 0..=3).prop_map(TransactionMetadatum::List),
                prop::collection::vec((inner.clone(), inner), 0..=3)
                    .prop_map(TransactionMetadatum::Map),
            ]
        },
    )
}

// ---------------------------------------------------------------------------
// Helper: decode CBOR unsigned integer
// ---------------------------------------------------------------------------

fn decode_cbor_uint(data: &[u8]) -> Option<(u64, usize)> {
    if data.is_empty() {
        return None;
    }
    let major = data[0] >> 5;
    if major != 0 {
        return None; // Not unsigned int
    }
    let additional = data[0] & 0x1f;
    match additional {
        0..=23 => Some((additional as u64, 1)),
        24 => {
            if data.len() < 2 {
                return None;
            }
            Some((data[1] as u64, 2))
        }
        25 => {
            if data.len() < 3 {
                return None;
            }
            Some((u16::from_be_bytes([data[1], data[2]]) as u64, 3))
        }
        26 => {
            if data.len() < 5 {
                return None;
            }
            Some((
                u32::from_be_bytes([data[1], data[2], data[3], data[4]]) as u64,
                5,
            ))
        }
        27 => {
            if data.len() < 9 {
                return None;
            }
            Some((
                u64::from_be_bytes([
                    data[1], data[2], data[3], data[4], data[5], data[6], data[7], data[8],
                ]),
                9,
            ))
        }
        _ => None,
    }
}

/// Decode a CBOR signed integer (major type 0 or 1).
fn decode_cbor_int(data: &[u8]) -> Option<(i128, usize)> {
    if data.is_empty() {
        return None;
    }
    let major = data[0] >> 5;
    match major {
        0 => {
            let (val, len) = decode_cbor_uint(data)?;
            Some((val as i128, len))
        }
        1 => {
            // Negative integer: value is -1 - additional
            let additional = data[0] & 0x1f;
            match additional {
                0..=23 => Some((-(additional as i128) - 1, 1)),
                24 => {
                    if data.len() < 2 {
                        return None;
                    }
                    Some((-(data[1] as i128) - 1, 2))
                }
                25 => {
                    if data.len() < 3 {
                        return None;
                    }
                    let val = u16::from_be_bytes([data[1], data[2]]) as i128;
                    Some((-val - 1, 3))
                }
                26 => {
                    if data.len() < 5 {
                        return None;
                    }
                    let val = u32::from_be_bytes([data[1], data[2], data[3], data[4]]) as i128;
                    Some((-val - 1, 5))
                }
                27 => {
                    if data.len() < 9 {
                        return None;
                    }
                    let val = u64::from_be_bytes([
                        data[1], data[2], data[3], data[4], data[5], data[6], data[7], data[8],
                    ]) as i128;
                    Some((-val - 1, 9))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// Decode a CBOR byte string and return (bytes, total_consumed).
fn decode_cbor_bytes(data: &[u8]) -> Option<(Vec<u8>, usize)> {
    if data.is_empty() {
        return None;
    }
    let major = data[0] >> 5;
    if major != 2 {
        return None;
    }
    let additional = data[0] & 0x1f;
    let (len, header_size) = match additional {
        0..=23 => (additional as usize, 1),
        24 => {
            if data.len() < 2 {
                return None;
            }
            (data[1] as usize, 2)
        }
        25 => {
            if data.len() < 3 {
                return None;
            }
            (u16::from_be_bytes([data[1], data[2]]) as usize, 3)
        }
        26 => {
            if data.len() < 5 {
                return None;
            }
            (
                u32::from_be_bytes([data[1], data[2], data[3], data[4]]) as usize,
                5,
            )
        }
        _ => return None,
    };
    if data.len() < header_size + len {
        return None;
    }
    Some((
        data[header_size..header_size + len].to_vec(),
        header_size + len,
    ))
}

/// Decode a CBOR text string and return (string, total_consumed).
fn decode_cbor_text(data: &[u8]) -> Option<(String, usize)> {
    if data.is_empty() {
        return None;
    }
    let major = data[0] >> 5;
    if major != 3 {
        return None;
    }
    let additional = data[0] & 0x1f;
    let (len, header_size) = match additional {
        0..=23 => (additional as usize, 1),
        24 => {
            if data.len() < 2 {
                return None;
            }
            (data[1] as usize, 2)
        }
        25 => {
            if data.len() < 3 {
                return None;
            }
            (u16::from_be_bytes([data[1], data[2]]) as usize, 3)
        }
        _ => return None,
    };
    if data.len() < header_size + len {
        return None;
    }
    let s = std::str::from_utf8(&data[header_size..header_size + len]).ok()?;
    Some((s.to_string(), header_size + len))
}

// ===========================================================================
// Property-based tests: CBOR primitive encoding round-trips
// ===========================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    // -----------------------------------------------------------------------
    // encode_uint / decode round-trip
    // -----------------------------------------------------------------------
    #[test]
    fn prop_encode_uint_roundtrip(value in any::<u64>()) {
        let encoded = encode_uint(value);
        let (decoded, consumed) = decode_cbor_uint(&encoded).unwrap();
        prop_assert_eq!(decoded, value);
        prop_assert_eq!(consumed, encoded.len());
    }

    // -----------------------------------------------------------------------
    // encode_uint produces minimal CBOR encoding
    // -----------------------------------------------------------------------
    #[test]
    fn prop_encode_uint_minimal(value in any::<u64>()) {
        let encoded = encode_uint(value);
        let expected_len = if value < 24 { 1 }
            else if value < 256 { 2 }
            else if value < 65536 { 3 }
            else if value < 4294967296 { 5 }
            else { 9 };
        prop_assert_eq!(encoded.len(), expected_len);
    }

    // -----------------------------------------------------------------------
    // encode_int / decode round-trip (positive and negative)
    // -----------------------------------------------------------------------
    #[test]
    fn prop_encode_int_roundtrip(value in -1_000_000_000_000i128..1_000_000_000_000i128) {
        let encoded = encode_int(value);
        let (decoded, consumed) = decode_cbor_int(&encoded).unwrap();
        prop_assert_eq!(decoded, value);
        prop_assert_eq!(consumed, encoded.len());
    }

    // -----------------------------------------------------------------------
    // encode_bytes / decode round-trip
    // -----------------------------------------------------------------------
    #[test]
    fn prop_encode_bytes_roundtrip(data in prop::collection::vec(any::<u8>(), 0..=512)) {
        let encoded = encode_bytes(&data);
        let (decoded, consumed) = decode_cbor_bytes(&encoded).unwrap();
        prop_assert_eq!(decoded, data);
        prop_assert_eq!(consumed, encoded.len());
    }

    // -----------------------------------------------------------------------
    // encode_text / decode round-trip
    // -----------------------------------------------------------------------
    #[test]
    fn prop_encode_text_roundtrip(text in "[a-zA-Z0-9 _\\-\\.]{0,256}") {
        let encoded = encode_text(&text);
        let (decoded, consumed) = decode_cbor_text(&encoded).unwrap();
        prop_assert_eq!(decoded, text);
        prop_assert_eq!(consumed, encoded.len());
    }

    // -----------------------------------------------------------------------
    // Hash32 CBOR encode / decode round-trip
    // -----------------------------------------------------------------------
    #[test]
    fn prop_hash32_cbor_roundtrip(hash in arb_hash32()) {
        let encoded = encode_hash32(&hash);
        let (decoded, consumed) = decode_hash32(&encoded).unwrap();
        prop_assert_eq!(decoded, hash);
        prop_assert_eq!(consumed, encoded.len());
        // Hash32 always encodes to exactly 34 bytes (0x58, 0x20, + 32 bytes)
        prop_assert_eq!(encoded.len(), 34);
    }

    // -----------------------------------------------------------------------
    // Hash28 CBOR encode: correct length and decodable
    // -----------------------------------------------------------------------
    #[test]
    fn prop_hash28_cbor_encode(hash in arb_hash28()) {
        let encoded = encode_hash28(&hash);
        // Hash28 always encodes to exactly 30 bytes (0x58, 0x1c, + 28 bytes)
        prop_assert_eq!(encoded.len(), 30);
        // Verify it decodes as valid CBOR byte string
        let (decoded_bytes, consumed) = decode_cbor_bytes(&encoded).unwrap();
        prop_assert_eq!(decoded_bytes.len(), 28);
        prop_assert_eq!(&decoded_bytes[..], hash.as_bytes());
        prop_assert_eq!(consumed, encoded.len());
    }

    // -----------------------------------------------------------------------
    // Point CBOR encoding: Origin always fixed, Specific encodes correctly
    // -----------------------------------------------------------------------
    #[test]
    fn prop_point_specific_encoding(slot in any::<u64>(), hash in arb_hash32()) {
        use torsten_primitives::block::Point;
        let point = Point::Specific(SlotNo(slot), hash);
        let encoded = encode_point(&point);
        // Should start with 0x82 (array of 2)
        prop_assert_eq!(encoded[0], 0x82);
        // After the array header, decode the slot
        let (decoded_slot, slot_len) = decode_cbor_uint(&encoded[1..]).unwrap();
        prop_assert_eq!(decoded_slot, slot);
        // After the slot, decode the hash
        let (decoded_hash, _) = decode_hash32(&encoded[1 + slot_len..]).unwrap();
        prop_assert_eq!(decoded_hash, hash);
    }

    // -----------------------------------------------------------------------
    // TransactionInput CBOR encoding round-trip
    // -----------------------------------------------------------------------
    #[test]
    fn prop_tx_input_cbor_roundtrip(input in arb_tx_input()) {
        let encoded = encode_tx_input(&input);
        // Should start with 0x82 (array of 2)
        prop_assert_eq!(encoded[0], 0x82);
        // Decode hash
        let (decoded_hash, hash_len) = decode_hash32(&encoded[1..]).unwrap();
        prop_assert_eq!(decoded_hash, input.transaction_id);
        // Decode index
        let (decoded_index, _) = decode_cbor_uint(&encoded[1 + hash_len..]).unwrap();
        prop_assert_eq!(decoded_index as u32, input.index);
    }

    // -----------------------------------------------------------------------
    // Value (ADA-only): encode_value produces valid CBOR uint
    // -----------------------------------------------------------------------
    #[test]
    fn prop_value_ada_only_roundtrip(coin in any::<u64>()) {
        let value = Value::lovelace(coin);
        let encoded = encode_value(&value);
        // Pure ADA is just a CBOR uint
        let (decoded_coin, consumed) = decode_cbor_uint(&encoded).unwrap();
        prop_assert_eq!(decoded_coin, coin);
        prop_assert_eq!(consumed, encoded.len());
    }

    // -----------------------------------------------------------------------
    // Value (multi-asset): produces valid CBOR array(2)
    // -----------------------------------------------------------------------
    #[test]
    fn prop_value_multi_asset_structure(value in arb_value_multi_asset()) {
        let encoded = encode_value(&value);
        // Multi-asset value starts with 0x82 (array of 2)
        prop_assert_eq!(encoded[0], 0x82);
        // First element is the coin amount
        let (decoded_coin, _) = decode_cbor_uint(&encoded[1..]).unwrap();
        prop_assert_eq!(decoded_coin, value.coin.0);
    }

    // -----------------------------------------------------------------------
    // PlutusData CBOR encoding: well-formed (non-empty output, no panics)
    // -----------------------------------------------------------------------
    #[test]
    fn prop_plutus_data_encodes_without_panic(data in arb_plutus_data()) {
        let encoded = encode_plutus_data(&data);
        prop_assert!(!encoded.is_empty());
    }

    // -----------------------------------------------------------------------
    // PlutusData::Integer CBOR round-trip
    // -----------------------------------------------------------------------
    #[test]
    fn prop_plutus_data_integer_roundtrip(n in -1_000_000_000i128..1_000_000_000i128) {
        let data = PlutusData::Integer(n);
        let encoded = encode_plutus_data(&data);
        let (decoded, consumed) = decode_cbor_int(&encoded).unwrap();
        prop_assert_eq!(decoded, n);
        prop_assert_eq!(consumed, encoded.len());
    }

    // -----------------------------------------------------------------------
    // PlutusData::Bytes CBOR round-trip
    // -----------------------------------------------------------------------
    #[test]
    fn prop_plutus_data_bytes_roundtrip(bytes in prop::collection::vec(any::<u8>(), 0..=128)) {
        let data = PlutusData::Bytes(bytes.clone());
        let encoded = encode_plutus_data(&data);
        let (decoded, consumed) = decode_cbor_bytes(&encoded).unwrap();
        prop_assert_eq!(decoded, bytes);
        prop_assert_eq!(consumed, encoded.len());
    }

    // -----------------------------------------------------------------------
    // PlutusData::Constr tag encoding correctness
    // -----------------------------------------------------------------------
    #[test]
    fn prop_plutus_constr_small_tag(tag in 0u64..7) {
        let data = PlutusData::Constr(tag, vec![]);
        let encoded = encode_plutus_data(&data);
        // Small constructors use 1-byte CBOR tag (0xd8) followed by 121+tag
        prop_assert_eq!(encoded[0], 0xd8);
        prop_assert_eq!(encoded[1], (121 + tag) as u8);
        prop_assert_eq!(encoded[2], 0x80); // empty array
    }

    #[test]
    fn prop_plutus_constr_medium_tag(tag in 7u64..128) {
        let data = PlutusData::Constr(tag, vec![]);
        let encoded = encode_plutus_data(&data);
        // Medium constructors use 2-byte CBOR tag (0xd9) followed by 1280+(tag-7)
        prop_assert_eq!(encoded[0], 0xd9);
        let expected_tag = 1280 + (tag - 7);
        let actual_tag = u16::from_be_bytes([encoded[1], encoded[2]]);
        prop_assert_eq!(actual_tag as u64, expected_tag);
        prop_assert_eq!(encoded[3], 0x80); // empty array
    }

    #[test]
    fn prop_plutus_constr_large_tag(tag in 128u64..512) {
        let data = PlutusData::Constr(tag, vec![]);
        let encoded = encode_plutus_data(&data);
        // Large constructors use tag 102 (0xd8 0x66)
        prop_assert_eq!(encoded[0], 0xd8);
        prop_assert_eq!(encoded[1], 0x66);
        // Followed by array(2): [constructor_index, fields_array]
        prop_assert_eq!(encoded[2], 0x82);
    }

    // -----------------------------------------------------------------------
    // TransactionMetadatum: encoding never panics and produces non-empty output
    // -----------------------------------------------------------------------
    #[test]
    fn prop_metadatum_encodes_without_panic(meta in arb_metadatum()) {
        let encoded = encode_metadatum(&meta);
        prop_assert!(!encoded.is_empty());
    }

    // -----------------------------------------------------------------------
    // TransactionMetadatum::Int round-trip
    // -----------------------------------------------------------------------
    #[test]
    fn prop_metadatum_int_roundtrip(n in -1_000_000_000i128..1_000_000_000i128) {
        let meta = TransactionMetadatum::Int(n);
        let encoded = encode_metadatum(&meta);
        let (decoded, consumed) = decode_cbor_int(&encoded).unwrap();
        prop_assert_eq!(decoded, n);
        prop_assert_eq!(consumed, encoded.len());
    }

    // -----------------------------------------------------------------------
    // TransactionMetadatum::Bytes round-trip
    // -----------------------------------------------------------------------
    #[test]
    fn prop_metadatum_bytes_roundtrip(bytes in prop::collection::vec(any::<u8>(), 0..=64)) {
        let meta = TransactionMetadatum::Bytes(bytes.clone());
        let encoded = encode_metadatum(&meta);
        let (decoded, consumed) = decode_cbor_bytes(&encoded).unwrap();
        prop_assert_eq!(decoded, bytes);
        prop_assert_eq!(consumed, encoded.len());
    }

    // -----------------------------------------------------------------------
    // TransactionMetadatum::Text round-trip
    // -----------------------------------------------------------------------
    #[test]
    fn prop_metadatum_text_roundtrip(text in "[a-zA-Z0-9 ]{0,64}") {
        let meta = TransactionMetadatum::Text(text.clone());
        let encoded = encode_metadatum(&meta);
        let (decoded, consumed) = decode_cbor_text(&encoded).unwrap();
        prop_assert_eq!(decoded, text);
        prop_assert_eq!(consumed, encoded.len());
    }

    // -----------------------------------------------------------------------
    // encode_bool round-trip
    // -----------------------------------------------------------------------
    #[test]
    fn prop_encode_bool_roundtrip(value in any::<bool>()) {
        let encoded = encode_bool(value);
        prop_assert_eq!(encoded.len(), 1);
        let decoded = match encoded[0] {
            0xf5 => true,
            0xf4 => false,
            _ => panic!("unexpected bool encoding"),
        };
        prop_assert_eq!(decoded, value);
    }

    // -----------------------------------------------------------------------
    // encode_array_header: correct CBOR encoding
    // -----------------------------------------------------------------------
    #[test]
    fn prop_encode_array_header_valid(len in 0usize..=1000) {
        let encoded = encode_array_header(len);
        // Verify CBOR major type 4 (array)
        prop_assert_eq!(encoded[0] >> 5, 4);
        // Decode the length
        let additional = encoded[0] & 0x1f;
        let decoded_len = if additional < 24 {
            additional as usize
        } else if additional == 24 {
            encoded[1] as usize
        } else if additional == 25 {
            u16::from_be_bytes([encoded[1], encoded[2]]) as usize
        } else {
            panic!("unexpected array header size");
        };
        prop_assert_eq!(decoded_len, len);
    }

    // -----------------------------------------------------------------------
    // encode_map_header: correct CBOR encoding
    // -----------------------------------------------------------------------
    #[test]
    fn prop_encode_map_header_valid(len in 0usize..=1000) {
        let encoded = encode_map_header(len);
        // Verify CBOR major type 5 (map)
        prop_assert_eq!(encoded[0] >> 5, 5);
        // Decode the length
        let additional = encoded[0] & 0x1f;
        let decoded_len = if additional < 24 {
            additional as usize
        } else if additional == 24 {
            encoded[1] as usize
        } else if additional == 25 {
            u16::from_be_bytes([encoded[1], encoded[2]]) as usize
        } else {
            panic!("unexpected map header size");
        };
        prop_assert_eq!(decoded_len, len);
    }

    // -----------------------------------------------------------------------
    // encode_tag: correct CBOR encoding
    // -----------------------------------------------------------------------
    #[test]
    fn prop_encode_tag_valid(tag in 0u64..=65535) {
        let encoded = encode_tag(tag);
        // Verify CBOR major type 6 (tag)
        prop_assert_eq!(encoded[0] >> 5, 6);
        // Decode the tag value
        let additional = encoded[0] & 0x1f;
        let decoded_tag = if additional < 24 {
            additional as u64
        } else if additional == 24 {
            encoded[1] as u64
        } else if additional == 25 {
            u16::from_be_bytes([encoded[1], encoded[2]]) as u64
        } else {
            panic!("unexpected tag encoding for {}", tag);
        };
        prop_assert_eq!(decoded_tag, tag);
    }

    // -----------------------------------------------------------------------
    // Encode/decode idempotence: encoding the same value twice gives same bytes
    // -----------------------------------------------------------------------
    #[test]
    fn prop_encode_uint_deterministic(value in any::<u64>()) {
        let a = encode_uint(value);
        let b = encode_uint(value);
        prop_assert_eq!(a, b);
    }

    #[test]
    fn prop_encode_int_deterministic(value in -1_000_000_000_000i128..1_000_000_000_000i128) {
        let a = encode_int(value);
        let b = encode_int(value);
        prop_assert_eq!(a, b);
    }

    #[test]
    fn prop_encode_hash32_deterministic(hash in arb_hash32()) {
        let a = encode_hash32(&hash);
        let b = encode_hash32(&hash);
        prop_assert_eq!(a, b);
    }

    #[test]
    fn prop_encode_plutus_data_deterministic(data in arb_plutus_data()) {
        let a = encode_plutus_data(&data);
        let b = encode_plutus_data(&data);
        prop_assert_eq!(a, b);
    }

    #[test]
    fn prop_encode_value_deterministic(value in arb_value()) {
        let a = encode_value(&value);
        let b = encode_value(&value);
        prop_assert_eq!(a, b);
    }

    #[test]
    fn prop_encode_metadatum_deterministic(meta in arb_metadatum()) {
        let a = encode_metadatum(&meta);
        let b = encode_metadatum(&meta);
        prop_assert_eq!(a, b);
    }
}
