//! Variable-length byte key with lexicographic ordering.
//!
//! Keys are arbitrary byte sequences compared lexicographically, matching the
//! natural ordering of Cardano UTxO keys (32-byte tx hash + 4-byte index BE).

/// A variable-length byte key that orders lexicographically.
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub struct Key(Vec<u8>);

impl Key {
    /// Create a key from raw bytes.
    #[inline]
    pub fn new(data: Vec<u8>) -> Self {
        Key(data)
    }

    /// Returns the key bytes as a slice.
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Returns the length in bytes.
    #[inline]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns true if the key is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Consume the key and return the underlying bytes.
    #[inline]
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

impl Ord for Key {
    #[inline]
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.cmp(&other.0)
    }
}

impl PartialOrd for Key {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl AsRef<[u8]> for Key {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl From<&[u8]> for Key {
    #[inline]
    fn from(data: &[u8]) -> Self {
        Key(data.to_vec())
    }
}

impl<const N: usize> From<[u8; N]> for Key {
    #[inline]
    fn from(data: [u8; N]) -> Self {
        Key(data.to_vec())
    }
}

impl From<Vec<u8>> for Key {
    #[inline]
    fn from(data: Vec<u8>) -> Self {
        Key(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_ordering() {
        let a = Key::from([0x00, 0x01]);
        let b = Key::from([0x00, 0x02]);
        let c = Key::from([0x01, 0x00]);
        assert!(a < b);
        assert!(b < c);
        assert!(a < c);
    }

    #[test]
    fn test_key_empty_prefix() {
        let empty = Key::from([0u8; 0]);
        let nonempty = Key::from([0x00]);
        assert!(empty < nonempty);
    }

    #[test]
    fn test_key_from_slice() {
        let data = [1u8, 2, 3];
        let key = Key::from(&data[..]);
        assert_eq!(key.as_ref(), &[1, 2, 3]);
        assert_eq!(key.len(), 3);
    }

    #[test]
    fn test_key_from_array() {
        let key = Key::from([0xABu8; 32]);
        assert_eq!(key.len(), 32);
        assert_eq!(key.as_bytes()[0], 0xAB);
    }

    #[test]
    fn test_key_equality() {
        let a = Key::from([1, 2, 3]);
        let b = Key::from([1, 2, 3]);
        let c = Key::from([1, 2, 4]);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
