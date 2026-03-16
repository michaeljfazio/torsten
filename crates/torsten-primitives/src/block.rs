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
