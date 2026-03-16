//! Ouroboros Praos consensus: chain selection, epoch transitions, VRF leader checks.

pub mod chain_selection;
pub mod epoch;
pub mod praos;
pub mod slot_leader;

pub use chain_selection::{ChainPreference, ChainSelection};
pub use praos::{CryptoVerificationParams, OuroborosPraos, ValidationMode};
pub use slot_leader::{compute_leader_schedule, LeaderSlot};
