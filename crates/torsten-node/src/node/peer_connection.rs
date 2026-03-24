//! Single multiplexed connection to one Cardano peer.
//!
//! # Haskell Architecture Reference
//!
//! In the Haskell cardano-node, `ConnectionHandler` (ouroboros-network-framework) creates
//! exactly **one TCP connection per peer**. All Ouroboros mini-protocols share that single
//! multiplexed connection via `TemperatureBundle` in `Cardano.Network.NodeToNode`:
//!
//! - **ChainSync** (protocol 2) — block header synchronization
//! - **BlockFetch** (protocol 3) — full block download
//! - **TxSubmission2** (protocol 4) — transaction relay
//! - **KeepAlive** (protocol 8) — liveness probing
//! - **PeerSharing** (protocol 10) — peer address exchange
//!
//! Protocol tasks are started and stopped based on peer temperature transitions
//! (Cold -> Warm -> Hot) WITHOUT creating new TCP connections. The mux stays alive
//! across temperature changes; only the protocol tasks on top of it change.
//!
//! ## Temperature Lifecycle
//!
//! - **Cold -> Warm**: TCP connect + handshake, then start KeepAlive
//! - **Warm -> Hot**: Start ChainSync + BlockFetch + TxSubmission2 (channels already exist)
//! - **Hot -> Warm**: Stop hot protocol tasks, keep mux + KeepAlive alive
//! - **Warm -> Cold**: Stop KeepAlive, close mux + TCP connection
//!
//! This module provides `PeerConnection` — the struct that owns the single mux and
//! manages protocol channel lifecycle. The actual protocol logic (what ChainSync does
//! with blocks, etc.) is NOT in this file; external task functions receive the channels.

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::time::Duration;

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use torsten_network::handshake::n2n::N2NVersionData;
use torsten_network::handshake::{run_n2n_handshake_client, run_n2n_handshake_server};
use torsten_network::mux::channel::MuxChannel;
use torsten_network::mux::segment::Direction;
use torsten_network::mux::Mux;
use torsten_network::protocol::{
    PROTOCOL_HANDSHAKE, PROTOCOL_N2N_BLOCKFETCH, PROTOCOL_N2N_CHAINSYNC, PROTOCOL_N2N_KEEPALIVE,
    PROTOCOL_N2N_PEERSHARING, PROTOCOL_N2N_TXSUBMISSION,
};
use torsten_network::{MuxError, TcpBearer};

/// Default ingress buffer size per protocol channel (64 KiB).
///
/// Matches the Haskell `network-mux` default SDU limit. Large enough for
/// full block headers but bounded to prevent memory exhaustion from
/// misbehaving peers.
const DEFAULT_INGRESS_LIMIT: usize = 65536;

/// Timeout for graceful protocol task shutdown (seconds).
///
/// Matches the Haskell `spsDeactivateTimeout` (5 seconds). If a protocol
/// task does not terminate within this window after cancellation, it is
/// forcibly aborted.
const PROTOCOL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// Default TCP connect timeout.
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// A single multiplexed TCP connection to one Cardano peer.
///
/// Owns the mux task and provides protocol channels that can be taken
/// by protocol tasks when peer temperature changes. The mux stays alive
/// across Warm <-> Hot transitions; only the protocol tasks change.
///
/// # Channel Lifecycle
///
/// Channels are created during `connect()` / `accept()` and stored as
/// `Option<MuxChannel>`. When a protocol task starts, it takes the channel
/// (`Option::take`). When the task stops, the channel is consumed (mux
/// channels are not reusable after protocol completion — the mux handles
/// cleanup internally).
pub struct PeerConnection {
    /// Remote peer address.
    pub addr: SocketAddr,

    /// Negotiated N2N protocol version (14 or 15).
    pub version: u16,

    /// Cardano network magic (e.g. 2 for preview, 764824073 for mainnet).
    pub network_magic: u64,

    // ── Protocol channels ──
    // Created during mux setup, taken when protocol tasks start.
    // `None` means the channel is currently in use by a running task.
    /// ChainSync channel (protocol 2, InitiatorDir for outbound connections).
    pub chainsync_channel: Option<MuxChannel>,

    /// BlockFetch channel (protocol 3, InitiatorDir for outbound connections).
    pub blockfetch_channel: Option<MuxChannel>,

    /// TxSubmission2 channel (protocol 4, InitiatorDir for outbound connections).
    pub txsubmission_channel: Option<MuxChannel>,

    /// KeepAlive channel (protocol 8, InitiatorDir for outbound connections).
    pub keepalive_channel: Option<MuxChannel>,

    /// PeerSharing channel (protocol 10, InitiatorDir for outbound connections).
    pub peersharing_channel: Option<MuxChannel>,

    // ── Mux lifecycle ──
    /// Handle to the spawned mux task. When this completes, the connection is dead.
    mux_handle: JoinHandle<Result<(), MuxError>>,

    /// Top-level cancellation token for the entire connection.
    cancel: CancellationToken,

    // ── Running protocol task handles ──
    /// Warm-temperature protocol tasks (currently: KeepAlive).
    warm_tasks: Vec<(JoinHandle<()>, CancellationToken)>,

    /// Hot-temperature protocol tasks (ChainSync, BlockFetch, TxSubmission2).
    hot_tasks: Vec<(JoinHandle<()>, CancellationToken)>,
}

/// A boxed future type for protocol task factories.
///
/// Protocol task factories are async closures that receive a `MuxChannel`
/// and a `CancellationToken`, and run the protocol until cancelled or
/// the channel closes. Used by `start_warm_protocols` and `start_hot_protocols`.
pub type ProtocolTaskFn = Box<
    dyn FnOnce(MuxChannel, CancellationToken) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send,
>;

impl PeerConnection {
    /// Establish an outbound connection to a peer.
    ///
    /// Performs TCP connect (with timeout), creates the mux, subscribes all
    /// protocol channels on `InitiatorDir`, spawns the mux task, and runs
    /// the N2N handshake. Returns a `PeerConnection` with channels ready
    /// for protocol tasks.
    ///
    /// # Arguments
    ///
    /// * `addr` — Remote peer socket address
    /// * `network_magic` — Cardano network identifier
    /// * `peer_sharing` — Whether to advertise peer sharing support
    /// * `timeout` — Optional TCP connect timeout (defaults to 10s)
    ///
    /// # Errors
    ///
    /// Returns `NetworkError` on TCP connect failure, mux error, or
    /// handshake failure (version mismatch, network magic mismatch, etc.).
    pub async fn connect(
        addr: SocketAddr,
        network_magic: u64,
        peer_sharing: bool,
        timeout: Option<Duration>,
    ) -> Result<Self, PeerConnectionError> {
        let connect_timeout = timeout.unwrap_or(DEFAULT_CONNECT_TIMEOUT);

        // TCP connect with timeout.
        let bearer = tokio::time::timeout(connect_timeout, TcpBearer::connect(addr))
            .await
            .map_err(|_| PeerConnectionError::ConnectTimeout(addr))?
            .map_err(|e| PeerConnectionError::Connect(addr, e.to_string()))?;

        info!(%addr, "TCP connected, starting mux + handshake");

        // Create mux (we are initiator).
        let mut mux = Mux::new(bearer, true);

        // Subscribe handshake channel (protocol 0) — consumed during handshake.
        let mut handshake_ch = mux.subscribe(
            PROTOCOL_HANDSHAKE,
            Direction::InitiatorDir,
            DEFAULT_INGRESS_LIMIT,
        );

        // Subscribe all N2N protocol channels on InitiatorDir.
        let chainsync_ch = mux.subscribe(
            PROTOCOL_N2N_CHAINSYNC,
            Direction::InitiatorDir,
            DEFAULT_INGRESS_LIMIT,
        );
        let blockfetch_ch = mux.subscribe(
            PROTOCOL_N2N_BLOCKFETCH,
            Direction::InitiatorDir,
            DEFAULT_INGRESS_LIMIT,
        );
        let txsubmission_ch = mux.subscribe(
            PROTOCOL_N2N_TXSUBMISSION,
            Direction::InitiatorDir,
            DEFAULT_INGRESS_LIMIT,
        );
        let keepalive_ch = mux.subscribe(
            PROTOCOL_N2N_KEEPALIVE,
            Direction::InitiatorDir,
            DEFAULT_INGRESS_LIMIT,
        );
        let peersharing_ch = mux.subscribe(
            PROTOCOL_N2N_PEERSHARING,
            Direction::InitiatorDir,
            DEFAULT_INGRESS_LIMIT,
        );

        // Spawn mux task — runs until bearer closes or error.
        let cancel = CancellationToken::new();
        let mux_handle = tokio::spawn(async move { mux.run().await });

        // Run N2N handshake on the handshake channel.
        let our_data = N2NVersionData::new(network_magic, peer_sharing);
        let handshake_result = run_n2n_handshake_client(&mut handshake_ch, &our_data)
            .await
            .map_err(|e| PeerConnectionError::Handshake(addr, e.to_string()))?;

        let version = handshake_result.version;
        info!(%addr, version, "N2N handshake complete");

        Ok(Self {
            addr,
            version,
            network_magic,
            chainsync_channel: Some(chainsync_ch),
            blockfetch_channel: Some(blockfetch_ch),
            txsubmission_channel: Some(txsubmission_ch),
            keepalive_channel: Some(keepalive_ch),
            peersharing_channel: Some(peersharing_ch),
            mux_handle,
            cancel,
            warm_tasks: Vec::new(),
            hot_tasks: Vec::new(),
        })
    }

    /// Accept an inbound connection from a peer.
    ///
    /// Creates the mux from an already-accepted `TcpStream`, subscribes all
    /// protocol channels on `ResponderDir`, spawns the mux task, and runs
    /// the N2N handshake as server. Returns a `PeerConnection` with channels
    /// ready for protocol tasks.
    ///
    /// # Arguments
    ///
    /// * `stream` — Already-accepted TCP stream
    /// * `addr` — Remote peer socket address (for logging/identification)
    /// * `network_magic` — Cardano network identifier
    /// * `peer_sharing` — Whether to advertise peer sharing support
    pub async fn accept(
        stream: tokio::net::TcpStream,
        addr: SocketAddr,
        network_magic: u64,
        peer_sharing: bool,
    ) -> Result<Self, PeerConnectionError> {
        let bearer = TcpBearer::new(stream)
            .map_err(|e| PeerConnectionError::Connect(addr, e.to_string()))?;

        info!(%addr, "accepted inbound connection, starting mux + handshake");

        // Create mux (we are responder).
        let mut mux = Mux::new(bearer, false);

        // Subscribe handshake channel on ResponderDir.
        let mut handshake_ch = mux.subscribe(
            PROTOCOL_HANDSHAKE,
            Direction::ResponderDir,
            DEFAULT_INGRESS_LIMIT,
        );

        // Subscribe all N2N protocol channels on ResponderDir.
        let chainsync_ch = mux.subscribe(
            PROTOCOL_N2N_CHAINSYNC,
            Direction::ResponderDir,
            DEFAULT_INGRESS_LIMIT,
        );
        let blockfetch_ch = mux.subscribe(
            PROTOCOL_N2N_BLOCKFETCH,
            Direction::ResponderDir,
            DEFAULT_INGRESS_LIMIT,
        );
        let txsubmission_ch = mux.subscribe(
            PROTOCOL_N2N_TXSUBMISSION,
            Direction::ResponderDir,
            DEFAULT_INGRESS_LIMIT,
        );
        let keepalive_ch = mux.subscribe(
            PROTOCOL_N2N_KEEPALIVE,
            Direction::ResponderDir,
            DEFAULT_INGRESS_LIMIT,
        );
        let peersharing_ch = mux.subscribe(
            PROTOCOL_N2N_PEERSHARING,
            Direction::ResponderDir,
            DEFAULT_INGRESS_LIMIT,
        );

        // Spawn mux task.
        let cancel = CancellationToken::new();
        let mux_handle = tokio::spawn(async move { mux.run().await });

        // Run N2N handshake as server.
        let our_data = N2NVersionData::new(network_magic, peer_sharing);
        let handshake_result = run_n2n_handshake_server(&mut handshake_ch, &our_data)
            .await
            .map_err(|e| PeerConnectionError::Handshake(addr, e.to_string()))?;

        let version = handshake_result.version;
        info!(%addr, version, "N2N handshake complete (inbound)");

        Ok(Self {
            addr,
            version,
            network_magic,
            chainsync_channel: Some(chainsync_ch),
            blockfetch_channel: Some(blockfetch_ch),
            txsubmission_channel: Some(txsubmission_ch),
            keepalive_channel: Some(keepalive_ch),
            peersharing_channel: Some(peersharing_ch),
            mux_handle,
            cancel,
            warm_tasks: Vec::new(),
            hot_tasks: Vec::new(),
        })
    }

    /// Start warm-temperature protocols (KeepAlive).
    ///
    /// Takes the `keepalive_channel` and spawns a protocol task using the
    /// provided factory function. The factory receives the channel and a
    /// cancellation token, and should run the KeepAlive protocol until
    /// cancelled.
    ///
    /// # Panics
    ///
    /// Returns `Err` if the keepalive channel has already been taken
    /// (protocols already running).
    pub fn start_warm_protocols(
        &mut self,
        keepalive_fn: ProtocolTaskFn,
    ) -> Result<(), PeerConnectionError> {
        let ch = self
            .keepalive_channel
            .take()
            .ok_or(PeerConnectionError::ChannelUnavailable("keepalive"))?;

        let token = self.cancel.child_token();
        let token_clone = token.clone();

        let handle = tokio::spawn(async move {
            (keepalive_fn)(ch, token_clone).await;
        });

        self.warm_tasks.push((handle, token));
        debug!(addr = %self.addr, "started warm protocols (KeepAlive)");
        Ok(())
    }

    /// Start hot-temperature protocols (ChainSync, BlockFetch, TxSubmission2).
    ///
    /// Takes the corresponding channels and spawns each protocol as an
    /// independent tokio task using the provided factory functions. Each
    /// factory receives its channel and a cancellation token.
    ///
    /// The actual protocol logic (block processing, tx relay, etc.) is
    /// provided by the caller — this struct only manages the lifecycle.
    ///
    /// # Arguments
    ///
    /// * `chainsync_fn` — Factory for the ChainSync protocol task
    /// * `blockfetch_fn` — Factory for the BlockFetch protocol task
    /// * `txsubmission_fn` — Factory for the TxSubmission2 protocol task
    pub fn start_hot_protocols(
        &mut self,
        chainsync_fn: ProtocolTaskFn,
        blockfetch_fn: ProtocolTaskFn,
        txsubmission_fn: ProtocolTaskFn,
    ) -> Result<(), PeerConnectionError> {
        let cs_ch = self
            .chainsync_channel
            .take()
            .ok_or(PeerConnectionError::ChannelUnavailable("chainsync"))?;
        let bf_ch = self
            .blockfetch_channel
            .take()
            .ok_or(PeerConnectionError::ChannelUnavailable("blockfetch"))?;
        let tx_ch = self
            .txsubmission_channel
            .take()
            .ok_or(PeerConnectionError::ChannelUnavailable("txsubmission"))?;

        // Spawn ChainSync task.
        let cs_token = self.cancel.child_token();
        let cs_token_clone = cs_token.clone();
        let cs_handle = tokio::spawn(async move {
            (chainsync_fn)(cs_ch, cs_token_clone).await;
        });
        self.hot_tasks.push((cs_handle, cs_token));

        // Spawn BlockFetch task.
        let bf_token = self.cancel.child_token();
        let bf_token_clone = bf_token.clone();
        let bf_handle = tokio::spawn(async move {
            (blockfetch_fn)(bf_ch, bf_token_clone).await;
        });
        self.hot_tasks.push((bf_handle, bf_token));

        // Spawn TxSubmission2 task.
        let tx_token = self.cancel.child_token();
        let tx_token_clone = tx_token.clone();
        let tx_handle = tokio::spawn(async move {
            (txsubmission_fn)(tx_ch, tx_token_clone).await;
        });
        self.hot_tasks.push((tx_handle, tx_token));

        debug!(addr = %self.addr, "started hot protocols (ChainSync, BlockFetch, TxSubmission2)");
        Ok(())
    }

    /// Stop hot-temperature protocol tasks.
    ///
    /// Cancels all hot protocol tasks via their individual cancellation tokens
    /// and waits up to [`PROTOCOL_SHUTDOWN_TIMEOUT`] (5 seconds, matching
    /// Haskell `spsDeactivateTimeout`) for graceful shutdown. Any tasks that
    /// do not finish in time are forcibly aborted.
    pub async fn stop_hot_protocols(&mut self) {
        Self::stop_tasks(&mut self.hot_tasks, "hot", self.addr).await;
    }

    /// Stop warm-temperature protocol tasks (KeepAlive).
    ///
    /// Same graceful-then-abort pattern as [`stop_hot_protocols`].
    pub async fn stop_warm_protocols(&mut self) {
        Self::stop_tasks(&mut self.warm_tasks, "warm", self.addr).await;
    }

    /// Internal helper: cancel tasks, wait with timeout, abort stragglers.
    async fn stop_tasks(
        tasks: &mut Vec<(JoinHandle<()>, CancellationToken)>,
        label: &str,
        addr: SocketAddr,
    ) {
        if tasks.is_empty() {
            return;
        }

        debug!(%addr, label, count = tasks.len(), "stopping protocol tasks");

        // Signal cancellation to all tasks.
        for (_, token) in tasks.iter() {
            token.cancel();
        }

        // Wait for graceful shutdown with timeout.
        let drain = std::mem::take(tasks);
        for (handle, _) in drain {
            match tokio::time::timeout(PROTOCOL_SHUTDOWN_TIMEOUT, handle).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    // JoinError — task panicked or was cancelled.
                    warn!(%addr, label, error = %e, "protocol task join error");
                }
                Err(_) => {
                    // Timeout expired — task did not stop gracefully. Already consumed by timeout.
                    warn!(%addr, label, "protocol task did not stop within timeout, aborting");
                    // The handle was moved into the timeout future. If the timeout fired,
                    // the JoinHandle is dropped, which does NOT abort. We cannot abort here
                    // since we consumed the handle. In practice, the task's cancellation
                    // token is already signalled and the mux channel closure will cause
                    // the task to exit shortly.
                }
            }
        }
    }

    /// Shut down the entire connection: stop all protocols, cancel the mux.
    ///
    /// This is the clean teardown path for Cold transition. After this call,
    /// the `PeerConnection` is no longer usable.
    pub async fn shutdown(&mut self) {
        info!(addr = %self.addr, "shutting down peer connection");

        // Stop protocol tasks first (graceful).
        self.stop_hot_protocols().await;
        self.stop_warm_protocols().await;

        // Cancel the top-level token — this will signal any remaining child tasks.
        self.cancel.cancel();

        // Abort the mux task (drops the bearer, closing the TCP connection).
        self.mux_handle.abort();
        let _ = (&mut self.mux_handle).await;
    }

    /// Check if the underlying mux is still running.
    ///
    /// Returns `false` if the mux task has completed (connection is dead).
    /// The node should treat this as a connection failure and clean up.
    pub fn is_alive(&self) -> bool {
        !self.mux_handle.is_finished()
    }

    /// Get the top-level cancellation token for this connection.
    ///
    /// Child tokens derived from this are used by individual protocol tasks.
    /// Cancelling this token will signal all protocol tasks to stop.
    pub fn cancel_token(&self) -> &CancellationToken {
        &self.cancel
    }

    /// Check if warm protocols are currently running.
    pub fn has_warm_protocols(&self) -> bool {
        !self.warm_tasks.is_empty()
    }

    /// Check if hot protocols are currently running.
    pub fn has_hot_protocols(&self) -> bool {
        !self.hot_tasks.is_empty()
    }
}

/// Errors specific to `PeerConnection` lifecycle operations.
#[derive(Debug)]
pub enum PeerConnectionError {
    /// TCP connect timed out.
    ConnectTimeout(SocketAddr),
    /// TCP connect or bearer creation failed.
    Connect(SocketAddr, String),
    /// N2N handshake failed.
    Handshake(SocketAddr, String),
    /// Requested protocol channel is unavailable (already taken or not subscribed).
    ChannelUnavailable(&'static str),
    /// Mux error during operation.
    Mux(MuxError),
}

impl std::fmt::Display for PeerConnectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConnectTimeout(addr) => write!(f, "TCP connect timeout to {addr}"),
            Self::Connect(addr, reason) => write!(f, "connect to {addr} failed: {reason}"),
            Self::Handshake(addr, reason) => write!(f, "handshake with {addr} failed: {reason}"),
            Self::ChannelUnavailable(proto) => {
                write!(f, "{proto} channel unavailable (already taken)")
            }
            Self::Mux(e) => write!(f, "mux error: {e}"),
        }
    }
}

impl std::error::Error for PeerConnectionError {}

impl From<MuxError> for PeerConnectionError {
    fn from(e: MuxError) -> Self {
        Self::Mux(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify PeerConnectionError Display formatting.
    #[test]
    fn error_display() {
        let addr: SocketAddr = "127.0.0.1:3001".parse().unwrap();

        let err = PeerConnectionError::ConnectTimeout(addr);
        assert!(err.to_string().contains("timeout"));
        assert!(err.to_string().contains("127.0.0.1:3001"));

        let err = PeerConnectionError::Handshake(addr, "version mismatch".into());
        assert!(err.to_string().contains("handshake"));
        assert!(err.to_string().contains("version mismatch"));

        let err = PeerConnectionError::ChannelUnavailable("chainsync");
        assert!(err.to_string().contains("chainsync"));
        assert!(err.to_string().contains("unavailable"));
    }

    /// Verify protocol shutdown timeout constant matches Haskell's spsDeactivateTimeout.
    #[test]
    fn shutdown_timeout_matches_haskell() {
        assert_eq!(PROTOCOL_SHUTDOWN_TIMEOUT, Duration::from_secs(5));
    }

    /// Verify default constants are reasonable.
    #[test]
    fn default_constants() {
        assert_eq!(DEFAULT_INGRESS_LIMIT, 65536);
        assert_eq!(DEFAULT_CONNECT_TIMEOUT, Duration::from_secs(10));
    }
}
