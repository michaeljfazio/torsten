use dugite_primitives::hash::{blake2b_224, Hash28};
use ed25519_dalek::{SigningKey, VerifyingKey};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum KeyError {
    #[error("Invalid key length: expected {expected}, got {got}")]
    InvalidLength { expected: usize, got: usize },
    #[error("Invalid key bytes")]
    InvalidBytes,
    #[error("Signature verification failed")]
    VerificationFailed,
    #[error("Ed25519 error: {0}")]
    Ed25519(#[from] ed25519_dalek::SignatureError),
}

/// Ed25519 signing (private) key
#[derive(Clone)]
pub struct PaymentSigningKey {
    inner: SigningKey,
}

/// Ed25519 verification (public) key
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentVerificationKey {
    inner: VerifyingKey,
}

impl PaymentSigningKey {
    pub fn generate() -> Self {
        let mut seed = [0u8; 32];
        rand::rng().fill_bytes(&mut seed);
        PaymentSigningKey {
            inner: SigningKey::from_bytes(&seed),
        }
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, KeyError> {
        if bytes.len() != 32 {
            return Err(KeyError::InvalidLength {
                expected: 32,
                got: bytes.len(),
            });
        }
        let mut key_bytes = [0u8; 32];
        key_bytes.copy_from_slice(bytes);
        Ok(PaymentSigningKey {
            inner: SigningKey::from_bytes(&key_bytes),
        })
    }

    /// Create a signing key from 32 bytes (seed) or 64 bytes (seed + public key).
    ///
    /// For 32 bytes: used directly as an ed25519-dalek seed (SHA-512 hashed
    /// internally to derive the scalar).
    ///
    /// For 64 bytes: the first 32 bytes are used as the seed; bytes 32-63 are
    /// the public key which is derived automatically and ignored.
    ///
    /// **Important:** This function handles standard Ed25519 seed-based keys.
    /// It does NOT support Cardano Ed25519-BIP32 extended keys where the first
    /// 32 bytes are a pre-clamped private scalar (not a seed).  For BIP32
    /// extended signing keys (e.g., from `cardano-cli` `PaymentExtendedSigningKeyShelley_ed25519_bip32`),
    /// the scalar must be used directly without SHA-512 hashing, which requires
    /// the `ed25519-bip32` crate or equivalent low-level scalar construction.
    pub fn from_extended_bytes(bytes: &[u8]) -> Result<Self, KeyError> {
        match bytes.len() {
            32 | 64 => Self::from_bytes(&bytes[..32]),
            other => Err(KeyError::InvalidLength {
                expected: 32,
                got: other,
            }),
        }
    }

    pub fn to_bytes(&self) -> [u8; 32] {
        self.inner.to_bytes()
    }

    pub fn verification_key(&self) -> PaymentVerificationKey {
        PaymentVerificationKey {
            inner: self.inner.verifying_key(),
        }
    }

    pub fn sign(&self, message: &[u8]) -> Vec<u8> {
        use ed25519_dalek::Signer;
        self.inner.sign(message).to_bytes().to_vec()
    }
}

impl PaymentVerificationKey {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, KeyError> {
        if bytes.len() != 32 {
            return Err(KeyError::InvalidLength {
                expected: 32,
                got: bytes.len(),
            });
        }
        // Safety: bytes.len() == 32 is verified above, so try_into cannot fail
        let vk = VerifyingKey::from_bytes(bytes.try_into().expect("32-byte slice"))?;
        Ok(PaymentVerificationKey { inner: vk })
    }

    pub fn to_bytes(&self) -> [u8; 32] {
        self.inner.to_bytes()
    }

    /// Hash to get the verification key hash (credential)
    pub fn hash(&self) -> Hash28 {
        blake2b_224(&self.to_bytes())
    }

    pub fn verify(&self, message: &[u8], signature: &[u8]) -> Result<(), KeyError> {
        use ed25519_dalek::{Signature, Verifier};
        if signature.len() != 64 {
            return Err(KeyError::InvalidLength {
                expected: 64,
                got: signature.len(),
            });
        }
        // Safety: signature.len() == 64 is verified above, so try_into cannot fail
        let sig = Signature::from_bytes(signature.try_into().expect("64-byte slice"));
        self.inner.verify(message, &sig)?;
        Ok(())
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.to_bytes())
    }
}

/// Stake signing key (same as payment, different semantic use)
pub type StakeSigningKey = PaymentSigningKey;
pub type StakeVerificationKey = PaymentVerificationKey;

/// Cardano key envelope format (used in .skey and .vkey files)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextEnvelope {
    #[serde(rename = "type")]
    pub type_: String,
    pub description: String,
    #[serde(rename = "cborHex")]
    pub cbor_hex: String,
}

impl TextEnvelope {
    pub fn payment_signing_key(key: &PaymentSigningKey) -> Self {
        let cbor_bytes = simple_cbor_wrap(&key.to_bytes());
        TextEnvelope {
            type_: "PaymentSigningKeyShelley_ed25519".to_string(),
            description: "Payment Signing Key".to_string(),
            cbor_hex: hex::encode(cbor_bytes),
        }
    }

    pub fn payment_verification_key(key: &PaymentVerificationKey) -> Self {
        let cbor_bytes = simple_cbor_wrap(&key.to_bytes());
        TextEnvelope {
            type_: "PaymentVerificationKeyShelley_ed25519".to_string(),
            description: "Payment Verification Key".to_string(),
            cbor_hex: hex::encode(cbor_bytes),
        }
    }

    pub fn stake_signing_key(key: &StakeSigningKey) -> Self {
        let cbor_bytes = simple_cbor_wrap(&key.to_bytes());
        TextEnvelope {
            type_: "StakeSigningKeyShelley_ed25519".to_string(),
            description: "Stake Signing Key".to_string(),
            cbor_hex: hex::encode(cbor_bytes),
        }
    }

    pub fn stake_verification_key(key: &StakeVerificationKey) -> Self {
        let cbor_bytes = simple_cbor_wrap(&key.to_bytes());
        TextEnvelope {
            type_: "StakeVerificationKeyShelley_ed25519".to_string(),
            description: "Stake Verification Key".to_string(),
            cbor_hex: hex::encode(cbor_bytes),
        }
    }
}

impl TextEnvelope {
    /// Read a text envelope from a file path.
    ///
    /// Parses the JSON text envelope format used by cardano-cli for key and
    /// certificate files (`{ "type": "...", "description": "...", "cborHex": "..." }`).
    pub fn from_file(path: &std::path::Path) -> Result<Self, TextEnvelopeError> {
        let content = std::fs::read_to_string(path).map_err(|e| TextEnvelopeError::Io {
            path: path.display().to_string(),
            source: e,
        })?;
        serde_json::from_str(&content).map_err(|e| TextEnvelopeError::Json {
            path: path.display().to_string(),
            source: e,
        })
    }

    /// Decode the `cborHex` field and unwrap the outer CBOR byte string header,
    /// returning the raw payload bytes.
    pub fn raw_cbor_bytes(&self) -> Result<Vec<u8>, TextEnvelopeError> {
        let cbor = hex::decode(&self.cbor_hex).map_err(TextEnvelopeError::Hex)?;
        Ok(unwrap_cbor_bytestring(&cbor).to_vec())
    }

    /// Check that the envelope `type` field matches an expected value.
    ///
    /// Returns `Ok(())` on match, or a descriptive error on mismatch.
    pub fn expect_type(&self, expected: &str) -> Result<(), TextEnvelopeError> {
        if self.type_ == expected {
            Ok(())
        } else {
            Err(TextEnvelopeError::TypeMismatch {
                expected: expected.to_string(),
                actual: self.type_.clone(),
            })
        }
    }

    /// Check that the envelope `type` field matches one of several expected values.
    pub fn expect_type_one_of(&self, expected: &[&str]) -> Result<(), TextEnvelopeError> {
        if expected.iter().any(|e| self.type_ == *e) {
            Ok(())
        } else {
            Err(TextEnvelopeError::TypeMismatch {
                expected: expected.join(" or "),
                actual: self.type_.clone(),
            })
        }
    }
}

/// Errors from text envelope parsing.
#[derive(Debug)]
pub enum TextEnvelopeError {
    /// File I/O error.
    Io {
        path: String,
        source: std::io::Error,
    },
    /// JSON parse error.
    Json {
        path: String,
        source: serde_json::Error,
    },
    /// Hex decode error.
    Hex(hex::FromHexError),
    /// Envelope type doesn't match expected key type.
    TypeMismatch { expected: String, actual: String },
}

impl std::fmt::Display for TextEnvelopeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "failed to read {path}: {source}"),
            Self::Json { path, source } => {
                write!(f, "invalid text envelope JSON in {path}: {source}")
            }
            Self::Hex(e) => write!(f, "invalid cborHex: {e}"),
            Self::TypeMismatch { expected, actual } => {
                write!(
                    f,
                    "wrong text envelope type: expected {expected}, got {actual}"
                )
            }
        }
    }
}

impl std::error::Error for TextEnvelopeError {}

/// Unwrap a single CBOR byte string header, returning the inner payload.
///
/// Handles all CBOR major-type-2 length forms (tiny, 1-byte, 2-byte, 4-byte).
/// If the data doesn't look like a CBOR byte string, returns it unchanged.
fn unwrap_cbor_bytestring(data: &[u8]) -> &[u8] {
    if data.is_empty() {
        return data;
    }
    match data[0] {
        // 1-byte length prefix (0x58 LL)
        0x58 if data.len() > 2 => &data[2..],
        // 2-byte length prefix (0x59 HH LL)
        0x59 if data.len() > 3 => &data[3..],
        // 4-byte length prefix (0x5a HH HH LL LL)
        0x5a if data.len() > 5 => &data[5..],
        // Tiny byte string (0x40..0x57 — length encoded in lower 5 bits)
        b if (b & 0xe0) == 0x40 && data.len() > 1 => &data[1..],
        _ => data,
    }
}

/// Wrap raw key bytes in a simple CBOR byte string
fn simple_cbor_wrap(data: &[u8]) -> Vec<u8> {
    let mut result = Vec::new();
    // CBOR byte string (major type 2)
    if data.len() < 24 {
        result.push(0x40 | data.len() as u8);
    } else if data.len() < 256 {
        result.push(0x58);
        result.push(data.len() as u8);
    } else {
        result.push(0x59);
        result.extend_from_slice(&(data.len() as u16).to_be_bytes());
    }
    result.extend_from_slice(data);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_generation_and_signing() {
        let sk = PaymentSigningKey::generate();
        let vk = sk.verification_key();

        let message = b"hello dugite";
        let signature = sk.sign(message);

        assert!(vk.verify(message, &signature).is_ok());
        assert!(vk.verify(b"wrong message", &signature).is_err());
    }

    #[test]
    fn test_key_hash() {
        let sk = PaymentSigningKey::generate();
        let vk = sk.verification_key();
        let hash = vk.hash();
        assert_eq!(hash.as_bytes().len(), 28);
    }

    #[test]
    fn test_text_envelope_roundtrip() {
        let sk = PaymentSigningKey::generate();
        let envelope = TextEnvelope::payment_signing_key(&sk);
        let json = serde_json::to_string_pretty(&envelope).unwrap();
        let recovered: TextEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(envelope.type_, recovered.type_);
    }
}
