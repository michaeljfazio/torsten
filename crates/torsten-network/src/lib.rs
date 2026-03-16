pub(crate) mod bandwidth;
pub(crate) mod client;
pub(crate) mod dns;
pub(crate) mod governor;
pub(crate) mod miniprotocols;
pub(crate) mod multiplexer;
pub(crate) mod n2c;
pub(crate) mod n2c_client;
pub mod n2n_server;
pub(crate) mod peer;
pub(crate) mod peer_manager;
pub(crate) mod pipelined;
pub mod query_handler;
pub mod server;
pub(crate) mod tcp;

pub use bandwidth::TokenBucketRateLimiter;
pub use client::{
    BlockFetchPool, ChainSyncEvent, EbbInfo, HeaderBatchResult, HeaderInfo, NodeToNodeClient,
};
pub use dns::DnsResolver;
pub use miniprotocols::peersharing::{
    request_peers_from, PeerAddress, PeerSharingMessage, PeerSharingState,
};
pub use miniprotocols::txsubmission::{TxSubmissionClient, TxSubmissionError, TxSubmissionStats};
pub use n2c::{N2CServer, TxValidationError, TxValidator};
pub use n2c_client::N2CClient;
// Re-export mempool trait and types from torsten-primitives for convenience
pub use governor::{Governor, GovernorEvent, PeerTargets};
pub use n2c::encode_query_result;
pub use n2n_server::{
    BlockAnnouncement, BlockProvider, N2NRateLimitConfig, N2NServer, RollbackAnnouncement, TipInfo,
};
pub use peer::PeerConnection;
pub use peer_manager::{
    CircuitState, ConnectionDirection, DiffusionMode, PeerCategory, PeerManager, PeerManagerConfig,
    PeerPerformance,
};
pub use pipelined::PipelinedPeerClient;
pub use query_handler::{NodeStateSnapshot, QueryHandler, QueryResult};
pub use server::NodeServer;
pub use tcp::{configure_tcp_keepalive, TimeoutConfig};
pub use torsten_primitives::mempool::{
    MempoolAddError, MempoolAddResult, MempoolProvider, MempoolSnapshot,
};

/// Optional metrics callbacks for connection tracking.
/// Implemented by the node layer to bridge protocol-level events to the metrics system.
pub trait ConnectionMetrics: Send + Sync + 'static {
    fn on_connect(&self);
    fn on_disconnect(&self);
    fn on_error(&self, label: &str);
}
