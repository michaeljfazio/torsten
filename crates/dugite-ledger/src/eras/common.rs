//! Shared helpers used across multiple era rule implementations.
//!
//! These are NOT on the EraRules trait — they are internal building blocks
//! that era impls compose to avoid duplicating logic. The pattern is
//! composition over inheritance.
