//! Connection manager — orchestrates peer connections and protocol lifecycle.
//!
//! The connection manager is the top-level component that:
//! - Accepts inbound TCP connections and Unix socket connections
//! - Establishes outbound TCP connections to peers
//! - Runs the handshake protocol on new connections
//! - Spawns protocol tasks (ChainSync, BlockFetch, etc.) based on peer temperature
//! - Handles connection deduplication and simultaneous open
//! - Enforces rate limits and connection limits

pub mod handler;
pub mod manager;
pub mod state;

pub use handler::ConnectionHandler;
pub use manager::ConnectionManager;
pub use state::ConnectionState;
