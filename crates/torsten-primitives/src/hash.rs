use blake2::digest::consts::U28;
use blake2::digest::consts::U32;
use blake2::Blake2b;
use blake2::Digest;
use serde::{Deserialize, Serialize};
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

/// Blake2b-256 hash
pub fn blake2b_256(data: &[u8]) -> Hash32 {
    let mut hasher = Blake2b::<U32>::new();
    hasher.update(data);
    let result = hasher.finalize();
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&result);
    Hash(hash)
}

/// Blake2b-224 hash (used for addresses, key hashes)
pub fn blake2b_224(data: &[u8]) -> Hash28 {
    let mut hasher = Blake2b::<U28>::new();
    hasher.update(data);
    let result = hasher.finalize();
    let mut hash = [0u8; 28];
    hash.copy_from_slice(&result);
    Hash(hash)
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
}
