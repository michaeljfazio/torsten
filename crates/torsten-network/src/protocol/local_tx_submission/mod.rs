//! N2C LocalTxSubmission server.
//!
//! Handles transaction submission from local clients (cardano-cli, etc.)
//! via the Unix domain socket (N2C) connection.
//!
//! ## Wire format
//! - `MsgSubmitTx` = `[0, [era_id, tx_bytes]]`
//! - `MsgAcceptTx` = `[1]`
//! - `MsgRejectTx` = `[2, [[era_id, [failure_0, failure_1, ...]]]]`
//! - `MsgDone` = `[3]`
//!
//! Rejection reasons are structured CBOR matching the Haskell `ApplyTxErr` encoding
//! with Conway-era predicate failure tags (see `encode` module).

pub mod encode;
pub mod server;
