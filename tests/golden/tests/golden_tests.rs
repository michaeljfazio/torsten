//! Golden test validation against official Haskell cardano-node test vectors.

mod cbor_golden;
// N2C encoding and query tests have moved to dugite-node::n2c_query
// (the query encoding logic was ported from dugite-network to dugite-node
// during the networking layer rewrite)
// mod n2c_encoding;
// mod n2c_queries;
mod vrf_nonintegral;
