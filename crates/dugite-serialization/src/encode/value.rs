use crate::cbor::*;
use dugite_primitives::hash::Hash28;
use dugite_primitives::value::{AssetName, Value};
use std::collections::BTreeMap;

/// Encode a Value to CBOR.
///
/// Pure ADA: just the coin amount.
/// Multi-asset: [coin, {policy_id: {asset_name: quantity}, ...}]
pub fn encode_value(value: &Value) -> Vec<u8> {
    if value.is_pure_ada() {
        encode_uint(value.coin.0)
    } else {
        let mut buf = encode_array_header(2);
        buf.extend(encode_uint(value.coin.0));
        buf.extend(encode_multi_asset(&value.multi_asset));
        buf
    }
}

/// Encode multi-asset map: {policy_id: {asset_name: quantity}}
pub(crate) fn encode_multi_asset(
    multi_asset: &BTreeMap<Hash28, BTreeMap<AssetName, u64>>,
) -> Vec<u8> {
    let mut buf = encode_map_header(multi_asset.len());
    for (policy_id, assets) in multi_asset {
        buf.extend(encode_hash28(policy_id));
        buf.extend(encode_map_header(assets.len()));
        for (asset_name, qty) in assets {
            buf.extend(encode_bytes(&asset_name.0));
            buf.extend(encode_uint(*qty));
        }
    }
    buf
}

/// Encode mint map: {policy_id: {asset_name: i64}}
pub(crate) fn encode_mint(mint: &BTreeMap<Hash28, BTreeMap<AssetName, i64>>) -> Vec<u8> {
    let mut buf = encode_map_header(mint.len());
    for (policy_id, assets) in mint {
        buf.extend(encode_hash28(policy_id));
        buf.extend(encode_map_header(assets.len()));
        for (asset_name, qty) in assets {
            buf.extend(encode_bytes(&asset_name.0));
            buf.extend(encode_int(*qty as i128));
        }
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use dugite_primitives::value::Lovelace;

    // ---------------------------------------------------------------------------
    // Helpers
    // ---------------------------------------------------------------------------

    /// Build a 28-byte Hash28 from a repeating seed byte (deterministic, cheap).
    fn policy(seed: u8) -> Hash28 {
        Hash28::from_bytes([seed; 28])
    }

    /// Build an AssetName from a byte slice.
    fn asset(name: &[u8]) -> AssetName {
        AssetName(name.to_vec())
    }

    // ---------------------------------------------------------------------------
    // encode_value — pure ADA
    // ---------------------------------------------------------------------------

    /// ADA-only value must encode as a bare CBOR uint (not wrapped in an array).
    #[test]
    fn test_encode_value_pure_ada() {
        let v = Value::lovelace(1_000_000);
        let enc = encode_value(&v);
        // 1_000_000 = 0x0F4240  → 0x1a 0x00 0x0f 0x42 0x40
        assert_eq!(enc, vec![0x1a, 0x00, 0x0f, 0x42, 0x40]);
        // Must NOT start with an array header (0x82)
        assert_ne!(enc[0], 0x82, "pure-ADA value must not be wrapped in array");
    }

    /// ADA-only zero value encodes as CBOR uint 0.
    #[test]
    fn test_encode_value_zero_ada() {
        let v = Value::lovelace(0);
        let enc = encode_value(&v);
        assert_eq!(enc, vec![0x00]);
    }

    // ---------------------------------------------------------------------------
    // encode_value — multi-asset
    // ---------------------------------------------------------------------------

    /// Multi-asset value encodes as CBOR array(2): [coin, {policy: {asset: qty}}].
    #[test]
    fn test_encode_value_multi_asset_structure() {
        let mut v = Value::lovelace(2_000_000);
        let mut assets = BTreeMap::new();
        assets.insert(asset(b"tokenA"), 100u64);
        v.multi_asset.insert(policy(0x01), assets);

        let enc = encode_value(&v);

        // First byte: array(2) = 0x82
        assert_eq!(enc[0], 0x82, "multi-asset value must start with array(2)");

        // Coin: 2_000_000 = 0x1E8480
        // 0x1a 0x00 0x1e 0x84 0x80
        assert_eq!(&enc[1..6], &[0x1a, 0x00, 0x1e, 0x84, 0x80], "coin mismatch");

        // Multi-asset map header: map(1) = 0xa1
        assert_eq!(enc[6], 0xa1, "multi-asset outer map must have 1 entry");
    }

    /// Multi-asset encoding with two policies, each with one asset.
    #[test]
    fn test_encode_value_two_policies() {
        let mut v = Value::lovelace(0);
        let mut assets_a = BTreeMap::new();
        assets_a.insert(asset(b"A"), 1u64);
        let mut assets_b = BTreeMap::new();
        assets_b.insert(asset(b"B"), 2u64);
        // BTreeMap is ordered by key, so policy(0x01) < policy(0x02)
        v.multi_asset.insert(policy(0x01), assets_a);
        v.multi_asset.insert(policy(0x02), assets_b);

        let enc = encode_value(&v);

        // array(2)
        assert_eq!(enc[0], 0x82);
        // coin = 0
        assert_eq!(enc[1], 0x00);
        // outer map(2) = 0xa2
        assert_eq!(enc[2], 0xa2, "outer map must report 2 policies");
    }

    /// Multi-asset value with an empty inner asset map still encodes correctly.
    #[test]
    fn test_encode_value_empty_multi_asset_map() {
        let mut v = Value::lovelace(500);
        // Insert a policy with zero assets (degenerate but must not panic)
        v.multi_asset.insert(policy(0xAA), BTreeMap::new());

        let enc = encode_value(&v);

        // array(2) header
        assert_eq!(enc[0], 0x82);
        // outer map(1) = 0xa1
        // offset: coin is 500 = 0x19 0x01 0xf4 → 3 bytes
        assert_eq!(enc[4], 0xa1, "outer map must have 1 policy entry");
        // inner map(0) = 0xa0 — immediately after the 30-byte Hash28 header+body
        // Hash28 header: 0x58 0x1c = 2 bytes, then 28 bytes = 30 bytes after outer-map byte
        let inner_map_offset = 4 + 1 + 2 + 28; // outer_map + policy_header(2) + policy_bytes(28)
        assert_eq!(
            enc[inner_map_offset], 0xa0,
            "empty inner asset map must encode as map(0)"
        );
    }

    // ---------------------------------------------------------------------------
    // encode_multi_asset
    // ---------------------------------------------------------------------------

    /// encode_multi_asset with a single policy, two assets encodes both correctly.
    #[test]
    fn test_encode_multi_asset_two_assets_per_policy() {
        let mut multi: BTreeMap<Hash28, BTreeMap<AssetName, u64>> = BTreeMap::new();
        let mut assets = BTreeMap::new();
        // Use short names so lengths are predictable
        assets.insert(asset(b"x"), 10u64);
        assets.insert(asset(b"y"), 20u64);
        multi.insert(policy(0x05), assets);

        let enc = encode_multi_asset(&multi);

        // outer map(1) = 0xa1
        assert_eq!(enc[0], 0xa1);
        // policy Hash28: 0x58 0x1c ...
        assert_eq!(enc[1], 0x58);
        assert_eq!(enc[2], 28);
        // inner map(2) = 0xa2, located at byte 1 + 30 = 31
        assert_eq!(enc[31], 0xa2, "inner map must have 2 asset entries");
    }

    // ---------------------------------------------------------------------------
    // encode_mint — negative quantities
    // ---------------------------------------------------------------------------

    /// encode_mint with a negative quantity must use CBOR negative integer encoding.
    #[test]
    fn test_encode_mint_negative_quantity() {
        let mut mint: BTreeMap<Hash28, BTreeMap<AssetName, i64>> = BTreeMap::new();
        let mut assets = BTreeMap::new();
        assets.insert(asset(b"burn"), -500i64);
        mint.insert(policy(0x10), assets);

        let enc = encode_mint(&mint);

        // outer map(1) = 0xa1
        assert_eq!(enc[0], 0xa1);

        // After policy bytes (1+30=31 bytes) we have:
        // inner map(1) = 0xa1
        assert_eq!(enc[31], 0xa1);

        // asset name b"burn" (4 bytes) → encode_bytes: 0x44 + 4 bytes = 5 bytes
        // at offset 32
        assert_eq!(enc[32], 0x44, "asset name should be 4-byte bytestring");

        // quantity -500:  -(500) - 1 = 499 = 0x01F3 → 0x39 0x01 0xf3
        let qty_offset = 32 + 1 + 4; // map_header(1) + bytestr_header(1) + "burn"(4)
        assert_eq!(enc[qty_offset], 0x39, "negative qty must use 2-byte CBOR negative");
        assert_eq!(enc[qty_offset + 1], 0x01);
        assert_eq!(enc[qty_offset + 2], 0xf3);
    }

    /// encode_mint with a positive quantity uses normal uint encoding.
    #[test]
    fn test_encode_mint_positive_quantity() {
        let mut mint: BTreeMap<Hash28, BTreeMap<AssetName, i64>> = BTreeMap::new();
        let mut assets = BTreeMap::new();
        assets.insert(asset(b"mint"), 42i64);
        mint.insert(policy(0x20), assets);

        let enc = encode_mint(&mint);

        // quantity 42 → 0x18 0x2a  (one-byte uint)
        // offset: 0xa1 + policy(30) + 0xa1 + name_bytes(5 for "mint") = 37
        let qty_offset = 1 + 30 + 1 + 1 + 4;
        assert_eq!(enc[qty_offset], 0x18, "positive qty 42 should use 0x18 prefix");
        assert_eq!(enc[qty_offset + 1], 42);
    }

    /// encode_mint with multiple assets per policy encodes all of them.
    #[test]
    fn test_encode_mint_multiple_assets_per_policy() {
        let mut mint: BTreeMap<Hash28, BTreeMap<AssetName, i64>> = BTreeMap::new();
        let mut assets = BTreeMap::new();
        assets.insert(asset(b"a"), 1i64);
        assets.insert(asset(b"b"), -1i64);
        assets.insert(asset(b"c"), 0i64);
        mint.insert(policy(0x30), assets);

        let enc = encode_mint(&mint);

        // inner map must have 3 entries: 0xa3
        assert_eq!(enc[31], 0xa3, "inner map must have 3 asset entries");
    }

    // ---------------------------------------------------------------------------
    // Roundtrip length sanity checks
    // ---------------------------------------------------------------------------

    /// Verifies the total byte length of a known single-asset encoding.
    #[test]
    fn test_encode_value_known_byte_length() {
        // Value: 1 ADA + 1 policy with 1 asset named "X" (1 byte), qty=1
        let mut v = Value {
            coin: Lovelace(1_000_000),
            multi_asset: BTreeMap::new(),
        };
        let mut assets = BTreeMap::new();
        assets.insert(asset(b"X"), 1u64);
        v.multi_asset.insert(policy(0xBE), assets);

        let enc = encode_value(&v);

        // Layout:
        //   0x82                        — array(2)         1
        //   0x1a 0x00 0x0f 0x42 0x40   — coin=1_000_000   5
        //   0xa1                        — map(1)           1
        //   0x58 0x1c <28 bytes>        — policy_id       30
        //   0xa1                        — map(1)           1
        //   0x41 0x58                   — bytes(1) "X"     2
        //   0x01                        — uint 1           1
        // Total: 1+5+1+30+1+2+1 = 41
        assert_eq!(enc.len(), 41, "unexpected encoded length for single-asset value");
    }
}
