#[cfg(not(target_arch = "x86_64"))]
use blake2::Digest;
use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Hash<const N: usize>(pub [u8; N]);

impl<const N: usize> serde::Serialize for Hash<N> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de, const N: usize> serde::Deserialize<'de> for Hash<N> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::from_hex(&s).map_err(serde::de::Error::custom)
    }
}

pub type Hash28 = Hash<28>;
pub type Hash32 = Hash<32>;

impl Hash28 {
    /// Convert a 28-byte hash to a 32-byte hash by zero-padding the last 4 bytes.
    ///
    /// This is used throughout the codebase when 28-byte credential/key hashes
    /// (e.g., pool IDs, DRep key hashes, stake key hashes) need to be used as
    /// Hash32 map keys or compared with Hash32 values.
    pub fn to_hash32_padded(&self) -> Hash32 {
        let mut bytes = [0u8; 32];
        bytes[..28].copy_from_slice(self.as_bytes());
        Hash32::from_bytes(bytes)
    }
}

pub type BlockHeaderHash = Hash32;
pub type TransactionHash = Hash32;
pub type ScriptHash = Hash28;
pub type PolicyId = Hash28;
pub type DatumHash = Hash32;
pub type VrfKeyHash = Hash32;
pub type PoolKeyHash = Hash28;
pub type GenesisHash = Hash28;
pub type GenesisDelegateHash = Hash28;
pub type AuxiliaryDataHash = Hash32;

impl<const N: usize> Hash<N> {
    pub const ZERO: Self = Self([0u8; N]);

    pub fn from_bytes(bytes: [u8; N]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; N] {
        &self.0
    }

    pub fn to_vec(&self) -> Vec<u8> {
        self.0.to_vec()
    }

    pub fn from_hex(hex_str: &str) -> Result<Self, hex::FromHexError> {
        let bytes = hex::decode(hex_str)?;
        let mut arr = [0u8; N];
        if bytes.len() != N {
            return Err(hex::FromHexError::InvalidStringLength);
        }
        arr.copy_from_slice(&bytes);
        Ok(Self(arr))
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
}

impl<const N: usize> fmt::Debug for Hash<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Hash({})", self.to_hex())
    }
}

impl<const N: usize> fmt::Display for Hash<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

impl<const N: usize> Default for Hash<N> {
    fn default() -> Self {
        Self([0u8; N])
    }
}

impl<const N: usize> AsRef<[u8]> for Hash<N> {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl<const N: usize> TryFrom<&[u8]> for Hash<N> {
    type Error = std::array::TryFromSliceError;

    fn try_from(slice: &[u8]) -> Result<Self, Self::Error> {
        let arr: [u8; N] = slice.try_into()?;
        Ok(Self(arr))
    }
}

/// Blake2b-256 hash.
/// On x86_64: uses `blake2b_simd` with SSE2/SSE4.1/AVX2 intrinsics.
/// On ARM/AArch64: uses `blake2` (RustCrypto) which LLVM auto-vectorizes well for NEON.
#[cfg(target_arch = "x86_64")]
pub fn blake2b_256(data: &[u8]) -> Hash32 {
    let hash = blake2b_simd::Params::new().hash_length(32).hash(data);
    let mut out = [0u8; 32];
    out.copy_from_slice(hash.as_bytes());
    Hash(out)
}

#[cfg(not(target_arch = "x86_64"))]
pub fn blake2b_256(data: &[u8]) -> Hash32 {
    let mut hasher = blake2::Blake2b::<blake2::digest::consts::U32>::new();
    hasher.update(data);
    let result = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    Hash(out)
}

/// Blake2b-224 hash (used for addresses, key hashes).
/// On x86_64: uses `blake2b_simd` with SSE2/SSE4.1/AVX2 intrinsics.
/// On ARM/AArch64: uses `blake2` (RustCrypto) which LLVM auto-vectorizes well for NEON.
#[cfg(target_arch = "x86_64")]
pub fn blake2b_224(data: &[u8]) -> Hash28 {
    let hash = blake2b_simd::Params::new().hash_length(28).hash(data);
    let mut out = [0u8; 28];
    out.copy_from_slice(hash.as_bytes());
    Hash(out)
}

#[cfg(not(target_arch = "x86_64"))]
pub fn blake2b_224(data: &[u8]) -> Hash28 {
    let mut hasher = blake2::Blake2b::<blake2::digest::consts::U28>::new();
    hasher.update(data);
    let result = hasher.finalize();
    let mut out = [0u8; 28];
    out.copy_from_slice(&result);
    Hash(out)
}

/// Hash `[tag_byte] || data` with Blake2b-224.
///
/// Matches pallas `Hasher::<224>::hash_tagged(&self.0, VERSION as u8)`
/// which hashes `[tag] || raw_bytes` for Plutus script hashing.
/// The tag byte indicates the script version:
/// 0 = NativeScript, 1 = PlutusV1, 2 = PlutusV2, 3 = PlutusV3.
///
/// For Plutus scripts, `data` is the raw script bytes (the inner content
/// of the CBOR bstr, NOT CBOR-encoded).
pub fn blake2b_224_tagged(tag: u8, data: &[u8]) -> Hash28 {
    let mut buf = Vec::with_capacity(1 + data.len());
    buf.push(tag);
    buf.extend_from_slice(data);
    blake2b_224(&buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_hex_roundtrip() {
        let hash = blake2b_256(b"hello cardano");
        let hex_str = hash.to_hex();
        let recovered = Hash32::from_hex(&hex_str).unwrap();
        assert_eq!(hash, recovered);
    }

    #[test]
    fn test_blake2b_224() {
        let hash = blake2b_224(b"test");
        assert_eq!(hash.as_bytes().len(), 28);
    }

    #[test]
    fn test_hash_display() {
        let hash = Hash32::ZERO;
        assert_eq!(
            hash.to_string(),
            "0000000000000000000000000000000000000000000000000000000000000000"
        );
    }

    #[test]
    fn test_hash28_to_hash32_padded_zeros() {
        let h28 = Hash28::ZERO;
        let h32 = h28.to_hash32_padded();
        assert_eq!(h32, Hash32::ZERO);
    }

    #[test]
    fn test_hash28_to_hash32_padded_nonzero() {
        let h28 = Hash28::from_bytes([0xAB; 28]);
        let h32 = h28.to_hash32_padded();
        // First 28 bytes should match the original
        assert_eq!(&h32.as_bytes()[..28], h28.as_bytes());
        // Last 4 bytes should be zero-padded
        assert_eq!(&h32.as_bytes()[28..], &[0u8; 4]);
    }

    #[test]
    fn test_hash28_to_hash32_padded_preserves_content() {
        let h28 = blake2b_224(b"cardano pool key");
        let h32 = h28.to_hash32_padded();
        // Round-trip: the first 28 bytes of h32 should reconstruct h28
        let recovered = Hash28::try_from(&h32.as_bytes()[..28]).unwrap();
        assert_eq!(recovered, h28);
    }

    #[test]
    fn test_hash28_to_hash32_padded_distinct_inputs() {
        let h28_a = Hash28::from_bytes([1u8; 28]);
        let h28_b = Hash28::from_bytes([2u8; 28]);
        assert_ne!(h28_a.to_hash32_padded(), h28_b.to_hash32_padded());
    }

    // -----------------------------------------------------------------------
    // Additional hash tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_hash32_from_hex_roundtrip_various() {
        // Test with known hex values
        let hex_str = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
        let hash = Hash32::from_hex(hex_str).unwrap();
        assert_eq!(hash.to_hex(), hex_str);

        // All zeros
        let zero_hex = "0000000000000000000000000000000000000000000000000000000000000000";
        let zero_hash = Hash32::from_hex(zero_hex).unwrap();
        assert_eq!(zero_hash, Hash32::ZERO);

        // All ff
        let ff_hex = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
        let ff_hash = Hash32::from_hex(ff_hex).unwrap();
        assert_eq!(ff_hash.to_hex(), ff_hex);
    }

    #[test]
    fn test_hash32_from_hex_invalid_odd_length() {
        // Odd-length hex string should fail
        let result = Hash32::from_hex("abc");
        assert!(result.is_err());
    }

    #[test]
    fn test_hash32_from_hex_invalid_non_hex_chars() {
        // Non-hex characters should fail
        let result =
            Hash32::from_hex("gggggggg00000000000000000000000000000000000000000000000000000000");
        assert!(result.is_err());
    }

    #[test]
    fn test_hash32_from_hex_wrong_length() {
        // Valid hex but wrong length (too short)
        let result = Hash32::from_hex("abcdef");
        assert!(result.is_err());

        // Valid hex but too long
        let result =
            Hash32::from_hex("abcdef0123456789abcdef0123456789abcdef0123456789abcdef012345678900");
        assert!(result.is_err());
    }

    #[test]
    fn test_hash28_to_hash32_and_back() {
        let original = blake2b_224(b"cardano key hash test");
        let padded = original.to_hash32_padded();

        // Extract first 28 bytes to recover original
        let recovered = Hash28::try_from(&padded.as_bytes()[..28]).unwrap();
        assert_eq!(recovered, original);

        // Last 4 bytes should be zero
        assert_eq!(&padded.as_bytes()[28..], &[0u8; 4]);
    }

    #[test]
    fn test_hash_try_from_slice() {
        let bytes = [0xABu8; 32];
        let hash = Hash32::try_from(bytes.as_slice()).unwrap();
        assert_eq!(hash, Hash32::from_bytes([0xAB; 32]));

        // Wrong length should fail
        let short = [0u8; 16];
        assert!(Hash32::try_from(short.as_slice()).is_err());
    }

    #[test]
    fn test_hash_default_is_zero() {
        assert_eq!(Hash32::default(), Hash32::ZERO);
        assert_eq!(Hash28::default(), Hash28::ZERO);
    }

    #[test]
    fn test_hash_ordering() {
        let h1 = Hash32::from_bytes([0u8; 32]);
        let h2 = Hash32::from_bytes([1u8; 32]);
        let h3 = Hash32::from_bytes([255u8; 32]);
        assert!(h1 < h2);
        assert!(h2 < h3);
    }

    #[test]
    fn test_hash_serde_roundtrip() {
        let hash = blake2b_256(b"serde test");
        let json = serde_json::to_string(&hash).unwrap();
        let recovered: Hash32 = serde_json::from_str(&json).unwrap();
        assert_eq!(hash, recovered);
    }

    #[test]
    fn test_blake2b_256_deterministic() {
        let a = blake2b_256(b"test input");
        let b = blake2b_256(b"test input");
        assert_eq!(a, b);

        let c = blake2b_256(b"different input");
        assert_ne!(a, c);
    }

    #[test]
    fn test_blake2b_224_deterministic() {
        let a = blake2b_224(b"test input");
        let b = blake2b_224(b"test input");
        assert_eq!(a, b);

        let c = blake2b_224(b"different input");
        assert_ne!(a, c);
    }
}
