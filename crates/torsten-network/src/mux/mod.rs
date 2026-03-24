//! Ouroboros multiplexer.
//!
//! Multiplexes multiple mini-protocols over a single bearer (TCP or Unix socket).
//! Matches the Haskell `network-mux` architecture with SDU framing, direction bits,
//! and per-protocol channels.

pub mod segment;

// These modules are added in Task 4:
// pub mod channel;
// pub mod egress;
// pub mod ingress;

pub use segment::{Direction, SduHeader, HEADER_SIZE};
