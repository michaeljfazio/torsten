use crate::cbor::*;
use std::collections::BTreeMap;
use torsten_primitives::hash::Hash28;
use torsten_primitives::value::{AssetName, Value};

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
