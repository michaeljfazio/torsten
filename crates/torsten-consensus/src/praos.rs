use thiserror::Error;
use torsten_primitives::block::{Block, BlockHeader, Point, Tip};
use torsten_primitives::hash::{BlockHeaderHash, Hash32};
use torsten_primitives::time::{BlockNo, EpochLength, EpochNo, SlotNo};

#[derive(Error, Debug)]
pub enum ConsensusError {
    #[error("Invalid block: {0}")]
    InvalidBlock(String),
    #[error("Block from future slot: current={current}, block={block}")]
    FutureBlock { current: u64, block: u64 },
    #[error("Not a slot leader")]
    NotSlotLeader,
    #[error("Invalid VRF proof")]
    InvalidVrfProof,
    #[error("Invalid KES signature")]
    InvalidKesSignature,
    #[error("Invalid operational certificate")]
    InvalidOperationalCert,
    #[error("Block does not extend chain")]
    DoesNotExtendChain,
}

/// Active slot coefficient (f) - probability that a slot has a block
/// Mainnet value: 1/20 = 0.05 (one block every ~20 seconds on average)
pub const ACTIVE_SLOT_COEFF: f64 = 0.05;

/// Security parameter k
pub const SECURITY_PARAM: u64 = 2160;

/// Ouroboros Praos consensus engine
pub struct OuroborosPraos {
    /// Active slot coefficient
    pub active_slot_coeff: f64,
    /// Security parameter
    pub security_param: u64,
    /// Epoch length in slots
    pub epoch_length: EpochLength,
    /// Current tip
    pub tip: Tip,
}

impl OuroborosPraos {
    pub fn new() -> Self {
        OuroborosPraos {
            active_slot_coeff: ACTIVE_SLOT_COEFF,
            security_param: SECURITY_PARAM,
            epoch_length: torsten_primitives::time::mainnet_epoch_length(),
            tip: Tip::origin(),
        }
    }

    pub fn with_params(active_slot_coeff: f64, security_param: u64, epoch_length: EpochLength) -> Self {
        OuroborosPraos {
            active_slot_coeff,
            security_param,
            epoch_length,
            tip: Tip::origin(),
        }
    }

    /// Validate a block header against consensus rules
    pub fn validate_header(&self, header: &BlockHeader, current_slot: SlotNo) -> Result<(), ConsensusError> {
        // Block must not be from the future
        if header.slot > current_slot {
            return Err(ConsensusError::FutureBlock {
                current: current_slot.0,
                block: header.slot.0,
            });
        }

        // Block number must be sequential
        if self.tip.block_number.0 > 0 && header.block_number.0 != self.tip.block_number.0 + 1 {
            // Allow for chain selection to handle forks
        }

        // Verify VRF proof (placeholder - needs real VRF verification)
        // In production: verify that the VRF output proves the pool is the slot leader

        // Verify KES signature (placeholder - needs real KES verification)
        // In production: verify the block signature with the KES key

        // Verify operational certificate (placeholder)
        // In production: verify the opcert chain to the cold key

        Ok(())
    }

    /// Check if a slot is within the stability window (last k blocks)
    pub fn is_in_stability_window(&self, slot: SlotNo) -> bool {
        match self.tip.point.slot() {
            Some(tip_slot) => {
                tip_slot.0.saturating_sub(self.stability_window()) <= slot.0
            }
            None => true,
        }
    }

    /// The stability window: 3k/f slots
    pub fn stability_window(&self) -> u64 {
        (3.0 * self.security_param as f64 / self.active_slot_coeff) as u64
    }

    /// Calculate which epoch a slot belongs to
    pub fn slot_to_epoch(&self, slot: SlotNo) -> EpochNo {
        slot.to_epoch(self.epoch_length)
    }

    /// Get the first slot of an epoch
    pub fn epoch_first_slot(&self, epoch: EpochNo) -> SlotNo {
        SlotNo(epoch.0 * self.epoch_length.0)
    }

    /// Check if we're at an epoch boundary
    pub fn is_epoch_boundary(&self, slot: SlotNo) -> bool {
        slot.0 % self.epoch_length.0 == 0
    }

    /// Maximum rollback depth
    pub fn max_rollback(&self) -> u64 {
        self.security_param
    }

    /// Update the tip
    pub fn update_tip(&mut self, tip: Tip) {
        self.tip = tip;
    }
}

impl Default for OuroborosPraos {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_praos() {
        let praos = OuroborosPraos::new();
        assert_eq!(praos.tip, Tip::origin());
        assert!((praos.active_slot_coeff - 0.05).abs() < f64::EPSILON);
        assert_eq!(praos.security_param, 2160);
    }

    #[test]
    fn test_stability_window() {
        let praos = OuroborosPraos::new();
        // 3 * 2160 / 0.05 = 129600
        assert_eq!(praos.stability_window(), 129600);
    }

    #[test]
    fn test_slot_to_epoch() {
        let praos = OuroborosPraos::new();
        assert_eq!(praos.slot_to_epoch(SlotNo(0)), EpochNo(0));
        assert_eq!(praos.slot_to_epoch(SlotNo(431999)), EpochNo(0));
        assert_eq!(praos.slot_to_epoch(SlotNo(432000)), EpochNo(1));
        assert_eq!(praos.slot_to_epoch(SlotNo(864000)), EpochNo(2));
    }

    #[test]
    fn test_epoch_first_slot() {
        let praos = OuroborosPraos::new();
        assert_eq!(praos.epoch_first_slot(EpochNo(0)), SlotNo(0));
        assert_eq!(praos.epoch_first_slot(EpochNo(1)), SlotNo(432000));
    }

    #[test]
    fn test_epoch_boundary() {
        let praos = OuroborosPraos::new();
        assert!(praos.is_epoch_boundary(SlotNo(0)));
        assert!(praos.is_epoch_boundary(SlotNo(432000)));
        assert!(!praos.is_epoch_boundary(SlotNo(1)));
    }

    #[test]
    fn test_max_rollback() {
        let praos = OuroborosPraos::new();
        assert_eq!(praos.max_rollback(), 2160);
    }

    #[test]
    fn test_future_block_rejected() {
        let praos = OuroborosPraos::new();
        let header = BlockHeader {
            header_hash: Hash32::ZERO,
            prev_hash: Hash32::ZERO,
            issuer_vkey: vec![],
            vrf_vkey: vec![],
            vrf_result: torsten_primitives::block::VrfOutput {
                output: vec![],
                proof: vec![],
            },
            block_number: BlockNo(1),
            slot: SlotNo(200),
            epoch_nonce: Hash32::ZERO,
            body_size: 0,
            body_hash: Hash32::ZERO,
            operational_cert: torsten_primitives::block::OperationalCert {
                hot_vkey: vec![],
                sequence_number: 0,
                kes_period: 0,
                sigma: vec![],
            },
            protocol_version: torsten_primitives::block::ProtocolVersion {
                major: 9,
                minor: 0,
            },
        };

        let result = praos.validate_header(&header, SlotNo(100));
        assert!(matches!(result, Err(ConsensusError::FutureBlock { .. })));
    }

    #[test]
    fn test_valid_header() {
        let praos = OuroborosPraos::new();
        let header = BlockHeader {
            header_hash: Hash32::ZERO,
            prev_hash: Hash32::ZERO,
            issuer_vkey: vec![],
            vrf_vkey: vec![],
            vrf_result: torsten_primitives::block::VrfOutput {
                output: vec![],
                proof: vec![],
            },
            block_number: BlockNo(1),
            slot: SlotNo(100),
            epoch_nonce: Hash32::ZERO,
            body_size: 0,
            body_hash: Hash32::ZERO,
            operational_cert: torsten_primitives::block::OperationalCert {
                hot_vkey: vec![],
                sequence_number: 0,
                kes_period: 0,
                sigma: vec![],
            },
            protocol_version: torsten_primitives::block::ProtocolVersion {
                major: 9,
                minor: 0,
            },
        };

        let result = praos.validate_header(&header, SlotNo(200));
        assert!(result.is_ok());
    }
}
