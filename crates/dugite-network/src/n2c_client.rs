//! High-level N2C (node-to-client) connection client.
//!
//! Provides a convenience wrapper that composes Bearer -> Mux -> Handshake -> protocol
//! channels into a single connected client suitable for CLI tools and other consumers.
//!
//! This is NOT the N2C server (which runs inside the node). This is the client
//! that connects TO the node via Unix domain socket.

use std::path::Path;

use tracing::debug;

use crate::bearer::unix::UnixBearer;
use crate::error::NetworkError;
use crate::mux::channel::MuxChannel;
use crate::mux::{Direction, Mux};

/// Result of a tip query.
#[derive(Debug, Clone)]
pub struct TipResult {
    /// Slot number at the tip.
    pub slot: u64,
    /// Block header hash (32 bytes).
    pub hash: Vec<u8>,
    /// Block number at the tip.
    pub block_no: u64,
    /// Epoch number (filled in by caller if needed).
    pub epoch: u64,
    /// Era index (filled in by caller if needed).
    pub era: u32,
}

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
    /// Mux task handle -- kept alive to sustain the connection.
    _mux_handle: tokio::task::JoinHandle<Result<(), crate::error::MuxError>>,
}

impl N2CClient {
    /// Connect to a Cardano node via Unix domain socket.
    ///
    /// Performs the N2C handshake with the given `network_magic` and returns
    /// a connected client with protocol channels ready for use.
    pub async fn connect<P: AsRef<Path>>(
        socket_path: P,
        network_magic: u64,
    ) -> Result<Self, NetworkError> {
        let stream = tokio::net::UnixStream::connect(socket_path.as_ref())
            .await
            .map_err(|e| NetworkError::Bearer(crate::error::BearerError::Io(e)))?;

        let bearer = UnixBearer::new(stream);
        let mut mux = Mux::new(bearer, true); // we are initiator

        // Subscribe protocol channels
        let mut handshake_channel = mux.subscribe(0, Direction::InitiatorDir, 65536);
        let state_query_channel = mux.subscribe(7, Direction::InitiatorDir, 16_777_216);
        let tx_submission_channel = mux.subscribe(6, Direction::InitiatorDir, 65536);
        let tx_monitor_channel = mux.subscribe(9, Direction::InitiatorDir, 65536);
        let chain_sync_channel = mux.subscribe(5, Direction::InitiatorDir, 4_194_304);

        // Start the mux
        let mux_handle = tokio::spawn(async move { mux.run().await });

        // Run N2C handshake with the provided network magic
        let our_data = crate::handshake::n2c::N2CVersionData::new(network_magic);
        let handshake_result =
            crate::handshake::run_n2c_handshake_client(&mut handshake_channel, &our_data)
                .await
                .map_err(NetworkError::Handshake)?;

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

    // ── Raw channel I/O ──────────────────────────────────────────────────

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

    // ── LocalStateQuery: acquire / release / done ────────────────────────

    /// Acquire the ledger state at the volatile tip for subsequent queries.
    ///
    /// Sends `MsgAcquire` (tag 8) with VolatileTip target on the
    /// LocalStateQuery channel and waits for `MsgAcquired` (tag 1).
    pub async fn acquire(&mut self) -> Result<(), NetworkError> {
        // MsgAcquire VolatileTip = [8] (just the tag, no target payload)
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(1).map_err(cbor_err)?;
        enc.u32(8).map_err(cbor_err)?; // MsgAcquire VolatileTip
        self.send_query(buf).await?;

        let resp = self.recv_query().await?;
        let mut dec = minicbor::Decoder::new(&resp);
        let _ = dec.array();
        let tag = dec
            .u32()
            .map_err(|e| protocol_err(format!("bad acquire response: {e}")))?;
        if tag != 1 {
            // tag 1 = MsgAcquired, tag 2 = MsgFailure
            return Err(protocol_err(format!("expected MsgAcquired(1), got {tag}")));
        }
        debug!("State acquired");
        Ok(())
    }

    /// Release the acquired ledger state.
    ///
    /// Sends `MsgRelease` (tag 5) on the LocalStateQuery channel.
    pub async fn release(&mut self) -> Result<(), NetworkError> {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(1).map_err(cbor_err)?;
        enc.u32(5).map_err(cbor_err)?; // MsgRelease
        self.send_query(buf).await
    }

    /// Send `MsgDone` (tag 7) to end the LocalStateQuery protocol.
    pub async fn done(&mut self) -> Result<(), NetworkError> {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(1).map_err(cbor_err)?;
        enc.u32(7).map_err(cbor_err)?; // MsgDone
        self.send_query(buf).await
    }

    // ── LocalStateQuery: high-level query methods ────────────────────────

    /// Query the chain tip (`GetLedgerTip` -- Shelley query tag 0).
    ///
    /// Returns a [`TipResult`] with slot, hash, and block_no populated.
    /// The `epoch` and `era` fields default to 0; callers fill them via
    /// [`query_epoch`](Self::query_epoch) / [`query_era`](Self::query_era).
    pub async fn query_tip(&mut self) -> Result<TipResult, NetworkError> {
        let result = self.send_shelley_query(0).await?;
        parse_tip_result(&result)
    }

    /// Query the current epoch number (`GetEpochNo` -- Shelley query tag 1).
    pub async fn query_epoch(&mut self) -> Result<u64, NetworkError> {
        let result = self.send_shelley_query(1).await?;
        parse_epoch_result(&result)
    }

    /// Query the current era (`GetCurrentEra` -- QueryHardFork sub-tag 1).
    ///
    /// Wire format: `MsgQuery [3, [2, [1]]]` — QueryHardFork, GetCurrentEra.
    /// Response is unwrapped (no HFC success envelope).
    pub async fn query_era(&mut self) -> Result<u32, NetworkError> {
        let payload = encode_hard_fork_query(1)?; // sub-tag 1 = GetCurrentEra
        self.send_query(payload).await?;
        let resp = self.recv_query().await?;

        let mut dec = minicbor::Decoder::new(&resp);
        let _ = dec.array();
        let tag = dec
            .u32()
            .map_err(|e| protocol_err(format!("bad MsgResult tag: {e}")))?;
        if tag != 4 {
            return Err(protocol_err(format!("expected MsgResult(4), got {tag}")));
        }
        let era = dec
            .u32()
            .map_err(|e| protocol_err(format!("bad era result: {e}")))?;
        Ok(era)
    }

    /// Query the system start time (`GetEraStart` -- QueryAnytime sub-tag 0).
    ///
    /// Wire format: `MsgQuery [3, [1, [0]]]` — QueryAnytime, GetEraStart.
    /// Returns an ISO-8601 UTC string, e.g. `"2022-10-25T00:00:00Z"`.
    pub async fn query_system_start(&mut self) -> Result<String, NetworkError> {
        let payload = encode_anytime_query(0)?; // sub-tag 0 = GetEraStart
        self.send_query(payload).await?;
        let resp = self.recv_query().await?;

        let mut dec = minicbor::Decoder::new(&resp);
        let _ = dec.array();
        let tag = dec
            .u32()
            .map_err(|e| protocol_err(format!("bad MsgResult tag: {e}")))?;
        if tag != 4 {
            return Err(protocol_err(format!("expected MsgResult(4), got {tag}")));
        }
        // SystemStart encoded as UTCTime: [year, day_of_year, pico_of_day]
        let _ = dec.array();
        let year = dec
            .u32()
            .map_err(|e| protocol_err(format!("bad year: {e}")))?;
        let day_of_year = dec
            .u32()
            .map_err(|e| protocol_err(format!("bad day: {e}")))?;
        let pico_of_day = dec
            .u64()
            .map_err(|e| protocol_err(format!("bad pico: {e}")))?;

        Ok(utctime_to_iso8601(year, day_of_year, pico_of_day))
    }

    /// Query the chain block number (`GetChainBlockNo` -- Shelley query tag 2).
    ///
    /// Note: Despite being conceptually top-level, this is encoded as a
    /// Shelley BlockQuery tag 2 for Dugite compatibility.
    pub async fn query_block_no(&mut self) -> Result<u64, NetworkError> {
        let result = self.send_shelley_query(2).await?;
        parse_u64_result(&result)
    }

    /// Query protocol parameters (`GetCurrentPParams` -- Shelley query tag 3).
    ///
    /// Returns a JSON string matching `cardano-cli 10.x` output format.
    pub async fn query_protocol_params(&mut self) -> Result<String, NetworkError> {
        let result = self.send_shelley_query(3).await?;
        parse_protocol_params_cbor(&result)
    }

    /// Query stake distribution (`GetStakeDistribution` -- Shelley query tag 5).
    ///
    /// Returns raw MsgResult CBOR payload for the CLI to parse.
    pub async fn query_stake_distribution(&mut self) -> Result<Vec<u8>, NetworkError> {
        self.send_shelley_query(5).await
    }

    /// Query UTxOs by specific transaction inputs (`GetUTxOByTxIn` -- Shelley query tag 15).
    ///
    /// Each input is a `(tx_hash_bytes, output_index)` pair.
    /// Returns raw MsgResult CBOR payload.
    pub async fn query_utxo_by_txin(
        &mut self,
        inputs: &[(Vec<u8>, u32)],
    ) -> Result<Vec<u8>, NetworkError> {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).map_err(cbor_err)?;
        enc.u32(3).map_err(cbor_err)?;
        enc.array(2).map_err(cbor_err)?;
        enc.u32(0).map_err(cbor_err)?;
        enc.array(2).map_err(cbor_err)?;
        enc.u32(15).map_err(cbor_err)?;
        // tag(258) Set<TxIn> where TxIn = [tx_hash, output_index]
        enc.tag(minicbor::data::Tag::new(258)).map_err(cbor_err)?;
        enc.array(inputs.len() as u64).map_err(cbor_err)?;
        for (tx_hash, index) in inputs {
            enc.array(2).map_err(cbor_err)?;
            enc.bytes(tx_hash).map_err(cbor_err)?;
            enc.u32(*index).map_err(cbor_err)?;
        }

        self.send_query(buf).await?;
        self.recv_query().await
    }

    /// Query UTxOs at a specific address (`GetUTxOByAddress` -- Shelley query tag 6).
    ///
    /// `addr_bytes` is the raw address bytes (e.g. decoded from bech32).
    /// Returns raw MsgResult CBOR payload.
    pub async fn query_utxo_by_address(
        &mut self,
        addr_bytes: &[u8],
    ) -> Result<Vec<u8>, NetworkError> {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).map_err(cbor_err)?;
        enc.u32(3).map_err(cbor_err)?;
        enc.array(2).map_err(cbor_err)?;
        enc.u32(0).map_err(cbor_err)?;
        enc.array(2).map_err(cbor_err)?;
        enc.u32(6).map_err(cbor_err)?;
        enc.bytes(addr_bytes).map_err(cbor_err)?;

        self.send_query(buf).await?;
        self.recv_query().await
    }

    /// Query the entire UTxO set (`GetUTxOWhole` -- Shelley query tag 7).
    ///
    /// Returns raw MsgResult CBOR payload. Warning: response can be very large
    /// on mainnet (~10M UTxO entries).
    pub async fn query_utxo_whole(&mut self) -> Result<Vec<u8>, NetworkError> {
        self.send_shelley_query(7).await
    }

    /// Query stake pool parameters (`GetStakePoolParams` -- Shelley query tag 17).
    ///
    /// Returns raw MsgResult CBOR payload.
    pub async fn query_pool_params(&mut self) -> Result<Vec<u8>, NetworkError> {
        self.send_shelley_query(17).await
    }

    /// Query DRep state (`GetDRepState` -- Shelley query tag 25).
    ///
    /// Returns raw MsgResult CBOR payload.
    pub async fn query_drep_state(&mut self) -> Result<Vec<u8>, NetworkError> {
        self.send_shelley_query(25).await
    }

    /// Query constitution (`GetConstitution` -- Shelley query tag 23).
    ///
    /// Returns raw MsgResult CBOR payload.
    pub async fn query_constitution(&mut self) -> Result<Vec<u8>, NetworkError> {
        self.send_shelley_query(23).await
    }

    /// Query committee members state (`GetCommitteeMembersState` -- Shelley query tag 27).
    ///
    /// Returns raw MsgResult CBOR payload.
    pub async fn query_committee_state(&mut self) -> Result<Vec<u8>, NetworkError> {
        self.send_shelley_query(27).await
    }

    /// Query governance state (`GetGovState` -- Shelley query tag 24).
    ///
    /// Returns raw MsgResult CBOR payload.
    pub async fn query_gov_state(&mut self) -> Result<Vec<u8>, NetworkError> {
        self.send_shelley_query(24).await
    }

    /// Query stake address info (`GetFilteredDelegationsAndRewardAccounts` -- Shelley query tag 10).
    ///
    /// `credential_hash` is the 28-byte staking credential. If empty, queries all.
    /// Returns raw MsgResult CBOR payload.
    pub async fn query_stake_address_info(
        &mut self,
        credential_hash: &[u8],
    ) -> Result<Vec<u8>, NetworkError> {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).map_err(cbor_err)?;
        enc.u32(3).map_err(cbor_err)?;
        enc.array(2).map_err(cbor_err)?;
        enc.u32(0).map_err(cbor_err)?;
        enc.array(2).map_err(cbor_err)?;
        enc.u32(10).map_err(cbor_err)?;

        if credential_hash.is_empty() {
            enc.tag(minicbor::data::Tag::new(258)).map_err(cbor_err)?;
            enc.array(0).map_err(cbor_err)?;
        } else {
            enc.tag(minicbor::data::Tag::new(258)).map_err(cbor_err)?;
            enc.array(1).map_err(cbor_err)?;
            enc.array(2).map_err(cbor_err)?;
            enc.u32(0).map_err(cbor_err)?;
            enc.bytes(credential_hash).map_err(cbor_err)?;
        }

        self.send_query(buf).await?;
        self.recv_query().await
    }

    /// Query stake snapshots (`GetStakeSnapshots` -- Shelley query tag 20).
    ///
    /// Returns raw MsgResult CBOR payload.
    pub async fn query_stake_snapshot(&mut self) -> Result<Vec<u8>, NetworkError> {
        self.send_shelley_query(20).await
    }

    /// Query account state (`GetAccountState` -- Shelley query tag 29).
    ///
    /// Returns raw MsgResult CBOR payload (treasury and reserves).
    pub async fn query_account_state(&mut self) -> Result<Vec<u8>, NetworkError> {
        self.send_shelley_query(29).await
    }

    /// Query ledger state (`DebugLedgerState` -- Shelley query tag 4).
    ///
    /// Returns raw MsgResult CBOR payload.
    pub async fn query_ledger_state(&mut self) -> Result<Vec<u8>, NetworkError> {
        self.send_shelley_query(4).await
    }

    /// Query protocol state (`DebugProtocolState` -- Shelley query tag 8).
    ///
    /// Returns raw MsgResult CBOR payload.
    pub async fn query_protocol_state(&mut self) -> Result<Vec<u8>, NetworkError> {
        self.send_shelley_query(8).await
    }

    /// Query consensus chain-dependent state (`DebugChainDepState` -- Shelley query tag 13).
    ///
    /// Returns the PraosState CBOR (nonces, opcert counters, last slot).
    /// This is much smaller than `query_protocol_state()` (tag 8) which returns the
    /// full debug ledger state.
    pub async fn query_chain_dep_state(&mut self) -> Result<Vec<u8>, NetworkError> {
        self.send_shelley_query(13).await
    }

    /// Query ratification state (`GetRatifyState` -- Shelley query tag 32).
    ///
    /// Returns raw MsgResult CBOR payload.
    pub async fn query_ratify_state(&mut self) -> Result<Vec<u8>, NetworkError> {
        self.send_shelley_query(32).await
    }

    /// Query era history (`GetInterpreter` / `GetEraHistory`) via HardFork combinator.
    ///
    /// Wire format: `MsgQuery [3, [2, [0]]]` where 2=QueryHardFork, 0=GetInterpreter.
    /// Returns raw MsgResult payload bytes.
    pub async fn query_era_history(&mut self) -> Result<Vec<u8>, NetworkError> {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).map_err(cbor_err)?;
        enc.u32(3).map_err(cbor_err)?;
        enc.array(2).map_err(cbor_err)?;
        enc.u32(2).map_err(cbor_err)?;
        enc.array(1).map_err(cbor_err)?;
        enc.u32(0).map_err(cbor_err)?;

        self.send_query(buf).await?;
        self.recv_query().await
    }

    /// Evaluate a transaction and return script execution costs (`EvaluateTx` -- Shelley query tag 36).
    ///
    /// This queries the node to evaluate the Plutus scripts in the given transaction
    /// and return the actual execution units consumed by each script.
    ///
    /// The `tx_body_cbor` should be the raw CBOR-encoded transaction body.
    /// Returns a CBOR-encoded evaluation result that can be parsed into script costs.
    pub async fn evaluate_tx(&mut self, tx_body_cbor: &[u8]) -> Result<Vec<u8>, NetworkError> {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).map_err(cbor_err)?;
        enc.u32(3).map_err(cbor_err)?;
        enc.array(2).map_err(cbor_err)?;
        enc.u32(0).map_err(cbor_err)?;
        enc.array(2).map_err(cbor_err)?;
        enc.u32(36).map_err(cbor_err)?;
        enc.bytes(tx_body_cbor).map_err(cbor_err)?;

        self.send_query(buf).await?;
        self.recv_query().await
    }

    // ── LocalTxMonitor ───────────────────────────────────────────────────

    /// Acquire a mempool snapshot. Returns the slot number.
    pub async fn monitor_acquire(&mut self) -> Result<u64, NetworkError> {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(1).map_err(cbor_err)?;
        enc.u32(1).map_err(cbor_err)?;

        self.tx_monitor_channel
            .send(buf)
            .await
            .map_err(NetworkError::Mux)?;

        let resp = self
            .tx_monitor_channel
            .recv()
            .await
            .map_err(NetworkError::Mux)?;
        let mut dec = minicbor::Decoder::new(&resp);
        let _ = dec.array();
        let tag = dec
            .u32()
            .map_err(|e| protocol_err(format!("bad monitor acquire: {e}")))?;
        if tag != 2 {
            return Err(protocol_err(format!("expected MsgAcquired(2), got {tag}")));
        }
        let slot = dec
            .u64()
            .map_err(|e| protocol_err(format!("bad mempool slot: {e}")))?;
        Ok(slot)
    }

    /// Check if a transaction is in the mempool.
    pub async fn monitor_has_tx(&mut self, tx_hash: &[u8; 32]) -> Result<bool, NetworkError> {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).map_err(cbor_err)?;
        enc.u32(7).map_err(cbor_err)?;
        enc.bytes(tx_hash).map_err(cbor_err)?;

        self.tx_monitor_channel
            .send(buf)
            .await
            .map_err(NetworkError::Mux)?;

        let resp = self
            .tx_monitor_channel
            .recv()
            .await
            .map_err(NetworkError::Mux)?;
        let mut dec = minicbor::Decoder::new(&resp);
        let _ = dec.array();
        let tag = dec
            .u32()
            .map_err(|e| protocol_err(format!("bad monitor has_tx: {e}")))?;
        if tag != 8 {
            return Err(protocol_err(format!(
                "expected MsgReplyHasTx(8), got {tag}"
            )));
        }
        let has_tx = dec.bool().unwrap_or(false);
        Ok(has_tx)
    }

    /// Get mempool sizes (capacity, size, num_txs).
    pub async fn monitor_get_sizes(&mut self) -> Result<(u32, u32, u32), NetworkError> {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(1).map_err(cbor_err)?;
        enc.u32(9).map_err(cbor_err)?;

        self.tx_monitor_channel
            .send(buf)
            .await
            .map_err(NetworkError::Mux)?;

        let resp = self
            .tx_monitor_channel
            .recv()
            .await
            .map_err(NetworkError::Mux)?;
        let mut dec = minicbor::Decoder::new(&resp);
        let _ = dec.array();
        let tag = dec
            .u32()
            .map_err(|e| protocol_err(format!("bad monitor get_sizes: {e}")))?;
        if tag != 10 {
            return Err(protocol_err(format!(
                "expected MsgReplyGetSizes(10), got {tag}"
            )));
        }
        let _ = dec.array();
        let capacity = dec
            .u32()
            .map_err(|e| protocol_err(format!("bad capacity: {e}")))?;
        let size = dec
            .u32()
            .map_err(|e| protocol_err(format!("bad size: {e}")))?;
        let num_txs = dec
            .u32()
            .map_err(|e| protocol_err(format!("bad num_txs: {e}")))?;
        Ok((capacity, size, num_txs))
    }

    /// Get the next transaction from the mempool snapshot.
    ///
    /// Returns `Some((tx_hash, tx_cbor))` for the next transaction, or `None` if
    /// there are no more transactions in the mempool.
    ///
    /// The server sends `[6, tx_cbor]` for a present tx, or `[6]` when exhausted.
    /// The tx hash is computed from the returned CBOR (Blake2b-256).
    pub async fn monitor_next_tx(&mut self) -> Result<Option<(Vec<u8>, Vec<u8>)>, NetworkError> {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(1).map_err(cbor_err)?;
        enc.u32(5).map_err(cbor_err)?; // MsgNextTx (tag 5)

        self.tx_monitor_channel
            .send(buf)
            .await
            .map_err(NetworkError::Mux)?;

        let resp = self
            .tx_monitor_channel
            .recv()
            .await
            .map_err(NetworkError::Mux)?;
        let mut dec = minicbor::Decoder::new(&resp);
        let arr_len = dec.array();
        let tag = dec
            .u32()
            .map_err(|e| protocol_err(format!("bad monitor next_tx: {e}")))?;

        match tag {
            6 => {
                // MsgReplyNextTx — array(1) means no tx, array(2) means tx present
                let has_tx = matches!(arr_len, Ok(Some(n)) if n >= 2);
                if has_tx {
                    let tx_cbor = dec.bytes().unwrap_or(&[]).to_vec();
                    // Compute tx hash from CBOR (Blake2b-256)
                    let tx_hash = dugite_primitives::hash::blake2b_256(&tx_cbor);
                    Ok(Some((tx_hash.as_ref().to_vec(), tx_cbor)))
                } else {
                    Ok(None)
                }
            }
            other => Err(protocol_err(format!(
                "unexpected next_tx response tag: {other}"
            ))),
        }
    }

    /// Send `MsgDone` (tag 0) for the LocalTxMonitor protocol — ends the session.
    pub async fn monitor_done(&mut self) -> Result<(), NetworkError> {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(1).map_err(cbor_err)?;
        enc.u32(0).map_err(cbor_err)?; // TAG_DONE

        self.tx_monitor_channel
            .send(buf)
            .await
            .map_err(NetworkError::Mux)
    }

    /// Send `MsgRelease` (tag 3) for the LocalTxMonitor protocol — releases snapshot, returns to StIdle.
    pub async fn monitor_release(&mut self) -> Result<(), NetworkError> {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(1).map_err(cbor_err)?;
        enc.u32(3).map_err(cbor_err)?; // TAG_RELEASE

        self.tx_monitor_channel
            .send(buf)
            .await
            .map_err(NetworkError::Mux)
    }

    // ── LocalTxSubmission ────────────────────────────────────────────────

    /// Submit a signed transaction via the LocalTxSubmission protocol.
    ///
    /// `tx_cbor` is the raw CBOR bytes of the signed transaction.
    /// Returns `Ok(())` if accepted, or an error with the rejection reason.
    pub async fn submit_tx(&mut self, tx_cbor: &[u8]) -> Result<(), NetworkError> {
        // MsgSubmitTx: [0, [era_id, tag(24, tx_bytes)]]
        // era_id 6 = Conway; tx wrapped in CBOR tag 24 (wrapCBORinCBOR)
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).map_err(cbor_err)?;
        enc.u32(0).map_err(cbor_err)?;
        enc.array(2).map_err(cbor_err)?;
        enc.u32(6).map_err(cbor_err)?;
        enc.tag(minicbor::data::Tag::new(24)).map_err(cbor_err)?;
        enc.bytes(tx_cbor).map_err(cbor_err)?;

        self.send_tx_submission(buf).await?;

        let resp = self.recv_tx_submission().await?;
        let mut dec = minicbor::Decoder::new(&resp);
        let _ = dec.array();
        let msg_tag = dec
            .u32()
            .map_err(|e| protocol_err(format!("bad tx response tag: {e}")))?;

        match msg_tag {
            1 => Ok(()),
            2 => {
                let reason = decode_reject_reason(&mut dec)
                    .unwrap_or_else(|| "unknown rejection reason".to_string());
                Err(protocol_err(format!("Transaction rejected: {reason}")))
            }
            other => Err(protocol_err(format!(
                "unexpected tx submission response tag: {other}"
            ))),
        }
    }

    // ── Internal helpers ─────────────────────────────────────────────────

    /// Send a Shelley-era BlockQuery and return the raw MsgResult payload bytes.
    ///
    /// Wire format: `MsgQuery [3, [0, [shelley_tag]]]`
    async fn send_shelley_query(&mut self, shelley_tag: u32) -> Result<Vec<u8>, NetworkError> {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).map_err(cbor_err)?;
        enc.u32(3).map_err(cbor_err)?;
        enc.array(2).map_err(cbor_err)?;
        enc.u32(0).map_err(cbor_err)?;
        enc.array(1).map_err(cbor_err)?;
        enc.u32(shelley_tag).map_err(cbor_err)?;

        self.send_query(buf).await?;
        self.recv_query().await
    }
}

// ── Free-standing helpers ────────────────────────────────────────────────

/// Convert a minicbor encode error into a `NetworkError`.
fn cbor_err<T: std::fmt::Display>(e: T) -> NetworkError {
    protocol_err(format!("CBOR encode error: {e}"))
}

/// Build a `NetworkError::Protocol` from a string message.
fn protocol_err(reason: String) -> NetworkError {
    NetworkError::Protocol(crate::error::ProtocolError::CborDecode {
        protocol: "LocalStateQuery",
        reason,
    })
}

/// Encode a QueryAnytime query: `MsgQuery [3, [1, [sub_tag]]]`.
///
/// Sub-tags: 0=GetEraStart, 2=GetCurrentEra.
fn encode_anytime_query(sub_tag: u32) -> Result<Vec<u8>, NetworkError> {
    let mut buf = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut buf);
    enc.array(2).map_err(cbor_err)?;
    enc.u32(3).map_err(cbor_err)?; // MsgQuery
    enc.array(2).map_err(cbor_err)?;
    enc.u32(1).map_err(cbor_err)?; // QueryAnytime
    enc.array(1).map_err(cbor_err)?;
    enc.u32(sub_tag).map_err(cbor_err)?;
    Ok(buf)
}

/// Encode a QueryHardFork query: `MsgQuery [3, [2, [sub_tag]]]`.
///
/// Sub-tags: 0=GetInterpreter (EraHistory), 1=GetCurrentEra.
fn encode_hard_fork_query(sub_tag: u32) -> Result<Vec<u8>, NetworkError> {
    let mut buf = Vec::new();
    let mut enc = minicbor::Encoder::new(&mut buf);
    enc.array(2).map_err(cbor_err)?;
    enc.u32(3).map_err(cbor_err)?; // MsgQuery
    enc.array(2).map_err(cbor_err)?;
    enc.u32(2).map_err(cbor_err)?; // QueryHardFork
    enc.array(1).map_err(cbor_err)?;
    enc.u32(sub_tag).map_err(cbor_err)?;
    Ok(buf)
}

/// Strip the `MsgResult [4, ...]` envelope from a response payload.
fn strip_msg_result(decoder: &mut minicbor::Decoder) -> Result<(), NetworkError> {
    let _ = decoder.array();
    let tag = decoder.u32().unwrap_or(999);
    if tag != 4 {
        return Err(protocol_err(format!("expected MsgResult(4), got {tag}")));
    }
    Ok(())
}

/// Strip the HardFork Combinator EitherMismatch success wrapper `[result]`.
///
/// BlockQuery QueryIfCurrent results are wrapped in an array(1): `[result]`.
/// The HFC uses array length as discriminator: 1 = success, 2 = era mismatch.
/// After stripping, the decoder is positioned at the actual result.
/// If the response is unwrapped (non-BlockQuery), the position is reset
/// so the caller can parse directly.
fn strip_hfc_wrapper(decoder: &mut minicbor::Decoder) -> Result<(), NetworkError> {
    let pos = decoder.position();
    match decoder.array() {
        Ok(Some(1)) => {
            // HFC success: array(1) containing the result — decoder is now at the result
            Ok(())
        }
        Ok(Some(2)) => {
            // HFC era mismatch: array(2) [query_era, ledger_era]
            // This shouldn't happen in normal operation; reset and let caller handle.
            decoder.set_position(pos);
            Ok(())
        }
        _ => {
            decoder.set_position(pos);
            Ok(())
        }
    }
}

/// Parse a `GetLedgerTip` MsgResult into a [`TipResult`].
fn parse_tip_result(payload: &[u8]) -> Result<TipResult, NetworkError> {
    let mut dec = minicbor::Decoder::new(payload);
    strip_msg_result(&mut dec)?;
    strip_hfc_wrapper(&mut dec)?;

    // Wire format: [[slot, hash], block_no]
    let _ = dec.array();
    let _ = dec.array();
    let slot = dec
        .u64()
        .map_err(|e| protocol_err(format!("bad slot: {e}")))?;
    let hash = dec
        .bytes()
        .map_err(|e| protocol_err(format!("bad hash: {e}")))?
        .to_vec();
    let block_no = dec
        .u64()
        .map_err(|e| protocol_err(format!("bad block_no: {e}")))?;

    Ok(TipResult {
        slot,
        hash,
        block_no,
        epoch: 0,
        era: 0,
    })
}

/// Parse a `GetEpochNo` MsgResult into a `u64`.
fn parse_epoch_result(payload: &[u8]) -> Result<u64, NetworkError> {
    let mut dec = minicbor::Decoder::new(payload);
    strip_msg_result(&mut dec)?;
    strip_hfc_wrapper(&mut dec)?;
    dec.u64()
        .map_err(|e| protocol_err(format!("bad epoch: {e}")))
}

/// Parse a MsgResult containing a single `u64` (with HFC wrapper).
fn parse_u64_result(payload: &[u8]) -> Result<u64, NetworkError> {
    let mut dec = minicbor::Decoder::new(payload);
    strip_msg_result(&mut dec)?;
    strip_hfc_wrapper(&mut dec)?;
    dec.u64().map_err(|e| protocol_err(format!("bad u64: {e}")))
}

/// Convert a UTCTime triple `(year, day_of_year, pico_of_day)` to ISO-8601 UTC.
fn utctime_to_iso8601(year: u32, day_of_year: u32, pico_of_day: u64) -> String {
    let is_leap = (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400);
    let days_in_months: [u32; 12] = if is_leap {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut remaining = day_of_year;
    let mut month = 1u32;
    for &days in &days_in_months {
        if remaining <= days {
            break;
        }
        remaining -= days;
        month += 1;
    }
    let day = remaining;
    let secs_of_day = (pico_of_day / 1_000_000_000_000) as u32;
    let hours = secs_of_day / 3600;
    let minutes = (secs_of_day % 3600) / 60;
    let seconds = secs_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}Z")
}

/// Format a tagged rational `(num, den)` as a cardano-cli compatible decimal.
fn format_rational_decimal(num: u64, den: u64) -> String {
    if den == 0 {
        return "0.0".to_string();
    }
    let int_part = num / den;
    let remainder = num % den;
    if remainder == 0 {
        return format!("{int_part}.0");
    }
    let mut frac_digits = String::new();
    let mut rem = remainder;
    for _ in 0..12 {
        rem *= 10;
        frac_digits.push(char::from_digit((rem / den) as u32, 10).unwrap_or('0'));
        rem %= den;
        if rem == 0 {
            break;
        }
    }
    let trimmed = frac_digits.trim_end_matches('0');
    let trimmed = if trimmed.is_empty() { "0" } else { trimmed };
    format!("{int_part}.{trimmed}")
}

/// Parse protocol params from MsgResult CBOR into a JSON string matching
/// `cardano-cli 10.x` output format.
#[allow(clippy::too_many_lines)]
fn parse_protocol_params_cbor(payload: &[u8]) -> Result<String, NetworkError> {
    let mut decoder = minicbor::Decoder::new(payload);
    strip_msg_result(&mut decoder)?;
    strip_hfc_wrapper(&mut decoder)?;

    match decoder.datatype() {
        Ok(minicbor::data::Type::Array) => {
            let arr_len = decoder
                .array()
                .map_err(|e| protocol_err(format!("bad array: {e}")))?
                .unwrap_or(0);

            let field_names: &[&str] = &[
                "txFeePerByte",
                "txFeeFixed",
                "maxBlockBodySize",
                "maxTxSize",
                "maxBlockHeaderSize",
                "stakeAddressDeposit",
                "stakePoolDeposit",
                "poolRetireMaxEpoch",
                "stakePoolTargetNum",
            ];

            let rational_fields: &[&str] =
                &["poolPledgeInfluence", "monetaryExpansion", "treasuryCut"];

            let mut entries = Vec::new();

            for i in 0..arr_len {
                match i {
                    0..=8 => {
                        let val = decoder.u64().unwrap_or(0);
                        if let Some(name) = field_names.get(i as usize) {
                            entries.push(format!("  \"{name}\": {val}"));
                        }
                    }
                    9..=11 => {
                        let _ = decoder.tag();
                        let _ = decoder.array();
                        let num = decoder.u64().unwrap_or(0);
                        let den = decoder.u64().unwrap_or(1);
                        if let Some(name) = rational_fields.get((i - 9) as usize) {
                            let decimal = format_rational_decimal(num, den);
                            entries.push(format!("  \"{name}\": {decimal}"));
                        }
                    }
                    12 => {
                        let _ = decoder.array();
                        let major = decoder.u64().unwrap_or(0);
                        let minor = decoder.u64().unwrap_or(0);
                        entries.push(format!(
                            "  \"protocolVersion\": {{\n    \"major\": {major},\n    \"minor\": {minor}\n  }}"
                        ));
                    }
                    13 => {
                        let val = decoder.u64().unwrap_or(0);
                        entries.push(format!("  \"minPoolCost\": {val}"));
                    }
                    14 => {
                        let val = decoder.u64().unwrap_or(0);
                        entries.push(format!("  \"utxoCostPerByte\": {val}"));
                    }
                    15 => {
                        let mut cm_entries = Vec::new();
                        if let Ok(Some(map_len)) = decoder.map() {
                            for _ in 0..map_len {
                                let lang = decoder.u32().unwrap_or(0);
                                let lang_name = match lang {
                                    0 => "PlutusV1",
                                    1 => "PlutusV2",
                                    2 => "PlutusV3",
                                    _ => "Unknown",
                                };
                                let mut costs = Vec::new();
                                if let Ok(Some(cost_len)) = decoder.array() {
                                    for _ in 0..cost_len {
                                        costs.push(decoder.i64().unwrap_or(0));
                                    }
                                }
                                let costs_str: Vec<String> =
                                    costs.iter().map(|c| c.to_string()).collect();
                                cm_entries.push(format!(
                                    "    \"{lang_name}\": [{}]",
                                    costs_str.join(", ")
                                ));
                            }
                        } else {
                            let _ = decoder.skip();
                        }
                        entries.push(format!(
                            "  \"costModels\": {{\n{}\n  }}",
                            cm_entries.join(",\n")
                        ));
                    }
                    16 => {
                        let _ = decoder.array();
                        let _ = decoder.tag();
                        let _ = decoder.array();
                        let mem_num = decoder.u64().unwrap_or(0);
                        let mem_den = decoder.u64().unwrap_or(1);
                        let _ = decoder.tag();
                        let _ = decoder.array();
                        let step_num = decoder.u64().unwrap_or(0);
                        let step_den = decoder.u64().unwrap_or(1);
                        let mem_decimal = format_rational_decimal(mem_num, mem_den);
                        let step_decimal = format_rational_decimal(step_num, step_den);
                        entries.push(format!(
                            "  \"executionUnitPrices\": {{\n    \"priceMemory\": {mem_decimal},\n    \"priceSteps\": {step_decimal}\n  }}"
                        ));
                    }
                    17 => {
                        let _ = decoder.array();
                        let mem = decoder.u64().unwrap_or(0);
                        let steps = decoder.u64().unwrap_or(0);
                        entries.push(format!(
                            "  \"maxTxExecutionUnits\": {{\n    \"memory\": {mem},\n    \"steps\": {steps}\n  }}"
                        ));
                    }
                    18 => {
                        let _ = decoder.array();
                        let mem = decoder.u64().unwrap_or(0);
                        let steps = decoder.u64().unwrap_or(0);
                        entries.push(format!(
                            "  \"maxBlockExecutionUnits\": {{\n    \"memory\": {mem},\n    \"steps\": {steps}\n  }}"
                        ));
                    }
                    19 => {
                        let val = decoder.u64().unwrap_or(0);
                        entries.push(format!("  \"maxValueSize\": {val}"));
                    }
                    20 => {
                        let val = decoder.u64().unwrap_or(0);
                        entries.push(format!("  \"collateralPercentage\": {val}"));
                    }
                    21 => {
                        let val = decoder.u64().unwrap_or(0);
                        entries.push(format!("  \"maxCollateralInputs\": {val}"));
                    }
                    22 => {
                        let pvt_names = [
                            "pvtMotionNoConfidence",
                            "pvtCommitteeNormal",
                            "pvtCommitteeNoConfidence",
                            "pvtHardForkInitiation",
                            "pvtPPSecurityGroup",
                        ];
                        let _ = decoder.array();
                        let mut pvt_entries = Vec::new();
                        for name in &pvt_names {
                            let _ = decoder.tag();
                            let _ = decoder.array();
                            let num = decoder.u64().unwrap_or(0);
                            let den = decoder.u64().unwrap_or(1);
                            let decimal = format_rational_decimal(num, den);
                            pvt_entries.push(format!("    \"{name}\": {decimal}"));
                        }
                        entries.push(format!(
                            "  \"poolVotingThresholds\": {{\n{}\n  }}",
                            pvt_entries.join(",\n")
                        ));
                    }
                    23 => {
                        let dvt_names = [
                            "dvtMotionNoConfidence",
                            "dvtCommitteeNormal",
                            "dvtCommitteeNoConfidence",
                            "dvtUpdateToConstitution",
                            "dvtHardForkInitiation",
                            "dvtPPNetworkGroup",
                            "dvtPPEconomicGroup",
                            "dvtPPTechnicalGroup",
                            "dvtPPGovGroup",
                            "dvtTreasuryWithdrawal",
                        ];
                        let _ = decoder.array();
                        let mut dvt_entries = Vec::new();
                        for name in &dvt_names {
                            let _ = decoder.tag();
                            let _ = decoder.array();
                            let num = decoder.u64().unwrap_or(0);
                            let den = decoder.u64().unwrap_or(1);
                            let decimal = format_rational_decimal(num, den);
                            dvt_entries.push(format!("    \"{name}\": {decimal}"));
                        }
                        entries.push(format!(
                            "  \"drepVotingThresholds\": {{\n{}\n  }}",
                            dvt_entries.join(",\n")
                        ));
                    }
                    24 => {
                        let val = decoder.u64().unwrap_or(0);
                        entries.push(format!("  \"committeeMinSize\": {val}"));
                    }
                    25 => {
                        let val = decoder.u64().unwrap_or(0);
                        entries.push(format!("  \"committeeMaxTermLength\": {val}"));
                    }
                    26 => {
                        let val = decoder.u64().unwrap_or(0);
                        entries.push(format!("  \"govActionLifetime\": {val}"));
                    }
                    27 => {
                        let val = decoder.u64().unwrap_or(0);
                        entries.push(format!("  \"govActionDeposit\": {val}"));
                    }
                    28 => {
                        let val = decoder.u64().unwrap_or(0);
                        entries.push(format!("  \"dRepDeposit\": {val}"));
                    }
                    29 => {
                        let val = decoder.u64().unwrap_or(0);
                        entries.push(format!("  \"dRepActivity\": {val}"));
                    }
                    30 => {
                        let _ = decoder.tag();
                        let _ = decoder.array();
                        let num = decoder.u64().unwrap_or(0);
                        let den = decoder.u64().unwrap_or(1);
                        let decimal = format_rational_decimal(num, den);
                        entries.push(format!("  \"minFeeRefScriptCostPerByte\": {decimal}"));
                    }
                    _ => {
                        let _ = decoder.skip();
                    }
                }
            }
            Ok(format!("{{\n{}\n}}", entries.join(",\n")))
        }
        Ok(minicbor::data::Type::Map) => {
            let map_len = decoder
                .map()
                .map_err(|e| protocol_err(format!("bad map: {e}")))?
                .unwrap_or(0);
            let mut entries = Vec::new();
            for _ in 0..map_len {
                let key = decoder.u32().unwrap_or(999);
                let _ = decoder.skip();
                entries.push(format!("  \"key_{key}\": \"skipped\""));
            }
            Ok(format!("{{\n{}\n}}", entries.join(",\n")))
        }
        Ok(minicbor::data::Type::String) => decoder
            .str()
            .map(|s| s.to_string())
            .map_err(|e| protocol_err(format!("bad string: {e}"))),
        Ok(dt) => Err(protocol_err(format!(
            "unexpected CBOR type for protocol params: {dt:?}"
        ))),
        Err(e) => Err(protocol_err(format!("decode error: {e}"))),
    }
}

// ── Transaction rejection decoding ───────────────────────────────────────

/// Decode the nested `MsgRejectTx` reason from CBOR wire format.
fn decode_reject_reason(decoder: &mut minicbor::Decoder<'_>) -> Option<String> {
    let _ = decoder.array().ok()?;
    let _ = decoder.array().ok()?;
    let _era_idx = decoder.u8().ok()?;
    let n_errors = decoder.array().ok()??;
    if n_errors == 0 {
        return Some("no errors in ApplyTxError".to_string());
    }

    let mut reasons = Vec::new();
    for _ in 0..n_errors {
        if let Some(reason) = decode_conway_pred_failure(decoder) {
            reasons.push(reason);
        } else {
            reasons.push("(undecodable error)".to_string());
            break;
        }
    }
    Some(reasons.join("; "))
}

/// Decode a single `ConwayLedgerPredFailure` from the CBOR stream.
fn decode_conway_pred_failure(decoder: &mut minicbor::Decoder<'_>) -> Option<String> {
    let _ = decoder.array().ok()?;
    let tag = decoder.u8().ok()?;
    match tag {
        1 => decode_conway_utxow_failure(decoder),
        7 => {
            let text = decoder.str().ok()?;
            Some(text.to_string())
        }
        other => {
            let _ = decoder.skip();
            Some(format!("ConwayLedgerPredFailure(tag={other})"))
        }
    }
}

/// Decode `ConwayUtxowPredFailure` which wraps `UtxoFailure`.
fn decode_conway_utxow_failure(decoder: &mut minicbor::Decoder<'_>) -> Option<String> {
    let _ = decoder.array().ok()?;
    let tag = decoder.u8().ok()?;
    match tag {
        // Tag 0: UtxoFailure — delegates to ConwayUtxoPredFailure
        0 => decode_conway_utxo_pred_failure(decoder),
        // Tag 1: InvalidWitnessesUTXOW — list of invalid vkey witnesses
        1 => {
            let _ = decoder.skip(); // skip the vkey list
            Some("InvalidWitnessesUTXOW: invalid witness signature(s)".to_string())
        }
        // Tag 2: MissingVKeyWitnessesUTXOW — tag(258) set of missing keyhashes
        2 => {
            let n = decode_tagged_set_count(decoder).unwrap_or(0);
            // Skip keyhash bytes
            for _ in 0..n {
                let _ = decoder.skip();
            }
            Some(format!(
                "MissingVKeyWitnessesUTXOW: {n} missing key witness(es)"
            ))
        }
        // Tag 3: MissingScriptWitnessesUTXOW
        3 => {
            let _ = decoder.skip();
            Some("MissingScriptWitnessesUTXOW: missing script witness(es)".to_string())
        }
        // Tag 13: PPViewHashesDontMatch — [supplied_maybe, expected_maybe]
        13 => {
            let _ = decoder.skip(); // supplied StrictMaybe
            let _ = decoder.skip(); // expected StrictMaybe
            Some("PPViewHashesDontMatch: script data hash mismatch".to_string())
        }
        other => {
            let _ = decoder.skip();
            Some(format!("ConwayUtxowPredFailure(tag={other})"))
        }
    }
}

/// Decode a `ConwayUtxoPredFailure` variant into a human-readable string.
fn decode_conway_utxo_pred_failure(decoder: &mut minicbor::Decoder<'_>) -> Option<String> {
    let _ = decoder.array().ok()?;
    let tag = decoder.u8().ok()?;
    match tag {
        // Tag 1: BadInputsUTxO — tag(258) set of TxIn
        1 => {
            let n = decode_tagged_set_count(decoder).unwrap_or(0);
            let mut inputs = Vec::new();
            for _ in 0..n {
                if let Some(txin) = decode_txin(decoder) {
                    inputs.push(txin);
                } else {
                    let _ = decoder.skip();
                }
            }
            Some(format!("BadInputsUTxO: [{}]", inputs.join(", ")))
        }
        // Tag 2: OutsideValidityIntervalUTxO — [ValidityInterval, current_slot]
        2 => {
            let _ = decoder.skip(); // skip ValidityInterval (complex nested structure)
            let current = decoder.u64().ok().unwrap_or(0);
            Some(format!("OutsideValidityInterval: current slot {current}"))
        }
        // Tag 3: MaxTxSizeUTxO — [supplied, expected]
        3 => {
            let supplied = decoder.u64().ok().unwrap_or(0);
            let expected = decoder.u64().ok().unwrap_or(0);
            Some(format!(
                "MaxTxSizeUTxO: tx size {supplied} > max {expected}"
            ))
        }
        // Tag 4: InputSetEmptyUTxO
        4 => Some("InputSetEmptyUTxO: no inputs".to_string()),
        // Tag 5: FeeTooSmallUTxO — [min_fee, actual_fee] (swapped)
        5 => {
            let expected = decoder.u64().ok().unwrap_or(0);
            let supplied = decoder.u64().ok().unwrap_or(0);
            Some(format!(
                "FeeTooSmallUTxO: minimum fee {expected} lovelace, actual fee {supplied} lovelace"
            ))
        }
        // Tag 6: ValueNotConservedUTxO — [consumed, produced]
        6 => {
            let consumed = decoder.u64().ok().unwrap_or(0);
            let produced = decoder.u64().ok().unwrap_or(0);
            Some(format!(
                "ValueNotConservedUTxO: consumed {consumed} lovelace, produced {produced} lovelace"
            ))
        }
        // Tag 9: OutputTooSmallUTxO — list of txouts
        9 => {
            let _ = decoder.skip(); // skip txout list
            Some("OutputTooSmallUTxO: output(s) below minimum value".to_string())
        }
        // Tag 11: OutputTooBigUTxO — list of [actual_size, max_size, txout]
        11 => {
            let _ = decoder.skip();
            Some("OutputTooBigUTxO: output value(s) exceed CBOR size limit".to_string())
        }
        // Tag 12: InsufficientCollateral — [balance_delta, required_collateral]
        12 => {
            let delta = decoder.i64().ok().unwrap_or(0);
            let required = decoder.u64().ok().unwrap_or(0);
            Some(format!(
                "InsufficientCollateral: balance {delta}, required {required} lovelace"
            ))
        }
        // Tag 15: CollateralContainsNonADA — value
        15 => {
            let _ = decoder.skip();
            Some("CollateralContainsNonADA: collateral contains non-ADA tokens".to_string())
        }
        // Tag 18: TooManyCollateralInputs — [max, actual] (swapped)
        18 => {
            let expected = decoder.u64().ok().unwrap_or(0);
            let supplied = decoder.u64().ok().unwrap_or(0);
            Some(format!(
                "TooManyCollateralInputs: max {expected}, provided {supplied}"
            ))
        }
        // Tag 19: NoCollateralInputs
        19 => Some("NoCollateralInputs".to_string()),
        // Tag 20: IncorrectTotalCollateralField — [delta, declared]
        20 => {
            let delta = decoder.i64().ok().unwrap_or(0);
            let declared = decoder.u64().ok().unwrap_or(0);
            Some(format!(
                "IncorrectTotalCollateralField: delta {delta}, declared {declared} lovelace"
            ))
        }
        // Tag 22: BabbageNonDisjointRefInputs — set of overlapping TxIn
        22 => {
            let _ = decoder.skip();
            Some("BabbageNonDisjointRefInputs: reference inputs overlap regular inputs".to_string())
        }
        other => Some(format!("ConwayUtxoPredFailure(tag={other})")),
    }
}

/// Decode a CBOR tag(258) set header and return the array length.
/// Consumes the tag and array header, leaving the decoder positioned at the first element.
fn decode_tagged_set_count(decoder: &mut minicbor::Decoder<'_>) -> Option<u64> {
    // Try to consume tag 258; if not present, just read the array directly
    let pos = decoder.position();
    if let Ok(tag) = decoder.tag() {
        if tag.as_u64() != 258 {
            decoder.set_position(pos);
        }
    } else {
        decoder.set_position(pos);
    }
    decoder.array().ok()?
}

/// Decode a single TxIn `[tx_hash_bytes, tx_index]` into a human-readable `"hex#index"` string.
fn decode_txin(decoder: &mut minicbor::Decoder<'_>) -> Option<String> {
    let _ = decoder.array().ok()?;
    let hash_bytes = decoder.bytes().ok()?;
    let index = decoder.u32().ok()?;
    let hex: String = hash_bytes.iter().map(|b| format!("{b:02x}")).collect();
    Some(format!("{hex}#{index}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_rational_decimal_basic() {
        assert_eq!(format_rational_decimal(3, 10), "0.3");
        assert_eq!(format_rational_decimal(3, 1000), "0.003");
        assert_eq!(format_rational_decimal(1, 20), "0.05");
        assert_eq!(format_rational_decimal(2, 10), "0.2");
    }

    #[test]
    fn test_format_rational_decimal_integers() {
        assert_eq!(format_rational_decimal(15, 1), "15.0");
        assert_eq!(format_rational_decimal(0, 1), "0.0");
    }

    #[test]
    fn test_format_rational_decimal_execution_unit_prices() {
        assert_eq!(format_rational_decimal(577, 10000), "0.0577");
        assert_eq!(format_rational_decimal(721, 10_000_000), "0.0000721");
    }

    #[test]
    fn test_format_rational_decimal_voting_thresholds() {
        assert_eq!(format_rational_decimal(51, 100), "0.51");
        assert_eq!(format_rational_decimal(67, 100), "0.67");
        assert_eq!(format_rational_decimal(60, 100), "0.6");
        assert_eq!(format_rational_decimal(75, 100), "0.75");
    }

    #[test]
    fn test_parse_tip_result() {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(4).unwrap(); // MsgResult tag
        enc.array(2).unwrap(); // HFC wrapper [1, result]
        enc.u64(1).unwrap(); // HFC success tag
        enc.array(2).unwrap(); // [[slot, hash], block_no]
        enc.array(2).unwrap();
        enc.u64(12345).unwrap();
        enc.bytes(&[0xab; 32]).unwrap();
        enc.u64(100).unwrap();

        let result = parse_tip_result(&buf).unwrap();
        assert_eq!(result.slot, 12345);
        assert_eq!(result.hash, vec![0xab; 32]);
        assert_eq!(result.block_no, 100);
    }

    #[test]
    fn test_parse_tip_result_no_hfc_wrapper() {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(4).unwrap(); // MsgResult tag
                             // No HFC wrapper — parser should handle unwrapped responses
        enc.array(2).unwrap(); // [[slot, hash], block_no]
        enc.array(2).unwrap();
        enc.u64(12345).unwrap();
        enc.bytes(&[0xab; 32]).unwrap();
        enc.u64(100).unwrap();

        let result = parse_tip_result(&buf).unwrap();
        assert_eq!(result.slot, 12345);
        assert_eq!(result.hash, vec![0xab; 32]);
        assert_eq!(result.block_no, 100);
    }

    #[test]
    fn test_parse_epoch_result() {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(4).unwrap(); // MsgResult tag
        enc.array(2).unwrap(); // HFC wrapper [1, result]
        enc.u64(1).unwrap(); // HFC success tag
        enc.u64(42).unwrap();

        let result = parse_epoch_result(&buf).unwrap();
        assert_eq!(result, 42);
    }

    #[test]
    fn test_utctime_to_iso8601() {
        let s = utctime_to_iso8601(2022, 298, 0);
        assert_eq!(s, "2022-10-25T00:00:00Z");
    }

    #[test]
    fn test_parse_u64_result() {
        let mut buf = Vec::new();
        let mut enc = minicbor::Encoder::new(&mut buf);
        enc.array(2).unwrap();
        enc.u32(4).unwrap(); // MsgResult tag
        enc.array(2).unwrap(); // HFC wrapper [1, result]
        enc.u64(1).unwrap(); // HFC success tag
        enc.u64(999).unwrap();

        let result = parse_u64_result(&buf).unwrap();
        assert_eq!(result, 999);
    }
}
