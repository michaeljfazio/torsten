//! Ouroboros Praos consensus: chain selection, epoch transitions, VRF leader checks,
//! and HFC era history tracking.

pub mod chain_fragment;
pub mod chain_selection;
pub mod epoch;
pub mod era_history;
pub mod overlay;
pub mod praos;
pub mod slot_leader;

pub use chain_selection::{ChainPreference, ChainSelection, DensityWindow};
pub use era_history::{Bound, EraHistory, EraParams, EraSummaryEntry, PastHorizonError};
pub use overlay::{OBftSlot, OverlayContext};
pub use praos::{CryptoVerificationParams, OuroborosPraos, ValidationMode};
pub use slot_leader::{compute_leader_schedule, LeaderSlot};
