//! # Torsten Formal Ledger Specification Conformance Tests
//!
//! This crate validates Torsten's ledger implementation against test vectors
//! derived from the Cardano formal ledger specification (Agda -> Haskell).
//!
//! ## Architecture
//!
//! The formal spec (https://github.com/IntersectMBO/formal-ledger-specifications)
//! defines STS (state transition system) rules in Agda, which are compiled to
//! Haskell via MAlonzo. A Haskell test vector generator calls these step functions
//! and serializes inputs/outputs as JSON test vectors.
//!
//! This crate consumes those test vectors and validates them against Torsten's
//! Rust implementation. The mapping between Agda abstract types and Torsten's
//! concrete types is handled by the [`adapters`] module.
//!
//! ## Supported Rules
//!
//! - **UTXO**: Transaction processing against UTxO state
//! - **CERT**: Certificate processing (delegation, registration)
//! - **GOV**: Governance actions (Conway era)
//! - **EPOCH**: Epoch boundary transitions

pub mod adapters;
pub mod runner;
pub mod schema;
