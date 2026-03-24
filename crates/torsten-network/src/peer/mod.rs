//! Peer management — lifecycle, reputation, discovery, and selection.
//!
//! ## Architecture
//! - [`manager`] — PeerManager: peer state, EWMA latency, reputation scoring
//! - [`governor`] — Governor: target-driven promotion/demotion decisions
//! - [`discovery`] — DNS, ledger-based, and peer sharing discovery
//! - [`selection`] — Peer selection scoring and address filtering

pub mod discovery;
pub mod governor;
pub mod manager;
pub mod selection;

pub use governor::{Governor, GovernorConfig, PeerTargets};
pub use manager::{PeerInfo, PeerManager, PeerState};
