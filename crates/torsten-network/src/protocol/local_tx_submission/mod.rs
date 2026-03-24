//! N2C LocalTxSubmission server.
//!
//! Handles transaction submission from local clients (cardano-cli, etc.)
//! via the Unix domain socket (N2C) connection.
//!
//! ## Wire format
//! - `MsgSubmitTx` = `[0, [era_id, tx_bytes]]`
//! - `MsgAcceptTx` = `[1]`
//! - `MsgRejectTx` = `[2, [era_id, [reason]]]`
//! - `MsgDone` = `[3]`

pub mod server;
