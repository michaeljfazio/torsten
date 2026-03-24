//! Ouroboros mini-protocol implementations.
//!
//! Each mini-protocol runs over a [`MuxChannel`] and follows the Ouroboros
//! state machine model with typed agency (client vs server turns).
//!
//! ## Protocol ID constants
//! These match the Haskell `MiniProtocolNum` assignments from
//! `ouroboros-network/network-mux/src/Network/Mux/Types.hs`.

pub mod blockfetch;
pub mod chainsync;
pub mod keepalive;

pub mod peersharing;
pub mod txsubmission;

pub mod local_chainsync;
pub mod local_tx_submission;

pub mod local_tx_monitor;

// Future protocol modules:
// pub mod local_state_query;

// ─── N2N Protocol IDs ───

/// Handshake protocol (both N2N and N2C, always protocol 0).
pub const PROTOCOL_HANDSHAKE: u16 = 0;
// Protocol ID 1 is reserved (unused, silently discarded by ingress).

/// N2N ChainSync — header-only chain synchronization.
pub const PROTOCOL_N2N_CHAINSYNC: u16 = 2;
/// N2N BlockFetch — download full blocks by range.
pub const PROTOCOL_N2N_BLOCKFETCH: u16 = 3;
/// N2N TxSubmission2 — pull-based transaction exchange.
pub const PROTOCOL_N2N_TXSUBMISSION: u16 = 4;
/// N2N KeepAlive — periodic ping/pong with RTT measurement.
pub const PROTOCOL_N2N_KEEPALIVE: u16 = 8;
/// N2N PeerSharing — peer address exchange.
pub const PROTOCOL_N2N_PEERSHARING: u16 = 10;

// ─── N2C Protocol IDs ───

/// N2C LocalChainSync — full-block chain synchronization.
pub const PROTOCOL_N2C_CHAINSYNC: u16 = 5;
/// N2C LocalTxSubmission — submit transactions for validation.
pub const PROTOCOL_N2C_TXSUBMISSION: u16 = 6;
/// N2C LocalStateQuery — query ledger state.
pub const PROTOCOL_N2C_STATEQUERY: u16 = 7;
/// N2C LocalTxMonitor — monitor mempool state.
pub const PROTOCOL_N2C_TXMONITOR: u16 = 9;
