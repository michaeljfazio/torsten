//! High-level N2C (node-to-client) connection client.
//!
//! Provides a convenience wrapper that composes Bearer → Mux → Handshake → protocol
//! channels into a single connected client suitable for CLI tools and other consumers.
//!
//! This is NOT the N2C server (which runs inside the node). This is the client
//! that connects TO the node via Unix domain socket.

use std::path::Path;

use crate::bearer::unix::UnixBearer;
use crate::error::NetworkError;
use crate::mux::channel::MuxChannel;
use crate::mux::{Direction, Mux};

/// High-level N2C client connected to a Cardano node via Unix socket.
///
/// After construction via [`connect`](Self::connect), provides access to
/// protocol channels for LocalStateQuery, LocalTxSubmission, and LocalTxMonitor.
pub struct N2CClient {
    /// Negotiated protocol version.
    pub version: u16,
    /// LocalStateQuery channel (protocol 7).
    pub state_query_channel: MuxChannel,
    /// LocalTxSubmission channel (protocol 6).
    pub tx_submission_channel: MuxChannel,
    /// LocalTxMonitor channel (protocol 9).
    pub tx_monitor_channel: MuxChannel,
    /// LocalChainSync channel (protocol 5).
    pub chain_sync_channel: MuxChannel,
    /// Mux task handle — kept alive to sustain the connection.
    _mux_handle: tokio::task::JoinHandle<Result<(), crate::error::MuxError>>,
}

impl N2CClient {
    /// Connect to a Cardano node via Unix domain socket.
    ///
    /// Performs the N2C handshake and returns a connected client with
    /// protocol channels ready for use.
    pub async fn connect<P: AsRef<Path>>(socket_path: P) -> Result<Self, NetworkError> {
        let stream = tokio::net::UnixStream::connect(socket_path.as_ref())
            .await
            .map_err(|e| NetworkError::Bearer(crate::error::BearerError::Io(e)))?;

        let bearer = UnixBearer::new(stream);
        let mut mux = Mux::new(bearer, true); // we are initiator

        // Subscribe protocol channels
        let mut handshake_channel = mux.subscribe(0, Direction::InitiatorDir, 65536); // Handshake
        let state_query_channel = mux.subscribe(7, Direction::InitiatorDir, 1_048_576); // LocalStateQuery
        let tx_submission_channel = mux.subscribe(6, Direction::InitiatorDir, 65536); // LocalTxSubmission
        let tx_monitor_channel = mux.subscribe(9, Direction::InitiatorDir, 65536); // LocalTxMonitor
        let chain_sync_channel = mux.subscribe(5, Direction::InitiatorDir, 4_194_304); // LocalChainSync

        // Start the mux
        let mux_handle = tokio::spawn(async move { mux.run().await });

        // Run N2C handshake
        let our_data = crate::handshake::n2c::N2CVersionData::new(0); // magic=0 for N2C (ignored)
        let handshake_result =
            crate::handshake::run_n2c_handshake_client(&mut handshake_channel, &our_data)
                .await
                .map_err(|e| NetworkError::Handshake(e))?;

        Ok(Self {
            version: handshake_result.version,
            state_query_channel,
            tx_submission_channel,
            tx_monitor_channel,
            chain_sync_channel,
            _mux_handle: mux_handle,
        })
    }

    /// Get the negotiated N2C protocol version.
    pub fn version(&self) -> u16 {
        self.version
    }

    /// Send raw CBOR bytes on the LocalStateQuery channel.
    pub async fn send_query(&mut self, msg: Vec<u8>) -> Result<(), NetworkError> {
        self.state_query_channel
            .send(msg)
            .await
            .map_err(NetworkError::Mux)
    }

    /// Receive raw CBOR bytes from the LocalStateQuery channel.
    pub async fn recv_query(&mut self) -> Result<Vec<u8>, NetworkError> {
        self.state_query_channel
            .recv()
            .await
            .map_err(NetworkError::Mux)
    }

    /// Send raw CBOR bytes on the LocalTxSubmission channel.
    pub async fn send_tx_submission(&mut self, msg: Vec<u8>) -> Result<(), NetworkError> {
        self.tx_submission_channel
            .send(msg)
            .await
            .map_err(NetworkError::Mux)
    }

    /// Receive raw CBOR bytes from the LocalTxSubmission channel.
    pub async fn recv_tx_submission(&mut self) -> Result<Vec<u8>, NetworkError> {
        self.tx_submission_channel
            .recv()
            .await
            .map_err(NetworkError::Mux)
    }
}
