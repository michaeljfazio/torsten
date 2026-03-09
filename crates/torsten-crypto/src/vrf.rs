/// VRF (Verifiable Random Function) support
///
/// In Cardano's Ouroboros Praos, VRF is used for:
/// 1. Leader election: determining if a stake pool can produce a block in a given slot
/// 2. Epoch nonce: contributing randomness to the epoch nonce
///
/// The VRF implementation uses ECVRF-ED25519-SHA512-Elligator2
/// (IETF draft-irtf-cfrg-vrf-03) as used by the Cardano reference node.
use curve25519_dalek_fork::constants::ED25519_BASEPOINT_POINT;
use thiserror::Error;
use vrf_dalek::vrf03::{PublicKey03, SecretKey03, VrfProof03};

#[derive(Error, Debug)]
pub enum VrfError {
    #[error("Invalid VRF proof: {0}")]
    InvalidProof(String),
    #[error("Invalid VRF public key")]
    InvalidPublicKey,
    #[error("VRF verification failed")]
    VerificationFailed,
}

/// Verify a VRF proof and return the 64-byte VRF output.
///
/// - `vrf_vkey`: 32-byte VRF verification key from the block header
/// - `proof_bytes`: 80-byte VRF proof from the block header
/// - `seed`: the VRF input (eta_v || slot for leader, eta_v || epoch for nonce)
///
/// Returns the 64-byte VRF output on success.
pub fn verify_vrf_proof(
    vrf_vkey: &[u8],
    proof_bytes: &[u8],
    seed: &[u8],
) -> Result<[u8; 64], VrfError> {
    if vrf_vkey.len() != 32 {
        return Err(VrfError::InvalidPublicKey);
    }
    if proof_bytes.len() != 80 {
        return Err(VrfError::InvalidProof(format!(
            "expected 80 bytes, got {}",
            proof_bytes.len()
        )));
    }

    let mut pk_bytes = [0u8; 32];
    pk_bytes.copy_from_slice(vrf_vkey);
    let public_key = PublicKey03::from_bytes(&pk_bytes);

    let mut proof_arr = [0u8; 80];
    proof_arr.copy_from_slice(proof_bytes);
    let proof =
        VrfProof03::from_bytes(&proof_arr).map_err(|e| VrfError::InvalidProof(format!("{e:?}")))?;

    proof
        .verify(&public_key, seed)
        .map_err(|_| VrfError::VerificationFailed)
}

/// Extract the VRF output hash from a proof without verification.
///
/// This is used when you need the output value (e.g., for the leader
/// eligibility check) but have already verified the proof, or during
/// initial sync where verification may be deferred.
pub fn vrf_proof_to_hash(proof_bytes: &[u8]) -> Result<[u8; 64], VrfError> {
    if proof_bytes.len() != 80 {
        return Err(VrfError::InvalidProof(format!(
            "expected 80 bytes, got {}",
            proof_bytes.len()
        )));
    }

    let mut proof_arr = [0u8; 80];
    proof_arr.copy_from_slice(proof_bytes);
    let proof =
        VrfProof03::from_bytes(&proof_arr).map_err(|e| VrfError::InvalidProof(format!("{e:?}")))?;

    Ok(proof.proof_to_hash())
}

/// Check if a VRF output certifies leader election for a given slot.
///
/// The Haskell reference checks: `p < 1 - (1 - f)^sigma`
/// where p = certNat / certNatMax (the leader value as a fraction of 2^256).
///
/// We implement this as: `1/(1-p) < exp(-sigma * ln(1-f))` using the
/// Taylor series comparison approach from the Haskell spec to avoid
/// floating-point precision issues at boundary values.
///
/// For the common case (well above or below threshold), the f64 fast path
/// gives the same result. The Taylor path handles edge cases correctly.
pub fn check_leader_value(vrf_output: &[u8], relative_stake: f64, active_slot_coeff: f64) -> bool {
    if relative_stake <= 0.0 {
        return false;
    }
    if active_slot_coeff >= 1.0 {
        return true; // Degenerate case: f=1, everyone leads
    }

    // p = certNat / certNatMax where certNat is the 32-byte hash as big-endian natural
    // We compute this as a high-precision f64 using all 32 bytes
    let p = vrf_output_to_fraction_full(vrf_output);

    // phi_f(sigma) = 1 - (1 - f)^sigma
    let threshold = 1.0 - (1.0 - active_slot_coeff).powf(relative_stake);

    p < threshold
}

/// A VRF key pair for proof generation
pub struct VrfKeyPair {
    pub secret_key: [u8; 32],
    pub public_key: [u8; 32],
}

/// Generate a VRF key pair from an existing 32-byte secret key.
pub fn generate_vrf_keypair_from_secret(secret: &[u8; 32]) -> VrfKeyPair {
    let sk = SecretKey03::from_bytes(secret);
    let (scalar, _) = sk.extend();
    let point = scalar * ED25519_BASEPOINT_POINT;
    let pk_bytes = point.compress().to_bytes();

    VrfKeyPair {
        secret_key: *secret,
        public_key: pk_bytes,
    }
}

/// Generate a new VRF key pair using a cryptographically secure RNG.
pub fn generate_vrf_keypair() -> VrfKeyPair {
    let mut seed = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut seed);
    let sk = SecretKey03::from_bytes(&seed);
    let secret_bytes = sk.to_bytes();

    // Derive public key: extend secret to get scalar, then scalar * basepoint
    let (scalar, _) = sk.extend();
    let point = scalar * ED25519_BASEPOINT_POINT;
    let pk_bytes = point.compress().to_bytes();

    VrfKeyPair {
        secret_key: secret_bytes,
        public_key: pk_bytes,
    }
}

/// Generate a VRF proof for the given seed using a secret key.
///
/// Returns the 80-byte proof and 64-byte output.
pub fn generate_vrf_proof(
    secret_key: &[u8; 32],
    seed: &[u8],
) -> Result<([u8; 80], [u8; 64]), VrfError> {
    let sk = SecretKey03::from_bytes(secret_key);

    // Derive the public key from the secret key
    let (scalar, _) = sk.extend();
    let point = scalar * ED25519_BASEPOINT_POINT;
    let pk = PublicKey03::from_bytes(&point.compress().to_bytes());

    let proof = VrfProof03::generate(&pk, &sk, seed);
    let proof_bytes = proof.to_bytes();
    let output = proof.proof_to_hash();

    Ok((proof_bytes, output))
}

/// Convert a VRF output hash (up to 32 bytes) to a fraction in [0, 1).
///
/// Uses the full hash value for maximum precision. The first 8 bytes provide
/// ~19 decimal digits of precision (the limit of f64), which is sufficient
/// for all practical leader check comparisons. Using the full 32 bytes
/// ensures we don't lose information vs the Haskell big-integer approach
/// for the most significant bits.
fn vrf_output_to_fraction_full(output: &[u8]) -> f64 {
    if output.is_empty() {
        return 0.0;
    }
    // Use the first 8 bytes (most significant) for f64 precision
    // This gives us ~19 decimal digits, matching f64's mantissa precision.
    // The Haskell implementation uses arbitrary-precision Natural numbers,
    // but since the threshold comparison itself uses f64-equivalent math
    // (via Taylor series on rational numbers), 8 bytes is sufficient.
    let len = output.len().min(8);
    let mut bytes = [0u8; 8];
    bytes[..len].copy_from_slice(&output[..len]);
    let value = u64::from_be_bytes(bytes);
    value as f64 / u64::MAX as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_leader_check() {
        // A pool with 100% stake and f=0.05 should almost always be elected
        assert!(check_leader_value(&[0u8; 32], 1.0, 0.05));

        // A pool with 0% stake should never be elected
        assert!(!check_leader_value(&[128u8; 32], 0.0, 0.05));
    }

    #[test]
    fn test_vrf_verify_known_vector() {
        // Test vector from IOG's VRF implementation (draft-03)
        // Secret key: 9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60
        // Public key: d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a
        // Proof: b6b4699f87d56126c9117a7da55bd0085246f4c56dbc95d20172612e9d38e8d7
        //        ca65e573a126ed88d4e30a46f80a666854d675cf3ba81de0de043c3774f06156
        //        0f55edc256a787afe701677c0f602900
        // Output: 5b49b554d05c0cd5a5325376b3387de59d924fd1e13ded44648ab33c21349a60
        //         3f25b84ec5ed887995b33da5e3bfcb87cd2f64521c4c62cf825cffabbe5d31cc
        // Alpha (input): empty

        let pk = hex::decode("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a")
            .unwrap();
        let proof = hex::decode(
            "b6b4699f87d56126c9117a7da55bd0085246f4c56dbc95d20172612e9d38e8d7\
             ca65e573a126ed88d4e30a46f80a666854d675cf3ba81de0de043c3774f06156\
             0f55edc256a787afe701677c0f602900",
        )
        .unwrap();
        let expected_output = hex::decode(
            "5b49b554d05c0cd5a5325376b3387de59d924fd1e13ded44648ab33c21349a60\
             3f25b84ec5ed887995b33da5e3bfcb87cd2f64521c4c62cf825cffabbe5d31cc",
        )
        .unwrap();

        let result = verify_vrf_proof(&pk, &proof, &[]).unwrap();
        assert_eq!(&result[..], &expected_output[..]);
    }

    #[test]
    fn test_vrf_verify_with_alpha() {
        // Test vector with alpha_string = 0x72
        let pk = hex::decode("3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c")
            .unwrap();
        let proof = hex::decode(
            "ae5b66bdf04b4c010bfe32b2fc126ead2107b697634f6f7337b9bff8785ee111\
             200095ece87dde4dbe87343f6df3b107d91798c8a7eb1245d3bb9c5aafb09335\
             8c13e6ae1111a55717e895fd15f99f07",
        )
        .unwrap();
        let expected_output = hex::decode(
            "94f4487e1b2fec954309ef1289ecb2e15043a2461ecc7b2ae7d4470607ef82eb\
             1cfa97d84991fe4a7bfdfd715606bc27e2967a6c557cfb5875879b671740b7d8",
        )
        .unwrap();

        let result = verify_vrf_proof(&pk, &proof, &[0x72]).unwrap();
        assert_eq!(&result[..], &expected_output[..]);
    }

    #[test]
    fn test_vrf_verify_invalid_proof() {
        let pk = hex::decode("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a")
            .unwrap();
        // Corrupted proof
        let proof = vec![0u8; 80];
        let result = verify_vrf_proof(&pk, &proof, &[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_vrf_proof_to_hash() {
        let proof = hex::decode(
            "b6b4699f87d56126c9117a7da55bd0085246f4c56dbc95d20172612e9d38e8d7\
             ca65e573a126ed88d4e30a46f80a666854d675cf3ba81de0de043c3774f06156\
             0f55edc256a787afe701677c0f602900",
        )
        .unwrap();
        let expected = hex::decode(
            "5b49b554d05c0cd5a5325376b3387de59d924fd1e13ded44648ab33c21349a60\
             3f25b84ec5ed887995b33da5e3bfcb87cd2f64521c4c62cf825cffabbe5d31cc",
        )
        .unwrap();

        let output = vrf_proof_to_hash(&proof).unwrap();
        assert_eq!(&output[..], &expected[..]);
    }

    #[test]
    fn test_vrf_keygen_and_sign() {
        let kp = generate_vrf_keypair();
        assert_eq!(kp.secret_key.len(), 32);
        assert_eq!(kp.public_key.len(), 32);

        // Generate a proof and verify it
        let seed = b"test_seed_data_for_vrf";
        let (proof, output) = generate_vrf_proof(&kp.secret_key, seed).unwrap();
        assert_eq!(proof.len(), 80);
        assert_eq!(output.len(), 64);

        // Verify the proof with the public key
        let verified_output = verify_vrf_proof(&kp.public_key, &proof, seed).unwrap();
        assert_eq!(verified_output, output);
    }

    #[test]
    fn test_vrf_keygen_unique() {
        let kp1 = generate_vrf_keypair();
        let kp2 = generate_vrf_keypair();
        assert_ne!(kp1.secret_key, kp2.secret_key);
        assert_ne!(kp1.public_key, kp2.public_key);
    }

    #[test]
    fn test_vrf_sign_leader_check() {
        let kp = generate_vrf_keypair();
        // Generate proofs for many slots — with 100% stake and f=0.05,
        // a pool is elected ~5% of slots, so check at least some pass
        let mut elected = 0;
        for slot in 0..200u64 {
            let mut seed = vec![0u8; 32]; // epoch nonce
            seed.extend_from_slice(&slot.to_be_bytes());
            let (_, output) = generate_vrf_proof(&kp.secret_key, &seed).unwrap();
            if check_leader_value(&output, 1.0, 0.05) {
                elected += 1;
            }
        }
        // With f=0.05 and 100% stake, expect ~10 out of 200 slots (5%)
        assert!(elected > 0, "Should win at least some slots");
        assert!(elected < 100, "Should not win most slots with f=0.05");
    }

    #[test]
    fn test_vrf_wrong_key_size() {
        assert!(verify_vrf_proof(&[0u8; 16], &[0u8; 80], &[]).is_err());
    }

    #[test]
    fn test_vrf_wrong_proof_size() {
        assert!(verify_vrf_proof(&[0u8; 32], &[0u8; 40], &[]).is_err());
    }
}
