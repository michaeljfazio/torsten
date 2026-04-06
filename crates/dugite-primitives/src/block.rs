use crate::era::Era;
use crate::hash::{BlockHeaderHash, Hash32};
use crate::time::{BlockNo, SlotNo};
use crate::transaction::Transaction;
use serde::{Deserialize, Serialize};

/// A point on the chain (for chain-sync protocol)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Point {
    /// The genesis / origin point
    Origin,
    /// A specific block identified by slot and hash
    Specific(SlotNo, BlockHeaderHash),
}

impl Point {
    pub fn slot(&self) -> Option<SlotNo> {
        match self {
            Point::Origin => None,
            Point::Specific(slot, _) => Some(*slot),
        }
    }

    pub fn hash(&self) -> Option<&BlockHeaderHash> {
        match self {
            Point::Origin => None,
            Point::Specific(_, hash) => Some(hash),
        }
    }
}

impl std::fmt::Display for Point {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Point::Origin => write!(f, "origin"),
            Point::Specific(slot, hash) => write!(f, "{}@{}", slot, hash),
        }
    }
}

impl PartialOrd for Point {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Point {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match (self, other) {
            (Point::Origin, Point::Origin) => std::cmp::Ordering::Equal,
            (Point::Origin, _) => std::cmp::Ordering::Less,
            (_, Point::Origin) => std::cmp::Ordering::Greater,
            (Point::Specific(s1, _), Point::Specific(s2, _)) => s1.cmp(s2),
        }
    }
}

/// Block header (Shelley+ era)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockHeader {
    pub header_hash: BlockHeaderHash,
    pub prev_hash: BlockHeaderHash,
    pub issuer_vkey: Vec<u8>,
    pub vrf_vkey: Vec<u8>,
    pub vrf_result: VrfOutput,
    pub block_number: BlockNo,
    pub slot: SlotNo,
    pub epoch_nonce: Hash32,
    pub body_size: u64,
    pub body_hash: Hash32,
    pub operational_cert: OperationalCert,
    pub protocol_version: ProtocolVersion,
    /// KES signature over the header body (448 bytes for Sum6Kes)
    #[serde(default)]
    pub kes_signature: Vec<u8>,
    /// Pre-computed nonce VRF contribution (eta) for the nonce state machine.
    ///
    /// This is the era-specific, single-step-hashed nonce value fed into
    /// `evolving_nonce = blake2b_256(evolving_nonce || nonce_vrf_output)`:
    ///
    /// - Shelley / Allegra / Mary / Alonzo (TPraos, proto < 7):
    ///   `nonce_vrf_output = blake2b_256(nonce_vrf_cert.output)`
    ///   Uses the *nonce* VRF certificate (separate from the leader certificate),
    ///   hashed once without prefix.  This matches Haskell's `vrfNonceValue`
    ///   in the TPraos era where `hashRaw id (certifiedOutput vrf)`.
    ///
    /// - Babbage / Conway (Praos, proto >= 7):
    ///   `nonce_vrf_output = blake2b_256("N" || vrf_result.output)`
    ///   The single `vrf_result` field replaces both nonce_vrf and leader_vrf.
    ///   The nonce contribution is derived with the "N" tag.  Matches pallas's
    ///   `HeaderBody::nonce_vrf_output()` and Haskell's `vrfNonceValue` in Praos.
    ///
    /// Empty for Byron blocks (OBFT — no VRF).
    #[serde(default)]
    pub nonce_vrf_output: Vec<u8>,
    /// TPraos nonce VRF proof (80 bytes for Shelley–Alonzo, empty for Praos/Byron).
    ///
    /// In TPraos (proto < 7), the header contains separate leader_vrf and nonce_vrf
    /// certificates. This field preserves the nonce VRF proof so consensus can
    /// cryptographically verify it. For Praos (proto >= 7) there is only one VRF
    /// certificate, so this field is empty.
    #[serde(default)]
    pub nonce_vrf_proof: Vec<u8>,
}

/// VRF output
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VrfOutput {
    pub output: Vec<u8>,
    pub proof: Vec<u8>,
}

/// Operational certificate for block production
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationalCert {
    pub hot_vkey: Vec<u8>,
    pub sequence_number: u64,
    pub kes_period: u64,
    pub sigma: Vec<u8>,
}

/// Protocol version (major.minor)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolVersion {
    pub major: u64,
    pub minor: u64,
}

impl ProtocolVersion {
    pub fn era(&self) -> Era {
        match self.major {
            0 | 1 => Era::Byron,
            2 => Era::Shelley,
            3 => Era::Allegra,
            4 => Era::Mary,
            5 | 6 => Era::Alonzo,
            7 | 8 => Era::Babbage,
            9.. => Era::Conway,
        }
    }
}

/// A complete block with header and body
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Block {
    pub header: BlockHeader,
    pub transactions: Vec<Transaction>,
    pub era: Era,
    pub raw_cbor: Option<Vec<u8>>,
}

impl Block {
    pub fn hash(&self) -> &BlockHeaderHash {
        &self.header.header_hash
    }

    pub fn slot(&self) -> SlotNo {
        self.header.slot
    }

    pub fn block_number(&self) -> BlockNo {
        self.header.block_number
    }

    pub fn prev_hash(&self) -> &BlockHeaderHash {
        &self.header.prev_hash
    }

    pub fn tx_count(&self) -> usize {
        self.transactions.len()
    }

    pub fn point(&self) -> Point {
        Point::Specific(self.header.slot, self.header.header_hash)
    }

    pub fn tip(&self) -> Tip {
        Tip {
            point: self.point(),
            block_number: self.header.block_number,
        }
    }
}

/// Chain tip information
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tip {
    pub point: Point,
    pub block_number: BlockNo,
}

impl Tip {
    pub fn origin() -> Self {
        Tip {
            point: Point::Origin,
            block_number: BlockNo(0),
        }
    }
}

impl std::fmt::Display for Tip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (block {})", self.point, self.block_number)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::Hash;
    use crate::time::{BlockNo, SlotNo};

    fn test_block_hash() -> BlockHeaderHash {
        Hash::from_bytes([0xab; 32])
    }

    fn test_block_hash_2() -> BlockHeaderHash {
        Hash::from_bytes([0xcd; 32])
    }

    /// Helper: build a minimal BlockHeader for testing Block accessors.
    fn test_header(slot: u64, block_number: u64) -> BlockHeader {
        BlockHeader {
            header_hash: test_block_hash(),
            prev_hash: test_block_hash_2(),
            issuer_vkey: vec![],
            vrf_vkey: vec![],
            vrf_result: VrfOutput {
                output: vec![],
                proof: vec![],
            },
            block_number: BlockNo(block_number),
            slot: SlotNo(slot),
            epoch_nonce: Hash::from_bytes([0; 32]),
            body_size: 0,
            body_hash: Hash::from_bytes([0; 32]),
            operational_cert: OperationalCert {
                hot_vkey: vec![],
                sequence_number: 0,
                kes_period: 0,
                sigma: vec![],
            },
            protocol_version: ProtocolVersion { major: 9, minor: 0 },
            kes_signature: vec![],
            nonce_vrf_output: vec![],
            nonce_vrf_proof: vec![],
        }
    }

    fn test_block(slot: u64, block_number: u64, num_txs: usize) -> Block {
        Block {
            header: test_header(slot, block_number),
            transactions: (0..num_txs)
                .map(|_| Transaction::empty_with_hash(Hash::from_bytes([0; 32])))
                .collect(),
            era: Era::Conway,
            raw_cbor: None,
        }
    }

    // ========== Point ==========

    #[test]
    fn test_point_origin_slot_is_none() {
        assert_eq!(Point::Origin.slot(), None);
    }

    #[test]
    fn test_point_origin_hash_is_none() {
        assert_eq!(Point::Origin.hash(), None);
    }

    #[test]
    fn test_point_specific_slot() {
        let p = Point::Specific(SlotNo(42), test_block_hash());
        assert_eq!(p.slot(), Some(SlotNo(42)));
    }

    #[test]
    fn test_point_specific_hash() {
        let h = test_block_hash();
        let p = Point::Specific(SlotNo(0), h);
        assert_eq!(p.hash(), Some(&test_block_hash()));
    }

    #[test]
    fn test_point_display_origin() {
        assert_eq!(Point::Origin.to_string(), "origin");
    }

    #[test]
    fn test_point_display_specific() {
        let p = Point::Specific(SlotNo(100), test_block_hash());
        let s = p.to_string();
        // SlotNo displays as "slot:100"
        assert!(s.starts_with("slot:100@"));
        assert!(s.contains("abababab"));
    }

    #[test]
    fn test_point_ord_origin_less_than_specific() {
        let specific = Point::Specific(SlotNo(0), test_block_hash());
        assert!(Point::Origin < specific);
    }

    #[test]
    fn test_point_ord_origin_equal_origin() {
        assert_eq!(Point::Origin.cmp(&Point::Origin), std::cmp::Ordering::Equal);
    }

    #[test]
    fn test_point_ord_specific_greater_than_origin() {
        let specific = Point::Specific(SlotNo(0), test_block_hash());
        assert!(specific > Point::Origin);
    }

    #[test]
    fn test_point_ord_specific_by_slot() {
        let p1 = Point::Specific(SlotNo(10), test_block_hash());
        let p2 = Point::Specific(SlotNo(20), test_block_hash_2());
        assert!(p1 < p2);
    }

    #[test]
    fn test_point_ord_same_slot_is_equal() {
        // Ord compares by slot only, ignoring hash
        let p1 = Point::Specific(SlotNo(10), test_block_hash());
        let p2 = Point::Specific(SlotNo(10), test_block_hash_2());
        assert_eq!(p1.cmp(&p2), std::cmp::Ordering::Equal);
    }

    #[test]
    fn test_point_serde_roundtrip_origin() {
        let p = Point::Origin;
        let json = serde_json::to_string(&p).unwrap();
        let p2: Point = serde_json::from_str(&json).unwrap();
        assert_eq!(p, p2);
    }

    #[test]
    fn test_point_serde_roundtrip_specific() {
        let p = Point::Specific(SlotNo(999), test_block_hash());
        let json = serde_json::to_string(&p).unwrap();
        let p2: Point = serde_json::from_str(&json).unwrap();
        assert_eq!(p, p2);
    }

    // ========== ProtocolVersion::era() ==========

    #[test]
    fn test_protocol_version_era_byron() {
        assert_eq!(ProtocolVersion { major: 0, minor: 0 }.era(), Era::Byron);
        assert_eq!(ProtocolVersion { major: 1, minor: 0 }.era(), Era::Byron);
    }

    #[test]
    fn test_protocol_version_era_shelley() {
        assert_eq!(ProtocolVersion { major: 2, minor: 0 }.era(), Era::Shelley);
    }

    #[test]
    fn test_protocol_version_era_allegra() {
        assert_eq!(ProtocolVersion { major: 3, minor: 0 }.era(), Era::Allegra);
    }

    #[test]
    fn test_protocol_version_era_mary() {
        assert_eq!(ProtocolVersion { major: 4, minor: 0 }.era(), Era::Mary);
    }

    #[test]
    fn test_protocol_version_era_alonzo() {
        assert_eq!(ProtocolVersion { major: 5, minor: 0 }.era(), Era::Alonzo);
        assert_eq!(ProtocolVersion { major: 6, minor: 0 }.era(), Era::Alonzo);
    }

    #[test]
    fn test_protocol_version_era_babbage() {
        assert_eq!(ProtocolVersion { major: 7, minor: 0 }.era(), Era::Babbage);
        assert_eq!(ProtocolVersion { major: 8, minor: 0 }.era(), Era::Babbage);
    }

    #[test]
    fn test_protocol_version_era_conway() {
        assert_eq!(ProtocolVersion { major: 9, minor: 0 }.era(), Era::Conway);
        assert_eq!(
            ProtocolVersion {
                major: 10,
                minor: 0
            }
            .era(),
            Era::Conway
        );
        assert_eq!(
            ProtocolVersion {
                major: 100,
                minor: 0
            }
            .era(),
            Era::Conway
        );
    }

    // ========== Block accessors ==========

    #[test]
    fn test_block_hash_accessor() {
        let block = test_block(100, 5, 0);
        assert_eq!(block.hash(), &test_block_hash());
    }

    #[test]
    fn test_block_slot() {
        let block = test_block(42, 5, 0);
        assert_eq!(block.slot(), SlotNo(42));
    }

    #[test]
    fn test_block_number() {
        let block = test_block(0, 99, 0);
        assert_eq!(block.block_number(), BlockNo(99));
    }

    #[test]
    fn test_block_prev_hash() {
        let block = test_block(0, 0, 0);
        assert_eq!(block.prev_hash(), &test_block_hash_2());
    }

    #[test]
    fn test_block_tx_count() {
        assert_eq!(test_block(0, 0, 0).tx_count(), 0);
        assert_eq!(test_block(0, 0, 3).tx_count(), 3);
    }

    #[test]
    fn test_block_point() {
        let block = test_block(42, 5, 0);
        assert_eq!(
            block.point(),
            Point::Specific(SlotNo(42), test_block_hash())
        );
    }

    #[test]
    fn test_block_tip() {
        let block = test_block(42, 5, 0);
        let tip = block.tip();
        assert_eq!(tip.point, Point::Specific(SlotNo(42), test_block_hash()));
        assert_eq!(tip.block_number, BlockNo(5));
    }

    // ========== Tip ==========

    #[test]
    fn test_tip_origin() {
        let tip = Tip::origin();
        assert_eq!(tip.point, Point::Origin);
        assert_eq!(tip.block_number, BlockNo(0));
    }

    #[test]
    fn test_tip_display_origin() {
        let tip = Tip::origin();
        assert_eq!(tip.to_string(), "origin (block block:0)");
    }

    #[test]
    fn test_tip_display_specific() {
        let tip = Tip {
            point: Point::Specific(SlotNo(100), test_block_hash()),
            block_number: BlockNo(50),
        };
        let s = tip.to_string();
        assert!(s.starts_with("slot:100@"));
        assert!(s.ends_with("(block block:50)"));
    }

    #[test]
    fn test_tip_serde_roundtrip() {
        let tip = Tip {
            point: Point::Specific(SlotNo(100), test_block_hash()),
            block_number: BlockNo(50),
        };
        let json = serde_json::to_string(&tip).unwrap();
        let tip2: Tip = serde_json::from_str(&json).unwrap();
        assert_eq!(tip, tip2);
    }
}
