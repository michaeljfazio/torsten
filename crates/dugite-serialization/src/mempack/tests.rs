//! Tests for the MemPack UTxO decoder.

use super::compact::decode_varlen;
use super::txout::decode_mempack_txout;
use super::{decode_mempack_txin, TvarIterator};

#[test]
fn test_decode_varlen_small() {
    assert_eq!(decode_varlen(&[0]).unwrap(), (0, 1));
    assert_eq!(decode_varlen(&[1]).unwrap(), (1, 1));
    assert_eq!(decode_varlen(&[29]).unwrap(), (29, 1));
    assert_eq!(decode_varlen(&[127]).unwrap(), (127, 1));
}

#[test]
fn test_decode_varlen_multi_byte() {
    assert_eq!(decode_varlen(&[0x96, 0x01]).unwrap(), (150, 2));
    assert_eq!(decode_varlen(&[0x80, 0x01]).unwrap(), (128, 2));
}

#[test]
fn test_decode_varlen_three_byte() {
    // 28398 = 0xee, 0xdd, 0x01
    // 0x6e | (0x5d << 7) | (0x01 << 14) = 110 + 11904 + 16384 = 28398
    assert_eq!(decode_varlen(&[0xee, 0xdd, 0x01]).unwrap(), (28398, 3));
}

#[test]
fn test_decode_varlen_empty_input() {
    assert!(decode_varlen(&[]).is_err());
}

#[test]
fn test_decode_mempack_txin() {
    // Real key from preview tvar fixture:
    // TxId = 00000c339a7d28e08060a69e3d9adf16846382f59a4d321f8b9580ffdb597c0b
    // TxIx = 1 (bytes 01 00 in LE)
    let key = hex::decode("00000c339a7d28e08060a69e3d9adf16846382f59a4d321f8b9580ffdb597c0b0100")
        .unwrap();
    let txin = decode_mempack_txin(&key).unwrap();
    assert_eq!(txin.txix, 1);
    assert_eq!(
        txin.txid.to_hex(),
        "00000c339a7d28e08060a69e3d9adf16846382f59a4d321f8b9580ffdb597c0b"
    );
}

#[test]
fn test_decode_mempack_txin_wrong_length() {
    let short = vec![0u8; 33];
    assert!(decode_mempack_txin(&short).is_err());
    let long = vec![0u8; 35];
    assert!(decode_mempack_txin(&long).is_err());
}

#[test]
fn test_decode_mempack_txin_txix_zero() {
    let key = vec![0u8; 34];
    // TxIx = 0x0000 LE = 0
    let txin = decode_mempack_txin(&key).unwrap();
    assert_eq!(txin.txix, 0);
}

#[test]
fn test_decode_mempack_txin_txix_large() {
    let mut key = vec![0xAA; 34];
    // TxIx = 0xFF 0x00 LE = 255
    key[32] = 0xFF;
    key[33] = 0x00;
    let txin = decode_mempack_txin(&key).unwrap();
    assert_eq!(txin.txix, 255);
}

#[test]
fn test_decode_mempack_txout_tag0() {
    // Real tag-0 entry from preview tvar:
    // tag=0, addr_len=29 (0x1d), addr=60986c..., value_tag=0, coin=28398
    let val = hex::decode("001d60986cdecfc4f555a8605d621505a4a82c25c574f59fd0b79e2acdaf0200eedd01")
        .unwrap();
    let (txout, consumed) = decode_mempack_txout(&val).unwrap();
    assert_eq!(consumed, val.len());
    assert_eq!(txout.tag, 0);
    assert_eq!(txout.address.len(), 29);
    assert_eq!(txout.address[0], 0x60); // Enterprise address testnet
    assert_eq!(txout.coin, 28398);
    assert!(txout.multi_asset.is_none());
    assert!(txout.datum_hash.is_none());
    assert!(txout.datum.is_none());
    assert!(txout.script_ref.is_none());
}

#[test]
fn test_decode_mempack_txout_tag0_larger_coin() {
    // Another real tag-0 entry: addr=6000d5c82a..., coin=136067723
    let val =
        hex::decode("001d6000d5c82abfa96b4daa29e7ee3ca4a642fa256d3bae3f7a7c1b78ad47008bf5f040")
            .unwrap();
    let (txout, consumed) = decode_mempack_txout(&val).unwrap();
    assert_eq!(consumed, val.len());
    assert_eq!(txout.coin, 136_067_723);
    assert_eq!(txout.address[0], 0x60);
}

#[test]
fn test_decode_mempack_txout_tag2() {
    // Real tag-2 entry from preview tvar:
    let val = hex::decode(
        "02015691d68ad87582fc89b9ac43fd0227cfa4108efb791b9987b290a9ba\
         85b06c5a4edd9c1b857a1b55106ee40191bb091025984a9d01000000\
         f68597d600c99f00",
    )
    .unwrap();
    let (txout, consumed) = decode_mempack_txout(&val).unwrap();
    assert_eq!(consumed, val.len());
    assert_eq!(txout.tag, 2);
    // address = credential_type(1) + hash28(28) = 29 bytes
    assert_eq!(txout.address.len(), 29);
    assert_eq!(txout.address[0], 0x01); // ScriptHash credential
                                        // coin is 0 because we can't extract it from the packed form.
    assert_eq!(txout.coin, 0);
    // Opaque tail should contain the Addr28Extra + CompactCoin data.
    assert!(txout.opaque_tail.is_some());
    assert_eq!(txout.opaque_tail.as_ref().unwrap().len(), val.len() - 30);
}

#[test]
fn test_decode_mempack_txout_tag4_ada_only() {
    // Construct a synthetic tag-4 ADA-only entry:
    // tag(4) + addr_len(29) + addr(29 bytes) + value_tag(0) + coin_varlen + datum
    let mut val = Vec::new();
    val.push(4); // tag
    val.push(29); // addr len VarLen
    val.extend_from_slice(&[0x70; 29]); // 29-byte enterprise script address
    val.push(0); // value tag = 0 (ADA-only)
    val.extend_from_slice(&[0xee, 0xdd, 0x01]); // coin = 28398
    val.extend_from_slice(&[0xd8, 0x79, 0x9f, 0xff]); // 4 bytes of CBOR datum

    let (txout, consumed) = decode_mempack_txout(&val).unwrap();
    assert_eq!(consumed, val.len());
    assert_eq!(txout.tag, 4);
    assert_eq!(txout.address.len(), 29);
    assert_eq!(txout.coin, 28398);
    assert!(txout.multi_asset.is_none());
    let datum = txout.datum.unwrap();
    assert_eq!(datum, &[0xd8, 0x79, 0x9f, 0xff]);
}

#[test]
fn test_decode_mempack_txout_unknown_tag() {
    let data = [0x06]; // tag 6 doesn't exist
    assert!(decode_mempack_txout(&data).is_err());
}

#[test]
fn test_tvar_iterator_fixture() {
    let data = include_bytes!("../../test_fixtures/preview_tvar_head_64k.bin");
    let iter = TvarIterator::new(data).unwrap();
    let mut count = 0;
    let mut tag_counts = [0u32; 6];
    for result in iter {
        match result {
            Ok((txin, txout)) => {
                assert_eq!(txin.txid.as_bytes().len(), 32);
                // For tags 0/1/4/5 the coin should be > 0 OR multi_asset present.
                // For tags 2/3 the coin is 0 (opaque) but opaque_tail is present.
                match txout.tag {
                    0 | 1 => {
                        assert!(
                            txout.coin > 0 || txout.multi_asset.is_some(),
                            "tag {} entry {}: zero coin without multi-asset",
                            txout.tag,
                            count
                        );
                    }
                    2 | 3 => {
                        assert!(txout.opaque_tail.is_some());
                    }
                    4 | 5 => {
                        // Coin may be zero for multi-asset entries, but we still
                        // expect some value data.
                        assert!(
                            txout.coin > 0
                                || txout.multi_asset.is_some()
                                || txout.opaque_tail.is_some()
                        );
                    }
                    _ => panic!("unexpected tag {}", txout.tag),
                }
                if txout.tag < 6 {
                    tag_counts[txout.tag as usize] += 1;
                }
                count += 1;
            }
            Err(e) => panic!("iteration error at entry {count}: {e}"),
        }
    }

    // The 64KB fixture holds ~400 entries (last one may be truncated and
    // silently skipped by the iterator).
    assert!(
        count >= 350,
        "expected >= 350 entries in 64KB fixture, got {count}"
    );

    // Verify we see multiple tag variants.
    assert!(tag_counts[0] > 100, "expected many tag-0 entries");
    assert!(tag_counts[2] > 50, "expected many tag-2 entries");
    assert!(tag_counts[4] > 30, "expected many tag-4 entries");

    eprintln!(
        "tvar iterator: {count} entries, tags: [0]={}, [1]={}, [2]={}, [3]={}, [4]={}, [5]={}",
        tag_counts[0], tag_counts[1], tag_counts[2], tag_counts[3], tag_counts[4], tag_counts[5]
    );
}

#[test]
fn test_tvar_iterator_empty() {
    assert!(TvarIterator::new(&[]).is_err());
}

#[test]
fn test_tvar_iterator_truncated_header() {
    // Just array(1) without the map.
    assert!(TvarIterator::new(&[0x81]).is_err());
}

#[test]
fn test_tvar_iterator_immediate_break() {
    // array(1) + map(indef) + break byte.
    let data = [0x81, 0xbf, 0xff];
    let iter = TvarIterator::new(&data).unwrap();
    let entries: Vec<_> = iter.collect();
    assert!(entries.is_empty());
}
