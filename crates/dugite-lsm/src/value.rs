//! Variable-length byte value for LSM-tree storage.

/// A variable-length byte value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Value(Vec<u8>);

impl Value {
    /// Create a value from raw bytes.
    #[inline]
    pub fn new(data: Vec<u8>) -> Self {
        Value(data)
    }

    /// Returns the value bytes as a slice.
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Returns the length in bytes.
    #[inline]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns true if the value is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Consume the value and return the underlying bytes.
    #[inline]
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

impl AsRef<[u8]> for Value {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl From<&[u8]> for Value {
    #[inline]
    fn from(data: &[u8]) -> Self {
        Value(data.to_vec())
    }
}

impl<const N: usize> From<[u8; N]> for Value {
    #[inline]
    fn from(data: [u8; N]) -> Self {
        Value(data.to_vec())
    }
}

impl From<Vec<u8>> for Value {
    #[inline]
    fn from(data: Vec<u8>) -> Self {
        Value(data)
    }
}
