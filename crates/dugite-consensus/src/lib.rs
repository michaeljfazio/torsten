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

/// The maximum protocol version that this node software supports.
///
/// This is a **capability signal**, not a reflection of the current on-chain protocol
/// version from the ledger's `PParams`. It tells the network "my software supports up
/// to this version" and is used for intra-era upgrade readiness voting. When enough
/// stake produces blocks with a higher minor version, the network can enact a minor
/// protocol bump via governance.
///
/// Equivalent to Haskell's `cardanoProtocolVersion` in
/// `Cardano.Node.Protocol.Cardano` — a compile-time constant that is stamped into
/// every forged block header, and also used to derive `MaxMajorProtVer` for the
/// `ObsoleteNode` check.
///
/// This value must be updated when Dugite adds support for newer protocol features.
/// Current value matches cardano-node 10.2.x / master (major=10, minor=8).
pub const NODE_PROTOCOL_VERSION: (u64, u64) = (10, 8);
