//! N2C LocalChainSync server — sends full blocks to local clients.
//!
//! Same state machine as N2N ChainSync but sends full blocks (not just headers),
//! wrapped in HFC era encoding: `[era_id, CBOR_tag_24(block_bytes)]`.
//!
//! The ingress queue is effectively unlimited (0xFFFFFFFF) since N2C operates
//! over Unix domain sockets with no bandwidth constraints.

pub mod server;
