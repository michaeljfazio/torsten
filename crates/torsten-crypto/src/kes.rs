//! KES (Key Evolving Signature) implementation for Cardano.
//!
//! Uses pallas-crypto's Sum6Kes implementation (depth-6 binary sum composition
//! over Ed25519), matching cardano-node's KES scheme.

use pallas_crypto::kes::common::PublicKey as KesPublicKey;
use pallas_crypto::kes::errors::Error as PallasKesError;
use pallas_crypto::kes::summed_kes::{Sum6Kes, Sum6KesSig};
use pallas_crypto::kes::traits::{KesSig, KesSk};
use thiserror::Error;

/// KES period (each period is 129600 slots = 36 hours on mainnet)
pub const KES_PERIOD_SLOTS: u64 = 129600;

/// Maximum number of KES evolutions (2^6 - 2 = 62 for Sum6)
pub const MAX_KES_EVOLUTIONS: u64 = 62;

/// Size of a KES secret key in bytes (Sum6Kes)
pub const KES_SECRET_KEY_SIZE: usize = Sum6Kes::SIZE;

/// Size of a KES secret key buffer including period counter
pub const KES_SECRET_KEY_BUFFER_SIZE: usize = Sum6Kes::SIZE + 4;

/// Size of a KES public key in bytes
pub const KES_PUBLIC_KEY_SIZE: usize = 32;

#[derive(Error, Debug)]
pub enum KesError {
    #[error("KES key error: {0}")]
    KeyError(String),
    #[error("KES signature error: {0}")]
    SignatureError(String),
    #[error("KES key cannot evolve further (period {0} >= max {1})")]
    KeyExpired(u64, u64),
    #[error("Invalid KES key size: expected {expected}, got {actual}")]
    InvalidKeySize { expected: usize, actual: usize },
    #[error("KES verification failed: {0}")]
    VerificationFailed(String),
}

impl From<PallasKesError> for KesError {
    fn from(e: PallasKesError) -> Self {
        KesError::KeyError(e.to_string())
    }
}

/// Generate a new KES key pair from a 32-byte seed.
///
/// Returns (secret_key_bytes, public_key_bytes).
/// The secret key bytes include the period counter (SIZE + 4 bytes total).
///
/// IMPORTANT: The pallas Sum6Kes Drop implementation zeroizes the buffer,
/// so we must copy the bytes before the key object is dropped.
pub fn kes_keygen(seed: &[u8; 32]) -> Result<(Vec<u8>, [u8; 32]), KesError> {
    let mut key_buffer = vec![0u8; KES_SECRET_KEY_BUFFER_SIZE];
    let mut seed_copy = *seed;

    let (sk, pk) = Sum6Kes::keygen(&mut key_buffer, &mut seed_copy);

    // Copy bytes BEFORE Drop zeroizes them
    let sk_bytes = sk.as_bytes().to_vec();
    let mut pk_bytes = [0u8; 32];
    pk_bytes.copy_from_slice(pk.as_bytes());

    // sk is dropped here and zeroizes key_buffer, but we already copied
    drop(sk);

    Ok((sk_bytes, pk_bytes))
}

/// Sign a message using a KES secret key.
///
/// Returns the Sum6KesSig and the period at which it was signed.
pub fn kes_sign_message(sk_bytes: &[u8], message: &[u8]) -> Result<(Sum6KesSig, u32), KesError> {
    check_key_size(sk_bytes.len())?;

    let mut sk_copy = sk_bytes.to_vec();
    let sk = Sum6Kes::from_bytes(&mut sk_copy).map_err(|e| KesError::KeyError(e.to_string()))?;

    let period = sk.get_period();
    let sig = sk.sign(message);

    Ok((sig, period))
}

/// Sign a message using a KES secret key and return the signature as raw bytes.
///
/// Returns (signature_bytes, period). The signature is 448 bytes (Sum6KesSig).
pub fn kes_sign_bytes(sk_bytes: &[u8], message: &[u8]) -> Result<(Vec<u8>, u32), KesError> {
    let (sig, period) = kes_sign_message(sk_bytes, message)?;
    Ok((sig.to_bytes().to_vec(), period))
}

/// Evolve a KES secret key to the target period.
///
/// Returns the evolved key bytes. If already at or past the target period, returns as-is.
pub fn kes_evolve_to_period(sk_bytes: &[u8], target_period: u32) -> Result<Vec<u8>, KesError> {
    let current = kes_get_period(sk_bytes)?;
    if current >= target_period {
        return Ok(sk_bytes.to_vec());
    }
    let mut current_sk = sk_bytes.to_vec();
    for _ in current..target_period {
        let (new_sk, _) = kes_update(&current_sk)?;
        current_sk = new_sk;
    }
    Ok(current_sk)
}

/// Verify a KES signature from raw bytes against a public key and message.
///
/// This parses the signature bytes into a Sum6KesSig and verifies it.
pub fn kes_verify_bytes(
    pk_bytes: &[u8; 32],
    period: u32,
    sig_bytes: &[u8],
    message: &[u8],
) -> Result<(), KesError> {
    let sig = Sum6KesSig::from_bytes(sig_bytes)
        .map_err(|e| KesError::VerificationFailed(format!("invalid KES sig bytes: {e}")))?;
    kes_verify(pk_bytes, period, &sig, message)
}

/// Verify a KES signature against a public key and message.
pub fn kes_verify(
    pk_bytes: &[u8; 32],
    period: u32,
    sig: &Sum6KesSig,
    message: &[u8],
) -> Result<(), KesError> {
    let pk = KesPublicKey::from_bytes(pk_bytes)
        .map_err(|e| KesError::VerificationFailed(e.to_string()))?;

    sig.verify(period, &pk, message)
        .map_err(|e| KesError::VerificationFailed(e.to_string()))
}

/// Update (evolve) a KES secret key to the next period.
///
/// Returns (new_key_bytes, new_period).
pub fn kes_update(sk_bytes: &[u8]) -> Result<(Vec<u8>, u32), KesError> {
    check_key_size(sk_bytes.len())?;

    let mut sk_copy = sk_bytes.to_vec();
    let mut sk =
        Sum6Kes::from_bytes(&mut sk_copy).map_err(|e| KesError::KeyError(e.to_string()))?;

    let current_period = sk.get_period();
    if current_period as u64 >= MAX_KES_EVOLUTIONS {
        return Err(KesError::KeyExpired(
            current_period as u64,
            MAX_KES_EVOLUTIONS,
        ));
    }

    sk.update().map_err(|e| KesError::KeyError(e.to_string()))?;
    let new_period = sk.get_period();

    // Copy bytes before Drop zeroizes
    let new_sk_bytes = sk.as_bytes().to_vec();
    drop(sk);

    Ok((new_sk_bytes, new_period))
}

/// Get the KES period for a given slot number.
pub fn kes_period_for_slot(slot: u64) -> u64 {
    slot / KES_PERIOD_SLOTS
}

/// Get the public key from a KES secret key.
pub fn kes_sk_to_pk(sk_bytes: &[u8]) -> Result<[u8; 32], KesError> {
    check_key_size(sk_bytes.len())?;

    let mut sk_copy = sk_bytes.to_vec();
    let sk = Sum6Kes::from_bytes(&mut sk_copy).map_err(|e| KesError::KeyError(e.to_string()))?;

    let pk = sk.to_pk();
    let mut pk_bytes = [0u8; 32];
    pk_bytes.copy_from_slice(pk.as_bytes());

    Ok(pk_bytes)
}

/// Get the current period from a KES secret key.
pub fn kes_get_period(sk_bytes: &[u8]) -> Result<u32, KesError> {
    check_key_size(sk_bytes.len())?;

    let mut sk_copy = sk_bytes.to_vec();
    let sk = Sum6Kes::from_bytes(&mut sk_copy).map_err(|e| KesError::KeyError(e.to_string()))?;

    Ok(sk.get_period())
}

fn check_key_size(len: usize) -> Result<(), KesError> {
    if len != KES_SECRET_KEY_BUFFER_SIZE {
        return Err(KesError::InvalidKeySize {
            expected: KES_SECRET_KEY_BUFFER_SIZE,
            actual: len,
        });
    }
    Ok(())
}

/// KES key pair (wraps pallas Sum6Kes key material)
#[derive(Debug, Clone)]
pub struct KesKeyPair {
    pub secret: Vec<u8>,
    pub public: Vec<u8>,
    pub period: u64,
}

/// KES signature
#[derive(Debug, Clone)]
pub struct KesSignature {
    pub signature: Vec<u8>,
    pub period: u64,
}

impl KesKeyPair {
    /// Get the current KES period for a given slot
    pub fn kes_period_for_slot(slot: u64) -> u64 {
        kes_period_for_slot(slot)
    }

    /// Check if the KES key can still evolve
    pub fn can_evolve(&self) -> bool {
        self.period < MAX_KES_EVOLUTIONS
    }

    /// Remaining KES evolutions
    pub fn remaining_evolutions(&self) -> u64 {
        MAX_KES_EVOLUTIONS.saturating_sub(self.period)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kes_keygen() {
        let seed = [42u8; 32];
        let (sk, pk) = kes_keygen(&seed).unwrap();
        assert_eq!(sk.len(), KES_SECRET_KEY_BUFFER_SIZE);
        assert_ne!(pk, [0u8; 32]);
    }

    #[test]
    fn test_kes_keygen_different_seeds_produce_different_keys() {
        let (_, pk1) = kes_keygen(&[1u8; 32]).unwrap();
        let (_, pk2) = kes_keygen(&[2u8; 32]).unwrap();
        assert_ne!(pk1, pk2);
    }

    #[test]
    fn test_kes_sign_and_verify() {
        let seed = [99u8; 32];
        let (sk, pk) = kes_keygen(&seed).unwrap();

        let message = b"test message for KES signing";
        let (sig, period) = kes_sign_message(&sk, message).unwrap();
        assert_eq!(period, 0);

        // Verify should succeed
        assert!(kes_verify(&pk, 0, &sig, message).is_ok());

        // Verify with wrong message should fail
        assert!(kes_verify(&pk, 0, &sig, b"wrong message").is_err());
    }

    #[test]
    fn test_kes_update() {
        let seed = [77u8; 32];
        let (sk, _pk) = kes_keygen(&seed).unwrap();

        let period = kes_get_period(&sk).unwrap();
        assert_eq!(period, 0);

        let (sk_updated, new_period) = kes_update(&sk).unwrap();
        assert_eq!(new_period, 1);

        let period = kes_get_period(&sk_updated).unwrap();
        assert_eq!(period, 1);
    }

    #[test]
    fn test_kes_sk_to_pk() {
        let seed = [55u8; 32];
        let (sk, expected_pk) = kes_keygen(&seed).unwrap();
        let derived_pk = kes_sk_to_pk(&sk).unwrap();
        assert_eq!(derived_pk, expected_pk);
    }

    #[test]
    fn test_kes_period_for_slot() {
        assert_eq!(kes_period_for_slot(0), 0);
        assert_eq!(kes_period_for_slot(129599), 0);
        assert_eq!(kes_period_for_slot(129600), 1);
        assert_eq!(kes_period_for_slot(259200), 2);
    }

    #[test]
    fn test_kes_key_size() {
        // Sum6Kes: INDIVIDUAL_SECRET_SIZE + depth * 32 + depth * (PUBLIC_KEY_SIZE * 2)
        // = 32 + 6*32 + 6*64 = 32 + 192 + 384 = 608
        assert_eq!(Sum6Kes::SIZE, 608);
        assert_eq!(KES_SECRET_KEY_BUFFER_SIZE, 612);
    }

    #[test]
    fn test_kes_sign_after_update() {
        let seed = [88u8; 32];
        let (sk, pk) = kes_keygen(&seed).unwrap();
        let message = b"message after update";

        // Update to period 1
        let (sk_updated, _) = kes_update(&sk).unwrap();

        // Sign at period 1
        let (sig, period) = kes_sign_message(&sk_updated, message).unwrap();
        assert_eq!(period, 1);

        // Verify at period 1 should succeed
        assert!(kes_verify(&pk, 1, &sig, message).is_ok());

        // Verify at period 0 should fail
        assert!(kes_verify(&pk, 0, &sig, message).is_err());
    }

    #[test]
    fn test_kes_invalid_key_size() {
        let too_short = vec![0u8; 100];
        assert!(kes_get_period(&too_short).is_err());
        assert!(kes_update(&too_short).is_err());
        assert!(kes_sk_to_pk(&too_short).is_err());
    }

    #[test]
    fn test_kes_keypair_helpers() {
        let kp = KesKeyPair {
            secret: vec![0; KES_SECRET_KEY_BUFFER_SIZE],
            public: vec![0; 32],
            period: 0,
        };
        assert!(kp.can_evolve());
        assert_eq!(kp.remaining_evolutions(), 62);

        let kp_expired = KesKeyPair {
            secret: vec![],
            public: vec![],
            period: 62,
        };
        assert!(!kp_expired.can_evolve());
        assert_eq!(kp_expired.remaining_evolutions(), 0);
    }

    #[test]
    fn test_kes_multiple_updates() {
        let seed = [66u8; 32];
        let (sk, pk) = kes_keygen(&seed).unwrap();

        // Evolve through periods 0..5
        let mut current_sk = sk;
        for expected_period in 1..=5u32 {
            let (new_sk, period) = kes_update(&current_sk).unwrap();
            assert_eq!(period, expected_period);
            current_sk = new_sk;
        }

        // Sign at period 5
        let message = b"period 5 message";
        let (sig, period) = kes_sign_message(&current_sk, message).unwrap();
        assert_eq!(period, 5);
        assert!(kes_verify(&pk, 5, &sig, message).is_ok());
    }

    // =========================================================================
    // KES Evolution Edge Cases
    // =========================================================================

    #[test]
    fn test_kes_evolve_to_max_period() {
        // Evolve all the way to period 62 (MAX_KES_EVOLUTIONS)
        let seed = [0xAB; 32];
        let (sk, pk) = kes_keygen(&seed).unwrap();

        let evolved_sk = kes_evolve_to_period(&sk, MAX_KES_EVOLUTIONS as u32).unwrap();
        let period = kes_get_period(&evolved_sk).unwrap();
        assert_eq!(period, MAX_KES_EVOLUTIONS as u32);

        // Sign and verify at max period
        let message = b"max period test";
        let (sig, sig_period) = kes_sign_message(&evolved_sk, message).unwrap();
        assert_eq!(sig_period, MAX_KES_EVOLUTIONS as u32);
        assert!(kes_verify(&pk, MAX_KES_EVOLUTIONS as u32, &sig, message).is_ok());
    }

    #[test]
    fn test_kes_evolve_past_max_period_errors() {
        // Evolve to max, then try to evolve once more — should error
        let seed = [0xCD; 32];
        let (sk, _pk) = kes_keygen(&seed).unwrap();

        let evolved_sk = kes_evolve_to_period(&sk, MAX_KES_EVOLUTIONS as u32).unwrap();
        let result = kes_update(&evolved_sk);
        assert!(
            result.is_err(),
            "Evolving past MAX_KES_EVOLUTIONS should return an error"
        );
        match result.unwrap_err() {
            KesError::KeyExpired(current, max) => {
                assert_eq!(current, MAX_KES_EVOLUTIONS);
                assert_eq!(max, MAX_KES_EVOLUTIONS);
            }
            other => panic!("Expected KeyExpired, got: {:?}", other),
        }
    }

    #[test]
    fn test_kes_sign_verify_at_period_boundaries() {
        let seed = [0xEF; 32];
        let (sk, pk) = kes_keygen(&seed).unwrap();
        let message = b"boundary period message";

        // Test at period 0
        let (sig_0, _) = kes_sign_message(&sk, message).unwrap();
        assert!(kes_verify(&pk, 0, &sig_0, message).is_ok());

        // Test at period 1
        let sk_1 = kes_evolve_to_period(&sk, 1).unwrap();
        let (sig_1, _) = kes_sign_message(&sk_1, message).unwrap();
        assert!(kes_verify(&pk, 1, &sig_1, message).is_ok());
        // Period 0 signature should not verify at period 1
        assert!(kes_verify(&pk, 1, &sig_0, message).is_err());

        // Test at period 31 (midpoint)
        let sk_31 = kes_evolve_to_period(&sk, 31).unwrap();
        let (sig_31, _) = kes_sign_message(&sk_31, message).unwrap();
        assert!(kes_verify(&pk, 31, &sig_31, message).is_ok());
        // Wrong period should fail
        assert!(kes_verify(&pk, 30, &sig_31, message).is_err());

        // Test at period 62 (max)
        let sk_62 = kes_evolve_to_period(&sk, 62).unwrap();
        let (sig_62, _) = kes_sign_message(&sk_62, message).unwrap();
        assert!(kes_verify(&pk, 62, &sig_62, message).is_ok());
    }

    #[test]
    fn test_kes_key_serialization_roundtrip() {
        let seed = [0x42; 32];
        let (sk, pk) = kes_keygen(&seed).unwrap();

        // Verify key size
        assert_eq!(sk.len(), KES_SECRET_KEY_BUFFER_SIZE);

        // Derive public key from secret key
        let derived_pk = kes_sk_to_pk(&sk).unwrap();
        assert_eq!(derived_pk, pk);

        // Evolve, then check round-trip still works
        let (sk_evolved, period) = kes_update(&sk).unwrap();
        assert_eq!(period, 1);
        assert_eq!(sk_evolved.len(), KES_SECRET_KEY_BUFFER_SIZE);

        // Can still derive public key after evolution
        let derived_pk_evolved = kes_sk_to_pk(&sk_evolved).unwrap();
        // Public key should remain the same across evolutions
        assert_eq!(derived_pk_evolved, pk);

        // Get period from evolved key
        let current_period = kes_get_period(&sk_evolved).unwrap();
        assert_eq!(current_period, 1);
    }

    #[test]
    fn test_kes_evolve_to_same_period_is_noop() {
        let seed = [0x77; 32];
        let (sk, _pk) = kes_keygen(&seed).unwrap();

        // Evolve to period 5
        let sk_5 = kes_evolve_to_period(&sk, 5).unwrap();
        assert_eq!(kes_get_period(&sk_5).unwrap(), 5);

        // Evolving to 5 again should be a no-op (returns as-is)
        let sk_5_again = kes_evolve_to_period(&sk_5, 5).unwrap();
        assert_eq!(kes_get_period(&sk_5_again).unwrap(), 5);

        // Evolving to a lower period should also be a no-op
        let sk_lower = kes_evolve_to_period(&sk_5, 3).unwrap();
        assert_eq!(kes_get_period(&sk_lower).unwrap(), 5);
    }

    #[test]
    fn test_kes_sign_bytes_roundtrip() {
        let seed = [0x99; 32];
        let (sk, pk) = kes_keygen(&seed).unwrap();
        let message = b"sign bytes roundtrip test";

        let (sig_bytes, period) = kes_sign_bytes(&sk, message).unwrap();
        assert_eq!(period, 0);
        assert!(!sig_bytes.is_empty());

        // Verify using kes_verify_bytes
        assert!(kes_verify_bytes(&pk, 0, &sig_bytes, message).is_ok());

        // Wrong message should fail
        assert!(kes_verify_bytes(&pk, 0, &sig_bytes, b"wrong").is_err());
    }

    // =========================================================================
    // Sum6Kes IOHK Reference Test Vectors (#323)
    // =========================================================================

    #[test]
    fn test_sum6kes_deterministic_keygen() {
        let seed = [0u8; 32];
        let (sk1, pk1) = kes_keygen(&seed).unwrap();
        let (sk2, pk2) = kes_keygen(&seed).unwrap();
        assert_eq!(pk1, pk2, "Same seed must produce same public key");
        assert_eq!(sk1, sk2, "Same seed must produce same secret key");
    }

    #[test]
    fn test_sum6kes_cross_period_signature_isolation() {
        let seed = [0x42; 32];
        let (sk, pk) = kes_keygen(&seed).unwrap();
        let message = b"KES cross-period isolation test";

        let (sig_bytes_0, _) = kes_sign_bytes(&sk, message).unwrap();
        assert!(kes_verify_bytes(&pk, 0, &sig_bytes_0, message).is_ok());
        assert!(kes_verify_bytes(&pk, 1, &sig_bytes_0, message).is_err());

        let sk_3 = kes_evolve_to_period(&sk, 3).unwrap();
        let (sig_bytes_3, period_3) = kes_sign_bytes(&sk_3, message).unwrap();
        assert_eq!(period_3, 3);
        assert!(kes_verify_bytes(&pk, 3, &sig_bytes_3, message).is_ok());
        assert!(kes_verify_bytes(&pk, 2, &sig_bytes_3, message).is_err());
        assert!(kes_verify_bytes(&pk, 4, &sig_bytes_3, message).is_err());
        assert!(kes_verify_bytes(&pk, 3, &sig_bytes_0, message).is_err());
    }

    #[test]
    fn test_sum6kes_wrong_key_rejection() {
        let (sk_a, pk_a) = kes_keygen(&[0xAA; 32]).unwrap();
        let (_sk_b, pk_b) = kes_keygen(&[0xBB; 32]).unwrap();
        let message = b"KES wrong key rejection test";

        let (sig_bytes, _) = kes_sign_bytes(&sk_a, message).unwrap();
        assert!(kes_verify_bytes(&pk_a, 0, &sig_bytes, message).is_ok());
        assert!(kes_verify_bytes(&pk_b, 0, &sig_bytes, message).is_err());
    }

    #[test]
    fn test_sum6kes_corrupted_signature_rejection() {
        let seed = [0xCC; 32];
        let (sk, pk) = kes_keygen(&seed).unwrap();
        let message = b"KES corruption test";

        let (mut sig_bytes, _) = kes_sign_bytes(&sk, message).unwrap();
        assert_eq!(sig_bytes.len(), 448, "Sum6KesSig should be 448 bytes");
        assert!(kes_verify_bytes(&pk, 0, &sig_bytes, message).is_ok());

        sig_bytes[0] ^= 0x01;
        assert!(kes_verify_bytes(&pk, 0, &sig_bytes, message).is_err());
    }

    #[test]
    fn test_sum6kes_public_key_stable_across_all_evolutions() {
        let seed = [0xDD; 32];
        let (sk, pk) = kes_keygen(&seed).unwrap();

        let mut current_sk = sk;
        for period in 1..=MAX_KES_EVOLUTIONS as u32 {
            let (new_sk, new_period) = kes_update(&current_sk).unwrap();
            assert_eq!(new_period, period);
            let derived_pk = kes_sk_to_pk(&new_sk).unwrap();
            assert_eq!(
                derived_pk, pk,
                "Public key must be stable at period {period}"
            );
            current_sk = new_sk;
        }
    }
}
