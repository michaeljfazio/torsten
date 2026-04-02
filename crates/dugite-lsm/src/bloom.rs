//! Bloom filter for probabilistic key existence checks.
//!
//! Uses double hashing (Kirsch-Mitzenmacher optimization) to simulate k
//! independent hash functions from two base hashes. At 10 bits per key,
//! achieves approximately 1% false positive rate.
//!
//! Serialization: raw bytes with header.

use crate::key::Key;

/// A bloom filter for probabilistic set membership testing.
#[derive(Clone)]
pub struct BloomFilter {
    /// Bit array stored as bytes.
    bits: Vec<u8>,
    /// Total number of bits.
    num_bits: usize,
    /// Number of hash functions (k).
    num_hashes: u32,
}

impl BloomFilter {
    /// Create a new bloom filter sized for `num_keys` keys with the given
    /// bits-per-key ratio.
    ///
    /// `bits_per_key` of 10 gives approximately 1% false positive rate.
    pub fn new(num_keys: usize, bits_per_key: usize) -> Self {
        let num_bits = (num_keys.saturating_mul(bits_per_key)).max(64);
        let num_bytes = num_bits.div_ceil(8);
        // Optimal k = bits_per_key * ln(2) ~ bits_per_key * 0.693
        let num_hashes = ((bits_per_key as f64 * 0.693) as u32).clamp(1, 30);

        BloomFilter {
            bits: vec![0u8; num_bytes],
            num_bits: num_bytes * 8,
            num_hashes,
        }
    }

    /// Add a key to the filter.
    pub fn insert(&mut self, key: &Key) {
        let (h1, h2) = double_hash(key.as_ref());

        for i in 0..self.num_hashes {
            let bit_pos = (h1.wrapping_add((i as u64).wrapping_mul(h2))) % self.num_bits as u64;
            let byte_idx = (bit_pos / 8) as usize;
            let bit_idx = (bit_pos % 8) as u8;
            self.bits[byte_idx] |= 1 << bit_idx;
        }
    }

    /// Test whether a key might be in the set.
    ///
    /// Returns `true` if the key is possibly present (may be a false positive).
    /// Returns `false` if the key is definitely not present.
    pub fn may_contain(&self, key: &Key) -> bool {
        if self.bits.is_empty() {
            return false;
        }
        let (h1, h2) = double_hash(key.as_ref());

        for i in 0..self.num_hashes {
            let bit_pos = (h1.wrapping_add((i as u64).wrapping_mul(h2))) % self.num_bits as u64;
            let byte_idx = (bit_pos / 8) as usize;
            let bit_idx = (bit_pos % 8) as u8;
            if self.bits[byte_idx] & (1 << bit_idx) == 0 {
                return false;
            }
        }
        true
    }

    /// Serialize the bloom filter to bytes.
    ///
    /// Format: [num_hashes: u32 LE] [num_bits: u32 LE] [bit data...]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(8 + self.bits.len());
        buf.extend_from_slice(&self.num_hashes.to_le_bytes());
        buf.extend_from_slice(&(self.num_bits as u32).to_le_bytes());
        buf.extend_from_slice(&self.bits);
        buf
    }

    /// Deserialize a bloom filter from bytes.
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 8 {
            return None;
        }
        let num_hashes = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        let num_bits = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;

        let num_bytes = num_bits.div_ceil(8);
        if data.len() < 8 + num_bytes {
            return None;
        }

        let bits = data[8..8 + num_bytes].to_vec();

        Some(BloomFilter {
            bits,
            num_bits,
            num_hashes,
        })
    }
}

/// Double hashing using FNV-1a-inspired mixing.
/// Returns two independent 64-bit hash values for Kirsch-Mitzenmacher scheme.
fn double_hash(data: &[u8]) -> (u64, u64) {
    // Hash 1: FNV-1a
    let mut h1: u64 = 0xcbf29ce484222325;
    for &b in data {
        h1 ^= b as u64;
        h1 = h1.wrapping_mul(0x100000001b3);
    }

    // Hash 2: variant with different seed and multiplier
    let mut h2: u64 = 0x517cc1b727220a95;
    for &b in data {
        h2 ^= b as u64;
        h2 = h2.wrapping_mul(0x9e3779b97f4a7c15);
    }

    (h1, h2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bloom_insert_and_query() {
        let mut bloom = BloomFilter::new(100, 10);

        let key1 = Key::from([1, 2, 3]);
        let key2 = Key::from([4, 5, 6]);

        bloom.insert(&key1);
        assert!(bloom.may_contain(&key1));
        assert!(!bloom.may_contain(&key2));
    }

    #[test]
    fn test_bloom_false_positive_rate() {
        let n = 10_000;
        let mut bloom = BloomFilter::new(n, 10);

        // Insert n keys
        for i in 0..n {
            let key = Key::from((i as u64).to_le_bytes());
            bloom.insert(&key);
        }

        // Check all inserted keys are found
        for i in 0..n {
            let key = Key::from((i as u64).to_le_bytes());
            assert!(bloom.may_contain(&key));
        }

        // Check false positive rate with keys not inserted
        let mut false_positives = 0;
        let test_count = 10_000;
        for i in n..(n + test_count) {
            let key = Key::from((i as u64).to_le_bytes());
            if bloom.may_contain(&key) {
                false_positives += 1;
            }
        }

        let fpr = false_positives as f64 / test_count as f64;
        // With 10 bits/key, FPR should be around 1%. Allow up to 3%.
        assert!(
            fpr < 0.03,
            "false positive rate too high: {fpr:.4} ({false_positives}/{test_count})"
        );
    }

    #[test]
    fn test_bloom_serialization_roundtrip() {
        let mut bloom = BloomFilter::new(100, 10);
        for i in 0u8..50 {
            bloom.insert(&Key::from([i]));
        }

        let bytes = bloom.to_bytes();
        let restored = BloomFilter::from_bytes(&bytes).unwrap();

        for i in 0u8..50 {
            assert!(restored.may_contain(&Key::from([i])));
        }
    }

    #[test]
    fn test_bloom_empty() {
        let bloom = BloomFilter::new(0, 10);
        assert!(!bloom.may_contain(&Key::from([1])));
    }
}
