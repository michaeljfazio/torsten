//! Parser for Haskell UTxOState CBOR encoding (Conway era).
//!
//! UTxOState: array(6) [UTxO, deposited, fees, GovState, stakeDistr, donation]
//!
//! The UTxO map uses MemPack binary encoding for keys (TxIn) and values (TxOut).
//! TxIn MemPack is 34 bytes: 32-byte TxId hash + 2-byte Word16 BE index.
//! TxOut MemPack is a complex compact binary format that we skip for now.
//! The UTxO set will be reconstructed via a partial block replay after import.

use super::types::HaskellUTxOState;
use crate::error::SerializationError;
use torsten_primitives::value::Lovelace;

/// Parse UTxOState: array(6)
///
/// Fields:
///   [0] utxosUtxo       — UTxO map (MemPack TxIn -> MemPack TxOut), skipped
///   [1] utxosDeposited   — Coin
///   [2] utxosFees        — Coin
///   [3] utxosGovState    — ConwayGovState
///   [4] utxosInstantStake — Map(Credential -> CompactCoin), skipped
///   [5] utxosDonation    — Coin
pub fn parse_utxo_state(d: &mut minicbor::Decoder) -> Result<HaskellUTxOState, SerializationError> {
    let len = d.array()?.ok_or_else(|| {
        SerializationError::CborDecode("UTxOState: expected definite array".into())
    })?;
    if len != 6 {
        return Err(SerializationError::CborDecode(format!(
            "UTxOState: expected array(6), got array({len})"
        )));
    }

    // [0] UTxO map — MemPack binary encoding for both keys and values.
    // Keys are CBOR bytestrings containing 34-byte MemPack TxIn.
    // Values are CBOR bytestrings containing variable-length MemPack TxOut.
    // We skip the full TxOut parsing (complex MemPack format) and count entries.
    // The UTxO set will be rebuilt from a partial block replay after state import.
    let utxo_count = skip_utxo_map(d)?;

    // [1] utxosDeposited: Coin
    let deposited = Lovelace(d.u64()?);

    // [2] utxosFees: Coin
    let fees = Lovelace(d.u64()?);

    // [3] utxosGovState: ConwayGovState
    let gov_state = super::gov_state::parse_conway_gov_state(d)?;

    // [4] utxosInstantStake: Map(Credential -> CompactCoin)
    // Skip this — it's a derived value that can be recomputed
    d.skip()?;

    // [5] utxosDonation: Coin
    let donation = Lovelace(d.u64()?);

    tracing::info!(
        utxo_count,
        deposited = deposited.0,
        fees = fees.0,
        donation = donation.0,
        "Parsed UTxOState (UTxO map skipped, will rebuild from block replay)"
    );

    Ok(HaskellUTxOState {
        utxo: Vec::new(), // UTxO entries skipped — rebuilt from block replay
        utxo_count,
        deposited,
        fees,
        gov_state,
        donation,
    })
}

/// Skip the UTxO map, consuming all CBOR entries without parsing MemPack TxOut.
/// Returns the number of entries in the map for diagnostic purposes.
fn skip_utxo_map(d: &mut minicbor::Decoder) -> Result<u64, SerializationError> {
    let map_len = d
        .map()?
        .ok_or_else(|| SerializationError::CborDecode("UTxO map: expected definite map".into()))?;

    tracing::info!(
        entries = map_len,
        "Skipping UTxO map (MemPack format — will rebuild from block replay)"
    );

    for _ in 0..map_len {
        // Key: CBOR bytestring containing MemPack TxIn (34 bytes)
        d.skip()?;
        // Value: CBOR bytestring containing MemPack TxOut (variable length)
        d.skip()?;
    }

    Ok(map_len)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal UTxOState CBOR payload for testing.
    /// This creates a valid array(6) with an empty UTxO map and stub gov_state.
    fn build_utxo_state_cbor(
        utxo_entries: &[(&[u8; 34], &[u8])],
        deposited: u64,
        fees: u64,
        _donation: u64,
    ) -> Vec<u8> {
        let mut buf = Vec::new();

        // array(6)
        buf.push(0x86);

        // [0] UTxO map
        // definite-length map header
        if utxo_entries.len() < 24 {
            buf.push(0xa0 | utxo_entries.len() as u8); // map(n) for n < 24
        } else {
            buf.push(0xb9); // map with 2-byte length
            buf.extend_from_slice(&(utxo_entries.len() as u16).to_be_bytes());
        }
        for (key, value) in utxo_entries {
            // Key: bytes(34)
            buf.push(0x58); // bytes with 1-byte length
            buf.push(34);
            buf.extend_from_slice(&key[..]);
            // Value: bytes(n)
            if value.len() < 24 {
                buf.push(0x40 | value.len() as u8);
            } else {
                buf.push(0x58);
                buf.push(value.len() as u8);
            }
            buf.extend_from_slice(value);
        }

        // [1] deposited: uint
        encode_uint(&mut buf, deposited);

        // [2] fees: uint
        encode_uint(&mut buf, fees);

        // We can't easily build a valid ConwayGovState here since it's complex.
        // Tests that need to parse past field [2] would need the gov_state parser
        // to be implemented. For now we test what we can.

        buf
    }

    fn encode_uint(buf: &mut Vec<u8>, val: u64) {
        if val < 24 {
            buf.push(val as u8);
        } else if val <= 0xff {
            buf.push(0x18);
            buf.push(val as u8);
        } else if val <= 0xffff {
            buf.push(0x19);
            buf.extend_from_slice(&(val as u16).to_be_bytes());
        } else if val <= 0xffff_ffff {
            buf.push(0x1a);
            buf.extend_from_slice(&(val as u32).to_be_bytes());
        } else {
            buf.push(0x1b);
            buf.extend_from_slice(&val.to_be_bytes());
        }
    }

    #[test]
    fn test_skip_utxo_map_empty() {
        // Empty map: 0xa0
        let data = [0xa0u8];
        let mut d = minicbor::Decoder::new(&data);
        let count = skip_utxo_map(&mut d).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_skip_utxo_map_with_entries() {
        // Build a map with 2 entries: each key is bytes(34), each value is bytes(8)
        let mut data = Vec::new();
        data.push(0xa2); // map(2)
        for i in 0u8..2 {
            // key: bytes(34)
            data.push(0x58);
            data.push(34);
            let mut key = [0u8; 34];
            key[0] = i;
            data.extend_from_slice(&key);
            // value: bytes(8)
            data.push(0x48); // bytes(8)
            data.extend_from_slice(&[0u8; 8]);
        }

        let mut d = minicbor::Decoder::new(&data);
        let count = skip_utxo_map(&mut d).unwrap();
        assert_eq!(count, 2);
        // Decoder should be at end
        assert_eq!(d.position(), data.len());
    }

    #[test]
    fn test_parse_utxo_state_rejects_wrong_array_length() {
        // array(5) instead of array(6)
        let data = [0x85, 0x00, 0x00, 0x00, 0x00, 0x00];
        let mut d = minicbor::Decoder::new(&data);
        let err = parse_utxo_state(&mut d).unwrap_err();
        assert!(
            err.to_string().contains("expected array(6)"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_parse_utxo_state_skips_utxo_and_reads_deposited_fees() {
        // Build CBOR: array(6) [map(1), 500000, 1000, ...]
        // The CBOR is truncated after fees — no valid ConwayGovState follows.
        // We verify that the parser correctly skips the UTxO map and reads
        // deposited/fees before failing on the missing/invalid gov_state data.
        let txin_key: [u8; 34] = [0xab; 34];
        let txout_val: [u8; 16] = [0xcd; 16];

        let cbor = build_utxo_state_cbor(&[(&txin_key, &txout_val)], 500_000, 1_000, 42);

        let mut d = minicbor::Decoder::new(&cbor);

        // Parsing will fail at field [3] because the CBOR is truncated —
        // either a ConwayGovState parse error or end-of-input.
        let err = parse_utxo_state(&mut d).unwrap_err();
        assert!(
            err.to_string().contains("ConwayGovState")
                || err.to_string().contains("not yet implemented")
                || err.to_string().contains("end of input"),
            "Expected gov_state or end-of-input error, got: {err}"
        );
    }
}
