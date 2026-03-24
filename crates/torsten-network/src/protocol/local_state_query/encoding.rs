//! Query-specific CBOR encoding helpers for LocalStateQuery.
//!
//! This module provides encoding utilities for common query response patterns.
//! The actual query-specific encoding (protocol params, UTxO, stake distribution,
//! governance, etc.) lives in the node integration layer which has access to
//! the ledger types.
//!
//! ## HFC wrapping
//! BlockQuery results must be wrapped in the HFC `QueryIfCurrent` success
//! envelope: `[1, result]`. QueryAnytime and QueryHardFork results are unwrapped.

use minicbor::Encoder;

/// Wrap a CBOR result in the HFC QueryIfCurrent success envelope: `[1, result]`.
pub fn wrap_hfc_success(result_cbor: &[u8]) -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut enc = Encoder::new(&mut buf);
        enc.array(2).expect("infallible");
        enc.u64(1).expect("infallible"); // success tag
    }
    buf.extend_from_slice(result_cbor);
    buf
}

/// Encode an HFC failure (era mismatch): `[0, [era_index, era_start, era_end]]`.
pub fn encode_hfc_era_mismatch(era_index: u64) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut enc = Encoder::new(&mut buf);
    enc.array(2).expect("infallible");
    enc.u64(0).expect("infallible"); // failure tag
    enc.array(1).expect("infallible");
    enc.u64(era_index).expect("infallible");
    buf
}

/// Encode a CBOR tag 24 (embedded CBOR) wrapper around raw bytes.
/// Used by GetCBOR (tag 9) which wraps the inner query result in tag 24.
pub fn encode_cbor_tag24(inner: &[u8]) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut enc = Encoder::new(&mut buf);
    enc.tag(minicbor::data::Tag::new(24)).expect("infallible");
    enc.bytes(inner).expect("infallible");
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use minicbor::Decoder;

    #[test]
    fn hfc_success_wrapping() {
        // Wrap a simple integer result
        let mut result = Vec::new();
        Encoder::new(&mut result).u64(42).expect("infallible");

        let wrapped = wrap_hfc_success(&result);
        let mut dec = Decoder::new(&wrapped);
        dec.array().unwrap();
        assert_eq!(dec.u64().unwrap(), 1); // success tag
        assert_eq!(dec.u64().unwrap(), 42); // inner result
    }

    #[test]
    fn cbor_tag24_wrapping() {
        let inner = vec![0x82, 0x01, 0x02]; // [1, 2]
        let wrapped = encode_cbor_tag24(&inner);
        let mut dec = Decoder::new(&wrapped);
        let tag = dec.tag().unwrap();
        assert_eq!(tag.as_u64(), 24);
        let bytes = dec.bytes().unwrap();
        assert_eq!(bytes, &[0x82, 0x01, 0x02]);
    }
}
