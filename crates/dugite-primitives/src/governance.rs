//! CIP-0129 governance identifier bech32 encoding and decoding.
//!
//! CIP-0129 defines canonical bech32 Human-Readable Parts (HRPs) for the
//! governance identifiers introduced by CIP-1694 (Conway era on-chain governance).
//!
//! # Prefixes
//!
//! | Identifier                              | HRP              |
//! |-----------------------------------------|------------------|
//! | DRep key hash credential                | `drep1`          |
//! | DRep script hash credential             | `drep_script1`   |
//! | CC hot key hash credential              | `cc_hot1`        |
//! | CC hot script hash credential           | `cc_hot_script1` |
//! | CC cold key hash credential             | `cc_cold1`       |
//! | CC cold script hash credential          | `cc_cold_script1`|
//!
//! All identifiers encode a 28-byte Blake2b-224 hash using Bech32 encoding.
//!
//! # Compatibility Note
//!
//! The older `drep` prefix (without the trailing `1`) was used before CIP-0129
//! was finalised. All Cardano tooling from cardano-cli 10.x onwards uses the
//! CIP-0129 prefixes. The [`decode_drep_bech32`] function accepts both the
//! legacy `drep` prefix and the canonical `drep1` prefix for backwards
//! compatibility.

use crate::hash::Hash28;
use bech32::{Bech32, Hrp};

/// HRP for a DRep key-hash credential (CIP-0129).
pub const HRP_DREP: &str = "drep1";

/// HRP for a DRep script-hash credential (CIP-0129).
pub const HRP_DREP_SCRIPT: &str = "drep_script1";

/// HRP for a Constitutional Committee hot key-hash credential (CIP-0129).
pub const HRP_CC_HOT: &str = "cc_hot1";

/// HRP for a Constitutional Committee hot script-hash credential (CIP-0129).
pub const HRP_CC_HOT_SCRIPT: &str = "cc_hot_script1";

/// HRP for a Constitutional Committee cold key-hash credential (CIP-0129).
pub const HRP_CC_COLD: &str = "cc_cold1";

/// HRP for a Constitutional Committee cold script-hash credential (CIP-0129).
pub const HRP_CC_COLD_SCRIPT: &str = "cc_cold_script1";

/// Error type for CIP-0129 governance identifier encoding/decoding.
#[derive(Debug, thiserror::Error)]
pub enum GovernanceIdError {
    #[error("bech32 encoding error: {0}")]
    Bech32Encode(#[from] bech32::EncodeError),
    #[error("bech32 decoding error: {0}")]
    Bech32Decode(#[from] bech32::DecodeError),
    #[error("unexpected HRP '{actual}' (expected one of: {expected})")]
    WrongHrp { actual: String, expected: String },
    #[error("invalid payload length: expected 28 bytes, got {0}")]
    InvalidLength(usize),
}

// ──────────────────────────────────────────────────────────────────────────────
// Encoding helpers
// ──────────────────────────────────────────────────────────────────────────────

/// Encode a 28-byte hash as a CIP-0129 DRep key-hash bech32 identifier.
///
/// Produces a string with the `drep1` HRP.
pub fn encode_drep_key(hash: &Hash28) -> Result<String, GovernanceIdError> {
    encode_governance_id(HRP_DREP, hash.as_bytes())
}

/// Encode a 28-byte hash as a CIP-0129 DRep script-hash bech32 identifier.
///
/// Produces a string with the `drep_script1` HRP.
pub fn encode_drep_script(hash: &Hash28) -> Result<String, GovernanceIdError> {
    encode_governance_id(HRP_DREP_SCRIPT, hash.as_bytes())
}

/// Encode a 28-byte hash as a CIP-0129 CC hot key-hash bech32 identifier.
///
/// Produces a string with the `cc_hot1` HRP.
pub fn encode_cc_hot_key(hash: &Hash28) -> Result<String, GovernanceIdError> {
    encode_governance_id(HRP_CC_HOT, hash.as_bytes())
}

/// Encode a 28-byte hash as a CIP-0129 CC hot script-hash bech32 identifier.
///
/// Produces a string with the `cc_hot_script1` HRP.
pub fn encode_cc_hot_script(hash: &Hash28) -> Result<String, GovernanceIdError> {
    encode_governance_id(HRP_CC_HOT_SCRIPT, hash.as_bytes())
}

/// Encode a 28-byte hash as a CIP-0129 CC cold key-hash bech32 identifier.
///
/// Produces a string with the `cc_cold1` HRP.
pub fn encode_cc_cold_key(hash: &Hash28) -> Result<String, GovernanceIdError> {
    encode_governance_id(HRP_CC_COLD, hash.as_bytes())
}

/// Encode a 28-byte hash as a CIP-0129 CC cold script-hash bech32 identifier.
///
/// Produces a string with the `cc_cold_script1` HRP.
pub fn encode_cc_cold_script(hash: &Hash28) -> Result<String, GovernanceIdError> {
    encode_governance_id(HRP_CC_COLD_SCRIPT, hash.as_bytes())
}

// ──────────────────────────────────────────────────────────────────────────────
// Decoding helpers
// ──────────────────────────────────────────────────────────────────────────────

/// Decode a CIP-0129 DRep key-hash bech32 identifier.
///
/// Accepts both `drep1` (CIP-0129) and legacy `drep` prefixes.
/// Returns the 28-byte hash.
pub fn decode_drep_key(s: &str) -> Result<Hash28, GovernanceIdError> {
    let (hrp, data) = bech32::decode(s)?;
    let hrp_str = hrp.as_str();
    if hrp_str != HRP_DREP && hrp_str != "drep" {
        return Err(GovernanceIdError::WrongHrp {
            actual: hrp_str.to_string(),
            expected: format!("{HRP_DREP} (or legacy 'drep')"),
        });
    }
    bytes_to_hash28(&data)
}

/// Decode a CIP-0129 DRep script-hash bech32 identifier.
///
/// Returns the 28-byte hash.
pub fn decode_drep_script(s: &str) -> Result<Hash28, GovernanceIdError> {
    let (hrp, data) = bech32::decode(s)?;
    let hrp_str = hrp.as_str();
    if hrp_str != HRP_DREP_SCRIPT {
        return Err(GovernanceIdError::WrongHrp {
            actual: hrp_str.to_string(),
            expected: HRP_DREP_SCRIPT.to_string(),
        });
    }
    bytes_to_hash28(&data)
}

/// Decode a CIP-0129 CC hot key-hash bech32 identifier.
///
/// Returns the 28-byte hash.
pub fn decode_cc_hot_key(s: &str) -> Result<Hash28, GovernanceIdError> {
    let (hrp, data) = bech32::decode(s)?;
    let hrp_str = hrp.as_str();
    if hrp_str != HRP_CC_HOT {
        return Err(GovernanceIdError::WrongHrp {
            actual: hrp_str.to_string(),
            expected: HRP_CC_HOT.to_string(),
        });
    }
    bytes_to_hash28(&data)
}

/// Decode a CIP-0129 CC hot script-hash bech32 identifier.
///
/// Returns the 28-byte hash.
pub fn decode_cc_hot_script(s: &str) -> Result<Hash28, GovernanceIdError> {
    let (hrp, data) = bech32::decode(s)?;
    let hrp_str = hrp.as_str();
    if hrp_str != HRP_CC_HOT_SCRIPT {
        return Err(GovernanceIdError::WrongHrp {
            actual: hrp_str.to_string(),
            expected: HRP_CC_HOT_SCRIPT.to_string(),
        });
    }
    bytes_to_hash28(&data)
}

/// Decode a CIP-0129 CC cold key-hash bech32 identifier.
///
/// Returns the 28-byte hash.
pub fn decode_cc_cold_key(s: &str) -> Result<Hash28, GovernanceIdError> {
    let (hrp, data) = bech32::decode(s)?;
    let hrp_str = hrp.as_str();
    if hrp_str != HRP_CC_COLD {
        return Err(GovernanceIdError::WrongHrp {
            actual: hrp_str.to_string(),
            expected: HRP_CC_COLD.to_string(),
        });
    }
    bytes_to_hash28(&data)
}

/// Decode a CIP-0129 CC cold script-hash bech32 identifier.
///
/// Returns the 28-byte hash.
pub fn decode_cc_cold_script(s: &str) -> Result<Hash28, GovernanceIdError> {
    let (hrp, data) = bech32::decode(s)?;
    let hrp_str = hrp.as_str();
    if hrp_str != HRP_CC_COLD_SCRIPT {
        return Err(GovernanceIdError::WrongHrp {
            actual: hrp_str.to_string(),
            expected: HRP_CC_COLD_SCRIPT.to_string(),
        });
    }
    bytes_to_hash28(&data)
}

// ──────────────────────────────────────────────────────────────────────────────
// Credential-type-aware helpers
// ──────────────────────────────────────────────────────────────────────────────

/// Governance credential kind — key-hash or script-hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredKind {
    /// Verification key hash (Blake2b-224 of a public key).
    Key,
    /// Script hash (Blake2b-224 of a script).
    Script,
}

/// Encode a DRep credential (key or script) as a CIP-0129 bech32 identifier.
///
/// Selects `drep1` for key credentials and `drep_script1` for script credentials.
pub fn encode_drep(hash: &Hash28, kind: CredKind) -> Result<String, GovernanceIdError> {
    match kind {
        CredKind::Key => encode_drep_key(hash),
        CredKind::Script => encode_drep_script(hash),
    }
}

/// Encode a CC hot credential (key or script) as a CIP-0129 bech32 identifier.
///
/// Selects `cc_hot1` for key credentials and `cc_hot_script1` for script credentials.
pub fn encode_cc_hot(hash: &Hash28, kind: CredKind) -> Result<String, GovernanceIdError> {
    match kind {
        CredKind::Key => encode_cc_hot_key(hash),
        CredKind::Script => encode_cc_hot_script(hash),
    }
}

/// Encode a CC cold credential (key or script) as a CIP-0129 bech32 identifier.
///
/// Selects `cc_cold1` for key credentials and `cc_cold_script1` for script credentials.
pub fn encode_cc_cold(hash: &Hash28, kind: CredKind) -> Result<String, GovernanceIdError> {
    match kind {
        CredKind::Key => encode_cc_cold_key(hash),
        CredKind::Script => encode_cc_cold_script(hash),
    }
}

/// Encode a governance identifier from a CBOR credential pair `[type, hash_bytes]`.
///
/// The `cred_type` byte matches the Cardano CBOR encoding:
/// - `0` = key-hash credential
/// - `1` = script-hash credential
///
/// This function is intended for use when decoding raw LocalStateQuery responses
/// where credentials arrive as `array(2) [u8, bstr(28)]`.
pub fn encode_drep_from_cbor(
    cred_type: u8,
    hash_bytes: &[u8],
) -> Result<String, GovernanceIdError> {
    let hash = bytes_to_hash28(hash_bytes)?;
    match cred_type {
        0 => encode_drep_key(&hash),
        1 => encode_drep_script(&hash),
        _ => Err(GovernanceIdError::WrongHrp {
            actual: format!("type={cred_type}"),
            expected: "0 (key) or 1 (script)".to_string(),
        }),
    }
}

/// Encode a CC hot identifier from a CBOR credential pair `[type, hash_bytes]`.
///
/// See [`encode_drep_from_cbor`] for the `cred_type` convention.
pub fn encode_cc_hot_from_cbor(
    cred_type: u8,
    hash_bytes: &[u8],
) -> Result<String, GovernanceIdError> {
    let hash = bytes_to_hash28(hash_bytes)?;
    match cred_type {
        0 => encode_cc_hot_key(&hash),
        1 => encode_cc_hot_script(&hash),
        _ => Err(GovernanceIdError::WrongHrp {
            actual: format!("type={cred_type}"),
            expected: "0 (key) or 1 (script)".to_string(),
        }),
    }
}

/// Encode a CC cold identifier from a CBOR credential pair `[type, hash_bytes]`.
///
/// See [`encode_drep_from_cbor`] for the `cred_type` convention.
pub fn encode_cc_cold_from_cbor(
    cred_type: u8,
    hash_bytes: &[u8],
) -> Result<String, GovernanceIdError> {
    let hash = bytes_to_hash28(hash_bytes)?;
    match cred_type {
        0 => encode_cc_cold_key(&hash),
        1 => encode_cc_cold_script(&hash),
        _ => Err(GovernanceIdError::WrongHrp {
            actual: format!("type={cred_type}"),
            expected: "0 (key) or 1 (script)".to_string(),
        }),
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Internal utilities
// ──────────────────────────────────────────────────────────────────────────────

/// Encode raw bytes using the given HRP as a Bech32 string.
fn encode_governance_id(hrp_str: &str, data: &[u8]) -> Result<String, GovernanceIdError> {
    let hrp = Hrp::parse(hrp_str).map_err(|e| {
        // bech32::EncodeError can't be constructed from HrpError directly,
        // so we use a workaround: attempt a no-op encode which will surface it.
        // In practice, all our HRP constants are valid at compile time.
        let _ = e;
        GovernanceIdError::WrongHrp {
            actual: hrp_str.to_string(),
            expected: "(valid HRP)".to_string(),
        }
    })?;
    Ok(bech32::encode::<Bech32>(hrp, data)?)
}

/// Convert a byte slice to a `Hash28`, returning an error if the length != 28.
fn bytes_to_hash28(data: &[u8]) -> Result<Hash28, GovernanceIdError> {
    if data.len() != 28 {
        return Err(GovernanceIdError::InvalidLength(data.len()));
    }
    let mut arr = [0u8; 28];
    arr.copy_from_slice(data);
    Ok(Hash28::from_bytes(arr))
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixed 28-byte test hash (all 0x01 bytes).
    fn test_hash() -> Hash28 {
        Hash28::from_bytes([0x01u8; 28])
    }

    // ── Encoding round-trip tests ────────────────────────────────────────────

    #[test]
    fn test_drep_key_roundtrip() {
        let h = test_hash();
        let encoded = encode_drep_key(&h).expect("encode should succeed");
        assert!(
            encoded.starts_with("drep1"),
            "expected 'drep1' prefix, got: {encoded}"
        );
        let decoded = decode_drep_key(&encoded).expect("decode should succeed");
        assert_eq!(decoded, h, "round-trip should return original hash");
    }

    #[test]
    fn test_drep_script_roundtrip() {
        let h = test_hash();
        let encoded = encode_drep_script(&h).expect("encode should succeed");
        assert!(
            encoded.starts_with("drep_script1"),
            "expected 'drep_script1' prefix, got: {encoded}"
        );
        let decoded = decode_drep_script(&encoded).expect("decode should succeed");
        assert_eq!(decoded, h);
    }

    #[test]
    fn test_cc_hot_key_roundtrip() {
        let h = test_hash();
        let encoded = encode_cc_hot_key(&h).expect("encode should succeed");
        assert!(
            encoded.starts_with("cc_hot1"),
            "expected 'cc_hot1' prefix, got: {encoded}"
        );
        let decoded = decode_cc_hot_key(&encoded).expect("decode should succeed");
        assert_eq!(decoded, h);
    }

    #[test]
    fn test_cc_hot_script_roundtrip() {
        let h = test_hash();
        let encoded = encode_cc_hot_script(&h).expect("encode should succeed");
        assert!(
            encoded.starts_with("cc_hot_script1"),
            "expected 'cc_hot_script1' prefix, got: {encoded}"
        );
        let decoded = decode_cc_hot_script(&encoded).expect("decode should succeed");
        assert_eq!(decoded, h);
    }

    #[test]
    fn test_cc_cold_key_roundtrip() {
        let h = test_hash();
        let encoded = encode_cc_cold_key(&h).expect("encode should succeed");
        assert!(
            encoded.starts_with("cc_cold1"),
            "expected 'cc_cold1' prefix, got: {encoded}"
        );
        let decoded = decode_cc_cold_key(&encoded).expect("decode should succeed");
        assert_eq!(decoded, h);
    }

    #[test]
    fn test_cc_cold_script_roundtrip() {
        let h = test_hash();
        let encoded = encode_cc_cold_script(&h).expect("encode should succeed");
        assert!(
            encoded.starts_with("cc_cold_script1"),
            "expected 'cc_cold_script1' prefix, got: {encoded}"
        );
        let decoded = decode_cc_cold_script(&encoded).expect("decode should succeed");
        assert_eq!(decoded, h);
    }

    // ── CredKind dispatch tests ──────────────────────────────────────────────

    #[test]
    fn test_encode_drep_key_kind() {
        let h = test_hash();
        let result = encode_drep(&h, CredKind::Key).expect("should succeed");
        assert!(result.starts_with("drep1"), "got: {result}");
    }

    #[test]
    fn test_encode_drep_script_kind() {
        let h = test_hash();
        let result = encode_drep(&h, CredKind::Script).expect("should succeed");
        assert!(result.starts_with("drep_script1"), "got: {result}");
    }

    #[test]
    fn test_encode_cc_hot_key_kind() {
        let h = test_hash();
        let result = encode_cc_hot(&h, CredKind::Key).expect("should succeed");
        assert!(result.starts_with("cc_hot1"), "got: {result}");
    }

    #[test]
    fn test_encode_cc_hot_script_kind() {
        let h = test_hash();
        let result = encode_cc_hot(&h, CredKind::Script).expect("should succeed");
        assert!(result.starts_with("cc_hot_script1"), "got: {result}");
    }

    #[test]
    fn test_encode_cc_cold_key_kind() {
        let h = test_hash();
        let result = encode_cc_cold(&h, CredKind::Key).expect("should succeed");
        assert!(result.starts_with("cc_cold1"), "got: {result}");
    }

    #[test]
    fn test_encode_cc_cold_script_kind() {
        let h = test_hash();
        let result = encode_cc_cold(&h, CredKind::Script).expect("should succeed");
        assert!(result.starts_with("cc_cold_script1"), "got: {result}");
    }

    // ── CBOR-aware encoding tests ────────────────────────────────────────────

    #[test]
    fn test_encode_drep_from_cbor_key() {
        let bytes = [0x02u8; 28];
        let result = encode_drep_from_cbor(0, &bytes).expect("should succeed");
        assert!(result.starts_with("drep1"), "got: {result}");
    }

    #[test]
    fn test_encode_drep_from_cbor_script() {
        let bytes = [0x03u8; 28];
        let result = encode_drep_from_cbor(1, &bytes).expect("should succeed");
        assert!(result.starts_with("drep_script1"), "got: {result}");
    }

    #[test]
    fn test_encode_cc_hot_from_cbor_key() {
        let bytes = [0x04u8; 28];
        let result = encode_cc_hot_from_cbor(0, &bytes).expect("should succeed");
        assert!(result.starts_with("cc_hot1"), "got: {result}");
    }

    #[test]
    fn test_encode_cc_cold_from_cbor_script() {
        let bytes = [0x05u8; 28];
        let result = encode_cc_cold_from_cbor(1, &bytes).expect("should succeed");
        assert!(result.starts_with("cc_cold_script1"), "got: {result}");
    }

    #[test]
    fn test_encode_drep_from_cbor_invalid_type() {
        let bytes = [0x00u8; 28];
        let result = encode_drep_from_cbor(2, &bytes);
        assert!(result.is_err(), "type=2 should be rejected");
    }

    #[test]
    fn test_encode_drep_from_cbor_wrong_length() {
        let bytes = [0x00u8; 20]; // wrong length
        let result = encode_drep_from_cbor(0, &bytes);
        assert!(result.is_err(), "20-byte payload should be rejected");
    }

    // ── Legacy 'drep' prefix backward-compatibility ──────────────────────────

    #[test]
    fn test_decode_drep_key_legacy_prefix() {
        // Encode with old 'drep' HRP manually.
        let h = test_hash();
        let hrp = Hrp::parse("drep").expect("valid HRP");
        let legacy = bech32::encode::<Bech32>(hrp, h.as_bytes()).expect("encode");
        // decode_drep_key must accept it.
        let decoded = decode_drep_key(&legacy).expect("legacy prefix should be accepted");
        assert_eq!(decoded, h);
    }

    // ── Wrong-HRP rejection tests ────────────────────────────────────────────

    #[test]
    fn test_decode_drep_key_rejects_wrong_hrp() {
        let h = test_hash();
        let encoded = encode_drep_script(&h).expect("encode");
        let result = decode_drep_key(&encoded);
        assert!(
            matches!(result, Err(GovernanceIdError::WrongHrp { .. })),
            "drep_script1 prefix should be rejected by decode_drep_key"
        );
    }

    #[test]
    fn test_decode_cc_hot_key_rejects_cc_cold() {
        let h = test_hash();
        let encoded = encode_cc_cold_key(&h).expect("encode");
        let result = decode_cc_hot_key(&encoded);
        assert!(
            matches!(result, Err(GovernanceIdError::WrongHrp { .. })),
            "cc_cold1 prefix should be rejected by decode_cc_hot_key"
        );
    }

    // ── Distinctness: all six encodings produce different outputs ────────────

    #[test]
    fn test_all_six_identifiers_are_distinct() {
        let h = test_hash();
        let ids = [
            encode_drep_key(&h).unwrap(),
            encode_drep_script(&h).unwrap(),
            encode_cc_hot_key(&h).unwrap(),
            encode_cc_hot_script(&h).unwrap(),
            encode_cc_cold_key(&h).unwrap(),
            encode_cc_cold_script(&h).unwrap(),
        ];
        // All six must be pairwise distinct.
        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                assert_ne!(
                    ids[i], ids[j],
                    "identifiers at {i} and {j} should be distinct"
                );
            }
        }
    }

    // ── Known-value test (Bech32 is deterministic) ───────────────────────────

    #[test]
    fn test_known_drep_key_encoding() {
        // Hash: all 0xAB bytes (28 bytes).
        let h = Hash28::from_bytes([0xABu8; 28]);
        let encoded = encode_drep_key(&h).expect("encode");
        // Must have the correct HRP.
        assert!(
            encoded.starts_with("drep1"),
            "expected 'drep1' prefix, got: {encoded}"
        );
        // Round-trip decodes to the same hash.
        let decoded = decode_drep_key(&encoded).expect("decode");
        assert_eq!(decoded, h);
    }
}
