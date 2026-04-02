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

pub mod local_state_query;

// All protocol modules are now implemented.

// ─── Shared HFC helpers ───

/// CBOR tag number for embedded CBOR (RFC 7049 §2.4.4.1 / RFC 8949 §3.4.5.1).
pub(crate) const CBOR_TAG_EMBEDDED: u64 = 24;

/// Convert a pallas/ImmutableDB block storage era tag to the HFC NS index used
/// in the N2N wire format.
///
/// | Era     | Storage tag (pallas) | HFC NS index |
/// |---------|----------------------|--------------|
/// | Byron   | 0 or 1               | 0            |
/// | Shelley | 2                    | 1            |
/// | Allegra | 3                    | 2            |
/// | Mary    | 4                    | 3            |
/// | Alonzo  | 5                    | 4            |
/// | Babbage | 6                    | 5            |
/// | Conway  | 7                    | 6            |
pub(crate) fn storage_era_tag_to_hfc_index(storage_era_tag: u64) -> Result<u8, String> {
    match storage_era_tag {
        // Byron: pallas uses both 0 and 1 depending on context; both map to HFC index 0.
        0 | 1 => Ok(0),
        2 => Ok(1), // Shelley
        3 => Ok(2), // Allegra
        4 => Ok(3), // Mary
        5 => Ok(4), // Alonzo
        6 => Ok(5), // Babbage
        7 => Ok(6), // Conway
        other => Err(format!(
            "unknown storage era tag {other}: cannot convert to HFC index"
        )),
    }
}

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
