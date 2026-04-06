use super::cbor_utils::*;
use dugite_primitives::hash::Hash32;

// ── decode_uint ────────────────────────────────────────────────────────────────

#[test]
fn test_decode_uint_small() {
    // Values 0-23 are inline in the initial byte (additional info 0-23).
    assert_eq!(decode_uint(&[0x00]).unwrap(), (0, 1));
    assert_eq!(decode_uint(&[0x17]).unwrap(), (23, 1));
    // 24 requires a one-byte follow-on (additional info 24).
    assert_eq!(decode_uint(&[0x18, 0x18]).unwrap(), (24, 2));
    assert_eq!(decode_uint(&[0x18, 0xff]).unwrap(), (255, 2));
}

#[test]
fn test_decode_uint_large() {
    // Two-byte uint (additional info 25).
    assert_eq!(decode_uint(&[0x19, 0x01, 0x00]).unwrap(), (256, 3));
    // Four-byte uint (additional info 26).
    assert_eq!(
        decode_uint(&[0x1a, 0x00, 0x01, 0x00, 0x00]).unwrap(),
        (65536, 5)
    );
    // Eight-byte uint (additional info 27).
    assert_eq!(
        decode_uint(&[0x1b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x2c]).unwrap(),
        (44, 9)
    );
}

#[test]
fn test_decode_uint_wrong_major() {
    // Major type 1 (negative integer) should be rejected by decode_uint.
    assert!(decode_uint(&[0x20]).is_err());
}

// ── decode_int ─────────────────────────────────────────────────────────────────

#[test]
fn test_decode_int_positive() {
    assert_eq!(decode_int(&[0x00]).unwrap(), (0, 1));
    assert_eq!(decode_int(&[0x0a]).unwrap(), (10, 1));
}

#[test]
fn test_decode_int_negative() {
    // 0x20 = major 1, info 0 → -1
    assert_eq!(decode_int(&[0x20]).unwrap(), (-1, 1));
    // 0x37 = major 1, info 23 → -24
    assert_eq!(decode_int(&[0x37]).unwrap(), (-24, 1));
}

// ── decode_array_len ───────────────────────────────────────────────────────────

#[test]
fn test_decode_array_len() {
    assert_eq!(decode_array_len(&[0x80]).unwrap(), (0, 1)); // array(0)
    assert_eq!(decode_array_len(&[0x82]).unwrap(), (2, 1)); // array(2)
    assert_eq!(decode_array_len(&[0x87]).unwrap(), (7, 1)); // array(7)
    assert_eq!(decode_array_len(&[0x98, 0x1f]).unwrap(), (31, 2)); // array(31)
}

#[test]
fn test_decode_array_len_wrong_major() {
    // 0xa0 = map(0) — not an array.
    assert!(decode_array_len(&[0xa0]).is_err());
}

// ── decode_map_len ─────────────────────────────────────────────────────────────

#[test]
fn test_decode_map_len_definite() {
    assert_eq!(decode_map_len(&[0xa0]).unwrap(), (Some(0), 1));
    assert_eq!(decode_map_len(&[0xa3]).unwrap(), (Some(3), 1));
}

#[test]
fn test_decode_map_len_indefinite() {
    // 0xbf = indefinite-length map
    assert_eq!(decode_map_len(&[0xbf]).unwrap(), (None, 1));
}

// ── decode_nonce ───────────────────────────────────────────────────────────────

#[test]
fn test_decode_nonce_neutral() {
    // array(1) [0] = NeutralNonce → zero hash
    let data = [0x81, 0x00];
    let (hash, consumed) = decode_nonce(&data).unwrap();
    assert_eq!(consumed, 2);
    assert_eq!(hash, Hash32::ZERO);
}

#[test]
fn test_decode_nonce_value() {
    // array(2) [1, bytes(32)] = Nonce carrying a hash value
    let mut data = vec![0x82, 0x01, 0x58, 0x20];
    data.extend_from_slice(&[0xab; 32]);
    let (hash, consumed) = decode_nonce(&data).unwrap();
    // 1 (array hdr) + 1 (tag uint) + 2 (bytes hdr) + 32 (payload) = 36
    assert_eq!(consumed, 36);
    assert_eq!(hash.as_bytes(), &[0xab; 32]);
}

#[test]
fn test_decode_nonce_invalid_tag() {
    // array(1) [2] — tag 2 is not valid for a Nonce
    let data = [0x81, 0x02];
    assert!(decode_nonce(&data).is_err());
}

// ── decode_with_origin_len ─────────────────────────────────────────────────────

#[test]
fn test_decode_with_origin_absent() {
    // array(0) = Origin
    let data = [0x80];
    let (present, consumed) = decode_with_origin_len(&data).unwrap();
    assert_eq!(consumed, 1);
    assert!(present.is_none());
}

#[test]
fn test_decode_with_origin_present() {
    // array(1) = At x; only the array header is consumed by this function
    let data = [0x81, 0x19, 0x04, 0x00]; // [1024]
    let (present, consumed) = decode_with_origin_len(&data).unwrap();
    assert_eq!(consumed, 1); // only the array header byte
    assert!(present.is_some());
}

#[test]
fn test_decode_with_origin_invalid_len() {
    // array(2) is neither Origin nor At — must error
    let data = [0x82, 0x01, 0x02];
    assert!(decode_with_origin_len(&data).is_err());
}

// ── decode_rational ────────────────────────────────────────────────────────────

#[test]
fn test_decode_rational_with_tag() {
    // tag(30) array(2) [3, 10]  =  3/10
    let data = [0xd8, 0x1e, 0x82, 0x03, 0x0a];
    let ((num, den), consumed) = decode_rational(&data).unwrap();
    assert_eq!(num, 3);
    assert_eq!(den, 10);
    assert_eq!(consumed, 5);
}

#[test]
fn test_decode_rational_no_tag() {
    // array(2) [0x19 0x02 0x41, 0x19 0x27 0x10]  =  [577, 10000]
    let data = [0x82, 0x19, 0x02, 0x41, 0x19, 0x27, 0x10];
    let ((num, den), consumed) = decode_rational(&data).unwrap();
    assert_eq!(num, 577);
    assert_eq!(den, 10000);
    assert_eq!(consumed, 7);
}

#[test]
fn test_decode_rational_small() {
    // Plain array(2) [1, 1]  = 1/1
    let data = [0x82, 0x01, 0x01];
    let ((num, den), consumed) = decode_rational(&data).unwrap();
    assert_eq!(num, 1);
    assert_eq!(den, 1);
    assert_eq!(consumed, 3);
}

// ── decode_credential ─────────────────────────────────────────────────────────

#[test]
fn test_decode_credential_keyhash() {
    // array(2) [0, bytes(28)]  = KeyHash credential
    let mut data = vec![0x82, 0x00, 0x58, 0x1c];
    data.extend_from_slice(&[0xaa; 28]);
    let ((tag, hash), consumed) = decode_credential(&data).unwrap();
    assert_eq!(tag, 0);
    assert_eq!(hash.as_bytes(), &[0xaa; 28]);
    // 1 (array hdr) + 1 (tag uint) + 2 (bytes hdr 0x58 0x1c) + 28 (payload) = 32
    assert_eq!(consumed, 32);
}

#[test]
fn test_decode_credential_scripthash() {
    // array(2) [1, bytes(28)]  = ScriptHash credential
    let mut data = vec![0x82, 0x01, 0x58, 0x1c];
    data.extend_from_slice(&[0xbb; 28]);
    let ((tag, hash), consumed) = decode_credential(&data).unwrap();
    assert_eq!(tag, 1);
    assert_eq!(hash.as_bytes(), &[0xbb; 28]);
    assert_eq!(consumed, 32);
}

// ── skip_cbor_value ────────────────────────────────────────────────────────────

#[test]
fn test_skip_uint() {
    assert_eq!(skip_cbor_value(&[0x05]).unwrap(), 1);
    assert_eq!(skip_cbor_value(&[0x18, 0x64]).unwrap(), 2);
}

#[test]
fn test_skip_bytes() {
    // bytes(4) 0x44 0x01 0x02 0x03 0x04
    assert_eq!(skip_cbor_value(&[0x44, 0x01, 0x02, 0x03, 0x04]).unwrap(), 5);
}

#[test]
fn test_skip_nested_array() {
    // array(2) [1, bytes(32)]
    let mut data = vec![0x82, 0x01, 0x58, 0x20];
    data.extend_from_slice(&[0x00; 32]);
    assert_eq!(skip_cbor_value(&data).unwrap(), 36);
}

#[test]
fn test_skip_map() {
    // map(1) {0 => 1}  =  0xa1 0x00 0x01
    assert_eq!(skip_cbor_value(&[0xa1, 0x00, 0x01]).unwrap(), 3);
}

#[test]
fn test_skip_tagged_value() {
    // tag(30) array(2) [1, 2]  = rational 1/2
    assert_eq!(
        skip_cbor_value(&[0xd8, 0x1e, 0x82, 0x01, 0x02]).unwrap(),
        5
    );
}

// ── decode_null ───────────────────────────────────────────────────────────────

#[test]
fn test_decode_null_is_null() {
    assert_eq!(decode_null(&[0xf6]).unwrap(), (true, 1));
}

#[test]
fn test_decode_null_not_null() {
    // A non-null value: cursor should not be advanced.
    assert_eq!(decode_null(&[0x00]).unwrap(), (false, 0));
    assert_eq!(decode_null(&[0x80]).unwrap(), (false, 0));
}

// ── decode_bytes / decode_text ─────────────────────────────────────────────────

#[test]
fn test_decode_bytes_short() {
    // bytes(3) 0x41 0x42 0x43
    let data = [0x43, 0x41, 0x42, 0x43];
    let (b, n) = decode_bytes(&data).unwrap();
    assert_eq!(b, b"ABC");
    assert_eq!(n, 4);
}

#[test]
fn test_decode_text_short() {
    // text(5) "hello"  = 0x65 h e l l o
    let data = [0x65, b'h', b'e', b'l', b'l', b'o'];
    let (s, n) = decode_text(&data).unwrap();
    assert_eq!(s, "hello");
    assert_eq!(n, 6);
}

#[test]
fn test_decode_text_wrong_major() {
    // 0x43 is bytes(3), not text
    assert!(decode_text(&[0x43, 0x41, 0x42, 0x43]).is_err());
}

// ── decode_hash28 / decode_hash32 ─────────────────────────────────────────────

#[test]
fn test_decode_hash28_correct_length() {
    let mut data = vec![0x58, 0x1c]; // bytes(28)
    data.extend_from_slice(&[0xde; 28]);
    let (h, n) = decode_hash28(&data).unwrap();
    assert_eq!(h.as_bytes(), &[0xde; 28]);
    assert_eq!(n, 30);
}

#[test]
fn test_decode_hash28_wrong_length() {
    // bytes(32) should be rejected by decode_hash28
    let mut data = vec![0x58, 0x20];
    data.extend_from_slice(&[0x00; 32]);
    assert!(decode_hash28(&data).is_err());
}

#[test]
fn test_decode_hash32_correct_length() {
    let mut data = vec![0x58, 0x20]; // bytes(32)
    data.extend_from_slice(&[0xef; 32]);
    let (h, n) = decode_hash32(&data).unwrap();
    assert_eq!(h.as_bytes(), &[0xef; 32]);
    assert_eq!(n, 34);
}

#[test]
fn test_decode_hash32_wrong_length() {
    // bytes(28) should be rejected by decode_hash32
    let mut data = vec![0x58, 0x1c];
    data.extend_from_slice(&[0x00; 28]);
    assert!(decode_hash32(&data).is_err());
}

// ── decode_bigint_or_uint ─────────────────────────────────────────────────────

#[test]
fn test_decode_bigint_plain_uint() {
    assert_eq!(decode_bigint_or_uint(&[0x0a]).unwrap(), (10, 1));
}

#[test]
fn test_decode_bigint_tag2() {
    // tag(2) bytes(2) [0x01, 0x00]  = bignum 256
    let data = [0xc2, 0x42, 0x01, 0x00];
    let (v, n) = decode_bigint_or_uint(&data).unwrap();
    assert_eq!(v, 256);
    assert_eq!(n, 4);
}
